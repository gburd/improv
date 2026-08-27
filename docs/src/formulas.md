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
- `Call(FuncId, args)` — a function call, used for aggregation.

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

The engine evaluates: input-measure references, unary negation, the arithmetic
and comparison binary ops, dimension-aligning joins, `SUM`/`AVG`/`MIN`/`MAX`
aggregation, named scalar functions (`ABS`/`ROUND`/`FLOOR`/`CEIL`/`SQRT`/`NEG`/
`MIN2`/`MAX2`), non-numeric (Text/Boolean/Error) values, and derived measures
that reference other derived measures. External-language functions are called
via the `CALL(...)` definition form and evaluated host-side (see the
[README](https://codeberg.org/gregburd/improv#readme)). **Planned / not yet
implemented:** standalone (broadcast) literals and additional external runtimes
(R/Julia/WASM).
