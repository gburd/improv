# Contributing to Improv

Thanks for your interest. This document is the short version; `AGENTS.md` and
`.agent/AGENT_STEERING.md` (+ `.agent/steering/`) carry the full engineering
conventions.

## Before you commit

Every commit keeps the tree green. Run the full gate locally:

```sh
# fmt/clippy target only our 11 crates (not `--all`/`--workspace`): that would
# also walk the vendored Mentat path dependency (../mentat) and, transitively,
# whatever it depends on.
IMPROV_CRATES=$(ls crates | sed 's/^/-p improv_/')
cargo fmt $IMPROV_CRATES -- --check
cargo clippy $IMPROV_CRATES --all-targets --all-features -- -D warnings
cargo test --workspace          # or: cargo nextest run --workspace
cargo deny check
typos
cargo doc --workspace --no-deps
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

If you change a public API, the CLI surface, the formula/CNL grammar, or the
Mentat schema, update the rustdoc, the mdBook page under `/docs`, the man page,
and `CHANGELOG.md` in the same change. Stale docs are treated as defects.

## License

By contributing you agree your work is dual-licensed under Apache-2.0 OR MIT
(the recipient's choice), matching the project license.
