# NeXTSTEP visual and interaction spec — 2026-10-03

This is the implementation contract for Batches B–D of
`2026-10-03-gui-ux-review-plan.md`. Direction: **identical to NeXTSTEP Lotus
Improv**; modern features only where required, styled to match.

References (in `refs/`):

- **W**: `improv-next-workspace-1.0.png`, 640×475, an Improv 1.0 workspace
  screenshot.
- **F**: `improv-next-worksheet-formulas.png`, 788×544, a worksheet window
  with its formula pane.
- **P**: `improv-pivot-gesture.jpg`.
- **N**: `improv-next-nested-headers.jpg`.

All captures are downscaled from a 1120×832 NeXT display, so geometry below
is in **1× NeXT pixels**: W values ×1.75, F values ×1.42. Where a
measurement is ambiguous, the stock NeXTSTEP AppKit value is used and marked
*(AppKit)*. Items marked **unverified** are guesses the references do not
show.

## 1. Palette (strict grayscale)

NeXTSTEP 2-bit gray has four levels. Sampled means, after scan and JPEG noise:

| Role | Sampled (W / F) | Spec |
| --- | --- | --- |
| Desktop / workspace background | 85 / — | `#555555` dark gray |
| Window and panel background, menu item face, tile item field | 172 / 173 | `#AAAAAA` light gray |
| Grid cells, formula rows, text fields | 242–254 / 231–233 | `#FFFFFF` white |
| Key-window title bar, menu title bar | 0–10 | `#000000` black, white text |
| Non-key window title bar | 85–104 | `#555555`, light-gray text |
| Category tile face | 86 (column tile) | `#555555`, white text |
| Tile being dragged, or selected | — | `#000000`, white text |
| Scroller trough | 127–152 | `#808080` |
| Scroller knob / buttons | 172 | `#AAAAAA` raised bevel |
| Cell selection fill | 170 vs 246 for unselected | `#AAAAAA` (never blue, never inverted) |
| Grid rules | 0–60 | `#000000`, 1 px |
| Group rules (between item groups) | dark, 2 px | `#000000`, 2 px |
| Bevel light / dark edges | 253 / 0–60 | `#FFFFFF` top-left, `#555555` + `#000000` bottom-right |
| Disabled text | — | `#555555` on `#AAAAAA` **(unverified)** |

The current `theme.rs` uses `#AAAAAA` / `#BEBEBE`, a blue selection and
rounded widgets. All of that is replaced. No color appears anywhere in the
default theme. Accessibility check: black on `#AAAAAA` is 9.0:1, black on
white is 21:1, and white on `#555555` is 7.5:1. All of these pass WCAG AA,
which closes D4 for the default theme.

## 2. Typography

NeXT used Helvetica 12 everywhere and Helvetica-Bold 12 for title and menu
titles *(AppKit)*.

**Bundle Arimo** (Regular + Bold) and register it as the primary
proportional family, ahead of egui's defaults, which stay as glyph fallback:

- It is metric-compatible with Arial, which is itself metric-compatible with
  Helvetica.
- It is OFL-1.1 (verified against google/fonts `ofl/arimo/METADATA.pb`,
  2026-10-03). OFL permits bundling in software of any license, and the
  binary already ships OFL and Ubuntu-Font-Licence fonts through egui's
  defaults (Noto Emoji, Ubuntu-Light), and `OFL-1.1` is already in
  `deny.toml`'s allow list. No new license class enters the project.
- Alternatives considered:
  - Liberation Sans 2.x: also OFL-1.1, derived from Arimo; equivalent.
  - Nimbus Sans (AGPL) and the TeX Gyre Helvetica clone (GUST): rejected.

Ship the TTFs under `crates/gui/assets/fonts/` with `OFL-Arimo.txt` and
load them with `include_bytes!` in `theme.rs`.

Sizes at 1×:

| Use | Font |
| --- | --- |
| Grid cells, headers, formula text | 12 Regular |
| Window titles, menu titles | 12 Bold |
| Menu items | 12 Regular |
| Tiles | 12 Regular |
| Info panel | 12 Regular |

## 3. Geometry (1× NeXT pixels)

| Element | Measured | Spec |
| --- | --- | --- |
| Title bar height | ≈21 (W 12 px) | 23 *(AppKit)* |
| Close box | ≈17 (W ≈10) | 15×15, right end of title bar, inset 4 |
| Menu item height | ≈20 (W 11.5, F 13) | 20 |
| Menu width | W 70 → ≈120 | content + 24, min 100 |
| Grid row height | ≈17 (W 9.7) | 17 |
| Cell padding | — | 4 left/right |
| Header band height (per column category) | ≈17 | 17 |
| Category tile | ≈17 high | 17 high, text + 12, min 64 wide |
| Scroller width | 18–24 (W 14) | 19 *(AppKit)* |
| Scroller arrows | both at the far end, grouped | 18×18 each |
| Formula pane row | ≈17 (F 12) | 17 per line; wraps |
| ✓ column | ≈22 (F 15) | 22 |
| Bevel | 1 px light + 1 px dark | 1 px each; 2 px on the outside of title bars and menus |
| Window border | 1 px black | 1 px black |
| Resize bar | NeXT bottom resize bar | 9 px high, split into thirds by 2 notches (AppKit) |

## 4. Components

### 4.1 Application menu

The menu is a vertical floating panel, top-left by default, and can be torn
off:

```
┌──────────────┐
│ Improv       │  black title, white bold text
├──────────────┤
│ Info        r│  #AAAAAA face, black text; `r` = submenu marker
│ File        r│
│ Edit        r│
│ Format      r│
│ Worksheet   r│
│ View        r│
│ Graph       r│
│ Print...   p │  key equivalent right-aligned
│ Services    r│
│ Hide       h │
│ Quit       q │
└──────────────┘
```

States:

- Idle: raised.
- Hover: no change. NeXT highlights only on press.
- Pressed / open submenu: white face.
- Disabled: `#555555` text.
- Submenus cascade to the right of their item, aligned to it.

Key equivalents display as the bare letter. Bindings: Ctrl on Linux and
Windows, Cmd on macOS (a modernization).

Submenu contents, all **unverified** beyond the top level:

| Menu | Items |
| --- | --- |
| Info | About Improv, Info Panel… (opens §4.7) |
| File | New, Open…, Save, Save As…, Import CSV…, Export CSV…, Close |
| Edit | Undo, Redo, Cut, Copy, Paste, Select All |
| Worksheet | New View, Add Category, Add Item, Add Formula, Calc |
| View | Model Browser, Formula Pane, (matrix commands) |
| Graph | New Graph |

### 4.2 Model browser window

This is a NeXT window ("Untitled2" in W, bottom-left). It holds a scrolling
list of views (`Name - Worksheet` with a small icon) above a description text
area. Double-click opens the view as a worksheet window. This replaces the
docked explorer, the tab strip (B4) and the "Save view" field. Categories,
items and measures move into a disclosure list within it, a modernization
that keeps the old explorer reachable.

### 4.3 Worksheet window

```
┌───────────── Share - Worksheet - Untitled2 ─────────────┬─┐ black title, close box
│[Region][All regions ]                       [Year     ]   │ pages TL; columns TR
│         ┌──────┬──────┬──────┐                       ▲    │
│ Galaxy  │1988  │1989  │1990  │                       │    │
│  Vending│ 10.0%│  8.6%│  8.7%│ grid (white, ruled)   │    │
│  …      │      │      │      │                       ▼    │
│[Product][Channel] ◀▶ ═════════════ h-scroller ════════    │ rows tiles BL
├──────────────────────────────────────────────────── Calc ┤
│ ✓ 1. Product=Units:Product/Units:All channels:…          │ formula pane
└──────────────────────────────────────────────────────────┘ resize bar
```

Title format: `<View> - Worksheet - <File>`.

### 4.4 Tile gutters

- **Rows tiles**: bottom-left, inline with the horizontal scroller, left of
  its arrows.
- **Column tiles**: top-right, stacked vertically, right-aligned.
- **Page tiles**: top-left, each followed by a white item field showing the
  current item. Clicking the field pops a list; Up and Down step through
  items (B3).

Tile states:

| State | Style |
| --- | --- |
| Idle | `#555555` face, white text |
| Selected / dragging | black face |
| Drag ghost | black tile following the pointer with a 1 px dotted white outline, as in P |
| Drop target | a 2 px black insertion bar where the tile will land, shown only while dragging (B2) |

Gutters size to their tiles. No empty wells.

### 4.5 Grid

- White cells, 1 px black rules, 2 px rules between item groups.
- Outer row categories render as one cell spanning their children,
  vertically centered (N).
- `Total` rows and columns are ordinary items. When a group summary exists,
  the stub reads `Total`.
- Numbers are right-aligned; text and booleans are left-aligned.
- The selected cell or range is filled `#AAAAAA`, and its row and column
  header items are filled `#AAAAAA` too (C5).
- Headers are plain text with no bevel. That already landed in A4; keep it.

### 4.6 Formula pane

The formula pane is attached below the grid, separated by a split bar with a
`Calc` button at its right end.

- Each line shows a ✓ toggle (enable/disable), a number `n.`, and the
  formula text, which wraps with a hanging indent.
- Errors show inline, underlined at the error position. That already exists
  through `formula_highlight`.
- **Add a formula** by clicking below the last line and typing
  `Name = expr`, then pressing Enter (B5). The "New derived measure" form and
  the operator palette are removed (B6).

### 4.7 Info panel

The Info panel is a NeXT panel window, opened from Info ▸ Info Panel… It
holds what the docked Inspector showed: name, kind, type, dimensions,
dependencies, and the formula in English. The id appears only here.

### 4.8 Scrollers

- NeXT style: an `#808080` trough with a raised `#AAAAAA` knob.
- Both arrow buttons sit together at the bottom or right end.
- The bottom-right corner holds the resize glyph ⇕ (P).
- egui's scroll bars are restyled through `Spacing.scroll` and visuals where
  possible. Grouped arrows need a custom scroller. Accept egui arrows-less
  bars first, then add arrows later (**deferred**, cosmetic).

## 5. Window model

**Decision: one native window acting as the NeXT workspace (`#555555`
desktop), with every Improv window drawn in-app as a NeXT-styled floating
`egui::Window` (custom frame, our own title bar).** Not native multi-viewport.

Reasons:

1. **The identical look requires drawing our own title bars.** Native
   viewports get the host window manager's decorations (GNOME, macOS,
   Windows), not black NeXT bars. Undecorated native windows lose window
   manager move and resize on Wayland.
2. **Wayland cannot position windows.** winit 0.30's `set_outer_position` is
   unsupported on Wayland, so a NeXT layout (menu top-left, browser
   bottom-left) cannot be reproduced there.
3. **Tile drag-and-drop across windows** stays inside one egui `Context` and
   one pointer stream. Across native viewports it is unreliable.
4. **The headless harness keeps working.** `Context::run` frames see every
   window. Native viewports would be embedded in tests anyway, which would
   test a different layout.

Consequence for existing code: the Step-3b child-`Ui` canvas existed because
an `egui::Window` cannot scroll inside a `ScrollArea`. The NeXT workspace
does not scroll; each worksheet window scrolls its own grid. So each
`Matrix` becomes one worksheet window, and `CanvasRect` / `MatrixPlacement`
persist window rects in workspace coordinates without a schema change.

The native window still gets OS decorations. Offer View ▸ Full Screen so the
workspace can fill the display.

## 6. Allowed modernizations

| Feature | Styling within the palette |
| --- | --- |
| Undo / redo | Edit menu items with key equivalents (already implemented) |
| Range selection (C3) | Drag or Shift+arrow; range filled `#AAAAAA`; status shows `Sum` in the window's bottom bar, left of the resize bar |
| Input vs derived (C2) | Derived cells keep white; input cells too. Distinction is a 1 px `#AAAAAA` inner underline on editable cells, plus hover tooltip "formula: …" on derived cells. **Unverified**: NeXT showed no distinction. Keep it subtle. |
| HiDPI | Scale every geometry value by `pixels_per_point`; never hard-code device pixels |
| Dark mode | Off by default; inverts the four levels (`#000`↔`#FFF`, `#555`↔`#AAA`) |
| Status feedback (D3) | Transient text in the worksheet window's bottom bar |
| Window title dirty marker (D1) | Native title `Improv — <file>` with `•` when dirty; NeXT title bar shows a broken close box (NeXT convention for unsaved) |
| Model tree icons (D2) | Text labels `input` / `formula` in `#555555` small caps, no glyphs |

## 7. Plan mapping and commit order

Each step is one atomic commit and keeps the tree green. ∥ marks steps that
can run in parallel because they touch disjoint code.

| Order | Commit | Plan items | Touches |
| --- | --- | --- | --- |
| 1 ∥ | Screenshot harness: script runs Xvfb at 1400×900, loads a fixed demo model, captures screenshots, renders side by side with refs | D5 | new `scripts/gui-shot.sh`, docs |
| 1 ∥ | Palette + bundled Arimo + square corners + gray selection | B7 (part), D4 | `theme.rs`, `main.rs`, assets |
| 1 ∥ | NeXT window chrome helper: title bar, close box, resize bar, key/non-key | B7 (part) | new `crates/gui/src/nxwin.rs` |
| 2 | Application menu replaces tool palette and top tab strip; actions dispatch to existing methods | C1, B4 (part) | new `menu.rs`, `app.rs` |
| 3 | Workspace: matrices become worksheet windows (uses `nxwin`); explorer → model browser window; inspector → Info panel | B1, B4 | `app.rs` |
| 4 | Formula pane inside worksheet window: ✓, numbering, Calc, type-to-add; delete operator palette and "new measure" form | B5, B6 | `app.rs` |
| 5 | Tile gutters to NeXT positions; page tile + item field; drop insertion bar | B2, B3, C4 | `app.rs` |
| 6 | Grid: selection fill + header highlight, range selection, input underline | C5, C3, C2 | `app.rs` |
| 7 | Window title / dirty marker, status feedback, browser labels | D1, D3, D2 | `app.rs`, `main.rs` |
| 8 | Scroller restyle | B7 (rest) | `theme.rs` |

Steps 3–7 all edit `app.rs` and must run serially. Each commit is checked
with the step-1 screenshot harness against the refs.

## 8. Gaps and unverified points

- Submenu contents (§4.1) are reconstructed from the Improv manual era, not
  from a screenshot.
- Disabled-state styling, menu pressed-state styling, and the exact
  drag-ghost pattern are inferred from AppKit conventions.
- The page item field's popup behavior is not visible in any reference.
- Scroller arrow glyphs and the resize glyph are not in Arimo. Draw them as
  egui shapes, not text.
- The input-vs-derived distinction (C2) has no NeXT precedent. It is a
  modernization and is kept deliberately faint.
- Exact title-bar and close-box metrics come from downscaled captures. The
  AppKit values were chosen where measurement and AppKit differ.
- A runnable Improv 3.0 remains the only way to close pixel fidelity. Until
  then, steering must not claim a pixel match.
