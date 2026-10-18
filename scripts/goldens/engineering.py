#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/engineering_goldens.tsv (P7 engineering:
bases, bits, complex numbers, Bessel functions, CONVERT).

    python3 scripts/goldens/engineering.py > crates/engine/tests/data/engineering_goldens.tsv

LibreOffice rows run through p1.py's runner (exact doubles rebuilt from
helper cells); Err:502 is #NUM! (Excel's domain error) and Err:504 #VALUE!.
A case marked `excel=...` is one where LibreOffice disagrees with Excel's
documentation; the row records Excel's value (source `Excel docs`) and
stderr shows LibreOffice's.

Gnumeric-only functions (HEXREP, IMNEG, IMINV, the IMARC* families, IMFACT,
IMGAMMA, IMIGAMMA) run through Gnumeric's ssconvert (GNUMERIC_SSCONVERT, or
`ssconvert` on PATH); without it the values recorded in
engineering_gnumeric.json are used (refresh with `--record`).
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
from xml.sax.saxutils import escape

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402  (the LibreOffice runner and helpers)

err = p1.err
NUM, VALUE, NA, DIV0 = err("#NUM!"), err("#VALUE!"), err("#N/A"), err("#DIV/0!")

LO = []  # (formula, Excel's value where LibreOffice differs)
GN = []  # (formula, "text" | "num", tolerance or None): Gnumeric's value


def lo(f, excel=None):
    LO.append((f, excel))


def many(*fs):
    for f in fs:
        lo(f)


def gn(f, kind="text", tol=None):
    GN.append((f, kind, tol))


# --- bases ----------------------------------------------------------------
many('BIN2DEC("1100100")', 'BIN2DEC("1111111111")', 'BIN2DEC(101)', 'BIN2DEC("")',
     'BIN2DEC("11111111111")', 'BIN2DEC("2")',
     'BIN2HEX("11111011",4)', 'BIN2HEX("1110")', 'BIN2HEX("1111111111")',
     'BIN2HEX("1111111111",3)', 'BIN2HEX("1",0)', 'BIN2HEX("1",10.9)',
     'BIN2OCT("1001",3)', 'BIN2OCT("1100100")', 'BIN2OCT("1111111111")', 'BIN2OCT("1001",1)',
     'DEC2BIN(9,4)', 'DEC2BIN(-100)', 'DEC2BIN(9.9)', 'DEC2BIN(-512)', 'DEC2BIN(512)',
     'DEC2BIN(3,0)', 'DEC2BIN(-3,11)',
     'DEC2HEX(100,4)', 'DEC2HEX(-54)', 'DEC2HEX(28)', 'DEC2HEX(549755813887)',
     'DEC2HEX(549755813888)', 'DEC2HEX(64,1)',
     'DEC2OCT(58,3)', 'DEC2OCT(-100)', 'DEC2OCT(536870911)', 'DEC2OCT(-536870913)', 'DEC2OCT(0)',
     'HEX2BIN("F",8)', 'HEX2BIN("B7")', 'HEX2BIN("FFFFFFFE00")', 'HEX2BIN("FFFFFFFDFF")',
     'HEX2BIN("200")',
     'HEX2DEC("A5")', 'HEX2DEC("FFFFFFFF5B")', 'HEX2DEC("3DA408B9")', 'HEX2DEC("fffffffffe")',
     'HEX2DEC("G")', 'HEX2DEC("12345678901")',
     'HEX2OCT("F",3)', 'HEX2OCT("3B4E")', 'HEX2OCT("FFFFFFFF00")', 'HEX2OCT("FFDFFFFFFF")',
     'OCT2BIN("3",3)', 'OCT2BIN("7777777000")', 'OCT2BIN("1000")', 'OCT2BIN("8")',
     'OCT2DEC("54")', 'OCT2DEC("7777777533")', 'OCT2DEC("4000000000")', 'OCT2DEC("19")',
     'OCT2HEX("100",4)', 'OCT2HEX("7777777533")', 'OCT2HEX("7777777777")', 'OCT2HEX("1",11)',
     'BASE(7,2)', 'BASE(100,16)', 'BASE(15,2,10)', 'BASE(5.9,2)', 'BASE(-1,2)', 'BASE(5,37)',
     'BASE(5,2.9)', 'BASE(35,36)',
     'DECIMAL("FF",16)', 'DECIMAL("111",2)', 'DECIMAL("zap",36)', 'DECIMAL("12",2)',
     'DECIMAL("",2)', 'DECIMAL("ff",16.9)', 'DECIMAL("1",1)')
# Excel: number < 2^53 and min_length < 256; LibreOffice converts both.
lo("BASE(9007199254740992,2)", excel=NUM)
lo("BASE(5,2,256)", excel=NUM)
gn("HEXREP(1)")
gn("HEXREP(-2.5)")
gn("HEXREP(0)")
gn("HEXREP(0.1)")

# --- bits -----------------------------------------------------------------
many("BITAND(1,5)", "BITAND(13,25)", "BITAND(3.9,3)", "BITAND(-1,1)",
     "BITAND(281474976710656,1)", "BITAND(281474976710655,255)",
     "BITOR(23,10)", "BITOR(0,0)", "BITOR(-1,0)", "BITOR(281474976710655,1)",
     "BITXOR(5,3)", "BITXOR(5,5)", "BITXOR(1,281474976710656)", "BITXOR(12.5,10)",
     "BITLSHIFT(4,2)", "BITLSHIFT(5,-2)", "BITLSHIFT(1,2.9)",
     "BITLSHIFT(8,-2.5)", "BITLSHIFT(281474976710655,53)", "BITLSHIFT(-1,2)",
     "BITRSHIFT(13,2)", "BITRSHIFT(13,-2)", "BITRSHIFT(16,1.5)",
     "BITRSHIFT(281474976710656,1)")
# Excel: |shift| > 53 is #NUM!; LibreOffice shifts anyway.
lo("BITRSHIFT(1,54)", excel=NUM)
lo("BITLSHIFT(1,60)", excel=NUM)

# --- tests ----------------------------------------------------------------
many("DELTA(5,4)", "DELTA(5,5)", "DELTA(0.5,0)", "DELTA(0)", "DELTA(-1,-1)",
     "GESTEP(5,4)", "GESTEP(5,5)", "GESTEP(-4,-5)", "GESTEP(-1)", "GESTEP(0)")


# --- runners ----------------------------------------------------------------
def map_error(f, shown):
    if shown in p1.LO_ERRORS:
        return err(shown)
    if shown == "Err:502":
        return NUM
    if shown == "Err:504":
        return VALUE
    raise SystemExit(f"unmapped LibreOffice error {shown} for {f}")


def gnumeric(formulas):
    exe = os.environ.get("GNUMERIC_SSCONVERT") or shutil.which("ssconvert")
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
            return fh.read().splitlines()


RECORD = os.path.join(os.path.dirname(os.path.abspath(__file__)), "engineering_gnumeric.json")


def gnumeric_values():
    fs = [f for f, _, _ in GN]
    got = gnumeric(fs)
    if got is None:
        with open(RECORD, encoding="utf-8") as fh:
            rec = json.load(fh)
        got = [rec[f] for f in fs]
    return dict(zip(fs, got))


def gn_value(shown, kind):
    if shown.startswith("#"):
        return err(shown)
    return shown if kind == "text" else float(shown)


def main():
    vals = p1.run_lo(["PI()"] + [p1.to_lo(f) for f, _ in LO])
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    rows = []
    for (f, excel), v in zip(LO, vals[1:]):
        if isinstance(v, tuple):
            v = map_error(f, v[1])
        if excel is not None:
            print(f"LibreOffice differs: {f} LO={p1.fmt(v)} Excel={p1.fmt(excel)}", file=sys.stderr)
            rows.append((f, excel, "Excel docs"))
        else:
            rows.append((f, v, "LibreOffice"))
    shown = gnumeric_values()
    for f, kind, tol in GN:
        v = gn_value(shown[f], kind)
        if tol is not None and not isinstance(v, (dict, str)):
            v = {"approx": v, "tol": tol}
        rows.append((f, v, "Gnumeric 1.12"))
    for f, _, _ in rows:
        assert "\t" not in f and "^" not in f, f
    print("formula\texpected\tsource")
    for f, v, s in rows:
        print(f"{f}\t{p1.fmt(v)}\t{s}")


if __name__ == "__main__":
    if sys.argv[1:] == ["--record"]:
        fs = [f for f, _, _ in GN]
        with open(RECORD, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(dict(zip(fs, gnumeric(fs))), indent=1) + "\n")
    else:
        main()
