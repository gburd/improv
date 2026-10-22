//! LINEST and LOGEST: the least-squares fit, and with `stats` Excel's block
//! of regression statistics flattened row by row (5×2 for one x; 5×(k+1)
//! for k x's, see [`multi`]).

use super::List;
use crate::builtins::{b, err, serial};
use crate::reduce::Rows;
use crate::CellValue;

/// A cell as a number: numbers, and dates as serials (text and booleans
/// are skipped).
pub(super) fn number(v: &CellValue) -> Option<f64> {
    match v {
        CellValue::Date(ms) => Some(serial(*ms)),
        _ => v.as_num(),
    }
}

/// How many values LINEST/LOGEST of `k` x's return: 5 × (k + 1) with
/// stats, else k + 1.
pub(super) fn len(a: &[CellValue], k: usize) -> usize {
    if a.len() > 1 && b(a, 1) == Ok(true) {
        5 * (k + 1)
    } else {
        k + 1
    }
}

/// LINEST(y, x OVER T, [const = TRUE], [stats = FALSE]), or LOGEST (`log`):
/// the same fit of ln y, reporting m = e^slope and b = e^intercept. A
/// value that is not a number is #VALUE!; LOGEST's y must be positive.
///
/// `[m, b]`, or with stats `[m, b, se_m, se_b, r², se_y, F, df, ss_reg,
/// ss_resid]`. Without const, b is 0 (LOGEST: 1) and se_b is #N/A. An exact
/// fit (no residual, or no degree of freedom) has se_m, se_b and se_y 0, r²
/// 1 and F #N/A, as in LibreOffice. A constant x is collinear with the
/// constant term and is dropped, as Excel documents: m = 0 (LOGEST: 1) and
/// se_m = 0, b fits alone, df gains one, and F is #N/A.
pub(super) fn linest(rows: &Rows, a: &[CellValue], log: bool, k: usize) -> List {
    if k > 1 {
        return multi(rows, a, log, k);
    }
    let konst = a.is_empty() || b(a, 0)?;
    let stats = a.len() > 1 && b(a, 1)?;
    let mut ps: Vec<(f64, f64)> = Vec::new();
    for r in &rows.vals {
        let y = r.first().and_then(number).ok_or(err::VALUE)?;
        let x = r.get(1).and_then(number).ok_or(err::VALUE)?;
        if log && y <= 0.0 {
            return Err(err::NUM);
        }
        ps.push((x, if log { y.ln() } else { y }));
    }
    let n = ps.len() as f64;
    let (mx, my) = if konst {
        (
            ps.iter().map(|p| p.0).sum::<f64>() / n,
            ps.iter().map(|p| p.1).sum::<f64>() / n,
        )
    } else {
        (0.0, 0.0)
    };
    // Centred (about the means) with const, raw without, as LibreOffice.
    let c: Vec<(f64, f64)> = ps.iter().map(|p| (p.0 - mx, p.1 - my)).collect();
    let sxx: f64 = c.iter().map(|p| p.0 * p.0).sum();
    let sxy: f64 = c.iter().map(|p| p.0 * p.1).sum();
    // x is collinear with the constant term (every x equal; without const,
    // every x zero).
    let collinear = ps.iter().all(|p| p.0 == if konst { ps[0].0 } else { 0.0 });
    let slope = if collinear { 0.0 } else { sxy / sxx };
    let intercept = my - slope * mx;
    let out = |x: f64| CellValue::num(if log { x.exp() } else { x });
    let mut res = vec![out(slope), out(intercept)];
    if !stats {
        return Ok(res);
    }
    let used = f64::from(u8::from(!collinear));
    let df = n - used - f64::from(u8::from(konst));
    let ss_reg = slope * slope * sxx;
    let ss_resid: f64 = c.iter().map(|p| (p.1 - slope * p.0).powi(2)).sum();
    let na = CellValue::Err(err::NA);
    let (se_m, se_b, r2, se_y, f, ss_resid) = if df <= 0.0 || ss_resid == 0.0 {
        let se_b = if konst {
            CellValue::num(0.0)
        } else {
            na.clone()
        };
        (0.0, se_b, 1.0, 0.0, na, 0.0)
    } else {
        let se_y = (ss_resid / df).sqrt();
        let se_m = if collinear { 0.0 } else { se_y / sxx.sqrt() };
        let se_b = if !konst {
            na.clone()
        } else if collinear {
            CellValue::num(se_y / n.sqrt())
        } else {
            CellValue::num(se_y * (mx * mx / sxx + 1.0 / n).sqrt())
        };
        let f = if collinear {
            na
        } else {
            CellValue::num(ss_reg / (ss_resid / df))
        };
        (se_m, se_b, ss_reg / (ss_reg + ss_resid), se_y, f, ss_resid)
    };
    res.extend([
        CellValue::num(se_m),
        se_b,
        CellValue::num(r2),
        CellValue::num(se_y),
        f,
        CellValue::num(df.max(0.0)),
        CellValue::num(ss_reg),
        CellValue::num(ss_resid),
    ]);
    Ok(res)
}

/// LINEST/LOGEST of `k` ≥ 2 x's (rows hold y, x1, …, xk): ordinary least
/// squares by the normal equations, Gaussian elimination with partial
/// pivoting. Excel's order: `[mk, …, m1, b]`, and with stats a 5 × (k + 1)
/// block row by row — `se` of those; `r², se_y` then #N/A; `F, df` then
/// #N/A; `ss_reg, ss_resid` then #N/A. An x that is a linear combination
/// of the ones before it (or, with const, of the constant) is dropped, as
/// Excel documents and Gnumeric does: its m and se are 0, the others fit
/// without it, and df gains one per dropped x.
// ponytail: normal equations square the condition number; switch to QR if
// ill-conditioned fits ever matter.
fn multi(rows: &Rows, a: &[CellValue], log: bool, k: usize) -> List {
    let konst = a.is_empty() || b(a, 0)?;
    let stats = a.len() > 1 && b(a, 1)?;
    let p = k + usize::from(konst);
    let mut xs: Vec<Vec<f64>> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    for r in &rows.vals {
        let mut y = r.first().and_then(number).ok_or(err::VALUE)?;
        if log {
            if y <= 0.0 {
                return Err(err::NUM);
            }
            y = y.ln();
        }
        let mut row: Vec<f64> = (1..=k)
            .map(|j| r.get(j).and_then(number).ok_or(err::VALUE))
            .collect::<Result<_, _>>()?;
        if konst {
            row.push(1.0);
        }
        xs.push(row);
        ys.push(y);
    }
    let n = ys.len();
    // The columns kept: the constant first, then each x not a combination
    // of those kept before it. Indices into a row (x1…xk, then the 1).
    let kept = independent(&xs, k, konst);
    let q = kept.len();
    if n < q {
        return Err(err::NUM);
    }
    // XᵀX | Xᵀy over the kept columns, then XᵀX⁻¹ (for the standard
    // errors) by Gauss–Jordan.
    let mut m = vec![vec![0.0; 2 * q + 1]; q];
    for (row, y) in xs.iter().zip(&ys) {
        for (i, &ci) in kept.iter().enumerate() {
            for (j, &cj) in kept.iter().enumerate() {
                m[i][j] += row[ci] * row[cj];
            }
            m[i][2 * q] += row[ci] * y;
        }
    }
    for (i, r) in m.iter_mut().enumerate() {
        r[q + i] = 1.0;
    }
    for c in 0..q {
        let piv = (c..q)
            .max_by(|x, y| m[*x][c].abs().total_cmp(&m[*y][c].abs()))
            .unwrap_or(c);
        if m[piv][c].abs() < 1e-12 {
            return Err(err::NUM);
        }
        m.swap(c, piv);
        let d = m[c][c];
        m[c].iter_mut().for_each(|v| *v /= d);
        for r in 0..q {
            if r != c {
                let f = m[r][c];
                let pivot = m[c].clone();
                m[r].iter_mut().zip(pivot).for_each(|(v, q)| *v -= f * q);
            }
        }
    }
    // Back in row order (x1…xk, the 1); a dropped column's coefficient
    // and variance are 0.
    let mut coef = vec![0.0; p];
    let mut var = vec![0.0; p];
    for (i, &c) in kept.iter().enumerate() {
        coef[c] = m[i][2 * q];
        var[c] = m[i][q + i];
    }
    let fitted: Vec<f64> = xs
        .iter()
        .map(|r| r.iter().zip(&coef).map(|(x, c)| x * c).sum())
        .collect();
    let out = |x: f64| CellValue::num(if log { x.exp() } else { x });
    // Excel lists the slopes last x first, then b.
    let mut res: Vec<CellValue> = (0..k).rev().map(|j| out(coef[j])).collect();
    res.push(if konst { out(coef[k]) } else { out(0.0) });
    if !stats {
        return Ok(res);
    }
    let na = || CellValue::Err(err::NA);
    let mean = if konst {
        ys.iter().sum::<f64>() / n as f64
    } else {
        0.0
    };
    let ss_resid: f64 = ys.iter().zip(&fitted).map(|(y, f)| (y - f).powi(2)).sum();
    let ss_tot: f64 = ys.iter().map(|y| (y - mean).powi(2)).sum();
    let ss_reg = ss_tot - ss_resid;
    let df = n - q;
    let se_y = if df > 0 {
        (ss_resid / df as f64).sqrt()
    } else {
        0.0
    };
    let se = |j: usize| CellValue::num(se_y * var[j].max(0.0).sqrt());
    res.extend((0..k).rev().map(se));
    res.push(if konst { se(k) } else { na() });
    let pad = |v: &mut Vec<CellValue>| v.extend((0..k - 1).map(|_| na()));
    res.push(CellValue::num(if ss_tot == 0.0 {
        1.0
    } else {
        ss_reg / ss_tot
    }));
    res.push(CellValue::num(se_y));
    pad(&mut res);
    // F's numerator has one degree of freedom per x kept.
    let used = q - usize::from(konst);
    res.push(if df > 0 && ss_resid > 0.0 && used > 0 {
        CellValue::num((ss_reg / used as f64) / (ss_resid / df as f64))
    } else {
        na()
    });
    res.push(CellValue::num(df as f64));
    pad(&mut res);
    res.push(CellValue::num(ss_reg));
    res.push(CellValue::num(ss_resid));
    pad(&mut res);
    Ok(res)
}

/// The row columns `multi` fits: with `konst` the constant (column `k`)
/// first, then x1…xk in order, each kept when it is not (to a relative
/// 1e-9) a linear combination of those kept so far, by Gram–Schmidt on
/// the columns.
fn independent(xs: &[Vec<f64>], k: usize, konst: bool) -> Vec<usize> {
    let col = |c: usize| xs.iter().map(|r| r[c]).collect::<Vec<f64>>();
    let dot = |a: &[f64], b: &[f64]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f64>();
    let mut basis: Vec<Vec<f64>> = Vec::new();
    let mut kept = Vec::new();
    let order = konst.then_some(k).into_iter().chain(0..k);
    for c in order {
        let mut v = col(c);
        let norm = dot(&v, &v).sqrt();
        for b in &basis {
            let f = dot(&v, b);
            v.iter_mut().zip(b).for_each(|(x, y)| *x -= f * y);
        }
        let rest = dot(&v, &v).sqrt();
        if norm > 0.0 && rest > 1e-9 * norm {
            v.iter_mut().for_each(|x| *x /= rest);
            basis.push(v);
            kept.push(c);
        }
    }
    kept.sort_unstable();
    kept
}

#[cfg(test)]
mod tests {
    use super::super::tests::rows;
    use super::*;

    fn n(x: f64) -> CellValue {
        CellValue::num(x)
    }

    fn yx(ys: &[f64], xs: &[f64]) -> Rows {
        rows(ys.iter().zip(xs).map(|(y, x)| vec![n(*y), n(*x)]).collect())
    }

    const NA: CellValue = CellValue::Err(err::NA);
    const T: CellValue = CellValue::Bool(true);
    const F: CellValue = CellValue::Bool(false);

    #[test]
    fn exact_and_collinear_fits() {
        // Two points: no degree of freedom left.
        let two = yx(&[1.0, 3.0], &[1.0, 2.0]);
        assert_eq!(
            linest(&two, &[T, T], false, 1).unwrap(),
            [
                n(2.0),
                n(-1.0),
                n(0.0),
                n(0.0),
                n(1.0),
                n(0.0),
                NA,
                n(0.0),
                n(2.0),
                n(0.0)
            ]
        );
        // Without const the exact fit's se_b is #N/A.
        let line = yx(&[2.0, 4.0], &[1.0, 2.0]);
        let r = linest(&line, &[F, T], false, 1).unwrap();
        assert_eq!(r[..4], [n(2.0), n(0.0), n(0.0), NA]);
        // A constant x: b is the mean (Excel: LINEST({0,0}, {1,1}) is 0).
        let flat = yx(&[1.0, 3.0, 5.0], &[2.0, 2.0, 2.0]);
        assert_eq!(linest(&flat, &[], false, 1).unwrap(), [n(0.0), n(3.0)]);
        let r = linest(&flat, &[T, T], false, 1).unwrap();
        let se_y = 2.0; // sqrt(8 / 2)
        assert_eq!(
            r,
            [
                n(0.0),
                n(3.0),
                n(0.0),
                n(se_y / 3f64.sqrt()),
                n(0.0),
                n(se_y),
                NA,
                n(2.0),
                n(0.0),
                n(8.0)
            ]
        );
        // ...and without const, x = 0 drops out entirely: b = 0.
        let zero = yx(&[1.0, 3.0], &[0.0, 0.0]);
        let r = linest(&zero, &[F, T], false, 1).unwrap();
        assert_eq!(r[..4], [n(0.0), n(0.0), n(0.0), NA]);
        assert_eq!(r[7], n(2.0));
    }

    /// Two x's: y = 1 + 2·x1 + 3·x2 exactly, and a noisy fit checked
    /// against LibreOffice's LINEST({..}, {..;..}, TRUE, TRUE).
    #[test]
    fn several_xs() {
        let pts = [(1.0, 0.0), (2.0, 1.0), (3.0, 1.0), (4.0, 3.0), (5.0, 2.0)];
        let r = |noise: &[f64]| {
            rows(
                pts.iter()
                    .zip(noise)
                    .map(|((a, b), e)| vec![n(1.0 + 2.0 * a + 3.0 * b + e), n(*a), n(*b)])
                    .collect(),
            )
        };
        let exact = linest(&r(&[0.0; 5]), &[], false, 2).unwrap();
        let close = |v: &CellValue, x: f64| (v.as_num().unwrap() - x).abs() < 1e-9;
        for (v, x) in exact.iter().zip([3.0, 2.0, 1.0]) {
            assert!(close(v, x), "{exact:?}");
        }
        let s = linest(&r(&[0.1, -0.2, 0.05, 0.0, 0.1]), &[T, T], false, 2).unwrap();
        assert_eq!(s.len(), 15);
        // LibreOffice 25.8 LINEST(y; x1:x2; 1; 1), row by row.
        let want = [
            2.912_499_999_999_999,
            2.072_500_000_000_000_2,
            0.915_000_000_000_000_9,
            0.119_569_540_435_681_59,
            0.086_222_821_804_902_97,
            0.165_680_415_257_809_4,
            0.999_713_241_654_235_3,
            0.151_244_834_622_542_2,
            f64::NAN,
            3_486.256_830_601_072_6,
            2.0,
            f64::NAN,
            159.496_250_000_000_03,
            0.045_750_000_000_000_28,
        ];
        for (i, x) in want.iter().enumerate() {
            if x.is_nan() {
                assert_eq!(s[i], NA, "{i}");
            } else {
                let got = s[i].as_num().unwrap();
                assert!(
                    (got - x).abs() <= 1e-9 * x.abs().max(1.0),
                    "{i}: {got} vs {x}"
                );
            }
        }
        assert_eq!(s[14], NA);
        // Without const (LibreOffice: 2.721875, 2.415625, 0; se 0.3768,
        // 0.1968, #N/A).
        let nc = linest(&r(&[0.1, -0.2, 0.05, 0.0, 0.1]), &[F, T], false, 2).unwrap();
        for (i, x) in [
            (0, 2.721_874_999_999_997_6),
            (1, 2.415_625_000_000_001),
            (3, 0.376_796_910_096_584_34),
            (4, 0.196_775_768_642_889_65),
        ] {
            let got = nc[i].as_num().unwrap();
            assert!(
                (got - x).abs() <= 1e-9 * x.abs().max(1.0),
                "nc {i}: {got} vs {x}"
            );
        }
        assert_eq!(nc[2], n(0.0));
        assert_eq!(nc[5], NA);
        let lg = rows(vec![
            vec![n(2.0), n(0.0), n(1.0)],
            vec![n(4.0), n(1.0), n(1.0)],
            vec![n(16.0), n(2.0), n(2.0)],
        ]);
        let l = linest(&lg, &[], true, 2).unwrap();
        assert!(l.iter().all(|v| v.as_num().is_some_and(|x| x > 0.0)));
        // Collinear x's are dropped (m 0); too few rows, a bad value.
        let col = rows(vec![
            vec![n(1.0), n(1.0), n(2.0)],
            vec![n(2.0), n(2.0), n(4.0)],
            vec![n(3.0), n(3.0), n(6.0)],
        ]);
        assert_eq!(
            linest(&col, &[], false, 2),
            Ok(vec![n(0.0), n(1.0), n(0.0)])
        );
        // One row: both x's are collinear with b, which fits alone (the
        // one-x rule; LibreOffice and Gnumeric give an error).
        assert_eq!(
            linest(&rows(vec![vec![n(1.0), n(1.0), n(2.0)]]), &[], false, 2),
            Ok(vec![n(0.0), n(0.0), n(1.0)])
        );
        // Without const, x2 = 2·x1 is dropped: y = 1·x1.
        assert_eq!(
            linest(&rows(vec![vec![n(1.0), n(1.0), n(2.0)]]), &[F], false, 2),
            Ok(vec![n(0.0), n(1.0), n(0.0)])
        );
        assert_eq!(
            linest(&rows(vec![vec![n(1.0), n(1.0)]]), &[], false, 2),
            Err(err::VALUE)
        );
        assert_eq!(
            linest(&rows(vec![vec![n(-1.0), n(1.0), n(1.0)]]), &[], true, 2),
            Err(err::NUM)
        );
        assert_eq!(len(&[T, T], 3), 20);
        assert_eq!(len(&[], 3), 4);
        assert_eq!(len(&[T, F], 3), 4);
    }

    /// Collinear x's are dropped, the later one, as Excel documents
    /// ("removes any redundant X columns") and Gnumeric 1.12.57 does:
    /// LINEST(y, xs, const, TRUE) blocks from Gnumeric (LibreOffice 25.2
    /// gives an ill-conditioned fit or #VALUE! for these), `None` for #N/A.
    #[test]
    fn collinear_xs_are_dropped() {
        let y = [1.0, 3.0, 2.0, 5.0, 7.5, 6.0];
        let x1 = [1.0, 2.0, 3.0, 4.0, 6.0, 7.0];
        let x3 = [0.5, 1.0, 4.0, 2.0, 3.0, 1.0];
        let fit = |xs: &[Vec<f64>], konst: bool| {
            let r = rows(
                (0..6)
                    .map(|i| {
                        let mut row = vec![n(y[i])];
                        row.extend(xs.iter().map(|x| n(x[i])));
                        row
                    })
                    .collect(),
            );
            linest(&r, &[CellValue::Bool(konst), T], false, xs.len()).unwrap()
        };
        let check = |got: Vec<CellValue>, want: &[Option<f64>]| {
            assert_eq!(got.len(), want.len());
            for (i, (g, w)) in got.iter().zip(want).enumerate() {
                match w {
                    None => assert_eq!(*g, NA, "{i}: {got:?}"),
                    Some(x) => {
                        let v = g.as_num().unwrap();
                        assert!((v - x).abs() <= 1e-9 * x.abs().max(1.0), "{i}: {v} vs {x}");
                    }
                }
            }
        };
        let double: Vec<f64> = x1.iter().map(|v| 2.0 * v).collect();
        let sum: Vec<f64> = x1.iter().zip(&x3).map(|(a, b)| a + b).collect();
        let (s, na) = (Some, None);
        // x2 = 2·x1: x2 dropped; the fit is y on x1 alone, df 4.
        check(
            fit(&[x1.to_vec(), double.clone()], true),
            &[
                s(0.0),
                s(0.972_049_689_440_993_8),
                s(0.357_142_857_142_857_2),
                s(0.0),
                s(0.233_539_546_714_543_1),
                s(1.022_430_338_797_216),
                s(0.812_420_701_722_379_3),
                s(1.209_755_891_397_18),
                na,
                s(17.324_314_765_694_08),
                s(4.0),
                na,
                s(25.354_296_066_252_59),
                s(5.854_037_267_080_745),
                na,
            ],
        );
        check(
            fit(&[x1.to_vec(), double], false),
            &[
                s(0.0),
                s(1.043_478_260_869_565),
                s(0.0),
                s(0.0),
                s(0.102_427_991_212_084_7),
                na,
                s(0.954_037_267_080_745_3),
                s(1.098_417_834_492_155),
                na,
                s(103.783_783_783_783_7),
                s(5.0),
                na,
                s(125.217_391_304_347_8),
                s(6.032_608_695_652_174),
                na,
            ],
        );
        // x1 + x3 after x1 and x3: dropped.
        check(
            fit(&[x1.to_vec(), x3.to_vec(), sum], true),
            &[
                s(0.0),
                s(-0.031_858_407_079_646_02),
                s(0.976_106_194_690_265_5),
                s(0.402_654_867_256_637_2),
                s(0.0),
                s(0.471_254_241_030_072_6),
                s(0.276_063_172_656_084_5),
                s(1.358_279_759_198_106),
                s(0.812_706_026_914_942_6),
                s(1.395_842_964_348_406),
                na,
                na,
                s(6.508_800_151_400_455),
                s(3.0),
                na,
                na,
                s(25.363_200_589_970_5),
                s(5.845_132_743_362_832),
                na,
                na,
            ],
        );
        // A constant x with const: collinear with b, dropped; without const
        // it is a regressor like any other.
        let three = vec![3.0; 6];
        check(
            fit(&[x1.to_vec(), three.clone()], true),
            &[
                s(0.0),
                s(0.972_049_689_440_993_8),
                s(0.357_142_857_142_857_2),
                s(0.0),
                s(0.233_539_546_714_543_1),
                s(1.022_430_338_797_216),
                s(0.812_420_701_722_379_3),
                s(1.209_755_891_397_18),
                na,
                s(17.324_314_765_694_08),
                s(4.0),
                na,
                s(25.354_296_066_252_59),
                s(5.854_037_267_080_745),
                na,
            ],
        );
        check(
            fit(&[x1.to_vec(), three], false),
            &[
                s(0.119_047_619_047_619),
                s(0.972_049_689_440_993_8),
                s(0.0),
                s(0.340_810_112_932_405_4),
                s(0.233_539_546_714_543_1),
                na,
                s(0.955_397_811_298_432_4),
                s(1.209_755_891_397_18),
                na,
                s(42.840_848_806_366),
                s(4.0),
                na,
                s(125.395_962_732_919_3),
                s(5.854_037_267_080_745),
                na,
            ],
        );
        // Every x zero without const: nothing to fit, all m 0 and F #N/A.
        let zero = vec![0.0; 6];
        let z = fit(&[zero.clone(), zero], false);
        assert_eq!(&z[..3], [n(0.0), n(0.0), n(0.0)]);
        assert_eq!(z[9], NA, "F with no x");
    }

    /// One x with residuals: LibreOffice 25.2 LINEST({1;2;4}; {1;2;3}; 1; 1).
    #[test]
    fn one_x_stats() {
        let r = linest(&yx(&[1.0, 2.0, 4.0], &[1.0, 2.0, 3.0]), &[T, T], false, 1).unwrap();
        let want = [
            1.5,
            -0.666_666_666_666_666_5,
            0.288_675_134_594_812_87,
            0.623_609_564_462_323_6,
            0.964_285_714_285_714_2,
            0.408_248_290_463_863,
            27.0,
            1.0,
            4.5,
            0.166_666_666_666_666_69,
        ];
        for (i, (g, w)) in r.iter().zip(want).enumerate() {
            let g = g.as_num().unwrap();
            assert!((g - w).abs() <= 1e-12 * w.abs(), "{i}: {g} vs {w}");
        }
        // Without const: m = 17/14, b 0 and se_b #N/A (LibreOffice).
        let nc = linest(&yx(&[1.0, 2.0, 4.0], &[1.0, 2.0, 3.0]), &[F, T], false, 1).unwrap();
        assert!((nc[0].as_num().unwrap() - 17.0 / 14.0).abs() < 1e-15);
        assert_eq!((nc[1].clone(), nc[3].clone()), (n(0.0), NA));
    }

    /// Two x's with stats against LibreOffice 25.2 LINEST(y; x1:x2; 1; 1):
    /// a 3-point exact fit (df 0: se 0, F #N/A), and a constant y (ss_tot
    /// 0: r² 1, F #N/A).
    #[test]
    fn several_xs_without_residual() {
        let xs = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
        let fit = |ys: &[f64]| {
            let r = rows(
                ys.iter()
                    .zip(xs)
                    .map(|(y, (a, b))| vec![n(*y), n(a), n(b)])
                    .collect(),
            );
            linest(&r, &[T, T], false, 2).unwrap()
        };
        let close = |v: &CellValue, x: f64| (v.as_num().unwrap() - x).abs() < 1e-12;
        let three = fit(&[1.0, 2.0, 4.0]);
        for (i, x) in [
            (0, 3.0),
            (1, 1.0),
            (2, 1.0),
            (3, 0.0),
            (6, 1.0),
            (7, 0.0),
            (10, 0.0),
        ] {
            assert!(close(&three[i], x), "{i}: {three:?}");
        }
        assert_eq!(three[9], NA);
        assert!(close(&three[12], 4.666_666_666_666_667));
        let flat = fit(&[2.0, 2.0, 2.0, 2.0]);
        for (i, x) in [(6, 1.0), (7, 0.0), (10, 1.0)] {
            assert!(close(&flat[i], x), "{i}: {flat:?}");
        }
        assert_eq!(flat[9], NA);
    }

    #[test]
    fn logest_and_errors() {
        let r = linest(&yx(&[1.0, 2.0, 4.0], &[1.0, 2.0, 3.0]), &[], true, 1).unwrap();
        assert!((r[0].as_num().unwrap() - 2.0).abs() < 1e-12);
        assert!((r[1].as_num().unwrap() - 0.5).abs() < 1e-12); // y = 0.5·2^x
        let bad = yx(&[1.0, 0.0], &[1.0, 2.0]);
        assert_eq!(linest(&bad, &[], true, 1), Err(err::NUM));
        // Excel: a non-number is #VALUE! (not skipped).
        let text = rows(vec![vec![CellValue::Text("a".into()), n(1.0)]]);
        assert_eq!(linest(&text, &[], false, 1), Err(err::VALUE));
        assert_eq!(
            linest(&rows(vec![vec![n(1.0)]]), &[], false, 1),
            Err(err::VALUE)
        );
        assert_eq!(
            linest(
                &yx(&[1.0], &[1.0]),
                &[CellValue::Text("x".into())],
                false,
                1
            ),
            Err(err::VALUE)
        );
        // Dates read as serial numbers.
        let d = rows(vec![
            vec![n(1.0), CellValue::Date(0)],
            vec![n(2.0), CellValue::Date(86_400_000)],
        ]);
        assert_eq!(linest(&d, &[], false, 1).unwrap()[0], n(1.0));
    }
}
