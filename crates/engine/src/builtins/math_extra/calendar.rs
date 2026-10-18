//! Calendars: the Gregorian movable feasts and Julian day numbers.

use super::super::{err, R};
use crate::CellValue;
use chrono::{NaiveDate, TimeDelta};

/// Unix day 0 (1970-01-01) as a Julian day number.
pub(super) const UNIX_JDN: i64 = 2_440_588;

/// Easter Sunday of `year` plus `offset` days. Years 0–29 mean 2000–2029
/// and 30–99 mean 1930–1999; otherwise 1583–9956 (LibreOffice's range).
pub(super) fn easter(year: f64, offset: i64) -> R {
    let y = year.trunc();
    let y = match y {
        _ if (0.0..30.0).contains(&y) => 2000.0 + y,
        _ if (30.0..100.0).contains(&y) => 1900.0 + y,
        _ => y,
    };
    if !(1583.0..=9956.0).contains(&y) {
        return Err(err::NUM);
    }
    let y = y as i32;
    // The anonymous Gregorian computus (Meeus, "Astronomical Algorithms").
    let (a, b, c) = (y % 19, y / 100, y % 100);
    let (d, e, f) = (b / 4, b % 4, (b + 8) / 25);
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let l = (32 + 2 * e + 2 * (c / 4) - h - c % 4) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let k = h + l - 7 * m + 114;
    let day = NaiveDate::from_ymd_opt(y, (k / 31) as u32, (k % 31 + 1) as u32).ok_or(err::NUM)?;
    let day = day + TimeDelta::days(offset);
    Ok(CellValue::Date(
        day.and_time(chrono::NaiveTime::MIN)
            .and_utc()
            .timestamp_millis(),
    ))
}

/// The Julian day number of Gregorian (y, m, d) by Fliegel and Van
/// Flandern's formula, with C's truncating division as Gnumeric has it:
/// out-of-range days and months roll over.
pub(super) fn jdn(y: f64, m: f64, d: f64) -> Result<i64, u8> {
    let ok = |v: f64| (-1e7..=1e7).contains(&v);
    if !(ok(y) && ok(m) && ok(d)) {
        return Err(err::NUM);
    }
    let (y, m, d) = (y as i64, m as i64, d as i64);
    let a = (m - 14) / 12;
    Ok((1461 * (y + 4800 + a)) / 4 + (367 * (m - 2 - 12 * a)) / 12
        - (3 * ((y + 4900 + a) / 100)) / 4
        + d
        - 32075)
}
