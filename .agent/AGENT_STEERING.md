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
- `crates/storage_sql` — SQL import/export (SQLite). [Phase 7]
- `crates/engine` — formula compiler (AST -> typed -> plan) + DD integration. [Phase 1]
- `crates/cli` — headless model runner / import-export. [Phase 2]
- `crates/tui` — VisiCalc-like terminal UI. [Phase 2]
- `crates/server` — HTTP/RPC API. [Phase 3]
- `crates/nl_formula` — CNL <-> formula. [Phase 4]
- `crates/gui` — egui/eframe desktop app (`improv-gui`). [Phase 5]
- `crates/extfn` — external-language function runtime (Python). [Phase 6]

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
  - **Non-numeric value lane: DONE.** The dataflow carries `CellValue`
    (`Num`/`Bool`/`Text`/`Err`), so boolean/text derived measures work
    end-to-end (`Hot = Price > 15` → `Bool`). Consumers (CLI/TUI/server) render
    via `CellValue`'s `Display`/`as_num`.
  - **Phase 1 follow-ups (deferred):** `Date` values in the DD lane;
    standalone/broadcast literals.
- **Phase 2 — CLI: DONE.** `crates/cli` `improv` binary: init / add-category /
  add-item / add-measure (with dimensions) / add-derived (textual formula) /
  set / list / show / eval (engine compute) / export over a Mentat-backed store
  (4 tests). The full v1 flow works end-to-end from the CLI.
- **Phase 2 — TUI: DONE (viewer).** `improv_tui` renders a measure as a pivot
  grid with keyboard navigation and measure cycling (4 tests). Live editing /
  re-pivot is the next increment.
- **Phase 3 — Server: DONE.** `improv_server` JSON HTTP API over a model store
  (10 tests): model/measures/values, NL parse/describe, set-cell. **Bearer-token
  auth** (Auth::Tokens from `IMPROV_API_TOKEN`/`IMPROV_API_TOKENS`; `/health`
  public, 401/403 otherwise; open mode when unset).
- **Phase 4 — CNL: DONE (initial grammar).** `crates/nl_formula` parse/describe
  with a controlled grammar + round-trip (10 tests).
- **v1 core (Phases 0–4): DONE.** v1 follow-ups now done too: `Date` values in
  the DD lane (`CellValue::Date`) and standalone/broadcast literal measures
  (a bare `X = 5` scalar broadcasts by reference).
- **Phase 5 (Desktop GUI): DONE.** `crates/gui` `improv-gui` in the NeXTSTEP
  look-and-feel: explorer, editable pivot grid, on-grid margin category tiles,
  top formula bar, inspector, charts, saved views, filters, keyboard nav,
  multi-category-per-axis stacking, and **virtualized rendering** (mixed-radix
  `nth_tuple` + `body.rows`, so a row axis of millions of lines never
  materializes). Charts plot the full Cartesian product of stacked axes.
- **Scenario management (what-if): DONE.** First-class `Scenario` = a named
  overlay of input-cell overrides (`Model.scenarios`, `with_scenario`);
  deterministic (the engine sees ordinary inputs). Persisted via Mentat; CLI
  `scenario` + `eval --scenario`.
- **Phase 6 (External-language functions): IN PROGRESS.** In-process named
  scalar functions are callable from formula text — `ABS`/`ROUND`/`FLOOR`/
  `CEIL`/`SQRT`/`NEG`/`MIN2`/`MAX2` via `core_model::parser::scalar_func`,
  evaluated deterministically by the engine. The external runtime lives in
  `crates/extfn` (`improv_extfn`): a `Registry` of typed, purity-asserted
  `ExternalFn`s (defined in `core_model`) evaluated in an isolated Python
  subprocess (timeout + JSON marshalling). **Integrated host-side**:
  `Model.external_calls` marks a measure as `func(arg_measures...)`, and
  `engine::external::refresh_external_measure` evaluates it per coordinate and
  writes input cells — external calls stay OFF the differential-dataflow hot
  path, so the engine core stays pure/deterministic. Persisted via Mentat
  `:meta/*` blobs. A `CALL(func, args...)` formula-grammar form parses (via
  `core_model::parser::parse_definition`) to an `ExternalCall`; the CLI exposes
  `register-ext` / `define` / `refresh-ext` end to end. Runtimes: **Python, R,
  Julia, WASM (in-process wasmi), and Pure-lang** (per-language runners sharing
  a spawn/timeout/JSON-envelope helper; wasm has a numeric f64 ABI). REMAINING:
  an OS-level sandbox (seccomp/namespaces) — currently isolated-mode + timeout.
- **Phase 7 (SQL connectivity): IN PROGRESS.** `crates/storage_sql` imports a
  `SELECT` into an input measure (columns → categories/items/cells), exports a
  measure's cells to a SQL table, and supports **live-query refresh**
  (`SqlSource` on the model + `refresh_sql_measure`). Works over a
  backend abstraction (`SqlConn`/`Backend`): **SQLite and PostgreSQL** (pg via
  `postgres 0.19`). **Connection management**: serde `Connection` descriptors
  hold a password-less DSN + `password_env`; the secret is read from the
  environment at connect time and redacted from logs (credentials out of band).
  CLI `import-sql` / `refresh-sql` / `export-sql`. SQL data enters as ordinary
  input cells (deterministic core untouched); identifiers validated, values
  bound (injection-safe). A `SQL("...")` definition-grammar form parses (via
  `parse_definition`) as a whole-RHS source; column→dimension mapping stays with
  the `import-sql` CLI command. Refresh-policy metadata (`RefreshPolicy`:
  manual/on-load/interval) is recordable per SQL *and* external-call measure
  (`--refresh` flag), `refresh-all` re-runs every external-sourced measure at
  once, and `serve-refresh` is a daemon that *honors* the policy timing
  automatically (pure decision in `core_model::schedule`). PLANNED: GUI
  import/export wizards. **DuckDB** is a first-class backend (bundled build;
  Backend::Duckdb + ConnKind::Duckdb; deny allows the build-time
  CDLA-Permissive-2.0 it pulls).

## Definition of done for v1

Define categories/items/measures (input + formula); build a multidimensional
pivot view; enter data and edit formulas; instant incremental recalculation on
non-trivial models; save/reopen across Linux/macOS/Windows; deterministic,
tested engine.

## Post-v0.5.0 plan (re-prioritized toward credibility & usability)

v0.5.0 delivered engine breadth (5 extfn languages, 3 SQL backends, scenarios,
a scheduler daemon, a NeXTSTEP-styled GUI with stacking/virtualization) faster
than table-stakes usability. The plan now re-orders toward what makes people
*trust and adopt* the tool, in phases:

- **Phase A — credibility gaps:**
  1. **CSV/TSV import/export: DONE (incl. GUI/TUI wiring).**
     `improv_storage_csv` (`import_csv`/`export_measure_csv`); CLI
     `import-csv`/`export-csv`; a GUI wizard (toggle-able Import/Export window,
     dimension-mapping rows, pure builders); a TUI `I`/`E` command prompt
     reusing the cell-edit buffer machinery (compact CLI-like syntax).
  2. **Crash-safety: DONE.** `save_model` used to transact the model in up to
     6 separate SQLite transactions (a crash between them left a partial
     save); it now opens ONE `InProgress` and commits once, so a save is
     all-or-nothing. Proven by a regression test that forces a genuine
     mid-save failure and asserts nothing leaks through. See
     `.agent/steering/AGENT_DATABASE_CONNECTIVITY.md` §9.
  3. **Honest GUI L&F labeling: DONE.** README/steering now say
     "NeXTSTEP-inspired, unverified" rather than implying parity with real
     Lotus Improv 3.0 / Quantrix Modeler (see §6.5 in
     `.agent/steering/AGENT_GUI_STEERING.md`).
- **Phase B — formula editor UX: DONE.** Syntax highlighting (identifiers,
  functions, numbers, strings, date literals, operators) + inline error
  position highlighting in the GUI formula bar, via a pure
  `gui::formula_highlight::scan`/`highlight_formula`.
- **Phase C — harden what exists:**
  1. **Formula-parser fuzz target: DONE**
     (`fuzz/fuzz_targets/fuzz_formula_parser.rs`) — and it found two real bugs,
     both fixed: a tokenizer panic on multi-byte UTF-8 (byte-cast-to-`char`
     walking off a UTF-8 boundary), and an unbounded-recursion stack overflow
     on deeply-nested parens (now bounded by `MAX_PARSE_DEPTH`). This validates
     the whole point of Phase C.
  2. **extfn OS-level sandbox: DONE.** `extfn::sandbox::{SandboxPolicy,apply}`:
     on Linux, wraps subprocess runtimes in `bwrap` (read-only root, no
     network/IPC/UTS namespace, fresh tmpfs `/tmp`) when available, plus
     `setrlimit` (CPU/address-space/FDs/procs) always; macOS gets the rlimits;
     Windows is a documented no-op (timeout only). Fail-open by design
     (never breaks functionality if bwrap/rlimits are unavailable) — a
     best-effort boundary, not a hard guarantee (upgrade path: require
     bwrap/gVisor). `ExternalFn.pure` maps to `Restricted` by default.
- **Phase D — reach (after A–C):**
  1. **Import/export "plugin" dedup: DONE.** New tiny `improv_data_source`
     crate holds the import logic byte-for-byte duplicated between
     `storage_sql`/`storage_csv` (category/measure setup + name→id item
     interning); both delegate to it. No trait/dynamic-loading system built
     — there is exactly one real caller per backend today, so a generic
     `DataSource` abstraction would be pure indirection (ponytail: no
     interface for a plugin loader nobody's asked to use at runtime). Every
     public function signature is unchanged; zero call-site changes anywhere.
  2. **Out-of-core storage: investigated, not implemented.**
     Empirically-verified ceiling is now **5,000,000 cells** in-memory (up
     from the previous 1M), ~3.5GB peak RSS and ~2.5-4 min `evaluate()`
     wall-clock at that size (`cargo test -p improv_engine --test stress --
     --ignored --nocapture scale_evaluate_5m`); 10M is estimated (~7GB RSS)
     but was not run to avoid risking an OOM. Design doc at
     `.agent/steering/AGENT_OUT_OF_CORE_DESIGN.md` recommends changing the
     storage-to-engine boundary so a `Model` handed to the DD graph is a
     dependency-closure *window* over the measures an operation actually
     needs (`ModelStore::load_partial`), not the whole model, as the first
     step — not a DD/engine-internals rewrite.
  3. **GUI import/export wizards: DONE.** A toggle-able Import/Export CSV
     window in the GUI (dimension-mapping rows, pure builders); a TUI
     `I`/`E` command prompt. (Same landing as the Phase A CSV item above.)
  4. **Hosted refresh-scheduler service: DONE.** `improv-server` now runs the
     same due-measure/refresh loop as the CLI's `serve-refresh` daemon as a
     background tokio task (`IMPROV_SCHEDULER`/`IMPROV_SCHEDULER_TICK_SECS`),
     so CALL measures refresh automatically while the API runs, no separate
     process needed. `GET /scheduler/status` (bearer-protected) reports it.
     SQL-sourced measures stay CLI-only (`SqlSource` carries no connection
     string on the model; inventing one would be scope creep).
  5. **`improv stream` (stdin→stdout incremental compute): DONE.**
     `improv stream <db> <target-measure> [...]` reads
     `<measure> <value> [Cat=Item,...]` lines from stdin (measure by id or
     name), applies each via the live `session::Engine::set`/`apply`
     (delta-only recompute, not a full reload/re-eval per line), and prints
     only the CHANGED cells of the target measure(s) to stdout as
     `<measure> <Cat=Item,...> = <value>` — diffed against the prior
     snapshot host-side (the engine itself always returns a full snapshot).
     A malformed line warns on stderr and is skipped, not fatal; on EOF the
     final state saves back to `<db>`. Verified end to end:
     `producer | improv stream model.db Revenue | consumer`. 3 new tests
     (pure `resolve_measure`/`apply_stream_line`/diff logic; no process
     spawning needed — `apply_stream_line` takes the `Model`+`Engine`
     directly, the natural test seam).

Phase status is tracked per-item above as it lands.
