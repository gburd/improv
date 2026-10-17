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

/// The kind of an error value. The first four are the spreadsheet errors a
/// formula produces (named as Excel shows them, see [`ValueErrorKind::code`]);
/// the rest carry Improv's own structural and external-function failures.
///
/// Variant names are persisted (an error literal in a stored formula), so
/// existing names keep their meaning: `DivisionByZero` is `#DIV/0!` and
/// `TypeMismatch` is `#VALUE!`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ValueErrorKind {
    /// `#VALUE!`: wrong value type for an operation.
    TypeMismatch,
    /// Dimensions don't line up (a structural error).
    DimensionMismatch,
    /// `#DIV/0!`.
    DivisionByZero,
    /// A required input cell had no value.
    MissingInput,
    Custom(String),
    /// `#NULL!`.
    Null,
    /// `#REF!`.
    Ref,
    /// `#NAME?`.
    Name,
    /// `#NUM!`: a numeric domain error (`SQRT(-1)`, `LN(0)`, overflow).
    Num,
    /// `#N/A`: no value available (`NA()`, a failed lookup).
    Na,
}

impl ValueErrorKind {
    /// The spreadsheet error text: `#DIV/0!`, `#N/A`, …; Improv's own kinds
    /// show as `#ERR`.
    pub fn code(&self) -> &'static str {
        match self {
            ValueErrorKind::Null => "#NULL!",
            ValueErrorKind::DivisionByZero => "#DIV/0!",
            ValueErrorKind::TypeMismatch => "#VALUE!",
            ValueErrorKind::Ref => "#REF!",
            ValueErrorKind::Name => "#NAME?",
            ValueErrorKind::Num => "#NUM!",
            ValueErrorKind::Na => "#N/A",
            _ => "#ERR",
        }
    }

    /// `ERROR.TYPE`'s number for a spreadsheet error (1 `#NULL!` … 7 `#N/A`),
    /// or `None` for Improv's own kinds.
    pub fn error_type(&self) -> Option<u8> {
        Some(match self {
            ValueErrorKind::Null => 1,
            ValueErrorKind::DivisionByZero => 2,
            ValueErrorKind::TypeMismatch => 3,
            ValueErrorKind::Ref => 4,
            ValueErrorKind::Name => 5,
            ValueErrorKind::Num => 6,
            ValueErrorKind::Na => 7,
            _ => return None,
        })
    }

    /// The kind for an `ERROR.TYPE` number (the inverse of [`Self::error_type`]).
    pub fn from_error_type(n: u8) -> Option<Self> {
        Some(match n {
            1 => ValueErrorKind::Null,
            2 => ValueErrorKind::DivisionByZero,
            3 => ValueErrorKind::TypeMismatch,
            4 => ValueErrorKind::Ref,
            5 => ValueErrorKind::Name,
            6 => ValueErrorKind::Num,
            7 => ValueErrorKind::Na,
            _ => return None,
        })
    }

    /// The kind an error text names (`"#DIV/0!"`, case-insensitive).
    pub fn from_code(code: &str) -> Option<Self> {
        (1..=7)
            .filter_map(Self::from_error_type)
            .find(|k| k.code().eq_ignore_ascii_case(code.trim()))
    }
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
