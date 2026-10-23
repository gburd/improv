# Engine heap profile: memory per cell (B7)

Road-to-1.0 task B7: the engine held about 0.8 GB per million cells
(v0.10.0 benchmark §3a). This note records where the bytes were, what
changed, and the out-of-core decision (B7c).

## Method

- Box: EC2 c7i.12xlarge (48 vCPU, 92 GB), Debian 13, rustc 1.98, release
  builds, one process per size.
- Shape: the stress grid, `Revenue = Price * Quantity` with `Price[Product]`
  and `Quantity[Time, Product]` fully populated, 1000 × 1000 (1M cells) and
  5000 × 1000 (5M cells).
- Tools: heaptrack 1.5 (`heaptrack_print --flamegraph-cost-type peak`,
  folded stacks grouped by the DD operator or engine function on the
  stack) for the peak, and `/usr/bin/time -v` for peak RSS.
- Two harnesses:
  - `tests/stress.rs` `scale_evaluate_{1m,5m}`: model build plus
    `evaluate`, as shipped. Its peak RSS is the v0.10.0 benchmark's
    "per M cells" figure.
  - `tests/probe.rs` `grid` (new): the same model, with the live heap
    (`mallinfo2`) after each phase: model, `evaluate` (result held, then
    dropped), `Engine::new` (snapshot held, then dropped). It reports
    bytes per cell for each.

## Where the bytes were (before)

Peak heap of `Engine::new` at 5M cells (`probe grid`, 5.24 GB peak):

| Holder | MB | Per cell |
|---|---:|---:|
| DD arrangements: the two `align` join inputs, and the collapse `reduce`'s input **and** output arrangements, `seal`ed batches | 1,812 | 380 B |
| The live engine's snapshot (`inspect` → `HashMap<CoordKey, CellValue>`) | 684 | 143 B |
| `Model.inputs` (`HashMap<(MeasureId, Coordinate), Value>`, a `BTreeMap` per coordinate) | 1,016 | 213 B |
| The worker's input map (`inputs: HashMap<MeasureId, MeasureValues>`, kept to retract old values) | 456 | 96 B |
| Join outputs and batcher chains in flight (`map_times`, `Merger`) | 343 | 72 B |
| `reduce`'s output batches and their time `Antichain`s | 512 | 107 B |
| `union_keys` output keys spilled to the heap | 153 | 32 B |

`evaluate` alone at 5M cells (`stress`, 2.98 GB peak): the result map 684 MB,
the reduce arrangements 606 MB, the join input arrangements 382 MB, the
spilled keys 153 MB, plus the test's own model 1,016 MB.

Text (`CellValue::Text`) was not a holder: these grids are numeric, and a
`CellValue` is 24 bytes, inline (a `Box<str>` would be 24 too). Interning
text would not have moved any of these numbers, so it was not done.

## What was wrong

1. **A join's measure was collapsed by a `reduce`.** `build_derived` wraps
   every measure that is not a map of an input in `reduce` (keep each
   positive value), so a broadcast `Price * Quantity` arranged its 5M rows
   twice more (the reduce's input and output traces) on top of the join's
   two input arrangements. A join of one-value-per-key collections is
   already one value per key (a cell's coordinate names every one of its
   measure's categories, so a joined row's coordinate fixes the pair it
   joined): the collapse kept a copy for nothing.
2. **The broadcast joined twice.** The compiler lowers `a op b` over
   different categories to `MapBinary(op, Join{a, b}, b)`. The dataflow
   built the `Join` (`a` ⋈ `b`, then drop `b`'s value) and then joined its
   5M rows with `b` again on the full key, arranging the 5M-row side a
   second time.
3. **Keys spilled.** `SmallVec::from_vec` keeps a `Vec` whose *capacity*
   exceeds the inline size on the heap even when its length fits.
   `union_keys` allocates `a.len() + b.len()` (4 for a 2+2 union of length
   2), so every grid key was a 32-byte heap allocation.
4. **`evaluate` grew its result map while the dataflow was alive**, so the
   peak held every arrangement plus the map's old and new table.

## What changed

- `dataflow::functional` now also counts a join (or `MapBinary`) of
  functional operands, and a single-result reduction (which pushes one
  value per group), as one value per key, so neither is collapsed.
  Without the collapse, a measure's deltas within a round arrive in no set
  order (a join can send `+new` before `-old`), so the live engine buffers
  each round's deltas and applies them consolidated
  (`session::settle`), retractions first; a delta that cancels marks no
  measure dirty. The seed round (time 0) only asserts and applies
  directly, so it is never buffered.
- `MapBinary(op, Join{a, b}, b)` builds one join of `a` and `b`
  (`dataflow::broadcast_of`).
- `inline_key` shrinks a key into the inline buffer when it fits;
  `project` and `union_keys` use it.
- `evaluate` collects rows during the run and builds the result maps once
  the dataflow has finished.

Tests: `session::tests::uncollapsed_joins_follow_every_edit` (joins and
reductions, uncollapsed, against `evaluate` after sets, re-sets, clears and
clears-then-sets in one batch), `settle_applies_a_rounds_deltas_in_any_order`,
`inline_key_spills_only_what_does_not_fit`. Goldens, properties,
determinism and FIXPOINT suites are unchanged and green.

## After

| Measure (release, this box) | Before | After | Change |
|---|---:|---:|---:|
| `Engine::new` heap per cell, 1M (`probe grid`) | 747 B | 322 B | −57 % |
| `Engine::new` heap per cell, 5M | 732 B | 269 B | **−63 %** |
| `Engine::new` peak heap, 5M (heaptrack) | 5.24 GB | 2.72 GB | −48 % |
| `probe grid` peak RSS, 5M (model + evaluate + engine) | 6.13 GB | 3.59 GB | −41 % |
| `stress` `scale_evaluate_1m` peak RSS | 0.79 GB | 0.54 GB | −32 % |
| `stress` `scale_evaluate_5m` peak RSS | 3.38 GB | 2.35 GB | −30 % |
| `stress` `scale_evaluate_5m` peak heap (heaptrack) | 2.98 GB | 2.01 GB | −33 % |
| `evaluate`, 1M / 5M cells | 2.40 / 13.5 s | 0.87 / 4.48 s | 2.8× / 3.0× faster |
| `Engine::new`, 1M / 5M cells | 2.45 / 14.2 s | 1.13 / 5.95 s | 2.2× / 2.4× faster |
| Single-cell edit, 5M (median) | 0.41 ms | 0.30 ms | |

The stress figure includes the test's own `Model` (1 GB at 5M: 213 B per
cell in `Model.inputs`), which is not the engine's. Per engine cell at 5M,
the cut is 63 %.

What is left in `Engine::new` at 5M (2.72 GB peak): the snapshot 684 MB,
`Model.inputs` 1,016 MB (the caller's), the worker's input map 456 MB, and
the join's two input arrangements 363 MB. The remaining copies are one per
holder with a reason: the snapshot is what a reply hands out; the
worker's input map is what a set retracts (a DD trace could serve it, at
the cost of a cursor walk per edit); the join must arrange both sides.

No regressions on the other probes (median edit latency, peak RSS, before
→ after): `SUM(X OVER W)` 1M items 0.67 → 0.65 ms, 1.07 → 1.07 GB;
`RANK(X OVER W)` 1M 1.48 → 1.51 s, 2.25 → 2.20 GB; `PREV` 1M 0.04–0.06 →
0.05–0.08 ms (three runs of 40 edits each); 20k-measure fan 130 → 134 ms,
1.22 → 1.17 GB; PageRank 10k/1M edges: engine 1,000 → 769 B per edge.

## Out of core (B7c): deferred past 1.0

Decision: **defer**. Reasons:

- At 269 B per engine cell, a 92 GB box holds about 300M cells in the live
  engine, and the 61 GB benchmark box about 200M. That covers the 1.0
  target models by two orders of magnitude.
- `storage_mentat::ModelStore::load_partial` (§4 of
  `AGENT_OUT_OF_CORE_DESIGN.md`, built) already bounds an operation to its
  measures' dependency closure, which is the common case for the CLI, TUI,
  GUI and server.
- §3a (dimension partitioning) is only safe per formula: any aggregation
  `OVER` the partition dimension needs a per-function accumulator, and the
  order-dependent reductions (`RANK`, `PREV`, windows, FIXPOINT) need the
  whole dimension at once. Building it means a partition-safety analysis
  in the compiler and a second evaluation path that every golden and
  differential test would have to cover. That is a post-1.0 project.
- §3b (DD spilling) is not available at the pinned DD 0.13.0
  (`AGENT_ENGINE_STEERING.md` §2).

Revisit when a user model approaches 100M live cells, or the server needs
several large models resident at once.
