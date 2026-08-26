# Controlled Natural Language

`improv_nl_formula` translates between formulas and a **controlled** natural
language: a small, fixed, deterministic grammar — not open-English NLP. There
are no LLMs and no external services. The same grammar parses a sentence into a
formula and renders a formula back into a sentence.

Two functions form the surface:

- `parse_nl_formula(ctx, text)` → `Formula`
- `describe_formula(ctx, formula)` → `String`

Both take an `NlContext` built from a `Model`, which resolves measure and
category **names** (case-insensitively) to ids.

## The grammar

```text
sentence   = [ subject copula ] expr [ "." ]
subject    = measure-name          (* parsed, then discarded *)
copula     = "equals" | "is"

expr       = term { ("plus" | "+" | "minus" | "-") term }
term       = factor { ("times" | "*" | "divided" "by" | "/") factor }
factor     = number
           | agg-call
           | measure-ref
           | "(" expr ")"

agg-call   = ["the"] ("sum" | "average" | "min" | "max") "of" measure-ref
measure-ref= measure-name { dim-phrase }
dim-phrase = "over" cat-list                 (* aggregate over  -> DimensionSpec.over *)
           | ("by" | "for" "each") cat-list  (* keep / group by -> DimensionSpec.by   *)
cat-list   = category-name { "and" category-name }
```

## Subject and copula are discarded

A sentence may open with `<measure> equals` or `<measure> is`, but the parser
strips it: the returned `Formula` is only the right-hand-side expression. The
target measure is a property of *where* the formula is stored, not of the
expression tree. So both of these parse to the same formula:

```text
Revenue equals price times quantity.
price times quantity
```

The copula is only stripped when one actually follows the first word, so a bare
expression like `price times quantity` (no copula) still parses correctly.

## Operators

| Phrase        | Symbol | Operation |
|---------------|--------|-----------|
| `plus`        | `+`    | addition |
| `minus`       | `-`    | subtraction |
| `times`       | `*`    | multiplication |
| `divided by`  | `/`    | division |

Precedence is the usual arithmetic one: `times`/`divided by` bind tighter than
`plus`/`minus`. So `price times quantity plus revenue` parses as
`(price * quantity) + revenue`. Parentheses override precedence.

## Aggregation phrases

An aggregation reads as `the <func> of <measure-ref>` (the `the` is optional).
The function words map to the same function ids as the
[formula language](./formulas.md):

| Phrase       | `FuncId` |
|--------------|----------|
| `sum of`     | `1`      |
| `average of` | `2`      |
| `min of`     | `3`      |
| `max of`     | `4`      |

## Dimension phrases

A dimension phrase binds to the nearest preceding measure reference and fills in
that reference's `DimensionSpec`:

- `over <categories>` sets `over` — the categories an enclosing aggregation
  collapses.
- `by <categories>` or `for each <categories>` sets `by` — the categories to
  keep / group by.

Categories in a list are joined with `and`, e.g. `over time and region`.

The `except` field of a `DimensionSpec` is **not** expressible in this grammar.

## A full example

```text
the sum of revenue over time for each product
```

parses to a `SUM` call over a `Revenue` reference whose `over = [Time]` and
`by = [Product]` — i.e. total revenue per product, summed across years.

## Round-trip and rendering

`describe_formula` is the inverse rendering. It uses a fixed vocabulary:

- Operators render as their words (`plus`, `minus`, `times`, `divided by`).
- Aggregations render as `the <func> of ...`.
- A reference's `over` renders as `over ...`; its `by` renders as
  **`for each ...`** (not `by`).

So the example above renders back as:

```text
the sum of Revenue over Time for each Product
```

Because `describe` uses `for each` for the `by` list and the grammar accepts
both `by` and `for each`, describe → parse is a stable round trip for the
supported constructs. Comparison and logical operators have best-effort English
renderings (`is greater than`, `and`, `or`, ...) so `describe` never fails, but
they are outside the parseable surface.
