#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/financial_reduce.tsv.

    python3 scripts/goldens/financial_reduce.py > crates/engine/tests/data/financial_reduce.tsv

The cash-flow reductions (NPV, XNPV, IRR, XIRR, MIRR, FVSCHEDULE, FMRR).
Each row is one group of one case (a case with several groups is one model
with a second category, Group): the case number, the group, the Improv
formula (over the measures CF, Day = the dates as DateTime values, and
Serial = the same dates as numbers, reduced OVER Time), the group's flows in
item order (`null` = no cell, `{"error": ...}` = an error cell), its dates
as serial numbers, the expected value and its source. IRR, XIRR and FMRR
expect `{"approx": x, "tol": 1e-9}` (relative): the solvers stop at 1e-10.

LibreOffice rows: every group's flows go into column B (dates in C) of a data
sheet, and the formula is evaluated over that range with p1.py's machinery
(its exact-double reconstruction too). LibreOffice's Err:502 and Err:523 (no
convergence) are #NUM!. Rows where LibreOffice disagrees with Excel's
documentation record Excel's value (source `Excel docs`). An error cell among
the flows is the result in Improv (the first in item order, the reduction
rule); LibreOffice instead gives #VALUE! (XNPV, FVSCHEDULE) or skips the
cell (MIRR), and those rows say `Improv rule`. stderr shows both values.

FMRR rows (LibreOffice has no FMRR) come from `oleo_fmrr`, a line-by-line
transliteration of `do_fmrr` in GNU Oleo 1.99.16's src/busi.c.
"""

import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402

err = p1.err
NUM, DIV0 = err("#NUM!"), err("#DIV/0!")

# Excel's documentation examples (XNPV/XIRR, MIRR, IRR, NPV, FVSCHEDULE).
XD = [39448, 39508, 39751, 39859, 39904]  # 2008-01-01 … 2009-04-01
XV = [-10000, 2750, 4250, 3250, 2750]
MV = [-120000, 39000, 30000, 21000, 37000, 46000]
IV = [-70000, 12000, 15000, 18000, 21000, 26000]
NV = [-10000, 3000, 4200, 6800]
LOAN = [10000, -2000, -2000, -2000, -2000, -2000, -2000]
SIGNS = [-1000, 800, -500, 900, -200, 600]  # several sign changes
POS = [100, 200, 300]
GAP = [-1000, None, 400, 700]
BAD = [-1000, err("#DIV/0!"), 500]
TWO = [[-1000, 300, 400, 500], [-200, 50, 60, 70, 80]]

# (formula, [(flows, dates)], expected override per group or None, source)
CASES = []


def lo(formula, *groups, excel=None, rule=None):
    """A LibreOffice case; `groups` are flow lists or (flows, dates)."""
    gs = [g if isinstance(g, tuple) else (g, []) for g in groups]
    CASES.append((formula, gs, (excel, "Excel docs") if excel else (rule, "Improv rule")))


# --- NPV ----------------------------------------------------------------------
NPV = "NPV(0.1, CF OVER Time)"
lo(NPV, NV)
lo(NPV, LOAN)
lo(NPV, [-500])
lo(NPV, GAP)
lo(NPV, BAD)
lo(NPV, *TWO)
lo("NPV(-0.5, CF OVER Time)", SIGNS)
lo("NPV(-1, CF OVER Time)", NV)

# --- XNPV ---------------------------------------------------------------------
lo("XNPV(0.09, CF, Day OVER Time)", (XV, XD))
lo("XNPV(0.09, CF, Serial OVER Time)", (XV[:4], [39448, 39508, 39751, 39859.7]))
lo("XNPV(0.09, CF, Day OVER Time)", (LOAN[:4], [45658, 45689, 45717, 45748]),
   (XV[:3], XD[:3]))
lo("XNPV(0.09, CF, Day OVER Time)", ([-500], [45658]))
lo("XNPV(0.09, CF, Day OVER Time)", (XV, [39448, 39508, 39751, 39400, 39904]), excel=[NUM])
lo("XNPV(-1, CF, Day OVER Time)", (XV, XD))
lo("XNPV(0.09, CF, Day OVER Time)", (BAD, XD[:3]), rule=[DIV0])

# --- IRR ----------------------------------------------------------------------
lo("IRR(CF OVER Time)", IV)
lo("IRR(CF OVER Time)", IV[:5])
lo("IRR(CF OVER Time, -0.1)", IV[:3])
lo("IRR(CF OVER Time)", LOAN)
lo("IRR(CF OVER Time)", SIGNS)
lo("IRR(CF OVER Time)", POS)
lo("IRR(CF OVER Time)", [-500])
lo("IRR(CF OVER Time)", GAP)
lo("IRR(CF OVER Time)", BAD)
lo("IRR(CF OVER Time)", *TWO)

# --- XIRR ---------------------------------------------------------------------
lo("XIRR(CF, Day OVER Time)", (XV, XD))
lo("XIRR(CF, Day OVER Time, 0.5)", (XV, XD))
lo("XIRR(CF, Serial OVER Time)", (LOAN[:4], [45658, 45689, 45717, 45748.25]))
lo("XIRR(CF, Day OVER Time)", (POS, XD[:3]))
lo("XIRR(CF, Day OVER Time)", ([-500], [45658]))
lo("XIRR(CF, Day OVER Time)", (XV, [39448, 39508, 39751, 39400, 39904]), excel=[NUM])
lo("XIRR(CF, Day OVER Time)", (XV[:3], XD[:3]), (SIGNS, [45658 + 30 * k for k in range(6)]))

# --- MIRR ---------------------------------------------------------------------
lo("MIRR(CF OVER Time, 0.1, 0.12)", MV)
lo("MIRR(CF OVER Time, 0.1, 0.12)", MV[:4])
lo("MIRR(CF OVER Time, 0.1, 0.14)", MV)
lo("MIRR(CF OVER Time, 0.05, 0.08)", SIGNS)
lo("MIRR(CF OVER Time, 0.1, 0.1)", POS, excel=[DIV0])
lo("MIRR(CF OVER Time, 0.1, 0.12)", GAP)
lo("MIRR(CF OVER Time, 0.1, 0.12)", BAD, rule=[DIV0])
lo("MIRR(CF OVER Time, 0.1, 0.12)", *TWO)

# --- FVSCHEDULE -----------------------------------------------------------------
lo("FVSCHEDULE(1, CF OVER Time)", [0.09, 0.11, 0.1])
lo("FVSCHEDULE(1000, CF OVER Time)", [0.05, -0.02, 0.03, 0])
lo("FVSCHEDULE(250, CF OVER Time)", [0.07])
lo("FVSCHEDULE(100, CF OVER Time)", [0.05, None, 0.05])
lo("FVSCHEDULE(100, CF OVER Time)", [0.05, err("#N/A"), 0.05], rule=[err("#N/A")])
lo("FVSCHEDULE(-100, CF OVER Time)", [0.1, 0.2], [-0.5, 0.25, 1])


# --- FMRR (GNU Oleo) ------------------------------------------------------------
def oleo_fmrr(flows, safe_rate, reinv_rate, reinv_min):
    """do_fmrr, GNU Oleo 1.99.16 src/busi.c, line by line."""
    v = [float(x) for x in flows]
    num = len(v)
    # pull back negative values to most recent positive at safe rate
    for i in range(num - 1, 0, -1):
        while v[i] < 0.:
            j = i - 1
            while j > 0:
                if v[j] > 0.:
                    amt = -v[i]
                    if amt > v[j]:
                        amt = v[j]
                    v[i] += amt
                    amt /= pow(1. + safe_rate, float(i - j))
                    v[j] -= amt
                    break
                j -= 1
            if j == 0:  # leftover falls into initial investment
                v[0] -= -v[i] / pow(1. + safe_rate, float(i))
                v[i] = 0.
    # push forward accumulations at reinvestment rate, as long as over min
    for i in range(1, num - 1):
        if v[i]:
            if v[i] >= reinv_min:
                v[num - 1] += v[i] * pow(1. + reinv_rate, float(num - 1 - i))
                v[i] = 0.
            else:
                v[i + 1] += v[i] * (1. + safe_rate)
    # now only v[0] and v[num-1] are nonzero, what is rate to connect them?
    return pow(v[num - 1] / -v[0], 1. / float(num - 1)) - 1.


OLEO = "GNU Oleo busi.c"
HAND = []  # (formula, [(flows, dates)], [expected per group], source)


def oleo(params, *groups):
    s, r, m = params
    HAND.append((f"FMRR(CF OVER Time, {s}, {r}, {m})", [(g, []) for g in groups],
                 [oleo_fmrr(g, s, r, m) for g in groups], OLEO))


oleo((0.05, 0.1, 100), [-1000, 300, -200, 500, 600])
oleo((0.05, 0.12, 400), [-1000, 300, 200, 500, 600])  # small inflows move at the safe rate
oleo((0.06, 0.1, 0), [-1000, -500, 200, -100, 900])  # an outflow no inflow covers
oleo((0.04, 0.1, 50), [-1000, 100, -150, 800])  # a capped, repeated pull-back
oleo((0.04, 0.1, 50), [-1000, 100, -103, 800])  # repeated, and the inflow covers it
oleo((0, 0.1, 50), [-1000, 100, 200, -350, 900])  # a zero safe rate
oleo((0.05, 0.1, 100), [-8000, 2000, -500, 3000, 4000], [-200, 50, 60, 70, 80])
# A missing item is skipped, as in the other reductions (Oleo reads an empty
# cell in an allocated row as 0, and skips unallocated ones).
HAND.append(("FMRR(CF OVER Time, 0.05, 0.1, 100)", [(GAP, [])],
             [oleo_fmrr([x for x in GAP if x is not None], 0.05, 0.1, 100)],
             "GNU Oleo busi.c (no cell skipped)"))
# Oleo gives pow(-1, 1/0) - 1 = 0 for one flow and a meaningless rate when
# no outflow is left in period 0; Improv says #NUM!.
HAND.append(("FMRR(CF OVER Time, 0.05, 0.1, 100)", [([-500], [])], [NUM], "Improv (Oleo: 0)"))
HAND.append(("FMRR(CF OVER Time, 0.05, 0.1, 100)", [(POS, [])], [NUM],
             "Improv (Oleo: NaN)"))
HAND.append(("FMRR(CF OVER Time, 0.05, 0.1, 100)", [(BAD, [])], [DIV0], OLEO))


# --- LibreOffice ------------------------------------------------------------------
def cell(v):
    if v is None:
        return "<table:table-cell/>"
    if isinstance(v, dict):  # an error cell: a formula that yields it
        f = {"#DIV/0!": "1/0", "#N/A": "NA()"}[v["error"]]
        return f"<table:table-cell table:formula=\"of:={f}\"/>"
    return f'<table:table-cell office:value-type="float" office:value="{v!r}"/>'


def to_lo(formula, a, b):
    """CF -> column B rows a..b, Day/Serial -> column C, drop `OVER Time`."""
    f = formula.replace(" OVER Time", "")
    f = re.sub(r"\bCF\b", f"[D.B{a}:.B{b}]", f)
    f = re.sub(r"\b(Day|Serial)\b", f"[D.C{a}:.C{b}]", f)
    return p1.to_lo(f)


def main():
    formulas, data, row = [], [], 1
    for f, groups, _ in CASES:
        for flows, dates in groups:
            for k, v in enumerate(flows):
                d = dates[k] if k < len(dates) else None
                data.append("<table:table-row><table:table-cell/>" + cell(v) + cell(d)
                            + "</table:table-row>")
            formulas.append(to_lo(f, row, row + len(flows) - 1))
            row += len(flows)
    # p1.run_lo writes sheet S; the data sheet D follows it.
    p1.TAIL = '</table:table><table:table table:name="D">' + "".join(data) + p1.TAIL
    vals = iter(p1.run_lo(formulas))
    out = []
    for case, (f, groups, (fix, src)) in enumerate(CASES):
        for g, (flows, dates) in enumerate(groups):
            v = next(vals)
            if isinstance(v, tuple):
                v = NUM if v[1] in ("Err:502", "Err:523") else p1.map_error(f, v[1])
            if fix is not None:
                print(f"LibreOffice differs: {f} {flows} LO={p1.fmt(v)} {src}={p1.fmt(fix[g])}",
                      file=sys.stderr)
                out.append((case, g, f, flows, dates, fix[g], src))
            else:
                out.append((case, g, f, flows, dates, v, "LibreOffice"))
    if p1.UNMAPPED:
        raise SystemExit("unmapped LibreOffice errors:\n  " + "\n  ".join(p1.UNMAPPED))
    for case, (f, groups, want, src) in enumerate(HAND, len(CASES)):
        for g, (flows, dates) in enumerate(groups):
            out.append((case, g, f, flows, dates, want[g], src))
    print("case\tgroup\tformula\tflows\tdates\texpected\tsource")
    for case, g, f, flows, dates, v, src in out:
        if isinstance(v, float) and re.match(r"(IRR|XIRR|FMRR)\(", f):
            v = {"approx": v, "tol": 1e-9}
        print(f"{case}\t{g}\t{f}\t{json.dumps(flows)}\t{json.dumps(dates)}\t{p1.fmt(v)}\t{src}")


if __name__ == "__main__":
    main()
