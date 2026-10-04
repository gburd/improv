//! `improv-tui`: a VisiCalc-style pivot grid over a Mentat-backed Improv model.
//!
//! Usage: `improv-tui <model.db>`
//!
//! The binary is a thin shell: [`run`] owns the real terminal (raw mode, alt
//! screen, panic hook) and feeds `crossterm` events into [`event_loop`], which
//! is generic over the ratatui backend and the event source so tests can drive
//! it with a `TestBackend` and a scripted `Vec<Event>`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

mod app;
mod ui;

use app::App;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use improv_storage_mentat::ModelStore;
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use std::io;

// Coverage exclusion: process entry point (argv + `process::exit`); its logic
// lives in `load`/`save`, which are tested.
#[cfg_attr(coverage_nightly, coverage(off))]
fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: improv-tui <model.db>");
        std::process::exit(1);
    };
    // Load the model BEFORE touching the terminal, so errors print cleanly.
    if let Err(e) = load(&path).and_then(|app| run(app, &path)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Open the model at `path` and build the app state.
fn load(path: &str) -> Result<App, String> {
    let model = ModelStore::open(path)
        .and_then(|mut s| s.load_model())
        .map_err(|e| format!("cannot open model '{path}': {e}"))?;
    App::new(model)
}

/// Autosave the (possibly edited) model back to `path` on clean exit; edits
/// are held in memory during the session.
fn save(app: &App, path: &str) -> Result<(), String> {
    ModelStore::open(path)
        .and_then(|mut s| s.save_model(&app.model))
        .map_err(|e| format!("autosave failed: {e}"))
}

/// Enter the alt screen, run the loop, and always restore the terminal — even
/// on error or panic — then autosave.
// Coverage exclusion: needs a real tty (raw mode, alt screen, mouse capture,
// panic-hook restore); everything it calls is covered via `event_loop`.
#[cfg_attr(coverage_nightly, coverage(off))]
fn run(mut app: App, path: &str) -> Result<(), String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture).map_err(|e| e.to_string())?;

    // Restore the terminal if the run loop panics, then re-raise.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        prev_hook(info);
    }));

    let result = Terminal::new(CrosstermBackend::new(io::stdout()))
        .map_err(|e| e.to_string())
        .and_then(|mut term| {
            event_loop(
                &mut term,
                &mut app,
                path,
                std::iter::repeat_with(event::read),
            )
        });

    // Always restore on the normal path too.
    let _ = restore();
    result?;
    save(&app, path)
}

/// Undo terminal setup. Safe to call multiple times.
// Coverage exclusion: real-terminal teardown (see `run`).
#[cfg_attr(coverage_nightly, coverage(off))]
fn restore() -> io::Result<()> {
    execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()
}

/// Draw, then handle one event, until the app quits or `events` runs dry.
fn event_loop<B: Backend>(
    term: &mut Terminal<B>,
    app: &mut App,
    path: &str,
    events: impl IntoIterator<Item = io::Result<Event>>,
) -> Result<(), String> {
    let mut events = events.into_iter();
    while !app.should_quit {
        term.draw(|f| ui::render(f, app))
            .map_err(|e| e.to_string())?;
        let Some(ev) = events.next() else { break };
        handle_event(app, ev.map_err(|e| e.to_string())?, path);
    }
    Ok(())
}

/// Dispatch one terminal event by mode (edit / command prompt / normal).
fn handle_event(app: &mut App, ev: Event, path: &str) {
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            if app.edit.is_some() {
                edit_key(app, k.code);
            } else if app.command.is_some() {
                command_key(app, k.code);
            } else {
                normal_key(app, k.code, path);
            }
        }
        // Mouse: left-click selects the cell under the pointer (hit-tested
        // against the last-rendered grid geometry stored on `app`).
        Event::Mouse(m) if app.edit.is_none() && app.command.is_none() => {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                app.click_at(m.column, m.row);
            }
        }
        _ => {}
    }
}

/// Key handling while a cell edit is in progress.
fn edit_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => app.commit_edit(),
        KeyCode::Esc => app.cancel_edit(),
        KeyCode::Backspace => {
            if let Some(buf) = app.edit.as_mut() {
                buf.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some(buf) = app.edit.as_mut() {
                buf.push(c);
            }
        }
        _ => {}
    }
}

/// Key handling while the import-csv/export-csv command prompt is active.
fn command_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Enter => app.commit_command(),
        KeyCode::Esc => app.cancel_command(),
        KeyCode::Backspace => {
            if let Some((_, buf)) = app.command.as_mut() {
                buf.pop();
            }
        }
        KeyCode::Char(c) => {
            if let Some((_, buf)) = app.command.as_mut() {
                buf.push(c);
            }
        }
        _ => {}
    }
}

/// Key handling in normal (navigation) mode.
fn normal_key(app: &mut App, code: KeyCode, path: &str) {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('e') | KeyCode::Enter => app.begin_edit(),
        KeyCode::Tab | KeyCode::Char('m') => app.next_measure(),
        KeyCode::Up => app.move_cursor(-1, 0),
        KeyCode::Down => app.move_cursor(1, 0),
        KeyCode::Left => app.move_cursor(0, -1),
        KeyCode::Right => app.move_cursor(0, 1),
        KeyCode::Char('[') => app.page(-1),
        KeyCode::Char(']') => app.page(1),
        KeyCode::Char('p') => app.pivot(),
        // Filters: 'f' hides/shows the row item under the cursor; 'F' clears.
        KeyCode::Char('f') => filter_row_under_cursor(app),
        KeyCode::Char('F') => app.clear_filters(),
        // Views: 'S' saves the current layout; 'v' cycles saved views.
        KeyCode::Char('S') => save_current_view(app, path),
        KeyCode::Char('v') => app.cycle_view(),
        // CSV import/export: 'I' opens the import-csv prompt, 'E' export-csv.
        KeyCode::Char('I') => app.begin_import_csv(),
        KeyCode::Char('E') => app.begin_export_csv(),
        KeyCode::Esc => app.status = None,
        _ => {}
    }
}

/// Toggle the visibility of the row item under the cursor within its (row)
/// category. Repeated 'f' on different rows narrows the row axis to a subset.
fn filter_row_under_cursor(app: &mut App) {
    let (Some((cat, _)), Some((item, _))) = (
        app.grid.row_cat.clone(),
        app.grid.rows.get(app.cursor_row).cloned(),
    ) else {
        app.status = Some("no row category to filter".into());
        return;
    };
    app.toggle_filter_item(cat, item);
}

/// Save the current layout as a generated "view N" name and persist to `path`.
/// (Inline text entry is intentionally omitted; the view can be renamed via a
/// future dialog. Documented simplification.)
fn save_current_view(app: &mut App, path: &str) {
    let n = app.model.views.len() + 1;
    app.save_view(&format!("view {n}"), Some(path));
}
