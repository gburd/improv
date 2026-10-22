#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/numtext_goldens.tsv: a number joined
to text (`"x" & 1e21`), Excel's General form.

    python3 scripts/goldens/numtext.py > crates/engine/tests/data/numtext_goldens.tsv

The expected text is C's `%.15G`: 15 significant digits (Excel's
precision), in exponent form below 1E-4 and from 1E+15, with at least two
exponent digits (`1E+21`, `1.5E-07`), as Excel spells it. Each row is run
through LibreOffice (soffice --headless) and Gnumeric (ssconvert --recalc),
and its source names the products that agree. They differ from each other:
LibreOffice writes three exponent digits (`1E+021`) and switches later
(1E-15, 1E+16); Gnumeric writes up to 17 digits (`0.30000000000000004`)
and switches at 1E+17. A row neither matches is the rule's own (source
`%.15G`): from 1E+15 a 15-digit value has no fixed form without an
invented trailing zero.
"""

import csv
import json
import os
import subprocess
import sys
import tempfile

# (Improv literal, spreadsheet expression, value)
CASES = [
    ("1e21", "1E+21"), ("1e20", "1E+20"), ("1e100", "1E+100"), ("2.5e-300", "2.5E-300"),
    ("1e-10", "1E-10"), ("1e-5", "1E-05"), ("1.5e-7", "1.5E-07"), ("0.0000123", "0.0000123"),
    ("0.0001", "0.0001"), ("0.000123456789", "0.000123456789"),
    ("123456789012345", "123456789012345"), ("999999999999999", "999999999999999"),
    ("1e15", "1E+15"), ("1125899906842624", "2^50"), ("1e16", "1E+16"),
    ("123456789012345678901", "123456789012345678901"), ("1.23456789e20", "123456.789E+15"),
    ("12345678901.2345", "12345678901.2345"), ("1 / 3", "1/3"), ("0.1 + 0.2", "0.1+0.2"),
    ("2 / 3 * 1e-5", "2/3*1E-5"), ("0 - 1e21", "-1E+21"), ("0 - 1e-10", "-1E-10"),
]


def run(tool, rows):
    d = tempfile.mkdtemp(dir=os.path.expanduser("~"))
    src = os.path.join(d, "in.csv")
    with open(src, "w") as f:
        for _, e in rows:
            f.write(f'"=""x""&({e})"\n')
    if tool == "lo":
        subprocess.run(["soffice", "--headless", "--convert-to",
                        "csv:Text - txt - csv (StarCalc):44,34,76,1,,0,false,true,false,false",
                        "--outdir", os.path.join(d, "lo"), src], capture_output=True, check=True)
        out = os.path.join(d, "lo", "in.csv")
    else:
        out = os.path.join(d, "out.csv")
        subprocess.run(["ssconvert", "--recalc", src, out], capture_output=True, check=True)
    return [r[0] for r in csv.reader(open(out))]


def main():
    lo, gn = run("lo", CASES), run("gn", CASES)
    print("formula\texpected\tsource")
    for (f, e), l, g in zip(CASES, lo, gn):
        want = "x" + "%.15G" % eval(e.replace("^", "**"))
        agree = [n for n, got in (("Gnumeric", g), ("LibreOffice", l)) if got == want]
        if not agree:
            print(f"{f}: LibreOffice {l}, Gnumeric {g}, %.15G {want}", file=sys.stderr)
        print(f'"x" & {f}\t{json.dumps(want)}\t{" and ".join(agree) or "%.15G"}')


main()
