//! Marshal `improv_core_model::Value` <-> JSON for the subprocess boundary.
//!
//! The Python side sees plain JSON: numbers, bools, strings, and (for
//! DateTime/Enum/Error) small tagged objects. Args go in as a JSON array; the
//! function returns exactly one JSON value which we map back and type-check.

use improv_core_model::{Value, ValueError, ValueErrorKind, ValueType};
use serde_json::{json, Value as J};

/// Encode one `Value` as JSON for stdin.
///
/// * Number  -> JSON number
/// * Boolean -> JSON bool
/// * Text    -> JSON string
/// * DateTime-> `{"__improv__":"datetime","value":"<rfc3339>"}`
/// * Enum    -> `{"__improv__":"enum","value":<u32>}`
/// * Error   -> `{"__improv__":"error","kind":"...","message":"..."}`
pub fn value_to_json(v: &Value) -> J {
    match v {
        Value::Number(n) => json!(n),
        Value::Boolean(b) => json!(b),
        Value::Text(s) => json!(s),
        Value::DateTime(dt) => json!({
            "__improv__": "datetime",
            "value": dt.to_rfc3339(),
        }),
        Value::Enum(i) => json!({ "__improv__": "enum", "value": i }),
        Value::Error(e) => json!({
            "__improv__": "error",
            "kind": err_kind_str(&e.kind),
            "message": err_message(&e.kind),
        }),
    }
}

/// Decode one JSON value returned by the function, validating it against the
/// declared return type. Returns a marshalling error string on mismatch.
pub fn json_to_value(j: &J, expected: ValueType) -> Result<Value, String> {
    match expected {
        ValueType::Number => j
            .as_f64()
            .map(Value::Number)
            .ok_or_else(|| format!("expected Number, got {}", short(j))),
        ValueType::Boolean => j
            .as_bool()
            .map(Value::Boolean)
            .ok_or_else(|| format!("expected Boolean, got {}", short(j))),
        ValueType::Text => j
            .as_str()
            .map(|s| Value::Text(s.to_string()))
            .ok_or_else(|| format!("expected Text, got {}", short(j))),
        ValueType::DateTime => {
            let s = tagged_str(j, "datetime")
                .or_else(|| j.as_str().map(str::to_string))
                .ok_or_else(|| format!("expected DateTime, got {}", short(j)))?;
            chrono::DateTime::parse_from_rfc3339(&s)
                .map(|dt| Value::DateTime(dt.with_timezone(&chrono::Utc)))
                .map_err(|e| format!("invalid DateTime {s:?}: {e}"))
        }
        ValueType::Enum => tagged_u64(j, "enum")
            .or_else(|| j.as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .map(Value::Enum)
            .ok_or_else(|| format!("expected Enum index, got {}", short(j))),
    }
}

/// Type-check an argument against its declared type (used before invocation).
/// `Value::Error` args are always rejected: external functions receive
/// well-typed inputs (the engine short-circuits errors before dispatch).
pub fn arg_matches(v: &Value, expected: ValueType) -> bool {
    v.type_of() == Some(expected)
}

fn tagged_str(j: &J, tag: &str) -> Option<String> {
    if j.get("__improv__")?.as_str()? == tag {
        j.get("value")?.as_str().map(str::to_string)
    } else {
        None
    }
}

fn tagged_u64(j: &J, tag: &str) -> Option<u64> {
    if j.get("__improv__")?.as_str()? == tag {
        j.get("value")?.as_u64()
    } else {
        None
    }
}

fn err_kind_str(k: &ValueErrorKind) -> &'static str {
    match k {
        ValueErrorKind::TypeMismatch => "type_mismatch",
        ValueErrorKind::DimensionMismatch => "dimension_mismatch",
        ValueErrorKind::DivisionByZero => "division_by_zero",
        ValueErrorKind::MissingInput => "missing_input",
        ValueErrorKind::Custom(_) => "custom",
    }
}

fn err_message(k: &ValueErrorKind) -> String {
    match k {
        ValueErrorKind::Custom(m) => m.clone(),
        other => err_kind_str(other).to_string(),
    }
}

/// A short, non-panicking preview of a JSON value for error messages.
fn short(j: &J) -> String {
    let s = j.to_string();
    if s.len() > 60 {
        format!("{}…", &s[..60])
    } else {
        s
    }
}

/// Convenience: build a `Value::Error` (used by callers mapping ExtFnError).
pub fn error_value(msg: impl Into<String>) -> Value {
    Value::Error(ValueError::new(ValueErrorKind::Custom(msg.into())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_scalars() {
        assert_eq!(
            json_to_value(&value_to_json(&Value::Number(2.5)), ValueType::Number).unwrap(),
            Value::Number(2.5)
        );
        assert_eq!(
            json_to_value(&value_to_json(&Value::Boolean(true)), ValueType::Boolean).unwrap(),
            Value::Boolean(true)
        );
        assert_eq!(
            json_to_value(&value_to_json(&Value::Text("hi".into())), ValueType::Text).unwrap(),
            Value::Text("hi".into())
        );
        assert_eq!(
            json_to_value(&value_to_json(&Value::Enum(7)), ValueType::Enum).unwrap(),
            Value::Enum(7)
        );
    }

    #[test]
    fn roundtrip_datetime() {
        let dt = chrono::DateTime::parse_from_rfc3339("2025-01-02T03:04:05Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let back =
            json_to_value(&value_to_json(&Value::DateTime(dt)), ValueType::DateTime).unwrap();
        assert_eq!(back, Value::DateTime(dt));
    }

    #[test]
    fn type_mismatch_is_error() {
        // A JSON string cannot become a Number.
        assert!(json_to_value(&json!("nope"), ValueType::Number).is_err());
        // A JSON number cannot become a Boolean.
        assert!(json_to_value(&json!(1), ValueType::Boolean).is_err());
    }

    #[test]
    fn arg_check() {
        assert!(arg_matches(&Value::Number(1.0), ValueType::Number));
        assert!(!arg_matches(&Value::Number(1.0), ValueType::Text));
        assert!(!arg_matches(&error_value("x"), ValueType::Number));
    }
}
