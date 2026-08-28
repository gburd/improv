//! `improv-server`: a small JSON HTTP API over an Improv model store.
//!
//! State is just the DB path; every request opens the (blocking SQLite-backed)
//! Mentat store inside `spawn_blocking` and loads the model. Models are small in
//! v1, so open-per-request keeps the store off the async runtime and avoids
//! sharing a non-Send store across tasks.

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::{Path, Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use improv_core_model::{Coordinate, MeasureId, MeasureKind, Model};
use improv_engine::dataflow::evaluate;
use improv_engine::session::{Edit, Engine};
use improv_engine::{decode_coord, CoordKey};
use improv_nl_formula::{describe_formula, parse_nl_formula, NlContext};
use improv_storage_mentat::ModelStore;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};

/// Accepted bearer tokens, or open mode.
///
/// `Disabled` = no auth (local dev / tests, preserves v1 behavior);
/// `Tokens` = require `Authorization: Bearer <t>` for a `t` in the set.
#[derive(Clone)]
pub enum Auth {
    Disabled,
    Tokens(HashSet<String>),
}

impl Auth {
    /// Build from `IMPROV_API_TOKEN` (single) + `IMPROV_API_TOKENS` (comma-sep).
    /// Empty/whitespace tokens are ignored; no tokens => `Disabled`.
    fn from_env() -> Self {
        let mut tokens = HashSet::new();
        if let Ok(t) = std::env::var("IMPROV_API_TOKEN") {
            let t = t.trim();
            if !t.is_empty() {
                tokens.insert(t.to_string());
            }
        }
        if let Ok(list) = std::env::var("IMPROV_API_TOKENS") {
            for t in list.split(',').map(str::trim).filter(|t| !t.is_empty()) {
                tokens.insert(t.to_string());
            }
        }
        if tokens.is_empty() {
            Auth::Disabled
        } else {
            Auth::Tokens(tokens)
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    db_path: String,
    auth: Auth,
}

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
    let state = Arc::new(AppState { db_path, auth });
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("bind {addr}: {e}"));
    eprintln!("improv-server listening on {addr}");
    axum::serve(listener, app(state)).await.expect("serve");
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
        .route_layer(middleware::from_fn_with_state(state.clone(), require_token));

    Router::new()
        .route("/health", get(health))
        .merge(protected)
        .with_state(state)
}

/// Bearer-token gate. In open mode every request passes. Otherwise:
/// missing/blank `Authorization` -> 401; present but not an accepted token -> 403.
/// The token value is never logged.
async fn require_token(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let tokens = match &state.auth {
        Auth::Disabled => return next.run(req).await,
        Auth::Tokens(t) => t,
    };
    let presented = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty());
    match presented {
        None => (StatusCode::UNAUTHORIZED, "missing bearer token").into_response(),
        Some(t) if tokens.contains(t) => next.run(req).await,
        Some(_) => (StatusCode::FORBIDDEN, "invalid bearer token").into_response(),
    }
}

// --- error handling: never panic on request input ---

enum ApiError {
    BadRequest(String),
    NotFound(String),
    Internal(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, m),
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (status, Json(json!({ "error": msg }))).into_response()
    }
}

/// Open the store and load the model on a blocking thread.
async fn load_model(state: &Arc<AppState>) -> Result<Model, ApiError> {
    let path = state.db_path.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = ModelStore::open(&path).map_err(|e| e.to_string())?;
        store.load_model().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("task join: {e}")))?
    .map_err(ApiError::Internal)
}

// --- handlers ---

async fn health() -> &'static str {
    "ok"
}

async fn get_model(State(state): State<Arc<AppState>>) -> Result<Json<Model>, ApiError> {
    Ok(Json(load_model(&state).await?))
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
    Path(id): Path<u32>,
    body: Result<Json<EvalReq>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<Vec<Cell>>, ApiError> {
    let Json(req) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let mid = MeasureId(id);

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

    // Persist first (so a persisted edit is durable even if the caller only
    // wants the side effect); the engine below then sees the same values.
    if req.persist {
        for e in &req.edits {
            let coord = decode_coord(&e.coord);
            match e.value {
                Some(v) => model.set_input(
                    MeasureId(e.measure),
                    coord,
                    improv_core_model::Value::Number(v),
                ),
                None => {
                    model.inputs.remove(&(MeasureId(e.measure), coord));
                }
            }
        }
        let path = state.db_path.clone();
        let to_save = model.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = ModelStore::open(&path).map_err(|e| e.to_string())?;
            store.save_model(&to_save).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| ApiError::Internal(format!("task join: {e}")))?
        .map_err(ApiError::Internal)?;
    }

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
    let values = tokio::task::spawn_blocking(move || {
        let (mut engine, _initial) = Engine::new(&model, &targets).map_err(|e| e.to_string())?;
        let mut snap = engine.apply(edits).map_err(|e| e.to_string())?;
        Ok::<_, String>(snap.remove(&mid).unwrap_or_default())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("task join: {e}")))?
    .map_err(ApiError::Internal)?;

    // Re-borrow the model for coord rendering: load it once more (cheap, small).
    let model = load_model(&state).await?;
    let cells = values
        .into_iter()
        .map(|(k, n)| readable_key(&model, &k, n.as_num().unwrap_or(f64::NAN)))
        .collect();
    Ok(Json(cells))
}

#[derive(Deserialize)]
struct NlParseReq {
    text: String,
}

async fn nl_parse(
    State(state): State<Arc<AppState>>,
    body: Result<Json<NlParseReq>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<JsonValue>, ApiError> {
    let Json(req) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
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
    body: Result<Json<NlDescribeReq>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<JsonValue>, ApiError> {
    let Json(req) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
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
    body: Result<Json<SetCellReq>, axum::extract::rejection::JsonRejection>,
) -> Result<Json<JsonValue>, ApiError> {
    let Json(req) = body.map_err(|e| ApiError::BadRequest(e.body_text()))?;
    let mid = MeasureId(id);

    let mut model = load_model(&state).await?;
    let measure = model
        .measures
        .get(&mid)
        .ok_or_else(|| ApiError::NotFound(format!("no measure {id}")))?;
    if !measure.is_input() {
        return Err(ApiError::BadRequest(format!(
            "measure {id} is derived; cannot set a cell"
        )));
    }
    model.set_input(mid, req.coord, improv_core_model::Value::Number(req.value));

    let path = state.db_path.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = ModelStore::open(&path).map_err(|e| e.to_string())?;
        store.save_model(&model).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| ApiError::Internal(format!("task join: {e}")))?
    .map_err(ApiError::Internal)?;

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
        let state = Arc::new(AppState {
            db_path: path.to_str().unwrap().to_string(),
            auth: Auth::Disabled,
        });
        (app(state), path)
    }

    // Same seeded store, but auth enabled with a single accepted token.
    fn authed_app(token: &str) -> (Router, std::path::PathBuf) {
        let (_, path) = seeded_app();
        let state = Arc::new(AppState {
            db_path: path.to_str().unwrap().to_string(),
            auth: Auth::Tokens(HashSet::from([token.to_string()])),
        });
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
}
