# Response to the pg_improv feature requests of 2026-10-18

From: the Improv maintainers
Revision: main after these changes (pg_improv 0.10.0, unreleased; tested on
PostgreSQL 17 and 18.6, the customer's version)

Each request below has three parts: what was built, how to use it, and how it
stands against your acceptance criteria. "Met" means a test checks that
criterion. Every test runs in CI, except the full-size scale run, which is
manual.

## P1-1. Replace or redefine a derived measure from SQL — met

```sql
SELECT improv.define('merit', 'Merit = 0.30*Vn + 0.25*An', replace => true);
SELECT improv.drop_measure('merit', 'OldComponent');
```

- **Re-running is a no-op.** Re-running the same `define(..., replace => true)`
  doesn't bump `model_version` and doesn't rewrite a single row: every
  result row keeps its ctid. This is tested in manual, trigger, commit and
  logical mode.
- **Only changed rows move.** A changed formula rewrites only the result rows
  whose value changed, in the caller's transaction, in all four modes. In
  logical mode the worker's follow-up rewrites nothing.
- **Errors name the dependent.** A replacement that changes the value type,
  a dimension or a dependency cycle is an ERROR naming the measures that
  read it. So is a drop of a measure something still references, e.g.
  "Revenue still references it".
- **What a drop removes.** `drop_measure` on an input measure removes its
  binding and stored cells. On a derived measure it drops the result table.

## P1-2. A default for a missing cell — met, with one limit

```text
An = COALESCE(Authored, Vol * 0)   -- Authored where it has a cell, else 0 wherever Vol has one
An = FILL(Authored, 0)             -- Authored, else 0 at every Person
```

- **Your example works.** Person has four items, `Authored` is set for two
  and `Vol` for all four. Both formulas give 4 cells (p1 = 300, p2 = 2,
  p3 = 0, p4 = 0). `IF`'s laziness is unchanged.
- **Limit.** Your criterion "adding an item to Person adds its filled cell
  without a full recompute" is met for `COALESCE` but not for `FILL`:
  - `COALESCE` adds the cell incrementally.
  - `FILL` makes one cell per item combination, so a new item in a category
    it fills rebuilds the model in logical mode. The new cell appears after
    that rebuild, in about 8–10 s at your scale.
  - For your merit components, `COALESCE(x, Vol * 0)` gives the same cells
    and stays incremental.

## P1-3. Incremental apply for growing dimensions and aggregated bindings — met

- **New items no longer force a rebuild.** The logical worker mints new
  items through `improv.mint_items` inside its apply transaction, with the
  same order and ids as before.
  - It rebuilds only when a formula reads that category's item list:
    positions, PREV/NEXT, windows, per-item and INTO results, FILL,
    RELABEL/SWAP, and order-dependent reductions.
  - Your contribution counts read no item lists, so they never rebuild.
- **Aggregated bindings are incremental.** `sum`, `count`, `min`, `max`,
  `avg` and the new `count_distinct` are kept as per-coordinate fold state.
  - An UPDATE that moves a row between coordinates is handled.
  - TRUNCATE no longer forces a rebuild.
- **`improv.status` reports the worker's activity.** It now shows
  `incremental_applies` and `rebuilds`.
- **Trigger mode reads only touched groups.** It loads only the touched
  coordinates' groups for an aggregated binding.
- **Limit.** Commit mode still recomputes in full. It knows which tables a
  transaction wrote but not which rows, and finding the rows would need a
  trigger on your tables, which is what commit mode avoids.

Measured with 2.5M rows bound as `count` + `count_distinct` into 44,750
cells over Person × Year × Release × Scope, on a c7i.8xlarge:

| | PG 17 | PG 18 |
| --- | ---: | ---: |
| initial build by the worker | 9.7 s | 7.5–8.1 s |
| 1,000-row INSERT batch, commit → visible (one batch brings a new Person) | 88–150 ms | 79–114 ms |
| rebuilds during the batches | 0 | 0 |

- **Correctness check.** After 1,000 random INSERT/UPDATE/DELETE batches,
  `count` and `sum` equal a fresh `recalc`, and `count_distinct` equals
  `count(DISTINCT …)`. This is `crates/pg_improv/tests/scale.sh`; CI runs it
  small, and `ROWS=2500000 BATCHES=1000` runs it at full size.
- **On your 910 s refresh.** The same GROUP BY over 2.5M rows takes 4–5 s on
  that machine, so most of your 910 s is probably the view's joins rather
  than the counting.
  - Bind a persisted contribution table (one row per person, bucket and key)
    instead of the view, as your "not asking for" section suggests.
  - Then a batch costs about 0.1–0.2 s, independent of table size.

Known limits:
- A text `min`/`max` binding rebuilds on every change, because Postgres
  orders text by collation.
- A float8 `sum` can differ from a fresh sum in the last bits. A numeric
  column is exact.
- Tables with aggregated bindings get `REPLICA IDENTITY FULL`.

## P1-4. Distinct counting — met

```sql
SELECT improv.bind_measure('contrib', 'Commits', 'contribution_rows', 'commit_sha',
                           agg => 'count_distinct');
```

- **Equivalence.** After random batches it equals `count(DISTINCT
  commit_sha)` per coordinate.
- **Shared keys.** Deleting one of two rows that share a key keeps the
  count; deleting the second lowers it.
- **In formulas.** The reduction `COUNTUNIQUE(x OVER T)` (Google Sheets'
  name) does the same inside a formula.

## P2-1. Fixed-point iteration (PageRank) — met for correctness; slower than your SQL

```text
Out      = SUM(W OVER Dst)
M        = W / Out
Dangling = FILL(0 * Out, 1)
PR       = FIXPOINT(SEQUENCE(Src, 1 / @ITEMCOUNT(Src), 0),
             LAMBDA(r, RELABEL((1 - 0.85) / @ITEMCOUNT(Src)
               + 0.85 * (FILL(SUMPRODUCT(r, M OVER Src), 0)
                         + SUMPRODUCT(r, Dangling OVER Src) / @ITEMCOUNT(Src)),
               Dst, Src)),
             100, 0.0000000001)
```

- **Form.** `FIXPOINT(init, LAMBDA(r, body), [max_iter = 100], [tolerance])`.
  - Past `max_iter` it returns the last iterate. It never hangs, and the
    language stays total (`max_iter` ≤ 10,000).
  - Arguments are positional: `max_iter => …` is not supported.
  - Formulas have no exponent notation yet: write `0.0000000001` or
    `10^-10`.
  - `SUM(RELABEL(r, …) * M OVER Src)` is written
    `SUMPRODUCT(r, M OVER Src)`. Measures reduced together may now range
    over different categories beyond the reduced ones, which is the
    matrix-vector step.
- **Accuracy.** On 1,500 nodes and 100,000 edges the L1 distance to a
  reference power iteration is 3.4e-15, against your 1e-9 bound.
- **Edits.** After one edge weight changes, the result is within 2.6e-15 of
  the reference and matches a fresh evaluation.
- **Speed: about 14 s on that graph**, roughly 0.9 s per round, against your
  4.6 s for 40 iterations in SQL. Every round feeds the whole iterate
  through the dataflow, and an edit costs about as much, because a warm
  start saves only a couple of rounds.
  - We'd keep PeopleRank in SQL for now.
  - Making each round incremental is the next step for FIXPOINT.

## P2-2. Role-playing categories — met

```text
R2   = RELABEL(R[Dst, Iter], Dst, Src)     -- over [Src, Iter], same values
Mut  = W > 0 AND SWAP(W, Src, Dst) > 0     -- reciprocal correspondents
```

- **Matching.** Items match by name. An item with no same-named counterpart
  is dropped.
- **Your XLOOKUP case now works.** `XLOOKUP(k, Key OVER Dst, X)` with an
  extra category on `X`, which failed with "must range over the same
  categories", now works.

## P3-1. DENSE_RANK — met

`RANK.DENSE(x OVER T, [order])` equals `dense_rank() OVER (ORDER BY x DESC)`,
ties included. The goldens were checked against PostgreSQL.

## P3-2. Dimensionless results in pg_improv — met

`MaxVol = MAX(Vol OVER Person)` gets a one-row result table with a single
`value` column. It works in `recalc`, all sync modes, `what_if`, `status`
and the logical worker.

## P3-3. Named LAMBDAs inside pg_improv — met

```sql
SELECT improv.define('merit', 'LOGNORM = LAMBDA(x, c, LN1P(x) / LN1P(MAX(x OVER c)))');
SELECT improv.define('merit', 'An = LOGNORM(Authored, Person)');   -- and Rn, Mn, Cn
```

- **Category parameters.** A parameter used where a category goes (`c`
  above) takes a category at the call.
- **Redefining.** Redefining `LOGNORM` (with `replace => true`) rewrites all
  four measures, in every sync mode, in the caller's transaction.
- **Dropping.** `drop_measure` refuses to drop a LAMBDA that is still called.
- **Storage.** LAMBDAs live in `improv.lambda`. The Improv GUI's Postgres
  store reads and writes the same table.
- **Limit.** A reduced parameter must be given a measure: write
  `LOGNORM(Authored, Person)`, not `LOGNORM(Authored * 2, Person)`.

## Also fixed while doing this

- **A logical-mode race.** A `define` committed while the logical worker
  applied a batch could be overwritten. The worker now reads
  `model_version` under the model lock.
- **storage_pg deadlock.** `storage_pg` deadlocked when several clients
  opened an empty database at once.
- **IMIGAMMA on the negative real axis.** `IMIGAMMA` is now correct there
  (e.g. Γ(0, −1) = −1.895 − πi), matching mpmath.

## Upgrading

`ALTER EXTENSION pg_improv UPDATE` from 0.9.0 to 0.10.0 adds:
- `improv.lambda`;
- the `count_distinct` aggregate;
- the new `status` columns;
- `define`'s `replace` argument;
- `drop_measure`.

`tests/upgrade.sh` checks the upgrade.
