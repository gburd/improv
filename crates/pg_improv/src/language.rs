//! `LANGUAGE improv`: SQL functions whose bodies are Improv formulas.
//!
//! ```sql
//! CREATE FUNCTION margin(price float8, cost float8) RETURNS float8
//!   LANGUAGE improv IMMUTABLE AS $$ price - cost $$;
//! ```
//!
//! Each SQL argument is a scalar input measure named after it; each
//! `Name = expr` line of the body is a derived measure; the result is a
//! trailing bare expression, else the last definition. Lines are the DSL or
//! controlled English (as in the GUI). Evaluation is
//! [`improv_engine::scalar::eval`], the engine's own per-cell operators: a
//! NULL argument is a missing input (NULL out where the result depends on
//! it), and an error value (`#DIV/0!` for division by zero, `#NUM!`, …) is
//! NULL, or raised when `improv.raise_errors` is on.
//!
//! A trusted language: bodies compute and nothing else, so any role may
//! create one. Compiled bodies are cached per backend, keyed by function OID
//! and invalidated when the `pg_proc` row's version (xmin, ctid) changes.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::rc::Rc;

use improv_core_model::{
    parser, DimensionSpec, Expr, Formula, Measure, MeasureId, MeasureKind, Model, Name, ValueType,
};
use improv_engine::compiler::{infer, CompileContext};
use improv_engine::scalar::{self, ScalarError};
use improv_engine::CellValue;
use improv_nl_formula::{parse_nl_formula, NlContext, NlError};
use pgrx::pg_catalog::pg_proc::{PgProc, ProArgMode};
use pgrx::pg_sys::{self, Oid};
use pgrx::prelude::*;

extension_sql!(
    r#"
CREATE FUNCTION improv.plimprov_call_handler() RETURNS language_handler
    AS 'MODULE_PATHNAME', 'plimprov_call_handler' LANGUAGE C;
CREATE FUNCTION improv.plimprov_validator(oid) RETURNS void
    AS 'MODULE_PATHNAME', 'plimprov_validator' LANGUAGE C STRICT;
CREATE TRUSTED LANGUAGE improv
    HANDLER improv.plimprov_call_handler VALIDATOR improv.plimprov_validator;
COMMENT ON LANGUAGE improv IS 'SQL functions written in Improv''s formula language';
"#,
    name = "plimprov",
    requires = ["catalog"]
);

/// Days and microseconds from the Unix epoch to Postgres's (2000-01-01).
const PG_EPOCH_DAYS: i64 = 10_957;
const PG_EPOCH_MS: i64 = PG_EPOCH_DAYS * MS_PER_DAY;
const MS_PER_DAY: i64 = 86_400_000;

/// A function body ready to run: argument `i` is `MeasureId(i + 1)`.
struct Compiled {
    args: Vec<Oid>,
    steps: Vec<(MeasureId, Expr)>,
    result: Expr,
    ret: Oid,
}

/// A function's checked signature.
struct Sig {
    names: Vec<String>,
    args: Vec<Oid>,
    ret: Oid,
}

/// The Improv value type a supported SQL type maps to.
fn value_type(t: Oid) -> Option<ValueType> {
    Some(match t {
        pg_sys::FLOAT8OID
        | pg_sys::FLOAT4OID
        | pg_sys::NUMERICOID
        | pg_sys::INT2OID
        | pg_sys::INT4OID
        | pg_sys::INT8OID => ValueType::Number,
        pg_sys::BOOLOID => ValueType::Boolean,
        pg_sys::TEXTOID | pg_sys::VARCHAROID => ValueType::Text,
        pg_sys::DATEOID | pg_sys::TIMESTAMPOID | pg_sys::TIMESTAMPTZOID => ValueType::DateTime,
        _ => return None,
    })
}

fn type_name(t: Oid) -> String {
    // SAFETY: format_type_be returns a palloc'd C string (or raises ERROR).
    unsafe { CStr::from_ptr(pg_sys::format_type_be(t)) }
        .to_string_lossy()
        .into_owned()
}

fn signature(proc: &PgProc) -> Result<Sig, String> {
    if proc.proretset() {
        return Err("LANGUAGE improv functions cannot return sets".into());
    }
    if proc.proargmodes().iter().any(|m| *m != ProArgMode::In) {
        return Err("LANGUAGE improv functions take only IN arguments".into());
    }
    let ret = proc.prorettype();
    if value_type(ret).is_none() {
        return Err(format!(
            "LANGUAGE improv functions cannot return type {}",
            type_name(ret)
        ));
    }
    let args = proc.proargtypes();
    let all_names = proc.proargnames();
    let mut names = Vec::with_capacity(args.len());
    for (i, t) in args.iter().enumerate() {
        let name = all_names
            .get(i)
            .cloned()
            .flatten()
            .filter(|n| !n.is_empty())
            .ok_or_else(|| {
                format!(
                    "argument {} has no name; LANGUAGE improv refers to arguments by name",
                    i + 1
                )
            })?;
        if value_type(*t).is_none() {
            return Err(format!(
                "argument {name} has type {}, which LANGUAGE improv does not support",
                type_name(*t)
            ));
        }
        names.push(name);
    }
    Ok(Sig { names, args, ret })
}

/// `Name = rhs` as `(Name, rhs)`; a comparison (`a == b`, `a <= b`) is not one.
fn split_definition(line: &str) -> Option<(&str, &str)> {
    let (lhs, rhs) = line.split_once('=')?;
    let name = lhs.trim();
    let ident = name
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    (ident && !rhs.starts_with('=')).then_some((name, rhs))
}

/// Parse one formula as the DSL, else as controlled English; reject what a
/// scalar evaluation cannot do.
fn parse(model: &Model, text: &str) -> Result<Expr, String> {
    let expr = match parser::parse_expr(model, text) {
        Ok(f) => f.expr,
        Err(dsl) => match parse_nl_formula(&NlContext::new(model), text) {
            Ok(f) => f.expr,
            // A scalar function has no categories, so naming one (an
            // aggregation's OVER, a BY list) is asking for dimensions.
            Err(nl) => {
                let category = match nl {
                    NlError::UnknownCategory(c) => Some(format!("unknown category: {c}")),
                    _ => dsl
                        .message
                        .starts_with("unknown category")
                        .then(|| dsl.message.clone()),
                };
                return Err(match category {
                    Some(c) => ScalarError::NeedsDimensions(format!(
                        "an aggregation or dimension reference ({c})"
                    ))
                    .to_string(),
                    None => dsl.to_string(),
                });
            }
        },
    };
    scalar::check(&expr).map_err(|e| e.to_string())?;
    Ok(expr)
}

fn type_of(model: &Model, expr: &Expr) -> Result<ValueType, String> {
    let ctx = CompileContext::new(&model.measures);
    infer(&ctx, MeasureId(0), expr)
        .map(|t| t.ty.value_type)
        .map_err(|e| e.to_string())
}

fn compile(sig: Sig, src: &str) -> Result<Compiled, String> {
    let mut model = Model::new();
    let add = |model: &mut Model, name: &str, value_type, kind| {
        let id = MeasureId(model.measures.len() as u32 + 1);
        model.add_measure(Measure {
            id,
            name: Name(name.into()),
            value_type,
            categories: vec![],
            kind,
            description: None,
        });
        id
    };
    for (name, t) in sig.names.iter().zip(&sig.args) {
        add(
            &mut model,
            name,
            value_type(*t).unwrap(),
            MeasureKind::Input,
        );
    }
    let lines: Vec<&str> = src
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let mut steps = Vec::new();
    let mut result = None;
    for (n, line) in lines.iter().enumerate() {
        let at = |e: String| format!("line {}: {e}", n + 1);
        let last = n + 1 == lines.len();
        match split_definition(line) {
            Some((name, rhs)) => {
                if model.measure_by_name(name).is_some() {
                    return Err(at(format!("{name} is already defined")));
                }
                let expr = parse(&model, rhs).map_err(at)?;
                let vt = type_of(&model, &expr).map_err(at)?;
                let derived = MeasureKind::Derived(Formula::new(expr.clone()));
                let id = add(&mut model, name, vt, derived);
                steps.push((id, expr));
                if last {
                    result = Some((Expr::Ref(id, DimensionSpec::default()), vt));
                }
            }
            None if last => {
                let expr = parse(&model, line).map_err(at)?;
                result = Some((expr.clone(), type_of(&model, &expr).map_err(at)?));
            }
            None => {
                return Err(at(
                    "only the last line may be a bare expression; name this one (Name = ...)"
                        .into(),
                ))
            }
        }
    }
    let (result, vt) = result.ok_or("the function body is empty")?;
    if Some(vt) != value_type(sig.ret) {
        return Err(format!(
            "the body yields {vt:?} but the function returns {}",
            type_name(sig.ret)
        ));
    }
    Ok(Compiled {
        args: sig.args,
        steps,
        result,
        ret: sig.ret,
    })
}

/// Argument `i` of SQL type `t` as an Improv value; `None` for SQL NULL.
///
/// # Safety
/// `fcinfo` is a live call with at least `i + 1` arguments, argument `i` of type `t`.
unsafe fn arg(
    fcinfo: pg_sys::FunctionCallInfo,
    i: usize,
    t: Oid,
) -> Result<Option<CellValue>, String> {
    use pgrx::pg_getarg;
    // SAFETY: per the contract, each read matches the argument's type.
    let v = unsafe {
        match t {
            pg_sys::FLOAT8OID => pg_getarg::<f64>(fcinfo, i).map(CellValue::num),
            pg_sys::FLOAT4OID => pg_getarg::<f32>(fcinfo, i).map(|x| CellValue::num(x.into())),
            pg_sys::NUMERICOID => match pg_getarg::<AnyNumeric>(fcinfo, i) {
                Some(n) => Some(CellValue::num(f64::try_from(n).map_err(|e| e.to_string())?)),
                None => None,
            },
            pg_sys::INT2OID => pg_getarg::<i16>(fcinfo, i).map(|x| CellValue::num(x.into())),
            pg_sys::INT4OID => pg_getarg::<i32>(fcinfo, i).map(|x| CellValue::num(x.into())),
            // Exact to 2^53, like every Improv number.
            pg_sys::INT8OID => pg_getarg::<i64>(fcinfo, i).map(|x| CellValue::num(x as f64)),
            pg_sys::BOOLOID => pg_getarg::<bool>(fcinfo, i).map(CellValue::Bool),
            pg_sys::TEXTOID | pg_sys::VARCHAROID => {
                pg_getarg::<String>(fcinfo, i).map(CellValue::Text)
            }
            pg_sys::DATEOID => pg_getarg::<i32>(fcinfo, i)
                .map(|d| CellValue::Date((i64::from(d) + PG_EPOCH_DAYS) * MS_PER_DAY)),
            // Microseconds since 2000-01-01; Improv keeps milliseconds.
            _ => pg_getarg::<i64>(fcinfo, i)
                .map(|us| CellValue::Date(us.div_euclid(1000) + PG_EPOCH_MS)),
        }
    };
    Ok(v)
}

/// `v` as a Datum of the function's return type.
fn to_datum(v: &CellValue, ret: Oid) -> Result<pg_sys::Datum, String> {
    let out_of_range = || format!("{} out of range", type_name(ret));
    let int = |f: f64, lo: f64, hi: f64| {
        // float8 -> integer rounds half to even, as Postgres's casts do.
        let r = f.round_ties_even();
        (r >= lo && r < hi).then_some(r).ok_or_else(out_of_range)
    };
    let d = match (ret, v) {
        (_, CellValue::Err(_)) => return Err(format!("the formula evaluated to {v}")),
        (pg_sys::FLOAT8OID, CellValue::Num(_)) => v.as_num().into_datum(),
        (pg_sys::FLOAT4OID, CellValue::Num(_)) => v.as_num().map(|f| f as f32).into_datum(),
        (pg_sys::NUMERICOID, CellValue::Num(_)) => {
            AnyNumeric::try_from(v.as_num().unwrap_or(f64::NAN))
                .map_err(|e| e.to_string())?
                .into_datum()
        }
        (pg_sys::INT2OID, CellValue::Num(_)) => {
            let r = int(v.as_num().unwrap_or(f64::NAN), -32_768.0, 32_768.0)?;
            (r as i16).into_datum()
        }
        (pg_sys::INT4OID, CellValue::Num(_)) => {
            let r = int(
                v.as_num().unwrap_or(f64::NAN),
                -2_147_483_648.0,
                2_147_483_648.0,
            )?;
            (r as i32).into_datum()
        }
        (pg_sys::INT8OID, CellValue::Num(_)) => {
            let r = int(
                v.as_num().unwrap_or(f64::NAN),
                -9.223_372_036_854_776e18,
                9.223_372_036_854_776e18,
            )?;
            (r as i64).into_datum()
        }
        (pg_sys::BOOLOID, CellValue::Bool(b)) => b.into_datum(),
        (pg_sys::TEXTOID | pg_sys::VARCHAROID, CellValue::Text(s)) => s.as_str().into_datum(),
        (pg_sys::DATEOID, CellValue::Date(ms)) => {
            let days = ms.div_euclid(MS_PER_DAY) - PG_EPOCH_DAYS;
            i32::try_from(days)
                .map_err(|_| out_of_range())?
                .into_datum()
        }
        (pg_sys::TIMESTAMPOID | pg_sys::TIMESTAMPTZOID, CellValue::Date(ms)) => ms
            .checked_sub(PG_EPOCH_MS)
            .and_then(|us| us.checked_mul(1000))
            .ok_or_else(out_of_range)?
            .into_datum(),
        _ => None,
    };
    d.ok_or_else(|| format!("cannot return {v} as {}", type_name(ret)))
}

/// The `pg_proc` row version: its xmin and ctid (`CREATE OR REPLACE` in the
/// creating transaction keeps the xmin but moves the ctid).
type RowVersion = (pg_sys::TransactionId, (u32, u16));

fn row_version(oid: Oid) -> Result<RowVersion, String> {
    // SAFETY: a syscache lookup, released before returning.
    unsafe {
        let tup = pg_sys::SearchSysCache1(pg_sys::SysCacheIdentifier::PROCOID as _, oid.into());
        if tup.is_null() {
            return Err(format!(
                "cache lookup failed for function {}",
                u32::from(oid)
            ));
        }
        // The raw xmin field (HeapTupleHeaderGetRawXmin). Read directly:
        // PG 18's headers export a C function of the same name, which pgrx
        // also defines, so the name is ambiguous there.
        let xmin = (*(*tup).t_data).t_choice.t_heap.t_xmin;
        let tid = pgrx::itemptr::item_pointer_get_both((*tup).t_self);
        pg_sys::ReleaseSysCache(tup);
        Ok((xmin, tid))
    }
}

thread_local! {
    // ponytail: entries for dropped functions linger until backend exit
    // (a few hundred bytes each); evict on a syscache callback if it matters.
    static CACHE: RefCell<HashMap<Oid, (RowVersion, Rc<Compiled>)>> = RefCell::new(HashMap::new());
}

fn compiled(oid: Oid) -> Result<Rc<Compiled>, String> {
    let version = row_version(oid)?;
    let hit = CACHE.with_borrow(|c| {
        c.get(&oid)
            .filter(|(v, _)| *v == version)
            .map(|(_, f)| f.clone())
    });
    if let Some(f) = hit {
        return Ok(f);
    }
    let proc = PgProc::new(oid).ok_or("function vanished")?;
    let f = Rc::new(compile(signature(&proc)?, &proc.prosrc())?);
    CACHE.with_borrow_mut(|c| c.insert(oid, (version, f.clone())));
    Ok(f)
}

/// Evaluate `f` on the call's arguments; `Ok(None)` is SQL NULL.
///
/// # Safety
/// `fcinfo` is a live call of the function `f` was compiled from.
unsafe fn run(
    f: &Compiled,
    fcinfo: pg_sys::FunctionCallInfo,
) -> Result<Option<pg_sys::Datum>, String> {
    let mut env = HashMap::with_capacity(f.args.len() + f.steps.len());
    for (i, t) in f.args.iter().enumerate() {
        // SAFETY: `f` was compiled from this function's signature.
        if let Some(v) = unsafe { arg(fcinfo, i, *t)? } {
            env.insert(MeasureId(i as u32 + 1), v);
        }
    }
    let eval = |e: &Expr, env: &HashMap<_, _>| scalar::eval(e, env).map_err(|e| e.to_string());
    for (id, e) in &f.steps {
        if let Some(v) = eval(e, &env)? {
            env.insert(*id, v);
        }
    }
    match eval(&f.result, &env)? {
        // An error value is NULL unless improv.raise_errors asks to raise it.
        Some(CellValue::Err(_)) if !crate::RAISE_ERRORS.get() => Ok(None),
        v => v.map(|v| to_datum(&v, f.ret)).transpose(),
    }
}

fn raise(code: PgSqlErrorCode, msg: String) -> ! {
    ereport!(ERROR, code, msg);
}

/// The `LANGUAGE improv` call handler.
///
/// # Safety
/// Called by Postgres's fmgr with a valid `fcinfo`.
#[pg_guard]
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn plimprov_call_handler(
    fcinfo: pg_sys::FunctionCallInfo,
) -> pg_sys::Datum {
    // SAFETY: fmgr passes a valid fcinfo with flinfo set.
    let oid = unsafe { (*(*fcinfo).flinfo).fn_oid };
    let f = compiled(oid)
        .unwrap_or_else(|e| raise(PgSqlErrorCode::ERRCODE_INVALID_FUNCTION_DEFINITION, e));
    // SAFETY: `f` is this function's compiled body.
    match unsafe { run(&f, fcinfo) } {
        Ok(Some(d)) => d,
        // SAFETY: as above.
        Ok(None) => unsafe { pgrx::pg_return_null(fcinfo) },
        Err(e) => raise(PgSqlErrorCode::ERRCODE_DATA_EXCEPTION, e),
    }
}

/// The `LANGUAGE improv` validator: checks the signature always, and parses
/// and type-checks the body when `check_function_bodies` is on.
///
/// # Safety
/// Called by Postgres's fmgr with a valid `fcinfo`.
#[pg_guard]
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn plimprov_validator(
    fcinfo: pg_sys::FunctionCallInfo,
) -> pg_sys::Datum {
    // SAFETY: fmgr passes a valid fcinfo; the SQL declaration is (oid) STRICT.
    let (validator, func) = unsafe {
        (
            (*(*fcinfo).flinfo).fn_oid,
            pgrx::pg_getarg::<Oid>(fcinfo, 0).unwrap_or(pg_sys::InvalidOid),
        )
    };
    // SAFETY: plain catalog checks.
    if !unsafe { pg_sys::CheckFunctionValidatorAccess(validator, func) } {
        return pgrx::pg_return_void();
    }
    let proc = PgProc::new(func).unwrap_or_else(|| {
        raise(
            PgSqlErrorCode::ERRCODE_UNDEFINED_FUNCTION,
            "function vanished".into(),
        )
    });
    // SAFETY: reading a GUC variable on the backend thread.
    let check_body = unsafe { pg_sys::check_function_bodies };
    let checked = signature(&proc).and_then(|sig| {
        if check_body {
            compile(sig, &proc.prosrc()).map(drop)
        } else {
            Ok(())
        }
    });
    if let Err(e) = checked {
        raise(PgSqlErrorCode::ERRCODE_INVALID_FUNCTION_DEFINITION, e);
    }
    pgrx::pg_return_void()
}

/// Required by Postgres to call the handlers above (`PG_FUNCTION_INFO_V1`).
#[unsafe(no_mangle)]
#[doc(hidden)]
pub extern "C" fn pg_finfo_plimprov_call_handler() -> &'static pg_sys::Pg_finfo_record {
    const V1: pg_sys::Pg_finfo_record = pg_sys::Pg_finfo_record { api_version: 1 };
    &V1
}

#[unsafe(no_mangle)]
#[doc(hidden)]
pub extern "C" fn pg_finfo_plimprov_validator() -> &'static pg_sys::Pg_finfo_record {
    const V1: pg_sys::Pg_finfo_record = pg_sys::Pg_finfo_record { api_version: 1 };
    &V1
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    fn run(sql: &str) {
        Spi::run(sql).unwrap();
    }

    fn one<T: FromDatum + IntoDatum>(sql: &str) -> Option<T> {
        Spi::get_one::<T>(sql).unwrap()
    }

    /// The message `sql` fails with, or "no error". Runs in a subtransaction
    /// (a PL/pgSQL exception block) so the test's transaction survives.
    fn error_of(sql: &str) -> String {
        run(&format!(
            "DO $do$ BEGIN EXECUTE $sql${sql}$sql$; \
               PERFORM set_config('plimprov.err', 'no error', true); \
             EXCEPTION WHEN others THEN PERFORM set_config('plimprov.err', SQLERRM, true); \
             END $do$"
        ));
        one::<String>("SELECT current_setting('plimprov.err')").unwrap()
    }

    fn create(sig: &str, body: &str) {
        run(&format!(
            "CREATE FUNCTION {sig} LANGUAGE improv AS $body${body}$body$"
        ));
    }

    fn create_error(sig: &str, body: &str) -> String {
        error_of(&format!(
            "CREATE FUNCTION {sig} LANGUAGE improv AS $body${body}$body$"
        ))
    }

    #[pg_test]
    fn margin_over_a_table() {
        run("CREATE TABLE sales (product text, price float8, cost float8)");
        run("INSERT INTO sales VALUES ('Widget', 10, 4), ('Gadget', 20, 5.5)");
        create(
            "margin(price float8, cost float8) RETURNS float8 IMMUTABLE",
            "price - cost",
        );
        assert_eq!(
            one::<String>(
                "SELECT string_agg(product || '=' || margin(price, cost), ',' ORDER BY product) \
                 FROM sales"
            ),
            Some("Gadget=14.5,Widget=6".into())
        );
    }

    #[pg_test]
    fn definitions_and_builtins() {
        create(
            "bonus(sales float8, target float8) RETURNS float8",
            "
              Ratio = sales / target
              MAX2(ROUND(Ratio * 100) - 100, 0) * 10
            ",
        );
        assert_eq!(one::<f64>("SELECT bonus(150, 100)"), Some(500.0));
        assert_eq!(one::<f64>("SELECT bonus(1234, 1000)"), Some(230.0));
        assert_eq!(one::<f64>("SELECT bonus(50, 100)"), Some(0.0));
        // With no trailing expression the last definition is the result.
        create("double(x float8) RETURNS float8", "Y = x + x\nZ = Y * 1");
        assert_eq!(one::<f64>("SELECT double(4)"), Some(8.0));
    }

    #[pg_test]
    fn controlled_english() {
        create(
            "revenue(price float8, qty float8) RETURNS float8",
            "price times qty",
        );
        assert_eq!(one::<f64>("SELECT revenue(3, 4)"), Some(12.0));
    }

    #[pg_test]
    fn booleans_and_text() {
        create("gt(a float8, b float8) RETURNS bool", "a > b");
        assert_eq!(one::<bool>("SELECT gt(2, 1)"), Some(true));
        assert_eq!(one::<bool>("SELECT gt(1, 2)"), Some(false));
        create("is_widget(p text) RETURNS bool", r#"p == "Widget""#);
        assert_eq!(one::<bool>("SELECT is_widget('Widget')"), Some(true));
        assert_eq!(one::<bool>("SELECT is_widget('Gadget')"), Some(false));
        create("echo(p varchar) RETURNS text", "p");
        assert_eq!(one::<String>("SELECT echo('abc')"), Some("abc".into()));
    }

    #[pg_test]
    fn numeric_integer_and_date_types() {
        create("scale(a numeric, b int8) RETURNS numeric", "a * b");
        assert_eq!(
            one::<String>("SELECT scale(1.5, 4)::text"),
            Some("6".into())
        );
        // Integer results round half to even, as Postgres's float casts do.
        create("half(a int2) RETURNS int4", "a * 2 + 0.5");
        assert_eq!(one::<i32>("SELECT half(2::int2)"), Some(4));
        create("f4(a float4) RETURNS float4", "a / 4");
        assert_eq!(one::<f32>("SELECT f4(1)"), Some(0.25));
        create("same_day(d date) RETURNS date", "d");
        assert_eq!(
            one::<bool>("SELECT same_day('2024-02-29') = '2024-02-29'::date"),
            Some(true)
        );
        assert_eq!(
            one::<bool>("SELECT same_day('1999-12-31') = '1999-12-31'::date"),
            Some(true)
        );
        create("same_ts(t timestamptz) RETURNS timestamptz", "t");
        let ts = "'2024-05-06 07:08:09.123+00'::timestamptz";
        assert_eq!(
            one::<bool>(&format!("SELECT same_ts({ts}) = {ts}")),
            Some(true)
        );
        create("same_naive(t timestamp) RETURNS timestamp", "t");
        assert_eq!(
            one::<bool>("SELECT same_naive('1970-01-01 00:00:01') = '1970-01-01 00:00:01'"),
            Some(true)
        );
    }

    #[pg_test]
    fn null_argument_is_null() {
        create(
            "margin(price float8, cost float8) RETURNS float8",
            "price - cost",
        );
        assert_eq!(one::<bool>("SELECT margin(NULL, 1) IS NULL"), Some(true));
        // A result that does not depend on the NULL argument is computed.
        create("first(a float8, b float8) RETURNS float8", "a");
        assert_eq!(one::<f64>("SELECT first(7, NULL)"), Some(7.0));
    }

    #[pg_test]
    fn error_values_are_null_or_raised() {
        create("ratio(a float8, b float8) RETURNS float8", "a / b");
        assert_eq!(one::<bool>("SELECT ratio(1, 0) IS NULL"), Some(true));
        assert_eq!(one::<bool>("SELECT ratio(0, 0) IS NULL"), Some(true));
        create("iratio(a float8, b float8) RETURNS int4", "a / b");
        assert_eq!(one::<bool>("SELECT iratio(1, 0) IS NULL"), Some(true));
        // IFERROR replaces it; functions and IF work in a body.
        create(
            "safe(a float8, b float8) RETURNS float8",
            "IFERROR(a / b, -1)",
        );
        assert_eq!(one::<f64>("SELECT safe(1, 0)"), Some(-1.0));
        create(
            "grade(s float8) RETURNS text",
            r#"IF(s >= 90, "A", IF(s >= 80, "B", "C"))"#,
        );
        assert_eq!(one::<String>("SELECT grade(85)"), Some("B".into()));
        create("r2(x float8) RETURNS float8", "ROUND(x, 2)");
        assert_eq!(one::<f64>("SELECT r2(2.675)"), Some(2.68));
        Spi::run("SET improv.raise_errors = on").unwrap();
        assert_eq!(
            error_of("SELECT ratio(1, 0)"),
            "the formula evaluated to #DIV/0!"
        );
        Spi::run("RESET improv.raise_errors").unwrap();
    }

    #[pg_test]
    fn bad_bodies_fail_at_create() {
        let e = create_error("bad(a float8) RETURNS float8", "a +");
        assert!(e.starts_with("line 1: parse error"), "{e}");
        let e = create_error("bad(a float8) RETURNS float8", "b + 1");
        assert!(e.contains("unknown measure: b"), "{e}");
        let e = create_error("bad(a float8) RETURNS bool", "a + 1");
        assert_eq!(e, "the body yields Number but the function returns boolean");
        let e = create_error("bad(a float8) RETURNS float8", "a + 1\na");
        assert!(
            e.contains("only the last line may be a bare expression"),
            "{e}"
        );
        let e = create_error("bad(a float8) RETURNS float8", "a = 1");
        assert_eq!(e, "line 1: a is already defined");
        let e = create_error("bad(a float8) RETURNS float8", "  ");
        assert_eq!(e, "the function body is empty");
        // check_function_bodies = off defers the body check to the first call.
        run("SET LOCAL check_function_bodies = off");
        create("later(a float8) RETURNS float8", "a +");
        run("RESET check_function_bodies");
        assert!(error_of("SELECT later(1)").starts_with("line 1: parse error"));
    }

    #[pg_test]
    fn signatures_are_checked() {
        let e = create_error("anon(float8) RETURNS float8", "1");
        assert_eq!(
            e,
            "argument 1 has no name; LANGUAGE improv refers to arguments by name"
        );
        let e = create_error("arr(a int4[]) RETURNS float8", "1");
        assert_eq!(
            e,
            "argument a has type integer[], which LANGUAGE improv does not support"
        );
        let e = create_error("ret(a float8) RETURNS json", "a");
        assert_eq!(e, "LANGUAGE improv functions cannot return type json");
        let e = create_error("srf(a float8) RETURNS SETOF float8", "a");
        assert_eq!(e, "LANGUAGE improv functions cannot return sets");
        let e = create_error("outp(a float8, OUT b float8)", "a");
        assert_eq!(e, "LANGUAGE improv functions take only IN arguments");
    }

    #[pg_test]
    fn aggregations_need_a_model() {
        let e = create_error("total(x float8) RETURNS float8", "SUM(x OVER T)");
        assert!(e.contains("needs dimensions"), "{e}");
        assert!(e.contains("Improv model"), "{e}");
        let e = create_error("total(x float8) RETURNS float8", "the sum of x over Time");
        assert!(e.contains("needs dimensions"), "{e}");
    }

    #[pg_test]
    fn create_or_replace_recompiles() {
        create("f(x float8) RETURNS float8", "x + 1");
        assert_eq!(one::<f64>("SELECT f(1)"), Some(2.0));
        run("CREATE OR REPLACE FUNCTION f(x float8) RETURNS float8 LANGUAGE improv AS 'x * 10'");
        assert_eq!(one::<f64>("SELECT f(1)"), Some(10.0));
    }

    #[pg_test]
    fn trusted_for_non_superusers() {
        assert_eq!(
            one::<bool>("SELECT lanpltrusted FROM pg_language WHERE lanname = 'improv'"),
            Some(true)
        );
        run("CREATE ROLE plimprov_user");
        run("GRANT CREATE, USAGE ON SCHEMA public TO plimprov_user");
        run("SET ROLE plimprov_user");
        create("public.twice(x float8) RETURNS float8", "x * 2");
        assert_eq!(one::<f64>("SELECT public.twice(21)"), Some(42.0));
        run("RESET ROLE");
    }

    #[pg_test]
    fn many_calls_match_sql() {
        create(
            "margin(price float8, cost float8) RETURNS float8 IMMUTABLE",
            "price - cost",
        );
        assert_eq!(
            one::<bool>(
                "SELECT sum(margin(g, g / 2.0)) = sum(g - g / 2.0) \
                 FROM generate_series(1, 100000) g"
            ),
            Some(true)
        );
    }
}
