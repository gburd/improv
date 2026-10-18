# Project instructions — Improv

A cross-platform multidimensional spreadsheet (Lotus Improv / Quantrix lineage)
in Rust, with incremental recalculation on differential dataflow and storage on
the embedded (SQLite) Mentat fork.

## Steering & design docs (read these)

This file is the single top-level agent doc: contributor workflow, git hygiene,
and the CI quality gate. The rest of the steering lives under `.agent/`:

- **`.agent/AGENT_STEERING.md`** — the *live* phase-status tracker (DONE / NEXT).
- **`.agent/steering/`** — the *detailed design* set. Start with
  `STEERING_SYSTEM_OVERVIEW.md`, then `AGENT_MASTER_STEERING.md`, then the
  subsystem doc for your work (engine, storage, formula language, GUI,
  database connectivity, testing & release).
- **`IMPROV.txt`** (repo root) — the source design of record.

When a phase lands, update `.agent/AGENT_STEERING.md`. When a subsystem's design
changes, update the relevant `.agent/steering/` doc. (`.agent-steering-domains.md`
is tool-generated, local, and gitignored — not part of the steering set.)

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
  (`.claude/`, `.kiro/`, `.agent-steering-domains.md`, and any
  `.agent/**/.maki` or `.agent/**/.memelord` runtime artifacts).

## Quality gates (required — CI enforces, run locally before commit)

The full "tidy" gate — all must be error- AND warning-free:

1. `cargo fmt -p improv_<crate> ... -- --check` for every crate under
   `crates/` except `pg_improv` (rustfmt, config in `rustfmt.toml`). Scope to
   our crates, not `--all`, which would also lint the Mentat dependency.
2. `cargo clippy -p improv_<crate> ... --all-targets --all-features -- -D
   warnings` (config in `clippy.toml`), same scoping.
3. `cargo test --workspace` (all suites green; `IMPROV_TEST_PG` set runs the
   live Postgres tests too)
4. `cargo deny check` (licenses, advisories, bans — config in `deny.toml`)
5. `typos` (spell-check code + docs)
6. `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps`, and
   `mdbook build docs` (the guide, with link checking)
7. `scripts/gui-shot-check.sh` (GUI screenshot vs the golden) and
   `scripts/coverage.sh` (≥ 90% functions and branches per crate)
8. pg_improv, its own Cargo workspace: `ci/pg_improv.sh <PGMAJOR>` runs
   fmt, clippy, `cargo pgrx test`, pgspot, `tests/txn.sh`, `tests/logical.sh`,
   and on 17 `tests/upgrade.sh` and the PGXN dist build (see CONTRIBUTING.md)

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
- `README.md`, `CHANGELOG.md` (Keep a Changelog), `CONTRIBUTING.md`,
  `LICENSE-APACHE` + `LICENSE-MIT` (dual Apache-2.0 OR MIT).
- **Steering requires docs stay in sync:** any change to public API, CLI
  surface, formula/CNL grammar, or the Mentat schema updates the corresponding
  rustdoc, mdBook page, man page, and CHANGELOG in the SAME change. A phase
  landing updates `.agent/AGENT_STEERING.md`'s phase-status section. Stale docs
  are a defect.

## Layout

Cargo workspace, `crates/*`: `core_model`, `engine`, `nl_formula`,
`storage_mentat`, `storage_pg`, `store`, `storage_sql`, `storage_csv`,
`data_source`, `extfn`, `cli`, `tui`, `gui`, `server`, `pg_model`, and
`pg_improv` (the Postgres extension: its own workspace and `Cargo.lock`,
excluded from the root because pgrx needs `pg_config` to build). Shared deps pinned in
`[workspace.dependencies]`; the `differential-dataflow`/`timely` pin is delicate
(see `.agent/steering/AGENT_ENGINE_STEERING.md` §2 — do not bump blindly). MSRV
is rustc **1.97** (`rust-version` in the workspace; CI has a 1.97 job).

## CI / storage-backend pin

- **Mentat comes from crates.io**, pinned exactly, declared once in
  `[workspace.dependencies]`:
  `mentat = { package = "mentatdb", version = "=<x.y.z>" }` (Mentat 1.12+
  publishes the embedded engine as `mentatdb`; the library is still
  `mentat`, so `use mentat::…` is unchanged). Currently 1.16.0. Never a path
  dependency: `path = "../mentat"` made a fresh clone unbuildable. When
  Improv needs newer Mentat behavior: release it from Mentat `master` to
  crates.io, bump the version, commit the resulting `Cargo.lock`, and record
  an A/B storage benchmark (`docs/reviews/bench/bench-mentat.sh`). A local
  checkout is still handy for *developing* Mentat — use `[patch]` or `cargo
  add --path` temporarily, but never commit that.
- **GitHub** (`.github/workflows/`): `ci.yml` (fmt, clippy `-D warnings` on our
  crates only, typos, cargo-deny, matrix build+test on Linux/macOS/Windows ×
  default/all-features via nextest, MSRV, rustdoc+mdBook build, the
  coverage gate, the GUI screenshot golden, and `pg_improv` on PG 15–18 via
  `ci/pg_improv.sh`); `docs.yml` (guide + API on every push to main,
  deployed to GitHub Pages, https://gburd.github.io/improv/); `release.yml`
  (on a `v*` tag: qualify on four targets, all four binaries per target,
  pg_improv packages per PG major, the GitHub Release from the CHANGELOG
  section, PGXN upload, and `ci/announce.sh` for X.Y.0 releases).
- **Codeberg/Forgejo** (`.forgejo/workflows/`): `pages.yml` builds the guide on
  Codeberg's hosted `codeberg-small` runner and publishes it to
  https://gregburd.codeberg.page/improv/ with `actions/git-pages` (the
  automatic token; no secret); its `/api/` redirects to the GitHub Pages
  rustdoc. `ci.yml` mirrors the gate but needs a self-hosted runner with the
  `docker` label (Codeberg's hosted runners stop jobs after 2–10 minutes).
- Clippy in CI lints only Improv crates (`-p improv_*`); the Mentat
  dependency emits its own upstream warnings that must not fail our gate.
- Action versions are pinned by commit SHA; keep them current but pinned.
