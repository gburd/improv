//! The worksheet grid: a canvas that paints only the cells in view, so a
//! measure of a million cells draws as fast as one of a hundred. Headers
//! stay pinned; the wheel and the scroll keys move a viewport over the
//! [`pivot::Grid`]; a click selects a cell. Colors come from the COSMIC
//! theme: its background, dividers, text, and the accent for the selection.
//! Where the window can drag a range to other applications (Windows), a
//! press inside the range and a move start that drag.

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
/// How far a press inside the range moves before it drags the range out
/// (Windows' default drag threshold, `SM_CXDRAG`).
pub const DRAG_SLOP: f32 = 4.0;

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
    /// Shift+click: extend the selection to this cell.
    Extended(Cell),
    /// The canvas is at these bounds in the window now: the view's extent
    /// follows its size, and a drop from another application finds its cell
    /// by position.
    Placed(Rectangle),
    /// The range was pressed and dragged: drag it out of the window.
    DragOut,
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

/// Where the cell `at` is drawn, relative to the canvas, when `origin` is
/// the view's top-left: `None` when it is out of view.
pub fn cell_rect(grid: &Grid, origin: Cell, at: Cell, size: Size) -> Option<Rectangle> {
    let hw = HEAD_W * grid.rows.cats.len().max(1) as f32;
    let hh = ROW_H * grid.cols.cats.len().max(1) as f32;
    let (vr, vc) = visible(size, grid.rows.cats.len(), grid.cols.cats.len());
    let in_view = (origin.row..origin.row + vr).contains(&at.row)
        && (origin.col..origin.col + vc).contains(&at.col);
    in_view.then(|| Rectangle {
        x: hw + (at.col - origin.col) as f32 * COL_W,
        y: hh + (at.row - origin.row) as f32 * ROW_H,
        width: COL_W,
        height: ROW_H,
    })
}

/// The data cell drawn under `p` (relative to the canvas) when `origin` is
/// the view's top-left, if any.
pub fn cell_at(grid: &Grid, origin: Cell, p: Point) -> Option<Cell> {
    let hw = HEAD_W * grid.rows.cats.len().max(1) as f32;
    let hh = ROW_H * grid.cols.cats.len().max(1) as f32;
    if p.x < hw || p.y < hh {
        return None;
    }
    let cell = Cell {
        row: origin.row + ((p.y - hh) / ROW_H) as usize,
        col: origin.col + ((p.x - hw) / COL_W) as usize,
    };
    (cell.row < grid.rows.len() && cell.col < grid.cols.len()).then_some(cell)
}

/// How many whole data rows and columns fit (the page size for the keys;
/// at least one each).
pub fn whole(size: Size, row_cats: usize, col_cats: usize) -> (usize, usize) {
    let head_h = ROW_H * col_cats.max(1) as f32;
    let head_w = HEAD_W * row_cats.max(1) as f32;
    let rows = ((size.height - head_h).max(0.0) / ROW_H).floor() as usize;
    let cols = ((size.width - head_w).max(0.0) / COL_W).floor() as usize;
    (rows.max(1), cols.max(1))
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
    /// The selected range, inclusive (the cursor alone when no anchor).
    pub selection: (Cell, Cell),
    /// Where the window last heard the canvas is; a change is reported.
    pub bounds: Rectangle,
    /// A cell under an open editor: its value is not drawn.
    pub editing: Option<Cell>,
    /// A press inside a range and a move drag the range out
    /// ([`Event::DragOut`]); without it a press there selects the cell.
    pub drag_out: bool,
}

/// The canvas's own state: the last click (to tell a double click),
/// whether Shift is held, and a press inside the range that may become a
/// drag (where, relative to the canvas, and on which cell).
#[derive(Default)]
pub struct State {
    last_click: Option<(std::time::Instant, Cell)>,
    shift: bool,
    press: Option<(Point, Cell)>,
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
        cell_at(self.grid, self.origin, p)
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
        if bounds != sheet.bounds {
            let e = Event::Placed(bounds);
            return Some(canvas::Action::publish((self.on_event)(e)));
        }
        if let canvas::Event::Keyboard(cosmic::iced::keyboard::Event::ModifiersChanged(m)) = event {
            state.shift = m.shift();
            return None;
        }
        // A press inside the range waits: a move drags the range out (also
        // past the canvas's edge), a release is the click it would have been.
        if let Some((at, cell)) = state.press {
            match event {
                canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                    let moved = cursor
                        .position_from(bounds.position())
                        .is_some_and(|p| p.distance(at) >= DRAG_SLOP);
                    if !moved {
                        return None;
                    }
                    state.press = None;
                    let e = (self.on_event)(Event::DragOut);
                    return Some(canvas::Action::publish(e).and_capture());
                }
                canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                    state.press = None;
                    let e = (self.on_event)(Event::Clicked(cell));
                    return Some(canvas::Action::publish(e).and_capture());
                }
                _ => {}
            }
        }
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
                let (a, b) = sheet.selection;
                let e = if state.shift {
                    Event::Extended(cell)
                } else if double {
                    Event::Activated(cell)
                } else if sheet.drag_out && a != b && crate::nav::contains((a, b), cell) {
                    state.press = Some((p, cell));
                    return Some(canvas::Action::capture());
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
                let here = Cell { row, col };
                if crate::nav::contains(s.selection, here) {
                    f.fill_rectangle(at, Size::new(COL_W, ROW_H), Color { a: 0.18, ..accent });
                }
                if here == s.cursor {
                    let edge = Stroke::default().with_color(accent).with_width(2.0);
                    f.stroke_rectangle(at, Size::new(COL_W, ROW_H), edge);
                }
                let Some(k) = s.grid.key(row, col).filter(|_| s.editing != Some(here)) else {
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
            selection: Default::default(),
            bounds: Rectangle::default(),
            editing: None,
            drag_out: false,
        };
        let end = s.scrolled(isize::MAX, 0);
        assert_eq!(end.row, 999_999);
        let k = g.key(end.row, 0).unwrap();
        assert_eq!(k.as_slice(), &[(1, 10_999), (2, 20_999)]);
    }

    /// What a frame reads at 10^6 input cells: every visible cell's key and
    /// value. Not timed in CI (hosts vary); prints its time with
    /// `--nocapture`, and bounds it loosely.
    #[test]
    fn a_frame_of_a_million_cell_measure_reads_only_what_it_shows() {
        let (a, b) = (
            improv_core_model::CategoryId(1),
            improv_core_model::CategoryId(2),
        );
        let mut m = Model::new();
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
        for i in 0..1000 {
            for j in 0..1000 {
                m.set_input(
                    MeasureId(1),
                    improv_core_model::Coordinate::from_pairs([
                        (a, improv_core_model::ItemId(10_000 + i)),
                        (b, improv_core_model::ItemId(20_000 + j)),
                    ]),
                    improv_core_model::Value::Number(f64::from(i + j)),
                );
            }
        }
        let l = Layout::natural(&m, MeasureId(1)).unwrap();
        let g = Grid::new(&m, &l);
        let (rows, cols) = visible(Size::new(1920.0, 1200.0), 1, 1);
        let t = std::time::Instant::now();
        let mut shown = 0;
        for r in 500..500 + rows {
            for c in 500..500 + cols {
                let k = g.key(r, c).unwrap();
                shown += usize::from(value(&m, &Snapshot::default(), MeasureId(1), &k).is_some());
            }
        }
        let took = t.elapsed();
        println!("one 1920x1200 frame at 10^6 cells: {shown} cells read in {took:?}");
        assert_eq!(shown, rows * cols);
        assert!(took < std::time::Duration::from_millis(50), "{took:?}");
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

    /// What the canvas says to `event` (`None`: nothing), at `at` in a
    /// canvas placed at (10, 20).
    fn says(
        program: &SheetProgram<'_, Event>,
        state: &mut State,
        event: mouse::Event,
        at: Point,
    ) -> Option<Event> {
        let bounds = program.sheet.bounds;
        let cursor = mouse::Cursor::Available(Point::new(at.x + bounds.x, at.y + bounds.y));
        let action =
            canvas::Program::update(program, state, &canvas::Event::Mouse(event), bounds, cursor)?;
        action.into_inner().0
    }

    #[test]
    fn a_press_in_the_range_and_a_move_drag_it_out_and_a_release_clicks() {
        let m = revenue_model();
        let l = Layout::natural(&m, MeasureId(100)).unwrap();
        let g = Grid::new(&m, &l);
        let snap = Snapshot::default();
        let locale = improv_core_model::Locale::en();
        // The first row's two cells; the second row is outside.
        let range = (Cell::default(), Cell { row: 0, col: 1 });
        let program = |drag_out| SheetProgram {
            sheet: Sheet {
                model: &m,
                snapshot: &snap,
                measure: MeasureId(100),
                grid: &g,
                format: None,
                locale: &locale,
                origin: Cell::default(),
                cursor: Cell::default(),
                selection: range,
                bounds: Rectangle::new(Point::new(10.0, 20.0), Size::new(600.0, 400.0)),
                editing: None,
                drag_out,
            },
            on_event: |e| e,
        };
        let (on, off) = (program(true), program(false));
        let press = mouse::Event::ButtonPressed(mouse::Button::Left);
        let release = mouse::Event::ButtonReleased(mouse::Button::Left);
        let to = |p: Point| mouse::Event::CursorMoved { position: p };
        let in_range = Point::new(HEAD_W + COL_W + 5.0, ROW_H + 5.0);
        let cell = Cell { row: 0, col: 1 };
        // Pressed in the range: it waits; a small move is no drag yet.
        let mut st = State::default();
        assert!(says(&on, &mut st, press, in_range).is_none());
        let near = Point::new(in_range.x + 2.0, in_range.y + 2.0);
        assert!(says(&on, &mut st, to(near), near).is_none());
        // A move of the slop drags it out, past the canvas's edge too.
        let far = Point::new(-50.0, in_range.y);
        assert!(matches!(
            says(&on, &mut st, to(far), far),
            Some(Event::DragOut)
        ));
        assert!(
            says(&on, &mut st, release, far).is_none(),
            "the drag owns it"
        );
        // Pressed and released in place: a click on that cell.
        let mut st = State::default();
        assert!(says(&on, &mut st, press, in_range).is_none());
        assert!(
            matches!(says(&on, &mut st, release, in_range), Some(Event::Clicked(c)) if c == cell)
        );
        // Outside the range, or without drag-out: a click at once.
        let mut st = State::default();
        let outside = Point::new(HEAD_W + 5.0, ROW_H * 2.0 + 5.0);
        let below = Cell { row: 1, col: 0 };
        assert!(
            matches!(says(&on, &mut st, press, outside), Some(Event::Clicked(c)) if c == below)
        );
        let mut st = State::default();
        assert!(
            matches!(says(&off, &mut st, press, in_range), Some(Event::Clicked(c)) if c == cell)
        );
    }

    #[test]
    fn a_move_is_reported_as_well_as_a_resize() {
        let m = revenue_model();
        let l = Layout::natural(&m, MeasureId(100)).unwrap();
        let g = Grid::new(&m, &l);
        let snap = Snapshot::default();
        let locale = improv_core_model::Locale::en();
        let program = SheetProgram {
            sheet: Sheet {
                model: &m,
                snapshot: &snap,
                measure: MeasureId(100),
                grid: &g,
                format: None,
                locale: &locale,
                origin: Cell::default(),
                cursor: Cell::default(),
                selection: Default::default(),
                bounds: Rectangle::new(Point::new(10.0, 20.0), Size::new(600.0, 400.0)),
                editing: None,
                drag_out: false,
            },
            on_event: |e| e,
        };
        let moved = Rectangle::new(Point::new(10.0, 60.0), Size::new(600.0, 400.0));
        let a = canvas::Program::update(
            &program,
            &mut State::default(),
            &canvas::Event::Mouse(mouse::Event::CursorLeft),
            moved,
            mouse::Cursor::Unavailable,
        );
        assert!(matches!(a.and_then(|a| a.into_inner().0), Some(Event::Placed(b)) if b == moved));
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
            selection: Default::default(),
            bounds: Rectangle::default(),
            editing: None,
            drag_out: false,
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
