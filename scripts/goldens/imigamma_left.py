#!/usr/bin/env python3
"""IMIGAMMA for Re z <= 0: mpmath 1.3 gammainc on the principal branch.

upper = gammainc(a, z) (z to infinity), lower = gamma(a) - upper, and the
regularized forms divide by gamma(a). At a = 0, -1, -2, ... gamma(a) is a
pole: P = 1 and Q = 0 by convention (as IMIGAMMA has always given), the
unregularized lower form is a pole (skipped), the upper one finite.

`rust` prints tuples for the unit test, anything else the TSV golden rows.
"""
import sys
import mpmath as mp

mp.mp.dps = 250

A = [0.5, 1, 2.5, -0.5, -2.5, 0, -1, -3, 1 + 1j, -1.5 + 2j, 3 - 1j, 1j, 7.25]
Z = [-1, -0.5, -5, -12, -1 + 2j, -3 - 4j, 3j, -10 + 1j, -0.1, -2.5j]


def pole(a):
    a = complex(a)
    return a.imag == 0 and a.real <= 0 and a.real == int(a.real)


def val(a, z, lower, reg):
    if pole(a):
        if reg:
            return complex(1 if lower else 0)
        if lower:
            return None
    a, z = mp.mpc(a), mp.mpc(z)
    up = mp.gammainc(a, z)
    if pole(complex(a)):
        return complex(up)
    g = mp.gamma(a)
    v = g - up if lower else up
    return complex(v / g if reg else v)


rows = [(complex(a), complex(z), lo, rg, val(a, z, lo, rg))
        for a in A for z in Z for lo in (True, False) for rg in (True, False)]
rows = [r for r in rows if r[4] is not None]

if sys.argv[1:] == ["rust"]:
    for a, z, lo, rg, v in rows:
        print(f"    (({a.real!r}, {a.imag!r}), ({z.real!r}, {z.imag!r}), "
              f"{str(lo).lower()}, {str(rg).lower()}, ({v.real!r}, {v.imag!r})),")
else:
    for a, z, lo, rg in [("0", "-1", False, False), ("0", "-2.5", False, False),
                         ("-1", "-1", False, False), ("-2", "-0.5", False, False),
                         ("0.5", "-2", False, False), ("2.5", "-3", True, False),
                         ("0", "-1+2i", False, False), ("1.5", "-4", True, True),
                         ("1+i", "-3-4i", False, True), ("-1.5+2i", "-5", True, False)]:
        v = val(complex(a.replace("i", "j")), complex(z.replace("i", "j")), lo, rg)
        f = f'IMIGAMMA("{a}","{z}",{str(lo).upper()},{str(rg).upper()})'
        for part, x in (("IMREAL", v.real), ("IMAGINARY", v.imag)):
            print(f'{part}({f})\t{{"approx": {x!r}, "tol": 1e-13}}\t'
                  f'mpmath 1.3 gammainc (Gnumeric 1.12 gives #NUM!)')
