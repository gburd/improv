//! Rendering: a status box (title, prompt/message, pages, filters, key help)
//! sized to its content, over the pivot grid as a table.

use crate::app::{App, CommandKind, Grid, GridGeom};
use crate::diagnose::Diagnostic;
use crate::edit::LineEdit;
use crate::fnhelp::FnHelp;
use improv_engine::CellValue;
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
    "e/Enter/0-9: edit",
    "Tab (formula): complete",
    "[ ]: page",
    "p: pivot",
    "f/F: filter",
    "S: save view",
    "v: view",
    "Tab/m: measure",
    "I: import-csv",
    "E: export-csv",
    "c: category/items",
    "n: input measure",
    "=: define",
    "w: save",
    "R: revert",
    "u/Ctrl+R: undo/redo",
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

    let mut lines = vec![Line::from(if app.measures.is_empty() {
        crate::app::EMPTY.to_string()
    } else {
        format!("{} [{}]   {axes}   {coord}", g.measure_name, kind)
    })];
    if let Some(f) = app.formula_text() {
        lines.push(Line::from(format!("{} = {f}", g.measure_name)));
    }
    if let Some(e) = &app.edit {
        let formula = app.editing_formula();
        let prompt = match (app.defining, formula) {
            (true, _) => "define> ",
            (_, true) => "formula> ",
            _ => "edit> ",
        };
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
            CommandKind::Category => "category",
            CommandKind::Input => "input",
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
/// underlined. A fresh edit's text is reversed too, as selected: typing
/// replaces it. In a formula with the caret at the end inside a call, the
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
        if i == e.caret || e.fresh {
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
/// The widest a data column grows to fit its values (H1-3).
const MAX_COL_W: u16 = 20;

/// How many of `widths` fit side by side in `room` columns, each after a
/// `SPACING` gap: at least one, when there is one.
fn fitting(widths: &[u16], room: u16) -> usize {
    let mut used = 0u16;
    let n = widths
        .iter()
        .take_while(|w| {
            used = used.saturating_add(**w + SPACING);
            used <= room
        })
        .count();
    n.max(1).min(widths.len())
}

/// A number's text `s` in `w` columns (H1-3): as it is if it fits, else
/// with fewer decimals, else `#` across the cell. Never fewer digits
/// before the point, so a cut-off number never reads as a smaller one.
fn fit_number(s: &str, w: usize) -> String {
    if s.len() <= w {
        return s.into();
    }
    let (head, exp) = s.split_at(s.find('E').unwrap_or(s.len()));
    if let (Some(dot), Ok(m)) = (head.find('.'), head.parse::<f64>()) {
        let most = w.saturating_sub(dot + 1 + exp.len());
        if let Some(r) = (0..=most)
            .rev()
            .map(|d| format!("{m:.d$}{exp}"))
            .find(|r| r.len() <= w)
        {
            return r;
        }
    }
    "#".repeat(w)
}

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
    let cols = geom.col0..geom.col0 + geom.col_w.len();
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

    // H2: each also has a modifier, so NO_COLOR (or a monochrome terminal)
    // still shows the cursor (reversed) and the range (underlined).
    let sel = Style::default()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD | Modifier::REVERSED);

    let cols_r = cols.clone();
    let col_w = &geom.col_w;
    let range = Style::default()
        .bg(Color::Cyan)
        .fg(Color::Black)
        .add_modifier(Modifier::UNDERLINED);
    let rows = rows.map(move |r| {
        let rname = g.rows[r].1.clone();
        let mut cells =
            vec![Cell::from(rname).style(Style::default().add_modifier(Modifier::BOLD))];
        for c in cols_r.clone() {
            let mut text = g.display_at(r, c);
            if let Some(CellValue::Num(_)) = g.cell_at(r, c) {
                text = fit_number(&text, col_w[c - geom.col0] as usize);
            }
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
    widths.extend(geom.col_w.iter().map(|w| Constraint::Length(*w)));

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
    // H1-3: a column is as wide as its widest value, at least COL_W, at
    // most MAX_COL_W or the room there is; `fit_number` does the rest.
    let room = inner_w.saturating_sub(LABEL_W);
    let cap = MAX_COL_W.min(room.saturating_sub(SPACING)).max(1);
    let widths: Vec<u16> = (app.grid.widths.iter())
        .map(|&w| (w.min(cap.into()) as u16).max(COL_W.min(cap)))
        .collect();
    // Scroll the columns minimally so the cursor's is in the window.
    let prev = app.grid_geom.as_ref().map_or((0, 0), |g| (g.row0, g.col0));
    let mut col0 = prev.1.min(app.cursor_col).min(widths.len());
    while col0 < app.cursor_col && app.cursor_col >= col0 + fitting(&widths[col0..], room) {
        col0 += 1;
    }
    let shown = fitting(&widths[col0..], room);
    let geom = GridGeom {
        x0: area.x + 1 + LABEL_W + SPACING,
        y0: area.y + 2,
        col_w: widths[col0..col0 + shown].to_vec(),
        spacing: SPACING,
        row0: scroll(prev.0, app.cursor_row, vis_rows),
        col0,
        rows: vis_rows,
    };
    app.grid_geom = Some(geom.clone());

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
        .title(" Keys and functions  (type to search, Enter inserts, Esc closes) ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    // The keys first, then the function search.
    let keys = help_lines(inner.width as usize);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(keys.len() as u16 + 1),
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(6),
        ])
        .split(inner);
    f.render_widget(
        Paragraph::new(keys).block(Block::default().borders(Borders::BOTTOM)),
        chunks[0],
    );
    let chunks = &chunks[1..];

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
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
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

    /// H2: with every color stripped (NO_COLOR, a monochrome terminal),
    /// what color marks still shows: the cursor cell, the range, the error
    /// token and the help selection each keep a modifier, and the error
    /// lines say "error:".
    #[test]
    fn nothing_is_shown_by_color_alone() {
        let mut app = App::new(revenue_model()).unwrap();
        app.step(1, 0, true);
        let buf = draw(&mut app, 100, 30);
        let marked = |x, y| -> Modifier { buf[(x, y)].modifier };
        let lines = text(&buf);
        let (x, y) = find(&lines, "2025");
        let data_x = x + LABEL_W + SPACING;
        assert!(marked(data_x, y).contains(Modifier::UNDERLINED), "range");
        assert!(marked(data_x, y + 1).contains(Modifier::REVERSED), "cursor");
        assert!(!marked(data_x + COL_W + SPACING, y)
            .intersects(Modifier::UNDERLINED | Modifier::REVERSED));
        // Every cell with a background color carries a modifier too, in the
        // grid and the help overlay's selection.
        for help in [false, true] {
            app.help = help.then(crate::fnhelp::FnHelp::new);
            let buf = draw(&mut app, 100, 30);
            for (i, c) in buf.content.iter().enumerate() {
                if c.bg != Color::Reset {
                    assert!(
                        !c.modifier.is_empty(),
                        "cell {i} ({:?}) is marked by color only",
                        c.symbol()
                    );
                }
            }
        }
        // A formula error: the bad token underlined, the lines say error.
        let mut app = App::new(revenue_model()).unwrap();
        app.begin_define();
        for c in "Bad = Prise * 2".chars() {
            app.edit_key(crossterm::event::KeyCode::Char(c));
        }
        app.commit_edit();
        let buf = draw(&mut app, 100, 30);
        let lines = text(&buf);
        assert!(lines.iter().any(|l| l.contains("error: ")), "{lines:?}");
        // Red text is either marked (the ^~~ marker, the token) or on a
        // line that says "error:".
        let w = buf.area.width as usize;
        for (i, c) in buf.content.iter().enumerate() {
            if c.fg == Color::Red && c.modifier.is_empty() {
                assert!(lines[i / w].contains("error: "), "red only at {i}");
            }
        }
        let (bx, by) = find(&lines, "Prise");
        assert!(marked_in(&buf, bx, by).contains(Modifier::UNDERLINED));
    }

    fn marked_in(buf: &Buffer, x: u16, y: u16) -> Modifier {
        buf[(x, y)].modifier
    }

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
    fn numbers_too_wide_round_or_show_hashes() {
        assert_eq!(fit_number("300", 12), "300");
        assert_eq!(fit_number("12345678901234", 12), "############");
        assert_eq!(fit_number("-1234567590.125", 12), "-1234567590");
        assert_eq!(fit_number("3.14159", 4), "3.14");
        assert_eq!(fit_number("99.96", 4), "100", "a carry drops a decimal");
        assert_eq!(fit_number("1.25E+20", 6), "1E+20");
        assert_eq!(fit_number("1.5E-07", 4), "####");
        assert_eq!(fitting(&[12, 12], 26), 2);
        assert_eq!(fitting(&[12, 12], 25), 1);
        assert_eq!(fitting(&[40], 5), 1, "one always shows");
        assert_eq!(fitting(&[], 50), 0);
    }

    /// H1-3: a wide number widens its column (never drops its leading
    /// digits); past the cap a fraction rounds; on a narrow terminal a
    /// number that cannot fit shows `#`, and clicks follow the widths.
    #[test]
    fn wide_numbers_widen_their_column_and_never_lose_digits() {
        use improv_core_model::{CategoryId, Coordinate, ItemId, MeasureId};
        let mut app = App::new(revenue_model()).unwrap();
        crate::app::tests::view(&mut app, MeasureId(101));
        app.apply_edit(0, 0, 12345678901234.0).unwrap();
        app.apply_edit(1, 0, -1234567590.125).unwrap();
        app.apply_edit(0, 1, -1.23456789012345e20).unwrap();
        app.apply_edit(1, 1, 1.0 / 3.0).unwrap();
        // The status line shows the cursor's cell: keep it off the numbers.
        app.cursor_row = 1;
        app.cursor_col = 1;
        let lines = text(&draw(&mut app, 100, 30));
        let (x, y) = find(&lines, "12345678901234");
        row_of(&lines, "-1234567590.125");
        row_of(&lines, "0.333333333333333");
        // Past the 20-column cap a fraction rounds.
        let (x1, y1) = find(&lines, "-1.2345678901235E+20");
        assert_eq!(y1, y);
        assert_eq!(x1, x + 15 + SPACING, "the first column is 15 wide");
        app.click_at(x1, y1);
        assert_eq!((app.cursor_row, app.cursor_col), (0, 1));
        app.click_at(x1 - 1, y1);
        assert_eq!(
            (app.cursor_row, app.cursor_col),
            (0, 0),
            "the gap is col 0's"
        );
        app.click_at(x1 + 21, y1);
        assert_eq!(
            (app.cursor_row, app.cursor_col),
            (0, 0),
            "past the last column"
        );
        // 28 columns leave 11 for a data column: no number loses a digit.
        app.cursor_row = 1;
        let lines = text(&draw(&mut app, 28, 24));
        let y = row_of(&lines, "###########");
        assert!(
            lines[y + 1].contains("-1234567590\u{2502}"),
            "{:?}",
            lines[y + 1]
        );
        assert!(!lines.iter().any(|l| l.contains("1234567890")), "{lines:?}");
        let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
        assert!(app.model.input(MeasureId(101), &at).is_some());
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
        let lines = text(&draw(&mut app, 32, 24));
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
        let buf = draw(&mut app, 32, 24);
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
        // The overlay lists the keys above the search.
        assert!(row_of(&lines, "=: define") < row_of(&lines, "search> "));
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
    fn empty_model_and_build_prompts_render() {
        let mut app = App::new(improv_core_model::Model::new()).unwrap();
        let lines = text(&draw(&mut app, 100, 30));
        row_of(&lines, "no measures yet: c adds a category");
        app.begin_command(CommandKind::Category);
        app.command.as_mut().unwrap().1 = "Time: Q1".into();
        row_of(&text(&draw(&mut app, 100, 30)), "category> Time: Q1");
        app.begin_command(CommandKind::Input);
        row_of(&text(&draw(&mut app, 100, 30)), "input> ");
        app.cancel_command();
        app.begin_define();
        app.edit = Some("X = PMT(".into());
        let lines = text(&draw(&mut app, 100, 30));
        let y = row_of(&lines, "define> X = PMT(");
        assert!(
            lines[y + 1].contains("PMT("),
            "a definition shows the signature"
        );
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
        assert!(!buf[(x + 6, prompt as u16)]
            .modifier
            .contains(Modifier::REVERSED));
        // A fresh edit shows its old value selected: typing replaces it.
        app.edit.as_mut().unwrap().fresh = true;
        let buf = draw(&mut app, 60, 30);
        assert!(buf[(x + 6, prompt as u16)]
            .modifier
            .contains(Modifier::REVERSED));
        app.edit.as_mut().unwrap().fresh = false;
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
