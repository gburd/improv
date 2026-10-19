# Function Reference

<!-- Generated from crates/core_model/src/functions.rs by
     `UPDATE_DOCS=1 cargo test -p improv_core_model`. Do not edit. -->

Every built-in function, by category. Names are case-insensitive;
`[x]` is optional (`[x=1]`: 1 if omitted), `…` repeats the
arguments before it, `x OVER T` is reduced over a category and
`INTO K` lays a list result along one. Positions are
1-based. An error argument (`#DIV/0!`, `#N/A`, …) makes the result
that error, except in the functions that test for errors. See
[The Formula Language](./formulas.md) for operators, types and
dimensions.

## Aggregations

Written `F(Measure OVER Category)`: collapse one category. Each also has a scalar form over its arguments, listed under its category.

| Function | Returns | Description |
|---|---|---|
| `SUM(values OVER T)` | number | the sum over a category (also `DSUM`) |
| `AVERAGE(values OVER T)` | number | the mean over a category (also `AVG`, `DAVERAGE`) |
| `MIN(values OVER T)` | number | the smallest value over a category (also `DMIN`) |
| `MAX(values OVER T)` | number | the largest value over a category (also `DMAX`) |
| `PRODUCT(values OVER T)` | number | the product over a category (also `DPRODUCT`) |
| `SUMSQ(values OVER T)` | number | the sum of squares over a category |
| `AND(values OVER T)` | boolean | whether every value over a category is true |
| `OR(values OVER T)` | boolean | whether any value over a category is true |
| `XOR(values OVER T)` | boolean | whether an odd number of values over a category are true |
| `FIRST(values OVER T)` | the branches' type | the value at the first item (in category order) that has one |
| `LAST(values OVER T)` | the branches' type | the value at the last item that has one |
| `S(values OVER T)` | text | the first value, as text (Quantrix) |
| `TEXTJOIN(delimiter, ignore_empty, values OVER T)` | text | the values in item order joined by the delimiter (skipping empty text when ignore_empty is TRUE) |

## Logical

`IF` and the functions built on it evaluate only the branch they choose, so an error or missing cell in the other branch does not matter.

| Function | Returns | Description |
|---|---|---|
| `AND(logical1, …)` | boolean | whether every argument is true |
| `OR(logical1, …)` | boolean | whether any argument is true |
| `XOR(logical1, …)` | boolean | whether an odd number of arguments are true |
| `IF(logical_test, value_if_true, [value_if_false])` | the branches' type | then where the test is true, else (or no cell) where it is false |
| `IFS(logical_test1, value_if_true1, …)` | the branches' type | the value paired with the first true test |
| `SWITCH(expression, value1, result1, …, [default])` | the branches' type | the result paired with the first value equal to the expression, else the default |
| `CHOOSE(index_num, value1, …)` | the branches' type | the index_num-th value (1-based) |
| `IFERROR(value, value_if_error)` | the branches' type | the value, or the alternative where it is an error |
| `IFNA(value, value_if_na)` | the branches' type | the value, or the alternative where it is #N/A |
| `CLEARERROR(value, [alternative])` | the branches' type | the value, or the alternative (or no cell) where it is an error |
| `CLEARZERO(value, [alternative])` | number | the value, or the alternative (or no cell) where it is zero |
| `COALESCE(value1, …, default)` | the branches' type | a cell wherever any argument that reads a measure has one: the first argument with a cell, else the last (the default) |
| `BETWEEN(x, low, high, [inclusive])` | boolean | whether low ≤ x ≤ high (strict when inclusive is FALSE) |
| `TRUE()` | boolean | TRUE |
| `FALSE()` | boolean | FALSE |
| `SCAN(initial_value, array OVER T, lambda)` | the branches' type | SCAN(initial_value, array OVER T, LAMBDA(acc, v, body)): the running fold along T, one value per item |
| `REDUCE(initial_value, array OVER T, lambda)` | the branches' type | REDUCE(initial_value, array OVER T, LAMBDA(acc, v, body)): the fold's last value |
| `LET(name, value, …, body)` | the branches' type | LET(name, value, …, body): the body with each name bound to its value |
| `LAMBDA(p, …, body)` | the branches' type | LAMBDA(p, …, body): a function of its parameters, called on the spot or passed to MAP, REDUCE or SCAN |
| `MAKEARRAY(rows, cols, lambda)` | the branches' type | MAKEARRAY(rows, cols, LAMBDA(r, c, body)): the body over the categories rows × cols, with r and c the item positions |
| `BYROW(array OVER C, lambda)` | the branches' type | BYROW(array OVER C, LAMBDA(row, body)): the body per item of array's other categories, row being the values along C (the body reduces it: SUM(row), …) (also `BYCOL`) |
| `MAP(array1, …, lambda)` | the branches' type | MAP(array1, …, LAMBDA(p, …, body)): the body applied element by element |

## Information and errors

| Function | Returns | Description |
|---|---|---|
| `ISERROR(value)` | boolean | whether the value is any error |
| `ISERR(value)` | boolean | whether the value is an error other than #N/A |
| `ISNA(value)` | boolean | whether the value is #N/A |
| `NA()` | number | the #N/A error |
| `ERROR(name)` | number | the error a code names ("#DIV/0!") |
| `ERROR.TYPE(error_val)` | number | 1 #NULL!, 2 #DIV/0!, 3 #VALUE!, 4 #REF!, 5 #NAME?, 6 #NUM!, 7 #N/A; #N/A for a non-error |
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
| `SUM(number1, …)` | number | the sum of the arguments |
| `AVERAGE(number1, …)` | number | the mean of the arguments |
| `MIN(number1, …)` | number | the smallest argument (also `MIN2`) |
| `MAX(number1, …)` | number | the largest argument (also `MAX2`) |
| `PRODUCT(number1, …)` | number | the product of the arguments |
| `SUMSQ(number1, …)` | number | the sum of the squares of the arguments |
| `ABS(number)` | number | absolute value |
| `NEG(number)` | number | negation |
| `SIGN(number)` | number | −1, 0 or 1 |
| `SQRT(number)` | number | square root (#NUM! below zero) |
| `SQRTPI(number)` | number | √(x·π) |
| `EXP(number)` | number | e to the power x |
| `EXPM1(x)` | number | eˣ − 1, accurate near 0 |
| `LN(number)` | number | natural logarithm |
| `LN1P(x)` | number | ln(1 + x), accurate near 0 |
| `LOG(number, [base=10])` | number | logarithm to a base (default 10) |
| `LOG10(number)` | number | base-10 logarithm |
| `LOG2(x)` | number | base-2 logarithm |
| `POWER(number, power)` | number | number raised to power (same as ^) |
| `MOD(number, divisor)` | number | remainder, with the divisor's sign |
| `QUOTIENT(numerator, denominator)` | number | integer part of a quotient |
| `PI()` | number | π |
| `DEGREES(angle)` | number | radians to degrees |
| `RADIANS(angle)` | number | degrees to radians |
| `SIN(number)` | number | sine (radians) |
| `COS(number)` | number | cosine |
| `TAN(number)` | number | tangent |
| `ASIN(number)` | number | arcsine |
| `ACOS(number)` | number | arccosine |
| `ATAN(number)` | number | arctangent |
| `ATAN2(x_num, y_num)` | number | angle of the point (x, y) |
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
| `SINPI(x)` | number | sin(πx), exact at integers |
| `COSPI(x)` | number | cos(πx), exact at integers |
| `TANPI(x)` | number | tan(πx), exact at integers |
| `COTPI(x)` | number | cot(πx), exact at integers |
| `HYPOT(number1, …)` | number | √(Σ xᵢ²) without overflow |
| `FACT(number)` | number | factorial |
| `FACTDOUBLE(number)` | number | double factorial n!! |
| `COMBIN(number, number_chosen)` | number | combinations of number items taken number_chosen at a time |
| `COMBINA(number, number_chosen)` | number | combinations with repetition |
| `PERMUT(number, number_chosen)` | number | permutations of number items taken number_chosen at a time |
| `PERMUTATIONA(number, number_chosen)` | number | permutations with repetition |
| `GCD(number1, …)` | number | greatest common divisor |
| `LCM(number1, …)` | number | least common multiple |
| `MULTINOMIAL(number1, …)` | number | (Σ n)! / Π n! |
| `RAWSUBTRACT(number1, …)` | number | the first argument minus the rest |
| `INVSUMINV(number1, …)` | number | 1 / Σ(1/xᵢ) |
| `GAMMA(number)` | number | the gamma function Γ(x) |
| `GAMMALN(x)` | number | ln Γ(x), for x > 0 (also `GAMMALN.PRECISE`) |
| `BETA(x, y)` | number | the Euler beta function B(a, b) |
| `BETALN(x, y)` | number | ln |B(a, b)| |
| `DIGAMMA(x)` | number | the digamma function ψ(x) |
| `IGAMMA(a, x, [lower=TRUE], [regularize=TRUE], [real=TRUE])` | number | the incomplete gamma function of a and x: lower (default) or upper, regularized (default) or not; the imaginary part (real = FALSE) is 0 |
| `POCHHAMMER(x, n)` | number | the rising factorial Γ(x + n) / Γ(x) |
| `ERF(lower_limit, [upper_limit])` | number | the error function erf(x), or erf(upper) − erf(lower) |
| `ERF.PRECISE(x)` | number | the error function erf(x) |
| `ERFC(x)` | number | the complementary error function 1 − erf(x) (also `ERFC.PRECISE`) |
| `SUMA(values OVER T)` | number | the sum, counting TRUE as 1 and text as 0 |
| `SUMPRODUCT(array1, … OVER T)` | number | Σ xᵢ·yᵢ·… over the rows where every measure has a cell |
| `SUMX2MY2(array_x, array_y OVER T)` | number | Σ(x² − y²) |
| `SUMX2PY2(array_x, array_y OVER T)` | number | Σ(x² + y²) |
| `SUMXMY2(array_x, array_y OVER T)` | number | Σ(x − y)² |
| `SERIESSUM(x, n, m, coefficients OVER T)` | number | Σ cᵢ·x^(n + i·m), the coefficients c in item order (i from 0) |
| `POLYNOMIAL(x, coefficients OVER T)` | number | Σ cᵢ·xⁱ, the coefficients c in item order (the constant c₀ first) |
| `PERCENTOF(data_subset, data_all OVER T)` | number | Σ subset / Σ all |
| `LAMBERTW(x, [k=0])` | number | the Lambert W function: the w with w·eʷ = x, on branch k 0 (the default, x ≥ −1/e) or −1 (−1/e ≤ x < 0) |
| `AGM(a, b)` | number | the arithmetic-geometric mean of a, b ≥ 0 |
| `GD(x)` | number | the Gudermannian function: 2·atan(tanh(x/2)) |
| `ILOG(x, [base=10])` | number | the integer logarithm: the largest k with baseᵏ ≤ x, for x > 0 and an integer base ≥ 2 (default 10) |
| `REDUCEPI(x, e, [q])` | number | x reduced modulo π/2ᵉ as if π were exact: the xr with x = xr + j·π/2ᵉ and |xr| ≤ π/2ᵉ⁺¹ (e −1 to 7, truncated), or with q TRUE the e+1 low bits of j |
| `FIB(n)` | number | the n-th Fibonacci number, n ≥ 1 (truncated); #NUM! past n = 1476, where it overflows |
| `ISPRIME(n)` | boolean | whether n is prime (FALSE below 2) |
| `ITHPRIME(i)` | number | the i-th prime, i from 1 to 10⁷ (Gnumeric: 10⁸) |
| `PFACTOR(n)` | number | the smallest prime factor of n ≥ 2 (#VALUE! below 2) |
| `NT_D(n)` | number | the number of divisors of n ≥ 1 |
| `NT_SIGMA(n)` | number | the sum of the divisors of n ≥ 1 |
| `NT_PHI(n)` | number | Euler's totient: how many of 1…n are coprime to n ≥ 1 |
| `NT_MU(n)` | number | the Möbius function of n ≥ 1: 0 if a square divides n, else (−1)^(number of prime factors) |
| `NT_OMEGA(n)` | number | the number of distinct prime factors of n ≥ 1 |
| `NT_PI(n)` | number | how many primes are ≤ n (0 below 2; n up to 179 424 673, the 10⁷-th prime) |
| `NT_RADICAL(n)` | number | the product of the distinct prime factors of n ≥ 1 |
| `PARTITIONS(n)` | number | the number of ways to write n ≥ 0 as a sum of positive integers (n up to 4000) |
| `FLT.MAX()` | number | the largest finite double, 1.7976931348623157e308 |
| `FLT.MIN()` | number | the smallest positive normal double, 2.2250738585072014e-308 |
| `FLT.RADIX()` | number | the floating-point base, 2 |
| `FLT.NEXTAFTER(x, y)` | number | the next double after x toward y ("+" or "-" for the infinities); y itself if y = x |
| `MDETERM(A OVER R, C)` | number | MDETERM(A OVER R, C): the determinant of the square matrix A (rows along R, columns along C); 0 if singular |
| `LOGMDETERM(A OVER R, C)` | number | LOGMDETERM(A OVER R, C): ln |det A| (no overflow for large matrices); #NUM! if singular |
| `MINVERSE(A OVER R, C INTO R2, C2)` | number | MINVERSE(A OVER R, C INTO R2, C2): the inverse of the square matrix A, row by row; #NUM! if singular |
| `MPSEUDOINVERSE(A OVER R, C, [threshold] INTO C2, R2)` | number | MPSEUDOINVERSE(A OVER R, C, \[threshold\] INTO C2, R2): the Moore–Penrose pseudo-inverse (C × R, row by row); singular values up to threshold (default 256 ε) times the largest count as zero |
| `CHOLESKY(A OVER R, C INTO R2, C2)` | number | CHOLESKY(A OVER R, C INTO R2, C2): the lower-triangular L with L·Lᵀ = A for a symmetric positive-definite A (asymmetric pairs averaged), row by row; #NUM! where A is not positive definite |
| `EIGEN(A OVER R, C INTO R2, C2)` | number | EIGEN(A OVER R, C INTO R2, C2): for a symmetric A (asymmetric pairs averaged), its eigenvalues largest magnitude first, then the matrix of their unit eigenvectors as columns: (n + 1) × n values, row by row |
| `FOURIER(y OVER T, [inverse] INTO K)` | text | FOURIER(y OVER T, \[inverse\] INTO K): Gnumeric's discrete Fourier transform (scaled by 1/n, zero-padded to a power of two), each value as complex text "a+bi" |

## Rounding

Rounding is decimal: `ROUND(2.675, 2)` is 2.68.

| Function | Returns | Description |
|---|---|---|
| `ROUND(number, [num_digits=0])` | number | to num_digits decimal places, halves away from zero |
| `ROUNDUP(number, [num_digits=0])` | number | away from zero to num_digits places |
| `ROUNDDOWN(number, [num_digits=0])` | number | toward zero to num_digits places |
| `ROUNDSIG(value, digits)` | number | to digits significant digits |
| `TRUNC(number, [num_digits=0])` | number | truncate to num_digits places |
| `INT(number)` | number | down to an integer |
| `FLOOR(number, [significance=1])` | number | down to a multiple of significance (default 1) |
| `CEIL(x)` | number | up to an integer |
| `CEILING(number, [significance=1])` | number | up to a multiple of significance (default 1) |
| `FLOOR.MATH(number, [significance=1], [mode=0])` | number | down to a multiple; mode ≠ 0 rounds negatives toward zero |
| `CEILING.MATH(number, [significance=1], [mode=0])` | number | up to a multiple; mode ≠ 0 rounds negatives away from zero |
| `FLOOR.PRECISE(number, [significance=1])` | number | down to a multiple, ignoring the multiple's sign |
| `CEILING.PRECISE(number, [significance=1])` | number | up to a multiple, ignoring the multiple's sign |
| `ISO.CEILING(number, [significance=1])` | number | same as CEILING.PRECISE |
| `MROUND(number, multiple)` | number | to the nearest multiple |
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
| `CONCAT(text1, …)` | text | join values as text (same as &) |
| `CONCATENATE(text1, …)` | text | join values as text |
| `EXACT(text1, text2)` | boolean | case-sensitive equality |
| `FIND(find_text, within_text, [start_num=1])` | number | 1-based position of find_text in within_text, case-sensitive |
| `SEARCH(find_text, within_text, [start_num=1])` | number | 1-based position, case-insensitive, with * and ? wildcards |
| `LEFT(text, [num_chars=1])` | text | the first num_chars characters (default 1) |
| `RIGHT(text, [num_chars=1])` | text | the last num_chars characters (default 1) |
| `MID(text, start_num, num_chars)` | text | num_chars characters from a 1-based start_num |
| `LEN(text)` | number | the number of characters |
| `LOWER(text)` | text | lowercase |
| `UPPER(text)` | text | uppercase |
| `PROPER(text)` | text | capitalize each word |
| `REPLACE(old_text, start_num, num_chars, new_text)` | text | replace num_chars characters from a 1-based start_num |
| `REPT(text, number_times)` | text | repeat text number_times times |
| `SUBSTITUTE(text, old_text, new_text, [instance_num])` | text | replace every (or the instance_num-th) occurrence of old_text |
| `TRIM(text)` | text | remove leading, trailing and repeated spaces |
| `VALUE(text)` | number | text as a number |
| `TEXTBEFORE(text, delimiter, [instance_num=1], [match_mode=0], [match_end=0], [if_not_found])` | text | text before the instance_num-th delimiter |
| `TEXTAFTER(text, delimiter, [instance_num=1], [match_mode=0], [match_end=0], [if_not_found])` | text | text after the instance_num-th delimiter |
| `TEXTBETWEEN(text, start_delimiter, end_delimiter)` | text | text between the first start delimiter and the next end delimiter |
| `COUNTMATCHES(text, search)` | number | how many times search occurs in text |
| `TEXT(value, format_text)` | text | format with an Excel number-format code: 0 # ? . , % E+ E- "text" \c _c *c, General, ; sections (positive;negative;zero;text with @), colors; dates y m d h s AM/PM A/P. Unsupported (#VALUE!): conditions, locales, \[h\] elapsed time, fractional seconds, fractions, eras |
| `FIXED(number, [decimals=2], [no_commas=FALSE])` | text | rounded to decimals places (default 2) as text, with thousands commas unless no_commas |
| `DOLLAR(number, [decimals=2])` | text | as en-US currency text: $1,234.57, negatives in parentheses, decimals default 2 |
| `NUMBERVALUE(text, [decimal_separator="."], [group_separator=","])` | number | text as a number with explicit decimal (default .) and group (default ,) separators |
| `VALUETOTEXT(value, [format=0])` | text | a value as text; format 1 quotes text |
| `REGEX(text, expression, [replacement], [flags])` | text | the first match (#N/A if none), or with a replacement the text with the first match replaced; the fourth argument is "g" (replace all) or the occurrence to replace. Patterns use the Rust regex syntax, not ICU's |
| `REGEXTEST(text, pattern, [case_sensitivity=0])` | boolean | whether the pattern matches; case_sensitivity 1 ignores case. Rust regex syntax |
| `REGEXEXTRACT(text, pattern, [return_mode=0], [case_sensitivity=0])` | text | the first match (#N/A if none); only return mode 0, modes 1 and 2 (arrays) are #VALUE!. Rust regex syntax (also `REGEX.EXTRACT`) |
| `REGEXREPLACE(text, pattern, replacement, [occurrence=0], [case_sensitivity=0])` | text | replace every match (or the occurrence-th, negative from the end) with $n-style replacement text. Rust regex syntax |
| `TEXTSPLIT(text OVER T, col_delimiter, [row_delimiter], [ignore_empty=FALSE] INTO K)` | text | the parts of the text between col_delimiter matches (case-sensitive), in order; written without OVER, each cell splits on its own: TEXTSPLIT(Text, "," INTO Part). row_delimiter must be "" (splitting into rows is not supported yet); ignore_empty drops empty parts; empty text is #VALUE! |
| `ARRAYTOTEXT(array OVER T, [format=0])` | text | the values as text in item order: format 0 "1, 2, a", format 1 {1,2,"a"} |
| `ASC(text)` | text | full-width ASCII and katakana as half-width, as LibreOffice's ASC (a voiced kana becomes its letter and a sound mark) |
| `JIS(text)` | text | half-width ASCII and katakana as full-width, as LibreOffice's JIS (a letter and its sound mark join) (also `DBCS`) |
| `BAHTTEXT(number)` | text | a number as Thai baht and satang in words, rounded to the satang |
| `ROT13(text)` | text | ASCII letters rotated 13 places; anything else unchanged |
| `ENCODEURL(text)` | text | UTF-8 percent-encoding of every byte but A–Z a–z 0–9 - _ . ~ (#VALUE! for empty text) |
| `ROMAN(number, [form=0])` | text | 0–3999 in Roman numerals; form 0 classic (or TRUE) to 4 most concise (or FALSE) |
| `ARABIC(text)` | number | Roman numerals (0–3999) as a number; case and surrounding spaces ignored, a leading - negates, "" is 0 |

## Date and time

Dates are date-time values (UTC, millisecond precision). A number given where a date is wanted is a spreadsheet serial date (1 = 1900-01-01).

| Function | Returns | Description |
|---|---|---|
| `DATE(year, month, day)` | date | a date; out-of-range months and days roll over, years 0–1899 add 1900 |
| `DATEVALUE(date_text)` | date | parse a date: 2025-01-31, 2025-01-31T09:30:00Z (the time is dropped) or 1/31/2025 |
| `TIME(hour, minute, second)` | date | a time of day on serial day 0 (1899-12-30), wrapping at 24 hours |
| `TIMEVALUE(time_text)` | number | parse a time (09:30, 9:30 PM, 2025-01-31T09:30:00Z) as a fraction of a day |
| `YEAR(serial_number)` | number | the year |
| `MONTH(serial_number)` | number | the month, 1–12 |
| `DAY(serial_number)` | number | the day of the month |
| `HOUR(serial_number)` | number | the hour, 0–23 |
| `MINUTE(serial_number)` | number | the minute |
| `SECOND(serial_number)` | number | the second, rounded |
| `WEEKDAY(serial_number, [return_type=1])` | number | day of the week: type 1 Sunday=1 (default), 2 Monday=1, 3 Monday=0, 11–17 day 1 is Monday…Sunday |
| `WEEKNUM(serial_number, [return_type=1])` | number | week of the year (the week of January 1 is 1), weeks starting on type's day as in WEEKDAY (1, 2, 11–17); type 21 is the ISO week (also `WEEKNUM_EXCEL2003`) |
| `ISOWEEKNUM(date)` | number | the ISO 8601 week number |
| `ISOYEAR(date)` | number | the ISO 8601 week-numbering year |
| `DAYOFYEAR(date)` | number | the day of the year, 1–366 |
| `DAYNAME(date)` | text | the English weekday name of a date, or of a day number 1–7 (1 is Sunday) |
| `MONTHNAME(month)` | text | the English name of month 1–12 |
| `DAYS(end_date, start_date)` | number | days from start to end (end − start, times included) |
| `DAYS360(start_date, end_date, [method=FALSE])` | number | days between on a 360-day year, US (NASD) method or European when TRUE |
| `DATEDIF(start_date, end_date, unit)` | number | complete "Y" years, "M" months or "D" days between; "YM", "YD", "MD" ignore the larger units |
| `EDATE(start_date, months)` | date | the same day n months later (clamped to the month's end) |
| `EOMONTH(start_date, months)` | date | the last day of the month n months later |
| `YEARFRAC(start_date, end_date, [basis=0])` | number | the fraction of a year between: basis 0 US 30/360 (default), 1 actual/actual, 2 actual/360, 3 actual/365, 4 European 30/360 |
| `NETWORKDAYS(start_date, end_date)` | number | working days (Monday–Friday) from start to end inclusive; no holidays argument yet (it needs an aggregation) (also `NETWORKDAYS_EXCEL2003`) |
| `NETWORKDAYS.INTL(start_date, end_date, [weekend=1])` | number | working days with a weekend of 1–7 (two days), 11–17 (one day) or a "0000011" Monday-first mask; no holidays argument yet |
| `WORKDAY(start_date, days)` | date | the date n working days (Monday–Friday) after start; no holidays argument yet |
| `WORKDAY.INTL(start_date, days, [weekend=1])` | date | WORKDAY with a weekend as in NETWORKDAYS.INTL; no holidays argument yet |
| `NOW()` | date | the current date and time (UTC); re-evaluated when the model is recalculated |
| `TODAY()` | date | the current date (UTC midnight); re-evaluated when the model is recalculated |
| `DAYSINMONTH(date)` | number | the number of days in the date's month |
| `DAYSINYEAR(date)` | number | the number of days in the date's year |
| `WEEKSINYEAR(date)` | number | the number of ISO weeks (52 or 53) in the date's year |
| `ISLEAPYEAR(date)` | boolean | whether the date's year is a leap year |
| `MONTHS(start_date, end_date, type)` | number | months between: type 0 complete months, 1 calendar months |
| `WEEKS(start_date, end_date, type)` | number | weeks between: type 0 complete weeks, 1 calendar (Monday) weeks |
| `YEARS(start_date, end_date, type)` | number | years between: type 0 complete years, 1 calendar years |
| `UNIX2DATE(t)` | date | the date of a Unix time in seconds |
| `DATE2UNIX(d)` | number | a date as Unix time in seconds |
| `DUR2DAYS(duration)` | number | a duration (a number of days) in days |
| `DUR2HOURS(duration)` | number | a duration in days as hours |
| `DUR2MINUTES(duration)` | number | a duration in days as minutes |
| `DUR2SECONDS(duration)` | number | a duration in days as seconds |
| `DUR2MILLISECONDS(duration)` | number | a duration in days as milliseconds |
| `DUR2WEEKS(duration)` | number | a duration in days as weeks |
| `STRIPDURATION(duration)` | number | the value (durations are already a number of days) |
| `EASTERSUNDAY(year)` | date | Easter Sunday (Gregorian) of a year 1583–9956; 0–29 mean 2000–2029, 30–99 mean 1930–1999 |
| `ASCENSIONTHURSDAY(year)` | date | Ascension Thursday, 39 days after Easter Sunday |
| `ASHWEDNESDAY(year)` | date | Ash Wednesday, 46 days before Easter Sunday |
| `GOODFRIDAY(year)` | date | Good Friday, 2 days before Easter Sunday |
| `PENTECOSTSUNDAY(year)` | date | Pentecost Sunday, 49 days after Easter Sunday |
| `DATE2JULIAN(date)` | number | the Julian day number of a date |
| `HDATE_JULIAN(year, month, day)` | number | the Julian day number of Gregorian year, month, day (out-of-range days and months roll over) |
| `HDATE(year, month, day)` | text | the Hebrew date of Gregorian year, month, day, as "20 Tebet 5784" (#VALUE! for a month outside 1–12 or a day outside 1–31) |
| `HDATE_HEB(year, month, day)` | text | the Hebrew date of Gregorian year, month, day in Hebrew, as "כ׳ בְּטֵבֵת התשפ״ד" |
| `HDATE_DAY(year, month, day)` | number | the day of the Hebrew month of Gregorian year, month, day |
| `HDATE_MONTH(year, month, day)` | number | the Hebrew month of Gregorian year, month, day: 0 Tishri … 11 Elul, 12 Adar I, 13 Adar II |
| `HDATE_YEAR(year, month, day)` | number | the Hebrew year of Gregorian year, month, day |
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
| `PREV(x, category, [n=1], [default])` | the branches' type | the value at the n-th previous item along the category (n = 1); off the end, the default, else no cell |
| `NEXT(x, category, [n=1], [default])` | the branches' type | the value at the n-th next item along the category (n = 1); off the end, the default, else no cell |
| `SEQUENCE(T, [start=1], [step=1])` | the branches' type | SEQUENCE(T, \[start=1\], \[step=1\]): start, start + step, … along T's items, (POSITION(T) - 1) * step + start |
| `RELABEL(x, from_category, to_category)` | the branches' type | x read with category From replaced by To, each item mapped to To's item of the same name (items with no namesake are dropped) |
| `SWAP(x, category_a, category_b)` | the branches' type | x read with categories A and B exchanged, items matched by name (W at Src = a, Dst = b is SWAP(W, Src, Dst) at Src = b, Dst = a) |
| `FILL(x, default)` | the branches' type | a cell at every item combination of x's categories: x where it has a cell, else the default |
| `SORT(array OVER T, [sort_order=1] INTO K)` | the branches' type | the values in ascending order (descending when sort_order is -1) |
| `UNIQUE(array OVER T, [exactly_once=FALSE] INTO K)` | the branches' type | each distinct value once, in item order (only values that occur once when exactly_once is TRUE) |
| `FILTER(array, include OVER T, [if_empty] INTO K)` | the branches' type | the values of array whose include is TRUE, in item order; if none, if_empty (else #N/A: Excel's #CALC!) |
| `SORTBY(array, by_array1 OVER T, [sort_order1], … INTO K)` | the branches' type | SORTBY(array, by_array1 OVER T, \[sort_order1\], \[by_array2, sort_order2\], …): the values of array sorted by by_array1, ties by by_array2, … (stable; a key descends when its order is -1) |
| `TAKE(array OVER T, rows INTO K)` | the branches' type | the first rows values in item order (the last -rows when rows is negative) |
| `DROP(array OVER T, rows INTO K)` | the branches' type | the values TAKE leaves out: all but the first rows (the last -rows when rows is negative), in item order |
| `SELECTS(values, tests OVER T, condition INTO K)` | the branches' type | Quantrix: the values whose test meets the condition (a criterion like ">5" or "a*", as in SUMIF), in item order |
| `XLOOKUP(lookup_value, lookup_array OVER T, return_array, [if_not_found], [match_mode=0], [search_mode=1])` | the values' type | return_array at the item where lookup_array matches lookup_value (match_mode 0 exact, -1 next smaller, 1 next larger, 2 wildcard; search_mode 1 first to last, -1 last to first, ±2 the same as ±1); if_not_found (else #N/A) when none does |
| `XMATCH(lookup_value, lookup_array OVER T, [match_mode=0], [search_mode=1])` | number | the 1-based item position along the category where lookup_array matches lookup_value (XLOOKUP's modes); #N/A when none does |
| `MATCH(lookup_value, lookup_array OVER T, [match_type=1])` | number | the 1-based item position of lookup_value: match_type 1 (default) the largest ≤ it, 0 the first equal (text with * and ? wildcards), -1 the smallest ≥ it; #N/A when none |
| `LOOKUP(lookup_value, lookup_vector OVER T, result_vector)` | the values' type | result_vector at the largest lookup_vector ≤ lookup_value (ascending); #N/A when none |
| `INDEX(array OVER T, row_num)` | the branches' type | the value at the row_num-th item (1-based); #REF! out of range, #N/A where that item has no cell (also `VALUEAT`) |
| `SELECT(values, keys OVER T, lookup)` | the branches' type | values at the first item (in item order) whose key equals lookup; #N/A when none (SELECTLIST: a lookup over its own category gives one result per lookup) (also `SELECTLIST`) |
| `SELECTBETWEEN(values, keys OVER T, low, high, [inclusive])` | the branches' type | values at the first item whose key is between low and high (strictly when inclusive is FALSE); #N/A when none |
| `SELECTGREATERTHAN(values, keys OVER T, lookup, [inclusive])` | the branches' type | values at the first item whose key is > lookup (≥ when inclusive is TRUE); #N/A when none |
| `SELECTLESSTHAN(values, keys OVER T, lookup, [inclusive])` | the branches' type | values at the first item whose key is < lookup (≤ when inclusive is TRUE); #N/A when none |

## Statistical

| Function | Returns | Description |
|---|---|---|
| `MODE.MULT(values OVER T INTO K)` | number | every most frequent value, in order of first occurrence (#N/A if none repeats) |
| `LINEST(known_ys, known_xs1, … OVER T, [const=TRUE], [stats=FALSE] INTO K)` | number | LINEST(known_ys, known_xs1, \[known_xs2, …\] OVER T, \[const\], \[stats\]): the least-squares fit y = m1·x1 + … + b: mk, …, m1, b (const = FALSE: b = 0); with stats, Excel's 5×(k+1) block row by row (one x: m, b, se_m, se_b, r², se_y, F, df, ss_reg, ss_resid); INTO R, C lays it out as the block |
| `LOGEST(known_ys, known_xs1, … OVER T, [const=TRUE], [stats=FALSE] INTO K)` | number | LOGEST(known_ys, known_xs1, \[known_xs2, …\] OVER T, \[const\], \[stats\]): y = b·m1^x1·…, fitted to ln y: mk, …, m1, b (const = FALSE: b = 1); with stats LINEST's block, for ln y |
| `FREQUENCY(data_array OVER T, bin1, … INTO K)` | number | counts of x in the bins given as numbers b1, b2, …: ≤ b1, (b1, b2\], …, > bn (n + 1 counts; Excel takes the bins as a range) |
| `BETA.DIST(x, alpha, beta, cumulative, [a=0], [b=1])` | number | the beta distribution of x on \[A, B\] (default \[0, 1\]): density, or cumulative |
| `BETADIST(x, alpha, beta, [a=0], [b=1])` | number | the cumulative beta distribution of x on \[A, B\] (default \[0, 1\]) |
| `BETA.INV(probability, alpha, beta, [a=0], [b=1])` | number | the beta quantile on \[A, B\] (default \[0, 1\]) (also `BETAINV`) |
| `BINOM.DIST(number_s, trials, probability_s, cumulative)` | number | the binomial probability of k successes in n trials: P(K = k), or P(K ≤ k) (also `BINOMDIST`) |
| `BINOM.INV(trials, probability_s, alpha)` | number | the smallest k with binomial P(K ≤ k) ≥ alpha, for n trials of probability p (also `CRITBINOM`) |
| `BINOM.DIST.RANGE(trials, probability_s, number_s, [number_s2])` | number | the binomial P(k1 ≤ K ≤ k2) for n trials of probability p (k2 defaults to k1) (also `B`) |
| `CHISQ.DIST(x, deg_freedom, cumulative)` | number | the chi-square distribution: density, or left tail (also `CHISQDIST`) |
| `CHISQ.DIST.RT(x, deg_freedom)` | number | the chi-square right tail (also `CHIDIST`) |
| `CHISQ.INV(probability, deg_freedom)` | number | the chi-square left-tail quantile (also `CHISQINV`) |
| `CHISQ.INV.RT(probability, deg_freedom)` | number | the chi-square right-tail quantile (also `CHIINV`) |
| `EXPON.DIST(x, lambda, cumulative)` | number | the exponential distribution of rate λ: density, or cumulative (also `EXPONDIST`) |
| `F.DIST(x, deg_freedom1, deg_freedom2, cumulative)` | number | the F distribution: density, or left tail |
| `F.DIST.RT(x, deg_freedom1, deg_freedom2)` | number | the F right tail (also `FDIST`) |
| `F.INV(probability, deg_freedom1, deg_freedom2)` | number | the F left-tail quantile |
| `F.INV.RT(probability, deg_freedom1, deg_freedom2)` | number | the F right-tail quantile (also `FINV`) |
| `GAMMA.DIST(x, alpha, beta, cumulative)` | number | the gamma distribution of shape α and scale β: density, or cumulative (also `GAMMADIST`) |
| `GAMMA.INV(probability, alpha, beta)` | number | the gamma quantile (also `GAMMAINV`) |
| `GAUSS(z)` | number | Φ(z) − 0.5: the standard normal probability between 0 and z |
| `HYPGEOM.DIST(sample_s, number_sample, population_s, number_pop, cumulative)` | number | the hypergeometric probability of k successes in a sample of n from N holding K: P(X = k), or P(X ≤ k) |
| `HYPGEOMDIST(sample_s, number_sample, population_s, number_pop)` | number | the hypergeometric probability P(X = k) |
| `LOGNORM.DIST(x, mean, standard_dev, cumulative)` | number | the log-normal distribution: density, or cumulative |
| `LOGNORMDIST(x, mean, standard_dev)` | number | the cumulative log-normal distribution |
| `LOGNORM.INV(probability, mean, standard_dev)` | number | the log-normal quantile (also `LOGINV`) |
| `NEGBINOM.DIST(number_f, number_s, probability_s, cumulative)` | number | the negative binomial probability of f failures before the s-th success: P(F = f), or P(F ≤ f) |
| `NEGBINOMDIST(number_f, number_s, probability_s)` | number | the negative binomial probability P(F = f) |
| `NORM.DIST(x, mean, standard_dev, cumulative)` | number | the normal distribution: density, or cumulative (also `NORMDIST`) |
| `NORM.INV(probability, mean, standard_dev)` | number | the normal quantile (also `NORMINV`) |
| `NORM.S.DIST(z, cumulative)` | number | the standard normal distribution: density, or cumulative |
| `NORMSDIST(z)` | number | the cumulative standard normal distribution Φ(z) |
| `NORM.S.INV(probability)` | number | the standard normal quantile (also `NORMSINV`) |
| `PHI(x)` | number | the standard normal density |
| `POISSON.DIST(x, mean, cumulative)` | number | the Poisson probability of k events at mean λ: P(X = k), or P(X ≤ k) (also `POISSON`) |
| `T.DIST(x, deg_freedom, cumulative)` | number | Student's t distribution: density, or left tail |
| `T.DIST.RT(x, deg_freedom)` | number | Student's t right tail |
| `T.DIST.2T(x, deg_freedom)` | number | Student's t two-tailed probability |
| `TDIST(x, deg_freedom, tails)` | number | Student's t right tail (tails = 1) or two-tailed probability (tails = 2), for x ≥ 0 |
| `T.INV(probability, deg_freedom)` | number | Student's t left-tail quantile |
| `T.INV.2T(probability, deg_freedom)` | number | Student's t two-tailed quantile (also `TINV`) |
| `WEIBULL.DIST(x, alpha, beta, cumulative)` | number | the Weibull distribution of shape α and scale β: density, or cumulative (also `WEIBULL`) |
| `FISHER(x)` | number | the Fisher transformation atanh(x) |
| `FISHERINV(y)` | number | the inverse Fisher transformation tanh(y) |
| `CONFIDENCE.NORM(alpha, standard_dev, size)` | number | the half-width of a normal confidence interval at significance alpha, for sd and sample size n (also `CONFIDENCE`) |
| `CONFIDENCE.T(alpha, standard_dev, size)` | number | the half-width of a Student's t confidence interval at significance alpha, for sd and sample size n |
| `STANDARDIZE(x, mean, standard_dev)` | number | the z-score (x − mean) / sd |
| `AVERAGEA(values OVER T)` | number | the mean, counting TRUE as 1 and text and FALSE as 0 |
| `AVERAGEIF(range OVER T, criteria, [average_range])` | number | the mean of average_range (else range) where range meets the criteria |
| `AVERAGEIFS(average_range, criteria_range1 OVER T, criteria1, …)` | number | the mean of x where every criteria measure meets its criterion |
| `AVEDEV(values OVER T)` | number | the mean absolute deviation from the mean |
| `COUNT(values OVER T)` | number | the number of numbers (and dates) (also `DCOUNT`) |
| `COUNTA(values OVER T)` | number | the number of cells (errors included) (also `DCOUNTA`) |
| `COUNTUNIQUE(values OVER T)` | number | the number of distinct values, SQL's count(DISTINCT …) (text compares case-sensitively; an error among them is the result) |
| `COUNTBLANK(range OVER T)` | number | the number of items with no cell (or empty text) |
| `COUNTIF(range OVER T, criteria)` | number | the number of cells meeting the criterion (">5", "<>x", "a*", 3) |
| `COUNTIFS(criteria_range1 OVER T, criteria1, …)` | number | the number of rows where every criteria measure meets its criterion |
| `SUMIF(range OVER T, criteria, [sum_range])` | number | the sum of sum_range (else range) where range meets the criteria |
| `SUMIFS(sum_range, criteria_range1 OVER T, criteria1, …)` | number | the sum of x where every criteria measure meets its criterion |
| `MAXA(values OVER T)` | number | the largest value, counting TRUE as 1 and text and FALSE as 0 |
| `MAXIFS(max_range, criteria_range1 OVER T, criteria1, …)` | number | the largest x where every criteria measure meets its criterion (0 if none) |
| `MINA(values OVER T)` | number | the smallest value, counting TRUE as 1 and text and FALSE as 0 |
| `MINIFS(min_range, criteria_range1 OVER T, criteria1, …)` | number | the smallest x where every criteria measure meets its criterion (0 if none) |
| `MEDIAN(values OVER T)` | number | the median |
| `MODE(values OVER T)` | number | the most frequent value (the first in item order on a tie; #N/A if none repeats) (also `MODE.SNGL`) |
| `GEOMEAN(values OVER T)` | number | the geometric mean (#NUM! for a value ≤ 0) |
| `HARMEAN(values OVER T)` | number | the harmonic mean (#NUM! for a value ≤ 0) |
| `TRIMMEAN(array OVER T, percent)` | number | the mean without the fraction of values at the tails (rounded down to an even count) |
| `DEVSQ(values OVER T)` | number | Σ(x − mean)² |
| `STDEV(values OVER T)` | number | the sample standard deviation (also `STDEV.S`, `DSTDEV`) |
| `STDEVP(values OVER T)` | number | the population standard deviation (also `STDEV.P`, `DSTDEVP`) |
| `STDEVA(values OVER T)` | number | the sample standard deviation, counting TRUE as 1 and text and FALSE as 0 |
| `STDEVPA(values OVER T)` | number | the population standard deviation, counting TRUE as 1 and text and FALSE as 0 |
| `VAR(values OVER T)` | number | the sample variance (also `VAR.S`, `DVAR`) |
| `VARP(values OVER T)` | number | the population variance (also `VAR.P`, `DVARP`) |
| `VARA(values OVER T)` | number | the sample variance, counting TRUE as 1 and text and FALSE as 0 |
| `VARPA(values OVER T)` | number | the population variance, counting TRUE as 1 and text and FALSE as 0 |
| `KURT(values OVER T)` | number | the sample excess kurtosis |
| `KURTP(values OVER T)` | number | the population excess kurtosis |
| `SKEW(values OVER T)` | number | the sample skewness |
| `SKEWP(values OVER T)` | number | the population skewness (also `SKEW.P`) |
| `LARGE(array OVER T, k)` | number | the k-th largest value |
| `SMALL(array OVER T, k)` | number | the k-th smallest value |
| `PERCENTILE(array OVER T, k)` | number | the p-th percentile, interpolated, 0 ≤ p ≤ 1 (also `PERCENTILE.INC`) |
| `PERCENTILE.EXC(array OVER T, k)` | number | the p-th percentile, exclusive of 0 and 1 |
| `QUARTILE(array OVER T, quart)` | number | the q-th quartile (0 min … 4 max) (also `QUARTILE.INC`) |
| `QUARTILE.EXC(array OVER T, quart)` | number | the q-th quartile (1 … 3), exclusive |
| `PROB(x_range, prob_range OVER T, lower_limit, [upper_limit])` | number | Σ p over the values x in \[low, high\] (x = low without high); #NUM! unless every 0 < p ≤ 1 and Σ p = 1 |
| `SUBTOTAL(function_num, values OVER T)` | number | the reduction a code picks: 1 AVERAGE, 2 COUNT, 3 COUNTA, 4 MAX, 5 MIN, 6 PRODUCT, 7 STDEV, 8 STDEVP, 9 SUM, 10 VAR, 11 VARP (101–111 the same) |
| `AGGREGATE(function_num, options, array OVER T, [k])` | number | SUBTOTAL's codes and 12 MEDIAN, 13 MODE, 14 LARGE, 15 SMALL, 16 PERCENTILE, 17 QUARTILE, 18 PERCENTILE.EXC, 19 QUARTILE.EXC; options 2, 3, 6, 7 skip errors |
| `RANK(ref OVER T, [order=0])` | number | each item's rank among the values (order 0, the default, ranks the largest 1) (also `RANK.EQ`) |
| `RANK.AVG(ref OVER T, [order=0])` | number | each item's rank, ties sharing their mean rank |
| `PERCENTRANK(array OVER T, [x], [significance=3])` | number | each item's (or x's) rank as a fraction 0 … 1, to significance (3) significant digits (also `PERCENTRANK.INC`) |
| `RANK.DENSE(ref OVER T, [order=0])` | number | each item's dense rank: tied values share a rank and the next value ranks one after them, SQL's dense_rank() (order 0, the default, ranks the largest 1) |
| `PERCENTRANK.EXC(array OVER T, [x], [significance=3])` | number | each item's (or x's) rank as a fraction, exclusive of 0 and 1 |
| `TREND(known_ys, known_xs OVER T, [const=TRUE])` | number | each item's value on the least-squares line of y on x, at its own x (through 0 if const is FALSE) |
| `GROWTH(known_ys, known_xs OVER T, [const=TRUE])` | number | each item's value on the least-squares curve y = b·mˣ (fitting ln y; b = 1 if const is FALSE) |
| `CORREL(array1, array2 OVER T)` | number | the Pearson correlation of x and y (also `PEARSON`) |
| `RSQ(known_ys, known_xs OVER T)` | number | the square of the Pearson correlation |
| `COVAR(array1, array2 OVER T)` | number | the population covariance (also `COVARIANCE.P`) |
| `COVARIANCE.S(array1, array2 OVER T)` | number | the sample covariance |
| `SLOPE(known_ys, known_xs OVER T)` | number | the least-squares slope of y on x |
| `INTERCEPT(known_ys, known_xs OVER T)` | number | the least-squares intercept of y on x |
| `STEYX(known_ys, known_xs OVER T)` | number | the standard error of the predicted y |
| `FORECAST(x, known_ys, known_xs OVER T)` | number | the least-squares line of y on x at x0 (also `FORECAST.LINEAR`) |
| `EXPA(known_ys, known_xs OVER T, [simple])` | number | A of the fit y = A·e^(Bx): least squares of ln y weighted by y (simple = TRUE: unweighted, as LOGEST) |
| `EXPB(known_ys, known_xs OVER T, [simple])` | number | B of the fit y = A·e^(Bx) |
| `EXPFORECAST(x, known_ys, known_xs OVER T, [simple])` | number | the fit y = A·e^(Bx) at x0 |
| `CHITEST(actual_range, expected_range OVER T)` | number | the χ² test p-value of actual against expected (also `CHISQ.TEST`) |
| `FTEST(array1, array2 OVER T)` | number | the two-tailed F-test p-value that a and b have the same variance (also `F.TEST`) |
| `TTEST(array1, array2 OVER T, tails, type)` | number | the t-test p-value (tails 1 or 2; type 1 paired, 2 equal variance, 3 unequal) (also `T.TEST`) |
| `ZTEST(array OVER T, x, [sigma])` | number | the one-tailed z-test p-value of the mean against μ0 (σ defaults to the sample's) (also `Z.TEST`) |
| `AUTOCORREL(values OVER T)` | number | the lag-1 autocorrelation in item order (GSL) (also `GSL_STATS_LAG1_AUTOCORRELATION`) |
| `DGET(values OVER T)` | the branches' type | the single value (with WHERE, the single match); #NUM! when more than one (a WHERE that keeps none leaves no cell) |
| `PROBBLOCK(traffic, circuits)` | number | the Erlang-B probability that a call is blocked when traffic erlangs load circuits (1 to 10⁶; truncated below 100) |
| `OFFTRAF(traffic, circuits)` | number | the offered traffic that carries traffic erlangs on circuits (#VALUE! if circuits cannot carry it) |
| `DIMCIRC(traffic, gos)` | number | the fewest circuits (up to 10⁶) that carry traffic erlangs with blocking at most gos, 0 < gos ≤ 1 |
| `OFFCAP(circuits, gos)` | number | the traffic circuits carry with blocking gos, 0 < gos < 1 |
| `BERNOULLI(k, p)` | number | the Bernoulli probability of k (0 or 1) for success probability p |
| `CAUCHY(x, a, cumulative)` | number | the Cauchy distribution of scale a at x: density, or (as Gnumeric) the right tail P(X > x) |
| `EXPPOWDIST(x, a, b)` | number | the exponential-power density of scale a and exponent b at x |
| `GEOMDIST(k, p, cumulative)` | number | the geometric probability of k failures before the first success: P(K = k), or P(K ≤ k) |
| `LANDAU(x)` | number | the Landau density at x (Kölbig and Schorr's approximation, about 9 digits) |
| `LAPLACE(x, a)` | number | the Laplace density of width a at x |
| `LOGISTIC(x, a)` | number | the logistic density of scale a at x |
| `PARETO(x, a, b)` | number | the Pareto density of exponent a and scale b at x |
| `RAYLEIGH(x, sigma)` | number | the Rayleigh density of scale σ at x |
| `RAYLEIGHTAIL(x, a, sigma)` | number | the Rayleigh tail density (lower limit a, scale σ) at x |
| `SNORM.DIST.RANGE(x1, x2)` | number | Φ(x2) − Φ(x1): the standard normal probability between x1 and x2 |
| `OWENT(h, a)` | number | Owen's T function T(h, a) |
| `CUM_BIV_NORM_DIST(a, b, rho)` | number | the standard bivariate normal P(X ≤ a, Y ≤ b) with correlation ρ, |ρ| < 1 |
| `R.DBETA(x, shape1, shape2, [give_log])` | number | the density at x of the beta distribution of shapes shape1 and shape2, or its logarithm (give_log) |
| `R.PBETA(x, shape1, shape2, [lower_tail], [log_p])` | number | P(X ≤ x) for the beta distribution of shapes shape1 and shape2, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QBETA(p, shape1, shape2, [lower_tail], [log_p])` | number | the quantile of the beta distribution of shapes shape1 and shape2: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DBINOM(x, size, prob, [give_log])` | number | the probability mass at x of the binomial distribution of size trials of success probability prob, or its logarithm (give_log) |
| `R.PBINOM(x, size, prob, [lower_tail], [log_p])` | number | P(X ≤ x) for the binomial distribution of size trials of success probability prob, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QBINOM(p, size, prob, [lower_tail], [log_p])` | number | the quantile of the binomial distribution of size trials of success probability prob: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DCAUCHY(x, location, scale, [give_log])` | number | the density at x of the Cauchy distribution of location and scale, or its logarithm (give_log) |
| `R.PCAUCHY(x, location, scale, [lower_tail], [log_p])` | number | P(X ≤ x) for the Cauchy distribution of location and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QCAUCHY(p, location, scale, [lower_tail], [log_p])` | number | the quantile of the Cauchy distribution of location and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DCHISQ(x, df, [give_log])` | number | the density at x of the chi-square distribution of df degrees of freedom, or its logarithm (give_log) |
| `R.PCHISQ(x, df, [lower_tail], [log_p])` | number | P(X ≤ x) for the chi-square distribution of df degrees of freedom, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QCHISQ(p, df, [lower_tail], [log_p])` | number | the quantile of the chi-square distribution of df degrees of freedom: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DEXP(x, scale, [give_log])` | number | the density at x of the exponential distribution of the given scale (mean), or its logarithm (give_log) |
| `R.PEXP(x, scale, [lower_tail], [log_p])` | number | P(X ≤ x) for the exponential distribution of the given scale (mean), or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QEXP(p, scale, [lower_tail], [log_p])` | number | the quantile of the exponential distribution of the given scale (mean): the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DF(x, df1, df2, [give_log])` | number | the density at x of the F distribution of df1 and df2 degrees of freedom, or its logarithm (give_log) |
| `R.PF(x, df1, df2, [lower_tail], [log_p])` | number | P(X ≤ x) for the F distribution of df1 and df2 degrees of freedom, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QF(p, df1, df2, [lower_tail], [log_p])` | number | the quantile of the F distribution of df1 and df2 degrees of freedom: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DGAMMA(x, shape, scale, [give_log])` | number | the density at x of the gamma distribution of shape and scale, or its logarithm (give_log) |
| `R.PGAMMA(x, shape, scale, [lower_tail], [log_p])` | number | P(X ≤ x) for the gamma distribution of shape and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QGAMMA(p, shape, scale, [lower_tail], [log_p])` | number | the quantile of the gamma distribution of shape and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DGEOM(x, prob, [give_log])` | number | the probability mass at x of the geometric distribution (failures before the first success) of success probability prob, or its logarithm (give_log) |
| `R.PGEOM(x, prob, [lower_tail], [log_p])` | number | P(X ≤ x) for the geometric distribution (failures before the first success) of success probability prob, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QGEOM(p, prob, [lower_tail], [log_p])` | number | the quantile of the geometric distribution (failures before the first success) of success probability prob: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DGUMBEL(x, mu, beta, [give_log])` | number | the density at x of the Gumbel distribution of location μ and scale β, or its logarithm (give_log) |
| `R.PGUMBEL(x, mu, beta, [lower_tail], [log_p])` | number | P(X ≤ x) for the Gumbel distribution of location μ and scale β, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QGUMBEL(p, mu, beta, [lower_tail], [log_p])` | number | the quantile of the Gumbel distribution of location μ and scale β: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DHYPER(x, m, n, k, [give_log])` | number | the probability mass at x of the hypergeometric distribution of the white balls among k drawn from m white and n black, or its logarithm (give_log) |
| `R.PHYPER(x, m, n, k, [lower_tail], [log_p])` | number | P(X ≤ x) for the hypergeometric distribution of the white balls among k drawn from m white and n black, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QHYPER(p, m, n, k, [lower_tail], [log_p])` | number | the quantile of the hypergeometric distribution of the white balls among k drawn from m white and n black: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DLNORM(x, meanlog, sdlog, [give_log])` | number | the density at x of the log-normal distribution of log-mean and log-sd, or its logarithm (give_log) |
| `R.PLNORM(x, meanlog, sdlog, [lower_tail], [log_p])` | number | P(X ≤ x) for the log-normal distribution of log-mean and log-sd, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QLNORM(p, meanlog, sdlog, [lower_tail], [log_p])` | number | the quantile of the log-normal distribution of log-mean and log-sd: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DNBINOM(x, size, prob, [give_log])` | number | the probability mass at x of the negative binomial distribution (failures before the size-th success) of success probability prob, or its logarithm (give_log) |
| `R.PNBINOM(x, size, prob, [lower_tail], [log_p])` | number | P(X ≤ x) for the negative binomial distribution (failures before the size-th success) of success probability prob, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QNBINOM(p, size, prob, [lower_tail], [log_p])` | number | the quantile of the negative binomial distribution (failures before the size-th success) of success probability prob: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DNORM(x, mean, sd, [give_log])` | number | the density at x of the normal distribution of mean μ and standard deviation σ, or its logarithm (give_log) |
| `R.PNORM(x, mean, sd, [lower_tail], [log_p])` | number | P(X ≤ x) for the normal distribution of mean μ and standard deviation σ, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QNORM(p, mean, sd, [lower_tail], [log_p])` | number | the quantile of the normal distribution of mean μ and standard deviation σ: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DPOIS(x, lambda, [give_log])` | number | the probability mass at x of the Poisson distribution of mean λ, or its logarithm (give_log) |
| `R.PPOIS(x, lambda, [lower_tail], [log_p])` | number | P(X ≤ x) for the Poisson distribution of mean λ, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QPOIS(p, lambda, [lower_tail], [log_p])` | number | the quantile of the Poisson distribution of mean λ: the smallest x with P(X ≤ x) ≥ p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DRAYLEIGH(x, scale, [give_log])` | number | the density at x of the Rayleigh distribution of the given scale, or its logarithm (give_log) |
| `R.PRAYLEIGH(x, scale, [lower_tail], [log_p])` | number | P(X ≤ x) for the Rayleigh distribution of the given scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QRAYLEIGH(p, scale, [lower_tail], [log_p])` | number | the quantile of the Rayleigh distribution of the given scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DSNORM(x, shape, location, scale, [give_log])` | number | the density at x of the skew-normal distribution of shape, location and scale, or its logarithm (give_log) |
| `R.PSNORM(x, shape, location, scale, [lower_tail], [log_p])` | number | P(X ≤ x) for the skew-normal distribution of shape, location and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QSNORM(p, shape, location, scale, [lower_tail], [log_p])` | number | the quantile of the skew-normal distribution of shape, location and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DST(x, df, shape, [give_log])` | number | the density at x of the skew-t distribution of df degrees of freedom and the given shape, or its logarithm (give_log) |
| `R.PST(x, df, shape, [lower_tail], [log_p])` | number | P(X ≤ x) for the skew-t distribution of df degrees of freedom and the given shape, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QST(p, df, shape, [lower_tail], [log_p])` | number | the quantile of the skew-t distribution of df degrees of freedom and the given shape: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DT(x, df, [give_log])` | number | the density at x of the Student t distribution of df degrees of freedom, or its logarithm (give_log) |
| `R.PT(x, df, [lower_tail], [log_p])` | number | P(X ≤ x) for the Student t distribution of df degrees of freedom, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QT(p, df, [lower_tail], [log_p])` | number | the quantile of the Student t distribution of df degrees of freedom: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.DWEIBULL(x, shape, scale, [give_log])` | number | the density at x of the Weibull distribution of shape and scale, or its logarithm (give_log) |
| `R.PWEIBULL(x, shape, scale, [lower_tail], [log_p])` | number | P(X ≤ x) for the Weibull distribution of shape and scale, or P(X > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QWEIBULL(p, shape, scale, [lower_tail], [log_p])` | number | the quantile of the Weibull distribution of shape and scale: the x with P(X ≤ x) = p, or of P(X > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `R.PTUKEY(x, nmeans, df, [nranges=1], [lower_tail], [log_p])` | number | P(Q ≤ x) for the largest of nranges (default 1) studentized ranges of nmeans means with df degrees of freedom, or P(Q > x) (lower_tail = FALSE), or its logarithm (log_p) |
| `R.QTUKEY(p, nmeans, df, [nranges=1], [lower_tail], [log_p])` | number | the quantile of the studentized range of nmeans means with df degrees of freedom, the largest of nranges (default 1): the x with P(Q ≤ x) = p, or of P(Q > x) (lower_tail = FALSE), p may be a logarithm (log_p) |
| `SSMEDIAN(array OVER T, [interval=1])` | number | the median of grouped data (Gnumeric): each value the midpoint of an interval of width interval (default 1), interpolated within the median's interval |
| `INTERPOLATION(xs, ys OVER T, target, [method], [next])` | number | INTERPOLATION(xs, ys OVER T, target, \[method\], \[next\]): ys interpolated at target (Gnumeric): method 0 linear, 2 staircase, 4 natural cubic spline; 1, 3, 5 the same averaged over \[target, next\] |
| `HPFILTER(values OVER T, [lambda=1600])` | number | each item's trend under the Hodrick–Prescott filter with smoothing λ (default 1600); the cycle is y − trend; needs 6 values |
| `ADTEST(values OVER T)` | number | the p-value of the Anderson–Darling test of normality (Gnumeric; 8 values or more) |
| `CVMTEST(values OVER T)` | number | the p-value of the Cramér–von Mises test of normality (Gnumeric; 8 values or more) |
| `LKSTEST(values OVER T)` | number | the p-value of the Lilliefors (Kolmogorov–Smirnov) test of normality (Gnumeric; 5 values or more) |
| `SFTEST(values OVER T)` | number | the p-value of the Shapiro–Francia test of normality (Gnumeric; 5 to 5000 values) |
| `ADTEST.STAT(values OVER T)` | number | the Anderson–Darling statistic A² (adjusted for n), Gnumeric's ADTEST second value |
| `CVMTEST.STAT(values OVER T)` | number | the Cramér–von Mises statistic W², Gnumeric's CVMTEST second value |
| `LKSTEST.STAT(values OVER T)` | number | the Lilliefors statistic D, Gnumeric's LKSTEST second value |
| `SFTEST.STAT(values OVER T)` | number | the Shapiro–Francia statistic W', Gnumeric's SFTEST second value |
| `LOGREG(known_ys, known_xs OVER T, [affine], [stats] INTO K)` | number | LOGREG(known_ys, known_xs OVER T, \[affine\], \[stats\] INTO K): the least-squares fit y = m·ln x + b (Gnumeric): m, b (affine = FALSE: b = 0); with stats LINEST's 5×2 block for ln x, row by row |
| `LOGFIT(known_ys, known_xs OVER T INTO K)` | number | LOGFIT(known_ys, known_xs OVER T INTO K): the fit y = a + b·ln(sign·(x − c)) by Gnumeric's trial-and-error search: sign, a, b, c and the sum of squared residuals; #NUM! if the points are not logarithmic enough |
| `PERIODOGRAM(y OVER T, [filter] INTO K)` | number | PERIODOGRAM(y OVER T, \[filter\] INTO K): the magnitudes of the first half of the Fourier transform (Gnumeric), after a window: 0 none, 1 Bartlett, 2 Hann, 3 Welch |
| `CRONBACH(x1, x2, … OVER T)` | number | CRONBACH(x1, x2, … OVER T): Cronbach's α of the items x1, x2, … (one row per respondent; population variances, as Gnumeric) |
| `FORECAST.ETS(target_date, values, timeline OVER T, [seasonality], [data_completion], [aggregation])` | number | FORECAST.ETS(target_date, values, timeline OVER T, \[seasonality\], \[data_completion\], \[aggregation\]): the additive Holt–Winters (triple exponential smoothing) forecast at target_date, LibreOffice's algorithm; seasonality 1 detects the season, 0 none; a target_date before the timeline's end is #NUM! (also `FORECAST.ETS.ADD`) |
| `FORECAST.ETS.MULT(target_date, values, timeline OVER T, [seasonality], [data_completion], [aggregation])` | number | FORECAST.ETS.MULT(target_date, values, timeline OVER T, …): FORECAST.ETS with multiplicative seasons (LibreOffice) |
| `FORECAST.ETS.CONFINT(target_date, values, timeline OVER T, [confidence_level=0.95], [seasonality], [data_completion], [aggregation])` | number | FORECAST.ETS.CONFINT(target_date, values, timeline OVER T, \[confidence_level\], \[seasonality\], …): the half-width of the prediction interval (default confidence_level 0.95); only for seasonality 0 (LibreOffice simulates seasonal intervals randomly): otherwise #N/A (also `FORECAST.ETS.PI.ADD`) |
| `FORECAST.ETS.PI.MULT(target_date, values, timeline OVER T, [confidence_level=0.95], [seasonality], [data_completion], [aggregation])` | number | FORECAST.ETS.PI.MULT(target_date, values, timeline OVER T, …): FORECAST.ETS.CONFINT for the multiplicative model (the same for seasonality 0, the only one supported) |
| `FORECAST.ETS.STAT(values, timeline OVER T, statistic_type, [seasonality], [data_completion], [aggregation])` | number | FORECAST.ETS.STAT(values, timeline OVER T, statistic_type, …): a statistic of FORECAST.ETS's fit: 1 α, 2 β (trend), 3 γ (season), 4 MASE, 5 SMAPE, 6 MAE, 7 RMSE, 8 step, 9 seasonality (also `FORECAST.ETS.STAT.ADD`) |
| `FORECAST.ETS.STAT.MULT(values, timeline OVER T, statistic_type, [seasonality], [data_completion], [aggregation])` | number | FORECAST.ETS.STAT.MULT(values, timeline OVER T, statistic_type, …): FORECAST.ETS.STAT of the multiplicative model |
| `FORECAST.ETS.SEASONALITY(values, timeline OVER T, [data_completion], [aggregation])` | number | FORECAST.ETS.SEASONALITY(values, timeline OVER T, \[data_completion\], \[aggregation\]): the season length FORECAST.ETS detects (1: none) |
| `LEVERAGE(A OVER R, C INTO K)` | number | LEVERAGE(A OVER R, C INTO K): the regression leverages, the diagonal of A·(AᵀA)⁻¹·Aᵀ, one per row of A (Gnumeric); #NUM! if A's columns are dependent |

## Financial

Rates are per period; cash flows are in item order along their `OVER` category.

| Function | Returns | Description |
|---|---|---|
| `FV(rate, nper, pmt, [pv], [type])` | number | future value: FV(rate, nper, pmt, \[pv\], \[type\]) |
| `PV(rate, nper, pmt, [fv], [type])` | number | present value: PV(rate, nper, pmt, \[fv\], \[type\]) |
| `PMT(rate, nper, pv, [fv], [type])` | number | payment per period: PMT(rate, nper, pv, \[fv\], \[type\]) |
| `IPMT(rate, per, nper, pv, [fv], [type])` | number | interest part of payment per: IPMT(rate, per, nper, pv, \[fv\], \[type\]) |
| `PPMT(rate, per, nper, pv, [fv], [type])` | number | principal part of payment per: PPMT(rate, per, nper, pv, \[fv\], \[type\]) |
| `NPER(rate, pmt, pv, [fv], [type])` | number | number of periods: NPER(rate, pmt, pv, \[fv\], \[type\]) |
| `RATE(nper, pmt, pv, [fv], [type], [guess])` | number | rate per period (iterative): RATE(nper, pmt, pv, \[fv\], \[type\], \[guess\]) |
| `CUMIPMT(rate, nper, pv, start_period, end_period, type)` | number | interest paid from period start to end: CUMIPMT(rate, nper, pv, start_period, end_period, type) (also `CUMIPMT_ADD`) |
| `CUMPRINC(rate, nper, pv, start_period, end_period, type)` | number | principal paid from period start to end: CUMPRINC(rate, nper, pv, start_period, end_period, type) (also `CUMPRINC_ADD`) |
| `ISPMT(rate, per, nper, pv)` | number | interest of period per with straight-line principal: ISPMT(rate, per, nper, pv) |
| `EFFECT(nominal_rate, npery)` | number | effective annual rate of a nominal rate compounded npery times (also `EFFECT_ADD`) |
| `NOMINAL(effect_rate, npery)` | number | nominal annual rate of an effective rate compounded npery times (also `NOMINAL_ADD`) |
| `PDURATION(rate, pv, fv)` | number | periods for pv to grow to fv at rate: PDURATION(rate, pv, fv) (also `G_DURATION`) |
| `RRI(nper, pv, fv)` | number | equivalent growth rate per period: RRI(nper, pv, fv) |
| `DOLLARDE(fractional_dollar, fraction)` | number | a fractional dollar price (1.02 with fraction 16 is 1 and 2/16) as a decimal |
| `DOLLARFR(decimal_dollar, fraction)` | number | a decimal dollar price as a fraction (in the integer part's units) |
| `SLN(cost, salvage, life)` | number | straight-line depreciation per period: SLN(cost, salvage, life) |
| `SYD(cost, salvage, life, per)` | number | sum-of-years'-digits depreciation: SYD(cost, salvage, life, per) |
| `DB(cost, salvage, life, period, [month=12])` | number | fixed declining balance: DB(cost, salvage, life, period, \[month=12\]) |
| `DDB(cost, salvage, life, period, [factor=2])` | number | double declining balance: DDB(cost, salvage, life, period, \[factor=2\]) |
| `VDB(cost, salvage, life, start_period, end_period, [factor=2], [no_switch])` | number | declining balance between two periods: VDB(cost, salvage, life, start_period, end_period, \[factor=2\], \[no_switch\]) |
| `AMORDEGRC(cost, date_purchased, first_period, salvage, period, rate, [basis])` | number | French degressive depreciation: AMORDEGRC(cost, date_purchased, first_period, salvage, period, rate, \[basis\]) |
| `AMORLINC(cost, date_purchased, first_period, salvage, period, rate, [basis])` | number | French linear depreciation: AMORLINC(cost, date_purchased, first_period, salvage, period, rate, \[basis\]) |
| `ACCRINT(issue, first_interest, settlement, rate, par, frequency, [basis], [calc_method])` | number | accrued interest of a periodic-coupon security: ACCRINT(issue, first_interest, settlement, rate, par, frequency, \[basis\], \[calc_method\]) |
| `ACCRINTM(issue, settlement, rate, [par=1000], [basis])` | number | accrued interest paid at maturity: ACCRINTM(issue, settlement, rate, \[par=1000\], \[basis\]) |
| `COUPDAYBS(settlement, maturity, frequency, [basis])` | number | days from the coupon period's start to settlement |
| `COUPDAYS(settlement, maturity, frequency, [basis])` | number | days in the coupon period holding settlement |
| `COUPDAYSNC(settlement, maturity, frequency, [basis])` | number | days from settlement to the next coupon |
| `COUPNCD(settlement, maturity, frequency, [basis])` | date | the next coupon date after settlement |
| `COUPNUM(settlement, maturity, frequency, [basis])` | number | coupons payable between settlement and maturity |
| `COUPPCD(settlement, maturity, frequency, [basis])` | date | the coupon date on or before settlement |
| `DISC(settlement, maturity, pr, redemption, [basis])` | number | discount rate: DISC(settlement, maturity, pr, redemption, \[basis\]) |
| `DURATION(settlement, maturity, coupon, yld, frequency, [basis])` | number | Macaulay duration: DURATION(settlement, maturity, coupon, yld, frequency, \[basis\]) (also `BONDDURATION`) |
| `MDURATION(settlement, maturity, coupon, yld, frequency, [basis])` | number | modified duration: MDURATION(settlement, maturity, coupon, yld, frequency, \[basis\]) (also `BONDMDURATION`) |
| `INTRATE(settlement, maturity, investment, redemption, [basis])` | number | interest rate of a fully invested security: INTRATE(settlement, maturity, investment, redemption, \[basis\]) |
| `ODDFPRICE(settlement, maturity, issue, first_coupon, rate, yld, redemption, frequency, [basis])` | number | price with an odd first period: ODDFPRICE(settlement, maturity, issue, first_coupon, rate, yld, redemption, frequency, \[basis\]) |
| `ODDFYIELD(settlement, maturity, issue, first_coupon, rate, pr, redemption, frequency, [basis])` | number | yield with an odd first period: ODDFYIELD(settlement, maturity, issue, first_coupon, rate, pr, redemption, frequency, \[basis\]) |
| `ODDLPRICE(settlement, maturity, last_interest, rate, yld, redemption, frequency, [basis])` | number | price with an odd last period: ODDLPRICE(settlement, maturity, last_interest, rate, yld, redemption, frequency, \[basis\]) |
| `ODDLYIELD(settlement, maturity, last_interest, rate, pr, redemption, frequency, [basis])` | number | yield with an odd last period: ODDLYIELD(settlement, maturity, last_interest, rate, pr, redemption, frequency, \[basis\]) |
| `PRICE(settlement, maturity, rate, yld, redemption, frequency, [basis])` | number | price per 100 face value: PRICE(settlement, maturity, rate, yld, redemption, frequency, \[basis\]) |
| `PRICEDISC(settlement, maturity, discount, redemption, [basis])` | number | price of a discounted security: PRICEDISC(settlement, maturity, discount, redemption, \[basis\]) |
| `PRICEMAT(settlement, maturity, issue, rate, yld, [basis])` | number | price of a security paying interest at maturity: PRICEMAT(settlement, maturity, issue, rate, yld, \[basis\]) |
| `RECEIVED(settlement, maturity, investment, discount, [basis])` | number | amount at maturity of a fully invested security: RECEIVED(settlement, maturity, investment, discount, \[basis\]) |
| `TBILLEQ(settlement, maturity, discount)` | number | bond-equivalent yield of a Treasury bill: TBILLEQ(settlement, maturity, discount) |
| `TBILLPRICE(settlement, maturity, discount)` | number | price per 100 face of a Treasury bill: TBILLPRICE(settlement, maturity, discount) |
| `TBILLYIELD(settlement, maturity, pr)` | number | yield of a Treasury bill: TBILLYIELD(settlement, maturity, pr) |
| `YIELD(settlement, maturity, rate, pr, redemption, frequency, [basis])` | number | yield of a periodic-coupon security (iterative): YIELD(settlement, maturity, rate, pr, redemption, frequency, \[basis\]) |
| `YIELDDISC(settlement, maturity, pr, redemption, [basis])` | number | annual yield of a discounted security: YIELDDISC(settlement, maturity, pr, redemption, \[basis\]) |
| `YIELDMAT(settlement, maturity, issue, rate, pr, [basis])` | number | annual yield of a security paying interest at maturity: YIELDMAT(settlement, maturity, issue, rate, pr, \[basis\]) |
| `NPV(rate, values OVER T)` | number | net present value of the flows in item order, the first discounted one period (a missing item is skipped, not zero): NPV(rate, values OVER T) |
| `XNPV(rate, values, dates OVER T)` | number | net present value of flows at dates, discounted from the first date (365-day years): XNPV(rate, values, dates OVER T) |
| `IRR(values OVER T, [guess=0.1])` | number | internal rate of return of the flows in item order (iterative): IRR(values OVER T, \[guess=0.1\]) |
| `XIRR(values, dates OVER T, [guess=0.1])` | number | internal rate of return of flows at dates (iterative): XIRR(values, dates OVER T, \[guess=0.1\]) |
| `MIRR(values OVER T, finance_rate, reinvest_rate)` | number | modified internal rate of return, outflows financed and inflows reinvested: MIRR(values OVER T, finance_rate, reinvest_rate) |
| `FMRR(values OVER T, safe_rate, reinvest_rate, reinvest_min)` | number | financial management rate of return (GNU Oleo): outflows pulled back at the safe rate, inflows of at least reinvest_min pushed forward at the reinvestment rate: FMRR(values OVER T, safe_rate, reinvest_rate, reinvest_min) |
| `FVSCHEDULE(principal, schedule OVER T)` | number | principal compounded over the rates in item order: FVSCHEDULE(principal, schedule OVER T) |
| `OPT_BS(call_put, spot, strike, time, rate, volatility, [cost_of_carry])` | number | Black-Scholes price of a European option: OPT_BS(call_put, spot, strike, time, rate, volatility, \[cost_of_carry\]) |
| `OPT_BS_DELTA(call_put, spot, strike, time, rate, volatility, [cost_of_carry])` | number | Black-Scholes delta: OPT_BS_DELTA(call_put, spot, strike, time, rate, volatility, \[cost_of_carry\]) |
| `OPT_BS_GAMMA(spot, strike, time, rate, volatility, [cost_of_carry])` | number | Black-Scholes gamma (calls and puts alike): OPT_BS_GAMMA(spot, strike, time, rate, volatility, \[cost_of_carry\]) |
| `OPT_BS_RHO(call_put, spot, strike, time, rate, volatility, [cost_of_carry])` | number | Black-Scholes rho: OPT_BS_RHO(call_put, spot, strike, time, rate, volatility, \[cost_of_carry\]) |
| `OPT_BS_THETA(call_put, spot, strike, time, rate, volatility, [cost_of_carry])` | number | Black-Scholes theta: OPT_BS_THETA(call_put, spot, strike, time, rate, volatility, \[cost_of_carry\]) |
| `OPT_BS_VEGA(spot, strike, time, rate, volatility, [cost_of_carry])` | number | Black-Scholes vega (calls and puts alike): OPT_BS_VEGA(spot, strike, time, rate, volatility, \[cost_of_carry\]) |
| `OPT_BS_CARRYCOST(call_put, spot, strike, time, rate, volatility, [cost_of_carry])` | number | Black-Scholes sensitivity to the cost of carry: OPT_BS_CARRYCOST(call_put, spot, strike, time, rate, volatility, \[cost_of_carry\]) |
| `OPT_GARMAN_KOHLHAGEN(call_put, spot, strike, time, domestic_rate, foreign_rate, volatility)` | number | currency option: OPT_GARMAN_KOHLHAGEN(call_put, spot, strike, time, domestic_rate, foreign_rate, volatility) |
| `OPT_FRENCH(call_put, spot, strike, trading_time, calendar_time, rate, volatility, cost_of_carry)` | number | Black-Scholes with trading-day volatility: OPT_FRENCH(call_put, spot, strike, trading_time, calendar_time, rate, volatility, cost_of_carry) |
| `OPT_JUMP_DIFF(call_put, spot, strike, time, rate, volatility, lambda, gamma)` | number | Merton jump diffusion: OPT_JUMP_DIFF(call_put, spot, strike, time, rate, volatility, lambda, gamma) |
| `OPT_MILTERSEN_SCHWARTZ(call_put, p_t, f_t, strike, t1, t2, v_s, v_e, v_f, rho_se, rho_sf, rho_ef, kappa_e, kappa_f)` | number | commodity futures option: OPT_MILTERSEN_SCHWARTZ(call_put, p_t, f_t, strike, t1, t2, v_s, v_e, v_f, rho_se, rho_sf, rho_ef, kappa_e, kappa_f) |
| `OPT_EXEC(call_put, spot, strike, time, rate, volatility, cost_of_carry, lambda)` | number | executive stock option: OPT_EXEC(call_put, spot, strike, time, rate, volatility, cost_of_carry, lambda) |
| `OPT_FORWARD_START(call_put, spot, alpha, time_start, time, rate, volatility, cost_of_carry)` | number | forward start option: OPT_FORWARD_START(call_put, spot, alpha, time_start, time, rate, volatility, cost_of_carry) |
| `OPT_TIME_SWITCH(call_put, spot, strike, a, time, m, dt, rate, cost_of_carry, volatility)` | number | discrete time switch option: OPT_TIME_SWITCH(call_put, spot, strike, a, time, m, dt, rate, cost_of_carry, volatility) |
| `OPT_SIMPLE_CHOOSER(spot, strike, time1, time2, rate, cost_of_carry, volatility)` | number | simple chooser option: OPT_SIMPLE_CHOOSER(spot, strike, time1, time2, rate, cost_of_carry, volatility) |
| `OPT_BAW_AMER(call_put, spot, strike, time, rate, volatility, cost_of_carry)` | number | Barone-Adesi-Whaley American approximation: OPT_BAW_AMER(call_put, spot, strike, time, rate, volatility, cost_of_carry) |
| `OPT_BJER_STENS(call_put, spot, strike, time, rate, volatility, cost_of_carry)` | number | Bjerksund-Stensland American approximation: OPT_BJER_STENS(call_put, spot, strike, time, rate, volatility, cost_of_carry) |
| `OPT_RGW(spot, strike, time_payout, time_exp, rate, d, volatility)` | number | Roll-Geske-Whaley American call with one known dividend: OPT_RGW(spot, strike, time_payout, time_exp, rate, d, volatility) |
| `OPT_BINOMIAL(amer_euro, call_put, steps, spot, strike, time, rate, volatility, [cost_of_carry])` | number | Cox-Ross-Rubinstein binomial tree: OPT_BINOMIAL(amer_euro, call_put, steps, spot, strike, time, rate, volatility, \[cost_of_carry\]) |
| `OPT_COMPLEX_CHOOSER(spot, strike_call, strike_put, time, time_call, time_put, rate, cost_of_carry, volatility)` | number | complex chooser option: OPT_COMPLEX_CHOOSER(spot, strike_call, strike_put, time, time_call, time_put, rate, cost_of_carry, volatility) |
| `OPT_ON_OPTIONS(type, spot, strike1, strike2, time1, time2, rate, cost_of_carry, volatility)` | number | option on an option ("cc", "cp", "pc", "pp"): OPT_ON_OPTIONS(type, spot, strike1, strike2, time1, time2, rate, cost_of_carry, volatility) |
| `OPT_EXTENDIBLE_WRITER(call_put, spot, strike1, strike2, time1, time2, rate, cost_of_carry, volatility)` | number | writer-extendible option: OPT_EXTENDIBLE_WRITER(call_put, spot, strike1, strike2, time1, time2, rate, cost_of_carry, volatility) |
| `OPT_2_ASSET_CORRELATION(call_put, spot1, spot2, strike1, strike2, time, cost_of_carry1, cost_of_carry2, rate, volatility1, volatility2, rho)` | number | two-asset correlation option: OPT_2_ASSET_CORRELATION(call_put, spot1, spot2, strike1, strike2, time, cost_of_carry1, cost_of_carry2, rate, volatility1, volatility2, rho) |
| `OPT_EURO_EXCHANGE(spot1, spot2, qty1, qty2, time, rate, cost_of_carry1, cost_of_carry2, volatility1, volatility2, rho)` | number | European option to exchange asset 2 for asset 1: OPT_EURO_EXCHANGE(spot1, spot2, qty1, qty2, time, rate, cost_of_carry1, cost_of_carry2, volatility1, volatility2, rho) |
| `OPT_AMER_EXCHANGE(spot1, spot2, qty1, qty2, time, rate, cost_of_carry1, cost_of_carry2, volatility1, volatility2, rho)` | number | American option to exchange asset 2 for asset 1: OPT_AMER_EXCHANGE(spot1, spot2, qty1, qty2, time, rate, cost_of_carry1, cost_of_carry2, volatility1, volatility2, rho) |
| `OPT_SPREAD_APPROX(call_put, fut_price1, fut_price2, strike, time, rate, volatility1, volatility2, rho)` | number | Kirk's futures spread option approximation: OPT_SPREAD_APPROX(call_put, fut_price1, fut_price2, strike, time, rate, volatility1, volatility2, rho) |
| `OPT_FIXED_STRK_LKBK(call_put, spot, spot_min, spot_max, strike, time, rate, cost_of_carry, volatility)` | number | fixed-strike lookback: OPT_FIXED_STRK_LKBK(call_put, spot, spot_min, spot_max, strike, time, rate, cost_of_carry, volatility) |
| `OPT_FLOAT_STRK_LKBK(call_put, spot, spot_min, spot_max, time, rate, cost_of_carry, volatility)` | number | floating-strike lookback: OPT_FLOAT_STRK_LKBK(call_put, spot, spot_min, spot_max, time, rate, cost_of_carry, volatility) |
| `OPT_BARRIER(spot, vol, r, rf, maturity, strike, lower, upper, rebate, put_call, in_out, continuous_or_end, [greek])` | number | barrier option: OPT_BARRIER(spot, vol, r, rf, maturity, strike, lower, upper, rebate, put_call, in_out, continuous_or_end, \[greek\]) |
| `OPT_TOUCH(spot, vol, r, rf, maturity, lower, upper, foreign_domestic, in_out, continuous_or_end, [greek])` | number | one/no-touch option: OPT_TOUCH(spot, vol, r, rf, maturity, lower, upper, foreign_domestic, in_out, continuous_or_end, \[greek\]) |
| `OPT_PROB_HIT(spot, vol, mu, maturity, lower, upper)` | number | probability of touching a barrier: OPT_PROB_HIT(spot, vol, mu, maturity, lower, upper) |
| `OPT_PROB_INMONEY(spot, vol, mu, maturity, lower, upper, [strike], [put_call])` | number | probability of ending between the barriers (and in the money): OPT_PROB_INMONEY(spot, vol, mu, maturity, lower, upper, \[strike\], \[put_call\]) |

## Engineering

Complex numbers are text, `"3+4i"` (suffix `i` or `j`); base conversions use 10-digit two's complement for negatives.

| Function | Returns | Description |
|---|---|---|
| `BIN2DEC(number)` | number | binary text as a number |
| `BIN2HEX(number, [places])` | text | binary text as hexadecimal, zero-padded to `places` |
| `BIN2OCT(number, [places])` | text | binary text as octal, zero-padded to `places` |
| `DEC2BIN(number, [places])` | text | an integer in −512…511 as binary text |
| `DEC2HEX(number, [places])` | text | an integer in −2³⁹…2³⁹−1 as hexadecimal text |
| `DEC2OCT(number, [places])` | text | an integer in −2²⁹…2²⁹−1 as octal text |
| `HEX2BIN(number, [places])` | text | hexadecimal text as binary |
| `HEX2DEC(number)` | number | hexadecimal text as a number |
| `HEX2OCT(number, [places])` | text | hexadecimal text as octal |
| `OCT2BIN(number, [places])` | text | octal text as binary |
| `OCT2DEC(number)` | number | octal text as a number |
| `OCT2HEX(number, [places])` | text | octal text as hexadecimal |
| `BASE(number, radix, [min_length])` | text | an integer in 0…2⁵³−1 as text in a radix 2…36, zero-padded to `min_length` (also `NUMTOBASE`) |
| `DECIMAL(text, radix)` | number | text in a radix 2…36 as a number (also `BASETONUM`) |
| `HEXREP(x)` | text | the bytes of a number's IEEE double, little-endian, in hexadecimal |
| `BITAND(number1, number2)` | number | bitwise and |
| `BITOR(number1, number2)` | number | bitwise or |
| `BITXOR(number1, number2)` | number | bitwise exclusive or |
| `BITLSHIFT(number, shift_amount)` | number | shifted left by `shift_amount` bits (right if negative), at most 53 either way |
| `BITRSHIFT(number, shift_amount)` | number | shifted right by `shift_amount` bits (left if negative), at most 53 either way |
| `CONVERT(number, from_unit, to_unit)` | number | a measurement converted between units of Excel's table (SI and binary prefixes allowed); #N/A for unknown or mismatched units |
| `DELTA(number1, [number2=0])` | number | 1 if the numbers are equal (number2 defaults to 0), else 0 |
| `GESTEP(number, [step=0])` | number | 1 if number ≥ step (default 0), else 0 |
| `COMPLEX(real_num, i_num, [suffix="i"])` | text | the complex number re + im·i as text, with suffix "i" (default) or "j" |
| `IMREAL(inumber)` | number | a complex number's real part |
| `IMAGINARY(inumber)` | number | a complex number's imaginary part |
| `IMABS(inumber)` | number | a complex number's modulus |z| |
| `IMARGUMENT(inumber)` | number | a complex number's angle in radians, −π…π (#NUM! at 0) |
| `IMCONJUGATE(inumber)` | text | the complex conjugate |
| `IMNEG(inumber)` | text | −z |
| `IMINV(inumber)` | text | 1/z |
| `IMSUM(inumber1, …)` | text | the sum of complex numbers |
| `IMPRODUCT(inumber1, …)` | text | the product of complex numbers |
| `IMSUB(inumber1, inumber2)` | text | z1 − z2 |
| `IMDIV(inumber1, inumber2)` | text | z1 / z2 |
| `IMPOWER(inumber, number)` | text | z raised to a real power |
| `IMSQRT(inumber)` | text | the principal square root |
| `IMEXP(inumber)` | text | e^z |
| `IMLN(inumber)` | text | the principal natural logarithm |
| `IMLOG10(inumber)` | text | the principal base-10 logarithm |
| `IMLOG2(inumber)` | text | the principal base-2 logarithm |
| `IMSIN(inumber)` | text | the complex sine |
| `IMCOS(inumber)` | text | the complex cosine |
| `IMTAN(inumber)` | text | the complex tangent |
| `IMSEC(inumber)` | text | the complex secant |
| `IMCSC(inumber)` | text | the complex cosecant |
| `IMCOT(inumber)` | text | the complex cotangent |
| `IMSINH(inumber)` | text | the complex hyperbolic sine |
| `IMCOSH(inumber)` | text | the complex hyperbolic cosine |
| `IMTANH(inumber)` | text | the complex hyperbolic tangent |
| `IMSECH(inumber)` | text | the complex hyperbolic secant |
| `IMCSCH(inumber)` | text | the complex hyperbolic cosecant |
| `IMCOTH(inumber)` | text | the complex hyperbolic cotangent |
| `IMARCSIN(inumber)` | text | the complex arcsine (principal branch) |
| `IMARCCOS(inumber)` | text | the complex arccosine |
| `IMARCTAN(inumber)` | text | the complex arctangent |
| `IMARCSEC(inumber)` | text | the complex arcsecant |
| `IMARCCSC(inumber)` | text | the complex arccosecant |
| `IMARCCOT(inumber)` | text | the complex arccotangent |
| `IMARCSINH(inumber)` | text | the complex inverse hyperbolic sine |
| `IMARCCOSH(inumber)` | text | the complex inverse hyperbolic cosine |
| `IMARCTANH(inumber)` | text | the complex inverse hyperbolic tangent |
| `IMARCSECH(inumber)` | text | the complex inverse hyperbolic secant |
| `IMARCCSCH(inumber)` | text | the complex inverse hyperbolic cosecant |
| `IMARCCOTH(inumber)` | text | the complex inverse hyperbolic cotangent |
| `IMFACT(inumber)` | text | the complex factorial z! = Γ(z + 1) |
| `IMGAMMA(inumber)` | text | the complex gamma function Γ(z) |
| `BESSELI(x, n)` | number | the modified Bessel function Iₙ(x) |
| `BESSELJ(x, n)` | number | the Bessel function Jₙ(x) |
| `BESSELK(x, n)` | number | the modified Bessel function Kₙ(x), x > 0 |
| `BESSELY(x, n)` | number | the Bessel function Yₙ(x), x > 0 |
| `IMIGAMMA(a, inumber, [lower=TRUE], [regularize=TRUE])` | text | the complex incomplete gamma function of a and z: lower (default) or upper, regularized (default) or not |
