//! P7: more distributions and Gnumeric's R-style distribution kernels. Ids 1550–1749.
//!
//! 1550–1599: Gnumeric's extra distributions (`BERNOULLI` … `CUM_BIV_NORM_DIST`).
//! 1600–1699: Gnumeric's R-style kernels `R.D*` (density), `R.P*` (CDF) and
//! `R.Q*` (quantile), with R's `lower_tail` (default TRUE) and
//! `log_p`/`give_log` (default FALSE) flags, in Gnumeric's argument order.

use super::{s, Arg, Category, FnDef};
use crate::value::ValueType;

use Arg::{Boolean as B, Number as N};
use Category as C;
use ValueType as V;

#[rustfmt::skip]
pub(super) static DIST_EXTRA: &[FnDef] = &[
    s(1550, "BERNOULLI", C::Statistical, &[N, N], V::Number, "the Bernoulli probability of k (0 or 1) for success probability p"),
    s(1551, "CAUCHY", C::Statistical, &[N, N, B], V::Number, "the Cauchy distribution of scale a at x: density, or (as Gnumeric) the right tail P(X > x)"),
    s(1552, "EXPPOWDIST", C::Statistical, &[N, N, N], V::Number, "the exponential-power density of scale a and exponent b at x"),
    s(1553, "GEOMDIST", C::Statistical, &[N, N, B], V::Number, "the geometric probability of k failures before the first success: P(K = k), or P(K ≤ k)"),
    s(1555, "LAPLACE", C::Statistical, &[N, N], V::Number, "the Laplace density of width a at x"),
    s(1556, "LOGISTIC", C::Statistical, &[N, N], V::Number, "the logistic density of scale a at x"),
    s(1557, "PARETO", C::Statistical, &[N, N, N], V::Number, "the Pareto density of exponent a and scale b at x"),
    s(1558, "RAYLEIGH", C::Statistical, &[N, N], V::Number, "the Rayleigh density of scale σ at x"),
    s(1559, "RAYLEIGHTAIL", C::Statistical, &[N, N, N], V::Number, "the Rayleigh tail density (lower limit a, scale σ) at x"),
    s(1560, "SNORM.DIST.RANGE", C::Statistical, &[N, N], V::Number, "Φ(x2) − Φ(x1): the standard normal probability between x1 and x2"),
];
