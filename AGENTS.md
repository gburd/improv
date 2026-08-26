# Project instructions — Improv

A cross-platform multidimensional spreadsheet (Lotus Improv / Quantrix lineage)
in Rust, with incremental recalculation on differential dataflow and storage on
the embedded (SQLite) Mentat fork. See `AGENT_STEERING.md` for architecture,
constraints, and live phase status. `IMPROV.txt` is the source design.

See `.agent-steering-domains.md` for domain-specific steering (local).

## Git hygiene (required)

- **Author:** every commit is `Greg Burd <greg@burd.me>` (repo `user.name` /
  `user.email` are set; do not override per-commit).
- **Signed:** every commit MUST be signed (`commit.gpgsign = true`, SSH signing
  key configured). Never `--no-gpg-sign`.
- **Remote:** `origin` → Codeberg (`ssh://git@codeberg.org/gregburd/improv.git`),
  which auto-mirrors to GitHub. Push to `origin` only.
- **Conventional Commits:** `type(scope): summary` (types: feat, fix, docs,
  test, refactor, chore, ci, build, perf). Scope = crate name where sensible
  (e.g. `feat(engine): ...`, `test(core_model): ...`).
- **Atomic commits:** one logical change per commit; keep the tree green at
  every commit (build + tests + lints pass). Never commit a broken state to a
  shared branch. Formatting-only changes are their own commit.
- **No force-push to shared history** (the local git-policy allows force-push
  for maintainer branch cleanup, but treat `main` as protected — rebase local
  work, don't rewrite pushed `main`).
- Commit generated `Cargo.lock` (this workspace ships binaries). Do not commit
  `target/`, coverage artifacts, fuzz corpora, or agent-local files
  (`.claude/`, `.kiro/`, `.agent-steering-domains.md`).

## Quality gates (required — CI enforces, run locally before commit)

The full "tidy" gate — all must be error- AND warning-free:

1. `cargo fmt --all -- --check` (rustfmt, config in `rustfmt.toml`)
2. `cargo clippy --all-targets --all-features -- -D warnings` (config in
   `clippy.toml`)
3. `cargo test --workspace` (all suites green)
4. `cargo deny check` (licenses, advisories, bans — config in `deny.toml`)
5. `typos` (spell-check code + docs)
6. `cargo doc --workspace --no-deps` builds without warnings

`RUSTFLAGS=-Dwarnings` in CI: a warning is a failure. New code must not
introduce warnings anywhere in the workspace.

## Testing requirements

Every crate carries, where meaningful:

- **Unit tests** (`#[cfg(test)]`) — pure logic, one runnable check per non-trivial branch.
- **Integration tests** (`tests/`) — end-to-end through public APIs.
- **Property tests** (`proptest`) — invariants / round-trips (parse↔print,
  save↔load, encode↔decode).
- **Deterministic tests** — the canonical Time×Product revenue oracle with known
  numeric results; the engine must be reproducible run-to-run.
- **Fuzz targets** (`cargo-fuzz`, `/fuzz`) — the formula parser, EDN/coordinate
  (de)serialization, CNL parser. No crashes / panics on arbitrary input.
- **Stress tests** — large models (hundreds of thousands of cells) exercising
  incremental recalculation; gated behind `--ignored` or a feature to keep the
  default suite fast.

Prefer `cargo nextest run` for the fast suite. Property/fuzz crates:
`proptest`, `arbitrary`. (See the `hegel` skill for property-test conventions.)

## Documentation (keep current, accurate, concise)

- Rustdoc on all public items; `cargo doc` warning-free.
- `/docs` mdBook (user guide + design). Build clean (`mdbook build`), links
  checked (`mdbook-linkcheck` / `lychee`).
- Man page(s) for the `improv` CLI under `/docs/man` (generated or hand-written
  roff), one per command surface.
- `README.md`, `CHANGELOG.md` (Keep a Changelog), `CONTRIBUTING.md`, `LICENSE`
  (Apache-2.0).
- **Steering requires docs stay in sync:** any change to public API, CLI
  surface, formula/CNL grammar, or the Mentat schema updates the corresponding
  rustdoc, mdBook page, man page, and CHANGELOG in the SAME change. A phase
  landing updates `AGENT_STEERING.md`'s phase-status section. Stale docs are a
  defect.

## Layout

Cargo workspace, `crates/*`: `core_model`, `storage_mentat`, `engine`, `cli`,
`nl_formula` (+ `tui`, `server` as phases land). Shared deps pinned in
`[workspace.dependencies]`; the `differential-dataflow`/`timely` pin is delicate
(see `AGENT_STEERING.md` — do not bump blindly).

## CI / storage-backend pin

- **Mentat is a sibling path dependency** (`mentat = { path = "../mentat" }`).
  CI clones it next to the checkout from
  `https://codeberg.org/gregburd/mentat.git` at branch **`improv-base`**
  (env `MENTAT_REPO` / `MENTAT_REF` in the workflows). When Improv needs newer
  Mentat behavior, commit + push it to `improv-base` in the mentat repo first,
  then bump the ref if the branch name changes.
- **GitHub** (`.github/workflows/`): `ci.yml` (fmt, clippy `-D warnings` on our
  crates only, typos, cargo-deny, matrix build+test on Linux/macOS/Windows ×
  default/all-features via nextest, MSRV, rustdoc+mdBook build); `docs.yml`
  (build guide+API, deploy to GitHub Pages); `release.yml` (tag-triggered
  cross-platform qualify + CLI artifact upload).
- **Codeberg/Forgejo** (`.forgejo/workflows/`): `ci.yml` mirrors the gate;
  `pages.yml` publishes the site to the `pages` branch (needs a
  `PAGES_PUSH_TOKEN` repo secret with contents:write).
- Clippy in CI lints only Improv crates (`-p improv_*`); the vendored Mentat
  dependency emits its own upstream warnings that must not fail our gate.
- Action versions are pinned by commit SHA; keep them current but pinned.
