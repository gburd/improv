# AGENT_GUI_STEERING.md
Interfaces Steering Document for Improv (CLI, TUI, Server, and desktop GUI)

## 1. Purpose

This document defines the user-facing interfaces of Improv. The **early
iterations are terminal- and service-oriented — CLI, TUI, and server** — with a
desktop GUI landing in Phase 5. All interfaces are thin: they drive the shared
`core_model` + `engine` + `storage_mentat` stack and never embed business logic.

> **Scope note.** The CLI, TUI, and server (§3–§5) are Phases 2–3. The desktop
> GUI (§9) is Phase 5; its toolkit is chosen at the start of that phase — no
> framework is mandated by the source design, so do not assume Dioxus, egui, or
> any other.

---

## 2. Shared Interface Architecture

Every interface follows the same shape:

1. Open a Mentat-backed store and `load_model()`.
2. Build the engine and `evaluate(model, targets)` for the requested measures.
3. Render a view slice (grid / JSON / text).
4. Apply edits as engine deltas and persist via Mentat transactions.

Interfaces depend on `core_model`, `engine`, `storage_mentat`, and (where they
accept English) `nl_formula`. Logic lives in those crates, not in the UI.

---

## 3. CLI (`improv_cli`) — Phase 2, DONE (headless subset)

The `improv` binary is a headless model runner and import/export tool over a
Mentat-backed store.

### 3.1 Commands

- `init <db>` — create a new model store
- `add-category <db> <id> <name>`
- `add-item <db> <id> <category-id> <name>`
- `add-measure <db> <id> <name> <value-type> <kind>`
- `set <db> <measure-id> <value> --at <Category=Item ...>`
- `list <db>` — list model structure
- `show <db> <measure-id>` — show a measure's values
- `export <db>` — export a computed view

### 3.2 Design Notes

- Deterministic, scriptable output (suitable for batch processing and tests).
- All mutations are Mentat transactions (durable, atomic).
- The CLI is the reference driver for the model API; new engine capabilities
  should be exercisable here first.

---

## 4. TUI (`improv_tui`) — Phase 2, DONE (viewer)

A VisiCalc-style terminal interface built on **ratatui** + **crossterm**.

### 4.1 Current Capabilities

- Renders a measure as a 2-D pivot grid (first two categories on rows/columns).
- **Pivoting**: `p` rotates which categories sit on rows/columns/pages (the
  Improv/Quantrix "move a category to another axis" gesture, no formula edits).
- Keyboard navigation (arrows), **mouse** (click a cell to select it; the
  terminal must support mouse reporting, which the TUI enables), measure
  cycling (`Tab`/`m`), and paging extra dimensions with `[` / `]`.
- **Live cell editing** of input measures (`e`/Enter to edit, Enter commits,
  Esc cancels; `q` quits): commits push through the engine's live incremental
  edit API so derived measures recompute from the delta, and re-render.
  Derived cells are read-only. Autosaves to Mentat on quit.
- A key-hint footer and panic-safe terminal teardown (restores on error).

### 4.2 Layout

- Grid view bound to a `View`
- (Planned) formula editor pane and model explorer

### 4.3 Next Increment

Re-pivot (drag categories between axes) and save-on-every-commit; a formula
editor pane. The incremental-edit dependency is satisfied (`session::Engine`).

---

## 5. Server (`improv_server`) — Phase 3, DONE

A JSON HTTP API built on **axum** + **tokio**, over a model store.

### 5.1 Endpoints

- `GET  /health`
- `GET  /model`
- `GET  /measures`
- `GET  /measures/:id/values`
- `POST /measures/:id/eval` — apply input-cell edits through the live engine and
  return the recomputed snapshot (what-if by default; `"persist": true` writes
  through)
- `POST /measures/:id/cells` — set a cell
- `POST /nl/parse` — CNL → formula
- `POST /nl/describe` — formula → CNL

### 5.2 Design Notes

- Stateless request handlers over a shared model store.
- Authentication/authorization is **deferred** (documented as not-yet-present).
- The server reuses the same evaluate/persist path as the other interfaces.

---

## 6. Interaction Model (common)

### 6.1 Editing Measures

Select a measure → edit its formula (typed DSL or CNL) → engine recalculates
incrementally → the view updates.

### 6.2 Editing Data / Categories

Set input cells or add/remove items → dependent measures recompute → the view
updates.

### 6.3 Pivoting

Assign categories to rows / columns / pages / filters → the engine materializes
a new slice → the interface renders it. This is the signature Improv/Quantrix
interaction: the same measures are re-projected without rewriting any formula.

### 6.4 Quantrix / Lotus Improv feature parity

Improv aims to match the modeling hallmarks of Lotus Improv and Quantrix
Modeler. Parity status (drives the plan):

- Named categories/items/measures; formulas over names, not cells. **Done.**
- Multidimensional cube; automatic broadcasting; dimension-aware aggregation.
  **Done.**
- Live incremental recalculation. **Done.**
- Scenarios as an ordinary category (what-if by adding/switching an item).
  **Supported** via the category model.
- **Pivoting — drag/reassign categories between rows / columns / pages /
  filters, without rewriting formulas.** The defining interaction. *In
  progress*: axis reassignment (move a category to rows/cols/pages) is being
  added to the GUI (drag-and-drop) and TUI (keyboard + mouse). Filters (restrict
  a dimension to a subset) follow.
- **Multiple saved views** per model — a named layout (which measure, category
  placement, sort/filter). *Planned* (a `View` on the model, persisted as
  datoms; see IMPROV.txt "View layer").
- Charts on a view. *Planned*, lower priority.

### 6.5 NeXTSTEP look-and-feel (GUI)

The desktop GUI targets the look-and-feel of the original NeXTSTEP Lotus Improv:

- **NeXTSTEP theme** (`gui::theme::next_style`) — light neutral gray desktop,
  chiseled/beveled controls (raised light faces, dark bevel edges, sunken
  pressed faces), squared corners, muted blue selection, paper-white grid.
- **On-grid margin category tiles** — the signature pivot gesture: category
  *tiles at the grid margins* (top = Columns, left = Rows, plus a Pages strip);
  each tile is a drag source, each margin a drop zone. Drag a tile between
  margins to re-pivot, no formula rewrite. (Routes through `set_axis`.)
- **Top formula bar** — the selected measure's formula on one line
  (`<Measure> = <expr>`, Enter/Commit), Improv-style.
- **Chiseled grid headers** with a top-left corner stub showing the current
  `Rows \ Columns` category names.
- **NeXT-style tool palette** — a narrow left column of beveled buttons
  (pivot / chart / save view / save model).
- **Multi-category-per-axis stacking** — more than one category can be stacked
  on the row or column axis (`n_rows`/`n_cols` split of `axis_order`); the grid
  renders the Cartesian product with nested column headers and group-outlined
  row stubs. Dragging a category onto an axis margin stacks it; dragging to
  Pages unstacks. Persisted in `View` (`n_rows`/`n_cols`, serde-default 1).

Both the GUI (`improv-gui`) and the TUI (`improv-tui`) support pivoting; the TUI
additionally accepts **mouse input** (click to select/pivot) where the terminal
supports it, in addition to keyboard control.

---

## 7. Error Handling

- **Formula errors** — surfaced inline (TUI/GUI) or in the response
  (CLI/server), with the offending measure identified.
- **Engine errors** (type / dimension / cycle) — propagate as values and are
  reported per measure.
- **Storage errors** — reported at load/save boundaries.

---

## 8. Persistence

All interfaces persist through `improv_storage_mentat` (embedded SQLite Mentat).
There is no separate file format; the model *is* the datom store. Interfaces may
additionally **export** a computed view (e.g. CSV) as a read-only projection —
see `AGENT_STORAGE_STEERING.md` §8.

---

## 9. Desktop GUI — Phase 5

The desktop GUI lands in Phase 5, after the CLI/TUI/server milestones. It reuses
the engine and Mentat storage unchanged.

### 9.1 Toolkit Selection

Choosing the toolkit is the first task of the phase. It must balance performance
(large-grid rendering), native feel across Linux/macOS/Windows, and
Rust-ecosystem maturity. No framework is mandated by the source design; select
one deliberately and record the decision here.

**Decision: `egui` / `eframe`.** Rationale:

- **Data-grid fit.** egui is immediate-mode: the whole UI redraws each frame,
  which maps cleanly onto the engine's live recalculation (edit → snapshot →
  redraw). `egui_extras::TableBuilder` handles large, virtualized grids.
- **Maturity + portability.** eframe is the most mature pure-Rust GUI, ships on
  Linux/macOS/Windows (and web) from one codebase, and is actively maintained.
- **Keyboard-centric.** egui has first-class keyboard focus/navigation, matching
  the §9.3 constraint.
- It is the first candidate the source design names (IMPROV.txt: "egui, iced,
  druid, or similar").

Pinned at `eframe`/`egui`/`egui_extras` `0.36` in `[workspace.dependencies]`.
The GUI crate (`improv_gui`, binary `improv-gui`) is a thin view over
`improv_engine` (live `session::Engine`) and `improv_storage_mentat`; it adds no
modeling semantics.

### 9.2 Surface

**Implemented** (`improv-gui <db>`, egui/eframe):

- Model explorer (left): categories (expandable to items) and measures
  (input vs derived), click to select.
- Pivot/grid view (center): an **axis shelf** (Rows / Columns / Pages drop
  zones) above the grid — **drag category chips between axes** (egui
  drag-and-drop) or use per-chip `→` / a Pivot button to re-pivot live, plus
  per-page-dimension `< >` selectors. The Improv/Quantrix pivot, no formula
  edits. Input cells are editable (click → text field → `set_cell` through the
  live engine → incremental recompute + autosave); derived cells are read-only,
  rendered via `CellValue` (bool/text/`#ERR`).
- Formula editor (bottom): edit a derived measure's formula, or add a new
  derived measure from formula text; both parse via `core_model::parser` and
  rebuild the live engine.
- Inspector (right): id, name, kind, value type, dimensions, derived
  dependencies, formula-in-English, and an error-cell count.

**Not yet:** keyboard grid-cursor navigation, filters (restrict a dimension to
a subset), multiple saved views, scenario management, efficient virtualized
rendering for very large grids.

### 9.3 Constraints

- Keyboard-centric, cross-platform desktop UX.
- The GUI is a *view* over the engine; it introduces no new modeling semantics.
- Web/mobile targets remain out of scope until the desktop GUI ships.

---

## 10. Accessibility, i18n, Theming (GUI phase)

Goals for the GUI phase: keyboard navigation, high-contrast mode, UTF-8
throughout, locale-aware formatting, translatable strings, and light/dark
themes.

---

## 11. Definition of Success

The interfaces succeed when: the CLI scripts a full model lifecycle
deterministically; the TUI renders and (next) edits pivots fluidly; the server
exposes the model over a clean JSON API; and — later — the GUI presents the same
model with a polished, responsive desktop experience.

---

## 12. Document Index

Part of the full steering set:

- `AGENT_MASTER_STEERING.md`
- `AGENT_GUI_STEERING.md`
- `AGENT_ENGINE_STEERING.md`
- `AGENT_STORAGE_STEERING.md`
- `AGENT_FORMULA_LANGUAGE.md`
- `AGENT_DATABASE_CONNECTIVITY.md`
- `AGENT_TESTING_AND_RELEASE_QUALIFICATION.md`
- `STEERING_SYSTEM_OVERVIEW.md`
