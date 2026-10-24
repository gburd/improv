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
// Declarations only (the clap derive).
#[cfg_attr(coverage_nightly, coverage(off))]
mod args;
mod complete;
mod diagnose;
mod edit;
mod fnhelp;
mod ui;

use app::App;
use clap::Parser;
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
    let path = args::Cli::parse().model;
    // Load the model BEFORE touching the terminal, so errors print cleanly.
    if let Err(e) = load(&path).and_then(|app| run(app, &path)) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Open the model at `path` and build the app state.
fn load(path: &str) -> Result<App, String> {
    let (model, save_token) = Store::open(path)
        .and_then(|mut s| Ok((s.load_model()?, s.take_save_token())))
        .map_err(|e| format!("cannot open model '{path}': {e}"))?;
    let saved = model.clone();
    let mut app = App::new(model)?;
    app.save_token = save_token;
    app.saved = Some(saved);
    Ok(app)
}

/// Autosave the (possibly edited) model back to `path` on clean exit; edits
/// are held in memory during the session. Writes only what changed.
fn save(app: &mut App, path: &str) -> Result<(), String> {
    app.save(path).map_err(|e| format!("autosave failed: {e}"))
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
        .and_then(|mut term| {
            event_loop(
                &mut term,
                &mut app,
                path,
                std::iter::repeat_with(event::read),
            )
        })
        .map_err(|e| e.to_string());

    // Always restore on the normal path too.
    let _ = restore();
    result?;
    save(&mut app, path)
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
) -> io::Result<()> {
    let mut events = events.into_iter();
    while !app.should_quit {
        term.draw(|f| ui::render(f, app))?;
        let Some(ev) = events.next() else { break };
        handle_event(app, ev?, path);
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
            (code, Some(_)) => app.edit_key(code),
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

/// Line-editing keys for the command prompt (the cell edit has a caret:
/// [`edit::LineEdit::key`]).
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
                buf.insert(&format!("{}(", d.name));
                app.tab_cycle = None;
                app.diagnostic = None; // its span is into the old text
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
        // Building a model: a category and its items, an input measure, a
        // definition (formula or LAMBDA); 'w' saves now.
        KeyCode::Char('c') => app.begin_command(app::CommandKind::Category),
        KeyCode::Char('n') => app.begin_command(app::CommandKind::Input),
        KeyCode::Char('=') => app.begin_define(),
        KeyCode::Char('w') => app.write(path),
        KeyCode::Char('u') => app.undo(),
        KeyCode::Char('r') if k.modifiers.contains(KeyModifiers::CONTROL) => app.redo(),
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
    if app.measures.is_empty() {
        app.status = Some(app::EMPTY.into());
        return;
    }
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
    fn run_events(app: &mut App, path: &str, events: Vec<io::Result<Event>>) -> io::Result<()> {
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
        // An unbound key does nothing.
        let before = (app.selected, app.cursor_row, app.cursor_col);
        run_events(&mut app, "", vec![key(KeyCode::Char('z'))]).unwrap();
        assert_eq!((app.selected, app.cursor_row, app.cursor_col), before);
        assert!(!app.should_quit);
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
        // A click while the prompt is open is ignored (it would hit row 0,
        // column 1: the prompt line pushes the grid down one row).
        view(&mut app, MeasureId(101));
        run_events(&mut app, "", vec![]).unwrap();
        let g = app.grid_geom.unwrap();
        let (x, y) = (g.x0 + g.col_w + g.spacing + 1, g.y0 + 1);
        let ev = vec![
            key(KeyCode::Char('I')),
            click(MouseEventKind::Down(MouseButton::Left), x, y),
            key(KeyCode::Esc),
        ];
        run_events(&mut app, "", ev).unwrap();
        assert_eq!((app.cursor_row, app.cursor_col), (0, 0));
    }

    #[test]
    fn tab_completes_in_a_formula_only() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(102));
        let mut ev = vec![key(KeyCode::Char('e'))];
        ev.extend((0..30).map(|_| key(KeyCode::Backspace)));
        ev.extend(chars("Pri"));
        ev.push(key(KeyCode::Tab));
        ev.extend(chars(" * quan"));
        ev.push(key(KeyCode::Tab));
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.edit.as_deref(), Some("Price * Quantity"));
        // Several matches: the status lists them; Tab cycles; a key resets.
        let mut ev = chars(" + sumx");
        ev.extend([key(KeyCode::Tab), key(KeyCode::Tab)]);
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.edit.as_deref(), Some("Price * Quantity + SUMXMY2("));
        assert!(app.status.as_deref().unwrap().starts_with("Tab cycles:"));
        run_events(&mut app, "", vec![key(KeyCode::Backspace)]).unwrap();
        assert!(app.tab_cycle.is_none());
        assert!(app.status.is_none());
        run_events(&mut app, "", vec![key(KeyCode::Esc)]).unwrap();
        // In a value edit Tab does nothing.
        view(&mut app, MeasureId(101));
        let mut ev = vec![key(KeyCode::Char('e'))];
        ev.extend(chars("pm"));
        ev.push(key(KeyCode::Tab));
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.edit.as_deref(), Some("100pm"));
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
        assert!(app.help.is_none());
        assert!(app.edit.is_none());
        run_events(&mut app, "", vec![key(KeyCode::F(1)), key(KeyCode::Esc)]).unwrap();
        assert!(app.help.is_none());
        // F1 mid-edit: Enter inserts the selected function's `NAME(`.
        let mut ev = vec![key(KeyCode::Char('e')), key(KeyCode::F(1))];
        ev.extend(chars("pmt"));
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, "", ev).unwrap();
        assert!(app.help.is_none());
        assert_eq!(app.edit.as_deref(), Some("100PMT("));
        // Help keys with the overlay closed do nothing.
        help_key(&mut app, KeyCode::Enter);
        assert!(app.help.is_none());
        assert_eq!(app.edit.as_deref(), Some("100PMT("));
    }

    /// Inserting from the help overlay changes the text, so the error
    /// located in the old text goes: its byte span would now point into
    /// shifted text, mid-'€' here, and drawing the marker would panic.
    #[test]
    fn help_insert_drops_the_stale_diagnostic() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(102));
        let mut ev = vec![key(KeyCode::Char('e'))];
        ev.extend((0..30).map(|_| key(KeyCode::Backspace)));
        ev.extend(chars("\"€\"+Quantty"));
        ev.extend([key(KeyCode::Enter), key(KeyCode::Home)]);
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.diagnostic.as_ref().unwrap().span, Some(6..13));
        let mut ev = vec![key(KeyCode::F(1))];
        ev.extend(chars("pmt"));
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.edit.as_deref(), Some("PMT(\"€\"+Quantty"));
        assert!(app.diagnostic.is_none());
    }

    #[test]
    fn event_errors_stop_the_loop() {
        let mut app = App::new(revenue_model()).unwrap();
        let err = run_events(&mut app, "", vec![Err(io::Error::other("boom"))]);
        assert_eq!(err.unwrap_err().to_string(), "boom");
    }

    #[test]
    fn load_save_round_trip_and_save_view_key() {
        let path = tmp_path("tui-main.db");
        let p = path.to_str().unwrap();
        assert!(
            load(p).unwrap().model.measures.is_empty(),
            "a new file opens empty"
        );
        let mut app = App::new(revenue_model()).unwrap();
        save(&mut app, p).unwrap();
        run_events(&mut app, p, chars("S")).unwrap();
        assert_eq!(app.status.as_deref(), Some("saved view 'view 1'"));
        let loaded = load(p).unwrap();
        assert_eq!(loaded.model.views.len(), 1);
        assert!(load_err("/nonexistent/dir/x.db").starts_with("cannot open model"));
        assert!(save(&mut app, "/nonexistent/dir/x.db")
            .unwrap_err()
            .starts_with("autosave failed"));
    }

    #[test]
    fn undo_and_redo_keys() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        let mut ev = vec![key(KeyCode::Enter)];
        ev.extend(chars("0"));
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.grid.value_at(0, 0), Some(1000.0));
        run_events(&mut app, "", chars("ur")).unwrap();
        assert_eq!(
            app.grid.value_at(0, 0),
            Some(100.0),
            "plain r redoes nothing"
        );
        let ctrl_r = KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL);
        run_events(&mut app, "", vec![Ok(Event::Key(ctrl_r))]).unwrap();
        assert_eq!(app.grid.value_at(0, 0), Some(1000.0));
    }

    /// H7/H8: a Time × Product revenue model built from a new file with keys
    /// only, saved with `w`, quit, reopened, and checked.
    #[test]
    fn builds_the_revenue_model_from_a_new_file() {
        let path = tmp_path("tui-build.db");
        let p = path.to_str().unwrap();
        let mut app = load(p).unwrap();
        // 'S' with nothing to show saves no view.
        run_events(&mut app, p, chars("S")).unwrap();
        assert_eq!(app.status.as_deref(), Some(app::EMPTY));
        let enter = || key(KeyCode::Enter);
        let mut ev = Vec::new();
        let line = |ev: &mut Vec<_>, k: &str, text: &str| {
            ev.extend(chars(k));
            ev.extend(chars(text));
            ev.push(enter());
        };
        line(&mut ev, "c", "Time: 2025, 2026");
        line(&mut ev, "c", "Product: WidgetA, WidgetB");
        // Price[Product]: rows are Product, one column.
        line(&mut ev, "n", "Price: Product");
        line(&mut ev, "e", "10");
        ev.push(key(KeyCode::Down));
        line(&mut ev, "e", "20");
        // Quantity[Time, Product]: rows Time, columns Product. The cursor
        // stays on row 1; Up takes it to the first cell.
        line(&mut ev, "n", "Quantity: Time, Product");
        for (step, q) in [
            (KeyCode::Up, "100"),
            (KeyCode::Right, "50"),
            (KeyCode::Down, "80"),
            (KeyCode::Left, "120"),
        ] {
            ev.push(key(step));
            line(&mut ev, "e", q);
        }
        line(&mut ev, "=", "Revenue = Price * Quantity");
        ev.extend(chars("wq"));
        run_events(&mut app, p, ev).unwrap();
        assert!(app.should_quit);
        assert_eq!(app.status.as_deref(), Some("saved"));

        let mut app = load(p).unwrap();
        let id = app.model.measure_by_name("Revenue").unwrap().id;
        view(&mut app, id);
        let g = &app.grid;
        assert_eq!(
            (
                g.row_cat.as_ref().unwrap().1.as_str(),
                g.n_rows(),
                g.n_cols()
            ),
            ("Time", 2, 2)
        );
        let cells: Vec<String> = (0..2)
            .flat_map(|r| (0..2).map(move |c| (r, c)))
            .map(|(r, c)| g.display_at(r, c))
            .collect();
        assert_eq!(cells, ["1000", "1000", "1200", "1600"]);
        let _ = std::fs::remove_file(&path);
    }

    /// Two sessions on one store: the second's autosave conflicts after the
    /// first saved; 'R' reverts it to the saved model and its next save works.
    #[test]
    fn revert_key_recovers_from_a_save_conflict() {
        let path = tmp_path("tui-revert.db");
        let p = path.to_str().unwrap();
        save(&mut App::new(revenue_model()).unwrap(), p).unwrap();
        let mut a = load(p).unwrap();
        let mut b = load(p).unwrap();
        run_events(&mut a, p, chars("S")).unwrap();
        b.model.views.clear();
        b.model
            .add_view(b.build_view(improv_core_model::ViewId(77), "mine"));
        let e = save(&mut b, p).unwrap_err();
        assert!(e.contains("conflict"), "{e}");
        run_events(&mut b, p, chars("R")).unwrap();
        assert_eq!(b.status.as_deref(), Some("reverted to the saved model"));
        assert_eq!(b.model.views.len(), 1, "picked up the other session's view");
        save(&mut b, p).expect("saves after the revert");
        run_events(&mut b, "/nonexistent/dir/x.db", chars("R")).unwrap();
        assert!(b.status.as_deref().unwrap().starts_with("revert failed"));
        assert_eq!(b.model.views.len(), 1, "a failed revert keeps the session");
    }
}
