#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/lookup_reduce_goldens.tsv (the P6
lookups, the Quantrix SELECT family, the database functions and
ARRAYTOTEXT).

    python3 scripts/goldens/lookup_reduce.py \\
        > crates/engine/tests/data/lookup_reduce_goldens.tsv

Each dataset (DATA) is a LibreOffice sheet: a header row of measure names,
then one row per item of category T, one column per table measure and group
(category G); a blank cell is an item with no cell. Measures over category E
(the lookup keys, `Q`, `P`, `I`) get a column of their own, one row per item
of E: `XLOOKUP(Q, K OVER T, V)` with Q over E is one lookup per item of E
(the broadcast). Database functions read the group's columns as the
database and a criteria block (CRIT) placed beside it. The formulas run
through p1.py's runner (the same exact-double reconstruction).

The TSV has two kinds of line:
    @name <TAB> {"T": items, "G": groups, "E": items, "m": {…}, "e": {…}}
    name <TAB> improv formula <TAB> expected <TAB> source
where expected lists one entry per group: a value, or a list with one value
per item of E when the formula reads an E measure (null: no cell).

Hand rows: SELECT and its range forms (Quantrix, plan §5.26; LibreOffice
has none), ARRAYTOTEXT (Excel's docs; LibreOffice 25.8 lacks it), an E item
whose key has no cell (no cell, where LibreOffice reads a blank as 0),
INDEX at an item with no cell (#N/A; LibreOffice reads the blank as 0) and
a database function whose WHERE keeps no row (no cell: an empty group has
none in Improv; LibreOffice gives 0 or #VALUE!).
"""

import json
import os
import re
import sys
from xml.sax.saxutils import escape

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402  (the LibreOffice runner)

NA, NUM = {"error": "#N/A"}, {"error": "#NUM!"}

DATA = {
    # Two groups; K ascending with a tie, D descending, W with an error.
    "A": {"T": 6, "G": 2, "m": {
        "K": [[1, 3, 3, 5, 8, 13], [2, 4, 6, 8, 10, 12]],
        "D": [[13, 8, 5, 3, 3, 1], [12, 10, 8, 6, 4, 2]],
        "V": [[10, 20, 30, 40, 50, 60], [5, 15, 25, 35, 45, 55]],
        "W": [[100, NA, 300, 400, 500, 600], [1, 2, 3, 4, 5, 6]],
    }},
    # Text keys; the last item has a key but no value (no row).
    "X": {"T": 6, "m": {
        "A": [["apple", "Banana", "cherry", "banana", "date", "fig"]],
        "B": [["red", "yellow", "dark red", "green", "brown", None]],
        "N": [[1.5, 2, 2, 7, 4, 9]],
    }},
    # The broadcast: a table over T (bands), keys over E (employees).
    "B": {"T": 4, "E": 6, "m": {
        "K": [[0, 50, 75, 90]],
        "V": [[1, 1.5, 2, 3]],
        "A": [["C", "B", "A", "S"]],
        "B": [["fair", "good", "great", "star"]],
    }, "e": {
        "Q": [10, 50, 80, 95, -5, None],
        "P": ["a", "S", "x", "b", "B", None],
        "I": [1, 4, 2, 5, 3, None],
    }},
}

# Criteria blocks for the database functions, per dataset: {field: test}.
CRIT = {"A": [{"K": ">2"}, {"K": "=5"}, {"K": ">=3", "V": "<50"}, {"K": ">100"}],
        "X": [{"A": "date"}]}

CASES = {name: [] for name in DATA}


def lo(ds, *fs):
    for f in fs:
        CASES[ds].append(("lo", f, None, None, None))


def db(ds, f, tmpl, override=None, src=None):
    """A database case: `tmpl` is the LibreOffice formula ({DB}, {C0}…);
    `override` replaces LibreOffice's per-group result (source `src`)."""
    CASES[ds].append(("lo", f, tmpl, override, src))


def hand(ds, f, fn, src):
    """fn(ds, g, r) -> expected (r is the E item, or None)."""
    CASES[ds].append(("hand", f, fn, src))


# --- hand definitions ---------------------------------------------------------
def table(ds, g, *ms):
    """The rows where every measure has a cell (the engine joins them)."""
    cols = [DATA[ds]["m"][m][g] for m in ms]
    return [r for r in zip(*cols) if all(v is not None for v in r)]


def key(ds, f, r):
    """The lookup argument: a literal, or E measure f's value at item r."""
    e = DATA[ds].get("e", {})
    return e[f][r] if isinstance(f, str) and f in e else f


def is_num(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def cmp(a, b):
    if isinstance(a, str) and isinstance(b, str):
        a, b = a.lower(), b.lower()
    elif not (is_num(a) and is_num(b)):
        return None
    return (a > b) - (a < b)


def select(vals, keys, test):
    def fn(ds, g, r):
        for v, k in table(ds, g, vals, keys):
            if test(k, r, ds):
                return v
        return NA
    return fn


def eq(f):
    return lambda k, r, ds: cmp(k, key(ds, f, r)) == 0


def side(f, want, inclusive):
    return lambda k, r, ds: cmp(k, key(ds, f, r)) in ((want, 0) if inclusive else (want,))


def between(lo_, hi, inclusive):
    return lambda k, r, ds: side(lo_, 1, inclusive)(k, r, ds) and side(hi, -1, inclusive)(k, r, ds)


def text(v, strict):
    if isinstance(v, dict):
        return v["error"]
    if isinstance(v, bool):
        return "TRUE" if v else "FALSE"
    if isinstance(v, str):
        return f'"{v}"' if strict else v
    return p1.fmt(v)


def arraytotext(m, strict):
    def fn(ds, g, r):
        vs = [v for v in DATA[ds]["m"][m][g] if v is not None]
        parts = [text(v, strict) for v in vs]
        return "{" + ",".join(parts) + "}" if strict else ", ".join(parts)
    return fn


def const(v):
    return lambda ds, g, r: v


EXCEL, QX, IMPROV = "Excel docs", "Quantrix (plan §5.26)", "Improv rule"

# --- dataset A: number keys, two groups ---------------------------------------
lo("A",
   "MATCH(3, K OVER T)", "MATCH(3, K OVER T, 0)", "MATCH(4.5, K OVER T, 1)",
   "MATCH(0, K OVER T)", "MATCH(99, K OVER T)", "MATCH(4, D OVER T, -1)",
   "MATCH(3, D OVER T, -1)", "MATCH(99, D OVER T, -1)", "MATCH(3, K OVER T, 0.5)",
   "LOOKUP(4, K OVER T, V)", "LOOKUP(3, K OVER T, V)", "LOOKUP(0, K OVER T, V)",
   "LOOKUP(99, K OVER T, W)",
   "XLOOKUP(3, K OVER T, V)", "XLOOKUP(3, K OVER T, V, -1)",
   "XLOOKUP(3, K OVER T, V, -1, 0, -1)", "XLOOKUP(7, K OVER T, V, 0, 1)",
   "XLOOKUP(7, K OVER T, V, 0, -1)", "XLOOKUP(99, K OVER T, V, 0, 1)",
   "XLOOKUP(3, K OVER T, W)", "XLOOKUP(3, K OVER T, W, 0, 0, -1)",
   "XLOOKUP(8, K OVER T, V, 0, 0, 2)", "XLOOKUP(8, K OVER T, V, 0, 0, 5)",
   "XMATCH(8, K OVER T)", "XMATCH(9, K OVER T, 1)", "XMATCH(9, K OVER T, -1)",
   "XMATCH(3, K OVER T, 0, -1)", "XMATCH(1, K OVER T, -1)", "XMATCH(14, K OVER T, 1)",
   "INDEX(V OVER T, 2)", "INDEX(V OVER T, 6)", "INDEX(W OVER T, 2)",
   "INDEX(V OVER T, 7)", "INDEX(V OVER T, 2.9)")
hand("A", "INDEX(V OVER T, 0)", const({"error": "#VALUE!"}), IMPROV)
for f, test in [("SELECT(V, K OVER T, 3)", eq(3)), ("SELECT(V, K OVER T, 4)", eq(4)),
                ("SELECTBETWEEN(V, K OVER T, 4, 9)", between(4, 9, True)),
                ("SELECTBETWEEN(V, K OVER T, 5, 8, FALSE)", between(5, 8, False)),
                ("SELECTBETWEEN(V, K OVER T, 5, 8, TRUE)", between(5, 8, True)),
                ("SELECTGREATERTHAN(V, K OVER T, 5)", side(5, 1, False)),
                ("SELECTGREATERTHAN(V, K OVER T, 5, TRUE)", side(5, 1, True)),
                ("SELECTGREATERTHAN(V, K OVER T, 13)", side(13, 1, False)),
                ("SELECTLESSTHAN(V, K OVER T, 3)", side(3, -1, False)),
                ("SELECTLESSTHAN(V, K OVER T, 1)", side(1, -1, False)),
                ("SELECTLESSTHAN(V, K OVER T, 2, TRUE)", side(2, -1, True)),
                ("SELECT(W, K OVER T, 3)", eq(3))]:
    hand("A", f, select(f[f.index("(") + 1], "K", test), QX)
hand("A", "ARRAYTOTEXT(V OVER T)", arraytotext("V", False), EXCEL)
hand("A", "ARRAYTOTEXT(W OVER T, 1)", arraytotext("W", True), EXCEL)
for name, lo_name in [("DSUM", "DSUM"), ("DAVERAGE", "DAVERAGE"), ("DCOUNT", "DCOUNT"),
                      ("DCOUNTA", "DCOUNTA"), ("DMAX", "DMAX"), ("DMIN", "DMIN"),
                      ("DPRODUCT", "DPRODUCT"), ("DSTDEV", "DSTDEV"), ("DSTDEVP", "DSTDEVP"),
                      ("DVAR", "DVAR"), ("DVARP", "DVARP")]:
    db("A", f"{name}(V OVER T WHERE K > 2)", f'{lo_name}({{DB}};"V";{{C0}})')
db("A", "DSUM(V OVER T WHERE K >= 3 AND V < 50)", 'DSUM({DB};"V";{C2})')
db("A", "DSUM(W OVER T WHERE K > 2)", 'DSUM({DB};"W";{C0})')
# No match: an empty group has no cell in Improv (Excel: #VALUE!); more
# than one: Excel's #NUM! (LibreOffice: Err:502).
db("A", "DGET(V OVER T WHERE K == 5)", 'DGET({DB};"V";{C1})', [40, None], IMPROV)
db("A", "DGET(V OVER T WHERE K > 2)", 'DGET({DB};"V";{C0})', [NUM, NUM], EXCEL)
hand("A", "DSUM(V OVER T WHERE K > 100)", const(None), IMPROV)
hand("A", "DGET(V OVER T WHERE K > 100)", const(None), IMPROV)

# --- dataset X: text keys ------------------------------------------------------
lo("X",
   'XLOOKUP("banana", A OVER T, B)', 'XLOOKUP("banana", A OVER T, B, "?", 0, -1)',
   'XLOOKUP("b*", A OVER T, B, "none")', 'XLOOKUP("b*", A OVER T, B, "none", 2)',
   'XLOOKUP("c?erry", A OVER T, B, "none", 2)', 'XLOOKUP("kiwi", A OVER T, B, "none")',
   'XMATCH("CHERRY", A OVER T)', 'XMATCH("blueberry", A OVER T, 1)',
   'XMATCH("blueberry", A OVER T, -1)', 'XMATCH("*e", A OVER T, 2, -1)',
   'MATCH("d*", A OVER T, 0)', 'MATCH("Date", A OVER T, 0)', 'MATCH("zz", A OVER T, 0)',
   'XLOOKUP(2, N OVER T, B)', 'XLOOKUP(2, N OVER T, B, "-", 0, -1)',
   'XLOOKUP(3, N OVER T, B, "-", 1)', 'XMATCH(7, N OVER T)')
db("X", "DGET(B OVER T WHERE A == \"date\")", 'DGET({DB};"B";{C0})')
for f, test in [('SELECT(B, A OVER T, "BANANA")', eq("BANANA")),
                ('SELECT(B, A OVER T, "b*")', eq("b*")),
                ('SELECTGREATERTHAN(B, A OVER T, "c")', side("c", 1, False))]:
    hand("X", f, select("B", "A", test), QX)
hand("X", "ARRAYTOTEXT(B OVER T)", arraytotext("B", False), EXCEL)
hand("X", "ARRAYTOTEXT(A OVER T, 1)", arraytotext("A", True), EXCEL)
hand("X", "ARRAYTOTEXT(N OVER T, 1)", arraytotext("N", True), EXCEL)
hand("X", "INDEX(B OVER T, 6)", const(NA), IMPROV)
# fig has a key but no value: its item is not in the table.
hand("X", 'XLOOKUP("fig", A OVER T, B, "none")', const("none"), IMPROV)

# --- dataset B: keys over another category (the broadcast) ----------------------
lo("B",
   "LOOKUP(Q, K OVER T, V)", "XLOOKUP(Q, K OVER T, V, 0, -1)", "MATCH(Q, K OVER T)",
   "XMATCH(Q, K OVER T, 1)", 'XLOOKUP(P, A OVER T, B, "none")', "XMATCH(P, A OVER T)",
   "INDEX(V OVER T, I)", "INDEX(B OVER T, I)")
for f, test in [("SELECT(B, A OVER T, P)", eq("P")),
                ("SELECTGREATERTHAN(V, K OVER T, Q)", side("Q", 1, False)),
                ("SELECTLESSTHAN(B, K OVER T, Q, TRUE)", side("Q", -1, True))]:
    vals = f[f.index("(") + 1]
    hand("B", f, select(vals, "A" if "A OVER" in f else "K", test), QX)

LO_NAMES = {"XLOOKUP": "COM.MICROSOFT.XLOOKUP", "XMATCH": "COM.MICROSOFT.XMATCH"}
p1.LO_NAMES = LO_NAMES


def col_name(i):
    assert i < 25, "more columns than B..Z"
    return chr(ord("B") + i)


def layout(ds):
    """Columns: each group's table measures, then the E measures, then the
    criteria blocks. Returns (table cols, E cols, criteria (first, last))."""
    d, i = DATA[ds], 0
    tcols, ecols, crit = {}, {}, []
    for g in range(d.get("G", 1)):
        for m in d["m"]:
            tcols[(m, g)] = col_name(i)
            i += 1
    for m in d.get("e", {}):
        ecols[m] = col_name(i)
        i += 1
    for block in CRIT.get(ds, []):
        crit.append((col_name(i), col_name(i + len(block) - 1)))
        i += len(block)
    return tcols, ecols, crit


def value_cell(v):
    if v is None:
        return "<table:table-cell/>"
    if isinstance(v, dict):
        fn = {"#N/A": "NA()", "#DIV/0!": "1/0"}[v["error"]]
        return f'<table:table-cell table:formula="of:={fn}"/>'
    if isinstance(v, bool):
        return ('<table:table-cell office:value-type="boolean" '
                f'office:boolean-value="{str(v).lower()}"/>')
    if isinstance(v, str):
        return ('<table:table-cell office:value-type="string">'
                f"<text:p>{escape(v)}</text:p></table:table-cell>")
    return f'<table:table-cell office:value-type="float" office:value="{v!r}"/>'


def sheet(ds):
    d = DATA[ds]
    tcols, ecols, _ = layout(ds)
    header = [None] * (len(tcols) + len(ecols))
    for (m, _g), c in tcols.items():
        header[ord(c) - ord("B")] = m
    for m, c in ecols.items():
        header[ord(c) - ord("B")] = m
    crit_rows = [[], []]
    for block in CRIT.get(ds, []):
        for field, test in block.items():
            crit_rows[0].append(field)
            crit_rows[1].append(test)
    rows = [header + crit_rows[0]]
    n_rows = max(d["T"], d.get("E", 0), 1)
    for r in range(n_rows):
        row = []
        for (m, g) in tcols:
            vals = d["m"][m][g]
            row.append(vals[r] if r < len(vals) else None)
        for m in ecols:
            vals = d["e"][m]
            row.append(vals[r] if r < len(vals) else None)
        if r == 0:
            row += crit_rows[1]
        rows.append(row)
    out = [f'<table:table table:name="D{ds}">']
    for row in rows:
        # Column A stays empty (the runner skips the row).
        cells = "".join(value_cell(v) for v in row)
        out.append(f"<table:table-row><table:table-cell/>{cells}</table:table-row>")
    out.append("</table:table>")
    return "\n".join(out)


def names(ds):
    return set(DATA[ds]["m"]) | set(DATA[ds].get("e", {}))


def to_sheet(f, ds, g, r):
    """Improv-derived ODFF `f` over group g at E item r."""
    d = DATA[ds]
    tcols, ecols, crit = layout(ds)
    n = d["T"]
    pat = r"(?<![\w.$\[])(" + "|".join(sorted(names(ds))) + r")\b(?![(.\w])"

    def sub(m):
        name = m.group(1)
        if name in ecols:
            return f"[$D{ds}.{ecols[name]}{r + 2}]"
        c = tcols[(name, g)]
        return f"[$D{ds}.{c}2:.{c}{n + 1}]"

    parts = re.split(r'("[^"]*")', f)
    for k in range(0, len(parts), 2):
        parts[k] = re.sub(pat, sub, parts[k])
    out = "".join(parts)
    gcols = [c for (_m, gg), c in tcols.items() if gg == g]
    out = out.replace("{DB}", f"[$D{ds}.{gcols[0]}1:.{gcols[-1]}{n + 1}]")
    for i, (a, b) in enumerate(crit):
        out = out.replace("{C%d}" % i, f"[$D{ds}.{a}1:.{b}2]")
    return out


def derive(f):
    """Improv `F(k, x OVER T, …)` -> ODFF `F(k;x;…)`."""
    return p1.to_lo(f.replace(" OVER T", ""))


def map_error(shown):
    if shown in p1.LO_ERRORS:
        return {"error": shown}
    if shown in ("Err:502", "Err:504"):
        return {"error": "#VALUE!"}
    return {"error": shown, "unmapped": True}


def e_items(ds, f):
    """The E items a formula reads (None: it reads no E measure)."""
    e = DATA[ds].get("e", {})
    used = [m for m in e if re.search(r"(?<![\w\"])" + m + r"\b(?![(\"])", f)]
    return range(DATA[ds]["E"]) if used else None, used


def has_cell(ds, used, r):
    return all(DATA[ds]["e"][m][r] is not None for m in used)


def groups(ds):
    return range(DATA[ds].get("G", 1))


def main():
    formulas, slots = [], []
    for ds, cases in CASES.items():
        for ci, case in enumerate(cases):
            if case[0] == "hand":
                continue
            _, f, tmpl, _, _ = case
            items, used = e_items(ds, f)
            for g in groups(ds):
                for r in (items or [None]):
                    if r is not None and not has_cell(ds, used, r):
                        continue
                    formulas.append(to_sheet(tmpl or derive(f), ds, g, r or 0))
                    slots.append((ds, ci, g, r))
    p1.TAIL = ("</table:table>" + "".join(sheet(ds) for ds in DATA)
               + "</office:spreadsheet></office:body></office:document>\n")
    vals = p1.run_lo(["PI()"] + formulas)
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    got = {}
    for slot, v in zip(slots, vals[1:]):
        got[slot] = map_error(v[1]) if isinstance(v, tuple) else v
    print("formula\texpected\tsource")
    for ds, cases in CASES.items():
        print(f"@{ds}\t{json.dumps(DATA[ds], ensure_ascii=False)}")
        for ci, case in enumerate(cases):
            f = case[1]
            items, used = e_items(ds, f)
            src = case[3] if case[0] == "hand" else "LibreOffice"
            if case[0] == "lo" and case[3] is not None:
                lo_exp = [got[(ds, ci, g, None)] for g in groups(ds)]
                print(f"LibreOffice differs: {ds} {f} LO={json.dumps(lo_exp)} "
                      f"{case[4]}={json.dumps(case[3])}", file=sys.stderr)
                print(f"{ds}\t{f}\t{json.dumps(case[3])}\t{case[4]}")
                continue
            exp = []
            for g in groups(ds):
                def at(r):
                    if r is not None and not has_cell(ds, used, r):
                        return None  # the key has no cell: no lookup
                    if case[0] == "hand":
                        return case[2](ds, g, r)
                    return got[(ds, ci, g, r)]
                exp.append([at(r) for r in items] if items else at(None))
            if "unmapped" in json.dumps(exp):
                raise SystemExit(f"unmapped LibreOffice error for {ds} {f}: {exp}")
            assert "\t" not in f and "^" not in f, f
            print(f"{ds}\t{f}\t{json.dumps(exp, ensure_ascii=False)}\t{src}")


if __name__ == "__main__":
    main()
