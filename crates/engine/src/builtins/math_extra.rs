//! Evaluation of the P7 math extras (ids 1450–1549): special functions,
//! number theory, floating point, calendars, telephone traffic and text.

mod calendar;
mod hebrew;
mod number;
mod special;
mod width;

use super::{b, d, domain, err, n, ok_num, ok_text, t, DAY_MS, R};
use crate::CellValue;

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    let int = |i: usize| n(a, i).map(f64::floor);
    match id {
        // --- special functions ---
        1450 => ok_num(special::lambert_w(
            n(a, 0)?,
            if a.len() > 1 { n(a, 1)? } else { 0.0 },
        )?),
        1451 => ok_num(agm(n(a, 0)?, n(a, 1)?)),
        1452 => ok_num(2.0 * (n(a, 0)? / 2.0).tanh().atan()),
        1453 => ok_num(ilog(n(a, 0)?, if a.len() > 1 { n(a, 1)? } else { 10.0 })?),
        1454 => {
            let e = n(a, 1)?.trunc();
            if !(-1.0..=7.0).contains(&e) {
                return Err(err::VALUE);
            }
            let (xr, j) = special::reduce_pi(n(a, 0)?, e as i32)?;
            let q = a.len() > 2 && b(a, 2)?;
            ok_num(if q { j as f64 } else { xr })
        }
        1455 => ok_num(fib(int(0)?)?),
        // --- number theory ---
        1460..=1470 => number::eval(id, int(0)?),
        // --- floating point ---
        1475 => ok_num(f64::MAX),
        1476 => ok_num(f64::MIN_POSITIVE),
        1477 => ok_num(2.0),
        1478 => {
            let x = n(a, 0)?;
            let y = match &a[1] {
                CellValue::Text(s) if s == "+" => f64::INFINITY,
                CellValue::Text(s) if s == "-" => f64::NEG_INFINITY,
                CellValue::Text(_) => return Err(err::VALUE),
                _ => n(a, 1)?,
            };
            ok_num(next_after(x, y))
        }
        // --- calendars ---
        1490..=1494 => calendar::easter(n(a, 0)?, [0, 39, -46, -2, 49][(id - 1490) as usize]),
        1495 => ok_num((d(a, 0)?.div_euclid(DAY_MS) + calendar::UNIX_JDN) as f64),
        1496 => ok_num(calendar::jdn(n(a, 0)?, n(a, 1)?, n(a, 2)?)? as f64),
        1497..=1501 => {
            let h = hebrew::hdate(n(a, 0)?, n(a, 1)?, n(a, 2)?)?;
            Ok(match id {
                1497 => CellValue::Text(hebrew::text(h)),
                1498 => CellValue::Text(hebrew::text_heb(h)),
                1499 => CellValue::num(h.0 as f64),
                1500 => CellValue::num(h.1 as f64),
                _ => CellValue::num(h.2 as f64),
            })
        }
        1502 | 1503 => {
            let h = hebrew::hdate_of_jdn(d(a, 0)?.div_euclid(DAY_MS) + calendar::UNIX_JDN)?;
            ok_text(if id == 1502 {
                hebrew::text(h)
            } else {
                hebrew::text_heb(h)
            })
        }
        // --- telephone traffic ---
        1520 => ok_num(special::probblock(n(a, 0)?, n(a, 1)?)?),
        1521 => ok_num(special::offtraf(n(a, 0)?, n(a, 1)?)?),
        1522 => ok_num(special::dimcirc(n(a, 0)?, n(a, 1)?)?),
        1523 => ok_num(special::offcap(n(a, 0)?, n(a, 1)?)?),
        // --- text ---
        1480 => ok_text(asc(&t(a, 0)?)),
        1481 => ok_text(jis(&t(a, 0)?)),
        1482 => ok_text(baht_text(n(a, 0)?)?),
        1483 => ok_text(rot13(&t(a, 0)?)),
        1484 => encode_url(&t(a, 0)?),
        1485 => ok_text(roman(n(a, 0)?, roman_form(a)?)?),
        1486 => arabic(&t(a, 0)?),
        _ => Err(err::NAME),
    }
}

/// LibreOffice's `approxFloor`: the floor after rounding to 15 significant
/// digits, so `0.29*100` (28.999…) floors to 29.
fn approx_floor(x: f64) -> f64 {
    format!("{x:.14e}").parse::<f64>().unwrap_or(x).floor()
}

// --- special functions ------------------------------------------------------------

/// Gnumeric's `gnm_agm`: at most 20 steps (quadratic convergence), with
/// a power-of-two rescale when a·b under- or overflows.
fn agm(mut a: f64, mut b: f64) -> f64 {
    if a < 0.0 || b < 0.0 || (a * b).is_nan() {
        return f64::NAN;
    }
    if a == b || a == 0.0 || b == 0.0 {
        return a.min(b);
    }
    let mut scale = 1.0;
    let ab = a * b;
    if ab == 0.0 || ab.is_infinite() {
        let (ea, eb) = (libm::frexp(a).1, libm::frexp(b).1);
        scale = libm::ldexp(1.0, -(ea + eb) / 2);
        a *= scale;
        b *= scale;
    }
    for _ in 1..20 {
        (a, b) = ((a + b) / 2.0, (a * b).sqrt());
        if (a - b).abs() < a * f64::EPSILON {
            break;
        }
    }
    a / scale
}

/// The largest k with baseᵏ ≤ x. Exact for a power-of-two base (from the
/// exponent) and for 1 ≤ x < 2¹²⁷ (integer powers); elsewhere from ln x/ln b
/// corrected by one step either way.
fn ilog(x: f64, base: f64) -> Result<f64, u8> {
    // Gnumeric accepts only integer bases ≥ 2 (others are "not implemented").
    domain(x > 0.0 && x.is_finite() && base >= 2.0 && base.fract() == 0.0 && base.is_finite())?;
    let (m, e) = libm::frexp(base);
    if m == 0.5 {
        // base = 2^(e−1): floor(floor(log₂ x)/(e−1)).
        let l2 = libm::ilogb(x);
        return Ok(f64::from(l2.div_euclid(e - 1)));
    }
    if (1.0..1.7e38).contains(&x) {
        let (xi, bi) = (x as u128, base as u128);
        let (mut k, mut p) = (0.0, bi);
        while p <= xi {
            k += 1.0;
            match p.checked_mul(bi) {
                Some(q) => p = q,
                None => break,
            }
        }
        return Ok(k);
    }
    // ponytail: f64 check, wrong only within an ulp of an inexact power.
    let k = (x.ln() / base.ln()).floor();
    Ok(if base.powf(k + 1.0) <= x {
        k + 1.0
    } else if base.powf(k) > x {
        k - 1.0
    } else {
        k
    })
}

/// F(n) for n ≥ 1 by addition: exact to F(78), then within about n ulps.
fn fib(n: f64) -> Result<f64, u8> {
    domain((1.0..=1476.0).contains(&n))?;
    let (mut a, mut b) = (0.0f64, 1.0f64);
    for _ in 1..n as u32 {
        (a, b) = (b, a + b);
    }
    Ok(b)
}

/// C's `nextafter`.
fn next_after(x: f64, y: f64) -> f64 {
    if x.is_nan() || y.is_nan() {
        f64::NAN
    } else if y > x {
        x.next_up()
    } else if y < x {
        x.next_down()
    } else {
        y
    }
}

// --- width folding (ASC, JIS) ------------------------------------------------

/// `c` through a one-to-one `(from, to)` table sorted by `from`.
fn fold(table: &[(u16, u16)], c: char) -> char {
    table
        .binary_search_by_key(&(c as u32), |e| u32::from(e.0))
        .ok()
        .and_then(|i| char::from_u32(table[i].1.into()))
        .unwrap_or(c)
}

fn ch(u: u16) -> char {
    char::from_u32(u.into()).unwrap_or('\u{FFFD}')
}

/// Full-width to half-width: a voiced katakana splits into its letter and a
/// sound mark first (ガ → カ゛ → ｶﾞ), as LibreOffice's ASC does.
fn asc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match width::DECOMPOSE.binary_search_by_key(&(c as u32), |e| u32::from(e.0)) {
            Ok(i) => {
                let (_, letter, mark) = width::DECOMPOSE[i];
                out.push(fold(width::FULL_TO_HALF, ch(letter)));
                out.push(fold(width::FULL_TO_HALF, ch(mark)));
            }
            Err(_) => out.push(fold(width::FULL_TO_HALF, c)),
        }
    }
    out
}

/// Half-width to full-width, then a kana and its sound mark join (ｶﾞ → ガ),
/// except ウ (LibreOffice's JIS keeps ウ゛ apart).
fn jis(s: &str) -> String {
    let full: Vec<char> = s.chars().map(|c| fold(width::HALF_TO_FULL, c)).collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < full.len() {
        let c = full[i];
        let mark = match full.get(i + 1).map(|&m| m as u32) {
            Some(0x3099 | 0x309B) => Some(0),
            Some(0x309A | 0x309C) => Some(1),
            _ => None,
        };
        let joined = mark.filter(|_| c != '\u{30A6}').and_then(|j| {
            width::COMPOSE
                .binary_search_by_key(&(c as u32, j), |e| (u32::from(e.0), e.1))
                .ok()
        });
        match joined {
            Some(k) => {
                out.push(ch(width::COMPOSE[k].2));
                i += 2;
            }
            None => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

// --- BAHTTEXT ------------------------------------------------------------------

const TH_DIGIT: [&str; 10] = [
    "ศูนย์",
    "หนึ่ง",
    "สอง",
    "สาม",
    "สี่",
    "ห้า",
    "หก",
    "เจ็ด",
    "แปด",
    "เก้า",
];

/// LibreOffice's `ScBahtText`: baht in blocks of a million, then satang.
fn baht_text(x: f64) -> Result<String, u8> {
    let total = approx_floor(x.abs() * 100.0 + 0.5);
    // ponytail: integer satang, so beyond ~9.2e16 baht is #NUM! (LibreOffice
    // carries on in doubles, whose digits there are already noise).
    domain(total < 9.2e18)?;
    let total = total as u64;
    let (mut baht, satang) = (total / 100, total % 100);
    let mut out = String::new();
    if x < 0.0 {
        out.push_str("ลบ");
    }
    if baht == 0 && satang == 0 {
        out.push_str(TH_DIGIT[0]);
    }
    let mut blocks = Vec::new();
    while baht > 0 {
        blocks.push(baht % 1_000_000);
        baht /= 1_000_000;
    }
    for (k, &b) in blocks.iter().enumerate().rev() {
        if b > 0 {
            thai_block(&mut out, b);
        }
        if k > 0 {
            out.push_str("ล้าน");
        }
    }
    if !blocks.is_empty() || satang == 0 {
        out.push_str("บาท");
    }
    if satang == 0 {
        out.push_str("ถ้วน");
    } else {
        thai_block(&mut out, satang);
        out.push_str("สตางค์");
    }
    Ok(out)
}

/// 1 to 999 999 in Thai words (21 is ยี่สิบเอ็ด: "two-ten-one", 1 after a
/// ten is เอ็ด).
fn thai_block(out: &mut String, mut v: u64) {
    for (p, name) in [
        (100_000, "แสน"),
        (10_000, "หมื่น"),
        (1000, "พัน"),
        (100, "ร้อย"),
    ] {
        if v >= p {
            out.push_str(TH_DIGIT[(v / p) as usize]);
            out.push_str(name);
            v %= p;
        }
    }
    let (ten, one) = (v / 10, v % 10);
    if ten >= 3 {
        out.push_str(TH_DIGIT[ten as usize]);
    } else if ten == 2 {
        out.push_str("ยี่");
    }
    if ten > 0 {
        out.push_str("สิบ");
    }
    if ten > 0 && one == 1 {
        out.push_str("เอ็ด");
    } else if one > 0 {
        out.push_str(TH_DIGIT[one as usize]);
    }
}

// --- ROT13, ENCODEURL ------------------------------------------------------------

fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='m' | 'A'..='M' => (c as u8 + 13) as char,
            'n'..='z' | 'N'..='Z' => (c as u8 - 13) as char,
            _ => c,
        })
        .collect()
}

/// RFC 3986 percent-encoding of the UTF-8 bytes, keeping the unreserved
/// characters (Excel's documented result keeps `.`; LibreOffice also encodes
/// `.` and `~`).
fn encode_url(s: &str) -> R {
    if s.is_empty() {
        return Err(err::VALUE);
    }
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    ok_text(out)
}

// --- ROMAN, ARABIC -----------------------------------------------------------------

const ROMAN_CHARS: [char; 7] = ['M', 'D', 'C', 'L', 'X', 'V', 'I'];
const ROMAN_VALUES: [u32; 7] = [1000, 500, 100, 50, 10, 5, 1];

/// ROMAN's form: a number, or Excel's TRUE (classic, 0) / FALSE (concise, 4).
fn roman_form(a: &[CellValue]) -> Result<f64, u8> {
    match a.get(1) {
        None => Ok(0.0),
        Some(CellValue::Bool(b)) => Ok(if *b { 0.0 } else { 4.0 }),
        Some(_) => n(a, 1),
    }
}

/// LibreOffice's `ScRoman`: each power of ten, where a 4 or 9 digit
/// subtracts; each form step lets the subtracted numeral reach one place
/// further (499: CDXCIX, LDVLIV, XDIX, VDIV, ID).
fn roman(x: f64, form: f64) -> Result<String, u8> {
    let (x, form) = (approx_floor(x), approx_floor(form));
    if !(0.0..4000.0).contains(&x) || !(0.0..5.0).contains(&form) {
        return Err(err::VALUE);
    }
    let (mut val, mode) = (x as u32, form as u32);
    let mut out = String::new();
    for i in 0..4 {
        let mut idx = 2 * i;
        let digit = val / ROMAN_VALUES[idx];
        if digit % 5 == 4 {
            // 4 subtracts from the five above, 9 from the ten above.
            let big = if digit == 4 { idx - 1 } else { idx - 2 };
            let mut steps = 0;
            while steps < mode && idx < 6 {
                steps += 1;
                if ROMAN_VALUES[big] - ROMAN_VALUES[idx + 1] <= val {
                    idx += 1;
                } else {
                    steps = mode;
                }
            }
            out.push(ROMAN_CHARS[idx]);
            out.push(ROMAN_CHARS[big]);
            val = val + ROMAN_VALUES[idx] - ROMAN_VALUES[big];
        } else {
            if digit > 4 {
                out.push(ROMAN_CHARS[idx - 1]);
            }
            out.extend(std::iter::repeat_n(ROMAN_CHARS[idx], (digit % 5) as usize));
            val %= ROMAN_VALUES[idx];
        }
    }
    Ok(out)
}

/// LibreOffice's `ScArabic` validation (which numerals may follow which, at
/// most 3999), plus Excel's documented leading `-` and trimmed spaces.
fn arabic(s: &str) -> R {
    let s = s.trim_matches(' ');
    let (neg, s) = s.strip_prefix('-').map_or((false, s), |r| (true, r));
    let digits: Vec<(u32, bool)> = s
        .chars()
        .map(|c| match c.to_ascii_uppercase() {
            'M' => Some((1000, true)),
            'D' => Some((500, false)),
            'C' => Some((100, true)),
            'L' => Some((50, false)),
            'X' => Some((10, true)),
            'V' => Some((5, false)),
            'I' => Some((1, true)),
            _ => None,
        })
        .collect::<Option<_>>()
        .ok_or(err::VALUE)?;
    let (mut value, mut rest, mut i) = (0u32, 3999u32, 0);
    while i < digits.len() {
        let (d1, decimal) = digits[i];
        let d2 = digits.get(i + 1).map_or(0, |d| d.0);
        if d1 >= d2 {
            value += d1;
            rest %= d1 * if decimal { 5 } else { 2 };
            if rest < d1 {
                return Err(err::VALUE);
            }
            rest -= d1;
            i += 1;
        } else if d1 * 2 != d2 {
            let diff = d2 - d1;
            if rest < diff {
                return Err(err::VALUE);
            }
            value += diff;
            rest = d1 - 1;
            i += 2;
        } else {
            // VX, LC, DM: a five-numeral cannot subtract.
            return Err(err::VALUE);
        }
    }
    let v = f64::from(value);
    ok_num(if neg { 0.0 - v } else { v })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn text(id: u32, a: &[CellValue]) -> CellValue {
        eval(id, a).unwrap_or_else(CellValue::Err)
    }

    fn s(v: &str) -> CellValue {
        CellValue::Text(v.into())
    }

    #[test]
    fn unknown_id_is_name() {
        assert_eq!(eval(1549, &[]), Err(err::NAME));
    }

    #[test]
    fn special_branches_the_goldens_miss() {
        // AGM: NaN, a rescale on underflow and on overflow.
        assert!(agm(f64::NAN, 1.0).is_nan());
        let tiny = agm(1e-200, 4e-200);
        assert!((tiny / agm(1.0, 4.0) - 1e-200).abs() < 1e-212, "{tiny}");
        let huge = agm(1e200, 4e200);
        assert!((huge / agm(1.0, 4.0) / 1e200 - 1.0).abs() < 1e-14, "{huge}");
        // ILOG: x ≥ 2¹²⁷ and x < 1 take the f64 path.
        assert_eq!(ilog(1e300, 10.0), Ok(300.0));
        assert_eq!(ilog(0.001, 10.0), Ok(-3.0));
        assert_eq!(ilog(0.0011, 10.0), Ok(-3.0));
        assert_eq!(ilog(1e-300, 3.0), Ok(-629.0));
        assert_eq!(ilog(3.4e38, 3.0), Ok(80.0));
        assert_eq!(ilog(f64::MAX, 255.0), Ok(128.0));
        assert_eq!(ilog(f64::INFINITY, 10.0), Err(err::NUM));
        // ln x / ln 10 rounds up to an integer just above a power below 1
        // (Gnumeric -6 and -21), or is -2.9999… for 0.001 itself (-3).
        assert_eq!(ilog(9.999_999_999_999_999e-6, 10.0), Ok(-6.0));
        assert_eq!(ilog(9.999_999_999_999_998e-21, 10.0), Ok(-21.0));
        // Powers of two from the exponent; others by integer powers
        // (Gnumeric 10, 4, 3; a negative x #NUM!). Below 1 with a base
        // 2^k, k > 1, the exact answer: 8⁻¹ ≤ 0.5 < 8⁰ is -1 (Gnumeric
        // says 0 there, as it says -1 for ILOG(0.2, 4); see the "exact"
        // goldens).
        assert_eq!(ilog(1024.0, 2.0), Ok(10.0));
        assert_eq!(ilog(1000.0, 4.0), Ok(4.0));
        assert_eq!(ilog(1000.0, 10.0), Ok(3.0));
        assert_eq!(ilog(0.5, 8.0), Ok(-1.0));
        assert_eq!(ilog(-1.0, 10.0), Err(err::NUM));
        assert_eq!(ilog(8.0, 1.5), Err(err::NUM));
        assert_eq!(ilog(8.0, f64::INFINITY), Err(err::NUM));
        // AGM: equal arguments, or a zero one (Gnumeric 2 and 0).
        assert_eq!(agm(2.0, 2.0), 2.0);
        assert_eq!(agm(0.0, 1.0), 0.0);
        assert_eq!(agm(1.0, 0.0), 0.0);
        assert!(agm(-1.0, 1.0).is_nan() && agm(1.0, -1.0).is_nan());
        assert!(next_after(1.0, f64::NAN).is_nan());
        assert_eq!(next_after(1.0, 1.0), 1.0);
        assert_eq!(next_after(1.0, 2.0), 1.0 + f64::EPSILON);
        // HDATE_JULIAN: arguments out of range.
        assert_eq!(calendar::jdn(1e8, 1.0, 1.0), Err(err::NUM));
        assert_eq!(calendar::jdn(2000.0, -1e8, 1.0), Err(err::NUM));
        assert_eq!(calendar::jdn(2000.0, 1.0, 1e8), Err(err::NUM));
        assert_eq!(hebrew::hdate_of_jdn(0), Err(err::NUM));
        // FLT.NEXTAFTER: NaN, downward, other text.
        assert!(next_after(f64::NAN, 1.0).is_nan());
        assert_eq!(next_after(1.0, 0.0), 1.0 - f64::EPSILON / 2.0);
        let x = CellValue::num(1.0);
        assert_eq!(eval(1478, &[x.clone(), s("up")]), Err(err::VALUE));
        assert_eq!(
            text(1478, &[x, s("-")]),
            CellValue::num(1.0 - f64::EPSILON / 2.0)
        );
    }

    #[test]
    fn text_branches_the_goldens_miss() {
        // ROMAN's form: a non-number is #VALUE!; a date reads as a number.
        assert_eq!(
            text(1485, &[CellValue::num(4.0), s("x")]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(roman(3999.0, 0.0), Ok("MMMCMXCIX".into()));
        assert_eq!(roman(3999.0, 4.0), Ok("MMMIM".into()));
        // The forms, against LibreOffice 25.2.
        for (x, form, want) in [
            (499.0, 0.0, "CDXCIX"),
            (499.0, 1.0, "LDVLIV"),
            (499.0, 2.0, "XDIX"),
            (499.0, 3.0, "VDIV"),
            (499.0, 4.0, "ID"),
            (999.0, 1.0, "LMVLIV"),
            (490.0, 4.0, "XD"),
            (45.0, 1.0, "VL"),
            (8.0, 0.0, "VIII"),
            (1950.0, 4.0, "MLM"),
        ] {
            assert_eq!(roman(x, form), Ok(want.into()), "{x} {form}");
        }
        assert_eq!(roman(4000.0, 0.0), Err(err::VALUE));
        assert_eq!(roman(1.0, 5.0), Err(err::VALUE));
        // ARABIC: an invalid second numeral, a lone minus.
        assert_eq!(arabic("XZ"), Err(err::VALUE));
        assert_eq!(arabic("-"), Ok(CellValue::num(0.0)));
        assert_eq!(arabic("VX"), Err(err::VALUE));
        assert_eq!(arabic("IXIX"), Err(err::VALUE));
        // LibreOffice 25.2: 1999, 4 (IIII), 99 (IC), 40 (XXXX); VV, IIX
        // and CMM are invalid. Excel's leading minus and spaces.
        assert_eq!(arabic("MCMXCIX"), Ok(CellValue::num(1999.0)));
        assert_eq!(arabic("IIII"), Ok(CellValue::num(4.0)));
        assert_eq!(arabic("IC"), Ok(CellValue::num(99.0)));
        assert_eq!(arabic("XXXX"), Ok(CellValue::num(40.0)));
        assert_eq!(arabic("VV"), Err(err::VALUE));
        assert_eq!(arabic("IIX"), Err(err::VALUE));
        assert_eq!(arabic("CMM"), Err(err::VALUE));
        assert_eq!(arabic(" -MMXIV "), Ok(CellValue::num(-2014.0)));
        // BAHTTEXT: too large; a negative that rounds to zero keeps its sign.
        assert_eq!(baht_text(1e17), Err(err::NUM));
        assert_eq!(baht_text(-0.001), Ok("ลบศูนย์บาทถ้วน".into()));
        // JIS: ウ and a sound mark stay apart; a mark with nothing before it.
        assert_eq!(jis("ｳﾞ"), "ウ゛");
        assert_eq!(jis("ﾞ"), "゛");
        assert_eq!(fold(width::FULL_TO_HALF, '😀'), '😀');
        assert_eq!(ch(0xD800), '\u{FFFD}');
    }
}
