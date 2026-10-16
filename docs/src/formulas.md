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
| 3 | `==` `<>` `!=` `<` `<=` `>` `>=` | at most one per level: `a < b < c` is an error |
| 4 | `+` `-` | left-associative |
| 5 | `*` `/` | left-associative |
| 6 | unary `-`, `NOT` | prefix; apply to the next primary |

Parentheses group as usual. Keywords (`AND`, `OR`, `NOT`, `OVER`, `TRUE`,
`FALSE`, function names) are case-insensitive.

`=` is only the assignment after the target. Inside an expression, equality is
`==`, and `<>` and `!=` both mean not-equal. So `Flag = Price == 10` is a
formula, and `X = Price = 10` is an error.

`NOT` and unary `-` bind tighter than any binary operator. `NOT Price > 15`
parses as `(NOT Price) > 15`, which fails type checking. Write
`NOT (Price > 15)`.

Type rules, checked when the formula compiles:

| Operators | Operands | Result |
|-----------|----------|--------|
| `+ - * /` | numbers | number |
| `AND OR` | booleans | boolean |
| comparisons | two values of the same type | boolean |
| unary `-` | number | number |
| `NOT` | boolean | boolean |

Division by zero gives `NaN`, not an error.

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

- the argument must be a measure reference (with an optional `[…]` list), not
  an expression: `SUM(Price * 2 OVER Product)` is an error;
- exactly one category follows `OVER`: `SUM(Q OVER Time, Product)` is an
  error, and so is `SUM(Q)` with no `OVER`.

`SUM` etc. are functions only when `(` follows; otherwise the word is an
ordinary measure name.

### Scalar functions

| Function | Arguments | Result |
|----------|-----------|--------|
| `ABS(x)` | 1 | absolute value |
| `ROUND(x)` | 1 | `f64::round`: nearest integer, halves away from zero |
| `FLOOR(x)` | 1 | floor |
| `CEIL(x)` | 1 | ceiling |
| `SQRT(x)` | 1 | square root (`NaN` for a negative number) |
| `NEG(x)` | 1 | negation |
| `MIN2(x, y)` | 2 | the smaller |
| `MAX2(x, y)` | 2 | the larger |

Arguments are any numeric expressions. The argument count is checked when
parsing: `ROUND(Price, 2)` is an error. Two-argument functions broadcast like
a binary operator.

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
| `Price ^ 2` | error: `'^' (power) is not supported` |
| `POWER(Price, 2)`, `IF(…)` | error: `unknown measure: POWER` / `IF` (no such functions) |
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

The v1 built-in aggregation functions and their function ids are fixed:

| Function | `FuncId` |
|----------|----------|
| `SUM`    | `1`      |
| `AVG`    | `2`      |
| `MIN`    | `3`      |
| `MAX`    | `4`      |

For example, `SUM(Revenue OVER Time)` takes `Revenue[Time, Product]` and
collapses `Time`, producing a per-product total `Revenue[Product]`. In the
compiler this becomes an `Aggregate` plan node grouped by the surviving
categories (`Product`), with `func = FuncId(1)`.

`AVG` divides the sum by the count of contributing cells; `MIN` and `MAX` take
the extremum. An aggregation argument must be numeric.

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
operators, dimension-aligning joins, `SUM`/`AVG`/`MIN`/`MAX` aggregation, the
named scalar functions, non-numeric (Text, Boolean, DateTime, Error) values,
and derived measures that reference other derived measures.

External functions use the `CALL(...)` definition form and are evaluated
host-side by `improv_extfn`. Python, R, Julia and Pure run in a sandboxed
subprocess; Wasm runs in-process. SQL-sourced measures use `SQL("...")`.

**Gaps:**

- no power operator (`^`) or `POWER` function;
- no `IF` or other conditional;
- no `EXCEPT` syntax (the AST field exists; the parser never sets it);
- one `OVER` category per aggregation;
- no reference to the previous period (or any relative item).
