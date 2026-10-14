# pg_improv

A Postgres extension that runs Improv's incremental modeling engine over
Postgres data. You bind fact-table columns to Improv categories and measures,
define derived measures in Improv's formula language, and read the results
from ordinary tables (`improv_<model>.<measure>`).

Status: **layers L0 and L1** (see `docs/reviews/2026-10-14-pg-improv-design.md`
§6). The explicit `improv.recalc` and the `trigger` sync mode are
implemented. The `logical` mode (the default `sync_mode`) comes in L2; until
then a `logical` model updates only through `recalc`.

## Install

Needs Postgres 17 and `cargo-pgrx` 0.17 (`cargo install cargo-pgrx
--version 0.17.0 --locked && cargo pgrx init --pg17 $(which pg_config)`).

```sh
cd crates/pg_improv
cargo pgrx install --release      # into the pg_config on PATH
cargo pgrx test pg17              # the extension's tests
```

L0 needs no `shared_preload_libraries` entry: there is no background worker
yet. `CREATE EXTENSION` needs a superuser.

This crate is **not** part of the root Cargo workspace (pgrx needs
`pg_config` and libclang to build), so it has its own `Cargo.lock`, and
`cargo test --workspace` at the repo root does not build it. All the
Postgres-free logic is in `crates/pg_model` (`improv_pg_model`), which is a
normal workspace member.

## Quick start

```sql
CREATE EXTENSION pg_improv;

CREATE TABLE sales (quarter text, product text, price float8, qty float8);
INSERT INTO sales VALUES ('Q1', 'Widget', 10, 100), ('Q1', 'Gadget', 20, 30),
                         ('Q2', 'Widget', 10, 200), ('Q2', 'Gadget', 20, 60);

SELECT improv.create_model('plan');
SELECT improv.bind_category('plan', 'Time',    'sales', 'quarter');
SELECT improv.bind_category('plan', 'Product', 'sales', 'product');
SELECT improv.bind_measure ('plan', 'Price',    'sales', 'price');
SELECT improv.bind_measure ('plan', 'Quantity', 'sales', 'qty');
SELECT improv.define('plan', 'Revenue = Price * Quantity');
SELECT improv.define('plan', 'Total = SUM(Revenue OVER Time)');

SELECT improv.recalc('plan');           -- 6: result rows written
SELECT * FROM improv_plan.revenue;      -- time | product | value
SELECT * FROM improv_plan.total;        -- product | value

UPDATE sales SET price = 12 WHERE quarter = 'Q2' AND product = 'Widget';
SELECT improv.recalc('plan');           -- 2: one revenue row, one total row
```

## SQL API (schema `improv`, all `SECURITY INVOKER`)

| Function | Does |
| --- | --- |
| `create_model(name text) → bigint` | Creates the model and its result schema `improv_<lower(name)>`. |
| `bind_category(model, category, rel regclass, col text)` | Creates the category if missing and records that `rel.col` supplies its items. |
| `bind_measure(model, measure, rel regclass, value_col text)` | Creates an input measure read from `rel.value_col`. Its dimensions are the categories already bound on `rel`, in category-id order. |
| `define(model, definition text) → bigint` | Adds a derived measure from `Target = expr` and creates its result table. `SQL(...)` and `CALL(...)` are rejected. |
| `recalc(model) → bigint` | Recomputes every derived measure in the caller's transaction and returns the number of result rows inserted, updated or deleted. |
| `recalc(model, measures text[]) → bigint` | Recomputes only the named measures (input or derived, case-insensitive) and every derived measure that depends on them. `NULL` means all. |
| `set_sync_mode(model, mode text)` | `trigger`: installs the sync triggers on every bound relation and recomputes once. `logical`/`manual`: removes them. Re-run it after binding a new relation. |
| `what_if(model, edits jsonb) → TABLE(measure text, coord jsonb, value float8)` | Evaluates the model with `edits` applied in memory and returns the derived cells that differ from the stored results (`value` NULL: the cell would vanish). Writes nothing. |
| `drop_model(name)` | Drops the catalog rows and the result schema. |

Names must match `^[A-Za-z][A-Za-z0-9_]*$` (at most 56 characters, and not
`value`). Result tables are `improv_<model>.<lower(measure)>` with one `text`
column per dimension (named `lower(category)`), `value double precision`, and
a primary key on the dimensions.

`improv.max_cells` (default 5 000 000, settable per session) caps the number
of input cells `recalc` loads; above it, `recalc` raises an ERROR.

Bound data rules: a NULL value means no cell, and a row with a NULL dimension
is skipped. Two rows on the same coordinate are an ERROR (the `agg` option
from the design is not implemented yet). Item ids for bound values are
minted once, in lexical order, and are never reused.

## Keeping results current: trigger vs logical vs recalc

| | `trigger` (L1) | `logical` (L2, default) | `recalc` (explicit) |
| --- | --- | --- | --- |
| When results update | In the writer's statement, before it returns | After the commit, by a background worker (eventual) | When you call it, in your transaction |
| Consistency | Transactional: commit both or neither; read-your-writes | Lags the commit | Transactional, in your snapshot |
| Writer cost | Added latency and result-row locks on every write statement (batched per statement) | None | None until you call it |
| Requirements | None | `wal_level=logical`, `REPLICA IDENTITY` (FULL or a PK) on bound tables | None |
| Failure mode | A failing recompute fails the writer's statement | A stuck worker retains WAL (bounded by `max_slot_wal_keep_size`) | Errors are yours to handle |
| Best for | Small, read-your-writes models | Busy tables, replicas | Bulk loads, migrations, `manual` mode |

### Trigger mode

`set_sync_mode(model, 'trigger')` installs four `AFTER … FOR EACH STATEMENT`
triggers per bound relation (`improv_<model>_sync_{ins,upd,del,trn}`; Postgres
allows transition tables only on single-event triggers). Each calls
`improv.on_change(model)`. That function reads the touched dimension values
from the statement's transition tables (`improv_old` ∪ `improv_new`) and
recomputes only the derived measures that depend on the relation's bound
measures. If every measure in that chain is element-wise (operators and
scalar functions over references to measures with the same or fewer
dimensions, so no `SUM … OVER`, `BY`, `EXCEPT` or `CALL`), it loads and
rewrites only the touched coordinates. Otherwise it recomputes those measures
in full. Unchanged result rows are never rewritten. `TRUNCATE` recomputes the
dependents in full. New dimension values mint items, as `recalc` does.

```sql
SELECT improv.set_sync_mode('plan', 'trigger');
BEGIN;
UPDATE sales SET price = 11 WHERE product = 'Widget';
SELECT value FROM improv_plan.revenue WHERE product = 'Widget';  -- already 1100, …
ROLLBACK;                                                         -- results revert too

SELECT * FROM improv.what_if('plan',
  '[{"measure": "Price", "coord": {"Time": "Q1", "Product": "Widget"}, "value": 12}]');
```

Benchmark sketch, from `docs/reviews/bench/bench-pg-trigger.sh` (c7i.4xlarge,
release build, 10 000 input cells, medians): a 1000-row UPDATE takes 4.3 ms in
manual mode, 32 ms in trigger mode when the chain is element-wise, and 45 ms
when it contains a `SUM`. A full `recalc` takes 34 ms. Loading the whole model
dominates the cost, so trigger mode costs about one recalc per statement.

## Catalog

The `improv.*` tables are a compatible superset of the catalog that
`improv_storage_pg` (the GUI/CLI's `postgres://` store) creates, so a model
created in SQL opens in the GUI and vice versa. The extension adds
`improv.category_binding`. A pg_test checks the superset property against
`improv_storage_pg::CATALOG_SQL`.
