# pg_improv — Improv models over Postgres data (design + build plan)

Status: **approved 2026-10-14; L0–L5 done.** v1 apply strategy in logical mode is a full recompute per batch; the live incremental engine in the worker is the next step (§4.1). Approach B2 (Improv's own
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
- Built (G1): the worker keeps per model the loaded `Model`, the live
  `Engine` and its last `Snapshot` (a worker-thread-local map; the engine
  thread is spawned with Postgres' signals blocked). `pg_model::live::Mapper`
  turns a batch's row changes into one edit per changed input cell (INSERT
  sets, DELETE removes, UPDATE sets or moves; NULL value = remove, NULL
  dimension = skip). Only measures whose snapshot `Arc` changed are diffed,
  old against new snapshot, and written. It rebuilds (full load, compute,
  diff against the tables) when there is no state, `model_version` moved,
  on TRUNCATE, a new item, a duplicate coordinate, an aggregated binding, or
  an old row whose dimensions were not replicated; any error drops the
  state. After each batch it logs a standby snapshot so the slot's restart
  point keeps up and each poll decodes only new WAL. 100k cells, p50
  freshness: 1-row 1161 → 52 ms, bulk 1552 → 559 ms.
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

| Layer | Deliverable | Proof | Status |
| --- | --- | --- | --- |
| L0 | `crates/pg_improv` skeleton (pgrx 0.17, pg17), catalog SQL, `improv.create_model/define/bind_*`, `improv.recalc` (full recompute, backend thread) | `cargo pgrx test pg17`: define Revenue over a `sales` table, recalc, assert oracle values | done (`eff870a`) |
| L1 | Triggers mode: `on_change` + statement transition tables, closure-restricted recompute | pg_test: UPDATE price in a txn → result row changes in the same txn; ROLLBACK leaves results unchanged | done (`4e91590`) |
| L2 | Logical mode: bgworker, slot/publication lifecycle, pgoutput decode, live Engine, atomic write+advance | pg_test (+ integration script): UPDATE commits → result updates within N ms; kill -9 worker → no loss/no dupes | done (`0928fc9`, `eecb8c4`; e2e `crates/pg_improv/tests/logical.sh`) |
| L3 | `improv_storage_pg` + `Store::open` seam; GUI/TUI/CLI open `postgres://` | test: CLI `list postgres://…` shows the model; GUI headless test loads it; screenshot via gui-shot against a pg model | done (`76656ce`) |
| L4 | Benchmarks on kiwi: trigger vs logical vs recalc — writer latency, end-to-end freshness, throughput at 100k/1M cells | `docs/reviews/<date>-pg-improv-bench.md` | done (`docs/reviews/bench/bench-pg-modes.sh`; table in `crates/pg_improv/README.md`) |
| L5 | Docs (mdBook page, README, man), CI job (pgrx test on pg17), CHANGELOG | gate green | done (mdBook `postgres.md`, README, CI `pg_improv` job, CHANGELOG) |

**Status.** L3 landed on `work/pg-store` (through `76656ce`): crates
`improv_storage_pg` (`PgModelStore`, `CATALOG_SQL`) and `improv_store`
(`Store::open`); CLI, TUI, GUI, server and scheduler all open through
`Store::open`. v1 writes `measure.formula` as serde JSON of the `Formula` AST
(the DSL printer lives in the GUI crate) and reads JSON or `Target = expr` text, and structure is written by plain
upserts, not `improv.save_structure`, until the extension ships it.

**L0 status.** Landed on `work/pg-l0`: `crates/pg_model` (`improv_pg_model`,
a pure workspace crate holding the logic) and `crates/pg_improv` (the pgrx
shell: its own workspace and `Cargo.lock`, excluded from the root workspace
because pgrx-pg-sys needs `pg_config` and libclang). The catalog is a
superset of `CATALOG_SQL`; the extension adds `improv.category_binding`, and
a pg_test checks the superset property. `recalc` builds the model from the
catalog, bound rows and unbound `improv.cell` rows, runs
`dataflow::evaluate` on the backend thread under `catch_unwind`, and applies
one batched upsert plus one batched delete per result table. Every
structural change bumps `improv.model_version`, including a `recalc` that
mints items. Deferred: binding `agg` (L1); making the extension the only
minter of bound items, since storage_pg also mints client-side on load
today (L2); `improv.status`; `what_if`; non-number value types for bound
or derived measures.

**L1 status.** Landed on `work/pg-l1` (through `4e91590`): `crates/pg_improv/src/trigger.rs`
(`set_sync_mode`, `on_change`, `recalc(model, measures)`, `what_if`) and
`crates/pg_model/src/delta.rs` (the pure plan). Deviations from §4.2: the
triggers are **statement-level** (one firing per statement, with transition
tables), one per event (`improv_<model>_sync_{ins,upd,del,trn}`), because
Postgres forbids transition tables on multi-event triggers. The recompute is
"the changed inputs' dependents". It is restricted to the touched
coordinates only when the dependency closure is element-wise (no
aggregation, projection or external call). Otherwise it is a full recompute
of those dependents. The restricted path still reads every bound row; the
restriction only shrinks the engine input and the result diff. The
`recalc(model, measures)` overload has no `DEFAULT NULL`, which would make
`recalc('m')` ambiguous with L0's `recalc(text)`. Benchmark sketch
(`docs/reviews/bench/bench-pg-trigger.sh`, 10k input cells): a 1000-row
UPDATE costs 4.3 ms in manual mode, 32 ms in trigger mode (element-wise) and
45 ms (with `SUM`), against a 34 ms full recalc. Deferred: reading only the
touched coordinates' bound rows (a `WHERE dims IN (…)` load, the real L4
win); restricting through aggregations (a reverse projection of the
touched keys); binding `agg`.

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


## 9. Phase 2 (2026-10-15): `LANGUAGE improv` + closing the v1 gaps

### 9.1 `LANGUAGE improv` — SQL functions written in Improv's formula language

```sql
CREATE FUNCTION margin(price float8, cost float8) RETURNS float8
  LANGUAGE improv IMMUTABLE AS $$ price - cost $$;
CREATE FUNCTION bonus(sales float8, target float8) RETURNS float8
  LANGUAGE improv AS $$
    Ratio = sales / target
    MAX2(Ratio - 1, 0) * 1000
  $$;
CREATE FUNCTION revenue(price float8, qty float8) RETURNS float8
  LANGUAGE improv AS 'price times qty';                -- controlled English too
SELECT product, margin(price, cost) FROM sales;
```

- A **trusted** procedural language (pure computation: no file, network or
  catalog access), so non-superusers may `CREATE FUNCTION … LANGUAGE improv`.
- Arguments are scalar input measures named after the SQL argument names
  (named arguments are required). The body is one or more `Name = expr`
  lines; the result is a trailing bare expression, else the last definition.
  The DSL and the controlled-English form are both accepted (as in the GUI).
- Types: `float8/float4/numeric/int2/int4/int8 → Number`, `bool → Boolean`,
  `text/varchar → Text`, `date/timestamp/timestamptz → DateTime`; the return
  type likewise. Anything else is a validation error.
- Semantics are the engine's: a pure **scalar evaluator** in `improv_engine`
  that shares the per-cell operators with the dataflow engine (identical
  results, identical error values). A NULL argument is a missing input (the
  result is NULL where it depends on it); an Improv error value (e.g. division
  by zero) raises an ERROR carrying its message. Aggregations (`SUM … OVER`)
  need dimensions and are rejected with a pointer to models.
- A **validator** parses and type-checks at `CREATE FUNCTION` time (honouring
  `check_function_bodies`). Compiled bodies are cached per function OID and
  invalidated when the `pg_proc` row changes (`CREATE OR REPLACE`).

### 9.2 Closing the v1 gaps

| # | Gap | Fix |
| --- | --- | --- |
| G1 | Logical mode recomputes the whole model per batch (~11 µs/cell) | **Done.** The worker keeps a live `session::Engine` per model, applies decoded row changes as edits (`pg_model::live`), and writes only the changed result cells. Rebuilt on a structural change (`model_version` moved), TRUNCATE, a new item, or an error. 100k-cell freshness p50: 1161 → 52 ms (1 row), 1552 → 559 ms (bulk). |
| G2 | A GUI save rewrites every bound cell it loaded, clobbering SQL edits | `storage_pg` saves are diff-based against the loaded snapshot, with a per-cell optimistic check (`… WHERE value IS NOT DISTINCT FROM <loaded>`; zero rows → `Conflict`). A removed bound cell sets the value column to NULL (never deletes the row, which may hold other measures). Structural saves never delete rows the saver did not load. |
| G3 | Client and extension both mint item ids | One SQL function, `improv.mint_items(model_id, category_ids[], names[])`, used by both, serialized per model; unique index on `(model_id, category_id, name)`. Minting is append-only and does **not** bump `model_version`. |
| G4 | Duplicate rows per coordinate are rejected | `improv.binding.agg ∈ {sum, avg, min, max, count}` (NULL = duplicates are an error). `bind_measure(…, agg => 'sum')`. Aggregated measures are read-only in the GUI. |
| G5 | Measures must be numbers | `bind_measure` infers the value type from the column type (§9.1 mapping); result tables get a typed `value` column; `define` accepts boolean, text and date results. |
| G6 | The `pg_improv` CI job has never run | Run the job's steps in the same `rust:1.97-trixie` container on the kiwi box before merge. |

Shared catalog DDL (added to both `improv_storage_pg::CATALOG_SQL` and the
extension's catalog, verbatim):

```sql
ALTER TABLE improv.binding ADD COLUMN IF NOT EXISTS agg text
  CHECK (agg IN ('sum', 'avg', 'min', 'max', 'count'));
CREATE UNIQUE INDEX IF NOT EXISTS item_name_uniq ON improv.item (model_id, category_id, name);
CREATE OR REPLACE FUNCTION improv.mint_items(p_model bigint, p_cats bigint[], p_names text[])
RETURNS TABLE (category_id bigint, name text, id bigint) LANGUAGE plpgsql AS $$
BEGIN
  PERFORM 1 FROM improv.model m WHERE m.id = p_model FOR UPDATE;
  INSERT INTO improv.item (model_id, id, category_id, name)
  SELECT p_model,
         (SELECT coalesce(max(i.id), 0) FROM improv.item i WHERE i.model_id = p_model)
           + row_number() OVER (ORDER BY s.c, s.n),
         s.c, s.n
  FROM (SELECT DISTINCT u.c, u.n FROM unnest(p_cats, p_names) AS u(c, n)
        WHERE NOT EXISTS (SELECT 1 FROM improv.item i
                          WHERE i.model_id = p_model AND i.category_id = u.c AND i.name = u.n)) s
  ON CONFLICT DO NOTHING;
  RETURN QUERY
    SELECT i.category_id, i.name, i.id FROM improv.item i
    JOIN (SELECT DISTINCT u.c, u.n FROM unnest(p_cats, p_names) AS u(c, n)) w
      ON i.model_id = p_model AND i.category_id = w.c AND i.name = w.n;
END $$;
```
