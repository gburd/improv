//! App state and the pure pivot-grid logic (terminal-free, unit-tested).
//!
//! A measure is a tensor over N categories. We render the first two categories
//! as the row/column axes; any remaining categories are "pages" pinned to their
//! first item (v1: fixed, shown on a status line). Values come from the model's
//! input cells (input measures) or from `engine::evaluate` (derived measures),
//! both keyed by `CoordKey = Vec<(u32, u32)>`.

use improv_core_model::{CategoryId, ItemId, MeasureId, Model, Value};
use improv_engine::dataflow::evaluate;
use improv_engine::{encode_coord, CoordKey};
use std::collections::HashMap;

/// One rendered pivot grid for a single measure.
pub struct Grid {
    pub measure: MeasureId,
    pub measure_name: String,
    /// Category on the rows axis (None if the measure has 0 categories).
    pub row_cat: Option<(CategoryId, String)>,
    /// Category on the columns axis (None if the measure has < 2 categories).
    pub col_cat: Option<(CategoryId, String)>,
    /// Row header items: (id, name). Length >= 1 (a synthetic single row when
    /// there is no row category).
    pub rows: Vec<(ItemId, String)>,
    /// Column header items: (id, name). Length >= 1.
    pub cols: Vec<(ItemId, String)>,
    /// Categories beyond the first two, pinned to a chosen item ("pages").
    pub pages: Vec<PageDim>,
    /// Cell values indexed `[row][col]`. `None` = empty cell.
    pub cells: Vec<Vec<Option<f64>>>,
}

/// A fixed extra dimension shown on the status line.
pub struct PageDim {
    pub cat: CategoryId,
    pub cat_name: String,
    pub item_name: String,
    pub item: ItemId,
}

impl Grid {
    pub fn n_rows(&self) -> usize {
        self.rows.len()
    }
    pub fn n_cols(&self) -> usize {
        self.cols.len()
    }
    pub fn value_at(&self, row: usize, col: usize) -> Option<f64> {
        self.cells
            .get(row)
            .and_then(|r| r.get(col))
            .copied()
            .flatten()
    }
}

/// Item names for a category, in the category's declared item order.
fn items_of(model: &Model, cat: CategoryId) -> Vec<(ItemId, String)> {
    model
        .categories
        .get(&cat)
        .map(|c| {
            c.items
                .iter()
                .map(|id| {
                    let name = model
                        .items
                        .get(id)
                        .map(|it| it.name.0.clone())
                        .unwrap_or_else(|| id.0.to_string());
                    (*id, name)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The value map for a measure: `CoordKey -> f64`.
///
/// Input measures read straight from `model.inputs`; derived measures are
/// computed by the engine. Engine failure yields an empty map (grid renders
/// blank) rather than crashing the UI.
fn values_for(model: &Model, measure: MeasureId) -> HashMap<CoordKey, f64> {
    let is_derived = model.measures.get(&measure).map(|m| m.is_derived());
    match is_derived {
        Some(true) => evaluate(model, &[measure])
            .ok()
            .and_then(|mut out| out.remove(&measure))
            .unwrap_or_default(),
        _ => model
            .inputs
            .iter()
            .filter(|((mid, _), _)| *mid == measure)
            .filter_map(|((_, coord), v)| match v {
                Value::Number(n) => Some((encode_coord(coord), *n)),
                _ => None,
            })
            .collect(),
    }
}

/// Build the pivot grid for `measure`. Uses the measure's first two categories
/// as the row/column axes and pins the rest to their first item.
pub fn build_grid(model: &Model, measure: MeasureId) -> Grid {
    let m = model.measures.get(&measure);
    let measure_name = m
        .map(|m| m.name.0.clone())
        .unwrap_or_else(|| measure.0.to_string());
    let cats: Vec<CategoryId> = m.map(|m| m.categories.clone()).unwrap_or_default();

    let cat_name = |c: CategoryId| {
        model
            .categories
            .get(&c)
            .map(|x| x.name.0.clone())
            .unwrap_or_else(|| c.0.to_string())
    };

    let row_cat = cats.first().map(|c| (*c, cat_name(*c)));
    let col_cat = cats.get(1).map(|c| (*c, cat_name(*c)));

    // Row / column header items (synthetic single entry when the axis is absent).
    let rows = match row_cat {
        Some((c, _)) => items_of(model, c),
        None => vec![(ItemId(0), String::new())],
    };
    let cols = match col_cat {
        Some((c, _)) => items_of(model, c),
        None => vec![(ItemId(0), String::new())],
    };

    // Extra dims pinned to their first item.
    let mut pages = Vec::new();
    for c in cats.iter().skip(2) {
        let its = items_of(model, *c);
        if let Some((id, name)) = its.into_iter().next() {
            pages.push(PageDim {
                cat: *c,
                cat_name: cat_name(*c),
                item_name: name,
                item: id,
            });
        }
    }

    let values = values_for(model, measure);

    // Fill cells. Build each cell's coordinate key from the row/col items plus
    // the pinned page items, then look it up.
    let mut cells = Vec::with_capacity(rows.len());
    for (row_item, _) in &rows {
        let mut row_cells = Vec::with_capacity(cols.len());
        for (col_item, _) in &cols {
            let mut key: CoordKey = Vec::new();
            if let Some((c, _)) = &row_cat {
                key.push((c.0, row_item.0));
            }
            if let Some((c, _)) = &col_cat {
                key.push((c.0, col_item.0));
            }
            for p in &pages {
                key.push((p.cat.0, p.item.0));
            }
            key.sort();
            row_cells.push(values.get(&key).copied());
        }
        cells.push(row_cells);
    }

    Grid {
        measure,
        measure_name,
        row_cat,
        col_cat,
        rows,
        cols,
        pages,
        cells,
    }
}

/// Measures sorted for stable cycling: derived first, then by id.
pub fn measure_order(model: &Model) -> Vec<MeasureId> {
    let mut ids: Vec<MeasureId> = model.measures.keys().copied().collect();
    ids.sort_by_key(|id| {
        let derived = model
            .measures
            .get(id)
            .map(|m| m.is_derived())
            .unwrap_or(false);
        (!derived, id.0) // derived (false sorts first) then ascending id
    });
    ids
}

/// The whole application state.
pub struct App {
    pub model: Model,
    pub measures: Vec<MeasureId>,
    pub selected: usize, // index into `measures`
    pub grid: Grid,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub should_quit: bool,
}

impl App {
    pub fn new(model: Model) -> Result<App, String> {
        let measures = measure_order(&model);
        if measures.is_empty() {
            return Err("model has no measures to display".into());
        }
        let selected = 0;
        let grid = build_grid(&model, measures[selected]);
        Ok(App {
            model,
            measures,
            selected,
            grid,
            cursor_row: 0,
            cursor_col: 0,
            should_quit: false,
        })
    }

    fn reselect(&mut self) {
        self.grid = build_grid(&self.model, self.measures[self.selected]);
        self.cursor_row = self.cursor_row.min(self.grid.n_rows().saturating_sub(1));
        self.cursor_col = self.cursor_col.min(self.grid.n_cols().saturating_sub(1));
    }

    /// Cycle to the next measure (wraps).
    pub fn next_measure(&mut self) {
        self.selected = (self.selected + 1) % self.measures.len();
        self.reselect();
    }

    /// Cursor movement, clamped to the grid (never goes out of range).
    pub fn move_cursor(&mut self, drow: isize, dcol: isize) {
        let max_row = self.grid.n_rows().saturating_sub(1) as isize;
        let max_col = self.grid.n_cols().saturating_sub(1) as isize;
        self.cursor_row = (self.cursor_row as isize + drow).clamp(0, max_row) as usize;
        self.cursor_col = (self.cursor_col as isize + dcol).clamp(0, max_col) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::Coordinate;
    use improv_core_model::{
        BinaryOp, DimensionSpec, Expr, Formula, Measure, MeasureKind, Name, ValueType,
    };

    // Canonical Time x Product revenue model (mirrors engine's fixture).
    fn revenue_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(11), time, "2026");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(21), product, "WidgetB");

        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });

        let coord = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
        m.set_input(
            MeasureId(100),
            coord(&[(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m.set_input(
            MeasureId(100),
            coord(&[(product, ItemId(21))]),
            Value::Number(20.0),
        );
        for (t, p, q) in [
            (ItemId(10), ItemId(20), 100.0),
            (ItemId(10), ItemId(21), 50.0),
            (ItemId(11), ItemId(20), 120.0),
            (ItemId(11), ItemId(21), 80.0),
        ] {
            m.set_input(
                MeasureId(101),
                coord(&[(time, t), (product, p)]),
                Value::Number(q),
            );
        }
        m
    }

    #[test]
    fn derived_grid_has_right_dims_and_values() {
        let model = revenue_model();
        let grid = build_grid(&model, MeasureId(102)); // Revenue[Time, Product]
        assert_eq!(grid.n_rows(), 2, "two Time items");
        assert_eq!(grid.n_cols(), 2, "two Product items");
        assert_eq!(grid.row_cat.as_ref().unwrap().1, "Time");
        assert_eq!(grid.col_cat.as_ref().unwrap().1, "Product");

        // Rows follow declared item order: 2025, 2026. Cols: WidgetA, WidgetB.
        // Revenue[2025, WidgetA] = 10 * 100 = 1000.
        assert_eq!(grid.value_at(0, 0), Some(1000.0));
        // Revenue[2026, WidgetB] = 20 * 80 = 1600.
        assert_eq!(grid.value_at(1, 1), Some(1600.0));
    }

    #[test]
    fn input_grid_single_axis() {
        let model = revenue_model();
        let grid = build_grid(&model, MeasureId(100)); // Price[Product]
        assert_eq!(
            grid.n_rows(),
            2,
            "two Product items on the single (row) axis"
        );
        assert_eq!(
            grid.n_cols(),
            1,
            "no column axis => single synthetic column"
        );
        assert!(grid.col_cat.is_none());
        assert_eq!(grid.value_at(0, 0), Some(10.0)); // WidgetA
        assert_eq!(grid.value_at(1, 0), Some(20.0)); // WidgetB
    }

    #[test]
    fn measure_cycling_wraps_and_derived_first() {
        let model = revenue_model();
        let mut app = App::new(model).unwrap();
        // Derived (Revenue, 102) sorts first.
        assert_eq!(app.grid.measure, MeasureId(102));
        app.next_measure();
        assert_eq!(app.grid.measure, MeasureId(100)); // then Price (id 100)
        app.next_measure();
        assert_eq!(app.grid.measure, MeasureId(101)); // then Quantity (id 101)
        app.next_measure();
        assert_eq!(app.grid.measure, MeasureId(102)); // wraps back
    }

    #[test]
    fn cursor_stays_in_bounds() {
        let model = revenue_model();
        let mut app = App::new(model).unwrap();
        // Revenue grid is 2x2. Push cursor past every edge.
        app.move_cursor(-5, -5);
        assert_eq!((app.cursor_row, app.cursor_col), (0, 0));
        app.move_cursor(100, 100);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));

        // Switching to Price (2 rows, 1 col) must re-clamp the column.
        app.selected = app
            .measures
            .iter()
            .position(|m| *m == MeasureId(100))
            .unwrap();
        app.reselect();
        assert_eq!(app.cursor_col, 0, "column re-clamped to the narrower grid");
        assert!(app.cursor_row <= 1);
    }
}
