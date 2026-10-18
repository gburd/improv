//! P7: more distributions and Gnumeric's R-style distribution kernels. Ids 1550–1749.
//!
//! 1550–1599: Gnumeric's extra distributions (`BERNOULLI` … `CUM_BIV_NORM_DIST`).
//! 1600–1699: Gnumeric's R-style kernels `R.D*` (density), `R.P*` (CDF) and
//! `R.Q*` (quantile), with R's `lower_tail` (default TRUE) and
//! `log_p`/`give_log` (default FALSE) flags, in Gnumeric's argument order.

use super::{s, so, Arg, Category, FnDef};
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
    // --- R-style kernels: R.D*, R.P*, R.Q* (ids 1600 + 3 × family) ---
    so(1606, "R.DCAUCHY", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the Cauchy distribution of location and scale, or its logarithm (give_log)"),
    so(1607, "R.PCAUCHY", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the Cauchy distribution of location and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1608, "R.QCAUCHY", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the Cauchy distribution of location and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1612, "R.DEXP", C::Statistical, &[N, N, B], 2, V::Number, "the density at x of the exponential distribution of the given scale (mean), or its logarithm (give_log)"),
    so(1613, "R.PEXP", C::Statistical, &[N, N, B, B], 2, V::Number, "P(X ≤ x) for the exponential distribution of the given scale (mean), or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1614, "R.QEXP", C::Statistical, &[N, N, B, B], 2, V::Number, "the quantile of the exponential distribution of the given scale (mean): the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1624, "R.DGUMBEL", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the Gumbel distribution of location μ and scale β, or its logarithm (give_log)"),
    so(1625, "R.PGUMBEL", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the Gumbel distribution of location μ and scale β, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1626, "R.QGUMBEL", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the Gumbel distribution of location μ and scale β: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1630, "R.DLNORM", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the log-normal distribution of log-mean and log-sd, or its logarithm (give_log)"),
    so(1631, "R.PLNORM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the log-normal distribution of log-mean and log-sd, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1632, "R.QLNORM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the log-normal distribution of log-mean and log-sd: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1636, "R.DNORM", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the normal distribution of mean μ and standard deviation σ, or its logarithm (give_log)"),
    so(1637, "R.PNORM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the normal distribution of mean μ and standard deviation σ, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1638, "R.QNORM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the normal distribution of mean μ and standard deviation σ: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1642, "R.DRAYLEIGH", C::Statistical, &[N, N, B], 2, V::Number, "the density at x of the Rayleigh distribution of the given scale, or its logarithm (give_log)"),
    so(1643, "R.PRAYLEIGH", C::Statistical, &[N, N, B, B], 2, V::Number, "P(X ≤ x) for the Rayleigh distribution of the given scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1644, "R.QRAYLEIGH", C::Statistical, &[N, N, B, B], 2, V::Number, "the quantile of the Rayleigh distribution of the given scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1654, "R.DWEIBULL", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the Weibull distribution of shape and scale, or its logarithm (give_log)"),
    so(1655, "R.PWEIBULL", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the Weibull distribution of shape and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1656, "R.QWEIBULL", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the Weibull distribution of shape and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
];
