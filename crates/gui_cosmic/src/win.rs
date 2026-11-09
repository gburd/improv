//! Windows: copy and paste a range as text and HTML, and drag and drop
//! with other applications. libcosmic's Windows clipboard backend
//! (pop-os/window_clipboard) does neither at the pin: it reads and writes
//! one string, and its drag and drop is all no-ops (review note, "Windows
//! gaps"). So the window does them itself, through the Win32 clipboard
//! (clipboard-win) and OLE (the windows crate). The formats are
//! [`crate::winfmt`]'s, pure and tested everywhere.
//!
//! - **Copy** puts CF_UNICODETEXT (the TSV) and "HTML Format" (the table)
//!   on the clipboard in one open. **Paste** prefers the HTML, else text.
//! - **Drops**: the window's OLE drop target is ours (winit's, which takes
//!   only files, is revoked). Text or HTML dropped on a cell pastes there;
//!   a file dropped opens it, as winit's did.
//! - **Dragging a range out** runs `DoDragDrop` with a data object that
//!   offers both formats. It starts from a thread timer, so its modal loop
//!   runs outside winit's event handler and the window keeps drawing.
//!
//! Each `unsafe` block says why it is sound. They are FFI calls on this
//! (the UI) thread, and pointers OLE hands to our COM methods.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::mem::ManuallyDrop;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use clipboard_win::{formats, options::NoClear, raw, Clipboard};
use cosmic::iced::{window, Point, Task};
use futures::channel::{mpsc, oneshot};
use windows::core::{implement, Ref, BOOL, HRESULT};
use windows::Win32::Foundation::{
    GlobalFree, DATA_S_SAMEFORMATETC, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP,
    DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC, DV_E_TYMED, E_NOTIMPL, E_POINTER, HGLOBAL, HWND,
    OLE_E_ADVISENOTSUPPORTED, POINT, POINTL, S_OK,
};
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{
    IAdviseSink, IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, DATADIR_GET,
    DVASPECT_CONTENT, FORMATETC, STGMEDIUM, STGMEDIUM_0, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop,
    ReleaseStgMedium, RevokeDragDrop, CF_HDROP, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY,
    DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_LBUTTON};
use windows::Win32::UI::Shell::SHCreateStdEnumFmtEtc;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use crate::winfmt;

/// The registered "HTML Format" clipboard format.
fn html_format() -> Option<u16> {
    formats::Html::new().and_then(|h| u16::try_from(h.code()).ok())
}

/// A range's two formats, as the clipboard and dragged data carry them:
/// CF_UNICODETEXT (UTF-16, NUL-terminated) and "HTML Format" (CF_HTML,
/// with a NUL after it that its offsets do not count).
fn range_formats(tsv: &str, html: &str) -> Vec<(u16, Vec<u8>)> {
    let mut out = vec![(CF_UNICODETEXT.0, winfmt::utf16z(tsv))];
    if let Some(cf) = html_format() {
        let mut b = winfmt::cf_html(html).into_bytes();
        b.push(0);
        out.push((cf, b));
    }
    out
}

/// Copy a range: its TSV and its HTML table on the clipboard, together.
pub fn copy(tsv: &str, html: &str) -> Result<(), String> {
    let _open = Clipboard::new_attempts(10).map_err(|e| format!("the clipboard is busy: {e}"))?;
    raw::empty().map_err(|e| e.to_string())?;
    raw::set_string_with(tsv, NoClear).map_err(|e| e.to_string())?;
    if let Some(cf) = html_format() {
        let mut b = winfmt::cf_html(html).into_bytes();
        b.push(0);
        raw::set_without_clear(u32::from(cf), &b).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The cells on the clipboard: from its HTML table when it has one, else
/// its text read as TSV. `None` when it holds neither.
///
/// shortcut: "HTML Format" that is not UTF-8 (Windows PowerShell 5.1's
/// `Set-Clipboard -AsHtml` writes the ANSI code page) is skipped for the
/// text; decode it as Windows-1252 if such a writer turns up in practice.
pub fn paste() -> Option<Vec<Vec<String>>> {
    let _open = Clipboard::new_attempts(10).ok()?;
    if let Some(cf) = html_format()
        .map(u32::from)
        .filter(|cf| raw::is_format_avail(*cf))
    {
        let mut b = Vec::new();
        if raw::get_vec(cf, &mut b).is_ok() {
            if let Some(cells) = winfmt::html_fragment(&b).and_then(winfmt::html_cells) {
                return Some(cells);
            }
        }
    }
    let mut b = Vec::new();
    raw::get_string(&mut b).ok()?;
    Some(crate::clip::from_tsv(&String::from_utf8(b).ok()?))
}

/// Format `cf` as an HGLOBAL of the content, for any device (the
/// default's target device pointer is null).
fn formatetc(cf: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: cf,
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
        ..FORMATETC::default()
    }
}

/// `bytes` in a new movable global block (what an HGLOBAL medium holds).
fn global(bytes: &[u8]) -> windows::core::Result<HGLOBAL> {
    // SAFETY: a fresh block of `bytes.len()` (at least 1) bytes, locked
    // while we copy into it and unlocked before it is handed out.
    unsafe {
        let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let p = GlobalLock(h);
        if p.is_null() {
            let e = windows::core::Error::from_thread();
            let _ = GlobalFree(Some(h));
            return Err(e);
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), p.cast::<u8>(), bytes.len());
        // Unlocking to a lock count of 0 reports "failure" with no error.
        let _ = GlobalUnlock(h);
        Ok(h)
    }
}

/// What `data` holds in format `cf`, as an HGLOBAL medium.
fn get(data: &IDataObject, cf: u16) -> Option<Vec<u8>> {
    let f = formatetc(cf);
    // SAFETY: `f` outlives the call; GetData hands us a medium we own.
    let mut medium = unsafe { data.GetData(&f) }.ok()?;
    let bytes = if medium.tymed == TYMED_HGLOBAL.0 as u32 {
        // SAFETY: an HGLOBAL medium's `hGlobal` is a live global block; it
        // is locked for the copy only. The medium is released after.
        unsafe {
            let h = medium.u.hGlobal;
            let p = GlobalLock(h);
            let b = (!p.is_null())
                .then(|| std::slice::from_raw_parts(p.cast::<u8>(), GlobalSize(h)).to_vec());
            let _ = GlobalUnlock(h);
            b
        }
    } else {
        None
    };
    // SAFETY: the medium GetData filled, released once.
    unsafe { ReleaseStgMedium(&mut medium) };
    bytes
}

fn offers(data: &IDataObject, cf: u16) -> bool {
    // SAFETY: a valid FORMATETC for the call's duration.
    let hr = unsafe { data.QueryGetData(&formatetc(cf)) };
    hr == S_OK
}

/// What a drop from another application brought.
#[derive(Debug, Clone, PartialEq)]
pub enum Dropped {
    /// Cells (from HTML or text), dropped at this point of the window, in
    /// pixels at the system scale (the app applies its own on top).
    Cells(Point, Vec<Vec<String>>),
    /// A file: open it.
    File(PathBuf),
}

/// What `data` carries for the grid: HTML or text as cells, else a file.
fn read_drop(data: &IDataObject, at: Point) -> Option<Dropped> {
    let html = html_format().and_then(|cf| get(data, cf));
    if let Some(cells) = html
        .as_deref()
        .and_then(winfmt::html_fragment)
        .and_then(winfmt::html_cells)
    {
        return Some(Dropped::Cells(at, cells));
    }
    if let Some(text) = get(data, CF_UNICODETEXT.0).map(|b| winfmt::from_utf16z(&b)) {
        return Some(Dropped::Cells(at, crate::clip::from_tsv(&text)));
    }
    let files = get(data, CF_HDROP.0).map(|b| winfmt::drop_files(&b))?;
    files.into_iter().next().map(|p| Dropped::File(p.into()))
}

thread_local! {
    /// Set while our own range is being dragged: the grid refuses it (a
    /// slip of the pointer would paste it over its neighbors).
    static DRAGGING: Cell<bool> = const { Cell::new(false) };
    /// A drag waiting for its timer: the data, and who hears the outcome.
    static PENDING: RefCell<Option<(IDataObject, oneshot::Sender<bool>)>> =
        const { RefCell::new(None) };
}

/// The window's drop target: it says what it takes as the pointer enters,
/// and sends a drop's cells or file to the app.
#[implement(IDropTarget)]
struct Target {
    hwnd: HWND,
    tx: mpsc::UnboundedSender<Dropped>,
    takes: Cell<bool>,
}

impl Target {
    /// `effect`, the source's allowed effects on entry, as our answer.
    fn answer(&self, effect: *mut DROPEFFECT) -> windows::core::Result<()> {
        // SAFETY: OLE passes a valid in/out DROPEFFECT; null is refused.
        let effect = unsafe { effect.as_mut() }.ok_or(windows::core::Error::from(E_POINTER))?;
        *effect = if self.takes.get() && effect.contains(DROPEFFECT_COPY) {
            DROPEFFECT_COPY
        } else {
            DROPEFFECT_NONE
        };
        Ok(())
    }

    /// A screen point as a point of the window, at the system scale.
    fn client(&self, pt: &POINTL) -> Point {
        let mut p = POINT { x: pt.x, y: pt.y };
        // SAFETY: plain Win32 calls on our window with a local POINT.
        let dpi = unsafe {
            let _ = ScreenToClient(self.hwnd, &mut p);
            GetDpiForWindow(self.hwnd)
        };
        let scale = if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 };
        Point::new(p.x as f32 / scale, p.y as f32 / scale)
    }
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let takes = data.as_ref().is_some_and(|d| {
            let html = html_format().is_some_and(|cf| offers(d, cf));
            html || offers(d, CF_UNICODETEXT.0) || offers(d, CF_HDROP.0)
        });
        self.takes.set(takes && !DRAGGING.get());
        self.answer(effect)
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        self.answer(effect)
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        self.takes.set(false);
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        self.answer(effect)?;
        if self.takes.replace(false) {
            if let Some(d) = data.as_ref().and_then(|d| read_drop(d, self.client(pt))) {
                let _ = self.tx.unbounded_send(d);
            }
        }
        Ok(())
    }
}

/// The drops the window's target sends, process-wide: there is one main
/// window, and the subscription that reads them starts before it opens.
static DROPS: LazyLock<(mpsc::UnboundedSender<Dropped>, Mutex<Option<DropRx>>)> =
    LazyLock::new(|| {
        let (tx, rx) = mpsc::unbounded();
        (tx, Mutex::new(Some(rx)))
    });

type DropRx = mpsc::UnboundedReceiver<Dropped>;

/// What other applications drop on the window, once (a second call gets
/// an empty stream).
pub fn drops() -> impl futures::Stream<Item = Dropped> {
    let rx = DROPS.1.lock().ok().and_then(|mut r| r.take());
    futures::StreamExt::flatten(futures::stream::iter(rx))
}

/// Make window `id`'s drop target ours, its drops sent to [`drops`]: a
/// task that says whether it took. Call it once the window is open (a
/// handle asked for earlier is dropped unanswered).
pub fn take_drops(id: window::Id) -> Task<Result<(), String>> {
    let tx = DROPS.0.clone();
    window::run_with_handle(id, move |handle| {
        let window::raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() else {
            return Err("not a Win32 window".to_string());
        };
        let hwnd = HWND(h.hwnd.get() as *mut c_void);
        let target: IDropTarget = Target {
            hwnd,
            tx,
            takes: Cell::new(false),
        }
        .into();
        // SAFETY: on the UI thread that owns `hwnd`, where winit called
        // OleInitialize. winit's own target (files only) goes first; OLE
        // holds a reference to ours until winit revokes it on WM_DESTROY.
        unsafe {
            let _ = RevokeDragDrop(hwnd);
            RegisterDragDrop(hwnd, &target).map_err(|e| e.to_string())
        }
    })
}

/// Data offered to a drop: fixed formats, each as an HGLOBAL.
#[implement(IDataObject)]
struct Offer {
    formats: Vec<(u16, Vec<u8>)>,
}

impl Offer {
    fn find(&self, f: *const FORMATETC) -> Result<&[u8], HRESULT> {
        // SAFETY: OLE passes a valid FORMATETC; null is refused.
        let f = unsafe { f.as_ref() }.ok_or(E_POINTER)?;
        let (_, bytes) = self
            .formats
            .iter()
            .find(|(cf, _)| *cf == f.cfFormat)
            .ok_or(DV_E_FORMATETC)?;
        if f.tymed & TYMED_HGLOBAL.0 as u32 == 0 {
            return Err(DV_E_TYMED);
        }
        Ok(bytes)
    }
}

impl IDataObject_Impl for Offer_Impl {
    fn GetData(&self, f: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let bytes = self.find(f)?;
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 {
                hGlobal: global(bytes)?,
            },
            pUnkForRelease: ManuallyDrop::new(None),
        })
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn QueryGetData(&self, f: *const FORMATETC) -> HRESULT {
        self.find(f).map_or_else(|e| e, |_| S_OK)
    }

    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
        DATA_S_SAMEFORMATETC
    }

    fn SetData(
        &self,
        _: *const FORMATETC,
        _: *const STGMEDIUM,
        _: BOOL,
    ) -> windows::core::Result<()> {
        Err(E_NOTIMPL.into())
    }

    fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(E_NOTIMPL.into());
        }
        let all: Vec<FORMATETC> = self.formats.iter().map(|(cf, _)| formatetc(*cf)).collect();
        // SAFETY: the shell copies the array into the enumerator it returns.
        unsafe { SHCreateStdEnumFmtEtc(&all) }
    }

    fn DAdvise(
        &self,
        _: *const FORMATETC,
        _: u32,
        _: Ref<IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(OLE_E_ADVISENOTSUPPORTED.into())
    }
}

/// The range a drag carries: its text and its HTML.
fn offer(tsv: &str, html: &str) -> IDataObject {
    Offer {
        formats: range_formats(tsv, html),
    }
    .into()
}

/// The drag's source: it ends the drag when the button is let go (a drop)
/// or Escape is pressed (a cancel), with the system's cursors.
#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if !keys.contains(MK_LBUTTON) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drag a range out of the window: answers whether another application
/// took it. Call it on the UI thread. The drag starts from a thread timer,
/// on the next turn of the message loop, not inside the event handler that
/// asked for it: its modal loop then runs as a window resize's does, and
/// the window keeps drawing. It does not start when the button is already
/// up.
pub fn drag_out(tsv: &str, html: &str) -> impl std::future::Future<Output = bool> + Send + 'static {
    let (tx, rx) = oneshot::channel();
    let pending = PENDING.with_borrow_mut(|p| p.replace((offer(tsv, html), tx)));
    drop(pending);
    // SAFETY: a thread timer (no window) with a callback that kills it.
    if unsafe { SetTimer(None, 0, 0, Some(start_drag)) } == 0 {
        PENDING.with_borrow_mut(Option::take);
    }
    async move { rx.await.unwrap_or(false) }
}

/// The drag's timer fired: run the drag, once.
unsafe extern "system" fn start_drag(_: HWND, _: u32, id: usize, _: u32) {
    // SAFETY: our own thread timer, killed once.
    let _ = unsafe { KillTimer(None, id) };
    let Some((data, done)) = PENDING.with_borrow_mut(Option::take) else {
        return;
    };
    // SAFETY: reads this thread's view of the mouse button.
    let pressed = unsafe { GetKeyState(i32::from(VK_LBUTTON.0)) } < 0;
    let _ = done.send(pressed && run_drag(&data));
}

fn run_drag(data: &IDataObject) -> bool {
    let source: IDropSource = Source.into();
    let mut effect = DROPEFFECT_NONE;
    DRAGGING.set(true);
    // SAFETY: on the UI thread, where winit called OleInitialize; `data`
    // and `source` outlive the call, which returns when the drag ends.
    let hr = unsafe { DoDragDrop(data, &source, DROPEFFECT_COPY, &mut effect) };
    DRAGGING.set(false);
    hr == DRAGDROP_S_DROP && effect.contains(DROPEFFECT_COPY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Ole::OleInitialize;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, DispatchMessageW, PeekMessageW, MSG, PM_REMOVE,
        WINDOW_EX_STYLE, WS_POPUP,
    };

    fn ole() {
        // SAFETY: once per test thread; a repeat answers S_FALSE.
        let _ = unsafe { OleInitialize(None) };
    }

    fn rows(r: &[&[&str]]) -> Vec<Vec<String>> {
        r.iter()
            .map(|r| r.iter().map(|s| s.to_string()).collect())
            .collect()
    }

    /// The formats `data` lists, in order.
    fn listed(data: &IDataObject) -> Vec<u16> {
        // SAFETY: plain COM calls on live objects with local buffers.
        unsafe {
            let e = data.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
            let mut out = Vec::new();
            let mut f = [FORMATETC::default()];
            while e.Next(&mut f, None) == S_OK {
                assert_eq!(f[0].tymed, TYMED_HGLOBAL.0 as u32);
                out.push(f[0].cfFormat);
            }
            out
        }
    }

    #[test]
    fn a_dragged_range_offers_text_and_html() {
        ole();
        let html = crate::clip::to_html(&rows(&[&["", "Q1"], &["Région", "1,5"]]), true);
        let data = offer("1,5\tGröße", &html);
        let cf = html_format().unwrap();
        assert_eq!(listed(&data), vec![CF_UNICODETEXT.0, cf]);
        let text = get(&data, CF_UNICODETEXT.0).unwrap();
        assert_eq!(winfmt::from_utf16z(&text), "1,5\tGröße");
        let h = get(&data, cf).unwrap();
        let frag = winfmt::html_fragment(&h).unwrap();
        assert_eq!(frag, html);
        assert_eq!(winfmt::html_cells(frag), Some(rows(&[&["1,5"]])));
        assert!(!offers(&data, CF_HDROP.0));
        // Only HGLOBAL media.
        let mut f = formatetc(CF_UNICODETEXT.0);
        f.tymed = 4; // TYMED_ISTREAM
                     // SAFETY: a valid FORMATETC.
        assert_eq!(unsafe { data.QueryGetData(&f) }, DV_E_TYMED);
    }

    /// A hidden window at (100, 200) on the screen, as the drop target's.
    struct Win(HWND);

    impl Win {
        fn new() -> Win {
            // SAFETY: a plain popup of a system class, destroyed on drop.
            let h = unsafe {
                CreateWindowExW(
                    WINDOW_EX_STYLE(0),
                    windows::core::w!("STATIC"),
                    windows::core::w!("improv drop test"),
                    WS_POPUP,
                    100,
                    200,
                    400,
                    300,
                    None,
                    None,
                    None,
                    None,
                )
            }
            .unwrap();
            Win(h)
        }

        fn scale(&self) -> f32 {
            // SAFETY: our live window.
            let dpi = unsafe { GetDpiForWindow(self.0) };
            if dpi == 0 {
                1.0
            } else {
                dpi as f32 / 96.0
            }
        }
    }

    impl Drop for Win {
        fn drop(&mut self) {
            // SAFETY: our window, destroyed once.
            let _ = unsafe { DestroyWindow(self.0) };
        }
    }

    /// Drag `data` over the target and drop it at `pt`: the effect it
    /// answered on entry, and what it sent.
    fn drop_on(
        target: &IDropTarget,
        rx: &mut mpsc::UnboundedReceiver<Dropped>,
        data: &IDataObject,
        pt: POINTL,
    ) -> (DROPEFFECT, Option<Dropped>) {
        let mut entered = DROPEFFECT_COPY;
        let mut over = DROPEFFECT_COPY;
        let mut dropped = DROPEFFECT_COPY;
        // SAFETY: the target's own methods, with live arguments.
        unsafe {
            target
                .DragEnter(data, MK_LBUTTON, pt, &mut entered)
                .unwrap();
            target.DragOver(MK_LBUTTON, pt, &mut over).unwrap();
            target
                .Drop(data, MODIFIERKEYS_FLAGS(0), pt, &mut dropped)
                .unwrap();
        }
        assert_eq!(entered, over);
        assert_eq!(entered, dropped);
        (entered, rx.try_recv().ok())
    }

    #[test]
    fn the_drop_target_takes_html_then_text_then_files() {
        ole();
        let win = Win::new();
        let (tx, mut rx) = mpsc::unbounded();
        let target: IDropTarget = Target {
            hwnd: win.0,
            tx,
            takes: Cell::new(false),
        }
        .into();
        let s = win.scale();
        // 30 px right of and 40 below the window's corner, in screen pixels.
        let pt = POINTL {
            x: 100 + (30.0 * s) as i32,
            y: 200 + (40.0 * s) as i32,
        };
        let at = Point::new(30.0, 40.0);
        // Text and HTML that disagree: the HTML wins.
        let cf = html_format().unwrap();
        let both: IDataObject = Offer {
            formats: vec![
                (CF_UNICODETEXT.0, winfmt::utf16z("1\t2")),
                (
                    cf,
                    winfmt::cf_html("<table><tr><td>7</td><td>8</td></tr></table>").into_bytes(),
                ),
            ],
        }
        .into();
        let (e, d) = drop_on(&target, &mut rx, &both, pt);
        assert_eq!(e, DROPEFFECT_COPY);
        assert_eq!(d, Some(Dropped::Cells(at, rows(&[&["7", "8"]]))));
        // Text alone, as Notepad drags it.
        let text: IDataObject = Offer {
            formats: vec![(CF_UNICODETEXT.0, winfmt::utf16z("5\t6\n7\t8"))],
        }
        .into();
        let (_, d) = drop_on(&target, &mut rx, &text, pt);
        assert_eq!(
            d,
            Some(Dropped::Cells(at, rows(&[&["5", "6"], &["7", "8"]])))
        );
        // A file from Explorer opens.
        let files: IDataObject = Offer {
            formats: vec![(CF_HDROP.0, winfmt::hdrop(&[r"C:\m\sales.db"]))],
        }
        .into();
        let (_, d) = drop_on(&target, &mut rx, &files, pt);
        assert_eq!(d, Some(Dropped::File(r"C:\m\sales.db".into())));
        // Something else: refused, and nothing sent.
        let other: IDataObject = Offer {
            formats: vec![(2, vec![1, 2, 3])],
        }
        .into();
        assert_eq!(
            drop_on(&target, &mut rx, &other, pt),
            (DROPEFFECT_NONE, None)
        );
        // What a drag out of improv-cosmic offers reads back as its values.
        let html = crate::clip::to_html(&rows(&[&["", "Q1"], &["2025", "9"]]), true);
        let (_, d) = drop_on(&target, &mut rx, &offer("9", &html), pt);
        assert_eq!(d, Some(Dropped::Cells(at, rows(&[&["9"]]))));
        // Our own range, mid-drag: refused.
        DRAGGING.set(true);
        assert_eq!(
            drop_on(&target, &mut rx, &text, pt),
            (DROPEFFECT_NONE, None)
        );
        DRAGGING.set(false);
        // A source that allows no copy: refused.
        let mut effect = DROPEFFECT(2); // DROPEFFECT_MOVE
                                        // SAFETY: the target's own method, with live arguments.
        unsafe {
            target
                .DragEnter(&text, MK_LBUTTON, pt, &mut effect)
                .unwrap()
        };
        assert_eq!(effect, DROPEFFECT_NONE);
    }

    // `run_drag` itself (DoDragDrop) is not tested here: with no input
    // desktop (ssh's session 0) its modal loop never asks the source to
    // continue, so it waits for ever. Checked by hand on a desktop (the
    // checklist in docs/reviews/patches/window_clipboard-windows-formats.md).

    #[test]
    fn a_drag_with_the_button_up_never_starts() {
        ole();
        let done = drag_out("1", "<table><tr><td>1</td></tr></table>");
        let mut done = Box::pin(done);
        let mut msg = MSG::default();
        for _ in 0..200 {
            // SAFETY: this thread's message queue, into a local MSG.
            unsafe {
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    DispatchMessageW(&msg);
                }
            }
            if let Some(took) = futures::FutureExt::now_or_never(&mut done) {
                assert!(!took);
                assert!(PENDING.with_borrow(Option::is_none));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the drag's timer never fired");
    }
}
