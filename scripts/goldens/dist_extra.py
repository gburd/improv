#!/usr/bin/env python3
"""Golden values for the P7 distributions (ids 1550-1749), from Gnumeric.

Gnumeric 1.12.57's `ssconvert --recalc` is the reference for all of these
(Gnumeric-only functions). It prints 17 significant digits, so the doubles
round-trip exactly. A few rows are cross-checked against python3-mpmath
(`checks`, to 1e-12 relative unless `CHECK_TOL` says otherwise); the script
fails if Gnumeric and mpmath disagree. On the rows in `MPMATH_WINS`
Gnumeric is known to lose digits, so the mpmath value is the golden (source
`mpmath`) and Gnumeric need only agree to the tolerance given there.

Usage: scripts/goldens/dist_extra.py > crates/engine/tests/data/dist_extra_goldens.tsv
(needs `ssconvert` on PATH, or GNUMERIC_SSCONVERT naming it).

Formulas write numbers in decimal (Improv's parser has no E notation).
A row is `formula` (exact to 1e-12 relative, the runner's default) or
`(formula, tol)`: within `tol` of the Gnumeric value, relative to
max(|x|, 1), where the value comes from an iteration or a quadrature whose
stopping rule differs from Improv's. The quantiles that invert an
incomplete gamma or beta get 1e-13: both products bisect to the last bit,
but the incomplete functions they invert differ by a few ulps.
"""

import json
import os
import subprocess
import sys
import tempfile
from xml.sax.saxutils import escape

# Where Gnumeric loses digits (formula -> how far it is allowed to be off,
# relative to max(|x|, 1e-300); a negative bound is absolute):
MPMATH_WINS = {
    # pnorm2 clamps Φ(x2) − Φ(x1) between dx·φ(x2) and dx·φ(x1) when the
    # tails nearly cancel, keeping ~8 digits.
    "SNORM.DIST.RANGE(1,1.00000001)": 1e-8,
    # Its qnorm at log_p = −1000 is 6e-14 off, which exp() grows 44×.
    "R.QLNORM(-1000,0,1,TRUE,TRUE)": 1e-11,
    # dsnorm's non-log branch passes location/shape where it means
    # shape·location (2φ(z)Φ(αx; ξ/α, ω), not Φ(α(x − ξ)/ω)): wrong unless
    # the location is 0. Its give_log branch is right.
    "R.DSNORM(1,2,0.5,1)": None,
    "R.DSNORM(-1,-3,0.5,2)": None,
    # Its quadrature stops early at large df.
    "R.PTUKEY(2,3,100)": 1e-9,
    "R.PTUKEY(3,6,3000)": 1e-7,
}
# Rows where Gnumeric is wrong and the right answer needs no computing
# (formula -> golden, source `R semantics`):
OVERRIDES = {
    # pst drops its log_p for n ≤ 100 (`gnm_log (pst (…))` is a statement
    # whose value is discarded), so it returns the probability itself.
    "R.PST(1,3,2,FALSE,TRUE)": "LN(R.PST(1,3,2,FALSE))",
    # psnorm does not check the scale (R and dsnorm reject σ ≤ 0).
    "R.PSNORM(1,2,0.5,-1)": {"error": "#NUM!"},
    # Where the CDF is undefined (a fractional n; nmeans < 2) Gnumeric's
    # inverter returns its first guess (−1 and 1); R gives NaN.
    "R.QST(0.3,3.5,2)": {"error": "#NUM!"},
    "R.QTUKEY(0.95,1,10)": {"error": "#NUM!"},
}
# CUM_BIV_NORM_DIST: Gnumeric uses Drezner's 5-point Gauss rule (1978),
# good to ~3e-7 absolute; Improv's Owen's-T form is exact, so its goldens are
# mpmath's.
BIVARIATE = [(0.5, 0.3, 0.4), (-0.5, -0.3, -0.4), (-0.5, 0.3, 0.4), (0.5, -0.3, 0.4),
             (0.5, 0.3, -0.4), (0, 0, 0.5), (1, -1, 0), (-1, 0.5, -0.3), (0, 0.7, 0.3),
             (-3, -2, 0.5), (2, 1, -0.9)]
MPMATH_WINS.update({f"CUM_BIV_NORM_DIST({h},{k},{r})": -1e-6 for h, k, r in BIVARIATE})
# A Gnumeric bug: where a·b·ρ > 0 with a, b of opposite signs its recursion
# computes δ = (1 − sgn a · sgn b)/4 in integer arithmetic (0, not ½), so
# the value is exactly ½ too high (0.582 > Φ(−1)).
MPMATH_WINS["CUM_BIV_NORM_DIST(-1,0.5,-0.3)"] = None
# LANDAU is Kölbig and Schorr's approximation (as Gnumeric has it), about
# 9 digits off the true density.
CHECK_TOL = {f"LANDAU({x})": 1e-8 for x in (0.34, -3, -1, 2, 7, 20)}

ROWS = [
    # --- Gnumeric's extra distributions ---
    "BERNOULLI(0,0.3)", "BERNOULLI(1,0.3)", "BERNOULLI(1,0)",
    "BERNOULLI(0.5,0.3)", "BERNOULLI(1,1.5)",
    "CAUCHY(0.43,1,TRUE)", "CAUCHY(0.43,1,FALSE)", "CAUCHY(-2,0.5,TRUE)",
    "CAUCHY(3,2,FALSE)", "CAUCHY(1,0,FALSE)", "CAUCHY(1,-1,TRUE)",
    "EXPPOWDIST(0.5,1,2)", "EXPPOWDIST(-1.2,2,0.5)", "EXPPOWDIST(1,-1,2)",
    "EXPPOWDIST(0,1,1)", "EXPPOWDIST(1,1,0)",
    "GEOMDIST(2,0.3,FALSE)", "GEOMDIST(2,0.3,TRUE)", "GEOMDIST(2.7,0.3,FALSE)",
    "GEOMDIST(0,1,TRUE)", "GEOMDIST(-1,0.3,TRUE)", "GEOMDIST(1,0,FALSE)",
    "GEOMDIST(1,1.2,TRUE)",
    "LAPLACE(0.5,1)", "LAPLACE(-2,0.5)", "LAPLACE(0,3)", "LAPLACE(1,0)",
    "LOGISTIC(0.5,1)", "LOGISTIC(-2,0.5)", "LOGISTIC(40,1)", "LOGISTIC(1,0)",
    "PARETO(0.6,1,2)", "PARETO(3,1,2)", "PARETO(2,2.5,2)", "PARETO(5,0.5,1)",
    "PARETO(1,0,1)", "PARETO(1,1,-1)",
    "RAYLEIGH(0.4,1)", "RAYLEIGH(2,0.5)", "RAYLEIGH(-1,1)", "RAYLEIGH(3,2)",
    "RAYLEIGH(1,0)",
    "RAYLEIGHTAIL(0.6,0.3,1)", "RAYLEIGHTAIL(2,1,0.5)", "RAYLEIGHTAIL(0.1,0.3,1)",
    "RAYLEIGHTAIL(5,4,2)", "RAYLEIGHTAIL(1,0,0)",
    "SNORM.DIST.RANGE(-1,1)", "SNORM.DIST.RANGE(1,2)", "SNORM.DIST.RANGE(2,1)",
    "SNORM.DIST.RANGE(1,1.00000001)", "SNORM.DIST.RANGE(5,6)",
    "SNORM.DIST.RANGE(-3,-2)", "SNORM.DIST.RANGE(0,1)",
    # --- R-style kernels: closed forms and the normal family ---
    "R.DNORM(1,0,1)", "R.DNORM(1.5,2,3,TRUE)", "R.DNORM(50,0,1,TRUE)", "R.DNORM(1,0,0)",
    "R.DNORM(1,0,-1)", "R.DNORM(0,0,0)",
    "R.PNORM(1,0,1)", "R.PNORM(1,0,1,FALSE)", "R.PNORM(-50,0,1,TRUE,TRUE)",
    "R.PNORM(40,0,1,FALSE,TRUE)", "R.PNORM(1.5,2,3,FALSE,TRUE)", "R.PNORM(1,0,0)",
    "R.PNORM(1,0,-1)", "R.PNORM(-10,0,1)",
    "R.QNORM(0.3,0,1)", "R.QNORM(0.3,1,2,FALSE)", "R.QNORM(-1000,0,1,TRUE,TRUE)",
    "R.QNORM(-0.00000000000000000001,0,1,FALSE,TRUE)", "R.QNORM(0.999,0,1)", "R.QNORM(0,0,1)",
    "R.QNORM(1.5,0,1)", "R.QNORM(0.3,0,0)", "R.QNORM(0.5,1,-1)",
    "R.DLNORM(2,0.5,1)", "R.DLNORM(2,0.5,1,TRUE)", "R.DLNORM(0,0,1)", "R.DLNORM(1,0,0)",
    "R.PLNORM(2,0.5,1)", "R.PLNORM(2,0.5,1,FALSE,TRUE)", "R.PLNORM(-1,0,1)", "R.PLNORM(1,0,-1)",
    "R.QLNORM(0.3,0.5,1)", "R.QLNORM(0.3,0.5,1,FALSE)", "R.QLNORM(-1000,0,1,TRUE,TRUE)",
    "R.QLNORM(0.5,0,0)", "R.QLNORM(1.5,0,1)",
    "R.DEXP(1.5,4)", "R.DEXP(1.5,4,TRUE)", "R.DEXP(-1,2)", "R.DEXP(1,0)",
    "R.PEXP(1.5,4)", "R.PEXP(1.5,4,FALSE)", "R.PEXP(1000,1,FALSE,TRUE)",
    "R.PEXP(0.0000000001,1,TRUE,TRUE)", "R.PEXP(-1,2)", "R.PEXP(1,-2)", "R.PEXP(1,0)",
    "R.QEXP(0.3,4)", "R.QEXP(0.3,4,FALSE)", "R.QEXP(-1000,1,FALSE,TRUE)", "R.QEXP(0,2)",
    "R.QEXP(1,2)", "R.QEXP(0.5,-1)",
    "R.DWEIBULL(1.5,2,3)", "R.DWEIBULL(1.5,2,3,TRUE)", "R.DWEIBULL(0,0.5,1)",
    "R.DWEIBULL(-1,2,3)", "R.DWEIBULL(1,0,1)",
    "R.PWEIBULL(1.5,2,3)", "R.PWEIBULL(1.5,2,3,FALSE)", "R.PWEIBULL(1000,1,1,FALSE,TRUE)",
    "R.PWEIBULL(0.01,2,3,TRUE,TRUE)", "R.PWEIBULL(-1,2,3)", "R.PWEIBULL(1,2,0)",
    "R.QWEIBULL(0.3,2,3)", "R.QWEIBULL(0.3,2,3,FALSE)", "R.QWEIBULL(-2,2,3,TRUE,TRUE)",
    "R.QWEIBULL(0,2,3)", "R.QWEIBULL(1,2,3)",
    "R.DRAYLEIGH(1.5,2)", "R.DRAYLEIGH(1.5,2,TRUE)", "R.DRAYLEIGH(-1,2)", "R.DRAYLEIGH(1,0)",
    "R.PRAYLEIGH(1.5,2)", "R.PRAYLEIGH(1.5,2,FALSE)", "R.PRAYLEIGH(50,1,FALSE,TRUE)",
    "R.PRAYLEIGH(0.001,1,TRUE,TRUE)", "R.PRAYLEIGH(1,-1)",
    "R.QRAYLEIGH(0.5,2)", "R.QRAYLEIGH(0.3,2,FALSE)", "R.QRAYLEIGH(-3,2,TRUE,TRUE)",
    "R.QRAYLEIGH(1.5,2)",
    "R.DGUMBEL(1,0.5,2)", "R.DGUMBEL(1,0.5,2,TRUE)", "R.DGUMBEL(-3,0,1)", "R.DGUMBEL(1,0,0)",
    "R.PGUMBEL(1,0.5,2)", "R.PGUMBEL(1,0.5,2,FALSE)", "R.PGUMBEL(-3,0,1,TRUE,TRUE)",
    "R.PGUMBEL(40,0,1,FALSE,TRUE)", "R.PGUMBEL(1,0,-1)",
    "R.QGUMBEL(0.3,0.5,2)", "R.QGUMBEL(0.3,0.5,2,FALSE)", "R.QGUMBEL(-5,0,1,TRUE,TRUE)",
    "R.QGUMBEL(-0.0000000001,0,1,FALSE,TRUE)", "R.QGUMBEL(1,0,1)",
    "R.DCAUCHY(1,0.5,2)", "R.DCAUCHY(1,0.5,2,TRUE)", "R.DCAUCHY(1,0,0)", "R.DCAUCHY(10000000000,0,1)",
    "R.PCAUCHY(1,0.5,2)", "R.PCAUCHY(1,0.5,2,FALSE)", "R.PCAUCHY(-10000000000,0,1,TRUE,TRUE)",
    "R.PCAUCHY(3,0,1,FALSE,TRUE)", "R.PCAUCHY(-3,0,1)", "R.PCAUCHY(1,0,0)",
    "R.QCAUCHY(0.3,0.5,2)", "R.QCAUCHY(0.3,0.5,2,FALSE)", "R.QCAUCHY(0.9,0,1)",
    "R.QCAUCHY(-0.1,0,1,TRUE,TRUE)", "R.QCAUCHY(-3,0,1,FALSE,TRUE)", "R.QCAUCHY(0.3,1,0)",
    "R.QCAUCHY(0.3,1,-1)", "R.QCAUCHY(0.5,1,2)",
    # --- R-style kernels: gamma, chi-square, beta, F, t ---
    "R.DGAMMA(2,3,0.5)", "R.DGAMMA(2,3,0.5,TRUE)", "R.DGAMMA(0,1,2)", "R.DGAMMA(0,0.5,1)",
    "R.DGAMMA(0,2,1)", "R.DGAMMA(-1,2,1)", "R.DGAMMA(1,0,1)", "R.DGAMMA(1,2,0)",
    "R.PGAMMA(2,3,0.5)", "R.PGAMMA(2,3,0.5,FALSE)", "R.PGAMMA(0.001,3,1,TRUE,TRUE)",
    "R.PGAMMA(30,3,1,FALSE,TRUE)", "R.PGAMMA(-1,2,1)", "R.PGAMMA(1,0,1)", "R.PGAMMA(1,2,-1)",
    ("R.QGAMMA(0.3,3,0.5)", 1e-13), ("R.QGAMMA(0.95,3,0.5)", 1e-13),
    ("R.QGAMMA(0.3,3,0.5,FALSE)", 1e-13), ("R.QGAMMA(-5,0.5,1,TRUE,TRUE)", 1e-12),
    "R.QGAMMA(0,3,1)", "R.QGAMMA(1,3,1)", "R.QGAMMA(0.5,0,1)",
    "R.DCHISQ(3,4)", "R.DCHISQ(3,4,TRUE)", "R.DCHISQ(0,2)", "R.DCHISQ(0,1)", "R.DCHISQ(1,0)",
    "R.PCHISQ(3,4)", "R.PCHISQ(3,4,FALSE)", "R.PCHISQ(3,4,FALSE,TRUE)", "R.PCHISQ(1,0)",
    "R.PCHISQ(-1,3)",
    ("R.QCHISQ(0.3,4)", 1e-13), ("R.QCHISQ(0.3,4,FALSE)", 1e-13),
    ("R.QCHISQ(-0.01,4,TRUE,TRUE)", 1e-13), "R.QCHISQ(0.5,0)", "R.QCHISQ(1.5,4)",
    "R.DBETA(0.5,2,3)", "R.DBETA(0.5,2,3,TRUE)", "R.DBETA(0,1,3)", "R.DBETA(1,2,0.5)",
    "R.DBETA(1.2,2,3)", "R.DBETA(0.5,-1,3)",
    "R.PBETA(0.3,2,3)", "R.PBETA(0.3,2,3,FALSE)", "R.PBETA(0.999,2,3,TRUE,TRUE)",
    "R.PBETA(0.3,2,3,FALSE,TRUE)", "R.PBETA(1.5,2,3)",
    ("R.QBETA(0.3,2,3)", 1e-13), ("R.QBETA(0.95,2,3)", 1e-13), ("R.QBETA(0.3,2,3,FALSE)", 1e-13),
    ("R.QBETA(-1,2,3,TRUE,TRUE)", 1e-13), ("R.QBETA(0.3,0.5,0.5)", 1e-13), "R.QBETA(1,2,3)",
    "R.QBETA(1.5,2,3)",
    "R.DF(1.5,3,7)", "R.DF(1.5,3,7,TRUE)", "R.DF(0,2,3)", "R.DF(0,1,3)", "R.DF(1,0,3)",
    "R.PF(1.5,3,7)", "R.PF(1.5,3,7,FALSE)", "R.PF(1.5,3,7,FALSE,TRUE)", "R.PF(0,3,7)",
    "R.PF(1,0.5,3)",
    ("R.QF(0.5,3,7)", 1e-13), ("R.QF(0.95,3,7)", 1e-13), ("R.QF(0.3,3,7,FALSE)", 1e-13),
    ("R.QF(-2,3,7,FALSE,TRUE)", 1e-13), "R.QF(0.5,0,3)",
    "R.DT(1,3)", "R.DT(1,3,TRUE)", "R.DT(1,0.5)", "R.DT(1,0)",
    "R.PT(1,3)", "R.PT(-2,3)", "R.PT(1,3,FALSE)", "R.PT(1,0.5,FALSE,TRUE)", "R.PT(-1,3,TRUE,TRUE)",
    ("R.QT(0.3,3)", 1e-13), ("R.QT(0.975,3)", 1e-13), ("R.QT(0.3,0.5)", 1e-13),
    ("R.QT(0.1,3,FALSE)", 1e-13), ("R.QT(-0.05,3,TRUE,TRUE)", 1e-13), "R.QT(0.5,3)",
    "R.QT(0.3,0)",
    # --- R-style kernels: the discrete families ---
    "R.DBINOM(3,10,0.3)", "R.DBINOM(3,10,0.3,TRUE)", "R.DBINOM(3.5,10,0.3)", "R.DBINOM(-1,10,0.3)",
    "R.DBINOM(11,10,0.3)", "R.DBINOM(3,10.5,0.3)", "R.DBINOM(3,10,1.3)",
    "R.PBINOM(3,10,0.3)", "R.PBINOM(3,10,0.3,FALSE)", "R.PBINOM(3,10,0.3,FALSE,TRUE)",
    "R.PBINOM(3.7,10,0.3)", "R.PBINOM(-1,10,0.3)", "R.PBINOM(10,10,0.3)", "R.PBINOM(2.5,5,0.5)",
    "R.PBINOM(3,10.5,0.3)",
    "R.QBINOM(0.3,10,0.3)", "R.QBINOM(0.3,10,0.3,FALSE)", "R.QBINOM(-0.1,10,0.3,TRUE,TRUE)",
    "R.QBINOM(0.5,1,0.5)", "R.QBINOM(0.3,10,0)", "R.QBINOM(0.3,10,1)", "R.QBINOM(1,10,0.3)",
    "R.QBINOM(0,10,0.3)", "R.QBINOM(0.3,10.5,0.3)",
    "R.DGEOM(2,0.3)", "R.DGEOM(2,0.3,TRUE)", "R.DGEOM(2.5,0.3)", "R.DGEOM(-1,0.3)", "R.DGEOM(0,1)",
    "R.DGEOM(1,0)",
    "R.PGEOM(2,0.3)", "R.PGEOM(2,0.3,FALSE)", "R.PGEOM(2.5,0.3,FALSE,TRUE)", "R.PGEOM(-1,0.3)",
    "R.PGEOM(3,1)",
    "R.QGEOM(0.3,0.2)", "R.QGEOM(0.3,0.2,FALSE)", "R.QGEOM(-0.1,0.2,TRUE,TRUE)", "R.QGEOM(1,0.2)",
    "R.QGEOM(0.5,1)", "R.QGEOM(0.5,0)",
    "R.DHYPER(1,3,2,3)", "R.DHYPER(1,3,2,3,TRUE)", "R.DHYPER(1.5,3,2,3)", "R.DHYPER(4,3,2,3)",
    "R.DHYPER(1,3,2,6)", "R.DHYPER(1,-3,2,3)", "R.DHYPER(1,3.5,2,3)",
    "R.PHYPER(1,3,2,3)", "R.PHYPER(1,3,2,3,FALSE)", "R.PHYPER(2,30,20,10,FALSE,TRUE)",
    "R.PHYPER(1.5,3,2,3)", "R.PHYPER(1,3.5,2,3)", "R.PHYPER(0,3,2,3)", "R.PHYPER(5,3,2,3)",
    "R.QHYPER(0.3,30,20,10)", "R.QHYPER(0.3,30,20,10,FALSE)", "R.QHYPER(-0.5,30,20,10,TRUE,TRUE)",
    "R.QHYPER(0,3,2,3)", "R.QHYPER(1,3,2,3)", "R.QHYPER(0.5,3,2,6)",
    "R.DNBINOM(2,3,0.4)", "R.DNBINOM(2,3,0.4,TRUE)", "R.DNBINOM(2,2.5,0.5)", "R.DNBINOM(0,0,0.5)",
    "R.DNBINOM(2.5,3,0.4)", "R.DNBINOM(2,3,0)",
    "R.PNBINOM(2,3,0.4)", "R.PNBINOM(2,3,0.4,FALSE)", "R.PNBINOM(20,3,0.4,FALSE,TRUE)",
    "R.PNBINOM(2,0,0.5)", "R.PNBINOM(-1,3,0.4)",
    "R.QNBINOM(0.3,3,0.4)", "R.QNBINOM(0.3,3,0.4,FALSE)", "R.QNBINOM(-2,3,0.4,TRUE,TRUE)",
    "R.QNBINOM(1,3,0.4)", "R.QNBINOM(1,3,1)", "R.QNBINOM(0.5,0,0.5)", "R.QNBINOM(0.5,-1,0.4)",
    "R.DPOIS(3,2.5)", "R.DPOIS(3,2.5,TRUE)", "R.DPOIS(2.5,3)", "R.DPOIS(-1,2.5)", "R.DPOIS(2,0)",
    "R.DPOIS(3,-1)",
    "R.PPOIS(3,2.5)", "R.PPOIS(3,2.5,FALSE)", "R.PPOIS(30,2.5,FALSE,TRUE)", "R.PPOIS(-1,2.5)",
    "R.PPOIS(2,0)",
    "R.QPOIS(0.3,2.5)", "R.QPOIS(0.3,2.5,FALSE)", "R.QPOIS(-0.01,2.5,FALSE,TRUE)",
    "R.QPOIS(0,2.5)", "R.QPOIS(0.5,0)", "R.QPOIS(1,3)", "R.QPOIS(0.3,-1)",
    "R.QPOIS(0.5,0.6931471805599453)",
    # --- the special cases: Landau, Owen's T, bivariate normal ---
    "LANDAU(0.34)", "LANDAU(-6)", "LANDAU(-3)", "LANDAU(-1)", "LANDAU(2)", "LANDAU(7)",
    "LANDAU(20)", "LANDAU(100)", "LANDAU(400)",
    "OWENT(0.5,0.7)", "OWENT(0.5,-0.7)", "OWENT(-0.5,2)", "OWENT(0,0.5)", "OWENT(1,1)",
    "OWENT(1,0)", "OWENT(3,0.2)", "OWENT(0.01,0.5)", "OWENT(5,0.95)", "OWENT(2,0.999999)",
    "OWENT(0.3,5)", "OWENT(2,5)", "OWENT(8,0.5)",
] + [f"CUM_BIV_NORM_DIST({h},{k},{r})" for h, k, r in BIVARIATE] + [
    "CUM_BIV_NORM_DIST(0.5,0.3,1)", "CUM_BIV_NORM_DIST(0.5,0.3,-1.2)",
    # --- R-style kernels: skew-normal, skew-t (through Owen's T) ---
    "R.DSNORM(1,2,0,1)", "R.DSNORM(1,2,0,1,TRUE)", "R.DSNORM(1,0,0.5,2)", "R.DSNORM(1,2,0.5,1)",
    "R.DSNORM(-1,-3,0.5,2)", "R.DSNORM(1,2,0.5,-1)",
    "R.PSNORM(1,2,0.5,2)", "R.PSNORM(1,2,0.5,2,FALSE,TRUE)", "R.PSNORM(1,2,0.5,2,TRUE,TRUE)",
    "R.PSNORM(0.5,-5,0,1)", "R.PSNORM(0.5,20,0,1)", "R.PSNORM(-0.5,20,0,1)",
    "R.PSNORM(-1,-3,0.5,2)", "R.PSNORM(-2,15,0,1)", "R.PSNORM(1,0,0.5,2)",
    "R.PSNORM(1,2,0.5,-1)",
    ("R.QSNORM(0.3,2,0.5,2)", 1e-12), ("R.QSNORM(0.95,2,0.5,2)", 1e-12),
    ("R.QSNORM(0.3,2,0.5,2,FALSE)", 1e-12), ("R.QSNORM(-1,2,0.5,2,TRUE,TRUE)", 1e-12),
    ("R.QSNORM(0.001,-2,0,1)", 1e-12), "R.QSNORM(0.3,0,0.5,2)", "R.QSNORM(1.5,2,0,1)",
    "R.DST(1,3,2)", "R.DST(1,3,2,TRUE)", "R.DST(1,3.5,2)", "R.DST(-1,5,2)", "R.DST(1,3,0)",
    "R.DST(1,0,2)",
    "R.PST(1,3,2)", "R.PST(1,3,2,FALSE)", "R.PST(1,3,2,FALSE,TRUE)", "R.PST(0.5,1,-1)",
    "R.PST(0.5,2,-1)", "R.PST(2,7,0.5)", "R.PST(-2,4,3)", "R.PST(1,30,2)", "R.PST(-3,9,-2)",
    "R.PST(1,3,0)", "R.PST(1,3.5,2)",
    ("R.QST(0.3,3,2)", 1e-12), ("R.QST(0.3,3,2,FALSE)", 1e-12), ("R.QST(0.95,7,-1)", 1e-12),
    ("R.QST(0.999,3,2)", 1e-12), "R.QST(0.3,3,0)", "R.QST(0.3,3.5,2)",
    # --- the studentized range: Gnumeric's quadrature stops at a looser
    # point than Improv's (R.PTUKEY(3,6,3000) is 3e-8 off mpmath there), so
    # these are compared to 1e-9 (1e-7 at df > 1000, against mpmath). ---
    ("R.PTUKEY(3.5,4,10)", 1e-9), ("R.PTUKEY(3.5,4,10,2)", 1e-9),
    ("R.PTUKEY(3.5,4,10,1,FALSE,TRUE)", 1e-9), ("R.PTUKEY(1,2,2)", 1e-9),
    ("R.PTUKEY(6,10,5)", 1e-9), ("R.PTUKEY(10,4,3)", 1e-9), ("R.PTUKEY(0.1,4,10)", 1e-9),
    ("R.PTUKEY(2,3,100)", 1e-9), ("R.PTUKEY(3,6,3000)", 1e-7),
    ("R.PTUKEY(1000,4,2,1,FALSE)", 1e-9), "R.PTUKEY(0,4,10)", "R.PTUKEY(-1,4,1)",
    "R.PTUKEY(3.5,1,10)", "R.PTUKEY(3.5,4,1.5)", "R.PTUKEY(3.5,4,10,0.5)",
    ("R.QTUKEY(0.95,4,10)", 1e-9), ("R.QTUKEY(0.95,4,10,2)", 1e-9),
    ("R.QTUKEY(0.95,4,10,1,FALSE)", 1e-9), ("R.QTUKEY(0.99,10,5)", 1e-9),
    ("R.QTUKEY(-0.05,4,10,1,TRUE,TRUE)", 1e-9), "R.QTUKEY(0,4,10)", "R.QTUKEY(1.5,4,10)",
    "R.QTUKEY(0.95,1,10)",
]



def checks():
    """Independent mpmath values for a few rows (formula -> value)."""
    import mpmath as mp
    mp.mp.dps = 30
    phi = lambda z: mp.ncdf(z)

    def owen(h, a):
        h, a = mp.mpf(h), mp.mpf(a)
        return mp.quad(lambda x: mp.exp(-h * h * (1 + x * x) / 2) / (1 + x * x), [0, a]) / (2 * mp.pi)

    def bivariate(h, k, r):
        s = mp.sqrt(1 - mp.mpf(r) ** 2)
        return mp.quad(lambda x: mp.npdf(x) * phi((k - r * x) / s), [-mp.inf, h])

    def landau(x):
        f = lambda t: mp.exp(-t * mp.log(t) - x * t) * mp.sin(mp.pi * t)
        return mp.quad(f, [0, 1, 5, 20, mp.inf]) / mp.pi

    extra = {f"CUM_BIV_NORM_DIST({h},{k},{r})": bivariate(h, k, r) for h, k, r in BIVARIATE}
    extra.update({f"LANDAU({x})": landau(x) for x in (0.34, -3, -1, 2, 7, 20)})
    def sn_pdf(x, a, loc, scale):
        z = (x - loc) / scale
        return 2 / scale * mp.npdf(z) * phi(a * z)

    sn_tail = lambda x: mp.exp(-2 * (1 + x * x)) / (1 + x * x)

    def ptukey(q, k, df):
        def w(r):
            first = mp.erf(r / 2 / mp.sqrt(2)) ** k
            f = lambda v: mp.npdf(v) * (phi(v) - phi(v - r)) ** (k - 1)
            return first + 2 * k * mp.quad(f, [r / 2, r / 2 + 3, r / 2 + 10, mp.inf])
        h = mp.mpf(df) / 2
        lc = mp.log(2) + h * mp.log(h) - mp.loggamma(h)
        f = lambda s: mp.exp(lc + (df - 1) * mp.log(s) - h * s * s) * w(q * s)
        sd = 1 / mp.sqrt(2 * df)
        return mp.quad(f, [max(0, 1 - 14 * sd), 1 - 4 * sd, 1 - sd, 1, 1 + sd, 1 + 4 * sd, 1 + 14 * sd])

    mp.mp.dps = 20
    extra.update({
        "R.DSNORM(1,2,0.5,1)": sn_pdf(1, 2, 0.5, 1),
        "R.DSNORM(-1,-3,0.5,2)": sn_pdf(-1, -3, 0.5, 2),
        # P(Z ≤ h) = (1/π) ∫_α^∞ e^(−h²(1+x²)/2)/(1+x²) dx for h < 0 < α; the
        # integrand falls ~e^(−60 s) from x = 15, so the nodes crowd there.
        "R.PSNORM(-2,15,0,1)": (mp.quad(sn_tail, mp.linspace(15, 15.5, 201))
                                + mp.quad(sn_tail, mp.linspace(15.5, 17, 61))
                                + mp.quad(sn_tail, [17, 20, mp.inf])) / mp.pi,
        "R.PTUKEY(2,3,100)": ptukey(2, 3, 100),
        "R.PTUKEY(3,6,3000)": ptukey(3, 6, 3000),
    })
    mp.mp.dps = 30
    extra.update({f"OWENT({h},{a})": owen(h, a) for h, a in
                  ((0.5, 0.7), (-0.5, 2), (3, 0.2), (5, 0.95), (2, 0.999999), (0.3, 5), (8, 0.5))})
    q1000 = mp.findroot(lambda z: mp.log(phi(z)) + 1000, mp.mpf(-44.6), tol=1e-40)
    return {
        "CAUCHY(0.43,1,TRUE)": 0.5 - mp.atan(0.43) / mp.pi,
        "EXPPOWDIST(-1.2,2,0.5)": mp.exp(-mp.sqrt(0.6)) / (4 * mp.gamma(3)),
        "LOGISTIC(-2,0.5)": mp.exp(-4) / (0.5 * (1 + mp.exp(-4)) ** 2),
        "RAYLEIGHTAIL(2,1,0.5)": 8 * mp.exp((4 - 16) / 2),
        "SNORM.DIST.RANGE(1,1.00000001)": phi(mp.mpf(1.00000001)) - phi(1),
        "SNORM.DIST.RANGE(5,6)": phi(6) - phi(5),
        "R.PNORM(-50,0,1,TRUE,TRUE)": mp.log(phi(-50)),
        "R.QNORM(-1000,0,1,TRUE,TRUE)": q1000,
        "R.QLNORM(-1000,0,1,TRUE,TRUE)": mp.exp(q1000),
        "R.PGUMBEL(40,0,1,FALSE,TRUE)": mp.log(-mp.expm1(-mp.exp(-40))),
        "R.QCAUCHY(-0.1,0,1,TRUE,TRUE)": mp.tan(mp.pi * (mp.exp(-0.1) - 0.5)),
        "R.PRAYLEIGH(0.001,1,TRUE,TRUE)": mp.log(-mp.expm1(-mp.mpf(0.001) ** 2 / 2)),
        "R.PGAMMA(30,3,1,FALSE,TRUE)": mp.log(mp.gammainc(3, 30, regularized=True)),
        "R.PBETA(0.999,2,3,TRUE,TRUE)": mp.log(mp.betainc(2, 3, 0, 0.999, regularized=True)),
        "R.PPOIS(30,2.5,FALSE,TRUE)": mp.log(mp.gammainc(31, 0, 2.5, regularized=True)),
        "R.PHYPER(2,30,20,10,FALSE,TRUE)": mp.log(1 - sum(
            mp.binomial(30, k) * mp.binomial(20, 10 - k) for k in range(3)) / mp.binomial(50, 10)),
        "R.PT(-1,3,TRUE,TRUE)": mp.log(mp.betainc(1.5, 0.5, 0, mp.mpf(3) / 4, regularized=True) / 2),
        **extra,
    }


def gnumeric(formulas):
    exe = os.environ.get("GNUMERIC_SSCONVERT", "ssconvert")
    with tempfile.TemporaryDirectory(dir=os.path.expanduser("~")) as d:
        src = os.path.join(d, "in.gnumeric")
        cells = "".join(f'<gnm:Cell Row="{i}" Col="0">={escape(f)}</gnm:Cell>'
                        for i, f in enumerate(formulas))
        with open(src, "w", encoding="utf-8") as fh:
            fh.write('<?xml version="1.0"?><gnm:Workbook xmlns:gnm="http://www.gnumeric.org/v10.dtd">'
                     '<gnm:SheetNameIndex><gnm:SheetName>S</gnm:SheetName></gnm:SheetNameIndex>'
                     '<gnm:Sheets><gnm:Sheet><gnm:Name>S</gnm:Name><gnm:MaxCol>1</gnm:MaxCol>'
                     f'<gnm:MaxRow>{len(formulas)}</gnm:MaxRow><gnm:Cells>{cells}</gnm:Cells>'
                     '</gnm:Sheet></gnm:Sheets></gnm:Workbook>')
        out = os.path.join(d, "out.txt")
        subprocess.run([exe, "--recalc", src, out], check=True, capture_output=True)
        with open(out, encoding="utf-8") as fh:
            vals = fh.read().splitlines()
    assert len(vals) == len(formulas), (len(vals), len(formulas))
    return [{"error": v} if v.startswith("#") else float(v) for v in vals]


def main():
    rows = [(r, None) if isinstance(r, str) else r for r in ROWS]
    derived = [v for v in OVERRIDES.values() if isinstance(v, str)]
    vals = gnumeric(["PI()"] + [f for f, _ in rows] + derived)
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    got = dict(zip([f for f, _ in rows] + derived, vals[1:]))
    source = {}
    for f, want in checks().items():
        g, want = got[f], float(want)
        tol = MPMATH_WINS.get(f, CHECK_TOL.get(f, 1e-12))
        if tol is None:
            print(f"Gnumeric bug: {f} = {g!r}, mpmath {want!r}", file=sys.stderr)
        elif abs(g - want) > (-tol if tol < 0 else tol * max(abs(want), 1e-300)):
            raise SystemExit(f"Gnumeric and mpmath disagree: {f} = {g!r} vs {want!r}")
        if f in MPMATH_WINS:
            got[f], source[f] = want, "mpmath"
    for f, v in OVERRIDES.items():
        print(f"Gnumeric bug: {f} = {got[f]!r}", file=sys.stderr)
        got[f], source[f] = (got[v], "Gnumeric 1.12.57 (LN of the probability)") \
            if isinstance(v, str) else (v, "R semantics")
    print("formula\texpected\tsource")
    for f, tol in rows:
        assert "\t" not in f and "^" not in f, f
        v = got[f]
        if tol is not None and not isinstance(v, dict):
            v = {"approx": v, "tol": tol}
        print(f"{f}\t{json.dumps(v)}\t{source.get(f, 'Gnumeric 1.12.57')}")


if __name__ == "__main__":
    sys.exit(main())
