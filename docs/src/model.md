# The Improv Model

Improv models a problem as a **multidimensional cube** rather than a flat grid.
The vocabulary is small and maps directly onto the types in
`improv_core_model`.

## Categories

A **category** is a dimension of the model — for example `Time`, `Product`, or
`Region`. Each category has a stable numeric id (`CategoryId`) and a
human-readable name.

## Items

An **item** is one member of a category: `2025` and `2026` are items of `Time`;
`Widget A` and `Widget B` are items of `Product`. Each item has an id
(`ItemId`), the id of the category it belongs to, and a name.

## Measures

A **measure** is a named variable indexed by a subset of the categories — its
*tensor dimensions*. `Price` might be defined over `[Product]`, while `Quantity`
and `Revenue` are defined over `[Time, Product]`.

A measure is one of two kinds:

- **Input** — raw data you enter.
- **Derived** — computed from a [formula](./formulas.md).

Every measure declares a value type: `Number`, `Boolean`, `Text`, `DateTime`,
or `Enum`. (The engine and CLI operate on `Number`, `Boolean`, and `Text`
today; `DateTime` and `Enum` exist in the model but are not yet exercised
end-to-end.)

## Coordinates

A **coordinate** names one cell of a measure's tensor by mapping categories to
items — for example `{Time=2025, Product=Widget A}`. Internally a coordinate is
an ordered map (`BTreeMap<CategoryId, ItemId>`), so it is
insertion-order-independent: `{Time=2025, Product=WidgetA}` and
`{Product=WidgetA, Time=2025}` are the same coordinate. That ordering is what
lets the engine derive a stable dataflow key (see
[Architecture](./architecture.md)).

Input data lives in the model as a map from `(measure, coordinate)` to a value.
Only input measures have entries; derived measures are computed by the engine.

## Structure, logic, and data

The payoff of naming everything is that the three concerns move independently:

- **Rename freely.** Names are cosmetic; formulas bind to ids, not names, so
  renaming a category or measure never breaks a formula.
- **Add items without touching formulas.** Adding `2027` to `Time` extends
  every measure defined over `Time` automatically — a formula like
  `Revenue = Price * Quantity` needs no edit.
- **Edit logic in one place.** A formula is defined once per measure, over
  dimensions, instead of being copied down a column of cells.

## The canonical example

Throughout these docs the running example is a `Time × Product` revenue model:

```text
Revenue[Time, Product] = Price[Product] * Quantity[Time, Product]
```

`Price` is defined over `Product` only, so it is *broadcast* over `Time`:
every year reuses the same per-product price. With

| Product  | Price |
|----------|-------|
| Widget A | 10    |
| Widget B | 20    |

and

| Time | Product  | Quantity |
|------|----------|----------|
| 2025 | Widget A | 100      |
| 2025 | Widget B | 50       |
| 2026 | Widget A | 120      |
| 2026 | Widget B | 80       |

the engine computes:

| Time | Product  | Revenue |
|------|----------|---------|
| 2025 | Widget A | 1000    |
| 2025 | Widget B | 1000    |
| 2026 | Widget A | 1200    |
| 2026 | Widget B | 1600    |

These are the values the engine's test suite checks against.
