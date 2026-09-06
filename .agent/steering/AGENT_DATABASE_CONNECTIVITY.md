# AGENT_DATABASE_CONNECTIVITY.md
Database Connectivity Steering Document for Improv (Phase 7)

> **Status: IN PROGRESS — Phase 7.** SQLite import/export is implemented
> (`improv_storage_sql`); connection management, live-query measures, and other
> backends (Postgres/DuckDB/…) are still planned.
>
> This is connectivity *to* external databases (import/export, live queries) — a
> distinct concern from Improv's own persistence, which is always the embedded
> Mentat datom store (`AGENT_STORAGE_STEERING.md`). External SQL is a data
> source/sink, never Improv's system of record.

## 1. Purpose

Define how Improv connects to external SQL databases to:

- Import external data into categories, items, and measures
- Export computed measures/views back to SQL tables
- Drive live-query measures that refresh from a database

## 2. Candidate Databases

A unified connection layer targets, roughly in priority order:
PostgreSQL, SQLite, DuckDB (local analytics), MySQL/MariaDB, SQL Server, Oracle;
Snowflake and BigQuery are later additions.

## 3. Connection Management

Connections are stored as datoms in the model, with credentials handled
**out of band** — encrypted at rest, never written to the model in plaintext,
never exposed to any external-function runtime, never logged.

```clojure
{:connection/id   uuid
 :connection/type :postgres        ; :mysql :sqlserver :oracle :sqlite :duckdb
 :connection/name "SalesDB"
 :connection/uri  "<no secrets inline>"}
```

Lifecycle: create → test → save → edit → delete.

## 4. SQL Live-Query Measures

**Implemented** as SQL-backed input measures with a stored, refreshable source
(`improv_storage_sql::{add_sql_measure, refresh_sql_measure}`; CLI `import-sql`
+ `refresh-sql`; metadata persisted on the model as `SqlSource` /
`:measure/sql-source`). A future `SQL("...")` *formula form* (a grammar
extension per `AGENT_FORMULA_LANGUAGE.md` §11.3) would be sugar over this.

- SQL columns map to a measure's dimension categories (distinct values → items)
  and one value column; the mapping is recorded so refresh reuses it.
- **Refresh** re-runs the query and replaces the measure's cells; new dimension
  values become new items (items are interned by name, so refresh reuses them).
  Manual/on-demand today; on-load / interval refresh is a follow-up.
- The refreshed cells are ordinary input cells, so the engine recomputes
  dependents with no SQL path of its own. SQL measures are **marked**
  (`Model.sql_sources`) so determinism tests treat only non-SQL measures as pure.

## 5. Import / Export Workflows

**Implemented** (`improv_storage_sql`, SQLite; CLI `import-sql` / `export-sql`):

- **Import:** run a `SELECT` against a SQLite connection, map result columns to
  categories (distinct values → items) and one value column to a new input
  measure's numeric cells. SQL data enters as ordinary input cells — the engine
  gains no SQL path and stays deterministic.
- **Export:** write a measure's cells to a SQL table (one column per dimension
  category + a value column, created if absent). Identifiers are validated;
  values are bound as parameters (no interpolation of data).

**Planned:** GUI import/export wizards (column-mapping preview), other backends
(PostgreSQL/DuckDB/…), and the `SQL("...")` live-query measure form (§4).

## 6. Security

- Credentials encrypted at rest; never in plaintext, never exported, never
  logged.
- Parameterized queries only — no string concatenation; sanitized inputs.
- Connections isolated/sandboxed from the deterministic engine core.

## 7. Integration Boundaries

The implementation must preserve the invariants that make Improv's core
trustworthy:

- **Determinism.** External data introduces nondeterminism; live-query measures
  must be clearly marked and must not sit on paths the determinism tests treat
  as pure. The Time×Product oracle stays offline and deterministic.
- **Storage separation.** External SQL is a data *source/sink*, not Improv's
  persistence. The canonical model still lives in Mentat.
- **Engine API.** SQL measures enter through the same measure-collection
  abstraction as any other input; the engine core gains no SQL-specific code
  paths beyond a source operator.

## 8. Definition of Success

Connections are easy to manage; imports/exports are intuitive and reliable;
live queries refresh smoothly; errors are clear; security is airtight; and none
of it compromises the deterministic, Mentat-backed core.

## 9. Crash Safety

> Improv's own persistence (`improv_storage_mentat`, not this document's
> external-DB connectivity) is the embedded, SQLite-backed Mentat store. This
> section records the crash-safety audit of `ModelStore::save_model` done for
> IMPROV.txt's "atomic saves" requirement (Post-v0.5.0 plan, Phase A).

**Finding (before the fix).** `save_model` transacted the model in up to six
steps -- categories, items, measures, cells, views, then the `:meta/*`
singleton (external fns/calls + scenarios) -- each via its own
`Store::transact(&edn)` call. Reading `../mentat/src/store.rs` confirms
`Store::transact` opens a fresh `InProgress` (`Conn::begin_transaction`, which
issues `BEGIN IMMEDIATE` on the underlying `rusqlite::Connection`), transacts,
and calls `ip.commit()` (`COMMIT`) -- all within that one call. So each of the
six `transact` calls in the old `save_model` was its own independent, fully
committed SQLite transaction. A crash (or any error, e.g. an unsaveable value)
between two of those calls left the earlier steps durably committed and the
later ones missing or stale: a real "atomic saves" gap, exactly as IMPROV.txt
warns against. (Mentat's `(lookup-ref ...)` also only resolves against
*already-committed* data -- confirmed experimentally -- which is *why* the
original code needed six separate commits in dependency order in the first
place: items' `(lookup-ref :category/id ...)` had to see a committed category.)

**Fix.** `save_model` now opens ONE `InProgress` via `Store::begin_transaction`
(one `BEGIN IMMEDIATE`), issues the same per-kind `ip.transact(edn)` calls
(categories, items, measures, cells, views, meta) against that single
`InProgress`, and commits once at the end (`ip.commit()`). This still works
with unmodified `(lookup-ref ...)` EDN in `convert.rs`, because a lookup-ref in
a *later* `ip.transact` call resolves against data written by an *earlier*
`ip.transact` call on the **same** `InProgress`, even though nothing has been
committed to SQLite yet (verified experimentally against `../mentat`: a
lookup-ref in one `Store::transact` call cannot see a sibling entity from a
different `Store::transact` call, but it CAN see one written by an earlier
`ip.transact` call on the same still-open `InProgress`). If any step's EDN
fails to transact, `?` returns before `ip.commit()` runs; the `InProgress`
(and its `rusqlite::Transaction`) is dropped, and `rusqlite::Transaction`'s
`Drop` impl issues `ROLLBACK` by default -- so the underlying SQLite
transaction, and every step transacted so far in this save, is undone.

**Guarantee.** `save_model` is now all-or-nothing: either every category,
item, measure, cell, view, and the `:meta/*` blob for one call are durably
committed together in one SQLite transaction (WAL mode, already configured in
`make_connection` -- `PRAGMA journal_mode=wal`), or none of them are. A crash
at any point during a save leaves the store exactly as it was before that
`save_model` call started (the prior successful save, or empty on first save).
This relies on SQLite's own transactional durability, not a custom
write-ahead/temp-file scheme.

**Proof (tests in `crates/storage_mentat/src/lib.rs`).**

- `save_then_load_round_trips` -- baseline: saves a model touching every
  category the multi-step save writes (categories, items, measures, an input
  cell, a view, AND a non-empty `:meta/*` blob via an external fn + an
  external-call measure + a scenario), reloads into a fresh `ModelStore`, and
  asserts full `Model` equality (order-insensitive on the two cardinality-many
  `Vec` fields). This is the "happy path" the atomicity fix must not break.
- `save_partway_failure_does_not_leave_a_partial_write` -- the atomicity
  regression test. Saves M1 successfully, then attempts to save M2 (a
  modification of M1 that adds a category/item, renames a measure, sets an
  input cell to `Value::Number(f64::NAN)` -- a real, publicly-reachable
  transact failure, since Mentat's `:db.type/double` rejects the bare EDN
  token `NaN` with `BadValuePair` -- and would add a new view/scenario if the
  save got that far). The save fails as expected; the test then reloads and
  asserts the store equals M1 exactly, proving that none of M2's changes --
  including ones from steps that would have run *before* the failing `cells`
  step -- were left behind. Run against the pre-fix (six-separate-transacts)
  version of `save_model`, this test fails (the renamed measure and the new
  category/item DO leak through), which is the regression-catching property
  this test exists to guarantee going forward.

## 10. Document Index

Part of the full steering set:

- `AGENT_MASTER_STEERING.md`
- `AGENT_GUI_STEERING.md`
- `AGENT_ENGINE_STEERING.md`
- `AGENT_STORAGE_STEERING.md`
- `AGENT_FORMULA_LANGUAGE.md`
- `AGENT_DATABASE_CONNECTIVITY.md` (this document — Phase 7)
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md`
- `STEERING_SYSTEM_OVERVIEW.md`
