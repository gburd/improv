# When to Use Improv, and When Not

Improv is good at one shape of problem: **a model**. Named dimensions,
formulas written once over them, data that changes, and results that must
follow the changes without a full recompute. When the problem has that
shape, Improv saves you the SQL you would otherwise write and maintain by
hand. When it does not, another tool is simpler and faster. This page lists
the common mismatches, what to use instead, and how to tell that you have
outgrown Improv.

The limits quoted here were measured on EC2 c7i.8xlarge with release
builds. Sources: the
[v0.10.0 scale benchmark][bench] and the post-0.10.0 fixes listed in the
[CHANGELOG][changelog] (marked "after the fixes"). [Capacity
planning](./capacity.md) turns them into sizing formulas.

## Use Improv when

- **The model has dimensions and logic.** Revenue by quarter and product,
  shares of a total, a budget against actuals, a forecast that rolls a
  balance forward: formulas over categories ([Concepts](./concepts.md)).
- **The inputs change, and the results must follow.** In Postgres, a
  1,000-row batch into a 2.5M-row table shows up in the results 86–130 ms
  after its commit, where a `REFRESH MATERIALIZED VIEW` of the same
  `GROUP BY` takes about 5 s ([benchmark §1][bench]; re-measured on `main`).
- **People explore it.** The GUI and TUI pivot and edit the same model that
  SQL reads, with no export step.
- **The logic should live in one place**, versioned and typed, rather than
  copied across spreadsheet cells or SQL views.

## Ad-hoc GROUP BY: use SQL or DuckDB

A question you ask once ("revenue by region last quarter") is a query, not
a model. Plain SQL answers it in one statement, with no model to define,
bind or keep. DuckDB's `GROUP BY ALL` of the benchmark's 2.5M rows takes
0.045 s; Improv's `improv(source, model)` over the same rows takes 0.65 s,
most of it building the model ([benchmark §3h][bench]).

**Instead:** SQL in Postgres, or DuckDB over Parquet and CSV. For a single
formula inside such a query, `improv_calc` in DuckDB or a
[`LANGUAGE improv`](./postgres.md#functions-written-in-improvs-language)
function in Postgres gives you Improv's function library without a model.

**You have outgrown Improv when** each new question means a new model,
or you define a model and then never edit or refresh it.

## Data far bigger than memory: aggregate first

The engine keeps every cell of a model in memory: about 0.7–0.9 GB per
million cells at v0.10.0 ([benchmark §3a][bench]), 0.7–0.8 GB after the fixes.
A model does not need your raw rows as cells, though. In Postgres and DuckDB a
measure bound with an aggregate (`agg => 'sum'`, `count`, `count_distinct`,
…) folds the rows to one cell per coordinate. 2.5M fact rows become 44,750
cells, and the fold runs where the data is. Only the logical worker's
memory grows with the rows: about 180 bytes per fact row in all, fold state
and engine together ([benchmark §1][bench]).

**Instead:** bind with `agg`, or summarize first (a view or a rollup table)
and bind the summary. Keep raw event-level analytics in the database or a
warehouse.

**You have outgrown Improv when** the model itself, after aggregation,
needs tens of millions of cells: a 5M-cell grid takes 12.7 s to evaluate
and 3.4 GB after the fixes, and `improv.max_cells` refuses more than 5M
bound input cells by default.

## A single nightly aggregate: use a materialized view

If the results are needed once a day and nobody edits the model, a
materialized view is the whole solution. `REFRESH MATERIALIZED VIEW
CONCURRENTLY` of the benchmark's `GROUP BY` takes about 5 s; nothing runs
between refreshes, and there is no replication slot to watch.

**Instead:** `CREATE MATERIALIZED VIEW … ; REFRESH … CONCURRENTLY` from
cron or `pg_cron`. If the logic is easier to write as formulas, keep the
Improv model in `manual` mode and call `improv.recalc()` from the same job
(2.4–2.8 s on the same data, measured on `main`): no worker, no slot.

**You have outgrown the view (and want Improv) when** the refresh window
becomes a staleness complaint, or several views repeat the same logic.

## Large graphs: use a graph engine

`FIXPOINT` runs PageRank-style iterations correctly, and incrementally after
an edit, but it holds every edge in memory several times: about 1.0 kB per
edge after the fixes (15.0 GB at 10M edges). A 20M-edge solve took 3.4 min
and 31.8 GB at v0.10.0 ([benchmark §2][bench]).

**Instead:** a graph engine or library (NetworkX or igraph for analysis,
Apache AGE in Postgres, a dedicated graph database) for anything past a few
million edges, or for traversals and path queries, which Improv does not
have. Feed the per-node result back into Improv as an input measure if the
model needs it.

**You have outgrown Improv when** the edge count reaches tens of millions,
or a solve has to finish in seconds.

## High-write tables in trigger or commit mode: use logical mode

`trigger` and `commit` modes recompute inside the writer's transaction,
which is what makes them read-your-writes. The writer pays for it.

- **Trigger mode:** about 290 ms per write statement on the 2.5M-row model
  (354 ms per 1,000-row statement, 2.9 s for ten one-row statements in one
  transaction) ([benchmark §3e][bench]).
- **Commit mode:** it reloads every table the transaction wrote. That is
  2.1–2.4 s per transaction on the same model after the fixes (5.6 s at
  v0.10.0).

**Instead:** `logical` mode. The writer pays nothing, and results follow
about one poll after the commit. Where one transaction must read its own
results back, call `improv.recalc(model)` in that transaction.

**The sign:** write latency or lock waits on the bound tables rose after
you chose `trigger` or `commit`, or commit-mode transactions take seconds.

## Many models in one database

All logical-mode models in a database share one worker and one replication
slot. With 1,000 small models in one database (0.11.0, measured on `main`
for this page), a one-row change was visible in 54–146 ms and a
transaction touching every model in 2.6 s. The worker used about 0.6 MB per
small model. It applies models one at a time on one core, so a model whose
apply is slow delays all the others; parallel apply is planned (B1 in the
[road to 1.0][plan]).

**The sign:** `improv.status` shows growing `lag_bytes` for every model at
once while one of them is busy. Split the heavy model into its own database,
or put it in `manual` mode and refresh it on a schedule.

## Measured limits at a glance

| Limit | Value | Source |
| --- | --- | --- |
| Engine memory per cell | 0.7–0.9 GB per million cells (v0.10.0); 0.54 GB at 1M, 2.35 GB at 5M after the fixes | [§3a][bench], `CoordKey` and B7 entries in the [CHANGELOG][changelog] |
| Engine evaluate | 0.9 s per million cells after the fixes (4.7–5.5 s at v0.10.0) | same |
| Single-cell edit | 56 µs, independent of model size (64 µs at v0.10.0) | same |
| Logical-mode apply, 1,000-row batch, 2.5M rows / 44,750 cells | 86–130 ms commit → visible | [§1][bench], re-measured on `main` |
| Worker memory, same model | about 450 MB private (180 B per fact row, engine included) | [§1][bench], re-measured on `main` |
| Trigger mode, same model | about 290 ms per statement | [§3e][bench] |
| Commit mode, same model | 2.1–2.4 s per transaction after the fixes | [CHANGELOG][changelog] |
| Models per database (logical) | 1,000 small models: 54–146 ms per change, 0.6 MB each | re-measured on `main` with the §3f harness of the [benchmark][bench] |
| FIXPOINT | about 1.0 kB per edge after the fixes | [§2][bench], [CHANGELOG][changelog] |
| Store file import | linear since Mentat 1.18.0: 100k rows 7 s, 1M about 105 s (1.16.0: 100k 21 s, 1M did not finish in 15 min) | [Mentat 1.18 upgrade](https://codeberg.org/gregburd/improv/src/branch/main/docs/reviews/2026-10-23-mentat-1.18-upgrade.md) |

[bench]: https://codeberg.org/gregburd/improv/src/branch/main/docs/reviews/2026-10-20-v0.10.0-benchmark.md
[changelog]: https://codeberg.org/gregburd/improv/src/branch/main/CHANGELOG.md
[plan]: https://codeberg.org/gregburd/improv/src/branch/main/docs/reviews/2026-10-21-road-to-1.0.md
