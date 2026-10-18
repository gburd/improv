#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/analysis_reduce_goldens.tsv (the P7
analysis reductions: matrices, regression, forecasting, signal processing,
normality tests).

    python3 scripts/goldens/analysis_reduce.py \\
        > crates/engine/tests/data/analysis_reduce_goldens.tsv

Each dataset (DATA) has categories (`cats`: name -> item count; G, when
present, is the group category kept by every result) and measures (`m`:
name -> the categories reduced, and per group the values flattened row by
row). On the spreadsheet a measure's group is a block on the dataset's
sheet: a column for one category, R rows x C columns for two.

An Improv formula such as `MINVERSE(A OVER R, C INTO P, Q)` becomes, per
group, the spreadsheet formula over the group's blocks, `MINVERSE(range)`
(OVER and INTO dropped; a case may give its own spreadsheet formula, `sheet`,
with the measure names standing for their ranges). The result is read back
by shape:
  - an aggregation: the cell's value;
  - a per-item function (one value per item of T): INDEX(F, i, 1);
  - an array (INTO): ROWS(F), COLUMNS(F) and INDEX(F, r, c), flattened row
    by row onto the INTO categories' cells (row-major), cut to their count.
LibreOffice rows go through p1.py's runner (its exact-double
reconstruction); Gnumeric rows through ssconvert's text export, which writes
shortest round-trip doubles (exact).

The TSV has two kinds of line:
    @name <TAB> dataset JSON
    name <TAB> improv formula <TAB> expected <TAB> source
where expected has, per group, a list with one entry per result cell (one
for an aggregation; one per item of T; one per INTO cell), null for no cell.

Where a product disagrees with Excel's documentation the Excel value is kept
(source `Excel docs`) and stderr shows the product's value.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
from xml.sax.saxutils import escape

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402  (the LibreOffice runner)

NUM = {"error": "#NUM!"}
VALUE = {"error": "#VALUE!"}
NA = {"error": "#N/A"}

DATA = {
    # Two 3x3 matrices (group 0 symmetric positive definite, group 1
    # general), a singular one and one needing row swaps.
    "M": {"cats": {"R": 3, "C": 3, "G": 2, "P": 3, "Q": 3, "E": 4, "K": 10}, "m": {
        "A": [["R", "C"], [[4, 2, -2, 2, 10, 4, -2, 4, 9], [1, 2, 3, 4, 5, 6, 7, 8, 10]]],
        "S": [["R", "C"], [[1, 2, 3, 2, 4, 6, 1, 0, 1], [0, 1, 2, 1, 0, 3, 4, -3, 8]]],
    }},
    # A 4x4 and a 1x1; a 2x3 (not square) and a rank-1 3x2.
    "N": {"cats": {"R": 4, "C": 4, "P": 4, "Q": 4, "E": 5, "K": 17}, "m": {
        "A": [["R", "C"], [[2, -1, 0, 3, 1, 5, -2, 0, 0, 4, 7, 1, 6, 0, 1, -3]]],
    }},
    "O": {"cats": {"R": 1, "C": 1, "P": 1, "Q": 1, "E": 2}, "m": {
        "A": [["R", "C"], [[-2.5]]],
    }},
    "W": {"cats": {"R": 2, "C": 3, "P": 3, "Q": 2, "K": 7}, "m": {
        "A": [["R", "C"], [[1, 2, 3, 4, 5, 6.5]]],
        "Z": [["R", "C"], [[0, 0, 0, 0, 0, 0]]],
    }},
    "V": {"cats": {"R": 3, "C": 2, "P": 2, "Q": 3}, "m": {
        "A": [["R", "C"], [[1, 2, 2, 4, 3, 6]]],
    }},
    # Series: two groups (the second's x out of order), ties in Z.
    "S": {"cats": {"T": 8, "G": 2}, "m": {
        "X": [["T"], [[1, 2, 3, 4, 5, 6, 7, 8], [3, 1, 4, 1.5, 5, 9, 2, 6]]],
        "Y": [["T"], [[2, 4, 3, 8, 7, 10, 9, 14], [5, 3, 8, 2, 4, 1, 6, 7]]],
        "Z": [["T"], [[7, 7, 8, 9, 9, 9, 10, 12], [1, 1, 1, 2, 2, 3, 3, 3]]],
    }},
    # A repeated x; six values (HPFILTER's minimum).
    "D": {"cats": {"T": 6}, "m": {
        "X": [["T"], [[1, 2, 2, 3, 4, 5]]],
        "Y": [["T"], [[1, 5, 2, 8, 3, 9]]],
        "Z": [["T"], [[7, 7, 8, 8, 9, 10]]],
    }},
    # Normality: a near-normal sample, a skewed one; ties; constant data.
    "T": {"cats": {"T": 12, "G": 2}, "m": {
        "X": [["T"], [[4.2, 5.1, 3.9, 5.6, 4.8, 5.0, 4.4, 6.1, 5.3, 4.7, 5.5, 4.9],
                      [1, 1, 2, 1, 3, 8, 1, 2, 15, 1, 2, 40]]],
        "C": [["T"], [[3] * 12, [7] * 12]],
    }},
    "U": {"cats": {"T": 1}, "m": {"Z": [["T"], [[4]]]}},
    "P": {"cats": {"T": 2}, "m": {"Z": [["T"], [[3, 8]]]}},
}

GN = "Gnumeric 1.12"
LO = "LibreOffice"
CASES = {name: [] for name in DATA}


def case(ds, f, src, sheet=None, excel=None, text=False):
    """A golden row: the Improv formula `f`, computed by `src` (LO or GN)
    from `sheet` (default: `f` without OVER/INTO)."""
    CASES[ds].append({"f": f, "src": src, "sheet": sheet, "excel": excel, "text": text})


# --- group 1: determinants and inverses ---------------------------------------
for ds in ("M", "N", "O"):
    case(ds, "MDETERM(A OVER R, C)", LO)
    case(ds, "LOGMDETERM(A OVER R, C)", LO, sheet="LN(ABS(MDETERM(A)))")
    case(ds, "MINVERSE(A OVER R, C INTO P, Q)", LO)
    case(ds, "MPSEUDOINVERSE(A OVER R, C INTO P, Q)", GN)
case("M", "MDETERM(S OVER R, C)", LO)
# LibreOffice's LN(0) is Err:502; LOGMDETERM of a singular matrix is #NUM!.
case("M", "LOGMDETERM(S OVER R, C)", LO, sheet="LN(ABS(MDETERM(S)))")
# A singular matrix: LibreOffice's Err:502 is Excel's #NUM!.
case("M", "MINVERSE(S OVER R, C INTO P, Q)", LO)
case("M", "MINVERSE(A OVER R, C INTO K)", LO)
case("M", "MPSEUDOINVERSE(S OVER R, C INTO P, Q)", GN)
case("M", "MPSEUDOINVERSE(S OVER R, C, 0.01 INTO P, Q)", GN)
case("N", "MINVERSE(A OVER R, C INTO K)", LO)
# "MDETERM returns the #VALUE! error when array does not have an equal
# number of rows and columns"; likewise MINVERSE (LibreOffice: Err:502).
case("W", "MDETERM(A OVER R, C)", LO, excel=[[VALUE]])
case("W", "MINVERSE(A OVER R, C INTO P, Q)", LO, excel=[[VALUE] + [None] * 5])
case("W", "MPSEUDOINVERSE(A OVER R, C INTO P, Q)", GN)
case("W", "MPSEUDOINVERSE(A OVER R, C INTO K)", GN)
case("W", "MPSEUDOINVERSE(Z OVER R, C INTO P, Q)", GN)
case("V", "MPSEUDOINVERSE(A OVER R, C INTO P, Q)", GN)


# --- group 2: decompositions (Gnumeric) ----------------------------------------
for ds in ("M", "N", "O"):
    case(ds, "CHOLESKY(A OVER R, C INTO P, Q)", GN)
    case(ds, "EIGEN(A OVER R, C INTO E, Q)", GN)
case("M", "CHOLESKY(S OVER R, C INTO P, Q)", GN)
case("M", "EIGEN(S OVER R, C INTO E, Q)", GN)
case("M", "EIGEN(A OVER R, C INTO K)", GN)
case("W", "CHOLESKY(A OVER R, C INTO P, Q)", GN)
case("W", "EIGEN(A OVER R, C INTO P, Q)", GN)


# --- group 3: series (Gnumeric) -------------------------------------------------
for t in ("0.5", "2.5", "4", "7.25", "9.5"):
    for meth in ("0", "2", "4"):
        case("S", f"INTERPOLATION(X, Y OVER T, {t}, {meth})", GN)
case("S", "INTERPOLATION(X, Y OVER T, 3.3)", GN)
for t0, t1 in (("2.5", "6.2"), ("0.5", "1.5"), ("3", "3.5"), ("7", "9.5"), ("1", "8")):
    for meth in ("1", "3", "5"):
        case("S", f"INTERPOLATION(X, Y OVER T, {t0}, {meth}, {t1})", GN,
             sheet=f"INTERPOLATION(X, Y, {{{t0};{t1}}}, {meth})")
case("D", "INTERPOLATION(X, Y OVER T, 2.5)", GN)
case("D", "INTERPOLATION(Y, Z OVER T, 2.5, 4)", GN)
for lam in ("", ", 100", ", 100000", ", 0.5"):
    case("S", f"HPFILTER(Y{lam} OVER T)".replace(f"Y{lam} OVER T", f"Y OVER T{lam}"), GN)
case("S", "HPFILTER(Z OVER T)", GN)
case("D", "HPFILTER(Y OVER T, 10)", GN)
for iv in ("", ", 2", ", 0.5", ", 0"):
    case("S", f"SSMEDIAN(Z OVER T{iv})", GN)
    case("D", f"SSMEDIAN(Z OVER T{iv})", GN)
case("S", "SSMEDIAN(X OVER T)", GN)
case("U", "SSMEDIAN(Z OVER T)", GN)
case("P", "SSMEDIAN(Z OVER T)", GN)
# "Each data point in array is the midpoint of the interval": a single
# point is its own median; HPFILTER needs six.
case("P", "HPFILTER(Z OVER T)", GN, sheet="HPFILTER(Z)")


# --- group 4: normality tests (Gnumeric: {p; statistic; n}) --------------------
for t in ("ADTEST", "CVMTEST", "LKSTEST", "SFTEST"):
    for m in ("X", "C"):
        case("T", f"{t}({m} OVER T)", GN, sheet=f"INDEX({t}({m}),1)")
        case("T", f"{t}.STAT({m} OVER T)", GN, sheet=f"INDEX({t}({m}),2)")
    for ds in ("S", "D"):
        case(ds, f"{t}(Y OVER T)", GN, sheet=f"INDEX({t}(Y),1)")
        case(ds, f"{t}.STAT(Y OVER T)", GN, sheet=f"INDEX({t}(Y),2)")


# --- layout -------------------------------------------------------------------
def col_name(i):
    return (col_name(i // 26 - 1) if i >= 26 else "") + chr(ord("A") + i % 26)


def groups(ds):
    return DATA[ds]["cats"].get("G", 1)


def layout(ds):
    """(measure, group) -> (first column index, rows, cols) on the sheet
    (column A stays empty)."""
    d, out, at = DATA[ds], {}, 1
    for m, (over, vals) in d["m"].items():
        rows = d["cats"][over[0]]
        cols = d["cats"][over[1]] if len(over) > 1 else 1
        for g in range(len(vals)):
            out[(m, g)] = (at, rows, cols)
            at += cols + 1
    return out


def cells(ds):
    """(row, col, value) of every data cell."""
    d = DATA[ds]
    for (m, g), (c0, rows, cols) in layout(ds).items():
        vals = d["m"][m][1][g]
        for k, v in enumerate(vals):
            yield k // cols, c0 + k % cols, v


def ref(ds, m, g, lo):
    c0, rows, cols = layout(ds)[(m, g)]
    a, b = col_name(c0), col_name(c0 + cols - 1)
    if lo:
        return f"[$D{ds}.${a}$1:.${b}${rows}]"
    return f"D{ds}!${a}$1:${b}${rows}"


def into(f):
    m = re.search(r" INTO ([A-Z0-9, ]+)\)$", f)
    return m.group(1).split(", ") if m else []


def shape(ds, f):
    """('agg' | 'item' | 'array', number of result cells)."""
    cats = DATA[ds]["cats"]
    if into(f):
        n = 1
        for c in into(f):
            n *= cats[c]
        return "array", n
    if f.split("(")[0] in PER_ITEM:
        return "item", cats["T"]
    return "agg", 1


PER_ITEM = {"HPFILTER"}


def sheet_formula(ds, c, g, lo):
    f = c["sheet"]
    if f is None:
        cats = "|".join(DATA[ds]["cats"])
        f = re.sub(r" INTO [A-Z0-9, ]+\)$", ")", c["f"])
        f = re.sub(rf" OVER ({cats})(, ({cats}))*(?=[,)])", "", f)
    if lo:
        f = p1.to_lo(f)
    parts = re.split(r'("[^"]*")', f)
    names = "|".join(DATA[ds]["m"])
    for k in range(0, len(parts), 2):
        parts[k] = re.sub(rf"(?<![.\w$])({names})\b(?![.(])",
                          lambda mm: ref(ds, mm.group(1), g, lo), parts[k])
    return "".join(parts)


def probes(ds, c, g, lo):
    """The formulas that read case c's group g back."""
    f = sheet_formula(ds, c, g, lo)
    sep = ";" if lo else ","
    kind, n = shape(ds, c["f"])
    if kind == "agg":
        return [f]
    if kind == "item":
        return [f] + [f"INDEX({f}{sep}{i}{sep}1)" for i in range(1, n + 1)]
    # The k-th value row by row: INDEX(F, (k-1) div cols + 1, (k-1) mod cols + 1).
    cols = f"COLUMNS({f})"
    return [f, f"ROWS({f})", cols] + [
        f"INDEX({f}{sep}INT({k - 1}/{cols})+1{sep}MOD({k - 1}{sep}{cols})+1)"
        for k in range(1, n + 1)]


def assemble(ds, c, vals):
    """A group's probe values -> its list of result cells."""
    kind, n = shape(ds, c["f"])
    if kind == "agg":
        return [vals[0]]
    if kind == "item":
        return vals[1:]
    first, rows, cols = vals[0], vals[1], vals[2]
    if not isinstance(rows, float):
        return [first] + [None] * (n - 1)
    m = min(int(rows) * int(cols), n)
    return vals[3:3 + m] + [None] * (n - m)


# --- LibreOffice ----------------------------------------------------------------
def lo_sheet(ds):
    rows = {}
    for r, c, v in cells(ds):
        rows.setdefault(r, {})[c] = v
    out = [f'<table:table table:name="D{ds}">']
    for r in range(max(rows) + 1):
        row, last = [], 0
        for c in sorted(rows[r]):
            if c > last:
                row.append(f'<table:table-cell table:number-columns-repeated="{c - last}"/>')
            v = rows[r][c]
            if isinstance(v, dict):
                row.append('<table:table-cell table:formula="of:=NA()"/>')
            else:
                row.append(f'<table:table-cell office:value-type="float" office:value="{v!r}"/>')
            last = c + 1
        out.append("<table:table-row>" + "".join(row) + "</table:table-row>")
    out.append("</table:table>")
    return "\n".join(out)


def lo_value(v):
    if isinstance(v, tuple):
        shown = v[1]
        if shown in p1.LO_ERRORS:
            return {"error": shown}
        if shown == "Err:502":
            return NUM
        if shown == "Err:504":
            return VALUE
        return {"error": shown, "unmapped": True}
    return v


def run_lo(formulas):
    p1.TAIL = ("</table:table>" + "".join(lo_sheet(ds) for ds in DATA)
               + "</office:spreadsheet></office:body></office:document>\n")
    vals = p1.run_lo(["PI()"] + formulas)
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    return [lo_value(v) for v in vals[1:]]


# --- Gnumeric -------------------------------------------------------------------
def gn_cell(r, c, v):
    if isinstance(v, dict):
        return f'<gnm:Cell Row="{r}" Col="{c}">=NA()</gnm:Cell>'
    return f'<gnm:Cell Row="{r}" Col="{c}" ValueType="40">{v!r}</gnm:Cell>'


def gn_sheet(name, cs):
    return (f"<gnm:Sheet><gnm:Name>{name}</gnm:Name><gnm:MaxCol>256</gnm:MaxCol>"
            f"<gnm:MaxRow>65536</gnm:MaxRow><gnm:Cells>{''.join(cs)}</gnm:Cells></gnm:Sheet>")


def run_gn(formulas, text):
    names = ["F"] + [f"D{ds}" for ds in DATA]
    sheets = [gn_sheet("F", [f'<gnm:Cell Row="{i}" Col="0">={escape(f)}</gnm:Cell>'
                             for i, f in enumerate(formulas)])]
    sheets += [gn_sheet(f"D{ds}", [gn_cell(r, c, v) for r, c, v in cells(ds)]) for ds in DATA]
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "in.gnumeric")
        with open(src, "w", encoding="utf-8") as fh:
            fh.write('<?xml version="1.0"?><gnm:Workbook xmlns:gnm="http://www.gnumeric.org/v10.dtd">'
                     "<gnm:SheetNameIndex>"
                     + "".join(f"<gnm:SheetName>{n}</gnm:SheetName>" for n in names)
                     + "</gnm:SheetNameIndex><gnm:Sheets>" + "".join(sheets)
                     + "</gnm:Sheets></gnm:Workbook>")
        out = os.path.join(d, "out.txt")
        subprocess.run(["ssconvert", "--recalc", "--export-type=Gnumeric_stf:stf_assistant",
                        "-O", "sheet=F separator=\t format=raw quote=never", src, out],
                       check=True, capture_output=True)
        with open(out, encoding="utf-8") as fh:
            lines = fh.read().split("\n")
    vals = []
    for line, is_text in zip(lines, text):
        v = line.split("\t")[0]
        if v.startswith("#"):
            vals.append({"error": v})
        elif is_text:
            vals.append(v)
        else:
            vals.append(float(v))
    return vals


def main():
    jobs = {LO: [], GN: []}  # src -> [(ds, case index, group, n probes)]
    forms = {LO: [], GN: []}
    texts = []
    for ds, cs in CASES.items():
        for ci, c in enumerate(cs):
            for g in range(groups(ds)):
                ps = probes(ds, c, g, c["src"] == LO)
                jobs[c["src"]].append((ds, ci, g, len(ps)))
                forms[c["src"]] += ps
                if c["src"] == GN:
                    # A list's shape probes (ROWS, COLUMNS) are numbers.
                    t = [c["text"]] * len(ps)
                    if shape(ds, c["f"])[0] == "array":
                        t[1:3] = [False, False]
                    texts += t
    got = {}
    for src, run in ((LO, run_lo), (GN, lambda fs: run_gn(fs, texts))):
        vals, at = run(forms[src]) if forms[src] else [], 0
        for ds, ci, g, n in jobs[src]:
            got[(ds, ci, g)] = assemble(ds, CASES[ds][ci], vals[at:at + n])
            at += n
    print("formula\texpected\tsource")
    for ds, cs in CASES.items():
        print(f"@{ds}\t{json.dumps(DATA[ds])}")
        for ci, c in enumerate(cs):
            exp = [got[(ds, ci, g)] for g in range(groups(ds))]
            src = c["src"]
            if c["excel"] is not None:
                excel = [e if e is not None else exp[g] for g, e in enumerate(c["excel"])]
                print(f"{src} differs: {ds} {c['f']} got={json.dumps(exp)} "
                      f"Excel={json.dumps(excel)}", file=sys.stderr)
                exp, src = excel, "Excel docs"
            elif "unmapped" in json.dumps(exp):
                raise SystemExit(f"unmapped error for {ds} {c['f']}: {exp}")
            assert "\t" not in c["f"] and "^" not in c["f"], c["f"]
            print(f"{ds}\t{c['f']}\t{json.dumps(exp, ensure_ascii=False)}\t{src}")


if __name__ == "__main__":
    main()
