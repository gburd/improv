//! The array functions (P6): a list result laid along an `INTO` category.

use super::{f, over, Arg, Category, FnDef, Kind, Over, Ret};
use crate::value::ValueType;

use Arg::{Any as A, Boolean as B, Number as N, Text as T};
use Category::{Dimensional as D, Statistical as St};

const NUM: Ret = Ret::Fixed(ValueType::Number);

#[rustfmt::skip]
pub(super) static ARRAY: &[FnDef] = &[
    over(f(1250, "SORT", Kind::Array, D, &[A, N], 1, false, Ret::Unify, "the values in ascending order (descending when order is -1)"), Over::At(&[0])),
    over(f(1251, "UNIQUE", Kind::Array, D, &[A, B], 1, false, Ret::Unify, "each distinct value once, in item order (only values that occur once when exactly_once is TRUE)"), Over::At(&[0])),
    over(f(1252, "FILTER", Kind::Array, D, &[A, B, A], 2, false, Ret::Unify, "the values of x whose include is TRUE, in item order; if none, if_empty (else #N/A: Excel's #CALC!)"), Over::At(&[0, 1])),
    over(f(1253, "SORTBY", Kind::Array, D, &[A, A, N], 2, false, Ret::Unify, "the values of x sorted by the values of by (stable; descending when order is -1); one sort key"), Over::At(&[0, 1])),
    over(f(1254, "TAKE", Kind::Array, D, &[A, N], 2, false, Ret::Unify, "the first n values in item order (the last -n when n is negative)"), Over::At(&[0])),
    over(f(1255, "DROP", Kind::Array, D, &[A, N], 2, false, Ret::Unify, "the values TAKE leaves out: all but the first n (the last -n when n is negative), in item order"), Over::At(&[0])),
    over(f(1256, "MODE.MULT", Kind::Array, St, &[N], 1, false, NUM, "every most frequent value, in order of first occurrence (#N/A if none repeats)"), Over::At(&[0])),
    over(f(1257, "LINEST", Kind::Array, St, &[N, N, B, B], 2, false, NUM, "the least-squares line y = m·x + b of one x: m, b (const = FALSE: b = 0); with stats, Excel's 5×2 block row by row: m, b, se_m, se_b, r², se_y, F, df, ss_reg, ss_resid"), Over::At(&[0, 1])),
    over(f(1258, "LOGEST", Kind::Array, St, &[N, N, B, B], 2, false, NUM, "the curve y = b·m^x fitted to ln y: m, b (const = FALSE: b = 1); with stats the 10 values of LINEST's block, those after b for ln y"), Over::At(&[0, 1])),
    over(f(1259, "TEXTSPLIT", Kind::Array, Category::Text, &[T, T, T, B], 2, false, Ret::Fixed(ValueType::Text), "the parts of the text between col_delimiters (case-sensitive), in order; written without OVER, each cell splits on its own: TEXTSPLIT(Text, \",\" INTO Part). row_delimiter must be \"\" (splitting into rows is not supported yet); ignore_empty drops empty parts; empty text is #VALUE!"), Over::At(&[0])),
    over(f(1260, "FREQUENCY", Kind::Array, St, &[N, N], 2, true, NUM, "counts of x in the bins given as numbers b1, b2, …: ≤ b1, (b1, b2], …, > bn (n + 1 counts; Excel takes the bins as a range)"), Over::At(&[0])),
];
