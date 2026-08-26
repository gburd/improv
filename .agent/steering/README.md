# Improv Steering System

This directory is the **detailed design steering set** for Improv — a
cross-platform, standalone multidimensional spreadsheet in Rust, inspired by
Lotus Improv and Quantrix Modeler.

These documents describe the architecture in depth. They complement, and are
pointed into by:

- **`/AGENTS.md`** (repo root) — the single top-level agent file: contributor
  workflow, git hygiene, the CI quality gate, and pointers into this directory.
- **`.agent/AGENT_STEERING.md`** — the *live* phase-status tracker (what is
  DONE / NEXT). The source design of record is `IMPROV.txt`.

> When a phase lands, update `.agent/AGENT_STEERING.md`. When a subsystem's
> design changes, update the relevant document here. Keep them consistent.

---

## Reading Order

1. **`STEERING_SYSTEM_OVERVIEW.md`** — start here; unified summary of the whole
   system.
2. **`AGENT_MASTER_STEERING.md`** — mission, architecture, principles, crate
   layout, roadmap.
3. The subsystem document relevant to your work (below).

---

## Document Index

| Document | Covers |
|----------|--------|
| `AGENT_MASTER_STEERING.md` | Mission, architecture, core principles, module relationships, crate layout, roadmap, v1 success criteria. |
| `AGENT_ENGINE_STEERING.md` | Differential-dataflow engine: the DD/timely version pin, coordinate-key encoding, formula compiler, dependency graph, broadcasting, aggregation, determinism. |
| `AGENT_STORAGE_STEERING.md` | Persistence on the embedded Mentat (SQLite) datom store: canonical schema, coordinate/formula serialization, save/load. |
| `AGENT_FORMULA_LANGUAGE.md` | Dimension-aware formula DSL (grammar, AST, types, broadcasting, aggregation) and controlled-natural-language translation. |
| `AGENT_GUI_STEERING.md` | Interfaces: CLI, TUI, and server (Phases 2–3); the desktop GUI (Phase 5). |
| `AGENT_DATABASE_CONNECTIVITY.md` | Optional SQL connectivity to external databases (Phase 7). |
| `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md` | Test strategy, the CI quality gate, and release qualification. |
| `STEERING_SYSTEM_OVERVIEW.md` | Unified summary of how the subsystems interact. |

---

## Phase Map (read before implementing)

The roadmap is a single sequence of phases; build in order.

- **Phases 0–4 (the v1 core):** core model, differential-dataflow engine, Mentat
  storage, formula DSL + CNL, and the CLI / TUI / server interfaces.
- **Phase 5:** desktop GUI (toolkit chosen at the start of the phase).
- **Phase 6:** external-language functions (Python first, then R/Julia/WASM).
- **Phase 7:** SQL database connectivity (import/export + live-query measures).

Phases 5–7 are committed plan built on the core, each with an invariant that
keeps the engine deterministic and Mentat as the system of record. See
`AGENT_MASTER_STEERING.md` §6–§7 for the authoritative roadmap.

---

## License

Part of the Improv project; dual-licensed Apache-2.0 OR MIT.
