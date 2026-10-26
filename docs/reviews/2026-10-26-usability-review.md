# H1: usability review of the TUI and the GUI

A scripted heuristic review (Nielsen's ten), run 2026-10-25 against the
tasks in the budget tutorial (`docs/src/tutorial-budget.md`), the quick
start (`docs/src/getting-started.md`), the GUI tour (`docs/src/gui-tour.md`,
"Building a model from scratch") and `improv-tui(1)`.

## Method

- **Build:** `main` at `1964289`, debug build of `improv`, `improv-tui` and
  `improv-gui`, all reporting 0.11.0 (egui 0.37).
- **CLI:** the tutorial's `sh` blocks, run as written, then deliberate slips
  (typos, reused ids, wrong argument order, names in place of ids).
- **TUI:** driven in tmux 3.7b (`send-keys`, `capture-pane`) at 110×32,
  100×24 and 80×24. Screens are kept as text in `refs/h1/tui-screens.txt`.
- **GUI:** Xvfb at 1400×900 with no window manager, driven with xdotool
  (`mousemove`, `click`, `type`, `key`) and captured with xwd and
  ImageMagick, the way `scripts/gui-shot.sh` and `docs/demo/gui-movie.sh` do
  it. Every result was checked against the store with `improv show`/`list`.
  The screenshots cited below are `gui-NN-*.png`; the ones kept are in [`refs/h1/`](refs/h1/), with the TUI screens as text in `refs/h1/tui-screens.txt`.
- **Each task was done twice:** once on the tutorial's finished `budget.db`
  (open it, change an input, look at views), and once from an empty file,
  building the tutorial's model in the front end itself.

**Limits.** One evaluator, who had read the code and docs (Nielsen
recommends three to five). A real first-time user would hit more
discoverability problems and fewer edge cases. Times are estimates for a
first-time user with the tutorial open. Under Xvfb with no window manager,
focus must be given with `xdotool windowfocus`; one early GUI edit failed
for that reason and is not counted. Not covered: screen readers and
scaling (H2), large models (H3), Postgres models, CSV import/export,
macOS/Windows.

## Tasks

### CLI (the tutorial's steps 1–6)

| Task | Done | Time, steps | Errors and confusion |
|---|---|---|---|
| Steps 1–6 as written | yes | ~3 min, 40 commands | none |
| Slip: reuse an id (`add-category 1 Foo`) | **silent damage** | 1 | renames Month to Foo; `Saved[Foo=Jan]` follows (H1-9) |
| Slip: `define` a name already used (`Left = 1`) | **silent** | 1 | a second measure named `Left` (H1-9) |
| Slip: `set` a derived measure | **accepted** | 1 | `set measure 200`; then `eval 200` panics (H1-10) |
| Slip: `eval budget.db Left` | no | 1 | `<measure-id> must be a number` (H1-25) |
| Slip: a misspelled file, `improv list budgte.db` | **silent** | 1 | creates an empty model file (H1-11) |
| `improv list budget.db \| head` | yes | 1 | panics with "Broken pipe" (H1-30) |

### TUI

| Task | Done | Time, steps | Errors and confusion |
|---|---|---|---|
| Open `budget.db`, find the Spending view | yes | <1 min, 1 key (`v`) | opens on `Left`, not a view (H1-34) |
| Change Feb/Fun from 300 to 350 | with workaround | ~1 min, 3 tries | `e` seeds `300` with the caret at its end: typing gives `300350` (H1-2) |
| Undo it | yes | 1 key (`u`) | the cursor moves up a row (H1-26) |
| Build the model from scratch (`c`, `n`, `=`) | yes | ~10 min, ~90 keys | prompts give no syntax, so you need the man page (H1-24) |
| Type the 19 input values | yes | ~3 min | `e`, value, Enter, arrow for each cell. Enter does not move on |
| Formulas with `=`, one mistyped | yes | <1 min each | located error with "did you mean Spent?". Very good |
| Save a view | with workaround | 1 key | always named "view N". No rename or delete (H1-15) |
| Quit and save | yes | `q` | an edit is lost if the terminal closes before `q` (H1-5) |
| Function help | yes | `?`, type `sum` | none |
| Slip: Enter on a derived cell, type 5 | **formula destroyed** | 3 keys | `Left = 5`, every cell blank (H1-4) |
| Slip: a value of 12345678901234 | **shown wrong** | — | the grid shows `123456789012` (H1-3) |
| Clear a cell | no | — | `not a number: ''` (H1-18) |

### GUI

| Task | Done | Time, steps | Errors and confusion |
|---|---|---|---|
| Open `budget.db`, open Spending | yes | <1 min, 1 double-click | the window opens under the application menu (H1-14, gui-02) |
| "Click a Spent cell, type a new number and press Enter" (tutorial §7) | **no** | — | typing on a selected cell does nothing. Enter then opens the editor with the old value, and the next digit is inserted into it: 300 → 3001, 3900 or 9300 (H1-1, gui-03) |
| The same with a double-click or Enter, then Ctrl+A | yes | 4 actions | none |
| Watch Left over update | yes | Add Matrix, pick Spent in the browser | the new window lands nearly on top of the old one (H1-28, gui-11) |
| Start with no file, File ▸ New, type a path | with workaround | ~1 min | typing goes nowhere until you click the field. Escape does not close it (H1-12, gui-06) |
| Add categories and items | yes | ~2 min | good: the field takes focus and clears after each Enter |
| Add `Budget` over Line, `Income` over Month | **no** | — | an input measure always covers every category (H1-6, gui-07) |
| Enter a value in a cell that has one | **wrong value** | — | 1500 becomes 15001501 (H1-1, gui-04) |
| Add a formula, mistyped first | yes | ~1 min | the error is placed well. After Enter, letters go to the grid: `n` switched the measure (H1-8, gui-08, gui-09) |
| Pivot by dragging a tile, filter by right-click | yes | 1 drag | good (gui-12) |
| Save a view | yes | name field and button | Worksheet ▸ Save View names it "view 4". Views cannot be renamed or deleted (H1-15) |
| Delete… with a view row selected | wrong target | — | acts on the measure picked earlier (H1-15, gui-16) |
| Type `1,500` | refused | — | the editor closes and the typing is lost (H1-16, gui-10) |
| Edit a derived cell | refused, with the reason | — | "derived cells are computed, not editable". Good |
| Function help (F1) | yes | F1, type `sum` | good (gui-13) |
| Work in memory, then keep it | **no** | — | Save is disabled and there is no Save As (H1-7, gui-15) |

## Findings, ranked by severity

Nielsen's scale: 4 = catastrophe, fix before release; 3 = major; 2 = minor;
1 = cosmetic. Each finding is written to become one plan task: the
road-to-1.0 plan's H1 row lists them, and severity 3 and 4 are its exit
criterion 5.

### 4: catastrophe

**H1-1 GUI: typing into a cell does not replace its value.** *Heuristics:
match with the real world; error prevention.* Typing on a selected cell
does nothing, with no feedback. Enter, F2 or a double-click opens the
editor holding the old value with nothing selected. The caret sits at the
end, or wherever that cell's last edit left it, so the typed digits are
inserted into the old number. Following tutorial §7 to the letter stored
3001. Other runs stored 3900 and 9300 for a typed 9, and a Budget cell of
1500 became 15001501. The narrow cell shows the old value clipped ("30"),
so the user cannot see it is still there. The wrong number is autosaved
at once. **Fix:** a printable
key on a selected input cell opens the editor with that key as its whole
text, as every spreadsheet does. Enter, F2 and a double-click open it with
all the text selected, using the `CCursorRange::two` that `rename_line`
already uses. **Files:** `crates/gui/src/app.rs` (`handle_grid_keys`,
`begin_edit_cursor`, the cell `TextEdit` in `render_grid`).

### 3: major

**[fixed: `fix(tui): typing on a cell replaces its value (H1-2)`] H1-2 TUI: `e` on a cell appends to the old value.** *Match with the
real world; error prevention.* `edit> 300` with the caret at the end, so
typing `350` gives `300350`. **Fix:** a digit, `-` or `.` in normal mode
starts an edit holding only that key. `e`/Enter keep the old text, but the
first printable key replaces it unless the user moved the caret first.
**Files:** `crates/tui/src/main.rs` (`normal_key`), `crates/tui/src/app.rs`
(`begin_edit`), `crates/tui/src/edit.rs`.

**H1-3 TUI: numbers wider than 12 characters are cut off without a
mark.** *Visibility of system status.* 12345678901234 shows as
`123456789012`, a hundredth of its value. -1234567590.125 shows as
`-1234567590.`. Only the status line shows the true value. **Fix:** size
each column to its widest value, as the GUI does, up to a cap. Past the
cap, show `###` or round the decimals to fit; never drop leading digits.
**Files:** `crates/tui/src/ui.rs` (`COL_W`, the grid widths),
`crates/tui/src/app.rs` (`Grid::display_at`).

**H1-4 TUI: Enter on a derived cell edits the formula, and a number
replaces it.** *Error prevention; consistency.* A user who wants to
override one `Left` cell presses Enter and types `5`. `Left = 5` is
stored, a scalar, and every cell of the grid goes blank with no message.
`u` undoes it, but nothing hints at what happened. The GUI refuses the same
edit ("derived cells are computed, not editable"). **Fix:** match the GUI:
Enter on a derived cell says so, and `e` (named in the message) edits the
formula. When an edit would drop all of a formula's categories (a
constant), ask for a second Enter. **Files:** `crates/tui/src/app.rs`
(`begin_edit`, `commit_edit`).

**H1-5 TUI: edits are held in memory until `w` or `q`, with no unsaved
marker.** *Visibility of system status; error prevention.* An edit
followed by closing the terminal (tmux `kill-server`, which sends SIGHUP)
was lost. Ctrl+C and Ctrl+Q do nothing, and nothing on screen says the
model has unsaved changes. The GUI autosaves every edit. **Fix:** autosave
after each change, as the GUI does (`App::save` already writes only what
changed). At the least, show "modified" in the status title and save on
SIGHUP/SIGTERM. **Files:** `crates/tui/src/main.rs` (`run`, `quit`),
`crates/tui/src/app.rs` (`save`, `record_history`).

**H1-6 GUI: an input measure always ranges over every category.**
*Flexibility; match with the real world.* Worksheet ▸ Add Input Measure
gives `Budget` both Month and Line. The tutorial's Budget (per line) and
Income (per month) cannot be built, and dimensions cannot be changed
later. Typing the TUI's form, `Spent: Month, Line`, created a measure
named `Spent: Month, Line`. **Fix:** accept `Name: Category, Category` as
the TUI does (`run_add_input`), refuse `:` and `,` in names, and let the
Info panel change an input measure's categories. **Files:**
`crates/gui/src/app.rs` (`add_named`, `NewThing::Input`),
`crates/tui/src/app.rs` (`run_add_input`, to share), `docs/src/gui-tour.md`.

**H1-7 GUI: a model built in memory cannot be saved.** *User control and
freedom; error prevention.* Started with no file (or File ▸ New with an
empty path), the title reads `Untitled •`. File ▸ Save is disabled and
Save As is not built, so an hour's work can only be discarded. Quit does
warn once. **Fix:** Save on an in-memory model asks for a path (the New
panel), writes the model there and switches to it. Build Save As the same
way. **Files:** `crates/gui/src/app.rs` (`command_enabled`, `file_panel`,
`switch_model`), `crates/gui/src/menu.rs`.

**H1-8 GUI: after a formula error, the keyboard goes to the grid.**
*Error recovery; consistency.* Enter on `Pct = Spnt / Budget` shows the
error, placed well, but the line loses focus. The next letters drive the
grid: `n` switched the worksheet from Income to Saved, and `h j k l` move
the cursor. A formula row being edited does the same. The TUI keeps the
edit open. **Fix:** when a commit fails, request focus for the same field
again, caret at the error's span. **Files:** `crates/gui/src/app.rs`
(`new_formula_line`, `add_from_formula_line`, the row editor in
`formula_list`).

**[fixed: `fix(cli,engine): the usability review's CLI findings (H1-9, H1-10, H1-11, H1-30)`] H1-9 CLI: an id or name already in use silently replaces or
duplicates.** *Error prevention.* `add-category 1 Foo` renamed Month to
Foo throughout. `add-item 10 1 January` renamed Jan. `add-item 10 1 Feb`
left two items named Feb. `add-measure 101 Spent2 …` renamed Spent, and
its view followed. `define 300 "Left = 1"` added a second `Left`. Each
prints "added …". **Fix:** refuse an id or a name already in use, with the
existing object named, and add a `--replace` flag for the deliberate case.
**Files:** `crates/cli/src/main.rs` (`cmd_add_category`, `cmd_add_item`,
`cmd_add_measure`, `cmd_define`).

**[fixed: `fix(cli,engine): the usability review's CLI findings (H1-9, H1-10, H1-11, H1-30)`] H1-10 CLI: `set` writes into a derived measure, and `eval` then
panics.** *Error prevention.* `improv set budget.db 200 5 --at …` prints
`set measure 200`. Every later `eval 200` dies with `panicked at
crates/engine/src/dataflow.rs:1387: a session per input measure`, even
after the formula is redefined. The GUI and TUI do not show the stray
cell. **Fix:** `cmd_set` refuses a derived measure ("Left is a formula;
set its inputs"). The engine skips stored cells of a derived measure
instead of `expect`, and `load` drops or reports them. **Files:**
`crates/cli/src/main.rs` (`cmd_set`), `crates/engine/src/dataflow.rs`.

### 2: minor

**[fixed: `fix(cli,engine): the usability review's CLI findings (H1-9, H1-10, H1-11, H1-30)`] H1-11 All: a misspelled model path creates an empty model.** *Error
prevention.* `improv list budgte.db` and `improv eval budgte.db 200`
create the file. `improv-gui typo.db` creates it at launch, before any
edit. `improv-tui budgte.db` opens empty and `q` saves it. **Fix:** only
`init`, File ▸ New and an explicit flag create a model. Otherwise print
"no model at budgte.db (improv init creates one)". The TUI may keep
opening a new file empty, but should say "new model, not saved yet" and
write nothing until the first save. **Files:** `crates/store/src/lib.rs`
(`Store::open`), `crates/cli/src/main.rs` (`open`), `crates/tui/src/main.rs`
(`load`), `crates/gui/src/app.rs` (`ImprovApp::load`), and the man-page
source `crates/tui/src/args.rs`.

**H1-12 GUI: the New and Open panels do not take the keyboard.**
*Consistency.* The path typed right after File ▸ New went nowhere. You
must click the field, and Escape leaves the panel open. **Fix:** request
focus for the path field when the panel opens, and close on Escape.
**Files:** `crates/gui/src/app.rs` (`file_panel`).

**H1-13 GUI: the empty-model hint is outdated and hidden.** *Help.* "No
measures. Open a model store with `improv-gui <db>`." is drawn at (0,0)
under the menu (gui-05) and names the command line, though File ▸ New
and Worksheet ▸ Add Category exist. **Fix:** draw it right of the menu,
and say "Empty model: Worksheet ▸ Add Category to begin, or File ▸
Open…". **Files:** `crates/gui/src/app.rs` (`canvas_panel`).

**H1-14 GUI: a view saved by `improv save-view` opens under the menu.**
*Aesthetic; flexibility.* The CLI writes `CanvasRect::default()` (x 0,
y 0), which the GUI restores as it is, so the menu covers the row labels
and the first column (gui-02). **Fix:** `save-view` writes the GUI's
fresh-window origin (`MENU_CLEAR`), or `Matrix::from_placement` moves a
rect that overlaps the menu. **Files:** `crates/cli/src/main.rs`
(`cmd_save_view`), `crates/core_model/src/lib.rs` (`CanvasRect`),
`crates/gui/src/app.rs` (`from_placement`).

**H1-15 TUI, GUI: views cannot be named (TUI), renamed or deleted.**
*User control; consistency.* TUI `S` always saves "view N".
Worksheet ▸ Save View does the same. Neither front end can rename or
delete a view. With a view row selected in the GUI browser, Delete…
acted on the measure picked earlier ("cannot delete Spent", gui-16),
because `Thing` has no view case. **Fix:** a name prompt for `S`
(default "view N") and for Save View. Add `Thing::View`, so that Rename
and Delete… act on the selected view. Add a TUI command for both.
**Files:** `crates/tui/src/main.rs` (`save_current_view`),
`crates/gui/src/app.rs` (`Thing`, `edit_target`, `Command::SaveView`).

**H1-16 GUI: a refused entry throws away what was typed.** *Error
recovery.* `1,500` closes the editor with "edit error: not a number"
(gui-10), and the text is gone. The TUI keeps it open for correction.
**Fix:** keep the editor open with the text and show the message.
**Files:** `crates/gui/src/app.rs` (the commit after `commit_cell_text` in
`render_grid`).

**H1-17 TUI: formulas show and edit as controlled English.**
*Consistency.* `Left = Budget - Spent`, typed with `=`, comes back as
`Left = Budget minus Spent`, and `e` edits that text. The GUI and CLI show
the symbolic form. **Fix:** show and seed the symbolic text whenever it
spells the formula exactly, as the GUI's `formula_source` does, and keep
English as the fallback or a toggle. **Files:** `crates/tui/src/app.rs`
(`formula_text`, `begin_edit`), `crates/gui/src/app.rs` (`formula_source`,
to share).

**H1-18 TUI: a cell cannot be cleared.** *User control.* An empty edit
gives `not a number: ''`. The GUI clears the cell. **Fix:** an empty
commit clears it, as the GUI's `commit_cell_text` does. **Files:**
`crates/tui/src/app.rs` (`commit_edit`).

**H1-19 TUI: a category may take a measure's name.** *Error prevention.*
`c Spent: a` created a category `Spent` next to the measure. The GUI
refuses the same ("already a category, measure or LAMBDA"). **Fix:** the
GUI's check, in `run_add_category` and `run_add_input`. **Files:**
`crates/tui/src/app.rs`.

**H1-20 TUI: `R` drops the session's edits and undo history without
asking.** *User control.* One key, no confirmation, and afterwards
"nothing to undo". **Fix:** when there are unsaved edits, ask for a
second `R`, as `q` does after a failed save. **Files:**
`crates/tui/src/main.rs` (`revert`).

**H1-21 TUI: no rename or delete of categories, items or measures.**
*User control.* A misspelled item can only be fixed in the GUI. **Fix:**
port the GUI's rename and delete, with the same blockers and undo.
**Files:** `crates/tui/src/app.rs`; `crates/gui/src/app.rs` (`delete`,
`delete_blockers`, `rename`), to share through `core_model`.

**H1-22 GUI, TUI: no number format.** *Match with the real world.* A
derived cell shows `299.666666666667`, and the Format menu is grayed
out (gui-14). A budget wants two decimals or none. **Fix:** a
per-measure decimal places setting, stored with the measure and used by
the GUI, TUI and `eval`. **Files:** `crates/core_model/src/lib.rs`,
`crates/gui/src/menu.rs`, `crates/tui/src/ui.rs`.

**H1-23 GUI: Help has only Functions….** *Help and documentation.* Help
has no way to the guide, the tutorial or the keyboard list. **Fix:**
Help ▸ Keys (the tour's "Keyboard" section) and Help ▸ User Guide (open
the published URL). **Files:** `crates/gui/src/menu.rs`,
`crates/gui/src/app.rs`.

**H1-24 TUI: the `c`, `n` and `=` prompts show no syntax.**
*Recognition rather than recall.* `category>` and `input>` give no hint
that `Month: Jan, Feb, Mar` or `Spent: Line, Month` is wanted. The forms
are only in the man page. **Fix:** a dim example after an empty prompt
(`category> Name: item, item, …`). **Files:** `crates/tui/src/ui.rs`
(prompt strings).

**H1-25 CLI: measures are named by id only.** *Recognition rather than
recall.* `improv eval budget.db Left` fails with "must be a number", so
you need `improv list` to find 200. **Fix:** `<measure>` takes an id or a
name (one lookup, `measure_by_name`). **Files:** `crates/cli/src/main.rs`
(the value parsers and the commands).

### 1: cosmetic

**H1-26 TUI:** after `u`, the cursor moves one row up (Fun → Utilities).
Keep the cursor cell. `crates/tui/src/app.rs` (`travel`, `relayout`).

**H1-27 GUI, TUI:** both refuse `1,500`, and the TUI refuses `$1500`
(not tried in the GUI). Accept the locale's group separator and a leading
currency sign for number measures, in each crate's `parse_typed`.

**H1-28 GUI:** Add Matrix opens the new window 28 px from the old one, on
the same measure, which reads as one window. Place it beside the old
window if it fits. `crates/gui/src/app.rs` (`add_matrix`, `CASCADE`).

**H1-29 Docs:** tutorial §7 asks for File ▸ Save after an edit. The GUI
has already autosaved, so say so. §7 also needs the H1-1 fix, or should
say "double-click, select all, type". `docs/src/tutorial-budget.md`.

**[fixed: `fix(cli,engine): the usability review's CLI findings (H1-9, H1-10, H1-11, H1-30)`] H1-30 CLI:** `improv list … | head` panics "failed printing to stdout:
Broken pipe". Exit quietly on EPIPE. `crates/cli/src/main.rs` (`main`).

**H1-31 CLI:** `add-measure db 103 X number Line` (no `input`) answers
"only 'input' measures are supported, got 'Line'". Say "expected
`input` before the categories". `crates/cli/src/main.rs:336`.

**H1-32 TUI:** the key list takes 3 of 32 rows at 110 columns and 5 of 24
at 80. Show it on `?` and keep one line ("? keys") in the status block.
`crates/tui/src/ui.rs`.

**H1-33 GUI:** input and derived cells look almost the same. The tour's
"faint underline" on input cells is the only cue, and it is hard to see at
1:1 (compare gui-01 with gui-02). Use a clearer cue, such as a gray fill
or italic text for derived cells. `crates/gui/src/app.rs`
(`render_grid`), `crates/gui/src/theme.rs`.

**H1-34 GUI, TUI:** both open on the first derived measure, not the first
saved view. Open the first view when there is one.
`crates/tui/src/app.rs` (`measure_order`), `crates/gui/src/app.rs`
(`pick_default_measure`).

## Nielsen's ten, per front end

| Heuristic | TUI | GUI |
|---|---|---|
| 1 Visibility of system status | status line names measure, axes, cell and Sum; no unsaved mark (H1-5); cut-off numbers (H1-3) | title `•`, status line, Sum; the Changed panel (H9) |
| 2 Match with the real world | type-to-replace missing (H1-2); no number format (H1-22) | type-to-replace missing (H1-1); no number format (H1-22) |
| 3 User control and freedom | `u`/Ctrl+R undo; `R` without confirmation (H1-20); no rename/delete (H1-21) | undo/redo; in-memory work can't be kept (H1-7); views (H1-15) |
| 4 Consistency and standards | formulas in English, not as typed (H1-17); derived Enter differs from the GUI (H1-4) | panels without focus or Escape (H1-12); Delete… target (H1-15) |
| 5 Error prevention | derived cell becomes a constant (H1-4); name clash (H1-19) | input measure dimensions (H1-6); typo path creates (H1-11) |
| 6 Recognition rather than recall | prompts without syntax (H1-24) | menus, tiles and the browser are all visible; good |
| 7 Flexibility and efficiency | `c Month: Jan, Feb, Mar` is fast; Enter does not move down | drag pivots, key equivalents, autocomplete |
| 8 Aesthetic and minimalist design | key list always shown (H1-32) | NeXT look is clean; windows under the menu (H1-14, H1-13) |
| 9 Help users recover from errors | located errors, `^~~~`, "did you mean" | located errors, but focus lost (H1-8), text lost (H1-16) |
| 10 Help and documentation | `?` key list and function search | F1 function help; no guide or keys in Help (H1-23) |

## What works well (keep)

- **Located formula errors with a hint**, in both front ends: the token
  underlined, `unknown measure: Spnt`, `did you mean Spent?`. The best
  thing found in this review.
- **Function help** (F1 / `?`): search by name or description, signatures
  with parameter names.
- **Live recalculation across worksheets**: an edit to Spent changed the
  Left over window at once (gui-11).
- **GUI pivots and tiles**: a drag stacks an axis cleanly (gui-12). The
  tile menu moves, filters and adds items.
- **GUI model building**: Add Category and Add Item focus the field, clear
  after Enter, and take the next name. The GUI refuses edits to derived
  cells with the reason. Delete… names what still depends on a measure.
- **TUI one-line structure**: `Month: Jan, Feb, Mar` and `Spent: Line,
  Month` (the GUI should take the same, H1-6). A refused TUI entry keeps
  the edit open with the reason.
- **Autosave and undo in the GUI**: every change was in the store when
  checked, and Ctrl+Z took it out again.
- **CLI `--at` errors** name what is missing (`missing Line`).
