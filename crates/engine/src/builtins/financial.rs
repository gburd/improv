//! Evaluation of the financial functions (see `improv_core_model::functions`):
//! time value of money and depreciation here, bonds in [`bonds`], day
//! counting in [`daycount`]. Formulas follow LibreOffice's
//! (`sc/source/core/tool/interpr2.cxx` and `scaddins/source/analysis`) so
//! results match it, except where Excel's documentation disagrees (noted per
//! function). Cash paid out is negative (Excel's sign convention).

mod bonds;
mod daycount;

use super::{b, domain, err, n, n_or, ok_num, R};
use crate::CellValue;

/// Evaluate financial function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    match id {
        700..=715 => tvm(id, a),
        720..=726 => depreciation(id, a),
        740..=765 => bonds::eval(id, a),
        // Unknown ids are #NAME? (the reductions live elsewhere).
        _ => Err(err::NAME),
    }
}

/// `x` floored after rounding to 15 significant digits, so 2.9999999999999996
/// floors to 3 (LibreOffice's `rtl::math::approxFloor`).
fn approx_floor(x: f64) -> f64 {
    if approx_eq(x, x.round()) {
        x.round()
    } else {
        x.floor()
    }
}

fn approx_ceil(x: f64) -> f64 {
    if approx_eq(x, x.round()) {
        x.round()
    } else {
        x.ceil()
    }
}

/// LibreOffice's `rtl::math::approxEqual`: equal within 2⁻⁴⁸ relative.
fn approx_eq(a: f64, b: f64) -> bool {
    a == b || (a - b).abs() < a.abs().max(b.abs()) * (1.0 / 281_474_976_710_656.0)
}

/// Payment timing: 0 (or omitted) at the end of a period, any other number at
/// the beginning.
fn advance(a: &[CellValue], i: usize) -> Result<bool, u8> {
    Ok(n_or(a, i, 0.0)? != 0.0)
}

fn fv(rate: f64, nper: f64, pmt: f64, pv: f64, adv: bool) -> f64 {
    let fv = if rate == 0.0 {
        pv + pmt * nper
    } else {
        // (1+r)ⁿ − 1 through expm1 and log1p, as in PMT: no cancellation at
        // a tiny rate.
        let grow = (nper * rate.ln_1p()).exp_m1();
        let due = if adv { 1.0 + rate } else { 1.0 };
        pv * (grow + 1.0) + pmt * due * grow / rate
    };
    -fv
}

fn pv(rate: f64, nper: f64, pmt: f64, fv: f64, adv: bool) -> f64 {
    let pv = if rate == 0.0 {
        fv + pmt * nper
    } else {
        // (1+r)⁻ⁿ, and 1 − (1+r)⁻ⁿ through expm1 and log1p (each without
        // cancellation, at a tiny rate and at a large one).
        let k = -nper * rate.ln_1p();
        let due = if adv { 1.0 + rate } else { 1.0 };
        fv * k.exp() - pmt * due * k.exp_m1() / rate
    };
    -pv
}

fn pmt(rate: f64, nper: f64, pv: f64, fv: f64, adv: bool) -> f64 {
    let p = if rate == 0.0 {
        (pv + fv) / nper
    } else {
        let l = rate.ln_1p();
        let num = (fv + pv * (nper * l).exp()) * rate;
        if adv {
            num / (((nper + 1.0) * l).exp_m1() - rate)
        } else {
            num / (nper * l).exp_m1()
        }
    };
    -p
}

/// (interest part of payment `per`, the payment).
fn ipmt(rate: f64, per: f64, nper: f64, pv: f64, fv_: f64, adv: bool) -> (f64, f64) {
    let p = pmt(rate, nper, pv, fv_, adv);
    let i = if per == 1.0 {
        if adv {
            0.0
        } else {
            -pv
        }
    } else if adv {
        fv(rate, per - 2.0, p, pv, true) - p
    } else {
        fv(rate, per - 1.0, p, pv, false)
    };
    (i * rate, p)
}

/// RATE: the root of `fv + pv·(1+x)ⁿ + pmt·(1+x·type)·((1+x)ⁿ−1)/x`.
fn rate(nper: f64, pmt: f64, pv: f64, fv: f64, adv: bool, guess: f64) -> Option<f64> {
    let t = if adv { 1.0 } else { 0.0 };
    // (1+x)ⁿ − 1 via expm1, accurate near x = 0.
    let grow = |x: f64| (nper * x.ln_1p()).exp_m1();
    let f = |x: f64| {
        if x == 0.0 {
            fv + pv + pmt * nper
        } else {
            let g = grow(x);
            fv + pv * (g + 1.0) + pmt * (1.0 + x * t) * g / x
        }
    };
    let df = |x: f64| {
        if x == 0.0 {
            pv * nper + pmt * (t * nper + nper * (nper - 1.0) / 2.0)
        } else {
            let p1 = (1.0 + x).powf(nper - 1.0);
            let g = grow(x) / x;
            let dg = (nper * p1 - g) / x;
            pv * nper * p1 + pmt * (t * g + (1.0 + x * t) * dg)
        }
    };
    solve(&f, &df, guess, -1.0)
}

/// The deterministic root finder of RATE and the bond yields: Newton's method
/// from `guess` (at most 50 steps, done when a step is under 1e-10); failing
/// that, bisection of the sign change on a fixed grid nearest the guess.
/// Roots lie above `lower`; `None` if there is none.
fn solve(f: &dyn Fn(f64) -> f64, df: &dyn Fn(f64) -> f64, guess: f64, lower: f64) -> Option<f64> {
    let mut x = guess;
    if x > lower {
        for _ in 0..50 {
            let (y, dy) = (f(x), df(x));
            if y == 0.0 {
                return Some(x);
            }
            let next = x - y / dy;
            if !next.is_finite() || next <= lower {
                break;
            }
            if (next - x).abs() < 1e-10 {
                return Some(next);
            }
            x = next;
        }
    }
    bisect(f, guess, lower)
}

const GRID: [f64; 21] = [
    -0.999, -0.99, -0.9, -0.75, -0.5, -0.25, -0.1, -0.05, -0.01, 0.0, 0.01, 0.05, 0.1, 0.25, 0.5,
    1.0, 2.0, 5.0, 10.0, 100.0, 1000.0,
];

fn bisect(f: &dyn Fn(f64) -> f64, guess: f64, lower: f64) -> Option<f64> {
    let pts: Vec<(f64, f64)> = GRID
        .iter()
        .filter(|&&x| x > lower)
        .map(|&x| (x, f(x)))
        .filter(|p| p.1.is_finite())
        .collect();
    let dist = |w: &&[(f64, f64)]| ((w[0].0 + w[1].0) / 2.0 - guess).abs();
    let w = pts
        .windows(2)
        .filter(|w| (w[0].1 < 0.0) != (w[1].1 < 0.0))
        .min_by(|a, b| dist(a).total_cmp(&dist(b)))?;
    let ((mut lo, flo), (mut hi, _)) = (w[0], w[1]);
    let neg = flo < 0.0;
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if mid <= lo || mid >= hi {
            break;
        }
        if (f(mid) < 0.0) == neg {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some((lo + hi) / 2.0)
}

fn tvm(id: u32, a: &[CellValue]) -> R {
    let x = |i| n(a, i);
    match id {
        700 => ok_num(fv(x(0)?, x(1)?, x(2)?, n_or(a, 3, 0.0)?, advance(a, 4)?)),
        701 => ok_num(pv(x(0)?, x(1)?, x(2)?, n_or(a, 3, 0.0)?, advance(a, 4)?)),
        702 => ok_num(pmt(x(0)?, x(1)?, x(2)?, n_or(a, 3, 0.0)?, advance(a, 4)?)),
        703 | 704 => {
            let (rate, per, nper, pv) = (x(0)?, x(1)?, x(2)?, x(3)?);
            domain(per >= 1.0 && per <= nper)?;
            let (i, p) = ipmt(rate, per, nper, pv, n_or(a, 4, 0.0)?, advance(a, 5)?);
            ok_num(if id == 703 { i } else { p - i })
        }
        705 => {
            let (rate, pmt, pv, fv, adv) = (x(0)?, x(1)?, x(2)?, n_or(a, 3, 0.0)?, advance(a, 4)?);
            ok_num(if pv + fv == 0.0 {
                0.0
            } else if rate == 0.0 {
                -(pv + fv) / pmt
            } else if adv {
                (-(rate * fv - pmt * (1.0 + rate)) / (rate * pv + pmt * (1.0 + rate))).ln()
                    / rate.ln_1p()
            } else {
                (-(rate * fv - pmt) / (rate * pv + pmt)).ln() / rate.ln_1p()
            })
        }
        706 => {
            let nper = x(0)?;
            domain(nper > 0.0)?;
            let r = rate(
                nper,
                x(1)?,
                x(2)?,
                n_or(a, 3, 0.0)?,
                advance(a, 4)?,
                n_or(a, 5, 0.1)?,
            );
            ok_num(r.ok_or(err::NUM)?)
        }
        707 | 708 => {
            let (rate, nper, pv) = (x(0)?, x(1)?, x(2)?);
            let (start, end, flag) = (approx_floor(x(3)?), approx_floor(x(4)?), x(5)?);
            domain(
                start >= 1.0
                    && end >= start
                    && rate > 0.0
                    && end <= nper
                    && pv > 0.0
                    && (flag == 0.0 || flag == 1.0)
                    && end - start < CUM_MAX_PERIODS,
            )?;
            ok_num(cumulative(
                id == 707,
                rate,
                nper,
                pv,
                start,
                end,
                flag == 1.0,
            ))
        }
        709 => {
            let (rate, per, nper, pv) = (x(0)?, x(1)?, x(2)?, x(3)?);
            ok_num(pv * rate * (per / nper - 1.0))
        }
        710 => {
            // Excel: nominal ≤ 0 is #NUM! (LibreOffice returns 0 for 0).
            let (nominal, k) = (x(0)?, approx_floor(x(1)?));
            domain(nominal > 0.0 && k >= 1.0)?;
            ok_num((1.0 + nominal / k).powf(k) - 1.0)
        }
        711 => {
            let (effect, k) = (x(0)?, x(1)?);
            domain(effect > 0.0 && k >= 1.0)?;
            let k = approx_floor(k);
            ok_num(((effect + 1.0).powf(1.0 / k) - 1.0) * k)
        }
        712 => {
            let (rate, pv, fv) = (x(0)?, x(1)?, x(2)?);
            domain(rate > 0.0 && pv > 0.0 && fv > 0.0)?;
            ok_num((fv / pv).ln() / rate.ln_1p())
        }
        713 => {
            let (nper, pv, fv) = (x(0)?, x(1)?, x(2)?);
            domain(nper > 0.0 && pv != 0.0)?;
            ok_num((fv / pv).powf(1.0 / nper) - 1.0)
        }
        _ => {
            // 714 DOLLARDE, 715 DOLLARFR. Excel: fraction truncated; below 0
            // #NUM!, below 1 #DIV/0!.
            let (price, frac) = (x(0)?, x(1)?.trunc());
            domain(frac >= 0.0)?;
            if frac < 1.0 {
                return Err(err::DIV0);
            }
            let int = price.trunc();
            let digits = frac.log10().ceil();
            ok_num(if id == 714 {
                (price - int) / frac * 10f64.powf(digits) + int
            } else {
                (price - int) * frac * 10f64.powf(-digits) + int
            })
        }
    }
}

/// CUMIPMT and CUMPRINC sum one period at a time, so the span is capped
/// (#NUM! past it): 1e7 periods take about 0.07 s. LibreOffice and Gnumeric
/// have no cap (Gnumeric's int periods stop at 2³¹).
// ponytail: the interest sum has a closed form (a geometric series) that
// would lift the cap.
const CUM_MAX_PERIODS: f64 = 1e7;

/// CUMIPMT (`interest`) or CUMPRINC from period `start` to `end`.
fn cumulative(
    interest: bool,
    rate: f64,
    nper: f64,
    pv: f64,
    start: f64,
    end: f64,
    adv: bool,
) -> f64 {
    let p = pmt(rate, nper, pv, 0.0, adv);
    // The balance-derived interest of period i (times rate below).
    let owed = |i: f64| {
        if adv {
            fv(rate, i - 2.0, p, pv, true) - p
        } else {
            fv(rate, i - 1.0, p, pv, false)
        }
    };
    let mut sum = 0.0;
    let mut i = start;
    if i == 1.0 {
        sum = match (interest, adv) {
            (true, true) => 0.0,
            (true, false) => -pv,
            (false, true) => p,
            (false, false) => p + pv * rate,
        };
        i += 1.0;
    }
    while i <= end {
        sum += if interest {
            owed(i)
        } else {
            p - owed(i) * rate
        };
        i += 1.0;
    }
    if interest {
        sum * rate
    } else {
        sum
    }
}

fn ddb(cost: f64, salvage: f64, life: f64, period: f64, factor: f64) -> f64 {
    let mut rate = factor / life;
    let old = if rate >= 1.0 {
        rate = 1.0;
        if period == 1.0 {
            cost
        } else {
            0.0
        }
    } else {
        cost * (1.0 - rate).powf(period - 1.0)
    };
    let new = cost * (1.0 - rate).powf(period);
    let v = if new < salvage {
        old - salvage
    } else {
        old - new
    };
    v.max(0.0)
}

/// LibreOffice's `ScInterVDB`: declining balance switching to straight line,
/// over `period` periods of a `life1`-period remainder.
fn inter_vdb(cost: f64, salvage: f64, life: f64, life1: f64, period: f64, factor: f64) -> f64 {
    let end = approx_ceil(period);
    let mut value = cost - salvage;
    let (mut vdb, mut sln, mut now_sln) = (0.0, 0.0, false);
    let mut i = 1.0;
    while i <= end {
        let mut term = if now_sln {
            sln
        } else {
            let d = ddb(cost, salvage, life, i, factor);
            sln = value / (life1 - (i - 1.0));
            if sln > d {
                now_sln = true;
                sln
            } else {
                value -= d;
                d
            }
        };
        if i == end {
            term *= period + 1.0 - end;
        }
        vdb += term;
        i += 1.0;
    }
    vdb
}

/// VDB walks every period up to `end`, so the end period is capped (#NUM!
/// past it). LibreOffice has no cap (VDB(1000;0;1E10;0;1E10) runs for
/// minutes, and past 2⁶⁴ its integer conversion yields 0); Gnumeric gives
/// #VALUE! past 10,000 periods without the switch. 1e5 periods (274 years
/// of days) take a few milliseconds.
const VDB_MAX_PERIODS: f64 = 1e5;

fn vdb(
    cost: f64,
    salvage: f64,
    life: f64,
    start: f64,
    end: f64,
    factor: f64,
    no_switch: bool,
) -> f64 {
    let (istart, iend) = (approx_floor(start), approx_ceil(end));
    if no_switch {
        let mut sum = 0.0;
        let mut i = istart + 1.0;
        while i <= iend {
            let mut term = ddb(cost, salvage, life, i, factor);
            if i == istart + 1.0 {
                term *= end.min(istart + 1.0) - start;
            } else if i == iend {
                term *= end + 1.0 - iend;
            }
            sum += term;
            i += 1.0;
        }
        return sum;
    }
    let mut part = 0.0;
    if !approx_eq(start, istart) {
        let rest = cost - inter_vdb(cost, salvage, life, life, istart, factor);
        part += (start - istart) * inter_vdb(rest, salvage, life, life - istart, 1.0, factor);
    }
    if !approx_eq(end, iend) {
        let before = iend - 1.0;
        let rest = cost - inter_vdb(cost, salvage, life, life, before, factor);
        part += (iend - end) * inter_vdb(rest, salvage, life, life - before, 1.0, factor);
    }
    let cost = cost - inter_vdb(cost, salvage, life, life, istart, factor);
    inter_vdb(cost, salvage, life, life - istart, iend - istart, factor) - part
}

fn db(cost: f64, salvage: f64, life: f64, period: f64, month: f64) -> f64 {
    let off = 1.0 - (salvage / cost).powf(1.0 / life);
    let off = approx_floor(off * 1000.0 + 0.5) / 1000.0;
    let first = cost * off * month / 12.0;
    if approx_floor(period) == 1.0 {
        return first;
    }
    let (mut sum, mut v) = (first, 0.0);
    let last = approx_floor(life.min(period));
    let mut i = 2.0;
    while i <= last {
        v = -(sum - cost) * off;
        sum += v;
        i += 1.0;
    }
    if period > life {
        v = -(sum - cost) * off * (12.0 - month) / 12.0;
    }
    v
}

fn depreciation(id: u32, a: &[CellValue]) -> R {
    let x = |i| n(a, i);
    match id {
        720 => {
            let life = x(2)?;
            if life == 0.0 {
                return Err(err::DIV0);
            }
            ok_num((x(0)? - x(1)?) / life)
        }
        721 => {
            let (cost, salvage, life, per) = (x(0)?, x(1)?, x(2)?, x(3)?);
            ok_num((cost - salvage) * (life - per + 1.0) / (life * (life + 1.0) / 2.0))
        }
        722 => {
            let (cost, salvage, life, period) = (x(0)?, x(1)?, x(2)?, x(3)?);
            let month = approx_floor(n_or(a, 4, 12.0)?);
            domain(
                (1.0..=12.0).contains(&month)
                    && life <= 1200.0
                    && salvage >= 0.0
                    && period <= life + 1.0
                    && salvage <= cost
                    && cost > 0.0
                    && life > 0.0
                    && period > 0.0,
            )?;
            ok_num(db(cost, salvage, life, period, month))
        }
        723 => {
            let (cost, salvage, life, period) = (x(0)?, x(1)?, x(2)?, x(3)?);
            let factor = n_or(a, 4, 2.0)?;
            domain(
                cost >= 0.0
                    && salvage >= 0.0
                    && factor > 0.0
                    && salvage <= cost
                    && period >= 1.0
                    && period <= life,
            )?;
            ok_num(ddb(cost, salvage, life, period, factor))
        }
        724 => {
            let (cost, salvage, life, start, end) = (x(0)?, x(1)?, x(2)?, x(3)?, x(4)?);
            let factor = n_or(a, 5, 2.0)?;
            let no_switch = a.len() > 6 && b(a, 6)?;
            domain(
                start >= 0.0
                    && end >= start
                    && end <= life
                    && approx_ceil(end) <= VDB_MAX_PERIODS
                    && cost >= 0.0
                    && salvage <= cost
                    && factor > 0.0,
            )?;
            ok_num(vdb(cost, salvage, life, start, end, factor, no_switch))
        }
        _ => bonds::amortize(id == 725, a),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::super::{eval as call, DAY_MS};
    use super::*;
    use improv_core_model::FuncId;

    fn f(id: u32, args: &[f64]) -> CellValue {
        let a: Vec<CellValue> = args.iter().map(|x| CellValue::num(*x)).collect();
        call(FuncId(id), &a)
    }

    fn close(v: CellValue, want: f64) {
        let got = v.as_num().unwrap_or_else(|| panic!("{v:?}, want {want}"));
        assert!(
            (got - want).abs() <= 1e-9 * want.abs().max(1.0),
            "{got} != {want}"
        );
    }

    #[test]
    fn mortgage_payment_sign() {
        close(
            f(702, &[0.05 / 12.0, 360.0, 200_000.0]),
            -1_073.643_246_024_278,
        );
        // type: any nonzero is the beginning of the period.
        assert_eq!(
            f(702, &[0.05 / 12.0, 360.0, 200_000.0, 0.0, 1.0]),
            f(702, &[0.05 / 12.0, 360.0, 200_000.0, 0.0, 7.0])
        );
        // rate 0 is linear.
        close(f(702, &[0.0, 10.0, 1000.0]), -100.0);
        close(f(700, &[0.0, 10.0, -100.0]), 1000.0);
        close(f(701, &[0.0, 10.0, -100.0, 0.0, 1.0]), 1000.0);
        close(f(703, &[0.0, 2.0, 10.0, 1000.0]), 0.0);
        close(f(704, &[0.0, 2.0, 10.0, 1000.0, 0.0, 1.0]), -100.0);
        close(f(705, &[0.0, -100.0, 1000.0]), 10.0);
        close(f(705, &[0.05, -100.0, 1000.0, -1000.0]), 0.0);
        assert_eq!(f(702, &[0.0, 0.0, 1000.0]), CellValue::Err(err::NUM));
        assert_eq!(f(703, &[0.1, 0.0, 10.0, 1000.0]), CellValue::Err(err::NUM));
    }

    #[test]
    fn rate_newton_then_bisection() {
        // Round trip through PMT.
        let p = pmt(0.004, 360.0, 200_000.0, 0.0, false);
        close(f(706, &[360.0, p, 200_000.0]), 0.004);
        close(f(706, &[360.0, p, 200_000.0, 0.0, 0.0, 0.9]), 0.004);
        // Zero rate root.
        close(f(706, &[10.0, -100.0, 1000.0]), 0.0);
        // A guess at or below -1 starts with bisection.
        close(f(706, &[10.0, -100.0, 1000.0, 0.0, 0.0, -2.0]), 0.0);
        // Newton diverges from a bad guess; bisection recovers the root.
        let r = rate(10.0, 0.0, -100.0, 200.0, false, 1e6).unwrap();
        assert!((r - 2f64.powf(0.1) + 1.0).abs() < 1e-12, "{r}");
        // No root: all inflows.
        assert_eq!(f(706, &[10.0, 100.0, 1000.0]), CellValue::Err(err::NUM));
        assert_eq!(f(706, &[0.0, -100.0, 1000.0]), CellValue::Err(err::NUM));
        // Paying in advance.
        let p = pmt(0.01, 48.0, 8000.0, 0.0, true);
        close(f(706, &[48.0, p, 8000.0, 0.0, 1.0]), 0.01);
        // Newton from -95% for 2^0.1 - 1 steps below -100%: bisection takes
        // over and finds the root (LibreOffice and Gnumeric give up there:
        // Err:523 and #NUM!).
        let r = rate(10.0, 0.0, -100.0, 200.0, false, -0.95).unwrap();
        assert!((r - 2f64.powf(0.1) + 1.0).abs() < 1e-12, "{r}");
        // A step to a non-finite rate (a zero derivative): bisection.
        let flat = solve(&|x: f64| x - 0.5, &|_| 0.0, 0.1, -1.0).unwrap();
        assert!((flat - 0.5).abs() < 1e-12, "{flat}");
        // Two sign changes on the grid (roots 0 and 1 of x(1 - x)): the
        // bracket nearest the guess wins.
        let two = |x: f64| -(x * (x - 1.0));
        assert!(bisect(&two, 0.9, -1.0).is_some_and(|r| (r - 1.0).abs() < 1e-12));
        assert!(bisect(&two, -0.2, -1.0).is_some_and(|r| r.abs() < 1e-12));
        // The derivative at x = 0 (Newton starting there).
        close(f(706, &[10.0, -100.0, 1000.0, 0.0, 1.0, 0.0]), 0.0);
    }

    #[test]
    fn cumulative_and_misc_tvm() {
        let ci = f(707, &[0.0075, 360.0, 125_000.0, 1.0, 12.0, 1.0]);
        let cp = f(708, &[0.0075, 360.0, 125_000.0, 1.0, 12.0, 1.0]);
        let p = pmt(0.0075, 360.0, 125_000.0, 0.0, true);
        close(
            CellValue::num(ci.as_num().unwrap() + cp.as_num().unwrap()),
            12.0 * p,
        );
        assert_eq!(
            f(707, &[0.0075, 360.0, 125_000.0, 1.0, 12.0, 2.0]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(
            f(708, &[0.0, 360.0, 125_000.0, 1.0, 12.0, 0.0]),
            CellValue::Err(err::NUM)
        );
        close(f(710, &[0.0525, 4.9]), 0.053_542_667_370_758_19);
        assert_eq!(f(710, &[0.0, 4.0]), CellValue::Err(err::NUM));
        assert_eq!(f(711, &[0.05, 0.5]), CellValue::Err(err::NUM));
        assert_eq!(f(712, &[0.0, 1.0, 2.0]), CellValue::Err(err::NUM));
        assert_eq!(f(713, &[0.0, 1.0, 2.0]), CellValue::Err(err::NUM));
        assert_eq!(f(714, &[1.02, -1.0]), CellValue::Err(err::NUM));
        assert_eq!(f(715, &[1.02, 0.5]), CellValue::Err(err::DIV0));
        close(f(714, &[1.5, 10.0]), 1.5);
        close(f(715, &[-1.125, 16.0]), -1.02);
    }

    #[test]
    fn depreciation_edges() {
        assert_eq!(f(720, &[100.0, 10.0, 0.0]), CellValue::Err(err::DIV0));
        // DDB: a factor over the life writes everything off in period 1.
        close(f(723, &[1000.0, 100.0, 2.0, 1.0, 4.0]), 900.0);
        close(f(723, &[1000.0, 100.0, 2.0, 2.0, 4.0]), 0.0);
        assert_eq!(f(723, &[1000.0, 100.0, 5.0, 6.0]), CellValue::Err(err::NUM));
        // DB past the life uses the remaining months.
        assert!(f(722, &[1000.0, 100.0, 3.0, 4.0, 6.0]).as_num().unwrap() > 0.0);
        assert_eq!(
            f(722, &[1000.0, 100.0, 3.0, 1.0, 13.0]),
            CellValue::Err(err::NUM)
        );
        // VDB with no_switch over a fractional end.
        let a = [2400.0, 300.0, 10.0, 1.0, 2.5, 2.0].map(CellValue::num);
        let mut a = a.to_vec();
        a.push(CellValue::Bool(true));
        // 384 (period 2) + half of period 3's 307.2.
        close(call(FuncId(724), &a), 537.6);
        assert_eq!(
            f(724, &[2400.0, 300.0, 10.0, 3.0, 2.0]),
            CellValue::Err(err::NUM)
        );
        assert!(approx_eq(0.1 + 0.2, 0.3) && !approx_eq(1.0, 1.0001));
        assert_eq!(approx_ceil(2.000_000_000_000_000_4), 2.0);
        assert_eq!(approx_ceil(2.1), 3.0);
        let _ = DAY_MS;
    }

    /// VDB over more than 1e5 periods is #NUM!; it walked every period, and
    /// past 2⁵³ its f64 period counter stopped advancing (a hang).
    #[test]
    fn vdb_caps_its_periods() {
        let t0 = std::time::Instant::now();
        let num = CellValue::Err(super::super::err::NUM);
        assert_eq!(f(724, &[1000.0, 0.0, 9.47e153, 0.0, 9.47e153]), num);
        assert_eq!(f(724, &[1000.0, 0.0, 2e5, 0.0, 2e5]), num);
        assert_eq!(f(724, &[1000.0, 0.0, 1e17, 1e17, 1e17, 2.0, 1.0]), num);
        close(f(724, &[1000.0, 0.0, 1e5, 0.0, 1e5]), 1000.0);
        let a = [1000.0, 10.0, 1e5, 0.5, 1e5 - 0.5, 2.0, 1.0];
        assert!(f(724, &a).as_num().is_some());
        // A huge life with a small end period still evaluates (LibreOffice
        // and Gnumeric 5.27983104540655E-151).
        let v = f(724, &[1000.0, 0.0, 9.47e153, 0.0, 5.0]).as_num().unwrap();
        assert!((v / 5.279_831_045_406_55e-151 - 1.0).abs() < 1e-12, "{v}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
    }

    /// PV and FV at a tiny rate keep their digits (Gnumeric, and mpmath
    /// 9.9999999945000000022 and 10.0000000045000000012); (1+r)ⁿ − 1 lost
    /// seven of them, so PV of PMT's payment missed the present value.
    #[test]
    fn pv_and_fv_at_a_tiny_rate() {
        let near = |v: CellValue, want: f64| {
            let got = v.as_num().unwrap();
            assert!((got - want).abs() <= 1e-15 * want.abs(), "{got} vs {want}");
        };
        near(f(701, &[1e-10, 10.0, -1.0]), 9.999_999_994_5);
        near(f(700, &[1e-10, 10.0, -1.0]), 10.000_000_004_5);
        near(f(701, &[1e-10, 10.0, -1.0, 0.0, 1.0]), 9.999_999_995_5);
        near(f(700, &[1e-10, 10.0, -1.0, 0.0, 1.0]), 10.000_000_005_5);
        let p = pmt(6.1e-8, 1.0, 0.0, 1.0, false);
        assert!(pv(6.1e-8, 1.0, p, 1.0, false).abs() < 1e-15);
        // A large rate: (1+r)⁻ⁿ = 1.5⁻⁴⁹ is 2.4e-9, and 1 − (1 − it) lost
        // half its digits.
        let p = pmt(0.5, 49.0, 0.0, 1.0, true);
        let back = pv(0.5, 49.0, p, 1.0, true);
        assert!(back.abs() < 1e-23, "{back}");
    }

    /// CUMIPMT and CUMPRINC over more than 1e7 periods are #NUM!: they summed
    /// every period, and past 2⁵³ the counter stopped advancing (a hang).
    #[test]
    fn cumulative_caps_its_span() {
        let t0 = std::time::Instant::now();
        let num = CellValue::Err(err::NUM);
        assert_eq!(f(707, &[1e-300, 1e15, 1e6, 1.0, 1e15, 0.0]), num);
        assert_eq!(f(708, &[0.05, 1e300, 1e6, 1.0, 1e300, 1.0]), num);
        assert_eq!(f(707, &[0.05, 2e7, 1e6, 1e7, 2e7, 0.0]), num);
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
        // Gnumeric -0.5000838333319472 over a million periods.
        close(
            f(707, &[1e-9, 1e6, 1000.0, 1.0, 1e6, 0.0]),
            -0.500_083_833_331_947,
        );
    }
}
