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
/// #N/A; `ss_reg, ss_resid` then #N/A. Collinear x's are #NUM! (Excel
/// drops them; that is not done here).
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
    if n < p {
        return Err(err::NUM);
    }
    // XᵀX | Xᵀy, then XᵀX⁻¹ (for the standard errors) by Gauss–Jordan.
    let mut m = vec![vec![0.0; 2 * p + 1]; p];
    for (row, y) in xs.iter().zip(&ys) {
        for i in 0..p {
            for j in 0..p {
                m[i][j] += row[i] * row[j];
            }
            m[i][2 * p] += row[i] * y;
        }
    }
    for (i, r) in m.iter_mut().enumerate() {
        r[p + i] = 1.0;
    }
    for c in 0..p {
        let piv = (c..p)
            .max_by(|x, y| m[*x][c].abs().total_cmp(&m[*y][c].abs()))
            .unwrap_or(c);
        if m[piv][c].abs() < 1e-12 {
            return Err(err::NUM);
        }
        m.swap(c, piv);
        let d = m[c][c];
        m[c].iter_mut().for_each(|v| *v /= d);
        for r in 0..p {
            if r != c {
                let f = m[r][c];
                let pivot = m[c].clone();
                m[r].iter_mut().zip(pivot).for_each(|(v, q)| *v -= f * q);
            }
        }
    }
    let coef: Vec<f64> = m.iter().map(|r| r[2 * p]).collect();
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
    let df = n - p;
    let se_y = if df > 0 {
        (ss_resid / df as f64).sqrt()
    } else {
        0.0
    };
    let se = |j: usize| CellValue::num(se_y * m[j][p + j].max(0.0).sqrt());
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
    res.push(if df > 0 && ss_resid > 0.0 {
        CellValue::num((ss_reg / k as f64) / (ss_resid / df as f64))
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
        assert!(close(&exact[0], 3.0) && close(&exact[1], 2.0) && close(&exact[2], 1.0));
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
        // Collinear x's, too few rows, a bad value.
        let col = rows(vec![
            vec![n(1.0), n(1.0), n(2.0)],
            vec![n(2.0), n(2.0), n(4.0)],
            vec![n(3.0), n(3.0), n(6.0)],
        ]);
        assert_eq!(linest(&col, &[], false, 2), Err(err::NUM));
        assert_eq!(
            linest(&rows(vec![vec![n(1.0), n(1.0), n(2.0)]]), &[], false, 2),
            Err(err::NUM)
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
