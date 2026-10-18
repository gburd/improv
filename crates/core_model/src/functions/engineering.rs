//! P7: engineering: base conversion, bits, complex numbers, Bessel functions, units. Ids 1300–1449.
//!
//! Base-conversion and complex-number arguments are [`Arg::Any`]: Excel
//! takes `BIN2DEC(1010)` as well as `BIN2DEC("1010")`, and `IMSUM("1+i", 2)`.

use super::{alias, s, so, Arg, Category, FnDef};
use crate::value::ValueType;

use Arg::{Any as A, Number as N};
use Category as C;
use ValueType as V;

#[rustfmt::skip]
pub(super) static ENGINEERING: &[FnDef] = &[
    // --- bases: 10 digits at most, negatives in 10-digit two's complement ---
    s(1300, "BIN2DEC", C::Engineering, &[A], V::Number, "binary text as a number"),
    so(1301, "BIN2HEX", C::Engineering, &[A, N], 1, V::Text, "binary text as hexadecimal, zero-padded to `places`"),
    so(1302, "BIN2OCT", C::Engineering, &[A, N], 1, V::Text, "binary text as octal, zero-padded to `places`"),
    so(1303, "DEC2BIN", C::Engineering, &[N, N], 1, V::Text, "an integer in −512…511 as binary text"),
    so(1304, "DEC2HEX", C::Engineering, &[N, N], 1, V::Text, "an integer in −2³⁹…2³⁹−1 as hexadecimal text"),
    so(1305, "DEC2OCT", C::Engineering, &[N, N], 1, V::Text, "an integer in −2²⁹…2²⁹−1 as octal text"),
    so(1306, "HEX2BIN", C::Engineering, &[A, N], 1, V::Text, "hexadecimal text as binary"),
    s(1307, "HEX2DEC", C::Engineering, &[A], V::Number, "hexadecimal text as a number"),
    so(1308, "HEX2OCT", C::Engineering, &[A, N], 1, V::Text, "hexadecimal text as octal"),
    so(1309, "OCT2BIN", C::Engineering, &[A, N], 1, V::Text, "octal text as binary"),
    s(1310, "OCT2DEC", C::Engineering, &[A], V::Number, "octal text as a number"),
    so(1311, "OCT2HEX", C::Engineering, &[A, N], 1, V::Text, "octal text as hexadecimal"),
    alias(so(1312, "BASE", C::Engineering, &[N, N, N], 2, V::Text, "an integer in 0…2⁵³−1 as text in a radix 2…36, zero-padded to `min_length`"), &["NUMTOBASE"]),
    alias(s(1313, "DECIMAL", C::Engineering, &[A, N], V::Number, "text in a radix 2…36 as a number"), &["BASETONUM"]),
    s(1314, "HEXREP", C::Engineering, &[N], V::Text, "the bytes of a number's IEEE double, little-endian, in hexadecimal"),
    // --- bits: integers in 0…2⁴⁸−1 ---
    s(1315, "BITAND", C::Engineering, &[N, N], V::Number, "bitwise and"),
    s(1316, "BITOR", C::Engineering, &[N, N], V::Number, "bitwise or"),
    s(1317, "BITXOR", C::Engineering, &[N, N], V::Number, "bitwise exclusive or"),
    s(1318, "BITLSHIFT", C::Engineering, &[N, N], V::Number, "shifted left by `shift` bits (right if negative), |shift| ≤ 53"),
    s(1319, "BITRSHIFT", C::Engineering, &[N, N], V::Number, "shifted right by `shift` bits (left if negative), |shift| ≤ 53"),
    // --- tests ---
    so(1320, "DELTA", C::Engineering, &[N, N], 1, V::Number, "1 if the numbers are equal (b defaults to 0), else 0"),
    so(1321, "GESTEP", C::Engineering, &[N, N], 1, V::Number, "1 if x ≥ step (default 0), else 0"),
];
