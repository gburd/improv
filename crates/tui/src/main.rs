//! `improv-tui`: a VisiCalc-style pivot grid over an Improv model.
//!
//! Usage: `improv-tui <model.db | postgres://…?model=NAME>`
//!
//! The binary is a thin shell: [`run`] owns the real terminal (raw mode, alt
//! screen, panic hook) and feeds `crossterm` events into [`event_loop`], which
//! is generic over the ratatui backend and the event source so tests can drive
//! it with a `TestBackend` and a scripted `Vec<Event>`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

mod app;
mod fnhelp;
mod ui;

use app::App;
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use improv_store::Store;
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
    let (model, save_token) = Store::open(path)
        .and_then(|mut s| Ok((s.load_model()?, s.save_token())))
        .map_err(|e| format!("cannot open model '{path}': {e}"))?;
    let mut app = App::new(model)?;
    app.save_token = save_token;
    Ok(app)
}

/// Autosave the (possibly edited) model back to `path` on clean exit; edits
/// are held in memory during the session.
fn save(app: &App, path: &str) -> Result<(), String> {
    Store::open(path)
        .and_then(|mut s| {
            s.set_save_token(&app.save_token);
            s.save_model(&app.model)
        })
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
        Event::Key(k) if k.kind != KeyEventKind::Release && app.help.is_some() => {
            help_key(app, k.code)
        }
        Event::Key(k) if k.kind != KeyEventKind::Release => match (k.code, &mut app.edit) {
            (KeyCode::F(1), _) => app.help = Some(fnhelp::FnHelp::new()),
            (KeyCode::Enter, Some(_)) => app.commit_edit(),
            (KeyCode::Esc, Some(_)) => app.cancel_edit(),
            (code, Some(buf)) => line_key(buf, code),
            (code, None) => match (code, &mut app.command) {
                (KeyCode::Enter, Some(_)) => app.commit_command(),
                (KeyCode::Esc, Some(_)) => app.cancel_command(),
                (code, Some((_, buf))) => line_key(buf, code),
                (_, None) => normal_key(app, k, path),
            },
        },
        // Mouse: left-click selects the cell under the pointer (hit-tested
        // against the last-rendered grid geometry stored on `app`).
        Event::Mouse(m) if app.edit.is_none() && app.command.is_none() && app.help.is_none() => {
            if let MouseEventKind::Down(MouseButton::Left) = m.kind {
                app.click_at(m.column, m.row);
            }
        }
        _ => {}
    }
}

/// Line-editing keys shared by the cell edit and the command prompt.
fn line_key(buf: &mut String, code: KeyCode) {
    match code {
        KeyCode::Backspace => {
            buf.pop();
        }
        KeyCode::Char(c) => buf.push(c),
        _ => {}
    }
}

/// Keys while the function help overlay is open: type to search, move the
/// selection, Enter inserts `NAME(` into an edit in progress, Esc closes.
fn help_key(app: &mut App, code: KeyCode) {
    let Some(h) = &mut app.help else { return };
    match code {
        KeyCode::Esc => app.help = None,
        KeyCode::Enter => {
            if let (Some(d), Some(buf)) = (h.current(), &mut app.edit) {
                buf.push_str(d.name);
                buf.push('(');
            }
            app.help = None;
        }
        KeyCode::Up => h.step(-1),
        KeyCode::Down => h.step(1),
        KeyCode::PageUp => h.page_by(-1),
        KeyCode::PageDown => h.page_by(1),
        KeyCode::Backspace => h.pop(),
        KeyCode::Char(c) => h.push(c),
        _ => {}
    }
}

/// Key handling in normal (navigation) mode.
fn normal_key(app: &mut App, k: KeyEvent, path: &str) {
    let shift = k.modifiers.contains(KeyModifiers::SHIFT);
    match k.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('e') | KeyCode::Enter => app.begin_edit(),
        KeyCode::Tab | KeyCode::Char('m') => app.next_measure(),
        // Shift+arrow grows a range selection (its Sum shows in the status).
        KeyCode::Up => app.step(-1, 0, shift),
        KeyCode::Down => app.step(1, 0, shift),
        KeyCode::Left => app.step(0, -1, shift),
        KeyCode::Right => app.step(0, 1, shift),
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
        // 'R' reverts to the saved model (e.g. after another writer saved).
        KeyCode::Char('R') => revert(app, path),
        KeyCode::Char('?') => app.help = Some(fnhelp::FnHelp::new()),
        KeyCode::Esc => app.status = None,
        _ => {}
    }
}

/// Reload the model from `path`, dropping this session's unsaved edits. The
/// way out of a save conflict: the quit autosave refuses to clobber another
/// writer, so revert, pick up their changes, and redo yours. On failure the
/// session is untouched.
fn revert(app: &mut App, path: &str) {
    match load(path) {
        Ok(fresh) => {
            *app = fresh;
            app.status = Some("reverted to the saved model".into());
        }
        Err(e) => app.status = Some(format!("revert failed: {e}")),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{revenue_model, tmp_path, view};
    use crossterm::event::MouseEvent;
    use improv_core_model::{CategoryId, Coordinate, ItemId, MeasureId, Value};
    use ratatui::backend::TestBackend;

    fn key(code: KeyCode) -> io::Result<Event> {
        Ok(Event::Key(KeyEvent::new(code, KeyModifiers::NONE)))
    }
    fn chars(s: &str) -> Vec<io::Result<Event>> {
        s.chars().map(|c| key(KeyCode::Char(c))).collect()
    }
    fn click(kind: MouseEventKind, column: u16, row: u16) -> io::Result<Event> {
        Ok(Event::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }))
    }
    fn load_err(p: &str) -> String {
        load(p).err().expect("load should fail")
    }
    /// Run `events` against `app` on a 100x30 test terminal.
    fn run_events(app: &mut App, path: &str, events: Vec<io::Result<Event>>) -> Result<(), String> {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        event_loop(&mut term, app, path, events)
    }

    #[test]
    fn typing_an_edit_updates_the_model() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        let mut ev = vec![key(KeyCode::Enter), key(KeyCode::Backspace)];
        ev.extend(chars("x"));
        ev.extend([key(KeyCode::Backspace), key(KeyCode::Backspace)]);
        ev.extend(chars("7"));
        ev.extend([key(KeyCode::F(5)), key(KeyCode::Enter)]);
        run_events(&mut app, "", ev).unwrap();
        let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
        assert_eq!(
            app.model.input(MeasureId(101), &at),
            Some(&Value::Number(17.0))
        );
        // Esc cancels an edit without writing.
        run_events(
            &mut app,
            "",
            vec![key(KeyCode::Char('e')), key(KeyCode::Esc)],
        )
        .unwrap();
        assert!(app.edit.is_none());
    }

    #[test]
    fn navigation_keys_and_release_events() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        let mut release = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        let ev = vec![
            key(KeyCode::Down),
            key(KeyCode::Right),
            Ok(Event::Key(release)), // ignored
            Ok(Event::FocusGained),  // ignored
        ];
        run_events(&mut app, "", ev).unwrap();
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
        run_events(&mut app, "", vec![key(KeyCode::Up), key(KeyCode::Left)]).unwrap();
        assert_eq!((app.cursor_row, app.cursor_col), (0, 0));
        // Tab and m both cycle the measure; q quits before later events run.
        let before = app.selected;
        let ev = vec![
            key(KeyCode::Tab),
            key(KeyCode::Char('m')),
            key(KeyCode::Char('q')),
            key(KeyCode::Tab),
        ];
        run_events(&mut app, "", ev).unwrap();
        assert!(app.should_quit);
        assert_eq!(app.selected, (before + 2) % app.measures.len());
    }

    #[test]
    fn layout_keys_page_pivot_filter_and_status() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        let ev = vec![
            key(KeyCode::Char('[')),
            key(KeyCode::Char(']')),
            key(KeyCode::Esc),
        ];
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.status, None, "Esc clears the paging message");
        run_events(&mut app, "", chars("f")).unwrap();
        assert_eq!(app.grid.n_rows(), 1);
        run_events(&mut app, "", chars("F")).unwrap();
        assert_eq!(app.grid.n_rows(), 2);
        run_events(&mut app, "", chars("p")).unwrap();
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Product");
        run_events(&mut app, "", chars("v")).unwrap();
        assert!(app.status.as_deref().unwrap().starts_with("no saved views"));
        // A scalar measure has no row category to filter.
        let mut m = improv_core_model::Model::new();
        m.add_measure(improv_core_model::Measure {
            id: MeasureId(1),
            name: improv_core_model::Name("S".into()),
            value_type: improv_core_model::ValueType::Number,
            categories: vec![],
            kind: improv_core_model::MeasureKind::Input,
            description: None,
        });
        let mut scalar = App::new(m).unwrap();
        run_events(&mut scalar, "", chars("f")).unwrap();
        assert_eq!(scalar.status.as_deref(), Some("no row category to filter"));
    }

    #[test]
    fn mouse_selects_only_in_normal_mode() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        // Status box is 4 rows here (title + 2 help + border); data starts
        // after the grid border + header. Click well inside row 1, col 1.
        run_events(&mut app, "", vec![]).unwrap();
        let g = app.grid_geom.unwrap();
        let (x, y) = (g.x0 + g.col_w + g.spacing + 1, g.y0 + 1);
        let ev = vec![
            click(MouseEventKind::Moved, x, y),
            key(KeyCode::Char('e')),
            click(MouseEventKind::Down(MouseButton::Left), x, y), // ignored while editing
            key(KeyCode::Esc),
        ];
        run_events(&mut app, "", ev).unwrap();
        assert_eq!((app.cursor_row, app.cursor_col), (0, 0));
        run_events(
            &mut app,
            "",
            vec![click(MouseEventKind::Down(MouseButton::Left), x, y)],
        )
        .unwrap();
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
    }

    #[test]
    fn shift_arrows_extend_the_selection() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        let shift = |c| Ok(Event::Key(KeyEvent::new(c, KeyModifiers::SHIFT)));
        let ev = vec![
            shift(KeyCode::Down),
            shift(KeyCode::Right),
            shift(KeyCode::Up),
            shift(KeyCode::Left),
        ];
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.anchor, Some((0, 0)));
        run_events(
            &mut app,
            "",
            vec![shift(KeyCode::Right), shift(KeyCode::Down)],
        )
        .unwrap();
        assert_eq!(app.selection_sum(), Some(350.0));
        run_events(&mut app, "", vec![key(KeyCode::Left)]).unwrap();
        assert_eq!(app.anchor, None);
    }

    #[test]
    fn command_prompt_keys() {
        let mut app = App::new(revenue_model()).unwrap();
        let mut ev = vec![key(KeyCode::Char('E'))];
        ev.extend(chars("xy"));
        ev.extend([
            key(KeyCode::Backspace),
            key(KeyCode::Home),
            key(KeyCode::Enter),
        ]);
        run_events(&mut app, "", ev).unwrap();
        assert!(app.command.is_none());
        assert!(app.status.as_deref().unwrap().starts_with("export-csv:"));
        run_events(
            &mut app,
            "",
            vec![key(KeyCode::Char('I')), key(KeyCode::Esc)],
        )
        .unwrap();
        assert!(app.command.is_none());
    }

    #[test]
    fn help_overlay_keys() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        let mut ev = vec![key(KeyCode::Char('?'))];
        ev.extend(chars("pmtx"));
        ev.extend([
            key(KeyCode::Backspace),
            key(KeyCode::Down),
            key(KeyCode::Up),
            key(KeyCode::PageDown),
            key(KeyCode::PageUp),
            key(KeyCode::Right),                                    // ignored
            click(MouseEventKind::Down(MouseButton::Left), 40, 20), // ignored
        ]);
        run_events(&mut app, "", ev).unwrap();
        let h = app.help.as_ref().expect("? opens the overlay");
        assert_eq!(h.query, "pmt");
        assert_eq!(h.current().unwrap().name, "PMT");
        // Enter with no edit in progress just closes; Esc closes too.
        run_events(&mut app, "", vec![key(KeyCode::Enter)]).unwrap();
        assert!(app.help.is_none() && app.edit.is_none());
        run_events(&mut app, "", vec![key(KeyCode::F(1)), key(KeyCode::Esc)]).unwrap();
        assert!(app.help.is_none());
        // F1 mid-edit: Enter inserts the selected function's `NAME(`.
        let mut ev = vec![key(KeyCode::Char('e')), key(KeyCode::F(1))];
        ev.extend(chars("pmt"));
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, "", ev).unwrap();
        assert!(app.help.is_none());
        assert_eq!(app.edit.as_deref(), Some("100PMT("));
    }

    #[test]
    fn event_errors_stop_the_loop() {
        let mut app = App::new(revenue_model()).unwrap();
        let err = run_events(&mut app, "", vec![Err(io::Error::other("boom"))]);
        assert_eq!(err, Err("boom".into()));
    }

    #[test]
    fn load_save_round_trip_and_save_view_key() {
        let path = tmp_path("tui-main.db");
        let p = path.to_str().unwrap();
        assert!(load_err(p).contains("no measures"), "fresh store is empty");
        let mut app = App::new(revenue_model()).unwrap();
        save(&app, p).unwrap();
        run_events(&mut app, p, chars("S")).unwrap();
        assert_eq!(app.status.as_deref(), Some("saved view 'view 1'"));
        let loaded = load(p).unwrap();
        assert_eq!(loaded.model.views.len(), 1);
        assert!(load_err("/nonexistent/dir/x.db").starts_with("cannot open model"));
        assert!(save(&app, "/nonexistent/dir/x.db")
            .unwrap_err()
            .starts_with("autosave failed"));
    }

    /// Two sessions on one store: the second's autosave conflicts after the
    /// first saved; 'R' reverts it to the saved model and its next save works.
    #[test]
    fn revert_key_recovers_from_a_save_conflict() {
        let path = tmp_path("tui-revert.db");
        let p = path.to_str().unwrap();
        save(&App::new(revenue_model()).unwrap(), p).unwrap();
        let mut a = load(p).unwrap();
        let mut b = load(p).unwrap();
        run_events(&mut a, p, chars("S")).unwrap();
        b.model.views.clear();
        b.model
            .add_view(b.build_view(improv_core_model::ViewId(77), "mine"));
        let e = save(&b, p).unwrap_err();
        assert!(e.contains("conflict") || e.contains("changed"), "{e}");
        run_events(&mut b, p, chars("R")).unwrap();
        assert_eq!(b.status.as_deref(), Some("reverted to the saved model"));
        assert_eq!(b.model.views.len(), 1, "picked up the other session's view");
        save(&b, p).expect("saves after the revert");
        run_events(&mut b, "/nonexistent/dir/x.db", chars("R")).unwrap();
        assert!(b.status.as_deref().unwrap().starts_with("revert failed"));
        assert_eq!(b.model.views.len(), 1, "a failed revert keeps the session");
    }
}
