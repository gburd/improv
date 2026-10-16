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

The GUI shows the model as worksheet windows. Editing a formula there
redefines it in Postgres; editing an input cell updates the bound row of
`sales`; everyone's SQL view of `improv_plan.revenue` follows.

## Three ways to keep results current

**Transactions.** In `trigger` mode, and with `improv.recalc()`, the results
change inside the writer's transaction: it reads its own new results, other
sessions see them only once it commits, and `ROLLBACK`, an error or
`ROLLBACK TO SAVEPOINT` undo them with the change. In `logical` mode the
results follow the commit in the worker's own transaction, and an aborted
write never reaches them at all, because logical decoding carries committed
transactions only. A batch's results and the slot's position commit
together, so a crash neither loses nor doubles a change.

| | `logical` (default) | `trigger` | `improv.recalc()` |
| --- | --- | --- | --- |
| Results update | after the commit, by a background worker | inside the writer's statement | when you call it |
| Consistency | eventual (lags the commit by one poll plus the change's own work) | transactional, read-your-writes | transactional, in your snapshot |
| Writer pays | nothing (the worker applies only the changed cells) | the recompute, on every write statement | the recompute, when you choose |
| Needs | `wal_level = logical`, preload, a PK or `REPLICA IDENTITY FULL` (set for you) | nothing | nothing |

Switch with `SELECT improv.set_sync_mode('plan', 'trigger')` (or `'logical'`,
`'manual'`). `improv.recalc('plan')` works in every mode, and
`improv.recalc('plan', ARRAY['Price'])` recomputes only what depends on Price.
`improv.what_if('plan', '[{"measure":"Price","coord":{"Time":"Q1","Product":"Widget"},"value":12}]')`
returns the derived cells that would change, writing nothing. Its `value`
column is `jsonb`: a number, `true`/`false`, a string, or a date as an RFC
3339 string, and an edit's `"value"` takes the same forms.
`improv.status('plan')` shows the mode and, in logical mode, the slot's lag.

## Several rows per coordinate

A fact table often has many rows per coordinate (one per order, say). Bind
the measure with an aggregate and each coordinate's rows are folded into
one input cell: `improv.bind_measure('plan', 'Amount', 'orders', 'amount',
agg => 'sum')` (also `avg`, `min`, `max`, `count`). Without `agg`, two rows on
one coordinate are an error. An aggregated measure is read-only from the
GUI: a save that edits one of its cells fails, so change the rows in SQL.

## Value types

A measure's type comes from its column: numbers (`int*`, `float*`,
`numeric`), `bool`, text (`text`, `varchar`, `char`) and dates (`date`,
`timestamp`, `timestamptz`, kept to the millisecond in UTC). A derived
measure has the type of its formula, so `improv.define('plan', 'Pricey =
Price > 15')` makes `improv_plan.pricey` a `boolean` column. Result tables'
`value` columns are `double precision`, `boolean`, `text` or `timestamptz`.
Other column types are refused by `bind_measure`.

**Choosing:** use `logical` for busy tables (writers stay fast; measured
at 100k cells: 8 ms per UPDATE and its results visible ~50 ms later, vs
~1.1 s with a trigger). Use `trigger` when a
writer must read derived results in its own transaction. Use `recalc` after
bulk loads or inside migrations.

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
NULL argument gives NULL wherever the result depends on it, and division by
zero gives `NaN`. Aggregations need categories, so `SUM(x OVER Time)` is
rejected; put it in a model. A call costs about 0.14 µs. See
`crates/pg_improv/README.md` for the types and rules.

## Upgrades and backups

pg_improv supports Postgres 15–18. To upgrade, install the new release,
restart Postgres, then run `ALTER EXTENSION pg_improv UPDATE`. `pg_dump`
keeps the models and their results. A restored database's worker makes a
new replication slot and catches up with one recompute.

## Durability

In logical mode the worker writes a batch of results and advances its slot
in **one** transaction, so after a crash or `pg_terminate_backend` it
re-reads and re-applies; nothing is lost or counted twice, and rolled-back
writes never appear. A slot retains WAL until confirmed, so set
`max_slot_wal_keep_size` and watch `improv.status(model).lag_bytes`.

See `crates/pg_improv/README.md` for the full SQL API, limits
(`improv.max_cells`), and the benchmark table.
