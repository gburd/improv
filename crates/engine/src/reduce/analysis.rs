//! Evaluation of the P7 analysis reductions (ids 1850–1999): matrices
//! (`matrix`), and the regression, forecasting, signal-processing and
//! normality-test functions over one category.
//!
//! An error among the rows is the result (the first in item order; for a
//! list, its one value).

use super::Rows;
use crate::builtins::{err, num};
use crate::CellValue;

mod matrix;

type List = Result<Vec<CellValue>, u8>;

/// One result per group.
pub(super) fn reduce(id: u32, rows: &Rows, _params: &[CellValue]) -> CellValue {
    if let Some(bad) = rows.first_error() {
        return bad;
    }
    let x = match id {
        1850 | 1851 => matrix::det(rows, id == 1851),
        _ => Err(err::NAME),
    };
    x.map_or_else(CellValue::Err, num)
}

/// One result per row.
pub(super) fn per_item(_id: u32, rows: &Rows, _params: &[CellValue]) -> Vec<CellValue> {
    vec![CellValue::Err(err::NAME); rows.len()]
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
    }
}
