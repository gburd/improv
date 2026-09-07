# Out-of-Core Storage — Investigation & Design Notes (Phase D)

**STATUS: §4's recommended first step is BUILT** (`Model::
measure_dependency_closure` + `ModelStore::load_partial`, wired into CLI
`eval`/`show` — see `.agent/AGENT_STEERING.md` Phase D.2). The rest of this
document (§3a dimension partitioning, §3b DD spilling) remains proposed, not
scheduled. This was originally a one-time investigation task (Phase D of the
post-v0.5.0 plan); it records where the real scale ceiling is, why, what
fully fixing it would concretely require beyond step 1, and why step 1 alone
does not raise the hard whole-model ceiling (only avoids loading data a
bounded operation never touches). Treat §3a/3b as the reference to read
before anyone signs up to build the rest, not as a promise that they will be.

---

## 1. Where the wall actually is

Improv's storage (Mentat/SQLite) is disk-backed and does not itself limit
scale — Mentat is a datom store over SQLite and SQLite comfortably handles
data far larger than RAM. **The bottleneck is entirely in the engine
boundary above storage**: today, using a model requires the *entire* model
resident in one in-memory `Model` struct, and the *entire* set of input
cells copied again into differential-dataflow's in-memory `InputSession`s,
before a single result comes out. There is no partial load, no streaming,
and no paging anywhere in this path. Confirmed by reading the actual code:

- **`storage_mentat::ModelStore::load_model()`**
  (`crates/storage_mentat/src/lib.rs:143`) calls `load_categories`,
  `load_items`, `load_measures`, `load_cells`, `load_views`, `load_meta` in
  sequence, each of which queries Mentat and inserts every row into one
  `Model` (`crates/core_model/src/lib.rs:115`) — there is no filter
  argument, no `LIMIT`, no measure/category subset. `load_cells`
  (`lib.rs:278`) runs one Datalog query returning *every* cell in the store
  and inserts each into `model.inputs: HashMap<(MeasureId, Coordinate),
  Value>` (`core_model/src/lib.rs:125`, an ordinary in-memory `HashMap`).
  `save_model()` (`lib.rs:87`) is symmetric: it transacts every category,
  item, measure, and cell from the in-memory `Model` in one shared SQLite
  transaction. **Whole-model load/save, no windowing, confirmed.**

- **`engine::dataflow::evaluate(model, targets)`**
  (`crates/engine/src/dataflow.rs:396`) takes a `&Model` (already fully
  resident) and, at line 421, does
  `for ((mid, coord), val) in &model.inputs { ... input_cells.entry(*mid)
  .or_default().push(...) }` — copying *every* input cell in the model into
  a `HashMap<MeasureId, Vec<(CoordKey, CellValue)>>`, regardless of whether
  `targets` needs them. Then (line 445) it creates one DD `InputSession`
  per input measure and (line 486) inserts every one of those cells into
  the session before running the dataflow to completion. There is no
  measure-reachability pruning of *input cells* (only derived-measure
  *plan* construction is limited to `targets`'s transitive dependencies via
  `derived_build_order`, `dataflow.rs` — but the input side is not pruned
  at all: a derived measure's plan only references certain input measures,
  but ALL cells of ALL input measures in the model get fed regardless of
  whether they're reachable from `targets`).

- **`session::Engine::new(model, targets)`** (`crates/engine/src/session.rs`,
  `pub fn new` at line 67) is the same shape for the live/incremental path:
  it collects `input_ids` as "every input measure in the model" (comment at
  the call site: *"we over-approximate with all input measures in the
  model"*), then seeds one `Edit` per input cell across the whole model
  into the dataflow before it will answer a single `set()`.

- **DD's own `InputSession`** (checked directly in the vendored
  `differential-dataflow-0.13.0` source,
  `~/.cargo/registry/.../differential-dataflow-0.13.0/src/input.rs`) buffers
  updates in a plain `Vec<(D, T, R)>` (`buffer: Vec<(D, T, R)>` in
  `InputSession`'s definition) — nothing disk-backed, nothing spillable. A
  `grep -ril "disk|mmap|spill|persist"` over the whole DD 0.13.0 source tree
  found nothing. **DD is RAM-resident streaming compute; it has no
  out-of-core story at this pinned version, full stop.**

So: three layers (`load_model`, `evaluate`/`Engine::new`, DD's
`InputSession`) each independently assume "the whole thing fits in RAM," and
none of them offers a hook to load or feed a subset. Fixing any one layer
without the others doesn't help — `load_model` could be made partial, but
`evaluate`/`Engine::new` still copy every input cell of every input measure
present in whatever `Model` they're handed.

## 2. Empirically-verified ceiling (this investigation)

Using the existing `grid_model`/`evaluate` stress pattern
(`crates/engine/tests/stress.rs`, `Time × Product`, `Revenue = Price *
Quantity`), on this dev machine (8 cores, 32GB RAM, ~9GB available under
normal desktop load):

| cells | build | evaluate | peak RSS | notes |
|---|---|---|---|---|
| 1,000,000 | ~2-4.6s | ~22.3s | ~0.90 GB | existing `scale_evaluate_1m` |
| 2,000,000 | ~9.8s | ~57.0s | ~1.58 GB | new `scale_evaluate_2m`, ran clean |
| 5,000,000 | ~23.2-23.7s | ~143-215s (2.4-3.6 min) | ~3.55 GB | new `scale_evaluate_5m`, ran clean, run twice (variance from shared-host load) |
| 10,000,000 | — | **not run** | est. ~6.9 GB | extrapolated only, see below |

Peak RSS scales roughly linearly (~0.66 GB per million cells, measured via
`/usr/bin/time -v` wrapping the compiled test binary directly). Evaluate
wall-clock scales *super*-linearly (best power-law fit over the 1M→5M points
gives exponent ≈1.29, i.e. going from 1M to 5M cells — 5x the data — costs
~8x the time), consistent with `evaluate`'s `Join`/`reduce` operators doing
more-than-linear work as collections grow and this being a single-threaded
`execute_directly` run (no timely worker parallelism engaged — see §3b).

**10M cells was deliberately not run.** Extrapolating from the 1M/2M/5M
points: ~6.9 GB peak RSS and (power-law) 7-18 minutes of wall-clock. This
sandbox had ~9GB "available" RAM shared with other running processes; a
model that size risked evicting page cache and/or triggering the OOM killer
on a machine that isn't dedicated to this test, for a result whose value
(one more data point) didn't justify the risk. The `scale_evaluate_10m` test
is added to `stress.rs`, `#[ignore]`d with an explicit "not yet run, ~7GB
estimated" note, for a future run on a dedicated/big-RAM host.

**Empirically-verified new ceiling: 5,000,000 cells** (up from the
previously-verified 1,000,000), at ~3.5GB peak RSS and ~2.5-4 minutes of
evaluate time — already past the point of being pleasant to wait for
interactively, which is a separate, softer ceiling from the hard OOM wall.
The *practical* ceiling (where it stops being usable, not where it crashes)
is arguably closer to 1-2M cells for anything interactive; 5M is where a
batch `evaluate()` call is still tolerable but a human waiting on it is not.

## 3. What true out-of-core would require

Three directions, each with real tradeoffs. None is a small patch; all are
significant engine-boundary work. This section is deliberately written as
"if we ever do this," not a build plan.

### 3a. Partition by category/dimension, evaluate one slice at a time

Idea: pick a category (say, `Time`), evaluate the model one item's worth of
`Time` at a time (or a batch of items), keep only that slice's input cells
in memory, accumulate/append results, discard, move to the next slice.

**Where it works:** formulas whose result at coordinate `c` depends only on
input cells that also vary along the *same* partition value as `c` — i.e.
element-wise formulas like `Revenue[t,p] = Price[p] * Quantity[t,p]`
partitioned by `Time`: computing `Revenue[t=2025, *]` only needs
`Quantity[t=2025, *]` and all of `Price[*]` (which is `Time`-invariant, so
it's small and can stay resident across every slice). This is the common
case for "wide-but-shallow" formulas (arithmetic across measures at the same
coordinate).

**Where it breaks:** any aggregation whose `OVER` includes the partition
dimension. `SUM(Revenue OVER Time)` needs *every* `Time` value's `Revenue`
for the group being summed, in memory *simultaneously*, before the group's
sum can be finalized — partitioning by `Time` specifically defeats this,
because the whole point of the partition was to never hold all `Time`
values at once. The fix (partial sums accumulated slice-by-slice, only
finalized after the last slice) works for `SUM`/`COUNT` (associative,
one-pass-accumulable) but not for `MIN`/`MAX` naively-accumulable-the-same-
way-but-fine-actually (min/max ARE associative too, so they're fine) — the
real problem is anything that needs an *order* or *all values at once* for
a non-associative reduction (there are none in the current SUM/AVG/MIN/MAX
set, but a hypothetical MEDIAN or PERCENTILE would break this cleanly; AVG
needs sum+count, both associative, so it survives with a two-accumulator
carry). **Rule of thumb: partition along a dimension NOT named in any
formula's `OVER` clause reachable from the measures you care about; if a
formula aggregates over the exact dimension you partitioned by, this
approach needs a per-formula accumulator strategy, not a generic one.** That
per-formula analysis is real compiler work (the compiler already computes
`DimensionSpec.over` per aggregation — this info exists, but nothing today
uses it to decide partition safety).

**Verdict:** works well for a *known* access pattern with a *chosen* safe
partition dimension; does not generalize to "partition arbitrarily and it
just works" — every derived measure would need to be checked against the
chosen partition dimension, and a single "unsafe" formula in the target set
forces falling back to whole-slice-for-that-dimension anyway.

### 3b. DD's own incremental/spilling story

Checked directly against the vendored source
(`~/.cargo/registry/.../differential-dataflow-0.13.0`): **no**. `grep -ril
"disk|mmap|spill|persist"` over the whole crate found nothing; `InputSession`
(`src/input.rs`) is a plain in-memory `Vec` buffer; traces/arrangements
(`src/trace/`) are in-memory representations (spine/batch structures) with no
disk backing at this version. Differential dataflow (and timely underneath
it) is designed for RAM-resident streaming/incremental compute — its value
proposition is "recompute only deltas fast," not "handle data bigger than
RAM." Later DD/timely versions (see the "columnar" `timely_communication`
0.18 line mentioned in `AGENT_ENGINE_STEERING.md` §2) trend toward more
memory-efficient in-RAM representations (columnar layouts), which would help
the constant factor (today: `Vec<(CoordKey, CellValue)>`, `CoordKey` is a
`Vec<(u32,u32)>` — one heap allocation per cell, which is why RSS is ~0.66GB
per million *simple 2D* cells rather than the ~24 bytes/cell the raw data
would need) but not the fundamental "must fit in RAM" constraint. **Also
worth noting:** `evaluate()` currently runs via
`timely::execute::execute_directly` (single-threaded, one worker) rather
than a multi-worker `timely::execute::execute` — so today's numbers don't
even reflect DD's own intra-machine parallelism, which is a separate,
easier, and orthogonal win (more cores, not more RAM) worth chasing before
or alongside anything below.

**Verdict:** no help from upstream at the pinned version; do not expect a
disk-spilling DD operator to appear and solve this — if DD grows one
upstream, re-evaluate then, but don't design around a feature that doesn't
exist in `=0.13.0` today (and the version pin is deliberately delicate,
see `AGENT_ENGINE_STEERING.md` §2 — bumping it is its own risky project).

### 3c. Change the storage-to-engine boundary: hand the DD graph a *window*, not the whole model

Idea: keep Mentat/SQLite exactly as-is (already disk-backed, already fine
at any scale) but stop handing `evaluate`/`Engine::new` the *entire*
`Model`. Instead, load (and feed to DD) only the categories/items/measures/
cells actually reachable from whatever set of measures the caller is
currently interested in — a `Model::load_partial(store, measure_ids)` that
walks each requested derived measure's formula dependencies (the compiler
already computes `referenced_measures()` — see
`crates/engine/src/dataflow.rs`'s `derived_build_order`) back to its input
measures, and loads/feeds *only those input cells*, not every input cell of
every measure in the store.

**Why this is the most promising of the three:** it requires zero changes
to `engine::dataflow`/`engine::session`/DD internals — `evaluate(model,
targets)` already only *builds plans* for `targets`'s transitive derived
dependencies (`derived_build_order`); the gap is purely that
`load_model()`/the input-cell-feeding loop in `evaluate` (line 421) and
`Engine::new` (the `input_ids` collection) don't apply the same pruning to
*input cells*. **This is a storage-layer change (a new/parameterized load
function alongside the existing whole-model `load_model`) plus a small
change to how `evaluate`/`Engine::new` decide which input measures to feed
— not an engine-internals rewrite.** It also matches how the GUI/TUI
already work: a pivot view only ever displays a bounded slice of a
potentially huge model (see the GUI's virtualized rendering,
`AGENT_STEERING.md`'s Phase 5 entry — "a row axis of millions of lines
never materializes"); the GUI/TUI/CLI/server already never *need* the
whole model loaded to answer one query. Only the storage/engine hand-off
currently forces it to be loaded anyway.

**Tradeoffs:**
- A single "give me everything" operation (e.g. a full-model export,
  `cargo run -- export`, or a `SUM(Revenue OVER Time)` grand total across a
  billion-cell model) still needs the whole reachable set in memory — this
  approach does not make an operation whose *definition* touches the whole
  model cheap; it only avoids loading data the operation *doesn't* touch.
  For a model with a billion cells across many independent measures, most
  single operations (edit one measure, view one pivot slice, evaluate one
  derived measure with a bounded dependency set) touch a bounded subset —
  but "give me the grand total of everything" is inherently whole-model
  work no architecture change avoids, short of a from-scratch materialized-
  aggregate/OLAP-cube design (much bigger scope, not analyzed further here).
- Requires the storage layer to answer "which cells does measure set X
  need" efficiently — today `load_cells` (`storage_mentat/src/lib.rs:278`)
  runs one Datalog query for *all* cells; a partial load needs a
  measure-filtered query (`:where [?e :cell/measure ?m] [?m :measure/id
  ?mid] [(contains? #{...} ?mid)] ...` or equivalent), which Mentat should
  support without schema changes (cells are already keyed by measure) —
  but this is unverified against Mentat's actual query planner performance
  at scale (a `contains?`-style filter over a huge cell table might not use
  an index well; would need to be checked, not assumed).
- Session/incremental editing (`session::Engine`) would need a policy for
  "what happens when a user edits a cell in a measure that isn't loaded" —
  either disallow it (require a reload with a wider measure set) or make
  `Engine` able to grow its loaded set on demand. Not designed here.
- Does not address `save_model`'s whole-model save — a billion-cell model
  still gets rewritten via one Mentat transaction on every save, which is a
  separate crash-safety-already-solved (see `AGENT_DATABASE_CONNECTIVITY.md`
  §9) but scale-unsolved problem; likely fine because Mentat/SQLite already
  streams the transaction rather than buffering it all in a `Model` twice,
  but this was not verified as part of this investigation and would need
  its own check before being relied upon at billion-cell scale.

## 4. Recommendation

**Pursue 3c (window the storage-to-engine boundary) first, if/when this
work is ever scheduled** — it is the only one of the three that:
- requires no change to DD or the pinned version (avoiding the delicate
  version-pin risk called out in `AGENT_ENGINE_STEERING.md` §2),
- requires no change to `engine::dataflow`/`engine::session` internals
  (which this task was explicitly scoped to leave untouched, and which are
  a tested, working core not to be put at risk for a speculative feature),
- matches an assumption the GUI/TUI/CLI/server *already* make (operations
  touch a bounded subset of a model, not the whole thing) — so it's fixing
  the one layer (`storage_mentat` load) that doesn't yet honor that
  assumption, rather than introducing a new one.

3a (dimension partitioning) is a real technique but only safe per-formula,
and the analysis to prove safety for a given target measure set is
non-trivial compiler work; it's a *complement* to 3c for the "one massive
aggregate over a huge dimension" case that 3c alone doesn't fix, not a
replacement for it. 3b (waiting for DD to grow disk-spilling) is not
actionable at the pinned version and shouldn't be designed around.

**Concrete first step — BUILT:** `ModelStore::load_partial(&mut self,
measure_ids: &[MeasureId]) -> Result<Model>` in
`crates/storage_mentat/src/lib.rs`, alongside (not replacing) the existing
`load_model()`. It: (1) computes the transitive measure dependency set for
`measure_ids` via `Model::measure_dependency_closure` (a pure walk over
`Formula::referenced_measures()` and external-call `arg_measures`, added to
`core_model`), (2) loads categories/items/measures/views/meta in full (their
volume scales with model *shape*, cheap regardless of data size) but filters
`load_cells` to only the closure's measures, (3) returns an ordinary `Model`
— `engine::dataflow::evaluate`/`engine::session::Engine::new` needed **zero
changes**, confirmed by a test asserting identical `evaluate()` output over a
full load vs. a partial load of the same derived measure. Wired into CLI
`eval`/`show` (both operate over one measure's dependency closure); `stream`
stays on `load_model()` since it accepts arbitrary-measure edits from stdin,
which needs the full measure set resident — not a safe windowing candidate.

This lets a model with a billion cells across many measures work fine for any
single operation whose formula-dependency closure is a bounded subset of the
whole — the common case for "open the CLI/TUI/GUI/server and look at one
measure or edit one cell." It is NOT a general disk-spilling engine and does
NOT help a genuine "aggregate literally everything" operation (`stream`'s
arbitrary edits, a full-model export, a grand total over a huge dimension) —
see §3a/3b below for what THOSE would still require. A real, useful,
honestly-scoped win, not a promise of unbounded scale.

## 5. Non-goals / explicitly out of scope

- No engine/DD rewrite happened. `crates/engine/src/dataflow.rs` and
  `crates/engine/src/session.rs` are unchanged — §4's first step
  deliberately required none, and that remains true after building it.
- §3a (dimension partitioning) and §3b (waiting on DD to grow disk-spilling)
  remain **not built, not scheduled**. See `.agent/AGENT_STEERING.md` Phase
  D.2 for the live status of the whole out-of-core line item.
- No attempt to design a full OLAP-cube/materialized-aggregate system for
  the "aggregate literally everything in a billion-cell model" case — that
  is a materially bigger, separate design problem than the common-case
  windowing this document recommends starting with.
