//! LINEST and LOGEST of one x variable: the least-squares line, and with
//! `stats` Excel's 5×2 block of regression statistics flattened row by row.

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

/// How many values LINEST/LOGEST return: 10 with stats, else 2.
pub(super) fn len(a: &[CellValue]) -> usize {
    if a.len() > 1 && b(a, 1) == Ok(true) {
        10
    } else {
        2
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
pub(super) fn linest(rows: &Rows, a: &[CellValue], log: bool) -> List {
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
            linest(&two, &[T, T], false).unwrap(),
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
        let r = linest(&line, &[F, T], false).unwrap();
        assert_eq!(r[..4], [n(2.0), n(0.0), n(0.0), NA]);
        // A constant x: b is the mean (Excel: LINEST({0,0}, {1,1}) is 0).
        let flat = yx(&[1.0, 3.0, 5.0], &[2.0, 2.0, 2.0]);
        assert_eq!(linest(&flat, &[], false).unwrap(), [n(0.0), n(3.0)]);
        let r = linest(&flat, &[T, T], false).unwrap();
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
        let r = linest(&zero, &[F, T], false).unwrap();
        assert_eq!(r[..4], [n(0.0), n(0.0), n(0.0), NA]);
        assert_eq!(r[7], n(2.0));
    }

    #[test]
    fn logest_and_errors() {
        let r = linest(&yx(&[1.0, 2.0, 4.0], &[1.0, 2.0, 3.0]), &[], true).unwrap();
        assert!((r[0].as_num().unwrap() - 2.0).abs() < 1e-12);
        assert!((r[1].as_num().unwrap() - 0.5).abs() < 1e-12); // y = 0.5·2^x
        let bad = yx(&[1.0, 0.0], &[1.0, 2.0]);
        assert_eq!(linest(&bad, &[], true), Err(err::NUM));
        // Excel: a non-number is #VALUE! (not skipped).
        let text = rows(vec![vec![CellValue::Text("a".into()), n(1.0)]]);
        assert_eq!(linest(&text, &[], false), Err(err::VALUE));
        assert_eq!(
            linest(&rows(vec![vec![n(1.0)]]), &[], false),
            Err(err::VALUE)
        );
        assert_eq!(
            linest(&yx(&[1.0], &[1.0]), &[CellValue::Text("x".into())], false),
            Err(err::VALUE)
        );
        // Dates read as serial numbers.
        let d = rows(vec![
            vec![n(1.0), CellValue::Date(0)],
            vec![n(2.0), CellValue::Date(86_400_000)],
        ]);
        assert_eq!(linest(&d, &[], false).unwrap()[0], n(1.0));
    }
}
