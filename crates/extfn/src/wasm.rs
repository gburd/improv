//! WebAssembly runner (in-process, via the `wasmi` interpreter).
//!
//! Unlike the scripting runners there is no subprocess: `wasmi` is a pure-Rust
//! Wasm **interpreter** (no JIT, MSRV-friendly). The `body` is the module
//! source, in one of two forms:
//!
//! * `wat:<text>`  — WebAssembly Text, assembled at load time via the `wat`
//!   crate. (Only enabled when the `wat` feature is on; on by default.)
//! * `base64:<b64>` — base64-encoded `.wasm` bytes.
//! * otherwise — a filesystem **path** to a `.wasm` module.
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
//! # Timeout
//!
//! Wasm runs in-process, so there is no pid to SIGKILL. The module is executed
//! on a worker thread and joined with a wall-clock deadline; on overrun `eval`
//! returns [`ExtFnError::Timeout`]. (The interpreter thread is left to unwind on
//! its own — v1 assumes reviewed, numeric module bodies, same trust model as the
//! scripting bodies.)

use crate::{ExtFnError, ExternalFn};
use improv_core_model::{Value, ValueType};
use std::sync::mpsc;
use std::time::Duration;

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

    // Run on a worker thread and join with the wall-clock deadline.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(run_wasm(&wasm, &inputs));
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

/// Resolve `body` to Wasm bytes: `wat:`, `base64:`, or a filesystem path.
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
    std::fs::read(body.trim())
        .map_err(|e| runtime_err(format!("reading wasm module at {:?}: {e}", body.trim())))
}

/// Instantiate the module and call `improv_call(f64...) -> f64` via the untyped
/// API (so any arity works). Any wasmi error becomes a `Runtime` error.
fn run_wasm(wasm: &[u8], inputs: &[f64]) -> Result<f64, ExtFnError> {
    use wasmi::{Engine, Linker, Module, Store, Val};

    let runtime_err = |m: String| ExtFnError::Runtime {
        message: m,
        stderr: String::new(),
    };

    let engine = Engine::default();
    let module =
        Module::new(&engine, wasm).map_err(|e| runtime_err(format!("wasm compile error: {e}")))?;
    let mut store = Store::new(&engine, ());
    let linker = <Linker<()>>::new(&engine);
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
    func.call(&mut store, &params, &mut results)
        .map_err(|e| runtime_err(format!("wasm trap: {e}")))?;

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
