//! The array functions (P6): a list result laid along an `INTO` category.

use super::{f, over, Arg, Category, FnDef, Kind, Over, Ret};
use crate::value::ValueType;

use Arg::{Any as A, Boolean as B, Number as N, Text as T};
use Category::{Dimensional as D, Statistical as St};

const NUM: Ret = Ret::Fixed(ValueType::Number);

#[rustfmt::skip]
pub(super) static ARRAY: &[FnDef] = &[
    over(f(1250, "SORT", Kind::Array, D, &[A, N], 1, false, Ret::Unify, "the values in ascending order (descending when sort_order is -1)"), Over::At(&[0])),
    over(f(1251, "UNIQUE", Kind::Array, D, &[A, B], 1, false, Ret::Unify, "each distinct value once, in item order (only values that occur once when exactly_once is TRUE)"), Over::At(&[0])),
    over(f(1252, "FILTER", Kind::Array, D, &[A, B, A], 2, false, Ret::Unify, "the values of array whose include is TRUE, in item order; if none, if_empty (else #N/A: Excel's #CALC!)"), Over::At(&[0, 1])),
    over(f(1253, "SORTBY", Kind::Array, D, &[A, A, A], 2, true, Ret::Unify, "SORTBY(array, by_array1 OVER T, [sort_order1], [by_array2, sort_order2], …): the values of array sorted by by_array1, ties by by_array2, … (stable; a key descends when its order is -1)"), Over::AtEvery(&[0, 1], 3, 2)),
    over(f(1254, "TAKE", Kind::Array, D, &[A, N], 2, false, Ret::Unify, "the first rows values in item order (the last -rows when rows is negative)"), Over::At(&[0])),
    over(f(1255, "DROP", Kind::Array, D, &[A, N], 2, false, Ret::Unify, "the values TAKE leaves out: all but the first rows (the last -rows when rows is negative), in item order"), Over::At(&[0])),
    over(f(1256, "MODE.MULT", Kind::Array, St, &[N], 1, false, NUM, "every most frequent value, in order of first occurrence (#N/A if none repeats)"), Over::At(&[0])),
    over(f(1257, "LINEST", Kind::Array, St, &[N, N, B, B], 2, true, NUM, "LINEST(known_ys, known_xs1, [known_xs2, …] OVER T, [const], [stats]): the least-squares fit y = m1·x1 + … + b: mk, …, m1, b (const = FALSE: b = 0); with stats, Excel's 5×(k+1) block row by row (one x: m, b, se_m, se_b, r², se_y, F, df, ss_reg, ss_resid); INTO R, C lays it out as the block"), Over::Leading(2)),
    over(f(1258, "LOGEST", Kind::Array, St, &[N, N, B, B], 2, true, NUM, "LOGEST(known_ys, known_xs1, [known_xs2, …] OVER T, [const], [stats]): y = b·m1^x1·…, fitted to ln y: mk, …, m1, b (const = FALSE: b = 1); with stats LINEST's block, for ln y"), Over::Leading(2)),
    over(f(1259, "TEXTSPLIT", Kind::Array, Category::Text, &[T, T, T, B], 2, false, Ret::Fixed(ValueType::Text), "the parts of the text between col_delimiter matches (case-sensitive), in order; written without OVER, each cell splits on its own: TEXTSPLIT(Text, \",\" INTO Part). row_delimiter must be \"\" (splitting into rows is not supported yet); ignore_empty drops empty parts; empty text is #VALUE!"), Over::At(&[0])),
    over(f(1261, "SELECTS", Kind::Array, Category::Dimensional, &[A, A, A], 3, false, Ret::Unify, "Quantrix: the values whose test meets the condition (a criterion like \">5\" or \"a*\", as in SUMIF), in item order"), Over::At(&[0, 1])),
    over(f(1260, "FREQUENCY", Kind::Array, St, &[N, N], 2, true, NUM, "counts of x in the bins given as numbers b1, b2, …: ≤ b1, (b1, b2], …, > bn (n + 1 counts; Excel takes the bins as a range)"), Over::At(&[0])),
];
