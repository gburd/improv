//! Evaluation of the P7 engineering functions (ids 1300–1449): base
//! conversion and bit operations (Excel's rules), complex numbers as
//! `"a+bi"` text, Bessel functions and CONVERT's unit table.

use super::{domain, err, n, n_or, ok_num, ok_text, t, R};
use crate::CellValue;

/// 2⁴⁸: the bit functions' exclusive bound.
const BIT_LIMIT: f64 = 281_474_976_710_656.0;

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    let x = |i: usize| n(a, i);
    let places = |i: usize| {
        if a.len() > i {
            n(a, i).map(Some)
        } else {
            Ok(None)
        }
    };
    match id {
        // --- bases ---
        1300 => ok_num(from_base(a, 2)? as f64),
        1301 => to_base(from_base(a, 2)?, 16, places(1)?),
        1302 => to_base(from_base(a, 2)?, 8, places(1)?),
        1303 => to_base(dec(x(0)?)?, 2, places(1)?),
        1304 => to_base(dec(x(0)?)?, 16, places(1)?),
        1305 => to_base(dec(x(0)?)?, 8, places(1)?),
        1306 => to_base(from_base(a, 16)?, 2, places(1)?),
        1307 => ok_num(from_base(a, 16)? as f64),
        1308 => to_base(from_base(a, 16)?, 8, places(1)?),
        1309 => to_base(from_base(a, 8)?, 2, places(1)?),
        1310 => ok_num(from_base(a, 8)? as f64),
        1311 => to_base(from_base(a, 8)?, 16, places(1)?),
        1312 => base(x(0)?, x(1)?, n_or(a, 2, 0.0)?),
        1313 => decimal(&t(a, 0)?, x(1)?),
        1314 => ok_text(
            x(0)?
                .to_le_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
        ),
        // --- bits ---
        1315 => ok_num((bits(x(0)?)? & bits(x(1)?)?) as f64),
        1316 => ok_num((bits(x(0)?)? | bits(x(1)?)?) as f64),
        1317 => ok_num((bits(x(0)?)? ^ bits(x(1)?)?) as f64),
        1318 => shift(x(0)?, x(1)?.floor()),
        1319 => shift(x(0)?, -x(1)?.floor()),
        // --- tests ---
        1320 => ok_num(f64::from(u8::from(x(0)? == n_or(a, 1, 0.0)?))),
        1321 => ok_num(f64::from(u8::from(x(0)? >= n_or(a, 1, 0.0)?))),
        _ => Err(err::NAME),
    }
}

// --- bases ---------------------------------------------------------------

/// Argument 0 as digits in `base`: at most 10 of them; 10 digits whose
/// first is at least `base / 2` are negative (10-digit two's complement).
/// Empty text is 0.
fn from_base(a: &[CellValue], base: u32) -> Result<i64, u8> {
    if matches!(a.first(), Some(CellValue::Bool(_))) {
        return Err(err::VALUE);
    }
    let s = t(a, 0)?;
    domain(s.chars().count() <= 10)?;
    let mut v: i64 = 0;
    for c in s.chars() {
        v = v * i64::from(base) + i64::from(c.to_digit(base).ok_or(err::NUM)?);
    }
    let first = s.chars().next().and_then(|c| c.to_digit(base));
    if s.len() == 10 && first >= Some(base / 2) {
        v -= i64::from(base).pow(10);
    }
    Ok(v)
}

/// A decimal argument, truncated (its range is checked by [`to_base`]).
fn dec(x: f64) -> Result<i64, u8> {
    let x = x.trunc();
    domain(x.abs() < 1e12)?;
    Ok(x as i64)
}

/// `v` as text in `base` (2, 8 or 16), which must hold it in 10 digits of
/// two's complement. A non-negative `v` is zero-padded to `places` (1–10,
/// truncated); a negative one is always 10 digits.
fn to_base(v: i64, base: u32, places: Option<f64>) -> R {
    let full = i64::from(base).pow(10);
    domain(-full / 2 <= v && v < full / 2)?;
    let places = match places {
        Some(p) => {
            let p = p.trunc();
            domain((1.0..=10.0).contains(&p))?;
            p as usize
        }
        None => 0,
    };
    let u = if v < 0 { v + full } else { v };
    let digits = match base {
        2 => format!("{u:b}"),
        8 => format!("{u:o}"),
        _ => format!("{u:X}"),
    };
    if v < 0 {
        return ok_text(digits);
    }
    domain(digits.len() <= places || places == 0)?;
    ok_text(format!("{digits:0>places$}"))
}

/// The radix argument of BASE / DECIMAL: 2…36, truncated.
fn radix(r: f64) -> Result<u32, u8> {
    let r = r.trunc();
    domain((2.0..=36.0).contains(&r))?;
    Ok(r as u32)
}

/// BASE: a non-negative integer below 2⁵³ in `radix`, at least `min_len`
/// (0…255) digits.
fn base(x: f64, r: f64, min_len: f64) -> R {
    let (x, r, min_len) = (x.trunc(), radix(r)?, min_len.trunc());
    domain((0.0..9_007_199_254_740_992.0).contains(&x) && (0.0..=255.0).contains(&min_len))?;
    let mut u = x as u64;
    let mut digits = Vec::new();
    loop {
        digits.push(char::from_digit((u % u64::from(r)) as u32, r).unwrap_or('0'));
        u /= u64::from(r);
        if u == 0 {
            break;
        }
    }
    let s: String = digits.iter().rev().collect::<String>().to_uppercase();
    let width = min_len as usize;
    ok_text(format!("{s:0>width$}"))
}

/// DECIMAL: digits in `radix` (either case, at most 255) as a number.
fn decimal(s: &str, r: f64) -> R {
    let r = radix(r)?;
    domain(s.chars().count() <= 255)?;
    let mut v = 0.0;
    for c in s.chars() {
        v = v * f64::from(r) + f64::from(c.to_digit(r).ok_or(err::NUM)?);
    }
    ok_num(v)
}

// --- bits ----------------------------------------------------------------

/// A bit-function operand: truncated, in 0…2⁴⁸−1.
fn bits(x: f64) -> Result<u64, u8> {
    let x = x.trunc();
    domain((0.0..BIT_LIMIT).contains(&x))?;
    Ok(x as u64)
}

/// `x` shifted left by `by` (an integer) bits, right if negative, |by| ≤ 53.
/// A fractional shift is floored first, as LibreOffice does.
fn shift(x: f64, by: f64) -> R {
    let x = bits(x)?;
    domain(by.abs() <= 53.0)?;
    if by >= 0.0 {
        ok_num(x as f64 * 2f64.powi(by as i32))
    } else {
        ok_num((x >> (-by) as u32) as f64)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn call(id: u32, args: &[CellValue]) -> CellValue {
        eval(id, args).unwrap_or_else(CellValue::Err)
    }

    fn txt(s: &str) -> CellValue {
        CellValue::Text(s.into())
    }

    fn num(x: f64) -> CellValue {
        CellValue::num(x)
    }

    /// The branches the goldens do not reach.
    #[test]
    fn base_edges() {
        let e = CellValue::Err;
        assert_eq!(call(1300, &[CellValue::Bool(true)]), e(err::VALUE));
        assert_eq!(call(1300, &[txt("")]), num(0.0));
        assert_eq!(call(1300, &[num(101.0)]), num(5.0));
        assert_eq!(call(1307, &[txt("ff")]), num(255.0));
        // places: too small, too large, negative numbers ignore a valid one
        assert_eq!(call(1303, &[num(4.0), num(2.0)]), e(err::NUM));
        assert_eq!(call(1303, &[num(4.0), num(11.0)]), e(err::NUM));
        assert_eq!(call(1303, &[num(-4.0), num(3.0)]), txt("1111111100"));
        assert_eq!(call(1303, &[num(1e13)]), e(err::NUM));
        assert_eq!(call(1312, &[num(255.0), num(16.0)]), txt("FF"));
        assert_eq!(call(1312, &[num(0.0), num(2.0)]), txt("0"));
        assert_eq!(call(1312, &[num(1.0), num(2.0), num(256.0)]), e(err::NUM));
        assert_eq!(call(1313, &[txt("zz"), num(36.0)]), num(1295.0));
        assert_eq!(call(1313, &[txt("1"), num(37.0)]), e(err::NUM));
        assert_eq!(call(1313, &[txt(&"1".repeat(256)), num(2.0)]), e(err::NUM));
        assert_eq!(call(1314, &[num(1.0)]), txt("000000000000f03f"));
    }

    #[test]
    fn bit_edges() {
        let e = CellValue::Err;
        assert_eq!(call(1315, &[num(-1.0), num(1.0)]), e(err::NUM));
        assert_eq!(call(1316, &[num(1.0), num(BIT_LIMIT)]), e(err::NUM));
        assert_eq!(call(1318, &[num(1.0), num(54.0)]), e(err::NUM));
        assert_eq!(call(1318, &[num(1.0), num(53.0)]), num(2f64.powi(53)));
        assert_eq!(call(1319, &[num(1.0), num(-2.0)]), num(4.0));
        assert_eq!(call(1320, &[num(0.0)]), num(1.0));
        assert_eq!(call(1321, &[num(-1.0)]), num(0.0));
    }
}
