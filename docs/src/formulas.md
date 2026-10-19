# The Formula Language

A formula defines a **derived measure** in terms of other measures and
categories — never in terms of cell addresses. This is what makes logic
dimension-aware: you write the rule once, and it applies across every cell of
the target measure's tensor.

The formula AST lives in `improv_core_model::formula`. A `Formula` wraps an
`Expr`, which is one of:

- `Literal(Value)` — a constant.
- `Ref(MeasureId, DimensionSpec)` — a reference to another measure, with an
  optional dimension projection.
- `UnaryOp(op, expr)` — negation (`Neg`) or logical not (`Not`).
- `BinaryOp(op, left, right)` — arithmetic (`Add`, `Sub`, `Mul`, `Div`),
  logical (`And`, `Or`), or comparison (`Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`).
- `Call(FuncId, args)` — an aggregation (`SUM`, `AVG`, `MIN`, `MAX`) or a
  named scalar function (`ABS`, `ROUND`, …).

## Writing a formula

You write a formula as text: a target name, `=`, and an expression.

```text
Revenue = Price * Quantity
```

The parser (`improv_core_model::parser`) resolves each name in the expression
against the model, so every measure and category it names must exist. The
target need not exist yet. An unknown name, or anything the grammar does not
allow, is a parse error with a byte position; the parser never panics.

The target may carry a bracketed category list, as in
`Revenue[Time, Product] = …`. The parser skips that list; it does not set the
measure's categories.

`improv add-derived <db> <id> <name> "<expression>"` takes only the
expression (the part after `=`). `improv define <db> <id> "<Target> = …"`
takes the whole definition.

### Operators and precedence

From lowest to highest precedence:

| Level | Operators | Notes |
|-------|-----------|-------|
| 1 | `OR` | left-associative |
| 2 | `AND` | left-associative |
| 3 | `NOT` | prefix; applies to a whole comparison |
| 4 | `==` `<>` `!=` `<` `<=` `>` `>=` | at most one per level: `a < b < c` is an error |
| 5 | `&` | text join, left-associative |
| 6 | `+` `-` | left-associative |
| 7 | `*` `/` | left-associative |
| 8 | unary `-` | prefix |
| 9 | `^` | power, **right**-associative |
| 10 | `%` | postfix: `x%` is `x / 100` |

Parentheses group as usual. Keywords (`AND`, `OR`, `NOT`, `OVER`, `TRUE`,
`FALSE`, function names) are case-insensitive.

`=` is only the assignment after the target. Inside an expression, equality is
`==`, and `<>` and `!=` both mean not-equal. So `Flag = Price == 10` is a
formula, and `X = Price = 10` is an error.

`NOT` binds looser than a comparison, as in SQL: `NOT Price > 15` means
`NOT (Price > 15)`, and `NOT a AND b` means `(NOT a) AND b`.

`^` follows the mathematical convention: it is right-associative and binds
tighter than unary minus, so `2^3^2` is 2⁹ = 512 and `-2^2` is −(2²) = −4.
The spreadsheets read both the other way (64 and 4): write `(2^3)^2` and
`(-2)^2` for their reading. The exponent may be negative (`2^-1` is 0.5).
`0^0` is 1, `0^-1` is `#DIV/0!` and a negative base with a fractional
exponent is `#NUM!`; `POWER(x, y)` is the same as `x ^ y`.

`&` joins two values as text (`"Q" & 3` is `Q3`; numbers print as they
display, booleans as `TRUE`/`FALSE`). `50%` is 0.5.

Type rules, checked when the formula compiles:

| Operators | Operands | Result |
|-----------|----------|--------|
| `+ - * / ^` | numbers | number |
| `&` | any two values | text |
| `AND OR` | booleans | boolean |
| comparisons | two values of the same type | boolean |
| unary `-` | number | number |
| `NOT` | boolean | boolean |

### Errors

An error is a value, shown as the spreadsheets show it:

| Error | Produced by |
|-------|-------------|
| `#DIV/0!` | division by zero (`x / 0`, including `0 / 0`) |
| `#NUM!` | a numeric domain error (`SQRT(-1)`, `LN(0)`) or a result too large for a number |
| `#VALUE!` | a value of the wrong kind (`VALUE("abc")`, `CHOOSE` out of range) |
| `#N/A` | no value available (`NA()`, `IFS` with no true test) |
| `#NAME?`, `#REF!`, `#NULL!` | `ERROR("#NAME?")` etc.; Improv itself does not produce them |

An operator or function given an error returns that error (the left operand,
or the first argument, first). An aggregation over a category containing an
error returns the error (the smallest `ERROR.TYPE` code if there are
several). `ISERROR`, `ISERR`, `ISNA` and `ERROR.TYPE` inspect errors, and
`IFERROR`, `IFNA` and `CLEARERROR` replace them.

A missing cell is not an error: where an input has no cell, the result has
no cell either (an aggregation skips it).

### Typing formulas in your language

The formula bar (GUI) and `improv define` / `add-derived` (CLI) read
formulas in the **formula locale**: `IMPROV_LOCALE`, else the OS locale
(`LANG`). In German (`de`) a formula uses LibreOffice's German function
names, a decimal comma and `;` between arguments, and the GUI shows stored
formulas the same way:

| English (stored) | German |
|---|---|
| `IF(Price > 1.5, ROUND(Price, 2), 0)` | `WENN(Price > 1,5; RUNDEN(Price; 2); 0)` |
| `SUM(Revenue OVER Time)` | `SUMME(Revenue OVER Time)` |

The English spelling is always accepted as well. Models store formulas in
the English form, so a model opens the same in every locale. Measure and
category names, text in `"…"` and `OVER` are never translated. English and
German are the locales so far; another is a table of names
(`crates/core_model/src/locale/<tag>.tsv`).

### Literals

| Literal | Example | Notes |
|---------|---------|-------|
| number | `42`, `1.07` | digits and `.`; no exponent (`1e3`) and no leading `.` (`.5`) |
| text | `"hello"` | double quotes; no escapes, so the text cannot contain `"` |
| boolean | `TRUE`, `FALSE` | |
| date | `#2025-01-01#` | midnight UTC |
| date and time | `#2025-01-01T09:30:00Z#` | RFC 3339 |

A literal may stand alone (`X = 42`) or appear inside an expression
(`Price * 1.07`). A literal has no categories, so it broadcasts over the other
operand.

### Names

A bare name starts with a letter or `_` and continues with letters, digits or
`_`. Any other name goes in **single** quotes:

```text
X = 'Unit Price' * 2
X = 'Bob''s Rate' + 1
```

Inside single quotes, write `''` for one `'`: `'Bob''s Rate'` names the
measure `Bob's Rate`. A quoted name is always a
measure or category name, never a keyword or function, so `'SUM'` and
`'Over'` name measures. `''` (empty) and an unclosed quote are errors.
Single quotes are names; double quotes are text.

A qualified name like `Matrix.Measure` parses, but is always rejected:
Improv has no matrices to qualify a name with. The error names the
qualifier.

### Measure references and BY

`Measure[Cat, …]` sets the reference's `by` list: keep only the listed
categories. Inside an aggregation the categories it leaves out are collapsed,
so `SUM(Quantity[Product] OVER Time)` is a per-Product total. Outside one,
the list must name every category of the measure (`Quantity[Time, Product]`):
`Quantity[Product]` alone is refused, because nothing would collapse Time
(see [Dimension projection](#dimension-projection-by--over--except) below).

### Aggregations

```text
Total = SUM(Revenue OVER Time)
```

The form is `AGG(Measure OVER Categories, …)`. The aggregations include
`SUM`, `AVERAGE` (or `AVG`), `MIN`, `MAX`, `PRODUCT`, `SUMSQ`, over booleans
`AND`, `OR` and `XOR`, the ordered `FIRST`, `LAST`, `S` and `TEXTJOIN`, and
the statistics (`COUNT`, `MEDIAN`, `STDEV`, `PERCENTILE`, `COUNTIF`,
`CORREL`, `SLOPE`, `TTEST`, …); see the [Function Reference](./functions.md).

```text
Total    = SUM(Revenue OVER Time, Region)        -- several categories
Others   = SUM(Revenue OVER ALL EXCEPT Product)  -- all but Product
Big      = SUM(Revenue OVER Time WHERE Revenue > 1000)
Names    = TEXTJOIN(", ", TRUE, Label OVER Product)
Opening  = FIRST(Balance OVER Time)
Spread   = STDEV(Revenue OVER Time)
Top3     = LARGE(Revenue OVER Time, 3)
Fit      = CORREL(Price, Quantity OVER Time)
Big      = COUNTIF(Revenue OVER Time, ">1000")
Sold     = SUMIFS(Revenue, Region OVER Time, "N*", Quantity, ">=10")
Rank     = RANK(Revenue OVER Product)                -- per item: keeps Product
Tier     = RANK.DENSE(Score OVER Person)             -- ties share a rank, no gaps
Buyers   = COUNTUNIQUE(Customer OVER Order)          -- distinct values
```

- The reduced arguments must be measure references (with an optional `[…]`
  list), not expressions: `SUM(Price * 2 OVER Product)` is an error; define
  the expression as a measure first.
- `OVER` follows a reduced argument and applies to all of them. Several
  categories are separated by commas; a comma followed by anything but a
  category name starts the next argument (`TEXTJOIN("-", TRUE, x OVER T)`).
- `OVER ALL EXCEPT Cat, …` reduces every category of the measure but those.
- `WHERE condition` keeps only the cells where the condition is true; the
  condition may use the measure and any measure over its categories. If no
  cell passes, the group has no cell.
- Rows reach the reduction in **item order** along the reduced categories,
  which is what `FIRST`, `LAST` and `TEXTJOIN` see.
- Other arguments are parameters with one value per group: a literal, or a
  measure over the categories the result keeps (`TEXTJOIN(Sep, TRUE, x OVER
  T)` with `Sep` per Product).
- The cash-flow functions (`NPV`, `IRR`, `MIRR`, `XNPV`, `XIRR`, `FMRR`,
  `FVSCHEDULE`) read the flows in item order: the first item with a cell is
  period 0 (`NPV` discounts it one period, as Excel does). An item with no
  cell is skipped, not a zero flow, so the flows after it move up a period;
  enter 0 for a period without a flow.
- Without `OVER`, nothing is reduced (each cell is its own group).
- The `…IF` and `…IFS` functions take Excel's criteria: a number or boolean
  means equality, and text may start with `=`, `<>`, `<`, `<=`, `>` or
  `>=`. Text is compared without regard to case, and `=`/`<>` text takes
  the wildcards `*` (any run), `?` (any one character) and `~` (the next
  character literally). In `SUMIFS(x, c1 OVER T, k1, c2, k2)` the summed
  measure and every criteria measure are reduced together.
- Per-item reductions (`RANK`, `RANK.AVG`, `RANK.DENSE`, `PERCENTRANK`,
  `TREND`, `GROWTH`) give one result per item of their single `OVER`
  category, and the result keeps it. `RANK.DENSE` is SQL's `dense_rank()
  OVER (ORDER BY x DESC)` (order 1: `ASC`): tied values share a rank and the
  next value ranks one after them, so 9, 7, 7, 4 rank 1, 2, 2, 3.
- `COUNTUNIQUE` (Google Sheets' name) is SQL's `count(DISTINCT x)`: the
  number of distinct values among the cells, numbers by value and text
  case-sensitively. Neither LibreOffice nor Gnumeric has it or a dense rank.
- An error among the rows is the result, except that `COUNT` skips errors,
  `COUNTA` counts them, the `…IF` functions skip rows whose criteria cell is
  an error, and `AGGREGATE`'s options 2, 3, 6 and 7 ignore them.

`SUM` etc. are functions only when `(` follows; otherwise the word is an
ordinary measure name.

### Positions, neighbours and running totals

Every category has an order: its items' order. Formulas can use it.

```text
Rank     = POSITION(Time)                          -- 1, 2, 3, …
Year     = @ITEM(Time)                             -- the item's name
Growth   = Revenue - PREV(Revenue, Time, 1, 0)     -- change since last year
Ahead    = NEXT(Revenue, Time)                     -- next year's value
Running  = SUM(Revenue OVER Time[FIRST..THIS])     -- running total
Trailing = AVERAGE(Revenue OVER Time[THIS-2..THIS])-- three-period average
ToGo     = SUM(Budget OVER Time[THIS..LAST])
```

- `POSITION(T)` is the 1-based position of each item of `T`; `@ITEM(T)` its
  name; `@ITEMCOUNT(T)` how many items `T` has. They are measures over `T`,
  so they broadcast like any other.
- `PREV(x, T, [n = 1], [default])` is `x` at the n-th previous item along
  `T`, `NEXT` at the n-th next. Before the first item (or where that item
  has no cell) the result is the `default`, or no cell without one.
- A **window** `OVER T[from..to]` gives one result per item: the
  aggregation over the items from `from` to `to`, where each bound is
  `FIRST`, `LAST`, `THIS`, `THIS-n` or `THIS+n`. An empty window is `#N/A`.

### LET and LAMBDA

```text
Margin = LET(rev, Price * Quantity, cost, Unit * Quantity, (rev - cost) / rev)
Double = MAP(Quantity, LAMBDA(q, q * 2))
Square = LAMBDA(x, x * x)(Price - 1)
Running = SCAN(0, Deposits OVER Time, LAMBDA(acc, d, acc * 1.01 + d))
Final   = REDUCE(0, Deposits OVER Time, LAMBDA(acc, d, acc * 1.01 + d))
```

- `LET(name, value, …, body)` names values for the body (a later value may
  use an earlier name). A name shadows a measure with the same name.
- `LAMBDA(p, …, body)` is a function of its parameters. Call it on the
  spot (`LAMBDA(x, x * x)(Price)`) or pass it to `MAP`, `REDUCE` or
  `SCAN`.
- `MAP(x, …, LAMBDA(p, …, body))` applies the body cell by cell.
- `SCAN(init, x OVER T, LAMBDA(acc, v, body))` folds along `T` in item
  order: each item's result is the body with `acc` the previous item's
  result (`init` before the first) and `v` the item's value. `REDUCE`
  keeps only the last. Both are recurrences underneath, so they update
  incrementally.

More forms of the same kind:

```text
Grid  = MAKEARRAY(Row, Col, LAMBDA(r, c, r * 10 + c))   -- over Row × Col
Steps = SEQUENCE(Month, 100, 5)                          -- 100, 105, 110, …
Range = BYROW(Sales OVER Product, LAMBDA(row, MAX(row) - MIN(row)))
Total = BYCOL(Sales OVER Month, LAMBDA(col, SUM(col)))
```

- `MAKEARRAY(R, C, LAMBDA(r, c, body))` evaluates the body over R × C with
  `r` and `c` the item positions.
- `SEQUENCE(T, [start = 1], [step = 1])` is `(POSITION(T) - 1) * step +
  start`: a category, not a count, sets its length.
- `BYROW(x OVER C, LAMBDA(row, body))` runs the body once per item of x's
  other categories, `row` being x's values along C; the body must reduce
  it (`SUM(row)`, `MAX(row) - MIN(row)`). `BYCOL` is the same form: in a
  dimensional model a row and a column differ only in the category named.

LET, LAMBDA, MAP, MAKEARRAY, SEQUENCE and BYROW expand when the formula is
read: the stored formula (and what the formula bar shows) is the expanded
expression. A LAMBDA cannot call itself, so every formula still terminates.

**Named LAMBDAs** (Excel's Name Manager functions) are defined once and
called by name:

```text
improv define model.db 1 "Margin = LAMBDA(price, cost, (price - cost) / price)"
improv define model.db 120 "Pct = Margin(Price, Cost) * 100"
```

A call is stored as a call, not its expansion, so redefining `Margin`
changes every formula that uses it. A named LAMBDA reads only its
parameters (pass measures as arguments), may call LAMBDAs defined before
it, and never itself. They persist with the model (Mentat and Postgres
stores).

A parameter the body uses where a category goes (`OVER c`, `POSITION(c)`,
`RELABEL`'s categories) is a **category parameter**: the call passes a
category name, and a parameter the body reduces (`MAX(x OVER c)`) takes a
measure:

```text
LOGNORM = LAMBDA(x, c, LN1P(x) / LN1P(MAX(x OVER c)))
An      = LOGNORM(Authored, Person)   -- LN1P(Authored) / LN1P(MAX(Authored OVER Person))
```

A parameter used both as a value and as a category is an error.

### List results: INTO

A function that returns a list says where the list lands: `INTO` a
category, whose items take the results in order.

```text
Top = SORT(Sales OVER Product, -1 INTO Rank)   -- largest first
Ids = UNIQUE(Customer OVER Order INTO Slot)
```

The result keeps the measure's other categories and gains `Rank`: the
k-th value lands at Rank's k-th item. Results beyond Rank's last item are
dropped; if there are fewer, the later items have no cell. `INTO` must name
a category the measure does not range over.

`INTO R, C` lays the list over two (or more) categories row by row, the
last varying fastest, as Excel spills a block:

```text
Fit  = LINEST(Sales, Price, Promo OVER Week, TRUE, TRUE INTO StatRow, StatCol)
Grid = SORT(Sales OVER Week INTO Row, Col)
```

**Matrices** are a measure over two categories: `MDETERM(A OVER R, C)`.
Matrix results (`MINVERSE`, `MPSEUDOINVERSE`, `CHOLESKY`, `EIGEN`) come
row by row, so `INTO R2, C2` lays them out as a block:
`Inv = MINVERSE(A OVER R, C INTO R2, C2)`.

A matrix product needs no function: operands that share a category join
over it, so with `A` over R × K and `B` over K × C,

```text
AB = A * B              -- over R × K × C
P  = SUM(AB OVER K)     -- over R × C: the product
```

(`LINSOLVE` is `MINVERSE` then the same product.) Two measures with no
category in common are still refused, as a likely mistake.

`LINEST` and `LOGEST` take any number of x's (every measure before the
options is reduced): the coefficients come last x first, then b, and with
`stats` Excel's 5 × (k + 1) block. `SORTBY(x, by1 OVER T, order1, by2,
order2, …)` sorts by several keys. `SELECTS(values, tests OVER T,
condition)` keeps the values whose test meets a SUMIF-style criterion.

### Lookups

A lookup is a reduction whose key is a parameter: it searches a table along
a category and returns the value at the matching item.

```text
Rate  = XLOOKUP(Grade, GradeKey OVER Band, GradeRate, 0)
Slot  = MATCH(Score, Threshold OVER Band)      -- 1-based position along Band
Third = INDEX(Sales OVER Month, 3)
Name  = SELECT(Label, Code OVER Item, Wanted)  -- Quantrix argument order
Names = SELECTLIST(Label, Code OVER Item, Wanted)  -- Wanted over its own category
```

The key may range over categories the table lacks: with `Grade` per
Employee and the table per Band, `Rate` is per Employee, each cell looking
its own grade up in the whole table. Positions (`MATCH`, `XMATCH`,
`INDEX`) are item positions along the category, so an item with no cell
still counts. A table row needs every reduced measure: an item with a key
but no value is not found. The approximate modes do not assume sorted keys;
they pick the nearest key (for `MATCH`/`LOOKUP` type 1, the largest ≤ the
key, the last on a tie). `XLOOKUP`'s binary search modes (±2) are the linear
search. Database functions are the aggregations under their spreadsheet
names, with the criteria as `WHERE`: `DSUM(Sales OVER Region WHERE Year ==
2025)`; `DGET` wants exactly one match.

### Recurrences

A measure may read its own earlier values through `PREV`:

```text
Balance = PREV(Balance, Time, 1, Opening) + Deposits - Withdrawals
```

Along `Time`, each period's balance starts from the previous period's
(`Opening`, a measure per account, before the first). The engine computes it
as a scan in item order, per group of the other categories, and updates it
incrementally: an edit re-runs the chain it is in. Rules:

- every reference to the measure itself is `PREV(itself, T, n, default)`
  along one of its own categories (`NEXT` cannot refer back);
- the rest of the formula is element-wise (no aggregation or other
  positional function inside it; define those as measures first);
- the measures it reads together range over all of its categories (they
  supply its cells);
- other cycles between measures are still errors.

Every evaluation still terminates: a scan is as long as the category.

### Functions

Improv has the spreadsheet functions people expect, with Excel's names:
`ROUND(Price, 2)`, `MOD(Units, 12)`, `LEFT(Code, 3)`, `SUM(a, b, c)`.
[The Function Reference](./functions.md) lists every one, generated from the
code. The argument count and types are checked when the formula is parsed
and compiled: `ROUND(Price, 1, 2)` is a parse error that shows the
signature, and `LEN(Price)` (a number where text is wanted) a type error.

Arguments are expressions and broadcast like a binary operator's operands:
`ROUND(Revenue / Units, 2)` ranges over the union of the arguments'
categories. A literal argument costs nothing extra.

**R-style distributions.** `R.DNAME(x, params…, [give_log])` is the
density or mass at x (or its logarithm); `R.PNAME(x, params…,
[lower_tail], [log_p])` is P(X ≤ x), or P(X > x) when `lower_tail` is
FALSE, as a logarithm when `log_p` is TRUE; `R.QNAME(p, …)` inverts it,
reading p the same way (a discrete quantile is the smallest k with P(X ≤ k)
≥ p). Parameters out of range are #NUM!. `CAUCHY(x, a, TRUE)` is the right
tail P(X > x), as in Gnumeric. Improv does not copy Gnumeric's defects:
`CUM_BIV_NORM_DIST` is exact (Owen's formula), `R.DSNORM` is right for a
non-zero location, `R.PST` honours `log_p`.

**Math extras** follow Gnumeric, within Improv's limits so every call is
quick: `ITHPRIME` and `NT_PI` stop at the 10⁷-th prime, `PARTITIONS` at n =
4000, the Erlang functions at 10⁶ circuits, other number theory at 2⁵²;
past a limit the answer is #NUM!. `HDATE_MONTH` numbers Tishri 0 to Elul
11 (12 and 13 for Adar I and II). `ROMAN` takes Excel's TRUE/FALSE forms;
`ARABIC` accepts a leading `-`; `ENCODEURL` keeps RFC 3986's unreserved
characters.

**Complex numbers** are text in Excel's form, `"3+4i"`, `"-j"`,
`"2.5e-3-i"`: suffix `i` or `j`, no spaces, the same suffix in every
argument of one call. A plain number is a real complex number. Results
print with 15 significant digits, and a real result is still text
(`IMSUM("1+i", "1-i")` is `"2"`).

**Option pricing.** The `OPT_*` functions follow Gnumeric: the first
argument is `"c"` or `"p"` (`"a"`/`"e"` first for `OPT_BINOMIAL`,
`"cc"`/`"cp"`/`"pc"`/`"pp"` for `OPT_ON_OPTIONS`); `OPT_BINOMIAL` takes
at most 10,000 steps (Gnumeric 100,000: the tree is quadratic, and that
took seconds). Times are in years;
rates, volatilities and cost of carry are annual fractions (cost of carry
is r for a stock, r − q with a dividend yield, 0 for a future, r − rf for
a currency). `OPT_BARRIER`, `OPT_TOUCH`, `OPT_PROB_HIT` and
`OPT_PROB_INMONEY` follow LibreOffice, with case-sensitive flags
(`"c"`/`"p"`, `"i"`/`"o"` knock in/out, `"c"`/`"e"` barrier watched
continuously or at expiry, `"f"`/`"d"` pays foreign/domestic) and an
optional Greek (`"delta"`, `"gamma"`, `"theta"`, `"vega"`, `"volga"`,
`"vanna"`, `"rho"`, `"rhof"`). Where Gnumeric 1.12's bivariate normal is
wrong (some Roll–Geske–Whaley, compound, extendible and two-asset inputs),
Improv returns the correct value.

### IF and friends

```text
Bonus = IF(Sales > Target, Sales * 0.1, 0)
Label = IF(Price > 15, "dear", "cheap")
Safe  = IFERROR(Revenue / Units, 0)
```

`IF(test, then, [else])` needs a boolean test and two branches of the same
type. It is **lazy**: where the test is true only `then` matters, so a
missing cell or an error in `else` never shows (and the other way round).
Without `else`, a false test gives no cell. Where the test is an error, the
result is that error.

The result ranges over the categories of the test and both branches. If the
test has fewer categories than the result (`IF(Price > 15, Revenue, 0)`,
with `Price` over Product and `Revenue` over Time×Product), the cells are
those of the test joined with a branch that has the rest (here
`Revenue`): Improv never invents coordinates. A formula where neither branch
fills in the test's missing categories is a dimension error.

`IFS`, `SWITCH`, `CHOOSE`, `IFERROR`, `IFNA`, `CLEARERROR` and `CLEARZERO`
are built on `IF` and behave the same way.

### Missing cells: COALESCE and FILL

A formula over a missing cell has no cell, and `IF`, `IFNA` and `ISNUMBER`
cannot change that: they only see the cells their test has. Two functions
give a value where an input has none:

```text
An    = COALESCE(Authored, Vol * 0)   -- Authored, else 0 wherever Vol has a cell
An    = FILL(Authored, 0)             -- Authored, else 0 for every Person
Merit = 0.30 * Vn + 0.25 * FILL(Authored, 0)
Best  = COALESCE(Final, Draft, Estimate, 0)
```

- `COALESCE(a, b, …, default)` has a cell wherever an argument that ranges
  over all of the result's categories has one, and takes the first argument
  with a cell there (the last argument is the default). With `Authored` set
  for 2 of 4 people and `Vol` for all 4, `COALESCE(Authored, Vol * 0)` has 4
  cells.
- `FILL(x, default)` has a cell at every item combination of x's
  categories: x where it has a cell, else the default.
- The arguments' categories must nest, and the result ranges over the
  largest set. An argument over fewer categories (a literal, or `Vol` over
  Person under a Person × Year result) broadcasts onto the cells the others
  supply but adds none of its own: `COALESCE(Mail, Vol)` with `Mail` over
  Person × Year has `Mail`'s cells, `FILL(Mail, Vol)` every Person × Year.
  `FILL`'s default may not range beyond x's categories.
- The arguments must have one type (an error-only argument, `NA()`, takes
  the others'). An error is a value, so `COALESCE` keeps it: `IFERROR`
  replaces errors.
- `FILL`'s cells come from the item list, which is fixed for the life of an
  engine: an edit to an input moves the result incrementally, but a new
  item takes a rebuild, as any structural change does.

### Source forms: CALL and SQL

Two forms are allowed only as the **whole** right-hand side, and only through
`parse_definition` (the `improv define` command):

```text
Score = CALL(myfn, Price, Quantity)
Sales = SQL("SELECT product, amount FROM sales")
```

- `CALL(fn, m1, m2, …)` makes an external-function measure: `fn` is a
  registered external function, followed by zero or more argument measure
  names (bare or quoted). The function runs outside the dataflow and its
  results become input cells.
- `SQL("query")` takes one double-quoted query string. It describes a
  SQL-sourced input measure; the caller maps the result columns to
  dimensions.

Neither may appear inside a larger expression.

### Not in the grammar

| Written | Result |
|---------|--------|
| `M.Price` | error: `unknown qualifier: M` |
| `Quantity EXCEPT Time` | error: no `EXCEPT` syntax |

## Broadcasting: the Revenue example

The canonical formula is:

```text
Revenue[Time, Product] = Price[Product] * Quantity[Time, Product]
```

`Price` ranges over `Product` only; `Quantity` ranges over `Time` and
`Product`. Their dimensions differ, so the compiler must align them before it
can multiply element-wise. One dimension is a subset of the other
(`{Product} ⊆ {Time, Product}`), so `Price` is **broadcast** over `Time`: the
result is defined over the union, `[Time, Product]`, and each `Time` reuses the
matching product's price.

The rule is strict: for a binary op, one operand's dimension must be a subset of
the other's. If neither is (say, `Product`-only added to `Region`-only), the
compiler rejects it with a **dimension mismatch** rather than guessing.

## Dimension projection: BY / OVER / EXCEPT

A measure reference carries a `DimensionSpec` with three category lists that
reshape which dimensions the reference exposes:

| Field    | Meaning |
|----------|---------|
| `by`     | Categories to **keep** (group by). If non-empty, only these survive. |
| `over`   | Categories to **aggregate over** (collapse). Used with an aggregation call. |
| `except` | Categories to **drop**. |

Starting from the measure's base categories, the compiler keeps a category if it
passes `by` (or `by` is empty), is not in `except`, and is not in `over`. The
`over` set specifically marks the dimensions an enclosing aggregation collapses.

In the text syntax, `Measure[Cat, …]` fills `by` and `AGG(Measure OVER Cat)`
fills `over`. Nothing fills `except`.

Only an aggregation collapses categories. So a bare reference whose `by`
leaves a category out is a compile error: `X = Quantity[Product]`, with
`Quantity` over `[Time, Product]`, says "Quantity\[…\] leaves out category
Time, which only an aggregation can collapse". Inside an aggregation, the
result keeps the categories that `by` keeps and `over` does not name.

## Aggregation

Aggregation is expressed as a `Call` whose single argument is a measure
reference. The reference's `over` list names the categories to collapse; the
result is defined over the remaining categories.

Every function has a fixed `FuncId`, stored in saved formulas and never
reused (`crates/core_model/src/functions.rs`). The aggregations:

| Function | `FuncId` |
|----------|----------|
| `SUM` | `1` |
| `AVERAGE` (`AVG`) | `2` |
| `MIN` | `3` |
| `MAX` | `4` |
| `PRODUCT` | `5` |
| `SUMSQ` | `6` |
| `AND`, `OR`, `XOR` | `7`, `8`, `9` |

For example, `SUM(Revenue OVER Time)` takes `Revenue[Time, Product]` and
collapses `Time`, producing a per-product total `Revenue[Product]`. In the
compiler this becomes an `Aggregate` plan node grouped by the surviving
categories (`Product`), with `func = FuncId(1)`.

`AVERAGE` divides the sum by the count of contributing cells; `MIN` and `MAX`
take the extremum. An aggregation's argument must be numeric, except `AND`,
`OR` and `XOR`, which take booleans.

## Type and dimension checking

Compiling a formula (`improv_engine::compiler::compile_formula`) runs two
passes:

1. **`infer`** — walks the AST, resolving each measure reference against the
   model's metadata and computing every subexpression's value type **and**
   dimension. It rejects type errors (e.g. multiplying non-numbers, `Not` on a
   number) and structural errors (dimensions that cannot be broadcast).
2. **`build_plan`** — lowers the typed tree to a `PlanNode` graph, inserting a
   `Join` where operand dimensions differ (to broadcast) and an `Aggregate`
   node for each aggregation call.

The result is an operator plan that maps directly onto differential-dataflow
operators — see [Architecture](./architecture.md).

## Scope today

The engine evaluates input-measure references, literals (standalone or as
operands), unary negation and `NOT`, the arithmetic, logical and comparison
operators, dimension-aligning joins, aggregations (several measures,
parameters, `WHERE`, windows, per-item results such as `RANK`, lookups,
list results with `INTO`), `LET` and `LAMBDA`, the 674
built-in functions of the [Function Reference](./functions.md) (math,
rounding, text, dates, statistics and distributions, financial, engineering), the lazy
`IF` family, positions and neighbours (`POSITION`, `PREV`, `NEXT`),
recurrences, typed error values, non-numeric (Text, Boolean, DateTime)
values, and derived measures that reference other derived measures.

External functions use the `CALL(...)` definition form and are evaluated
host-side by `improv_extfn`. Python, R, Julia and Pure run in a sandboxed
subprocess; Wasm runs in-process. SQL-sourced measures use `SQL("...")`.

**Gaps:**

- no `EXCEPT` on a plain reference (only `OVER ALL EXCEPT …` inside an
  aggregation);
- a window or per-item reduction takes one `OVER` category;
- `TEXTSPLIT` takes no row delimiter; `LINEST` with several x's refuses
  collinear x's (#NUM!) rather than dropping them as Excel does;
- named LAMBDAs are defined with `improv define` (no GUI editor yet);
- no `MMULT` or `LINSOLVE` function: a matrix product is a join and a sum
  (see Matrices); `FORECAST.ETS.CONFINT` is #N/A for seasonal models
  (LibreOffice's interval is a random simulation);
- `NETWORKDAYS` and `WORKDAY` take no holiday list yet.
