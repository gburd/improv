//! Rendering: a header/status line plus the pivot grid as a table.

use crate::app::{App, Grid};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Cell, Row, Table},
    Frame,
};

fn fmt_cell(v: Option<f64>) -> String {
    match v {
        Some(n) => {
            if n.is_nan() {
                "#ERR".to_string()
            } else if n.fract() == 0.0 && n.abs() < 1e15 {
                format!("{n:.0}")
            } else {
                format!("{n:.2}")
            }
        }
        None => String::new(),
    }
}

fn status_lines(app: &App) -> Vec<Line<'static>> {
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
        fmt_cell(g.value_at(app.cursor_row, app.cursor_col)),
    );

    let mut lines = vec![Line::from(format!(
        "{} [{}]   {axes}   {coord}",
        g.measure_name, kind
    ))];
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
    lines.push(Line::from(
        "arrows: move   e/Enter: edit   [ ]: page   Tab/m: measure   Esc: cancel   q: quit",
    ));
    if let Some(buf) = &app.edit {
        lines.push(Line::from(format!("edit> {buf}")));
    } else if let Some(msg) = &app.status {
        lines.push(Line::from(msg.clone()));
    }
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
            let text = fmt_cell(g.value_at(r, c));
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

pub fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(5), Constraint::Min(3)])
        .split(f.area());

    let status = ratatui::widgets::Paragraph::new(status_lines(app))
        .block(Block::default().borders(Borders::ALL).title(" Status "));
    f.render_widget(status, chunks[0]);

    let table = grid_table(app, &app.grid);
    f.render_widget(table, chunks[1]);
}
