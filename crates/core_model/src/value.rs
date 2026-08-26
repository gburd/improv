//! Value types and runtime values, including error values that propagate
//! through the dataflow graph.

use crate::ids::MeasureId;
use serde::{Deserialize, Serialize};

/// The declared type of a measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ValueType {
    Number,
    Boolean,
    Text,
    DateTime,
    Enum,
}

/// A runtime value in a cell. `Error` lets structural/runtime errors flow
/// through the computation graph as first-class values (as the design requires).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Number(f64),
    Boolean(bool),
    Text(String),
    DateTime(chrono::DateTime<chrono::Utc>),
    /// Index into an enum domain.
    Enum(u32),
    Error(ValueError),
}

impl Value {
    pub fn type_of(&self) -> Option<ValueType> {
        match self {
            Value::Number(_) => Some(ValueType::Number),
            Value::Boolean(_) => Some(ValueType::Boolean),
            Value::Text(_) => Some(ValueType::Text),
            Value::DateTime(_) => Some(ValueType::DateTime),
            Value::Enum(_) => Some(ValueType::Enum),
            Value::Error(_) => None,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, Value::Error(_))
    }

    pub fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueErrorKind {
    /// Wrong value type for an operation.
    TypeMismatch,
    /// Dimensions don't line up (a structural error).
    DimensionMismatch,
    DivisionByZero,
    /// A required input cell had no value.
    MissingInput,
    Custom(String),
}

/// An error value, carrying enough context to trace it back to its source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValueError {
    pub kind: ValueErrorKind,
    pub source_measure: Option<MeasureId>,
}

impl ValueError {
    pub fn new(kind: ValueErrorKind) -> Self {
        ValueError {
            kind,
            source_measure: None,
        }
    }

    pub fn from_measure(kind: ValueErrorKind, measure: MeasureId) -> Self {
        ValueError {
            kind,
            source_measure: Some(measure),
        }
    }
}

impl From<ValueError> for Value {
    fn from(e: ValueError) -> Value {
        Value::Error(e)
    }
}
