# Tutorial: a live sales model in Postgres

An online shop keeps its orders in an ordinary Postgres table. You will put
an Improv model on top of that table: revenue per month and product, the
month's total, each product's share of it, and a target check. Then you
switch the model to **logical** sync, so a background worker keeps the
result tables current as orders arrive, with no triggers on the hot table.

This page is tested: `crates/pg_improv/tests/tutorial-sales.sh` starts a
scratch cluster with the settings below, runs every `sql` block in order
through `psql`, and checks each output block (`docs/tutorials/mdtest.py`).
CI runs it on every Postgres version in `ci/pg_improv.sh`.

## 1. Set up the server

Logical sync needs a logical-decoding WAL level and pg_improv preloaded, so
its worker starts with the server. In `postgresql.conf`:

```ini
shared_preload_libraries = 'pg_improv'
wal_level = logical
improv.database = 'shop'
improv.poll_ms = 50
```

`improv.database` names the one database the worker serves. `poll_ms` is how
often it reads the slot; the default, 200, is fine in production, and 50
makes this tutorial quick. Restart the server, then create the database and
the extension:

<!-- mdtest: skip -->
```sh
createdb shop
psql -d shop
```

```sql
CREATE EXTENSION pg_improv;
```

## 2. The orders table

One row per order line. The shop's own tables carry no Improv-specific
columns:

```sql
CREATE TABLE orders (
    order_id  bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    month     text    NOT NULL,             -- '2026-01'
    product   text    NOT NULL,
    qty       integer NOT NULL,
    price     numeric(10,2) NOT NULL
);
INSERT INTO orders (month, product, qty, price) VALUES
    ('2026-01', 'Kettle',  3, 40.00), ('2026-01', 'Kettle', 2, 40.00),
    ('2026-01', 'Teapot',  4, 25.00), ('2026-01', 'Mug',   20,  8.00),
    ('2026-02', 'Kettle',  6, 40.00), ('2026-02', 'Teapot', 2, 25.00),
    ('2026-02', 'Mug',    15,  8.00), ('2026-02', 'Mug',   10,  8.00);
```

Improv needs one value per cell, and there are many orders per month and
product. Each order line's amount is the cell's input, added up per
(month, product); a view computes it:

```sql
CREATE VIEW order_amount AS
    SELECT order_id, month, product, qty * price AS amount FROM orders;
```

A logical-mode model reads the WAL of the tables it binds, and a view has
no WAL. So bind the **table**, with an aggregate: `bind_measure`'s last
argument folds the rows that share a cell. Bind two measures from `orders`:
`Units`, the summed quantity, and `Price`, the largest price on the cell
(each product has one price here).

## 3. The model

```sql
SELECT improv.create_model('shop');
SELECT improv.bind_category('shop', 'Month',   'orders', 'month');
SELECT improv.bind_category('shop', 'Product', 'orders', 'product');
SELECT improv.bind_measure ('shop', 'Units', 'orders', 'qty',   'sum');
SELECT improv.bind_measure ('shop', 'Price', 'orders', 'price', 'max');
```

The categories take their items from the column values: `Month` has
`2026-01` and `2026-02`, `Product` has `Kettle`, `Mug` and `Teapot`. Now the
formulas, each written once for every cell:

```sql
SELECT improv.define('shop', 'Revenue = Units * Price');
SELECT improv.define('shop', 'MonthTotal = SUM(Revenue OVER Product)');
SELECT improv.define('shop', 'Share = Revenue / MonthTotal');
SELECT improv.define('shop', 'OnTarget = Revenue >= 250');
```

`MonthTotal` adds `Revenue` up over `Product`, so it is per month only.
`Share` divides a (month, product) cell by its month's total. `OnTarget`
is a boolean measure.

## 4. Logical sync

```sql
SELECT improv.set_sync_mode('shop', 'logical');
```

This adds `orders` to the publication `improv_db`. The worker makes the
replication slot, builds the model from the table, and writes every result
table. Each derived measure has one, in schema `improv_shop`, with one
`text` column per dimension and a typed `value`:

<!-- mdtest: retry -->
```sql
SELECT month, product, value FROM improv_shop.revenue ORDER BY 1, 2;
```

```text
2026-01|Kettle|200
2026-01|Mug|160
2026-01|Teapot|100
2026-02|Kettle|240
2026-02|Mug|200
2026-02|Teapot|50
```

<!-- mdtest: retry -->
```sql
SELECT r.month, r.product, round(s.value::numeric, 3) AS share, t.value AS on_target
FROM improv_shop.revenue r
JOIN improv_shop.share s USING (month, product)
JOIN improv_shop.ontarget t USING (month, product)
ORDER BY 1, 2;
```

```text
2026-01|Kettle|0.435|f
2026-01|Mug|0.348|f
2026-01|Teapot|0.217|f
2026-02|Kettle|0.490|f
2026-02|Mug|0.408|f
2026-02|Teapot|0.102|f
```

(The outputs on this page are `psql -At`: unaligned, no headers.)

## 5. Orders arrive

The shop takes more orders, in its own transactions. Nothing calls Improv:

```sql
INSERT INTO orders (month, product, qty, price) VALUES
    ('2026-02', 'Kettle', 1, 40.00),
    ('2026-02', 'Teapot', 2, 25.00);
INSERT INTO orders (month, product, qty, price) VALUES
    ('2026-03', 'Kettle', 7, 40.00);
```

Within a poll interval the worker reads the commits from the slot, applies
only the changed cells to its live model, and writes the result rows that
changed. February's Kettle revenue is now 280, so it is on target, and its
total is 580; March is a new item, which appears on its own:

<!-- mdtest: retry -->
```sql
SELECT month, value FROM improv_shop.monthtotal ORDER BY 1;
SELECT month, product FROM improv_shop.ontarget WHERE value ORDER BY 1, 2;
```

```text
2026-01|460
2026-02|580
2026-03|280
2026-02|Kettle
2026-03|Kettle
```

A correction is an `UPDATE`, and a cancelled order a `DELETE`: both flow
through the same way.

```sql
DELETE FROM orders WHERE month = '2026-01' AND product = 'Mug';
```

<!-- mdtest: retry -->
```sql
SELECT month, value FROM improv_shop.monthtotal ORDER BY 1;
SELECT count(*) FROM improv_shop.revenue WHERE month = '2026-01';
```

```text
2026-01|300
2026-02|580
2026-03|280
2
```

The January Mug cell is gone: with no orders, it has no `Units`, so no
`Revenue` either, and its result row was deleted.

## 6. Is it keeping up?

`improv.status` reports how the model is kept current:

<!-- mdtest: retry -->
```sql
SELECT sync_mode, slot, last_error IS NULL AS healthy, rebuilds >= 1, incremental_applies >= 1
FROM improv.status('shop');
```

```text
logical|improv_db|t|t|t
```

`rebuilds` counts full loads (the first catch-up is one), and
`incremental_applies` the batches applied to the live model. `lag_bytes` is
the WAL the slot still holds, and `last_error` says why a model stopped
updating. [Monitoring](./monitoring.md) explains every column and the
alerts to set.

## 7. What-if, without writing

`what_if` evaluates the model with edits applied in memory and returns only
the cells that would change. Raising February's Teapot price to 30:

```sql
SELECT measure, coord->>'Month' AS month, coord->>'Product' AS product,
       round((value#>>'{}')::numeric, 3) AS value
FROM improv.what_if('shop', '[{"measure": "Price",
    "coord": {"Month": "2026-02", "Product": "Teapot"}, "value": 30}]')
ORDER BY 1, 2, 3;
```

```text
MonthTotal|2026-02||600.000
Revenue|2026-02|Teapot|120.000
Share|2026-02|Kettle|0.467
Share|2026-02|Mug|0.333
Share|2026-02|Teapot|0.200
```

`MonthTotal` has no `Product`, hence the empty column. Nothing was written:
the result tables still say 580.

## Clean up

```sql
SELECT improv.drop_model('shop');
```

`drop_model` removes the catalog rows and the `improv_shop` schema, and
takes `orders` out of the publication. With no logical-mode model left, the
worker drops the slot. The `orders` table is untouched.

## Next

- [Improv in Postgres](./postgres.md): every sync mode, privileges, and the
  catalog.
- [A family's finances](./postgres-finance.md): a larger model on six
  related tables.
- [Operations runbook](./runbook-ops.md): what to do when the worker
  stops or a slot falls behind.
