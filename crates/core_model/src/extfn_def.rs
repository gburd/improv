//! Definitions of external-language functions (Phase 6), as plain, serializable
//! model data. The *runtime* that evaluates these lives in the `improv_extfn`
//! crate; this module holds only the declaration so a `Model` can persist which
//! functions a formula may `CALL` and how to type-check them.
//!
//! External functions must be **pure**, **typed**, and declare their
//! dimensionality (arity) so the engine can treat a `CALL` as an ordinary,
//! deterministic operator (see AGENT_MASTER_STEERING §7 / AGENT_FORMULA_LANGUAGE
//! §11.3).

use crate::value::ValueType;
use serde::{Deserialize, Serialize};

/// The runtime language an external function targets. Each variant has a runner
/// in the `improv_extfn` crate (subprocess for the scripting languages, an
/// in-process `wasmi` interpreter for `Wasm`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Language {
    Python,
    R,
    Julia,
    Wasm,
    Pure,
}

/// A registered external function: everything needed to type-check a call and
/// (via the `improv_extfn` runtime) invoke it deterministically.
///
/// `body` is the function *body* (statements), evaluated with the arguments
/// bound and expected to produce a result (the runtime defines the exact
/// contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalFn {
    pub name: String,
    pub language: Language,
    pub body: String,
    /// Declared argument types, in order. `arity() == arg_types.len()`.
    pub arg_types: Vec<ValueType>,
    pub return_type: ValueType,
    /// The author asserts the body is pure (deterministic, no engine-observable
    /// side effects). Enforcement beyond the runtime's isolated invocation is
    /// future work.
    pub pure: bool,
}

impl ExternalFn {
    pub fn arity(&self) -> usize {
        self.arg_types.len()
    }
}
