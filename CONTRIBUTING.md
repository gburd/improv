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
cargo pgrx install --release && ./tests/logical.sh
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
under `docs/src/`, the man pages under `docs/man/`, and `CHANGELOG.md` in the
same change. Stale docs are treated as defects.

The README's movies are reproducible: `docs/demo/tui-demo.sh` and
`docs/demo/pg-demo.sh` record the terminal ones (asciinema-style casts
rendered with `agg`), and `docs/demo/gui-movie.sh` records the GUI on a
headless X server. Re-record them when the UI they show changes.

The guide and API docs publish from `main`: GitHub Pages through
`.github/workflows/docs.yml`, and Codeberg Pages through
`.forgejo/workflows/pages.yml`, which pushes the built site to the `pages`
branch (it needs a `PAGES_PUSH_TOKEN` secret).

## License

By contributing you agree your work is dual-licensed under Apache-2.0 OR MIT
(the recipient's choice), matching the project license.
