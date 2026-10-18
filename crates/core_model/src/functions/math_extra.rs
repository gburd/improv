//! P7: more math: special functions, number theory, floating point, calendars, text extras. Ids 1450–1549.

use super::{alias, s, so, Arg, Category as C, FnDef};
use crate::value::ValueType as V;
use Arg::{Any as A, Number as N, Text as T};

#[rustfmt::skip]
pub(super) static MATH_EXTRA: &[FnDef] = &[
    // --- text (1480–1489) ---
    s(1480, "ASC", C::Text, &[T], V::Text, "full-width ASCII and katakana as half-width, as LibreOffice's ASC (a voiced kana becomes its letter and a sound mark)"),
    alias(s(1481, "JIS", C::Text, &[T], V::Text, "half-width ASCII and katakana as full-width, as LibreOffice's JIS (a letter and its sound mark join)"), &["DBCS"]),
    s(1482, "BAHTTEXT", C::Text, &[N], V::Text, "a number as Thai baht and satang in words, rounded to the satang"),
    s(1483, "ROT13", C::Text, &[T], V::Text, "ASCII letters rotated 13 places; anything else unchanged"),
    s(1484, "ENCODEURL", C::Text, &[T], V::Text, "UTF-8 percent-encoding of every byte but A–Z a–z 0–9 - _ . ~ (#VALUE! for empty text)"),
    so(1485, "ROMAN", C::Text, &[N, A], 1, V::Text, "0–3999 in Roman numerals; form 0 classic (or TRUE) to 4 most concise (or FALSE)"),
    s(1486, "ARABIC", C::Text, &[T], V::Number, "Roman numerals (0–3999) as a number; case and surrounding spaces ignored, a leading - negates, \"\" is 0"),
];
