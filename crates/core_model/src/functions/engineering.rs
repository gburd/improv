//! P7: engineering: base conversion, bits, complex numbers, Bessel functions, units. Ids 1300–1449.
//!
//! Base-conversion and complex-number arguments are [`Arg::Any`]: Excel
//! takes `BIN2DEC(1010)` as well as `BIN2DEC("1010")`, and `IMSUM("1+i", 2)`.

use super::{alias, s, so, sv, Arg, Category, FnDef};
use crate::value::ValueType;

use Arg::{Any as A, Boolean as B, Number as N, Text as T};
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
    s(1318, "BITLSHIFT", C::Engineering, &[N, N], V::Number, "shifted left by `shift_amount` bits (right if negative), at most 53 either way"),
    s(1319, "BITRSHIFT", C::Engineering, &[N, N], V::Number, "shifted right by `shift_amount` bits (left if negative), at most 53 either way"),
    // --- units ---
    s(1390, "CONVERT", C::Engineering, &[N, T, T], V::Number, "a measurement converted between units of Excel's table (SI and binary prefixes allowed); #N/A for unknown or mismatched units"),
    // --- tests ---
    so(1320, "DELTA", C::Engineering, &[N, N], 1, V::Number, "1 if the numbers are equal (number2 defaults to 0), else 0"),
    so(1321, "GESTEP", C::Engineering, &[N, N], 1, V::Number, "1 if number ≥ step (default 0), else 0"),
    // --- complex numbers: "a+bi" text (or a number), suffix i or j ---
    so(1330, "COMPLEX", C::Engineering, &[N, N, T], 2, V::Text, "the complex number re + im·i as text, with suffix \"i\" (default) or \"j\""),
    s(1331, "IMREAL", C::Engineering, &[A], V::Number, "a complex number's real part"),
    s(1332, "IMAGINARY", C::Engineering, &[A], V::Number, "a complex number's imaginary part"),
    s(1333, "IMABS", C::Engineering, &[A], V::Number, "a complex number's modulus |z|"),
    s(1334, "IMARGUMENT", C::Engineering, &[A], V::Number, "a complex number's angle in radians, −π…π (#NUM! at 0)"),
    s(1335, "IMCONJUGATE", C::Engineering, &[A], V::Text, "the complex conjugate"),
    s(1336, "IMNEG", C::Engineering, &[A], V::Text, "−z"),
    s(1337, "IMINV", C::Engineering, &[A], V::Text, "1/z"),
    sv(1338, "IMSUM", C::Engineering, &[A], V::Text, "the sum of complex numbers"),
    sv(1339, "IMPRODUCT", C::Engineering, &[A], V::Text, "the product of complex numbers"),
    s(1340, "IMSUB", C::Engineering, &[A, A], V::Text, "z1 − z2"),
    s(1341, "IMDIV", C::Engineering, &[A, A], V::Text, "z1 / z2"),
    s(1342, "IMPOWER", C::Engineering, &[A, N], V::Text, "z raised to a real power"),
    s(1343, "IMSQRT", C::Engineering, &[A], V::Text, "the principal square root"),
    s(1344, "IMEXP", C::Engineering, &[A], V::Text, "e^z"),
    s(1345, "IMLN", C::Engineering, &[A], V::Text, "the principal natural logarithm"),
    s(1346, "IMLOG10", C::Engineering, &[A], V::Text, "the principal base-10 logarithm"),
    s(1347, "IMLOG2", C::Engineering, &[A], V::Text, "the principal base-2 logarithm"),
    s(1348, "IMSIN", C::Engineering, &[A], V::Text, "the complex sine"),
    s(1349, "IMCOS", C::Engineering, &[A], V::Text, "the complex cosine"),
    s(1350, "IMTAN", C::Engineering, &[A], V::Text, "the complex tangent"),
    s(1351, "IMSEC", C::Engineering, &[A], V::Text, "the complex secant"),
    s(1352, "IMCSC", C::Engineering, &[A], V::Text, "the complex cosecant"),
    s(1353, "IMCOT", C::Engineering, &[A], V::Text, "the complex cotangent"),
    s(1354, "IMSINH", C::Engineering, &[A], V::Text, "the complex hyperbolic sine"),
    s(1355, "IMCOSH", C::Engineering, &[A], V::Text, "the complex hyperbolic cosine"),
    s(1356, "IMTANH", C::Engineering, &[A], V::Text, "the complex hyperbolic tangent"),
    s(1357, "IMSECH", C::Engineering, &[A], V::Text, "the complex hyperbolic secant"),
    s(1358, "IMCSCH", C::Engineering, &[A], V::Text, "the complex hyperbolic cosecant"),
    s(1359, "IMCOTH", C::Engineering, &[A], V::Text, "the complex hyperbolic cotangent"),
    s(1360, "IMARCSIN", C::Engineering, &[A], V::Text, "the complex arcsine (principal branch)"),
    s(1361, "IMARCCOS", C::Engineering, &[A], V::Text, "the complex arccosine"),
    s(1362, "IMARCTAN", C::Engineering, &[A], V::Text, "the complex arctangent"),
    s(1363, "IMARCSEC", C::Engineering, &[A], V::Text, "the complex arcsecant"),
    s(1364, "IMARCCSC", C::Engineering, &[A], V::Text, "the complex arccosecant"),
    s(1365, "IMARCCOT", C::Engineering, &[A], V::Text, "the complex arccotangent"),
    s(1366, "IMARCSINH", C::Engineering, &[A], V::Text, "the complex inverse hyperbolic sine"),
    s(1367, "IMARCCOSH", C::Engineering, &[A], V::Text, "the complex inverse hyperbolic cosine"),
    s(1368, "IMARCTANH", C::Engineering, &[A], V::Text, "the complex inverse hyperbolic tangent"),
    s(1369, "IMARCSECH", C::Engineering, &[A], V::Text, "the complex inverse hyperbolic secant"),
    s(1370, "IMARCCSCH", C::Engineering, &[A], V::Text, "the complex inverse hyperbolic cosecant"),
    s(1371, "IMARCCOTH", C::Engineering, &[A], V::Text, "the complex inverse hyperbolic cotangent"),
    s(1372, "IMFACT", C::Engineering, &[A], V::Text, "the complex factorial z! = Γ(z + 1)"),
    s(1373, "IMGAMMA", C::Engineering, &[A], V::Text, "the complex gamma function Γ(z)"),
    // --- Bessel functions of integer order n (truncated, ≥ 0) ---
    s(1380, "BESSELI", C::Engineering, &[N, N], V::Number, "the modified Bessel function Iₙ(x)"),
    s(1381, "BESSELJ", C::Engineering, &[N, N], V::Number, "the Bessel function Jₙ(x)"),
    s(1382, "BESSELK", C::Engineering, &[N, N], V::Number, "the modified Bessel function Kₙ(x), x > 0"),
    s(1383, "BESSELY", C::Engineering, &[N, N], V::Number, "the Bessel function Yₙ(x), x > 0"),
    so(1374, "IMIGAMMA", C::Engineering, &[A, A, B, B], 2, V::Text, "the complex incomplete gamma function of a and z: lower (default) or upper, regularized (default) or not"),
];
