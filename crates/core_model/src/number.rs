//! Numbers as the front ends show them: Excel's General form and a
//! measure's [`NumberFormat`] (H1-22). Here, not in the engine, so a front
//! end without the engine (the browser) shows numbers the same way.

use crate::{Locale, NumberFormat};

/// A number as people read it, in Excel's General form: at most 15
/// significant digits, the precision an `f64` always carries through
/// decimal, so binary noise like `58.650000000000006` (from `1173 * 0.05`)
/// shows as `58.65`; in exponent form (`1E+21`, `1.5E-07`) below 1E-4 or
/// from 1E+15, as C's `%.15G`. The value itself is untouched; exports (CSV,
/// SQL, Postgres) keep every bit.
pub fn display_number(n: f64) -> String {
    if !n.is_finite() || n == 0.0 {
        return if n == 0.0 { "0".into() } else { n.to_string() };
    }
    // `{:.14e}` rounds to 15 significant digits (`1.50000000000000e-7`).
    let sci = format!("{n:.14e}");
    let (mantissa, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    if (-4..15).contains(&exp) {
        // The shortest decimal for the rounded value.
        let rounded: f64 = sci.parse().unwrap_or(n);
        return rounded.to_string();
    }
    let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mantissa}E{sign}{:02}", exp.unsigned_abs())
}

/// A number as a measure's `format` shows it, in `locale` (H1-22).
///
/// No format is [`display_number`]'s General form, unchanged. A format
/// writes the locale's separators (`1.234,50` in German). Its decimals
/// round the 15 significant digits General shows, half away from zero, as
/// Excel does: 2.5 is `3` at no decimals, 1.005 is `1.01` at two (in
/// binary it is a hair under), and a result that rounds to zero has no
/// sign. Without decimals the General digits are kept and only grouped.
/// From 1E+15, and where General writes an exponent, General's digits
/// stay, ungrouped. NaN and infinities are General's spelling.
pub fn format_number(n: f64, format: Option<&NumberFormat>, locale: &Locale) -> String {
    let general = display_number(n);
    let Some(f) = format else {
        return general;
    };
    let digits = match f.decimals {
        _ if !n.is_finite() => None,
        Some(d) => fixed(n, d.into()),
        None if general.contains('E') => None,
        None => {
            let (int, frac) = general.split_once('.').unwrap_or((&general, ""));
            let (neg, int) = int.strip_prefix('-').map_or((false, int), |i| (true, i));
            Some((neg, int.to_string(), frac.to_string()))
        }
    };
    let Some((neg, int, frac)) = digits else {
        return general.replace('.', &locale.decimal.to_string());
    };
    let mut out = String::with_capacity(int.len() * 4 / 3 + frac.len() + 2);
    if neg {
        out.push('-');
    }
    for (i, c) in int.chars().enumerate() {
        if f.grouping && i > 0 && (int.len() - i) % 3 == 0 {
            out.push(locale.group());
        }
        out.push(c);
    }
    if !frac.is_empty() {
        out.push(locale.decimal);
        out.push_str(&frac);
    }
    out
}

/// `n`'s 15 significant digits rounded half away from zero to `decimals`
/// places: (negative, integer digits, `decimals` fraction digits). `None`
/// from 1E+15, where General writes an exponent.
fn fixed(n: f64, decimals: usize) -> Option<(bool, String, String)> {
    let sci = format!("{:.14e}", n.abs());
    let (mantissa, exp) = sci.split_once('e')?;
    let exp: i32 = exp.parse().ok()?;
    if exp >= 15 {
        return None;
    }
    // The value is 0.d1d2…d15 × 10^(exp + 1): `point` digits before the point.
    let mut digits: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();
    let mut point = usize::try_from(exp + 1).unwrap_or(0);
    if exp < 0 {
        digits.splice(0..0, std::iter::repeat_n(b'0', exp.unsigned_abs() as usize));
        point = 1;
    }
    let keep = point + decimals;
    if digits.len() <= keep {
        digits.resize(keep, b'0');
    } else {
        let up = digits[keep] >= b'5';
        digits.truncate(keep);
        if up {
            match digits.iter().rposition(|&d| d != b'9') {
                Some(i) => {
                    digits[i] += 1;
                    digits[i + 1..].fill(b'0');
                }
                None => {
                    digits.fill(b'0');
                    digits.insert(0, b'1');
                    point += 1;
                }
            }
        }
    }
    let neg = n.is_sign_negative() && digits.iter().any(|&d| d != b'0');
    let (int, frac) = digits.split_at(point);
    let lead = int.iter().position(|&d| d != b'0').unwrap_or(int.len() - 1);
    let text = |d: &[u8]| String::from_utf8_lossy(d).into_owned();
    Some((neg, text(&int[lead..]), text(frac)))
}
