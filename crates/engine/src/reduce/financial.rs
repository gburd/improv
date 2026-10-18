//! The financial reductions (ids 1150–1199): cash flows in item order.
//!
//! Every function reads the rows that are present, in item order: the first
//! row is the first period. A period with no cell is a missing flow, not a
//! zero, so a flow's period is its ordinal among the present rows (Excel's
//! rule for a range with blank cells). Enter 0 for a period with no flow.

use super::Rows;
use crate::builtins::{d, err, n, n_or, num, serial};
use crate::CellValue;

/// Reduce one group (an error among the rows is the result).
pub(super) fn reduce(id: u32, rows: &Rows, p: &[CellValue]) -> CellValue {
    if let Some(bad) = rows.first_error() {
        return bad;
    }
    match eval(id, rows, p) {
        Ok(x) => num(x),
        Err(c) => CellValue::Err(c),
    }
}

fn eval(id: u32, rows: &Rows, p: &[CellValue]) -> Result<f64, u8> {
    match id {
        // NPV(rate, cf OVER T): the first flow is discounted one period.
        1150 => {
            let b = 1.0 + n(p, 0)?;
            Ok(flows(rows)
                .iter()
                .enumerate()
                .map(|(i, v)| v / b.powf(i as f64 + 1.0))
                .sum())
        }
        // XNPV(rate, cf, dates OVER T)
        1151 => {
            let rate = n(p, 0)?;
            let (v, t) = dated(rows)?;
            if rate <= -1.0 {
                return Err(err::NUM);
            }
            Ok(v.iter()
                .zip(&t)
                .map(|(v, t)| v / (1.0 + rate).powf(t / 365.0))
                .sum())
        }
        // IRR(cf OVER T, [guess])
        1152 => {
            let v = flows(rows);
            let t: Vec<f64> = (0..v.len()).map(|i| i as f64).collect();
            irr(&v, &t, n_or(p, 0, 0.1)?)
        }
        // XIRR(cf, dates OVER T, [guess])
        1153 => {
            let guess = n_or(p, 0, 0.1)?;
            let (v, t) = dated(rows)?;
            if guess <= -1.0 {
                return Err(err::NUM);
            }
            let t: Vec<f64> = t.iter().map(|t| t / 365.0).collect();
            irr(&v, &t, guess)
        }
        // MIRR(cf OVER T, finance_rate, reinvest_rate)
        1154 => {
            let (fin, re) = (1.0 + n(p, 0)?, 1.0 + n(p, 1)?);
            let v = flows(rows);
            if !mixed(&v) {
                return Err(err::DIV0);
            }
            let (mut neg, mut pos, mut dfin, mut dre) = (0.0, 0.0, 1.0, 1.0);
            for x in &v {
                if *x > 0.0 {
                    pos += x * dre;
                } else if *x < 0.0 {
                    neg += x * dfin;
                }
                dre /= re;
                dfin /= fin;
            }
            let m = (v.len() - 1) as f64;
            Ok((-pos / neg * re.powf(m)).powf(1.0 / m) - 1.0)
        }
        // FMRR(cf OVER T, safe_rate, reinvest_rate, reinvest_min)
        1155 => fmrr(flows(rows), n(p, 0)?, n(p, 1)?, n(p, 2)?),
        // FVSCHEDULE(principal, rate OVER T)
        1156 => Ok(flows(rows).iter().fold(n(p, 0)?, |acc, r| acc * (1.0 + r))),
        _ => Err(err::NAME),
    }
}

/// The numeric flows in item order (non-numbers are skipped, Excel's range
/// rule; a measure has one type, so there are none in practice).
fn flows(rows: &Rows) -> Vec<f64> {
    rows.col(0).filter_map(CellValue::as_num).collect()
}

/// Whether `v` has a positive and a negative flow.
fn mixed(v: &[f64]) -> bool {
    v.iter().any(|x| *x > 0.0) && v.iter().any(|x| *x < 0.0)
}

/// The flows and their days after the first date (`XNPV`, `XIRR`): at least
/// two, dates truncated to whole days, none before the first (Excel: #NUM!).
fn dated(rows: &Rows) -> Result<(Vec<f64>, Vec<f64>), u8> {
    let mut v = Vec::with_capacity(rows.len());
    let mut t = Vec::with_capacity(rows.len());
    for r in &rows.vals {
        v.push(r[0].as_num().ok_or(err::VALUE)?);
        t.push(serial(d(r, 1)?).floor());
    }
    if v.len() < 2 || t.iter().any(|x| *x < t[0]) {
        return Err(err::NUM);
    }
    let t0 = t[0];
    Ok((v, t.iter().map(|x| x - t0).collect()))
}

/// The rate `r` with `Σ v_i / (1 + r)^t_i = 0`: Newton's method from
/// `guess` (at most 100 steps, to 1e-10), else bisection on the first sign
/// change along a fixed grid of rates above -1. #NUM! without a positive and
/// a negative flow, or without a root above -100%.
fn irr(v: &[f64], t: &[f64], guess: f64) -> Result<f64, u8> {
    if !mixed(v) {
        return Err(err::NUM);
    }
    let f = |r: f64| {
        let b = 1.0 + r;
        v.iter().zip(t).fold((0.0, 0.0), |(f, df), (v, t)| {
            let x = v * b.powf(-t);
            (f + x, df - t * x / b)
        })
    };
    let mut x = guess;
    for _ in 0..100 {
        let (fx, dfx) = f(x);
        let next = x - fx / dfx;
        if !next.is_finite() {
            break;
        }
        if (next - x).abs() <= 1e-10 * x.abs().max(1.0) {
            if next > -1.0 {
                return Ok(next);
            }
            break;
        }
        x = next;
    }
    bisect(|r| f(r).0).ok_or(err::NUM)
}

/// A root of `f` in the first bracket along a fixed grid of rates (-0.999999
/// … 2^30), refined by bisection.
fn bisect(f: impl Fn(f64) -> f64) -> Option<f64> {
    let grid = [-0.999_999, -0.9999, -0.999]
        .into_iter()
        .chain((-99..=100).map(|k| f64::from(k) / 100.0))
        .chain((1..=30).map(|k| 2f64.powi(k)));
    let mut prev: Option<(f64, f64)> = None;
    for x in grid {
        let fx = f(x);
        if fx == 0.0 {
            return Some(x);
        }
        if fx.is_nan() {
            prev = None;
            continue;
        }
        if let Some((lo, flo)) = prev {
            if (flo < 0.0) != (fx < 0.0) {
                let (mut lo, mut hi) = (lo, x);
                for _ in 0..200 {
                    let mid = lo + (hi - lo) / 2.0;
                    if mid == lo || mid == hi {
                        break;
                    }
                    let fm = f(mid);
                    if fm == 0.0 {
                        return Some(mid);
                    }
                    if (fm < 0.0) == (flo < 0.0) {
                        lo = mid;
                    } else {
                        hi = mid;
                    }
                }
                return Some(lo + (hi - lo) / 2.0);
            }
        }
        prev = Some((x, fx));
    }
    None
}

/// GNU Oleo 1.99.16's `do_fmrr` (`src/busi.c`), on the flows `v`.
///
/// 1. From the last flow back, each negative flow is pulled back to the most
///    recent earlier positive flow `j > 0` at the safe rate (the positive flow
///    pays its discounted value); what no positive flow covers falls into the
///    initial flow, discounted to period 0.
/// 2. From period 1 on, a flow of at least `min` is pushed to the last period
///    at the reinvestment rate; a smaller one moves to the next period at the
///    safe rate.
/// 3. The result is the rate linking the first and last flows:
///    `(v_last / -v_0)^(1 / (n - 1)) - 1`.
///
/// Oleo caps a transfer at the positive flow and repeats it, so the positive
/// flow shrinks geometrically (by `1 - 1/c`, `c = (1 + safe)^k` for a flow
/// `k` periods later). Summed, the repeats transfer `min(-v_i, v_j c)`; that
/// sum is computed here in one step. (Where the positive flow cannot cover the
/// outflow and `c ≥ 2`, Oleo never ends: the shrinking flow sticks at the
/// smallest subnormal.) #NUM! with fewer than two flows, or without an
/// initial outflow after step 1 (Oleo returns 0 for one flow and a
/// meaningless rate or NaN for the other case).
fn fmrr(mut v: Vec<f64>, safe: f64, reinvest: f64, min: f64) -> Result<f64, u8> {
    let n = v.len();
    if n < 2 {
        return Err(err::NUM);
    }
    for i in (1..n).rev() {
        while v[i] < 0.0 {
            let Some(j) = (1..i).rev().find(|&j| v[j] > 0.0) else {
                v[0] -= -v[i] / (1.0 + safe).powf(i as f64);
                v[i] = 0.0;
                break;
            };
            let (x, y, c) = (-v[i], v[j], (1.0 + safe).powf((i - j) as f64));
            if x <= y || c <= 1.0 {
                // One transfer, as Oleo makes it.
                let amt = x.min(y);
                v[i] += amt;
                v[j] -= amt / c;
            } else if x < y * c {
                // The limit of Oleo's repeated capped transfers.
                v[i] = 0.0;
                v[j] = y - x / c;
            } else {
                v[i] += y * c;
                v[j] = 0.0;
            }
        }
    }
    let last = n - 1;
    for i in 1..last {
        if v[i] != 0.0 {
            if v[i] >= min {
                v[last] += v[i] * (1.0 + reinvest).powf((last - i) as f64);
                v[i] = 0.0;
            } else {
                v[i + 1] += v[i] * (1.0 + safe);
            }
        }
    }
    if v[0] >= 0.0 {
        return Err(err::NUM);
    }
    Ok((v[last] / -v[0]).powf(1.0 / last as f64) - 1.0)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::builtins::SERIAL_EPOCH_MS;

    fn n(x: f64) -> CellValue {
        CellValue::num(x)
    }

    fn rows(vals: Vec<Vec<CellValue>>) -> Rows {
        Rows {
            pos: (0..vals.len() as u32).map(|i| vec![i]).collect(),
            vals,
            capacity: 8,
        }
    }

    fn flows(v: &[f64]) -> Rows {
        rows(v.iter().map(|x| vec![n(*x)]).collect())
    }

    fn approx(got: CellValue, want: f64) {
        let g = got
            .as_num()
            .unwrap_or_else(|| panic!("{got:?}, want {want}"));
        assert!(
            (g - want).abs() <= 1e-12 * want.abs().max(1.0),
            "{g} != {want}"
        );
    }

    #[test]
    fn irr_falls_back_to_bisection() {
        // Roots 0 and -2: Newton from -1.5 finds -2, bisection then the root
        // above -100% (exactly on the grid).
        let r = flows(&[1.0, 0.0, -1.0]);
        approx(reduce(1152, &r, &[n(-1.5)]), 0.0);
        // -1 + 1/b - 1/b² has no root: #NUM!, whether Newton wanders or
        // hits the flat point (b = 2, rate 100%).
        let none = flows(&[-1.0, 1.0, -1.0]);
        assert_eq!(reduce(1152, &none, &[]), CellValue::Err(err::NUM));
        assert_eq!(reduce(1152, &none, &[n(1.0)]), CellValue::Err(err::NUM));
        // A guess of -100% starts Newton at a pole; bisection still finds it.
        let r = flows(&[-1000.0, 300.0, 400.0, 500.0]);
        approx(reduce(1152, &r, &[n(-1.0)]), 0.088_963_394_693_344_69);
    }

    #[test]
    fn bisection_skips_undefined_rates() {
        let root = bisect(|r| if r < 0.0 { f64::NAN } else { r - 0.5 }).unwrap();
        assert!((root - 0.5).abs() < 1e-15);
        assert_eq!(bisect(|_| 1.0), None);
        // A sign change with no zero between (a jump at 1/3): bisection
        // narrows to adjacent doubles and stops there.
        let jump = bisect(|r| if r < 1.0 / 3.0 { -1.0 } else { 1.0 }).unwrap();
        assert!((jump - 1.0 / 3.0).abs() <= f64::EPSILON);
        // IRR: no sign change in the flows is #NUM! before any search
        // (LibreOffice Err:523).
        assert_eq!(
            reduce(1152, &flows(&[1.0, 2.0, 3.0]), &[]),
            CellValue::Err(err::NUM)
        );
        // An IRR whose Newton steps from a -100% guess land at a pole
        // finds the root by bisection over the grid's defined rates
        // (LibreOffice 1/3 and 2/7).
        approx(reduce(1152, &flows(&[-3.0, 4.0]), &[n(-1.0)]), 1.0 / 3.0);
        approx(reduce(1152, &flows(&[-7.0, 9.0]), &[n(-1.0)]), 2.0 / 7.0);
        // -1 + 2·b⁻⁵² − b⁻⁵³ (b = 1 + r): at the grid's first rate both
        // powers overflow (∞ − ∞, skipped), and the first sign change
        // brackets the root just above -50% (u = 1/b solves 2u⁵² − u⁵³ = 1,
        // u = 2 − 2⁻⁵² to double precision), not the root at 0.
        let mut v = vec![0.0; 54];
        (v[0], v[52], v[53]) = (-1.0, 2.0, -1.0);
        approx(reduce(1152, &flows(&v), &[n(-1.0)]), -0.5);
    }

    #[test]
    fn dated_flows() {
        let day = |s: i64| CellValue::Date(SERIAL_EPOCH_MS + s * 86_400_000);
        let r = rows(vec![
            vec![n(-1000.0), day(45658)],
            vec![n(1100.0), day(46023)],
        ]);
        approx(reduce(1151, &r, &[n(0.1)]), 0.0);
        // XNPV at -100% or below (LibreOffice Err:502, Excel #NUM!).
        assert_eq!(reduce(1151, &r, &[n(-1.0)]), CellValue::Err(err::NUM));
        approx(reduce(1153, &r, &[]), 0.1);
        assert_eq!(reduce(1153, &r, &[n(-1.0)]), CellValue::Err(err::NUM));
        // A flow that is not a number, a date that is not a date.
        let text = rows(vec![
            vec![CellValue::Text("x".into()), day(1)],
            vec![n(1.0), day(2)],
        ]);
        assert_eq!(reduce(1151, &text, &[n(0.1)]), CellValue::Err(err::VALUE));
        let bad = rows(vec![
            vec![n(1.0), CellValue::Bool(true)],
            vec![n(1.0), day(2)],
        ]);
        assert_eq!(reduce(1153, &bad, &[]), CellValue::Err(err::VALUE));
    }

    #[test]
    fn mirr_fvschedule_and_errors() {
        let r = flows(&[-500.0]);
        assert_eq!(
            reduce(1154, &r, &[n(0.1), n(0.1)]),
            CellValue::Err(err::DIV0)
        );
        approx(reduce(1156, &flows(&[0.1, 0.1]), &[n(100.0)]), 121.0);
        // A zero flow still counts as a period.
        let z = flows(&[-100.0, 0.0, 121.0]);
        approx(reduce(1154, &z, &[n(0.0), n(0.0)]), 0.1);
        // A missing parameter, an unknown id, a non-number flow skipped.
        assert_eq!(reduce(1150, &r, &[]), CellValue::Err(err::VALUE));
        assert_eq!(reduce(1199, &r, &[]), CellValue::Err(err::NAME));
        let mixed = rows(vec![
            vec![n(-100.0)],
            vec![CellValue::Bool(true)],
            vec![n(110.0)],
        ]);
        approx(reduce(1150, &mixed, &[n(0.1)]), -100.0 / 1.1 + 110.0 / 1.21);
        // Through the dispatcher in reduce.rs.
        approx(
            super::super::reduce(improv_core_model::FuncId(1156), &flows(&[1.0]), &[n(3.0)]),
            6.0,
        );
    }

    #[test]
    fn fmrr_follows_oleo() {
        let f = |v: &[f64], s: f64, r: f64, m: f64| fmrr(v.to_vec(), s, r, m);
        // Two flows: the growth over one period.
        approx(num(f(&[-100.0, 121.0], 0.05, 0.1, 0.0).unwrap()), 0.21);
        // A negative safe rate: the inflow pays more than it covers, goes
        // negative, and that falls into period 0 (Oleo visits it next).
        let v = f(&[-1000.0, 100.0, -100.0, 800.0], -0.1, 0.1, 0.0).unwrap();
        let v0: f64 = -1000.0 - (100.0 / 0.9 - 100.0) / 0.9;
        approx(num(v), (800.0 / -v0).powf(1.0 / 3.0) - 1.0);
        // Small flows roll forward at the safe rate, large ones jump to the end.
        let v = f(&[-1000.0, 10.0, 500.0, 600.0], 0.05, 0.1, 100.0).unwrap();
        let last: f64 = 600.0 + (500.0 + 10.0 * 1.05) * 1.1;
        approx(num(v), (last / 1000.0).powf(1.0 / 3.0) - 1.0);
        assert_eq!(f(&[1.0], 0.0, 0.0, 0.0), Err(err::NUM));
        assert_eq!(f(&[100.0, -50.0], 0.0, 0.0, 0.0), Err(err::NUM));
    }
}
