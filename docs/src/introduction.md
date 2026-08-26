# Introduction

Improv is a cross-platform, standalone **multidimensional spreadsheet** written
in Rust, inspired by Lotus Improv and Quantrix. It separates three things a
traditional grid tangles together:

- **Structure** — the named dimensions of a model (categories and their items).
- **Logic** — formulas defined over dimensions, not cell addresses.
- **Data** — the raw input values.

You model in named **categories**, **items**, and **measures** instead of `A1`
cell coordinates, and derived values flow through a
[differential-dataflow](https://github.com/TimelyDataflow/differential-dataflow)
graph so recalculation is always incremental: an edit propagates only to the
cells it actually affects.

## What works today

- **`improv_core_model`** — categories, items, measures, coordinates, formulas,
  values. GUI-free and storage-free.
- **`improv_storage_mentat`** — persists a model as datoms in an embedded
  (SQLite-backed) [Mentat](https://codeberg.org/gregburd/mentat) store.
- **`improv_engine`** — the formula compiler
  (`Formula → TypedExpr → PlanNode`) and its differential-dataflow evaluator
  for numeric measures.
- **`improv_nl_formula`** — a controlled-natural-language grammar that parses to
  and renders from a formula.
- **`improv_cli`** — the headless `improv` command-line tool.

## What is planned

The following are named in the design but **not yet implemented**:

- A desktop GUI (Phase 5).
- TUI re-pivot (dragging categories between axes) and a formula-editor pane —
  the TUI edits input cells and recomputes live today, but re-pivot is pending.
- Non-numeric (Text/Boolean) derived values and general (non-aggregation)
  function calls.
- External-language functions (Phase 6) and SQL connectivity (Phase 7).
- Server authentication (the API is localhost-only for now).

Per-phase status is tracked in `.agent/AGENT_STEERING.md`; the detailed design
is in `.agent/steering/`.

## Reading order

1. [The Improv Model](./model.md) — the vocabulary: categories, items,
   measures, coordinates.
2. [Getting Started](./getting-started.md) — build the workspace and drive a
   model from the CLI.
3. [The Formula Language](./formulas.md) — dimension-aware formulas and
   aggregation.
4. [Controlled Natural Language](./cnl.md) — the English-ish formula surface.
5. [Architecture & Design](./architecture.md) — the compile-to-dataflow
   pipeline.
6. [Storage & Persistence](./storage.md) — the Mentat datom schema.
