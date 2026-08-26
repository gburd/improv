# AGENT_TESTING_AND_RELEASE_QUALIFICATION.md
Testing & Release Qualification Steering Document for Improv

## 1. Purpose

This document defines the testing, validation, and release-qualification strategy
for Improv. It ensures every subsystem — Core Model, Engine, Storage, Formula
Language / CNL, and the interfaces (CLI, TUI, server) — meets correctness,
determinism, and performance standards before release.

It complements the contributor-facing quality gate in the project root
`AGENTS.md`; where they overlap, `AGENTS.md` is the operational source of truth
for commands and CI.

---

## 2. Testing Philosophy

- **Determinism** — same inputs produce same outputs, bit-for-bit, run to run.
- **Incrementality** — small edits trigger small recomputation.
- **Dimensional correctness** — measures align, broadcast, and aggregate
  correctly.
- **Semantic correctness** — named-measure formulas behave predictably.
- **Reproducibility** — tests give identical results across machines and OSes.
- **Isolation** — tests do not depend on external state or network.
- **Automation** — CI runs the full gate on every commit.

---

## 3. The Quality Gate (CI-enforced; run locally before commit)

All must be error- **and** warning-free (`RUSTFLAGS=-Dwarnings` in CI):

1. `cargo fmt --all -- --check` (config in `rustfmt.toml`)
2. `cargo clippy --all-targets --all-features -- -D warnings` (config in
   `clippy.toml`; CI lints only Improv crates `-p improv_*`, since the vendored
   Mentat dependency emits its own upstream warnings)
3. `cargo test --workspace` (prefer `cargo nextest run` for the fast suite)
4. `cargo deny check` (licenses, advisories, bans — `deny.toml`)
5. `typos` (spell-check code + docs)
6. `cargo doc --workspace --no-deps` builds without warnings

MSRV (rustc 1.97+) is part of the CI matrix.

---

## 4. Test Categories

### 4.1 Unit Tests (`#[cfg(test)]`)

Pure logic, one check per non-trivial branch:

- Formula parser, AST builder, type checker, dimension checker, operator planner
- Datom serialization, coordinate/formula codecs
- CNL grammar rules
- Interface logic (non-rendering)

### 4.2 Integration Tests (`tests/`)

End-to-end through public APIs:

- Engine + Formula compiler
- Engine + Storage (Mentat load → evaluate)
- CLI over a Mentat-backed store
- Server endpoints
- CNL parse/describe against a model

### 4.3 Property Tests (`proptest`)

Invariants and round-trips: model JSON round-trip, coordinate codec round-trip,
formula parse↔print, save↔load, CNL parse↔describe.

### 4.4 Deterministic Oracle

The canonical **Time×Product revenue** fixture with known results
(1000 / 1000 / 1200 / 1600): the engine must reproduce it bit-for-bit and be
independent of insertion order.

### 4.5 Fuzz Targets (`cargo-fuzz`, `/fuzz`)

No crashes/panics on arbitrary input for: the formula parser, EDN/coordinate
(de)serialization, and the CNL parser.

### 4.6 Stress Tests

Large models (100k+ cells) exercising incremental recalculation; gated behind
`#[ignore]` or a feature so the default suite stays fast.

---

## 5. Subsystem Test Focus

### 5.1 Engine

- Formula evaluation: arithmetic, comparison, aggregation, broadcasting, error
  propagation.
- Differential dataflow: incremental updates, delta propagation, operator
  correctness, deterministic results.
- Dependency graph: cycle detection, topological ordering, multi-layer derived
  measures.

### 5.2 Storage

- Datom creation and immutability; save↔load round-trip; schema consistency;
  transaction atomicity.

### 5.3 Formula Language / CNL

- Parser and AST correctness; type inference and type errors; dimension
  alignment and broadcasting; CNL parse/describe round-trip.

### 5.4 Interfaces

- CLI command behavior and deterministic output.
- TUI rendering/navigation logic (non-rendering assertions where possible).
- Server endpoint contracts.

---

## 6. Cross-Platform Qualification

The engine and storage must behave identically on **Linux, macOS, and Windows**
(CI matrix builds and tests all three, default and all-features, via nextest).

---

## 7. Documentation Qualification

Docs are part of the gate; stale docs are a defect. Any change to a public API,
CLI surface, formula/CNL grammar, or the Mentat schema updates — in the **same
change** — the corresponding rustdoc, mdBook page (`/docs`), man page
(`/docs/man`), and `CHANGELOG.md`. A phase landing updates
`.agent/AGENT_STEERING.md`'s phase-status section and the relevant document
here.

---

## 8. Release Qualification

A release candidate is approved only when:

- **Functional** — all unit, integration, property, and determinism tests pass.
- **Performance** — recalculation and view materialization within the targets in
  `AGENT_ENGINE_STEERING.md` §13; no regressions.
- **Stability** — no crashes, deadlocks, memory leaks, or data corruption; fuzz
  targets clean.
- **Cross-platform** — Linux/macOS/Windows builds and tests green.
- **Documentation** — rustdoc, mdBook, man pages, and CHANGELOG current.

---

## 9. Release Process

- **Stages:** Alpha → Beta → Release Candidate → General Availability.
- **Checklist:** gate green → docs updated → release notes written → version
  bumped → signed tag created → artifacts built (CI `release.yml`, tag-triggered
  cross-platform qualify + CLI artifact upload).
- **Git hygiene:** signed, conventional commits authored by
  `Greg Burd <greg@burd.me>`; push to `origin` (Codeberg, auto-mirrors to
  GitHub); never rewrite pushed `main`. See `AGENTS.md`.
- **Post-release:** smoke tests and crash/issue triage.

---

## 10. Definition of Success

Testing and release qualification succeed when releases are stable, the engine is
deterministic, storage is durable, no regressions ship, errors are clear, and
users trust the system.

---

## 11. Document Index

Part of the full steering set:

- `AGENT_MASTER_STEERING.md`
- `AGENT_GUI_STEERING.md`
- `AGENT_ENGINE_STEERING.md`
- `AGENT_STORAGE_STEERING.md`
- `AGENT_FORMULA_LANGUAGE.md`
- `AGENT_DATABASE_CONNECTIVITY.md`
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md` (this document)
- `STEERING_SYSTEM_OVERVIEW.md`
