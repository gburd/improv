//! P7: analysis reductions (regression and forecasting, normality tests,
//! matrices, signal processing). Ids 1850–1999.
//!
//! A matrix is a measure over two categories, `MDETERM(A OVER R, C)`: rows
//! along R, columns along C, in item order. A matrix result is a list
//! flattened row by row; `INTO R2, C2` lays it out as the block.

use super::{f, over, Arg, Category, FnDef, Kind, Over, Ret};
use crate::value::ValueType;

use Arg::Number as N;
use Category::{Math as M, Statistical as St};
use Kind::{Aggregate as Ag, Array as Ar, PerItem as P};

const NUM: Ret = Ret::Fixed(ValueType::Number);

#[rustfmt::skip]
pub(super) static ANALYSIS: &[FnDef] = &[
    over(f(1850, "MDETERM", Ag, M, &[N], 1, false, NUM, "MDETERM(A OVER R, C): the determinant of the square matrix A (rows along R, columns along C); 0 if singular"), Over::At(&[0])),
    over(f(1851, "LOGMDETERM", Ag, M, &[N], 1, false, NUM, "LOGMDETERM(A OVER R, C): ln |det A| (no overflow for large matrices); #NUM! if singular"), Over::At(&[0])),
    over(f(1852, "MINVERSE", Ar, M, &[N], 1, false, NUM, "MINVERSE(A OVER R, C INTO R2, C2): the inverse of the square matrix A, row by row; #NUM! if singular"), Over::At(&[0])),
    over(f(1853, "MPSEUDOINVERSE", Ar, M, &[N, N], 1, false, NUM, "MPSEUDOINVERSE(A OVER R, C, [threshold] INTO C2, R2): the Moore–Penrose pseudo-inverse (C × R, row by row); singular values up to threshold (default 256 ε) times the largest count as zero"), Over::At(&[0])),
    over(f(1854, "CHOLESKY", Ar, M, &[N], 1, false, NUM, "CHOLESKY(A OVER R, C INTO R2, C2): the lower-triangular L with L·Lᵀ = A for a symmetric positive-definite A (asymmetric pairs averaged), row by row; #NUM! where A is not positive definite"), Over::At(&[0])),
    over(f(1855, "EIGEN", Ar, M, &[N], 1, false, NUM, "EIGEN(A OVER R, C INTO R2, C2): for a symmetric A (asymmetric pairs averaged), its eigenvalues largest magnitude first, then the matrix of their unit eigenvectors as columns: (n + 1) × n values, row by row"), Over::At(&[0])),
    over(f(1856, "SSMEDIAN", Ag, St, &[N, N], 1, false, NUM, "the median of grouped data (Gnumeric): each value the midpoint of an interval of width interval (default 1), interpolated within the median's interval"), Over::At(&[0])),
    over(f(1857, "INTERPOLATION", Ag, St, &[N, N, N, N, N], 3, false, NUM, "INTERPOLATION(xs, ys OVER T, target, [method], [next]): ys interpolated at target (Gnumeric): method 0 linear, 2 staircase, 4 natural cubic spline; 1, 3, 5 the same averaged over [target, next]"), Over::At(&[0, 1])),
    over(f(1858, "HPFILTER", P, St, &[N, N], 1, false, NUM, "each item's trend under the Hodrick–Prescott filter with smoothing λ (default 1600); the cycle is y − trend; needs 6 values"), Over::At(&[0])),
];
