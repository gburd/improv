//! Rendering: a status box (title, prompt/message, pages, filters, key help)
//! sized to its content, over the pivot grid as a table.

use crate::app::{App, CommandKind, Grid, GridGeom};
use crate::diagnose::Diagnostic;
use crate::edit::LineEdit;
use crate::fnhelp::FnHelp;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, Wrap,
    },
    Frame,
};

/// Key help entries, packed greedily into lines that fit the status box.
const HELP: &[&str] = &[
    "arrows/click: move",
    "Shift+arrows: range",
    "e/Enter: edit",
    "Tab (formula): complete",
    "[ ]: page",
    "p: pivot",
    "f/F: filter",
    "S: save view",
    "v: view",
    "Tab/m: measure",
    "I: import-csv",
    "E: export-csv",
    "R: revert",
    "?/F1: functions",
    "q: quit",
];

fn help_lines(width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for h in HELP {
        if !cur.is_empty() && cur.len() + 2 + h.len() > width {
            lines.push(Line::from(std::mem::take(&mut cur)));
        }
        if !cur.is_empty() {
            cur.push_str("  ");
        }
        cur.push_str(h);
    }
    lines.push(Line::from(cur));
    lines
}

/// Status box content for an inner width of `width` columns. The prompt or
/// message comes right after the title, so a short terminal clips the help
/// text first, never the line being typed.
fn status_lines(app: &App, width: usize) -> Vec<Line<'static>> {
    let g = &app.grid;
    let axes = match (&g.row_cat, &g.col_cat) {
        (Some((_, r)), Some((_, c))) => format!("rows: {r}  cols: {c}"),
        (Some((_, r)), None) => format!("rows: {r}"),
        (None, _) => "scalar".to_string(),
    };
    let kind = if app
        .model
        .measures
        .get(&g.measure)
        .map(|m| m.is_derived())
        .unwrap_or(false)
    {
        "derived"
    } else {
        "input"
    };
    let sum = match app.selection_sum() {
        Some(v) => format!("Sum {v}"),
        None => "Sum \u{2014}".to_string(),
    };
    let coord = format!(
        "cell [{}, {}] = {}   {sum}",
        g.rows
            .get(app.cursor_row)
            .map(|(_, n)| n.as_str())
            .unwrap_or(""),
        g.cols
            .get(app.cursor_col)
            .map(|(_, n)| n.as_str())
            .unwrap_or(""),
        g.display_at(app.cursor_row, app.cursor_col),
    );

    let mut lines = vec![Line::from(format!(
        "{} [{}]   {axes}   {coord}",
        g.measure_name, kind
    ))];
    if let Some(f) = app.formula_text() {
        lines.push(Line::from(format!("{} = {f}", g.measure_name)));
    }
    if let Some(e) = &app.edit {
        let formula = app.editing_formula();
        let prompt = if formula { "formula> " } else { "edit> " };
        let d = app.diagnostic.as_ref();
        lines.push(edit_line(e, prompt, formula, d));
        if let Some(d) = d {
            lines.extend(diagnostic_lines(e, d, prompt));
        } else if formula {
            lines.extend(signature_line(e));
        }
    } else if let Some((kind, buf)) = &app.command {
        let prompt = match kind {
            CommandKind::ImportCsv => "import-csv",
            CommandKind::ExportCsv => "export-csv",
        };
        lines.push(Line::from(format!("{prompt}> {buf}")));
    }
    // Under the prompt, so an edit's error shows while it is still open.
    if let Some(msg) = &app.status {
        lines.push(Line::from(msg.clone()));
    }
    if !g.pages.is_empty() {
        let pages = g
            .pages
            .iter()
            .map(|p| {
                format!(
                    "{}={} ({}/{})",
                    p.cat_name,
                    p.item_name,
                    p.item_index + 1,
                    p.item_count
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(Line::from(format!("pages: {pages}   ([ ] to change)")));
    }
    if !app.filters.is_empty() {
        let f = app
            .filters
            .iter()
            .map(|f| {
                let cname = app
                    .model
                    .categories
                    .get(&f.category)
                    .map(|c| c.name.0.clone())
                    .unwrap_or_default();
                format!("{cname}: {} shown", f.items.len())
            })
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(Line::from(format!("filters: {f}   (F to clear)")));
    }
    lines.extend(help_lines(width));
    lines
}

/// The `prompt` (`edit> `, or `formula> ` for a formula) and the text,
/// the cell under the caret reversed and a diagnostic's span red and
/// underlined. In a formula with the caret at the end inside a call, the
/// parameters still to type follow, dimmed.
fn edit_line(
    e: &LineEdit,
    prompt: &'static str,
    formula: bool,
    d: Option<&Diagnostic>,
) -> Line<'static> {
    let bad = d.and_then(|d| d.span.clone()).unwrap_or_default();
    let err = Style::default()
        .fg(Color::Red)
        .add_modifier(Modifier::UNDERLINED);
    let mut spans = vec![Span::raw(prompt)];
    // One span per char: few enough to not matter on an edit line.
    let chars = e.text.char_indices().map(|(i, c)| (i, c.to_string()));
    for (i, c) in chars.chain(std::iter::once((e.text.len(), " ".to_string()))) {
        let mut style = if bad.contains(&i) {
            err
        } else {
            Style::default()
        };
        if i == e.caret {
            style = style.add_modifier(Modifier::REVERSED);
        } else if i == e.text.len() {
            continue;
        }
        spans.push(Span::styled(c, style));
    }
    if let Some(g) = formula
        .then(|| crate::complete::ghost(e, e.caret))
        .flatten()
    {
        spans.push(Span::styled(
            g,
            Style::default().add_modifier(Modifier::DIM),
        ));
    }
    Line::from(spans)
}

/// Under the edit line: a `^~~~` marker under the diagnostic's span (lined
/// up past `prompt`), then the message and its hint.
fn diagnostic_lines(e: &LineEdit, d: &Diagnostic, prompt: &str) -> Vec<Line<'static>> {
    let red = Style::default().fg(Color::Red);
    let mut lines = Vec::new();
    if let Some(s) = &d.span {
        // Display columns, so wide characters keep the marker aligned.
        let pad = prompt.len() + Span::raw(&e.text[..s.start]).width();
        let n = Span::raw(&e.text[s.clone()]).width().max(1);
        let marker = format!("^{}", "~".repeat(n - 1));
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled(marker, red.add_modifier(Modifier::BOLD)),
        ]));
    }
    lines.push(Line::styled(format!("error: {}", d.message), red));
    if let Some(h) = &d.hint {
        lines.push(Line::from(format!("hint: {h}")));
    }
    lines
}

/// The signature of the call the caret is in, the current argument marked
/// `‹pv›` and reversed, then the description:
/// `PMT(rate, nper, ‹pv›, [fv=0], [type=0]) — payment per period`.
fn signature_line(e: &LineEdit) -> Option<Line<'static>> {
    let (d, arg) = improv_core_model::functions::call_at(e, e.caret)?;
    let names = d.param_names();
    // A variadic tail (`…`) takes every argument past the named ones.
    let cur = (arg < names.len() || d.variadic).then(|| arg.min(names.len().saturating_sub(1)));
    let mut spans = vec![Span::raw(format!("{}(", d.name))];
    for (i, n) in names.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(", "));
        }
        spans.push(if Some(i) == cur {
            Span::styled(
                format!("\u{2039}{n}\u{203a}"),
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
            )
        } else {
            Span::raw(n.clone())
        });
    }
    // Drop a doc's trailing restatement of the signature.
    let doc = d
        .doc
        .find(&format!(": {}(", d.name))
        .map_or(d.doc, |i| &d.doc[..i]);
    spans.push(Span::raw(format!(") \u{2014} {doc}")));
    Some(Line::from(spans))
}

/// Row-label column width, data column width, and inter-column spacing.
const LABEL_W: u16 = 14;
const COL_W: u16 = 12;
const SPACING: u16 = 1;

/// First visible index after scrolling minimally from `prev` so `cursor`
/// lies in a window of `vis` entries.
fn scroll(prev: usize, cursor: usize, vis: usize) -> usize {
    if cursor < prev {
        cursor
    } else if cursor >= prev + vis {
        cursor + 1 - vis
    } else {
        prev
    }
}

/// The grid table over the visible window described by `geom`.
fn grid_table<'a>(app: &'a App, g: &'a Grid, geom: &GridGeom) -> Table<'a> {
    let cols = geom.col0..(geom.col0 + geom.cols).min(g.n_cols());
    let rows = geom.row0..(geom.row0 + geom.rows).min(g.n_rows());
    // Header: a corner cell (row-category name) then one per column item.
    let corner = g
        .row_cat
        .as_ref()
        .map(|(_, n)| n.clone())
        .unwrap_or_default();
    let mut header_cells = vec![Cell::from(corner)];
    header_cells.extend(
        g.cols[cols.clone()]
            .iter()
            .map(|(_, n)| Cell::from(n.clone())),
    );
    let header = Row::new(header_cells)
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(0);

    let sel = Style::default()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let cols_r = cols.clone();
    let range = Style::default().bg(Color::Cyan).fg(Color::Black);
    let rows = rows.map(move |r| {
        let rname = g.rows[r].1.clone();
        let mut cells =
            vec![Cell::from(rname).style(Style::default().add_modifier(Modifier::BOLD))];
        for c in cols_r.clone() {
            let text = g.display_at(r, c);
            let cell = Cell::from(text);
            let cell = if r == app.cursor_row && c == app.cursor_col {
                cell.style(sel)
            } else if app.is_selected(r, c) {
                cell.style(range)
            } else {
                cell
            };
            cells.push(cell);
        }
        Row::new(cells)
    });

    // One header column plus one per visible data column.
    let mut widths = vec![Constraint::Length(LABEL_W)];
    widths.extend(cols.clone().map(|_| Constraint::Length(COL_W)));

    Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Improv "))
        .column_spacing(SPACING)
}

pub fn render(f: &mut Frame, app: &mut App) {
    let lines = status_lines(app, f.area().width.saturating_sub(2) as usize);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(lines.len() as u16 + 2),
            Constraint::Min(3),
        ])
        .split(f.area());

    let status = ratatui::widgets::Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    f.render_widget(status, chunks[0]);

    // Window the grid to what fits inside the border (minus the header row
    // and row-label column), scrolled so the cursor stays visible, and record
    // that geometry so mouse clicks map back to cells.
    let area = chunks[1];
    let inner_w = area.width.saturating_sub(2);
    let vis_rows = (area.height.saturating_sub(3) as usize).max(1);
    let vis_cols = ((inner_w.saturating_sub(LABEL_W) / (SPACING + COL_W)) as usize).max(1);
    let prev = app.grid_geom.map_or((0, 0), |g| (g.row0, g.col0));
    let geom = GridGeom {
        x0: area.x + 1 + LABEL_W + SPACING,
        y0: area.y + 2,
        col_w: COL_W,
        spacing: SPACING,
        row0: scroll(prev.0, app.cursor_row, vis_rows),
        col0: scroll(prev.1, app.cursor_col, vis_cols),
        rows: vis_rows,
        cols: vis_cols,
    };
    app.grid_geom = Some(geom);

    let table = grid_table(app, &app.grid, &geom);
    f.render_widget(table, area);

    if let Some(h) = &mut app.help {
        render_help(f, h, f.area());
    }
}

/// The function help overlay over `area` (inset by a margin): the search
/// line, the result list scrolled to the selection, and the selected
/// function's details.
fn render_help(f: &mut Frame, h: &mut FnHelp, area: Rect) {
    let area = area.inner(ratatui::layout::Margin::new(2, 1));
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Functions  (type to search, Enter inserts, Esc closes) ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(6),
        ])
        .split(inner);

    let found = format!("  ({} found)", h.results.len());
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(format!("search> {}", h.query)),
            Span::styled(found, Style::default().add_modifier(Modifier::DIM)),
        ])),
        chunks[0],
    );

    h.page = chunks[1].height as usize;
    let items: Vec<ListItem> = h.results.iter().map(|d| ListItem::new(d.hint())).collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::TOP | Borders::BOTTOM))
        .highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        );
    let mut state = ListState::default()
        .with_selected(h.current().map(|_| h.selected))
        .with_offset(h.top);
    f.render_stateful_widget(list, chunks[1], &mut state);
    h.top = state.offset();
    // The list's top and bottom borders take two of its rows.
    h.page = h.page.saturating_sub(2);

    let detail = match h.current() {
        Some(d) => {
            let mut lines = vec![
                Line::styled(d.hint(), Style::default().add_modifier(Modifier::BOLD)),
                Line::from(format!("category: {:?}", d.category)),
            ];
            if !d.aliases.is_empty() {
                lines.push(Line::from(format!("aliases: {}", d.aliases.join(", "))));
            }
            lines.push(Line::from(d.doc));
            lines
        }
        None => vec![Line::from("no function matches")],
    };
    f.render_widget(Paragraph::new(detail).wrap(Wrap { trim: true }), chunks[2]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::revenue_model;
    use ratatui::{backend::TestBackend, buffer::Buffer, Terminal};

    /// Render `app` into a `w`x`h` test terminal and return the buffer.
    fn draw(app: &mut App, w: u16, h: u16) -> Buffer {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| render(f, app)).unwrap();
        term.backend().buffer().clone()
    }

    /// The buffer as one string per row.
    fn text(buf: &Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn row_of(lines: &[String], needle: &str) -> usize {
        let row = lines.iter().position(|l| l.contains(needle));
        assert!(
            row.is_some(),
            "{needle:?} not on screen:\n{}",
            lines.join("\n")
        );
        row.unwrap()
    }

    /// Screen position of the first occurrence of `needle`.
    fn find(lines: &[String], needle: &str) -> (u16, u16) {
        let y = row_of(lines, needle);
        let x = lines[y].find(needle).unwrap();
        (lines[y][..x].chars().count() as u16, y as u16)
    }

    #[test]
    fn scroll_keeps_cursor_in_window() {
        assert_eq!(scroll(0, 3, 5), 0);
        assert_eq!(scroll(0, 7, 5), 3);
        assert_eq!(scroll(4, 2, 5), 2);
    }

    #[test]
    fn grid_scrolls_to_cursor_and_clicks_hit_the_cell_drawn() {
        // Regression: the grid never scrolled (cursor could sit off-screen),
        // and click hit-testing ignored the row-label column, so clicking a
        // cell selected the column to its right.
        let mut app = App::new(revenue_model()).unwrap();
        crate::app::tests::view(&mut app, improv_core_model::MeasureId(101));
        app.cursor_col = 1;
        // 32 cols: border + 14 label + 1 + one 12-wide column fits.
        let lines = text(&draw(&mut app, 32, 20));
        assert!(
            !lines.iter().any(|l| l.contains("WidgetA")),
            "scrolled past col 0"
        );
        let (x, y) = find(&lines, "WidgetB");
        let (rx, ry) = find(&lines, "2026");
        app.click_at(x + 3, ry);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
        // Clicks on the row-label column or header are not cells.
        app.click_at(rx, ry - 1);
        app.click_at(x, y);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
        // The cursor cell is drawn in the selection style.
        let buf = draw(&mut app, 32, 20);
        assert_eq!(buf[(x, ry)].bg, Color::Blue);
        assert_ne!(buf[(rx, ry)].bg, Color::Blue);
    }

    #[test]
    fn tiny_terminals_render_without_panicking() {
        let mut app = App::new(revenue_model()).unwrap();
        app.status = Some("a long status message that will not fit".into());
        for (w, h) in [(20, 5), (1, 1), (0, 0), (5, 40), (200, 3)] {
            draw(&mut app, w, h);
            app.move_cursor(1, 1);
        }
    }

    #[test]
    fn status_shows_formula_sum_and_pages_and_range_style() {
        let mut m = revenue_model();
        let mut q = m.measures[&improv_core_model::MeasureId(101)].clone();
        q.id = improv_core_model::MeasureId(300);
        m.add_category(improv_core_model::CategoryId(3), "Region");
        m.add_item(
            improv_core_model::ItemId(30),
            improv_core_model::CategoryId(3),
            "North",
        );
        q.categories.push(improv_core_model::CategoryId(3));
        m.add_measure(q);
        let mut app = App::new(m).unwrap();
        let lines = text(&draw(&mut app, 100, 30));
        row_of(&lines, "Revenue = Price times Quantity");
        row_of(&lines, "Sum 1000");
        crate::app::tests::view(&mut app, improv_core_model::MeasureId(300));
        let lines = text(&draw(&mut app, 100, 30));
        row_of(&lines, "pages: Region=North (1/1)");
        row_of(&lines, "Sum \u{2014}");
        crate::app::tests::view(&mut app, improv_core_model::MeasureId(101));
        app.step(1, 0, true);
        let buf = draw(&mut app, 100, 30);
        let lines = text(&buf);
        row_of(&lines, "Sum 220");
        let (x, y) = find(&lines, "2025");
        let data_x = x + LABEL_W + SPACING;
        assert_eq!(
            buf[(data_x, y)].bg,
            Color::Cyan,
            "anchor cell in range style"
        );
        assert_eq!(buf[(data_x, y + 1)].bg, Color::Blue, "cursor cell");
    }

    #[test]
    fn formula_edit_shows_signature_and_ghost_text() {
        let mut app = App::new(revenue_model()).unwrap();
        crate::app::tests::view(&mut app, improv_core_model::MeasureId(102));
        app.begin_edit();
        app.edit = Some("Price * PMT(0.05, 12, ".into());
        let buf = draw(&mut app, 120, 30);
        let lines = text(&buf);
        let (x, y) = find(&lines, "pv, [fv=0], [type=0])");
        assert_eq!(row_of(&lines, "formula> Price * PMT(0.05, 12,"), y as usize);
        assert!(buf[(x, y)].modifier.contains(Modifier::DIM), "ghost is dim");
        let sig =
            "PMT(rate, nper, \u{2039}pv\u{203a}, [fv=0], [type=0]) \u{2014} payment per period";
        let (sx, sy) = find(&lines, sig);
        assert_eq!(sy, y + 1, "the hint is right under the edit line");
        let at = sx + "PMT(rate, nper, ".chars().count() as u16;
        assert!(buf[(at, sy)].modifier.contains(Modifier::REVERSED));
        assert!(!buf[(sx, sy)].modifier.contains(Modifier::REVERSED));
        // A caret before the end: no ghost text, the hint follows the caret.
        app.edit.as_mut().unwrap().caret = "Price * PMT(0".len();
        let lines = text(&draw(&mut app, 120, 30));
        assert!(!lines.iter().any(|l| l.contains("12, pv")));
        row_of(&lines, "PMT(\u{2039}rate\u{203a}, nper");
        // A variadic tail marks its last parameter; too many marks none.
        app.edit = Some("CONCAT(1, 2, 3".into());
        row_of(
            &text(&draw(&mut app, 120, 30)),
            "CONCAT(text1, \u{2039}\u{2026}\u{203a})",
        );
        app.edit = Some("ABS(1, 2".into());
        let lines = text(&draw(&mut app, 120, 30));
        assert!(!lines.iter().any(|l| l.contains('\u{2039}')));
        // Outside a call, and in a value edit, neither shows.
        app.edit = Some("Price".into());
        let lines = text(&draw(&mut app, 120, 30));
        assert!(!lines.iter().any(|l| l.contains('\u{2014}')));
        crate::app::tests::view(&mut app, improv_core_model::MeasureId(101));
        app.edit = Some("PMT(".into());
        let lines = text(&draw(&mut app, 120, 30));
        assert!(!lines.iter().any(|l| l.contains("rate")));
    }

    #[test]
    fn formula_errors_are_located_and_hinted() {
        let mut app = App::new(revenue_model()).unwrap();
        crate::app::tests::view(&mut app, improv_core_model::MeasureId(102));
        app.begin_edit();
        app.edit = Some("Price * Quantty".into());
        app.commit_edit();
        let buf = draw(&mut app, 100, 30);
        let lines = text(&buf);
        let (x, y) = find(&lines, "formula> Price * Quantty");
        let bad = x + "formula> Price * ".len() as u16;
        assert_eq!(buf[(bad, y)].fg, Color::Red);
        assert!(buf[(bad, y)].modifier.contains(Modifier::UNDERLINED));
        assert_ne!(buf[(bad - 2, y)].fg, Color::Red, "only the bad token");
        // The marker sits right under the span, then the message and hint.
        let (mx, my) = find(&lines, "^~~~~~~");
        assert_eq!((mx, my), (bad, y + 1));
        assert!(
            !lines[my as usize].contains("^~~~~~~~"),
            "as long as the token"
        );
        assert_eq!(
            row_of(&lines, "error: unknown measure: Quantty"),
            y as usize + 2
        );
        assert_eq!(
            row_of(&lines, "hint: did you mean Quantity?"),
            y as usize + 3
        );
        // Wide characters before and in the span keep it aligned.
        app.edit = Some("\u{5e02}\u{5834} + Quantty".into());
        app.commit_edit();
        let lines = text(&draw(&mut app, 100, 30));
        let y = row_of(&lines, "formula> ");
        let col = |l: &str, pat: &str| l[..l.find(pat).unwrap()].chars().count();
        let marker = col(&lines[y + 1], "^");
        assert_eq!(marker, col(&lines[y], "formula> ") + "formula> ".len());
        assert!(
            lines[y + 1].contains("^~~~ "),
            "two wide chars, four columns"
        );
        app.edit = Some("\"\u{5e02}\" & Quantty".into());
        app.commit_edit();
        let lines = text(&draw(&mut app, 100, 30));
        let y = row_of(&lines, "formula> ");
        // `"市" & ` is 7 columns wide: the 2-wide 市 counts twice.
        assert_eq!(col(&lines[y + 1], "^~~~~~~"), marker + 7);
        app.edit = Some("Price * Quantty".into());
        app.commit_edit();
        // An error without a position has no marker.
        app.diagnostic.as_mut().unwrap().span = None;
        let lines = text(&draw(&mut app, 100, 30));
        assert!(!lines.iter().any(|l| l.contains('^')));
        row_of(&lines, "error: unknown measure");
        // Typing clears it.
        app.edit_key(crossterm::event::KeyCode::Char('x'));
        let lines = text(&draw(&mut app, 100, 30));
        assert!(!lines.iter().any(|l| l.contains("error:")));
    }

    #[test]
    fn help_overlay_lists_searches_and_details() {
        let mut app = App::new(revenue_model()).unwrap();
        app.help = Some(FnHelp::new());
        let lines = text(&draw(&mut app, 100, 30));
        row_of(&lines, "search> ");
        row_of(&lines, "ABS(");
        for c in "pmt".chars() {
            app.help.as_mut().unwrap().push(c);
        }
        let buf = draw(&mut app, 100, 30);
        let lines = text(&buf);
        // The selected row is the list's first, in the selection style.
        let (x, y) = find(&lines, "PMT(rate, nper, pv, [fv=0], [type=0])");
        assert_eq!(buf[(x, y)].bg, Color::Blue);
        row_of(&lines, "category: Financial");
        row_of(&lines, "payment per period");
        // Aliases show for a function that has them.
        let h = app.help.as_mut().unwrap();
        h.query.clear();
        for c in "avg".chars() {
            h.push(c);
        }
        let lines = text(&draw(&mut app, 100, 30));
        row_of(&lines, "aliases: AVG");
        // Paging scrolls the list so the selection stays visible.
        let mut app = App::new(revenue_model()).unwrap();
        app.help = Some(FnHelp::new());
        draw(&mut app, 100, 30);
        let h = app.help.as_mut().unwrap();
        assert!(h.page > 5, "page size comes from the render: {}", h.page);
        h.page_by(1);
        h.page_by(1);
        let name = h.current().unwrap().hint();
        let lines = text(&draw(&mut app, 100, 30));
        row_of(&lines, &name);
        assert!(app.help.as_ref().unwrap().top > 0, "scrolled");
        // No match, and tiny terminals, still render.
        let h = app.help.as_mut().unwrap();
        for c in "zzzzq".chars() {
            h.push(c);
        }
        row_of(&text(&draw(&mut app, 100, 30)), "no function matches");
        for (w, h) in [(3, 3), (0, 0), (20, 6)] {
            draw(&mut app, w, h);
        }
    }

    #[test]
    fn help_packs_to_width() {
        assert_eq!(help_lines(1000).len(), 1);
        let narrow = help_lines(40);
        assert!(narrow.len() > 2);
        assert!(narrow.iter().all(|l| l.width() <= 40));
        assert_eq!(help_lines(0).len(), HELP.len(), "one entry per line");
    }

    #[test]
    fn prompt_and_full_help_visible_with_pages_and_filters() {
        // Regression: a fixed 5-row status box (3 inner lines) pushed the
        // edit prompt off-screen once a pages: or filters: line appeared, and
        // the one-line help was cut off at the right edge.
        let mut app = App::new(revenue_model()).unwrap();
        crate::app::tests::view(&mut app, improv_core_model::MeasureId(101));
        app.toggle_filter_item(
            improv_core_model::CategoryId(1),
            improv_core_model::ItemId(10),
        );
        app.edit = Some("42".into());
        let lines = text(&draw(&mut app, 60, 30));
        let prompt = row_of(&lines, "edit> 42");
        // The caret sits after the text, and moves with Left.
        let buf = draw(&mut app, 60, 30);
        let (x, _) = find(&lines, "edit> 42");
        assert!(buf[(x + 8, prompt as u16)]
            .modifier
            .contains(Modifier::REVERSED));
        app.edit.as_mut().unwrap().caret = 1;
        let buf = draw(&mut app, 60, 30);
        assert!(buf[(x + 7, prompt as u16)]
            .modifier
            .contains(Modifier::REVERSED));
        // An error shows under the open edit.
        app.status = Some("not a number: 'x'".into());
        let lines = text(&draw(&mut app, 60, 30));
        assert_eq!(row_of(&lines, "not a number"), prompt + 1);
        // A derived measure's edit is its formula.
        crate::app::tests::view(&mut app, improv_core_model::MeasureId(102));
        app.begin_edit();
        row_of(
            &text(&draw(&mut app, 60, 30)),
            "formula> Price times Quantity",
        );
        assert!(prompt < row_of(&lines, "filters: Time: 1 shown"));
        assert!(row_of(&lines, "arrows/click: move") > prompt);
        row_of(&lines, "q: quit");
    }
}
