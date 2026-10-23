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

The CLI is a headless front-end over the model store. Every subcommand opens
a store (a single SQLite file, a Postgres model named by a
`postgres://…?model=` URL, or an in-memory database if you pass `""`),
mutates the loaded model, and saves it back.

Run `improv help` for the full command list. The commands, exactly as accepted
by the binary, are:

| Command | Arguments |
|---------|-----------|
| `init` | `<db>` |
| `add-category` | `<db> <id> <name>` |
| `add-item` | `<db> <id> <category-id> <name>` |
| `add-measure` | `<db> <id> <name> <number\|boolean\|text> input [Category ...]` |
| `add-derived` | `<db> <id> <name> <formula>` |
| `define` | `<db> <id> "<Name> = <formula>"` or `"<Name> = CALL(fn, Arg, ...)"` |
| `set` | `<db> <measure-id> <value> [--at Cat=Item,Cat=Item ...]` |
| `scenario` | `<db> <scenario> <measure-id> <value> [--at Cat=Item,...]` |
| `list` | `<db>` |
| `show` | `<db> <measure-id>` |
| `eval` | `<db> <measure-id> [--scenario <name>]` |
| `save-view` | `<db> <view-id> <name> <measure> [--rows Cat,Cat] [--cols Cat,...]` |
| `export` | `<db>` (JSON) |
| `stream` | `<db> <target-measure> ...` (edits on stdin, changed cells on stdout) |
| `import-csv`, `export-csv` | see `man improv` |
| `import-sql`, `refresh-sql`, `export-sql` | see `man improv` |
| `register-ext`, `refresh-ext`, `refresh-all`, `serve-refresh` | external functions; see `man improv` |
| `help` | (also `--help`, `-h`) |
| `--version` | (also `-V`) |

Notes:

- Ids (`<id>`, `<category-id>`, `<measure-id>`) are plain numbers.
- `add-measure` adds an **input** measure; the trailing `input` keyword is
  required, and any further arguments are category **names** declaring the
  measure's dimensions (e.g. `... input Time Product`). The type (`number`,
  `boolean`, `text`) declares how `set` parses the value.
- `add-derived` adds a **formula** measure from Improv formula text (e.g.
  `"Price * Quantity"` or `"SUM(Revenue OVER Time)"`); its categories are
  inferred from the measures it references. Compute it with `eval`.
- `set --at` maps category **names** to item **names**. Pairs may be
  comma-joined (`--at Time=2025,Product=WidgetA`), space-separated
  (`--at Time=2025 Product=WidgetA`), or given as repeated `--at` flags; they
  are merged, and a category may appear only once. Omit
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
improv set           "$DB" 101 50  --at Time=2025,Product=WidgetB
improv set           "$DB" 101 120 --at Time=2026,Product=WidgetA
improv set           "$DB" 101 80  --at Time=2026,Product=WidgetB
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
views:
inputs:
  100::{Product=WidgetA} = 10
  100::{Product=WidgetB} = 20
  101::{Time=2025,Product=WidgetA} = 100
  101::{Time=2025,Product=WidgetB} = 50
  101::{Time=2026,Product=WidgetA} = 120
  101::{Time=2026,Product=WidgetB} = 80
```

Save a named view (the GUI opens it from the model browser). `--rows` and
`--cols` stack category names; leftover categories become pages:

```sh
improv save-view "$DB" 1 "Quantity by year" Quantity --rows Time --cols Product
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
added derived measure 102 'Revenue'
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
added derived measure 103 'RevByProduct'
eval 103 'RevByProduct' (2 cells):
  RevByProduct[Product=WidgetA] = 2200
  RevByProduct[Product=WidgetB] = 2600
```

## Beyond the CLI

Open the same file interactively: `improv-tui model.db` in a terminal, or
`improv-gui model.db` on the desktop, where the saved view shows up in the
model browser. Feed it new numbers with `improv stream model.db Revenue`.
[Ways to Use Improv](./ways.md) compares the front ends, and `man improv`
lists every CLI command, including CSV and SQL import and export and
sandboxed Python functions.
