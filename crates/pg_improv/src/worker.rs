//! L2: logical-decoding sync, the default `sync_mode`.
//!
//! A background worker (`pg_improv sync`, registered when the library is in
//! `shared_preload_libraries`) polls, per model in `logical` mode, a logical
//! replication slot `improv_<model>` (plugin `pgoutput`, publication
//! `improv_<model>` over the model's bound relations). When committed source
//! transactions touched a bound relation it applies their row changes to
//! the model's derived results and writes them **in one worker transaction
//! that also advances the slot**, so a crash re-reads and re-applies (the
//! result writes are idempotent upserts and deletes) and never loses a
//! change.
//!
//! Slots cannot be created in a transaction that has already written, so
//! `improv.enable_logical` (called by `bind_measure` and
//! `set_sync_mode(…, 'logical')`) only creates the publication and records
//! the intent in `improv.sync_state`; the worker creates the slot at the
//! start of its own fresh transaction.
//!
//! Apply strategy (G1): incremental. The worker keeps, per model, a [`Live`]
//! state: the loaded model, a live [`Engine`] (its own compute thread, which
//! never calls Postgres) and its last [`Snapshot`]. Each batch's row changes
//! become input-cell edits ([`live::Mapper`]); the engine recomputes only
//! what they affect, and only the measures whose snapshot changed are diffed
//! and written. The worker rebuilds (a full load + compute + diff against
//! the result tables) when there is no live state, the model's
//! `model_version` moved (a structural change), or the batch cannot be
//! applied cell by cell (TRUNCATE, a new dimension value, …). Any error
//! drops the live state, so the next poll rebuilds from the tables.

use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::Arc;
use std::time::Duration;

use improv_core_model::{MeasureId, Model, Value};
use improv_engine::session::{Engine, MeasureValues, Snapshot};
use improv_pg_model as pm;
use improv_pg_model::live::{self, Rebuild};
use improv_pg_model::pgoutput;
use pgrx::bgworkers::*;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use pgrx::{GucContext, GucFlags, GucRegistry, GucSetting};

use super::{bump_version, id32, lock_model, read_results, req, Res, MAX_CELLS};

/// `improv.database`: the database the sync worker connects to.
pub(crate) static DATABASE: GucSetting<Option<std::ffi::CString>> =
    GucSetting::<Option<std::ffi::CString>>::new(Some(c"postgres"));
/// `improv.poll_ms`: how often the worker polls each slot.
pub(crate) static POLL_MS: GucSetting<i32> = GucSetting::<i32>::new(200);
/// `improv.max_changes`: the most decoded changes one poll reads per model.
pub(crate) static MAX_CHANGES: GucSetting<i32> = GucSetting::<i32>::new(10_000);

/// Register the L2 GUCs (always) and the worker (only under preload).
pub(crate) fn init() {
    GucRegistry::define_string_guc(
        c"improv.database",
        c"Database the pg_improv sync worker connects to.",
        c"The logical-decoding worker serves the Improv models of this one database.",
        &DATABASE,
        GucContext::Postmaster,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"improv.poll_ms",
        c"How often the pg_improv sync worker polls each slot, in ms.",
        c"Lower is fresher results; higher is fewer idle wakeups.",
        &POLL_MS,
        10,
        60_000,
        GucContext::Sighup,
        GucFlags::default(),
    );
    GucRegistry::define_int_guc(
        c"improv.max_changes",
        c"Most decoded changes one poll reads per model.",
        c"Bounds the work (and memory) of one worker transaction.",
        &MAX_CHANGES,
        1,
        i32::MAX,
        GucContext::Sighup,
        GucFlags::default(),
    );
    // SAFETY: a plain read of a postmaster global during _PG_init.
    if unsafe { pg_sys::process_shared_preload_libraries_in_progress } {
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
    last_error      text
);
-- Slots whose model left logical mode (or was dropped): the worker drops them.
CREATE TABLE improv.slot_to_drop (slot text PRIMARY KEY);
"#,
    name = "l2_logical",
    requires = ["catalog"]
);

/// Ensure model `model` can be synced by logical decoding: give PK-less
/// bound relations `REPLICA IDENTITY FULL`, create (or reset) the
/// publication `improv_<model>` over the bound relations, and record the
/// slot intent in `improv.sync_state`. Idempotent. The worker creates the
/// slot.
#[pg_extern(security_invoker)]
fn enable_logical(model: &str) -> Res<()> {
    let (mid, name) = lock_model(model)?;
    enable(mid, &name)
}

pub(crate) fn enable(mid: i64, name: &str) -> Res<()> {
    let slot = pm::slot_name(name)?;
    let rels = bound_rels(mid)?;
    // Before publishing: Postgres rejects UPDATE/DELETE on a published table
    // that has no replica identity.
    ensure_replica_identity(mid)?;
    let exists = Spi::get_one_with_args::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_publication WHERE pubname = $1)",
        &[slot.clone().into()],
    )? == Some(true);
    let q = pm::quote_ident(&slot);
    match (exists, rels.is_empty()) {
        (false, true) => Spi::run(&format!("CREATE PUBLICATION {q}"))?,
        (false, false) => Spi::run(&format!(
            "CREATE PUBLICATION {q} FOR TABLE {}",
            rels.join(", ")
        ))?,
        (true, true) => {}
        (true, false) => Spi::run(&format!(
            "ALTER PUBLICATION {q} SET TABLE {}",
            rels.join(", ")
        ))?,
    }
    Spi::run_with_args(
        "INSERT INTO improv.sync_state (model_id, slot, publication) VALUES ($1, $2, $2) \
         ON CONFLICT (model_id) DO NOTHING",
        &[mid.into(), slot.clone().into()],
    )?;
    Spi::run_with_args(
        "DELETE FROM improv.slot_to_drop WHERE slot = $1",
        &[slot.into()],
    )?;
    Ok(())
}

/// Undo [`enable`]: drop the publication, forget the sync state, and queue
/// the slot for the worker to drop.
pub(crate) fn disable(mid: i64, name: &str) -> Res<()> {
    let slot = pm::slot_name(name)?;
    Spi::run(&format!(
        "DROP PUBLICATION IF EXISTS {}",
        pm::quote_ident(&slot)
    ))?;
    Spi::run_with_args(
        "DELETE FROM improv.sync_state WHERE model_id = $1",
        &[mid.into()],
    )?;
    Spi::run_with_args(
        "INSERT INTO improv.slot_to_drop (slot) VALUES ($1) ON CONFLICT DO NOTHING",
        &[slot.into()],
    )?;
    bump_version(mid)
}

/// Give every bound relation without a primary key `REPLICA IDENTITY FULL`,
/// so its updated and deleted rows carry the old dimension values. The
/// caller must not hold the table open (see `bind_measure`).
fn ensure_replica_identity(mid: i64) -> Res<()> {
    for rel in bound_rels(mid)? {
        let needs_full = Spi::get_one_with_args::<bool>(
            "SELECT c.relreplident <> 'f' AND NOT EXISTS ( \
               SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary) \
             FROM pg_class c WHERE c.oid = $1::regclass",
            &[rel.clone().into()],
        )? == Some(true);
        if needs_full {
            Spi::run(&format!("ALTER TABLE {rel} REPLICA IDENTITY FULL"))?;
        }
    }
    Ok(())
}

fn bound_rels(mid: i64) -> Res<Vec<String>> {
    Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT DISTINCT rel::text FROM improv.binding WHERE model_id = $1 ORDER BY 1",
            None,
            &[mid.into()],
        )?
        .map(|r| req(&r, 1))
        .collect()
    })
}

/// One row of `improv.status` (see [`status`] for the column names).
type StatusRow = (
    String,
    Option<String>,
    Option<i64>,
    Option<String>,
    Option<pgrx::datum::TimestampWithTimeZone>,
    Option<i64>,
    Option<String>,
);

/// How model `model`'s results are kept current, and (in logical mode) how
/// far the worker is behind: `lag_bytes` is the WAL between the current
/// insert position and the slot's confirmed position.
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
                    (pg_current_wal_lsn() - r.confirmed_flush_lsn)::int8, \
                    r.confirmed_flush_lsn::text, s.last_applied_at, s.rows_applied, s.last_error \
             FROM improv.model m \
             LEFT JOIN improv.sync_state s ON s.model_id = m.id \
             LEFT JOIN pg_replication_slots r ON r.slot_name = s.slot \
             WHERE m.name = $1",
            None,
            &[model.into()],
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
        ))
    })?;
    Ok(row)
}

/// The sync worker's entry point.
#[pg_guard]
#[unsafe(no_mangle)]
pub extern "C-unwind" fn improv_worker_main(_arg: pg_sys::Datum) {
    BackgroundWorker::attach_signal_handlers(SignalWakeFlags::SIGHUP | SignalWakeFlags::SIGTERM);
    let db = DATABASE
        .get()
        .and_then(|c| c.into_string().ok())
        .unwrap_or_else(|| "postgres".into());
    BackgroundWorker::connect_worker_to_spi(Some(&db), None);
    log!("pg_improv sync worker started on database {db:?}");
    loop {
        let poll = Duration::from_millis(POLL_MS.get().max(10) as u64);
        if !BackgroundWorker::wait_latch(Some(poll)) {
            break; // SIGTERM
        }
        if BackgroundWorker::sighup_received() {
            // GUCs are reloaded by the latch wait; nothing else to refresh.
        }
        // Nothing to do until the extension is installed in this database.
        // (A Postgres ERROR is not a Rust `Err`: querying a missing
        // `improv.*` table would unwind out of the worker and kill it.)
        if !BackgroundWorker::transaction(installed) {
            continue;
        }
        // Each model in its own transaction, so one model's error neither
        // blocks the others nor rolls back their progress.
        let models = BackgroundWorker::transaction(|| {
            PgTryBuilder::new(|| list_models().unwrap_or_default())
                .catch_others(|_| Vec::new())
                .execute()
        });
        LIVE.with(|l| {
            l.borrow_mut()
                .retain(|mid, _| models.iter().any(|(m, _)| m == mid))
        });
        for (mid, name) in models {
            // Slot creation needs a transaction that has not written (and a
            // failed attempt leaves a half-made slot with no plugin), so it
            // gets its own transaction, before the model's poll.
            let created = BackgroundWorker::transaction(|| {
                PgTryBuilder::new(|| ensure_slot(&name).map_err(|e| e.to_string()))
                    .catch_others(|e| Err(format!("{e:?}")))
                    .execute()
            });
            if let Err(e) = created {
                log!("pg_improv sync: model {name}: creating its slot: {e}");
                continue;
            }
            let r = BackgroundWorker::transaction(|| {
                PgTryBuilder::new(|| tick(mid, &name).map_err(|e| e.to_string()))
                    .catch_others(|e| Err(format!("{e:?}")))
                    .execute()
            });
            if let Err(e) = r {
                BackgroundWorker::transaction(|| {
                    let _ = Spi::run_with_args(
                        "UPDATE improv.sync_state SET last_error = $2 WHERE model_id = $1",
                        &[mid.into(), e.clone().into()],
                    );
                });
                log!("pg_improv sync: model {name}: {e}");
            }
        }
        BackgroundWorker::transaction(|| {
            PgTryBuilder::new(|| {
                if let Err(e) = drop_queued_slots() {
                    log!("pg_improv sync: dropping queued slots: {e}");
                }
            })
            .catch_others(|e| log!("pg_improv sync: dropping queued slots: {e:?}"))
            .execute()
        });
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

/// Whether pg_improv's L2 catalog exists in the worker's database.
fn installed() -> bool {
    Spi::get_one::<bool>(
        "SELECT to_regclass('improv.sync_state') IS NOT NULL \
           AND to_regclass('improv.slot_to_drop') IS NOT NULL",
    )
    .ok()
    .flatten()
        == Some(true)
}

/// Models in logical mode that have sync state (i.e. a publication).
fn list_models() -> Res<Vec<(i64, String)>> {
    Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT m.id, m.name FROM improv.model m JOIN improv.sync_state s ON s.model_id = m.id \
             WHERE m.sync_mode = 'logical' ORDER BY m.id",
            None,
            &[],
        )?
        .map(|r| Ok((req(&r, 1)?, req(&r, 2)?)))
        .collect()
    })
}

/// Make sure model `name`'s slot exists with the `pgoutput` plugin. Runs in
/// its own worker transaction, before anything in it writes (Postgres
/// refuses to create a slot otherwise). A slot left without a plugin by an
/// earlier failed creation is dropped and recreated. Returns whether it
/// created the slot (the caller then catches up with a full recompute).
fn ensure_slot(name: &str) -> Res<bool> {
    // Read-only SPI only (`Spi::connect` + `select`): pgrx's `get_one*` and
    // `run*` helpers go through `connect_mut`, which assigns a transaction
    // id, and Postgres refuses to create a slot in a transaction that has
    // one ("…that has performed writes").
    let slot = pm::slot_name(name)?;
    let plugin: Option<String> = Spi::connect(|c| -> Res<_> {
        let t = c.select(
            "SELECT plugin::text FROM pg_replication_slots WHERE slot_name = $1",
            None,
            &[slot.clone().into()],
        )?;
        Ok(if t.is_empty() {
            None
        } else {
            t.first().get::<String>(1)?
        })
    })?;
    match plugin.as_deref() {
        Some("pgoutput") => return Ok(false),
        Some(_) => {
            Spi::connect(|c| -> Res<()> {
                c.select(
                    "SELECT pg_drop_replication_slot($1)",
                    None,
                    &[slot.clone().into()],
                )?;
                Ok(())
            })?;
        }
        None => {}
    }
    Spi::connect(|c| -> Res<()> {
        c.select(
            "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
            None,
            &[slot.into()],
        )?;
        Ok(())
    })?;
    Ok(true)
}

thread_local! {
    /// The worker's live state per model id (the worker is one thread).
    static LIVE: RefCell<HashMap<i64, Live>> = RefCell::new(HashMap::new());
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

/// One poll of one model, inside the caller's (worker) transaction.
fn tick(mid: i64, name: &str) -> Res<()> {
    // Taken out for the poll and put back only on success: any error (or a
    // Postgres ERROR unwinding through here) drops it, and the next poll
    // rebuilds from the tables.
    let mut state = LIVE.with(|l| l.borrow_mut().remove(&mid));
    let slot = pm::slot_name(name)?;
    // A brand-new slot starts at the current WAL position: everything
    // committed before it is caught up by one full recompute.
    let caught_up = Spi::get_one_with_args::<bool>(
        "SELECT confirmed_lsn IS NOT NULL OR last_applied_at IS NOT NULL \
         FROM improv.sync_state WHERE model_id = $1",
        &[mid.into()],
    )? == Some(true);
    if !caught_up {
        ensure_replica_identity(mid)?;
        let _ = lock_model(name)?;
        drop(state);
        let (state, changed) = rebuild(mid, name)?;
        record(mid, changed, None)?;
        keep(mid, state);
        return Ok(());
    }
    let max = MAX_CHANGES.get().max(1);
    let rows: Vec<(String, Vec<u8>)> = Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT lsn::text, data FROM pg_logical_slot_peek_binary_changes( \
               $1, NULL, $2, 'proto_version', '1', 'publication_names', $1)",
            None,
            &[slot.clone().into(), max.into()],
        )?
        .map(|r| Ok((req(&r, 1)?, req(&r, 2)?)))
        .collect()
    })?;
    let mut stream = pgoutput::Stream::default();
    for (_, data) in &rows {
        stream.feed(data)?;
    }
    // Only whole committed transactions are applied; a transaction cut off
    // by `max_changes` is re-read next poll.
    let Some(last) = stream.done.last() else {
        keep(mid, state);
        return Ok(());
    };
    let advance_to = last.end_lsn;
    let touched = stream.done.iter().any(|t| !t.changes.is_empty());
    let changed = if touched {
        let _ = lock_model(name)?;
        let version = model_version(mid)?;
        let applied = match state.as_mut() {
            Some(l) if l.version == version && l.name == name => apply(l, &stream)?,
            _ => Err(Rebuild("no live state")),
        };
        match applied {
            Ok(n) => n,
            Err(why) => {
                log!("pg_improv sync: model {name}: rebuilding ({})", why.0);
                drop(state.take()); // free the old engine before building one
                                    // The load runs after the peek: it already holds the batch.
                let (l, n) = rebuild(mid, name)?;
                state = l;
                n
            }
        }
    } else {
        0
    };
    Spi::run_with_args(
        "SELECT pg_replication_slot_advance($1, $2::pg_lsn)",
        &[slot.into(), pgoutput::format_lsn(advance_to).into()],
    )?;
    record(mid, changed, Some(advance_to))?;
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
/// `l` is stale: drop it).
fn apply(l: &mut Live, stream: &pgoutput::Stream) -> Res<Result<i64, Rebuild>> {
    let edits = match l.mapper.map(
        &mut l.model,
        &stream.relations,
        &stream.done,
        l.replay_until,
    ) {
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
        changed += write(
            table,
            dims,
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
    let model = super::trigger::load(mid, name.to_string(), true)?.model;
    // Everything the load saw committed before this position.
    let replay_until = Spi::get_one::<String>("SELECT pg_current_wal_insert_lsn()::text")?
        .as_deref()
        .and_then(pgoutput::parse_lsn)
        .ok_or("cannot read the WAL insert position")?;
    if model
        .inputs
        .values()
        .any(|v| !matches!(v, Value::Number(_)))
    {
        // ponytail: the live engine takes numeric inputs only; such models
        // recompute in full per batch (reloading once more) until it types
        // its input lanes.
        return Ok((None, super::trigger::recompute_all(mid, name)?));
    }
    let mapper = live::Mapper::new(&model, bindings(mid)?);
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
        // Checked first: `IF NOT EXISTS` would NOTICE on every statement.
        if !super::trigger::exists(&table)? {
            Spi::run(&format!(
                "CREATE SCHEMA IF NOT EXISTS {}",
                pm::quote_ident(&pm::result_schema(name))
            ))?;
            Spi::run(&pm::create_table_sql(&table, &dims))?;
        }
        let new = pm::result_rows(&model, target, snapshot.get(&target).map_or(&empty, |m| m))?;
        changed += write(&table, &dims, &read_results(&table, &dims)?, &new)?;
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

/// Run the statements that turn result rows `old` into `new`; returns the
/// rows changed.
fn write(table: &str, dims: &[String], old: &pm::ResultRows, new: &pm::ResultRows) -> Res<i64> {
    let mut changed = 0;
    for st in pm::diff_statements(table, dims, old, new) {
        let mut args: Vec<DatumWithOid> = st.dims.iter().map(|d| d.clone().into()).collect();
        if let Some(v) = &st.values {
            args.push(v.clone().into());
        }
        Spi::run_with_args(&st.sql, &args)?;
        changed += st.rows() as i64;
    }
    Ok(changed)
}

/// Model `mid`'s bindings, per relation oid.
fn bindings(mid: i64) -> Res<HashMap<u32, Vec<live::Binding>>> {
    type Row = (i64, i64, String, Vec<i64>, Vec<String>, bool);
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
                    to_jsonb(b) ->> 'agg' IS NOT NULL \
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
                req(&r, 6)?,
            ))
        })
        .collect()
    })?;
    let mut out: HashMap<u32, Vec<live::Binding>> = HashMap::new();
    for (measure, rel, value_col, cats, cols, aggregated) in rows {
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
                aggregated,
            });
    }
    Ok(out)
}

fn record(mid: i64, changed: i64, lsn: Option<u64>) -> Res<()> {
    Spi::run_with_args(
        "UPDATE improv.sync_state SET last_applied_at = now(), last_error = NULL, \
           rows_applied = rows_applied + $2, \
           confirmed_lsn = COALESCE($3::pg_lsn, confirmed_lsn) \
         WHERE model_id = $1",
        &[
            mid.into(),
            changed.into(),
            lsn.map(pgoutput::format_lsn).into(),
        ],
    )?;
    Ok(())
}

/// Drop slots queued by [`disable`]. Each drop needs a transaction that has
/// not written, so a failure (e.g. the slot is active) leaves it queued.
fn drop_queued_slots() -> Res<()> {
    let queued: Vec<String> = Spi::connect(|c| -> Res<_> {
        c.select("SELECT slot FROM improv.slot_to_drop", None, &[])?
            .map(|r| req(&r, 1))
            .collect()
    })?;
    for slot in queued {
        let exists = Spi::get_one_with_args::<bool>(
            "SELECT EXISTS (SELECT 1 FROM pg_replication_slots WHERE slot_name = $1)",
            &[slot.clone().into()],
        )? == Some(true);
        if exists {
            Spi::run_with_args(
                "SELECT pg_drop_replication_slot($1)",
                &[slot.clone().into()],
            )?;
        }
        Spi::run_with_args(
            "DELETE FROM improv.slot_to_drop WHERE slot = $1",
            &[slot.into()],
        )?;
    }
    Ok(())
}
