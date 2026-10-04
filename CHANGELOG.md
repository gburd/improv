# Changelog

All notable changes to Improv are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Security

- **External functions: the host picks the sandbox.** Every body runs
  `Restricted` unless the operator sets `IMPROV_EXTFN_TRUSTED=1`; a model's
  `pure: false` no longer turns the sandbox off (S2).
- **External functions fail closed.** Without `bwrap` (bubblewrap) a
  `Restricted` body is refused with "bwrap not found; install bubblewrap or
  set IMPROV_EXTFN_ALLOW_UNSANDBOXED=1"; that variable falls back to rlimits
  only. A failed `setrlimit` aborts the spawn (S4, S18).
- **External functions: tighter bwrap.** Only `/usr`, `/bin`, `/lib`,
  `/lib64` and `/etc` are bound read-only; `/tmp`, `/home` and `/root` are
  empty tmpfs; all namespaces are unshared (no network). The child
  environment is cleared to `PATH=/usr/bin:/bin`, `HOME=/tmp` and `LANG`, so
  API tokens and cloud credentials no longer leak (S5, S6).
- **External functions: bounded output.** stdout and stderr are drained
  concurrently and capped at 1 MiB each; output over 64 KiB no longer
  deadlocks into a timeout (S7).
- **External functions: the timeout kills the process group**, so forked
  grandchildren die with the body (S8).
- **Wasm functions:** a fuel budget stops runaway loops (the worker thread
  ends), linear memory is capped at 64 MiB, and bodies must start with
  `wat:` or `base64:` (a bare path is no longer read from disk) (S9).
- **Python functions:** the body is passed verbatim as JSON data instead of
  inside an escaped raw string, so backslashes keep their meaning (S20).
- **Server: the refresh scheduler is off by default** (`IMPROV_SCHEDULER=1`
  enables it) and runs only external functions listed by digest in the
  `IMPROV_EXTFN_ALLOW` file, always in the `Restricted` sandbox. Serving a
  shared model no longer runs its code (S1).
- **Server: request limits.** 10 s header-read timeout (slow-loris), 30 s
  request timeout, 64 requests in flight, 1 MiB bodies, 10,000 edits per
  `/eval` (S10, S13).
- **Server: write validation.** Writes need an existing input measure, a
  coordinate covering exactly its categories, and finite values. Writes are
  serialized, so concurrent ones no longer lose updates. New
  `IMPROV_API_TOKENS_RO` read-only tokens (S12).
- **Server: internal errors** return `{"error":"internal error","request_id":N}`
  and log the detail, instead of echoing store paths (S14).
- **Server: auth hardening.** Constant-time token comparison; open mode on a
  non-loopback address needs `IMPROV_ALLOW_OPEN=1` (S15).
- **Server: model cache.** Requests share one cached model, reloaded only
  after a write, instead of loading the store up to three times per request
  (P4).
- **SQL refresh is read-only.** A stored `SqlSource.query` must be a single
  `SELECT`/`WITH` statement (`check_read_only_sql`; stacked statements are
  refused), and every backend executes it read-only: Postgres in a
  `READ ONLY` transaction, DuckDB in a `READ ONLY` transaction with
  `enable_external_access = false` (no `read_csv('/etc/passwd')`, URLs or
  `ATTACH`), SQLite only if `sqlite3_stmt_readonly`. A data-modifying CTE
  (`WITH x AS (DELETE ...) SELECT ...`) is refused by the server.
- **Postgres fails closed on cleartext:** this build has no TLS, so
  `connect_postgres` refuses a non-loopback host unless the connection
  string says `sslmode=disable` explicitly, and refuses `sslmode=require`
  (it cannot be honoured). Use an SSH tunnel to a loopback port.
- Postgres `NUMERIC` columns now import as numbers (they failed to decode).
- **CSV export neutralizes formula injection.** Text cells, item names and
  headers starting with `=`, `+`, `-`, `@`, tab or CR are written with a
  leading `'`, so Excel/LibreOffice show them as text instead of running
  them (S16). Numbers are untouched. `ExportSpec { neutralize_formulas:
  false, .. }` with `export_measure_csv_with` turns it off for an exact
  round trip.
- **CSV import is size-capped.** A field over 1 MiB, a record over 64 MiB
  (checked while streaming, so an unterminated quote can't buffer the
  file) or more than `ImportSpec::max_records` records (default
  10,000,000) is an error instead of an out-of-memory (S17).
- **CNL parser depth bound (S3).** The `nl_formula` recursive-descent parser
  had no nesting limit: one ~200 KB `POST /nl/parse` of nested parens
  overflowed the stack and aborted `improv-server`. Paren nesting plus
  operator count is now capped at 200 (as in the formula parser), returning
  the new `NlError::TooDeep`; tested on a 2 MiB stack with 100k parens.

### Performance

- **Store load is no longer one query per cell (P1).** `load_model` /
  `load_partial` fetch (measure, coord, value) in one query per value
  attribute (three in all) instead of a query per cell. On a 100k-cell
  model (release CLI, 100×1000 CSV) `improv eval` went from 279.9 s to
  0.58 s and `improv list` from 283.2 s to 0.67 s. Date cells now load back
  as dates instead of text.
- **Engine edits cost O(changed measures), not O(model)** (P3).
  `session::Engine` now returns a copy-on-write `session::Snapshot`
  (`Arc<HashMap<MeasureId, Arc<MeasureValues>>>`): an edit shares every
  unchanged measure with the previous snapshot instead of cloning all
  tracked cells. On the `incremental_many_edits` stress test (release,
  5,000 single-cell edits) the time per edit drops from 950 us to 62 us at
  10k cells (1,052 to 16,097 edits/s), and from 5,639 us to 71 us at 100k
  cells (177 to 14,039 edits/s). It is now almost flat in model size.
  `Engine::new`/`set`/`clear`/`apply` return `Snapshot`, and callers clone
  it cheaply.

### Changed

- TUI: the status box grows to fit its content, so the edit/command prompt
  and messages stay visible when `pages:`/`filters:` lines are shown. The
  key help wraps to the terminal width, and the prompt sits right under the
  title so a short terminal clips the help first.
- TUI: a derived measure's formula shows on a status line in controlled
  English (`Revenue = Price times Quantity`).

- GUI: the model browser now leads with a NeXT-style white list of saved
  views (`Name - Worksheet` rows with a worksheet icon). A single click
  selects a view and shows its computed description (measure, categories,
  pages); a double-click opens it. Categories and Measures move below as
  collapsible sections.
- GUI: NeXT scrollers. The grid, model browser and formula list show an
  always-visible 19 px `#808080` trough with a square `#AAAAAA` knob,
  a 1 px bevel on the knob and NeXT's two arrow buttons grouped at the
  bottom of the vertical trough (one line per click).

### Added

- TUI: Shift+arrows select a range, drawn highlighted, and the status line
  shows its `Sum` (numeric cells only; `Sum —` when there are none), as the
  GUI does. A plain arrow, a click, or any re-layout collapses the range.
- **GUI screenshot regression in CI (review plan D5).** A `gui-shot` job
  (GitHub and Forgejo, `rust:1.97-trixie`) runs `scripts/gui-shot-check.sh`,
  which compares the headless 1400×900 capture against the golden
  `docs/reviews/golden/gui-1400x900.png` and fails above 200 differing pixels
  (`magick compare -metric AE -fuzz 2%`), uploading the diff. `--update`
  refreshes the golden (see CONTRIBUTING.md).
- **CLI `save-view`.** `improv save-view <db> <view-id> <name> <measure>
  [--rows Cat,Cat] [--cols Cat,...]` writes a named view the same shape the
  GUI saves (stacked rows/cols, remaining categories as pages pinned to their
  first item). `improv list` now prints a `views:` section.

- **GUI screenshot harness (`scripts/gui-shot.sh`).** Builds the binaries,
  loads a fixed Time×Product×Scenario demo model, runs `improv-gui` on a
  headless Xvfb at 1400×900 and writes `target/gui-shot/gui.png` plus
  `compare.png`, the capture beside the NeXT Improv reference. Output is
  pixel-identical run to run.
- **GUI range selection.** Shift+arrow (or Shift+hjkl), Shift+click and
  dragging across cells select a rectangle; a plain arrow or click collapses
  it. The status-bar **Sum** adds the range's numbers.
- **GUI window title.** The native title reads `Improv — <file>`, with a
  trailing `•` while edits are unsaved (an edited in-memory model, or a
  failed autosave).
- **GUI transient status.** Status-bar messages (save, recalc, undo/redo,
  added formula, errors) clear themselves after 5 seconds.

### Changed

- **GUI filtering moved to the tile menu.** The worksheet's **Filters**
  shelf is gone; right-click a category tile for a checkbox per item (uncheck
  to hide it from that worksheet) and **Show all**. A filtered tile reads
  `Name*`.
- **GUI grid field fills the table area.** The white field reaches the
  worksheet's edges instead of stopping at the last cell.
- **GUI model browser labels measures.** Each measure shows a small gray
  `input` or `formula` label instead of the `·` / `=` prefix.
- **GUI first worksheet window.** A fresh window opens clear of the
  application menu and is sized to its grid (360–900 × 300–700 px).

- **GUI grid follows NeXT Improv.** Cells are plain white with thin black
  rules (no stripes, no per-cell buttons); the selection and its row and
  column header items fill `#AAAAAA`. Input cells carry a faint inset
  underline; derived cells show `formula: …` on hover. **Double-click** (or
  Enter / F2) edits a cell; a single click only selects it.

- **GUI category tiles follow NeXT Improv.** Tiles are 17 px dark-gray chips
  with white names (no grip, no ⏷ button; right-click for
  **Move to Rows / Columns / Pages**). Page tiles sit top-left, each above a
  white item field that picks the page (replacing the `< item [i/n] >`
  stepper); column tiles sit top-right and row tiles bottom-left. Gutters size
  to their tiles, with no placeholder text, and highlight as drop targets
  only during a drag.
- **GUI worksheet windows open at 560×420** (was 480×320) so the grid and
  formula list fit.
- **GUI formulas are edited in place in the formula list.** The top formula
  bar is gone: the selected row is the editor (Enter commits, errors show
  under the row, Esc reverts), and typing `Name = formula` on the list's
  last line adds a derived measure.
- **GUI formula list lives in each worksheet window**, under the grid, with a
  `Calc` button. The operator palette and the "New derived measure" form are
  gone.
- **GUI model browser and Info panel are NeXT windows.** The docked
  explorer and inspector side panels are gone, so the dark workspace spans
  the full width above the bottom panels. The model
  browser (`<file> - Model`, bottom left; Categories, Measures, Views) has a
  close box and **View ▸ Model Browser** toggles it. The Info panel (top
  right, closed at start) opens from **Info ▸ Info Panel…** and closes from
  its close box. A fresh matrix starts right of the application menu.
- **GUI matrices are NeXT worksheet windows on a dark workspace.** The
  scrollable canvas is gone: the area between the panels is a `#555555`
  workspace, and each matrix floats on it as its own window with a black
  (key) or dark gray title bar reading `<matrix> - Worksheet - <file>`
  (`Untitled` when in memory), a close box while more than one matrix is
  open, and a bottom resize bar. Drag the title bar to move a window; clicking
  a window gives its matrix the keyboard. Window positions and sizes are saved
  in views as before, so existing views load unchanged.
- **GUI application menu replaces the tool palette and tab strip.** A
  floating NeXTSTEP vertical menu (Info, File, Edit, Format, Worksheet, View,
  Graph, Print…, Services, Hide, Quit) with cascading submenus now carries
  pivot, undo/redo, chart, CSV, save view, save model, add/remove matrix, and
  the Info panel (inspector) toggle. The View submenu lists the saved views
  that the top tab strip used to show. New key equivalents: Ctrl/Cmd+S saves
  the model, Ctrl/Cmd+H hides and Ctrl/Cmd+Q quits, next to the existing
  undo/redo chords. None of them fire while a text field has focus. A
  selected row in the explorer and formula lists now shows a white face, so
  selection no longer looks the same as hover.
- **GUI theme is NeXTSTEP grayscale.** The GUI uses NeXT's four grays
  (`#000000`, `#555555`, `#AAAAAA`, `#FFFFFF`, plus a `#808080` scroller
  trough) and nothing else: the selection is filled `#AAAAAA` instead of blue,
  corners are square, strokes are 1 px, the grid sits on white, and category
  tiles are dark gray with white text. Formula errors are marked by a 2 px
  underline rather than red, and charts use gray shades. Body text passes WCAG
  AA: black on `#AAAAAA` is 9.0:1, black on white 21:1, white on `#555555`
  7.5:1.
- **GUI text is Arimo 12.** The GUI bundles Arimo 1.33 Regular and Bold
  (OFL-1.1, `crates/gui/assets/fonts/`), metric-compatible with NeXTSTEP's
  Helvetica, as its primary proportional font; egui's fonts stay as glyph
  fallback. Body and button text are 12 pt, and headings are 12 pt bold.
- **GUI grid right-aligns numbers** so their digits line up, as Improv does;
  text, booleans and dates stay left-aligned. The cell editor for a numeric
  measure is right-aligned to match.
- **GUI item headers are plain text in a ruled band.** Row and column headers
  (`Q1`, `Widget`) were beveled button chips. They now draw as plain dark text
  with rule lines, as in Improv, so only the category tiles look draggable.

### Fixed

- **A save no longer clobbers another process's writes.** `save_model` retracts
  stored cells the model lacks, so when two processes shared a store (`improv
  set` beside a running GUI or server) the second saver deleted the first's
  new cells. A save now checks the store's transaction high-water against the
  one it loaded at and, if another writer committed since, fails with
  `StoreError::Conflict` and writes nothing. The GUI and TUI report "reload and
  retry"; the server answers `409 Conflict` and drops its cached model so a
  retry reloads. New `ModelStore::{tx_high_water, loaded_tx, set_loaded_tx,
  reload_model}`.
- Storage: a measure's category order survives save/load. `:measure/categories`
  is a set, so a reload returned the categories sorted by id, and anything
  mapped by position (SQL/CSV columns, coordinates) landed on swapped
  categories: `refresh-sql` on a 2-dimension measure wrote
  `{Year=A,Region=2025}`. The order is now stored in a new
  `:measure/category-order` attribute. Stores written before it load as
  before (id order) and gain it on their next save; no migration needed.
- TUI: the grid scrolls to keep the cursor visible on models larger than
  the terminal (previously the cursor could move off-screen).
- TUI: mouse clicks select the cell under the pointer (they were offset by
  the row-label column and selected the column to the right).
- TUI: text and boolean input cells can be edited (parsed per the measure's
  type, as `improv set` does); previously every edit had to be a number.
- TUI: editing is refused when an axis is empty or fully filtered. It used
  to store a cell with a missing dimension.
- TUI: `[` / `]` page correctly when an empty category precedes another
  page dimension.

- **CLI `--at` accepts every natural spelling of a coordinate.** `set` and
  `scenario` now take repeated flags (`--at Time=Q1 --at Product=Widget`) and
  space-separated pairs (`--at Time=Q1 Product=Widget`) as well as the
  comma-joined form, merging them. Naming a category twice is a clear error,
  and stray non-pair arguments are still rejected.
- **GUI tool-palette icons no longer render as tofu boxes.** Undo, redo,
  import/export and save-view used glyphs (`↶ ↷ ⇄ ▤`) missing from egui's
  bundled fonts; they now use glyphs those fonts contain, and a unit test
  checks every palette glyph against the loaded fonts.
- **The matrix no longer draws over the Inspector.** Gutter tiles and the page
  well escaped the canvas clip (egui's nested panels reset the clip rect) and
  painted on top of the side panels in small windows. They are now clipped to
  the canvas. The window also opens at 1280×800 (was eframe's 800×600) with a
  960×600 minimum.
- **The GUI no longer loses in-memory work silently.** File ▸ Save (Ctrl+S)
  is disabled for an in-memory model instead of reporting "saved model", and
  quitting an edited in-memory model warns first; a second Quit discards it.
- **GUI menus stay on screen.** A submenu that would run off the right edge
  cascades to the menu's left instead and is lifted to keep its bottom on
  screen, and the menu now opens on the workspace rather than over the formula
  bar's label.
- **Re-saving a model no longer piles up cell datoms (P2).** `save_model`
  diffs cells against the store: an unchanged cell is not re-asserted, a
  value left under another type's attribute is retracted, and a cell removed
  from the model (cleared in the GUI, deleted through the server) is
  retracted instead of lingering on disk and coming back on the next load.
  An unchanged re-save of a 100k-cell model now adds no datoms (it added 200
  and grew the file ~30 KB) and takes 1.3 s instead of 321 s.
- **`improv init` no longer wipes an existing store.** It installs the
  schema only; it used to save an empty model, which under the cell diff
  above would have retracted every stored cell.

## [0.7.0] - 2026-09-24

The GUI is rebuilt against Lotus Improv / Quantrix reference material, and
Improv becomes buildable by anyone who clones it.

### Added

- **Margin gutters with edge-docked category tiles.** Tiles now sit in gutters
  that *frame* the grid — column tiles along its top edge, row tiles down its
  left, page/unplaced categories in a corner well — and pivoting is dragging a
  tile from one gutter to another. The previous horizontal shelf above the table
  was structurally wrong: the references show the axes anchored to the edges they
  control. Adjacency is *constructed* from egui's panel contract rather than
  approximated, and is asserted by this codebase's first headless egui frame
  test, proven non-vacuous by feeding the old shelf's geometry in and requiring
  rejection.
- **A canvas of matrices.** A view holds several independently pivoted matrices,
  each with its own axes, filters, cursor and gutters, placed and resized on a
  scrollable canvas. This also delivers multi-measure display, previously open.
  Matrices are child `Ui`s, not `egui::Window`s, because a window is a floating
  layer that cannot scroll inside a parent scroll area.
- **Numbered formula list.** Every derived measure's formula is visible at once,
  numbered, with an enable checkbox per formula, inline error state, and
  click-to-select. Disabling is a real model operation that survives save and
  reload, not a GUI-only flag, and dependents recompute without the disabled
  formula.
- **Quoted and qualified names in formulas.** `'Unit Price'` — exactly what a
  CSV header produces — now has a spelling, with `''` for an embedded quote.
  Eight of nine previously unspellable names round-trip; only the empty name
  remains unspellable. Dotted names (`Matrix.Measure`) parse and fail with a
  clear error rather than inventing a namespace that does not exist yet.
- **Multi-matrix view schema**: `CanvasRect` and `MatrixPlacement`, with
  `View::from_matrices`/`primary`/`matrices`.
- **Per-matrix title bars, document tabs for saved views, and a selection
  aggregate readout.**
- **Undo/redo** over bounded model snapshots, and focus-aware keyboard shortcuts
  so grid keys only fire when the grid owns focus.

### Fixed

- **Strings containing newlines or tabs were silently corrupted on save.**
  `"Line\nBreak"` reloaded as `"LinenBreak"`. The escape was emitted but the EDN
  reader dropped the backslash, so any description, name, or Text cell carrying a
  newline or tab lost data with no error. Fixed on both sides: raw emission here,
  and the reader/printer fixed upstream in Mentat.
- **Models larger than ~5461 cells could not be saved** — `save_model` panicked
  inside Mentat's transactor. Saves are chunked, and the ceiling is per attribute
  queue rather than per transact.
- **Typed cell display and editing** key on the measure's declared type, never
  the text's shape, so `42` typed into a Text measure stays text.
- **Derived measures exported their empty input storage** instead of computed
  values; CSV export now evaluates through the engine.
- **`improv set` silently wrote a dimensionally invalid cell** when `--at` was
  forgotten: `improv set db 100 10 Product=A` reported success and stored a cell
  at the empty coordinate, which then evaluated to nothing. Stray arguments are
  rejected with a message naming the flag you meant, and the coordinate is
  checked against the measure's declared dimensions.
- A Unicode crash in the formula highlighter, an empty-axis divide-by-zero,
  CSV item-ID aliasing across imports, non-atomic CSV import and cell edits, a
  formula bar that offered text no parser would accept, and stale
  non-dimension categories blanking the grid.
- `rustls` 0.23.43 → 0.23.45 (RUSTSEC-2026-0285, build dependency only).

### Changed

- **Mentat is a pinned git dependency, not a sibling path dependency.** A fresh
  clone previously could not even *parse* its manifest (`failed to read
  ../mentat/Cargo.toml`), so nobody without a sibling checkout could build
  Improv; CI masked this by cloning Mentat beside the checkout in every job.
  Verified by building a bare clone with no sibling present.
- The demo scripts build the CLI once instead of invoking `cargo run` per
  command: `sample-model.sh` went from 12m31s to about 3s.
- Scale claims are stated end-to-end (build + save + reload + evaluate) rather
  than from `evaluate()` alone.


## [0.6.0] - 2026-09-07

### Added

- **`ModelStore::load_partial`**: loads only a measure set's dependency
  closure (its formula chain / external-call args), not every cell of every
  measure in the store — the out-of-core design doc's recommended first step,
  built. `engine::dataflow`/`engine::session` needed zero changes. CLI
  `eval`/`show` now use it.
- **`improv stream`**: a stdin→stdout streaming compute pipeline. Reads
  `<measure> <value> [Cat=Item,...]` edit lines from stdin, applies each
  incrementally to a live engine, and prints only the changed cells of one or
  more target measures to stdout as they land —
  `producer | improv stream model.db Revenue | consumer`.
- **CSV/TSV import/export** (`improv_storage_csv`): the baseline data on-ramp
  named in IMPROV.txt, previously missing. CLI `import-csv`/`export-csv`
  mirror `import-sql`/`export-sql`'s column-to-dimension mapping syntax; a GUI
  Import/Export wizard and a TUI `I`/`E` command prompt wire it into all three
  interfaces.
- **Hosted refresh scheduler** in `improv-server`: the same due-measure/
  refresh loop as the CLI's `serve-refresh` daemon now runs as a background
  task (`IMPROV_SCHEDULER`/`IMPROV_SCHEDULER_TICK_SECS`), with
  `GET /scheduler/status` to observe it.
- **`improv_data_source`**: shared import logic (category/measure setup,
  name→id item interning) extracted out of `storage_sql`/`storage_csv`'s
  independent duplication; every public function signature unchanged.
- **GUI formula syntax highlighting + inline error display**: the formula bar
  highlights identifiers/functions/numbers/strings/date-literals/operators and
  shows a failed parse's error position inline, via a pure
  `formula_highlight::scan`/`highlight_formula`.
- **Formula-parser fuzz target** (`fuzz/fuzz_targets/fuzz_formula_parser.rs`).
- **extfn OS-level sandbox**: subprocess runtimes (Python/R/Julia/Pure) now run
  under `bwrap` (namespace isolation) when available plus OS resource limits
  (CPU/memory/FDs/procs) always, fail-open by design; WASM was already
  sandboxed by construction (`wasmi`, no host imports).

### Fixed

- **Non-atomic model saves** (crash-safety): `save_model` transacted the
  model in up to 6 separate SQLite transactions; a crash between them left a
  partial save. Now one `InProgress`, one commit — all-or-nothing.
- **Parser panic on multi-byte UTF-8** (found by the new fuzz target /
  adversarial-input test): the tokenizer cast a raw byte to `char` and could
  walk off a UTF-8 character boundary. Now decodes real characters.
- **Parser stack overflow on deeply-nested parentheses** (a real DoS on
  attacker-controlled formula text): recursive descent now bounds nesting
  depth (`MAX_PARSE_DEPTH`) and errors cleanly instead of overflowing.
- **CI/local gate scoping**: `cargo fmt`/`cargo clippy` now target our 11
  crates explicitly; `--all`/`--workspace` could reach the vendored Mentat
  path dependency and, transitively, whatever it depends on.

## [0.5.0] - 2026-08-28

### Added

- **DuckDB** as a first-class SQL backend (bundled build; `Backend::Duckdb`,
  `ConnKind::Duckdb`).
- **Date values** in the engine's DD value lane (`CellValue::Date`), and
  **standalone/broadcast literal measures** (a bare `X = 5` scalar that
  broadcasts into other formulas).
- **What-if scenarios** as first-class model data: `Scenario` = a named overlay
  of input-cell overrides (`Model.scenarios`, `with_scenario`), deterministic;
  CLI `scenario` + `eval --scenario`.
- **External-function runtimes** R, Julia, WASM (in-process `wasmi`), and
  **Pure-lang**, alongside Python.
- **Server bearer-token authentication** (`IMPROV_API_TOKEN` /
  `IMPROV_API_TOKENS`; `/health` public; open mode when unset).
- **Virtualized large-grid rendering** in the GUI (mixed-radix on-demand row
  tuples via `body.rows` — millions of row lines never materialized), and
  **charting the full Cartesian product** of stacked axes.
- **Large-model stress tests** (`crates/engine/tests/stress.rs`): scale-eval up
  to 1M cells and an incremental-edit throughput test (`--ignored`).
- **NeXTSTEP look-and-feel for the desktop GUI**, matching the original Lotus
  Improv: a NeXTSTEP theme (gray desktop, chiseled/beveled controls, squared
  corners, paper-white grid), on-grid **category tiles at the grid margins** as
  the pivot gesture (drag a tile between Rows/Columns/Pages margins), a top
  **formula bar** (`<Measure> = <expr>`), chiseled grid headers with a
  `Rows \ Columns` corner stub, a NeXT-style **tool palette**, and
  **multi-category-per-axis stacking** (stack >1 category on an axis; the grid
  renders the Cartesian product with nested headers and group-outlined rows).
- **Refresh scheduler daemon** (`serve-refresh`): honors each measure's
  `RefreshPolicy` automatically — on every tick it refreshes the due measures
  (on-load once, interval every N seconds). The scheduling decision is a pure,
  unit-tested `core_model::schedule`; external-call measures also carry a
  policy (`define ... --refresh`).
- **SQL refresh policy + `refresh-all`**: `SqlSource` records a `RefreshPolicy`
  (manual / on-load / interval); CLI `import-sql --refresh ...` sets it and
  `refresh-all <db> [source.sqlite]` refreshes every external-sourced measure
  (CALL + SQL) at once. Policy is advisory metadata (a scheduler that honors
  timing is future work); the engine core is untouched.
- **`CALL(...)` / `SQL(...)` definition grammar**: `core_model::parser::
  parse_definition` parses a measure definition as an ordinary formula or a
  whole-RHS source form (`Target = CALL(func, args...)` /
  `Target = SQL("query")`), keeping source forms off the engine's expression
  grammar. CLI `define` / `register-ext` / `refresh-ext` make external-call
  measures usable end to end.
- **Chart view** (`improv-gui`): a Chart toggle plots the selected measure as
  grouped bars (optional line overlay), drawn with `egui::Painter` (no new
  dependency), honoring filters and page pinning.
- **External-function measures wired into the engine** (Phase 6): a measure can
  be defined as `func(arg_measures...)` (`Model.external_calls`);
  `engine::external::refresh_external_measure` evaluates the registered
  `ExternalFn` per coordinate via `improv_extfn` and writes input cells —
  host-side, so the dataflow core stays pure. `ExternalFn`/`Language` moved to
  `core_model`; defs + calls persist through Mentat.
- **Saved views + filters in the UI** (`improv-gui`, `improv-tui`): save the
  current layout as a named view, load it to reproduce measure + axis placement
  + pins + filters, and filter a category to a subset of items. GUI: Views
  panel + Filters checkboxes. TUI: `S` save, `v` cycle, `f`/`F` filter.
- **PostgreSQL backend + connection management** (Phase 7): SQL import/export/
  refresh now work over a backend abstraction (SQLite + Postgres via
  `postgres`); serde `Connection` descriptors keep credentials out of band
  (password-less DSN + `password_env`, resolved at connect time, redacted logs).
- **External-function runtime** (`improv_extfn`, Phase 6): a registry of typed,
  purity-asserted external functions evaluated in an isolated Python subprocess
  (timeout + JSON marshalling), speaking `core_model::Value` — the runtime the
  engine's `Expr::Call` will dispatch to (wiring pending). Isolated-mode +
  timeout, not yet an OS sandbox.
- **GUI keyboard grid navigation** (`improv-gui`): arrow/hjkl move a highlighted
  cell cursor; Enter/F2 edit, `[`/`]` page, `n`/`N` cycle measures; click also
  moves the cursor.
- **Saved views + filters** (`core_model::View`/`Filter`, Quantrix/Improv
  parity): a `View` captures a measure, an axis-order permutation
  (rows/columns/pages), pinned page items, and per-category filters (restrict a
  dimension to a subset). Persisted through Mentat (`:view/*` datoms), survives
  save/load. The data model both interfaces can drive; UI wiring follows.
- **GUI drag-and-drop pivoting** (`improv-gui`): an axis shelf (Rows / Columns /
  Pages) lets you drag category chips between axes (egui drag-and-drop) or use
  per-chip `→` / a Pivot button, with per-page-dimension `< >` selectors — the
  Improv/Quantrix re-pivot, no formula edits. Grid re-pivots live.
- **SQL live-query measures** (Phase 7): an imported SQL measure now records a
  refreshable `SqlSource` on the model (`improv_storage_sql::add_sql_measure` /
  `refresh_sql_measure`); CLI `refresh-sql` re-runs the stored query and
  replaces the measure's cells (persisted via a `:measure/sql-source` datom).
  SQL data re-enters as ordinary input cells, so the engine recomputes
  dependents with no SQL path of its own; these measures are marked
  nondeterministic (refresh-gated).
- **TUI pivoting + mouse** (`improv-tui`): `p` rotates categories across
  rows/columns/pages (Improv/Quantrix-style re-pivot, no formula changes), and
  left-click selects a cell (mouse capture enabled). Complements the existing
  arrow navigation, `[`/`]` paging, `e`/Enter editing, and measure cycling.
- **Nix flake**: `nix develop` dev shell with the Rust toolchain, GUI runtime
  libraries on `LD_LIBRARY_PATH` (so `improv-gui` runs), and the tooling.
- **TUI paging** (`improv-tui`): `[` / `]` cycle the pinned item of a measure's
  extra (3rd+) dimension, so multi-dimensional measures are fully viewable
  slice-by-slice; a `page i/n` indicator and a key-hint footer were added.
- **SQL connectivity** (`improv_storage_sql`, Phase 7, SQLite): import a
  `SELECT` into a new input measure (columns → categories/items/cells) and
  export a measure's cells to a SQL table. CLI `import-sql` / `export-sql`.
  SQL data enters as ordinary input cells, so the deterministic engine core is
  untouched; identifiers are validated and values bound (injection-safe).
- **Named scalar functions in formulas**: `ABS`, `ROUND`, `FLOOR`, `CEIL`,
  `SQRT`, `NEG`, `MIN2`, `MAX2` are now callable as `NAME(args...)` in formula
  text (parser registry `core_model::parser::scalar_func`, arity-checked),
  evaluated deterministically by the engine — the in-process foundation for
  Phase 6's external-language `CALL(...)` runtime.
- **Desktop GUI** (`improv_gui`, egui/eframe): `improv-gui <db>` — a model
  explorer, an editable pivot grid (input cells edit through the live engine
  with autosave; derived cells read-only), a formula editor (edit/add derived
  measures from formula text), and an inspector (metadata, dimensions,
  dependencies, formula-in-English, error-cell count). Phase 5 toolkit
  decision: egui/eframe.
- **Non-numeric value lane**: the engine dataflow now carries a tagged
  `CellValue` (`Num`/`Bool`/`Text`/`Err`), so boolean and text derived measures
  evaluate end-to-end (e.g. `Hot = Price > 15` yields a boolean). Comparisons
  and `NOT`/`AND`/`OR` produce `Bool`. CLI/TUI/server render via `CellValue`.
- **Live incremental edit API** (`improv_engine::session::Engine`): builds the
  differential-dataflow graph once and applies input-cell edits as deltas
  (`set`/`clear`/`apply`), recomputing only affected coordinates — no rebuild.
- **TUI live editing**: edit input cells in the pivot grid (`e`/Enter),
  derived measures recompute live via the session engine; autosave on quit.
- **Server** `POST /measures/:id/eval`: apply edits through the live engine and
  return the recomputed snapshot (what-if by default; `"persist": true` writes
  through).
- **Textual formula parser** (`improv_core_model::parser`): `parse_formula` /
  `parse_expr` accept Improv formula text (`Revenue = Price * Quantity`,
  precedence, comparisons, `AND`/`OR`/`NOT`, `SUM|AVG|MIN|MAX(x OVER Cat)`),
  resolving names against the model.
- **Scalar built-in functions** in the engine: `ABS`, `ROUND`, `FLOOR`, `CEIL`,
  `SQRT`, `NEG`, `MIN2`, `MAX2` (numeric); comparison/logical operators
  evaluate in the numeric lane (1.0/0.0).
- **CLI**: `add-derived <db> <id> <name> <formula>` (define a formula measure)
  and `eval <db> <measure-id>` (compute a derived measure via the engine);
  `add-measure` now takes trailing category names to declare a measure's
  dimensions. The full v1 flow — model, formula, incremental recalculation —
  now works end-to-end from the CLI.
- **TUI** (`improv_tui`): a VisiCalc-style terminal pivot viewer
  (`improv-tui <db>`) — renders a measure as a 2-D grid (categories on
  rows/columns, extra dims as pages), keyboard navigation, measure cycling,
  panic-safe terminal teardown.
- **Server** (`improv_server`): a JSON HTTP API (`improv-server <db> [addr]`)
  — `/health`, `/model`, `/measures`, `/measures/:id/values`, `/nl/parse`,
  `/nl/describe`, `/measures/:id/cells`.
- **Multi-layer derived measures**: `engine::evaluate` builds derived measures
  in topological order (cycles rejected), so a derived measure may reference
  another derived measure.
- **Tests**: property tests (proptest) for model/codec/CNL round-trips; fuzz
  targets (cargo-fuzz) for the CNL parser, model JSON, and coordinate codec;
  engine determinism suite (bit-for-bit reproducibility, insertion-order
  independence) and `#[ignore]` stress tests (100k-cell recalculation).
- **CI/CD**: GitHub + Codeberg workflows for the full quality gate,
  cross-platform build/test, release qualification, and docs publishing.
- **Core model** (`improv_core_model`): categories, items, measures,
  coordinates, dimension-aware formula AST, value/error types. JSON round-trip.
- **Storage** (`improv_storage_mentat`): persist a model as datoms on the
  embedded (SQLite) Mentat store and reconstruct it by query. Save↔load round
  trip.
- **Engine** (`improv_engine`): formula compiler (type + dimension inference,
  lowering to a Join/Aggregate/Map plan) and differential-dataflow evaluation.
  Verified against the canonical Time×Product revenue oracle.
- **CLI** (`improv_cli`): `init`, `add-category`, `add-item`, `add-measure`,
  `set`, `list`, `show`, `export` over a Mentat-backed store.
- **Natural language** (`improv_nl_formula`): controlled-English ⇄ formula
  parse/describe with a stable round trip.
- Project infrastructure: workspace, `rustfmt.toml`, `clippy.toml`, `deny.toml`,
  `.typos.toml`, CI, dual license (Apache-2.0 OR MIT), contributor guide.

[Unreleased]: https://codeberg.org/gregburd/improv
