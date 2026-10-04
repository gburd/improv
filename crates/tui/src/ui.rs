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
    "e/Enter: edit",
    "[ ]: page",
    "p: pivot",
    "f/F: filter",
    "S: save view",
    "v: view",
    "Tab/m: measure",
    "I: import-csv",
    "E: export-csv",
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
    let coord = format!(
        "cell [{}, {}] = {}",
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

fn grid_table<'a>(app: &'a App, g: &'a Grid) -> Table<'a> {
    // Header: a corner cell (row-category name) then one per column item.
    let corner = g
        .row_cat
        .as_ref()
        .map(|(_, n)| n.clone())
        .unwrap_or_default();
    let mut header_cells = vec![Cell::from(corner)];
    header_cells.extend(g.cols.iter().map(|(_, n)| Cell::from(n.clone())));
    let header = Row::new(header_cells)
        .style(Style::default().add_modifier(Modifier::BOLD))
        .bottom_margin(0);

    let sel = Style::default()
        .bg(Color::Blue)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);

    let rows = g.rows.iter().enumerate().map(|(r, (_, rname))| {
        let mut cells =
            vec![Cell::from(rname.clone()).style(Style::default().add_modifier(Modifier::BOLD))];
        for c in 0..g.n_cols() {
            let text = g.display_at(r, c);
            let cell = Cell::from(text);
            let cell = if r == app.cursor_row && c == app.cursor_col {
                cell.style(sel)
            } else {
                cell
            };
            cells.push(cell);
        }
        Row::new(cells)
    });

    // One header column plus one per data column, evenly sized.
    let mut widths = vec![Constraint::Length(14)];
    widths.extend((0..g.n_cols()).map(|_| Constraint::Length(12)));

    Table::new(rows, widths)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Improv "))
        .column_spacing(1)
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

    // Record the grid geometry so mouse clicks can be mapped back to cells.
    // Layout mirrors `grid_table`: a bordered block (1-cell inset on each side),
    // a header row, then data rows; a row-label column (width 16) + N data
    // columns (width 12) separated by 1-cell spacing.
    let area = chunks[1];
    app.grid_geom = Some(GridGeom {
        // First data cell origin: inside the border (+1,+1), below the header (+1).
        x0: area.x + 1,
        y0: area.y + 2,
        col_w: 12,
        spacing: 1,
    });

    let table = grid_table(app, &app.grid);
    f.render_widget(table, chunks[1]);
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
