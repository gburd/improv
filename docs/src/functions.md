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
| `SUM(Measure OVER Category)` | number | the sum over a category |
| `AVERAGE(Measure OVER Category)` | number | the mean over a category (also `AVG`) |
| `MIN(Measure OVER Category)` | number | the smallest value over a category |
| `MAX(Measure OVER Category)` | number | the largest value over a category |
| `PRODUCT(Measure OVER Category)` | number | the product over a category |
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

## Statistical

| Function | Returns | Description |
|---|---|---|
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
