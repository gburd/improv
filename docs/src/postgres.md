# Improv in Postgres (`pg_improv`)

`pg_improv` brings Improv's models to data that already lives in Postgres.
Your tables stay the source of truth for the data; the **structure** — the
categories, measures and formulas — is an Improv model stored in the `improv`
schema; the **results** of every derived measure land in ordinary tables
(`improv_<model>.<measure>`) that any SQL client can read, index, `GRANT` or
replicate. The same model opens in the desktop GUI.

## Five-minute tour

```ini
# postgresql.conf (then restart)
shared_preload_libraries = 'pg_improv'
wal_level = logical
improv.database = 'sales'
```

```sql
CREATE EXTENSION pg_improv;
CREATE TABLE sales (quarter text, product text, price float8, qty float8,
                    PRIMARY KEY (quarter, product));
INSERT INTO sales VALUES ('Q1','Widget',10,100), ('Q1','Gadget',20,30),
                         ('Q2','Widget',10,200), ('Q2','Gadget',20,60);

SELECT improv.create_model('plan');
SELECT improv.bind_category('plan', 'Time',    'sales', 'quarter');
SELECT improv.bind_category('plan', 'Product', 'sales', 'product');
SELECT improv.bind_measure ('plan', 'Price',    'sales', 'price');
SELECT improv.bind_measure ('plan', 'Quantity', 'sales', 'qty');
SELECT improv.define('plan', 'Revenue = Price * Quantity');
SELECT improv.define('plan', 'Total = SUM(Revenue OVER Time)');

SELECT * FROM improv_plan.revenue;     -- kept current from here on
```

```sh
improv-gui "postgres://me@localhost/sales?model=plan"
```

A migration can re-run `improv.define('plan', 'Revenue = …', replace =>
true)`: the same formula changes nothing (no version bump, no row
written), and a new one recomputes the measure and its dependents in the
same transaction, writing only the rows whose value changed. It is an
ERROR, naming the dependent, when the new formula would change the type a
dependent reads. `improv.drop_measure('plan', 'Total')` drops a measure
and its result table (an input's binding), and refuses while another
measure references it.

A named LAMBDA is defined the same way and called by any formula of the
model:

```sql
SELECT improv.define('merit', 'LOGNORM = LAMBDA(x, c, LN1P(x) / LN1P(MAX(x OVER c)))');
SELECT improv.define('merit', 'An = LOGNORM(Authored, Person)');
```

`c` is a category parameter: the call passes a category. The lambda lives
in `improv.lambda`. Redefining it with `replace => true` recomputes every
measure that calls it, as redefining a measure does;
`improv.drop_measure('merit', 'LOGNORM')` drops it once nothing calls it.
The GUI shares the table, so a lambda defined in either place is called
from the other.

The GUI shows the model as worksheet windows. Editing a formula there
redefines it in Postgres; editing an input cell updates the bound row of
`sales`; everyone's SQL view of `improv_plan.revenue` follows.

## Ways to keep results current

**Transactions.** In `trigger` and `commit` modes, and with
`improv.recalc()`, the results change inside the writer's transaction: other
sessions see them only once it commits, and `ROLLBACK`, an error or
`ROLLBACK TO SAVEPOINT` undo them with the change. `trigger` recomputes
after every write statement, so the writer reads its own new results.
`commit` recomputes once, just before `COMMIT`, so a read earlier in the
transaction sees the old results (call `improv.recalc('plan')` there to
read your writes). In `logical` mode the
results follow the commit in the worker's own transaction, and an aborted
write never reaches them at all, because logical decoding carries committed
transactions only. A batch's results and the model's recorded position
commit together, so a crash neither loses nor doubles a change. The worker
reads one replication slot per database (`improv_db`), however many models
there are, and routes each change to the models bound to its table.

| | `logical` (default) | `trigger` | `commit` | `improv.recalc()` |
| --- | --- | --- | --- | --- |
| Results update | after the commit, by a background worker | inside the writer's statement | inside the writer's transaction, just before `COMMIT` | when you call it |
| Consistency | eventual (lags the commit by one poll plus the change's own work) | transactional, read-your-writes | transactional; reads before `COMMIT` see the old results | transactional, in your snapshot |
| Writer pays | nothing (the worker applies only the changed cells) | the recompute, on every write statement | one recompute per transaction, of the written tables' dependents | the recompute, when you choose |
| On your tables | membership of the publication `improv_db` (and `REPLICA IDENTITY FULL` if no PK) | four statement triggers each | nothing | nothing |
| Needs | `wal_level = logical`, preload, a PK or `REPLICA IDENTITY FULL` (set for you) | nothing | preload | nothing |

Switch with `SELECT improv.set_sync_mode('plan', 'commit')` (or `'logical'`,
`'trigger'`, `'manual'`). Entering `trigger` or `commit` mode recomputes
once. `commit` mode needs `pg_improv` in `shared_preload_libraries` (or
`session_preload_libraries`), because the hooks that notice a write must be
in every backend; `set_sync_mode` refuses it otherwise. A recompute that
fails at `COMMIT` rolls the transaction back with an error naming the
model. `improv.recalc('plan')` works in every mode, and
`improv.recalc('plan', ARRAY['Price'])` recomputes only what depends on Price.
`improv.what_if('plan', '[{"measure":"Price","coord":{"Time":"Q1","Product":"Widget"},"value":12}]')`
returns the derived cells that would change, writing nothing. Its `value`
column is `jsonb`: a number, `true`/`false`, a string, or a date as an RFC
3339 string, and an edit's `"value"` takes the same forms.
`improv.status('plan')` shows the mode and, in logical mode, the slot's lag
and how the worker applied its batches: `incremental_applies` counts the
batches it fed to the live engine, `rebuilds` its full reloads. On an
append-only fact table `rebuilds` stays put while `incremental_applies`
grows.

## Several rows per coordinate

A fact table often has many rows per coordinate (one per order, say). Bind
the measure with an aggregate and each coordinate's rows are folded into
one input cell: `improv.bind_measure('plan', 'Amount', 'orders', 'amount',
agg => 'sum')` (also `avg`, `min`, `max`, `count`, and `count_distinct`, SQL's
`count(DISTINCT value_col)`; over a `bool` column, `min` and `max` are
SQL's `bool_and` and `bool_or`). Without `agg`, two rows on
one coordinate are an error. An aggregated measure is read-only from the
GUI: a save that edits one of its cells fails, so change the rows in SQL.

In `logical` mode the sync worker folds each change into the cell, and
the result is the aggregate a fresh `improv.recalc` computes, with these
cases:

- **Text `min` and `max`** order by the column's collation: `C` and
  `POSIX` by bytes, any other deterministic collation (libc, ICU,
  builtin) by Postgres's own compare.
- **Nondeterministic collations** (`CREATE COLLATION … deterministic =
  false`) can find texts with different bytes equal. Only Postgres can
  tell, so a text `min`, `max` or `count_distinct` over such a column
  reloads the model on every change (`rebuilds` grows, the log says
  `nondeterministic collation`). The result is still exact.
- **Sums of `float4` and `float8` columns** are the values' exact sum
  rounded once. Postgres adds the floats in scan order, so a fresh
  `sum` can differ in the last bits, as two scans of the same table can
  in Postgres itself. The fold's total does not depend on the order of
  the changes: inserting and deleting the same rows gives the same bits.
- **`numeric` values with more than 9 decimals**, or written with an
  exponent, are read as `float8` and summed exactly as floats. Integer
  columns, and `numeric` values with up to 9 decimals, sum exactly, as
  Postgres does.

## Value types

A measure's type comes from its column: numbers (`int*`, `float*`,
`numeric`), `bool`, text (`text`, `varchar`, `char`) and dates (`date`,
`timestamp`, `timestamptz`, kept to the millisecond in UTC). A derived
measure has the type of its formula, so `improv.define('plan', 'Pricey =
Price > 15')` makes `improv_plan.pricey` a `boolean` column. Result tables'
`value` columns are `double precision`, `boolean`, `text` or `timestamptz`.
Other column types are refused by `bind_measure`. A derived measure with
no dimensions (`MaxVol = MAX(Vol OVER Person)` over a measure of Person
alone) gets a one-row result table with just the `value` column; it has no
row while the formula has no cell.
Logical and trigger sync follow a table's row changes, so in those modes
only tables bind; a view binds in `manual` mode (refresh with
`improv.recalc()`).

**Choosing:** use `logical` for busy tables (writers stay fast; measured
at 100k cells: 8 ms per UPDATE and its results visible ~50 ms later, vs
~1.1 s with a trigger). Use `trigger` when a
writer must read derived results in its own transaction. Use `commit` when
results must commit with the write but a transaction makes many writes (100
one-row updates in one transaction at 10k cells: 82 ms, against 6.6 s in
trigger mode), or when you can't put triggers on the tables. Use `recalc`
after bulk loads or inside migrations.

## Functions written in Improv's language

`LANGUAGE improv` lets any role write a SQL function as an Improv formula:

```sql
CREATE FUNCTION bonus(sales float8, target float8) RETURNS float8
  LANGUAGE improv IMMUTABLE AS $$
    Ratio = sales / target
    MAX2(Ratio - 1, 0) * 1000
  $$;
SELECT rep, bonus(sales, target) FROM quota;
```

The SQL arguments are the inputs, by name; `Name = expr` lines define
intermediate measures; the last line is the result. Lines may be the DSL or
controlled English (`price times qty`). The semantics are the engine's: a
NULL argument gives NULL wherever the result depends on it, and an error
value (`#DIV/0!` from a division by zero, …) is NULL, or an ERROR with
`improv.raise_errors = on`. Aggregations need categories, so `SUM(x OVER Time)` is
rejected; put it in a model. A call costs about 0.14 µs for a one-line
body and 0.24 µs for `bonus` above. See
`crates/pg_improv/README.md` for the types and rules.

## Upgrades and backups

pg_improv supports Postgres 15–18. To upgrade, install the new release,
restart Postgres, then run `ALTER EXTENSION pg_improv UPDATE`. `pg_dump`
keeps the models and their results. A restored database's worker makes a
new replication slot and catches up with one recompute. Updating to 0.11.0
moves logical-mode models from one slot each (`improv_<model>`) to the
shared `improv_db`: the update drops the old publications, the worker
drops the old slots and catches each model up with one recompute.

## Privileges

A model reads its bound tables and writes its results as its **owner**,
the role that called `create_model` (`improv.model.owner`), in every sync
mode and in `recalc`, whoever writes the tables. Row-level security and
privileges therefore apply to a model as they do to its owner: its results
aggregate only the rows the owner may read, and are as visible as their
tables' grants. If the owner loses SELECT on a bound table, the next logical
apply fails with a `last_error` naming the columns; nothing is read as a
superuser. Only a role that may act as a model's owner (a superuser, or a
member of the owner with `SET`) may change the model, or its owner:
row-level security on the `improv.*` tables enforces it, by the API and
by direct DML alike, so no one can make a model read, as a role that sees
more than they do, what they cannot. Its names and formulas stay readable;
its items (the bound tables' keys) and input cells are readable by the same
roles only. `what_if` is theirs too: it computes as the owner, so its
results reveal what the owner can read. Give it to an analyst with a
`SECURITY DEFINER` wrapper the owner creates for one model, which grants
`what_if` on that model and nothing else, or by `GRANT alice TO analyst
WITH INHERIT FALSE, SET TRUE`, which also lets the analyst change alice's
models.

A non-superuser builds models as a member of **`improv_user`**, which the
extension creates: `GRANT improv_user TO alice`, plus `SELECT` (and, for
trigger and commit mode, `TRIGGER`) on the tables alice binds. No `CREATE`
on the database is needed. **`improv_admin`** grants and revokes
`improv_user` and acts on no one else's model. Acting on a model means
acting as its owner, so only a superuser acts on every model.
`improv.models` lists every model with its owner, sync mode, result
schema, and `may_change` for the caller. See the
[pg_improv README](https://codeberg.org/gregburd/improv/src/branch/main/crates/pg_improv/README.md#privileges)
for the mechanism, the wrapper's text, and the full privilege matrix
(superuser, `improv_admin`, owner, `SET` member, `improv_user`, writer,
`PUBLIC`), which `tests/privileges.sh` checks cell by cell.

## Durability

In logical mode the worker writes a model's share of a batch and records
that model's position in **one** transaction, and confirms the shared slot
once every model has applied the batch, so after a crash or
`pg_terminate_backend` it re-reads and re-applies only what was not
recorded; nothing is lost or counted twice, and rolled-back writes never
appear. A model that fails records `last_error` and rebuilds from its
tables on its next poll, so it never holds the other models' slot back. A
slot retains WAL until confirmed, so set `max_slot_wal_keep_size` and watch
`improv.status(model).lag_bytes`.

See `crates/pg_improv/README.md` for the full SQL API, limits
(`improv.max_cells`), and the benchmark table.
