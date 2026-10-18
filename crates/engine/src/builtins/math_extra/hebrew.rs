//! The Hebrew calendar, as Gnumeric's `fn-hebrew-date` computes it: the
//! molad of Tishri by the fixed arithmetic of the calendar (1 month =
//! 29 d 12 h 793 parts, 1080 parts an hour) and its four postponements,
//! then the month lengths a year's length implies.

use super::calendar::jdn;
use crate::builtins::err;

const HOUR: i64 = 1080;
const DAY: i64 = 24 * HOUR;
const WEEK: i64 = 7 * DAY;
const MONTH: i64 = DAY + 12 * HOUR + 793;

const fn parts(h: i64, p: i64) -> i64 {
    h * HOUR + p
}

/// Days from 1 Tishri 3744 to 1 Tishri of year 3744 + y (y ≥ 0).
fn days_from_start(y: i64) -> i64 {
    let leaps = y * 7 + 1;
    let months = y * 12 + leaps / 19;
    let cycle = leaps % 19;
    // The molad of 3744 plus 6 hours (the day starts at 18:00).
    let mut nm = months * MONTH + parts(7, 779);
    let mut s = months * 28 + nm / DAY - 2;
    nm %= WEEK;
    let mut dw = nm / DAY;
    nm %= DAY;
    // GaTaRaD and BeTUTaKPaT: a late molad on Tuesday or Monday.
    if (cycle < 12 && dw == 3 && nm >= parts(9 + 6, 204))
        || (cycle < 7 && dw == 2 && nm >= parts(15 + 6, 589))
    {
        s += 1;
        dw += 1;
    }
    // Lo ADU Rosh: never Sunday, Wednesday or Friday.
    if matches!(dw, 1 | 4 | 6) {
        s += 1;
    }
    s
}

/// The Hebrew (day 1-based, month 0 Tishri … 11 Elul, 12 Adar I,
/// 13 Adar II, year) of a Julian day number.
fn from_jdn(jd: i64) -> (i64, i64, i64) {
    // The Gregorian year of jd, as an estimate of the Hebrew one.
    let mut l = jd + 68569;
    let n = (4 * l) / 146_097;
    l -= (146_097 * n + 3) / 4;
    let i = (4000 * (l + 1)) / 1_461_001;
    l = l - (1461 * i) / 4 + 31;
    let j = (80 * l) / 2447;
    let mut y = 100 * (n - 49) + i + j / 11 + 16;
    let mut d = jd - 1_715_119;
    let mut s = days_from_start(y);
    let mut next = days_from_start(y + 1);
    while d >= next {
        s = next;
        y += 1;
        next = days_from_start(y + 1);
    }
    d -= s;
    let len = next - s;
    let year = y + 3744;
    let month = if d >= len - 236 {
        // The last 8 months are regular, 30 and 29 days alternating.
        d -= len - 236;
        let mut m = d * 2 / 59;
        d -= (m * 59 + 1) / 2;
        m += 4;
        if len > 365 && m <= 5 {
            // Adar I and Adar II of a leap year.
            m += 8;
        }
        m
    } else {
        // Tishri to Tevet: 117 to 119 days (deficient, regular, complete).
        let first = 114 + len % 10;
        let m = d * 4 / first;
        d -= (m * first + 3) / 4;
        m
    };
    (d + 1, month, year)
}

/// The Hebrew date of Gregorian (y, m, d): Gnumeric's checks (month 1–12,
/// day 1–31, year ≥ 1; out-of-range days roll over) else `#VALUE!`.
pub(super) fn hdate(y: f64, m: f64, d: f64) -> Result<(i64, i64, i64), u8> {
    let (y, m, d) = (y.trunc(), m.trunc(), d.trunc());
    if !((1.0..=12.0).contains(&m) && (1.0..=31.0).contains(&d) && (1.0..=1e6).contains(&y)) {
        return Err(err::VALUE);
    }
    Ok(from_jdn(jdn(y, m, d)?))
}

/// The Hebrew date of a Julian day number (a date argument); before
/// Gregorian year 1 is `#NUM!`.
pub(super) fn hdate_of_jdn(jd: i64) -> Result<(i64, i64, i64), u8> {
    if !(1_721_426..=366_000_000).contains(&jd) {
        return Err(err::NUM);
    }
    Ok(from_jdn(jd))
}

const MONTHS: [&str; 14] = [
    "Tishri", "Heshwan", "Kislev", "Tebet", "Shebat", "Adar", "Nisan", "Iyar", "Sivan", "Tammuz",
    "Ab", "Elul", "Adar I", "Adar II",
];

/// The pointed Hebrew month names Gnumeric uses (its `hdate_strings.c`).
const MONTHS_HEB: [&str; 14] = [
    "ת\u{05BC}\u{05B4}ש\u{05C1}\u{05B0}ר\u{05B5}י",
    "ח\u{05B6}ש\u{05C1}\u{05B0}ו\u{05C7}ן",
    "כ\u{05BC}\u{05B4}ס\u{05B0}ל\u{05B5}ו",
    "ט\u{05B5}ב\u{05B5}ת",
    "ש\u{05C1}\u{05B0}ב\u{05C7}ט",
    "א\u{05B7}ד\u{05C7}ר",
    "נ\u{05B4}יס\u{05C7}ן",
    "א\u{05B4}י\u{05C7}יר",
    "ס\u{05B4}יו\u{05C7}ן",
    "ת\u{05BC}\u{05C7}מו\u{05BC}ז",
    "א\u{05B8}ב",
    "א\u{05B1}לו\u{05BC}ל",
    "א\u{05B7}ד\u{05C7}ר א\u{05F3}",
    "א\u{05B7}ד\u{05C7}ר ב\u{05F3}",
];

/// "20 Tebet 5784".
pub(super) fn text((d, m, y): (i64, i64, i64)) -> String {
    format!("{d} {} {y}", MONTHS[m as usize])
}

/// "כ׳ בְּטֵבֵת התשפ״ד": day and year in Hebrew numerals, the month pointed.
pub(super) fn text_heb((d, m, y): (i64, i64, i64)) -> String {
    format!(
        "{} ב\u{05BC}\u{05B0}{} {}",
        numeral(d),
        MONTHS_HEB[m as usize],
        numeral(y)
    )
}

/// A Hebrew numeral (1–9999): thousands as a letter, hundreds from ת
/// down, 15 and 16 as ט״ו and ט״ז, then a geresh after one letter or a
/// gershayim before the last.
fn numeral(mut n: i64) -> String {
    const ONES: [char; 10] = [' ', 'א', 'ב', 'ג', 'ד', 'ה', 'ו', 'ז', 'ח', 'ט'];
    const TENS: [char; 10] = ['ט', 'י', 'כ', 'ל', 'מ', 'נ', 'ס', 'ע', 'פ', 'צ'];
    const HUNDREDS: [char; 5] = [' ', 'ק', 'ר', 'ש', 'ת'];
    if !(1..=10_000).contains(&n) {
        return String::new();
    }
    let mut out: Vec<char> = Vec::new();
    if n >= 1000 {
        out.push(ONES[(n / 1000) as usize]);
        n %= 1000;
    }
    while n >= 400 {
        out.push('ת');
        n -= 400;
    }
    if n >= 100 {
        out.push(HUNDREDS[(n / 100) as usize]);
        n %= 100;
    }
    if n >= 10 {
        if n == 15 || n == 16 {
            n -= 9;
        }
        out.push(TENS[(n / 10) as usize]);
        n %= 10;
    }
    if n > 0 {
        out.push(ONES[n as usize]);
    }
    if out.len() < 2 {
        out.push('\u{05F3}');
    } else {
        out.insert(out.len() - 1, '\u{05F4}');
    }
    out.into_iter().collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn numerals() {
        assert_eq!(numeral(1), "א׳");
        assert_eq!(numeral(15), "ט״ו");
        assert_eq!(numeral(16), "ט״ז");
        assert_eq!(numeral(5784), "התשפ״ד");
        assert_eq!(numeral(0), "");
        assert_eq!(numeral(1000), "א׳");
    }

    #[test]
    fn round_trip_of_a_year() {
        // Day numbers advance by one across 4 years, months 0–13 in range.
        let start = jdn(2023.0, 1.0, 1.0).unwrap();
        let mut prev = from_jdn(start - 1);
        for jd in start..start + 1461 {
            let cur = from_jdn(jd);
            assert!(cur.0 == prev.0 + 1 || cur.0 == 1, "{prev:?} -> {cur:?}");
            assert!((0..14).contains(&cur.1));
            prev = cur;
        }
    }
}
