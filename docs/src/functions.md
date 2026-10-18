# Function Reference

<!-- Generated from crates/core_model/src/functions.rs by
     `UPDATE_DOCS=1 cargo test -p improv_core_model`. Do not edit. -->

Every built-in function, by category. Names are case-insensitive;
`[x]` is optional and `…` repeats the last argument. Positions are
1-based. An error argument (`#DIV/0!`, `#N/A`, …) makes the result
that error, except in the functions that test for errors. See
[The Formula Language](./formulas.md) for operators, types and
dimensions.

## Aggregations

Written `F(Measure OVER Category)`: collapse one category. Each also has a scalar form over its arguments, listed under its category.

| Function | Returns | Description |
|---|---|---|
| `SUM(Measure OVER Category)` | number | the sum over a category (also `DSUM`) |
| `AVERAGE(Measure OVER Category)` | number | the mean over a category (also `AVG`, `DAVERAGE`) |
| `MIN(Measure OVER Category)` | number | the smallest value over a category (also `DMIN`) |
| `MAX(Measure OVER Category)` | number | the largest value over a category (also `DMAX`) |
| `PRODUCT(Measure OVER Category)` | number | the product over a category (also `DPRODUCT`) |
| `SUMSQ(Measure OVER Category)` | number | the sum of squares over a category |
| `AND(Measure OVER Category)` | boolean | whether every value over a category is true |
| `OR(Measure OVER Category)` | boolean | whether any value over a category is true |
| `XOR(Measure OVER Category)` | boolean | whether an odd number of values over a category are true |
| `FIRST(Measure OVER Category)` | the branches' type | the value at the first item (in category order) that has one |
| `LAST(Measure OVER Category)` | the branches' type | the value at the last item that has one |
| `S(Measure OVER Category)` | text | the first value, as text (Quantrix) |
| `TEXTJOIN(text, boolean, value OVER category)` | text | the values in item order joined by the separator (skipping empty text when skip_empty is TRUE) |

## Logical

`IF` and the functions built on it evaluate only the branch they choose, so an error or missing cell in the other branch does not matter.

| Function | Returns | Description |
|---|---|---|
| `AND(boolean, …)` | boolean | whether every argument is true |
| `OR(boolean, …)` | boolean | whether any argument is true |
| `XOR(boolean, …)` | boolean | whether an odd number of arguments are true |
| `IF(boolean, value, [value])` | the branches' type | then where the test is true, else (or no cell) where it is false |
| `IFS(boolean, value, …)` | the branches' type | the value paired with the first true test |
| `SWITCH(value, value, value, …)` | the branches' type | the result paired with the first value equal to the expression, else the default |
| `CHOOSE(number, value, …)` | the branches' type | the index-th value (1-based) |
| `IFERROR(value, value)` | the branches' type | the value, or the alternative where it is an error |
| `IFNA(value, value)` | the branches' type | the value, or the alternative where it is #N/A |
| `CLEARERROR(value, [value])` | the branches' type | the value, or the alternative (or no cell) where it is an error |
| `CLEARZERO(number, [number])` | number | the value, or the alternative (or no cell) where it is zero |
| `BETWEEN(number, number, number, [boolean])` | boolean | whether low ≤ x ≤ high (strict when inclusive is FALSE) |
| `TRUE()` | boolean | TRUE |
| `FALSE()` | boolean | FALSE |
| `SCAN(value, value, value)` | the branches' type | SCAN(init, x OVER T, LAMBDA(acc, v, body)): the running fold along T, one value per item |
| `REDUCE(value, value, value)` | the branches' type | REDUCE(init, x OVER T, LAMBDA(acc, v, body)): the fold's last value |
| `LET(value, value, value, …)` | the branches' type | LET(name, value, …, body): the body with each name bound to its value |
| `LAMBDA(value, value, …)` | the branches' type | LAMBDA(p, …, body): a function of its parameters, called on the spot or passed to MAP, REDUCE or SCAN |
| `MAKEARRAY(value, value, value)` | the branches' type | MAKEARRAY(R, C, LAMBDA(r, c, body)): the body over R × C, with r and c the item positions |
| `BYROW(value, value)` | the branches' type | BYROW(x OVER C, LAMBDA(row, body)): the body per item of x's other categories, row being the values along C (the body reduces it: SUM(row), …) (also `BYCOL`) |
| `MAP(value, value, …)` | the branches' type | MAP(x, …, LAMBDA(p, …, body)): the body applied element by element |

## Information and errors

| Function | Returns | Description |
|---|---|---|
| `ISERROR(value)` | boolean | whether the value is any error |
| `ISERR(value)` | boolean | whether the value is an error other than #N/A |
| `ISNA(value)` | boolean | whether the value is #N/A |
| `NA()` | number | the #N/A error |
| `ERROR(text)` | number | the error a code names ("#DIV/0!") |
| `ERROR.TYPE(value)` | number | 1 #NULL!, 2 #DIV/0!, 3 #VALUE!, 4 #REF!, 5 #NAME?, 6 #NUM!, 7 #N/A; #N/A for a non-error |
| `ISBLANK(value)` | boolean | whether the value is blank: always FALSE for a value, since a missing cell has none (ISBLANK("") is FALSE) (also `ISEMPTY`) |
| `ISNUMBER(value)` | boolean | whether the value is a number; a date is one, as in Excel |
| `ISTEXT(value)` | boolean | whether the value is text |
| `ISNONTEXT(value)` | boolean | whether the value is not text (TRUE for an error) |
| `ISLOGICAL(value)` | boolean | whether the value is a boolean |
| `ISDATE(value)` | boolean | whether the value is a date |
| `ISNUMBERORDATE(value)` | boolean | whether the value is a number or a date |
| `ISEVEN(number)` | boolean | whether the integer part is even (also `ISEVEN_ADD`) |
| `ISODD(number)` | boolean | whether the integer part is odd (also `ISODD_ADD`) |
| `N(value)` | number | the value as a number: a date's serial number, TRUE 1, FALSE and text 0 |
| `T(value)` | text | the value if it is text, else "" |
| `TYPE(value)` | number | 1 number (or date), 2 text, 4 boolean, 16 error |

## Math and trigonometry

Angles are in radians.

| Function | Returns | Description |
|---|---|---|
| `SUM(number, …)` | number | the sum of the arguments |
| `AVERAGE(number, …)` | number | the mean of the arguments |
| `MIN(number, …)` | number | the smallest argument (also `MIN2`) |
| `MAX(number, …)` | number | the largest argument (also `MAX2`) |
| `PRODUCT(number, …)` | number | the product of the arguments |
| `SUMSQ(number, …)` | number | the sum of the squares of the arguments |
| `ABS(number)` | number | absolute value |
| `NEG(number)` | number | negation |
| `SIGN(number)` | number | −1, 0 or 1 |
| `SQRT(number)` | number | square root (#NUM! below zero) |
| `SQRTPI(number)` | number | √(x·π) |
| `EXP(number)` | number | e to the power x |
| `EXPM1(number)` | number | eˣ − 1, accurate near 0 |
| `LN(number)` | number | natural logarithm |
| `LN1P(number)` | number | ln(1 + x), accurate near 0 |
| `LOG(number, [number])` | number | logarithm to a base (default 10) |
| `LOG10(number)` | number | base-10 logarithm |
| `LOG2(number)` | number | base-2 logarithm |
| `POWER(number, number)` | number | base to the power exponent (same as ^) |
| `MOD(number, number)` | number | remainder, with the divisor's sign |
| `QUOTIENT(number, number)` | number | integer part of a quotient |
| `PI()` | number | π |
| `DEGREES(number)` | number | radians to degrees |
| `RADIANS(number)` | number | degrees to radians |
| `SIN(number)` | number | sine (radians) |
| `COS(number)` | number | cosine |
| `TAN(number)` | number | tangent |
| `ASIN(number)` | number | arcsine |
| `ACOS(number)` | number | arccosine |
| `ATAN(number)` | number | arctangent |
| `ATAN2(number, number)` | number | angle of the point (x, y) |
| `SINH(number)` | number | hyperbolic sine |
| `COSH(number)` | number | hyperbolic cosine |
| `TANH(number)` | number | hyperbolic tangent |
| `ASINH(number)` | number | inverse hyperbolic sine |
| `ACOSH(number)` | number | inverse hyperbolic cosine |
| `ATANH(number)` | number | inverse hyperbolic tangent |
| `SEC(number)` | number | secant |
| `CSC(number)` | number | cosecant |
| `COT(number)` | number | cotangent |
| `SECH(number)` | number | hyperbolic secant |
| `CSCH(number)` | number | hyperbolic cosecant |
| `COTH(number)` | number | hyperbolic cotangent |
| `ACOT(number)` | number | inverse cotangent |
| `ACOTH(number)` | number | inverse hyperbolic cotangent |
| `SINPI(number)` | number | sin(πx), exact at integers |
| `COSPI(number)` | number | cos(πx), exact at integers |
| `TANPI(number)` | number | tan(πx), exact at integers |
| `COTPI(number)` | number | cot(πx), exact at integers |
| `HYPOT(number, …)` | number | √(Σ xᵢ²) without overflow |
| `FACT(number)` | number | factorial |
| `FACTDOUBLE(number)` | number | double factorial n!! |
| `COMBIN(number, number)` | number | combinations of n things k at a time |
| `COMBINA(number, number)` | number | combinations with repetition |
| `PERMUT(number, number)` | number | permutations of n things k at a time |
| `PERMUTATIONA(number, number)` | number | permutations with repetition |
| `GCD(number, …)` | number | greatest common divisor |
| `LCM(number, …)` | number | least common multiple |
| `MULTINOMIAL(number, …)` | number | (Σ n)! / Π n! |
| `RAWSUBTRACT(number, …)` | number | the first argument minus the rest |
| `INVSUMINV(number, …)` | number | 1 / Σ(1/xᵢ) |
| `GAMMA(number)` | number | the gamma function Γ(x) |
| `GAMMALN(number)` | number | ln Γ(x), for x > 0 (also `GAMMALN.PRECISE`) |
| `BETA(number, number)` | number | the Euler beta function B(a, b) |
| `BETALN(number, number)` | number | ln |B(a, b)| |
| `DIGAMMA(number)` | number | the digamma function ψ(x) |
| `IGAMMA(number, number, [boolean], [boolean], [boolean])` | number | the incomplete gamma function of a and x: lower (default) or upper, regularized (default) or not; the imaginary part (real = FALSE) is 0 |
| `POCHHAMMER(number, number)` | number | the rising factorial Γ(x + n) / Γ(x) |
| `ERF(number, [number])` | number | the error function erf(x), or erf(upper) − erf(lower) |
| `ERF.PRECISE(number)` | number | the error function erf(x) |
| `ERFC(number)` | number | the complementary error function 1 − erf(x) (also `ERFC.PRECISE`) |
| `SUMA(Measure OVER Category)` | number | the sum, counting TRUE as 1 and text as 0 |
| `SUMPRODUCT(Measure OVER Category)` | number | Σ xᵢ·yᵢ·… over the rows where every measure has a cell |
| `SUMX2MY2(number OVER category, number OVER category)` | number | Σ(x² − y²) |
| `SUMX2PY2(number OVER category, number OVER category)` | number | Σ(x² + y²) |
| `SUMXMY2(number OVER category, number OVER category)` | number | Σ(x − y)² |
| `SERIESSUM(number, number, number, number OVER category)` | number | Σ cᵢ·x^(n + i·m), the coefficients c in item order (i from 0) |
| `POLYNOMIAL(number, number OVER category)` | number | Σ cᵢ·xⁱ, the coefficients c in item order (the constant c₀ first) |
| `PERCENTOF(number OVER category, number OVER category)` | number | Σ subset / Σ all |
| `AGM(number, number)` | number | the arithmetic-geometric mean of a, b ≥ 0 |
| `GD(number)` | number | the Gudermannian function: 2·atan(tanh(x/2)) |
| `ILOG(number, [number])` | number | the integer logarithm: the largest k with baseᵏ ≤ x, for x > 0 and an integer base ≥ 2 (default 10) |
| `FIB(number)` | number | the n-th Fibonacci number, n ≥ 1 (truncated); #NUM! past n = 1476, where it overflows |
| `ISPRIME(number)` | boolean | whether n is prime (FALSE below 2) |
| `ITHPRIME(number)` | number | the i-th prime, i from 1 to 10⁷ (Gnumeric: 10⁸) |
| `PFACTOR(number)` | number | the smallest prime factor of n ≥ 2 (#VALUE! below 2) |
| `NT_D(number)` | number | the number of divisors of n ≥ 1 |
| `NT_SIGMA(number)` | number | the sum of the divisors of n ≥ 1 |
| `NT_PHI(number)` | number | Euler's totient: how many of 1…n are coprime to n ≥ 1 |
| `NT_MU(number)` | number | the Möbius function of n ≥ 1: 0 if a square divides n, else (−1)^(number of prime factors) |
| `NT_OMEGA(number)` | number | the number of distinct prime factors of n ≥ 1 |
| `NT_PI(number)` | number | how many primes are ≤ n (0 below 2; n up to 179 424 673, the 10⁷-th prime) |
| `NT_RADICAL(number)` | number | the product of the distinct prime factors of n ≥ 1 |
| `PARTITIONS(number)` | number | the number of ways to write n ≥ 0 as a sum of positive integers (n up to 4000) |
| `FLT.MAX()` | number | the largest finite double, 1.7976931348623157e308 |
| `FLT.MIN()` | number | the smallest positive normal double, 2.2250738585072014e-308 |
| `FLT.RADIX()` | number | the floating-point base, 2 |
| `FLT.NEXTAFTER(number, value)` | number | the next double after x toward y ("+" or "-" for the infinities); y itself if y = x |

## Rounding

Rounding is decimal: `ROUND(2.675, 2)` is 2.68.

| Function | Returns | Description |
|---|---|---|
| `ROUND(number, [number])` | number | to digits decimal places, halves away from zero |
| `ROUNDUP(number, [number])` | number | away from zero to digits places |
| `ROUNDDOWN(number, [number])` | number | toward zero to digits places |
| `ROUNDSIG(number, number)` | number | to digits significant digits |
| `TRUNC(number, [number])` | number | truncate to digits places |
| `INT(number)` | number | down to an integer |
| `FLOOR(number, [number])` | number | down to a multiple of significance (default 1) |
| `CEIL(number)` | number | up to an integer |
| `CEILING(number, [number])` | number | up to a multiple of significance (default 1) |
| `FLOOR.MATH(number, [number], [number])` | number | down to a multiple; mode ≠ 0 rounds negatives toward zero |
| `CEILING.MATH(number, [number], [number])` | number | up to a multiple; mode ≠ 0 rounds negatives away from zero |
| `FLOOR.PRECISE(number, [number])` | number | down to a multiple, ignoring the multiple's sign |
| `CEILING.PRECISE(number, [number])` | number | up to a multiple, ignoring the multiple's sign |
| `ISO.CEILING(number, [number])` | number | same as CEILING.PRECISE |
| `MROUND(number, number)` | number | to the nearest multiple |
| `EVEN(number)` | number | away from zero to an even integer |
| `ODD(number)` | number | away from zero to an odd integer |

## Text

Lengths and positions count characters, not bytes.

| Function | Returns | Description |
|---|---|---|
| `CHAR(number)` | text | the character with code n (1–255) |
| `UNICHAR(number)` | text | the Unicode character n |
| `CODE(text)` | number | the code of the first character |
| `UNICODE(text)` | number | the code point of the first character |
| `CLEAN(text)` | text | remove non-printing characters |
| `CONCAT(value, …)` | text | join values as text (same as &) |
| `CONCATENATE(value, …)` | text | join values as text |
| `EXACT(text, text)` | boolean | case-sensitive equality |
| `FIND(text, text, [number])` | number | 1-based position of find in text, case-sensitive |
| `SEARCH(text, text, [number])` | number | 1-based position, case-insensitive, with * and ? wildcards |
| `LEFT(text, [number])` | text | the first n characters (default 1) |
| `RIGHT(text, [number])` | text | the last n characters (default 1) |
| `MID(text, number, number)` | text | n characters from a 1-based start |
| `LEN(text)` | number | the number of characters |
| `LOWER(text)` | text | lowercase |
| `UPPER(text)` | text | uppercase |
| `PROPER(text)` | text | capitalize each word |
| `REPLACE(text, number, number, text)` | text | replace n characters from a 1-based start |
| `REPT(text, number)` | text | repeat text n times |
| `SUBSTITUTE(text, text, text, [number])` | text | replace every (or the instance-th) occurrence of old |
| `TRIM(text)` | text | remove leading, trailing and repeated spaces |
| `VALUE(text)` | number | text as a number |
| `TEXTBEFORE(text, text, [number], [number], [number], [value])` | text | text before the instance-th delimiter |
| `TEXTAFTER(text, text, [number], [number], [number], [value])` | text | text after the instance-th delimiter |
| `TEXTBETWEEN(text, text, text)` | text | text between the first start delimiter and the next end delimiter |
| `COUNTMATCHES(text, text)` | number | how many times search occurs in text |
| `TEXT(value, text)` | text | format with an Excel number-format code: 0 # ? . , % E+ E- "text" \c _c *c, General, ; sections (positive;negative;zero;text with @), colors; dates y m d h s AM/PM A/P. Unsupported (#VALUE!): conditions, locales, [h] elapsed time, fractional seconds, fractions, eras |
| `FIXED(number, [number], [boolean])` | text | rounded to decimals places (default 2) as text, with thousands commas unless no_commas |
| `DOLLAR(number, [number])` | text | as en-US currency text: $1,234.57, negatives in parentheses, decimals default 2 |
| `NUMBERVALUE(text, [text], [text])` | number | text as a number with explicit decimal (default .) and group (default ,) separators |
| `VALUETOTEXT(value, [number])` | text | a value as text; format 1 quotes text |
| `REGEX(text, text, [text], [value])` | text | the first match (#N/A if none), or with a replacement the text with the first match replaced; the fourth argument is "g" (replace all) or the occurrence to replace. Patterns use the Rust regex syntax, not ICU's |
| `REGEXTEST(text, text, [number])` | boolean | whether the pattern matches; case_sensitivity 1 ignores case. Rust regex syntax |
| `REGEXEXTRACT(text, text, [number], [number])` | text | the first match (#N/A if none); only return mode 0, modes 1 and 2 (arrays) are #VALUE!. Rust regex syntax (also `REGEX.EXTRACT`) |
| `REGEXREPLACE(text, text, text, [number], [number])` | text | replace every match (or the occurrence-th, negative from the end) with $n-style replacement text. Rust regex syntax |
| `TEXTSPLIT(text OVER category, text, [text], [boolean])` | text | the parts of the text between col_delimiters (case-sensitive), in order; written without OVER, each cell splits on its own: TEXTSPLIT(Text, "," INTO Part). row_delimiter must be "" (splitting into rows is not supported yet); ignore_empty drops empty parts; empty text is #VALUE! |
| `ARRAYTOTEXT(value OVER category, [number])` | text | the values as text in item order: format 0 "1, 2, a", format 1 {1,2,"a"} |
| `ASC(text)` | text | full-width ASCII and katakana as half-width, as LibreOffice's ASC (a voiced kana becomes its letter and a sound mark) |
| `JIS(text)` | text | half-width ASCII and katakana as full-width, as LibreOffice's JIS (a letter and its sound mark join) (also `DBCS`) |
| `BAHTTEXT(number)` | text | a number as Thai baht and satang in words, rounded to the satang |
| `ROT13(text)` | text | ASCII letters rotated 13 places; anything else unchanged |
| `ENCODEURL(text)` | text | UTF-8 percent-encoding of every byte but A–Z a–z 0–9 - _ . ~ (#VALUE! for empty text) |
| `ROMAN(number, [value])` | text | 0–3999 in Roman numerals; form 0 classic (or TRUE) to 4 most concise (or FALSE) |
| `ARABIC(text)` | number | Roman numerals (0–3999) as a number; case and surrounding spaces ignored, a leading - negates, "" is 0 |

## Date and time

Dates are date-time values (UTC, millisecond precision). A number given where a date is wanted is a spreadsheet serial date (1 = 1900-01-01).

| Function | Returns | Description |
|---|---|---|
| `DATE(number, number, number)` | date | a date; out-of-range months and days roll over, years 0–1899 add 1900 |
| `DATEVALUE(text)` | date | parse a date: 2025-01-31, 2025-01-31T09:30:00Z (the time is dropped) or 1/31/2025 |
| `TIME(number, number, number)` | date | a time of day on serial day 0 (1899-12-30), wrapping at 24 hours |
| `TIMEVALUE(text)` | number | parse a time (09:30, 9:30 PM, 2025-01-31T09:30:00Z) as a fraction of a day |
| `YEAR(date)` | number | the year |
| `MONTH(date)` | number | the month, 1–12 |
| `DAY(date)` | number | the day of the month |
| `HOUR(date)` | number | the hour, 0–23 |
| `MINUTE(date)` | number | the minute |
| `SECOND(date)` | number | the second, rounded |
| `WEEKDAY(date, [number])` | number | day of the week: type 1 Sunday=1 (default), 2 Monday=1, 3 Monday=0, 11–17 day 1 is Monday…Sunday |
| `WEEKNUM(date, [number])` | number | week of the year (the week of January 1 is 1), weeks starting on type's day as in WEEKDAY (1, 2, 11–17); type 21 is the ISO week (also `WEEKNUM_EXCEL2003`) |
| `ISOWEEKNUM(date)` | number | the ISO 8601 week number |
| `ISOYEAR(date)` | number | the ISO 8601 week-numbering year |
| `DAYOFYEAR(date)` | number | the day of the year, 1–366 |
| `DAYNAME(date)` | text | the English weekday name of a date, or of a day number 1–7 (1 is Sunday) |
| `MONTHNAME(number)` | text | the English name of month 1–12 |
| `DAYS(date, date)` | number | days from start to end (end − start, times included) |
| `DAYS360(date, date, [boolean])` | number | days between on a 360-day year, US (NASD) method or European when TRUE |
| `DATEDIF(date, date, text)` | number | complete "Y" years, "M" months or "D" days between; "YM", "YD", "MD" ignore the larger units |
| `EDATE(date, number)` | date | the same day n months later (clamped to the month's end) |
| `EOMONTH(date, number)` | date | the last day of the month n months later |
| `YEARFRAC(date, date, [number])` | number | the fraction of a year between: basis 0 US 30/360 (default), 1 actual/actual, 2 actual/360, 3 actual/365, 4 European 30/360 |
| `NETWORKDAYS(date, date)` | number | working days (Monday–Friday) from start to end inclusive; no holidays argument yet (it needs an aggregation) (also `NETWORKDAYS_EXCEL2003`) |
| `NETWORKDAYS.INTL(date, date, [value])` | number | working days with a weekend of 1–7 (two days), 11–17 (one day) or a "0000011" Monday-first mask; no holidays argument yet |
| `WORKDAY(date, number)` | date | the date n working days (Monday–Friday) after start; no holidays argument yet |
| `WORKDAY.INTL(date, number, [value])` | date | WORKDAY with a weekend as in NETWORKDAYS.INTL; no holidays argument yet |
| `NOW()` | date | the current date and time (UTC); re-evaluated when the model is recalculated |
| `TODAY()` | date | the current date (UTC midnight); re-evaluated when the model is recalculated |
| `DAYSINMONTH(date)` | number | the number of days in the date's month |
| `DAYSINYEAR(date)` | number | the number of days in the date's year |
| `WEEKSINYEAR(date)` | number | the number of ISO weeks (52 or 53) in the date's year |
| `ISLEAPYEAR(date)` | boolean | whether the date's year is a leap year |
| `MONTHS(date, date, number)` | number | months between: type 0 complete months, 1 calendar months |
| `WEEKS(date, date, number)` | number | weeks between: type 0 complete weeks, 1 calendar (Monday) weeks |
| `YEARS(date, date, number)` | number | years between: type 0 complete years, 1 calendar years |
| `UNIX2DATE(number)` | date | the date of a Unix time in seconds |
| `DATE2UNIX(date)` | number | a date as Unix time in seconds |
| `DUR2DAYS(number)` | number | a duration (a number of days) in days |
| `DUR2HOURS(number)` | number | a duration in days as hours |
| `DUR2MINUTES(number)` | number | a duration in days as minutes |
| `DUR2SECONDS(number)` | number | a duration in days as seconds |
| `DUR2MILLISECONDS(number)` | number | a duration in days as milliseconds |
| `DUR2WEEKS(number)` | number | a duration in days as weeks |
| `STRIPDURATION(number)` | number | the value (durations are already a number of days) |
| `EASTERSUNDAY(number)` | date | Easter Sunday (Gregorian) of a year 1583–9956; 0–29 mean 2000–2029, 30–99 mean 1930–1999 |
| `ASCENSIONTHURSDAY(number)` | date | Ascension Thursday, 39 days after Easter Sunday |
| `ASHWEDNESDAY(number)` | date | Ash Wednesday, 46 days before Easter Sunday |
| `GOODFRIDAY(number)` | date | Good Friday, 2 days before Easter Sunday |
| `PENTECOSTSUNDAY(number)` | date | Pentecost Sunday, 49 days after Easter Sunday |
| `DATE2JULIAN(date)` | number | the Julian day number of a date |
| `HDATE_JULIAN(number, number, number)` | number | the Julian day number of Gregorian year, month, day (out-of-range days and months roll over) |
| `HDATE(number, number, number)` | text | the Hebrew date of Gregorian year, month, day, as "20 Tebet 5784" (#VALUE! for a month outside 1–12 or a day outside 1–31) |
| `HDATE_HEB(number, number, number)` | text | the Hebrew date of Gregorian year, month, day in Hebrew, as "כ׳ בְּטֵבֵת התשפ״ד" |
| `HDATE_DAY(number, number, number)` | number | the day of the Hebrew month of Gregorian year, month, day |
| `HDATE_MONTH(number, number, number)` | number | the Hebrew month of Gregorian year, month, day: 0 Tishri … 11 Elul, 12 Adar I, 13 Adar II |
| `HDATE_YEAR(number, number, number)` | number | the Hebrew year of Gregorian year, month, day |
| `DATE2HDATE(date)` | text | the Hebrew date of a date, as "20 Tebet 5784" |
| `DATE2HDATE_HEB(date)` | text | the Hebrew date of a date in Hebrew |

## Dimensional

Functions of a cell's position in a category, and the ordered (previous/next, running) forms.

| Function | Returns | Description |
|---|---|---|
| `POSITION(category)` | number | 1-based position of this cell's item along the category |
| `@ITEM(category)` | text | the name of this cell's item along the category |
| `@ID(category)` | text | the item name (Improv item names are unique within a category) |
| `@ITEMCOUNT(category)` | number | the number of items in the category |
| `PREV(value, category, [number], [value])` | the branches' type | the value at the n-th previous item along the category (n = 1); off the end, the default, else no cell |
| `NEXT(value, category, [number], [value])` | the branches' type | the value at the n-th next item along the category (n = 1); off the end, the default, else no cell |
| `SEQUENCE(value, [value], [value])` | the branches' type | SEQUENCE(T, [start = 1], [step = 1]): start, start + step, … along T's items, (POSITION(T) - 1) * step + start |
| `SORT(value OVER category, [number])` | the branches' type | the values in ascending order (descending when order is -1) |
| `UNIQUE(value OVER category, [boolean])` | the branches' type | each distinct value once, in item order (only values that occur once when exactly_once is TRUE) |
| `FILTER(value OVER category, boolean OVER category, [value])` | the branches' type | the values of x whose include is TRUE, in item order; if none, if_empty (else #N/A: Excel's #CALC!) |
| `SORTBY(value OVER category, value OVER category, [value], …)` | the branches' type | SORTBY(x, by1 OVER T, [order1], [by2, order2], …): the values of x sorted by by1, ties by by2, … (stable; a key descends when its order is -1) |
| `TAKE(value OVER category, number)` | the branches' type | the first n values in item order (the last -n when n is negative) |
| `DROP(value OVER category, number)` | the branches' type | the values TAKE leaves out: all but the first n (the last -n when n is negative), in item order |
| `SELECTS(value OVER category, value OVER category, value)` | the branches' type | Quantrix: the values whose test meets the condition (a criterion like ">5" or "a*", as in SUMIF), in item order |
| `XLOOKUP(value, value OVER category, value OVER category, [value], [number], [number])` | the values' type | values at the item where keys matches key (match_mode 0 exact, -1 next smaller, 1 next larger, 2 wildcard; search_mode 1 first to last, -1 last to first, ±2 the same as ±1); if_not_found (else #N/A) when none does |
| `XMATCH(value, value OVER category, [number], [number])` | number | the 1-based item position along the category where keys matches key (XLOOKUP's modes); #N/A when none does |
| `MATCH(value, value OVER category, [number])` | number | the 1-based item position of key: type 1 (default) the largest key ≤ it, 0 the first equal (text with * and ? wildcards), -1 the smallest ≥ it; #N/A when none |
| `LOOKUP(value, value OVER category, value OVER category)` | the values' type | values at the largest key ≤ key (keys ascending); #N/A when none |
| `INDEX(value OVER category, number)` | the branches' type | the value at the n-th item (1-based position); #REF! out of range, #N/A where that item has no cell (also `VALUEAT`) |
| `SELECT(value OVER category, value OVER category, value)` | the branches' type | values at the first item (in item order) whose key equals lookup; #N/A when none (SELECTLIST: a lookup over its own category gives one result per lookup) (also `SELECTLIST`) |
| `SELECTBETWEEN(value OVER category, value OVER category, value, value, [boolean])` | the branches' type | values at the first item whose key is between low and high (strictly when inclusive is FALSE); #N/A when none |
| `SELECTGREATERTHAN(value OVER category, value OVER category, value, [boolean])` | the branches' type | values at the first item whose key is > lookup (≥ when inclusive is TRUE); #N/A when none |
| `SELECTLESSTHAN(value OVER category, value OVER category, value, [boolean])` | the branches' type | values at the first item whose key is < lookup (≤ when inclusive is TRUE); #N/A when none |

## Statistical

| Function | Returns | Description |
|---|---|---|
| `MODE.MULT(Measure OVER Category)` | number | every most frequent value, in order of first occurrence (#N/A if none repeats) |
| `LINEST(number OVER category, number OVER category, [boolean], [boolean], …)` | number | LINEST(y, x1, [x2, …] OVER T, [const], [stats]): the least-squares fit y = m1·x1 + … + b: mk, …, m1, b (const = FALSE: b = 0); with stats, Excel's 5×(k+1) block row by row (one x: m, b, se_m, se_b, r², se_y, F, df, ss_reg, ss_resid); INTO R, C lays it out as the block |
| `LOGEST(number OVER category, number OVER category, [boolean], [boolean], …)` | number | LOGEST(y, x1, [x2, …] OVER T, [const], [stats]): y = b·m1^x1·…, fitted to ln y: mk, …, m1, b (const = FALSE: b = 1); with stats LINEST's block, for ln y |
| `FREQUENCY(number OVER category, number, …)` | number | counts of x in the bins given as numbers b1, b2, …: ≤ b1, (b1, b2], …, > bn (n + 1 counts; Excel takes the bins as a range) |
| `BETA.DIST(number, number, number, boolean, [number], [number])` | number | the beta distribution of x on [A, B] (default [0, 1]): density, or cumulative |
| `BETADIST(number, number, number, [number], [number])` | number | the cumulative beta distribution of x on [A, B] (default [0, 1]) |
| `BETA.INV(number, number, number, [number], [number])` | number | the beta quantile on [A, B] (default [0, 1]) (also `BETAINV`) |
| `BINOM.DIST(number, number, number, boolean)` | number | the binomial probability of k successes in n trials: P(K = k), or P(K ≤ k) (also `BINOMDIST`) |
| `BINOM.INV(number, number, number)` | number | the smallest k with binomial P(K ≤ k) ≥ alpha, for n trials of probability p (also `CRITBINOM`) |
| `BINOM.DIST.RANGE(number, number, number, [number])` | number | the binomial P(k1 ≤ K ≤ k2) for n trials of probability p (k2 defaults to k1) (also `B`) |
| `CHISQ.DIST(number, number, boolean)` | number | the chi-square distribution: density, or left tail (also `CHISQDIST`) |
| `CHISQ.DIST.RT(number, number)` | number | the chi-square right tail (also `CHIDIST`) |
| `CHISQ.INV(number, number)` | number | the chi-square left-tail quantile (also `CHISQINV`) |
| `CHISQ.INV.RT(number, number)` | number | the chi-square right-tail quantile (also `CHIINV`) |
| `EXPON.DIST(number, number, boolean)` | number | the exponential distribution of rate λ: density, or cumulative (also `EXPONDIST`) |
| `F.DIST(number, number, number, boolean)` | number | the F distribution: density, or left tail |
| `F.DIST.RT(number, number, number)` | number | the F right tail (also `FDIST`) |
| `F.INV(number, number, number)` | number | the F left-tail quantile |
| `F.INV.RT(number, number, number)` | number | the F right-tail quantile (also `FINV`) |
| `GAMMA.DIST(number, number, number, boolean)` | number | the gamma distribution of shape α and scale β: density, or cumulative (also `GAMMADIST`) |
| `GAMMA.INV(number, number, number)` | number | the gamma quantile (also `GAMMAINV`) |
| `GAUSS(number)` | number | Φ(z) − 0.5: the standard normal probability between 0 and z |
| `HYPGEOM.DIST(number, number, number, number, boolean)` | number | the hypergeometric probability of k successes in a sample of n from N holding K: P(X = k), or P(X ≤ k) |
| `HYPGEOMDIST(number, number, number, number)` | number | the hypergeometric probability P(X = k) |
| `LOGNORM.DIST(number, number, number, boolean)` | number | the log-normal distribution: density, or cumulative |
| `LOGNORMDIST(number, number, number)` | number | the cumulative log-normal distribution |
| `LOGNORM.INV(number, number, number)` | number | the log-normal quantile (also `LOGINV`) |
| `NEGBINOM.DIST(number, number, number, boolean)` | number | the negative binomial probability of f failures before the s-th success: P(F = f), or P(F ≤ f) |
| `NEGBINOMDIST(number, number, number)` | number | the negative binomial probability P(F = f) |
| `NORM.DIST(number, number, number, boolean)` | number | the normal distribution: density, or cumulative (also `NORMDIST`) |
| `NORM.INV(number, number, number)` | number | the normal quantile (also `NORMINV`) |
| `NORM.S.DIST(number, boolean)` | number | the standard normal distribution: density, or cumulative |
| `NORMSDIST(number)` | number | the cumulative standard normal distribution Φ(z) |
| `NORM.S.INV(number)` | number | the standard normal quantile (also `NORMSINV`) |
| `PHI(number)` | number | the standard normal density |
| `POISSON.DIST(number, number, boolean)` | number | the Poisson probability of k events at mean λ: P(X = k), or P(X ≤ k) (also `POISSON`) |
| `T.DIST(number, number, boolean)` | number | Student's t distribution: density, or left tail |
| `T.DIST.RT(number, number)` | number | Student's t right tail |
| `T.DIST.2T(number, number)` | number | Student's t two-tailed probability |
| `TDIST(number, number, number)` | number | Student's t right tail (tails = 1) or two-tailed probability (tails = 2), for x ≥ 0 |
| `T.INV(number, number)` | number | Student's t left-tail quantile |
| `T.INV.2T(number, number)` | number | Student's t two-tailed quantile (also `TINV`) |
| `WEIBULL.DIST(number, number, number, boolean)` | number | the Weibull distribution of shape α and scale β: density, or cumulative (also `WEIBULL`) |
| `FISHER(number)` | number | the Fisher transformation atanh(x) |
| `FISHERINV(number)` | number | the inverse Fisher transformation tanh(y) |
| `CONFIDENCE.NORM(number, number, number)` | number | the half-width of a normal confidence interval at significance alpha, for sd and sample size n (also `CONFIDENCE`) |
| `CONFIDENCE.T(number, number, number)` | number | the half-width of a Student's t confidence interval at significance alpha, for sd and sample size n |
| `STANDARDIZE(number, number, number)` | number | the z-score (x − mean) / sd |
| `AVERAGEA(Measure OVER Category)` | number | the mean, counting TRUE as 1 and text and FALSE as 0 |
| `AVERAGEIF(value OVER category, value, [number OVER category])` | number | the mean of avg_x (else x) where x meets the criterion |
| `AVERAGEIFS(number OVER category, value OVER category, value, …)` | number | the mean of x where every criteria measure meets its criterion |
| `AVEDEV(Measure OVER Category)` | number | the mean absolute deviation from the mean |
| `COUNT(Measure OVER Category)` | number | the number of numbers (and dates) (also `DCOUNT`) |
| `COUNTA(Measure OVER Category)` | number | the number of cells (errors included) (also `DCOUNTA`) |
| `COUNTBLANK(Measure OVER Category)` | number | the number of items with no cell (or empty text) |
| `COUNTIF(value OVER category, value)` | number | the number of cells meeting the criterion (">5", "<>x", "a*", 3) |
| `COUNTIFS(value OVER category, value, …)` | number | the number of rows where every criteria measure meets its criterion |
| `SUMIF(value OVER category, value, [number OVER category])` | number | the sum of sum_x (else x) where x meets the criterion |
| `SUMIFS(number OVER category, value OVER category, value, …)` | number | the sum of x where every criteria measure meets its criterion |
| `MAXA(Measure OVER Category)` | number | the largest value, counting TRUE as 1 and text and FALSE as 0 |
| `MAXIFS(number OVER category, value OVER category, value, …)` | number | the largest x where every criteria measure meets its criterion (0 if none) |
| `MINA(Measure OVER Category)` | number | the smallest value, counting TRUE as 1 and text and FALSE as 0 |
| `MINIFS(number OVER category, value OVER category, value, …)` | number | the smallest x where every criteria measure meets its criterion (0 if none) |
| `MEDIAN(Measure OVER Category)` | number | the median |
| `MODE(Measure OVER Category)` | number | the most frequent value (the first in item order on a tie; #N/A if none repeats) (also `MODE.SNGL`) |
| `GEOMEAN(Measure OVER Category)` | number | the geometric mean (#NUM! for a value ≤ 0) |
| `HARMEAN(Measure OVER Category)` | number | the harmonic mean (#NUM! for a value ≤ 0) |
| `TRIMMEAN(number OVER category, number)` | number | the mean without the fraction of values at the tails (rounded down to an even count) |
| `DEVSQ(Measure OVER Category)` | number | Σ(x − mean)² |
| `STDEV(Measure OVER Category)` | number | the sample standard deviation (also `STDEV.S`, `DSTDEV`) |
| `STDEVP(Measure OVER Category)` | number | the population standard deviation (also `STDEV.P`, `DSTDEVP`) |
| `STDEVA(Measure OVER Category)` | number | the sample standard deviation, counting TRUE as 1 and text and FALSE as 0 |
| `STDEVPA(Measure OVER Category)` | number | the population standard deviation, counting TRUE as 1 and text and FALSE as 0 |
| `VAR(Measure OVER Category)` | number | the sample variance (also `VAR.S`, `DVAR`) |
| `VARP(Measure OVER Category)` | number | the population variance (also `VAR.P`, `DVARP`) |
| `VARA(Measure OVER Category)` | number | the sample variance, counting TRUE as 1 and text and FALSE as 0 |
| `VARPA(Measure OVER Category)` | number | the population variance, counting TRUE as 1 and text and FALSE as 0 |
| `KURT(Measure OVER Category)` | number | the sample excess kurtosis |
| `KURTP(Measure OVER Category)` | number | the population excess kurtosis |
| `SKEW(Measure OVER Category)` | number | the sample skewness |
| `SKEWP(Measure OVER Category)` | number | the population skewness (also `SKEW.P`) |
| `LARGE(number OVER category, number)` | number | the k-th largest value |
| `SMALL(number OVER category, number)` | number | the k-th smallest value |
| `PERCENTILE(number OVER category, number)` | number | the p-th percentile, interpolated, 0 ≤ p ≤ 1 (also `PERCENTILE.INC`) |
| `PERCENTILE.EXC(number OVER category, number)` | number | the p-th percentile, exclusive of 0 and 1 |
| `QUARTILE(number OVER category, number)` | number | the q-th quartile (0 min … 4 max) (also `QUARTILE.INC`) |
| `QUARTILE.EXC(number OVER category, number)` | number | the q-th quartile (1 … 3), exclusive |
| `PROB(number OVER category, number OVER category, number, [number])` | number | Σ p over the values x in [low, high] (x = low without high); #NUM! unless every 0 < p ≤ 1 and Σ p = 1 |
| `SUBTOTAL(number, number OVER category)` | number | the reduction a code picks: 1 AVERAGE, 2 COUNT, 3 COUNTA, 4 MAX, 5 MIN, 6 PRODUCT, 7 STDEV, 8 STDEVP, 9 SUM, 10 VAR, 11 VARP (101–111 the same) |
| `AGGREGATE(number, number, number OVER category, [number])` | number | SUBTOTAL's codes and 12 MEDIAN, 13 MODE, 14 LARGE, 15 SMALL, 16 PERCENTILE, 17 QUARTILE, 18 PERCENTILE.EXC, 19 QUARTILE.EXC; options 2, 3, 6, 7 skip errors |
| `RANK(number OVER category, [number])` | number | each item's rank among the values (order 0, the default, ranks the largest 1) (also `RANK.EQ`) |
| `RANK.AVG(number OVER category, [number])` | number | each item's rank, ties sharing their mean rank |
| `PERCENTRANK(number OVER category, [number], [number])` | number | each item's (or v's) rank as a fraction 0 … 1, to digits (3) significant digits (also `PERCENTRANK.INC`) |
| `PERCENTRANK.EXC(number OVER category, [number], [number])` | number | each item's (or v's) rank as a fraction, exclusive of 0 and 1 |
| `TREND(number OVER category, number OVER category, [boolean])` | number | each item's value on the least-squares line of y on x, at its own x (through 0 if const is FALSE) |
| `GROWTH(number OVER category, number OVER category, [boolean])` | number | each item's value on the least-squares curve y = b·mˣ (fitting ln y; b = 1 if const is FALSE) |
| `CORREL(number OVER category, number OVER category)` | number | the Pearson correlation of x and y (also `PEARSON`) |
| `RSQ(number OVER category, number OVER category)` | number | the square of the Pearson correlation |
| `COVAR(number OVER category, number OVER category)` | number | the population covariance (also `COVARIANCE.P`) |
| `COVARIANCE.S(number OVER category, number OVER category)` | number | the sample covariance |
| `SLOPE(number OVER category, number OVER category)` | number | the least-squares slope of y on x |
| `INTERCEPT(number OVER category, number OVER category)` | number | the least-squares intercept of y on x |
| `STEYX(number OVER category, number OVER category)` | number | the standard error of the predicted y |
| `FORECAST(number, number OVER category, number OVER category)` | number | the least-squares line of y on x at x0 (also `FORECAST.LINEAR`) |
| `EXPA(number OVER category, number OVER category, [boolean])` | number | A of the fit y = A·e^(Bx): least squares of ln y weighted by y (simple = TRUE: unweighted, as LOGEST) |
| `EXPB(number OVER category, number OVER category, [boolean])` | number | B of the fit y = A·e^(Bx) |
| `EXPFORECAST(number, number OVER category, number OVER category, [boolean])` | number | the fit y = A·e^(Bx) at x0 |
| `CHITEST(number OVER category, number OVER category)` | number | the χ² test p-value of actual against expected (also `CHISQ.TEST`) |
| `FTEST(number OVER category, number OVER category)` | number | the two-tailed F-test p-value that a and b have the same variance (also `F.TEST`) |
| `TTEST(number OVER category, number OVER category, number, number)` | number | the t-test p-value (tails 1 or 2; type 1 paired, 2 equal variance, 3 unequal) (also `T.TEST`) |
| `ZTEST(number OVER category, number, [number])` | number | the one-tailed z-test p-value of the mean against μ0 (σ defaults to the sample's) (also `Z.TEST`) |
| `AUTOCORREL(Measure OVER Category)` | number | the lag-1 autocorrelation in item order (GSL) (also `GSL_STATS_LAG1_AUTOCORRELATION`) |
| `DGET(Measure OVER Category)` | the branches' type | the single value (with WHERE, the single match); #NUM! when more than one (a WHERE that keeps none leaves no cell) |

## Financial

Rates are per period; cash flows are in item order along their `OVER` category.

| Function | Returns | Description |
|---|---|---|
| `FV(number, number, number, [number], [number])` | number | future value: FV(rate, nper, pmt, [pv], [type]) |
| `PV(number, number, number, [number], [number])` | number | present value: PV(rate, nper, pmt, [fv], [type]) |
| `PMT(number, number, number, [number], [number])` | number | payment per period: PMT(rate, nper, pv, [fv], [type]) |
| `IPMT(number, number, number, number, [number], [number])` | number | interest part of payment per: IPMT(rate, per, nper, pv, [fv], [type]) |
| `PPMT(number, number, number, number, [number], [number])` | number | principal part of payment per: PPMT(rate, per, nper, pv, [fv], [type]) |
| `NPER(number, number, number, [number], [number])` | number | number of periods: NPER(rate, pmt, pv, [fv], [type]) |
| `RATE(number, number, number, [number], [number], [number])` | number | rate per period (iterative): RATE(nper, pmt, pv, [fv], [type], [guess]) |
| `CUMIPMT(number, number, number, number, number, number)` | number | interest paid from period start to end: CUMIPMT(rate, nper, pv, start, end, type) (also `CUMIPMT_ADD`) |
| `CUMPRINC(number, number, number, number, number, number)` | number | principal paid from period start to end: CUMPRINC(rate, nper, pv, start, end, type) (also `CUMPRINC_ADD`) |
| `ISPMT(number, number, number, number)` | number | interest of period per with straight-line principal: ISPMT(rate, per, nper, pv) |
| `EFFECT(number, number)` | number | effective annual rate of a nominal rate compounded npery times (also `EFFECT_ADD`) |
| `NOMINAL(number, number)` | number | nominal annual rate of an effective rate compounded npery times (also `NOMINAL_ADD`) |
| `PDURATION(number, number, number)` | number | periods for pv to grow to fv at rate: PDURATION(rate, pv, fv) (also `G_DURATION`) |
| `RRI(number, number, number)` | number | equivalent growth rate per period: RRI(nper, pv, fv) |
| `DOLLARDE(number, number)` | number | a fractional dollar price (1.02 with fraction 16 is 1 and 2/16) as a decimal |
| `DOLLARFR(number, number)` | number | a decimal dollar price as a fraction (in the integer part's units) |
| `SLN(number, number, number)` | number | straight-line depreciation per period: SLN(cost, salvage, life) |
| `SYD(number, number, number, number)` | number | sum-of-years'-digits depreciation: SYD(cost, salvage, life, per) |
| `DB(number, number, number, number, [number])` | number | fixed declining balance: DB(cost, salvage, life, period, [month=12]) |
| `DDB(number, number, number, number, [number])` | number | double declining balance: DDB(cost, salvage, life, period, [factor=2]) |
| `VDB(number, number, number, number, number, [number], [boolean])` | number | declining balance between two periods: VDB(cost, salvage, life, start, end, [factor=2], [no_switch]) |
| `AMORDEGRC(number, date, date, number, number, number, [number])` | number | French degressive depreciation: AMORDEGRC(cost, purchased, first, salvage, period, rate, [basis]) |
| `AMORLINC(number, date, date, number, number, number, [number])` | number | French linear depreciation: AMORLINC(cost, purchased, first, salvage, period, rate, [basis]) |
| `ACCRINT(date, date, date, number, number, number, [number], [boolean])` | number | accrued interest of a periodic-coupon security: ACCRINT(issue, first, settle, rate, par, freq, [basis], [calc_method]) |
| `ACCRINTM(date, date, number, [number], [number])` | number | accrued interest paid at maturity: ACCRINTM(issue, settle, rate, [par=1000], [basis]) |
| `COUPDAYBS(date, date, number, [number])` | number | days from the coupon period's start to settlement |
| `COUPDAYS(date, date, number, [number])` | number | days in the coupon period holding settlement |
| `COUPDAYSNC(date, date, number, [number])` | number | days from settlement to the next coupon |
| `COUPNCD(date, date, number, [number])` | date | the next coupon date after settlement |
| `COUPNUM(date, date, number, [number])` | number | coupons payable between settlement and maturity |
| `COUPPCD(date, date, number, [number])` | date | the coupon date on or before settlement |
| `DISC(date, date, number, number, [number])` | number | discount rate: DISC(settle, maturity, price, redemption, [basis]) |
| `DURATION(date, date, number, number, number, [number])` | number | Macaulay duration: DURATION(settle, maturity, coupon, yld, freq, [basis]) (also `BONDDURATION`) |
| `MDURATION(date, date, number, number, number, [number])` | number | modified duration: MDURATION(settle, maturity, coupon, yld, freq, [basis]) (also `BONDMDURATION`) |
| `INTRATE(date, date, number, number, [number])` | number | interest rate of a fully invested security: INTRATE(settle, maturity, investment, redemption, [basis]) |
| `ODDFPRICE(date, date, date, date, number, number, number, number, [number])` | number | price with an odd first period: ODDFPRICE(settle, maturity, issue, first, rate, yld, redemption, freq, [basis]) |
| `ODDFYIELD(date, date, date, date, number, number, number, number, [number])` | number | yield with an odd first period: ODDFYIELD(settle, maturity, issue, first, rate, price, redemption, freq, [basis]) |
| `ODDLPRICE(date, date, date, number, number, number, number, [number])` | number | price with an odd last period: ODDLPRICE(settle, maturity, last, rate, yld, redemption, freq, [basis]) |
| `ODDLYIELD(date, date, date, number, number, number, number, [number])` | number | yield with an odd last period: ODDLYIELD(settle, maturity, last, rate, price, redemption, freq, [basis]) |
| `PRICE(date, date, number, number, number, number, [number])` | number | price per 100 face value: PRICE(settle, maturity, rate, yld, redemption, freq, [basis]) |
| `PRICEDISC(date, date, number, number, [number])` | number | price of a discounted security: PRICEDISC(settle, maturity, discount, redemption, [basis]) |
| `PRICEMAT(date, date, date, number, number, [number])` | number | price of a security paying interest at maturity: PRICEMAT(settle, maturity, issue, rate, yld, [basis]) |
| `RECEIVED(date, date, number, number, [number])` | number | amount at maturity of a fully invested security: RECEIVED(settle, maturity, investment, discount, [basis]) |
| `TBILLEQ(date, date, number)` | number | bond-equivalent yield of a Treasury bill: TBILLEQ(settle, maturity, discount) |
| `TBILLPRICE(date, date, number)` | number | price per 100 face of a Treasury bill: TBILLPRICE(settle, maturity, discount) |
| `TBILLYIELD(date, date, number)` | number | yield of a Treasury bill: TBILLYIELD(settle, maturity, price) |
| `YIELD(date, date, number, number, number, number, [number])` | number | yield of a periodic-coupon security (iterative): YIELD(settle, maturity, rate, price, redemption, freq, [basis]) |
| `YIELDDISC(date, date, number, number, [number])` | number | annual yield of a discounted security: YIELDDISC(settle, maturity, price, redemption, [basis]) |
| `YIELDMAT(date, date, date, number, number, [number])` | number | annual yield of a security paying interest at maturity: YIELDMAT(settle, maturity, issue, rate, price, [basis]) |
| `NPV(number, number OVER category)` | number | net present value of the flows in item order, the first discounted one period (a missing item is skipped, not zero): NPV(rate, cf OVER T) |
| `XNPV(number, number OVER category, date OVER category)` | number | net present value of flows at dates, discounted from the first date (365-day years): XNPV(rate, cf, dates OVER T) |
| `IRR(number OVER category, [number])` | number | internal rate of return of the flows in item order (iterative): IRR(cf OVER T, [guess = 0.1]) |
| `XIRR(number OVER category, date OVER category, [number])` | number | internal rate of return of flows at dates (iterative): XIRR(cf, dates OVER T, [guess = 0.1]) |
| `MIRR(number OVER category, number, number)` | number | modified internal rate of return, outflows financed and inflows reinvested: MIRR(cf OVER T, finance_rate, reinvest_rate) |
| `FMRR(number OVER category, number, number, number)` | number | financial management rate of return (GNU Oleo): outflows pulled back at the safe rate, inflows of at least reinvest_min pushed forward at the reinvestment rate: FMRR(cf OVER T, safe_rate, reinvest_rate, reinvest_min) |
| `FVSCHEDULE(number, number OVER category)` | number | principal compounded over the rates in item order: FVSCHEDULE(principal, rate OVER T) |
| `OPT_BS(text, number, number, number, number, number, [number])` | number | Black-Scholes price of a European option: OPT_BS(call_put, spot, strike, time, rate, volatility, [cost_of_carry]) |
| `OPT_BS_DELTA(text, number, number, number, number, number, [number])` | number | Black-Scholes delta: OPT_BS_DELTA(call_put, spot, strike, time, rate, volatility, [cost_of_carry]) |
| `OPT_BS_GAMMA(number, number, number, number, number, [number])` | number | Black-Scholes gamma (calls and puts alike): OPT_BS_GAMMA(spot, strike, time, rate, volatility, [cost_of_carry]) |
| `OPT_BS_RHO(text, number, number, number, number, number, [number])` | number | Black-Scholes rho: OPT_BS_RHO(call_put, spot, strike, time, rate, volatility, [cost_of_carry]) |
| `OPT_BS_THETA(text, number, number, number, number, number, [number])` | number | Black-Scholes theta: OPT_BS_THETA(call_put, spot, strike, time, rate, volatility, [cost_of_carry]) |
| `OPT_BS_VEGA(number, number, number, number, number, [number])` | number | Black-Scholes vega (calls and puts alike): OPT_BS_VEGA(spot, strike, time, rate, volatility, [cost_of_carry]) |
| `OPT_BS_CARRYCOST(text, number, number, number, number, number, [number])` | number | Black-Scholes sensitivity to the cost of carry: OPT_BS_CARRYCOST(call_put, spot, strike, time, rate, volatility, [cost_of_carry]) |
| `OPT_GARMAN_KOHLHAGEN(text, number, number, number, number, number, number)` | number | currency option: OPT_GARMAN_KOHLHAGEN(call_put, spot, strike, time, domestic_rate, foreign_rate, volatility) |
| `OPT_FRENCH(text, number, number, number, number, number, number, number)` | number | Black-Scholes with trading-day volatility: OPT_FRENCH(call_put, spot, strike, trading_time, calendar_time, rate, volatility, cost_of_carry) |
| `OPT_JUMP_DIFF(text, number, number, number, number, number, number, number)` | number | Merton jump diffusion: OPT_JUMP_DIFF(call_put, spot, strike, time, rate, volatility, lambda, gamma) |
| `OPT_MILTERSEN_SCHWARTZ(text, number, number, number, number, number, number, number, number, number, number, number, number, number)` | number | commodity futures option: OPT_MILTERSEN_SCHWARTZ(call_put, p_t, f_t, strike, t1, t2, v_s, v_e, v_f, rho_se, rho_sf, rho_ef, kappa_e, kappa_f) |
| `OPT_EXEC(text, number, number, number, number, number, number, number)` | number | executive stock option: OPT_EXEC(call_put, spot, strike, time, rate, volatility, cost_of_carry, lambda) |
| `OPT_FORWARD_START(text, number, number, number, number, number, number, number)` | number | forward start option: OPT_FORWARD_START(call_put, spot, alpha, time_start, time, rate, volatility, cost_of_carry) |
| `OPT_TIME_SWITCH(text, number, number, number, number, number, number, number, number, number)` | number | discrete time switch option: OPT_TIME_SWITCH(call_put, spot, strike, a, time, m, dt, rate, cost_of_carry, volatility) |
| `OPT_SIMPLE_CHOOSER(number, number, number, number, number, number, number)` | number | simple chooser option: OPT_SIMPLE_CHOOSER(spot, strike, time1, time2, rate, cost_of_carry, volatility) |
| `OPT_BAW_AMER(text, number, number, number, number, number, number)` | number | Barone-Adesi-Whaley American approximation: OPT_BAW_AMER(call_put, spot, strike, time, rate, volatility, cost_of_carry) |
| `OPT_BJER_STENS(text, number, number, number, number, number, number)` | number | Bjerksund-Stensland American approximation: OPT_BJER_STENS(call_put, spot, strike, time, rate, volatility, cost_of_carry) |
| `OPT_RGW(number, number, number, number, number, number, number)` | number | Roll-Geske-Whaley American call with one known dividend: OPT_RGW(spot, strike, time_payout, time_exp, rate, d, volatility) |
| `OPT_BINOMIAL(text, text, number, number, number, number, number, number, [number])` | number | Cox-Ross-Rubinstein binomial tree: OPT_BINOMIAL(amer_euro, call_put, steps, spot, strike, time, rate, volatility, [cost_of_carry]) |
| `OPT_COMPLEX_CHOOSER(number, number, number, number, number, number, number, number, number)` | number | complex chooser option: OPT_COMPLEX_CHOOSER(spot, strike_call, strike_put, time, time_call, time_put, rate, cost_of_carry, volatility) |
| `OPT_ON_OPTIONS(text, number, number, number, number, number, number, number, number)` | number | option on an option ("cc", "cp", "pc", "pp"): OPT_ON_OPTIONS(type, spot, strike1, strike2, time1, time2, rate, cost_of_carry, volatility) |
| `OPT_EXTENDIBLE_WRITER(text, number, number, number, number, number, number, number, number)` | number | writer-extendible option: OPT_EXTENDIBLE_WRITER(call_put, spot, strike1, strike2, time1, time2, rate, cost_of_carry, volatility) |
| `OPT_2_ASSET_CORRELATION(text, number, number, number, number, number, number, number, number, number, number, number)` | number | two-asset correlation option: OPT_2_ASSET_CORRELATION(call_put, spot1, spot2, strike1, strike2, time, cost_of_carry1, cost_of_carry2, rate, volatility1, volatility2, rho) |
| `OPT_EURO_EXCHANGE(number, number, number, number, number, number, number, number, number, number, number)` | number | European option to exchange asset 2 for asset 1: OPT_EURO_EXCHANGE(spot1, spot2, qty1, qty2, time, rate, cost_of_carry1, cost_of_carry2, volatility1, volatility2, rho) |
| `OPT_AMER_EXCHANGE(number, number, number, number, number, number, number, number, number, number, number)` | number | American option to exchange asset 2 for asset 1: OPT_AMER_EXCHANGE(spot1, spot2, qty1, qty2, time, rate, cost_of_carry1, cost_of_carry2, volatility1, volatility2, rho) |
| `OPT_SPREAD_APPROX(text, number, number, number, number, number, number, number, number)` | number | Kirk's futures spread option approximation: OPT_SPREAD_APPROX(call_put, fut_price1, fut_price2, strike, time, rate, volatility1, volatility2, rho) |
| `OPT_FIXED_STRK_LKBK(text, number, number, number, number, number, number, number, number)` | number | fixed-strike lookback: OPT_FIXED_STRK_LKBK(call_put, spot, spot_min, spot_max, strike, time, rate, cost_of_carry, volatility) |
| `OPT_FLOAT_STRK_LKBK(text, number, number, number, number, number, number, number)` | number | floating-strike lookback: OPT_FLOAT_STRK_LKBK(call_put, spot, spot_min, spot_max, time, rate, cost_of_carry, volatility) |
| `OPT_BARRIER(number, number, number, number, number, number, number, number, number, text, text, text, [text])` | number | barrier option: OPT_BARRIER(spot, vol, r, rf, T, strike, lower, upper, rebate, put_call, in_out, continuous_or_end, [greek]) |
| `OPT_TOUCH(number, number, number, number, number, number, number, text, text, text, [text])` | number | one/no-touch option: OPT_TOUCH(spot, vol, r, rf, T, lower, upper, foreign_domestic, in_out, continuous_or_end, [greek]) |
| `OPT_PROB_HIT(number, number, number, number, number, number)` | number | probability of touching a barrier: OPT_PROB_HIT(spot, vol, mu, T, lower, upper) |
| `OPT_PROB_INMONEY(number, number, number, number, number, number, [number], [text])` | number | probability of ending between the barriers (and in the money): OPT_PROB_INMONEY(spot, vol, mu, T, lower, upper, [strike], [put_call]) |

## Engineering

Complex numbers are text, `"3+4i"` (suffix `i` or `j`); base conversions use 10-digit two's complement for negatives.

| Function | Returns | Description |
|---|---|---|
| `BIN2DEC(value)` | number | binary text as a number |
| `BIN2HEX(value, [number])` | text | binary text as hexadecimal, zero-padded to `places` |
| `BIN2OCT(value, [number])` | text | binary text as octal, zero-padded to `places` |
| `DEC2BIN(number, [number])` | text | an integer in −512…511 as binary text |
| `DEC2HEX(number, [number])` | text | an integer in −2³⁹…2³⁹−1 as hexadecimal text |
| `DEC2OCT(number, [number])` | text | an integer in −2²⁹…2²⁹−1 as octal text |
| `HEX2BIN(value, [number])` | text | hexadecimal text as binary |
| `HEX2DEC(value)` | number | hexadecimal text as a number |
| `HEX2OCT(value, [number])` | text | hexadecimal text as octal |
| `OCT2BIN(value, [number])` | text | octal text as binary |
| `OCT2DEC(value)` | number | octal text as a number |
| `OCT2HEX(value, [number])` | text | octal text as hexadecimal |
| `BASE(number, number, [number])` | text | an integer in 0…2⁵³−1 as text in a radix 2…36, zero-padded to `min_length` (also `NUMTOBASE`) |
| `DECIMAL(value, number)` | number | text in a radix 2…36 as a number (also `BASETONUM`) |
| `HEXREP(number)` | text | the bytes of a number's IEEE double, little-endian, in hexadecimal |
| `BITAND(number, number)` | number | bitwise and |
| `BITOR(number, number)` | number | bitwise or |
| `BITXOR(number, number)` | number | bitwise exclusive or |
| `BITLSHIFT(number, number)` | number | shifted left by `shift` bits (right if negative), |shift| ≤ 53 |
| `BITRSHIFT(number, number)` | number | shifted right by `shift` bits (left if negative), |shift| ≤ 53 |
| `CONVERT(number, text, text)` | number | a measurement converted between units of Excel's table (SI and binary prefixes allowed); #N/A for unknown or mismatched units |
| `DELTA(number, [number])` | number | 1 if the numbers are equal (b defaults to 0), else 0 |
| `GESTEP(number, [number])` | number | 1 if x ≥ step (default 0), else 0 |
| `COMPLEX(number, number, [text])` | text | the complex number re + im·i as text, with suffix "i" (default) or "j" |
| `IMREAL(value)` | number | a complex number's real part |
| `IMAGINARY(value)` | number | a complex number's imaginary part |
| `IMABS(value)` | number | a complex number's modulus |z| |
| `IMARGUMENT(value)` | number | a complex number's angle in radians, −π…π (#NUM! at 0) |
| `IMCONJUGATE(value)` | text | the complex conjugate |
| `IMNEG(value)` | text | −z |
| `IMINV(value)` | text | 1/z |
| `IMSUM(value, …)` | text | the sum of complex numbers |
| `IMPRODUCT(value, …)` | text | the product of complex numbers |
| `IMSUB(value, value)` | text | z1 − z2 |
| `IMDIV(value, value)` | text | z1 / z2 |
| `IMPOWER(value, number)` | text | z raised to a real power |
| `IMSQRT(value)` | text | the principal square root |
| `IMEXP(value)` | text | e^z |
| `IMLN(value)` | text | the principal natural logarithm |
| `IMLOG10(value)` | text | the principal base-10 logarithm |
| `IMLOG2(value)` | text | the principal base-2 logarithm |
| `IMSIN(value)` | text | the complex sine |
| `IMCOS(value)` | text | the complex cosine |
| `IMTAN(value)` | text | the complex tangent |
| `IMSEC(value)` | text | the complex secant |
| `IMCSC(value)` | text | the complex cosecant |
| `IMCOT(value)` | text | the complex cotangent |
| `IMSINH(value)` | text | the complex hyperbolic sine |
| `IMCOSH(value)` | text | the complex hyperbolic cosine |
| `IMTANH(value)` | text | the complex hyperbolic tangent |
| `IMSECH(value)` | text | the complex hyperbolic secant |
| `IMCSCH(value)` | text | the complex hyperbolic cosecant |
| `IMCOTH(value)` | text | the complex hyperbolic cotangent |
| `IMARCSIN(value)` | text | the complex arcsine (principal branch) |
| `IMARCCOS(value)` | text | the complex arccosine |
| `IMARCTAN(value)` | text | the complex arctangent |
| `IMARCSEC(value)` | text | the complex arcsecant |
| `IMARCCSC(value)` | text | the complex arccosecant |
| `IMARCCOT(value)` | text | the complex arccotangent |
| `IMARCSINH(value)` | text | the complex inverse hyperbolic sine |
| `IMARCCOSH(value)` | text | the complex inverse hyperbolic cosine |
| `IMARCTANH(value)` | text | the complex inverse hyperbolic tangent |
| `IMARCSECH(value)` | text | the complex inverse hyperbolic secant |
| `IMARCCSCH(value)` | text | the complex inverse hyperbolic cosecant |
| `IMARCCOTH(value)` | text | the complex inverse hyperbolic cotangent |
| `IMFACT(value)` | text | the complex factorial z! = Γ(z + 1) |
| `IMGAMMA(value)` | text | the complex gamma function Γ(z) |
| `BESSELI(number, number)` | number | the modified Bessel function Iₙ(x) |
| `BESSELJ(number, number)` | number | the Bessel function Jₙ(x) |
| `BESSELK(number, number)` | number | the modified Bessel function Kₙ(x), x > 0 |
| `BESSELY(number, number)` | number | the Bessel function Yₙ(x), x > 0 |
| `IMIGAMMA(value, value, [boolean], [boolean])` | text | the complex incomplete gamma function of a and z: lower (default) or upper, regularized (default) or not |
