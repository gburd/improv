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
    s(1554, "LANDAU", C::Statistical, &[N], V::Number, "the Landau density at x (Kölbig and Schorr's approximation, about 9 digits)"),
    s(1555, "LAPLACE", C::Statistical, &[N, N], V::Number, "the Laplace density of width a at x"),
    s(1556, "LOGISTIC", C::Statistical, &[N, N], V::Number, "the logistic density of scale a at x"),
    s(1557, "PARETO", C::Statistical, &[N, N, N], V::Number, "the Pareto density of exponent a and scale b at x"),
    s(1558, "RAYLEIGH", C::Statistical, &[N, N], V::Number, "the Rayleigh density of scale σ at x"),
    s(1559, "RAYLEIGHTAIL", C::Statistical, &[N, N, N], V::Number, "the Rayleigh tail density (lower limit a, scale σ) at x"),
    s(1560, "SNORM.DIST.RANGE", C::Statistical, &[N, N], V::Number, "Φ(x2) − Φ(x1): the standard normal probability between x1 and x2"),
    s(1561, "OWENT", C::Statistical, &[N, N], V::Number, "Owen's T function T(h, a)"),
    s(1562, "CUM_BIV_NORM_DIST", C::Statistical, &[N, N, N], V::Number, "the standard bivariate normal P(X ≤ a, Y ≤ b) with correlation ρ, |ρ| < 1"),
    // --- R-style kernels: R.D*, R.P*, R.Q* (ids 1600 + 3 × family) ---
    so(1600, "R.DBETA", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the beta distribution of shapes a and b, or its logarithm (give_log)"),
    so(1601, "R.PBETA", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the beta distribution of shapes a and b, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1602, "R.QBETA", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the beta distribution of shapes a and b: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1603, "R.DBINOM", C::Statistical, &[N, N, N, B], 3, V::Number, "the probability mass at x of the binomial distribution of n trials of success probability psuc, or its logarithm (give_log)"),
    so(1604, "R.PBINOM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the binomial distribution of n trials of success probability psuc, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1605, "R.QBINOM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the binomial distribution of n trials of success probability psuc: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1606, "R.DCAUCHY", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the Cauchy distribution of location and scale, or its logarithm (give_log)"),
    so(1607, "R.PCAUCHY", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the Cauchy distribution of location and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1608, "R.QCAUCHY", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the Cauchy distribution of location and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1609, "R.DCHISQ", C::Statistical, &[N, N, B], 2, V::Number, "the density at x of the chi-square distribution of df degrees of freedom, or its logarithm (give_log)"),
    so(1610, "R.PCHISQ", C::Statistical, &[N, N, B, B], 2, V::Number, "P(X ≤ x) for the chi-square distribution of df degrees of freedom, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1611, "R.QCHISQ", C::Statistical, &[N, N, B, B], 2, V::Number, "the quantile of the chi-square distribution of df degrees of freedom: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1612, "R.DEXP", C::Statistical, &[N, N, B], 2, V::Number, "the density at x of the exponential distribution of the given scale (mean), or its logarithm (give_log)"),
    so(1613, "R.PEXP", C::Statistical, &[N, N, B, B], 2, V::Number, "P(X ≤ x) for the exponential distribution of the given scale (mean), or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1614, "R.QEXP", C::Statistical, &[N, N, B, B], 2, V::Number, "the quantile of the exponential distribution of the given scale (mean): the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1615, "R.DF", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the F distribution of n1 and n2 degrees of freedom, or its logarithm (give_log)"),
    so(1616, "R.PF", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the F distribution of n1 and n2 degrees of freedom, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1617, "R.QF", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the F distribution of n1 and n2 degrees of freedom: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1618, "R.DGAMMA", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the gamma distribution of shape and scale, or its logarithm (give_log)"),
    so(1619, "R.PGAMMA", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the gamma distribution of shape and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1620, "R.QGAMMA", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the gamma distribution of shape and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1621, "R.DGEOM", C::Statistical, &[N, N, B], 2, V::Number, "the probability mass at x of the geometric distribution (failures before the first success) of success probability psuc, or its logarithm (give_log)"),
    so(1622, "R.PGEOM", C::Statistical, &[N, N, B, B], 2, V::Number, "P(X ≤ x) for the geometric distribution (failures before the first success) of success probability psuc, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1623, "R.QGEOM", C::Statistical, &[N, N, B, B], 2, V::Number, "the quantile of the geometric distribution (failures before the first success) of success probability psuc: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1624, "R.DGUMBEL", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the Gumbel distribution of location μ and scale β, or its logarithm (give_log)"),
    so(1625, "R.PGUMBEL", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the Gumbel distribution of location μ and scale β, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1626, "R.QGUMBEL", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the Gumbel distribution of location μ and scale β: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1627, "R.DHYPER", C::Statistical, &[N, N, N, N, B], 4, V::Number, "the probability mass at x of the hypergeometric distribution of the red balls among n drawn from r red and b black, or its logarithm (give_log)"),
    so(1628, "R.PHYPER", C::Statistical, &[N, N, N, N, B, B], 4, V::Number, "P(X ≤ x) for the hypergeometric distribution of the red balls among n drawn from r red and b black, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1629, "R.QHYPER", C::Statistical, &[N, N, N, N, B, B], 4, V::Number, "the quantile of the hypergeometric distribution of the red balls among n drawn from r red and b black: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1630, "R.DLNORM", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the log-normal distribution of log-mean and log-sd, or its logarithm (give_log)"),
    so(1631, "R.PLNORM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the log-normal distribution of log-mean and log-sd, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1632, "R.QLNORM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the log-normal distribution of log-mean and log-sd: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1633, "R.DNBINOM", C::Statistical, &[N, N, N, B], 3, V::Number, "the probability mass at x of the negative binomial distribution (failures before the n-th success) of success probability psuc, or its logarithm (give_log)"),
    so(1634, "R.PNBINOM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the negative binomial distribution (failures before the n-th success) of success probability psuc, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1635, "R.QNBINOM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the negative binomial distribution (failures before the n-th success) of success probability psuc: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1636, "R.DNORM", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the normal distribution of mean μ and standard deviation σ, or its logarithm (give_log)"),
    so(1637, "R.PNORM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the normal distribution of mean μ and standard deviation σ, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1638, "R.QNORM", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the normal distribution of mean μ and standard deviation σ: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1639, "R.DPOIS", C::Statistical, &[N, N, B], 2, V::Number, "the probability mass at x of the Poisson distribution of mean λ, or its logarithm (give_log)"),
    so(1640, "R.PPOIS", C::Statistical, &[N, N, B, B], 2, V::Number, "P(X ≤ x) for the Poisson distribution of mean λ, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1641, "R.QPOIS", C::Statistical, &[N, N, B, B], 2, V::Number, "the quantile of the Poisson distribution of mean λ: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1642, "R.DRAYLEIGH", C::Statistical, &[N, N, B], 2, V::Number, "the density at x of the Rayleigh distribution of the given scale, or its logarithm (give_log)"),
    so(1643, "R.PRAYLEIGH", C::Statistical, &[N, N, B, B], 2, V::Number, "P(X ≤ x) for the Rayleigh distribution of the given scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1644, "R.QRAYLEIGH", C::Statistical, &[N, N, B, B], 2, V::Number, "the quantile of the Rayleigh distribution of the given scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1645, "R.DSNORM", C::Statistical, &[N, N, N, N, B], 4, V::Number, "the density at x of the skew-normal distribution of shape, location and scale, or its logarithm (give_log)"),
    so(1646, "R.PSNORM", C::Statistical, &[N, N, N, N, B, B], 4, V::Number, "P(X ≤ x) for the skew-normal distribution of shape, location and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1647, "R.QSNORM", C::Statistical, &[N, N, N, N, B, B], 4, V::Number, "the quantile of the skew-normal distribution of shape, location and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1648, "R.DST", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the skew-t distribution of n degrees of freedom and the given shape, or its logarithm (give_log)"),
    so(1649, "R.PST", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the skew-t distribution of n degrees of freedom and the given shape, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1650, "R.QST", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the skew-t distribution of n degrees of freedom and the given shape: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1651, "R.DT", C::Statistical, &[N, N, B], 2, V::Number, "the density at x of the Student t distribution of n degrees of freedom, or its logarithm (give_log)"),
    so(1652, "R.PT", C::Statistical, &[N, N, B, B], 2, V::Number, "P(X ≤ x) for the Student t distribution of n degrees of freedom, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1653, "R.QT", C::Statistical, &[N, N, B, B], 2, V::Number, "the quantile of the Student t distribution of n degrees of freedom: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1654, "R.DWEIBULL", C::Statistical, &[N, N, N, B], 3, V::Number, "the density at x of the Weibull distribution of shape and scale, or its logarithm (give_log)"),
    so(1655, "R.PWEIBULL", C::Statistical, &[N, N, N, B, B], 3, V::Number, "P(X ≤ x) for the Weibull distribution of shape and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1656, "R.QWEIBULL", C::Statistical, &[N, N, N, B, B], 3, V::Number, "the quantile of the Weibull distribution of shape and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
    so(1657, "R.PTUKEY", C::Statistical, &[N, N, N, N, B, B], 3, V::Number, "P(Q ≤ x) for the largest of nranges (default 1) studentized ranges of nmeans means with df degrees of freedom, or P(Q > x) (lower_tail = FALSE), or its logarithm (log_p)"),
    so(1658, "R.QTUKEY", C::Statistical, &[N, N, N, N, B, B], 3, V::Number, "the quantile of the studentized range of nmeans means with df degrees of freedom, the largest of nranges (default 1): the x with P(Q ≤ x) = p, or of P(Q > x) (lower_tail = FALSE), p may be a logarithm (log_p)"),
];
