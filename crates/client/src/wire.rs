//! The `/v1` API a GUI drives, as Rust types: what the browser sends and
//! receives, what `improv-server` parses and answers, and what a local
//! model service exchanges with its caller. One definition, so the two
//! sides cannot drift.
//!
//! Shaped for a filtered view (`docs/reviews/2026-11-02-cedar-policies.md`,
//! "The model-service contract"): a client holds only what it may see.
//! Hidden measures, items and cells are **absent** (no placeholder, no
//! count); [`Structure`] may be partial; cells are read a window at a time
//! and derived values always come from the authority; an edit batch is
//! refused whole, naming the cell, with the same words for a hidden cell
//! as for one that does not exist.

use std::collections::BTreeMap;

use improv_core_model::{
    CategoryId, Expr, Formula, ItemId, Measure, MeasureId, MeasureKind, Model, Name, NumberFormat,
    Value, ValueType, View,
};
use serde::{Deserialize, Serialize};

/// A cell's value on the wire: a JSON number, boolean or string, or an
/// object for a date (`{"date": "2025-01-31T00:00:00+00:00"}`) or an error
/// value (`{"error": "#DIV/0!"}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum WireValue {
    Number(f64),
    Bool(bool),
    Text(String),
    Date { date: String },
    Error { error: String },
}

impl WireValue {
    /// The model value this is, for a measure of type `ty`; `Err` names
    /// what was wrong with it.
    pub fn to_value(&self, ty: ValueType) -> Result<Value, String> {
        match (self, ty) {
            (WireValue::Number(n), ValueType::Number) if n.is_finite() => Ok(Value::Number(*n)),
            (WireValue::Number(_), ValueType::Number) => Err("not a finite number".into()),
            (WireValue::Bool(b), ValueType::Boolean) => Ok(Value::Boolean(*b)),
            (WireValue::Text(t), ValueType::Text) => Ok(Value::Text(t.clone())),
            (WireValue::Date { date }, ValueType::DateTime) => {
                chrono::DateTime::parse_from_rfc3339(date)
                    .map(|d| Value::DateTime(d.with_timezone(&chrono::Utc)))
                    .map_err(|e| format!("not an RFC 3339 date: {e}"))
            }
            (_, ty) => Err(format!("this measure holds {}", type_name(ty))),
        }
    }

    /// The wire form of a model value.
    pub fn from_value(v: &Value) -> WireValue {
        match v {
            Value::Number(n) => WireValue::Number(*n),
            Value::Boolean(b) => WireValue::Bool(*b),
            Value::Text(t) => WireValue::Text(t.clone()),
            Value::DateTime(d) => WireValue::Date {
                date: d.to_rfc3339(),
            },
            Value::Enum(e) => WireValue::Number(f64::from(*e)),
            Value::Error(e) => WireValue::Error {
                error: e.kind.code().into(),
            },
        }
    }

    /// The cell as the grid shows it: a number in `format` and `locale`
    /// (as the desktop GUI shows it), the rest as text.
    pub fn shown(
        &self,
        format: Option<&NumberFormat>,
        locale: &improv_core_model::Locale,
    ) -> String {
        match self {
            WireValue::Number(n) => improv_core_model::format_number(*n, format, locale),
            WireValue::Bool(b) => b.to_string(),
            WireValue::Text(t) => t.clone(),
            WireValue::Date { date } => date.clone(),
            WireValue::Error { error } => error.clone(),
        }
    }

    /// The cell as its editor opens with it: what typing it back sets.
    pub fn typed(&self) -> String {
        match self {
            WireValue::Number(n) => improv_core_model::display_number(*n),
            other => other.shown(None, &improv_core_model::Locale::en()),
        }
    }
}

fn type_name(ty: ValueType) -> &'static str {
    match ty {
        ValueType::Number => "numbers",
        ValueType::Boolean => "true or false",
        ValueType::Text => "text",
        ValueType::DateTime => "dates",
        ValueType::Enum => "enum indexes",
    }
}

/// Parse a cell as typed into a measure of type `ty`: the type decides
/// (`42` in a text measure is text). Numbers are [`improv_core_model::Locale::parse_number`]'s
/// (`$1,500`); dates are a formula's date literals (`2025-01-31`, RFC 3339).
pub fn parse_typed(
    model: &Model,
    locale: &improv_core_model::Locale,
    ty: ValueType,
    text: &str,
) -> Result<WireValue, String> {
    let literal = |src: String| match improv_core_model::parser::parse_expr(model, &src) {
        Ok(f) => match f.expr {
            Expr::Literal(v) if v.type_of() == Some(ty) => Some(WireValue::from_value(&v)),
            _ => None,
        },
        Err(_) => None,
    };
    match ty {
        ValueType::Number => locale
            .parse_number(text)
            .map(WireValue::Number)
            .ok_or_else(|| format!("not a number: {text:?} ({})", locale.number_hint())),
        ValueType::Text => Ok(WireValue::Text(text.to_string())),
        ValueType::Boolean => literal(text.to_ascii_uppercase())
            .ok_or_else(|| format!("not a boolean (use true/false): {text:?}")),
        ValueType::DateTime => literal(format!("#{text}#"))
            .ok_or_else(|| format!("not a date (use YYYY-MM-DD or RFC3339): {text:?}")),
        ValueType::Enum => text
            .parse::<u32>()
            .map(|e| WireValue::Number(f64::from(e)))
            .map_err(|_| format!("not an enum index: {text:?}")),
    }
}

/// A cell's coordinate: `[[category, item], …]`, sorted by category id.
pub type At = Vec<(u32, u32)>;

/// What the client may see of the model's structure (`GET /v1/structure`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Structure {
    /// The model's version when it was read: opaque, compare for equality
    /// only.
    pub version: String,
    /// Categories in id order, each with its visible items in grid order.
    pub categories: Vec<CategoryOut>,
    /// Visible measures in id order.
    pub measures: Vec<MeasureOut>,
    /// Named LAMBDAs, in id order.
    #[serde(default)]
    pub lambdas: Vec<LambdaOut>,
    /// Saved views, with only the matrices, filters and pages of visible
    /// measures and items.
    #[serde(default)]
    pub views: Vec<View>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CategoryOut {
    pub id: u32,
    pub name: String,
    pub items: Vec<ItemOut>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ItemOut {
    pub id: u32,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MeasureOut {
    pub id: u32,
    pub name: String,
    #[serde(rename = "type")]
    pub value_type: ValueType,
    /// `input` or `derived`.
    pub kind: String,
    /// Its categories, by id, in its order.
    pub categories: Vec<u32>,
    /// A derived measure's formula as it would be typed, when shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    /// Why its formula does not build, when it does not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<NumberFormat>,
    /// A hint: whether this caller may edit its cells (an input, not filled
    /// from SQL or an external function, and allowed). Every edit is still
    /// checked.
    #[serde(default)]
    pub editable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LambdaOut {
    pub id: u32,
    pub name: String,
    pub params: Vec<String>,
}

impl Structure {
    /// A model with these categories, items, measures and LAMBDA names and
    /// no cells: what the formula editor's autocomplete and located errors
    /// need, and what the pivot code lays out. A derived measure's formula
    /// is a stand-in (`0`): the client never computes.
    pub fn to_model(&self) -> Model {
        let mut m = Model::new();
        for c in &self.categories {
            m.add_category(CategoryId(c.id), c.name.clone());
            for i in &c.items {
                m.add_item(ItemId(i.id), CategoryId(c.id), i.name.clone());
            }
        }
        for x in &self.measures {
            m.add_measure(Measure {
                id: MeasureId(x.id),
                name: Name(x.name.clone()),
                value_type: x.value_type,
                categories: x.categories.iter().copied().map(CategoryId).collect(),
                kind: match x.kind.as_str() {
                    "derived" => {
                        MeasureKind::Derived(Formula::new(Expr::Literal(Value::Number(0.0))))
                    }
                    _ => MeasureKind::Input,
                },
                description: None,
                format: x.format,
            });
        }
        for v in &self.views {
            m.add_view(v.clone());
        }
        m
    }

    pub fn measure(&self, id: u32) -> Option<&MeasureOut> {
        self.measures.iter().find(|m| m.id == id)
    }

    pub fn category(&self, id: u32) -> Option<&CategoryOut> {
        self.categories.iter().find(|c| c.id == id)
    }
}

/// `POST /v1/measures/{id}/window`: the cells of a measure at these items,
/// per category (a category left out: all its visible items).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowReq {
    #[serde(default)]
    pub at: BTreeMap<u32, Vec<u32>>,
}

/// One visible cell.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellOut {
    pub at: At,
    pub value: WireValue,
}

/// A window's visible cells, as the authority computed them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cells {
    pub version: String,
    pub measure: u32,
    pub cells: Vec<CellOut>,
}

/// One cell edit: set it, or clear it (`value` null or absent).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CellEdit {
    pub measure: u32,
    pub at: At,
    #[serde(default)]
    pub value: Option<WireValue>,
}

/// `POST /v1/cells`: edits applied together, or not at all. A batch has no
/// base version: when another writer moved the model, the authority reloads,
/// checks every edit again and applies them, so cell edits never conflict.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EditReq {
    pub edits: Vec<CellEdit>,
}

/// `POST /v1/measures`: a definition (`Revenue = Price * Quantity`, or
/// `Margin = LAMBDA(p, c, p - c)`), or a new input measure.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DefineReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub definition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// An input measure's categories, by id (default: all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<u32>>,
    /// An input measure's type (default: number).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub value_type: Option<ValueType>,
}

/// `PUT /v1/measures/{id}`: any of a new formula, name and format, as one
/// step.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UpdateMeasureReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub formula: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `Some(None)` clears the format (General).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "double_option"
    )]
    pub format: Option<Option<NumberFormat>>,
}

/// A name, for adding and renaming categories and items.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NameReq {
    pub name: String,
}

/// `POST /v1/views` and `PUT /v1/views/{id}`: a view's name and matrices
/// (the first is the primary one).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ViewReq {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub matrices: Option<Vec<improv_core_model::MatrixPlacement>>,
}

/// What a change did: the new version, a status line, and the id of what
/// it made.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Done {
    pub version: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
}

/// An error located in formula text: byte offsets into what was sent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagnosticOut {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span: Option<(usize, usize)>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl From<&crate::diagnostics::Diagnostic> for DiagnosticOut {
    fn from(d: &crate::diagnostics::Diagnostic) -> Self {
        DiagnosticOut {
            span: d.span.as_ref().map(|r| (r.start, r.end)),
            message: d.message.clone(),
            hint: d.hint.clone(),
        }
    }
}

impl From<&DiagnosticOut> for crate::diagnostics::Diagnostic {
    fn from(d: &DiagnosticOut) -> Self {
        crate::diagnostics::Diagnostic {
            span: d.span.map(|(a, b)| a..b),
            message: d.message.clone(),
            hint: d.hint.clone(),
        }
    }
}

/// A refused request's body: the message, the edit that failed (its index
/// in the batch), and where in a formula the error is.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Failure {
    pub error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<DiagnosticOut>,
}

/// One event of `GET /v1/changes`: cells that changed (already filtered
/// for the subscriber), or `reload`: re-read the structure and every cell.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangeEvent {
    pub version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cells: Vec<ChangedCell>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reload: bool,
}

/// A changed cell: its new value, or none (it is now blank).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangedCell {
    pub measure: u32,
    pub at: At,
    #[serde(default)]
    pub value: Option<WireValue>,
}

/// What a client asks the model service for. [`Request::http`] is how the
/// remote service sends it; the server parses each route back into one.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Structure,
    Window(u32, WindowReq),
    Edit(EditReq),
    Define(DefineReq),
    UpdateMeasure(u32, UpdateMeasureReq),
    DeleteMeasure(u32),
    AddCategory(NameReq),
    RenameCategory(u32, NameReq),
    DeleteCategory(u32),
    AddItem(u32, NameReq),
    RenameItem(u32, NameReq),
    DeleteItem(u32),
    SaveView(ViewReq),
    UpdateView(u32, ViewReq),
    DeleteView(u32),
    Undo,
    Redo,
}

/// What a request answers.
#[derive(Debug, Clone, PartialEq)]
pub enum Response {
    Structure(Structure),
    Cells(Cells),
    Done(Done),
}

impl Request {
    /// The HTTP method, `/v1` path and JSON body that send this request.
    pub fn http(&self) -> (&'static str, String, Option<serde_json::Value>) {
        use Request as R;
        let j = |v: &dyn erased::Ser| Some(v.json());
        match self {
            R::Structure => ("GET", "/v1/structure".into(), None),
            R::Window(m, w) => ("POST", format!("/v1/measures/{m}/window"), j(w)),
            R::Edit(e) => ("POST", "/v1/cells".into(), j(e)),
            R::Define(d) => ("POST", "/v1/measures".into(), j(d)),
            R::UpdateMeasure(m, u) => ("PUT", format!("/v1/measures/{m}"), j(u)),
            R::DeleteMeasure(m) => ("DELETE", format!("/v1/measures/{m}"), None),
            R::AddCategory(n) => ("POST", "/v1/categories".into(), j(n)),
            R::RenameCategory(c, n) => ("PUT", format!("/v1/categories/{c}"), j(n)),
            R::DeleteCategory(c) => ("DELETE", format!("/v1/categories/{c}"), None),
            R::AddItem(c, n) => ("POST", format!("/v1/categories/{c}/items"), j(n)),
            R::RenameItem(i, n) => ("PUT", format!("/v1/items/{i}"), j(n)),
            R::DeleteItem(i) => ("DELETE", format!("/v1/items/{i}"), None),
            R::SaveView(v) => ("POST", "/v1/views".into(), j(v)),
            R::UpdateView(id, v) => ("PUT", format!("/v1/views/{id}"), j(v)),
            R::DeleteView(id) => ("DELETE", format!("/v1/views/{id}"), None),
            R::Undo => ("POST", "/v1/undo".into(), None),
            R::Redo => ("POST", "/v1/redo".into(), None),
        }
    }

    /// Whether it changes the model (and so needs a write token).
    pub fn writes(&self) -> bool {
        !matches!(self, Request::Structure | Request::Window(..))
    }

    /// Parse a response body for this request.
    pub fn parse_reply(&self, body: &str) -> Result<Response, String> {
        let e = |e: serde_json::Error| format!("bad reply: {e}");
        Ok(match self {
            Request::Structure => Response::Structure(serde_json::from_str(body).map_err(e)?),
            Request::Window(..) => Response::Cells(serde_json::from_str(body).map_err(e)?),
            _ => Response::Done(serde_json::from_str(body).map_err(e)?),
        })
    }
}

/// `serde_json::to_value` behind a trait object, for [`Request::http`].
mod erased {
    pub trait Ser {
        fn json(&self) -> serde_json::Value;
    }
    impl<T: serde::Serialize> Ser for T {
        fn json(&self) -> serde_json::Value {
            serde_json::to_value(self).unwrap_or_default()
        }
    }
}

/// `Option<Option<T>>` where an explicit `null` is `Some(None)`.
mod double_option {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<T: Serialize, S: Serializer>(
        v: &Option<Option<T>>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        match v {
            Some(inner) => inner.serialize(s),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<T>>, D::Error> {
        Option::<T>::deserialize(d).map(Some)
    }
}

/// The model service: everything a GUI asks of the model, local or remote.
///
/// Calls do not block: [`ModelService::send`] starts a request and
/// [`ModelService::poll`] returns the replies and pushed changes that have
/// arrived (a frame loop calls it once a frame). A local service answers at
/// once; a remote one when the network does. Every call can be refused
/// (a [`Failure`]), and a GUI must expect that of every call: capabilities
/// in the [`Structure`] are hints.
pub trait ModelService {
    /// Start `req`; its reply comes back from [`ModelService::poll`] under
    /// the returned ticket.
    fn send(&mut self, req: Request) -> u64;
    /// Replies and changes that have arrived since the last call.
    fn poll(&mut self) -> Vec<Event>;
    /// Use `secret` (a bearer token) from now on, after
    /// [`Event::LoggedOut`]. Held in memory only. A local service needs
    /// none.
    fn authenticate(&mut self, secret: String) {
        let _ = secret;
    }
    /// Whether pushed changes are arriving (the change stream is
    /// connected). Always true in process.
    fn live(&self) -> bool {
        true
    }
}

/// What [`ModelService::poll`] returns.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Reply(u64, Result<Response, Failure>),
    Changed(ChangeEvent),
    /// The authority refused the credentials (HTTP 401): ask for them
    /// again. A local service never sends it.
    LoggedOut(String),
}

/// A cell as a client can know it.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell<'a> {
    /// It holds this value.
    Value(&'a WireValue),
    /// Nothing to show: empty, or not visible to this caller (the two are
    /// deliberately the same).
    Blank,
    /// Outside every window read so far: not asked for yet.
    NotLoaded,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_as_json() {
        for (v, json) in [
            (WireValue::Number(1.5), "1.5"),
            (WireValue::Bool(true), "true"),
            (WireValue::Text("hi".into()), "\"hi\""),
            (
                WireValue::Date {
                    date: "2025-01-31T00:00:00+00:00".into(),
                },
                "{\"date\":\"2025-01-31T00:00:00+00:00\"}",
            ),
            (
                WireValue::Error {
                    error: "#DIV/0!".into(),
                },
                "{\"error\":\"#DIV/0!\"}",
            ),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), json);
            assert_eq!(serde_json::from_str::<WireValue>(json).unwrap(), v);
        }
    }

    #[test]
    fn values_convert_by_the_measures_type() {
        let n = WireValue::Number(2.0);
        assert_eq!(n.to_value(ValueType::Number), Ok(Value::Number(2.0)));
        assert!(n.to_value(ValueType::Text).is_err());
        assert!(WireValue::Number(f64::NAN)
            .to_value(ValueType::Number)
            .is_err());
        let d = WireValue::Date {
            date: "2025-01-31T00:00:00+00:00".into(),
        };
        let v = d.to_value(ValueType::DateTime).unwrap();
        assert_eq!(WireValue::from_value(&v), d);
        assert!(WireValue::Date { date: "x".into() }
            .to_value(ValueType::DateTime)
            .is_err());
        assert_eq!(WireValue::Number(1500.0).typed(), "1500");
        let f = NumberFormat {
            decimals: Some(2),
            grouping: true,
        };
        let en = improv_core_model::Locale::en();
        assert_eq!(WireValue::Number(1500.0).shown(Some(&f), &en), "1,500.00");
    }

    #[test]
    fn typed_text_parses_by_type() {
        let m = Model::new();
        let en = improv_core_model::Locale::en();
        let p = |ty, t| parse_typed(&m, &en, ty, t);
        assert_eq!(
            p(ValueType::Number, "$1,500"),
            Ok(WireValue::Number(1500.0))
        );
        assert!(p(ValueType::Number, "abc").is_err());
        assert_eq!(p(ValueType::Text, "42"), Ok(WireValue::Text("42".into())));
        assert_eq!(p(ValueType::Boolean, "true"), Ok(WireValue::Bool(true)));
        assert!(p(ValueType::Boolean, "maybe").is_err());
        assert!(matches!(
            p(ValueType::DateTime, "2025-01-31"),
            Ok(WireValue::Date { .. })
        ));
        assert!(p(ValueType::DateTime, "soon").is_err());
        assert_eq!(p(ValueType::Enum, "3"), Ok(WireValue::Number(3.0)));
        assert!(p(ValueType::Enum, "x").is_err());
    }

    #[test]
    fn requests_map_to_routes_and_replies_parse() {
        let r = Request::Window(
            102,
            WindowReq {
                at: [(1, vec![10, 11])].into(),
            },
        );
        let (m, p, b) = r.http();
        assert_eq!((m, p.as_str()), ("POST", "/v1/measures/102/window"));
        assert_eq!(b.unwrap()["at"]["1"], serde_json::json!([10, 11]));
        assert!(!r.writes() && Request::Undo.writes());
        let cells =
            r.parse_reply(r#"{"version":"3","measure":102,"cells":[{"at":[[1,10]],"value":5}]}"#);
        assert!(
            matches!(cells, Ok(Response::Cells(c)) if c.cells[0].value == WireValue::Number(5.0))
        );
        assert!(Request::Structure.parse_reply("nope").is_err());
        let u = UpdateMeasureReq {
            format: Some(None),
            ..Default::default()
        };
        let json = serde_json::to_string(&u).unwrap();
        assert_eq!(json, r#"{"format":null}"#);
        assert_eq!(serde_json::from_str::<UpdateMeasureReq>(&json).unwrap(), u);
        assert_eq!(
            serde_json::from_str::<UpdateMeasureReq>("{}").unwrap(),
            UpdateMeasureReq::default()
        );
        for req in [
            Request::Structure,
            Request::Edit(EditReq::default()),
            Request::Define(DefineReq::default()),
            Request::UpdateMeasure(1, u),
            Request::DeleteMeasure(1),
            Request::AddCategory(NameReq::default()),
            Request::RenameCategory(1, NameReq::default()),
            Request::DeleteCategory(1),
            Request::AddItem(1, NameReq::default()),
            Request::RenameItem(1, NameReq::default()),
            Request::DeleteItem(1),
            Request::SaveView(ViewReq::default()),
            Request::UpdateView(1, ViewReq::default()),
            Request::DeleteView(1),
            Request::Undo,
            Request::Redo,
        ] {
            let (m, p, _) = req.http();
            assert!(p.starts_with("/v1/") && !m.is_empty(), "{req:?}");
            if req.writes() {
                let done = req.parse_reply(r#"{"version":"1","message":"ok"}"#);
                assert!(matches!(done, Ok(Response::Done(_))), "{req:?}");
            }
        }
    }

    #[test]
    fn a_structure_becomes_a_model_without_cells() {
        let s = Structure {
            version: "1".into(),
            categories: vec![CategoryOut {
                id: 1,
                name: "Time".into(),
                items: vec![ItemOut {
                    id: 10,
                    name: "2025".into(),
                }],
            }],
            measures: vec![
                MeasureOut {
                    id: 100,
                    name: "Price".into(),
                    value_type: ValueType::Number,
                    kind: "input".into(),
                    categories: vec![1],
                    formula: None,
                    error: None,
                    format: None,
                    editable: true,
                },
                MeasureOut {
                    id: 101,
                    name: "Twice".into(),
                    value_type: ValueType::Number,
                    kind: "derived".into(),
                    categories: vec![1],
                    formula: Some("Price * 2".into()),
                    error: None,
                    format: None,
                    editable: false,
                },
            ],
            lambdas: vec![],
            views: vec![],
        };
        let m = s.to_model();
        assert_eq!(m.measure_by_name("Price").unwrap().id, MeasureId(100));
        assert!(m.measures[&MeasureId(101)].is_derived());
        assert_eq!(m.categories[&CategoryId(1)].items, vec![ItemId(10)]);
        assert!(m.inputs.is_empty());
        assert_eq!(s.measure(101).unwrap().name, "Twice");
        assert_eq!(s.category(1).unwrap().items.len(), 1);
        assert!(s.measure(9).is_none() && s.category(9).is_none());
        let d = crate::diagnostics::Diagnostic {
            span: Some(2..5),
            message: "m".into(),
            hint: None,
        };
        let out = DiagnosticOut::from(&d);
        assert_eq!(out.span, Some((2, 5)));
        assert_eq!(crate::diagnostics::Diagnostic::from(&out), d);
    }
}
