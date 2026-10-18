//! Evaluation of the P7 option pricing (Gnumeric's and LibreOffice's) (ids 1750–1849).

use super::{err, R};
use crate::CellValue;

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(_id: u32, _a: &[CellValue]) -> R {
    Err(err::NAME)
}
