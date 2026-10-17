#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/statistical_goldens.tsv (P4 statistical
scalars: special functions and distributions).

    nix-shell -p python3Packages.mpmath --run \\
        'python3 scripts/goldens/statistical.py > crates/engine/tests/data/statistical_goldens.tsv'

(redirect inside --run: a dev shell may print a banner on stdout.)

LibreOffice rows run through p1.py's runner (the same flat-ODS sheet and the
same exact-double reconstruction), with Excel's function names mapped to
LibreOffice's ODFF names. LibreOffice's Err:502 (invalid argument) and
Err:523 (no convergence) become #NUM!.

A case marked `excel=...` is one where LibreOffice disagrees with Excel's
documentation; the row records Excel's value (source `Excel docs`) and stderr
shows LibreOffice's. Most are domain rules: Excel rejects x outside the
support (EXPON.DIST, LOGNORM.DIST, BETADIST, HYPGEOM.DIST) where
LibreOffice returns 0 or 1, and says #NUM! where LibreOffice says #N/A or
#DIV/0!.

Hand rows (HAND) are the Gnumeric-only special functions (BETA, BETALN,
DIGAMMA, IGAMMA, POCHHAMMER): their domain rules follow Gnumeric's source
(plugins/fn-math/functions.c, src/sf-gamma.c) and their values are computed
with mpmath at 50 digits, then rounded to the nearest double.
"""

import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402  (the LibreOffice runner and helpers)

try:
    import mpmath
except ImportError:
    raise SystemExit("needs mpmath: nix-shell -p python3Packages.mpmath")

mpmath.mp.dps = 50
err = p1.err
NUM, DIV0 = err("#NUM!"), err("#DIV/0!")

LO = []


def lo(f, excel=None):
    LO.append((f, None, excel))


def many(*fs):
    for f in fs:
        lo(f)


# --- special functions (LibreOffice has GAMMA, GAMMALN, ERF, ERFC) ------------
many("GAMMA(5)", "GAMMA(0.5)", "GAMMA(2.5)", "GAMMA(-0.5)", "GAMMA(-1.5)", "GAMMA(10.1)",
     "GAMMA(0)", "GAMMA(-2)", "GAMMA(172)", "GAMMA(170.5)",
     "GAMMALN(4)", "GAMMALN(0.5)", "GAMMALN(100)", "GAMMALN(1.5)", "GAMMALN(0.001)",
     "GAMMALN(0)", "GAMMALN(-1)",
     "GAMMALN.PRECISE(4.5)", "GAMMALN.PRECISE(0.25)",
     "ERF(0.745)", "ERF(1)", "ERF(0,1.5)", "ERF(1,2)", "ERF(-1)", "ERF(3)", "ERF(2,1)",
     "ERF.PRECISE(0.745)", "ERF.PRECISE(1)", "ERF.PRECISE(-0.5)",
     "ERFC(1)", "ERFC(0)", "ERFC(-1)", "ERFC(3)", "ERFC(10)",
     "ERFC.PRECISE(1)", "ERFC.PRECISE(2)")

# --- distributions ------------------------------------------------------------
many("BETA.DIST(2,8,10,TRUE,1,3)", "BETA.DIST(2,8,10,FALSE,1,3)", "BETA.DIST(0.3,2,3,TRUE)",
     "BETA.DIST(0.3,2,3,FALSE)", "BETA.DIST(0.5,0.5,0.5,TRUE)", "BETA.DIST(4,8,10,TRUE,1,3)",
     "BETA.DIST(0.5,0,3,TRUE)", "BETA.DIST(0.9,5,1.5,FALSE)",
     "BETADIST(2,8,10,1,3)", "BETADIST(0.3,2,3)", "BETADIST(0.9,5,1.5)",
     "BETA.INV(0.685470581,8,10,1,3)", "BETA.INV(0.2,2,3)", "BETA.INV(0.9,0.5,0.5)",
     "BETA.INV(0.5,2,3,-1,1)", "BETA.INV(1.5,2,3)", "BETA.INV(1,2,3)", "BETAINV(0.01,5,1.5)",
     "BINOM.DIST(6,10,0.5,FALSE)", "BINOM.DIST(6,10,0.5,TRUE)", "BINOM.DIST(6.9,10,0.5,FALSE)",
     "BINOM.DIST(0,10,0.3,TRUE)", "BINOM.DIST(11,10,0.5,TRUE)", "BINOM.DIST(3,10,1.5,TRUE)",
     "BINOMDIST(50,100,0.3,TRUE)", "BINOM.DIST(3,10,0,FALSE)", "BINOM.DIST(0,10,0,FALSE)",
     "BINOM.INV(6,0.5,0.75)", "CRITBINOM(10,0.3,0.5)", "BINOM.INV(100,0.3,0.05)",
     "BINOM.INV(10,0.5,0)", "BINOM.INV(10,0.5,1)", "BINOM.INV(10,1.5,0.5)",
     "BINOM.INV(-1,0.5,0.5)", "BINOM.INV(10,0.9,0.3)",
     "BINOM.DIST.RANGE(60,0.75,48)", "BINOM.DIST.RANGE(60,0.75,45,50)",
     "BINOM.DIST.RANGE(10,0.5,0,10)", "BINOM.DIST.RANGE(10,0.5,6,5)", "BINOM.DIST.RANGE(10,0.5,11)",
     "BINOM.DIST.RANGE(10,0,0,3)", "B(20,0.3,5)",
     "CHISQ.DIST(0.5,1,TRUE)", "CHISQ.DIST(2,3,FALSE)", "CHISQ.DIST(3,4,TRUE)",
     "CHISQ.DIST(-1,2,TRUE)", "CHISQ.DIST(1,0,TRUE)", "CHISQ.DIST(5,2.7,TRUE)",
     "CHISQDIST(1.5,3,TRUE)",
     "CHISQ.DIST.RT(18.307,10)", "CHIDIST(3,4)", "CHISQ.DIST.RT(0,3)", "CHISQ.DIST.RT(-1,3)",
     "CHISQ.DIST.RT(50,10)", "CHISQ.DIST.RT(2,0.5)",
     "CHISQ.INV(0.93,1)", "CHISQ.INV(0.6,2)", "CHISQ.INV(0,3)", "CHISQ.INV(1,3)",
     "CHISQ.INV(0.5,10)", "CHISQINV(0.01,5)", "CHISQ.INV(-0.1,2)",
     "CHISQ.INV.RT(0.050001,10)", "CHIINV(0.5,3)", "CHISQ.INV.RT(0.999,4)", "CHISQ.INV.RT(1,3)",
     "CHISQ.INV.RT(0,3)", "CHISQ.INV.RT(0.05,0)",
     "EXPON.DIST(0.2,10,TRUE)", "EXPON.DIST(0.2,10,FALSE)", "EXPONDIST(0,2,FALSE)",
     "EXPON.DIST(0,2,TRUE)", "EXPON.DIST(1,0,TRUE)", "EXPON.DIST(3,0.5,TRUE)",
     "F.DIST(15.2069,6,4,TRUE)", "F.DIST(15.2069,6,4,FALSE)", "F.DIST(1,2,3,TRUE)",
     "F.DIST(0,2,3,FALSE)", "F.DIST(-1,2,3,TRUE)", "F.DIST(1,0,3,TRUE)", "F.DIST(2,5.9,10.5,TRUE)",
     "F.DIST.RT(15.2069,6,4)", "FDIST(1,2,3)", "F.DIST.RT(0,2,3)", "F.DIST.RT(-1,2,3)",
     "F.DIST.RT(3,10,20)",
     "F.INV(0.01,6,4)", "F.INV(0.5,2,3)", "F.INV(0.99,6,4)", "F.INV(1.5,2,3)", "F.INV(0.5,0,3)",
     "F.INV.RT(0.01,6,4)", "FINV(0.5,2,3)", "F.INV.RT(0.95,10,20)", "F.INV.RT(0,2,3)",
     "GAMMA.DIST(10.00001131,9,2,FALSE)", "GAMMA.DIST(10.00001131,9,2,TRUE)",
     "GAMMADIST(0,1,2,FALSE)", "GAMMA.DIST(-1,2,2,TRUE)", "GAMMA.DIST(1,0,2,TRUE)",
     "GAMMA.DIST(5,0.5,3,TRUE)", "GAMMA.DIST(100,50,1.5,FALSE)",
     "GAMMA.INV(0.068094,9,2)", "GAMMA.INV(0.5,0.5,1)", "GAMMA.INV(0.999,3,2)", "GAMMA.INV(0,2,2)",
     "GAMMA.INV(1,2,2)", "GAMMA.INV(0.5,0,2)", "GAMMAINV(0.00001,0.2,1)",
     "GAUSS(2)", "GAUSS(0)", "GAUSS(-1.5)", "GAUSS(0.1)", "GAUSS(-10)",
     "HYPGEOM.DIST(1,4,8,20,TRUE)", "HYPGEOM.DIST(1,4,8,20,FALSE)", "HYPGEOMDIST(1,4,8,20)",
     "HYPGEOM.DIST(4,4,8,20,TRUE)", "HYPGEOM.DIST(5,4,8,20,FALSE)", "HYPGEOM.DIST(1,25,8,20,FALSE)",
     "HYPGEOM.DIST(30,100,400,1000,TRUE)", "HYPGEOMDIST(2,10,5,50)", "HYPGEOM.DIST(3,4,8,20,TRUE)",
     "HYPGEOM.DIST(60,100,400,1000,TRUE)",
     "LOGNORM.DIST(4,3.5,1.2,TRUE)", "LOGNORM.DIST(4,3.5,1.2,FALSE)", "LOGNORMDIST(4,3.5,1.2)",
     "LOGNORM.DIST(4,3.5,0,TRUE)", "LOGNORM.DIST(1,0,1,FALSE)",
     "LOGNORM.INV(0.039084,3.5,1.2)", "LOGINV(0.5,0,1)", "LOGNORM.INV(0.9,1,0.5)",
     "LOGNORM.INV(0,1,1)", "LOGNORM.INV(1,1,1)", "LOGNORM.INV(0.5,1,-1)",
     "NEGBINOM.DIST(10,5,0.25,TRUE)", "NEGBINOM.DIST(10,5,0.25,FALSE)", "NEGBINOMDIST(10,5,0.25)",
     "NEGBINOM.DIST(0,1,0.5,FALSE)", "NEGBINOM.DIST(-1,5,0.25,TRUE)", "NEGBINOM.DIST(10,0,0.25,TRUE)",
     "NEGBINOM.DIST(3,2,1.5,TRUE)",
     "NORM.DIST(42,40,1.5,TRUE)", "NORM.DIST(42,40,1.5,FALSE)", "NORMDIST(-3,0,1,TRUE)",
     "NORM.DIST(1,0,0,TRUE)", "NORM.DIST(-20,0,1,TRUE)", "NORMDIST(0.5,1,2,FALSE)",
     "NORM.INV(0.908789,40,1.5)", "NORMINV(0.5,3,2)", "NORM.INV(0.0000000001,0,1)",
     "NORM.INV(0.5,0,0)", "NORM.INV(0.975,0,1)",
     "NORM.S.DIST(1.333333,TRUE)", "NORM.S.DIST(1.333333,FALSE)", "NORM.S.DIST(0,TRUE)",
     "NORM.S.DIST(-5,TRUE)", "NORM.S.DIST(-5,FALSE)",
     "NORMSDIST(1.333333)", "NORMSDIST(0)", "NORMSDIST(-2.5)", "NORMSDIST(8)",
     "NORM.S.INV(0.908789)", "NORMSINV(0.5)", "NORM.S.INV(0.001)", "NORM.S.INV(0.999999)",
     "PHI(0.75)", "PHI(0)", "PHI(-2)", "PHI(10)",
     "POISSON.DIST(2,5,TRUE)", "POISSON.DIST(2,5,FALSE)", "POISSON(0,1,FALSE)",
     "POISSON.DIST(2.9,5,FALSE)", "POISSON.DIST(-1,5,TRUE)", "POISSON.DIST(2,-1,TRUE)",
     "POISSON.DIST(800,750,TRUE)",
     "T.DIST(60,1,TRUE)", "T.DIST(8,3,FALSE)", "T.DIST(-1,5,TRUE)", "T.DIST(0.5,10,TRUE)",
     "T.DIST(1,0,TRUE)", "T.DIST(2,3.7,TRUE)", "T.DIST(0.01,4,TRUE)",
     "T.DIST.RT(1.959999998,60)", "T.DIST.RT(-1,5)", "T.DIST.RT(0,4)", "T.DIST.RT(3,0.5)",
     "T.DIST.RT(0.1,30)",
     "T.DIST.2T(1.959999998,60)", "T.DIST.2T(0,5)", "T.DIST.2T(-1,5)", "T.DIST.2T(3,2)",
     "T.DIST.2T(1,0)",
     "TDIST(1.959999998,60,2)", "TDIST(1.959999998,60,1)", "TDIST(1,5,3)", "TDIST(-1,5,1)",
     "TDIST(2.5,10,1)",
     "T.INV(0.75,2)", "T.INV(0.25,2)", "T.INV(0.5,5)", "T.INV(0.001,10)", "T.INV(0,5)",
     "T.INV(1,5)", "T.INV(0.9,0)", "T.INV(0.55,7)",
     "T.INV.2T(0.546449,60)", "TINV(0.05,10)", "T.INV.2T(1,5)", "T.INV.2T(0,5)",
     "T.INV.2T(0.001,3)", "T.INV.2T(0.5,0)",
     "WEIBULL.DIST(105,20,100,TRUE)", "WEIBULL.DIST(105,20,100,FALSE)", "WEIBULL(0,1,2,FALSE)",
     "WEIBULL.DIST(-1,2,3,TRUE)", "WEIBULL.DIST(1,0,3,TRUE)", "WEIBULL.DIST(2,0.5,3,TRUE)",
     "FISHER(0.75)", "FISHER(0)", "FISHER(-0.5)", "FISHER(1)", "FISHER(-1)", "FISHER(0.999)",
     "FISHERINV(0.972955)", "FISHERINV(0)", "FISHERINV(-2)", "FISHERINV(20)",
     "CONFIDENCE.NORM(0.05,2.5,50)", "CONFIDENCE(0.05,2.5,50)", "CONFIDENCE.NORM(0,2.5,50)",
     "CONFIDENCE.NORM(0.05,0,50)", "CONFIDENCE.NORM(0.05,2.5,0.5)", "CONFIDENCE.NORM(0.1,1,10.9)",
     "CONFIDENCE.T(0.05,1,50)", "CONFIDENCE.T(0.05,1,1)", "CONFIDENCE.T(0.1,2,5)",
     "CONFIDENCE.T(1,1,10)", "CONFIDENCE.T(0.05,1,0)",
     "STANDARDIZE(42,40,1.5)", "STANDARDIZE(10,10,1)", "STANDARDIZE(-5,0,2)",
     "STANDARDIZE(1,0,-1)")

# Overrides: LibreOffice disagrees with Excel's documented result.
# "If x < A, x > B, or A = B, BETADIST returns the #NUM! error value."
lo("BETADIST(-0.5,2,3)", excel=NUM)
lo("BETADIST(0.5,2,3,1,1)")
# BETA.INV: "If probability <= 0 or probability > 1, BETA.INV returns #NUM!".
lo("BETA.INV(0,2,3)", excel=NUM)
# EXPON.DIST: "If x < 0, EXPON.DIST returns the #NUM! error value."
lo("EXPON.DIST(-1,2,TRUE)", excel=NUM)
# HYPGEOM.DIST: "If sample_s is less than the larger of 0 or (number_sample -
# number_population + population_s), HYPGEOM.DIST returns #NUM!".
lo("HYPGEOM.DIST(0,15,8,20,FALSE)", excel=NUM)
# LOGNORM.DIST: "If x <= 0 or if standard_dev <= 0, returns the #NUM! error value."
lo("LOGNORM.DIST(0,3.5,1.2,TRUE)", excel=NUM)
lo("LOGNORMDIST(-1,0,1)", excel=NUM)
# NEGBINOMDIST: only "number_f < 0 or number_s < 1" is #NUM!; LibreOffice also
# rejects number_f + number_s <= 1. P(F = 0) with one success at p = 0.5 is 0.5.
lo("NEGBINOMDIST(0,1,0.5)", excel=0.5)
# NORM.INV / NORM.S.INV: "If probability <= 0 or if probability >= 1, returns
# #NUM!" (LibreOffice: #N/A).
lo("NORM.INV(0,0,1)", excel=NUM)
lo("NORM.INV(1,0,1)", excel=NUM)
lo("NORM.S.INV(0)", excel=NUM)
lo("NORM.S.INV(1)", excel=NUM)
# POISSON.DIST: only "mean < 0" is #NUM!; with mean 0 every event count but 0
# has probability 0 (LibreOffice rejects mean <= 0).
lo("POISSON.DIST(2,0,FALSE)", excel=0.0)
# GAUSS is "NORM.S.DIST(z,TRUE) - 0.5"; LibreOffice's Taylor series for it is
# off by 4e-11 at z = 5.
lo("GAUSS(5)", excel=float(mpmath.ncdf(5) - mpmath.mpf(0.5)))
# STANDARDIZE: "If standard_dev <= 0, STANDARDIZE returns the #NUM! error value."
lo("STANDARDIZE(1,0,0)", excel=NUM)

# Excel name -> LibreOffice ODFF name. Excel's F.DIST/F.INV are ODFF's
# FDIST/FINV; Excel's FDIST/FINV (right tail) are LEGACY.FDIST/LEGACY.FINV.
MS = ["GAMMALN.PRECISE", "ERF.PRECISE", "ERFC.PRECISE", "BETA.DIST", "BETA.INV", "BINOM.DIST",
      "BINOM.INV", "CHISQ.DIST", "CHISQ.DIST.RT", "CHISQ.INV", "CHISQ.INV.RT", "EXPON.DIST",
      "F.DIST.RT", "F.INV.RT", "GAMMA.DIST", "GAMMA.INV", "HYPGEOM.DIST", "LOGNORM.DIST",
      "LOGNORM.INV", "NEGBINOM.DIST", "NORM.DIST", "NORM.INV", "NORM.S.DIST", "NORM.S.INV",
      "POISSON.DIST", "T.DIST", "T.DIST.RT", "T.DIST.2T", "T.INV", "T.INV.2T", "WEIBULL.DIST",
      "CONFIDENCE.NORM", "CONFIDENCE.T"]
p1.LO_NAMES = {n: "COM.MICROSOFT." + n for n in MS}
p1.LO_NAMES.update({
    "F.DIST": "FDIST", "F.INV": "FINV", "FDIST": "LEGACY.FDIST", "FINV": "LEGACY.FINV",
    "NORMSDIST": "LEGACY.NORMSDIST", "NORMSINV": "LEGACY.NORMSINV", "TDIST": "LEGACY.TDIST",
    "CHIDIST": "LEGACY.CHIDIST", "CHIINV": "LEGACY.CHIINV", "B": "BINOM.DIST.RANGE",
})


# Every name parses (dotted names such as NORM.S.DIST and T.DIST.2T too).
UNPARSED: dict = {}


def map_error(f, shown):
    if shown in p1.LO_ERRORS:
        return err(shown)
    if shown in ("Err:502", "Err:523"):  # invalid argument, no convergence
        return NUM
    if shown == "Err:504":
        return err("#VALUE!")
    raise SystemExit(f"unmapped LibreOffice error {shown} for {f}")


# --- hand rows: Gnumeric-only special functions -------------------------------
GM = "Gnumeric source plugins/fn-math/functions.c"
mp = mpmath.mpf


def nonpos_int(x):
    return x <= 0 and x == math.floor(x)


def g_beta(a, b):  # gnm_beta: NaN at a or b a pole, 0 where only a + b is one
    if nonpos_int(a) or nonpos_int(b):
        return NUM
    if nonpos_int(a + b):
        return 0.0
    return mpmath.beta(mp(a), mp(b))


def g_betaln(a, b):  # gnm_lbeta3: log |B|; log 0 is -inf, so #NUM!
    v = g_beta(a, b)
    if isinstance(v, dict) or v == 0:
        return NUM
    return mpmath.log(abs(v))


def g_digamma(x):
    return NUM if nonpos_int(x) else mpmath.digamma(mp(x))


def g_igamma(a, x, lower=True, reg=True, real=True):
    # The real domain only (a, x >= 0); Gnumeric's complex results are not tested.
    if a == 0:
        if not reg:
            return NUM
        return 1.0 if lower else 0.0
    if not real:
        return 0.0
    v = mpmath.gammainc(mp(a), 0, mp(x)) if lower else mpmath.gammainc(mp(a), mp(x))
    return v / mpmath.gamma(mp(a)) if reg else v


def g_pochhammer(x, n):  # sf-gamma.c pochhammer
    if n == 0:
        return 1.0
    if nonpos_int(x):
        if n != math.floor(n) or (x == 0 and n > 0):
            return 0.0
        if n > -x:
            return NUM
        if x == 0:
            return (1 if math.fmod(-n, 2) == 0 else -1) / math.factorial(int(-n))
    if n == math.floor(n) and abs(n) < 100:  # the finite product, poles or not
        return mpmath.rf(mp(x), mp(n))
    if nonpos_int(x + n):
        return NUM
    return mpmath.rf(mp(x), mp(n))


def hand(name, fn, *args, **kw):
    literals = [repr(a) if isinstance(a, float) else str(a) for a in args]
    literals += ["TRUE" if v else "FALSE" for v in kw.values()]
    v = fn(*[float(a) for a in args], **kw)
    if not isinstance(v, dict):
        v = p1.gnm_float(float(v))
    return (f"{name}({','.join(literals)})", v, GM)


HAND = [
    hand("BETA", g_beta, 2, 3), hand("BETA", g_beta, -0.5, 0.5), hand("BETA", g_beta, 0.5, 0.5),
    hand("BETA", g_beta, -1, 2), hand("BETA", g_beta, 2.5, -1.5), hand("BETA", g_beta, 10, 20),
    hand("BETA", g_beta, 0, 1), hand("BETA", g_beta, 100.5, 80.25), hand("BETA", g_beta, 0.5, -1.5),
    hand("BETALN", g_betaln, 2, 3), hand("BETALN", g_betaln, -0.5, 0.5),
    hand("BETALN", g_betaln, 100, 200), hand("BETALN", g_betaln, 0.5, 0.5),
    hand("BETALN", g_betaln, -1.5, 0.7), hand("BETALN", g_betaln, -1, 2),
    hand("DIGAMMA", g_digamma, 1.46), hand("DIGAMMA", g_digamma, 15000),
    hand("DIGAMMA", g_digamma, 1), hand("DIGAMMA", g_digamma, 0), hand("DIGAMMA", g_digamma, -2),
    hand("DIGAMMA", g_digamma, -0.5), hand("DIGAMMA", g_digamma, 0.25),
    hand("DIGAMMA", g_digamma, 10),
    hand("IGAMMA", g_igamma, 2.5, 1.8), hand("IGAMMA", g_igamma, 2.5, 1.8, lower=False),
    hand("IGAMMA", g_igamma, 2.5, 1.8, lower=True, reg=False),
    hand("IGAMMA", g_igamma, 2.5, 1.8, lower=False, reg=False),
    hand("IGAMMA", g_igamma, 2.5, 1.8, lower=True, reg=True, real=False),
    hand("IGAMMA", g_igamma, 1, 0), hand("IGAMMA", g_igamma, 0, 2),
    hand("IGAMMA", g_igamma, 3, 10),
    hand("POCHHAMMER", g_pochhammer, 1, 5), hand("POCHHAMMER", g_pochhammer, 6, 0.5),
    hand("POCHHAMMER", g_pochhammer, 2.5, 3), hand("POCHHAMMER", g_pochhammer, 3, -2),
    hand("POCHHAMMER", g_pochhammer, -3, 2), hand("POCHHAMMER", g_pochhammer, 0, 3),
    hand("POCHHAMMER", g_pochhammer, 0, -2), hand("POCHHAMMER", g_pochhammer, -3, 5),
    hand("POCHHAMMER", g_pochhammer, 0.5, -1), hand("POCHHAMMER", g_pochhammer, -1.5, -0.5),
]


def main():
    lo_formulas = [p1.to_lo(f) for f, _, _ in LO]
    vals = p1.run_lo(["PI()"] + lo_formulas)
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    rows = []
    for (f, _, excel), v in zip(LO, vals[1:]):
        if isinstance(v, tuple):
            v = map_error(f, v[1])
        if excel is not None:
            print(f"LibreOffice differs: {f} LO={p1.fmt(v)} Excel={p1.fmt(excel)}", file=sys.stderr)
            rows.append((f, excel, "Excel docs"))
        else:
            rows.append((f, v, "LibreOffice"))
    rows += HAND
    kept = []
    for f, v, src in rows:
        name = p1.outer(f)
        if name in UNPARSED:
            if UNPARSED[name] is None:
                print(f"unparsed, not written: {f}\t{p1.fmt(v)}", file=sys.stderr)
                continue
            f = UNPARSED[name] + f[len(name):]
        kept.append((f, v, src))
    rows = kept
    for f, _, _ in rows:
        assert "\t" not in f and "^" not in f, f
    print("formula\texpected\tsource")
    for f, v, s in rows:
        print(f"{f}\t{p1.fmt(v)}\t{s}")


if __name__ == "__main__":
    main()
