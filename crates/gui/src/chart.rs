//! Improv's graphs: Graph ▸ New Graph charts the focused worksheet's
//! selection in a NeXT graph window, in color.
//!
//! What Improv 1.0 for NeXT did (`docs/reviews/2026-11-02-nextstep-fidelity.md`
//! §3.2): Line, Bar, Scatter, Pie, Area, Stock and 3D Bar graphs, made from
//! the selected cells, with each row or each column of the selection a data
//! series, or chosen "Smart"ly: one selected row makes each column a series,
//! one selected column makes each row a series, otherwise the longer side is
//! the series. The default type is Bar. Item names label the points and the
//! axis scale spans the data. Color charts on a color NeXT; the four grays
//! on a monochrome one.
//!
//! [`ImprovApp::chart_series`] is a pure function of the app state: the
//! focused matrix's measure, filters, pivot, pinned page and selection. With
//! one cell or nothing selected it charts the whole grid. An axis filtered to
//! nothing (or a page with no pinnable item) charts nothing, as the grid
//! renders nothing. [`render_chart`] paints a [`ChartData`] with the painter
//! only. Non-numeric and error cells are gaps (`None`), never a panic. The
//! graph never changes the model or the engine.

use crate::app::ImprovApp;
use crate::theme::{BLACK, DARK_GRAY, LIGHT_GRAY, WHITE};
use egui::Color32;

/// Improv's graph types, in its own order (the beta's `DefaultChartType`
/// list): Line, Bar, Scatter, Pie, Area, Stock, 3D Bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ChartKind {
    Line,
    /// Improv's default graph type.
    #[default]
    Bar,
    Scatter,
    Pie,
    Area,
    /// High-low-close: the first three series are high, low and close.
    Stock,
    Bar3D,
}

impl ChartKind {
    /// Every type, as the Graph Inspector lists them.
    pub const ALL: [ChartKind; 7] = [
        ChartKind::Line,
        ChartKind::Bar,
        ChartKind::Scatter,
        ChartKind::Pie,
        ChartKind::Area,
        ChartKind::Stock,
        ChartKind::Bar3D,
    ];

    /// The name Improv's menus and inspector use.
    pub fn label(self) -> &'static str {
        match self {
            ChartKind::Line => "Line",
            ChartKind::Bar => "Bar",
            ChartKind::Scatter => "Scatter",
            ChartKind::Pie => "Pie",
            ChartKind::Area => "Area",
            ChartKind::Stock => "Stock",
            ChartKind::Bar3D => "3D Bar",
        }
    }
}

/// Which way the selection becomes series (Improv's Info ▸ Preferences ▸
/// Graphing: "Each row … / Each column of data selected is a series", or
/// Smart).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParseBy {
    /// Each row of the selection is a series.
    Rows,
    /// Each column of the selection is a series.
    Columns,
    /// One row: its columns are the series; one column: its rows are; else
    /// the longer side is the series (Improv's Smart Graph Selection).
    #[default]
    Smart,
}

impl ParseBy {
    pub const ALL: [ParseBy; 3] = [ParseBy::Rows, ParseBy::Columns, ParseBy::Smart];

    pub fn label(self) -> &'static str {
        match self {
            ParseBy::Rows => "Rows",
            ParseBy::Columns => "Columns",
            ParseBy::Smart => "Smart",
        }
    }

    /// Whether each COLUMN of a `rows` x `cols` selection is a series.
    fn columns_are_series(self, rows: usize, cols: usize) -> bool {
        match self {
            ParseBy::Rows => false,
            ParseBy::Columns => true,
            // "If a selection is one row, then each column of data is a
            // series; if one column, then each row; otherwise the longest of
            // the rows or columns becomes the data series."
            ParseBy::Smart if rows == 1 => true,
            ParseBy::Smart if cols == 1 => false,
            ParseBy::Smart => rows >= cols,
        }
    }
}

/// The series colors on a color display: NeXT's named colors (the
/// `NX_COLOR*` set of the NeXTSTEP 2 AppKit; the same values OpenStep's
/// `NSColor` kept), in an order that keeps neighbours far apart. Improv's
/// own default order is not documented anywhere we found; this one is a
/// reconstruction (fidelity note §3.2). It repeats past eight series.
pub const PALETTE: [Color32; 8] = [
    Color32::from_rgb(0xFF, 0x00, 0x00), // red
    Color32::from_rgb(0x00, 0x00, 0xFF), // blue
    Color32::from_rgb(0xFF, 0xFF, 0x00), // yellow
    Color32::from_rgb(0x00, 0xFF, 0x00), // green
    Color32::from_rgb(0xFF, 0x80, 0x00), // orange
    Color32::from_rgb(0x00, 0xFF, 0xFF), // cyan
    Color32::from_rgb(0x80, 0x00, 0x80), // purple
    Color32::from_rgb(0x99, 0x66, 0x33), // brown
];

/// The monochrome fills (a NeXT MegaPixel display), in Presentation
/// Builder's fill order: black, dark gray, light gray, white.
pub const GRAYS: [Color32; 4] = [BLACK, DARK_GRAY, LIGHT_GRAY, WHITE];

/// The color of series `i` (repeating), in color or in the four grays.
pub fn series_color(i: usize, color: bool) -> Color32 {
    if color {
        PALETTE[i % PALETTE.len()]
    } else {
        GRAYS[i % GRAYS.len()]
    }
}

/// The plotted data: shared x labels and named series of values aligned to
/// them. A `None` value is a gap (a non-numeric or missing cell).
#[derive(Debug, Default, PartialEq)]
pub struct ChartData {
    /// The joined names of the categories along x (e.g. "Time / Region").
    pub x_title: String,
    /// One label per x position: joined item names, in grid order.
    pub x_labels: Vec<String>,
    /// One series each (a single unnamed one for a 1-D selection).
    pub series: Vec<Series>,
}

#[derive(Debug, PartialEq)]
pub struct Series {
    /// Joined item names (empty for a 1-D grid's single series).
    pub name: String,
    /// One value per x position; `None` is a gap.
    pub points: Vec<Option<f64>>,
}

impl ChartData {
    /// The numeric min/max over all plotted points, with 0 always included.
    /// `(0.0, 1.0)` when there is no numeric data (an empty axis).
    pub fn y_range(&self) -> (f64, f64) {
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for s in &self.series {
            for y in s.points.iter().flatten() {
                lo = lo.min(*y);
                hi = hi.max(*y);
            }
        }
        if lo == hi {
            (lo, hi + 1.0)
        } else {
            (lo, hi)
        }
    }

    fn is_empty(&self) -> bool {
        self.x_labels.is_empty() || self.series.is_empty()
    }
}

impl ImprovApp {
    /// The chart data for the FOCUSED matrix, parsed `by`. The data is its
    /// range selection when one is made (more than one cell), else its whole
    /// grid; it honors the matrix's filters and pinned page items, so it is
    /// exactly what the grid shows. Before parsing, rows run along x and each
    /// column is a series; [`ParseBy`] may swap them. Pure: no egui.
    ///
    /// Empty whenever nothing is addressable: no measure, an axis category
    /// filtered to zero items, or a page category with no pinnable item
    /// (matches `Matrix::grid_dims`).
    pub fn chart_series(&self, by: ParseBy) -> ChartData {
        let Some(measure) = self.selected() else {
            return ChartData::default();
        };
        let Some((row_cats, col_cats, pinned)) = self.chart_axes_pub() else {
            return ChartData::default();
        };
        let values = self.values_for_pub(measure);
        let mut row_tuples = self.axis_tuples_pub(&row_cats);
        let mut col_tuples = self.axis_tuples_pub(&col_cats);
        if row_tuples.is_empty() || col_tuples.is_empty() {
            return ChartData::default();
        }
        // A range selection charts just itself (Improv graphs the selection).
        if let Some((rows, cols)) = self.chart_selection_pub() {
            let cut = |t: &mut Vec<_>, r: std::ops::RangeInclusive<usize>| {
                let end = (*r.end()).min(t.len().saturating_sub(1));
                *t = t
                    .drain(..)
                    .skip(*r.start())
                    .take(end + 1 - r.start())
                    .collect();
            };
            if *rows.start() < row_tuples.len() && *cols.start() < col_tuples.len() {
                cut(&mut row_tuples, rows);
                cut(&mut col_tuples, cols);
            }
        }
        let names = |cats: &[_]| {
            cats.iter()
                .filter_map(|c| self.category_name_pub(*c))
                .collect::<Vec<_>>()
                .join(" / ")
        };
        let at = |r: &[_], c: &[_]| {
            let key = self.cell_key_multi_pub(&row_cats, r, &col_cats, c, &pinned);
            values.get(&key).copied()
        };
        if by.columns_are_series(row_tuples.len(), col_tuples.len()) {
            ChartData {
                x_title: names(&row_cats),
                x_labels: row_tuples.iter().map(|t| join_names(t)).collect(),
                series: col_tuples
                    .iter()
                    .map(|c| Series {
                        name: join_names(c),
                        points: row_tuples.iter().map(|r| at(r, c)).collect(),
                    })
                    .collect(),
            }
        } else {
            ChartData {
                x_title: names(&col_cats),
                x_labels: col_tuples.iter().map(|t| join_names(t)).collect(),
                series: row_tuples
                    .iter()
                    .map(|r| Series {
                        name: join_names(r),
                        points: col_tuples.iter().map(|c| at(r, c)).collect(),
                    })
                    .collect(),
            }
        }
    }
}

/// Join a tuple's item names with " / " (empty tuple -> empty string).
fn join_names(tuple: &[(improv_core_model::ItemId, String)]) -> String {
    tuple
        .iter()
        .map(|(_, n)| n.as_str())
        .collect::<Vec<_>>()
        .join(" / ")
}

use egui::{Align2, FontId, Pos2, Rect, Shape, Stroke, Vec2};

/// Where the plot, its labels and the legend sit in the graph's rect.
struct Layout {
    plot: Rect,
    font: FontId,
}

/// Paint `data` into the current `ui` as a `kind` graph, in color or in the
/// four grays: a white plot area ruled in black, black text, an outlined
/// legend at the right. Read-only: painter shapes only.
pub fn render_chart(ui: &mut egui::Ui, data: &ChartData, kind: ChartKind, color: bool) {
    if data.is_empty() {
        ui.label("Nothing to graph: select numbers in a worksheet.");
        return;
    }
    let desired = ui.available_size_before_wrap();
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(desired.x.max(240.0), desired.y.max(160.0)),
        egui::Sense::hover(),
    );
    // H2: a screen reader hears what the picture shows.
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Image,
            true,
            format!(
                "{} graph of {} series over {}",
                kind.label(),
                data.series.len(),
                if data.x_title.is_empty() {
                    "the selection"
                } else {
                    &data.x_title
                }
            ),
        )
    });
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, WHITE);
    let font = FontId::proportional(12.0);
    let legend_w = legend_width(&p, data, &font);
    let (ml, mb, mt, mr) = if kind == ChartKind::Pie {
        (8.0, 8.0, 8.0, legend_w + 8.0)
    } else {
        (52.0, 34.0, 10.0, legend_w + 12.0)
    };
    let f = Layout {
        plot: Rect::from_min_max(
            Pos2::new(rect.left() + ml, rect.top() + mt),
            Pos2::new(rect.right() - mr, rect.bottom() - mb),
        ),
        font,
    };
    if f.plot.width() < 20.0 || f.plot.height() < 20.0 {
        return;
    }
    match kind {
        ChartKind::Pie => pie(&p, data, &f, color),
        _ => xy(&p, data, &f, kind, color),
    }
    legend(&p, data, &f, rect, kind, color);
}

/// The 1 px black outline every bar, wedge, swatch and marker carries, so a
/// white or light-gray series still shows on white.
const OUTLINE: Stroke = Stroke {
    width: 1.0,
    color: BLACK,
};

/// The legend's width: the widest series name plus its swatch.
fn legend_width(p: &egui::Painter, data: &ChartData, font: &FontId) -> f32 {
    if data.series.len() < 2 {
        return 0.0;
    }
    let w = data
        .series
        .iter()
        .map(|s| {
            p.layout_no_wrap(s.name.clone(), font.clone(), BLACK)
                .rect
                .width()
        })
        .fold(0.0, f32::max);
    w + 22.0
}

fn legend(
    p: &egui::Painter,
    data: &ChartData,
    f: &Layout,
    rect: Rect,
    kind: ChartKind,
    color: bool,
) {
    if data.series.len() < 2 {
        return;
    }
    let x = f.plot.right() + 10.0;
    for (i, s) in data.series.iter().enumerate() {
        let y = f.plot.top() + 16.0 * i as f32;
        if y + 12.0 > rect.bottom() {
            break;
        }
        let sw = Rect::from_min_size(Pos2::new(x, y + 1.0), Vec2::splat(10.0));
        // A pie's wedges are the points, so its legend lists the x labels.
        if kind != ChartKind::Pie {
            p.rect(
                sw,
                0.0,
                series_color(i, color),
                OUTLINE,
                egui::StrokeKind::Inside,
            );
            p.text(
                Pos2::new(x + 14.0, y),
                Align2::LEFT_TOP,
                &s.name,
                f.font.clone(),
                BLACK,
            );
        }
    }
    if kind == ChartKind::Pie {
        for (i, l) in data.x_labels.iter().enumerate() {
            let y = f.plot.top() + 16.0 * i as f32;
            if y + 12.0 > rect.bottom() {
                break;
            }
            let sw = Rect::from_min_size(Pos2::new(x, y + 1.0), Vec2::splat(10.0));
            p.rect(
                sw,
                0.0,
                series_color(i, color),
                OUTLINE,
                egui::StrokeKind::Inside,
            );
            p.text(
                Pos2::new(x + 14.0, y),
                Align2::LEFT_TOP,
                l,
                f.font.clone(),
                BLACK,
            );
        }
    }
}

/// A pie of the first series: one wedge per x label, outlined in black,
/// from twelve o'clock clockwise. Negative and missing values are left out.
fn pie(p: &egui::Painter, data: &ChartData, f: &Layout, color: bool) {
    let s = &data.series[0];
    let total: f64 = s.points.iter().flatten().filter(|v| **v > 0.0).sum();
    if total <= 0.0 {
        return;
    }
    let c = f.plot.center();
    let r = f.plot.width().min(f.plot.height()) / 2.0 - 2.0;
    let mut a = -std::f32::consts::FRAC_PI_2;
    for (i, v) in s.points.iter().enumerate() {
        let Some(v) = v.filter(|v| *v > 0.0) else {
            continue;
        };
        let sweep = (v / total) as f32 * std::f32::consts::TAU;
        let steps = ((sweep / 0.1).ceil() as usize).max(2);
        let mut pts = vec![c];
        pts.extend((0..=steps).map(|k| {
            let t = a + sweep * k as f32 / steps as f32;
            c + Vec2::new(t.cos(), t.sin()) * r
        }));
        // One triangle fan from the center: no seams (a wedge over half the
        // pie is not convex, and two feathered halves would show a line).
        let fill = series_color(i, color);
        let mut mesh = egui::Mesh::default();
        for q in &pts {
            mesh.colored_vertex(*q, fill);
        }
        for k in 1..pts.len() as u32 - 1 {
            mesh.add_triangle(0, k, k + 1);
        }
        p.add(Shape::mesh(mesh));
        p.add(Shape::closed_line(pts, OUTLINE));
        a += sweep;
    }
}

/// Line, Bar, Scatter, Area, Stock and 3D Bar: a value axis at the left
/// with three ruled ticks, category labels below.
fn xy(p: &egui::Painter, data: &ChartData, f: &Layout, kind: ChartKind, color: bool) {
    let plot = f.plot;
    let (y_lo, y_hi) = data.y_range();
    let span = (y_hi - y_lo).max(f64::EPSILON);
    let y_of = |v: f64| plot.bottom() - ((v - y_lo) / span) as f32 * plot.height();
    let rule = Stroke::new(1.0, LIGHT_GRAY);
    for frac in [0.0, 0.5, 1.0] {
        let v = y_lo + span * frac;
        let y = y_of(v);
        p.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            rule,
        );
        p.text(
            Pos2::new(plot.left() - 4.0, y),
            Align2::RIGHT_CENTER,
            format!("{v:.0}"),
            f.font.clone(),
            BLACK,
        );
    }
    let zero = y_of(0.0f64.clamp(y_lo, y_hi));
    let n_x = data.x_labels.len();
    let n_s = data.series.len();
    let slot = plot.width() / n_x as f32;
    let cx = |xi: usize| plot.left() + (xi as f32 + 0.5) * slot;
    match kind {
        ChartKind::Bar | ChartKind::Bar3D => {
            let pad = slot * 0.15;
            let bar_w = ((slot - 2.0 * pad) / n_s as f32).max(1.0);
            // 3D bars: an oblique top and side, as Improv's three-D bar.
            let depth = if kind == ChartKind::Bar3D {
                (bar_w * 0.4).min(8.0)
            } else {
                0.0
            };
            for (si, s) in data.series.iter().enumerate() {
                let fill = series_color(si, color);
                for (xi, v) in s.points.iter().enumerate() {
                    let Some(v) = v else { continue };
                    let x0 = plot.left() + xi as f32 * slot + pad + si as f32 * bar_w;
                    let y = y_of(*v);
                    let bar = Rect::from_min_max(
                        Pos2::new(x0, y.min(zero)),
                        Pos2::new(x0 + bar_w - 1.0, y.max(zero)),
                    );
                    if depth > 0.0 {
                        let d = Vec2::new(depth, -depth);
                        let top = vec![
                            bar.left_top(),
                            bar.left_top() + d,
                            bar.right_top() + d,
                            bar.right_top(),
                        ];
                        let side = vec![
                            bar.right_top(),
                            bar.right_top() + d,
                            bar.right_bottom() + d,
                            bar.right_bottom(),
                        ];
                        p.add(Shape::convex_polygon(top, shade(fill, 0.8), OUTLINE));
                        p.add(Shape::convex_polygon(side, shade(fill, 0.6), OUTLINE));
                    }
                    p.rect(bar, 0.0, fill, OUTLINE, egui::StrokeKind::Middle);
                }
            }
        }
        ChartKind::Area => {
            // Stacked from the axis, the first series at the bottom.
            let mut base = vec![0.0f64; n_x];
            for (si, s) in data.series.iter().enumerate() {
                let top: Vec<f64> = (0..n_x)
                    .map(|xi| base[xi] + s.points[xi].unwrap_or(0.0))
                    .collect();
                let fill = series_color(si, color);
                for xi in 0..n_x.saturating_sub(1) {
                    let quad = vec![
                        Pos2::new(cx(xi), y_of(base[xi])),
                        Pos2::new(cx(xi), y_of(top[xi])),
                        Pos2::new(cx(xi + 1), y_of(top[xi + 1])),
                        Pos2::new(cx(xi + 1), y_of(base[xi + 1])),
                    ];
                    p.add(Shape::convex_polygon(quad, fill, Stroke::NONE));
                }
                let edge: Vec<Pos2> = (0..n_x)
                    .map(|xi| Pos2::new(cx(xi), y_of(top[xi])))
                    .collect();
                p.add(Shape::line(edge, OUTLINE));
                base = top;
            }
        }
        ChartKind::Stock => {
            // High, low, close: a vertical high-low bar with a close tick.
            let get = |si: usize, xi: usize| data.series.get(si).and_then(|s| s.points[xi]);
            for xi in 0..n_x {
                let (Some(hi), Some(lo)) = (get(0, xi), get(1, xi)) else {
                    continue;
                };
                let x = cx(xi);
                p.line_segment(
                    [Pos2::new(x, y_of(hi)), Pos2::new(x, y_of(lo))],
                    Stroke::new(2.0, series_color(0, color)),
                );
                if let Some(close) = get(2, xi) {
                    let y = y_of(close);
                    p.line_segment([Pos2::new(x, y), Pos2::new(x + 5.0, y)], OUTLINE);
                }
            }
        }
        ChartKind::Line | ChartKind::Scatter => {
            for (si, s) in data.series.iter().enumerate() {
                let c = series_color(si, color);
                let mut run: Vec<Pos2> = Vec::new();
                let flush = |run: &mut Vec<Pos2>| {
                    if kind == ChartKind::Line && run.len() > 1 {
                        p.add(Shape::line(run.clone(), Stroke::new(3.0, BLACK)));
                        p.add(Shape::line(run.clone(), Stroke::new(1.5, c)));
                    }
                    run.clear();
                };
                for (xi, v) in s.points.iter().enumerate() {
                    match v {
                        Some(v) => run.push(Pos2::new(cx(xi), y_of(*v))),
                        None => flush(&mut run),
                    }
                }
                flush(&mut run);
                for (xi, v) in s.points.iter().enumerate() {
                    if let Some(v) = v {
                        marker(p, Pos2::new(cx(xi), y_of(*v)), si, c);
                    }
                }
            }
        }
        ChartKind::Pie => unreachable!("drawn by pie()"),
    }
    // The axes, over the data.
    p.line_segment([plot.left_bottom(), plot.right_bottom()], OUTLINE);
    p.line_segment([plot.left_top(), plot.left_bottom()], OUTLINE);
    if y_lo < 0.0 && y_hi > 0.0 {
        p.line_segment(
            [Pos2::new(plot.left(), zero), Pos2::new(plot.right(), zero)],
            OUTLINE,
        );
    }
    for (xi, label) in data.x_labels.iter().enumerate() {
        p.text(
            Pos2::new(cx(xi), plot.bottom() + 3.0),
            Align2::CENTER_TOP,
            label,
            f.font.clone(),
            BLACK,
        );
    }
    if !data.x_title.is_empty() {
        p.text(
            Pos2::new(plot.center().x, plot.bottom() + 18.0),
            Align2::CENTER_TOP,
            &data.x_title,
            f.font.clone(),
            DARK_GRAY,
        );
    }
}

/// A line or scatter point: square, diamond or triangle by series, outlined.
fn marker(p: &egui::Painter, at: Pos2, si: usize, fill: Color32) {
    let r = 3.5;
    let pts = match si % 3 {
        0 => vec![
            at + Vec2::new(-r, -r),
            at + Vec2::new(r, -r),
            at + Vec2::new(r, r),
            at + Vec2::new(-r, r),
        ],
        1 => vec![
            at + Vec2::new(0.0, -r - 1.0),
            at + Vec2::new(r + 1.0, 0.0),
            at + Vec2::new(0.0, r + 1.0),
            at + Vec2::new(-r - 1.0, 0.0),
        ],
        _ => vec![
            at + Vec2::new(0.0, -r - 1.0),
            at + Vec2::new(r + 1.0, r),
            at + Vec2::new(-r - 1.0, r),
        ],
    };
    p.add(Shape::convex_polygon(pts, fill, OUTLINE));
}

/// `c` darkened to `f` of its brightness (the shaded faces of a 3D bar).
fn shade(c: Color32, f: f32) -> Color32 {
    let m = |v: u8| (f32::from(v) * f).round() as u8;
    Color32::from_rgb(m(c.r()), m(c.g()), m(c.b()))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::TestFrames as _;

    /// Paint `data` in one headless 600x400 frame; return every shape drawn.
    fn paint(data: &ChartData, kind: ChartKind, color: bool) -> Vec<Shape> {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0))),
            ..Default::default()
        };
        let out = ctx.test_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| render_chart(ui, data, kind, color));
        });
        fn flat(s: Shape, acc: &mut Vec<Shape>) {
            match s {
                Shape::Vec(v) => v.into_iter().for_each(|s| flat(s, acc)),
                s => acc.push(s),
            }
        }
        let mut acc = Vec::new();
        out.shapes.into_iter().for_each(|c| flat(c.shape, &mut acc));
        acc
    }

    fn texts(shapes: &[Shape]) -> Vec<String> {
        shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }

    fn count(shapes: &[Shape], pred: impl Fn(&Shape) -> bool) -> usize {
        shapes.iter().filter(|s| pred(s)).count()
    }

    /// A bar or legend swatch: a rect outlined 1 px black.
    fn is_bar(s: &Shape) -> bool {
        matches!(s, Shape::Rect(r) if r.stroke == OUTLINE)
    }

    /// Every fill color any shape uses.
    fn fills(shapes: &[Shape]) -> Vec<Color32> {
        shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Rect(r) => Some(r.fill),
                Shape::Path(p) => Some(p.fill),
                _ => None,
            })
            .collect()
    }

    fn two_series() -> ChartData {
        ChartData {
            x_title: "Time".into(),
            x_labels: vec!["q1".into(), "q2".into(), "q3".into(), "q4".into()],
            series: vec![
                Series {
                    name: "North".into(),
                    points: vec![Some(1.0), None, Some(-2.0), Some(3.0)],
                },
                Series {
                    name: "South".into(),
                    points: vec![Some(4.0), Some(5.0), None, None],
                },
            ],
        }
    }

    #[test]
    fn y_range_of_no_or_flat_data_is_zero_to_one_above() {
        assert_eq!(ChartData::default().y_range(), (0.0, 1.0));
        let flat = ChartData {
            series: vec![Series {
                name: String::new(),
                points: vec![Some(0.0), None],
            }],
            ..Default::default()
        };
        assert_eq!(flat.y_range(), (0.0, 1.0));
        let mixed = ChartData {
            series: vec![Series {
                name: String::new(),
                points: vec![Some(-2.0), Some(5.0)],
            }],
            ..Default::default()
        };
        assert_eq!(mixed.y_range(), (-2.0, 5.0));
    }

    /// Improv's Smart Graph Selection: one row -> columns are series; one
    /// column -> rows are; else the longer side is the series.
    #[test]
    fn smart_parsing_follows_improvs_rule() {
        use ParseBy::*;
        assert!(Smart.columns_are_series(1, 5), "one row");
        assert!(!Smart.columns_are_series(5, 1), "one column");
        assert!(Smart.columns_are_series(6, 3), "more rows: x is rows");
        assert!(
            !Smart.columns_are_series(3, 6),
            "more columns: x is columns"
        );
        assert!(Columns.columns_are_series(1, 5) && Columns.columns_are_series(5, 1));
        assert!(!Rows.columns_are_series(1, 5) && !Rows.columns_are_series(5, 1));
    }

    #[test]
    fn improvs_seven_types_have_their_names_and_bar_is_the_default() {
        let names: Vec<_> = ChartKind::ALL.iter().map(|k| k.label()).collect();
        assert_eq!(
            names,
            ["Line", "Bar", "Scatter", "Pie", "Area", "Stock", "3D Bar"]
        );
        assert_eq!(ChartKind::default(), ChartKind::Bar);
        assert_eq!(ParseBy::default(), ParseBy::Smart);
    }

    #[test]
    fn empty_chart_says_nothing_to_graph_and_draws_no_bars() {
        for data in [
            ChartData::default(),
            ChartData {
                x_labels: vec!["a".into()],
                ..Default::default()
            },
        ] {
            let shapes = paint(&data, ChartKind::Bar, true);
            assert!(texts(&shapes)
                .iter()
                .any(|t| t.contains("Nothing to graph")));
            assert_eq!(count(&shapes, is_bar), 0);
        }
    }

    /// Color graphs use NeXT's named colors, one per series, and the legend
    /// swatches match their bars; the gray graph uses only the four grays.
    #[test]
    fn bars_are_in_next_colors_or_in_the_four_grays() {
        let data = two_series();
        let shapes = paint(&data, ChartKind::Bar, true);
        // 5 numeric points -> 5 bars, + 2 legend swatches.
        assert_eq!(count(&shapes, is_bar), 7);
        let f = fills(&shapes);
        assert_eq!(f.iter().filter(|c| **c == PALETTE[0]).count(), 3 + 1);
        assert_eq!(f.iter().filter(|c| **c == PALETTE[1]).count(), 2 + 1);
        let gray = paint(&data, ChartKind::Bar, false);
        for c in fills(&gray) {
            assert!(
                c.r() == c.g() && c.g() == c.b() || c == Color32::TRANSPARENT,
                "{c:?} in a gray graph"
            );
        }
        let t = texts(&shapes);
        for want in ["Time", "North", "South", "q1", "q4"] {
            assert!(t.iter().any(|x| x == want), "{want} missing from {t:?}");
        }
    }

    #[test]
    fn palette_colors_are_distinct_and_repeat() {
        let mut seen = PALETTE.to_vec();
        seen.dedup();
        assert_eq!(seen.len(), 8);
        assert_eq!(series_color(8, true), PALETTE[0]);
        assert_eq!(series_color(5, false), GRAYS[1]);
    }

    /// A line graph breaks at a gap and marks each point; scatter marks
    /// without lines.
    #[test]
    fn lines_break_at_gaps_and_scatter_has_no_lines() {
        let data = two_series();
        let path_lines = |s: &[Shape]| count(s, |s| matches!(s, Shape::Path(p) if !p.closed));
        let markers = |s: &[Shape]| {
            count(
                s,
                |s| matches!(s, Shape::Path(p) if p.closed && p.stroke.width == 1.0),
            )
        };
        let line = paint(&data, ChartKind::Line, true);
        // North: q3-q4 run (q1 alone draws no line); South: q1-q2. Each run
        // is a black under-stroke and a colored stroke.
        assert_eq!(path_lines(&line), 4);
        assert_eq!(markers(&line), 5);
        let scatter = paint(&data, ChartKind::Scatter, true);
        assert_eq!(path_lines(&scatter), 0);
        assert_eq!(markers(&scatter), 5);
    }

    #[test]
    fn a_pie_has_a_wedge_per_positive_point_and_lists_the_labels() {
        let data = ChartData {
            x_title: "Product".into(),
            x_labels: vec!["a".into(), "b".into(), "c".into()],
            series: vec![Series {
                name: String::new(),
                points: vec![Some(1.0), Some(3.0), Some(-1.0)],
            }],
        };
        let shapes = paint(&data, ChartKind::Pie, true);
        let outlines = count(
            &shapes,
            |s| matches!(s, Shape::Path(p) if p.closed && p.fill == Color32::TRANSPARENT),
        );
        assert_eq!(outlines, 2, "the negative point has no wedge");
        let wedges: Vec<Color32> = shapes
            .iter()
            .filter_map(|s| match s {
                Shape::Mesh(m) => m.vertices.first().map(|v| v.color),
                _ => None,
            })
            .collect();
        assert_eq!(wedges, [PALETTE[0], PALETTE[1]]);
    }

    #[test]
    fn area_stock_and_3d_bar_draw_without_panicking() {
        let data = two_series();
        for kind in [ChartKind::Area, ChartKind::Stock, ChartKind::Bar3D] {
            let shapes = paint(&data, kind, true);
            assert!(texts(&shapes).iter().any(|t| t == "q1"), "{kind:?}");
        }
        // 3D bars: each bar has a top and a side face.
        let bars3d = paint(&data, ChartKind::Bar3D, true);
        assert_eq!(
            count(
                &bars3d,
                |s| matches!(s, Shape::Path(p) if p.closed && p.points.len() == 4)
            ),
            10
        );
        // Stock: a high-low bar only where both exist (q1 alone here).
        let stock = paint(&data, ChartKind::Stock, true);
        assert_eq!(
            count(
                &stock,
                |s| matches!(s, Shape::LineSegment { stroke, .. } if stroke.width == 2.0)
            ),
            1
        );
    }

    #[test]
    fn no_measure_charts_nothing_and_all_negative_data_has_no_zero_line() {
        let app = ImprovApp::load("").expect("in-memory model");
        assert_eq!(app.chart_series(ParseBy::Smart), ChartData::default());
        let data = ChartData {
            x_title: String::new(),
            x_labels: vec!["a".into(), "b".into()],
            series: vec![Series {
                name: String::new(),
                points: vec![Some(-1.0), Some(-2.0)],
            }],
        };
        assert_eq!(data.y_range(), (-2.0, 0.0));
        let shapes = paint(&data, ChartKind::Bar, false);
        assert_eq!(count(&shapes, is_bar), 2);
    }

    /// `n` series named `s0`.. over `x` labels, every point 1.
    fn many(n: usize, x: usize) -> ChartData {
        ChartData {
            x_title: "Time".into(),
            x_labels: (0..x).map(|i| format!("x{i}")).collect(),
            series: (0..n)
                .map(|i| Series {
                    name: format!("s{i}"),
                    points: vec![Some(1.0); x],
                })
                .collect(),
        }
    }

    /// A pie of two or more series lists its x labels, not the series, and
    /// a legend longer than the graph stops at its bottom edge.
    #[test]
    fn a_pies_legend_lists_the_points_and_legends_stop_at_the_bottom() {
        let shapes = paint(&many(2, 3), ChartKind::Pie, true);
        let t = texts(&shapes);
        assert!(t.iter().any(|x| x == "x2"), "{t:?}");
        assert!(!t.iter().any(|x| x == "s1"), "{t:?}");
        let swatch = |s: &Shape| matches!(s, Shape::Rect(r) if r.stroke == OUTLINE);
        assert_eq!(count(&shapes, swatch), 3, "a swatch per x label");
        // 40 rows at 16 px do not fit 400 px: the legend is cut.
        let bars = paint(&many(40, 1), ChartKind::Bar, true);
        let names = texts(&bars)
            .into_iter()
            .filter(|x| x.starts_with('s'))
            .count();
        assert!(names > 5 && names < 40, "{names} series named");
        let pie = paint(&many(2, 40), ChartKind::Pie, true);
        let labels = texts(&pie)
            .into_iter()
            .filter(|x| x.starts_with('x'))
            .count();
        assert!(labels > 5 && labels < 40, "{labels} points named");
    }

    /// A pie with nothing positive draws no wedge.
    #[test]
    fn a_pie_of_no_positive_values_is_empty() {
        let data = ChartData {
            x_title: "Product".into(),
            x_labels: vec!["a".into(), "b".into()],
            series: vec![Series {
                name: String::new(),
                points: vec![Some(-1.0), None],
            }],
        };
        let shapes = paint(&data, ChartKind::Pie, true);
        assert_eq!(count(&shapes, |s| matches!(s, Shape::Mesh(_))), 0);
    }

    /// Stock: the third series is the close, a tick right of the bar.
    #[test]
    fn a_stock_graph_ticks_the_close() {
        let mut data = many(3, 2);
        data.series[0].points = vec![Some(9.0), Some(8.0)];
        data.series[1].points = vec![Some(2.0), Some(3.0)];
        data.series[2].points = vec![Some(5.0), None];
        let shapes = paint(&data, ChartKind::Stock, true);
        let high_low =
            |s: &Shape| matches!(s, Shape::LineSegment { stroke, .. } if stroke.width == 2.0);
        assert_eq!(count(&shapes, high_low), 2);
        let ticks = count(&shapes, |s| {
            matches!(s, Shape::LineSegment { points, stroke }
                if *stroke == OUTLINE && (points[1].x - points[0].x - 5.0).abs() < 1e-3)
        });
        assert_eq!(ticks, 1, "only the first x has a close");
    }

    /// A legend wider than the graph leaves no room to plot: nothing is
    /// drawn but the white field.
    #[test]
    fn no_room_to_plot_draws_no_data() {
        let mut data = many(2, 2);
        data.series[0].name = "W".repeat(200);
        let shapes = paint(&data, ChartKind::Bar, true);
        assert_eq!(count(&shapes, is_bar), 0);
        assert!(!texts(&shapes).iter().any(|t| t == "x0"));
    }

    /// A screen reader hears the graph's type, series and x axis, or "the
    /// selection" when the x axis has no category.
    #[test]
    fn the_graph_is_described_to_a_screen_reader() {
        let label = |data: &ChartData| {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(600.0, 400.0))),
                ..Default::default()
            };
            let out = ctx.test_ui(raw, |ui| {
                egui::CentralPanel::default()
                    .show(ui, |ui| render_chart(ui, data, ChartKind::Line, true));
            });
            out.platform_output
                .accesskit_update
                .expect("AccessKit is on")
                .nodes
                .iter()
                .find(|(_, n)| n.role() == egui::accesskit::Role::Image)
                .and_then(|(_, n)| n.label().map(str::to_owned))
        };
        let mut data = many(2, 3);
        assert_eq!(
            label(&data).as_deref(),
            Some("Line graph of 2 series over Time")
        );
        data.x_title.clear();
        assert_eq!(
            label(&data).as_deref(),
            Some("Line graph of 2 series over the selection")
        );
    }
}
