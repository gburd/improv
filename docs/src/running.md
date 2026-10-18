# Running Improv

This page is the operator's quick reference: build, run each front end, run
the tests, check coverage, and the security knobs. The [getting-started
guide](./getting-started.md) walks through modeling itself.

## Build

```sh
# Nix (recommended): toolchain + GUI runtime libs + dev tools
nix develop
cargo build --workspace --release

# Or plain Debian/Ubuntu (what the EC2 dev box used):
sudo apt-get install -y build-essential pkg-config libssl-dev libsqlite3-dev \
  libxkbcommon-x11-0 libgl1 libegl1 libgl1-mesa-dri python3 bubblewrap
curl -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain 1.97.0   # MSRV; any newer stable works
cargo build --workspace --release
```

The binaries land in `target/release/`: `improv` (CLI), `improv-tui`,
`improv-gui` and `improv-server`.

## A model to play with

```sh
IMPROV=target/release/improv bash docs/demo/sample-model.sh sample.db   # Time × Product × Region
target/release/improv list sample.db
```

Or build one by hand:

```sh
I=target/release/improv
$I init m.db
$I add-category m.db 1 Time;    $I add-item m.db 10 1 Q1; $I add-item m.db 11 1 Q2
$I add-category m.db 2 Product; $I add-item m.db 20 2 Widget
$I add-measure m.db 100 Price number input Time Product
$I add-measure m.db 101 Quantity number input Time Product
$I add-derived m.db 102 Revenue "Price * Quantity"
$I set m.db 100 10 --at Time=Q1 Product=Widget   # or --at Time=Q1,Product=Widget
$I set m.db 101 5  --at Time=Q1 --at Product=Widget
$I eval m.db 102
$I save-view m.db 1 "Revenue by quarter" 102 --rows Time --cols Product
```

## Run each front end

| Front end | Command | Notes |
| --- | --- | --- |
| CLI | `improv --help` | Scriptable; see `man docs/man/improv.1`. |
| TUI | `improv-tui m.db` | Arrows move, `e`/Enter edit a cell (or a derived measure's formula), Shift+arrows select a range (Sum in the status line), `p` pivot, `[ ]` page, `f`/`F` filter, `Tab` measure, `R` revert to saved, `?`/F1 function help, `q` quit (autosaves). |
| GUI | `improv-gui m.db` | NeXTSTEP-style workspace: application menu top-left, worksheet windows, model browser. Double-click or Enter edits a cell; right-click a tile to move or filter it; type `Name = formula` on the formula list's last line. Run inside `nix develop` on NixOS so the GL/X11 libraries are found. |
| Streaming | `producer \| improv stream m.db Revenue` | Edits in on stdin, changed cells out on stdout; saved at end of input. |
| Server | `IMPROV_API_TOKEN=secret improv-server m.db` | JSON API on `127.0.0.1:3000`; see [The HTTP Server](./server.md). |
| Postgres | `CREATE EXTENSION pg_improv` | Models over your tables; see [Improv in Postgres](./postgres.md). |

[Ways to Use Improv](./ways.md) explains when to pick which.

Example server calls:

```sh
curl -s -H 'Authorization: Bearer secret' localhost:3000/measures
curl -s -H 'Authorization: Bearer secret' -X POST localhost:3000/measures/102/eval \
  -H 'content-type: application/json' -d '{"edits":[]}'
```

## Open a Postgres-hosted model

Every front end also accepts a Postgres URL naming a model instead of a file:

```sh
improv list      'postgres://me:pw@127.0.0.1/sales?model=plan'
improv-tui       'postgres://me:pw@127.0.0.1/sales?model=plan'
improv-gui       'postgres://me:pw@127.0.0.1/sales?model=plan'
improv-server    'postgres://me:pw@127.0.0.1/sales?model=plan'
```

`?model=` is required; the other parameters go to libpq as usual. The model
lives in the `improv.*` catalog (created on first open when the `pg_improv`
extension isn't installed). Input measures bound to a table
(`improv.binding`) are read from it, and their edited cells are written back
to that table with `UPDATE`/`INSERT` (values only, no DDL). Unbound cells go
to `improv.cell`. As with a file store, a save fails with "reload and retry"
when another writer changed the model after you loaded it. There is no TLS
in this build, so a non-loopback host needs an explicit `sslmode=disable`
(use an SSH tunnel to a loopback port instead).

## Security defaults (and how to relax them)

Everything stored in a model is treated as untrusted: a shared model can
carry code or SQL.

| Behavior | Default | Override |
| --- | --- | --- |
| External functions run sandboxed (bubblewrap: read-only `/usr`, no `$HOME`, no network, cleared env, rlimits) | always, regardless of the model's `pure` flag | `IMPROV_EXTFN_TRUSTED=1` (host decides, never the model) |
| No bubblewrap installed | **refuse** to run external functions | `IMPROV_EXTFN_ALLOW_UNSANDBOXED=1` (rlimits only) |
| Server's refresh scheduler | **off** | `IMPROV_SCHEDULER=1` plus `IMPROV_EXTFN_ALLOW=<file>` listing `sha256(name\0body)` digests |
| Server with no token on a non-loopback address | **refuses to start** | `IMPROV_ALLOW_OPEN=1` |
| Read-only API tokens | none | `IMPROV_API_TOKENS_RO=a,b` |
| Server limits | 1 MiB body, 30 s request, 64 in flight, 10 s header read, 10k edits per `/eval` | `IMPROV_HEADER_TIMEOUT_MS` |
| SQL refresh queries | single `SELECT`/`WITH` only, run read-only; DuckDB external access off | none |
| Postgres transport | loopback/socket only (no TLS built in) | `sslmode=disable` in the connection string to accept cleartext to a remote host |
| CSV export of text starting `= + - @ \t \r` | prefixed with `'` (formula-injection safe) | `ExportSpec { neutralize_formulas: false }` (API) |
| Two processes saving one store | the second save is refused with "reload and retry" (no clobbering) | none |

## Tests

```sh
cargo test --workspace                          # ~750 tests, ~1 min
cargo test --workspace --release -- --ignored   # stress: 5M cells verified; the 10M case needs ~7 GB
```

Live Postgres tests run when `IMPROV_TEST_PG` is set, e.g.

```sh
IMPROV_TEST_PG="host=127.0.0.1 user=improv password=improv dbname=improv_test" \
  cargo test -p improv_storage_sql -p improv_storage_pg -p improv_store
```

The sandbox tests need `python3` and `bubblewrap`; without them they skip
(and say so) rather than fail.

## Coverage (≥ 90% functions and branches per crate)

```sh
rustup toolchain install nightly -c llvm-tools-preview
cargo install cargo-llvm-cov
scripts/coverage.sh            # prints a per-crate table; exits 1 below 90%
scripts/coverage.sh --html     # also writes target/coverage-html/html/index.html
```

Inline `#[cfg(test)]` modules are excluded from the denominator (via the
`coverage_nightly` cfg cargo-llvm-cov sets), so the percentages measure
library code. CI runs the same gate (`coverage` job).

## Full quality gate

```sh
P=$(ls crates | grep -v '^pg_improv$' | sed 's/^/-p improv_/')   # pg_improv has its own workspace
cargo fmt $P -- --check
cargo clippy $P --all-targets --all-features -- -D warnings
cargo test --workspace
cargo deny check
typos
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps
scripts/gui-shot-check.sh       # GUI screenshot vs golden (needs Xvfb, ImageMagick, xdotool)
scripts/coverage.sh             # coverage gate (nightly)
(cd crates/pg_improv && cargo fmt --check \
  && cargo clippy --features pg17 --all-targets -- -D warnings \
  && cargo pgrx test pg17 && ./tests/logical.sh)   # the extension
```
