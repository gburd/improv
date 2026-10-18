//! Evaluation of the P7 more distributions and Gnumeric's R-style distribution kernels (ids 1550–1749).

use super::{err, R};
use crate::CellValue;

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(_id: u32, _a: &[CellValue]) -> R {
    Err(err::NAME)
}
