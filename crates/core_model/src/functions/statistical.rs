//! The statistical functions (see the functions plan): the special functions
//! (ids 160–199, [`Category::Math`]) and the probability distributions
//! (ids 500–699, [`Category::Statistical`]).
//!
//! A legacy name is an alias only when it takes the same arguments and means
//! the same thing (`NORMDIST` = `NORM.DIST`); otherwise it has its own id
//! (`NORMSDIST(z)` vs `NORM.S.DIST(z, cumulative)`, `TDIST`'s `tails`).

#[allow(unused_imports)]
use super::{agg, alias, f, over, s, so, sv, Arg, Category, FnDef, Kind, Over, Ret};
#[allow(unused_imports)]
use crate::value::ValueType;

use Arg::{Boolean as B, Number as N};
use Category as C;
use ValueType as V;

#[rustfmt::skip]
pub(super) static STATISTICAL: &[FnDef] = &[
    // --- special functions ---
    s(160, "GAMMA", C::Math, &[N], V::Number, "the gamma function Γ(x)"),
    alias(s(161, "GAMMALN", C::Math, &[N], V::Number, "ln Γ(x), for x > 0"), &["GAMMALN.PRECISE"]),
    s(162, "BETA", C::Math, &[N, N], V::Number, "the Euler beta function B(a, b)"),
    s(163, "BETALN", C::Math, &[N, N], V::Number, "ln |B(a, b)|"),
    s(164, "DIGAMMA", C::Math, &[N], V::Number, "the digamma function ψ(x)"),
    so(165, "IGAMMA", C::Math, &[N, N, B, B, B], 2, V::Number, "the incomplete gamma function of a and x: lower (default) or upper, regularized (default) or not; the imaginary part (real = FALSE) is 0"),
    s(166, "POCHHAMMER", C::Math, &[N, N], V::Number, "the rising factorial Γ(x + n) / Γ(x)"),
    so(167, "ERF", C::Math, &[N, N], 1, V::Number, "the error function erf(x), or erf(upper) − erf(lower)"),
    s(169, "ERF.PRECISE", C::Math, &[N], V::Number, "the error function erf(x)"),
    alias(s(168, "ERFC", C::Math, &[N], V::Number, "the complementary error function 1 − erf(x)"), &["ERFC.PRECISE"]),
    // --- distributions ---
    so(500, "BETA.DIST", C::Statistical, &[N, N, N, B, N, N], 4, V::Number, "the beta distribution of x on [A, B] (default [0, 1]): density, or cumulative"),
    so(501, "BETADIST", C::Statistical, &[N, N, N, N, N], 3, V::Number, "the cumulative beta distribution of x on [A, B] (default [0, 1])"),
    alias(so(502, "BETA.INV", C::Statistical, &[N, N, N, N, N], 3, V::Number, "the beta quantile on [A, B] (default [0, 1])"), &["BETAINV"]),
    alias(s(503, "BINOM.DIST", C::Statistical, &[N, N, N, B], V::Number, "the binomial probability of k successes in n trials: P(K = k), or P(K ≤ k)"), &["BINOMDIST"]),
    alias(s(504, "BINOM.INV", C::Statistical, &[N, N, N], V::Number, "the smallest k with binomial P(K ≤ k) ≥ alpha, for n trials of probability p"), &["CRITBINOM"]),
    alias(so(505, "BINOM.DIST.RANGE", C::Statistical, &[N, N, N, N], 3, V::Number, "the binomial P(k1 ≤ K ≤ k2) for n trials of probability p (k2 defaults to k1)"), &["B"]),
    alias(s(506, "CHISQ.DIST", C::Statistical, &[N, N, B], V::Number, "the chi-square distribution: density, or left tail"), &["CHISQDIST"]),
    alias(s(507, "CHISQ.DIST.RT", C::Statistical, &[N, N], V::Number, "the chi-square right tail"), &["CHIDIST"]),
    alias(s(508, "CHISQ.INV", C::Statistical, &[N, N], V::Number, "the chi-square left-tail quantile"), &["CHISQINV"]),
    alias(s(509, "CHISQ.INV.RT", C::Statistical, &[N, N], V::Number, "the chi-square right-tail quantile"), &["CHIINV"]),
    alias(s(510, "EXPON.DIST", C::Statistical, &[N, N, B], V::Number, "the exponential distribution of rate λ: density, or cumulative"), &["EXPONDIST"]),
    s(511, "F.DIST", C::Statistical, &[N, N, N, B], V::Number, "the F distribution: density, or left tail"),
    alias(s(512, "F.DIST.RT", C::Statistical, &[N, N, N], V::Number, "the F right tail"), &["FDIST"]),
    s(513, "F.INV", C::Statistical, &[N, N, N], V::Number, "the F left-tail quantile"),
    alias(s(514, "F.INV.RT", C::Statistical, &[N, N, N], V::Number, "the F right-tail quantile"), &["FINV"]),
    alias(s(515, "GAMMA.DIST", C::Statistical, &[N, N, N, B], V::Number, "the gamma distribution of shape α and scale β: density, or cumulative"), &["GAMMADIST"]),
    alias(s(516, "GAMMA.INV", C::Statistical, &[N, N, N], V::Number, "the gamma quantile"), &["GAMMAINV"]),
    s(517, "GAUSS", C::Statistical, &[N], V::Number, "Φ(z) − 0.5: the standard normal probability between 0 and z"),
    s(518, "HYPGEOM.DIST", C::Statistical, &[N, N, N, N, B], V::Number, "the hypergeometric probability of k successes in a sample of n from N holding K: P(X = k), or P(X ≤ k)"),
    s(519, "HYPGEOMDIST", C::Statistical, &[N, N, N, N], V::Number, "the hypergeometric probability P(X = k)"),
    s(520, "LOGNORM.DIST", C::Statistical, &[N, N, N, B], V::Number, "the log-normal distribution: density, or cumulative"),
    s(521, "LOGNORMDIST", C::Statistical, &[N, N, N], V::Number, "the cumulative log-normal distribution"),
    alias(s(522, "LOGNORM.INV", C::Statistical, &[N, N, N], V::Number, "the log-normal quantile"), &["LOGINV"]),
    s(523, "NEGBINOM.DIST", C::Statistical, &[N, N, N, B], V::Number, "the negative binomial probability of f failures before the s-th success: P(F = f), or P(F ≤ f)"),
    s(524, "NEGBINOMDIST", C::Statistical, &[N, N, N], V::Number, "the negative binomial probability P(F = f)"),
    alias(s(525, "NORM.DIST", C::Statistical, &[N, N, N, B], V::Number, "the normal distribution: density, or cumulative"), &["NORMDIST"]),
    alias(s(526, "NORM.INV", C::Statistical, &[N, N, N], V::Number, "the normal quantile"), &["NORMINV"]),
    s(527, "NORM.S.DIST", C::Statistical, &[N, B], V::Number, "the standard normal distribution: density, or cumulative"),
    s(528, "NORMSDIST", C::Statistical, &[N], V::Number, "the cumulative standard normal distribution Φ(z)"),
    alias(s(529, "NORM.S.INV", C::Statistical, &[N], V::Number, "the standard normal quantile"), &["NORMSINV"]),
    s(530, "PHI", C::Statistical, &[N], V::Number, "the standard normal density"),
    alias(s(531, "POISSON.DIST", C::Statistical, &[N, N, B], V::Number, "the Poisson probability of k events at mean λ: P(X = k), or P(X ≤ k)"), &["POISSON"]),
    s(532, "T.DIST", C::Statistical, &[N, N, B], V::Number, "Student's t distribution: density, or left tail"),
    s(533, "T.DIST.RT", C::Statistical, &[N, N], V::Number, "Student's t right tail"),
    s(534, "T.DIST.2T", C::Statistical, &[N, N], V::Number, "Student's t two-tailed probability"),
    s(535, "TDIST", C::Statistical, &[N, N, N], V::Number, "Student's t right tail (tails = 1) or two-tailed probability (tails = 2), for x ≥ 0"),
    s(536, "T.INV", C::Statistical, &[N, N], V::Number, "Student's t left-tail quantile"),
    alias(s(537, "T.INV.2T", C::Statistical, &[N, N], V::Number, "Student's t two-tailed quantile"), &["TINV"]),
    alias(s(538, "WEIBULL.DIST", C::Statistical, &[N, N, N, B], V::Number, "the Weibull distribution of shape α and scale β: density, or cumulative"), &["WEIBULL"]),
    s(539, "FISHER", C::Statistical, &[N], V::Number, "the Fisher transformation atanh(x)"),
    s(540, "FISHERINV", C::Statistical, &[N], V::Number, "the inverse Fisher transformation tanh(y)"),
    alias(s(541, "CONFIDENCE.NORM", C::Statistical, &[N, N, N], V::Number, "the half-width of a normal confidence interval at significance alpha, for sd and sample size n"), &["CONFIDENCE"]),
    s(542, "CONFIDENCE.T", C::Statistical, &[N, N, N], V::Number, "the half-width of a Student's t confidence interval at significance alpha, for sd and sample size n"),
    s(543, "STANDARDIZE", C::Statistical, &[N, N, N], V::Number, "the z-score (x − mean) / sd"),
];
