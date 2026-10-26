# Introduction

![The Improv workspace](./img/gui-workspace.png)

Improv is a cross-platform **multidimensional spreadsheet** written in Rust,
in the line of Lotus Improv and Quantrix. It separates three things a
traditional grid tangles together:

- **Structure**: the named dimensions of a model (categories and their items).
- **Logic**: formulas over dimensions, not cell addresses.
- **Data**: the raw input values.

You model in named **categories**, **items** and **measures** instead of `A1`
cell coordinates:

```text
Revenue = Price * Quantity
Total   = SUM(Revenue OVER Time)
```

Derived values flow through a
[differential-dataflow](https://github.com/TimelyDataflow/differential-dataflow)
graph, so recalculation is always incremental: an edit propagates only to the
cells it actually affects.

## One model, many ways in

The same model opens in a NeXTSTEP-style **desktop GUI**, a **terminal UI**,
a scriptable **CLI**, a **streaming** pipe, a JSON **HTTP server**, and
**inside Postgres**, where `pg_improv` keeps results current as your tables
change and `LANGUAGE improv` turns a formula into a SQL function. A model is
a single store file, or lives in a Postgres database. [Ways to Use
Improv](./ways.md) says which to reach for and when.

## Status

Version 0.11.0 builds and passes its tests on Linux, macOS and Windows; the
Postgres extension supports Postgres 15 to 18. External functions in Python, R,
Julia or Pure run under a bubblewrap sandbox on Linux; WebAssembly functions
run in-process with fuel and memory limits. On other systems they are
refused unless the operator sets `IMPROV_EXTFN_ALLOW_UNSANDBOXED=1`. The
GUI has no Print yet, and the Postgres connection has no
TLS (use a loopback port or an SSH tunnel). See the
[changelog](https://codeberg.org/gregburd/improv/src/branch/main/CHANGELOG.md)
for what changed in each release.

## Reading order

1. [Concepts](./concepts.md): the ideas, before the syntax.
2. [When to Use Improv](./when-to-use.md), and when not.
3. [Ways to Use Improv](./ways.md): pick a front end.
4. [The Improv Model](./model.md): categories, items, measures, coordinates.
5. [Getting Started](./getting-started.md): build a model from the CLI.
6. [The Formula Language](./formulas.md): dimension-aware formulas and
   aggregation.
7. [The Desktop GUI](./gui-tour.md) and [Improv in Postgres](./postgres.md).
8. Running pg_improv in production: [Monitoring](./monitoring.md),
   [Capacity Planning](./capacity.md), [Backup and
   Restore](./runbook-backup.md), the [Operations Runbook](./runbook-ops.md)
   and [Troubleshooting](./troubleshooting.md).
9. [Architecture & Design](./architecture.md) and
   [Storage & Persistence](./storage.md): how it works inside.
