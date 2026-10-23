# Docs review before 1.0 (F10), 2026-10-23

The road-to-1.0 plan's F10: every page in `docs/src` read against the
product (main at 96e0fbe plus this branch), checked three ways:

- **Numbers.** Every function count, version and limit.
- **Examples.** Every code block extracted and run where it can be:
  - the CLI, the HTTP server and the CNL endpoints against this build;
  - SQL against a scratch pg_improv 0.11.0 on PG 17;
  - DuckDB blocks under the v1.5.6 CLI with this build's extension.
- **Links.** Internal ones by `mdbook-linkcheck`; the 38 external ones
  with curl.

Not run: build and install steps (`cargo`, `nix`, `apt`), the GUI and
TUI (interactive), cosign, and fragments with placeholders (`<m>`,
`'...'`), which are not meant to run.

## Fixed in this change

1. **`tests/finance.sh` failed, and CI did not run it.** The guide's
   family-finance example (`postgres-finance.md`) waited for four
   per-model replication slots. Since 0.11.0 there is one, `improv_db`,
   so its logical-sync check timed out. It now waits for that slot, all
   51 checks pass on PG 17 and 18, and `ci/pg_improv.sh` runs it.
2. **`getting-started.md`, Quick start.** The two `add-derived` output
   blocks left out the `added derived measure …` line the command prints.
   That section is now a test (`crates/cli/tests/tutorials.rs`).
3. **`formulas.md`:**
   - `Square = LAMBDA(x, x * x)(Price - 1)` did not parse. A definition
     whose right-hand side starts with `LAMBDA` is a named LAMBDA. It now
     reads `(LAMBDA(x, x * x)(Price - 1))`, and the text says why.
   - The examples' `-- note` annotations are not formula syntax (a pasted
     one is a parse error: `unknown measure: a`). The page now says so.
     One note had no space before it (`…THIS])-- three-period`).
   - Every other `Name = …` line of the page's 20 blocks defines, checked
     against a model with the names it uses.
4. **`duckdb.md`.** The `Revnue` error example said `parse error at 12`.
   The product says `at 9`.
5. **`running.md`, Full quality gate:**
   - `P=$(ls crates | grep -v '^pg_improv$' …)` passed
     `-p improv_duckdb_improv`, which is not a workspace member, so
     `cargo fmt $P` failed at once. It now leaves out both extensions.
   - "~750 tests" is now ~1,200 (1,170 `#[test]` functions outside
     pg_improv).
   - The two extension CI scripts are listed.

## Found, not fixed: other owners' pages and code

1. **pg_improv: a fast shutdown can hang** (crate code, not docs).
   - After `tests/finance.sh`, `pg_ctl stop -m fast` waited forever: the
     `pg_improv sync` worker did not act on SIGTERM.
   - gdb shows it in `worker::peek` →
     `pg_logical_slot_peek_binary_changes` → `XLogReadRecord` →
     `pg_usleep`.
   - The postmaster stayed up in shutdown, holding its port, and the
     test's cleanup then deleted its data directory under it.
   - The test scripts now fall back to `-m immediate`. The worker needs a
     fix: check for interrupts while decoding waits for WAL, or bound the
     peek to the flushed LSN.
   - Repro: `KEEP=1 crates/pg_improv/tests/finance.sh`, then
     `pg_ctl -D ~/pgi-finance stop -m fast -t 20`.
2. **`ways.md`:**
   - **Streaming.** `improv stream m.db Revenue Margin` fails on the
     page's `m.db`: `error: no measure named or numbered 'Margin'`.
     Either drop `Margin` or define it in the CLI example.
   - **DuckDB.** The `improv('sales', …)` example has `input Price = price`
     without an aggregate, so on `duckdb.md`'s `sales` table (two 2025
     WidgetA rows) it fails with "Price: two source rows have the
     coordinate (2025, WidgetA)". `duckdb.md` uses `price max`.
   - **Parity table.** It says File ▸ New and File ▸ Open are disabled in
     the GUI. That is true today, and H7 changes it; update the table
     when H7 lands.
3. **`server.md` and `gui-tour.md`.** No error found. `server.md`'s
   route table matches `docs/openapi.json`'s ten operations. The GUI tour
   is interactive and was not run, apart from its `sample-model.sh` step,
   which works.

## Checked and correct

- **Versions:**
  - workspace 0.10.0 (`introduction.md`'s "Version 0.10.0" is the last
    release) and pg_improv 0.11.0;
  - MSRV 1.97; differential-dataflow and timely `=0.13.0`;
  - DuckDB v1.5.6 and v2.0; Postgres 15 to 18.
- **Counts:** README's "681 built-in functions" matches
  `improv_functions()` (681 rows, 672 distinct names) and `functions.md`'s
  681 entries.
- **GUCs:** every name, default, unit and context in
  `compatibility.md`'s table matches `pg_settings`, including `poll_ms`'s
  default of 200, which `capacity.md` and `monitoring.md` also quote.
- **Examples that ran as written:**
  - the pg_improv README quick start, including the update and `recalc`;
  - `postgres.md`'s five-minute tour;
  - every `monitoring.md` and `runbook-ops.md` query that has no
    placeholder;
  - the `ways.md` CLI, HTTP and stream examples (except `Margin`, above);
  - `cnl.md`'s three sentences, which parse and describe back as shown;
  - `running.md`'s CLI walk-through and `sample-model.sh`;
  - `duckdb.md`'s `improv_calc`, `improv_functions` and `improv(...)`
    examples, and its three error messages (after fix 4).
- **External links:** 35 of 38 answer 200. Of the other three:
  - two are a cosign certificate identity and an OIDC issuer, which are
    identifiers, not pages;
  - `https://crates.io/crates/mentatdb` gets a 404 from curl, because
    crates.io refuses non-browser clients. Its API confirms the crate
    exists.

## So it does not drift again

- The three tutorials and Getting Started's quick start are now tests.
  `docs/tutorials/mdtest.py` runs a page's blocks and compares each
  `text` block with the output. It runs from:
  - `cargo test -p improv_cli` (the CLI pages);
  - `ci/pg_improv.sh` (Postgres);
  - `ci/duckdb_improv.sh` (DuckDB).
- The references are generated, and their tests fail when they go
  stale:
  - `pg-reference.md`, from the live extension, in `ci/pg_improv.sh`;
  - `cli-reference.md` and `http-reference.md`, from clap and
    `docs/openapi.json`, in `cargo test -p improv_cli`.
- Next candidates for the same treatment: `duckdb.md` (its blocks need a
  shared `sales` table, which the page could create in a hidden step),
  and `postgres.md`'s tour.
