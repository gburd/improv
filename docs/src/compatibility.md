# Compatibility

From 1.0.0 Improv follows [Semantic Versioning](https://semver.org/). A
1.x release (a *minor*, 1.y, or a *patch*, 1.y.z) does not break what this
page calls stable. Anything that would break it waits for 2.0.

Each section below lists, for one surface, what is stable, what a 1.x minor
may add, and what is not covered. What it does not list is not promised.

## The rules for every surface

- **A minor may add:** new functions, commands, flags, endpoints, columns,
  GUCs and optional arguments. An omitted new argument keeps today's
  behavior. Read results by name, not by position, wherever a list can grow
  (JSON fields, `SELECT *` columns, `improv_functions` rows).
- **A patch** only fixes bugs and documents. A behavior that contradicts
  this guide is a bug; its fix goes under **Fixed** in the
  [CHANGELOG](https://codeberg.org/gregburd/improv/src/branch/main/CHANGELOG.md),
  with the old and the new behavior.
- **Deprecation.** Nothing stable is removed in 1.x. To retire something, a
  minor marks it deprecated: the CHANGELOG and the page that documents it
  name the replacement. It keeps working, unchanged, for the rest of 1.x.
  Where the surface has a channel for it, using it warns: a `NOTICE` in
  Postgres, a line on stderr from a command, a `Deprecation` header over
  HTTP. 2.0 may remove it.
- **Upgrades go forward.** Data written by 1.y opens in every later 1.x.
  Opening it with an older release is not supported.

## Formulas

The formula language has its own promise, in
[The Formula Language: Language stability](./formulas.md#language-stability):
which formulas stay valid, what their results may and may not do across
1.x, how a bug fix is listed, and how a function is deprecated. It covers
every place a formula is written: models, `improv define`, the GUI and TUI
editors, `LANGUAGE improv` function bodies, `improv.define`, and DuckDB's
`improv_calc` and `improv(source, model)`.

## pg_improv

The extension lives in the schema `improv` (`relocatable = false`).

**Stable: the functions.** Their names, argument names, argument order and
types, defaults and return types:

| Function | Returns |
| --- | --- |
| `improv.create_model(name text)` | `bigint`, the model's id |
| `improv.drop_model(name text)` | `void` |
| `improv.bind_category(model text, category text, rel regclass, col text)` | `void` |
| `improv.bind_measure(model text, measure text, rel regclass, value_col text, agg text DEFAULT NULL)` | `void` |
| `improv.define(model text, definition text, replace boolean DEFAULT false)` | `bigint`, the measure's or LAMBDA's id |
| `improv.drop_measure(model text, measure text)` | `void` |
| `improv.recalc(model text)` | `bigint` |
| `improv.recalc(model text, measures text[])` | `bigint` |
| `improv.set_sync_mode(model text, mode text)` | `void` |
| `improv.enable_logical(model text)` | `void` |
| `improv.sync_publication()` | `void` |
| `improv.what_if(model text, edits jsonb)` | `TABLE(measure text, coord jsonb, value jsonb)` |
| `improv.status(model text)` | one row; columns below |

Also stable: the `agg` values (`sum`, `avg`, `min`, `max`, `count`,
`count_distinct`), the sync modes (`logical`, `trigger`, `commit`,
`manual`), the `edits` JSON of `what_if` (see
[Improv in Postgres](./postgres.md)), and `LANGUAGE improv`: a
`CREATE FUNCTION … LANGUAGE improv` that works in 1.0 works in every 1.x.
Every function runs as its caller (`SECURITY INVOKER`) and acts on a model
as the model's owner; that privilege model (see
[Privileges](./postgres.md#privileges)) is stable too.

**Stable: the GUCs.** Their names, types, units, contexts and defaults:

| GUC | Type | Default | Set in |
| --- | --- | --- | --- |
| `improv.max_cells` | integer | 5000000 | any session |
| `improv.raise_errors` | boolean | off | any session |
| `improv.commit_capture` | enum: `relations`, `rows` | `relations` | any session; read by `set_sync_mode` |
| `improv.database` | string | | `postgresql.conf`, restart; only with `shared_preload_libraries` |
| `improv.poll_ms` | integer, ms (10–60000) | 200 | `postgresql.conf`, reload |
| `improv.max_slot_lag` | integer, MB (0 = never) | 4096 | `postgresql.conf`, reload |
| `improv.max_changes` | integer | 10000 | `postgresql.conf`, reload |
| `improv.max_model_memory` | integer, MB (0 = no cap) | 0 | `postgresql.conf`, reload |

A minor may add GUCs. It does not change a default in a way that changes
results; a default that only tunes performance (`poll_ms`, `max_changes`)
may change, and the CHANGELOG says so.

**Stable: the result tables.** A derived measure's results are the table
`improv_<model>.<measure>`, both names lower-cased. It has one
`text NOT NULL` column per category of the measure, named by the category
lower-cased, in the measure's category order, then `value`: `double
precision`, `boolean`, `text` or `timestamp with time zone`, after the
measure's type. The primary key is the category columns. A measure with no
categories has just `value` and at most one row. You may index, `GRANT` and
query these tables; the extension owns their rows.

**Stable: `improv.status`.** Its columns, in this order, with these types:
`sync_mode text`, `slot text`, `lag_bytes bigint`, `confirmed_lsn text`,
`last_applied_at timestamptz`, `rows_applied bigint`, `last_error text`,
`incremental_applies bigint`, `rebuilds bigint`, `fold_bytes bigint`,
`engine_bytes bigint` (the last two added in 0.11.0).
[Monitoring](./monitoring.md) says what each means. A minor may append
columns. The logical worker's slot and publication are named `improv_db`.

**Not stable: the catalog tables.** `improv.model`, `improv.category`,
`improv.item`, `improv.measure`, `improv.binding`, `improv.category_binding`,
`improv.cell`, `improv.view`, `improv.meta`, `improv.lambda`,
`improv.model_version`, `improv.sync_state` and `improv.slot_to_drop` are
internal. Change models through the functions, never by writing these
tables. A minor may change them, with an update script. One exception, for
monitoring queries like the ones in this guide: `improv.model`'s `id`,
`name`, `sync_mode` and `owner` columns stay readable.

**Upgrades.** Every 1.x release ships an update script from each earlier
1.x, so `ALTER EXTENSION pg_improv UPDATE` (after installing the new
release and restarting) moves any 1.x database forward with its models and
results. `tests/upgrade.sh` checks this against the released tags. There
are no downgrade scripts. A minor may support a new Postgres major; it drops
one only after PGDG ends its support, and says so in the CHANGELOG.

## Model files

A model file is a [Mentat](https://crates.io/crates/mentatdb) store: a
SQLite database holding datoms. Its schema is the attributes in
`crates/storage_mentat/src/schema.rs` (`:category/*`, `:item/*`,
`:measure/*`, `:cell/*`, `:view/*`, `:meta/*`). Views, scenarios, named
LAMBDAs and external functions are JSON values inside it.

**Stable:** a file saved by 1.0 opens in every 1.x, with the same model.
The same holds for a Postgres model saved through the CLI, TUI, GUI or
server.

**How the schema changes.** There is no schema version number and no
separate migration step. Every open installs the schema, which is
idempotent: an attribute that exists is unchanged and a new one is added.
So a minor may only:

- add attributes, which an older file simply lacks (a new reader treats a
  missing one as its default; see the tests
  `store_without_category_order_still_loads` and
  `measure_format_round_trips_and_old_stores_load`);
- add fields to the JSON values, with defaults, so an older value still
  reads;
- read an older shape and write the new one on the next save (the views
  did this when one view became a canvas of several worksheets).

It does not rename, retype or remove an attribute, or change what a stored
value means. The `mentatdb` version is pinned exactly; a bump that changes
Mentat's own on-disk format must open older files unchanged, and the
CHANGELOG records it.

**Not stable:** opening a file with an older release than the one that
last saved it. It may load, but a save then drops whatever the older
release does not know. An added attribute usually survives: a release
ignores an attribute it does not know and its saves keep it (the test
`a_later_releases_attribute_is_ignored_and_kept`). So 0.11 opens a file
with 0.12's number formats, shows its numbers in the General form, and
keeps the formats when it saves; the same holds for a Postgres model,
whose `improv.measure.format` column 0.11 neither reads nor writes.

## The command line

`improv`, `improv-tui`, `improv-gui`, `improv-server` and their
[man pages](https://codeberg.org/gregburd/improv/src/branch/main/docs/man)
describe one surface.

**Stable:**

- the subcommands, their positional arguments and order, their flags, and
  the environment variables the man pages list (`IMPROV_LOCALE`,
  `IMPROV_EXTFN_TRUSTED`, `IMPROV_EXTFN_ALLOW_UNSANDBOXED`, the server's
  `IMPROV_ADDR` and token variables);
- the exit status of `improv`: **0** success, **1** the command failed
  (the reason is on stderr after `error:`), **2** the arguments were wrong
  (a usage error); `--help` and `--version` exit 0;
- stdout of the commands scripts read: what `crates/cli/tests/compat.rs`
  records. That test runs every invocation from the tests, the man pages'
  EXAMPLES, this guide and the smoke scripts, and checks its stdout byte
  for byte. A change that breaks it is a breaking change.

**A minor may add** subcommands, flags, and fields to the JSON of
`improv export`.

**Not stable:** the wording of error messages and `--help` text, and the
layout of the TUI and GUI.

## The HTTP API

`improv-server` serves its API under the versioned path `/v1/`. Within 1.x:

**Stable:** every `/v1` endpoint's method and path; its request fields and
their meaning; its response fields, their types and their meaning; its
status codes (400 a bad request, 401 no token, 403 a token that may not do
this, 404 no such measure, 409 a conflicting write, 413 a body over the
limit, 408 a timeout); bearer-token authentication, and what a read-only
token may do. `/health` stays public. [The HTTP Server](./server.md) lists
the endpoints.

**A minor may add:** endpoints, optional request fields, and response
fields. Clients must ignore fields they do not know. A minor may tighten a
limit (body size, rate, concurrency) only to fix a security or stability
problem, and says so.

**Breaking changes** go under a new prefix, `/v2/`. When it ships, `/v1`
keeps being served for the rest of 1.x. A deprecated `/v1` endpoint
answers with a `Deprecation` header and keeps working.

Paths outside `/v1/` (other than `/health`) are not covered, nor is any
endpoint the OpenAPI document marks `x-improv-unstable`. Today that is
`GET /v1/model`: it returns the store's internal serialization of the
model, which follows the Rust types (see "Rust crates" below) and may
change in any release. Read measures and values through `/v1/measures`.

## DuckDB

The extension `improv` (`INSTALL improv FROM community; LOAD improv;`).

**Stable:**

| Function | Arguments | Result |
| --- | --- | --- |
| `improv_calc` | `(formula VARCHAR)`, `(formula VARCHAR, inputs VARCHAR)` | `DOUBLE` |
| `improv_calc_text` | the same | `VARCHAR` |
| `improv_functions` | `()`, or `(query := VARCHAR)` | table: `name`, `category`, `signature`, `returns`, `description`, `aliases`, all `VARCHAR` |
| `improv` | `(source VARCHAR, model VARCHAR)` | table: `measure VARCHAR`, one `VARCHAR` column per category of the model, `value DOUBLE`, `value_text VARCHAR` |

`inputs` is a JSON object of names to values (`json_object(…)`). The
`model` text is the definition language [Improv in DuckDB](./duckdb.md)
documents, with formulas under the formula language's promise.

**The DuckDB version floor.** The extension is built against DuckDB's
*stable* C API with the floor `v1.5.6`, so one binary loads into DuckDB
1.5.6 and every later release that keeps that API, including 2.0. CI loads
it into both. A 1.x minor may raise the floor only when a DuckDB release
makes keeping it impossible, and the CHANGELOG says so first.

**A minor may add** functions, optional named parameters, and
`improv_functions` rows as the language grows.

## The Rust crates

**The crates are not a stable API in 1.x.** `improv_core_model`,
`improv_engine` and the storage crates are the internals of the programs
above, not a library: they are not published to crates.io, and a 1.x minor
may change any of their items.

The reason is `improv_core_model::Model`. It exposes ten `pub` fields
(`categories`, `items`, `measures`, `inputs`, `sql_sources`, `views`,
`external_fns`, `external_calls`, `scenarios`, `lambdas`), all plain maps.
Any caller can change them behind the model's back, which is how the
name-index compromise of 0.11 came about. Freezing that would freeze the
internals with it. Hiding them behind methods first is the road-to-1.0 task
J2; once that lands, a later minor may declare a smaller API stable.

To build on Improv, use a stable surface: the CLI (scripts), the
`/v1` HTTP API (programs), pg_improv (SQL over Postgres) or the DuckDB
extension. Each of them reads and writes the same model.
