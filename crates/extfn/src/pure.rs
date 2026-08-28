//! Pure-lang subprocess runner (<https://agraef.github.io/pure-lang/>).
//!
//! Invokes `pure -q` (quiet, no banner) with the program on stdin — Pure's
//! interpreter evaluates piped stdin as a script when stdin is not a tty. Pure
//! is a term-rewriting functional language; rather than decode JSON on the Pure
//! side (no JSON in its prelude), the driver builds the `args` list **directly**
//! as a Pure list literal from the typed arguments, runs the user `body` (which
//! must yield/bind `result`), and a tiny Pure encoder prints one
//! `{"ok":..}` / `{"error":..}` envelope on stdout. [`crate::runner`] enforces
//! the wall-clock timeout with a hard kill.
//!
//! Contract: the user `body` is Pure source that ends by defining `result`
//! (e.g. `let result = args!0 + args!1;`). `args` is a Pure list; `args!i`
//! indexes it.

use crate::runner;
use crate::{ExtFnError, ExternalFn};
use improv_core_model::Value;
use std::fmt::Write as _;
use std::time::Duration;

fn program(body: &str, args: &[Value]) -> Result<String, ExtFnError> {
    let mut p = String::new();
    p.push_str(PURE_PRELUDE);
    p.push_str("let args = ");
    p.push_str(&pure_args_literal(args)?);
    p.push_str(";\n");
    p.push_str(body);
    p.push('\n');
    // Print the envelope. `_improv_to_json` handles numbers/strings/bools.
    p.push_str("puts (\"{\\\"ok\\\":\" + _improv_to_json result + \"}\");\n");
    Ok(p)
}

/// Run the function. Assumes arity/type checks already passed.
pub fn eval(f: &ExternalFn, args: &[Value], timeout: Duration) -> Result<Value, ExtFnError> {
    let prog = program(&f.body, args)?;
    let out = runner::run_interpreter("pure", &["-q"], &prog, timeout, "pure")?;
    runner::parse_envelope(&out.stdout, &out.stderr, f.return_type, "pure")
}

/// Build a Pure list literal from typed args. Only the scalar types this
/// runtime marshals are supported; DateTime/Enum/Error args are rejected (they
/// don't reach here — the engine short-circuits errors and `arg_matches` gates
/// the declared types, so this is a defensive fallback).
fn pure_args_literal(args: &[Value]) -> Result<String, ExtFnError> {
    let mut s = String::from("[");
    for (i, a) in args.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        match a {
            Value::Number(n) => {
                let _ = write!(s, "{n:?}");
            }
            Value::Boolean(b) => s.push_str(if *b { "true" } else { "false" }),
            Value::Text(t) => {
                let _ = write!(s, "{}", serde_json::Value::String(t.clone()));
            }
            other => {
                return Err(ExtFnError::Runtime {
                    message: format!("pure runner: unsupported argument type {other:?}"),
                    stderr: String::new(),
                })
            }
        }
    }
    s.push(']');
    Ok(s)
}

/// Minimal result encoder + `true`/`false` bindings. Pure has booleans as the
/// integers 1/0 by convention, so we bind names and detect them structurally.
const PURE_PRELUDE: &str = r#"
_improv_to_json x = "true" if x === true;
                  = "false" if x === false;
                  = str x if intp x;
                  = str x if doublep x;
                  = "\"" + x + "\"" if stringp x;
                  = "null" otherwise;
"#;
