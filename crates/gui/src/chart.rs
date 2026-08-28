//! A read-only chart view of the selected measure.
//!
//! `chart_series` is a pure function of the app state (snapshot + filters +
//! pivot/page): the x-axis is the full Cartesian product of the ROW categories
//! (labels are the joined tuple names, e.g. "2024Q1 / North"), and there is one
//! series per full COLUMN tuple (series name = joined column tuple names). The
//! single-category case is the 1-length-tuple case; a 1-D grid (no columns) is
//! one unnamed series. No egui calls. `render_chart` just paints that
//! `ChartData` with `egui::Painter` (grouped bars, optional line overlay).
//! Non-numeric / Error cells are gaps (`None`), never a panic. The chart never
//! mutates the model or engine.

use crate::app::ImprovApp;

/// The plotted data for the current measure/pivot: shared x-labels and one or
/// more named series of y-values aligned to those labels. A `None` y is a gap
/// (non-numeric / missing cell).
#[derive(Debug, Default, PartialEq)]
pub struct ChartData {
    /// X-axis title: the joined ROW category names (e.g. "Time / Region").
    pub x_title: String,
    /// One label per x position: the joined row-tuple item names, in grid order.
    pub x_labels: Vec<String>,
    /// One series per full column tuple (a single unnamed series if 1-D).
    pub series: Vec<Series>,
}

#[derive(Debug, PartialEq)]
pub struct Series {
    /// Joined column-tuple item names (empty for a 1-D grid's single series).
    pub name: String,
    /// y-value per x position; `None` is a gap.
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
}

impl ImprovApp {
    /// Build the chart data for the selected measure from the current snapshot,
    /// honoring the active filters and pinned page items (same visible-item set
    /// as the grid). The x-axis is the full Cartesian product of the ROW
    /// categories (x-label = joined tuple item names, e.g. "2024Q1 / North");
    /// one series per full COLUMN tuple (series name = joined column tuple
    /// names). The single-category case is just the 1-length-tuple case. A 1-D
    /// grid (no column categories) is a single unnamed series. Non-numeric /
    /// missing cells are gaps (`None`). Pure: no egui, no mutation.
    pub fn chart_series(&self) -> ChartData {
        let Some(measure) = self.selected() else {
            return ChartData::default();
        };
        let (row_cats, col_cats, pinned) = self.chart_axes_pub();
        let values = self.values_for_pub(measure);

        // Full row/column products (each element is a tuple of (ItemId, name)).
        let row_tuples = self.axis_tuples_pub(&row_cats);
        let col_tuples = self.axis_tuples_pub(&col_cats);
        // An empty product (a category filtered to nothing) yields no x/series.
        let row_tuples = if row_tuples.is_empty() {
            vec![Vec::new()]
        } else {
            row_tuples
        };
        let col_tuples = if col_tuples.is_empty() {
            vec![Vec::new()]
        } else {
            col_tuples
        };

        // Axis title: the joined row-category names (e.g. "Time / Region").
        let x_title = row_cats
            .iter()
            .filter_map(|c| self.category_name_pub(*c))
            .collect::<Vec<_>>()
            .join(" / ");
        // x-label per row tuple: joined item names.
        let x_labels: Vec<String> = row_tuples.iter().map(|t| join_names(t)).collect();

        let series = col_tuples
            .iter()
            .map(|col_tuple| Series {
                name: join_names(col_tuple),
                points: row_tuples
                    .iter()
                    .map(|row_tuple| {
                        let key = self.cell_key_multi_pub(
                            &row_cats, row_tuple, &col_cats, col_tuple, &pinned,
                        );
                        values.get(&key).copied()
                    })
                    .collect(),
            })
            .collect();

        ChartData {
            x_title,
            x_labels,
            series,
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

/// Paint `data` into the current `ui` as grouped bars (and, when `line` is set,
/// an overlaid line per series). Read-only: draws with `egui::Painter` only.
pub fn render_chart(ui: &mut egui::Ui, data: &ChartData, line: bool) {
    use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2};

    if data.x_labels.is_empty() || data.series.is_empty() {
        ui.weak("(no data to chart — select a numeric measure with items)");
        return;
    }

    let desired = ui.available_size_before_wrap();
    let (rect, _resp) = ui.allocate_exact_size(
        Vec2::new(desired.x.max(240.0), desired.y.max(200.0)),
        Sense::hover(),
    );
    let painter = ui.painter_at(rect);

    let (y_lo, y_hi) = data.y_range();
    let span = (y_hi - y_lo).max(f64::EPSILON);

    // Plot area: leave margins for the y-scale (left) and x-labels (bottom).
    let ml = 48.0;
    let mb = 28.0;
    let mt = 8.0;
    let mr = 8.0;
    let plot = Rect::from_min_max(
        Pos2::new(rect.left() + ml, rect.top() + mt),
        Pos2::new(rect.right() - mr, rect.bottom() - mb),
    );

    let visuals = ui.visuals();
    let axis_stroke = Stroke::new(1.0_f32, visuals.weak_text_color());
    let text_color = visuals.text_color();
    let font = FontId::proportional(11.0);

    // y for a data value (top = y_hi).
    let y_of = |v: f64| -> f32 {
        let t = (v - y_lo) / span;
        plot.bottom() - t as f32 * plot.height()
    };

    // Axes.
    painter.line_segment([plot.left_bottom(), plot.right_bottom()], axis_stroke);
    painter.line_segment([plot.left_top(), plot.left_bottom()], axis_stroke);

    // y-scale ticks (lo, mid, hi) with a light gridline and label.
    for frac in [0.0, 0.5, 1.0] {
        let v = y_lo + span * frac;
        let y = y_of(v);
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            Stroke::new(0.5_f32, visuals.faint_bg_color),
        );
        painter.text(
            Pos2::new(plot.left() - 4.0, y),
            Align2::RIGHT_CENTER,
            format!("{v:.0}"),
            font.clone(),
            text_color,
        );
    }
    // Zero line, if 0 is strictly inside the range.
    if y_lo < 0.0 && y_hi > 0.0 {
        let y = y_of(0.0);
        painter.line_segment(
            [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
            axis_stroke,
        );
    }

    let n_x = data.x_labels.len();
    let n_s = data.series.len();
    let slot = plot.width() / n_x as f32; // per-x-group width
    let pad = slot * 0.15;
    let group_w = slot - 2.0 * pad;
    let bar_w = (group_w / n_s as f32).max(1.0);

    let palette = [
        Color32::from_rgb(70, 130, 200),
        Color32::from_rgb(210, 120, 70),
        Color32::from_rgb(90, 170, 100),
        Color32::from_rgb(180, 90, 170),
        Color32::from_rgb(200, 190, 80),
        Color32::from_rgb(110, 110, 200),
    ];
    let zero_y = y_of(0.0f64.clamp(y_lo, y_hi));

    for (si, s) in data.series.iter().enumerate() {
        let color = palette[si % palette.len()];
        // Bars.
        for (xi, y) in s.points.iter().enumerate() {
            let Some(v) = y else { continue }; // gap
            let x0 = plot.left() + xi as f32 * slot + pad + si as f32 * bar_w;
            let yv = y_of(*v);
            let bar = Rect::from_min_max(
                Pos2::new(x0, yv.min(zero_y)),
                Pos2::new(x0 + bar_w - 1.0, yv.max(zero_y)),
            );
            painter.rect_filled(bar, 0.0, color);
        }
        // Optional line overlay through the bar centers, skipping gaps.
        if line {
            let mut prev: Option<Pos2> = None;
            for (xi, y) in s.points.iter().enumerate() {
                match y {
                    Some(v) => {
                        let cx =
                            plot.left() + xi as f32 * slot + pad + si as f32 * bar_w + bar_w / 2.0;
                        let p = Pos2::new(cx, y_of(*v));
                        if let Some(pp) = prev {
                            painter.line_segment([pp, p], Stroke::new(1.5_f32, color));
                        }
                        painter.circle_filled(p, 2.0, color);
                        prev = Some(p);
                    }
                    None => prev = None,
                }
            }
        }
    }

    // x-labels centered under each group.
    for (xi, label) in data.x_labels.iter().enumerate() {
        let cx = plot.left() + (xi as f32 + 0.5) * slot;
        painter.text(
            Pos2::new(cx, plot.bottom() + 3.0),
            Align2::CENTER_TOP,
            label,
            font.clone(),
            text_color,
        );
    }

    // Axis title + a simple legend for multi-series charts.
    if !data.x_title.is_empty() {
        painter.text(
            Pos2::new(plot.center().x, rect.bottom() - 2.0),
            Align2::CENTER_BOTTOM,
            &data.x_title,
            font.clone(),
            visuals.weak_text_color(),
        );
    }
    if n_s > 1 {
        let mut lx = plot.left() + 4.0;
        let ly = plot.top() + 2.0;
        for (si, s) in data.series.iter().enumerate() {
            let color = palette[si % palette.len()];
            painter.rect_filled(
                Rect::from_min_size(Pos2::new(lx, ly), Vec2::new(10.0, 10.0)),
                0.0,
                color,
            );
            let galley = painter.layout_no_wrap(s.name.clone(), font.clone(), text_color);
            let w = galley.rect.width();
            painter.galley(Pos2::new(lx + 14.0, ly - 1.0), galley, text_color);
            lx += 14.0 + w + 12.0;
        }
    }
}
