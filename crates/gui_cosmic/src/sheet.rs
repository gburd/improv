//! The worksheet grid: a canvas that paints only the cells in view, so a
//! measure of a million cells draws as fast as one of a hundred. Headers
//! stay pinned; the wheel and the scroll keys move a viewport over the
//! [`pivot::Grid`]; a click selects a cell. Colors come from the COSMIC
//! theme: its background, dividers, text, and the accent for the selection.

use cosmic::iced::mouse::{self, ScrollDelta};
use cosmic::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use cosmic::iced::{alignment, Color, Point, Rectangle, Size};
use cosmic::Theme;

use improv_core_model::{MeasureId, Model, NumberFormat};
use improv_engine::session::Snapshot;
use improv_engine::{CellValue, CoordKey};

use crate::pivot::{self, Grid};

/// Cell geometry, in logical pixels.
pub const ROW_H: f32 = 28.0;
pub const COL_W: f32 = 112.0;
/// Width of one row-header column (one per row category).
pub const HEAD_W: f32 = 120.0;
const PAD: f32 = 8.0;
const TEXT: f32 = 14.0;

/// A cell position in the grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cell {
    pub row: usize,
    pub col: usize,
}

/// What the grid tells the window.
#[derive(Debug, Clone)]
pub enum Event {
    /// The view scrolled to this top-left cell.
    Scrolled(Cell),
    /// A cell was clicked (selected).
    Clicked(Cell),
    /// A cell was double-clicked (edit it).
    Activated(Cell),
}

/// The values a measure shows: its input cells (from the model) or its
/// computed cells (from the engine's snapshot).
pub fn value(
    model: &Model,
    snapshot: &Snapshot,
    measure: MeasureId,
    k: &CoordKey,
) -> Option<CellValue> {
    let m = model.measures.get(&measure)?;
    if m.is_derived() {
        snapshot.get(&measure)?.get(k).cloned()
    } else {
        let at = improv_engine::decode_coord(k);
        CellValue::from_model_value(model.inputs.get(&(measure, at))?)
    }
}

/// How many data rows and columns fit in `size` beside the headers.
pub fn visible(size: Size, row_cats: usize, col_cats: usize) -> (usize, usize) {
    let head_h = ROW_H * col_cats.max(1) as f32;
    let head_w = HEAD_W * row_cats.max(1) as f32;
    let rows = ((size.height - head_h).max(0.0) / ROW_H).ceil() as usize;
    let cols = ((size.width - head_w).max(0.0) / COL_W).ceil() as usize;
    (rows, cols)
}

/// The grid of one measure, as the window last laid it out.
pub struct Sheet<'a> {
    pub model: &'a Model,
    pub snapshot: &'a Snapshot,
    pub measure: MeasureId,
    pub grid: &'a Grid,
    pub format: Option<NumberFormat>,
    pub locale: &'a improv_core_model::Locale,
    /// The top-left data cell in view.
    pub origin: Cell,
    pub cursor: Cell,
}

/// The canvas's own state: the last click, to tell a double click.
#[derive(Default)]
pub struct State {
    last_click: Option<(std::time::Instant, Cell)>,
}

impl Sheet<'_> {
    fn head(&self) -> (f32, f32) {
        (
            HEAD_W * self.grid.rows.cats.len().max(1) as f32,
            ROW_H * self.grid.cols.cats.len().max(1) as f32,
        )
    }

    /// The data cell under `p` (relative to the canvas), if any.
    fn cell_at(&self, p: Point) -> Option<Cell> {
        let (hw, hh) = self.head();
        if p.x < hw || p.y < hh {
            return None;
        }
        let cell = Cell {
            row: self.origin.row + ((p.y - hh) / ROW_H) as usize,
            col: self.origin.col + ((p.x - hw) / COL_W) as usize,
        };
        (cell.row < self.grid.rows.len() && cell.col < self.grid.cols.len()).then_some(cell)
    }

    /// The origin after scrolling by `rows` and `cols` (clamped).
    pub fn scrolled(&self, rows: isize, cols: isize) -> Cell {
        let clamp = |at: usize, by: isize, len: usize| {
            at.saturating_add_signed(by).min(len.saturating_sub(1))
        };
        Cell {
            row: clamp(self.origin.row, rows, self.grid.rows.len()),
            col: clamp(self.origin.col, cols, self.grid.cols.len()),
        }
    }
}

fn color(c: impl Into<Color>) -> Color {
    c.into()
}

impl<Message: Clone> canvas::Program<Message, Theme> for SheetProgram<'_, Message> {
    type State = State;

    fn update(
        &self,
        state: &mut State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let sheet = &self.sheet;
        let p = cursor.position_in(bounds)?;
        match event {
            canvas::Event::Mouse(mouse::Event::WheelScrolled { delta }) => {
                let (dx, dy) = match delta {
                    ScrollDelta::Lines { x, y } => (*x * 3.0, *y * 3.0),
                    ScrollDelta::Pixels { x, y } => (*x / COL_W, *y / ROW_H),
                };
                let to = sheet.scrolled(-dy.round() as isize, -dx.round() as isize);
                Some(canvas::Action::publish((self.on_event)(Event::Scrolled(to))).and_capture())
            }
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let cell = sheet.cell_at(p)?;
                let now = std::time::Instant::now();
                let double = state.last_click.is_some_and(|(t, c)| {
                    c == cell && now.duration_since(t) < std::time::Duration::from_millis(400)
                });
                state.last_click = Some((now, cell));
                let e = if double {
                    Event::Activated(cell)
                } else {
                    Event::Clicked(cell)
                };
                Some(canvas::Action::publish((self.on_event)(e)).and_capture())
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        _state: &State,
        renderer: &cosmic::Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let s = &self.sheet;
        let c = theme.cosmic();
        let bg = color(c.bg_color());
        let header = color(c.primary_container_color());
        let text = color(c.on_bg_color());
        let dim = color(c.on_primary_container_color());
        let rule = color(c.bg_divider());
        let accent = color(c.accent_color());
        let mut f = Frame::new(renderer, bounds.size());
        f.fill_rectangle(Point::ORIGIN, bounds.size(), bg);
        let (hw, hh) = s.head();
        let (vr, vc) = visible(
            bounds.size(),
            s.grid.rows.cats.len(),
            s.grid.cols.cats.len(),
        );
        let rows = s.origin.row..(s.origin.row + vr).min(s.grid.rows.len());
        let cols = s.origin.col..(s.origin.col + vc).min(s.grid.cols.len());
        let label = |f: &mut Frame, at: Point, w: f32, t: String, col: Color, right: bool| {
            f.fill_text(Text {
                content: t,
                position: Point::new(
                    if right { at.x + w - PAD } else { at.x + PAD },
                    at.y + ROW_H / 2.0,
                ),
                max_width: w - 2.0 * PAD,
                color: col,
                size: TEXT.into(),
                align_x: if right {
                    cosmic::iced::advanced::text::Alignment::Right
                } else {
                    cosmic::iced::advanced::text::Alignment::Left
                },
                align_y: alignment::Vertical::Center,
                ..Text::default()
            });
        };
        // Headers.
        f.fill_rectangle(Point::ORIGIN, Size::new(bounds.width, hh), header);
        f.fill_rectangle(Point::ORIGIN, Size::new(hw, bounds.height), header);
        for (d, cat) in s.grid.cols.cats.iter().enumerate() {
            let y = d as f32 * ROW_H;
            let _ = cat;
            for (i, col) in cols.clone().enumerate() {
                let Some(line) = s.grid.cols.line(col) else {
                    break;
                };
                let x = hw + i as f32 * COL_W;
                label(
                    &mut f,
                    Point::new(x, y),
                    COL_W,
                    pivot::item_name(s.model, line[d]),
                    dim,
                    false,
                );
            }
        }
        for (i, row) in rows.clone().enumerate() {
            let Some(line) = s.grid.rows.line(row) else {
                break;
            };
            let y = hh + i as f32 * ROW_H;
            for (d, item) in line.iter().enumerate() {
                let x = d as f32 * HEAD_W;
                label(
                    &mut f,
                    Point::new(x, y),
                    HEAD_W,
                    pivot::item_name(s.model, *item),
                    dim,
                    false,
                );
            }
        }
        // Cells.
        for (i, row) in rows.clone().enumerate() {
            for (j, col) in cols.clone().enumerate() {
                let at = Point::new(hw + j as f32 * COL_W, hh + i as f32 * ROW_H);
                if (Cell { row, col }) == s.cursor {
                    let sel = Color { a: 0.25, ..accent };
                    f.fill_rectangle(at, Size::new(COL_W, ROW_H), sel);
                    f.stroke_rectangle(
                        at,
                        Size::new(COL_W, ROW_H),
                        Stroke::default().with_color(accent).with_width(2.0),
                    );
                }
                let Some(k) = s.grid.key(row, col) else {
                    continue;
                };
                if let Some(v) = value(s.model, s.snapshot, s.measure, &k) {
                    let right = matches!(v, CellValue::Num(_));
                    label(
                        &mut f,
                        at,
                        COL_W,
                        v.formatted(s.format.as_ref(), s.locale),
                        text,
                        right,
                    );
                }
            }
        }
        // Rules.
        let stroke = Stroke::default().with_color(rule).with_width(1.0);
        for i in 0..=rows.len() {
            let y = hh + i as f32 * ROW_H;
            f.stroke(
                &Path::line(Point::new(0.0, y), Point::new(bounds.width, y)),
                stroke,
            );
        }
        for j in 0..=cols.len() {
            let x = hw + j as f32 * COL_W;
            f.stroke(
                &Path::line(Point::new(x, 0.0), Point::new(x, bounds.height)),
                stroke,
            );
        }
        vec![f.into_geometry()]
    }
}

/// [`Sheet`] as a canvas program, with the window's message constructor.
pub struct SheetProgram<'a, Message> {
    pub sheet: Sheet<'a>,
    pub on_event: fn(Event) -> Message,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pivot::Layout;
    use crate::service::tests::revenue_model;

    #[test]
    fn a_million_rows_scroll_in_constant_work() {
        // 1000 × 1000 items: the grid never materializes its lines.
        let mut m = Model::new();
        let (a, b) = (
            improv_core_model::CategoryId(1),
            improv_core_model::CategoryId(2),
        );
        m.add_category(a, "A");
        m.add_category(b, "B");
        for i in 0..1000 {
            m.add_item(improv_core_model::ItemId(10_000 + i), a, format!("a{i}"));
            m.add_item(improv_core_model::ItemId(20_000 + i), b, format!("b{i}"));
        }
        m.add_measure(improv_core_model::Measure {
            id: MeasureId(1),
            name: improv_core_model::Name("V".into()),
            value_type: improv_core_model::ValueType::Number,
            categories: vec![a, b],
            kind: improv_core_model::MeasureKind::Input,
            description: None,
            format: None,
        });
        // Both on rows: a million lines by one column.
        let l = Layout {
            measure: MeasureId(1),
            order: vec![a, b],
            n_rows: 2,
            n_cols: 0,
            page_items: vec![],
        };
        let g = Grid::new(&m, &l);
        assert_eq!(g.rows.len(), 1_000_000);
        let snap = Snapshot::default();
        let locale = improv_core_model::Locale::en();
        let s = Sheet {
            model: &m,
            snapshot: &snap,
            measure: MeasureId(1),
            grid: &g,
            format: None,
            locale: &locale,
            origin: Cell::default(),
            cursor: Cell::default(),
        };
        let end = s.scrolled(isize::MAX, 0);
        assert_eq!(end.row, 999_999);
        let k = g.key(end.row, 0).unwrap();
        assert_eq!(k.as_slice(), &[(1, 10_999), (2, 20_999)]);
    }

    #[test]
    fn cells_show_inputs_and_computed_values() {
        let m = revenue_model();
        let k: CoordKey = [(1, 10), (2, 20)].into_iter().collect();
        assert_eq!(
            value(&m, &Snapshot::default(), MeasureId(100), &k),
            Some(CellValue::num(100.0))
        );
        assert_eq!(value(&m, &Snapshot::default(), MeasureId(102), &k), None);
    }

    #[test]
    fn hit_testing_skips_headers_and_honors_the_origin() {
        let m = revenue_model();
        let l = Layout::natural(&m, MeasureId(100)).unwrap();
        let g = Grid::new(&m, &l);
        let snap = Snapshot::default();
        let locale = improv_core_model::Locale::en();
        let s = Sheet {
            model: &m,
            snapshot: &snap,
            measure: MeasureId(100),
            grid: &g,
            format: None,
            locale: &locale,
            origin: Cell { row: 1, col: 0 },
            cursor: Cell::default(),
        };
        assert_eq!(s.cell_at(Point::new(5.0, 5.0)), None);
        let p = Point::new(HEAD_W + COL_W + 1.0, ROW_H + 1.0);
        assert_eq!(s.cell_at(p), Some(Cell { row: 1, col: 1 }));
        assert_eq!(s.cell_at(Point::new(HEAD_W + 1.0, ROW_H * 5.0)), None);
        assert_eq!(
            visible(Size::new(HEAD_W + COL_W * 2.0, ROW_H * 3.0), 1, 1),
            (2, 2)
        );
    }
}
