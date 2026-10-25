//! `improv-server`: a small JSON HTTP API over an Improv model store.
//!
//! The (blocking SQLite-backed) Mentat store is opened inside `spawn_blocking`
//! and never shared across tasks. The loaded model is cached in [`AppState`]
//! and replaced on every write this process makes (API or scheduler). Each
//! read first checks the store's high-water mark (one cheap query on a kept
//! probe handle) and reloads when another process has written since, so reads
//! cost one store load per write, from any writer, not per request.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

// Declarations only (the clap derive).
#[cfg_attr(coverage_nightly, coverage(off))]
mod args;

use std::collections::HashMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::Duration;

use axum::{
    error_handling::HandleErrorLayer,
    extract::{rejection::JsonRejection, ConnectInfo, DefaultBodyLimit, Path, Request, State},
    http::{header, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post, MethodRouter},
    Extension, Json, Router,
};
use improv_core_model::{Coordinate, MeasureId, MeasureKind, Model};
use improv_engine::dataflow::evaluate;
use improv_engine::session::{Edit, Engine};
use improv_engine::{decode_coord, CellValue, CoordKey};
use improv_nl_formula::{describe_formula, parse_nl_formula, NlContext};
use improv_store::{Store, StoreError};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use tower::ServiceBuilder;

mod scheduler;
use scheduler::SharedStatus;

/// What a route needs. A `Read` token may call `Read` routes, a `Write`
/// token every route; `Public` routes need no token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    Public,
    Read,
    Write,
}

/// One accepted bearer token. `name` identifies the caller in the audit log
/// and the rate limiter; the secret is never logged.
#[derive(Clone)]
pub struct Token {
    pub name: String,
    pub scope: Scope,
    pub secret: String,
}

/// Accepted bearer tokens, or open mode.
///
/// `Disabled` = no auth (local dev / tests); `Tokens` = every non-public
/// route needs `Authorization: Bearer <secret>` naming one of them (S12).
#[derive(Clone)]
pub enum Auth {
    Disabled,
    Tokens(Vec<Token>),
}

impl Auth {
    /// Build from the environment (`get` reads a variable): `IMPROV_API_TOKEN`
    /// and `IMPROV_API_TOKENS` (comma-separated) are write tokens,
    /// `IMPROV_API_TOKENS_RO` read tokens, and `IMPROV_API_TOKEN_FILE` names a
    /// file of `name:scope:secret` lines. Blank tokens are ignored; no tokens
    /// => `Disabled`. An unreadable or malformed file, or a name or secret
    /// given twice, is an error.
    fn from_lookup(get: &impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut tokens = Vec::new();
        if let Some(secret) = get("IMPROV_API_TOKEN").filter(|t| !t.trim().is_empty()) {
            tokens.push(Token {
                name: "IMPROV_API_TOKEN".into(),
                scope: Scope::Write,
                secret: secret.trim().into(),
            });
        }
        for (var, scope) in [
            ("IMPROV_API_TOKENS", Scope::Write),
            ("IMPROV_API_TOKENS_RO", Scope::Read),
        ] {
            let value = get(var).unwrap_or_default();
            let secrets = value.split(',').map(str::trim).filter(|t| !t.is_empty());
            for (i, secret) in secrets.enumerate() {
                tokens.push(Token {
                    name: format!("{var}#{}", i + 1),
                    scope,
                    secret: secret.into(),
                });
            }
        }
        if let Some(path) = get("IMPROV_API_TOKEN_FILE") {
            let err = |e: String| format!("IMPROV_API_TOKEN_FILE {path}: {e}");
            let text = std::fs::read_to_string(&path).map_err(|e| err(e.to_string()))?;
            tokens.extend(parse_token_file(&text).map_err(err)?);
            #[cfg(unix)]
            if let Ok(meta) = std::fs::metadata(&path) {
                use std::os::unix::fs::PermissionsExt;
                if meta.permissions().mode() & 0o077 != 0 {
                    eprintln!("warning: {path} is readable by other users; chmod 600 it");
                }
            }
        }
        for (i, t) in tokens.iter().enumerate() {
            if let Some(dup) = tokens[..i]
                .iter()
                .find(|u| u.name == t.name || u.secret == t.secret)
            {
                return Err(format!(
                    "tokens {} and {} repeat a name or a secret",
                    dup.name, t.name
                ));
            }
        }
        Ok(if tokens.is_empty() {
            Auth::Disabled
        } else {
            Auth::Tokens(tokens)
        })
    }
}

/// Parse a token file: one `name:scope:secret` per line, `scope` being `read`
/// or `write` (the secret may itself contain `:`); `#` comments and blank
/// lines are skipped.
fn parse_token_file(text: &str) -> Result<Vec<Token>, String> {
    let lines = text.lines().enumerate().map(|(n, l)| (n + 1, l.trim()));
    lines
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
        .map(|(n, line)| {
            let mut f = line.splitn(3, ':').map(str::trim);
            let (name, scope, secret) = (f.next(), f.next(), f.next());
            let scope = match scope {
                Some("read") => Scope::Read,
                Some("write") => Scope::Write,
                _ => return Err(format!("line {n}: want name:read|write:secret")),
            };
            match (name, secret) {
                (Some(name), Some(secret)) if !name.is_empty() && !secret.is_empty() => Ok(Token {
                    name: name.into(),
                    scope,
                    secret: secret.into(),
                }),
                _ => Err(format!("line {n}: want name:read|write:secret")),
            }
        })
        .collect()
}

/// The authenticated caller, a request extension set by [`require_token`].
#[derive(Clone)]
struct Caller {
    name: String,
    scope: Scope,
}

/// Request extension set by [`audit`]: marking it (with the caller's name)
/// makes the request a write the audit log records. Shared, so a mark made
/// before a timeout drops the handler still counts.
#[derive(Clone, Default)]
struct Audited(Arc<std::sync::Mutex<Option<String>>>);

impl Audited {
    fn mark(&self, who: &str) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(who.to_string());
    }

    fn who(&self) -> Option<String> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Request body cap (S13), default. Over-limit bodies get 413.
const MAX_BODY_BYTES: usize = 1 << 20;
/// Edits accepted per `/eval` request (S13). More gets 400.
const MAX_EDITS: usize = 10_000;
/// Whole-request deadline, including time queued for a concurrency slot (S10).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Requests in flight across all routes (S10); the rest wait (then time out).
const MAX_IN_FLIGHT: usize = 64;
/// Default deadline for a client to finish sending request headers (S10).
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(10);
/// Default requests per minute per caller (token name, or client IP).
const RATE_PER_MIN: u32 = 600;
/// Callers the rate limiter tracks before it forgets idle ones.
const MAX_BUCKETS: usize = 10_000;

/// Per-caller token bucket: `per_min` requests a minute, refilled
/// continuously, all of which may come at once. `per_min == 0` disables it.
pub struct RateLimiter {
    per_min: u32,
    buckets: std::sync::Mutex<HashMap<String, (f64, tokio::time::Instant)>>,
}

impl RateLimiter {
    pub fn new(per_min: u32) -> Self {
        RateLimiter {
            per_min,
            buckets: Default::default(),
        }
    }

    /// Take one request from `key`'s bucket, or return the wait until one is free.
    fn check(&self, key: &str) -> Result<(), Duration> {
        if self.per_min == 0 {
            return Ok(());
        }
        let cap = f64::from(self.per_min);
        let per_sec = cap / 60.0;
        let now = tokio::time::Instant::now();
        let mut buckets = self.buckets.lock().unwrap_or_else(PoisonError::into_inner);
        if buckets.len() >= MAX_BUCKETS && !buckets.contains_key(key) {
            // A bucket idle for a minute is full again: forgetting it is lossless.
            buckets.retain(|_, (_, t)| now - *t < Duration::from_secs(60));
            // shortcut: under a flood of distinct callers, start over; a
            // proxy-side limit is the upgrade if that matters.
            if buckets.len() >= MAX_BUCKETS {
                buckets.clear();
            }
        }
        let (tokens, last) = buckets.entry(key.to_string()).or_insert((cap, now));
        *tokens = (*tokens + (now - *last).as_secs_f64() * per_sec).min(cap);
        *last = now;
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            Ok(())
        } else {
            Err(Duration::from_secs_f64((1.0 - *tokens) / per_sec))
        }
    }
}

/// Where the write audit log goes: stderr, or appended to a file. One JSON
/// object per line.
pub enum AuditLog {
    Stderr,
    File(std::sync::Mutex<std::fs::File>),
}

impl AuditLog {
    /// Append to `path`, or stderr for `None`.
    pub fn open(path: Option<&str>) -> Result<Self, String> {
        let Some(path) = path else {
            return Ok(AuditLog::Stderr);
        };
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map(|f| AuditLog::File(std::sync::Mutex::new(f)))
            .map_err(|e| format!("IMPROV_AUDIT_LOG {path}: {e}"))
    }

    fn record(&self, entry: &JsonValue) {
        let line = format!("{entry}\n");
        match self {
            AuditLog::Stderr => eprint!("{line}"),
            AuditLog::File(f) => {
                let mut f = f.lock().unwrap_or_else(PoisonError::into_inner);
                if let Err(e) = f.write_all(line.as_bytes()) {
                    eprint!("audit log write failed ({e}): {line}");
                }
            }
        }
    }
}

/// The model as the audit log names it: the path, or the URL without a
/// password (`user:pw@` or a `password=` parameter).
fn model_label(db: &str) -> String {
    let Some((scheme, rest)) = db.split_once("://") else {
        return db.to_string();
    };
    let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
    let authority = match authority.rsplit_once('@') {
        Some((user, host)) => format!("{}@{host}", user.split(':').next().unwrap_or("")),
        None => authority.to_string(),
    };
    let query: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty() && !p.starts_with("password="))
        .collect();
    let query = match query.is_empty() {
        true => String::new(),
        false => format!("?{}", query.join("&")),
    };
    format!("{scheme}://{authority}{path}{query}")
}

/// Read a number from the environment, or `default` when unset.
fn env_num<T: std::str::FromStr>(
    get: &impl Fn(&str) -> Option<String>,
    var: &str,
    default: T,
) -> Result<T, String> {
    match get(var) {
        None => Ok(default),
        Some(v) => v
            .trim()
            .parse()
            .map_err(|_| format!("{var}={v}: not a number")),
    }
}

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
    /// cache (another process wrote since) is refused with 409. Changed only
    /// with the `cache` write lock held, so a `cache` reader sees a matching pair.
    loaded_tx: std::sync::Mutex<Option<i64>>,
    /// A store handle kept open for the per-request high-water check (H9):
    /// opened on first use, dropped after an error so the next check reconnects.
    probe: std::sync::Mutex<Option<Store>>,
    /// Request body cap in bytes.
    max_body: usize,
    rate: RateLimiter,
    audit: AuditLog,
    /// `db_path` without credentials, for the audit log.
    model_label: String,
    /// `--static DIR`, canonical: files served at `/` beside the API.
    static_dir: Option<std::path::PathBuf>,
    /// `IMPROV_CORS_ORIGINS`: browser origins allowed to call the API from
    /// another origin. Empty (the default): no CORS headers at all.
    cors_origins: Vec<String>,
    /// Store loads performed, so tests can check the cache.
    #[cfg(test)]
    loads: AtomicU64,
}

impl AppState {
    pub fn new(db_path: String, auth: Auth, scheduler_status: Option<SharedStatus>) -> Self {
        AppState {
            model_label: model_label(&db_path),
            db_path,
            auth,
            scheduler_status,
            write_lock: tokio::sync::Mutex::new(()),
            cache: tokio::sync::RwLock::new(None),
            loaded_tx: std::sync::Mutex::new(None),
            probe: std::sync::Mutex::new(None),
            max_body: MAX_BODY_BYTES,
            rate: RateLimiter::new(RATE_PER_MIN),
            audit: AuditLog::Stderr,
            static_dir: None,
            cors_origins: Vec::new(),
            #[cfg(test)]
            loads: AtomicU64::new(0),
        }
    }

    /// Serve the files under `dir` at `/` (`--static`). Refused unless it is
    /// a readable directory.
    pub fn serve_static(&mut self, dir: &std::path::Path) -> Result<(), String> {
        let dir = dir
            .canonicalize()
            .map_err(|e| format!("--static {}: {e}", dir.display()))?;
        if !dir.is_dir() {
            return Err(format!("--static {}: not a directory", dir.display()));
        }
        self.static_dir = Some(dir);
        Ok(())
    }

    /// Apply `IMPROV_MAX_BODY_BYTES`, `IMPROV_RATE_LIMIT` (requests a minute
    /// per caller, 0 = off), `IMPROV_AUDIT_LOG` (a file; default stderr) and
    /// `IMPROV_CORS_ORIGINS` (comma-separated origins; default none).
    pub fn configure(&mut self, get: &impl Fn(&str) -> Option<String>) -> Result<(), String> {
        self.max_body = env_num(get, "IMPROV_MAX_BODY_BYTES", MAX_BODY_BYTES)?;
        self.rate = RateLimiter::new(env_num(get, "IMPROV_RATE_LIMIT", RATE_PER_MIN)?);
        self.audit = AuditLog::open(get("IMPROV_AUDIT_LOG").as_deref())?;
        self.cors_origins = cors_origins(get("IMPROV_CORS_ORIGINS").as_deref())?;
        Ok(())
    }

    /// Log one write: who, the route (`METHOD path`), the model, the status.
    fn audit_write(&self, who: &str, route: &str, status: u16) {
        self.audit.record(&json!({
            "ts": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "audit": "write",
            "who": who,
            "route": route,
            "model": self.model_label,
            "status": status,
        }));
    }
}

// Coverage exclusion: process entry point (argv, env, bind, `process::exit`);
// its parts (`Auth::from_lookup`, `configure`, `startup_check`, `serve`,
// `app`) are tested.
#[cfg_attr(coverage_nightly, coverage(off))]
#[tokio::main]
async fn main() {
    // The model (default improv.db), and the address: an argument, else
    // IMPROV_ADDR, else 127.0.0.1:3000.
    let args::Cli {
        model: db_path,
        addr,
        static_dir,
    } = <args::Cli as clap::Parser>::parse();

    let get = |k: &str| std::env::var(k).ok();
    let auth = Auth::from_lookup(&get).unwrap_or_else(|e| {
        eprintln!("improv-server: {e}");
        std::process::exit(2);
    });
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
    let mut state = AppState::new(db_path, auth, scheduler_status.clone());
    if let Err(e) = state
        .configure(&get)
        .and_then(|()| static_dir.map_or(Ok(()), |d| state.serve_static(&d)))
    {
        eprintln!("improv-server: {e}");
        std::process::exit(2);
    }
    let state = Arc::new(state);
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
    // SIGTERM (docker stop, systemd) and Ctrl-C end the server at once. A
    // request in flight is cut off; its save is one store transaction, so
    // it lands whole or not at all.
    tokio::select! {
        () = serve(listener, app(state), header_timeout) => {}
        () = shutdown_signal() => eprintln!("improv-server: shutting down"),
    }
}

/// SIGTERM or SIGINT (Ctrl-C; Ctrl-Break on Windows).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut term = signal(SignalKind::terminate()).expect("install a SIGTERM handler");
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
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
        let (stream, peer) = match listener.accept().await {
            Ok(v) => v,
            Err(e) => {
                // EMFILE etc.: back off instead of spinning.
                eprintln!("accept: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        // The peer address keys the rate limiter for callers without a token.
        let svc = tower::ServiceExt::map_request(
            app.clone(),
            move |mut req: axum::http::Request<hyper::body::Incoming>| {
                req.extensions_mut().insert(ConnectInfo(peer));
                req
            },
        );
        let svc = hyper_util::service::TowerToHyperService::new(svc);
        tokio::spawn(async move {
            let _ = hyper::server::conn::http1::Builder::new()
                .timer(TokioTimer::new())
                .header_read_timeout(header_timeout)
                .serve_connection(TokioIo::new(stream), svc)
                .await;
        });
    }
}

/// The API: method, path, the scope it needs, and the handler. Each route is
/// served at `/v1<path>` and, as an alias, at `<path>`. `docs/openapi.json`
/// documents exactly these (a test checks).
fn routes() -> Vec<(
    &'static str,
    &'static str,
    Scope,
    MethodRouter<Arc<AppState>>,
)> {
    vec![
        ("get", "/health", Scope::Public, get(health)),
        ("get", "/openapi.json", Scope::Public, get(openapi)),
        ("get", "/model", Scope::Read, get(get_model)),
        ("get", "/measures", Scope::Read, get(list_measures)),
        (
            "get",
            "/measures/:id/values",
            Scope::Read,
            get(measure_values),
        ),
        // A what-if read; `"persist": true` needs a write token (checked in the handler).
        (
            "post",
            "/measures/:id/eval",
            Scope::Read,
            post(eval_measure),
        ),
        ("post", "/measures/:id/cells", Scope::Write, post(set_cell)),
        ("post", "/nl/parse", Scope::Read, post(nl_parse)),
        ("post", "/nl/describe", Scope::Read, post(nl_describe)),
        (
            "get",
            "/scheduler/status",
            Scope::Read,
            get(scheduler_status_handler),
        ),
    ]
}

/// The OpenAPI document for [`routes`].
const OPENAPI: &str = include_str!("../../../docs/openapi.json");

/// Build the router. Exposed so tests can drive it via `oneshot`.
pub fn app(state: Arc<AppState>) -> Router {
    let mut router = Router::new();
    for (_, path, scope, handler) in routes() {
        let handler = match scope {
            Scope::Public => handler,
            _ => handler.route_layer(middleware::from_fn_with_state(
                (state.clone(), scope),
                require_token,
            )),
        };
        router = router
            .route(&format!("/v1{path}"), handler.clone())
            .route(path, handler);
    }

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

    if state.static_dir.is_some() {
        router = router.fallback(get(static_file));
    }
    let router = router
        .layer(limits)
        // Outside the timeout, so a write that times out is audited too.
        .layer(middleware::from_fn_with_state(state.clone(), audit))
        .layer(DefaultBodyLimit::max(state.max_body));
    // Outermost, so a preflight is answered before auth and limits.
    let router = if state.cors_origins.is_empty() {
        router
    } else {
        router.layer(middleware::from_fn_with_state(state.clone(), cors))
    };
    router.with_state(state)
}

/// `IMPROV_CORS_ORIGINS`: comma-separated origins (`https://host[:port]`),
/// matched exactly. `*` is refused: tokens ride in headers, so a wildcard
/// would let any page call the API with a visitor's token.
fn cors_origins(v: Option<&str>) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for o in v
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|o| !o.is_empty())
    {
        let o = o.trim_end_matches('/');
        let ok = (o.starts_with("http://") || o.starts_with("https://"))
            && !o["https://".len().min(o.len())..].contains('/');
        if o == "*" || !ok {
            return Err(format!(
                "IMPROV_CORS_ORIGINS: {o:?} is not an origin (https://host[:port]); \
                 \"*\" is not allowed"
            ));
        }
        out.push(o.to_owned());
    }
    Ok(out)
}

/// CORS for the origins in `IMPROV_CORS_ORIGINS` (only installed when there
/// are some). A preflight (`OPTIONS` with `Access-Control-Request-Method`)
/// from an allowed origin is answered here with 204; any other request
/// from one gets `Access-Control-Allow-Origin` on its response. Requests
/// from other origins get no CORS headers, so the browser blocks them.
async fn cors(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .filter(|o| state.cors_origins.iter().any(|a| a == o))
        .map(str::to_owned);
    let preflight = req.method() == axum::http::Method::OPTIONS
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD);
    let mut resp = match (&origin, preflight) {
        (Some(_), true) => {
            let mut r = StatusCode::NO_CONTENT.into_response();
            let h = r.headers_mut();
            h.insert(
                header::ACCESS_CONTROL_ALLOW_METHODS,
                header::HeaderValue::from_static("GET, POST, OPTIONS"),
            );
            h.insert(
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                header::HeaderValue::from_static("authorization, content-type"),
            );
            h.insert(
                header::ACCESS_CONTROL_MAX_AGE,
                header::HeaderValue::from_static("600"),
            );
            r
        }
        _ => next.run(req).await,
    };
    let h = resp.headers_mut();
    h.append(header::VARY, header::HeaderValue::from_static("origin"));
    if let Some(o) = origin.and_then(|o| header::HeaderValue::from_str(&o).ok()) {
        h.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
    }
    resp
}

/// `--static`: a GET for a path the API does not route reads the file under
/// the static directory (`/` and directories are their `index.html`).
/// Paths that leave the directory (`..`, symlinks out) are 404, as are
/// missing files. Public, like `/health`: a web front end's files hold no
/// model data; the API calls it makes still need their tokens.
async fn static_file(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let not_found = || (StatusCode::NOT_FOUND, "not found").into_response();
    let Some(root) = state.static_dir.clone() else {
        return not_found();
    };
    let rel = req.uri().path().trim_start_matches('/').to_owned();
    let file = tokio::task::spawn_blocking(move || {
        // Percent-encoded names are not decoded: a front end's build has
        // plain ASCII file names, and a decoded `%2e%2e` cannot sneak in.
        let mut p = root.join(&rel);
        if p.is_dir() {
            p = p.join("index.html");
        }
        let p = p.canonicalize().ok().filter(|p| p.starts_with(&root))?;
        std::fs::read(&p).ok().map(|bytes| (p, bytes))
    })
    .await
    .ok()
    .flatten();
    let Some((path, bytes)) = file else {
        return not_found();
    };
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    ([(header::CONTENT_TYPE, content_type(ext))], bytes).into_response()
}

/// The content type for a file extension, for what a web build holds.
fn content_type(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "wasm" => "application/wasm",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        _ => "application/octet-stream",
    }
}

/// Write audit: a handler (or [`require_token`]) marks a request as a write
/// with [`Audited::mark`]; this logs it with the final status.
async fn audit(State(state): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let audited = Audited::default();
    req.extensions_mut().insert(audited.clone());
    let route = format!("{} {}", req.method(), req.uri().path());
    let resp = next.run(req).await;
    if let Some(who) = audited.who() {
        state.audit_write(&who, &route, resp.status().as_u16());
    }
    resp
}

/// Bearer-token gate for a route needing `scope`. A missing or unknown token
/// gets 401; a read token on a write route 403. The caller (the token's name,
/// or `anonymous` in open mode) is passed on as a [`Caller`] extension and
/// is rate limited; a request without a valid token is limited by client
/// address. The secret is never logged.
async fn require_token(
    State((state, scope)): State<(Arc<AppState>, Scope)>,
    mut req: Request,
    next: Next,
) -> Response {
    let caller = match &state.auth {
        Auth::Disabled => Ok(Caller {
            name: "anonymous".into(),
            scope: Scope::Write,
        }),
        Auth::Tokens(tokens) => req
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or("missing bearer token")
            .and_then(|t| find_token(tokens, t).ok_or("invalid bearer token"))
            .map(|t| Caller {
                name: t.name.clone(),
                scope: t.scope,
            }),
    };
    let key = match (&caller, &state.auth) {
        (Ok(c), Auth::Tokens(_)) => format!("token:{}", c.name),
        _ => match req.extensions().get::<ConnectInfo<SocketAddr>>() {
            Some(ConnectInfo(a)) => format!("ip:{}", a.ip()),
            None => "ip:unknown".into(),
        },
    };
    if scope == Scope::Write {
        // Refused attempts are audited too.
        let who = caller.as_ref().map_or("unauthenticated", |c| &c.name);
        if let Some(a) = req.extensions().get::<Audited>() {
            a.mark(who);
        }
    }
    if let Err(wait) = state.rate.check(&key) {
        let secs = wait.as_secs() + 1;
        let body = Json(json!({ "error": "rate limit exceeded" }));
        let retry = [(header::RETRY_AFTER, secs.to_string())];
        return (StatusCode::TOO_MANY_REQUESTS, retry, body).into_response();
    }
    let caller = match caller {
        Ok(c) => c,
        Err(msg) => {
            let challenge = [(header::WWW_AUTHENTICATE, "Bearer")];
            let body = Json(json!({ "error": msg }));
            return (StatusCode::UNAUTHORIZED, challenge, body).into_response();
        }
    };
    if scope == Scope::Write && caller.scope != Scope::Write {
        return ApiError::Forbidden("this route needs a write token".into()).into_response();
    }
    req.extensions_mut().insert(caller);
    next.run(req).await
}

/// The token whose secret is `presented`. Constant-time (S15): every token is
/// compared, and each comparison folds over all bytes, so timing reveals
/// neither which token matched nor the length of a matching prefix.
fn find_token<'a>(tokens: &'a [Token], presented: &str) -> Option<&'a Token> {
    tokens.iter().fold(None, |hit, t| {
        match ct_eq(t.secret.as_bytes(), presented.as_bytes()) {
            true => Some(t),
            false => hit,
        }
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

/// The store's high-water mark, read on the kept probe handle on a blocking
/// thread: Mentat's newest transaction, Postgres's `improv.model_version`.
// shortcut: one probe handle serializes the checks; pool them if reads contend.
async fn store_high_water(state: &Arc<AppState>) -> Result<i64, ApiError> {
    let st = state.clone();
    tokio::task::spawn_blocking(move || {
        let mut probe = st.probe.lock().unwrap_or_else(PoisonError::into_inner);
        if probe.is_none() {
            *probe = Some(Store::open(&st.db_path)?);
        }
        let hw = probe.as_mut().map(Store::tx_high_water).unwrap_or(Ok(0));
        if hw.is_err() {
            *probe = None;
        }
        hw
    })
    .await
    .map_err(|e| ApiError::Internal(format!("task join: {e}")))?
    .map_err(|e| ApiError::Internal(e.to_string()))
}

/// The current model: the cached copy while the store's high-water still
/// matches it, else one load from the store on a blocking thread (H9: another
/// process's write shows up on the next request). The cache write lock is held
/// across the load, so a concurrent writer's [`save_model`] replaces it only
/// after a stale load lands.
async fn load_model(state: &Arc<AppState>) -> Result<Arc<Model>, ApiError> {
    // `None`: nothing was cached, so any copy cached since is new enough.
    let cached = state.cache.read().await.is_some();
    let hw = match cached {
        true => Some(store_high_water(state).await?),
        false => None,
    };
    let fresh = |cache: &Option<Arc<Model>>| {
        let tx = *state.loaded_tx.lock().unwrap();
        cache.clone().filter(|_| hw.is_none() || hw == tx)
    };
    if let Some(m) = fresh(&*state.cache.read().await) {
        return Ok(m);
    }
    let mut cache = state.cache.write().await;
    if let Some(m) = fresh(&cache) {
        return Ok(m);
    }
    #[cfg(test)]
    state.loads.fetch_add(1, Ordering::Relaxed);
    let path = state.db_path.clone();
    let (model, tx) = tokio::task::spawn_blocking(move || {
        let mut store = Store::open(&path).map_err(|e| e.to_string())?;
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
        let mut store = Store::open(&path)?;
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
    let mut cache = state.cache.write().await;
    *state.loaded_tx.lock().unwrap() = tx;
    *cache = Some(model.clone());
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

async fn openapi() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], OPENAPI).into_response()
}

/// Scheduler observability: enabled/tick/last-refreshed. Protected (not a
/// liveness probe like `/health`) since it can reveal which measures exist.
async fn scheduler_status_handler(State(state): State<Arc<AppState>>) -> Json<JsonValue> {
    match &state.scheduler_status {
        None => Json(json!({ "enabled": false })),
        Some(status) => {
            let s = status.lock().unwrap_or_else(PoisonError::into_inner);
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
    Extension(caller): Extension<Caller>,
    Extension(audited): Extension<Audited>,
    Path(id): Path<u32>,
    body: Result<Json<EvalReq>, JsonRejection>,
) -> Result<Json<Vec<Cell>>, ApiError> {
    let Json(req) = body?;
    if req.persist {
        audited.mark(&caller.name);
        if caller.scope != Scope::Write {
            return Err(ApiError::Forbidden("a read token cannot persist".into()));
        }
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
            value: e.value.map(CellValue::num),
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
    Path(id): Path<u32>,
    body: Result<Json<SetCellReq>, JsonRejection>,
) -> Result<Json<JsonValue>, ApiError> {
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

    /// A fresh path in this process's temp dir. A counter, not the clock:
    /// parallel tests could read the same nanosecond.
    fn temp_path(tag: &str) -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("improv-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{tag}-{}", N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_file(&path);
        path
    }

    // Seed a fresh on-disk store in a temp dir and return the router + path.
    fn seeded_app() -> (Router, std::path::PathBuf) {
        let path = temp_path("model.db");
        let mut store = Store::open(path.to_str().unwrap()).unwrap();
        store.save_model(&revenue_model()).unwrap();
        let state = Arc::new(AppState::new(
            path.to_str().unwrap().to_string(),
            Auth::Disabled,
            None,
        ));
        (app(state), path)
    }

    fn token(name: &str, scope: Scope, secret: &str) -> Token {
        Token {
            name: name.into(),
            scope,
            secret: secret.into(),
        }
    }

    // Same seeded store, but auth enabled with a single write token.
    fn authed_app(secret: &str) -> (Router, std::path::PathBuf) {
        let (_, path) = seeded_app();
        let state = Arc::new(AppState::new(
            path.to_str().unwrap().to_string(),
            Auth::Tokens(vec![token("t", Scope::Write, secret)]),
            None,
        ));
        (app(state), path)
    }

    /// A seeded store with a write token `w` (secret `ws`) and a read token
    /// `r` (secret `rs`), state settings from `env`.
    fn scoped_state(env: &[(&str, &str)]) -> Arc<AppState> {
        let (_, path) = seeded_app();
        let auth = Auth::Tokens(vec![
            token("w", Scope::Write, "wsecret-9f3c"),
            token("r", Scope::Read, "rsecret-7a1d"),
        ]);
        let mut state = AppState::new(path.to_str().unwrap().into(), auth, None);
        state.configure(&lookup(env)).unwrap();
        Arc::new(state)
    }

    /// An environment lookup over fixed pairs (tests never touch the real one).
    fn lookup<'a>(env: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| {
            env.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string())
        }
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

    // -- the web front end's needs (I1): static files and CORS

    /// An app over the seeded model with `state` adjusted by `f`.
    fn app_with(f: impl FnOnce(&mut AppState)) -> (Router, std::path::PathBuf) {
        let path = temp_path("model.db");
        Store::open(path.to_str().unwrap())
            .unwrap()
            .save_model(&revenue_model())
            .unwrap();
        let mut state = AppState::new(
            path.to_str().unwrap().to_string(),
            Auth::Tokens(vec![token("w", Scope::Write, "s3cret")]),
            None,
        );
        f(&mut state);
        (app(Arc::new(state)), path)
    }

    async fn body_bytes(resp: Response) -> Vec<u8> {
        to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    /// `--static DIR` serves its files (index.html for `/` and a directory)
    /// with their types, public, beside the API; nothing outside DIR, and
    /// without the flag nothing at all.
    #[tokio::test]
    async fn static_files_are_served_beside_the_api() {
        let dir = temp_path("web");
        std::fs::create_dir_all(dir.join("pkg")).unwrap();
        std::fs::write(dir.join("index.html"), "<p>improv</p>").unwrap();
        std::fs::write(dir.join("pkg/app.wasm"), b"\0asm").unwrap();
        std::fs::write(dir.join("pkg/index.html"), "pkg").unwrap();
        let outside = temp_path("secret.txt");
        std::fs::write(&outside, "no").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, dir.join("link.txt")).unwrap();
        let (app, _p) = app_with(|s| s.serve_static(&dir).unwrap());
        let get = |uri: &str| {
            app.clone()
                .oneshot(Request::get(uri).body(Body::empty()).unwrap())
        };

        let r = get("/").await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(body_bytes(r).await, b"<p>improv</p>");
        let r = get("/pkg/app.wasm").await.unwrap();
        assert_eq!(r.headers()[header::CONTENT_TYPE], "application/wasm");
        assert_eq!(body_bytes(r).await, b"\0asm");
        assert_eq!(body_bytes(get("/pkg/").await.unwrap()).await, b"pkg");
        for bad in [
            "/nope.js",
            "/../secret.txt",
            "/pkg/../../secret.txt",
            "/link.txt",
        ] {
            let r = get(bad).await.unwrap();
            assert_eq!(r.status(), StatusCode::NOT_FOUND, "{bad}");
        }
        // The API still routes, and still wants its token.
        assert_eq!(get("/health").await.unwrap().status(), StatusCode::OK);
        assert_eq!(
            get("/v1/measures").await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
        // No flag: no files.
        let (plain, _p) = app_with(|_| ());
        let r = plain
            .oneshot(Request::get("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        // A bad directory is refused at startup.
        let mut s = AppState::new(String::new(), Auth::Disabled, None);
        assert!(s
            .serve_static(&outside)
            .unwrap_err()
            .contains("not a directory"));
        assert!(s.serve_static(&dir.join("missing")).is_err());
        assert_eq!(content_type("CSS"), "text/css; charset=utf-8");
        assert_eq!(content_type("bin"), "application/octet-stream");
        let _ = std::fs::remove_file(&outside);
    }

    /// CORS is off by default (no headers); with IMPROV_CORS_ORIGINS an
    /// allowed origin's preflight is answered and its responses carry
    /// Access-Control-Allow-Origin, and any other origin gets nothing.
    #[tokio::test]
    async fn cors_is_off_by_default_and_exact_when_configured() {
        let from = |origin: &str, method: &str, preflight: bool| {
            let mut b = Request::builder()
                .method(method)
                .uri("/v1/measures")
                .header(header::ORIGIN, origin);
            if preflight {
                b = b
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization");
            } else {
                b = b.header(header::AUTHORIZATION, "Bearer s3cret");
            }
            b.body(Body::empty()).unwrap()
        };
        let (off, _p) = app_with(|_| ());
        let r = off
            .oneshot(from("https://ui.example", "GET", false))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert!(!r
            .headers()
            .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));

        let get = |k: &str| {
            (k == "IMPROV_CORS_ORIGINS")
                .then(|| "https://ui.example, http://localhost:8080/".to_owned())
        };
        let (on, _p) = app_with(|s| s.configure(&get).unwrap());
        let r = on
            .clone()
            .oneshot(from("https://ui.example", "OPTIONS", true))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::NO_CONTENT, "preflight before auth");
        assert_eq!(
            r.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://ui.example"
        );
        assert!(r.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
            .to_str()
            .unwrap()
            .contains("authorization"));
        let r = on
            .clone()
            .oneshot(from("http://localhost:8080", "GET", false))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://localhost:8080"
        );
        assert_eq!(r.headers()[header::VARY], "origin");
        for other in [
            from("https://evil.example", "GET", false),
            from("https://evil.example", "OPTIONS", true),
        ] {
            let r = on.clone().oneshot(other).await.unwrap();
            assert!(!r
                .headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN));
            assert_ne!(r.status(), StatusCode::NO_CONTENT);
        }
        // Without the token the API still refuses an allowed origin.
        let mut req = from("https://ui.example", "GET", false);
        req.headers_mut().remove(header::AUTHORIZATION);
        let r = on.oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            r.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "https://ui.example"
        );
        // Bad values are refused.
        for bad in ["*", "ui.example", "https://ui.example/app", "ftp://x"] {
            assert!(cors_origins(Some(bad)).is_err(), "{bad}");
        }
        assert_eq!(cors_origins(None).unwrap(), Vec::<String>::new());
        assert_eq!(cors_origins(Some(" , ")).unwrap(), Vec::<String>::new());
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

    /// Another process (a second store handle) sets Price[WidgetB] = `v` in
    /// the store at `target`.
    fn other_process_sets_price_b(target: &str, v: f64) {
        let mut other = Store::open(target).unwrap();
        let mut m = other.load_model().unwrap();
        m.set_input(MeasureId(100), price_b(), Value::Number(v));
        other.save_model(&m).unwrap();
    }

    fn price_b() -> Coordinate {
        Coordinate::from_pairs([(CategoryId(2), ItemId(21))])
    }

    async fn get_price_b(app: &Router) -> Option<f64> {
        let req = Request::get("/measures/100/values").body(Body::empty());
        let resp = app.clone().oneshot(req.unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let cells = body_json(resp).await;
        let want = json!([["Product", "WidgetB"]]);
        let cell = cells
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["coord"] == want);
        cell.and_then(|c| c["value"].as_f64())
    }

    /// H9: another process saves after the server cached its model; the
    /// server's next GET shows the change, with one reload, and a GET after
    /// that reuses the reloaded copy.
    #[tokio::test]
    async fn outside_write_shows_in_the_next_get() {
        let (_, p) = seeded_app();
        let state = Arc::new(AppState::new(
            p.to_str().unwrap().into(),
            Auth::Disabled,
            None,
        ));
        let app = app(state.clone());
        assert_eq!(get_price_b(&app).await, Some(20.0));
        other_process_sets_price_b(p.to_str().unwrap(), 77.0);
        assert_eq!(get_price_b(&app).await, Some(77.0));
        assert_eq!(get_price_b(&app).await, Some(77.0));
        assert_eq!(state.loads.load(Ordering::Relaxed), 2, "one reload");
    }

    /// The same against the Postgres store (live: needs `IMPROV_TEST_PG`).
    /// A plain test with its own runtime: a `postgres::Client` owns a runtime
    /// and must not be dropped inside an async context.
    #[test]
    fn outside_write_shows_in_the_next_get_postgres() {
        let Some(url) = pg_url() else { return };
        let url = format!("{url}?model=server_h9_{}", std::process::id());
        Store::open(&url)
            .unwrap()
            .save_model(&revenue_model())
            .unwrap();
        let state = Arc::new(AppState::new(url.clone(), Auth::Disabled, None));
        let app = app(state.clone());
        let rt = tokio::runtime::Runtime::new().unwrap();
        assert_eq!(rt.block_on(get_price_b(&app)), Some(20.0));
        other_process_sets_price_b(&url, 77.0);
        assert_eq!(rt.block_on(get_price_b(&app)), Some(77.0));
        assert_eq!(rt.block_on(get_price_b(&app)), Some(77.0));
        assert_eq!(state.loads.load(Ordering::Relaxed), 2, "one reload");
    }

    /// `IMPROV_TEST_PG` (a URL or a `key=value` DSN) as a `postgres://` URL.
    fn pg_url() -> Option<String> {
        let Ok(dsn) = std::env::var("IMPROV_TEST_PG") else {
            println!("skipped: IMPROV_TEST_PG not set");
            return None;
        };
        if improv_store::is_postgres(&dsn) {
            return Some(dsn);
        }
        let kv: HashMap<_, _> = dsn
            .split_whitespace()
            .filter_map(|p| p.split_once('='))
            .collect();
        let get = |k| kv.get(k).copied().unwrap_or_default();
        Some(format!(
            "postgres://{}:{}@{}/{}",
            get("user"),
            get("password"),
            get("host"),
            get("dbname")
        ))
    }

    /// A write after another process's change starts from that change (the
    /// server reloads first), so it keeps it.
    #[tokio::test]
    async fn write_after_another_processes_change_keeps_it() {
        let (app, p) = seeded_app();
        assert_eq!(get_price_b(&app).await, Some(20.0));
        other_process_sets_price_b(p.to_str().unwrap(), 77.0);
        let body = json!({"coord": {"dims": {"2": 20}}, "value": 42.0});
        let resp = app.oneshot(post_cell("/measures/100/cells", body, None));
        assert_eq!(resp.await.unwrap().status(), StatusCode::OK);
        let after = Store::open(p.to_str().unwrap())
            .unwrap()
            .load_model()
            .unwrap();
        assert_eq!(
            after.input(MeasureId(100), &price_b()),
            Some(&Value::Number(77.0))
        );
        let a = Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
        assert_eq!(after.input(MeasureId(100), &a), Some(&Value::Number(42.0)));
    }

    /// A save racing another process's commit (between the server's load and
    /// its save) is refused with 409, not saved over it; the retry reloads.
    #[tokio::test]
    async fn save_over_a_racing_write_is_409() {
        let (_, p) = seeded_app();
        let state = Arc::new(AppState::new(
            p.to_str().unwrap().into(),
            Auth::Disabled,
            None,
        ));
        let model = load_model(&state).await.ok().unwrap();
        other_process_sets_price_b(p.to_str().unwrap(), 77.0);
        let err = save_model(&state, (*model).clone()).await.err().unwrap();
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        assert!(body_json(resp).await["error"]
            .as_str()
            .unwrap()
            .contains("reload"));
        assert!(state.cache.read().await.is_none(), "stale cache dropped");
        let m = load_model(&state).await.ok().unwrap();
        assert_eq!(
            m.input(MeasureId(100), &price_b()),
            Some(&Value::Number(77.0))
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
        assert_eq!(resp.headers()[header::WWW_AUTHENTICATE], "Bearer");
        // Wrong token -> 401 too.
        let resp = app
            .clone()
            .oneshot(get_with_bearer("/measures", Some("nope")))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
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
    fn ct_eq_and_find_token() {
        assert!(ct_eq(b"secret", b"secret"));
        assert!(!ct_eq(b"secret", b"secreT"));
        assert!(!ct_eq(b"secret", b"secret2"));
        assert!(!ct_eq(b"", b"x"));
        let toks = [
            token("a", Scope::Read, "a1"),
            token("b", Scope::Write, "b2"),
        ];
        assert_eq!(find_token(&toks, "b2").map(|t| t.name.as_str()), Some("b"));
        assert_eq!(find_token(&toks, "a1").map(|t| t.name.as_str()), Some("a"));
        assert!(find_token(&toks, "b").is_none());
    }

    #[test]
    fn startup_check_refuses_open_non_loopback() {
        let open = Auth::Disabled;
        let toks = Auth::Tokens(vec![token("t", Scope::Write, "s")]);
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
        let app = app(scoped_state(&[]));
        let resp = app
            .clone()
            .oneshot(get_with_bearer("/measures", Some("rsecret-7a1d")));
        assert_eq!(resp.await.unwrap().status(), StatusCode::OK);
        let cell = json!({"coord": {"dims": {"2": 20}}, "value": 5.0});
        let resp = app.clone().oneshot(post_cell(
            "/measures/100/cells",
            cell.clone(),
            Some("rsecret-7a1d"),
        ));
        assert_eq!(resp.await.unwrap().status(), StatusCode::FORBIDDEN);
        let edit = json!([{"measure": 101, "coord": [[1, 10], [2, 20]], "value": 1.0}]);
        let persist = json!({"persist": true, "edits": edit});
        let resp = app.clone().oneshot(post_cell(
            "/measures/102/eval",
            persist,
            Some("rsecret-7a1d"),
        ));
        assert_eq!(resp.await.unwrap().status(), StatusCode::FORBIDDEN);
        let what_if = json!({"edits": edit});
        let resp = app.clone().oneshot(post_cell(
            "/measures/102/eval",
            what_if,
            Some("rsecret-7a1d"),
        ));
        assert_eq!(resp.await.unwrap().status(), StatusCode::OK);
        let resp = app.oneshot(post_cell("/measures/100/cells", cell, Some("wsecret-9f3c")));
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

    fn names(auth: &Auth) -> Vec<(String, Scope)> {
        match auth {
            Auth::Disabled => vec![],
            Auth::Tokens(t) => t.iter().map(|t| (t.name.clone(), t.scope)).collect(),
        }
    }

    #[test]
    fn auth_from_lookup_reads_every_source() {
        let file = temp_path("tokens");
        std::fs::write(&file, "# ops\n\nci : read : c:i\n  admin:write:root \n").unwrap();
        let f = file.to_str().unwrap();
        let env = [
            ("IMPROV_API_TOKEN", " solo "),
            ("IMPROV_API_TOKENS", "a, b,,"),
            ("IMPROV_API_TOKENS_RO", "r"),
            ("IMPROV_API_TOKEN_FILE", f),
        ];
        let auth = Auth::from_lookup(&lookup(&env)).unwrap();
        let (w, r) = (Scope::Write, Scope::Read);
        let want = [
            ("IMPROV_API_TOKEN", w),
            ("IMPROV_API_TOKENS#1", w),
            ("IMPROV_API_TOKENS#2", w),
            ("IMPROV_API_TOKENS_RO#1", r),
            ("ci", r),
            ("admin", w),
        ];
        assert_eq!(names(&auth), want.map(|(n, s)| (n.to_string(), s)));
        let Auth::Tokens(t) = &auth else { panic!() };
        assert_eq!(t[0].secret, "solo");
        assert_eq!(t[4].secret, "c:i", "a secret may contain ':'");

        // A blank single token is ignored; read tokens alone still enable auth.
        let env = [("IMPROV_API_TOKEN", "  "), ("IMPROV_API_TOKENS_RO", "r")];
        let auth = Auth::from_lookup(&lookup(&env)).unwrap();
        assert_eq!(names(&auth), [("IMPROV_API_TOKENS_RO#1".into(), r)]);
        assert!(matches!(
            Auth::from_lookup(&lookup(&[])),
            Ok(Auth::Disabled)
        ));
    }

    #[test]
    fn auth_from_lookup_rejects_bad_sources() {
        let err = |env: &[(&str, &str)]| Auth::from_lookup(&lookup(env)).err().unwrap();
        let missing = temp_path("no-such-tokens");
        let e = err(&[("IMPROV_API_TOKEN_FILE", missing.to_str().unwrap())]);
        assert!(e.starts_with("IMPROV_API_TOKEN_FILE"), "{e}");
        let bad = temp_path("bad-tokens");
        std::fs::write(&bad, "ok:read:x\nnope\n").unwrap();
        let e = err(&[("IMPROV_API_TOKEN_FILE", bad.to_str().unwrap())]);
        assert!(e.contains("line 2"), "{e}");
        // The same secret twice (a read and a write) would be ambiguous.
        let e = err(&[("IMPROV_API_TOKEN", "s"), ("IMPROV_API_TOKENS_RO", "s")]);
        assert!(e.contains("repeat"), "{e}");
        let dup = temp_path("dup-tokens");
        std::fs::write(&dup, "x:read:1\nx:write:2\n").unwrap();
        let e = err(&[("IMPROV_API_TOKEN_FILE", dup.to_str().unwrap())]);
        assert!(e.contains("x and x"), "{e}");
    }

    #[test]
    fn token_file_lines() {
        for bad in ["a:admin:s", "a:read", ":read:s", "a:write:", "a"] {
            assert!(parse_token_file(bad).is_err(), "{bad}");
        }
        let t = parse_token_file("# c\n x:write:y \n").unwrap();
        assert_eq!((t[0].name.as_str(), t[0].scope), ("x", Scope::Write));
        assert_eq!(t[0].secret, "y");
    }

    #[cfg(unix)]
    #[test]
    fn token_file_mode_is_checked_not_required() {
        use std::os::unix::fs::PermissionsExt;
        for mode in [0o600, 0o644] {
            let f = temp_path("mode-tokens");
            std::fs::write(&f, "m:read:s\n").unwrap();
            std::fs::set_permissions(&f, std::fs::Permissions::from_mode(mode)).unwrap();
            let env = [("IMPROV_API_TOKEN_FILE", f.to_str().unwrap())];
            assert_eq!(names(&Auth::from_lookup(&lookup(&env)).unwrap()).len(), 1);
        }
    }

    #[test]
    fn configure_reads_limits_and_rejects_junk() {
        let mut state = AppState::new("x.db".into(), Auth::Disabled, None);
        let log = temp_path("audit.log");
        let env = [
            ("IMPROV_MAX_BODY_BYTES", "100"),
            ("IMPROV_RATE_LIMIT", " 7 "),
            ("IMPROV_AUDIT_LOG", log.to_str().unwrap()),
        ];
        state.configure(&lookup(&env)).unwrap();
        assert_eq!((state.max_body, state.rate.per_min), (100, 7));
        assert!(matches!(state.audit, AuditLog::File(_)));
        for env in [
            [("IMPROV_MAX_BODY_BYTES", "1M")],
            [("IMPROV_RATE_LIMIT", "-1")],
            [("IMPROV_AUDIT_LOG", "/nonexistent-improv-dir/a.log")],
        ] {
            assert!(state.configure(&lookup(&env)).is_err(), "{env:?}");
        }
    }

    #[test]
    fn model_label_hides_passwords() {
        for (db, want) in [
            ("model.db", "model.db"),
            ("postgres://u:pw@h/d?model=m", "postgres://u@h/d?model=m"),
            (
                "postgres://u@h/d?password=pw&model=m",
                "postgres://u@h/d?model=m",
            ),
            ("postgresql://h/d?password=pw", "postgresql://h/d"),
            ("postgres://u:p@w@h", "postgres://u@h"),
        ] {
            assert_eq!(model_label(db), want, "{db}");
        }
    }

    // Every route's scope, through both the /v1 path and the alias.
    #[tokio::test]
    async fn scopes_gate_every_route() {
        let app = app(scoped_state(&[]));
        let cell = json!({"coord": {"dims": {"2": 20}}, "value": 5.0});
        let what_if = json!({"edits": []});
        let parse = json!({"text": "revenue is price times quantity"});
        let resp = app
            .clone()
            .oneshot(post_cell("/nl/parse", parse.clone(), Some("rsecret-7a1d")));
        let describe = json!({"formula": body_json(resp.await.unwrap()).await});
        let req = |m: &str, uri: &str, t: Option<&str>| -> Request<Body> {
            let body = match uri.rsplit('/').next().unwrap() {
                "cells" => &cell,
                "eval" => &what_if,
                "parse" => &parse,
                "describe" => &describe,
                _ => &JsonValue::Null,
            };
            let mut b = Request::builder()
                .method(m)
                .uri(uri)
                .header("content-type", "application/json");
            if let Some(t) = t {
                b = b.header("authorization", format!("Bearer {t}"));
            }
            match m {
                "get" => b.body(Body::empty()),
                _ => b.body(Body::from(body.to_string())),
            }
            .unwrap()
        };
        for (method, path, scope, _) in routes() {
            let m = method.to_uppercase();
            for prefix in ["", "/v1"] {
                let uri = format!("{prefix}{}", path.replace(":id", "102"));
                let uri = uri.replace("102/cells", "100/cells");
                let status = |t| {
                    let r = app.clone().oneshot(req(&m, &uri, t));
                    async { r.await.unwrap().status() }
                };
                let (none, read, write) = (
                    status(None),
                    status(Some("rsecret-7a1d")),
                    status(Some("wsecret-9f3c")),
                );
                let (none, read, write) = (none.await, read.await, write.await);
                let want = match scope {
                    Scope::Public => [StatusCode::OK; 3],
                    Scope::Read => [StatusCode::UNAUTHORIZED, StatusCode::OK, StatusCode::OK],
                    Scope::Write => [
                        StatusCode::UNAUTHORIZED,
                        StatusCode::FORBIDDEN,
                        StatusCode::OK,
                    ],
                };
                assert_eq!([none, read, write], want, "{m} {uri}");
            }
        }
    }

    #[tokio::test]
    async fn body_limit_is_configurable() {
        let app = app(scoped_state(&[("IMPROV_MAX_BODY_BYTES", "64")]));
        let small = json!({"text": "x"}).to_string();
        let big = json!({"text": "x".repeat(64)}).to_string();
        for (body, want) in [(small, 400), (big, 413)] {
            let req = Request::post("/v1/nl/parse")
                .header("content-type", "application/json")
                .header("authorization", "Bearer rsecret-7a1d")
                .body(Body::from(body))
                .unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status().as_u16(), want);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limit_is_per_token_and_refills() {
        let app = app(scoped_state(&[("IMPROV_RATE_LIMIT", "2")]));
        let status = |t: &'static str| {
            let r = app
                .clone()
                .oneshot(get_with_bearer("/v1/measures", Some(t)));
            async { r.await.unwrap() }
        };
        for _ in 0..2 {
            assert_eq!(status("rsecret-7a1d").await.status(), StatusCode::OK);
        }
        let resp = status("rsecret-7a1d").await;
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()[header::RETRY_AFTER], "31");
        // Another token has its own bucket.
        assert_eq!(status("wsecret-9f3c").await.status(), StatusCode::OK);
        // Two a minute: one is back after 30 s.
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(status("rsecret-7a1d").await.status(), StatusCode::OK);
        assert_eq!(
            status("rsecret-7a1d").await.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
        // Bad tokens are limited by address (here: none known), not let through.
        let bad = |_| {
            app.clone()
                .oneshot(get_with_bearer("/v1/measures", Some("x")))
        };
        assert_eq!(bad(()).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(bad(()).await.unwrap().status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            bad(()).await.unwrap().status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn rate_limiter_zero_is_off_and_buckets_are_bounded() {
        let off = RateLimiter::new(0);
        assert!((0..1000).all(|_| off.check("k").is_ok()));
        let rl = RateLimiter::new(1);
        for i in 0..MAX_BUCKETS {
            rl.check(&i.to_string()).unwrap();
        }
        assert!(rl.check("0").is_err(), "a used bucket is remembered");
        // Full of fresh buckets: the next caller starts the table over.
        rl.check("new").unwrap();
        assert_eq!(rl.buckets.lock().unwrap().len(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn rate_limiter_forgets_idle_callers_first() {
        let rl = RateLimiter::new(1);
        for i in 0..MAX_BUCKETS - 1 {
            rl.check(&i.to_string()).unwrap();
        }
        tokio::time::advance(Duration::from_secs(61)).await;
        rl.check("recent").unwrap();
        rl.check("new").unwrap();
        let b = rl.buckets.lock().unwrap();
        assert_eq!(b.len(), 2, "idle buckets dropped, recent one kept");
        assert!(b.contains_key("recent"));
    }

    // Over a real socket the limiter keys unauthenticated callers by address.
    #[tokio::test]
    async fn rate_limit_by_client_address() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (_, path) = seeded_app();
        let mut state = AppState::new(path.to_str().unwrap().into(), Auth::Disabled, None);
        state
            .configure(&lookup(&[("IMPROV_RATE_LIMIT", "1")]))
            .unwrap();
        let state = Arc::new(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve(listener, app(state), HEADER_READ_TIMEOUT));
        let mut codes = Vec::new();
        for _ in 0..2 {
            let mut c = tokio::net::TcpStream::connect(addr).await.unwrap();
            let req = b"GET /v1/measures HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n";
            c.write_all(req).await.unwrap();
            let mut buf = String::new();
            c.read_to_string(&mut buf).await.unwrap();
            codes.push(buf[9..12].to_string());
        }
        server.abort();
        assert_eq!(codes, ["200", "429"]);
    }

    fn audit_lines(path: &std::path::Path) -> Vec<JsonValue> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        text.lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn audit_log_records_writes_not_reads() {
        let log = temp_path("audit.log");
        let state = scoped_state(&[("IMPROV_AUDIT_LOG", log.to_str().unwrap())]);
        let app = app(state.clone());
        let cell = json!({"coord": {"dims": {"2": 20}}, "value": 5.0});
        let bad_cell = json!({"coord": {"dims": {}}, "value": 5.0});
        let edit = json!([{"measure": 101, "coord": [[1, 10], [2, 20]], "value": 1.0}]);
        let persist = json!({"persist": true, "edits": edit});
        let what_if = json!({"edits": edit});
        for (uri, body, tok, want) in [
            ("/v1/measures/100/cells", &cell, Some("wsecret-9f3c"), 200),
            ("/measures/100/cells", &bad_cell, Some("wsecret-9f3c"), 400),
            ("/v1/measures/100/cells", &cell, Some("rsecret-7a1d"), 403),
            ("/v1/measures/100/cells", &cell, None, 401),
            ("/v1/measures/102/eval", &persist, Some("wsecret-9f3c"), 200),
            ("/v1/measures/102/eval", &persist, Some("rsecret-7a1d"), 403),
            ("/v1/measures/102/eval", &what_if, Some("rsecret-7a1d"), 200),
        ] {
            let resp = app.clone().oneshot(post_cell(uri, body.clone(), tok));
            assert_eq!(resp.await.unwrap().status().as_u16(), want, "{uri}");
        }
        let resp = app.oneshot(get_with_bearer("/v1/measures", Some("rsecret-7a1d")));
        assert_eq!(resp.await.unwrap().status(), StatusCode::OK);

        let lines = audit_lines(&log);
        let got: Vec<(&str, &str, u64)> = lines
            .iter()
            .map(|l| {
                assert_eq!(l["audit"], "write");
                assert_eq!(l["model"], state.model_label.as_str());
                assert!(l["ts"].as_str().unwrap().ends_with('Z'));
                let s = |k: &str| l[k].as_str().unwrap();
                (s("who"), s("route"), l["status"].as_u64().unwrap())
            })
            .collect();
        assert_eq!(
            got,
            [
                ("w", "POST /v1/measures/100/cells", 200),
                ("w", "POST /measures/100/cells", 400),
                ("r", "POST /v1/measures/100/cells", 403),
                ("unauthenticated", "POST /v1/measures/100/cells", 401),
                ("w", "POST /v1/measures/102/eval", 200),
                ("r", "POST /v1/measures/102/eval", 403),
            ]
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            !text.contains("wsecret-9f3c") && !text.contains("rsecret-7a1d"),
            "a secret leaked"
        );
    }

    // A write that times out is still audited, with its 408.
    #[tokio::test(start_paused = true)]
    async fn audit_log_records_a_timed_out_write() {
        let log = temp_path("audit.log");
        let mut state = AppState::new("unused.db".into(), Auth::Disabled, None);
        let env = [("IMPROV_AUDIT_LOG", log.to_str().unwrap())];
        state.configure(&lookup(&env)).unwrap();
        let state = Arc::new(state);
        let _held = state.write_lock.lock().await;
        let body = json!({"coord": {"dims": {"2": 20}}, "value": 1.0});
        let resp = app(state.clone())
            .oneshot(post_cell("/v1/measures/100/cells", body, None))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
        let lines = audit_lines(&log);
        assert_eq!(lines.len(), 1);
        assert_eq!(
            (&lines[0]["who"], &lines[0]["status"]),
            (&json!("anonymous"), &json!(408))
        );
    }

    #[test]
    fn audit_log_sinks() {
        let entry = json!({"audit": "write"});
        AuditLog::Stderr.record(&entry);
        AuditLog::open(None).unwrap().record(&entry);
        // A failing write falls back to stderr instead of losing the entry.
        #[cfg(target_os = "linux")]
        AuditLog::open(Some("/dev/full")).unwrap().record(&entry);
    }

    /// `(method, path, scope)` for every operation in `docs/openapi.json`.
    fn openapi_ops() -> Vec<(String, String, String)> {
        let doc: JsonValue = serde_json::from_str(OPENAPI).unwrap();
        let mut ops = Vec::new();
        for (path, item) in doc["paths"].as_object().unwrap() {
            for (method, op) in item.as_object().unwrap() {
                let scope = op["x-improv-scope"].as_str().unwrap_or("?").to_string();
                ops.push((method.clone(), path.clone(), scope));
            }
        }
        ops.sort();
        ops
    }

    // The document and the router agree on every method, path and scope.
    #[test]
    fn openapi_matches_routes() {
        let mut want: Vec<(String, String, String)> = routes()
            .into_iter()
            .map(|(m, p, scope, _)| {
                let p = format!("/v1{}", p.replace(":id", "{id}"));
                (m.to_string(), p, format!("{scope:?}").to_lowercase())
            })
            .collect();
        want.sort();
        assert_eq!(openapi_ops(), want, "docs/openapi.json is out of date");
        // GET /v1/model dumps the internal Model (the J2 problem): it stays
        // outside the /v1 promise until it returns a documented shape.
        let doc: serde_json::Value = serde_json::from_str(OPENAPI).unwrap();
        let unstable: Vec<&str> = doc["paths"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(_, ops)| {
                ops.as_object()
                    .unwrap()
                    .values()
                    .any(|o| o["x-improv-unstable"] == true)
            })
            .map(|(p, _)| p.as_str())
            .collect();
        assert_eq!(
            unstable,
            ["/v1/model"],
            "the endpoints outside the /v1 promise"
        );
    }

    #[tokio::test]
    async fn openapi_is_served() {
        let (app, _p) = authed_app("secret");
        for uri in ["/openapi.json", "/v1/openapi.json"] {
            let resp = app.clone().oneshot(get_with_bearer(uri, None)).await;
            let resp = resp.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            assert_eq!(resp.headers()[header::CONTENT_TYPE], "application/json");
            assert_eq!(body_json(resp).await["openapi"], "3.1.0");
        }
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
        Store::open(path.to_str().unwrap())
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
