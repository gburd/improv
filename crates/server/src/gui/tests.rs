//! The GUI routes over HTTP: each route through the router, the change
//! stream with two principals under different policies, and resuming.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use improv_core_model::{CategoryId, ItemId, MeasureId};
use improv_service::policy::{Action, Deny, Resource};
use serde_json::{json, Value as JsonValue};
use tower::util::ServiceExt;

use crate::tests::{body_json, revenue_model, temp_path};
use crate::{app, AppState, Auth, Scope, Token};

const ANN: &str = "ann-secret-1";
const BOB: &str = "bob-secret-2";
const RO: &str = "ro-secret-3";

/// A served revenue model with write tokens `ann` and `bob` and a read
/// token `ro`; ann may not see WidgetB (item 21).
fn served() -> (axum::Router, Arc<AppState>, std::path::PathBuf) {
    let path = temp_path("gui.db");
    improv_store::Store::open(path.to_str().unwrap())
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    let tok = |n: &str, s, secret: &str| Token {
        name: n.into(),
        scope: s,
        secret: secret.into(),
    };
    let auth = Auth::Tokens(vec![
        tok("ann", Scope::Write, ANN),
        tok("bob", Scope::Write, BOB),
        tok("ro", Scope::Read, RO),
    ]);
    let state = AppState::new(path.to_str().unwrap().into(), auth, None);
    *state.gui.policy.lock().unwrap() = Some(Box::new(Deny(vec![(
        "ann".into(),
        Action::ReadItem,
        Resource::Item(ItemId(21)),
    )])));
    let state = Arc::new(state);
    (app(state.clone()), state, path)
}

fn req(method: &str, uri: &str, token: &str, body: Option<JsonValue>) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"))
        .header("content-type", "application/json")
        .header("x-improv-session", "w1")
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap()
}

async fn call(app: &axum::Router, r: Request<Body>) -> (StatusCode, JsonValue) {
    let resp = app.clone().oneshot(r).await.unwrap();
    let status = resp.status();
    (status, body_json(resp).await)
}

async fn window(app: &axum::Router, token: &str, m: u32) -> JsonValue {
    let (s, b) = call(
        app,
        req(
            "POST",
            &format!("/v1/measures/{m}/window"),
            token,
            Some(json!({})),
        ),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    b
}

fn value_at(w: &JsonValue, at: JsonValue) -> Option<JsonValue> {
    w["cells"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["at"] == at)
        .map(|c| c["value"].clone())
}

#[tokio::test]
async fn every_gui_route_works_over_http() {
    let (app, _, _) = served();
    let (s, st) = call(&app, req("GET", "/v1/structure", BOB, None)).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(st["measures"][2]["formula"], "Price * Quantity");
    // An edit batch, and the recalculated Revenue.
    let edit = json!({"edits": [{"measure": 101, "at": [[1, 10], [2, 20]], "value": 150}]});
    let (s, d) = call(&app, req("POST", "/v1/cells", BOB, Some(edit.clone()))).await;
    assert_eq!(
        (s, d["message"].as_str()),
        (StatusCode::OK, Some("1 cell set"))
    );
    let w = window(&app, BOB, 102).await;
    assert_eq!(value_at(&w, json!([[1, 10], [2, 20]])), Some(json!(1500.0)));
    // A read token reads, and cannot write.
    let (s, _) = call(&app, req("POST", "/v1/cells", RO, Some(edit))).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // A refused batch names the edit and leaves the rest alone.
    let bad = json!({"edits": [
        {"measure": 101, "at": [[1, 10], [2, 20]], "value": 1},
        {"measure": 102, "at": [[1, 10], [2, 20]], "value": 1}]});
    let (s, f) = call(&app, req("POST", "/v1/cells", BOB, Some(bad))).await;
    assert_eq!(
        (s, f["index"].as_u64()),
        (StatusCode::BAD_REQUEST, Some(1)),
        "{f}"
    );
    // A formula with a located error, then a good one.
    let def = |t: &str| Some(json!({"definition": t}));
    let (s, f) = call(
        &app,
        req("POST", "/v1/measures", BOB, def("Tax = Revenu * 0.1")),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(f["diagnostic"]["span"], json!([6, 12]), "{f}");
    let (s, d) = call(
        &app,
        req("POST", "/v1/measures", BOB, def("Tax = Revenue * 0.1")),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{d}");
    let tax = d["id"].as_u64().unwrap();
    let put = json!({"formula": "Revenue * 0.2", "name": "Duty", "format": {"decimals": 2}});
    let (s, _) = call(
        &app,
        req("PUT", &format!("/v1/measures/{tax}"), BOB, Some(put)),
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // Categories, items and views.
    let (_, c) = call(
        &app,
        req(
            "POST",
            "/v1/categories",
            BOB,
            Some(json!({"name": "Region"})),
        ),
    )
    .await;
    let region = c["id"].as_u64().unwrap();
    let (_, i) = call(
        &app,
        req(
            "POST",
            &format!("/v1/categories/{region}/items"),
            BOB,
            Some(json!({"name": "North"})),
        ),
    )
    .await;
    let north = i["id"].as_u64().unwrap();
    for (m, uri, body) in [
        (
            "PUT",
            format!("/v1/items/{north}"),
            Some(json!({"name": "Nord"})),
        ),
        (
            "PUT",
            format!("/v1/categories/{region}"),
            Some(json!({"name": "Area"})),
        ),
        ("DELETE", format!("/v1/items/{north}"), None),
        ("DELETE", format!("/v1/categories/{region}"), None),
        (
            "POST",
            "/v1/views".into(),
            Some(json!({"name": "V", "matrices": [{"measure": 102}]})),
        ),
        ("PUT", "/v1/views/1".into(), Some(json!({"name": "W"}))),
        ("DELETE", "/v1/views/1".into(), None),
        ("DELETE", format!("/v1/measures/{tax}"), None),
        ("POST", "/v1/undo".into(), None),
        ("POST", "/v1/redo".into(), None),
    ] {
        let (s, b) = call(&app, req(m, &uri, BOB, body)).await;
        assert_eq!(s, StatusCode::OK, "{m} {uri}: {b}");
    }
    // A malformed body is a 400, not a panic.
    let r = Request::post("/v1/cells")
        .header("authorization", format!("Bearer {BOB}"))
        .header("content-type", "application/json")
        .body(Body::from("{nope"))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(r).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_policy_filters_every_answer_per_principal() {
    let (app, _, _) = served();
    let (_, mine) = call(&app, req("GET", "/v1/structure", ANN, None)).await;
    assert!(!mine.to_string().contains("WidgetB"), "{mine}");
    let (_, bobs) = call(&app, req("GET", "/v1/structure", BOB, None)).await;
    assert!(bobs.to_string().contains("WidgetB"));
    let w = window(&app, ANN, 102).await;
    assert!(
        w["cells"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["at"][1][1] != 21),
        "{w}"
    );
    // Ann editing a cell she cannot see: refused as a missing one.
    let hidden = json!({"edits": [{"measure": 101, "at": [[1, 10], [2, 21]], "value": 1}]});
    let missing = json!({"edits": [{"measure": 101, "at": [[1, 10], [2, 29]], "value": 1}]});
    let a = call(&app, req("POST", "/v1/cells", ANN, Some(hidden))).await;
    let b = call(&app, req("POST", "/v1/cells", ANN, Some(missing))).await;
    assert_eq!(a, b);
}

/// Read SSE `data:` events from a response body until `n` have arrived.
async fn events(body: Body, n: usize) -> Vec<JsonValue> {
    use futures_util::StreamExt;
    let mut stream = body.into_data_stream();
    let mut buf = String::new();
    let mut out = Vec::new();
    while out.len() < n {
        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("an event within 5 s")
            .expect("the stream is open")
            .unwrap();
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
        while let Some(end) = buf.find("\n\n") {
            let frame: String = buf.drain(..end + 2).collect();
            for line in frame.lines() {
                if let Some(d) = line.strip_prefix("data:") {
                    out.push(serde_json::from_str(d.trim()).unwrap());
                }
            }
        }
    }
    out
}

async fn stream(app: &axum::Router, token: &str, since: Option<&str>) -> Body {
    let uri = since.map_or("/v1/changes".into(), |s| format!("/v1/changes?since={s}"));
    let resp = app
        .clone()
        .oneshot(req("GET", &uri, token, None))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()["content-type"], "text/event-stream");
    resp.into_body()
}

#[tokio::test]
async fn one_change_reaches_each_subscriber_filtered_for_them() {
    let (app, _, _) = served();
    let ann = stream(&app, ANN, None).await;
    let bob = stream(&app, BOB, None).await;
    // Bob changes a cell each may see and one only he may.
    let edit = json!({"edits": [
        {"measure": 101, "at": [[1, 10], [2, 20]], "value": 7},
        {"measure": 101, "at": [[1, 10], [2, 21]], "value": 8}]});
    let (s, _) = call(&app, req("POST", "/v1/cells", BOB, Some(edit))).await;
    assert_eq!(s, StatusCode::OK);
    let (a, b) = tokio::join!(events(ann, 2), events(bob, 2));
    let at = |e: &JsonValue| -> Vec<JsonValue> {
        e["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["at"].clone())
            .collect()
    };
    assert!(at(&b[1]).contains(&json!([[1, 10], [2, 21]])), "{:?}", b[1]);
    assert!(at(&b[1]).contains(&json!([[1, 10], [2, 20]])));
    assert!(at(&a[1]).contains(&json!([[1, 10], [2, 20]])), "{:?}", a[1]);
    assert!(
        !a[1].to_string().contains("21"),
        "ann sees nothing of WidgetB: {:?}",
        a[1]
    );
    assert_eq!(a[1]["version"], b[1]["version"]);
}

#[tokio::test]
async fn a_change_nobody_may_see_sends_nothing_and_resume_catches_up() {
    let (app, state, _) = served();
    let ann = stream(&app, ANN, None).await;
    let first = events(ann, 1).await;
    let v0 = first[0]["version"].as_str().unwrap().to_owned();
    // Bob edits WidgetB only: ann's stream stays quiet.
    let hidden = json!({"edits": [{"measure": 100, "at": [[2, 21]], "value": 3}]});
    call(&app, req("POST", "/v1/cells", BOB, Some(hidden))).await;
    let ann = stream(&app, ANN, Some(&v0)).await;
    let quiet = tokio::time::timeout(Duration::from_millis(800), events(ann, 1)).await;
    assert!(quiet.is_err(), "no event for a hidden change: {quiet:?}");
    // Dropped mid-edit: ann reconnects with the version she saw and gets
    // what changed since, filtered.
    let seen = {
        let a = state.gui.authority.lock().unwrap();
        a.as_ref().unwrap().version()
    };
    let shown = json!({"edits": [{"measure": 101, "at": [[1, 11], [2, 20]], "value": 9}]});
    call(&app, req("POST", "/v1/cells", BOB, Some(shown))).await;
    let resumed = events(stream(&app, ANN, Some(&seen)).await, 1).await;
    let cells = resumed[0]["cells"].as_array().unwrap();
    assert!(
        cells
            .iter()
            .any(|c| c["measure"] == 101 && c["value"] == 9.0),
        "{resumed:?}"
    );
    // From an unknown version: reload.
    let r = events(stream(&app, ANN, Some("12345")).await, 1).await;
    assert_eq!(r[0]["reload"], true);
    // A structural change: reload.
    let ann = stream(&app, ANN, None).await;
    call(
        &app,
        req(
            "POST",
            "/v1/categories",
            BOB,
            Some(json!({"name": "Region"})),
        ),
    )
    .await;
    let e = events(ann, 2).await;
    assert_eq!(e[1]["reload"], true, "{e:?}");
    let _ = (MeasureId(0), CategoryId(0));
}

#[tokio::test]
async fn another_programs_write_reaches_the_stream() {
    let (app, _, path) = served();
    let bob = stream(&app, BOB, None).await;
    let mut other = improv_store::Store::open(path.to_str().unwrap()).unwrap();
    let mut m = other.load_model().unwrap();
    m.add_category(CategoryId(9), "Region");
    other.save_model(&m).unwrap();
    let e = events(bob, 2).await;
    assert_eq!(e[1]["reload"], true, "{e:?}");
}

/// After another program writes the file, this server's own next edit is a
/// change of its own: a new version, delivered with its cells. (Mentat's
/// cached ids made the edit reuse the other program's transaction, so the
/// version did not move and the stream never sent it.)
#[tokio::test]
async fn an_edit_after_another_programs_write_reaches_the_stream() {
    let (app, _, path) = served();
    let mut bob = stream(&app, BOB, None).await.into_data_stream();
    // Another program changes one cell: one transaction.
    let mut other = improv_store::Store::open(path.to_str().unwrap()).unwrap();
    let mut m = other.load_model().unwrap();
    let at = improv_core_model::Coordinate::from_pairs([
        (CategoryId(1), ItemId(10)),
        (CategoryId(2), ItemId(20)),
    ]);
    m.set_input(
        MeasureId(101),
        at.clone(),
        improv_core_model::Value::Number(77.0),
    );
    other.save_cells(&m, &[(MeasureId(101), at)]).unwrap();
    let (st, _) = call(&app, req("GET", "/v1/structure", BOB, None)).await;
    assert_eq!(st, StatusCode::OK);
    let first = next_events(&mut bob, 2).await;
    assert_eq!(first[1]["reload"], true, "{first:?}");
    // This server's own edit after it.
    let edit = json!({"edits": [{"measure": 101, "at": [[1, 11], [2, 20]], "value": 5}]});
    let (s, done) = call(&app, req("POST", "/v1/cells", BOB, Some(edit))).await;
    assert_eq!(s, StatusCode::OK, "{done}");
    assert_ne!(
        done["version"], first[1]["version"],
        "the edit is a new version"
    );
    let e = next_events(&mut bob, 1).await;
    assert!(
        e[0]["cells"]
            .as_array()
            .is_some_and(|c| c.iter().any(|c| c["measure"] == 101 && c["value"] == 5.0)),
        "{e:?}"
    );
}

/// The next `n` events of an open stream.
async fn next_events(
    stream: &mut (impl futures_util::Stream<Item = Result<axum::body::Bytes, axum::Error>> + Unpin),
    n: usize,
) -> Vec<JsonValue> {
    use futures_util::StreamExt;
    let mut buf = String::new();
    let mut out = Vec::new();
    while out.len() < n {
        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("an event within 5 s")
            .expect("the stream is open")
            .unwrap();
        buf.push_str(std::str::from_utf8(&chunk).unwrap());
        while let Some(end) = buf.find("\n\n") {
            let frame: String = buf.drain(..end + 2).collect();
            for line in frame.lines() {
                if let Some(d) = line.strip_prefix("data:") {
                    out.push(serde_json::from_str(d.trim()).unwrap());
                }
            }
        }
    }
    out
}

/// Another program's write that a request's own refresh finds first (not
/// the poller) still reaches the streams: whatever moves the authority's
/// version wakes them.
#[tokio::test]
async fn an_outside_write_a_request_finds_first_reaches_the_stream() {
    let (app, _, path) = served();
    let bob = stream(&app, BOB, None).await;
    let mut other = improv_store::Store::open(path.to_str().unwrap()).unwrap();
    let mut m = other.load_model().unwrap();
    m.add_category(CategoryId(9), "Region");
    other.save_model(&m).unwrap();
    // At once, well before the poller's first look (every 500 ms).
    let (st, _) = call(&app, req("GET", "/v1/structure", ANN, None)).await;
    assert_eq!(st, StatusCode::OK);
    let e = events(bob, 2).await;
    assert_eq!(e[1]["reload"], true, "{e:?}");
}

/// An open stream's response, kept open.
async fn open_stream(app: &axum::Router, token: &str) -> Result<Body, StatusCode> {
    let resp = app
        .clone()
        .oneshot(req("GET", "/v1/changes", token, None))
        .await
        .unwrap();
    match resp.status() {
        StatusCode::OK => Ok(resp.into_body()),
        s => Err(s),
    }
}

#[tokio::test]
async fn change_streams_are_capped_in_all_and_per_caller() {
    let (_, state, _) = served();
    let mut st = Arc::into_inner(state).unwrap();
    st.max_streams = 10;
    let app = app(Arc::new(st));
    let mut bobs = Vec::new();
    for _ in 0..crate::gui::MAX_STREAMS_PER_CALLER {
        bobs.push(open_stream(&app, BOB).await.expect("under the cap"));
    }
    assert_eq!(
        open_stream(&app, BOB).await.err(),
        Some(StatusCode::TOO_MANY_REQUESTS)
    );
    // Another caller still gets in, up to the total.
    let a1 = open_stream(&app, ANN).await.expect("ann's first");
    let a2 = open_stream(&app, ANN).await.expect("ann's second");
    assert_eq!(
        open_stream(&app, ANN).await.err(),
        Some(StatusCode::TOO_MANY_REQUESTS)
    );
    // A closed stream frees its slot.
    drop(bobs.pop());
    let back = open_stream(&app, ANN).await.expect("a slot came free");
    drop((a1, a2, back, bobs));
    assert!(
        open_stream(&app, BOB).await.is_ok(),
        "all closed: bob again"
    );
}

/// One shared poller, not one per stream: N idle streams read the store
/// about once a POLL between them, and none blocks other requests.
#[tokio::test]
async fn idle_streams_share_one_poller_and_take_no_request_slots() {
    // More streams than MAX_IN_FLIGHT requests could hold at once, from
    // enough callers to stay under the per-caller cap.
    let path = temp_path("gui.db");
    improv_store::Store::open(path.to_str().unwrap())
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    let n = crate::MAX_IN_FLIGHT + 8;
    let callers = n.div_ceil(crate::gui::MAX_STREAMS_PER_CALLER);
    let secrets: Vec<String> = (0..callers).map(|i| format!("caller-secret-{i}")).collect();
    let auth = Auth::Tokens(
        secrets
            .iter()
            .enumerate()
            .map(|(i, secret)| Token {
                name: format!("c{i}"),
                scope: Scope::Read,
                secret: secret.clone(),
            })
            .collect(),
    );
    let mut st = AppState::new(path.to_str().unwrap().into(), auth, None);
    st.configure(&|k: &str| (k == "IMPROV_RATE_LIMIT").then(|| "0".to_string()))
        .unwrap();
    let state = Arc::new(st);
    let app = app(state.clone());
    let mut open = Vec::new();
    for i in 0..n {
        open.push(
            open_stream(&app, &secrets[i % callers])
                .await
                .expect("under the caps"),
        );
    }
    assert!(open.len() > crate::MAX_IN_FLIGHT);
    let probes = || state.gui.authority.lock().unwrap().as_ref().unwrap().probes;
    let before = probes();
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let reads = probes() - before;
    assert!(
        reads <= 5,
        "{} streams read the store {reads} times in 1.6 s",
        open.len()
    );
    // Other requests go through at once.
    let t0 = std::time::Instant::now();
    let (s, _) = call(&app, req("GET", "/v1/structure", &secrets[0], None)).await;
    assert_eq!(s, StatusCode::OK);
    assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
    // The poller ends with the last stream.
    drop(open);
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert!(!state.gui.streams.lock().unwrap().polling);
}

/// A stream outlives REQUEST_TIMEOUT: the timeout layer stops at the
/// response headers. The same stream still delivers an edit made after
/// the deadline.
#[tokio::test]
async fn a_change_stream_outlives_the_request_timeout() {
    use futures_util::StreamExt;
    let (_, state, _) = served();
    let mut st = Arc::into_inner(state).unwrap();
    // Opened first: the stream's first request would otherwise also load
    // the model and build the engine, which can pass a short deadline on
    // a slow runner (Windows answered 408 before the stream began).
    super::with_authority(&st, |_| Ok(())).unwrap();
    st.request_timeout = Duration::from_millis(1000);
    let app = app(Arc::new(st));
    let body = open_stream(&app, BOB).await.expect("open");
    let mut data = body.into_data_stream();
    let hello = data.next().await.expect("the first event").unwrap();
    assert!(std::str::from_utf8(&hello).unwrap().contains("version"));
    // Past the deadline (a short one, set for this test).
    tokio::time::sleep(Duration::from_millis(1300)).await;
    let edit = json!({"edits": [{"measure": 101, "at": [[1, 10], [2, 20]], "value": 3}]});
    let (s, _) = call(&app, req("POST", "/v1/cells", ANN, Some(edit))).await;
    assert_eq!(s, StatusCode::OK);
    let next = tokio::time::timeout(Duration::from_secs(5), data.next())
        .await
        .expect("an event after the deadline")
        .expect("the stream is still open")
        .unwrap();
    assert!(
        std::str::from_utf8(&next).unwrap().contains("\"cells\""),
        "{next:?}"
    );
}

/// The same filtering with the model's stored Cedar policies and no test
/// hook: what the server does in production. Ann (analyst without
/// WidgetB) gets her part of bob's edit, nothing of WidgetB, and no
/// event at all for a change only to WidgetB.
#[tokio::test]
async fn the_change_stream_is_filtered_by_the_stored_cedar_policies() {
    let path = temp_path("cedar-gui.db");
    let p = path.to_str().unwrap();
    let mut s = improv_store::Store::open(p).unwrap();
    s.save_model(&revenue_model()).unwrap();
    let who = |n: &str| improv_policy::templates::Who::User(n.into());
    let r = improv_policy::templates::render;
    let text = [
        r("analyst", &who("ann"), &[], &[ItemId(21)]).unwrap(),
        r("admin", &who("bob"), &[], &[]).unwrap(),
    ]
    .join("\n");
    s.save_policies(Some((&text, ""))).unwrap();
    let tok = |n: &str, secret: &str| Token {
        name: n.into(),
        scope: Scope::Write,
        secret: secret.into(),
    };
    let auth = Auth::Tokens(vec![tok("ann", ANN), tok("bob", BOB)]);
    let app = app(Arc::new(AppState::new(p.into(), auth, None)));
    let ann = stream(&app, ANN, None).await;
    let first = events(ann, 1).await;
    let v0 = first[0]["version"].as_str().unwrap().to_owned();
    let both = json!({"edits": [
        {"measure": 101, "at": [[1, 10], [2, 20]], "value": 7},
        {"measure": 101, "at": [[1, 10], [2, 21]], "value": 8}]});
    let (st, _) = call(&app, req("POST", "/v1/cells", BOB, Some(both))).await;
    assert_eq!(st, StatusCode::OK);
    let got = events(stream(&app, ANN, Some(&v0)).await, 1).await;
    let text = got[0].to_string();
    assert!(text.contains("[1,10],[2,20]"), "{text}");
    assert!(!text.contains("21"), "nothing of WidgetB: {text}");
    // An edit of WidgetB only: ann's stream stays quiet.
    let v1 = got[0]["version"].as_str().unwrap().to_owned();
    let hidden = json!({"edits": [{"measure": 100, "at": [[2, 21]], "value": 3}]});
    call(&app, req("POST", "/v1/cells", BOB, Some(hidden))).await;
    let quiet = tokio::time::timeout(
        Duration::from_millis(800),
        events(stream(&app, ANN, Some(&v1)).await, 1),
    )
    .await;
    assert!(quiet.is_err(), "no event for a hidden change: {quiet:?}");
    // And ann may not edit at all.
    let mine = json!({"edits": [{"measure": 101, "at": [[1, 10], [2, 20]], "value": 1}]});
    let (st, _) = call(&app, req("POST", "/v1/cells", ANN, Some(mine))).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// The per-caller stream cap counts by the principal the policies see
/// (`Caller::principal().user`, the token's name), never by the secret:
/// the cap's table holds bob's name and no secret, and a read token is
/// its own caller.
#[tokio::test]
async fn the_stream_cap_keys_on_the_principal_not_the_token() {
    let (app, state, _) = served();
    let b = open_stream(&app, BOB).await.expect("bob's stream");
    let r = open_stream(&app, RO).await.expect("ro's stream");
    {
        let s = state.gui.streams.lock().unwrap();
        let mut keys: Vec<&String> = s.per.keys().collect();
        keys.sort();
        assert_eq!(keys, ["bob", "ro"]);
        assert!(s.per.keys().all(|k| k != BOB && k != RO), "no secret");
    }
    let caller = crate::Caller {
        name: "bob".into(),
        scope: Scope::Write,
    };
    assert_eq!(caller.principal().user, "bob");
    drop((b, r));
}
