//! Python subprocess runner.
//!
//! Invokes `python3 -I -S -` (isolated, no site, program on stdin). Because
//! `python3 -` consumes stdin as the *program*, the JSON args payload is
//! embedded into the program as a Python string literal (still via stdin, never
//! interpolated into a shell command line — no shell is spawned). The driver
//! binds `args` (a list), `exec`s the user body, which must assign `result`,
//! and prints one JSON envelope to stdout: `{"ok": <value>}` or
//! `{"error": "<msg>"}`. The shared [`crate::runner`] enforces the wall-clock
//! timeout with a hard kill.

use crate::marshal::value_to_json;
use crate::runner;
use crate::{ExtFnError, ExternalFn};
use improv_core_model::Value;
use serde_json::json;
use std::time::Duration;

/// Build the full program: driver + embedded payload + user body.
///
/// The body is embedded in a triple-quoted raw string; we escape backslashes
/// and any `"""` so an authored body cannot break out of the literal.
fn program(body: &str, payload_json: &str) -> String {
    let escaped_body = body.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"");
    let payload_lit = py_str_literal(payload_json);
    format!(
        r#"import sys, json
_payload = json.loads({payload_lit})
args = _payload["args"]
_ns = {{"args": args}}
try:
    exec(r"""{escaped_body}""", {{"__builtins__": __builtins__}}, _ns)
    if "result" not in _ns:
        print(json.dumps({{"error": "function body did not set `result`"}}))
    else:
        print(json.dumps({{"ok": _ns["result"]}}))
except Exception as e:
    print(json.dumps({{"error": "{{}}: {{}}".format(type(e).__name__, e)}}))
"#
    )
}

/// Run the function. Assumes arity/type checks already passed.
pub fn eval(f: &ExternalFn, args: &[Value], timeout: Duration) -> Result<Value, ExtFnError> {
    let payload = json!({
        "args": args.iter().map(value_to_json).collect::<Vec<_>>(),
    })
    .to_string();
    let prog = program(&f.body, &payload);

    let out = runner::run_interpreter("python3", &["-I", "-S", "-"], &prog, timeout, "python")?;
    runner::parse_envelope(&out.stdout, &out.stderr, f.return_type, "python")
}

/// Encode a Rust string as a Python string literal. serde_json emits a valid
/// double-quoted string whose escapes Python accepts for these characters.
fn py_str_literal(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}
