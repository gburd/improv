//! `improv-tui`: a VisiCalc-style pivot grid over a Mentat-backed Improv model.
//!
//! Usage: `improv-tui <model.db>`

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
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io::{self, Stdout};

fn main() {
    let path = match std::env::args().nth(1) {
        Some(p) => p,
        None => {
            eprintln!("usage: improv-tui <model.db>");
            std::process::exit(1);
        }
    };

    // Load the model BEFORE touching the terminal, so errors print cleanly.
    let model = match ModelStore::open(&path).and_then(|mut s| s.load_model()) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("error: cannot open model '{path}': {e}");
            std::process::exit(1);
        }
    };
    let app = match App::new(model) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = run(app, &path) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Enter the alt screen, run the loop, and always restore the terminal — even
/// on error or panic. On a clean exit the (possibly edited) model is saved back
/// to `path` (autosave-on-quit; edits are held in memory during the session).
fn run(mut app: App, path: &str) -> Result<(), String> {
    enable_raw_mode().map_err(|e| e.to_string())?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture).map_err(|e| e.to_string())?;

    // Restore the terminal if the run loop panics, then re-raise.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        prev_hook(info);
    }));

    let backend = CrosstermBackend::new(io::stdout());
    let result = Terminal::new(backend)
        .map_err(|e| e.to_string())
        .and_then(|mut term| event_loop(&mut term, &mut app));

    // Always restore on the normal path too.
    let _ = restore();
    result?;

    // Autosave the edited model on clean exit (edits live in memory otherwise).
    ModelStore::open(path)
        .and_then(|mut s| s.save_model(&app.model))
        .map_err(|e| format!("autosave failed: {e}"))
}

/// Undo terminal setup. Safe to call multiple times.
fn restore() -> io::Result<()> {
    let mut stdout = io::stdout();
    execute!(stdout, DisableMouseCapture, LeaveAlternateScreen)?;
    disable_raw_mode()
}

fn event_loop(term: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<(), String> {
    while !app.should_quit {
        term.draw(|f| ui::render(f, app))
            .map_err(|e| e.to_string())?;

        match event::read().map_err(|e| e.to_string())? {
            Event::Key(k) if k.kind != KeyEventKind::Release => {
                if app.edit.is_some() {
                    edit_key(app, k.code);
                } else {
                    normal_key(app, k.code);
                }
            }
            // Mouse: left-click selects the cell under the pointer; a click on a
            // page indicator / axis label pivots (handled via cell hit-testing
            // against the last-rendered grid rect stored on `app`).
            Event::Mouse(m) if app.edit.is_none() => {
                if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                    app.click_at(m.column, m.row);
                }
            }
            _ => {}
        }
    }
    Ok(())
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

/// Key handling in normal (navigation) mode.
fn normal_key(app: &mut App, code: KeyCode) {
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
        KeyCode::Esc => app.status = None,
        _ => {}
    }
}
