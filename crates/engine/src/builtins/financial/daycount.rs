//! Day counting for the bond and depreciation functions, ported from
//! LibreOffice's `scaddins/source/analysis/analysishelper.cxx` (`ScaDate`,
//! `GetDiffDate`, `GetYearFrac`, `GetDiffDate360` and the `COUP*` helpers)
//! so results match LibreOffice bit for bit.
//!
//! Dates are whole days since 1970-01-01. `basis`: 0 US (NASD) 30/360,
//! 1 actual/actual, 2 actual/360, 3 actual/365, 4 European 30/360.
//!
//! LibreOffice has two year fractions and the functions pick one each:
//! [`year_diff`] (`GetYearDiff`: ACCRINT, ACCRINTM, INTRATE, PRICEDISC,
//! RECEIVED) and [`year_frac`] (`YEARFRAC`: DISC, DURATION, PRICEMAT,
//! YIELDDISC, YIELDMAT, ODDL*, AMOR*). Their 30/360 rules differ:
//! `year_diff` counts raw day differences (with a February fix-up only
//! inside one year); `year_frac` applies the ODFF end-of-month rules, where a
//! start on the last day of February counts as the 30th but an end on the
//! 31st after such a start is not moved, as in Excel's YEARFRAC.

/// (year, month, day) of day `z` (days since 1970-01-01; proleptic
/// Gregorian, H. Hinnant's `civil_from_days`).
pub(super) fn ymd(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The day number of a valid (year, month, day).
pub(super) fn days(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = i64::from(if m > 2 { m - 3 } else { m + 9 });
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

pub(super) fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

pub(super) fn month_days(m: u32, y: i64) -> u32 {
    match m {
        2 if is_leap(y) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `z` plus `k` months, the day clamped to the target month's end (GLib's
/// `g_date_add_months`, Excel's EDATE).
pub(super) fn add_months(z: i64, k: i64) -> i64 {
    let (y, m, d) = ymd(z);
    let t = y * 12 + i64::from(m) - 1 + k;
    let (y, m) = (t.div_euclid(12), t.rem_euclid(12) as u32 + 1);
    days(y, m, d.min(month_days(m, y)))
}

/// `GetYearDiff`: `GetDiffDate`'s day count over the days in the start
/// date's year.
pub(super) fn year_diff(a: i64, b: i64, basis: u32) -> f64 {
    let (lo, hi) = (a.min(b), a.max(b));
    let (n, year) = match basis {
        0 | 4 => {
            let ((y1, m1, d1), (y2, m2, d2)) = (ymd(lo), ymd(hi));
            let mut n = ((y2 - y1) * 12 + i64::from(m2) - i64::from(m1)) * 30 + i64::from(d2)
                - i64::from(d1);
            if basis == 0 && m1 == 2 && m2 != 2 && y1 == y2 {
                n -= if is_leap(y1) { 1 } else { 2 };
            }
            (n, 360.0)
        }
        1 => (hi - lo, if is_leap(ymd(lo).0) { 366.0 } else { 365.0 }),
        2 => (hi - lo, 360.0),
        _ => (hi - lo, 365.0),
    };
    let n = if a > b { -n } else { n };
    n as f64 / year
}

/// `GetYearFrac` (the YEARFRAC function, ODFF 1.2 §4.11.7): never negative.
pub(super) fn year_frac(a: i64, b: i64, basis: u32) -> f64 {
    if a == b {
        return 0.0;
    }
    let (a, b) = (a.min(b), a.max(b));
    let ((y1, m1, mut d1), (y2, m2, mut d2)) = (ymd(a), ymd(b));
    let feb_end = |y: i64, m: u32, d: u32| m == 2 && d == month_days(2, y);
    let diff = match basis {
        0 | 4 => {
            if d1 == 31 {
                d1 = 30;
            }
            if basis == 4 {
                if d2 == 31 {
                    d2 = 30;
                }
            } else if d1 == 30 && d2 == 31 {
                d2 = 30;
            } else if feb_end(y1, m1, d1) {
                d1 = 30;
                if feb_end(y2, m2, d2) {
                    d2 = 30;
                }
            }
            (y2 - y1) * 360 + (i64::from(m2) - i64::from(m1)) * 30 + i64::from(d2) - i64::from(d1)
        }
        _ => b - a,
    };
    let year = match basis {
        1 => {
            let span = y1 != y2 && (y2 != y1 + 1 || m1 < m2 || (m1 == m2 && d1 < d2));
            if span {
                // The average year length over y1..=y2.
                let total: i64 = (y1..=y2).map(|y| if is_leap(y) { 366 } else { 365 }).sum();
                total as f64 / (y2 - y1 + 1) as f64
            } else if (y1 == y2 && is_leap(y1))
                || (y1 != y2
                    // (m1 ≤ 2: any February day is ≤ 29.)
                    && ((is_leap(y1) && m1 <= 2)
                        || (is_leap(y2) && (m2 > 2 || (m2 == 2 && d2 == 29)))))
            {
                366.0
            } else {
                365.0
            }
        }
        3 => 365.0,
        _ => 360.0,
    };
    diff as f64 / year
}

/// `GetDiffDate360` (DAYS360): US (NASD) method when `us`, else European.
pub(super) fn diff360(a: i64, b: i64, us: bool) -> i64 {
    let ((y1, m1, mut d1), (mut y2, mut m2, mut d2)) = (ymd(a), ymd(b));
    if d1 == 31 || (us && m1 == 2 && (d1 == 29 || (d1 == 28 && !is_leap(y1)))) {
        d1 = 30;
    }
    if d2 == 31 {
        if us && d1 != 30 {
            d2 = 1;
            if m2 == 12 {
                y2 += 1;
                m2 = 1;
            } else {
                m2 += 1;
            }
        } else {
            d2 = 30;
        }
    }
    i64::from(d2) + i64::from(m2) * 30 + y2 * 360 - i64::from(d1) - i64::from(m1) * 30 - y1 * 360
}

/// LibreOffice's `ScaDate`: a date that remembers whether it was the last day
/// of its month, so adding months keeps month ends at month ends, and that
/// counts 30-day months in the 30/360 bases.
#[derive(Debug, Clone, Copy)]
pub(super) struct ScaDate {
    orig_day: u32,
    day: u32,
    month: u32,
    year: i64,
    last_day: bool,
    d30: bool,
    us: bool,
}

impl ScaDate {
    pub(super) fn new(date: i64, basis: u32) -> Self {
        let (year, month, orig_day) = ymd(date);
        let mut s = ScaDate {
            orig_day,
            day: orig_day,
            month,
            year,
            last_day: orig_day >= month_days(month, year),
            d30: basis == 0 || basis == 4,
            us: basis == 0,
        };
        s.set_day();
        s
    }

    fn set_day(&mut self) {
        let last = month_days(self.month, self.year);
        if self.d30 {
            self.day = self.orig_day.min(30);
            if self.last_day || self.day >= last {
                self.day = 30;
            }
        } else {
            self.day = if self.last_day {
                last
            } else {
                self.orig_day.min(last)
            };
        }
    }

    fn month_len(&self, m: u32) -> i64 {
        if self.d30 {
            30
        } else {
            i64::from(month_days(m, self.year))
        }
    }

    pub(super) fn add_months(&mut self, k: i64) {
        let total = self.year * 12 + i64::from(self.month) - 1 + k;
        self.year = total.div_euclid(12);
        self.month = total.rem_euclid(12) as u32 + 1;
        self.set_day();
    }

    pub(super) fn set_year(&mut self, y: i64) {
        self.year = y;
        self.set_day();
    }

    pub(super) fn year(&self) -> i64 {
        self.year
    }

    pub(super) fn month(&self) -> u32 {
        self.month
    }

    /// The actual calendar day this stands for.
    pub(super) fn date(&self) -> i64 {
        let last = month_days(self.month, self.year);
        let d = if self.last_day {
            last
        } else {
            last.min(self.orig_day)
        };
        days(self.year, self.month, d)
    }

    fn key(&self) -> (i64, u32, u32) {
        (self.year, self.month, self.day)
    }

    /// `ScaDate::operator<`.
    pub(super) fn lt(&self, o: &ScaDate) -> bool {
        if self.key() != o.key() {
            return self.key() < o.key();
        }
        if self.last_day || o.last_day {
            return !self.last_day && o.last_day;
        }
        self.orig_day < o.orig_day
    }

    /// `ScaDate::getDiff`: days from `from` to `to` in this basis.
    pub(super) fn diff(from: &ScaDate, to: &ScaDate) -> i64 {
        if to.lt(from) {
            return Self::diff(to, from);
        }
        let (mut a, mut b) = (*from, *to);
        if to.d30 {
            if to.us {
                if (from.month == 2 || from.day < 30) && b.orig_day == 31 {
                    b.day = 31;
                } else if b.month == 2 && b.last_day {
                    b.day = month_days(2, b.year);
                }
            } else {
                if a.month == 2 && a.day == 30 {
                    a.day = month_days(2, a.year);
                }
                if b.month == 2 && b.day == 30 {
                    b.day = month_days(2, b.year);
                }
            }
        }
        let mut n = 0i64;
        if (a.year, a.month) < (b.year, b.month) {
            // To the 1st of the next month, then whole years, then months.
            n = a.month_len(a.month) - i64::from(a.day) + 1;
            a.orig_day = 1;
            a.day = 1;
            a.last_day = false;
            a.add_months(1);
            if a.year < b.year {
                n += (a.month..=12).map(|m| a.month_len(m)).sum::<i64>();
                a.add_months(i64::from(13 - a.month));
                n += (a.year..b.year)
                    .map(|y| {
                        if a.d30 {
                            360
                        } else if is_leap(y) {
                            366
                        } else {
                            365
                        }
                    })
                    .sum::<i64>();
                a.set_year(b.year);
            }
            n += (a.month..b.month).map(|m| a.month_len(m)).sum::<i64>();
            a.add_months(i64::from(b.month) - i64::from(a.month));
        }
        n += i64::from(b.day) - i64::from(a.day);
        n.max(0)
    }
}

/// The coupon date on or before `settle` (maturity's schedule).
pub(super) fn pcd(settle: i64, mat: i64, freq: u32, basis: u32) -> ScaDate {
    let s = ScaDate::new(settle, basis);
    let mut d = ScaDate::new(mat, basis);
    d.set_year(s.year);
    if d.lt(&s) {
        d.set_year(d.year + 1);
    }
    while s.lt(&d) {
        d.add_months(-12 / i64::from(freq));
    }
    d
}

/// The first coupon date after `settle`.
pub(super) fn ncd(settle: i64, mat: i64, freq: u32, basis: u32) -> ScaDate {
    let s = ScaDate::new(settle, basis);
    let mut d = ScaDate::new(mat, basis);
    d.set_year(s.year);
    if s.lt(&d) {
        d.set_year(d.year - 1);
    }
    while !s.lt(&d) {
        d.add_months(12 / i64::from(freq));
    }
    d
}

/// COUPDAYBS: days from the previous coupon to settlement.
pub(super) fn coupdaybs(settle: i64, mat: i64, freq: u32, basis: u32) -> f64 {
    let p = pcd(settle, mat, freq, basis);
    ScaDate::diff(&p, &ScaDate::new(settle, basis)) as f64
}

/// COUPDAYS: days in the coupon period holding settlement.
pub(super) fn coupdays(settle: i64, mat: i64, freq: u32, basis: u32) -> f64 {
    match basis {
        1 => {
            let p = pcd(settle, mat, freq, basis);
            let mut q = p;
            q.add_months(12 / i64::from(freq));
            ScaDate::diff(&p, &q) as f64
        }
        3 => 365.0 / f64::from(freq),
        _ => 360.0 / f64::from(freq),
    }
}

/// COUPDAYSNC: days from settlement to the next coupon.
pub(super) fn coupdaysnc(settle: i64, mat: i64, freq: u32, basis: u32) -> f64 {
    if basis != 0 && basis != 4 {
        let n = ncd(settle, mat, freq, basis);
        return ScaDate::diff(&ScaDate::new(settle, basis), &n) as f64;
    }
    coupdays(settle, mat, freq, basis) - coupdaybs(settle, mat, freq, basis)
}

/// COUPNUM: coupons payable after settlement up to maturity.
pub(super) fn coupnum(settle: i64, mat: i64, freq: u32, basis: u32) -> f64 {
    let m = ScaDate::new(mat, basis);
    let p = pcd(settle, mat, freq, basis);
    let months = (m.year() - p.year()) * 12 + i64::from(m.month()) - i64::from(p.month());
    (months * i64::from(freq) / 12) as f64
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        for z in [-800_000, -25_569, -1, 0, 59, 11_016, 19_000, 2_932_896] {
            let (y, m, d) = ymd(z);
            assert_eq!(days(y, m, d), z, "{z} -> {y}-{m}-{d}");
        }
        assert_eq!(ymd(0), (1970, 1, 1));
        assert_eq!(ymd(days(2024, 2, 29)), (2024, 2, 29));
        assert!(is_leap(2000) && !is_leap(1900) && is_leap(2024) && !is_leap(2023));
    }

    #[test]
    fn thirty_360_month_ends() {
        let d = |y, m, dd| days(y, m, dd);
        // US: Feb 28 (non-leap) start counts as the 30th, so the 31st too.
        assert_eq!(diff360(d(2023, 2, 28), d(2023, 3, 31), true), 30);
        assert_eq!(diff360(d(2023, 2, 28), d(2023, 3, 31), false), 32);
        assert_eq!(diff360(d(2024, 1, 31), d(2024, 12, 31), true), 330);
        // year_frac basis 0: Feb end -> 30, but the later 31st is kept.
        assert_eq!(year_frac(d(2023, 2, 28), d(2023, 3, 31), 0), 31.0 / 360.0);
        assert_eq!(year_frac(d(2023, 2, 28), d(2024, 2, 29), 0), 1.0);
        assert_eq!(year_frac(d(2023, 1, 31), d(2023, 3, 31), 4), 60.0 / 360.0);
        assert_eq!(year_frac(d(2024, 1, 1), d(2024, 1, 1), 1), 0.0);
        // year_diff keeps raw day differences and is signed.
        assert_eq!(year_diff(d(2023, 1, 31), d(2023, 3, 1), 0), 30.0 / 360.0);
        assert_eq!(year_diff(d(2023, 3, 1), d(2023, 1, 31), 0), -30.0 / 360.0);
        assert_eq!(year_diff(d(2023, 2, 1), d(2023, 3, 1), 0), 28.0 / 360.0);
        assert_eq!(year_diff(d(2024, 2, 1), d(2024, 3, 1), 0), 29.0 / 360.0);
    }

    /// LibreOffice 25.8's YEARFRAC and DAYS360 on the same dates.
    #[test]
    fn match_libreoffice_yearfrac_and_days360() {
        let d = |y, m, dd| days(y, m, dd);
        let near = |a: f64, b: f64| assert!((a - b).abs() < 1e-15, "{a} != {b}");
        near(
            year_frac(d(2008, 1, 15), d(2010, 3, 1), 1),
            2.124_087_591_240_876,
        );
        near(
            year_frac(d(2008, 2, 1), d(2009, 1, 15), 1),
            0.953_551_912_568_306,
        );
        near(
            year_frac(d(2007, 3, 1), d(2008, 2, 29), 1),
            0.997_267_759_562_841_5,
        );
        near(
            year_frac(d(2007, 3, 1), d(2008, 2, 28), 1),
            0.997_260_273_972_602_8,
        );
        near(
            year_frac(d(2009, 3, 1), d(2008, 2, 28), 1),
            1.004_103_967_168_262_6,
        );
        near(year_frac(d(2007, 1, 30), d(2007, 3, 31), 0), 1.0 / 6.0);
        near(year_frac(d(2007, 1, 31), d(2007, 3, 31), 3), 59.0 / 365.0);
        near(year_frac(d(2007, 1, 31), d(2007, 3, 31), 2), 59.0 / 360.0);
        near(
            year_frac(d(2007, 3, 1), d(2007, 6, 1), 1),
            0.252_054_794_520_547_96,
        );
        near(
            year_frac(d(2008, 3, 1), d(2009, 2, 1), 1),
            0.923_287_671_232_876_7,
        );
        near(
            year_frac(d(2008, 2, 15), d(2009, 2, 1), 1),
            0.961_748_633_879_781_4,
        );
        near(
            year_frac(d(2008, 1, 15), d(2009, 1, 1), 1),
            0.961_748_633_879_781_4,
        );
        near(
            year_frac(d(2011, 3, 1), d(2012, 2, 15), 1),
            0.961_643_835_616_438_4,
        );
        assert_eq!(diff360(d(2008, 2, 28), d(2008, 3, 31), true), 33);
        assert_eq!(diff360(d(2007, 2, 27), d(2007, 3, 31), true), 34);
        assert_eq!(diff360(d(2008, 1, 15), d(2008, 12, 31), true), 346);
        assert_eq!(diff360(d(2008, 2, 29), d(2008, 3, 31), true), 30);
        assert_eq!(diff360(d(2008, 1, 15), d(2008, 3, 31), false), 75);
        assert_eq!(year_diff(d(2008, 1, 31), d(2008, 3, 1), 1), 30.0 / 366.0);
        assert_eq!(year_diff(d(2008, 1, 31), d(2008, 3, 1), 2), 30.0 / 360.0);
        assert_eq!(year_diff(d(2008, 1, 31), d(2008, 3, 1), 3), 30.0 / 365.0);
        assert_eq!(year_diff(d(2008, 2, 10), d(2008, 3, 10), 4), 30.0 / 360.0);
    }

    #[test]
    fn sca_date_ordering_and_corrections() {
        let s = |y, m, dd, b| ScaDate::new(days(y, m, dd), b);
        // Feb 28 and Feb 29 both count as day 30 in 30/360; the month end
        // sorts last.
        assert!(s(2007, 2, 27, 0).lt(&s(2007, 2, 28, 0)));
        assert!(!s(2007, 2, 28, 0).lt(&s(2007, 2, 28, 0)));
        let mut a = s(2008, 1, 30, 0);
        a.add_months(1);
        assert!(a.lt(&s(2008, 2, 29, 0)) && !s(2008, 2, 29, 0).lt(&a));
        let mut a = s(2008, 1, 30, 1);
        a.add_months(1);
        let mut b = s(2008, 1, 29, 1);
        b.add_months(1);
        assert!(b.lt(&a), "same day, ordered by the original day");
        // US: a 31st after a 30th is not moved; after a February end it is.
        assert_eq!(ScaDate::diff(&s(2008, 3, 30, 0), &s(2008, 5, 31, 0)), 60);
        assert_eq!(ScaDate::diff(&s(2008, 2, 29, 0), &s(2008, 3, 31, 0)), 31);
        assert_eq!(ScaDate::diff(&s(2008, 1, 15, 0), &s(2009, 2, 28, 0)), 403);
        // Over several years, actual and 30-day years.
        assert_eq!(ScaDate::diff(&s(2007, 6, 1, 1), &s(2010, 6, 1, 1)), 1096);
        assert_eq!(ScaDate::diff(&s(2007, 6, 1, 4), &s(2010, 6, 1, 4)), 1080);
        assert_eq!(ScaDate::diff(&s(2007, 6, 1, 3), &s(2010, 6, 1, 3)), 1096);
        let mut c = s(2008, 12, 15, 1);
        c.add_months(-12);
        assert_eq!(ymd(c.date()), (2007, 12, 15));
        assert_eq!(diff360(days(2008, 12, 15), days(2008, 12, 31), true), 16);
        assert_eq!(diff360(days(2008, 11, 15), days(2008, 12, 31), true), 46);
    }

    /// year_diff (ACCRINTM's day count), year_frac (YEARFRAC) and
    /// ScaDate::diff (COUPDAYBS) on the dates that reach their remaining
    /// branches, against LibreOffice 25.2.
    #[test]
    fn day_counts_match_libreoffice() {
        let d = |y, m, dd| days(y, m, dd);
        let near = |a: f64, b: f64| assert!((a - b).abs() < 1e-15, "{a} != {b}");
        // ACCRINTM(…, 1, 1, basis): a February start in a later month of
        // the same year, a leap and a common actual year.
        near(
            year_diff(d(2008, 2, 10), d(2008, 3, 10), 0),
            0.080_555_555_555_555_56,
        );
        near(
            year_diff(d(2007, 2, 10), d(2008, 4, 10), 0),
            1.166_666_666_666_666_7,
        );
        near(
            year_diff(d(2007, 2, 10), d(2007, 4, 11), 0),
            0.163_888_888_888_888_9,
        );
        near(
            year_diff(d(2008, 1, 1), d(2008, 12, 31), 1),
            0.997_267_759_562_841_5,
        );
        near(year_diff(d(2007, 1, 10), d(2008, 1, 10), 1), 1.0);
        near(
            year_diff(d(2008, 2, 5), d(2008, 2, 20), 0),
            0.041_666_666_666_666_664,
        );
        // YEARFRAC basis 1 within a leap year, and across into one (ending
        // before, on and after its February 29th); basis 4.
        near(
            year_frac(d(2007, 6, 1), d(2008, 2, 1), 1),
            0.671_232_876_712_328_8,
        );
        near(
            year_frac(d(2007, 6, 1), d(2008, 2, 29), 1),
            0.745_901_639_344_262_3,
        );
        near(
            year_frac(d(2007, 6, 1), d(2008, 3, 1), 1),
            0.748_633_879_781_420_8,
        );
        near(
            year_frac(d(2008, 2, 10), d(2008, 3, 10), 1),
            0.079_234_972_677_595_63,
        );
        near(
            year_frac(d(2007, 3, 1), d(2008, 1, 15), 1),
            0.876_712_328_767_123_2,
        );
        near(year_frac(d(2008, 2, 10), d(2008, 4, 10), 4), 1.0 / 6.0);
        // COUPDAYBS: 30E/360 from a February end, actual/actual across a
        // month end, 30/360 US from a 31st.
        let s = |y, m, dd, b| ScaDate::new(days(y, m, dd), b);
        assert_eq!(ScaDate::diff(&s(2007, 8, 31, 4), &s(2008, 2, 15, 4)), 165);
        // (LibreOffice COUPDAYBS(2008-03-01, 2009-02-28, 2, 4) = 2.)
        assert_eq!(ScaDate::diff(&s(2008, 2, 29, 4), &s(2008, 3, 1, 4)), 2);
        assert_eq!(coupdaybs(d(2008, 3, 1), d(2009, 2, 28), 2, 4), 2.0);
        assert_eq!(ScaDate::diff(&s(2008, 3, 1, 1), &s(2008, 3, 15, 1)), 14);
        assert_eq!(ScaDate::diff(&s(2008, 5, 31, 0), &s(2008, 7, 31, 0)), 60);
        // Mid-month to mid-February, 30/360 US and 30E/360.
        assert_eq!(ScaDate::diff(&s(2008, 1, 15, 0), &s(2008, 2, 15, 0)), 30);
        assert_eq!(ScaDate::diff(&s(2008, 2, 10, 4), &s(2008, 3, 10, 4)), 30);
        assert_eq!(coupdaybs(d(2008, 2, 15), d(2008, 8, 31), 2, 4), 165.0);
        assert_eq!(coupdaybs(d(2008, 3, 15), d(2009, 3, 1), 2, 1), 14.0);
        assert_eq!(coupdaybs(d(2008, 7, 31), d(2008, 11, 30), 2, 0), 60.0);
    }

    #[test]
    fn sca_date_keeps_month_ends() {
        let mut s = ScaDate::new(days(2024, 2, 29), 1);
        s.add_months(6);
        assert_eq!(ymd(s.date()), (2024, 8, 31));
        s.add_months(-14);
        assert_eq!(ymd(s.date()), (2023, 6, 30));
        let a = ScaDate::new(days(2023, 1, 31), 4);
        let b = ScaDate::new(days(2023, 2, 28), 4);
        assert_eq!(ScaDate::diff(&a, &b), 28);
        assert_eq!(ScaDate::diff(&b, &a), 28);
    }
}
