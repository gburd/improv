//! Evaluation of the date family (see `improv_core_model::functions`): the
//! information functions, text formatting and regular expressions
//! ([`text`]), and date and time.
//!
//! Dates are epoch milliseconds (UTC). Calendar functions work on whole
//! days: a date's time of day is dropped (Excel truncates serial dates the
//! same way). Day numbers are spreadsheet serial days, 0 = 1899-12-30.

use super::{
    d, date_plus_days, domain, err, n, n_or, ok_num, ok_text, serial, t, DAY_MS, R, SERIAL_EPOCH_MS,
};
use crate::CellValue;
use chrono::{Datelike, Months, NaiveDate, NaiveDateTime, TimeDelta};

mod text;

/// Evaluate date-family function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    match id {
        56..=79 => info(id, a),
        330..=399 => text::eval(id, a),
        _ => date_time(id, a),
    }
}

/// The information functions here that see error arguments: the ones that
/// ask about a value's kind (`ISNUMBER(#N/A)` is FALSE, `TYPE(#N/A)` 16),
/// and `VALUETOTEXT`, which spells an error out.
pub(super) fn inspects_errors(id: u32) -> bool {
    matches!(id, 56..=62 | 67 | 334)
}

fn info(id: u32, a: &[CellValue]) -> R {
    use CellValue as V;
    let v = a.first().ok_or(err::VALUE)?;
    Ok(match id {
        56 => V::Bool(false),
        // A date is a number, as in Excel (TYPE 1, N its serial).
        57 | 62 => V::Bool(matches!(v, V::Num(_) | V::Date(_))),
        58 => V::Bool(matches!(v, V::Text(_))),
        59 => V::Bool(!matches!(v, V::Text(_))),
        60 => V::Bool(matches!(v, V::Bool(_))),
        61 => V::Bool(matches!(v, V::Date(_))),
        63 | 64 => V::Bool((n(a, 0)?.trunc() % 2.0 == 0.0) == (id == 63)),
        65 => match v {
            V::Date(ms) => V::num(serial(*ms)),
            V::Text(_) => V::num(0.0),
            _ => V::num(n(a, 0)?),
        },
        66 => V::Text(match v {
            V::Text(s) => s.clone(),
            _ => String::new(),
        }),
        67 => V::num(match v {
            V::Num(_) | V::Date(_) => 1.0,
            V::Text(_) => 2.0,
            V::Bool(_) => 4.0,
            V::Err(_) => 16.0,
        }),
        _ => return Err(err::NAME),
    })
}

const DAY_NAMES: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// Epoch milliseconds as a UTC date-time (`#NUM!` out of range).
fn at(ms: i64) -> Result<NaiveDateTime, u8> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.naive_utc())
        .ok_or(err::NUM)
}

/// Date argument `i` as its calendar day.
fn day_arg(a: &[CellValue], i: usize) -> Result<NaiveDate, u8> {
    Ok(at(d(a, i)?)?.date())
}

/// Date argument `i` as a serial day number (the time of day dropped).
fn dn(a: &[CellValue], i: usize) -> Result<i64, u8> {
    Ok((d(a, i)? - SERIAL_EPOCH_MS).div_euclid(DAY_MS))
}

fn day_no(day: NaiveDate) -> i64 {
    (ms_of(day) - SERIAL_EPOCH_MS) / DAY_MS
}

fn ms_of(day: NaiveDate) -> i64 {
    day.and_time(chrono::NaiveTime::MIN)
        .and_utc()
        .timestamp_millis()
}

fn ok_date(day: NaiveDate) -> R {
    Ok(CellValue::Date(ms_of(day)))
}

/// The date of serial day `k`, or `#NUM!` out of range.
fn ok_dn(k: i64) -> R {
    match date_plus_days(SERIAL_EPOCH_MS, k as f64) {
        CellValue::Err(c) => Err(c),
        v => Ok(v),
    }
}

/// Monday = 0 … Sunday = 6 of serial day `k` (day 0 was a Saturday).
fn wd(k: i64) -> usize {
    (k + 5).rem_euclid(7) as usize
}

/// The first of the month `months` after January of year 0.
fn month_start(months: i64) -> Result<NaiveDate, u8> {
    let y = i32::try_from(months.div_euclid(12)).map_err(|_| err::NUM)?;
    NaiveDate::from_ymd_opt(y, months.rem_euclid(12) as u32 + 1, 1).ok_or(err::NUM)
}

fn months_of(day: NaiveDate) -> i64 {
    day.year() as i64 * 12 + day.month0() as i64
}

fn days_in_month(day: NaiveDate) -> Result<i64, u8> {
    let next = month_start(months_of(day) + 1)?;
    Ok(day_no(next) - day_no(day.with_day(1).ok_or(err::NUM)?))
}

fn is_leap(y: i32) -> bool {
    NaiveDate::from_ymd_opt(y, 2, 29).is_some()
}

/// A whole-number argument in `-limit..limit` (truncated), else `#NUM!`.
fn int(x: f64, limit: f64) -> Result<i64, u8> {
    let x = x.trunc();
    domain(x.abs() < limit)?;
    Ok(x as i64)
}

fn date_time(id: u32, a: &[CellValue]) -> R {
    match id {
        400 => date(n(a, 0)?, n(a, 1)?, n(a, 2)?),
        401 => ok_date(parse_date(&t(a, 0)?).ok_or(err::VALUE)?),
        402 => {
            let (h, m, s) = (n(a, 0)?.trunc(), n(a, 1)?.trunc(), n(a, 2)?.trunc());
            let secs = h * 3600.0 + m * 60.0 + s;
            domain(h < 32768.0 && m < 32768.0 && s < 32768.0 && secs >= 0.0)?;
            Ok(CellValue::Date(
                SERIAL_EPOCH_MS + (secs % 86400.0) as i64 * 1000,
            ))
        }
        403 => ok_num(parse_time(&t(a, 0)?).ok_or(err::VALUE)? as f64 / DAY_MS as f64),
        404 => ok_num(day_arg(a, 0)?.year() as f64),
        405 => ok_num(day_arg(a, 0)?.month() as f64),
        406 => ok_num(day_arg(a, 0)?.day() as f64),
        407..=409 => {
            // Rounded to the nearest second, but never into the next day.
            let s = ((d(a, 0)?.rem_euclid(DAY_MS) + 500) / 1000).min(86399);
            ok_num(match id {
                407 => s / 3600,
                408 => s / 60 % 60,
                _ => s % 60,
            } as f64)
        }
        410 => weekday(dn(a, 0)?, n_or(a, 1, 1.0)?),
        411 => weeknum(day_arg(a, 0)?, n_or(a, 1, 1.0)?),
        412 => ok_num(day_arg(a, 0)?.iso_week().week() as f64),
        413 => ok_num(day_arg(a, 0)?.iso_week().year() as f64),
        414 => ok_num(day_arg(a, 0)?.ordinal() as f64),
        415 => {
            let k = match a.first() {
                Some(CellValue::Date(_)) => (wd(dn(a, 0)?) + 1) % 7,
                _ => {
                    let k = n(a, 0)?.trunc();
                    domain((1.0..=7.0).contains(&k))?;
                    k as usize - 1
                }
            };
            ok_text(DAY_NAMES[k].into())
        }
        416 => {
            let k = n(a, 0)?.trunc();
            domain((1.0..=12.0).contains(&k))?;
            ok_text(MONTH_NAMES[k as usize - 1].into())
        }
        417 => ok_num((d(a, 0)? - d(a, 1)?) as f64 / DAY_MS as f64),
        418 => ok_num(days360(
            day_arg(a, 0)?,
            day_arg(a, 1)?,
            a.len() > 2 && super::b(a, 2)?,
        ) as f64),
        419 => datedif(day_arg(a, 0)?, day_arg(a, 1)?, &t(a, 2)?),
        420 => {
            let k = int(n(a, 1)?, 1e7)?;
            let day = day_arg(a, 0)?;
            let m = Months::new(k.unsigned_abs() as u32);
            let r = if k >= 0 {
                day.checked_add_months(m)
            } else {
                day.checked_sub_months(m)
            };
            ok_date(r.ok_or(err::NUM)?)
        }
        421 => {
            let k = int(n(a, 1)?, 1e7)?;
            let next = month_start(months_of(day_arg(a, 0)?) + k + 1)?;
            ok_date(next.pred_opt().ok_or(err::NUM)?)
        }
        422 => yearfrac(day_arg(a, 0)?, day_arg(a, 1)?, n_or(a, 2, 0.0)?),
        423 | 424 => {
            let wk = weekend(a, 2, err::NUM)?;
            let (s, e) = (dn(a, 0)?, dn(a, 1)?);
            let k = if s <= e {
                workdays(s, e, &wk)
            } else {
                -workdays(e, s, &wk)
            };
            ok_num(k as f64)
        }
        425 | 426 => workday(dn(a, 0)?, n(a, 1)?, &weekend(a, 2, err::VALUE)?),
        427 => Ok(CellValue::Date(chrono::Utc::now().timestamp_millis())),
        428 => {
            let now = chrono::Utc::now().timestamp_millis();
            Ok(CellValue::Date(now.div_euclid(DAY_MS) * DAY_MS))
        }
        429 => ok_num(days_in_month(day_arg(a, 0)?)? as f64),
        430 => ok_num(if is_leap(day_arg(a, 0)?.year()) {
            366.0
        } else {
            365.0
        }),
        431 => {
            let dec28 = NaiveDate::from_ymd_opt(day_arg(a, 0)?.year(), 12, 28).ok_or(err::NUM)?;
            ok_num(dec28.iso_week().week() as f64)
        }
        432 => Ok(CellValue::Bool(is_leap(day_arg(a, 0)?.year()))),
        433..=435 => periods(id, a),
        436 => {
            let ms = n(a, 0)? * 1000.0;
            domain(ms.abs() < 8.64e15)?;
            Ok(CellValue::Date(ms.round() as i64))
        }
        437 => ok_num((d(a, 0)? + 500).div_euclid(1000) as f64),
        438..=444 => {
            let x = n(a, 0)?;
            ok_num(match id {
                439 => x * 24.0,
                440 => x * 1440.0,
                441 => x * 86400.0,
                442 => x * 86_400_000.0,
                443 => x / 7.0,
                _ => x,
            })
        }
        _ => Err(err::NAME),
    }
}

/// `DATE(y, m, d)`: years 0–1899 add 1900; months and days roll over.
fn date(y: f64, m: f64, dd: f64) -> R {
    let y = y.trunc();
    domain((0.0..10000.0).contains(&y))?;
    let y = if y < 1900.0 { y + 1900.0 } else { y } as i64;
    let first = month_start(y * 12 + int(m, 1e9)? - 1)?;
    let days = TimeDelta::try_days(int(dd, 1e9)? - 1).ok_or(err::NUM)?;
    ok_date(first.checked_add_signed(days).ok_or(err::NUM)?)
}

/// `WEEKDAY(d, type)`: 1 Sunday=1, 2 Monday=1, 3 Monday=0, 11–17 day 1 is
/// Monday…Sunday.
fn weekday(k: i64, ty: f64) -> R {
    let w = wd(k) as i64;
    if ty.trunc() == 3.0 {
        return ok_num(w as f64);
    }
    let start = week_start(ty)?;
    ok_num(((w - start).rem_euclid(7) + 1) as f64)
}

/// The weekday (Monday = 0) a `WEEKDAY`/`WEEKNUM` type starts the week on.
fn week_start(ty: f64) -> Result<i64, u8> {
    match ty.trunc() as i64 {
        1 | 17 => Ok(6),
        2 | 11 => Ok(0),
        k @ 12..=16 => Ok(k - 11),
        _ => Err(err::NUM),
    }
}

/// `WEEKNUM(d, type)`: the week holding January 1 is week 1; type 21 is ISO.
fn weeknum(day: NaiveDate, ty: f64) -> R {
    if ty.trunc() == 21.0 {
        return ok_num(day.iso_week().week() as f64);
    }
    let start = week_start(ty)?;
    let jan1 = day.with_ordinal(1).ok_or(err::NUM)?;
    let off = (jan1.weekday().num_days_from_monday() as i64 - start).rem_euclid(7);
    ok_num(((day.ordinal0() as i64 + off) / 7 + 1) as f64)
}

/// `DAYS360`: the US (NASD) method as Excel and LibreOffice compute it, or
/// the European method.
fn days360(s: NaiveDate, e: NaiveDate, european: bool) -> i64 {
    let mut day1 = s.day() as i64;
    let mut day2 = e.day() as i64;
    let feb_end = s.month() == 2 && (day1 == 29 || (day1 == 28 && !is_leap(s.year())));
    if day1 == 31 || (!european && feb_end) {
        day1 = 30;
    }
    if day2 == 31 && (european || day1 == 30) {
        day2 = 30;
    }
    day2 + e.month() as i64 * 30 + e.year() as i64 * 360
        - day1
        - s.month() as i64 * 30
        - s.year() as i64 * 360
}

/// `DATEDIF(start, end, unit)`, with LibreOffice's (Excel-matching) "MD"
/// and "YD" roll-over.
fn datedif(s: NaiveDate, e: NaiveDate, unit: &str) -> R {
    domain(s <= e)?;
    let (y1, m1, d1) = (s.year() as i64, s.month() as i64, s.day() as i64);
    let (y2, m2, d2) = (e.year() as i64, e.month() as i64, e.day() as i64);
    let months = (y2 - y1) * 12 + m2 - m1 - i64::from(d1 > d2);
    // Year y, month m (1-based), day d1 of the start, rolling over.
    let rolled =
        |y: i64, m: i64| -> Result<i64, u8> { Ok(day_no(month_start(y * 12 + m - 1)?) + d1 - 1) };
    let v = match unit.to_ascii_uppercase().as_str() {
        "D" => day_no(e) - day_no(s),
        "M" => months,
        "Y" => months / 12,
        "YM" => months % 12,
        "MD" if d1 <= d2 => d2 - d1,
        "MD" => day_no(e) - rolled(y2, m2 - 1)?,
        "YD" => {
            let y = if (m2, d2) >= (m1, d1) { y2 } else { y2 - 1 };
            day_no(e) - rolled(y, m1)?
        }
        _ => return Err(err::NUM),
    };
    ok_num(v as f64)
}

/// `YEARFRAC(start, end, basis)`, as LibreOffice's analysis add-in (which
/// matches Excel) computes it.
fn yearfrac(d1: NaiveDate, d2: NaiveDate, basis: f64) -> R {
    let basis = basis.trunc();
    domain((0.0..=4.0).contains(&basis))?;
    let (s, e) = if d1 <= d2 { (d1, d2) } else { (d2, d1) };
    if s == e {
        return ok_num(0.0);
    }
    let (y1, m1, mut day1) = (s.year(), s.month() as i64, s.day() as i64);
    let (y2, m2, mut day2) = (e.year(), e.month() as i64, e.day() as i64);
    let last_feb = |y: i32, m: i64, day: i64| m == 2 && day == if is_leap(y) { 29 } else { 28 };
    let thirty =
        |day1: i64, day2: i64| ((y2 - y1) as i64 * 360 + (m2 - m1) * 30 + day2 - day1) as f64;
    let (diff, year) = match basis as i64 {
        0 => {
            if day1 == 31 {
                day1 -= 1;
            }
            if day1 == 30 && day2 == 31 {
                day2 -= 1;
            } else if last_feb(y1, m1, day1) {
                day1 = 30;
                if last_feb(y2, m2, day2) {
                    day2 = 30;
                }
            }
            (thirty(day1, day2), 360.0)
        }
        4 => (thirty(day1.min(30), day2.min(30)), 360.0),
        b => {
            let diff = (day_no(e) - day_no(s)) as f64;
            let year = match b {
                2 => 360.0,
                3 => 365.0,
                _ if y1 != y2 && (y2 != y1 + 1 || (m1, day1) < (m2, day2)) => {
                    let days: i64 = (y1..=y2).map(|y| if is_leap(y) { 366 } else { 365 }).sum();
                    days as f64 / (y2 - y1 + 1) as f64
                }
                _ => {
                    let leap = if y1 != y2 {
                        (is_leap(y1) && m1 < 3) || (is_leap(y2) && (m2, day2) >= (2, 29))
                    } else {
                        is_leap(y1)
                    };
                    if leap {
                        366.0
                    } else {
                        365.0
                    }
                }
            };
            (diff, year)
        }
    };
    ok_num(diff / year)
}

/// The weekend mask (Monday first) of `NETWORKDAYS.INTL`/`WORKDAY.INTL`
/// argument `i`: 1–7 two days (1 Saturday–Sunday), 11–17 one day (11
/// Sunday), or a 7-character "0000011" string; absent is Saturday–Sunday.
/// A bad string is `#VALUE!`, a bad number `bad` (LibreOffice gives
/// `#NUM!` in NETWORKDAYS.INTL and `#VALUE!` in WORKDAY.INTL).
fn weekend(a: &[CellValue], i: usize, bad: u8) -> Result<[bool; 7], u8> {
    let k = match a.get(i) {
        None => 1,
        Some(CellValue::Text(s)) => {
            let b = s.as_bytes();
            if b.len() != 7 || !b.iter().all(|c| matches!(c, b'0' | b'1')) {
                return Err(err::VALUE);
            }
            return Ok(std::array::from_fn(|j| b[j] == b'1'));
        }
        Some(CellValue::Num(_)) => n(a, i)?.trunc() as i64,
        Some(_) => return Err(err::VALUE),
    };
    let mut wk = [false; 7];
    match k {
        1..=7 => {
            wk[((k + 4) % 7) as usize] = true;
            wk[((k + 5) % 7) as usize] = true;
        }
        11..=17 => wk[((k - 5) % 7) as usize] = true,
        _ => return Err(bad),
    }
    Ok(wk)
}

/// Working days in serial days `s..=e` (`s <= e`).
fn workdays(s: i64, e: i64, wk: &[bool; 7]) -> i64 {
    let per_week = wk.iter().filter(|w| !**w).count() as i64;
    let total = e - s + 1;
    let tail = s + total / 7 * 7;
    total / 7 * per_week + (tail..=e).filter(|k| !wk[wd(*k)]).count() as i64
}

/// `WORKDAY`: `days` working days after (before, when negative) `start`.
fn workday(start: i64, days: f64, wk: &[bool; 7]) -> R {
    let per_week = wk.iter().filter(|w| !**w).count() as i64;
    if per_week == 0 {
        return Err(err::VALUE);
    }
    let days = int(days, 1e9)?;
    let step = days.signum();
    let mut left = days.abs();
    let mut cur = start;
    if left > 0 {
        // Each 7 days hold exactly per_week working days.
        let weeks = (left - 1) / per_week;
        cur += step * 7 * weeks;
        left -= weeks * per_week;
        while left > 0 {
            cur += step;
            if !wk[wd(cur)] {
                left -= 1;
            }
        }
    }
    ok_dn(cur)
}

/// LibreOffice's `MONTHS`/`WEEKS`/`YEARS(start, end, type)`: type 0 counts
/// whole periods, 1 calendar periods (month, Monday-week or year boundaries
/// crossed).
fn periods(id: u32, a: &[CellValue]) -> R {
    let ty = n(a, 2)?.trunc();
    if ty != 0.0 && ty != 1.0 {
        return Err(err::NUM);
    }
    let cal = ty == 1.0;
    let (s, e) = (day_arg(a, 0)?, day_arg(a, 1)?);
    let (k1, k2) = (day_no(s), day_no(e));
    let months = || {
        let m = months_of(e) - months_of(s);
        if cal || k1 == k2 {
            m
        } else if k1 < k2 {
            m - i64::from(s.day() > e.day())
        } else {
            m + i64::from(s.day() < e.day())
        }
    };
    let v = match id {
        433 => months(),
        434 if cal => ((k2 - wd(k2) as i64) - (k1 - wd(k1) as i64)) / 7,
        434 => (k2 - k1) / 7,
        _ if cal => (e.year() - s.year()) as i64,
        _ => months() / 12,
    };
    ok_num(v as f64)
}

/// `DATEVALUE` text: ISO `2025-01-31` (a `T` time part is dropped) or US
/// `1/31/2025`.
fn parse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    if let Ok(day) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Some(day);
    }
    if let Some(ms) = parse_iso_datetime(s) {
        return at(ms).ok().map(|t| t.date());
    }
    let p: Vec<&str> = s.split('/').collect();
    let digits = |x: &str, w: std::ops::RangeInclusive<usize>| {
        w.contains(&x.len()) && x.bytes().all(|c| c.is_ascii_digit())
    };
    if p.len() == 3 && digits(p[0], 1..=2) && digits(p[1], 1..=2) && digits(p[2], 4..=4) {
        return NaiveDate::from_ymd_opt(p[2].parse().ok()?, p[0].parse().ok()?, p[1].parse().ok()?);
    }
    None
}

/// `2025-01-31T09:30:00Z` (any RFC 3339 offset) or without a zone, UTC.
fn parse_iso_datetime(s: &str) -> Option<i64> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_millis());
    }
    ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"]
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
        .map(|t| t.and_utc().timestamp_millis())
}

/// `TIMEVALUE` text as milliseconds into the day: `09:30`, `9:30:15`,
/// `9:30 PM`, or the time of an ISO date-time.
fn parse_time(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(ms) = parse_iso_datetime(s) {
        return Some(ms.rem_euclid(DAY_MS));
    }
    let up = s.to_ascii_uppercase();
    let (clock, pm) = match (up.strip_suffix("AM"), up.strip_suffix("PM")) {
        (Some(c), _) => (c.trim_end(), Some(false)),
        (_, Some(c)) => (c.trim_end(), Some(true)),
        _ => (up.as_str(), None),
    };
    let parts: Vec<&str> = clock.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut f = parts
        .iter()
        .map(|p| (1..=2).contains(&p.len()) && p.bytes().all(|c| c.is_ascii_digit()));
    if !f.all(|ok| ok) {
        return None;
    }
    let v: Vec<i64> = parts.iter().map(|p| p.parse().unwrap_or(99)).collect();
    let (h, m, sec) = (v[0], v[1], v.get(2).copied().unwrap_or(0));
    if m > 59 || sec > 59 {
        return None;
    }
    let h = match pm {
        None if h < 24 => h,
        Some(pm) if (1..=12).contains(&h) => h % 12 + if pm { 12 } else { 0 },
        _ => return None,
    };
    Some(((h * 60 + m) * 60 + sec) * 1000)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::super::eval as call;
    use super::*;
    use improv_core_model::FuncId;

    fn ev(id: u32, a: &[CellValue]) -> CellValue {
        call(FuncId(id), a)
    }
    fn num(x: f64) -> CellValue {
        CellValue::num(x)
    }
    fn s(x: &str) -> CellValue {
        CellValue::Text(x.into())
    }
    fn ymd(y: i32, m: u32, dd: u32) -> CellValue {
        CellValue::Date(ms_of(NaiveDate::from_ymd_opt(y, m, dd).unwrap()))
    }

    #[test]
    fn information_sees_kinds_and_errors() {
        let na = CellValue::Err(err::NA);
        let date = ymd(2025, 1, 1);
        assert_eq!(ev(56, &[s("")]), CellValue::Bool(false));
        assert_eq!(ev(57, std::slice::from_ref(&na)), CellValue::Bool(false));
        assert_eq!(ev(57, std::slice::from_ref(&date)), CellValue::Bool(true));
        assert_eq!(ev(59, std::slice::from_ref(&na)), CellValue::Bool(true));
        assert_eq!(ev(61, std::slice::from_ref(&date)), CellValue::Bool(true));
        assert_eq!(ev(61, &[num(45658.0)]), CellValue::Bool(false));
        assert_eq!(ev(62, std::slice::from_ref(&date)), CellValue::Bool(true));
        assert_eq!(ev(65, std::slice::from_ref(&date)), num(45658.0));
        assert_eq!(ev(65, &[CellValue::Bool(true)]), num(1.0));
        assert_eq!(ev(65, std::slice::from_ref(&na)), na);
        assert_eq!(ev(66, std::slice::from_ref(&na)), na);
        assert_eq!(ev(67, &[date]), num(1.0));
        assert_eq!(ev(67, &[na]), num(16.0));
        assert_eq!(ev(63, &[num(-3.0)]), CellValue::Bool(false));
        assert_eq!(ev(64, &[num(-3.0)]), CellValue::Bool(true));
        assert_eq!(info(68, &[num(1.0)]), Err(err::NAME));
        assert_eq!(info(56, &[]), Err(err::VALUE));
        assert!(!inspects_errors(63));
    }

    #[test]
    fn calendar_edges() {
        // DATE rolls over and adds 1900 to small years; out of range is #NUM!.
        assert_eq!(
            ev(400, &[num(2025.0), num(14.0), num(1.0)]),
            ymd(2026, 2, 1)
        );
        assert_eq!(
            ev(400, &[num(2025.0), num(-1.0), num(0.0)]),
            ymd(2024, 10, 31)
        );
        assert_eq!(ev(400, &[num(108.0), num(1.0), num(2.0)]), ymd(2008, 1, 2));
        assert_eq!(
            ev(400, &[num(-1.0), num(1.0), num(1.0)]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(
            ev(400, &[num(2025.0), num(1e12), num(1.0)]),
            CellValue::Err(err::NUM)
        );
        // TIME wraps at 24 hours; a negative total is #NUM!.
        let noon = CellValue::Date(SERIAL_EPOCH_MS + DAY_MS / 2);
        assert_eq!(ev(402, &[num(36.0), num(0.0), num(0.0)]), noon);
        assert_eq!(
            ev(402, &[num(0.0), num(-1.0), num(0.0)]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(
            ev(402, &[num(40000.0), num(0.0), num(0.0)]),
            CellValue::Err(err::NUM)
        );
        // HOUR/MINUTE/SECOND round to the second.
        let t = CellValue::Date(SERIAL_EPOCH_MS + DAY_MS - 400);
        assert_eq!(ev(407, std::slice::from_ref(&t)), num(23.0));
        assert_eq!(
            ev(409, &[CellValue::Date(SERIAL_EPOCH_MS + 61_600)]),
            num(2.0)
        );
        assert_eq!(
            ev(408, &[CellValue::Date(SERIAL_EPOCH_MS + 61_600)]),
            num(1.0)
        );
        // WEEKDAY / WEEKNUM types.
        let sun = ymd(2025, 1, 5);
        for (ty, want) in [
            (1.0, 1.0),
            (2.0, 7.0),
            (3.0, 6.0),
            (11.0, 7.0),
            (16.0, 2.0),
            (17.0, 1.0),
        ] {
            assert_eq!(ev(410, &[sun.clone(), num(ty)]), num(want), "type {ty}");
        }
        assert_eq!(ev(410, &[sun.clone(), num(4.0)]), CellValue::Err(err::NUM));
        assert_eq!(ev(411, &[sun.clone(), num(21.0)]), num(1.0));
        assert_eq!(ev(411, &[sun.clone(), num(3.0)]), CellValue::Err(err::NUM));
        assert_eq!(ev(413, &[ymd(2024, 12, 30)]), num(2025.0));
        // DAYNAME of a date and of a day number; MONTHNAME.
        assert_eq!(ev(415, std::slice::from_ref(&sun)), s("Sunday"));
        assert_eq!(ev(415, &[ymd(2025, 1, 4)]), s("Saturday"));
        assert_eq!(ev(415, &[num(7.0)]), s("Saturday"));
        assert_eq!(ev(415, &[num(8.0)]), CellValue::Err(err::NUM));
        assert_eq!(ev(416, &[num(12.0)]), s("December"));
        assert_eq!(ev(416, &[num(0.0)]), CellValue::Err(err::NUM));
        // DAYS keeps the time of day (end − start, as Excel documents).
        assert_eq!(ev(417, &[num(10.5), num(9.0)]), num(1.5));
        // EDATE back; EOMONTH out of range.
        assert_eq!(ev(420, &[ymd(2025, 3, 31), num(-1.0)]), ymd(2025, 2, 28));
        assert_eq!(
            ev(421, &[ymd(2025, 3, 31), num(1e8)]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(
            ev(420, &[ymd(2025, 3, 31), num(5e6)]),
            CellValue::Err(err::NUM)
        );
    }

    #[test]
    fn day_counts() {
        let (a, b) = (ymd(2024, 1, 31), ymd(2024, 2, 29));
        assert_eq!(
            ev(418, &[b.clone(), a.clone(), CellValue::Bool(true)]),
            num(-29.0)
        );
        assert_eq!(
            ev(419, &[a.clone(), b.clone(), s("x")]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(
            ev(419, &[b.clone(), a.clone(), s("d")]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(
            ev(422, &[a.clone(), b.clone(), num(5.0)]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(ev(422, &[a.clone(), a.clone()]), num(0.0));
        // Basis 1 within a year that is not a leap year, and across two.
        let f = ev(422, &[ymd(2023, 3, 1), ymd(2023, 9, 1), num(1.0)]);
        assert_eq!(f, num(184.0 / 365.0));
        let f = ev(422, &[ymd(2023, 3, 1), ymd(2024, 3, 1), num(1.0)]);
        assert_eq!(f, num(366.0 / 366.0));
        // Weekend masks.
        let (s1, e1) = (ymd(2025, 1, 1), ymd(2025, 1, 31));
        assert_eq!(ev(424, &[s1.clone(), e1.clone(), s("0000000")]), num(31.0));
        assert_eq!(ev(424, &[s1.clone(), e1.clone(), s("1111111")]), num(0.0));
        assert_eq!(
            ev(424, &[s1.clone(), e1.clone(), s("000001")]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(
            ev(424, &[s1.clone(), e1.clone(), num(8.0)]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(
            ev(424, &[s1.clone(), e1.clone(), CellValue::Bool(true)]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(ev(423, &[e1.clone(), s1.clone()]), num(-23.0));
        assert_eq!(
            ev(426, &[s1.clone(), num(3.0), s("1111111")]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(ev(425, &[s1.clone(), num(0.0)]), s1);
        assert_eq!(ev(425, &[ymd(2025, 1, 6), num(-1.0)]), ymd(2025, 1, 3));
        assert_eq!(ev(425, &[s1.clone(), num(1e10)]), CellValue::Err(err::NUM));
        assert_eq!(ev(425, &[s1, num(9e8)]), CellValue::Err(err::NUM));
        // MONTHS/WEEKS/YEARS: bad type, equal dates and backwards.
        let (x, y) = (ymd(2025, 3, 31), ymd(2025, 1, 15));
        assert_eq!(
            ev(433, &[x.clone(), y.clone(), num(2.0)]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(ev(433, &[x.clone(), x.clone(), num(0.0)]), num(0.0));
        assert_eq!(ev(433, &[x.clone(), y.clone(), num(0.0)]), num(-2.0));
        assert_eq!(ev(433, &[y.clone(), x.clone(), num(0.0)]), num(2.0));
        assert_eq!(ev(435, &[y, x, num(1.0)]), num(0.0));
    }

    #[test]
    fn now_today_and_unix() {
        let now = chrono::Utc::now().timestamp_millis();
        let CellValue::Date(t) = ev(427, &[]) else {
            panic!("NOW is a date")
        };
        assert!((t - now).abs() < 60_000);
        let CellValue::Date(t) = ev(428, &[]) else {
            panic!("TODAY is a date")
        };
        assert_eq!(t.rem_euclid(DAY_MS), 0);
        assert!(now - t < DAY_MS + 60_000);
        assert_eq!(ev(436, &[num(1.5)]), CellValue::Date(1500));
        assert_eq!(ev(436, &[num(1e300)]), CellValue::Err(err::NUM));
        assert_eq!(ev(437, &[CellValue::Date(-1500)]), num(-1.0));
        assert_eq!(date_time(499, &[]), Err(err::NAME));
        assert_eq!(ok_dn(i64::MAX / DAY_MS), Err(err::NUM));
    }

    #[test]
    fn parsing() {
        let day = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd);
        assert_eq!(parse_date("2025-01-31"), day(2025, 1, 31));
        assert_eq!(parse_date(" 1/31/2025 "), day(2025, 1, 31));
        assert_eq!(parse_date("2025-01-31T23:30"), day(2025, 1, 31));
        assert_eq!(parse_date("2025-01-31T23:30:00-05:00"), day(2025, 2, 1));
        for bad in ["1/31/25", "2/30/2025", "31.1.2025", "", "a/b/2025"] {
            assert_eq!(parse_date(bad), None, "{bad}");
        }
        let h = |h: i64, m: i64| Some((h * 60 + m) * 60_000);
        assert_eq!(parse_time("09:30"), h(9, 30));
        assert_eq!(parse_time("9:30 pm"), h(21, 30));
        assert_eq!(parse_time("12:05AM"), h(0, 5));
        assert_eq!(parse_time("9:30:15"), Some(h(9, 30).unwrap() + 15_000));
        assert_eq!(parse_time("2025-01-31T09:30:00Z"), h(9, 30));
        for bad in [
            "24:00", "13:00 PM", "9:60", "9:30:60", "9", "9:3a", "1:2:3:4", "123:00",
        ] {
            assert_eq!(parse_time(bad), None, "{bad}");
        }
    }
}
