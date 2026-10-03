# GUI UX review and plan — 2026-10-03

Source: running `improv-gui` under Xvfb (800×600 default and 1400×900) on a
Time × Product × Scenario demo model, compared against
`refs/improv-pivot-gesture.jpg`. Each item names the defect and the fix.
Status is updated in place as items land.

## Batch A — broken now (cheap)

| # | Item | Status |
| --- | --- | --- |
| A1 | Tool-palette glyphs (`↶ ↷` etc.) render as tofu `□`: egui's default font lacks them. Bundle an icon font or use glyphs the default font has; add a test that every palette label is covered by the loaded fonts. | in progress |
| A2 | Default 800×600 window: Inspector overlaps the matrix and gutter tiles draw over its text. Start at ~1280×800, set a minimum size, keep the canvas's space before side panels. | in progress |
| A3 | Numeric cells are left-aligned; Improv right-aligns numbers so digits line up. | in progress |
| A4 | Item headers (`Q1`, `Widget`) are beveled button chips; Improv draws them as plain text in a ruled band — only category tiles look draggable. | in progress |
| A5 | CLI `set --at` rejects repeated `--at` / space-separated pairs (only one comma-joined arg works). | in progress |

## Batch B — layout and hierarchy

| # | Item | Status |
| --- | --- | --- |
| B1 | Canvas gets a small fraction of the window. Model and Inspector become collapsible drawers; Inspector trivia (id/kind/type) moves to tooltip/status bar; matrix fills its canvas. | planned |
| B2 | Empty gutter wells: row gutter is a tall grey box for one tile; top-right gutter mostly empty. Size gutters to their tiles; highlight as drop zones only during a drag. | planned |
| B3 | Pages has two controls (tile + separate `< Plan [1/2] >` stepper). Merge into the tile (`Scenario: Plan ▾`). | planned |
| B4 | Top bar leads with an empty-state message. Show document tabs only when views exist; move `+ Canvas`/`+ Matrix` to a View menu. | planned |
| B5 | Third formula entry point ("New derived measure" form). Add formulas by typing `X = …` on a new formula-list line; drop the form. | planned |
| B6 | Operator palette (`= + - * / ( ) [ ]`) wastes space. Remove, or show only while a formula field has focus. | planned |

## Batch C — interaction basics (closes GUI steering gate #4)

| # | Item | Status |
| --- | --- | --- |
| C1 | No menu bar. Add File/Edit/View/Model menus with shortcut hints (New/Open/Save As/Import/Export/Undo/Redo). | planned |
| C2 | Input vs derived cells look identical. Tint editable cells; lock affordance on derived hover. | planned |
| C3 | No range selection; `Sum` readout is one cell. Drag / Shift+arrow ranges. | planned |
| C4 | Verify tile-drag ghost + drop-zone highlight and nested group-header result match the reference (needs screenshot/recording, not just a headless test). | planned |
| C5 | Selection is a hard blue box on the value. Thin focus ring plus highlighted matching row/column headers. | planned |

## Batch D — polish and qualification

| # | Item | Status |
| --- | --- | --- |
| D1 | Window title shows document name and a dirty marker. | planned |
| D2 | Model tree `·`/`=` prefixes are cryptic; use labelled icons/tooltips. | planned |
| D3 | Status-bar feedback on recalc/save. | planned |
| D4 | Contrast: grey-on-grey text (`Filters`, `(no saved views)`) fails WCAG AA 4.5:1; add a high-contrast/dark variant. | planned |
| D5 | Fixed-geometry screenshot regression (Xvfb + `xwd`, scripted) — GUI steering gate #5 — so overlaps like A2 are caught. | planned |
