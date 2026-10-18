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
DATE_RESULT = {"EASTERSUNDAY"}


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


# --- special functions -------------------------------------------------------
gn("num", "AGM(1,2)", "AGM(24,6)", "AGM(0,5)", "AGM(3,3)", "AGM(POWER(10,-300),1)", "AGM(-1,2)")
gn("num", "GD(0)", "GD(1)", "GD(-2.5)", "GD(20)", "GD(POWER(10,-10))")
gn("num", "ILOG(1000)", "ILOG(999)", "ILOG(POWER(10,22),10)", "ILOG(0.001,10)", "ILOG(1024,2)",
   "ILOG(1023,2)", "ILOG(80,3)", "ILOG(81,3)", "ILOG(POWER(10,300),7)", "ILOG(0.3,2)", "ILOG(0,10)",
   "ILOG(10,1)", "ILOG(10,0)", "ILOG(10,1.5)")
# Gnumeric truncates toward zero for x < 1 and a base 2^k, k > 1: it says
# ILOG(0.2,4) = -1, but 4^-1 = 0.25 > 0.2.
HAND += [("ILOG(0.2,4)", -2.0, "exact"), ("ILOG(0.0625,4)", -2.0, "exact")]
gn("num", "FIB(1)", "FIB(2)", "FIB(12)", "FIB(46)", "FIB(78)", "FIB(2.9)", "FIB(0)", "FIB(-3)",
   "FIB(1477)")
# Gnumeric's Binet form is inexact past FIB(46) (FIB(47) = 2971215073.000005)
# and overflows its pow() at 1475; the exact values, rounded to a double:
def fib(n):
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a


def partitions(n):
    p = [1] + [0] * n
    for m in range(1, n + 1):
        k = 1
        while k * (3 * k - 1) // 2 <= m:
            sg = 1 if k % 2 else -1
            p[m] += sg * p[m - k * (3 * k - 1) // 2]
            if k * (3 * k + 1) // 2 <= m:
                p[m] += sg * p[m - k * (3 * k + 1) // 2]
            k += 1
    return p[n]


HAND += [(f"FIB({n})", float(fib(n)), "exact") for n in (47, 79, 100, 1476)]

# --- number theory -----------------------------------------------------------
gn("bool", "ISPRIME(2)", "ISPRIME(1)", "ISPRIME(0)", "ISPRIME(-7)", "ISPRIME(97)", "ISPRIME(91)",
   "ISPRIME(1000000007)", "ISPRIME(561)", "ISPRIME(7.9)", "ISPRIME(4503599627370449)")
gn("num", "ITHPRIME(1)", "ITHPRIME(2)", "ITHPRIME(10)", "ITHPRIME(1000)", "ITHPRIME(10000000)",
   "ITHPRIME(0)", "ITHPRIME(2.5)",
   "PFACTOR(2)", "PFACTOR(91)", "PFACTOR(97)", "PFACTOR(1)", "PFACTOR(1000000007)",
   "PFACTOR(4503599627370449)", "PFACTOR(4503599627370495)",
   "NT_D(1)", "NT_D(12)", "NT_D(720720)", "NT_D(0)", "NT_D(4503599627370495)",
   "NT_SIGMA(1)", "NT_SIGMA(12)", "NT_SIGMA(97)", "NT_SIGMA(0)", "NT_SIGMA(1000000)",
   "NT_PHI(1)", "NT_PHI(9)", "NT_PHI(97)", "NT_PHI(1000000)", "NT_PHI(0)",
   "NT_MU(1)", "NT_MU(6)", "NT_MU(30)", "NT_MU(12)", "NT_MU(97)", "NT_MU(0)",
   "NT_OMEGA(1)", "NT_OMEGA(12)", "NT_OMEGA(30030)", "NT_OMEGA(1024)", "NT_OMEGA(0)",
   "NT_PI(-5)", "NT_PI(1)", "NT_PI(2)", "NT_PI(100)", "NT_PI(1000000)", "NT_PI(179424673)",
   "NT_RADICAL(1)", "NT_RADICAL(12)", "NT_RADICAL(1024)", "NT_RADICAL(30030)", "NT_RADICAL(0)",
   "NT_RADICAL(4503599627370495)")
# Gnumeric 1.12.57 has no PARTITIONS (later releases do): exact values.
HAND += [(f"PARTITIONS({n})", float(partitions(int(n))), "exact")
         for n in (0, 5, 100, 1000, 4000, 10.5)]
HAND += [("PARTITIONS(-1)", NUM, "Gnumeric source"),
         ("PARTITIONS(4001)", NUM, "Improv limit")]
# Improv's limits, below Gnumeric's 10⁸-entry prime table (each answers in
# about 2 ms at most; Gnumeric's #LIMIT! is #NUM! here).
HAND += [("ITHPRIME(10000001)", NUM, "Improv limit"), ("NT_PI(179424674)", NUM, "Improv limit"),
         ("ISPRIME(4503599627370497)", NUM, "Improv limit")]

# --- floating point ----------------------------------------------------------
gn("num", "FLT.MAX()", "FLT.MIN()", "FLT.RADIX()", "FLT.NEXTAFTER(1,2)", "FLT.NEXTAFTER(1,0)",
   'FLT.NEXTAFTER(0,"+")', 'FLT.NEXTAFTER(-1,"-")', "FLT.NEXTAFTER(3,3)",
   'FLT.NEXTAFTER(1,"x")')


# --- calendars ---------------------------------------------------------------
# LibreOffice types the result as a date, which p1's reader rejects: N() it.
for y in (2024, 2025, 24, 95, 1583, 9956, 2038.7, 1582, 9957, -1):
    lo(f"EASTERSUNDAY({y})", lo_f=f"N(EASTERSUNDAY({y}))")
gn("date", "ASCENSIONTHURSDAY(2024)", "ASCENSIONTHURSDAY(2025)", "ASCENSIONTHURSDAY(1999)",
   "ASHWEDNESDAY(2024)", "ASHWEDNESDAY(2025)", "ASHWEDNESDAY(1900)",
   "GOODFRIDAY(2024)", "GOODFRIDAY(2025)", "GOODFRIDAY(2000)",
   "PENTECOSTSUNDAY(2024)", "PENTECOSTSUNDAY(2025)", "PENTECOSTSUNDAY(10)")
gn("num", "ASHWEDNESDAY(9957)", "GOODFRIDAY(-1)",
   "DATE2JULIAN(DATE(2001,3,30))", "DATE2JULIAN(DATE(1970,1,1))", "DATE2JULIAN(DATE(2000,1,1))",
   "DATE2JULIAN(DATE(1900,3,1))", "DATE2JULIAN(DATE(2024,12,31))",
   "HDATE_JULIAN(2001,3,30)", "HDATE_JULIAN(2024,2,30)", "HDATE_JULIAN(2024,13,1)",
   "HDATE_JULIAN(2024,0,15)", "HDATE_JULIAN(1582,10,15)", "HDATE_JULIAN(2024.9,1.5,1.9)")

# Hebrew calendar. HDATE_MONTH counts from 0 (Tishri); 12 and 13 are Adar I
# and II. The 1900-system serial dates DATE2HDATE reads agree with Gnumeric's
# from 1900-03-01 on.
gn("text", "HDATE(2024,1,1)", "HDATE(2024,3,24)", "HDATE(2024,10,3)", "HDATE(2023,9,16)",
   "HDATE(2000,2,29)", "HDATE(1,1,1)", "HDATE(2024,2,30)", "HDATE(2024,13,1)", "HDATE(2024,0,1)",
   "HDATE(0,1,1)", "HDATE(2024,1,32)",
   "HDATE_HEB(2024,1,1)", "HDATE_HEB(2024,10,17)", "HDATE_HEB(2024,3,24)", "HDATE_HEB(2025,3,1)",
   "HDATE_HEB(2240,9,1)", "HDATE_HEB(2024,13,1)",
   "DATE2HDATE(DATE(2024,1,1))", "DATE2HDATE(DATE(2024,4,23))", "DATE2HDATE(DATE(1948,5,14))",
   "DATE2HDATE(45000)", "DATE2HDATE(DATE(2024,9,3))",
   "DATE2HDATE_HEB(DATE(2024,3,24))", "DATE2HDATE_HEB(DATE(2024,6,12))",
   "DATE2HDATE_HEB(DATE(2023,12,8))", "DATE2HDATE_HEB(DATE(2025,4,13))")
gn("num", "HDATE_DAY(2024,1,1)", "HDATE_DAY(2024,3,24)", "HDATE_DAY(2023,9,16)",
   "HDATE_DAY(2024,2,30)", "HDATE_DAY(2024,13,1)",
   "HDATE_MONTH(2024,1,1)", "HDATE_MONTH(2024,3,15)", "HDATE_MONTH(2024,3,1)",
   "HDATE_MONTH(2025,3,1)", "HDATE_MONTH(2024,9,1)", "HDATE_MONTH(2024,10,3)",
   "HDATE_MONTH(2024,0,1)",
   "HDATE_YEAR(2024,1,1)", "HDATE_YEAR(2024,10,3)", "HDATE_YEAR(2024,10,2)",
   "HDATE_YEAR(1,1,1)", "HDATE_YEAR(4000,12,31)", "HDATE_YEAR(-5,1,1)")
# Gnumeric's molad arithmetic is C int and overflows past about Gregorian
# 4360 (HDATE(4400,1,1) = "24 Ab 8613"); Improv's is 64-bit. 31 December is
# after 1 Tishri (still before mid-November by 9999), so its Hebrew year is
# the Gregorian year + 3761.
HAND += [("HDATE_YEAR(9999,12,31)", 13760.0, "exact"), ("HDATE_YEAR(4400,12,31)", 8161.0, "exact")]


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
        # Gnumeric's 1900 system counts the fictitious 1900-02-29 (serial 60),
        # so its serials below 60 are one day later than LibreOffice's.
        s = float(v)
        return as_date(s + 1 if s < 60 else s)
    return float(v)


def main():
    vals = p1.run_lo(["PI()"] + [lo_f or p1.to_lo(f) for f, lo_f, _, _ in LO])
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    rows = []
    for (f, _, excel, src), v in zip(LO, vals[1:]):
        if isinstance(v, tuple):
            v = p1.map_error(f, v[1])
        elif p1.outer(f) in DATE_RESULT:
            v = as_date(v)
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
