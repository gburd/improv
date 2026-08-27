//! Python subprocess runner.
//!
//! Invokes `python3 -I -S -` (isolated, no site, program on stdin). Because
//! `python3 -` consumes stdin as the *program*, the JSON args payload is
//! embedded into the program as a Python string literal (still via stdin, never
//! interpolated into a shell command line — no shell is spawned). The driver
//! binds `args` (a list), `exec`s the user body, which must assign `result`,
//! and prints one JSON envelope to stdout: `{"ok": <value>}` or
//! `{"error": "<msg>"}`. A killer thread enforces the wall-clock timeout.

use crate::marshal::{json_to_value, value_to_json};
use crate::{ExtFnError, ExternalFn};
use improv_core_model::{Value, ValueType};
use serde_json::json;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
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

    let mut child = Command::new("python3")
        .args(["-I", "-S", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ExtFnError::LanguageUnavailable(format!("python3: {e}")))?;

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(prog.as_bytes())
        .map_err(|e| ExtFnError::Runtime {
            message: format!("writing program to python3 stdin: {e}"),
            stderr: String::new(),
        })?;

    // Killer thread: sleep the timeout, then kill unless the child already
    // finished. Uses only the raw pid so we don't need to share the `Child`.
    let pid = child.id();
    let done = Arc::new(AtomicBool::new(false));
    let killed = Arc::new(AtomicBool::new(false));
    let killer = {
        let done = Arc::clone(&done);
        let killed = Arc::clone(&killed);
        std::thread::spawn(move || {
            // Poll so we can exit promptly once the child is done.
            let step = Duration::from_millis(20);
            let mut waited = Duration::ZERO;
            while waited < timeout {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                std::thread::sleep(step);
                waited += step;
            }
            if !done.load(Ordering::SeqCst) {
                killed.store(true, Ordering::SeqCst);
                kill_pid(pid);
            }
        })
    };

    let status = child.wait();
    done.store(true, Ordering::SeqCst);
    let _ = killer.join();

    if killed.load(Ordering::SeqCst) {
        return Err(ExtFnError::Timeout(timeout));
    }
    // A failed wait is itself a runtime problem.
    status.map_err(|e| ExtFnError::Runtime {
        message: format!("waiting on python3: {e}"),
        stderr: String::new(),
    })?;

    let mut out = String::new();
    let mut err = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut out);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut err);
    }

    parse_envelope(&out, &err, f.return_type)
}

/// Kill a child by pid. Portable-enough for v1: SIGKILL on Unix, `taskkill` on
/// Windows. std has no cross-platform "kill by pid", so this is the minimal
/// shim.
#[cfg(unix)]
fn kill_pid(pid: u32) {
    // ponytail: raw libc-free kill via /proc is unavailable; shell out to `kill`.
    // Upgrade to nix::sys::signal if this ever needs to avoid a fork.
    let _ = Command::new("kill").args(["-9", &pid.to_string()]).status();
}

#[cfg(windows)]
fn kill_pid(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status();
}

/// Encode a Rust string as a Python string literal. serde_json emits a valid
/// double-quoted string whose escapes Python accepts for these characters.
fn py_str_literal(s: &str) -> String {
    serde_json::Value::String(s.to_string()).to_string()
}

/// Parse the `{"ok":..}` / `{"error":..}` envelope and map/type-check.
fn parse_envelope(out: &str, err: &str, return_type: ValueType) -> Result<Value, ExtFnError> {
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return Err(ExtFnError::Runtime {
            message: "python produced no output".into(),
            stderr: err.to_string(),
        });
    }
    let env: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| ExtFnError::Runtime {
            message: format!("could not parse python output as JSON: {e} (output: {trimmed:?})"),
            stderr: err.to_string(),
        })?;

    if let Some(msg) = env.get("error").and_then(|v| v.as_str()) {
        return Err(ExtFnError::Runtime {
            message: msg.to_string(),
            stderr: err.to_string(),
        });
    }
    let ok = env.get("ok").ok_or_else(|| ExtFnError::Runtime {
        message: format!("python envelope missing `ok`/`error`: {trimmed:?}"),
        stderr: err.to_string(),
    })?;

    json_to_value(ok, return_type).map_err(|m| ExtFnError::Runtime {
        message: format!("return type mismatch: {m}"),
        stderr: err.to_string(),
    })
}
