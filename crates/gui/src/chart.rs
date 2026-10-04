//! A read-only chart view of the FOCUSED matrix's measure.
//!
//! `chart_series` is a pure function of the app state (snapshot + the focused
//! matrix's filters + its pivot/page); an axis filtered to nothing (or a page
//! category with no pinnable item) charts nothing at all, exactly as the grid
//! renders nothing. With several matrices on the canvas the chart follows the
//! keyboard, as every other single-measure surface does — see
//! `ImprovApp::focused`.
//!
//! The x-axis is the full Cartesian product of the ROW categories
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
    /// Build the chart data for the FOCUSED matrix's measure from the current
    /// snapshot, honoring THAT matrix's active filters and pinned page items
    /// (same visible-item set as its grid). The x-axis is the full Cartesian
    /// product of the ROW categories (x-label = joined tuple item names, e.g.
    /// "2024Q1 / North"); one series per full COLUMN tuple (series name = joined
    /// column tuple names). The single-category case is just the 1-length-tuple
    /// case. A 1-D grid (no column categories) is a single unnamed series.
    /// Non-numeric / missing cells are gaps (`None`). Pure: no egui, no mutation.
    ///
    /// Empty [`ChartData`] (no x, no series) whenever nothing is addressable:
    /// no measure on the focused matrix, an axis category filtered to zero
    /// items, or a page category with no pinnable item. `render_chart` reports
    /// that as "no data" — the chart never plots an under-specified key. Matches
    /// `Matrix::grid_dims`.
    pub fn chart_series(&self) -> ChartData {
        let Some(measure) = self.selected() else {
            return ChartData::default();
        };
        // No pinnable page item -> every key would omit that category. The grid
        // renders nothing there; so does the chart.
        let Some((row_cats, col_cats, pinned)) = self.chart_axes_pub() else {
            return ChartData::default();
        };
        let values = self.values_for_pub(measure);

        // Full row/column products (each element is a tuple of (ItemId, name)).
        // No categories on an axis yields one empty tuple (scalar in that
        // direction); a category filtered to nothing yields NO tuples and stays
        // that way — a synthesized line's key would omit that category.
        let row_tuples = self.axis_tuples_pub(&row_cats);
        let col_tuples = self.axis_tuples_pub(&col_cats);
        if row_tuples.is_empty() || col_tuples.is_empty() {
            return ChartData::default();
        }

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
    use crate::theme::{BLACK, DARK_GRAY, LIGHT_GRAY, WHITE};
    use egui::{Align2, FontId, Pos2, Rect, Sense, Stroke, Vec2};

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
    let font = FontId::proportional(12.0);

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

    // The four NeXT grays, each bar, point and line outlined in black so a
    // light-gray series stays visible on the light-gray panel.
    // ponytail: four shades then repeats; add hatch patterns if charts with
    // more than four series matter.
    let palette = [BLACK, WHITE, DARK_GRAY, LIGHT_GRAY];
    let outline = Stroke::new(1.0_f32, BLACK);
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
            painter.rect(bar, 0.0, color, outline);
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
                            painter.line_segment([pp, p], Stroke::new(3.5_f32, BLACK));
                            painter.line_segment([pp, p], Stroke::new(1.5_f32, color));
                        }
                        painter.circle(p, 2.5, color, outline);
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
            painter.rect(
                Rect::from_min_size(Pos2::new(lx, ly), Vec2::new(10.0, 10.0)),
                0.0,
                color,
                outline,
            );
            let galley = painter.layout_no_wrap(s.name.clone(), font.clone(), text_color);
            let w = galley.rect.width();
            painter.galley(Pos2::new(lx + 14.0, ly - 1.0), galley, text_color);
            lx += 14.0 + w + 12.0;
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use egui::epaint::Shape;

    /// Paint `data` in one headless 600x400 frame; return every shape drawn.
    fn paint(data: &ChartData, line: bool) -> Vec<Shape> {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600.0, 400.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(raw, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| render_chart(ui, data, line));
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

    /// A chart bar or legend swatch: a rect outlined 1px black (the panel's
    /// own background rect is not).
    fn is_bar(s: &Shape) -> bool {
        use crate::theme::BLACK;
        matches!(s, Shape::Rect(r) if r.stroke == egui::Stroke::new(1.0_f32, BLACK))
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

    #[test]
    fn empty_chart_says_no_data_and_draws_no_bars() {
        for data in [
            ChartData::default(),
            ChartData {
                x_labels: vec!["a".into()],
                ..Default::default()
            },
        ] {
            let shapes = paint(&data, true);
            assert!(texts(&shapes).iter().any(|t| t.contains("no data")));
            assert_eq!(count(&shapes, is_bar), 0);
        }
    }

    #[test]
    fn two_series_with_gaps_and_negatives_draw_bars_lines_zero_and_legend() {
        let data = ChartData {
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
        };
        let bars_lines = paint(&data, true);
        let bars_only = paint(&data, false);
        let rects = |s: &[Shape]| count(s, is_bar);
        let circles = |s: &[Shape]| count(s, |s| matches!(s, Shape::Circle(_)));
        let segs = |s: &[Shape]| count(s, |s| matches!(s, Shape::LineSegment { .. }));
        // 5 numeric points -> 5 bars, + 2 legend swatches (n_s > 1).
        assert_eq!(rects(&bars_only), 7);
        assert_eq!(rects(&bars_lines), 7);
        assert_eq!(circles(&bars_only), 0);
        assert_eq!(circles(&bars_lines), 5);
        // Line overlay: North joins q3-q4 only (q2 gap breaks it), South q1-q2;
        // each join is a black under-stroke plus a colored stroke.
        assert_eq!(segs(&bars_lines) - segs(&bars_only), 4);
        // 2 axes + 3 gridlines + the zero line (range straddles 0).
        assert_eq!(segs(&bars_only), 6);
        let t = texts(&bars_only);
        for want in ["Time", "North", "South", "q1", "q4"] {
            assert!(t.iter().any(|x| x == want), "{want} missing from {t:?}");
        }
    }

    #[test]
    fn one_positive_untitled_series_has_no_zero_line_or_legend() {
        let data = ChartData {
            x_title: String::new(),
            x_labels: vec!["a".into(), "b".into()],
            series: vec![Series {
                name: "only".into(),
                points: vec![Some(1.0), Some(2.0)],
            }],
        };
        let shapes = paint(&data, false);
        assert_eq!(count(&shapes, is_bar), 2);
        assert_eq!(
            count(&shapes, |s| matches!(s, Shape::LineSegment { .. })),
            5
        );
        assert!(!texts(&shapes).iter().any(|t| t == "only"));
    }

    #[test]
    fn no_measure_charts_nothing_and_all_negative_data_has_no_zero_line() {
        let app = ImprovApp::load("").expect("in-memory model");
        assert_eq!(app.chart_series(), ChartData::default());
        let data = ChartData {
            x_title: String::new(),
            x_labels: vec!["a".into(), "b".into()],
            series: vec![Series {
                name: String::new(),
                points: vec![Some(-1.0), Some(-2.0)],
            }],
        };
        assert_eq!(data.y_range(), (-2.0, 0.0));
        let shapes = paint(&data, false);
        assert_eq!(count(&shapes, is_bar), 2);
        // Axes + 3 gridlines; 0 is the top of the range, so no zero line.
        assert_eq!(
            count(&shapes, |s| matches!(s, Shape::LineSegment { .. })),
            5
        );
    }
}
