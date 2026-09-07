//! Hosted refresh scheduler: a background tokio task mirroring the CLI's
//! standalone `improv serve-refresh` daemon (`crates/cli/src/main.rs`,
//! `cmd_serve_refresh`), but running in-process alongside the API instead of
//! as a separate process.
//!
//! # Scope decision: CALL measures only, not SQL
//!
//! The CLI daemon refreshes two measure kinds: external-function (`CALL`)
//! measures via [`improv_engine::external::refresh_external_measure`], and
//! SQL-sourced measures via `improv_storage_sql::refresh_sql_measure`, the
//! latter needing a *source connection* (a `rusqlite::Connection` opened from
//! an optional `source.sqlite` CLI argument). The hosted server has no
//! per-request or per-server notion of a SQL source connection string —
//! `improv_core_model::SqlSource` (the model's SQL-measure metadata) carries
//! only the query/column mapping, not a connection string, so inventing one
//! here would be scope creep the model doesn't support. The hosted scheduler
//! therefore refreshes only `CALL` (external-function) measures
//! automatically; SQL-sourced measures still need `improv serve-refresh
//! <db> <source.sqlite>` or a manual refresh path.
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use improv_core_model::{MeasureId, RefreshPolicy};
use improv_storage_mentat::ModelStore;

use crate::AppState;

/// Snapshot of the scheduler's last tick, for the `/scheduler/status` route.
#[derive(Clone, Default)]
pub struct SchedulerStatus {
    pub tick_secs: u64,
    /// Ids of measures refreshed on the most recent tick (empty if none were
    /// due, or if the tick errored before refreshing anything).
    pub last_tick_refreshed: Vec<u32>,
    /// Error from the most recent tick, if any (cleared on a successful tick).
    pub last_error: Option<String>,
}

pub type SharedStatus = Arc<Mutex<SchedulerStatus>>;

/// Spawn the background refresh loop: every `tick`, reload the model, refresh
/// any due `CALL` measures, and save if anything changed. Never panics or
/// exits on a tick error — it logs (`eprintln!`, matching the CLI daemon's
/// style) and tries again next tick.
pub fn spawn_scheduler(
    state: Arc<AppState>,
    tick: Duration,
    status: SharedStatus,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let start = Instant::now();
        // last_run per measure, in whole seconds since `start` — lives for the
        // task's lifetime, tracked across iterations like the CLI daemon's
        // `HashMap<MeasureId, u64>`.
        let mut last_run: HashMap<MeasureId, u64> = HashMap::new();
        eprintln!(
            "scheduler: refreshing due CALL measures every {}s",
            tick.as_secs()
        );
        loop {
            tokio::time::sleep(tick).await;
            match tick_once(&state, &mut last_run, start).await {
                Ok(refreshed) => {
                    if let Ok(mut s) = status.lock() {
                        s.last_tick_refreshed = refreshed.iter().map(|m| m.0).collect();
                        s.last_error = None;
                    }
                }
                Err(e) => {
                    eprintln!("scheduler: tick error (continuing): {e}");
                    if let Ok(mut s) = status.lock() {
                        s.last_tick_refreshed.clear();
                        s.last_error = Some(e);
                    }
                }
            }
        }
    })
}

/// One scheduler tick: load the model, refresh due CALL measures, save if any
/// were refreshed. Returns the ids refreshed. All I/O runs on a blocking
/// thread, matching the request handlers' `spawn_blocking` pattern.
async fn tick_once(
    state: &Arc<AppState>,
    last_run: &mut HashMap<MeasureId, u64>,
    start: Instant,
) -> Result<Vec<MeasureId>, String> {
    let now_secs = start.elapsed().as_secs();
    let path = state.db_path.clone();

    // Load the model, compute due CALL measures against `last_run`, and
    // refresh them, all on a blocking thread (Mentat's store and the external
    // runtime's subprocess calls are blocking).
    let refreshed = tokio::task::spawn_blocking({
        let last_run_snapshot = last_run.clone();
        move || -> Result<Vec<MeasureId>, String> {
            let mut store = ModelStore::open(&path).map_err(|e| e.to_string())?;
            let mut model = store.load_model().map_err(|e| e.to_string())?;

            let policies: HashMap<MeasureId, RefreshPolicy> = model
                .external_calls
                .iter()
                .map(|(id, c)| (*id, c.refresh_policy))
                .collect();
            let due =
                improv_core_model::schedule::due_measures(&policies, now_secs, &last_run_snapshot);

            let mut refreshed = Vec::new();
            for mid in &due {
                match improv_engine::external::refresh_external_measure(
                    &mut model,
                    improv_engine::external::DEFAULT_TIMEOUT,
                    *mid,
                ) {
                    Ok(n) => {
                        eprintln!(
                            "scheduler: [{now_secs}s] refreshed measure {} ({n} cells)",
                            mid.0
                        );
                        refreshed.push(*mid);
                    }
                    Err(e) => {
                        // One measure's refresh failing must not stop the
                        // others, nor crash the tick: log and continue.
                        eprintln!("scheduler: measure {} refresh failed: {e}", mid.0);
                    }
                }
            }
            if !refreshed.is_empty() {
                store.save_model(&model).map_err(|e| e.to_string())?;
            }
            Ok(refreshed)
        }
    })
    .await
    .map_err(|e| format!("task join: {e}"))??;

    for mid in &refreshed {
        last_run.insert(*mid, now_secs);
    }
    Ok(refreshed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{
        CategoryId, ExternalFn, ItemId, Language, Measure, MeasureKind, Model, Name, Value,
        ValueType,
    };
    use improv_storage_mentat::ModelStore;

    fn python_available() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn temp_db_path(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("improv-sched-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "{tag}-{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    // A model with one input measure `X` and a `CALL(double, X)` measure `H`
    // with the given refresh policy.
    fn call_model(policy: RefreshPolicy) -> Model {
        let mut m = Model::new();
        let product = CategoryId(2);
        m.add_category(product, "Product");
        m.add_item(ItemId(20), product, "A");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("X".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.set_input(
            MeasureId(100),
            improv_core_model::Coordinate::from_pairs([(product, ItemId(20))]),
            Value::Number(3.0),
        );
        m.add_measure(Measure {
            id: MeasureId(200),
            name: Name("H".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input, // populated by refresh, like a CALL measure
            description: None,
        });
        m.external_fns.insert(
            "double".into(),
            ExternalFn {
                name: "double".into(),
                language: Language::Python,
                body: "result = args[0] * 2".into(),
                arg_types: vec![ValueType::Number],
                return_type: ValueType::Number,
                pure: true,
            },
        );
        m.external_calls.insert(
            MeasureId(200),
            improv_core_model::ExternalCall {
                func: "double".into(),
                arg_measures: vec![MeasureId(100)],
                refresh_policy: policy,
            },
        );
        m
    }

    fn app_state(db_path: &str) -> Arc<AppState> {
        Arc::new(AppState {
            db_path: db_path.to_string(),
            auth: crate::Auth::Disabled,
            scheduler_status: None,
        })
    }

    // The scheduler actually runs: a due CALL measure gets refreshed and the
    // written cells land on disk.
    #[tokio::test]
    async fn scheduler_refreshes_due_call_measure() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        let path = temp_db_path("due");
        let mut store = ModelStore::open(path.to_str().unwrap()).unwrap();
        store
            .save_model(&call_model(RefreshPolicy::Interval { secs: 0 }))
            .unwrap();

        let state = app_state(path.to_str().unwrap());
        let status: SharedStatus = Arc::new(Mutex::new(SchedulerStatus {
            tick_secs: 0,
            ..Default::default()
        }));
        let handle = spawn_scheduler(state, Duration::from_millis(50), status.clone());

        // Give it a few ticks to run; python3 subprocess startup can take a
        // while under load, so poll instead of a single fixed sleep.
        let mut refreshed_val = None;
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let mut probe = ModelStore::open(path.to_str().unwrap()).unwrap();
            let model = probe.load_model().unwrap();
            let coord = improv_core_model::Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
            if let Some(v) = model.input(MeasureId(200), &coord) {
                refreshed_val = Some(v.clone());
                break;
            }
        }
        // The status update happens right after the disk write, in the same
        // task but a separate poll; give it a moment to land.
        let mut saw_status = false;
        for _ in 0..20 {
            if status.lock().unwrap().last_tick_refreshed.contains(&200) {
                saw_status = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        handle.abort();

        assert_eq!(
            refreshed_val,
            Some(Value::Number(6.0)),
            "scheduler should have refreshed H = double(X) = 6"
        );
        assert!(saw_status, "status should report the refreshed measure");
    }

    // A tick with no due measures (Manual policy) is a no-op: it must not
    // error, and must not rewrite the file (checked via unchanged mtime).
    #[tokio::test]
    async fn no_due_measures_is_a_noop() {
        let path = temp_db_path("noop");
        let mut store = ModelStore::open(path.to_str().unwrap()).unwrap();
        store
            .save_model(&call_model(RefreshPolicy::Manual))
            .unwrap();
        let mtime_before = std::fs::metadata(&path).unwrap().modified().unwrap();

        let state = app_state(path.to_str().unwrap());
        let mut last_run = HashMap::new();
        let refreshed = tick_once(&state, &mut last_run, Instant::now())
            .await
            .expect("a no-op tick must not error");
        assert!(refreshed.is_empty());

        let mtime_after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert_eq!(
            mtime_before, mtime_after,
            "a tick that refreshes nothing must not rewrite the store"
        );
    }

    // A refresh failure (unregistered external function) on one measure must
    // not panic the tick, and the scheduler loop must still be alive and able
    // to run subsequent ticks.
    #[tokio::test]
    async fn refresh_error_does_not_kill_the_loop() {
        let path = temp_db_path("err");
        let mut model = call_model(RefreshPolicy::Interval { secs: 0 });
        // Point the CALL measure at a function that isn't registered.
        model.external_calls.get_mut(&MeasureId(200)).unwrap().func = "nope".into();
        let mut store = ModelStore::open(path.to_str().unwrap()).unwrap();
        store.save_model(&model).unwrap();

        let state = app_state(path.to_str().unwrap());
        let status: SharedStatus = Arc::new(Mutex::new(SchedulerStatus::default()));
        let handle = spawn_scheduler(state, Duration::from_millis(50), status);

        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !handle.is_finished(),
            "a per-measure refresh error must not end the scheduler task"
        );
        handle.abort();
    }
}
