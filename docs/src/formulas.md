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

The form is `SUM|AVG|MIN|MAX ( MeasureRef OVER Category )`:

The aggregations are `SUM`, `AVERAGE` (or `AVG`), `MIN`, `MAX`, `PRODUCT`,
`SUMSQ`, and over booleans `AND`, `OR` and `XOR`; see the
[Function Reference](./functions.md).

- the argument must be a measure reference (with an optional `[…]` list), not
  an expression: `SUM(Price * 2 OVER Product)` is an error;
- exactly one category follows `OVER`: `SUM(Q OVER Time, Product)` is an
  error, and so is `SUM(Q)` with no `OVER`.

`SUM` etc. are functions only when `(` follows; otherwise the word is an
ordinary measure name.

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
`Quantity` over `[Time, Product]`, says "Quantity[…] leaves out category
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
operators, dimension-aligning joins, aggregations, the built-in functions
(including the lazy `IF` family), typed error values, non-numeric (Text,
Boolean, DateTime) values, and derived measures that reference other derived
measures.

External functions use the `CALL(...)` definition form and are evaluated
host-side by `improv_extfn`. Python, R, Julia and Pure run in a sandboxed
subprocess; Wasm runs in-process. SQL-sourced measures use `SQL("...")`.

**Gaps:**

- no `EXCEPT` syntax (the AST field exists; the parser never sets it);
- one `OVER` category per aggregation;
- no reference to the previous period (or any relative item).
