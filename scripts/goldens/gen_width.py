#!/usr/bin/env python3
"""Emit the ASC/JIS width-folding tables (from LibreOffice's
i18nutil/source/utility/widthfolding_data.h) as a Rust module."""
import re
import sys

src = open(sys.argv[1], encoding="utf-8").read()


def block(name):
    i = src.index(name)
    j = src.index("};", i)
    return src[i:j]


def pairs(name, flag):
    out = []
    for m in re.finditer(r"\{\s*0x([0-9A-Fa-f]+),\s*0x([0-9A-Fa-f]+),\s*([A-Z0-9_ |]+)\}", block(name)):
        if flag in m.group(3):
            out.append((int(m.group(1), 16), int(m.group(2), 16)))
    keys = [a for a, _ in out]
    assert len(keys) == len(set(keys)), name
    return sorted(out)


def rows(name):
    return [(int(a, 16), int(b, 16)) for a, b in
            re.findall(r"\{\s*0x([0-9A-Fa-f]+),\s*0x([0-9A-Fa-f]+)\s*\}", block(name))]


asc = pairs("full2half[]", "FULL2HALF_ASC_FUNCTION")
jis = pairs("half2full[]", "HALF2FULL_JIS_FUNCTION")
dec = rows("decomposition_table[]")
comp = rows("composition_table[][2]")
assert len(dec) == 0xC0 and len(comp) == 0xC0
decomp = [(0x3040 + i, a, b) for i, (a, b) in enumerate(dec) if a and 0x30A0 <= 0x3040 + i]
compose = []
for i, (v, s) in enumerate(comp):
    for j, r in enumerate((v, s)):
        if r:
            compose.append((0x3040 + i, j, r))


def emit(name, ty, items, fmt):
    print(f"pub(super) static {name}: &[{ty}] = &[")
    line = "   "
    for it in items:
        cell = " " + fmt(it) + ","
        if len(line) + len(cell) > 99:
            print(line)
            line = "   "
        line += cell
    print(line)
    print("];\n")


print("//! Width-folding tables for ASC and JIS, generated from LibreOffice's")
print("//! `i18nutil/source/utility/widthfolding_data.h` (the entries its")
print("//! `FULLWIDTH_HALFWIDTH_LIKE_ASC` and `HALFWIDTH_FULLWIDTH_LIKE_JIS`")
print("//! transliterations use) by `scripts/goldens/gen_width.py`. Do not edit.\n")
h = lambda t: "(" + ", ".join(f"0x{x:04X}" if x > 1 else str(x) for x in t) + ")"  # noqa: E731
print("/// Full-width → half-width, sorted by the first code point.")
emit("FULL_TO_HALF", "(u16, u16)", asc, h)
print("/// Half-width → full-width, sorted by the first code point.")
emit("HALF_TO_FULL", "(u16, u16)", jis, h)
print("/// A voiced katakana → its base letter and its (spacing) sound mark.")
emit("DECOMPOSE", "(u16, u16, u16)", decomp, h)
print("/// (kana, mark: 0 voiced, 1 semi-voiced) → the composed kana, sorted.")
emit("COMPOSE", "(u16, u8, u16)", compose, h)
