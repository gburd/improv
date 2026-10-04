//! Shared subprocess plumbing for the scripting-language runners (Python, R,
//! Julia, Pure).
//!
//! Each scripting runner builds a full interpreter *program* (driver + embedded
//! JSON payload + user body) as a string, feeds it to the interpreter on stdin,
//! and reads one line of JSON back on stdout. That spawn / write-stdin /
//! drain-stdout+stderr / wall-clock-kill dance is identical across runners, so
//! it lives here as [`run_interpreter`]. The per-language modules only build the
//! program string and parse the `{"ok":..}` / `{"error":..}` envelope.

use crate::sandbox::SandboxPolicy;
use crate::ExtFnError;
use std::io::{Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Per-stream cap on captured stdout and stderr. Past it the stream is
/// drained to nowhere and the call fails, so a chatty body costs CPU (bounded
/// by the timeout), never memory.
const MAX_OUTPUT: u64 = 1 << 20;

/// Captured output of a finished (or killed) interpreter process.
pub struct Output {
    pub stdout: String,
    pub stderr: String,
}

/// Spawn `cmd args` under `policy`'s OS sandbox (see the `sandbox` module),
/// write `program` to its stdin, and collect stdout/stderr, killing the child
/// once the wall-clock `timeout` passes.
///
/// Stdin is fed and both output pipes are drained on their own threads while
/// this thread waits, so output larger than a pipe buffer cannot deadlock the
/// child. Each stream is capped at 1 MiB; past it the call fails with an
/// "output too large" [`ExtFnError::Runtime`].
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
    policy: SandboxPolicy,
) -> Result<Output, ExtFnError> {
    let mut command = Command::new(cmd);
    command.args(args);
    let mut command = crate::sandbox::apply(command, policy)?;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Own process group, so the timeout kill reaches forked grandchildren.
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    let mut child = command
        .spawn()
        .map_err(|e| ExtFnError::LanguageUnavailable(format!("{runtime_name} ({cmd}): {e}")))?;
    let deadline = Instant::now() + timeout;

    // A write error (the child exited without reading) surfaces through the
    // child's own exit and output, so it is dropped here.
    let mut stdin = child.stdin.take().expect("piped stdin");
    let program = program.to_owned();
    std::thread::spawn(move || {
        let _ = stdin.write_all(program.as_bytes());
    });
    let (tx, rx) = mpsc::channel();
    spawn_reader(child.stdout.take().expect("piped stdout"), 0, tx.clone());
    spawn_reader(child.stderr.take().expect("piped stderr"), 1, tx);

    // ponytail: poll try_wait with backoff (1 ms -> 50 ms) instead of a
    // blocking wait plus killer thread; it never kills a reaped (reusable)
    // pid. Use pidfd/waitid if the wake-up latency ever matters.
    let mut poll = Duration::from_millis(1);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {
                let now = Instant::now();
                if now >= deadline {
                    kill(&mut child);
                    return Err(ExtFnError::Timeout(timeout));
                }
                std::thread::sleep(poll.min(deadline - now));
                poll = (poll * 2).min(Duration::from_millis(50));
            }
            Err(e) => {
                kill(&mut child);
                return Err(ExtFnError::Runtime {
                    message: format!("waiting on {runtime_name}: {e}"),
                    stderr: String::new(),
                });
            }
        }
    }

    // The child has exited; a grandchild still holding a pipe open is cut
    // off at the same deadline.
    let mut streams = [Vec::new(), Vec::new()];
    let mut too_large = None;
    for _ in 0..2 {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok((i, buf, over)) = rx.recv_timeout(left) else {
            kill(&mut child);
            return Err(ExtFnError::Timeout(timeout));
        };
        if over {
            too_large = Some(["stdout", "stderr"][i]);
        }
        streams[i] = buf;
    }
    let [stdout, stderr] = streams.map(|b| String::from_utf8_lossy(&b).into_owned());
    if let Some(stream) = too_large {
        return Err(ExtFnError::Runtime {
            message: format!("{runtime_name} {stream} output too large (over {MAX_OUTPUT} bytes)"),
            stderr,
        });
    }
    Ok(Output { stdout, stderr })
}

/// Read `pipe` to EOF on a new thread, keeping at most [`MAX_OUTPUT`] bytes,
/// then send `(idx, bytes, overflowed)`.
fn spawn_reader(
    mut pipe: impl Read + Send + 'static,
    idx: usize,
    tx: mpsc::Sender<(usize, Vec<u8>, bool)>,
) {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.by_ref().take(MAX_OUTPUT + 1).read_to_end(&mut buf);
        let over = buf.len() as u64 > MAX_OUTPUT;
        if over {
            buf.truncate(MAX_OUTPUT as usize);
            // Keep draining so the child is not blocked on a full pipe.
            let _ = std::io::copy(&mut pipe, &mut std::io::sink());
        }
        let _ = tx.send((idx, buf, over));
    });
}

/// SIGKILL the child's whole process group (grandchildren included), then
/// reap the child. The group id stays reserved while any member is alive, so
/// this cannot hit a recycled pid.
#[cfg(unix)]
fn kill(child: &mut Child) {
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.wait();
}

/// `TerminateProcess` the child and reap it (Windows has no process groups
/// here; a Job Object would be the upgrade path).
#[cfg(not(unix))]
fn kill(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
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
