//! Series over one category in item order: the Hodrick–Prescott filter and
//! interpolation (Gnumeric's time-series plugin, ported line by line so
//! the results match it).

use crate::builtins::{err, n, n_or};
use crate::reduce::statistical::number;
use crate::reduce::Rows;
use crate::CellValue;

/// The numbers of measure `i`, in item order; anything else is #VALUE!.
pub(super) fn column(rows: &Rows, i: usize) -> Result<Vec<f64>, u8> {
    rows.vals
        .iter()
        .map(|r| r.get(i).and_then(number).ok_or(err::VALUE))
        .collect()
}

/// HPFILTER(y OVER T, [λ = 1600]): each item's trend under the
/// Hodrick–Prescott filter (Gnumeric's second column, the cycle, is y −
/// trend). Gnumeric's direct solve of the pentadiagonal system
/// (I + λ KᵀK) τ = y. Fewer than 6 values is #VALUE!; a zero pivot
/// #DIV/0!.
pub(super) fn hpfilter(rows: &Rows, a: &[CellValue]) -> Result<Vec<f64>, u8> {
    let mut data = column(rows, 0)?;
    let lambda = n_or(a, 0, 1600.0)?;
    let len = data.len();
    if len < 6 {
        return Err(err::VALUE);
    }
    let (l6, l4) = (6.0 * lambda + 1.0, -4.0 * lambda);
    let (mut av, mut bv, mut cv) = (vec![l6; len], vec![l4; len], vec![lambda; len]);
    av[0] = lambda + 1.0;
    bv[0] = -2.0 * lambda;
    av[1] = l6 - lambda;
    av[len - 2] = l6 - lambda;
    av[len - 1] = av[0];
    bv[len - 2] = bv[0];
    bv[len - 1] = 0.0;
    cv[len - 2] = 0.0;
    cv[len - 1] = 0.0;
    let (mut h, mut g) = ([0.0f64; 5], [0.0f64; 5]);
    for i in 0..len {
        let denom = av[i] - h[3] * h[0] - g[4] * g[1];
        if denom == 0.0 {
            return Err(err::DIV0);
        }
        let h_b = bv[i];
        g[0] = h[0];
        h[0] = (h_b - h[3] * h[1]) / denom;
        bv[i] = h[0];
        let h_c = cv[i];
        g[1] = h[1];
        h[1] = h_c / denom;
        cv[i] = h[1];
        av[i] = (data[i] - g[2] * g[4] - h[2] * h[3]) / denom;
        g[2] = h[2];
        h[2] = av[i];
        h[3] = h_b - h[4] * g[0];
        g[4] = h[4];
        h[4] = h_c;
    }
    let mut j = [av[len - 1], 0.0];
    data[len - 1] = j[0];
    for i in (1..len).rev() {
        data[i - 1] = av[i - 1] - bv[i - 1] * j[0] - cv[i - 1] * j[1];
        j[1] = j[0];
        j[0] = data[i - 1];
    }
    Ok(data)
}

/// Strictly increasing (Gnumeric's `go_range_increasing`, which needs at
/// least one value).
fn increasing(xs: &[f64]) -> bool {
    !xs.is_empty() && xs.windows(2).all(|w| w[0] < w[1])
}

/// A natural cubic spline (goffice's `go_cspline_init`): on [xᵢ, xᵢ₊₁],
/// y = yᵢ + dx·(cᵢ + dx·(bᵢ + dx·aᵢ)).
struct Spline<'a> {
    x: &'a [f64],
    y: &'a [f64],
    a: Vec<f64>,
    b: Vec<f64>,
    c: Vec<f64>,
}

impl<'a> Spline<'a> {
    fn new(x: &'a [f64], y: &'a [f64]) -> Option<Spline<'a>> {
        let n = x.len();
        if n < 3 || !increasing(x) {
            return None;
        }
        let (nm1, nm2) = (n - 1, n - 2);
        let (mut d1, mut d2, mut d3, mut d4) =
            (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]);
        let mut dx1 = x[1] - x[0];
        let mut dy1 = (y[1] - y[0]) / dx1 * 3.0;
        for i in 1..=nm2 {
            let dx2 = x[i + 1] - x[i];
            let dy2 = (y[i + 1] - y[i]) / dx2 * 3.0;
            d1[i] = dx1;
            d2[i] = 2.0 * (dx1 + dx2);
            d3[i] = dx2;
            d4[i] = dy2 - dy1;
            dx1 = dx2;
            dy1 = dy2;
        }
        // The tridiagonal system of rows 1..=n-2 (natural ends).
        for i in 2..=nm2 {
            d1[i] /= d2[i - 1];
            d2[i] -= d1[i] * d3[i - 1];
            d4[i] -= d1[i] * d4[i - 1];
        }
        d4[nm2] /= d2[nm2];
        for j in (1..nm2).rev() {
            d4[j] = (d4[j] - d3[j] * d4[j + 1]) / d2[j];
        }
        d4[0] = 0.0;
        d4[nm1] = 0.0;
        let mut s = Spline {
            x,
            y,
            a: Vec::with_capacity(nm1),
            b: Vec::with_capacity(nm1),
            c: Vec::with_capacity(nm1),
        };
        for i in 0..nm1 {
            let h = x[i + 1] - x[i];
            s.a.push((d4[i + 1] - d4[i]) / (3.0 * h));
            s.b.push(d4[i]);
            s.c.push((y[i + 1] - y[i]) / h - (2.0 * d4[i] + d4[i + 1]) * h / 3.0);
        }
        Some(s)
    }

    /// The segment index for `t` (`go_cspline_get_values`).
    fn seg(&self, t: f64) -> usize {
        let mut j = 1;
        while t > self.x[j] && j < self.x.len() - 1 {
            j += 1;
        }
        j - 1
    }

    fn value(&self, t: f64) -> f64 {
        let k = self.seg(t);
        let dx = t - self.x[k];
        self.y[k] + dx * (self.c[k] + dx * (self.b[k] + dx * self.a[k]))
    }

    /// ∫ from xₖ to xₖ + d of segment k.
    fn area(&self, k: usize, d: f64) -> f64 {
        d * (self.y[k] + d * (self.c[k] / 2.0 + d * (self.b[k] / 3.0 + d * self.a[k] / 4.0)))
    }

    /// ∫ over [t0, t1] (`go_cspline_get_integrals` for one interval).
    fn integral(&self, t0: f64, t1: f64) -> f64 {
        let jmax = self.x.len() - 1;
        let mut j = 1;
        // ponytail: goffice reads past the last knot when t0 ≥ it; bounded
        // here, so such a t0 extrapolates the last segment.
        while j < jmax && t0 >= self.x[j] {
            j += 1;
        }
        let mut k = j - 1;
        let mut sum = -self.area(k, t0 - self.x[k]);
        while j < jmax && t1 > self.x[j] {
            sum += self.area(k, self.x[j] - self.x[k]);
            j += 1;
            k = j - 1;
        }
        sum + self.area(k, t1 - self.x[k])
    }
}

/// Gnumeric's `linear_interpolation` at one target.
fn linear(x: &[f64], y: &[f64], t: f64) -> Option<f64> {
    if x.len() < 2 {
        return None;
    }
    let jmax = x.len() - 1;
    let mut j = 1;
    while j < jmax && t > x[j] {
        j += 1;
    }
    let k = j - 1;
    Some((t - x[k]) * (y[j] - y[k]) / (x[j] - x[k]) + y[k])
}

/// Gnumeric's `linear_averaging` over [t0, t1], line by line (its
/// extrapolation past the last knot included).
fn linear_avg(x: &[f64], y: &[f64], t0: f64, t1: f64) -> Option<f64> {
    if x.len() < 2 || t0 >= t1 {
        return None;
    }
    let jmax = x.len() - 1;
    let mut j = 1;
    while j < jmax && t0 > x[j] {
        j += 1;
    }
    let mut k = j - 1;
    let mut slope = (y[j] - y[k]) / (x[j] - x[k]) / 2.0;
    let part = |d: f64, slope: f64, k: usize| d * (slope * d + y[k]);
    if t1 < x[j] || j == jmax {
        let (x0, x1) = (t0 - x[k], t1 - x[k]);
        return Some((part(x1, slope, k) - part(x0, slope, k)) / (x1 - x0));
    }
    let mut res = part(x[j] - x[k], slope, k) - part(t0 - x[k], slope, k);
    while j < jmax {
        j += 1;
        if t1 <= x[j] {
            break;
        }
        k += 1;
        let d = x[j] - x[k];
        slope = (y[j] - y[k]) / d / 2.0;
        res += part(d, slope, k);
    }
    if j > k + 1 {
        k = j - 1;
        slope = (y[j] - y[k]) / (x[j] - x[k]) / 2.0;
    } else {
        k = j;
    }
    res += part(t1 - x[k], slope, k);
    Some(res / (t1 - t0))
}

/// Gnumeric's `staircase_interpolation` at one target: the y of the last
/// knot at or before t (the first y before the first knot).
fn staircase(x: &[f64], y: &[f64], t: f64) -> Option<f64> {
    if x.is_empty() {
        return None;
    }
    let j = 1 + x[1..].iter().take_while(|xj| t >= **xj).count();
    Some(y[j - 1])
}

/// Gnumeric's `staircase_averaging` over [t0, t1].
fn staircase_avg(x: &[f64], y: &[f64], t0: f64, t1: f64) -> Option<f64> {
    if x.is_empty() || t0 >= t1 {
        return None;
    }
    let jmax = x.len() - 1;
    let mut j = 1;
    while j <= jmax && t0 >= x[j] {
        j += 1;
    }
    if j > jmax || t1 < x[j] {
        return Some(y[j - 1]);
    }
    let mut res = (x[j] - t0) * y[j - 1];
    while j < jmax {
        j += 1;
        if t1 < x[j] {
            break;
        }
        res += (x[j] - x[j - 1]) * y[j - 1];
    }
    if t1 >= x[j] {
        j += 1;
    }
    res += (t1 - x[j - 1]) * y[j - 1];
    Some(res / (t1 - t0))
}

/// INTERPOLATION(xs, ys OVER T, target, [method = 0], [next]): ys
/// interpolated at target, Gnumeric's methods: 0 linear, 2 staircase, 4
/// natural cubic spline; the averaging methods 1, 3, 5 give the average
/// height over [target, next] (without next, #VALUE!, as Gnumeric's single
/// target). Pairs with a non-number are skipped; the knots are sorted by x
/// (two equal x are #VALUE!). Too few knots (2 for linear, 3 for the
/// spline, 1 for the staircase) or a method outside 0–5 is #VALUE!.
pub(super) fn interpolation(rows: &Rows, a: &[CellValue]) -> Result<f64, u8> {
    let t = n(a, 0)?;
    let method = n_or(a, 1, 0.0)?.floor();
    if !(0.0..=5.0).contains(&method) {
        return Err(err::VALUE);
    }
    let mut ps: Vec<(f64, f64)> = rows
        .vals
        .iter()
        .filter_map(|r| Some((r.first().and_then(number)?, r.get(1).and_then(number)?)))
        .collect();
    ps.sort_by(|p, q| p.0.total_cmp(&q.0));
    if ps.windows(2).any(|w| w[0].0 == w[1].0) {
        return Err(err::VALUE);
    }
    let (x, y): (Vec<f64>, Vec<f64>) = ps.into_iter().unzip();
    let next = || {
        if a.len() > 2 {
            n(a, 2)
        } else {
            Err(err::VALUE)
        }
    };
    let r = match method as u8 {
        0 => linear(&x, &y, t),
        1 => linear_avg(&x, &y, t, next()?),
        2 => staircase(&x, &y, t),
        3 => staircase_avg(&x, &y, t, next()?),
        4 => Spline::new(&x, &y).map(|s| s.value(t)),
        _ => {
            let t1 = next()?;
            Spline::new(&x, &y)
                .filter(|_| t < t1)
                .map(|s| s.integral(t, t1) / (t1 - t))
        }
    };
    r.ok_or(err::VALUE)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn rows(xs: &[f64], ys: &[f64]) -> Rows {
        Rows {
            pos: (0..xs.len() as u32).map(|i| vec![i]).collect(),
            vals: xs
                .iter()
                .zip(ys)
                .map(|(x, y)| vec![CellValue::num(*x), CellValue::num(*y)])
                .collect(),
            capacity: xs.len(),
        }
    }

    fn at(r: &Rows, args: &[f64]) -> Result<f64, u8> {
        let a: Vec<CellValue> = args.iter().map(|x| CellValue::num(*x)).collect();
        interpolation(r, &a)
    }

    #[test]
    fn interpolation_edges() {
        let r = rows(&[1.0, 2.0, 4.0], &[10.0, 20.0, 0.0]);
        assert_eq!(at(&r, &[3.0, 6.0]), Err(err::VALUE));
        assert_eq!(at(&r, &[3.0, -1.0]), Err(err::VALUE));
        // Averaging without next, or with next ≤ target.
        assert_eq!(at(&r, &[3.0, 1.0]), Err(err::VALUE));
        assert_eq!(at(&r, &[3.0, 3.0, 3.0]), Err(err::VALUE));
        assert_eq!(at(&r, &[3.0, 5.0, 2.0]), Err(err::VALUE));
        // Too few knots.
        let one = rows(&[1.0], &[5.0]);
        assert_eq!(at(&one, &[3.0, 0.0]), Err(err::VALUE));
        assert_eq!(at(&one, &[3.0, 2.0]), Ok(5.0));
        assert_eq!(at(&rows(&[], &[]), &[3.0, 2.0]), Err(err::VALUE));
        assert_eq!(at(&rows(&[], &[]), &[3.0, 3.0, 4.0]), Err(err::VALUE));
        assert_eq!(
            at(&rows(&[1.0, 2.0], &[1.0, 2.0]), &[1.5, 4.0]),
            Err(err::VALUE)
        );
        // A non-number pair is skipped.
        let mut r = rows(&[1.0, 2.0, 4.0], &[10.0, 20.0, 0.0]);
        r.vals[1][1] = CellValue::Text("x".into());
        assert_eq!(at(&r, &[2.0, 0.0]), Ok(10.0 - 10.0 / 3.0));
        // Staircase averaging inside one step, and past the last knot.
        let r = rows(&[1.0, 2.0, 4.0], &[10.0, 20.0, 0.0]);
        assert_eq!(at(&r, &[2.5, 3.0, 3.0]), Ok(20.0));
        assert_eq!(at(&r, &[5.0, 3.0, 6.0]), Ok(0.0));
        // Linear averaging that stays left of the second knot.
        assert_eq!(at(&r, &[0.0, 1.0, 1.0]), Ok(5.0));
    }

    #[test]
    fn hpfilter_edges() {
        let r = rows(&[1.0; 5], &[0.0; 5]);
        assert_eq!(hpfilter(&r, &[]), Err(err::VALUE));
        // λ = 0 keeps the data.
        let xs = [3.0, 1.0, 4.0, 1.0, 5.0, 9.0];
        let r = rows(&xs, &xs);
        assert_eq!(hpfilter(&r, &[CellValue::num(0.0)]), Ok(xs.to_vec()));
        // A λ that makes the first pivot zero.
        assert_eq!(hpfilter(&r, &[CellValue::num(-1.0)]), Err(err::DIV0));
        let mut t = rows(&xs, &xs);
        t.vals[2][0] = CellValue::Bool(true);
        assert_eq!(hpfilter(&t, &[]), Err(err::VALUE));
    }
}
