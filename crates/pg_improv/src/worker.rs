//! L2: logical-decoding sync, the default `sync_mode`.
//!
//! A background worker (`pg_improv sync`, registered when the library is in
//! `shared_preload_libraries`) reads one logical replication slot per
//! database, `improv_db` (plugin `pgoutput`, publication `improv_db` over
//! the union of every logical-mode model's bound relations; see
//! [`sync_publication`]). Each poll decodes the slot once, routes each
//! committed transaction's changes by relation oid to the models bound
//! there ([`live::route`]), and applies each model's share to its derived
//! results in that model's own worker transaction, which also records the
//! model's progress (`improv.sync_state.confirmed_lsn`). A model skips
//! transactions that ended at or before its own progress, so a crash
//! re-reads and re-applies only what was not recorded (the result writes
//! are idempotent upserts and deletes) and never loses a change.
//!
//! Every model applies a poll's batch in the same pass, so the slot is then
//! confirmed (`pg_replication_slot_advance`) to the batch's end. A model
//! that fails (its `last_error` says why) has dropped its live state, and
//! its retry rebuilds it from its tables, which needs none of the WAL it
//! skipped: a failing model never holds the others' slot back. Retries back
//! off from 0.5 s to 60 s, so a failing model's rebuilds do not stall the
//! others.
//!
//! Slots cannot be created in a transaction that has already written, so
//! `improv.enable_logical` (called by `bind_measure` and
//! `set_sync_mode(…, 'logical')`) only updates the publication and records
//! the intent in `improv.sync_state`; the worker creates the slot at the
//! start of its own fresh transaction.
//!
//! Apply strategy (G1): incremental. The worker keeps, per model, a [`Live`]
//! state: the loaded model, a live [`Engine`] (its own compute thread, which
//! never calls Postgres) and its last [`Snapshot`]. Each batch's row changes
//! become input-cell edits ([`live::Mapper`]); the engine recomputes only
//! what they affect, and only the measures whose snapshot changed are diffed
//! and written. The worker rebuilds (a full load + compute + diff against
//! the result tables) when there is no live state or the model's
//! `model_version` moved (a structural change such as `define`, checked on
//! every poll, data change or not), and when a batch cannot be applied cell
//! by cell (two rows on one coordinate, or a new item in a category whose
//! item list a formula reads: see [`Engine::serves`]). New dimension values
//! are minted in the worker's transaction. Aggregated bindings keep fold state
//! in the mapper, seeded at each rebuild ([`seed_folds`]). Any error drops
//! the live state, so the next poll rebuilds from the tables.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::{Duration, Instant};

use improv_core_model::{MeasureId, Model};
use improv_engine::session::{Engine, MeasureValues, Snapshot};
use improv_pg_model as pm;
use improv_pg_model::live::{self, Rebuild};
use improv_pg_model::pgoutput;
use pgrx::bgworkers::*;
use pgrx::prelude::*;
use pgrx::{GucContext, GucFlags, GucRegistry, GucSetting};

use super::{
    bump_version, ensure_table, id32, lock_model, mint, read_results, req, write_results, Res,
    MAX_CELLS,
};

/// `improv.database`: the database the sync worker connects to.
pub(crate) static DATABASE: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(Some(c"postgres"));
/// `improv.poll_ms`: how often the worker polls the slot.
pub(crate) static POLL_MS: GucSetting<i32> = GucSetting::<i32>::new(200);
/// `improv.max_changes`: the most decoded changes one poll reads.
pub(crate) static MAX_CHANGES: GucSetting<i32> = GucSetting::<i32>::new(10_000);

/// Whether this library was in `shared_preload_libraries`, so a sync worker
/// runs (a backend inherits it from the postmaster's `_PG_init`).
static WORKER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The database's one slot and publication.
pub(crate) const SLOT: &str = "improv_db";

/// Register the L2 GUCs (always) and the worker (only under preload).
pub(crate) fn init() {
    // SAFETY: a plain read of a postmaster global during _PG_init.
    let preload = unsafe { pg_sys::process_shared_preload_libraries_in_progress };
    // `improv.database` picks the worker's database at postmaster start, so
    // it is a Postmaster GUC; Postgres refuses to define one after startup
    // (a FATAL that kills the backend), so a plain `CREATE EXTENSION`
    // without the preload must not register it. There is no worker then
    // either, so nothing reads it.
    if preload {
        GucRegistry::define_string_guc(
            c"improv.database",
            c"Database the pg_improv sync worker connects to.",
            c"The logical-decoding worker serves the Improv models of this one database.",
            &DATABASE,
            GucContext::Postmaster,
            GucFlags::default(),
        );
    }
    GucRegistry::define_int_guc(
        c"improv.poll_ms",
        c"How often the pg_improv sync worker polls its slot, in ms.",
        c"Lower is fresher results; higher is fewer idle wakeups.",
        &POLL_MS,
        10,
        60_000,
        GucContext::Sighup,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"improv.max_changes",
        c"Most decoded changes one poll of the sync worker reads.",
        c"Bounds the work (and memory) of one poll.",
        &MAX_CHANGES,
        1,
        i32::MAX,
        GucContext::Sighup,
        GucFlags::default(),
    );

    if preload {
        WORKER.store(true, std::sync::atomic::Ordering::Relaxed);
        BackgroundWorkerBuilder::new("pg_improv sync")
            .set_function("improv_worker_main")
            .set_library("pg_improv")
            .set_argument(0i32.into_datum())
            .enable_spi_access()
            .set_start_time(BgWorkerStartTime::RecoveryFinished)
            .set_restart_time(Some(Duration::from_secs(5)))
            .load();
    }
}

extension_sql!(
    r#"
CREATE TABLE improv.sync_state (
    model_id        bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    slot            text NOT NULL,
    publication     text NOT NULL,
    confirmed_lsn   pg_lsn,
    last_applied_at timestamptz,
    rows_applied    bigint NOT NULL DEFAULT 0,
    last_error      text,
    -- Batches applied incrementally, and full rebuilds, since the slot was
    -- made (the sync worker's work, for improv.status).
    incremental_applies bigint NOT NULL DEFAULT 0,
    rebuilds        bigint NOT NULL DEFAULT 0
);
-- Slots no longer used (0.10.0's per-model slots): the worker drops them.
CREATE TABLE improv.slot_to_drop (slot text PRIMARY KEY);
-- A model's sync state is part of it, so a dump keeps it. A restored
-- database has no slot: the worker creates one and resets the row's
-- progress, then catches up with one full recompute.
SELECT pg_catalog.pg_extension_config_dump('improv.sync_state', '');
"#,
    name = "l2_logical",
    requires = ["catalog"]
);

/// The database the sync worker serves when it runs (`improv.database`),
/// or `None` when the library is not preloaded (then no worker runs).
fn served_database() -> Option<String> {
    WORKER
        .load(std::sync::atomic::Ordering::Relaxed)
        .then(|| DATABASE.get())
        .flatten()
        .and_then(|c| c.into_string().ok())
}

/// Refuse logical mode for `model` in a database no sync worker serves:
/// the model would report logical mode and never follow its tables.
/// Without the preload there is no worker at all, and logical models
/// update only through `recalc` (documented), so that is not refused.
pub(crate) fn check_served(model: &str) -> Res<()> {
    // The pg_test cluster's worker serves `postgres` (pgrx drops and
    // recreates its own test database), and the tests' models are logical.
    if cfg!(feature = "pg_test") {
        return Ok(());
    }
    let Some(db) = served_database() else {
        return Ok(());
    };
    let here = Spi::get_one::<String>("SELECT current_database()::text")?.unwrap_or_default();
    if here == db {
        return Ok(());
    }
    pgrx::pg_sys::panic::ErrorReport::new(
        PgSqlErrorCode::ERRCODE_OBJECT_NOT_IN_PREREQUISITE_STATE,
        format!(
            "logical sync of model {model:?} needs the sync worker, which serves database \
             {db:?}, not {here:?}"
        ),
        "improv logical sync",
    )
    .set_hint(
        "Set improv.database to this database and restart Postgres, or use \
         improv.set_sync_mode(model, 'commit'), 'trigger' or 'manual' here.",
    )
    .report(PgLogLevel::ERROR);
    unreachable!("an ERROR does not return")
}

/// Ensure model `model` can be synced by logical decoding: give PK-less
/// bound relations `REPLICA IDENTITY FULL`, add them to the database's
/// publication `improv_db` ([`sync_publication`]), and record the model in
/// `improv.sync_state`. Idempotent. The worker creates the slot.
#[pg_extern(security_invoker)]
fn enable_logical(model: &str) -> Res<()> {
    let (mid, name) = lock_model(model)?;
    enable(mid, &name)
}

pub(crate) fn enable(mid: i64, _name: &str) -> Res<()> {
    // Before publishing: Postgres rejects UPDATE/DELETE on a published table
    // that has no replica identity.
    ensure_replica_identity(mid)?;
    Spi::run_with_args(
        "INSERT INTO improv.sync_state (model_id, slot, publication) VALUES ($1, $2, $2) \
         ON CONFLICT (model_id) DO NOTHING",
        &[mid.into(), SLOT.into()],
    )?;
    sync_publication()
}

/// Undo [`enable`]: forget the model's sync state and take relations no
/// other logical-mode model binds out of the publication.
pub(crate) fn disable(mid: i64, _name: &str) -> Res<()> {
    Spi::run_with_args(
        "DELETE FROM improv.sync_state WHERE model_id = $1",
        &[mid.into()],
    )?;
    sync_publication()?;
    bump_version(mid)
}

/// Set the publication `improv_db` to the union of the relations bound by
/// every logical-mode model with sync state (creating it when missing), so
/// one slot carries every model's changes. Run by `bind_measure`,
/// `drop_measure`, `set_sync_mode` and `drop_model`; idempotent, and
/// callable by hand to repair the publication.
#[pg_extern(security_invoker)]
fn sync_publication() -> Res<()> {
    // Serializes publication changes: two sessions' SET TABLE must not
    // interleave with each other's catalog reads.
    Spi::run("SELECT pg_advisory_xact_lock(hashtext('improv.sync_publication'))")?;
    let rels: Vec<String> = Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT DISTINCT b.rel::text FROM improv.binding b \
             JOIN improv.model m ON m.id = b.model_id \
             JOIN improv.sync_state s ON s.model_id = m.id \
             WHERE m.sync_mode = 'logical' ORDER BY 1",
            None,
            &[],
        )?
        .map(|r| req(&r, 1))
        .collect()
    })?;
    let exists = Spi::get_one_with_args::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_publication WHERE pubname = $1)",
        &[SLOT.into()],
    )? == Some(true);
    let q = pm::quote_ident(SLOT);
    // A partition's changes are published as its root's, the relation the
    // models bind (and route by), in the root's column layout.
    // shortcut: a model that binds a partition directly misses its changes
    // while another logical model binds the root; bind the root instead.
    let via_root = "WITH (publish_via_partition_root = true)";
    match (exists, rels.is_empty()) {
        (false, true) => Spi::run(&format!("CREATE PUBLICATION {q} {via_root}"))?,
        (false, false) => Spi::run(&format!(
            "CREATE PUBLICATION {q} FOR TABLE {} {via_root}",
            rels.join(", ")
        ))?,
        // `SET TABLE` with an empty list is a syntax error.
        (true, true) => {
            let have: Vec<String> = Spi::connect(|c| -> Res<_> {
                c.select(
                    "SELECT format('%I.%I', schemaname, tablename) FROM pg_publication_tables \
                     WHERE pubname = $1",
                    None,
                    &[SLOT.into()],
                )?
                .map(|r| req(&r, 1))
                .collect()
            })?;
            if !have.is_empty() {
                Spi::run(&format!(
                    "ALTER PUBLICATION {q} DROP TABLE {}",
                    have.join(", ")
                ))?;
            }
        }
        (true, false) => Spi::run(&format!(
            "ALTER PUBLICATION {q} SET TABLE {}",
            rels.join(", ")
        ))?,
    }
    // Made before 0.12.0, without it.
    let via = Spi::get_one_with_args::<bool>(
        "SELECT pubviaroot FROM pg_publication WHERE pubname = $1",
        &[SLOT.into()],
    )?;
    if via == Some(false) {
        Spi::run(&format!(
            "ALTER PUBLICATION {q} SET (publish_via_partition_root = true)"
        ))?;
    }
    Ok(())
}

/// Give every bound relation without a primary key, and every relation an
/// aggregated binding reads (its folds need the old row's value),
/// `REPLICA IDENTITY FULL`, so its updated and deleted rows carry the old
/// values. The caller must not hold the table open (see `bind_measure`).
///
/// Each partition of a bound partitioned table too: a partition's old row
/// is sent by its own replica identity (which `ALTER TABLE` on the root does
/// not set), and Postgres refuses an UPDATE or DELETE of a published
/// partition that has none.
fn ensure_replica_identity(mid: i64) -> Res<()> {
    let rels: Vec<String> = Spi::connect(|c| -> Res<_> {
        c.select(
            &format!(
                "{TREE} SELECT DISTINCT c.oid::regclass::text FROM tree t \
                 JOIN pg_class c ON c.oid = t.rel \
                 WHERE c.relkind IN ('r', 'p') AND c.relreplident <> 'f' AND (NOT EXISTS ( \
                   SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary) \
                   OR EXISTS (SELECT 1 FROM improv.binding b WHERE b.model_id = $1 \
                     AND b.rel = t.root AND b.agg IS NOT NULL)) \
                 ORDER BY 1"
            ),
            None,
            &[mid.into()],
        )?
        .map(|r| req(&r, 1))
        .collect()
    })?;
    for rel in rels {
        Spi::run(&format!("ALTER TABLE {rel} REPLICA IDENTITY FULL"))?;
    }
    Ok(())
}

/// `tree(root, rel)`: model `$1`'s bound relations (`root`) and every
/// partition (or inheritance child) below them, read from `pg_inherits`
/// without the locks `pg_partition_tree` takes on every partition.
const TREE: &str = "WITH RECURSIVE tree(root, rel) AS ( \
       SELECT DISTINCT b.rel::oid, b.rel::oid FROM improv.binding b WHERE b.model_id = $1 \
     UNION \
       SELECT t.root, i.inhrelid FROM pg_inherits i JOIN tree t ON i.inhparent = t.rel)";

/// One row of `improv.status` (see [`status`] for the column names).
type StatusRow = (
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<pgrx::datum::TimestampWithTimeZone>,
    Option<i64>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

/// How model `model`'s results are kept current, and (in logical mode) how
/// far the worker is behind: `lag_bytes` is the WAL between the current
/// insert position and the shared slot's confirmed position (a failing
/// model does not hold the slot: see `last_error`). `incremental_applies`
/// counts the batches the worker applied to its live engine, `rebuilds`
/// the full loads (the first catch-up, structural changes, and batches it
/// could not apply incrementally).
// The `name!` tuple must be spelled inline: pgrx cannot derive SQL through
// a type alias, so clippy's type-complexity suggestion does not apply.
#[allow(clippy::type_complexity)]
#[pg_extern(security_invoker)]
fn status(
    model: &str,
) -> TableIterator<
    'static,
    (
        name!(sync_mode, String),
        name!(slot, Option<String>),
        name!(lag_bytes, Option<i64>),
        name!(confirmed_lsn, Option<String>),
        name!(last_applied_at, Option<pgrx::datum::TimestampWithTimeZone>),
        name!(rows_applied, Option<i64>),
        name!(last_error, Option<String>),
        name!(incremental_applies, Option<i64>),
        name!(rebuilds, Option<i64>),
    ),
> {
    // pgrx 0.17 cannot generate SQL for a `Result<TableIterator>`.
    match status_row(model) {
        Ok(row) => TableIterator::once(row),
        Err(e) => error!("{e}"),
    }
}

fn status_row(model: &str) -> Res<StatusRow> {
    let row = Spi::connect(|c| -> Res<_> {
        let t = c.select(
            "SELECT m.sync_mode, s.slot, \
                    (CASE WHEN pg_is_in_recovery() THEN pg_last_wal_replay_lsn() \
                          ELSE pg_current_wal_lsn() END - r.confirmed_flush_lsn)::int8, \
                    r.confirmed_flush_lsn::text, s.last_applied_at, s.rows_applied, \
                    coalesce(CASE WHEN r.wal_status = 'lost' THEN \
                      'the slot ' || s.slot || ' is invalidated (the WAL it held is gone): \
                       the sync worker makes a new one and rebuilds every model' END, \
                    CASE WHEN m.sync_mode = 'logical' AND $2::text IS DISTINCT FROM \
                               current_database()::text THEN \
                      'no sync worker serves this database (improv.database = ' \
                      || coalesce($2, 'unset: pg_improv is not preloaded') || ')' END, \
                    s.last_error), \
                    s.incremental_applies, s.rebuilds \
             FROM improv.model m \
             LEFT JOIN improv.sync_state s ON s.model_id = m.id \
             LEFT JOIN pg_replication_slots r ON r.slot_name = s.slot \
               AND r.database = current_database() \
             WHERE m.name = $1",
            None,
            &[model.into(), served_database().into()],
        )?;
        let r = t.first();
        if r.is_empty() {
            return Err(format!("no Improv model named {model:?}").into());
        }
        Ok((
            r.get::<String>(1)?.unwrap_or_default(),
            r.get::<String>(2)?,
            r.get::<i64>(3)?,
            r.get::<String>(4)?,
            r.get::<pgrx::datum::TimestampWithTimeZone>(5)?,
            r.get::<i64>(6)?,
            r.get::<String>(7)?,
            r.get::<i64>(8)?,
            r.get::<i64>(9)?,
        ))
    })?;
    Ok(row)
}

/// The sync worker's entry point.
#[pg_guard]
#[unsafe(no_mangle)]
pub extern "C-unwind" fn improv_worker_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
    // pg_upgrade starts both clusters in binary-upgrade mode with their
    // configured preload, and checks (then copies) the slot: hold nothing.
    // Status 0, so the postmaster does not restart the worker.
    // SAFETY: a plain read of a postmaster global, and a normal exit.
    unsafe {
        if pg_sys::IsBinaryUpgrade {
            pg_sys::proc_exit(0);
        }
    }
    let db = DATABASE
        .get()
        .and_then(|c| c.into_string().ok())
        .unwrap_or_else(|| "postgres".into());
    BackgroundWorker::connect_worker_to_spi(Some(&db), None);
    // The slot sends values as text in this session's style; `live` reads
    // dates as ISO and a zoneless `timestamp` as UTC, as the docs promise.
    BackgroundWorker::transaction(|| {
        Spi::run("SET TimeZone = 'UTC'; SET DateStyle = 'ISO'").expect("set the session style")
    });
    log!("pg_improv sync worker started on database {db:?}");
    loop {
        let poll = Duration::from_millis(POLL_MS.get().max(10) as u64);
        if !BackgroundWorker::wait_latch(Some(poll)) {
            break; // SIGTERM
        }
        if BackgroundWorker::sighup_received() {
            // pgrx's latch wait does not reload the configuration (its
            // SIGHUP handler only flags it): reload here, as a backend's
            // main loop does, so `poll_ms` and `max_changes` follow
            // `pg_reload_conf()`.
            // SAFETY: outside any transaction, as `PostgresMain` calls it.
            unsafe {
                (&raw mut pg_sys::ConfigReloadPending).write_volatile(0);
                pg_sys::ProcessConfigFile(pg_sys::GucContext::PGC_SIGHUP);
            }
            log!(
                "pg_improv sync: reloaded the configuration (poll_ms = {}, max_changes = {})",
                POLL_MS.get(),
                MAX_CHANGES.get()
            );
        }
        // Nothing to do until the extension (0.11.0 or later: the shared
        // slot) is installed in this database. (A Postgres ERROR is not a
        // Rust `Err`: querying a missing `improv.*` table would unwind out
        // of the worker and kill it.)
        if !BackgroundWorker::transaction(installed) {
            // A re-created extension reuses model ids: forget them.
            LIVE.with(|l| l.borrow_mut().clear());
            PEEKED_TO.set(0);
            release_slot();
            orphaned();
            continue;
        }
        sweep();
    }
    log!("pg_improv sync worker stopping");
    // A bgworker that exits with status 0 is never restarted. Exit 1 so a
    // `pg_terminate_backend` (or SIGTERM) brings the worker back after
    // `bgw_restart_time`; the slot position makes that restart lossless.
    // During a server shutdown the postmaster starts no workers, so this is
    // harmless then.
    // SAFETY: `proc_exit` runs the backend's exit callbacks and never returns.
    unsafe { pg_sys::proc_exit(1) };
}

/// `f` in its own worker transaction, a Postgres ERROR as an `Err`.
fn in_txn<R>(f: impl FnOnce() -> Res<R>) -> Result<R, String> {
    let f = AssertUnwindSafe(f);
    BackgroundWorker::transaction(move || {
        PgTryBuilder::new(move || {
            // The whole wrapper, not its field: closures capture disjointly.
            let f = f;
            (f.0)().map_err(|e| e.to_string())
        })
        .catch_others(|e| Err(format!("{e:?}")))
        .execute()
    })
}

/// WAL the slot may lag with nothing to apply before an idle advance.
const IDLE_ADVANCE: u64 = 16 << 20;
/// A failing model's first retry delay, doubled per failure up to the max.
const RETRY_FIRST: Duration = Duration::from_millis(500);
const RETRY_MAX: Duration = Duration::from_secs(60);

/// One poll: decode the shared slot once, route each transaction's changes
/// to the models bound to its relations, apply each model's share in its
/// own transaction, then confirm the slot. Every model applies the batch in
/// the same pass, so the slot moves to its end; a model that fails has
/// dropped its live state and rebuilds from its tables on its next poll,
/// which does not need the WAL it skipped (so it never holds the others
/// back). Each model records its own progress, and skips transactions that
/// ended at or before it: a crash between the models' commits and the
/// slot's advance re-reads the batch, and re-applies none of it.
fn sweep() {
    release_slot();
    let listed = match in_txn(list_models) {
        Ok(m) => m,
        Err(e) => return log!("pg_improv sync: listing models: {e}"),
    };
    LIVE.with(|l| {
        l.borrow_mut()
            .retain(|mid, _| listed.iter().any(|m| m.id == *mid))
    });
    RETRY.with(|r| {
        r.borrow_mut()
            .retain(|mid, _| listed.iter().any(|m| m.id == *mid))
    });
    PARTITIONS.with(|p| {
        p.borrow_mut()
            .retain(|mid, _| listed.iter().any(|m| m.id == *mid))
    });
    if listed.is_empty() {
        // No model to serve: drop the slot rather than let it retain WAL;
        // the next model to enter logical mode gets a new one.
        if let Err(e) = in_txn(|| drop_slot(SLOT)) {
            log!("pg_improv sync: dropping {SLOT}: {e}");
        }
        drop_queued();
        return;
    }
    // Slot creation needs a transaction that has not written (and a failed
    // attempt leaves a half-made slot with no plugin): its own transaction.
    match in_txn(ensure_slot) {
        Err(e) => return slot_error(&listed, &format!("creating the slot {SLOT}: {e}")),
        // The publication was dropped by hand: make it; the slot follows.
        Ok(None) => {
            if let Err(e) = in_txn(sync_publication) {
                slot_error(&listed, &format!("creating the publication {SLOT}: {e}"));
            }
            return;
        }
        Ok(Some(Slot::Kept)) => {}
        // A new slot starts at the current WAL position, so the models'
        // recorded progress (a restored dump's, an upgrade's, or a slot
        // dropped by hand or invalidated) no longer applies: each catches up
        // from scratch. An invalidation stays each model's `last_error`
        // until its rebuild succeeds.
        Ok(Some(made)) => {
            log!("pg_improv sync: created the slot {SLOT}");
            let note = match made {
                Slot::Recreated(why) => {
                    let note = format!(
                        "the slot {SLOT} was invalidated ({why}), so the WAL it held is gone: \
                         made a new one, and every model rebuilds from its tables"
                    );
                    log!("pg_improv sync: {note}");
                    Some(note)
                }
                _ => None,
            };
            let reset = in_txn(|| {
                Spi::run_with_args(
                    "UPDATE improv.sync_state \
                     SET confirmed_lsn = NULL, last_applied_at = NULL, last_error = $1",
                    &[note.clone().into()],
                )?;
                Ok(())
            });
            if let Err(e) = reset {
                return log!("pg_improv sync: resetting progress: {e}");
            }
        }
    }
    let peeked = match in_txn(peek) {
        Ok(Some(p)) => p,
        // No WAL since the last decode: nothing to route, nor to confirm.
        Ok(None) => Peeked {
            stream: pgoutput::Stream::default(),
            full: true,
            flush: 0,
        },
        Err(e) => return slot_error(&listed, &format!("reading the slot {SLOT}: {e}")),
    };
    slot_ok();
    let mut by_rel: HashMap<u32, Vec<i64>> = HashMap::new();
    for m in &listed {
        for r in &m.rels {
            by_rel.entry(*r).or_default().push(m.id);
        }
    }
    let mut routed = live::route(&peeked.stream.done, &by_rel);
    let rels = &peeked.stream.relations;
    for m in &listed {
        let txns = routed.remove(&m.id).unwrap_or_default();
        let moved = PARTITIONS.with(|p| {
            p.borrow_mut()
                .insert(m.id, m.partitions.clone())
                .is_some_and(|was| was != m.partitions)
        });
        if moved {
            log!(
                "pg_improv sync: model {}: rebuilding (partitions changed)",
                m.name
            );
            LIVE.with(|l| l.borrow_mut().remove(&m.id));
            // An attached partition needs the root's replica identity.
            if let Err(e) = in_txn(|| ensure_replica_identity(m.id)) {
                log!("pg_improv sync: model {}: {e}", m.name);
            }
        }
        let current = LIVE.with(|l| {
            l.borrow()
                .get(&m.id)
                .is_some_and(|l| l.version == m.version && l.name == m.name)
        });
        if txns.is_empty() && m.caught_up && current {
            continue; // nothing for it, and its live state is current
        }
        // A failing model is retried with backoff: each retry is a full
        // rebuild, which must not stall the other models every poll.
        let now = Instant::now();
        if RETRY.with(|r| r.borrow().get(&m.id).is_some_and(|(at, _)| now < *at)) {
            continue;
        }
        let r = in_txn(|| tick(m.id, &m.name, txns, rels));
        RETRY.with(|retry| {
            let mut retry = retry.borrow_mut();
            match &r {
                Ok(()) => {
                    retry.remove(&m.id);
                }
                Err(_) => {
                    let wait = retry
                        .get(&m.id)
                        .map_or(RETRY_FIRST, |(_, w)| (*w * 2).min(RETRY_MAX));
                    retry.insert(m.id, (now + wait, wait));
                }
            }
        });
        if let Err(e) = r {
            let _ = in_txn(|| {
                Spi::run_with_args(
                    "UPDATE improv.sync_state SET last_error = $2 WHERE model_id = $1",
                    &[m.id.into(), e.clone().into()],
                )?;
                Ok(())
            });
            log!("pg_improv sync: model {}: {e}", m.name);
        }
    }
    // A poll that stopped short of `max_changes` decoded the WAL up to at
    // least the flush position read before it, so the slot may move there
    // even past WAL with nothing for any model (which otherwise it would
    // retain forever while the bound tables are idle).
    let last = peeked.stream.done.last().map(|t| t.end_lsn);
    let target = if peeked.full {
        last
    } else {
        Some(last.unwrap_or(0).max(peeked.flush))
    };
    let Some(target) = target else {
        return drop_queued();
    };
    // After a standby snapshot, advance once more even when idle, so the
    // restart point passes it and idle peeks stop re-decoding the batch.
    let eager = last.is_some() || SNAPSHOT_PENDING.get();
    match in_txn(|| advance(target, eager)) {
        Err(e) => log!("pg_improv sync: advancing {SLOT}: {e}"),
        Ok(true) if last.is_none() => SNAPSHOT_PENDING.set(false),
        Ok(true) => {
            SNAPSHOT_PENDING.set(true);
            // The slot's restart point only moves past a running-xacts WAL
            // record (the bgwriter logs one every 15 s), and every peek and
            // advance re-decodes from it. Log one now, once per poll that
            // applied something, in a transaction without an xid (one
            // logged inside a model's transaction names it as running and
            // pins the restart point), so the next advance moves it here
            // and a poll re-reads about one batch of WAL.
            let _ = in_txn(|| {
                Spi::connect(|c| {
                    c.select(
                        "SELECT pg_log_standby_snapshot() WHERE NOT pg_is_in_recovery()",
                        None,
                        &[],
                    )
                    .map(|_| ())
                })?;
                Ok(())
            });
        }
        Ok(_) => {}
    }
    drop_queued();
}

/// With pg_improv not installed (a `DROP EXTENSION`, which cannot drop a
/// slot), drop the database's `improv_db` slot, which would retain WAL
/// forever, and its publication. Logs once per drop; idle otherwise.
fn orphaned() {
    let r = in_txn(|| {
        let slot = Spi::connect(|c| -> Res<_> {
            let t = c.select(
                "SELECT EXISTS (SELECT 1 FROM pg_replication_slots \
                   WHERE slot_name = $1 AND database = current_database()), \
                        EXISTS (SELECT 1 FROM pg_publication WHERE pubname = $1)",
                None,
                &[SLOT.into()],
            )?;
            let r = t.first();
            Ok((
                r.get::<bool>(1)? == Some(true),
                r.get::<bool>(2)? == Some(true),
            ))
        })?;
        if slot.0 {
            // Read-only SPI: a slot drop refuses a transaction that wrote.
            Spi::connect(|c| -> Res<()> {
                c.select("SELECT pg_drop_replication_slot($1)", None, &[SLOT.into()])?;
                Ok(())
            })?;
        }
        Ok(slot)
    });
    match r {
        Ok((false, false)) => {}
        Ok((slot, publication)) => {
            if publication {
                if let Err(e) = in_txn(|| {
                    Spi::run(&format!("DROP PUBLICATION {}", pm::quote_ident(SLOT)))?;
                    Ok(())
                }) {
                    return log!("pg_improv sync: dropping the publication {SLOT}: {e}");
                }
            }
            log!(
                "pg_improv sync: pg_improv is not installed in this database: dropped the \
                 {} {SLOT}, which nothing reads",
                match (slot, publication) {
                    (true, true) => "slot and publication",
                    (true, false) => "slot",
                    _ => "publication",
                }
            );
        }
        Err(e) => log!("pg_improv sync: dropping the slot {SLOT}: {e}"),
    }
}

/// A Postgres ERROR in a slot function (the peek of an invalidated slot)
/// leaves the slot acquired by this session, and an acquired slot cannot be
/// dropped: a background worker has no `PostgresMain` error cleanup to
/// release it, so each poll does that first.
fn release_slot() {
    // SAFETY: a read of this backend's own global, and the release of the
    // slot it holds, as `PostgresMain`'s error cleanup does.
    unsafe {
        if !pg_sys::MyReplicationSlot.is_null() {
            pg_sys::ReplicationSlotRelease();
        }
    }
}

/// The shared slot (or its publication) failed, so no model follows its
/// tables: record why as every model's `last_error`, which `improv.status`
/// shows, and log it once rather than every poll.
fn slot_error(listed: &[Listed], e: &str) {
    if SLOT_ERROR
        .with(|s| s.replace(Some(e.to_string())))
        .as_deref()
        != Some(e)
    {
        log!("pg_improv sync: {e}");
    }
    let ids: Vec<i64> = listed.iter().map(|m| m.id).collect();
    let _ = in_txn(|| {
        Spi::run_with_args(
            "UPDATE improv.sync_state SET last_error = $2 \
             WHERE model_id = ANY($1) AND last_error IS DISTINCT FROM $2",
            &[ids.into(), e.into()],
        )?;
        Ok(())
    });
}

/// The slot was read after a [`slot_error`]: clear that error.
fn slot_ok() {
    if let Some(e) = SLOT_ERROR.with(|s| s.take()) {
        log!("pg_improv sync: reading the slot {SLOT} again");
        let _ = in_txn(|| {
            Spi::run_with_args(
                "UPDATE improv.sync_state SET last_error = NULL WHERE last_error = $1",
                &[e.into()],
            )?;
            Ok(())
        });
    }
}

fn drop_queued() {
    if let Err(e) = in_txn(drop_queued_slots) {
        log!("pg_improv sync: dropping queued slots: {e}");
    }
}

/// Whether pg_improv's L2 catalog, 0.11.0's shared slot included, exists in
/// the worker's database.
fn installed() -> bool {
    // Read-only SPI: pgrx's `get_one` assigns a transaction id, so every
    // idle poll would write a commit record (see [`advance`]).
    Spi::connect(|c| {
        c.select(
            "SELECT to_regclass('improv.sync_state') IS NOT NULL \
               AND to_regclass('improv.slot_to_drop') IS NOT NULL \
               AND to_regprocedure('improv.sync_publication()') IS NOT NULL",
            None,
            &[],
        )?
        .first()
        .get::<bool>(1)
    })
    .ok()
    .flatten()
        == Some(true)
}

/// A logical-mode model with sync state, as one poll sees it.
struct Listed {
    id: i64,
    name: String,
    /// Its `model_version`.
    version: i64,
    /// Whether it has caught up once since its progress was reset.
    caught_up: bool,
    /// The relations it binds.
    rels: Vec<u32>,
    /// Their partitions, each with its relfilenode: attaching, detaching or
    /// dropping a partition, or truncating one (published via its root,
    /// a partition's `TRUNCATE` is not sent), moves rows without a row
    /// change in the WAL, so a change here rebuilds.
    partitions: Option<String>,
}

/// The models in logical mode that have sync state, in id order.
fn list_models() -> Res<Vec<Listed>> {
    Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT m.id, m.name, coalesce(v.tx, 0), \
                    s.confirmed_lsn IS NOT NULL OR s.last_applied_at IS NOT NULL, \
                    ARRAY(SELECT DISTINCT b.rel::oid::int8 FROM improv.binding b \
                          WHERE b.model_id = m.id), \
                    (SELECT string_agg(c.oid || ':' || c.relfilenode, ',' ORDER BY c.oid) \
                     FROM pg_class c WHERE c.oid IN ( \
                       WITH RECURSIVE p(rel) AS ( \
                           SELECT i.inhrelid FROM improv.binding b \
                           JOIN pg_inherits i ON i.inhparent = b.rel WHERE b.model_id = m.id \
                         UNION \
                           SELECT i.inhrelid FROM pg_inherits i JOIN p ON i.inhparent = p.rel) \
                       SELECT rel FROM p)) \
             FROM improv.model m JOIN improv.sync_state s ON s.model_id = m.id \
             LEFT JOIN improv.model_version v ON v.model_id = m.id \
             WHERE m.sync_mode = 'logical' ORDER BY m.id",
            None,
            &[],
        )?
        .map(|r| {
            Ok(Listed {
                id: req(&r, 1)?,
                name: req(&r, 2)?,
                version: req(&r, 3)?,
                caught_up: req(&r, 4)?,
                rels: req::<Vec<i64>>(&r, 5)?
                    .into_iter()
                    .map(u32::try_from)
                    .collect::<Result<_, _>>()?,
                partitions: r.get(6)?,
            })
        })
        .collect()
    })
}

/// What [`ensure_slot`] did.
enum Slot {
    /// The slot was there, usable.
    Kept,
    /// Made it: there was none, or one without the plugin.
    Created,
    /// Dropped one that was invalidated (why) and made it again.
    Recreated(String),
}

/// Make sure the shared slot exists with the `pgoutput` plugin. Runs in its
/// own worker transaction, before anything in it writes (Postgres refuses
/// to create a slot otherwise). A slot left without a plugin by an earlier
/// failed creation, or invalidated (its WAL removed past
/// `max_slot_wal_keep_size`, or a recovery conflict), is dropped and made
/// again; a peek of an invalidated slot fails every time. Returns what it
/// did (after a new slot the models catch up with a full recompute), or
/// `None` while the publication does not exist: a slot must not start
/// before it, or decoding the WAL in between fails on the missing
/// publication.
fn ensure_slot() -> Res<Option<Slot>> {
    // Read-only SPI only (`Spi::connect` + `select`): pgrx's `get_one*` and
    // `run*` helpers go through `connect_mut`, which assigns a transaction
    // id, and Postgres refuses to create a slot in a transaction that has
    // one ("…that has performed writes").
    let (published, plugin, invalid) = Spi::connect(|c| -> Res<_> {
        // `invalidation_reason` is PG 17+, `conflicting` PG 16+; `lost`
        // (PG 13+) is a slot whose WAL was removed.
        let t = c.select(
            "SELECT EXISTS (SELECT 1 FROM pg_publication WHERE pubname = $1), \
                    (SELECT plugin::text FROM pg_replication_slots WHERE slot_name = $1), \
                    (SELECT coalesce(to_jsonb(r) ->> 'invalidation_reason', \
                              CASE WHEN r.wal_status = 'lost' THEN 'wal_removed' \
                                   WHEN to_jsonb(r) ->> 'conflicting' = 'true' \
                                   THEN 'rows_removed' END) \
                     FROM pg_replication_slots r WHERE r.slot_name = $1)",
            None,
            &[SLOT.into()],
        )?;
        let r = t.first();
        Ok((
            r.get::<bool>(1)? == Some(true),
            r.get::<String>(2)?,
            r.get::<String>(3)?,
        ))
    })?;
    if !published {
        return Ok(None);
    }
    let made = match (plugin.as_deref(), invalid) {
        (Some("pgoutput"), None) => return Ok(Some(Slot::Kept)),
        (None, _) => Slot::Created,
        (Some(_), why) => {
            Spi::connect(|c| -> Res<()> {
                c.select("SELECT pg_drop_replication_slot($1)", None, &[SLOT.into()])?;
                Ok(())
            })?;
            why.map_or(Slot::Created, Slot::Recreated)
        }
    };
    // PG 17+: a failover slot, which a standby with `sync_replication_slots`
    // keeps in step, so after a promotion the worker continues on it with
    // its models' progress (otherwise it makes a new slot and each model
    // rebuilds).
    let create = if cfg!(any(feature = "pg15", feature = "pg16")) {
        "SELECT pg_create_logical_replication_slot($1, 'pgoutput')"
    } else {
        "SELECT pg_create_logical_replication_slot($1, 'pgoutput', failover => true)"
    };
    Spi::connect(|c| -> Res<()> {
        c.select(create, None, &[SLOT.into()])?;
        Ok(())
    })?;
    Ok(Some(made))
}

/// One decode of the shared slot.
struct Peeked {
    stream: pgoutput::Stream,
    /// The decode returned `max_changes` rows, so it may have stopped early.
    full: bool,
    /// The WAL flush position read just before the decode.
    flush: u64,
}

/// The decode under `log_min_messages = fatal`: every decoding context
/// logs "starting logical decoding" and "found consistent point" at LOG,
/// which outranks ERROR in the server log (so `warning` would not hide
/// them), and an idle worker at `poll_ms = 200` made about 310 MB of log a
/// day. The worker catches and logs its errors itself.
const QUIET: &str = "SELECT set_config('log_min_messages', 'fatal', true)";

/// One decode of the shared slot, or `None` when no WAL was written since
/// the last complete one (so an idle worker does not decode at all).
fn peek() -> Res<Option<Peeked>> {
    let max = MAX_CHANGES.get().max(1);
    let got = Spi::connect(|c| -> Res<_> {
        let t = c.select(
            "SELECT pg_current_wal_insert_lsn()::text, pg_current_wal_flush_lsn()::text",
            None,
            &[],
        )?;
        let r = t.first();
        let lsn = |i| -> Res<u64> {
            Ok(r.get::<String>(i)?
                .as_deref()
                .and_then(pgoutput::parse_lsn)
                .ok_or("cannot read the WAL position")?)
        };
        let (insert, flush) = (lsn(1)?, lsn(2)?);
        if insert <= PEEKED_TO.get() {
            return Ok(None);
        }
        c.select(QUIET, None, &[])?;
        let rows: Vec<Vec<u8>> = c
            .select(
                "SELECT data FROM pg_logical_slot_peek_binary_changes( \
                   $1, NULL, $2, 'proto_version', '1', 'publication_names', $1)",
                None,
                &[SLOT.into(), max.into()],
            )?
            .map(|r| req(&r, 1))
            .collect::<Res<_>>()?;
        Ok(Some((flush, rows)))
    })?;
    let Some((flush, rows)) = got else {
        return Ok(None);
    };
    let mut stream = pgoutput::Stream::default();
    for data in &rows {
        stream.feed(data)?;
    }
    let full = rows.len() >= max as usize;
    // A decode that stopped short read everything flushed before it; WAL
    // inserted but not yet flushed is past `flush`, so it is read next poll.
    PEEKED_TO.set(if full { 0 } else { flush });
    Ok(Some(Peeked {
        stream,
        full,
        flush,
    }))
}

/// Confirm the slot up to `target` when it is past the slot's position: at
/// once when `eager`, else only once the slot lags by [`IDLE_ADVANCE`].
/// Returns whether it moved.
///
/// Read-only SPI throughout: a transaction id would write a commit record
/// past the slot's new position, so the slot would never catch up to the
/// end of the WAL. An idle worker would then write WAL (and decode it)
/// every poll, and `pg_upgrade` refuses a slot that has not caught up.
fn advance(target: u64, eager: bool) -> Res<bool> {
    Spi::connect(|c| -> Res<bool> {
        let at = c
            .select(
                "SELECT confirmed_flush_lsn::text FROM pg_replication_slots WHERE slot_name = $1",
                None,
                &[SLOT.into()],
            )?
            .first()
            .get::<String>(1)?
            .as_deref()
            .and_then(pgoutput::parse_lsn)
            .ok_or("the slot is gone")?;
        if target <= at || (!eager && target - at < IDLE_ADVANCE) {
            return Ok(false);
        }
        // The advance decodes too, so it would log the same two lines.
        c.select(QUIET, None, &[])?;
        c.select(
            "SELECT pg_replication_slot_advance($1, $2::pg_lsn)",
            None,
            &[SLOT.into(), pgoutput::format_lsn(target).into()],
        )?;
        Ok(true)
    })
}

thread_local! {
    /// The worker's live state per model id (the worker is one thread).
    static LIVE: RefCell<HashMap<i64, Live>> = RefCell::new(HashMap::new());
    /// Failing models: when to retry, and the current delay.
    static RETRY: RefCell<HashMap<i64, (Instant, Duration)>> = RefCell::new(HashMap::new());
    /// Each model's partitions as the last poll saw them ([`Listed`]).
    static PARTITIONS: RefCell<HashMap<i64, Option<String>>> = RefCell::new(HashMap::new());
    /// The flush position the last complete decode read to ([`peek`]).
    static PEEKED_TO: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// The shared slot's last failure, logged once ([`slot_error`]).
    static SLOT_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    /// A standby snapshot was logged that the slot has not advanced past.
    static SNAPSHOT_PENDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// A model kept loaded between polls, with its live engine.
struct Live {
    /// The model's name and `model_version` when this was built: a change
    /// to either rebuilds.
    name: String,
    version: i64,
    /// The model with its current input cells (the mapper keeps it current).
    model: Model,
    mapper: live::Mapper,
    engine: Engine,
    /// The engine's output as last written to the result tables.
    snapshot: Snapshot,
    /// Per derived measure: its result table and dimension columns.
    tables: Vec<(MeasureId, String, Vec<String>)>,
    /// Transactions that ended at or before this WAL position may already
    /// be in the load this was built from.
    replay_until: u64,
}

/// One model's share of a poll (`txns`, of relations `rels`), inside the
/// caller's worker transaction: skip what its recorded progress already
/// holds, (re)build its live state when it has none or the model changed,
/// apply the rest, and record its progress with the result writes.
fn tick(
    mid: i64,
    name: &str,
    mut txns: Vec<pgoutput::Txn>,
    rels: &HashMap<u32, pgoutput::Relation>,
) -> Res<()> {
    // Taken out for the poll and put back only on success: any error (or a
    // Postgres ERROR unwinding through here) drops it, and the next poll
    // rebuilds from the tables.
    let mut state = LIVE.with(|l| l.borrow_mut().remove(&mid));
    let (caught_up, progress) = Spi::connect(|c| -> Res<_> {
        let t = c.select(
            "SELECT confirmed_lsn IS NOT NULL OR last_applied_at IS NOT NULL, confirmed_lsn::text \
             FROM improv.sync_state WHERE model_id = $1",
            None,
            &[mid.into()],
        )?;
        let r = t.first();
        Ok((
            r.get::<bool>(1)? == Some(true),
            r.get::<String>(2)?.as_deref().and_then(pgoutput::parse_lsn),
        ))
    })?;
    // Applied (with the result writes) before a crash or a failed advance.
    if let Some(p) = progress {
        txns.retain(|t| t.end_lsn > p);
    }
    if !caught_up {
        // A new or reset model: everything before the slot is caught up by
        // one full recompute.
        ensure_replica_identity(mid)?;
        drop(state.take());
    }
    // A structural change (`define`, a new binding) bumps `model_version`
    // but may touch no bound row, so no decoded change would ever prompt a
    // rebuild: build the live state when there is none (after a worker
    // restart) or the version moved, so results always follow the model as
    // it is now. The load runs after the decode, so it already holds every
    // transaction in `txns`; re-applying them converges (see `live`).
    let version = model_version(mid)?;
    if state
        .as_ref()
        .is_none_or(|l| l.version != version || l.name != name)
    {
        if state.is_some() {
            log!("pg_improv sync: model {name}: rebuilding (model changed)");
        }
        let _ = lock_model(name)?;
        drop(state.take());
        let (l, n) = rebuild(mid, name)?;
        record(mid, n, None, Applied::Rebuild)?;
        state = l;
    }
    let Some(last) = txns.last().map(|t| t.end_lsn) else {
        keep(mid, state);
        return Ok(());
    };
    let mut how = Applied::Nothing;
    let changed = if txns.iter().any(|t| !t.changes.is_empty()) {
        let _ = lock_model(name)?;
        // Again under the lock: a define that committed while this waited
        // for it must not see its results overwritten by the old formula.
        let version = model_version(mid)?;
        let applied = match state.as_mut() {
            Some(l) if l.version == version && l.name == name => apply(mid, l, rels, &txns)?,
            Some(_) => Err(Rebuild("model changed")),
            None => Err(Rebuild("no live state")),
        };
        match applied {
            Ok(n) => {
                how = Applied::Incremental;
                n
            }
            Err(why) => {
                how = Applied::Rebuild;
                log!("pg_improv sync: model {name}: rebuilding ({})", why.0);
                // Free the old engine before building one. The load runs
                // after the decode, so it already holds this batch.
                drop(state.take());
                let (l, n) = rebuild(mid, name)?;
                state = l;
                n
            }
        }
    } else {
        0
    };
    record(mid, changed, Some(last), how)?;
    keep(mid, state);
    Ok(())
}

fn keep(mid: i64, state: Option<Live>) {
    if let Some(l) = state {
        LIVE.with(|m| m.borrow_mut().insert(mid, l));
    }
}

fn model_version(mid: i64) -> Res<i64> {
    Ok(Spi::get_one_with_args::<i64>(
        "SELECT tx FROM improv.model_version WHERE model_id = $1",
        &[mid.into()],
    )?
    .unwrap_or(0))
}

/// Apply a batch to live state `l` incrementally and write the changed
/// result rows; `Ok(Err(_))` when the batch needs a rebuild instead (then
/// `l` is stale: drop it). Dimension values with no item yet are minted
/// first, in this (the worker's) transaction; the engine keeps going when
/// its plans do not read the item lists that grew ([`Engine::serves`]).
fn apply(
    mid: i64,
    l: &mut Live,
    rels: &HashMap<u32, pgoutput::Relation>,
    txns: &[pgoutput::Txn],
) -> Res<Result<i64, Rebuild>> {
    let new = l.mapper.unknown(rels, txns);
    if !new.is_empty() {
        for (cat, name, id) in mint(mid, new)? {
            l.mapper.add_item(cat, &name, id);
            l.model.add_item(id, cat, name);
        }
        if !l.engine.serves(&l.model)? {
            return Ok(Err(Rebuild("new item in a category a formula enumerates")));
        }
    }
    let edits = match l.mapper.map(&mut l.model, rels, txns, l.replay_until) {
        Ok(e) => e,
        Err(why) => return Ok(Err(why)),
    };
    pm::check_cell_budget(l.model.inputs.len(), MAX_CELLS.get().max(0) as usize)?;
    if edits.is_empty() {
        return Ok(Ok(0));
    }
    check_for_interrupts!();
    let snapshot = l.engine.apply(edits)?;
    let mut changed = 0;
    for (target, table, dims) in &l.tables {
        let (old, new) = (l.snapshot.get(target), snapshot.get(target));
        // Copy-on-write: a measure the edits did not reach is the same Arc.
        let same = match (old, new) {
            (Some(o), Some(n)) => Arc::ptr_eq(o, n),
            (o, n) => o.is_none() && n.is_none(),
        };
        if same {
            continue;
        }
        let empty = MeasureValues::new();
        let (old, new) = (old.map_or(&empty, |m| m), new.map_or(&empty, |m| m));
        // ponytail: O(cells of each changed measure) per batch (and the engine
        // copies a changed measure whole while `l.snapshot` holds it); an
        // engine API returning the output delta lifts both.
        let mut was = MeasureValues::new();
        let mut now = MeasureValues::new();
        for (k, v) in new {
            if old.get(k) != Some(v) {
                now.insert(k.clone(), v.clone());
                if let Some(o) = old.get(k) {
                    was.insert(k.clone(), o.clone());
                }
            }
        }
        for (k, v) in old {
            if !new.contains_key(k) {
                was.insert(k.clone(), v.clone());
            }
        }
        changed += write_results(
            table,
            dims,
            l.model.measures[target].value_type,
            &pm::result_rows(&l.model, *target, &was)?,
            &pm::result_rows(&l.model, *target, &now)?,
        )?;
    }
    l.snapshot = snapshot;
    Ok(Ok(changed))
}

/// Load model `mid` from its tables, compute every derived measure, and
/// write the result rows that differ from the tables. Returns the live state
/// (none for a model the live engine cannot serve) and the rows changed.
fn rebuild(mid: i64, name: &str) -> Res<(Option<Live>, i64)> {
    let mut model = super::trigger::load(mid, name.to_string(), true)?.model;
    // Everything the load saw committed before this position.
    let replay_until = Spi::get_one::<String>("SELECT pg_current_wal_insert_lsn()::text")?
        .as_deref()
        .and_then(pgoutput::parse_lsn)
        .ok_or("cannot read the WAL insert position")?;
    let mut mapper = live::Mapper::new(&model, bindings(mid)?);
    seed_folds(&mut model, &mut mapper)?;
    let version = model_version(mid)?;
    let targets = super::trigger::all_derived(&model);
    let (engine, snapshot) = start_engine(&model, &targets)?;
    let mut tables = Vec::with_capacity(targets.len());
    let mut changed = 0;
    let empty = MeasureValues::new();
    for &target in &targets {
        check_for_interrupts!();
        let measure = &model.measures[&target].name.0;
        pm::validate_name("measure", measure)?;
        let table = pm::result_table(name, measure);
        let dims = pm::dim_columns(&model, target)?;
        let vt = model.measures[&target].value_type;
        ensure_table(name, &table, &dims, vt)?;
        let new = pm::result_rows(&model, target, snapshot.get(&target).map_or(&empty, |m| m))?;
        let old = read_results(&table, &dims, vt)?;
        changed += write_results(&table, &dims, vt, &old, &new)?;
        tables.push((target, table, dims));
    }
    let l = Live {
        name: name.to_string(),
        version,
        model,
        mapper,
        engine,
        snapshot,
        tables,
        replay_until,
    };
    Ok((Some(l), changed))
}

/// `Engine::new` with every Postgres-handled signal blocked, so the compute
/// thread it spawns inherits a mask that never runs a Postgres signal
/// handler off the worker's main thread.
fn start_engine(model: &Model, targets: &[MeasureId]) -> Res<(Engine, Snapshot)> {
    // SAFETY: sets this thread's signal mask to Postgres' BlockSig; restored
    // to the worker's usual mask (UnBlockSig) below, panic or not.
    unsafe { pg_sys::BackgroundWorkerBlockSignals() };
    let r = catch_unwind(AssertUnwindSafe(|| Engine::new(model, targets)));
    // SAFETY: as above.
    unsafe { pg_sys::BackgroundWorkerUnblockSignals() };
    match r {
        Ok(r) => Ok(r?),
        Err(_) => Err("improv engine panicked while starting".into()),
    }
}

/// Load each aggregated binding's fold state ([`live::seed_sql`]) into
/// `mapper`, and its cells into `model`. Each query reads its own snapshot,
/// which the mapper keeps to skip the transactions already in it.
// ponytail: a second scan of each aggregated relation per rebuild (the
// loader's GROUP BY, then this one); feed the loader from these rows if
// rebuilds of huge fact tables ever dominate.
fn seed_folds(model: &mut Model, mapper: &mut live::Mapper) -> Res<()> {
    for (rel, b) in mapper.aggregated() {
        check_for_interrupts!();
        let (Some(agg), dims) = (
            b.agg,
            b.dims.iter().map(|(_, c)| c.clone()).collect::<Vec<_>>(),
        ) else {
            continue;
        };
        let (name, oid) = Spi::connect(|c| -> Res<(String, u32)> {
            let t = c.select(
                "SELECT $1::oid::regclass::text, \
                        (SELECT atttypid FROM pg_attribute WHERE attrelid = $1::oid \
                           AND attname = $2 AND NOT attisdropped)::int8",
                None,
                &[i64::from(rel).into(), b.value_col.clone().into()],
            )?;
            let r = t.first();
            let name = r
                .get::<String>(1)?
                .ok_or("an aggregated relation is gone")?;
            let oid = r
                .get::<i64>(2)?
                .ok_or("an aggregated value column is gone")?;
            Ok((name, id32(oid)?))
        })?;
        let sql = live::seed_sql(&name, &dims, &b.value_col, agg);
        let n = dims.len();
        let (rows, snap) = Spi::connect(|c| -> Res<_> {
            let mut rows = Vec::new();
            let mut snap = None;
            for r in c.select(&sql, None, &[])? {
                if let Some(s) = r.get::<String>(n + 4)? {
                    snap = live::Snap::parse(&s);
                    continue;
                }
                let d = (1..=n)
                    .map(|i| r.get::<String>(i))
                    .collect::<Result<Vec<_>, _>>()?;
                rows.push((d, r.get::<String>(n + 1)?, req(&r, n + 2)?, req(&r, n + 3)?));
            }
            Ok((rows, snap.ok_or("the fold seed read no snapshot")?))
        })?;
        mapper
            .seed(model, b.measure, oid, &rows, snap)
            .map_err(|e| format!("seeding the fold of measure {}: {}", b.measure.0, e.0))?;
    }
    Ok(())
}

/// Model `mid`'s bindings, per relation oid.
fn bindings(mid: i64) -> Res<HashMap<u32, Vec<live::Binding>>> {
    type Row = (i64, i64, String, Vec<i64>, Vec<String>, Option<String>);
    let rows: Vec<Row> = Spi::connect(|c| -> Res<_> {
        c.select(
            // `agg` via to_jsonb: the column is newer than this query.
            "SELECT b.measure_id, b.rel::oid::int8, b.value_col, \
                    ARRAY(SELECT c.id FROM unnest(m.categories) WITH ORDINALITY u(cid, ord) \
                          JOIN improv.category c ON c.model_id = b.model_id AND c.id = u.cid \
                          ORDER BY u.ord), \
                    ARRAY(SELECT b.dim_cols ->> c.name \
                          FROM unnest(m.categories) WITH ORDINALITY u(cid, ord) \
                          JOIN improv.category c ON c.model_id = b.model_id AND c.id = u.cid \
                          ORDER BY u.ord), \
                    to_jsonb(b) ->> 'agg' \
             FROM improv.binding b \
             JOIN improv.measure m ON m.model_id = b.model_id AND m.id = b.measure_id \
             WHERE b.model_id = $1",
            None,
            &[mid.into()],
        )?
        .map(|r| {
            Ok((
                req(&r, 1)?,
                req(&r, 2)?,
                req(&r, 3)?,
                req(&r, 4)?,
                req(&r, 5)?,
                r.get(6)?,
            ))
        })
        .collect()
    })?;
    let mut out: HashMap<u32, Vec<live::Binding>> = HashMap::new();
    for (measure, rel, value_col, cats, cols, agg) in rows {
        let agg = match agg.as_deref() {
            None => None,
            Some(a) => Some(live::Agg::parse(a).ok_or_else(|| format!("unknown agg {a:?}"))?),
        };
        let dims = cats
            .into_iter()
            .map(|c| id32(c).map(improv_core_model::CategoryId))
            .zip(cols)
            .map(|(c, col)| c.map(|c| (c, col)))
            .collect::<Res<Vec<_>>>()?;
        out.entry(u32::try_from(rel)?)
            .or_default()
            .push(live::Binding {
                measure: MeasureId(id32(measure)?),
                value_col,
                dims,
                agg,
            });
    }
    Ok(out)
}

/// How a poll brought the results up to date, for `improv.status`'s counters.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Applied {
    Nothing,
    Incremental,
    Rebuild,
}

fn record(mid: i64, changed: i64, lsn: Option<u64>, how: Applied) -> Res<()> {
    Spi::run_with_args(
        "UPDATE improv.sync_state SET last_applied_at = now(), last_error = NULL, \
           rows_applied = rows_applied + $2, \
           confirmed_lsn = COALESCE($3::pg_lsn, confirmed_lsn), \
           incremental_applies = incremental_applies + $4, rebuilds = rebuilds + $5 \
         WHERE model_id = $1",
        &[
            mid.into(),
            changed.into(),
            lsn.map(pgoutput::format_lsn).into(),
            i64::from(how == Applied::Incremental).into(),
            i64::from(how == Applied::Rebuild).into(),
        ],
    )?;
    Ok(())
}

/// Drop slot `slot` if it exists.
fn drop_slot(slot: &str) -> Res<()> {
    let exists = Spi::get_one_with_args::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_replication_slots WHERE slot_name = $1)",
        &[slot.into()],
    )? == Some(true);
    if exists {
        Spi::run_with_args("SELECT pg_drop_replication_slot($1)", &[slot.into()])?;
    }
    Ok(())
}

/// Drop the slots queued in `improv.slot_to_drop` (0.10.0's per-model
/// slots, after an upgrade). Each drop needs a transaction that has not
/// written, so a failure (e.g. the slot is active) leaves it queued.
fn drop_queued_slots() -> Res<()> {
    let queued: Vec<String> = Spi::connect(|c| -> Res<_> {
        c.select("SELECT slot FROM improv.slot_to_drop", None, &[])?
            .map(|r| req(&r, 1))
            .collect()
    })?;
    for slot in queued {
        drop_slot(&slot)?;
        Spi::run_with_args(
            "DELETE FROM improv.slot_to_drop WHERE slot = $1",
            &[slot.into()],
        )?;
    }
    Ok(())
}
