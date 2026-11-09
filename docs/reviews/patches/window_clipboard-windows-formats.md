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

On santorini a release build and a model are ready in `C:\w-cwin\`:

```bat
cd C:\w-cwin\check
C:\w-cwin\target\release\improv-cosmic.exe check.db
```

Elsewhere: `cd crates/gui_cosmic && cargo run --locked --release --
<a model>.db`, with a measure of numbers (the `check.db` above is
Quantity over Time × Product, made by `C:\w-cwin\check\mkcheck.cmd`).

Open Notepad and Word (or Excel; WordPad is gone in Windows 11 24H2).

1. **Copy out (HTML).** In improv-cosmic, click a cell, Shift+click
   another to select 2 × 2, press Ctrl+C. Paste into Word or Excel:
   *expect a table* with the row and column headers. Paste into Notepad:
   *expect the values*, tab-separated.
2. **Paste in (HTML).** Select a 2 × 2 table in Word or Excel, Ctrl+C.
   In improv-cosmic click an empty area of the grid's values, Ctrl+V:
   *expect the four values* there, and Ctrl+Z to undo them as one step.
3. **Drop in (text).** In Notepad type `1⇥2` (a tab between), select it,
   and drag it onto an improv-cosmic cell: *expect the cursor to show a
   copy (+)*, and `1` and `2` to land in that cell and the next.
4. **Drop in (HTML).** Drag a selected 2 × 2 table from Word (or a table
   in a browser) onto a cell: *expect the values* (headers skipped) at
   that cell.
5. **Drag out.** In improv-cosmic select 2 × 2, press inside the
   selection and drag it into Word: *expect a table*. Drag it into
   Notepad: *expect tab-separated values*. The status line says
   "dragged 2 × 2". Press Escape mid-drag: *expect nothing dropped*.
6. **Still works.** Drag a `.db` model file from Explorer onto the
   window: *expect it to open*. Drag a category tile between Rows and
   Columns: *expect the pivot* (in-window drag is unchanged). While
   dragging out (step 5), the improv-cosmic window keeps redrawing.

Record the Windows build (`winver`), the apps used, and each step's
result in the review note's "Windows gaps" table. Once all six pass,
the two Windows release blockers are gone.
