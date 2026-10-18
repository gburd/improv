//! Evaluation of the P7 math extras (ids 1450–1549): special functions,
//! number theory, floating point, calendars, telephone traffic and text.

mod width;

use super::{domain, err, n, ok_num, ok_text, t, R};
use crate::CellValue;

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    match id {
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
    fn text_branches_the_goldens_miss() {
        // ROMAN's form: a non-number is #VALUE!; a date reads as a number.
        assert_eq!(
            text(1485, &[CellValue::num(4.0), s("x")]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(roman(3999.0, 0.0), Ok("MMMCMXCIX".into()));
        assert_eq!(roman(3999.0, 4.0), Ok("MMMIM".into()));
        // ARABIC: an invalid second numeral, a lone minus.
        assert_eq!(arabic("XZ"), Err(err::VALUE));
        assert_eq!(arabic("-"), Ok(CellValue::num(0.0)));
        assert_eq!(arabic("VX"), Err(err::VALUE));
        assert_eq!(arabic("IXIX"), Err(err::VALUE));
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
