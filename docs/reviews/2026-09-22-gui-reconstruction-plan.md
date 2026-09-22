# GUI reconstruction plan — reference-backed

Date: 2026-09-22. Supersedes the "reference gate" placeholder in
`.agent/steering/AGENT_GUI_STEERING.md`.

## Provenance of this plan

Derived from material supplied by the maintainer, not from memory:

| Source | What it establishes |
| --- | --- |
| `~/Downloads/improv/Lotus Improv 1.0.png` | NeXTSTEP Improv: worksheet windows, corner-stacked category tiles, nested row groups with `All …` summary rows, numbered formula list |
| `~/Downloads/improv/improv-categories.jpg` | **The pivot gesture**: tiles docked in bottom-left and top-right margin gutters; a tile dragged between gutters re-pivots; after the drop the tiles stack vertically in the corner and the moved category becomes grouped column headers |
| `~/Downloads/improv/page36_1.jpg` | Quantrix/NeXTSTEP: tiles above the grid's left edge, deep row nesting with `Total` subtotals + outline collapse, grouped column headers, numbered formula list with per-formula checkmarks, a separate model/worksheet navigator window, a function-browser palette |
| `~/Downloads/improv/improv_next.webp` | Same structure localized; charts in their own window with an inspector |
| `~/Pictures/Screenshots/*.png` (17, Quantrix online demo) | Modern descendant: free-form **canvas** holding several matrices plus rich text, document tabs, category tiles as `⁝ Name ▾` chips at a matrix's bottom-left, a formula bar with operator palette above a numbered checked formula list, dotted `Matrix.Measure` qualification, `'quoted names with spaces'`, selection-aggregate readout |
| `~/Downloads/Quantrix-Modeler-Product-Brochure-1.pdf` | Named UI differentiators: always-on pivot engine, "modifications in-situ from any slice", **free-form canvas dashboarding** ("not made to fit a pre-made layout"), natural-language formulas, visual dependency inspector |

Chat/Qloud/Groovy/scripting are explicitly out of scope per the maintainer.

## What the references overturn in our current GUI

Our `update()` stacks six fixed panels around one grid: a top formula bar, a
left tool palette, a left explorer, a right inspector, a right chart panel, a
bottom definitions panel, and a central grid. Every reference contradicts that
shape in three specific ways.

1. **Category tiles belong in the grid's own margin gutters, not a shelf.**
   `improv-categories.jpg` is decisive: `Country`/`Travel` sit in the
   bottom-left gutter inline with the horizontal scrollbar, `Hours` in the
   top-right, and the pivot *is* dragging a tile from one gutter to the other.
   Our `margin_tiles` renders Columns/Rows/Pages as three drop zones in one
   horizontal row above the table — the axes read left-to-right instead of
   being anchored to the edges they control. This is a known open defect.
2. **A model is a canvas of matrices, not one grid.** Improv shows multiple
   worksheet windows; Quantrix shows several matrices plus explanatory text
   freely placed on one scrollable canvas, with document tabs for alternate
   views. We render exactly one measure at a time. The brochure calls
   free-form canvas layout a differentiator, and the audit already logged
   "no simultaneous multi-measure display" as an open capability gap.
3. **Formulas are a numbered, individually-toggleable list, not one line.**
   Every reference shows a persistent list (`✓ 1. …`, `✓ 2. …`) with checkmarks
   and, in Quantrix, an operator palette above it and author attribution
   beside it. Ours is a single-line bar for the selected measure only, so a
   model's logic is never visible as a whole.

The references also validate decisions we already made: natural-language
formulas (Quantrix's `Sum of Revenue`), the always-on pivot, and in-situ
structural edits are all core to the lineage, not embellishments.

## Sequenced plan

Ordered so each step is independently shippable and testable. Steps 1–2 are
layout truth; 3–4 are capability; 5 is polish.

### Step 1 — Margin gutters and real edge-docked tiles

Replace the three-zone shelf with gutters that frame the table: a **top gutter**
holding column-axis tiles, a **left gutter** holding row-axis tiles, and a
**bottom-left corner well** holding unplaced/page categories (Improv's
`Country`/`Travel` position). A tile is dragged between gutters; dropping into a
gutter appends to that axis (preserving the stacking we already support).
Tiles keep the `⁝ Name ▾` affordance from Quantrix: a grip to drag, a menu for
per-category actions (filter, sort, collapse).

Acceptance: a headless layout test asserting the row gutter's rect adjoins the
table's left edge and the column gutter's rect adjoins its top edge — the
geometric check the current shelf fails.

### Step 2 — Formula list

A dockable formula pane listing every formula in the model, numbered, each with
an enable checkbox, its target measure, and inline error state. Keep the
one-line bar as the editor for the selected row. `Matrix.Measure` qualification
and quoted names are deferred to Step 4.

Acceptance: a model with several derived measures shows all of them; toggling
one off recomputes dependents; clicking a row selects that measure.

### Step 3 — Multiple matrices on a canvas

Generalize the central area from "one grid" to "N positioned matrices". A
`View` gains a list of placed matrices (position, size, measure, axis layout)
rather than a single `measure`. `View` is persisted, so this is a schema change
— add fields with serde defaults exactly as `n_rows`/`n_cols` were added, so
existing saved views keep loading.

Acceptance: two matrices visible simultaneously, each independently pivotable;
an edit in one recomputes a dependent measure shown in the other; save/reload
round-trips placement.

### Step 4 — Nested headers with group summaries, and qualified names

Row/column groups render Improv's `All …`/`Total` summary lines with an outline
collapse control in the gutter. Add a quoted-identifier form to the formula
grammar (`'Unit Price'`) plus `Matrix.Measure` qualification — this closes the
open defect where a CSV-derived measure name like `Unit Price` is unspellable in
both surface languages and therefore read-only.

### Step 5 — Chrome and theme

Keep the NeXTSTEP-derived palette. Revisit only what the references show
concretely: per-matrix title bars, the selection-aggregate readout in the status
bar (Quantrix shows `Sum`), and a function browser reachable from the formula
pane.

## Honest labeling

Steps 1, 2 and 4's summary rows are **Improv-derived** (from the NeXTSTEP
screenshots). Step 3's canvas, the tile chrome, and Step 5's status readout are
**Quantrix-derived** — the modern descendant, which has drifted from Improv 3.0
in toolkit and convention. Docs must attribute accordingly and must not claim
pixel fidelity to NeXTSTEP Improv on the strength of Quantrix screenshots. A
runnable Improv 3.0 or its manual remains the only way to close that gap.

## Not in this plan

Chat, Qloud, Groovy/scripting, permissions, DataLink/DataPush connectors,
Tableau export, and the YouTube walkthrough's video-only details (unwatchable
here — no video capability). Undo/redo, typed editing, derived export and focus
handling are correctness items tracked in the audit, not layout work, and are
being fixed independently.
