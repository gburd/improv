# The Web GUI

The web GUI is the desktop GUI's NeXT worksheet in a browser. egui draws
it into a WebGL canvas, from the desktop's own theme, window, menu and
formula-highlighting code. The model, the engine and the store stay in
`improv-server`: the browser draws what the server sends and sends back
what you type, and it computes nothing. A derived cell is always the
server's answer, which is what lets the server hide what a user may not
see (see [Access](#access)).

![The web GUI in Chromium: the main menu and three worksheet windows, Price, Quantity and Revenue, each with its Pages, Columns and Rows gutters, and the status bar reading ready, live](./img/web-worksheet.png)

*Three worksheets in the browser after logging in; the status bar says
`live` while the change stream is up.*

<video src="./img/web-demo.mp4" controls muted loop playsinline width="100%"
  poster="./img/web-demo.png">
  <img src="./img/web-demo.gif" alt="The web GUI: a token login, pivoting the Time tile and back, an edit in one browser reaching another with the server's new Revenue, a formula refused with its error located, undo and the Model Browser">
</video>

*Two browsers on one improv-server: a token login, a pivot and back, an
edit in one browser that the other shows at once with the server's new
Revenue, a misspelled formula refused with the error located, an undo,
and the Model Browser. A caption under the picture says what each step
does.*

## Serve it

Build it once (it needs the `wasm32-unknown-unknown` Rust target and
`wasm-bindgen-cli` at the version in `Cargo.lock`; `build.sh` says which
when it is missing), then serve it beside the API:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked
crates/gui_web/build.sh --release web        # index.html, the .js and the .wasm, in ./web
IMPROV_API_TOKEN_FILE=/etc/improv/tokens improv-server sales.db 127.0.0.1:3000 --static ./web
```

Open `http://127.0.0.1:3000/`. The page and the API share one origin, so
no CORS is needed. For anything past your own machine, use TLS: the token
travels on every request. Give the server `--tls-cert` and `--tls-key`, or
put a proxy in front ([The HTTP Server](./server.md#tls)).

## Log in

A server with tokens asks for one. Type it in the login panel. The page
holds it in memory only, never in the URL, a cookie or the browser's
storage, and sends it as `Authorization: Bearer`. Nothing rides on a
cookie, so another site cannot make your browser act for you (CSRF). A
reload asks for the token again. A read-only token
(`IMPROV_API_TOKENS_RO`) shows the model and refuses edits. A server in
open mode (no tokens, loopback only) logs in at once.

![The Improv - Log In panel: a masked access token half typed, the Log In button, and the line "This server needs an access token."](./img/web-login.png)

## What it does

- **Worksheets.** Each measure in its own window, NeXT-style: the title
  bar, close box and resize bar. The first view, or else the first three
  measures, open at start. Windows move and resize as on the desktop.
- **Pivoting.** Each worksheet has Pages, Columns and Rows gutters, with
  a tile for each category. Drag a tile to another gutter to pivot, or
  right-click it (or Tab to it and press Enter) for Move to…. Several
  categories may stack on one axis.
  A page category's tile has a list to pick the page item from, and
  every item of a page category can be picked.
- **Cells.** Double-click an input cell to edit it, then Enter writes it
  (Esc cancels). From the keyboard: Tab to the grid, move with the
  arrows, Home and End (Ctrl+Home, Ctrl+End: the corners), and press
  Enter or start typing to edit. A value is read by the measure's type: numbers as the
  desktop reads them (`$1,500`), text, `true`/`false`, dates
  (`2025-01-31`). An empty cell clears it. A refused edit says why, and
  the cell shows the server's value again.
- **Formulas.** The key window (the one with the black title bar) has the
  formula pane: every formula, as typed. Click one to edit it, then Enter
  saves. The `Name = formula` line adds a formula (or a LAMBDA). Both
  editors autocomplete measures, categories and functions (Up and Down
  pick, Tab accepts, Esc closes). A formula the server refuses is
  underlined where it went wrong, with the message and a hint, as on the
  desktop.
- **Menus.** The desktop's main menu, Improv 1.0's (see [the desktop
  GUI](./gui-tour.md#the-pivot-grid)), with the commands the web GUI
  has; the rest draw grayed. Edit has Undo and Redo (Ctrl+Z,
  Ctrl+Shift+Z), Add Category, Add Item, Add Input Measure, Rename and
  Delete…. Format has the decimals and thousands separators. Worksheet
  has Save View. View has New View (another window on the same measure),
  Close Window and Swap Rows/Columns, then the saved views. Help ▸ User
  Guide opens this guide. **File ▸ Model Browser…**, or the status bar's
  **Model Browser** button, lists measures, categories and items, to
  open, rename, add and delete them.
- **Undo** is per window session: each page load is a session, and its
  Undo undoes its own changes, not another user's.
- **Live updates.** The page reads the server's change stream
  (`GET /v1/changes`). An edit in another browser, the CLI or the desktop
  GUI shows within a fraction of a second (about 0.1 s measured on one
  machine), with the cells it recalculated. The status bar says `live`
  while the stream is up. When the connection drops, the page retries
  with backoff and resumes from the last version it saw, so nothing is
  missed. A structural change (a new category, a renamed measure) makes
  it re-read the model.

![Two browser sessions side by side: in session A, 250 typed into Quantity 2025Q2 Widget; in session B, the same 250 and Revenue 3000, recalculated by the server](./img/web-live.png)

*Session A types 250 into Quantity; session B, another login, shows it
and the server's new Revenue (12 × 250) without reloading.*

![The Model Browser window at the right, listing the measures and the categories with their items, each with rename, add-item and delete buttons](./img/web-browser.png)

*The Model Browser, opened from the status bar's **Model Browser**
button.*

The desktop GUI runs the same worksheets for a server URL
(`improv-gui https://…`; see [the desktop GUI](./gui-tour.md#a-model-on-a-server)),
from the same code (`crates/gui/src/served.rs`).

## What it does not do yet

The CSV wizard, graphs, the Info panel, the Inspector and the Format
panel, printing, context help, function help (F1), scenarios, item
filters, and range selection with Copy and Paste are desktop-only. A model with more
than about 200,000 cells on one page needs paging the grid does not do
yet.

## Access

Every request goes through the server's token check, then through the
model's [access policies](./policies.md), for each measure, item and cell
it touches, with the token's name as the Cedar user (see [The HTTP
Server](./server.md#access-policies)). A hidden measure, item or cell is
absent from what the page receives, and an edit of a cell the user may
not change is refused. Totals stay computed over everything, by the
server. What the user may not do is greyed out, from the server's
`GET /v1/capabilities`: the menu commands, the Model Browser's buttons,
the formula lines and the input cells. They are hints; the server checks
every request.

## Accessibility

The web GUI works with the keyboard and a screen reader. eframe's web
runner does not pass egui's accessibility tree to the browser (as of
eframe 0.37), so the web GUI does it itself: it keeps hidden elements
inside the canvas in step with the tree, with ARIA roles, names and
states. They are never drawn. (The design note is
`docs/reviews/2026-11-08-web-accessibility.md`.)

What works:

- **The grid** is an ARIA grid. A screen reader announces each cell with
  its row and column headers and its value ("2025Q1, Widget: 100"), and
  can say its row and column position. A computed cell is read-only. Tab
  reaches the grid at its cursor cell. The arrows, Home and End move it,
  and Enter or typing edits the cell. Enter commits, Esc cancels, and
  the cell has the focus again.
- **Tab and Shift+Tab** go through everything in order: each window's
  title and close box, its tiles and page lists, its grid, the formulas
  and the `Name = formula` line, the main menu's rows, the Model
  Browser, and the status bar. The formula line keeps Tab only while its
  completion list is open (Tab accepts).
- **Menus.** Enter on a menu row opens its submenu or runs it, and so
  does a screen reader's "activate". A tile's Enter opens its Move to
  menu.
- **Focus.** The screen reader follows the keyboard focus. When the
  screen reader moves onto an element (browse mode, a rotor, a click),
  the app's focus moves there too.
- **Announcements.** The status bar is a live region: "ready", "not
  saved: …" and other status messages are read when they change. A
  formula error is an alert.
- **Names.** Every control has one: the tiles say their axis, the
  Model Browser's buttons say what they act on, and the login and
  prompt fields are labeled.

What does not:

- **Reading a field back.** While a text field has the keyboard, the
  screen reader echoes what is typed, but it cannot read back or review
  the whole field: the browser's input only holds the text being typed
  (eframe's input agent).
- **Positions on screen.** The hidden elements have no layout, so a
  magnifier or a focus highlight that follows them shows the whole
  canvas.
- **Rows scrolled out of view** are left out of the grid (the grid's
  row count still says how many there are), except the cursor's row.
  Move the cursor to reach them.
- **Not yet tried with Orca, NVDA, JAWS or VoiceOver.** The checks read
  Chromium's accessibility tree, which is what a screen reader reads.
  The desktop GUI's screen-reader runs are still open too (road-to-1.0
  H2). The look has one text size, as on the desktop, and browser zoom
  scales the whole canvas.

## Testing it

`scripts/web-e2e.sh` runs it all: it builds the web GUI, `improv` and
`improv-server`, runs the end-to-end check below, and compares the first
frame with the golden, `docs/reviews/golden/web-1280x713.png` (`--update`
replaces it). `crates/gui_web/e2e/e2e.sh` builds a model with the CLI, serves it and a
web build with `improv-server`, and drives two browser sessions in
headless Chromium over the DevTools protocol (`drive.py`). It edits a
cell in one and checks the other shows it, recalculated, within a second.
It restarts the server mid-session and checks the other session catches
up, then pivots by dragging a tile and types a misspelled formula to
check where the error lands. A third session reads Chromium's
accessibility tree (DevTools `Accessibility.getFullAXTree`): the grid's
cells carry their headers and values, Tab reaches the tiles, cells,
formulas and menus, a screen reader's click on a menu row runs it, and
the keyboard edits a cell. The app publishes its visible cells, tiles
and formula error on `<body data-…>`, which is how a test reads a canvas.
