#!/usr/bin/env python3
"""The D5 goldens (crates/engine/tests/sum_10k.rs): SUM and AVERAGE of
10,000 fractional values, the exact sum by mpmath (3,000-bit floats, so
exact for any 10,000 doubles) rounded once to the nearest double, and the
mean as that double divided by 10,000 in IEEE arithmetic (what Improv
promises: the correctly rounded sum, then one division).

The values are the engine probe's LCG (tests/probe.rs) in three shapes:
uniform in [0.1, 1.1), signed in [-1, 1), and mixed magnitudes (values in
[-0.5, 0.5) scaled by 1 to 1e6), which cancel. Prints the Rust table."""
import struct
import mpmath

mpmath.mp.prec = 3000


def lcg(s):
    s = (s * 6364136223846793005 + 1442695040888963407) % 2**64
    return s, (s >> 11) / 2**53


def values(kind, seed, n=10000):
    xs, s = [], seed
    for _ in range(n):
        s, u = lcg(s)
        if kind == "Uniform":
            xs.append(u + 0.1)
        elif kind == "Signed":
            xs.append(u * 2 - 1)
        else:
            s, v = lcg(s)
            xs.append((u - 0.5) * [1.0, 10.0, 100.0, 1000.0, 1e4, 1e5, 1e6][int(v * 7)])
    return xs


def bits(x):
    return "0x%016x" % struct.unpack("<Q", struct.pack("<d", x))[0]


for kind, seed in [("Uniform", 42), ("Signed", 7), ("Mixed", 1234)]:
    xs = values(kind, seed)
    total = float(mpmath.fsum([mpmath.mpf(x) for x in xs]))
    naive = 0.0
    for x in xs:
        naive += x
    print(f"    (Shape::{kind}, {seed}, {bits(total)}, {bits(total / len(xs))}), "
          f"// SUM {total!r}, AVERAGE {total / len(xs)!r}; a left fold: {naive!r}")
