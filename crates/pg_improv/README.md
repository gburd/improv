# pg_improv

A Postgres extension that runs Improv's incremental modeling engine over
Postgres data. You bind fact-table columns to Improv categories and measures,
define derived measures in Improv's formula language, and read the results
from ordinary tables (`improv_<model>.<measure>`).

Status: **layers L0–L4 done** (see `docs/reviews/2026-10-14-pg-improv-design.md`
§6). All three ways of keeping results current work: `logical` (the default,
a background worker tailing a logical replication slot), `trigger` (inside
the writer's transaction), and an explicit `improv.recalc` you call in your
own transaction. The desktop apps open a Postgres-hosted model directly:
`improv-gui "postgres://user@host/db?model=plan"`.

## Install

Needs Postgres 17 and `cargo-pgrx` 0.17 (`cargo install cargo-pgrx
--version 0.17.0 --locked && cargo pgrx init --pg17 $(which pg_config)`).

```sh
cd crates/pg_improv
cargo pgrx install --release      # into the pg_config on PATH
cargo pgrx test pg17              # the extension's tests
```

For the default `logical` mode, preload the library and enable logical
decoding (then restart Postgres):

```ini
# postgresql.conf
shared_preload_libraries = 'pg_improv'
wal_level = logical
improv.database = 'sales'       # the database the sync worker serves
#improv.poll_ms = 200           # how often it polls each model's slot
#improv.max_changes = 10000     # most decoded changes per poll
max_slot_wal_keep_size = 10GB   # recommended: caps WAL a stuck worker can retain
```

Without the preload, `CREATE EXTENSION` still works and `trigger` / `manual`
models (and `recalc`) behave the same; `logical` models then update only
through `recalc`. `CREATE EXTENSION` needs a superuser.

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

-- logical mode (default): the worker creates the slot and catches up
SELECT * FROM improv_plan.revenue;      -- time | product | value
SELECT * FROM improv_plan.total;        -- product | value

UPDATE sales SET price = 12 WHERE quarter = 'Q2' AND product = 'Widget';
-- …a moment later (commit -> visible ≈ poll interval + one recompute):
SELECT * FROM improv_plan.revenue WHERE product = 'Widget';
SELECT * FROM improv.status('plan');    -- mode, slot, lag_bytes, last apply

-- or, in your own transaction, right now:
SELECT improv.recalc('plan');           -- result rows changed
```

Then open the same model in the NeXT-style desktop app (it reads the catalog,
edits formulas and input cells, and writes the bound fact table):

```sh
improv-gui "postgres://me@localhost/sales?model=plan"
```
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
| `enable_logical(model)` | Creates (or resets) the publication `improv_<model>` over the bound relations and gives PK-less ones `REPLICA IDENTITY FULL`. Called for you by `bind_measure` and `set_sync_mode(…, 'logical')`. The worker creates the slot. |
| `status(model) → TABLE(sync_mode, slot, lag_bytes, confirmed_lsn, last_applied_at, rows_applied, last_error)` | How the model is kept current, and how far the logical worker is behind. |
| `drop_model(name)` | Drops the catalog rows, the result schema, the triggers and the publication, and queues the slot for the worker to drop. |

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

### Logical mode (default)

A background worker (`pg_improv sync`, one per server, serving
`improv.database`) polls, for each model in `logical` mode, a logical
replication slot `improv_<model>` (plugin `pgoutput`) over the publication
`improv_<model>`. When committed source transactions touched a bound
relation it recomputes the model and writes the results **in one worker
transaction that also advances the slot**. A crash or `pg_terminate_backend`
therefore re-reads and re-applies (the writes are idempotent upserts and
deletes); nothing is lost or applied twice. Rolled-back source transactions
never appear. The worker restarts itself 5 s after it is killed.

The slot retains WAL until the worker confirms it: watch `lag_bytes` in
`improv.status`, and set `max_slot_wal_keep_size`. Slots are created by the
worker (Postgres forbids creating one in a transaction that has written), so
a new model in logical mode starts updating within one poll of being bound.

Apply strategy: one full recompute per poll that saw changes. A live
incremental engine fed with the decoded deltas is the planned next step.

### Benchmark

From `docs/reviews/bench/bench-pg-modes.sh` on a c7i.4xlarge (Debian 13,
PG 17, release build; medians of 15; `improv.poll_ms = 20`). "Writer" is the
writer's UPDATE+COMMIT round trip; "fresh" is commit → result visible to
another session.

| cells | change | mode | writer | fresh |
| ---: | --- | --- | ---: | ---: |
| 10k | 1 row | logical | **8 ms** | 99 ms |
| 10k | 1 row | trigger | 82 ms | **92 ms** |
| 10k | 1 row | recalc in txn | 83 ms | 92 ms |
| 10k | 1k rows | logical | **14 ms** | 153 ms |
| 10k | 1k rows | trigger | 171 ms | 181 ms |
| 10k | 1k rows | recalc in txn | 102 ms | **112 ms** |
| 100k | 1 row | logical | **8 ms** | 1161 ms |
| 100k | 1 row | trigger | 1100 ms | 1111 ms |
| 100k | 1 row | recalc in txn | 1088 ms | **1098 ms** |
| 100k | 1k rows | logical | **72 ms** | 1552 ms |
| 100k | 1k rows | trigger | 2355 ms | 2366 ms |
| 100k | 1k rows | recalc in txn | 1312 ms | **1323 ms** |

Logical mode keeps the writer fast at any model size; results trail by about
one recompute. Trigger mode makes results visible at commit but charges that
recompute to every write statement. All three are dominated by reloading the
model's inputs per recompute (about 11 µs per cell); the live incremental
engine in the worker is what removes it.

## Catalog

The `improv.*` tables are a compatible superset of the catalog that
`improv_storage_pg` (the GUI/CLI's `postgres://` store) creates, so a model
created in SQL opens in the GUI and vice versa. The extension adds
`improv.category_binding`. A pg_test checks the superset property against
`improv_storage_pg::CATALOG_SQL`.
