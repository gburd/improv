# Contributing to Improv

Thanks for your interest. This document is the short version; `AGENTS.md` and
`.agent/AGENT_STEERING.md` (+ `.agent/steering/`) carry the full engineering
conventions.

## Before you commit

Every commit keeps the tree green. Run the full gate locally:

```sh
# fmt/clippy target only our crates (not `--all`/`--workspace`), which would
# also lint the Mentat dependency. pg_improv is its own Cargo workspace (pgrx).
IMPROV_CRATES=$(ls crates | grep -v '^pg_improv$' | sed 's/^/-p improv_/')
cargo fmt $IMPROV_CRATES -- --check
cargo clippy $IMPROV_CRATES --all-targets --all-features -- -D warnings
cargo test --workspace          # or: cargo nextest run --workspace
cargo deny check
typos
RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps
mdbook build docs               # the guide (mdbook, mdbook-mermaid, mdbook-linkcheck)
```

The Postgres extension, from `crates/pg_improv` (needs `cargo-pgrx` 0.17 and
Postgres 17):

```sh
cargo fmt --check && cargo clippy --features pg17 --all-targets -- -D warnings
cargo pgrx test pg17
cargo pgrx install --release && ./tests/logical.sh && ./tests/scale.sh
ROWS=2500000 BATCHES=1000 ./tests/scale.sh   # the full-size acceptance run
SOAK_HOURS=72 ./tests/soak.sh                # the J4 soak (a short check: SOAK_HOURS=0.25 CHECK_MIN=2 EVENT_MIN=1)
```

A warning is a failure (`RUSTFLAGS=-Dwarnings` in CI).

## Commits

- **Author:** `Greg Burd <greg@burd.me>` (repo default).
- **Signed:** all commits are signed (SSH signing). Do not disable it.
- **Conventional Commits:** `type(scope): summary`
  (`feat`, `fix`, `docs`, `test`, `refactor`, `chore`, `ci`, `build`, `perf`).
  Scope is the crate, e.g. `feat(engine): aggregate over multiple categories`.
- One logical change per commit. Formatting-only changes are their own commit.
- Push to `origin` (Codeberg; mirrors to GitHub). Don't rewrite pushed `main`.

## Tests

New non-trivial logic ships with a test. We use, as appropriate: unit tests
(`#[cfg(test)]`), integration tests (`tests/`), property tests (`proptest`),
deterministic oracles (the Time×Product revenue model), fuzz targets
(`cargo fuzz`, `/fuzz`), and `--ignored` stress tests.

For GUI changes, run `scripts/gui-shot.sh` from the dev shell. It captures the
GUI on a headless Xvfb with a fixed demo model and writes
`target/gui-shot/gui.png` and `compare.png` (beside the NeXT reference) for
visual review. Missing X tools come from `nix shell`. See `--help`.

### pg_improv's upgrade test

`crates/pg_improv/tests/upgrade.sh` updates a database from every released
version (the `v*` tags that ship pg_improv) to the tree's, on the major of
`PG_CONFIG`. `ci/pg_improv.sh` runs it on each major. It builds each old
release once into `PGI_OLD_CACHE` (default `~/.cache/pgi-old/<version>/<major>`:
the `.so`, `.control` and `.sql` files; CI keeps it in `actions/cache`), so a
cold cache costs about a minute per release and a warm run about 7 s each.
A release adds its update script to `sql/`; the test fails if a released
version has no chain of scripts to the new one.

### The scale gate

`.github/workflows/scale-gate.yml` runs weekly, or by hand (`gh workflow run
scale-gate.yml -R gburd/improv`), and fails when a scale probe is more than
25% slower than `docs/reviews/bench/baseline.json`. Times are taken relative
to a reference probe from the same run, so runner speed cancels out (the
workflow's comment explains how). After a change that is meant to move a
probe, refresh the baseline on one machine:

```sh
cargo build --release -p improv_cli
cargo test --release -p improv_engine --test probe --test stress --no-run
for i in 1 2 3 4 5; do
  PG_BIN=<bindir with pg_improv> docs/reviews/bench/scale-gate.py run --out r$i.json
done
docs/reviews/bench/scale-gate.py baseline r*.json
```

## Coverage

Every crate must keep **>= 90% of functions and >= 90% of branches**
covered, measured on library code: inline `#[cfg(test)]` modules carry
`#[cfg_attr(coverage_nightly, coverage(off))]`, so the percentages are not
inflated (or deflated) by test code. Thin real-terminal/`main` shells are
excluded the same way, each with a comment saying why. Run the gate locally:

```sh
rustup toolchain install nightly -c llvm-tools-preview && cargo install cargo-llvm-cov
scripts/coverage.sh          # per-crate table; exits 1 below 90%
scripts/coverage.sh --html   # + target/coverage-html/html/index.html
```

Set `IMPROV_TEST_PG` to include the live Postgres tests, and install
`python3` + `bubblewrap` for the sandboxed external-function tests (they
skip, not fail, without them). CI runs the same gate.

## GUI screenshot golden

CI's `gui-shot` job runs `scripts/gui-shot-check.sh`: it renders the demo
model headless (Xvfb, Mesa llvmpipe) at 1400×900 and fails when more than
200 pixels differ from `docs/reviews/golden/gui-1400x900.png` by over 2%
(`magick compare -metric AE -fuzz 2%`). The capture and a highlighted
`diff.png` are uploaded as the `gui-shot` artifact.

A deliberate visual change refreshes the golden in the same commit. Render
it on Debian 13 (trixie), the distro of CI's `rust:1.97-trixie` container,
so Mesa and ImageMagick match:

```sh
scripts/gui-shot-check.sh --update   # writes the golden
scripts/gui-shot-check.sh            # 0 pixels differ
```

Look at the new golden before committing it.

## Docs

If you change a public API, the CLI surface, the formula/CNL grammar, the
Mentat schema or the `pg_improv` SQL API, update the rustdoc, the guide page
under `docs/src/`, the man pages under `docs/man/` (the `(1)` pages are
generated: edit `crates/cli/src/args.rs` or `crates/cli/man/`, then
`UPDATE_DOCS=1 cargo test -p improv_cli`), and `CHANGELOG.md` in the
same change. Stale docs are treated as defects.

The README's movies are reproducible: `docs/demo/tui-demo.sh` and
`docs/demo/pg-demo.sh` record the terminal ones (asciinema-style casts
rendered with `agg`), and `docs/demo/gui-movie.sh` records the GUI on a
headless X server. Re-record them when the UI they show changes.

The guide and API docs publish from `main`: GitHub Pages through
`.github/workflows/docs.yml`, and Codeberg Pages through
`.forgejo/workflows/pages.yml`, which pushes the built site to the `pages`
branch (it needs a `PAGES_PUSH_TOKEN` secret).

## Releasing

1. Bump the version (`Cargo.toml`'s workspace version,
   `crates/pg_improv/Cargo.toml` and `crates/duckdb_improv/Cargo.toml`),
   run `cargo update -w` in each of the four workspaces (the root,
   `crates/pg_improv`, `crates/duckdb_improv` and `fuzz`; CI builds each
   with `--locked`), add the update script
   `crates/pg_improv/sql/pg_improv--<old>--<new>.sql`, and move the
   CHANGELOG's `[Unreleased]` section under the new version.
2. Push to `main`, wait for CI, then push a signed annotated tag `vX.Y.Z`.
   Before tagging, dry-run the release on the GitHub mirror:
   `gh workflow run release.yml -R gburd/improv --ref main` builds and
   install-tests everything a tag would and publishes nothing.
3. On the GitHub mirror, `release.yml` builds and qualifies the binaries
   and pg_improv packages, makes the GitHub Release, publishes to
   [PGXN](https://pgxn.org/dist/pg_improv/) (`pgxn.yml`), and drafts the
   postgresql.org news for an X.Y.0 release (`ci/announce.sh`).
4. Check the run succeeded and that https://api.pgxn.org/dist/pg_improv.json
   lists the version (`pgxn.yml` waits for it and fails if it does not).
5. Verify the release as a user would ([Verifying a
   release](docs/src/running.md#verifying-a-release)): download it with
   `gh release download vX.Y.Z -R gburd/improv`, then run `cosign
   verify-blob` on `SHA256SUMS` and `sha256sum -c SHA256SUMS`.

Every Linux artifact is built in the manylinux_2_28 container (AlmaLinux
8), so it needs at most glibc 2.28 and GLIBCXX_3.4.25: RHEL 8 and every
later distribution. The same steps run locally with podman:

```sh
ci/manylinux.sh cargo build --release --target x86_64-unknown-linux-gnu \
  -p improv_cli -p improv_tui -p improv_server -p improv_gui
ci/manylinux.sh ci/pg_improv-dist.sh 17 dist      # pg_improv for PG 17
ci/manylinux.sh make configure release test_release   # the DuckDB extension
ci/glibc-floor.sh 2.28 3.4.25 target/manylinux/x86_64-unknown-linux-gnu/release/improv
ci/packages.sh bin X.Y.Z <staging-dir> pkg        # and `pg` per major; needs nfpm
ci/install-test.sh X.Y.Z pkg                      # every distribution, or name some
```

The `release` job runs `ci/release-attest.sh` on the downloaded artifacts
before it publishes. The script writes a CycloneDX SBOM per binary and
extension (`cargo cyclonedx`), `SHA256SUMS` over everything, and
`SHA256SUMS.sigstore.json`, a cosign keyless signature through the job's
GitHub OIDC token (`id-token: write`). Keyless means there is no signing key
or secret to keep, rotate or leak: the certificate names the workflow and the
tag, and Rekor logs the signing publicly. A minisign key would need a
secret in the repository, and users would have to find the public key
somewhere they trust. The script refuses an artifact it has no SBOM rule
for, so a new release file fails the release until the script learns about
it. To try it on locally built archives, without signing:

```sh
ci/release-attest.sh vX.Y.Z dist    # needs cargo-cyclonedx 0.5.9
```

PGXN needs the `PGXN_USERNAME`/`PGXN_PASSWORD` secrets on `gburd/improv`
(PGXN Manager account `gregburd`; PGXN has no API tokens). A tag released
before they were set is published by hand:
`gh workflow run pgxn.yml -R gburd/improv -f tag=vX.Y.Z`.

## License

By contributing you agree your work is dual-licensed under Apache-2.0 OR MIT
(the recipient's choice), matching the project license.
