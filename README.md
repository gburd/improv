# Improv

A cross-platform, standalone **multidimensional spreadsheet** in Rust, inspired
by Lotus Improv and Quantrix. Improv separates *structure*, *logic*, and *data*:
you model in named **categories**, **items**, and **measures** instead of cell
coordinates, and derived values flow from a **differential-dataflow** graph so
recalculation is always incremental.

- **Modeling first, grid second.** Think in dimensions (Time, Product, Region),
  not rows and columns.
- **Always-incremental recalculation.** Each measure is a materialized view;
  edits propagate as deltas through differential dataflow.
- **Durable, scriptable, auditable.** The model persists as immutable facts in
  an embedded (SQLite-backed) [Mentat](https://codeberg.org/gregburd/mentat)
  datom store.
- **Interfaces:** a headless CLI today; a VisiCalc-style TUI, a server mode, and
  controlled-natural-language formulas are in progress.

> Status: early. See [`AGENT_STEERING.md`](AGENT_STEERING.md) for architecture,
> design constraints, and live per-phase status.

## Workspace layout

| Crate | Purpose |
|-------|---------|
| `improv_core_model` | Categories, items, measures, coordinates, formulas, values. GUI/storage-free. |
| `improv_storage_mentat` | Persistence: model ⇄ datoms on embedded Mentat (SQLite). |
| `improv_engine` | Formula compiler (`Formula → TypedExpr → PlanNode`) + differential-dataflow evaluation. |
| `improv_nl_formula` | Controlled-natural-language ⇄ formula translation. |
| `improv_cli` | The `improv` command-line tool. |
| `improv_tui` | VisiCalc-style terminal pivot viewer (`improv-tui`). |
| `improv_server` | JSON HTTP API over a model store (`improv-server`). |

## Quick start

```sh
cargo build --workspace
cargo test  --workspace

# Build and drive a model with the CLI:
cargo run -p improv_cli -- init model.db
cargo run -p improv_cli -- add-category model.db 1 Product
cargo run -p improv_cli -- add-item     model.db 20 1 WidgetA
cargo run -p improv_cli -- add-measure  model.db 100 Price number input
cargo run -p improv_cli -- set          model.db 100 10.0 --at Product=WidgetA
cargo run -p improv_cli -- list         model.db
```

## The idea, in one formula

```text
Revenue[Time, Product] = Price[Product] * Quantity[Time, Product]
```

`Price` is broadcast over `Time`; `Revenue` recomputes only for the cells an
edit actually touches. `SUM(Revenue OVER Time)` aggregates to `Revenue[Product]`.

## Development

See [`CONTRIBUTING.md`](CONTRIBUTING.md). The quality gate (all warning-free):

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
typos
```

## License

Apache-2.0. See [`LICENSE`](LICENSE).
