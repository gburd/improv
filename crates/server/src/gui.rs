//! The `/v1` routes a GUI drives (I2): each parses its request into an
//! [`improv_client::wire::Request`], has [`improv_service`]'s authority
//! serve it for the caller, and answers with what the caller may see.
//! `GET /v1/changes` streams what changed, as Server-Sent Events.
//!
//! **The policy seam.** Every route goes through [`serve`]: the bearer
//! token's scope is checked first (the outer cap: a read token never
//! writes), then the authority asks its [`improv_service::policy::Policy`]
//! for every measure, item and cell it touches. Cedar plugs in there.
//!
//! **One authority, one at a time.** It holds the model and a live engine,
//! and runs on a blocking thread behind a mutex. Writes made through the
//! older routes (`/cells`, `/eval` with `persist`) or by other processes
//! are seen on the next request, by the store's high-water check.

use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::{rejection::JsonRejection, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use improv_client::wire::{
    DefineReq, EditReq, Failure, NameReq, Request as Req, Response as Resp, UpdateMeasureReq,
    ViewReq, WindowReq,
};
use improv_service::authority::{Authority, Refusal};
use serde::Deserialize;

use crate::{AppState, Audited, Caller, Scope};

/// The authority, made on first use (opening it loads the model and
/// builds the engine).
pub struct Gui {
    authority: Mutex<Option<Authority>>,
    /// The policy the authority opens with, when a test sets one. Else
    /// it opens with the model's stored Cedar policies
    /// ([`improv_service::cedar::CedarPolicy`]): none stored is open.
    pub policy: Mutex<Option<Box<dyn improv_service::policy::Policy>>>,
    /// Bumped after every change the authority has seen (a write through
    /// these routes, or the [`poller`]'s refresh finding another writer's):
    /// change streams wait on it and never read the store themselves.
    changed: tokio::sync::watch::Sender<u64>,
    /// Open change streams: in all, and per principal.
    streams: Mutex<Streams>,
}

impl Default for Gui {
    fn default() -> Self {
        Gui {
            authority: Mutex::default(),
            policy: Mutex::default(),
            changed: tokio::sync::watch::Sender::new(0),
            streams: Mutex::default(),
        }
    }
}

/// Open change streams, counted for [`MAX_STREAMS`] and
/// [`MAX_STREAMS_PER_CALLER`]; the poller runs while any is open.
#[derive(Default)]
struct Streams {
    total: usize,
    per: std::collections::HashMap<String, usize>,
    polling: bool,
}

/// Default most change streams open at once (`IMPROV_MAX_STREAMS`).
pub const MAX_STREAMS: usize = 256;
/// Most change streams one principal may hold open.
pub const MAX_STREAMS_PER_CALLER: usize = 8;

/// One open stream's slot: freed when the stream is dropped (the client
/// went away, or the server ended it).
struct Slot {
    state: Arc<AppState>,
    who: String,
}

impl Drop for Slot {
    fn drop(&mut self) {
        let mut s = self
            .state
            .gui
            .streams
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        s.total = s.total.saturating_sub(1);
        if let Some(n) = s.per.get_mut(&self.who) {
            *n -= 1;
            if *n == 0 {
                s.per.remove(&self.who);
            }
        }
    }
}

/// Take a stream slot for `who`, or say which limit is full.
fn take_slot(state: &Arc<AppState>, who: &str) -> Result<Slot, &'static str> {
    let mut s = state
        .gui
        .streams
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if s.total >= state.max_streams {
        return Err("too many change streams are open; try again later");
    }
    let mine = s.per.entry(who.to_owned()).or_default();
    if *mine >= MAX_STREAMS_PER_CALLER {
        return Err("this caller has too many change streams open");
    }
    *mine += 1;
    s.total += 1;
    let start = !std::mem::replace(&mut s.polling, true);
    drop(s);
    if start {
        tokio::spawn(poller(state.clone()));
    }
    Ok(Slot {
        state: state.clone(),
        who: who.to_owned(),
    })
}

/// The one task that looks for other writers' changes (one store read
/// every [`POLL`], however many streams are open) and wakes the streams
/// when it finds one. Ends when the last stream closes.
async fn poller(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(POLL).await;
        {
            let mut s = state
                .gui
                .streams
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if s.total == 0 {
                s.polling = false;
                return;
            }
        }
        let st = state.clone();
        let moved = tokio::task::spawn_blocking(move || with_authority(&st, |a| a.refresh())).await;
        // An error (the store is unreachable) wakes the streams too: they
        // end, and their clients reconnect with `since`.
        if !matches!(moved, Ok(Ok(false))) {
            state.gui.changed.send_modify(|v| *v += 1);
        }
    }
}

/// The paths of these routes (without `/v1`), as `routes()` lists them.
#[cfg(test)]
pub const ROUTES: &[&str] = &[
    "/structure",
    "/capabilities",
    "/measures/:id/window",
    "/cells",
    "/measures",
    "/measures/:id",
    "/categories",
    "/categories/:id",
    "/categories/:id/items",
    "/items/:id",
    "/views",
    "/views/:id",
    "/undo",
    "/redo",
    "/changes",
];

/// How often the poller looks for other writers' changes.
const POLL: Duration = Duration::from_millis(500);

/// The session a request's undo belongs to: the `x-improv-session` header
/// (a random id a client makes once), else the caller's own stack.
fn session(headers: &HeaderMap, caller: &Caller) -> String {
    let id = headers
        .get("x-improv-session")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty() && s.len() <= 64)
        .unwrap_or("default");
    // Prefixed by the caller, so one token cannot undo another's steps.
    format!("{}/{id}", caller.name)
}

/// Run `req` for the caller on the authority, on a blocking thread.
async fn serve(
    state: &Arc<AppState>,
    caller: &Caller,
    audited: &Audited,
    session: String,
    req: Req,
) -> Result<Resp, Refusal> {
    if req.writes() {
        audited.mark(&caller.name);
        if caller.scope != Scope::Write {
            return Err(refusal(403, "this route needs a write token"));
        }
    }
    let writes = req.writes();
    let st = state.clone();
    let who = caller.principal();
    let r = tokio::task::spawn_blocking(move || {
        // Writes here and through the older routes go one at a time.
        let _write = writes.then(|| st.write_lock.blocking_lock());
        with_authority(&st, |a| a.serve(&who, &session, req))
    })
    .await
    .map_err(|e| refusal(500, format!("task join: {e}")))?;
    if writes {
        // The older routes' cache must see this write.
        *state.cache.write().await = None;
        state.gui.changed.send_modify(|v| *v += 1);
    }
    r
}

/// Run `f` on the authority, opening it first if need be. When `f` moves
/// the model's version (a write here, or any refresh that found another
/// program's write: every request refreshes first, not only the poller),
/// the change streams are woken.
fn with_authority<T>(
    st: &AppState,
    f: impl FnOnce(&mut Authority) -> Result<T, Refusal>,
) -> Result<T, Refusal> {
    let mut slot = st
        .gui
        .authority
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if slot.is_none() {
        let policy = st
            .gui
            .policy
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let policy = policy.unwrap_or_else(|| {
            Box::new(improv_service::cedar::CedarPolicy::new(
                &st.db_path,
                improv_policy::Channel::Server,
            ))
        });
        let opened = Authority::with_policy(&st.db_path, policy);
        *slot = Some(opened.map_err(|e| refusal(500, e))?);
    }
    let a = slot.as_mut().expect("opened above");
    let before = a.version();
    let r = f(a);
    if a.version() != before {
        st.gui.changed.send_modify(|v| *v += 1);
    }
    r
}

fn refusal(status: u16, error: impl Into<String>) -> Refusal {
    Refusal {
        status,
        body: Failure {
            error: error.into(),
            ..Default::default()
        },
    }
}

fn refused(r: Refusal) -> Response {
    let status = StatusCode::from_u16(r.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    if status == StatusCode::INTERNAL_SERVER_ERROR {
        // The detail (store errors carry the path) stays in the log.
        return crate::ApiError::Internal(r.body.error).into_response();
    }
    (status, Json(r.body)).into_response()
}

fn reply(r: Result<Resp, Refusal>) -> Response {
    match r {
        Ok(Resp::Structure(s)) => Json(s).into_response(),
        Ok(Resp::Cells(c)) => Json(c).into_response(),
        Ok(Resp::Done(d)) => Json(d).into_response(),
        Err(e) => refused(e),
    }
}

fn bad_body(e: JsonRejection) -> Response {
    let status = match e.status() {
        StatusCode::PAYLOAD_TOO_LARGE => 413,
        _ => 400,
    };
    refused(refusal(status, e.body_text()))
}

/// What every route hands [`serve`].
pub struct Ctx {
    state: Arc<AppState>,
    caller: Caller,
    audited: Audited,
    session: String,
}

impl Ctx {
    async fn run(self, req: Req) -> Response {
        reply(serve(&self.state, &self.caller, &self.audited, self.session, req).await)
    }
}

#[axum::async_trait]
impl axum::extract::FromRequestParts<Arc<AppState>> for Ctx {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Infallible> {
        let caller = parts.extensions.get::<Caller>().cloned().unwrap_or(Caller {
            name: "anonymous".into(),
            scope: Scope::Read,
        });
        let audited = parts
            .extensions
            .get::<Audited>()
            .cloned()
            .unwrap_or_default();
        Ok(Ctx {
            session: session(&parts.headers, &caller),
            state: state.clone(),
            caller,
            audited,
        })
    }
}

/// A JSON body, or the 400/413 that says what was wrong with it.
macro_rules! body {
    ($b:expr) => {
        match $b {
            Ok(Json(v)) => v,
            Err(e) => return bad_body(e),
        }
    };
}

pub async fn structure(cx: Ctx) -> Response {
    cx.run(Req::Structure).await
}

/// `GET /v1/capabilities`: what the caller may do, as hints for a GUI.
pub async fn capabilities(cx: Ctx) -> Response {
    let who = cx.caller.principal();
    let st = cx.state.clone();
    let r = tokio::task::spawn_blocking(move || with_authority(&st, |a| a.capabilities(&who)))
        .await
        .map_err(|e| refusal(500, format!("task join: {e}")));
    match r.and_then(|r| r) {
        Ok(c) => Json(c).into_response(),
        Err(e) => refused(e),
    }
}

pub async fn window(
    cx: Ctx,
    Path(id): Path<u32>,
    b: Result<Json<WindowReq>, JsonRejection>,
) -> Response {
    let w = body!(b);
    cx.run(Req::Window(id, w)).await
}

pub async fn edit_cells(cx: Ctx, b: Result<Json<EditReq>, JsonRejection>) -> Response {
    let e = body!(b);
    cx.run(Req::Edit(e)).await
}

pub async fn define(cx: Ctx, b: Result<Json<DefineReq>, JsonRejection>) -> Response {
    let d = body!(b);
    cx.run(Req::Define(d)).await
}

pub async fn update_measure(
    cx: Ctx,
    Path(id): Path<u32>,
    b: Result<Json<UpdateMeasureReq>, JsonRejection>,
) -> Response {
    let u = body!(b);
    cx.run(Req::UpdateMeasure(id, u)).await
}

pub async fn delete_measure(cx: Ctx, Path(id): Path<u32>) -> Response {
    cx.run(Req::DeleteMeasure(id)).await
}

pub async fn add_category(cx: Ctx, b: Result<Json<NameReq>, JsonRejection>) -> Response {
    let n = body!(b);
    cx.run(Req::AddCategory(n)).await
}

pub async fn rename_category(
    cx: Ctx,
    Path(id): Path<u32>,
    b: Result<Json<NameReq>, JsonRejection>,
) -> Response {
    let n = body!(b);
    cx.run(Req::RenameCategory(id, n)).await
}

pub async fn delete_category(cx: Ctx, Path(id): Path<u32>) -> Response {
    cx.run(Req::DeleteCategory(id)).await
}

pub async fn add_item(
    cx: Ctx,
    Path(id): Path<u32>,
    b: Result<Json<NameReq>, JsonRejection>,
) -> Response {
    let n = body!(b);
    cx.run(Req::AddItem(id, n)).await
}

pub async fn rename_item(
    cx: Ctx,
    Path(id): Path<u32>,
    b: Result<Json<NameReq>, JsonRejection>,
) -> Response {
    let n = body!(b);
    cx.run(Req::RenameItem(id, n)).await
}

pub async fn delete_item(cx: Ctx, Path(id): Path<u32>) -> Response {
    cx.run(Req::DeleteItem(id)).await
}

pub async fn save_view(cx: Ctx, b: Result<Json<ViewReq>, JsonRejection>) -> Response {
    let v = body!(b);
    cx.run(Req::SaveView(v)).await
}

pub async fn update_view(
    cx: Ctx,
    Path(id): Path<u32>,
    b: Result<Json<ViewReq>, JsonRejection>,
) -> Response {
    let v = body!(b);
    cx.run(Req::UpdateView(id, v)).await
}

pub async fn delete_view(cx: Ctx, Path(id): Path<u32>) -> Response {
    cx.run(Req::DeleteView(id)).await
}

pub async fn undo(cx: Ctx) -> Response {
    cx.run(Req::Undo).await
}

pub async fn redo(cx: Ctx) -> Response {
    cx.run(Req::Redo).await
}

#[derive(Deserialize)]
pub struct Since {
    since: Option<String>,
}

/// `GET /v1/changes[?since=V]`: Server-Sent Events. Without `since` the
/// first event is the current version (`{"version": V}`); with it, what
/// the caller may see of what changed since. Then one event per change
/// the caller may see: the changed
/// cells, or `"reload": true` after a structural change or a gap the log
/// no longer covers. A change the caller may see none of sends nothing.
///
/// A stream outlives the request timeout and takes no in-flight slot
/// (tower's timeout and concurrency limit apply until the response
/// headers go out); [`MAX_STREAMS`] and [`MAX_STREAMS_PER_CALLER`] bound
/// them instead, with 429 past either.
/// Each event's `id` is its version, so a browser's `Last-Event-ID`
/// resumes too.
pub async fn changes(
    State(state): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    headers: HeaderMap,
    Query(q): Query<Since>,
) -> Response {
    let since = q.since.or_else(|| {
        headers
            .get("last-event-id")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    });
    let who = caller.principal();
    // Keyed by the principal (never the secret), as the policy sees it.
    let slot = match take_slot(&state, &who.user) {
        Ok(s) => s,
        Err(e) => return refused(refusal(429, e)),
    };
    let mut changed = state.gui.changed.subscribe();
    // The first look: the current version, or the gap since `since`.
    let st = state.clone();
    let w = who.clone();
    let first = tokio::task::spawn_blocking(move || {
        with_authority(&st, |a| {
            a.refresh()?;
            let v = a.version();
            Ok(match since {
                // What changed since, or nothing when the caller may see
                // none of it.
                Some(s) => (a.changes_since(&w, &s), v),
                // A new subscriber learns the version it starts from.
                None => (
                    Some(improv_client::wire::ChangeEvent {
                        version: v.clone(),
                        ..Default::default()
                    }),
                    v,
                ),
            })
        })
    })
    .await;
    let (first, seen) = match first {
        Ok(Ok(v)) => v,
        Ok(Err(r)) => return refused(r),
        Err(e) => return refused(refusal(500, format!("task join: {e}"))),
    };
    changed.mark_unchanged();
    let stream = futures_util::stream::unfold(
        (state, who, seen, first, changed, slot),
        |(state, who, seen, pending, mut changed, slot)| async move {
            if let Some(ev) = pending {
                return Some((sse(&ev), (state, who, seen, None, changed, slot)));
            }
            loop {
                // Woken by a write here or the poller's refresh; no store
                // read of its own. The sender lives in `state`.
                changed.changed().await.ok()?;
                let st = state.clone();
                let w = who.clone();
                let s = seen.clone();
                let looked = tokio::task::spawn_blocking(move || {
                    let slot = st
                        .gui
                        .authority
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner);
                    slot.as_ref()
                        .map(|a| (a.changes_since(&w, &s), a.version()))
                })
                .await;
                match looked {
                    Ok(Some((Some(ev), now))) => {
                        return Some((sse(&ev), (state, who, now, None, changed, slot)));
                    }
                    // Nothing this caller may see changed: no event.
                    Ok(Some((None, now))) if now == seen => {}
                    Ok(Some((None, now))) => {
                        return Some((None, (state, who, now, None, changed, slot)));
                    }
                    // The store went away (the poller's refresh failed):
                    // end the stream; the client reconnects with `since`.
                    _ => return None,
                }
            }
        },
    );
    let stream =
        futures_util::StreamExt::filter_map(stream, |e| async move { e.map(Ok::<_, Infallible>) });
    let mut resp = Sse::new(stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response();
    // Not cached, not buffered by a proxy (nginx).
    let h = resp.headers_mut();
    h.insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    h.insert("x-accel-buffering", header::HeaderValue::from_static("no"));
    resp
}

fn sse(ev: &improv_client::wire::ChangeEvent) -> Option<SseEvent> {
    SseEvent::default()
        .id(ev.version.clone())
        .json_data(ev)
        .ok()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
