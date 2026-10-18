//! Evaluation of the P7 engineering: base conversion, bits, complex numbers, Bessel functions, units (ids 1300–1449).

use super::{err, R};
use crate::CellValue;

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(_id: u32, _a: &[CellValue]) -> R {
    Err(err::NAME)
}
