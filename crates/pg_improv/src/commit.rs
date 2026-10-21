//! Commit-mode sync (`sync_mode = 'commit'`): one recompute per writing
//! transaction, just before it commits, with nothing installed on the
//! user's tables.
//!
//! Hooks note, per backend, which relations the current transaction wrote:
//! `ExecutorFinish_hook` (INSERT, UPDATE, DELETE, MERGE, writable CTEs),
//! `ProcessUtility_hook` (`TRUNCATE`, `COPY … FROM`; a `TRUNCATE … CASCADE`
//! counts as a write to every relation). A bound partitioned table counts
//! as written when one of its partitions is, and the reverse. A rolled-back
//! subtransaction forgets the relations it alone wrote. At `XACT_EVENT_PRE_COMMIT` (and `PRE_PREPARE`) every
//! commit-mode model bound to a written relation is locked, loaded and
//! recomputed inside the transaction, as the role current at the commit:
//! the results commit or roll back with the writes, and a failing recompute
//! aborts the commit. The hooks must be in every writing backend, so the
//! mode needs the library preloaded.
//!
//! A transaction that wrote no commit-bound relation pays a set lookup at
//! commit: each backend caches the commit-mode models' bound relations,
//! and a statement trigger on `improv.model` and `improv.binding` sends a
//! relcache invalidation that drops every backend's copy.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use improv_core_model::MeasureId;
use improv_pg_model as pm;
use pgrx::pg_sys::ffi::pg_guard_ffi_boundary;
use pgrx::pg_sys::panic::{CaughtError, ErrorReport};
use pgrx::prelude::*;
use pgrx::{PgList, PgLogLevel, PgSqlErrorCode};

use super::{id32, lock_model, req, Res};

thread_local! {
    /// Relations this transaction wrote, each with the subtransaction that
    /// first wrote it.
    static WRITTEN: RefCell<HashMap<pg_sys::Oid, pg_sys::SubTransactionId>> =
        RefCell::default();
    /// Pre-commit model recomputes run by this backend (read by the tests).
    static RECOMPUTES: Cell<u64> = const { Cell::new(0) };
    /// The commit-mode models' bound relations (and their partition
    /// ancestors and descendants), loaded at [`CACHE_GEN`] = the first.
    static BOUND: RefCell<Option<(u64, HashSet<pg_sys::Oid>)>> = RefCell::default();
    /// Bumped by an invalidation of a [`WATCHED`] relation.
    static CACHE_GEN: Cell<u64> = const { Cell::new(0) };
    /// `improv.model` and `improv.binding`; unset until the first load.
    static WATCHED: Cell<Option<[pg_sys::Oid; 2]>> = const { Cell::new(None) };
}

/// Whether the library was loaded at backend start (shared, session or
/// local preload), so every backend has the hooks.
static PRELOADED: AtomicBool = AtomicBool::new(false);
static mut PREV_FINISH: pg_sys::ExecutorFinish_hook_type = None;
static mut PREV_UTILITY: pg_sys::ProcessUtility_hook_type = None;

/// Install the hooks and the transaction callbacks (from `_PG_init`).
pub(crate) fn init() {
    // SAFETY: _PG_init runs once per process, on the backend thread, before
    // any query; the hooks chain to whatever was installed before them.
    unsafe {
        PRELOADED.store(
            pg_sys::process_shared_preload_libraries_in_progress
                || pg_sys::Mode == pg_sys::ProcessingMode::InitProcessing,
            Ordering::Relaxed,
        );
        PREV_FINISH = pg_sys::ExecutorFinish_hook;
        pg_sys::ExecutorFinish_hook = Some(executor_finish);
        PREV_UTILITY = pg_sys::ProcessUtility_hook;
        pg_sys::ProcessUtility_hook = Some(process_utility);
        pg_sys::RegisterXactCallback(Some(xact_callback), std::ptr::null_mut());
        pg_sys::RegisterSubXactCallback(Some(subxact_callback), std::ptr::null_mut());
        pg_sys::CacheRegisterRelcacheCallback(Some(relcache_callback), pg_sys::Datum::from(0));
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn relcache_callback(_arg: pg_sys::Datum, rel: pg_sys::Oid) {
    // Unset WATCHED (no load yet, or no extension): any invalidation, as
    // CREATE EXTENSION sends some, may mean there is something to load. A
    // bound relation's own invalidation may be a partition attached to it.
    let stale = rel == pg_sys::InvalidOid
        || WATCHED.get().is_none_or(|w| w.contains(&rel))
        || BOUND.with(|b| {
            b.try_borrow().map_or(true, |b| {
                b.as_ref().is_some_and(|(_, set)| set.contains(&rel))
            })
        });
    if stale {
        CACHE_GEN.set(CACHE_GEN.get() + 1);
    }
}

/// A statement trigger on `improv.model` and `improv.binding`: a change may
/// move relations in or out of commit-mode sync, so invalidate every
/// backend's cache of them (this one's now, the others' at commit).
#[pg_trigger]
fn commit_catalog_changed<'a>(
    t: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, PgHeapTupleError> {
    match t.relid() {
        // SAFETY: queues an invalidation, in the firing transaction.
        Ok(rel) => unsafe { pg_sys::CacheInvalidateRelcacheByRelid(rel) },
        Err(e) => error!("improv.commit_catalog_changed: {e}"),
    }
    Ok(None)
}

extension_sql!(
    r#"
CREATE TRIGGER commit_sync AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON improv.model
  FOR EACH STATEMENT EXECUTE FUNCTION improv.commit_catalog_changed();
CREATE TRIGGER commit_sync AFTER INSERT OR UPDATE OR DELETE OR TRUNCATE ON improv.binding
  FOR EACH STATEMENT EXECUTE FUNCTION improv.commit_catalog_changed();
"#,
    name = "commit_sync",
    requires = ["catalog", commit_catalog_changed]
);

/// For `set_sync_mode(…, 'commit')` and binding a relation to a commit-mode
/// model: as trigger mode's `CREATE TRIGGER`, require the TRIGGER privilege
/// on `rel` (a model that runs in other roles' commits must not be set up
/// by a role that could not attach a trigger there) and take its SHARE ROW
/// EXCLUSIVE lock, so writers in flight finish before the mode change and
/// later ones see it.
pub(crate) fn guard(rel: pg_sys::Oid, name: &str) -> Res<()> {
    let ok =
        Spi::get_one_with_args::<bool>("SELECT has_table_privilege($1, 'TRIGGER')", &[rel.into()])?;
    if ok != Some(true) {
        return Err(format!(
            "commit sync recomputes the model in the commits of every role that writes {name}, \
             so it needs the TRIGGER privilege on {name}, as trigger mode does"
        )
        .into());
    }
    // SAFETY: a lock on an existing relation, in a live transaction.
    unsafe { pg_sys::LockRelationOid(rel, pg_sys::ShareRowExclusiveLock as _) };
    Ok(())
}

/// Refuse commit mode unless every backend loads the hooks at start.
pub(crate) fn check_preloaded() -> Res<()> {
    if PRELOADED.load(Ordering::Relaxed) {
        return Ok(());
    }
    Err(
        "commit sync mode needs pg_improv in shared_preload_libraries (or \
         session_preload_libraries): its hooks must run in every backend that writes a \
         bound table, and this backend loaded the library on demand. Add it, restart (or \
         reconnect), and retry"
            .into(),
    )
}

/// Stands for every relation (`TRUNCATE … CASCADE`).
const ANY_REL: pg_sys::Oid = pg_sys::InvalidOid;

/// Note that the current (sub)transaction wrote relation `rel`.
fn note(rel: pg_sys::Oid) {
    if rel.to_u32() >= pg_sys::FirstNormalObjectId {
        note_raw(rel); // not a catalog, nor InvalidOid (a missing name)
    }
}

fn note_raw(rel: pg_sys::Oid) {
    // SAFETY: a plain read of the transaction state.
    let sub = unsafe { pg_sys::GetCurrentSubTransactionId() };
    WRITTEN.with(|w| {
        w.borrow_mut().entry(rel).or_insert(sub);
    });
}

#[pg_guard]
unsafe extern "C-unwind" fn executor_finish(qd: *mut pg_sys::QueryDesc) {
    match PREV_FINISH {
        Some(prev) => pg_guard_ffi_boundary(|| prev(qd)),
        None => pg_sys::standard_ExecutorFinish(qd),
    }
    let ps = (*qd).plannedstmt;
    if ps.is_null() || (*ps).resultRelations.is_null() {
        return;
    }
    // A plain write that changed no row changes no result. (A writable
    // CTE's rows are not in es_processed, so those always count.)
    let estate = (*qd).estate;
    if !(*ps).hasModifyingCTE && !estate.is_null() && (*estate).es_processed == 0 {
        return;
    }
    // Every ModifyTable's result relations (writable CTEs included), as
    // 1-based range-table indexes; over-noting is harmless.
    let rtable = PgList::<pg_sys::RangeTblEntry>::from_pg((*ps).rtable);
    for i in PgList::<()>::from_pg((*ps).resultRelations).iter_int() {
        if let Some(rte) = usize::try_from(i - 1).ok().and_then(|i| rtable.get_ptr(i)) {
            note((*rte).relid);
        }
    }
}

#[pg_guard]
#[allow(clippy::too_many_arguments)]
unsafe extern "C-unwind" fn process_utility(
    pstmt: *mut pg_sys::PlannedStmt,
    query: *const std::ffi::c_char,
    read_only_tree: bool,
    context: pg_sys::ProcessUtilityContext::Type,
    params: pg_sys::ParamListInfo,
    env: *mut pg_sys::QueryEnvironment,
    dest: *mut pg_sys::DestReceiver,
    qc: *mut pg_sys::QueryCompletion,
) {
    match PREV_UTILITY {
        Some(prev) => pg_guard_ffi_boundary(|| {
            prev(pstmt, query, read_only_tree, context, params, env, dest, qc)
        }),
        None => pg_sys::standard_ProcessUtility(
            pstmt,
            query,
            read_only_tree,
            context,
            params,
            env,
            dest,
            qc,
        ),
    }
    // TRUNCATE and COPY FROM write without the executor hooks. Resolved
    // after the statement ran: the names resolve as they did for it.
    let node = (*pstmt).utilityStmt;
    let relid = |rv: *mut pg_sys::RangeVar| {
        pg_sys::RangeVarGetRelidExtended(
            rv,
            pg_sys::NoLock as _,
            pg_sys::RVROption::RVR_MISSING_OK,
            None,
            std::ptr::null_mut(),
        )
    };
    if node.is_null() {
    } else if pgrx::is_a(node, pg_sys::NodeTag::T_TruncateStmt) {
        let t = node.cast::<pg_sys::TruncateStmt>();
        if (*t).behavior == pg_sys::DropBehavior::DROP_CASCADE {
            note_raw(ANY_REL);
        }
        for rv in PgList::<pg_sys::RangeVar>::from_pg((*t).relations).iter_ptr() {
            note(relid(rv));
        }
    } else if pgrx::is_a(node, pg_sys::NodeTag::T_CopyStmt) {
        let copy = node.cast::<pg_sys::CopyStmt>();
        if (*copy).is_from && !(*copy).relation.is_null() {
            note(relid((*copy).relation));
        }
    }
}

#[pg_guard]
unsafe extern "C-unwind" fn subxact_callback(
    event: pg_sys::SubXactEvent::Type,
    my: pg_sys::SubTransactionId,
    parent: pg_sys::SubTransactionId,
    _arg: *mut std::ffi::c_void,
) {
    WRITTEN.with(|w| {
        let mut w = w.borrow_mut();
        match event {
            pg_sys::SubXactEvent::SUBXACT_EVENT_ABORT_SUB => w.retain(|_, s| *s != my),
            pg_sys::SubXactEvent::SUBXACT_EVENT_COMMIT_SUB => {
                for s in w.values_mut().filter(|s| **s == my) {
                    *s = parent;
                }
            }
            _ => {}
        }
    });
}

#[pg_guard]
pub(crate) unsafe extern "C-unwind" fn xact_callback(
    event: pg_sys::XactEvent::Type,
    _arg: *mut std::ffi::c_void,
) {
    use pg_sys::XactEvent::*;
    match event {
        XACT_EVENT_PRE_COMMIT | XACT_EVENT_PRE_PREPARE => pre_commit(),
        _ => WRITTEN.with(|w| w.borrow_mut().clear()),
    }
}

/// The relations in `written` that a commit-mode model binds (all of them
/// for [`ANY_REL`]).
fn hits(set: &HashSet<pg_sys::Oid>, written: &[pg_sys::Oid]) -> Vec<pg_sys::Oid> {
    if set.is_empty() {
        Vec::new()
    } else if written.contains(&ANY_REL) {
        vec![ANY_REL]
    } else {
        written
            .iter()
            .copied()
            .filter(|r| set.contains(r))
            .collect()
    }
}

/// [`hits`] from the cache; `None` when an invalidation dropped it.
fn cached_hits(written: &[pg_sys::Oid]) -> Option<Vec<pg_sys::Oid>> {
    BOUND.with(|b| {
        b.borrow()
            .as_ref()
            .filter(|(generation, _)| *generation == CACHE_GEN.get())
            .map(|(_, set)| hits(set, written))
    })
}

/// [`hits`], reloading the cache when an invalidation dropped it.
fn bound_hits(written: &[pg_sys::Oid]) -> Res<Vec<pg_sys::Oid>> {
    if let Some(h) = cached_hits(written) {
        return Ok(h);
    }
    let generation = CACHE_GEN.get();
    let set = load_bound()?;
    let h = hits(&set, written);
    // A REPEATABLE READ snapshot may predate a committed mode switch: use
    // what it reads, but never cache it for later transactions.
    // SAFETY: a plain read of the transaction's isolation level.
    if unsafe { pg_sys::XactIsoLevel } < pg_sys::XACT_REPEATABLE_READ as i32 {
        BOUND.with(|b| *b.borrow_mut() = Some((generation, set)));
    }
    Ok(h)
}

/// The commit-mode models' bound relations, with their partition ancestors
/// and descendants (empty when the extension is not installed here).
fn load_bound() -> Res<HashSet<pg_sys::Oid>> {
    let watched = Spi::get_two::<pg_sys::Oid, pg_sys::Oid>(
        "SELECT to_regclass('improv.model')::oid, to_regclass('improv.binding')::oid",
    )?;
    let (Some(model), Some(binding)) = watched else {
        WATCHED.set(None);
        return Ok(HashSet::new());
    };
    WATCHED.set(Some([model, binding]));
    Spi::connect(|c| {
        c.select(
            "SELECT DISTINCT x.r FROM improv.binding b JOIN improv.model m ON m.id = b.model_id, \
               LATERAL (SELECT b.rel::oid \
                        UNION SELECT t.relid::oid FROM pg_partition_tree(b.rel) t \
                        UNION SELECT a.relid::oid FROM pg_partition_ancestors(b.rel) a) x(r) \
             WHERE m.sync_mode = 'commit'",
            None,
            &[],
        )?
        .map(|r| req::<pg_sys::Oid>(&r, 1))
        .collect()
    })
}

/// Recompute every commit-mode model bound to a relation this transaction
/// wrote: the body of the `XACT_EVENT_PRE_COMMIT` / `PRE_PREPARE` callback.
/// Raises (aborting the commit) when one fails.
pub(crate) fn pre_commit() {
    if WRITTEN.with(|w| w.borrow().is_empty()) {
        return;
    }
    // SAFETY: in a live transaction; applies other backends' committed
    // catalog changes (a mode switch) to the cache before it is read.
    unsafe { pg_sys::AcceptInvalidationMessages() };
    // The common case, writes to no commit-bound relation: one set lookup.
    let written: Vec<pg_sys::Oid> = WRITTEN.with(|w| w.borrow().keys().copied().collect());
    if cached_hits(&written).is_some_and(|h| h.is_empty()) {
        WRITTEN.with(|w| w.borrow_mut().clear());
        return;
    }
    // SPI needs an active snapshot that sees this transaction's writes.
    push_snapshot();
    // A model's results may be another commit-mode model's input, so go
    // again on what the recomputes wrote (bounded: models could feed each
    // other in a cycle).
    for _ in 0..16 {
        let written: Vec<pg_sys::Oid> =
            WRITTEN.with(|w| w.borrow_mut().drain().map(|(r, _)| r).collect());
        if written.is_empty() {
            break;
        }
        let hits = match super::owner::as_catalog(|| bound_hits(&written)) {
            Ok(h) => h,
            Err(e) => error!("improv: commit-mode sync could not read its catalog: {e}"),
        };
        if hits.is_empty() {
            break;
        }
        let models = match super::owner::as_catalog(|| models_for(&hits)) {
            Ok(m) => m,
            Err(e) => error!("improv: commit-mode sync could not read its catalog: {e}"),
        };
        for (name, measures) in models {
            let r = PgTryBuilder::new(|| {
                // The catalog as the extension (the writer needs no
                // privilege on `improv`), the model as its owner.
                super::owner::as_catalog(|| recompute(&name, &measures))
                    .map_err(|e| (PgSqlErrorCode::ERRCODE_INTERNAL_ERROR, e.to_string()))
            })
            .catch_others(|e| {
                Err(match e {
                    CaughtError::PostgresError(r)
                    | CaughtError::ErrorReport(r)
                    | CaughtError::RustPanic { ereport: r, .. } => {
                        (r.sql_error_code(), r.message().to_string())
                    }
                })
            })
            .execute();
            if let Err((code, msg)) = r {
                ErrorReport::new(
                    code,
                    format!(
                        "improv: model {name:?} (sync_mode commit) could not recompute before \
                         COMMIT, so the transaction is rolled back: {msg}"
                    ),
                    "improv commit sync",
                )
                .report(PgLogLevel::ERROR);
            }
        }
    }
    // SAFETY: pops the snapshot pushed above.
    unsafe { pg_sys::PopActiveSnapshot() };
    WRITTEN.with(|w| w.borrow_mut().clear());
}

/// Give SPI an active snapshot that sees this transaction's own writes (an
/// error aborts the transaction, which unwinds the snapshot stack).
fn push_snapshot() {
    // SAFETY: in a live transaction, before its commit work starts.
    unsafe {
        pg_sys::CommandCounterIncrement();
        pg_sys::PushActiveSnapshot(pg_sys::GetTransactionSnapshot());
    }
}

/// Commit-mode models bound to any of `written` (or to a partition
/// ancestor or descendant of one; [`ANY_REL`]: any relation), in id order
/// (the lock order), each with its measures bound on those relations.
fn models_for(written: &[pg_sys::Oid]) -> Res<Vec<(String, Vec<i64>)>> {
    Spi::connect(|c| {
        c.select(
            "WITH w AS (SELECT unnest($1::oid[]) AS r), \
             hit AS (SELECT r FROM w \
               UNION SELECT a.relid::oid FROM w, pg_partition_ancestors(w.r::regclass) a \
               UNION SELECT t.relid::oid FROM w, pg_partition_tree(w.r::regclass) t) \
             SELECT m.name, array_agg(b.measure_id ORDER BY b.measure_id) \
             FROM improv.model m JOIN improv.binding b ON b.model_id = m.id \
             WHERE m.sync_mode = 'commit' \
               AND (b.rel::oid IN (SELECT r FROM hit) OR 0::oid = ANY($1)) \
             GROUP BY m.id, m.name ORDER BY m.id",
            None,
            &[written.to_vec().into()],
        )?
        .map(|r| Ok((req(&r, 1)?, req(&r, 2)?)))
        .collect()
    })
}

/// Lock, load and recompute model `name`: the dependents of `measures`.
fn recompute(name: &str, measures: &[i64]) -> Res<()> {
    let (mid, name) = lock_model(name)?;
    let mode = Spi::get_one_with_args::<String>(
        "SELECT sync_mode FROM improv.model WHERE id = $1",
        &[mid.into()],
    )?;
    if mode.as_deref() != Some("commit") {
        return Ok(()); // switched while we waited for the lock
    }
    if let Some(tables) = unkeyed_results(mid, &name)? {
        // Mid-restore: pg_restore and a plain dump load each table's data in
        // its own transaction and add primary keys after all of it, so a
        // bound table's COPY commits while the result tables have no key to
        // upsert on. The dump carries the results, consistent with the
        // tables; the next write (or recalc) recomputes.
        notice!(
            "improv: model {name:?} (sync_mode commit) skipped its recompute at this COMMIT: its \
             result tables {tables} have no primary key yet, as during a pg_restore; the next \
             write to its tables (or improv.recalc) recomputes it"
        );
        return Ok(());
    }
    let mut l = super::trigger::load(mid, name, true)?;
    let changed = measures
        .iter()
        .map(|&i| id32(i).map(MeasureId))
        .collect::<Res<Vec<_>>>()?;
    // shortcut: recomputes the written relations' dependents in full (2.1 s
    // per commit at 2.5M rows, the load). A row scope needs the touched
    // rows, and the executor hook sees only which relations were written.
    // Measured in 0.11.0 and rejected: a transition-table statement trigger
    // costs about 1 ms per 1,000-row statement but 35% on a 1M-row INSERT,
    // and is the trigger on users' tables commit mode exists to avoid; an
    // `xmin = current xid` scan reads the whole table (0.44 s at 2.5M rows,
    // a fifth of the load, before any recompute). Upgrade when a hook can
    // see modified tuples without a trigger. Use trigger or logical mode for
    // large aggregated fact tables.
    let targets = pm::dependents(&l.model, &changed);
    super::trigger::recompute(
        &mut l,
        &pm::Plan {
            targets,
            scope: None,
        },
    )?;
    RECOMPUTES.with(|n| n.set(n.get() + 1));
    Ok(())
}

/// Model `mid`'s result tables that have dimension columns but no primary
/// key (the upsert's conflict target), or `None`. A restore makes those:
/// it creates the tables first and adds their keys after all the data.
fn unkeyed_results(mid: i64, name: &str) -> Res<Option<String>> {
    Ok(Spi::get_one_with_args::<String>(
        "SELECT string_agg(format('%I.%I', n.nspname, c.relname), ', ' ORDER BY c.relname) \
         FROM improv.measure d \
         JOIN pg_namespace n ON n.nspname = $2 \
         JOIN pg_class c ON c.relnamespace = n.oid AND c.relname = lower(d.name) \
         WHERE d.model_id = $1 AND d.kind = 'derived' \
           AND EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attnum > 0 \
                       AND NOT a.attisdropped AND a.attname <> 'value') \
           AND NOT EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary)",
        &[mid.into(), pm::result_schema(name).into()],
    )?)
}

/// Pre-commit model recomputes run by this backend so far.
#[cfg(any(test, feature = "pg_test"))]
pub(crate) fn recomputes() -> u64 {
    RECOMPUTES.with(Cell::get)
}
