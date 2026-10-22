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

## Verifying a release

Each GitHub Release carries, besides the archives:

| File | What |
| --- | --- |
| `SHA256SUMS` | The SHA-256 of every other file in the release. |
| `SHA256SUMS.sigstore.json` | A [cosign](https://docs.sigstore.dev/) keyless signature of `SHA256SUMS`: the certificate names the release workflow and the tag, and the signing is logged in Rekor, the public transparency log. |
| `<binary>-vX.Y.Z-<target>.cdx.json` | A CycloneDX SBOM per binary (`improv`, `improv-tui`, `improv-server`, `improv-gui`) and target, from `cargo cyclonedx`. |
| `pg_improv-X.Y.Z-pgN-x86_64-linux.cdx.json`, `improv-X.Y.Z-duckdb-linux_amd64.cdx.json` | The extensions' SBOMs. |

Check the signature on the sums, then the files against the sums (cosign
2.4 or later):

```sh
tag=v1.0.0
cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
  --certificate-identity "https://github.com/gburd/improv/.github/workflows/release.yml@refs/tags/$tag" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
sha256sum --ignore-missing -c SHA256SUMS    # macOS: shasum -a 256 --ignore-missing -c SHA256SUMS
```

`Verified OK` means GitHub Actions signed these sums while running
`release.yml` for that tag in `gburd/improv`; `--ignore-missing` checks only
the files you downloaded. Without cosign, `sha256sum -c` still catches a
corrupt download, but not a substituted one.

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
| CLI | `improv --help` | Scriptable; `improv help <command>` for one command, and see the [man pages](#man-pages-and-shell-completions). |
| TUI | `improv-tui m.db` | Arrows move, `e`/Enter edit a cell (or a derived measure's formula), Shift+arrows select a range (Sum in the status line), `p` pivot, `[ ]` page, `f`/`F` filter, `Tab` measure, `R` revert to saved, `?`/F1 function help, Tab completes names in a formula, `q` quit (autosaves). |
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

## Man pages and shell completions

Every command has a man page in `docs/man/`. The release archive lays them
out as `man/man1`, `man/man5` and `man/man7`, so with its `man/` on
`MANPATH`, `man -k improv` lists them all:

| Page | What |
| --- | --- |
| `improv(1)` | the CLI: every command, formulas in brief, examples, environment |
| `improv-<command>(1)` | one page per command, git-style: `improv-init`, `improv-add-category`, `improv-add-item`, `improv-add-measure`, `improv-add-derived`, `improv-define`, `improv-register-ext`, `improv-refresh-ext`, `improv-set`, `improv-list`, `improv-save-view`, `improv-show`, `improv-eval`, `improv-scenario`, `improv-export`, `improv-import-sql`, `improv-refresh-sql`, `improv-refresh-all`, `improv-serve-refresh`, `improv-export-sql`, `improv-import-csv`, `improv-export-csv`, `improv-stream` |
| `improv-gui(1)`, `improv-tui(1)`, `improv-server(1)` | the desktop, terminal and HTTP front ends |
| `improv-files(5)` | the model file, the CSV import format, the DuckDB model text |
| `improv-formulas(7)` | the formula language, in brief |
| `improv-functions(7)` | every built-in function |
| `pg_improv(7)` | the Postgres extension's SQL API and settings |

```sh
man -l docs/man/improv-import-csv.1               # from a checkout
MANPATH=improv-v1.0.0-x86_64-unknown-linux-gnu/man: man improv-import-csv
```

The `(1)` pages and `improv-functions(7)` are generated from the argument
parsers and the function registry; the hand-written parts of the `(1)` pages
are in `crates/cli/man/`. `UPDATE_DOCS=1 cargo test -p improv_cli`
regenerates them, and the test fails when they are stale or `groff -man -ww`
complains. The other pages are written by hand.

Shell completions for all four binaries are in `docs/completions/` (and the
archive's `completions/`), generated the same way:

```sh
source docs/completions/improv.bash                                  # bash
fpath=(docs/completions $fpath); autoload -Uz compinit; compinit     # zsh
cp docs/completions/*.fish ~/.config/fish/completions/              # fish
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
