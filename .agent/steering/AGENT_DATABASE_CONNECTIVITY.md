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

A `SQL("...")` formula form (a Phase 7 grammar extension; see
`AGENT_FORMULA_LANGUAGE.md` §11.3) produces a measure collection:

```text
SalesData = SQL("SELECT time, product, revenue FROM sales")
```

- SQL columns map to categories/items/measures; dimensionality is inferred from
  the selected columns.
- Refresh modes: manual, on model load, on interval, on demand.
- A refresh re-runs the query, updates the collection, and lets the engine
  recompute dependents incrementally (SQL measures behave like derived measures
  in the dependency graph).

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

## 9. Document Index

Part of the full steering set:

- `AGENT_MASTER_STEERING.md`
- `AGENT_GUI_STEERING.md`
- `AGENT_ENGINE_STEERING.md`
- `AGENT_STORAGE_STEERING.md`
- `AGENT_FORMULA_LANGUAGE.md`
- `AGENT_DATABASE_CONNECTIVITY.md` (this document — Phase 7)
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md`
- `STEERING_SYSTEM_OVERVIEW.md`
