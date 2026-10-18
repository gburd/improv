//! P7: more math: special functions, number theory, floating point, calendars, text extras. Ids 1450–1549.

use super::{alias, s, so, Arg, Category as C, FnDef};
use crate::value::ValueType as V;
use Arg::{Any as A, DateLike as D, Number as N, Text as T};

#[rustfmt::skip]
pub(super) static MATH_EXTRA: &[FnDef] = &[
    // --- special functions (1450–1459) ---
    s(1451, "AGM", C::Math, &[N, N], V::Number, "the arithmetic-geometric mean of a, b ≥ 0"),
    s(1452, "GD", C::Math, &[N], V::Number, "the Gudermannian function: 2·atan(tanh(x/2))"),
    so(1453, "ILOG", C::Math, &[N, N], 1, V::Number, "the integer logarithm: the largest k with baseᵏ ≤ x, for x > 0 and an integer base ≥ 2 (default 10)"),
    s(1455, "FIB", C::Math, &[N], V::Number, "the n-th Fibonacci number, n ≥ 1 (truncated); #NUM! past n = 1476, where it overflows"),
    // --- number theory (1460–1474); n is truncated, and at most 2⁵² as in Gnumeric ---
    s(1460, "ISPRIME", C::Math, &[N], V::Boolean, "whether n is prime (FALSE below 2)"),
    s(1461, "ITHPRIME", C::Math, &[N], V::Number, "the i-th prime, i from 1 to 10⁷ (Gnumeric: 10⁸)"),
    s(1462, "PFACTOR", C::Math, &[N], V::Number, "the smallest prime factor of n ≥ 2 (#VALUE! below 2)"),
    s(1463, "NT_D", C::Math, &[N], V::Number, "the number of divisors of n ≥ 1"),
    s(1464, "NT_SIGMA", C::Math, &[N], V::Number, "the sum of the divisors of n ≥ 1"),
    s(1465, "NT_PHI", C::Math, &[N], V::Number, "Euler's totient: how many of 1…n are coprime to n ≥ 1"),
    s(1466, "NT_MU", C::Math, &[N], V::Number, "the Möbius function of n ≥ 1: 0 if a square divides n, else (−1)^(number of prime factors)"),
    s(1467, "NT_OMEGA", C::Math, &[N], V::Number, "the number of distinct prime factors of n ≥ 1"),
    s(1468, "NT_PI", C::Math, &[N], V::Number, "how many primes are ≤ n (0 below 2; n up to 179 424 673, the 10⁷-th prime)"),
    s(1469, "NT_RADICAL", C::Math, &[N], V::Number, "the product of the distinct prime factors of n ≥ 1"),
    s(1470, "PARTITIONS", C::Math, &[N], V::Number, "the number of ways to write n ≥ 0 as a sum of positive integers (n up to 4000)"),
    // --- floating point (1475–1479) ---
    s(1475, "FLT.MAX", C::Math, &[], V::Number, "the largest finite double, 1.7976931348623157e308"),
    s(1476, "FLT.MIN", C::Math, &[], V::Number, "the smallest positive normal double, 2.2250738585072014e-308"),
    s(1477, "FLT.RADIX", C::Math, &[], V::Number, "the floating-point base, 2"),
    s(1478, "FLT.NEXTAFTER", C::Math, &[N, A], V::Number, "the next double after x toward y (\"+\" or \"-\" for the infinities); y itself if y = x"),
    // --- calendars (1490–1509) ---
    s(1490, "EASTERSUNDAY", C::DateTime, &[N], V::DateTime, "Easter Sunday (Gregorian) of a year 1583–9956; 0–29 mean 2000–2029, 30–99 mean 1930–1999"),
    s(1491, "ASCENSIONTHURSDAY", C::DateTime, &[N], V::DateTime, "Ascension Thursday, 39 days after Easter Sunday"),
    s(1492, "ASHWEDNESDAY", C::DateTime, &[N], V::DateTime, "Ash Wednesday, 46 days before Easter Sunday"),
    s(1493, "GOODFRIDAY", C::DateTime, &[N], V::DateTime, "Good Friday, 2 days before Easter Sunday"),
    s(1494, "PENTECOSTSUNDAY", C::DateTime, &[N], V::DateTime, "Pentecost Sunday, 49 days after Easter Sunday"),
    s(1495, "DATE2JULIAN", C::DateTime, &[D], V::Number, "the Julian day number of a date"),
    s(1496, "HDATE_JULIAN", C::DateTime, &[N, N, N], V::Number, "the Julian day number of Gregorian year, month, day (out-of-range days and months roll over)"),
    // --- text (1480–1489) ---
    s(1480, "ASC", C::Text, &[T], V::Text, "full-width ASCII and katakana as half-width, as LibreOffice's ASC (a voiced kana becomes its letter and a sound mark)"),
    alias(s(1481, "JIS", C::Text, &[T], V::Text, "half-width ASCII and katakana as full-width, as LibreOffice's JIS (a letter and its sound mark join)"), &["DBCS"]),
    s(1482, "BAHTTEXT", C::Text, &[N], V::Text, "a number as Thai baht and satang in words, rounded to the satang"),
    s(1483, "ROT13", C::Text, &[T], V::Text, "ASCII letters rotated 13 places; anything else unchanged"),
    s(1484, "ENCODEURL", C::Text, &[T], V::Text, "UTF-8 percent-encoding of every byte but A–Z a–z 0–9 - _ . ~ (#VALUE! for empty text)"),
    so(1485, "ROMAN", C::Text, &[N, A], 1, V::Text, "0–3999 in Roman numerals; form 0 classic (or TRUE) to 4 most concise (or FALSE)"),
    s(1486, "ARABIC", C::Text, &[T], V::Number, "Roman numerals (0–3999) as a number; case and surrounding spaces ignored, a leading - negates, \"\" is 0"),
];
