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

/// Save the model back to `path` on clean exit (each change is saved as it
/// is made: [`App::autosave`]). Writes only what changed.
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

/// H9: the store's high-water when another writer saved since this session
/// loaded or saved (`None` too when unknown or unreadable).
fn outside_save(app: &App, path: &str) -> Option<i64> {
    let mine = app.save_token.tx()?;
    let hw = Store::open(path).and_then(|mut s| s.tx_high_water()).ok()?;
    (hw != mine).then_some(hw)
}

/// H9: on each key, say once per outside save that another program saved,
/// and that `R` loads it. Saving from here is refused until then.
fn check_outside(app: &mut App, path: &str) {
    let Some(hw) = outside_save(app, path) else {
        return;
    };
    if app.outside_seen != Some(hw) {
        app.outside_seen = Some(hw);
        app.status = Some("another program saved this model: press R to reload it".into());
    }
}

/// Dispatch one terminal event by mode (edit / command prompt / normal),
/// then save what it changed (H1-5).
fn handle_event(app: &mut App, ev: Event, path: &str) {
    if matches!(ev, Event::Key(_)) {
        check_outside(app, path);
    }
    dispatch(app, ev, path);
    app.autosave(path);
}

fn dispatch(app: &mut App, ev: Event, path: &str) {
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
    // Any other key disarms a quit whose save failed, or a revert.
    let armed = std::mem::take(&mut app.quit_armed);
    let revert_armed = std::mem::take(&mut app.revert_armed);
    match k.code {
        KeyCode::Char('q') => quit(app, path, armed),
        KeyCode::Char('e') => app.begin_edit(),
        KeyCode::Enter => app.enter_cell(),
        // H1-2: typing a number on a cell starts its edit.
        KeyCode::Char(c @ ('0'..='9' | '-' | '.')) => app.type_into_cell(c),
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
        KeyCode::Char('R') => revert(app, path, revert_armed),
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

/// `q`: save, then quit (H6). A failed save (a full disk, another writer)
/// keeps the session open with the reason, so the edits are not lost; a
/// second `q` (`armed`) quits anyway.
fn quit(app: &mut App, path: &str, armed: bool) {
    if armed {
        app.should_quit = true;
        return;
    }
    match app.save(path) {
        Ok(()) => app.should_quit = true,
        Err(e) => {
            app.quit_armed = true;
            app.status = Some(format!("save failed: {e}. q again quits without saving"));
        }
    }
}

/// Reload the model from `path`, dropping this session's unsaved edits and
/// its undo history. The way out of a save conflict: a save refuses to
/// clobber another writer, so revert, pick up their changes, and redo
/// yours. With edits or history to lose it asks first, and a second `R`
/// (`armed`) reverts (H1-20). On failure the session is untouched.
fn revert(app: &mut App, path: &str, armed: bool) {
    let lost = match (app.unsaved, app.undo_stack.is_empty()) {
        (true, _) => "this session's unsaved edits and its undo history",
        (false, false) => "the undo history",
        (false, true) => "",
    };
    if !armed && !lost.is_empty() {
        app.revert_armed = true;
        app.status = Some(format!(
            "R reloads the saved model and drops {lost}: R again to revert"
        ));
        return;
    }
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

    /// H1-2: `e` then typing replaces the old value (300 -> 350, not
    /// 300350); an arrow first edits in place; a digit in normal mode
    /// starts an edit holding just that digit.
    #[test]
    fn typing_replaces_a_cell_and_a_digit_starts_an_edit() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        let mut ev = chars("e350");
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.grid.value_at(0, 0), Some(350.0));
        let mut ev = vec![key(KeyCode::Enter), key(KeyCode::Left)];
        ev.extend(chars("1"));
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.grid.value_at(0, 0), Some(3510.0), "35|0 -> 3510");
        let mut ev = vec![key(KeyCode::Right)];
        ev.extend(chars("42"));
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.edit.as_deref(), Some("42"));
        run_events(&mut app, "", vec![key(KeyCode::Enter)]).unwrap();
        assert_eq!(app.grid.value_at(0, 1), Some(42.0));
        let mut ev = chars(".5");
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.grid.value_at(0, 1), Some(0.5));
    }

    /// H1-4: Enter then 5 on a derived cell no longer replaces its
    /// formula: Enter says why and names `e`.
    #[test]
    fn enter_on_a_derived_cell_keeps_its_formula() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(102));
        let formula = app.model.measures[&MeasureId(102)].kind.clone();
        let ev = vec![
            key(KeyCode::Enter),
            key(KeyCode::Char('5')),
            key(KeyCode::Enter),
        ];
        run_events(&mut app, "", ev).unwrap();
        assert_eq!(app.model.measures[&MeasureId(102)].kind, formula);
        assert_eq!(app.status.as_deref(), Some(app::DERIVED));
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
        let g = app.grid_geom.clone().unwrap();
        let (x, y) = (g.x0 + g.col_w[0] + g.spacing + 1, g.y0 + 1);
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
        let g = app.grid_geom.clone().unwrap();
        let (x, y) = (g.x0 + g.col_w[0] + g.spacing + 1, g.y0 + 1);
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
        let mut ev = vec![key(KeyCode::Char('e')), key(KeyCode::End)];
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
        let mut ev = vec![
            key(KeyCode::Char('e')),
            key(KeyCode::End),
            key(KeyCode::F(1)),
        ];
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
        ev.extend(chars("1000"));
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

    /// H1-5: each change is in the store as soon as it is made, with no
    /// `w` or `q` (closing the terminal loses nothing); a failed autosave
    /// marks the session "not saved" and says why, and `w` clears it.
    #[test]
    fn every_change_is_saved_as_it_is_made() {
        let path = tmp_path("tui-h1-5.db");
        let _guard = crate::app::tests::TmpFile(path.clone());
        let p = path.to_str().unwrap();
        save(&mut App::new(revenue_model()).unwrap(), p).unwrap();
        let mut app = load(p).unwrap();
        view(&mut app, MeasureId(101));
        let stored = |m: MeasureId, at: &Coordinate| {
            let model = Store::open(p).and_then(|mut s| s.load_model()).unwrap();
            model.input(m, at).cloned()
        };
        let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
        let mut ev = chars("7");
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, p, ev).unwrap();
        assert_eq!(stored(MeasureId(101), &at), Some(Value::Number(7.0)));
        assert!(!app.unsaved);
        assert_eq!(app.status, None, "a good autosave says nothing");
        let mut ev = chars("c");
        ev.extend(chars("Time: 2027"));
        ev.extend([key(KeyCode::Enter), key(KeyCode::Char('u'))]);
        run_events(&mut app, p, ev).unwrap();
        assert_eq!(stored(MeasureId(101), &at), Some(Value::Number(7.0)));
        let back = Store::open(p).and_then(|mut s| s.load_model()).unwrap();
        assert_eq!(back, app.model, "the undo is saved too");
        assert!(!lines(&mut app).iter().any(|l| l.contains("not saved")));
        // Another program saves: the next change is refused, and marked.
        let mut other = Store::open(p).unwrap();
        let mut m = other.load_model().unwrap();
        m.add_view(app.build_view(improv_core_model::ViewId(9), "theirs"));
        other.save_model(&m).unwrap();
        let mut ev = vec![key(KeyCode::Esc)];
        ev.extend(chars("8"));
        ev.push(key(KeyCode::Enter));
        run_events(&mut app, p, ev).unwrap();
        assert!(app.unsaved);
        assert!(
            app.status.as_deref().unwrap().contains("conflict"),
            "{:?}",
            app.status
        );
        assert!(
            lines(&mut app)[0].contains("not saved"),
            "{:?}",
            lines(&mut app)[0]
        );
        assert_eq!(stored(MeasureId(101), &at), Some(Value::Number(7.0)));
        // A key that changes nothing does not retry; R R then w saves.
        run_events(&mut app, p, vec![key(KeyCode::Down)]).unwrap();
        assert!(app.status.as_deref().unwrap().contains("conflict"));
        run_events(&mut app, p, chars("RR")).unwrap();
        assert!(!app.unsaved);
        view(&mut app, MeasureId(101));
        app.apply_edit(0, 0, 9.0).unwrap();
        assert!(app.unsaved, "a change outside the loop waits for w");
        run_events(&mut app, p, chars("w")).unwrap();
        assert!(!app.unsaved);
        assert_eq!(stored(MeasureId(101), &at), Some(Value::Number(9.0)));
    }

    /// The screen of `app` at 100x30, one string per row.
    fn lines(app: &mut App) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
        term.draw(|f| ui::render(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    /// H1-20: `R` with unsaved edits or undo history asks first, naming
    /// what goes; another key disarms it, and a second `R` reverts. With
    /// nothing to lose, one `R` reverts.
    #[test]
    fn revert_asks_before_dropping_edits() {
        let path = tmp_path("tui-h1-20.db");
        let _guard = crate::app::tests::TmpFile(path.clone());
        let p = path.to_str().unwrap();
        save(&mut App::new(revenue_model()).unwrap(), p).unwrap();
        let mut app = load(p).unwrap();
        run_events(&mut app, p, chars("R")).unwrap();
        assert_eq!(app.status.as_deref(), Some("reverted to the saved model"));
        view(&mut app, MeasureId(101));
        app.apply_edit(0, 0, 5.0).unwrap();
        run_events(&mut app, p, chars("R")).unwrap();
        let asked = app.status.clone().unwrap();
        assert!(
            asked.contains("unsaved edits and its undo history"),
            "{asked}"
        );
        assert_eq!(app.grid.value_at(0, 0), Some(5.0), "nothing dropped yet");
        run_events(&mut app, p, vec![key(KeyCode::Down)]).unwrap();
        run_events(&mut app, p, chars("R")).unwrap();
        assert_eq!(app.grid.value_at(0, 0), Some(5.0), "Down disarmed it");
        run_events(&mut app, p, chars("w")).unwrap();
        run_events(&mut app, p, chars("R")).unwrap();
        assert!(app
            .status
            .as_deref()
            .unwrap()
            .ends_with("drops the undo history: R again to revert"));
        run_events(&mut app, p, chars("R")).unwrap();
        assert_eq!(app.status.as_deref(), Some("reverted to the saved model"));
        assert!(app.undo_stack.is_empty());
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

    // -- H6: crash safety (kill -9 during a save, a full disk, two writers)

    /// One input measure over 100 x 50 items, every cell 0: a save that
    /// rewrites them all takes long enough to be killed midway.
    fn crash_model() -> improv_core_model::Model {
        use improv_core_model::{Measure, MeasureKind, Model, Name, ValueType};
        let mut m = Model::new();
        let (a, b) = (CategoryId(1), CategoryId(2));
        m.add_category(a, "A");
        m.add_category(b, "B");
        (0..100).for_each(|i| m.add_item(ItemId(100 + i), a, format!("a{i}")));
        (0..50).for_each(|j| m.add_item(ItemId(1000 + j), b, format!("b{j}")));
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("X".into()),
            value_type: ValueType::Number,
            categories: vec![a, b],
            kind: MeasureKind::Input,
            description: None,
        });
        for i in 0..100 {
            for j in 0..50 {
                let at = Coordinate::from_pairs([(a, ItemId(100 + i)), (b, ItemId(1000 + j))]);
                m.set_input(MeasureId(1), at, Value::Number(0.0));
            }
        }
        m
    }

    /// The one value every cell holds: two values mean a half-applied save.
    fn uniform_value(model: &improv_core_model::Model) -> f64 {
        let vals: std::collections::BTreeSet<u64> = model
            .inputs
            .values()
            .map(|v| match v {
                Value::Number(n) => n.to_bits(),
                other => panic!("not a number: {other:?}"),
            })
            .collect();
        assert_eq!(model.inputs.len(), 5000, "cells were lost");
        assert_eq!(vals.len(), 1, "a save was half applied: {vals:?}");
        f64::from_bits(*vals.first().expect("a value"))
    }

    /// SQLite's own check of the store file.
    fn integrity(db: &str) -> String {
        mentat::Store::open(db)
            .expect("open the store")
            .sqlite_ref()
            .pragma_query_value(None, "integrity_check", |r| r.get::<_, String>(0))
            .expect("integrity_check")
    }

    /// This test binary's [`h6_child`] in `mode`, with `db`.
    fn h6_child_command(mode: &str, db: &str) -> std::process::Command {
        let mut c = std::process::Command::new(std::env::current_exe().expect("the test binary"));
        c.args(["--exact", "tests::h6_child", "--ignored", "--nocapture"])
            .env("IMPROV_TUI_H6_CHILD", mode)
            .env("IMPROV_TUI_H6_DB", db);
        c
    }

    /// The child process of the H6 tests; does nothing unless one runs it.
    #[test]
    #[ignore = "a child process of the H6 crash-safety tests"]
    fn h6_child() {
        let (Ok(mode), Ok(db)) = (
            std::env::var("IMPROV_TUI_H6_CHILD"),
            std::env::var("IMPROV_TUI_H6_DB"),
        ) else {
            return;
        };
        match mode.as_str() {
            "saves" => h6_child_saves(&db),
            "disk-full" => h6_child_disk_full(&db),
            other => panic!("unknown mode {other}"),
        }
    }

    /// Save k = 1, 2, … through the TUI's save (`w`), every cell set to k,
    /// saying `saving k` and `saved k`. Runs until killed.
    fn h6_child_saves(db: &str) {
        let mut app = load(db).expect("load");
        let cells: Vec<_> = app.model.inputs.keys().cloned().collect();
        for k in 1..=u16::MAX {
            for (m, at) in &cells {
                app.model
                    .set_input(*m, at.clone(), Value::Number(f64::from(k)));
            }
            println!("saving {k}");
            app.write(db);
            assert_eq!(app.status.as_deref(), Some("saved"));
            println!("saved {k}");
        }
    }

    /// kill -9 a TUI that is saving, then reopen: the store holds every
    /// acknowledged save and nothing of the killed one, SQLite finds no
    /// corruption, and the TUI opens and saves it again.
    #[test]
    fn kill_9_during_a_save_loses_nothing_and_corrupts_nothing() {
        use std::io::BufRead;
        let path = tmp_path("tui-h6-kill.db");
        let db = path.to_str().unwrap().to_owned();
        let _files: Vec<_> = ["", "-wal", "-shm"]
            .iter()
            .map(|s| crate::app::tests::TmpFile(format!("{db}{s}").into()))
            .collect();
        Store::open(&db)
            .and_then(|mut s| s.save_model(&crash_model()))
            .expect("seed");
        let mut midway = 0;
        // Three rounds, and more (to nine) until one kill lands mid-save.
        let fractions = [0.2, 0.5, 0.8, 0.35, 0.65, 0.1, 0.9, 0.45, 0.55];
        for (round, fraction) in fractions.into_iter().enumerate() {
            if round >= 3 && midway > 0 {
                break;
            }
            let mut child = h6_child_command("saves", &db)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .expect("spawn the saving child");
            let mut lines = std::io::BufReader::new(child.stdout.take().expect("stdout")).lines();
            let mut next = |want: &str| loop {
                let line = lines.next().expect("the child stopped").expect("read");
                if let Some(k) = line.strip_prefix(want) {
                    break (k.parse::<u32>().expect("k"), std::time::Instant::now());
                }
            };
            // Time the second save: the first is a full save, the rest
            // write the changed cells (B12), much faster.
            next("saving ");
            next("saved ");
            let (_, t1) = next("saving ");
            let (_, t2) = next("saved ");
            let (k, _) = next("saving ");
            std::thread::sleep((t2 - t1).mul_f64(fraction));
            child.kill().expect("kill -9");
            let _ = child.wait();
            // Where the child really stopped: on Windows TerminateProcess
            // returns before the child does, so it can start more saves
            // after save k. The last "saving" it printed is the one that
            // may have been cut off; every earlier one finished.
            let mut k = k;
            let mut finished = false;
            for l in lines.map_while(Result::ok) {
                if let Some(n) = l.strip_prefix("saving ") {
                    k = n.parse().expect("k");
                    finished = false;
                } else if l == format!("saved {k}") {
                    finished = true;
                }
            }
            let acked = if finished { k } else { k - 1 };
            let reopened = Store::open(&db)
                .and_then(|mut s| s.load_model())
                .expect("the store reopens");
            let v = uniform_value(&reopened);
            assert!(
                v == f64::from(acked) || v == f64::from(k),
                "round {round}: store holds {v}, acknowledged {acked}, killed during {k}"
            );
            if !finished && v == f64::from(acked) {
                midway += 1;
            }
            println!("H6 kill -9 round {round}: killed during save {k} (finished: {finished}), store holds {v}");
            assert_eq!(integrity(&db), "ok");
            let mut app = load(&db).expect("the TUI reopens it");
            for v in app.model.inputs.values_mut() {
                *v = Value::Number(-1.0);
            }
            app.write(&db);
            assert_eq!(app.status.as_deref(), Some("saved"), "and saves");
        }
        assert!(midway > 0, "no kill landed inside a save");
    }

    /// In its own mount namespace, on a tiny tmpfs at `dir`: fill the disk,
    /// edit and save until a save fails, check the failure and that `q`
    /// stays, free the space, save, and check the store.
    fn h6_child_disk_full(dir: &str) {
        let db = format!("{dir}/model.db");
        save(&mut App::new(revenue_model()).unwrap(), &db).expect("seed");
        let mut app = load(&db).expect("load");
        view(&mut app, MeasureId(101));
        let filler = format!("{dir}/filler");
        {
            use std::io::Write;
            let mut f = std::fs::File::create(&filler).expect("filler");
            let chunk = vec![0u8; 4096];
            while f.write_all(&chunk).is_ok() {}
        }
        let mut failed = None;
        for i in 0..2000 {
            // Alternate a cell and a new item: both are saved by `w`.
            let mut ev = if i % 2 == 0 {
                let mut ev = chars("e");
                ev.extend(chars(&i.to_string()));
                ev.push(key(KeyCode::Enter));
                ev
            } else {
                let mut ev = chars("c");
                ev.extend(chars(&format!("Time: y{i}")));
                ev.push(key(KeyCode::Enter));
                ev
            };
            ev.extend(chars("w"));
            run_events(&mut app, &db, ev).unwrap();
            let status = app.status.clone().unwrap_or_default();
            if status != "saved" {
                assert!(status.contains("the disk is full"), "unclear: {status}");
                assert!(status.contains("edits are still here"), "{status}");
                println!("H6 disk-full message: {status}");
                failed = Some(i);
                break;
            }
        }
        let i = failed.expect("no save failed on a full disk");
        let kept = app.model.clone();
        run_events(&mut app, &db, chars("q")).unwrap();
        assert!(!app.should_quit, "q keeps a session whose save failed");
        assert!(app
            .status
            .as_deref()
            .unwrap()
            .ends_with("q again quits without saving"));
        assert_eq!(app.model, kept, "the in-memory model is kept");
        std::fs::remove_file(&filler).expect("free the space");
        // With room again, w saves (and disarms the quit); q then quits.
        run_events(&mut app, &db, chars("w")).unwrap();
        assert_eq!(app.status.as_deref(), Some("saved"));
        run_events(&mut app, &db, chars("q")).unwrap();
        assert!(app.should_quit);
        let back = load(&db).expect("reopen");
        assert_eq!(back.model, kept, "nothing lost, nothing half-saved");
        assert_eq!(integrity(&db), "ok");
        println!("H6 disk-full: save {i} failed cleanly");
    }

    /// A full disk during a save: a clear error, the session (and `q`)
    /// kept, the store intact. Runs on a small tmpfs in a user namespace
    /// (Linux; skipped where those are not allowed).
    #[cfg(target_os = "linux")]
    #[test]
    fn a_full_disk_fails_the_save_clearly_and_keeps_the_model() {
        let dir = tmp_path("tui-h6-full");
        std::fs::create_dir(&dir).expect("mount point");
        struct Rmdir(std::path::PathBuf);
        impl Drop for Rmdir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir(&self.0);
            }
        }
        let _rmdir = Rmdir(dir.clone());
        let child = h6_child_command("disk-full", dir.to_str().unwrap());
        let script = r#"mount -t tmpfs -o size=1m tmpfs "$IMPROV_TUI_H6_DB" || exit 77; exec "$@""#;
        let mut c = std::process::Command::new("unshare");
        c.args([
            "--user",
            "--map-root-user",
            "--mount",
            "sh",
            "-c",
            script,
            "sh",
        ])
        .arg(child.get_program())
        .args(child.get_args())
        .envs(child.get_envs().filter_map(|(k, v)| Some((k, v?))));
        let out = match c.output() {
            Ok(o) => o,
            Err(e) => return eprintln!("skipped: no unshare ({e})"),
        };
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        if out.status.code() == Some(77) || stderr.starts_with("unshare:") {
            return eprintln!("skipped: no user and mount namespaces: {stderr}");
        }
        assert!(
            out.status.success() && stdout.contains("H6 disk-full: save"),
            "the disk-full child failed:\n{stdout}\n{stderr}"
        );
        stdout
            .lines()
            .filter(|l| l.starts_with("H6"))
            .for_each(|l| println!("{l}"));
    }

    /// Two sessions on one store: the second to save is refused with a
    /// message naming `R`, overwriting nothing, and `q` does not throw its
    /// edits away; `R` then picks up the other writer's change.
    #[test]
    fn two_writers_conflict_and_neither_overwrites_the_other() {
        let path = tmp_path("tui-h6-two.db");
        let _guard = crate::app::tests::TmpFile(path.clone());
        let p = path.to_str().unwrap();
        save(&mut App::new(revenue_model()).unwrap(), p).unwrap();
        let mut a = load(p).unwrap();
        let mut b = load(p).unwrap();
        for app in [&mut a, &mut b] {
            view(app, MeasureId(101));
        }
        let edit = |app: &mut App, v: &str| {
            let mut ev = vec![key(KeyCode::Enter)];
            ev.extend((0..8).map(|_| key(KeyCode::Backspace)));
            ev.extend(chars(v));
            ev.push(key(KeyCode::Enter));
            run_events(app, p, ev).unwrap();
        };
        edit(&mut b, "7");
        run_events(&mut b, p, chars("w")).unwrap();
        assert_eq!(b.status.as_deref(), Some("saved"));
        edit(&mut a, "9");
        run_events(&mut a, p, chars("w")).unwrap();
        let e = a.status.clone().unwrap();
        assert!(e.contains("another program changed this model"), "{e}");
        assert!(e.contains("Press R"), "{e}");
        assert_eq!(a.grid.value_at(0, 0), Some(9.0), "A keeps its edit");
        run_events(&mut a, p, chars("q")).unwrap();
        assert!(!a.should_quit, "q does not drop A's edit");
        run_events(&mut a, p, vec![key(KeyCode::Down)]).unwrap();
        run_events(&mut a, p, chars("q")).unwrap();
        assert!(!a.should_quit, "another key disarmed the quit");
        let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
        let stored = Store::open(p).and_then(|mut s| s.load_model()).unwrap();
        assert_eq!(
            stored.input(MeasureId(101), &at),
            Some(&Value::Number(7.0)),
            "B's edit kept"
        );
        run_events(&mut a, p, chars("RR")).unwrap();
        assert_eq!(
            a.model.input(MeasureId(101), &at),
            Some(&Value::Number(7.0))
        );
        view(&mut a, MeasureId(101));
        edit(&mut a, "9");
        run_events(&mut a, p, chars("q")).unwrap();
        assert!(a.should_quit, "after R, A's edit saves and q quits");
        // An armed q quits even when the save would fail.
        edit(&mut b, "8");
        run_events(&mut b, p, chars("qq")).unwrap();
        assert!(b.should_quit);
        let e = save(&mut b, p).unwrap_err();
        assert!(
            e.starts_with("autosave failed: conflict: another program"),
            "{e}"
        );
    }

    /// The wording for each kind of failed save.
    #[test]
    fn save_errors_say_what_happened() {
        use improv_storage_mentat::StoreError as M;
        use improv_store::StoreError;
        let full = StoreError::Mentat(M::Mentat("SQL error: database or disk is full".into()));
        assert!(app::save_error(full).starts_with("the disk is full"));
        let io = StoreError::Mentat(M::Mentat("SQL error: disk I/O error".into()));
        assert!(app::save_error(io).contains("free some space"));
        assert!(app::save_error(StoreError::Conflict("x".into())).starts_with("conflict: another"));
        let other = StoreError::Mentat(M::Integrity("bad".into()));
        assert_eq!(app::save_error(other), "data integrity: bad");
    }

    /// H9: another store handle saves; the next key says so once, `R`
    /// reloads, and this session's own saves never trigger it.
    #[test]
    fn a_key_after_another_writers_save_offers_reload() {
        let path = tmp_path("tui-h9.db");
        let p = path.to_str().unwrap();
        save(&mut App::new(revenue_model()).unwrap(), p).unwrap();
        let mut a = load(p).unwrap();
        run_events(&mut a, p, chars("w")).unwrap();
        run_events(&mut a, p, vec![key(KeyCode::Down)]).unwrap();
        assert_eq!(a.status.as_deref(), Some("saved"), "its own save");

        let mut other = Store::open(p).unwrap();
        let mut m = other.load_model().unwrap();
        m.add_view(a.build_view(improv_core_model::ViewId(9), "theirs"));
        other.save_model(&m).unwrap();
        run_events(&mut a, p, vec![key(KeyCode::Down)]).unwrap();
        let said = "another program saved this model: press R to reload it";
        assert_eq!(a.status.as_deref(), Some(said));
        run_events(&mut a, p, vec![key(KeyCode::Esc), key(KeyCode::Down)]).unwrap();
        assert_eq!(a.status, None, "said once per outside save");
        run_events(&mut a, p, chars("w")).unwrap();
        assert!(
            a.status.as_deref().unwrap().contains("conflict"),
            "never overwrites"
        );
        run_events(&mut a, p, chars("R")).unwrap();
        assert_eq!(a.status.as_deref(), Some("reverted to the saved model"));
        assert!(a.model.view_by_name("theirs").is_some());
        run_events(&mut a, p, vec![key(KeyCode::Down)]).unwrap();
        assert_eq!(a.status.as_deref(), Some("reverted to the saved model"));
        // An unreadable store or an unsaved model offers nothing.
        assert_eq!(outside_save(&a, "/nonexistent/dir/x.db"), None);
        assert_eq!(outside_save(&App::new(revenue_model()).unwrap(), p), None);
        let _ = std::fs::remove_file(&path);
    }
}
