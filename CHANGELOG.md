# Changelog

All notable changes to Improv are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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
