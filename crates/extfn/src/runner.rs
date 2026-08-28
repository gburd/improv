//! Shared subprocess plumbing for the scripting-language runners (Python, R,
//! Julia, Pure).
//!
//! Each scripting runner builds a full interpreter *program* (driver + embedded
//! JSON payload + user body) as a string, feeds it to the interpreter on stdin,
//! and reads one line of JSON back on stdout. That spawn / write-stdin /
//! wall-clock-kill / read-stdout+stderr dance is identical across runners, so
//! it lives here as [`run_interpreter`]. The per-language modules only build the
//! program string and parse the `{"ok":..}` / `{"error":..}` envelope.

use crate::ExtFnError;
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Captured output of a finished (or killed) interpreter process.
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

/// Spawn `cmd args`, write `program` to its stdin, and read stdout/stderr with a
/// wall-clock timeout enforced by a killer thread (SIGKILL / taskkill).
///
/// `runtime_name` is used only for error messages. A spawn failure (interpreter
/// not on PATH) maps to [`ExtFnError::LanguageUnavailable`] so `eval` never
/// panics when a runtime is absent; a timeout maps to [`ExtFnError::Timeout`].
pub fn run_interpreter(
    cmd: &str,
    args: &[&str],
    program: &str,
    timeout: Duration,
    runtime_name: &str,
) -> Result<Output, ExtFnError> {
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| ExtFnError::LanguageUnavailable(format!("{runtime_name} ({cmd}): {e}")))?;

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(program.as_bytes())
        .map_err(|e| ExtFnError::Runtime {
            message: format!("writing program to {runtime_name} stdin: {e}"),
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
    status.map_err(|e| ExtFnError::Runtime {
        message: format!("waiting on {runtime_name}: {e}"),
        stderr: String::new(),
    })?;

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut s) = child.stdout.take() {
        let _ = s.read_to_string(&mut stdout);
    }
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut stderr);
    }
    Ok(Output { stdout, stderr })
}

/// Parse the shared `{"ok":..}` / `{"error":..}` stdout envelope emitted by the
/// scripting runners, then map/type-check the `ok` value against `return_type`.
///
/// `runtime_name` names the interpreter in error messages ("python", "R", ...).
/// The envelope is taken as the LAST non-empty line of stdout, so an interpreter
/// that prints banners/diagnostics before it does not confuse the parse.
pub fn parse_envelope(
    out: &str,
    err: &str,
    return_type: improv_core_model::ValueType,
    runtime_name: &str,
) -> Result<improv_core_model::Value, ExtFnError> {
    let trimmed = out.trim();
    if trimmed.is_empty() {
        return Err(ExtFnError::Runtime {
            message: format!("{runtime_name} produced no output"),
            stderr: err.to_string(),
        });
    }
    let last = trimmed
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(trimmed)
        .trim();
    let env: serde_json::Value = serde_json::from_str(last).map_err(|e| ExtFnError::Runtime {
        message: format!("could not parse {runtime_name} output as JSON: {e} (output: {last:?})"),
        stderr: err.to_string(),
    })?;

    if let Some(msg) = env.get("error").and_then(|v| v.as_str()) {
        return Err(ExtFnError::Runtime {
            message: msg.to_string(),
            stderr: err.to_string(),
        });
    }
    let ok = env.get("ok").ok_or_else(|| ExtFnError::Runtime {
        message: format!("{runtime_name} envelope missing `ok`/`error`: {last:?}"),
        stderr: err.to_string(),
    })?;

    crate::marshal::json_to_value(ok, return_type).map_err(|m| ExtFnError::Runtime {
        message: format!("return type mismatch: {m}"),
        stderr: err.to_string(),
    })
}

/// Kill a child by pid. std has no cross-platform "kill by pid", so this is the
/// minimal shim: SIGKILL on Unix, `taskkill` on Windows.
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
