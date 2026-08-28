//! Julia subprocess runner.
//!
//! Invokes `julia --startup-file=no -` (program on stdin — Julia reads the
//! script from stdin when the file argument is `-`). Julia's standard library
//! does **not** ship a JSON parser (that lives in the `JSON` package), so the
//! driver uses a tiny hand-rolled decoder/encoder covering the scalar/vector
//! shapes this runtime marshals — no package install required. The user `body`
//! runs with `args` bound (a `Vector`) and must assign `result`; the driver
//! prints one `{"ok":..}` / `{"error":..}` envelope on stdout.
//! [`crate::runner`] enforces the wall-clock timeout with a hard kill.

use crate::marshal::value_to_json;
use crate::runner;
use crate::{ExtFnError, ExternalFn};
use improv_core_model::Value;
use serde_json::json;
use std::time::Duration;

fn program(body: &str, payload_json: &str) -> String {
    let payload_lit = jl_str_literal(payload_json);
    let mut p = String::new();
    p.push_str(JL_PRELUDE);
    p.push_str("_payload = _improv_from_json(");
    p.push_str(&payload_lit);
    p.push_str(")\n");
    p.push_str("args = _payload[\"args\"]\n");
    p.push_str("result = nothing\n");
    p.push_str("try\n");
    p.push_str(body);
    p.push('\n');
    p.push_str("    if result === nothing\n");
    p.push_str("        println(\"{\\\"error\\\":\\\"function body did not set `result`\\\"}\")\n");
    p.push_str("    else\n");
    p.push_str("        println(\"{\\\"ok\\\":\" * _improv_to_json(result) * \"}\")\n");
    p.push_str("    end\n");
    p.push_str("catch e\n");
    p.push_str("    println(\"{\\\"error\\\":\" * _improv_to_json(string(e)) * \"}\")\n");
    p.push_str("end\n");
    p
}

/// Run the function. Assumes arity/type checks already passed.
pub fn eval(f: &ExternalFn, args: &[Value], timeout: Duration) -> Result<Value, ExtFnError> {
    let payload = json!({
        "args": args.iter().map(value_to_json).collect::<Vec<_>>(),
    })
    .to_string();
    let prog = program(&f.body, &payload);
    let out = runner::run_interpreter(
        "julia",
        &["--startup-file=no", "-"],
        &prog,
        timeout,
        "julia",
    )?;
    runner::parse_envelope(&out.stdout, &out.stderr, f.return_type, "julia")
}

/// Encode a Rust string as a Julia string literal (Julia accepts JSON's
/// double-quoted escapes for our characters, and `$` is escaped defensively).
fn jl_str_literal(s: &str) -> String {
    let json = serde_json::Value::String(s.to_string()).to_string();
    json.replace('$', "\\$")
}

/// Minimal JSON codec in pure Julia (no `JSON` package). Objects -> `Dict`,
/// arrays -> `Vector{Any}`, numbers -> `Float64`.
const JL_PRELUDE: &str = r#"
function _improv_from_json(s::AbstractString)
    chars = collect(s)
    i = Ref(1)
    n = length(chars)
    skipws() = while i[] <= n && chars[i[]] in (' ', '\t', '\n', '\r'); i[] += 1; end
    function pstr()
        i[] += 1
        buf = IOBuffer()
        while i[] <= n && chars[i[]] != '"'
            if chars[i[]] == '\\'
                i[] += 1
                e = chars[i[]]
                write(buf, e == 'n' ? '\n' : e == 't' ? '\t' : e == 'r' ? '\r' : e)
            else
                write(buf, chars[i[]])
            end
            i[] += 1
        end
        i[] += 1
        String(take!(buf))
    end
    function pnum()
        start = i[]
        while i[] <= n && (chars[i[]] in ('-', '+', '.', 'e', 'E') || isdigit(chars[i[]]))
            i[] += 1
        end
        parse(Float64, String(chars[start:i[]-1]))
    end
    function parr()
        i[] += 1
        out = Any[]
        skipws()
        if chars[i[]] == ']'; i[] += 1; return out; end
        while true
            push!(out, pval())
            skipws()
            if chars[i[]] == ','; i[] += 1; continue; end
            if chars[i[]] == ']'; i[] += 1; break; end
        end
        out
    end
    function pobj()
        i[] += 1
        out = Dict{String,Any}()
        skipws()
        if chars[i[]] == '}'; i[] += 1; return out; end
        while true
            skipws()
            k = pstr()
            skipws(); i[] += 1  # skip ':'
            out[k] = pval()
            skipws()
            if chars[i[]] == ','; i[] += 1; continue; end
            if chars[i[]] == '}'; i[] += 1; break; end
        end
        out
    end
    function pval()
        skipws()
        c = chars[i[]]
        c == '{' ? pobj() :
        c == '[' ? parr() :
        c == '"' ? pstr() :
        c == 't' ? (i[] += 4; true) :
        c == 'f' ? (i[] += 5; false) :
        c == 'n' ? (i[] += 4; nothing) :
        pnum()
    end
    pval()
end

function _improv_to_json(x)
    if x === nothing
        return "null"
    elseif isa(x, Bool)
        return x ? "true" : "false"
    elseif isa(x, Number)
        return isinteger(x) ? string(Int(x)) : string(Float64(x))
    elseif isa(x, AbstractString)
        esc = replace(x, "\\" => "\\\\", "\"" => "\\\"", "\n" => "\\n")
        return "\"" * esc * "\""
    elseif isa(x, AbstractDict)
        parts = ["\"" * string(k) * "\":" * _improv_to_json(v) for (k, v) in x]
        return "{" * join(parts, ",") * "}"
    elseif isa(x, AbstractVector)
        return "[" * join([_improv_to_json(v) for v in x], ",") * "]"
    else
        return _improv_to_json(string(x))
    end
end
"#;
