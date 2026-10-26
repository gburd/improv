# Command-line reference

<!-- Generated from the clap definitions (crates/*/src/args.rs) by
     `UPDATE_DOCS=1 cargo test -p improv_cli`. Do not edit. -->

Every command and flag of the four binaries, as `--help` prints them.
The same text is in the man pages: `man improv`, `man improv-set`, and
so on. Every command also takes `-h` (a summary) and `--help`, and each
binary `-V`/`--version`. [Running Improv](./running.md) shows which
binary to use when.

## `improv`

Build, edit and evaluate Improv models from the shell. Each command opens a model store, reads or changes the model, and saves it back. Run 'improv help COMMAND' for one command.

```text
Usage: improv [COMMAND]
```

### `improv init`

Create or open a store and install the schema.

An existing model is kept. An empty string is an in-memory store.

```text
Usage: improv init <db>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |

### `improv add-category`

Add a category (a dimension, such as Time)

```text
Usage: improv add-category <db> <id> <name>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<id>` | The category's numeric id |
| `<name>` | The category's name |

### `improv add-item`

Add an item (a member of a category).

The category must exist.

```text
Usage: improv add-item <db> <id> <category-id> <name>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<id>` | The item's numeric id |
| `<category-id>` | The id of the item's category |
| `<name>` | The item's name |

### `improv add-measure`

Add an input measure.

Its type says how set parses its values. The word input is required, after the type and before the categories it ranges over, which follow by name: add-measure m.db 101 Quantity number input Time Product.

```text
Usage: improv add-measure <db> <id> <name> <type> <input> [category]...
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<id>` | The measure's numeric id |
| `<name>` | The measure's name |
| `<type>` | number, boolean or text |
| `<input>` | The word input |
| `<category>` | The categories the measure ranges over, by name |

### `improv add-derived`

Add a derived (formula) measure.

The formula is the right-hand side only, e.g. "Price * Quantity" or "SUM(Revenue OVER Time)", with names resolved against the model. The measure's categories come from the measures it references. A formula cycle is refused.

```text
Usage: improv add-derived <db> <id> <name> <formula>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<id>` | The measure's numeric id |
| `<name>` | The measure's name |
| `<formula>` | The formula text (in IMPROV_LOCALE's spelling) |

### `improv define`

Create a measure or LAMBDA from a definition, "Target = ...".

Three forms: a formula, "Revenue = Price * Quantity"; an external call, "H = CALL(hypot, Price, Quantity)", which refresh-ext populates; or a named LAMBDA, "Margin = LAMBDA(p, c, p - c)", which formulas then call as Margin(Price, Cost). The id names a LAMBDA, and redefining one keeps its first id. The SQL(...) form needs a column-to-dimension map: use import-sql. A formula cycle is refused.

```text
Usage: improv define [OPTIONS] <db> <id> <definition>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<id>` | The measure's (or LAMBDA's) numeric id |
| `<definition>` | The definition: Target = formula, CALL(...) or LAMBDA(...) |
| `--refresh` `<policy>` | When serve-refresh re-runs it: manual, on-load or interval:SECS (default: `manual`) |

### `improv register-ext`

Register a Python external function.

Number arguments and a Number result: the body reads the list args and sets result, e.g. register-ext m.db hypot 2 "result = (args\[0]**2 + args\[1]**2) ** 0.5".

```text
Usage: improv register-ext <db> <name> <arity> <python-body>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<name>` | The function's name, as CALL(...) names it |
| `<arity>` | How many arguments it takes |
| `<python-body>` | The Python body |

### `improv refresh-ext`

Run a CALL(...) measure's function and store its cells.

The function runs host-side, over its argument measures; external calls stay off the dataflow engine.

```text
Usage: improv refresh-ext <db> <measure-id>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<measure-id>` | A measure's numeric id |

### `improv set`

Set an input cell.

The value is parsed by the measure's type: a number, true or false, or verbatim text. --at maps category names to item names; omit it for a scalar cell. Pairs may be comma-joined (--at Time=Q1,Product=A), space-separated (--at Time=Q1 Product=A) or in repeated flags; they are merged, and a category may appear only once.

```text
Usage: improv set [OPTIONS] <db> <measure-id> <value>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<measure-id>` | A measure's numeric id |
| `<value>` | The cell's value |
| `--at` `<Cat=Item>` | The cell's coordinate: Category=Item pairs, comma-joined, space-separated, or in repeated --at flags |

### `improv list`

Print the categories, items, measures, views and input cells

```text
Usage: improv list <db>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |

### `improv save-view`

Save a named view of a measure, as the GUI does.

--rows and --cols stack category names outer to inner; each defaults to the next unused category of the measure. The other categories are pages, pinned to their first item. A view with the same id is replaced.

```text
Usage: improv save-view [OPTIONS] <db> <view-id> <name> <measure>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<view-id>` | The view's numeric id |
| `<name>` | The view's name |
| `<measure>` | The measure, by id or name |
| `--rows` `<Cat,...>` | The row categories, comma-separated, outer first |
| `--cols` `<Cat,...>` | The column categories, comma-separated, outer first |

### `improv show`

Print one measure and its input cells

```text
Usage: improv show <db> <measure-id>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<measure-id>` | A measure's numeric id |

### `improv eval`

Compute a derived measure and print its cells.

With --scenario, under that what-if scenario's overrides.

```text
Usage: improv eval [OPTIONS] <db> <measure-id>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<measure-id>` | A measure's numeric id |
| `--scenario` `<name>` | The scenario to evaluate under |

### `improv scenario`

Add an input-cell override to a what-if scenario.

The scenario is created if it is new. The base model is untouched; evaluate under it with eval ... --scenario NAME. --at takes the same forms as for set.

```text
Usage: improv scenario [OPTIONS] <db> <scenario-name> <measure-id> <value>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<scenario-name>` | The scenario's name |
| `<measure-id>` | A measure's numeric id |
| `<value>` | The overriding value |
| `--at` `<Cat=Item>` | The cell's coordinate: Category=Item pairs, comma-joined, space-separated, or in repeated --at flags |

### `improv export`

Print the whole model as JSON

```text
Usage: improv export <db>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |

### `improv import-sql`

Import a SQLite query into a new SQL-backed input measure.

Each dimension spec maps a result column to a category (its distinct values become items); the value column holds the numbers. Re-run the query with refresh-sql. Example: improv import-sql m.db sales.db 100 Revenue "SELECT t,p,r FROM sales" r t:1:Time p:2:Product

```text
Usage: improv import-sql [OPTIONS] <db> <source.sqlite> <measure-id> <measure-name> <SELECT> <value-col> <dim-col:cat-id:cat-name>...
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<source.sqlite>` | The SQLite database to query |
| `<measure-id>` | The new measure's numeric id |
| `<measure-name>` | The new measure's name |
| `<SELECT>` | The query |
| `<value-col>` | The result column holding the values |
| `<dim-col:cat-id:cat-name>` | COLUMN:CATEGORY-ID:CATEGORY-NAME: a column, and the category its distinct values become items of |
| `--refresh` `<policy>` | When serve-refresh re-runs it: manual, on-load or interval:SECS (default: `manual`) |

### `improv refresh-sql`

Re-run an SQL-backed measure's query and replace its cells.

New dimension values become new items.

```text
Usage: improv refresh-sql <db> <source.sqlite> <measure-id>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<source.sqlite>` | The SQLite database to query |
| `<measure-id>` | A measure's numeric id |

### `improv refresh-all`

Refresh every external measure now.

Every CALL(...) measure, and every SQL-backed measure when a source is given. Each measure's refresh policy is reported, not applied.

```text
Usage: improv refresh-all <db> [source.sqlite]
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<source.sqlite>` | The SQLite database the SQL-backed measures query |

### `improv serve-refresh`

Refresh measures as their refresh policies fall due, until interrupted.

On every tick: on-load measures once, interval:N measures every N seconds. SQL-backed measures refresh only when a source is given.

```text
Usage: improv serve-refresh [OPTIONS] <db> [source.sqlite]
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<source.sqlite>` | The SQLite database the SQL-backed measures query |
| `--tick` `<SECS>` | Seconds between ticks (default: `5`) |

### `improv export-sql`

Write a measure's input cells to a SQLite table.

One column per dimension category, then the value column; the table is created if absent. Identifiers are validated and values bound as parameters.

```text
Usage: improv export-sql <db> <target.sqlite> <measure-id> <table> <value-col>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<target.sqlite>` | The SQLite database to write |
| `<measure-id>` | A measure's numeric id |
| `<table>` | The table |
| `<value-col>` | The value column's name |

### `improv import-csv`

Import a CSV or TSV file into a new input measure.

Columns are header names, or 0-based indices with --no-header. The delimiter is a comma unless --tsv is given or the file ends in .tsv. Example: improv import-csv m.db sales.csv 100 Revenue r t:1:Time p:2:Product

```text
Usage: improv import-csv [OPTIONS] <db> <file.csv> <measure-id> <measure-name> <value-col> <dim-col:cat-id:cat-name>...
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<file.csv>` | The file to read |
| `<measure-id>` | The new measure's numeric id |
| `<measure-name>` | The new measure's name |
| `<value-col>` | The column holding the values |
| `<dim-col:cat-id:cat-name>` | COLUMN:CATEGORY-ID:CATEGORY-NAME: a column, and the category its distinct values become items of |
| `--tsv` `<TSV>` | Tab-separated |
| `--no-header` `<NO_HEADER>` | The file has no header row: columns are 0-based indices |

### `improv export-csv`

Write a measure's cells to a CSV or TSV file.

An input measure's stored cells or a derived measure's computed ones: one column per dimension category, then the value; header row first. Text a spreadsheet would run as a formula is prefixed with a quote.

```text
Usage: improv export-csv [OPTIONS] <db> <target.csv> <measure-id>
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<target.csv>` | The file to write (tab-separated if it ends in .tsv) |
| `<measure-id>` | A measure's numeric id |
| `--tsv` `<TSV>` | Tab-separated |

### `improv stream`

Apply input edits from stdin and print the cells they change.

One edit per line, MEASURE VALUE \[Cat=Item,...] (the measure by id or name; no coordinate for a scalar measure). Each applies to a live incremental engine, and every changed cell of a target measure prints as MEASURE Cat=Item,... = VALUE. A malformed line is reported on stderr and skipped. At end of input the model is saved. Example: printf 'Price 12 Product=WidgetA\n' | improv stream m.db Revenue

```text
Usage: improv stream <db> <target-measure>...
```

| Argument | Description |
| --- | --- |
| `<db>` | The model store: a file, or postgres://USER@HOST/DB?model=NAME |
| `<target-measure>` | The measures whose changes to print, by id or name |

## `improv-gui`

Open a model in a NeXTSTEP-style workspace.

```text
Usage: improv-gui [model]
```

| Argument | Description |
| --- | --- |
| `<model>` | The model: a store file, or postgres://USER@HOST/DB?model=NAME. Without it the model is in memory only |

## `improv-tui`

Show one measure of a model as a pivot grid in the terminal.

```text
Usage: improv-tui <model>
```

| Argument | Description |
| --- | --- |
| `<model>` | The model: a store file (a new one opens empty), or postgres://USER@HOST/DB?model=NAME |

## `improv-server`

Serve one model over HTTP/1.1 as JSON.

```text
Usage: improv-server [OPTIONS] [model] [addr]
```

| Argument | Description |
| --- | --- |
| `<model>` | The model: a store file, or postgres://USER@HOST/DB?model=NAME (default: `improv.db`) |
| `<addr>` | The address to listen on (default: `127.0.0.1:3000`) (env: `IMPROV_ADDR`) |
| `--static` `<DIR>` | Also serve the files in DIR (a web front end's build) at /, beside the API; off by default |
