# Changelog

All notable changes to Improv are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **NeXTSTEP look-and-feel for the desktop GUI**, matching the original Lotus
  Improv: a NeXTSTEP theme (gray desktop, chiseled/beveled controls, squared
  corners, paper-white grid), on-grid **category tiles at the grid margins** as
  the pivot gesture (drag a tile between Rows/Columns/Pages margins), a top
  **formula bar** (`<Measure> = <expr>`), chiseled grid headers with a
  `Rows \ Columns` corner stub, a NeXT-style **tool palette**, and
  **multi-category-per-axis stacking** (stack >1 category on an axis; the grid
  renders the Cartesian product with nested headers and group-outlined rows).
- **Refresh scheduler daemon** (`serve-refresh`): honors each measure's
  `RefreshPolicy` automatically — on every tick it refreshes the due measures
  (on-load once, interval every N seconds). The scheduling decision is a pure,
  unit-tested `core_model::schedule`; external-call measures also carry a
  policy (`define ... --refresh`).
- **SQL refresh policy + `refresh-all`**: `SqlSource` records a `RefreshPolicy`
  (manual / on-load / interval); CLI `import-sql --refresh ...` sets it and
  `refresh-all <db> [source.sqlite]` refreshes every external-sourced measure
  (CALL + SQL) at once. Policy is advisory metadata (a scheduler that honors
  timing is future work); the engine core is untouched.
- **`CALL(...)` / `SQL(...)` definition grammar**: `core_model::parser::
  parse_definition` parses a measure definition as an ordinary formula or a
  whole-RHS source form (`Target = CALL(func, args...)` /
  `Target = SQL("query")`), keeping source forms off the engine's expression
  grammar. CLI `define` / `register-ext` / `refresh-ext` make external-call
  measures usable end to end.
- **Chart view** (`improv-gui`): a Chart toggle plots the selected measure as
  grouped bars (optional line overlay), drawn with `egui::Painter` (no new
  dependency), honoring filters and page pinning.
- **External-function measures wired into the engine** (Phase 6): a measure can
  be defined as `func(arg_measures...)` (`Model.external_calls`);
  `engine::external::refresh_external_measure` evaluates the registered
  `ExternalFn` per coordinate via `improv_extfn` and writes input cells —
  host-side, so the dataflow core stays pure. `ExternalFn`/`Language` moved to
  `core_model`; defs + calls persist through Mentat.
- **Saved views + filters in the UI** (`improv-gui`, `improv-tui`): save the
  current layout as a named view, load it to reproduce measure + axis placement
  + pins + filters, and filter a category to a subset of items. GUI: Views
  panel + Filters checkboxes. TUI: `S` save, `v` cycle, `f`/`F` filter.
- **PostgreSQL backend + connection management** (Phase 7): SQL import/export/
  refresh now work over a backend abstraction (SQLite + Postgres via
  `postgres`); serde `Connection` descriptors keep credentials out of band
  (password-less DSN + `password_env`, resolved at connect time, redacted logs).
- **External-function runtime** (`improv_extfn`, Phase 6): a registry of typed,
  purity-asserted external functions evaluated in an isolated Python subprocess
  (timeout + JSON marshalling), speaking `core_model::Value` — the runtime the
  engine's `Expr::Call` will dispatch to (wiring pending). Isolated-mode +
  timeout, not yet an OS sandbox.
- **GUI keyboard grid navigation** (`improv-gui`): arrow/hjkl move a highlighted
  cell cursor; Enter/F2 edit, `[`/`]` page, `n`/`N` cycle measures; click also
  moves the cursor.
- **Saved views + filters** (`core_model::View`/`Filter`, Quantrix/Improv
  parity): a `View` captures a measure, an axis-order permutation
  (rows/columns/pages), pinned page items, and per-category filters (restrict a
  dimension to a subset). Persisted through Mentat (`:view/*` datoms), survives
  save/load. The data model both interfaces can drive; UI wiring follows.
- **GUI drag-and-drop pivoting** (`improv-gui`): an axis shelf (Rows / Columns /
  Pages) lets you drag category chips between axes (egui drag-and-drop) or use
  per-chip `→` / a Pivot button, with per-page-dimension `< >` selectors — the
  Improv/Quantrix re-pivot, no formula edits. Grid re-pivots live.
- **SQL live-query measures** (Phase 7): an imported SQL measure now records a
  refreshable `SqlSource` on the model (`improv_storage_sql::add_sql_measure` /
  `refresh_sql_measure`); CLI `refresh-sql` re-runs the stored query and
  replaces the measure's cells (persisted via a `:measure/sql-source` datom).
  SQL data re-enters as ordinary input cells, so the engine recomputes
  dependents with no SQL path of its own; these measures are marked
  nondeterministic (refresh-gated).
- **TUI pivoting + mouse** (`improv-tui`): `p` rotates categories across
  rows/columns/pages (Improv/Quantrix-style re-pivot, no formula changes), and
  left-click selects a cell (mouse capture enabled). Complements the existing
  arrow navigation, `[`/`]` paging, `e`/Enter editing, and measure cycling.
- **Nix flake**: `nix develop` dev shell with the Rust toolchain, GUI runtime
  libraries on `LD_LIBRARY_PATH` (so `improv-gui` runs), and the tooling.
- **TUI paging** (`improv-tui`): `[` / `]` cycle the pinned item of a measure's
  extra (3rd+) dimension, so multi-dimensional measures are fully viewable
  slice-by-slice; a `page i/n` indicator and a key-hint footer were added.
- **SQL connectivity** (`improv_storage_sql`, Phase 7, SQLite): import a
  `SELECT` into a new input measure (columns → categories/items/cells) and
  export a measure's cells to a SQL table. CLI `import-sql` / `export-sql`.
  SQL data enters as ordinary input cells, so the deterministic engine core is
  untouched; identifiers are validated and values bound (injection-safe).
- **Named scalar functions in formulas**: `ABS`, `ROUND`, `FLOOR`, `CEIL`,
  `SQRT`, `NEG`, `MIN2`, `MAX2` are now callable as `NAME(args...)` in formula
  text (parser registry `core_model::parser::scalar_func`, arity-checked),
  evaluated deterministically by the engine — the in-process foundation for
  Phase 6's external-language `CALL(...)` runtime.
- **Desktop GUI** (`improv_gui`, egui/eframe): `improv-gui <db>` — a model
  explorer, an editable pivot grid (input cells edit through the live engine
  with autosave; derived cells read-only), a formula editor (edit/add derived
  measures from formula text), and an inspector (metadata, dimensions,
  dependencies, formula-in-English, error-cell count). Phase 5 toolkit
  decision: egui/eframe.
- **Non-numeric value lane**: the engine dataflow now carries a tagged
  `CellValue` (`Num`/`Bool`/`Text`/`Err`), so boolean and text derived measures
  evaluate end-to-end (e.g. `Hot = Price > 15` yields a boolean). Comparisons
  and `NOT`/`AND`/`OR` produce `Bool`. CLI/TUI/server render via `CellValue`.
- **Live incremental edit API** (`improv_engine::session::Engine`): builds the
  differential-dataflow graph once and applies input-cell edits as deltas
  (`set`/`clear`/`apply`), recomputing only affected coordinates — no rebuild.
- **TUI live editing**: edit input cells in the pivot grid (`e`/Enter),
  derived measures recompute live via the session engine; autosave on quit.
- **Server** `POST /measures/:id/eval`: apply edits through the live engine and
  return the recomputed snapshot (what-if by default; `"persist": true` writes
  through).
- **Textual formula parser** (`improv_core_model::parser`): `parse_formula` /
  `parse_expr` accept Improv formula text (`Revenue = Price * Quantity`,
  precedence, comparisons, `AND`/`OR`/`NOT`, `SUM|AVG|MIN|MAX(x OVER Cat)`),
  resolving names against the model.
- **Scalar built-in functions** in the engine: `ABS`, `ROUND`, `FLOOR`, `CEIL`,
  `SQRT`, `NEG`, `MIN2`, `MAX2` (numeric); comparison/logical operators
  evaluate in the numeric lane (1.0/0.0).
- **CLI**: `add-derived <db> <id> <name> <formula>` (define a formula measure)
  and `eval <db> <measure-id>` (compute a derived measure via the engine);
  `add-measure` now takes trailing category names to declare a measure's
  dimensions. The full v1 flow — model, formula, incremental recalculation —
  now works end-to-end from the CLI.
- **TUI** (`improv_tui`): a VisiCalc-style terminal pivot viewer
  (`improv-tui <db>`) — renders a measure as a 2-D grid (categories on
  rows/columns, extra dims as pages), keyboard navigation, measure cycling,
  panic-safe terminal teardown.
- **Server** (`improv_server`): a JSON HTTP API (`improv-server <db> [addr]`)
  — `/health`, `/model`, `/measures`, `/measures/:id/values`, `/nl/parse`,
  `/nl/describe`, `/measures/:id/cells`.
- **Multi-layer derived measures**: `engine::evaluate` builds derived measures
  in topological order (cycles rejected), so a derived measure may reference
  another derived measure.
- **Tests**: property tests (proptest) for model/codec/CNL round-trips; fuzz
  targets (cargo-fuzz) for the CNL parser, model JSON, and coordinate codec;
  engine determinism suite (bit-for-bit reproducibility, insertion-order
  independence) and `#[ignore]` stress tests (100k-cell recalculation).
- **CI/CD**: GitHub + Codeberg workflows for the full quality gate,
  cross-platform build/test, release qualification, and docs publishing.
- **Core model** (`improv_core_model`): categories, items, measures,
  coordinates, dimension-aware formula AST, value/error types. JSON round-trip.
- **Storage** (`improv_storage_mentat`): persist a model as datoms on the
  embedded (SQLite) Mentat store and reconstruct it by query. Save↔load round
  trip.
- **Engine** (`improv_engine`): formula compiler (type + dimension inference,
  lowering to a Join/Aggregate/Map plan) and differential-dataflow evaluation.
  Verified against the canonical Time×Product revenue oracle.
- **CLI** (`improv_cli`): `init`, `add-category`, `add-item`, `add-measure`,
  `set`, `list`, `show`, `export` over a Mentat-backed store.
- **Natural language** (`improv_nl_formula`): controlled-English ⇄ formula
  parse/describe with a stable round trip.
- Project infrastructure: workspace, `rustfmt.toml`, `clippy.toml`, `deny.toml`,
  `.typos.toml`, CI, dual license (Apache-2.0 OR MIT), contributor guide.

[Unreleased]: https://codeberg.org/gregburd/improv
