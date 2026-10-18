//! Matrices: a measure over two categories read as a matrix,
//! `MDETERM(A OVER R, C)`: rows along R, columns along C, both in item
//! order (a measure over one category is a column). Every cell must hold a
//! number: a missing cell, text or a boolean is #VALUE!, as Excel's empty
//! and text cells are. List results are flattened row by row (`INTO R2,
//! C2` lays them out as the block).

use super::List;
use crate::builtins::{err, n};
use crate::reduce::statistical::number;
use crate::reduce::Rows;
use crate::CellValue;

/// A dense row-major matrix.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Mat {
    pub r: usize,
    pub c: usize,
    pub a: Vec<f64>,
}

impl Mat {
    fn zero(r: usize, c: usize) -> Mat {
        Mat {
            r,
            c,
            a: vec![0.0; r * c],
        }
    }

    fn identity(n: usize) -> Mat {
        let mut m = Mat::zero(n, n);
        for i in 0..n {
            m.a[i * n + i] = 1.0;
        }
        m
    }

    fn at(&self, i: usize, j: usize) -> f64 {
        self.a[i * self.c + j]
    }

    fn set(&mut self, i: usize, j: usize, x: f64) {
        self.a[i * self.c + j] = x;
    }

    fn transpose(&self) -> Mat {
        let mut t = Mat::zero(self.c, self.r);
        for i in 0..self.r {
            for j in 0..self.c {
                t.set(j, i, self.at(i, j));
            }
        }
        t
    }

    /// The square matrix, or #VALUE! (Excel: unequal rows and columns).
    fn square(self) -> Result<Mat, u8> {
        if self.r == self.c {
            Ok(self)
        } else {
            Err(err::VALUE)
        }
    }

    /// Off-diagonal pairs averaged, as Gnumeric's CHOLESKY and EIGEN do
    /// (the asymmetry is taken for rounding noise).
    fn symmetric(mut self) -> Mat {
        for i in 0..self.r {
            for j in i + 1..self.c {
                let x = (self.at(i, j) + self.at(j, i)) / 2.0;
                self.set(i, j, x);
                self.set(j, i, x);
            }
        }
        self
    }

    fn list(&self) -> Vec<CellValue> {
        self.a.iter().map(|x| crate::builtins::num(*x)).collect()
    }
}

/// The group's rows as a matrix (the first reduced measure).
pub(super) fn read(rows: &Rows) -> Result<Mat, u8> {
    let dim = |k: usize| {
        rows.pos
            .iter()
            .map(|p| p.get(k).map_or(1, |x| *x as usize + 1))
            .max()
            .unwrap_or(0)
    };
    let (r, c) = (dim(0), dim(1));
    // Every cell present: the last row and column hold one, and the count
    // fills the categories.
    if r * c != rows.capacity || rows.len() != r * c {
        return Err(err::VALUE);
    }
    let mut m = Mat::zero(r, c);
    for (p, v) in rows.pos.iter().zip(&rows.vals) {
        let x = v.first().and_then(number).ok_or(err::VALUE)?;
        m.set(p[0] as usize, p.get(1).map_or(0, |j| *j as usize), x);
    }
    Ok(m)
}

/// LibreOffice's LU decomposition with scaled partial pivoting
/// (`lcl_LUP_decompose`, its pivot scale included): the combined L\U,
/// the row permutation, and the determinant's sign (0: singular).
fn lup(mut a: Mat) -> (Mat, Vec<usize>, f64) {
    let n = a.r;
    let mut scale = Vec::with_capacity(n);
    for i in 0..n {
        let max = (0..n).map(|j| a.at(i, j).abs()).fold(0.0, f64::max);
        if max == 0.0 {
            return (a, Vec::new(), 0.0);
        }
        scale.push(1.0 / max);
    }
    let mut p: Vec<usize> = (0..n).collect();
    let mut sign = 1.0;
    for k in 0..n.saturating_sub(1) {
        let (mut max, mut kp) = (0.0, k);
        for i in k..n {
            let t = scale[k] * a.at(i, k).abs();
            if max < t {
                (max, kp) = (t, i);
            }
        }
        if max == 0.0 {
            return (a, p, 0.0);
        }
        if k != kp {
            p.swap(k, kp);
            sign = -sign;
            scale.swap(k, kp);
            for j in 0..n {
                a.a.swap(k * n + j, kp * n + j);
            }
        }
        for i in k + 1..n {
            let (num, den) = (a.at(i, k), a.at(k, k));
            a.set(i, k, num / den);
            for j in k + 1..n {
                let x = (a.at(i, j) * den - num * a.at(k, j)) / den;
                a.set(i, j, x);
            }
        }
    }
    if (0..n).any(|i| a.at(i, i) == 0.0) {
        sign = 0.0;
    }
    (a, p, sign)
}

/// MDETERM(a OVER R, C): the determinant (0 for a singular matrix, as
/// LibreOffice); LOGMDETERM (`log`): ln |det|, from the LU diagonal so it
/// neither overflows nor underflows (#NUM! when singular).
pub(super) fn det(rows: &Rows, log: bool) -> Result<f64, u8> {
    let (lu, _, sign) = lup(read(rows)?.square()?);
    let diag = (0..lu.r).map(|i| lu.at(i, i));
    match (sign == 0.0, log) {
        (true, true) => Err(err::NUM),
        (true, false) => Ok(0.0),
        (false, true) => Ok(diag.map(|d| d.abs().ln()).sum()),
        (false, false) => Ok(diag.fold(sign, |acc, d| acc * d)),
    }
}

/// LibreOffice's `KahanSum`: Neumaier's sum with the latest term held
/// back, so a total that cancels to within 2⁻⁴⁸ is exactly 0.
struct Kahan {
    sum: f64,
    err: f64,
    mem: f64,
}

impl Kahan {
    fn new(x: f64) -> Kahan {
        Kahan {
            sum: x,
            err: 0.0,
            mem: 0.0,
        }
    }

    fn neumaier(&mut self, x: f64) {
        let t = self.sum + x;
        self.err += if self.sum.abs() >= x.abs() {
            (self.sum - t) + x
        } else {
            (x - t) + self.sum
        };
        self.sum = t;
    }

    fn add(&mut self, x: f64) {
        if x == 0.0 {
            return;
        }
        if self.mem != 0.0 {
            self.neumaier(self.mem);
        }
        self.mem = x;
    }

    fn get(&mut self) -> f64 {
        let total = self.sum + self.err;
        let m = self.mem;
        if m == 0.0 {
            return total;
        }
        // rtl::math::approxEqual(m, -total).
        if (m < 0.0) != (total < 0.0)
            && total != 0.0
            && (m + total).abs() < m.abs() * 2f64.powi(-48)
        {
            return 0.0;
        }
        self.neumaier(m);
        self.mem = 0.0;
        self.sum + self.err
    }
}

/// MINVERSE(a OVER R, C): the inverse, row by row, by LibreOffice's LU
/// solve; #NUM! when singular (Excel; LibreOffice: Err:502).
pub(super) fn inverse(rows: &Rows) -> List {
    let (lu, p, sign) = lup(read(rows)?.square()?);
    if sign == 0.0 {
        return Err(err::NUM);
    }
    let n = lu.r;
    let mut inv = Mat::zero(n, n);
    let mut x = vec![0.0; n];
    for col in 0..n {
        // Forward: L y = P e_col (skipping the leading zeros).
        let mut first = None;
        for i in 0..n {
            let mut s = Kahan::new(f64::from(u8::from(p[i] == col)));
            if let Some(f) = first {
                for (j, xj) in x.iter().enumerate().take(i).skip(f) {
                    s.add(-lu.at(i, j) * xj);
                }
            } else if s.sum != 0.0 {
                first = Some(i);
            }
            x[i] = s.get();
        }
        // Back: U x = y.
        for i in (0..n).rev() {
            let mut s = Kahan::new(x[i]);
            for (j, xj) in x.iter().enumerate().skip(i + 1) {
                s.add(-lu.at(i, j) * xj);
            }
            x[i] = s.get() / lu.at(i, i);
        }
        for (i, xi) in x.iter().enumerate() {
            inv.set(i, col, *xi);
        }
    }
    Ok(inv.list())
}

/// A = U Σ Vᵀ (A m×n, m ≥ n) by one-sided Jacobi (Hestenes) rotations:
/// U's columns are A's rotated columns made orthogonal, σ their norms.
fn svd(a: &Mat) -> (Mat, Vec<f64>, Mat) {
    let (m, n) = (a.r, a.c);
    let mut u = a.clone();
    let mut v = Mat::identity(n);
    let rotate = |x: &mut Mat, p: usize, q: usize, c: f64, s: f64| {
        for i in 0..x.r {
            let (xp, xq) = (x.at(i, p), x.at(i, q));
            x.set(i, p, c * xp - s * xq);
            x.set(i, q, s * xp + c * xq);
        }
    };
    for _ in 0..64 {
        let mut done = true;
        for p in 0..n {
            for q in p + 1..n {
                let (mut al, mut be, mut ga) = (0.0, 0.0, 0.0);
                for i in 0..m {
                    let (x, y) = (u.at(i, p), u.at(i, q));
                    al += x * x;
                    be += y * y;
                    ga += x * y;
                }
                if ga == 0.0 || ga.abs() <= f64::EPSILON * (al * be).sqrt() {
                    continue;
                }
                done = false;
                let zeta = (be - al) / (2.0 * ga);
                let t = zeta.signum() / (zeta.abs() + zeta.hypot(1.0));
                let c = 1.0 / t.hypot(1.0);
                rotate(&mut u, p, q, c, c * t);
                rotate(&mut v, p, q, c, c * t);
            }
        }
        if done {
            break;
        }
    }
    let mut sigma = Vec::with_capacity(n);
    for j in 0..n {
        let s = (0..m).map(|i| u.at(i, j).powi(2)).sum::<f64>().sqrt();
        if s > 0.0 {
            for i in 0..m {
                u.set(i, j, u.at(i, j) / s);
            }
        }
        sigma.push(s);
    }
    (u, sigma, v)
}

/// Gnumeric's default threshold: singular values at most 256 ε times the
/// largest count as zero.
const THRESHOLD: f64 = 256.0 * f64::EPSILON;

/// MPSEUDOINVERSE(a OVER R, C, [threshold]): the Moore–Penrose
/// pseudo-inverse (C × R, row by row): V Σ⁺ Uᵀ, singular values at most
/// threshold × the largest taken as zero (Gnumeric's rule).
pub(super) fn pseudo_inverse(rows: &Rows, a: &[CellValue]) -> List {
    let m = read(rows)?;
    let limit = if a.is_empty() { THRESHOLD } else { n(a, 0)? };
    let wide = m.r < m.c;
    let m = if wide { m.transpose() } else { m };
    let (u, s, v) = svd(&m);
    let cut = s.iter().copied().fold(0.0, f64::max) * limit;
    let mut b = Mat::zero(m.c, m.r);
    for i in 0..m.c {
        for j in 0..m.r {
            let x: f64 = (0..m.c)
                .filter(|k| s[*k] > cut)
                .map(|k| v.at(i, k) * u.at(j, k) / s[k])
                .sum();
            b.set(i, j, x);
        }
    }
    Ok(if wide { b.transpose() } else { b }.list())
}

/// CHOLESKY(a OVER R, C): the lower-triangular L with L Lᵀ = A (A made
/// symmetric first), row by row, by Gnumeric's Cholesky–Banachiewicz
/// loop. A matrix that is not positive definite has #NUM! where a square
/// root of a negative number (and everything computed from it) falls.
pub(super) fn cholesky(rows: &Rows) -> List {
    let a = read(rows)?.square()?.symmetric();
    let n = a.r;
    let mut l = Mat::zero(n, n);
    for r in 0..n {
        for c in 0..r {
            let s: f64 = (0..c).map(|k| l.at(r, k) * l.at(c, k)).sum();
            l.set(r, c, (a.at(r, c) - s) / l.at(c, c));
        }
        let s: f64 = (0..r).map(|k| l.at(r, k) * l.at(r, k)).sum();
        l.set(r, r, (a.at(r, r) - s).sqrt());
    }
    Ok(l.list())
}

/// EIGEN(a OVER R, C INTO R2, C2): the eigenvalues of A (made symmetric),
/// largest magnitude first (ties: the larger value), then the matrix whose
/// columns are their unit eigenvectors: (n + 1) × n values, row by row.
/// Gnumeric's cyclic Jacobi rotations, so the vectors' signs match it.
pub(super) fn eigen(rows: &Rows) -> List {
    let mut a = read(rows)?.square()?.symmetric();
    let n = a.r;
    let mut v = Mat::identity(n);
    let mut e: Vec<f64> = (0..n).map(|i| a.at(i, i)).collect();
    // The column of row i's largest off-diagonal entry right of the diagonal.
    let max_index = |a: &Mat, i: usize| {
        let mut best = i + 1;
        if best >= n {
            return n - 1;
        }
        for j in best + 1..n {
            if a.at(i, j).abs() > a.at(i, best).abs() {
                best = j;
            }
        }
        best
    };
    let mut ind: Vec<usize> = (0..n).map(|i| max_index(&a, i)).collect();
    // How many eigenvalues still move; done when none does.
    let mut changed = vec![true; n];
    let mut moving = n;
    let mut update = |e: &mut [f64], k: usize, t: f64| {
        let y = e[k];
        e[k] += t;
        let still = y == e[k];
        if changed[k] && still {
            changed[k] = false;
            moving -= 1;
        } else if !changed[k] && !still {
            changed[k] = true;
            moving += 1;
        }
        moving
    };
    let mut left = n;
    let rotate = |a: &mut Mat, (k, l): (usize, usize), (i, j): (usize, usize), c: f64, s: f64| {
        let (x, y) = (a.at(k, l), a.at(i, j));
        a.set(k, l, c * x - s * y);
        a.set(i, j, s * x + c * y);
    };
    let mut steps = 0;
    while n > 1 && left != 0 {
        steps += 1;
        if steps > 400_000 {
            return Err(err::NUM);
        }
        let mut m = 0;
        for k in 1..n - 1 {
            if a.at(k, ind[k]).abs() > a.at(m, ind[m]).abs() {
                m = k;
            }
        }
        let l = ind[m];
        let pivot = a.at(m, l);
        if pivot == 0.0 {
            break;
        }
        let y = (e[l] - e[m]) / 2.0;
        let mut t = y.abs() + pivot.hypot(y);
        let mut s = pivot.hypot(t);
        let c = t / s;
        s = pivot / s;
        t = pivot * pivot / t;
        if y < 0.0 {
            s = -s;
            t = -t;
        }
        a.set(m, l, 0.0);
        update(&mut e, m, -t);
        left = update(&mut e, l, t);
        for i in 0..m {
            rotate(&mut a, (i, m), (i, l), c, s);
        }
        for i in m + 1..l {
            rotate(&mut a, (m, i), (i, l), c, s);
        }
        for i in l + 1..n {
            rotate(&mut a, (m, i), (l, i), c, s);
        }
        for i in 0..n {
            rotate(&mut v, (i, m), (i, l), c, s);
        }
        ind[m] = max_index(&a, m);
        ind[l] = max_index(&a, l);
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|i, j| {
        let (x, y) = (e[*i], e[*j]);
        y.abs().total_cmp(&x.abs()).then(y.total_cmp(&x))
    });
    let mut out = Mat::zero(n + 1, n);
    for (c, k) in order.iter().enumerate() {
        out.set(0, c, e[*k]);
        for r in 0..n {
            out.set(r + 1, c, v.at(r, *k));
        }
    }
    Ok(out.list())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// A matrix measure's rows, row by row.
    pub(crate) fn mat(r: usize, c: usize, xs: &[f64]) -> Rows {
        Rows {
            pos: (0..r * c)
                .map(|k| vec![(k / c) as u32, (k % c) as u32])
                .collect(),
            vals: xs.iter().map(|x| vec![CellValue::num(*x)]).collect(),
            capacity: r * c,
        }
    }

    fn close(got: List, want: &[f64]) {
        let got = got.unwrap();
        assert_eq!(got.len(), want.len());
        for (g, w) in got.iter().zip(want) {
            assert!(
                (g.as_num().unwrap() - w).abs() < 1e-12,
                "{got:?} vs {want:?}"
            );
        }
    }

    #[test]
    fn shapes_and_cells() {
        // A missing cell, a text cell, a non-square matrix: #VALUE!.
        let mut r = mat(2, 2, &[1.0, 2.0, 3.0, 4.0]);
        r.capacity = 6;
        assert_eq!(read(&r), Err(err::VALUE));
        let mut r = mat(2, 2, &[1.0, 2.0, 3.0, 4.0]);
        r.vals[1][0] = CellValue::Text("x".into());
        assert_eq!(read(&r), Err(err::VALUE));
        assert_eq!(det(&mat(2, 3, &[1.0; 6]), false), Err(err::VALUE));
        // One category: a column.
        let col = Rows {
            pos: vec![vec![0], vec![1]],
            vals: vec![vec![CellValue::num(1.0)], vec![CellValue::num(2.0)]],
            capacity: 2,
        };
        assert_eq!(read(&col).map(|m| (m.r, m.c)), Ok((2, 1)));
    }

    #[test]
    fn eigen_and_cholesky_edges() {
        // Already diagonal: no rotation; sorted by magnitude, then value.
        close(
            eigen(&mat(3, 3, &[-2.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 1.0])),
            &[2.0, -2.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0],
        );
        // Not positive definite: the root of a negative is #NUM!.
        let r = cholesky(&mat(2, 2, &[1.0, 2.0, 2.0, 1.0])).unwrap();
        assert_eq!(r[3], CellValue::Err(err::NUM));
        assert_eq!(cholesky(&mat(1, 2, &[1.0, 2.0])), Err(err::VALUE));
    }

    #[test]
    fn singular_and_zero() {
        // A zero row, and a zero pivot column, are singular.
        assert_eq!(det(&mat(2, 2, &[0.0, 0.0, 1.0, 2.0]), false), Ok(0.0));
        assert_eq!(det(&mat(2, 2, &[0.0, 1.0, 0.0, 2.0]), false), Ok(0.0));
        assert_eq!(det(&mat(2, 2, &[1.0, 2.0, 2.0, 4.0]), true), Err(err::NUM));
        assert_eq!(inverse(&mat(2, 2, &[1.0, 2.0, 2.0, 4.0])), Err(err::NUM));
        assert_eq!(det(&mat(1, 1, &[-3.0]), true), Ok(3f64.ln()));
        // The pseudo-inverse of zero is zero; of a rank-1 matrix A, Aᵀ/‖A‖².
        close(pseudo_inverse(&mat(1, 2, &[0.0, 0.0]), &[]), &[0.0, 0.0]);
        let r1 = [1.0, 2.0, 2.0, 4.0, 3.0, 6.0];
        let want: Vec<f64> = [1.0, 2.0, 3.0, 2.0, 4.0, 6.0]
            .iter()
            .map(|x| x / 70.0)
            .collect();
        close(pseudo_inverse(&mat(3, 2, &r1), &[]), &want);
        // A large threshold drops every singular value.
        close(
            pseudo_inverse(&mat(1, 1, &[2.0]), &[CellValue::num(2.0)]),
            &[0.0],
        );
        assert_eq!(
            pseudo_inverse(&mat(1, 1, &[2.0]), &[CellValue::Text("x".into())]),
            Err(err::VALUE)
        );
    }
}
