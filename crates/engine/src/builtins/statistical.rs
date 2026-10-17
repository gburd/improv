//! Evaluation of the statistical functions (see `improv_core_model::functions`).

#[allow(unused_imports)]
use super::{b, d, domain, e, err, n, n_or, nums, ok_num, ok_text, serial, t, R};
#[allow(unused_imports)]
use crate::CellValue;

/// Evaluate statistical function `id` (already type- and arity-checked).
pub(super) fn eval(_id: u32, _a: &[CellValue]) -> R {
    // Filled in as the family lands; an unknown id is #NAME?.
    Err(err::NAME)
}
