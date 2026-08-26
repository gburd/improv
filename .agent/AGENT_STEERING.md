# Improv — Agent Steering (live phase tracker)

This is the **live status tracker**: what is DONE / NEXT right now. The
*detailed design* lives in `.agent/steering/` (start with
`STEERING_SYSTEM_OVERVIEW.md`); the *source design of record* is `IMPROV.txt`;
contributor workflow and the CI quality gate are in the top-level `/AGENTS.md`.
When a phase lands, update this file.

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

The authoritative roadmap and Phase 5–7 invariants live in
`.agent/steering/AGENT_MASTER_STEERING.md` §6–§7. Summary:

- **Phase 0 — Foundations:** core_model + storage_mentat + tests.
- **Phase 1 — Engine + formula compiler:** typed inference, plan (joins/aggs),
  differential-dataflow evaluation.
- **Phase 2 — CLI + TUI.**
- **Phase 3 — Server.**
- **Phase 4 — CNL natural-language formulas.**
- **Phase 5 — Desktop GUI** (toolkit chosen at phase start; no new modeling
  semantics).
- **Phase 6 — External-language functions** (`CALL(func, ...)`; Python first,
  then R/Julia/WASM; pure, typed, dimension-declaring).
- **Phase 7 — SQL database connectivity** (import/export + `SQL("...")`
  live-query measures; external SQL is a source/sink, never the system of
  record).

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
  - Multi-layer derived measures: DONE — `evaluate` builds derived measures in
    topological dependency order (cycles rejected), so a derived measure may
    reference another derived measure.
  - **Textual formula parser: DONE.** `core_model::parser::parse_formula` parses
    the v1 EBNF grammar into a `Formula` (used by the CLI's `add-derived`).
  - **Scalar functions: DONE.** ABS/ROUND/FLOOR/CEIL/SQRT/NEG/MIN2/MAX2 and
    numeric comparison/logical ops evaluate through the engine.
  - **Live incremental edit API: DONE.** `session::Engine` builds the dataflow
    once on a worker thread; `set`/`clear`/`apply` push input-cell edits as
    deltas, recomputing only affected coordinates (4 session tests).
  - **Phase 1 follow-ups (deferred):** non-numeric (Text/Boolean) derived
    values as a true DD lane; standalone/broadcast literals.
- **Phase 2 — CLI: DONE.** `crates/cli` `improv` binary: init / add-category /
  add-item / add-measure (with dimensions) / add-derived (textual formula) /
  set / list / show / eval (engine compute) / export over a Mentat-backed store
  (4 tests). The full v1 flow works end-to-end from the CLI.
- **Phase 2 — TUI: DONE (viewer).** `improv_tui` renders a measure as a pivot
  grid with keyboard navigation and measure cycling (4 tests). Live editing /
  re-pivot is the next increment.
- **Phase 3 — Server: DONE.** `improv_server` JSON HTTP API over a model store
  (10 tests): model/measures/values, NL parse/describe, set-cell. Auth deferred.
- **Phase 4 — CNL: DONE (initial grammar).** `crates/nl_formula` parse/describe
  with a controlled grammar + round-trip (10 tests).
- **v1 core (Phases 0–4): DONE.** Remaining v1 follow-ups: non-numeric derived
  values (Text/Boolean DD lane) and general `FuncCall`. (Live TUI editing and
  the incremental edit API are done — see the Phase 1 / Phase 2 entries above.)
- **Phase 5 (Desktop GUI): PLANNED.**
- **Phase 6 (External-language functions): PLANNED.**
- **Phase 7 (SQL connectivity): PLANNED.**

## Definition of done for v1

Define categories/items/measures (input + formula); build a multidimensional
pivot view; enter data and edit formulas; instant incremental recalculation on
non-trivial models; save/reopen across Linux/macOS/Windows; deterministic,
tested engine.
