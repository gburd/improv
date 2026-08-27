# Getting Started

## Build

Improv is a Cargo workspace. From the repository root:

```sh
cargo build --workspace
cargo test  --workspace
```

The `improv` CLI binary is produced by the `improv_cli` crate. You can run it
either through Cargo:

```sh
cargo run -p improv_cli -- <command> [args]
```

or directly once built:

```sh
./target/debug/improv <command> [args]
```

The examples below use the built binary.

## The CLI

The CLI is a headless front-end over the Mentat-backed model store. Every
subcommand opens a store (a single SQLite file, or an in-memory database if you
pass `""`), mutates the loaded model, and saves it back.

Run `improv help` for the full command list. The commands, exactly as accepted
by the binary, are:

| Command | Arguments |
|---------|-----------|
| `init` | `<db>` |
| `add-category` | `<db> <id> <name>` |
| `add-item` | `<db> <id> <category-id> <name>` |
| `add-measure` | `<db> <id> <name> <number\|boolean\|text> input [Category ...]` |
| `add-derived` | `<db> <id> <name> <formula>` |
| `set` | `<db> <measure-id> <value> [--at Cat=Item,Cat=Item ...]` |
| `list` | `<db>` |
| `show` | `<db> <measure-id>` |
| `eval` | `<db> <measure-id>` |
| `export` | `<db>` |
| `help` | (also `--help`, `-h`) |

Notes:

- Ids (`<id>`, `<category-id>`, `<measure-id>`) are plain numbers.
- `add-measure` adds an **input** measure; the trailing `input` keyword is
  required, and any further arguments are category **names** declaring the
  measure's dimensions (e.g. `... input Time Product`). The type (`number`,
  `boolean`, `text`) declares how `set` parses the value.
- `add-derived` adds a **formula** measure from Improv formula text (e.g.
  `"Price * Quantity"` or `"SUM(Revenue OVER Time)"`); its categories are
  inferred from the measures it references. Compute it with `eval`.
- `set --at` maps category **names** to item **names**, comma-separated. Omit
  `--at` for a scalar (zero-dimensional) cell.
- `set` parses the value according to the measure's declared type: a `number`
  measure wants a float, a `boolean` measure wants `true`/`false`, a `text`
  measure takes the string verbatim.

## Quick start

Build the `Time × Product` revenue model from the
[model chapter](./model.md), one command at a time:

```sh
DB=model.db

improv init          "$DB"
improv add-category  "$DB" 1 Time
improv add-category  "$DB" 2 Product
improv add-item      "$DB" 10 1 2025
improv add-item      "$DB" 11 1 2026
improv add-item      "$DB" 20 2 WidgetA
improv add-item      "$DB" 21 2 WidgetB
improv add-measure   "$DB" 100 Price    number input Product
improv add-measure   "$DB" 101 Quantity number input Time Product
improv set           "$DB" 100 10 --at Product=WidgetA
improv set           "$DB" 100 20 --at Product=WidgetB
improv set           "$DB" 101 100 --at Time=2025,Product=WidgetA
```

Inspect the model:

```sh
improv list "$DB"
```

```text
categories:
  1 Time
  2 Product
items:
  10 2025 (category 1)
  11 2026 (category 1)
  20 WidgetA (category 2)
  21 WidgetB (category 2)
measures:
  100 Price [input] Number
  101 Quantity [input] Number
inputs:
  100::{Product=WidgetA} = 10
  100::{Product=WidgetB} = 20
  101::{Time=2025,Product=WidgetA} = 100
```

Look at a single measure and its cells:

```sh
improv show "$DB" 100
```

```text
measure 100 Price [input] Number
  cells:
    100::{Product=WidgetA} = 10
    100::{Product=WidgetB} = 20
```

Dump the whole model as JSON (useful for scripting or diffing):

```sh
improv export "$DB"
```

## Define a derived measure and compute it

Define `Revenue` from a textual formula, then evaluate it through the engine:

```sh
improv add-derived "$DB" 102 Revenue "Price * Quantity"
improv eval        "$DB" 102
```

```text
eval 102 'Revenue' (4 cells):
  Revenue[Time=2025, Product=WidgetA] = 1000
  Revenue[Time=2025, Product=WidgetB] = 1000
  Revenue[Time=2026, Product=WidgetA] = 1200
  Revenue[Time=2026, Product=WidgetB] = 1600
```

Derived measures can build on other derived measures. Roll `Revenue` up over
`Time` with an aggregation:

```sh
improv add-derived "$DB" 103 RevByProduct "SUM(Revenue OVER Time)"
improv eval        "$DB" 103
```

```text
eval 103 'RevByProduct' (2 cells):
  RevByProduct[Product=WidgetA] = 2200
  RevByProduct[Product=WidgetB] = 2600
```

## Beyond the CLI

The CLI defines categories, items, input and derived measures, sets input
cells, evaluates derived measures through the engine, imports/exports SQL, and
registers and runs external Python functions. For interactive use, the
`improv-tui` terminal viewer edits input cells and re-pivots live, and the
`improv-gui` desktop app adds drag-and-drop pivoting, charts, saved views, and
filters. See the [architecture chapter](./architecture.md) for the full
pipeline and the [README](https://codeberg.org/gregburd/improv#readme) for the
per-interface run instructions and keybindings.
