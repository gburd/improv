# The improv-cosmic redesign: one family with Hylki

2026-11-09, against `main` at dc24984 (libcosmic 95236a9). The design
half of a redesign of `crates/gui_cosmic`. No code changes here. It says
what the COSMIC GUI should become, how its parts map onto libcosmic, and
the stages that get it there.

## Contents

- (a) Goal, family resemblance, and the clean-room process
- (b) Pattern map
- (c) Wireframes
- (d) libcosmic widgets
- (e) Keyboard
- (f) Theming
- (g) Accessibility
- (h) What stays from today's improv-cosmic
- (i) Staged plan
- (j) Open questions

## (a) Goal, family resemblance, and the clean-room process

### Goal

Someone who runs [Hylki](https://hylki.hyprlab.co) (a GNOME email client
in Rust) and improv-cosmic side by side should feel they are two
purpose-built applications from one family: one for mail, one for
multidimensional models. They share a window structure, an interaction
model, a level of finish, and a way of being kind to the keyboard and
to people who make mistakes.

What does *not* change is Improv:

- a model is categories, items and measures;
- formulas live apart from cells and are written over categories;
- category tiles are dragged to rows, columns and pages to pivot;
- one model has several views;
- the authority computes derived values, and a restricted user sees only
  what the policies allow (hidden is absent);
- the NeXTSTEP-style egui GUI (`crates/gui`) is separate and untouched.

Today's improv-cosmic (see the
[2026-11-02 spike](2026-11-02-cosmic-gui.md)) is one column: a row of
eleven buttons and dropdowns across the top, the tile strips, the grid,
and the formula list under it. It works, and it reaches parity with
`crates/gui`'s core, but it reads as a test harness. Everything has the
same weight, nothing is grouped, there is no navigation, and the only
feedback is a caption at the top right.

### What makes it the same family

These are the traits the two applications share. Each is an idea; none
copies Hylki's expression.

1. **Panes with their own header bars.** A navigation sidebar on the left
   with a primary action at its top, a list or structure pane, and a
   content pane. Each pane's top bar belongs to that pane. Window
   controls live only in the rightmost pane.
2. **One rounded, inset language.** Selection and hover are rounded
   pills inset from the pane edge, not full-bleed bars. Cards sit on a
   slightly deeper ground. One radius family runs through everything.
3. **The accent means "here" and "go".** The accent fills the primary
   action, the current selection while the pane has the keyboard, and
   counts. Nothing else uses it.
4. **Act, then offer undo.** Undoable things happen at once, with a toast
   and an Undo. Only irreversible things ask first.
5. **The keyboard reaches everything.** A command palette, single-key
   navigation where it is safe, shortcuts shown beside menu entries and
   palette rows, and a shortcuts window that is generated from the same
   table the key handler uses.
6. **It adapts instead of overflowing.** Below a width the sidebar
   becomes a rail, and toolbars fold their secondary group into "⋯".
7. **Focus mode** folds the chrome away and leaves the work.
8. **Settings in one window**, with a searchable category sidebar.
9. **Calm empty states and a welcome**: a large symbolic icon, a title and
   one line, with a clear next step.

### Licensing and the clean-room process

Hylki is AGPL-3.0-or-later. Improv is MIT OR Apache-2.0. **Nothing of
Hylki's expression enters Improv**: no code or CSS, reworded or not; no
comments, doc text or UI strings beyond generic labels of three words or
fewer ("Settings", "Undo", "Search"); and no assets (icons, images,
sounds, logos, fonts, or palettes by name). A pattern is an idea. Its
expression stays in Hylki.

The process has two halves, done by different agents.

1. **The design half (this note).** One agent read Hylki's source,
   stylesheet, documentation and screenshots. It wrote two things:
   - an uncommitted description of Hylki's design language, in its own
     words, with measurements as facts and file paths for traceability;
   - this note, which maps the patterns onto Improv and libcosmic.

   Neither contains Hylki text. This note does not name Hylki's files, its
   palettes, its icons or its strings.
2. **The implementation half.** The implementer works from this note,
   that description and libcosmic's API (MPL-2.0, fine to read), and
   **does not open Hylki's source**. Questions about Hylki go to the
   design agent, who answers in words.
3. **A provenance check on every stage.** A checker (kept outside the
   repository, alongside the description) indexes Hylki's
   `src/**/*.rs` and `src/styles.css` as normalized word 8-grams. The
   normalization lowercases, splits identifiers into words, drops Rust
   keywords and punctuation, and skips grams of numbers only. The checker
   reports any line an Improv diff adds that is part of an 8-gram found
   there, with the Hylki file. Each stage of section (i) runs it on its
   diff, and the result must be 0. An option also indexes Hylki's
   documentation and translation catalogue, for prose.
   - It was demonstrated on a planted file. A verbatim doc comment, a
     verbatim stylesheet rule and a re-cased, re-punctuated copy of a
     comment were all caught. Control lines written fresh passed.
   - It was then run on this branch's diff (0 hits) and on the
     uncommitted description (0 hits, after nine phrases that had drifted
     toward the source's wording were rewritten).
4. **Screenshots** of Hylki are not committed, and nothing derived from
   them is. This note's wireframes are drawn from Improv's needs.
5. **Assets are Improv's own.** improv-cosmic uses COSMIC's icon theme
   (`icon::from_name` with freedesktop names) and its own app icon (stage
   1). Colours come from the COSMIC theme at run time.

## (b) Pattern map

How each Hylki pattern lands in Improv: its equivalent, or "not
applicable" and why. **N** = stage in section (i).

| Hylki pattern | Improv equivalent | N |
|---|---|---|
| **Three panes**: sidebar, list, content | **Sidebar** (navigation: the model's measures, views and categories) · **content** (the worksheet). No separate list pane: Improv's list *is* the sidebar's measure list, and a second column of the same items wastes the width a grid needs. The formulas go to a bottom drawer, the details to a context drawer (below). | 2 |
| Each pane has its **own header bar**; window controls only in the rightmost | libcosmic draws one header bar across the window. Its `header_start` sits over the nav bar, `header_center` over the content and `header_end` at the right with the window controls. The parts are placed so they read as belonging to the pane under them. A true per-pane header is not available without `show_headerbar(false)` and drawing our own; not worth it (open question 4). | 2 |
| **Sidebar** with a primary action at the top | `nav_bar` with sections. **Measures** (inputs, then derived, each with a type glyph), **Views** (saved layouts), and **Categories** (for structure work, with item counts). The current model's name and its store kind (file, Postgres, server) form an identity chip at the very top. | 2 |
| **Primary split button**: full width, accent, caret for variants | **New Measure** split button at the top of the sidebar. The main part adds a derived measure: it opens the formula drawer on a new card with the cursor in the name. The caret offers *New Input Measure…*, *New View…* and *New Category…*. The main part keeps one meaning everywhere; it does not change with the context (open question 2). Hidden when the caller may not `DefineMeasure`. | 3 |
| **Counts and badges** on sidebar rows | Measures: no count (a cell count is noise). **Categories: the item count** in a neutral pill (not the accent: it is not news). **Views**: none. **Accent badges are kept for news**: the formula drawer's tab shows the number of formulas with an error, and the server identity chip shows a dot while live changes from others are arriving (stage 8). | 2, 8 |
| **Chevrons** that expand a row's children | A category row expands to its items (read-only list; rename and reorder are stage 9). A measure row has no children. | 2 |
| **Avatars and identity chips** | No people, so no avatars. **The model chip** stands in its place: the model's initials in a circle tinted by store kind (accent for a local file, a neutral for a server), the name, and the target under it, dimmed (`sales.db`, `pg://…/plan`, `https://…`). A restricted session adds a lock glyph and the principal ("as *analyst*"). | 2 |
| **List rows as inset pills**, hover wash, accent selection, grey selection when unfocused | Sidebar rows (libcosmic's nav bar already draws inset rounded rows); the command palette's rows; the settings search results. The **grid's cursor** follows the same rule: the accent while the grid has the keyboard, a neutral outline while focus is elsewhere (today it is always the accent). | 2, 4 |
| **Single-line vs multi-line rows** | Sidebar density: *comfortable* (name plus a dim second line: the formula for a derived measure, the dimensions for an input) or *compact* (the name only). A setting, default comfortable. | 2 |
| **Keyboard cursor distinct from selection** | The grid already has a cursor and a range. The sidebar takes ↑/↓ through libcosmic's focus. No extra cursor. | — |
| **Cards** | **Formulas as cards** in the formula drawer: one card per derived measure, with the target name as the title, the formula in monospace, and a status line (ok; error with a located caret and a hint; *hidden from you* when the policy withholds the formula). The editing card grows to show completions and signature help. Also **the welcome screen's choices** and **settings sections** (`settings::section`, which is a card). | 3, 5 |
| **Bottom drawer** with a header, fold to header, resize by a grab handle | **The formula drawer**: under the worksheet, folded to its header row (*Formulas · 3*, an error badge, a **+** button, and a function-browser toggle) or open to about a third of the height, resizable. The function browser shares the drawer as a second tab (segmented control in the drawer's header). A grab pill on the boundary resizes it. libcosmic has no drawer widget: a column with a draggable divider (the pivot's mouse tracking already does this kind of drag). | 3 |
| **Context drawer** (right) | libcosmic's `context_drawer`, three uses: **Inspector** (the cursor cell's coordinate, value, whether it is input or derived, the formula that computes it, its format and type, and a Copy coordinate button), **Function help** (one function's signature, arguments and examples, from `improv_client::function_help`), and **About**. It docks beside the grid when wide and overlays when narrow (see (d)). | 4, 7 |
| **Customizable toolbar with overflow** | The **worksheet's action bar** (in `header_center`): Undo, Redo, Swap, Chart/Grid, Save View, Inspector. The group to the right of the separator (Print, Export, Recalculate) folds into "⋯" when narrow. **Customization is deferred**: Improv's toolbar is short, and dragging chips between zones is a lot of UI for six buttons (open question 3). | 2 |
| **Dialogs**: a notice; a destructive confirm whose default is Cancel | `Application::dialog` with `widget::dialog`: title, body, `primary_action` (destructive style for a delete), and a `secondary_action` of Cancel as the default. Used only for what cannot be undone: deleting a category or a measure that has input cells (the cells go), dropping a server's policies, opening a model over unsaved in-memory work. | 6, 9 |
| **Toasts with an undo countdown** | Every undoable change: a cell edit, a paste, a formula, a pivot, a saved view, a deleted view. A toast says what changed ("Pasted 12 × 4 cells", "Revenue redefined") and has an **Undo** button. libcosmic's toaster gives a pill, an action and a timeout. The **countdown line** (a thin accent bar along the toast's bottom edge, draining as the toast's life runs out, computed from elapsed time) is drawn by improv-cosmic. Unlike a delayed send, an Improv edit **commits at once** (the store saves each change, and other clients see it). The countdown therefore shows how long the toast's Undo stays offered, not a pending commit. Undo itself stays available afterwards from Ctrl+Z and the menu. A *delayed commit* for edits would fight the authority model: a remote writer's edit could land between ours and our undo. | 6 |
| **Command palette** (Ctrl+K), actions plus places, `>` for actions only | **Every Improv command**, every measure, view and category (go to), and **function search** (`=` prefix: functions from `improv_client::function_help`; Enter inserts at the formula caret or opens the help in the context drawer). Each row has a title, a dim kind ("Command", "Measure", "View", "Function · Statistical") and its shortcut as a keycap. Words all have to match; ranking puts prefix matches first. | 7 |
| **Focus mode** (Ctrl+Shift+F) | **The grid alone.** It folds the sidebar, the tile strips (to a one-line summary: *Rows: Time · Columns: Product · Page: Plan*), the formula drawer and the context drawer. The header keeps the title, a "⋯" and an exit control. Pivoting still works from the palette and the keys. A fixed, short list of switches in Settings (fold the tiles, fold the drawer, hide the status line), not thirty. | 8 |
| **Single-key shortcuts** (j/k, r, a…) | **Grid navigation, but typing in a cell wins.** In a spreadsheet, a printable key on a cell *starts an edit with that key* (today's behaviour, and every spreadsheet's). So single letters are **not** commands on the grid. Commands use Ctrl, Alt, function keys and the palette. Single keys work only where nothing is typed: the sidebar (↑/↓, Enter, `/` to filter the list) and the tile strips (←/→ to pick a tile, R/C/P to send it to Rows, Columns, Pages). See (e). | 4 |
| **Keymap as data**, a test that every key is documented | One `keys.rs` table: action, key, context and help text. The key handler, the menu hints, the palette's keycaps and the shortcuts window all read it. A test asserts that each action with a key appears in the help, and that no two actions share a key in the same context. | 4 |
| **Shortcuts window** (?, F1) | Ctrl+? opens it, and so does `?` when the grid is not editing and the focus is not in a text field. F1 stays the function help (Improv's convention in `crates/gui`). It is a dialog of sections with keycap rows. Toggle: the same key closes it. | 4 |
| **Welcome / onboarding carousel** | **A welcome screen, not a wizard.** Improv needs no setup. Shown when improv-cosmic starts with no argument. Choices: **Open the demo model** (built in memory, the Time × Product revenue oracle), **recent models** (file paths and URLs, most recent first, with a remove ×), **Open a file…**, **Connect to a server…** (an `https://` URL and an optional token), **Open a Postgres model…** (a `postgres://` URL and a model name), and **New empty model**. A single page, no carousel. | 5 |
| **Demo mode** (an environment variable, a demo backend, scripted captures) | Improv already has `IMPROV_COSMIC_SHOT`. The demo model becomes a first-class choice (above). **Staged screenshots** go through `IMPROV_COSMIC_SHOT_STAGE=<scene>` (welcome, palette, focus, settings, error, narrow), so every golden in section (i) is scripted and reproducible. | 1, each |
| **Empty states** (an icon, a title and a line; "nothing" told apart from "filtered out") | No measure: *This model has no measures yet*, with **New Measure**. A measure with no cells: the grid draws its headers and a centred line, *No values yet. Type in a cell or paste.* (or, read-only: *No values you can see.*). A page with nothing: *Nothing on this page*. A failed open: an error state (see below). | 5 |
| **Error state** | A failed open (missing file, wrong password, refused token, server down, policy shuts the user out) fills the content with an error status: a symbolic icon, a title naming what failed, one line of why, with **Try Again** and **Open Another…**. A restricted user shut out entirely sees *Nothing here for you*, with the principal named. It never says what exists. | 5 |
| **Status bar / notification area** | The status caption moves to a **status line in the footer** (`Application::footer`): the cursor coordinate on the left; the pending, saved or recalculating state on the right. Errors become toasts. A long operation (open, rebuild) shows a thin progress bar in the footer. | 2 |
| **Settings window**, a category sidebar, search | One settings page in the **context drawer** for a quick look, or a dedicated settings view (nav bar swapped for categories). Categories: **Appearance** (follow COSMIC, or light or dark; density; focus mode), **Numbers & Locale** (formula locale: `en`, `de`, …; decimal and argument separators shown; date format), **Keyboard** (the shortcut table; Enter moves down or right; type-to-edit on or off), **Servers & Tokens** (saved server URLs, tokens kept in the Secret Service through the oo7 crate, never in a file; Postgres URLs without passwords), **Recent Models** (clear), **Diagnostics** (export a report, the log level). A search field filters the *rows* across categories. | 7 |
| **Themes and accents** (follow the system; optional bundled themes) | **Follow COSMIC only.** libcosmic restyles live. No bundled themes: COSMIC users pick their own in COSMIC Settings, and Improv would only be adding a second theme switch. Light, dark or follow is the one override. See (f). | 2 |
| **Text size** | COSMIC's interface density and font settings (libcosmic reads them). The grid's row height and column width follow the theme's spacing and font size (stage 2). | 2 |
| **Typography, spacing, radius, motion tokens** | **COSMIC's tokens**, not new ones: `theme.cosmic().spacing` (4/8/12/16/24/32 px at standard density) and `corner_radii` (4/8/16/32/160). Motion: about 150 ms for hover and colour, about 200 ms for the rail and drawers, about 250 ms for focus mode. See (f). | 2 |
| **Swipe actions** | Not applicable: rows here are not triaged. | — |
| **Threads and conversations** | Not applicable. The closest is formula dependencies: the Inspector lists what a derived cell reads and what reads it (stage 4, read from the model, not computed). | 4 |
| **Pop-out windows** | Deferred. `multi-window` is on. A second view of the same model in its own window would be Improv's "several matrices", but parity with `crates/gui`'s multi-matrix canvas is its own project. | — |
| **Drop zones** for files | A model file dropped on the window opens (today). A dropped CSV would start the import wizard, which is not in this plan (`crates/gui`'s `csv_wizard` has the pure half to reuse). | — |
| **Tray, background running, notifications** | Not applicable: a spreadsheet does not run in the background. Desktop notifications only for a long open or rebuild that finished while the window was unfocused (stage 8, optional). | 8 |
| **i18n** | Strings through **Fluent**, the system libcosmic already uses (the `i18n-embed` and `i18n-embed-fl` crates are in the lockfile), with `en` first and `de` as the second locale to prove it. Separate from the *formula* locale (which is the model's: `Locale::de()` changes function names and separators), which stays in Settings → Numbers & Locale. | 10 |
| **Packaging**: a desktop entry, AppStream metainfo with branding colours, an icon, Flatpak | `res/me.burd.Improv.desktop` (with a *New Model* desktop action), `res/me.burd.Improv.metainfo.xml` (branding colours of our own, screenshots from the goldens, OARS), a scalable and a symbolic app icon drawn for Improv (a tile grid motif), and a Flatpak manifest on the freedesktop runtime with `finish-args` each commented: wayland and fallback-x11, ipc, dri, network (Postgres and servers), the Secret Service for tokens, and no home filesystem (portals). | 1 |
| **About, release notes, diagnostics export** | `widget::about` in the context drawer (libcosmic's `about` feature): name, version, licences, links, the developers. **Diagnostics → Export report**: the version, the libcosmic pin, the renderer (wgpu or tiny-skia), the platform, the store kind (never the URL's credentials), model size, engine timings and the last errors, written through the portal's save dialog. | 7 |

## (c) Wireframes

Plain ASCII, about 1200 × 800 (wide) and 640 × 800 (narrow). `[ ]` is a
button, `▾` a menu caret, `●` the accent, `◌` a neutral pill, `▌` the
cursor.

### Main window, wide (≥ 1100 px)

```
┌─────────────────────────────┬────────────────────────────────────────────────────────────────────────┐
│ ☰  Improv                   │  Revenue · Worksheet                  [↶][↷] [⇄][▦][✚ View][ⓘ] ⋯  ─□×  │
├─────────────────────────────┼────────────────────────────────────────────────────────────────────────┤
│ ┌───────────────────────┐   │ Rows   ◖Time◗                    Columns  ◖Product◗                    │
│ │ ●S  sales             │   │ Pages  ◖Scenario▾ Plan◗                                                │
│ │     ~/models/sales.db │   ├────────────────────────────────────────────────────────────────────────┤
│ └───────────────────────┘   │            │ Widget A  │ Widget B  │ Gadget    │                       │
│ ┌──────────────────────┬─┐  │ 2025 Q1    │ ▌  1,000  │    1,200  │      800  │                       │
│ │ ✚  New Measure       │▾│  │ 2025 Q2    │    1,100  │    1,250  │      820  │                       │
│ └──────────────────────┴─┘  │ 2025 Q3    │    1,150  │    1,300  │      860  │                       │
│ MEASURES                    │ 2025 Q4    │    1,300  │    1,450  │      900  │                       │
│  ⌗ Price            Product │ 2026 Q1    │    1,200  │    1,400  │      880  │                       │
│  ⌗ Quantity    Time·Product │            │           │           │           │                       │
│ ╭─────────────────────────╮ │            │           │           │           │                       │
│ │ƒ Revenue  Price*Quantity│ │ ══════════════════════════════ ▬▬▬ ══════════════════════════════      │
│ ╰─────────────────────────╯ │ Formulas · 2   [Formulas│Functions]                       [✚]  ⌄       │
│  ƒ Total     SUM(… OVER …)  │ ┌──────────────────────────────┐ ┌──────────────────────────────┐      │
│ VIEWS                       │ │ Revenue                    ⋯ │ │ Total                      ⋯ │      │
│  ▦ By region                │ │ Price * Quantity             │ │ SUM(Revenue OVER Time)       │      │
│  ▦ Plan vs actual           │ │ ✓ over Time, Product         │ │ ✓ over Product               │      │
│ CATEGORIES                  │ └──────────────────────────────┘ └──────────────────────────────┘      │
│  › Time                ◌ 8  │                                                                        │
│  › Product             ◌ 3  │                                                                        │
│  › Scenario            ◌ 2  │                                                                        │
├─────────────────────────────┴────────────────────────────────────────────────────────────────────────┤
│ Revenue · {Time=2025 Q1, Product=Widget A} · derived                                        ● Saved  │
└──────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- The sidebar is `nav_bar`, about 280 px. The model chip and the split
  button are rows above its sections (a custom nav bar element, see (d)).
- The header's middle names the shown measure and the view. The action
  bar is right-aligned before the window controls. "⋯" holds Print,
  Export CSV, Recalculate, Focus mode and Settings.
- The tile strips are one band, Rows and Columns side by side and Pages
  under them. They are **not** cards in cards (today's three bordered
  boxes go).
- The formula drawer is open. `▬▬▬` is its grab pill. Its header is the
  fold-up target.
- The footer status line is libcosmic's `footer`.

### Main window, narrow (< 1100 px; condensed below about 650 px)

```
┌──────┬────────────────────────────────────────────┐
│  ☰   │ Revenue               [↶][↷][⇄][ⓘ] ⋯  ─□×  │
├──────┼────────────────────────────────────────────┤
│ ●S   │ R ◖Time◗   C ◖Product◗   P ◖Plan▾◗         │
│ ┌──┐ ├────────────────────────────────────────────┤
│ │✚▾│ │          │ Widget A │ Widget B │ Gad… │    │
│ └──┘ │ 2025 Q1  │ ▌ 1,000  │   1,200  │  80… │    │
│  ⌗   │ 2025 Q2  │   1,100  │   1,250  │  82… │    │
│  ⌗   │ 2025 Q3  │   1,150  │   1,300  │  86… │    │
│ ╭──╮ │ 2025 Q4  │   1,300  │   1,450  │  90… │    │
│ │ƒ │ │                                            │
│ ╰──╯ │                                            │
│  ƒ   │                                            │
│ ──── │                                            │
│  ▦   │                                            │
│  ▦   │                                            │
│ ──── │ ═══════════════ ▬▬▬ ═══════════════        │
│      │ Formulas · 2  ●1                    ⌃      │
├──────┴────────────────────────────────────────────┤
│ Revenue · 2025 Q1 · Widget A             ● Saved  │
└───────────────────────────────────────────────────┘
```

- Below about 1100 px the sidebar is a **rail** (about 72 px): glyphs
  only, with a tooltip each. The split button collapses to ✚ with its
  caret.
- A click on ☰ or on a rail glyph **peeks** the full sidebar over the
  content. Picking a destination closes the peek. Below libcosmic's
  condensed width (about 650 px at scale 1) the nav bar is replaced by the
  peek alone.
- The tile strips compress to one line of `R` / `C` / `P` labels.
- The drawer is folded, with an error badge (`●1`).

### Focus mode

```
┌────────────────────────────────────────────────────────────────────────────────────────────────────┐
│  Revenue · Worksheet                                              Time × Product · Plan  ⤢ ⋯  ─□×  │
├────────────────────────────────────────────────────────────────────────────────────────────────────┤
│            │ Widget A  │ Widget B  │ Gadget    │                                                   │
│ 2025 Q1    │ ▌  1,000  │    1,200  │      800  │                                                   │
│ 2025 Q2    │    1,100  │    1,250  │      820  │                                                   │
│ 2025 Q3    │    1,150  │    1,300  │      860  │                                                   │
│ 2025 Q4    │    1,300  │    1,450  │      900  │                                                   │
│ 2026 Q1    │    1,200  │    1,400  │      880  │                                                   │
│  …                                                                                                 │
└────────────────────────────────────────────────────────────────────────────────────────────────────┘
```

- No sidebar, no tile strips, no drawer and no footer. The layout summary
  in the header is a button that opens the tile strips as a popover.
- `⤢` (or Ctrl+Shift+F, or Escape with nothing else to close) leaves
  focus mode. The palette and every key still work.
- Narrow is the same, with the summary shortened to `Time × Product`.

### Command palette

```
┌────────────────────────────────────────────────────────────────────────────┐
│                                                                            │
│         ┌────────────────────────────────────────────────────────┐         │
│         │ ⌕ sum o                                                │         │
│         ├────────────────────────────────────────────────────────┤         │
│         │╭──────────────────────────────────────────────────────╮│         │
│         ││ SUM                    Function · Math     =SUM(     ││         │
│         │╰──────────────────────────────────────────────────────╯│         │
│         │  SUMSQ                   Function · Math               │         │
│         │  Swap rows and columns   Command              Ctrl+/   │         │
│         │  Total                   Measure                       │         │
│         │  Revenue                 Measure                       │         │
│         ├────────────────────────────────────────────────────────┤         │
│         │ > commands  = functions  @ go to       ↑↓  ⏎  esc      │         │
│         └────────────────────────────────────────────────────────┘         │
│                                                                            │
│                      (the window dimmed behind)                            │
└────────────────────────────────────────────────────────────────────────────┘
```

Narrow:

```
┌────────────────────────────────────────────────────┐
│                                                    │
│  ┌──────────────────────────────────────────────┐  │
│  │ ⌕ sum o                                      │  │
│  ├──────────────────────────────────────────────┤  │
│  │╭────────────────────────────────────────────╮│  │
│  ││ SUM                          =SUM(         ││  │
│  ││   Function · Math                          ││  │
│  │╰────────────────────────────────────────────╯│  │
│  │  Swap rows and columns         Ctrl+/        │  │
│  │    Command                                   │  │
│  │  Total                                       │  │
│  │    Measure                                   │  │
│  ├──────────────────────────────────────────────┤  │
│  │ >  =  @          ↑↓  ⏎  esc                  │  │
│  └──────────────────────────────────────────────┘  │
│                                                    │
└────────────────────────────────────────────────────┘
```

- A `dialog` about 560 px wide, at most 60% of the window's height, in
  the top third (not centred, so the grid stays visible under it).
- The highlighted row is the accent pill. ↑↓ move, Enter runs, Escape
  closes.
- The prefixes are shown in the palette's footer row.
- Narrow: the full window width minus 16 px margins. The kind column
  drops under the title.

### Welcome

```
┌──────────────────────────────────────────────────────────────────────────────────────────────┐
│ Improv                                                                                  ─□×  │
├──────────────────────────────────────────────────────────────────────────────────────────────┤
│                                                                                              │
│                                      ▦  Improv                                               │
│                      Models of categories, items and measures.                               │
│                                                                                              │
│     ┌──────────────────────────┐ ┌──────────────────────────┐ ┌──────────────────────────┐   │
│     │ ▶  Open the demo model   │ │ ▸  Open a model file…    │ │ ✚  New empty model       │   │
│     │ Revenue over Time and    │ │ A .db store from disk    │ │ In memory until saved    │   │
│     │ Product, ready to pivot  │ │                          │ │                          │   │
│     └──────────────────────────┘ └──────────────────────────┘ └──────────────────────────┘   │
│     ┌──────────────────────────┐ ┌──────────────────────────┐                                │
│     │ ⇄  Connect to a server…  │ │ ▤  Open a Postgres model…│                                │
│     │ https:// and a token     │ │ postgres:// and a model  │                                │
│     │                          │ │                          │                                │
│     └──────────────────────────┘ └──────────────────────────┘                                │
│                                                                                              │
│     RECENT                                                                                   │
│     ┌─────────────────────────────────────────────────────────────────────────────────┐      │
│     │ ●S sales            ~/models/sales.db                          yesterday     ×  │      │
│     │ ●P plan             postgres://me@db/fin?model=plan            3 days ago    ×  │      │
│     │ ◌B budget (locked)  https://improv.example.com, as analyst     last week     ×  │      │
│     └─────────────────────────────────────────────────────────────────────────────────┘      │
│                                                                                              │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

Narrow:

```
┌──────────────────────────────────────────────────┐
│ Improv                                      ─□×  │
├──────────────────────────────────────────────────┤
│                                                  │
│                ▦  Improv                         │
│                                                  │
│  ┌──────────────────────────────────────────┐    │
│  │ ▶  Open the demo model                   │    │
│  │ Revenue over Time and Product            │    │
│  └──────────────────────────────────────────┘    │
│  ┌──────────────────────────────────────────┐    │
│  │ ▸  Open a model file…                    │    │
│  │ A .db store from disk                    │    │
│  └──────────────────────────────────────────┘    │
│  ┌──────────────────────────────────────────┐    │
│  │ ✚  New empty model                       │    │
│  │ In memory until saved                    │    │
│  └──────────────────────────────────────────┘    │
│  ┌──────────────────────────────────────────┐    │
│  │ ⇄  Connect to a server…                  │    │
│  │ https:// and a token                     │    │
│  └──────────────────────────────────────────┘    │
│  ┌──────────────────────────────────────────┐    │
│  │ ▤  Open a Postgres model…                │    │
│  │ postgres:// and a model                  │    │
│  └──────────────────────────────────────────┘    │
│                                                  │
│  RECENT                                          │
│  ┌──────────────────────────────────────────┐    │
│  │ ●S sales      ~/models/sales.db      ×   │    │
│  │ ●P plan       postgres://me@db/fin   ×   │    │
│  └──────────────────────────────────────────┘    │
│                                                  │
└──────────────────────────────────────────────────┘
```

- Choices are cards (buttons with the `Card` container class) in a
  `flex_row` that wraps. Recent models are a `settings::section` list.
- *Connect to a server…* and *Open a Postgres model…* open a `dialog`
  with the URL (and token, or model name) fields. The token field is a
  `secure_input`. The dialog has a **Remember** toggle that stores the
  token in the Secret Service.
- Narrow: the cards stack one per row at full width, and Recent shows the
  name and the target only.
- There is no sidebar on this screen. It is the content of an App with no
  model yet; `nav_model()` is `None`.

### Settings

```
┌─────────────────────────────┬────────────────────────────────────────────────────────────────┐
│ ←  Settings                 │  Numbers & Locale                                          ─□× │
├─────────────────────────────┼────────────────────────────────────────────────────────────────┤
│ ⌕ [ decimal             ]   │  ┌────────────────────────────────────────────────────────┐    │
│                             │  │ Formula locale                         [ Deutsch    ▾] │    │
│ Appearance                  │  │ Function names and separators in formulas              │    │
│ Numbers & Locale        ◀   │  ├────────────────────────────────────────────────────────┤    │
│ Keyboard                    │  │ Decimal separator                                   ,  │    │
│ Servers & Tokens            │  │ Argument separator                                  ;  │    │
│ Recent Models               │  ├────────────────────────────────────────────────────────┤    │
│ Diagnostics                 │  │ Dates                                  [ 2026-11-09 ▾] │    │
│ About                       │  └────────────────────────────────────────────────────────┘    │
│                             │                                                                │
│ ─── search results ───      │                                                                │
│ Decimal separator           │                                                                │
│   Numbers & Locale          │                                                                │
└─────────────────────────────┴────────────────────────────────────────────────────────────────┘
```

Narrow, a category open:

```
┌──────────────────────────────────────────────────┐
│ ←  Numbers & Locale                         ─□×  │
├──────────────────────────────────────────────────┤
│                                                  │
│  ┌────────────────────────────────────────────┐  │
│  │ Formula locale            [ Deutsch ▾]     │  │
│  │ Function names and separators              │  │
│  ├────────────────────────────────────────────┤  │
│  │ Decimal separator                    ,     │  │
│  │ Argument separator                   ;     │  │
│  ├────────────────────────────────────────────┤  │
│  │ Dates                   [ 2026-11-09 ▾]    │  │
│  └────────────────────────────────────────────┘  │
│                                                  │
└──────────────────────────────────────────────────┘
```

- Settings **replaces the nav bar's model** while it is open: the nav bar
  lists the categories, the content shows `settings::section`s, and ←
  returns to the model. It is not a second window. libcosmic is built
  around one main window with a nav bar and a context drawer, and a
  second window would be a second surface to keep in step with the
  theme, keys and focus. Open question 5 asks whether to use a separate
  window anyway.
- The search field filters rows across all categories. While it has
  text, the nav list shows the matches, each with its category dimmed
  under it. Picking one opens its category and scrolls to the row.
- The formula locale is the *model's* view of formulas for this user. It
  does not change the stored formulas, which are canonical.
- Narrow: the categories, and a category opened over them with ←.

### Error state

```
┌─────────────────────────────┬──────────────────────────────────────────────────────────────┐
│ ☰  Improv                   │  budget                                                  ─□× │
├─────────────────────────────┼──────────────────────────────────────────────────────────────┤
│ ┌───────────────────────┐   │                                                              │
│ │ ◌B  budget   (locked) │   │                             (!)                              │
│ │     improv.example…   │   │                                                              │
│ └───────────────────────┘   │                  Could not open budget                       │
│                             │        The server refused the token (401). It may have       │
│                             │        expired or been revoked.                              │
│                             │                                                              │
│                             │      [ Enter Another Token… ]  [ Open Another… ]             │
│                             │                                                              │
└─────────────────────────────┴──────────────────────────────────────────────────────────────┘
```

- The icon is large and symbolic. The title names the model and the
  verb. One line says why, from the error. The primary action is suggested
  style. No stack trace: the details go to Diagnostics.
- The **empty** variant has the same layout. *No measures yet* with
  **New Measure**; *No values yet* in a measure's grid; *Nothing here for
  you* for a user the policies shut out (it names the principal and
  offers **Switch Token…**).
- Narrow: the same, centred, with the buttons stacked.

## (d) libcosmic widgets

At the pin (95236a9). Everything named here exists at that commit unless
it says "compose".

| Part | libcosmic (or iced) | Notes |
|---|---|---|
| Window, header bar, window controls | `cosmic::Application` with `header_start` / `header_center` / `header_end`; `core.window.show_headerbar` | libcosmic draws the header and places the nav toggle at the start when `nav_model()` is `Some`. |
| Sidebar | `nav_bar::Model` (a `segmented_button` model: `.text`, `.icon`, `.data`, `.divider_above`, `.indent`) returned from `nav_model()`; `on_nav_select`; `nav_context_menu` for row menus | Sections are rows with `divider_above` plus an inactive heading row. Item counts and the formula second line need **a custom `nav_bar()`**: the default draws text and an icon only. Override `Application::nav_bar` to return a column of the model chip, the split button and a `widget::list_column` of `list::button` rows (inset, rounded, `.selected()`). |
| Rail and peek | `core.is_condensed()` (libcosmic computes it: nav 280 + content 360 + padding, plus 344 more when a non-overlay context drawer shows); `nav_bar_toggle`; `Core::nav_bar_toggle_condensed` | libcosmic **hides** the nav when condensed. The icon rail between 650 px and 1100 px is ours: the custom `nav_bar()` draws glyph-only rows when `on_window_resize` reports a width under 1100. |
| Primary split button | compose: a `row` of `button::custom` (the accent `Button::Suggested` class) and a caret `button::icon` in the same class, inside a container with `radius_m`; the caret opens a `widget::popover` with a `menu::items`-style column | No split-button widget exists in libcosmic. Keep the divider a 1 px `divider::vertical` in the accent's `on` colour at reduced alpha. |
| Identity chip | compose: `container` (Card) with a circle (`container` with `radius_xl`, tinted) holding initials as `text::title4`, beside `text::body` and `text::caption` | |
| Tile strips | today's `mouse_area` pills, restyled: `radius_xl`, `button_bg_color`; held = accent | Keep the mouse tracking (it works on Windows). A11y: each tile gets an accessible name and the R/C/P keys, see (g). |
| Grid | today's `canvas` (`sheet.rs`) | Plus an AccessKit layer, see (g). |
| Formula drawer | compose: `column` [grid, grab pill (`mouse_area` over a 100 × 5 `container` with `radius_xs`), drawer]; the drawer is a `container` with a header `row` and a `scrollable` `flex_row` of formula cards | No drawer widget exists; the height is app state, and the drag is the same mouse tracking as the tiles. |
| Drawer tabs | `segmented_control::horizontal` over a `segmented_button::SingleSelectModel` (Formulas, Functions) | |
| Formula card | `container` (`Container::Card`), `text::heading`, `text::monotext`, the located caret line (today's `formulas::caret_line`), `text::caption` | The editing card uses `text_input` as today, plus the completion row. |
| Context drawer (Inspector, function help, About) | `Application::context_drawer` returning `app::context_drawer(content, on_close).title(…).actions(…)`; `ApplicationExt::set_show_context`; `on_context_drawer` | Its width is 344–480 px (libcosmic). By default it overlays the content (`core.window.context_is_overlay` starts `true`). Set it `false` on wide windows so the drawer docks beside the grid, which libcosmic then counts in its condensed width. About is `context_drawer::about` (feature `about`). |
| Toasts | `widget::toaster(&toasts, content)` around the view; `Toasts::new(Message::CloseToast)`; `Toast::new(text).action("Undo", …).duration(…)`; `toasts.push(toast)` returns a `Task` | The **countdown line** is ours: wrap the toaster's content or overlay a 3 px `container` whose width is the remaining fraction, from an `Instant` and a `window::frames()` subscription that runs only while a toast is up. |
| Dialogs | `Application::dialog` returning `widget::dialog().title().body().icon().primary_action(button::destructive(…)).secondary_action(button::standard("Cancel"))` | libcosmic shows it in a modal popover over the view. |
| Command palette | `Application::dialog` with a `search_input` (`text_input::search_input`) and a `scrollable` `list_column` of `list::button` rows; keycaps as `container` + `text::monotext` with `radius_xs` | The palette is a dialog, so it is modal and gets the dim. Escape goes through `on_escape`. |
| Shortcuts window | `Application::dialog` with `settings::section` per group; keycaps as above | |
| Settings | swap `nav_model()` for a categories model; content `settings::view_column(vec![settings::section().title(…).add(settings::item(title, control))…])`; controls `toggler`, `dropdown`, `spin_button`, `text_input` | `settings::item::builder(title).description(…).toggler(…)` gives the title, description and control row. |
| Welcome | `flex_row` of card buttons (`button::custom` with `Container::Card` content), `settings::section` for recent models | |
| Empty / error states | compose: a centred `column` [`icon::from_name(…).size(64)`, `text::title3`, `text::body`, a `row` of buttons] | No status-page widget exists. Make one `status_page(icon, title, body, actions)` helper used everywhere. |
| Footer status line | `Application::footer` | |
| Overflow "⋯" | `menu::bar` / `menu::Tree` or a `popover` with `menu`-style rows | At the pin `responsive_menu_bar` needs the `surface-message` feature (on Wayland only). The fold is ours: measure the action bar's width in `on_window_resize` and move the right group into "⋯". |
| Keyboard | `keyboard::listen()` (today) for the grid; `menu::KeyBind` for matching the table's chords (it handles non-Latin layouts by physical key); `keyboard_nav::subscription` gives Tab/Shift+Tab focus, Escape, F11 (maximize) and Ctrl+F (`on_search`) | |
| Tooltips | `widget::tooltip(content, text, Position::Bottom)` | Every icon-only button gets one, with the shortcut. |
| Theme | `cosmic::theme::active()`, `theme.cosmic()` (spacing, `corner_radii`, `accent_color()`, `is_dark`, `is_high_contrast`), `system_theme_update` | See (f). |
| Secrets (tokens) | `oo7` (the Secret Service client; MIT; not yet in the lockfile, so a `deny.toml` check is needed) on Linux and BSD; Windows Credential Manager through the `windows` crate | Stage 5. Not keyring-rs: it pulls several backends. |

## (e) Keyboard

**Principles:**

1. **Typing in a cell wins.** A printable key with no modifier on the
   grid starts an edit holding that key. No single-letter command exists
   on the grid.
2. **Commands carry a modifier** (Ctrl, Alt, Ctrl+Shift), are function
   keys, or are reachable from the palette.
3. **Single keys only where nothing is typed:** the sidebar, the tile
   strips, the palette list and the shortcuts window.
4. **A focused text field keeps its keys**, Ctrl+Z included (text undo).
   Ctrl+K, Ctrl+Shift+F and Escape are the only window keys a text field
   does not take.
5. **One table** (`keys.rs`) is the key handler, the menu hints, the
   palette's keycaps and the shortcuts window. A test keeps them in step.

"Ctrl" below is Ctrl on Linux, BSD and Windows. On macOS it is Cmd, should
improv-cosmic ever ship there. `*` marks a key that is new.

### Everywhere (the window)

| Key | Action |
|---|---|
| Ctrl+K * | Command palette |
| Ctrl+Shift+K * | Palette, commands only (`>`) |
| Ctrl+F * | Find: the palette with `@` typed (go to a measure, view, category or item) |
| Ctrl+P | Print |
| Ctrl+O | Open a model… |
| Ctrl+N * | New model (the welcome screen) |
| Ctrl+Shift+S | Save As… |
| Ctrl+W * | Close the model (to the welcome screen) |
| Ctrl+Q * | Quit |
| Ctrl+Z | Undo (the window's, when no text field has focus) |
| Ctrl+Shift+Z, Ctrl+Y | Redo |
| Ctrl+, * | Settings |
| Ctrl+? * | Keyboard shortcuts |
| F1 * | Function help (in the context drawer; the word at the formula caret if editing) |
| Ctrl+I * | Inspector (the context drawer) on or off |
| Ctrl+Shift+F * | Focus mode on or off |
| F11 | Maximize or restore (libcosmic's `keyboard_nav` maps F11 to `toggle_maximize` at the pin) |
| Ctrl+B * | Sidebar shown or hidden (the rail when narrow) |
| Ctrl+J * | Formula drawer open or folded |
| Ctrl+Shift+J * | Function browser (the drawer's second tab) |
| Ctrl+1 … Ctrl+9 * | Show measure 1–9 (sidebar order) |
| Ctrl+Alt+1 … Ctrl+Alt+9 * | Open view 1–9 |
| Ctrl+R * | Recalculate (rebuild the engine) |
| Ctrl+Shift+V * | Save the layout as a view… |
| Escape | Close the topmost: palette or dialog, context drawer, edit, range, focus mode |
| Tab, Shift+Tab | Move focus between panes and controls (outside the grid) |
| F6, Shift+F6 * | Move focus to the next or previous pane: sidebar, tiles, grid, drawer, context drawer |

### The grid (not editing)

| Key | Action |
|---|---|
| Arrows | Move the cursor |
| Shift+arrows | Extend the range |
| Tab, Shift+Tab | Right, left (Tab inside the grid stays in the grid) |
| Page Up, Page Down | A screen up or down |
| Alt+Page Up, Alt+Page Down * | A screen left or right |
| Home, End | First or last column of the row |
| Ctrl+Home, Ctrl+End | First or last cell |
| Ctrl+arrows * | To the edge of the data in that direction |
| Ctrl+A * | Select all |
| Shift+Space, Ctrl+Space * | Select the row, or the column |
| Enter, F2 | Edit the cell (its text selected) |
| any printable key | Edit the cell, starting with that key |
| Delete, Backspace | Clear the range (one undoable change) |
| Ctrl+C, Ctrl+V | Copy, paste the range (TSV and HTML) |
| Ctrl+X * | Cut (copy, then clear) |
| Ctrl+/ * | Swap rows and columns |
| Ctrl+[ , Ctrl+] * | Previous or next item of the first page category |
| Ctrl+= * | Edit the shown measure's formula (opens the drawer on its card) |
| Menu, Shift+F10 * | Context menu of the cell (Inspect, Copy, Paste, Clear, Format…) |

### Editing a cell

| Key | Action |
|---|---|
| Enter | Commit, move down (a setting: down or right) |
| Shift+Enter * | Commit, move up |
| Tab, Shift+Tab | Commit, move right or left |
| Escape | Cancel |
| Ctrl+Z, Ctrl+Shift+Z | The text field's own undo and redo |

### Formula drawer (a card editing)

| Key | Action |
|---|---|
| Enter | Commit (with a completion list showing: take the first) |
| Tab | Take the highlighted completion |
| ↑, ↓ * | Move through the completions |
| Ctrl+Space * | Show completions now |
| F1 * | Help for the function at the caret |
| Escape | Cancel the edit (the card keeps its last good formula) |

### Sidebar (focused)

| Key | Action |
|---|---|
| ↑, ↓ | Previous or next row |
| Enter, Space | Open (show the measure or view) |
| → , ← | Expand or fold a category |
| `/` * | Filter the sidebar's rows |
| F2 * | Rename (stage 9; by capability) |
| Delete * | Delete the view, or the measure after confirming (by capability) |
| Menu, Shift+F10 | The row's context menu |

### Tile strips (focused)

| Key | Action |
|---|---|
| ← , → * | Previous or next tile |
| R, C, P * | Send the tile to Rows, Columns or Pages (at the end) |
| Shift+← , Shift+→ * | Move the tile within its strip |
| ↑, ↓ * | On a page tile: previous or next page item |
| Space, Enter * | Open a page tile's item list |

Typing letters is safe here: the strips have no text field.

### Palette, dialogs, shortcuts window

| Key | Action |
|---|---|
| ↑, ↓, Page Up, Page Down | Move |
| Enter | Run or accept |
| Escape | Close |
| `>` `=` `@` (first character) | Commands, functions, go to |

## (f) Theming

- **Follow COSMIC, always.** `cosmic::Application` restyles live from
  cosmic-settings-daemon and cosmic-config (the `dbus-config` feature,
  on today). The one override is *Appearance → Style: Follow COSMIC /
  Light / Dark*, applied with `cosmic::command::set_theme`.
- **No palettes of our own and none borrowed.** Every colour is a
  `theme.cosmic()` role:
  - `accent_color()`: the primary action, the selection while focused,
    news badges, the cursor cell's outline, the toast countdown.
  - `background`, `primary` and `secondary` containers: pane grounds and
    cards; the drawer and context drawer are `primary`.
  - `bg_divider()`: grid rules and pane dividers.
  - `destructive_color()` and `warning_color()`: refused input and formula
    errors (today an error uses the accent text; it should use the
    destructive role).
  - `on_*` colours for text on each ground.
  - Derived cells keep today's "dim" treatment (an `on_primary_container`
    tint), so input and derived read apart without colour alone (also a
    glyph in the Inspector).
- **Charts:** the accent first, then the palette's hues in COSMIC's order
  (today's `chart::colors`).
- **Tokens:** COSMIC's `spacing` and `corner_radii`, never literals.
  - Pane inset `space_xxs` (8). Row padding `space_xxs` × `space_xs`.
    Section gap `space_s` (16). Card padding `space_s`.
  - Radii: `radius_s` (8) for rows and keycaps, `radius_m` (16) for cards
    and the split button, `radius_xl` for pills (tiles, counts, toasts),
    `radius_xs` (4) for the grab pill.
  - With COSMIC's *Compact* density or *Square* roundness the whole app
    follows. That is the point of using the tokens.
- **Grid metrics follow the font:** the row height is the body line height
  plus `space_xxs`, and the column width starts at 8 em. Today they are the
  constants 28 and 112 px.
- **Motion:** about 150 ms ease-out for hover and colour, about 200 ms for
  the rail, drawer and context drawer, about 250 ms for focus mode, and
  the countdown is linear. COSMIC has no reduce-animations setting at
  this pin. improv-cosmic gets *Appearance → Reduce motion* (off), which
  sets every duration to 0, and reads the setting from COSMIC when one
  exists.
- **High contrast:** `theme.cosmic().is_high_contrast` thickens the
  cursor outline and grid rules from 1 to 2 px. Selection then uses the
  accent outline only, never a tint, since a tint can drop below 3:1.

## (g) Accessibility

**Today:** AccessKit is on for the free Unixes (`a11y` feature) and off
on Windows (`accesskit_winit` does not build there at this pin, an
accepted limitation). At this pin, the libcosmic and iced widgets that
publish AccessKit nodes are buttons, checkboxes, sliders, images and
SVGs, text, scrollables, containers, rows and columns, the header bar,
the context drawer and menus. **The `text_input`, the `dropdown` (its
node code is commented out), cosmic's own `toggler`, the
`segmented_button` and `nav_bar`, and every `canvas` publish nothing.**
So a screen reader hears the buttons but not the grid, the measure
picker, the nav bar, the settings switches, or the text being typed.

**Proposal: the grid publishes an ARIA-style grid through AccessKit.**

1. **A wrapper widget, not the canvas.** `canvas::Program` cannot publish
   nodes: only `Widget::a11y_nodes` can, and `Canvas` does not forward it.
   So `sheet.rs` gets a thin `GridA11y<'a>` widget that wraps the canvas
   element. It forwards layout, drawing and events to the canvas. It
   implements `a11y_nodes(layout, state, cursor)` and returns an
   `A11yTree` whose root is a `Role::Grid` node.
   - **The grid node:** the label is the measure name and the view ("Revenue,
     by Time and Product, page Plan"). Its row and column counts are the
     *whole* grid's (`set_row_count`, `set_column_count`), so the reader can
     say "row 3 of 1,000,000".
   - **Children:** `Role::Row` nodes for the rows **in view** (plus one above
     and one below, so arrowing announces before scrolling), each with
     `set_row_index`. Each row has a `Role::RowHeader` per row category
     (its item names) and `Role::GridCell` children for the visible cells.
     The header row has `Role::ColumnHeader` nodes.
   - **Each cell node:**
     - the label is the formatted value, the bounds come from
       `sheet::cell_rect`, with `set_column_index` and `set_row_index`;
     - the description gives the coordinate, read from the headers
       ("2025 Q1, Widget A");
     - it is read-only when the cell is derived or the policy forbids
       `EditCell`, with "derived" in the description;
     - the selected state is set for cells in the range.
   - **Ids:** each node id is derived from (row, column), so ids are
     stable across frames and AccessKit sends diffs, not a new tree.
2. **Focus.** The focused node is the cursor cell. When the cursor moves,
   the wrapper changes the tree's focus to that cell's node. Orca then
   announces the cell (label, row and column headers), the way it does in
   LibreOffice. A live region is **not** used for navigation, since that
   would double-announce.
3. **Actions from the assistive technology.** `Focus` on a cell node moves
   the cursor there (scrolling it into view). `Click` begins an edit.
   `SetValue` on an editable cell commits that text through the same path
   as typing (`parse_typed`, a refusal announced). `ScrollIntoView` scrolls.
   iced's winit runner delivers these as `Event::A11y(id, request)`, and
   the wrapper's `update` maps them to `sheet::Event`s.
4. **The editor overlay** is a `text_input`, which has no node at the pin.
   **Upstream fix:** a small `a11y_nodes` for cosmic's `text_input`
   (`Role::TextInput`, the value, the selection, the label), offered to
   pop-os/libcosmic. **Until then:** the grid wrapper publishes the open
   editor as a `Role::TextInput` child of the cell node, with its text and
   the caret, from the app's own state.
5. **Everything else.**
   - Every icon-only button gets a label: libcosmic's `button` takes one
     under `a11y`, or wrap it in a `tooltip`.
   - Tiles become `button` widgets, which publish nodes ("Time, on Rows.
     R, C or P to move"). The drag starts from `on_press_down`, as the
     mouse tracking does today.
   - Toasts announce through a `Live::Polite` text node, and errors
     through `Live::Assertive`.
   - The custom nav bar (section (d)) is built from `list::button`s, which
     render through `button::custom` and so publish.
   - The dropdown is replaced by the palette and the sidebar in the main
     window, and fixed upstream for Settings (offer the commented-out code
     back, finished). cosmic's `toggler` gets the same treatment: until
     it publishes, each settings switch sits in a labelled `button`
     that toggles it.
6. **Not canvas-only colour.** Input versus derived, a refused cell and
   the cursor are each distinguished by shape or text as well as colour
   (an outline, a glyph in the Inspector, the description).
7. **Windows** stays without AccessKit until `accesskit_winit` builds
   there (pop-os/libcosmic#1152). The design does not depend on it.

**Tests:** the node builder is pure: given a `Grid`, an origin, a size, a
cursor and a selection, it returns the node list. Unit tests cover the
counts, indices, headers, the read-only and selected states, and the ids
staying the same as the cursor moves. A proptest checks that every
visible cell has exactly one node, and that its indices decode back to
its key. A manual Orca run on lixi's COSMIC session (stage 10) is
recorded in the review note.

## (h) What stays from today's improv-cosmic

The redesign is a new view over the same insides. The parts below keep
their behaviour, with one exception, named at the end of the list.

- **The model service** (`service.rs`): one thread per window, the
  `ModelService` trait (local today; a remote service through
  `improv_client` is on `work/live`, not yet on main), the channel and
  `Subscription`, and edits applied when the service answers. The
  authority stays the authority.
- **Pivot and grid** (`pivot.rs`, `nav.rs`, `sheet.rs`'s drawing): the
  mixed-radix lines, painting only what is in view, a million rows in
  constant work, and the cursor and range rules.
- **Pivot by drag** (mouse tracking, which works on Windows), Swap, page
  items, and the refusal when a move would empty rows or columns.
- **Typed entry** (`entry.rs`): the value type decides, refusals have
  reasons, and derived cells are never edited.
- **Formulas** (`formulas.rs`): parse in the user's locale, print as DSL
  or controlled English, located errors, completion through
  `improv_client`. They move from a list to cards; the logic does not
  change.
- **Views** (`pivot::views`, `Layout::from_view`), **charts**
  (`chart.rs`), **copy and paste** (`clip.rs`, `win.rs`, `winfmt.rs`), the
  **portals** for open, save and print (`print.rs`), **file drop**, and
  **undo/redo** (`Step`, `Travel`, `UNDO_DEPTH`). Undo gains the toast;
  the stack is the same.
- **Pins and platforms:** the standalone workspace, `--locked`, the
  `windows_gaps` tripwire, the Solnix build, `deny.toml`. No new git
  dependency. `oo7` (stage 5) is the one new crates.io dependency, checked
  with cargo-deny.
- **`IMPROV_COSMIC_SHOT`**, extended with scenes (stage 1).
- **The app id** `me.burd.Improv` and the CLI (`improv-cosmic [model]`).
  The exception: with no model it now opens the welcome screen instead of
  an empty in-memory model (stage 5; the man page says so).

What goes:

- the eleven-button top row;
- the three bordered strip boxes;
- the status caption at the top right;
- the formula list under the grid;
- the dropdowns for measures and views (the sidebar and the palette
  replace them).

## (i) Staged plan

Each stage is one mergeable unit on its own branch:

- green under `ci/gui_cosmic.sh` (fmt, clippy `-D warnings`, tests,
  rustdoc, cargo-deny);
- the provenance check at 0 on its diff;
- `docs/src/cosmic-gui.md`, `improv-cosmic(1)` where the CLI changes, and
  a CHANGELOG `## [Unreleased]` entry in the same commit;
- **any screenshot golden updated deliberately**: the stage says which,
  and the commit message names the change.

Goldens are taken at 1200 × 800 (wide) and 640 × 800 (narrow) under Xvfb
with an empty `XDG_CONFIG_HOME` (COSMIC's default dark theme), through the
`IMPROV_COSMIC_SHOT_STAGE` scenes.

| # | Stage | Acceptance criteria |
|---|---|---|
| 1 | **Ground: the shot scenes, packaging, the status helper** | `IMPROV_COSMIC_SHOT_STAGE=<scene>` stages a scene before the capture (today's single shot is scene `main`). `scripts/cosmic-shot-check.sh` takes `--scene` and checks every golden. `res/me.burd.Improv.desktop` (with a *New Model* action), `res/me.burd.Improv.metainfo.xml` (passes `appstreamcli validate --no-net`), a scalable and a symbolic icon (`res/icons/hicolor/…`, drawn for Improv, SVG, under the repo licence), an install recipe in the README, and a Flatpak manifest under `crates/gui_cosmic/flatpak/` that builds offline from `cargo vendor` (the Solnix path already vendors). The `status_page` helper, with a unit test. **Goldens:** unchanged (scene `main` = today's). |
| 2 | **The shell: nav bar, header actions, tile band, footer** | A custom `nav_bar()` with the model chip, the measures, views and categories (item counts), comfortable or compact rows, and the rail between 650 and 1100 px. The header actions (Undo, Redo, Swap, Chart, Save View, Inspector) with the right group folding into "⋯". The tile band (one band; R/C/P compact when narrow). The footer status line (the coordinate, saved or pending). Grid metrics from the theme. The top-row buttons and the measure and view dropdowns are removed. Tests: picking a measure or view in the nav model shows it (app tests, as today's `opened()` helper); the rail threshold; the fold threshold (pure functions of width). **Goldens:** `main` and new `main-narrow`, both replaced, said so in the commit. |
| 3 | **The formula drawer and the split button** | Formulas as cards in a bottom drawer: fold, open, resize by the grab pill, an error badge in the folded header, the Functions tab (the `improv_client::function_help` groups, with search). The **New Measure** split button (a derived measure via the drawer; the caret offers a new input measure, a new view and a new category, each by capability, and is hidden for a read-only caller). Tests: a refused formula opens its card with the caret line (as today); a new derived measure's card commits through `Request::Restructure`; the capability hides the button (a fake service with no `DefineMeasure`). **Goldens:** `main` (the drawer open), `drawer-folded`. |
| 4 | **Keyboard and the Inspector** | `keys.rs`, the one table, with the handler, menu hints, palette keycaps and shortcuts window all read from it. Every key in section (e) except the palette's. F6 pane cycling. Tile-strip keys. The Inspector in the context drawer: the coordinate, value, input or derived, the formula, what it reads and what reads it, the format, and Copy coordinate. The cursor turns neutral when the grid loses focus. Tests: the table has no duplicate key per context; every keyed action is in the help; type-to-edit beats every single key on the grid (a property test over printable characters); R/C/P pivot like the drag. **Goldens:** `shortcuts`, `inspector`. |
| 5 | **Welcome, connections, empty and error states** | No argument → the welcome screen: the demo model (built in-process, the revenue oracle), recent models (kept in cosmic-config, the app's state), open a file, connect to a server (URL plus token, optionally remembered in the Secret Service through `oo7`, `secure_input`), open a Postgres model, and new empty. Ctrl+N and Ctrl+W go to it. Empty states (no measures; no values; nothing on this page; nothing for you) and the error state for a failed open, with Try Again. Tests: each error class (missing file, refused token, unreachable server, policy shut out) renders its title and actions without naming anything hidden; recent models are deduplicated and capped at 10; a token is never written to the recent list or the log. **Goldens:** `welcome`, `welcome-narrow`, `error`, `empty`. |
| 6 | **Toasts with undo, and dialogs** | `widget::toaster` around the view. Each undoable change toasts with Undo and the countdown line, at about 6 s, and is configurable to 0 (off). Undo from the toast is the same `travel(true)` as Ctrl+Z, and is disabled while a change is pending, as today. Only irreversible actions get a `dialog` (deleting a category, or a measure holding inputs), with Cancel as the default. Errors become toasts. Tests: one toast per answered change, none for a refused one; the toast's Undo restores the cells (a `Step::Cells` round trip, as today); the countdown fraction is computed from elapsed time (a pure function); two quick edits give two toasts and two undo steps. **Goldens:** `toast` (frozen with the countdown at 50% through a test hook). |
| 7 | **Command palette, function search, settings, about, diagnostics** | Ctrl+K palette over every keyed and unkeyed command, every measure, view and category, and functions (`=`); prefix filters; ranking (prefix before substring, then by kind); the keycaps. Settings in the nav bar's place: Appearance, Numbers & Locale, Keyboard, Servers & Tokens, Recent Models, Diagnostics, About, with row search. `widget::about`. Export a diagnostics report through the portal's save dialog (no credentials, a test asserts it). Tests: the palette's matcher (every word must match, prefixes, ranking: pure); every command in `keys.rs` is in the palette; the settings search finds a row by its title and description; the formula locale round trip (a formula shown in `de` reads back the same). **Goldens:** `palette`, `settings`, `settings-narrow`. |
| 8 | **Focus mode and live state** | Ctrl+Shift+F: the grid alone, the layout-summary button with the tiles in a popover, `⤢` to leave, and Escape. A short settings list. Animations through `cosmic::anim`, honouring Reduce motion. Live state from `work/live`'s change stream: the model chip's dot while others' changes arrive, a toast "3 cells changed by others" (no undo; it is not ours), and a cell flash (an accent outline fading over about 600 ms; off under Reduce motion). Tests: focus mode's state round trips; Escape leaves it only when nothing else is open; a remote change updates the grid without touching the undo stack. **Goldens:** `focus`, `focus-narrow`. |
| 9 | **Structure editing** | Add, rename and delete categories and items, and add and delete measures, from the sidebar (context menus, F2, Delete) and the split button's caret, by capability, through the service; deletes confirmed when irreversible. This closes much of the guide's "Not yet". Tests: each through `Request::Restructure`, undoable as a `Step::Structure`; refused by capability with the reason; the confirm dialog's default is Cancel. **Goldens:** `main` only if the sidebar changed. |
| 10 | **Accessibility and i18n** | The `GridA11y` wrapper (section (g)) with its pure node builder and tests; labels on every icon button; tiles as buttons; toasts as live regions; the `text_input` node offered upstream (link in the note). Strings through Fluent (`i18n/en/improv_cosmic.ftl`, `i18n/de/…`), with a test that every `fl!` id exists in `en`, and a `de` golden. A manual Orca session on lixi recorded in this note: navigate the grid, edit a cell, pivot by keys, use the palette. **Goldens:** `main-de`. |

Each stage is about one to three days of focused work. Stages 2–4 are
the bulk of the visual change; after stage 4 the app already reads as
the redesign.

## (j) Open questions for the maintainer

1. **Is the list pane really dropped?** This design has two columns
   (sidebar and worksheet), not Hylki's three, on the grounds that a
   spreadsheet needs the width and the sidebar is already the list. The
   alternative is a middle pane of formula cards beside the grid (the
   formulas as the "list", the grid as the "reader"), which is closer to
   the family look and costs about 320 px of grid.
2. **The primary verb.** *New Measure* (proposed: one meaning
   everywhere), or *New View*, or one that follows the context?
3. **Toolbar customization.** Deferred here: Improv's action bar has
   about six buttons. Is drag-and-drop customization wanted anyway, for
   the family resemblance?
4. **Per-pane header bars.** libcosmic draws one header across the
   window. Hiding it and drawing our own per-pane bars gets the family
   look exactly, but costs the window-control, drag and double-click
   handling libcosmic does for free, on every platform. Proposed: keep
   libcosmic's header.
5. **Settings in the window, or its own window?** Proposed: in the
   window, swapping the nav bar's content, so there is one surface.
6. **The toast's undo window.** About 6 s, with Ctrl+Z always able to undo
   afterwards (proposed). Or should the toast stay until the next change?
7. **Single keys on the grid.** Proposed: none (typing wins). Some users
   of keyboard-first spreadsheets want a "navigation mode" toggle (as in
   vi-like sheets) where letters are commands. Out of scope unless asked.
8. **Tokens in the Secret Service** through `oo7` on Linux and BSD (a new
   crates.io dependency, MIT), the Credential Manager on Windows, and
   nothing on illumos (it has no Secret Service there; the token is asked
   for each time). Acceptable?
9. **Flatpak and distribution.** Flathub, or a self-hosted repository, or
   only the manifest for now? Flathub needs the libcosmic git dependencies
   vendored (fine) and an app id the maintainer controls
   (`me.burd.Improv` is).
10. **The libcosmic localize file's licence header.** At the pin,
    `src/localize.rs` in libcosmic (compiled unconditionally, and used by
    its context drawer, about and calendar widgets) carries an
    `SPDX-License-Identifier: GPL-3.0-only` header. The crate's LICENSE is
    MPL-2.0 and `deny.toml` clarifies it as MPL-2.0. The file is 49 lines
    of i18n-embed loader boilerplate and is likely a template leftover.
    Ask upstream to relicense or confirm before the next release? This
    affects today's binary, not only the redesign.
