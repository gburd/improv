//! Evaluation of the P7 analysis reductions (ids 1850–1999).

use super::Rows;
use crate::builtins::err;
use crate::CellValue;

/// One result per group.
pub(super) fn reduce(_id: u32, _rows: &Rows, _params: &[CellValue]) -> CellValue {
    CellValue::Err(err::NAME)
}

/// One result per row.
pub(super) fn per_item(_id: u32, rows: &Rows, _params: &[CellValue]) -> Vec<CellValue> {
    vec![CellValue::Err(err::NAME); rows.len()]
}

/// A list of results, laid along the INTO category.
pub(super) fn array(_id: u32, _rows: &Rows, _params: &[CellValue]) -> Vec<CellValue> {
    vec![CellValue::Err(err::NAME)]
}
