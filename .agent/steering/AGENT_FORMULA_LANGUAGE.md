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

### 2.7 Stable from 1.0

The language is a compatibility promise from 1.0 (decided 2026-10-21; user
guide formulas.md, "Language stability" and "Differences from Excel"). A
1.x may add functions, optional trailing arguments and syntax that is a
parse error today; it never changes a valid formula's result, type or
categories (except a documented bug fix), a function's name or argument
order, precedence, or a FuncId. Deprecation warns in 1.x; removal is 2.0.
Before adding syntax, check that the text is an error today.

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

- Numbers: `123`, `3.14`, `1e-10`, `6.02E+23` (an exponent overflowing
  f64, `1e999`, is a parse error; decided 2026-10-21)
- Text: `"hello"`
- Boolean: `TRUE`, `FALSE`
- Date: `#2025-01-01#`

### 3.4 Operators

- Arithmetic: `+` `-` `*` `/` `^` (right-associative, tighter than unary
  minus: `-2^2` = −4, decided 2026-10-16), postfix `%`
- Text: `&`
- Logical: `AND` `OR` `NOT` (NOT binds looser than comparisons)
- Comparison: `==` `<>` `!=` `<` `<=` `>` `>=` (`=` is only the assignment)

---

## 4. Grammar (EBNF)

```ebnf
Formula      = Identifier "=" Expression ;
Expression   = OrExpr ;
OrExpr       = AndExpr { "OR" AndExpr } ;
AndExpr      = NotExpr { "AND" NotExpr } ;
NotExpr      = "NOT" NotExpr | Comparison ;        (* NOT a > b == NOT (a > b) *)
Comparison   = Concat [ CmpOp Concat ] ;          (* == <> != < <= > >= *)
Concat       = Additive { "&" Additive } ;
Additive     = Term { ("+" | "-") Term } ;
Term         = Unary { ("*" | "/") Unary } ;
Unary        = "-" Unary | Power ;
Power        = Postfix [ "^" Unary ] ;            (* right-assoc; -2^2 = -4 *)
Postfix      = Primary { "%" } ;                  (* x% = x / 100 *)
Primary      = Literal | Call | Special | MeasureRef | Local
             | "(" Expression ")" ;
Literal      = Number | Text | "TRUE" | "FALSE" | DateLiteral ;  (* #2025-01-31# *)
Number       = Digits [ "." Digits ] [ ( "e" | "E" ) [ "+" | "-" ] Digits ] ;
MeasureRef   = Name [ "[" Name { "," Name } "]" ] ;
Local        = Identifier ;                       (* bound by LET / LAMBDA *)

Call         = FuncName "(" [ Args ] ")" ;           (* Sep: "," or ";" (de) *)
Args         = Arg { Sep Arg } { Sep Named } | Named { Sep Named } ;
Arg          = Expression [ OverClause ] [ IntoClause ] ;
Named        = Identifier "=>" Arg ;              (* a parameter by name: type => 1 *)
OverClause   = "OVER" ( Cats [ Window ] | "ALL" [ "EXCEPT" Cats ] )
               [ "WHERE" Expression ] ;
Window       = "[" Bound ".." Bound "]" ;          (* FIRST | LAST | THIS[+-n] *)
IntoClause   = "INTO" Cats ;                      (* a list result, row-major *)
Cats         = Name { "," Name } ;

Special      = "LET" "(" { Identifier "," Expression "," } Expression ")"
             | Lambda "(" Expression { "," Expression } ")"   (* called on the spot *)
             | "MAP" "(" Expression { "," Expression } "," Lambda ")"
             | ("SCAN" | "REDUCE") "(" Expression "," MeasureRef "OVER" Name "," Lambda ")"
             | ("BYROW" | "BYCOL") "(" MeasureRef "OVER" Name "," Lambda ")"
             | "MAKEARRAY" "(" Name "," Name "," Lambda ")"
             | "FIXPOINT" "(" Expression "," Lambda { "," ( Expression | Named ) } ")"
             | "SEQUENCE" "(" Name { "," ( Expression | Named ) } ")"
             | NamedLambda "(" [ Expression { "," Expression } ] ")" ;
Lambda       = "LAMBDA" "(" Identifier { "," Identifier } "," Expression ")" ;

Definition   = Formula
             | Identifier "=" "SQL" "(" Text ")"          (* SQL-sourced input *)
             | Identifier "=" "CALL" "(" Name { "," Name } ")"  (* external fn *)
             | Identifier "=" Lambda ;                   (* a named LAMBDA *)
```

> **Named arguments** (`XLOOKUP(k, Key OVER T, V, search_mode => -1)`): the
> names are `FnDef::param_names`' (case-insensitive), only after the
> positional arguments, each once and not also by position; a function with
> a repeating parameter takes none (its names number a list). A skipped
> optional parameter gets the default `functions/params.rs` writes
> (`[type=0]`), and one without a default cannot be skipped (a parse error
> naming it). The parser stores the positional call, so printers need no
> change and parse(print(f)) is f. Properties in
> `engine/tests/properties.rs` check that a named call means its positional
> form and that each written default is what omitting the parameter does.
>
> The parser (`core_model::parser`, its module docs carry the same grammar)
> is the source of truth. Which names are functions, which arguments an
> aggregation reduces (`OVER` after any of them), and which return lists
> (`INTO`) come from the function registry (§11), not the grammar.

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
valid collections. Non-numeric derived values are supported: comparison/
logical operators produce real `Boolean` `CellValue`s (not a numeric 1.0/0.0
encoding), and `Text`/`Date`/`Error` flow through the same value lane.

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

One registry, `core_model::functions` (`functions.rs` plus one file per
family under `functions/`), drives the parser, type checking, the engine,
the GUI printer and completion, the CNL, the TUI help and the generated
reference `docs/src/functions.md` (a test fails if the page drifts:
`UPDATE_DOCS=1 cargo test -p improv_core_model`). 674 functions, with
Excel's names canonical and other products' as aliases (the functions plan,
`docs/reviews/2026-10-16-builtin-functions-plan.md`).

Each entry (`FnDef`) has a persisted id (never reused; ranges per family in
the plan), a kind (`Scalar`, `Lazy` like IF, `Aggregate`, `PerItem` like
RANK, `Array` with `INTO`), argument kinds, which arguments are reduced
(`Over`), a return type rule and a one-line description. Parameter names for
signature help come from `FnDef::param_names()`. Search and completion:
`functions::search`, `complete`, `call_at`.

Evaluation: scalars in `engine::builtins` (one module per family),
reductions, per-item and list results in `engine::reduce`. Named LAMBDAs
are model data (`Model::lambdas`), called by id and expanded at compile
time.

### 11.3 Sources outside the engine

- **External functions** — `Target = CALL(func, Arg, …)` runs a registered
  Python/R/Julia/Pure/Wasm function host-side (`improv_extfn`); the measure
  is an input the refresh fills, so the engine stays deterministic.
- **SQL-sourced measures** — `Target = SQL("SELECT …")` (CLI: `import-sql`,
  `refresh-sql`) fills an input measure from a query, mapping columns to
  categories. See `AGENT_DATABASE_CONNECTIVITY.md`. Set-based work (joins,
  filters over large tables) stays in the database; the model computes on
  the result.

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
