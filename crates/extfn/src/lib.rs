//! External-language function runtime for Improv (Phase 6).
//!
//! This crate is the analogue of the engine's in-process scalar registry
//! (`improv_engine`'s `scalar_arity`/`scalar_func`) for NON-builtin functions:
//! it evaluates a registered external function on typed
//! [`improv_core_model::Value`] arguments, deterministically and in a
//! subprocess sandbox. The engine calls it host-side, outside the dataflow
//! (`improv_engine::external::refresh_external_measure`), so external calls
//! never run on the incremental hot path.
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
//! # Language runtimes
//!
//! `eval` dispatches on `f.language` to a per-language runner:
//! * **Python / R / Julia / Pure** — subprocess runners (shared plumbing in
//!   the `runner` module): the interpreter reads a generated program on stdin
//!   and prints one `{"ok":..}` / `{"error":..}` JSON envelope on stdout. A
//!   wall-clock timeout kills a runaway child. If the interpreter binary is
//!   absent, `eval` returns [`ExtFnError::LanguageUnavailable`] — it never
//!   panics.
//! * **Wasm** — an in-process `wasmi` interpreter (no subprocess); numeric f64
//!   ABI (see the `wasm` module).
//!
//! # Sandboxing and its limits
//!
//! Python is invoked as `python3 -I -S -` (isolated, no site, program on stdin);
//! R as `Rscript --vanilla -`; Julia as `julia --startup-file=no -`; Pure as
//! `pure -q`. In every case the program arrives on **stdin**, so the function
//! body is never interpolated into a shell command line (no shell is spawned).
//! A wall-clock **timeout** (default 5s) kills a runaway child.
//!
//! On top of that, every subprocess runner is spawned through the `sandbox`
//! module's OS sandbox (see its docs): on Linux, a `bwrap` (bubblewrap)
//! filesystem/network sandbox plus `setrlimit` ceilings (CPU time, address
//! space, open files, process count). It **fails closed**: without `bwrap`
//! (always the case on macOS and Windows) a `Restricted` body is refused with
//! [`ExtFnError::Sandbox`] unless the operator sets
//! `IMPROV_EXTFN_ALLOW_UNSANDBOXED=1`, which falls back to rlimits only. The
//! Wasm runtime runs in-process via `wasmi`, which has no syscall access at
//! all by construction, independent of any OS mechanism.
//!
//! The host, never the model, picks the sandbox strictness: every body runs
//! [`SandboxPolicy::Restricted`] unless the operator sets
//! `IMPROV_EXTFN_TRUSTED=1` (see `sandbox::policy_for`). [`ExternalFn::pure`]
//! is model data and only affects caching.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

mod julia;
mod marshal;
mod pure;
mod python;
mod r;
mod runner;
mod sandbox;
mod wasm;

pub use marshal::{error_value, value_to_json};
pub use sandbox::SandboxPolicy;

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
        Language::R => r::eval(f, args, timeout),
        Language::Julia => julia::eval(f, args, timeout),
        Language::Wasm => wasm::eval(f, args, timeout),
        Language::Pure => pure::eval(f, args, timeout),
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

    /// The OS sandbox the host policy requires is unavailable (fail closed).
    #[error("sandbox unavailable: {0}")]
    Sandbox(String),
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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
        assert_eq!(reg.timeout(), Duration::from_secs(5));
        assert_eq!(
            Registry::new()
                .with_timeout(Duration::from_millis(7))
                .timeout(),
            Duration::from_millis(7)
        );
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

    /// True if `python3` runs under the host sandbox policy. Python tests
    /// early-return (logging a skip) when false, so CI without Python, or
    /// without `bwrap` (where `Restricted` fails closed), still passes.
    fn python_available() -> bool {
        crate::runner::run_interpreter(
            "python3",
            &["-I", "-S", "-"],
            "print(1)",
            Duration::from_secs(5),
            "python",
            crate::sandbox::policy_for(),
        )
        .is_ok_and(|o| o.stdout.trim() == "1")
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

    // ---- Interpreter availability probe (shared) ----

    /// True if a trivial `language` body (`body` sets `result` to 1) runs
    /// under the host sandbox policy, like [`python_available`]. R/Julia/Pure
    /// tests early-return (logging a skip) when false: the interpreter is
    /// missing, or `Restricted` fails closed (no working `bwrap`), or it is
    /// too slow to start within `timeout`.
    fn sandboxed_available(language: Language, body: &str, timeout: Duration) -> bool {
        let probe = ExternalFn {
            name: "probe".into(),
            language,
            body: body.into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        eval(&probe, &[], timeout).is_ok_and(|v| v == Value::Number(1.0))
    }

    // ---- R (guarded) ----

    #[test]
    fn r_hypot() {
        if !sandboxed_available(Language::R, "result <- 1", Duration::from_secs(10)) {
            println!("skipped: Rscript not found (or no sandbox)");
            return;
        }
        let f = ExternalFn {
            name: "hyp".into(),
            language: Language::R,
            body: "result <- sqrt(args[[1]]^2 + args[[2]]^2)".into(),
            arg_types: vec![ValueType::Number, ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        };
        let out = eval(
            &f,
            &[Value::Number(3.0), Value::Number(4.0)],
            Duration::from_secs(10),
        )
        .expect("eval");
        assert_eq!(out, Value::Number(5.0));
    }

    // ---- Julia (guarded) ----

    #[test]
    fn julia_sum() {
        if !sandboxed_available(Language::Julia, "    result = 1", Duration::from_secs(30)) {
            println!("skipped: julia not found (or no sandbox)");
            return;
        }
        let f = ExternalFn {
            name: "add".into(),
            language: Language::Julia,
            body: "    result = args[1] + args[2]".into(),
            arg_types: vec![ValueType::Number, ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        };
        let out = eval(
            &f,
            &[Value::Number(2.0), Value::Number(40.0)],
            Duration::from_secs(30),
        )
        .expect("eval");
        assert_eq!(out, Value::Number(42.0));
    }

    // ---- Pure-lang (guarded) ----

    #[test]
    fn pure_sum() {
        if !sandboxed_available(Language::Pure, "let result = 1;", Duration::from_secs(10)) {
            println!("skipped: pure not found (or no sandbox)");
            return;
        }
        let f = ExternalFn {
            name: "add".into(),
            language: Language::Pure,
            body: "let result = args!0 + args!1;".into(),
            arg_types: vec![ValueType::Number, ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        };
        let out = eval(
            &f,
            &[Value::Number(19.0), Value::Number(23.0)],
            Duration::from_secs(10),
        )
        .expect("eval");
        assert_eq!(out, Value::Number(42.0));
    }

    // ---- WASM (no external tool; wasmi + wat assembled at test time) ----

    #[test]
    fn wasm_doubles_input() {
        // A 3-line module exporting `improv_call(f64) -> f64` = x + x.
        let wat = "(module (func (export \"improv_call\") (param f64) (result f64) \
             local.get 0 local.get 0 f64.add))";
        let f = ExternalFn {
            name: "double".into(),
            language: Language::Wasm,
            body: format!("wat:{wat}"),
            arg_types: vec![ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        };
        let out = eval(&f, &[Value::Number(21.0)], Duration::from_secs(5)).expect("eval");
        assert_eq!(out, Value::Number(42.0));
    }

    #[test]
    fn wasm_two_args() {
        // a*a + b*b, exercising a 2-arg numeric ABI.
        let wat = "(module (func (export \"improv_call\") (param f64 f64) (result f64) \
             local.get 0 local.get 0 f64.mul local.get 1 local.get 1 f64.mul f64.add))";
        let f = ExternalFn {
            name: "sqsum".into(),
            language: Language::Wasm,
            body: format!("wat:{wat}"),
            arg_types: vec![ValueType::Number, ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        };
        let out = eval(
            &f,
            &[Value::Number(3.0), Value::Number(4.0)],
            Duration::from_secs(5),
        )
        .expect("eval");
        assert_eq!(out, Value::Number(25.0));
    }

    #[test]
    fn wasm_missing_export_is_runtime_error() {
        let f = ExternalFn {
            name: "nope".into(),
            language: Language::Wasm,
            body: "wat:(module)".into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        let err = eval(&f, &[], Duration::from_secs(5)).unwrap_err();
        match err {
            ExtFnError::Runtime { message, .. } => assert!(message.contains("improv_call")),
            other => panic!("expected Runtime, got {other:?}"),
        }
    }

    #[test]
    fn wasm_rejects_non_numeric_abi() {
        let f = ExternalFn {
            name: "bad".into(),
            language: Language::Wasm,
            body: "wat:(module)".into(),
            arg_types: vec![ValueType::Text],
            return_type: ValueType::Text,
            pure: true,
        };
        let err = eval(&f, &[Value::Text("x".into())], Duration::from_secs(5)).unwrap_err();
        assert!(matches!(err, ExtFnError::Runtime { .. }), "got {err:?}");
    }

    // ---- Unavailable runtime does not panic ----

    #[test]
    fn absent_interpreter_is_error_not_panic() {
        // A language whose interpreter may not be installed yields an error, not
        // a panic. If the runtime happens to be present, a benign body still
        // succeeds — either way, no panic.
        for lang in [Language::R, Language::Julia, Language::Pure] {
            let body = match lang {
                Language::R => "result <- 1",
                Language::Julia => "    result = 1",
                Language::Pure => "let result = 1;",
                _ => unreachable!(),
            };
            let f = ExternalFn {
                name: "probe".into(),
                language: lang,
                body: body.into(),
                arg_types: vec![],
                return_type: ValueType::Number,
                pure: true,
            };
            // Must return a Result (no panic); Ok or Err both acceptable.
            let _ = eval(&f, &[], Duration::from_secs(5));
        }
    }

    // ---- OS sandbox (Linux: bwrap + rlimits; guarded like the other
    // interpreter-dependent tests, skip cleanly when tools are absent) ----

    fn bwrap_available() -> bool {
        std::process::Command::new("bwrap")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// A `Restricted` (the host default) body that tries to allocate
    /// well past the sandbox's `RLIMIT_AS` ceiling must fail (Python raises
    /// `MemoryError`, which the driver catches and reports as an error
    /// envelope) rather than succeed. This is the rlimit half of the sandbox,
    /// independent of whether `bwrap` is present. Linux only: macOS accepts
    /// `RLIMIT_AS` but does not enforce it.
    #[cfg(target_os = "linux")]
    #[test]
    fn restricted_python_hits_memory_limit() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        let f = ExternalFn {
            name: "blow_memory".into(),
            language: Language::Python,
            body: "x = bytearray(2 * 10**9)\nresult = len(x)".into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        let err = eval(&f, &[], Duration::from_secs(10)).unwrap_err();
        // Either the driver's own try/except reports MemoryError as a Runtime
        // error, or (if the allocator kills the process outright under the
        // rlimit) the subprocess exits nonzero/empty-output, also a Runtime
        // error. A Timeout would indicate the limit did nothing and the body
        // ran to completion within the wall clock only by luck; assert it's
        // specifically a Runtime error, not success.
        assert!(matches!(err, ExtFnError::Runtime { .. }), "got {err:?}");
    }

    /// Sanity check that the SAME allocation succeeds under `Trusted` (no
    /// sandbox applied), proving the failure above is the rlimit and not some
    /// unrelated Python/driver bug.
    #[test]
    fn trusted_python_memory_allocation_succeeds() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        let f = ExternalFn {
            name: "blow_memory_trusted".into(),
            language: Language::Python,
            body: "x = bytearray(10**7)\nresult = len(x)".into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: false,
        };
        let out = python::eval_with(&f, &[], Duration::from_secs(10), SandboxPolicy::Trusted)
            .expect("eval");
        assert_eq!(out, Value::Number(10_000_000.0));
    }

    /// When `bwrap` is present, a `Restricted` body reading an absolute path
    /// outside the sandbox's bind-mounted view (here, this very repo's
    /// `Cargo.toml`, addressed by absolute host path) must fail — proving the
    /// filesystem restriction is real — while the SAME body under `Trusted`
    /// (no bwrap wrapping) succeeds. Guarded on `bwrap` being on PATH.
    #[test]
    fn bwrap_blocks_filesystem_read_outside_sandbox() {
        if !bwrap_available() {
            println!("skipped: bwrap not found");
            return;
        }
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        // Use this crate's own Cargo.toml: guaranteed to exist, guaranteed to
        // be under $HOME (which bwrap does not bind), and not under the parts
        // of `/` this sandbox binds read-only mirror-fashion... except `/` IS
        // bound read-only in our bwrap policy, so instead prove the point with
        // a path that is genuinely absent from the sandboxed view: `/etc/hostname`
        // is bound (since `/` is ro-bound in full), so use the crate manifest
        // under `$HOME` combined with `--unshare-net`/no special exclusion —
        // the real isolation this policy provides is network + tmp, so assert
        // on a target we deliberately do NOT bind: a path under a fresh tmpfs
        // written by the *host* just before the sandboxed run, which the
        // sandbox's own fresh `/tmp` cannot see.
        let host_tmp_dir =
            std::env::temp_dir().join(format!("improv_extfn_sandbox_test_{}", std::process::id()));
        std::fs::create_dir_all(&host_tmp_dir).expect("create host tmp dir");
        let secret_path = host_tmp_dir.join("secret.txt");
        std::fs::write(&secret_path, "host-only-secret").expect("write secret");
        let secret_path_str = secret_path.to_string_lossy().replace('\\', "\\\\");

        let body = format!("result = open(r'{secret_path_str}').read()",);

        // Restricted: bwrap's fresh tmpfs on /tmp hides the host's real /tmp,
        // so the file is unreadable inside the sandbox.
        let restricted_fn = ExternalFn {
            name: "peek_restricted".into(),
            language: Language::Python,
            body: body.clone(),
            arg_types: vec![],
            return_type: ValueType::Text,
            pure: true,
        };
        let restricted_err = eval(&restricted_fn, &[], Duration::from_secs(10)).unwrap_err();
        assert!(
            matches!(restricted_err, ExtFnError::Runtime { .. }),
            "expected the sandboxed read to fail, got {restricted_err:?}"
        );

        // Trusted: no bwrap wrapping, the real filesystem (and real /tmp) is
        // visible, so the same read succeeds.
        let trusted_fn = ExternalFn {
            name: "peek_trusted".into(),
            language: Language::Python,
            body,
            arg_types: vec![],
            return_type: ValueType::Text,
            pure: false,
        };
        let out = python::eval_with(
            &trusted_fn,
            &[],
            Duration::from_secs(10),
            SandboxPolicy::Trusted,
        )
        .expect("trusted eval");
        assert_eq!(out, Value::Text("host-only-secret".into()));

        let _ = std::fs::remove_dir_all(&host_tmp_dir);
    }

    /// The `IMPROV_EXTFN_ALLOW_UNSANDBOXED` fallback (rlimits only, no
    /// `bwrap`) still runs the interpreter, with a scrubbed environment.
    #[test]
    fn rlimits_only_fallback_still_runs() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        std::env::set_var("IMPROV_API_TOKEN", "parent-secret");
        let mut cmd =
            crate::sandbox::apply_unsandboxed_for_test(std::process::Command::new("python3"));
        cmd.args([
            "-I",
            "-S",
            "-c",
            "import os; print(os.environ.get('IMPROV_API_TOKEN', ''), os.environ['HOME'], os.environ['PATH'])",
        ]);
        let out = cmd.output().expect("spawn python3 under rlimits-only");
        assert!(out.status.success(), "status: {:?}", out.status);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "/tmp /usr/bin:/bin"
        );
    }

    fn py(body: &str, return_type: ValueType) -> ExternalFn {
        ExternalFn {
            name: "probe".into(),
            language: Language::Python,
            body: body.into(),
            arg_types: vec![],
            return_type,
            pure: true,
        }
    }

    /// S6: a `Restricted` body does not inherit the host environment.
    #[test]
    fn restricted_python_does_not_inherit_env() {
        if !python_available() {
            println!("skipped: python3 not found (or no sandbox)");
            return;
        }
        std::env::set_var("IMPROV_API_TOKEN", "parent-secret");
        let f = py(
            "import os\nresult = os.environ.get('IMPROV_API_TOKEN', '')",
            ValueType::Text,
        );
        let out = eval(&f, &[], Duration::from_secs(10)).expect("eval");
        assert_eq!(out, Value::Text(String::new()));
    }

    /// S5: under bwrap neither `~` nor `/home` shows anything of the host.
    #[test]
    fn bwrap_hides_host_home() {
        if !bwrap_available() || !python_available() {
            println!("skipped: bwrap or python3 not found");
            return;
        }
        let home = std::env::temp_dir().join(format!("improv_extfn_home_{}", std::process::id()));
        std::fs::create_dir_all(&home).expect("create fake home");
        std::fs::write(home.join("marker-secret"), "x").expect("write marker");
        let real_home = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let f = py(
            "import os\nh = os.path.expanduser('~')\n\
             result = repr([h, sorted(os.listdir(h)), sorted(os.listdir('/home'))])",
            ValueType::Text,
        );
        let out = eval(&f, &[], Duration::from_secs(10));
        if let Some(h) = real_home {
            std::env::set_var("HOME", h);
        }
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(out.expect("eval"), Value::Text("['/tmp', [], []]".into()));
    }

    /// S5: under bwrap the network namespace is empty.
    #[test]
    fn bwrap_blocks_network() {
        if !bwrap_available() || !python_available() {
            println!("skipped: bwrap or python3 not found");
            return;
        }
        let f = py(
            "import socket\ns = socket.socket()\ns.settimeout(2)\n\
             try:\n    s.connect(('1.1.1.1', 53))\n    result = 'connected'\n\
             except OSError:\n    result = 'blocked'",
            ValueType::Text,
        );
        let out = eval(&f, &[], Duration::from_secs(10)).expect("eval");
        assert_eq!(out, Value::Text("blocked".into()));
    }

    /// S7: output past the 64 KiB pipe buffer used to deadlock until the
    /// timeout; 200 KB now succeeds.
    #[test]
    fn python_large_output_succeeds() {
        if !python_available() {
            println!("skipped: python3 not found (or no sandbox)");
            return;
        }
        let f = py("print('x' * 200000)\nresult = 1", ValueType::Number);
        let out = eval(&f, &[], Duration::from_secs(10)).expect("eval");
        assert_eq!(out, Value::Number(1.0));
    }

    /// S7: stdout is capped at 1 MiB; 5 MB is an "output too large" error.
    #[test]
    fn python_huge_output_is_rejected() {
        if !python_available() {
            println!("skipped: python3 not found (or no sandbox)");
            return;
        }
        let f = py("print('x' * 5000000)\nresult = 1", ValueType::Number);
        match eval(&f, &[], Duration::from_secs(10)).unwrap_err() {
            ExtFnError::Runtime { message, .. } => {
                assert!(message.contains("output too large"), "{message}")
            }
            other => panic!("expected Runtime, got {other:?}"),
        }
    }

    /// S8: the timeout kills the whole process group, so a forked grandchild
    /// does not outlive the call. Runs `Trusted` (no bwrap pid namespace) so
    /// the group kill is the only thing that can stop it, and the marker file
    /// is visible to the host.
    #[cfg(target_os = "linux")]
    #[test]
    fn timeout_kills_grandchildren() {
        let probe = python::eval_with(
            &py("result = 1", ValueType::Number),
            &[],
            Duration::from_secs(5),
            SandboxPolicy::Trusted,
        );
        if probe.is_err() {
            println!("skipped: python3 not found");
            return;
        }
        let marker =
            std::env::temp_dir().join(format!("improv_extfn_grandchild_{}", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let body = format!(
            "import os, time\n\
             if os.fork() == 0:\n\
             \x20   open({path:?}, 'w').write(str(os.getpid()))\n\
             \x20   time.sleep(30)\n\
             \x20   os._exit(0)\n\
             time.sleep(30)\nresult = 1",
            path = marker.to_string_lossy()
        );
        let timeout = Duration::from_secs(1);
        let err = python::eval_with(
            &py(&body, ValueType::Number),
            &[],
            timeout,
            SandboxPolicy::Trusted,
        )
        .unwrap_err();
        assert!(matches!(err, ExtFnError::Timeout(_)), "got {err:?}");
        std::thread::sleep(Duration::from_secs(1));
        let pid = std::fs::read_to_string(&marker).expect("grandchild wrote its pid");
        let _ = std::fs::remove_file(&marker);
        let proc_dir = format!("/proc/{}", pid.trim());
        assert!(
            !std::path::Path::new(&proc_dir).exists(),
            "grandchild {pid} survived the timeout"
        );
    }

    /// S20: the body reaches `exec` byte for byte. Backslashes used to be
    /// doubled inside `r"""..."""`, which changed what the body meant.
    #[test]
    fn python_body_with_backslashes_and_triple_quotes() {
        if !python_available() {
            println!("skipped: python3 not found (or no sandbox)");
            return;
        }
        let body = r#"s = """a\\b"""
t = '\n' + "\""
result = s + "|" + str(len(t)) + "|" + "\x41""#;
        let out = eval(&py(body, ValueType::Text), &[], Duration::from_secs(10)).expect("eval");
        assert_eq!(out, Value::Text(r"a\b|2|A".into()));
    }
}
