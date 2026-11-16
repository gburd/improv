//! The console of the terminal improv-gui was started from (Windows only).
//!
//! improv-gui is a Windows GUI-subsystem program (see `main.rs`), so Windows
//! gives it no console: started from Explorer or the Start menu, it opens no
//! console window. Started from a terminal, it attaches to that terminal's
//! console, so that `--help`, `--version` and startup errors print there,
//! and it detaches again once its window is up.
//!
//! It must detach because an attached process receives the console's Ctrl+C
//! and close events. The shell does not wait for a GUI program and returns to
//! its prompt at once, so a Ctrl+C typed at that prompt, or closing the
//! terminal, would end the GUI without its final save.

use std::sync::atomic::{AtomicBool, Ordering};
use windows_sys::Win32::System::Console::{
    AttachConsole, FreeConsole, GetConsoleMode, GetStdHandle, SetStdHandle, ATTACH_PARENT_PROCESS,
    STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};

/// Whether [`attach`] attached to a console that [`detach`] must leave.
static ATTACHED: AtomicBool = AtomicBool::new(false);

/// Attach to the parent process's console, if it has one. Windows points
/// the standard handles at it, except those the parent redirected (to a
/// pipe or a file), which keep writing there. Call once, first in `main`.
pub fn attach() {
    // SAFETY: no pointers. It fails, harmlessly, when the parent has no
    // console (Explorer, the Start menu).
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } != 0 {
        ATTACHED.store(true, Ordering::Relaxed);
    }
}

/// Detach from the console [`attach`] attached to, if any. Call once the
/// window is up; later output to the console is dropped, while a stream
/// the parent redirected keeps working.
pub fn detach() {
    if !ATTACHED.swap(false, Ordering::Relaxed) {
        return;
    }
    for stream in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        let mut mode = 0;
        // SAFETY: GetStdHandle only reads the process's handle table, and
        // GetConsoleMode writes one u32 to `mode`, which outlives the call;
        // it fails for a handle that is not a console's.
        let console = unsafe { GetConsoleMode(GetStdHandle(stream), &mut mode) } != 0;
        if console {
            // SAFETY: a null handle is valid; std's stdout and stderr treat
            // it as no stream and drop what is written. NB: it is cleared
            // because once the console is freed, the next file opened can
            // reuse the stale handle's value, and output would land there.
            unsafe { SetStdHandle(stream, std::ptr::null_mut()) };
        }
    }
    // SAFETY: no pointers; detaches this process from its console.
    unsafe { FreeConsole() };
}
