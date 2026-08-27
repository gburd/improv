//! Conversions between the Improv model and Mentat EDN / `TypedValue`.

use crate::{Result, StoreError};
use improv_core_model::{
    Category, Coordinate, Item, Measure, MeasureId, MeasureKind, Value, ValueType,
};
use mentat::TypedValue;

/// Escape a Rust string as an EDN string literal (quotes + backslashes).
fn edn_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
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

pub fn cell_edn(measure: MeasureId, coord: &Coordinate, value: &Value) -> Result<String> {
    let coord_json = serde_json::to_string(coord)?;
    // A stable unique key so re-saving a cell updates in place.
    let key = format!("{}::{}", measure.0, coord_json);

    let mut fields = vec![
        format!(":cell/key {}", edn_str(&key)),
        format!(":cell/measure (lookup-ref :measure/id {})", measure.0),
        format!(":cell/coord {}", edn_str(&coord_json)),
    ];

    match value {
        Value::Number(n) => fields.push(format!(":cell/value-number {}", fmt_f64(*n))),
        Value::Boolean(b) => fields.push(format!(":cell/value-boolean {}", b)),
        Value::Text(t) => fields.push(format!(":cell/value-text {}", edn_str(t))),
        Value::Enum(e) => fields.push(format!(":cell/value-number {}", fmt_f64(*e as f64))),
        Value::DateTime(dt) => {
            fields.push(format!(":cell/value-text {}", edn_str(&dt.to_rfc3339())))
        }
        Value::Error(_) => {
            // Errors are computed, never stored as inputs.
            return Err(StoreError::Integrity(
                "cannot persist an error value as input".into(),
            ));
        }
    }

    Ok(format!("{{{}}}", fields.join(" ")))
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
