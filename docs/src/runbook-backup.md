# Backup and Restore Runbook

This runbook covers procedures for the person who backs up and restores
databases and model files that hold Improv models. Each one ends with a
**verify** step: a check that tells you the restore worked, not only that
it ran. It describes pg_improv 0.11.0 (`main`). Where 0.11 changes a step,
the step says so.

**Tested.** The `pg_dump`/`pg_restore` procedure (with one model in each
sync mode), the base-backup restore, moving a model between databases,
and the store-file copies (`VACUUM INTO` with a writer running, and a plain
copy with the app closed) were run on 2026-10-21 against PG 18.6 with
pg_improv 0.11.0 built from `main`, and against the `improv` CLI built from
`main`. The `pg_upgrade` and failover sections are short versions of
procedures tested separately (D1, D2 in the [road to 1.0][plan]); they are
not tested here.

## What is where

| Holds | Where | Backed up by |
| --- | --- | --- |
| A model's structure (categories, items, measures, formulas, bindings, views) and its unbound cells | the `improv` schema | `pg_dump` (the catalog is registered with `pg_extension_config_dump`), base backups |
| Bound input data | your own tables | whatever backs them up |
| Derived results | one table per measure in `improv_<model>` | `pg_dump`, base backups (and they can always be recomputed) |
| The logical worker's position | `improv.sync_state` plus the replication slot `improv_db` | the table is dumped; **the slot is in neither a dump nor a base backup** |
| A store-file model | one SQLite file, `model.db`, in WAL mode | a safe file copy (below) |

The slot is the one thing that never comes back. After any restore, the
worker creates a new slot, resets every logical model's recorded position,
and rebuilds each model once from its tables. That costs one full load per
model: about 2–3 s per million fact rows ([capacity](./capacity.md)).
Nothing is lost by it.

## `pg_dump` and `pg_restore`

Works for databases with models in any sync mode.

1. **Dump** as usual, in the custom format:

   ```sh
   pg_dump -Fc -f shop.dump shop
   ```

2. **Prepare the target server.** Install the same pg_improv version (or
   newer). For `logical` and `commit` models, set
   `shared_preload_libraries = 'pg_improv'`; for `logical`, also
   `wal_level = logical` and `improv.database = '<the database>'`.
   Restart, then create the empty database.

3. **Restore in one transaction:**

   ```sh
   createdb shop
   pg_restore --single-transaction --exit-on-error -d shop shop.dump
   ```

   Use `--single-transaction` on every version. It is the safe procedure in
   any case, since a failed restore leaves nothing half-done. On 0.10.0 and
   on `main` it is also required when any model is in `commit` mode. A
   restore without it (or `psql -f` of a plain-format dump) fails with
   `improv: model "<m>" (sync_mode commit) could not recompute before
   COMMIT, … there is no unique or exclusion constraint matching the ON
   CONFLICT specification`, because the bound table's data commits before
   the result table's primary key exists. D10 in the [road to 1.0][plan]
   fixes this in 0.11.

4. **What happens next, by mode:**

   | Mode | After the restore |
   | --- | --- |
   | `logical` | Within a few seconds the worker creates `improv_db`, resets the model's position, and rebuilds the model once (`rebuilds` goes up by one). Then it follows writes. |
   | `trigger` | The restore recreates the sync triggers, so writes are recomputed at once. |
   | `commit` | Works at once (needs the preload in every writing backend). |
   | `manual` | Results are as dumped; call `improv.recalc()` when you want them current. |

5. **Verify:**

   ```sql
   -- every model: no error; logical ones rebuilt once and on improv_db
   SELECT m.name, s.sync_mode, s.slot, s.rebuilds, s.last_error
     FROM improv.model m, improv.status(m.name) s;
   -- the slot exists and is healthy
   SELECT slot_name, wal_status FROM pg_replication_slots WHERE slot_name = 'improv_db';
   -- no result cell differs from a fresh computation (an empty what_if)
   SELECT m.name, count(w.*) AS stale
     FROM improv.model m LEFT JOIN LATERAL improv.what_if(m.name, '[]') w ON true
    GROUP BY 1;
   ```

   `stale` must be 0 for every model. Then write one row into a bound table
   of a logical model and check that its result follows within a second.

   In the test run, the restored results' sums matched the source for all
   four models. Every model had 0 stale cells. The worker created the slot
   and rebuilt the logical model once. A new row then appeared in each
   mode's results: at once in `trigger` and `commit`, within 2 s in
   `logical`, and after `recalc` in `manual`.

## Base backups and point-in-time recovery

`pg_basebackup` and file-level backups (pgBackRest, Barman, WAL-G) copy the
`improv` catalog and the result tables, which are ordinary tables. They do
**not** copy the replication slot: `pg_basebackup` leaves `pg_replslot`
empty. A server started from a base backup, or recovered to a point in
time, therefore has no `improv_db` slot.

1. Restore and start the server as your tool documents. Keep
   `shared_preload_libraries`, `wal_level = logical` and `improv.database`
   in its configuration.
2. **Expect one rebuild per logical model.** Once the server is out of
   recovery, the worker creates a new slot, resets every logical model's
   position and rebuilds it from its tables as they are at the recovery
   point. Writes between that point and the slot's creation are already
   in the tables, so the rebuild includes them.
3. **Verify:** the same queries as after `pg_restore`. In the test,
   `pg_replication_slots` was empty right after the start and showed
   `improv_db reserved` 4 s later, with `rebuilds` up by one and a new row
   followed within a second.

A standby that is promoted is the same case; see failover below.

## `pg_upgrade`

`pg_upgrade` copies logical slots only from a PG 17 or later old cluster,
and only when they are caught up. The simple, safe procedure, on every
version:

1. Stop writes to the bound tables, and wait until `lag_bytes` in
   `improv.status` stops falling.
2. Drop the slot. Stop the worker and drop the slot in quick succession,
   because the worker holds the slot while it polls:

   ```sql
   SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE backend_type = 'pg_improv sync';
   SELECT pg_drop_replication_slot('improv_db');
   ```

3. Install the same pg_improv version for the new major version, run
   `pg_upgrade`, and start the new cluster with the same pg_improv settings.
4. The worker creates a new slot and rebuilds each logical model once.
   Then upgrade the extension if you are also changing its version
   (`ALTER EXTENSION pg_improv UPDATE`).
5. **Verify:** as after `pg_restore`.

The full procedure, tested by `tests/pg_upgrade.sh`, is task D2 in the
[road to 1.0][plan]. The [pg_improv README][readme] carries it once it
lands.

## Failover to a standby

The result tables replicate to physical standbys like any table, so reads
on a standby see the results as of its replay position. The worker does
not run on a standby. Logical slots are not carried over to a standby
unless they are PG 17+ failover slots, which pg_improv does not create
yet. After a promotion:

1. The new primary has no `improv_db` slot. Once it is out of recovery, the
   worker (if the preload and `improv.database` are configured on it)
   creates one and rebuilds each logical model.
2. On the old primary, if it comes back, drop its `improv_db` slot before
   it rejoins as a standby, or it retains WAL.
3. **Verify:** as after `pg_restore`, on the new primary.

The tested procedure (`tests/failover.sh`, with failover slots on PG 17+)
is task D1 in the [road to 1.0][plan], and lands in the [pg_improv
README][readme].

## Moving a model to another database

There is no export function. A model is its catalog rows plus its bound
tables, so the move is: copy the tables, then replay the model's
definition in the target database.

1. **Copy the bound tables** to the target database (any way you like):

   ```sh
   pg_dump -d shop -t sales | psql -d shop2
   ```

2. **Generate the model's definition** from the source catalog. This
   query prints the calls that rebuild it: create the model in `manual`
   mode, bind the categories and measures in their original order, define
   the named LAMBDAs and derived measures, and set the original mode.

   ```sql
   -- psql -d shop -qAt -v model=plan -f gen_model.sql > plan.sql
   SELECT stmt FROM (
     SELECT 0 AS o, 0 AS k, format('SELECT improv.create_model(%L);', m.name) AS stmt
       FROM improv.model m WHERE m.name = :'model'
     UNION ALL
     SELECT 1, 0, format('SELECT improv.set_sync_mode(%L, ''manual'');', m.name)
       FROM improv.model m WHERE m.name = :'model'
     UNION ALL
     SELECT 2, c.id, format('SELECT improv.bind_category(%L, %L, %L, %L);',
                            m.name, c.name, cb.rel::text, cb.col)
       FROM improv.model m JOIN improv.category c ON c.model_id = m.id
       JOIN improv.category_binding cb ON cb.model_id = m.id AND cb.category_id = c.id
      WHERE m.name = :'model'
     UNION ALL
     SELECT 3, me.id, format('SELECT improv.bind_measure(%L, %L, %L, %L%s);',
                             m.name, me.name, b.rel::text, b.value_col,
                             CASE WHEN b.agg IS NULL THEN '' ELSE format(', agg => %L', b.agg) END)
       FROM improv.model m JOIN improv.measure me ON me.model_id = m.id
       JOIN improv.binding b ON b.model_id = m.id AND b.measure_id = me.id
      WHERE m.name = :'model'
     UNION ALL
     SELECT 4, l.id, format('SELECT improv.define(%L, %L);', m.name, l.definition)
       FROM improv.model m JOIN improv.lambda l ON l.model_id = m.id WHERE m.name = :'model'
     UNION ALL
     SELECT 5, me.id, format('SELECT improv.define(%L, %L);', m.name, me.formula)
       FROM improv.model m JOIN improv.measure me ON me.model_id = m.id
      WHERE m.name = :'model' AND me.kind = 'derived'
     UNION ALL
     SELECT 6, 0, format('SELECT improv.set_sync_mode(%L, %L);', m.name, m.sync_mode)
       FROM improv.model m WHERE m.name = :'model'
   ) s ORDER BY o, k;
   ```

   It covers bound models. A model with unbound input cells
   (`improv.cell`), saved views or scenarios keeps those in catalog rows
   this query does not replay. For such a model, open it with the CLI and
   copy the cells, or move the whole database with `pg_dump`. If one
   derived measure reads another defined later, reorder the `define` lines
   (they are emitted in measure-id order).

3. **Replay it** in the target, with the extension installed:

   ```sh
   psql -d shop2 -v ON_ERROR_STOP=1 -f plan.sql
   psql -d shop2 -c "SELECT improv.recalc('plan')"
   ```

4. **Logical mode only works in the database named by
   `improv.database`.** One worker serves one database per server. On
   0.10.0 and `main`, `set_sync_mode(…, 'logical')` succeeds in any other
   database, and `status` even shows `slot = improv_db` with no error, but
   nothing ever applies its changes. 0.11 refuses it (D12 in the
   [road to 1.0][plan]). Moving the logical model to another database of
   the same server means changing `improv.database` (a restart). Otherwise
   use `trigger`, `commit` or `manual` mode there.

5. **Verify:** compare every result table between the two databases:

   ```sh
   for t in revenue total; do
     diff <(psql -d shop  -Atc "TABLE improv_plan.$t ORDER BY 1, 2") \
          <(psql -d shop2 -Atc "TABLE improv_plan.$t ORDER BY 1, 2") && echo "$t identical"
   done
   ```

   Then drop the model in the source with `SELECT improv.drop_model('plan')`.
   That removes its catalog rows, result schema and triggers, and takes
   its tables out of the publication.

In the test the generated script recreated a two-category, four-measure
model, and both result tables were identical in the two databases.

## Store files (Mentat)

A store file is a SQLite database in WAL mode. While a program has it open
there are three files: `model.db`, `model.db-wal` and `model.db-shm`.
Recent commits can live only in `-wal` until a checkpoint copies them
back. **Copying `model.db` alone while a program has it open can lose
recent changes or produce a broken copy.** When every program that had it
open has exited cleanly, only `model.db` remains, and a plain copy is safe.

**With the app closed:**

1. Quit the GUI, TUI and server, and wait for any CLI command to finish.
2. Check that no `model.db-wal` exists, then copy:

   ```sh
   ls model.db-wal 2>/dev/null && echo "still open somewhere" || cp model.db backup.db
   ```

**While it is in use, with `VACUUM INTO`:** SQLite writes a consistent,
compacted copy as of one moment, without stopping writers:

```sh
sqlite3 'file:model.db?mode=ro' "VACUUM INTO 'backup-$(date +%F).db'"
# or, without the sqlite3 tool:
python3 -c "import sqlite3; sqlite3.connect('file:model.db?mode=ro', uri=True).execute(\"VACUUM INTO 'backup.db'\")"
```

The copy is a single file in rollback-journal mode. Improv switches it to
WAL the first time it opens it.

**Verify** either copy:

```sh
python3 -c "import sqlite3; print(sqlite3.connect('backup.db').execute('PRAGMA integrity_check').fetchone()[0])"   # ok
improv list backup.db | head
improv eval backup.db <measure-id>
```

In the test, `VACUUM INTO` ran while a loop of `improv set` commands kept
writing the store. The copy passed `integrity_check`, and `improv eval` read
a consistent earlier state from it. The plain copy, made after the last
command exited and with no `-wal` file present, evaluated to the
original's values.

**Restore** by copying the backup over `model.db` (with every program
closed), and delete any stale `model.db-wal` and `model.db-shm` next to it
first.

**Autosave.** The GUI writes the model to its store after every change to
the model: each cell edit, formula, import, undo or redo, and **Save View**
(a pivot by itself changes the layout, not the model, and is kept only as
a saved view). The TUI writes when
you save a view, press `w`, and quit. The CLI writes once per command. The
GUI saves in the background; its title has a trailing `•` until the store
has every edit. An autosave that fails is shown in the status line, the
edit stays in the window, and the next save (or **File ▸ Save**) writes it. Your backups are therefore current
as of the last edit, not the last explicit save. If another program saved
the same store in between, the save is refused with a conflict instead of
overwriting it, and the GUI saves nothing more until you reload: use
**File ▸ Revert to Saved** (GUI) or `R` (TUI), then redo the edit.

**Large stores:** an edit saves only what it changed (milliseconds for a
cell, even at 1M cells), but a full save (an import, a refresh) of a
1M-cell store takes about 8 s. Use `VACUUM INTO`, which copies at file
speed, rather than `improv export` and re-import for backups.

[plan]: https://codeberg.org/gregburd/improv/src/branch/main/docs/reviews/2026-10-21-road-to-1.0.md
[readme]: https://codeberg.org/gregburd/improv/src/branch/main/crates/pg_improv/README.md
