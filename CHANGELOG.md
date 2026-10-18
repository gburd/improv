# Changelog

All notable changes to Improv are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

Phases P1–P7 of the built-in functions plan: 674 functions (plus
aliases) with Excel's names, typed errors, the `IF` family,
positional forms and recurrences, lookups, list results, `LET` and
`LAMBDA`, and locale-aware entry.

### Added

- **TUI function help.** `?` (or F1, also while editing) opens a search
  over the built-in functions: the matches with their signatures, and the
  selected one's category, aliases and description. Enter inserts
  `NAME(` into the cell being edited.
- **pg_improv `commit` sync mode** (pg_improv 0.10.0;
  `improv.set_sync_mode(model, 'commit')`). Results commit or roll back
  with the writer's transaction, as in `trigger` mode, but nothing is
  installed on your tables, and the model recomputes once per transaction,
  just before COMMIT (or PREPARE), not once per statement: 100 one-row
  updates in one transaction on a 10k-cell model commit in 82 ms against
  6.6 s in trigger mode. A read before COMMIT sees the old results
  (`improv.recalc(model)` in the transaction reads your writes), and a
  recompute that fails aborts the COMMIT with an error naming the model.
  Needs `pg_improv` in `shared_preload_libraries` (or
  `session_preload_libraries`); `set_sync_mode` refuses it otherwise. As
  with trigger mode, the role that enables it needs the TRIGGER privilege
  on the bound tables, and every writer must be able to write the result
  tables. `ALTER EXTENSION pg_improv UPDATE` from 0.9.0 adds it.

- **Distributions** (phase P7, 72): Gnumeric's `BERNOULLI`, `CAUCHY`,
  `EXPPOWDIST`, `GEOMDIST`, `LANDAU`, `LAPLACE`, `LOGISTIC`, `PARETO`,
  `RAYLEIGH`, `RAYLEIGHTAIL`, `SNORM.DIST.RANGE`, `OWENT`,
  `CUM_BIV_NORM_DIST`; and the R-style `R.D*`/`R.P*`/`R.Q*` for BETA,
  BINOM, CAUCHY, CHISQ, EXP, F, GAMMA, GEOM, GUMBEL, HYPER, LNORM, NBINOM,
  NORM, POIS, RAYLEIGH, SNORM (skew-normal), ST (skew-t), T and WEIBULL,
  plus `R.PTUKEY`/`R.QTUKEY`, with R's `lower_tail` and `log_p` flags. 456
  goldens (Gnumeric 1.12.57; mpmath where Gnumeric is wrong: its bivariate
  normal is ½ too high for some sign combinations, `R.DSNORM` is wrong off
  location 0, `R.PST` ignores `log_p`).

- **Analysis reductions** (phase P7, 30): matrices as a measure over two
  categories (`MDETERM(A OVER R, C)`, `LOGMDETERM`, `MINVERSE`,
  `MPSEUDOINVERSE`, `CHOLESKY`, `EIGEN`, `LEVERAGE`; results row by row,
  `INTO R2, C2` for a block); time series (`HPFILTER`, `INTERPOLATION`,
  `FOURIER`, `PERIODOGRAM`); `SSMEDIAN`, `CRONBACH`, `LOGREG`, `LOGFIT`;
  the normality tests `ADTEST`, `CVMTEST`, `LKSTEST`, `SFTEST` and their
  `.STAT` statistics; `FORECAST.ETS` (`.ADD`), `.MULT`, `.STAT`,
  `.STAT.MULT`, `.SEASONALITY`, a port of LibreOffice's Holt–Winters, and
  `.CONFINT`/`.PI.MULT` for seasonality 0. 289 goldens (Gnumeric 1.12,
  LibreOffice), most to 1e-12.
- **Operands that share a category join over it**: `A[R, K] * B[K, C]`
  spans R × K × C, so a matrix product is `AB = A * B` then `SUM(AB OVER
  K)` (in place of `MMULT`). Measures with no category in common are still
  refused.

- **Math extras** (phase P7, 46): `LAMBERTW`, `AGM`, `GD`, `ILOG`,
  `REDUCEPI`, `FIB`; number theory (`ISPRIME`, `ITHPRIME`, `PFACTOR`,
  `NT_D`, `NT_SIGMA`, `NT_PHI`, `NT_MU`, `NT_OMEGA`, `NT_PI`, `NT_RADICAL`,
  `PARTITIONS`); `FLT.MAX`, `FLT.MIN`, `FLT.RADIX`, `FLT.NEXTAFTER`; the
  movable feasts (`EASTERSUNDAY`, `ASCENSIONTHURSDAY`, `ASHWEDNESDAY`,
  `GOODFRIDAY`, `PENTECOSTSUNDAY`); `DATE2JULIAN`, `HDATE_JULIAN` and the
  Hebrew calendar (`HDATE`, `HDATE_HEB`, `HDATE_DAY`, `HDATE_MONTH`,
  `HDATE_YEAR`, `DATE2HDATE`, `DATE2HDATE_HEB`); Erlang-B (`PROBBLOCK`,
  `OFFTRAF`, `DIMCIRC`, `OFFCAP`); text (`ASC`, `JIS`/`DBCS`, `BAHTTEXT`,
  `ROT13`, `ENCODEURL`, `ROMAN`, `ARABIC`). Bounded so each call finishes
  in about 2 ms. 303 goldens.

- **Engineering functions** (phase P7, 72, a new *Engineering* category):
  base conversion (`BIN2DEC` … `OCT2HEX`, `BASE`/`NUMTOBASE`,
  `DECIMAL`/`BASETONUM`, `HEXREP`); 48-bit `BITAND`/`BITOR`/`BITXOR`/
  `BITLSHIFT`/`BITRSHIFT`, `DELTA`, `GESTEP`; 45 complex-number functions
  over `"a+bi"` text, with Gnumeric's inverse trigonometric and hyperbolic
  ones, `IMFACT`, `IMGAMMA`, `IMIGAMMA`; `BESSELI`/`J`/`K`/`Y`; `CONVERT`
  with Excel's unit table and SI/binary prefixes. 530 goldens (LibreOffice,
  Gnumeric, Excel's docs).

- **Option pricing** (phase P7, 32 functions): Gnumeric's `OPT_BS` and its
  Greeks (`_DELTA`, `_GAMMA`, `_RHO`, `_THETA`, `_VEGA`, `_CARRYCOST`),
  Garman–Kohlhagen, French, Merton jump diffusion, Miltersen–Schwartz,
  Barone-Adesi–Whaley, Bjerksund–Stensland, Roll–Geske–Whaley, a binomial
  tree, executive, forward-start, time-switch, simple and complex chooser,
  compound, writer-extendible, two-asset correlation, exchange (European
  and American), Kirk's spread, fixed- and floating-strike lookbacks; and
  LibreOffice's `OPT_BARRIER`, `OPT_TOUCH`, `OPT_PROB_HIT`,
  `OPT_PROB_INMONEY`. 200 goldens from Gnumeric 1.12.57 and LibreOffice;
  where Gnumeric's bivariate normal is wrong, mpmath.

- **More list and LAMBDA forms.** `INTO R, C` lays a list over several
  categories row by row (Excel's 2-D spill); `LINEST`/`LOGEST` take any
  number of x's (`LINEST(y, x1, x2 OVER T, TRUE, TRUE INTO Row, Col)`
  gives Excel's 5 × (k + 1) block, checked against LibreOffice);
  `SORTBY` takes several by/order pairs; `SELECTS(values, tests OVER T,
  condition)`; `SELECTLIST` (`SELECT` with a lookup over its own
  category); `MAKEARRAY(R, C, LAMBDA(r, c, body))`; `SEQUENCE(T, [start],
  [step])`; `BYROW`/`BYCOL(x OVER C, LAMBDA(row, SUM(row)))`. An
  expression that reads no measure (positions, literals) now broadcasts
  with anything, so `POSITION(R) * 10 + POSITION(C)` spans R × C.
- **Named LAMBDAs.** `improv define model.db 1 "Margin = LAMBDA(p, c, p -
  c)"`, then `Margin(Price, Cost)` in any formula. Calls are stored as
  calls and expanded when compiled, so redefining a LAMBDA changes every
  use; a body reads only its parameters and earlier LAMBDAs (never
  itself). Saved in the Mentat (`:meta/lambdas`, installed over older
  stores on open) and Postgres (`improv.meta`) stores; the GUI and CNL
  print calls by name.

- **`LET`, `LAMBDA`, `MAP`, `SCAN` and `REDUCE`.** LET names values for its
  body; a LAMBDA is called on the spot or passed to MAP (cell by cell),
  SCAN (a running fold along a category, `SCAN(0, x OVER Time,
  LAMBDA(acc, v, acc + v))`) or REDUCE (its last value). Folds compile to
  recurrences and update incrementally; a LAMBDA cannot recurse, so every
  formula still terminates.

- **List results (array functions)** land along a category named with
  `INTO`: `Top = SORT(Sales OVER Product, -1 INTO Rank)` puts the largest
  value at Rank's first item: `SORT`, `UNIQUE`, `FILTER`, `SORTBY`,
  `TAKE`, `DROP`, `MODE.MULT`, `LINEST` and `LOGEST` (one x; with stats,
  Excel's 5×2 block row by row), `TEXTSPLIT` (each cell its own list, no
  OVER needed) and `FREQUENCY` (bins as separate numbers,
  `FREQUENCY(x OVER T, 10, 20 INTO Bin)`). Excel's empty-array `#CALC!`
  is `#N/A`.
- **Lookups** (phase P6), reductions whose key is a parameter (a key over
  another category gives one lookup per key cell): `XLOOKUP`, `XMATCH`,
  `MATCH`, `LOOKUP`, `INDEX` (`VALUEAT`), Quantrix's `SELECT`,
  `SELECTBETWEEN`, `SELECTGREATERTHAN`, `SELECTLESSTHAN`; the database
  functions as aggregations with `WHERE` (`DSUM`, `DAVERAGE`, `DCOUNT`,
  `DCOUNTA`, `DMAX`, `DMIN`, `DPRODUCT`, `DSTDEV`, `DSTDEVP`, `DVAR`,
  `DVARP`, and `DGET`); `ARRAYTOTEXT`.
- **Lookups' shape:** an aggregation's parameter may range over categories
  the reduced measure lacks; the result gains them and each cell reads the
  whole table (`TEXTJOIN(Sep, TRUE, x OVER Product)` with `Sep` per Time).

- **72 statistical reductions** (phase P4), each `F(x OVER T, …)`:
  - descriptive: `COUNT`, `COUNTA`, `COUNTBLANK`, `SUMA`, `AVERAGEA`,
    `MAXA`, `MINA`, `AVEDEV`, `MEDIAN`, `MODE` (`MODE.SNGL`), `GEOMEAN`,
    `HARMEAN`, `TRIMMEAN`, `DEVSQ`, `STDEV`/`STDEVP`/`STDEVA`/`STDEVPA`,
    `VAR`/`VARP`/`VARA`/`VARPA` (and the `.S`/`.P` spellings), `KURT`,
    `KURTP`, `SKEW`, `SKEWP`, `LARGE`, `SMALL`, `PERCENTILE`(`.INC`/`.EXC`),
    `QUARTILE`(`.INC`/`.EXC`), `PROB`, `SUBTOTAL` and `AGGREGATE`;
  - conditional, with Excel's criteria (`">5"`, `"<>x"`, wildcards
    `*`/`?`/`~`): `COUNTIF`, `COUNTIFS`, `SUMIF`, `SUMIFS`, `AVERAGEIF`,
    `AVERAGEIFS`, `MAXIFS`, `MINIFS`;
  - sums of products: `SUMPRODUCT`, `SUMX2MY2`, `SUMX2PY2`, `SUMXMY2`,
    `SERIESSUM`, `POLYNOMIAL`, `PERCENTOF`;
  - per item (the result keeps the category): `RANK` (`RANK.EQ`),
    `RANK.AVG`, `PERCENTRANK` (`.INC`/`.EXC`), `TREND`, `GROWTH`;
  - regression and tests: `CORREL` (`PEARSON`), `RSQ`, `COVAR`
    (`COVARIANCE.P`), `COVARIANCE.S`, `SLOPE`, `INTERCEPT`, `STEYX`,
    `FORECAST` (`FORECAST.LINEAR`), Quantrix's `EXPA`, `EXPB` and
    `EXPFORECAST`, `CHITEST`, `FTEST`, `TTEST`, `ZTEST` (and their dotted
    spellings), and `AUTOCORREL` (GSL's lag-1 autocorrelation).
- **Positions, neighbours, running totals and recurrences** (the plan's
  phase P3). `POSITION(T)`, `@ITEM(T)`, `@ID(T)`, `@ITEMCOUNT(T)`;
  `PREV(x, T, [n], [default])` and `NEXT`; windows `SUM(x OVER
  T[FIRST..THIS])`, `AVERAGE(x OVER T[THIS-2..THIS])`; and recurrences, a
  measure that reads its own earlier values (`Balance = PREV(Balance, Time,
  1, Opening) + Deposits`), computed as an incremental scan in item order.
- **Aggregations grow up:** several `OVER` categories (`SUM(x OVER Time,
  Region)`), `OVER ALL EXCEPT Region`, `WHERE` filters, parameters (one
  value per group, literal or a measure), reductions of several measures at
  once, and rows in item order. New: `FIRST`, `LAST`, `S`, `TEXTJOIN`.
- **Date arithmetic** in days: a date plus a number is a date, the
  difference of two dates a number of days.
- New measures (`improv add-derived` / `define`, the GUI's formula list,
  pg_improv `define`) take their type and categories from the engine, so a
  text- or date-valued formula is a text or date measure, and an
  aggregation's result drops the categories it reduces.

- **118 built-in functions**, with Excel's names (the plan's phase P1):
  - the logical family `IF`, `IFS`, `SWITCH`, `CHOOSE`, `IFERROR`, `IFNA`,
    `CLEARERROR`, `CLEARZERO` and `BETWEEN`, all lazy: an error or a missing
    cell in a branch `IF` does not choose never shows;
  - the error functions `ISERROR`, `ISERR`, `ISNA`, `NA`, `ERROR` and
    `ERROR.TYPE`;
  - 60 math and trigonometry functions (`POWER`, `MOD`, `LN`, `LOG`,
    `SIN`…`ACOTH`, `FACT`, `COMBIN`, `GCD`, `HYPOT`, …);
  - 17 rounding functions (`ROUND(x, digits)`, `ROUNDUP`, `TRUNC`,
    `CEILING`, `FLOOR.MATH`, `MROUND`, `EVEN`, …), decimal-correct
    (`ROUND(2.675, 2)` is 2.68);
  - 26 text functions (`LEFT`, `MID`, `FIND`, `SEARCH` with wildcards,
    `SUBSTITUTE`, `TEXTBEFORE`, `CONCAT`, `PROPER`, `VALUE`, …), counting
    characters, not bytes.
- **The date family, 66 functions** (phase P2):
  - information: `ISBLANK`, `ISNUMBER`, `ISTEXT`, `ISNONTEXT`, `ISLOGICAL`,
    `ISDATE`, `ISNUMBERORDATE`, `ISEVEN`, `ISODD`, `N`, `T`, `TYPE`;
  - text formatting: `TEXT` with Excel format codes, `FIXED`, `DOLLAR`,
    `NUMBERVALUE`, `VALUETOTEXT`; regular expressions `REGEX`, `REGEXTEST`,
    `REGEXEXTRACT` (first match only) and `REGEXREPLACE`, in the Rust
    `regex` syntax;
  - date and time: `DATE`, `TIME`, `DATEVALUE`, `TIMEVALUE`, the date and
    time parts, `WEEKDAY`, `WEEKNUM`, `ISOWEEKNUM`, `DAYS360`, `DATEDIF`,
    `EDATE`, `EOMONTH`, `YEARFRAC`, `NETWORKDAYS`/`WORKDAY` and their
    `.INTL` forms (no holidays argument yet), `NOW`, `TODAY`, LibreOffice's
    `DAYSINMONTH`…`YEARS`, Gnumeric's `UNIX2DATE`/`DATE2UNIX`, and Numbers'
    `DUR2*` over durations as days.
- **Statistical functions** (the plan's phase P4, scalars): 10 special
  functions (`GAMMA`, `GAMMALN`, `BETA`, `BETALN`, `DIGAMMA`, `IGAMMA`,
  `POCHHAMMER`, `ERF`, `ERF.PRECISE`, `ERFC`) and 44 distributions under
  Excel's names, with legacy names as aliases where they mean the same
  thing (`NORMDIST` = `NORM.DIST`) and their own entries where they do not
  (`NORMSDIST`, `TDIST`, `BETADIST`, `HYPGEOMDIST`, …): the beta, binomial,
  chi-square, exponential, F, gamma, hypergeometric, log-normal, negative
  binomial, normal, Poisson, Student's t and Weibull distributions and
  quantiles, plus `GAUSS`, `PHI`, `FISHER`, `FISHERINV`, `CONFIDENCE.NORM`,
  `CONFIDENCE.T` and `STANDARDIZE`. 326 more golden values
  (`scripts/goldens/statistical.py`). Dotted names whose prefixes are not
  functions themselves (`NORM.S.DIST`, `T.DIST.2T`) parse.
- **More aggregations:** `PRODUCT`, `SUMSQ`, and over booleans `AND`, `OR`
  and `XOR`; `AVERAGE` (with `AVG` as an alias). `SUM`, `MIN`, `MAX` and the
  rest also take a list of arguments (`SUM(a, b, c)`).
- **Optional and repeating arguments** (`ROUND(x)` or `ROUND(x, 2)`), and a
  parse error that shows the signature.
- **One function registry** (`improv_core_model::functions`) drives the
  parser, the type checker, the engine, the GUI's formula text, the CNL and
  a generated **Function Reference** page in the guide.
- 634 golden values, checked through both the scalar evaluator and the
  dataflow engine: from LibreOffice 25.8 headless
  (`scripts/goldens/p1.py`), or from the Excel, Gnumeric, Numbers and
  Quantrix documentation where LibreOffice lacks a function or disagrees
  with Excel.
- **49 financial functions** (the plan's phase P5 scalars), with Excel's
  sign convention (cash paid out is negative):
  - time value of money: `FV`, `PV`, `PMT`, `IPMT`, `PPMT`, `NPER`, `RATE`
    (Newton's method, then bisection; #NUM! without a root), `CUMIPMT`,
    `CUMPRINC`, `ISPMT`, `EFFECT`, `NOMINAL`, `PDURATION`, `RRI`, `DOLLARDE`
    and `DOLLARFR`;
  - depreciation: `SLN`, `SYD`, `DB`, `DDB`, `VDB`, and the French
    `AMORDEGRC` and `AMORLINC`;
  - bonds and securities, on the five day-count bases: `ACCRINT`,
    `ACCRINTM`, the `COUP*` family (`COUPNCD`/`COUPPCD` return dates),
    `DISC`, `DURATION`, `MDURATION`, `INTRATE`, `ODDFPRICE`, `ODDFYIELD`,
    `ODDLPRICE`, `ODDLYIELD`, `PRICE`, `PRICEDISC`, `PRICEMAT`, `RECEIVED`,
    `TBILLEQ`, `TBILLPRICE`, `TBILLYIELD`, `YIELD`, `YIELDDISC` and
    `YIELDMAT`.
  Day counting follows LibreOffice; where LibreOffice departs from Excel's
  documented results (`DURATION` on actual/actual, `PRICE`/`YIELD` with one
  coupon left, `EFFECT(0, n)`, `DOLLARDE`/`DOLLARFR` below 1, `ACCRINT`'s
  calc_method), Improv follows Excel. 457 golden values from LibreOffice
  25.8 and Gnumeric 1.12 (`scripts/goldens/financial.py`; Gnumeric for
  `ODDFPRICE`/`ODDFYIELD`, which LibreOffice cannot compute).
- **Cash-flow reductions** over an ordered category, in item order:
  `NPV(rate, cf OVER Time)`, `XNPV(rate, cf, dates OVER Time)`,
  `IRR(cf OVER Time, [guess])`, `XIRR(cf, dates OVER Time, [guess])`
  (Newton's method, then bisection; #NUM! without a sign change or a root),
  `MIRR(cf OVER Time, finance_rate, reinvest_rate)`,
  `FVSCHEDULE(principal, rate OVER Time)`, and GNU Oleo's financial
  management rate of return `FMRR(cf OVER Time, safe_rate, reinvest_rate,
  reinvest_min)`. A period with no cell is skipped, not zero (Excel's rule
  for blanks in a range); an error flow is the result. 65 golden values
  from LibreOffice 25.8 over a column of flows, and from a transliteration
  of Oleo's `do_fmrr` (`scripts/goldens/financial_reduce.py`).
- `improv.raise_errors` (pg_improv): raise a `LANGUAGE improv` error value
  instead of returning NULL.

- **Operators `^`, `&` and `%`.** `^` is power with the mathematical
  convention: right-associative and tighter than unary minus (`2^3^2` is
  512, `-2^2` is −4; the spreadsheets give 64 and 4). `&` joins two values
  as text, below `+`/`-` and above the comparisons. `%` divides by 100.

- **Formulas in your language.** The GUI formula bar and `improv define` /
  `add-derived` read formulas in the formula locale (`IMPROV_LOCALE`, else
  the OS locale); in German: LibreOffice's German names, a decimal comma and
  `;` separators (`WENN(Price > 1,5; RUNDEN(Price; 2); 0)`). The GUI shows
  formulas the same way. Models store the English form.

### Changed

- **Dimension errors say what and why**, by name: `A ranges over [Month]
  and B over [Region]: they share no category, so no cell has both …
  Reduce one first, e.g. SUM(B OVER Region), or give them a category in
  common`, instead of `[CategoryId(1)] vs [CategoryId(2)]`. The same for
  function arguments, IF, `[…]` that leaves a category out, and measures
  reduced together.

- **Mentat 1.16.0, from crates.io.** The storage engine is now the
  published `mentatdb` crate (`mentat = { package = "mentatdb", version =
  "=1.16.0" }`), no longer a git dependency on Codeberg; cargo-deny drops
  its git source and the per-crate license clarifications. No code
  changes; storage speed is unchanged (A/B benchmark:
  `docs/reviews/2026-10-18-mentat-1.16-upgrade.md`).

- **Errors are typed values**, shown as the spreadsheets show them:
  `#DIV/0!`, `#NUM!`, `#VALUE!`, `#N/A` (plus `#NULL!`, `#NAME?`, `#REF!`
  from `ERROR`). Division by zero is `#DIV/0!` and a domain error or
  overflow `#NUM!`, where they used to be `NaN`; an error operand
  propagates. `LANGUAGE improv` returns NULL for an error value (it used to
  return `NaN`), or raises it under `improv.raise_errors`.
- `MIN2`/`MAX2` are now aliases of `MIN`/`MAX`, which take any number of
  arguments.
- Comparisons order text, dates and booleans, not only numbers.
- **`NOT` binds looser than a comparison**, as in SQL: `NOT Price > 15` is
  `NOT (Price > 15)`. It used to parse as `(NOT Price) > 15`, a type error.
  Only a formula comparing a `NOT` of a boolean measure with `<`, `<=`, `>`
  or `>=` changes meaning (`NOT a == b` and `NOT a <> b` give the same
  values either way); write `(NOT a) < b` to keep the old reading.

### Fixed

- **Saving a partially loaded Mentat model no longer deletes data.** A
  model from `load_partial` saved through its handle used to retract every
  cell outside the loaded closure; the save now only retracts the closure's
  missing cells, as the Postgres store already did.
- **The TUI edits text, boolean and date cells incrementally**, as it does
  numbers (through `session::input_cell`, like the GUI); it used to rebuild
  the whole engine for every non-number edit.
- **`Measure[Cat]` outside an aggregation is refused** when the list leaves
  out one of the measure's categories. It used to change only the type the
  checker saw: `X = Quantity[Product]` still produced every Time×Product cell.
  Inside an aggregation (`SUM(Quantity[Product] OVER Time)`) it works as
  before.

### Security

- **duckdb 1.10506.1** (was 1.10505.0) moves its build-time `zip`
  dependency from 6.0.0 to 9.0.4, fixing RUSTSEC-2026-0367, -0368 and -0369
  (SUID bits, symlink targets and permissions applied during ZIP
  extraction). It only ever unpacked DuckDB's own archive at build time,
  but it no longer needs a waiver.

## [0.9.0] - 2026-10-16

pg_improv is ready for real databases: it builds and is tested on
Postgres 15, 16, 17 and 18; it upgrades in place with ALTER EXTENSION;
pg_dump keeps its models; it installs without the preload; and it is
published to PGXN, with binary packages per Postgres major. A worked
family-finance example shows a household's accounts, statement
reconciliation, budgets, net worth and savings goals in one model.

### Added

- **pg_improv on Postgres 15, 16, 17 and 18** (pgrx features `pg15`…`pg18`;
  17 stays the default). CI runs the whole suite on each.
- **Upgrades:** `sql/pg_improv--0.8.0--0.9.0.sql` and `--0.8.1--0.9.0.sql`,
  so `ALTER EXTENSION pg_improv UPDATE` moves a live database forward.
  `tests/upgrade.sh` builds each earlier release, puts a logical-mode model
  on it, upgrades, and checks results, the worker, `define` and `pg_dump`.
- **`tests/txn.sh`:** the transactional guarantees, end to end. In trigger
  mode a writer reads its own results, other sessions see them only after
  commit, and ROLLBACK, an error or a rolled-back savepoint undo them. In
  logical mode aborted work never reaches the results. Also checks a
  no-preload `CREATE EXTENSION`, that every object is in schema `improv`,
  and dump/restore.
- **PGXN:** `crates/pg_improv/META.json.in` and `ci/pgxn-dist.sh`, which
  builds a buildable source dist (the extension and the six crates it links)
  that CI builds on every push and the release uploads with `pgxn-release`.
- **Release packages:** `pg_improv-<ver>-pg<N>-x86_64-linux.tar.gz` for each
  supported major, from `cargo pgrx package` against that major's PGDG build.
- **Release announcements:** `ci/announce.sh` drafts and submits the
  postgresql.org news item (which feeds pgsql-announce) for feature and
  major releases. A patch release is skipped unless its CHANGELOG section
  has `### Security`.
- **CI:** `ci/pg_improv.sh <PG>`, one script for GitHub, Forgejo and a
  local run: fmt, clippy, `#[pg_test]`, pgspot, `txn.sh`, `logical.sh`,
  and on 17 also `upgrade.sh` and the PGXN dist build.
- **Family-finance example** (`crates/pg_improv/examples/family-finance`,
  guide chapter *Improv in Postgres: a family's finances*).

### Fixed

- **pg_improv: `CREATE EXTENSION` without `shared_preload_libraries` killed
  the session.** `_PG_init` defined the postmaster-level GUC
  `improv.database` on every load; Postgres refuses that after startup with
  a FATAL. It is now defined only when preloading.
- **pg_improv: `pg_dump` lost every model.** The `improv.*` catalog was not
  registered as extension configuration, so a dump held the result tables
  but restored the models empty. The catalog and its sequence are now
  dumped, and a restored database's worker creates a new slot and catches
  up instead of trusting the dumped slot position.
- **pg_improv: binding a view gave an obscure error and left the model half
  bound.** Logical and trigger sync need a table's row changes, so
  `bind_measure` and `set_sync_mode` now refuse a view (or materialized or
  foreign table) up front with a message saying to bind a table or use manual
  mode. Views still bind in manual mode.
- **pg_improv on PG 18:** `HeapTupleHeaderGetRawXmin` is ambiguous there
  (PG 18 exports a C function of that name), which broke the build.

### Security

- **pg_improv's install and update scripts pass pgspot.** `improv.mint_items`
  pins `search_path`, and the extension creates its objects with plain
  `CREATE` instead of `IF NOT EXISTS` / `OR REPLACE`, so it cannot adopt an
  object another role created first. CI runs pgspot on every change.

## [0.8.1] - 2026-10-15

### Fixed

- **pg_improv: a measure defined after the sync worker caught up is
  computed at once.** In logical mode the worker rebuilt only when a
  decoded transaction touched a bound row, and `improv.define()` changes
  the model, not the data, so the new result table stayed empty until the
  next write to the fact table, with no error in `improv.status`. The
  worker now checks `model_version` on every poll. `logical.sh` covers a
  `define` with no data change.

## [0.8.0] - 2026-10-15

Improv runs inside Postgres, and a formula can be a SQL function. The
`pg_improv` extension models your own tables and keeps results current
incrementally (at 100k cells, a one-row change is visible about 50 ms after
commit); `LANGUAGE improv` writes SQL functions in the formula language;
the desktop and terminal apps open a Postgres-hosted model by URL and edit
its rows. The GUI now follows NeXTSTEP Lotus Improv closely, scrolls wide
grids sideways and can revert after a save conflict. A security review's
findings are fixed, and every crate is held at 90% function and branch
coverage.

### Added

- **Documentation.** A *Ways to Use Improv* chapter (GUI, TUI, CLI,
  streaming, Postgres, `LANGUAGE improv`, server: what each is for and how
  to start), a refreshed introduction, getting-started and running pages,
  and man pages for `improv-tui`, `improv-gui` and `improv-server`
  (`improv(1)` gains `stream`, `save-view`, the CSV and refresh commands).
  The guide and API docs publish to Codeberg Pages and GitHub Pages.
- **Demo movies:** the README and guide show the GUI, the TUI and Improv in
  Postgres in motion, each recorded by a script under `docs/demo/`.
- **Release binaries** for Linux, macOS (arm64 and x86_64) and Windows: the
  CLI, TUI, GUI and server, with the man pages.
- **GUI: grids scroll sideways.** A worksheet wider than its window has a
  NeXT horizontal scroller in the bottom strip, right of the row tiles, with
  the arrows grouped at its left end (spec §4.4). Before, the far columns
  could not be reached.
- **Revert to Saved** after a save conflict: GUI *File ▸ Revert to Saved*
  and TUI `R` reload the model from the store (with a fresh save token),
  picking up the other writer's changes, so the conflicting edit can be
  redone. Undo history is dropped; a failed reload changes nothing.
- **`LANGUAGE improv`: SQL functions written in Improv's formula language**
  (`crates/pg_improv`). `CREATE FUNCTION margin(price float8, cost float8)
  RETURNS float8 LANGUAGE improv AS $$ price - cost $$`. A trusted language:
  arguments are named inputs, `Name = expr` lines define intermediates, the
  last line is the result, and the DSL and controlled English both work.
  Numbers, booleans, text and dates/timestamps; NULL in gives NULL out where
  the result depends on it; division by zero is `NaN`; aggregations are
  rejected. Validated at `CREATE FUNCTION`; compiled once per backend
  (0.14 µs per call over inline SQL). Evaluation is the new
  `improv_engine::scalar`, which shares the dataflow engine's per-cell
  operators.

- **pg_improv: Improv models over Postgres data** (`crates/pg_improv`,
  `crates/pg_model`, `crates/storage_pg`, `crates/store`). Bind fact-table
  columns to categories and measures, define formulas, read results from
  `improv_<model>.<measure>` tables. Three update paths: **logical decoding**
  (default; a background worker tails a slot and applies each batch with the
  slot advance in one transaction), **triggers** (in the writer's
  transaction), and **`improv.recalc()`** (explicit, in your transaction);
  plus `improv.what_if` and `improv.status`. The GUI, TUI, CLI and server
  open `postgres://…?model=<name>`. Measured on a c7i.4xlarge at 100k cells:
  logical keeps a writer's UPDATE at 8 ms (results ~50 ms later); a trigger
  makes them visible at commit for ~1.1 s per write. See
  `docs/src/postgres.md`.

- **pg_improv: aggregated bindings.** `improv.bind_measure(…, agg => 'sum')`
  (or `avg`, `min`, `max`, `count`) folds a fact table's rows per coordinate
  into one input cell, in `recalc`, trigger and logical mode and in the GUI's
  load; without `agg` duplicate rows remain an error. The GUI's store refuses
  to save an edit to an aggregated measure (`Integrity`).
- **pg_improv: boolean, text and date measures.** `bind_measure` takes the
  measure's type from its column (`bool`; `text`/`varchar`/`char`;
  `date`/`timestamp`/`timestamptz`; the number types as before) and refuses
  others; `define` accepts boolean, text and datetime results, and result
  tables get a matching `value` column (`boolean`, `text`, `timestamptz`).
  Logical mode applies their row changes cell by cell like numbers' (the
  worker reads the slot's text with `TimeZone = UTC`, `DateStyle = ISO`).

- **`pg_improv` Postgres extension, layer L0** (in progress). A pgrx 0.17
  extension (`crates/pg_improv`, pg17) with an `improv.*` catalog that is a
  superset of `improv_storage_pg`'s, plus `improv.create_model`,
  `bind_category`, `bind_measure`, `define`, `recalc` and `drop_model`.
  `recalc` recomputes every derived measure in the caller's transaction and
  writes only the changed rows to `improv_<model>.<measure>`. The new GUC
  `improv.max_cells` (default 5M) caps the input size. The Postgres-free
  logic is the new workspace crate `improv_pg_model`. Install and quick
  start: `crates/pg_improv/README.md`.
- **`pg_improv` layer L1: trigger-mode sync.**
  `improv.set_sync_mode(model, mode)` installs (for `trigger`) or removes
  statement-level `AFTER INSERT/UPDATE/DELETE/TRUNCATE` triggers with
  transition tables on every bound relation. `improv.on_change` recomputes,
  inside the writer's transaction, only the derived measures that depend on
  the changed inputs. It restricts the recompute to the touched coordinates
  when the chain is element-wise. Also new: `improv.recalc(model, measures
  text[])` recomputes a measure subset and its dependents, and
  `improv.what_if(model, edits jsonb)` returns the derived cells an in-memory
  edit would change, without writing anything. README has the
  trigger/logical/recalc trade-off table.

- **Postgres-hosted models** (pg_improv layer L3). The CLI, TUI, GUI and
  server open `postgres://…?model=<name>` as well as a Mentat file, through
  the new `improv_store::Store` seam. The new `improv_storage_pg::PgModelStore`
  keeps the model in the `improv.*` catalog: bound input measures read and
  write their fact table, unbound cells live in `improv.cell`, and
  `improv.model_version` drives the existing reload-and-retry conflict check.
  `scripts/gui-shot.sh` takes `GUI_SHOT_DB` to capture such a model.

- **Coverage gate.** `scripts/coverage.sh` (and a `coverage` CI job on
  GitHub and Forgejo) fails when any crate drops below 90% of functions or
  branches; the workspace stands at about 96% / 95%. New guide
  `docs/src/running.md`: build, run each front end, security defaults, tests,
  coverage.

- TUI: Shift+arrows select a range, drawn highlighted, and the status line
  shows its `Sum` (numeric cells only; `Sum —` when there are none), as the
  GUI does. A plain arrow, a click, or any re-layout collapses the range.
- **GUI screenshot regression in CI (review plan D5).** A `gui-shot` job
  (GitHub and Forgejo, `rust:1.97-trixie`) runs `scripts/gui-shot-check.sh`,
  which compares the headless 1400×900 capture against the golden
  `docs/reviews/golden/gui-1400x900.png` and fails above 200 differing pixels
  (`magick compare -metric AE -fuzz 2%`), uploading the diff. `--update`
  refreshes the golden (see CONTRIBUTING.md).
- **CLI `save-view`.** `improv save-view <db> <view-id> <name> <measure>
  [--rows Cat,Cat] [--cols Cat,...]` writes a named view the same shape the
  GUI saves (stacked rows/cols, remaining categories as pages pinned to their
  first item). `improv list` now prints a `views:` section.

- **GUI screenshot harness (`scripts/gui-shot.sh`).** Builds the binaries,
  loads a fixed Time×Product×Scenario demo model, runs `improv-gui` on a
  headless Xvfb at 1400×900 and writes `target/gui-shot/gui.png` plus
  `compare.png`, the capture beside the NeXT Improv reference. Output is
  pixel-identical run to run.
- **GUI range selection.** Shift+arrow (or Shift+hjkl), Shift+click and
  dragging across cells select a rectangle; a plain arrow or click collapses
  it. The status-bar **Sum** adds the range's numbers.
- **GUI window title.** The native title reads `Improv — <file>`, with a
  trailing `•` while edits are unsaved (an edited in-memory model, or a
  failed autosave).
- **GUI transient status.** Status-bar messages (save, recalc, undo/redo,
  added formula, errors) clear themselves after 5 seconds.

### Changed

- **`improv_engine::session::Edit::value` is a `CellValue`** (was `f64`), so
  the live engine takes boolean, text and date edits and seeds every typed
  input cell. `Engine::set` still takes an `f64`.
- **`improv.what_if` returns `jsonb` values** (was `float8`), so it reports
  boolean, text and date measures too (dates as RFC 3339 strings), and its
  edits may set inputs of those types.

- **Mentat 0.14 → 1.11.1** (pin `6fc2bc7f`, Mentat `master` at v1.11.1).
  No Improv API changes were needed. Measured on 100k/200k-cell models
  (`docs/reviews/2026-10-10-mentat-1.11-upgrade.md`): loads, partial loads,
  evaluation and edits 9–26% faster, re-import 13–15% faster, first import
  ~10% slower, store files ~29% larger (new history and value indexes).
  Reported RSS is higher because Mentat now memory-maps the store (1 GiB cap);
  set `MENTAT_MMAP_SIZE=0` to turn that off. The old pin `1af15e15` lived on
  an unpushed branch and was not fetchable from a clean clone; the new pin is
  on Mentat's pushed `master`.

- TUI: the status box grows to fit its content, so the edit/command prompt
  and messages stay visible when `pages:`/`filters:` lines are shown. The
  key help wraps to the terminal width, and the prompt sits right under the
  title so a short terminal clips the help first.
- TUI: a derived measure's formula shows on a status line in controlled
  English (`Revenue = Price times Quantity`).

- GUI: the model browser now leads with a NeXT-style white list of saved
  views (`Name - Worksheet` rows with a worksheet icon). A single click
  selects a view and shows its computed description (measure, categories,
  pages); a double-click opens it. Categories and Measures move below as
  collapsible sections.
- GUI: NeXT scrollers. The grid, model browser and formula list show an
  always-visible 19 px `#808080` trough with a square `#AAAAAA` knob,
  a 1 px bevel on the knob and NeXT's two arrow buttons grouped at the
  bottom of the vertical trough (one line per click).

- **GUI filtering moved to the tile menu.** The worksheet's **Filters**
  shelf is gone; right-click a category tile for a checkbox per item (uncheck
  to hide it from that worksheet) and **Show all**. A filtered tile reads
  `Name*`.
- **GUI grid field fills the table area.** The white field reaches the
  worksheet's edges instead of stopping at the last cell.
- **GUI model browser labels measures.** Each measure shows a small gray
  `input` or `formula` label instead of the `·` / `=` prefix.
- **GUI first worksheet window.** A fresh window opens clear of the
  application menu and is sized to its grid (360–900 × 300–700 px).

- **GUI grid follows NeXT Improv.** Cells are plain white with thin black
  rules (no stripes, no per-cell buttons); the selection and its row and
  column header items fill `#AAAAAA`. Input cells carry a faint inset
  underline; derived cells show `formula: …` on hover. **Double-click** (or
  Enter / F2) edits a cell; a single click only selects it.

- **GUI category tiles follow NeXT Improv.** Tiles are 17 px dark-gray chips
  with white names (no grip, no ⏷ button; right-click for
  **Move to Rows / Columns / Pages**). Page tiles sit top-left, each above a
  white item field that picks the page (replacing the `< item [i/n] >`
  stepper); column tiles sit top-right and row tiles bottom-left. Gutters size
  to their tiles, with no placeholder text, and highlight as drop targets
  only during a drag.
- **GUI worksheet windows open at 560×420** (was 480×320) so the grid and
  formula list fit.
- **GUI formulas are edited in place in the formula list.** The top formula
  bar is gone: the selected row is the editor (Enter commits, errors show
  under the row, Esc reverts), and typing `Name = formula` on the list's
  last line adds a derived measure.
- **GUI formula list lives in each worksheet window**, under the grid, with a
  `Calc` button. The operator palette and the "New derived measure" form are
  gone.
- **GUI model browser and Info panel are NeXT windows.** The docked
  explorer and inspector side panels are gone, so the dark workspace spans
  the full width above the bottom panels. The model
  browser (`<file> - Model`, bottom left; Categories, Measures, Views) has a
  close box and **View ▸ Model Browser** toggles it. The Info panel (top
  right, closed at start) opens from **Info ▸ Info Panel…** and closes from
  its close box. A fresh matrix starts right of the application menu.
- **GUI matrices are NeXT worksheet windows on a dark workspace.** The
  scrollable canvas is gone: the area between the panels is a `#555555`
  workspace, and each matrix floats on it as its own window with a black
  (key) or dark gray title bar reading `<matrix> - Worksheet - <file>`
  (`Untitled` when in memory), a close box while more than one matrix is
  open, and a bottom resize bar. Drag the title bar to move a window; clicking
  a window gives its matrix the keyboard. Window positions and sizes are saved
  in views as before, so existing views load unchanged.
- **GUI application menu replaces the tool palette and tab strip.** A
  floating NeXTSTEP vertical menu (Info, File, Edit, Format, Worksheet, View,
  Graph, Print…, Services, Hide, Quit) with cascading submenus now carries
  pivot, undo/redo, chart, CSV, save view, save model, add/remove matrix, and
  the Info panel (inspector) toggle. The View submenu lists the saved views
  that the top tab strip used to show. New key equivalents: Ctrl/Cmd+S saves
  the model, Ctrl/Cmd+H hides and Ctrl/Cmd+Q quits, next to the existing
  undo/redo chords. None of them fire while a text field has focus. A
  selected row in the explorer and formula lists now shows a white face, so
  selection no longer looks the same as hover.
- **GUI theme is NeXTSTEP grayscale.** The GUI uses NeXT's four grays
  (`#000000`, `#555555`, `#AAAAAA`, `#FFFFFF`, plus a `#808080` scroller
  trough) and nothing else: the selection is filled `#AAAAAA` instead of blue,
  corners are square, strokes are 1 px, the grid sits on white, and category
  tiles are dark gray with white text. Formula errors are marked by a 2 px
  underline rather than red, and charts use gray shades. Body text passes WCAG
  AA: black on `#AAAAAA` is 9.0:1, black on white 21:1, white on `#555555`
  7.5:1.
- **GUI text is Arimo 12.** The GUI bundles Arimo 1.33 Regular and Bold
  (OFL-1.1, `crates/gui/assets/fonts/`), metric-compatible with NeXTSTEP's
  Helvetica, as its primary proportional font; egui's fonts stay as glyph
  fallback. Body and button text are 12 pt, and headings are 12 pt bold.
- **GUI grid right-aligns numbers** so their digits line up, as Improv does;
  text, booleans and dates stay left-aligned. The cell editor for a numeric
  measure is right-aligned to match.
- **GUI item headers are plain text in a ruled band.** Row and column headers
  (`Q1`, `Widget`) were beveled button chips. They now draw as plain dark text
  with rule lines, as in Improv, so only the category tiles look draggable.

### Performance

- **pg_improv logical mode applies changes incrementally (G1).** The sync
  worker keeps each model loaded with a live engine, turns decoded row
  changes into input-cell edits, and writes only the result rows that
  changed (a one-row UPDATE on a 100k-cell model rewrites two result rows).
  It rebuilds on a structural change, TRUNCATE, a new item, or an error. It
  also logs a standby snapshot after each batch so polls decode only new
  WAL. Commit → result visible at 100k cells (p50): 1161 → 52 ms for one
  row, 1552 → 559 ms for a bulk UPDATE.
- **Store load is no longer one query per cell (P1).** `load_model` /
  `load_partial` fetch (measure, coord, value) in one query per value
  attribute (three in all) instead of a query per cell. On a 100k-cell
  model (release CLI, 100×1000 CSV) `improv eval` went from 279.9 s to
  0.58 s and `improv list` from 283.2 s to 0.67 s. Date cells now load back
  as dates instead of text.
- **Engine edits cost O(changed measures), not O(model)** (P3).
  `session::Engine` now returns a copy-on-write `session::Snapshot`
  (`Arc<HashMap<MeasureId, Arc<MeasureValues>>>`): an edit shares every
  unchanged measure with the previous snapshot instead of cloning all
  tracked cells. On the `incremental_many_edits` stress test (release,
  5,000 single-cell edits) the time per edit drops from 950 us to 62 us at
  10k cells (1,052 to 16,097 edits/s), and from 5,639 us to 71 us at 100k
  cells (177 to 14,039 edits/s). It is now almost flat in model size.
  `Engine::new`/`set`/`clear`/`apply` return `Snapshot`, and callers clone
  it cheaply.

### Fixed

- **Numbers display without binary noise.** Every front end showed a value
  at full `f64` precision, so `1173 * 0.05` read `58.650000000000006`. They
  now show 15 significant digits (`58.65`); stored values and exports keep
  every bit.
- **pg_improv: item ids are minted in one place.** `improv.mint_items`
  (created by both the extension and `improv_storage_pg::CATALOG_SQL`) mints
  ids for new dimension values, serialized per model, with a unique
  `(model_id, category_id, name)` index. `improv.recalc`, the trigger and
  worker paths and the GUI's store all call it, so concurrent loads agree on
  ids and nothing is minted twice. Minting no longer bumps `model_version`.

- **Postgres saves no longer clobber SQL edits to bound fact tables**
  (`storage_pg`, design §9.2 G2). `save_model` now writes only what changed
  since the handle loaded (or last saved). Each bound cell it changes or
  removes is guarded by its loaded value (`… WHERE value IS NOT DISTINCT FROM
  <loaded>`), so a cell SQL changed in the meantime fails the whole save
  with `Conflict` ("reload and retry"). A removed bound cell sets the value
  column to NULL and keeps the row. A save deletes only the catalog rows
  (categories, items, measures, views, `improv.cell` cells) it had loaded, so
  rows added by another client survive. A save with no changes writes no
  cells. `PgModelStore::snapshot` / `set_snapshot` carry the base across
  handles, like `loaded_tx`; without a snapshot a save writes everything
  unchecked, as before.
- `pg_improv`: `recalc`, `define` and `bind_*` no longer fail with "SELECT
  FOR UPDATE is not allowed in a non-volatile function" when they are the
  first statement of a transaction.
- **Broadcast binary ops with the larger operand on the left.** A formula
  such as `Net = Price[Time,Product] - Discount[Product]` (or `*`, `+`, `/`)
  evaluated to no cells. Only the mirrored `Discount - Price` worked. The
  dataflow `MapBinary` joined its operands on the full coordinate, so a row
  matched only when the right operand already spanned the result's
  dimensions. `Join`, `MapBinary` and two-argument scalar calls now share one
  broadcast-join helper that keys both sides on their shared categories.

- **Incremental recalculation no longer drops cells whose value went down.**
  The engine's update stream orders a changed cell's `+new` before `-old`
  when the new value sorts lower, and the session snapshot applied the
  retraction by deleting the key, which wiped the value just written. So
  `Engine::set` lost, for example, `Revenue` when a `Price` decreased. A
  retraction now removes a key only if it still holds the retracted value.

- **Sandboxed external functions on non-FHS hosts (NixOS).** The interpreter
  is now run by its canonical host path, and `/nix/store` is bound read-only
  when present. Before, every sandboxed call failed on NixOS with
  `execvp python3: No such file or directory`.

- **A save no longer clobbers another process's writes.** `save_model` retracts
  stored cells the model lacks, so when two processes shared a store (`improv
  set` beside a running GUI or server) the second saver deleted the first's
  new cells. A save now checks the store's transaction high-water against the
  one it loaded at and, if another writer committed since, fails with
  `StoreError::Conflict` and writes nothing. The GUI and TUI report "reload and
  retry"; the server answers `409 Conflict` and drops its cached model so a
  retry reloads. New `ModelStore::{tx_high_water, loaded_tx, set_loaded_tx,
  reload_model}`.
- Storage: a measure's category order survives save/load. `:measure/categories`
  is a set, so a reload returned the categories sorted by id, and anything
  mapped by position (SQL/CSV columns, coordinates) landed on swapped
  categories: `refresh-sql` on a 2-dimension measure wrote
  `{Year=A,Region=2025}`. The order is now stored in a new
  `:measure/category-order` attribute. Stores written before it load as
  before (id order) and gain it on their next save; no migration needed.
- TUI: the grid scrolls to keep the cursor visible on models larger than
  the terminal (previously the cursor could move off-screen).
- TUI: mouse clicks select the cell under the pointer (they were offset by
  the row-label column and selected the column to the right).
- TUI: text and boolean input cells can be edited (parsed per the measure's
  type, as `improv set` does); previously every edit had to be a number.
- TUI: editing is refused when an axis is empty or fully filtered. It used
  to store a cell with a missing dimension.
- TUI: `[` / `]` page correctly when an empty category precedes another
  page dimension.

- **CLI `--at` accepts every natural spelling of a coordinate.** `set` and
  `scenario` now take repeated flags (`--at Time=Q1 --at Product=Widget`) and
  space-separated pairs (`--at Time=Q1 Product=Widget`) as well as the
  comma-joined form, merging them. Naming a category twice is a clear error,
  and stray non-pair arguments are still rejected.
- **GUI tool-palette icons no longer render as tofu boxes.** Undo, redo,
  import/export and save-view used glyphs (`↶ ↷ ⇄ ▤`) missing from egui's
  bundled fonts; they now use glyphs those fonts contain, and a unit test
  checks every palette glyph against the loaded fonts.
- **The matrix no longer draws over the Inspector.** Gutter tiles and the page
  well escaped the canvas clip (egui's nested panels reset the clip rect) and
  painted on top of the side panels in small windows. They are now clipped to
  the canvas. The window also opens at 1280×800 (was eframe's 800×600) with a
  960×600 minimum.
- **The GUI no longer loses in-memory work silently.** File ▸ Save (Ctrl+S)
  is disabled for an in-memory model instead of reporting "saved model", and
  quitting an edited in-memory model warns first; a second Quit discards it.
- **GUI menus stay on screen.** A submenu that would run off the right edge
  cascades to the menu's left instead and is lifted to keep its bottom on
  screen, and the menu now opens on the workspace rather than over the formula
  bar's label.
- **Re-saving a model no longer piles up cell datoms (P2).** `save_model`
  diffs cells against the store: an unchanged cell is not re-asserted, a
  value left under another type's attribute is retracted, and a cell removed
  from the model (cleared in the GUI, deleted through the server) is
  retracted instead of lingering on disk and coming back on the next load.
  An unchanged re-save of a 100k-cell model now adds no datoms (it added 200
  and grew the file ~30 KB) and takes 1.3 s instead of 321 s.
- **`improv init` no longer wipes an existing store.** It installs the
  schema only; it used to save an empty model, which under the cell diff
  above would have retracted every stored cell.

### Security

- **External functions: the host picks the sandbox.** Every body runs
  `Restricted` unless the operator sets `IMPROV_EXTFN_TRUSTED=1`; a model's
  `pure: false` no longer turns the sandbox off (S2).
- **External functions fail closed.** Without `bwrap` (bubblewrap) a
  `Restricted` body is refused with "bwrap not found; install bubblewrap or
  set IMPROV_EXTFN_ALLOW_UNSANDBOXED=1"; that variable falls back to rlimits
  only. A failed `setrlimit` aborts the spawn (S4, S18).
- **External functions: tighter bwrap.** Only `/usr`, `/bin`, `/lib`,
  `/lib64` and `/etc` are bound read-only; `/tmp`, `/home` and `/root` are
  empty tmpfs; all namespaces are unshared (no network). The child
  environment is cleared to `PATH=/usr/bin:/bin`, `HOME=/tmp` and `LANG`, so
  API tokens and cloud credentials no longer leak (S5, S6).
- **External functions: bounded output.** stdout and stderr are drained
  concurrently and capped at 1 MiB each; output over 64 KiB no longer
  deadlocks into a timeout (S7).
- **External functions: the timeout kills the process group**, so forked
  grandchildren die with the body (S8).
- **Wasm functions:** a fuel budget stops runaway loops (the worker thread
  ends), linear memory is capped at 64 MiB, and bodies must start with
  `wat:` or `base64:` (a bare path is no longer read from disk) (S9).
- **Python functions:** the body is passed verbatim as JSON data instead of
  inside an escaped raw string, so backslashes keep their meaning (S20).
- **Server: the refresh scheduler is off by default** (`IMPROV_SCHEDULER=1`
  enables it) and runs only external functions listed by digest in the
  `IMPROV_EXTFN_ALLOW` file, always in the `Restricted` sandbox. Serving a
  shared model no longer runs its code (S1).
- **Server: request limits.** 10 s header-read timeout (slow-loris), 30 s
  request timeout, 64 requests in flight, 1 MiB bodies, 10,000 edits per
  `/eval` (S10, S13).
- **Server: write validation.** Writes need an existing input measure, a
  coordinate covering exactly its categories, and finite values. Writes are
  serialized, so concurrent ones no longer lose updates. New
  `IMPROV_API_TOKENS_RO` read-only tokens (S12).
- **Server: internal errors** return `{"error":"internal error","request_id":N}`
  and log the detail, instead of echoing store paths (S14).
- **Server: auth hardening.** Constant-time token comparison; open mode on a
  non-loopback address needs `IMPROV_ALLOW_OPEN=1` (S15).
- **Server: model cache.** Requests share one cached model, reloaded only
  after a write, instead of loading the store up to three times per request
  (P4).
- **SQL refresh is read-only.** A stored `SqlSource.query` must be a single
  `SELECT`/`WITH` statement (`check_read_only_sql`; stacked statements are
  refused), and every backend executes it read-only: Postgres in a
  `READ ONLY` transaction, DuckDB in a `READ ONLY` transaction with
  `enable_external_access = false` (no `read_csv('/etc/passwd')`, URLs or
  `ATTACH`), SQLite only if `sqlite3_stmt_readonly`. A data-modifying CTE
  (`WITH x AS (DELETE ...) SELECT ...`) is refused by the server.
- **Postgres fails closed on cleartext:** this build has no TLS, so
  `connect_postgres` refuses a non-loopback host unless the connection
  string says `sslmode=disable` explicitly, and refuses `sslmode=require`
  (it cannot be honoured). Use an SSH tunnel to a loopback port.
- Postgres `NUMERIC` columns now import as numbers (they failed to decode).
- **CSV export neutralizes formula injection.** Text cells, item names and
  headers starting with `=`, `+`, `-`, `@`, tab or CR are written with a
  leading `'`, so Excel/LibreOffice show them as text instead of running
  them (S16). Numbers are untouched. `ExportSpec { neutralize_formulas:
  false, .. }` with `export_measure_csv_with` turns it off for an exact
  round trip.
- **CSV import is size-capped.** A field over 1 MiB, a record over 64 MiB
  (checked while streaming, so an unterminated quote can't buffer the
  file) or more than `ImportSpec::max_records` records (default
  10,000,000) is an error instead of an out-of-memory (S17).
- **CNL parser depth bound (S3).** The `nl_formula` recursive-descent parser
  had no nesting limit: one ~200 KB `POST /nl/parse` of nested parens
  overflowed the stack and aborted `improv-server`. Paren nesting plus
  operator count is now capped at 200 (as in the formula parser), returning
  the new `NlError::TooDeep`; tested on a 2 MiB stack with 100k parens.

## [0.7.0] - 2026-09-24

The GUI is rebuilt against Lotus Improv / Quantrix reference material, and
Improv becomes buildable by anyone who clones it.

### Added

- **Margin gutters with edge-docked category tiles.** Tiles now sit in gutters
  that *frame* the grid — column tiles along its top edge, row tiles down its
  left, page/unplaced categories in a corner well — and pivoting is dragging a
  tile from one gutter to another. The previous horizontal shelf above the table
  was structurally wrong: the references show the axes anchored to the edges they
  control. Adjacency is *constructed* from egui's panel contract rather than
  approximated, and is asserted by this codebase's first headless egui frame
  test, proven non-vacuous by feeding the old shelf's geometry in and requiring
  rejection.
- **A canvas of matrices.** A view holds several independently pivoted matrices,
  each with its own axes, filters, cursor and gutters, placed and resized on a
  scrollable canvas. This also delivers multi-measure display, previously open.
  Matrices are child `Ui`s, not `egui::Window`s, because a window is a floating
  layer that cannot scroll inside a parent scroll area.
- **Numbered formula list.** Every derived measure's formula is visible at once,
  numbered, with an enable checkbox per formula, inline error state, and
  click-to-select. Disabling is a real model operation that survives save and
  reload, not a GUI-only flag, and dependents recompute without the disabled
  formula.
- **Quoted and qualified names in formulas.** `'Unit Price'` — exactly what a
  CSV header produces — now has a spelling, with `''` for an embedded quote.
  Eight of nine previously unspellable names round-trip; only the empty name
  remains unspellable. Dotted names (`Matrix.Measure`) parse and fail with a
  clear error rather than inventing a namespace that does not exist yet.
- **Multi-matrix view schema**: `CanvasRect` and `MatrixPlacement`, with
  `View::from_matrices`/`primary`/`matrices`.
- **Per-matrix title bars, document tabs for saved views, and a selection
  aggregate readout.**
- **Undo/redo** over bounded model snapshots, and focus-aware keyboard shortcuts
  so grid keys only fire when the grid owns focus.

### Fixed

- **Strings containing newlines or tabs were silently corrupted on save.**
  `"Line\nBreak"` reloaded as `"LinenBreak"`. The escape was emitted but the EDN
  reader dropped the backslash, so any description, name, or Text cell carrying a
  newline or tab lost data with no error. Fixed on both sides: raw emission here,
  and the reader/printer fixed upstream in Mentat.
- **Models larger than ~5461 cells could not be saved** — `save_model` panicked
  inside Mentat's transactor. Saves are chunked, and the ceiling is per attribute
  queue rather than per transact.
- **Typed cell display and editing** key on the measure's declared type, never
  the text's shape, so `42` typed into a Text measure stays text.
- **Derived measures exported their empty input storage** instead of computed
  values; CSV export now evaluates through the engine.
- **`improv set` silently wrote a dimensionally invalid cell** when `--at` was
  forgotten: `improv set db 100 10 Product=A` reported success and stored a cell
  at the empty coordinate, which then evaluated to nothing. Stray arguments are
  rejected with a message naming the flag you meant, and the coordinate is
  checked against the measure's declared dimensions.
- A Unicode crash in the formula highlighter, an empty-axis divide-by-zero,
  CSV item-ID aliasing across imports, non-atomic CSV import and cell edits, a
  formula bar that offered text no parser would accept, and stale
  non-dimension categories blanking the grid.
- `rustls` 0.23.43 → 0.23.45 (RUSTSEC-2026-0285, build dependency only).

### Changed

- **Mentat is a pinned git dependency, not a sibling path dependency.** A fresh
  clone previously could not even *parse* its manifest (`failed to read
  ../mentat/Cargo.toml`), so nobody without a sibling checkout could build
  Improv; CI masked this by cloning Mentat beside the checkout in every job.
  Verified by building a bare clone with no sibling present.
- The demo scripts build the CLI once instead of invoking `cargo run` per
  command: `sample-model.sh` went from 12m31s to about 3s.
- Scale claims are stated end-to-end (build + save + reload + evaluate) rather
  than from `evaluate()` alone.


## [0.6.0] - 2026-09-07

### Added

- **`ModelStore::load_partial`**: loads only a measure set's dependency
  closure (its formula chain / external-call args), not every cell of every
  measure in the store — the out-of-core design doc's recommended first step,
  built. `engine::dataflow`/`engine::session` needed zero changes. CLI
  `eval`/`show` now use it.
- **`improv stream`**: a stdin→stdout streaming compute pipeline. Reads
  `<measure> <value> [Cat=Item,...]` edit lines from stdin, applies each
  incrementally to a live engine, and prints only the changed cells of one or
  more target measures to stdout as they land —
  `producer | improv stream model.db Revenue | consumer`.
- **CSV/TSV import/export** (`improv_storage_csv`): the baseline data on-ramp
  named in IMPROV.txt, previously missing. CLI `import-csv`/`export-csv`
  mirror `import-sql`/`export-sql`'s column-to-dimension mapping syntax; a GUI
  Import/Export wizard and a TUI `I`/`E` command prompt wire it into all three
  interfaces.
- **Hosted refresh scheduler** in `improv-server`: the same due-measure/
  refresh loop as the CLI's `serve-refresh` daemon now runs as a background
  task (`IMPROV_SCHEDULER`/`IMPROV_SCHEDULER_TICK_SECS`), with
  `GET /scheduler/status` to observe it.
- **`improv_data_source`**: shared import logic (category/measure setup,
  name→id item interning) extracted out of `storage_sql`/`storage_csv`'s
  independent duplication; every public function signature unchanged.
- **GUI formula syntax highlighting + inline error display**: the formula bar
  highlights identifiers/functions/numbers/strings/date-literals/operators and
  shows a failed parse's error position inline, via a pure
  `formula_highlight::scan`/`highlight_formula`.
- **Formula-parser fuzz target** (`fuzz/fuzz_targets/fuzz_formula_parser.rs`).
- **extfn OS-level sandbox**: subprocess runtimes (Python/R/Julia/Pure) now run
  under `bwrap` (namespace isolation) when available plus OS resource limits
  (CPU/memory/FDs/procs) always, fail-open by design; WASM was already
  sandboxed by construction (`wasmi`, no host imports).

### Fixed

- **Non-atomic model saves** (crash-safety): `save_model` transacted the
  model in up to 6 separate SQLite transactions; a crash between them left a
  partial save. Now one `InProgress`, one commit — all-or-nothing.
- **Parser panic on multi-byte UTF-8** (found by the new fuzz target /
  adversarial-input test): the tokenizer cast a raw byte to `char` and could
  walk off a UTF-8 character boundary. Now decodes real characters.
- **Parser stack overflow on deeply-nested parentheses** (a real DoS on
  attacker-controlled formula text): recursive descent now bounds nesting
  depth (`MAX_PARSE_DEPTH`) and errors cleanly instead of overflowing.
- **CI/local gate scoping**: `cargo fmt`/`cargo clippy` now target our 11
  crates explicitly; `--all`/`--workspace` could reach the vendored Mentat
  path dependency and, transitively, whatever it depends on.

## [0.5.0] - 2026-08-28

### Added

- **DuckDB** as a first-class SQL backend (bundled build; `Backend::Duckdb`,
  `ConnKind::Duckdb`).
- **Date values** in the engine's DD value lane (`CellValue::Date`), and
  **standalone/broadcast literal measures** (a bare `X = 5` scalar that
  broadcasts into other formulas).
- **What-if scenarios** as first-class model data: `Scenario` = a named overlay
  of input-cell overrides (`Model.scenarios`, `with_scenario`), deterministic;
  CLI `scenario` + `eval --scenario`.
- **External-function runtimes** R, Julia, WASM (in-process `wasmi`), and
  **Pure-lang**, alongside Python.
- **Server bearer-token authentication** (`IMPROV_API_TOKEN` /
  `IMPROV_API_TOKENS`; `/health` public; open mode when unset).
- **Virtualized large-grid rendering** in the GUI (mixed-radix on-demand row
  tuples via `body.rows` — millions of row lines never materialized), and
  **charting the full Cartesian product** of stacked axes.
- **Large-model stress tests** (`crates/engine/tests/stress.rs`): scale-eval up
  to 1M cells and an incremental-edit throughput test (`--ignored`).
- **NeXTSTEP look-and-feel for the desktop GUI**, matching the original Lotus
  Improv: a NeXTSTEP theme (gray desktop, chiseled/beveled controls, squared
  corners, paper-white grid), on-grid **category tiles at the grid margins** as
  the pivot gesture (drag a tile between Rows/Columns/Pages margins), a top
  **formula bar** (`<Measure> = <expr>`), chiseled grid headers with a
  `Rows \ Columns` corner stub, a NeXT-style **tool palette**, and
  **multi-category-per-axis stacking** (stack >1 category on an axis; the grid
  renders the Cartesian product with nested headers and group-outlined rows).
- **Refresh scheduler daemon** (`serve-refresh`): honors each measure's
  `RefreshPolicy` automatically — on every tick it refreshes the due measures
  (on-load once, interval every N seconds). The scheduling decision is a pure,
  unit-tested `core_model::schedule`; external-call measures also carry a
  policy (`define ... --refresh`).
- **SQL refresh policy + `refresh-all`**: `SqlSource` records a `RefreshPolicy`
  (manual / on-load / interval); CLI `import-sql --refresh ...` sets it and
  `refresh-all <db> [source.sqlite]` refreshes every external-sourced measure
  (CALL + SQL) at once. Policy is advisory metadata (a scheduler that honors
  timing is future work); the engine core is untouched.
- **`CALL(...)` / `SQL(...)` definition grammar**: `core_model::parser::
  parse_definition` parses a measure definition as an ordinary formula or a
  whole-RHS source form (`Target = CALL(func, args...)` /
  `Target = SQL("query")`), keeping source forms off the engine's expression
  grammar. CLI `define` / `register-ext` / `refresh-ext` make external-call
  measures usable end to end.
- **Chart view** (`improv-gui`): a Chart toggle plots the selected measure as
  grouped bars (optional line overlay), drawn with `egui::Painter` (no new
  dependency), honoring filters and page pinning.
- **External-function measures wired into the engine** (Phase 6): a measure can
  be defined as `func(arg_measures...)` (`Model.external_calls`);
  `engine::external::refresh_external_measure` evaluates the registered
  `ExternalFn` per coordinate via `improv_extfn` and writes input cells —
  host-side, so the dataflow core stays pure. `ExternalFn`/`Language` moved to
  `core_model`; defs + calls persist through Mentat.
- **Saved views + filters in the UI** (`improv-gui`, `improv-tui`): save the
  current layout as a named view, load it to reproduce measure + axis placement
  + pins + filters, and filter a category to a subset of items. GUI: Views
  panel + Filters checkboxes. TUI: `S` save, `v` cycle, `f`/`F` filter.
- **PostgreSQL backend + connection management** (Phase 7): SQL import/export/
  refresh now work over a backend abstraction (SQLite + Postgres via
  `postgres`); serde `Connection` descriptors keep credentials out of band
  (password-less DSN + `password_env`, resolved at connect time, redacted logs).
- **External-function runtime** (`improv_extfn`, Phase 6): a registry of typed,
  purity-asserted external functions evaluated in an isolated Python subprocess
  (timeout + JSON marshalling), speaking `core_model::Value` — the runtime the
  engine's `Expr::Call` will dispatch to (wiring pending). Isolated-mode +
  timeout, not yet an OS sandbox.
- **GUI keyboard grid navigation** (`improv-gui`): arrow/hjkl move a highlighted
  cell cursor; Enter/F2 edit, `[`/`]` page, `n`/`N` cycle measures; click also
  moves the cursor.
- **Saved views + filters** (`core_model::View`/`Filter`, Quantrix/Improv
  parity): a `View` captures a measure, an axis-order permutation
  (rows/columns/pages), pinned page items, and per-category filters (restrict a
  dimension to a subset). Persisted through Mentat (`:view/*` datoms), survives
  save/load. The data model both interfaces can drive; UI wiring follows.
- **GUI drag-and-drop pivoting** (`improv-gui`): an axis shelf (Rows / Columns /
  Pages) lets you drag category chips between axes (egui drag-and-drop) or use
  per-chip `→` / a Pivot button, with per-page-dimension `< >` selectors — the
  Improv/Quantrix re-pivot, no formula edits. Grid re-pivots live.
- **SQL live-query measures** (Phase 7): an imported SQL measure now records a
  refreshable `SqlSource` on the model (`improv_storage_sql::add_sql_measure` /
  `refresh_sql_measure`); CLI `refresh-sql` re-runs the stored query and
  replaces the measure's cells (persisted via a `:measure/sql-source` datom).
  SQL data re-enters as ordinary input cells, so the engine recomputes
  dependents with no SQL path of its own; these measures are marked
  nondeterministic (refresh-gated).
- **TUI pivoting + mouse** (`improv-tui`): `p` rotates categories across
  rows/columns/pages (Improv/Quantrix-style re-pivot, no formula changes), and
  left-click selects a cell (mouse capture enabled). Complements the existing
  arrow navigation, `[`/`]` paging, `e`/Enter editing, and measure cycling.
- **Nix flake**: `nix develop` dev shell with the Rust toolchain, GUI runtime
  libraries on `LD_LIBRARY_PATH` (so `improv-gui` runs), and the tooling.
- **TUI paging** (`improv-tui`): `[` / `]` cycle the pinned item of a measure's
  extra (3rd+) dimension, so multi-dimensional measures are fully viewable
  slice-by-slice; a `page i/n` indicator and a key-hint footer were added.
- **SQL connectivity** (`improv_storage_sql`, Phase 7, SQLite): import a
  `SELECT` into a new input measure (columns → categories/items/cells) and
  export a measure's cells to a SQL table. CLI `import-sql` / `export-sql`.
  SQL data enters as ordinary input cells, so the deterministic engine core is
  untouched; identifiers are validated and values bound (injection-safe).
- **Named scalar functions in formulas**: `ABS`, `ROUND`, `FLOOR`, `CEIL`,
  `SQRT`, `NEG`, `MIN2`, `MAX2` are now callable as `NAME(args...)` in formula
  text (parser registry `core_model::parser::scalar_func`, arity-checked),
  evaluated deterministically by the engine — the in-process foundation for
  Phase 6's external-language `CALL(...)` runtime.
- **Desktop GUI** (`improv_gui`, egui/eframe): `improv-gui <db>` — a model
  explorer, an editable pivot grid (input cells edit through the live engine
  with autosave; derived cells read-only), a formula editor (edit/add derived
  measures from formula text), and an inspector (metadata, dimensions,
  dependencies, formula-in-English, error-cell count). Phase 5 toolkit
  decision: egui/eframe.
- **Non-numeric value lane**: the engine dataflow now carries a tagged
  `CellValue` (`Num`/`Bool`/`Text`/`Err`), so boolean and text derived measures
  evaluate end-to-end (e.g. `Hot = Price > 15` yields a boolean). Comparisons
  and `NOT`/`AND`/`OR` produce `Bool`. CLI/TUI/server render via `CellValue`.
- **Live incremental edit API** (`improv_engine::session::Engine`): builds the
  differential-dataflow graph once and applies input-cell edits as deltas
  (`set`/`clear`/`apply`), recomputing only affected coordinates — no rebuild.
- **TUI live editing**: edit input cells in the pivot grid (`e`/Enter),
  derived measures recompute live via the session engine; autosave on quit.
- **Server** `POST /measures/:id/eval`: apply edits through the live engine and
  return the recomputed snapshot (what-if by default; `"persist": true` writes
  through).
- **Textual formula parser** (`improv_core_model::parser`): `parse_formula` /
  `parse_expr` accept Improv formula text (`Revenue = Price * Quantity`,
  precedence, comparisons, `AND`/`OR`/`NOT`, `SUM|AVG|MIN|MAX(x OVER Cat)`),
  resolving names against the model.
- **Scalar built-in functions** in the engine: `ABS`, `ROUND`, `FLOOR`, `CEIL`,
  `SQRT`, `NEG`, `MIN2`, `MAX2` (numeric); comparison/logical operators
  evaluate in the numeric lane (1.0/0.0).
- **CLI**: `add-derived <db> <id> <name> <formula>` (define a formula measure)
  and `eval <db> <measure-id>` (compute a derived measure via the engine);
  `add-measure` now takes trailing category names to declare a measure's
  dimensions. The full v1 flow — model, formula, incremental recalculation —
  now works end-to-end from the CLI.
- **TUI** (`improv_tui`): a VisiCalc-style terminal pivot viewer
  (`improv-tui <db>`) — renders a measure as a 2-D grid (categories on
  rows/columns, extra dims as pages), keyboard navigation, measure cycling,
  panic-safe terminal teardown.
- **Server** (`improv_server`): a JSON HTTP API (`improv-server <db> [addr]`)
  — `/health`, `/model`, `/measures`, `/measures/:id/values`, `/nl/parse`,
  `/nl/describe`, `/measures/:id/cells`.
- **Multi-layer derived measures**: `engine::evaluate` builds derived measures
  in topological order (cycles rejected), so a derived measure may reference
  another derived measure.
- **Tests**: property tests (proptest) for model/codec/CNL round-trips; fuzz
  targets (cargo-fuzz) for the CNL parser, model JSON, and coordinate codec;
  engine determinism suite (bit-for-bit reproducibility, insertion-order
  independence) and `#[ignore]` stress tests (100k-cell recalculation).
- **CI/CD**: GitHub + Codeberg workflows for the full quality gate,
  cross-platform build/test, release qualification, and docs publishing.
- **Core model** (`improv_core_model`): categories, items, measures,
  coordinates, dimension-aware formula AST, value/error types. JSON round-trip.
- **Storage** (`improv_storage_mentat`): persist a model as datoms on the
  embedded (SQLite) Mentat store and reconstruct it by query. Save↔load round
  trip.
- **Engine** (`improv_engine`): formula compiler (type + dimension inference,
  lowering to a Join/Aggregate/Map plan) and differential-dataflow evaluation.
  Verified against the canonical Time×Product revenue oracle.
- **CLI** (`improv_cli`): `init`, `add-category`, `add-item`, `add-measure`,
  `set`, `list`, `show`, `export` over a Mentat-backed store.
- **Natural language** (`improv_nl_formula`): controlled-English ⇄ formula
  parse/describe with a stable round trip.
- Project infrastructure: workspace, `rustfmt.toml`, `clippy.toml`, `deny.toml`,
  `.typos.toml`, CI, dual license (Apache-2.0 OR MIT), contributor guide.

[Unreleased]: https://codeberg.org/gregburd/improv/compare/v0.9.0...main
[0.9.0]: https://codeberg.org/gregburd/improv/compare/v0.8.1...v0.9.0
[0.8.1]: https://codeberg.org/gregburd/improv/compare/v0.8.0...v0.8.1
[0.8.0]: https://codeberg.org/gregburd/improv/compare/v0.7.0...v0.8.0
