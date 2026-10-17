//! The array functions (P6): a list result laid along an `INTO` category.

use super::{f, over, Arg, Category, FnDef, Kind, Over, Ret};

use Arg::{Any as A, Boolean as B, Number as N};

#[rustfmt::skip]
pub(super) static ARRAY: &[FnDef] = &[
    over(f(1250, "SORT", Kind::Array, Category::Dimensional, &[A, N], 1, false, Ret::Unify, "the values in ascending order (descending when order is -1)"), Over::At(&[0])),
    over(f(1251, "UNIQUE", Kind::Array, Category::Dimensional, &[A, B], 1, false, Ret::Unify, "each distinct value once, in item order (only values that occur once when exactly_once is TRUE)"), Over::At(&[0])),
];
