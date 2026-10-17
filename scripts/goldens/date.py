#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/date_goldens.tsv (the P2 date family:
information, text formatting and regular expressions, date and time).

    python3 scripts/goldens/date.py > crates/engine/tests/data/date_goldens.tsv

LibreOffice rows run through p1.py's machinery (one flat-ODS sheet, exact
doubles rebuilt from helper cells, errors mapped to Excel's). A function that
returns a date in Improv is wrapped in N() for LibreOffice, and the exact
serial number is written as `{"date": "...Z"}` (serial 0 = 1899-12-30, the
same epoch in both). ISLEAPYEAR's 1/0 becomes TRUE/FALSE.

A case marked `excel=...` is one where LibreOffice disagrees with Excel's
documentation; the row records Excel's value (source `Excel docs`) and stderr
shows LibreOffice's.

Hand rows cover functions LibreOffice lacks:
- REGEXTEST, REGEXEXTRACT, REGEXREPLACE, VALUETOTEXT: values follow the Excel
  2024 documentation's semantics (patterns kept to the syntax common to ICU
  and Rust's regex crate; anchors are \\A and \\z, since formulas avoid ^).
- ISOYEAR, UNIX2DATE, DATE2UNIX: Gnumeric's definitions
  (plugins/fn-date/functions.c), computed with Python's datetime.
- DAYOFYEAR: Quantrix (assumed from the functions plan, not verified).
- ISDATE, ISNUMBERORDATE, DAYNAME, MONTHNAME, DUR2*, STRIPDURATION: Apple
  Numbers' function help. Improv has no duration type, so a duration is a
  number of days and DUR2HOURS(1.5) is 36.

- DATEVALUE/TIMEVALUE of an ISO 8601 date-time (`2025-01-31T09:30:00Z`):
  ISO 8601 itself; LibreOffice rejects the form.

NOW and TODAY change with the clock and have no golden rows.
Formulas use Improv syntax: `,` separators, TRUE/FALSE literals, no `^`.
"""

import datetime as dt
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import p1  # noqa: E402

err, T, F = p1.err, True, False
EPOCH = dt.datetime(1899, 12, 30)

p1.LO_NAMES.update({
    "NETWORKDAYS.INTL": "COM.MICROSOFT.NETWORKDAYS.INTL",
    "WORKDAY.INTL": "COM.MICROSOFT.WORKDAY.INTL",
    "REGEX": "ORG.LIBREOFFICE.REGEX",
    "DAYSINMONTH": "COM.SUN.STAR.SHEET.ADDIN.DATEFUNCTIONS.GETDAYSINMONTH",
    "DAYSINYEAR": "COM.SUN.STAR.SHEET.ADDIN.DATEFUNCTIONS.GETDAYSINYEAR",
    "WEEKSINYEAR": "COM.SUN.STAR.SHEET.ADDIN.DATEFUNCTIONS.GETWEEKSINYEAR",
    "ISLEAPYEAR": "COM.SUN.STAR.SHEET.ADDIN.DATEFUNCTIONS.GETISLEAPYEAR",
    "MONTHS": "COM.SUN.STAR.SHEET.ADDIN.DATEFUNCTIONS.GETDIFFMONTHS",
    "WEEKS": "COM.SUN.STAR.SHEET.ADDIN.DATEFUNCTIONS.GETDIFFWEEKS",
    "YEARS": "COM.SUN.STAR.SHEET.ADDIN.DATEFUNCTIONS.GETDIFFYEARS",
})
# Err:502 is #VALUE! (not #NUM!) for these.
p1.VALUE_502 |= {"DATEVALUE", "TIMEVALUE", "TEXT", "FIXED", "DOLLAR", "NUMBERVALUE", "REGEX"}

# (formula, excel override or None, kind): kind "date" or "bool" converts.
LO = []


def lo(f, excel=None, kind=None):
    LO.append((f, excel, kind))


def many(*fs, kind=None):
    for f in fs:
        lo(f, kind=kind)


def D(y, m, d):
    return f"DATE({y},{m},{d})"


# --- information --------------------------------------------------------------
# LibreOffice booleans are numbers; Excel's are not.
lo("ISNUMBER(TRUE)", excel=F)
lo("TYPE(TRUE)", excel=4.0)
many('ISBLANK("")', "ISBLANK(0)", 'ISBLANK("x")', "ISBLANK(NA())",
     "ISNUMBER(1)", 'ISNUMBER("1")', "ISNUMBER(NA())",
     f"ISNUMBER({D(2025, 1, 1)})", "ISNUMBER(SQRT(-1))",
     'ISTEXT("a")', "ISTEXT(1)", 'ISTEXT("")', "ISTEXT(NA())", "ISTEXT(TRUE)",
     'ISNONTEXT("a")', "ISNONTEXT(1)", "ISNONTEXT(NA())", "ISNONTEXT(TRUE)", 'ISNONTEXT("")',
     "ISLOGICAL(TRUE)", "ISLOGICAL(1)", 'ISLOGICAL("TRUE")', "ISLOGICAL(NA())", "ISLOGICAL(FALSE)",
     "ISEVEN(2)", "ISEVEN(3)", "ISEVEN(-2.5)", "ISEVEN(0)", "ISEVEN(2.9)", "ISEVEN(-1)",
     "ISODD(3)", "ISODD(2)", "ISODD(-3.7)", "ISODD(0)", "ISODD(1.5)",
     "N(5)", 'N("7")', "N(TRUE)", "N(FALSE)", f"N({D(2025, 1, 31)})", "N(NA())", "N(-2.5)",
     'T("a")', "T(1)", "T(TRUE)", 'T("")', "T(NA())", f"T({D(2025, 1, 1)})",
     "TYPE(1)", 'TYPE("a")', "TYPE(NA())", f"TYPE({D(2025, 1, 1)})",
     "TYPE(SQRT(-1))")

# --- text formatting ------------------------------------------------------------
many('TEXT(1234.567,"#,##0.00")', 'TEXT(1234.567,"0")', 'TEXT(1234567,"#,##0")',
     'TEXT(0.285,"0.0%")', 'TEXT(0.5,"0%")', 'TEXT(1234.5,"0.00E+00")', 'TEXT(0.000123,"0.00E+00")',
     'TEXT(-5,"0;(0)")', 'TEXT(-1234.5,"#,##0.00;-#,##0.00")', 'TEXT(0,"0.00")',
     'TEXT(5,"\\$0")', 'TEXT(3.5,"0.0 \\h")', 'TEXT(7,"00")', 'TEXT(2.5,"#.##")',
     'TEXT("abc","0")',
     f'TEXT({D(2025, 1, 5)},"dddd, mmmm d, yyyy")', f'TEXT({D(2025, 1, 5)},"ddd mmm dd yy")',
     f'TEXT({D(2025, 3, 7)},"yyyy-mm-dd")', f'TEXT({D(2025, 3, 7)},"m/d/yyyy")',
     'TEXT(45724.5625,"yyyy-mm-dd hh:mm:ss")', 'TEXT(45724.75,"h:mm AM/PM")',
     'TEXT(0.25,"h:mm AM/PM")', 'TEXT(0.5,"hh:mm")', 'TEXT(0.0006944444444444445,"mm:ss")')
many("FIXED(1234.567,1)", "FIXED(1234.567,-1)", "FIXED(-1234.567,-1,TRUE)", "FIXED(44.332)",
     "FIXED(0.5,0)", "FIXED(-0.004)", "FIXED(1234567.891,2,TRUE)", "FIXED(2.675,2)")
lo("DOLLAR(-1234.567,-2)", excel="($1,200)")
lo("DOLLAR(-0.123,4)", excel="($0.1230)")
many("DOLLAR(1234.567,2)", "DOLLAR(1234.567,-2)", "DOLLAR(99.888)", "DOLLAR(0)",
     "DOLLAR(1234567.5,0)")
# Excel's default separators are the locale's (en-US . and ,); LibreOffice
# rejects these.
lo('NUMBERVALUE("3.5%")', excel=0.035)
lo('NUMBERVALUE("1,234.5")', excel=1234.5)
lo('NUMBERVALUE("")', excel=0.0)
many('NUMBERVALUE("2.500,27",",",".")', 'NUMBERVALUE(" 12 ")', 'NUMBERVALUE("1e3")',
     'NUMBERVALUE("abc")', 'NUMBERVALUE("1 234,5",","," ")', 'NUMBERVALUE("9%%")')
many('REGEX("Hello World","o")', 'REGEX("abc","z")', 'REGEX("Hello World","o","0")',
     'REGEX("Hello World","o","0","g")', 'REGEX("Hello World","o","0",2)',
     'REGEX("abc","(b)","[$1]")', 'REGEX("2025-01-31","([0-9]+)-([0-9]+)","$2/$1")',
     'REGEX("aaa","a","b",4)')

# --- date and time ----------------------------------------------------------------
lo(D(108, 1, 2), excel={"date": "2008-01-02T00:00:00Z"}, kind="date")
many(D(2025, 1, 31), D(2025, 14, 1), D(2025, 1, 0), D(2024, 2, 29),
     D(2025, -1, 15), D(2025, 1, 32), D(1900, 3, 1), "DATE(2025.9,1.9,1.9)", kind="date")
lo(D(-1, 1, 1), kind="date")
lo(D(10000, 1, 1), excel=err("#NUM!"), kind="date")
many('DATEVALUE("2025-01-31")', 'DATEVALUE("1/31/2025")', 'DATEVALUE("2/29/2024")',
     'DATEVALUE("2/30/2025")', 'DATEVALUE("hello")', kind="date")
many("TIME(9,30,0)", "TIME(25,0,0)", "TIME(0,90,0)", "TIME(12,0,30)", "TIME(0,0,-1)",
     "TIME(23,59,59)", kind="date")
many('TIMEVALUE("09:30")', 'TIMEVALUE("9:30 PM")', 'TIMEVALUE("12:00 AM")',
     'TIMEVALUE("23:59:59")', 'TIMEVALUE("abc")', 'TIMEVALUE("12:30 PM")')
many(f"YEAR({D(2025, 1, 31)})", "YEAR(45658)", "YEAR(0)", "YEAR(2958465)",
     f"MONTH({D(2025, 12, 1)})", "MONTH(45658.75)", "MONTH(0)",
     f"DAY({D(2024, 2, 29)})", "DAY(0)", "DAY(45688.99)",
     "HOUR(0.75)", "HOUR(45658.5)", "HOUR(TIME(9,30,15))", "HOUR(1.999999)",
     "MINUTE(TIME(9,30,15))", "MINUTE(0.999)", "MINUTE(0.5)",
     "SECOND(TIME(9,30,15))", "SECOND(0.5000057870370371)", "SECOND(0.999)")
SUN, WED = D(2025, 1, 5), D(2025, 1, 8)
many(f"WEEKDAY({SUN})", f"WEEKDAY({SUN},2)", f"WEEKDAY({SUN},3)", f"WEEKDAY({SUN},11)",
     f"WEEKDAY({SUN},13)", f"WEEKDAY({SUN},17)", f"WEEKDAY({SUN},4)", f"WEEKDAY({WED},16)",
     f"WEEKDAY({WED},12)", f"WEEKDAY({WED},14)", f"WEEKDAY({WED},15)", "WEEKDAY(1)")
many(f"WEEKNUM({D(2025, 1, 1)})", f"WEEKNUM({SUN})", f"WEEKNUM({SUN},2)",
     f"WEEKNUM({D(2024, 12, 31)},21)", f"WEEKNUM({D(2025, 3, 15)},11)",
     f"WEEKNUM({D(2025, 3, 15)},15)", f"WEEKNUM({D(2025, 1, 1)},3)",
     f"WEEKNUM({D(2022, 1, 1)},2)", f"WEEKNUM({D(2025, 3, 15)},17)")
# LibreOffice puts the last days of December in week 1 of the next year when
# that week holds the next January 1; Excel keeps counting (53, or 54).
lo(f"WEEKNUM({D(2025, 12, 31)},1)", excel=53.0)
lo(f"WEEKNUM({D(2000, 12, 31)})", excel=54.0)
lo(f"WEEKNUM({D(2026, 12, 31)},2)", excel=53.0)
many(f"ISOWEEKNUM({D(2024, 12, 30)})", f"ISOWEEKNUM({D(2021, 1, 3)})",
     f"ISOWEEKNUM({D(2025, 6, 15)})", f"ISOWEEKNUM({D(2026, 12, 31)})", "ISOWEEKNUM(45658)")
many(f"DAYS({D(2025, 3, 15)},{D(2025, 1, 1)})", f"DAYS({D(2025, 1, 1)},{D(2025, 3, 15)})",
     "DAYS(45658,45657)", f"DAYS({D(2024, 3, 1)},{D(2024, 2, 1)})")
many(f"DAYS360({D(2025, 1, 30)},{D(2025, 2, 1)})", f"DAYS360({D(2025, 1, 31)},{D(2025, 3, 31)})",
     f"DAYS360({D(2024, 2, 29)},{D(2024, 3, 31)})", f"DAYS360({D(2024, 2, 29)},{D(2024, 3, 31)},TRUE)",
     f"DAYS360({D(2025, 3, 31)},{D(2025, 1, 1)})", f"DAYS360({D(2025, 2, 28)},{D(2025, 3, 31)})",
     f"DAYS360({D(2025, 3, 31)},{D(2025, 1, 1)},TRUE)", f"DAYS360({D(2025, 1, 1)},{D(2025, 12, 31)})")
A, B = D(2001, 6, 1), D(2002, 8, 15)
many(f'DATEDIF({D(2001, 1, 1)},{D(2003, 1, 1)},"Y")', f'DATEDIF({A},{B},"D")',
     f'DATEDIF({A},{B},"YD")', f'DATEDIF({A},{B},"MD")', f'DATEDIF({A},{B},"YM")',
     f'DATEDIF({A},{B},"M")', f'DATEDIF({D(2025, 1, 31)},{D(2025, 3, 1)},"MD")',
     f'DATEDIF({B},{A},"D")', f'DATEDIF({A},{B},"x")', f'DATEDIF({D(2024, 3, 15)},{D(2025, 2, 10)},"yd")',
     f'DATEDIF({D(2024, 2, 29)},{D(2025, 2, 28)},"y")')
many(f"EDATE({D(2025, 1, 31)},1)", f"EDATE({D(2024, 1, 31)},1)", f"EDATE({D(2025, 3, 31)},-1)",
     f"EDATE({D(2025, 1, 15)},12)", "EDATE(45658.75,0)", f"EDATE({D(2025, 1, 15)},1.9)",
     f"EOMONTH({D(2025, 1, 15)},0)", f"EOMONTH({D(2025, 1, 15)},1)", f"EOMONTH({D(2024, 1, 15)},1)",
     f"EOMONTH({D(2025, 1, 15)},-2)", f"EOMONTH({D(2025, 12, 31)},2.9)", kind="date")
YF = [(D(2012, 1, 1), D(2012, 7, 30)), (D(2024, 2, 29), D(2025, 2, 28)),
      (D(2025, 1, 31), D(2025, 3, 31)), (D(2023, 3, 1), D(2025, 6, 15))]
for s, e in YF:
    for basis in range(5):
        lo(f"YEARFRAC({s},{e},{basis})")
many(f"YEARFRAC({D(2012, 7, 30)},{D(2012, 1, 1)})", f"YEARFRAC({D(2012, 1, 1)},{D(2012, 7, 30)},5)",
     f"YEARFRAC({D(2024, 2, 28)},{D(2024, 2, 29)},0)", f"YEARFRAC({D(2023, 2, 28)},{D(2024, 2, 29)},0)",
     f"YEARFRAC({D(2024, 1, 1)},{D(2024, 12, 31)},1)", f"YEARFRAC({D(2023, 6, 1)},{D(2024, 3, 1)},1)")
J1, J31 = D(2025, 1, 1), D(2025, 1, 31)
many(f"NETWORKDAYS({J1},{J31})", f"NETWORKDAYS({J31},{J1})",
     f"NETWORKDAYS({D(2025, 1, 4)},{D(2025, 1, 5)})", f"NETWORKDAYS({J1},{J1})",
     f"NETWORKDAYS({D(2024, 1, 1)},{D(2024, 12, 31)})")
for wk in ("1", "2", "7", "11", "17", '"0000011"', '"1111100"', '"0000000"', '"1111111"', "8",
           '"000001"'):
    lo(f"NETWORKDAYS.INTL({J1},{J31},{wk})")
lo(f'NETWORKDAYS.INTL({J1},{J31},"0000012")', excel=err("#VALUE!"))
lo(f"NETWORKDAYS.INTL({J31},{J1})")
many(f"WORKDAY({J1},5)", f"WORKDAY({D(2025, 1, 3)},1)", f"WORKDAY({D(2025, 1, 6)},-1)",
     f"WORKDAY({D(2025, 1, 4)},0)", f"WORKDAY({J1},100)", f"WORKDAY({J1},-100)",
     f"WORKDAY({D(2025, 1, 4)},1)", f"WORKDAY({J1},2.9)", kind="date")
for wk in ("7", '"0000001"', "11", '"1111100"', '"1111111"', "3", "99", '"01"'):
    lo(f"WORKDAY.INTL({J1},5,{wk})", kind="date")
lo(f"WORKDAY.INTL({J1},-7,13)", kind="date")
many(f"DAYSINMONTH({D(2024, 2, 1)})", f"DAYSINMONTH({D(2025, 2, 15)})",
     f"DAYSINMONTH({D(2025, 12, 31)})", f"DAYSINMONTH({D(2025, 4, 1)})", "DAYSINMONTH(45658)",
     f"DAYSINYEAR({D(2024, 5, 1)})", f"DAYSINYEAR({D(2025, 5, 1)})", f"DAYSINYEAR({D(1900, 3, 1)})",
     f"DAYSINYEAR({D(2000, 1, 1)})",
     f"WEEKSINYEAR({D(2020, 1, 1)})", f"WEEKSINYEAR({D(2025, 1, 1)})", f"WEEKSINYEAR({D(2026, 6, 1)})",
     f"WEEKSINYEAR({D(2015, 1, 1)})", f"WEEKSINYEAR({D(2021, 12, 31)})")
for y in (2024, 2025, 1900, 2000):
    lo(f"ISLEAPYEAR({D(y, 6, 1)})", kind="bool")
for s, e in ((D(2025, 1, 31), D(2025, 3, 1)), (D(2025, 1, 15), D(2025, 3, 15)),
             (D(2025, 3, 1), D(2025, 1, 31)), (D(2024, 2, 29), D(2025, 2, 28))):
    for ty in (0, 1):
        lo(f"MONTHS({s},{e},{ty})")
for s, e in ((D(2025, 1, 4), D(2025, 1, 6)), (D(2025, 1, 1), D(2025, 3, 1)),
             (D(2025, 3, 1), D(2025, 1, 1)), (D(2025, 1, 6), D(2025, 1, 12))):
    for ty in (0, 1):
        lo(f"WEEKS({s},{e},{ty})")
for s, e in ((D(2024, 2, 29), D(2025, 2, 28)), (D(2020, 6, 1), D(2025, 5, 31)),
             (D(2025, 6, 1), D(2020, 6, 1)), (D(2024, 12, 31), D(2025, 1, 1))):
    for ty in (0, 1):
        lo(f"YEARS({s},{e},{ty})")

# --- hand rows --------------------------------------------------------------------
EXCEL, NUMBERS, QUANTRIX = "Excel docs", "Numbers docs", "Quantrix docs"
GD = "Gnumeric source plugins/fn-date/functions.c"
ISO = "ISO 8601"


def iso(t):
    s = t.strftime("%Y-%m-%dT%H:%M:%S")
    if t.microsecond:
        s += f".{t.microsecond // 1000:03d}"
    return {"date": s + "Z"}


def date_of(serial):
    return iso(EPOCH + dt.timedelta(milliseconds=round(serial * 86400000)))


DAYS_ = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]
UNIX0 = dt.datetime(1970, 1, 1)
HAND = [
    ('REGEXTEST("Hello","\\AH")', T, EXCEL), ('REGEXTEST("Hello","\\Ah")', F, EXCEL),
    ('REGEXTEST("Hello","\\Ah",1)', T, EXCEL), ('REGEXTEST("abc123","[0-9]+")', T, EXCEL),
    ('REGEXTEST("abc","[0-9]")', F, EXCEL), ('REGEXTEST("","\\A\\z")', T, EXCEL),
    ('REGEXEXTRACT("DylanWilliams","[A-Z][a-z]+")', "Dylan", EXCEL),
    ('REGEXEXTRACT("abc","[0-9]+")', err("#N/A"), EXCEL),
    ('REGEXEXTRACT("ABC","b",0,1)', "B", EXCEL),
    ('REGEXEXTRACT("Phone: 555-1234","[0-9]{3}-[0-9]{4}")', "555-1234", EXCEL),
    ('REGEXEXTRACT("ABC","b")', err("#N/A"), EXCEL),
    ('REGEXREPLACE("aaa","a","b")', "bbb", EXCEL), ('REGEXREPLACE("aaa","a","b",2)', "aba", EXCEL),
    ('REGEXREPLACE("aaa","a","b",-1)', "aab", EXCEL), ('REGEXREPLACE("AaA","a","x",0,1)', "xxx", EXCEL),
    ('REGEXREPLACE("abc","z","y")', "abc", EXCEL),
    ('REGEXREPLACE("John Smith","([A-Z][a-z]+) ([A-Z][a-z]+)","$2, $1")', "Smith, John", EXCEL),
    ('REGEXREPLACE("aaa","a","b",5)', "aaa", EXCEL),
    ('VALUETOTEXT("Seattle")', "Seattle", EXCEL), ('VALUETOTEXT("Seattle",1)', '"Seattle"', EXCEL),
    ("VALUETOTEXT(TRUE)", "TRUE", EXCEL), ("VALUETOTEXT(1234.01)", "1234.01", EXCEL),
    ("VALUETOTEXT(NA())", "#N/A", EXCEL), ("VALUETOTEXT(1234.01,1)", "1234.01", EXCEL),
    ("ISDATE(DATE(2025,1,1))", T, NUMBERS), ("ISDATE(45658)", F, NUMBERS), ('ISDATE("x")', F, NUMBERS),
    ("ISDATE(TRUE)", F, NUMBERS), ("ISDATE(EDATE(DATE(2025,1,31),1))", T, NUMBERS),
    ("ISNUMBERORDATE(DATE(2025,1,1))", T, NUMBERS), ("ISNUMBERORDATE(5)", T, NUMBERS),
    ('ISNUMBERORDATE("5")', F, NUMBERS), ("ISNUMBERORDATE(TRUE)", F, NUMBERS),
    ("DAYNAME(1)", "Sunday", NUMBERS), ("DAYNAME(7)", "Saturday", NUMBERS),
    ("DAYNAME(DATE(2025,1,5))", "Sunday", NUMBERS), ("DAYNAME(DATE(2025,1,8))", "Wednesday", NUMBERS),
    ("MONTHNAME(1)", "January", NUMBERS), ("MONTHNAME(12)", "December", NUMBERS),
    ("MONTHNAME(6)", "June", NUMBERS), ("MONTHNAME(3)", "March", NUMBERS),
    ("DUR2DAYS(2)", 2.0, NUMBERS), ("DUR2DAYS(0.25)", 0.25, NUMBERS),
    ("DUR2HOURS(1.5)", 36.0, NUMBERS), ("DUR2HOURS(0.125)", 3.0, NUMBERS),
    ("DUR2MINUTES(0.5)", 720.0, NUMBERS), ("DUR2MINUTES(1)", 1440.0, NUMBERS),
    ("DUR2SECONDS(1)", 86400.0, NUMBERS), ("DUR2SECONDS(0.5)", 43200.0, NUMBERS),
    ("DUR2MILLISECONDS(0.001)", 0.001 * 86400000, NUMBERS), ("DUR2MILLISECONDS(1)", 86400000.0, NUMBERS),
    ("DUR2WEEKS(14)", 2.0, NUMBERS), ("DUR2WEEKS(3.5)", 0.5, NUMBERS),
    ("STRIPDURATION(3.5)", 3.5, NUMBERS), ("STRIPDURATION(-1)", -1.0, NUMBERS),
    # ISO 8601 date-times, which LibreOffice's DATEVALUE/TIMEVALUE reject.
    ('DATEVALUE("2025-01-31T09:30:00Z")', {"date": "2025-01-31T00:00:00Z"}, ISO),
    ('DATEVALUE("2024-02-29T23:59:59Z")', {"date": "2024-02-29T00:00:00Z"}, ISO),
    ('TIMEVALUE("2025-01-31T09:30:00Z")', 9.5 / 24, ISO),
    ('TIMEVALUE("2025-01-31T18:00:00Z")', 0.75, ISO),
]
for y, m, d in ((2024, 12, 30), (2021, 1, 3), (2025, 6, 15), (2027, 1, 1), (2026, 12, 31)):
    HAND.append((f"ISOYEAR({D(y, m, d)})", float(dt.date(y, m, d).isocalendar()[0]), GD))
for y, m, d in ((2025, 1, 1), (2024, 12, 31), (2025, 3, 1), (2024, 3, 1), (2025, 12, 31)):
    HAND.append((f"DAYOFYEAR({D(y, m, d)})", float(dt.date(y, m, d).timetuple().tm_yday), QUANTRIX))
for t in ("0", "86400", "1738281600", "-86400", "1.5", "1000000000"):
    HAND.append((f"UNIX2DATE({t})", iso(UNIX0 + dt.timedelta(seconds=float(t))), GD))
for f, when in ((D(1970, 1, 1), dt.datetime(1970, 1, 1)), (D(2025, 1, 31), dt.datetime(2025, 1, 31)),
                ("25569.5", dt.datetime(1970, 1, 1, 12)), (D(1960, 1, 1), dt.datetime(1960, 1, 1))):
    HAND.append((f"DATE2UNIX({f})", float((when - UNIX0).total_seconds()), GD))


def main():
    vals = p1.run_lo(["PI()"] + [("N(%s)" if k == "date" else "%s") % p1.to_lo(f)
                                for f, _, k in LO])
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    rows = []
    for (f, excel, kind), v in zip(LO, vals[1:]):
        if isinstance(v, tuple):
            v = p1.map_error(f, v[1])
        elif kind == "date" and excel is None:
            v = date_of(v)
        elif kind == "bool":
            v = bool(v)
        if excel is not None:
            print(f"LibreOffice differs: {f} LO={p1.fmt(v)} Excel={p1.fmt(excel)}", file=sys.stderr)
            rows.append((f, excel, EXCEL))
        else:
            rows.append((f, v, "LibreOffice"))
    if p1.UNMAPPED:
        raise SystemExit("unmapped LibreOffice errors:\n  " + "\n  ".join(p1.UNMAPPED))
    rows += HAND
    for f, v, _ in rows:
        assert "\t" not in f and "^" not in f, f
        assert not (isinstance(v, float) and math.isnan(v)), f
    print("formula\texpected\tsource")
    for f, v, s in rows:
        print(f"{f}\t{p1.fmt(v)}\t{s}")


if __name__ == "__main__":
    main()
