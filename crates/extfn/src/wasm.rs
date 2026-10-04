//! WebAssembly runner (in-process, via the `wasmi` interpreter).
//!
//! Unlike the scripting runners there is no subprocess: `wasmi` is a pure-Rust
//! Wasm **interpreter** (no JIT, MSRV-friendly). The `body` is the module
//! source, in one of two forms:
//!
//! * `wat:<text>`  — WebAssembly Text, assembled at load time via the `wat`
//!   crate. (Only enabled when the `wat` feature is on; on by default.)
//! * `base64:<b64>` — base64-encoded `.wasm` bytes.
//!
//! Anything else is rejected. The body is model data, so it is never read as
//! a filesystem path.
//!
//! # ABI (numeric only)
//!
//! The module must export a function named `improv_call`. All arguments are
//! passed as Wasm `f64` params, in order, and the single return value is read
//! as `f64`. Consequently the declared `arg_types` must all be `Number` and
//! `return_type` must be `Number`; anything else is a runtime error. This is a
//! deliberately narrow ABI — a richer bytes/string ABI (linear-memory marshal)
//! is future work and would balloon scope.
//!
//! # Limits
//!
//! Wasm runs in-process, so there is no pid to SIGKILL. Each call runs on a
//! worker thread with:
//!
//! * a **fuel** budget of [`FUEL_PER_SEC`] per second of the timeout (capped
//!   at [`MAX_FUEL`]). When it runs out the interpreter traps and the thread
//!   ends, so a runaway loop cannot keep burning CPU after `eval` returns
//!   [`ExtFnError::Timeout`] on the wall-clock deadline.
//! * a [`StoreLimits`](wasmi::StoreLimits) cap of [`MAX_MEMORY`] bytes per
//!   linear memory: `memory.grow` past it returns `-1`, and a module that
//!   declares more fails to instantiate.
//!
//! ponytail: wasmi 0.32 has no epoch interruption (that is a wasmtime
//! feature), so the wall clock cannot stop the interpreter thread directly;
//! fuel is the stop. A fuel unit is roughly one instruction; the budget is
//! sized for a debug build, so an optimized build exhausts it well before the
//! deadline. Upgrade path: a wasmi release with resumable out-of-fuel calls
//! (refuel in slices and check the deadline) or wasmtime's epochs.

use crate::{ExtFnError, ExternalFn};
use improv_core_model::{Value, ValueType};
use std::sync::mpsc;
use std::time::Duration;

/// Fuel per second of wall-clock timeout (about one unit per instruction).
pub(crate) const FUEL_PER_SEC: u64 = 100_000_000;
/// Fuel ceiling whatever the timeout.
pub(crate) const MAX_FUEL: u64 = 1_000_000_000;
/// Linear-memory cap per memory: 64 MiB.
pub(crate) const MAX_MEMORY: usize = 64 << 20;

/// The fuel budget for a call with `timeout`.
fn fuel_for(timeout: Duration) -> u64 {
    let ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
    ms.saturating_mul(FUEL_PER_SEC / 1000).clamp(1, MAX_FUEL)
}

/// Run the function. Assumes arity/type checks already passed (so `args.len()`
/// matches `arg_types.len()`), but re-checks the numeric ABI here.
pub fn eval(f: &ExternalFn, args: &[Value], timeout: Duration) -> Result<Value, ExtFnError> {
    if f.return_type != ValueType::Number || f.arg_types.iter().any(|t| *t != ValueType::Number) {
        return Err(ExtFnError::Runtime {
            message: "wasm ABI supports only Number args and a Number return".into(),
            stderr: String::new(),
        });
    }
    let inputs: Vec<f64> = args
        .iter()
        .map(|v| match v {
            Value::Number(n) => *n,
            // Unreachable given the ABI check above + prior arg_matches gate.
            _ => f64::NAN,
        })
        .collect();

    let wasm = load_module_bytes(&f.body)?;
    let fuel = fuel_for(timeout);

    // Run on a worker thread and join with the wall-clock deadline; the fuel
    // budget ends the thread even if the deadline fires first.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_wasm(&wasm, &inputs, fuel));
    });
    match rx.recv_timeout(timeout) {
        Ok(res) => res.map(Value::Number),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(ExtFnError::Timeout(timeout)),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(ExtFnError::Runtime {
            message: "wasm worker thread died without a result".into(),
            stderr: String::new(),
        }),
    }
}

/// Resolve `body` to Wasm bytes: `wat:` or `base64:`; anything else is an
/// error (never a filesystem read).
fn load_module_bytes(body: &str) -> Result<Vec<u8>, ExtFnError> {
    let runtime_err = |m: String| ExtFnError::Runtime {
        message: m,
        stderr: String::new(),
    };
    if let Some(text) = body.strip_prefix("wat:") {
        #[cfg(feature = "wat")]
        {
            return wat::parse_str(text).map_err(|e| runtime_err(format!("invalid wat: {e}")));
        }
        #[cfg(not(feature = "wat"))]
        {
            let _ = text;
            return Err(runtime_err(
                "wat: bodies require the `wat` cargo feature".into(),
            ));
        }
    }
    if let Some(b64) = body.strip_prefix("base64:") {
        return base64_decode(b64.trim()).map_err(runtime_err);
    }
    Err(runtime_err(
        "wasm body must start with `wat:` or `base64:`".into(),
    ))
}

/// Instantiate the module and call `improv_call(f64...) -> f64` via the untyped
/// API (so any arity works), with `fuel` and the [`MAX_MEMORY`] cap. Running
/// out of fuel is a `Runtime` "ran out of fuel" error, as is any other wasmi
/// error.
pub(crate) fn run_wasm(wasm: &[u8], inputs: &[f64], fuel: u64) -> Result<f64, ExtFnError> {
    use wasmi::core::TrapCode;
    use wasmi::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder, Val};

    let runtime_err = |m: String| ExtFnError::Runtime {
        message: m,
        stderr: String::new(),
    };

    let mut config = Config::default();
    config.consume_fuel(true);
    let engine = Engine::new(&config);
    let module =
        Module::new(&engine, wasm).map_err(|e| runtime_err(format!("wasm compile error: {e}")))?;
    let limits = StoreLimitsBuilder::new().memory_size(MAX_MEMORY).build();
    let mut store = Store::new(&engine, limits);
    store.limiter(|limits| limits);
    store
        .set_fuel(fuel)
        .map_err(|e| runtime_err(format!("wasm fuel: {e}")))?;
    let linker = <Linker<StoreLimits>>::new(&engine);
    let instance = linker
        .instantiate(&mut store, &module)
        .map_err(|e| runtime_err(format!("wasm instantiate error: {e}")))?
        .start(&mut store)
        .map_err(|e| runtime_err(format!("wasm start error: {e}")))?;

    let func = instance
        .get_func(&store, "improv_call")
        .ok_or_else(|| runtime_err("wasm module does not export `improv_call`".into()))?;

    let params: Vec<Val> = inputs.iter().map(|n| Val::F64((*n).into())).collect();
    let mut results = [Val::F64(0.0f64.into())];
    let out_of_fuel = |e: &wasmi::Error| e.as_trap_code() == Some(TrapCode::OutOfFuel);
    func.call(&mut store, &params, &mut results).map_err(|e| {
        if out_of_fuel(&e) {
            ExtFnError::Runtime {
                message: format!("wasm ran out of fuel ({fuel} units)"),
                stderr: String::new(),
            }
        } else {
            runtime_err(format!("wasm trap: {e}"))
        }
    })?;

    match results[0] {
        Val::F64(v) => Ok(f64::from(v)),
        ref other => Err(runtime_err(format!(
            "wasm `improv_call` returned non-f64: {other:?}"
        ))),
    }
}

/// Tiny standard-base64 decoder (no `base64` crate dependency — the alphabet is
/// fixed and this keeps the dep tree small). Rejects invalid input.
fn base64_decode(s: &str) -> Result<Vec<u8>, String> {
    fn val(c: u8) -> Result<u8, String> {
        match c {
            b'A'..=b'Z' => Ok(c - b'A'),
            b'a'..=b'z' => Ok(c - b'a' + 26),
            b'0'..=b'9' => Ok(c - b'0' + 52),
            b'+' => Ok(62),
            b'/' => Ok(63),
            _ => Err(format!("invalid base64 char {:?}", c as char)),
        }
    }
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        if chunk.len() < 2 {
            return Err("truncated base64".into());
        }
        let b0 = val(chunk[0])?;
        let b1 = val(chunk[1])?;
        out.push((b0 << 2) | (b1 >> 4));
        if chunk.len() >= 3 && chunk[2] != b'=' {
            let b2 = val(chunk[2])?;
            out.push(((b1 & 0x0f) << 4) | (b2 >> 2));
            if chunk.len() == 4 && chunk[3] != b'=' {
                let b3 = val(chunk[3])?;
                out.push(((b2 & 0x03) << 6) | b3);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(wat: &str) -> Vec<u8> {
        load_module_bytes(&format!("wat:{wat}")).expect("valid wat")
    }

    /// An infinite loop runs out of fuel and the interpreter thread ends.
    #[cfg(feature = "wat")]
    #[test]
    fn infinite_loop_runs_out_of_fuel_and_thread_ends() {
        let wasm = module(
            r#"(module (func (export "improv_call") (result f64)
                 (loop br 0) f64.const 0))"#,
        );
        let start = std::time::Instant::now();
        let handle =
            std::thread::spawn(move || run_wasm(&wasm, &[], fuel_for(Duration::from_millis(500))));
        while !handle.is_finished() && start.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.is_finished(),
            "interpreter thread still running after 2 s"
        );
        let err = handle.join().unwrap().unwrap_err();
        assert!(format!("{err}").contains("out of fuel"), "got {err:?}");
    }

    /// Through `eval`, the same loop is an error (timeout or fuel), not a hang.
    #[cfg(feature = "wat")]
    #[test]
    fn eval_infinite_loop_is_an_error() {
        let f = ExternalFn {
            name: "spin".into(),
            language: crate::Language::Wasm,
            body:
                r#"wat:(module (func (export "improv_call") (result f64) (loop br 0) f64.const 0))"#
                    .into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        let err = eval(&f, &[], Duration::from_millis(300)).unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("timed out") || msg.contains("out of fuel"),
            "{msg}"
        );
    }

    /// `memory.grow` past [`MAX_MEMORY`] returns -1; within it succeeds.
    #[cfg(feature = "wat")]
    #[test]
    fn memory_grow_past_cap_fails() {
        let grow = |pages: u32| {
            let wasm = module(&format!(
                r#"(module (memory 1)
                     (func (export "improv_call") (result f64)
                       (f64.convert_i32_s (memory.grow (i32.const {pages})))))"#
            ));
            run_wasm(&wasm, &[], MAX_FUEL).expect("call")
        };
        // 64 KiB pages: 1 + 1023 = 64 MiB is the cap exactly.
        assert_eq!(grow(1023), 1.0);
        assert_eq!(grow(1024), -1.0);
        let too_big = module(
            r#"(module (memory 2000) (func (export "improv_call") (result f64) f64.const 0))"#,
        );
        assert!(run_wasm(&too_big, &[], MAX_FUEL).is_err());
    }

    /// A body without a prefix is rejected without touching the filesystem.
    #[test]
    fn bare_path_body_is_rejected() {
        let err = load_module_bytes("/etc/passwd").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("must start with"), "{msg}");
        assert!(!msg.contains("/etc/passwd"), "{msg}");
    }

    #[test]
    fn fuel_scales_with_timeout() {
        assert_eq!(fuel_for(Duration::from_secs(1)), FUEL_PER_SEC);
        assert_eq!(fuel_for(Duration::from_secs(3600)), MAX_FUEL);
        assert_eq!(fuel_for(Duration::ZERO), 1);
    }
}
