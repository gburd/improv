//! `improv-server`: a small JSON HTTP API over an Improv model store.
//!
//! The (blocking SQLite-backed) Mentat store is opened inside `spawn_blocking`
//! and never shared across tasks. The loaded model is cached in [`AppState`]
//! and replaced on every write this process makes (API or scheduler), so reads
//! cost one store load per write, not per request. The server assumes it owns
//! the store: a write from another process is not seen until restart.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    error_handling::HandleErrorLayer,
    extract::{rejection::JsonRejection, DefaultBodyLimit, Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use improv_core_model::{Coordinate, MeasureId, MeasureKind, Model};
use improv_engine::dataflow::evaluate;
use improv_engine::session::{Edit, Engine};
use improv_engine::{decode_coord, CoordKey};
use improv_nl_formula::{describe_formula, parse_nl_formula, NlContext};
use improv_storage_mentat::{ModelStore, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tower::ServiceBuilder;

mod scheduler;
use scheduler::SharedStatus;

/// Accepted bearer tokens, or open mode.
///
/// `Disabled` = no auth (local dev / tests, preserves v1 behavior);
/// `Tokens` = require `Authorization: Bearer <t>`: a `t` in `rw` may read and
/// write, a `t` in `ro` may only read (S12).
#[derive(Clone)]
pub enum Auth {
    Disabled,
    Tokens {
        rw: HashSet<String>,
        ro: HashSet<String>,
    },
}

impl Auth {
    /// Build from `IMPROV_API_TOKEN` (single) + `IMPROV_API_TOKENS` (comma-sep)
    /// for read-write, and `IMPROV_API_TOKENS_RO` (comma-sep) for read-only.
    /// Empty/whitespace tokens are ignored; no tokens => `Disabled`.
    fn from_env() -> Self {
        let list = |var: &str| -> HashSet<String> {
            std::env::var(var)
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|t| !t.is_empty())
                .map(String::from)
                .collect()
        };
        let mut rw = list("IMPROV_API_TOKENS");
        if let Ok(t) = std::env::var("IMPROV_API_TOKEN") {
            let t = t.trim();
            if !t.is_empty() {
                rw.insert(t.to_string());
            }
        }
        let ro = list("IMPROV_API_TOKENS_RO");
        if rw.is_empty() && ro.is_empty() {
            Auth::Disabled
        } else {
            Auth::Tokens { rw, ro }
        }
    }
}

/// Request extension set by [`require_token`] for a read-only token.
#[derive(Clone, Copy)]
struct ReadOnly;

/// Request body cap (S13). Over-limit bodies get 413.
const MAX_BODY_BYTES: usize = 1 << 20;
/// Edits accepted per `/eval` request (S13). More gets 400.
const MAX_EDITS: usize = 10_000;
/// Whole-request deadline, including time queued for a concurrency slot (S10).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Requests in flight across all routes (S10); the rest wait (then time out).
const MAX_IN_FLIGHT: usize = 64;
/// Default deadline for a client to finish sending request headers (S10).
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);

pub struct AppState {
    db_path: String,
    auth: Auth,
    /// `None` when the scheduler is disabled (the default; `IMPROV_SCHEDULER=1` enables).
    scheduler_status: Option<SharedStatus>,
    /// Held across every load-modify-save of the store (API writes and the
    /// scheduler), so concurrent writers cannot lose each other's updates (S12).
    write_lock: tokio::sync::Mutex<()>,
    /// The loaded model, `None` until first read or after invalidation (P4).
    cache: tokio::sync::RwLock<Option<Arc<Model>>>,
    /// The store high-water `cache` was loaded/saved at; a save from a stale
    /// cache (another process wrote since) is refused with 409.
    loaded_tx: std::sync::Mutex<Option<i64>>,
    /// Store loads performed, so tests can check the cache.
    #[cfg(test)]
    loads: AtomicU64,
}

impl AppState {
    pub fn new(db_path: String, auth: Auth, scheduler_status: Option<SharedStatus>) -> Self {
        AppState {
            db_path,
            auth,
            scheduler_status,
            write_lock: tokio::sync::Mutex::new(()),
            cache: tokio::sync::RwLock::new(None),
            loaded_tx: std::sync::Mutex::new(None),
            #[cfg(test)]
            loads: AtomicU64::new(0),
        }
    }
}

// Coverage exclusion: process entry point (argv, env, bind, `process::exit`);
// its parts (`Auth::from_env`, `startup_check`, `serve`, `app`) are tested.
#[cfg_attr(coverage_nightly, coverage(off))]
#[tokio::main]
async fn main() {
    // argv[1]: db path (default in-memory-ish temp file). argv[2] or
    // IMPROV_ADDR: bind address (default 127.0.0.1:3000).
    let mut args = std::env::args().skip(1);
    let db_path = args.next().unwrap_or_else(|| "improv.db".to_string());
    let addr = args
        .next()
        .or_else(|| std::env::var("IMPROV_ADDR").ok())
        .unwrap_or_else(|| "127.0.0.1:3000".to_string());

    let auth = Auth::from_env();
    if matches!(auth, Auth::Disabled) {
        eprintln!("auth disabled: set IMPROV_API_TOKEN to require a bearer token");
    }
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    let local = listener.local_addr().expect("bound socket has an address");
    let allow_open = std::env::var("IMPROV_ALLOW_OPEN").ok();
    if let Err(e) = startup_check(local, &auth, allow_open.as_deref()) {
        eprintln!("improv-server: {e}");
        std::process::exit(2);
    }

    let scheduler_enabled =
        scheduler::scheduler_enabled(std::env::var("IMPROV_SCHEDULER").ok().as_deref());
    let tick_secs = std::env::var("IMPROV_SCHEDULER_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(5);

    let scheduler_status = scheduler_enabled.then(|| {
        Arc::new(std::sync::Mutex::new(scheduler::SchedulerStatus {
            tick_secs,
            ..Default::default()
        }))
    });
    let state = Arc::new(AppState::new(db_path, auth, scheduler_status.clone()));
    if let Some(status) = scheduler_status {
        let allow = match std::env::var("IMPROV_EXTFN_ALLOW") {
            Ok(file) => match std::fs::read_to_string(&file) {
                Ok(text) => scheduler::parse_allowlist(&text),
                Err(e) => {
                    eprintln!("scheduler: cannot read IMPROV_EXTFN_ALLOW {file}: {e}; no function will run");
                    Default::default()
                }
            },
            Err(_) => {
                eprintln!("scheduler: IMPROV_EXTFN_ALLOW unset; no function will run");
                Default::default()
            }
        };
        scheduler::spawn_scheduler(
            state.clone(),
            std::time::Duration::from_secs(tick_secs),
            status,
            Arc::new(allow),
        );
    } else {
        eprintln!("scheduler disabled (set IMPROV_SCHEDULER=1 to enable)");
    }

    eprintln!("improv-server listening on {addr}");
    let header_timeout = std::env::var("IMPROV_HEADER_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(HEADER_READ_TIMEOUT);
    serve(listener, app(state), header_timeout).await;
}

/// Refuse open mode (no tokens) on a non-loopback address unless the
/// operator opts in with `IMPROV_ALLOW_OPEN=1` (S15).
pub fn startup_check(
    addr: SocketAddr,
    auth: &Auth,
    allow_open: Option<&str>,
) -> Result<(), String> {
    if matches!(auth, Auth::Disabled) && !addr.ip().is_loopback() && allow_open != Some("1") {
        return Err(format!(
            "refusing to serve {addr} without auth: set IMPROV_API_TOKEN, bind a \
             loopback address, or set IMPROV_ALLOW_OPEN=1"
        ));
    }
    Ok(())
}

/// Accept loop: hyper HTTP/1 with a header-read timeout (S10). `axum::serve`
/// (0.7) sets no header timer, so a client that never finishes its request
/// line holds a socket forever (slow-loris). HTTP/1 only: the `hyper-util`
/// `auto` builder would pull in h2, which the API does not need.
async fn serve(listener: tokio::net::TcpListener, app: Router, header_timeout: Duration) {
    use hyper_util::rt::{TokioIo, TokioTimer};
    loop {
        let stream = match listener.accept().await {
            Ok((stream, _)) => stream,
            Err(e) => {
                // EMFILE etc.: back off instead of spinning.
                eprintln!("accept: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let svc = hyper_util::service::TowerToHyperService::new(app.clone());
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(header_timeout)
                .serve_connection(TokioIo::new(stream), svc)
                .await;
        });
    }
}

/// Build the router. Exposed so tests can drive it via `oneshot`.
pub fn app(state: Arc<AppState>) -> Router {
    // Protected routes: everything except `/health` (public for liveness probes).
    let protected = Router::new()
        .route("/model", get(get_model))
        .route("/measures", get(list_measures))
        .route("/measures/:id/values", get(measure_values))
        .route("/measures/:id/eval", post(eval_measure))
        .route("/measures/:id/cells", post(set_cell))
        .route("/nl/parse", post(nl_parse))
        .route("/nl/describe", post(nl_describe))
        .route("/scheduler/status", get(scheduler_status_handler))
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));

    // Global (not per-route) limit: `Router::layer` clones the layer per
    // route, and `GlobalConcurrencyLimitLayer` shares one semaphore.
    let limits = ServiceBuilder::new()
        .layer(HandleErrorLayer::new(|e: tower::BoxError| async move {
            if e.is::<tower::timeout::error::Elapsed>() {
                (StatusCode::REQUEST_TIMEOUT, "request timed out")
            } else {
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error")
            }
        }))
        .layer(tower::timeout::TimeoutLayer::new(REQUEST_TIMEOUT))
        .layer(tower::limit::GlobalConcurrencyLimitLayer::new(
            MAX_IN_FLIGHT,
        ));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .layer(limits)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state)
}

/// Bearer-token gate. In open mode every request passes. Otherwise:
/// missing/blank `Authorization` -> 401; present but not an accepted token -> 403;
/// a read-only token passes with a [`ReadOnly`] extension the write paths refuse.
/// The token value is never logged.
async fn require_token(
    State(state): State<Arc<AppState>>,
    mut req: Request,
    next: Next,
) -> Response {
    let (rw, ro) = match &state.auth {
        Auth::Disabled => return next.run(req).await,
        Auth::Tokens { rw, ro } => (rw, ro),
    };
    // Check both classes every time so timing does not reveal which matched.
    let class = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| (token_matches(rw, t), token_matches(ro, t)));
    match class {
        None => (StatusCode::UNAUTHORIZED, "missing bearer token").into_response(),
        Some((true, _)) => next.run(req).await,
        Some((false, true)) => {
            req.extensions_mut().insert(ReadOnly);
            next.run(req).await
        }
        Some((false, false)) => (StatusCode::FORBIDDEN, "invalid bearer token").into_response(),
    }
}

/// Constant-time token check (S15): every accepted token is compared, and
/// each comparison folds over all bytes, so timing reveals neither which token
/// matched nor the length of a matching prefix.
fn token_matches<'a>(tokens: impl IntoIterator<Item = &'a String>, presented: &str) -> bool {
    tokens.into_iter().fold(false, |hit, t| {
        hit | ct_eq(t.as_bytes(), presented.as_bytes())
    })
}

/// Byte-wise xor accumulate; no early exit on the first differing byte.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    (a.len() == b.len()) & (diff == 0)
}

// --- error handling: never panic on request input ---

impl From<JsonRejection> for ApiError {
    fn from(e: JsonRejection) -> Self {
        // Keep the v1 contract (any bad body -> 400) except an oversized one.
        let status = match e.status() {
            StatusCode::PAYLOAD_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,
            _ => StatusCode::BAD_REQUEST,
        };
        ApiError::Rejected(status, e.body_text())
    }
}

enum ApiError {
    /// A rejected JSON body (malformed -> 400, oversized -> 413).
    Rejected(StatusCode, String),
    BadRequest(String),
    Forbidden(String),
    NotFound(String),
    /// The store changed under the cached model (another process wrote).
    Conflict(String),
    Internal(String),
}

/// Id logged with each internal error and echoed to the client as `request_id`.
static NEXT_ERROR_ID: AtomicU64 = AtomicU64::new(1);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::Rejected(s, m) => (s, m),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, m),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, m),
            ApiError::Internal(detail) => {
                // The detail (store/SQLite errors carry the DB path) stays in
                // the server log; the client gets an id to quote (S14).
                let id = NEXT_ERROR_ID.fetch_add(1, Ordering::Relaxed);
                eprintln!("request {id}: internal error: {detail}");
                let body = json!({ "error": "internal error", "request_id": id });
                return (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response();
            }
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

/// The current model: the cached copy, or (on a miss) one load from the store
/// on a blocking thread. The cache write lock is held across the load, so a
/// concurrent writer's [`save_model`] replaces it only after a stale load lands.
async fn load_model(state: &Arc<AppState>) -> Result<Arc<Model>, ApiError> {
    if let Some(m) = state.cache.read().await.as_ref() {
        return Ok(m.clone());
    }
    let mut cache = state.cache.write().await;
    if let Some(m) = cache.as_ref() {
        return Ok(m.clone());
    }
    #[cfg(test)]
    state.loads.fetch_add(1, Ordering::Relaxed);
    let path = state.db_path.clone();
    let (model, tx) = tokio::task::spawn_blocking(move || {
        let mut store = ModelStore::open(&path).map_err(|e| e.to_string())?;
        let model = store.load_model().map_err(|e| e.to_string())?;
        Ok((model, store.loaded_tx()))
    })
    .await
    .map_err(|e| ApiError::Internal(format!("task join: {e}")))?
    .map_err(ApiError::Internal)?;
    let model = Arc::new(model);
    *state.loaded_tx.lock().unwrap() = tx;
    *cache = Some(model.clone());
    Ok(model)
}

/// Save the model on a blocking thread and make it the cached copy. Call with
/// `write_lock` held.
async fn save_model(state: &Arc<AppState>, model: Model) -> Result<Arc<Model>, ApiError> {
    let path = state.db_path.clone();
    let base = *state.loaded_tx.lock().unwrap();
    let saved = tokio::task::spawn_blocking(move || {
        let mut store = ModelStore::open(&path)?;
        store.set_loaded_tx(base);
        store.save_model(&model)?;
        Ok((model, store.loaded_tx()))
    })
    .await
    .map_err(|e| ApiError::Internal(format!("task join: {e}")))?;
    let (model, tx) = match saved {
        Ok(v) => v,
        Err(e @ StoreError::Conflict(_)) => {
            // Drop the stale cache so the client's retry reloads.
            *state.cache.write().await = None;
            return Err(ApiError::Conflict(e.to_string()));
        }
        Err(e) => return Err(ApiError::Internal(e.to_string())),
    };
    let model = Arc::new(model);
    *state.loaded_tx.lock().unwrap() = tx;
    *state.cache.write().await = Some(model.clone());
    Ok(model)
}

/// S12: a write must target an existing input measure, at a coordinate naming
/// exactly the measure's categories, each with an item of that category, and
/// carry a finite value (`None` clears the cell).
fn validate_write(
    model: &Model,
    mid: MeasureId,
    coord: &Coordinate,
    value: Option<f64>,
) -> Result<(), ApiError> {
    let bad = |m: String| Err(ApiError::BadRequest(m));
    let Some(measure) = model.measures.get(&mid) else {
        return bad(format!("no measure {}", mid.0));
    };
    if !measure.is_input() {
        return bad(format!("measure {} is derived; cannot set a cell", mid.0));
    }
    if value.is_some_and(|v| !v.is_finite()) {
        return bad("value must be a finite number".into());
    }
    let dims_ok = coord.dims.len() == measure.categories.len()
        && measure.categories.iter().all(|c| {
            coord
                .dims
                .get(c)
                .and_then(|i| model.items.get(i))
                .is_some_and(|item| item.category == *c)
        });
    if !dims_ok {
        return bad(format!(
            "coordinate must name one item for each category of measure {}",
            mid.0
        ));
    }
    Ok(())
}

// --- handlers ---

async fn health() -> &'static str {
    "ok"
}

/// Scheduler observability: enabled/tick/last-refreshed. Protected (not a
/// liveness probe like `/health`) since it can reveal which measures exist.
async fn scheduler_status_handler(State(state): State<Arc<AppState>>) -> Json<JsonValue> {
    match &state.scheduler_status {
        None => Json(json!({ "enabled": false })),
        Some(status) => {
            let s = status.lock().unwrap_or_else(|e| e.into_inner());
            Json(json!({
                "enabled": true,
                "tick_secs": s.tick_secs,
                "last_tick_refreshed": s.last_tick_refreshed,
                "last_error": s.last_error,
            }))
        }
    }
}

async fn get_model(State(state): State<Arc<AppState>>) -> Result<Response, ApiError> {
    Ok(Json(&*load_model(&state).await?).into_response())
}

#[derive(Serialize)]
struct MeasureSummary {
    id: u32,
    name: String,
    kind: &'static str,
    categories: Vec<String>,
}

async fn list_measures(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<MeasureSummary>>, ApiError> {
    let model = load_model(&state).await?;
    let mut out: Vec<MeasureSummary> = model
        .measures
        .values()
        .map(|m| MeasureSummary {
            id: m.id.0,
            name: m.name.0.clone(),
            kind: if m.is_input() { "input" } else { "derived" },
            categories: m
                .categories
                .iter()
                .map(|c| category_name(&model, *c))
                .collect(),
        })
        .collect();
    out.sort_by_key(|m| m.id);
    Ok(Json(out))
}

#[derive(Serialize)]
struct Cell {
    coord: Vec<[String; 2]>,
    value: f64,
}

async fn measure_values(
    State(state): State<Arc<AppState>>,
    Path(id): Path<u32>,
) -> Result<Json<Vec<Cell>>, ApiError> {
    let model = load_model(&state).await?;
    let mid = MeasureId(id);
    let measure = model
        .measures
        .get(&mid)
        .ok_or_else(|| ApiError::NotFound(format!("no measure {id}")))?;

    let cells = match &measure.kind {
        MeasureKind::Input => model
            .inputs
            .iter()
            .filter(|((m, _), _)| *m == mid)
            .filter_map(|((_, coord), val)| {
                val.as_number().map(|n| readable_cell(&model, coord, n))
            })
            .collect(),
        MeasureKind::Derived(_) => {
            let mut out =
                evaluate(&model, &[mid]).map_err(|e| ApiError::Internal(e.to_string()))?;
            let values = out.remove(&mid).unwrap_or_default();
            values
                .into_iter()
                .map(|(k, n)| readable_key(&model, &k, n.as_num().unwrap_or(f64::NAN)))
                .collect()
        }
    };
    Ok(Json(cells))
}

/// One edit in an `/eval` request: `measure` id, `coord` as `[[cat_id, item_id], ...]`
/// (numeric ids, matching `CoordKey`), and `value` (`null` clears the cell).
#[derive(Deserialize)]
struct EvalEdit {
    measure: u32,
    coord: CoordKey,
    value: Option<f64>,
}

#[derive(Deserialize)]
struct EvalReq {
    #[serde(default)]
    edits: Vec<EvalEdit>,
    #[serde(default)]
    persist: bool,
}

/// Incremental / what-if evaluation. Builds a short-lived live `Engine` over
/// all derived measures, applies the batch of edits as deltas, and returns the
/// recomputed snapshot for measure `:id` as `[{coord: [[cat_name,item_name]..], value}]`.
///
/// Stateless by design: the `Engine` owns a worker thread and a live dataflow
/// and is not cheaply shareable across async tasks, so v1 rebuilds one per
/// request inside `spawn_blocking`. A persistent per-session engine pool is a
/// later optimization. Edits are pure what-if unless `"persist": true`, in
/// which case each edit is also written to the model store.
async fn eval_measure(
    State(state): State<Arc<AppState>>,
    ro: Option<Extension<ReadOnly>>,
    Path(id): Path<u32>,
    body: Result<Json<EvalReq>, JsonRejection>,
) -> Result<Json<Vec<Cell>>, ApiError> {
    let Json(req) = body?;
    if req.persist && ro.is_some() {
        return Err(ApiError::Forbidden("read-only token cannot persist".into()));
    }
    if req.edits.len() > MAX_EDITS {
        return Err(ApiError::BadRequest(format!(
            "too many edits ({}); at most {MAX_EDITS} per request",
            req.edits.len()
        )));
    }
    let mid = MeasureId(id);

    let write = match req.persist {
        true => Some(state.write_lock.lock().await),
        false => None,
    };
    let mut model = load_model(&state).await?;
    let measure = model
        .measures
        .get(&mid)
        .ok_or_else(|| ApiError::NotFound(format!("no measure {id}")))?;
    if measure.is_input() {
        return Err(ApiError::BadRequest(format!(
            "measure {id} is an input; use /eval on a derived measure"
        )));
    }

    // Validate every edit, what-if ones too: the engine never sees junk.
    let coords = req
        .edits
        .iter()
        .map(|e| {
            let coord = decode_coord(&e.coord);
            if coord.dims.len() != e.coord.len() {
                return Err(ApiError::BadRequest("coordinate repeats a category".into()));
            }
            validate_write(&model, MeasureId(e.measure), &coord, e.value)?;
            Ok(coord)
        })
        .collect::<Result<Vec<_>, ApiError>>()?;

    // Persist first (so a persisted edit is durable even if the caller only
    // wants the side effect); the engine below then sees the same values.
    if req.persist {
        let mut updated = (*model).clone();
        for (e, coord) in req.edits.iter().zip(coords) {
            match e.value {
                Some(v) => updated.set_input(
                    MeasureId(e.measure),
                    coord,
                    improv_core_model::Value::Number(v),
                ),
                None => {
                    updated.inputs.remove(&(MeasureId(e.measure), coord));
                }
            }
        }
        model = save_model(&state, updated).await?;
    }
    drop(write);

    // All derived measures are the engine's targets.
    let targets: Vec<MeasureId> = model
        .measures
        .iter()
        .filter(|(_, m)| !m.is_input())
        .map(|(id, _)| *id)
        .collect();
    let edits: Vec<Edit> = req
        .edits
        .into_iter()
        .map(|e| Edit {
            measure: MeasureId(e.measure),
            coord: e.coord,
            value: e.value,
        })
        .collect();

    // Engine owns a worker thread and holds a live dataflow: do it all on a
    // blocking thread and never across an await point.
    let engine_model = model.clone();
    let values = tokio::task::spawn_blocking(move || {
        let (mut engine, _initial) =
            Engine::new(&engine_model, &targets).map_err(|e| e.to_string())?;
        let snap = engine.apply(edits).map_err(|e| e.to_string())?;
        Ok::<_, String>(snap.get(&mid).cloned().unwrap_or_default())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("task join: {e}")))?
    .map_err(ApiError::Internal)?;

    let cells = values
        .iter()
        .map(|(k, n)| readable_key(&model, k, n.as_num().unwrap_or(f64::NAN)))
        .collect();
    Ok(Json(cells))
}

#[derive(Deserialize)]
struct NlParseReq {
    text: String,
}

async fn nl_parse(
    State(state): State<Arc<AppState>>,
    body: Result<Json<NlParseReq>, JsonRejection>,
) -> Result<Json<JsonValue>, ApiError> {
    let Json(req) = body?;
    let model = load_model(&state).await?;
    let ctx = NlContext::new(&model);
    match parse_nl_formula(&ctx, &req.text) {
        Ok(formula) => Ok(Json(
            serde_json::to_value(&formula).map_err(|e| ApiError::Internal(e.to_string()))?,
        )),
        Err(e) => Err(ApiError::BadRequest(e.to_string())),
    }
}

#[derive(Deserialize)]
struct NlDescribeReq {
    formula: JsonValue,
}

async fn nl_describe(
    State(state): State<Arc<AppState>>,
    body: Result<Json<NlDescribeReq>, JsonRejection>,
) -> Result<Json<JsonValue>, ApiError> {
    let Json(req) = body?;
    let formula = serde_json::from_value(req.formula)
        .map_err(|e| ApiError::BadRequest(format!("invalid formula: {e}")))?;
    let model = load_model(&state).await?;
    let ctx = NlContext::new(&model);
    Ok(Json(json!({ "text": describe_formula(&ctx, &formula) })))
}

#[derive(Deserialize)]
struct SetCellReq {
    coord: Coordinate,
    value: f64,
}

async fn set_cell(
    State(state): State<Arc<AppState>>,
    ro: Option<Extension<ReadOnly>>,
    Path(id): Path<u32>,
    body: Result<Json<SetCellReq>, JsonRejection>,
) -> Result<Json<JsonValue>, ApiError> {
    if ro.is_some() {
        return Err(ApiError::Forbidden("read-only token cannot write".into()));
    }
    let Json(req) = body?;
    let mid = MeasureId(id);

    let _write = state.write_lock.lock().await;
    let model = load_model(&state).await?;
    if !model.measures.contains_key(&mid) {
        return Err(ApiError::NotFound(format!("no measure {id}")));
    }
    validate_write(&model, mid, &req.coord, Some(req.value))?;
    let mut model = (*model).clone();
    model.set_input(mid, req.coord, improv_core_model::Value::Number(req.value));
    save_model(&state, model).await?;

    Ok(Json(json!({ "ok": true })))
}

// --- coordinate rendering helpers ---

fn category_name(model: &Model, c: improv_core_model::CategoryId) -> String {
    model
        .categories
        .get(&c)
        .map(|cat| cat.name.0.clone())
        .unwrap_or_else(|| format!("category {}", c.0))
}

/// Render a coordinate as `[[category_name, item_name], ...]`, sorted by
/// category id for stability.
fn readable_cell(model: &Model, coord: &Coordinate, value: f64) -> Cell {
    let mut pairs: Vec<[String; 2]> = coord
        .dims
        .iter()
        .map(|(cat, item)| {
            let cat_name = category_name(model, *cat);
            let item_name = model
                .items
                .get(item)
                .map(|i| i.name.0.clone())
                .unwrap_or_else(|| format!("item {}", item.0));
            [cat_name, item_name]
        })
        .collect();
    pairs.sort();
    Cell {
        coord: pairs,
        value,
    }
}

fn readable_key(model: &Model, key: &CoordKey, value: f64) -> Cell {
    readable_cell(model, &decode_coord(key), value)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use improv_core_model::{
        BinaryOp, DimensionSpec, Expr, ItemId, Measure, MeasureKind, Name, Value, ValueType,
    };
    use improv_core_model::{CategoryId, Coordinate};
    use tower::util::ServiceExt; // for `oneshot`

    // Canonical Time x Product revenue model, matching the engine's fixture.
    fn revenue_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(11), time, "2026");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(21), product, "WidgetB");

        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Derived(improv_core_model::Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });

        let coord = |pairs: &[(CategoryId, ItemId)]| Coordinate::from_pairs(pairs.iter().copied());
        m.set_input(
            MeasureId(100),
            coord(&[(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m.set_input(
            MeasureId(100),
            coord(&[(product, ItemId(21))]),
            Value::Number(20.0),
        );
        m.set_input(
            MeasureId(101),
            coord(&[(time, ItemId(10)), (product, ItemId(20))]),
            Value::Number(100.0),
        );
        m.set_input(
            MeasureId(101),
            coord(&[(time, ItemId(10)), (product, ItemId(21))]),
            Value::Number(50.0),
        );
        m.set_input(
            MeasureId(101),
            coord(&[(time, ItemId(11)), (product, ItemId(20))]),
            Value::Number(120.0),
        );
        m.set_input(
            MeasureId(101),
            coord(&[(time, ItemId(11)), (product, ItemId(21))]),
            Value::Number(80.0),
        );
        m
    }

    // Seed a fresh on-disk store in a temp dir and return the router + path.
    fn seeded_app() -> (Router, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("improv-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!(
            "model-{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);
        let mut store = ModelStore::open(path.to_str().unwrap()).unwrap();
        store.save_model(&revenue_model()).unwrap();
        let state = Arc::new(AppState::new(
            path.to_str().unwrap().to_string(),
            Auth::Disabled,
            None,
        ));
        (app(state), path)
    }

    // Same seeded store, but auth enabled with a single accepted token.
    fn authed_app(token: &str) -> (Router, std::path::PathBuf) {
        let (_, path) = seeded_app();
        let state = Arc::new(AppState::new(
            path.to_str().unwrap().to_string(),
            Auth::Tokens {
                rw: HashSet::from([token.to_string()]),
                ro: HashSet::new(),
            },
            None,
        ));
        (app(state), path)
    }

    fn get_with_bearer(uri: &str, token: Option<&str>) -> Request<Body> {
        let mut b = Request::get(uri);
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        b.body(Body::empty()).unwrap()
    }

    async fn body_json(resp: Response) -> JsonValue {
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn health_ok() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"ok");
    }

    #[tokio::test]
    async fn lists_measures() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(Request::get("/measures").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        let arr = v.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        let revenue = arr.iter().find(|m| m["name"] == "Revenue").unwrap();
        assert_eq!(revenue["kind"], "derived");
        let price = arr.iter().find(|m| m["name"] == "Price").unwrap();
        assert_eq!(price["kind"], "input");
        assert_eq!(price["categories"], json!(["Product"]));
    }

    #[tokio::test]
    async fn derived_values_match_known_revenue() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::get("/measures/102/values")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        let cells = v.as_array().unwrap();
        assert_eq!(cells.len(), 4);
        // Collect values keyed by the readable coordinate.
        let mut got = std::collections::HashMap::new();
        for c in cells {
            let coord = serde_json::to_string(&c["coord"]).unwrap();
            got.insert(coord, c["value"].as_f64().unwrap());
        }
        // Revenue[2025,WidgetA] = 10*100 = 1000
        let key = |t: &str, p: &str| {
            serde_json::to_string(&json!([["Product", p], ["Time", t]])).unwrap()
        };
        assert_eq!(got[&key("2025", "WidgetA")], 1000.0);
        assert_eq!(got[&key("2025", "WidgetB")], 1000.0);
        assert_eq!(got[&key("2026", "WidgetA")], 1200.0);
        assert_eq!(got[&key("2026", "WidgetB")], 1600.0);
    }

    #[tokio::test]
    async fn input_values_returned() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::get("/measures/100/values")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cells = body_json(resp).await;
        assert_eq!(cells.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn unknown_measure_is_404() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::get("/measures/999/values")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn nl_parse_price_times_quantity() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::post("/nl/parse")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"text": "price times quantity"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        // Formula -> Expr::BinaryOp(Mul, Ref(100), Ref(101)).
        let op = &v["expr"]["BinaryOp"];
        assert_eq!(op[0], "Mul");
    }

    #[tokio::test]
    async fn nl_parse_unknown_measure_is_400() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::post("/nl/parse")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"text": "widgets times quantity"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn malformed_body_is_400_no_panic() {
        let (app, _p) = seeded_app();
        let resp = app
            .clone()
            .oneshot(
                Request::post("/nl/parse")
                    .header("content-type", "application/json")
                    .body(Body::from("{ not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        // Missing required field also 400.
        let resp = app
            .oneshot(
                Request::post("/nl/parse")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"nope": 1}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn nl_describe_round_trips() {
        let (app, _p) = seeded_app();
        // First parse to get a formula JSON, then describe it.
        let resp = app
            .clone()
            .oneshot(
                Request::post("/nl/parse")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"text": "price times quantity"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let formula = body_json(resp).await;
        let resp = app
            .oneshot(
                Request::post("/nl/describe")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"formula": formula}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["text"], "Price times Quantity");
    }

    /// Another process writes the store after the server cached its model: the
    /// server's next write must 409 (not retract that write), and a retry,
    /// which reloads, succeeds.
    #[tokio::test]
    async fn write_over_another_processes_change_is_409() {
        let (app, p) = seeded_app();
        let get = || Request::get("/measures").body(Body::empty()).unwrap();
        assert_eq!(
            app.clone().oneshot(get()).await.unwrap().status(),
            StatusCode::OK
        );

        let mut other = ModelStore::open(p.to_str().unwrap()).unwrap();
        let mut m = other.load_model().unwrap();
        let c = Coordinate::from_pairs([(improv_core_model::CategoryId(2), ItemId(21))]);
        m.set_input(
            MeasureId(100),
            c.clone(),
            improv_core_model::Value::Number(77.0),
        );
        other.save_model(&m).unwrap();

        let post = || {
            Request::post("/measures/100/cells")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"coord": {"dims": {"2": 20}}, "value": 42.0}).to_string(),
                ))
                .unwrap()
        };
        let resp = app.clone().oneshot(post()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert!(body_json(resp).await["error"]
            .as_str()
            .unwrap()
            .contains("reload"));
        assert_eq!(app.oneshot(post()).await.unwrap().status(), StatusCode::OK);
        let after = ModelStore::open(p.to_str().unwrap())
            .unwrap()
            .load_model()
            .unwrap();
        assert_eq!(
            after.input(MeasureId(100), &c),
            Some(&improv_core_model::Value::Number(77.0))
        );
    }

    #[tokio::test]
    async fn set_cell_persists() {
        let (app, _p) = seeded_app();
        let body = json!({
            "coord": {"dims": {"2": 20}},
            "value": 42.0
        });
        let resp = app
            .clone()
            .oneshot(
                Request::post("/measures/100/cells")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Re-read Price values; WidgetA should now be 42.
        let resp = app
            .oneshot(
                Request::get("/measures/100/values")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cells = body_json(resp).await;
        let has_42 = cells
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["value"].as_f64() == Some(42.0));
        assert!(has_42, "updated cell value should persist");
    }

    // Read Revenue[2025,WidgetA] from a /values or /eval response body.
    fn revenue_2025_widgeta(cells: &JsonValue) -> Option<f64> {
        let want = json!([["Product", "WidgetA"], ["Time", "2025"]]);
        cells
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["coord"] == want)
            .and_then(|c| c["value"].as_f64())
    }

    #[tokio::test]
    async fn eval_what_if_does_not_persist() {
        let (app, _p) = seeded_app();
        // Set Quantity[2025,WidgetA] (time cat 1/item 10, product cat 2/item 20)
        // from 100 to 200 -> Revenue there should be 10*200 = 2000 in response.
        let body = json!({
            "edits": [{"measure": 101, "coord": [[1, 10], [2, 20]], "value": 200.0}]
        });
        let resp = app
            .clone()
            .oneshot(
                Request::post("/measures/102/eval")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cells = body_json(resp).await;
        assert_eq!(revenue_2025_widgeta(&cells), Some(2000.0));

        // Without persist, the stored model is untouched: GET /values still 1000.
        let resp = app
            .oneshot(
                Request::get("/measures/102/values")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cells = body_json(resp).await;
        assert_eq!(revenue_2025_widgeta(&cells), Some(1000.0));
    }

    #[tokio::test]
    async fn eval_with_persist_writes_through() {
        let (app, _p) = seeded_app();
        let body = json!({
            "persist": true,
            "edits": [{"measure": 101, "coord": [[1, 10], [2, 20]], "value": 200.0}]
        });
        let resp = app
            .clone()
            .oneshot(
                Request::post("/measures/102/eval")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cells = body_json(resp).await;
        assert_eq!(revenue_2025_widgeta(&cells), Some(2000.0));

        // With persist, GET /values now reflects the change: 10*200 = 2000.
        let resp = app
            .oneshot(
                Request::get("/measures/102/values")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cells = body_json(resp).await;
        assert_eq!(revenue_2025_widgeta(&cells), Some(2000.0));
    }

    #[tokio::test]
    async fn eval_on_input_measure_is_400() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::post("/measures/101/eval")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"edits": []}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn eval_unknown_measure_is_404() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::post("/measures/999/eval")
                    .header("content-type", "application/json")
                    .body(Body::from(json!({"edits": []}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn eval_malformed_body_is_400_no_panic() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::post("/measures/102/eval")
                    .header("content-type", "application/json")
                    .body(Body::from("{ not json"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn authed_health_is_public() {
        let (app, _p) = authed_app("secret");
        let resp = app.oneshot(get_with_bearer("/health", None)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn authed_protected_route_requires_token() {
        let (app, _p) = authed_app("secret");
        // No header -> 401.
        let resp = app
            .clone()
            .oneshot(get_with_bearer("/measures", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        // Wrong token -> 403.
        let resp = app
            .clone()
            .oneshot(get_with_bearer("/measures", Some("nope")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        // Correct token -> 200.
        let resp = app
            .oneshot(get_with_bearer("/measures", Some("secret")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn scheduler_status_is_protected_and_reports_disabled() {
        // Disabled scheduler (test apps carry `scheduler_status: None`).
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(
                Request::get("/scheduler/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["enabled"], false);

        // Auth still gates it like any other protected route.
        let (app, _p) = authed_app("secret");
        let resp = app
            .oneshot(get_with_bearer("/scheduler/status", None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    fn post_json(uri: &str, body: String) -> Request<Body> {
        Request::post(uri)
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    #[tokio::test]
    async fn over_limit_body_is_413() {
        let (app, _p) = seeded_app();
        let big = format!("{{\"text\":\"{}\"}}", "x".repeat(MAX_BODY_BYTES + 1));
        let resp = app.oneshot(post_json("/nl/parse", big)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn too_many_edits_is_400() {
        let (app, _p) = seeded_app();
        let edit = json!({"measure": 101, "coord": [[1, 10], [2, 20]], "value": 1.0});
        let body = json!({ "edits": vec![edit; MAX_EDITS + 1] }).to_string();
        assert!(
            body.len() < MAX_BODY_BYTES,
            "must hit the edit cap, not the body cap"
        );
        let resp = app
            .oneshot(post_json("/measures/102/eval", body))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        assert!(body_json(resp).await["error"]
            .as_str()
            .unwrap()
            .contains("too many edits"));
    }

    // Slow-loris: a partial request must be dropped once the header timeout
    // passes, not held open forever. Returns the time until the server closes.
    async fn partial_request_closed_after(header_timeout: Duration) -> Duration {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (app, _p) = seeded_app();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, app, header_timeout));
        let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
        c.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n")
            .await
            .unwrap();
        let start = std::time::Instant::now();
        let mut buf = Vec::new();
        // EOF (or a reset) is the server hanging up; cap the wait well past it.
        let _ = tokio::time::timeout(header_timeout * 3, c.read_to_end(&mut buf))
            .await
            .expect("server should close a stalled connection");
        server.abort();
        start.elapsed()
    }

    #[tokio::test]
    async fn slow_loris_closed_fast_timeout() {
        let t = partial_request_closed_after(Duration::from_millis(200)).await;
        assert!(t < Duration::from_secs(2), "closed after {t:?}");
    }

    #[tokio::test]
    #[ignore = "takes ~10 s (default header timeout)"]
    async fn slow_loris_closed_default_timeout() {
        let t = partial_request_closed_after(HEADER_READ_TIMEOUT).await;
        assert!(t < Duration::from_secs(11), "closed after {t:?}");
    }

    #[tokio::test]
    async fn internal_error_hides_store_path() {
        let state = Arc::new(AppState::new(
            "/nonexistent-improv-dir/secret-store.db".into(),
            Auth::Disabled,
            None,
        ));
        let resp = app(state)
            .oneshot(Request::get("/measures").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains("nonexistent-improv-dir"),
            "path leaked: {text}"
        );
        let v: JsonValue = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"], "internal error");
        assert!(v["request_id"].as_u64().is_some());
    }

    #[test]
    fn ct_eq_and_token_matches() {
        assert!(ct_eq(b"secret", b"secret"));
        assert!(!ct_eq(b"secret", b"secreT"));
        assert!(!ct_eq(b"secret", b"secret2"));
        assert!(!ct_eq(b"", b"x"));
        let toks = HashSet::from(["a1".to_string(), "b2".to_string()]);
        assert!(token_matches(&toks, "b2"));
        assert!(!token_matches(&toks, "b"));
    }

    #[test]
    fn startup_check_refuses_open_non_loopback() {
        let open = Auth::Disabled;
        let toks = Auth::Tokens {
            rw: HashSet::from(["t".to_string()]),
            ro: HashSet::new(),
        };
        let lo: SocketAddr = "127.0.0.1:3000".parse().unwrap();
        let lo6: SocketAddr = "[::1]:3000".parse().unwrap();
        let any: SocketAddr = "0.0.0.0:3000".parse().unwrap();
        assert!(startup_check(lo, &open, None).is_ok());
        assert!(startup_check(lo6, &open, None).is_ok());
        assert!(startup_check(any, &open, None).is_err());
        assert!(startup_check(any, &open, Some("0")).is_err());
        assert!(startup_check(any, &open, Some("1")).is_ok());
        assert!(startup_check(any, &toks, None).is_ok());
    }

    fn post_cell(uri: &str, body: JsonValue, token: Option<&str>) -> Request<Body> {
        let mut b = Request::post(uri).header("content-type", "application/json");
        if let Some(t) = token {
            b = b.header("authorization", format!("Bearer {t}"));
        }
        b.body(Body::from(body.to_string())).unwrap()
    }

    #[tokio::test]
    async fn write_to_derived_or_unknown_measure_is_rejected() {
        let (app, _p) = seeded_app();
        let cell = json!({"coord": {"dims": {"1": 10, "2": 20}}, "value": 1.0});
        let resp = app
            .clone()
            .oneshot(post_cell("/measures/102/cells", cell.clone(), None));
        assert_eq!(resp.await.unwrap().status(), StatusCode::BAD_REQUEST);
        let resp = app
            .clone()
            .oneshot(post_cell("/measures/999/cells", cell, None));
        assert_eq!(resp.await.unwrap().status(), StatusCode::NOT_FOUND);
        // /eval edits must target an existing input measure too.
        for m in [102, 999] {
            let body = json!({"persist": true,
                "edits": [{"measure": m, "coord": [[1, 10], [2, 20]], "value": 1.0}]});
            let resp = app
                .clone()
                .oneshot(post_cell("/measures/102/eval", body, None));
            assert_eq!(
                resp.await.unwrap().status(),
                StatusCode::BAD_REQUEST,
                "measure {m}"
            );
        }
    }

    #[tokio::test]
    async fn write_with_bad_coordinate_is_400() {
        let (app, _p) = seeded_app();
        // Price is dimensioned by Product (cat 2) only.
        for dims in [
            json!({"1": 10}),          // wrong category
            json!({"2": 10}),          // item 10 belongs to Time, not Product
            json!({"2": 20, "1": 10}), // extra dimension
            json!({"2": 99}),          // unknown item
            json!({}),                 // missing dimension
        ] {
            let body = json!({"coord": {"dims": dims}, "value": 1.0});
            let resp = app
                .clone()
                .oneshot(post_cell("/measures/100/cells", body, None));
            assert_eq!(
                resp.await.unwrap().status(),
                StatusCode::BAD_REQUEST,
                "{dims}"
            );
        }
        for coord in [json!([[2, 20]]), json!([[1, 10], [1, 11], [2, 20]])] {
            let body = json!({"edits": [{"measure": 101, "coord": coord, "value": 1.0}]});
            let resp = app
                .clone()
                .oneshot(post_cell("/measures/102/eval", body, None));
            assert_eq!(
                resp.await.unwrap().status(),
                StatusCode::BAD_REQUEST,
                "{coord}"
            );
        }
    }

    #[test]
    fn validate_write_rejects_non_finite() {
        let m = revenue_model();
        let c = Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
        assert!(validate_write(&m, MeasureId(100), &c, Some(1.0)).is_ok());
        assert!(validate_write(&m, MeasureId(100), &c, None).is_ok());
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(validate_write(&m, MeasureId(100), &c, Some(v)).is_err());
        }
    }

    #[tokio::test]
    async fn read_only_token_can_read_not_write() {
        let (_, path) = seeded_app();
        let auth = Auth::Tokens {
            rw: HashSet::from(["w".to_string()]),
            ro: HashSet::from(["r".to_string()]),
        };
        let app = app(Arc::new(AppState::new(
            path.to_str().unwrap().to_string(),
            auth,
            None,
        )));
        let resp = app.clone().oneshot(get_with_bearer("/measures", Some("r")));
        assert_eq!(resp.await.unwrap().status(), StatusCode::OK);
        let cell = json!({"coord": {"dims": {"2": 20}}, "value": 5.0});
        let resp = app
            .clone()
            .oneshot(post_cell("/measures/100/cells", cell.clone(), Some("r")));
        assert_eq!(resp.await.unwrap().status(), StatusCode::FORBIDDEN);
        let edit = json!([{"measure": 101, "coord": [[1, 10], [2, 20]], "value": 1.0}]);
        let persist = json!({"persist": true, "edits": edit});
        let resp = app
            .clone()
            .oneshot(post_cell("/measures/102/eval", persist, Some("r")));
        assert_eq!(resp.await.unwrap().status(), StatusCode::FORBIDDEN);
        let what_if = json!({"edits": edit});
        let resp = app
            .clone()
            .oneshot(post_cell("/measures/102/eval", what_if, Some("r")));
        assert_eq!(resp.await.unwrap().status(), StatusCode::OK);
        let resp = app.oneshot(post_cell("/measures/100/cells", cell, Some("w")));
        assert_eq!(resp.await.unwrap().status(), StatusCode::OK);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writes_do_not_lose_updates() {
        let (app, _p) = seeded_app();
        let mut tasks = Vec::new();
        for (t, p) in [(10, 20), (10, 21), (11, 20), (11, 21)] {
            let app = app.clone();
            tasks.push(tokio::spawn(async move {
                let body = json!({"coord": {"dims": {"1": t, "2": p}}, "value": 1000.0 + f64::from(t * 100 + p)});
                app.oneshot(post_cell("/measures/101/cells", body, None)).await.unwrap().status()
            }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap(), StatusCode::OK);
        }
        let resp = app.oneshot(
            Request::get("/measures/101/values")
                .body(Body::empty())
                .unwrap(),
        );
        let cells = body_json(resp.await.unwrap()).await;
        let mut vals: Vec<f64> = cells
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["value"].as_f64().unwrap())
            .collect();
        vals.sort_by(f64::total_cmp);
        assert_eq!(
            vals,
            vec![2020.0, 2021.0, 2120.0, 2121.0],
            "an update was lost"
        );
    }

    #[tokio::test]
    async fn reads_share_one_load_until_a_write() {
        let (_, path) = seeded_app();
        let state = Arc::new(AppState::new(
            path.to_str().unwrap().into(),
            Auth::Disabled,
            None,
        ));
        let app = app(state.clone());
        let get = |uri: &str| Request::get(uri).body(Body::empty()).unwrap();
        for uri in ["/measures", "/measures/102/values"] {
            let resp = app.clone().oneshot(get(uri)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }
        assert_eq!(state.loads.load(Ordering::Relaxed), 1, "two GETs, one load");

        // A write replaces the cache: the next read sees it without reloading.
        let body = json!({"coord": {"dims": {"2": 20}}, "value": 42.0});
        let resp = app
            .clone()
            .oneshot(post_cell("/measures/100/cells", body, None));
        assert_eq!(resp.await.unwrap().status(), StatusCode::OK);
        let resp = app.oneshot(get("/measures/100/values")).await.unwrap();
        let cells = body_json(resp).await;
        assert!(cells.as_array().unwrap().iter().any(|c| c["value"] == 42.0));
        assert_eq!(state.loads.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn auth_from_env_reads_rw_and_ro_tokens() {
        // The only test touching these vars (`main` is their only other reader).
        std::env::set_var("IMPROV_API_TOKEN", " solo ");
        std::env::set_var("IMPROV_API_TOKENS", "a, b,,");
        std::env::set_var("IMPROV_API_TOKENS_RO", "r");
        let Auth::Tokens { rw, ro } = Auth::from_env() else {
            panic!("tokens are set");
        };
        assert_eq!(rw, HashSet::from(["solo", "a", "b"].map(String::from)));
        assert_eq!(ro, HashSet::from(["r".to_string()]));

        // A blank single token is ignored; read-only tokens alone still enable auth.
        std::env::set_var("IMPROV_API_TOKEN", "  ");
        std::env::remove_var("IMPROV_API_TOKENS");
        let Auth::Tokens { rw, ro } = Auth::from_env() else {
            panic!("a read-only token is set");
        };
        assert!(rw.is_empty());
        assert_eq!(ro.len(), 1);

        std::env::remove_var("IMPROV_API_TOKEN");
        std::env::remove_var("IMPROV_API_TOKENS_RO");
        assert!(matches!(Auth::from_env(), Auth::Disabled));
    }

    // A request still waiting when REQUEST_TIMEOUT elapses gets 408 (S10).
    #[tokio::test(start_paused = true)]
    async fn request_past_the_deadline_is_408() {
        let state = Arc::new(AppState::new("unused.db".into(), Auth::Disabled, None));
        let _held = state.write_lock.lock().await;
        let body = json!({"coord": {"dims": {"2": 20}}, "value": 1.0});
        let resp = app(state.clone())
            .oneshot(post_cell("/measures/100/cells", body, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
    }

    #[tokio::test]
    async fn scheduler_status_reports_a_failing_tick() {
        let status: SharedStatus = Arc::new(std::sync::Mutex::new(scheduler::SchedulerStatus {
            tick_secs: 1,
            ..Default::default()
        }));
        let state = Arc::new(AppState::new(
            "/nonexistent-improv-dir/sched.db".into(),
            Auth::Disabled,
            Some(status.clone()),
        ));
        let handle = scheduler::spawn_scheduler(
            state.clone(),
            Duration::from_millis(10),
            status.clone(),
            Default::default(),
        );
        for _ in 0..500 {
            if status.lock().unwrap().last_error.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        handle.abort();
        let resp = app(state)
            .oneshot(
                Request::get("/scheduler/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(resp).await;
        assert_eq!(v["enabled"], true);
        assert_eq!(v["tick_secs"], 1);
        assert!(v["last_error"].is_string(), "{v}");
    }

    #[tokio::test]
    async fn get_model_returns_the_stored_model() {
        let (app, _p) = seeded_app();
        let resp = app
            .oneshot(Request::get("/model").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            body_json(resp).await["measures"].as_object().unwrap().len(),
            3
        );
    }

    // A formula naming a missing measure is a 500, not a panic; a cell whose
    // coordinate names a missing category/item renders a placeholder name.
    #[tokio::test]
    async fn unresolvable_model_parts_are_reported_not_panicked() {
        let mut m = revenue_model();
        m.add_measure(Measure {
            id: MeasureId(103),
            name: Name("Broken".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(1)],
            kind: MeasureKind::Derived(improv_core_model::Formula::new(Expr::Ref(
                MeasureId(999),
                DimensionSpec::default(),
            ))),
            description: None,
        });
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(CategoryId(9), ItemId(99))]),
            Value::Number(1.0),
        );
        let (app, path) = seeded_app();
        ModelStore::open(path.to_str().unwrap())
            .unwrap()
            .save_model(&m)
            .unwrap();

        let get = |uri: &str| Request::get(uri).body(Body::empty()).unwrap();
        let resp = app
            .clone()
            .oneshot(get("/measures/103/values"))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let resp = app
            .clone()
            .oneshot(post_json("/measures/102/eval", "{}".into()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let resp = app
            .clone()
            .oneshot(get("/measures/100/values"))
            .await
            .unwrap();
        let cells = body_json(resp).await;
        assert!(
            cells
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["coord"] == json!([["category 9", "item 99"]])),
            "{cells}"
        );

        let resp = app
            .oneshot(post_json("/nl/describe", json!({"formula": 5}).to_string()))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
