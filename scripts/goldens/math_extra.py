#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/math_extra_goldens.tsv (P7 math
extras: special functions, number theory, floating point, calendars,
telephone traffic and text).

    python3 scripts/goldens/math_extra.py > crates/engine/tests/data/math_extra_goldens.tsv

Sources, per row:
- `LibreOffice`: the Excel functions (ROMAN, ARABIC, ASC, JIS, BAHTTEXT,
  ROT13, ENCODEURL, the Easter family) through p1.py's runner.
- `Excel docs`: LibreOffice departs from Excel's documentation (`excel=`;
  stderr shows LibreOffice's value).
- `Gnumeric 1.12`: the Gnumeric-only functions, from `ssconvert --recalc`
  (on PATH, or GNUMERIC_SSCONVERT).
- `exact`: where Gnumeric's own result is inexact or an overflow artifact,
  the exact value (mpmath at 60 digits, rounded to the nearest double).

Formulas use Improv syntax: `,` separators, DATE(y,m,d) for dates, no `^`.
"""

import csv
import datetime
import json
import os
import shutil
import subprocess
import sys
import tempfile
from xml.sax.saxutils import escape

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402

err, fmt = p1.err, p1.fmt
NUM, VAL, NA, DIV0 = err("#NUM!"), err("#VALUE!"), err("#N/A"), err("#DIV/0!")
p1.LO_NAMES.update({
    "BAHTTEXT": "COM.MICROSOFT.BAHTTEXT", "ENCODEURL": "COM.MICROSOFT.ENCODEURL",
    "ROT13": "ORG.OPENOFFICE.ROT13",
})
p1.VALUE_502 |= {"ROMAN", "ARABIC", "ASC", "JIS", "BAHTTEXT", "ROT13", "ENCODEURL"}

LO = []    # (formula, LibreOffice formula or None, override or None, override source)
GN = []    # (formula, result kind: num | bool | text | date)
HAND = []  # (formula, value, source)


def lo(f, lo_f=None, excel=None, src="Excel docs"):
    LO.append((f, lo_f, excel, src))


def many(*fs):
    for f in fs:
        lo(f)


def gn(kind, *fs):
    for f in fs:
        GN.append((f, kind))


# --- text --------------------------------------------------------------------
many('ASC("ＡＢＣ　１２３ガパヴカ")', 'ASC("ｶﾀｶﾅ abc")', 'ASC("「テスト」、。")', 'ASC("")',
     'JIS("ABC 123 ｶﾞﾊﾟｳﾞｶ~")', 'JIS("ﾃｽﾄ!")', 'JIS("全角")', 'JIS("a\\b")')
lo('DBCS("abc 1")', lo_f='JIS("abc 1")')
many("BAHTTEXT(1234.5)", "BAHTTEXT(21)", "BAHTTEXT(1000001)", "BAHTTEXT(-0.25)",
     "BAHTTEXT(0)", "BAHTTEXT(100000000)", "BAHTTEXT(0.011)", "BAHTTEXT(111.11)")
many('ROT13("Hello, World!")', 'ROT13("")', 'ROT13("Zebra-123")', 'ROT13("Ärger")',
     'ROT13("NOPQRSTUVWXYZ")')
many('ENCODEURL("a b&c/é")', 'ENCODEURL("")', 'ENCODEURL("x=1;y=2?")', 'ENCODEURL("€100")')
# Excel's documented example keeps '.'; LibreOffice also encodes '.' and '~',
# which RFC 3986 lists as unreserved.
URL = "http://contoso.sharepoint.com/teams/Finance/Documents/April Reports/Profit and Loss Statement.xlsx"
lo(f'ENCODEURL("{URL}")', excel="http%3A%2F%2Fcontoso.sharepoint.com%2Fteams%2FFinance%2FDocuments"
   "%2FApril%20Reports%2FProfit%20and%20Loss%20Statement.xlsx")
lo('ENCODEURL("a.b~c")', excel="a.b~c", src="RFC 3986")
many("ROMAN(499)", "ROMAN(499,0)", "ROMAN(499,1)", "ROMAN(499,2)", "ROMAN(499,3)",
     "ROMAN(499,4)", "ROMAN(1999,4)", "ROMAN(3999)", "ROMAN(0)", "ROMAN(4000)", "ROMAN(-1)",
     "ROMAN(10,5)", "ROMAN(12.9)")
# Excel: TRUE is the classic form 0, FALSE the concise form 4 (LibreOffice
# reads TRUE as 1).
lo("ROMAN(499,TRUE)", excel="CDXCIX")
lo("ROMAN(499,FALSE)", excel="ID")
many('ARABIC("mcmxii")', 'ARABIC("MMMCMXCIX")', 'ARABIC("IIII")', 'ARABIC("")',
     'ARABIC("IM")', 'ARABIC("ABC")', 'ARABIC("MMMM")', 'ARABIC("VX")')
# Excel: a leading minus negates, surrounding spaces are ignored.
lo('ARABIC("-MMXI")', excel=-2011.0)
lo('ARABIC(" X ")', excel=10.0)


# --- runners -----------------------------------------------------------------
def gnumeric(formulas):
    exe = os.environ.get("GNUMERIC_SSCONVERT") or shutil.which("ssconvert")
    if not exe:
        raise SystemExit("needs Gnumeric's ssconvert (on PATH or GNUMERIC_SSCONVERT)")
    with tempfile.TemporaryDirectory(dir=os.path.expanduser("~")) as d:
        src = os.path.join(d, "in.gnumeric")
        cells = "".join(f'<gnm:Cell Row="{i}" Col="0">={escape(f)}</gnm:Cell>'
                        for i, f in enumerate(formulas))
        with open(src, "w", encoding="utf-8") as fh:
            fh.write('<?xml version="1.0"?><gnm:Workbook xmlns:gnm="http://www.gnumeric.org/v10.dtd">'
                     '<gnm:SheetNameIndex><gnm:SheetName>S</gnm:SheetName></gnm:SheetNameIndex>'
                     '<gnm:Sheets><gnm:Sheet><gnm:Name>S</gnm:Name><gnm:MaxCol>1</gnm:MaxCol>'
                     f'<gnm:MaxRow>{len(formulas)}</gnm:MaxRow><gnm:Cells>{cells}</gnm:Cells>'
                     '</gnm:Sheet></gnm:Sheets></gnm:Workbook>')
        out = os.path.join(d, "out.csv")
        subprocess.run([exe, "--recalc", src, out], check=True, capture_output=True)
        with open(out, encoding="utf-8", newline="") as fh:
            vals = [r[0] if r else "" for r in csv.reader(fh)]
    if len(vals) != len(formulas):
        raise SystemExit(f"ssconvert gave {len(vals)} values for {len(formulas)} formulas")
    return vals


def as_date(serial):
    d = datetime.date(1899, 12, 30) + datetime.timedelta(days=int(serial))
    return {"date": d.isoformat() + "T00:00:00Z"}


def gn_value(v, kind):
    if v.startswith("#"):
        # Gnumeric's #LIMIT! (beyond its tables) is Improv's #NUM!.
        return NUM if v == "#LIMIT!" else err(v)
    if kind == "text":
        return v
    if kind == "bool":
        return {"TRUE": True, "FALSE": False}[v]
    if kind == "date":
        return as_date(float(v))
    return float(v)


def main():
    vals = p1.run_lo(["PI()"] + [lo_f or p1.to_lo(f) for f, lo_f, _, _ in LO])
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    rows = []
    for (f, _, excel, src), v in zip(LO, vals[1:]):
        if isinstance(v, tuple):
            v = p1.map_error(f, v[1])
        if excel is not None:
            print(f"LibreOffice differs: {f} LO={fmt(v)} {src}={fmt(excel)}", file=sys.stderr)
            rows.append((f, excel, src))
        else:
            rows.append((f, v, "LibreOffice"))
    if p1.UNMAPPED:
        raise SystemExit("unmapped LibreOffice errors:\n  " + "\n  ".join(p1.UNMAPPED))
    if GN:
        for (f, kind), v in zip(GN, gnumeric([f for f, _ in GN])):
            rows.append((f, gn_value(v, kind), "Gnumeric 1.12"))
    rows += HAND
    for f, _, _ in rows:
        assert "\t" not in f and "^" not in f, f
    print("formula\texpected\tsource")
    for f, v, s in rows:
        print(f"{f}\t{fmt(v) if not isinstance(v, dict) else json.dumps(v)}\t{s}")


if __name__ == "__main__":
    main()
