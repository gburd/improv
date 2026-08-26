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
| `add-measure` | `<db> <id> <name> <number\|boolean\|text> input` |
| `set` | `<db> <measure-id> <value> [--at Cat=Item,Cat=Item ...]` |
| `list` | `<db>` |
| `show` | `<db> <measure-id>` |
| `export` | `<db>` |
| `help` | (also `--help`, `-h`) |

Notes:

- Ids (`<id>`, `<category-id>`, `<measure-id>`) are plain numbers.
- `add-measure` only accepts **input** measures today; the trailing `input`
  keyword is required. The type (`number`, `boolean`, `text`) declares how
  `set` parses the value.
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
improv add-measure   "$DB" 100 Price    number input
improv add-measure   "$DB" 101 Quantity number input
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

## Where the CLI stops today

The CLI creates categories, items, and **input** measures, sets input cells, and
inspects the model. It does **not** yet define derived (formula) measures or run
the engine to compute them; that is exercised through the `improv_engine`
crate's API and tests. Building derived measures from the CLI, along with a TUI
and server, is planned.
