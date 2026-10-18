//! P7: analysis reductions (regression and forecasting, normality tests,
//! matrices, signal processing). Ids 1850–1999.
//!
//! A matrix is a measure over two categories, `MDETERM(A OVER R, C)`: rows
//! along R, columns along C, in item order. A matrix result is a list
//! flattened row by row; `INTO R2, C2` lays it out as the block.

use super::{f, over, Arg, Category, FnDef, Kind, Over, Ret};
use crate::value::ValueType;

use Arg::Number as N;
use Category::Math as M;
use Kind::{Aggregate as Ag, Array as Ar};

const NUM: Ret = Ret::Fixed(ValueType::Number);

#[rustfmt::skip]
pub(super) static ANALYSIS: &[FnDef] = &[
    over(f(1850, "MDETERM", Ag, M, &[N], 1, false, NUM, "MDETERM(A OVER R, C): the determinant of the square matrix A (rows along R, columns along C); 0 if singular"), Over::At(&[0])),
    over(f(1851, "LOGMDETERM", Ag, M, &[N], 1, false, NUM, "LOGMDETERM(A OVER R, C): ln |det A| (no overflow for large matrices); #NUM! if singular"), Over::At(&[0])),
    over(f(1852, "MINVERSE", Ar, M, &[N], 1, false, NUM, "MINVERSE(A OVER R, C INTO R2, C2): the inverse of the square matrix A, row by row; #NUM! if singular"), Over::At(&[0])),
    over(f(1853, "MPSEUDOINVERSE", Ar, M, &[N, N], 1, false, NUM, "MPSEUDOINVERSE(A OVER R, C, [threshold] INTO C2, R2): the Moore–Penrose pseudo-inverse (C × R, row by row); singular values up to threshold (default 256 ε) times the largest count as zero"), Over::At(&[0])),
];
