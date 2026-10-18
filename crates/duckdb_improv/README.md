# `improv` — the DuckDB extension

Improv's formula language and multidimensional models inside DuckDB. The
logic lives in `crates/duckdb_model` (`improv_duckdb_model`, tested in the
root workspace); this crate is the thin C-API shell, the same split as
`pg_model` / `pg_improv`.

```sql
LOAD improv;
SELECT improv_calc('ROUND(PMT(rate/12, n, -pv), 2)',
                   json_object('rate', 0.06, 'n', 360, 'pv', 200000));   -- 1199.1
SELECT * FROM improv('sales', '
  category Time = year
  category Product = product
  input Price = price
  input Quantity = qty sum
  Revenue = Price * Quantity
  Share = Revenue / SUM(Revenue OVER Product)
  Growth = Revenue - PREV(Revenue, Time, 1, 0)');
```

The user guide is `docs/src/duckdb.md` (*Using Improv → DuckDB*).

## SQL API

| Function | Returns | |
|---|---|---|
| `improv_calc(formula VARCHAR [, inputs VARCHAR])` | `DOUBLE` | A scalar formula. Booleans are 1/0, dates their spreadsheet serial; text and error values are NULL. |
| `improv_calc_text(formula [, inputs])` | `VARCHAR` | The same, any type as text; errors as their code (`#DIV/0!`). |
| `improv_functions([query := VARCHAR])` | table | `name, category, signature, returns, description, aliases`; with `query`, the functions `functions::search` finds, best match first. |
| `improv(source VARCHAR, model VARCHAR)` | table | `measure`, one `VARCHAR` column per category (declaration order), `value DOUBLE`, `value_text VARCHAR`. One row per derived cell. |

- **inputs** is a JSON object (`json_object(...)` or a literal). A key is an
  input name; its JSON type is the input's type (number, boolean, string).
  JSON `null` is a missing input: a result that depends on it is NULL. A
  NULL `formula` gives NULL. A formula that does not parse or type-check,
  or bad JSON, fails the query.
- Compiled formulas are cached (1024 entries, shared by all threads), keyed
  by formula text and the inputs' names and types.
- **source** is a table or view name, or a query (`SELECT`, `WITH`, `FROM`,
  `VALUES`, or parenthesized). It is read at bind time through one of the
  extension's own connections (a pool cloned from the loading connection
  in the entrypoint, `IMPROV_DUCKDB_POOL`, default one per core up to 8, as
  in mentat's DuckDB extension). It therefore sees committed data, not the
  calling transaction's uncommitted writes, and not the caller's `TEMP`
  tables (they belong to one connection): materialize into a regular table
  first.
- **model**: one item per line; blank lines and `#` / `--` comments skipped.

  ```text
  category <Name> = <column>
  input    <Name> = <column> [sum|avg|min|max|count]
  <Name> = <formula>
  ```

  Every input ranges over every category. A category's items are its
  column's distinct non-NULL values, in the column's own order (numbers
  numerically, dates chronologically), which `PREV`/`NEXT` follow. Without
  an aggregation, two rows on one coordinate are an error. A row with a
  NULL category value is skipped; a NULL input value is no cell. Inputs may
  be numeric (any integer, decimal or float type), `BOOLEAN` or `VARCHAR`;
  `count` reads any type. A column name with spaces is `"double quoted"`.
  Errors name the line: `improv: model line 3: definition: parse error …`.

## Building

```sh
git submodule update --init crates/duckdb_improv/extension-ci-tools
cd crates/duckdb_improv
make configure        # venv with duckdb==1.5.6 + the SQLLogicTest runner
make release          # build/release/improv.duckdb_extension
make test_release     # test/sql/improv.test
DUCKDB=/path/to/duckdb bash test/smoke.sh                  # standalone CLI
python3 test/concurrency.py build/release/improv.duckdb_extension
DUCKDB=... IMPROV=target/debug/improv bash test/compare_cli.sh  # vs `improv eval`
```

`make release` at the repo root does the same and copies the binary to
`build/release/extension/improv/`, where the registry CI looks. The crate is
its own Cargo workspace (listed in the root's `exclude`): the root's
`storage_sql` links duckdb with `bundled`, and cargo would unify that with
this crate's `loadable-extension` duckdb.

Load an unsigned local build with `duckdb -unsigned` (or
`SET allow_unsigned_extensions = true`), then `LOAD '/path/improv.duckdb_extension'`.

## Pins (and why they differ from mentat's)

| Pin | Value | Where |
|---|---|---|
| duckdb-rs | `~1.10506.1` (DuckDB v1.5.6) | `Cargo.toml` |
| C API | **stable** (`USE_UNSTABLE_C_API` unset), floor `TARGET_DUCKDB_VERSION=v1.5.6` | `Makefile` |
| extension-ci-tools | `v2.0-cyanoptera` (969bc76) | submodule; `.github/workflows/duckdb.yml` |
| Tested DuckDB | v1.5.6 (CLI, Python) and v2.0.0-alpha46057 (CLI built at b15358c3, Python dev wheel) | CI `test-duckdb`, `duckdb.yml` |

mentat's DuckDB extension uses the **unstable** C API: its footer says
`C_STRUCT_UNSTABLE` + `v1.5.6`, and DuckDB loads such a binary only into
exactly v1.5.6. That was right while the registry built against v1.5.6. It
no longer does:

- **The registry builds against v2.0 now.** Since duckdb/community-extensions
  PR #3079 (merged 2026-10-15), `.github/workflows/build.yml` builds and
  deploys every merged descriptor with `duckdb_version: b15358c3…`,
  `duckdb_tag: v2.0.0-alpha46057`, `ci_tools_version: v2.0-cyanoptera`. The
  v1.4.5 (`build_andium.yml`) and "next" (`build_next.yml`) workflows are
  `if: false`; `repo.variegata` / `repo.andium` refs are used only when
  someone dispatches `build.yml` by hand for v1.5.6 / v1.4.5.
- **Evidence it deploys to the 2.0 path:** extensions merged after #3079
  (finetype, duck_hunt, clickhouse_scanner) have fresh artifacts under
  `community-extensions.duckdb.org/v2.0.0-alpha46057/linux_amd64/`, dated
  2026-10-16, while their `v1.5.6/` artifacts are from September.
- **Its test step still uses DuckDB 1.5.6.** The registry's test venv runs
  `pip install duckdb` (= 1.5.6, the latest stable on PyPI), so an
  unstable-API v1.5.6 binary passes the registry's CI (mentat #3072 does)
  and is then published where DuckDB 2.0 users install from. We checked:
  DuckDB 2.0 refuses mentat's v1.5.6 binary ("built specifically for DuckDB
  version 'v1.5.6'").
- **The stable C API is forward compatible.** A `C_STRUCT` binary records a
  C API *floor*. DuckDB loads it when the floor is ≤ its own and hands it
  the v1 function-pointer struct. finetype's stable-API binary (floor
  v1.2.0) loads into both v1.5.6 and 2.0. In v2.0.0-alpha46057's
  `duckdb_extension.h` the `duckdb_ext_api_v1` struct is v1.5.6's 546
  entries with identical signatures in the same order, plus 2 appended
  (`duckdb_create_timestamp_tz_ns`, `duckdb_get_timestamp_tz_ns`), 548 in
  all. duckdb-rs 1.10506's struct has the same 546. A v1.5.6-floor binary
  is therefore valid against 2.0's struct.
- **Nothing we use is unstable-only.** `vscalar` and `vtab` (scalar and table
  functions, bind/init parameters, named parameters, data chunks) are all
  in the stable struct; the extension builds and runs with the stable ABI.
- **Proof, kept tested:** one `make release` artifact passes `test/smoke.sh`
  and `test/sql/improv.test` under the DuckDB v1.5.6 CLI and Python package
  *and* the v2.0.0-alpha46057 CLI and 2.0 dev Python package. CI's
  `test-duckdb` job and `.github/workflows/duckdb.yml` run both.

Consequences: a DuckDB patch release does **not** need an improv release.
What can still break it is DuckDB dropping or renumbering the v1 C API
(there is a `duckdb_extension_v2.h`). If 2.0 final does that, the smoke test
against 2.0 fails first; then bump duckdb-rs to its 2.0 line. To raise the
floor, bump duckdb-rs and `TARGET_DUCKDB_VERSION` together: the floor must
never be lower than the struct duckdb-rs was generated from. Do not set
`USE_UNSTABLE_C_API=1` here without reading this section again.

## Registry

`community-extensions/description.yml` is the descriptor. The first
registration is a manual PR adding it as `extensions/improv/description.yml`
to duckdb/community-extensions with `repo.ref` set to a commit that
`.github/workflows/duckdb.yml` qualified on every platform. After that, each
release tag's `duckdb-registry-pr` job (`release.yml`) opens the version
bump PR from the fork `gburd/community-extensions`; a DuckDB maintainer
merges it. `excluded_platforms` drops musl, mingw and wasm (untried).
