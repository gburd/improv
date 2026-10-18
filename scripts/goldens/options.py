#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/options_goldens.tsv (P7 option pricing).

    python3 scripts/goldens/options.py > crates/engine/tests/data/options_goldens.tsv

Gnumeric's `fn-derivatives` functions (OPT_* except the four LibreOffice
ones) are computed by Gnumeric 1.12.57's `ssconvert --recalc` when
GNUMERIC_SSCONVERT names its binary; otherwise the values recorded in
options_gnumeric.json are used. ssconvert's text export prints the shortest
round-trip repr, so its doubles are exact.

Closed forms are compared within 1e-9 relative; Gnumeric's iterative
solvers (Newton, bisection) stop at their own loose tolerances, so those
rows carry a looser `approx` tolerance.
"""

import json
import os
import subprocess
import sys
import tempfile
from xml.sax.saxutils import escape

GN = []  # (formula, tolerance)
FIX = {}  # formula -> (value, why): Gnumeric is wrong, an independent value instead


def gn(f, tol=1e-9):
    GN.append((f, tol))


def fix(f, value, tol):
    GN.append((f, tol))
    FIX[f] = value


# Gnumeric's cum_biv_norm_dist1 computes `delta = (1 - Sgn(a)*Sgn(b)) / 4` in
# integer arithmetic, so when a*b*rho > 0 and a, b differ in sign it is ½ too
# high. Rows that reach that branch are checked against mpmath instead (exact
# bivariate normal by quadrature, same formulas as Gnumeric otherwise).
def mp_bvn(a, b, rho):
    import mpmath as m
    m.mp.dps = 30
    f = lambda x: m.npdf(x) * m.ncdf((b - rho * x) / m.sqrt(1 - rho * rho))  # noqa: E731
    return float(m.quad(f, [-m.inf, min(a, 0), a]) if a > 0 else m.quad(f, [-m.inf, a]))


def mp_n(x):
    import mpmath as m
    return float(m.ncdf(x))


def mp_bs_call(s, x, t, r, v, b):
    import math
    d1 = (math.log(s / x) + (b + v * v / 2) * t) / (v * math.sqrt(t))
    return s * math.exp((b - r) * t) * mp_n(d1) - x * math.exp(-r * t) * mp_n(d1 - v * math.sqrt(t))


def mp_d1(s, x, t, v, b):
    import math
    return (math.log(s / x) + (b + v * v / 2) * t) / (v * math.sqrt(t))


def mp_bs(z, s, x, t, r, v, b):
    import math
    d1 = mp_d1(s, x, t, v, b)
    d2 = d1 - v * math.sqrt(t)
    return z * (s * math.exp((b - r) * t) * mp_n(z * d1) - x * math.exp(-r * t) * mp_n(z * d2))


def mp_on_options(kind, s, x1, x2, t1, t2, r, b, v):
    """Gnumeric's compound option formulas, critical price solved exactly."""
    import math
    z = 1 if kind in ("cc", "pc") else -1
    si = x1
    for _ in range(100):
        g = mp_bs(z, si, x1, t2 - t1, r, v, b) - x2
        dg = math.exp((b - r) * (t2 - t1)) * (mp_n(mp_d1(si, x1, t2 - t1, v, b)) - (z < 0))
        step = g / dg
        si -= step
        if abs(step) <= 1e-13 * abs(si):
            break
    rho = math.sqrt(t1 / t2)
    y1 = mp_d1(s, si, t1, v, b)
    y2 = y1 - v * math.sqrt(t1)
    z1 = mp_d1(s, x1, t2, v, b)
    z2 = z1 - v * math.sqrt(t2)
    c = s * math.exp((b - r) * t2)
    d1, d2 = x1 * math.exp(-r * t2), x2 * math.exp(-r * t1)
    B, N = mp_bvn, mp_n
    return {"cc": c * B(z1, y1, rho) - d1 * B(z2, y2, rho) - d2 * N(y2),
            "pc": d1 * B(z2, -y2, -rho) - c * B(z1, -y1, -rho) + d2 * N(-y2),
            "cp": d1 * B(-z2, -y2, rho) - c * B(-z1, -y1, rho) - d2 * N(-y2),
            "pp": c * B(-z1, y1, -rho) - d1 * B(-z2, y2, -rho) + d2 * N(y2)}[kind]


def mp_extendible_writer(z, s, x1, x2, t1, t2, r, b, v):
    import math
    rho = math.sqrt(t1 / t2)
    z1, z2 = mp_d1(s, x2, t2, v, b), mp_d1(s, x1, t1, v, b)
    w1, w2 = v * math.sqrt(t1), v * math.sqrt(t2)
    return mp_bs(z, s, x1, t1, r, v, b) + z * (
        s * math.exp((b - r) * t2) * mp_bvn(z * z1, -z * z2, -rho)
        - x2 * math.exp(-r * t2) * mp_bvn(z * (z1 - w2), z * (w1 - z2), -rho))


def mp_two_asset(z, s1, s2, x1, x2, t, b1, b2, r, v1, v2, rho):
    import math
    st = math.sqrt(t)
    y1 = (math.log(s1 / x1) + (b1 - v1 * v1 / 2) * t) / (v1 * st)
    y2 = (math.log(s2 / x2) + (b2 - v2 * v2 / 2) * t) / (v2 * st)
    return z * (s2 * math.exp((b2 - r) * t) * mp_bvn(z * (y2 + v2 * st), z * (y1 + rho * v2 * st), rho)
                - x2 * math.exp(-r * t) * mp_bvn(z * y2, z * y1, rho))


def mp_rgw(s, x, t1, t2, r, d, v):
    """Roll-Geske-Whaley as Gnumeric computes it (bisection to 1e-5)."""
    import math
    sx = s - d * math.exp(-r * t1)
    gap = lambda i: mp_bs_call(i, x, t2 - t1, r, v, 0) - i - d + x  # noqa: E731
    hi = s
    while gap(hi) > 0 and hi < 1e8:
        hi *= 2
    lo, i = 0.0, hi / 2
    g = gap(i)
    while abs(g) > 1e-5 and hi - lo > 1e-5:
        hi, lo = (i, lo) if g < 0 else (hi, i)
        i = (hi + lo) / 2
        g = gap(i)
    sq = math.sqrt
    a1 = (math.log(sx / x) + (r + v * v / 2) * t2) / (v * sq(t2))
    a2 = a1 - v * sq(t2)
    b1 = (math.log(sx / i) + (r + v * v / 2) * t1) / (v * sq(t1))
    b2 = b1 - v * sq(t1)
    rho = -sq(t1 / t2)
    return (sx * mp_n(b1) + sx * mp_bvn(a1, -b1, rho)
            - x * math.exp(-r * t2) * mp_bvn(a2, -b2, rho)
            - (x - d) * math.exp(-r * t1) * mp_n(b2))


def many(*fs, tol=1e-9):
    for f in fs:
        gn(f, tol)


# --- Black–Scholes and the Greeks ---------------------------------------------
for name in ("OPT_BS", "OPT_BS_DELTA", "OPT_BS_RHO", "OPT_BS_THETA", "OPT_BS_CARRYCOST"):
    many(f'{name}("c",100,95,0.5,0.1,0.25)', f'{name}("p",100,95,0.5,0.1,0.25)',
         f'{name}("c",75,70,0.25,0.06,0.3,0.06)', f'{name}("P",75,70,0.25,0.06,0.3,0.02)',
         f'{name}("c",100,100,0,0.1,0.25)', f'{name}("c",100,90,0,0.1,0.25)',
         f'{name}("c",100,100,0.5,0.1,0)',
         f'{name}("x",100,100,0.5,0.1,0.25)')
for name in ("OPT_BS_GAMMA", "OPT_BS_VEGA"):
    many(f"{name}(100,95,0.5,0.1,0.25)", f"{name}(75,70,0.25,0.06,0.3,0.06)",
         f"{name}(55,60,0.75,0.1,0.3,0.1)", f"{name}(100,100,0,0.1,0.25)",
         f"{name}(100,100,0.5,0.1,0)")
# --- European variants ----------------------------------------------------------
many('OPT_GARMAN_KOHLHAGEN("c",1.56,1.6,0.5,0.06,0.08,0.12)',
     'OPT_GARMAN_KOHLHAGEN("p",1.56,1.6,0.5,0.06,0.08,0.12)',
     'OPT_GARMAN_KOHLHAGEN("c",100,95,1,0.03,0.01,0.2)',
     'OPT_GARMAN_KOHLHAGEN("c",1.56,1.6,0,0.06,0.08,0.12)',
     'OPT_GARMAN_KOHLHAGEN("z",1.56,1.6,0.5,0.06,0.08,0.12)')
many('OPT_FRENCH("c",70,75,0.1,0.11,0.1,0.2,0.1)', 'OPT_FRENCH("p",70,75,0.1,0.11,0.1,0.2,0.1)',
     'OPT_FRENCH("c",100,95,0.5,0.55,0.05,0.3,0.02)', 'OPT_FRENCH("c",70,75,0,0.11,0.1,0.2,0.1)',
     'OPT_FRENCH("q",70,75,0.1,0.11,0.1,0.2,0.1)')

many('OPT_JUMP_DIFF("c",45,55,0.25,0.1,0.25,3,0.4)', 'OPT_JUMP_DIFF("p",45,55,0.25,0.1,0.25,3,0.4)',
     'OPT_JUMP_DIFF("c",100,95,0.5,0.05,0.3,1,0.2)', 'OPT_JUMP_DIFF("c",45,55,0.25,0.1,0.25,0,0.4)',
     'OPT_JUMP_DIFF("c",45,55,0,0.1,0.25,3,0.4)', 'OPT_JUMP_DIFF("n",45,55,0.25,0.1,0.25,3,0.4)')
MS = "0.9608,1,1,0.25,0.5,0.3345,0.2155,0.0089,0.805,0.0964,0.1243,1.045,0.2"
many(f'OPT_MILTERSEN_SCHWARTZ("c",{MS})', f'OPT_MILTERSEN_SCHWARTZ("p",{MS})',
     'OPT_MILTERSEN_SCHWARTZ("c",0.95,1.1,1,0.5,1,0.3,0.2,0.01,0.8,0.1,0.1,1,0.3)',
     'OPT_MILTERSEN_SCHWARTZ("p",0.95,1.1,1,0.5,1,0.3,0.2,0.01,0.8,0.1,0.1,1,0.3)',
     f'OPT_MILTERSEN_SCHWARTZ("x",{MS})')
# --- exotics ----------------------------------------------------------------------
many('OPT_EXEC("c",65,64,2,0.07,0.38,0.04,0.15)', 'OPT_EXEC("p",65,64,2,0.07,0.38,0.04,0.15)',
     'OPT_EXEC("c",65,64,2,0.07,0.38,0.04,0)', 'OPT_EXEC("c",65,64,0,0.07,0.38,0.04,0.15)',
     'OPT_EXEC("y",65,64,2,0.07,0.38,0.04,0.15)')
many('OPT_FORWARD_START("c",60,1.1,0.25,1,0.08,0.3,0.04)',
     'OPT_FORWARD_START("p",60,1.1,0.25,1,0.08,0.3,0.04)',
     'OPT_FORWARD_START("c",100,0.9,0.5,1.5,0.05,0.2,0.05)',
     'OPT_FORWARD_START("c",60,1.1,1,1,0.08,0.3,0.04)',
     'OPT_FORWARD_START("w",60,1.1,0.25,1,0.08,0.3,0.04)')
many('OPT_TIME_SWITCH("c",100,110,5,1,0,1/365,0.06,0.06,0.26)',
     'OPT_TIME_SWITCH("p",100,110,5,1,0,1/365,0.06,0.06,0.26)',
     'OPT_TIME_SWITCH("c",100,90,1,0.5,10,1/52,0.05,0.02,0.3)',
     'OPT_TIME_SWITCH("c",100,110,5,0,3,1/365,0.06,0.06,0.26)',
     'OPT_TIME_SWITCH("c",100,110,5,1,0,0,0.06,0.06,0.26)',
     'OPT_TIME_SWITCH("k",100,110,5,1,0,1/365,0.06,0.06,0.26)')
many("OPT_SIMPLE_CHOOSER(50,50,0.25,0.5,0.08,0.08,0.25)",
     "OPT_SIMPLE_CHOOSER(100,95,0.5,1,0.05,0.02,0.3)",
     "OPT_SIMPLE_CHOOSER(50,50,0,0.5,0.08,0.08,0.25)",
     "OPT_SIMPLE_CHOOSER(50,50,0.25,0.5,0.08,0.08,0)")

# --- American (Gnumeric's solvers stop at 1e-6 relative / 1e-5 absolute) --------
# BAW: Gnumeric stops the critical-price search at 1e-6 relative in the
# exercise condition, Improv at 1e-12 in the price, ~1e-5 apart.
many('OPT_BAW_AMER("c",100,100,0.5,0.1,0.25,0)', 'OPT_BAW_AMER("p",100,100,0.5,0.1,0.25,0)',
     'OPT_BAW_AMER("c",100,90,0.25,0.08,0.35,-0.04)', 'OPT_BAW_AMER("p",90,100,0.25,0.08,0.35,0.08)',
     'OPT_BAW_AMER("p",60,100,0.25,0.08,0.2,0.08)', 'OPT_BAW_AMER("c",100,100,0.5,0.1,0.25,0.1)',
     'OPT_BAW_AMER("b",100,100,0.5,0.1,0.25,0)', tol=1e-5)
many('OPT_BJER_STENS("c",42,40,0.75,0.04,0.35,-0.04)', 'OPT_BJER_STENS("p",42,40,0.75,0.04,0.35,-0.04)',
     'OPT_BJER_STENS("p",40,40,0.5,0.06,0.3,0.06)', 'OPT_BJER_STENS("c",100,100,0.5,0.1,0.25,0.1)',
     'OPT_BJER_STENS("c",200,100,0.5,0.1,0.25,0)', 'OPT_BJER_STENS("h",42,40,0.75,0.04,0.35,-0.04)')
# RGW: Haug's example (his book: 4.3860) reaches the bvn bug (Gnumeric 2.2267).
fix("OPT_RGW(80,82,0.25,0.3333,0.06,4,0.3)", mp_rgw(80, 82, 0.25, 0.3333, 0.06, 4, 0.3), 1e-7)
fix("OPT_RGW(100,90,0.5,1,0.05,5,0.25)", mp_rgw(100, 90, 0.5, 1, 0.05, 5, 0.25), 1e-7)
fix("OPT_RGW(82.5,82,0.25,0.3333,0.06,4,0.3)", mp_rgw(82.5, 82, 0.25, 0.3333, 0.06, 4, 0.3), 1e-7)
many("OPT_RGW(80,82,0.25,0.3333,0.06,0.1,0.3)",
     "OPT_RGW(0,82,0.25,0.3333,0.06,4,0.3)", tol=1e-7)
many('OPT_BINOMIAL("a","p",100,100,95,0.5,0.08,0.3,0.08)',
     'OPT_BINOMIAL("e","p",100,100,95,0.5,0.08,0.3,0.08)',
     'OPT_BINOMIAL("A","c",250,100,95,0.5,0.08,0.3,0.04)',
     'OPT_BINOMIAL("e","c",500,100,95,0.5,0.08,0.3)',
     'OPT_BINOMIAL("a","c",0,100,95,0.5,0.08,0.3,0.08)',
     'OPT_BINOMIAL("a","c",-1,100,95,0.5,0.08,0.3,0.08)',
     'OPT_BINOMIAL("x","c",10,100,95,0.5,0.08,0.3,0.08)',
     'OPT_BINOMIAL("a","x",10,100,95,0.5,0.08,0.3,0.08)', tol=1e-11)

# --- exotics needing the bivariate normal ------------------------------------------
# Gnumeric's bivariate normal (Drezner 1978) is good to about 1e-6, and its
# critical-price searches stop at 1e-3 (chooser) and 1e-4 (compound).
many("OPT_COMPLEX_CHOOSER(50,55,48,0.25,0.5,0.5833,0.1,0.05,0.35)",
     "OPT_COMPLEX_CHOOSER(100,100,100,0.25,1,1,0.05,0.02,0.3)",
     "OPT_COMPLEX_CHOOSER(50,55,48,0.25,0.5,0.5833,0.1,0.05,0)", tol=1e-5)
# pc and pp reach the bug (Gnumeric 267.6 for a put worth at most its strike
# of 50). mp_on_options agrees with direct quadrature of the payoff over the
# lognormal spot at t1 to 1e-8.
OO = (500, 520, 50, 0.25, 0.5, 0.08, 0.03, 0.35)
for k in ("pc", "pp"):
    fix(f'OPT_ON_OPTIONS("{k}",500,520,50,0.25,0.5,0.08,0.03,0.35)', mp_on_options(k, *OO), 1e-9)
many('OPT_ON_OPTIONS("cc",500,520,50,0.5,0.75,0.08,0.03,0.35)',
     'OPT_ON_OPTIONS("cp",500,520,50,0.5,0.75,0.08,0.03,0.35)',
     'OPT_ON_OPTIONS("xx",500,520,50,0.5,0.75,0.08,0.03,0.35)', tol=1e-5)
many('OPT_EXTENDIBLE_WRITER("c",80,90,82,0.5,0.75,0.1,0.1,0.3)',
     'OPT_EXTENDIBLE_WRITER("p",80,90,82,0.5,0.75,0.1,0.1,0.3)',
     'OPT_EXTENDIBLE_WRITER("c",80,90,82,0.5,0.75,0.1,0.1,0)',
     'OPT_EXTENDIBLE_WRITER("v",80,90,82,0.5,0.75,0.1,0.1,0.3)', tol=1e-6)
fix('OPT_EXTENDIBLE_WRITER("c",100,95,100,0.25,0.5,0.05,0.02,0.25)',
    mp_extendible_writer(1, 100, 95, 100, 0.25, 0.5, 0.05, 0.02, 0.25), 1e-9)
fix('OPT_2_ASSET_CORRELATION("c",100,100,95,105,1,0.05,0.03,0.05,0.25,0.2,-0.4)',
    mp_two_asset(1, 100, 100, 95, 105, 1, 0.05, 0.03, 0.05, 0.25, 0.2, -0.4), 1e-9)
many('OPT_2_ASSET_CORRELATION("c",52,65,50,70,0.5,0.1,0.1,0.1,0.2,0.3,0.75)',
     'OPT_2_ASSET_CORRELATION("p",52,65,50,70,0.5,0.1,0.1,0.1,0.2,0.3,0.75)',
     'OPT_2_ASSET_CORRELATION("c",52,65,50,70,0,0.1,0.1,0.1,0.2,0.3,0.75)',
     'OPT_2_ASSET_CORRELATION("x",52,65,50,70,0.5,0.1,0.1,0.1,0.2,0.3,0.75)', tol=1e-6)


def err(name):
    return {"error": name}


def gnumeric(formulas):
    exe = os.environ.get("GNUMERIC_SSCONVERT")
    if not exe:
        return None
    with tempfile.TemporaryDirectory() as d:
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
    if len(vals) != len(formulas):
        raise SystemExit(f"read {len(vals)} results for {len(formulas)} formulas")
    return [err(v) if v.startswith("#") else float(v) for v in vals]


def cell(v, tol):
    if isinstance(v, dict):
        return json.dumps(v)
    if tol <= 1e-9:
        return repr(v)
    return json.dumps({"approx": v, "tol": tol})


RECORD = os.path.join(os.path.dirname(os.path.abspath(__file__)), "options_gnumeric.json")


def main():
    fs = [f for f, _ in GN]
    got = gnumeric(fs)
    if got is None:
        with open(RECORD, encoding="utf-8") as fh:
            rec = json.load(fh)
        got = [rec[f] for f in fs]
    elif sys.argv[1:] == ["--record"]:
        with open(RECORD, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(dict(zip(fs, got)), indent=1) + "\n")
        return
    print("formula\texpected\tsource")
    for (f, tol), v in zip(GN, got):
        assert "\t" not in f and "^" not in f, f
        if f in FIX:
            print(f"Gnumeric bug: {f} Gnumeric={v!r} mpmath={FIX[f]!r}", file=sys.stderr)
            print(f"{f}\t{json.dumps({'approx': FIX[f], 'tol': tol})}\tmpmath (Gnumeric 1.12.57 bivariate normal bug)")
        else:
            print(f"{f}\t{cell(v, tol)}\tGnumeric 1.12.57")


if __name__ == "__main__":
    main()
