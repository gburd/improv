# AGENT_FORMULA_LANGUAGE.md
Formula Language Steering Document for Improv

## 1. Purpose

This document defines the formula language for Improv:

- Syntax and grammar
- AST structure
- Type system
- Dimension semantics and broadcasting
- Aggregation
- Error model
- Controlled-natural-language (CNL) translation
- Built-in function registry
- Later-phase extension points (external functions, SQL) tied to phases

The formula language must be readable, declarative, dimension-aware,
named-measure-based, deterministic, easy to learn, and easy to debug.

---

## 2. Design Philosophy

Inspired by Lotus Improv, Quantrix Modeler, and dimensional algebra:

### 2.1 Named Measures, Not Cell References

```text
Revenue = Price * Quantity
```

### 2.2 Explicit Dimensions

```text
Revenue[Time, Product]
```

### 2.3 Automatic Broadcasting

```text
Price[Product] * Quantity[Time, Product]
```

### 2.4 Declarative Aggregation

```text
TotalRevenue[Product] = SUM(Revenue OVER Time)
```

### 2.5 Pure and Deterministic

Formulas have no side effects; identical inputs yield identical outputs.

### 2.6 Errors Are Values

Errors propagate through the dependency graph rather than aborting evaluation.

---

## 3. Syntax Overview

### 3.1 Basic Structure

```text
MeasureName = Expression
```

### 3.2 Expressions

Literals, measure references, arithmetic / logical / comparison operators,
function calls, and aggregations.

### 3.3 Literals

- Numbers: `123`, `3.14`
- Text: `"hello"`
- Boolean: `TRUE`, `FALSE`
- Date: `#2025-01-01#`

### 3.4 Operators

- Arithmetic: `+` `-` `*` `/` `^`
- Logical: `AND` `OR` `NOT`
- Comparison: `=` `<>` `<` `<=` `>` `>=`

---

## 4. Grammar (EBNF)

```ebnf
Formula      = Identifier "=" Expression ;
Expression   = Term { ("+" | "-") Term } ;
Term         = Factor { ("*" | "/") Factor } ;
Factor       = Primary [ "^" Factor ] ;
Primary      = Literal
             | MeasureRef
             | FunctionCall
             | Aggregation
             | "(" Expression ")" ;

MeasureRef   = Identifier [ "[" DimList "]" ] ;
DimList      = Identifier { "," Identifier } ;

FunctionCall = Identifier "(" [ ArgList ] ")" ;
ArgList      = Expression { "," Expression } ;

Aggregation  = AggFunc "(" MeasureRef "OVER" Identifier ")" ;
AggFunc      = "SUM" | "AVG" | "MIN" | "MAX" ;
```

> The `CALL(...)` (external function, Phase 6) and `SQL("...")` (live query,
> Phase 7) productions are added in later phases and are intentionally omitted
> from the initial v1 grammar. See §11.3.

---

## 5. AST Structure

Nodes: expression, operator (unary/binary), function call, measure reference,
and aggregation. Example for `Revenue = Price * Quantity`:

```text
Assignment(
  name = "Revenue",
  expr = BinaryOp(op = "*",
                  left  = MeasureRef("Price"),
                  right = MeasureRef("Quantity")))
```

This is `core_model::Formula`; the compiler consumes it (see
`AGENT_ENGINE_STEERING.md` §5) and storage serializes it (see
`AGENT_STORAGE_STEERING.md` §5).

---

## 6. Type System

### 6.1 Primitive Types

Number, Text, Boolean, Date, Error.

### 6.2 Type Checking

Ensures operators and functions receive valid types and aggregations receive
valid collections. The v1 numeric core operates on Number; non-numeric derived
values are a deferred engine follow-up.

---

## 7. Dimension Semantics

### 7.1 Dimensionality

Each measure declares its dimensions: `Revenue[Time, Product]`.

### 7.2 Alignment and Broadcasting

A lower-arity measure is broadcast across missing dimensions:

```text
Price[Product] * Quantity[Time, Product]   -- Price broadcast across Time
```

### 7.3 Dimension Errors

Raised when dimensions cannot be aligned or an aggregation is misapplied.

---

## 8. Aggregation

### 8.1 Syntax

```text
SUM(Revenue OVER Time)
```

### 8.2 Rules

Aggregation reduces dimensionality, must name the collapsed category, and
operates on a measure collection. Supported: `SUM`, `AVG`, `MIN`, `MAX`
(engine func ids 1–4; see `AGENT_ENGINE_STEERING.md` §5.3).

### 8.3 Examples

```text
TotalRevenue[Product] = SUM(Revenue OVER Time)
AveragePrice          = AVG(Price OVER Product)
```

---

## 9. Error Model

Errors: syntax, type, dimension, cycle, and runtime. Errors are values that
propagate through the dependency graph and surface in the interfaces.

---

## 10. Controlled Natural Language (CNL)

**v1 uses a controlled grammar, not open English.** This keeps the deterministic
core honest. Implemented in `improv_nl_formula` as a bidirectional translation.

### 10.1 CNL → Formula

Explicit dimension phrases (`over Time`, `by Product`, `for each Region`) map to
`DimensionSpec`:

```text
"Revenue equals price times quantity"
  → Revenue = Price * Quantity

"Total revenue by product is the sum of revenue over time"
  → TotalRevenue[Product] = SUM(Revenue OVER Time)
```

`parse_nl_formula(ctx, text) -> Result<Formula, NlError>` tokenizes, parses the
controlled grammar, and resolves measure/category names against the model.

### 10.2 Formula → CNL

```text
Revenue = Price * Quantity
  → "Revenue is price multiplied by quantity."
```

`describe_formula(ctx, formula) -> String` walks the AST and emits English
phrases. Parse↔describe must round-trip (a tested property).

---

## 11. Built-in Function Registry

### 11.1 v1 Aggregations

`SUM`, `AVG`, `MIN`, `MAX` (via the `OVER` aggregation form).

### 11.2 Planned Scalar Functions

As the numeric core grows to general `FuncCall` (an engine follow-up),
add scalar built-ins incrementally, e.g. `ABS`, `ROUND`, `FLOOR`, `CEIL`;
logical `AND`/`OR`/`NOT`; date `TODAY`/`YEAR`/`MONTH`/`DAY`. Add functions only
as the engine gains the ability to evaluate them deterministically — do not
document a function the engine cannot run.

### 11.3 Later-Phase Extension Points

Two grammar extensions are committed for later phases (built after the v1 core,
so the v1 grammar above stays minimal):

- **External-language functions (Phase 6)** — a `CALL(func, args...)` form
  dispatching to an external runtime. Python first (Resolver One lineage), then
  R, Julia, and WASM. External functions must be pure, typed, and declare
  dimensionality, so they behave as ordinary operators to the engine and keep it
  deterministic.
- **SQL live-query measures (Phase 7)** — a `SQL("...")` form producing a
  measure collection. See `AGENT_DATABASE_CONNECTIVITY.md`. Live-query measures
  are explicitly marked so determinism tests continue to treat pure paths as
  pure.

---

## 12. Examples

```text
Profit = Revenue - Cost
TotalRevenue[Product] = SUM(Revenue OVER Time)
Revenue[Time, Product] = Price[Product] * Quantity[Time, Product]
```

---

## 13. Definition of Success

The formula language succeeds when formulas are readable and maintainable,
dimension semantics and broadcasting are intuitive and correct, aggregation is
easy, errors are clear, and CNL parse/describe round-trips reliably.

---

## 14. Document Index

Part of the full steering set:

- `AGENT_MASTER_STEERING.md`
- `AGENT_GUI_STEERING.md`
- `AGENT_ENGINE_STEERING.md`
- `AGENT_STORAGE_STEERING.md`
- `AGENT_FORMULA_LANGUAGE.md`
- `AGENT_DATABASE_CONNECTIVITY.md`
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md`
- `STEERING_SYSTEM_OVERVIEW.md`
