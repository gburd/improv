//! The remote [`ModelService`]: requests over `fetch` to the same origin's
//! `/v1`, and the change stream read with `fetch` too (an `EventSource`
//! cannot send the `Authorization` header). The bearer token lives in this
//! struct only: never in the URL, a cookie or storage, so there is nothing
//! for another site to ride (no CSRF), and a reload asks for it again.

use std::cell::RefCell;
use std::rc::Rc;

use improv_client::wire::{ChangeEvent, Event, Failure, ModelService, Request};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// What the callbacks share with the frame loop.
#[derive(Default)]
struct Shared {
    out: Vec<Event>,
    /// The last version the stream delivered (`?since=` on a reconnect).
    seen: Option<String>,
    /// The stream is connected.
    live: bool,
}

pub struct Remote {
    token: Rc<RefCell<Option<String>>>,
    session: String,
    shared: Rc<RefCell<Shared>>,
    next: u64,
    /// Bumped on each (re)login: an older stream loop stops.
    epoch: Rc<RefCell<u64>>,
    repaint: Rc<dyn Fn()>,
}

impl Remote {
    pub fn new(session: String, repaint: Rc<dyn Fn()>) -> Self {
        Remote {
            token: Rc::default(),
            session,
            shared: Rc::default(),
            next: 0,
            epoch: Rc::default(),
            repaint,
        }
    }

    /// Start (or restart) the change stream.
    fn stream(&self) {
        let epoch = {
            let mut e = self.epoch.borrow_mut();
            *e += 1;
            *e
        };
        let (token, shared, mine, repaint) = (
            self.token.clone(),
            self.shared.clone(),
            self.epoch.clone(),
            self.repaint.clone(),
        );
        wasm_bindgen_futures::spawn_local(async move {
            let mut backoff = 250u32;
            while *mine.borrow() == epoch {
                let since = shared.borrow().seen.clone();
                let r = read_stream(&token, &shared, &mine, epoch, since, &*repaint).await;
                shared.borrow_mut().live = false;
                repaint();
                match r {
                    Err(Gone::Auth(e)) => {
                        shared.borrow_mut().out.push(Event::LoggedOut(e));
                        repaint();
                        return;
                    }
                    // A clean read resets the wait; a failure doubles it.
                    Ok(()) => backoff = 250,
                    Err(Gone::Net(e)) => {
                        web_sys::console::warn_1(&format!("improv: change stream: {e}").into());
                        backoff = (backoff * 2).min(10_000);
                    }
                }
                sleep(backoff).await;
            }
        });
    }
}

impl ModelService for Remote {
    fn send(&mut self, req: Request) -> u64 {
        self.next += 1;
        let ticket = self.next;
        let (method, path, body) = req.http();
        let (token, session, shared, repaint) = (
            self.token.borrow().clone(),
            self.session.clone(),
            self.shared.clone(),
            self.repaint.clone(),
        );
        wasm_bindgen_futures::spawn_local(async move {
            let r = call(method, &path, body.map(|b| b.to_string()), token, &session).await;
            let ev = match r {
                Ok(text) => Event::Reply(
                    ticket,
                    req.parse_reply(&text).map_err(|e| Failure {
                        error: e,
                        ..Default::default()
                    }),
                ),
                Err((401, f)) => Event::LoggedOut(f.error),
                Err((_, f)) => Event::Reply(ticket, Err(f)),
            };
            shared.borrow_mut().out.push(ev);
            repaint();
        });
        ticket
    }

    fn poll(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.shared.borrow_mut().out)
    }

    fn live(&self) -> bool {
        self.shared.borrow().live
    }

    fn authenticate(&mut self, secret: String) {
        *self.token.borrow_mut() = (!secret.is_empty()).then_some(secret);
        self.stream();
    }
}

/// Why a stream ended.
enum Gone {
    Auth(String),
    Net(String),
}

fn js_err(e: JsValue) -> String {
    e.as_string().unwrap_or_else(|| format!("{e:?}"))
}

fn request(
    method: &str,
    path: &str,
    body: Option<String>,
    token: Option<&str>,
    session: Option<&str>,
) -> Result<web_sys::Request, JsValue> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    init.set_cache(web_sys::RequestCache::NoStore);
    if let Some(b) = &body {
        init.set_body(&JsValue::from_str(b));
    }
    let req = web_sys::Request::new_with_str_and_init(path, &init)?;
    let h = req.headers();
    if body.is_some() {
        h.set("content-type", "application/json")?;
    }
    if let Some(t) = token {
        h.set("authorization", &format!("Bearer {t}"))?;
    }
    if let Some(s) = session {
        h.set("x-improv-session", s)?;
    }
    Ok(req)
}

/// One request; the body text, or the status and the server's refusal.
async fn call(
    method: &str,
    path: &str,
    body: Option<String>,
    token: Option<String>,
    session: &str,
) -> Result<String, (u16, Failure)> {
    let net = |e: JsValue| {
        (
            0,
            Failure {
                error: format!("network: {}", js_err(e)),
                ..Default::default()
            },
        )
    };
    let req = request(method, path, body, token.as_deref(), Some(session)).map_err(net)?;
    let window = web_sys::window().ok_or_else(|| net(JsValue::from_str("no window")))?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_request(&req))
        .await
        .and_then(|r| r.dyn_into())
        .map_err(net)?;
    let text = JsFuture::from(resp.text().map_err(net)?)
        .await
        .map_err(net)?
        .as_string()
        .unwrap_or_default();
    if resp.ok() {
        return Ok(text);
    }
    let f = serde_json::from_str::<Failure>(&text).unwrap_or(Failure {
        error: format!("HTTP {}: {text}", resp.status()),
        ..Default::default()
    });
    Err((resp.status(), f))
}

/// Read `GET /v1/changes` until it ends, pushing each event.
async fn read_stream(
    token: &Rc<RefCell<Option<String>>>,
    shared: &Rc<RefCell<Shared>>,
    epoch: &Rc<RefCell<u64>>,
    mine: u64,
    since: Option<String>,
    repaint: &dyn Fn(),
) -> Result<(), Gone> {
    let path = match &since {
        Some(v) => format!("/v1/changes?since={}", js_sys::encode_uri_component(v)),
        None => "/v1/changes".to_owned(),
    };
    let net = |e: JsValue| Gone::Net(js_err(e));
    let t = token.borrow().clone();
    let req = request("GET", &path, None, t.as_deref(), None).map_err(net)?;
    let window = web_sys::window().ok_or_else(|| Gone::Net("no window".into()))?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_request(&req))
        .await
        .and_then(|r| r.dyn_into())
        .map_err(net)?;
    if resp.status() == 401 {
        return Err(Gone::Auth("the token was refused".into()));
    }
    if !resp.ok() {
        return Err(Gone::Net(format!("HTTP {}", resp.status())));
    }
    let body = resp.body().ok_or_else(|| Gone::Net("no body".into()))?;
    let reader: web_sys::ReadableStreamDefaultReader = body
        .get_reader()
        .dyn_into()
        .map_err(|o: js_sys::Object| net(o.into()))?;
    shared.borrow_mut().live = true;
    repaint();
    let mut buf = String::new();
    while *epoch.borrow() == mine {
        let chunk = JsFuture::from(reader.read()).await.map_err(net)?;
        let done = js_sys::Reflect::get(&chunk, &"done".into())
            .map(|v| v.is_truthy())
            .unwrap_or(true);
        if done {
            return Ok(());
        }
        let bytes = js_sys::Reflect::get(&chunk, &"value".into()).map_err(net)?;
        let bytes = js_sys::Uint8Array::new(&bytes).to_vec();
        buf.push_str(&String::from_utf8_lossy(&bytes));
        for ev in take_events(&mut buf) {
            let mut s = shared.borrow_mut();
            s.seen = Some(ev.version.clone());
            s.out.push(Event::Changed(ev));
        }
        repaint();
    }
    let _ = reader.cancel();
    Ok(())
}

/// The complete events at the front of `buf` (each ends with a blank
/// line), parsed; what is left stays for the next chunk.
pub fn take_events(buf: &mut String) -> Vec<ChangeEvent> {
    let mut out = Vec::new();
    while let Some(end) = buf.find("\n\n") {
        let frame: String = buf.drain(..end + 2).collect();
        let data: String = frame
            .lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .map(str::trim_start)
            .collect();
        if let Ok(ev) = serde_json::from_str::<ChangeEvent>(&data) {
            out.push(ev);
        }
    }
    out
}

async fn sleep(ms: u32) {
    let p = js_sys::Promise::new(&mut |resolve, _| {
        if let Some(w) = web_sys::window() {
            let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms as i32);
        }
    });
    let _ = JsFuture::from(p).await;
}
