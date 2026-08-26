# Improv — Agent Steering

Full design rationale lives in `IMPROV.txt` (the source conversation). This file
is the actionable steering: architecture, constraints, and phase status.

## Mission

A cross-platform, standalone multidimensional spreadsheet inspired by Lotus
Improv / Quantrix, in Rust, with incremental recalculation and separation of
structure, logic, and data. First iteration: TUI (VisiCalc-like) + CLI + server;
GUI later.

## Mandated substrate (from IMPROV.txt)

- **Computation engine: differential dataflow.** `differential-dataflow` +
  `timely` (TimelyDataflow crates). This is a hard requirement in the design
  (52 references; "uses differential dataflow at its core" is in the v1
  definition of done). Pinned in the workspace `Cargo.toml`.
- **Storage: the embedded SQLite Mentat fork** at `../mentat` (Datomic-style
  datom store). NOT Postgres. Model persists as categories/items/measures/cells
  as datoms (see IMPROV.txt "Mentat schema (steering version)").
- Formula language: dimension-aware DSL + (later) controlled-English
  bidirectional translation.

## Constraints & Contracts (the load-bearing risks)

1. **DD key-type viability (GATE for Phase 1).** `Coordinate` is a `BTreeMap`
   (dynamic arity) and `Value` carries `f64` (not `Ord`/`Eq`). Differential
   dataflow keys must be fixed, `Ord`+`Hash`, and diffs need well-behaved
   arithmetic. Before building the engine on DD, a spike MUST prove a workable
   encoding (e.g. per-measure fixed-arity key tuples, or a canonical serialized
   `Coordinate` key; values as a separate non-diffed payload). If DD can't be
   made to fit cleanly, fall back to a hand-rolled incremental dependency-graph
   evaluator behind the same engine API. Do not write the whole engine before
   this spike passes.
   - **STATUS: GATE CLEARED.** `crates/engine` spike `dd_revenue_is_incremental`
     computes `Revenue = Price * Quantity` incrementally and passes, incl. a
     delta round proving unaffected cells don't recompute. Viable encoding:
     **DD key = serialized coordinate `Vec<(u32,u32)>`** (sorted, `Ord+Hash`,
     exchange-safe), **numeric value = `f64::to_bits()` as `u64`** in data
     position, diff = `isize` multiplicity, `reduce` collapses to one value/key.
   - **Version pin (delicate, do not bump blindly):** `differential-dataflow =
     "=0.13.0"`, `timely = "=0.13.0"`. DD 0.13.7 pulls timely 0.19 /
     timely_communication 0.18 (a columnar line that does not compile on rustc
     1.97); DD 0.12.0 compiles but aborts on a `merge_batcher` `get_unchecked`
     UB under debug assertions. 0.13.0 + timely 0.13.0 both compiles and runs.
2. **Toolchain:** rustc 1.97+ (matches the mentat workspace).
3. **Determinism:** the core engine must be deterministic and unit/property
   tested (Time x Product revenue is the canonical fixture with known results).
4. **NL translation is CNL-only for v1** (controlled grammar), not open English
   — keeps the "deterministic core" honest.

## Crate layout

- `crates/core_model` — categories, items, measures, coordinates, formulas,
  value types. GUI/storage-free. **[Phase 0: DONE]**
- `crates/storage_mentat` — persistence via the embedded Mentat. [Phase 0]
- `crates/engine` — formula compiler (AST -> typed -> plan) + DD integration. [Phase 1]
- `crates/cli` — headless model runner / import-export. [Phase 2]
- `crates/tui` — VisiCalc-like terminal UI. [Phase 2]
- `crates/server` — HTTP/RPC API. [Phase 3]
- `crates/nl_formula` — CNL <-> formula. [Phase 4]

## Phases (build in order)

- **Phase 0 — Foundations:** core_model + storage_mentat + tests.
  - core_model: DONE (ids, value, formula, model; JSON round-trip; 4 tests).
  - storage_mentat: NEXT.
- **Phase 1 — Engine + formula compiler:** typed inference, plan (joins/aggs),
  DD integration. *DD-viability spike: DONE (gate cleared).* Next: formula
  compiler (AST -> typed -> PlanNode) and a generic plan->dataflow builder
  driven by `core_model` measures (the spike hardcodes one formula).
- **Phase 2 — TUI + CLI.**
- **Phase 3 — Server.**
- **Phase 4 — CNL natural-language formulas.**
- **Phase 5 — Desktop GUI (future).**

## Phase status (live)

- **Phase 0 — Foundations: DONE.** `core_model` (4 tests), `storage_mentat`
  round-trips a model through embedded SQLite Mentat (1 test).
- **Phase 1 — Engine: DONE (numeric core).**
  - DD-viability spike: cleared (`engine` `dd_revenue_is_incremental`).
  - Formula compiler `Formula -> TypedExpr -> PlanNode` (`compiler.rs`): type +
    dimension inference, Join insertion for broadcast, Aggregate for SUM/AVG/
    MIN/MAX. Convention: aggregation `Call` takes one measure-ref arg whose
    `DimensionSpec.over` names collapsed categories; func ids 1=SUM 2=AVG 3=MIN
    4=MAX.
  - Dataflow builder + `evaluate(model, targets)` (`dataflow.rs`): compiles each
    derived measure, builds a DD graph (InputMeasure / MapUnary / MapBinary /
    Join / Aggregate), feeds numeric input cells, returns computed values.
    Verified against the canonical Time×Product revenue results
    (1000/1000/1200/1600).
  - **Phase 1 follow-ups (deferred):** multi-layer derived measures (topological
    build so a derived measure can reference another derived measure — currently
    single-layer input->derived); non-numeric values; general `FuncCall`;
    standalone/broadcast literals; a live incremental edit API on top of
    `evaluate` (the spike proves deltas work; `evaluate` currently runs once).
- **Phase 2 — CLI: DONE (headless subset).** `crates/cli` `improv` binary:
  init/add-category/add-item/add-measure/set/list/show/export over a
  Mentat-backed store (3 tests). TUI still pending.
- **Phase 4 — CNL: DONE (initial grammar).** `crates/nl_formula` parse/describe
  with a controlled grammar + round-trip (10 tests).
- **Phase 2 (TUI), Phase 3 (server), Phase 5 (GUI): pending.**

## Definition of done for v1

Define categories/items/measures (input + formula); build a multidimensional
pivot view; enter data and edit formulas; instant incremental recalculation on
non-trivial models; save/reopen across Linux/macOS/Windows; deterministic,
tested engine.
