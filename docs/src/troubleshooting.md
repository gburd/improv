# Troubleshooting and FAQ

This page is for anyone whose formula, command or model does not do what
they expected. Look up the message you got, or the symptom. Messages are
quoted as Improv prints them; `<…>` marks the part that changes.

## Formula errors

A formula is checked in two steps. Each has its own kind of message.

1. **Parsing** reads the text and resolves every name against the model.
   Its errors carry a **position**, a byte offset into the text you typed
   (counting from 0, the target name included): `parse error at 18:
   unknown measure: Quantty` points at the `Q`. The GUI and TUI underline
   that spot ([The Desktop GUI](./gui-tour.md#formula-errors)).
2. **Compiling** checks the types and the dimensions of the whole
   expression. Its errors have no position; they name the measures and
   categories involved. They start with `type mismatch:`, `dimension
   mismatch:` or `unsupported:`.

The grammar itself is in [The Formula Language](./formulas.md).

### Parse errors

| Message | Means | Fix |
| --- | --- | --- |
| `unknown measure: <name>` | No measure has that name. Names are case-sensitive (`price` is not `Price`). Often a typo, or a measure defined later in a script. | Check the spelling with `improv list`, or define the other measure first. |
| `unknown category: <name>` | The name after `OVER`, or a category argument, is not a category. | As above. |
| `unknown function: <name>; did you mean <a>, <b>?` | Not one of the built-in functions or the model's named LAMBDAs. | Use a suggested name, or see the [Function Reference](./functions.md). |
| `unknown qualifier: <x> (no such matrix; …)` | `X.Name` was written; there are no matrix-qualified names. | Write the name alone. |
| `expected ')'`, `expected ')' to close function call`, `expected ']' to close dimension list` | A bracket is not closed. The position is where the parser gave up, usually the end. | Close it. In the GUI, the `(` that was never closed is underlined. |
| `unexpected end of input` | The expression stops early: `1 +`. | Finish it. |
| `unexpected trailing input` | Something follows a complete expression. Common causes are two expressions side by side, a missing operator, or `1e-10` (there are no exponent literals yet). | Add the operator, or write `0.0000000001`. |
| `unexpected operator: <op>` | An operator where a value should be: `* 2`. | |
| `unexpected character: <c>` | A character that is not part of the language. | Quote a name with spaces as `'Unit Price'`. |
| `expected '=' after target measure name`, `formula must start with a target measure name` | A definition needs `Target = expression`. | `improv add-derived` takes the expression alone; `improv define` and the GUI take the whole definition. |
| `unterminated string literal`, `unterminated quoted name`, `unterminated date literal` | A `"`, `'` or `#` is not closed. | |
| `invalid date: "<text>"`, `invalid number: <text>` | `#2025-13-01#` is not a date; a number is malformed. | Dates are `#YYYY-MM-DD#`. |
| `expected '(' after function name` | A function name without its argument list. | |
| `<NAME> takes <n> argument(s), got <m>` | Wrong argument count for a fixed-arity function. The GUI spells out the parameters. | |
| `only one OVER clause per aggregation`, `only one INTO clause per call` | `SUM(X OVER A OVER B)`. | List several categories in one clause: `OVER A, B`. |
| `<NAME> does not aggregate OVER a category`, `<NAME> cannot take a window` | `OVER` given to a function that is not a reduction, or a window to one that cannot take one. | |
| `expected FIRST, LAST or THIS in a window`, `expected '..' in a window: OVER T[FIRST..THIS]` | A malformed running window. | `OVER Time[FIRST..THIS]`. |
| `FIXPOINT needs LAMBDA(r, body)`, `LAMBDA needs at least one parameter`, `<NAME> needs LAMBDA(row, body)` | A higher-order function without its LAMBDA. | |
| `expression nested too deeply` | More than the parser's nesting limit (64 nested parentheses, calls or unary operators). | Split it into intermediate measures, or `LET`. |
| `formula nests too deeply (N levels; the limit is 40)` | The formula's tree is deeper than 40 levels: a chain of more than 40 `+` terms, say, or deep nesting. See [Limits](./formulas.md#limits). | Split it into intermediate measures, or use `SUM(...)` instead of a long `+` chain. |
| `formula is too large (more than 100000 terms once LET and LAMBDA are expanded)` | `LET` and `LAMBDA` copy a value at each use, so bindings that each use the previous one twice double the formula. | Make the repeated part its own measure. |
| `empty formula`, `empty expression` | Nothing to parse. | |

A model saved before 0.11 can hold a formula past these limits:

| Symptom | Means | Fix |
| --- | --- | --- |
| A measure shows `#REF!` everywhere; its description starts `[unreadable formula]` | Its stored formula is too deep to read back (embedded store). The rest of the model loads. | Redefine it: `improv add-derived` or `improv define` with the same id. |
| `measure X: formula nests too deeply (N levels; the limit is 256); drop it …` | A Postgres model's text formula is deeper than the load bound. Nothing that builds the model works until it is fixed. | `SELECT improv.define('model', 'X = …', replace => true)` with a shallower formula, or `SELECT improv.drop_measure('model', 'X')`. |

### Compile errors

| Message (start) | Means | Fix |
| --- | --- | --- |
| `dimension mismatch: <A> ranges over [<x>] and <B> over [<y>]: they share no category, so no cell has both …` | Two operands whose categories are not a subset one of the other: `Price[Product] + Headcount[Region]`. Improv will not guess how to line them up. | Reduce one first (`SUM(Headcount OVER Region)`), or give both a common category. |
| `dimension mismatch: <M>[…] leaves out category <C>, which only an aggregation can collapse: write SUM(<M> OVER <C>) …` | A plain reference must keep all of its measure's categories: `X = Quantity[Product]`. | Say what to do with the left-out category: `SUM(Quantity OVER Time)`. |
| `dimension mismatch: <F> arguments not broadcastable: one ranges over …, another over …` | A function's arguments have unrelated categories. | As for operands. |
| `dimension mismatch: <F>: the measures reduced together must range over the same categories …` | A multi-measure reduction (`CORREL`, `SUMPRODUCT`, …) over measures with different categories. | |
| `dimension mismatch: <F>: the WHERE condition ranges over categories the measure does not` | `SUM(X OVER T WHERE Y > 0)` where `Y` has categories `X` lacks. | |
| `type mismatch: <Op> requires Number` | Arithmetic on text or booleans: `Label * 2`. | Convert (`VALUE(Label)`), or check the measure's type with `improv list`. |
| `type mismatch: IF test must be Boolean, got Number` | `IF(Price, …)`. | Compare: `IF(Price > 0, …)`. |
| `type mismatch: IF branches must have the same type, got Number and Text` | One branch is a number, the other text. | Make both the same type (`TEXT(Price, "0")`, or `NA()` for "no value"). |
| `type mismatch: <F> argument <n> must be <type>, got <type>` | A function was given the wrong type. | |
| `unsupported: <M> refers to itself; only PREV may read its earlier values …` | `Balance = Balance + Flow`. | Write the recurrence with `PREV(Balance, Time, 1, 0) + Flow`. |
| `unsupported: <F> returns a list: say where it lands, <F>(… INTO Category)` | `SORT`, `UNIQUE`, `FILTER` and similar return several values. | Add `INTO` a category for the positions ([List results](./formulas.md#list-results-into)). |
| `unsupported: <F> over a window or per item takes exactly one OVER category` | `RANK(X OVER Time, Product)`. | Rank over one category. |
| `unsupported: FIXPOINT must be a measure's whole formula …`, `FIXPOINT may not read its own measure …` | FIXPOINT inside a larger expression, or reading its target. | Define it as its own measure; use `r` for the iterate. |
| `unknown measure referenced in formula: MeasureId(<n>)` | A saved formula refers to a measure that was deleted. | Redefine the formula. |
| `formula cycle: <A> → <B> → <A>` | Measures read each other in a circle (A reads B, B reads A). In 0.11 `improv define` refuses the definition that closes the cycle (G4), as the GUI and pg_improv's `define` already do. | Break the cycle (use `PREV` if it is a recurrence). Up to 0.10.0 the CLI saved such a definition, and `eval` of a measure in the cycle then failed with `unsupported plan node in v1 dataflow builder: cyclic measure dependency at MeasureId(<n>)`. |

### Errors in values

An error is also a value: a cell can hold `#DIV/0!`, `#NUM!`, `#VALUE!` or
`#N/A`, and it flows through every formula that reads it
([Errors](./formulas.md#errors)). If a total shows `#DIV/0!`, one of the
cells it adds holds it. Find the cell with `improv eval`, and guard the
division (`IFERROR(A / B, 0)`, or `IF(B = 0, NA(), A / B)`).

## Common modeling mistakes

- **Mixing categories that do not belong together.** `Price[Product]` and
  `Headcount[Region]` do not combine, because no cell has both. If they
  should combine, the model is missing a category (Region of a Product)
  or a mapping measure. Add it, and sum over it.
- **Forgetting that a reduction removes a category.** `Total = SUM(Revenue
  OVER Time)` has no `Time`. To show the total next to each year, the
  formula broadcasts it back: `Share = Revenue / SUM(Revenue OVER Time)`.
- **Expecting a missing cell to be 0.** Where an input has no cell, a
  formula has no cell either, and a reduction skips it. Use
  `COALESCE(X, 0)` or `FILL(X, 0)` when "missing" should count as zero
  ([Missing cells](./formulas.md#missing-cells-coalesce-and-fill)).
- **Two rows for one coordinate in Postgres.** A measure bound without
  `agg` needs one row per coordinate. A second row is an error (in
  `recalc`, trigger and commit modes) or a rebuild (in logical mode). Bind
  with `agg => 'sum'` (or `count`, `avg`, …).
- **A measure that never converges.** `FIXPOINT` stops after `max_iter`
  rounds (default 100) and returns the last iterate. It never hangs, but
  it reports no error either. A body that grows without bound (`r * 2`)
  returns a huge number after `max_iter` rounds. If a result looks wrong,
  check that the step is a contraction (PageRank's damping below 1), and
  raise `max_iter` or the tolerance only after that. `PREV` recurrences
  run once along their category and always finish.
- **Exponent literals.** `1e-10` is not a number yet. Write
  `0.0000000001` or `10^-10`.
- **Per-item reductions over huge categories.** `RANK`, `SORT` and the
  other per-item reductions re-read the whole group on each edit, so an
  edit costs seconds over a million items. Keep them on smaller categories,
  or in a separate model.

## CLI errors

| Message | Means |
| --- | --- |
| `error: no measure with id <n>` | Commands take measure **ids**, not names (`improv list` shows them). |
| `error: measure <n> '<name>' is an input measure; use `show` to see its cells` | `eval` evaluates derived measures. |
| `error: no item named '<item>' in category '<cat>'` | `--at` names items that must exist; add them with `add-item`. |
| `error: measure <n> '<name>' is declared over [<cats>], but --at gave [<cats>] — missing <cat>` | `set` needs one item for every category of the measure. |
| `error: value must be a number, got '<v>'` | `set` parses the value by the measure's declared type. |
| `error: mentat error: SQL error: unable to open database file: <path> …` | The store path's directory does not exist, or is not writable. |
| `error: postgres error: a postgres model URL needs ?model=<name> …` | A Postgres target names one model: `postgres://me@db/sales?model=plan`. |
| `error: postgres error: postgres connect failed (connection error)` | The server is unreachable, or refused the connection (host, port, `pg_hba.conf`, password). The Postgres connection has no TLS yet; use a loopback port or an SSH tunnel. |
| `save failed: … conflict …` (GUI, TUI, server: 409) | Someone else saved the same model after you loaded it. Your save was refused, not merged. Reload (**File ▸ Revert to Saved**, `R` in the TUI) and redo the edit. |

Errors are printed on stderr after `error:`. `improv help` lists the
commands and their arguments.

## Why is this slow?

| Symptom | Likely cause | What to do |
| --- | --- | --- |
| Importing a large CSV (`import-csv`) or saving a large store file takes minutes | Before Improv 0.11 (Mentat 1.16.0), writing a store file was quadratic: 100k rows 21 s, 200k 91 s. Mentat 1.18.0 made it linear: 100k 7 s, 1M about 105 s. In 0.11 a cell edit saves only that cell (milliseconds at 1M cells), but opening a 1M-cell store still loads it all (about 5 s), and an import or refresh still rewrites the whole store (about 8 s at 1M cells) | Upgrade to 0.11. For models of millions of cells that change often, keep the data in Postgres or DuckDB, bound with `agg`. |
| Opening a model is slow | Every open loads and evaluates the whole model (about 2–2.5 µs per cell to evaluate, plus the load) | Normal for millions of cells; see [capacity](./capacity.md). |
| Every edit takes 100 ms or more on a model with thousands of measures | Each edit is one dataflow round that visits every tracked measure (129 ms at 20k simple measures) | Split the model, or remove unused measures. |
| One edit takes seconds | A `RANK`/`SORT`/per-item reduction over a very large category, or an edit that changes a `FIXPOINT`'s input (it re-solves) | Move it to a smaller category or a separate model. |
| The first open after adding many items is slow, later edits fast | A rebuild, because a formula reads the category's item list | Expected; see [Concepts](./concepts.md#incremental-recalculation). |
| Postgres: writes got slower after `set_sync_mode` | `trigger` or `commit` mode makes the writer pay for the recompute (0.4 s per statement / 2.2 s per transaction on 2.5M rows) | Use `logical` ([When to use](./when-to-use.md#high-write-tables-in-trigger-or-commit-mode-use-logical-mode)). |
| Postgres: logical results lag by seconds | One worker applies every model in turn; a slow model or a rebuild delays all | [Operations runbook](./runbook-ops.md#a-slow-apply). |
| Postgres: `recalc` or a commit-mode commit is slow | Both reload the bound tables in full (about 1 s per million fact rows) | Expected; use logical mode for large tables. |
| DuckDB: `improv()` is slow on many distinct coordinates | It builds one engine cell per distinct coordinate (2.5M distinct cells: 4.5 s) | Bind with an aggregate so DuckDB folds rows first (2.5M rows into 44,750 cells: 0.4 s). |

[plan]: https://codeberg.org/gregburd/improv/src/branch/main/docs/reviews/2026-10-21-road-to-1.0.md
