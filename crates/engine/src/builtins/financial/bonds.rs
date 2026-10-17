//! Bonds and securities (ids 740–765) and the French depreciation functions
//! AMORDEGRC/AMORLINC. Day counting is LibreOffice's ([`super::daycount`]),
//! except ODDFPRICE/ODDFYIELD: LibreOffice does not implement them (it
//! always returns an error), so they port Gnumeric's `calc_oddfprice`
//! (plugins/fn-financial/functions.c) with its own day counting ([`gnm`]).
//!
//! Where LibreOffice departs from Excel's documented formulas, Excel wins:
//! PRICE and YIELD with one coupon left use Excel's simple-interest formula
//! (LibreOffice compounds), and DURATION/MDURATION time each cash flow at
//! `k − 1 + DSC/E` periods (LibreOffice derives the offset from YEARFRAC,
//! which drifts under actual/actual).

use super::super::{b, d, domain, err, n, n_or, ok_num, DAY_MS, R};
use super::daycount as dc;
use super::solve;
use crate::CellValue;

/// Date argument `i` as a whole day number (days since 1970-01-01).
fn day(a: &[CellValue], i: usize) -> Result<i64, u8> {
    Ok(d(a, i)?.div_euclid(DAY_MS))
}

/// Coupons per year: 1, 2 or 4 (truncated).
fn freq(a: &[CellValue], i: usize) -> Result<u32, u8> {
    match n(a, i)?.trunc() {
        1.0 => Ok(1),
        2.0 => Ok(2),
        4.0 => Ok(4),
        _ => Err(err::NUM),
    }
}

/// Day-count basis 0–4 (truncated; default 0).
fn basis(a: &[CellValue], i: usize) -> Result<u32, u8> {
    let x = n_or(a, i, 0.0)?.trunc();
    domain((0.0..=4.0).contains(&x))?;
    Ok(x as u32)
}

fn date(z: i64) -> R {
    Ok(CellValue::Date(z * DAY_MS))
}

pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    let x = |i| n(a, i);
    match id {
        740 => {
            let (issue, first, settle) = (day(a, 0)?, day(a, 1)?, day(a, 2)?);
            let (rate, par, _, bs) = (x(3)?, x(4)?, freq(a, 5)?, basis(a, 6)?);
            let from_issue = a.len() <= 7 || b(a, 7)?;
            domain(rate > 0.0 && par > 0.0 && issue < settle)?;
            let start = if from_issue || first >= settle {
                issue
            } else {
                first
            };
            ok_num(par * rate * dc::year_diff(start, settle, bs))
        }
        741 => {
            let (issue, settle, rate) = (day(a, 0)?, day(a, 1)?, x(2)?);
            let (par, bs) = (n_or(a, 3, 1000.0)?, basis(a, 4)?);
            domain(rate > 0.0 && par > 0.0 && issue < settle)?;
            ok_num(par * rate * dc::year_diff(issue, settle, bs))
        }
        742..=747 => {
            let (s, m, f, bs) = (day(a, 0)?, day(a, 1)?, freq(a, 2)?, basis(a, 3)?);
            domain(s < m)?;
            match id {
                742 => ok_num(dc::coupdaybs(s, m, f, bs)),
                743 => ok_num(dc::coupdays(s, m, f, bs)),
                744 => ok_num(dc::coupdaysnc(s, m, f, bs)),
                745 => date(dc::ncd(s, m, f, bs).date()),
                746 => ok_num(dc::coupnum(s, m, f, bs)),
                _ => date(dc::pcd(s, m, f, bs).date()),
            }
        }
        748 | 751 | 757 | 759 | 764 => {
            // DISC, INTRATE, PRICEDISC, RECEIVED, YIELDDISC(settle, maturity, p, q, [basis]).
            let (s, m, p, q, bs) = (day(a, 0)?, day(a, 1)?, x(2)?, x(3)?, basis(a, 4)?);
            domain(p > 0.0 && q > 0.0 && s < m)?;
            ok_num(match id {
                748 => (1.0 - p / q) / dc::year_frac(s, m, bs),
                751 => (q / p - 1.0) / dc::year_diff(s, m, bs),
                757 => q * (1.0 - p * dc::year_diff(s, m, bs)),
                759 => p / (1.0 - q * dc::year_diff(s, m, bs)),
                _ => (q / p - 1.0) / dc::year_frac(s, m, bs),
            })
        }
        749 | 750 => {
            let (s, m, coupon, yld) = (day(a, 0)?, day(a, 1)?, x(2)?, x(3)?);
            let (f, bs) = (freq(a, 4)?, basis(a, 5)?);
            domain(coupon >= 0.0 && yld >= 0.0 && s < m)?;
            let dur = duration(s, m, coupon, yld, f, bs);
            ok_num(if id == 749 {
                dur
            } else {
                dur / (1.0 + yld / f64::from(f))
            })
        }
        752 | 753 => {
            let (s, m, issue, first) = (day(a, 0)?, day(a, 1)?, day(a, 2)?, day(a, 3)?);
            let (rate, v, redemp) = (x(4)?, x(5)?, x(6)?);
            let (f, bs) = (freq(a, 7)?, basis(a, 8)?);
            domain(rate >= 0.0 && redemp > 0.0 && issue < s && s < first && first < m)?;
            let price = |y| gnm::oddfprice(s, m, issue, first, rate, y, redemp, f, bs);
            if id == 752 {
                domain(v >= 0.0)?;
                return ok_num(price(v));
            }
            domain(v > 0.0)?;
            yield_of(&|y| price(y) - v, f)
        }
        754 | 755 => {
            let (s, m, last) = (day(a, 0)?, day(a, 1)?, day(a, 2)?);
            let (rate, v, redemp) = (x(3)?, x(4)?, x(5)?);
            let (f, bs) = (f64::from(freq(a, 6)?), basis(a, 7)?);
            domain(rate >= 0.0 && redemp > 0.0 && last < s && s < m)?;
            // LibreOffice's formulas: periods as YEARFRAC × frequency.
            let dci = dc::year_frac(last, m, bs) * f;
            let dsci = dc::year_frac(s, m, bs) * f;
            let ai = dc::year_frac(last, s, bs) * f;
            let c = 100.0 * rate / f;
            if id == 754 {
                domain(v >= 0.0)?;
                ok_num((redemp + dci * c) / (dsci * v / f + 1.0) - ai * c)
            } else {
                // The exact inverse of ODDLPRICE (linear in 1/(1 + yield)).
                domain(v > 0.0)?;
                ok_num(((redemp + dci * c) / (v + ai * c) - 1.0) * f / dsci)
            }
        }
        756 | 763 => {
            let (s, m, rate, v, redemp) = (day(a, 0)?, day(a, 1)?, x(2)?, x(3)?, x(4)?);
            let (f, bs) = (freq(a, 5)?, basis(a, 6)?);
            domain(rate >= 0.0 && redemp > 0.0 && s < m)?;
            let bond = Bond::new(s, m, rate, redemp, f, bs);
            if id == 756 {
                domain(v >= 0.0)?;
                return ok_num(bond.price(v));
            }
            domain(v > 0.0)?;
            if bond.n <= 1.0 {
                return ok_num(bond.one_period_yield(v));
            }
            yield_of(&|y| bond.price(y) - v, f)
        }
        758 | 765 => {
            let (s, m, issue, rate, v) = (day(a, 0)?, day(a, 1)?, day(a, 2)?, x(3)?, x(4)?);
            let bs = basis(a, 5)?;
            let (im, is, sm) = (
                dc::year_frac(issue, m, bs),
                dc::year_frac(issue, s, bs),
                dc::year_frac(s, m, bs),
            );
            if id == 758 {
                domain(rate >= 0.0 && v >= 0.0 && s < m)?;
                ok_num(((1.0 + im * rate) / (1.0 + sm * v) - is * rate) * 100.0)
            } else {
                domain(v > 0.0 && rate >= 0.0 && s < m && issue <= s)?;
                ok_num(((1.0 + im * rate) / (v / 100.0 + is * rate) - 1.0) / sm)
            }
        }
        760 => {
            // LibreOffice: DSM counted 30/360 (US) to the day after maturity.
            let (s, m, disc) = (day(a, 0)?, day(a, 1)? + 1, x(2)?);
            let dsm = dc::diff360(s, m, true) as f64;
            domain(disc > 0.0 && s < m && dsm <= 360.0)?;
            ok_num(365.0 * disc / (360.0 - disc * dsm))
        }
        761 => {
            // LibreOffice's formula (30/360 to the day after maturity) with
            // Excel's limit: more than a year to maturity is #NUM!
            // (LibreOffice instead rejects a whole number of 30/360 years).
            let (s, m, disc) = (day(a, 0)?, day(a, 1)?, x(2)?);
            domain(disc > 0.0 && s <= m && m <= dc::add_months(s, 12))?;
            ok_num(100.0 * (1.0 - disc * dc::year_frac(s, m + 1, 0)))
        }
        _ => {
            // 762 TBILLYIELD
            let (s, m, price) = (day(a, 0)?, day(a, 1)?, x(2)?);
            let dsm = (dc::diff360(s, m, true) + 1) as f64;
            domain(price > 0.0 && s < m && dsm <= 360.0)?;
            ok_num((100.0 / price - 1.0) / dsm * 360.0)
        }
    }
}

/// A yield: the root of `gap(y)` (price at y minus the given price), by the
/// RATE solver with a numeric derivative. Yields stay above `−frequency`.
fn yield_of(gap: &dyn Fn(f64) -> f64, f: u32) -> R {
    let df = |y: f64| {
        let h = 1e-6 * (1.0 + y.abs());
        (gap(y + h) - gap(y - h)) / (2.0 * h)
    };
    ok_num(solve(gap, &df, 0.1, -f64::from(f)).ok_or(err::NUM)?)
}

/// The coupon schedule of a regular bond at settlement.
struct Bond {
    /// Coupons left, days from the previous coupon, days to the next, days in
    /// the period.
    n: f64,
    a: f64,
    dsc: f64,
    e: f64,
    coupon: f64,
    redemp: f64,
    f: f64,
}

impl Bond {
    fn new(s: i64, m: i64, rate: f64, redemp: f64, f: u32, bs: u32) -> Self {
        Bond {
            n: dc::coupnum(s, m, f, bs),
            a: dc::coupdaybs(s, m, f, bs),
            dsc: dc::coupdaysnc(s, m, f, bs),
            e: dc::coupdays(s, m, f, bs),
            coupon: 100.0 * rate / f64::from(f),
            redemp,
            f: f64::from(f),
        }
    }

    /// Excel's PRICE: compound discounting, or simple interest when one
    /// coupon is left.
    fn price(&self, y: f64) -> f64 {
        let (c, s, v) = (self.coupon, self.dsc / self.e, 1.0 + y / self.f);
        let accrued = c * self.a / self.e;
        if self.n <= 1.0 {
            return (self.redemp + c) / (1.0 + s * y / self.f) - accrued;
        }
        let mut p = self.redemp / v.powf(self.n - 1.0 + s) - accrued;
        let mut k = 0.0;
        while k < self.n {
            p += c / v.powf(k + s);
            k += 1.0;
        }
        p
    }

    /// Excel's closed-form YIELD for one coupon left (the inverse of
    /// [`Bond::price`] there).
    fn one_period_yield(&self, price: f64) -> f64 {
        let (r, c) = (self.redemp / 100.0, self.coupon / 100.0);
        let paid = price / 100.0 + self.a / self.e * c;
        (r + c - paid) / paid * self.f * self.e / self.dsc
    }
}

/// Macaulay duration in years, cash flow k at `k − 1 + DSC/E` periods.
fn duration(s: i64, m: i64, coupon: f64, yld: f64, f: u32, bs: u32) -> f64 {
    let n = dc::coupnum(s, m, f, bs);
    let off = dc::coupdaysnc(s, m, f, bs) / dc::coupdays(s, m, f, bs) - 1.0;
    let (c, v) = (100.0 * coupon / f64::from(f), 1.0 + yld / f64::from(f));
    let (mut num, mut den) = (0.0, 0.0);
    let mut k = 1.0;
    while k <= n {
        let t = k + off;
        let cf = if k == n { c + 100.0 } else { c };
        let pv = cf / v.powf(t);
        num += t * pv;
        den += pv;
        k += 1.0;
    }
    num / den / f64::from(f)
}

/// AMORDEGRC (`degressive`) or AMORLINC(cost, purchased, first, salvage,
/// period, rate, [basis]): LibreOffice's French depreciation.
pub(super) fn amortize(degressive: bool, a: &[CellValue]) -> R {
    let (cost, bought, first) = (n(a, 0)?, day(a, 1)?, day(a, 2)?);
    let (salvage, per, rate, bs) = (n(a, 3)?, n(a, 4)?, n(a, 5)?, basis(a, 6)?);
    domain(
        bought <= first
            && rate > 0.0
            && salvage <= cost
            && cost > 0.0
            && salvage >= 0.0
            && per >= 0.0,
    )?;
    let per = per.trunc();
    let yf = dc::year_frac(bought, first, bs);
    if !degressive {
        let one = cost * rate;
        let first_rate = yf * rate * cost;
        let full = ((cost - salvage - first_rate) / one).trunc().max(0.0);
        let v = if per == 0.0 {
            first_rate
        } else if per <= full {
            one
        } else if per == full + 1.0 {
            cost - salvage - one * full - first_rate
        } else {
            0.0
        };
        return ok_num(v.max(0.0));
    }
    let life = 1.0 / rate;
    let coeff = if life < 3.0 {
        1.0
    } else if life < 5.0 {
        1.5
    } else if life <= 6.0 {
        2.0
    } else {
        2.5
    };
    let rate = rate * coeff;
    let mut nrate = (yf * rate * cost).round();
    let mut cost = cost - nrate;
    let mut rest = cost - salvage;
    let mut i = 0.0;
    while i < per {
        nrate = (rate * cost).round();
        rest -= nrate;
        if rest < 0.0 {
            return ok_num(if per - i <= 1.0 {
                (cost * 0.5).round()
            } else {
                0.0
            });
        }
        cost -= nrate;
        i += 1.0;
    }
    ok_num(nrate)
}

/// Gnumeric's ODDFPRICE and the day counting it relies on (goffice
/// `go_date_days_between_basis`, `go_coup_cd`, `go_coupdays`; Gnumeric
/// `coupnum`, `date_ratio`), with end-of-month coupon rules on.
mod gnm {
    use super::dc::{add_months, days, month_days, ymd};

    fn last_of_month(z: i64) -> bool {
        let (y, m, d) = ymd(z);
        d == month_days(m, y)
    }

    /// Days from `a` to `b` (signed) in `basis` (0 MSRB 30/360, 4 30E/360).
    fn between(a: i64, b: i64, basis: u32) -> f64 {
        let (sign, from, to) = if a > b { (-1, b, a) } else { (1, a, b) };
        let ((y1, m1, mut d1), (y2, m2, mut d2)) = (ymd(from), ymd(to));
        let n = match basis {
            0 | 4 if from == to => 0,
            0 => {
                if d1 == 31 {
                    d1 = 30;
                }
                if d2 == 31 && d1 == 30 {
                    d2 = 30;
                }
                if m1 == 2 && last_of_month(from) {
                    if m2 == 2 && last_of_month(to) {
                        d2 = 30;
                    }
                    d1 = 30;
                }
                (y2 - y1) * 360 + (i64::from(m2) - i64::from(m1)) * 30 + i64::from(d2)
                    - i64::from(d1)
            }
            4 => {
                d1 = d1.min(30);
                d2 = d2.min(30);
                (y2 - y1) * 360 + (i64::from(m2) - i64::from(m1)) * 30 + i64::from(d2)
                    - i64::from(d1)
            }
            _ => to - from,
        };
        (sign * n) as f64
    }

    /// `go_coup_cd`: the coupon date after (`next`) or on/before `settle`.
    fn coup_cd(settle: i64, mat: i64, freq: u32, next: bool) -> i64 {
        let eom = last_of_month(mat);
        let months = i64::from(12 / freq);
        let at = |periods: i64| {
            let z = add_months(mat, -periods * months);
            if eom {
                let (y, m, _) = ymd(z);
                days(y, m, month_days(m, y))
            } else {
                z
            }
        };
        let mut periods = ymd(mat).0 - ymd(settle).0;
        if periods > 0 {
            periods = (periods - 1) * i64::from(freq);
        }
        let mut r;
        loop {
            periods += 1;
            r = at(periods);
            if settle >= r {
                break;
            }
        }
        if next {
            at(periods - 1)
        } else {
            r
        }
    }

    fn coupdays(settle: i64, mat: i64, freq: u32, basis: u32) -> f64 {
        match basis {
            1 => between(
                coup_cd(settle, mat, freq, false),
                coup_cd(settle, mat, freq, true),
                1,
            ),
            3 => 365.0 / f64::from(freq),
            _ => f64::from(360 / freq),
        }
    }

    fn coupnum(settle: i64, mat: i64, freq: u32) -> i64 {
        let ((ys, ms, ds), (ym, mm, _)) = (ymd(settle), ymd(mat));
        let mut months = i64::from(mm) - i64::from(ms) + 12 * (ym - ys);
        let mut this = add_months(mat, -months);
        if last_of_month(mat) {
            while !last_of_month(this) {
                this += 1;
            }
        }
        if ds >= ymd(this).2 {
            months -= 1;
        }
        1 + months / i64::from(12 / freq)
    }

    fn date_ratio(d1: i64, d2: i64, d3: i64, freq: u32, basis: u32) -> f64 {
        let mut next = coup_cd(d1, d3, freq, true);
        let prev = coup_cd(d1, d3, freq, false);
        if next >= d2 {
            return between(d1, d2, basis) / coupdays(prev, next, freq, basis);
        }
        let mut res = between(d1, next, basis) / coupdays(prev, next, freq, basis);
        loop {
            let prev = next;
            next = add_months(next, i64::from(12 / freq));
            if next >= d2 {
                return res + between(prev, d2, basis) / coupdays(prev, next, freq, basis);
            }
            res += 1.0;
        }
    }

    #[allow(clippy::too_many_arguments, reason = "the spreadsheet signature")]
    pub(super) fn oddfprice(
        settle: i64,
        mat: i64,
        issue: i64,
        first: i64,
        rate: f64,
        yld: f64,
        redemp: f64,
        freq: u32,
        basis: u32,
    ) -> f64 {
        let mut a = between(issue, settle, basis);
        let mut ds = between(settle, first, basis);
        let mut df = between(issue, first, basis);
        let e = coupdays(settle, mat, freq, basis);
        let mut n = coupnum(settle, mat, freq) as f64;
        let scale = 100.0 * rate / f64::from(freq);
        let f = 1.0 + yld / f64::from(freq);
        if ds > e {
            // Odd long first period.
            if basis == 0 || basis == 4 {
                n = 1.0 + (between(first, mat, basis) / e).ceil();
            } else {
                let mut d = first;
                let mut k = 0.0;
                loop {
                    let prev = d;
                    d = add_months(d, i64::from(12 / freq));
                    if d >= mat {
                        k += (between(prev, mat, basis) / coupdays(prev, d, freq, basis)).ceil()
                            + 1.0;
                        break;
                    }
                    k += 1.0;
                }
                n = k;
                a = e * date_ratio(issue, settle, first, freq, basis);
                ds = e * date_ratio(settle, first, first, freq, basis);
                df = e * date_ratio(issue, first, first, freq, basis);
            }
        }
        let term1 = redemp / f.powf(n - 1.0 + ds / e);
        let term2 = (df / e) / f.powf(ds / e);
        let sum = f.powf(-ds / e) * (f.powf(-n) - 1.0 / f) / (1.0 / f - 1.0);
        term1 + scale * (term2 + sum - a / e)
    }

    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    mod tests {
        use super::*;

        #[test]
        fn gnumeric_day_counts() {
            let d = days;
            assert_eq!(between(d(2008, 3, 1), d(2008, 1, 31), 0), -31.0);
            assert_eq!(between(d(2008, 2, 29), d(2009, 2, 28), 0), 360.0);
            assert_eq!(between(d(2008, 1, 31), d(2008, 3, 31), 4), 60.0);
            assert_eq!(between(d(2008, 1, 31), d(2008, 1, 31), 0), 0.0);
            assert_eq!(
                coup_cd(d(2008, 11, 11), d(2021, 3, 1), 2, false),
                d(2008, 9, 1)
            );
            assert_eq!(
                coup_cd(d(2008, 11, 11), d(2021, 3, 1), 2, true),
                d(2009, 3, 1)
            );
            // End-of-month maturity keeps coupons on month ends.
            assert_eq!(
                coup_cd(d(2008, 3, 15), d(2010, 2, 28), 2, true),
                d(2008, 8, 31)
            );
            assert_eq!(coupnum(d(2008, 3, 15), d(2010, 2, 28), 2), 4);
            assert_eq!(add_months(d(2008, 1, 31), 1), d(2008, 2, 29));
        }
    }
}
