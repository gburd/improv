//! App state and the pure pivot-grid logic (terminal-free, unit-tested).
//!
//! A measure is a tensor over N categories. We render the first two categories
//! as the row/column axes; any remaining categories are "pages" pinned to their
//! first item (v1: fixed, shown on a status line). Values come from the model's
//! input cells (input measures) or from `engine::evaluate` (derived measures),
//! both keyed by `CoordKey = Vec<(u32, u32)>`.

#[cfg(test)]
use improv_core_model::Coordinate;
use improv_core_model::{CategoryId, Filter, ItemId, MeasureId, Model, Name, Value, View, ViewId};
use improv_engine::session::{Engine, MeasureValues};
use improv_engine::{decode_coord, encode_coord, CoordKey};
use std::collections::HashMap;

/// Live derived-measure values, keyed by measure then coordinate. Kept in
/// `App` and refreshed incrementally by the `Engine` on each committed edit;
/// used to render derived measures instead of a fresh full `evaluate()`.
pub type Snapshot = HashMap<MeasureId, MeasureValues>;

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

/// An extra dimension not on the row/column axes, pinned to one item (a
/// "page"). `item_index`/`item_count` let the UI page through the dimension's
/// items with `[` / `]`.
pub struct PageDim {
    pub cat: CategoryId,
    pub cat_name: String,
    pub item_name: String,
    pub item: ItemId,
    pub item_index: usize,
    pub item_count: usize,
}

/// Geometry of the last-rendered grid, recorded by `ui::render` so mouse
/// clicks (screen col/row) can be mapped back to a grid cell.
#[derive(Clone, Copy, Debug)]
pub struct GridGeom {
    /// Screen coords of the top-left DATA cell (row 0, col 0).
    pub x0: u16,
    pub y0: u16,
    /// Width of each data column and the inter-column spacing.
    pub col_w: u16,
    pub spacing: u16,
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
    /// The sorted `CoordKey` for the cell at `[row][col]`.
    pub fn coord_key(&self, row: usize, col: usize) -> CoordKey {
        cell_key(
            &self.row_cat,
            &self.col_cat,
            &self.rows,
            &self.cols,
            &self.pages,
            row,
            col,
        )
    }
}

/// Item names for a category, in the category's declared item order, keeping
/// only items that pass `filters` (a category absent from `filters` is
/// unfiltered). Presentation-only: it hides items, it never touches data.
fn items_of(model: &Model, cat: CategoryId, filters: &[Filter]) -> Vec<(ItemId, String)> {
    let keep = |id: ItemId| match filters.iter().find(|f| f.category == cat) {
        Some(f) => f.items.contains(&id),
        None => true,
    };
    model
        .categories
        .get(&cat)
        .map(|c| {
            c.items
                .iter()
                .filter(|id| keep(**id))
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
/// Input measures read straight from `model.inputs`; derived measures come from
/// the live engine `snapshot` (empty map -> grid renders blank).
fn values_for(model: &Model, measure: MeasureId, snapshot: &Snapshot) -> HashMap<CoordKey, f64> {
    let is_derived = model.measures.get(&measure).map(|m| m.is_derived());
    match is_derived {
        // Derived: read the live engine snapshot, projecting each CellValue to a
        // number (non-numeric derived cells show as their number/NaN for now).
        // ponytail: numeric grid only; a typed cell renderer is future TUI work.
        Some(true) => snapshot
            .get(&measure)
            .map(|m| {
                m.iter()
                    .filter_map(|(k, v)| v.as_num().map(|n| (k.clone(), n)))
                    .collect()
            })
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
pub fn build_grid(model: &Model, measure: MeasureId, snapshot: &Snapshot) -> Grid {
    build_grid_paged(model, measure, snapshot, &[])
}

/// Like [`build_grid`] but pins each extra (page) dimension to the item at the
/// given index (clamped). `page_idx[i]` selects the item for the i-th extra
/// dimension (categories beyond the first two); a missing/short entry defaults
/// to 0. Enables paging through a 3+ dimensional measure.
pub fn build_grid_paged(
    model: &Model,
    measure: MeasureId,
    snapshot: &Snapshot,
    page_idx: &[usize],
) -> Grid {
    build_grid_pivoted(model, measure, snapshot, page_idx, None, &[])
}

/// The most general grid builder. `axis_order`, when `Some`, is a permutation
/// of the measure's categories that sets which category is on rows (first),
/// columns (second), and pages (rest) — this is how the UI *pivots* without
/// touching formulas. `None` uses the measure's natural category order.
/// `filters` restricts which items of a category are shown (presentation only;
/// unlisted categories show all items).
pub fn build_grid_pivoted(
    model: &Model,
    measure: MeasureId,
    snapshot: &Snapshot,
    page_idx: &[usize],
    axis_order: Option<&[CategoryId]>,
    filters: &[Filter],
) -> Grid {
    let m = model.measures.get(&measure);
    let measure_name = m
        .map(|m| m.name.0.clone())
        .unwrap_or_else(|| measure.0.to_string());
    let natural: Vec<CategoryId> = m.map(|m| m.categories.clone()).unwrap_or_default();
    // Use the requested axis order if it is a valid permutation of the measure's
    // categories; otherwise fall back to natural order (defensive).
    let cats: Vec<CategoryId> = match axis_order {
        Some(order)
            if order.len() == natural.len() && natural.iter().all(|c| order.contains(c)) =>
        {
            order.to_vec()
        }
        _ => natural,
    };

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
        Some((c, _)) => items_of(model, c, filters),
        None => vec![(ItemId(0), String::new())],
    };
    let cols = match col_cat {
        Some((c, _)) => items_of(model, c, filters),
        None => vec![(ItemId(0), String::new())],
    };

    // Extra dims pinned to the selected page item (default first).
    let mut pages = Vec::new();
    for (pi, c) in cats.iter().skip(2).enumerate() {
        let its = items_of(model, *c, filters);
        if its.is_empty() {
            continue;
        }
        let sel = page_idx.get(pi).copied().unwrap_or(0).min(its.len() - 1);
        let (id, name) = its[sel].clone();
        pages.push(PageDim {
            cat: *c,
            cat_name: cat_name(*c),
            item_name: name,
            item: id,
            item_index: sel,
            item_count: its.len(),
        });
    }

    let values = values_for(model, measure, snapshot);

    // Fill cells by looking up each cell's coordinate key.
    let mut cells = Vec::with_capacity(rows.len());
    for r in 0..rows.len() {
        let mut row_cells = Vec::with_capacity(cols.len());
        for c in 0..cols.len() {
            let key = cell_key(&row_cat, &col_cat, &rows, &cols, &pages, r, c);
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

/// The sorted `CoordKey` for the cell at `[row][col]` given the axes/pages.
/// Shared by grid fill and cursor->coordinate mapping so they can't drift.
#[allow(clippy::too_many_arguments)]
fn cell_key(
    row_cat: &Option<(CategoryId, String)>,
    col_cat: &Option<(CategoryId, String)>,
    rows: &[(ItemId, String)],
    cols: &[(ItemId, String)],
    pages: &[PageDim],
    row: usize,
    col: usize,
) -> CoordKey {
    let mut key: CoordKey = Vec::new();
    if let (Some((c, _)), Some((item, _))) = (row_cat, rows.get(row)) {
        key.push((c.0, item.0));
    }
    if let (Some((c, _)), Some((item, _))) = (col_cat, cols.get(col)) {
        key.push((c.0, item.0));
    }
    for p in pages {
        key.push((p.cat.0, p.item.0));
    }
    key.sort();
    key
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
///
/// The live `Engine` tracks **all** derived measures in the model up front
/// (built once in `new`), so switching the viewed measure never rebuilds it and
/// there is no per-measure rebuild logic. Editing an input cell pushes a delta
/// through the engine and the returned snapshot is kept here to render derived
/// measures. A model with zero derived measures gets no engine (`None`); input
/// edits still work, there is just nothing to recompute.
pub struct App {
    pub model: Model,
    pub measures: Vec<MeasureId>,
    pub selected: usize, // index into `measures`
    pub grid: Grid,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub should_quit: bool,
    /// Live incremental engine over all derived measures (None if there are none).
    pub engine: Option<Engine>,
    /// Latest derived-measure values from the engine.
    pub snapshot: Snapshot,
    /// The last saved view applied via `cycle_view`, so cycling advances.
    pub applied_view: Option<ViewId>,
    /// When `Some`, edit mode is active and this holds the in-progress text.
    pub edit: Option<String>,
    /// Transient status/error message (e.g. "derived cells are computed").
    pub status: Option<String>,
    /// Selected item index for each extra (page) dimension of the viewed
    /// measure; `[` / `]` cycle the first one. Reset when the measure changes.
    pub page_idx: Vec<usize>,
    /// Current axis order (a permutation of the viewed measure's categories):
    /// element 0 is on rows, 1 on columns, the rest are pages. Pivoting mutates
    /// this; it resets to the measure's natural order on measure switch.
    pub axis_order: Vec<CategoryId>,
    /// Active per-category display filters for the current layout (which items
    /// of a category are shown). Presentation only — never changes model data.
    /// Captured when saving a view; reset on measure switch.
    pub filters: Vec<Filter>,
    /// Geometry of the last render, for mapping mouse clicks to cells.
    pub grid_geom: Option<GridGeom>,
}

impl App {
    pub fn new(model: Model) -> Result<App, String> {
        let measures = measure_order(&model);
        if measures.is_empty() {
            return Err("model has no measures to display".into());
        }

        // Track every derived measure in one engine, built once.
        let derived: Vec<MeasureId> = measures
            .iter()
            .copied()
            .filter(|id| {
                model
                    .measures
                    .get(id)
                    .map(|m| m.is_derived())
                    .unwrap_or(false)
            })
            .collect();
        let (engine, snapshot) = if derived.is_empty() {
            (None, Snapshot::new())
        } else {
            let (e, s) = Engine::new(&model, &derived).map_err(|e| e.to_string())?;
            (Some(e), s)
        };

        let selected = 0;
        let natural_axes = |mid: MeasureId| -> Vec<CategoryId> {
            model
                .measures
                .get(&mid)
                .map(|m| m.categories.clone())
                .unwrap_or_default()
        };
        let axis_order = natural_axes(measures[selected]);
        let grid = build_grid(&model, measures[selected], &snapshot);
        let page_idx = vec![0; grid.pages.len()];
        Ok(App {
            model,
            measures,
            selected,
            grid,
            cursor_row: 0,
            cursor_col: 0,
            should_quit: false,
            engine,
            snapshot,
            applied_view: None,
            edit: None,
            status: None,
            page_idx,
            axis_order,
            filters: Vec::new(),
            grid_geom: None,
        })
    }

    fn reselect(&mut self) {
        self.grid = build_grid_pivoted(
            &self.model,
            self.measures[self.selected],
            &self.snapshot,
            &self.page_idx,
            Some(&self.axis_order),
            &self.filters,
        );
        self.cursor_row = self.cursor_row.min(self.grid.n_rows().saturating_sub(1));
        self.cursor_col = self.cursor_col.min(self.grid.n_cols().saturating_sub(1));
    }

    /// The viewed measure's categories in natural (declared) order.
    fn natural_axis_order(&self) -> Vec<CategoryId> {
        self.model
            .measures
            .get(&self.measures[self.selected])
            .map(|m| m.categories.clone())
            .unwrap_or_default()
    }

    /// Pivot: rotate the axis order left, so rows->pages, cols->rows, first
    /// page->cols. A single discoverable key repeatedly cycles which category
    /// sits on each axis — the Improv/Quantrix "move a category to another
    /// axis" gesture, without touching formulas. No-op for < 2 categories.
    pub fn pivot(&mut self) {
        if self.axis_order.len() < 2 {
            self.status = Some("nothing to pivot (measure has < 2 dimensions)".into());
            return;
        }
        self.axis_order.rotate_left(1);
        // Axis roles changed; reset page selection to first items.
        self.page_idx.clear();
        self.reselect();
        self.page_idx = self.grid.pages.iter().map(|p| p.item_index).collect();
        let names: Vec<String> = self
            .axis_order
            .iter()
            .map(|c| {
                self.model
                    .categories
                    .get(c)
                    .map(|x| x.name.0.clone())
                    .unwrap_or_else(|| c.0.to_string())
            })
            .collect();
        self.status = Some(format!(
            "pivot: rows={} cols={}",
            names[0],
            names.get(1).cloned().unwrap_or_default()
        ));
    }

    /// Cycle the first page (extra) dimension's selected item by `delta`
    /// (wrapping). No-op if the viewed measure has no extra dimensions.
    pub fn page(&mut self, delta: isize) {
        let Some(pd) = self.grid.pages.first() else {
            self.status = Some("no extra dimensions to page".into());
            return;
        };
        let count = pd.item_count.max(1);
        // Ensure page_idx has an entry per current page dim.
        if self.page_idx.len() != self.grid.pages.len() {
            self.page_idx = self.grid.pages.iter().map(|p| p.item_index).collect();
        }
        let cur = self.page_idx.first().copied().unwrap_or(0);
        let next = (cur as isize + delta).rem_euclid(count as isize) as usize;
        if let Some(slot) = self.page_idx.first_mut() {
            *slot = next;
        }
        self.reselect();
    }

    /// Cycle to the next measure (wraps).
    pub fn next_measure(&mut self) {
        self.selected = (self.selected + 1) % self.measures.len();
        // A new measure has its own dimensions; reset axis order, paging, filters.
        self.axis_order = self.natural_axis_order();
        self.page_idx.clear();
        self.filters.clear();
        self.reselect();
        // Size page_idx to the new grid so `[`/`]` work immediately.
        self.page_idx = self.grid.pages.iter().map(|p| p.item_index).collect();
    }

    /// Cursor movement, clamped to the grid (never goes out of range).
    pub fn move_cursor(&mut self, drow: isize, dcol: isize) {
        let max_row = self.grid.n_rows().saturating_sub(1) as isize;
        let max_col = self.grid.n_cols().saturating_sub(1) as isize;
        self.cursor_row = (self.cursor_row as isize + drow).clamp(0, max_row) as usize;
        self.cursor_col = (self.cursor_col as isize + dcol).clamp(0, max_col) as usize;
    }

    /// Handle a left-click at screen `(col, row)`: if it lands on a data cell,
    /// move the cursor there. Uses the geometry recorded by the last render.
    /// Out-of-grid clicks are ignored. (Enables mouse-driven navigation; the
    /// terminal must support mouse reporting, which `improv-tui` enables.)
    pub fn click_at(&mut self, col: u16, row: u16) {
        let Some(g) = self.grid_geom else { return };
        if row < g.y0 || col < g.x0 {
            return;
        }
        // Which data row: each grid row is one terminal line from y0.
        let r = (row - g.y0) as usize;
        // Which data column: columns are col_w + spacing wide, starting at x0.
        let stride = (g.col_w + g.spacing) as usize;
        let c = ((col - g.x0) as usize) / stride.max(1);
        if r < self.grid.n_rows() && c < self.grid.n_cols() {
            self.cursor_row = r;
            self.cursor_col = c;
        }
    }

    /// True if the currently viewed measure is an input measure (editable).
    pub fn viewed_is_input(&self) -> bool {
        self.model
            .measures
            .get(&self.grid.measure)
            .map(|m| m.is_input())
            .unwrap_or(false)
    }

    /// Enter edit mode on the cell under the cursor. Rejects derived measures
    /// with a status message (they are computed, not editable).
    pub fn begin_edit(&mut self) {
        if !self.viewed_is_input() {
            self.status = Some("derived cells are computed, not editable".into());
            return;
        }
        // Seed the buffer with the current value so editing is a tweak, not a
        // retype. Blank cell -> empty buffer.
        let seed = match self.grid.value_at(self.cursor_row, self.cursor_col) {
            Some(n) if n.fract() == 0.0 => format!("{n:.0}"),
            Some(n) => format!("{n}"),
            None => String::new(),
        };
        self.edit = Some(seed);
        self.status = None;
    }

    /// Cancel edit mode without committing.
    pub fn cancel_edit(&mut self) {
        self.edit = None;
        self.status = None;
    }

    /// Commit the in-progress edit buffer. Parses the number, applies it to the
    /// model + engine, refreshes the grid, and exits edit mode. On a parse
    /// error the buffer stays open with a status message.
    pub fn commit_edit(&mut self) {
        let Some(buf) = self.edit.clone() else {
            return;
        };
        let text = buf.trim();
        let value: f64 = match text.parse() {
            Ok(v) => v,
            Err(_) => {
                self.status = Some(format!("not a number: '{text}'"));
                return;
            }
        };
        match self.apply_edit(self.cursor_row, self.cursor_col, value) {
            Ok(()) => {
                self.edit = None;
                self.status = None;
            }
            Err(e) => self.status = Some(e),
        }
    }

    /// Pure edit-apply: set input `[row][col]` to `value`, updating both the
    /// in-memory model and the live engine, then rebuild the grid from the new
    /// snapshot. Returns `Err` (no state change committed) if the viewed
    /// measure is derived. Terminal-free -> unit-testable.
    pub fn apply_edit(&mut self, row: usize, col: usize, value: f64) -> Result<(), String> {
        if !self.viewed_is_input() {
            return Err("derived cells are computed, not editable".into());
        }
        let measure = self.grid.measure;
        let key = self.grid.coord_key(row, col);
        let coord = decode_coord(&key);

        self.model.set_input(measure, coord, Value::Number(value));

        // Push the delta through the engine (if any derived measures exist) and
        // keep the recomputed snapshot for rendering.
        if let Some(engine) = &mut self.engine {
            self.snapshot = engine.set(measure, key, value).map_err(|e| e.to_string())?;
        }
        self.reselect();
        Ok(())
    }

    // -- views & filters (pure; terminal-free -> unit-testable) ------------

    /// The viewed measure's id.
    fn viewed_measure(&self) -> MeasureId {
        self.measures[self.selected]
    }

    /// Build a `View` capturing the current layout: viewed measure, axis order,
    /// pinned page items, and active filters. Presentation only — no data.
    pub fn build_view(&self, id: ViewId, name: &str) -> View {
        let page_items = self.grid.pages.iter().map(|p| (p.cat, p.item)).collect();
        View {
            id,
            name: Name(name.to_string()),
            measure: self.viewed_measure(),
            axis_order: self.axis_order.clone(),
            page_items,
            filters: self.filters.clone(),
        }
    }

    /// The smallest unused view id (>= 1).
    fn next_view_id(&self) -> ViewId {
        ViewId(
            self.model
                .views
                .keys()
                .map(|v| v.0)
                .max()
                .map(|m| m + 1)
                .unwrap_or(1),
        )
    }

    /// Save the current layout as a named view: mint an id, add it to the
    /// model, and (when `path` is set) persist immediately. Returns the id.
    pub fn save_view(&mut self, name: &str, path: Option<&str>) -> ViewId {
        let id = self.next_view_id();
        let view = self.build_view(id, name);
        self.model.add_view(view);
        if let Some(path) = path {
            match improv_storage_mentat::ModelStore::open(path)
                .and_then(|mut s| s.save_model(&self.model))
            {
                Ok(()) => self.status = Some(format!("saved view '{name}'")),
                Err(e) => self.status = Some(format!("save failed: {e}")),
            }
        } else {
            self.status = Some(format!("saved view '{name}'"));
        }
        id
    }

    /// Apply a saved `View` to the live layout: select its measure, restore the
    /// axis order, page items, and filters, then re-render. Presentation only
    /// — measures and data are untouched. No-op if the measure is gone.
    pub fn apply_view(&mut self, view: &View) {
        let Some(idx) = self.measures.iter().position(|m| *m == view.measure) else {
            self.status = Some("view's measure no longer exists".into());
            return;
        };
        self.selected = idx;
        self.axis_order = if view.axis_order.is_empty() {
            self.natural_axis_order()
        } else {
            view.axis_order.clone()
        };
        self.filters = view.filters.clone();
        self.reselect();
        // Restore pinned page items positionally by page dimension.
        self.page_idx = self
            .grid
            .pages
            .iter()
            .map(|p| {
                view.page_items
                    .iter()
                    .find(|(c, _)| *c == p.cat)
                    .and_then(|(_, it)| {
                        items_of(&self.model, p.cat, &self.filters)
                            .iter()
                            .position(|(id, _)| id == it)
                    })
                    .unwrap_or(p.item_index)
            })
            .collect();
        self.reselect();
    }

    /// Cycle to the next saved view (by id order) and apply it. No-op if there
    /// are no saved views.
    pub fn cycle_view(&mut self) {
        let mut ids: Vec<ViewId> = self.model.views.keys().copied().collect();
        if ids.is_empty() {
            self.status = Some("no saved views (press S to save one)".into());
            return;
        }
        ids.sort_by_key(|v| v.0);
        // Advance past the last-applied view if it is still present.
        let start = self
            .applied_view
            .and_then(|cur| ids.iter().position(|v| *v == cur))
            .map(|p| (p + 1) % ids.len())
            .unwrap_or(0);
        let id = ids[start];
        self.applied_view = Some(id);
        let view = self.model.views[&id].clone();
        self.apply_view(&view);
        self.status = Some(format!("view: {}", view.name.0));
    }

    /// Toggle whether `item` of `category` is shown. Building the filter from
    /// the full item set on first toggle, then removing/re-adding the item.
    /// Removing the last shown item leaves an explicit empty filter (nothing
    /// shown for that category). Presentation only.
    pub fn toggle_filter_item(&mut self, category: CategoryId, item: ItemId) {
        let all: Vec<ItemId> = self
            .model
            .categories
            .get(&category)
            .map(|c| c.items.clone())
            .unwrap_or_default();
        let pos = self.filters.iter().position(|f| f.category == category);
        match pos {
            None => {
                // First toggle: start from all items, then hide this one.
                let items: Vec<ItemId> = all.into_iter().filter(|i| *i != item).collect();
                self.filters.push(Filter { category, items });
            }
            Some(i) => {
                let f = &mut self.filters[i];
                if let Some(p) = f.items.iter().position(|x| *x == item) {
                    f.items.remove(p);
                } else {
                    f.items.push(item);
                }
                // If the filter now keeps everything, drop it (unfiltered).
                if f.items.len() == all.len() && all.iter().all(|i| f.items.contains(i)) {
                    self.filters.remove(i);
                }
            }
        }
        self.reselect();
    }

    /// Clear all active filters, showing every item again.
    pub fn clear_filters(&mut self) {
        self.filters.clear();
        self.reselect();
    }
    /// The `(measure, Coordinate)` of the cell under the cursor.
    #[cfg(test)]
    pub fn cursor_coord(&self) -> (MeasureId, Coordinate) {
        let key = self.grid.coord_key(self.cursor_row, self.cursor_col);
        (self.grid.measure, decode_coord(&key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let app = App::new(model).unwrap();
        let grid = build_grid(&app.model, MeasureId(102), &app.snapshot); // Revenue[Time, Product]
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
        let grid = build_grid(&model, MeasureId(100), &Snapshot::new()); // Price[Product]
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

    // Position the app on a specific measure by id.
    fn view(app: &mut App, measure: MeasureId) {
        app.selected = app.measures.iter().position(|m| *m == measure).unwrap();
        app.reselect();
    }

    #[test]
    fn editing_input_recomputes_derived_via_engine() {
        let mut app = App::new(revenue_model()).unwrap();
        // View Quantity[Time, Product]; rows=Time(2025,2026), cols=Product(WidgetA,WidgetB).
        view(&mut app, MeasureId(101));
        // Cell [0,0] = Quantity[2025, WidgetA]; confirm the mapping first.
        let (m, coord) = {
            app.cursor_row = 0;
            app.cursor_col = 0;
            app.cursor_coord()
        };
        assert_eq!(m, MeasureId(101));
        assert_eq!(coord.get(CategoryId(1)), Some(ItemId(10))); // Time=2025
        assert_eq!(coord.get(CategoryId(2)), Some(ItemId(20))); // Product=WidgetA

        // Set Quantity[2025, WidgetA] = 200 (was 100).
        app.apply_edit(0, 0, 200.0).unwrap();
        assert_eq!(
            app.model.input(MeasureId(101), &coord),
            Some(&Value::Number(200.0)),
            "model input updated"
        );

        // Revenue[2025, WidgetA] = Price(10) * 200 = 2000 in the live snapshot.
        let rev_key = {
            let mut k = vec![(1u32, 10u32), (2, 20)];
            k.sort();
            k
        };
        assert_eq!(
            app.snapshot
                .get(&MeasureId(102))
                .unwrap()
                .get(&rev_key)
                .and_then(|v| v.as_num()),
            Some(2000.0),
            "derived Revenue recomputed incrementally"
        );
    }

    #[test]
    fn editing_derived_cell_is_rejected() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(102)); // Revenue is derived
        let before = app.model.clone();
        let err = app.apply_edit(0, 0, 42.0);
        assert!(err.is_err(), "derived edit must be rejected");
        assert_eq!(app.model, before, "model unchanged on rejected edit");

        // begin_edit path sets a status and does not enter edit mode.
        app.begin_edit();
        assert!(app.edit.is_none());
        assert_eq!(
            app.status.as_deref(),
            Some("derived cells are computed, not editable")
        );
    }

    #[test]
    fn cursor_maps_to_expected_coordinate() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // Quantity[Time, Product], 2x2

        // [1,1] = Quantity[2026, WidgetB] = Time(11), Product(21).
        app.cursor_row = 1;
        app.cursor_col = 1;
        let key = app.grid.coord_key(1, 1);
        let mut expect = vec![(1u32, 11u32), (2, 21)];
        expect.sort();
        assert_eq!(key, expect, "cursor cell -> CoordKey");

        // Round-trip: key -> Coordinate -> back matches cursor_coord.
        let (_m, coord) = app.cursor_coord();
        assert_eq!(improv_engine::encode_coord(&coord), key);
        assert_eq!(coord.get(CategoryId(1)), Some(ItemId(11)));
        assert_eq!(coord.get(CategoryId(2)), Some(ItemId(21)));
    }

    #[test]
    fn paging_changes_the_pinned_extra_dimension() {
        // A 3-D input measure Sales[Time, Product, Region]; Time/Product are the
        // grid axes, Region is a page dimension navigated with [ / ].
        let mut m = Model::new();
        let (time, product, region) = (CategoryId(1), CategoryId(2), CategoryId(3));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_category(region, "Region");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(30), region, "North");
        m.add_item(ItemId(31), region, "South");
        m.add_measure(Measure {
            id: MeasureId(200),
            name: Name("Sales".into()),
            value_type: ValueType::Number,
            categories: vec![time, product, region],
            kind: MeasureKind::Input,
            description: None,
        });
        let c = |pairs: &[(CategoryId, ItemId)]| Coordinate::from_pairs(pairs.iter().copied());
        m.set_input(
            MeasureId(200),
            c(&[
                (time, ItemId(10)),
                (product, ItemId(20)),
                (region, ItemId(30)),
            ]),
            Value::Number(100.0),
        );
        m.set_input(
            MeasureId(200),
            c(&[
                (time, ItemId(10)),
                (product, ItemId(20)),
                (region, ItemId(31)),
            ]),
            Value::Number(250.0),
        );

        let mut app = App::new(m).unwrap();
        // One page dimension (Region), starting on North (index 0).
        assert_eq!(app.grid.pages.len(), 1);
        assert_eq!(app.grid.pages[0].item_name, "North");
        assert_eq!(app.grid.pages[0].item_count, 2);
        assert_eq!(app.grid.value_at(0, 0), Some(100.0)); // North cell

        // Page to the next Region: South, and the visible cell changes.
        app.page(1);
        assert_eq!(app.grid.pages[0].item_name, "South");
        assert_eq!(app.grid.value_at(0, 0), Some(250.0));

        // Wraps back to North.
        app.page(1);
        assert_eq!(app.grid.pages[0].item_name, "North");
        assert_eq!(app.grid.value_at(0, 0), Some(100.0));
    }

    #[test]
    fn pivot_swaps_axes() {
        // Revenue[Time, Product]: rows=Time, cols=Product initially.
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // Quantity[Time, Product] input, 2x2
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Time");
        assert_eq!(app.grid.col_cat.as_ref().unwrap().1, "Product");

        // Pivot: rotate axes -> rows=Product, cols=Time.
        app.pivot();
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Product");
        assert_eq!(app.grid.col_cat.as_ref().unwrap().1, "Time");

        // Pivot again on a 2-D measure returns to the original orientation.
        app.pivot();
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Time");
        assert_eq!(app.grid.col_cat.as_ref().unwrap().1, "Product");
    }

    #[test]
    fn click_moves_cursor_to_cell() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // 2x2
                                        // Simulate the geometry a render would record.
        app.grid_geom = Some(GridGeom {
            x0: 2,
            y0: 5,
            col_w: 12,
            spacing: 1,
        });
        // Click in data row 1, column 1: x within the 2nd column (stride 13).
        app.click_at(2 + 13, 5 + 1);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
        // A click above the grid (in the status area) is ignored.
        app.click_at(2, 0);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
    }

    #[test]
    fn filter_hides_a_row_item_from_the_grid() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // Quantity[Time, Product]: rows=Time(2025,2026)
        assert_eq!(app.grid.n_rows(), 2);
        // Hide 2026 (ItemId 11) from the Time row axis.
        app.toggle_filter_item(CategoryId(1), ItemId(11));
        let rows: Vec<ItemId> = app.grid.rows.iter().map(|(id, _)| *id).collect();
        assert_eq!(rows, vec![ItemId(10)], "2026 filtered out of rows");
        // Columns (unfiltered Product) still show both.
        assert_eq!(app.grid.n_cols(), 2);
        // Toggling it back restores both rows (filter dropped when it keeps all).
        app.toggle_filter_item(CategoryId(1), ItemId(11));
        assert_eq!(app.grid.n_rows(), 2);
        assert!(app.filters.is_empty(), "unfiltered again");
    }

    #[test]
    fn save_view_captures_axis_order_and_filters() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.pivot(); // rows=Product, cols=Time
        app.toggle_filter_item(CategoryId(2), ItemId(21)); // hide WidgetB

        let id = app.save_view("L1", None);
        let v = app.model.views.get(&id).expect("view saved");
        assert_eq!(v.measure, MeasureId(101));
        assert_eq!(v.axis_order, vec![CategoryId(2), CategoryId(1)]);
        let f = v
            .filters
            .iter()
            .find(|f| f.category == CategoryId(2))
            .unwrap();
        assert_eq!(f.items, vec![ItemId(20)]); // only WidgetA kept
        assert_eq!(app.model.view_by_name("L1").map(|v| v.id), Some(id));
    }

    #[test]
    fn apply_view_restores_axis_order_and_filters() {
        // Build a view from one app, apply it to a fresh app -> layout matches.
        let mut src = App::new(revenue_model()).unwrap();
        view(&mut src, MeasureId(101));
        src.pivot(); // rows=Product, cols=Time
        src.toggle_filter_item(CategoryId(2), ItemId(21)); // hide WidgetB
        let v = src.build_view(ViewId(1), "L1");

        let mut dst = App::new(revenue_model()).unwrap();
        assert_ne!(dst.grid.measure, MeasureId(101)); // starts on derived Revenue
        dst.apply_view(&v);

        assert_eq!(dst.grid.measure, MeasureId(101));
        assert_eq!(dst.axis_order, vec![CategoryId(2), CategoryId(1)]);
        assert_eq!(dst.filters, v.filters);
        // The filter is reflected in the grid: WidgetB gone from the row axis.
        let rows: Vec<ItemId> = dst.grid.rows.iter().map(|(id, _)| *id).collect();
        assert_eq!(rows, vec![ItemId(20)], "only WidgetA on rows");
        assert_eq!(dst.grid.row_cat.as_ref().unwrap().1, "Product");
        assert_eq!(dst.grid.col_cat.as_ref().unwrap().1, "Time");
    }

    #[test]
    fn apply_view_restores_page_item() {
        // 3-D Sales[Time, Product, Region]: save on South, apply to a fresh app.
        let mut m = Model::new();
        let (time, product, region) = (CategoryId(1), CategoryId(2), CategoryId(3));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_category(region, "Region");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(30), region, "North");
        m.add_item(ItemId(31), region, "South");
        m.add_measure(Measure {
            id: MeasureId(200),
            name: Name("Sales".into()),
            value_type: ValueType::Number,
            categories: vec![time, product, region],
            kind: MeasureKind::Input,
            description: None,
        });
        let c = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
        m.set_input(
            MeasureId(200),
            c(&[
                (time, ItemId(10)),
                (product, ItemId(20)),
                (region, ItemId(30)),
            ]),
            Value::Number(100.0),
        );
        m.set_input(
            MeasureId(200),
            c(&[
                (time, ItemId(10)),
                (product, ItemId(20)),
                (region, ItemId(31)),
            ]),
            Value::Number(250.0),
        );

        let mut src = App::new(m.clone()).unwrap();
        src.page(1); // page Region to South
        assert_eq!(src.grid.pages[0].item_name, "South");
        let v = src.build_view(ViewId(1), "south");
        assert_eq!(v.page_items, vec![(region, ItemId(31))]);

        let mut dst = App::new(m).unwrap();
        assert_eq!(dst.grid.pages[0].item_name, "North");
        dst.apply_view(&v);
        assert_eq!(dst.grid.pages[0].item_name, "South", "page item restored");
    }
}
