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


def gn(f, tol=1e-9):
    GN.append((f, tol))


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
    if tol == 1e-9:
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
        print(f"{f}\t{cell(v, tol)}\tGnumeric 1.12.57")


if __name__ == "__main__":
    main()
