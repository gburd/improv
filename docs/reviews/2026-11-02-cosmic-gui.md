# A COSMIC desktop GUI for Improv: feasibility spike

2026-11-02. A second desktop GUI, `crates/gui_cosmic` (binary
`improv-cosmic`), built on libcosmic (System76's toolkit for the COSMIC
desktop, on iced). It sits beside `crates/gui` (the NeXTSTEP-style egui GUI,
which stays). Branch `work/cosmic`.

## Verdict

It is feasible on every target. At this pin the spike compiles and runs on
four platforms: Linux, FreeBSD, illumos (Solnix, as a client of a live
COSMIC session) and Windows 11 arm64. Windows x64 is left to GitHub CI.
Three caveats:

1. **illumos needs patched dependencies.** It builds only after Solnix's
   own vendor fixups (the scripts Solnix uses to build COSMIC itself), and
   only without wgpu, which has no illumos backend. A plain
   `cargo build` there fails in `wayland-backend`.
2. **Windows is a second-class COSMIC target.** It builds and starts.
   AccessKit's winit adapter at the libcosmic pin does not compile on
   Windows, so a11y is off there. Rich clipboard formats and drag and drop
   are no-ops in libcosmic's Windows clipboard backend, so improv-cosmic
   does both itself on Windows (`src/win.rs`, 2026-11-08). a11y is an
   accepted limitation. The clipboard and DnD gaps stay **release
   blockers for Windows** until a real drag between two applications is
   checked by hand on an interactive desktop (see "Windows gaps"). No
   window could be shown: ssh runs in session 0, and the interactive
   scheduled task found nobody logged on, so the drawn window is
   unverified there.
3. **libcosmic is pre-release.** It is git-only and 1.0.0 in name, with a
   dozen companion git forks (winit, softbuffer, window_clipboard,
   accesskit). Its API moved between the examples and master. Each bump
   needs a rebuild on all hosts and a `deny.toml` review (README, "Pins").

## Stack and pins

| Piece | Version | Source |
|---|---|---|
| libcosmic | 1.0.0 @ `95236a944bfd51c63a2fc65fe50a592c47168d40` (2026-10-27) | git, pinned by `Cargo.lock` |
| iced (pop-os fork) | 0.14.0, submodule `229186b` | in libcosmic |
| winit (pop-os fork) | 0.31.0-beta.2 @ `9567503` | git |
| wgpu | 28.0.0 | crates.io |
| tiny-skia (software renderer) | 0.11 / 0.12 | crates.io |
| AccessKit (wash2 fork) | 0.22.0 @ `6c20249` | git |
| ashpd (XDG portals) | 0.12.3 | crates.io |
| rfd (Windows dialogs) | 0.16.1 | crates.io |

The root workspace has eframe 0.37 with **wgpu 30 and winit 0.30**.
libcosmic has **wgpu 28 and a winit 0.31 beta fork**. One lockfile cannot
hold both without two copies of each, and `cargo deny` on the root would
then have to allow a dozen git sources.

**Decision: `crates/gui_cosmic` is its own Cargo workspace,** like
`pg_improv` and `duckdb_improv`. It has its own `Cargo.lock` (807 crates),
its own `deny.toml`, and an entry in the root's `exclude`. The root
lockfile, the root `deny.toml` (still `unknown-git = "deny"` with no
`allow-git`) and the egui GUI are untouched. It path-depends on
`improv_core_model`, `improv_engine` and `improv_store`, as `pg_improv`
does.

**The pin lives in `Cargo.lock`, not `rev =`.** libcosmic's own crates
(`cosmic-launch`, via cosmic-settings-daemon) name
`https://github.com/pop-os/libcosmic` without a rev. A `rev = …` in our
manifest makes a second source for the same repository. cargo builds
that, but `cargo vendor` refuses it ("duplicate version vendored from two
sources"). Vendoring is how Solnix builds (see illumos below) and how
distributions and Flatpak build. So there is one source, locked to the
commit.

The lockfile is the only pin for libcosmic and for four companions
libcosmic itself names with no rev: cosmic-settings-daemon,
dbus-settings-bindings, freedesktop-icons and rust-atomicwrites.
**Every build is `--locked`**: `ci/gui_cosmic.sh` (clippy, test, doc,
cargo-deny), `crates/gui_cosmic/ci/solnix-build.sh`, and the README's
commands. To move the pin, on purpose only:

```sh
cd crates/gui_cosmic
cargo update -p libcosmic --precise <commit>
git diff --stat Cargo.lock    # what moved with it
```

`--precise` also re-resolves libcosmic's git dependencies, the unpinned
four to their current heads; a dry run against this same commit moved
cosmic-settings-config `655b14f → 108e095`. So read the lockfile diff,
then re-test:

0. `cargo test --locked --test windows_gaps` fails if the bump
   moved libcosmic, window_clipboard or accesskit_winit: re-check each
   Windows gap (below) on the new commits, update the table, record them;
1. `ci/gui_cosmic.sh`;
2. re-read the license files `deny.toml` clarifies, for every git crate
   whose commit moved (cargo-deny 0.20 does not hash git crates'
   licenses), and review its advisory ignores;
3. build and run on the matrix below: Linux, FreeBSD, illumos
   (`ci/solnix-build.sh` in a COSMIC session) and Windows;
4. update this note's versions and matrix.

**Licenses** (`crates/gui_cosmic/deny.toml`). Everything is in the root's
allow list except the Unlicense (`apply`, a public-domain crate libcosmic
uses), which is added there. Eight git crates have no `license` field,
so they were read by hand at the pinned commits and clarified:

- libcosmic, cosmic-config, cosmic-config-derive, cosmic-theme: MPL-2.0
  (libcosmic's LICENSE).
- iced_accessibility: MIT (pop-os/iced).
- dnd and mime 0.1: MIT (pop-os/window_clipboard).
- cosmic-settings-daemon: MPL-2.0. This is the D-Bus *bindings* repo
  (`dbus-settings-bindings`), not the GPL-3.0 daemon. The GPL
  `cosmic-settings-config` appears in the lockfile through `cosmic-launch`
  (the `desktop` feature), but we do not enable that feature, so it is
  not built: `cargo tree --target all -i cosmic-settings-config` prints
  nothing.

cargo-deny 0.20 does not hash a git crate's license files, so a bump must
re-read them (the comment in `deny.toml` says so). Advisories:

- three unmaintained crates in libcosmic's text and i18n stack:
  ttf-parser, rustybuzz, basic-toml;
- the root's existing zbus-5 trio.

All are ignored there with reasons. `cargo deny check`:
advisories, bans, licenses, sources ok.

**libcosmic's `src/localize.rs` (2026-11-09).** cargo-deny reads the
manifest, which says nothing about this: the one file of libcosmic that
says `SPDX-License-Identifier: GPL-3.0-only` (the Fluent loader and `fl!`
macro behind its context drawer's Close button and its calendar's month
and day names, compiled unconditionally). Asked upstream,
[pop-os/libcosmic#1500](https://github.com/pop-os/libcosmic/issues/1500).
Until it is answered, release and distributed builds vendor libcosmic
with that file replaced by `crates/gui_cosmic/release/localize.rs`, Improv's
own MIT OR Apache-2.0 implementation, written clean-room from the
module's interface (its callers, `i18n.toml`, `i18n/en/libcosmic.ftl`) and
i18n-embed's documentation, without opening upstream's file.
`release/vendor-libcosmic.sh` vendors, replaces and re-stamps the
checksum, and refuses if the file is gone or its first line is no longer
that GPL header. `release/license-scan.py` then reads every vendored
file's SPDX headers: GPL and AGPL fail, and an LGPL file fails unless
`release/license-allow.txt` lists it and it matches its upstream crate.
`ci/gui_cosmic.sh` runs both, CI's `deny` job scans the root's vendored
crates, and `release/build.sh` and `ci/solnix-build.sh` build from the
patched tree. Checked at this pin: 836 crates vendored, 789 files with SPDX
headers, the 18 LGPL-2.1-or-later KDE protocol XMLs of
`wayland-protocols-plasma` and `wayland-protocols-misc` (MIT crates; also
in the root's lockfile), and no other copyleft header (the root adds
nix's test kernel module, `GPL-2.0+ or MIT`, taken as MIT). Solnix's
vendor fixups touch neither LGPL crate. The release build's libcosmic is
compiled from our file: its dep-info names the vendored copy, whose
sha256 is ours, and its rlib carries our file's log strings. improv-cosmic
uses none of the widgets that call `fl!`, so neither file's code is left
in the linked binary. Once upstream relicenses the file, the patch and
the check are dropped.

## Platform matrix

The same tree on each host: `work/cosmic` plus the hello window (one
model, one measure's grid, through the model service).

| Host | OS | Compiles | Runs | Display / renderer | Notes |
|---|---|---|---|---|---|
| werk | NixOS (Linux 6.18) x86_64 | yes, 1m24 cold debug; release 2m13 | yes | X11 (Xvfb), wgpu on llvmpipe (GL) | Release binary 65 MB (48 MB stripped); 182 MB peak RSS on the demo model. The in-app capture (`IMPROV_COSMIC_SHOT`) works. |
| lixi | Solnix (illumos) x86_64, rustc 1.99.1 | yes, with Solnix's vendor fixups (`ci/solnix-build.sh`, below); 1m54 | **yes, in a live COSMIC session** | Wayland client of cosmic-comp 1.5.0, `ICED_BACKEND=tiny-skia`, 2× scale | Needs `cargo vendor` and `cosmic-comp-vendor-fixup.sh` + `cosmic-widen-linux.py` (rustix ioctl, wayland-backend MSG_CMSG_CLOEXEC/epoll, polling's epoll backend, winit `is_main_thread`, softbuffer shm names), wgpu off, and libxkbcommon/wayland/mesa from the Nix store on `PKG_CONFIG_PATH`. Found and fixed: `improv_extfn` used `RLIMIT_NPROC`, which illumos lacks. The in-app capture works. No screenshot portal (`org.freedesktop.portal.Desktop` is not on the bus), so the portals are absent there. |
| nuc | FreeBSD 15.1 amd64, rustc 1.99.1 | yes, 3m02 | yes, under Xvfb | X11 (Xvfb), wgpu on Mesa llvmpipe; tiny-skia fails on Xvfb's 24-bit visual ("does not use softbuffer's pixel format") | No display stack is installed and there is no root. libxkbcommon, wayland, X11, Mesa and Xvfb came from pkg.freebsd.org into `~/w-cosmic/pkgs/root`. Xvfb's hard-coded `/usr/local/bin/xkbcomp` was binary-patched for the test. Captured with xwd; the in-app capture never answered there. No Wayland compositor tried. |
| santorini | Windows 11 arm64, rustc 1.99.1 (MSVC) | yes, 2m33 cold | starts; window not verified | wgpu: "Invalid surface" panic over ssh (session 0, no desktop); tiny-skia: runs until killed, no capture | **a11y off**: `accesskit_winit` @ `6c20249` fails E0782 on Windows. Dialogs via rfd (native), not the portal. A run on the user's desktop (`schtasks /create … /it` then `/run`) did not start: nobody was logged on (`query user`: none; console session 1 has no user), so `/it` had no session (last result 267011, never ran). Needs someone logged on at the console or over RDP. |
| Windows x64 | (GitHub CI only) | expected (same code as arm64) | n/a | | The `gui-cosmic` CI job builds and tests on `windows-latest`. nara (Win11 x64) is not reachable from werk yet (keys refused). |

## Windows gaps

The maintainer's call (2026-11-02): one gap is accepted, two block a
Windows release.

| Gap | Where it lives (at the pin) | Status | Upstream |
|---|---|---|---|
| **Accessibility off on Windows.** AccessKit's winit adapter (`accesskit_winit` @ wash2 `6c20249`) fails E0782 (`&Window` where winit 0.31 has a trait), so `a11y` is enabled only on the free Unixes (`Cargo.toml`). | wash2/accesskit `platforms/winit/src/platform_impl/windows.rs:20` | **Known limitation**, accepted. | [pop-os/libcosmic#1152](https://github.com/pop-os/libcosmic/issues/1152) ("Windows build broken in main branch": exactly this error, open since 2026-03-06). Behind it, [AccessKit/accesskit#654](https://github.com/AccessKit/accesskit/issues/654) (winit 0.31 support). |
| **Range copy/paste of rich formats on Windows.** iced's `clipboard::write_data`/`read_data` (several MIME types at once: TSV plus HTML) reach `window_clipboard`'s Windows provider, which implements only `read`/`write` of one string; the multi-format calls fall back to the trait defaults, which do nothing. | pop-os/window_clipboard `f68595e` (tag `sctk-0.20`), `src/platform/windows.rs` | **Worked around in improv-cosmic** (2026-11-08): on Windows, Ctrl+C writes CF_UNICODETEXT (the TSV) and "HTML Format" (CF_HTML) in one clipboard open, through clipboard-win; Ctrl+V prefers the HTML, else the text (`src/win.rs`; the CF_HTML encoder and decoder are pure, `src/winfmt.rs`, with property tests). Verified on santorini (Windows 11 25H2, build 26200, arm64) over ssh: the real Ctrl+C/Ctrl+V path ran in `cargo test`, then another process read the clipboard back. It held formats 13 (CF_UNICODETEXT) and 49316 ("HTML Format"), every header offset pointed where it says (EndHTML = the length before the NUL), a non-ASCII header cell came through as UTF-8 bytes `47 72 c3 b6 c3 9f 65 20 e2 80 93 20 e2 82 ac` ("Größe – €"), and `Get-Clipboard -TextFormatType Html` returned the document. **Release blocker** until the checklist below is run on a desktop. | None found. No issue in pop-os/libcosmic, iced-rs/iced or hecrj/window_clipboard, and pop-os/window_clipboard has issues disabled. A patch outline for the maintainer to file or not: [`patches/window_clipboard-windows-formats.md`](patches/window_clipboard-windows-formats.md). |
| **Drag and drop on Windows.** The same provider's `DndProvider` is all no-ops (`start_dnd`, `register_dnd_destination`; `peek_offer` returns "DnD not supported"). So libcosmic's `dnd_source`/`dnd_destination` never fire there. Before the workaround, only dropping files on the window worked (`window::Event::FileDropped`, from winit's files-only OLE target); text and HTML from other applications were refused, and nothing could be dragged out. | same file | **Worked around in improv-cosmic** (2026-11-08): the window's OLE drop target is ours (`RegisterDragDrop`; winit's is revoked once the window opens). Text or HTML dropped on a cell pastes there as one change, and a dropped file still opens. A range pressed and dragged starts `DoDragDrop` with an `IDataObject` offering CF_UNICODETEXT and "HTML Format". It starts from a thread timer, so its modal loop runs outside winit's event handler. Verified in-process on santorini: the target's `IDropTarget` methods called with data objects carrying both formats, text alone, a file, and neither; `EnumFormatEtc` and `GetData` for both formats on the offered `IDataObject`; and the registration in a running improv-cosmic. **Not yet hand-verified:** a real drag between two applications on an interactive desktop (`DoDragDrop` cannot finish in session 0). **Release blocker** until then. | None found for Windows. [pop-os/libcosmic#810](https://github.com/pop-os/libcosmic/issues/810) is DnD on non-COSMIC Linux desktops, not Windows. Same patch outline. |

**Closing the blockers.** Someone at santorini's console (or in an RDP
session) runs the two-minute checklist at the end of
[`patches/window_clipboard-windows-formats.md`](patches/window_clipboard-windows-formats.md)
and records the result here. If every step passes, drop "release blocker"
from these two rows, from `docs/src/cosmic-gui.md`'s Platforms table and
from the README. When window_clipboard gains these upstream, remove
`src/win.rs` and go back to iced's clipboard and DnD.

**The tripwire.** `crates/gui_cosmic/tests/windows_gaps.rs` records the
commit of libcosmic, window_clipboard and accesskit_winit each gap was
last assessed at. It **fails** when `Cargo.lock` moves any of them, and
names what to re-check, until the new commits are recorded there. So every
pin bump re-opens this table, and it is the bump checklist's step 0
(README, "Pins"). In-window drag (the pivot tiles, stage 4) uses plain
mouse tracking, which works on Windows too. The blockers are drag
between applications and rich formats.

## Building on illumos (Solnix)

`crates/gui_cosmic/ci/solnix-build.sh` reproduces the lixi build on any
Solnix host with a COSMIC session installed. It builds the way Solnix
builds its own COSMIC apps (`graphics/cosmic-apps.nix` and
`cosmic-build.sh` in Solnix's store), and takes everything from the
installed session so the build matches what runs:

1. **Find the session's inputs.** `cosmic-comp`, from the system
   profile, links the client libraries we need (libxkbcommon, wayland,
   libffi, mesa). Its derivation names the `graphics` fixup scripts it
   was built with. `SOLNIX_LIBS` and `SOLNIX_GRAPHICS` override them.
2. **Wrap the compiler.** A `cc` wrapper adds `-L` and `-Wl,-R` for those
   libraries and gcc's libgcc_s. Without it, rustup's own binaries and our
   build scripts fail to load `libgcc_s.so.1`.
3. **Vendor and patch.** `cargo vendor --locked` (once per Cargo.lock),
   then Solnix's `cosmic-comp-vendor-fixup.sh` on the vendored copies
   (9 files: rustix's ioctl encoding, wayland-backend's epoll and
   MSG_CMSG_CLOEXEC arms, polling's epoll backend, winit's
   `is_main_thread`, softbuffer's shm names), then
   `cosmic-widen-linux.py` (14 `target_os = "linux"` gates in libcosmic
   and iced_winit widened to illumos, so the Wayland client side, cctk
   and dbus-config build). Both scripts re-stamp the vendored checksums.
4. **Build offline,** `--locked --offline` against the patched vendor
   directory. wgpu is already off on illumos (`Cargo.toml`), since it has
   no backend there.
5. **Run** in the session with `ICED_BACKEND=tiny-skia` and
   `UMEM_OPTIONS=backend=mmap`. libumem's default sbrk backend fails the
   renderer's first large allocation; Solnix's app wrappers set the same
   two variables.

A fixup whose anchor upstream removed fails loudly (Solnix wrote them to),
so a libcosmic bump that outruns Solnix shows up as a build error here,
not a silent miscompile. The one Improv-side fix is `e666179`
(`improv_extfn`: no `RLIMIT_NPROC` on illumos).

## How each COSMIC integration point is done

- **Theme, light/dark, accent.** Free with `cosmic::Application`. The
  runtime follows cosmic-settings-daemon over D-Bus (`dbus-config`) and
  `cosmic-config` files, and restyles live. The grid paints with
  `theme.cosmic()` colors: `bg_color`, `primary_container_color` for
  headers, `bg_divider` for rules, and `accent_color` for the selection.
  Off COSMIC (Windows, other desktops) the default dark or light COSMIC
  theme applies. High contrast comes with the theme.
- **Clipboard.** `iced::clipboard::write_data` takes an `AsMimeTypes`
  value, so one copy can offer `text/plain;charset=utf-8` (TSV) and
  `text/html` (a `<table>`) together; `read_data` asks for the best
  format offered. Caveat from the pinned `window_clipboard`:
  multi-format data works on **Wayland only**. On X11, `write_data` is a
  no-op, so we also write plain text (TSV), which every spreadsheet
  accepts. On Windows, improv-cosmic skips iced's clipboard and writes
  CF_UNICODETEXT and "HTML Format" itself, in one open, through
  clipboard-win (`src/win.rs`). A paste prefers the HTML. The HTML's
  headers are `<th>` (the column and row labels), and a paste skips them,
  so a copied range pastes back as its values. (Stage 7; Windows
  2026-11-08.)
- **File chooser.** `cosmic::dialog::file_chooser` uses the XDG portal
  (ashpd) on the free Unixes and rfd (native dialogs) on Windows. Both
  return URLs; we open `file://` paths as Mentat stores. (Stage 8.)
- **Printing.** libcosmic has no print API. The XDG Print portal is
  `ashpd::desktop::print` (ashpd 0.12, already in the graph): render the
  grid to PDF and hand the portal its fd. Windows needs a separate path
  (open the PDF with the default handler). (Stage 9.)
- **Opening URIs.** `ashpd::desktop::open_uri` on Unix (planned: help
  links, a function's documentation); `ShellExecuteW` on Windows.
- **Drag and drop.** Within the window (pivot tiles), plain mouse
  tracking, which works everywhere. Dropping files onto the window uses
  `window::Event::FileDropped`. Between applications, libcosmic offers
  `dnd_source`/`dnd_destination` over the Wayland data-device, which
  improv-cosmic does not use yet, so on Linux a range is not dragged
  between applications. On Windows, where window_clipboard's DnD is all
  no-ops, improv-cosmic registers its own OLE drop target (text and HTML
  paste at the drop cell, a file opens) and drags a range out with
  `DoDragDrop`. The drag starts from a thread timer, not in the event
  handler: inside winit's handler, the modal loop's messages would
  re-enter winit's runner, which panics. The canvas starts it when the
  press is inside a range of more than one cell and the pointer moves
  4 px; a press elsewhere selects as before.
- **Desktop entry and icon.** `me.burd.Improv` is the `APP_ID`, which
  COSMIC uses as the Wayland app id. Planned: a
  `res/me.burd.Improv.desktop` and a scalable icon installed under
  `hicolor`, which COSMIC's dock and launcher match on that id.
- **Notifications** (planned). `org.freedesktop.Notifications` via zbus
  (in the graph already) or `ashpd::desktop::notification` under a
  portal. Use them only for what happens out of view: a long import
  finished, a save failed after the window lost focus.
- **Keyboard shortcuts** (planned). libcosmic's `menu::KeyBind` maps
  (Ctrl+O, S, Z, Shift+Z, C, V, P, Q). Its `responsive_menu_bar` in
  `header_start` shows them, and the HIG's header-bar menu layout comes
  with it.

## Accessibility

iced in libcosmic's fork has AccessKit (`a11y` feature, accesskit 0.22
from wash2's fork, AT-SPI via accesskit_unix and zbus). Standard
widgets publish nodes: buttons, text inputs, scrollables, menus,
toggles and the rest.

**A canvas publishes nothing.** The worksheet grid is a canvas, so
before stage 2 ships it needs its own `a11y_nodes`: a table node, plus
row and cell nodes for the cells in view. The AccessKit `Table` role
supports that. Until then a screen reader sees only the header bar and
the measure list.

AccessKit is off on Windows at this pin (the build failure above). Turn
it on when libcosmic moves past `6c20249`.

## Architecture

```
 UI thread (iced/libcosmic, MVU)          model thread (one per window)
 ─────────────────────────────            ─────────────────────────────
 App { model: Arc<Model>, snapshot,  ──Request──▶  dyn ModelService
       layout, grid, cursor, … }                   LocalService:
   view(): pure, paints visible cells              Store (Mentat | Postgres)
   update(): applies Updates          ◀──Update──  Engine (DD worker thread)
                                     (futures mpsc → Subscription)
```

- **`ModelService` trait** (`service.rs`): `open`, then (stage 2)
  `edit(cells) -> (stored, Changes)` and (stage 3)
  `restructure(model) -> Snapshot`. `LocalService` opens the store and
  runs `improv_engine::session::Engine` exactly as `crates/gui` does: one
  engine over every derived measure, `apply_delta` per edit round,
  `save_changes` with a carried `SaveToken`. A `RemoteService` over
  `improv-server` (the Cedar-policy session work) implements the same
  trait. The window never knows which one it has.
- **No waiting on the UI thread.** The service runs on its own thread
  with `ENGINE_STACK`, and talks to the window through an unbounded
  futures channel that a `Subscription` polls. Load, build,
  recalculation and save never block a frame. This is the H3 property
  `crates/gui` reached with three background threads and polling; here it
  falls out of the Elm architecture.
- **Edits are async.** The window applies a sent edit only when the
  service's `Update::Edited` comes back, with the engine's `Changes`
  (deltas, not snapshots: O(change)). So the service stays the authority,
  as a remote server would be. An optimistic local echo can come later.
- **Grid** (`pivot.rs`, `sheet.rs`). The pivot layout (rows, columns,
  pages) is pure and tested. Lines are decoded from an index by mixed
  radix and never materialized, and the canvas paints only the cells in
  view. A 1000 × 1000 stack (10⁶ rows) scrolls to its last row in
  constant work (test `a_million_rows_scroll_in_constant_work`).

## Staged plan to parity with `crates/gui`

Each stage is one atomic, green commit with tests.

| # | Stage | Reuse | Size |
|---|---|---|---|
| 1 | Worksheet grid, virtual scrolling at 10⁶ cells, keyboard cursor | `pivot`, `sheet` (done in the spike, plus keys) | S |
| 2 | Cell editing (typed by value type, refusals) | service `edit`; `parse_typed` rules from `crates/gui` | M |
| 3 | Formula list + editor: located errors, autocomplete, signature help, highlighting | `improv_client` (main's shared front-end crate: `diagnostics`, `completion`, `function_help`, `dsl`), by path | L |
| 4 | Category tiles, pivot by drag (rows/cols/pages, stacking) | `pivot::Layout::place` | M |
| 5 | Views (save/open/rename/delete) | `core_model::View`/`MatrixPlacement` | M |
| 6 | Charts (bar/line) in theme accent + palette | the pure `ChartData` half of `crates/gui/src/chart.rs` (→ shared crate) drawn on a canvas | M |
| 7 | Copy/paste ranges as TSV + HTML | new pure `clip.rs` (TSV/HTML encode, TSV/HTML-table decode) | M |
| 8 | Save / open through the portal | `cosmic::dialog::file_chooser` | S |
| 9 | Print through the portal | ashpd print + a PDF of the grid (needs a PDF writer: `printpdf` or `krilla`, license check) | M–L |
| 10 | Undo / redo | `crates/gui`'s step model (cell steps, structure steps) on the service | M |
| — | Then: CSV import wizard, live DB sessions (`postgres://`, H9 refresh), number formats, scenarios, LAMBDAs, multi-matrix canvas | `csv_wizard` pure half; service methods | L |

Effort: stages 1–10 are roughly 6–8 focused days for one engineer. Real
parity with `crates/gui` (its 20k lines of app logic, the multi-matrix
canvas, every dialog) is 4–6 weeks, most of it UI work rather than
model work. The model work is shared.

## Parity reached (2026-11-02, same day)

Stages 1–10 landed on `work/cosmic`, one commit each: keyboard and
ranges, cell entry, the formula list and editor (`improv_client`),
category tiles and pivoting by drag, views, charts, copy and paste (TSV
and HTML), open and Save As through the file chooser, print through the
print portal (pdf-writer), and undo. The guide page is
`docs/src/cosmic-gui.md`; `improv-cosmic(1)` is generated with the other
man pages. Checked with a real pointer and keyboard under Xvfb (X11), on
Solnix in its COSMIC session (the grid), and by `xclip` (copy and paste
on X11). Not exercised: the portal dialogs (no host here runs
xdg-desktop-portal) and the Wayland clipboard's HTML (no wl-clipboard on
lixi).

The final tree (all ten stages, rebased on main `d3eb4e5`), rebuilt on
each host:

| Host | Build | Tests | Ran |
|---|---|---|---|
| werk (Linux) | yes | 67 + 2 pass, cargo-deny ok | Xvfb (X11, wgpu); `scripts/cosmic-shot-check.sh` 0 px |
| lixi (Solnix) | `ci/solnix-build.sh`, 4m27 | not run | its COSMIC session (Wayland, tiny-skia), full UI captured |
| nuc (FreeBSD) | yes | not run | Xvfb (X11, wgpu on llvmpipe), full UI captured |
| santorini (Windows arm64) | yes | 67 + 2 pass | no window: nobody logged on | What `improv-gui` has that this does not yet: the guide's "Not
yet".

## Risks

- **libcosmic churn.** It is git-only and pre-release; plan on API
  fixes per bump. The pin and a per-host build at each bump contain it.
- **illumos depends on Solnix's fixup scripts.** They live in its Nix
  store, not upstream. For an illumos package, build the way Solnix
  builds its COSMIC apps (vendor, fixups, no wgpu).
- **Windows** lacks a11y, and its window is not yet seen on real
  hardware. Rich copy and paste and drag and drop are improv-cosmic's own
  COM code there (`src/win.rs`), tested in-process only. Run the checklist
  in an RDP or console session on santorini before claiming Windows
  support.
- **Binary size.** Debug builds are about 1.2 GB on every host, release
  65 MB. Fine for a desktop app, heavy for CI caches.
