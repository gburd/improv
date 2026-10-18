//! Evaluation of the P7 more math: special functions, number theory, floating point, calendars, text extras (ids 1450–1549).

use super::{err, R};
use crate::CellValue;

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(_id: u32, _a: &[CellValue]) -> R {
    Err(err::NAME)
}
