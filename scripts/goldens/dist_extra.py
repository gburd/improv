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

A row is `formula` (exact to 1e-12 relative, the runner's default) or
`(formula, tol)`: within `tol` of the Gnumeric value, relative to
max(|x|, 1), where the value comes from an iteration or a quadrature whose
stopping rule differs from Improv's.
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
]


# Gnumeric's pnorm2 clamps Φ(x2) − Φ(x1) between dx·φ(x2) and dx·φ(x1) when
# the tails nearly cancel, so it keeps only ~8 digits (2.4197072123754992e-9;
# the exact value is 2.4197072183871969e-9).
MPMATH_WINS = {"SNORM.DIST.RANGE(1,1.00000001)"}


def checks():
    """Independent mpmath values for a few rows (formula -> value)."""
    import mpmath as mp
    mp.mp.dps = 30
    phi = lambda z: mp.ncdf(z)
    return {
        "CAUCHY(0.43,1,TRUE)": 0.5 - mp.atan(0.43) / mp.pi,
        "EXPPOWDIST(-1.2,2,0.5)": mp.exp(-mp.sqrt(0.6)) / (4 * mp.gamma(3)),
        "LOGISTIC(-2,0.5)": mp.exp(-4) / (0.5 * (1 + mp.exp(-4)) ** 2),
        "RAYLEIGHTAIL(2,1,0.5)": 8 * mp.exp((4 - 16) / 2),
        "SNORM.DIST.RANGE(1,1.00000001)": phi(mp.mpf(1.00000001)) - phi(1),
        "SNORM.DIST.RANGE(5,6)": phi(6) - phi(5),
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
        if abs(g - want) <= 1e-14 * max(abs(want), 1e-300):
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
