# Storage & Persistence

`improv_storage_mentat` persists a `Model` to an embedded, SQLite-backed
[Mentat](https://codeberg.org/gregburd/mentat) store — a Datomic-style datom
database. Categories, items, measures, and input cells are stored as
first-class entities, so the model is **queryable**, not an opaque blob.

`ModelStore::open(path)` opens (or creates) a store; pass `""` for an in-memory
database. Opening transacts the schema (a no-op after the first time).
`save_model` writes the model; `load_model` rebuilds it by querying.

Front ends do not use `ModelStore` directly. They open
`improv_store::Store`, which can also open a model kept in Postgres (see
[The Postgres store](#the-postgres-store)).

## The datom schema

The schema is defined as an EDN transaction in
`improv_storage_mentat::schema::SCHEMA_EDN`. It declares these attributes.

### Category

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:category/id` | `long` | one | `unique/identity`, indexed |
| `:category/name` | `string` | one | |

### Item

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:item/id` | `long` | one | `unique/identity`, indexed |
| `:item/name` | `string` | one | |
| `:item/category` | `ref` | one | reference to a category entity |

### Measure

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:measure/id` | `long` | one | `unique/identity`, indexed |
| `:measure/name` | `string` | one | |
| `:measure/value-type` | `string` | one | the declared value type |
| `:measure/categories` | `ref` | **many** | the measure's tensor dimensions (a set) |
| `:measure/category-order` | `string` | one | JSON array of category ids: the order of the set above. Coordinates and SQL/CSV columns depend on it. Absent in older stores, which load sorted by id. |
| `:measure/kind` | `string` | one | `"input"` or `"derived"` |
| `:measure/description` | `string` | one | optional |
| `:measure/formula` | `string` | one | JSON-serialized `Formula`, on derived measures |
| `:measure/sql-source` | `string` | one | JSON `SqlSource` (query, dimension columns, value column, refresh policy), on SQL-backed measures |

### Cell (an input value)

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:cell/key` | `string` | one | `unique/identity`, indexed — synthetic `measure::coord` key |
| `:cell/measure` | `ref` | one | reference to the owning measure |
| `:cell/coord` | `string` | one | JSON-serialized `Coordinate` |
| `:cell/value-number` | `double` | one | present for numeric cells |
| `:cell/value-boolean` | `boolean` | one | present for boolean cells |
| `:cell/value-text` | `string` | one | present for text cells |

### View (a saved pivot layout)

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:view/id` | `long` | one | `unique/identity`, indexed |
| `:view/json` | `string` | one | the whole view, as JSON |

### Meta (one entity per store)

Maps with no natural per-entity shape are stored as JSON on a singleton.

| Attribute | Value type | Cardinality | Notes |
|-----------|------------|-------------|-------|
| `:meta/singleton` | `long` | one | `unique/identity`, indexed; always the same value |
| `:meta/external-fns` | `string` | one | JSON: external function definitions |
| `:meta/external-calls` | `string` | one | JSON: `CALL(...)` measures and their arguments |
| `:meta/scenarios` | `string` | one | JSON: scenarios |

## How a model maps to datoms

- **Native attributes.** Ids, names, kinds, and value types map straight to
  native datom value types (`long`, `string`, `boolean`, `double`, `ref`).
- **Formulas and coordinates are JSON.** A `Formula` AST and a `Coordinate` are
  serialized with serde to JSON strings and stored in `:measure/formula` and
  `:cell/coord`. This is why a derived measure's formula survives a round trip
  even though the datom layer has no notion of an expression tree. Views, the
  SQL source of a measure, and the meta maps are JSON too.
- **Category order.** `:measure/categories` is a set, so the store loses its
  order. `:measure/category-order` keeps it. On load the categories are sorted
  by that list. A category the list does not name sorts after those it does.
- **Typed cell values.** A cell writes exactly one of the three typed value
  attributes, chosen by the value's type:

  | Value | Attribute |
  |-------|-----------|
  | `Number`, `Enum` | `:cell/value-number` |
  | `Boolean` | `:cell/value-boolean` |
  | `Text`, `DateTime` (RFC 3339 text) | `:cell/value-text` |

  An `Error` value cannot be saved as an input; the save fails with
  `Integrity`.
- **Cell identity.** Each cell is keyed by a synthetic string
  `"<measure-id>::<coord-json>"` on `:cell/key`, which is `unique/identity`.
  Re-saving the same measure+coordinate updates the cell in place.

## Save

`save_model` is **one SQLite transaction**. It calls
`Store::begin_transaction` once, makes several `transact` calls inside it, and
calls `commit` once at the end. Later steps can use lookup-refs (for example
`:item/category`, `:measure/categories`, `:cell/measure`) to entities that
earlier steps wrote but have not committed yet. If any step fails, the
transaction is dropped without a commit and nothing is written.

The steps run in this order:

| Step | What is transacted |
|------|--------------------|
| 1 | categories |
| 2 | items |
| 3 | measures |
| 4 | cell retractions |
| 5 | cells |
| 6 | views |
| 7 | meta (only if the model has external functions, external calls, or scenarios) |

Each step is split into `transact` calls of at most
`MAX_DATOMS_PER_TRANSACT` (2000) datoms. Mentat aborts the process (a hard
`assert!`, not an error) on a transact carrying 5461 or more datoms for one
attribute queue. The chunks share the one transaction, so the save is still
all-or-nothing.

A single entity cannot be split. A measure carries one `:measure/categories`
datom per category, so a measure with more than **5460** categories is refused
up front with `StoreError::Integrity`, before anything is written.

### Cells are diffed

Before writing cells, the save reads every stored cell and compares:

- a cell whose stored value is unchanged is skipped (no datoms);
- a cell whose value changed is re-asserted;
- a value left under another type's attribute (the cell held text and now
  holds a number, say) is retracted;
- a stored cell the model no longer has is retracted whole.

After a save, the store's cells are exactly `model.inputs`.

### What is not retracted

Only cells are diffed. Categories, items, measures and views that the model no
longer has are **not** retracted; they stay in the store and come back on the
next load. Stale `:measure/categories` refs are not retracted either: re-save a
measure with a changed category set and the store keeps the union. A loaded
model can therefore grow past the 5460-category limit without any single save
having been that wide. The source marks this as a known bug (see the comments
in `save_model`).

### Optimistic concurrency

`load_model` records the store's high-water transaction, read live from
SQLite by `TX_HIGH_WATER_SQL`. It is the newest transaction in the log that
changed anything other than `:db/txInstant`, so the schema transact every
`open` performs does not count as a write.

`save_model` re-reads the high-water inside its transaction. If another writer
has committed since this handle loaded (or last saved), the save returns
`StoreError::Conflict` and writes nothing. A handle that never loaded saves
unconditionally. `loaded_tx` and `set_loaded_tx` carry the token between
handles, for callers that open the store fresh for each save.

To recover from a conflict, read the model again. `reload_model` does that;
it is the same as `load_model`. The GUI's **File ▸ Revert to Saved** and the
TUI's `R` key do the same thing: they open the store, load the model with a
fresh token, and rebuild the engine. The conflicting edit is lost; the other
writer's changes are picked up.

## Load

`load_model` runs Datalog queries and rebuilds the `Model`:

- categories, then items (each relinked to its category);
- measures: one query for the required fields, then per measure one query each
  for the formula (derived measures only), description, categories, category
  order, and SQL source;
- cells: **one query per typed value attribute** (three in all), each
  returning `(measure id, coord, value)` rows. The value is decoded by the
  owning measure's declared type. If an old store holds a cell under more than
  one attribute, the attribute matching the declared type wins;
- views and the meta singleton.

`load_partial(measure_ids)` loads the same structure, views and meta, but only
the cells of `measure_ids` and their dependency closure (every measure reached
through a derived formula or an external call's arguments). The filter is in
the query. The handle remembers that closure, and its next save retracts
only the closure's missing cells, so a partial model saves safely: cells of
measures it never loaded stay as they are. A full load lifts the limit again.

## The Postgres store

`improv_store::Store` is the one store type the GUI, TUI, CLI and server use.
`Store::open(target)` looks at the target:

| Target | Backend |
|--------|---------|
| starts with `postgres://` or `postgresql://` | `improv_storage_pg::PgModelStore` |
| anything else (a path; `""` for in-memory) | `improv_storage_mentat::ModelStore` |

Both backends have the same methods: `load_model`, `load_partial`,
`save_model`, `reload_model`, `tx_high_water`, `loaded_tx`, `set_loaded_tx`.
A conflict from either backend becomes `improv_store::StoreError::Conflict`.

A Postgres target names one model: `postgres://me@db/sales?model=plan`.
`split_model_url` removes the `model` parameter and passes the rest to the
driver. `?model=` is required; without it `open` fails. Opening a model name
that does not exist yet creates it.

### The catalog

The model is stored normalized in the `improv` schema. The DDL is
`CATALOG_SQL` in `crates/storage_pg/src/lib.rs`:

| Table | Holds |
|-------|-------|
| `improv.model` | one row per model: `name` (unique), `sync_mode` (`logical`, `trigger`, `manual`), `slot_name` |
| `improv.category` | categories, keyed by `(model_id, id)` |
| `improv.item` | items, with `category_id` |
| `improv.measure` | `name`, `value_type`, `kind`, `formula`, `description`, `categories bigint[]` |
| `improv.binding` | a measure bound to a fact table: `rel`, `value_col`, `dim_cols` (category name → column), optional `agg` (`sum`, `avg`, `min`, `max`, `count`) |
| `improv.cell` | unbound input cells: `coord jsonb`, `value jsonb` |
| `improv.view` | saved views, `layout jsonb` |
| `improv.meta` | one `blob jsonb` per model: external functions, external calls, scenarios, SQL sources |
| `improv.model_version` | one `tx` counter per model: the conflict token |

`CATALOG_SQL` also creates the unique index `item_name_uniq` on
`(model_id, category_id, name)` and the function `improv.mint_items`, which
assigns ids to item names it has not seen. The pg_improv extension uses the
same function.

`open` creates the catalog, under an advisory lock, when
`improv.mint_items` does not exist. When the pg_improv extension is installed
the function exists, so the client runs no DDL and the extension owns the
tables. The extension's catalog is the same plus `improv.category_binding`,
which records the column that supplies a bound category.

Formulas are written as JSON of the `Formula` AST. On load, a formula that is
not JSON is parsed as formula text (`Target = expr`), which is what the
extension's `improv.define` writes. Cell values are JSON scalars; dates are
RFC 3339 strings.

### Bound and unbound cells

A measure with a `binding` row lives in its fact table. Load runs
`SELECT dims…, value FROM rel` (with `GROUP BY` when the binding has an
`agg`). Dimension values map to items by name; names with no item are minted
by `improv.mint_items`. A row with a NULL value is no cell; a row with a NULL
dimension is ignored. Two rows on one coordinate of an unaggregated binding
are an `Integrity` error. Cells of unbound measures come from `improv.cell`.

Load reads in one `REPEATABLE READ`, read-only transaction. Item minting runs
after it closes, read-committed, so it sees items other clients minted. The
handle keeps a snapshot of what it loaded.

### Save

`save_model` is one transaction. It locks the model's `model_version` row and
returns `Conflict` if `tx` moved since this handle loaded. Then it writes the
**diff against the snapshot**:

- categories, items, measures and views are upserted; only rows the snapshot
  held but the model dropped are deleted, so rows another client added
  survive;
- bound cells that were added, changed or removed are written to the fact
  table with a guarded `UPDATE … WHERE <dims> AND value IS NOT DISTINCT FROM
  <loaded value>`. A removed cell sets the value column to NULL and keeps the
  row. An added cell with no row is `INSERT`ed. If a guard matches no row, or
  several, SQL changed that cell since the load, and the save fails with
  `Conflict`. The save never runs DDL and never deletes a fact-table row;
- an aggregated measure is read-only: a save that changes one of its cells
  fails with `Integrity`;
- unbound cells that changed are upserted into `improv.cell`, and removed
  ones are deleted;
- the meta blob is replaced, and `model_version.tx` is incremented.

A handle with no snapshot (it never loaded) writes the whole model unchecked
and deletes rows the model lacks. As with the Mentat store, a partial model
from `load_partial` saves safely through the handle that loaded it, since the
diff only touches what it loaded.

`Store::save_token` carries both the version and the snapshot to a later
handle, for callers that reopen the store per save.

### Transport

This build has no TLS. `open` refuses:

- `sslmode=require`, which cannot be honoured (use an SSH tunnel to a loopback
  port);
- any host that is not `localhost`, a loopback address, or a Unix socket,
  unless the URL sets `sslmode=disable` explicitly, since the password would
  cross the network in cleartext.

A connect error that is not from the database is reported without the
connection string, so a password in the URL is not echoed.

For the extension itself (binding tables, result tables, sync modes) see
[Improv in Postgres (pg_improv)](./postgres.md).
