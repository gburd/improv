# Changelog

All notable changes to Improv are documented here.
Format: [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
this project follows [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- **Linux packages and a Homebrew formula** on every release, from the next
  one on (`ci/packages.sh`, nfpm; `ci/homebrew-formula.sh`): `improv` as a
  `.deb` and `.rpm`, pg_improv per PostgreSQL major under PGDG's names and
  paths (`postgresql-17-improv`, `postgresql17-improv`), and `improv.rb`.
  The release installs each `.deb`/`.rpm` in a clean Debian and Fedora
  container and runs it before attaching it.
- **The GUI recalculates in the background** (H3). After a new or edited
  formula, a rename, a new category or item, an import, the undo or redo
  of any of these, Calc, Revert to Saved, or opening a model, the engine
  is rebuilt on a thread of its own. Until then the grid shows the last
  results and the status bar says "Recalculating…". A cell edit made
  meanwhile is applied to the new engine when it is ready; a second
  structural change builds again. At 1M cells, a new formula held the
  window for 1.6–2.6 s (and its undo and redo for 1.8–2.0 s); the longest
  frame is now 15–26 ms. A change that leaves the cells alone no longer
  copies or diffs them for its undo step and its save (0.6 s at 1M).
  An engine that fails to build now says so in the status bar.
- **A container image**, `ghcr.io/gburd/improv`, from the next release on:
  `improv-server` and the `improv` CLI (`Containerfile`), as an
  unprivileged user, refusing to start without a token. The release job
  checks that, `/health` and a 401 without a token, before it pushes.

### Fixed

- **Typing on a TUI cell replaces its value.** `e` or Enter showed the old
  value with the caret at its end, so typing `350` over `300` stored
  `300350`. The old value now shows selected and the first key typed
  replaces it; an arrow, Home, End or Backspace first edits it in place. A
  digit, `-` or `.` in the grid starts an edit holding just that key
  (usability review H1-2).
- **The TUI grid never cuts a number short.** A column was 12 characters,
  so 12345678901234 showed as `123456789012`, a hundredth of its value.
  Each column now widens to its widest value, up to 20 characters; past
  that a number drops decimals to fit, and one that still cannot fit shows
  `###` (H1-3).
- **Enter on a TUI formula cell no longer turns the formula into a
  number.** Enter opened the formula, so Enter, `5`, Enter stored
  `Left = 5` and blanked the grid without a word. Enter on a derived cell
  now says "derived cells are computed, not editable; press e to edit the
  formula", as the GUI does; `e` edits it. A formula edit that leaves the
  measure one value over no categories asks for a second Enter, and when
  one is accepted the measure takes the formula's shape, so the grid shows
  the value instead of blank cells (H1-4).
- **The TUI saves each change as it is made.** Edits were held in memory
  until `w` or `q`, with nothing on screen to say so, and closing the
  terminal lost them. Each change (a cell, a formula, a category, an
  undo) is now written at once with the incremental save `w` uses, as the
  GUI does. A failed save puts "not saved" in the status box's title and
  the reason on the status line; `w` retries and `q` still refuses to drop
  the edits (H1-5).
- **An empty TUI edit clears the cell**, as in the GUI; it was refused
  with "not a number: ''" (H1-18).
- **The TUI's prompts show their syntax.** `category>`, `input>`, `define>`
  and the CSV prompts gave no hint of the form they take; an empty prompt
  now shows it dimmed, as in `category> Name: item, item, …` (H1-24).
- **`improv set` refuses a formula measure, and a stored cell of one no
  longer panics the engine.** `set` on a derived measure was accepted and
  saved, and every later `eval` of the model panicked ("a session per input
  measure"). `set` now says the measure is computed by its formula, and
  `evaluate` and the live engine ignore such a cell in an existing file
  (usability review H1-10).
- **The CLI's `add-category`, `add-item` and `add-measure` refuse a taken id
  or name, and `define`/`add-derived` a name another measure or a category
  has.** A reused id silently renamed or replaced what had it
  (`add-category 1 Foo` renamed Month), `define 300 "Left = 1"` made a
  second `Left`, and a category could share a measure's name (H1-9).
  Redefining a measure under its own id still works.
- **A mistyped model path is an error**, not a new empty model that the
  command then ran against: every command but `init`, `import-csv` and
  `import-sql` needs the file to exist (H1-11).
- **`improv … | head` exits quietly** (status 141) instead of panicking with
  "Broken pipe" (H1-30).
- **`improv-server` stops on SIGTERM.** It had no handler: run as a
  container's PID 1, where SIGTERM's default is to ignore it, `docker stop`
  waited its 10 s and then killed it. SIGTERM and Ctrl-C now end it at once.
### Changed

- **A formula may be 256 levels deep** (was 40; C3b). The limit
  (`MAX_FORMULA_DEPTH`) is now the load bound (`MAX_STORED_FORMULA_DEPTH`),
  so a 45-term sum is fine as a new formula too, and a formula at the limit
  saves, exports, reloads and evaluates in every store and on every thread
  Improv runs on, a 1 MiB Windows main thread included. The parser's nesting
  bound is 320 (was 64, and 100 for stored text), the controlled-English
  parser's too (was 200). Past 256: "formula nests too deeply (257 levels;
  the limit is 256)", and a stored formula past it still loads as `#REF!`
  (JSON) or names its measure (Postgres text).
- **Model and formula JSON reads past serde_json's 128 levels** (C3b).
  `improv_core_model::json` reads up to 784 levels (a whole model holding a
  formula 256 deep) and writes at any depth, on a stack that grows
  (`serde_stacker`); the Mentat and Postgres stores, `improv export` and the
  server's `/nl/describe` use it. A stored formula past the load bound (256
  levels) still loads as `#REF!`. The parser, the compiler, named-LAMBDA
  expansion, the scalar evaluator and the controlled-English parser and
  describer grow the stack where the caller's thread is nearly used up
  (`stacker`), and engine threads have 16 MiB (was 8).
- **`evaluate` runs on several timely workers** for `improv eval`, the
  server's derived-measure reads, DuckDB's `improv()` and pg_improv's
  trigger and commit modes: one per 10,000 input cells, at most 8 and the
  cores (`IMPROV_ENGINE_WORKERS` overrides it; in Postgres, at most
  `max_parallel_workers_per_gather` + 1). Release build, 48-core box,
  before → after (8 workers): `tests/stress.rs` 1M cells 0.88 → 0.22 s,
  2M 1.80 → 0.45 s, `SUM` over 1M cells 1.30 → 0.28 s; DuckDB `improv()`
  over 2.5M distinct cells 4.55 → 4.0 s (its `evaluate` 0.75 → 0.35 s;
  the rest is building the model). Peak memory rises 15–20 % (each worker
  arranges its share and buffers exchanges). Results are the same, bit for
  bit, on any count (a reduction still sees a whole group, sorted, on one
  worker); a FIXPOINT model and the live `Engine` stay on one worker,
  since workers make single edits slower.
- **Windowed reductions cost O(n) per group, not O(n²).**
  `SUM(X OVER W[FIRST..THIS])` at 5,000 items went from 1.12 s to 6 ms
  per edit, and at 20,000 items from 18.1 s to 25 ms (evaluate 17.9 s to
  32 ms); `SUM(X OVER W[THIS-1..THIS])` at 20,000 items from 228 to 8 ms
  (release build, `tests/probe.rs` `wide`). Each row's window is found
  with two pointers, and `SUM` … `XOR` slide a running state over the
  rows: an exact sum that terms leave as well as join, a monotone deque
  for `MIN` and `MAX`, counts for `AND`/`OR`/`XOR`. Results are the old
  kernel's bit for bit (a property test holds them to it). A window
  bound past the largest offset now reaches the last item instead of
  overflowing.
- **`Engine::apply_delta` returns what an edit round changed**: per
  measure, each changed cell's coordinate, old and new value, consolidated
  (`session::Changes`). `Engine::snapshot` returns the current snapshot.
  A caller that held the previous `Snapshot` across an edit made the
  engine copy each changed measure whole: at 1M cells of
  `Revenue = Price * Quantity` an edit took 40 ms with the old snapshot
  held and 0.06 ms through `apply_delta` (release build, `tests/probe.rs`
  `grid`). The GUI drops its snapshot before each cell edit: its
  incremental recalc went from 52 to 0.8 ms per edit and an undo from 67
  to 15 ms at 1M cells (`h3_scale_probe`). pg_improv's logical worker
  applies batches through `apply_delta` and writes the changed rows
  straight from it, instead of holding the last snapshot and re-diffing
  every changed measure.
- **Reduction rows are stored inline** (B8). A reduction's row (its item
  positions and values, as the dataflow's `reduce` arranges it) held two
  `Vec`s; it now holds them inline for up to four reduced categories and
  one value, the common shape, with no allocation. PageRank at 100k
  nodes / 10M edges: engine heap 556 -> 501 bytes per edge, peak RSS
  8.8 -> 7.9 GB, `Engine::new` 43 -> 34 s; at 10k / 1M, 611 -> 555
  (`Out = SUM(W OVER Dst)` alone 264 -> 208). `RANK(X OVER W)` at 1M items
  evaluates in 1.8 s (was 2.5) and its edits take about 20% less; a
  one-cell edit under `SUM(X OVER W)` at 1M items 0.65 -> 0.49 ms
  (release build, `tests/probe.rs` `pagerank` and `wide`; RANK's edits
  vary 2x run to run on a shared box). Every result is unchanged.
- **An edit costs what it reaches, not the number of measures** (B6). The
  live engine groups measures into components by what they read and
  advances only the input sessions of the components an edit feeds; the
  rest of the dataflow sees no progress. An edit that reaches 1 of 20,000
  measures went from 44.7 to 0.18 ms (at 5,000: 8.5 to 0.03 ms; release
  build, `tests/probe.rs` `measures`, split). A model that is one big
  component (20,000 measures off one input) is unchanged, about 130 ms.

### Security

- **No text result is longer than 32,767 characters** (Excel's cell
  limit): longer is `#VALUE!`. `REPT` and `TEXTJOIN` had the limit;
  `SUBSTITUTE`, `&` and the other text functions did not, so a short
  formula could square a string's length per step:
  `SUBSTITUTE(REPT("x", 30000), "x", REPT("y", 30000))` is 900 million
  characters. Through `LANGUAGE improv`, any role that may create a
  function could exhaust a Postgres backend's memory with it (C3 review,
  `docs/reviews/2026-10-25-security-review.md`).

## [0.11.0] - 2026-10-25

The release that makes Improv safe to share and fast at scale. In
Postgres, every model now reads and writes as its owner, under
row-level security on its catalog; one replication slot serves a whole
database. Mentat 1.18.0 makes saving a large model linear (a 1M-row
import takes about 105 s; it did not finish in 15 minutes before), and a
one-cell save writes one cell (8 s → 4–8 ms at 1M cells). The desktop
GUI and the TUI build models from scratch, rename and delete, undo, and
offer a reload when another program saves; the GUI moves to egui 0.37
and passes keyboard-only and AccessKit checks. SUM and AVERAGE are
exactly rounded. Formulas have named arguments, exponent literals and a
depth limit that keeps every saved model loadable. Releases are signed,
with checksums and SBOMs. The release qualification, against 0.10.0 on
the same benchmark, is in
[docs/reviews/2026-10-25-v0.11.0-qualification.md](docs/reviews/2026-10-25-v0.11.0-qualification.md).

### Added

- **Incremental saves** (B12). `improv_store::Store::save_changes(model,
  &ChangeSet)` writes only what an edit changed: the cells it names (each
  read by an index seek and upserted, or removed), and, if
  `ChangeSet::structure` is set, the categories, items, measures, views,
  LAMBDAs and meta, without a pass over every stored cell. It runs under
  the same high-water conflict check as `save_model`, so a concurrent
  writer is still refused, never overwritten. `ChangeSet::between(old,
  new)` derives the set in memory. The same API is in both backends
  (`ModelStore` / `PgModelStore::save_changes` and `save_cells`).
  `Store::take_save_token` / `adopt_save_token` move a save token
  without copying its Postgres snapshot. On a 1M-cell Mentat store a
  one-cell save takes 4–8 ms (it took 8.1 s), and on a 1M-cell Postgres
  model 2–11 ms (1.9 s).

- **Tutorials, tested in CI** (guide, new "Tutorials" section). "A
  household budget on the desktop" builds a model with the CLI and opens
  it in `improv-gui`. The page is the test: `docs/tutorials/mdtest.py`
  runs its `sh` blocks in order and checks each `text` block against the
  output (`crates/cli/tests/tutorials.rs`, part of `cargo test`).
  "A live sales model in Postgres" binds an orders table, defines four
  formulas, switches to logical sync and watches inserts and deletes
  arrive; `crates/pg_improv/tests/tutorial-sales.sh` runs its `sql`
  blocks through `psql` on a scratch cluster configured from the page,
  in `ci/pg_improv.sh` on every Postgres major. "Analyzing a Parquet
  file in DuckDB" writes a Parquet file from DuckDB and models it with
  `improv(source, model)`; `crates/duckdb_improv/test/tutorial.sh` runs
  it under both DuckDB CLIs in `ci/duckdb_improv.sh`.
- **A generated pg_improv SQL reference** (`docs/src/pg-reference.md`):
  every function in schema `improv` with its signature, every `improv.*`
  GUC with its type, default, range and context, and the columns of
  `improv.status` and `improv.models`, read from a live extension by
  `crates/pg_improv/tests/pg-reference.py`. `ci/pg_improv.sh` runs it on
  every major and fails when the page is stale, or when a function, GUC
  or column has no description (`UPDATE_DOCS=1` rewrites the page).
- **Generated CLI and HTTP references** (`docs/src/cli-reference.md`,
  `docs/src/http-reference.md`): every command, argument, flag, default
  and environment variable of `improv`, `improv-gui`, `improv-tui` and
  `improv-server` from their clap definitions, and every operation of
  `docs/openapi.json` with its scope, parameters, body, responses and
  schemas. `cargo test -p improv_cli` fails when either is stale, as for
  the man pages.
- **Build a model from scratch in the TUI.** `improv-tui new.db` opens
  a new (or empty) model instead of refusing it, and new keys build it:
  `c` adds a category and its items (`Time: 2025, 2026`, again to add
  more items), `n` a number input measure over the categories you list
  (`Quantity: Time, Product`), and `w` saves without quitting. `=` opens
  a `define>` line that takes what `improv define` takes: `Revenue =
  Price * Quantity` adds a derived measure (or gives an existing one a
  new formula), and `Margin = LAMBDA(p, q, p - q)` defines a named
  LAMBDA. Its errors are located and completed like a formula edit's.
  `?` now lists the keys above the function search. Every change undoes.
- **TUI undo and redo.** `u` undoes the last change to the model (a
  cell, a formula, an import) and Ctrl+R redoes it, up to 50 steps, as
  the GUI's Edit ▸ Undo does: the history keeps whole-model snapshots.
  Saved views are layout and stay.
  A failed import is no longer half-applied: it leaves the model as it
  was.
- **Build a model from scratch in the GUI.** **File ▸ New** (Ctrl+N)
  starts an empty model at a new store file, a `postgres://` URL, or in
  memory, and **File ▸ Open…** (Ctrl+O) opens one, both from a small
  panel; neither was built before. **Worksheet ▸ Add Category**, **Add
  Item** and **Add Input Measure** open the model browser at a new
  "New category" / "New item" / "New input measure" line, which each of
  its lists now ends with. A category's tile also takes a new item: its
  right-click menu ends in a New item line. Each add autosaves and is one
  undo step; a name already used by a category, measure or LAMBDA is
  refused. A new input measure is a number over every category.
- **`improv-server --static DIR` and `IMPROV_CORS_ORIGINS`.** For a web
  front end on the API: `--static` serves a directory's files at `/`
  beside the API (an `index.html` for `/`, `.wasm` typed, nothing outside
  the directory); `IMPROV_CORS_ORIGINS` lists the browser origins allowed
  to call it cross-origin (off by default; `*` is refused). The HTTP
  Server page documents both, and polling `/v1/measures` as the way to see
  other writers until a change feed exists.
- **Accessibility checks (H2).** Tests dump the GUI's AccessKit tree and
  require a name on every Tab stop (573, with every window open); drive
  the grid and the menus with key events only; compute the theme's WCAG
  contrast ratios; and check that the TUI shows nothing by color alone.
  Fixed on the way: menu rows, window title bars and close boxes, grid
  cells, tiles, view and function rows and 15 text fields had no
  accessible name (a cell now reads "2025, WidgetA: 100"); scroller
  arrows and resize bars were empty Tab stops; Enter on a grid cell
  opened its editor and committed it at once, so a cell could not be
  edited from the keyboard; and the TUI's cursor and range were shown by
  color only (now reversed and underlined too), which `NO_COLOR` hid.
- **The TUI keeps your edits when a save fails (H6).** `q` saves first;
  when that fails, from a full disk or because another program saved
  the model, the TUI stays open with the reason, and a second `q` quits
  without saving. A full disk now says so and that the edits are still
  there, and a conflict says to press `R`. Tests kill a TUI mid-save
  (`kill -9`) and reopen without loss or corruption, fill a 1 MB tmpfs
  under a save, and run two writers on one store.
- **The GUI and TUI notice other writers (H9).** The GUI checks the
  store's high-water every two seconds; when another program has saved,
  a **Changed** panel offers **Reload** (Revert to Saved) or **Not Now**,
  which hides it until the next outside save. The TUI checks on each key
  press and says once per outside save that `R` reloads. Nothing is
  overwritten meanwhile: a save from the stale window is refused as a
  conflict, as before.
- **The GUI says it is importing.** A CSV import and its save block the
  window (8 s at 1M cells); the status line now reads "importing and
  saving…" with a wait cursor before the work starts, instead of the
  window freezing with no sign.
- **Rename and delete in the GUI.** Click a category, item or measure in
  the model browser to pick it. **Worksheet ▸ Rename** turns its row into
  a line with the name selected (double-clicking an item or a measure
  does the same), and Enter renames it. Formulas follow the new name, and
  the clash rules are the ones adding uses. **Worksheet ▸ Delete…** asks
  in a panel what goes with it (an input measure's or an item's cells, a
  category's items), then deletes. A measure a formula reads, or a
  category a measure ranges over or names, is refused with a message
  naming them, as `improv.drop_measure` refuses. Saved views lose a
  deleted measure's worksheets and a deleted item's filters and pages.
  Each autosaves and is one undo step.
- **Named LAMBDAs from the GUI's formula list.** `Name = LAMBDA(p, …,
  body)` on the last line defines a named LAMBDA, parsed by
  `parse_definition` and stored as `improv define` stores it; an existing
  name is redefined under its id, so its callers recompute. A redefinition
  that breaks a caller is refused. Each is autosaved and one undo step, and
  the model browser lists them.

- **`improv.status` shows each logical model's memory, and
  `improv.max_model_memory` caps it.** Two new columns, `fold_bytes` (the
  fold state of the model's aggregated bindings) and `engine_bytes` (its
  live engine and input cells), estimate the heap the model holds in the
  sync worker, as of its last apply. They come from the sizes of what the
  worker holds and match the live heap within about 10 %. The new GUC
  `improv.max_model_memory` (MB, reloadable, 0 = no cap, the default) refuses
  a model whose live state would exceed it: the worker logs why, sets the
  model's `last_error`, drops its live state and retries it with backoff,
  instead of growing until the backend is OOM-killed. New in
  `improv_engine`: `Engine::estimated_bytes`; in `improv_pg_model::live`:
  `Mapper::fold_bytes` and `inputs_bytes`. The 0.10.0 → 0.11.0 update adds
  the columns.
- **`TEXTSPLIT` takes Excel's `match_mode` and `pad_with`:**
  `TEXTSPLIT(Code, "x", "", FALSE, 1 INTO Part)` splits on `x` or `X`
  (match_mode 0 is exact, the default; any other value is `#VALUE!`).
  `pad_with` is accepted; it pads the short rows of a 2-D split, and a
  row delimiter is still `#VALUE!` (now a documented difference from
  Excel, with the reason), so it changes nothing yet. Both were argument
  count errors before. Goldens from Excel's documentation; LibreOffice
  25.2 and Gnumeric have no `TEXTSPLIT`.
- **`""` inside a text literal is one `"`**, as in Excel: `"say ""hi"""`
  is the text `say "hi"`. It was a parse error, so no formula changes
  meaning. The GUI formula bar prints such text the same way, so a formula
  with a `"` in a literal shows as DSL instead of controlled English.
- **Named arguments in formulas:** `PMT(0.05, 12, Loan, type => 1)`,
  `XLOOKUP(Id, Key OVER Row, Name, search_mode => -1)`,
  `FIXPOINT(0, LAMBDA(r, r / 2 + 1), max_iter => 50)`. After the positional
  arguments, `name => value` sets a parameter by its Function Reference
  name (case-insensitive), each once and not one already given by
  position. A skipped optional parameter takes its documented default
  (the reference now writes it: `[fv=0]`, `[if_not_found=NA()]`,
  `[lower_tail=TRUE]`); one without a default cannot be skipped. Functions
  with a repeating parameter (`SUM`, `LINEST`, `IFS`) take arguments by
  position only. A misspelt name suggests the right one. The formula is
  stored and shown as the positional call, so saved models are unchanged.
  The GUI and TUI editors offer `name => ` for the call's remaining
  parameters, and signature help follows a named argument. New in
  `improv_core_model::functions`: `FnDef::param_index`, `param_name`,
  `param_default`, `closest_param`, and `complete_named`; `call_at` maps a
  named argument to its parameter.
- **`improv.commit_capture = 'rows'`: commit mode recomputes only what a
  transaction touched.** Commit mode recomputed the written tables'
  dependents in full at every `COMMIT` (2.1 s at 2.5M rows). Set the GUC
  to `rows` before `improv.set_sync_mode(model, 'commit')` and entering
  the mode installs statement triggers that capture the touched
  coordinates, so `COMMIT` recomputes only those: a 1,000-row INSERT
  commits in 145 ms, ten one-row INSERT statements in 112 ms. The
  triggers cost about 30% on a 1M-row INSERT. A write the capture did not
  see, a `TRUNCATE`, partitioned bound tables and aggregating formulas
  fall back to the full recompute. The default, `relations`, is
  unchanged: no triggers on your tables.

- **pg_improv: the resource limits left in D7.** If `max_worker_processes`
  leaves no room for the sync worker, `improv.status` says the worker is
  not running and names the setting. `wal_keep_size` turns out not to
  limit the slot. On disk full while writing results, logical mode reports
  the error in `last_error`, leaves nothing half-written, and keeps the
  worker and the other models going. The new `tests/limits.sh` checks all
  three, and `ci/pg_improv.sh` runs it.
- **pg_improv: the full privilege matrix**, in the README's Privileges
  and summarized in the guide's Postgres page. It covers superuser,
  `improv_admin`, owner, `SET` member, `improv_user`, writer and `PUBLIC`
  against every API action and read, and `tests/privileges.sh` checks each
  cell.
- **pg_improv: `improv.models`**, a view listing every model with its
  owner, sync mode and result schema, plus `may_change`, which says
  whether the caller may act as its owner. It follows the catalog's read
  policy.
- **pg_improv: the `improv_user` and `improv_admin` roles.** `CREATE
  EXTENSION` (and the update to 0.11.0) makes them when missing.
  `improv_user` is what a non-superuser needs to build models: `USAGE` on
  schema `improv`, `EXECUTE` on the API, and the catalog DML that
  row-level security confines to its own models. Result schemas are now
  made by the extension, owned by the model's owner, so a builder needs no
  `CREATE` on the database. `improv_admin` grants and revokes
  `improv_user`. It does not act on other roles' models, because that
  would mean acting as their owners; only a superuser does. `DROP
  EXTENSION` keeps both roles. `drop_model` and `drop_measure` drop result
  tables as the model's owner, so a member of the owner with `SET` but not
  `INHERIT` may drop them too.
- **Release sums, signatures and SBOMs.** Every GitHub Release carries
  `SHA256SUMS` over all its files, `SHA256SUMS.sigstore.json` (a cosign
  keyless signature from the release workflow's GitHub OIDC identity,
  logged in Rekor), and a CycloneDX SBOM per binary, target and extension.
  `ci/release-attest.sh` makes them, and runs locally without the signing.
  Verifying a release is in the guide's Running page and in
  CONTRIBUTING.md.
- **improv-server: token scopes, a token file, limits, an audit log, `/v1`
  and OpenAPI.** Each token has a name and a read or write scope; the
  read routes take either, `POST /cells` and `/eval` with
  `"persist": true` need write. `IMPROV_API_TOKEN_FILE` reads
  `name:scope:secret` lines, so secrets need not sit in the environment;
  the existing variables still work (`IMPROV_API_TOKEN` is a write token).
  `IMPROV_MAX_BODY_BYTES` sets the body limit (default 1 MiB), and
  `IMPROV_RATE_LIMIT` a per-token (or per-address) limit, 600 a minute by
  default, answering 429 with `Retry-After`. Every write, allowed or
  refused, is logged as a JSON line (who, route, model, status) to stderr or
  `IMPROV_AUDIT_LOG`. Routes are served under `/v1`, the old paths kept as
  aliases; `/v1/openapi.json` serves `docs/openapi.json`, which a test
  keeps in step with the router. The guide's server page gains Audit log
  and Deployment (TLS through Caddy or nginx) sections.
- **What 1.x promises:** [Compatibility](docs/src/compatibility.md), a new
  Reference page. For each surface (formulas, the pg_improv functions,
  GUCs, result tables and `improv.status`, model files, the CLI's arguments
  and exit codes, the `/v1` HTTP API, the DuckDB functions and version
  floor) it says what is stable, what a 1.x minor may add, and how a
  deprecation works. The pg_improv catalog tables are internal, and the
  Rust crates are not a stable API in 1.x (J2 first).
- **pg_improv's upgrade test covers every release on every major.**
  `tests/upgrade.sh` checks that each released version (0.8.0, 0.8.1,
  0.9.0, 0.10.0) has a chain of update scripts to this one and that
  `ALTER EXTENSION pg_improv UPDATE` finds a path from each. Then, on the
  Postgres major it runs on (CI: 15, 16, 17 and 18; 0.8.x shipped for 17
  only), it updates a database from each release, with one model in each
  sync mode that release had (logical, trigger, manual, and commit from
  0.10.0), and checks every model's results, the 0.11.0 owner and
  row-level security, the worker, `define`, `pg_dump` and commit mode.
  Each release's build is cached (`PGI_OLD_CACHE`, default
  `~/.cache/pgi-old/<version>/<major>`, and an `actions/cache` in CI), so
  the whole matrix takes under a minute per major once warm. The test now
  runs Postgres from a private copy of the install, so it never writes the
  system's extension directories.
- **A weekly scale regression gate** (`.github/workflows/scale-gate.yml`,
  also run by hand). It runs the v0.10.0 benchmark's probes at sizes that
  fit a GitHub runner: the engine's wide, measures and pagerank probes,
  `stress.rs` at 100k cells, `improv import-csv` of 100k cells into
  Mentat, and pg_improv's `tests/scale.sh`. It fails when a probe is more
  than 25% slower than `docs/reviews/bench/baseline.json`. Each time is
  taken relative to a reference workload (`reference.rs`, no Improv code)
  measured in the same run, so a slow or fast runner does not move the
  result. `docs/reviews/bench/scale-gate.py` runs and checks the probes,
  and writes a new baseline.

- **Concepts, operations and troubleshooting guides.** The guide gains
  Concepts (the ideas before the syntax), When to Use Improv (each wrong
  fit, what to use instead, and the measured limits), and an Operations
  part: Monitoring (`improv.status` column by column, the `improv_db`
  slot, the worker's log lines, alert rules, the HTTP server), Capacity
  Planning (sizing formulas and a worked 2.5M-row example), Backup and
  Restore and Operations runbooks (each procedure tested on PG 18), and
  Troubleshooting (every formula error message, CLI errors, "why is this
  slow"). `docs/ops/` ships a postgres_exporter queries file and a
  Grafana dashboard. The guide's contents are regrouped into Guides,
  Operations, Reference and Design.
- **Exponent literals in formulas:** `1e-10`, `2.5E+3`, `6.02e23`. Before,
  `2e3` was a parse error and FIXPOINT's tolerance had to be written
  `0.0000000001`. An exponent without digits (`1e`, `1e+`) is a located
  error; one that overflows a 64-bit float (`1e999`) is the error `number
  out of range` (not infinity, and not Excel's `#NUM!`), and one that
  underflows reads as 0. Names are unaffected, since a name never starts
  with a digit: `e`, `E` and `e1` are still measures. The GUI formula bar
  prints such numbers back in exponent form (below 1e-6 and from 1e21), so
  they stay editable, the controlled English reads and writes them
  (`price minus 1e-10`, and now `2.5`, which split at the point before),
  DuckDB's `improv_calc` and pg_improv's `plimprov` accept them, and
  `improv_core_model::parser::number_literal` is the shared printer.
- **Differences from Excel**, a section of the formula guide
  (`formulas.md`): every place the language departs from Excel on purpose
  (`==` for equality, `NOT` and `^` precedence, no coercion, case-sensitive
  `==`, dates as timestamps on LibreOffice's serial numbering, missing
  cells, `#N/A` for Excel's `#CALC!`, lists `INTO` a category, variadic
  `FREQUENCY` bins, `TEXTSPLIT`, no volatile functions), with what Excel
  does and why. Five that look accidental are planned as follow-ups
  (road-to-1.0 plan, E5).
- **The formula language's stability policy for 1.x**
  ([Language stability](https://gburd.github.io/improv/formulas.html#language-stability)): a 1.x
  release may add functions, optional trailing arguments, and syntax that
  is a parse error today. It will not change the result, result type or
  categories of a valid formula (except a documented bug fix, listed under
  Fixed), a function's name or argument order, precedence, or the stored
  function ids. A deprecated form keeps working, warns in the editors, and
  is removed only in 2.0.
- **A man page for every command and surface, generated from the
  parsers.** `improv(1)`, one page per command in the git style
  (`improv-import-csv(1)`, `improv-set(1)`, … 23 in all),
  `improv-gui(1)`, `improv-tui(1)` and `improv-server(1)` are built from
  the clap definitions and the hand-written sections in `crates/cli/man/`;
  `improv-functions(7)` from the function registry, the same source as the
  guide's Function Reference. New hand-written pages: `improv-formulas(7)`
  (the language), `pg_improv(7)` (the SQL API and settings) and
  `improv-files(5)` (the model file, the CSV import format, the DuckDB
  model text). `cargo test -p improv_cli` fails when `docs/man` is stale
  or `groff -man -ww` warns; `UPDATE_DOCS=1 cargo test -p improv_cli`
  regenerates it. The release archive installs them as `man/man{1,5,7}`.

- **Shell completions** for `improv`, `improv-gui`, `improv-tui` and
  `improv-server`, in bash, zsh and fish, generated from the same parsers
  into `docs/completions/` (checked for drift the same way) and shipped in
  the release archive's `completions/`.
- **pg_improv records each model's owner.** `improv.model.owner`
  (`regrole`) is the role that called `create_model`. The update from
  0.10.0 fills it from the owner of each model's result schema, which
  `create_model` made as the caller; a model without one gets the
  bootstrap superuser and a NOTICE.
- **`crates/pg_improv/tests/privileges.sh`** (in `ci/pg_improv.sh`): a
  bound table with a row-level security policy that shows the owner half
  its rows gives results over that half in trigger, commit, logical and
  manual + `recalc` modes, whoever writes the table.

### Changed

- **The GUI runs on egui and eframe 0.37** (was 0.29), the newest release
  (H4). The NeXTSTEP look is kept; text renders a little differently, the
  formula enable boxes draw filled, and the empty formula line's hint is
  gray. Ctrl+Q again goes through Quit's check for an unsaved in-memory
  model (egui 0.37 would close the window by itself). `cargo deny` allows
  egui's Ubuntu font licence and no longer needs three advisory ignores.
- **FIXPOINT holds 34% less memory per edge, and a warm re-solve is
  31% faster** (B8, in part). At 10M edges (PageRank, 100k nodes) the
  live engine went from 845 to 556 bytes per edge, peak RSS from 13.3 to
  10.1 GB, and the re-solve after one edge's weight changes from 9.7 to
  6.6 s. The FIXPOINT host evaluator keeps a reduction's rows in flat
  vectors (it held two vectors per row), and a measure's cell order
  stays put across an edit, so the row layouts still fit. A sum over a
  large category whose groups are sparse (`SUM(W OVER Dst)` of an edge
  list, a few cells per node) reduces in one level: its blocks held a
  row or two each and cost two arrangements. With exact sums the result
  is the same bits either way. The 500 bytes per edge and 2× re-solve
  targets are not met yet; what is left is in the road-to-1.0 plan, B8.
- **A category growing past 1,024 items no longer rebuilds** (B4(b)).
  Over more than 1,024 items the simple reductions reduce in blocks, and
  the live engine (`Engine::serves`, so also pg_improv's logical worker)
  treated the crossing as a change of plan and rebuilt the model. With
  exact sums, the blocked `SUM`, `AVERAGE`, `MIN`, `MAX`, the counts and
  `AND`/`OR`/`XOR` give the flat results bit for bit, so new items keep
  arriving as data. `PRODUCT` and `SUMSQ`, whose blocked rounding
  differs, still rebuild once at the crossing. `MIN` and `MAX` now order
  `-0` below `0` (they took either, by position).
- **`SUM` and `AVERAGE` are exactly rounded.** They add every value
  with no rounding (a fixed-point superaccumulator) and round the total
  once to the nearest double, in every evaluation path: the flat reduce,
  the two-level reduce of a category of more than 1,024 items, the live
  engine and the FIXPOINT host step. A flat and a blocked sum of the same
  values are now bit-identical whatever the grouping; before, Neumaier
  compensation was the exact sum's nearest double except under extreme
  cancellation. No golden moved (the 10,000-value goldens were already
  exact). `PRODUCT` and `SUMSQ` are unchanged. The cost (none
  measurable on the 1M-item `SUM` probe) is in the guide's Formula
  Language page.
- **Saves write what changed** (B12). `improv set` and `stream` save only
  their cells; `add-*`, `define`, `register-ext`, `scenario` and
  `save-view` save structure without re-reading the cells. On the 1M-cell
  store `improv set` takes 5.3 s, all of it the load (it took 13.4 s; the
  save is now milliseconds). The GUI's autosave after a cell edit, an
  undo, a formula or a new category writes only what the edit changed: a
  cell edit on the 1M-cell store takes 64 ms in total, of which the save is
  16 ms (it took 8.0 s on the UI thread). A failed autosave's changes are
  kept and written by the next save. The TUI's saves (`w`, saving a view,
  the quit autosave) write the difference from the last save. Opening a
  Mentat store no longer re-transacts an installed schema: 0.21 s and
  490 MB less per open of a 1M-cell store, which every GUI and TUI save
  paid. Imports and refreshes still do the full save.

- **Mentat 1.18.0** (was 1.16.0). Saving a large model is linear again:
  identity upserts and lookup-refs no longer scan the whole datom view per
  value. `import-csv` of 1M rows takes about 105 s; under 1.16.0 it did not
  finish in 15 min. At 200k rows it went from 99.5 s to 16.1 s, and a re-save
  from 185 s to 21 s. Reads are unchanged, and stores written by 1.16.0 open
  as they are (`docs/reviews/2026-10-23-mentat-1.18-upgrade.md`).

- **DuckDB `improv()` over 2.5M distinct coordinates: 11 s and 2.34 GB is
  now 4.6 s and 1.47 GB** (DuckDB v1.5.6; v2.0: 4.7 s, 1.49 GB). The
  results are no longer turned into a row of owned strings per cell at
  bind time: they stay in the engine's form, and each output chunk writes
  its 2,048 rows from them. The cells are sorted with a key per
  comparison, not one allocated per cell. The loaded inputs move into the
  model instead of being copied, so they are never held twice. New in
  `improv_duckdb_model`: `cells` and `Cells` (a range at a time);
  `run` still returns every row.
- **The engine holds 63% less memory per cell, and evaluates 3× faster
  (B7).** A measure that joins measures of different categories
  (`Revenue = Price * Quantity`) is no longer collapsed by a `reduce`
  afterwards, nor is a single-result reduction, which saved two copies of
  every cell; the broadcast joins once, not twice; and coordinate keys stay
  inline. At 5M cells: the live engine 732 → 269 bytes per cell,
  `evaluate`'s peak RSS 3.38 → 2.35 GB, `evaluate` 13.5 → 4.5 s,
  `Engine::new` 14.2 → 6.0 s. Results are unchanged. The heap profile and
  the out-of-core decision (deferred past 1.0) are in
  `docs/reviews/2026-10-23-engine-heap-profile.md`.
- **improv-server: an unknown token gets 401, not 403.** 403 now means the
  token is valid but its scope is too narrow. A 401 carries
  `WWW-Authenticate: Bearer` and a JSON error body.
- **pg_improv trigger mode is 3x faster per statement, and 20x with an
  index.** A trigger-mode statement read each bound table once per
  binding, serially: pgrx ran the scoped load read-write, which kept
  Postgres from using parallel workers. It now finds the touched rows in
  one read-only (parallel) scan per table, reads them back by `ctid` for
  every binding, and reads only the touched coordinates' result rows.
  Integer, text, `bool`, `numeric` and `uuid` dimension columns compare
  as themselves, so an index on the bound dimension columns serves the
  scan. At 2.5M rows (PG 18): a one-row INSERT takes 105 ms (was 310),
  ten in one transaction 900 ms (was 2.9 s), a 1,000-row INSERT 159 ms
  (was 370); with an index on the dimension columns, 15 ms, 50 ms and
  79 ms. The guide's Postgres page says when to add the index.

- **The logical worker folds text `min`/`max` under the column's
  collation.** Before, every change to a text `min` or `max` binding
  reloaded the model. The fold now orders texts by the value column's
  collation: bytes for `C` and `POSIX`, Postgres's own compare
  (`varstr_cmp`) for any other deterministic one. A column with a
  nondeterministic collation, where only Postgres can tell which texts
  are equal, reloads on a change to its `min`, `max` or `count_distinct`
  (the rebuild reason is `nondeterministic collation`). Before,
  `count_distinct` compared bytes there and could count `Apple` and
  `apple` twice under a case-insensitive collation.
- **Float sums in the logical worker are exact and order-free.** A
  `float4`/`float8` column's `sum` and `avg` keep the floats' exact sum
  (a fixed-point accumulator) and round once, so the result no longer
  depends on the order the changes arrived in. A fresh `sum` adds floats
  in scan order and can still differ in the last bits; the guide's
  Postgres page documents this. An `avg` of a `numeric` column keeps its
  scale after deletes.
- **pg_improv: `what_if` is for those who may act as the model's owner,
  and computes as the owner.** It read as its caller, so a caller who saw
  other rows than the owner got deltas that came from what they could see,
  not from their edits. Once items became owner-only it would have
  returned nothing. Other roles now get `permission denied for improv
  model "m": what_if needs to act as its owner, alice (see Privileges)`.
  It then reads the bound and result tables as the owner, as `recalc`
  does. To give a role `what_if`, the owner either grants membership
  (`GRANT alice TO analyst WITH INHERIT FALSE, SET TRUE` on PG 16+, `GRANT
  alice TO analyst` on 15), which also lets the role change the owner's
  models, or creates a `SECURITY DEFINER` wrapper for one model and grants
  `EXECUTE` on it, which gives `what_if` on that model and nothing else.
  Either way the role sees results computed from what the owner can read.
  The pg_improv README's Privileges section has the wrapper's text.

- **`improv` parses its arguments with clap.** Every invocation that worked
  still works, with the same output (a test, `crates/cli/tests/compat.rs`,
  pins them). What changed: `--help` and `improv help <command>` are clap's,
  with a page per command; `-V`/`--version` print `improv <version>` as
  before. **A usage error (unknown command, missing or malformed argument)
  now exits 2, not 1**, and prints the usage line after the message; the
  message itself is unchanged (`missing argument <db>`, `<measure-id> must
  be a number, got 'x'`, …). Errors from the model itself still exit 1.
  `--at=Time=Q1` and `--` before an argument now work as clap allows.
  **An argument a command does not take is an error** (exit 2): the hand
  parser ignored it, so `improv eval m.db 102 --scenrio high` evaluated
  without the scenario and `improv init m.db extra` succeeded.
- **`improv-gui`, `improv-tui` and `improv-server` parse their arguments
  with clap**, and gain `--help` and `--version`. Their arguments are
  unchanged (`improv-server [model] [addr]`, `IMPROV_ADDR` when `addr` is
  not given). An extra argument is a usage error (exit 2): `improv-server`
  panicked on one, and `improv-tui` with no model now exits 2 (it exited
  1).
- **Adding items to a wide category is linear, not quadratic.**
  `Model::add_item` and `ItemInterner::register` (CSV/SQL import, every
  pg_improv load, DuckDB `improv()`) scanned the category's item list once
  per item. 300k items took 12.5 s through `add_item` and now take
  40 ms (1M items: 140 s, now 0.15 s); DuckDB `improv()` over one
  300k-item category went from 38 s to 1.6 s.
- **Defining many measures is linear, not quadratic.**
  `Model::measure_by_name` and `category_by_name` scanned every measure
  (category) per lookup; they now go through a name → id index the model
  builds on first use (never saved). Parsing 20k chained definitions took
  558 ms and now takes 25 ms.
- **One edit under `SUM`/`AVERAGE`/`MIN`/`MAX`/`COUNT` … over a large
  category is incremental.** Over more than 1,024 items those reductions
  (and `PRODUCT`, `SUMSQ`, `AND`, `OR`, `XOR`, `COUNTA`) reduce in two
  levels, by blocks of 1,024 item ids, so an edit re-reduces one block:
  `SUM(X OVER W)` over 1M items takes 0.4 ms per edit (0.6 s before).
  Fractional `SUM`/`AVERAGE`/`PRODUCT`/`SUMSQ` over such a category are a
  fixed two-level summation and may differ from a single pass in the last
  bits; at up to 1,024 items results are unchanged.
- **Edits cost less per tracked measure.** A measure that maps one input or
  map measure (`M = X + 1`, `-X`, `X * 2`) no longer gets a collapse
  operator and arrangement of its own, and the live engine has one output
  probe instead of one per measure: at 20,000 such measures an edit takes
  0.13 s (0.73 s before), and the engine uses 1.4 GB (3.4 GB before).
- **`improv_engine::CoordKey` is a `SmallVec<[(u32, u32); 3]>`** (was a
  `Vec<(u32, u32)>`): a key of up to three categories needs no heap
  allocation. `evaluate` of a 1M-cell grid takes 2.2 s (4.3 s), 5M cells
  12.7 s (27.7 s), with 10–15 % less memory; a single-cell edit 56 µs
  (73 µs). API: build keys with `CoordKey::from_slice`, `.into()` from a
  `Vec`, or `improv_engine::smallvec::smallvec![…]`, and look them up by
  slice (`map.get(&[(c, i)][..])`).
- **A live engine keeps two fewer copies of its input cells.** `Engine`
  no longer shadows every input cell (nothing read the copy), and the
  worker frees the seed once it has applied it: a PageRank FIXPOINT over
  10M edges peaks at 15.0 GB (16.8 GB before), about 1.0 kB per edge in
  the engine (1.2 kB).
- **`count_distinct` bindings load about 4x faster.** pg_improv, `storage_pg`
  and the DuckDB `improv()` function read them as `count(v)` over
  `SELECT DISTINCT dims, v` (two hash aggregates) instead of
  `count(DISTINCT v)`, which sorted every fact row by its text keys. On
  2.5M rows the load query drops from 4.5 s to 1.1 s. Results are
  unchanged, NULLs included. `recalc` and a `commit`-mode transaction on
  the benchmark model drop from 5.6 s to 2.1–2.4 s.
- **pg_improv 0.11.0: one replication slot per database, not per model.**
  Logical-mode models share the slot and publication `improv_db` (the
  union of their bound tables, kept by the new `improv.sync_publication()`).
  The sync worker decodes the WAL once per poll and routes each change by
  table to the models bound there; each model records its own position
  with its result writes, so a crash re-applies nothing twice, and a
  failing model rebuilds from its tables on its retry (backing off from
  0.5 s to 60 s) instead of holding the slot back. One standby snapshot is logged per poll, not one
  per model. At 1,000 small models a one-row change used to wait 5–70 s and
  a write touching every model 97 s. `ALTER EXTENSION pg_improv UPDATE`
  drops the old `improv_<model>` publications, and the worker drops their
  slots and catches each model up with one rebuild. `improv.status` reports
  `slot = 'improv_db'`.
- **`improv.what_if` loads only the edited coordinates** when every edit
  has the same categories and the chain they feed is element-wise (as in
  trigger mode): 0.27 s instead of 5.3 s for a one-cell edit on 2.5M
  rows. Other edits still load the whole model.

### Fixed

- **FORECAST.ETS no longer panics on a target past 2⁶⁴ steps.** A target
  far beyond the series (1E300 on a series of dates milliseconds apart)
  saturated the step count, and the next step overflowed; it now
  saturates too. Found by the reduction contract property test.

- **Evaluating a deep formula no longer overflows on Windows.** `evaluate`
  ran on the caller's thread, 1 MiB on Windows, where a formula at the
  depth limit (40) overflowed; the live engine's worker had the platform
  default too. Both now run on an 8 MiB engine thread.

- **A formula nested too deeply no longer makes a model unopenable.** A long
  operator chain (about 63 `+` terms) or deep nesting was accepted and saved,
  then every `list`/`show` failed with "recursion limit exceeded". A formula's
  tree may now be at most 40 levels deep (`MAX_FORMULA_DEPTH`) and 100,000
  nodes once `LET` and LAMBDAs are expanded (`MAX_FORMULA_NODES`); past
  either, the parser says "formula nests too deeply (N levels; the limit is
  40)" or "formula is too large", at the operator where a chain passes it or
  the expression's start. Every entry point checks: the CLI, the GUI and TUI
  editors, controlled English, `improv.define`, `LANGUAGE improv`, the DuckDB
  model text, and named-LAMBDA expansion. A `LET` chain that doubles per
  binding fails at once instead of taking seconds.
- **Formulas saved before the limit keep working.** One stored as text (the
  Postgres catalog, `LANGUAGE improv`) loads up to 256 levels, so a 45-term
  sum defined in 0.10 still computes. Past that, the error names the measure,
  and `drop_measure` or `define(…, replace => true)` of it still work. One
  stored as JSON and too deep to read back loads as `#REF!` with a
  description saying why, instead of making the model unopenable.
- **The guide's examples, checked against the product** (the pre-1.0
  docs review, `docs/reviews/2026-10-23-docs-review.md`). Getting
  Started's quick start now shows the line `add-derived` prints, and runs
  as a test. In The Formula Language, the immediately-called LAMBDA
  example is in parentheses (as written it parsed as a named LAMBDA and
  failed), and the page says the `-- notes` are not formula syntax. Improv
  in DuckDB gives the parse error's real position. Running Improv's
  quality gate leaves out both extension crates, so its `cargo fmt` line
  works.
- **`crates/pg_improv/tests/finance.sh` passes again, and CI runs it.**
  It waited for four replication slots, one per model, but since 0.11.0
  a database's models share the one slot `improv_db`, so the guide's
  family-finance example failed its logical-sync check. It now waits for
  that slot, and `ci/pg_improv.sh` runs it on every major.
- **`improv-server` sees other writers (H9).** It cached the model and
  served that copy until one of its own writes was refused with 409, so a
  CLI `set` stayed invisible to `GET /measures/:id/values`. Each request
  now checks the store's high-water mark first (Mentat's newest
  transaction, a Postgres model's `improv.model_version`: one query on a
  kept connection) and reloads when it moved. A write that races another
  writer's save still gets 409.

- **A wide GUI grid draws only its visible columns.** Rows were
  virtualized, but every row laid out every column, off screen or not:
  at 1,000 × 1,000 cells a frame took 125 ms (8 fps), idle or scrolling.
  Cells past the window's edges are now skipped, and a row stops at the
  first column past the right edge. The same grid draws in 7.4 ms (135
  fps) idle, 7.5 ms scrolling down and 9 ms scrolling right (release
  build, headless, tessellation included).
- **A GUI cell edit no longer clones the model.** Every grid edit took
  a whole-model copy for its undo step (170 ms at 1M cells), and undoing
  it rebuilt the engine (2 s). An edit's undo step is now the cell's
  previous value, put back through the live engine: at 1M cells an edit
  takes 66 ms instead of 209, and its undo 52 ms instead of 2 s.
  Structural changes still record the whole model.
- **A failed GUI undo or redo can be retried.** When the save behind an
  undo failed, the undo stack got the current model back instead of the
  undo point, so the retry, once the store was writable again, changed
  nothing and the step was lost. Redo had the same fault.
- **The GUI says why a save failed.** A save conflict now reads "another
  program changed this model since it was opened, so the change was not
  saved and nothing was overwritten. Choose File ▸ Revert to Saved…",
  and a full disk (SQLite's "database or disk is full", or the "disk I/O
  error" it reports when the disk fills as it grows a side file) says the
  disk is full. Before, the backend's text was shown as is. New tests
  kill -9 a process mid-save and reopen the store (every acknowledged
  save there, nothing of the killed one, `integrity_check` ok), fill a
  real disk during saves (a tmpfs in a user namespace), and run two
  writers against one store.
- **A Mentat store forgets what the model dropped.** `save_model` diffed
  only cells, so a category, item, measure or view the model no longer had
  stayed in the file and came back on the next load. An undone add in the
  GUI (a new derived measure, say) reappeared after a reopen. The save now
  retracts them in the same transaction, as the Postgres backend already
  deleted them. Likewise the last named LAMBDA, scenario or external
  function a model drops: the save skipped an empty meta blob, so the old
  one stayed.
- **`pg_ctl stop -m fast` no longer hangs on the pg_improv sync worker.**
  The worker saw SIGTERM only between polls, so a fast shutdown waited for
  the poll in progress. A large apply (a 300k-row batch) ran past
  `pg_ctl`'s timeout, and the stop failed with "server does not shut
  down". SIGTERM is now Postgres's own `die` handler, so the worker stops
  at its next interrupt check, in under a second, with `FATAL: terminating
  background worker "pg_improv sync" due to administrator command`. It
  restarts from the slot position, so no change is lost.
- **`R.QTUKEY` no longer stalls on small probabilities with nranges > 1.**
  It inverted `R.PTUKEY`, a double integral, by up to 65 bisections:
  `R.QTUKEY(1E-10, 2, 2, 19)` took 8 s in a debug build, and the property
  suite timed out on `R.QTUKEY(1E-300, 2, 2, 19)`. It now runs a regula
  falsi on log p against log q, about a dozen integrals and at most 32
  steps (R's qtukey uses a secant). Results agree with the bisection to
  1e-12 where `R.PTUKEY` is smooth, and the goldens are unchanged.
- **`LINEST` and `LOGEST` drop collinear x's**, as Excel documents
  ("removes any redundant X columns"), instead of `#NUM!`. An x that is a
  linear combination of the x's before it (or, with `const`, a constant x)
  gets m = 0 and se = 0, the others fit without it, and df gains one per
  dropped x. `LINEST(Y, X, D OVER T, TRUE, TRUE INTO K)` with D = 2·X was
  `#NUM!`; it is now Y's fit on X alone, with a 0 for D. Goldens from
  Gnumeric 1.12.57, which drops them the same way (dataset `C` in
  `array_reduce_goldens.tsv`); LibreOffice 25.2 returns an
  ill-conditioned fit or `#VALUE!`. One x was already dropped this way.
- **`UNIQUE` ignores case**, as Excel's does (and LibreOffice's and
  Gnumeric's): `"Fig"` and `"fig"` are one value, kept in its first
  spelling, and with `exactly_once` they count as a repeat. Before, text
  differing only in case stayed apart: `UNIQUE` of `pear, Apple, fig,
  apple, kiwi, Fig` was all six, now `pear, Apple, fig, kiwi`. Numbers,
  booleans and dates are unchanged. Goldens in
  `array_reduce_goldens.tsv`.
- **Number to text uses Excel's General form.** A number joined to text
  (`"x" & n`, `CONCAT`, `TEXT(n, "General")`), shown in the grid, or
  returned by DuckDB's `improv_calc_text` switches to an exponent below
  1E-4 and from 1E+15, with 15 significant digits, as Excel (and C's
  `%.15G`) does. Before: `"x" & 1e21` was `x1000000000000000000000`, now
  `x1E+21`; `"x" & 1e-10` was `x0.0000000001`, now `x1E-10`; `"x" &
  1.5e-7` was `x0.00000015`, now `x1.5E-07`. Numbers from 1E-4 to under
  1E+15 are unchanged. Goldens: `numtext_goldens.tsv`
  (`scripts/goldens/numtext.py`), checked against LibreOffice and
  Gnumeric, which differ from Excel and each other at the switch points
  (LibreOffice writes `1E+021` and switches at 1E-15 and 1E+16; Gnumeric
  keeps 17 digits).
- **pg_improv: a failed logical apply rolls back.** The sync worker
  committed a model's transaction even when its apply failed, keeping the
  result rows written before the error. Now the transaction aborts, so a
  failed apply (disk full, a constraint on a result table) writes nothing,
  and the abort releases everything the error left held.
- **`SUM` and `AVERAGE` are compensated sums: the exact sum's nearest
  double.** They now add with Neumaier compensation in every evaluation
  path, flat and two-level. Before, a sum of fractions could miss by
  tens of ulps in one pass and, over more than 1,024 items, a few ulps
  in the two-level reduce (which differed from 0.10.0's one pass). Now
  `SUM(0.1, 0.2, 0.3)` is `0.6` (was `0.6000000000000001`), and over
  10,000 fractional values `SUM` is bit-exact against mpmath where the
  old two-level sum missed by an ulp and a left fold by 14 to 18. A sum
  that was already exact (integers) is unchanged, and no existing golden
  moved. `PRODUCT` and `SUMSQ` are unchanged. The decision and its cost
  (none measurable on the 1M-item edit) are in the guide's Formula
  Language page.

- **`-0 == 0` is TRUE.** `==` and `!=` compared numbers by bit pattern, so
  `ROUND(-0.4, 0) == 0` and `0 * -1 == 0` were FALSE while `<=` said TRUE.
  Numbers now compare numerically (NaN stays unequal to itself); other
  values compare exactly, as before.

- **The coordinate-codec fuzz target builds again.** `CoordKey` became a
  `SmallVec` (no `Arbitrary`), so it fuzzes the pairs as a `Vec`. Its
  lockfile, last refreshed at 0.5.0, is current, and CI now checks that the
  fuzz targets build.
- **The function reference's date intro said serial date 1 is
  1900-01-01**, which is Excel's numbering. Improv numbers as LibreOffice
  does (1 is 1899-12-31, 61 is 1900-03-01); the text now says so.
- **`improv define` and `add-derived` refuse a formula cycle.** They saved
  formulas that depend on each other in a loop (`A1 = B1 + 1` with
  `B1 = A1 * 2`), and every later `eval` of them failed. Now the command
  fails and saves nothing. The engine names the loop in its error, everywhere:
  `formula cycle: A1 → B1 → A1` (was `unsupported plan node in v1 dataflow
  builder: cyclic measure dependency at MeasureId(300)`).
- **`improv refresh-all` reports measures in id order.** It walked a hash
  map, so the order of its lines changed from run to run.
- **pg_improv: the idle sync worker no longer floods the server log.**
  Every poll decoded the slot, and Postgres logs two LOG lines per decode
  ("starting logical decoding for slot improv_db", "logical decoding found
  consistent point"), about 310 MB a day at the default `poll_ms`. A poll
  now decodes only when WAL was written since its last decode, and it
  decodes with `log_min_messages = fatal`.
- **pg_improv: `pg_reload_conf()` reaches the sync worker.**
  `improv.poll_ms` and `improv.max_changes` changed only when the worker
  restarted. pgrx's latch wait never reread the configuration; the worker
  now does that on SIGHUP.
- **pg_improv: `pg_upgrade` carries logical-mode models (PG 17 → 18).** The
  sync worker exits in binary-upgrade mode, where it had held the slot that
  `pg_upgrade --check` reads ("replication slot improv_db is active"). An
  idle worker no longer writes WAL: two commit records a poll, which kept
  the slot from ever catching up, so `pg_upgrade` refused it. The
  README's "pg_upgrade" section gives the procedure, including from PG 15
  and 16 (drop the slot first, and the worker rebuilds), and
  `tests/pg_upgrade.sh` tests it.
- **pg_improv: failover.** On PG 17+ the sync worker makes `improv_db` a
  failover slot. With `sync_replication_slots` on a standby, the models
  continue on the synced slot after a promotion. Without it (or on PG 15
  and 16) the worker makes a new slot and rebuilds each model. `improv.status`
  works on a hot standby: `lag_bytes` was an error there, because it called
  `pg_current_wal_lsn()`, which recovery forbids. The README's "Standbys
  and failover" gives the settings, and `tests/failover.sh` tests a
  primary, a streaming standby and a promotion, in both modes.
- **pg_improv: logical mode follows partitioned tables.** The publication
  sent each partition's changes under the partition's own oid, which no
  model binds, so a model on a partitioned table followed no writes.
  `improv_db` is now `publish_via_partition_root` (set on an existing one
  too). Each partition gets the replica identity it needs, and attaching,
  detaching, dropping or truncating a partition rebuilds the model.
- **pg_improv: `DROP EXTENSION pg_improv CASCADE` no longer leaves the slot
  retaining WAL forever.** The worker saw no extension and did nothing, so
  the `improv_db` slot pinned WAL until someone dropped it by hand. It now
  drops the database's `improv_db` slot and publication when the extension
  is gone, and logs this once. The README's "Removing pg_improv" gives the
  clean order and what a bare drop leaves: the result schemas (user data)
  and `REPLICA IDENTITY FULL`.
- **pg_improv: logical mode outside `improv.database` is refused.** The sync
  worker serves one database. In any other,
  `set_sync_mode(m, 'logical')` succeeded and `improv.status` reported no
  error, with `lag_bytes` read from the served database's slot of the same
  name, while results never followed writes. It is now an ERROR, with a
  HINT naming `improv.database`, and so is `bind_measure` on a logical
  model. `improv.status` matches the slot by database, and a logical model
  outside it reports "no sync worker serves this database" in `last_error`.
- **pg_improv: `improv.max_slot_lag`** (default 4 GB, 0 = off). When the
  sync slot's restart point is further behind, the worker drops the slot
  and rebuilds each model rather than retain more WAL. Logged once, and in
  `last_error` until the rebuild.
- **pg_improv: a failed slot creation no longer blocks every slot in the
  cluster.** With `max_replication_slots` all in use, the worker's caught
  ERROR left `ReplicationSlotAllocationLock` held, since a caught error
  skips the abort that releases LWLocks. Every later slot creation or drop
  in the cluster, `pg_drop_replication_slot` included, then hung. The
  worker now releases them, reports the reason in `last_error`, and makes
  its slot once one is free.
- **pg_improv: an invalidated slot no longer stops logical sync silently.**
  When `max_slot_wal_keep_size` invalidated the `improv_db` slot, every poll
  logged "can no longer access replication slot", `improv.status` showed no
  error, and results stopped following writes. The worker now drops the
  slot, makes a new one, and rebuilds every model, and it logs this once.
  `last_error` reports the lost slot until each model's rebuild succeeds.
  Any other failure to read the slot also shows in `last_error`.
- **A plain restore of a database with a commit-mode pg_improv model
  failed.** `pg_restore` without `--single-transaction`, and `psql -f` of a
  plain dump, commit each table's data on its own and add primary keys
  last, so the commit hook's recompute upserted into a result table with no
  key yet and aborted the restore ("there is no unique or exclusion
  constraint matching the ON CONFLICT specification"). A commit-mode
  recompute is now skipped, with a NOTICE, while a result table lacks its
  primary key; the dump carries the results, and the next write or
  `recalc` recomputes. `tests/txn.sh` restores a model of each sync mode
  both ways.
- **The PGXN publish job found no cargo.** The `pgxn/pgxn-tools` image sets
  `CARGO_HOME=/usr/share/cargo`, so rustup's env file is not under
  `~/.cargo`; v0.10.0's upload failed there and was re-run by hand.

### Security

- **tokio 1.54.1** (was 1.53.1): RUSTSEC-2026-0386, `AsyncFd`'s safe
  constructors break I/O safety. Improv never constructs an `AsyncFd`; the
  bump clears `cargo deny`. pg_improv's lock has tokio only through its test
  harness, bumped too.

- **Dependencies: ctutils 0.4.5 and clipboard-win 5.4.3.** ctutils 0.4.2
  (pulled in through `postgres-protocol`'s `hmac`/`sha2`/`md-5`, so the
  server and the Postgres stores) could branch on secret values
  (RUSTSEC-2026-0385); clipboard-win 5.4.1 (the GUI's clipboard, on
  Windows) was yanked. `cargo deny check` fails on both.

- **The GUI's Postgres store refuses a model its role may not act as.**
  Under pg_improv 0.11's row security such a role sees none of the model's
  items or cells, so `improv-gui postgres://…?model=` (and the CLI) opened
  it empty. Opening now fails: "permission denied for improv model …: only
  its owner, …, or a role that may act as it may open it".

- **pg_improv models act as their owner (C1).** The logical sync worker
  connected as the bootstrap superuser and read every bound table that way,
  bypassing row-level security and table privileges, then wrote aggregates
  of rows the model's owner may not see into result tables with ordinary
  grants. Every read of a model's bound tables and every write of its
  results now runs as `improv.model.owner`
  (`SetUserIdAndSecContext(owner, SECURITY_RESTRICTED_OPERATION)`, with
  `search_path` pinned), in the worker, in trigger and commit mode (which
  ran as the writing role), in `define` and in `recalc` (which ran as the
  caller). A worker apply whose owner lost SELECT fails with a
  `last_error` naming the columns. `what_if` reads as the owner too (see
  Changed). Only a role that may act as a model's owner (a superuser, or a member
  with `SET`, the rule of `ALTER TABLE … OWNER TO`) may change the model or
  its owner: row-level security on every `improv.*` table, and a trigger on
  the owner. Before, anyone who could write the catalog could point a model
  at a superuser, or bind a table into another role's model and redefine a
  result it shared, and so publish a table they could not read. A model's
  input cells (`improv.cell`, `improv.meta`) are now readable by those
  roles only. A restore of the catalog runs as a superuser, as before, or
  as a role that may act as every model's owner (`pg_dump
  --enable-row-security --inserts`).
- **A model's items are readable only by those who may act as its owner.**
  `improv.item` holds the distinct values of every bound category column:
  the bound tables' keys. Its read policy was `USING (true)`, so any role
  with USAGE on schema `improv` could list the keys of a table it could
  not read. It now has the same owner-only policy as `improv.cell` and
  `improv.meta`, in a fresh install and in `ALTER EXTENSION pg_improv
  UPDATE` alike.
- **Writers no longer need privileges on schema `improv`.** With the
  library preloaded, any write by a role without USAGE on `improv` failed
  ("permission denied for schema improv": the commit hook looked up the
  catalog as the writer), as did any write to a trigger-mode model's table
  by a role without the catalog privileges. Those lookups now run as the
  bootstrap superuser.

- **rustls-webpki 0.103.16 and webpki-roots 1.0.10** (were 0.103.15 and
  1.0.9): RUSTSEC-2026-0382 and RUSTSEC-2026-0381, certificate paths through
  the distrusted emSign intermediates. Only libduckdb-sys's build script
  uses them (to fetch DuckDB's sources); no Improv binary links them.

## [0.10.0] - 2026-10-20

The formula language gets a function registry of 681 built-in functions
with Excel's and Gnumeric's names (0.9.0 had a few scalars): math, text,
dates, statistics, finance, lookups, arrays, LET and LAMBDA (named
LAMBDAs too), engineering, options, distributions, matrix analysis, and
`FIXPOINT` for iterative models such as PageRank. Every function has
parameter names, help, autocomplete with ghost arguments in the GUI and
TUI, and located error messages.

pg_improv 0.10.0 keeps large models live. Aggregated bindings (`sum`,
`count`, `min`, `max`, `avg` and the new `count_distinct`) and new items
apply incrementally in logical mode: on 2.5 million rows, a 1,000-row
batch is visible in about 0.1 s with no rebuild. There is a new `commit`
sync mode; you can redefine and drop measures from SQL; dimensionless
results are supported; and named LAMBDAs are kept in `improv.lambda`.

New here: a DuckDB extension, `improv`, which loads into DuckDB 1.5.6 and
2.0; Mentat 1.16.0 from crates.io; Verus-proven integer kernels; and every
release is published to PGXN.

Phases P1–P7 of the built-in functions plan: 674 functions (plus
aliases) with Excel's names, typed errors, the `IF` family,
positional forms and recurrences, lookups, list results, `LET` and
`LAMBDA`, and locale-aware entry.

### Added

- **Category parameters in named LAMBDAs.** A parameter the body uses
  where a category goes (`OVER c`, `POSITION(c)`, `RELABEL`'s categories)
  takes a category at the call, so `LOGNORM = LAMBDA(x, c, LN1P(x) /
  LN1P(MAX(x OVER c)))` is called as `LOGNORM(Authored, Person)`. A
  reduced parameter takes a measure, which keeps the body's `OVER` when
  the call expands.
- **Named LAMBDAs in pg_improv.** `improv.define(model, 'LOGNORM =
  LAMBDA(…)')` stores a named LAMBDA in the new catalog table
  `improv.lambda` (dumped with the model); the model's formulas call it
  by name. Redefining one (`replace => true`) recomputes every measure
  that calls it, in the caller's transaction, writing only the rows whose
  value changed; it is an error naming the measure when it would break
  one or change its type or dimensions. `improv.drop_measure` drops one,
  and refuses while a measure or another lambda calls it. The GUI's
  Postgres store reads and writes the same table (it creates it on an
  older catalog). A model it saved with its lambdas in `improv.meta`
  still loads, and its next save moves them into the table.
- **Redefine and drop measures from SQL.** `improv.define(model,
  definition, replace => true)` redefines a derived measure. Re-running
  the same definition changes nothing: no `model_version` bump, no
  result row written. A new formula recomputes the measure and its
  dependents in the caller's transaction, in every sync mode, and
  writes only the rows whose value changed. `improv.drop_measure(model,
  measure)` drops a measure: a derived measure's result table, or an
  input's binding and cells. A replacement that changes the value type
  a dependent reads, and a drop of a referenced measure, are errors
  that name the dependent.
- **Dimensionless results in pg_improv.** A derived measure with no
  categories, such as `MaxVol = MAX(Vol OVER Person)`, gets a one-row
  result table with just a `value` column. `recalc`, every sync mode,
  `what_if` (its `coord` is `{}`) and `status` handle it; `define` no
  longer refuses it.
- **Role-playing categories: `RELABEL(x, From, To)` and `SWAP(x, A,
  B)`.** `RELABEL(R[Dst, Iter], Dst, Src)` ranges over `Src × Iter` with
  the same values; `W > 0 AND SWAP(W, Src, Dst) > 0` is the reciprocal
  test (W read with its two roles exchanged), with no `@ITEM` or
  `XLOOKUP`. Items match by name; one with no namesake is dropped. See
  *The Formula Language → Roles*.
- **`COALESCE(a, …, default)` and `FILL(x, default)`: a value for a missing
  cell.** `COALESCE` has a cell wherever an argument over all the result's
  categories has one, and takes the first argument with a cell there;
  `FILL` has a cell at every item combination of x's categories. With
  `Authored` set for 2 of 4 people, `COALESCE(Authored, Vol * 0)` and
  `FILL(Authored, 0)` both give 4 cells, where `IFNA` and `IF(ISNUMBER(…))`
  give 2. Both are an outer union in the dataflow (the arguments' cells,
  tagged by position, reduced to the first per coordinate), so input edits
  stay incremental; `IF`'s laziness is unchanged. See *The Formula
  Language → Missing cells*.
- **`FIXPOINT(init, LAMBDA(r, body), [max_iter=100], [tolerance=1e-10])`.**
  A measure that is the fixed point of a step: r = init, then r = body(r),
  until Σ|Δ| ≤ tolerance or max_iter rounds pass. Past max_iter it is the
  last iterate, never a hang (max_iter ≤ 10,000), so the language stays
  total. The engine solves it round by round, warm-starting from the
  previous result after an edit. A round evaluates the step's plan in
  memory, with the cells of the measures it reads fixed for the solve
  (they still update incrementally on the dataflow); a step the host
  evaluator does not cover (`PREV`, recurrences) is a dataflow round. The
  two give the same cells bit for bit. The customer's PageRank (damping
  0.85, dangling mass spread uniformly) on 1,500 nodes and 100,000 edges
  matches a plain power iteration to 3e-15 in L1, in 14 rounds and 0.44 s
  (release), and to the same after one edge weight changes (0.07 s). On
  10,000 nodes and 1,000,000 edges it takes 6.9 s, 1.2 s after an edit.
  See *Fixed points* in the formula guide.
- **Measures reduced together may range over different categories.**
  They need to share only the `OVER` categories. The rest broadcast, and
  the result keeps them: `SUMPRODUCT(r, M OVER Src)` with `r` over `Src`
  and `M` over `Src × Dst` gives one value per `Dst`, and `XLOOKUP(k,
  Key OVER Dst, X)` takes an `X` with more categories than `Key`. These
  were "the measures reduced together must range over the same
  categories".
- **`RANK.DENSE(x OVER T, [order])` and `COUNTUNIQUE(x OVER T)`.**
  `RANK.DENSE` equals SQL's `dense_rank() OVER (ORDER BY x DESC)`, ties
  included (order 1 ranks ascending); `COUNTUNIQUE` (Google Sheets' name)
  equals `count(DISTINCT x)`. The goldens are checked against PostgreSQL's
  `dense_rank()` and `count(DISTINCT …)`; LibreOffice and Gnumeric have
  neither function.
- **pg_improv: aggregated bindings are incremental in logical mode.** A
  measure bound with `agg` (`sum`, `avg`, `min`, `max`, `count`,
  `count_distinct`) keeps per-coordinate fold state in the sync worker,
  so an INSERT, UPDATE (also one that moves a row to another coordinate)
  or DELETE updates its cell without a rebuild; deleting one of two rows
  that share a `count_distinct` key keeps the count. A `TRUNCATE` removes
  the relation's cells without a rebuild too. Relations an aggregated
  binding reads get `REPLICA IDENTITY FULL`. A text `min`/`max` binding
  still rebuilds on change.
- **pg_improv: new items without a rebuild in logical mode.** A row with
  a dimension value the model has no item for no longer rebuilds the
  model: the worker mints the item through `improv.mint_items` in its
  own transaction (same ids and order as a load) and feeds the row to the
  live engine. It still rebuilds when a formula reads that category's
  item list (`POSITION`, `PREV`/`NEXT`, windows, `INTO`, `FILL`, `RANK`,
  …; the README lists them). `improv_engine::session::Engine::serves`
  tells whether a grown model still compiles to the engine's graph.
- **pg_improv: `improv.status` counts the worker's applies and
  rebuilds.** Two new columns: `incremental_applies` (batches applied
  to the live engine) and `rebuilds` (full loads). The 0.9.0 → 0.10.0
  update script adds them to `improv.sync_state` and recreates
  `improv.status`.
- **pg_improv: trigger mode loads only the touched groups.** When the
  recompute is element-wise, each binding is read under a `WHERE` on the
  statement's touched dimension values, so an aggregated binding loads
  only those coordinates' `GROUP BY` groups, not the whole fact table.
  Commit mode still recomputes the written relations' dependents in full
  (the README says why).
- **pg_improv: distinct counting.** `improv.bind_measure(model, measure,
  rel, key_col, agg => 'count_distinct')` counts each coordinate's distinct
  `key_col` values, as `count(DISTINCT key_col)` does (the result is a
  number). The `improv.binding.agg` CHECK accepts it; the 0.9.0 → 0.10.0
  update script widens it. DuckDB's `input X = col count_distinct` too.
- **A DuckDB extension, `improv`.** `improv_calc(formula, inputs)`
  evaluates any Improv formula on named inputs from a JSON object
  (`improv_calc('ROUND(PMT(rate/12, n, -pv), 2)', json_object(...))`),
  NULL for an error value; `improv_calc_text` returns any type and renders
  errors (`#DIV/0!`). `improv_functions()` lists the 674 built-in
  functions with signatures (`query :=` searches them). `improv(source,
  model)` reads a table or query, builds a model from a short spec
  (`category Time = year`, `input Quantity = qty sum`, `Revenue = Price *
  Quantity`) and returns one row per computed cell. Errors name the spec
  line. It uses DuckDB's stable C API, so one binary loads into DuckDB
  v1.5.6 and v2.0. The DuckDB-free logic is a new crate,
  `improv_duckdb_model`. See *Improv in DuckDB*.
- **Formally verified engine kernels.** A new crate, `improv_verified`,
  holds integer kernels with [Verus](https://github.com/verus-lang/verus)
  specifications and machine-checked proofs, and the engine calls them:
  `GCD` / `LCM` (the result is the greatest common divisor, and `LCM`'s
  2⁵³ overflow check is exact), base conversion (`DEC2BIN`, `HEX2DEC`,
  …, `BASE`: decoding an encoding gives back the value, every digit is
  in range), the 48-bit `BIT*` operations, the derived-measure build
  order (each measure after its dependencies; an error exactly when
  there is a dependency cycle), `TAKE` / `DROP`'s index range, and the
  coordinate-key projection and union behind every dimensional join (a
  sorted merge, which makes a 1M-cell `Price × Qty` evaluate about 18%
  faster). `scripts/verify.sh` and a CI job check the proofs. See
  *Architecture & Design → Formal verification*.
- **GUI formula autocomplete.** In both formula editors a name being
  typed drops a list of matching functions (with their signatures and
  descriptions), measures and categories; Up/Down select, Tab or Enter
  accepts (a function as `NAME(`, caret after the `(`), Esc closes, and
  with the list closed Enter commits. The rest of the selected name, or
  inside a call the parameters still to give, trail the caret in gray,
  and a tooltip shows the signature with the current argument in bold.
- **GUI function help.** Help ▸ Functions… (or F1, also while editing a
  formula) opens a window searching the built-in functions by name,
  alias and description, the matches grouped by category with their
  signatures, and the selected one's category, aliases, description and
  argument types. Insert (or a double-click, or Enter) puts `NAME(` into
  the formula being edited, at the caret.
- **GUI located formula errors.** Both formula editors (the selected
  row and the `Name = formula` line, which before reported only to the
  status bar) underline where a formula fails and show, under it, the
  message and a hint: the nearest measure or category names for an
  unknown one, what is missing or extra in a call, the `(` left
  open, the `OVER` form an aggregation needs. A type or dimension error
  underlines the first function or measure its message names.
- **TUI function help and formula editing.** `?` (or F1, also while
  editing) opens a search over the built-in functions: the matches with
  their signatures, and the selected one's category, aliases and
  description. Enter inserts `NAME(` into the edit. `e`/Enter on a
  derived measure edits its formula (symbolic or controlled English),
  with a caret. Tab completes function, measure and category names
  (again to cycle the matches); inside a call the status line shows its
  signature with the current argument marked, and the parameters still
  to type trail the caret, dimmed. A parse error keeps the edit open with
  the bad token underlined, a `^~~~` marker under it, the message, and a
  hint: "did you mean …?" for a misspelled name, the signature for a
  wrong argument count, the `(` left open.
- **pg_improv `commit` sync mode** (pg_improv 0.10.0;
  `improv.set_sync_mode(model, 'commit')`). Results commit or roll back
  with the writer's transaction, as in `trigger` mode, but nothing is
  installed on your tables, and the model recomputes once per transaction,
  just before COMMIT (or PREPARE), not once per statement: 100 one-row
  updates in one transaction on a 10k-cell model commit in 82 ms against
  6.6 s in trigger mode. A read before COMMIT sees the old results
  (`improv.recalc(model)` in the transaction reads your writes), and a
  recompute that fails aborts the COMMIT with an error naming the model.
  Needs `pg_improv` in `shared_preload_libraries` (or
  `session_preload_libraries`); `set_sync_mode` refuses it otherwise. As
  with trigger mode, the role that enables it needs the TRIGGER privilege
  on the bound tables, and every writer must be able to write the result
  tables. `ALTER EXTENSION pg_improv UPDATE` from 0.9.0 adds it.

- **Distributions** (phase P7, 72): Gnumeric's `BERNOULLI`, `CAUCHY`,
  `EXPPOWDIST`, `GEOMDIST`, `LANDAU`, `LAPLACE`, `LOGISTIC`, `PARETO`,
  `RAYLEIGH`, `RAYLEIGHTAIL`, `SNORM.DIST.RANGE`, `OWENT`,
  `CUM_BIV_NORM_DIST`; and the R-style `R.D*`/`R.P*`/`R.Q*` for BETA,
  BINOM, CAUCHY, CHISQ, EXP, F, GAMMA, GEOM, GUMBEL, HYPER, LNORM, NBINOM,
  NORM, POIS, RAYLEIGH, SNORM (skew-normal), ST (skew-t), T and WEIBULL,
  plus `R.PTUKEY`/`R.QTUKEY`, with R's `lower_tail` and `log_p` flags. 456
  goldens (Gnumeric 1.12.57; mpmath where Gnumeric is wrong: its bivariate
  normal is ½ too high for some sign combinations, `R.DSNORM` is wrong off
  location 0, `R.PST` ignores `log_p`).

- **Analysis reductions** (phase P7, 30): matrices as a measure over two
  categories (`MDETERM(A OVER R, C)`, `LOGMDETERM`, `MINVERSE`,
  `MPSEUDOINVERSE`, `CHOLESKY`, `EIGEN`, `LEVERAGE`; results row by row,
  `INTO R2, C2` for a block); time series (`HPFILTER`, `INTERPOLATION`,
  `FOURIER`, `PERIODOGRAM`); `SSMEDIAN`, `CRONBACH`, `LOGREG`, `LOGFIT`;
  the normality tests `ADTEST`, `CVMTEST`, `LKSTEST`, `SFTEST` and their
  `.STAT` statistics; `FORECAST.ETS` (`.ADD`), `.MULT`, `.STAT`,
  `.STAT.MULT`, `.SEASONALITY`, a port of LibreOffice's Holt–Winters, and
  `.CONFINT`/`.PI.MULT` for seasonality 0. 289 goldens (Gnumeric 1.12,
  LibreOffice), most to 1e-12.
- **Operands that share a category join over it**: `A[R, K] * B[K, C]`
  spans R × K × C, so a matrix product is `AB = A * B` then `SUM(AB OVER
  K)` (in place of `MMULT`). Measures with no category in common are still
  refused.

- **Math extras** (phase P7, 46): `LAMBERTW`, `AGM`, `GD`, `ILOG`,
  `REDUCEPI`, `FIB`; number theory (`ISPRIME`, `ITHPRIME`, `PFACTOR`,
  `NT_D`, `NT_SIGMA`, `NT_PHI`, `NT_MU`, `NT_OMEGA`, `NT_PI`, `NT_RADICAL`,
  `PARTITIONS`); `FLT.MAX`, `FLT.MIN`, `FLT.RADIX`, `FLT.NEXTAFTER`; the
  movable feasts (`EASTERSUNDAY`, `ASCENSIONTHURSDAY`, `ASHWEDNESDAY`,
  `GOODFRIDAY`, `PENTECOSTSUNDAY`); `DATE2JULIAN`, `HDATE_JULIAN` and the
  Hebrew calendar (`HDATE`, `HDATE_HEB`, `HDATE_DAY`, `HDATE_MONTH`,
  `HDATE_YEAR`, `DATE2HDATE`, `DATE2HDATE_HEB`); Erlang-B (`PROBBLOCK`,
  `OFFTRAF`, `DIMCIRC`, `OFFCAP`); text (`ASC`, `JIS`/`DBCS`, `BAHTTEXT`,
  `ROT13`, `ENCODEURL`, `ROMAN`, `ARABIC`). Bounded so each call finishes
  in about 2 ms. 303 goldens.

- **Engineering functions** (phase P7, 72, a new *Engineering* category):
  base conversion (`BIN2DEC` … `OCT2HEX`, `BASE`/`NUMTOBASE`,
  `DECIMAL`/`BASETONUM`, `HEXREP`); 48-bit `BITAND`/`BITOR`/`BITXOR`/
  `BITLSHIFT`/`BITRSHIFT`, `DELTA`, `GESTEP`; 45 complex-number functions
  over `"a+bi"` text, with Gnumeric's inverse trigonometric and hyperbolic
  ones, `IMFACT`, `IMGAMMA`, `IMIGAMMA`; `BESSELI`/`J`/`K`/`Y`; `CONVERT`
  with Excel's unit table and SI/binary prefixes. 530 goldens (LibreOffice,
  Gnumeric, Excel's docs).

- **Option pricing** (phase P7, 32 functions): Gnumeric's `OPT_BS` and its
  Greeks (`_DELTA`, `_GAMMA`, `_RHO`, `_THETA`, `_VEGA`, `_CARRYCOST`),
  Garman–Kohlhagen, French, Merton jump diffusion, Miltersen–Schwartz,
  Barone-Adesi–Whaley, Bjerksund–Stensland, Roll–Geske–Whaley, a binomial
  tree, executive, forward-start, time-switch, simple and complex chooser,
  compound, writer-extendible, two-asset correlation, exchange (European
  and American), Kirk's spread, fixed- and floating-strike lookbacks; and
  LibreOffice's `OPT_BARRIER`, `OPT_TOUCH`, `OPT_PROB_HIT`,
  `OPT_PROB_INMONEY`. 200 goldens from Gnumeric 1.12.57 and LibreOffice;
  where Gnumeric's bivariate normal is wrong, mpmath.

- **More list and LAMBDA forms.** `INTO R, C` lays a list over several
  categories row by row (Excel's 2-D spill); `LINEST`/`LOGEST` take any
  number of x's (`LINEST(y, x1, x2 OVER T, TRUE, TRUE INTO Row, Col)`
  gives Excel's 5 × (k + 1) block, checked against LibreOffice);
  `SORTBY` takes several by/order pairs; `SELECTS(values, tests OVER T,
  condition)`; `SELECTLIST` (`SELECT` with a lookup over its own
  category); `MAKEARRAY(R, C, LAMBDA(r, c, body))`; `SEQUENCE(T, [start],
  [step])`; `BYROW`/`BYCOL(x OVER C, LAMBDA(row, SUM(row)))`. An
  expression that reads no measure (positions, literals) now broadcasts
  with anything, so `POSITION(R) * 10 + POSITION(C)` spans R × C.
- **Named LAMBDAs.** `improv define model.db 1 "Margin = LAMBDA(p, c, p -
  c)"`, then `Margin(Price, Cost)` in any formula. Calls are stored as
  calls and expanded when compiled, so redefining a LAMBDA changes every
  use; a body reads only its parameters and earlier LAMBDAs (never
  itself). Saved in the Mentat (`:meta/lambdas`, installed over older
  stores on open) and Postgres (`improv.meta`) stores; the GUI and CNL
  print calls by name.

- **`LET`, `LAMBDA`, `MAP`, `SCAN` and `REDUCE`.** LET names values for its
  body; a LAMBDA is called on the spot or passed to MAP (cell by cell),
  SCAN (a running fold along a category, `SCAN(0, x OVER Time,
  LAMBDA(acc, v, acc + v))`) or REDUCE (its last value). Folds compile to
  recurrences and update incrementally; a LAMBDA cannot recurse, so every
  formula still terminates.

- **List results (array functions)** land along a category named with
  `INTO`: `Top = SORT(Sales OVER Product, -1 INTO Rank)` puts the largest
  value at Rank's first item: `SORT`, `UNIQUE`, `FILTER`, `SORTBY`,
  `TAKE`, `DROP`, `MODE.MULT`, `LINEST` and `LOGEST` (one x; with stats,
  Excel's 5×2 block row by row), `TEXTSPLIT` (each cell its own list, no
  OVER needed) and `FREQUENCY` (bins as separate numbers,
  `FREQUENCY(x OVER T, 10, 20 INTO Bin)`). Excel's empty-array `#CALC!`
  is `#N/A`.
- **Lookups** (phase P6), reductions whose key is a parameter (a key over
  another category gives one lookup per key cell): `XLOOKUP`, `XMATCH`,
  `MATCH`, `LOOKUP`, `INDEX` (`VALUEAT`), Quantrix's `SELECT`,
  `SELECTBETWEEN`, `SELECTGREATERTHAN`, `SELECTLESSTHAN`; the database
  functions as aggregations with `WHERE` (`DSUM`, `DAVERAGE`, `DCOUNT`,
  `DCOUNTA`, `DMAX`, `DMIN`, `DPRODUCT`, `DSTDEV`, `DSTDEVP`, `DVAR`,
  `DVARP`, and `DGET`); `ARRAYTOTEXT`.
- **Lookups' shape:** an aggregation's parameter may range over categories
  the reduced measure lacks; the result gains them and each cell reads the
  whole table (`TEXTJOIN(Sep, TRUE, x OVER Product)` with `Sep` per Time).

- **72 statistical reductions** (phase P4), each `F(x OVER T, …)`:
  - descriptive: `COUNT`, `COUNTA`, `COUNTBLANK`, `SUMA`, `AVERAGEA`,
    `MAXA`, `MINA`, `AVEDEV`, `MEDIAN`, `MODE` (`MODE.SNGL`), `GEOMEAN`,
    `HARMEAN`, `TRIMMEAN`, `DEVSQ`, `STDEV`/`STDEVP`/`STDEVA`/`STDEVPA`,
    `VAR`/`VARP`/`VARA`/`VARPA` (and the `.S`/`.P` spellings), `KURT`,
    `KURTP`, `SKEW`, `SKEWP`, `LARGE`, `SMALL`, `PERCENTILE`(`.INC`/`.EXC`),
    `QUARTILE`(`.INC`/`.EXC`), `PROB`, `SUBTOTAL` and `AGGREGATE`;
  - conditional, with Excel's criteria (`">5"`, `"<>x"`, wildcards
    `*`/`?`/`~`): `COUNTIF`, `COUNTIFS`, `SUMIF`, `SUMIFS`, `AVERAGEIF`,
    `AVERAGEIFS`, `MAXIFS`, `MINIFS`;
  - sums of products: `SUMPRODUCT`, `SUMX2MY2`, `SUMX2PY2`, `SUMXMY2`,
    `SERIESSUM`, `POLYNOMIAL`, `PERCENTOF`;
  - per item (the result keeps the category): `RANK` (`RANK.EQ`),
    `RANK.AVG`, `PERCENTRANK` (`.INC`/`.EXC`), `TREND`, `GROWTH`;
  - regression and tests: `CORREL` (`PEARSON`), `RSQ`, `COVAR`
    (`COVARIANCE.P`), `COVARIANCE.S`, `SLOPE`, `INTERCEPT`, `STEYX`,
    `FORECAST` (`FORECAST.LINEAR`), Quantrix's `EXPA`, `EXPB` and
    `EXPFORECAST`, `CHITEST`, `FTEST`, `TTEST`, `ZTEST` (and their dotted
    spellings), and `AUTOCORREL` (GSL's lag-1 autocorrelation).
- **Positions, neighbours, running totals and recurrences** (the plan's
  phase P3). `POSITION(T)`, `@ITEM(T)`, `@ID(T)`, `@ITEMCOUNT(T)`;
  `PREV(x, T, [n], [default])` and `NEXT`; windows `SUM(x OVER
  T[FIRST..THIS])`, `AVERAGE(x OVER T[THIS-2..THIS])`; and recurrences, a
  measure that reads its own earlier values (`Balance = PREV(Balance, Time,
  1, Opening) + Deposits`), computed as an incremental scan in item order.
- **Aggregations grow up:** several `OVER` categories (`SUM(x OVER Time,
  Region)`), `OVER ALL EXCEPT Region`, `WHERE` filters, parameters (one
  value per group, literal or a measure), reductions of several measures at
  once, and rows in item order. New: `FIRST`, `LAST`, `S`, `TEXTJOIN`.
- **Date arithmetic** in days: a date plus a number is a date, the
  difference of two dates a number of days.
- New measures (`improv add-derived` / `define`, the GUI's formula list,
  pg_improv `define`) take their type and categories from the engine, so a
  text- or date-valued formula is a text or date measure, and an
  aggregation's result drops the categories it reduces.

- **118 built-in functions**, with Excel's names (the plan's phase P1):
  - the logical family `IF`, `IFS`, `SWITCH`, `CHOOSE`, `IFERROR`, `IFNA`,
    `CLEARERROR`, `CLEARZERO` and `BETWEEN`, all lazy: an error or a missing
    cell in a branch `IF` does not choose never shows;
  - the error functions `ISERROR`, `ISERR`, `ISNA`, `NA`, `ERROR` and
    `ERROR.TYPE`;
  - 60 math and trigonometry functions (`POWER`, `MOD`, `LN`, `LOG`,
    `SIN`…`ACOTH`, `FACT`, `COMBIN`, `GCD`, `HYPOT`, …);
  - 17 rounding functions (`ROUND(x, digits)`, `ROUNDUP`, `TRUNC`,
    `CEILING`, `FLOOR.MATH`, `MROUND`, `EVEN`, …), decimal-correct
    (`ROUND(2.675, 2)` is 2.68);
  - 26 text functions (`LEFT`, `MID`, `FIND`, `SEARCH` with wildcards,
    `SUBSTITUTE`, `TEXTBEFORE`, `CONCAT`, `PROPER`, `VALUE`, …), counting
    characters, not bytes.
- **The date family, 66 functions** (phase P2):
  - information: `ISBLANK`, `ISNUMBER`, `ISTEXT`, `ISNONTEXT`, `ISLOGICAL`,
    `ISDATE`, `ISNUMBERORDATE`, `ISEVEN`, `ISODD`, `N`, `T`, `TYPE`;
  - text formatting: `TEXT` with Excel format codes, `FIXED`, `DOLLAR`,
    `NUMBERVALUE`, `VALUETOTEXT`; regular expressions `REGEX`, `REGEXTEST`,
    `REGEXEXTRACT` (first match only) and `REGEXREPLACE`, in the Rust
    `regex` syntax;
  - date and time: `DATE`, `TIME`, `DATEVALUE`, `TIMEVALUE`, the date and
    time parts, `WEEKDAY`, `WEEKNUM`, `ISOWEEKNUM`, `DAYS360`, `DATEDIF`,
    `EDATE`, `EOMONTH`, `YEARFRAC`, `NETWORKDAYS`/`WORKDAY` and their
    `.INTL` forms (no holidays argument yet), `NOW`, `TODAY`, LibreOffice's
    `DAYSINMONTH`…`YEARS`, Gnumeric's `UNIX2DATE`/`DATE2UNIX`, and Numbers'
    `DUR2*` over durations as days.
- **Statistical functions** (the plan's phase P4, scalars): 10 special
  functions (`GAMMA`, `GAMMALN`, `BETA`, `BETALN`, `DIGAMMA`, `IGAMMA`,
  `POCHHAMMER`, `ERF`, `ERF.PRECISE`, `ERFC`) and 44 distributions under
  Excel's names, with legacy names as aliases where they mean the same
  thing (`NORMDIST` = `NORM.DIST`) and their own entries where they do not
  (`NORMSDIST`, `TDIST`, `BETADIST`, `HYPGEOMDIST`, …): the beta, binomial,
  chi-square, exponential, F, gamma, hypergeometric, log-normal, negative
  binomial, normal, Poisson, Student's t and Weibull distributions and
  quantiles, plus `GAUSS`, `PHI`, `FISHER`, `FISHERINV`, `CONFIDENCE.NORM`,
  `CONFIDENCE.T` and `STANDARDIZE`. 326 more golden values
  (`scripts/goldens/statistical.py`). Dotted names whose prefixes are not
  functions themselves (`NORM.S.DIST`, `T.DIST.2T`) parse.
- **More aggregations:** `PRODUCT`, `SUMSQ`, and over booleans `AND`, `OR`
  and `XOR`; `AVERAGE` (with `AVG` as an alias). `SUM`, `MIN`, `MAX` and the
  rest also take a list of arguments (`SUM(a, b, c)`).
- **Optional and repeating arguments** (`ROUND(x)` or `ROUND(x, 2)`), and a
  parse error that shows the signature.
- **One function registry** (`improv_core_model::functions`) drives the
  parser, the type checker, the engine, the GUI's formula text, the CNL and
  a generated **Function Reference** page in the guide.
- 634 golden values, checked through both the scalar evaluator and the
  dataflow engine: from LibreOffice 25.8 headless
  (`scripts/goldens/p1.py`), or from the Excel, Gnumeric, Numbers and
  Quantrix documentation where LibreOffice lacks a function or disagrees
  with Excel.
- **49 financial functions** (the plan's phase P5 scalars), with Excel's
  sign convention (cash paid out is negative):
  - time value of money: `FV`, `PV`, `PMT`, `IPMT`, `PPMT`, `NPER`, `RATE`
    (Newton's method, then bisection; #NUM! without a root), `CUMIPMT`,
    `CUMPRINC`, `ISPMT`, `EFFECT`, `NOMINAL`, `PDURATION`, `RRI`, `DOLLARDE`
    and `DOLLARFR`;
  - depreciation: `SLN`, `SYD`, `DB`, `DDB`, `VDB`, and the French
    `AMORDEGRC` and `AMORLINC`;
  - bonds and securities, on the five day-count bases: `ACCRINT`,
    `ACCRINTM`, the `COUP*` family (`COUPNCD`/`COUPPCD` return dates),
    `DISC`, `DURATION`, `MDURATION`, `INTRATE`, `ODDFPRICE`, `ODDFYIELD`,
    `ODDLPRICE`, `ODDLYIELD`, `PRICE`, `PRICEDISC`, `PRICEMAT`, `RECEIVED`,
    `TBILLEQ`, `TBILLPRICE`, `TBILLYIELD`, `YIELD`, `YIELDDISC` and
    `YIELDMAT`.
  Day counting follows LibreOffice; where LibreOffice departs from Excel's
  documented results (`DURATION` on actual/actual, `PRICE`/`YIELD` with one
  coupon left, `EFFECT(0, n)`, `DOLLARDE`/`DOLLARFR` below 1, `ACCRINT`'s
  calc_method), Improv follows Excel. 457 golden values from LibreOffice
  25.8 and Gnumeric 1.12 (`scripts/goldens/financial.py`; Gnumeric for
  `ODDFPRICE`/`ODDFYIELD`, which LibreOffice cannot compute).
- **Cash-flow reductions** over an ordered category, in item order:
  `NPV(rate, cf OVER Time)`, `XNPV(rate, cf, dates OVER Time)`,
  `IRR(cf OVER Time, [guess])`, `XIRR(cf, dates OVER Time, [guess])`
  (Newton's method, then bisection; #NUM! without a sign change or a root),
  `MIRR(cf OVER Time, finance_rate, reinvest_rate)`,
  `FVSCHEDULE(principal, rate OVER Time)`, and GNU Oleo's financial
  management rate of return `FMRR(cf OVER Time, safe_rate, reinvest_rate,
  reinvest_min)`. A period with no cell is skipped, not zero (Excel's rule
  for blanks in a range); an error flow is the result. 65 golden values
  from LibreOffice 25.8 over a column of flows, and from a transliteration
  of Oleo's `do_fmrr` (`scripts/goldens/financial_reduce.py`).
- `improv.raise_errors` (pg_improv): raise a `LANGUAGE improv` error value
  instead of returning NULL.

- **Operators `^`, `&` and `%`.** `^` is power with the mathematical
  convention: right-associative and tighter than unary minus (`2^3^2` is
  512, `-2^2` is −4; the spreadsheets give 64 and 4). `&` joins two values
  as text, below `+`/`-` and above the comparisons. `%` divides by 100.

- **Formulas in your language.** The GUI formula bar and `improv define` /
  `add-derived` read formulas in the formula locale (`IMPROV_LOCALE`, else
  the OS locale); in German: LibreOffice's German names, a decimal comma and
  `;` separators (`WENN(Price > 1,5; RUNDEN(Price; 2); 0)`). The GUI shows
  formulas the same way. Models store the English form.

### Changed

- **Every release publishes pg_improv to PGXN, or fails.** The PGXN step is
  its own workflow (`pgxn.yml`): called by each tagged release, or run by
  hand for a tag that missed PGXN. It checks the tag, Cargo and control
  versions agree, uploads `ci/pgxn-dist.sh`'s zip (a re-run of an uploaded
  version is a warning, not an error), and waits until api.pgxn.org lists
  the version. Missing PGXN secrets now fail the release instead of
  skipping it, which is how 0.8.0–0.9.0 never reached PGXN.

- **Every function names its parameters** in signature help and the
  Function Reference, instead of their types: Excel's argument names where
  Excel has the function, else LibreOffice's, Gnumeric's or R's
  (`ROUND(number, [num_digits=0])`, `SUMIF(range OVER T, criteria,
  [sum_range])`, `SORT(array OVER T, [sort_order=1] INTO K)`).

- **Dimension errors say what and why**, by name: `A ranges over [Month]
  and B over [Region]: they share no category, so no cell has both …
  Reduce one first, e.g. SUM(B OVER Region), or give them a category in
  common`, instead of `[CategoryId(1)] vs [CategoryId(2)]`. The same for
  function arguments, IF, `[…]` that leaves a category out, and measures
  reduced together.

- **Mentat 1.16.0, from crates.io.** The storage engine is now the
  published `mentatdb` crate (`mentat = { package = "mentatdb", version =
  "=1.16.0" }`), no longer a git dependency on Codeberg; cargo-deny drops
  its git source and the per-crate license clarifications. No code
  changes; storage speed is unchanged (A/B benchmark:
  `docs/reviews/2026-10-18-mentat-1.16-upgrade.md`).

- **Errors are typed values**, shown as the spreadsheets show them:
  `#DIV/0!`, `#NUM!`, `#VALUE!`, `#N/A` (plus `#NULL!`, `#NAME?`, `#REF!`
  from `ERROR`). Division by zero is `#DIV/0!` and a domain error or
  overflow `#NUM!`, where they used to be `NaN`; an error operand
  propagates. `LANGUAGE improv` returns NULL for an error value (it used to
  return `NaN`), or raises it under `improv.raise_errors`.
- `MIN2`/`MAX2` are now aliases of `MIN`/`MAX`, which take any number of
  arguments.
- Comparisons order text, dates and booleans, not only numbers.
- **`NOT` binds looser than a comparison**, as in SQL: `NOT Price > 15` is
  `NOT (Price > 15)`. It used to parse as `(NOT Price) > 15`, a type error.
  Only a formula comparing a `NOT` of a boolean measure with `<`, `<=`, `>`
  or `>=` changes meaning (`NOT a == b` and `NOT a <> b` give the same
  values either way); write `(NOT a) < b` to keep the old reading.

### Fixed

- **The GUI's Postgres store loads `bool` min/max bindings.** `storage_pg`
  built `max(b)` for a boolean column, which Postgres does not have; it
  now reads `bool_or`/`bool_and`, as pg_model's loader does.

- **pg_improv: `agg => 'min'` or `'max'` on a `bool` column.** The load
  read `max(value)`, which Postgres does not have for `bool`, so every
  load of such a binding (in each sync mode, and the logical worker's
  rebuild) failed. It reads `bool_and` (min) and `bool_or` (max).
- **pg_improv: a `define` racing a logical-mode batch.** The worker read
  `model_version` before taking the model's lock, so a `define` that
  committed while the worker waited could see its new results
  overwritten by the old formula's. The version is read again under the
  lock.
- **Calls that never returned now finish in milliseconds.** Property tests
  over every built-in function (hegel, each call under a time budget)
  found loops whose trip count came from an argument: `FACT`, `COMBIN`,
  `PERMUT`, `MULTINOMIAL` of counts near 1e15 (now `#NUM!` once they
  overflow); `VDB` past 1e5 periods, `CUMIPMT`/`CUMPRINC` past 1e7,
  `AMORDEGRC` once its write-down rounds to 0 (0) or after 1e6 periods
  (`#NUM!`); the Bessel functions past order 1e7 and `OPT_BINOMIAL` past
  1e4 steps (`#NUM!`; Gnumeric allows 1e5, which took 11 s);
  `BINOM.INV` and the `R.Q*` discrete quantiles past 2⁵³ trials; the
  quantile bisections (now 65 steps in the floats' order, where
  `BETA.INV` with huge shapes took 16 s); `R.QHYPER` of huge
  populations; `R.PTUKEY` with nmeans near f64::MAX; `FMRR` with a safe
  rate at or below −100% (`#NUM!`); and the skew-normal and skew-t far
  tails.
- **Precision.** `FLOOR`/`CEILING` and their variants snap the quotient to
  15 digits as LibreOffice does (`CEILING(1.17e-10, 117)` is 117, it was
  0); `PV`/`FV` at small rates; `R.PBETA`'s upper tail at a small x; the
  incomplete beta just below x = 1, with one shape below the other's ulp,
  or with shapes whose sum overflows; `R.PF`'s upper tail at a small x;
  beta quantiles under ε above the median (they were 0); the
  hypergeometric mass of populations past 1e6.
- **Panics.** `HDATE` past Gregorian year 29000, `IMIGAMMA` of a real
  shape past 1e12 (`#NUM!`) and `FORECAST.ETS` with a seasonality near
  1e19 (`#VALUE!`); `UNIQUE(…, TRUE)` of only repeated values is
  `#N/A`, not an empty list.
- **`IMIGAMMA` off the real axis.** The upper regularized value for large
  |z| was computed as 1 − P and cancelled: `IMIGAMMA("0.5","30+i",FALSE)`
  gave 1.2e-5+3.4e-5i (it is 5.0e-15−8.1e-15i), the same as Gnumeric 1.12.
  For Re z > 0 past |z| ≈ Re a + |Im a|/2 it now uses Legendre's continued
  fraction, so all four forms match mpmath to about 2e-13. The
  unregularized lower value after the asymptotic series was the regularized
  one (`IMIGAMMA("1+i","40",TRUE,FALSE)` gave 1, not 0.498−0.155i), and
  Γ(0, z) is now E₁(z) rather than `#NUM!`.
- **`IMIGAMMA` left of the imaginary axis.** For Re z ≤ 0 (where neither
  continued fraction applies) it gave `#NUM!`, as Gnumeric does. All four
  forms now follow mpmath's `gammainc` on the principal branch:
  `IMIGAMMA("0","-1",FALSE,FALSE)` is −1.895−πi (Γ(0, −1) = −Ei(1) − iπ),
  through the lower series (or Legendre's fraction where the series would
  cancel) and Γ(a, z) = Γ(a) − γ(a, z). At a = 0, −1, −2, … the upper value
  is E₁ and its recurrence, the lower one `#NUM!` (a pole).
- **TUI import-csv that breaks a formula says so.** Importing into a
  measure a formula uses (giving `Price` a new dimension, say) left every
  derived measure blank with only "imported N cell(s)" in the status. The
  status now adds "formulas not recomputed: …" with the engine's error.
- **Binomial, beta, F and t with very large parameters.** The regularized
  incomplete beta behind `BINOM.DIST`, `BETA.DIST`, `F.DIST`, `T.DIST`,
  `NEGBINOM.DIST` and the `R.*` kernels lost accuracy once both shapes
  passed about 1000, and was wrong from about 10⁸: `BINOM.DIST(4e7, 1e8,
  0.4, TRUE)` gave −0.42 (it is 0.500043), and `R.QBINOM(0.3, 1e16,
  0.4)` was 5.7 standard deviations low. For large shapes it now uses
  Loader's saddle-point prefactor and runs the continued fraction to
  convergence.
- **`LAMBERTW` near −1/e.** For x within about 700 doubles of −1/e it
  returned `#NUM!`: Halley's iteration bounced between two values
  without converging. It now uses the branch-point series there, which
  matches mpmath to 1e-14 on both branches.
- **`RANK`, `PERCENTRANK` and `TREND` over an item that is text.** That
  item is now `#VALUE!`, as in Excel, LibreOffice and Gnumeric, and the
  other items keep their results. `PERCENTRANK(x OVER T)` used to panic
  on such an item, and `RANK` gave it a rank.
- **Saving a partially loaded Mentat model no longer deletes data.** A
  model from `load_partial` saved through its handle used to retract every
  cell outside the loaded closure; the save now only retracts the closure's
  missing cells, as the Postgres store already did.
- **The TUI edits text, boolean and date cells incrementally**, as it does
  numbers (through `session::input_cell`, like the GUI); it used to rebuild
  the whole engine for every non-number edit.
- **`Measure[Cat]` outside an aggregation is refused** when the list leaves
  out one of the measure's categories. It used to change only the type the
  checker saw: `X = Quantity[Product]` still produced every Time×Product cell.
  Inside an aggregation (`SUM(Quantity[Product] OVER Time)`) it works as
  before.

### Security

- **duckdb 1.10506.1** (was 1.10505.0) moves its build-time `zip`
  dependency from 6.0.0 to 9.0.4, fixing RUSTSEC-2026-0367, -0368 and -0369
  (SUID bits, symlink targets and permissions applied during ZIP
  extraction). It only ever unpacked DuckDB's own archive at build time,
  but it no longer needs a waiver.

## [0.9.0] - 2026-10-16

pg_improv is ready for real databases: it builds and is tested on
Postgres 15, 16, 17 and 18; it upgrades in place with ALTER EXTENSION;
pg_dump keeps its models; it installs without the preload; and it is
published to PGXN, with binary packages per Postgres major. A worked
family-finance example shows a household's accounts, statement
reconciliation, budgets, net worth and savings goals in one model.

### Added

- **pg_improv on Postgres 15, 16, 17 and 18** (pgrx features `pg15`…`pg18`;
  17 stays the default). CI runs the whole suite on each.
- **Upgrades:** `sql/pg_improv--0.8.0--0.9.0.sql` and `--0.8.1--0.9.0.sql`,
  so `ALTER EXTENSION pg_improv UPDATE` moves a live database forward.
  `tests/upgrade.sh` builds each earlier release, puts a logical-mode model
  on it, upgrades, and checks results, the worker, `define` and `pg_dump`.
- **`tests/txn.sh`:** the transactional guarantees, end to end. In trigger
  mode a writer reads its own results, other sessions see them only after
  commit, and ROLLBACK, an error or a rolled-back savepoint undo them. In
  logical mode aborted work never reaches the results. Also checks a
  no-preload `CREATE EXTENSION`, that every object is in schema `improv`,
  and dump/restore.
- **PGXN:** `crates/pg_improv/META.json.in` and `ci/pgxn-dist.sh`, which
  builds a buildable source dist (the extension and the six crates it links)
  that CI builds on every push and the release uploads with `pgxn-release`.
- **Release packages:** `pg_improv-<ver>-pg<N>-x86_64-linux.tar.gz` for each
  supported major, from `cargo pgrx package` against that major's PGDG build.
- **Release announcements:** `ci/announce.sh` drafts and submits the
  postgresql.org news item (which feeds pgsql-announce) for feature and
  major releases. A patch release is skipped unless its CHANGELOG section
  has `### Security`.
- **CI:** `ci/pg_improv.sh <PG>`, one script for GitHub, Forgejo and a
  local run: fmt, clippy, `#[pg_test]`, pgspot, `txn.sh`, `logical.sh`,
  and on 17 also `upgrade.sh` and the PGXN dist build.
- **Family-finance example** (`crates/pg_improv/examples/family-finance`,
  guide chapter *Improv in Postgres: a family's finances*).

### Fixed

- **pg_improv: `CREATE EXTENSION` without `shared_preload_libraries` killed
  the session.** `_PG_init` defined the postmaster-level GUC
  `improv.database` on every load; Postgres refuses that after startup with
  a FATAL. It is now defined only when preloading.
- **pg_improv: `pg_dump` lost every model.** The `improv.*` catalog was not
  registered as extension configuration, so a dump held the result tables
  but restored the models empty. The catalog and its sequence are now
  dumped, and a restored database's worker creates a new slot and catches
  up instead of trusting the dumped slot position.
- **pg_improv: binding a view gave an obscure error and left the model half
  bound.** Logical and trigger sync need a table's row changes, so
  `bind_measure` and `set_sync_mode` now refuse a view (or materialized or
  foreign table) up front with a message saying to bind a table or use manual
  mode. Views still bind in manual mode.
- **pg_improv on PG 18:** `HeapTupleHeaderGetRawXmin` is ambiguous there
  (PG 18 exports a C function of that name), which broke the build.

### Security

- **pg_improv's install and update scripts pass pgspot.** `improv.mint_items`
  pins `search_path`, and the extension creates its objects with plain
  `CREATE` instead of `IF NOT EXISTS` / `OR REPLACE`, so it cannot adopt an
  object another role created first. CI runs pgspot on every change.

## [0.8.1] - 2026-10-15

### Fixed

- **pg_improv: a measure defined after the sync worker caught up is
  computed at once.** In logical mode the worker rebuilt only when a
  decoded transaction touched a bound row, and `improv.define()` changes
  the model, not the data, so the new result table stayed empty until the
  next write to the fact table, with no error in `improv.status`. The
  worker now checks `model_version` on every poll. `logical.sh` covers a
  `define` with no data change.

## [0.8.0] - 2026-10-15

Improv runs inside Postgres, and a formula can be a SQL function. The
`pg_improv` extension models your own tables and keeps results current
incrementally (at 100k cells, a one-row change is visible about 50 ms after
commit); `LANGUAGE improv` writes SQL functions in the formula language;
the desktop and terminal apps open a Postgres-hosted model by URL and edit
its rows. The GUI now follows NeXTSTEP Lotus Improv closely, scrolls wide
grids sideways and can revert after a save conflict. A security review's
findings are fixed, and every crate is held at 90% function and branch
coverage.

### Added

- **Documentation.** A *Ways to Use Improv* chapter (GUI, TUI, CLI,
  streaming, Postgres, `LANGUAGE improv`, server: what each is for and how
  to start), a refreshed introduction, getting-started and running pages,
  and man pages for `improv-tui`, `improv-gui` and `improv-server`
  (`improv(1)` gains `stream`, `save-view`, the CSV and refresh commands).
  The guide and API docs publish to Codeberg Pages and GitHub Pages.
- **Demo movies:** the README and guide show the GUI, the TUI and Improv in
  Postgres in motion, each recorded by a script under `docs/demo/`.
- **Release binaries** for Linux, macOS (arm64 and x86_64) and Windows: the
  CLI, TUI, GUI and server, with the man pages.
- **GUI: grids scroll sideways.** A worksheet wider than its window has a
  NeXT horizontal scroller in the bottom strip, right of the row tiles, with
  the arrows grouped at its left end (spec §4.4). Before, the far columns
  could not be reached.
- **Revert to Saved** after a save conflict: GUI *File ▸ Revert to Saved*
  and TUI `R` reload the model from the store (with a fresh save token),
  picking up the other writer's changes, so the conflicting edit can be
  redone. Undo history is dropped; a failed reload changes nothing.
- **`LANGUAGE improv`: SQL functions written in Improv's formula language**
  (`crates/pg_improv`). `CREATE FUNCTION margin(price float8, cost float8)
  RETURNS float8 LANGUAGE improv AS $$ price - cost $$`. A trusted language:
  arguments are named inputs, `Name = expr` lines define intermediates, the
  last line is the result, and the DSL and controlled English both work.
  Numbers, booleans, text and dates/timestamps; NULL in gives NULL out where
  the result depends on it; division by zero is `NaN`; aggregations are
  rejected. Validated at `CREATE FUNCTION`; compiled once per backend
  (0.14 µs per call over inline SQL). Evaluation is the new
  `improv_engine::scalar`, which shares the dataflow engine's per-cell
  operators.

- **pg_improv: Improv models over Postgres data** (`crates/pg_improv`,
  `crates/pg_model`, `crates/storage_pg`, `crates/store`). Bind fact-table
  columns to categories and measures, define formulas, read results from
  `improv_<model>.<measure>` tables. Three update paths: **logical decoding**
  (default; a background worker tails a slot and applies each batch with the
  slot advance in one transaction), **triggers** (in the writer's
  transaction), and **`improv.recalc()`** (explicit, in your transaction);
  plus `improv.what_if` and `improv.status`. The GUI, TUI, CLI and server
  open `postgres://…?model=<name>`. Measured on a c7i.4xlarge at 100k cells:
  logical keeps a writer's UPDATE at 8 ms (results ~50 ms later); a trigger
  makes them visible at commit for ~1.1 s per write. See
  `docs/src/postgres.md`.

- **pg_improv: aggregated bindings.** `improv.bind_measure(…, agg => 'sum')`
  (or `avg`, `min`, `max`, `count`) folds a fact table's rows per coordinate
  into one input cell, in `recalc`, trigger and logical mode and in the GUI's
  load; without `agg` duplicate rows remain an error. The GUI's store refuses
  to save an edit to an aggregated measure (`Integrity`).
- **pg_improv: boolean, text and date measures.** `bind_measure` takes the
  measure's type from its column (`bool`; `text`/`varchar`/`char`;
  `date`/`timestamp`/`timestamptz`; the number types as before) and refuses
  others; `define` accepts boolean, text and datetime results, and result
  tables get a matching `value` column (`boolean`, `text`, `timestamptz`).
  Logical mode applies their row changes cell by cell like numbers' (the
  worker reads the slot's text with `TimeZone = UTC`, `DateStyle = ISO`).

- **`pg_improv` Postgres extension, layer L0** (in progress). A pgrx 0.17
  extension (`crates/pg_improv`, pg17) with an `improv.*` catalog that is a
  superset of `improv_storage_pg`'s, plus `improv.create_model`,
  `bind_category`, `bind_measure`, `define`, `recalc` and `drop_model`.
  `recalc` recomputes every derived measure in the caller's transaction and
  writes only the changed rows to `improv_<model>.<measure>`. The new GUC
  `improv.max_cells` (default 5M) caps the input size. The Postgres-free
  logic is the new workspace crate `improv_pg_model`. Install and quick
  start: `crates/pg_improv/README.md`.
- **`pg_improv` layer L1: trigger-mode sync.**
  `improv.set_sync_mode(model, mode)` installs (for `trigger`) or removes
  statement-level `AFTER INSERT/UPDATE/DELETE/TRUNCATE` triggers with
  transition tables on every bound relation. `improv.on_change` recomputes,
  inside the writer's transaction, only the derived measures that depend on
  the changed inputs. It restricts the recompute to the touched coordinates
  when the chain is element-wise. Also new: `improv.recalc(model, measures
  text[])` recomputes a measure subset and its dependents, and
  `improv.what_if(model, edits jsonb)` returns the derived cells an in-memory
  edit would change, without writing anything. README has the
  trigger/logical/recalc trade-off table.

- **Postgres-hosted models** (pg_improv layer L3). The CLI, TUI, GUI and
  server open `postgres://…?model=<name>` as well as a Mentat file, through
  the new `improv_store::Store` seam. The new `improv_storage_pg::PgModelStore`
  keeps the model in the `improv.*` catalog: bound input measures read and
  write their fact table, unbound cells live in `improv.cell`, and
  `improv.model_version` drives the existing reload-and-retry conflict check.
  `scripts/gui-shot.sh` takes `GUI_SHOT_DB` to capture such a model.

- **Coverage gate.** `scripts/coverage.sh` (and a `coverage` CI job on
  GitHub and Forgejo) fails when any crate drops below 90% of functions or
  branches; the workspace stands at about 96% / 95%. New guide
  `docs/src/running.md`: build, run each front end, security defaults, tests,
  coverage.

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

- **`improv_engine::session::Edit::value` is a `CellValue`** (was `f64`), so
  the live engine takes boolean, text and date edits and seeds every typed
  input cell. `Engine::set` still takes an `f64`.
- **`improv.what_if` returns `jsonb` values** (was `float8`), so it reports
  boolean, text and date measures too (dates as RFC 3339 strings), and its
  edits may set inputs of those types.

- **Mentat 0.14 → 1.11.1** (pin `6fc2bc7f`, Mentat `master` at v1.11.1).
  No Improv API changes were needed. Measured on 100k/200k-cell models
  (`docs/reviews/2026-10-10-mentat-1.11-upgrade.md`): loads, partial loads,
  evaluation and edits 9–26% faster, re-import 13–15% faster, first import
  ~10% slower, store files ~29% larger (new history and value indexes).
  Reported RSS is higher because Mentat now memory-maps the store (1 GiB cap);
  set `MENTAT_MMAP_SIZE=0` to turn that off. The old pin `1af15e15` lived on
  an unpushed branch and was not fetchable from a clean clone; the new pin is
  on Mentat's pushed `master`.

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

### Performance

- **pg_improv logical mode applies changes incrementally (G1).** The sync
  worker keeps each model loaded with a live engine, turns decoded row
  changes into input-cell edits, and writes only the result rows that
  changed (a one-row UPDATE on a 100k-cell model rewrites two result rows).
  It rebuilds on a structural change, TRUNCATE, a new item, or an error. It
  also logs a standby snapshot after each batch so polls decode only new
  WAL. Commit → result visible at 100k cells (p50): 1161 → 52 ms for one
  row, 1552 → 559 ms for a bulk UPDATE.
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

### Fixed

- **Numbers display without binary noise.** Every front end showed a value
  at full `f64` precision, so `1173 * 0.05` read `58.650000000000006`. They
  now show 15 significant digits (`58.65`); stored values and exports keep
  every bit.
- **pg_improv: item ids are minted in one place.** `improv.mint_items`
  (created by both the extension and `improv_storage_pg::CATALOG_SQL`) mints
  ids for new dimension values, serialized per model, with a unique
  `(model_id, category_id, name)` index. `improv.recalc`, the trigger and
  worker paths and the GUI's store all call it, so concurrent loads agree on
  ids and nothing is minted twice. Minting no longer bumps `model_version`.

- **Postgres saves no longer clobber SQL edits to bound fact tables**
  (`storage_pg`, design §9.2 G2). `save_model` now writes only what changed
  since the handle loaded (or last saved). Each bound cell it changes or
  removes is guarded by its loaded value (`… WHERE value IS NOT DISTINCT FROM
  <loaded>`), so a cell SQL changed in the meantime fails the whole save
  with `Conflict` ("reload and retry"). A removed bound cell sets the value
  column to NULL and keeps the row. A save deletes only the catalog rows
  (categories, items, measures, views, `improv.cell` cells) it had loaded, so
  rows added by another client survive. A save with no changes writes no
  cells. `PgModelStore::snapshot` / `set_snapshot` carry the base across
  handles, like `loaded_tx`; without a snapshot a save writes everything
  unchecked, as before.
- `pg_improv`: `recalc`, `define` and `bind_*` no longer fail with "SELECT
  FOR UPDATE is not allowed in a non-volatile function" when they are the
  first statement of a transaction.
- **Broadcast binary ops with the larger operand on the left.** A formula
  such as `Net = Price[Time,Product] - Discount[Product]` (or `*`, `+`, `/`)
  evaluated to no cells. Only the mirrored `Discount - Price` worked. The
  dataflow `MapBinary` joined its operands on the full coordinate, so a row
  matched only when the right operand already spanned the result's
  dimensions. `Join`, `MapBinary` and two-argument scalar calls now share one
  broadcast-join helper that keys both sides on their shared categories.

- **Incremental recalculation no longer drops cells whose value went down.**
  The engine's update stream orders a changed cell's `+new` before `-old`
  when the new value sorts lower, and the session snapshot applied the
  retraction by deleting the key, which wiped the value just written. So
  `Engine::set` lost, for example, `Revenue` when a `Price` decreased. A
  retraction now removes a key only if it still holds the retracted value.

- **Sandboxed external functions on non-FHS hosts (NixOS).** The interpreter
  is now run by its canonical host path, and `/nix/store` is bound read-only
  when present. Before, every sandboxed call failed on NixOS with
  `execvp python3: No such file or directory`.

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

[Unreleased]: https://codeberg.org/gregburd/improv/compare/v0.11.0...main
[0.11.0]: https://codeberg.org/gregburd/improv/compare/v0.10.0...v0.11.0
[0.10.0]: https://codeberg.org/gregburd/improv/compare/v0.9.0...v0.10.0
[0.9.0]: https://codeberg.org/gregburd/improv/compare/v0.8.1...v0.9.0
[0.8.1]: https://codeberg.org/gregburd/improv/compare/v0.8.0...v0.8.1
[0.8.0]: https://codeberg.org/gregburd/improv/compare/v0.7.0...v0.8.0
