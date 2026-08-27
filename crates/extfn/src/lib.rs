//! External-language function runtime for Improv (Phase 6).
//!
//! This crate is the analogue of the engine's in-process scalar registry
//! (`improv_engine`'s `scalar_arity`/`scalar_func`) for NON-builtin functions:
//! it evaluates a registered external function on typed
//! [`improv_core_model::Value`] arguments, deterministically and in a
//! subprocess sandbox. It is intentionally NOT wired into the engine yet — the
//! engine's `Expr::Call` path can later dispatch here.
//!
//! # Invariant (see `AGENT_MASTER_STEERING.md` §7)
//!
//! External functions must be **pure**, return a **typed** value, and declare
//! **dimensionality**, so they behave as ordinary operators and keep the engine
//! deterministic. This runtime is deterministic *given the same inputs + body*;
//! purity of the user's function body is the user's contract (the engine treats
//! external calls as pure). Dimensionality is declared on the descriptor
//! (`arg_types` / `return_type`); higher-arity dimension broadcasting is the
//! engine's job at the call site, not this runtime's.
//!
//! # Sandboxing (Python v1) and its limits
//!
//! Python is invoked as `python3 -I -S -`:
//! * `-I` isolated mode: ignores `PYTHON*` env vars and the user site dir, and
//!   does not prepend the script's directory to `sys.path`.
//! * `-S` no `site`: skips automatic site-package imports.
//! * `-` reads the program from **stdin**, so the function body is never
//!   interpolated into a shell command line (no shell is spawned at all —
//!   `std::process::Command` execs `python3` directly).
//! * A wall-clock **timeout** (default 5s) kills a runaway child.
//!
//! This blocks the accidental-import / stray-config-file class of problems and
//! bounds runtime. It is **not** an OS sandbox: a determined body can still read
//! files or open sockets. Hardening (seccomp, namespaces, a container, or a
//! WASM runtime) is future work and is the real trust boundary for untrusted
//! code. v1 assumes function bodies are authored/reviewed by the model owner.

mod marshal;
mod python;

pub use marshal::{error_value, value_to_json};

use improv_core_model::{Value, ValueType};
use std::collections::HashMap;
use std::time::Duration;

// `ExternalFn`/`Language` are the model-level *definitions* (plain, serializable
// data); this crate is their *runtime*. Re-exported so callers can use
// `improv_extfn::{ExternalFn, Language}` interchangeably.
pub use improv_core_model::{ExternalFn, Language};

/// Name -> descriptor. The engine will resolve a `FuncId`/name to a descriptor
/// here for non-builtin calls.
#[derive(Debug, Default, Clone)]
pub struct Registry {
    fns: HashMap<String, ExternalFn>,
    /// Wall-clock kill deadline for a single evaluation.
    timeout: Duration,
}

impl Registry {
    pub fn new() -> Self {
        Registry {
            fns: HashMap::new(),
            timeout: Duration::from_secs(5),
        }
    }

    /// Override the per-call timeout (default 5s).
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Register (or replace) a function. Returns the previous descriptor if the
    /// name was already registered.
    pub fn register(&mut self, f: ExternalFn) -> Option<ExternalFn> {
        self.fns.insert(f.name.clone(), f)
    }

    pub fn get(&self, name: &str) -> Option<&ExternalFn> {
        self.fns.get(name)
    }

    pub fn len(&self) -> usize {
        self.fns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fns.is_empty()
    }

    /// Evaluate a registered function by name on typed arguments.
    pub fn eval(&self, name: &str, args: &[Value]) -> Result<Value, ExtFnError> {
        let f = self
            .fns
            .get(name)
            .ok_or_else(|| ExtFnError::NotFound(name.to_string()))?;
        eval(f, args, self.timeout)
    }
}

/// Evaluate one descriptor on typed arguments with an explicit timeout.
///
/// Validates arity and argument types (no subprocess needed for those), then
/// dispatches to the language runtime and type-checks the returned value
/// against `return_type`.
pub fn eval(f: &ExternalFn, args: &[Value], timeout: Duration) -> Result<Value, ExtFnError> {
    if args.len() != f.arity() {
        return Err(ExtFnError::ArityMismatch {
            name: f.name.clone(),
            expected: f.arity(),
            got: args.len(),
        });
    }
    for (i, (arg, &ty)) in args.iter().zip(&f.arg_types).enumerate() {
        if !marshal::arg_matches(arg, ty) {
            return Err(ExtFnError::TypeMismatch {
                name: f.name.clone(),
                position: i,
                expected: ty,
                got: arg.type_of(),
            });
        }
    }
    match f.language {
        Language::Python => python::eval(f, args, timeout),
        other => Err(ExtFnError::LanguageUnavailable(format!("{other:?}"))),
    }
}

/// Errors from resolving or evaluating an external function. The engine will
/// convert these to a `Value::Error` at the call site (see [`error_value`]);
/// this API returns `Result` so the caller decides.
#[derive(Debug, thiserror::Error)]
pub enum ExtFnError {
    #[error("external function not found: {0}")]
    NotFound(String),

    #[error("function {name}: expected {expected} argument(s), got {got}")]
    ArityMismatch {
        name: String,
        expected: usize,
        got: usize,
    },

    #[error(
        "function {name}: argument {position} type mismatch, expected {expected:?}, got {got:?}"
    )]
    TypeMismatch {
        name: String,
        position: usize,
        expected: ValueType,
        got: Option<ValueType>,
    },

    #[error("language runtime unavailable: {0}")]
    LanguageUnavailable(String),

    #[error("runtime error: {message}\n--- stderr ---\n{stderr}")]
    Runtime { message: String, stderr: String },

    #[error("function timed out after {0:?}")]
    Timeout(Duration),
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::Value;

    fn add_fn() -> ExternalFn {
        ExternalFn {
            name: "add".into(),
            language: Language::Python,
            body: "result = args[0] + args[1]".into(),
            arg_types: vec![ValueType::Number, ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        }
    }

    #[test]
    fn registry_roundtrip() {
        let mut reg = Registry::new();
        assert!(reg.is_empty());
        assert!(reg.register(add_fn()).is_none());
        assert_eq!(reg.len(), 1);
        let got = reg.get("add").expect("registered");
        assert_eq!(got.arity(), 2);
        assert_eq!(got.return_type, ValueType::Number);
        assert!(got.pure);
        // Re-register returns the previous descriptor.
        assert!(reg.register(add_fn()).is_some());
        assert!(reg.get("missing").is_none());
    }

    #[test]
    fn not_found() {
        let reg = Registry::new();
        assert!(matches!(
            reg.eval("nope", &[]),
            Err(ExtFnError::NotFound(_))
        ));
    }

    #[test]
    fn arity_mismatch_without_python() {
        let f = add_fn();
        let err = eval(&f, &[Value::Number(1.0)], Duration::from_secs(2)).unwrap_err();
        match err {
            ExtFnError::ArityMismatch { expected, got, .. } => {
                assert_eq!(expected, 2);
                assert_eq!(got, 1);
            }
            other => panic!("expected ArityMismatch, got {other:?}"),
        }
    }

    #[test]
    fn type_mismatch_without_python() {
        let f = add_fn();
        let err = eval(
            &f,
            &[Value::Number(1.0), Value::Text("nope".into())],
            Duration::from_secs(2),
        )
        .unwrap_err();
        match err {
            ExtFnError::TypeMismatch {
                position, expected, ..
            } => {
                assert_eq!(position, 1);
                assert_eq!(expected, ValueType::Number);
            }
            other => panic!("expected TypeMismatch, got {other:?}"),
        }
    }

    // ---- Python-dependent tests (guarded) ----

    /// True if `python3` is on PATH. Python tests early-return (logging a skip)
    /// when false so CI without Python still passes.
    fn python_available() -> bool {
        std::process::Command::new("python3")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    #[test]
    fn python_add_numbers() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        let mut reg = Registry::new().with_timeout(Duration::from_secs(2));
        reg.register(add_fn());
        let out = reg
            .eval("add", &[Value::Number(2.0), Value::Number(3.0)])
            .expect("eval");
        assert_eq!(out, Value::Number(5.0));
    }

    #[test]
    fn python_text_function() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        let f = ExternalFn {
            name: "shout".into(),
            language: Language::Python,
            body: "result = args[0].upper() + '!'".into(),
            arg_types: vec![ValueType::Text],
            return_type: ValueType::Text,
            pure: true,
        };
        let out = eval(&f, &[Value::Text("hi".into())], Duration::from_secs(2)).expect("eval");
        assert_eq!(out, Value::Text("HI!".into()));
    }

    #[test]
    fn python_raises_is_runtime_error() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        let f = ExternalFn {
            name: "boom".into(),
            language: Language::Python,
            body: "raise ValueError('nope')".into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        let err = eval(&f, &[], Duration::from_secs(2)).unwrap_err();
        match err {
            ExtFnError::Runtime { message, .. } => assert!(message.contains("ValueError")),
            other => panic!("expected Runtime, got {other:?}"),
        }
    }

    #[test]
    fn python_return_type_mismatch() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        // Declares Number but returns a string.
        let f = ExternalFn {
            name: "liar".into(),
            language: Language::Python,
            body: "result = 'not a number'".into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        let err = eval(&f, &[], Duration::from_secs(2)).unwrap_err();
        assert!(matches!(err, ExtFnError::Runtime { .. }));
    }

    #[test]
    fn python_infinite_loop_times_out() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        let f = ExternalFn {
            name: "spin".into(),
            language: Language::Python,
            body: "while True:\n    pass".into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        let start = std::time::Instant::now();
        let err = eval(&f, &[], Duration::from_secs(2)).unwrap_err();
        assert!(matches!(err, ExtFnError::Timeout(_)), "got {err:?}");
        // Killed near the deadline, not hung forever.
        assert!(start.elapsed() < Duration::from_secs(8));
    }
}
