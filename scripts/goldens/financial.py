#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/financial_goldens.tsv (P5 financial
scalar functions: time value of money, depreciation, bonds and securities).

    python3 scripts/goldens/financial.py > crates/engine/tests/data/financial_goldens.tsv

LibreOffice rows run through p1.py's runner (one flat-ODS sheet, recalculated
by `soffice --headless`, exact doubles rebuilt from helper cells). The
COUPNCD/COUPPCD serial results become `{"date": ...}` values.

Some rows come from elsewhere, each marked with its source:
- `Excel docs`: LibreOffice departs from Excel's documented formula
  (`excel=`; stderr shows LibreOffice's value). These are the documented
  examples, rounded as documented, so they carry an `approx` tolerance.
- `Gnumeric 1.12`: LibreOffice cannot compute it. Its ODDFPRICE and
  ODDFYIELD always fail. Gnumeric's ssconvert computes these rows when
  GNUMERIC_SSCONVERT names its binary; otherwise the values recorded here
  are used.
- Iterative yields (YIELD, ODDFYIELD) are compared within 1e-9. LibreOffice
  and Gnumeric stop their own searches less tightly than Improv's 1e-10
  Newton step does.

Formulas use Improv syntax: `,` separators, DATE(y,m,d) for dates, no `^`.
"""

import datetime
import json
import os
import subprocess
import sys
import tempfile
from xml.sax.saxutils import escape

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from p1 import UNMAPPED, err, fmt, map_error, run_lo, to_lo  # noqa: E402

NUM, VAL, DIV0 = err("#NUM!"), err("#VALUE!"), err("#DIV/0!")
LO = []  # (formula, excel override or None, tolerance or None)


def lo(f, excel=None, tol=None):
    LO.append((f, excel, tol))


def many(*fs):
    for f in fs:
        lo(f)


def approx(x, tol):
    return {"approx": x, "tol": tol}


S1, M1 = "DATE(2008,2,15)", "DATE(2017,11,15)"  # Excel's PRICE example
# --- time value of money -----------------------------------------------------
many("FV(0.06/12,10,-200,-500,1)", "FV(0.11/12,35,-2000,0,1)", "FV(0.12/12,12,-1000)",
     "FV(0,12,-100,-1000)", "FV(0.05,10,-100,-1000,2)", "FV(-0.5,4,10)")
many("PV(0.08/12,12*20,500,0,0)", "PV(0.05,10,-100,1000,1)", "PV(0,10,-100)",
     "PV(0.1,5,0,1000)", "PV(0.05/12,360,-1073.64)", "PV(0,10,-100,0,1)")
many("PMT(0.05/12,360,200000)", "PMT(0.08/12,10,10000)", "PMT(0.08/12,10,10000,0,1)",
     "PMT(0.06/12,18*12,0,50000)", "PMT(0,10,1000)", "PMT(0,0,1000)", "PMT(0.1,0,100)")
many("IPMT(0.1/12,1,3*12,8000)", "IPMT(0.1,3,3,8000)", "IPMT(0.1,3,3,8000,0,1)",
     "IPMT(0.1,1,3,8000,0,1)", "IPMT(0,2,10,1000)", "IPMT(0.1,0,3,8000)", "IPMT(0.1,4,3,8000)")
many("PPMT(0.1/12,1,24,2000)", "PPMT(0.08,10,10,200000)", "PPMT(0.08,10,10,200000,0,1)",
     "PPMT(0.05,1,5,1000,100)", "PPMT(0,3,10,1000)", "PPMT(0.08,11,10,200000)")
many("NPER(0.12/12,-100,-1000,10000,1)", "NPER(0.12/12,-100,-1000,10000)",
     "NPER(0.12/12,-100,-1000)", "NPER(0,-100,1000)", "NPER(0.05,-100,1000,-1000)",
     "NPER(0,0,100)", "NPER(0.1,-10,100,0,2)")
# RATE: LibreOffice stops its Newton search near 1e-7 steps, so its roots
# can be ~1e-11 off (the exact root of RATE(48,-200,8000) is
# 0.00770147248820204, LibreOffice gives 0.00770147248823279).
for f in ("RATE(4*12,-200,8000)", "RATE(48,-200,8000,0,1)", "RATE(10,0,-100,200)",
          "RATE(360,-1073.6432460242781,200000,0,0,0.5)", "RATE(10,-100,1000)",
          "RATE(10,-100,10000)"):
    lo(f, tol=1e-9)
many("RATE(0,-100,1000)", "RATE(5,100,1000)")
many("CUMIPMT(0.09/12,30*12,125000,13,24,0)", "CUMIPMT(0.09/12,30*12,125000,1,1,0)",
     "CUMIPMT(0.09/12,30*12,125000,1,12,1)", "CUMIPMT(0.05,10,1000,2.9,5.5,0)",
     "CUMIPMT(0,30,1000,1,1,0)", "CUMIPMT(0.1,10,1000,5,4,0)", "CUMIPMT(0.1,10,1000,1,2,2)",
     "CUMIPMT(0.1,10,1000,0,2,0)")
many("CUMPRINC(0.09/12,30*12,125000,13,24,0)", "CUMPRINC(0.09/12,30*12,125000,1,1,0)",
     "CUMPRINC(0.09/12,30*12,125000,1,12,1)", "CUMPRINC(0.05,10,1000,3,10,1)",
     "CUMPRINC(0.1,10,0,1,2,0)", "CUMPRINC(0.1,10,1000,1,11,0)")
many("ISPMT(0.1/12,1,36,8000000)", "ISPMT(0.1,1,3,8000000)", "ISPMT(0.1,0,4,4000)",
     "ISPMT(0.1,4,4,4000)", "ISPMT(0,2,4,4000)", "ISPMT(0.1,1,0,1000)")
many("EFFECT(0.0525,4)", "EFFECT(0.1,1)", "EFFECT(0.1,12.9)", "EFFECT(0.1,0.5)",
     "EFFECT(-0.1,4)")
lo("EFFECT(0,4)", excel=NUM)
many("NOMINAL(0.053543,4)", "NOMINAL(0.1,1)", "NOMINAL(0.1,12.5)", "NOMINAL(0,4)",
     "NOMINAL(0.1,0)")
many("PDURATION(0.025,2000,2200)", "PDURATION(0.025/12,1000,1200)", "PDURATION(0.1,100,50)",
     "PDURATION(0,100,200)", "PDURATION(0.1,-100,200)")
many("RRI(96,10000,11000)", "RRI(10,100,200)", "RRI(2,100,25)", "RRI(0,100,200)",
     "RRI(10,0,200)", "RRI(10,-100,200)")
many("DOLLARDE(1.02,16)", "DOLLARDE(1.1,32)", "DOLLARDE(-1.02,16)", "DOLLARDE(1.5,10)",
     "DOLLARDE(1.02,16.9)", "DOLLARDE(1.02,-1)")
lo("DOLLARDE(1.02,0.5)", excel=DIV0)
many("DOLLARFR(1.125,16)", "DOLLARFR(1.125,32)", "DOLLARFR(-1.125,16)", "DOLLARFR(2.5,8)",
     "DOLLARFR(1.125,-2)")
lo("DOLLARFR(1.125,0)", excel=DIV0)

# --- depreciation --------------------------------------------------------------
many("SLN(30000,7500,10)", "SLN(10000,1000,5)", "SLN(1000,1000,3)", "SLN(1000,2000,4)",
     "SLN(100,10,0)")
many("SYD(30000,7500,10,1)", "SYD(30000,7500,10,10)", "SYD(10000,1000,5,3)",
     "SYD(10000,1000,5,6)", "SYD(10000,1000,5,2.5)")
many("DB(1000000,100000,6,1,7)", "DB(1000000,100000,6,2,7)", "DB(1000000,100000,6,6,7)",
     "DB(1000000,100000,6,7,7)", "DB(1000000,100000,6,3)", "DB(1000,100,3,5)",
     "DB(1000,100,3,1,13)", "DB(1000,2000,3,1)", "DB(1000,100,1201,1)", "DB(1000,-1,3,1)",
     "DB(0,0,3,1)", "DB(1000,100,0,1)")
many("DDB(2400,300,10*365,1)", "DDB(2400,300,10*12,1,2)", "DDB(2400,300,10,1)",
     "DDB(2400,300,10,2,1.5)", "DDB(2400,300,10,10)", "DDB(1000,100,2,1,4)",
     "DDB(2400,300,10,11)", "DDB(2400,300,10,0.5)", "DDB(-100,-200,5,1)", "DDB(1000,-1,5,1)",
     "DDB(1000,100,5,1,0)", "DDB(100,1000,5,1)")
many("VDB(2400,300,10*365,0,1)", "VDB(2400,300,10*12,6,18)", "VDB(2400,300,10*12,6,18,1.5)",
     "VDB(2400,300,10,0,0.875,1.5)", "VDB(2400,300,10,0.5,3.25)",
     "VDB(2400,300,10,0.5,3.25,1.5,TRUE)", "VDB(2400,300,10,1,2.5,2,TRUE)",
     "VDB(2400,300,10,0,10)", "VDB(2400,300,10,3,2)", "VDB(2400,300,10,0,11)",
     "VDB(1000,0,5,0,5)", "VDB(1000,0,5,3,5)", "VDB(1000,100,5,3.5,5,1.5)",
     "VDB(1000,0,5,2.5,4.5,3,TRUE)", "VDB(2400,300,10,-1,2)", "VDB(-1,-2,10,0,1)",
     "VDB(100,300,10,0,1)")
AM = "2400,DATE(2008,8,19),DATE(2008,12,31),300"
many(f"AMORDEGRC({AM},1,0.15,1)", f"AMORDEGRC({AM},0,0.15,1)", f"AMORDEGRC({AM},3,0.15,1)",
     f"AMORDEGRC({AM},6,0.15,0)", f"AMORDEGRC({AM},1,0.4,3)", f"AMORDEGRC({AM},2,0.25,2)",
     f"AMORDEGRC({AM},1,0.15,5)", f"AMORDEGRC({AM},1,0,1)", f"AMORDEGRC({AM},1,0.18,1)",
     f"AMORDEGRC({AM},4,0.18,1)", "AMORDEGRC(0,DATE(2008,8,19),DATE(2008,12,31),0,1,0.15,1)")
many(f"AMORLINC({AM},1,0.15,1)", f"AMORLINC({AM},0,0.15,1)", f"AMORLINC({AM},6,0.15,1)",
     f"AMORLINC({AM},7,0.15,1)", f"AMORLINC({AM},8,0.15,4)", f"AMORLINC({AM},1,0.15,0)",
     "AMORLINC(2400,DATE(2009,1,1),DATE(2008,12,31),300,1,0.15,1)",
     "AMORLINC(2400,DATE(2008,8,19),DATE(2008,12,31),3000,1,0.15,1)",
     "AMORLINC(2400,DATE(2008,8,19),DATE(2008,12,31),-1,1,0.15,1)")

# --- bonds and securities ------------------------------------------------------
AC = "DATE(2008,3,1),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2"
many(f"ACCRINT({AC},0)", f"ACCRINT({AC},1)", f"ACCRINT({AC},2)", f"ACCRINT({AC},3)",
     f"ACCRINT({AC},4)", f"ACCRINT({AC})", f"ACCRINT({AC},5)",
     "ACCRINT(DATE(2008,3,1),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,3,0)",
     "ACCRINT(DATE(2008,6,1),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2,0)",
     "ACCRINT(DATE(2008,3,1),DATE(2008,8,31),DATE(2008,5,1),0,1000,2,0)",
     "ACCRINT(DATE(2008,3,1),DATE(2008,8,31),DATE(2008,5,1),0.1,0,2,0)",
     "ACCRINT(DATE(2009,1,31),DATE(2009,8,31),DATE(2009,5,1),0.1,1000,2,1)",
     "ACCRINT(DATE(2008,2,29),DATE(2008,8,31),DATE(2009,2,28),0.1,1000,2,0)")
# ACCRINT's calc_method: LibreOffice rejects the 8th argument (Err:504).
lo("ACCRINT(DATE(2008,3,5),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2,0,FALSE)",
   excel=approx(15.555556, 1e-6))
lo("ACCRINT(DATE(2008,4,5),DATE(2008,8,31),DATE(2008,5,1),0.1,1000,2,0,TRUE)",
   excel=approx(7.2222222, 1e-6))
many("ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0.1,1000,3)",
     "ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0.1)",
     "ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0.1,1000,1)",
     "ACCRINTM(DATE(2008,1,31),DATE(2008,3,31),0.1,100,4)",
     "ACCRINTM(DATE(2008,6,15),DATE(2008,4,1),0.1,1000,3)",
     "ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0,1000,3)",
     "ACCRINTM(DATE(2008,4,1),DATE(2008,6,15),0.1,0,3)",
     "ACCRINTM(DATE(2009,2,10),DATE(2009,2,28),0.1,1000,0)",
     "ACCRINTM(DATE(2008,2,10),DATE(2009,3,10),0.1,1000,0)")
CP = ["DATE(2011,1,25),DATE(2011,11,15)", "DATE(2007,2,28),DATE(2008,8,31)",
      "DATE(2008,2,29),DATE(2010,2,28)", "DATE(2012,1,31),DATE(2013,11,30)"]
for fn in ("COUPDAYBS", "COUPDAYS", "COUPDAYSNC", "COUPNCD", "COUPNUM", "COUPPCD"):
    for i, sm in enumerate(CP):
        for f, bs in (("2", 1), ("4", 0), ("1", 4), ("2", 3), ("4", 2))[i:i + 2]:
            lo(f"{fn}({sm},{f},{bs})")
    lo(f"{fn}({CP[0]},2)")
    lo(f"{fn}({CP[0]},3,1)")
    lo(f"{fn}(DATE(2011,11,15),DATE(2011,11,15),2,1)")
    lo(f"{fn}({CP[0]},2,5)")
# Month ends: February settlements, 31st/30th maturities, US vs European 30/360.
many("COUPDAYBS(DATE(2008,2,29),DATE(2010,8,31),2,0)", "COUPDAYBS(DATE(2008,3,31),DATE(2010,8,31),2,4)",
     "COUPDAYBS(DATE(2008,2,29),DATE(2010,8,31),2,4)", "COUPDAYSNC(DATE(2008,2,28),DATE(2010,8,31),2,4)",
     "COUPDAYBS(DATE(2008,1,30),DATE(2010,2,28),2,0)", "COUPDAYSNC(DATE(2008,1,30),DATE(2010,2,28),2,0)",
     "COUPDAYBS(DATE(2008,8,31),DATE(2010,2,28),2,0)", "COUPDAYSNC(DATE(2008,8,30),DATE(2010,2,28),2,0)",
     "COUPDAYBS(DATE(2008,3,31),DATE(2010,8,30),2,1)", "COUPPCD(DATE(2008,8,30),DATE(2010,8,31),2,1)",
     "COUPNCD(DATE(2008,8,30),DATE(2010,8,30),2,1)", "COUPDAYSNC(DATE(2008,8,30),DATE(2010,8,31),2,1)",
     "COUPDAYS(DATE(2007,12,15),DATE(2010,3,31),2,1)", "COUPNUM(DATE(2007,12,31),DATE(2010,6,30),4,0)",
     "COUPDAYBS(DATE(2008,3,15),DATE(2010,2,15),2,4)", "COUPDAYSNC(DATE(2008,3,15),DATE(2010,2,15),2,4)")
for fn, args in (("DISC", "97.975,100"), ("INTRATE", "1000000,1014420"),
                 ("PRICEDISC", "0.0525,100"), ("RECEIVED", "1000000,0.0575"),
                 ("YIELDDISC", "99.795,100")):
    sm = "DATE(2008,2,16),DATE(2008,3,1)" if fn in ("PRICEDISC", "YIELDDISC") else \
        "DATE(2008,2,15),DATE(2008,5,15)"
    for bs in range(5):
        lo(f"{fn}({sm},{args},{bs})")
    lo(f"{fn}({sm},{args})")
    lo(f"{fn}(DATE(2008,5,15),DATE(2008,2,15),{args},2)")
    lo(f"{fn}({sm},0,100,2)")
    lo(f"{fn}({sm},{args.split(',')[0]},0,2)")
# DISC over decades: Excel docs' example (LibreOffice and Excel agree).
lo("DISC(DATE(2018,7,1),DATE(2048,1,1),97.975,100,1)")
# DURATION/MDURATION: LibreOffice times cash flows from YEARFRAC, which
# drifts from Excel under actual/actual; Excel's documented examples.
lo("DURATION(DATE(2018,7,1),DATE(2048,1,1),0.08,0.09,2,1)", excel=approx(10.9191453, 1e-7))
lo("MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1)", excel=approx(5.736, 1e-4))
# On 30/360 the YEARFRAC offset equals Excel's DSC/E − 1, so LibreOffice
# agrees with Excel there, settlement on a coupon date or not.
many("DURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,0)",
     "DURATION(DATE(2008,3,10),DATE(2016,1,1),0.08,0.09,2,0)",
     "DURATION(DATE(2008,3,10),DATE(2016,1,1),0.08,0.09,4,4)",
     "DURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,4)",
     "DURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,-0.09,2,0)",
     "DURATION(DATE(2016,1,1),DATE(2008,1,1),0.08,0.09,2,0)",
     "DURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,3,0)",
     "MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,0)",
     "MDURATION(DATE(2008,3,10),DATE(2016,1,1),0.08,0.09,4,0)",
     "MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,1,4)",
     "MDURATION(DATE(2008,1,1),DATE(2016,1,1),-0.08,0.09,2,0)",
     "MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,5)")
# PRICE/YIELD: Excel's PRICE example, then each basis.
lo(f"PRICE({S1},{M1},0.0575,0.065,100,2,0)")
for bs in (1, 2, 3, 4):
    lo(f"PRICE({S1},{M1},0.0575,0.065,100,2,{bs})")
many(f"PRICE({S1},{M1},0.0575,0.065,100,1)", f"PRICE({S1},{M1},0.0575,0.065,100,4,1)",
     f"PRICE({S1},{M1},0,0.065,100,2)", f"PRICE({S1},{M1},0.0575,-0.065,100,2)",
     f"PRICE({S1},{M1},0.0575,0.065,0,2)", f"PRICE({M1},{S1},0.0575,0.065,100,2)",
     f"PRICE({S1},{M1},0.0575,0.065,100,3)")
# Actual/actual, settlement on a coupon date: every cash flow is a whole
# number of periods away (DSC = E), which Gnumeric's DURATION assumes always
# and Excel's formula gives here; LibreOffice's YEARFRAC offset does not.
GN_DUR = ["DURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,2,1)",
          "DURATION(DATE(2009,1,1),DATE(2016,1,1),0,0.09,1,1)",
          "MDURATION(DATE(2008,1,1),DATE(2016,1,1),0.08,0.09,4,1)"]
# One coupon left: LibreOffice compounds, Excel uses simple interest. The
# Excel-formula value is Gnumeric's (which implements the documented formula).
GN = []  # Gnumeric-computed rows: (formula, recorded value, tolerance)
GN += [(f, None, None) for f in GN_DUR]
GN.append((f"PRICE({S1},DATE(2008,6,15),0.0575,0.065,100,2,0)", None, None))
GN.append((f"YIELD({S1},DATE(2008,6,15),0.0575,99.5,100,2,0)", None, None))
GN.append((f"YIELD({S1},DATE(2008,6,15),0.0575,99.5,100,2,1)", None, None))
Y1 = "DATE(2016,11,15)"
lo(f"YIELD({S1},{Y1},0.0575,95.04287,100,2,0)", tol=1e-9)
for bs in (1, 2, 3, 4):
    lo(f"YIELD({S1},{Y1},0.0575,95.04287,100,2,{bs})", tol=1e-9)
for f in (f"YIELD({S1},{Y1},0.0575,95.04287,100,1)", f"YIELD({S1},{Y1},0.0575,120,100,4,1)",
          f"YIELD({S1},{Y1},0,60,100,2)"):
    lo(f, tol=1e-9)
many(f"YIELD({S1},{Y1},0.0575,0,100,2)", f"YIELD({S1},{Y1},-0.01,95,100,2)",
     f"YIELD({Y1},{S1},0.0575,95,100,2)", f"YIELD({S1},{Y1},0.0575,95,100,2,-1)")
PM = "DATE(2008,2,15),DATE(2008,4,13),DATE(2007,11,11),0.061"
for bs in range(5):
    lo(f"PRICEMAT({PM},0.061,{bs})")
many(f"PRICEMAT({PM},0.061)", f"PRICEMAT({PM},-0.061,0)",
     "PRICEMAT(DATE(2008,2,15),DATE(2008,4,13),DATE(2007,11,11),-0.061,0.061,0)",
     "PRICEMAT(DATE(2008,4,13),DATE(2008,2,15),DATE(2007,11,11),0.061,0.061,0)")
YM = "DATE(2008,3,15),DATE(2008,11,3),DATE(2007,11,8),0.0625"
for bs in range(5):
    lo(f"YIELDMAT({YM},100.0123,{bs})")
many(f"YIELDMAT({YM},100.0123)", f"YIELDMAT({YM},0,0)",
     "YIELDMAT(DATE(2008,3,15),DATE(2008,11,3),DATE(2007,11,8),-0.0625,100.0123,0)",
     "YIELDMAT(DATE(2008,3,15),DATE(2008,11,3),DATE(2008,4,8),0.0625,100.0123,0)")
# ODDLPRICE/ODDLYIELD: Excel's examples, then each basis.
OL = "DATE(2008,2,7),DATE(2008,6,15),DATE(2007,10,15),0.0375"
for bs in range(5):
    lo(f"ODDLPRICE({OL},0.0405,100,2,{bs})")
many(f"ODDLPRICE({OL},0.0405,100,1)", f"ODDLPRICE({OL},-0.0405,100,2,0)",
     "ODDLPRICE(DATE(2008,2,7),DATE(2008,6,15),DATE(2007,10,15),-0.0375,0.0405,100,2,0)",
     f"ODDLPRICE({OL},0.0405,100,3,0)",
     "ODDLPRICE(DATE(2007,10,15),DATE(2008,6,15),DATE(2008,2,7),0.0375,0.0405,100,2,0)")
OY = "DATE(2008,4,20),DATE(2008,6,15),DATE(2007,12,24),0.0375"
for bs in range(5):
    lo(f"ODDLYIELD({OY},99.875,100,2,{bs})")
many(f"ODDLYIELD({OY},99.875,100,4)", f"ODDLYIELD({OY},0,100,2,0)",
     f"ODDLYIELD({OY},99.875,0,2,0)")
# ODDFPRICE/ODDFYIELD: Gnumeric (LibreOffice always errors); Excel's examples
# are ODDFPRICE 113.60 and ODDFYIELD 7.72% (Gnumeric: 113.5977, 0.07725).
OF = "DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1)"
OS = "DATE(2008,11,11),DATE(2009,6,30),DATE(2008,10,15),DATE(2009,2,28)"
for f in [f"ODDFPRICE({OF},0.0785,0.0625,100,2,{bs})" for bs in range(5)] + [
        f"ODDFPRICE({OS},0.05,0.06,100,4,1)",
        f"ODDFPRICE({OF},0.0785,0.0625,100,1,0)",
        "ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2010,3,1),"
        "0.0785,0.0625,100,2,1)",
        "ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2010,3,1),"
        "0.0785,0.0625,100,2,0)",
        "ODDFPRICE(DATE(2008,11,11),DATE(2021,3,1),DATE(2008,10,15),DATE(2010,3,1),"
        "0.0785,0.0625,100,2,4)",
        "ODDFPRICE(DATE(2008,1,31),DATE(2012,8,31),DATE(2007,12,30),DATE(2008,8,31),"
        "0.0785,0.0625,100,2,0)",
        "ODDFPRICE(DATE(2008,2,29),DATE(2012,8,31),DATE(2008,1,31),DATE(2009,2,28),"
        "0.0785,0.0625,100,2,0)"]:
    GN.append((f, None, None))
for f in [f"ODDFYIELD({OF},0.0575,84.5,100,2,{bs})" for bs in range(5)] + [
        f"ODDFYIELD({OS},0.05,99,100,4,1)"]:
    GN.append((f, None, 1e-9))
GN_ERR = [f"ODDFPRICE({OF},0.0785,-0.0625,100,2,0)", f"ODDFPRICE({OF},0.0785,0.0625,100,3,0)",
          "ODDFPRICE(DATE(2008,10,1),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),"
          "0.0785,0.0625,100,2,0)",
          f"ODDFYIELD({OF},0.0575,0,100,2,0)", f"ODDFYIELD({OF},0.0575,84.5,100,2,7)",
          f"ODDFPRICE({OF},-0.0785,0.0625,100,2,0)", f"ODDFPRICE({OF},0.0785,0.0625,0,2,0)",
          "ODDFPRICE(DATE(2009,4,1),DATE(2021,3,1),DATE(2008,10,15),DATE(2009,3,1),"
          "0.0785,0.0625,100,2,0)"]
# T-bills: Excel's examples, a 31st and February, errors.
TB = ["DATE(2008,3,31),DATE(2008,6,1)", "DATE(2008,1,31),DATE(2008,3,1)",
      "DATE(2008,2,29),DATE(2008,8,31)"]
for sm in TB:
    lo(f"TBILLEQ({sm},0.0914)")
    lo(f"TBILLPRICE({sm},0.09)")
    lo(f"TBILLYIELD({sm},98.45)")
many("TBILLEQ(DATE(2008,3,31),DATE(2009,6,1),0.0914)", "TBILLEQ(DATE(2008,3,31),DATE(2008,6,1),0)",
     "TBILLEQ(DATE(2008,6,1),DATE(2008,3,31),0.0914)",
     "TBILLPRICE(DATE(2008,3,31),DATE(2008,6,1),-0.09)",
     "TBILLPRICE(DATE(2008,6,1),DATE(2008,3,31),0.09)",
     "TBILLEQ(DATE(2008,1,15),DATE(2008,3,30),0.0914)",
     "TBILLEQ(DATE(2008,10,15),DATE(2008,12,31),0.0914)",
     "TBILLYIELD(DATE(2008,3,31),DATE(2009,6,1),98.45)",
     "TBILLYIELD(DATE(2008,3,31),DATE(2008,6,1),0)")
# TBILLPRICE: LibreOffice accepts more than a year unless the 30/360 year
# fraction is whole; Excel's documentation makes more than a year #NUM!.
lo("TBILLPRICE(DATE(2008,3,31),DATE(2009,3,31),0.09)")
lo("TBILLPRICE(DATE(2008,3,31),DATE(2009,6,1),0.09)", excel=NUM)

DATE_RESULT = {"COUPNCD", "COUPPCD"}


def gnumeric(formulas):
    exe = os.environ.get("GNUMERIC_SSCONVERT")
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
            vals = fh.read().splitlines()
    res = []
    for v in vals:
        if v.startswith("#"):
            res.append(err(v))
        else:
            res.append(float(v))
    return res


def as_date(serial):
    d = datetime.date(1899, 12, 30) + datetime.timedelta(days=int(serial))
    return {"date": d.isoformat() + "T00:00:00Z"}


def main():
    vals = run_lo(["PI()"] + [to_lo(f) for f, _, _ in LO])
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    rows = []
    for (f, excel, tol), v in zip(LO, vals[1:]):
        if isinstance(v, tuple):
            # Err:523: the iteration did not converge (Excel's #NUM!).
            v = NUM if v[1] == "Err:523" else map_error(f, v[1])
        elif f.split("(")[0] in DATE_RESULT:
            v = as_date(v)
        if excel is not None:
            print(f"LibreOffice differs: {f} LO={fmt(v)} Excel={fmt(excel)}", file=sys.stderr)
            rows.append((f, excel, "Excel docs"))
        else:
            rows.append((f, approx(v, tol) if tol and not isinstance(v, dict) else v,
                         "LibreOffice"))
    if UNMAPPED:
        raise SystemExit("unmapped LibreOffice errors:\n  " + "\n  ".join(UNMAPPED))
    gn_formulas = [f for f, _, _ in GN] + GN_ERR
    got = gnumeric(gn_formulas)
    if got is None:
        got = [RECORDED[f] for f in gn_formulas]
    for (f, _, tol), v in zip(GN, got):
        rows.append((f, approx(v, tol) if tol and not isinstance(v, dict) else v, "Gnumeric 1.12"))
    for f, v in zip(GN_ERR, got[len(GN):]):
        rows.append((f, v, "Gnumeric 1.12"))
    for f, _, _ in rows:
        assert "\t" not in f and "^" not in f, f
    print("formula\texpected\tsource")
    for f, v, s in rows:
        print(f"{f}\t{fmt(v) if not isinstance(v, dict) else json.dumps(v)}\t{s}")


# Gnumeric 1.12.60 values, used when GNUMERIC_SSCONVERT is unset; refresh
# with `GNUMERIC_SSCONVERT=... financial.py --record`.
RECORD = os.path.join(os.path.dirname(os.path.abspath(__file__)), "financial_gnumeric.json")
RECORDED = {}
if os.path.exists(RECORD) and sys.argv[1:] != ["--record"]:
    with open(RECORD, encoding="utf-8") as fh:
        RECORDED = json.load(fh)

if __name__ == "__main__":
    if sys.argv[1:] == ["--record"]:
        fs = [f for f, _, _ in GN] + GN_ERR
        with open(RECORD, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(dict(zip(fs, gnumeric(fs))), indent=1) + "\n")
    else:
        main()
