//! Logarithmic fits (Gnumeric): LOGREG, the least-squares line of y on
//! ln x (LINEST's machinery on ln x), and LOGFIT, the trial-and-error fit
//! of y = a + b·ln(sign·(x − c)) (goffice's `go_logarithmic_fit`, ported
//! step by step so the search lands where Gnumeric's does).

use super::List;
use crate::builtins::err;
use crate::reduce::statistical::number;
use crate::reduce::Rows;
use crate::CellValue;
use improv_core_model::FuncId;

/// LOGREG(y, x OVER T, [affine = TRUE], [stats = FALSE]): LINEST(y, ln x):
/// m, b, and with stats the rest of LINEST's 5×2 block. An x ≤ 0 is #NUM!.
pub(super) fn logreg(rows: &Rows, a: &[CellValue]) -> List {
    let mut logged = rows.clone();
    for r in &mut logged.vals {
        let x = r.get(1).and_then(number).ok_or(err::VALUE)?;
        if x <= 0.0 {
            return Err(err::NUM);
        }
        r[1] = CellValue::num(x.ln());
    }
    // LINEST (id 1257), whose block LOGREG shares.
    let out = crate::reduce::array(FuncId(1257), &logged, a);
    match out.as_slice() {
        [CellValue::Err(c)] => Err(*c),
        _ => Ok(out),
    }
}

/// The fit for one sign and c: [sign, a, b, c, ss_resid].
fn fit_at(xs: &[f64], ys: &[f64], mean_y: f64, sign: f64, c: f64) -> [f64; 5] {
    let t: Vec<f64> = xs.iter().map(|x| (sign * (x - c)).ln()).collect();
    let mt = average(&t);
    let (mut s1, mut s2) = (0.0, 0.0);
    for (ti, yi) in t.iter().zip(ys) {
        let d = ti - mt;
        s1 += d * (yi - mean_y);
        s2 += d * d;
    }
    let b = s1 / s2;
    let a = mean_y - b * mt;
    let ss = t
        .iter()
        .zip(ys)
        .map(|(ti, yi)| (a + b * ti - yi).powi(2))
        .sum();
    [sign, a, b, c, ss]
}

/// goffice's `go_range_average`: exact for constant data.
fn average(xs: &[f64]) -> f64 {
    if xs.iter().all(|x| *x == xs[0]) {
        return xs[0];
    }
    xs.iter().sum::<f64>() / xs.len() as f64
}

/// goffice's `go_fake_round`: round half away from zero, with a 1 ulp
/// grace for values a hair below the half.
fn fake_round(x: f64) -> f64 {
    if x == x.floor() {
        return x;
    }
    let v = x.abs() + 0.5;
    let y = if v == v.floor() {
        v
    } else {
        v.next_up().floor()
    };
    y.copysign(x)
}

/// LOGFIT(y, x OVER T INTO K): Gnumeric's sign, a, b, c and the sum of
/// squared residuals of y = a + b·ln(sign·(x − c)). Fewer than 3 points,
/// fewer than 3 distinct x or y, or a cloud too far from logarithmic
/// (no sign bends it, or no local minimum in c) is #NUM! (#VALUE! for too
/// few points, as Gnumeric).
pub(super) fn logfit(rows: &Rows) -> List {
    let mut xs = Vec::new();
    let mut ys = Vec::new();
    for r in &rows.vals {
        if let (Some(y), Some(x)) = (r.first().and_then(number), r.get(1).and_then(number)) {
            ys.push(y);
            xs.push(x);
        }
    }
    if xs.len() < 3 {
        return Err(err::VALUE);
    }
    let range = |v: &[f64]| {
        let lo = v.iter().copied().fold(f64::INFINITY, f64::min);
        let hi = v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        (lo, hi)
    };
    let ((min_x, max_x), (min_y, max_y)) = (range(&xs), range(&ys));
    let three = |v: &[f64], lo: f64, hi: f64| v.iter().any(|x| *x != lo && *x != hi);
    if min_x == max_x || min_y == max_y || !three(&ys, min_y, max_y) || !three(&xs, min_x, max_x) {
        return Err(err::NUM);
    }
    let mean_y = average(&ys);
    let fit = |sign: f64, c: f64| fit_at(&xs, &ys, mean_y, sign, c);
    let x_range = max_x - min_x;
    // c's final accuracy: a power of ten below the x range, times 1e-6.
    let mut acc = x_range.log10();
    if acc < 0.0 && acc.fract() != 0.0 {
        acc -= 1.0;
    }
    let acc = 10f64.powf(acc.trunc()) * 0.000_001;
    let (step, c_range) = (x_range * 0.05, x_range * 100.0);
    // The sign: with c far below (above) the data, does moving c further
    // away make the fit worse (the curve must bend that way)?
    let plus = fit(1.0, min_x - c_range - step)[4] > fit(1.0, min_x - c_range)[4];
    let minus = fit(-1.0, max_x + c_range + step)[4] > fit(-1.0, max_x + c_range)[4];
    let sign = match (plus, minus) {
        (true, false) => 1.0,
        (false, true) => -1.0,
        _ => return Err(err::NUM),
    };
    let edge = if sign == 1.0 { min_x } else { max_x };
    let offset = acc
        * if sign == 1.0 {
            (edge / acc).floor()
        } else {
            (edge / acc).ceil()
        };
    // The fit must improve moving c away from the data's edge.
    let mut res = fit(sign, offset - sign * acc);
    if fit(sign, offset - sign * 2.0 * acc)[4] >= res[4] {
        return Err(err::NUM);
    }
    let end = offset - sign * c_range;
    let mut dist = sign * (offset - end) / 2.0;
    res[3] = end + sign * dist;
    loop {
        dist /= 2.0;
        res = fit(sign, res[3]);
        let up = fit(sign, res[3] + sign * dist);
        if up[4] <= res[4] {
            res = up;
        } else {
            let down = fit(sign, res[3] - sign * dist);
            if down[4] <= res[4] {
                res = down;
            }
        }
        if dist <= acc {
            break;
        }
    }
    let res = fit(sign, acc * fake_round(res[3] / acc));
    // At the far end of the range: no local minimum.
    if sign * (res[3] - end) < 1.1 * acc {
        return Err(err::NUM);
    }
    Ok(res.iter().map(|x| CellValue::num(*x)).collect())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn yx(ys: &[f64], xs: &[f64]) -> Rows {
        Rows {
            pos: (0..ys.len() as u32).map(|i| vec![i]).collect(),
            vals: ys
                .iter()
                .zip(xs)
                .map(|(y, x)| vec![CellValue::num(*y), CellValue::num(*x)])
                .collect(),
            capacity: ys.len(),
        }
    }

    #[test]
    fn logfit_rejects() {
        assert_eq!(logfit(&yx(&[1.0, 2.0], &[1.0, 2.0])), Err(err::VALUE));
        // Two distinct x (or y) values.
        assert_eq!(
            logfit(&yx(&[1.0, 2.0, 3.0], &[1.0, 2.0, 2.0])),
            Err(err::NUM)
        );
        assert_eq!(
            logfit(&yx(&[1.0, 2.0, 2.0], &[1.0, 2.0, 3.0])),
            Err(err::NUM)
        );
        // A straight line bends neither way.
        let line = yx(&[1.0, 2.0, 3.0, 4.0], &[1.0, 2.0, 3.0, 4.0]);
        assert_eq!(logfit(&line), Err(err::NUM));
        // A point that is not a number is skipped; three left.
        let mut t = yx(&[1.0, 2.0, 2.5, 2.8], &[1.0, 2.0, 3.0, 4.0]);
        t.vals[0][0] = CellValue::Text("x".into());
        assert!(logfit(&t).is_ok());
        // Constant y is never logarithmic.
        assert_eq!(logfit(&yx(&[5.0; 4], &[1.0, 2.0, 3.0, 4.0])), Err(err::NUM));
        assert_eq!(average(&[2.0, 2.0]), 2.0);
    }

    #[test]
    fn logreg_rejects_and_rounding() {
        assert_eq!(logreg(&yx(&[1.0, 2.0], &[1.0, 0.0]), &[]), Err(err::NUM));
        let mut t = yx(&[1.0, 2.0], &[1.0, 2.0]);
        t.vals[1][1] = CellValue::Text("x".into());
        assert_eq!(logreg(&t, &[]), Err(err::VALUE));
        // LINEST's own error (a bad const flag).
        let ok = yx(&[1.0, 2.0, 3.0], &[1.0, 2.0, 3.0]);
        assert_eq!(logreg(&ok, &[CellValue::Text("x".into())]), Err(err::VALUE));
        assert_eq!(fake_round(2.5), 3.0);
        assert_eq!(fake_round(-2.5), -3.0);
        assert_eq!(fake_round(2.4), 2.0);
        assert_eq!(fake_round(0.5f64.next_down()), 1.0);
    }
}
