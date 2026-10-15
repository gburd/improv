//! Rendering: a status box (title, prompt/message, pages, filters, key help)
//! sized to its content, over the pivot grid as a table.

use crate::app::{App, CommandKind, Grid, GridGeom};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};

/// Key help entries, packed greedily into lines that fit the status box.
const HELP: &[&str] = &[
    "arrows/click: move",
    "Shift+arrows: range",
    "e/Enter: edit",
    "[ ]: page",
    "p: pivot",
    "f/F: filter",
    "S: save view",
    "v: view",
    "Tab/m: measure",
    "I: import-csv",
    "E: export-csv",
    "R: revert",
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
    if let Some(buf) = &app.edit {
        lines.push(Line::from(format!("edit> {buf}")));
    } else if let Some((kind, buf)) = &app.command {
        let prompt = match kind {
            CommandKind::ImportCsv => "import-csv",
            CommandKind::ExportCsv => "export-csv",
        };
        lines.push(Line::from(format!("{prompt}> {buf}")));
    } else if let Some(msg) = &app.status {
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
        lines
            .iter()
            .position(|l| l.contains(needle))
            .unwrap_or_else(|| panic!("{needle:?} not on screen:\n{}", lines.join("\n")))
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
        app.toggle_filter_item(
            improv_core_model::CategoryId(1),
            improv_core_model::ItemId(10),
        );
        app.edit = Some("42".into());
        let lines = text(&draw(&mut app, 60, 30));
        let prompt = row_of(&lines, "edit> 42");
        assert!(prompt < row_of(&lines, "filters: Time: 1 shown"));
        assert!(row_of(&lines, "arrows/click: move") > prompt);
        row_of(&lines, "q: quit");
    }
}
