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

- A desktop GUI.
- Live cell editing / re-pivot in the TUI (it currently views and navigates;
  editing is the next increment).
- A live incremental edit API on top of the engine's one-shot `evaluate`
  (the engine spike proves deltas work; `evaluate` currently runs to
  completion once).
- Non-numeric derived values and general (non-aggregation) function calls.
- Server authentication (the API is localhost-only for now).

Per-phase status is tracked in `AGENT_STEERING.md` at the repository root.

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
