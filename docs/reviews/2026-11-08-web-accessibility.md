# Web GUI accessibility: the route (0.15)

2026-11-08, against `main` at f914e51 (egui/eframe 0.37.0, AccessKit
0.25.2). The web GUI (`crates/gui_web`) draws into one WebGL canvas, so a
screen reader sees one canvas. The goal is a GUI that works with a
screen reader and the keyboard.

## Decision

**No egui/eframe bump.** No version passes the AccessKit tree to the
browser, and no AccessKit web adapter exists. **gui_web bridges it
itself.** It turns the tree that egui already builds into hidden DOM
inside the `<canvas>`, with ARIA roles, names and states. Focus and
actions go back to egui as AccessKit requests. The grid is marked up as
an ARIA grid in `app.rs`. This is the "hidden DOM mirror" fallback of
the I1 review, built from egui's own tree instead of by hand.

## Evidence: there is no upstream route

- **eframe 0.37.0** is the newest release on crates.io (published
  2026-10-15). Its web runner still drops the update:
  `crates/eframe/src/web/app_runner.rs:403`,
  `accesskit_update: _, // not currently implemented`.
- **egui `main`** (91b5712, 2026-11-06) has the same line. The last
  change to that file is 394f0ec (2026-10-06). An issue search on
  emilk/egui for AccessKit and the web finds no open PR or plan.
- **AccessKit** (0.25.2, 2026-10-25; repo `main` at 9095a7a) has
  adapters for Android, AT-SPI, iOS, macOS, Unix, Windows and winit, and
  none for the web. crates.io has no `accesskit_web` or `accesskit_wasm`.
  A search of its issue tracker finds no web adapter work.
- eframe's `web_screen_reader` feature only speaks focus changes through
  the Speech Synthesis API. It is not exposed to assistive technology,
  and a screen-reader user would hear two voices.

## How the bridge works

1. **The tree.** The app calls `Context::enable_accesskit()`, so egui
   builds the same tree the desktop gives its platform adapter. The
   desktop's H2 names come along for free: the shared `menu` and `nxwin`
   code names its menu rows, window titles and close boxes. An egui
   `Plugin`'s `output_hook` sees each pass's `FullOutput` before eframe
   drops it, and takes `accesskit_update` out.
2. **The DOM.** A pure model (`a11y.rs`, unit tested natively) maps each
   AccessKit node to an element: its ARIA role, `aria-label`, states
   (disabled, read-only, checked/pressed, selected, expanded, live),
   grid indices and counts, and text for static labels. It diffs each
   frame against the last one and emits create/attribute/children/remove
   operations. A small wasm executor applies them to elements inside the
   `<canvas>`. Canvas fallback content is in the accessibility tree and
   never drawn, so the canvas pixels cannot change. The screenshot
   golden checks that.
3. **Focus.** eframe reads keys only while the canvas (or its hidden
   text input, the "text agent") has DOM focus, so DOM focus stays
   there. The canvas gets `role=application`, and its
   `aria-activedescendant` names the element of egui's focused widget.
   Screen-reader focus therefore follows egui's: Tab, the arrows and
   egui's own focus moves are announced. When a text field has focus,
   eframe focuses the text agent instead, and the bridge gives that input
   the field's name.
4. **Actions.** A click on a mirror element is what a screen reader's
   "activate" sends, and it becomes an AccessKit `Click` request. Moving
   the screen reader's focus onto an element becomes a `Focus` request,
   and DOM focus goes back to the canvas. The plugin's `input_hook` puts
   both in the next frame's input. egui handles them as it does on the
   desktop: a click on a button or cell, focus on a widget.
5. **The grid.** Each worksheet's grid gets `Grid`, `Row`, `ColumnHeader`,
   `RowHeader` and `GridCell` nodes, with row and column counts and
   indices. A cell is named "2025Q1, Widget: 100", as on the desktop, so
   every screen reader says its headers. A derived cell is read-only.
   The grid is one Tab stop, its cursor cell. The arrows move the cursor,
   Enter or typing opens the editor, Enter commits and Esc cancels, and
   focus then goes back to the cell. The cursor is outlined while it has
   focus.

Why a bridge rather than a hand-built mirror: one generic translation
covers every widget egui names (menus, tiles, formula lines, text
fields, buttons, the Model Browser, the prompts). It uses egui's own
focus and click handling, so the keyboard and the screen reader cannot
disagree. A hand-built grid mirror would still need the same grid
structure, and it would leave the rest of the GUI unnamed.

## Known limits

- **Field text can't be reviewed.** The text agent holds only IME text,
  so while typing a screen reader echoes keys, but it cannot read back a
  field's whole content.
- **No bounding boxes.** Canvas fallback content has no layout, so a
  magnifier or VoiceOver's focus box shows the whole canvas.
- **Not run with a real screen reader** (no desktop session here). The
  checks read Chromium's accessibility tree over the DevTools protocol.
  Orca, NVDA and VoiceOver runs are still open, as for the desktop (H2).
- **Drag-only pivots.** A tile's "Move to" menu opens with Enter as well
  as a right-click.
- When eframe's web runner gains AccessKit, the bridge goes away and the
  grid markup stays.

## Plan (one green commit each)

1. This note.
2. `a11y.rs`: the AccessKit-to-DOM model and diff, native unit tests.
3. The wasm executor, the plugin, focus and actions; names for the web
   GUI's own widgets (tiles, formula fields, Model Browser buttons, the
   status line as a live region).
4. The grid: ARIA grid structure, the keyboard cursor, Enter to edit.
5. The e2e check (`Accessibility.getFullAXTree`) in `drive.py`, the
   golden unchanged. Then the docs and the CHANGELOG.
