# pg_improv — Improv models over Postgres data (design + build plan)

Status: **approved 2026-10-14, in progress.** Approach B2 (Improv's own
incremental engine, live, inside Postgres) + C (the model lives in Postgres;
`improv-gui postgres://…` edits it). Supersedes nothing; extends
`AGENT_DATABASE_CONNECTIVITY.md` (external SQL) and
`AGENT_STORAGE_STEERING.md` (a second model store).

## 1. User story (the definition of done)

```sql
CREATE EXTENSION pg_improv;                 -- shared_preload_libraries = 'pg_improv'
SELECT improv.create_model('plan');
SELECT improv.bind_category('plan', 'Time',    'sales', 'quarter');
SELECT improv.bind_category('plan', 'Product', 'sales', 'product');
SELECT improv.bind_measure ('plan', 'Price',    'sales', 'price');
SELECT improv.bind_measure ('plan', 'Quantity', 'sales', 'qty');
SELECT improv.define('plan', 'Revenue = Price * Quantity');
SELECT improv.define('plan', 'Total = SUM(Revenue OVER Time)');
-- results appear (and stay current) in a table + view:
SELECT * FROM improv_plan.revenue;          -- quarter | product | value
```

```sh
improv-gui postgres://me@db/sales?model=plan
```
…and the NeXT GUI opens that model: tiles, formulas, grid. Editing a formula
in the GUI redefines it in Postgres; editing an input cell writes the bound
fact table; the derived cells everyone sees in SQL update.

## 2. Concepts and catalog (schema `improv`)

| Table | Holds |
| --- | --- |
| `improv.model(id, name, sync_mode, slot_name, created)` | one row per model; `sync_mode ∈ {'logical','trigger','manual'}` (default `'logical'`) |
| `improv.category(model_id, id, name)` | dimensions |
| `improv.item(model_id, id, category_id, name)` | members; for bound categories the items are *derived* from distinct column values (see §3) |
| `improv.measure(model_id, id, name, value_type, kind, formula, description, categories int[])` | inputs + derived |
| `improv.binding(model_id, measure_id, rel regclass, value_col, dim_cols jsonb)` | how an input measure reads a fact table: `{"Time":"quarter","Product":"product"}` |
| `improv.cell(model_id, measure_id, coord jsonb, value jsonb)` | *unbound* input cells (typed in the GUI) and scenario overrides |
| `improv.view(model_id, id, name, layout jsonb)` | saved GUI views (the existing `View` JSON) |
| `improv.meta(model_id, blob jsonb)` | external fns, scenarios, extras (the existing meta blob) |
| `improv.model_version(model_id, tx bigint)` | bumped on every structural change; the GUI's optimistic-concurrency token |

Results: per model a schema `improv_<model>`, per derived measure a table
`improv_<model>.<measure>(<dim cols…>, value <type>, PRIMARY KEY(dims))`.
Tables, not views, because the engine writes deltas into them; plain SQL users
read them like any table, can index/GRANT/RLS them, and they replicate.

## 3. Binding Postgres data

- A **bound category** takes its items from the distinct values of its column
  across all bindings that use it (`text` cast). Item ids are stable: a
  `improv.item` row is minted the first time a value is seen and never reused.
- A **bound input measure** reads `SELECT dims…, value FROM rel`. Duplicate
  coordinates are an error at sync time unless the binding sets
  `agg = 'sum'|'avg'|'min'|'max'|'last'` (default: error, loudly).
- NULL value → no cell. NULL dimension → row ignored + counted in
  `improv.status`.
- Value types: `numeric/int/float → Number`, `bool → Boolean`,
  `text → Text`, `date/timestamptz → DateTime`.

## 4. The engine in Postgres — three ways derived results update

All three share one pure core (`improv_engine::evaluate` / `session::Engine`)
and one writer (`apply_deltas`: upsert changed cells, delete vanished ones).

### 4.1 Logical decoding (default, `sync_mode='logical'`)
A background worker per database (`pg_improv sync`) owns, per model:
a logical replication slot (`pgoutput`, publication `improv_<model>` over the
bound tables), and a live `session::Engine` (its own compute thread; **no
Postgres call ever happens off the worker's main thread**).
Loop: read changes since the slot's confirmed LSN
(`pg_logical_slot_peek_binary_changes`), decode the bound tables' rows,
group by source transaction, apply each source transaction's deltas to the
engine, write the result delta to the result tables **in one worker
transaction that also advances the slot** (`pg_replication_slot_advance`) —
so a crash re-reads and re-applies (idempotent upserts), never loses.
- ✚ zero cost in the writer's transaction; batching; survives restarts
  (slot position is the checkpoint); replicas get result tables via WAL.
- ✖ results lag the commit (eventual); needs `wal_level=logical` and
  `REPLICA IDENTITY` (FULL or a PK) on bound tables; a stuck worker
  retains WAL (bounded by `max_slot_wal_keep_size`, surfaced in status).

### 4.2 Triggers (`sync_mode='trigger'`)
Row-level AFTER triggers on bound tables call `improv.on_change()`, which
evaluates the affected derived cells **inside the writer's transaction**
(backend-local: `evaluate()` on the dependency closure restricted to the
touched coordinates' dependents, computed with `execute_directly` on the
backend thread — no threads) and upserts the result rows.
- ✚ results are transactionally consistent with the source: commit both or
  neither; read-your-writes.
- ✖ adds latency + lock footprint to every writer; recompute per statement
  (statement-level transition tables batch it); hot contention on result rows.

### 4.3 Explicit, in your transaction: `improv.recalc(model [, measures])`
A SQL function that recomputes derived results **now, in the caller's
transaction and snapshot**, regardless of `sync_mode`. Returns the number of
result rows changed. Use it after a bulk load in `manual` mode, inside a
migration, or to force consistency in `logical` mode before reading.
`improv.what_if(model, edits jsonb) → SETOF (measure, coord, value)` computes
without writing anything.

Trade-off table goes in the user docs verbatim.

## 5. C — Postgres as the model store (for the GUI/TUI/CLI)

- New crate `improv_storage_pg`: implements the same surface as
  `storage_mentat::ModelStore` (`open`, `load_model`, `load_partial`,
  `save_model`, `tx_high_water`/`loaded_tx`) against the catalog + bound
  tables, over a client connection (`postgres` crate, already in the tree via
  storage_sql). Structure saves → `improv.*` rows (via `improv.save_structure`
  so the extension owns validation); input-cell saves → the bound fact table
  (`UPDATE … WHERE dims` / `INSERT`) or `improv.cell` for unbound measures.
- A `Store` enum seam in `core_model`-adjacent code (`improv_store` crate or a
  small module in storage_mentat): `Store::open(url)` dispatches on
  `postgres://` / `postgresql://` vs a file path. GUI, TUI, CLI, server switch
  from `ModelStore::open` to `Store::open` — one call site each.
- Concurrency: `improv.model_version` is the high-water; the existing
  Conflict path ("reload and retry") applies unchanged.
- The GUI then reads derived values from its own in-process engine as today
  (fast, offline-capable after load) — the result tables are for SQL users.

## 6. Layers (each lands green, in order)

| Layer | Deliverable | Proof |
| --- | --- | --- |
| L0 | `crates/pg_improv` skeleton (pgrx 0.17, pg17), catalog SQL, `improv.create_model/define/bind_*`, `improv.recalc` (full recompute, backend thread) | `cargo pgrx test pg17`: define Revenue over a `sales` table, recalc, assert oracle values |
| L1 | Triggers mode: `on_change` + statement transition tables, closure-restricted recompute | pg_test: UPDATE price in a txn → result row changes in the same txn; ROLLBACK leaves results unchanged |
| L2 | Logical mode: bgworker, slot/publication lifecycle, pgoutput decode, live Engine, atomic write+advance | pg_test (+ integration script): UPDATE commits → result updates within N ms; kill -9 worker → no loss/no dupes |
| L3 | `improv_storage_pg` + `Store::open` seam; GUI/TUI/CLI open `postgres://` | test: CLI `list postgres://…` shows the model; GUI headless test loads it; screenshot via gui-shot against a pg model |
| L4 | Benchmarks on kiwi: trigger vs logical vs recalc — writer latency, end-to-end freshness, throughput at 100k/1M cells | `docs/reviews/<date>-pg-improv-bench.md` |
| L5 | Docs (mdBook page, README, man), CI job (pgrx test on pg17), CHANGELOG | gate green |

**Status.** L3 landed on `work/pg-store` (through `76656ce`): crates
`improv_storage_pg` (`PgModelStore`, `CATALOG_SQL`) and `improv_store`
(`Store::open`); CLI, TUI, GUI, server and scheduler all open through
`Store::open`. v1 writes `measure.formula` as serde JSON of the `Formula` AST
(the DSL printer lives in the GUI crate) and reads JSON or `Target = expr` text, and structure is written by plain
upserts, not `improv.save_structure`, until the extension ships it.

## 7. Safety rules (non-negotiable, from the security review)

- Never call a Postgres API from a non-main thread; the engine thread only
  computes. `#[pg_guard]` every entry point; convert engine panics to
  `ERROR` (catch_unwind on the backend path).
- Memory: per-model cell budget GUC `improv.max_cells` (default 5M); refuse
  to build an engine over it, with a clear error.
- Cancellation: `recalc`/`what_if` check `CHECK_FOR_INTERRUPTS` between
  plan layers; the worker honors SIGTERM between batches.
- Privileges: functions `SECURITY INVOKER`; result tables owned by the
  model's owner; the worker reads bound tables as the model owner
  (`SET ROLE`) so it cannot exfiltrate what the owner can't read.
- External functions (`CALL`) are **disabled** inside Postgres in v1.
- Identifiers are always quoted via `quote_ident`/`format('%I')`.

## 8. Open questions (decide during the build, record here)

- Item ordering for bound categories (lexical by default; `ORDER BY` per binding later).
- `numeric` precision: engine is f64; result columns are `double precision` in v1.
- Multi-database: one worker per database listed in `improv.databases` GUC.
