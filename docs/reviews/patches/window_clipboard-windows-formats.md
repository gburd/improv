# window_clipboard on Windows: several formats, and drag and drop

A proposal for upstream, for the maintainer to decide whether to file it.
Nothing is filed. The target is
[pop-os/window_clipboard](https://github.com/pop-os/window_clipboard),
tag `sctk-0.20` (`f68595e`), which libcosmic pins. Its issues are
disabled, so a report goes to pop-os/libcosmic and a fix to
window_clipboard as a pull request.

## What is missing

`src/platform/windows.rs` implements only `read` and `write` of one
string (`clipboard_win::{get,set}_clipboard_string`). So on Windows:

- `read_raw` / `read_data` and `write_data` fall back to the
  `ClipboardProvider` defaults, which return `None`. iced's
  `clipboard::read_data` and `clipboard::write_data` (several MIME types at
  once) are no-ops. Note that iced's `read_data` calls `read_raw`, so
  `read_raw` is the method that matters.
- Every `DndProvider` method is a no-op, and `peek_offer` answers "DnD not
  supported". libcosmic's `dnd_source` and `dnd_destination` never fire.
  winit's own drop target takes files only (`WindowEvent::DragDropped`).

improv-cosmic works around both in its own code (`crates/gui_cosmic/src/
win.rs`, `winfmt.rs`), so what follows is what worked there.

## The patch, in outline

### 1. MIME types to clipboard formats

One table, used both ways:

| MIME | Windows format | Bytes |
|---|---|---|
| `text/plain;charset=utf-8`, `text/plain`, `UTF8_STRING`, `STRING`, `TEXT` | `CF_UNICODETEXT` (13) | UTF-16LE, NUL-terminated |
| `text/html` | registered `"HTML Format"` (CF_HTML) | UTF-8 with the CF_HTML byte-offset header |
| `text/uri-list` | `CF_HDROP` (15) | `DROPFILES` + wide paths (read only, at first) |
| anything else | `RegisterClipboardFormatW(mime)` | as given |

The last row makes any private MIME type round-trip between two iced
applications for free.

### 2. `write_data`

Open the clipboard once (`clipboard_win::Clipboard::new_attempts(10)`),
`EmptyClipboard` once, then for each of `contents.0.available()`:
convert with the table and `SetClipboardData` (clipboard-win's
`raw::set_without_clear`, or `raw::set_string_with(.., NoClear)` for
text). Skip a MIME type whose format is already written (the four
text aliases above all map to CF_UNICODETEXT). Return `Some(Ok(()))`.

CF_HTML is the only format that needs an encoder. clipboard-win has one
(`raw::set_html`), but it writes the fragment and its own header with no
terminating NUL. Some readers, Office among them, want the NUL, so write
it, as `winfmt::cf_html` does: the header is `Version:0.9`, then
`StartHTML`, `EndHTML`, `StartFragment` and `EndFragment`, each as ten
zero-padded digits, counting **bytes** of the UTF-8 buffer, with
`<!--StartFragment-->` and `<!--EndFragment-->` around the fragment.
`EndHTML` is the length before the NUL.

### 3. `read_raw` (and so `read_data`)

Open the clipboard, then for each allowed MIME type in order, the first
one whose format `IsClipboardFormatAvailable`: read its bytes
(`raw::get_vec`, or `raw::get_string` for text, which converts to UTF-8)
and return `(bytes, mime)`. For `text/html`, return the fragment: by the
header's offsets, else (writers get them wrong) between the comments,
else the document. Don't trust the offsets blindly; a reader must
bounds-check them and fall back.

### 4. DnD

Drop target, in `register_dnd_destination`:

- `RevokeDragDrop(hwnd)` (winit's files-only target), then
  `RegisterDragDrop(hwnd, target)` with an `IDropTarget` that keeps the
  rectangles.
- `DragEnter` / `DragOver`: map the point (`ScreenToClient`, then
  divide by `GetDpiForWindow / 96` for logical units), find the rectangle
  under it, and send `DndEvent::Offer(Some(id), OfferEvent::Enter { x, y,
  mime_types, surface })` / `Motion`. Answer `DROPEFFECT_COPY` only if a
  rectangle there accepts one of the offered types
  (`IDataObject::QueryGetData` per mapped type).
- `Drop`: keep the `IDataObject` for `peek_offer`, send `OfferEvent::Data`
  (or `Drop`) as Wayland does. `peek_offer` reads it with `GetData` and
  the same table as `read_raw`.
- Files: keep winit's behavior, `text/uri-list` from `CF_HDROP`.

Drag source, in `start_dnd`:

- An `IDataObject` over the `AsMimeTypes` content. `EnumFormatEtc`
  returns `SHCreateStdEnumFmtEtc` over the mapped formats, `GetData`
  answers an `HGLOBAL` per format, `QueryGetData` checks format and
  `TYMED_HGLOBAL`. `SetData` and the advise methods return `E_NOTIMPL` /
  `OLE_E_ADVISENOTSUPPORTED`.
- An `IDropSource`: `QueryContinueDrag` drops when the left button is up,
  cancels on Escape. `GiveFeedback` uses the default cursors.
- `DoDragDrop` is modal: it runs its own message loop until the drop. It
  must not run inside winit's event handler, or every message dispatched
  meanwhile re-enters winit's runner, which panics ("event handler is
  re-entrant"). Post it instead: a thread timer (`SetTimer(NULL, 0, 0,
  proc)`) or a posted message, so it runs from the top of the message
  loop, as a window resize's modal loop does. Then send
  `SourceEvent::Finished` or `Cancelled` from the result.
- `internal` drags (within the app) need no OLE at all; keep sending
  them through the existing channel.

`set_action` and `end_dnd` map to the effect `DragOver` answers, and to
a flag `QueryContinueDrag` reads.

### Size and risk

The clipboard part is around 120 lines and safe (clipboard-win does the
FFI). DnD is around 400 lines of COM in the `windows` crate (0.62:
`#[implement(IDropTarget)]` etc. need `windows-core` as a direct
dependency, since the macro's expansion names it). Both crates are in
window_clipboard's graph already (clipboard-win directly, windows through
wgpu-hal in libcosmic's).

## Manual check on an interactive desktop (two minutes)

Over ssh (session 0) improv-cosmic's copy, paste and drop target were
checked in-process (`cargo test` in `crates/gui_cosmic`, and the clipboard
read back by another process). These need a person at the console or in
an RDP session, because session 0 has no input desktop: `DoDragDrop` never
finishes there, and no window is drawn.

Build both binaries and a small model (Quantity over Time × Product), in
a Developer or plain command prompt at the checkout's root:

```bat
cargo build --locked --release -p improv_cli
cd crates\gui_cosmic && cargo build --locked --release && cd ..\..
set IM=target\release\improv.exe
%IM% init check.db
%IM% add-category check.db 1 Time
%IM% add-category check.db 2 Product
%IM% add-item check.db 10 1 2025
%IM% add-item check.db 11 1 2026
%IM% add-item check.db 20 2 Widget
%IM% add-item check.db 21 2 Gadget
%IM% add-measure check.db 100 Quantity number input Time Product
%IM% set check.db 100 100 --at Time=2025,Product=Widget
%IM% set check.db 100 120 --at Time=2026,Product=Widget
crates\gui_cosmic\target\release\improv-cosmic.exe check.db
```

(With `CARGO_TARGET_DIR` set, the binaries are under it instead.)

The grid shows Quantity: 2025 and 2026 by Widget and Gadget, four
cells. Open Word or Excel, and Windows 11's Notepad (WordPad is gone
since 24H2). Each step takes seconds:

1. **Copy out.** Click the first value, Shift+click the last (all four),
   Ctrl+C. Paste into Word or Excel: *a table with the year and product
   labels*. Paste into Notepad: *the four values, tab-separated*.
2. **Paste in (HTML).** In Excel type `7`, `8` in a row, select both,
   Ctrl+C. In improv-cosmic click the first value, Ctrl+V: *7 and 8
   replace the first row*. Ctrl+Z: *one undo puts both back*.
3. **Drop in (text).** In Notepad type `5`, Tab, `6`, select it, and drag
   it onto the first value: *the pointer shows a copy (+)*, and *5 and 6
   land in the first row*. Notepad keeps its text.
4. **Drop in (HTML).** Select two cells in Excel (or a two-cell table row
   in Word or a browser) and drag the selection onto the second row's
   first cell: *their values land there*, without labels.
5. **Drag out.** Select all four cells, press inside the selection and
   drag into Word or Excel: *a table*; into Notepad: *tab-separated
   values*. The status line says *dragged 2 × 2*. Drag again and press
   Escape before letting go: *nothing is dropped*. The window keeps
   drawing during the drag.
6. **Unchanged.** Drag `check.db` from Explorer onto the window: *it
   opens*. Drag a category tile from Rows to Columns: *the grid pivots*.
   A plain click inside a selection still selects that cell.

Record the Windows build (`winver`), the applications used, and each
step's result in the review note's "Windows gaps" table. When all six
pass, the two Windows release blockers are gone: say so there, in
`docs/src/cosmic-gui.md`'s Platforms table and in the crate's README.
