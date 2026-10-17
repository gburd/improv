#!/usr/bin/env python3
"""Regenerate crates/engine/tests/data/p1_goldens.tsv (P1 built-in functions).

    python3 scripts/goldens/p1.py > crates/engine/tests/data/p1_goldens.tsv

LibreOffice rows: every LO case goes into one flat-ODS sheet, which
`soffice --headless --convert-to fods` recalculates. The script reads the typed
results back. Every LO export format (fods, xlsx, csv) rounds a number to 15
significant digits, so each formula gets three helper cells that rebuild the
exact double (see HELPERS). The script checks that PI() comes back as
3.141592653589793. LibreOffice errors are mapped to Excel's: Err:502 (invalid
argument) becomes #VALUE! for the text functions and CHOOSE and #NUM! for the
rest, and Err:504 (parameter list) becomes #VALUE!.

A case marked `excel=...` is one where LibreOffice disagrees with Excel's
documentation. The row records Excel's value with source `Excel docs`, and
stderr shows what LibreOffice gave.

Hand rows (HAND) cover functions LibreOffice lacks:
- Gnumeric rows are computed with Python's `math`, mirroring the cited C code
  (plugins/fn-math, fn-eng and fn-info, plus goffice go-math.c for the *PI
  functions).
- Numbers rows use the examples and notes in Apple's Numbers function help.
- Quantrix rows follow the semantics in
  docs/reviews/2026-10-16-builtin-functions-plan.md. Quantrix's own docs were
  not reachable, so these are assumed, not verified.

Formulas use Improv syntax: `,` separators, TRUE/FALSE literals, and no `^`.
"""

import json
import math
import os
import re
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
from fractions import Fraction
from xml.sax.saxutils import quoteattr

def err(name):
    return {"error": name}


T, F = True, False
# The Excel docs TEXTBEFORE/TEXTAFTER examples (cells A2, A3 and the remarks text).
LR = "Little Red Riding Hood's red hood"
LR2 = "Little red Riding Hood's red hood"
RRH = "Red riding hood's, red hood"

# (improv formula, lo formula override or None, excel override or None)
# Expected values for LO rows come from LibreOffice; only `excel` is hand-given.
LO = []


def lo(f, lo_f=None, excel=None):
    LO.append((f, lo_f, excel))


def many(*fs):
    for f in fs:
        lo(f)


# --- existing scalars -------------------------------------------------------
many("ABS(-2.5)", "ABS(0)", "ABS(3)", "ABS(-0.000001)")
many("ROUND(2.5)", "ROUND(-2.5)", "ROUND(2.675,2)", "ROUND(-1.005,2)", "ROUND(0.5)",
     "ROUND(1.45,1)", "ROUND(1234.5678,-2)", "ROUND(21.5,-1)", "ROUND(3.14159,3)")
# FLOOR/CEILING: Excel 2010+ semantics = LibreOffice FLOOR.XCL/CEILING.XCL (2 args).
lo("FLOOR(2.5)", "COM.MICROSOFT.FLOOR(2.5;1)")
lo("FLOOR(-2.5)", "COM.MICROSOFT.FLOOR(-2.5;1)")
many("FLOOR(2.5,2)", "FLOOR(-2.5,2)", "FLOOR(-2.5,-2)", "FLOOR(2.5,-2)", "FLOOR(1.58,0.1)",
     "FLOOR(0,5)", "FLOOR(0.234,0.01)")
# CEIL is Improv's 1-arg ceiling-to-integer; CEILING.MATH(x) is the same thing.
for x in ("2.1", "-2.1", "5", "0", "-2.5"):
    lo(f"CEIL({x})", f"COM.MICROSOFT.CEILING.MATH({x})")
many("SQRT(16)", "SQRT(2)", "SQRT(0)", "SQRT(-1)", "SQRT(0.25)")
for x in ("2.5", "-3", "0"):
    lo(f"NEG({x})", f"-({x})")
many("MIN(3,1,2)", "MIN(-2.5,0)", "MIN(5)", "MAX(3,1,2)", "MAX(-2.5,-3)", "MAX(5)")
lo("MIN2(4,7)", "MIN(4;7)")
lo("MAX2(4,7)", "MAX(4;7)")

# --- n-ary twins of the aggregations ----------------------------------------
many("SUM(1,2,3)", "SUM(-2.5,2.5)", "SUM(5)", "SUM(1.5,2.25,-0.75)",
     "AVERAGE(1,2,3,4)", "AVERAGE(5)", "AVERAGE(-1,1)", "AVERAGE(1,2)",
     "PRODUCT(2,3,4)", "PRODUCT(-2,3)", "PRODUCT(0,5)", "PRODUCT(1.5)",
     "SUMSQ(3,4)", "SUMSQ(-2)", "SUMSQ(1,2,3)", "SUMSQ(0.5)",
     "AND(TRUE,FALSE)", "AND(TRUE,TRUE)", "AND(TRUE)", "AND(FALSE,FALSE,TRUE)",
     "OR(TRUE,FALSE)", "OR(FALSE,FALSE)", "OR(FALSE)", "OR(FALSE,FALSE,TRUE)",
     "XOR(TRUE,FALSE)", "XOR(TRUE,TRUE)", "XOR(TRUE,TRUE,TRUE)", "XOR(FALSE)")

# --- logical ------------------------------------------------------------------
# IF(FALSE,1) is FALSE in the spreadsheets but no cell in Improv (a
# documented deviation: no Number/Boolean mixed type), so it is not a golden.
many("IF(TRUE,1,2)", "IF(FALSE,1,2)",
     "IFS(TRUE,1,TRUE,2)", "IFS(FALSE,1,TRUE,2)", "IFS(FALSE,1)",
     'SWITCH(2,1,"a",2,"b")', 'SWITCH(3,1,"a",2,"b","z")', 'SWITCH(3,1,"a",2,"b")',
     'CHOOSE(2,"a","b","c")', 'CHOOSE(4,"a","b","c")', 'CHOOSE(0,"a")', 'CHOOSE(2.9,"a","b","c")',
     "IFERROR(SQRT(-1),0)", "IFERROR(5,0)", 'IFERROR(NA(),"x")',
     "IFNA(NA(),0)", "IFNA(5,0)", "IFNA(SQRT(-1),0)",
     "TRUE()", "FALSE()")

# --- errors / info ------------------------------------------------------------
many("ISERROR(NA())", "ISERROR(1)", "ISERROR(SQRT(-1))", "ISERROR(MOD(1,0))", 'ISERROR("x")',
     "ISERR(NA())", "ISERR(SQRT(-1))", "ISERR(1)", "ISERR(MOD(1,0))",
     "ISNA(NA())", "ISNA(SQRT(-1))", 'ISNA("x")', "ISNA(1)",
     "NA()",
     "ERROR.TYPE(NA())", "ERROR.TYPE(1)", "ERROR.TYPE(MOD(1,0))")

# --- math ---------------------------------------------------------------------
many("SIGN(-2.5)", "SIGN(0)", "SIGN(3)",
     "SQRTPI(1)", "SQRTPI(2)", "SQRTPI(0)", "SQRTPI(-1)",
     "EXP(0)", "EXP(1)", "EXP(-1)", "EXP(2.5)", "EXP(710)",
     "LN(1)", "LN(0)", "LN(-1)", "LN(10)", "LN(0.5)",
     "LOG(100)", "LOG(8,2)", "LOG(10)", "LOG(0)", "LOG(10,0)", "LOG(-1,10)",
     "LOG(86,2.7182818)",
     "LOG10(1000)", "LOG10(0.01)", "LOG10(0)", "LOG10(-5)", "LOG10(2)",
     "POWER(2,3)", "POWER(0,0)", "POWER(-2,3)", "POWER(2,-1)", "POWER(4,0.5)",
     "POWER(-1,0.5)", "POWER(98.6,3.2)",
     "MOD(-7,3)", "MOD(5,-3)", "MOD(7,3)", "MOD(5,0)", "MOD(5.5,2)", "MOD(-5.5,2)", "MOD(0,3)",
     "QUOTIENT(7,2)", "QUOTIENT(-7,2)", "QUOTIENT(4.5,3.1)", "QUOTIENT(-10,-3)",
     "PI()",
     "DEGREES(3.141592653589793)", "DEGREES(1)", "DEGREES(-0.5)", "DEGREES(0)",
     "RADIANS(180)", "RADIANS(-90)", "RADIANS(0)", "RADIANS(1)",
     "SIN(0)", "SIN(1)", "SIN(-1)", "SIN(1.5707963267948966)", "SIN(3.141592653589793)",
     "COS(0)", "COS(1)", "COS(-1)", "COS(3.141592653589793)",
     "TAN(0)", "TAN(1)", "TAN(-1)", "TAN(0.7853981633974483)",
     "ASIN(0.5)", "ASIN(1)", "ASIN(-1)", "ASIN(0)", "ASIN(2)",
     "ACOS(0.5)", "ACOS(-1)", "ACOS(1)", "ACOS(1.5)",
     "ATAN(1)", "ATAN(-1)", "ATAN(0)", "ATAN(1000)",
     "ATAN2(1,2)", "ATAN2(1,1)", "ATAN2(-1,0)", "ATAN2(0,-1)", "ATAN2(-1,-1)",
     "SINH(0)", "SINH(1)", "SINH(-1)", "SINH(1000)",
     "COSH(0)", "COSH(1)", "COSH(-1)", "COSH(1000)",
     "TANH(0)", "TANH(0.5)", "TANH(-2)", "TANH(1000)",
     "ASINH(0)", "ASINH(1)", "ASINH(-2.5)", "ASINH(10)",
     "ACOSH(1)", "ACOSH(2)", "ACOSH(10)", "ACOSH(0.5)",
     "ATANH(0)", "ATANH(0.5)", "ATANH(-0.76159416)", "ATANH(1)", "ATANH(-1)",
     "SEC(0)", "SEC(1)", "SEC(-1)", "SEC(45)",
     "CSC(1)", "CSC(-1)", "CSC(15)",
     "COT(1)", "COT(-1)", "COT(30)",
     "SECH(0)", "SECH(1)", "SECH(-1)", "SECH(2)",
     "CSCH(1.5)", "CSCH(-1)", "CSCH(0.5)",
     "COTH(2)", "COTH(-1)", "COTH(0.5)",
     "ACOT(2)", "ACOT(0)", "ACOT(-1)", "ACOT(1)",
     "ACOTH(6)", "ACOTH(-2)", "ACOTH(1)", "ACOTH(1.5)",
     "FACT(5)", "FACT(0)", "FACT(1.9)", "FACT(-1)", "FACT(20)", "FACT(170)",
     "FACTDOUBLE(6)", "FACTDOUBLE(7)", "FACTDOUBLE(0)", "FACTDOUBLE(-1)", "FACTDOUBLE(-2)",
     "COMBIN(8,2)", "COMBIN(5,0)", "COMBIN(0,0)", "COMBIN(10.7,2.2)", "COMBIN(2,3)", "COMBIN(-1,2)",
     "COMBINA(4,3)", "COMBINA(10,3)", "COMBINA(3,0)", "COMBINA(-1,2)",
     "PERMUT(100,3)", "PERMUT(3,2)", "PERMUT(5,0)", "PERMUT(2,3)", "PERMUT(-1,1)",
     "PERMUTATIONA(3,2)", "PERMUTATIONA(2,2)", "PERMUTATIONA(5,0)", "PERMUTATIONA(-1,2)",
     "GCD(5,2)", "GCD(24,36)", "GCD(5,0)", "GCD(0,0)", "GCD(12.9,8)", "GCD(-1,2)",
     "LCM(5,2)", "LCM(24,36)", "LCM(4,6,8)", "LCM(3,0)", "LCM(-1,2)",
     "MULTINOMIAL(2,3,4)", "MULTINOMIAL(1)", "MULTINOMIAL(0,0)", "MULTINOMIAL(-1,2)",
     "RAWSUBTRACT(10,3,2)", "RAWSUBTRACT(0.3,0.1)", "RAWSUBTRACT(1,-2)", "RAWSUBTRACT(0.1,0.1,0.1)")

# --- rounding -------------------------------------------------------------
many("ROUNDUP(3.2)", "ROUNDUP(-3.2)", "ROUNDUP(3.14159,3)", "ROUNDUP(31415.92654,-2)",
     "ROUNDUP(0)", "ROUNDUP(76.9)",
     "ROUNDDOWN(3.2)", "ROUNDDOWN(-3.14159,1)", "ROUNDDOWN(31415.92654,-2)", "ROUNDDOWN(76.9)",
     "ROUNDDOWN(-3.7)",
     "ROUNDSIG(123.456,2)", "ROUNDSIG(0.0012345,3)", "ROUNDSIG(-987.6,1)", "ROUNDSIG(0,3)",
     "ROUNDSIG(2.5,1)", "ROUNDSIG(5,0)",
     "TRUNC(8.9)", "TRUNC(-8.9)", "TRUNC(0.45)", "TRUNC(3.14159,2)", "TRUNC(1234.5,-2)",
     "INT(8.9)", "INT(-8.9)", "INT(0)", "INT(-0.5)", "INT(5)",
     "CEILING(2.5,1)", "CEILING(-2.5,2)", "CEILING(-2.5,-2)", "CEILING(1.5,0.1)",
     "CEILING(0.234,0.01)", "CEILING(2.5,-2)", "CEILING(2.5,0)")
lo("CEILING(2.5)", "COM.MICROSOFT.CEILING(2.5;1)")
many("FLOOR.MATH(24.3,5)", "FLOOR.MATH(6.7)", "FLOOR.MATH(-8.1,2)", "FLOOR.MATH(-5.5,2,-1)",
     "FLOOR.MATH(-2.5)", "FLOOR.MATH(2.5,0)", "FLOOR.MATH(-2.5,-2)",
     "CEILING.MATH(24.3,5)", "CEILING.MATH(6.7)", "CEILING.MATH(-8.1,2)",
     "CEILING.MATH(-5.5,2,-1)", "CEILING.MATH(-2.5)", "CEILING.MATH(2.5,0)",
     "FLOOR.PRECISE(-3.2,-1)", "FLOOR.PRECISE(3.2,1)", "FLOOR.PRECISE(-3.2,1)",
     "FLOOR.PRECISE(3.2,-1)", "FLOOR.PRECISE(3.2)", "FLOOR.PRECISE(3.2,0)",
     "CEILING.PRECISE(4.3)", "CEILING.PRECISE(-4.3)", "CEILING.PRECISE(4.3,2)",
     "CEILING.PRECISE(4.3,-2)", "CEILING.PRECISE(-4.3,2)", "CEILING.PRECISE(-4.3,-2)",
     "CEILING.PRECISE(4.3,0)",
     "ISO.CEILING(4.3)", "ISO.CEILING(-4.3)", "ISO.CEILING(4.3,2)", "ISO.CEILING(4.3,-2)",
     "ISO.CEILING(-4.3,2)", "ISO.CEILING(-4.3,-2)",
     "MROUND(10,3)", "MROUND(-10,-3)", "MROUND(1.3,0.2)", "MROUND(7.5,5)",
     "MROUND(-7.5,-5)", "MROUND(2.5,1)", "MROUND(5,0)",
     "EVEN(1.5)", "EVEN(3)", "EVEN(2)", "EVEN(-1)", "EVEN(0)", "EVEN(-2.5)",
     "ODD(1.5)", "ODD(3)", "ODD(2)", "ODD(-1)", "ODD(-2)", "ODD(0)")

# --- text ---------------------------------------------------------------------
many("CHAR(65)", "CHAR(33)", "CHAR(32)", "CHAR(65.9)", "CHAR(256)",
     "UNICHAR(66)", "UNICHAR(233)", "UNICHAR(8364)", "UNICHAR(128512)",
     'CODE("A")', 'CODE("Alphabet")', 'CODE("!")', 'CODE("a")',
     'UNICODE("B")', 'UNICODE("é")', 'UNICODE("€uro")', 'UNICODE("😀")', 'UNICODE("")',
     'CLEAN("abc")', "CLEAN(CHAR(9))", 'CLEAN(CONCATENATE("a",CHAR(10),"b"))',
     'CLEAN(CONCATENATE(CHAR(7),"text",CHAR(7)))', 'CLEAN("é😀")',
     'CONCAT("a","b","c")', 'CONCAT("x",1)', 'CONCAT("é","😀")',
     'CONCAT(1.5,2)', 'CONCAT("")',
     'CONCATENATE("a","b","c")', 'CONCATENATE("x",1)', 'CONCATENATE("é","😀")',
     'CONCATENATE(1.5,2)', 'CONCATENATE("")',
     'EXACT("word","word")', 'EXACT("Word","word")', 'EXACT("é","É")', 'EXACT("","")',
     'EXACT("w ord","word")',
     'FIND("M","Miriam McGovern")', 'FIND("m","Miriam McGovern")', 'FIND("M","Miriam McGovern",3)',
     'FIND("z","abc")', 'FIND("b","abc",0)', 'FIND("b","abc",5)',
     'FIND("é","café")',
     'SEARCH("e","Statements",6)', 'SEARCH("M","miriam")', 'SEARCH("z","abc")',
     'SEARCH("É","café")', 'SEARCH("b","abc",0)', 'SEARCH("b?d","abcd")', 'SEARCH("~?","a?b")',
     'LEFT("Sale Price",4)', 'LEFT("Sweden")', 'LEFT("abc",10)', 'LEFT("abc",0)',
     'LEFT("abc",-1)', 'LEFT("éa",1)',
     'RIGHT("Sale Price",5)', 'RIGHT("Stock Number")', 'RIGHT("abc",10)', 'RIGHT("abc",0)',
     'RIGHT("abc",-1)', 'RIGHT("café",1)',
     'MID("Fluid Flow",1,5)', 'MID("Fluid Flow",7,20)', 'MID("Fluid Flow",20,5)',
     'MID("abc",0,1)', 'MID("abc",1,-1)', 'MID("café",4,1)',
     'LEN("Phoenix, AZ")', 'LEN("")', 'LEN("  One   ")', 'LEN("café")', 'LEN("😀")',
     'LOWER("E. E. Cummings")', 'LOWER("Apt. 2B")', 'LOWER("ÉCOLE")', 'LOWER("")',
     'UPPER("total")', 'UPPER("Yield")', 'UPPER("café")', 'UPPER("")',
     'PROPER("this is a TITLE")', 'PROPER("2-way street")', 'PROPER("76BudGet")',
     'PROPER("élan vital")', 'PROPER("don\'t")',
     'REPLACE("abcdefghijk",6,5,"*")', 'REPLACE("2009",3,2,"10")', 'REPLACE("123456",1,3,"@")',
     'REPLACE("abc",5,1,"x")', 'REPLACE("café",4,1,"e")', 'REPLACE("abc",0,1,"x")',
     'REPT("*-",3)', 'REPT("é",2)', 'REPT("a",0)', 'REPT("ab",2.9)', 'REPT("a",-1)',
     'SUBSTITUTE("Sales Data","Sales","Cost")', 'SUBSTITUTE("Quarter 1, 2008","1","2",1)',
     'SUBSTITUTE("Quarter 1, 2011","1","2",3)', 'SUBSTITUTE("aaa","a","b")',
     'SUBSTITUTE("abc","","x")', 'SUBSTITUTE("café","é","e")', 'SUBSTITUTE("abc","b","x",0)',
     'TRIM(" First Quarter Earnings ")', 'TRIM("  a  b  ")', 'TRIM("")', 'TRIM("é  😀")',
     'VALUE("123")', 'VALUE("-2.5")', 'VALUE("  42  ")', 'VALUE("50%")', 'VALUE("1e3")',
     'VALUE("abc")', 'VALUE("")')
# Overrides: LibreOffice disagrees with Excel's documented result.
ex = lo
ex("COT(0)", excel=err("#DIV/0!"))  # "COT(0) returns the #DIV/0! error value"
ex("ATAN2(0,0)", excel=err("#DIV/0!"))  # "If both x_num and y_num are 0 ... #DIV/0!"
ex("MROUND(5,-2)", excel=err("#NUM!"))  # docs example: different signs -> #NUM!
ex("MROUND(-5,2)", excel=err("#NUM!"))
ex("SEC(134217728)", excel=err("#NUM!"))  # |number| must be < 2^27
ex("CSC(134217728)", excel=err("#NUM!"))
ex("UNICHAR(0)", excel=err("#VALUE!"))  # "If number is zero (0), UNICHAR returns #VALUE!"
ex("CHAR(0)", excel=err("#VALUE!"))  # number must be 1..255
ex('FIND("","abc")', excel=1.0)  # empty find_text matches at start_num
ex('FIND("","abc",2)', excel=2.0)
# ERROR.TYPE: LibreOffice's Err:502 is not a spreadsheet error, so it says #N/A.
# Excel: SQRT(-1) and LN(0) are #NUM! (6) and VALUE("abc") is #VALUE! (3).
ex("ERROR.TYPE(SQRT(-1))", excel=6.0)
ex("ERROR.TYPE(LN(0))", excel=6.0)
ex('ERROR.TYPE(VALUE("abc"))', excel=3.0)
# TEXTBEFORE/TEXTAFTER: instance_num 0 or > LEN(text) is #VALUE!; an empty
# delimiter matches immediately; match_end on "Socrates" gives "" for TEXTAFTER.
ex('TEXTBEFORE("abc","b",0)', excel=err("#VALUE!"))
ex('TEXTBEFORE("abc","b",4)', excel=err("#VALUE!"))
ex('TEXTAFTER("abc","b",0)', excel=err("#VALUE!"))
ex(f'TEXTBEFORE("{RRH}","")', excel="")
ex(f'TEXTBEFORE("{RRH}","",-1)', excel=RRH)
ex(f'TEXTAFTER("{RRH}","")', excel=RRH)
ex(f'TEXTAFTER("{RRH}","",-1)', excel="")
ex('TEXTAFTER("Socrates"," ",1,0,1)', excel="")
ex('TEXTAFTER("Marcus Aurelius"," ",1,0,1)', excel="Aurelius")

many(f'TEXTBEFORE("{LR}","Red")', f'TEXTBEFORE("{LR2}","Red")', f'TEXTBEFORE("{LR2}","red",2)',
     f'TEXTBEFORE("{LR2}","red",-2)', f'TEXTBEFORE("{LR}","RED",1,1)', f'TEXTBEFORE("{RRH}","hood")',
     'TEXTBEFORE("abc","z")', 'TEXTBEFORE("abc","z",1,0,0,"none")', 'TEXTBEFORE("a-b","-",2)',
     'TEXTBEFORE("Marcus Aurelius"," ",1,0,1)', 'TEXTBEFORE("Socrates"," ",1,0,1)',
     'TEXTBEFORE("Socrates"," ",1,0,0)', 'TEXTBEFORE("naïve","ï")',
     f'TEXTAFTER("{LR}","Red")', f'TEXTAFTER("{LR}","basket")', f'TEXTAFTER("{LR2}","red",2)',
     f'TEXTAFTER("{LR2}","red",-2)', f'TEXTAFTER("{LR2}","Red",1,0)', f'TEXTAFTER("{LR}","red",3)',
     f'TEXTAFTER("{LR}","RED",1,1)', f'TEXTAFTER("{RRH}","hood")', 'TEXTAFTER("abc","z",1,0,0,"none")',
     'TEXTAFTER("café","f")')


# --- hand rows ----------------------------------------------------------------
def gnm_float(x):
    """Gnumeric value_new_float: non-finite -> #NUM!, -0 -> 0."""
    if not math.isfinite(x):
        return err("#NUM!")
    return x + 0.0 if x else 0.0


def reduce_half(x):
    """goffice go-math.c reduce_half: x in [-1/4, 1/4], k = quarter turns mod 4."""
    if x < 0:
        x, k = reduce_half(-x)
        x, k = -x, 4 - k
        if x == -0.25:
            x, k = x + 0.5, k + 3
        return x, k & 3
    k = 0
    x = math.fmod(x, 2)
    if x >= 1:
        x, k = x - 1, k + 2
    if x >= 0.5:
        x, k = x - 0.5, k + 1
    if x > 0.25:
        x, k = x - 0.5, k + 1
    return x, k & 3


def do_sinpi(x, k):
    if x == 0:
        y = float(k & 1)
    elif x == 0.25:
        y = 0.707106781186547524400844362104849039284835937688474036588339
    else:
        y = math.cos(math.pi * x) if k & 1 else math.sin(math.pi * x)
    return -y if k & 2 else y


def sinpi(x):
    x0 = x
    x, k = reduce_half(x)
    if x == 0 and (k & 1) == 0:
        return math.copysign(0.0, x0)
    return do_sinpi(x, k)


def cospi(x):
    x, k = reduce_half(x)
    if x == 0 and (k & 1) == 1:
        return 0.0
    return do_sinpi(x, k + 1)


def tanpi(x):
    x = math.fmod(x, 1.0)
    if x == 0:
        return math.copysign(0.0, x)
    if abs(x) == 0.5:
        return math.nan
    return sinpi(x) / cospi(x)


def cotpi(x):
    x = math.fmod(x, 1.0)
    if x == 0:
        return math.nan
    if abs(x) == 0.5:
        return math.copysign(0.0, x)
    return cospi(x) / sinpi(x)


def hypot(*xs):  # gnm_range_hypot
    xs = list(xs)
    while xs and xs[0] == 0:
        xs.pop(0)
    while xs and xs[-1] == 0:
        xs.pop()
    if not xs:
        return 0.0
    if len(xs) == 1:
        return abs(xs[0])
    if len(xs) == 2:
        return math.hypot(*xs)
    return math.sqrt(sum(x * x for x in xs))


def invsuminv(*xs):  # fn-eng range_invsuminv; error -> GNM_ERROR_VALUE
    s, zero = 0.0, False
    for x in xs:
        if x < 0:
            return err("#VALUE!")
        if x == 0:
            zero = True
        else:
            s += 1 / x
    return 0.0 if zero else 1 / s


def ln1p(x):
    return err("#NUM!") if x <= -1 else gnm_float(math.log1p(x))


def log2(x):
    return err("#NUM!") if x <= 0 else gnm_float(math.log2(x))


def expm1(x):
    try:
        return gnm_float(math.expm1(x))
    except OverflowError:
        return err("#NUM!")


GM = "Gnumeric source plugins/fn-math/functions.c"
GE = "Gnumeric source plugins/fn-eng/functions.c"
GI = "Gnumeric source plugins/fn-info/functions.c"
QUANTRIX = "Quantrix docs"
NUMBERS = "Numbers docs"


def gn(name, fn, args, src=GM):
    f = f"{name}({','.join(args)})"
    v = fn(*[float(a) for a in args])
    return (f, v if isinstance(v, dict) else gnm_float(v), src)


HAND = []
for a in ("0", "0.01", "1", "-1", "0.000001", "710"):
    HAND.append(gn("EXPM1", expm1, [a]))
for a in ("0", "0.01", "1", "-0.5", "-1", "-2"):
    HAND.append(gn("LN1P", ln1p, [a]))
for a in ("1024", "1", "0.5", "3", "0", "-4"):
    HAND.append(gn("LOG2", log2, [a]))
for a in ("0.5", "1", "0.25", "-0.5", "0.1", "2.5", "-1", "0"):
    HAND.append(gn("SINPI", sinpi, [a]))
for a in ("0.5", "1", "0", "0.25", "-2", "1.5", "0.1"):
    HAND.append(gn("COSPI", cospi, [a]))
for a in ("1", "0.25", "-0.25", "0.75", "0.5", "0.1"):
    HAND.append(gn("TANPI", tanpi, [a]))
for a in ("0.5", "0.25", "-0.25", "0.1", "0", "1"):
    HAND.append(gn("COTPI", cotpi, [a]))
for args in (["3", "4"], ["-3", "4"], ["1", "1"], ["1", "2", "2"], ["2", "3", "6"],
             ["0", "0"], ["-5"], ["0", "5", "0"]):
    HAND.append(gn("HYPOT", hypot, args))
for args in (["2000", "2000"], ["1", "2", "3"], ["4"], ["0", "5"], ["-1", "2"], ["10", "10", "5"]):
    HAND.append(gn("INVSUMINV", invsuminv, args, GE))
HAND += [
    ('ERROR("#N/A")', err("#N/A"), GI),
    ('ERROR("#DIV/0!")', err("#DIV/0!"), GI),
    ('ERROR("#VALUE!")', err("#VALUE!"), GI),
    # Quantrix BETWEEN(x, low, high, [inclusive=TRUE]): low <= x <= high, or
    # low < x < high when inclusive is FALSE. Reversed bounds not tested.
    ("BETWEEN(5,1,10)", T, QUANTRIX), ("BETWEEN(1,1,10)", T, QUANTRIX), ("BETWEEN(10,1,10)", T, QUANTRIX),
    ("BETWEEN(1,1,10,FALSE)", F, QUANTRIX), ("BETWEEN(5,1,10,FALSE)", T, QUANTRIX),
    ("BETWEEN(11,1,10)", F, QUANTRIX), ("BETWEEN(-2.5,-3,0)", T, QUANTRIX), ("BETWEEN(0,1,10,TRUE)", F, QUANTRIX),
    # Quantrix CLEARERROR(value, alt): alt where value is an error, else value.
    # (The 1-arg "no cell" form has no TSV representation and is not tested.)
    ("CLEARERROR(NA(),0)", 0.0, QUANTRIX), ("CLEARERROR(SQRT(-1),-1)", -1.0, QUANTRIX),
    ("CLEARERROR(5,0)", 5.0, QUANTRIX),
    # Quantrix CLEARZERO(value, alt): alt where value is 0, else value.
    ("CLEARZERO(0,1)", 1.0, QUANTRIX), ("CLEARZERO(5,1)", 5.0, QUANTRIX), ("CLEARZERO(-2.5,1)", -2.5, QUANTRIX),
    # Numbers TEXTBETWEEN(source, first, second): the text between the first
    # `first` and the next `second` after it. Search is case-insensitive.
    # Improv takes three arguments, so the optional occurrence arguments and
    # REGEX are not tested, and neither is the undocumented not-found case.
    ('TEXTBETWEEN("Apple,Banana,Strawberry",",",",")', "Banana", NUMBERS),
    ('TEXTBETWEEN("Extract the text between the first occurrence of the and the last'
     ' occurrence of text","text","the")', " between ", NUMBERS),
    ('TEXTBETWEEN("a[b]c","[","]")', "b", NUMBERS), ('TEXTBETWEEN("xAyBz","a","b")', "y", NUMBERS),
    ('TEXTBETWEEN("<é😀>","<",">")', "é😀", NUMBERS), ('TEXTBETWEEN("[]","[","]")', "", NUMBERS),
    # Numbers COUNTMATCHES(source, search): occurrences of a substring,
    # overlapping matches counted separately, case-insensitive. REGEX not tested.
    ('COUNTMATCHES("Please, could you, if you don’t mind, count the number of commas?",",")',
     3.0, NUMBERS),
    ('COUNTMATCHES("asasa","asa")', 2.0, NUMBERS), ('COUNTMATCHES("aaa","aa")', 2.0, NUMBERS),
    ('COUNTMATCHES("banana","an")', 2.0, NUMBERS), ('COUNTMATCHES("BANANA","a")', 3.0, NUMBERS),
    ('COUNTMATCHES("abc","z")', 0.0, NUMBERS), ('COUNTMATCHES("café café","é")', 2.0, NUMBERS),
]

# --- LibreOffice translation --------------------------------------------------
LO_NAMES = {
    "FLOOR": "COM.MICROSOFT.FLOOR", "CEILING": "COM.MICROSOFT.CEILING",
    "FLOOR.MATH": "COM.MICROSOFT.FLOOR.MATH", "CEILING.MATH": "COM.MICROSOFT.CEILING.MATH",
    "FLOOR.PRECISE": "COM.MICROSOFT.FLOOR.PRECISE",
    "CEILING.PRECISE": "COM.MICROSOFT.CEILING.PRECISE", "ISO.CEILING": "COM.MICROSOFT.ISO.CEILING",
    "CONCAT": "COM.MICROSOFT.CONCAT", "IFS": "COM.MICROSOFT.IFS", "SWITCH": "COM.MICROSOFT.SWITCH",
    "TEXTBEFORE": "COM.MICROSOFT.TEXTBEFORE", "TEXTAFTER": "COM.MICROSOFT.TEXTAFTER",
    "ROUNDSIG": "ORG.LIBREOFFICE.ROUNDSIG", "RAWSUBTRACT": "ORG.LIBREOFFICE.RAWSUBTRACT",
}
# Err:502 means #VALUE! (not #NUM!) when the outermost function is one of these.
VALUE_502 = {"CHAR", "UNICHAR", "CODE", "UNICODE", "CLEAN", "CONCAT", "CONCATENATE", "EXACT",
             "FIND", "SEARCH", "LEFT", "RIGHT", "MID", "LEN", "LOWER", "UPPER", "PROPER",
             "REPLACE", "REPT", "SUBSTITUTE", "TRIM", "VALUE", "TEXTBEFORE", "TEXTAFTER", "CHOOSE"}
LO_ERRORS = {"#NULL!", "#DIV/0!", "#VALUE!", "#REF!", "#NAME?", "#NUM!", "#N/A"}


def to_lo(f):
    """Improv formula -> ODFF: `,` -> `;`, TRUE -> TRUE(), LO function names."""
    out, parts = [], re.split(r'("[^"]*")', f)
    for i, p in enumerate(parts):
        if i % 2:
            out.append(p)
            continue
        p = re.sub(r"\b(TRUE|FALSE)\b(?!\s*\()", r"\1()", p)
        p = re.sub(r"[A-Z][A-Z0-9.]*(?=\()", lambda m: LO_NAMES.get(m.group(0), m.group(0)), p)
        out.append(p.replace(",", ";"))
    return "".join(out)


def outer(f):
    return re.match(r"[A-Z][A-Z0-9.]*", f).group(0)


NS = {
    "office": "urn:oasis:names:tc:opendocument:xmlns:office:1.0",
    "table": "urn:oasis:names:tc:opendocument:xmlns:table:1.0",
    "text": "urn:oasis:names:tc:opendocument:xmlns:text:1.0",
    "calcext": "urn:org:documentfoundation:names:experimental:calc:xmlns:calcext:1.0",
}
HEAD = ('<?xml version="1.0" encoding="UTF-8"?>\n<office:document ' +
        " ".join(f'xmlns:{k}="{v}"' for k, v in NS.items()) +
        ' xmlns:of="urn:oasis:names:tc:opendocument:xmlns:of:1.2" office:version="1.3"'
        ' office:mimetype="application/vnd.oasis.opendocument.spreadsheet">\n'
        '<office:body><office:spreadsheet>'
        '<table:calculation-settings table:case-sensitive="false"'
        ' table:use-regular-expressions="false" table:use-wildcards="true"/>'
        '<table:table table:name="S">\n')
TAIL = "</table:table></office:spreadsheet></office:body></office:document>\n"


# Every LibreOffice export (fods office:value, xlsx <v>, csv) rounds numbers to
# 15 significant digits. The exact double is rebuilt from helper columns that
# hold only integers below 2^30, which 15 digits show exactly:
#   B = e ~ floor(log2|x|)   C = INT(|x|*2^(26-e))   D = (|x|*2^(26-e) - C) * 2^30
# Scaling by a power of two and RAWSUBTRACT are exact, so x = +-(C + D/2^30)*2^(e-26).
HELPERS = (
    "IF(ISNUMBER([.A{n}]);IF([.A{n}]=0;0;INT(LOG(ABS([.A{n}]);2)));0)",
    "INT(ABS([.A{n}])*POWER(2;26-[.B{n}]))",
    "ORG.LIBREOFFICE.RAWSUBTRACT(ABS([.A{n}])*POWER(2;26-[.B{n}]);[.C{n}])*POWER(2;30)",
)
SOFFICE = ["soffice", "--headless", "--convert-to", "fods", "--outdir"]


def run_lo(formulas):
    with tempfile.TemporaryDirectory() as d:
        src = os.path.join(d, "in.fods")
        with open(src, "w", encoding="utf-8") as fh:
            fh.write(HEAD)
            for n, f in enumerate(formulas, 1):
                cells = [f] + [h.format(n=n) for h in HELPERS]
                fh.write("<table:table-row>" + "".join(
                    "<table:table-cell table:formula=%s/>" % quoteattr("of:=" + c) for c in cells)
                    + "</table:table-row>\n")
            fh.write(TAIL)
        outdir = os.path.join(d, "out")
        # A private profile, so a running LibreOffice cannot swallow the job.
        cmd = SOFFICE[:1] + [f"-env:UserInstallation=file://{d}/profile"] + SOFFICE[1:]
        subprocess.run(cmd + [outdir, src], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        root = ET.parse(os.path.join(outdir, "in.fods")).getroot()
    q = lambda ns, n: f"{{{NS[ns]}}}{n}"  # noqa: E731
    vals = []
    for row in root.iter(q("table", "table-row")):
        cells = row.findall(q("table", "table-cell"))
        if not cells or cells[0].get(q("table", "formula")) is None:
            continue
        cell = cells[0]
        vt = cell.get(q("calcext", "value-type")) or cell.get(q("office", "value-type"))
        if vt in ("float", "percentage", "currency"):
            shown = float(cell.get(q("office", "value")))
            e, c, dd = (float(h.get(q("office", "value"))) for h in cells[1:4])
            if not all(h == int(h) and abs(h) < 2**31 for h in (e, c, dd)):
                raise SystemExit(f"row {len(vals) + 1}: helper columns not exact: {e} {c} {dd}")
            e, c, dd = int(e), int(c), int(dd)
            v = float((Fraction(c) + Fraction(dd, 2**30)) * Fraction(2) ** (e - 26))
            v = math.copysign(v, shown)
            if abs(v - shown) > 1e-14 * abs(v):
                raise SystemExit(f"reconstruction {v!r} disagrees with shown {shown!r}")
        elif vt == "boolean":
            v = cell.get(q("office", "boolean-value")) == "true"
        elif vt is None:  # LibreOffice writes an empty-text result with no type
            v = ""
        elif vt == "string":
            v = cell.get(q("office", "string-value"))
            if v is None:
                v = "".join(cell.find(q("text", "p")).itertext())
        elif vt == "error":
            v = ("ERR", "".join(cell.find(q("text", "p")).itertext()))
        else:
            raise SystemExit(f"row {len(vals) + 1}: unexpected value type {vt!r}")
        vals.append(v)
    if len(vals) != len(formulas):
        raise SystemExit(f"read {len(vals)} results for {len(formulas)} formulas")
    return vals


UNMAPPED = []


def map_error(f, shown):
    if shown in LO_ERRORS:
        return err(shown)
    if shown == "Err:502":
        return err("#VALUE!" if outer(f) in VALUE_502 else "#NUM!")
    if shown == "Err:504":  # error in the parameter list
        return err("#VALUE!")
    UNMAPPED.append(f"{f}: {shown}")
    return err(shown)


def fmt(v):
    if isinstance(v, dict):
        return json.dumps(v)
    if isinstance(v, (bool, str)):
        return json.dumps(v, ensure_ascii=False)
    if v == int(v) and abs(v) < 2**53:
        return str(int(v))
    return repr(v)  # shortest round-trip repr (<= 17 significant digits)


def main():
    lo_formulas = [lo_f or to_lo(f) for f, lo_f, _ in LO]
    vals = run_lo(["PI()"] + lo_formulas)
    if vals[0] != 3.141592653589793:
        raise SystemExit(f"precision check failed: PI() read back as {vals[0]!r}")
    rows = []
    for (f, _, excel), v in zip(LO, vals[1:]):
        if isinstance(v, tuple):
            v = map_error(f, v[1])
        if excel is not None:
            print(f"LibreOffice differs: {f} LO={fmt(v)} Excel={fmt(excel)}", file=sys.stderr)
            rows.append((f, excel, "Excel docs"))
        else:
            rows.append((f, v, "LibreOffice"))
    if UNMAPPED:
        raise SystemExit("unmapped LibreOffice errors:\n  " + "\n  ".join(UNMAPPED))
    rows += HAND
    for f, _, _ in rows:
        assert "\t" not in f and "^" not in f, f
    print("formula\texpected\tsource")
    for f, v, s in rows:
        print(f"{f}\t{fmt(v)}\t{s}")


if __name__ == "__main__":
    main()
