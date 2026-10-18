#!/usr/bin/env python3
"""Golden values for the P7 distributions (ids 1550-1749), from Gnumeric.

Gnumeric 1.12.57's `ssconvert --recalc` is the reference for all of these
(Gnumeric-only functions). It prints 17 significant digits, so the doubles
round-trip exactly. A few rows are cross-checked against python3-mpmath
(`checks`); the script fails if Gnumeric and mpmath disagree, except on the
rows in `MPMATH_WINS`, where Gnumeric is known to lose digits and the
mpmath value is the golden (source `mpmath`).

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
]


# Gnumeric's pnorm2 clamps Φ(x2) − Φ(x1) between dx·φ(x2) and dx·φ(x1) when
# the tails nearly cancel, so it keeps only ~8 digits (2.4197072123754992e-9;
# the exact value is 2.4197072183871969e-9).
# Its qnorm for log_p = −1000 is 6e-14 off, which exp() in qlnorm grows 44×.
MPMATH_WINS = {"SNORM.DIST.RANGE(1,1.00000001)", "R.QLNORM(-1000,0,1,TRUE,TRUE)"}


def checks():
    """Independent mpmath values for a few rows (formula -> value)."""
    import mpmath as mp
    mp.mp.dps = 30
    phi = lambda z: mp.ncdf(z)
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
        "R.PT(-1,3,TRUE,TRUE)": mp.log(mp.betainc(1.5, 0.5, 0, mp.mpf(3) / 4, regularized=True) / 2),
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
    vals = gnumeric(["PI()"] + [f for f, _ in rows])
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    got = dict(zip((f for f, _ in rows), vals[1:]))
    source = {}
    for f, want in checks().items():
        g, want = got[f], float(want)
        if abs(g - want) <= 1e-12 * max(abs(want), 1e-300):
            continue
        if f not in MPMATH_WINS:
            raise SystemExit(f"Gnumeric and mpmath disagree: {f} = {g!r} vs {want!r}")
        print(f"Gnumeric loses digits: {f} = {g!r}, mpmath {want!r}", file=sys.stderr)
        got[f], source[f] = want, "mpmath"
    print("formula\texpected\tsource")
    for f, tol in rows:
        assert "\t" not in f and "^" not in f, f
        v = got[f]
        if tol is not None and not isinstance(v, dict):
            v = {"approx": v, "tol": tol}
        print(f"{f}\t{json.dumps(v)}\t{source.get(f, 'Gnumeric 1.12.57')}")


if __name__ == "__main__":
    sys.exit(main())
