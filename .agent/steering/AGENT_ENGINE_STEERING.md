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

- **DD key = serialized coordinate** as a sorted `Vec<(u32, u32)>` of
  `(category_id, item_id)` pairs — fixed-shape, `Ord + Hash`, exchange-safe.
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

A coordinate is a tuple of `(category, item)` pairs. Values may be Number, Text,
Boolean, Date, or Error. The v1 numeric core operates on Number; non-numeric
derived values are a deferred follow-up (see §12).

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
