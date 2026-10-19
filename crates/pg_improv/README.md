# pg_improv

A Postgres extension that runs Improv's incremental modeling engine over
Postgres data. You bind fact-table columns to Improv categories and measures,
define derived measures in Improv's formula language, and read the results
from ordinary tables (`improv_<model>.<measure>`).

Status: **layers L0–L5 done** (see `docs/reviews/2026-10-14-pg-improv-design.md`
§6). Every way of keeping results current works: `logical` (the default,
a background worker tailing a logical replication slot), `trigger` (inside
the writer's statement), `commit` (once per writing transaction, just
before it commits), and an explicit `improv.recalc` you call in your own
transaction. The desktop apps open a Postgres-hosted model directly:
`improv-gui "postgres://user@host/db?model=plan"`.

## Install

Supports Postgres **15, 16, 17 and 18**. Three ways to get it:

- **Binary:** each release ([GitHub](https://github.com/gburd/improv/releases),
  [Codeberg](https://codeberg.org/gregburd/improv/releases))
  has `pg_improv-<version>-pg<N>-x86_64-linux.tar.gz`. Unpack it over `/`
  (it holds the `.so`, the control file, and the install and update scripts
  under the paths `pg_config` reports).
- **PGXN:** `pgxn download pg_improv`, then build it as below. It is a pgrx
  (Rust) extension, so it builds with `cargo pgrx`, not `make`.
- **Source:** needs `cargo-pgrx` 0.17 (`cargo install cargo-pgrx --version
  0.17.0 --locked && cargo pgrx init --pg17 $(which pg_config)`).

```sh
cd crates/pg_improv
cargo pgrx install --release --pg-config $(which pg_config)    # PG 17 (default)
cargo pgrx install --release --no-default-features --features pg18 \
  --pg-config /usr/lib/postgresql/18/bin/pg_config              # another major
cargo pgrx test pg17                                             # the extension's tests
```

**Upgrading:** install the new release, restart Postgres (the preloaded
library is loaded at startup), then `ALTER EXTENSION pg_improv UPDATE`. The
update scripts (`sql/pg_improv--<old>--<new>.sql`) ship with every release,
and `tests/upgrade.sh` checks each one against a live model.

**Backups:** `pg_dump` keeps your models (the `improv.*` catalog is
registered with `pg_extension_config_dump`) and their result tables.
After a restore, the sync worker creates a fresh replication slot and
catches up with one recompute. Before 0.9.0 a dump restored every model
empty.


For the default `logical` mode, preload the library and enable logical
decoding (then restart Postgres):

```ini
# postgresql.conf
shared_preload_libraries = 'pg_improv'
wal_level = logical
improv.database = 'sales'       # the database the sync worker serves (default: postgres)
#improv.poll_ms = 200           # how often it polls each model's slot
#improv.max_changes = 10000     # most decoded changes per poll
max_slot_wal_keep_size = 10GB   # recommended: caps WAL a stuck worker can retain
```

Without the preload, `CREATE EXTENSION` still works and `trigger` / `manual`
models (and `recalc`) behave the same; `logical` models then update only
through `recalc`, and `set_sync_mode(…, 'commit')` is refused (`commit`
mode also works with `session_preload_libraries = 'pg_improv'`). (Before 0.9.0, `CREATE EXTENSION` without the preload
crashed the session; `tests/txn.sh` now checks it.) `CREATE EXTENSION` needs
a superuser. Everything it creates is in the `improv` schema: every function
is `improv.<name>()`, and the only object outside it is the procedural
language `improv` (languages have no schema). Each model's results go in
its own schema, `improv_<model>`.

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

## SQL API (schema `improv`, all `SECURITY INVOKER`)

| Function | Does |
| --- | --- |
| `create_model(name text) → bigint` | Creates the model and its result schema `improv_<lower(name)>`. |
| `bind_category(model, category, rel regclass, col text)` | Creates the category if missing and records that `rel.col` supplies its items. |
| `bind_measure(model, measure, rel regclass, value_col text, agg text DEFAULT NULL)` | Creates an input measure read from `rel.value_col`. Its dimensions are the categories already bound on `rel`, in category-id order. Two rows on one coordinate are an error unless `agg` (`sum`, `avg`, `min`, `max`, `count`) folds them; an aggregated measure is read-only in the GUI. |
| `define(model, definition text) → bigint` | Adds a derived measure from `Target = expr` and creates its result table. `SQL(...)` and `CALL(...)` are rejected. |
| `recalc(model) → bigint` | Recomputes every derived measure in the caller's transaction and returns the number of result rows inserted, updated or deleted. |
| `recalc(model, measures text[]) → bigint` | Recomputes only the named measures (input or derived, case-insensitive) and every derived measure that depends on them. `NULL` means all. |
| `set_sync_mode(model, mode text)` | `trigger`: installs the sync triggers on every bound relation and recomputes once (re-run it after binding a new relation). `commit`: recomputes once, then once per writing transaction before it commits; needs the preload. `logical`/`manual`: removes the triggers. |
| `what_if(model, edits jsonb) → TABLE(measure text, coord jsonb, value jsonb)` | Evaluates the model with `edits` applied in memory and returns the derived cells that differ from the stored results (`value` NULL: the cell would vanish). Writes nothing. |
| `enable_logical(model)` | Creates (or resets) the publication `improv_<model>` over the bound relations and gives PK-less ones `REPLICA IDENTITY FULL`. Called for you by `bind_measure` and `set_sync_mode(…, 'logical')`. The worker creates the slot. |
| `status(model) → TABLE(sync_mode, slot, lag_bytes, confirmed_lsn, last_applied_at, rows_applied, last_error)` | How the model is kept current, and how far the logical worker is behind. |
| `drop_model(name)` | Drops the catalog rows, the result schema, the triggers and the publication, and queues the slot for the worker to drop. |

Names must match `^[A-Za-z][A-Za-z0-9_]*$` (at most 56 characters, and not
`value`). Result tables are `improv_<model>.<lower(measure)>` with one `text`
column per dimension (named `lower(category)`), a typed `value` column, and a
primary key on the dimensions. A measure with no dimensions
(`MaxVol = MAX(Vol OVER Person)`) has a one-row table of just `value` (no row
while it has no cell).

`bind_measure` takes the measure's type from its column, and a derived
measure has the type its formula gives (`Price > 15` is boolean):

| Column type | Measure type | Result `value` column |
| --- | --- | --- |
| `float4`, `float8`, `numeric`, `int2`, `int4`, `int8` | number | `double precision` |
| `bool` | boolean | `boolean` |
| `text`, `varchar`, `bpchar` | text | `text` |
| `date`, `timestamp`, `timestamptz` | datetime (millisecond UTC) | `timestamptz` |

Any other column type is an ERROR. `agg => 'count'` is a number; `sum` and
`avg` need a number column. A measure's type cannot change: if a result
table exists with another `value` type, drop the model and rebuild it.
`what_if` edits and results are JSON of the measure's type: a number, a
bool, a string, or a datetime as an RFC 3339 string (`"2026-01-02T00:00:00+00:00"`).

`improv.max_cells` (default 5 000 000, settable per session) caps the number
of input cells `recalc` loads; above it, `recalc` raises an ERROR.

Bound data rules: a NULL value means no cell, and a row with a NULL dimension
is skipped. Two rows on the same coordinate are an ERROR unless the
measure was bound with `agg` (`sum`, `avg`, `min`, `max`, `count`), which
folds them into one cell. Item ids for bound values are minted once, in
lexical order, and are never reused.

## Writing SQL functions in Improv's language

`CREATE EXTENSION pg_improv` also installs `LANGUAGE improv`: SQL functions
whose bodies are Improv formulas.

```sql
CREATE FUNCTION margin(price float8, cost float8) RETURNS float8
  LANGUAGE improv IMMUTABLE AS $$ price - cost $$;
CREATE FUNCTION bonus(sales float8, target float8) RETURNS float8
  LANGUAGE improv AS $$
    Ratio = sales / target
    MAX2(Ratio - 1, 0) * 1000
  $$;
CREATE FUNCTION revenue(price float8, qty float8) RETURNS float8
  LANGUAGE improv AS 'price times qty';                -- controlled English
SELECT product, margin(price, cost) FROM sales;
```

- **Body.** Each SQL argument is an input measure with the argument's name;
  arguments must be named, and the body uses the name as Postgres stores it
  (unquoted names are folded to lower case: `(Sales float8)` is `sales`). Each `Name = expr` line defines a measure the
  following lines can use. The result is the last line: a bare expression,
  or the last definition. A line is the formula DSL or controlled English,
  as in the GUI's formula bar. Every non-aggregating built-in works (`IF`,
  `IFERROR`, `ROUND(x, 2)`, `LEFT`, …; see the guide's Function Reference).
- **Types.** `float8 float4 numeric int2 int4 int8` are Numbers (f64, so
  `numeric` and `int8` are exact to 2⁵³), `bool` is Boolean, `text varchar`
  are Text, and `date timestamp timestamptz` are DateTimes (millisecond
  precision). The return type is one of these. An integer return type rounds
  half to even, as Postgres's casts do. No sets or `OUT` arguments.
- **NULL.** A NULL argument is a missing input: the result is NULL when it
  depends on that argument (`margin(NULL, 1)`), and computed when it does not.
  There is no short-circuit (`TRUE OR NULL` is NULL), as in the engine.
- **Errors are NULL.** A result that is an error value (`#DIV/0!` from
  `1 / 0`, `#NUM!` from `SQRT(-1)`, `#N/A`, …) returns NULL; with
  `SET improv.raise_errors = on` it raises an ERROR naming it (`the formula
  evaluated to #DIV/0!`). `IFERROR(a / b, 0)` replaces it in the body.
- **No dimensions.** `SUM(x OVER Time)` and other aggregations or category
  references are rejected: a scalar function has no categories. Use a model.
- **Checking.** The validator rejects unnamed or unsupported arguments at
  `CREATE FUNCTION`, and parses and type-checks the body unless
  `check_function_bodies` is off (then the first call reports it).
- **Trusted.** Bodies compute and nothing else (no I/O, no catalog access),
  so any role with `CREATE` on a schema may write one.
- **Cost.** Each backend compiles a body once and caches it by function OID
  until `CREATE OR REPLACE` changes it. On a c7i.4xlarge (release build,
  `docs/reviews/bench/bench-plimprov.sh`), over a million rows: `margin` adds
  0.14 µs per call over the inline `x - x / 2`; `bonus` 0.24 µs; the same
  margin in PL/pgSQL 0.21 µs, and as a non-inlined SQL function 0.91 µs.

The evaluator is `improv_engine::scalar`, which calls the dataflow engine's
own per-cell operators, so a body gives the value a model would give at one
cell.

## Keeping results current: trigger vs commit vs logical vs recalc

**Is a result in the same transaction as the write that caused it?** In
`trigger` and `commit` modes and with `improv.recalc()`, yes. In `logical`
mode (the default), no: it follows shortly after the commit, in a
transaction of its own. In every mode a write that does not commit never
changes a result:

| A write that… | `trigger` | `commit` | `logical` |
| --- | --- | --- | --- |
| commits | results commit with it, atomically, visible to others at the same moment | the same; recomputed once, just before COMMIT (or PREPARE TRANSACTION) | results follow one poll later (~50–100 ms), all of one batch at once |
| is rolled back (`ROLLBACK`, an error, a lost connection) | its result changes roll back with it | nothing was recomputed | never seen: logical decoding sends committed transactions only |
| is undone by `ROLLBACK TO SAVEPOINT` | that part's result changes are undone; the rest commit | the undone part is forgotten; the rest recomputes at COMMIT | the undone part is never decoded; the committed rest is applied |
| is read back in the same transaction | **sees** its own new results | sees the old ones (call `improv.recalc()` to see new ones) | sees the old ones (call `improv.recalc()` to see new ones) |

In logical mode the worker applies a batch of committed transactions and
moves its replication slot forward **in one transaction**: all of a batch's
result writes and the slot position commit together or not at all. A crash
or a killed worker re-applies the batch, and the writes are idempotent, so
nothing is lost or counted twice. `tests/txn.sh` checks every row of the
table above, and `tests/logical.sh` checks crash recovery.

| | `trigger` (L1) | `commit` | `logical` (L2, default) | `recalc` (explicit) |
| --- | --- | --- | --- | --- |
| When results update | In the writer's statement, before it returns | In the writer's transaction, just before COMMIT | After the commit, by a background worker (eventual) | When you call it, in your transaction |
| Consistency | Transactional: commit both or neither; read-your-writes | Transactional: commit both or neither; reads before COMMIT see the old results | Lags the commit | Transactional, in your snapshot |
| Writer cost | Added latency and result-row locks on every write statement (batched per statement) | One full recompute of the written tables' dependents per transaction, and the result-row locks from then to the commit | None | None until you call it |
| On the bound tables | Four statement triggers each | Nothing | A publication; `REPLICA IDENTITY FULL` if no PK | Nothing |
| Requirements | None | The library preloaded (`shared_preload_libraries` or `session_preload_libraries`) | `wal_level=logical`, `REPLICA IDENTITY` (FULL or a PK) on bound tables | None |
| Failure mode | A failing recompute fails the writer's statement | A failing recompute aborts the COMMIT, naming the model | A stuck worker retains WAL (bounded by `max_slot_wal_keep_size`) | Errors are yours to handle |
| Best for | Small, read-your-writes models | Transactional results from many-statement transactions; tables you can't add triggers to | Busy tables, replicas | Bulk loads, migrations, `manual` mode |

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

### Commit mode

`set_sync_mode(model, 'commit')` installs nothing on the bound tables and
recomputes once, so the results start consistent. From then on, hooks in
every backend (`ExecutorFinish_hook` for INSERT, UPDATE, DELETE, MERGE and
writable CTEs; `ProcessUtility_hook` for TRUNCATE and `COPY FROM`) note
which relations each transaction wrote. Just before it commits (or
prepares), every commit-mode model bound to one of them, or to a partition
or parent of one, is locked, loaded and its written measures' dependents
recomputed in full, in the writer's transaction and as the writing role.
Work rolled back to a savepoint is forgotten, and a statement that changed
no row does not count. A transaction that wrote no commit-bound table pays
one set lookup: each backend caches the commit-bound relations, and
triggers on `improv.model` and `improv.binding` invalidate every backend's
copy when a model changes mode or binds a table.

The hooks have to be in every backend that writes, so the library must be
preloaded (`shared_preload_libraries`, or `session_preload_libraries` for
new sessions), and `set_sync_mode` refuses `commit` in a backend that
loaded it on demand. As with trigger mode, the role that enables commit
mode (or binds a table to a commit-mode model) needs the TRIGGER privilege
on each bound table, since the model then runs in other roles' commits, and
every role that writes a bound table must be able to write the result
tables. A recompute that fails (a writer without that privilege, or a model
over `improv.max_cells`) aborts the COMMIT, and the error names the model.

```sql
SELECT improv.set_sync_mode('plan', 'commit');
BEGIN;
UPDATE sales SET price = 11 WHERE product = 'Widget';
UPDATE sales SET qty = qty + 1 WHERE product = 'Gadget';
SELECT value FROM improv_plan.revenue WHERE product = 'Widget';  -- still the old results
COMMIT;                                                           -- one recompute, here
```

From `docs/reviews/bench/bench-pg-commit.sh` (c7i.8xlarge, release build,
10 000 input cells, medians of 9, wall time of the whole round trip): one
transaction of 100 single-row updates takes **82 ms** in commit mode and
6574 ms in trigger mode (a recompute per statement); a single one-row
UPDATE takes 77 ms in both.

### Logical mode (default)

A background worker (`pg_improv sync`, one per server, serving
`improv.database`) polls, for each model in `logical` mode, a logical
replication slot `improv_<model>` (plugin `pgoutput`) over the publication
`improv_<model>`. When committed source transactions touched a bound
relation it applies their row changes and writes the changed results **in
one worker transaction that also advances the slot**. A crash or
`pg_terminate_backend` therefore re-reads and re-applies (the writes are
idempotent upserts and deletes); nothing is lost or applied twice. Rolled-back source transactions
never appear. The worker restarts itself 5 s after it is killed.

The slot retains WAL until the worker confirms it: watch `lag_bytes` in
`improv.status`, and set `max_slot_wal_keep_size`. Slots are created by the
worker (Postgres forbids creating one in a transaction that has written), so
a new model in logical mode starts updating within one poll of being bound.

Apply strategy: incremental. The worker keeps each model loaded, with a
live engine (its own compute thread; it never calls Postgres). Decoded row
changes become input-cell edits: an INSERT sets a cell, a DELETE removes
one, an UPDATE sets it (or, when it moved a dimension value, removes the old
cell and sets the new one). A NULL value removes the cell; a row with a NULL
dimension is ignored, as `recalc` ignores it. The engine recomputes only
what the edits reach, and only the derived measures whose output changed
are diffed and written, so a one-row UPDATE rewrites only its own result
rows. The worker rebuilds (a full load and recompute, diffed against the
result tables) when the model's structure changed (`model_version` moved: a
new measure, binding or formula), on a `TRUNCATE`, when a change brings a
dimension value with no item yet (it mints the item), when two rows land on
one coordinate, and after any error. A rebuild costs what a `recalc` costs.
The loaded model counts against `improv.max_cells`; leaving logical mode or
dropping the model frees it.

After each batch the worker logs a standby snapshot
(`pg_log_standby_snapshot()`), so the slot's restart point keeps up and a
poll decodes only new WAL rather than everything since the bgwriter's last
snapshot (up to 15 s of it).

### Benchmark

From `docs/reviews/bench/bench-pg-modes.sh` on a c7i.4xlarge (Debian 13,
PG 17, release build; medians of 15; `improv.poll_ms = 20`). "Writer" is the
writer's UPDATE+COMMIT round trip; "fresh" is commit → result visible to
another session.

| cells | change | mode | writer | fresh |
| ---: | --- | --- | ---: | ---: |
| 10k | 1 row | logical | **8 ms** | **40 ms** |
| 10k | 1 row | trigger | 82 ms | 92 ms |
| 10k | 1 row | recalc in txn | 83 ms | 93 ms |
| 10k | 1k rows | logical | **15 ms** | **94 ms** |
| 10k | 1k rows | trigger | 172 ms | 182 ms |
| 10k | 1k rows | recalc in txn | 105 ms | 115 ms |
| 100k | 1 row | logical | **8 ms** | **52 ms** |
| 100k | 1 row | trigger | 1108 ms | 1118 ms |
| 100k | 1 row | recalc in txn | 1121 ms | 1131 ms |
| 100k | 1k rows | logical | **72 ms** | **559 ms** |
| 100k | 1k rows | trigger | 2406 ms | 2416 ms |
| 100k | 1k rows | recalc in txn | 1330 ms | 1341 ms |
| 1M | 1 row | logical | **8 ms** | 234 ms |
| 1M | 1k rows | logical | 468 ms | 6214 ms |

Logical mode keeps the writer fast and, with the incremental apply, now also
gives the freshest results: at 100k cells a one-row UPDATE is visible about
50 ms after its commit (it was 1161 ms with a full recompute per batch; 1k
rows: 559 ms, was 1552 ms). Trigger mode and `recalc` reload the model's
inputs per recompute (about 11 µs per cell), so they grow with the model.
The "1k rows" UPDATE (`product <= 'P1999'`, a text comparison) actually
matches ~11k rows at 100k cells and ~111k at 1M. At 1M cells its cost is the
engine absorbing ~111k edits (~1.5 s), decoding them (~1.5 s), and scanning
each changed measure's snapshot for the diff (`MODES=logical SIZES=1000000`;
the p95s are 2.0 s and 8.0 s).

## Worked example

`examples/family-finance/` models a household's books: statement
reconciliation per account and month, budgets and variances, net worth across
bank, card, retirement, 529, brokerage, crypto, foreign-currency and property
accounts, and savings-goal projections with what-if. `tests/finance.sh` runs
it and checks the numbers; the guide chapter is
[A family's finances](../../docs/src/postgres-finance.md).

## Catalog

The `improv.*` tables are a compatible superset of the catalog that
`improv_storage_pg` (the GUI/CLI's `postgres://` store) creates, so a model
created in SQL opens in the GUI and vice versa. The extension adds
`improv.category_binding` (category ⇄ column bindings) and, for logical
mode, `improv.sync_state` (each model's slot, progress and last error) and
`improv.slot_to_drop` (slots the worker drops after a model leaves logical
mode). A pg_test checks the superset property against
`improv_storage_pg::CATALOG_SQL`.
