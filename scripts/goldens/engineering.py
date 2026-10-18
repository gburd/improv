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
import re
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
TOL = {}  # formula -> relative tolerance (an `approx` row)
GN = []  # (formula, "text" | "num", tolerance or None): Gnumeric's value


def lo(f, excel=None):
    LO.append((f, excel))


def many(*fs):
    for f in fs:
        lo(f)


def gn(f, kind="text", tol=None):
    GN.append((f, kind, tol))


# IMIGAMMA where Gnumeric 1.12 is wrong, so the golden is mpmath's
# gammainc (30 digits): (a, z, lower, regularized, why).
CANCELS = "mpmath (Gnumeric 1.12 has the same cancellation: 1 - P)"
MPMATH_WINS = {
    # The upper regularized value for large |z| off the real axis, as 1 − P
    # from the lower series: Q(0.5, 30+i) comes out 1.2e-5 for 5.0e-15.
    # Improv uses Legendre's continued fraction there.
    'IMIGAMMA("0.5","30+i",FALSE)': (0.5, 30 + 1j, False, True, CANCELS),
    'IMIGAMMA("0.5","20+i",FALSE)': (0.5, 20 + 1j, False, True, CANCELS),
    # After the asymptotic upper series, the unregularized lower value is
    # 1 − Γ(a)·Q, not Γ(a)·(1 − Q): γ(1+i, 40) comes out 1+2.3e-18i.
    'IMIGAMMA("1+i","40",TRUE,FALSE)': (1 + 1j, 40, True, False,
                                        "mpmath (Gnumeric 1.12 gives 1, the regularized value)"),
    # Γ(0, z) = E₁(z); Gnumeric's lower series has no Γ(0) to subtract from.
    'IMIGAMMA("0","1+i",FALSE,FALSE)': (0, 1 + 1j, False, False,
                                        "mpmath (Gnumeric 1.12 gives #NUM!)"),
}


def mpmath_value(a, z, lower, reg, _why):
    import mpmath as mp
    mp.mp.dps = 30
    a, z = mp.mpc(a), mp.mpc(z)
    v = mp.gammainc(a, 0, z) if lower else mp.gammainc(a, z)
    return complex(v / mp.gamma(a) if reg else v)


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


# --- complex numbers --------------------------------------------------------
many('COMPLEX(3,4)', 'COMPLEX(3,4,"j")', 'COMPLEX(0,1)', 'COMPLEX(0,-1,"j")', 'COMPLEX(0,0)',
     'COMPLEX(1.5,0)', 'COMPLEX(3,4,"")', 'COMPLEX(100000000000000000000,-2.5)',
     'COMPLEX(0.00001,1)',
     'IMREAL("6-9i")', 'IMREAL("-i")', 'IMREAL(5)', 'IMREAL("1e2+3i")', 'IMREAL("1.+.5i")',
     'IMREAL("1+2I")', 'IMREAL(" 1+2i")', 'IMREAL("")',
     'IMAGINARY("3+4i")', 'IMAGINARY("0-j")', 'IMAGINARY(4)', 'IMAGINARY("-2.5j")',
     'IMAGINARY("3+4k")',
     'IMABS("5+12i")', 'IMABS("-3")', 'IMABS("i")', 'IMABS("x")',
     'IMARGUMENT("3+4i")', 'IMARGUMENT("-1")', 'IMARGUMENT("-i")', 'IMARGUMENT("0")',
     'IMCONJUGATE("3+4i")', 'IMCONJUGATE("-j")', 'IMCONJUGATE("3")', 'IMCONJUGATE("2i")',
     'IMSUM("3+4i","5-3i")', 'IMSUM("0","-0")', 'IMSUM("1+j",2,"j")',
     'IMSUM("1",2,3)', 'IMSUM("1+i","q")',
     'IMPRODUCT("3+4i","5-3i")', 'IMPRODUCT("1+2i",30)', 'IMPRODUCT("i","i")',
     'IMPRODUCT("1+2j","j")', 'IMPRODUCT("2")',
     'IMSUB("13+4i","5+3i")', 'IMSUB(3,"1+j")', 'IMSUB("i","i")', 'IMSUB("1","0.99999")',
     'IMDIV("-238+240i","10+24i")', 'IMDIV("1","i")', 'IMDIV("1","0")', 'IMDIV("2j","1+j")',
     'IMPOWER("2+3i",3)', 'IMPOWER("i",2)', 'IMPOWER("4",0.5)', 'IMPOWER("-4",0.5)',
     'IMPOWER("0",0)', 'IMPOWER("0",2)', 'IMPOWER("2+3i",-1.5)',
     'IMSQRT("1+i")', 'IMSQRT("-4")', 'IMSQRT("4")', 'IMSQRT("-3+4j")', 'IMSQRT("0")',
     'IMEXP("1+i")', 'IMEXP("0")', 'IMEXP("2.5-j")', 'IMEXP("i")',
     'IMLN("3+4i")', 'IMLN("-1")', 'IMLN("0")', 'IMLN("1")', 'IMLN("-2-2j")',
     'IMLOG10("3+4i")', 'IMLOG10("100")', 'IMLOG10("-i")', 'IMLOG10("0")',
     'IMLOG2("3+4i")', 'IMLOG2("8")', 'IMLOG2("-1+j")', 'IMLOG2("0")',
     'IMSIN("4+3i")', 'IMSIN("1")', 'IMSIN("i")', 'IMSIN("1e19+i")', 'IMSIN("1e20")',
     'IMCOS("1+i")', 'IMCOS("0")', 'IMCOS("-2.5j")', 'IMCOS("1e20")',
     'IMTAN("4+3i")', 'IMTAN("1")', 'IMTAN("6e18+i")', 'IMTAN("1e19+i")', 'IMTAN("-2j")',
     'IMSEC("4+3i")', 'IMSEC("1+i")', 'IMSEC("0")', 'IMSEC("1e20")',
     'IMCSC("4+3i")', 'IMCSC("0")', 'IMCSC("1-2j")', 'IMCSC("2")',
     'IMCOT("4+3i")', 'IMCOT("0")', 'IMCOT("1")', 'IMCOT("-1+i")',
     'IMSINH("4+3i")', 'IMSINH("1")', 'IMSINH("i")', 'IMSINH("1+1e20i")',
     'IMCOSH("4+3i")', 'IMCOSH("0")', 'IMCOSH("2j")', 'IMCOSH("1+1e20i")',
     'IMSECH("4+3i")', 'IMSECH("0")', 'IMSECH("1-j")', 'IMSECH("1e20i")',
     'IMCSCH("4+3i")', 'IMCSCH("1+i")', 'IMCSCH("0")', 'IMCSCH("-2")')
# Excel: a suffix other than i or j, and mixed suffixes, are #VALUE!
# ("all functions that accept two or more complex numbers require that all
# suffixes match"); LibreOffice says Err:502 and mixes them.
lo('COMPLEX(3,4,"I")', excel=VALUE)
lo('IMSUM("1+i","2+j")', excel=VALUE)
lo('IMPRODUCT("1+2j","i")', excel=VALUE)
# Gnumeric-only (IMTANH and IMCOTH too: LibreOffice lacks them). Gnumeric
# prints each part's shortest round-trip digits where Improv keeps Excel's 15,
# so a complex result is checked part by part (IMREAL / IMAGINARY).
for f in ['IMTANH("1+i")', 'IMTANH("0")', 'IMTANH("2-3j")', 'IMTANH("0.5")', 'IMTANH("x")',
          'IMCOTH("1+i")', 'IMCOTH("2-3j")', 'IMCOTH("0.5")', 'IMCOTH("-i")',
          'IMNEG("3+4i")', 'IMNEG("0")', 'IMNEG("-j")', 'IMNEG("2")', 'IMNEG("i5")',
          'IMINV("2")', 'IMINV("1+i")', 'IMINV("-2j")', 'IMINV("0")',
          'IMARCSIN("1+i")', 'IMARCSIN("0.5")', 'IMARCSIN("2")', 'IMARCSIN("-2")', 'IMARCSIN("3-4j")',
          'IMARCSIN("0.1+0.1i")',
          'IMARCCOS("1+i")', 'IMARCCOS("0.5")', 'IMARCCOS("2")', 'IMARCCOS("-2")', 'IMARCCOS("-3+4j")',
          'IMARCTAN("1+i")', 'IMARCTAN("0.5")', 'IMARCTAN("2i")', 'IMARCTAN("-2i")', 'IMARCTAN("0.5i")',
          'IMARCTAN("0.01+0.01i")',
          'IMARCSEC("1+i")', 'IMARCSEC("2")', 'IMARCSEC("0.5")', 'IMARCSEC("0")',
          'IMARCCSC("1+i")', 'IMARCCSC("2")', 'IMARCCSC("-0.5j")', 'IMARCCSC("0")',
          'IMARCCOT("1+i")', 'IMARCCOT("0")', 'IMARCCOT("2")', 'IMARCCOT("-3j")',
          'IMARCSINH("1+i")', 'IMARCSINH("0.5")', 'IMARCSINH("2i")', 'IMARCSINH("-3+j")',
          'IMARCCOSH("1+i")', 'IMARCCOSH("1")', 'IMARCCOSH("0.5")', 'IMARCCOSH("2")', 'IMARCCOSH("-2j")',
          'IMARCTANH("1+i")', 'IMARCTANH("0.5")', 'IMARCTANH("2")', 'IMARCTANH("-2")', 'IMARCTANH("1")',
          'IMARCTANH("0.5j")',
          'IMARCSECH("1+i")', 'IMARCSECH("0.5")', 'IMARCSECH("2")', 'IMARCSECH("0")',
          'IMARCCSCH("1+i")', 'IMARCCSCH("2")', 'IMARCCSCH("0.5j")', 'IMARCCSCH("0")',
          'IMARCCOTH("1+i")', 'IMARCCOTH("2")', 'IMARCCOTH("0.5")', 'IMARCCOTH("3j")',
          'IMFACT("0.5+i")', 'IMFACT("4")', 'IMFACT("-0.5+2i")', 'IMFACT("i")', 'IMFACT("-1")',
          'IMGAMMA("1+i")', 'IMGAMMA("5")', 'IMGAMMA("-1.5+0.5i")', 'IMGAMMA("0.5")', 'IMGAMMA("0")',
          'IMGAMMA("3-4j")',
          'IMIGAMMA("1+i","2")', 'IMIGAMMA("2","3")', 'IMIGAMMA("2","3",FALSE)',
          'IMIGAMMA("2","3",TRUE,FALSE)', 'IMIGAMMA("1+i","2",FALSE,FALSE)', 'IMIGAMMA("-1","2")',
          'IMIGAMMA("0.5","-1")', 'IMIGAMMA("1+i","40",FALSE)', 'IMIGAMMA("0","2",TRUE,FALSE)',
          'IMIGAMMA("2+i","10",FALSE)', 'IMIGAMMA("i","5+5i",FALSE)',
          'IMIGAMMA("-2.5","3+4i",FALSE,FALSE)'] + list(MPMATH_WINS):
    gn(f, "complex", 1e-13)

# --- Bessel (LibreOffice's J and Y iterate where Improv calls musl's jn/yn) ----
for f in ["BESSELJ(1.9,2)", "BESSELJ(1.9,2.9)", "BESSELJ(-1.9,1)", "BESSELJ(0,0)", "BESSELJ(0,3)",
          "BESSELJ(10,5)", "BESSELJ(100,1)", "BESSELJ(1,-1)", "BESSELJ(30,40)",
          "BESSELI(1.5,1)", "BESSELI(-1.5,1)", "BESSELI(0,0)", "BESSELI(10,3)", "BESSELI(0.01,2)",
          "BESSELI(1,-1)", "BESSELI(50,0)",
          "BESSELK(1.5,1)", "BESSELK(0.5,0)", "BESSELK(3,0)", "BESSELK(5,4)", "BESSELK(0.1,3)",
          "BESSELK(0,1)", "BESSELK(1,-1)", "BESSELK(2,2)",
          "BESSELY(2.5,1)", "BESSELY(0.5,0)", "BESSELY(10,3)", "BESSELY(1,5)", "BESSELY(-1,1)",
          "BESSELY(1,-1)", "BESSELY(100,2)"]:
    lo(f)
    TOL[f] = 1e-9

# --- CONVERT ----------------------------------------------------------------
# Products disagree on some sizes in the 6th or 7th digit (LibreOffice's pound
# is 453.5923097 g, its slug, IT calorie and BTU are older approximations; Improv
# uses the exact definitions: 453.59237 g, lbf·s²/ft, 4.1868 J, 1055.05585262 J),
# so these rows hold to 1e-5 relative.
for f in ['CONVERT(1,"lbm","kg")', 'CONVERT(68,"F","C")', 'CONVERT(2.5,"ft","sec")',
          'CONVERT(1,"g","m")', 'CONVERT(1,"xyz","m")', 'CONVERT(1,"kmi","m")',
          'CONVERT(1,"mibit","bit")', 'CONVERT(10,"ozm","g")', 'CONVERT(1,"stone","lbm")',
          'CONVERT(1,"LTON","ton")', 'CONVERT(1,"sg","kg")', 'CONVERT(1,"u","g")',
          'CONVERT(5.8,"m","in")', 'CONVERT(1,"mi","km")', 'CONVERT(1,"Nmi","ft")',
          'CONVERT(72,"Pica","in")',
          'CONVERT(1,"pica","Pica")', 'CONVERT(1,"survey_mi","mi")', 'CONVERT(1,"dam","m")',
          'CONVERT(1,"yr","day")', 'CONVERT(90,"mn","hr")', 'CONVERT(1,"ms","s")',
          'CONVERT(1,"atm","Pa")', 'CONVERT(1,"atm","mmHg")', 'CONVERT(1,"psi","kPa")',
          'CONVERT(1,"lbf","N")', 'CONVERT(1,"N","dyn")', 'CONVERT(1,"pond","N")',
          'CONVERT(7.9,"cal","J")', 'CONVERT(1,"BTU","J")', 'CONVERT(1,"kWh","J")',
          'CONVERT(1,"eV","J")', 'CONVERT(1,"HPh","J")', 'CONVERT(1,"flb","J")', 'CONVERT(1,"c","cal")',
          'CONVERT(1,"HP","W")', 'CONVERT(1,"PS","W")', 'CONVERT(1,"T","ga")', 'CONVERT(1,"mT","ga")',
          'CONVERT(100,"C","F")', 'CONVERT(0,"K","C")', 'CONVERT(32,"F","K")', 'CONVERT(100,"C","Reau")',
          'CONVERT(491.67,"Rank","C")', 'CONVERT(1,"mK","K")', 'CONVERT(-500,"C","K")',
          'CONVERT(1,"gal","l")', 'CONVERT(1,"uk_gal","l")', 'CONVERT(1,"cup","tsp")',
          'CONVERT(1,"m3","l")', 'CONVERT(1,"ft3","in3")', 'CONVERT(1,"m^3","ft3")',
          'CONVERT(1,"barrel","gal")', 'CONVERT(1,"ml","tspm")',
          'CONVERT(1,"cm3","ml")',
          'CONVERT(1,"m2","ft2")', 'CONVERT(1,"mm2","m2")', 'CONVERT(1,"ha","ar")',
          'CONVERT(1,"uk_acre","m2")', 'CONVERT(1,"us_acre","m2")', 'CONVERT(1,"Morgen","ha")',
          'CONVERT(1,"m^2","ft2")',
          'CONVERT(1,"byte","bit")', 'CONVERT(1,"Gibyte","bit")', 'CONVERT(1,"kibyte","kbyte")',
          'CONVERT(3,"Yibyte","bit")', 'CONVERT(1,"cbyte","bit")',
          'CONVERT(1,"kn","m/s")', 'CONVERT(100,"km/h","mph")',
          'CONVERT(1,"m/s","m/hr")']:
    lo(f)
    TOL[f] = 1e-5
lo('CONVERT(1,"Torr","Pa")')
TOL['CONVERT(1,"Torr","Pa")'] = 1e-5
# LibreOffice's light-year (9.4605e15 m), parsec, measurement ton (0.00025 GRT)
# and admiralty knot (0.9999999 kn) are wrong; Gnumeric's match the definitions.
for f in ['CONVERT(1,"ly","m")', 'CONVERT(1,"ly","km")', 'CONVERT(1,"pc","ly")',
          'CONVERT(1,"MTON","GRT")', 'CONVERT(1,"admkn","kn")']:
    gn(f, "num", 1e-6)
# Excel documents "p" for the pascal and "Picapt" for the pica point; LibreOffice lacks both.
lo('CONVERT(1,"p","Pa")', excel=1)
lo('CONVERT(1,"Picapt","in")', excel={"approx": 1 / 72, "tol": 1e-12})

# --- runners ----------------------------------------------------------------
def map_error(f, shown):
    if shown in p1.LO_ERRORS:
        return err(shown)
    if shown == "Err:502":
        return NA if f.startswith("CONVERT(") else NUM
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


def complex_parts(text):
    """Gnumeric's complex text ("a", "bi", "a+bi", "-i", ...) as (re, im)."""
    if text[-1] not in "ij":
        return float(text), 0.0
    body = text[:-1]
    k = max((i for i, c in enumerate(body) if c in "+-" and i > 0 and body[i - 1] not in "eE"),
            default=None)
    re_s, im_s = ("", body) if k is None else (body[:k], body[k:])
    im = {"": 1.0, "+": 1.0, "-": -1.0}.get(im_s)
    return (float(re_s) if re_s else 0.0), (float(im_s) if im is None else im)


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
        elif f in TOL and not isinstance(v, dict):
            rows.append((f, {"approx": v, "tol": TOL[f]}, "LibreOffice"))
        else:
            rows.append((f, v, "LibreOffice"))
    shown = gnumeric_values()
    for f, kind, tol in GN:
        if f in MPMATH_WINS:
            v = mpmath_value(*MPMATH_WINS[f])
            print(f"Gnumeric loses this: {f} = {shown[f]}, mpmath {v!r}", file=sys.stderr)
            for part, x in (("IMREAL", v.real), ("IMAGINARY", v.imag)):
                rows.append((f"{part}({f})", {"approx": x, "tol": tol}, MPMATH_WINS[f][4]))
            continue
        if kind == "complex" and not shown[f].startswith("#"):
            re_, im_ = complex_parts(shown[f])
            for part, x in (("IMREAL", re_), ("IMAGINARY", im_)):
                rows.append((f"{part}({f})", {"approx": x, "tol": tol}, "Gnumeric 1.12"))
            continue
        v = gn_value(shown[f], kind)
        if tol is not None and not isinstance(v, (dict, str)):
            v = {"approx": v, "tol": tol}
        rows.append((f, v, "Gnumeric 1.12"))
    for f, _, _ in rows:
        # No ^ operator (a unit string such as "m^3" is fine).
        assert "\t" not in f and "^" not in re.sub(r'"[^"]*"', "", f), f
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
