//! Evaluation of the P7 analysis reductions (ids 1850–1999): matrices
//! (`matrix`), and the regression, forecasting, signal-processing and
//! normality-test functions over one category.
//!
//! An error among the rows is the result (the first in item order; for a
//! list, its one value).

use super::Rows;
use crate::builtins::{err, n_or, num};
use crate::CellValue;

mod matrix;
mod series;

type List = Result<Vec<CellValue>, u8>;

/// One result per group.
pub(super) fn reduce(id: u32, rows: &Rows, params: &[CellValue]) -> CellValue {
    if let Some(bad) = rows.first_error() {
        return bad;
    }
    let x = match id {
        1850 | 1851 => matrix::det(rows, id == 1851),
        1856 => ssmedian(rows, params),
        1857 => series::interpolation(rows, params),
        _ => Err(err::NAME),
    };
    x.map_or_else(CellValue::Err, num)
}

/// One result per row.
pub(super) fn per_item(id: u32, rows: &Rows, params: &[CellValue]) -> Vec<CellValue> {
    let out = match rows.first_error() {
        Some(CellValue::Err(c)) => Err(c),
        _ => match id {
            1858 => series::hpfilter(rows, params),
            _ => Err(err::NAME),
        },
    };
    match out {
        Ok(v) => v.into_iter().map(num).collect(),
        Err(c) => vec![CellValue::Err(c); rows.len()],
    }
}

/// SSMEDIAN(x OVER T, [interval = 1]): Gnumeric's median of grouped data.
/// Each value is the midpoint of an interval of that width; within the
/// median's interval the values are spread evenly: L + interval · (n/2 −
/// below) / within. One or two values: their mean; an even count whose
/// middle two differ: their mean. Non-numbers are skipped; none, or an
/// interval ≤ 0, is #NUM!.
fn ssmedian(rows: &Rows, a: &[CellValue]) -> Result<f64, u8> {
    let mut xs: Vec<f64> = rows.col(0).filter_map(super::statistical::number).collect();
    xs.sort_by(f64::total_cmp);
    let width = n_or(a, 0, 1.0)?;
    let len = xs.len();
    if width <= 0.0 || len == 0 {
        return Err(err::NUM);
    }
    let grouped = |mid: f64| {
        let lo = mid - width / 2.0;
        let below = xs.iter().filter(|x| **x < lo).count();
        let within = xs
            .iter()
            .filter(|x| **x >= lo && **x <= mid + width / 2.0)
            .count();
        lo + (len as f64 / 2.0 - below as f64) * width / within as f64
    };
    Ok(match len {
        1 => xs[0],
        2 => (xs[0] + xs[1]) / 2.0,
        _ if len % 2 == 1 => grouped(xs[len / 2]),
        _ if xs[len / 2 - 1] == xs[len / 2] => grouped(xs[len / 2]),
        _ => (xs[len / 2 - 1] + xs[len / 2]) / 2.0,
    })
}

/// A list of results, laid along the INTO categories (row-major).
pub(super) fn array(id: u32, rows: &Rows, params: &[CellValue]) -> Vec<CellValue> {
    if let Some(bad) = rows.first_error() {
        return vec![bad];
    }
    let out = match id {
        1852 => matrix::inverse(rows),
        1853 => matrix::pseudo_inverse(rows, params),
        1854 => matrix::cholesky(rows),
        1855 => matrix::eigen(rows),
        _ => Err(err::NAME),
    };
    out.unwrap_or_else(|c| vec![CellValue::Err(c)])
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn errors_and_unknown_ids() {
        let r = Rows {
            pos: vec![vec![0, 0]],
            vals: vec![vec![CellValue::Err(err::DIV0)]],
            capacity: 1,
        };
        assert_eq!(reduce(1850, &r, &[]), CellValue::Err(err::DIV0));
        assert_eq!(array(1852, &r, &[]), vec![CellValue::Err(err::DIV0)]);
        let ok = Rows {
            pos: vec![vec![0, 0]],
            vals: vec![vec![CellValue::num(2.0)]],
            capacity: 1,
        };
        assert_eq!(reduce(1999, &ok, &[]), CellValue::Err(err::NAME));
        assert_eq!(array(1999, &ok, &[]), vec![CellValue::Err(err::NAME)]);
        assert_eq!(per_item(1999, &ok, &[]), vec![CellValue::Err(err::NAME)]);
        assert_eq!(per_item(1858, &r, &[]), vec![CellValue::Err(err::DIV0)]);
        assert_eq!(
            reduce(1856, &ok, &[CellValue::Text("x".into())]),
            CellValue::Err(err::VALUE)
        );
    }
}
