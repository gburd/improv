# Tutorial: analyzing a Parquet file in DuckDB

A team exports its support tickets to Parquet every night. You will load
that file in DuckDB, describe a model in a few lines (ticket counts and
hours per week and team, each team's share of the week, the change from the
week before, and the hours per ticket), and get the answer back as an
ordinary table you can filter, join and pivot. Nothing is installed in the
database and no model is stored: `improv(source, model)` computes the
model on the fly.

This page is tested: `crates/duckdb_improv/test/tutorial.sh` feeds every
`sql` block below, in order, to the DuckDB CLI and checks each output block
(`docs/tutorials/mdtest.py`). CI runs it under DuckDB v1.5.6 and v2.0
(`ci/duckdb_improv.sh`).

## 1. Load the extension

Start the DuckDB CLI with a database file. The extension is in the
community repository; install it once:

<!-- mdtest: skip -->
```sql
INSTALL improv FROM community;
LOAD improv;
```

(The tested script loads a local build, `duckdb -unsigned` and
`LOAD 'build/release/improv.duckdb_extension'`, instead.) The outputs below
are DuckDB's `-list` mode without headers.

## 2. Make the Parquet file

In real use the file comes from somewhere else. Here, DuckDB writes it: a
table of tickets, saved with `COPY ... (FORMAT parquet)`:

```sql
CREATE TABLE tickets AS
SELECT * FROM (VALUES
    (1, DATE '2026-03-02', 'Billing',  1.5), (2, DATE '2026-03-03', 'Billing',  0.5),
    (3, DATE '2026-03-03', 'Platform', 4.0), (4, DATE '2026-03-04', 'Platform', 2.0),
    (5, DATE '2026-03-05', 'Platform', 3.0), (6, DATE '2026-03-09', 'Billing',  1.0),
    (7, DATE '2026-03-10', 'Billing',  2.0), (8, DATE '2026-03-10', 'Billing',  1.5),
    (9, DATE '2026-03-11', 'Platform', 6.0), (10, DATE '2026-03-12', 'Mobile',  2.5),
    (11, DATE '2026-03-13', 'Mobile',  1.5)
) t(id, opened, team, hours);
COPY tickets TO 'tickets.parquet' (FORMAT parquet);
DROP TABLE tickets;
SELECT count(*), sum(hours) FROM 'tickets.parquet';
```

```text
11|25.5
```

## 3. Shape the source

A model's **source** is a table, a view, or a query. Its columns become the
model's categories and inputs. Here a query reads the Parquet file and
turns each ticket's date into its week (as text, `2026-W10`) and adds a
column of ones to count with:

```sql
CREATE VIEW ticket_rows AS
SELECT strftime(opened, '%G-W%V') AS week, team, hours, 1 AS n
FROM 'tickets.parquet';
```

## 4. The model

The model is text, one line per item:

- `category` makes a dimension from a column: its items are the column's
  distinct values, in order.
- `input` reads a measure from a column; `sum` adds up the rows that land
  on the same (week, team) cell.
- `Name = formula` is a derived measure, written once over names.

```sql
SELECT measure, "Week", "Team", value_text
FROM improv('ticket_rows', '
    category Week = week
    category Team = team
    input Tickets = n sum
    input Hours = hours sum
    PerTicket = Hours / Tickets
    WeekHours = SUM(Hours OVER Team)
    Share = ROUND(Hours / WeekHours, 3)
    Change = Hours - PREV(Hours, Week, 1, 0)
');
```

```text
PerTicket|2026-W10|Billing|1
PerTicket|2026-W10|Platform|3
PerTicket|2026-W11|Billing|1.5
PerTicket|2026-W11|Mobile|2
PerTicket|2026-W11|Platform|6
WeekHours|2026-W10|NULL|11
WeekHours|2026-W11|NULL|14.5
Share|2026-W10|Billing|0.182
Share|2026-W10|Platform|0.818
Share|2026-W11|Billing|0.31
Share|2026-W11|Mobile|0.276
Share|2026-W11|Platform|0.414
Change|2026-W10|Billing|2
Change|2026-W10|Platform|9
Change|2026-W11|Billing|2.5
Change|2026-W11|Mobile|4
Change|2026-W11|Platform|-3
```

The result has one row per derived cell. `WeekHours` has no `Team`, so that
column is NULL. `PREV(Hours, Week, 1, 0)` is the previous week's hours, or
0 where there is none: in the first week, and for Mobile, which had no
tickets in week 10, so its change in week 11 is all of its 4 hours.

## 5. Use the result as a table

`improv(...)` is a table function, so the rest of SQL applies. The value
is in `value` (a `DOUBLE`) and `value_text`. Pivot the hours per ticket
into a week-by-team grid (one measure at a time: a pivot over `Team` needs
rows that have one):

```sql
PIVOT (SELECT "Week", "Team", value
       FROM improv('ticket_rows', '
           category Week = week
           category Team = team
           input Tickets = n sum
           input Hours = hours sum
           PerTicket = Hours / Tickets
       ') WHERE measure = 'PerTicket')
ON "Team" USING first(value) GROUP BY "Week" ORDER BY "Week";
```

```text
2026-W10|1.0|NULL|3.0
2026-W11|1.5|2.0|6.0
```

## 6. A query as the source

The source can be a query too, so the model can read the Parquet file
directly. Only last week's tickets:

```sql
SELECT "Team", value FROM improv(
    'SELECT team, hours FROM ''tickets.parquet'' WHERE opened >= DATE ''2026-03-09''',
    'category Team = team
     input Hours = hours sum
     Share = ROUND(Hours / SUM(Hours OVER Team), 2)')
ORDER BY value DESC;
```

```text
Platform|0.41
Billing|0.31
Mobile|0.28
```

## Next

- [Improv in DuckDB](./duckdb.md): the model syntax in full, scalar
  formulas (`improv_calc`), and the function registry.
- [The Formula Language](./formulas.md): `OVER`, `PREV` and the rest.
