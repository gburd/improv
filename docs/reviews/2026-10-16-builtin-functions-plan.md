# Built-in functions: a five-product inventory and a plan for Improv

Date: 2026-10-16 · Status: **approved 2026-10-16, with the §9 decisions
below. Nothing is built yet.**

## Decisions (2026-10-16)

| # | Question | Decision |
|---|---|---|
| 1 | Canonical names | **Excel names canonical**; Quantrix names are aliases. |
| 2 | Errors | **Typed errors** (`#DIV/0!`, `#N/A`, `#NUM!`, `#VALUE!`, …) that propagate, per §6.6. |
| 3 | Indexing | **1-based**, as in every product surveyed. |
| 4 | Dates | **Real `DateTime` values**; serial numbers only at import/export, `N()` and `DATEVALUE`. |
| 5 | LAMBDA | **Yes**: `LAMBDA`, `MAP`, `REDUCE`, `SCAN`, `MAKEARRAY`, `BYROW`, `BYCOL`, plus named `LAMBDA` definitions. See §6.12 for recursion. |
| 6 | Locale | **Locale-aware formula entry**: decimal comma, `;` argument separator, and translated function names, alongside the canonical locale-independent form (§6.13). |
| 7 | `^` | **Mathematical convention**: `^` is right-associative and binds tighter than unary minus, so `-2^2` = −4 and `2^3^2` = 512. This deliberately differs from Excel and LibreOffice; a formula imported from a spreadsheet is parenthesized on import so its value is kept. |


This review lists every built-in function of Excel (Microsoft 365), LibreOffice
Calc, Gnumeric, Apple Numbers and Quantrix Modeler. It sorts their union into
the categories Improv needs. It says how each family maps onto a dimensional
model, and it proposes a phased plan to add the union to Improv's formula
language. The user approves the plan (and answers §9) before anything is built.

Source letters used throughout: **E** Excel, **L** LibreOffice, **G** Gnumeric,
**N** Numbers, **Q** Quantrix. They are written in alphabetical order (`EGLNQ`).

## 1. Summary

| Product | Functions | How counted (2026-10-16) |
|---|---|---|
| Excel (Microsoft 365) | **521** | support.microsoft.com "Excel functions (alphabetical)" and "(by category)", 514 rows; rows such as "FIND, FINDB" split into 521 names |
| LibreOffice Calc 25.8.5.2 | **531** | the installed build's `com.sun.star.sheet.FunctionDescriptions` UNO service, queried with headless `soffice` (names, categories, argument lists) |
| Gnumeric 1.12.62 | **656** | source tag `GNUMERIC_1_12_62`: `plugins/fn-*/plugin.xml.in` (652) plus `src/func-builtin.c` (`sum`, `product`, `if`, `gnumeric_version`) |
| Apple Numbers | **355** | support.apple.com "Formulas and Functions Help" table of contents, 11 categories |
| Quantrix Modeler | **233** (lower bound) | Quantrix 5.1 web help (Wayback Machine, 2015) function pages, plus the structural functions and the 2021–2023 release notes |

- **Union by name:** 873 distinct names.
- **Union after folding aliases:** 787 distinct functions. 86 names are
  aliases: legacy and compatibility spellings (`STDEV`/`STDEV.S`), LibreOffice's
  `_ADD`/`_EXCEL2003` duplicates, the DBCS byte variants (`LENB`) and
  cross-product synonyms (`ISEMPTY`/`ISBLANK`).
- **In scope for Improv:** **672** functions, or 663 if the LAMBDA family is
  declined.
- **Out of scope:** 115, each with a one-line reason (§5.24).
- **Check:** the classified lists in §5 contain each product function exactly
  once. Recomputed totals: E 521, L 531, G 656, N 355, Q 233.

The brief expected roughly Excel 500, LibreOffice 500, Gnumeric 500+, Numbers
250+ and Quantrix 150. The measured figures are a little higher for every
product. Quantrix's current documentation could not be fetched, so 233 is a
floor.

Phase sizes (functions newly available per phase; details in §8):

| P1 | P2 | P3 | P4 | P5 | P6 | P7 |
|---|---|---|---|---|---|---|
| 121 | 66 | 9 | 124 | 55 | 44 | 253 |

P6 includes 9 LAMBDA-family functions, which depend on question 5. P3's value
comes mostly from language forms rather than named functions: previous/next,
running windows, multi-category `OVER` and `EXCEPT`.

## 2. Sources, fetch status, and what is unverified

- **Excel.** Fetched both Microsoft pages. Each row's category prefix gives the
  category. Page quirks are kept as published: `VSTACK`, `WRAPCOLS` and
  `WRAPROWS` are listed under "Look and reference", and `NORM.INV` under
  "Compatibility". The page lists `GROUPBY`, `PIVOTBY`, `TRIMRANGE`,
  `PERCENTOF`, `DETECTLANGUAGE`, `TRANSLATE` and the three `REGEX*` functions.
  It does not list `PY` or `COPILOT`, so they are not counted. Excel argument
  signatures are written from knowledge where LibreOffice's argument list
  differed **(from knowledge, unverified)**.
- **LibreOffice.** The help.libreoffice.org "Functions by Category" pages
  rendered only partly without JavaScript. The installed LibreOffice 25.8.5.2
  was therefore asked directly through UNO, which is authoritative for that
  build. LibreOffice's *Add-in* category (the Analysis add-in: Bessel, complex,
  bases) is folded into Engineering here.
- **Gnumeric.** `help.gnome.org/users/gnumeric` returns 404, and `ssconvert` is
  not installed here. The function list comes from the release tag's plugin
  manifests. Signatures and one-line meanings come from each plugin's
  `GnmFuncHelp` strings. Two test-suite-only builtins, `number_match` and
  `deriv`, are excluded.
- **Numbers.** The live table of contents lists 355 functions. The pages of the
  27 Numbers-only functions were spot-checked for their meaning.
- **Quantrix.** The current help site returned 403 or no response:
  `quantrix.com/help/modeler` redirects to `help.idbs.com/Quantrix/...`, which
  answers 403, as does `info.idbs.com`, and `docs.quantrix.com` did not answer.
  The inventory therefore comes from Wayback Machine snapshots of the Quantrix
  5.1 web help (July 2015). Pages used: the function-category pages (date and
  time, financial, information, logical, math, statistical, probability, text,
  trigonometric, lookup and reference), Structural Functions, Recurrence
  Formulas, Using the Select Function, Mathematical Operators, About Quantrix
  Formulas, and Summary Items. Added from release notes: `concatlists()`
  (2021) and `@itemCount()` (2023). Functions added after 5.1 and not named in
  those notes are missing, so treat 233 as a floor.
  - Quantrix has **no** functions named `previous`, `next`, `cumulative` or
    `running`. Those behaviours are *recurrence keywords*: `Cat[THIS]`,
    `[PREV]`, `[NEXT]`, `[FIRST]`, `[LAST]`, `[PREV-n]`/`[NEXT+n]`, and the
    "soft" `[~PREV]`, which treats a missing neighbour as 0. Ranges use `..`:
    `sum(Month[PREV-2]:Data .. Month[PREV]:Data)`.
  - Constraints are written `In X, …` and `… skip X`. Lookups are written
    `select(...)` and `… using A as B`.
  - `#Cat` is an item's 1-based index and `@Cat` its name.
  - Summary items are Sum, Count, CountA, Average, Max, Min and Product.
- **Behaviour checked against LibreOffice 25.8.5.2** (headless, today):
  - `PMT(0.05/12,360,200000)` = −1073.6432460242781.
  - `NORM.S.INV(0.975)` = 1.9599639845400536.
  - `IRR({-100,30,40,50})` = 0.08896339469334466.
  - `1/0` gives `#DIV/0!`; `SQRT(-1)`, `LN(0)`, `FLOOR(-2.5,1)` and
    `CEILING(-2.5,1)` give Err:502 (invalid argument).
  - `-2^2` = 4 and `2^3^2` = 64, so unary minus binds tighter than `^` and
    `^` is left-associative. `0^0` = 1.
  - `"a"&1+2` = `a3` and `1+2&"a"` = `3a`, so `+` binds tighter than `&`.
    `50%` = 0.5.
  - `MOD(-7,3)` = 2 and `MOD(5,-3)` = −1 (the result takes the divisor's sign).
  - `ROUND(2.5,0)` = 3, `ROUND(-2.5,0)` = −3, and `ROUND(2.675,2)` = 2.68.
    The last is decimal-correct; naive binary `f64` arithmetic gives 2.67.
  - `ATAN2(1,2)` = 1.1071487…, which is atan(2/1). The argument order is
    (x, y), the reverse of C's `atan2(y, x)`.
  - `DATE(2024,2,30)` rolls over to 2024-03-01.
  - `IF(FALSE(),1)` = FALSE.
- **From knowledge, unverified:**
  - Excel matches LibreOffice on `-2^2` and `2^3^2`, but returns `#NUM!` for
    `0^0`.
  - Excel's 1900 date system counts the fictitious 1900-02-29 (serial 60).
  - Excel's `FORECAST.ETS`, `FORECAST.ETS.CONFINT` and `FORECAST.ETS.STAT`
    correspond to LibreOffice's `FORECAST.ETS.ADD`, `FORECAST.ETS.PI.ADD` and
    `FORECAST.ETS.STAT.ADD`.
  - Excel's `REGEX*` functions use PCRE2 syntax.
  - Numbers' operator precedence.

## 3. What Improv has today, and how a function is added

Confirmed by reading the code:

- **Operators:** `+ - * /`, the comparisons `== <> != < <= > >=`, `AND`, `OR`,
  `NOT` and unary `-`. `=` is only the assignment separator.
- **Scalar functions** (`parser::scalar_func`, `compiler::scalar_arity`,
  `dataflow::apply_scalar`): `ABS`=10, `ROUND`=11, `FLOOR`=12, `CEIL`=13,
  `SQRT`=14, `NEG`=15 (all arity 1), and `MIN2`=20, `MAX2`=21 (arity 2). All
  take Number and return Number.
- **Aggregations:** `SUM`/`AVG`/`MIN`/`MAX` with ids 1–4, written
  `F(m OVER Cat)` with one category. The AST's `DimensionSpec.over` is already
  a `Vec`; the parser rejects `OVER A OVER B`.
- **Literals:** number, `"text"`, `TRUE`/`FALSE`, `#date#`. Measure names can
  be quoted (`'Unit Price'`).
- **Source forms:** `Target = CALL(fn, m…)` and `Target = SQL("…")`. Both are
  host-side refreshes and never enter the dataflow.
- **Values:** `ValueType` is Number, Boolean, Text, DateTime or Enum. The
  engine's `CellValue` is `Num(u64 bits)`, `Bool`, `Text`, `Date(i64 ms since
  the Unix epoch, UTC)` or `Err(u8)`. `ValueErrorKind` covers TypeMismatch,
  DimensionMismatch, DivisionByZero, MissingInput and Custom.
- **Missing:** `IF`, `^`/`POWER`, previous-period references, `EXCEPT` syntax,
  multi-category `OVER`. Today `x/0` and `SQRT(-1)` give **NaN**.

**Adding one scalar function today touches eight places,** and the id table is
copied five times:

1. `crates/core_model/src/parser.rs` — `scalar_func`: name → (`FuncId`, arity).
2. `crates/engine/src/compiler.rs` — `scalar_arity`, and the `infer` arm that
   assumes every argument is Number and the result is Number.
3. `crates/engine/src/dataflow.rs` — `apply_scalar(FuncId, &[f64]) -> f64`. In
   `build_coll`, `FuncCall` handles only `[a]` and `[a, b]`. The aggregation
   ids are copied again as constants here.
4. `crates/engine/src/scalar.rs` — `check` and `eval_checked`. These reuse
   `apply_scalar` and coerce arguments with `as_num().unwrap_or(NaN)`.
5. `crates/nl_formula/src/lib.rs` — CNL words exist only for ids 1–4. A scalar
   call is described as "function 10 of …".
6. `crates/gui/src/app.rs` — `func_name` hard-codes a third copy of the scalar
   names.
7. `crates/pg_model/src/delta.rs` — `expr_elementwise` treats any id with a
   `scalar_arity` as element-wise. A future positional function would wrongly
   be planned as a per-coordinate recompute.
8. Docs — `docs/src/formulas.md` (the scalar table and its "Gaps" list) and
   `.agent/steering/AGENT_FORMULA_LANGUAGE.md` §11.2.

Findings that bear on the plan:

- **The ROUND comment is wrong.** The compiler's table describes `ROUND` as
  "half-to-even (f64::round)". `f64::round` rounds half away from zero, which
  is what `docs/src/formulas.md` says.
- **FuncIds are a storage format.** `storage_mentat` stores a derived measure's
  formula as serde JSON of `Formula`, so ids are persisted. They must be
  append-only and never renumbered.
- **Existing arities conflict with the spreadsheets.** Improv has `ROUND(x)`,
  `FLOOR(x)` and `CEIL(x)`; the products have `ROUND(x, digits)` and
  `FLOOR(x, significance)`. Making the extra argument optional keeps every
  stored formula valid.
- **The steering grammar disagrees with every product checked.**
  `AGENT_FORMULA_LANGUAGE.md` §4 makes `^` right-associative
  (`Factor = Primary [ "^" Factor ]`). LibreOffice and Quantrix's documented
  precedence table make it left-associative, below unary minus (question 7).

## 4. Categories

"In scope" counts functions to implement (aliases excluded). The product
columns count every name from that product classified into the row, including
its aliases and out-of-scope names, so each column sums to that product's total.
Mapping kinds: **S** element-wise scalar · **R** reduction over a category
(`OVER`) · **P** positional over an ordered category · **L** lookup ·
**A** array/set · **F** language form · **—** not applicable.

| Category | In scope | E | L | G | N | Q | Improv mapping |
|---|---|---|---|---|---|---|---|
| Logical | 11 | 10 | 10 | 10 | 10 | 8 | S (IF family lazy, §6.1); AND/OR/XOR also R |
| Error handling | 10 | 7 | 8 | 8 | 2 | 8 | S over typed error values (§6.6) |
| Information / type | 13 | 10 | 12 | 10 | 8 | 8 | S |
| Math & trig, core | 65 | 53 | 55 | 65 | 53 | 38 | S; SUM/PRODUCT/SUMSQ/SUMPRODUCT also R |
| Math, special functions | 13 | 3 | 3 | 13 | 2 | 1 | S |
| Math, matrices | 8 | 3 | 3 | 8 | 3 | 0 | A over a pair of categories |
| Math, number theory & float | 15 | 0 | 0 | 15 | 0 | 0 | S |
| Rounding | 17 | 15 | 18 | 11 | 10 | 9 | S |
| Statistical, descriptive & conditional | 53 | 60 | 60 | 52 | 41 | 36 | R (with optional `WHERE`); RANK/PERCENTRANK reduce, then join back |
| Statistical, distributions | 109 | 61 | 63 | 103 | 26 | 26 | S |
| Statistical, regression & forecasting | 30 | 19 | 23 | 21 | 8 | 11 | R over measure pairs; TREND/GROWTH/ETS are P |
| Statistical, tests | 8 | 8 | 8 | 8 | 3 | 4 | R |
| Financial, TVM | 17 | 17 | 21 | 17 | 12 | 7 | S; FVSCHEDULE is R over an ordered category |
| Financial, depreciation | 7 | 7 | 7 | 7 | 5 | 5 | S |
| Financial, bonds & securities | 26 | 26 | 26 | 26 | 18 | 0 | S |
| Financial, cash flow (NPV/IRR/XIRR) | 5 | 5 | 5 | 5 | 5 | 5 | R over an ordered category |
| Financial, option pricing | 32 | 0 | 4 | 28 | 0 | 0 | S |
| Date & time | 59 | 25 | 36 | 43 | 32 | 18 | S |
| Text | 46 | 49 | 45 | 45 | 32 | 23 | S; TEXTJOIN is an ordered R; TEXTSPLIT is A |
| Lookup & reference | 5 | 5 | 5 | 5 | 5 | 3 | L (join on key) |
| Engineering, bases & bits | 20 | 19 | 19 | 20 | 19 | 0 | S |
| Engineering, complex numbers | 45 | 26 | 26 | 45 | 0 | 0 | S over `"a+bi"` text |
| Engineering, Bessel, erf, CONVERT | 9 | 11 | 11 | 9 | 7 | 0 | S |
| Database (DSUM …) | 12 | 12 | 12 | 12 | 0 | 0 | R with `WHERE` |
| Array / dynamic (FILTER … LAMBDA) | 18 | 16 | 8 | 3 | 17 | 1 | A, and F for LET/LAMBDA |
| Quantrix dimensional | 15 | 0 | 0 | 0 | 0 | 16 | structural metadata, P, L |
| Other (Erlang traffic) | 4 | 0 | 0 | 4 | 0 | 0 | S |
| Out: web, cube, external data, add-in | 0 (out: 24) | 16 | 4 | 2 | 6 | 0 | — |
| Out: cell / sheet / workbook references | 0 (out: 38) | 20 | 21 | 23 | 15 | 3 | — |
| Out: grid reshaping | 0 (out: 16) | 15 | 13 | 5 | 13 | 0 | — |
| Out: random numbers | 0 (out: 37) | 3 | 5 | 33 | 3 | 3 | — |
| **Total** | **672** | **521** | **531** | **656** | **355** | **233** | |

## 5. Per-category function lists

Columns: name · signature (Improv form for reductions) · meaning · sources ·
kind · phase. `alias` rows name a spelling that maps onto an earlier row and
count toward the "aliases", not "in scope". When two spellings exist, the one
with the most general signature is primary. When the signatures are identical,
the spelling most products share is primary.

In reductions, `x OVER T` means "the values of measure `x` along category `T`".
Extra arguments are scalars, broadcast per group. Every reduction also accepts
the spreadsheet variadic form `F(a, b, c)` element-wise over measures.

### 5.1 Logical <!-- sec:logical -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `IF` | IF(test, then, [else]) | the branch the test selects; lazy | EGLNQ | S | P1 |
| `IFS` | IFS(test1, value1, …) | value of the first true test | EGLN | S | P1 |
| `SWITCH` | SWITCH(expr, match1, result1, …, [default]) | result of the first match (Excel meaning; Quantrix `switch` is IFS-shaped, App. A) | EGLNQ | S | P1 |
| `CASE` | CASE(test, case1, value1, …, [default]) | Quantrix spelling of Excel `SWITCH` | Q | alias | = |
| `CHOOSE` | CHOOSE(index, v1, …) | the index-th argument, 1-based | EGLNQ | S | P1 |
| `AND` | AND(b1, …) or AND(b OVER T) | all true | EGLNQ | S | P1 |
| `OR` | OR(b1, …) | any true | EGLNQ | S | P1 |
| `XOR` | XOR(b1, …) | an odd number true | EGLN | S | P1 |
| `NOT` | NOT(b) | negation (operator exists) | EGLNQ | S | P1 |
| `TRUE`, `FALSE` | TRUE(), FALSE() | boolean constants (literals exist) | EGLN | S | P1 |
| `BETWEEN` | BETWEEN(x, low, high, [inclusive=TRUE]) | low ≤ x ≤ high | Q | S | P1 |

### 5.2 Error handling <!-- sec:error -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `IFERROR` | IFERROR(value, alt) | alt if value is any error | EGLN | S | P1 |
| `IFNA` | IFNA(value, alt) | alt if value is #N/A | EGL | S | P1 |
| `CLEARERROR` | CLEARERROR(value, [alt]) | alt, or no cell, where value is an error | Q | S | P1 |
| `CLEARZERO` | CLEARZERO(value, [alt]) | alt, or no cell, where value is 0 | Q | S | P1 |
| `ISERROR` | ISERROR(v) | any error | EGLNQ | S | P1 |
| `ISERR` | ISERR(v) | an error other than #N/A | EGLQ | S | P1 |
| `ISNA` | ISNA(v) | is #N/A | EGLQ | S | P1 |
| `NA` | NA() | the #N/A error | EGLQ | S | P1 |
| `ERROR` | ERROR(name) | the named error value | G | S | P1 |
| `ERR` | ERR([text]) | Quantrix: an error value, #VALUE! by default | Q | alias | = |
| `ERROR.TYPE` | ERROR.TYPE(v) | error code (#NULL!=1, #DIV/0!=2, #VALUE!=3, #REF!=4, #NAME?=5, #NUM!=6, #N/A=7) | EGL | S | P1 |
| `ERRORTYPE` | ERRORTYPE(v) | LibreOffice/Quantrix spelling (LibreOffice numbers its own Err:5xx) | LQ | alias | = |

### 5.3 Information / type <!-- sec:info -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `ISBLANK` | ISBLANK(v) | no cell at this coordinate | EGLNQ | S | P2 |
| `ISEMPTY` | ISEMPTY(v) | Quantrix spelling of ISBLANK | Q | alias | = |
| `ISNUMBER` | ISNUMBER(v) | is a number | EGLNQ | S | P2 |
| `ISTEXT` | ISTEXT(v) | is text | EGLNQ | S | P2 |
| `ISNONTEXT` | ISNONTEXT(v) | is not text (or is blank) | EGLQ | S | P2 |
| `ISLOGICAL` | ISLOGICAL(v) | is boolean | EGL | S | P2 |
| `ISDATE` | ISDATE(v) | is a date | N | S | P2 |
| `ISNUMBERORDATE` | ISNUMBERORDATE(v) | is a number or a date | N | S | P2 |
| `ISEVEN` | ISEVEN(n) | trunc(n) is even | EGLN | S | P2 |
| `ISODD` | ISODD(n) | trunc(n) is odd | EGLN | S | P2 |
| `ISEVEN_ADD`, `ISODD_ADD` | — | LibreOffice Analysis add-in duplicates | L | alias | = |
| `N` | N(v) | as number: TRUE→1, date→serial, other→0 | EGLQ | S | P2 |
| `T` | T(v) | v if text, else "" | EGLN | S | P2 |
| `TYPE` | TYPE(v) | 1 number, 2 text, 4 boolean, 16 error, 64 array | EGLQ | S | P2 |
| `S` | S(x OVER T) | Quantrix: the first value, as text (`T(FIRST(x OVER T))`) | Q | R | P3 |

### 5.4 Math & trig, core <!-- sec:math -->

Improv's `NEG`, `MIN2` and `MAX2` stay as aliases of `-x`, `MIN` and `MAX`.
They are Improv-only, so they are not counted here. `ABS` and `SQRT` exist
already. With typed errors, domain errors give `#NUM!` (§6.6).

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `ABS` | ABS(x) | absolute value (exists) | EGLNQ | S | P1 |
| `SIGN` | SIGN(x) | −1, 0, 1 | EGLNQ | S | P1 |
| `SQRT` | SQRT(x) | square root (exists) | EGLNQ | S | P1 |
| `SQRTPI` | SQRTPI(x) | √(x·π) | EGLN | S | P1 |
| `EXP` | EXP(x) | eˣ | EGLNQ | S | P1 |
| `EXPM1` | EXPM1(x) | eˣ − 1, accurate near 0 | G | S | P1 |
| `LN` | LN(x) | natural log | EGLNQ | S | P1 |
| `LN1P` | LN1P(x) | ln(1 + x), accurate near 0 | G | S | P1 |
| `LOG` | LOG(x, [base=10]) | log to a base | EGLNQ | S | P1 |
| `LOG10` | LOG10(x) | base-10 log | EGLNQ | S | P1 |
| `LOG2` | LOG2(x) | base-2 log | G | S | P1 |
| `POWER` | POWER(base, exp) | baseᵉˣᵖ, same as `^` | EGLNQ | S | P1 |
| `MOD` | MOD(n, d) | remainder with the divisor's sign | EGLNQ | S | P1 |
| `QUOTIENT` | QUOTIENT(n, d) | integer part of n/d | EGLN | S | P1 |
| `PI` | PI() | π | EGLNQ | S | P1 |
| `DEGREES` | DEGREES(rad) | radians → degrees | EGLNQ | S | P1 |
| `RADIANS` | RADIANS(deg) | degrees → radians | EGLNQ | S | P1 |
| `SIN`, `COS`, `TAN` | SIN(x) | trig, radians | EGLNQ | S | P1 |
| `ASIN`, `ACOS`, `ATAN` | ASIN(x) | inverse trig | EGLNQ | S | P1 |
| `ATAN2` | ATAN2(x, y) | angle of (x, y); spreadsheet argument order | EGLNQ | S | P1 |
| `SINH`, `COSH`, `TANH` | SINH(x) | hyperbolic | EGLNQ | S | P1 |
| `ASINH`, `ACOSH`, `ATANH` | ASINH(x) | inverse hyperbolic | EGLNQ | S | P1 |
| `SEC`, `CSC`, `COT` | SEC(x) | reciprocal trig | EGLN | S | P1 |
| `SECH`, `CSCH`, `COTH` | SECH(x) | reciprocal hyperbolic | EGLN | S | P1 |
| `ACOT`, `ACOTH` | ACOT(x) | inverse cotangent, plain and hyperbolic | EGLN | S | P1 |
| `SINPI`, `COSPI`, `TANPI`, `COTPI` | SINPI(x) | sin(πx) etc., exact at integers | G | S | P1 |
| `HYPOT` | HYPOT(a, b, …) | √(Σ aᵢ²) without overflow | G | S | P1 |
| `FACT` | FACT(n) | n! | EGLNQ | S | P1 |
| `FACTDOUBLE` | FACTDOUBLE(n) | n!! | EGLN | S | P1 |
| `COMBIN` | COMBIN(n, k) | combinations | EGLNQ | S | P1 |
| `COMBINA` | COMBINA(n, k) | combinations with repetition | EGLN | S | P1 |
| `PERMUT` | PERMUT(n, k) | permutations | EGLNQ | S | P1 |
| `PERMUTATIONA` | PERMUTATIONA(n, k) | permutations with repetition | EGLN | S | P1 |
| `GCD` | GCD(n1, …) | greatest common divisor | EGLNQ | S | P1 |
| `LCM` | LCM(n1, …) | least common multiple | EGLNQ | S | P1 |
| `GCD_EXCEL2003`, `LCM_EXCEL2003` | — | LibreOffice compatibility duplicates | L | alias | = |
| `MULTINOMIAL` | MULTINOMIAL(n1, …) | (Σn)! / Πn! | EGLN | S | P1 |
| `RAWSUBTRACT` | RAWSUBTRACT(a, b, …) | a − b − … with no round-off snapping | L | S | P1 |
| `INVSUMINV` | INVSUMINV(x1, …) | 1 / Σ(1/xᵢ) (parallel resistors) | G | S | P1 |
| `SUM` | SUM(a, …) or SUM(x OVER T) | sum (OVER form exists) | EGLNQ | R | P1 |
| `SUMA` | SUMA(a, …) | sum counting TRUE as 1 | G | R | P4 |
| `PRODUCT` | PRODUCT(a, …) or PRODUCT(x OVER T) | product | EGLNQ | R | P1 |
| `G_PRODUCT` | — | Gnumeric spelling of PRODUCT | G | alias | = |
| `SUMSQ` | SUMSQ(a, …) or SUMSQ(x OVER T) | sum of squares | EGLNQ | R | P1 |
| `SUMPRODUCT` | SUMPRODUCT(x, y, … OVER T) | Σ xᵢ·yᵢ·… | EGLNQ | R | P4 |
| `ODF.SUMPRODUCT` | — | Gnumeric's ODF variant of SUMPRODUCT | G | alias | = |
| `SUMX2MY2` | SUMX2MY2(x, y OVER T) | Σ(x² − y²) | EGLNQ | R | P4 |
| `SUMX2PY2` | SUMX2PY2(x, y OVER T) | Σ(x² + y²) | EGLNQ | R | P4 |
| `SUMXMY2` | SUMXMY2(x, y OVER T) | Σ(x − y)² | EGLNQ | R | P4 |
| `SERIESSUM` | SERIESSUM(x, n, m, c OVER T) | Σ cᵢ·x^(n+i·m), coefficients in item order | EGLNQ | P | P4 |
| `POLYNOMIAL` | POLYNOMIAL(x, c OVER T) | polynomial with coefficients in item order | N | P | P4 |
| `PERCENTOF` | PERCENTOF(subset OVER T, all OVER T) | Σsubset / Σall | EG | R | P4 |

### 5.5 Math, special functions <!-- sec:mathsp -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `GAMMA` | GAMMA(x) | Γ(x) | EGLN | S | P4 |
| `GAMMALN` | GAMMALN(x) | ln Γ(x) | EGLNQ | S | P4 |
| `GAMMALN.PRECISE` | — | identical to GAMMALN | EL | alias | = |
| `BETA` | BETA(a, b) | Euler beta | G | S | P4 |
| `BETALN` | BETALN(a, b) | ln B(a, b) | G | S | P4 |
| `DIGAMMA` | DIGAMMA(x) | ψ(x) | G | S | P4 |
| `IGAMMA` | IGAMMA(a, x, [lower], [regularize], [real]) | incomplete gamma | G | S | P4 |
| `POCHHAMMER` | POCHHAMMER(x, n) | Γ(x+n)/Γ(x) | G | S | P4 |
| `LAMBERTW` | LAMBERTW(x, [k]) | Lambert W, branch k | G | S | P7 |
| `AGM` | AGM(a, b) | arithmetic-geometric mean | G | S | P7 |
| `GD` | GD(x) | Gudermannian | G | S | P7 |
| `ILOG` | ILOG(x, base) | integer logarithm | G | S | P7 |
| `REDUCEPI` | REDUCEPI(x, e, [q]) | reduce x modulo π/2ᵉ | G | S | P7 |
| `FIB` | FIB(n) | n-th Fibonacci number | G | S | P7 |

### 5.6 Math, matrices <!-- sec:mathmx -->

In a dimensional model a matrix is a measure over two categories (rows, cols);
results land on the target measure's categories.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `MMULT` | MMULT(a OVER R×K, b OVER K×C) | matrix product: join on K, sum | EGLN | A | P7 |
| `MDETERM`, `LOGMDETERM` | MDETERM(a OVER R×C) | determinant and its log | EGLN | A | P7 |
| `MINVERSE`, `MPSEUDOINVERSE` | MINVERSE(a OVER R×C) | inverse, Moore–Penrose pseudo-inverse | EGLN | A | P7 |
| `CHOLESKY`, `EIGEN`, `LINSOLVE` | LINSOLVE(A, b) | Cholesky factor, symmetric eigen-decomposition, solve Ax=b | G | A | P7 |

### 5.7 Math, number theory & floating point <!-- sec:mathnt -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `ISPRIME`, `ITHPRIME`, `PFACTOR` | ISPRIME(n) | primality, i-th prime, smallest prime factor | G | S | P7 |
| `NT_D`, `NT_SIGMA`, `NT_PHI`, `NT_MU`, `NT_OMEGA`, `NT_PI`, `NT_RADICAL` | NT_PHI(n) | divisor count and sum, Euler φ, Möbius μ, distinct prime count, π(n), radical | G | S | P7 |
| `PARTITIONS` | PARTITIONS(n) | number of integer partitions | G | S | P7 |
| `FLT.MAX`, `FLT.MIN`, `FLT.RADIX` | FLT.MAX() | float constants | G | S | P7 |
| `FLT.NEXTAFTER` | FLT.NEXTAFTER(x, y) | next representable double | G | S | P7 |

### 5.8 Rounding <!-- sec:round -->

All take an optional count or significance, which defaults to the current
1-argument behaviour, so stored formulas keep their meaning. `ROUND` must
round in decimal: LibreOffice gives `ROUND(2.675, 2)` = 2.68, whereas a naive
`(x*100).round()/100` gives 2.67.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `ROUND` | ROUND(x, [digits=0]) | half away from zero (exists, 1-arg) | EGLNQ | S | P1 |
| `ROUNDUP` | ROUNDUP(x, [digits=0]) | away from zero | EGLNQ | S | P1 |
| `ROUNDDOWN` | ROUNDDOWN(x, [digits=0]) | toward zero | EGLNQ | S | P1 |
| `ROUNDSIG` | ROUNDSIG(x, digits) | to significant digits | L | S | P1 |
| `TRUNC` | TRUNC(x, [digits=0]) | truncate | EGLNQ | S | P1 |
| `INT` | INT(x) | floor to an integer | EGLNQ | S | P1 |
| `FLOOR` | FLOOR(x, [significance=1], [mode]) | down to a multiple (exists, 1-arg) | EGLNQ | S | P1 |
| `CEILING` | CEILING(x, [significance=1], [mode]) | up to a multiple | EGLNQ | S | P1 |
| `CEIL` | CEIL(x) | up to an integer (exists) | G | S | P1 |
| `FLOOR.MATH`, `CEILING.MATH` | FLOOR.MATH(x, [sig], [mode]) | sign-aware multiple | EL | S | P1 |
| `FLOOR.PRECISE`, `CEILING.PRECISE`, `ISO.CEILING` | CEILING.PRECISE(x, [sig]) | ignore the sign of sig | EL | S | P1 |
| `FLOOR.XCL`, `CEILING.XCL` | — | LibreOffice's Excel-semantics duplicates of FLOOR/CEILING | L | alias | = |
| `MROUND` | MROUND(x, multiple) | to the nearest multiple | EGLN | S | P1 |
| `EVEN`, `ODD` | EVEN(x) | away from zero to an even/odd integer | EGLNQ | S | P1 |

### 5.9 Statistical, descriptive & conditional <!-- sec:statdesc -->

The `…IF`/`…IFS` and `D…` (§5.21) families become one form,
`F(x OVER T WHERE predicate)`. A literal criterion string like `">10"` is
accepted and compiled into the predicate. Missing cells are skipped.
`COUNTBLANK` needs the category's item set (§7.2).

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `AVERAGE` | AVERAGE(x OVER T) | mean (Improv `AVG` stays an alias) | EGLNQ | R | P4 |
| `AVERAGEA` | AVERAGEA(x OVER T) | mean counting text as 0, TRUE as 1 | EGLNQ | R | P4 |
| `AVERAGEIF` | AVERAGEIF(x OVER T, criterion, [avg_x]) | conditional mean | EGLN | R | P4 |
| `AVERAGEIFS` | AVERAGEIFS(avg_x OVER T, c1, crit1, …) | multi-criteria mean | EGLN | R | P4 |
| `AVEDEV` | AVEDEV(x OVER T) | mean absolute deviation | EGLNQ | R | P4 |
| `COUNT` | COUNT(x OVER T) | count of numbers | EGLNQ | R | P4 |
| `COUNTA` | COUNTA(x OVER T) | count of non-blank | EGLNQ | R | P4 |
| `COUNTBLANK` | COUNTBLANK(x OVER T) | count of items with no cell | EGLNQ | R | P4 |
| `COUNTIF` | COUNTIF(x OVER T, criterion) | conditional count | EGLNQ | R | P4 |
| `COUNTIFS` | COUNTIFS(c1 OVER T, crit1, …) | multi-criteria count | EGLN | R | P4 |
| `SUMIF` | SUMIF(x OVER T, criterion, [sum_x]) | conditional sum | EGLNQ | R | P4 |
| `SUMIFS` | SUMIFS(sum_x OVER T, c1, crit1, …) | multi-criteria sum | EGLN | R | P4 |
| `MAX` | MAX(x OVER T) | maximum (exists) | EGLNQ | R | P4 |
| `MAXA` | MAXA(x OVER T) | max counting booleans | EGLNQ | R | P4 |
| `MAXIFS` | MAXIFS(x OVER T, c1, crit1, …) | conditional max | EGLN | R | P4 |
| `MIN` | MIN(x OVER T) | minimum (exists) | EGLNQ | R | P4 |
| `MINA` | MINA(x OVER T) | min counting booleans | EGLNQ | R | P4 |
| `MINIFS` | MINIFS(x OVER T, c1, crit1, …) | conditional min | EGLN | R | P4 |
| `MEDIAN` | MEDIAN(x OVER T) | median | EGLNQ | R | P4 |
| `MODE` | MODE(x OVER T) | most frequent value | EGLNQ | R | P4 |
| `MODE.SNGL` | — | identical to MODE | EL | alias | = |
| `MODE.MULT` | MODE.MULT(x OVER T) | all modes (array) | EGL | A | P6 |
| `GEOMEAN` | GEOMEAN(x OVER T) | geometric mean | EGLNQ | R | P4 |
| `HARMEAN` | HARMEAN(x OVER T) | harmonic mean | EGLNQ | R | P4 |
| `TRIMMEAN` | TRIMMEAN(x OVER T, fraction) | mean excluding tails | EGLQ | R | P4 |
| `DEVSQ` | DEVSQ(x OVER T) | Σ(x − mean)² | EGLNQ | R | P4 |
| `STDEV` | STDEV(x OVER T) | sample standard deviation | EGLNQ | R | P4 |
| `STDEV.S` | — | identical to STDEV | EL | alias | = |
| `STDEVP` | STDEVP(x OVER T) | population standard deviation | EGLNQ | R | P4 |
| `STDEV.P` | — | identical to STDEVP | EL | alias | = |
| `STDEVA`, `STDEVPA` | STDEVA(x OVER T) | standard deviation counting text/booleans | EGLNQ | R | P4 |
| `VAR` | VAR(x OVER T) | sample variance | EGLNQ | R | P4 |
| `VAR.S` | — | identical to VAR | EL | alias | = |
| `VARP` | VARP(x OVER T) | population variance | EGLNQ | R | P4 |
| `VAR.P` | — | identical to VARP | EL | alias | = |
| `VARA`, `VARPA` | VARA(x OVER T) | variance counting text/booleans | EGLNQ | R | P4 |
| `KURT` | KURT(x OVER T) | sample excess kurtosis | EGLQ | R | P4 |
| `KURTP` | KURTP(x OVER T) | population kurtosis | G | R | P4 |
| `SKEW` | SKEW(x OVER T) | sample skewness | EGLQ | R | P4 |
| `SKEWP` | SKEWP(x OVER T) | population skewness | GL | R | P4 |
| `SKEW.P` | — | Excel spelling of SKEWP | E | alias | = |
| `LARGE` | LARGE(x OVER T, k) | k-th largest | EGLNQ | R | P4 |
| `SMALL` | SMALL(x OVER T, k) | k-th smallest | EGLNQ | R | P4 |
| `PERCENTILE` | PERCENTILE(x OVER T, p) | inclusive percentile | EGLNQ | R | P4 |
| `PERCENTILE.INC` | — | identical to PERCENTILE | EL | alias | = |
| `PERCENTILE.EXC` | PERCENTILE.EXC(x OVER T, p) | exclusive percentile | EGL | R | P4 |
| `QUARTILE` | QUARTILE(x OVER T, q) | inclusive quartile | EGLNQ | R | P4 |
| `QUARTILE.INC` | — | identical to QUARTILE | EL | alias | = |
| `QUARTILE.EXC` | QUARTILE.EXC(x OVER T, q) | exclusive quartile | EGL | R | P4 |
| `PERCENTRANK` | PERCENTRANK(x OVER T, [v=x], [digits]) | rank as a fraction; per item when v omitted | EGLNQ | R | P4 |
| `PERCENTRANK.INC` | — | identical to PERCENTRANK | EL | alias | = |
| `PERCENTRANK.EXC` | PERCENTRANK.EXC(x OVER T, [v], [digits]) | exclusive rank fraction | EGL | R | P4 |
| `RANK` | RANK(x OVER T, [order]) | each item's rank among its siblings | EGLNQ | R | P4 |
| `RANK.EQ` | — | identical to RANK | EL | alias | = |
| `RANK.AVG` | RANK.AVG(x OVER T, [order]) | rank, ties averaged | EGL | R | P4 |
| `FREQUENCY` | FREQUENCY(x OVER T, bins OVER B) | counts per bin, onto bin category B | EGLN | A | P4 |
| `STANDARDIZE` | STANDARDIZE(x, mean, sd) | z-score | EGLNQ | S | P4 |
| `SUBTOTAL` | SUBTOTAL(code, x OVER T) | aggregate chosen by number (1–11, 101–111) | EGLN | R | P4 |
| `AGGREGATE` | AGGREGATE(code, options, x OVER T, [k]) | SUBTOTAL plus error-skipping options | EL | R | P4 |
| `SSMEDIAN` | SSMEDIAN(x OVER T, [interval]) | median for grouped data | G | R | P7 |
| `PROB` | PROB(x OVER T, p OVER T, low, [high]) | probability mass in [low, high] | EGLNQ | R | P4 |

### 5.10 Statistical, distributions <!-- sec:statdist -->

Each is a pure scalar. Legacy spellings with a subset signature become aliases
with argument mapping: `NORMSDIST(z)` = `NORM.S.DIST(z, TRUE)`; `TDIST(x, df,
tails)` is `T.DIST.RT` or `T.DIST.2T` depending on `tails`.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `BETA.DIST` | BETA.DIST(x, α, β, cumulative, [A], [B]) | beta distribution | EGL | S | P4 |
| `BETADIST` | BETADIST(x, α, β, [A], [B]) | = BETA.DIST(…, TRUE, …) | EGLNQ | alias | = |
| `BETAINV` | BETAINV(p, α, β, [A], [B]) | beta quantile | EGLNQ | S | P4 |
| `BETA.INV` | — | identical to BETAINV | EL | alias | = |
| `BINOMDIST` | BINOMDIST(k, n, p, cumulative) | binomial | EGLNQ | S | P4 |
| `BINOM.DIST` | — | identical to BINOMDIST | EL | alias | = |
| `CRITBINOM` | CRITBINOM(n, p, α) | smallest k with CDF ≥ α | EGLNQ | S | P4 |
| `BINOM.INV` | — | identical to CRITBINOM | EL | alias | = |
| `BINOM.DIST.RANGE` | BINOM.DIST.RANGE(n, p, k1, [k2]) | P(k1 ≤ K ≤ k2) | EG | S | P4 |
| `B` | B(n, p, k1, [k2]) | LibreOffice spelling of BINOM.DIST.RANGE | L | alias | = |
| `CHISQ.DIST` | CHISQ.DIST(x, df, cumulative) | chi-square, left tail | EL | S | P4 |
| `CHISQDIST` | — | LibreOffice spelling of CHISQ.DIST | L | alias | = |
| `CHIDIST` | CHIDIST(x, df) | chi-square right tail | EGLNQ | S | P4 |
| `CHISQ.DIST.RT` | — | identical to CHIDIST | EL | alias | = |
| `CHISQ.INV` | CHISQ.INV(p, df) | left-tail quantile | EL | S | P4 |
| `CHISQINV` | — | LibreOffice spelling of CHISQ.INV | L | alias | = |
| `CHIINV` | CHIINV(p, df) | right-tail quantile | EGLNQ | S | P4 |
| `CHISQ.INV.RT` | — | identical to CHIINV | EL | alias | = |
| `EXPONDIST` | EXPONDIST(x, λ, cumulative) | exponential | EGLNQ | S | P4 |
| `EXPON.DIST` | — | identical to EXPONDIST | EL | alias | = |
| `F.DIST` | F.DIST(x, d1, d2, cumulative) | F, left tail | EL | S | P4 |
| `FDIST` | FDIST(x, d1, d2) | F right tail | EGLNQ | S | P4 |
| `F.DIST.RT` | — | identical to FDIST | EL | alias | = |
| `F.INV` | F.INV(p, d1, d2) | F left-tail quantile | EL | S | P4 |
| `FINV` | FINV(p, d1, d2) | F right-tail quantile | EGLNQ | S | P4 |
| `F.INV.RT` | — | identical to FINV | EL | alias | = |
| `GAMMADIST` | GAMMADIST(x, α, β, cumulative) | gamma | EGLNQ | S | P4 |
| `GAMMA.DIST` | — | identical to GAMMADIST | EL | alias | = |
| `GAMMAINV` | GAMMAINV(p, α, β) | gamma quantile | EGLNQ | S | P4 |
| `GAMMA.INV` | — | identical to GAMMAINV | EL | alias | = |
| `GAUSS` | GAUSS(z) | Φ(z) − 0.5 | EGL | S | P4 |
| `HYPGEOM.DIST` | HYPGEOM.DIST(k, n, K, N, cumulative) | hypergeometric | EL | S | P4 |
| `HYPGEOMDIST` | HYPGEOMDIST(k, n, K, N) | = HYPGEOM.DIST(…, FALSE) | EGLQ | alias | = |
| `LOGNORM.DIST` | LOGNORM.DIST(x, μ, σ, cumulative) | log-normal | EL | S | P4 |
| `LOGNORMDIST` | LOGNORMDIST(x, μ, σ) | = LOGNORM.DIST(…, TRUE) | EGLNQ | alias | = |
| `LOGINV` | LOGINV(p, μ, σ) | log-normal quantile | EGLNQ | S | P4 |
| `LOGNORM.INV` | — | identical to LOGINV | EL | alias | = |
| `NEGBINOM.DIST` | NEGBINOM.DIST(f, s, p, cumulative) | negative binomial | EL | S | P4 |
| `NEGBINOMDIST` | NEGBINOMDIST(f, s, p) | = NEGBINOM.DIST(…, FALSE) | EGLNQ | alias | = |
| `NORMDIST` | NORMDIST(x, μ, σ, cumulative) | normal | EGLNQ | S | P4 |
| `NORM.DIST` | — | identical to NORMDIST | EL | alias | = |
| `NORMINV` | NORMINV(p, μ, σ) | normal quantile | EGLNQ | S | P4 |
| `NORM.INV` | — | identical to NORMINV | EL | alias | = |
| `NORM.S.DIST` | NORM.S.DIST(z, cumulative) | standard normal | EL | S | P4 |
| `NORMSDIST` | NORMSDIST(z) | = NORM.S.DIST(z, TRUE) | EGLNQ | alias | = |
| `NORMSINV` | NORMSINV(p) | standard normal quantile | EGLNQ | S | P4 |
| `NORM.S.INV` | — | identical to NORMSINV | EL | alias | = |
| `PHI` | PHI(x) | standard normal density | EGLN | S | P4 |
| `POISSON` | POISSON(k, λ, cumulative) | Poisson | EGLNQ | S | P4 |
| `POISSON.DIST` | — | identical to POISSON | EL | alias | = |
| `T.DIST` | T.DIST(x, df, cumulative) | Student t, left tail | EL | S | P4 |
| `T.DIST.RT` | T.DIST.RT(x, df) | t right tail | EL | S | P4 |
| `T.DIST.2T` | T.DIST.2T(x, df) | t two-tailed | EL | S | P4 |
| `TDIST` | TDIST(x, df, tails) | T.DIST.RT or T.DIST.2T by `tails` | EGLNQ | alias | = |
| `T.INV` | T.INV(p, df) | t left-tail quantile | EL | S | P4 |
| `TINV` | TINV(p, df) | t two-tailed quantile | EGLNQ | S | P4 |
| `T.INV.2T` | — | identical to TINV | EL | alias | = |
| `WEIBULL` | WEIBULL(x, α, β, cumulative) | Weibull | EGLNQ | S | P4 |
| `WEIBULL.DIST` | — | identical to WEIBULL | EL | alias | = |
| `FISHER`, `FISHERINV` | FISHER(x) | Fisher z-transform and inverse | EGLNQ | S | P4 |
| `CONFIDENCE` | CONFIDENCE(α, σ, n) | normal confidence half-width | EGLNQ | S | P4 |
| `CONFIDENCE.NORM` | — | identical to CONFIDENCE | EL | alias | = |
| `CONFIDENCE.T` | CONFIDENCE.T(α, σ, n) | t confidence half-width | EGL | S | P4 |
| `BERNOULLI` | BERNOULLI(k, p) | Bernoulli pmf | G | S | P7 |
| `CAUCHY` | CAUCHY(x, a, cumulative) | Cauchy | G | S | P7 |
| `EXPPOWDIST` | EXPPOWDIST(x, a, b) | exponential-power density | G | S | P7 |
| `GEOMDIST` | GEOMDIST(k, p, cumulative) | geometric | G | S | P7 |
| `LANDAU` | LANDAU(x) | Landau density | G | S | P7 |
| `LAPLACE` | LAPLACE(x, a) | Laplace density | G | S | P7 |
| `LOGISTIC` | LOGISTIC(x, a) | logistic density | G | S | P7 |
| `PARETO` | PARETO(x, a, b) | Pareto density | G | S | P7 |
| `RAYLEIGH`, `RAYLEIGHTAIL` | RAYLEIGH(x, σ) | Rayleigh density, and its tail | G | S | P7 |
| `SNORM.DIST.RANGE` | SNORM.DIST.RANGE(x1, x2) | Φ(x2) − Φ(x1) | G | S | P7 |
| `OWENT` | OWENT(h, a) | Owen's T | G | S | P7 |
| `CUM_BIV_NORM_DIST` | CUM_BIV_NORM_DIST(a, b, ρ) | bivariate normal CDF | G | S | P7 |

### 5.11 Statistical, R-style distribution kernels (Gnumeric) <!-- sec:statr -->

`R.D*` (density), `R.P*` (CDF) and `R.Q*` (quantile), with R's `lower_tail`
and `log_p` flags. They reuse the P4 kernels, so the added cost is the flags.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `R.DBETA`, `R.PBETA`, `R.QBETA` | R.PBETA(x, a, b, [lower_tail], [log_p]) | beta | G | S | P7 |
| `R.DBINOM`, `R.PBINOM`, `R.QBINOM` | R.PBINOM(x, n, p, …) | binomial | G | S | P7 |
| `R.DCAUCHY`, `R.PCAUCHY`, `R.QCAUCHY` | R.PCAUCHY(x, location, scale, …) | Cauchy | G | S | P7 |
| `R.DCHISQ`, `R.PCHISQ`, `R.QCHISQ` | R.PCHISQ(x, df, …) | chi-square | G | S | P7 |
| `R.DEXP`, `R.PEXP`, `R.QEXP` | R.PEXP(x, scale, …) | exponential | G | S | P7 |
| `R.DF`, `R.PF`, `R.QF` | R.PF(x, n1, n2, …) | F | G | S | P7 |
| `R.DGAMMA`, `R.PGAMMA`, `R.QGAMMA` | R.PGAMMA(x, shape, scale, …) | gamma | G | S | P7 |
| `R.DGEOM`, `R.PGEOM`, `R.QGEOM` | R.PGEOM(x, p, …) | geometric | G | S | P7 |
| `R.DGUMBEL`, `R.PGUMBEL`, `R.QGUMBEL` | R.PGUMBEL(x, μ, β, …) | Gumbel | G | S | P7 |
| `R.DHYPER`, `R.PHYPER`, `R.QHYPER` | R.PHYPER(x, r, b, n, …) | hypergeometric | G | S | P7 |
| `R.DLNORM`, `R.PLNORM`, `R.QLNORM` | R.PLNORM(x, logmean, logsd, …) | log-normal | G | S | P7 |
| `R.DNBINOM`, `R.PNBINOM`, `R.QNBINOM` | R.PNBINOM(x, n, p, …) | negative binomial | G | S | P7 |
| `R.DNORM`, `R.PNORM`, `R.QNORM` | R.PNORM(x, μ, σ, …) | normal | G | S | P7 |
| `R.DPOIS`, `R.PPOIS`, `R.QPOIS` | R.PPOIS(x, λ, …) | Poisson | G | S | P7 |
| `R.DRAYLEIGH`, `R.PRAYLEIGH`, `R.QRAYLEIGH` | R.PRAYLEIGH(x, scale, …) | Rayleigh | G | S | P7 |
| `R.DSNORM`, `R.PSNORM`, `R.QSNORM` | R.PSNORM(x, shape, location, scale, …) | skew-normal | G | S | P7 |
| `R.DST`, `R.PST`, `R.QST` | R.PST(x, n, shape, …) | skew-t | G | S | P7 |
| `R.DT`, `R.PT`, `R.QT` | R.PT(x, n, …) | Student t | G | S | P7 |
| `R.DWEIBULL`, `R.PWEIBULL`, `R.QWEIBULL` | R.PWEIBULL(x, shape, scale, …) | Weibull | G | S | P7 |
| `R.PTUKEY`, `R.QTUKEY` | R.PTUKEY(x, nmeans, df, [nranges], …) | Studentized range | G | S | P7 |

### 5.12 Statistical, regression & forecasting <!-- sec:statreg -->

Two-measure reductions: `x` and `y` are joined on their full coordinate, then
reduced over `T`. Forecasts and trends need item order (P).

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `CORREL` | CORREL(x, y OVER T) | Pearson correlation | EGLNQ | R | P4 |
| `PEARSON` | — | identical to CORREL | EGLNQ | alias | = |
| `RSQ` | RSQ(y, x OVER T) | r² | EGLNQ | R | P4 |
| `COVAR` | COVAR(x, y OVER T) | population covariance | EGLNQ | R | P4 |
| `COVARIANCE.P` | — | identical to COVAR | EL | alias | = |
| `COVARIANCE.S` | COVARIANCE.S(x, y OVER T) | sample covariance | EGL | R | P4 |
| `SLOPE` | SLOPE(y, x OVER T) | least-squares slope | EGLNQ | R | P4 |
| `INTERCEPT` | INTERCEPT(y, x OVER T) | least-squares intercept | EGLNQ | R | P4 |
| `STEYX` | STEYX(y, x OVER T) | standard error of predicted y | EGLQ | R | P4 |
| `FORECAST` | FORECAST(x0, y, x OVER T) | linear prediction at x0 | EGLNQ | R | P4 |
| `FORECAST.LINEAR` | — | identical to FORECAST | EL | alias | = |
| `TREND` | TREND(y, x OVER T, [new_x], [const]) | fitted line values per item | EGL | P | P4 |
| `GROWTH` | GROWTH(y, x OVER T, [new_x], [const]) | fitted exponential per item | EGL | P | P4 |
| `LINEST` | LINEST(y, x OVER T, [const], [stats]) | regression coefficients (array) | EGLN | A | P4 |
| `LOGEST` | LOGEST(y, x OVER T, [const], [stats]) | exponential-fit coefficients | EGL | A | P4 |
| `EXPA`, `EXPB` | EXPA(y, x OVER T, [simple]) | Quantrix: A and B of y = A·e^(Bx) | Q | R | P4 |
| `EXPFORECAST` | EXPFORECAST(x0, y, x OVER T, [simple]) | Quantrix: exponential prediction | Q | R | P4 |
| `LOGFIT` | LOGFIT(y, x OVER T) | logarithmic least-squares fit | G | A | P7 |
| `LOGREG` | LOGREG(y, x OVER T, [affine], [stats]) | logarithmic regression | G | A | P7 |
| `LEVERAGE` | LEVERAGE(A) | regression leverage | G | A | P7 |
| `CRONBACH` | CRONBACH(x1, x2, … OVER T) | Cronbach's α | G | R | P7 |
| `FORECAST.ETS.ADD`, `FORECAST.ETS.MULT` | FORECAST.ETS.ADD(target, y OVER T, …) | exponential-smoothing forecast | L | P | P7 |
| `FORECAST.ETS.PI.ADD`, `FORECAST.ETS.PI.MULT` | FORECAST.ETS.PI.ADD(…) | prediction interval | L | P | P7 |
| `FORECAST.ETS.STAT.ADD`, `FORECAST.ETS.STAT.MULT` | FORECAST.ETS.STAT.ADD(…) | model statistics | L | P | P7 |
| `FORECAST.ETS.SEASONALITY` | FORECAST.ETS.SEASONALITY(y OVER T, …) | detected season length | EL | P | P7 |
| `FORECAST.ETS` | — | Excel spelling of FORECAST.ETS.ADD (from knowledge, unverified) | E | alias | = |
| `FORECAST.ETS.CONFINT` | — | Excel spelling of FORECAST.ETS.PI.ADD (from knowledge, unverified) | E | alias | = |
| `FORECAST.ETS.STAT` | — | Excel spelling of FORECAST.ETS.STAT.ADD (from knowledge, unverified) | E | alias | = |
| `INTERPOLATION` | INTERPOLATION(xs, ys, targets, [method]) | interpolated values | G | P | P7 |
| `PERIODOGRAM` | PERIODOGRAM(y OVER T, …) | periodogram | G | P | P7 |
| `FOURIER` | FOURIER(y OVER T, [inverse], [separate]) | discrete Fourier transform | GL | P | P7 |
| `HPFILTER` | HPFILTER(y OVER T, λ) | Hodrick–Prescott filter | G | P | P7 |

### 5.13 Statistical, tests <!-- sec:stattest -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `CHITEST` | CHITEST(actual, expected OVER T) | χ² test p-value | EGLNQ | R | P4 |
| `CHISQ.TEST` | — | identical to CHITEST | EL | alias | = |
| `FTEST` | FTEST(a, b OVER T) | F-test p-value | EGLQ | R | P4 |
| `F.TEST` | — | identical to FTEST | EL | alias | = |
| `TTEST` | TTEST(a, b OVER T, tails, type) | t-test p-value | EGLNQ | R | P4 |
| `T.TEST` | — | identical to TTEST | EL | alias | = |
| `ZTEST` | ZTEST(x OVER T, μ0, [σ]) | z-test p-value | EGLNQ | R | P4 |
| `Z.TEST` | — | identical to ZTEST | EL | alias | = |
| `ADTEST`, `CVMTEST`, `LKSTEST`, `SFTEST` | ADTEST(x OVER T) | normality tests: Anderson–Darling, Cramér–von Mises, Lilliefors, Shapiro–Francia | G | R | P7 |

### 5.14 Financial, time value of money <!-- sec:fintvm -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `FV` | FV(rate, nper, pmt, [pv], [type]) | future value | EGLNQ | S | P5 |
| `PV` | PV(rate, nper, pmt, [fv], [type]) | present value | EGLNQ | S | P5 |
| `PMT` | PMT(rate, nper, pv, [fv], [type]) | payment | EGLNQ | S | P5 |
| `IPMT` | IPMT(rate, per, nper, pv, [fv], [type]) | interest part of a payment | EGLNQ | S | P5 |
| `PPMT` | PPMT(rate, per, nper, pv, [fv], [type]) | principal part | EGLNQ | S | P5 |
| `NPER` | NPER(rate, pmt, pv, [fv], [type]) | number of periods | EGLNQ | S | P5 |
| `RATE` | RATE(nper, pmt, pv, [fv], [type], [guess]) | rate per period (iterative) | EGLNQ | S | P5 |
| `CUMIPMT` | CUMIPMT(rate, nper, pv, start, end, type) | cumulative interest | EGLN | S | P5 |
| `CUMPRINC` | CUMPRINC(rate, nper, pv, start, end, type) | cumulative principal | EGLN | S | P5 |
| `CUMIPMT_ADD`, `CUMPRINC_ADD` | — | LibreOffice add-in duplicates | L | alias | = |
| `ISPMT` | ISPMT(rate, per, nper, pv) | interest, straight-line principal | EGLN | S | P5 |
| `EFFECT` | EFFECT(nominal, npery) | effective annual rate | EGLN | S | P5 |
| `NOMINAL` | NOMINAL(effect, npery) | nominal annual rate | EGLN | S | P5 |
| `EFFECT_ADD`, `NOMINAL_ADD` | — | LibreOffice add-in duplicates | L | alias | = |
| `FVSCHEDULE` | FVSCHEDULE(principal, rate OVER T) | compound over a rate schedule | EGL | R | P5 |
| `PDURATION` | PDURATION(rate, pv, fv) | periods to grow pv to fv | EL | S | P5 |
| `G_DURATION` | — | Gnumeric spelling of PDURATION | G | alias | = |
| `RRI` | RRI(nper, pv, fv) | equivalent growth rate | EGL | S | P5 |
| `DOLLARDE`, `DOLLARFR` | DOLLARDE(price, fraction) | fractional ↔ decimal dollar prices | EGL | S | P5 |

### 5.15 Financial, depreciation <!-- sec:findep -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `SLN` | SLN(cost, salvage, life) | straight line | EGLNQ | S | P5 |
| `SYD` | SYD(cost, salvage, life, per) | sum of years' digits | EGLNQ | S | P5 |
| `DB` | DB(cost, salvage, life, period, [month]) | fixed declining balance | EGLNQ | S | P5 |
| `DDB` | DDB(cost, salvage, life, period, [factor=2]) | double declining balance | EGLNQ | S | P5 |
| `VDB` | VDB(cost, salvage, life, start, end, [factor], [no_switch]) | variable declining balance | EGLNQ | S | P5 |
| `AMORDEGRC` | AMORDEGRC(cost, purchased, first, salvage, period, rate, [basis]) | French degressive | EGL | S | P5 |
| `AMORLINC` | AMORLINC(cost, purchased, first, salvage, period, rate, [basis]) | French linear | EGL | S | P5 |

### 5.16 Financial, cash flow <!-- sec:fincf -->

The cash flows are a measure over an **ordered** category: `NPV(rate, cf OVER
Time)` discounts in item order. `XNPV`/`XIRR` take a date measure on the same
category. `IRR` and `XIRR` iterate: Newton's method with bisection fallback,
fixed iteration cap, deterministic start.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `NPV` | NPV(rate, cf OVER T) | net present value, item order | EGLNQ | P | P5 |
| `XNPV` | XNPV(rate, cf, dates OVER T) | NPV at irregular dates | EGLNQ | R | P5 |
| `IRR` | IRR(cf OVER T, [guess]) | internal rate of return | EGLNQ | P | P5 |
| `XIRR` | XIRR(cf, dates OVER T, [guess]) | IRR at irregular dates | EGLNQ | R | P5 |
| `MIRR` | MIRR(cf OVER T, finance_rate, reinvest_rate) | modified IRR | EGLNQ | P | P5 |

### 5.17 Financial, bonds & securities <!-- sec:finbond -->

All take day-count `basis` (0 US 30/360, 1 actual/actual, 2 actual/360, 3
actual/365, 4 European 30/360), shared with `YEARFRAC` and `DAYS360`.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `ACCRINT` | ACCRINT(issue, first, settle, rate, par, freq, [basis], [method]) | accrued interest, periodic | EGLN | S | P5 |
| `ACCRINTM` | ACCRINTM(issue, settle, rate, [par], [basis]) | accrued interest at maturity | EGLN | S | P5 |
| `COUPDAYBS` | COUPDAYBS(settle, maturity, freq, [basis]) | days from coupon start to settlement | EGLN | S | P5 |
| `COUPDAYS` | COUPDAYS(settle, maturity, freq, [basis]) | days in the coupon period | EGLN | S | P5 |
| `COUPDAYSNC` | COUPDAYSNC(settle, maturity, freq, [basis]) | days to the next coupon | EGLN | S | P5 |
| `COUPNCD` | COUPNCD(settle, maturity, freq, [basis]) | next coupon date | EGL | S | P5 |
| `COUPNUM` | COUPNUM(settle, maturity, freq, [basis]) | coupons remaining | EGLN | S | P5 |
| `COUPPCD` | COUPPCD(settle, maturity, freq, [basis]) | previous coupon date | EGL | S | P5 |
| `DISC` | DISC(settle, maturity, price, redemption, [basis]) | discount rate | EGLN | S | P5 |
| `DURATION` | DURATION(settle, maturity, coupon, yld, freq, [basis]) | Macaulay duration (Numbers' `DURATION` differs, App. A) | EGLN | S | P5 |
| `BONDDURATION` | — | Numbers spelling of DURATION | N | alias | = |
| `MDURATION` | MDURATION(settle, maturity, coupon, yld, freq, [basis]) | modified duration | EGL | S | P5 |
| `BONDMDURATION` | — | Numbers spelling of MDURATION | N | alias | = |
| `INTRATE` | INTRATE(settle, maturity, investment, redemption, [basis]) | interest rate of a fully invested security | EGLN | S | P5 |
| `ODDFPRICE`, `ODDFYIELD` | ODDFPRICE(settle, maturity, issue, first, rate, yld, redemption, freq, [basis]) | odd first period price/yield | EGL | S | P5 |
| `ODDLPRICE`, `ODDLYIELD` | ODDLPRICE(settle, maturity, last, rate, yld, redemption, freq, [basis]) | odd last period price/yield | EGL | S | P5 |
| `PRICE` | PRICE(settle, maturity, rate, yld, redemption, freq, [basis]) | price per 100 face | EGLN | S | P5 |
| `PRICEDISC` | PRICEDISC(settle, maturity, discount, redemption, [basis]) | price of a discounted security | EGLN | S | P5 |
| `PRICEMAT` | PRICEMAT(settle, maturity, issue, rate, yld, [basis]) | price, interest at maturity | EGLN | S | P5 |
| `RECEIVED` | RECEIVED(settle, maturity, investment, discount, [basis]) | amount at maturity | EGLN | S | P5 |
| `TBILLEQ`, `TBILLPRICE`, `TBILLYIELD` | TBILLPRICE(settle, maturity, discount) | Treasury-bill equivalents | EGL | S | P5 |
| `YIELD` | YIELD(settle, maturity, rate, price, redemption, freq, [basis]) | yield (iterative) | EGLN | S | P5 |
| `YIELDDISC` | YIELDDISC(settle, maturity, price, redemption, [basis]) | yield of a discounted security | EGLN | S | P5 |
| `YIELDMAT` | YIELDMAT(settle, maturity, issue, rate, price, [basis]) | yield, interest at maturity | EGLN | S | P5 |

### 5.18 Financial, option pricing (Gnumeric, LibreOffice) <!-- sec:finopt -->

Closed-form or lattice pricing models. They are pure, but niche.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `OPT_BS`, `OPT_BS_DELTA`, `OPT_BS_GAMMA`, `OPT_BS_RHO`, `OPT_BS_THETA`, `OPT_BS_VEGA`, `OPT_BS_CARRYCOST` | OPT_BS(call_put, spot, strike, time, rate, volatility, [cost_of_carry]) | Black–Scholes price and Greeks | G | S | P7 |
| `OPT_GARMAN_KOHLHAGEN`, `OPT_FRENCH`, `OPT_JUMP_DIFF`, `OPT_MILTERSEN_SCHWARTZ` | (model-specific) | European variants: FX, trading-day volatility, jump diffusion, commodity futures | G | S | P7 |
| `OPT_BAW_AMER`, `OPT_BJER_STENS`, `OPT_RGW`, `OPT_BINOMIAL` | (model-specific) | American approximations and binomial tree | G | S | P7 |
| `OPT_EXEC`, `OPT_FORWARD_START`, `OPT_TIME_SWITCH`, `OPT_SIMPLE_CHOOSER`, `OPT_COMPLEX_CHOOSER`, `OPT_ON_OPTIONS`, `OPT_EXTENDIBLE_WRITER` | (model-specific) | exotic options | G | S | P7 |
| `OPT_2_ASSET_CORRELATION`, `OPT_EURO_EXCHANGE`, `OPT_AMER_EXCHANGE`, `OPT_SPREAD_APPROX` | (model-specific) | two-asset options | G | S | P7 |
| `OPT_FIXED_STRK_LKBK`, `OPT_FLOAT_STRK_LKBK` | (model-specific) | lookback options | G | S | P7 |
| `OPT_BARRIER`, `OPT_TOUCH`, `OPT_PROB_HIT`, `OPT_PROB_INMONEY` | OPT_BARRIER(spot, vol, r, rf, T, strike, lower, upper, rebate, put_call, knock, [greek]) | barrier/touch prices and probabilities (LibreOffice) | L | S | P7 |

### 5.19 Date & time <!-- sec:date -->

Dates are `DateTime` values (question 4). `NOW()`/`TODAY()` read the model's
**as-of instant**, a model parameter set by the host, so evaluation stays
reproducible (§6.9). Durations become Number days unless a Duration type is
added; Numbers' `DUR2*` then reduce to arithmetic.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `DATE` | DATE(y, m, d) | a date; out-of-range months and days roll over | EGLNQ | S | P2 |
| `DATEVALUE` | DATEVALUE(text) | parse a date (ISO first; locale per question 6) | EGLNQ | S | P2 |
| `TIME` | TIME(h, m, s) | a time of day (fraction of a day) | EGLNQ | S | P2 |
| `ODF.TIME` | — | Gnumeric's ODF variant of TIME | G | alias | = |
| `TIMEVALUE` | TIMEVALUE(text) | parse a time | EGLNQ | S | P2 |
| `YEAR`, `MONTH`, `DAY` | YEAR(d) | date parts | EGLNQ | S | P2 |
| `HOUR`, `MINUTE`, `SECOND` | HOUR(d) | time parts | EGLNQ | S | P2 |
| `WEEKDAY` | WEEKDAY(d, [type=1]) | day of week | EGLNQ | S | P2 |
| `WEEKNUM` | WEEKNUM(d, [type=1]) | week of year | EGLN | S | P2 |
| `WEEKNUM_EXCEL2003`, `WEEKNUM_OOO` | — | LibreOffice compatibility duplicates | L | alias | = |
| `ISOWEEKNUM` | ISOWEEKNUM(d) | ISO 8601 week | EGLN | S | P2 |
| `ISOYEAR` | ISOYEAR(d) | ISO 8601 week-year | G | S | P2 |
| `DAYOFYEAR` | DAYOFYEAR(d) | 1–366 | Q | S | P2 |
| `DAYNAME`, `MONTHNAME` | DAYNAME(n) | English day/month name | N | S | P2 |
| `DAYS` | DAYS(end, start) | days between | EGLN | S | P2 |
| `DAYS360` | DAYS360(start, end, [european]) | 30/360 days between | EGLNQ | S | P2 |
| `DATEDIF` | DATEDIF(start, end, unit) | difference in "y", "m", "d", "ym", "yd", "md" | EGLNQ | S | P2 |
| `EDATE` | EDATE(d, months) | same day n months later | EGLNQ | S | P2 |
| `EOMONTH` | EOMONTH(d, months) | end of month n months later | EGLNQ | S | P2 |
| `YEARFRAC` | YEARFRAC(start, end, [basis]) | fraction of a year | EGLN | S | P2 |
| `NETWORKDAYS` | NETWORKDAYS(start, end, [holidays OVER H]) | working days | EGLN | S | P2 |
| `NETWORKDAYS.INTL` | NETWORKDAYS.INTL(start, end, [weekend], [holidays]) | with a weekend mask | EGL | S | P2 |
| `NETWORKDAYS_EXCEL2003` | — | LibreOffice compatibility duplicate | L | alias | = |
| `WORKDAY` | WORKDAY(start, days, [holidays]) | date n working days on | EGLN | S | P2 |
| `WORKDAY.INTL` | WORKDAY.INTL(start, days, [weekend], [holidays]) | with a weekend mask | EGL | S | P2 |
| `NOW`, `TODAY` | NOW() | the model's as-of instant / date | EGLNQ | S | P2 |
| `DAYSINMONTH`, `DAYSINYEAR`, `WEEKSINYEAR` | DAYSINMONTH(d) | calendar sizes | L | S | P2 |
| `ISLEAPYEAR` | ISLEAPYEAR(d) | leap year | L | S | P2 |
| `MONTHS`, `WEEKS`, `YEARS` | MONTHS(start, end, type) | whole periods between | L | S | P2 |
| `UNIX2DATE`, `DATE2UNIX` | UNIX2DATE(t) | Unix seconds ↔ date | G | S | P2 |
| `DUR2DAYS`, `DUR2HOURS`, `DUR2MINUTES`, `DUR2SECONDS`, `DUR2MILLISECONDS`, `DUR2WEEKS` | DUR2HOURS(duration) | a duration in a unit | N | S | P2 |
| `STRIPDURATION` | STRIPDURATION(v) | duration → days, otherwise v | N | S | P2 |

### 5.20 Date & time, calendars (Gnumeric) <!-- sec:datecal -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `EASTERSUNDAY`, `ASCENSIONTHURSDAY`, `ASHWEDNESDAY`, `GOODFRIDAY`, `PENTECOSTSUNDAY` | EASTERSUNDAY(year) | Gregorian movable feasts | GL | S | P7 |
| `DATE2JULIAN`, `HDATE_JULIAN` | DATE2JULIAN(d) | Julian day number | G | S | P7 |
| `HDATE`, `HDATE_HEB`, `HDATE_DAY`, `HDATE_MONTH`, `HDATE_YEAR`, `DATE2HDATE`, `DATE2HDATE_HEB` | HDATE(y, m, d) | Hebrew calendar | G | S | P7 |

### 5.21 Text <!-- sec:text -->

Text is UTF-8. Lengths and positions count Unicode scalar values, so the DBCS
"B" byte variants become aliases. `CHAR`/`CODE` follow `UNICHAR`/`UNICODE`,
not a code page.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `CHAR` | CHAR(n) | character n | EGLNQ | S | P1 |
| `UNICHAR` | UNICHAR(n) | Unicode character n | EGL | S | P1 |
| `CODE` | CODE(t) | code of the first character | EGLNQ | S | P1 |
| `UNICODE` | UNICODE(t) | code point of the first character | EGL | S | P1 |
| `CLEAN` | CLEAN(t) | strip non-printing characters | EGLNQ | S | P1 |
| `CONCAT` | CONCAT(t1, …) | join (also as `&`) | EGLN | S | P1 |
| `CONCATENATE` | CONCATENATE(t1, …) | join | EGLNQ | S | P1 |
| `EXACT` | EXACT(a, b) | case-sensitive equality | EGLNQ | S | P1 |
| `FIND` | FIND(find, text, [start=1]) | position, case-sensitive | EGLNQ | S | P1 |
| `SEARCH` | SEARCH(find, text, [start=1]) | position, case-insensitive, `*`/`?` wildcards | EGLNQ | S | P1 |
| `LEFT`, `RIGHT` | LEFT(t, [n=1]) | first/last n characters | EGLNQ | S | P1 |
| `MID` | MID(t, start, n) | substring | EGLNQ | S | P1 |
| `LEN` | LEN(t) | length | EGLNQ | S | P1 |
| `LOWER`, `UPPER`, `PROPER` | LOWER(t) | case mapping | EGLNQ | S | P1 |
| `REPLACE` | REPLACE(t, start, n, new) | replace by position | EGLNQ | S | P1 |
| `REPT` | REPT(t, n) | repeat | EGLNQ | S | P1 |
| `SUBSTITUTE` | SUBSTITUTE(t, old, new, [instance]) | replace by match | EGLNQ | S | P1 |
| `TRIM` | TRIM(t) | collapse spaces | EGLNQ | S | P1 |
| `VALUE` | VALUE(t) | text → number | EGLNQ | S | P1 |
| `TEXTBEFORE`, `TEXTAFTER` | TEXTBEFORE(t, delim, [instance], [match_mode], [match_end], [if_not_found]) | text before/after a delimiter | EGLN | S | P1 |
| `TEXTBETWEEN` | TEXTBETWEEN(t, start_delim, end_delim, …) | text between delimiters (arguments from knowledge, unverified) | N | S | P1 |
| `COUNTMATCHES` | COUNTMATCHES(t, search) | number of occurrences | N | S | P1 |
| `FINDB`, `SEARCHB`, `LEFTB`, `RIGHTB`, `MIDB`, `LENB`, `REPLACEB` | — | DBCS byte variants; identical in UTF-8 | EGL | alias | = |
| `TEXT` | TEXT(x, format) | format with a number-format code | EGLQ | S | P2 |
| `FIXED` | FIXED(x, [decimals=2], [no_commas]) | fixed-decimal text | EGLNQ | S | P2 |
| `DOLLAR` | DOLLAR(x, [decimals=2]) | currency text | EGLNQ | S | P2 |
| `NUMBERVALUE` | NUMBERVALUE(t, [decimal_sep], [group_sep]) | locale-explicit parse | EGL | S | P2 |
| `VALUETOTEXT` | VALUETOTEXT(v, [format]) | value as text | E | S | P2 |
| `REGEX` | REGEX(t, expr, [replacement], [flags or occurrence]) | match, extract or replace | LN | S | P2 |
| `REGEXTEST` | REGEXTEST(t, pattern, [case_sensitivity]) | matches? | EG | S | P2 |
| `REGEXEXTRACT` | REGEXEXTRACT(t, pattern, [mode], [case_sensitivity]) | extract match/groups | EG | S | P2 |
| `REGEX.EXTRACT` | — | Numbers spelling of REGEXEXTRACT | N | alias | = |
| `REGEXREPLACE` | REGEXREPLACE(t, pattern, replacement, [occurrence], [case_sensitivity]) | replace matches | EG | S | P2 |
| `TEXTJOIN` | TEXTJOIN(sep, skip_empty, x OVER T) | join in item order | EGLN | P | P3 |
| `TEXTSPLIT` | TEXTSPLIT(t, col_delim, [row_delim], …) | split onto a target category | EGLN | A | P6 |
| `ARRAYTOTEXT` | ARRAYTOTEXT(x OVER T, [format]) | a collection as text | EN | R | P6 |
| `ASC`, `JIS`, `DBCS` | ASC(t) | full-width ↔ half-width | EGL | S | P7 |
| `BAHTTEXT` | BAHTTEXT(x) | Thai currency words | EL | S | P7 |
| `ROT13` | ROT13(t) | ROT13 | L | S | P7 |
| `ENCODEURL` | ENCODEURL(t) | percent-encode (pure, no network) | EGL | S | P7 |
| `ROMAN` | ROMAN(n, [form]) | Roman numerals | EGLNQ | S | P7 |
| `ARABIC` | ARABIC(t) | Roman numerals → number | EGL | S | P7 |

### 5.22 Lookup & reference <!-- sec:lookup -->

There are no cell ranges. A lookup searches a **key measure** along a category
and returns the **value measure** at the matching item. It compiles to a join
on the key, plus a reduce for "first match / nearest" modes.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `XLOOKUP` | XLOOKUP(key, keys OVER T, values, [if_not_found], [match_mode], [search_mode]) | value where keys = key | EGLN | L | P6 |
| `XMATCH` | XMATCH(key, keys OVER T, [match_mode], [search_mode]) | 1-based position of the match | EGLN | L | P6 |
| `MATCH` | MATCH(key, keys OVER T, [type=1]) | position, exact or sorted | EGLNQ | L | P6 |
| `LOOKUP` | LOOKUP(key, keys OVER T, values) | vector lookup | EGLNQ | L | P6 |
| `INDEX` | INDEX(x OVER T, n) | value at the n-th item | EGLN | P | P6 |
| `VALUEAT` | VALUEAT(list, index) | Quantrix spelling of INDEX | Q | alias | = |

### 5.23 Engineering <!-- sec:engbase -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `BIN2DEC`, `BIN2HEX`, `BIN2OCT` | BIN2HEX(t, [places]) | from binary text | EGLN | S | P7 |
| `DEC2BIN`, `DEC2HEX`, `DEC2OCT` | DEC2HEX(n, [places]) | from decimal | EGLN | S | P7 |
| `HEX2BIN`, `HEX2DEC`, `HEX2OCT` | HEX2DEC(t, [places]) | from hexadecimal text | EGLN | S | P7 |
| `OCT2BIN`, `OCT2DEC`, `OCT2HEX` | OCT2DEC(t, [places]) | from octal text | EGLN | S | P7 |
| `BASE` | BASE(n, radix, [min_length]) | number → text in a base | EGL | S | P7 |
| `NUMTOBASE` | — | Numbers spelling of BASE | N | alias | = |
| `DECIMAL` | DECIMAL(t, radix) | text in a base → number | EGL | S | P7 |
| `BASETONUM` | — | Numbers spelling of DECIMAL | N | alias | = |
| `HEXREP` | HEXREP(x) | hexadecimal bytes of a double | G | S | P7 |
| `BITAND`, `BITOR`, `BITXOR` | BITAND(a, b) | bitwise operations on 48-bit integers | EGLN | S | P7 |
| `BITLSHIFT`, `BITRSHIFT` | BITLSHIFT(n, shift) | bit shifts | EGLN | S | P7 |

Complex numbers are spreadsheet-style text, `"3+4i"`. Rust's `num-complex` is
already in `Cargo.lock`. <!-- sec:engcx -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `COMPLEX` | COMPLEX(re, im, [suffix]) | build `"a+bi"` | EGL | S | P7 |
| `IMREAL`, `IMAGINARY`, `IMABS`, `IMARGUMENT`, `IMCONJUGATE`, `IMNEG`, `IMINV` | IMABS(z) | parts and unary operations | EGL | S | P7 |
| `IMSUM`, `IMSUB`, `IMPRODUCT`, `IMDIV`, `IMPOWER`, `IMSQRT` | IMDIV(z1, z2) | arithmetic | EGL | S | P7 |
| `IMEXP`, `IMLN`, `IMLOG10`, `IMLOG2` | IMLN(z) | exp and logs | EGL | S | P7 |
| `IMSIN`, `IMCOS`, `IMTAN`, `IMSEC`, `IMCSC`, `IMCOT` | IMSIN(z) | trig | EGL | S | P7 |
| `IMSINH`, `IMCOSH`, `IMTANH`, `IMSECH`, `IMCSCH`, `IMCOTH` | IMSINH(z) | hyperbolic | EGL | S | P7 |
| `IMARCSIN`, `IMARCCOS`, `IMARCTAN`, `IMARCSEC`, `IMARCCSC`, `IMARCCOT` | IMARCSIN(z) | inverse trig | G | S | P7 |
| `IMARCSINH`, `IMARCCOSH`, `IMARCTANH`, `IMARCSECH`, `IMARCCSCH`, `IMARCCOTH` | IMARCSINH(z) | inverse hyperbolic | G | S | P7 |
| `IMFACT`, `IMGAMMA`, `IMIGAMMA` | IMGAMMA(z) | complex factorial and gamma | G | S | P7 |

Bessel functions, error function and units. <!-- sec:engsp -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `ERF` | ERF(lower, [upper]) | error function | EGLN | S | P4 |
| `ERF.PRECISE` | — | identical to 1-arg ERF | EL | alias | = |
| `ERFC` | ERFC(x) | complementary error function | EGLN | S | P4 |
| `ERFC.PRECISE` | — | identical to ERFC | EL | alias | = |
| `BESSELI`, `BESSELJ`, `BESSELK`, `BESSELY` | BESSELJ(x, n) | Bessel functions | EGLN | S | P7 |
| `CONVERT` | CONVERT(x, from_unit, to_unit) | unit conversion, Excel unit table | EGLN | S | P7 |
| `DELTA` | DELTA(a, [b=0]) | 1 if equal | EGLN | S | P7 |
| `GESTEP` | GESTEP(x, [step=0]) | 1 if x ≥ step | EGLN | S | P7 |

### 5.24 Database <!-- sec:db -->

In a spreadsheet the "database" is a range with a header row, and the criteria
are a second range. In Improv the database *is* the model. Each `D…` function
becomes the matching aggregate with `WHERE`: `DSUM(x OVER T WHERE pred)` =
`SUM(x OVER T WHERE pred)`. They are kept as names for migration.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `DSUM` | DSUM(x OVER T WHERE pred) | conditional sum | EGL | R | P6 |
| `DAVERAGE` | DAVERAGE(x OVER T WHERE pred) | conditional mean | EGL | R | P6 |
| `DCOUNT` | DCOUNT(x OVER T WHERE pred) | count numbers | EGL | R | P6 |
| `DCOUNTA` | DCOUNTA(x OVER T WHERE pred) | count non-blank | EGL | R | P6 |
| `DGET` | DGET(x OVER T WHERE pred) | the single match, else #VALUE!/#NUM! | EGL | R | P6 |
| `DMAX`, `DMIN` | DMAX(x OVER T WHERE pred) | conditional max/min | EGL | R | P6 |
| `DPRODUCT` | DPRODUCT(x OVER T WHERE pred) | conditional product | EGL | R | P6 |
| `DSTDEV`, `DSTDEVP` | DSTDEV(x OVER T WHERE pred) | conditional standard deviation | EGL | R | P6 |
| `DVAR`, `DVARP` | DVAR(x OVER T WHERE pred) | conditional variance | EGL | R | P6 |

### 5.25 Array / dynamic <!-- sec:array -->

A "spilled array" has no meaning in a model without a grid. Results land on an
**existing category** of the target measure (§6.11). Rows marked `P6*` are the
LAMBDA family, which depends on question 5.

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `FILTER` | FILTER(x OVER T, include, [if_empty]) | keep items where include holds (sparse result) | ELN | A | P6 |
| `SORT` | SORT(x OVER T, [order]) | values in sorted order, onto a positional category | EGLN | A | P6 |
| `SORTBY` | SORTBY(x OVER T, by1, [order1], …) | sorted by other measures | ELN | A | P6 |
| `UNIQUE` | UNIQUE(x OVER T, [exactly_once]) | distinct values, onto a positional category | EGLN | A | P6 |
| `SEQUENCE` | SEQUENCE(n, [cols], [start], [step]) | 1..n along a target category (= its positions) | ELN | A | P6 |
| `TAKE`, `DROP` | TAKE(x OVER T, n) | first/last n items (negative n from the end) | ELN | P | P6 |
| `ARRAY` | ARRAY(v1, …) | a literal list | G | A | P6 |
| `LIST` | LIST(v1, …) | Quantrix spelling of ARRAY | Q | alias | = |
| `LET` | LET(name, value, …, body) | local bindings | ELN | F | P6 |
| `LAMBDA` | LAMBDA(p1, …, body) | an anonymous function | EN | F | P6* |
| `LAMBDA.APPLY` | LAMBDA.APPLY(arg, LAMBDA(…)) | Numbers: apply a lambda | N | F | P6* |
| `MAP` | MAP(x OVER T, LAMBDA(v, …)) | element-wise application | EN | F | P6* |
| `REDUCE` | REDUCE(init, x OVER T, LAMBDA(acc, v, …)) | fold in item order | EN | F | P6* |
| `SCAN` | SCAN(init, x OVER T, LAMBDA(acc, v, …)) | running fold (P) | EN | F | P6* |
| `MAKEARRAY` | MAKEARRAY(R, C, LAMBDA(r, c, …)) | build over two categories | EN | F | P6* |
| `BYROW`, `BYCOL` | BYROW(x OVER R×C, LAMBDA(row, …)) | per item of one category | EN | F | P6* |
| `ISOMITTED` | ISOMITTED(param) | inside LAMBDA: was it passed? | EN | F | P6* |

### 5.26 Quantrix dimensional functions <!-- sec:quantrix -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `@ITEM` | @ITEM(T) | name of this cell's item along T | Q | S | P3 |
| `@ID` | @ID(T) | minimum unique item name | Q | S | P3 |
| `@ITEMCOUNT` | @ITEMCOUNT(T) | number of items in T | Q | S | P3 |
| `FIRST`, `LAST` | FIRST(x OVER T, [skip_empty]) | value at the first/last item | Q | P | P3 |
| `SUMMARY` | SUMMARY(T) | T's summary items (exclude them from ranges) | Q | S | P3 |
| `TIMELINEDATE` | TIMELINEDATE() | date of this cell's item on a time category | Q | S | P3 |
| `JOIN` | JOIN(x OVER T, sep) | Quantrix spelling of TEXTJOIN(sep, FALSE, …) | Q | alias | = |
| `SELECT` | SELECT(values, keys, lookup) | the values whose key = lookup (may be several, so wrap in SUM etc.) | Q | L | P6 |
| `SELECTLIST` | SELECTLIST(values, keys, lookups) | lookup with a list of keys | Q | L | P6 |
| `SELECTBETWEEN` | SELECTBETWEEN(values, keys, low, high, [inclusive]) | keys within a range | Q | L | P6 |
| `SELECTGREATERTHAN` | SELECTGREATERTHAN(values, keys, lookup, [inclusive]) | keys above | Q | L | P6 |
| `SELECTLESSTHAN` | SELECTLESSTHAN(values, keys, lookup, [inclusive]) | keys below | Q | L | P6 |
| `SELECTS` | SELECTS(values, tests, condition) | values whose test cell meets a condition | Q | L | P6 |
| `@DESCRIPTOR` | @DESCRIPTOR(T, name) | an item descriptor's value (Improv has no item descriptors yet) | Q | S | P7 |
| `@LEVEL` | @LEVEL(T, start, [end]) | group names in an item hierarchy (Improv has no item groups yet) | Q | S | P7 |

Quantrix's other dimensional features are syntax, not functions, so they are
not counted. Improv's equivalent forms, proposed in §6.7–§6.10:

| Quantrix (verified, 5.1 help) | Proposed Improv form | Compiles to |
|---|---|---|
| **`x:T[PREV]`**, `[NEXT]` | `PREV(x, T)`, `NEXT(x, T)` | key shift by item position (`map`) + join |
| **`[PREV-n]`**, `[NEXT+n]` | `PREV(x, T, n)`, `NEXT(x, T, n)` | the same, shift n |
| **`[~PREV]`** (missing → 0) | `PREV(x, T, 1, 0)` (4th argument = default) | shift + outer join with default |
| **`[FIRST]`**, `[LAST]` | `FIRST(x OVER T)`, `LAST(x OVER T)` | reduce to one value, broadcast back |
| **`T[THIS] = T[PREV] * g`** (recurrence) | `Bal = PREV(Bal, Time, 1, Opening) + Flow` | ordered scan per group (§7.3) |
| **`sum(T[PREV-2]:x .. T[PREV]:x)`** | `SUM(x OVER Time[THIS-2..THIS])` | ordered window reduce |
| running total (recurrence) | `SUM(x OVER Time[FIRST..THIS])` | ordered prefix reduce |
| **`#T`** (1-based index), **`@T`** (name) | `POSITION(T)`, `@ITEM(T)` | static item table |
| **`In X, …`** / **`… skip X`** | `… WHERE T IN (X, …)` / `NOT IN` | filter on coordinate |
| **`… using A as B`** | `SELECT(…)` / `XLOOKUP(…)` | join |
| summary items (Sum, Count, CountA, Average, Max, Min, Product) | derived measures `F(x OVER T)` | reduce |

`POSITION`, `PREV`, `NEXT` and the window syntax are Improv's own and are not
counted in the union.

### 5.27 Other <!-- sec:other -->

| Name | Signature | Meaning | Src | Kind | Phase |
|---|---|---|---|---|---|
| `PROBBLOCK`, `OFFTRAF`, `DIMCIRC`, `OFFCAP` | PROBBLOCK(traffic, circuits) | Erlang-B telephone-traffic formulas | G | S | P7 |

### 5.28 Out of scope, with reasons

Web, cube, external data and add-ins. <!-- sec:out-cube -->

| Name | Src | Reason |
|---|---|---|
| `CUBEKPIMEMBER`, `CUBEMEMBER`, `CUBEMEMBERPROPERTY`, `CUBERANKEDMEMBER`, `CUBESET`, `CUBESETCOUNT`, `CUBEVALUE` | E | OLAP-server connection functions; Improv *is* the cube, so a measure reference replaces them |

<!-- sec:out-ext -->

| Name | Src | Reason |
|---|---|---|
| `WEBSERVICE`, `RTD`, `DDE` | EL | network or live inter-process data inside a formula; breaks determinism (use `SQL(...)`/`CALL(...)` refreshes) |
| `STOCK`, `STOCKH`, `STOCKHISTORY`, `CURRENCY`, `CURRENCYH`, `CURRENCYCONVERT` | EN | remote market data; same reason; import it as an input measure |
| `CURRENCYCODE` | N | reads a currency format attribute Improv values do not have |
| `TRANSLATE`, `DETECTLANGUAGE` | E | cloud language services |
| `FILTERXML` | EL | needs an XPath engine; no modelling use; revisit on request |
| `CALL`, `REGISTER.ID` | E | Excel DLL calls; Improv's own `CALL(...)` source form covers external code |
| `EUROCONVERT`, `EURO` | EGL | fixed 1999–2002 pre-euro rates; no current use |

Cell, sheet and workbook references. <!-- sec:out-cell -->

| Name | Src | Reason |
|---|---|---|
| `ADDRESS`, `AREAS`, `CELL`, `COLUMN`, `COLUMNS`, `ROW`, `ROWS`, `OFFSET`, `INDIRECT` | EGLNQ | address cells by row/column; Improv addresses measures × items (`@ITEMCOUNT`, `POSITION`, `PREV` cover the dimensional needs) |
| `SHEET`, `SHEETS`, `REFERENCE.NAME`, `INTERSECT.RANGES`, `UNION.RANGES`, `TRIMRANGE`, `COLUMNNUMBER` | EGLN | sheets and ranges do not exist in the model |
| `FORMULATEXT`, `ISFORMULA`, `ISREF`, `FORMULA`, `GET.FORMULA`, `EXPRESSION`, `CURRENT` | EGLN | formula introspection; the formula list is first-class in the UI |
| `STYLE`, `COLOR`, `HYPERLINK`, `IMAGE`, `GET.LINK`, `PLAINTEXT`, `PHONETIC` | EGLN | presentation (styles, links, images, rich text, furigana) belongs to views |
| `INFO`, `GETENV`, `GNUMERIC_VERSION` | EGL | host environment queries; non-deterministic |
| `GETPIVOTDATA` | EGLN | a measure reference with item coordinates already *is* GETPIVOTDATA |
| `CONVERT_OOO` | L | reads LibreOffice's configuration table |
| `SIMTABLE` | G | Gnumeric scenario table; Improv scenarios are first-class |
| `@SILENT` | Q | works around Quantrix's #SIZE in `select`; Improv broadcasts implicitly |
| `CONCATLISTS` | Q | selects across Quantrix matrices; Improv measures already span the model |

Grid reshaping. <!-- sec:out-grid -->

| Name | Src | Reason |
|---|---|---|
| `TRANSPOSE`, `FLIP`, `TOCOL`, `TOROW`, `WRAPCOLS`, `WRAPROWS`, `EXPAND`, `HSTACK`, `VSTACK`, `CHOOSECOLS`, `CHOOSEROWS`, `MUNIT` | EGLN | reshape a 2-D grid; in Improv rows and columns are a view's layout (pivot), not part of a formula; `BY`/item filters select items |
| `VLOOKUP`, `HLOOKUP` | EGLN | index a column/row of a table range; the dimensional form is `XLOOKUP`/`SELECT` on a key measure (offer a migration rewrite) |
| `GROUPBY`, `PIVOTBY` | E | a derived measure with `BY`/`OVER` *is* a group-by/pivot |

Random numbers. <!-- sec:out-rand -->

| Name | Src | Reason |
|---|---|---|
| `RAND`, `RANDBETWEEN`, `RANDARRAY`, `RAND.NV`, `RANDBETWEEN.NV`, `RANDMT` | EGLNQ | non-deterministic; the engine must be reproducible (formula steering §2.5) |
| `RANDBERNOULLI`, `RANDBETA`, `RANDBINOM`, `RANDCAUCHY`, `RANDCHISQ`, `RANDDISCRETE`, `RANDEXP`, `RANDEXPPOW`, `RANDFDIST`, `RANDGAMMA`, `RANDGEOM`, `RANDGUMBEL`, `RANDHYPERG`, `RANDLANDAU`, `RANDLAPLACE`, `RANDLEVY`, `RANDLOG`, `RANDLOGISTIC`, `RANDLOGNORM`, `RANDNEGBINOM`, `RANDNORM`, `RANDNORMTAIL`, `RANDPARETO`, `RANDPOISSON`, `RANDRAYLEIGH`, `RANDRAYLEIGHTAIL`, `RANDSNORM`, `RANDSTDIST`, `RANDTDIST`, `RANDUNIFORM`, `RANDWEIBULL` | G | same; a later *seeded* Monte-Carlo design (`RAND(seed)` hashed with the coordinate) could bring them back deterministically |

## 6. Language features needed beyond functions

### 6.1 Conditionals: IF, IFS, SWITCH, CHOOSE

- **Representation.** Keep `Expr::Call`. The registry marks `IF`, `IFS`,
  `SWITCH`, `CHOOSE`, `IFERROR`, `IFNA` and `CLEARERROR` as *lazy*.
- **Laziness in the scalar evaluator** is real short-circuiting: only the
  selected branch is evaluated.
- **In the dataflow** IF compiles to `(cond ⋈ then) filtered on true` ∪
  `(cond ⋈ else) filtered on false`. That is two joins and a `concat`, so a
  branch that is missing or erroring where it is not selected never reaches
  the result. This matches the scalar evaluator, and a property test enforces
  it (§7.6). Inner joins alone would be wrong: today's `align` would drop the
  cell whenever the unselected branch is missing.
- **Type unification.** Both branches must have the same `ValueType`; an
  error branch unifies with anything. There is no Number↔Boolean coercion.
  Text against Number is a compile error. The dimension is the union of the
  condition's and the branches' dims, under the existing subset rule.
- **Omitted `else`.** `IF(c, x)` with no else gives *no cell* where `c` is
  false, rather than spreadsheet FALSE (LibreOffice returns FALSE, verified).
  This avoids a Number/Boolean mixed type. It is a documented deviation.

### 6.2 `^` and `POWER`

- **Decided: the mathematical rule** (question 7), as `AGENT_FORMULA_LANGUAGE.md`
  §4 already specifies: `^` is right-associative and binds tighter than unary
  minus, so `-2^2` = −(2²) = −4 and `2^3^2` = 2⁹ = 512. The order, tightest
  first: `%`, `^`, unary `-`, `* /`, `+ -`, `&`, comparisons, `NOT`, `AND`,
  `OR`.
- The spreadsheets differ (LibreOffice: `-2^2` = 4, `2^3^2` = 64; Quantrix
  likewise puts negation above `^`). Importing a spreadsheet formula
  (CSV/XLSX import, a pasted formula in Excel mode) parenthesizes so the
  value is kept: Excel `-2^2` imports as `(-2)^2`, `2^3^2` as `(2^3)^2`.
- `0^0` = 1 (LibreOffice behaviour).
- Add `BinaryOp::Pow` rather than a call, so the CNL can say "to the power of".

### 6.3 `&`, text concatenation

- **Precedence:** below `+ -`, above comparisons. This is Excel's and
  LibreOffice's rule (`1+2&"a"` = `3a`, verified). Quantrix puts `&` below the
  comparisons; that difference is noted, not adopted.
- **Coercion:** Number → text uses the shortest round-trip form (`3`, not
  `3.0`). Date → ISO 8601. Boolean → `TRUE`/`FALSE`.
- Add `BinaryOp::Concat`.

### 6.4 `%` postfix

`x%` = x/100. It binds tighter than `^` (Quantrix order; LibreOffice `50%` =
0.5). Add it as a parser rewrite to `x / 100`; no AST change.

### 6.5 LET

`LET(n1, e1, …, body)` is resolved at parse time. A bound name is a local
binding, shadowing measure names. The compiler shares the bound sub-plan
(build it once and reference it), so LET saves work in the dataflow too.
There is no runtime environment.

### 6.6 Errors and their propagation

- **Today** `x/0`, `SQRT(-1)` and non-numeric arguments become **NaN**, a
  Number. NaN compares false and sums to NaN, so where it came from is lost.
- **Recommendation (question 2): typed errors.** `CellValue::Err(u8)` already
  exists. Extend `ValueErrorKind` with the spreadsheet set: `Null`, `Div0`
  (rename DivisionByZero), `Value` (TypeMismatch), `Ref`, `Name`, `Num`, `Na`,
  plus Improv's DimensionMismatch, MissingInput and Custom. Then:
  - `x/0` → `#DIV/0!`; domain errors (`SQRT(-1)`, `LN(0)`, `FACT(-1)`) →
    `#NUM!`; a failed lookup → `#N/A`; a bad conversion → `#VALUE!`.
  - An operator or function given an error argument returns the first error,
    by argument order, except the error-handling functions in §5.2.
  - Comparisons involving an error give that error.
  - Reductions return the first error in *item order*. Item order is
    deterministic, unlike arrival order in the dataflow. `AGGREGATE` options
    can skip errors.
  - Missing ("no cell") stays distinct from an error and from blank:
    reductions skip it, `ISBLANK` tests it, `COUNTBLANK` counts it.
- **Migration.** Stored models change only where they produced NaN. Show NaN
  cells as `#NUM!` in the GUI.
- **`LANGUAGE improv`** returns float8 NaN today. After the change: SQL NULL by
  default, or a raised ERROR under a GUC (`improv.errors = 'null'|'raise'`).

### 6.7 Positional references on ordered categories

- Every Improv category already has an order: `Category.items: Vec<ItemId>`.
- **Proposed forms:** `PREV(x, T, [n=1], [default])` and `NEXT(…)`.
  `FIRST`/`LAST`(x OVER T). `POSITION(T)` is the 1-based index of this cell's
  item. They are written as functions first; the Quantrix bracket sugar
  (`x[T PREV]`) can follow once the function form has settled.
- **Off the end.** Out-of-range positions give no cell, or `default` if it is
  given. `default` is the explicit form of Quantrix's soft `[~PREV]`, which
  treats a missing neighbour as 0.
- **Recurrences.** A formula may reference its *own* measure only through
  `PREV` on one ordered category: `Bal = PREV(Bal, Time, 1, Open) + Flow`.
  `derived_build_order` currently rejects every self-reference as a cycle. It
  learns to accept exactly this shape and compiles it to an ordered scan
  (§7.3). Every other cycle is still rejected. Quantrix supports circular
  iteration; that is out of scope here.

### 6.8 Windows and running totals

`F(x OVER T[a..b])`, where each endpoint is `FIRST`, `LAST`, `THIS`, `THIS-n`
or `THIS+n`. The result keeps `T` (one value per item):

- running total: `SUM(x OVER Time[FIRST..THIS])`
- trailing three months: `AVERAGE(x OVER Month[THIS-2..THIS])`
- to-go: `SUM(x OVER T[THIS..LAST])`

It works with every reduction, including `TEXTJOIN`. It mirrors Quantrix's
`[PREV-2] .. [PREV]` ranges.

### 6.9 Volatile functions

`NOW()`/`TODAY()` read a model-level **as-of instant**. It defaults to the
host's clock when an evaluation starts and is recorded with the results, so a
re-run with the same as-of is identical. The `RAND` family is out (§5.28).

### 6.10 Multi-category `OVER`, `EXCEPT`, `WHERE`

- `SUM(x OVER Time, Region)`: the AST and the compiler already take a `Vec`,
  so only the parser changes.
- `SUM(x OVER ALL EXCEPT Region)` aggregates over every category but Region.
  The `except` field exists and the parser never sets it.
- `F(x OVER T WHERE pred)`: `pred` may use the measures being reduced,
  `@ITEM(T)`, `POSITION(T)` and `T IN (…)`. It compiles to a `filter` before
  the `reduce`. This one form carries all the `…IF`/`…IFS`/`D…` functions and
  Quantrix's `In`/`skip`.

### 6.11 Ranges and arrays in a dimensional model

- **"Range" arguments** become `measure OVER category`. Multi-measure
  reductions join their measures on the full coordinate first:
  `CORREL(x, y OVER T)`. The other arguments are scalars, or measures
  broadcast per group.
- **Variadic** `F(a, b, c)` stays element-wise over measures, as `MIN2` is
  today.
- **Array results** (`SORT`, `UNIQUE`, `SEQUENCE`, `FREQUENCY`, `LINEST`,
  `TEXTSPLIT`, `MODE.MULT`) need somewhere to land. The target measure must
  have a *positional* category, and result *k* goes to that category's *k*-th
  item. Too few items is a compile-time `#SPILL!`-equivalent error. Too many
  leaves no cells. This is the biggest design risk in the plan and is kept to
  P6.

### 6.12 Is the language Turing complete? Should it be?

**Today: no.** A formula is an expression over measures; the measure graph
must be acyclic (`derived_build_order` rejects every cycle); there is no
conditional, no iteration and no user-defined function. Every formula
evaluates in time bounded by the model's size. This is a *total* language:
every model terminates, and that is what makes incremental recalculation on
differential dataflow sound and the results reproducible.

**After this plan, still no, on purpose.** Each addition keeps evaluation
total:

- `IF`/`IFS`/`SWITCH` add branching, not looping.
- `PREV`-recurrences (§6.7) are bounded by the category's item count: an
  ordered scan of N items is N steps.
- Windows, reductions, `MAP`/`REDUCE`/`SCAN`/`BYROW`/`MAKEARRAY` iterate over
  a finite, known collection (a category's items or an array).
- `LAMBDA` is compile-time expanded and **non-recursive**: a named lambda may
  call only lambdas defined before it, so expansion terminates.

The result is like SQL without recursive CTEs, or Excel before LAMBDA: very
expressive, and every evaluation terminates. Excel itself became Turing
complete only through recursive `LAMBDA` (2021).

**Should it be?** Not in the formula engine. What Turing completeness would
cost:

- **Termination.** A model could hang. The engine would need fuel (an
  evaluation-step budget) and a "did not converge" error, as `Wasm` external
  functions already have.
- **Incrementality.** Differential dataflow handles fixed points
  (`iterate`), but an unbounded recursion has no small delta: one edit could
  re-run an arbitrarily long computation.
- **Determinism and explainability.** "Why is this cell 7?" no longer has a
  finite dependency answer.

Improv already has the escape hatch where general computation belongs:
`CALL(...)` external functions (Python, R, Julia, Pure, Wasm) are Turing
complete, run sandboxed with fuel and memory limits, and are evaluated
host-side outside the incremental hot path.

**What could be added, if a real model needs it** (none is in the phases;
each would be its own proposal):

1. **Recursive `LAMBDA` with fuel.** Allow self-reference in a named lambda,
   bounded by a per-cell step budget, returning `#NUM!` when exhausted. This
   is exactly what makes Excel Turing complete. Cost: low in the scalar
   evaluator, but it cannot be expanded at compile time, so it runs as an
   interpreted call per cell (like `CALL`).
2. **Iterative calculation (circular references).** Excel's "enable iterative
   calculation" and Quantrix's circular iteration: allow a cycle among
   measures, iterate until values change less than ε or N iterations pass.
   Maps onto differential dataflow's `iterate` scope. Needed by some
   financial models (interest on an average balance that depends on the
   interest). Cost: medium; it changes cycle detection and needs a
   convergence report.
3. **Goal seek / solver.** Find the input that makes a result hit a target.
   Not a language feature: a GUI/CLI command over the existing engine
   (`what_if` already evaluates scenarios).

Recommendation: ship the plan total; add (2) iterative calculation when a
model needs a circular reference; add (1) only on demand, gated by fuel.

### 6.13 Locale-aware entry

Decided (question 6): formulas can be typed in a locale's own spelling.

- **Canonical form.** Stored formulas stay locale-independent (`.` decimal,
  `,` separator, English canonical names, as JSON of the AST). A model opened
  in another locale shows the same formula in that locale's spelling.
- **Per-locale table:** decimal separator, argument separator (`;` where the
  decimal separator is `,`), array row/column separators, and translated
  function names. LibreOffice ships translated names for every function in
  its own locale data (its `formula/inc/core_resource.hrc` and the
  translations), the source for the translation table. Translations go in the
  registry as an extra alias list per locale.
- **Where the locale comes from:** the OS locale for the GUI and TUI, a
  `--locale` option for the CLI, and an `improv.locale` GUC for
  `LANGUAGE improv`. The canonical form is always accepted too, so a formula
  written in documentation works everywhere.
- **Ambiguity.** In a `,`-decimal locale `ROUND(1,5)` would be one argument;
  the separator is `;`, so it is `ROUND(1,5; 0)`. The tokenizer reads numbers
  by the active locale.
- **Rendering:** the GUI formula editor and the CNL describe in the active
  locale; `improv show` gains `--locale`.
- **Phase:** the parser and table mechanism in P1 (with `en` and one
  `,`-decimal locale, `de`, as the test pair); further locales as data, any
  time after.

## 7. Architecture

### 7.1 One registry drives everything

`crates/core_model/src/functions.rs` holds a static table. It lives in
core_model so that the parser, `nl_formula`, the GUI and `pg_model` can read it
without depending on the engine.

```rust
pub struct FnDef {
    pub id: FuncId,                // stable forever: persisted in stored formulas
    pub name: &'static str,        // canonical spelling (question 1)
    pub aliases: &'static [Alias], // spelling + optional argument mapping
    pub category: Category,        // drives docs grouping
    pub kind: Kind,                // Scalar | Reduce | Positional | Lookup | Array | Form
    pub params: &'static [Param],  // type, optional/default, variadic, Over, Lazy
    pub returns: RetRule,          // Fixed(T) | SameAs(i) | Unify(&[i])
    pub cnl: &'static str,         // "the absolute value of {0}"
    pub doc: &'static str,         // one line for the reference page
    pub sources: &'static str,     // "EGLNQ"
    pub since: &'static str,       // Improv version it shipped in
}
pub static FUNCTIONS: &[FnDef] = &[ /* … */ ];
```

What the table replaces:

- **Parser:** `scalar_func` and `agg_func` become one lookup by name or alias.
  It returns the id and the parameter schema, so arity, optional and default
  arguments, `OVER` placement and lazy arguments are checked in one place.
- **Typing:** `compiler::infer` reads `params` and `returns` instead of
  assuming Number → Number.
- **Kind dispatch:** `kind` replaces `is_aggregation`, `scalar_arity` and
  `pg_model`'s `expr_elementwise` test. Only `Scalar` is element-wise for
  trigger planning.
- **Evaluation** stays in the engine:
  `fn eval_scalar(id, &[CellValue]) -> CellValue` and
  `fn reduce(id, &[CellValue], params) -> CellValue`, each a `match` on id.
  A test walks `FUNCTIONS` and fails if any Scalar/Reduce id has no arm.
  `apply_scalar(&[f64])` goes away, because functions receive `CellValue`s
  (text, dates, errors).
- **CNL** reads the `cnl` template, so describe/parse round-trips for every
  function. The existing round-trip property extends to the registry.
- **GUI** `func_name` becomes a registry lookup, which deletes the third copy.
- **Docs:** a test renders `docs/src/functions.md` from the table and fails if
  the committed file differs. Running with `UPDATE_DOCS=1` rewrites it. No
  xtask is needed.
- **Stable ids:** existing ids 1–4, 10–15 and 20–21 keep their meaning. New
  ids are appended. A frozen `functions.ids` snapshot test fails on renumbering
  or reuse. Aliases resolve to the canonical id at parse time, so stored JSON
  never holds an alias.

### 7.2 Scalar functions in the dataflow

- `PlanNodeKind::FuncCall` generalises to n arguments. Literal arguments
  (`ROUND(x, 2)`) fold into the `map` closure. Measure arguments are aligned
  by folding `align` joins pairwise, as two-argument calls are today.
- Lazy calls lower to the filter/join/concat shape in §6.1.
- Functions that need a category's item set (`COUNTBLANK`, `@ITEMCOUNT`,
  `POSITION`, `SEQUENCE`) get a static `items(T)` collection built from
  `Category.items` at graph-build time. Changing items is a structural rebuild,
  as it is today.

### 7.3 Reductions and positional functions in differential dataflow

- **Reductions.** `Aggregate` keeps its shape: re-key to the group-by
  coordinate, then `reduce`. The closure gets each value with its item
  position, sorted, and calls `reduce(id, …)`. Item order makes "first error",
  `NPV`, `IRR` and `TEXTJOIN` deterministic.
- **Multi-measure reductions** join their inputs on the full coordinate before
  re-keying.
- **Scalar parameters** that are measures (`LARGE(x OVER T, k)` with `k[Region]`)
  join to the group key.
- **`WHERE`** is a `filter` before the reduce.
- **Per-item results** (`RANK`, `PERCENTRANK`, `TREND`) reduce to a per-group
  table, then join back on the group key, then map per item.
- **`PREV`/`NEXT`** are a key shift, not a reduce:
  `x.map(|(k, v)| (k with T = item[pos(k.T) + n], v))`, which drops anything
  out of range, then a join with the consumer. Each edit costs O(1) per
  changed cell. A default value adds an outer join against `items(T)`.
- **Windows** (`OVER T[a..b]`) group by every category except `T`, then
  `reduce` sorted by position. Each item gets its window's value as a prefix
  or sliding computation, and the results are flattened back onto T.
  - Ceiling: an edit recomputes its whole group, O(items in T). That is fine
    for time categories (tens to thousands of items).
  - Upgrade path: a prefix-sum tree for SUM/COUNT if profiling demands it.
- **Recurrences** (§6.7) use the same group-by-all-but-T reduce. It scans in
  position order, evaluating the formula's per-item scalar body with
  `scalar::eval` and feeding back the previous result. Same O(items) ceiling.
  No `iterate` scope is needed.
- **Lookups** re-key the key measure by its value and join with the lookup
  measure. `XLOOKUP` nearest-match modes reduce over the candidates. A
  `SELECT` returning several values becomes a collection that its enclosing
  reduction consumes. Without an enclosing reduction, more than one match is
  an error: `#VALUE!`, playing the role of Quantrix's `#SIZE`.

All of this uses `map`, `filter`, `join`, `reduce` and `concat`, the operators
already in use. The timely/DD pin is untouched.

### 7.4 pg_improv: `LANGUAGE improv` and the SQL side

- **Scalar functions for free.** `LANGUAGE improv` bodies use
  `scalar::eval`, so every Scalar function works in SQL once it is in the
  registry.
- **Reductions and positional functions** keep failing with `NeedsDimensions`.
  One later option: a SQL array argument (`float8[]`) acts as an implicit
  ordered category, so `IRR(cash)` and `NPV(r, cash)` work on arrays. This is
  noted, not planned.
- **Types:** `date`/`timestamptz` already map through `PG_EPOCH`; `text` and
  `bool` map directly. Error values map to NULL or a raised ERROR (§6.6).
- **Trigger mode** (`pg_model::delta`) must use the registry `kind`. Today any
  id with an arity counts as element-wise. That would be wrong for `PREV`,
  windows and lookups, which must force the full-recompute path.
- **No SQL-side function shims.** Postgres has its own `round`, `power` and so
  on. The plan does not mirror Improv functions as SQL functions.

### 7.5 Numerics and dependencies

- **Already available:**
  - `libm` (0.2.16, in `Cargo.lock`): `erf`, `erfc`, `tgamma`, `lgamma`,
    `j0`/`j1`/`jn` and `y0`/`y1`/`yn`. That covers ERF, GAMMA, BESSELJ and
    BESSELY.
  - `chrono` (a workspace dependency): dates.
  - `regex` (1.13.1, in `Cargo.lock`): REGEX*. Its syntax differs from
    ICU (LibreOffice) and PCRE2 (Excel, unverified), so look-around and
    back-references fail cleanly with `#VALUE!` and the docs say so.
  - `num-complex`: IM*.
- **Needed in P4:** incomplete gamma and beta functions and their inverses.
  Recommend `statrs` (MIT), after a `cargo deny` check.
- **Licence constraint:** Gnumeric's mathfunc and R's nmath are GPL, and
  LibreOffice is MPL. Their *behaviour* is the oracle; their code must not be
  copied into this Apache-2.0/MIT tree.
- **Decimal-correct rounding.** `ROUND`, `ROUNDUP`, `ROUNDDOWN`, `TRUNC`,
  `MROUND` and `FIXED` round the 15-significant-digit decimal representation,
  not the binary value. That matches LibreOffice's `ROUND(2.675, 2)` = 2.68.

### 7.6 Testing

- **Unit goldens per function**, in a table-driven test per category, with a
  case for every branch: domain edges, optional arguments, error inputs.
- **Scalar ≡ dataflow, per function.** Extend `scalar.rs`'s
  `matches_dataflow` proptest. Generate random `Expr` trees *from the
  registry*: pick an entry, generate arguments that satisfy its parameter
  schema, and include lazy calls and error-producing inputs. Assert
  `scalar::eval` equals the dataflow value at every coordinate, bit for bit,
  including which error. One generator covers all Scalar functions as they
  land.
- **Reductions and positional functions get a naive oracle.** A simple
  in-memory evaluator over `Model.inputs`, with sort, group, fold and shift,
  is compared with the engine on random models, as
  `broadcast_matches_naive_reference` already does for joins.
- **Goldens cross-checked against LibreOffice.**
  - LibreOffice 25.8.5.2 is installed here, and headless UNO evaluation works
    (verified for this review: PMT, NORM.S.INV, IRR, the error codes).
  - `scripts/goldens/lo_goldens.py` reads `crates/engine/tests/goldens/<cat>.cases`
    (formula + inputs). It evaluates each case in a headless Calc document and
    writes `<cat>.expected` (value, or error code).
  - Both files are committed. CI only reads them; it never needs LibreOffice.
    Regenerating is a manual, reviewed step.
  - Tolerance: 1e-12 relative for closed forms, 1e-9 for iterative or inverse
    functions (RATE, IRR, YIELD, quantiles).
  - Known deliberate deviations (omitted `else`, `0^0`, Unicode `CHAR`) are
    listed in the case file, not silently skipped.
- **Second oracle (optional).** Gnumeric's `ssconvert` is not installed. Where
  available it can recompute the same cases (`ssconvert --recalc`) for
  Gnumeric-only functions. Excel cannot run headless on Linux, so
  Excel-specific behaviour is taken from Microsoft's documentation examples,
  marked as such.
- **Fuzzing.** Seed `fuzz_formula_parser` and `fuzz_nl_formula` with a
  dictionary of every registry name and alias, `^ & %`, `OVER … WHERE`,
  `[FIRST..THIS]` and `EXCEPT`. Contract: no panic, and parse → print → parse
  is the identity on success.
- **Determinism and stress.** Extend the Time×Product oracle with IF,
  PREV-based growth, a running total and NPV, with known numeric results.
  Add an `--ignored` stress test: 200k-cell time series, random single-cell
  edits, checking the window and recurrence results against the naive oracle.

### 7.7 Documentation

- `docs/src/functions.md` is generated from the registry (§7.1). It holds one
  table per category: name, signature, meaning, phase/version and source
  letters.
- `docs/src/formulas.md` keeps the language: operators and precedence, IF,
  errors, `OVER`/`WHERE`/`EXCEPT`, windows, positional forms. Its "Gaps" list
  shrinks each phase.
- `AGENT_FORMULA_LANGUAGE.md` §4 grammar and §11 registry are updated in the
  same change as the code. `docs/man/improv.1` gains a FORMULAS pointer.
  CHANGELOG gets one entry per phase.

## 8. Phased delivery

Each phase ships on its own and keeps the tree green. Counts are functions
newly in scope. Sizes are rough engineering effort: S days, M one to two weeks,
L two to four weeks.

| Phase | Functions | Size | What ships |
|---|---|---|---|
| **P1** | 121 | M–L | **Foundation.** The registry (§7.1) replaces the five id tables; n-ary `FuncCall`; `CellValue`-typed function arguments; typed errors (§6.6). Language: `^` (right-associative, §6.2), `&`, `%`, IF/IFS/SWITCH/CHOOSE (lazy), error functions; locale-aware entry with `en` and `de` (§6.13). Functions: core math and trig, rounding (with optional digits, decimal-correct), core text. Generated `functions.md`. LibreOffice golden harness. |
| **P2** | 66 | M | **Dates, info, formatting.** DATE…WORKDAY.INTL on `DateTime` (§6.9 as-of for NOW/TODAY); IS* functions, N, T, TYPE; TEXT/FIXED/DOLLAR format codes; REGEX family. |
| **P3** | 9 | L | **Dimensional core.** Multi-category `OVER`, `EXCEPT`, `WHERE`; `PREV`/`NEXT`/`POSITION`/`FIRST`/`LAST`; windows `OVER T[a..b]` (running and trailing totals); self-recurrence via `PREV` (ordered scan); Quantrix `@ITEM`/`@ID`/`@ITEMCOUNT`/`SUMMARY`/`TIMELINEDATE`; `TEXTJOIN`; trigger-mode planning learns positional kinds. Few names, but the most Improv-specific value: it closes the "no previous-period" gap. |
| **P4** | 124 | L | **Statistics.** Generalised reduce (item-ordered, multi-measure, per-item results); descriptive and conditional aggregates; distributions (via `statrs`); tests; core regression; special functions; ERF/ERFC; SUMPRODUCT family. |
| **P5** | 55 | M | **Financial.** TVM, depreciation, NPV/IRR/XIRR on ordered categories, bonds and day-count bases. Iterative solvers are deterministic (fixed start, cap, bisection fallback). |
| **P6** | 44 | L | **Lookup, database, arrays.** XLOOKUP/XMATCH/MATCH/LOOKUP/INDEX; Quantrix SELECT family; D… functions as aliases of `WHERE` reductions; FILTER/SORT/SORTBY/UNIQUE/SEQUENCE/TAKE/DROP onto positional categories (§6.11); LET; LAMBDA family (9), non-recursive (§6.12). |
| **P7** | 253 | L (spread) | **The long tail, on demand.** Engineering (bases, bits, complex, Bessel, CONVERT); matrices; number theory and float; R-style kernels; option pricing; Erlang; Gnumeric/LibreOffice extra distributions, ETS forecasting, time-series analysis; calendars; text tail; `@DESCRIPTOR`/`@LEVEL` (needs item descriptors and groups in the model first). Each family is independent and can ship separately. |

P2 and P3 do not depend on each other. If previous-period modelling is the
priority, swap them. P4 depends on P3's generalised reduce only for windows.

## 9. Open questions for the user (answered: see Decisions at the top)

1. **Names.** Where Excel and Quantrix spell or mean the same thing
   differently, which is canonical? Examples: Quantrix `switch` is Excel
   `IFS`, `case` is Excel `SWITCH`, `clearerror` ≈ `IFERROR`, `select` ≈
   `XLOOKUP`/`FILTER`, and `x:Time[PREV]` vs a `PREV(x, Time)` function.
   Recommended: Excel names canonical, Quantrix names as aliases. For
   `SWITCH`, the Excel meaning wins and Quantrix-style `switch` is reachable as
   `IFS`.
2. **Error semantics.** Keep NaN for `x/0` and domain errors, or switch to
   typed error values (`#DIV/0!`, `#N/A`, `#NUM!`, `#VALUE!`) that propagate
   (recommended)? This changes results in stored models, and `LANGUAGE improv`
   output changes from NaN to NULL or a raised ERROR.
3. **1-based or 0-based.** 1-based (recommended; spreadsheets and Quantrix
   `#Cat`) or 0-based for `CHOOSE`, `INDEX`, `MATCH`, `POSITION` and
   `PREV`/`NEXT` offsets?
4. **Dates.** Keep real `DateTime` values, using serial numbers only at
   import/export and in `N()`/`DATEVALUE` (recommended)? Or adopt spreadsheet
   serial numbers (the 1900 system, with or without the fictitious 29-Feb-1900)
   as the date representation?
5. **LAMBDA.** Support `LAMBDA`/`MAP`/`REDUCE`/`SCAN`/`MAKEARRAY`/`BYROW`/`BYCOL`
   as compile-time-expanded, non-recursive definitions (recommended if yes)?
   Or decline the family (9 functions)?
6. **Locale.** Keep the formula language locale-independent: `.` decimal, `,`
   argument separator, English names, with locale applying only to
   `TEXT`/`NUMBERVALUE`/display (recommended)? Or should the parser also
   accept locale forms (decimal comma, `;` separator, translated function
   names), as LibreOffice and Excel do?
7. **`^` precedence.** Follow the spreadsheets, where unary minus binds
   tighter and `^` is left-associative (`-2^2` = 4, `2^3^2` = 64;
   recommended)? Or keep the steering grammar's mathematical convention
   (right-associative, `-2^2` = −4), which differs from every product
   checked?

## Appendix A — name collisions

| Name | Collision | Proposal |
|---|---|---|
| `SWITCH` | Excel/LibreOffice/Gnumeric/Numbers: `SWITCH(expr, match, result, …)`. Quantrix: `switch(cond1, value1, …)`, i.e. IFS | Excel meaning; Quantrix's is `IFS` (question 1) |
| `DURATION` | Excel/LibreOffice/Gnumeric: bond Macaulay duration. Numbers: `DURATION(weeks, days, hours, minutes, seconds, ms)` builds a duration (bond duration is `BONDDURATION`) | bond meaning; Numbers' becomes arithmetic in days |
| `ERRORTYPE` | LibreOffice numbers its own Err:5xx codes; Quantrix uses the `ERROR.TYPE` numbering | alias of `ERROR.TYPE` with its numbering |
| `ROUND`, `FLOOR`, `CEIL` | Improv's 1-argument forms vs the spreadsheets' 2-argument forms | optional second argument; defaults keep stored formulas |
| `AVG` | Improv's existing aggregation name; the products use `AVERAGE` | both; `AVERAGE` canonical (question 1) |
| `FIRST`, `LAST` | Quantrix list functions vs proposed positional forms | one function: `FIRST(x OVER T, [skip_empty])` |
| `ATAN2` | spreadsheet `(x, y)` vs Rust/C `atan2(y, x)` | spreadsheet order, tested against LibreOffice |
| `CHAR`, `CODE` | spreadsheets use the system code page | Unicode (= `UNICHAR`/`UNICODE`) |

## Appendix B — reproducing the inventory

The raw inventories were saved under `/tmp/fnref/` during this review; that
directory is not committed. To rebuild:

1. **Excel:** parse the table rows of the alphabetical page.
2. **LibreOffice:** headless `soffice --accept=socket,…` plus a pyuno script
   that enumerates `com.sun.star.sheet.FunctionDescriptions` (Name, Category,
   Arguments).
3. **Gnumeric:** collect the `<function name=…>` entries of every
   `plugins/fn-*/plugin.xml.in` at tag `GNUMERIC_1_12_62`, plus the four
   builtins in `src/func-builtin.c`.
4. **Numbers:** collect the `toc-item` names under each section of
   `support.apple.com/guide/functions`.
5. **Quantrix:** use the Wayback CDX query for
   `www.quantrix.com/help_resources/5.1/webhelp/*_functions.htm`, plus
   `structural_functions.htm`, `lookup_and_reference.htm` and
   `probability_distribution.htm`.

The union is keyed by upper-cased name. The source letters in §5 were filled
from that union mechanically, not typed by hand.
