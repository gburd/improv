# Tutorial: a household budget on the desktop

You will build a three-month household budget: a monthly budget per spending
line, what was actually spent, and the income. Improv then works out what is
left on every line and how much was saved each month, and you open the result
in the desktop app.

The model is built with the `improv` command-line tool, one command per step,
because each step shows one idea. The same file opens in `improv-gui` and
`improv-tui`.

This page is tested: `cargo test -p improv_cli` runs every `sh` block below
in order and checks that each output block matches what the command before
it printed (`docs/tutorials/mdtest.py`, called from
`crates/cli/tests/tutorials.rs`).

You need the `improv` binary on your `PATH` (see
[Getting Started](./getting-started.md)).

## 1. Create the model file

A model lives in one file. `init` creates it:

```sh
DB=budget.db
improv init "$DB"
```

```text
initialized store at 'budget.db'
```

## 2. The categories and their items

A budget has two dimensions: the month, and the spending line. Each is a
**category**, and its members are **items**. Every object takes a numeric
id that you choose; the categories here are 1 and 2, the months 10 to 12,
the lines 20 to 23.

```sh
improv add-category "$DB" 1 Month
improv add-category "$DB" 2 Line
improv add-item "$DB" 10 1 Jan
improv add-item "$DB" 11 1 Feb
improv add-item "$DB" 12 1 Mar
improv add-item "$DB" 20 2 Rent
improv add-item "$DB" 21 2 Groceries
improv add-item "$DB" 22 2 Utilities
improv add-item "$DB" 23 2 Fun
```

## 3. The input measures

A **measure** is a quantity, laid out over some of the categories. The
budget is set once per line, so `Budget` is over `Line` only. What was spent
varies by month and line. Income is per month.

```sh
improv add-measure "$DB" 100 Budget number input Line
improv add-measure "$DB" 101 Spent  number input Month Line
improv add-measure "$DB" 102 Income number input Month
```

Now type in the numbers. `--at` names the cell's coordinate, one
`Category=Item` pair per dimension:

```sh
improv set "$DB" 100 1500 --at Line=Rent
improv set "$DB" 100 600  --at Line=Groceries
improv set "$DB" 100 200  --at Line=Utilities
improv set "$DB" 100 300  --at Line=Fun

improv set "$DB" 102 4200 --at Month=Jan
improv set "$DB" 102 4200 --at Month=Feb
improv set "$DB" 102 4500 --at Month=Mar

while read -r month line amount; do
  improv set "$DB" 101 "$amount" --at "Month=$month,Line=$line"
done <<'EOF'
Jan Rent      1500
Jan Groceries  640
Jan Utilities  230
Jan Fun        180
Feb Rent      1500
Feb Groceries  575
Feb Utilities  210
Feb Fun        420
Mar Rent      1500
Mar Groceries  590
Mar Utilities  160
Mar Fun        250
EOF
```

## 4. Formulas, written once

In a grid spreadsheet you would copy a formula into every cell. In Improv a
formula is written once, over names, and applies to every cell of its
measure. `Left` is what remains of each line's budget in each month; the
budget has no `Month`, so the same budget figure is used for every month:

```sh
improv define "$DB" 200 "Left = Budget - Spent"
improv eval   "$DB" 200
```

```text
defined derived measure 200 'Left'
eval 200 'Left' (12 cells):
  Left[Month=Feb, Line=Fun] = -120
  Left[Month=Feb, Line=Groceries] = 25
  Left[Month=Feb, Line=Rent] = 0
  Left[Month=Feb, Line=Utilities] = -10
  Left[Month=Jan, Line=Fun] = 120
  Left[Month=Jan, Line=Groceries] = -40
  Left[Month=Jan, Line=Rent] = 0
  Left[Month=Jan, Line=Utilities] = -30
  Left[Month=Mar, Line=Fun] = 50
  Left[Month=Mar, Line=Groceries] = 10
  Left[Month=Mar, Line=Rent] = 0
  Left[Month=Mar, Line=Utilities] = 40
```

`eval` lists cells sorted by item name. A negative number is an overspent
line. `OVER` adds a measure up along a
category. `SUM(Spent OVER Line)` is a month's total spending, so the
savings are:

```sh
improv define "$DB" 201 "Saved = Income - SUM(Spent OVER Line)"
improv eval   "$DB" 201
```

```text
defined derived measure 201 'Saved'
eval 201 'Saved' (3 cells):
  Saved[Month=Feb] = 1495
  Saved[Month=Jan] = 1650
  Saved[Month=Mar] = 2000
```

Formulas can use other formulas. The quarter's totals add up along `Month`:

```sh
improv define "$DB" 202 "QuarterSpent = SUM(Spent OVER Month)"
improv define "$DB" 203 "QuarterSaved = SUM(Saved OVER Month)"
improv eval   "$DB" 202
improv eval   "$DB" 203
```

```text
defined derived measure 202 'QuarterSpent'
defined derived measure 203 'QuarterSaved'
eval 202 'QuarterSpent' (4 cells):
  QuarterSpent[Line=Fun] = 850
  QuarterSpent[Line=Groceries] = 1805
  QuarterSpent[Line=Rent] = 4500
  QuarterSpent[Line=Utilities] = 600
eval 203 'QuarterSaved' (1 cells):
  QuarterSaved[] = 5145
```

## 5. Change an input

A concert in February was refunded, so `Fun` in February is 300, not 420.
Change the one cell; everything that depends on it follows:

```sh
improv set  "$DB" 101 300 --at Month=Feb,Line=Fun
improv eval "$DB" 201
improv eval "$DB" 203
```

```text
set measure 101
eval 201 'Saved' (3 cells):
  Saved[Month=Feb] = 1615
  Saved[Month=Jan] = 1650
  Saved[Month=Mar] = 2000
eval 203 'QuarterSaved' (1 cells):
  QuarterSaved[] = 5265
```

## 6. Save views for the desktop app

A **view** is a saved layout of one measure: which categories run down the
rows and across the columns. Save two, one for spending and one for what is
left:

```sh
improv save-view "$DB" 1 "Spending"  Spent --rows Line --cols Month
improv save-view "$DB" 2 "Left over" Left  --rows Line --cols Month
improv list "$DB"
```

```text
saved view 1 'Spending'
saved view 2 'Left over'
categories:
  1 Month
  2 Line
items:
  10 Jan (category 1)
  11 Feb (category 1)
  12 Mar (category 1)
  20 Rent (category 2)
  21 Groceries (category 2)
  22 Utilities (category 2)
  23 Fun (category 2)
measures:
  100 Budget [input] Number
  101 Spent [input] Number
  102 Income [input] Number
  200 Left [derived] Number
  201 Saved [derived] Number
  202 QuarterSpent [derived] Number
  203 QuarterSaved [derived] Number
views:
  1 Spending (measure Spent, 1 matrices)
  2 Left over (measure Left, 1 matrices)
inputs:
  100::{Line=Fun} = 300
  100::{Line=Groceries} = 600
  100::{Line=Rent} = 1500
  100::{Line=Utilities} = 200
  101::{Month=Feb,Line=Fun} = 300
  101::{Month=Feb,Line=Groceries} = 575
  101::{Month=Feb,Line=Rent} = 1500
  101::{Month=Feb,Line=Utilities} = 210
  101::{Month=Jan,Line=Fun} = 180
  101::{Month=Jan,Line=Groceries} = 640
  101::{Month=Jan,Line=Rent} = 1500
  101::{Month=Jan,Line=Utilities} = 230
  101::{Month=Mar,Line=Fun} = 250
  101::{Month=Mar,Line=Groceries} = 590
  101::{Month=Mar,Line=Rent} = 1500
  101::{Month=Mar,Line=Utilities} = 160
  102::{Month=Feb} = 4200
  102::{Month=Jan} = 4200
  102::{Month=Mar} = 4500
```

## 7. Open it in the desktop app

<!-- mdtest: skip -->
```sh
improv-gui budget.db
```

The model browser at the bottom left lists the two views, **Spending -
Worksheet** and **Left over - Worksheet**. Double-click one to open it: the
lines run down the rows and the months across. Click a `Spent` cell, type a
new number and press Enter; the `Left over` worksheet and the savings
update at once. The GUI saves each edit to `budget.db` as you make it, so
`improv eval` sees it straight away; Ctrl+Z undoes it. [The Desktop
GUI](./gui-tour.md) describes every window.

In a terminal, `improv-tui budget.db` opens the same file.

## What you used

| Command | Did |
| --- | --- |
| `init` | Created the model file. |
| `add-category`, `add-item` | Made the dimensions. |
| `add-measure` | Made an input measure over some categories. |
| `set` | Typed a value into one cell. |
| `define` | Added a formula, written once over names. |
| `eval` | Computed a formula's cells. |
| `save-view` | Saved a layout for the GUI and TUI. |

Every command is in the [command-line reference](./cli-reference.md). The formula
language, including `OVER` and the other reductions, is in
[The Formula Language](./formulas.md).
