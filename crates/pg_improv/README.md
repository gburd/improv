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
and `tests/upgrade.sh` updates a live database from every earlier release,
with a model in each sync mode, on each supported Postgres major.

**Backups:** `pg_dump` keeps your models (the `improv.*` catalog is
registered with `pg_extension_config_dump`) and their result tables, and
`pg_restore` (with or without `--single-transaction`) or `psql -f` of a
plain dump brings them back in every sync mode; `tests/txn.sh` restores a
model of each mode both ways and checks each follows writes afterwards.
Restore the roles first (`pg_dumpall --globals-only`): a model's owner is
dumped by name, and its catalog row does not load without that role. A
restored logical-mode model's worker creates a fresh
replication slot and catches up with one recompute. While a restore loads
the data, a commit-mode model's result tables have no primary key yet (it
is added after the data), so its recompute at each COMMIT is skipped with a
NOTICE; its results come from the dump, and the next write recomputes.
Before 0.9.0 a dump restored every model empty.


For the default `logical` mode, preload the library and enable logical
decoding (then restart Postgres):

```ini
# postgresql.conf
shared_preload_libraries = 'pg_improv'
wal_level = logical
improv.database = 'sales'       # the database the sync worker serves (default: postgres)
#improv.poll_ms = 200           # how often it polls its slot (reloadable)
#improv.max_changes = 10000     # most decoded changes per poll (reloadable)
#improv.max_slot_lag = 4GB      # past this the worker drops its slot and rebuilds (reloadable; 0 = never)
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
| `define(model, definition text, replace bool DEFAULT false) → bigint` | Adds a derived measure from `Target = expr` and creates its result table, or a named LAMBDA from `Name = LAMBDA(p, …, body)` (see below). `SQL(...)` and `CALL(...)` are rejected. An existing name is an ERROR unless `replace => true`, which redefines the derived measure (see below). Returns the measure's (or lambda's) id. |
| `drop_measure(model, measure text)` | Drops a measure: a derived one's result table, an input's binding and stored cells, and its catalog row; or a named LAMBDA. An ERROR names the measures (and lambdas) that still reference it. |
| `recalc(model) → bigint` | Recomputes every derived measure in the caller's transaction and returns the number of result rows inserted, updated or deleted. |
| `recalc(model, measures text[]) → bigint` | Recomputes only the named measures (input or derived, case-insensitive) and every derived measure that depends on them. `NULL` means all. |
| `set_sync_mode(model, mode text)` | `trigger`: installs the sync triggers on every bound relation and recomputes once (re-run it after binding a new relation). `commit`: recomputes once, then once per writing transaction before it commits; needs the preload. `logical`/`manual`: removes the triggers. |
| `what_if(model, edits jsonb) → TABLE(measure text, coord jsonb, value jsonb)` | Evaluates the model with `edits` applied in memory and returns the derived cells that differ from the stored results (`value` NULL: the cell would vanish). Writes nothing. When every edit has the same categories and the measures they feed are element-wise, it loads and compares only the edited coordinates, as trigger mode does. Only a role that may act as the model's owner may call it, and it reads as the owner (see [Privileges](#privileges), which shows how to give it to other roles). |
| `enable_logical(model)` | Adds the model's bound relations to the database's publication `improv_db` and gives PK-less ones `REPLICA IDENTITY FULL`. Called for you by `bind_measure` and `set_sync_mode(…, 'logical')`. The worker creates the slot. |
| `sync_publication()` | Sets `improv_db` to the union of the relations every logical-mode model binds (creating it when missing). Called for you by every function that changes that set; run it by hand to repair a publication edited by hand. |
| `status(model) → TABLE(sync_mode, slot, lag_bytes, confirmed_lsn, last_applied_at, rows_applied, last_error, incremental_applies, rebuilds)` | How the model is kept current, how far the logical worker is behind, and how it applied its batches: `incremental_applies` to the live engine, `rebuilds` full loads (the first catch-up, structural changes, and what it could not apply incrementally). |
| `drop_model(name)` | Drops the catalog rows, the result schema and the triggers, and takes relations no other logical-mode model binds out of `improv_db`. |

**Redefining.** `define(model, 'Merit = …', replace => true)` is safe to
re-run from a migration. When the formula parses to the one the measure
has (spacing and case do not matter), it changes nothing: no
`model_version` bump, no result row written. A new formula is stored and
the measure and everything that depends on it are recomputed at once, in
the caller's transaction, in every sync mode; only the result rows whose
value changed are written. The replacement is an ERROR, naming the
dependent, when another measure reads the target and the new formula
changes its value type, when it would change a dependent's type or
dimensions, or when it would read one of its own dependents. A
replacement that changes the target's dimensions (with no dependent in
the way) recreates its result table.

**Named LAMBDAs.** `define(model, 'LOGNORM = LAMBDA(x, c, LN1P(x) /
LN1P(MAX(x OVER c)))')` stores a named LAMBDA in `improv.lambda`; every
formula of the model can then call it, `An = LOGNORM(Authored, Person)`.
A parameter the body uses as a category (`OVER c`) takes a category at
the call. A body reads only its parameters (no measures). Redefining a
lambda (`replace => true`) works as redefining a measure: the same
definition is a no-op, and a new one recomputes every measure that calls
it, at once, writing only the rows whose value changed; it is an ERROR,
naming the measure, when it would break one or change its type or
dimensions. `drop_measure` drops a lambda by name once no measure or
other lambda calls it.

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

Any other column type is an ERROR. `agg => 'count'` and `'count_distinct'`
are numbers; `sum` and `avg` need a number column. A derived measure's type
changes only through `define(…, replace => true)`, which recreates its table;
a result table found with another `value` type is an ERROR.
`what_if` edits and results are JSON of the measure's type: a number, a
bool, a string, or a datetime as an RFC 3339 string (`"2026-01-02T00:00:00+00:00"`).

`improv.max_cells` (default 5 000 000, settable per session) caps the number
of input cells any load of a model holds: `recalc`, `define`, a trigger-mode
statement's or commit-mode transaction's recompute (which then fails the
statement or aborts the COMMIT), and the sync worker's rebuilds and applies
(the model then records a `last_error`). Above it, the load raises an
ERROR.

Bound data rules: a NULL value means no cell, and a row with a NULL dimension
is skipped. Two rows on the same coordinate are an ERROR unless the
measure was bound with `agg` (`sum`, `avg`, `min`, `max`, `count`, or
`count_distinct`, SQL's `count(DISTINCT value_col)`), which folds them into
one cell. Item ids for bound values are minted once, in lexical order, and
are never reused.

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
rewrites only the touched coordinates: each binding is read under a `WHERE`
on the touched dimension values, so an aggregated binding loads only those
coordinates' groups (`GROUP BY` over the matching rows). Otherwise it
recomputes those measures in full. Unchanged result rows are never
rewritten. `TRUNCATE` recomputes the dependents in full. New dimension
values mint items, as `recalc` does.

Commit mode recomputes the written relations' dependents in full at each
COMMIT. It has no transition tables to name the touched rows, and
capturing them would need a row trigger (or a replica-identity read) on
the user's tables, which commit mode exists to avoid; use trigger or
logical mode for a large aggregated fact table.

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
recomputed in full, in the writer's transaction and as the model's owner
(see [Privileges](#privileges)).
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
on each bound table, since the model then runs in other roles' commits.
The writers need no privilege on the model: it reads and writes as its
owner. A recompute that fails (the owner lost its SELECT, or a model over
`improv.max_cells`) aborts the COMMIT, and the error names the model.

Commit mode reloads the written tables' measures in full (it knows which
tables a transaction wrote, not which rows): 2.1 s per commit on the 2.5M-row
benchmark model, against 0.36 s per 1,000-row statement in trigger mode and
about 0.1 s in logical mode. Pick it for small or unaggregated models.

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
`improv.database`) polls one logical replication slot, `improv_db` (plugin
`pgoutput`), over the publication `improv_db`, which holds every relation
any logical-mode model binds. Only that database has a worker. In any other
database, `set_sync_mode(…, 'logical')` and `bind_measure` on a logical model
fail with a hint naming `improv.database`. A model that is logical there
anyway (for example, from a restored dump) shows `last_error` = "no sync
worker serves this database" in `improv.status`. Use `commit`, `trigger` or
`manual` mode there instead. Each poll decodes the slot once and routes
each committed transaction's row changes, by relation, to the models bound
there; models with nothing in the batch cost nothing. Each model applies
its share and writes its changed results **in one worker transaction that
also records the model's position** (`improv.sync_state.confirmed_lsn`), and
skips transactions at or before it. When every model has applied the batch,
the worker advances the slot to its end. A crash or `pg_terminate_backend`
therefore re-reads and re-applies only what no model recorded (the writes
are idempotent upserts and deletes); nothing is lost or applied twice.
Rolled-back source transactions never appear. The worker restarts itself 5
s after it is killed.

A model that fails (an error in its transaction: `last_error` in
`improv.status` says why) drops its live state, and its next poll rebuilds
it from its tables, which needs none of the WAL it skipped. So a failing
model never holds the slot, or the other models, back: it is retried every
poll and catches up in one rebuild once it works again.

The slot retains WAL until the worker confirms it: watch `lag_bytes` in
`improv.status`, and set `max_slot_wal_keep_size`. The slot is created by
the worker (Postgres forbids creating one in a transaction that has
written), so the first model in logical mode starts updating within one
poll of being bound; a model that joins later is caught up by one rebuild
from its tables, and each model's rebuild reads its fold state with its own
snapshot, so it skips exactly the transactions its load already saw. With no
logical-mode model left the worker drops the slot.

A bound table may be partitioned (by range, list or hash). The publication
sends a partition's changes as its root's (`publish_via_partition_root`),
and the worker gives each partition the replica identity it needs.
Attaching, detaching or dropping a partition, or truncating one, moves rows
without a row change in the WAL, so the model rebuilds on its next poll.
Bind the root, not a partition: while another logical-mode model binds the
root, a model bound to one of its partitions misses that partition's
changes.

When `max_slot_wal_keep_size` is exceeded (say the worker was down too
long), Postgres invalidates the slot and removes the WAL it held. From then
on `improv.status` reports it in `last_error`. Once the worker runs, it
drops the slot, makes a new one, logs that once, and rebuilds every model
from its tables; each model's `last_error` clears when its rebuild
succeeds. Any other failure to read the slot also appears as every model's
`last_error`, not only in the server log.

`improv.max_slot_lag` (default 4 GB, 0 turns it off) caps the WAL a running
but slow worker retains. When the slot's restart point falls further
behind, the worker drops the slot and rebuilds every model, the same as for
an invalidated slot. While the worker is down only `max_slot_wal_keep_size`
caps the slot, so set both. When the worker cannot make its slot
(`max_replication_slots` all in use, say), each model's `last_error` gives
the reason. The worker logs it once and keeps polling, and it makes the slot
as soon as one is free.

Before 0.11.0 each model had its own slot and publication, `improv_<model>`,
and every poll decoded the WAL once per model. `ALTER EXTENSION pg_improv
UPDATE` to 0.11.0 drops those publications and queues their slots; the
worker drops the slots, creates `improv_db`, and catches each model up with
one rebuild.

Apply strategy: incremental. The worker keeps each model loaded, with a
live engine (its own compute thread; it never calls Postgres). Decoded row
changes become input-cell edits: an INSERT sets a cell, a DELETE removes
one, an UPDATE sets it (or, when it moved a dimension value, removes the old
cell and sets the new one). A NULL value removes the cell; a row with a NULL
dimension is ignored, as `recalc` ignores it. The engine recomputes only
what the edits reach, and only the derived measures whose output changed
are diffed and written, so a one-row UPDATE rewrites only its own result
rows. A measure bound with `agg` keeps per-coordinate fold state: row
counts, an exact running total for `sum` and `avg` (integer and
`numeric` columns sum exactly), and each value's row count for `min`, `max`
and `count_distinct`, so deleting one of two rows that share a key keeps
the count. Each change takes its old row out of its coordinate's fold and
puts its new row into its (maybe other) coordinate's; the relation gets
`REPLICA IDENTITY FULL` so its old rows carry their values. A `TRUNCATE`
removes the relation's cells. A dimension value with no item yet is minted
through `improv.mint_items` in the worker's transaction (the same ids, in
the same order, as a `recalc` would mint), and its cells go to the live
engine as edits.

The worker rebuilds (a full load and recompute, diffed against the result
tables) when:

- the model's structure changed (`model_version` moved: a new measure,
  binding or formula);
- a new item lands in a category whose item list a formula reads:
  `POSITION`, `@ITEM`, `@ITEMCOUNT`, `PREV`/`NEXT` and recurrences along
  it, a window (`OVER T[FIRST..THIS]`), a per-item or `INTO` reduction
  (`RANK`, `SORT`), `FILL` over it (one cell per item combination),
  `RELABEL`/`SWAP` of it (item maps), or any reduction over it other than
  `SUM`, `AVERAGE`, `MIN`, `MAX`, `PRODUCT`, `SUMSQ`, `AND`, `OR`, `XOR`,
  `COUNT`, `COUNTA` and `COUNTUNIQUE` (those read only their rows'
  values, so a new item reaches them as data; so does `COALESCE`);
- two rows land on one coordinate of a measure bound without `agg`;
- a text `min` or `max` binding changes (text order is the collation's);
- or after any error.

A rebuild costs what a `recalc` costs.
The loaded model counts against `improv.max_cells`; leaving logical mode or
dropping the model frees it.

After each poll that applied a batch the worker logs one standby snapshot
(`pg_log_standby_snapshot()`), so the slot's restart point keeps up and a
poll decodes only new WAL rather than everything since the bgwriter's last
snapshot (up to 15 s of it). With the bound tables idle it also moves the
slot past unrelated WAL once it lags by 16 MB.

An idle worker does not decode. Each poll first compares the WAL insert
position with where its last decode ended, and decodes only when something
was written since. Postgres logs "starting logical decoding" and "found
consistent point" at LOG for every decode, so the worker decodes with
`log_min_messages = fatal` and logs its own errors.

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

An aggregated fact table, from `ROWS=2500000 BATCHES=1000 tests/scale.sh`
(c7i.8xlarge, release build; PG 17 Debian, PG 18 PGDG in `rust:1.97-trixie`):
2.5M contribution rows bound with `agg => 'count'` and `'count_distinct'`
into 44,750 cells of Person × Year × Release × Scope.

| | PG 17 | PG 18 |
| --- | ---: | ---: |
| `REFRESH MATERIALIZED VIEW CONCURRENTLY` of the same `GROUP BY` | 5.4 s | 4.0 s |
| `improv.recalc` (load, compute, write 2 × 44,750 rows) | 6.4 s | 4.9 s |
| worker's initial build (slot, load, fold seed, live engine) | 9.7 s | 8.1 s |
| 1,000-row INSERT batch, commit → visible (incl. a new Person) | 88–150 ms | 82–114 ms |

`improv.status` shows no rebuild for the batches, and after 1,000 random
INSERT / UPDATE / DELETE batches `count` and `sum` equal a fresh `recalc`
and `count_distinct` equals `count(DISTINCT …)`.

### Standbys and failover

The result tables are ordinary tables, so a physical standby has them, and
`improv.status` works there (`lag_bytes` is measured from the replay
position). No sync worker runs on a hot standby: it starts once recovery
ends. A logical slot does not survive a promotion unless it is a failover
slot (PG 17+), so there are two cases.

- **PG 15 and 16, or no slot sync.** After a promotion the worker on the new
  primary finds no `improv_db` slot. It makes one and rebuilds every model
  from its tables (one `recalc` each). Nothing to configure.
- **PG 17+ with slot sync.** The worker makes `improv_db` with
  `failover => true`. A standby that syncs slots keeps it in step, and after
  a promotion the worker continues on it with every model's recorded
  progress (it loads each model into memory, as at any worker start). On the
  standby, set `sync_replication_slots = on` and `hot_standby_feedback = on`,
  use a physical slot (`primary_slot_name`), and put `dbname=` in
  `primary_conninfo`. On the primary, set `synchronized_standby_slots` to
  that physical slot, so the worker never confirms WAL the standby lacks. A
  standby made with `pg_basebackup` copies that setting: clear it when you
  promote, or logical decoding on the new primary waits for a slot that does
  not exist.

The worker makes `improv_db` a failover slot on PG 17+ (0.11.0 is the
first release with `improv_db`). A slot made on PG 15 or 16 and carried
over by `pg_upgrade` is not one. Drop it with
`SELECT pg_drop_replication_slot('improv_db')`; the worker makes a new
failover slot on its next poll and rebuilds each model once.
`tests/failover.sh` checks both cases on PG 17 and 18.

### pg_upgrade

Install pg_improv into both majors first (`cargo pgrx install --release
-c <pg_config>`, once per major), with the same `shared_preload_libraries`,
`wal_level = logical` and `improv.*` settings in the new cluster's
`postgresql.conf`. The sync worker exits at once in binary-upgrade mode, so
it holds no slot while `pg_upgrade` runs.

**From PG 17 or later.** `pg_upgrade` carries the `improv_db` slot when the
slot has caught up. It refuses a slot with a transaction past its confirmed
position ("has not consumed the WAL yet"). Before stopping the old cluster,
stop the writers to the bound tables and let the worker apply their last
transactions: `lag_bytes` in `improv.status` stops moving and
`last_applied_at` is after the last write. An idle worker writes no WAL, so
the slot then stays caught up. Run `pg_upgrade --check`, then `pg_upgrade`.
On the new cluster the worker continues on the carried slot with each
model's progress. Trigger, commit and manual models need nothing.

**From PG 15 or 16** (`pg_upgrade` does not carry logical slots, and
refuses a cluster that has one): drop the slot on the old cluster,
`SELECT pg_drop_replication_slot('improv_db')`, while the worker is not
holding it, for instance with `improv.database` pointed at another database
and a restart. Upgrade, then start the new cluster: the worker makes a new
slot and rebuilds each logical-mode model from its tables (one `recalc`
each). The same works from PG 17 if you would rather not wait for the slot
to catch up.

Either way, check afterwards: `SELECT improv.recalc(name) FROM improv.model`
returns 0 for every model whose results are current. `tests/pg_upgrade.sh`
upgrades a PG 17 cluster with a model in each sync mode to PG 18.

### Removing pg_improv

The clean order is `improv.drop_model(name)` for each model, then
`DROP EXTENSION pg_improv`. `drop_model` removes the model's result schema
and takes its tables out of the publication. The worker drops the slot once
no logical-mode model is left.

A bare `DROP EXTENSION pg_improv CASCADE` cannot drop the replication slot,
which would retain WAL forever. The sync worker notices that the extension is
gone and drops the `improv_db` slot and publication itself, within a poll,
and logs this once. It leaves two things in place:

- the `improv_<model>` result schemas, which are user data, kept
  deliberately (`DROP SCHEMA improv_<model> CASCADE` removes one);
- `REPLICA IDENTITY FULL`, which logical mode gave each bound table that has
  no primary key, or that an aggregated binding reads (`ALTER TABLE …
  REPLICA IDENTITY DEFAULT` restores it).

## Worked example

`examples/family-finance/` models a household's books: statement
reconciliation per account and month, budgets and variances, net worth across
bank, card, retirement, 529, brokerage, crypto, foreign-currency and property
accounts, and savings-goal projections with what-if. `tests/finance.sh` runs
it and checks the numbers; the guide chapter is
[A family's finances](../../docs/src/postgres-finance.md).

## Privileges

A model acts as its **owner**, `improv.model.owner`: the role that called
`create_model`. Every read of its bound tables and every write of its
result tables runs as that role, whoever triggers it: the sync worker
(which connects as the bootstrap superuser), a writer's trigger or commit,
`define`, or `recalc`. So a model's results aggregate **what its owner can
read**: row-level security policies and column privileges apply to the
model as they apply to the owner, and a superuser's `recalc` publishes no
row the owner could not see. The results are then as visible as their
tables' grants: the owner owns them and grants them like any table.

The switch is `SetUserIdAndSecContext(owner, SECURITY_RESTRICTED_OPERATION)`
(as `REFRESH MATERIALIZED VIEW` runs as the view's owner) with
`search_path = pg_catalog, pg_temp`, restored afterwards even on error. A
writer firing a trigger or committing is not a member of the owner, so
`SET ROLE` would not work there; the restriction also stops a function the
owner controls (a policy, a default) from changing the caller's session.
In a writer's trigger or commit the catalog lookups (`improv.*`) run as
the bootstrap superuser, so writers need no privilege on schema `improv`.

`what_if` needs the caller to be able to **act as the owner** (below), and
reads the bound and result tables as the owner, as `recalc` does: it
computes what the owner would see, so its deltas come from the edits alone.
Anyone else gets "permission denied for improv model "m": what_if needs to
act as its owner, alice (see Privileges)". To give it to other roles, see
[Giving others `what_if`](#giving-others-what_if).

In logical mode the decoded WAL carries every row, so the worker checks
the owner may still read each bound column before applying a batch; when
it may not, the model fails with a `last_error` that names the role and
the columns (it is not read as the superuser), and resumes once the
privilege is back. A bound table whose rows a policy filters for the owner
is not applied row by row from the WAL either: each batch that touches it
is applied by a rebuild, which reads the table as the owner (as costly as
a `recalc`). `tests/privileges.sh` checks all of this in every sync mode.

Models made before 0.11.0 get the owner of their result schema (the role
that created them) at `ALTER EXTENSION pg_improv UPDATE`, which also hands
it their result tables; a model without one gets the bootstrap superuser,
with a NOTICE.

**Roles: what to grant a builder.** `CREATE EXTENSION` makes two roles when
they are missing, both `NOLOGIN`. Roles belong to the cluster, so a second
database's install reuses them (and grants them that database's catalog),
and `DROP EXTENSION` keeps them and their members (`DROP ROLE` removes
them once no database's extension uses them).

- **`improv_user`** builds models. It has `USAGE` on schema `improv`,
  `EXECUTE` on the API, the catalog DML (`SELECT`, `INSERT`, `UPDATE`,
  `DELETE` on the per-model `improv.*` tables, which row-level security
  then confines to the models it may act as the owner of), and `SELECT` on
  `improv.models`. Its result schemas are made by the extension and owned
  by it, so a builder needs no `CREATE` on the database. A builder still
  needs `SELECT` on each table it binds, and `TRIGGER` on it for trigger
  and commit mode.
- **`improv_admin`** enrolls builders: it holds `improv_user` `WITH ADMIN
  OPTION`, so it may `GRANT improv_user TO r` and `REVOKE` it, and builds
  models itself. It acts on no one else's model. Acting on a model is
  acting as its owner (its reads and writes run as the owner), so a role
  that acted on every model would act as every owner, superusers among
  them, through any model. A policy clause `OR pg_has_role(current_user,
  'improv_admin', 'MEMBER')` would hand that to every admin, quietly and
  without `BYPASSRLS`. Only a superuser acts on every model, and handing a
  model to another role is `UPDATE improv.model SET owner`, by the rule below.

```sql
GRANT improv_user TO alice;                 -- as a superuser or an improv_admin
GRANT SELECT, TRIGGER ON sales TO alice;    -- the tables alice binds
```

**`improv.models`** lists every model with its `owner`, `sync_mode`,
`result_schema`, and `may_change`, which is true when the caller may act
as the owner (change it, read its items and cells, `what_if` it). It
follows the catalog's read policy: anyone who may read `improv.model` (an
`improv_user`, or a role granted it) sees every model's name, as `status`
does. Whether they may read the results depends on the result tables'
grants.

```sql
SELECT name, owner, sync_mode, may_change, result_schema FROM improv.models;
```

**Who may change a model.** Only a role that may **act as its owner**: a
superuser, or a member of the owner with the right to `SET ROLE` to it on
PG 16+ (any member on PG 15), the rule `ALTER TABLE … OWNER TO` uses
(`improv.may_act_as`). Otherwise anyone who may write the catalog could
bind a table they cannot read into another role's model, or redefine a
result its owner shared, and read the table through it. Row-level security
on every `improv.*` table enforces it, through the API functions (all
`SECURITY INVOKER`) and by direct DML alike:

| | anyone with USAGE on `improv` | who may act as the owner |
|---|---|---|
| read a model's names, bindings, formulas, `status` | yes | yes |
| read its items (`improv.item`: the bound tables' keys), input cells (`improv.cell`) and state (`improv.meta`) | no (0 rows) | yes |
| `bind_*`, `define`, `drop_*`, `set_sync_mode`, `recalc`, `drop_model` | no ("permission denied for improv model") | yes |
| `INSERT`, `UPDATE`, `DELETE` its catalog rows | no (refused, or 0 rows) | yes |
| `what_if` | no ("permission denied … what_if needs to act as its owner"); a wrapper the owner grants (below) | yes, computing as the owner |

A writer needs none of this: a writer's trigger and commit read the
catalog as the bootstrap superuser, and the sync worker is one, so any
role that may write a bound table drives every sync mode, and the model
reads the table as its owner.

**The privilege matrix.** What each role may do with one model, `x`, which
`alice` owns. `tests/privileges.sh` checks every cell (its section 7)
on each Postgres it runs on. The columns:

- **superuser**: a superuser.
- **improv_admin**: a member of `improv_admin`.
- **owner**: `alice`.
- **SET member**: a member of `alice` with `SET` (`GRANT alice TO carol
  WITH INHERIT FALSE, SET TRUE`; any member on PG 15), who is an
  `improv_user`.
- **improv_user**: an `improv_user` who is not the owner.
- **writer**: a role that may write a table `x` binds, with no grant on
  `improv`.
- **PUBLIC**: a role with no grants.

"no" means refused ("permission denied …") or, for a read of the
catalog, no rows.

| | superuser | improv_admin | owner | SET member | improv_user | writer | PUBLIC |
|---|---|---|---|---|---|---|---|
| `create_model` (a model of one's own) | yes | yes | yes | yes | yes | no | no |
| `bind_category`, `bind_measure` | yes | no | yes | yes | no | no | no |
| `define` | yes | no | yes | yes | no | no | no |
| `recalc` | yes | no | yes | yes | no | no | no |
| `what_if` | yes | no | yes | yes | no¹ | no¹ | no¹ |
| `set_sync_mode` | yes | no | yes | yes | no | no | no |
| `drop_model` | yes | no | yes | yes | no | no | no |
| read names and formulas (`improv.measure`, `improv.models`) | yes | yes | yes | yes | yes | no | no |
| read items (`improv.item`) and input cells (`improv.cell`, the same policy) | yes | no | yes | yes | no | no | no |
| read results (`improv_x.*`) | yes | no² | yes | no³ | no² | no² | no² |
| `status` | yes | yes | yes | yes | yes | no | no |
| drive sync (write a bound table) | ⁴ | ⁴ | ⁴ | ⁴ | ⁴ | yes | ⁴ |

1. A `SECURITY DEFINER` wrapper the owner grants gives `what_if` on one
   model to any role, with no grant on `improv` (see [Giving others
   `what_if`](#giving-others-what_if)).
2. Until the owner grants it: results are ordinary tables the owner owns,
   so `GRANT USAGE ON SCHEMA improv_x` and `GRANT SELECT ON improv_x.w`
   give them to any role.
3. On PG 16+, a member with `INHERIT FALSE` reads them after `SET ROLE
   alice`. On PG 15 a member inherits, so the answer is yes.
4. By design, not a cell: it takes the privilege to write the table, which
   is the table's grant, not pg_improv's. Every role that has it drives
   every sync mode, and the model reads the table as its owner (sections 1
   and 4 of the test).

**Who may set a model's owner.** The same rule, for the current and the
new owner (a trigger, `improv.check_owner`): "only a superuser or a member
of role X may make it a model's owner" (or "… may change the owner of
model M"). `create_model` makes the caller the owner, which always passes;
a direct `INSERT` naming another role is checked the same way. Change a
model's owner with `UPDATE improv.model SET owner = 'role' WHERE name =
'model'`, and its result tables' with `ALTER TABLE
improv_<model>.<measure> OWNER TO role` (and `ALTER SCHEMA improv_<model>
OWNER TO role`).

### Giving others `what_if`

`what_if` reveals what its owner can read: the deltas are computed from
the owner's rows, at the owner's items. Granting it is granting that.
There are two ways to do it:

**(a) Membership in the owner, with SET.** On PG 16+:

```sql
GRANT alice TO analyst WITH INHERIT FALSE, SET TRUE;   -- PG 15: GRANT alice TO analyst;
```

`analyst` may then call `improv.what_if` on every model `alice` owns. But
anyone who may act as the owner may also **change** the owner's models
(`define`, `bind_*`, `recalc`, `drop_model`) and `SET ROLE alice`, so
this grants everything `alice` may do. Use it for co-owners.

**(b) A wrapper the owner creates, for one model.** As `alice`:

```sql
CREATE SCHEMA alice_api;
CREATE FUNCTION alice_api.what_if_plan(edits jsonb)
RETURNS TABLE (measure text, coord jsonb, value jsonb)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, pg_temp
AS $$ SELECT w.measure, w.coord, w.value FROM improv.what_if('plan', edits) AS w $$;
REVOKE ALL ON FUNCTION alice_api.what_if_plan(jsonb) FROM PUBLIC;
GRANT USAGE ON SCHEMA alice_api TO analyst;
GRANT EXECUTE ON FUNCTION alice_api.what_if_plan(jsonb) TO analyst;
```

The function runs as `alice`, its owner, so it passes `what_if`'s check.
`analyst` gets `what_if` on `plan` and nothing else: it cannot read
`plan`'s items or cells, change it, or reach another model, and it needs
no privilege on schema `improv`. The edits are the caller's input. The
model name is fixed in the body, so the wrapper must not take the model
as an argument. The pinned `search_path` keeps a caller's schemas out of
the definer's call. `REVOKE … FROM PUBLIC` matters because functions are
executable by PUBLIC by default. Revoke `EXECUTE` to take it back.
Prefer (b) for read-only analysts. `tests/privileges.sh` runs this
function as a role with `EXECUTE` alone.

**Restores.** A restore re-inserts every catalog row with row-level
security and the owner trigger on. Restore as a superuser (as usual). A
role that may act as the owner of every model may restore them too, from
a dump made with `pg_dump --enable-row-security --inserts` (`COPY FROM`
does not support row-level security); a role that may not is refused.

## Catalog

The `improv.*` tables are a compatible superset of the catalog that
`improv_storage_pg` (the GUI/CLI's `postgres://` store) creates, so a model
created in SQL opens in the GUI and vice versa. The extension adds
`improv.model.owner` (the role that created the model, as which it reads
its tables and writes its results),
`improv.category_binding` (category ⇄ column bindings) and, for logical
mode, `improv.sync_state` (each model's slot, progress and last error) and
`improv.slot_to_drop` (old per-model slots the worker drops after the
update to 0.11.0). A pg_test checks the superset property against
`improv_storage_pg::CATALOG_SQL`.
