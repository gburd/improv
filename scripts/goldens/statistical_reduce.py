#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/statistical_reduce_goldens.tsv (the P4
statistical reductions).

    python3 scripts/goldens/statistical_reduce.py \\
        > crates/engine/tests/data/statistical_reduce_goldens.tsv

A reduction reads a category's cells, so each dataset (DATA) is written to
its own LibreOffice sheet, one column per measure and group, one row per item
of category T (a blank cell is an item with no cell). Each Improv formula,
`STDEV(X OVER T)`, becomes the spreadsheet formula over that column,
`STDEV($DA.B1:.B8)`, once per group (the items of category G); a per-item
reduction (RANK, PERCENTRANK, TREND, GROWTH) becomes one formula per item,
`RANK($DA.B3; $DA.B1:.B8)`. The formulas run through p1.py's runner (the
same exact-double reconstruction).

The TSV has two kinds of line:
    @name <TAB> {"T": items, "G": groups, "m": {"X": [[group 0 values], …]}}
    name <TAB> improv formula <TAB> expected <TAB> source
where expected lists one entry per group: a value, or for a per-item
reduction a list with one value per item (null: no cell).

Excel and LibreOffice differ where a range holds booleans: LibreOffice's
booleans are numbers, so its COUNT counts them; Excel's COUNT does not (rows
marked `Excel docs`). Hand rows cover what LibreOffice lacks: SUMA and KURTP
(Gnumeric's definitions, plugins/fn-stat), PERCENTOF (Excel's docs),
POLYNOMIAL (the plan's Numbers row), AUTOCORREL (GSL's
gsl_stats_lag1_autocorrelation) and EXPA/EXPB/EXPFORECAST (the plan's
Quantrix rows; Quantrix's own docs were not reachable, so the y-weighted fit
is assumed, not verified).
"""

import json
import math
import os
import re
import sys
from xml.sax.saxutils import escape

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402  (the LibreOffice runner)

T_, F_ = True, False
NA = {"error": "#N/A"}
NUM, DIV0, VAL = {"error": "#NUM!"}, {"error": "#DIV/0!"}, {"error": "#VALUE!"}

DATA = {
    # Ties in X (1, 1) and Y (8, 8, 8); text and boolean measures.
    "A": {"T": 8, "m": {
        "X": [[3, 1, 4, 1, 5, 9, 2, 6]],
        "Y": [[2, 7, 1, 8, 2, 8, 1.5, 8]],
        "C": [["apple", "Banana", "cherry", "apple", "date", "banana", "fig", "Apple"]],
        "P": [[0.1, 0.2, 0.1, 0.05, 0.15, 0.1, 0.2, 0.1]],
        "B": [[T_, F_, T_, T_, F_, T_, F_, F_]],
    }},
    # A single value: one row in a group whose category has three items.
    "S": {"T": 3, "m": {"X": [[7, None, None]], "Y": [[2, None, None]]}},
    # Two groups (category G), one with a missing item.
    "G": {"T": 6, "G": 2, "m": {
        "X": [[10, 20, 20, 40, 50, 60], [5, 3, 8, None, 3, 1]],
        "Y": [[1.5, 2.5, 2, 4.5, 5, 6.5], [2, 4, 1, None, 6, 5]],
        "C": [["a", "b", "a", "c", "b", "a"], ["b", "b", "a", None, "c", "c"]],
    }},
    # Negatives and zero, a missing item, and a tie between modes.
    "N": {"T": 7, "m": {
        "X": [[2.5, 4, 0, -1, None, -1, 4]],
        "Y": [[1, 2, 3, 4, None, 6, 7]],
    }},
    # Errors among the values.
    "E": {"T": 5, "m": {"X": [[4, NA, 2, 8, {"error": "#DIV/0!"}]]}},
}

CASES = {name: [] for name in DATA}


def lo(ds, f, tmpl=None, excel=None):
    """A LibreOffice case; `tmpl` overrides the derived spreadsheet formula
    (`@X` is the item's own cell, `X` the group's range)."""
    CASES[ds].append(("lo", f, tmpl, excel))


# LibreOffice's ZTEST is 0.5 − Φ(z) with its own Gauss, up to ~2e-10
# relative from the tail ½·erfc(z/√2) (which Python's math.erfc confirms);
# these rows carry a tolerance.
TOL = {"ZTEST": 1e-9, "Z.TEST": 1e-9}


def item(ds, f, tmpl, excel=None):
    """A per-item LibreOffice case; `excel` overrides: one value for every
    item, or a function (ds, g, r) -> value."""
    CASES[ds].append(("item", f, tmpl, excel))


def hand(ds, f, fn, src):
    CASES[ds].append(("hand", f, fn, src))


def many(ds, *fs):
    for f in fs:
        lo(ds, f)


# --- hand definitions ---------------------------------------------------------
def cells(ds, m, g):
    return DATA[ds]["m"][m][g]


def present(ds, ms, g):
    """Rows where every measure has a cell (the engine joins them)."""
    cols = [cells(ds, m, g) for m in ms]
    return [r for r in zip(*cols) if all(v is not None for v in r)]


def is_num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def suma(ds, g):
    return math.fsum(float(v) if is_num(v) else float(v is True)
                     for v in cells(ds, "X", g) if v is not None)


def kurtp(ds, g):
    xs = [float(v) for v in cells(ds, "X", g) if is_num(v)]
    n = len(xs)
    m = math.fsum(xs) / n
    s = math.sqrt(math.fsum((x - m) ** 2 for x in xs) / n)
    if s == 0:
        return {"error": "#DIV/0!"}
    return math.fsum(((x - m) / s) ** 4 for x in xs) / n - 3


def percentof(ds, g):
    rows = present(ds, "XY", g)
    return math.fsum(r[0] for r in rows) / math.fsum(r[1] for r in rows)


def polynomial(x):
    """Σ cᵢ·xⁱ, i the item's position (a missing item is a zero coefficient,
    as LibreOffice's SERIESSUM reads a blank)."""
    return lambda ds, g: math.fsum(c * x ** i for i, c in enumerate(cells(ds, "X", g))
                                   if c is not None)


def autocorrel(m):
    return lambda ds, g: lag1([float(v) for v in cells(ds, m, g) if v is not None])


def lag1(xs):
    m = math.fsum(xs) / len(xs)
    q, v = 0.0, (xs[0] - m) ** 2
    for i in range(1, len(xs)):
        d0, d1 = xs[i - 1] - m, xs[i] - m
        q += (d0 * d1 - q) / (i + 1)
        v += (d1 * d1 - v) / (i + 1)
    return q / v if v else {"error": "#DIV/0!"}


def exp_fit(ds, g, simple):
    """ln y = a + b·x, weighted by y unless `simple`; (A, B) = (eᵃ, b)."""
    rows = [(float(x), float(y)) for y, x in present(ds, "YX", g)]
    w = [1.0 if simple else y for _, y in rows]
    sw = math.fsum(w)
    mx = math.fsum(wi * x for wi, (x, _) in zip(w, rows)) / sw
    ml = math.fsum(wi * math.log(y) for wi, (_, y) in zip(w, rows)) / sw
    sxx = math.fsum(wi * (x - mx) ** 2 for wi, (x, _) in zip(w, rows))
    b = math.fsum(wi * (x - mx) * (math.log(y) - ml) for wi, (x, y) in zip(w, rows)) / sxx
    return math.exp(ml - b * mx), b


def growth(ds, g):
    """GROWTH at each item's own x: the unweighted fit of ln y."""
    a, b = exp_fit(ds, g, True)
    xs, ys = cells(ds, "X", g), cells(ds, "Y", g)
    return [None if x is None or y is None else a * math.exp(b * x) for x, y in zip(xs, ys)]


GN = "Gnumeric definition (plugins/fn-stat)"
QX = "Quantrix (plan §5.12, assumed)"

def dense_rank(m, ascending=False):
    """SQL's dense_rank() OVER (ORDER BY m DESC) (ASC with `ascending`): one
    plus the number of distinct values ahead; no cell, no rank."""
    def at(ds, g):
        xs = cells(ds, m, g)
        vals = sorted({float(v) for v in xs if v is not None})
        ahead = (lambda x: sum(1 for v in vals if v < x)) if ascending else \
            (lambda x: sum(1 for v in vals if v > x))
        return [None if x is None else float(ahead(x) + 1) for x in xs]
    return at


def count_distinct(m):
    """SQL's count(DISTINCT m): numbers by value, text case-sensitively."""
    return lambda ds, g: float(len({v for v in cells(ds, m, g) if v is not None}))


SQL = "PostgreSQL dense_rank() / count(DISTINCT)"

# --- dataset A: one group, ties -----------------------------------------------
many("A",
     "COUNT(X OVER T)", "COUNTA(C OVER T)", "COUNTBLANK(X OVER T)", "AVERAGEA(B OVER T)",
     "AVERAGEA(C OVER T)", "AVEDEV(X OVER T)", "MAXA(B OVER T)", "MINA(B OVER T)",
     "MAXA(X OVER T)", "MINA(X OVER T)", "MEDIAN(X OVER T)", "MEDIAN(Y OVER T)",
     "MODE(X OVER T)", "MODE(Y OVER T)", "MODE.SNGL(Y OVER T)",
     "GEOMEAN(X OVER T)", "HARMEAN(X OVER T)", "TRIMMEAN(X OVER T, 0.3)",
     "TRIMMEAN(X OVER T, 0.25)", "TRIMMEAN(X OVER T, 0)", "TRIMMEAN(X OVER T, 1)",
     "DEVSQ(X OVER T)", "STDEV(X OVER T)", "STDEV.S(X OVER T)", "STDEVP(X OVER T)",
     "STDEV.P(X OVER T)", "STDEVA(B OVER T)", "STDEVPA(B OVER T)", "VAR(X OVER T)",
     "VAR.S(X OVER T)", "VARP(X OVER T)", "VAR.P(X OVER T)", "VARA(B OVER T)",
     "VARPA(B OVER T)", "VARA(C OVER T)", "KURT(X OVER T)", "KURT(Y OVER T)",
     "SKEW(X OVER T)", "SKEWP(X OVER T)", "SKEW(Y OVER T)",
     "LARGE(X OVER T, 1)", "LARGE(X OVER T, 3)", "LARGE(X OVER T, 8)", "SMALL(X OVER T, 2)", "SMALL(X OVER T, 3)", "SMALL(X OVER T, 2.5)",
     "PERCENTILE(X OVER T, 0.3)", "PERCENTILE(X OVER T, 0)", "PERCENTILE(X OVER T, 1)",
     "PERCENTILE(X OVER T, 1.1)", "PERCENTILE.INC(Y OVER T, 0.9)",
     "PERCENTILE.EXC(X OVER T, 0.3)",
     "QUARTILE(X OVER T, 1)", "QUARTILE(X OVER T, 3)", "QUARTILE(X OVER T, 4)",
     "QUARTILE(X OVER T, 5)", "QUARTILE.INC(Y OVER T, 2)", "QUARTILE.EXC(X OVER T, 1)",
     "QUARTILE.EXC(X OVER T, 3)", "QUARTILE.EXC(X OVER T, 0)",
     "PROB(X, P OVER T, 2, 5)", "PROB(X, P OVER T, 1)", "PROB(X, P OVER T, 7)",
     "PROB(X, P OVER T, 5, 2)",
     "SUBTOTAL(1, X OVER T)", "SUBTOTAL(2, X OVER T)", "SUBTOTAL(3, X OVER T)",
     "SUBTOTAL(4, X OVER T)", "SUBTOTAL(5, X OVER T)", "SUBTOTAL(6, X OVER T)",
     "SUBTOTAL(7, X OVER T)", "SUBTOTAL(8, X OVER T)", "SUBTOTAL(9, X OVER T)",
     "SUBTOTAL(10, X OVER T)", "SUBTOTAL(11, X OVER T)", "SUBTOTAL(109, X OVER T)",
     "AGGREGATE(1, 0, X OVER T)", "AGGREGATE(12, 0, X OVER T)", "AGGREGATE(13, 0, Y OVER T)",
     "AGGREGATE(14, 0, X OVER T, 2)", "AGGREGATE(15, 0, X OVER T, 2)",
     "AGGREGATE(16, 0, X OVER T, 0.3)", "AGGREGATE(17, 0, X OVER T, 1)",
     "AGGREGATE(18, 0, X OVER T, 0.3)", "AGGREGATE(19, 0, X OVER T, 3)",
     "AGGREGATE(7, 6, X OVER T)",
     "SUMPRODUCT(X, Y OVER T)", "SUMPRODUCT(X, Y, P OVER T)", "SUMPRODUCT(X OVER T)",
     "SUMX2MY2(X, Y OVER T)", "SUMX2PY2(X, Y OVER T)", "SUMXMY2(X, Y OVER T)",
     "SERIESSUM(0.5, 1, 2, X OVER T)", "SERIESSUM(2, 0, 1, X OVER T)",
     "CORREL(X, Y OVER T)", "PEARSON(X, Y OVER T)", "RSQ(Y, X OVER T)",
     "COVAR(X, Y OVER T)", "COVARIANCE.P(X, Y OVER T)", "COVARIANCE.S(X, Y OVER T)",
     "SLOPE(Y, X OVER T)", "INTERCEPT(Y, X OVER T)", "STEYX(Y, X OVER T)",
     "FORECAST(5, Y, X OVER T)", "FORECAST.LINEAR(-2, Y, X OVER T)",
     "CHITEST(X, Y OVER T)", "CHISQ.TEST(Y, X OVER T)",
     "FTEST(X, Y OVER T)", "F.TEST(Y, X OVER T)",
     "TTEST(X, Y OVER T, 1, 1)", "TTEST(X, Y OVER T, 2, 1)", "TTEST(X, Y OVER T, 2, 2)",
     "TTEST(X, Y OVER T, 1, 3)", "T.TEST(X, Y OVER T, 2, 3)", "TTEST(X, Y OVER T, 3, 1)",
     "TTEST(X, Y OVER T, 2, 4)",
     "ZTEST(X OVER T, 3)", "ZTEST(X OVER T, 5)", "Z.TEST(X OVER T, 3, 2)",
     "ZTEST(X OVER T, 3, 0)",
     # criteria
     "COUNTIF(X OVER T, \">2\")", "COUNTIF(X OVER T, 1)", "COUNTIF(X OVER T, \"<>1\")",
     "COUNTIF(X OVER T, \"=4\")", "COUNTIF(X OVER T, \"<=3\")", "COUNTIF(C OVER T, \"apple\")",
     "COUNTIF(C OVER T, \"a*\")", "COUNTIF(C OVER T, \"?anana\")",
     "COUNTIF(C OVER T, \"<>apple\")", "COUNTIF(C OVER T, \"<c\")", "COUNTIF(C OVER T, \"*an*\")",
     "COUNTIF(B OVER T, TRUE)", "COUNTIF(C OVER T, \"zz\")",
     "SUMIF(X OVER T, \">=3\")", "SUMIF(C OVER T, \"a*\", X)", "SUMIF(C OVER T, \"apple\", Y)",
     "SUMIF(X OVER T, \"<0\")",
     "AVERAGEIF(X OVER T, \"<5\")", "AVERAGEIF(C OVER T, \"*a*\", Y)",
     "AVERAGEIF(X OVER T, \">100\")",
     "SUMIFS(X, C OVER T, \"a*\", Y, \">1\")", "SUMIFS(Y, X OVER T, \">2\")",
     "COUNTIFS(C OVER T, \"b*\", X, \">3\")", "COUNTIFS(X OVER T, \">1\")",
     "COUNTIFS(C OVER T, \"*e*\", Y, \"<8\", X, \"<>4\")",
     "AVERAGEIFS(Y, X OVER T, \">2\")", "AVERAGEIFS(Y, C OVER T, \"apple\", X, 3)",
     "AVERAGEIFS(Y, X OVER T, \">20\")",
     "MAXIFS(X, C OVER T, \"a*\")", "MINIFS(X, C OVER T, \"a*\")", "MAXIFS(X, C OVER T, \"zzz\")",
     "MINIFS(Y, X OVER T, \">1\", C, \"<>apple\")")
# Where LibreOffice disagrees with Excel's documentation (the Excel value is kept):
# LibreOffice's booleans are numbers, so its COUNT counts them;
lo("A", "COUNT(B OVER T)", excel=[0])
# "If k <= 0 or if k is greater than the number of data points, LARGE returns #NUM!";
for k in ("9", "0"):
    lo("A", f"LARGE(X OVER T, {k})", excel=[NUM])
# PERCENTILE.EXC: #NUM! where k cannot be interpolated (LibreOffice: #VALUE!);
for k in ("0.1", "0.95"):
    lo("A", f"PERCENTILE.EXC(X OVER T, {k})", excel=[NUM])
# SUBTOTAL: an unknown function number is #VALUE!;
lo("A", "SUBTOTAL(12, X OVER T)", excel=[VAL])
# PERCENTRANK: an x outside the values is #N/A.
item("A", "PERCENTRANK(X OVER T, 10)", "PERCENTRANK(X;10)", excel=NA)


def percentrank(m, inc, v=None, digits=3):
    """Excel's PERCENTRANK(.INC/.EXC), truncated to `digits` (the Excel
    examples: PERCENTRANK.INC({13,12,11,8,4,3,2,1,1,1}, 4) = 0.555, where
    LibreOffice rounds to 0.556)."""
    def at(ds, g, r):
        xs = sorted(x for x in cells(ds, m, g) if x is not None)
        x = cells(ds, m, g)[r] if v is None else v
        n, smaller = len(xs), sum(1 for y in xs if y < x)
        if n == 1:
            return 1.0
        pos = smaller if xs[smaller] == x else \
            smaller - 1 + (x - xs[smaller - 1]) / (xs[smaller] - xs[smaller - 1])
        frac = pos / (n - 1) if inc else (pos + 1) / (n + 1)
        scaled = frac * 10 ** digits
        if abs(scaled - round(scaled)) <= 4 * 2.220446049250313e-16 * abs(scaled):
            scaled = round(scaled)
        return math.floor(scaled) / 10 ** digits
    return at

lo("A", "AGGREGATE(14, 0, X OVER T)", excel=[{"error": "#VALUE!"}])
hand("A", "SUMA(X OVER T)", suma, GN)
hand("A", "SUMA(B OVER T)", lambda ds, g: 4.0, GN)
hand("A", "KURTP(X OVER T)", kurtp, GN)
hand("A", "PERCENTOF(X, Y OVER T)", percentof, "Excel docs")
hand("A", "POLYNOMIAL(0.5, X OVER T)", polynomial(0.5), "Numbers (plan §5.4)")
hand("A", "POLYNOMIAL(-2, X OVER T)", polynomial(-2.0), "Numbers (plan §5.4)")
hand("A", "AUTOCORREL(X OVER T)", autocorrel("X"), "GSL definition")
hand("A", "AUTOCORREL(Y OVER T)", autocorrel("Y"), "GSL definition")
hand("A", "EXPA(Y, X OVER T)", lambda ds, g: exp_fit(ds, g, False)[0], QX)
hand("A", "EXPB(Y, X OVER T)", lambda ds, g: exp_fit(ds, g, False)[1], QX)
hand("A", "EXPA(Y, X OVER T, TRUE)", lambda ds, g: exp_fit(ds, g, True)[0], QX)
hand("A", "EXPB(Y, X OVER T, TRUE)", lambda ds, g: exp_fit(ds, g, True)[1], QX)
hand("A", "EXPFORECAST(4.5, Y, X OVER T)",
     lambda ds, g: (lambda a, b: a * math.exp(b * 4.5))(*exp_fit(ds, g, False)), QX)
hand("A", "EXPFORECAST(4.5, Y, X OVER T, TRUE)",
     lambda ds, g: (lambda a, b: a * math.exp(b * 4.5))(*exp_fit(ds, g, True)), QX)
# EXPA with simple = TRUE is LOGEST's fit: its A is LibreOffice's GROWTH at x = 0.
lo("A", "EXPA(Y, X OVER T, TRUE)", "GROWTH(Y;X;0)")
lo("A", "EXPFORECAST(4.5, Y, X OVER T, TRUE)", "GROWTH(Y;X;4.5)")
for f, t in [("RANK(X OVER T)", "RANK(@X;X)"), ("RANK(X OVER T, 1)", "RANK(@X;X;1)"),
             ("RANK.EQ(Y OVER T)", "RANK(@Y;Y)"), ("RANK(X OVER T, 0)", "RANK(@X;X;0)"),
             ("RANK.AVG(X OVER T)", "COM.MICROSOFT.RANK.AVG(@X;X)"),
             ("RANK.AVG(Y OVER T, 1)", "COM.MICROSOFT.RANK.AVG(@Y;Y;1)"),
             ("TREND(Y, X OVER T)", "TREND(Y;X;@X)"),
             ("TREND(Y, X OVER T, FALSE)", "TREND(Y;X;@X;FALSE())"),
             ("GROWTH(Y, X OVER T)", "GROWTH(Y;X;@X)"),
             ("GROWTH(Y, X OVER T, FALSE)", "GROWTH(Y;X;@X;FALSE())")]:
    item("A", f, t)

for f, t, ex in [
        ("PERCENTRANK(X OVER T)", "PERCENTRANK(X;@X)", percentrank("X", True)),
        ("PERCENTRANK.INC(Y OVER T)", "PERCENTRANK(Y;@Y)", percentrank("Y", True)),
        ("PERCENTRANK(X OVER T, 2.5)", "PERCENTRANK(X;2.5)", percentrank("X", True, 2.5)),
        ("PERCENTRANK(X OVER T, 2.5, 2)", "PERCENTRANK(X;2.5;2)", percentrank("X", True, 2.5, 2)),
        ("PERCENTRANK(X OVER T, 7.3, 5)", "PERCENTRANK(X;7.3;5)", percentrank("X", True, 7.3, 5)),
        ("PERCENTRANK.EXC(X OVER T)", "COM.MICROSOFT.PERCENTRANK.EXC(X;@X)",
         percentrank("X", False)),
        ("PERCENTRANK.EXC(X OVER T, 4.5, 4)", "COM.MICROSOFT.PERCENTRANK.EXC(X;4.5;4)",
         percentrank("X", False, 4.5, 4))]:
    item("A", f, t, ex)

for f, fn in [("RANK.DENSE(X OVER T)", dense_rank("X")),
              ("RANK.DENSE(Y OVER T)", dense_rank("Y")),
              ("RANK.DENSE(Y OVER T, 1)", dense_rank("Y", True)),
              ("COUNTUNIQUE(X OVER T)", count_distinct("X")),
              ("COUNTUNIQUE(Y OVER T)", count_distinct("Y")),
              ("COUNTUNIQUE(C OVER T)", count_distinct("C"))]:
    hand("A", f, fn, SQL)

# --- dataset S: a single value -------------------------------------------------
many("S",
     "COUNT(X OVER T)", "COUNTA(X OVER T)", "COUNTBLANK(X OVER T)", "AVERAGEA(X OVER T)",
     "AVEDEV(X OVER T)", "MEDIAN(X OVER T)", "GEOMEAN(X OVER T)",
     "HARMEAN(X OVER T)", "TRIMMEAN(X OVER T, 0.5)", "DEVSQ(X OVER T)", "STDEV(X OVER T)",
     "STDEVP(X OVER T)", "VAR(X OVER T)", "VARP(X OVER T)", "VARPA(X OVER T)",
     "KURT(X OVER T)", "SKEW(X OVER T)", "SKEWP(X OVER T)", "LARGE(X OVER T, 1)",
     "PERCENTILE(X OVER T, 0.4)", "PERCENTILE.EXC(X OVER T, 0.5)",
     "QUARTILE(X OVER T, 3)", "SUBTOTAL(7, X OVER T)", "SUBTOTAL(8, X OVER T)",
     "CORREL(X, Y OVER T)", "COVAR(X, Y OVER T)",
     "SLOPE(Y, X OVER T)", "SUMPRODUCT(X, Y OVER T)",
     "SERIESSUM(3, 2, 1, X OVER T)", "ZTEST(X OVER T, 3, 2)", "ZTEST(X OVER T, 3)",
     "COUNTIF(X OVER T, 7)", "SUMIF(X OVER T, \">5\")")
# "If the data set contains no duplicate data points, MODE returns #N/A";
lo("S", "MODE(X OVER T)", excel=[NA])
lo("S", "SMALL(X OVER T, 2)", excel=[NUM])
# COVARIANCE.S and STEYX of a single point: #DIV/0!.
lo("S", "COVARIANCE.S(X, Y OVER T)", excel=[DIV0])
lo("S", "STEYX(Y, X OVER T)", excel=[DIV0])
hand("S", "KURTP(X OVER T)", kurtp, GN)
hand("S", "AUTOCORREL(X OVER T)", autocorrel("X"), "GSL definition")
for f, t in [("RANK(X OVER T)", "RANK(@X;X)"), ("RANK.AVG(X OVER T)", "COM.MICROSOFT.RANK.AVG(@X;X)"),
             ("PERCENTRANK(X OVER T)", "PERCENTRANK(X;@X)"),
             ("PERCENTRANK.EXC(X OVER T)", "COM.MICROSOFT.PERCENTRANK.EXC(X;@X)")]:
    item("S", f, t)

# --- dataset G: two groups ----------------------------------------------------
many("G",
     "COUNT(X OVER T)", "COUNTBLANK(X OVER T)", "AVERAGEA(X OVER T)", "MEDIAN(X OVER T)",
     "MODE(X OVER T)", "STDEV(X OVER T)", "VARP(Y OVER T)", "KURT(X OVER T)",
     "SKEW(Y OVER T)", "LARGE(X OVER T, 2)", "SMALL(X OVER T, 2)",
     "PERCENTILE(X OVER T, 0.35)", "QUARTILE.EXC(Y OVER T, 1)", "TRIMMEAN(X OVER T, 0.4)",
     "SUBTOTAL(9, X OVER T)", "AGGREGATE(16, 6, Y OVER T, 0.8)",
     "CORREL(X, Y OVER T)", "SLOPE(Y, X OVER T)", "INTERCEPT(Y, X OVER T)",
     "FORECAST(30, Y, X OVER T)", "STEYX(Y, X OVER T)", "SUMXMY2(X, Y OVER T)",
     "TTEST(X, Y OVER T, 2, 1)", "TTEST(X, Y OVER T, 2, 2)", "TTEST(X, Y OVER T, 1, 3)",
     "FTEST(X, Y OVER T)", "ZTEST(Y OVER T, 3)",
     "COUNTIF(C OVER T, \"a\")", "SUMIF(C OVER T, \"b\", X)", "AVERAGEIF(C OVER T, \"<>a\", Y)",
     "SUMIFS(Y, C OVER T, \"a\", X, \">3\")", "COUNTIFS(C OVER T, \"?\", Y, \">=2\")",
     "MAXIFS(Y, C OVER T, \"b\")", "MINIFS(Y, C OVER T, \"c\")")
hand("G", "AUTOCORREL(X OVER T)", autocorrel("X"), "GSL definition")
item("G", "PERCENTRANK(Y OVER T)", "PERCENTRANK(Y;@Y)", percentrank("Y", True))
hand("G", "EXPB(Y, X OVER T)", lambda ds, g: exp_fit(ds, g, False)[1], QX)
for f, t in [("RANK(X OVER T)", "RANK(@X;X)"), ("RANK.AVG(X OVER T, 1)", "COM.MICROSOFT.RANK.AVG(@X;X;1)"),
             ("TREND(Y, X OVER T)", "FORECAST(@X;Y;X)")]:
    item("G", f, t)
for f, fn in [("RANK.DENSE(X OVER T)", dense_rank("X")),
              ("RANK.DENSE(X OVER T, 1)", dense_rank("X", True)),
              ("COUNTUNIQUE(X OVER T)", count_distinct("X")),
              ("COUNTUNIQUE(C OVER T)", count_distinct("C"))]:
    hand("G", f, fn, SQL)
hand("G", "GROWTH(Y, X OVER T)", growth, "least squares of ln y (LibreOffice's GROWTH rejects blanks)")

# --- dataset N: negatives, zero, tied modes -------------------------------------
many("N",
     "COUNT(X OVER T)", "COUNTBLANK(X OVER T)", "MEDIAN(X OVER T)", "AVEDEV(X OVER T)",
     "HARMEAN(X OVER T)", "DEVSQ(X OVER T)", "STDEVA(X OVER T)",
     "KURT(X OVER T)", "SKEW(X OVER T)", "SKEWP(X OVER T)", "MAXA(X OVER T)",
     "MINA(X OVER T)", "PERCENTILE.EXC(X OVER T, 0.5)", "QUARTILE(X OVER T, 1)",
     "SUBTOTAL(6, X OVER T)", "SUBTOTAL(106, X OVER T)", "AGGREGATE(6, 0, X OVER T)",
     "SUMPRODUCT(X, Y OVER T)", "SUMX2MY2(X, Y OVER T)", "CORREL(X, Y OVER T)",
     "RSQ(Y, X OVER T)", "SERIESSUM(-1.5, 0, 2, X OVER T)",
     "COUNTIF(X OVER T, \"<0\")", "COUNTIF(X OVER T, \">=-1\")", "COUNTIF(X OVER T, 0)",
     "SUMIF(X OVER T, \"<>4\")", "AVERAGEIF(X OVER T, \"<=0\", Y)")
# "If any data point <= 0, GEOMEAN returns #NUM!" (LibreOffice: 0);
lo("N", "GEOMEAN(X OVER T)", excel=[NUM])
# two modes: Excel gives the first in data order, LibreOffice the smallest.
lo("N", "MODE(X OVER T)", excel=[4])
hand("N", "KURTP(X OVER T)", kurtp, GN)
hand("N", "AUTOCORREL(X OVER T)", autocorrel("X"), "GSL definition")
item("N", "PERCENTRANK(X OVER T)", "PERCENTRANK(X;@X)", percentrank("X", True))
item("N", "PERCENTRANK.EXC(X OVER T)", "COM.MICROSOFT.PERCENTRANK.EXC(X;@X)",
     percentrank("X", False))
hand("N", "POLYNOMIAL(3, X OVER T)", polynomial(3.0), "Numbers (plan §5.4)")
hand("N", "RANK.DENSE(X OVER T)", dense_rank("X"), SQL)
hand("N", "RANK.DENSE(X OVER T, 1)", dense_rank("X", True), SQL)
hand("N", "COUNTUNIQUE(X OVER T)", count_distinct("X"), SQL)
for f, t in [("RANK(X OVER T)", "RANK(@X;X)"), ("RANK.AVG(X OVER T)", "COM.MICROSOFT.RANK.AVG(@X;X)"),
             ("RANK.AVG(X OVER T, 1)", "COM.MICROSOFT.RANK.AVG(@X;X;1)"),
             ("TREND(Y, X OVER T)", "FORECAST(@X;Y;X)")]:
    item("N", f, t)

# --- dataset E: errors among the values ---------------------------------------
many("E",
     "COUNT(X OVER T)", "COUNTA(X OVER T)", "STDEV(X OVER T)", "MAXA(X OVER T)",
     "SUBTOTAL(9, X OVER T)", "AGGREGATE(9, 6, X OVER T)",
     "AGGREGATE(14, 6, X OVER T, 2)", "AGGREGATE(1, 7, X OVER T)",
     "AGGREGATE(2, 2, X OVER T)", "AGGREGATE(3, 3, X OVER T)",
     "COUNTIF(X OVER T, \">1\")", "SUMIF(X OVER T, \">3\")")

LO_NAMES = {n: "COM.MICROSOFT." + n for n in (
    "MAXIFS", "MINIFS", "PERCENTILE.EXC", "PERCENTILE.INC", "QUARTILE.EXC", "QUARTILE.INC",
    "COVARIANCE.S", "COVARIANCE.P", "AGGREGATE", "FORECAST.LINEAR", "STDEV.S", "STDEV.P",
    "VAR.S", "VAR.P", "MODE.SNGL", "CHISQ.TEST", "F.TEST", "T.TEST", "Z.TEST")}
LO_NAMES["CHITEST"] = "LEGACY.CHITEST"


def col_name(i):
    return chr(ord("B") + i)


def layout(ds):
    """(measure, group) -> column letter on the dataset's sheet."""
    cols, i = {}, 0
    for m, groups in DATA[ds]["m"].items():
        for g in range(len(groups)):
            cols[(m, g)] = col_name(i)
            i += 1
    return cols


def sheet(ds):
    d, cols = DATA[ds], layout(ds)
    out = [f'<table:table table:name="D{ds}">']
    for r in range(d["T"]):
        row = ["<table:table-cell/>"]  # column A stays empty (the runner skips the row)
        for (m, g), _ in cols.items():
            v = d["m"][m][g][r]
            if v is None:
                row.append("<table:table-cell/>")
            elif isinstance(v, dict):
                fn = {"#N/A": "NA()", "#DIV/0!": "1/0"}[v["error"]]
                row.append(f'<table:table-cell table:formula="of:={fn}"/>')
            elif isinstance(v, bool):
                row.append('<table:table-cell office:value-type="boolean" '
                           f'office:boolean-value="{str(v).lower()}"/>')
            elif isinstance(v, str):
                row.append('<table:table-cell office:value-type="string">'
                           f"<text:p>{escape(v)}</text:p></table:table-cell>")
            else:
                row.append(f'<table:table-cell office:value-type="float" office:value="{v!r}"/>')
        out.append("<table:table-row>" + "".join(row) + "</table:table-row>")
    out.append("</table:table>")
    return "\n".join(out)


def to_sheet(f, ds, g, r=None):
    """Spreadsheet formula `f` (ODFF, measure names X…) over group g; `@X` is
    row r's own cell."""
    cols, n = layout(ds), DATA[ds]["T"]
    parts = re.split(r'("[^"]*")', f)
    for k in range(0, len(parts), 2):
        parts[k] = re.sub(r"@([A-Z])\b", lambda m: f"[$D{ds}.{cols[(m.group(1), g)]}{r + 1}]", parts[k])
        parts[k] = re.sub(r"(?<![.\w$@])([A-Z])\b(?![.(])",
                          lambda m: f"[$D{ds}.{cols[(m.group(1), g)]}1:.{cols[(m.group(1), g)]}{n}]",
                          parts[k])
    return "".join(parts)


p1.LO_NAMES = LO_NAMES


def derive(f):
    """Improv `F(a, b OVER T, k)` -> ODFF `F(a;b;k)`."""
    return p1.to_lo(f.replace(" OVER T", ""))


def map_error(f, shown):
    if shown in p1.LO_ERRORS:
        return {"error": shown}
    if shown in ("Err:502", "Err:523"):
        return {"error": "#NUM!"}
    if shown == "Err:504":
        return {"error": "#VALUE!"}
    return {"error": shown, "unmapped": True}


def groups(ds):
    return range(DATA[ds].get("G", 1))


def own_cell(ds, f, g, r):
    """Whether a per-item row exists at item r: every reduced measure has a cell."""
    ms = re.findall(r"(?<![.\w])([A-Z])\b(?!\()", f.split("(", 1)[1].split(" OVER")[0] + " ")
    return all(DATA[ds]["m"][m][g][r] is not None for m in ms)


def main():
    formulas, slots = [], []  # slots: (ds, case index, group, item or None)
    for ds, cases in CASES.items():
        for ci, (kind, f, tmpl, _) in enumerate(cases):
            if kind == "hand":
                continue
            for g in groups(ds):
                if kind == "lo":
                    formulas.append(to_sheet(tmpl or derive(f), ds, g))
                    slots.append((ds, ci, g, None))
                    continue
                for r in range(DATA[ds]["T"]):
                    if own_cell(ds, f, g, r):
                        formulas.append(to_sheet(tmpl, ds, g, r))
                        slots.append((ds, ci, g, r))
    p1.TAIL = ("</table:table>" + "".join(sheet(ds) for ds in DATA)
               + "</office:spreadsheet></office:body></office:document>\n")
    vals = p1.run_lo(["PI()"] + formulas)
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    got = {}
    for (ds, ci, g, r), v in zip(slots, vals[1:]):
        f = CASES[ds][ci][1]
        if isinstance(v, tuple):
            v = map_error(f, v[1])
        got[(ds, ci, g, r)] = v
    print("formula\texpected\tsource")
    for ds, cases in CASES.items():
        print(f"@{ds}\t{json.dumps(DATA[ds], ensure_ascii=False)}")
        for ci, (kind, f, extra, excel) in enumerate(cases):
            exp, src = [], "LibreOffice"
            for g in groups(ds):
                if kind == "hand":
                    exp.append(extra(ds, g))
                    src = excel
                elif kind == "lo":
                    exp.append(got[(ds, ci, g, None)])
                else:
                    exp.append([got.get((ds, ci, g, r)) for r in range(DATA[ds]["T"])])
            if src == "LibreOffice" and excel is None and "unmapped" in json.dumps(exp):
                raise SystemExit(f"unmapped LibreOffice error for {ds} {f}: {exp}")
            if kind == "item" and excel is not None:
                pick = excel if callable(excel) else (lambda ds, g, r: excel)
                new = [[None if v is None else pick(ds, g, r) for r, v in enumerate(grp)]
                       for g, grp in enumerate(exp)]
                if new != exp:
                    print(f"LibreOffice differs: {ds} {f} LO={json.dumps(exp)} "
                          f"Excel={json.dumps(new)}", file=sys.stderr)
                    exp, src = new, "Excel docs"
            elif kind == "lo" and excel is not None:
                print(f"LibreOffice differs: {ds} {f} LO={json.dumps(exp)} Excel={json.dumps(excel)}",
                      file=sys.stderr)
                exp, src = excel, "Excel docs"
            tol = TOL.get(p1.outer(f))
            if tol:
                exp = [{"approx": v, "tol": tol} if isinstance(v, float) else v for v in exp]
            assert "\t" not in f and "^" not in f, f
            print(f"{ds}\t{f}\t{json.dumps(exp, ensure_ascii=False)}\t{src}")


if __name__ == "__main__":
    main()
