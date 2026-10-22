#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/array_reduce_goldens.tsv (the P6 array
functions: list results laid along an INTO category).

    python3 scripts/goldens/array_reduce.py \\
        > crates/engine/tests/data/array_reduce_goldens.tsv

Each dataset (DATA) is a LibreOffice sheet: one column per measure and group
(category G), one row per item of T. An Improv formula such as
`SORTBY(X, Y OVER T, -1 INTO K)` becomes, per group, the spreadsheet
formula over the group's columns, `COM.MICROSOFT.SORTBY($DA.B1:.B8; ...)`.
A formula without OVER (`TEXTSPLIT(C, "," INTO K)`) is one group per item of
T, over the item's own cell. The list is read back element by element:
`ROWS(TOCOL(F))` gives its length and `INDEX(TOCOL(F); k)` its k-th value
(row by row, as LINEST's 5×2 block flattens); when F is a single value or an
error (TOCOL refuses it), the cell `F` is that value. The formulas run
through p1.py's runner (its exact-double reconstruction).

The TSV has two kinds of line:
    @name <TAB> {"T": items, "G": groups, "K": items of K, "m": {"X": [[group 0], …]}}
    name <TAB> improv formula <TAB> expected <TAB> source
where expected has, per group, a list with one entry per item of K: the
value landing there, or null (no cell: the list is shorter than K). A list
longer than K is cut to K.

Excel's empty array (#CALC!) is #N/A in Improv. Where LibreOffice
disagrees with Excel's documentation the Excel value is kept (source `Excel
docs`): LibreOffice's empty array (Err:533) is #N/A; DROP of every value (LibreOffice returns them all), MODE.MULT with
no repeated value (LibreOffice: #VALUE!), LINEST of a constant x (Excel
drops the collinear x: m = 0, b the mean of y; LibreOffice: #VALUE!),
SORTBY of an error (LibreOffice: 0) and a trailing empty TEXTSPLIT part
(LibreOffice drops it).
"""

import json
import os
import re
import sys
from xml.sax.saxutils import escape

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402  (the LibreOffice runner)

T_, F_ = True, False
NA = {"error": "#N/A"}
DIV0 = {"error": "#DIV/0!"}

DATA = {
    # One group; ties in X (1, 1) and in C up to case; K shorter than some
    # lists (SORTBY, LINEST's stats) and longer than others.
    "A": {"T": 8, "K": 6, "m": {
        "X": [[3, 1, 4, 1, 5, 9, 2, 6]],
        "Y": [[2, 7, 1, 8, 2, 8, 1.5, 8]],
        "W": [[3, 1, 3, 1, 2, 2, 5, 7]],
        "U": [[2, 2, 2, 2, 2, 2, 2, 2]],
        "B": [[T_, F_, T_, T_, F_, T_, F_, F_]],
        "C": [["pear", "Apple", "fig", "apple", "kiwi", "Fig", "date", "plum"]],
    }},
    # LINEST's whole stats block (K holds all 10 values and two more).
    "L": {"T": 5, "K": 12, "m": {
        "X": [[1, 2, 3, 4, 6]],
        "Y": [[1, 3, 2, 5, 7.5]],
    }},
    # Two groups (category G); nothing included in the second.
    "G": {"T": 5, "G": 2, "K": 4, "m": {
        "X": [[5, 3, 8, 3, 1], [2, 2, 7, 7, 4]],
        "Y": [[1.5, 2.5, 2, 4.5, 5], [2, 4, 1, 3, 6]],
        "B": [[T_, T_, F_, T_, F_], [F_, F_, F_, F_, F_]],
    }},
    # Errors among the values, and in an include.
    "E": {"T": 5, "K": 5, "m": {
        "X": [[4, NA, 2, 8, DIV0]],
        "Y": [[1, 2, 3, 4, 5]],
        "B": [[T_, T_, F_, T_, F_]],
        "Q": [[T_, NA, T_, F_, T_]],
    }},
    # TEXTSPLIT, one cell at a time (no OVER).
    "S": {"T": 7, "K": 3, "m": {
        "C": [["a,b,,c", "one", ",x,", "aXbxc", "p, q, r", "", ",,"]],
    }},
}

CASES = {name: [] for name in DATA}


def lo(ds, f, excel=None):
    """A LibreOffice case; `excel` overrides it with one list per group."""
    CASES[ds].append((f, excel))


def many(ds, *fs):
    for f in fs:
        lo(ds, f)


many("A",
     "FILTER(X, B OVER T INTO K)", "FILTER(C, B OVER T INTO K)",
     "FILTER(X, B OVER T, -1 INTO K)",
     "SORTBY(C, X OVER T INTO K)", "SORTBY(X, C OVER T, -1 INTO K)",
     "SORTBY(C, Y OVER T, 1 INTO K)", "SORTBY(X, Y OVER T, 2 INTO K)",
     "TAKE(X OVER T, 3 INTO K)", "TAKE(X OVER T, -2 INTO K)", "TAKE(C OVER T, 2.7 INTO K)",
     "TAKE(X OVER T, 20 INTO K)", "TAKE(X OVER T, 0 INTO K)",
     "DROP(X OVER T, 3 INTO K)", "DROP(C OVER T, -5 INTO K)", "DROP(X OVER T, 0 INTO K)",
     "DROP(X OVER T, -7 INTO K)",
     "MODE.MULT(X OVER T INTO K)", "MODE.MULT(Y OVER T INTO K)", "MODE.MULT(W OVER T INTO K)",
     "LINEST(Y, X OVER T INTO K)", "LINEST(Y, X OVER T, TRUE, TRUE INTO K)",
     "LINEST(Y, X OVER T, FALSE INTO K)", "LINEST(U, X OVER T, TRUE, TRUE INTO K)",
     "LOGEST(Y, X OVER T INTO K)", "LOGEST(Y, X OVER T, TRUE, TRUE INTO K)",
     "LOGEST(Y, X OVER T, FALSE, TRUE INTO K)",
     "FREQUENCY(X OVER T, 2, 5 INTO K)", "FREQUENCY(X OVER T, 5, 2 INTO K)",
     "FREQUENCY(X OVER T, 1, 1, 4 INTO K)", "FREQUENCY(Y OVER T, 100 INTO K)",
     "FREQUENCY(X OVER T, 0.5, 1.5, 2.5, 3.5, 4.5, 5.5 INTO K)")
# UNIQUE ignores case and keeps the first spelling (Excel's docs: "not case
# sensitive"; LibreOffice and Gnumeric agree).
many("A", "UNIQUE(C OVER T INTO K)", "UNIQUE(C OVER T, TRUE INTO K)")
# "Excel returns a #CALC! error to indicate an empty array" (DROP);
# LibreOffice drops nothing.
for k in ("8", "-9"):
    lo("A", f"DROP(X OVER T, {k} INTO K)", excel=[[NA] + [None] * 5])
# "LINEST checks for collinearity and removes any redundant X columns";
# the constant term then fits alone (the doc's example: y 0, x 1 gives 0).
lo("A", "LINEST(Y, U OVER T INTO K)", excel=[[0, 4.6875] + [None] * 4])

# "If the data set contains no duplicate data points, MODE.MULT returns #N/A".
lo("L", "MODE.MULT(X OVER T INTO K)", excel=[[NA] + [None] * 11])
many("L",
     "LINEST(Y, X OVER T, TRUE, TRUE INTO K)", "LINEST(Y, X OVER T, FALSE, TRUE INTO K)",
     "LOGEST(Y, X OVER T, TRUE, TRUE INTO K)", "LOGEST(Y, X OVER T, FALSE, TRUE INTO K)",
     "LINEST(Y, X OVER T, TRUE, FALSE INTO K)")

many("G",
     "FILTER(X, B OVER T INTO K)", "FILTER(Y, B OVER T, 0 INTO K)",
     "SORTBY(X, Y OVER T, -1 INTO K)", "SORTBY(Y, X OVER T INTO K)",
     "TAKE(X OVER T, -3 INTO K)", "DROP(X OVER T, 1 INTO K)", "MODE.MULT(X OVER T INTO K)",
     "LINEST(Y, X OVER T, TRUE, TRUE INTO K)", "LOGEST(Y, X OVER T INTO K)",
     "FREQUENCY(X OVER T, 3, 6 INTO K)")

# SORTBY moves the array's values as they are, errors too (LibreOffice
# turns them into 0).
lo("E", "SORTBY(X, Y OVER T, -1 INTO K)", excel=[[DIV0, 8, 2, NA, 4]])
many("E",
     "FILTER(X, B OVER T INTO K)", "FILTER(Y, Q OVER T INTO K)",
     "TAKE(X OVER T, 2 INTO K)", "DROP(X OVER T, 2 INTO K)",
     "MODE.MULT(X OVER T INTO K)", "LINEST(Y, X OVER T INTO K)",
     "FREQUENCY(X OVER T, 3 INTO K)")

# "ignore_empty ... Defaults to FALSE, which creates an empty cell": a
# trailing delimiter too (LibreOffice drops the last empty part).
lo("S", "TEXTSPLIT(C, \",\" INTO K)", excel=[
    ["a", "b", ""], ["one", None, None], ["", "x", ""], ["aXbxc", None, None], ["p", " q", " r"],
    [{"error": "#VALUE!"}, None, None], ["", "", ""]])
many("S",
     "TEXTSPLIT(C, \",\", \"\", TRUE INTO K)",
     "TEXTSPLIT(C, \"x\" INTO K)", "TEXTSPLIT(C, \", \" INTO K)", "TEXTSPLIT(C, \"\" INTO K)",
     "TEXTSPLIT(C, \"b\", \"\", FALSE INTO K)")


M = "COM.MICROSOFT."
p1.LO_NAMES = {n: M + n for n in ("FILTER", "SORTBY", "TAKE", "DROP", "TEXTSPLIT", "MODE.MULT",
                                   "UNIQUE")}


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
        for (m, g) in cols:
            v = d["m"][m][g][r]
            if isinstance(v, dict):
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


def per_cell(f):
    """A formula without OVER: one group per item of T, over its own cell."""
    return " OVER " not in f


def groups(ds, f):
    return range(DATA[ds]["T"] if per_cell(f) else DATA[ds].get("G", 1))


def to_sheet(f, ds, g):
    """Improv `F(a, b OVER T, k INTO K)` -> ODFF over group g's columns (or,
    without OVER, item g's own cells)."""
    f0 = f.replace(" OVER T", "").replace(" INTO K", "")
    # FREQUENCY's bins, numbers in Improv, are a column in the spreadsheet.
    f0 = re.sub(r"^FREQUENCY\((\w), (.*)\)$",
                lambda m: f"FREQUENCY({m.group(1)}, {{{m.group(2).replace(', ', '|')}}})", f0)
    lo_f = p1.to_lo(f0)
    cols, n = layout(ds), DATA[ds]["T"]
    parts = re.split(r'("[^"]*")', lo_f)
    for k in range(0, len(parts), 2):
        if per_cell(f):
            ref = lambda m: f"[$D{ds}.{cols[(m.group(1), 0)]}{g + 1}]"  # noqa: E731
        else:
            ref = lambda m: (f"[$D{ds}.{cols[(m.group(1), g)]}1:"  # noqa: E731
                             f".{cols[(m.group(1), g)]}{n}]")
        parts[k] = re.sub(r"(?<![.\w$])([A-Z])\b(?![.(])", ref, parts[k])
    return "".join(parts)


def value(v):
    if isinstance(v, tuple):
        shown = v[1]
        if shown in p1.LO_ERRORS:
            return {"error": shown}
        if shown == "Err:504":
            return {"error": "#VALUE!"}
        if shown == "Err:533":  # LibreOffice's empty array: Excel's #CALC!
            return {"error": "#N/A"}
        return {"error": shown, "unmapped": True}
    return v


def main():
    formulas, slots = [], []  # slots: (ds, case, group)
    for ds, cases in CASES.items():
        k_n = DATA[ds]["K"]
        for ci, (f, _) in enumerate(cases):
            for g in groups(ds, f):
                lf = to_sheet(f, ds, g)
                formulas.append(f"ROWS({M}TOCOL({lf}))")
                formulas.append(lf)
                formulas += [f"INDEX({M}TOCOL({lf});{k})" for k in range(1, k_n + 1)]
                slots.append((ds, ci, g))
    p1.TAIL = ("</table:table>" + "".join(sheet(ds) for ds in DATA)
               + "</office:spreadsheet></office:body></office:document>\n")
    vals = p1.run_lo(["PI()"] + formulas)
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    got, at = {}, 1
    for ds, ci, g in slots:
        k_n = DATA[ds]["K"]
        rows, single, elems = vals[at], vals[at + 1], vals[at + 2: at + 2 + k_n]
        at += 2 + k_n
        if isinstance(rows, float):
            n = min(int(rows), k_n)
            lst = [value(v) for v in elems[:n]] + [None] * (k_n - n)
        else:
            lst = [value(single)] + [None] * (k_n - 1)
        got[(ds, ci, g)] = lst
    print("formula\texpected\tsource")
    for ds, cases in CASES.items():
        print(f"@{ds}\t{json.dumps(DATA[ds], ensure_ascii=False)}")
        for ci, (f, excel) in enumerate(cases):
            exp = [got[(ds, ci, g)] for g in groups(ds, f)]
            src = "LibreOffice"
            if excel is not None:
                print(f"LibreOffice differs: {ds} {f} LO={json.dumps(exp)} "
                      f"Excel={json.dumps(excel)}", file=sys.stderr)
                exp, src = excel, "Excel docs"
            elif "unmapped" in json.dumps(exp):
                raise SystemExit(f"unmapped LibreOffice error for {ds} {f}: {exp}")
            assert "\t" not in f and "^" not in f, f
            print(f"{ds}\t{f}\t{json.dumps(exp, ensure_ascii=False)}\t{src}")


if __name__ == "__main__":
    main()
