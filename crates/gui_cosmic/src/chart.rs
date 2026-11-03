//! A chart of the shown measure: grouped bars, one group per grid row
//! (labels are the row's item names), one bar per column (a series), in
//! the COSMIC theme's accent and palette colors. The same reading as
//! `crates/gui`'s chart: rows along x, one series per column tuple, a cell
//! that is not a number is a gap. [`data`] is pure; the canvas only paints
//! it.

use cosmic::iced::mouse;
use cosmic::iced::widget::canvas::{self, Frame, Geometry, Path, Stroke, Text};
use cosmic::iced::{alignment, Color, Point, Rectangle, Size};
use cosmic::Theme;

use improv_core_model::{MeasureId, Model};
use improv_engine::session::Snapshot;

use crate::pivot::{self, Grid};
use crate::sheet;

/// At most this many groups and series are drawn; a bigger grid charts its
/// first ones (a chart of a million rows is not a chart).
pub const MAX_GROUPS: usize = 48;
pub const MAX_SERIES: usize = 8;

/// What a chart plots.
#[derive(Debug, Default, PartialEq)]
pub struct Data {
    /// One label per group (grid row).
    pub labels: Vec<String>,
    /// One series per grid column: its name and a value per group.
    pub series: Vec<(String, Vec<Option<f64>>)>,
    /// Whether the grid had more rows or columns than are charted.
    pub truncated: bool,
}

impl Data {
    /// The value range, 0 always in it; `(0, 1)` with no numbers.
    pub fn range(&self) -> (f64, f64) {
        let (mut lo, mut hi) = (0.0f64, 0.0f64);
        for y in self.series.iter().flat_map(|(_, p)| p.iter().flatten()) {
            lo = lo.min(*y);
            hi = hi.max(*y);
        }
        if lo == hi {
            (lo, hi + 1.0)
        } else {
            (lo, hi)
        }
    }
}

/// The chart of `measure` laid out as `grid`.
pub fn data(model: &Model, snapshot: &Snapshot, measure: MeasureId, grid: &Grid) -> Data {
    let names = |line: Vec<improv_core_model::ItemId>| {
        line.into_iter()
            .map(|i| pivot::item_name(model, i))
            .collect::<Vec<_>>()
            .join(" / ")
    };
    let rows = grid.rows.len().min(MAX_GROUPS);
    let cols = grid.cols.len().min(MAX_SERIES);
    let labels = (0..rows)
        .filter_map(|r| grid.rows.line(r).map(names))
        .collect();
    let series = (0..cols)
        .filter_map(|c| {
            let name = names(grid.cols.line(c)?);
            let points = (0..rows)
                .map(|r| {
                    let k = grid.key(r, c)?;
                    sheet::value(model, snapshot, measure, &k)?.as_num()
                })
                .collect();
            Some((name, points))
        })
        .collect();
    Data {
        labels,
        series,
        truncated: grid.rows.len() > rows || grid.cols.len() > cols,
    }
}

/// The colors of the series: the accent first, then the palette's accent
/// hues, in the theme's order.
pub fn colors(theme: &Theme) -> Vec<Color> {
    let c = theme.cosmic();
    let p = &c.palette;
    let mut out = vec![Color::from(c.accent_color())];
    for x in [
        p.accent_orange,
        p.accent_green,
        p.accent_purple,
        p.accent_yellow,
        p.accent_pink,
        p.accent_red,
        p.accent_indigo,
        p.accent_blue,
    ] {
        let x = Color::from(x);
        if out.iter().all(|o| *o != x) {
            out.push(x);
        }
    }
    out
}

/// [`Data`] as a canvas program.
pub struct Chart<'a> {
    pub data: &'a Data,
}

impl<Message> canvas::Program<Message, Theme> for Chart<'_> {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &cosmic::Renderer,
        theme: &Theme,
        bounds: Rectangle,
        _: mouse::Cursor,
    ) -> Vec<Geometry> {
        let c = theme.cosmic();
        let text = Color::from(c.on_bg_color());
        let rule = Color::from(c.bg_divider());
        let colors = colors(theme);
        let mut f = Frame::new(renderer, bounds.size());
        let d = self.data;
        let (lo, hi) = d.range();
        let (left, bottom, top, legend) = (56.0, 28.0, 8.0, 22.0);
        let plot_w = (bounds.width - left - 8.0).max(1.0);
        let plot_h = (bounds.height - bottom - top - legend).max(1.0);
        let y_of = |v: f64| top + legend + plot_h * (1.0 - ((v - lo) / (hi - lo)) as f32);
        let label =
            |f: &mut Frame, s: String, at: Point, x: cosmic::iced::advanced::text::Alignment| {
                f.fill_text(Text {
                    content: s,
                    position: at,
                    color: text,
                    size: 12.0.into(),
                    align_x: x,
                    align_y: alignment::Vertical::Center,
                    ..Text::default()
                });
            };
        use cosmic::iced::advanced::text::Alignment as A;
        // Axis: zero line and the range's ends.
        let zero = y_of(0.0);
        f.stroke(
            &Path::line(Point::new(left, zero), Point::new(left + plot_w, zero)),
            Stroke::default().with_color(rule).with_width(1.0),
        );
        for v in [lo, hi] {
            label(
                &mut f,
                improv_engine::display_number(v),
                Point::new(left - 6.0, y_of(v)),
                A::Right,
            );
        }
        // Legend.
        let mut x = left;
        for (i, (name, _)) in d.series.iter().enumerate() {
            let col = colors[i % colors.len()];
            f.fill_rectangle(Point::new(x, top + 4.0), Size::new(10.0, 10.0), col);
            label(
                &mut f,
                name.clone(),
                Point::new(x + 14.0, top + 9.0),
                A::Left,
            );
            x += 24.0 + 7.0 * name.chars().count() as f32;
        }
        // Bars.
        let groups = d.labels.len().max(1);
        let group_w = plot_w / groups as f32;
        let n = d.series.len().max(1);
        let bar_w = (group_w * 0.8 / n as f32).max(1.0);
        for (g, name) in d.labels.iter().enumerate() {
            let gx = left + g as f32 * group_w + group_w * 0.1;
            for (s, (_, points)) in d.series.iter().enumerate() {
                let Some(v) = points.get(g).copied().flatten() else {
                    continue;
                };
                let (y0, y1) = (zero, y_of(v));
                let top_y = y0.min(y1);
                let h = (y0 - y1).abs().max(1.0);
                f.fill_rectangle(
                    Point::new(gx + s as f32 * bar_w, top_y),
                    Size::new(bar_w - 1.0, h),
                    colors[s % colors.len()],
                );
            }
            label(
                &mut f,
                name.clone(),
                Point::new(gx + group_w * 0.4, top + legend + plot_h + bottom / 2.0),
                A::Center,
            );
        }
        vec![f.into_geometry()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pivot::Layout;
    use crate::service::tests::revenue_model;
    use improv_engine::CellValue;

    #[test]
    fn rows_are_groups_and_columns_are_series() {
        let m = revenue_model();
        let l = Layout::natural(&m, MeasureId(100)).unwrap();
        let d = data(&m, &Snapshot::default(), MeasureId(100), &Grid::new(&m, &l));
        assert_eq!(d.labels, vec!["2025", "2026"]);
        assert_eq!(d.series.len(), 2);
        assert_eq!(
            d.series[0],
            ("WidgetA".to_string(), vec![Some(100.0), Some(120.0)])
        );
        assert!(!d.truncated);
        assert_eq!(d.range(), (0.0, 120.0));
    }

    #[test]
    fn derived_values_chart_and_a_missing_one_is_a_gap() {
        let m = revenue_model();
        let l = Layout::natural(&m, MeasureId(102)).unwrap();
        let k: improv_engine::CoordKey = [(1, 10), (2, 20)].into_iter().collect();
        let only = std::collections::HashMap::from([(k, CellValue::num(-5.0))]);
        let snap: Snapshot = std::sync::Arc::new(std::collections::HashMap::from([(
            MeasureId(102),
            std::sync::Arc::new(only),
        )]));
        let d = data(&m, &snap, MeasureId(102), &Grid::new(&m, &l));
        assert_eq!(d.series[0].1, vec![Some(-5.0), None]);
        assert_eq!(d.range(), (-5.0, 0.0));
        assert_eq!(Data::default().range(), (0.0, 1.0));
    }

    #[test]
    fn a_big_grid_charts_its_first_rows_and_says_so() {
        let mut m = revenue_model();
        for i in 0..100 {
            m.add_item(
                improv_core_model::ItemId(1000 + i),
                improv_core_model::CategoryId(1),
                format!("t{i}"),
            );
        }
        let l = Layout::natural(&m, MeasureId(100)).unwrap();
        let d = data(&m, &Snapshot::default(), MeasureId(100), &Grid::new(&m, &l));
        assert_eq!(d.labels.len(), MAX_GROUPS);
        assert!(d.truncated);
    }

    #[test]
    fn series_take_the_accent_then_distinct_palette_hues() {
        let t = cosmic::Theme::dark();
        let c = colors(&t);
        assert_eq!(c[0], Color::from(t.cosmic().accent_color()));
        for (i, a) in c.iter().enumerate() {
            assert!(c[i + 1..].iter().all(|b| b != a), "distinct");
        }
        assert!(c.len() >= MAX_SERIES);
    }
}
