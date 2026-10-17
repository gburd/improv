//! Evaluation of the scalar built-in functions in
//! [`improv_core_model::functions::all`]: one value per argument in,
//! one value out. The dataflow maps this over aligned cells and the scalar
//! evaluator calls it directly, so both give bit-identical results.
//!
//! Errors are values ([`CellValue::Err`] with an `ERROR.TYPE` code): an
//! error argument propagates (the first one, by argument order) except into
//! the functions that inspect errors (`ISERROR`, …); a numeric domain error
//! is `#NUM!` and a non-finite result is `#NUM!`. Positions are 1-based and
//! count characters (Unicode scalar values), not bytes.

use crate::CellValue;
use improv_core_model::FuncId;

/// `ERROR.TYPE` codes, as stored in [`CellValue::Err`]. `0` is Improv's own
/// generic error (`#ERR`).
pub mod err {
    pub const NULL: u8 = 1;
    pub const DIV0: u8 = 2;
    pub const VALUE: u8 = 3;
    pub const REF: u8 = 4;
    pub const NAME: u8 = 5;
    pub const NUM: u8 = 6;
    pub const NA: u8 = 7;
}

pub(crate) fn e(code: u8) -> CellValue {
    CellValue::Err(code)
}

/// A number result: non-finite (overflow, NaN) becomes `#NUM!`.
pub(crate) fn num(x: f64) -> CellValue {
    if x.is_finite() {
        CellValue::num(x)
    } else {
        e(err::NUM)
    }
}

mod date;
mod financial;
pub(crate) mod statistical;

/// The ids that see error arguments instead of propagating them (`ISERROR`
/// and the other information functions that ask about a value's kind).
fn inspects_errors(id: u32) -> bool {
    matches!(id, 50 | 51 | 52 | 55) || date::inspects_errors(id)
}

/// Evaluate built-in `func` on `args` (already checked against the registry's
/// arity and types). An unknown id is `#NAME?`.
pub fn eval(func: FuncId, args: &[CellValue]) -> CellValue {
    if !inspects_errors(func.0) {
        if let Some(bad) = args.iter().find(|a| matches!(a, CellValue::Err(_))) {
            return bad.clone();
        }
    }
    match eval_ok(func.0, args) {
        Ok(v) => v,
        Err(code) => e(code),
    }
}

pub(crate) type R = Result<CellValue, u8>;

pub(crate) fn n(args: &[CellValue], i: usize) -> Result<f64, u8> {
    match args.get(i) {
        Some(CellValue::Num(b)) => Ok(f64::from_bits(*b)),
        Some(CellValue::Bool(b)) => Ok(if *b { 1.0 } else { 0.0 }),
        Some(CellValue::Date(ms)) => Ok(*ms as f64),
        _ => Err(err::VALUE),
    }
}

/// Optional numeric argument `i`, or `default`.
pub(crate) fn n_or(args: &[CellValue], i: usize, default: f64) -> Result<f64, u8> {
    if args.len() > i {
        n(args, i)
    } else {
        Ok(default)
    }
}

// Used by the date and financial families as they land.
#[allow(dead_code)]
/// Milliseconds per day.
pub(crate) const DAY_MS: i64 = 86_400_000;
// Used by the date and financial families as they land.
#[allow(dead_code)]
/// 1899-12-30T00:00Z in epoch milliseconds: spreadsheet serial date 0, so
/// serial 1 is 1899-12-31 and serial 60 the (fictitious) 1900-02-29 is
/// skipped as in LibreOffice (serial 61 = 1900-03-01 in both systems).
pub(crate) const SERIAL_EPOCH_MS: i64 = -2_209_161_600_000;

// Used by the date and financial families as they land.
#[allow(dead_code)]
/// A date argument as epoch milliseconds: a date as itself, a number as a
/// spreadsheet serial date (fractional days are the time of day).
pub(crate) fn d(args: &[CellValue], i: usize) -> Result<i64, u8> {
    match args.get(i) {
        Some(CellValue::Date(ms)) => Ok(*ms),
        Some(CellValue::Num(b)) => {
            let serial = f64::from_bits(*b);
            let ms = serial * DAY_MS as f64;
            if !ms.is_finite() || ms.abs() > 1e16 {
                return Err(err::NUM);
            }
            Ok(SERIAL_EPOCH_MS + ms.round() as i64)
        }
        _ => Err(err::VALUE),
    }
}

// Used by the date and financial families as they land.
#[allow(dead_code)]
/// Epoch milliseconds as a spreadsheet serial date.
pub(crate) fn serial(ms: i64) -> f64 {
    (ms - SERIAL_EPOCH_MS) as f64 / DAY_MS as f64
}

pub(crate) fn t(args: &[CellValue], i: usize) -> Result<String, u8> {
    match args.get(i) {
        Some(v) => text_of(v),
        None => Err(err::VALUE),
    }
}

pub(crate) fn b(args: &[CellValue], i: usize) -> Result<bool, u8> {
    match args.get(i) {
        Some(CellValue::Bool(b)) => Ok(*b),
        Some(CellValue::Num(x)) => Ok(f64::from_bits(*x) != 0.0),
        _ => Err(err::VALUE),
    }
}

/// A value as text, the way `&` and `CONCAT` join it.
pub(crate) fn text_of(v: &CellValue) -> Result<String, u8> {
    Ok(match v {
        CellValue::Text(s) => s.clone(),
        CellValue::Num(bits) => crate::display_number(f64::from_bits(*bits)),
        CellValue::Bool(true) => "TRUE".into(),
        CellValue::Bool(false) => "FALSE".into(),
        CellValue::Date(_) => v.to_string(),
        CellValue::Err(c) => return Err(*c),
    })
}

pub(crate) fn ok_num(x: f64) -> R {
    Ok(num(x))
}

pub(crate) fn ok_text(s: String) -> R {
    Ok(CellValue::Text(s))
}

pub(crate) fn domain(ok: bool) -> Result<(), u8> {
    if ok {
        Ok(())
    } else {
        Err(err::NUM)
    }
}

/// A non-negative integer argument (truncated), or `#NUM!`.
pub(crate) fn count(x: f64) -> Result<u64, u8> {
    domain((0.0..1e15).contains(&x))?;
    Ok(x.trunc() as u64)
}

pub(crate) fn nums(args: &[CellValue]) -> Result<Vec<f64>, u8> {
    (0..args.len()).map(|i| n(args, i)).collect()
}

pub(crate) fn bools(args: &[CellValue]) -> Result<Vec<bool>, u8> {
    (0..args.len()).map(|i| b(args, i)).collect()
}

/// The id ranges each family module owns (the registry's id plan).
fn eval_ok(id: u32, a: &[CellValue]) -> R {
    match id {
        56..=79 | 330..=399 | 400..=499 => date::eval(id, a),
        160..=199 | 500..=699 => statistical::eval(id, a),
        700..=899 => financial::eval(id, a),
        _ => eval_core(id, a),
    }
}

fn eval_core(id: u32, a: &[CellValue]) -> R {
    use std::f64::consts::PI;
    Ok(match id {
        // --- aggregation twins ---
        20 => num(nums(a)?.into_iter().fold(f64::INFINITY, f64::min)),
        21 => num(nums(a)?.into_iter().fold(f64::NEG_INFINITY, f64::max)),
        22 => num(nums(a)?.iter().sum()),
        23 => {
            let v = nums(a)?;
            num(v.iter().sum::<f64>() / v.len() as f64)
        }
        24 => num(nums(a)?.iter().product()),
        25 => num(nums(a)?.iter().map(|x| x * x).sum()),
        26 => CellValue::Bool(bools(a)?.iter().all(|x| *x)),
        27 => CellValue::Bool(bools(a)?.iter().any(|x| *x)),
        28 => CellValue::Bool(bools(a)?.iter().filter(|x| **x).count() % 2 == 1),
        // --- logical ---
        38 => {
            let (x, lo, hi) = (n(a, 0)?, n(a, 1)?, n(a, 2)?);
            let inclusive = if a.len() > 3 { b(a, 3)? } else { true };
            CellValue::Bool(if inclusive {
                lo <= x && x <= hi
            } else {
                lo < x && x < hi
            })
        }
        39 => CellValue::Bool(true),
        40 => CellValue::Bool(false),
        // --- information / errors ---
        50 => CellValue::Bool(matches!(a[0], CellValue::Err(_))),
        51 => CellValue::Bool(matches!(a[0], CellValue::Err(c) if c != err::NA)),
        52 => CellValue::Bool(matches!(a[0], CellValue::Err(err::NA))),
        53 => return Err(err::NA),
        54 => {
            let code = t(a, 0)?;
            let kind = improv_core_model::ValueErrorKind::from_code(&code).ok_or(err::VALUE)?;
            return Err(kind.error_type().unwrap_or(err::VALUE));
        }
        55 => match a[0] {
            CellValue::Err(c) if c > 0 => CellValue::num(f64::from(c)),
            _ => return Err(err::NA),
        },
        // --- math ---
        10 => num(n(a, 0)?.abs()),
        15 => num(-n(a, 0)?),
        100 => {
            let x = n(a, 0)?;
            num(if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            })
        }
        14 => {
            let x = n(a, 0)?;
            domain(x >= 0.0)?;
            num(x.sqrt())
        }
        101 => {
            let x = n(a, 0)?;
            domain(x >= 0.0)?;
            num((x * PI).sqrt())
        }
        102 => num(n(a, 0)?.exp()),
        103 => num(n(a, 0)?.exp_m1()),
        104 => {
            let x = n(a, 0)?;
            domain(x > 0.0)?;
            num(x.ln())
        }
        105 => {
            let x = n(a, 0)?;
            domain(x > -1.0)?;
            num(x.ln_1p())
        }
        106 => {
            let (x, base) = (n(a, 0)?, n_or(a, 1, 10.0)?);
            domain(x > 0.0 && base > 0.0)?;
            if base == 1.0 {
                return Err(err::DIV0);
            }
            num(if base == 10.0 {
                x.log10()
            } else {
                x.ln() / base.ln()
            })
        }
        107 => {
            let x = n(a, 0)?;
            domain(x > 0.0)?;
            num(x.log10())
        }
        108 => {
            let x = n(a, 0)?;
            domain(x > 0.0)?;
            num(x.log2())
        }
        109 => return power(n(a, 0)?, n(a, 1)?),
        110 => {
            let (x, d) = (n(a, 0)?, n(a, 1)?);
            if d == 0.0 {
                return Err(err::DIV0);
            }
            num(x - d * (x / d).floor())
        }
        111 => {
            let (x, d) = (n(a, 0)?, n(a, 1)?);
            if d == 0.0 {
                return Err(err::DIV0);
            }
            num((x / d).trunc())
        }
        112 => CellValue::num(PI),
        113 => num(n(a, 0)?.to_degrees()),
        114 => num(n(a, 0)?.to_radians()),
        115 => num(n(a, 0)?.sin()),
        116 => num(n(a, 0)?.cos()),
        117 => num(n(a, 0)?.tan()),
        118 => {
            let x = n(a, 0)?;
            domain((-1.0..=1.0).contains(&x))?;
            num(x.asin())
        }
        119 => {
            let x = n(a, 0)?;
            domain((-1.0..=1.0).contains(&x))?;
            num(x.acos())
        }
        120 => num(n(a, 0)?.atan()),
        121 => {
            let (x, y) = (n(a, 0)?, n(a, 1)?);
            if x == 0.0 && y == 0.0 {
                return Err(err::DIV0);
            }
            num(y.atan2(x))
        }
        122 => num(n(a, 0)?.sinh()),
        123 => num(n(a, 0)?.cosh()),
        124 => num(n(a, 0)?.tanh()),
        125 => num(n(a, 0)?.asinh()),
        126 => {
            let x = n(a, 0)?;
            domain(x >= 1.0)?;
            num(x.acosh())
        }
        127 => {
            let x = n(a, 0)?;
            domain(x > -1.0 && x < 1.0)?;
            num(x.atanh())
        }
        128 => recip(trig_arg(n(a, 0)?)?.cos())?,
        129 => recip(trig_arg(n(a, 0)?)?.sin())?,
        130 => recip(trig_arg(n(a, 0)?)?.tan())?,
        131 => recip(n(a, 0)?.cosh())?,
        132 => recip(n(a, 0)?.sinh())?,
        133 => recip(n(a, 0)?.tanh())?,
        134 => num(PI / 2.0 - n(a, 0)?.atan()),
        135 => {
            let x = n(a, 0)?;
            domain(x.abs() > 1.0)?;
            num(0.5 * ((x + 1.0) / (x - 1.0)).ln())
        }
        136 => num(sin_pi(n(a, 0)?)),
        137 => num(cos_pi(n(a, 0)?)),
        138 => {
            let x = n(a, 0)?;
            let c = cos_pi(x);
            domain(c != 0.0)?;
            num(sin_pi(x) / c)
        }
        139 => {
            let x = n(a, 0)?;
            let s = sin_pi(x);
            domain(s != 0.0)?;
            num(cos_pi(x) / s)
        }
        140 => num(nums(a)?.into_iter().fold(0.0, f64::hypot)),
        141 => num(fact(count(n(a, 0)?)?)),
        142 => {
            let x = n(a, 0)?;
            domain(x >= 0.0)?;
            let k = x.trunc() as i64;
            let mut p = 1.0f64;
            let mut i = k;
            while i > 1 {
                p *= i as f64;
                i -= 2;
            }
            num(p)
        }
        143 => {
            let (nn, k) = (count(n(a, 0)?)?, count(n(a, 1)?)?);
            domain(k <= nn)?;
            num(combin(nn, k))
        }
        144 => {
            let (nn, k) = (count(n(a, 0)?)?, count(n(a, 1)?)?);
            if nn == 0 && k == 0 {
                CellValue::num(1.0)
            } else {
                domain(nn > 0)?;
                num(combin(nn + k - 1, k))
            }
        }
        145 => {
            let (nn, k) = (count(n(a, 0)?)?, count(n(a, 1)?)?);
            domain(k <= nn)?;
            num(((nn - k + 1)..=nn).map(|i| i as f64).product())
        }
        146 => {
            let (nn, k) = (count(n(a, 0)?)?, count(n(a, 1)?)?);
            num((nn as f64).powf(k as f64))
        }
        147 => {
            let v = nums(a)?;
            let mut g = 0u64;
            for x in v {
                g = gcd(g, count(x)?);
            }
            CellValue::num(g as f64)
        }
        148 => {
            let v = nums(a)?;
            let mut l = 1f64;
            for x in v {
                let k = count(x)?;
                if k == 0 {
                    return Ok(CellValue::num(0.0));
                }
                let g = gcd(l as u64, k) as f64;
                l = l / g * k as f64;
                domain(l < 9.007_199_254_740_992e15)?;
            }
            CellValue::num(l)
        }
        149 => {
            let v = nums(a)?;
            let mut total = 0u64;
            let mut r = 1f64;
            for x in v {
                let k = count(x)?;
                for i in 1..=k {
                    total += 1;
                    r = r * total as f64 / i as f64;
                }
            }
            num(r.round())
        }
        150 => {
            let v = nums(a)?;
            num(v[1..].iter().fold(v[0], |acc, x| acc - x))
        }
        151 => {
            let v = nums(a)?;
            if v.iter().any(|x| *x < 0.0) {
                return Err(err::VALUE);
            }
            if v.contains(&0.0) {
                return Ok(CellValue::num(0.0));
            }
            num(1.0 / v.iter().map(|x| 1.0 / x).sum::<f64>())
        }
        // --- rounding ---
        11 => num(round_to(n(a, 0)?, n_or(a, 1, 0.0)?, Dir::HalfAway)),
        200 => num(round_to(n(a, 0)?, n_or(a, 1, 0.0)?, Dir::Away)),
        201 | 203 => num(round_to(n(a, 0)?, n_or(a, 1, 0.0)?, Dir::Toward)),
        202 => {
            let (x, d) = (n(a, 0)?, n(a, 1)?);
            domain(d >= 1.0)?;
            if x == 0.0 {
                CellValue::num(0.0)
            } else {
                let digits = d.trunc() - 1.0 - x.abs().log10().floor();
                num(round_to(x, digits, Dir::HalfAway))
            }
        }
        204 => num(n(a, 0)?.floor()),
        13 => num(n(a, 0)?.ceil()),
        12 => {
            let (x, sig) = (n(a, 0)?, n_or(a, 1, 1.0)?);
            floor_xl(x, sig)?
        }
        205 => {
            let (x, sig) = (n(a, 0)?, n_or(a, 1, 1.0)?);
            ceiling_xl(x, sig)?
        }
        206 => {
            let (x, sig, mode) = (n(a, 0)?, n_or(a, 1, 1.0)?, n_or(a, 2, 0.0)?);
            let sig = sig.abs();
            if sig == 0.0 {
                return Ok(CellValue::num(0.0));
            }
            num(if x < 0.0 && mode != 0.0 {
                sig15(clean(x / sig).ceil() * sig)
            } else {
                sig15(clean(x / sig).floor() * sig)
            })
        }
        207 => {
            let (x, sig, mode) = (n(a, 0)?, n_or(a, 1, 1.0)?, n_or(a, 2, 0.0)?);
            let sig = sig.abs();
            if sig == 0.0 {
                return Ok(CellValue::num(0.0));
            }
            num(if x < 0.0 && mode != 0.0 {
                sig15(clean(x / sig).floor() * sig)
            } else {
                sig15(clean(x / sig).ceil() * sig)
            })
        }
        208 => {
            let (x, sig) = (n(a, 0)?, n_or(a, 1, 1.0)?.abs());
            if sig == 0.0 {
                return Ok(CellValue::num(0.0));
            }
            num(sig15(clean(x / sig).floor() * sig))
        }
        209 | 210 => {
            let (x, sig) = (n(a, 0)?, n_or(a, 1, 1.0)?.abs());
            if sig == 0.0 {
                return Ok(CellValue::num(0.0));
            }
            num(sig15(clean(x / sig).ceil() * sig))
        }
        211 => {
            let (x, m) = (n(a, 0)?, n(a, 1)?);
            if m == 0.0 {
                return Ok(CellValue::num(0.0));
            }
            domain(x == 0.0 || (x > 0.0) == (m > 0.0))?;
            num(sig15(round_to(x / m, 0.0, Dir::HalfAway) * m))
        }
        212 => {
            let x = n(a, 0)?;
            let r = (x.abs() / 2.0).ceil() * 2.0;
            num(r.copysign(x))
        }
        213 => {
            let x = n(a, 0)?;
            let m = x.abs().ceil();
            let r = if m % 2.0 == 1.0 { m } else { m + 1.0 };
            num(if x < 0.0 { -r } else { r })
        }
        // --- text ---
        300 => {
            let c = n(a, 0)?.trunc();
            if !(1.0..=255.0).contains(&c) {
                return Err(err::VALUE);
            }
            // Codes 128–255 as Latin-1 (Windows-1252 differs only in 128–159).
            ok_text(char::from_u32(c as u32).ok_or(err::VALUE)?.to_string())?
        }
        301 => {
            let c = n(a, 0)?.trunc();
            if !(1.0..=1_114_111.0).contains(&c) {
                return Err(err::VALUE);
            }
            ok_text(char::from_u32(c as u32).ok_or(err::VALUE)?.to_string())?
        }
        302 | 303 => {
            let s = t(a, 0)?;
            let c = s.chars().next().ok_or(err::VALUE)?;
            CellValue::num(f64::from(c as u32))
        }
        304 => ok_text(t(a, 0)?.chars().filter(|c| (*c as u32) >= 32).collect())?,
        305 | 306 => {
            let mut s = String::new();
            for v in a {
                s.push_str(&text_of(v)?);
            }
            ok_text(s)?
        }
        307 => CellValue::Bool(t(a, 0)? == t(a, 1)?),
        308 | 309 => {
            let (find, text) = (t(a, 0)?, t(a, 1)?);
            let start = n_or(a, 2, 1.0)?.trunc();
            let chars: Vec<char> = text.chars().collect();
            if start < 1.0 || start > chars.len() as f64 + 1.0 {
                return Err(err::VALUE);
            }
            let from = start as usize - 1;
            let hit = if id == 308 {
                find_chars(&chars, &find.chars().collect::<Vec<_>>(), from)
            } else {
                search_chars(&chars, &find, from)
            };
            CellValue::num(hit.ok_or(err::VALUE)? as f64 + 1.0)
        }
        310 | 311 => {
            let s: Vec<char> = t(a, 0)?.chars().collect();
            let k = n_or(a, 1, 1.0)?;
            if k < 0.0 {
                return Err(err::VALUE);
            }
            let k = (k.trunc() as usize).min(s.len());
            let part = if id == 310 {
                &s[..k]
            } else {
                &s[s.len() - k..]
            };
            ok_text(part.iter().collect())?
        }
        312 => {
            let s: Vec<char> = t(a, 0)?.chars().collect();
            let (start, k) = (n(a, 1)?.trunc(), n(a, 2)?.trunc());
            if start < 1.0 || k < 0.0 {
                return Err(err::VALUE);
            }
            let from = (start as usize - 1).min(s.len());
            let to = from.saturating_add(k as usize).min(s.len());
            ok_text(s[from..to].iter().collect())?
        }
        313 => CellValue::num(t(a, 0)?.chars().count() as f64),
        314 => ok_text(t(a, 0)?.to_lowercase())?,
        315 => ok_text(t(a, 0)?.to_uppercase())?,
        316 => ok_text(proper(&t(a, 0)?))?,
        317 => {
            let s: Vec<char> = t(a, 0)?.chars().collect();
            let (start, k, new) = (n(a, 1)?.trunc(), n(a, 2)?.trunc(), t(a, 3)?);
            if start < 1.0 || k < 0.0 {
                return Err(err::VALUE);
            }
            let from = (start as usize - 1).min(s.len());
            let to = from.saturating_add(k as usize).min(s.len());
            let mut out: String = s[..from].iter().collect();
            out.push_str(&new);
            out.extend(&s[to..]);
            ok_text(out)?
        }
        318 => {
            let (s, k) = (t(a, 0)?, n(a, 1)?);
            if k < 0.0 || (s.chars().count() as f64) * k.trunc() > 32_767.0 {
                return Err(err::VALUE);
            }
            ok_text(s.repeat(k.trunc() as usize))?
        }
        319 => {
            let (s, old, new) = (t(a, 0)?, t(a, 1)?, t(a, 2)?);
            if old.is_empty() {
                return ok_text(s);
            }
            if a.len() > 3 {
                let inst = n(a, 3)?.trunc();
                if inst < 1.0 {
                    return Err(err::VALUE);
                }
                match s.match_indices(&old).nth(inst as usize - 1) {
                    Some((i, _)) => ok_text(format!("{}{new}{}", &s[..i], &s[i + old.len()..]))?,
                    None => ok_text(s)?,
                }
            } else {
                ok_text(s.replace(&old, &new))?
            }
        }
        320 => ok_text(
            t(a, 0)?
                .split(' ')
                .filter(|w| !w.is_empty())
                .collect::<Vec<_>>()
                .join(" "),
        )?,
        321 => {
            let s = t(a, 0)?;
            num(parse_value(&s).ok_or(err::VALUE)?)
        }
        322 | 323 => text_before_after(id == 322, a)?,
        324 => {
            let s: Vec<char> = t(a, 0)?.chars().collect();
            let fold = |v: &[char]| v.iter().map(|c| fold_char(*c)).collect::<Vec<_>>();
            let hay = fold(&s);
            let open = fold(&t(a, 1)?.chars().collect::<Vec<_>>());
            let close = fold(&t(a, 2)?.chars().collect::<Vec<_>>());
            let i = find_chars(&hay, &open, 0).ok_or(err::NA)? + open.len();
            let j = find_chars(&hay, &close, i).ok_or(err::NA)?;
            ok_text(s[i..j].iter().collect())?
        }
        325 => {
            // Numbers counts overlapping, case-insensitive matches.
            let hay: Vec<char> = t(a, 0)?.chars().map(fold_char).collect();
            let needle: Vec<char> = t(a, 1)?.chars().map(fold_char).collect();
            if needle.is_empty() {
                return Err(err::VALUE);
            }
            let hits = hay
                .windows(needle.len())
                .filter(|w| *w == needle.as_slice())
                .count();
            CellValue::num(hits as f64)
        }
        _ => return Err(err::NAME),
    })
}

/// `d` (epoch milliseconds) plus `days` (fractional days allowed), or
/// `#NUM!` when the result leaves the representable range.
pub(crate) fn date_plus_days(d: i64, days: f64) -> CellValue {
    let ms = days * 86_400_000.0;
    if !ms.is_finite() || ms.abs() > 1e17 {
        return e(err::NUM);
    }
    d.checked_add(ms.round() as i64)
        .filter(|v| chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*v).is_some())
        .map_or(e(err::NUM), CellValue::Date)
}

/// `x ^ y` with the spreadsheet edge cases: `0^0` is 1, `0^-1` is `#DIV/0!`,
/// a negative base with a fractional exponent is `#NUM!`.
pub(crate) fn power(x: f64, y: f64) -> R {
    if x == 0.0 && y < 0.0 {
        return Err(err::DIV0);
    }
    if x < 0.0 && y.fract() != 0.0 {
        return Err(err::NUM);
    }
    ok_num(x.powf(y))
}

/// Excel's SEC/CSC/COT argument limit: |x| < 2²⁷.
fn trig_arg(x: f64) -> Result<f64, u8> {
    domain(x.abs() < 134_217_728.0)?;
    Ok(x)
}

/// One character, case-folded (lowercased; a multi-char mapping keeps the
/// first, which leaves character positions intact).
pub(crate) fn fold_char(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn recip(d: f64) -> R {
    if d == 0.0 {
        return Err(err::DIV0);
    }
    ok_num(1.0 / d)
}

/// sin(πx), exactly 0 at integers.
fn sin_pi(x: f64) -> f64 {
    let r = x.rem_euclid(2.0);
    if r.fract() == 0.0 {
        return 0.0;
    }
    if r == 0.5 {
        return 1.0;
    }
    if r == 1.5 {
        return -1.0;
    }
    (std::f64::consts::PI * r).sin()
}

/// cos(πx), exactly 0 at half-integers.
fn cos_pi(x: f64) -> f64 {
    let r = x.rem_euclid(2.0);
    if r == 0.5 || r == 1.5 {
        return 0.0;
    }
    if r == 0.0 {
        return 1.0;
    }
    if r == 1.0 {
        return -1.0;
    }
    (std::f64::consts::PI * r).cos()
}

fn fact(k: u64) -> f64 {
    (1..=k).map(|i| i as f64).product()
}

fn combin(n: u64, k: u64) -> f64 {
    let k = k.min(n - k);
    let mut r = 1f64;
    for i in 0..k {
        r = r * (n - i) as f64 / (i + 1) as f64;
    }
    r.round()
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

#[derive(Clone, Copy)]
enum Dir {
    HalfAway,
    Away,
    Toward,
}

/// Round `x` to `digits` decimal places (negative digits round left of the
/// point). Decimal-correct for the halfway cases people type
/// (`ROUND(2.675, 2)` is 2.68, as in Excel): the scaled value is first
/// rounded to 15 significant digits, the precision an `f64` holds in decimal.
fn round_to(x: f64, digits: f64, dir: Dir) -> f64 {
    let d = digits.trunc().clamp(-308.0, 308.0) as i32;
    let scale = 10f64.powi(d.abs());
    let y = if d >= 0 { x * scale } else { x / scale };
    let y = if y.abs() < 1e15 {
        format!("{y:.14e}").parse::<f64>().unwrap_or(y)
    } else {
        y
    };
    let r = match dir {
        Dir::HalfAway => y.round(),
        Dir::Away => y.abs().ceil().copysign(y),
        Dir::Toward => y.trunc(),
    };
    sig15(if d >= 0 { r / scale } else { r * scale })
}

/// Excel `FLOOR(x, sig)`: down to a multiple of sig (`FLOOR(-2.5, 2)` is −4,
/// `FLOOR(-2.5, -2)` is −2); a positive x with a negative sig is `#NUM!`.
fn floor_xl(x: f64, sig: f64) -> R {
    if sig == 0.0 {
        return if x == 0.0 {
            Ok(CellValue::num(0.0))
        } else {
            Err(err::DIV0)
        };
    }
    if x > 0.0 && sig < 0.0 {
        return Err(err::NUM);
    }
    ok_num(sig15(clean(x / sig).floor() * sig))
}

/// Excel `CEILING(x, sig)`.
fn ceiling_xl(x: f64, sig: f64) -> R {
    if sig == 0.0 {
        return Ok(CellValue::num(0.0));
    }
    if x > 0.0 && sig < 0.0 {
        return Err(err::NUM);
    }
    ok_num(sig15(clean(x / sig).ceil() * sig))
}

/// `x` rounded to 15 significant digits: a multiple of a decimal step
/// (`3 × 0.1`) shows as the decimal people expect (0.3, not
/// 0.30000000000000004), as the spreadsheets do.
fn sig15(x: f64) -> f64 {
    if x == 0.0 || !x.is_finite() {
        return x;
    }
    format!("{x:.14e}").parse().unwrap_or(x)
}

/// Snap a quotient to the nearest integer when it is one to 15 digits, so
/// `FLOOR(0.3, 0.1)` is 0.3, not 0.2.
fn clean(q: f64) -> f64 {
    let r = q.round();
    if (q - r).abs() <= 1e-12 * r.abs().max(1.0) {
        r
    } else {
        q
    }
}

fn proper(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_letter = false;
    for c in s.chars() {
        if c.is_alphabetic() {
            if prev_letter {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
            prev_letter = true;
        } else {
            out.push(c);
            prev_letter = false;
        }
    }
    out
}

pub(crate) fn find_chars(hay: &[char], needle: &[char], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(from);
    }
    (from..=hay.len().checked_sub(needle.len())?).find(|&i| hay[i..i + needle.len()] == *needle)
}

/// `SEARCH`: case-insensitive, `?` any one character, `*` any run, `~`
/// escapes the next character.
pub(crate) fn search_chars(hay: &[char], pattern: &str, from: usize) -> Option<usize> {
    let lower: Vec<char> = hay.iter().flat_map(|c| c.to_lowercase()).collect();
    if lower.len() != hay.len() {
        // A case mapping that changes length: fall back to exact positions.
        return find_chars(hay, &pattern.chars().collect::<Vec<_>>(), from);
    }
    let pat: Vec<Pat> = {
        let mut out = Vec::new();
        let mut it = pattern.chars();
        while let Some(c) = it.next() {
            out.push(match c {
                '~' => match it.next() {
                    Some(n) => Pat::Lit(n.to_lowercase().next().unwrap_or(n)),
                    None => Pat::Lit('~'),
                },
                '?' => Pat::One,
                '*' => Pat::Many,
                c => Pat::Lit(c.to_lowercase().next().unwrap_or(c)),
            });
        }
        out
    };
    (from..=lower.len()).find(|&i| matches_at(&lower[i..], &pat))
}

#[derive(Clone, Copy)]
enum Pat {
    Lit(char),
    One,
    Many,
}

/// Whether `pat` matches a prefix of `s`.
fn matches_at(s: &[char], pat: &[Pat]) -> bool {
    // Iterative glob with backtracking to the last `*`.
    let (mut si, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    loop {
        if pi == pat.len() {
            return true;
        }
        match pat[pi] {
            Pat::Many => {
                star = Some((pi, si));
                pi += 1;
                continue;
            }
            Pat::One if si < s.len() => {
                si += 1;
                pi += 1;
                continue;
            }
            Pat::Lit(c) if si < s.len() && s[si] == c => {
                si += 1;
                pi += 1;
                continue;
            }
            _ => {}
        }
        match star {
            Some((sp, ss)) if ss < s.len() => {
                star = Some((sp, ss + 1));
                pi = sp + 1;
                si = ss + 1;
            }
            _ => return false,
        }
    }
}

/// `VALUE`: a decimal number, optionally with a leading sign, thousands
/// commas, a trailing `%`, or surrounding spaces.
fn parse_value(s: &str) -> Option<f64> {
    let s = s.trim();
    let (s, pct) = match s.strip_suffix('%') {
        Some(rest) => (rest.trim_end(), true),
        None => (s, false),
    };
    let cleaned: String = s.chars().filter(|c| *c != ',').collect();
    if cleaned.is_empty()
        || !cleaned
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
    {
        return None;
    }
    let v: f64 = cleaned.parse().ok()?;
    Some(if pct { v / 100.0 } else { v })
}

/// `TEXTBEFORE` / `TEXTAFTER(text, delim, [instance=1], [match_mode=0],
/// [match_end=0], [if_not_found])`. A negative instance counts from the end;
/// match_mode 1 is case-insensitive; match_end 1 treats the end of the text
/// as a delimiter.
fn text_before_after(before: bool, a: &[CellValue]) -> R {
    let (s, delim) = (t(a, 0)?, t(a, 1)?);
    let inst = n_or(a, 2, 1.0)?.trunc() as i64;
    let insensitive = n_or(a, 3, 0.0)? != 0.0;
    let match_end = n_or(a, 4, 0.0)? != 0.0;
    let chars: Vec<char> = s.chars().collect();
    if inst == 0 || inst.unsigned_abs() as usize > chars.len().max(1) {
        return Err(err::VALUE);
    }
    let fold = |c: char| {
        if insensitive {
            c.to_lowercase().next().unwrap_or(c)
        } else {
            c
        }
    };
    let hay: Vec<char> = chars.iter().map(|c| fold(*c)).collect();
    let needle: Vec<char> = delim.chars().map(fold).collect();
    // Every (start, end) of a delimiter occurrence, in character offsets.
    let mut hits: Vec<(usize, usize)> = Vec::new();
    if needle.is_empty() {
        hits.push((0, 0));
        hits.push((chars.len(), chars.len()));
    } else {
        let mut i = 0;
        while let Some(p) = find_chars(&hay, &needle, i) {
            hits.push((p, p + needle.len()));
            i = p + needle.len();
        }
    }
    if match_end {
        if inst > 0 {
            hits.push((chars.len(), chars.len()));
        } else {
            hits.insert(0, (0, 0));
        }
    }
    let pick = if inst > 0 {
        hits.get(inst as usize - 1)
    } else {
        hits.len()
            .checked_sub(inst.unsigned_abs() as usize)
            .and_then(|i| hits.get(i))
    };
    let Some(&(start, end)) = pick else {
        return match a.get(5) {
            Some(v) => Ok(v.clone()),
            None => Err(err::NA),
        };
    };
    let part: String = if before {
        chars[..start].iter().collect()
    } else {
        chars[end..].iter().collect()
    };
    ok_text(part)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::functions::{all, Kind};

    /// Every evaluable registry entry has an implementation (unknown ids fall
    /// through to `#NAME?`).
    #[test]
    fn every_scalar_function_is_implemented() {
        // The dimensional ones compile to their own plan nodes.
        for d in all().filter(|d| {
            d.kind == Kind::Scalar
                && d.category != improv_core_model::functions::Category::Dimensional
        }) {
            let args: Vec<CellValue> = (0..d.required.max(1).min(d.params.len().max(1)))
                .take(d.required)
                .map(|i| match d.param(i) {
                    improv_core_model::functions::Arg::Text => CellValue::Text("ab".into()),
                    improv_core_model::functions::Arg::Boolean => CellValue::Bool(true),
                    _ => CellValue::num(1.0),
                })
                .collect();
            let v = eval(d.id, &args);
            assert_ne!(
                v,
                CellValue::Err(err::NAME),
                "{} ({:?}) has no implementation",
                d.name,
                d.id
            );
        }
    }

    fn num_of(f: u32, args: &[f64]) -> CellValue {
        let a: Vec<CellValue> = args.iter().map(|x| CellValue::num(*x)).collect();
        eval(FuncId(f), &a)
    }

    #[test]
    fn errors_propagate_and_are_inspected() {
        let div0 = CellValue::Err(err::DIV0);
        assert_eq!(eval(FuncId(10), std::slice::from_ref(&div0)), div0);
        assert_eq!(
            eval(FuncId(50), std::slice::from_ref(&div0)),
            CellValue::Bool(true)
        );
        assert_eq!(
            eval(FuncId(51), &[CellValue::Err(err::NA)]),
            CellValue::Bool(false)
        );
        assert_eq!(
            eval(FuncId(52), &[CellValue::Err(err::NA)]),
            CellValue::Bool(true)
        );
        assert_eq!(
            eval(FuncId(55), std::slice::from_ref(&div0)),
            CellValue::num(2.0)
        );
        assert_eq!(
            eval(FuncId(55), &[CellValue::num(1.0)]),
            CellValue::Err(err::NA)
        );
        assert_eq!(eval(FuncId(53), &[]), CellValue::Err(err::NA));
        assert_eq!(
            eval(FuncId(54), &[CellValue::Text("#num!".into())]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(num_of(14, &[-1.0]), CellValue::Err(err::NUM));
        assert_eq!(num_of(102, &[1000.0]), CellValue::Err(err::NUM), "overflow");
        assert_eq!(eval(FuncId(999), &[]), CellValue::Err(err::NAME));
    }

    #[test]
    fn rounding_is_decimal_correct() {
        assert_eq!(num_of(11, &[2.5]), CellValue::num(3.0));
        assert_eq!(num_of(11, &[-2.5]), CellValue::num(-3.0));
        assert_eq!(num_of(11, &[2.675, 2.0]), CellValue::num(2.68));
        assert_eq!(num_of(11, &[1234.5, -2.0]), CellValue::num(1200.0));
        assert_eq!(num_of(200, &[-3.125, 1.0]), CellValue::num(-3.2));
        assert_eq!(num_of(201, &[3.999, 2.0]), CellValue::num(3.99));
        assert_eq!(num_of(12, &[0.3, 0.1]), CellValue::num(0.3));
        assert_eq!(num_of(12, &[-2.5, 2.0]), CellValue::num(-4.0));
        assert_eq!(num_of(12, &[2.5, -2.0]), CellValue::Err(err::NUM));
        assert_eq!(num_of(205, &[2.5, 1.0]), CellValue::num(3.0));
        assert_eq!(num_of(205, &[-2.5, -2.0]), CellValue::num(-4.0));
        assert_eq!(num_of(212, &[-1.0]), CellValue::num(-2.0));
        assert_eq!(num_of(213, &[2.0]), CellValue::num(3.0));
        assert_eq!(num_of(213, &[0.0]), CellValue::num(1.0));
        assert_eq!(num_of(211, &[10.0, 3.0]), CellValue::num(9.0));
        assert_eq!(num_of(202, &[123.456, 2.0]), CellValue::num(120.0));
    }

    /// The edge cases of each family: one line per branch the goldens do not
    /// already reach (Excel's documented results).
    #[test]
    fn edge_cases() {
        let n = CellValue::num;
        let s = |x: &str| CellValue::Text(x.into());
        let b = CellValue::Bool;
        let e = CellValue::Err;
        let call = |f: u32, a: &[CellValue]| eval(FuncId(f), a);
        // Booleans and dates read as numbers; errors propagate first-come.
        assert_eq!(call(10, &[b(true)]), n(1.0));
        assert_eq!(call(10, &[CellValue::Date(5)]), n(5.0));
        assert_eq!(call(22, &[n(1.0), e(err::NA), e(err::DIV0)]), e(err::NA));
        assert_eq!(call(10, &[s("x")]), e(err::VALUE));
        // BETWEEN inclusive and strict; ISERR on a non-error.
        assert_eq!(call(38, &[n(1.0), n(1.0), n(2.0)]), b(true));
        assert_eq!(call(38, &[n(1.0), n(1.0), n(2.0), b(false)]), b(false));
        assert_eq!(call(38, &[n(1.5), n(1.0), n(2.0), b(false)]), b(true));
        assert_eq!(call(51, &[n(1.0)]), b(false));
        assert_eq!(call(55, &[e(0)]), e(err::NA));
        assert_eq!(call(54, &[s("#bogus")]), e(err::VALUE));
        // Logs, MOD/QUOTIENT, ATAN2 and the poles.
        assert_eq!(call(106, &[n(8.0), n(1.0)]), e(err::DIV0));
        assert_eq!(call(106, &[n(8.0), n(2.0)]), n(3.0));
        assert_eq!(call(110, &[n(5.0), n(0.0)]), e(err::DIV0));
        assert_eq!(call(110, &[n(-5.0), n(3.0)]), n(1.0));
        assert_eq!(call(111, &[n(5.0), n(0.0)]), e(err::DIV0));
        assert_eq!(call(121, &[n(0.0), n(0.0)]), e(err::DIV0));
        assert_eq!(call(130, &[n(0.0)]), e(err::DIV0));
        assert_eq!(call(128, &[n(2e8)]), e(err::NUM));
        assert_eq!(call(138, &[n(0.5)]), e(err::NUM));
        assert_eq!(call(139, &[n(0.0)]), e(err::NUM));
        assert_eq!(call(136, &[n(0.5)]), n(1.0));
        assert_eq!(call(136, &[n(1.5)]), n(-1.0));
        assert_eq!(call(136, &[n(2.0)]), n(0.0));
        assert_eq!(call(137, &[n(1.0)]), n(-1.0));
        assert_eq!(call(137, &[n(2.0)]), n(1.0));
        assert_eq!(call(137, &[n(1.5)]), n(0.0));
        // Combinatorics.
        assert_eq!(call(142, &[n(1.0)]), n(1.0));
        assert_eq!(call(142, &[n(-1.0)]), e(err::NUM));
        assert_eq!(call(143, &[n(2.0), n(3.0)]), e(err::NUM));
        assert_eq!(call(144, &[n(0.0), n(0.0)]), n(1.0));
        assert_eq!(call(144, &[n(0.0), n(2.0)]), e(err::NUM));
        assert_eq!(call(145, &[n(2.0), n(3.0)]), e(err::NUM));
        assert_eq!(call(148, &[n(4.0), n(0.0)]), n(0.0));
        assert_eq!(call(148, &[n(4.0), n(6.0)]), n(12.0));
        assert_eq!(call(141, &[n(-1.0)]), e(err::NUM));
        assert_eq!(call(151, &[n(2.0), n(0.0)]), n(0.0));
        assert_eq!(call(151, &[n(-2.0)]), e(err::VALUE));
        // Rounding: zero, zero significance, modes, MROUND signs.
        assert_eq!(call(202, &[n(0.0), n(3.0)]), n(0.0));
        assert_eq!(call(202, &[n(5.0), n(0.0)]), e(err::NUM));
        assert_eq!(call(12, &[n(0.0), n(0.0)]), n(0.0));
        assert_eq!(call(12, &[n(3.0), n(0.0)]), e(err::DIV0));
        assert_eq!(call(205, &[n(3.0), n(0.0)]), n(0.0));
        assert_eq!(call(205, &[n(3.0), n(-1.0)]), e(err::NUM));
        assert_eq!(call(206, &[n(-5.5), n(2.0), n(1.0)]), n(-4.0));
        assert_eq!(call(206, &[n(5.0), n(0.0)]), n(0.0));
        assert_eq!(call(207, &[n(-5.5), n(2.0), n(1.0)]), n(-6.0));
        assert_eq!(call(207, &[n(5.0), n(0.0)]), n(0.0));
        assert_eq!(call(208, &[n(5.0), n(0.0)]), n(0.0));
        assert_eq!(call(209, &[n(5.0), n(0.0)]), n(0.0));
        assert_eq!(call(211, &[n(5.0), n(0.0)]), n(0.0));
        assert_eq!(call(211, &[n(5.0), n(-2.0)]), e(err::NUM));
        assert_eq!(call(211, &[n(-5.0), n(-2.0)]), n(-6.0));
        assert_eq!(call(213, &[n(-2.0)]), n(-3.0));
        assert_eq!(call(213, &[n(3.0)]), n(3.0));
        // Text out of range.
        assert_eq!(call(300, &[n(0.0)]), e(err::VALUE));
        assert_eq!(call(300, &[n(65.0)]), s("A"));
        assert_eq!(call(301, &[n(0.0)]), e(err::VALUE));
        assert_eq!(
            call(301, &[n(55_296.0)]),
            e(err::VALUE),
            "a surrogate is no char"
        );
        assert_eq!(call(302, &[s("")]), e(err::VALUE));
        assert_eq!(call(308, &[s("a"), s("abc"), n(0.0)]), e(err::VALUE));
        assert_eq!(call(308, &[s("a"), s("abc"), n(5.0)]), e(err::VALUE));
        assert_eq!(call(308, &[s(""), s("abc"), n(2.0)]), n(2.0));
        assert_eq!(call(308, &[s("abcd"), s("abc")]), e(err::VALUE));
        assert_eq!(call(310, &[s("abc"), n(-1.0)]), e(err::VALUE));
        assert_eq!(call(311, &[s("abc"), n(9.0)]), s("abc"));
        assert_eq!(call(312, &[s("abc"), n(0.0), n(1.0)]), e(err::VALUE));
        assert_eq!(call(312, &[s("abc"), n(9.0), n(1.0)]), s(""));
        assert_eq!(
            call(317, &[s("abc"), n(0.0), n(1.0), s("x")]),
            e(err::VALUE)
        );
        assert_eq!(call(317, &[s("abc"), n(2.0), n(1.0), s("XY")]), s("aXYc"));
        assert_eq!(call(318, &[s("ab"), n(-1.0)]), e(err::VALUE));
        assert_eq!(call(318, &[s("ab"), n(20_000.0)]), e(err::VALUE));
        assert_eq!(call(319, &[s("abc"), s(""), s("x")]), s("abc"));
        assert_eq!(
            call(319, &[s("abc"), s("b"), s("x"), n(0.0)]),
            e(err::VALUE)
        );
        assert_eq!(call(319, &[s("abc"), s("b"), s("x"), n(5.0)]), s("abc"));
        assert_eq!(call(321, &[s("")]), e(err::VALUE));
        assert_eq!(call(321, &[s("1,234")]), n(1234.0));
        assert_eq!(call(324, &[s("abc"), s("["), s("]")]), e(err::NA));
        assert_eq!(call(325, &[s("abc"), s("")]), e(err::VALUE));
        // SEARCH: a case mapping that changes length; `~` escapes; `*`.
        assert_eq!(call(309, &[s("x"), s("İx")]), n(2.0));
        assert_eq!(call(309, &[s("~*"), s("a*b")]), n(2.0));
        assert_eq!(call(309, &[s("~"), s("a~b")]), n(2.0));
        assert_eq!(call(309, &[s("a*c"), s("xabbc")]), n(2.0));
        assert_eq!(call(309, &[s("a*z"), s("abc")]), e(err::VALUE));
        // TEXTBEFORE/AFTER: bad instance, empty delimiter, case, match_end,
        // a fallback value.
        assert_eq!(call(322, &[s("a-b"), s("-"), n(0.0)]), e(err::VALUE));
        assert_eq!(call(322, &[s("a-b"), s(""), n(1.0)]), s(""));
        assert_eq!(call(323, &[s("aXb"), s("x"), n(1.0), n(1.0)]), s("b"));
        assert_eq!(
            call(323, &[s("a-b"), s("-"), n(2.0), n(0.0), n(1.0)]),
            s("")
        );
        assert_eq!(
            call(322, &[s("a-b"), s("-"), n(-2.0), n(0.0), n(1.0)]),
            s("")
        );
        assert_eq!(
            call(322, &[s("ab"), s("-"), n(1.0), n(0.0), n(0.0), s("none")]),
            s("none")
        );
        assert_eq!(call(322, &[s("a-b-c"), s("-"), n(-5.0)]), e(err::NA));
        assert_eq!(call(322, &[s("a-b"), s("-"), n(-9.0)]), e(err::VALUE));
        // POWER edge cases.
        assert_eq!(power(0.0, -1.0), Err(err::DIV0));
        assert_eq!(power(-8.0, 1.0 / 3.0), Err(err::NUM));
        assert_eq!(power(-2.0, 3.0), Ok(n(-8.0)));
    }

    /// Serial dates: 1 is 1899-12-31, 61 is 1900-03-01 (the fictitious
    /// 1900-02-29 is skipped, as in LibreOffice), and a date adds days.
    #[test]
    fn serial_dates_and_date_arithmetic() {
        let ms = |y, m, dd| {
            chrono::NaiveDate::from_ymd_opt(y, m, dd)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp_millis()
        };
        assert_eq!(d(&[CellValue::num(1.0)], 0), Ok(ms(1899, 12, 31)));
        assert_eq!(d(&[CellValue::num(61.0)], 0), Ok(ms(1900, 3, 1)));
        assert_eq!(d(&[CellValue::num(45658.0)], 0), Ok(ms(2025, 1, 1)));
        assert_eq!(
            d(&[CellValue::num(0.5)], 0),
            Ok(ms(1899, 12, 30) + DAY_MS / 2)
        );
        assert_eq!(d(&[CellValue::Date(7)], 0), Ok(7));
        assert_eq!(d(&[CellValue::Text("x".into())], 0), Err(err::VALUE));
        assert_eq!(d(&[CellValue::num(1e300)], 0), Err(err::NUM));
        assert_eq!(serial(ms(2025, 1, 1)), 45658.0);
        assert_eq!(
            date_plus_days(ms(2025, 1, 31), 1.0),
            CellValue::Date(ms(2025, 2, 1))
        );
        assert_eq!(date_plus_days(0, 1e30), CellValue::Err(err::NUM));
        assert_eq!(date_plus_days(i64::MAX - 1, 1.0), CellValue::Err(err::NUM));
    }

    #[test]
    fn text_positions_are_characters() {
        let s = |x: &str| CellValue::Text(x.into());
        assert_eq!(eval(FuncId(313), &[s("héllo")]), CellValue::num(5.0));
        assert_eq!(
            eval(
                FuncId(312),
                &[s("héllo"), CellValue::num(2.0), CellValue::num(2.0)]
            ),
            s("él")
        );
        assert_eq!(
            eval(FuncId(309), &[s("A*"), s("xxaBc"),]),
            CellValue::num(3.0)
        );
        assert_eq!(
            eval(FuncId(309), &[s("b?d"), s("abcd")]),
            CellValue::num(2.0)
        );
        assert_eq!(
            eval(FuncId(308), &[s("B"), s("abc")]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(
            eval(FuncId(322), &[s("a-b-c"), s("-"), CellValue::num(-1.0)]),
            s("a-b")
        );
        assert_eq!(eval(FuncId(323), &[s("a-b-c"), s("-")]), s("b-c"));
        assert_eq!(
            eval(FuncId(323), &[s("abc"), s("-")]),
            CellValue::Err(err::NA)
        );
        assert_eq!(
            eval(
                FuncId(319),
                &[s("aaa"), s("a"), s("b"), CellValue::num(2.0)]
            ),
            s("aba")
        );
        assert_eq!(eval(FuncId(320), &[s("  a   b ")]), s("a b"));
        assert_eq!(eval(FuncId(316), &[s("hello wORLD")]), s("Hello World"));
        assert_eq!(eval(FuncId(321), &[s(" 12.5% ")]), CellValue::num(0.125));
        assert_eq!(
            eval(
                FuncId(305),
                &[s("a"), CellValue::num(1.5), CellValue::Bool(true)]
            ),
            s("a1.5TRUE")
        );
    }
}
