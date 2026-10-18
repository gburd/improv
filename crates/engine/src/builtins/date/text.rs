//! `TEXT`, `FIXED`, `DOLLAR`, `NUMBERVALUE`, `VALUETOTEXT` and the regular
//! expression functions. Patterns use the `regex` crate's syntax (no
//! look-around or back-references), not ICU's as in Excel and LibreOffice;
//! replacements take `$n` for group n and `\c` for a literal c, as ICU does.

use super::super::{b, d, err, n, n_or, ok_num, ok_text, t, text_of, R, SERIAL_EPOCH_MS};
use super::{DAY_NAMES, MONTH_NAMES};
use crate::CellValue;
use chrono::{Datelike, Timelike};
use regex::{Captures, Regex, RegexBuilder};

pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    match id {
        330 => ok_text(text(a)?),
        331 => {
            let commas = a.len() < 3 || !b(a, 2)?;
            ok_text(fixed(n(a, 0)?, n_or(a, 1, 2.0)?, commas)?)
        }
        332 => {
            let x = n(a, 0)?;
            let s = fixed(x.abs(), n_or(a, 1, 2.0)?, true)?;
            ok_text(if x < 0.0 {
                format!("(${s})")
            } else {
                format!("${s}")
            })
        }
        333 => number_value(a),
        334 => value_to_text(a),
        335 => regex_lo(a),
        336 => Ok(CellValue::Bool(
            re(&t(a, 1)?, case_flag(a, 2)?)?.is_match(&t(a, 0)?),
        )),
        337 => {
            if n_or(a, 2, 0.0)? != 0.0 {
                // Modes 1 and 2 return arrays in Excel.
                return Err(err::VALUE);
            }
            let s = t(a, 0)?;
            let m = re(&t(a, 1)?, case_flag(a, 3)?)?.find(&s).ok_or(err::NA)?;
            ok_text(m.as_str().into())
        }
        338 => {
            let (s, rep) = (t(a, 0)?, t(a, 2)?);
            let r = re(&t(a, 1)?, case_flag(a, 4)?)?;
            let occ = n_or(a, 3, 0.0)?.trunc();
            let count = r.find_iter(&s).count() as f64;
            let nth = match occ {
                0.0 => None,
                o if o > 0.0 => Some(o),
                o => Some(count + 1.0 + o),
            };
            ok_text(replace(&r, &s, &rep, nth))
        }
        _ => Err(err::NAME),
    }
}

// ponytail: compiles the pattern on every call; cache compiled patterns if
// regex-heavy models show it in a profile.
fn re(pat: &str, ignore_case: bool) -> Result<Regex, u8> {
    RegexBuilder::new(pat)
        .case_insensitive(ignore_case)
        .build()
        .map_err(|_| err::VALUE)
}

/// Excel's `case_sensitivity` argument `i`: 0 (default) matches case, 1
/// ignores it.
fn case_flag(a: &[CellValue], i: usize) -> Result<bool, u8> {
    match n_or(a, i, 0.0)? {
        0.0 => Ok(false),
        1.0 => Ok(true),
        _ => Err(err::VALUE),
    }
}

/// `rep` with `$n` replaced by group n of `c` and `\c` by c.
fn expand(c: &Captures, rep: &str) -> String {
    let mut out = String::new();
    let mut chars = rep.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => out.extend(chars.next()),
            '$' if chars.peek().is_some_and(|d| d.is_ascii_digit()) => {
                let mut k = 0usize;
                while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                    k = k.saturating_mul(10).saturating_add(d as usize);
                    chars.next();
                }
                out.push_str(c.get(k).map_or("", |m| m.as_str()));
            }
            _ => out.push(ch),
        }
    }
    out
}

/// `s` with every match (`nth` None) or the nth (1-based) replaced.
fn replace(r: &Regex, s: &str, rep: &str, nth: Option<f64>) -> String {
    let mut out = String::new();
    let mut last = 0;
    for (i, c) in r.captures_iter(s).enumerate() {
        let m = c.get(0).expect("group 0 is the match");
        if nth.is_none_or(|k| k == (i + 1) as f64) {
            out.push_str(&s[last..m.start()]);
            out.push_str(&expand(&c, rep));
            last = m.end();
        }
    }
    out.push_str(&s[last..]);
    out
}

/// LibreOffice `REGEX(text, expr, [replacement], [flags or occurrence])`.
fn regex_lo(a: &[CellValue]) -> R {
    let (s, r) = (t(a, 0)?, re(&t(a, 1)?, false)?);
    if a.len() < 3 {
        return ok_text(r.find(&s).ok_or(err::NA)?.as_str().into());
    }
    let nth = match a.get(3) {
        None => Some(1.0),
        Some(CellValue::Text(f)) if f.eq_ignore_ascii_case("g") => None,
        Some(CellValue::Num(_)) => {
            let k = n(a, 3)?.trunc();
            if k < 0.0 {
                return Err(err::VALUE);
            }
            Some(k)
        }
        Some(_) => return Err(err::VALUE),
    };
    ok_text(replace(&r, &s, &t(a, 2)?, nth))
}

/// `NUMBERVALUE(text, [decimal_separator], [group_separator])`.
fn number_value(a: &[CellValue]) -> R {
    let sep = |i: usize, default: char| -> Result<char, u8> {
        if a.len() > i {
            t(a, i)?.chars().next().ok_or(err::VALUE)
        } else {
            Ok(default)
        }
    };
    let (dec, group) = (sep(1, '.')?, sep(2, ',')?);
    if dec == group {
        return Err(err::VALUE);
    }
    let s: String = t(a, 0)?.chars().filter(|c| !c.is_whitespace()).collect();
    let body = s.trim_end_matches('%');
    let pct = s.len() - body.len();
    let (int, frac) = body.split_once(dec).unwrap_or((body, ""));
    if frac.contains(dec) || frac.contains(group) {
        return Err(err::VALUE);
    }
    let int: String = int.chars().filter(|c| *c != group).collect();
    let num = if body.contains(dec) {
        format!("{int}.{frac}")
    } else {
        int
    };
    if num.is_empty() || num == "." {
        return ok_num(0.0);
    }
    if !num
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | '+' | '-' | 'e' | 'E'))
    {
        return Err(err::VALUE);
    }
    let v: f64 = num.parse().map_err(|_| err::VALUE)?;
    ok_num((0..pct).fold(v, |v, _| v / 100.0))
}

/// `VALUETOTEXT(value, [format])`: 0 concise, 1 strict (text in quotes).
/// Error values become their code; an error format still propagates.
fn value_to_text(a: &[CellValue]) -> R {
    if let Some(CellValue::Err(c)) = a.get(1) {
        return Err(*c);
    }
    let strict = match n_or(a, 1, 0.0)? {
        0.0 => false,
        1.0 => true,
        _ => return Err(err::VALUE),
    };
    let v = a.first().ok_or(err::VALUE)?;
    ok_text(match v {
        CellValue::Err(_) => v.to_string(),
        CellValue::Text(s) if strict => format!("\"{s}\""),
        _ => text_of(v)?,
    })
}

/// `FIXED`: `x` rounded to `dec` places (negative rounds left of the point)
/// with optional thousands commas.
fn fixed(x: f64, dec: f64, commas: bool) -> Result<String, u8> {
    let dec = dec.trunc();
    if dec > 127.0 {
        return Err(err::VALUE);
    }
    let (int, frac) = if dec >= 0.0 {
        round_digits(x.abs(), dec as usize)?
    } else {
        let k = (-dec).min(400.0) as i32;
        let (i, _) = round_digits(x.abs() / 10f64.powi(k), 0)?;
        let i = if i.is_empty() {
            i
        } else {
            i + &"0".repeat(k as usize)
        };
        (i, String::new())
    };
    let int = if int.is_empty() { "0".into() } else { int };
    let zero = int == "0" && !frac.contains(|c| c != '0');
    let int = if commas { group(&int) } else { int };
    let sign = if x < 0.0 && !zero { "-" } else { "" };
    Ok(if frac.is_empty() {
        format!("{sign}{int}")
    } else {
        format!("{sign}{int}.{frac}")
    })
}

/// Digits with a comma every three from the right.
fn group(digits: &str) -> String {
    let n = digits.len();
    let mut out = String::with_capacity(n + n / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (n - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `x ≥ 0` rounded half away from zero to `dec` decimal places, decimally
/// (2.675 to 2 places is 2.68): the integer digits without leading zeros
/// ("" for 0) and exactly `dec` fraction digits.
fn round_digits(x: f64, dec: usize) -> Result<(String, String), u8> {
    let y = x * 10f64.powi(dec.min(400) as i32);
    if !y.is_finite() {
        return Err(err::VALUE);
    }
    let s = if y < 1e15 {
        let y: f64 = format!("{y:.14e}").parse().unwrap_or(y);
        format!("{:.0}", y.round())
    } else {
        // Beyond 15 significant digits the spreadsheets show zeros.
        let e = format!("{y:.14e}");
        let (m, exp) = e.split_once('e').expect("exponent form");
        let digits = m.replace('.', "");
        let exp: usize = exp.parse().expect("a non-negative exponent");
        digits + &"0".repeat(exp - 14)
    };
    let s = if s.len() <= dec {
        "0".repeat(dec + 1 - s.len()) + &s
    } else {
        s
    };
    let (i, f) = s.split_at(s.len() - dec);
    Ok((i.trim_start_matches('0').into(), f.into()))
}

// --- TEXT ------------------------------------------------------------------

/// A format-code token.
#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Lit(String),
    /// A `/` outside quotes: a date separator, or a fraction (unsupported).
    Slash,
    /// A digit placeholder: `0`, `#` or `?`.
    Dig(char),
    Point,
    Comma,
    Pct,
    /// `E+`/`E-` and its digit placeholders.
    Exp(char, bool, usize),
    At,
    General,
    Year(usize),
    Month(usize),
    Day(usize),
    Hour(usize),
    Minute(usize),
    Second(usize),
    /// `AM/PM` (true) or `A/P`, lowercase when written so.
    AmPm(bool, bool),
}

const COLORS: [&str; 8] = [
    "black", "blue", "cyan", "green", "magenta", "red", "white", "yellow",
];

/// The `;`-separated sections of a format, tokenized.
fn sections(f: &str) -> Result<Vec<Vec<Tok>>, u8> {
    let c: Vec<char> = f.chars().collect();
    let mut out = vec![Vec::new()];
    let mut i = 0;
    let run = |i: usize, ch: char| {
        c[i..]
            .iter()
            .take_while(|x| x.eq_ignore_ascii_case(&ch))
            .count()
    };
    let starts = |i: usize, w: &str| {
        let s: String = c[i..].iter().take(w.len()).collect();
        s.eq_ignore_ascii_case(w)
    };
    while i < c.len() {
        let toks = out.last_mut().expect("one section");
        let ch = c[i];
        let mut len = 1;
        match ch {
            ';' => {
                out.push(Vec::new());
            }
            '"' => {
                let end = c[i + 1..]
                    .iter()
                    .position(|x| *x == '"')
                    .ok_or(err::VALUE)?;
                toks.push(Tok::Lit(c[i + 1..i + 1 + end].iter().collect()));
                len = end + 2;
            }
            '\\' | '_' | '*' => {
                let next = *c.get(i + 1).ok_or(err::VALUE)?;
                match ch {
                    '\\' => toks.push(Tok::Lit(next.into())),
                    '_' => toks.push(Tok::Lit(" ".into())),
                    _ => {} // a repeat fill has no width in text
                }
                len = 2;
            }
            '[' => {
                let end = c[i..].iter().position(|x| *x == ']').ok_or(err::VALUE)?;
                let inner: String = c[i + 1..i + end].iter().collect();
                if !COLORS.iter().any(|k| inner.eq_ignore_ascii_case(k))
                    && !inner.to_ascii_lowercase().starts_with("color")
                {
                    return Err(err::VALUE); // conditions, locales, [h]
                }
                len = end + 1;
            }
            '0' | '#' | '?' => toks.push(Tok::Dig(ch)),
            '.' => toks.push(Tok::Point),
            ',' => toks.push(Tok::Comma),
            '%' => toks.push(Tok::Pct),
            '@' => toks.push(Tok::At),
            '/' => toks.push(Tok::Slash),
            'E' | 'e' if matches!(c.get(i + 1), Some('+' | '-')) => {
                let digits = c[i + 2..]
                    .iter()
                    .take_while(|x| matches!(x, '0' | '#'))
                    .count();
                toks.push(Tok::Exp(ch, c[i + 1] == '+', digits));
                len = 2 + digits;
            }
            'G' | 'g' if starts(i, "general") => {
                toks.push(Tok::General);
                len = 7;
            }
            'A' | 'a' if starts(i, "am/pm") => {
                toks.push(Tok::AmPm(true, ch == 'a'));
                len = 5;
            }
            'A' | 'a' if starts(i, "a/p") => {
                toks.push(Tok::AmPm(false, ch == 'a'));
                len = 3;
            }
            'y' | 'Y' | 'm' | 'M' | 'd' | 'D' | 'h' | 'H' | 's' | 'S' => {
                len = run(i, ch);
                toks.push(match ch.to_ascii_lowercase() {
                    'y' => Tok::Year(len),
                    'm' => Tok::Month(len),
                    'd' => Tok::Day(len),
                    'h' => Tok::Hour(len),
                    _ => Tok::Second(len),
                });
            }
            c if c.is_ascii_alphabetic() => return Err(err::VALUE),
            c => toks.push(Tok::Lit(c.into())),
        }
        i += len;
    }
    if out.len() > 4 {
        return Err(err::VALUE);
    }
    Ok(out)
}

fn is_date_tok(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Year(_)
            | Tok::Month(_)
            | Tok::Day(_)
            | Tok::Hour(_)
            | Tok::Minute(_)
            | Tok::Second(_)
            | Tok::AmPm(..)
    )
}

/// `TEXT(value, format)`.
fn text(a: &[CellValue]) -> Result<String, u8> {
    let secs = sections(&t(a, 1)?)?;
    let x = match &a[0] {
        CellValue::Text(s) => match s.trim().parse::<f64>() {
            Ok(x) if x.is_finite() => x,
            _ => return text_section(&secs, s),
        },
        CellValue::Bool(_) => return text_section(&secs, &text_of(&a[0])?),
        CellValue::Date(ms) => super::serial(*ms),
        _ => n(a, 0)?,
    };
    let (sec, x, minus) = match secs.len() {
        k if x < 0.0 && k >= 2 => (&secs[1], -x, false),
        k if x == 0.0 && k >= 3 => (&secs[2], x, false),
        _ => (&secs[0], x.abs(), x < 0.0),
    };
    if sec.iter().any(is_date_tok) {
        if minus {
            return Err(err::VALUE);
        }
        return date_section(sec, x);
    }
    number_section(sec, x, minus)
}

/// Text through the fourth section (or a lone section with `@`); other
/// formats leave text as it is.
fn text_section(secs: &[Vec<Tok>], s: &str) -> Result<String, u8> {
    let sec = match secs.len() {
        4 => &secs[3],
        1 if secs[0].contains(&Tok::At) => &secs[0],
        _ => return Ok(s.into()),
    };
    sec.iter()
        .map(|t| match t {
            Tok::At => Ok(s.into()),
            Tok::Lit(l) => Ok(l.clone()),
            _ => Err(err::VALUE),
        })
        .collect()
}

fn date_section(sec: &[Tok], x: f64) -> Result<String, u8> {
    let ms = d(&[CellValue::num(x)], 0)?;
    let ms = (ms - SERIAL_EPOCH_MS + 500).div_euclid(1000) * 1000 + SERIAL_EPOCH_MS;
    let dt = chrono::DateTime::from_timestamp_millis(ms).ok_or(err::VALUE)?;
    let twelve = sec.iter().any(|t| matches!(t, Tok::AmPm(..)));
    // `m` right after an hour or right before a second is minutes.
    let dates: Vec<usize> = (0..sec.len()).filter(|i| is_date_tok(&sec[*i])).collect();
    let mut toks = sec.to_vec();
    for (k, &i) in dates.iter().enumerate() {
        if let Tok::Month(len) = sec[i] {
            let after_h = k > 0 && matches!(sec[dates[k - 1]], Tok::Hour(_));
            let before_s = dates
                .get(k + 1)
                .is_some_and(|j| matches!(sec[*j], Tok::Second(_)));
            if len <= 2 && (after_h || before_s) {
                toks[i] = Tok::Minute(len);
            }
        }
    }
    let pad = |v: u32, len: usize| {
        if len >= 2 {
            format!("{v:02}")
        } else {
            v.to_string()
        }
    };
    let mut out = String::new();
    for t in &toks {
        match t {
            Tok::Lit(l) => out.push_str(l),
            Tok::Slash => out.push('/'),
            Tok::Point => out.push('.'),
            Tok::Comma => out.push(','),
            Tok::Pct => out.push('%'),
            Tok::Year(len) if *len <= 2 => {
                out.push_str(&format!("{:02}", dt.year().rem_euclid(100)))
            }
            Tok::Year(_) => out.push_str(&format!("{:04}", dt.year())),
            Tok::Month(len) => {
                let name = MONTH_NAMES[dt.month0() as usize];
                match len {
                    1 | 2 => out.push_str(&pad(dt.month(), *len)),
                    3 => out.push_str(&name[..3]),
                    5 => out.push_str(&name[..1]),
                    _ => out.push_str(name),
                }
            }
            Tok::Day(len) => {
                let name = DAY_NAMES[dt.weekday().num_days_from_sunday() as usize];
                match len {
                    1 | 2 => out.push_str(&pad(dt.day(), *len)),
                    3 => out.push_str(&name[..3]),
                    _ => out.push_str(name),
                }
            }
            Tok::Hour(len) => {
                let h = dt.hour();
                let h = if twelve { (h + 11) % 12 + 1 } else { h };
                out.push_str(&pad(h, *len));
            }
            Tok::Minute(len) => out.push_str(&pad(dt.minute(), *len)),
            Tok::Second(len) => out.push_str(&pad(dt.second(), *len)),
            Tok::AmPm(long, lower) => {
                let s = match (dt.hour() < 12, long) {
                    (true, true) => "AM",
                    (false, true) => "PM",
                    (true, false) => "A",
                    (false, false) => "P",
                };
                out.push_str(&if *lower {
                    s.to_ascii_lowercase()
                } else {
                    s.into()
                });
            }
            // Fractional seconds, digits or @ in a date format.
            _ => return Err(err::VALUE),
        }
    }
    Ok(out)
}

/// A number format section applied to `x ≥ 0`; `minus` prefixes a sign.
fn number_section(sec: &[Tok], mut x: f64, minus: bool) -> Result<String, u8> {
    let sign = if minus { "-" } else { "" };
    if sec.iter().any(|t| matches!(t, Tok::Slash | Tok::At)) {
        return Err(err::VALUE); // fractions; @ outside the text section
    }
    if let Some(g) = sec.iter().position(|t| *t == Tok::General) {
        let mut lit = String::new();
        for (i, t) in sec.iter().enumerate() {
            match t {
                Tok::Lit(l) => lit.push_str(l),
                _ if i == g => lit.push_str(&crate::display_number(x)),
                _ => return Err(err::VALUE), // General beside digits or %
            }
        }
        return Ok(format!("{sign}{lit}"));
    }
    let exp = sec.iter().position(|t| matches!(t, Tok::Exp(..)));
    let end = exp.unwrap_or(sec.len());
    let point = sec[..end]
        .iter()
        .position(|t| *t == Tok::Point)
        .unwrap_or(end);
    let digs = |r: std::ops::Range<usize>| -> Vec<char> {
        sec[r]
            .iter()
            .filter_map(|t| match t {
                Tok::Dig(c) => Some(*c),
                _ => None,
            })
            .collect()
    };
    let (ip, fp) = (digs(0..point), digs(point..end));
    let last_dig = sec[..end].iter().rposition(|t| matches!(t, Tok::Dig(_)));
    let mut grouped = false;
    for (i, t) in sec[..end].iter().enumerate() {
        if *t != Tok::Comma {
            continue;
        }
        match last_dig {
            Some(l) if i > l => x /= 1000.0,
            Some(_) if i < point && sec[..i].iter().any(|t| matches!(t, Tok::Dig(_))) => {
                grouped = true
            }
            _ => {}
        }
    }
    let pct = sec.iter().filter(|t| **t == Tok::Pct).count();
    x *= 100f64.powi(pct as i32);
    let mut e = 0i32;
    let (int, frac) = match exp {
        None => round_digits(x, fp.len())?,
        Some(_) => {
            // `0.0E+0`: one integer digit; `##0.0E+0`: an exponent that is a
            // multiple of the integer placeholders (engineering notation).
            let k = ip.len().max(1) as i32;
            let eng = k > 1 && ip.contains(&'#');
            let pick = |mag: i32| {
                if eng {
                    mag.div_euclid(k) * k
                } else {
                    mag - (k - 1)
                }
            };
            let mag = if x == 0.0 {
                k - 1
            } else {
                x.log10().floor() as i32
            };
            e = if x == 0.0 { 0 } else { pick(mag) };
            let mut r = round_digits(x / 10f64.powi(e), fp.len())?;
            if r.0.len() as i32 > k {
                // Rounding carried into a new digit (9.99 -> 10.0).
                e = pick(mag + 1);
                r = round_digits(x / 10f64.powi(e), fp.len())?;
            }
            r
        }
    };
    let last_frac = frac.rfind(|c| c != '0');
    let int: Vec<char> = int.chars().collect();
    let (mut ii, mut fi) = (0, 0);
    let mut head = String::new(); // the integer region, grouped below
    let mut out = String::new();
    for (i, t) in sec.iter().enumerate() {
        let buf = if i < point { &mut head } else { &mut out };
        match t {
            Tok::Lit(l) => buf.push_str(l),
            Tok::Pct => buf.push('%'),
            Tok::Comma | Tok::General | Tok::At | Tok::Slash => {}
            Tok::Point => {
                if ip.is_empty() {
                    head.extend(&int);
                }
                out.push('.');
            }
            Tok::Dig(c) if i < point => {
                // Right-to-left: placeholder r from the right takes digit r;
                // the leftmost also takes every digit beyond the placeholders.
                let r = ip.len() - 1 - ii;
                if ii == 0 && int.len() > ip.len() {
                    head.extend(&int[..int.len() - ip.len()]);
                }
                match int.len().checked_sub(r + 1) {
                    Some(j) => head.push(int[j]),
                    None => head.push_str(placeholder(*c)),
                }
                ii += 1;
            }
            Tok::Dig(c) if i < end => {
                if last_frac.is_some_and(|l| fi <= l) || *c == '0' {
                    out.push(frac.as_bytes()[fi] as char);
                } else {
                    out.push_str(placeholder(*c));
                }
                fi += 1;
            }
            Tok::Exp(ch, plus, width) => {
                out.push(*ch);
                if e < 0 {
                    out.push('-');
                } else if *plus {
                    out.push('+');
                }
                out.push_str(&format!("{:0width$}", e.unsigned_abs(), width = *width));
            }
            _ => return Err(err::VALUE),
        }
    }
    let head = if grouped { group_mixed(&head) } else { head };
    Ok(format!("{sign}{head}{out}"))
}

/// A missing digit: `0` shows 0, `?` a space, `#` nothing.
fn placeholder(c: char) -> &'static str {
    match c {
        '0' => "0",
        '?' => " ",
        _ => "",
    }
}

/// Thousands commas between the digits of `s`, which may hold literals.
fn group_mixed(s: &str) -> String {
    let total = s.chars().filter(char::is_ascii_digit).count();
    let mut seen = 0;
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            if seen > 0 && (total - seen).is_multiple_of(3) {
                out.push(',');
            }
            seen += 1;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::builtins::DAY_MS;

    fn fmt(x: f64, f: &str) -> Result<String, u8> {
        text(&[CellValue::num(x), CellValue::Text(f.into())])
    }

    /// TEXT, REGEX, NUMBERVALUE and VALUETOTEXT on the inputs the goldens
    /// leave to the integration binary. LibreOffice 25.2 and Gnumeric 1.12
    /// agree with these unless noted.
    #[test]
    fn text_edges_against_the_references() {
        let (x, s) = (CellValue::num, |v: &str| CellValue::Text(v.into()));
        let ok = |v: &str| Ok(v.to_string());
        // Colours (named and [colorN]) print nothing; `$` and `yyy` (four
        // digits, as Gnumeric; LibreOffice "23y"); `0,,` scales twice;
        // a trailing comma after the point scales too (LibreOffice and
        // Gnumeric 0.0); `#,###` groups.
        assert_eq!(fmt(5.0, "[Blue]0"), ok("5"));
        assert_eq!(fmt(5.0, "[color10]0"), ok("5"));
        assert_eq!(fmt(1234.5, "$0"), ok("$1235"));
        assert_eq!(fmt(45000.75, "yyy"), ok("2023"));
        assert_eq!(fmt(1_500_000.0, "0,,"), ok("2"));
        assert_eq!(fmt(1.5, "0.0,"), ok("0.0"));
        assert_eq!(fmt(1234.0, "#,###"), ok("1,234"));
        // A comma before any digit is a literal-free no-op here
        // (LibreOffice and Gnumeric print ",1234.5"), and one inside the
        // integer digits groups.
        assert_eq!(fmt(1234.5, ",0.0"), ok("1234.5"));
        assert_eq!(fmt(1234.5, "0,000.0"), ok("1,234.5"));
        // A comma among the fraction digits is dropped (LibreOffice and
        // Gnumeric 1.25).
        assert_eq!(fmt(1.25, "0.0,0"), ok("1.25"));
        // A comma between a literal and the digits scales nothing and
        // prints nothing (LibreOffice keeps it: "x,1235").
        assert_eq!(fmt(1234.5, "\"x\",0"), ok("x1235"));
        // Minutes after an hour and before a second; a lone m is the month.
        assert_eq!(fmt(45000.75, "yy-m-d h:mm"), ok("23-3-15 18:00"));
        assert_eq!(fmt(45000.75, "m"), ok("3"));
        assert_eq!(fmt(45000.75, "h m"), ok("18 0"));
        assert_eq!(fmt(45000.75, "m s"), ok("0 0"));
        assert_eq!(fmt(0.25, "h:mm a/p"), ok("6:00 a"));
        // General in either case (the G and the g arms), AM/PM upper and
        // lower case.
        assert_eq!(fmt(0.5, "General"), ok("0.5"));
        assert_eq!(fmt(0.5, "general"), ok("0.5"));
        assert_eq!(fmt(0.25, "h AM/PM"), ok("6 AM"));
        // (Lower-case am/pm prints lower case, as Excel; LibreOffice "PM".)
        assert_eq!(fmt(0.75, "h am/pm"), ok("6 pm"));
        // A capital G that is not General.
        assert_eq!(fmt(0.5, "Gx"), Err(err::VALUE));
        // Lower-case exponents (LibreOffice 1.2e+03, 0.0e0, 1.0e-3;
        // Gnumeric rejects them).
        assert_eq!(fmt(1234.0, "0.0e+00"), ok("1.2e+03"));
        assert_eq!(fmt(0.0, "0.0e-0"), ok("0.0e0"));
        assert_eq!(fmt(0.001, "0.0e+0"), ok("1.0e-3"));
        // Not formats here: a condition, an A without /P or M, a G that is
        // not General, an exponent without a sign (Excel #VALUE!; the
        // others print something).
        for f in ["[>3]0", "h:mm A", "g", "0.0E0"] {
            assert_eq!(fmt(5.0, f), Err(err::VALUE), "{f}");
        }
        // Text that parses as an infinite number is text.
        assert_eq!(text(&[s("1e999"), s("0")]), ok("1e999"));
        assert_eq!(text(&[s("abc"), s("0;0;0;[@]")]), Err(err::VALUE));
        // REGEX: a $ without a digit is literal; "G" is global as "g"
        // (LibreOffice Err:502 for both); occurrence 0 replaces nothing.
        let rx = |rep: &str, flag: CellValue| regex_lo(&[s("a1b2"), s("[0-9]"), s(rep), flag]);
        assert_eq!(rx("x$", x(1.0)), Ok(s("ax$b2")));
        assert_eq!(rx("#", s("G")), Ok(s("a#b#")));
        assert_eq!(rx("#", x(0.0)), Ok(s("a1b2")));
        // NUMBERVALUE: a lone separator is 0 (Excel; LibreOffice #VALUE!),
        // grouping in the integer part.
        assert_eq!(number_value(&[s(".")]), Ok(x(0.0)));
        assert_eq!(number_value(&[s(",")]), Ok(x(0.0)));
        assert_eq!(number_value(&[s("1,234.5")]), Ok(x(1234.5)));
        // VALUETOTEXT strict quotes text; REGEXREPLACE's negative
        // occurrence counts from the end (Excel docs: "aab").
        assert_eq!(value_to_text(&[s("a"), x(1.0)]), Ok(s("\"a\"")));
        assert_eq!(value_to_text(&[s("a"), x(0.0)]), Ok(s("a")));
        // FIXED of a negative (LibreOffice -1,234.6), and REGEXREPLACE's
        // positive occurrence (Excel docs: "aba").
        assert_eq!(fixed(-1234.567, 1.0, true), ok("-1,234.6"));
        assert_eq!(eval(338, &[s("aaa"), s("a"), s("b"), x(2.0)]), Ok(s("aba")));
        assert_eq!(
            eval(338, &[s("aaa"), s("a"), s("b"), x(-1.0)]),
            Ok(s("aab"))
        );
    }

    #[test]
    fn text_formats() {
        assert_eq!(fmt(1234.5, "#,##0.00").as_deref(), Ok("1,234.50"));
        assert_eq!(fmt(1234567.0, "#,##0,").as_deref(), Ok("1,235"));
        assert_eq!(fmt(5.0, "000-00").as_deref(), Ok("000-05"));
        assert_eq!(fmt(0.5, ".00").as_deref(), Ok(".50"));
        assert_eq!(fmt(1.5, "#.##").as_deref(), Ok("1.5"));
        assert_eq!(fmt(1.5, "?.??").as_deref(), Ok("1.5 "));
        assert_eq!(fmt(0.0, "#").as_deref(), Ok(""));
        assert_eq!(fmt(-1.0, "0;(0);\"zero\"").as_deref(), Ok("(1)"));
        assert_eq!(fmt(0.0, "0;(0);\"zero\"").as_deref(), Ok("zero"));
        assert_eq!(fmt(12345.0, "##0.0E+0").as_deref(), Ok("12.3E+3"));
        assert_eq!(fmt(0.0, "0.0E+00").as_deref(), Ok("0.0E+00"));
        assert_eq!(fmt(9.99, "0.0E+0").as_deref(), Ok("1.0E+1"));
        assert_eq!(fmt(0.012, "0.0E-0").as_deref(), Ok("1.2E-2"));
        assert_eq!(fmt(5.0, "[Red]0_)").as_deref(), Ok("5 "));
        assert_eq!(fmt(5.0, "*-0\\x").as_deref(), Ok("5x"));
        assert_eq!(fmt(-2.5, "\"v=\"General").as_deref(), Ok("-v=2.5"));
        assert_eq!(fmt(0.75, "h:mm a/p").as_deref(), Ok("6:00 p"));
        assert_eq!(fmt(0.25, "h AM/PM").as_deref(), Ok("6 AM"));
        assert_eq!(fmt(0.25, "h A/P").as_deref(), Ok("6 A"));
        assert_eq!(fmt(5.0, ",0").as_deref(), Ok("5"));
        assert_eq!(fmt(5.0, "General%"), Err(err::VALUE));
        assert_eq!(fmt(5.0, "0E+0 0"), Err(err::VALUE));
        assert_eq!(fmt(45658.0, "mmmmm d, yy").as_deref(), Ok("J 1, 25"));
        assert_eq!(fmt(45658.0, "m.d, \"x\"%").as_deref(), Ok("1.1, x%"));
        for bad in [
            "[>1]0",
            "0 x",
            "\"open",
            "0\\",
            "# ?/?",
            "ss.00",
            "e",
            "0;0;0;0;0",
            "0@",
        ] {
            assert_eq!(fmt(1.0, bad), Err(err::VALUE), "{bad}");
        }
        assert_eq!(fmt(-1.0, "yyyy"), Err(err::VALUE));
        assert_eq!(fmt(1e300, "0.00"), Ok(format!("1{}.00", "0".repeat(300))));
        assert_eq!(
            fmt(1e300, &format!("0.{}", "0".repeat(20))),
            Err(err::VALUE)
        );
        let s = |x: &str| CellValue::Text(x.into());
        let tx = |v: CellValue, f: &str| text(&[v, s(f)]);
        assert_eq!(tx(s("ab"), "0;0;0;[@]"), Err(err::VALUE));
        assert_eq!(tx(s("ab"), "\"<\"@\">\"").as_deref(), Ok("<ab>"));
        assert_eq!(tx(s("ab"), "0;0").as_deref(), Ok("ab"));
        assert_eq!(tx(s("ab"), "0;0;0;\"t:\"@").as_deref(), Ok("t:ab"));
        assert_eq!(tx(s("ab"), "0;0;0;0"), Err(err::VALUE));
        assert_eq!(tx(s(" 2 "), "0.0").as_deref(), Ok("2.0"));
        assert_eq!(tx(CellValue::Bool(true), "0").as_deref(), Ok("TRUE"));
        assert_eq!(
            tx(
                CellValue::Date(SERIAL_EPOCH_MS + DAY_MS + DAY_MS / 2),
                "d hh"
            )
            .as_deref(),
            Ok("31 12")
        );
    }

    #[test]
    fn fixed_dollar_and_numbervalue() {
        let call =
            |id, a: &[CellValue]| super::super::super::eval(improv_core_model::FuncId(id), a);
        let (x, s) = (CellValue::num, |v: &str| CellValue::Text(v.into()));
        assert_eq!(fixed(1234.567, -1.0, true).as_deref(), Ok("1,230"));
        assert_eq!(fixed(0.4, -1.0, true).as_deref(), Ok("0"));
        assert_eq!(fixed(-0.004, 2.0, true).as_deref(), Ok("0.00"));
        assert_eq!(fixed(1.0, 128.0, true), Err(err::VALUE));
        assert_eq!(call(332, &[x(-0.5), x(0.0)]), s("($1)"));
        assert_eq!(
            call(331, &[x(1234.5), x(1.0), CellValue::Bool(true)]),
            s("1234.5")
        );
        assert_eq!(call(333, &[s(" ")]), x(0.0));
        assert_eq!(call(333, &[s("1.2.3")]), CellValue::Err(err::VALUE));
        assert_eq!(call(333, &[s("1.2,3")]), CellValue::Err(err::VALUE));
        assert_eq!(call(333, &[s("1x")]), CellValue::Err(err::VALUE));
        assert_eq!(call(333, &[s("1-")]), CellValue::Err(err::VALUE));
        assert_eq!(call(333, &[s("1"), s("")]), CellValue::Err(err::VALUE));
        assert_eq!(
            call(333, &[s("1"), s(","), s(",")]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(call(333, &[s("5%%")]), x(0.0005));
    }

    #[test]
    fn value_to_text_and_regex() {
        let call =
            |id, a: &[CellValue]| super::super::super::eval(improv_core_model::FuncId(id), a);
        let (x, s) = (CellValue::num, |v: &str| CellValue::Text(v.into()));
        let na = CellValue::Err(err::NA);
        assert_eq!(value_to_text(std::slice::from_ref(&na)), Ok(s("#N/A")));
        assert_eq!(value_to_text(&[x(1.0), na.clone()]), Err(err::NA));
        assert_eq!(value_to_text(&[x(1.0), x(2.0)]), Err(err::VALUE));
        assert_eq!(value_to_text(&[]), Err(err::VALUE));
        assert_eq!(call(336, &[s("a"), s("(")]), CellValue::Err(err::VALUE));
        assert_eq!(
            call(336, &[s("a"), s("A"), x(2.0)]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(
            call(337, &[s("a"), s("a"), x(1.0)]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(
            call(335, &[s("ab"), s("b"), s("[$1\\$]"), s("x")]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(
            call(335, &[s("ab"), s("b"), s("x"), x(-1.0)]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(
            call(335, &[s("ab"), s("b"), s("x"), CellValue::Bool(true)]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(call(335, &[s("ab"), s("(b)"), s("[$1\\$$9]")]), s("a[b$]"));
        assert_eq!(super::super::eval(399, &[]), Err(err::NAME));
    }
}
