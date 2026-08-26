# AGENT_MASTER_STEERING.md
Master Steering Document for Improv

## 1. Mission and Scope

Improv is a cross-platform, standalone **multidimensional spreadsheet** in Rust,
inspired by Lotus Improv and Quantrix Modeler. It separates *structure*, *logic*,
and *data*: users model in named **categories**, **items**, and **measures**
instead of cell coordinates, and derived values flow through a
**differential-dataflow** graph so recalculation is always incremental.

Core deliverables:

- A dimension-aware, named-measure model (categories, items, measures, cells)
- A formula language based on named measures, not cell references
- A differential-dataflow engine for incremental recalculation
- Persistence on the embedded **Mentat** (SQLite-backed) datom store
- First-iteration interfaces: **TUI** (VisiCalc-like), **CLI**, and **server**
- Controlled-natural-language (CNL) formula translation
- A desktop GUI (Phase 5)
- External-language functions (Phase 6) and SQL database connectivity (Phase 7)

This document defines the overall architecture, design philosophy, and
inter-module relationships. It is the top-level map for the steering set; each
subsystem has its own document (see §8).

> **Two steering files share the `AGENT_STEERING` name; keep them distinct.**
> `.agent/AGENT_STEERING.md` is the *live phase tracker* (what is
> DONE / NEXT right now). This directory's documents (`.agent/steering/`) are
> the *detailed design* the tracker points into. When a phase lands, update
> `.agent/AGENT_STEERING.md`; when the
> design of a subsystem changes, update the document here.

---

## 2. Architectural Overview

Improv is built from a GUI-free, storage-free **core** wrapped by successive
interface and integration layers:

1. **Core Model** — categories, items, measures, coordinates, formulas, values
2. **Engine** — formula compiler + differential-dataflow evaluation
3. **Storage** — model ⇄ datoms on the embedded Mentat store
4. **Formula Language** — dimension-aware DSL, and CNL translation on top
5. **Interfaces** — CLI, TUI, server (Phases 2–3); desktop GUI (Phase 5)
6. **Integrations** — external-language functions (Phase 6) and SQL connectivity
   (Phase 7)

The dependency direction is strict: `core_model` depends on nothing in the
workspace; `engine` and `storage_mentat` depend on `core_model`; interfaces
depend on all three; `nl_formula` depends on `core_model`.

---

## 3. Core Principles

### 3.1 Named Measures, Not Cell References

Formulas reference concepts, not cell addresses:

```text
Revenue = Price * Quantity
```

This is the foundation of Improv and Quantrix.

### 3.2 Explicit Dimensions (Categories)

Data lives in categories, each with items:

- Time → 2025, 2026
- Product → Widget A, Widget B

Measures declare which categories they range over:

```text
Quantity[Time, Product]
Price[Product]
Revenue[Time, Product]
```

### 3.3 Automatic Broadcasting

Dimension alignment is implicit; the engine broadcasts lower-arity measures
across missing dimensions:

```text
Revenue[Time, Product] = Price[Product] * Quantity[Time, Product]
```

`Price` is broadcast across `Time`.

### 3.4 Always-Incremental Recalculation

Each measure is a materialized view; edits propagate as deltas through
differential dataflow. Only affected coordinates are recomputed, so large models
stay responsive.

### 3.5 Deterministic Core

The engine must be deterministic: identical inputs produce identical outputs,
bit-for-bit, independent of insertion order or run. The canonical Time×Product
revenue fixture (with known numeric results) is the oracle.

### 3.6 Semantic, Durable Storage

The model persists as immutable datoms in the embedded Mentat store — a
queryable, auditable record of categories, items, measures, and cells.

### 3.7 Controlled Extensibility

The core keeps a tight, deterministic surface. Later-phase extension points
(external-language functions, SQL connectivity) are built after the v1 core and
designed so they never compromise the deterministic core (see §7).

---

## 4. Module Relationships

### 4.1 Core Model ↔ Engine

The engine consumes categories, items, measures, formulas, and input cells; it
produces derived measure collections and view slices.

### 4.2 Engine ↔ Interfaces

Interfaces (CLI/TUI/server) request evaluated measures and view slices and
render them. The TUI additionally pushes edits back as deltas.

### 4.3 Storage ↔ Core Model

Storage loads/saves categories, items, measures, formulas, and input cells as
datoms; derived values are not stored (they are recomputed by the engine).

### 4.4 Formula Language ↔ Engine

The formula compiler produces a typed, dimension-checked AST, lowered to a plan
(`Join` / `Aggregate` / `Map`) that the engine executes on differential
dataflow.

### 4.5 CNL ↔ Core Model

`nl_formula` parses controlled English into the same `Formula` AST and
pretty-prints a `Formula` back to English, resolving measure/category names
against the model.

---

## 5. Crate Layout

Cargo workspace, `crates/*`:

| Crate | Purpose | Phase |
|-------|---------|-------|
| `improv_core_model` | Categories, items, measures, coordinates, formulas, values. No GUI/storage deps. | 0 |
| `improv_storage_mentat` | Model ⇄ datoms on the embedded Mentat (SQLite) store. | 0 |
| `improv_engine` | Formula compiler (`Formula → TypedExpr → PlanNode`) + differential-dataflow evaluation. | 1 |
| `improv_cli` | The `improv` headless command-line tool. | 2 |
| `improv_tui` | VisiCalc-style terminal pivot viewer/editor. | 2 |
| `improv_server` | JSON HTTP API over a model store. | 3 |
| `improv_nl_formula` | Controlled-natural-language ⇄ formula translation. | 4 |

Shared dependencies are pinned in `[workspace.dependencies]`. The
`differential-dataflow` / `timely` pin is delicate — see
`AGENT_ENGINE_STEERING.md` §2 and do not bump it blindly.

---

## 6. Development Roadmap

Build strictly in phase order:

- **Phase 0 — Foundations:** `core_model` + `storage_mentat` + tests. **DONE.**
- **Phase 1 — Engine + formula compiler:** typed inference, plan
  (joins/aggregates), DD evaluation. DD-viability gate cleared. **DONE (numeric
  core).**
- **Phase 2 — CLI + TUI.** CLI **DONE (headless subset)**; TUI **DONE (viewer)**,
  live editing pending.
- **Phase 3 — Server.** **DONE** (JSON HTTP API; auth deferred).
- **Phase 4 — CNL formulas.** **DONE (initial grammar).**
- **Phase 5 — Desktop GUI.** Full Improv-style desktop app; toolkit to be chosen
  early in the phase. Reuses engine + storage unchanged. **PLANNED.**
- **Phase 6 — External-language functions.** `CALL(func, args...)` dispatching to
  external runtimes; pure, typed, dimension-declaring. Python first (Resolver One
  lineage), then R, Julia, and WASM. **PLANNED.**
- **Phase 7 — SQL database connectivity.** Import/export and `SQL("...")`
  live-query measures over external databases. **PLANNED.**

The `.agent/AGENT_STEERING.md` file holds the authoritative live status; this
roadmap is the plan, that file is the truth.

---

## 7. Scope of Later Phases (5–7)

Phases 5–7 are committed plan, not deferred vision. They are later than the v1
core (Phases 0–4) and must not destabilize it, so each carries a hard invariant:

- **Desktop GUI (Phase 5).** A view over the engine; introduces no new modeling
  semantics. Toolkit is chosen at the start of the phase. See
  `AGENT_GUI_STEERING.md` §9.
- **External-language functions (Phase 6).** Functions must be pure, return typed
  values, and declare dimensionality, so they behave as ordinary operators and
  keep the engine deterministic. See `AGENT_FORMULA_LANGUAGE.md` §11.
- **SQL connectivity (Phase 7).** External SQL is a data source/sink, not
  Improv's persistence (the canonical model stays in Mentat); live-query
  measures are explicitly marked so determinism tests treat pure paths as pure.
  See `AGENT_DATABASE_CONNECTIVITY.md`.

---

## 8. Definition of Success (v1)

The v1 milestone (Phases 0–4) succeeds when a user can:

- Define categories, items, and measures (input + formula)
- Build a multidimensional pivot view
- Enter data and edit formulas
- Get instant incremental recalculation on non-trivial models
- Save and reopen models across Linux, macOS, and Windows
- Rely on a deterministic, tested engine

---

## 9. Document Index

This master document is complemented by:

- `AGENT_GUI_STEERING.md` — interfaces (CLI/TUI/server) and the desktop GUI
- `AGENT_ENGINE_STEERING.md` — differential-dataflow engine
- `AGENT_STORAGE_STEERING.md` — Mentat datom storage
- `AGENT_FORMULA_LANGUAGE.md` — formula DSL + CNL
- `AGENT_DATABASE_CONNECTIVITY.md` — SQL connectivity (Phase 7)
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md` — testing + release gates
- `STEERING_SYSTEM_OVERVIEW.md` — unified summary

Together, these form the complete steering set for Improv.
