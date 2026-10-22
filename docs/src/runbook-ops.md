# Operations Runbook

This runbook covers incidents for whoever is on call for a Postgres
database with `pg_improv` in it. Each entry is **symptom → cause →
diagnosis → fix**, and ends with how to check the fix. It describes
pg_improv 0.11.0 (`main`); where a fix in the [road to 1.0][plan] changes
the procedure, the entry says so. [Monitoring](./monitoring.md) has the
signals and alerts these entries start from.

Two queries come up in most entries:

```sql
-- every model at a glance
SELECT m.name, s.sync_mode, s.lag_bytes, s.last_applied_at, s.rebuilds,
       s.incremental_applies, s.last_error
  FROM improv.model m, improv.status(m.name) s;

-- the slot and the worker
SELECT slot_name, active_pid, wal_status, safe_wal_size,
       pg_size_pretty(pg_wal_lsn_diff(pg_current_wal_lsn(), restart_lsn)) AS retained
  FROM pg_replication_slots WHERE slot_name = 'improv_db';
SELECT pid, datname, backend_start FROM pg_stat_activity
 WHERE backend_type = 'pg_improv sync';
```

Restarting the worker is always safe: it re-applies only what no model has
recorded, and the postmaster starts it again 5 s after it exits.

```sql
SELECT pg_terminate_backend(pid) FROM pg_stat_activity
 WHERE backend_type = 'pg_improv sync';
```

## The worker stopped

**Symptom.** Logical-mode results stop following writes. `lag_bytes`
grows for every logical model, and `pg_stat_activity` has no
`pg_improv sync` row, or its `backend_start` keeps resetting.

**Causes.**
- `pg_improv` is not in `shared_preload_libraries` (often after a config
  rewrite or an upgrade). Without the preload there is no worker at all.
- `improv.database` names another database, or one that does not exist.
  The server log then shows `FATAL: database "<name>" does not exist`
  every 5 s.
- The worker crashes during each poll (a crash loop).
- `max_worker_processes` had no room at the last start: see
  [the worker never started](#the-worker-never-started-max_worker_processes).

**Diagnosis.**

```sql
SHOW shared_preload_libraries;      -- must contain pg_improv
SHOW improv.database;               -- must be this database
SELECT current_database();
```

Then read the server log for the worker's lines (`pg_improv sync`) and for
`background worker "pg_improv sync" … exited`.

**Fix.** Set `shared_preload_libraries = 'pg_improv'` and
`improv.database = '<db>'` in `postgresql.conf`, then restart Postgres
(both settings are read at server start). For a crash loop, the last
`pg_improv sync: …` line before each exit names the model or step: put that
model in `manual` mode (`SELECT improv.set_sync_mode('<m>', 'manual')`) to
get the others going, and see the entries below.

**Check.** One `pg_improv sync` row in `pg_stat_activity` with a stable
`backend_start`, and `lag_bytes` falling.

## The worker never started (max_worker_processes)

**Symptom.** Logical-mode results stop following writes after a restart.
`pg_stat_activity` has no `pg_improv sync` row, and every logical model's
`last_error` reads `the sync worker is not running: if the server log
says "too many background workers", raise max_worker_processes (now N)
and restart`.

**Cause.** The worker is a background worker registered at server start,
and `max_worker_processes` had no slot left for it: other extensions'
workers, parallel query, and the logical replication launcher share the
same pool. The postmaster logs the failure once at start and does not
retry, so there is no crash loop and no other log line.

**Diagnosis.**

```sql
SHOW max_worker_processes;
SELECT backend_type, count(*) FROM pg_stat_activity GROUP BY 1;
```

The server log from the last start has `LOG: too many background
workers` with the hint `Consider increasing the configuration parameter
"max_worker_processes"`.

**Fix.** Raise `max_worker_processes` (it takes a restart) to cover every
registered worker plus `max_parallel_workers`. Nothing is lost while the
worker is away: the slot keeps the WAL, up to `max_slot_wal_keep_size`
(see [the slot is lost](#the-slot-is-lost) if it ran past that).

**Check.** One `pg_improv sync` row, `last_error` NULL, and the writes
made meanwhile applied.

## Disk full while writing results

**Symptom.** A logical model's `last_error` reads `could not extend file
"base/…": No space left on device`, and the server log repeats
`pg_improv sync: model <m>: could not extend file …`, further and further
apart. In trigger or commit mode, writers to the model's bound tables get
the same error and their statements roll back.

**Cause.** The tablespace holding the model's result tables
(`improv_<m>.*`) is full.

**What pg_improv does.** The failed apply rolls back: no result row is
half-written, and the slot does not move past the batch. The worker keeps
running and keeps the models that still fit in sync, and retries the
failing model with backoff (0.5 s, doubling to 60 s). A full `pg_wal` is
different: Postgres itself stops (PANIC) until there is room; then
recovery runs, and the worker resumes from its slot.

**Fix.** Free space or grow the volume. To move a model's results
elsewhere, put its tables in another tablespace:
`ALTER TABLE improv_<m>.<measure> SET TABLESPACE <ts>` as the model's
owner. The next retry (at most 60 s later) rebuilds the model; restart
the worker to retry at once.

**Check.** `last_error` is NULL and `rebuilds` went up by one.

## `wal_keep_size` and the slot

`wal_keep_size` does not limit the slot. A slot keeps the WAL it needs
whatever `wal_keep_size` says (0 included), so lowering it does not make
a stopped worker lose changes, and it does not stop the slot from
retaining WAL either. The caps are `max_slot_wal_keep_size` (while the
worker is down) and `improv.max_slot_lag` (while it runs but lags); see
[slot lag growing](#slot-lag-growing-and-the-disk-filling).

## A model is stuck with `last_error`

**Symptom.** `improv.status('<m>')` shows a non-NULL `last_error`, and
the model's results are stale. The server log repeats
`pg_improv sync: model <m>: <error>`, further and further apart (the
retry backs off from 0.5 s to 60 s). Other models are not affected.

**Causes.** Whatever the error says. The common ones:
- `… bound input cells exceed improv.max_cells = N`: the model grew past
  the cell budget.
- A permission error on a result table or a bound table (the result table
  was altered or its grants changed by hand).
- A result table changed by hand: a `CHECK` constraint, a dropped column,
  or a changed `value` type.
- A formula that compiles but fails at load: a bound column that was
  dropped or retyped.

**Diagnosis.**

```sql
SELECT last_error FROM improv.status('<m>');
SELECT improv.recalc('<m>');   -- runs the same load in your session: the error, with context
```

**Fix.** Fix the cause, then wait: the next retry (at most 60 s later)
rebuilds the model from its tables and clears `last_error`. To retry at
once, restart the worker. For `improv.max_cells`, size the memory first
([capacity](./capacity.md)), then raise it in `postgresql.conf` and
reload, or split the model. The worker reads it per load, but it must be
restarted to see a reload on 0.10.0 and `main` (D9).

**Check.** `last_error` is NULL, `rebuilds` went up by one, and
`last_applied_at` is recent.

## Slot lag growing and the disk filling

**Symptom.** `pg_wal` grows. The slot's `retained` and the models'
`lag_bytes` keep rising, and `safe_wal_size` falls.

**Causes.**
- The worker is stopped (see above) or serves another database.
- The worker cannot keep up: a slow model, a rebuild loop, or more changed
  rows per second than one core applies (see [slow apply](#a-slow-apply)).
- Every poll fails to read the slot (`pg_improv sync: reading improv_db: …`
  in the log).

The slot keeps all of the server's WAL from its restart point, not only
the bound tables'. A stopped worker therefore retains WAL at the server's
full write rate ([capacity](./capacity.md#wal-retained-while-the-worker-is-down)).

**Diagnosis.** The two queries at the top, plus the server log's
`pg_improv sync:` lines from the last minutes.

**Fix.**
1. If the worker is stopped, start it (above).
2. If one model is slow or rebuilding, take it out of logical mode for now
   with `SELECT improv.set_sync_mode('<m>', 'manual')`, so the others catch
   up, and refresh it with `improv.recalc('<m>')` on a schedule.
3. If the disk will fill before the worker catches up, give up the slot.
   Dropping it costs one rebuild per logical model and loses no data:

   ```sql
   SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE backend_type = 'pg_improv sync';
   SELECT pg_drop_replication_slot('improv_db');   -- right after: the worker holds it while polling
   ```

   If the drop fails with `replication slot "improv_db" is active for PID
   …`, the worker restarted in between: run both statements again,
   together.
4. Prevent it next time: set `max_slot_wal_keep_size` (Postgres then
   invalidates the slot instead of filling the disk; see the next entry)
   and alert on retained WAL.

**Check.** `retained` drops to a few MB and stays there. If you dropped the
slot, `improv_db` comes back within a poll and every logical model shows
one more rebuild.

## The slot is lost

**Symptom.** `pg_replication_slots.wal_status = 'lost'` for `improv_db`
(`invalidation_reason = 'wal_removed'`). Logical-mode results have stopped
following writes. **`improv.status` shows no `last_error`**, and its
`lag_bytes` stays at its last value. The server log repeats, every poll:
`pg_improv sync: reading improv_db: … can no longer access replication
slot "improv_db"` with `This replication slot has been invalidated due to
"wal_removed"`.

**Cause.** The worker fell behind, or was down, by more than
`max_slot_wal_keep_size`, and Postgres removed WAL the slot still needed.
On 0.10.0 and `main` the worker does not recover by itself.

**Fix (0.10.0 and `main`).** Stop the worker and drop the slot, in that
order and at once:

```sql
SELECT pg_terminate_backend(pid, 5000) FROM pg_stat_activity
 WHERE backend_type = 'pg_improv sync';
SELECT pg_drop_replication_slot('improv_db');
```

The restarted worker creates a new slot, resets every logical model's
position and rebuilds each from its tables, so the results are exact
again. In 0.11, D11 makes this automatic: the worker recreates a lost
slot, rebuilds the models and reports it in `status`.

**Check.** `wal_status = 'reserved'`, every logical model's `rebuilds`
went up by one, and a test write to a bound table shows up in its results.
This was tested on PG 18.6: after the fix, writes made while the slot was
lost were in the results, and so were writes made after it.

## A rebuild loop

**Symptom.** `rebuilds` rises with almost every batch, and
`incremental_applies` stays flat. The log shows `pg_improv sync: model
<m>: rebuilding (<reason>)` again and again. Apply latency equals a full
load (seconds on large models), and the other models wait behind it.

**Causes**, by the reason in the log line (the full list is in
[Monitoring](./monitoring.md#the-workers-log-lines)):
- `new item in a category a formula enumerates`: writes keep adding new
  dimension values (a new day, a new order id) to a category that a
  `PREV`, `POSITION`, `RANK`, `FILL`, window or similar formula reads the
  item list of.
- `two rows on one coordinate`: a measure bound without `agg` gets
  duplicate keys.
- `nondeterministic collation`: a text `min`, `max` or `count_distinct`
  binding over a column with a nondeterministic (case- or
  accent-insensitive) collation changes. Bind a deterministic column, or
  use trigger or commit mode for that model.
- `truncate`: the bound table is truncated and reloaded.
- `old row not replicated`: UPDATE or DELETE statements on a table without
  `REPLICA IDENTITY FULL` (set it).
- `model changed`: something keeps calling `define` or `bind_*` (a
  migration in a loop).

**Fix.** For new items, move the item-list-reading formula to a separate
model in `manual` mode refreshed on a schedule, or bind the fast-growing
column to a category that only simple reductions read. For duplicates,
rebind with an aggregate:
`SELECT improv.bind_measure('<m>', '<measure>', '<table>', '<col>', agg => 'sum')`.
For `TRUNCATE` and reload, switch to `manual`, load, `recalc`, switch back.
For text min/max, accept it or bind a numeric column.

**Check.** Over 15 minutes of normal writes, `incremental_applies` rises
and `rebuilds` does not.

## A slow apply

**Symptom.** Results lag the commit by seconds rather than a poll. With
several models, all of them lag at once.

**Causes.**
- One large model's batches take long, or it rebuilds (above). The worker
  applies models one after the other, so every model waits for it
  (parallel apply is B1 in the [plan][plan]).
- Huge transactions: one transaction changing millions of rows is applied
  as one batch.
- A per-item reduction (`RANK`, `SORT`) over a category with very many
  items: one edit re-reads the whole group (seconds at 1M items).
- `improv.poll_ms` set high.

**Diagnosis.** Compare `last_applied_at` across models: the slow one is
recent while the others are stale. The server log's `rebuilding` lines
name rebuilds. For a closer look, time `improv.recalc('<m>')` in a session,
which is the cost of one rebuild.

**Fix.** Put the slow model in `manual` mode and refresh it on a schedule,
or give it its own database (and server). Break bulk loads into batches
of 10k–100k rows. Lower `improv.poll_ms` if it was raised. The default is
200 ms; in 0.11 a reload applies the change (D9), while on 0.10.0 and
`main` the worker must be restarted for it to take effect.

**Check.** A 1,000-row batch is visible within about two polls.

## Out of memory

**Symptom.** The server log shows `server process (PID …) was terminated
by signal 9: Killed` for the `pg_improv sync` worker, or the kernel log
shows the OOM killer choosing it. Postgres then restarts every backend
(crash recovery), and the worker rebuilds every model, which can be killed
again.

**Cause.** The worker holds every logical model's fold state and engine.
For aggregated bindings that is about 180 B per fact row plus 0.8 kB per
cell ([capacity](./capacity.md#memory)). A model that grew, or several
large models in one database, can exceed the server's memory. Trigger and
commit recomputes and `recalc` load the model in the writer's backend, so
those can run out of memory too.

**Diagnosis.** `RssAnon` of the worker (`grep RssAnon /proc/<pid>/status`)
against the server's memory, and the bound tables' row counts. In-backend
recomputes show up as large `RssAnon` on writers' backends.

**Fix.**
1. Stop the loop. Put the largest models in `manual` mode (from any
   session: `SELECT improv.set_sync_mode('<m>', 'manual')`). If Postgres
   keeps restarting, temporarily set `improv.database` to a database with no
   models and restart.
2. Lower `improv.max_cells` so an oversized model fails with a clear
   `last_error` instead of exhausting memory. It counts bound input cells
   (coordinates after aggregation), not fact rows, so it does not bound
   the fold state of a heavily aggregated binding. A per-model memory cap
   is B9 in the [plan][plan].
3. Add memory, or split the model, then return it to `logical`.

**Check.** The worker's `RssAnon` is stable through a day of writes.

## A stale publication

**Symptom.** A bound table's writes never reach its logical model:
`incremental_applies` does not move after writes to that table, while
other tables work. Or the log shows `pg_improv sync: creating the
publication improv_db: …`.

**Cause.** The publication `improv_db` was edited by hand
(`ALTER PUBLICATION … DROP TABLE`, `DROP PUBLICATION`), or a restore or
replication tool rewrote it.

**Diagnosis.**

```sql
SELECT * FROM pg_publication_tables WHERE pubname = 'improv_db';
-- against what the logical models bind
SELECT DISTINCT b.rel FROM improv.binding b JOIN improv.model m ON m.id = b.model_id
 WHERE m.sync_mode = 'logical';
```

**Fix.**

```sql
SELECT improv.sync_publication();
```

It sets the publication to exactly the union of the logical models'
tables, creating it if it is missing. Changes to a table made while it was
out of the publication were never decoded, so rebuild the affected models
once: `define` them again, or switch each to `manual` and back to
`logical`.

**Check.** The two queries list the same tables, and a test write moves
the model's `incremental_applies`.

## Logical mode in the wrong database

**Symptom.** A model reports `sync_mode = logical` and `slot = improv_db`
with no error, but its results never change.

**Cause.** Its database is not the one in `improv.database`. One worker
serves one database per server. On 0.10.0 and `main`, `set_sync_mode`
accepts `logical` anywhere, and `status` reports the other database's
slot. 0.11 refuses it (D12).

**Fix.** Use `trigger`, `commit` or `manual` in that database, or move the
model ([runbook](./runbook-backup.md#moving-a-model-to-another-database)).

## Permission denied for an improv model

**Symptom.** A role gets `permission denied for improv model "m": only a
superuser or a member of its owner, alice, may change it` (from `define`,
`bind_*`, `recalc`, `set_sync_mode`, `drop_*`), or `… what_if needs to
act as its owner, alice (see Privileges)`. Or it reads 0 rows from
`improv.item` or `improv.cell` for a model it can see in `improv.model`.

**Cause.** Since 0.11.0 only a role that may act as a model's owner (a
superuser, or a member of the owner with `SET`; any member on PG 15)
changes the model, runs `what_if` on it, or reads its items and cells.
Writers to bound tables are unaffected.

**Diagnosis.**

```sql
SELECT owner, improv.may_act_as(owner) FROM improv.model WHERE name = '<m>';
```

**Fix.** The model's owner decides. To let a co-owner change the model,
`GRANT alice TO <role> WITH INHERIT FALSE, SET TRUE` (PG 15: `GRANT alice
TO <role>`). To give a read-only analyst `what_if` on one model and
nothing else, the owner creates a `SECURITY DEFINER` wrapper and grants
`EXECUTE` on it. The text is in the pg_improv README's
[Privileges](https://codeberg.org/gregburd/improv/src/branch/main/crates/pg_improv/README.md#giving-others-what_if).
Either way the role sees results computed from what the owner can read.

**Check.** The diagnosis query returns `t` for the role (option a), or
the role's call to the wrapper returns rows (option b).

## An upgrade went wrong

**Symptom.** After installing a new release: `ALTER EXTENSION pg_improv
UPDATE` fails; or `CREATE EXTENSION`/the worker complain about a version
mismatch; or the worker does nothing.

**Causes and fixes.**
- **Postgres was not restarted after installing.** The preloaded library
  is still the old one. Restart, then run `ALTER EXTENSION pg_improv UPDATE`.
- **Mixed versions:** the `.so` is new, the SQL objects old (or the
  reverse). Check both:

  ```sql
  SELECT extversion FROM pg_extension WHERE extname = 'pg_improv';
  SELECT default_version FROM pg_available_extensions WHERE name = 'pg_improv';
  ```

  They must match after `ALTER EXTENSION … UPDATE`. The worker does nothing
  until the catalog is at 0.11.0 or later (it waits for
  `improv.sync_publication()` to exist).
- **The update script failed.** It runs in one transaction, so nothing
  changed. Read the error, fix it (often a hand-made object in the
  `improv` schema), and run it again.
- **After 0.10.0 → 0.11.0:** the update drops the old `improv_<model>`
  publications and queues their slots, the worker drops them and creates
  `improv_db`, and every logical model rebuilds once. Until that finishes,
  results lag. `pg_improv sync: dropping queued slots: …` in the log means
  an old slot is still in use; drop it by hand once nothing holds it.
- **Going back:** there are no downgrade scripts. Restore the pre-upgrade
  backup ([runbook](./runbook-backup.md)).

**Check.** `extversion` equals `default_version`, `improv.status` shows
`slot = improv_db` for every logical model, and no `improv_<model>` slot
remains in `pg_replication_slots`.

The tested upgrade matrix from every released version is D6 in the
[plan][plan]. The README's upgrade section is in the
[pg_improv README][readme].

## Removing pg_improv cleanly

On every version, drop the models first and the extension second.
Tested on 0.11.0, `DROP EXTENSION pg_improv CASCADE` alone, with a logical
model present, left the replication slot `improv_db`, the publication
`improv_db`, the result schemas `improv_<model>`, and
`REPLICA IDENTITY FULL` on the bound tables. The slot is the dangerous
part: no worker serves it any more, so it retains WAL forever. In 0.11 the
worker drops an orphaned `improv_db` slot and publication once the
extension is gone (D13 in the [plan][plan]). Result schemas stay in every
version, because they are your data.

1. **Drop every model**, while the extension still works. This removes
   their result schemas and triggers and empties the publication, and the
   worker then drops the slot on its next poll:

   ```sql
   SELECT improv.drop_model(name) FROM improv.model;
   ```

2. **Drop the extension:** `DROP EXTENSION pg_improv;` (no `CASCADE`
   needed once the models are gone).
3. **Remove the preload:** delete `pg_improv` from
   `shared_preload_libraries` and the `improv.*` settings from
   `postgresql.conf`, then restart. Until then the worker keeps running and
   idles.
4. Optionally, reset the replica identity of tables that had no primary
   key with `ALTER TABLE <t> REPLICA IDENTITY DEFAULT`. The worker set
   `FULL` on bound tables without one.

**Check.**

```sql
SELECT nspname FROM pg_namespace WHERE nspname LIKE 'improv%';             -- none (or only improv, if your own tables live there)
SELECT slot_name FROM pg_replication_slots WHERE slot_name LIKE 'improv%';  -- none
SELECT pubname FROM pg_publication WHERE pubname LIKE 'improv%';           -- none
SELECT lanname FROM pg_language WHERE lanname = 'improv';                  -- none
```

Your bound tables and their rows are untouched. This procedure was tested
on PG 18.6, and every check above returned nothing.

**Recovering an orphaned slot (0.10.0 and `main`).** If the extension was
dropped first and `improv_db` (or a pre-0.11 `improv_<model>` slot) is
still in `pg_replication_slots`, drop it once the worker no longer holds
it, and drop the publication. Stopping the worker first makes sure it
does not hold the slot:

```sql
SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE backend_type = 'pg_improv sync';
SELECT pg_drop_replication_slot(slot_name) FROM pg_replication_slots
 WHERE slot_name = 'improv_db' OR slot_name LIKE 'improv\_%';
DROP PUBLICATION IF EXISTS improv_db;
DROP SCHEMA improv_<model> CASCADE;   -- only if you do not want the old results
```

[plan]: https://codeberg.org/gregburd/improv/src/branch/main/docs/reviews/2026-10-21-road-to-1.0.md
[readme]: https://codeberg.org/gregburd/improv/src/branch/main/crates/pg_improv/README.md
