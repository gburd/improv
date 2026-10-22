#!/usr/bin/env python3
"""Print dataset C of crates/engine/tests/data/array_reduce_goldens.tsv:
LINEST with collinear x's, from Gnumeric (ssconvert --recalc).

    python3 scripts/goldens/linest_collinear.py

Excel drops an x that is a combination of the x's before it ("removes any
redundant X columns"); Gnumeric 1.12.57 does the same, and LibreOffice 25.2
does not (an ill-conditioned fit, or #VALUE!), so these rows come from
Gnumeric. The output replaces the @C dataset and its rows (before @S).
"""

import csv
import json
import os
import subprocess
import tempfile

Y = [1, 3, 2, 5, 7.5, 6]
X = [1, 2, 3, 4, 6, 7]
Z = [0.5, 1, 4, 2, 3, 1]
COLS = {"Y": Y, "X": X, "Z": Z, "D": [2 * v for v in X], "S": [a + b for a, b in zip(X, Z)]}
# (Improv formula, its x columns in order, const, stats)
CASES = [
    ("LINEST(Y, X, D OVER T, TRUE, TRUE INTO K)", ["X", "D"], 1, 1),
    ("LINEST(Y, X, Z, S OVER T, TRUE, TRUE INTO K)", ["X", "Z", "S"], 1, 1),
    ("LINEST(Y, X, D OVER T INTO K)", ["X", "D"], 1, 0),
    ("LINEST(Y, D, X OVER T, FALSE INTO K)", ["D", "X"], 0, 0),
]
K = 20


def gnumeric(xs, konst, stats):
    """LINEST's block, row by row, at 17 significant digits."""
    d = tempfile.mkdtemp(dir=os.path.expanduser("~"))
    src, out = os.path.join(d, "in.csv"), os.path.join(d, "out.csv")
    k = len(xs)
    last = chr(ord("A") + k)
    with open(src, "w") as f:
        for r in range(len(Y)):
            f.write(",".join([str(Y[r])] + [str(COLS[x][r]) for x in xs]) + "\n")
        cells = []
        for row in range(1, (5 if stats else 1) + 1):
            for col in range(1, k + 2):
                fx = (f"IFERROR(TEXT(INDEX(LINEST(A1:A6,B1:{last}6,{konst},{stats}),"
                      f'{row},{col}),"0.0000000000000000E+00"),"NA")')
                cells.append('"=' + fx.replace('"', '""') + '"')
        f.write(",".join(cells) + "\n")
    subprocess.run(["ssconvert", "--recalc", src, out], capture_output=True, check=True)
    vals = list(csv.reader(open(out)))[len(Y)]
    return [{"error": "#N/A"} if v == "NA" else float(v) for v in vals if v != ""]


def main():
    print("@C\t" + json.dumps({"T": len(Y), "K": K, "m": {n: [c] for n, c in COLS.items()}}))
    for form, xs, konst, stats in CASES:
        out = gnumeric(xs, konst, stats)
        print(f"C\t{form}\t{json.dumps([out + [None] * (K - len(out))])}\tGnumeric")


main()
