# Changelog

All notable changes to Improv are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

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
  `.typos.toml`, CI, license (Apache-2.0), contributor guide.

[Unreleased]: https://codeberg.org/gregburd/improv
