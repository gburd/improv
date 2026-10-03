# GUI UX review and plan — 2026-10-03

Source: running `improv-gui` under Xvfb (800×600 default and 1400×900) on a
Time × Product × Scenario demo model, compared against
`refs/improv-pivot-gesture.jpg`. Each item names the defect and the fix.
Status is updated in place as items land.

## Batch A — broken now (cheap)

| # | Item | Status |
| --- | --- | --- |
| A1 | Tool-palette glyphs (`↶ ↷` etc.) render as tofu `□`: egui's default font lacks them. Bundle an icon font or use glyphs the default font has; add a test that every palette label is covered by the loaded fonts. | done |
| A2 | Default 800×600 window: Inspector overlaps the matrix and gutter tiles draw over its text. Start at ~1280×800, set a minimum size, keep the canvas's space before side panels. | done: root cause was nested gutter panels resetting the canvas clip; each gutter now narrows its clip to the canvas, 1280×800 initial / 960×600 min |
| A3 | Numeric cells are left-aligned; Improv right-aligns numbers so digits line up. | done |
| A4 | Item headers (`Q1`, `Widget`) are beveled button chips; Improv draws them as plain text in a ruled band — only category tiles look draggable. | done |
| A5 | CLI `set --at` rejects repeated `--at` / space-separated pairs (only one comma-joined arg works). | done |

## Direction (maintainer, 2026-10-03)

**The GUI must look identical to NeXTSTEP Lotus Improv**, with modern
features added only where required (and styled to match). References:
`refs/improv-next-workspace-1.0.png`, `refs/improv-next-worksheet-formulas.png`,
`refs/improv-pivot-gesture.jpg`, `refs/improv-next-nested-headers.jpg`.
Observed NeXTSTEP traits that Batches B–D must reproduce:

- Workspace of separate windows: a floating vertical **application menu**
  (Info, File, Edit, Format, Worksheet, View, Graph, Print, Services, Hide,
  Quit; submenus marked `r`), a **model browser** window listing views, and
  one or more **worksheet windows**.
- Worksheet window: black title bar, centered white `View - Worksheet - File`
  text, close box. Formula pane attached at the bottom of the same window:
  ✓ column, numbered formulas, `Calc` button. No operator palette, no
  "new measure" form.
- Tiles are dark-gray with white text: rows tiles bottom-left beside the
  horizontal scroller; column tiles stacked top-right; page tiles top-left
  with an item field.
- Grid: white cells, thin black rules, right-aligned numbers, nested group
  cells spanning their children, `Total` group rows; strict grayscale palette
  (black / dark gray / light gray / white), Helvetica-like font; selection is
  dark/inverted, never blue. NeXT scrollers (arrows grouped at one end).

Items below are reworded where this direction supersedes them.

## Batch B — layout and hierarchy

| # | Item | Status |
| --- | --- | --- |
| B1 | Canvas gets a small fraction of the window. Replace the docked Model/Inspector panels with NeXTSTEP-style separate windows (model browser; Inspector as an Info panel opened from the menu); the worksheet fills its window. | planned |
| B2 | Empty gutter wells: row gutter is a tall grey box for one tile; top-right gutter mostly empty. Size gutters to their tiles; highlight as drop zones only during a drag. | planned |
| B3 | Pages has two controls (tile + separate `< Plan [1/2] >` stepper). Merge into the tile (`Scenario: Plan ▾`). | planned |
| B4 | Top bar leads with an empty-state message. Remove the tab strip; views are listed in the model browser and open as worksheet windows; `+ Canvas`/`+ Matrix` move to the Worksheet/View menu. | planned |
| B5 | Third formula entry point ("New derived measure" form). Add formulas by typing `X = …` on a new formula-list line; drop the form. | planned |
| B6 | Operator palette (`= + - * / ( ) [ ]`) is not in Improv. Remove it; formula pane gets the ✓ column and a `Calc` button. | planned |
| B7 | NeXTSTEP chrome: grayscale palette (no blue), Helvetica-like bundled font (license-checked), black title bars, dark tiles with white text, NeXT scrollers, inverted selection. Drop the left tool palette (actions live in the menu). | planned |

## Batch C — interaction basics (closes GUI steering gate #4)

| # | Item | Status |
| --- | --- | --- |
| C1 | No menus. Add the NeXTSTEP vertical application menu (Info/File/Edit/Format/Worksheet/View/Graph/Print/Services/Hide/Quit) with cascading submenus and key equivalents. | planned |
| C2 | Input vs derived cells look identical (modernization). Keep within the grayscale palette, e.g. derived cells in a light-gray band or italic; lock hint on hover. | planned |
| C3 | No range selection; `Sum` readout is one cell. Drag / Shift+arrow ranges. | planned |
| C4 | Verify tile-drag ghost + drop-zone highlight and nested group-header result match the reference (needs screenshot/recording, not just a headless test). | planned |
| C5 | Selection is a hard blue box on the value. Thin focus ring plus highlighted matching row/column headers. | planned |

## Batch D — polish and qualification

| # | Item | Status |
| --- | --- | --- |
| D1 | Window title shows document name and a dirty marker. | planned |
| D2 | Model tree `·`/`=` prefixes are cryptic; use labelled icons/tooltips. | planned |
| D3 | Status-bar feedback on recalc/save. | planned |
| D4 | Contrast: grey-on-grey text fails WCAG AA 4.5:1. The NeXT palette is high-contrast black-on-white; matching it fixes most of this. Optional dark variant is a modernization, off by default. | planned |
| D5 | Fixed-geometry screenshot regression (Xvfb + `xwd`, scripted) compared side by side with the refs — GUI steering gate #5. | planned |

Note: B1/B4/C1 imply multiple OS windows. egui 0.29 supports this via
`show_viewport_immediate`; where a platform can't, fall back to in-app
NeXT-styled floating windows. A design spec (`docs/reviews/2026-10-03-nextstep-visual-spec.md`)
precedes implementation.
