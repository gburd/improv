//! The statistical reductions (P4, ids 1000–1149): descriptive and
//! conditional statistics, order statistics, ranks, regression and the
//! classical tests, over one group's [`Rows`].
//!
//! Values follow Excel's range rule: the numeric functions read numbers (and
//! dates, as serial numbers) and skip anything else; the `…A` functions count
//! TRUE as 1 and FALSE and text as 0. An error among the rows is the result
//! (the first in item order), except where a function counts or skips
//! errors: `COUNT` skips them, `COUNTA` counts them, the `…IF`/`…IFS`
//! functions skip rows whose criteria cell is an error, and `AGGREGATE`'s
//! options 2, 3, 6 and 7 ignore them. Sums are compensated (Neumaier), as
//! LibreOffice's are. The array results (`FREQUENCY`, `LINEST`, `LOGEST`)
//! wait for P6.

use super::Rows;
use crate::builtins::statistical::{f_sf, gamma_q, norm_cdf, t_sf};
use crate::builtins::{b, err, fold_char, n, n_or, num, serial};
use crate::CellValue;
use std::cmp::Ordering;
use std::collections::HashMap;

type Num = Result<f64, u8>;

/// Reduce one group with statistical aggregation `id`.
pub(super) fn reduce(id: u32, rows: &Rows, a: &[CellValue]) -> CellValue {
    match eval(id, rows, a) {
        Ok(x) => num(x),
        Err(c) => CellValue::Err(c),
    }
}

/// One result per row: `RANK`, `PERCENTRANK`, `TREND`, `GROWTH`.
pub(super) fn per_item(id: u32, rows: &Rows, a: &[CellValue]) -> Vec<CellValue> {
    match no_error(rows).and_then(|()| per_item_ok(id, rows, a)) {
        Ok(v) => v
            .into_iter()
            .map(|x| x.map_or_else(CellValue::Err, num))
            .collect(),
        Err(c) => vec![CellValue::Err(c); rows.len()],
    }
}

/// The first error among the rows, as an `Err`.
fn no_error(rows: &Rows) -> Result<(), u8> {
    match rows.first_error() {
        Some(CellValue::Err(c)) => Err(c),
        _ => Ok(()),
    }
}

/// A cell as a number under the range rule (numbers, and dates as serials).
pub(super) fn number(v: &CellValue) -> Option<f64> {
    match v {
        CellValue::Num(bits) => Some(f64::from_bits(*bits)),
        CellValue::Date(ms) => Some(serial(*ms)),
        _ => None,
    }
}

/// A cell as a number for the `…A` functions: TRUE 1, FALSE and text 0.
fn number_a(v: &CellValue) -> Option<f64> {
    match v {
        CellValue::Bool(t) => Some(f64::from(u8::from(*t))),
        CellValue::Text(_) => Some(0.0),
        _ => number(v),
    }
}

/// A compensated (Neumaier) sum.
fn sum(xs: impl IntoIterator<Item = f64>) -> f64 {
    let (mut s, mut c) = (0.0_f64, 0.0_f64);
    for x in xs {
        let t = s + x;
        c += if s.abs() >= x.abs() {
            (s - t) + x
        } else {
            (x - t) + s
        };
        s = t;
    }
    s + c
}

fn mean(xs: &[f64]) -> Num {
    if xs.is_empty() {
        return Err(err::DIV0);
    }
    Ok(sum(xs.iter().copied()) / xs.len() as f64)
}

/// Σ(x − mean)².
fn devsq(xs: &[f64]) -> Num {
    let m = mean(xs)?;
    Ok(sum(xs.iter().map(|x| (x - m) * (x - m))))
}

/// The sample (n − 1) or population (n) variance.
fn var(xs: &[f64], sample: bool) -> Num {
    let d = xs.len() as f64 - f64::from(u8::from(sample));
    if d <= 0.0 {
        return Err(err::DIV0);
    }
    Ok(devsq(xs)? / d)
}

/// Σ((x − mean) / s)ᵖ, s the sample or population standard deviation.
fn std_moment(xs: &[f64], sample: bool, p: i32) -> Num {
    let m = mean(xs)?;
    let s = var(xs, sample)?.sqrt();
    if s == 0.0 {
        return Err(err::DIV0);
    }
    Ok(sum(xs.iter().map(|x| ((x - m) / s).powi(p))))
}

/// The largest or smallest value, 0 when there is none.
fn extreme(xs: &[f64], max: bool) -> f64 {
    let pick = if max { f64::max } else { f64::min };
    xs.iter().copied().reduce(pick).unwrap_or(0.0)
}

/// `x` snapped to the nearest integer when within a few ulps of it, so
/// `0.7 * 10` floors to 7 (LibreOffice's `approxFloor`).
fn snap(x: f64) -> f64 {
    let r = x.round();
    if (x - r).abs() <= 4.0 * f64::EPSILON * x.abs() {
        r
    } else {
        x
    }
}

fn sorted(mut xs: Vec<f64>) -> Vec<f64> {
    xs.sort_by(f64::total_cmp);
    xs
}

/// The value at 0-based fractional rank `h` ∈ [0, n − 1] of sorted `xs`.
fn interp(xs: &[f64], h: f64) -> f64 {
    let lo = snap(h).floor();
    let i = lo as usize;
    let d = h - lo;
    if d <= 0.0 || i + 1 >= xs.len() {
        xs[i]
    } else {
        xs[i] + d * (xs[i + 1] - xs[i])
    }
}

fn pct_inc(xs: Vec<f64>, p: f64) -> Num {
    if xs.is_empty() || !(0.0..=1.0).contains(&p) {
        return Err(err::NUM);
    }
    let xs = sorted(xs);
    Ok(interp(&xs, p * (xs.len() - 1) as f64))
}

fn pct_exc(xs: Vec<f64>, p: f64) -> Num {
    let n1 = (xs.len() + 1) as f64;
    if xs.is_empty() || p * n1 < 1.0 || p * n1 > n1 - 1.0 {
        return Err(err::NUM);
    }
    Ok(interp(&sorted(xs), p * n1 - 1.0))
}

/// The most frequent value; on a tie the one that occurs first.
fn mode(xs: &[f64]) -> Num {
    let mut seen: HashMap<u64, (usize, usize)> = HashMap::new();
    for (i, x) in xs.iter().enumerate() {
        seen.entry((x + 0.0).to_bits()).or_insert((0, i)).0 += 1;
    }
    match seen
        .into_iter()
        .map(|(bits, (count, first))| (count, std::cmp::Reverse(first), bits))
        .max()
    {
        Some((count, _, bits)) if count > 1 => Ok(f64::from_bits(bits)),
        _ => Err(err::NA),
    }
}

/// AGGREGATE's numeric reductions by code (1, 4–19); `k` for 14–19.
fn on_nums(code: u32, xs: Vec<f64>, k: f64) -> Num {
    match code {
        1 => mean(&xs),
        4 => Ok(extreme(&xs, true)),
        5 => Ok(extreme(&xs, false)),
        6 if xs.is_empty() => Ok(0.0),
        6 => Ok(xs.iter().product()),
        7 | 8 => Ok(var(&xs, code == 7)?.sqrt()),
        9 => Ok(sum(xs)),
        10 | 11 => var(&xs, code == 10),
        12 => {
            if xs.is_empty() {
                return Err(err::NUM);
            }
            let xs = sorted(xs);
            let h = xs.len() / 2;
            Ok(if xs.len() % 2 == 1 {
                xs[h]
            } else {
                (xs[h - 1] + xs[h]) / 2.0
            })
        }
        13 => mode(&xs),
        14 | 15 => {
            let k = snap(k).floor();
            if k < 1.0 || k > xs.len() as f64 {
                return Err(err::NUM);
            }
            let xs = sorted(xs);
            let i = k as usize - 1;
            Ok(if code == 14 {
                xs[xs.len() - 1 - i]
            } else {
                xs[i]
            })
        }
        16 => pct_inc(xs, k),
        17 => {
            let q = snap(k).floor();
            if !(0.0..=4.0).contains(&q) {
                return Err(err::NUM);
            }
            pct_inc(xs, q / 4.0)
        }
        18 => pct_exc(xs, k),
        _ => {
            let q = snap(k).floor();
            if q <= 0.0 || q >= 4.0 {
                return Err(err::NUM);
            }
            pct_exc(xs, q / 4.0)
        }
    }
}

/// SUBTOTAL/AGGREGATE code `code` over the cells (errors already handled).
fn by_code(code: u32, cells: &[&CellValue], k: f64) -> Num {
    match code {
        2 => Ok(cells.iter().filter(|v| number(v).is_some()).count() as f64),
        3 => Ok(cells.len() as f64),
        _ => on_nums(code, cells.iter().filter_map(|v| number(v)).collect(), k),
    }
}

/// A code argument as an integer in `range`, else `#VALUE!`.
fn code_in(x: f64, range: std::ops::RangeInclusive<u32>) -> Result<u32, u8> {
    let c = x.trunc();
    if c >= 0.0 && range.contains(&(c as u32)) {
        Ok(c as u32)
    } else {
        Err(err::VALUE)
    }
}

/// `AGGREGATE(code, options, x OVER T, [k])`.
fn aggregate(rows: &Rows, a: &[CellValue]) -> Num {
    let code = code_in(n(a, 0)?, 1..=19)?;
    let options = code_in(n(a, 1)?, 0..=7)?;
    let k = if code >= 14 {
        n(a, 2).map_err(|_| err::VALUE)?
    } else {
        0.0
    };
    if !matches!(options, 2 | 3 | 6 | 7) {
        no_error(rows)?;
    }
    let cells: Vec<&CellValue> = rows
        .col(0)
        .filter(|v| !matches!(v, CellValue::Err(_)))
        .collect();
    by_code(code, &cells, k)
}

/// The (x, y) pairs of reduced measures `xi` and `yi`, rows where both are numbers.
fn pairs(rows: &Rows, xi: usize, yi: usize) -> Vec<(f64, f64)> {
    rows.vals
        .iter()
        .filter_map(|r| Some((number(r.get(xi)?)?, number(r.get(yi)?)?)))
        .collect()
}

/// Means and centred sums of (x, y) pairs.
struct Fit {
    n: f64,
    mx: f64,
    my: f64,
    sxx: f64,
    syy: f64,
    sxy: f64,
}

fn fit(ps: &[(f64, f64)]) -> Result<Fit, u8> {
    if ps.is_empty() {
        return Err(err::DIV0);
    }
    let n = ps.len() as f64;
    let mx = sum(ps.iter().map(|p| p.0)) / n;
    let my = sum(ps.iter().map(|p| p.1)) / n;
    Ok(Fit {
        n,
        mx,
        my,
        sxx: sum(ps.iter().map(|p| (p.0 - mx) * (p.0 - mx))),
        syy: sum(ps.iter().map(|p| (p.1 - my) * (p.1 - my))),
        sxy: sum(ps.iter().map(|p| (p.0 - mx) * (p.1 - my))),
    })
}

/// The least-squares line y = a + b·x (through the origin without `konst`).
fn line(ps: &[(f64, f64)], konst: bool) -> Result<(f64, f64), u8> {
    if konst {
        let f = fit(ps)?;
        if f.sxx == 0.0 {
            return Err(err::DIV0);
        }
        let b = f.sxy / f.sxx;
        return Ok((f.my - b * f.mx, b));
    }
    let sxx = sum(ps.iter().map(|p| p.0 * p.0));
    if sxx == 0.0 {
        return Err(err::DIV0);
    }
    Ok((0.0, sum(ps.iter().map(|p| p.0 * p.1)) / sxx))
}

/// The pairs with y replaced by ln y (#NUM! for y ≤ 0).
fn ln_y(ps: &[(f64, f64)]) -> Result<Vec<(f64, f64)>, u8> {
    ps.iter()
        .map(|&(x, y)| {
            if y > 0.0 {
                Ok((x, y.ln()))
            } else {
                Err(err::NUM)
            }
        })
        .collect()
}

/// ln y = a + b·x by least squares, each point weighted by its y (the
/// better estimate of y = A·e^(Bx): Weisstein, "Least Squares Fitting,
/// Exponential", MathWorld); `simple` drops the weights (LOGEST's fit).
fn exp_fit(ps: &[(f64, f64)], simple: bool) -> Result<(f64, f64), u8> {
    let lp = ln_y(ps)?;
    if simple {
        return line(&lp, true);
    }
    let w = sum(ps.iter().map(|p| p.1));
    let mx = sum(ps.iter().map(|p| p.1 * p.0)) / w;
    let ml = sum(ps.iter().zip(&lp).map(|(p, q)| p.1 * q.1)) / w;
    let sxx = sum(ps.iter().map(|p| p.1 * (p.0 - mx) * (p.0 - mx)));
    if sxx == 0.0 {
        return Err(err::DIV0);
    }
    let b = sum(ps
        .iter()
        .zip(&lp)
        .map(|(p, q)| p.1 * (p.0 - mx) * (q.1 - ml)))
        / sxx;
    Ok((ml - b * mx, b))
}

/// Optional boolean parameter `i`, or `default`.
fn flag(a: &[CellValue], i: usize, default: bool) -> Result<bool, u8> {
    if a.len() > i {
        b(a, i)
    } else {
        Ok(default)
    }
}

fn eval(id: u32, rows: &Rows, a: &[CellValue]) -> Num {
    match id {
        1005 => return Ok(rows.col(0).filter(|v| number(v).is_some()).count() as f64),
        1006 => return Ok(rows.len() as f64),
        1007 => {
            let filled = rows
                .col(0)
                .filter(|v| !matches!(v, CellValue::Text(s) if s.is_empty()))
                .count();
            return Ok(rows.capacity.saturating_sub(filled) as f64);
        }
        1002 | 1003 | 1008..=1011 | 1013 | 1015 => return conditional(id, rows, a),
        1042 => return aggregate(rows, a),
        _ => {}
    }
    no_error(rows)?;
    let x0 = || rows.col(0).filter_map(number).collect::<Vec<f64>>();
    let xa = || rows.col(0).filter_map(number_a).collect::<Vec<f64>>();
    let k = || n_or(a, 0, 0.0);
    match id {
        1000 => Ok(sum(xa())),
        1001 => mean(&xa()),
        1012 | 1014 => Ok(extreme(&xa(), id == 1012)),
        1024 | 1025 => Ok(var(&xa(), id == 1024)?.sqrt()),
        1028 | 1029 => var(&xa(), id == 1028),
        1016 | 1017 | 1022 | 1023 | 1026 | 1027 | 1034..=1039 => {
            let code = match id {
                1016 => 12,
                1017 => 13,
                1022 => 7,
                1023 => 8,
                1026 => 10,
                1027 => 11,
                1034 => 14,
                1035 => 15,
                1036 => 16,
                1037 => 18,
                1038 => 17,
                _ => 19,
            };
            on_nums(code, x0(), k()?)
        }
        1004 | 1018..=1021 | 1030..=1033 => shape(id, &x0(), a),
        1040 => prob(rows, a),
        1041 => {
            let c = n(a, 0)?.trunc();
            let c = if (101.0..=111.0).contains(&c) {
                c - 100.0
            } else {
                c
            };
            let code = code_in(c, 1..=11)?;
            by_code(code, &rows.col(0).collect::<Vec<_>>(), 0.0)
        }
        1043 => Ok(sum(rows.vals.iter().map(|r| {
            r.iter().map(|v| number(v).unwrap_or(0.0)).product::<f64>()
        }))),
        1044..=1046 => {
            let f = match id {
                1044 => |x: f64, y: f64| x * x - y * y,
                1045 => |x: f64, y: f64| x * x + y * y,
                _ => |x: f64, y: f64| (x - y) * (x - y),
            };
            Ok(sum(pairs(rows, 0, 1).into_iter().map(|(x, y)| f(x, y))))
        }
        1047 | 1048 => {
            let x = n(a, 0)?;
            let (from, step) = if id == 1047 {
                (n(a, 1)?, n(a, 2)?)
            } else {
                (0.0, 1.0)
            };
            // The i-th coefficient is the item at position i along T (a
            // missing item is a zero coefficient); along several OVER
            // categories, the i-th row.
            let mut terms = Vec::new();
            for (r, row) in rows.vals.iter().enumerate() {
                let Some(c) = row.first().and_then(number) else {
                    continue;
                };
                let i = match rows.pos[r].as_slice() {
                    [p] => f64::from(*p),
                    _ => r as f64,
                };
                terms.push(c * pow(x, from + i * step)?);
            }
            Ok(sum(terms))
        }
        1049 => {
            let all = sum(rows.col(1).filter_map(number));
            if all == 0.0 {
                return Err(err::DIV0);
            }
            Ok(sum(x0()) / all)
        }
        1060..=1070 => regression(id, rows, a),
        1080..=1084 => test(id, rows, a),
        _ => Err(err::NAME),
    }
}

/// `x ^ y` with the spreadsheet edge cases (0⁻¹ is #DIV/0!, a negative base
/// with a fractional exponent #NUM!).
fn pow(x: f64, y: f64) -> Num {
    if x == 0.0 && y < 0.0 {
        return Err(err::DIV0);
    }
    if x < 0.0 && y.fract() != 0.0 {
        return Err(err::NUM);
    }
    Ok(x.powf(y))
}

/// AVEDEV, GEOMEAN, HARMEAN, TRIMMEAN, DEVSQ, KURT, KURTP, SKEW, SKEWP.
fn shape(id: u32, xs: &[f64], a: &[CellValue]) -> Num {
    let c = xs.len() as f64;
    match id {
        1004 | 1021 if xs.is_empty() => Err(err::NUM),
        1004 => {
            let m = mean(xs)?;
            Ok(sum(xs.iter().map(|x| (x - m).abs())) / c)
        }
        1018 | 1019 => {
            if xs.is_empty() || xs.iter().any(|x| *x <= 0.0) {
                return Err(err::NUM);
            }
            Ok(if id == 1018 {
                (sum(xs.iter().map(|x| x.ln())) / c).exp()
            } else {
                c / sum(xs.iter().map(|x| 1.0 / x))
            })
        }
        1020 => {
            let frac = n(a, 0)?;
            if xs.is_empty() || !(0.0..1.0).contains(&frac) {
                return Err(err::NUM);
            }
            let cut = snap(frac * c).floor() as usize / 2;
            let xs = sorted(xs.to_vec());
            mean(&xs[cut..xs.len() - cut])
        }
        1021 => devsq(xs),
        1030 => {
            if c < 4.0 {
                return Err(err::DIV0);
            }
            let s4 = std_moment(xs, true, 4)?;
            let kd = (c - 2.0) * (c - 3.0);
            Ok(s4 * (c * (c + 1.0) / ((c - 1.0) * kd)) - 3.0 * (c - 1.0) * (c - 1.0) / kd)
        }
        1031 => Ok(std_moment(xs, false, 4)? / c - 3.0),
        _ => {
            if c < 3.0 {
                return Err(err::DIV0);
            }
            let sample = id == 1032;
            let s3 = std_moment(xs, sample, 3)?;
            Ok(if sample {
                s3 * c / (c - 1.0) / (c - 2.0)
            } else {
                s3 / c
            })
        }
    }
}

/// `PROB(x, p OVER T, low, [high])`.
fn prob(rows: &Rows, a: &[CellValue]) -> Num {
    let ps = pairs(rows, 0, 1);
    let lo = n(a, 0)?;
    let hi = n_or(a, 1, lo)?;
    let (lo, hi) = if lo > hi { (hi, lo) } else { (lo, hi) };
    if ps.iter().any(|p| p.1 <= 0.0 || p.1 > 1.0)
        || (sum(ps.iter().map(|p| p.1)) - 1.0).abs() > 1e-7
    {
        return Err(err::NUM);
    }
    Ok(sum(ps
        .iter()
        .filter(|p| (lo..=hi).contains(&p.0))
        .map(|p| p.1)))
}

/// CORREL … EXPFORECAST.
fn regression(id: u32, rows: &Rows, a: &[CellValue]) -> Num {
    match id {
        // (x, y OVER T)
        1060..=1063 => {
            let f = fit(&pairs(rows, 0, 1))?;
            match id {
                1062 => Ok(f.sxy / f.n),
                1063 if f.n < 2.0 => Err(err::DIV0),
                1063 => Ok(f.sxy / (f.n - 1.0)),
                _ if f.sxx < f64::MIN_POSITIVE || f.syy < f64::MIN_POSITIVE => Err(err::DIV0),
                _ => {
                    let r = f.sxy / (f.sxx * f.syy).sqrt();
                    Ok(if id == 1061 { r * r } else { r })
                }
            }
        }
        // (y, x OVER T)
        1064 | 1065 => {
            let (a0, b0) = line(&pairs(rows, 1, 0), true)?;
            Ok(if id == 1064 { b0 } else { a0 })
        }
        1066 => {
            let f = fit(&pairs(rows, 1, 0))?;
            if f.n < 3.0 || f.sxx < f64::MIN_POSITIVE {
                return Err(err::DIV0);
            }
            Ok(((f.syy - f.sxy * f.sxy / f.sxx) / (f.n - 2.0)).sqrt())
        }
        // (x0, y, x OVER T)
        1067 => {
            let (a0, b0) = line(&pairs(rows, 1, 0), true)?;
            Ok(a0 + b0 * n(a, 0)?)
        }
        1068 | 1069 => {
            let (a0, b0) = exp_fit(&pairs(rows, 1, 0), flag(a, 0, false)?)?;
            Ok(if id == 1068 { a0.exp() } else { b0 })
        }
        _ => {
            let x = n(a, 0)?;
            let (a0, b0) = exp_fit(&pairs(rows, 1, 0), flag(a, 1, false)?)?;
            Ok((a0 + b0 * x).exp())
        }
    }
}

/// The χ² test's degrees of freedom: Π (kᵢ − 1) over the OVER categories
/// with more than one item among the rows (n − 1 for one category, (r − 1)(c − 1)
/// for a table).
fn chi_df(rows: &Rows) -> f64 {
    let dims = rows.pos.first().map_or(0, Vec::len);
    let mut df = 0.0;
    for d in 0..dims {
        let mut seen: Vec<u32> = rows.pos.iter().filter_map(|p| p.get(d).copied()).collect();
        seen.sort_unstable();
        seen.dedup();
        if seen.len() > 1 {
            let k = (seen.len() - 1) as f64;
            df = if df == 0.0 { k } else { df * k };
        }
    }
    df
}

/// CHITEST, FTEST, TTEST, ZTEST, AUTOCORREL.
fn test(id: u32, rows: &Rows, a: &[CellValue]) -> Num {
    let col = |i: usize| rows.col(i).filter_map(number).collect::<Vec<f64>>();
    match id {
        1080 => {
            let ps = pairs(rows, 0, 1);
            if ps.iter().any(|p| p.1 == 0.0) {
                return Err(err::DIV0);
            }
            let chi = sum(ps.iter().map(|&(o, e)| (o - e) * (o - e) / e));
            let df = chi_df(rows);
            if df < 1.0 {
                return Err(err::NA);
            }
            Ok(gamma_q(df / 2.0, chi / 2.0))
        }
        1081 => {
            let (x1, x2) = (col(0), col(1));
            let (s1, s2) = (var(&x1, true)?, var(&x2, true)?);
            if s1 == 0.0 || s2 == 0.0 {
                return Err(err::DIV0);
            }
            let (n1, n2) = (x1.len() as f64 - 1.0, x2.len() as f64 - 1.0);
            let p = if s1 > s2 {
                f_sf(s1 / s2, n1, n2)
            } else {
                f_sf(s2 / s1, n2, n1)
            };
            Ok(2.0 * p.min(1.0 - p))
        }
        1082 => {
            let tails = snap(n(a, 0)?).floor();
            let kind = snap(n(a, 1)?).floor();
            if tails != 1.0 && tails != 2.0 {
                return Err(err::NUM);
            }
            let (t, df) = if kind == 1.0 {
                let d: Vec<f64> = pairs(rows, 0, 1).iter().map(|p| p.0 - p.1).collect();
                let s = var(&d, true)?;
                if s == 0.0 {
                    return Err(err::DIV0);
                }
                let c = d.len() as f64;
                (mean(&d)?.abs() / (s / c).sqrt(), c - 1.0)
            } else if kind == 2.0 || kind == 3.0 {
                let (x1, x2) = (col(0), col(1));
                let (v1, v2) = (var(&x1, true)?, var(&x2, true)?);
                let (c1, c2) = (x1.len() as f64, x2.len() as f64);
                let dm = (mean(&x1)? - mean(&x2)?).abs();
                let (se, df) = if kind == 2.0 {
                    let pooled = ((c1 - 1.0) * v1 + (c2 - 1.0) * v2) / (c1 + c2 - 2.0);
                    ((pooled * (1.0 / c1 + 1.0 / c2)).sqrt(), c1 + c2 - 2.0)
                } else {
                    let (q1, q2) = (v1 / c1, v2 / c2);
                    let df = (q1 + q2) * (q1 + q2) / (q1 * q1 / (c1 - 1.0) + q2 * q2 / (c2 - 1.0));
                    ((q1 + q2).sqrt(), df)
                };
                if se == 0.0 {
                    return Err(err::DIV0);
                }
                (dm / se, df)
            } else {
                return Err(err::NUM);
            };
            Ok(tails * t_sf(t, df))
        }
        1083 => {
            let xs = col(0);
            if xs.len() < 2 {
                return Err(err::DIV0);
            }
            let mu = n(a, 0)?;
            let sigma = if a.len() > 1 {
                n(a, 1)?
            } else {
                var(&xs, true)?.sqrt()
            };
            if sigma <= 0.0 {
                return Err(err::NUM);
            }
            let z = (mean(&xs)? - mu) * (xs.len() as f64).sqrt() / sigma;
            Ok(norm_cdf(-z))
        }
        _ => {
            // GSL's gsl_stats_lag1_autocorrelation, running means and all.
            let xs = col(0);
            let m = mean(&xs)?;
            let mut q = 0.0;
            let mut v = (xs[0] - m) * (xs[0] - m);
            for i in 1..xs.len() {
                let (d0, d1) = (xs[i - 1] - m, xs[i] - m);
                q += (d0 * d1 - q) / (i + 1) as f64;
                v += (d1 * d1 - v) / (i + 1) as f64;
            }
            if v == 0.0 {
                return Err(err::DIV0);
            }
            Ok(q / v)
        }
    }
}

/// `PERCENTRANK` of `v` among sorted `xs`, inclusive or exclusive, before
/// truncation (#N/A outside the values).
fn percentrank(xs: &[f64], v: f64, inc: bool) -> Num {
    let c = xs.len();
    // NaN is outside, too (not between two values).
    if !xs
        .first()
        .zip(xs.last())
        .is_some_and(|(lo, hi)| (*lo..=*hi).contains(&v))
    {
        return Err(err::NA);
    }
    if c == 1 {
        return Ok(1.0);
    }
    let (smaller, cf) = (xs.partition_point(|x| *x < v), c as f64);
    let at = if xs[smaller] == v {
        smaller as f64
    } else {
        // Between xs[smaller − 1] and xs[smaller]: interpolate.
        let (lo, hi) = (xs[smaller - 1], xs[smaller]);
        smaller as f64 - 1.0 + (v - lo) / (hi - lo)
    };
    Ok(if inc {
        at / (cf - 1.0)
    } else {
        (at + 1.0) / (cf + 1.0)
    })
}

/// Each item's result; an item whose own value is not a number is #VALUE!
/// (Excel, LibreOffice and Gnumeric: `RANK("x", …)`).
fn per_item_ok(id: u32, rows: &Rows, a: &[CellValue]) -> Result<Vec<Num>, u8> {
    let own = |r: &Vec<CellValue>, i: usize| r.get(i).and_then(number).ok_or(err::VALUE);
    let xs = || sorted(rows.col(0).filter_map(number).collect());
    match id {
        1050 | 1051 => {
            let ascending = n_or(a, 0, 0.0)? != 0.0;
            let xs = xs();
            Ok(rows
                .vals
                .iter()
                .map(|r| {
                    let x = own(r, 0)?;
                    let less = xs.partition_point(|v| *v < x);
                    let ties = xs.partition_point(|v| *v <= x) - less;
                    let before = if ascending {
                        less
                    } else {
                        xs.len() - less - ties
                    } as f64;
                    Ok(if id == 1050 {
                        before + 1.0
                    } else {
                        before + (ties as f64 + 1.0) / 2.0
                    })
                })
                .collect())
        }
        1052 | 1053 => {
            let digits = snap(n_or(a, 1, 3.0)?).floor();
            if digits < 1.0 {
                return Err(err::NUM);
            }
            let scale = 10f64.powf(digits);
            let xs = xs();
            let rank =
                |v: f64| percentrank(&xs, v, id == 1052).map(|r| snap(r * scale).floor() / scale);
            if a.is_empty() {
                Ok(rows.vals.iter().map(|r| own(r, 0).and_then(rank)).collect())
            } else {
                Ok(vec![rank(n(a, 0)?); rows.len()])
            }
        }
        _ => {
            let konst = flag(a, 0, true)?;
            let ps = pairs(rows, 1, 0);
            let (a0, b0, grow) = if id == 1054 {
                let (a0, b0) = line(&ps, konst)?;
                (a0, b0, false)
            } else {
                let (a0, b0) = line(&ln_y(&ps)?, konst)?;
                (a0, b0, true)
            };
            Ok(rows
                .vals
                .iter()
                .map(|r| {
                    let fitted = a0 + b0 * own(r, 1)?;
                    Ok(if grow { fitted.exp() } else { fitted })
                })
                .collect())
        }
    }
}

/// How a criterion compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

impl Op {
    fn holds(self, o: Ordering) -> bool {
        match self {
            Op::Eq => o == Ordering::Equal,
            Op::Ne => o != Ordering::Equal,
            Op::Lt => o == Ordering::Less,
            Op::Le => o != Ordering::Greater,
            Op::Gt => o == Ordering::Greater,
            Op::Ge => o != Ordering::Less,
        }
    }
}

/// One element of a wildcard pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pat {
    Lit(char),
    One,
    Many,
}

#[derive(Debug, Clone, PartialEq)]
enum Rhs {
    Num(f64),
    Bool(bool),
    /// Case-folded text and its wildcard pattern.
    Text(Vec<char>, Vec<Pat>),
}

/// An Excel criterion, as `COUNTIF` and the other `…IF`/`…IFS` functions
/// take it: a number or boolean means equality; text may start with `=`,
/// `<>`, `<`, `<=`, `>` or `>=`, and the rest is a number, `TRUE`/`FALSE`,
/// or text compared case-insensitively, where `=`/`<>` take the wildcards
/// `*` (any run), `?` (any one character) and `~` (the next character
/// literally). `""` matches empty text, `"<>"` every non-empty cell. A
/// number criterion matches numbers (and dates, as serials) only; an error
/// cell matches nothing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Criterion {
    op: Op,
    rhs: Rhs,
}

impl Criterion {
    pub(crate) fn new(v: &CellValue) -> Self {
        let rhs = match v {
            CellValue::Bool(t) => Rhs::Bool(*t),
            CellValue::Text(s) => return Self::parse(s),
            other => Rhs::Num(number(other).unwrap_or(f64::NAN)),
        };
        Criterion { op: Op::Eq, rhs }
    }

    fn parse(s: &str) -> Self {
        let ops = [
            ("<=", Op::Le),
            (">=", Op::Ge),
            ("<>", Op::Ne),
            ("<", Op::Lt),
            (">", Op::Gt),
            ("=", Op::Eq),
        ];
        let (op, rest) = ops
            .iter()
            .find_map(|(p, op)| s.strip_prefix(p).map(|r| (*op, r)))
            .unwrap_or((Op::Eq, s));
        let rhs = match rest.trim().parse::<f64>() {
            Ok(x) if x.is_finite() => Rhs::Num(x),
            _ if rest.eq_ignore_ascii_case("TRUE") => Rhs::Bool(true),
            _ if rest.eq_ignore_ascii_case("FALSE") => Rhs::Bool(false),
            _ => {
                let folded: Vec<char> = rest.chars().map(fold_char).collect();
                let pat = pattern(&folded);
                Rhs::Text(folded, pat)
            }
        };
        Criterion { op, rhs }
    }

    pub(crate) fn matches(&self, v: &CellValue) -> bool {
        let ord = match (&self.rhs, v) {
            (_, CellValue::Err(_)) => return false,
            (Rhs::Num(c), _) => number(v).and_then(|x| x.partial_cmp(c)),
            (Rhs::Bool(c), CellValue::Bool(t)) => Some(t.cmp(c)),
            (Rhs::Text(raw, pat), CellValue::Text(t)) => {
                let t: Vec<char> = t.chars().map(fold_char).collect();
                if matches!(self.op, Op::Eq | Op::Ne) {
                    return glob(pat, &t) == (self.op == Op::Eq);
                }
                Some(t.cmp(raw))
            }
            _ => None,
        };
        ord.map_or(self.op == Op::Ne, |o| self.op.holds(o))
    }
}

/// A case-folded wildcard pattern: `*` any run, `?` any one character, `~`
/// the next character literally.
fn pattern(folded: &[char]) -> Vec<Pat> {
    let mut pat = Vec::new();
    let mut it = folded.iter();
    while let Some(&c) = it.next() {
        pat.push(match c {
            '~' => Pat::Lit(it.next().copied().unwrap_or('~')),
            '*' => Pat::Many,
            '?' => Pat::One,
            c => Pat::Lit(c),
        });
    }
    pat
}

/// Whether `text` matches the wildcard `pattern`, ignoring case (`MATCH`'s
/// and `XLOOKUP`'s wildcard mode).
pub(crate) fn like(pattern_text: &str, text: &str) -> bool {
    let p: Vec<char> = pattern_text.chars().map(fold_char).collect();
    let t: Vec<char> = text.chars().map(fold_char).collect();
    glob(&pattern(&p), &t)
}

/// Whether the whole of `t` matches `pat` (backtracking to the last `*`).
fn glob(pat: &[Pat], t: &[char]) -> bool {
    let (mut ti, mut pi) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        match pat.get(pi) {
            Some(Pat::Many) => {
                star = Some((pi, ti));
                pi += 1;
            }
            Some(Pat::One) => {
                ti += 1;
                pi += 1;
            }
            Some(Pat::Lit(c)) if *c == t[ti] => {
                ti += 1;
                pi += 1;
            }
            _ => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    pat[pi..].iter().all(|p| *p == Pat::Many)
}

/// COUNTIF, COUNTIFS, SUMIF, SUMIFS, AVERAGEIF, AVERAGEIFS, MAXIFS, MINIFS.
fn conditional(id: u32, rows: &Rows, a: &[CellValue]) -> Num {
    let width = rows.vals.first().map(Vec::len);
    let crits: Vec<Criterion> = a.iter().map(Criterion::new).collect();
    // (value column, criteria columns paired with `crits`)
    let (value, first_crit) = match id {
        1008 => (None, 0),
        1009 if width.is_some_and(|w| w != a.len()) => return Err(err::VALUE),
        1009 => (None, 0),
        // SUMIF / AVERAGEIF: x OVER T, criterion, [values]
        1002 | 1010 => (Some(usize::from(width == Some(2))), 0),
        _ if width.is_some_and(|w| w != a.len() + 1) => return Err(err::VALUE),
        _ => (Some(0), 1),
    };
    let mut count = 0usize;
    let mut picked = Vec::new();
    for r in &rows.vals {
        if !crits
            .iter()
            .enumerate()
            .all(|(j, k)| k.matches(&r[first_crit + j]))
        {
            continue;
        }
        count += 1;
        if let Some(v) = value {
            match &r[v] {
                CellValue::Err(c) => return Err(*c),
                cell => picked.extend(number(cell)),
            }
        }
    }
    match id {
        1008 | 1009 => Ok(count as f64),
        1010 | 1011 => Ok(sum(picked)),
        1002 | 1003 => mean(&picked),
        _ => Ok(extreme(&picked, id == 1013)),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::functions::{all, Kind};

    fn nv(x: f64) -> CellValue {
        CellValue::num(x)
    }
    fn tx(s: &str) -> CellValue {
        CellValue::Text(s.into())
    }

    /// Rows of one reduced measure per column, along one category.
    fn rows(cols: &[Vec<CellValue>]) -> Rows {
        let len = cols.first().map_or(0, Vec::len);
        Rows {
            pos: (0..len as u32).map(|i| vec![i]).collect(),
            vals: (0..len)
                .map(|r| cols.iter().map(|c| c[r].clone()).collect())
                .collect(),
            capacity: len + 2,
        }
    }
    fn col(xs: &[f64]) -> Vec<CellValue> {
        xs.iter().map(|x| nv(*x)).collect()
    }
    fn r1(xs: &[f64]) -> Rows {
        rows(&[col(xs)])
    }
    fn r2(xs: &[f64], ys: &[f64]) -> Rows {
        rows(&[col(xs), col(ys)])
    }
    fn go(id: u32, r: &Rows, a: &[CellValue]) -> CellValue {
        reduce(id, r, a)
    }
    fn e(c: u8) -> CellValue {
        CellValue::Err(c)
    }
    fn close(got: &CellValue, want: f64) {
        let g = got
            .as_num()
            .unwrap_or_else(|| panic!("{got:?}, want {want}"));
        assert!(
            (g - want).abs() <= 1e-12 * want.abs().max(1.0),
            "{g} vs {want}"
        );
    }

    /// Every registry entry in the range has an implementation.
    #[test]
    fn every_reduction_is_implemented() {
        let r = r2(&[1.0, 2.0, 4.0], &[2.0, 3.0, 7.0]);
        for d in all().filter(|d| (1000..=1149).contains(&d.id.0)) {
            let a: Vec<CellValue> = vec![nv(1.0); 3];
            let v = if d.kind == Kind::PerItem {
                per_item(d.id.0, &r, &a[..0])[0].clone()
            } else {
                go(d.id.0, &r, &a)
            };
            assert_ne!(v, e(err::NAME), "{} has no implementation", d.name);
        }
        assert_eq!(go(1149, &r, &[]), e(err::NAME));
    }

    #[test]
    fn criteria() {
        let m = |k: CellValue, v: CellValue| Criterion::new(&k).matches(&v);
        assert!(m(nv(3.0), nv(3.0)));
        assert!(!m(nv(3.0), tx("3")));
        assert!(!m(nv(3.0), CellValue::Bool(true)));
        assert!(m(tx(">5"), nv(6.0)));
        assert!(!m(tx(">5"), nv(5.0)));
        assert!(m(tx(">=5"), nv(5.0)));
        assert!(m(tx("<=5"), nv(5.0)));
        assert!(m(tx("<5"), nv(4.0)));
        assert!(!m(tx(">5"), tx("7")));
        assert!(m(tx("<>5"), tx("x")));
        assert!(m(tx("<>5"), nv(4.0)));
        assert!(!m(tx("<>5"), nv(5.0)));
        assert!(m(tx("=3"), nv(3.0)));
        assert!(m(tx(" 3 "), nv(3.0)));
        assert!(m(tx("1e1"), nv(10.0)));
        assert!(m(tx("inf"), tx("INF")));
        assert!(m(tx("a*"), tx("Apple")));
        assert!(!m(tx("a*"), tx("banana")));
        assert!(m(tx("?pple"), tx("APPLE")));
        assert!(!m(tx("?pple"), tx("pple")));
        assert!(m(tx("*an*"), tx("banana")));
        assert!(m(tx("b*a"), tx("banana")));
        assert!(!m(tx("b*n"), tx("banana")));
        assert!(m(tx("~*"), tx("*")));
        assert!(!m(tx("~*"), tx("x")));
        assert!(m(tx("a~?"), tx("a?")));
        assert!(m(tx("x~"), tx("x~")));
        assert!(m(tx("<>a*"), tx("banana")));
        assert!(!m(tx("<>a*"), tx("apple")));
        assert!(m(tx("<>a*"), nv(1.0)));
        assert!(!m(tx("a*"), nv(1.0)));
        assert!(m(tx("<b"), tx("Apple")));
        assert!(!m(tx("<b"), tx("cherry")));
        assert!(m(tx(">=B"), tx("b")));
        assert!(!m(tx("<b"), nv(1.0)));
        assert!(m(tx(""), tx("")));
        assert!(!m(tx(""), tx("a")));
        assert!(m(tx("="), tx("")));
        assert!(m(tx("<>"), tx("a")));
        assert!(m(tx("<>"), nv(0.0)));
        assert!(!m(tx("<>"), tx("")));
        assert!(m(CellValue::Bool(true), CellValue::Bool(true)));
        assert!(!m(CellValue::Bool(true), CellValue::Bool(false)));
        assert!(m(tx("false"), CellValue::Bool(false)));
        assert!(m(tx("<>TRUE"), CellValue::Bool(false)));
        assert!(m(tx("<>TRUE"), nv(1.0)));
        assert!(!m(tx("TRUE"), nv(1.0)));
        assert!(!m(tx("<>5"), e(err::NA)));
        assert!(!m(nv(1.0), e(err::NA)));
        // A date criterion and cell compare as serial dates.
        assert!(m(CellValue::Date(0), nv(25_569.0)));
        assert!(m(tx(">25000"), CellValue::Date(0)));
    }

    #[test]
    fn counts_and_errors() {
        let na = e(err::NA);
        let r = rows(&[vec![
            nv(1.0),
            tx(""),
            na.clone(),
            CellValue::Bool(true),
            tx("a"),
        ]]);
        assert_eq!(go(1005, &r, &[]), nv(1.0));
        assert_eq!(go(1006, &r, &[]), nv(5.0));
        // capacity 7, four non-blank cells.
        assert_eq!(go(1007, &r, &[]), nv(3.0));
        assert_eq!(go(1000, &r, &[]), na);
        let a = rows(&[vec![
            nv(2.0),
            tx("x"),
            CellValue::Bool(true),
            CellValue::Bool(false),
        ]]);
        assert_eq!(go(1000, &a, &[]), nv(3.0));
        assert_eq!(go(1001, &a, &[]), nv(0.75));
        assert_eq!(go(1012, &a, &[]), nv(2.0));
        assert_eq!(go(1014, &a, &[]), nv(0.0));
        close(&go(1028, &a, &[]), 0.916_666_666_666_666_6);
        close(&go(1029, &a, &[]), 0.6875);
        close(&go(1024, &a, &[]), 0.916_666_666_666_666_6_f64.sqrt());
        close(&go(1025, &a, &[]), 0.6875_f64.sqrt());
        let empty = r1(&[]);
        assert_eq!(go(1012, &empty, &[]), nv(0.0));
        assert_eq!(go(1001, &empty, &[]), e(err::DIV0));
        // Dates count as their serials.
        let d = rows(&[vec![CellValue::Date(0)]]);
        assert_eq!(go(1005, &d, &[]), nv(1.0));
        assert_eq!(go(1000, &d, &[]), nv(25_569.0));
    }

    #[test]
    fn conditional_reductions() {
        let x = col(&[1.0, 5.0, 7.0, 9.0]);
        let s = vec![tx("a"), tx("b"), tx("a"), e(err::NA)];
        let r = rows(&[x.clone(), s.clone()]);
        // COUNTIFS(x, ">2", s, "a")
        assert_eq!(go(1009, &r, &[tx(">2"), tx("a")]), nv(1.0));
        assert_eq!(go(1009, &r, &[tx(">2")]), e(err::VALUE));
        // SUMIFS(x, s, "a"), AVERAGEIFS, MAXIFS, MINIFS
        let sr = rows(&[x.clone(), s.clone()]);
        assert_eq!(go(1011, &sr, &[tx("a")]), nv(8.0));
        assert_eq!(go(1003, &sr, &[tx("a")]), nv(4.0));
        assert_eq!(go(1013, &sr, &[tx("a")]), nv(7.0));
        assert_eq!(go(1015, &sr, &[tx("a")]), nv(1.0));
        assert_eq!(go(1013, &sr, &[tx("zz")]), nv(0.0));
        assert_eq!(go(1003, &sr, &[tx("zz")]), e(err::DIV0));
        assert_eq!(go(1011, &sr, &[tx("a"), tx("b")]), e(err::VALUE));
        // SUMIF(s, "<>a", x): the #N/A criteria cell is skipped.
        let si = rows(&[s.clone(), x.clone()]);
        assert_eq!(go(1010, &si, &[tx("<>a")]), nv(5.0));
        // An error in a summed cell that matches is the result.
        let bad = rows(&[col(&[1.0, 2.0]), vec![nv(1.0), e(err::DIV0)]]);
        assert_eq!(go(1010, &bad, &[tx(">0")]), e(err::DIV0));
        // SUMIF / AVERAGEIF / COUNTIF over the criteria column itself.
        let one = rows(&[x]);
        assert_eq!(go(1010, &one, &[tx(">4")]), nv(21.0));
        assert_eq!(go(1002, &one, &[tx(">4")]), nv(7.0));
        assert_eq!(go(1008, &one, &[nv(5.0)]), nv(1.0));
        assert_eq!(go(1008, &rows(&[s]), &[tx("a")]), nv(2.0));
        assert_eq!(go(1009, &r1(&[]), &[tx("a")]), nv(0.0));
    }

    #[test]
    fn location_and_order() {
        let r = r1(&[3.0, 1.0, 4.0, 1.0, 5.0, 9.0, 2.0, 6.0]);
        assert_eq!(go(1016, &r, &[]), nv(3.5));
        assert_eq!(go(1016, &r1(&[2.0, 1.0, 3.0]), &[]), nv(2.0));
        assert_eq!(go(1017, &r, &[]), nv(1.0));
        assert_eq!(go(1017, &r1(&[2.0, 5.0, 5.0, 2.0]), &[]), nv(2.0));
        assert_eq!(go(1017, &r1(&[1.0, 2.0]), &[]), e(err::NA));
        assert_eq!(go(1034, &r, &[nv(2.0)]), nv(6.0));
        assert_eq!(go(1035, &r, &[nv(2.0)]), nv(1.0));
        assert_eq!(go(1035, &r, &[nv(2.5)]), nv(1.0));
        assert_eq!(go(1035, &r, &[nv(0.5)]), e(err::NUM));
        assert_eq!(go(1034, &r, &[nv(0.0)]), e(err::NUM));
        assert_eq!(go(1034, &r, &[nv(9.0)]), e(err::NUM));
        assert_eq!(go(1036, &r, &[nv(0.5)]), nv(3.5));
        assert_eq!(go(1036, &r, &[nv(1.5)]), e(err::NUM));
        assert_eq!(
            go(
                1036,
                &r1(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0]),
                &[nv(0.7)]
            ),
            nv(8.0)
        );
        assert_eq!(go(1037, &r, &[nv(0.5)]), nv(3.5));
        assert_eq!(go(1037, &r, &[nv(0.05)]), e(err::NUM));
        assert_eq!(go(1038, &r, &[nv(4.0)]), nv(9.0));
        assert_eq!(go(1038, &r, &[nv(5.0)]), e(err::NUM));
        assert_eq!(go(1039, &r, &[nv(2.0)]), nv(3.5));
        assert_eq!(go(1039, &r, &[nv(0.0)]), e(err::NUM));
        assert_eq!(go(1036, &r1(&[]), &[nv(0.5)]), e(err::NUM));
        assert_eq!(go(1016, &r1(&[]), &[]), e(err::NUM));
        // TRIMMEAN drops floor(0.25 · 8 / 2) = 1 from each end.
        assert_eq!(go(1020, &r, &[nv(0.25)]), nv(3.5));
        assert_eq!(go(1020, &r, &[nv(1.0)]), e(err::NUM));
        close(&go(1018, &r1(&[2.0, 8.0]), &[]), 4.0);
        assert_eq!(go(1018, &r1(&[2.0, 0.0]), &[]), e(err::NUM));
        close(&go(1019, &r1(&[1.0, 4.0, 4.0]), &[]), 2.0);
        assert_eq!(go(1021, &r1(&[1.0, 3.0]), &[]), nv(2.0));
        assert_eq!(go(1021, &r1(&[]), &[]), e(err::NUM));
        assert_eq!(go(1004, &r1(&[1.0, 3.0]), &[]), nv(1.0));
        assert_eq!(go(1004, &r1(&[]), &[]), e(err::NUM));
        assert_eq!(go(1022, &r1(&[1.0]), &[]), e(err::DIV0));
        assert_eq!(go(1027, &r1(&[1.0, 3.0]), &[]), nv(1.0));
        assert_eq!(go(1023, &r1(&[1.0, 3.0]), &[]), nv(1.0));
        assert_eq!(go(1026, &r1(&[1.0, 3.0]), &[]), nv(2.0));
        assert_eq!(go(1030, &r1(&[1.0, 2.0, 3.0]), &[]), e(err::DIV0));
        assert_eq!(go(1030, &r1(&[1.0, 1.0, 1.0, 1.0]), &[]), e(err::DIV0));
        assert_eq!(go(1032, &r1(&[1.0, 2.0]), &[]), e(err::DIV0));
        close(&go(1031, &r1(&[1.0, 2.0, 3.0, 4.0]), &[]), -1.36);
        assert_eq!(go(1033, &r1(&[1.0, 2.0, 3.0]), &[]), nv(0.0));
    }

    #[test]
    fn subtotal_and_aggregate() {
        let na = e(err::NA);
        let r = rows(&[vec![nv(1.0), nv(4.0), na.clone()]]);
        let ok = r1(&[1.0, 4.0, 2.0]);
        for (code, want) in [
            (1.0, 7.0 / 3.0),
            (2.0, 3.0),
            (3.0, 3.0),
            (4.0, 4.0),
            (5.0, 1.0),
            (6.0, 8.0),
            (9.0, 7.0),
            (109.0, 7.0),
        ] {
            close(&go(1041, &ok, &[nv(code)]), want);
        }
        assert_eq!(go(1041, &ok, &[nv(12.0)]), e(err::VALUE));
        assert_eq!(go(1041, &ok, &[nv(-1.0)]), e(err::VALUE));
        assert_eq!(go(1041, &r, &[nv(9.0)]), na);
        // AGGREGATE: options 6 skips the error, 4 does not.
        assert_eq!(go(1042, &r, &[nv(9.0), nv(6.0)]), nv(5.0));
        assert_eq!(go(1042, &r, &[nv(3.0), nv(6.0)]), nv(2.0));
        assert_eq!(go(1042, &r, &[nv(9.0), nv(4.0)]), na);
        assert_eq!(go(1042, &r, &[nv(14.0), nv(6.0), nv(1.0)]), nv(4.0));
        assert_eq!(go(1042, &r, &[nv(14.0), nv(6.0)]), e(err::VALUE));
        assert_eq!(go(1042, &r, &[nv(20.0), nv(6.0)]), e(err::VALUE));
        assert_eq!(go(1042, &r, &[nv(9.0), nv(8.0)]), e(err::VALUE));
        assert_eq!(go(1042, &r1(&[]), &[nv(6.0), nv(0.0)]), nv(0.0));
        assert_eq!(go(1042, &ok, &[nv(13.0), nv(0.0)]), e(err::NA));
        assert_eq!(go(1042, &ok, &[nv(19.0), nv(0.0), nv(2.0)]), nv(2.0));
        assert_eq!(go(1042, &ok, &[nv(19.0), nv(0.0), nv(4.0)]), e(err::NUM));
    }

    #[test]
    fn sums_and_series() {
        let r = r2(&[1.0, 2.0, 3.0], &[4.0, 5.0, 6.0]);
        assert_eq!(go(1043, &r, &[]), nv(32.0));
        assert_eq!(go(1043, &rows(&[vec![tx("a")]]), &[]), nv(0.0));
        assert_eq!(go(1044, &r, &[]), nv(14.0 - 77.0));
        assert_eq!(go(1045, &r, &[]), nv(91.0));
        assert_eq!(go(1046, &r, &[]), nv(27.0));
        assert_eq!(go(1049, &r, &[]), nv(6.0 / 15.0));
        assert_eq!(go(1049, &r2(&[1.0], &[0.0]), &[]), e(err::DIV0));
        assert_eq!(
            go(1040, &r2(&[1.0, 2.0], &[0.0, 1.0]), &[nv(1.0)]),
            e(err::NUM)
        );
        // SERIESSUM(2, 1, 2, {1, 2, 3}) = 2 + 2·8 + 3·32
        assert_eq!(
            go(1047, &r1(&[1.0, 2.0, 3.0]), &[nv(2.0), nv(1.0), nv(2.0)]),
            nv(114.0)
        );
        assert_eq!(
            go(1047, &r1(&[1.0]), &[nv(0.0), nv(-1.0), nv(1.0)]),
            e(err::DIV0)
        );
        assert_eq!(
            go(1047, &r1(&[1.0]), &[nv(-2.0), nv(0.5), nv(1.0)]),
            e(err::NUM)
        );
        // POLYNOMIAL(2, {1, 2, 3}) = 1 + 4 + 12
        assert_eq!(go(1048, &r1(&[1.0, 2.0, 3.0]), &[nv(2.0)]), nv(17.0));
        // A missing item keeps its power; along two categories, row order.
        let gap = Rows {
            pos: vec![vec![0], vec![2]],
            vals: vec![vec![nv(1.0)], vec![nv(1.0)]],
            capacity: 3,
        };
        assert_eq!(go(1048, &gap, &[nv(2.0)]), nv(5.0));
        let two = Rows {
            pos: vec![vec![0, 0], vec![1, 1]],
            ..gap
        };
        assert_eq!(go(1048, &two, &[nv(2.0)]), nv(3.0));
        // PROB
        let p = r2(&[1.0, 2.0, 3.0], &[0.2, 0.3, 0.5]);
        close(&go(1040, &p, &[nv(2.0)]), 0.3);
        close(&go(1040, &p, &[nv(3.0), nv(2.0)]), 0.8);
        assert_eq!(go(1040, &r2(&[1.0], &[0.5]), &[nv(1.0)]), e(err::NUM));
        assert_eq!(
            go(1040, &r2(&[1.0, 2.0], &[1.5, -0.5]), &[nv(1.0)]),
            e(err::NUM)
        );
    }

    #[test]
    fn regression_and_tests() {
        let r = r2(&[1.0, 2.0, 3.0, 4.0], &[2.0, 4.0, 5.0, 4.0]);
        // (x, y): CORREL, RSQ, COVAR, COVARIANCE.S
        close(&go(1060, &r, &[]), 3.5 / 23.75f64.sqrt());
        close(&go(1061, &r, &[]), 12.25 / 23.75);
        close(&go(1062, &r, &[]), 0.875);
        close(&go(1063, &r, &[]), 3.5 / 3.0);
        assert_eq!(go(1063, &r2(&[1.0], &[1.0]), &[]), e(err::DIV0));
        assert_eq!(go(1060, &r2(&[1.0, 1.0], &[1.0, 2.0]), &[]), e(err::DIV0));
        assert_eq!(go(1060, &r2(&[], &[]), &[]), e(err::DIV0));
        // (y, x): SLOPE of y = x is 1, INTERCEPT 0.
        let line = r2(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]);
        assert_eq!(go(1064, &line, &[]), nv(1.0));
        assert_eq!(go(1065, &line, &[]), nv(0.0));
        assert_eq!(go(1066, &line, &[]), nv(0.0));
        assert_eq!(go(1066, &r2(&[1.0, 2.0], &[1.0, 2.0]), &[]), e(err::DIV0));
        assert_eq!(go(1064, &r2(&[1.0, 2.0], &[1.0, 1.0]), &[]), e(err::DIV0));
        assert_eq!(go(1067, &line, &[nv(10.0)]), nv(10.0));
        // EXPA/EXPB of y = 2·e^x, simple and weighted.
        let xs = [0.0, 1.0, 2.0];
        let ys: Vec<f64> = xs.iter().map(|x: &f64| 2.0 * x.exp()).collect();
        let ex = r2(&ys, &xs);
        for simple in [true, false] {
            let s = [CellValue::Bool(simple)];
            close(&go(1068, &ex, &s), 2.0);
            close(&go(1069, &ex, &s), 1.0);
            close(
                &go(1070, &ex, &[nv(3.0), CellValue::Bool(simple)]),
                2.0 * 3f64.exp(),
            );
        }
        close(&go(1068, &ex, &[]), 2.0);
        assert_eq!(go(1068, &r2(&[0.0], &[1.0]), &[]), e(err::NUM));
        assert_eq!(
            go(
                1068,
                &r2(&[1.0, 2.0], &[1.0, 1.0]),
                &[CellValue::Bool(false)]
            ),
            e(err::DIV0)
        );
        // CHITEST
        let c = r2(&[10.0, 20.0, 30.0], &[20.0, 20.0, 20.0]);
        close(&go(1080, &c, &[]), (-5.0f64).exp());
        assert_eq!(go(1080, &r2(&[1.0], &[0.0]), &[]), e(err::DIV0));
        assert_eq!(go(1080, &r2(&[1.0], &[2.0]), &[]), e(err::NA));
        // A 2×2 table has one degree of freedom.
        let t = Rows {
            pos: vec![vec![0, 0], vec![0, 1], vec![1, 0], vec![1, 1]],
            vals: [(10.0, 15.0), (20.0, 15.0), (30.0, 25.0), (20.0, 25.0)]
                .iter()
                .map(|(o, e)| vec![nv(*o), nv(*e)])
                .collect(),
            capacity: 4,
        };
        assert_eq!(chi_df(&t), 1.0);
        // FTEST: equal variances give 1.
        close(&go(1081, &r2(&[1.0, 2.0, 3.0], &[2.0, 3.0, 4.0]), &[]), 1.0);
        assert_eq!(go(1081, &r2(&[1.0, 1.0], &[2.0, 3.0]), &[]), e(err::DIV0));
        let f = go(1081, &r2(&[1.0, 2.0, 9.0], &[2.0, 3.0, 4.0]), &[]);
        let g = go(1081, &r2(&[2.0, 3.0, 4.0], &[1.0, 2.0, 9.0]), &[]);
        assert_eq!(f, g);
        // TTEST
        let tt = r2(&[1.0, 2.0, 4.0], &[2.0, 4.0, 5.0]);
        for kind in [1.0, 2.0, 3.0] {
            let one = go(1082, &tt, &[nv(1.0), nv(kind)]).as_num().unwrap();
            let two = go(1082, &tt, &[nv(2.0), nv(kind)]).as_num().unwrap();
            assert!((two - 2.0 * one).abs() < 1e-15 && one > 0.0 && one < 0.5);
        }
        assert_eq!(go(1082, &tt, &[nv(3.0), nv(1.0)]), e(err::NUM));
        assert_eq!(go(1082, &tt, &[nv(1.0), nv(4.0)]), e(err::NUM));
        assert_eq!(
            go(1082, &r2(&[1.0, 2.0], &[2.0, 3.0]), &[nv(1.0), nv(1.0)]),
            e(err::DIV0)
        );
        assert_eq!(
            go(1082, &r2(&[1.0, 1.0], &[2.0, 2.0]), &[nv(1.0), nv(2.0)]),
            e(err::DIV0)
        );
        // ZTEST
        close(&go(1083, &r1(&[1.0, 3.0]), &[nv(2.0)]), 0.5);
        close(&go(1083, &r1(&[1.0, 3.0]), &[nv(2.0), nv(1.0)]), 0.5);
        assert_eq!(go(1083, &r1(&[1.0]), &[nv(2.0)]), e(err::DIV0));
        assert_eq!(go(1083, &r1(&[1.0, 3.0]), &[nv(2.0), nv(0.0)]), e(err::NUM));
        assert_eq!(
            go(1083, &r1(&[1.0, 3.0]), &[nv(2.0), nv(-1.0)]),
            e(err::NUM)
        );
        // AUTOCORREL of an alternating series is negative; a constant one #DIV/0!.
        close(&go(1084, &r1(&[1.0, -1.0, 1.0, -1.0]), &[]), -0.75);
        assert_eq!(go(1084, &r1(&[2.0, 2.0]), &[]), e(err::DIV0));
        // An error in the rows is the result.
        let bad = rows(&[vec![nv(1.0), e(err::REF)], col(&[1.0, 2.0])]);
        assert_eq!(go(1060, &bad, &[]), e(err::REF));
    }

    #[test]
    fn per_item_reductions() {
        let r = r1(&[3.0, 1.0, 4.0, 1.0, 5.0]);
        let nums = |v: Vec<CellValue>| v.iter().map(|c| c.as_num().unwrap()).collect::<Vec<_>>();
        assert_eq!(nums(per_item(1050, &r, &[])), [3.0, 4.0, 2.0, 4.0, 1.0]);
        assert_eq!(
            nums(per_item(1050, &r, &[nv(1.0)])),
            [3.0, 1.0, 4.0, 1.0, 5.0]
        );
        assert_eq!(nums(per_item(1051, &r, &[])), [3.0, 4.5, 2.0, 4.5, 1.0]);
        assert_eq!(
            nums(per_item(1051, &r, &[nv(1.0)])),
            [3.0, 1.5, 4.0, 1.5, 5.0]
        );
        // PERCENTRANK: the Excel examples' data.
        let x = r1(&[13.0, 12.0, 11.0, 8.0, 4.0, 3.0, 2.0, 1.0, 1.0, 1.0]);
        assert_eq!(per_item(1052, &x, &[nv(2.0)])[0], nv(0.333));
        assert_eq!(per_item(1052, &x, &[nv(4.0)])[0], nv(0.555));
        assert_eq!(per_item(1052, &x, &[nv(8.0)])[0], nv(0.666));
        assert_eq!(per_item(1052, &x, &[nv(5.0)])[0], nv(0.583));
        assert_eq!(per_item(1052, &x, &[nv(1.0)])[0], nv(0.0));
        assert_eq!(per_item(1052, &x, &[nv(14.0)])[0], e(err::NA));
        assert_eq!(per_item(1052, &x, &[nv(5.0), nv(0.0)])[0], e(err::NUM));
        let y = r1(&[1.0, 2.0, 3.0, 6.0, 6.0, 6.0, 7.0, 8.0, 9.0]);
        assert_eq!(per_item(1053, &y, &[nv(7.0)])[0], nv(0.7));
        assert_eq!(per_item(1053, &y, &[nv(5.43)])[0], nv(0.381));
        assert_eq!(per_item(1053, &y, &[nv(5.43), nv(1.0)])[0], nv(0.3));
        assert_eq!(nums(per_item(1052, &r1(&[2.0, 1.0]), &[])), [1.0, 0.0]);
        assert_eq!(per_item(1052, &r1(&[2.0]), &[]), vec![nv(1.0)]);
        // TREND/GROWTH at each item's own x.
        let t = r2(&[1.0, 3.0, 2.0], &[1.0, 2.0, 3.0]);
        let fitted = nums(per_item(1054, &t, &[]));
        for (got, want) in fitted.iter().zip([1.5, 2.0, 2.5]) {
            assert!((got - want).abs() < 1e-12);
        }
        let origin = nums(per_item(1054, &t, &[CellValue::Bool(false)]));
        assert!((origin[0] - 13.0 / 14.0).abs() < 1e-12);
        let g = r2(&[2.0, 4.0, 8.0], &[1.0, 2.0, 3.0]);
        for (got, want) in nums(per_item(1055, &g, &[])).iter().zip([2.0, 4.0, 8.0]) {
            assert!((got - want).abs() < 1e-12);
        }
        for (got, want) in nums(per_item(1055, &g, &[CellValue::Bool(false)]))
            .iter()
            .zip([2.0, 4.0, 8.0])
        {
            assert!((got - want).abs() < 1e-12, "{got} {want}");
        }
        assert_eq!(per_item(1055, &r2(&[0.0], &[1.0]), &[]), vec![e(err::NUM)]);
        assert_eq!(
            per_item(1054, &r2(&[1.0], &[0.0]), &[CellValue::Bool(false)]),
            vec![e(err::DIV0)]
        );
        let bad = rows(&[vec![nv(1.0), e(err::NA)]]);
        assert_eq!(per_item(1050, &bad, &[]), vec![e(err::NA); 2]);
    }

    /// An item whose own value is text: #VALUE! for that item alone, as
    /// Gnumeric 1.12 and LibreOffice (`RANK("x"; …)`, `PERCENTRANK(…;
    /// "x")`, TREND at a text x). PERCENTRANK used to panic here.
    #[test]
    fn per_item_text_items() {
        let t = rows(&[vec![nv(1.0), tx("x"), nv(3.0)]]);
        let v = e(err::VALUE);
        assert_eq!(per_item(1050, &t, &[]), [nv(2.0), v.clone(), nv(1.0)]);
        assert_eq!(per_item(1051, &t, &[]), [nv(2.0), v.clone(), nv(1.0)]);
        assert_eq!(per_item(1052, &t, &[]), [nv(0.0), v.clone(), nv(1.0)]);
        assert_eq!(per_item(1053, &t, &[]), [nv(0.333), v.clone(), nv(0.666)]);
        // TREND and GROWTH at each item's own x (column 1).
        let yx = rows(&[col(&[1.0, 2.0, 3.0]), vec![nv(1.0), tx("x"), nv(3.0)]]);
        assert_eq!(per_item(1054, &yx, &[]), [nv(1.0), v.clone(), nv(3.0)]);
        assert_eq!(per_item(1055, &yx, &[])[1], v);
        // PERCENTRANK of a value among no numbers, or of NaN: #N/A.
        let none = rows(&[vec![tx("a"), tx("b")]]);
        assert_eq!(per_item(1052, &none, &[nv(1.0)]), [e(err::NA), e(err::NA)]);
        assert_eq!(percentrank(&[1.0, 2.0], f64::NAN, true), Err(err::NA));
    }
}
