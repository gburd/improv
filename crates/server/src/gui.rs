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
use improv_service::policy::Principal;
use serde::Deserialize;

use crate::{AppState, Audited, Caller, Scope};

/// The authority, made on first use (opening it loads the model and
/// builds the engine).
#[derive(Default)]
pub struct Gui {
    authority: Mutex<Option<Authority>>,
    /// The policy the authority opens with (`None`: no policy). Set by
    /// tests; Cedar's loader sets it from the model's stored policies.
    pub policy: Mutex<Option<Box<dyn improv_service::policy::Policy>>>,
    /// Woken after every write through these routes, so change streams
    /// look at once; they also look every [`POLL`].
    wake: tokio::sync::Notify,
}

/// The paths of these routes (without `/v1`), as `routes()` lists them.
#[cfg(test)]
pub const ROUTES: &[&str] = &[
    "/structure",
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

/// How often a change stream looks for other writers' changes.
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
    let who = Principal::user(caller.name.clone());
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
        state.gui.wake.notify_waiters();
    }
    r
}

/// Run `f` on the authority, opening it first if need be.
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
        let opened = match policy {
            Some(p) => Authority::with_policy(&st.db_path, p),
            None => Authority::open(&st.db_path),
        };
        *slot = Some(opened.map_err(|e| refusal(500, e))?);
    }
    f(slot.as_mut().expect("opened above"))
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
    let who = Principal::user(caller.name.clone());
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
    let stream = futures_util::stream::unfold(
        (state, who, seen, first),
        |(state, who, seen, pending)| async move {
            if let Some(ev) = pending {
                return Some((sse(&ev), (state, who, seen, None)));
            }
            loop {
                // Armed before the look, so a write during it still wakes us.
                let armed = state.clone();
                let wake = armed.gui.wake.notified();
                let st = state.clone();
                let w = who.clone();
                let s = seen.clone();
                let looked = tokio::task::spawn_blocking(move || {
                    with_authority(&st, |a| {
                        a.refresh()?;
                        Ok((a.changes_since(&w, &s), a.version()))
                    })
                })
                .await;
                match looked {
                    Ok(Ok((Some(ev), now))) => return Some((sse(&ev), (state, who, now, None))),
                    Ok(Ok((None, now))) if now != seen => {
                        // Nothing this caller may see changed: no event.
                        return Some((None, (state, who, now, None)));
                    }
                    Ok(Ok(_)) => {}
                    // The store is unreachable: end the stream; the client
                    // reconnects with `since`.
                    _ => return None,
                }
                tokio::select! {
                    () = wake => {}
                    () = tokio::time::sleep(POLL) => {}
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
