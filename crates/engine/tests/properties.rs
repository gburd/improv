//! Property-based tests (hegel) over the whole built-in function registry:
//! every scalar function, every reduction, and the families' round trips
//! and independent oracles.
//!
//! The registry-driven properties walk [`functions::all`], so a function
//! added to the registry is covered without touching this file: each one
//! gets a small hegel run of its own. Every evaluation runs on a worker
//! thread under a time budget, so a call that never returns is a shrunk
//! counterexample, not a stuck suite.
//!
//! Knobs: `PBT_ONLY=id,id` restricts the per-function runs, `PBT_CASES` sets
//! their case count, `PBT_BUDGET_MS` the time budget (default 2000; a low
//! value in a release build finds calls that are merely slow, e.g.
//! `PBT_BUDGET_MS=50 cargo test --release`). The `#[ignore]`d `*_long`
//! tests are the heavier runs.
//!
//! Each `fix(engine)` these found carries its own regression test in the
//! module it fixed (FACT/COMBIN overflow, VDB, CUMIPMT, AMORDEGRC,
//! OPT_BINOMIAL and Bessel caps, BINOM.INV and the R.Q* bisections, the
//! beta kernels, FMRR, HDATE, IMIGAMMA, FLOOR/CEILING, PV/FV, UNIQUE).

use hegel::generators::{self as gs};
use hegel::{HealthCheck, Hegel, Settings, TestCase};
use improv_core_model::functions::{self, Arg, Category, FnDef, Kind, Over, Ret};
use improv_core_model::{Expr, FuncId, Value, ValueError, ValueErrorKind, ValueType};
use improv_engine::reduce::{self, Rows};
use improv_engine::{builtins, scalar, CellValue};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::OnceLock;
use std::time::Duration;

// ---------------------------------------------------------------------------
// The time budget
// ---------------------------------------------------------------------------

/// How long one evaluation may take before it counts as a hang.
fn budget() -> Duration {
    static B: OnceLock<Duration> = OnceLock::new();
    *B.get_or_init(|| {
        let ms = std::env::var("PBT_BUDGET_MS")
            .ok()
            .and_then(|s| s.parse().ok());
        Duration::from_millis(ms.unwrap_or(2000))
    })
}

/// Run `f` on a worker thread; fail if it panics or does not answer within
/// the budget (a hung worker is left behind: the case fails either way).
fn timed<T: Send + 'static>(
    what: impl FnOnce() -> String,
    f: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(budget()) {
        Ok(v) => v,
        Err(RecvTimeoutError::Timeout) => {
            panic!("{} did not finish within {:?}", what(), budget())
        }
        Err(RecvTimeoutError::Disconnected) => panic!("{} panicked", what()),
    }
}

fn settings(cases: u64) -> Settings {
    Settings::new()
        .test_cases(cases)
        .database(None)
        .suppress_health_check(HealthCheck::all())
}

// ---------------------------------------------------------------------------
// Generators
// ---------------------------------------------------------------------------

/// 0001-01-01T00:00Z and 9999-12-31T23:59:59.999Z in epoch milliseconds.
const YEAR_1_MS: i64 = -62_135_596_800_000;
const YEAR_9999_MS: i64 = 253_402_300_799_999;
const DAY_MS: i64 = 86_400_000;

/// The ids that read the clock (`NOW`, `TODAY`): not deterministic.
const CLOCK: [u32; 2] = [427, 428];

/// The ids documented to see error arguments instead of propagating them:
/// `ISERROR`, `ISERR`, `ISNA`, `ERROR.TYPE`, `ISBLANK` … `ISNUMBERORDATE`,
/// `TYPE`, `VALUETOTEXT`.
fn inspects_errors(id: u32) -> bool {
    matches!(id, 50..=52 | 55 | 56..=62 | 67 | 334)
}

fn float(tc: &TestCase, lo: f64, hi: f64) -> f64 {
    tc.draw(gs::floats::<f64>().min_value(lo).max_value(hi))
}

fn int(tc: &TestCase, lo: i64, hi: i64) -> i64 {
    tc.draw(gs::integers::<i64>().min_value(lo).max_value(hi))
}

/// Any finite number, weighted toward the values functions branch on.
fn number(tc: &TestCase) -> f64 {
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => int(tc, -1000, 1000) as f64,
        1 => tc.draw(gs::sampled_from(vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            2.0,
            10.0,
            1e-300,
            5e-324,
            -5e-324,
            f64::MIN_POSITIVE,
            1e15,
            9_007_199_254_740_992.0,
            1e300,
            f64::MAX,
            f64::MIN,
        ])),
        2 => float(tc, -1e6, 1e6),
        _ => tc.draw(gs::floats::<f64>().allow_nan(false).allow_infinity(false)),
    }
}

/// Any text: full Unicode, or text shaped like what the parsing functions
/// read (numbers, radix digits, complex numbers, Roman numerals, units).
fn text(tc: &TestCase) -> String {
    match tc.draw(gs::integers::<u8>().max_value(2)) {
        0 => tc.draw(gs::text().max_size(24)),
        1 => tc.draw(gs::from_regex(
            r"[-+]?[0-9A-Fa-f]{0,10}(\.[0-9]{0,4})?([eE][-+]?[0-9]{1,3})?([-+][0-9.]{0,5}[ij])?",
        )),
        _ => tc
            .draw(gs::sampled_from(vec![
                "",
                " ",
                "a",
                "ab",
                "MCMXCIV",
                "xiv",
                "1+2i",
                "-3j",
                "i",
                "m",
                "ft",
                "kg",
                "lbm",
                "C",
                "F",
                "K",
                "*",
                "?",
                "~",
                "a*b",
                "2020-02-29",
                "12:30",
                "TRUE",
                "#N/A",
            ]))
            .to_string(),
    }
}

fn date(tc: &TestCase) -> i64 {
    int(tc, YEAR_1_MS, YEAR_9999_MS)
}

/// An error value (code 0 is Improv's own `#ERR`, 1–7 the spreadsheet ones).
fn error(tc: &TestCase) -> CellValue {
    CellValue::Err(tc.draw(gs::integers::<u8>().max_value(7)))
}

/// A value of kind `a`, now and then an error.
fn arg(tc: &TestCase, a: Arg) -> CellValue {
    if tc.draw(gs::weighted_booleans(0.04)) {
        return error(tc);
    }
    value_of(tc, a)
}

fn value_of(tc: &TestCase, a: Arg) -> CellValue {
    match a {
        Arg::Number => CellValue::num(number(tc)),
        Arg::Text => CellValue::Text(text(tc)),
        Arg::Boolean => CellValue::Bool(tc.draw(gs::booleans())),
        Arg::Date => CellValue::Date(date(tc)),
        Arg::DateLike => {
            if tc.draw(gs::booleans()) {
                CellValue::Date(date(tc))
            } else {
                CellValue::num(number(tc))
            }
        }
        Arg::Any | Arg::Category => {
            let k = tc.draw(gs::sampled_from(vec![
                Arg::Number,
                Arg::Text,
                Arg::Boolean,
                Arg::Date,
            ]));
            value_of(tc, k)
        }
    }
}

/// A number, text or boolean of the tame kind (no errors, no dates): what
/// the round-trip properties move around.
fn plain(tc: &TestCase) -> CellValue {
    match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 | 1 => CellValue::num(int(tc, -50, 50) as f64 / 4.0),
        2 => CellValue::Text(tc.draw(gs::from_regex("[a-cA-C]{0,3}"))),
        _ => CellValue::Bool(tc.draw(gs::booleans())),
    }
}

/// An argument count `def` accepts (variadic tails up to four extra).
fn arity(tc: &TestCase, def: &FnDef) -> usize {
    let max = if def.variadic {
        def.params.len() + 4
    } else {
        def.params.len()
    };
    tc.draw(
        gs::integers::<usize>()
            .min_value(def.required)
            .max_value(max),
    )
}

fn args_for(tc: &TestCase, def: &FnDef) -> Vec<CellValue> {
    let n = arity(tc, def);
    (0..n).map(|i| arg(tc, def.param(i))).collect()
}

/// Up to `max` rows of `value`s of kinds `cols`, at increasing item
/// positions with now and then a gap (an item without a cell).
fn rows_of(
    tc: &TestCase,
    cols: &[Arg],
    max: usize,
    value: impl Fn(&TestCase, Arg) -> CellValue,
) -> Rows {
    let len = tc.draw(gs::integers::<usize>().max_value(max));
    let mut at = u32::from(tc.draw(gs::weighted_booleans(0.2)));
    let (mut pos, mut vals) = (Vec::new(), Vec::new());
    for _ in 0..len {
        pos.push(vec![at]);
        vals.push(cols.iter().map(|&a| value(tc, a)).collect());
        at += 1 + u32::from(tc.draw(gs::weighted_booleans(0.2)));
    }
    let capacity = at as usize + tc.draw(gs::integers::<usize>().max_value(2));
    Rows {
        pos,
        vals,
        capacity,
    }
}

/// One row per entry of `vals`, at items 0, 1, 2, ….
fn table(vals: Vec<Vec<CellValue>>) -> Rows {
    Rows {
        pos: (0..vals.len() as u32).map(|i| vec![i]).collect(),
        capacity: vals.len(),
        vals,
    }
}

fn column(vals: &[CellValue]) -> Rows {
    table(vals.iter().map(|v| vec![v.clone()]).collect())
}

// ---------------------------------------------------------------------------
// The registry
// ---------------------------------------------------------------------------

/// Every scalar function the evaluator runs (the dimensional ones read a
/// category and compile to their own plan nodes).
fn scalar_defs() -> Vec<&'static FnDef> {
    functions::all()
        .filter(|d| d.kind == Kind::Scalar && d.category != Category::Dimensional)
        .collect()
}

fn reduction_defs() -> Vec<&'static FnDef> {
    functions::all().filter(|d| d.kind.reduces()).collect()
}

/// Draw one of `defs` (its name is shown in a counterexample).
fn draw_def(tc: &TestCase, defs: &[&'static FnDef]) -> &'static FnDef {
    let names: Vec<(u32, &'static str)> = defs.iter().map(|d| (d.id.0, d.name)).collect();
    let (id, _name) = tc.draw(gs::sampled_from(names));
    functions::by_id(FuncId(id)).expect("a registry id")
}

/// A small hegel run of `check` for each of `defs`; every failing function
/// is reported (not just the first).
fn per_def(defs: Vec<&'static FnDef>, cases: u64, check: fn(&TestCase, &'static FnDef)) {
    let only = std::env::var("PBT_ONLY").ok();
    let cases = std::env::var("PBT_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(cases);
    let (mut failed, mut unrun) = (Vec::new(), Vec::new());
    for def in defs {
        if let Some(only) = &only {
            if !only.split(',').any(|x| x == def.id.0.to_string()) {
                continue;
            }
        }
        let ran = AtomicU64::new(0);
        let run = std::panic::catch_unwind(|| {
            Hegel::new(|tc: TestCase| {
                ran.fetch_add(1, Relaxed);
                check(&tc, def);
            })
            .settings(settings(cases))
            .run();
        });
        let name = format!("{} ({})", def.name, def.id.0);
        if run.is_err() {
            failed.push(name);
        } else if ran.load(Relaxed) == 0 {
            unrun.push(name);
        }
    }
    assert!(failed.is_empty(), "failed: {}", failed.join(", "));
    assert!(unrun.is_empty(), "never ran: {}", unrun.join(", "));
}

fn scalar(name: &str) -> FuncId {
    functions::by_name(name)
        .find(|d| d.kind == Kind::Scalar)
        .unwrap_or_else(|| panic!("no scalar {name}"))
        .id
}

fn reduction(name: &str) -> FuncId {
    functions::by_name(name)
        .find(|d| d.kind.reduces())
        .unwrap_or_else(|| panic!("no reduction {name}"))
        .id
}

fn call(name: &str, args: &[CellValue]) -> CellValue {
    builtins::eval(scalar(name), args)
}

fn nv(x: f64) -> CellValue {
    CellValue::num(x)
}

fn tv(s: &str) -> CellValue {
    CellValue::Text(s.to_string())
}

/// The number `v` holds; `what` names the call when it holds none.
fn num_of(v: &CellValue, what: impl std::fmt::Display) -> f64 {
    v.as_num()
        .unwrap_or_else(|| panic!("{what} = {v:?}, not a number"))
}

fn text_of(v: &CellValue, what: impl std::fmt::Display) -> String {
    match v {
        CellValue::Text(s) => s.clone(),
        _ => panic!("{what} = {v:?}, not text"),
    }
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

fn to_value(v: &CellValue) -> Value {
    match v {
        CellValue::Num(b) => Value::Number(f64::from_bits(*b)),
        CellValue::Bool(b) => Value::Boolean(*b),
        CellValue::Text(s) => Value::Text(s.clone()),
        CellValue::Date(ms) => Value::DateTime(
            chrono::DateTime::from_timestamp_millis(*ms).expect("a date in years 1–9999"),
        ),
        CellValue::Err(c) => Value::Error(ValueError::new(
            ValueErrorKind::from_error_type(*c).unwrap_or(ValueErrorKind::MissingInput),
        )),
    }
}

/// Whether `v` is of the declared type `ret`. TEXTBEFORE and TEXTAFTER
/// (322, 323) return their if_not_found argument as it is, of any type.
fn ret_ok(def: &FnDef, args: &[CellValue], v: &CellValue) -> bool {
    if matches!(def.id.0, 322 | 323) && args.get(5) == Some(v) {
        return true;
    }
    match (def.ret, v) {
        (_, CellValue::Err(_)) => true,
        (Ret::Fixed(ValueType::Number | ValueType::Enum), CellValue::Num(_)) => true,
        (Ret::Fixed(ValueType::Boolean), CellValue::Bool(_)) => true,
        (Ret::Fixed(ValueType::Text), CellValue::Text(_)) => true,
        (Ret::Fixed(ValueType::DateTime), CellValue::Date(_)) => true,
        (Ret::Fixed(_), _) => false,
        (Ret::Unify | Ret::Arg(_), _) => true,
    }
}

// ---------------------------------------------------------------------------
// 1. Every scalar function
// ---------------------------------------------------------------------------

/// One case of a scalar function's contract: (a) it answers within the
/// budget without panicking, on typed arguments and on arguments of any
/// type (the scalar evaluator does not type-check; `pg_improv` hands it SQL
/// values); (b) the result is an error or a value of the declared type;
/// (c) the same arguments give the same result; (d) an error argument is
/// the result (the first, in argument order), except into the functions
/// that inspect errors; (e) the scalar evaluator on the call with literal
/// arguments gives exactly the built-in's value.
fn check_scalar(tc: &TestCase, def: &'static FnDef) {
    let args = args_for(tc, def);
    let mut with_err = args.clone();
    if !with_err.is_empty() {
        let at = tc.draw(gs::integers::<usize>().max_value(with_err.len() - 1));
        with_err[at] = error(tc);
    }
    let n = arity(tc, def);
    let any: Vec<CellValue> = (0..n).map(|_| arg(tc, Arg::Any)).collect();
    let (id, a, w, x) = (def.id, args.clone(), with_err.clone(), any.clone());
    let (v, again, by_scalar, err_v) = timed(
        || format!("{}{args:?} (or {with_err:?}, {any:?})", def.name),
        move || {
            let list = a.iter().map(|v| Expr::Literal(to_value(v))).collect();
            let by_scalar =
                scalar::eval(&Expr::Call(id, list), &HashMap::new()).map_err(|e| format!("{e:?}"));
            let _ = builtins::eval(id, &x);
            (
                builtins::eval(id, &a),
                builtins::eval(id, &a),
                by_scalar,
                builtins::eval(id, &w),
            )
        },
    );
    let name = def.name;
    assert!(
        ret_ok(def, &args, &v),
        "{name}{args:?} = {v:?}, not a {:?}",
        def.ret
    );
    if !CLOCK.contains(&def.id.0) {
        assert_eq!(v, again, "{name}{args:?} is not deterministic");
        assert_eq!(by_scalar, Ok(Some(v)), "scalar::eval of {name}{args:?}");
    }
    if !inspects_errors(def.id.0) {
        if let Some(first) = with_err.iter().find(|a| matches!(a, CellValue::Err(_))) {
            assert_eq!(&err_v, first, "{name}{with_err:?}");
        }
    }
}

/// Every scalar function, each its own small run.
#[test]
fn every_scalar_function_meets_its_contract() {
    let defs = scalar_defs();
    assert!(defs.len() > 500, "only {} scalar functions", defs.len());
    per_def(defs, 25, check_scalar);
}

/// The heavier run: a few hundred cases per function.
#[test]
#[ignore]
fn every_scalar_function_meets_its_contract_long() {
    per_def(scalar_defs(), 400, check_scalar);
}

/// Functions drawn from the whole registry, one long run.
#[hegel::test(test_cases = 3000, suppress_health_check = HealthCheck::all())]
fn scalar_contract_across_the_registry(tc: TestCase) {
    check_scalar(&tc, draw_def(&tc, &scalar_defs()));
}

// ---------------------------------------------------------------------------
// 2. Reductions, per-item and array functions
// ---------------------------------------------------------------------------

/// Random rows and parameters for `def`: the arguments its `OVER` reduces
/// become the rows' columns, the rest its parameters.
fn reduction_args(tc: &TestCase, def: &FnDef) -> (Rows, Vec<CellValue>) {
    let n = arity(tc, def);
    let lead = match def.over {
        Over::Leading(min) => {
            Some(tc.draw(gs::integers::<usize>().min_value(min.min(n)).max_value(n)))
        }
        _ => None,
    };
    let reduced = |i: usize| lead.map_or_else(|| def.over.contains(i), |k| i < k);
    let cols: Vec<Arg> = (0..n)
        .filter(|&i| reduced(i))
        .map(|i| def.param(i))
        .collect();
    let params = (0..n)
        .filter(|&i| !reduced(i))
        .map(|i| arg(tc, def.param(i)))
        .collect();
    (rows_of(tc, &cols, 10, arg), params)
}

fn run_reduction(def: &'static FnDef, rows: &Rows, params: &[CellValue]) -> Vec<CellValue> {
    let (id, kind, r, p) = (def.id, def.kind, rows.clone(), params.to_vec());
    timed(
        || format!("{}({rows:?}, {params:?})", def.name),
        move || match kind {
            Kind::PerItem => reduce::per_item(id, &r, &p),
            Kind::Array => reduce::array(id, &r, &p),
            _ => vec![reduce::reduce(id, &r, &p)],
        },
    )
}

/// Every reduction answers within the budget without panicking, the same
/// way twice; a per-item function gives one value per row, an array
/// function a non-empty list.
fn check_reduction(tc: &TestCase, def: &'static FnDef) {
    let (rows, params) = reduction_args(tc, def);
    let out = run_reduction(def, &rows, &params);
    let name = def.name;
    match def.kind {
        Kind::PerItem => assert_eq!(out.len(), rows.len(), "{name}({rows:?}, {params:?})"),
        // A group has at least one row (the dataflow has no empty groups).
        Kind::Array if !rows.is_empty() => {
            assert!(!out.is_empty(), "{name}({rows:?}, {params:?}) is empty")
        }
        Kind::Array => {}
        _ => assert_eq!(out.len(), 1),
    }
    assert_eq!(
        out,
        run_reduction(def, &rows, &params),
        "{name}({rows:?}, {params:?}) is not deterministic"
    );
}

#[test]
fn every_reduction_meets_its_contract() {
    per_def(reduction_defs(), 60, check_reduction);
}

#[test]
#[ignore]
fn every_reduction_meets_its_contract_long() {
    per_def(reduction_defs(), 1000, check_reduction);
}

/// The aggregations that ignore the order of their values.
const COMMUTATIVE: &[&str] = &[
    "SUM",
    "AVERAGE",
    "MIN",
    "MAX",
    "PRODUCT",
    "SUMSQ",
    "AND",
    "OR",
    "XOR",
    "SUMA",
    "AVERAGEA",
    "AVEDEV",
    "COUNT",
    "COUNTA",
    "COUNTBLANK",
    "MAXA",
    "MINA",
    "MEDIAN",
    "GEOMEAN",
    "HARMEAN",
    "TRIMMEAN",
    "DEVSQ",
    "STDEV",
    "STDEVP",
    "STDEVA",
    "STDEVPA",
    "VAR",
    "VARP",
    "VARA",
    "VARPA",
    "KURT",
    "KURTP",
    "SKEW",
    "SKEWP",
    "LARGE",
    "SMALL",
    "PERCENTILE",
    "PERCENTILE.EXC",
    "QUARTILE",
    "QUARTILE.EXC",
];

/// Equal, numbers within a relative 1e-9; any two errors count as equal (the
/// first error in item order moves with the values).
fn close_enough(a: &CellValue, b: &CellValue) -> bool {
    match (a, b) {
        (CellValue::Err(_), CellValue::Err(_)) => true,
        (CellValue::Num(_), CellValue::Num(_)) => {
            let (x, y) = (a.as_num().unwrap(), b.as_num().unwrap());
            (x - y).abs() <= 1e-9 * x.abs().max(y.abs()).max(1.0)
        }
        _ => a == b,
    }
}

/// A commutative aggregation of the same values in another item order is
/// the same (within rounding).
#[hegel::test(test_cases = 600, suppress_health_check = HealthCheck::all())]
fn commutative_aggregations_ignore_order(tc: TestCase) {
    let name = tc.draw(gs::sampled_from(COMMUTATIVE.to_vec()));
    let def = functions::by_id(reduction(name)).unwrap();
    let mut rows = rows_of(&tc, &[Arg::Any], 12, |tc, _| plain(tc));
    let params: Vec<CellValue> = (0..arity(&tc, def))
        .filter(|i| !def.over.contains(*i))
        .map(|_| match tc.draw(gs::booleans()) {
            true => nv(int(&tc, -1, 12) as f64),
            false => nv(float(&tc, 0.0, 1.0)),
        })
        .collect();
    let before = reduce::reduce(def.id, &rows, &params);
    let order: Vec<usize> = tc.draw(gs::permutations((0..rows.len()).collect::<Vec<_>>()));
    rows.vals = order.iter().map(|&i| rows.vals[i].clone()).collect();
    let after = reduce::reduce(def.id, &rows, &params);
    assert!(
        close_enough(&before, &after),
        "{name}: {before:?} became {after:?} ({params:?})"
    );
}

// ---------------------------------------------------------------------------
// 3. Family round trips and oracles
// ---------------------------------------------------------------------------

/// DEC2BIN, DEC2OCT and DEC2HEX (negatives as ten-digit two's complement)
/// invert under BIN2DEC, OCT2DEC and HEX2DEC.
#[hegel::test(test_cases = 300, suppress_health_check = HealthCheck::all())]
fn radix_round_trips(tc: TestCase) {
    let (to, from, bits) = tc.draw(gs::sampled_from(vec![
        ("DEC2BIN", "BIN2DEC", 9),
        ("DEC2OCT", "OCT2DEC", 29),
        ("DEC2HEX", "HEX2DEC", 39),
    ]));
    let x = int(&tc, -(1 << bits), (1 << bits) - 1) as f64;
    let s = call(to, &[nv(x)]);
    text_of(&s, format!("{to}({x})"));
    assert_eq!(call(from, std::slice::from_ref(&s)), nv(x), "{from}({s:?})");
}

/// BASE(x, radix, min_length) inverts under DECIMAL for every radix.
#[hegel::test(test_cases = 300, suppress_health_check = HealthCheck::all())]
fn base_decimal_round_trip(tc: TestCase) {
    let x = int(&tc, 0, (1 << 53) - 1) as f64;
    let radix = int(&tc, 2, 36) as f64;
    let min = int(&tc, 0, 60) as f64;
    let s = call("BASE", &[nv(x), nv(radix), nv(min)]);
    let digits = text_of(&s, format!("BASE({x}, {radix}, {min})"));
    assert!(digits.chars().count() >= min as usize, "{digits}");
    assert_eq!(call("DECIMAL", &[s.clone(), nv(radix)]), nv(x), "{s:?}");
}

/// ROMAN in every form inverts under ARABIC.
#[hegel::test(test_cases = 300, suppress_health_check = HealthCheck::all())]
fn roman_arabic_round_trip(tc: TestCase) {
    let x = int(&tc, 0, 3999) as f64;
    let form = int(&tc, 0, 4) as f64;
    let s = call("ROMAN", &[nv(x), nv(form)]);
    text_of(&s, format!("ROMAN({x}, {form})"));
    assert_eq!(
        call("ARABIC", std::slice::from_ref(&s)),
        nv(x),
        "ARABIC({s:?})"
    );
}

/// A finite number, often an integer, of magnitude up to 1e10.
fn moderate(tc: &TestCase) -> f64 {
    if tc.draw(gs::booleans()) {
        int(tc, -1000, 1000) as f64
    } else {
        float(tc, -1e10, 1e10)
    }
}

/// COMPLEX's parts come back from IMREAL and IMAGINARY (to its 15
/// significant digits), and IMSUM commutes.
#[hegel::test(test_cases = 300, suppress_health_check = HealthCheck::all())]
fn complex_round_trip(tc: TestCase) {
    let (re, im) = (moderate(&tc), moderate(&tc));
    let suffix = tv(tc.draw(gs::sampled_from(vec!["i", "j"])));
    let z = call("COMPLEX", &[nv(re), nv(im), suffix.clone()]);
    text_of(&z, format!("COMPLEX({re}, {im})"));
    let near = |got: f64, want: f64| (got - want).abs() <= 1e-14 * want.abs();
    let got = num_of(&call("IMREAL", std::slice::from_ref(&z)), "IMREAL");
    assert!(near(got, re), "IMREAL({z:?}) = {got}, want {re}");
    let got = num_of(&call("IMAGINARY", std::slice::from_ref(&z)), "IMAGINARY");
    assert!(near(got, im), "IMAGINARY({z:?}) = {got}, want {im}");
    let w = call("COMPLEX", &[nv(moderate(&tc)), nv(moderate(&tc)), suffix]);
    assert_eq!(
        call("IMSUM", &[z.clone(), w.clone()]),
        call("IMSUM", &[w, z])
    );
}

/// CONVERT there and back is the identity (within rounding).
#[hegel::test(test_cases = 300, suppress_health_check = HealthCheck::all())]
fn convert_round_trip(tc: TestCase) {
    let (a, b) = tc.draw(gs::sampled_from(vec![
        ("m", "ft"),
        ("km", "mi"),
        ("in", "Nmi"),
        ("g", "lbm"),
        ("kg", "stone"),
        ("C", "F"),
        ("K", "C"),
        ("F", "Rank"),
        ("Reau", "K"),
        ("J", "cal"),
        ("kWh", "BTU"),
        ("W", "HP"),
        ("l", "gal"),
        ("m3", "ft3"),
        ("byte", "bit"),
        ("kibyte", "Mbit"),
        ("Pa", "atm"),
        ("mmHg", "psi"),
        ("hr", "sec"),
        ("yr", "day"),
        ("m2", "ft2"),
        ("ha", "us_acre"),
        ("N", "lbf"),
        ("T", "ga"),
    ]));
    let x = moderate(&tc);
    let there = call("CONVERT", &[nv(x), tv(a), tv(b)]);
    let y = num_of(&there, format!("CONVERT({x}, {a}, {b})"));
    let back = num_of(&call("CONVERT", &[nv(y), tv(b), tv(a)]), "back");
    assert!(
        (back - x).abs() <= 1e-12 * (x.abs() + 1000.0),
        "CONVERT({x}, {a}, {b}) = {y}, back {back}"
    );
}

/// A distribution of the `R.` family: its CDF, its quantile (continuous
/// ones), and a draw of parameters, an x range and a scale.
struct Dist {
    p: &'static str,
    q: Option<&'static str>,
    discrete: bool,
    draw: Draw,
}

type Draw = fn(&TestCase) -> (Vec<f64>, (f64, f64), f64);

fn dists() -> Vec<Dist> {
    fn c(p: &'static str, q: &'static str, draw: Draw) -> Dist {
        Dist {
            p,
            q: Some(q),
            discrete: false,
            draw,
        }
    }
    fn d(p: &'static str, draw: Draw) -> Dist {
        Dist {
            p,
            q: None,
            discrete: true,
            draw,
        }
    }
    vec![
        c("R.PNORM", "R.QNORM", |tc| {
            let (m, s) = (float(tc, -100.0, 100.0), float(tc, 0.01, 100.0));
            (vec![m, s], (m - 10.0 * s, m + 10.0 * s), s)
        }),
        c("R.PLNORM", "R.QLNORM", |tc| {
            let (m, s) = (float(tc, -3.0, 3.0), float(tc, 0.1, 3.0));
            (vec![m, s], (0.0, (m + 5.0 * s).exp()), m.exp())
        }),
        c("R.PEXP", "R.QEXP", |tc| {
            let s = float(tc, 0.01, 100.0);
            (vec![s], (0.0, 40.0 * s), s)
        }),
        c("R.PGAMMA", "R.QGAMMA", |tc| {
            let (k, s) = (float(tc, 0.1, 50.0), float(tc, 0.01, 10.0));
            (vec![k, s], (0.0, (k + 10.0 * k.sqrt() + 10.0) * s), k * s)
        }),
        c("R.PBETA", "R.QBETA", |tc| {
            let (a, b) = (float(tc, 0.1, 50.0), float(tc, 0.1, 50.0));
            (vec![a, b], (0.0, 1.0), 1.0)
        }),
        c("R.PCHISQ", "R.QCHISQ", |tc| {
            let k = float(tc, 0.5, 100.0);
            (vec![k], (0.0, k + 20.0 * k.sqrt() + 20.0), k)
        }),
        c("R.PT", "R.QT", |tc| {
            (vec![float(tc, 0.5, 100.0)], (-50.0, 50.0), 1.0)
        }),
        c("R.PF", "R.QF", |tc| {
            let (a, b) = (float(tc, 0.5, 100.0), float(tc, 0.5, 100.0));
            (vec![a, b], (0.0, 50.0), 1.0)
        }),
        c("R.PWEIBULL", "R.QWEIBULL", |tc| {
            let (k, s) = (float(tc, 0.2, 10.0), float(tc, 0.1, 10.0));
            (vec![k, s], (0.0, 10.0 * s), s)
        }),
        c("R.PCAUCHY", "R.QCAUCHY", |tc| {
            let (m, s) = (float(tc, -100.0, 100.0), float(tc, 0.01, 100.0));
            (vec![m, s], (m - 1000.0 * s, m + 1000.0 * s), s)
        }),
        c("R.PGUMBEL", "R.QGUMBEL", |tc| {
            let (m, s) = (float(tc, -100.0, 100.0), float(tc, 0.01, 100.0));
            (vec![m, s], (m - 5.0 * s, m + 30.0 * s), s)
        }),
        c("R.PRAYLEIGH", "R.QRAYLEIGH", |tc| {
            let s = float(tc, 0.01, 100.0);
            (vec![s], (0.0, 10.0 * s), s)
        }),
        c("R.PSNORM", "R.QSNORM", |tc| {
            let (a, m, s) = (
                float(tc, -5.0, 5.0),
                float(tc, -100.0, 100.0),
                float(tc, 0.01, 100.0),
            );
            (vec![a, m, s], (m - 10.0 * s, m + 10.0 * s), s)
        }),
        // Integer degrees of freedom only (as in Gnumeric).
        c("R.PST", "R.QST", |tc| {
            let (k, a) = (int(tc, 1, 50) as f64, float(tc, -5.0, 5.0));
            (vec![k, a], (-30.0, 30.0), 1.0)
        }),
        d("R.PBINOM", |tc| {
            let n = int(tc, 0, 1000) as f64;
            (vec![n, float(tc, 0.0, 1.0)], (0.0, n), 1.0)
        }),
        d("R.PPOIS", |tc| {
            let l = float(tc, 0.0, 1000.0);
            (vec![l], (0.0, l + 10.0 * l.sqrt() + 10.0), 1.0)
        }),
        d("R.PGEOM", |tc| {
            (vec![float(tc, 0.001, 1.0)], (0.0, 1000.0), 1.0)
        }),
        d("R.PNBINOM", |tc| {
            let (s, p) = (float(tc, 0.1, 100.0), float(tc, 0.001, 1.0));
            (vec![s, p], (0.0, 1000.0), 1.0)
        }),
        d("R.PHYPER", |tc| {
            let (m, n) = (int(tc, 0, 200), int(tc, 0, 200));
            let k = int(tc, 0, m + n) as f64;
            (vec![m as f64, n as f64, k], (0.0, k), 1.0)
        }),
    ]
}

/// A distribution, its parameters, and an x in its range (or two, sorted).
fn draw_dist(tc: &TestCase) -> (Dist, Vec<f64>, f64, f64, f64) {
    let all = dists();
    let i = tc.draw(gs::integers::<usize>().max_value(all.len() - 1));
    let dist = all.into_iter().nth(i).unwrap();
    let (params, (lo, hi), scale) = (dist.draw)(tc);
    let x = || {
        let x = float(tc, lo, hi);
        if dist.discrete {
            x.floor()
        } else {
            x
        }
    };
    let (a, b) = (x(), x());
    (dist, params, a.min(b), a.max(b), scale)
}

/// `name(x, params…, lower_tail)`.
fn dist_call(name: &str, x: f64, params: &[f64], lower: bool) -> f64 {
    let mut args = vec![nv(x)];
    args.extend(params.iter().map(|p| nv(*p)));
    args.push(CellValue::Bool(lower));
    let id = scalar(name);
    let v = timed(
        || format!("{name}({x}, {params:?}, {lower})"),
        move || builtins::eval(id, &args),
    );
    num_of(&v, format!("{name}({x}, {params:?}, {lower})"))
}

/// The CDFs lie in [0, 1] and do not decrease.
#[hegel::test(test_cases = 500, suppress_health_check = HealthCheck::all())]
fn cdfs_are_monotone_probabilities(tc: TestCase) {
    let (d, params, x1, x2, _) = draw_dist(&tc);
    let (p1, p2) = (
        dist_call(d.p, x1, &params, true),
        dist_call(d.p, x2, &params, true),
    );
    assert!((0.0..=1.0).contains(&p1) && (0.0..=1.0).contains(&p2));
    assert!(
        p1 <= p2 + 1e-14,
        "{}: P({x1}) = {p1} > P({x2}) = {p2} ({params:?})",
        d.p
    );
}

/// The lower and the upper tail add up to 1.
#[hegel::test(test_cases = 500, suppress_health_check = HealthCheck::all())]
fn cdf_tails_add_up_to_one(tc: TestCase) {
    let (d, params, x, _, _) = draw_dist(&tc);
    let (lo, up) = (
        dist_call(d.p, x, &params, true),
        dist_call(d.p, x, &params, false),
    );
    assert!(
        (lo + up - 1.0).abs() <= 1e-12,
        "{}({x}, {params:?}): {lo} + {up}",
        d.p
    );
}

/// The quantile inverts a continuous CDF: Q(P(x)) ≈ x, each on the tail
/// where the probability keeps its digits. A CDF that cancels carries an
/// absolute error (R.PST's recurrence about 2e-17: R.PST(-15, 7, 4) is
/// 2.578828e-12 against mpmath's 2.578852e-12, as in Gnumeric), so P(Q(p))
/// ≈ p to that also passes.
#[hegel::test(test_cases = 500, suppress_health_check = HealthCheck::all())]
fn quantile_inverts_cdf(tc: TestCase) {
    let (d, params, x, _, scale) = draw_dist(&tc);
    let Some(q) = d.q else { return };
    let lower = dist_call(d.p, x, &params, true) <= 0.5;
    let p = dist_call(d.p, x, &params, lower);
    if p < 1e-300 {
        return;
    }
    let back = dist_call(q, p, &params, lower);
    let hits = || (dist_call(d.p, back, &params, lower) - p).abs() <= 1e-6 * p + 1e-15;
    assert!(
        (back - x).abs() <= 1e-7 * (x.abs() + scale) || hits(),
        "{q}({p}, {params:?}, {lower}) = {back}, want {x}"
    );
}

/// PV of PMT's payment is the present value back.
#[hegel::test(test_cases = 500, suppress_health_check = HealthCheck::all())]
fn pv_inverts_pmt(tc: TestCase) {
    // Rates from 0.01% (or 0) and amounts in cents: an amount of 1e-268
    // times a rate of 1e-50 is subnormal.
    let rate = if tc.draw(gs::weighted_booleans(0.1)) {
        0.0
    } else {
        float(&tc, 1e-4, 0.5) * if tc.draw(gs::booleans()) { 1.0 } else { -0.1 }
    };
    let n = int(&tc, 1, 480) as f64;
    let cents = |tc: &TestCase| int(tc, -100_000_000, 100_000_000) as f64 / 100.0;
    let (pv, fv) = (cents(&tc), cents(&tc));
    let ty = nv(int(&tc, 0, 1) as f64);
    let pmt = call("PMT", &[nv(rate), nv(n), nv(pv), nv(fv), ty.clone()]);
    let p = num_of(&pmt, format!("PMT({rate}, {n}, {pv}, {fv})"));
    let back = num_of(&call("PV", &[nv(rate), nv(n), pmt, nv(fv), ty]), "PV");
    // The error PV can not avoid: fv's share is discounted n periods.
    let scale = pv.abs() + fv.abs() * (1.0f64 + rate).powf(-n) + p.abs() * n;
    assert!(
        (back - pv).abs() <= 1e-9 * scale + 1e-9,
        "PV({rate}, {n}, {p}, {fv}) = {back}, want {pv}"
    );
}

/// NPV at the IRR of a conventional investment (one outflow, then inflows)
/// is zero.
#[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
fn npv_at_irr_is_zero(tc: TestCase) {
    let mut cf = vec![-float(&tc, 1.0, 1e6)];
    let len = int(&tc, 1, 12);
    cf.extend((0..len).map(|_| float(&tc, 0.0, 1e6)));
    if cf[1..].iter().all(|x| *x == 0.0) {
        return;
    }
    let rows = column(&cf.iter().map(|x| nv(*x)).collect::<Vec<_>>());
    let Some(irr) = reduce::reduce(reduction("IRR"), &rows, &[]).as_num() else {
        return; // Newton from the default guess may miss a far IRR.
    };
    if irr < -0.5 {
        return; // NPV's slope grows like 1 / (1 + irr): ill-conditioned.
    }
    let npv = num_of(&reduce::reduce(reduction("NPV"), &rows, &[nv(irr)]), "NPV");
    let scale: f64 = cf
        .iter()
        .enumerate()
        .map(|(i, v)| v.abs() / (1.0 + irr).powi(i as i32 + 1))
        .sum();
    assert!(
        npv.abs() <= 1e-7 * scale,
        "NPV at IRR {irr} of {cf:?} = {npv}"
    );
}

/// YEAR, MONTH and DAY of DATE(y, m, d) are y, m and d.
#[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
fn date_parts_round_trip(tc: TestCase) {
    let (y, m, d) = (int(&tc, 1900, 9999), int(&tc, 1, 12), int(&tc, 1, 31));
    tc.assume(chrono::NaiveDate::from_ymd_opt(y as i32, m as u32, d as u32).is_some());
    let date = call("DATE", &[nv(y as f64), nv(m as f64), nv(d as f64)]);
    assert!(
        matches!(date, CellValue::Date(_)),
        "DATE({y}, {m}, {d}) = {date:?}"
    );
    for (part, want) in [("YEAR", y), ("MONTH", m), ("DAY", d)] {
        assert_eq!(
            call(part, std::slice::from_ref(&date)),
            nv(want as f64),
            "{part} of {y}-{m}-{d}"
        );
    }
}

/// EOMONTH(date, k) is the last day of the month k months on: the next day
/// is the 1st, and the months add up.
#[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
fn eomonth_is_a_months_last_day(tc: TestCase) {
    // 1910 … 9800, so a thousand months either way stays in years 1–9999.
    let start = CellValue::Date(int(&tc, -1_893_456_000_000, 247_000_000_000_000));
    let k = int(&tc, -1000, 1000);
    let e = call("EOMONTH", &[start.clone(), nv(k as f64)]);
    let CellValue::Date(ms) = e else {
        panic!("EOMONTH({start:?}, {k}) = {e:?}");
    };
    let next = CellValue::Date(ms + DAY_MS);
    assert_eq!(call("DAY", &[next]), nv(1.0), "EOMONTH({start:?}, {k})");
    let months = |v: &CellValue| {
        num_of(&call("YEAR", std::slice::from_ref(v)), "YEAR") * 12.0
            + num_of(&call("MONTH", std::slice::from_ref(v)), "MONTH")
    };
    assert_eq!(months(&e), months(&start) + k as f64);
}

/// LEN is additive over CONCAT, and UPPER is idempotent.
#[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
fn text_lengths_and_case(tc: TestCase) {
    let (a, b) = (text(&tc), text(&tc));
    let len = |s: &CellValue| num_of(&call("LEN", std::slice::from_ref(s)), "LEN");
    let ab = call("CONCAT", &[tv(&a), tv(&b)]);
    assert_eq!(len(&ab), len(&tv(&a)) + len(&tv(&b)), "{a:?} {b:?}");
    let up = call("UPPER", &[tv(&a)]);
    assert_eq!(call("UPPER", std::slice::from_ref(&up)), up, "UPPER({a:?})");
}

/// ROUND(x, d) is within half a unit of the d-th place.
#[hegel::test(test_cases = 500, suppress_health_check = HealthCheck::all())]
fn round_is_within_half_a_unit(tc: TestCase) {
    let x = float(&tc, -1e12, 1e12);
    let d = int(&tc, -12, 12);
    let r = num_of(
        &call("ROUND", &[nv(x), nv(d as f64)]),
        format!("ROUND({x}, {d})"),
    );
    let half = 0.5 * 10f64.powi(-d as i32);
    assert!(
        (r - x).abs() <= half * (1.0 + 1e-12) + 1e-14 * x.abs(),
        "ROUND({x}, {d}) = {r}"
    );
}

/// FLOOR ≤ x ≤ CEILING, at most one significance apart (FLOOR.MATH and
/// CEILING.MATH for any x, FLOOR and CEILING for x ≥ 0).
#[hegel::test(test_cases = 500, suppress_health_check = HealthCheck::all())]
fn floor_and_ceiling_bracket(tc: TestCase) {
    let math = tc.draw(gs::booleans());
    let x = if math {
        moderate(&tc)
    } else {
        moderate(&tc).abs()
    };
    let sig = if tc.draw(gs::booleans()) {
        int(&tc, 1, 100) as f64
    } else {
        float(&tc, 1e-3, 1e3)
    };
    let (f, c) = if math {
        ("FLOOR.MATH", "CEILING.MATH")
    } else {
        ("FLOOR", "CEILING")
    };
    let lo = num_of(&call(f, &[nv(x), nv(sig)]), format!("{f}({x}, {sig})"));
    let hi = num_of(&call(c, &[nv(x), nv(sig)]), format!("{c}({x}, {sig})"));
    let fuzz = 1e-14 * x.abs().max(sig);
    assert!(
        lo <= x + fuzz && x <= hi + fuzz,
        "{lo} ≤ {x} ≤ {hi} ({sig})"
    );
    assert!(hi - lo <= sig * (1.0 + 1e-12) + fuzz, "{lo} … {hi} ({sig})");
}

/// COMBIN(n, k) = COMBIN(n, n − k), and Pascal's rule holds.
#[hegel::test(test_cases = 500, suppress_health_check = HealthCheck::all())]
fn combin_symmetry_and_pascal(tc: TestCase) {
    let n = int(&tc, 1, 1100);
    let k = int(&tc, 0, n);
    let c = |n: i64, k: i64| call("COMBIN", &[nv(n as f64), nv(k as f64)]);
    assert_eq!(c(n, k), c(n, n - k), "COMBIN({n}, {k})");
    let (Some(a), Some(b), Some(s)) = (
        c(n - 1, k - 1).as_num().or((k == 0).then_some(0.0)),
        c(n - 1, k).as_num().or((k == n).then_some(0.0)),
        c(n, k).as_num(),
    ) else {
        return; // overflow
    };
    assert!(
        (a + b - s).abs() <= 1e-12 * s,
        "COMBIN({n}, {k}) = {s}, not {a} + {b}"
    );
}

/// The documented sort order: numbers, then text (ignoring case), then
/// FALSE, TRUE.
fn sort_order(a: &CellValue, b: &CellValue) -> Ordering {
    let rank = |v: &CellValue| match v {
        CellValue::Num(_) | CellValue::Date(_) => 0,
        CellValue::Text(_) => 1,
        CellValue::Bool(_) => 2,
        CellValue::Err(_) => 3,
    };
    rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
        (CellValue::Text(x), CellValue::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (CellValue::Bool(x), CellValue::Bool(y)) => x.cmp(y),
        _ => a.as_num().partial_cmp(&b.as_num()).unwrap(),
    })
}

fn multiset(v: &[CellValue]) -> Vec<String> {
    let mut s: Vec<String> = v.iter().map(|x| format!("{x:?}")).collect();
    s.sort();
    s
}

fn plain_list(tc: &TestCase) -> Vec<CellValue> {
    let len = int(tc, 1, 12);
    (0..len).map(|_| plain(tc)).collect()
}

// ---------------------------------------------------------------------------
// Named arguments
// ---------------------------------------------------------------------------

/// A literal of kind `a` as formula text.
fn literal_text(tc: &TestCase, a: Arg) -> String {
    let pick = |v: Vec<&str>| tc.draw(gs::sampled_from(v)).to_string();
    match a {
        Arg::Number => pick(vec![
            "0", "1", "2", "2.5", "-3", "0.05", "12", "100", "1e-3",
        ]),
        Arg::Text => pick(vec!["\"\"", "\"a\"", "\"ab,c\"", "\"10\"", "\"x y\""]),
        Arg::Boolean => pick(vec!["TRUE", "FALSE"]),
        Arg::Date => pick(vec!["#2025-01-31#", "#2024-02-29#"]),
        Arg::DateLike | Arg::Any | Arg::Category => {
            let k = tc.draw(gs::sampled_from(vec![
                Arg::Number,
                Arg::Text,
                Arg::Boolean,
                Arg::Date,
            ]));
            literal_text(tc, k)
        }
    }
}

/// A named call means its positional form: `F(a, c => z, b => y)` parses
/// to `F(a, y, z)` and evaluates the same, for every scalar function with
/// a fixed parameter list, any split into positional and named arguments,
/// and any order of the named ones.
#[hegel::test(test_cases = 2000, suppress_health_check = HealthCheck::all())]
fn named_arguments_mean_their_positional_form(tc: TestCase) {
    let defs: Vec<_> = scalar_defs()
        .into_iter()
        .filter(|d| !d.variadic && !CLOCK.contains(&d.id.0))
        .collect();
    let def = draw_def(&tc, &defs);
    let n = arity(&tc, def);
    let args: Vec<String> = (0..n).map(|i| literal_text(&tc, def.param(i))).collect();
    let k = tc.draw(gs::integers::<usize>().max_value(n));
    let mut named: Vec<usize> = (k..n).collect();
    for i in (1..named.len()).rev() {
        named.swap(i, tc.draw(gs::integers::<usize>().max_value(i)));
    }
    let mut parts: Vec<String> = args[..k].to_vec();
    for &i in &named {
        let name = def.param_name(i).expect("a named parameter");
        parts.push(format!("{name} => {}", args[i]));
    }
    let model = improv_core_model::Model::new();
    let parse = |t: &str| {
        improv_core_model::parse_expr(&model, t)
            .unwrap_or_else(|e| panic!("{t}: {e}"))
            .expr
    };
    let positional = format!("{}({})", def.name, args.join(", "));
    let by_name = format!("{}({})", def.name, parts.join(", "));
    let (p, q) = (parse(&positional), parse(&by_name));
    assert_eq!(q, p, "{by_name} vs {positional}");
    let eval = |e: Expr| {
        timed(
            || by_name.clone(),
            move || scalar::eval(&e, &HashMap::new()),
        )
    };
    assert_eq!(eval(q), eval(p), "{by_name}");
}

/// The default the parameter table writes for an optional parameter
/// (`[type=0]`, what a named argument fills in when it skips one) is what
/// the function does when the parameter is left out.
#[hegel::test(test_cases = 2000, suppress_health_check = HealthCheck::all())]
fn written_defaults_are_the_omitted_behavior(tc: TestCase) {
    // R.QTUKEY (1658) integrates numerically: two calls overrun the budget
    // in a debug build.
    let defs: Vec<_> = scalar_defs()
        .into_iter()
        .filter(|d| d.id.0 != 1658)
        .filter(|d| (d.required..d.params.len()).any(|i| d.param_default(i).is_some()))
        .collect();
    let def = draw_def(&tc, &defs);
    let with: Vec<usize> = (def.required..def.params.len())
        .filter(|&i| def.param_default(i).is_some())
        .collect();
    let i = tc.draw(gs::sampled_from(with));
    let model = improv_core_model::Model::new();
    let args: Vec<Expr> = (0..i)
        .map(|j| {
            let t = literal_text(&tc, def.param(j));
            improv_core_model::parse_expr(&model, &t).expect(&t).expr
        })
        .collect();
    let mut given = args.clone();
    given.push(def.param_default(i).expect("a default"));
    let (id, name) = (def.id, def.param_name(i).unwrap_or_default());
    let what = format!("{}({args:?}) with and without {name}", def.name);
    let (left, written) = timed(
        || what.clone(),
        move || {
            let e = HashMap::new();
            let call = |a| scalar::eval(&Expr::Call(id, a), &e);
            (call(args), call(given))
        },
    );
    assert_eq!(written, left, "{what}");
}

/// The same for the reductions: a written default given as a parameter is
/// what leaving it out does (`XLOOKUP`'s `[if_not_found=NA()]`, the
/// `FORECAST.ETS` family's options).
#[hegel::test(test_cases = 1000, suppress_health_check = HealthCheck::all())]
fn written_defaults_are_the_omitted_behavior_of_reductions(tc: TestCase) {
    let skippable = |d: &FnDef, i: usize| !d.over.contains(i) && d.param_default(i).is_some();
    let defs: Vec<_> = reduction_defs()
        .into_iter()
        .filter(|d| !d.variadic && (d.required..d.params.len()).any(|i| skippable(d, i)))
        .collect();
    let def = draw_def(&tc, &defs);
    let with: Vec<usize> = (def.required..def.params.len())
        .filter(|&i| skippable(def, i))
        .collect();
    let i = tc.draw(gs::sampled_from(with));
    let cols: Vec<Arg> = (0..i)
        .filter(|&j| def.over.contains(j))
        .map(|j| def.param(j))
        .collect();
    let rows = rows_of(&tc, &cols, 8, value_of);
    let params: Vec<CellValue> = (0..i)
        .filter(|&j| !def.over.contains(j))
        .map(|j| value_of(&tc, def.param(j)))
        .collect();
    let default = scalar::eval(&def.param_default(i).expect("a default"), &HashMap::new());
    let Ok(Some(default)) = default else {
        panic!("{}: default {default:?}", def.name)
    };
    let mut given = params.clone();
    given.push(default);
    assert_eq!(
        run_reduction(def, &rows, &given),
        run_reduction(def, &rows, &params),
        "{}({rows:?}, {params:?}) with and without {:?}",
        def.name,
        def.param_name(i)
    );
}

/// SORT's output is sorted and a permutation of its input.
#[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
fn sort_is_a_sorted_permutation(tc: TestCase) {
    let vals = plain_list(&tc);
    let desc = tc.draw(gs::booleans());
    let out = reduce::array(
        reduction("SORT"),
        &column(&vals),
        &[nv(if desc { -1.0 } else { 1.0 })],
    );
    assert_eq!(multiset(&out), multiset(&vals), "SORT({vals:?})");
    for w in out.windows(2) {
        let o = sort_order(&w[0], &w[1]);
        assert!(
            o != if desc {
                Ordering::Less
            } else {
                Ordering::Greater
            },
            "SORT({vals:?}, {desc}) = {out:?}"
        );
    }
}

/// UNIQUE keeps each value once, and only values of the input (with
/// exactly_once, only those it holds once).
#[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
fn unique_has_no_duplicates(tc: TestCase) {
    let vals = plain_list(&tc);
    let once = tc.draw(gs::booleans());
    let out = reduce::array(
        reduction("UNIQUE"),
        &column(&vals),
        &[CellValue::Bool(once)],
    );
    let count = |v: &CellValue| vals.iter().filter(|w| *w == v).count();
    if out == [CellValue::Err(builtins::err::NA)] {
        assert!(
            once && vals.iter().all(|v| count(v) > 1),
            "UNIQUE({vals:?})"
        );
        return;
    }
    for (i, v) in out.iter().enumerate() {
        assert!(!out[..i].contains(v), "UNIQUE({vals:?}) = {out:?}");
        assert!(count(v) == 1 || (!once && count(v) > 1), "{v:?} in {out:?}");
    }
    if !once {
        assert!(
            vals.iter().all(|v| out.contains(v)),
            "UNIQUE({vals:?}) = {out:?}"
        );
    }
}

/// TAKE(k) and DROP(k) partition the list (the empty list is #N/A).
#[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
fn take_and_drop_partition(tc: TestCase) {
    let vals = plain_list(&tc);
    let n = vals.len() as i64;
    let k = int(&tc, -n - 2, n + 2) as f64;
    let rows = column(&vals);
    let part = |name| {
        let out = reduce::array(reduction(name), &rows, &[nv(k)]);
        if out == [CellValue::Err(builtins::err::NA)] {
            Vec::new()
        } else {
            out
        }
    };
    let (take, drop) = (part("TAKE"), part("DROP"));
    let joined = if k >= 0.0 {
        [take.clone(), drop.clone()].concat()
    } else {
        [drop.clone(), take.clone()].concat()
    };
    assert_eq!(joined, vals, "TAKE {take:?} DROP {drop:?} at {k}");
}

/// INDEX(values, MATCH(key, keys, 0)) is XLOOKUP(key, keys, values) for
/// unique keys.
#[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
fn index_match_is_xlookup(tc: TestCase) {
    let keys: Vec<i32> = tc.draw(
        gs::vecs(gs::integers::<i32>().min_value(-20).max_value(20))
            .min_size(1)
            .max_size(10)
            .unique(true),
    );
    let as_text = tc.draw(gs::booleans());
    let key = |k: i32| {
        if as_text {
            tv(&format!("k{k}"))
        } else {
            nv(f64::from(k))
        }
    };
    let mut both = rows_of(&tc, &[Arg::Any, Arg::Any], keys.len(), |tc, _| plain(tc));
    for (r, k) in both.vals.iter_mut().zip(&keys) {
        r[0] = key(*k);
    }
    let only = |c: usize| Rows {
        pos: both.pos.clone(),
        vals: both.vals.iter().map(|r| vec![r[c].clone()]).collect(),
        capacity: both.capacity,
    };
    let wanted = key(int(&tc, -22, 22) as i32);
    let x = reduce::reduce(reduction("XLOOKUP"), &both, std::slice::from_ref(&wanted));
    let at = reduce::reduce(reduction("MATCH"), &only(0), &[wanted.clone(), nv(0.0)]);
    let im = match at {
        CellValue::Num(_) => reduce::reduce(reduction("INDEX"), &only(1), &[at]),
        e => e,
    };
    assert_eq!(im, x, "{wanted:?} in {both:?}");
}
