# Contributing to Improv

Thanks for your interest. This document is the short version; `AGENTS.md` and
`AGENT_STEERING.md` carry the full engineering conventions.

## Before you commit

Every commit keeps the tree green. Run the full gate locally:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
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

## Docs

If you change a public API, the CLI surface, the formula/CNL grammar, or the
Mentat schema, update the rustdoc, the mdBook page under `/docs`, the man page,
and `CHANGELOG.md` in the same change. Stale docs are treated as defects.

## License

By contributing you agree your work is licensed under Apache-2.0.
