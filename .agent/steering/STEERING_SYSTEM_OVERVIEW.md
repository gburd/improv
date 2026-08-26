# STEERING_SYSTEM_OVERVIEW.md
Overview of the Improv Steering System

## 1. Purpose

This document is a unified, high-level summary of the Improv steering set. It
explains what each document covers and how the subsystems fit together into a
modern, Rust re-imagining of Lotus Improv and Quantrix Modeler.

It is the recommended entry point for new contributors, architecture reviewers,
and maintainers. For live "what is done right now" status, see
`.agent/AGENT_STEERING.md`; for contributor workflow and quality gates, see the
top-level `/AGENTS.md`.

---

## 2. System Vision

Improv is a cross-platform, standalone multidimensional spreadsheet built on:

- Named measures and explicit dimensions (categories/items)
- Differential-dataflow incremental recalculation
- The embedded Mentat (SQLite) datom store for persistence
- A dimension-aware formula DSL with controlled-natural-language translation
- Terminal- and service-first interfaces (CLI, TUI, server), then a desktop GUI

Design values: fast, deterministic, durable, declarative, and intuitive.

---

## 3. Architectural Pillars

### 3.1 Core Model (`improv_core_model`)

Categories, items, measures, coordinates, formulas, and value types. GUI- and
storage-free; the semantic backbone every other crate depends on.

### 3.2 Engine (`improv_engine`)

Differential-dataflow evaluation: formula compiler
(`Formula → TypedExpr → PlanNode`), dependency graph, broadcasting, aggregation,
and incremental recalculation. Built on `differential-dataflow` + `timely`,
pinned `=0.13.0` (delicate — see `AGENT_ENGINE_STEERING.md` §2).

### 3.3 Storage (`improv_storage_mentat`)

The model persists as immutable datoms in the embedded Mentat store
(Category / Item / Measure / Cell). Only inputs are stored; derived values are
recomputed. Save↔load round-trips losslessly.

### 3.4 Formula Language + CNL (`improv_nl_formula`)

A readable, dimension-aware DSL and a bidirectional **controlled**-natural-
language translation (v1 is CNL-only, not open English).

### 3.5 Interfaces (`improv_cli`, `improv_tui`, `improv_server`)

Thin drivers over the core stack: a headless CLI, a VisiCalc-style ratatui TUI,
and an axum JSON HTTP server. A desktop GUI follows in Phase 5, with the toolkit
chosen at the start of that phase.

---

## 4. How the Subsystems Interact

- **Core Model ↔ Engine** — the engine consumes categories/items/measures/
  formulas/input cells and produces derived collections and view slices.
- **Engine ↔ Interfaces** — interfaces request evaluated measures/slices and
  render them; the TUI (next increment) pushes edits back as deltas.
- **Storage ↔ Core Model** — Mentat loads/saves the model; derived values are
  never stored.
- **Formula Language ↔ Engine** — the compiler emits a typed, dimension-checked
  plan (`Join`/`Aggregate`/`Map`) the engine executes.
- **CNL ↔ Core Model** — `nl_formula` parses controlled English into `Formula`
  and describes a `Formula` back to English.

---

## 5. Scenarios and Multidimensional Modeling

Scenarios are modeled as an ordinary special category
(`Scenario → Base, Optimistic, Pessimistic`), so overrides, layering, pivoting,
and comparison reuse the existing dimensional machinery rather than a bespoke
subsystem.

---

## 6. Determinism and Performance

The engine is deterministic (bit-for-bit, insertion-order-independent), verified
by the canonical Time×Product revenue oracle. Performance targets: sub-ms (small
models), sub-100ms (medium), sub-500ms (large), achieved via incremental
recalculation, topological scheduling, DD operator parallelism, and cached
slices.

---

## 7. Error Model

Errors (syntax, type, dimension, cycle, runtime) are values that propagate
through the dependency graph to dependent measures and surface in the
interfaces.

---

## 8. Later Phases (5–7)

Committed plan beyond the v1 core (Phases 0–4), each with an invariant that
protects the core:

- **Desktop GUI (Phase 5)** — a view over the engine, no new modeling semantics;
  toolkit chosen at the start of the phase. See `AGENT_GUI_STEERING.md` §9.
- **External-language functions (Phase 6)** — Python first (Resolver One
  lineage), then R/Julia/WASM; pure, typed, dimension-declaring, so the engine
  stays deterministic. See `AGENT_FORMULA_LANGUAGE.md` §11.
- **SQL database connectivity (Phase 7)** — import/export and `SQL("...")`
  live-query measures over external databases; external SQL is a source/sink,
  never Improv's system of record. See `AGENT_DATABASE_CONNECTIVITY.md`.

---

## 9. Definition of Success (v1 core)

The v1 milestone (Phases 0–4) succeeds when a user can define
categories/items/measures (input + formula), build a
multidimensional pivot view, enter data and edit formulas, get instant
incremental recalculation on non-trivial models, and save/reopen across Linux,
macOS, and Windows — on a deterministic, tested engine.

---

## 10. Document Index

- `AGENT_MASTER_STEERING.md` — mission, architecture, principles, roadmap
- `AGENT_GUI_STEERING.md` — interfaces (CLI/TUI/server) and the desktop GUI
- `AGENT_ENGINE_STEERING.md` — differential-dataflow engine
- `AGENT_STORAGE_STEERING.md` — Mentat datom storage
- `AGENT_FORMULA_LANGUAGE.md` — formula DSL + CNL
- `AGENT_DATABASE_CONNECTIVITY.md` — SQL connectivity (Phase 7)
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md` — testing + release gates
- `STEERING_SYSTEM_OVERVIEW.md` — this document

Together, these define the complete Improv architecture.
