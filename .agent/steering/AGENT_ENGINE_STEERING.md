# AGENT_ENGINE_STEERING.md
Engine Steering Document for Improv (Differential-Dataflow Engine)

## 1. Purpose

This document defines the architecture, data model, dependency graph,
recalculation strategy, operator pipeline, concurrency model, performance model,
and testing strategy for the Improv computational engine (`improv_engine`).

The engine is responsible for:

- Compiling formulas (`Formula → TypedExpr → PlanNode`)
- Managing dependencies between measures
- Maintaining incremental recalculation via differential dataflow
- Broadcasting and aligning across dimensions
- Materializing multidimensional view slices
- Providing fast, deterministic results to the interfaces

This is the authoritative guide for all engine development.

---

## 2. Mandated Substrate and Version Pin

The engine is built on **differential dataflow** (`differential-dataflow`) over
**timely dataflow** (`timely`). This is a hard requirement from the source
design, not a choice to revisit.

**Version pin (delicate — do not bump blindly):**

```toml
differential-dataflow = "=0.13.0"
timely               = "=0.13.0"
```

Rationale, so a future maintainer does not "helpfully" upgrade:

- **DD 0.13.7** pulls timely 0.19 / timely_communication 0.18 (a columnar line)
  that does **not** compile on rustc 1.97, the project toolchain.
- **DD 0.12.0** compiles but aborts on a `merge_batcher` `get_unchecked` UB under
  debug assertions.
- **DD 0.13.0 + timely 0.13.0** is the pre-breakage pair that both compiles and
  runs; it is the validated combination.

Toolchain floor: **rustc 1.97+** (matches the Mentat workspace).

### 2.1 Key-Type Viability (the Phase 1 gate)

Differential dataflow keys must be fixed-shape, `Ord` + `Hash`, and
exchange-safe; diffs need well-behaved arithmetic. The model's `Coordinate`
(dynamic-arity `BTreeMap`) and `Value` (carries `f64`, not `Ord`/`Eq`) do not
satisfy this directly. A spike had to prove a workable encoding before the engine
could be built on DD.

**Gate status: CLEARED** (`engine` test `dd_revenue_is_incremental`). The viable
encoding, which all engine code must follow:

- **DD key = serialized coordinate** as a sorted list of
  `(category_id, item_id)` pairs — fixed-shape, `Ord + Hash`, exchange-safe.
  `CoordKey` is `SmallVec<[(u32, u32); 3]>` (re-exported as
  `improv_engine::smallvec`): up to three categories inline. A `Vec` key
  was a heap allocation per cell, and malloc/free and `hash_one(Vec<..>)`
  led every profile; inline keys halve `evaluate` on the 1M/5M grids
  (4.3 → 2.2 s, 27.7 → 12.7 s) and cut peak RSS 10–15 %. Capacity 2 is
  slightly leaner on 2-D grids but spills every 3-D key (the 3-D stress
  case is 0.83 s at 2, 0.48 s at 3, 0.77 s with `Vec`); 4 costs memory
  for no speed. Look keys up by slice (`map.get(&[(c, i)][..])`):
  `SmallVec` borrows as `[T]`, not as `Vec`.
- **Numeric value = `f64::to_bits()` as `u64`** carried in data position (not as
  the diff), so equality/order are well-defined.
- **Diff = `isize` multiplicity**; a final `reduce` collapses to one value per
  key.

If DD ever proves unable to express a needed operator cleanly, the fallback is a
hand-rolled incremental dependency-graph evaluator **behind the same engine
API** — but that is a last resort, and the gate above is already cleared.

---

## 3. Data Model

### 3.1 Categories and Items

A category is a dimension of analysis (Time, Product, Region, Scenario); each
has items (Time → 2025, 2026). Categories and items are owned by `core_model`.

### 3.2 Measures

Measures are named quantities that declare their dimensionality:

```text
Revenue[Time, Product]
Price[Product]
Quantity[Time, Product]
```

Measure kinds:

- **Input** — user-entered cell values
- **Derived** — computed from a formula

### 3.3 Coordinates and Values

A coordinate is a tuple of `(category, item)` pairs. Values are carried in the
dataflow as `engine::CellValue` (`Num(f64 bits)` / `Bool` / `Text` / `Err`),
which is `Ord + Eq + Hash + serde` (timely 0.13 exchanges via serde). Numbers
are the fast path; comparisons and `NOT`/`AND`/`OR` yield `Bool`, so **boolean
and text derived measures work end-to-end** (e.g. `Hot = Price > 15` →
`Bool`). `Date` is not yet representable in the lane (skipped on input).

---

## 4. Dependency Graph

The engine builds a directed acyclic graph of measure dependencies:

- Nodes: measures
- Edges: formula references

```text
Revenue → Price
Revenue → Quantity
```

Used for topological ordering, incremental updates, and error propagation.

### 4.1 Construction

1. Parse formula (from `core_model` / compiler)
2. Extract measure references
3. Validate types and dimensions
4. Insert edges
5. Detect cycles (rejected with an error)
6. Produce topological order

`evaluate(model, targets)` builds derived measures in topological dependency
order, so a derived measure may reference another derived measure
(multi-layer — implemented and tested).

---

## 5. Formula Compiler

Located in `compiler.rs`. Pipeline: `Formula → TypedExpr → PlanNode`.

### 5.1 Type Inference

Validates that operators and functions receive well-typed arguments (Number,
Text, Boolean, Date).

### 5.2 Dimension Inference

Computes each expression's dimensionality, inserts a **Join** where broadcast is
needed, and an **Aggregate** for `SUM` / `AVG` / `MIN` / `MAX`.

### 5.3 Aggregation Convention

An aggregation is a `Call` taking one measure-reference argument whose
`DimensionSpec.over` names the collapsed categories. Function ids:
`1 = SUM`, `2 = AVG`, `3 = MIN`, `4 = MAX`.

### 5.4 Plan Nodes

- `InputMeasure`
- `MapUnary`
- `MapBinary`
- `Join`
- `Aggregate`

---

## 6. Differential-Dataflow Evaluation

Located in `dataflow.rs`.

### 6.1 Collections

Each measure is a DD collection keyed by the serialized coordinate (see §2.1):

```text
Collection<CoordinateKey, ValueBits>
```

### 6.2 Deltas

On an input change, only the delta propagates: operators recompute only affected
coordinates, and downstream measures update incrementally. The spike's delta
round proved unaffected cells are not recomputed.

The live engine hands the round's consolidated change back
(`Engine::apply_delta`: per measure, each changed cell's old and new value,
recorded as `settle` applies the round), so a caller never needs to hold or
re-diff whole snapshots. `apply` still returns the snapshot; a caller that
keeps one across an edit makes the engine copy each changed measure (the
snapshot is copy-on-write), so callers drop it first or use `apply_delta`.

### 6.3 Operators

`Map` (unary/binary), `Join` (broadcast/alignment), `Reduce`/`Aggregate`
(SUM/AVG/MIN/MAX).

### 6.4 evaluate()

`evaluate(model, targets)` compiles each derived measure, builds the DD graph,
feeds numeric input cells, runs, and returns computed values. It is verified
against the canonical Time×Product revenue results (1000 / 1000 / 1200 / 1600).

> `evaluate` runs once end-to-end. A **live incremental edit API** (`session.rs`
> `Engine`) builds the graph once on a dedicated worker thread and applies
> input-cell edits as deltas (`Engine::set` / `clear` / `apply`), recomputing
> only affected coordinates without a rebuild. Structural changes (new
> measures/formulas) require a new `Engine`. **DONE** — verified by the session
> tests (initial snapshot, affected-only update, broadcast, cell clear).

---

## 7. Broadcasting and Alignment

- If a measure has fewer dimensions than the expression context, it is broadcast
  across the missing dimensions.
- If dimensions cannot be aligned, a dimension error is produced.

```text
Price[Product] * Quantity[Time, Product]   -- Price broadcast across Time
```

---

## 8. Aggregation

`SUM` / `AVG` / `MIN` / `MAX` reduce dimensionality over a named category:

```text
TotalRevenue[Product] = SUM(Revenue OVER Time)
```

Aggregation must name the collapsed category and operate on a measure
collection.

**Two-level reduce** (`reduce/two_level.rs`). DD's `reduce` re-runs a key's
whole input when one value changes, so one edit under `SUM(X OVER W)` cost
O(|W|) (0.6 s at 1M items). The decomposable reductions (SUM … XOR,
COUNT, COUNTA; one input, no parameters) over a single category of more
than `FLAT_MAX` = 1,024 items reduce in two stages: rows keyed by
`(group, item id >> 10)` reduce to a `Part` (count, first error by
position, a text/bad flag, a true/number count, the float accumulator),
and the parts merge per group in block order. An edit re-reduces one block
(≤ 1,024 rows) and the group's parts (≤ |W|/1,024). Blocks are by item
*id*, not position, so an engine fed appended items (`Engine::serves`)
blocks them as a fresh engine would. SUM and AVERAGE are exact
(`exact.rs`: a fixed-point superaccumulator; a block's `Part` keeps its
exact sum, the merge adds them and rounds once), MIN and MAX order -0.0
below 0.0, and the counts and AND/OR/XOR are integers, so all of them
agree flat and two-level bit for bit, and `serves` ignores which path a
plan took: a category that grows past 1,024 items keeps the engine
(B4(b)). PRODUCT and SUMSQ are a fixed two-level fold, not bit-equal to
one left fold, so for them `serves` keeps the bit; at ≤ 1,024 items
nothing changes. The FIXPOINT host evaluator calls the same
`two_level::reduce`, so host and dataflow steps agree bit for bit.
`dataflow::mark_sparse` (called by `evaluate` and `Engine::build`) sets
an `Aggregate`'s `flat` bit where its input has at most `FLAT_MAX` cells
per group on average: an edge list's `SUM(W OVER Dst)` would otherwise
keep two arrangements of near-empty blocks (B8). PRODUCT and SUMSQ are
never marked; `serves` ignores the bit, since it only moves cost.

**Windows** (`reduce/window.rs`, `OVER T[from..to]`). A group's rows come
sorted by position, so each row's window is a run of rows whose two ends
only move forward: two pointers find them all in O(n). SUM … XOR slide a
running state over the run (`exact::WindowSum`, an exact sum that terms
leave as well as join; a monotone deque for MIN/MAX; true counts);
PRODUCT and SUMSQ, left folds, slide only when the window starts at
FIRST. Any other reduction is called on each window's rows. `scan`, the
per-row definition, is the reference a property test holds the kernel to,
bit for bit. The whole group still re-runs on an edit (O(n), 1.9 s at 1M
items); blocking the window category as the two-level reduce does is the
next step.

---

## 9. Scenarios (design target)

Scenarios are modeled as a special category (`Scenario → Base, Optimistic,
Pessimistic`) so that overrides, layering, pivoting, and comparison reuse the
existing dimensional machinery. This is a design target that rides on the same
engine primitives; there is no separate scenario subsystem.

---

## 10. View Materialization

Views are 2D slices materialized from measure collections:

- **Pivot** — assign categories to rows / columns / pages / filters
- **Slice** — extract a 2D slice from a higher-dimensional measure
- **Aggregate** — SUM/AVG/MIN/MAX over collapsed categories

Interfaces request a slice for a `View` and render it.

---

## 11. Concurrency and Determinism

The engine uses timely/DD's dataflow execution, which parallelizes operators.
Regardless of parallelism, results must be **deterministic**: identical inputs
yield identical outputs, bit-for-bit, independent of insertion order. This is a
tested invariant (see §14), not an aspiration.

---

## 12. Error Model

Errors: type errors, dimension errors, cycle errors, and runtime errors. Errors
are values that propagate through the dependency graph to dependent measures and
surface in the interfaces.

---

## 13. Performance Model

Goals:

- Sub-millisecond recalculation for small models
- Sub-100ms for medium models
- Sub-500ms for large models

Techniques: incremental recalculation (only deltas), topological scheduling,
DD operator parallelism, and cached view slices.

**Per-measure cost.** Every operator of every tracked measure is a timely
operator, and an edit round moves every frontier, so a round costs
progress tracking (`propagate_pointstamps`, its `BinaryHeap`) per operator
even where no data moves. Two things keep the operator count down:
`build_derived` does not collapse (a `reduce`, so an arrangement) a
measure whose plan is a map of an input or of another such measure
(`dataflow::functional`: inputs hold one value per key, and maps keep the
order the session sent, so the snapshot's +1/-1 rule still holds), and the
worker attaches one probe to the concatenation of every output rather than
one per measure. At 20,000 `Mk = X + k` measures an edit went from 734 ms
to 131 ms, and memory from 3.4 GB to 1.4 GB.

**Per-cell memory (B7, `docs/reviews/2026-10-23-engine-heap-profile.md`).**
`functional` also counts a join or a binary map of functional operands,
and a single-result reduction, so neither is collapsed: at 5M cells of
`Price * Quantity` the collapse was two more arrangements of every cell
(1.2 GB). Uncollapsed deltas within a round come in no set order (a join
may send `+new` before `-old`), so the worker buffers every round after
the seed and applies it consolidated, retractions first
(`session::settle`). `MapBinary(op, Join{a, b}, b)` builds one join
(`dataflow::broadcast_of`), and keys go through `inline_key` (a `Vec`
whose capacity exceeds 3 spills even at length 2). Engine heap per cell at
5M: 732 → 269 B. The holders left are the snapshot, the worker's input
map, the join's two input arrangements and the caller's `Model.inputs`.

**FIXPOINT memory (PageRank, live heap via `mallinfo2`, `tests/probe.rs`
`pagerank`).** At 10k nodes / 1M edges, the Model holds 250 B per edge
(`Model.inputs`: a `BTreeMap` coordinate plus the hash entry), and the
engine about 1.0 kB per edge: the worker's own map of the input cells
(a set retracts the old value from it; FIXPOINT steps read it) and the
input session's pending batch about 190 B, the dataflow build (`Out = SUM(W OVER Dst)`, `M =
W / Out` and the FIXPOINT's first iterate, whose joins arrange `W` and `M`
again keyed by the shared category) about 440 B, and the host evaluator's
step about 260 B (`M`'s cells as a `Vec`, the `M OVER Src` join index, and
the reduction's laid-out rows and sources). `Out` alone costs 290 B per edge
and `M` 720 B (it joins `W` to `Out`). The engine used to keep two more
copies of every input cell: `Engine::current` (a shadow map nothing read;
the worker keeps the values it retracts) and the seed `Vec`, held for the
worker's life by its closure. Both are gone: 1.29 kB → 1.06 kB per edge at
1M edges, 1.21 → 1.03 kB at 10M (peak RSS 16.8 → 15.0 GB). What is left is
one copy per operator that must arrange (DD arranges each join input and
each reduce input), so the next step is structural: share one arrangement
of `W` between `Out`'s reduce and `M`'s join (`arrange` once, `join_core` /
`reduce_core` on it), and let the host step read `M` from that arrangement
instead of its own `Vec` + index. Neither is a few-line change; it is not
done. A reduction's row (`dataflow::AggRow`, what the reduce arranges) is
inline (`SmallVec` positions and values: four categories, one value) for
the common one-input reduction: 556 → 501 B per edge at 10M edges, and the
same order as the `Vec`s it replaced, so results are bit for bit as before.

---

## 14. Testing Strategy

- **Unit** — parsing, type inference, dimension inference, plan construction.
- **Determinism** — the Time×Product revenue oracle with known results
  (1000/1000/1200/1600); bit-for-bit reproducibility and insertion-order
  independence.
- **Property** (`proptest`) — compiler / plan invariants.
- **Fuzz** (`cargo-fuzz`) — the formula parser (no panics on arbitrary input).
- **Stress** (`#[ignore]`-gated) — large models (100k+ cells) exercising
  incremental recalculation; kept out of the default fast suite.

See `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md` for the full strategy.

---

## 15. Deferred Follow-ups (Phase 1)

Explicitly out of the numeric core, tracked for later:

- Non-numeric derived values
- General `FuncCall` (beyond the aggregation convention)
- Standalone / broadcast literals
- A live incremental edit API on top of `evaluate`

---

## 16. Definition of Success

The engine succeeds when recalculation is instantaneous and incremental,
broadcasting and aggregation are correct, results are deterministic, and view
slices materialize quickly.

---

## 17. Document Index

Part of the full steering set:

- `AGENT_MASTER_STEERING.md`
- `AGENT_GUI_STEERING.md`
- `AGENT_ENGINE_STEERING.md`
- `AGENT_STORAGE_STEERING.md`
- `AGENT_FORMULA_LANGUAGE.md`
- `AGENT_DATABASE_CONNECTIVITY.md`
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md`
- `STEERING_SYSTEM_OVERVIEW.md`
- `AGENT_OUT_OF_CORE_DESIGN.md` — Phase D scale investigation (proposed, not
  scheduled)
