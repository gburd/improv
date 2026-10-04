//! Conversions between the Improv model and Mentat EDN / `TypedValue`.

use crate::{Result, StoreError};
use improv_core_model::{Category, Item, Measure, MeasureId, MeasureKind, Value, ValueType};
use mentat::TypedValue;

/// Escape a Rust string as an EDN string literal (quotes + backslashes).
fn edn_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            // Newlines, tabs and CRs are emitted RAW, not as \n / \t / \r.
            // They are legal unescaped inside an EDN string literal, and the
            // reader returns them verbatim, so raw is correct and lossless.
            //
            // This used to be a workaround: mentat's reader echoed the
            // character after a backslash instead of translating it, so an
            // emitted \n came back as the letter 'n' and "a\nb" round-tripped
            // as "anb". That is fixed upstream (mentat 1af15e15, pinned in
            // [workspace.dependencies]), so escaping them would now also work
            // -- raw simply stays the simpler of two correct options.
            // Regression: control_characters_survive_a_round_trip.
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

fn value_type_kw(vt: ValueType) -> &'static str {
    match vt {
        ValueType::Number => "number",
        ValueType::Boolean => "boolean",
        ValueType::Text => "text",
        ValueType::DateTime => "datetime",
        ValueType::Enum => "enum",
    }
}

pub fn value_type_from_kw(s: &str) -> Result<ValueType> {
    Ok(match s {
        "number" => ValueType::Number,
        "boolean" => ValueType::Boolean,
        "text" => ValueType::Text,
        "datetime" => ValueType::DateTime,
        "enum" => ValueType::Enum,
        other => return Err(StoreError::Integrity(format!("unknown value type {other}"))),
    })
}

pub fn category_edn(c: &Category) -> String {
    format!(
        "{{:category/id {} :category/name {}}}",
        c.id.0,
        edn_str(&c.name.0)
    )
}

pub fn item_edn(i: &Item) -> String {
    // :item/category is a ref, resolved by lookup-ref on the unique :category/id.
    format!(
        "{{:item/id {} :item/name {} :item/category (lookup-ref :category/id {})}}",
        i.id.0,
        edn_str(&i.name.0),
        i.category.0
    )
}

pub fn measure_edn(
    m: &Measure,
    sql_source: Option<&improv_core_model::SqlSource>,
) -> Result<String> {
    let mut fields = vec![
        format!(":measure/id {}", m.id.0),
        format!(":measure/name {}", edn_str(&m.name.0)),
        format!(
            ":measure/value-type {}",
            edn_str(value_type_kw(m.value_type))
        ),
    ];

    match &m.kind {
        MeasureKind::Input => fields.push(format!(":measure/kind {}", edn_str("input"))),
        MeasureKind::Derived(f) => {
            fields.push(format!(":measure/kind {}", edn_str("derived")));
            let json = serde_json::to_string(f)?;
            fields.push(format!(":measure/formula {}", edn_str(&json)));
        }
    }

    if let Some(desc) = &m.description {
        fields.push(format!(":measure/description {}", edn_str(desc)));
    }

    // SQL-source metadata (Phase 7 live-query measures), as a JSON blob.
    if let Some(src) = sql_source {
        let json = serde_json::to_string(src)?;
        fields.push(format!(":measure/sql-source {}", edn_str(&json)));
    }

    // Category refs (cardinality-many) via lookup-refs.
    if !m.categories.is_empty() {
        let refs: Vec<String> = m
            .categories
            .iter()
            .map(|c| format!("(lookup-ref :category/id {})", c.0))
            .collect();
        fields.push(format!(":measure/categories [{}]", refs.join(" ")));
    }

    Ok(format!("{{{}}}", fields.join(" ")))
}

/// One cell entity's EDN map, its value already rendered by
/// [`cell_value_edn`].
pub fn cell_edn(
    measure: MeasureId,
    coord_json: &str,
    key: &str,
    attr: &str,
    value_edn: &str,
) -> String {
    format!(
        "{{:cell/key {} :cell/measure (lookup-ref :measure/id {}) :cell/coord {} :cell/{attr} {value_edn}}}",
        edn_str(key),
        measure.0,
        edn_str(coord_json),
    )
}

/// A cell's stable unique `:cell/key` (measure id + coord JSON), so re-saving
/// a cell updates it in place.
pub fn cell_key(measure: MeasureId, coord_json: &str) -> String {
    format!("{}::{}", measure.0, coord_json)
}

/// The typed `:cell/<attr>` a value is stored under, and its EDN literal.
pub fn cell_value_edn(value: &Value) -> Result<(&'static str, String)> {
    Ok(match value {
        Value::Number(n) => (crate::VALUE_NUMBER, fmt_f64(*n)),
        Value::Boolean(b) => (crate::VALUE_BOOLEAN, b.to_string()),
        Value::Text(t) => (crate::VALUE_TEXT, edn_str(t)),
        Value::Enum(e) => (crate::VALUE_NUMBER, fmt_f64(f64::from(*e))),
        Value::DateTime(dt) => (crate::VALUE_TEXT, edn_str(&dt.to_rfc3339())),
        Value::Error(_) => {
            // Errors are computed, never stored as inputs.
            return Err(StoreError::Integrity(
                "cannot persist an error value as input".into(),
            ));
        }
    })
}

/// The EDN literal of a stored cell value, formatted exactly as
/// [`cell_value_edn`] would write it (so the two compare equal when unchanged,
/// and a retraction names the stored value exactly).
pub fn stored_value_edn(v: &TypedValue) -> Result<String> {
    match v {
        TypedValue::Double(d) => Ok(fmt_f64(d.0)),
        TypedValue::Boolean(b) => Ok(b.to_string()),
        TypedValue::String(s) => Ok(edn_str(s)),
        other => Err(StoreError::Integrity(format!(
            "unexpected cell value {other:?}"
        ))),
    }
}

/// An entity id (a `?e` or ref binding).
pub fn as_entid(v: &TypedValue) -> Result<i64> {
    match v {
        TypedValue::Ref(e) => Ok(*e),
        other => Err(StoreError::Integrity(format!(
            "expected entity, got {other:?}"
        ))),
    }
}

/// Format an f64 so it always parses back as a double in EDN (never bare int).
fn fmt_f64(n: f64) -> String {
    if n == n.trunc() && n.is_finite() {
        format!("{:.1}", n)
    } else {
        format!("{}", n)
    }
}

// --- TypedValue extractors (load path) ---

pub fn as_u32(v: &TypedValue) -> Result<u32> {
    match v {
        TypedValue::Long(n) => Ok(*n as u32),
        TypedValue::Ref(n) => Ok(*n as u32),
        other => Err(StoreError::Integrity(format!(
            "expected long, got {other:?}"
        ))),
    }
}

pub fn as_f64(v: &TypedValue) -> Result<f64> {
    match v {
        TypedValue::Double(d) => Ok(d.0),
        TypedValue::Long(n) => Ok(*n as f64),
        other => Err(StoreError::Integrity(format!(
            "expected double, got {other:?}"
        ))),
    }
}

pub fn as_bool(v: &TypedValue) -> Result<bool> {
    match v {
        TypedValue::Boolean(b) => Ok(*b),
        other => Err(StoreError::Integrity(format!(
            "expected bool, got {other:?}"
        ))),
    }
}

pub fn as_string(v: &TypedValue) -> Result<String> {
    match v {
        TypedValue::String(s) => Ok(s.as_ref().clone()),
        other => Err(StoreError::Integrity(format!(
            "expected string, got {other:?}"
        ))),
    }
}

/// Public EDN-string escaper, used by the load path to build queries safely.
pub fn edn_str_pub(s: &str) -> String {
    edn_str(s)
}
