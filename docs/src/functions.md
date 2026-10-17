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
