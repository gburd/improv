//! GUI application state and rendering.
//!
//! State: the loaded `Model`, a live `session::Engine` (built over all derived
//! measures), the current derived-measure snapshot, and the selected measure.
//! The panels (model explorer, pivot grid, formula editor, inspector) are all
//! rendered each frame from this state. The GUI is a pure *view/controller*
//! over `improv_engine` + the model; it adds no modeling semantics (see
//! `.agent/steering/AGENT_GUI_STEERING.md` §9.3).
//!
//! The engine is rebuilt (a structural rebuild) only when the measure structure
//! changes — a formula edit or a new derived measure. Plain cell-value edits go
//! through `engine.set` incrementally.

use std::collections::HashMap;

use improv_core_model::{
    parser, CategoryId, Filter, ItemId, Measure, MeasureId, MeasureKind, Model, Name, Value,
    ValueType, View, ViewId,
};
use improv_engine::session::{Engine, MeasureValues};
use improv_engine::{encode_coord, CellValue, CoordKey};
use improv_nl_formula::{describe_formula, NlContext};
use improv_storage_mentat::ModelStore;

/// The running GUI application.
pub struct ImprovApp {
    /// The store path (empty = in-memory scratch); used for saving.
    db: String,
    model: Model,
    /// Live incremental engine over all derived measures, plus its snapshot.
    engine: Option<Engine>,
    snapshot: HashMap<MeasureId, MeasureValues>,
    /// The measure currently shown in the grid.
    selected: Option<MeasureId>,
    status: String,

    // --- transient UI edit buffers (view state, not model state) ---
    /// The cell currently being edited in the grid, and its text buffer.
    editing: Option<(MeasureId, CoordKey)>,
    edit_buf: String,
    /// The formula-editor text for the selected derived measure.
    formula_buf: String,
    /// The measure whose formula `formula_buf` currently holds (so we reload
    /// the buffer when the selection changes).
    formula_for: Option<MeasureId>,
    /// New-derived-measure form: name + formula text.
    new_name: String,
    new_formula: String,

    // --- pivot state (mirrors the TUI's per-measure axis order + paging) ---
    /// A permutation of the selected measure's categories: index 0 -> rows,
    /// 1 -> columns, 2.. -> pages. Pivoting reorders this without touching
    /// formulas. Resets to the measure's natural order on measure switch.
    axis_order: Vec<CategoryId>,
    /// How many leading `axis_order` categories are stacked on the ROW axis,
    /// and how many (after those) on the COLUMN axis. The rest are pages.
    /// Default 1/1 (one category per axis); increasing them stacks categories
    /// on an axis (nested group headers over the Cartesian product of items).
    n_rows: usize,
    n_cols: usize,
    /// Selected item index for each page (extra) dimension, positionally by
    /// page dim (i.e. `axis_order[2 + i]`). Reset on measure switch.
    page_idx: Vec<usize>,
    /// The measure `axis_order`/`page_idx` currently describe (so we reset the
    /// pivot state when the selection changes).
    axis_for: Option<MeasureId>,
    /// Active per-category display filters for the current layout. Presentation
    /// only (hides items from the grid; never touches data). Captured when
    /// saving a view; reset on measure switch.
    filters: Vec<Filter>,
    /// Text buffer for the "Save view" name field.
    view_name: String,

    /// Keyboard cell cursor into the current grid (row/col indices), clamped to
    /// the grid's dimensions. Reset when the selected measure or pivot changes.
    cursor_row: usize,
    cursor_col: usize,

    /// Whether the read-only chart panel is shown, and its bar/line toggle.
    show_chart: bool,
    chart_line: bool,
}

/// Which grid axis a category is assigned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Rows,
    Columns,
    Pages,
}

impl ImprovApp {
    /// Load a model from the store at `db` (`""` = fresh in-memory model) and
    /// build the live engine over its derived measures.
    pub fn load(db: &str) -> Result<ImprovApp, String> {
        let model = if db.is_empty() {
            Model::new()
        } else {
            let mut store = ModelStore::open(db).map_err(|e| e.to_string())?;
            store.load_model().map_err(|e| e.to_string())?
        };

        let (engine, snapshot) = build_engine(&model);
        let selected = pick_default_measure(&model);
        let axis_order = natural_axis_order(&model, selected);

        Ok(ImprovApp {
            db: db.to_string(),
            model,
            engine,
            snapshot,
            selected,
            status: String::new(),
            editing: None,
            edit_buf: String::new(),
            formula_buf: String::new(),
            formula_for: None,
            new_name: String::new(),
            new_formula: String::new(),
            axis_order,
            n_rows: 1,
            n_cols: 1,
            page_idx: Vec::new(),
            axis_for: selected,
            filters: Vec::new(),
            view_name: String::new(),
            cursor_row: 0,
            cursor_col: 0,
            show_chart: false,
            chart_line: false,
        })
    }

    // -- read-only accessors for the chart module (crate-internal) ---------

    pub(crate) fn selected(&self) -> Option<MeasureId> {
        self.selected
    }
    /// Row/column category stacks and pinned pages for the current pivot — the
    /// general (stacked) form the chart needs. Row/col tuples come from
    /// `axis_tuples_pub`; keys from `cell_key_multi_pub`.
    pub(crate) fn chart_axes_pub(
        &self,
    ) -> (Vec<CategoryId>, Vec<CategoryId>, Vec<(CategoryId, ItemId)>) {
        (self.row_cats(), self.col_cats(), self.pinned_pages())
    }
    /// The Cartesian product of `cats`' filtered items (see `axis_tuples`),
    /// exposed for the chart. Empty `cats` -> one empty tuple.
    pub(crate) fn axis_tuples_pub(&self, cats: &[CategoryId]) -> Vec<Vec<(ItemId, String)>> {
        self.axis_tuples(cats)
    }
    /// The sorted `CoordKey` for a stacked cell (see `cell_key_multi`).
    pub(crate) fn cell_key_multi_pub(
        &self,
        row_cats: &[CategoryId],
        row_tuple: &[(ItemId, String)],
        col_cats: &[CategoryId],
        col_tuple: &[(ItemId, String)],
        pinned: &[(CategoryId, ItemId)],
    ) -> CoordKey {
        cell_key_multi(row_cats, row_tuple, col_cats, col_tuple, pinned)
    }
    pub(crate) fn values_for_pub(&self, measure: MeasureId) -> HashMap<CoordKey, f64> {
        self.values_for(measure)
    }
    pub(crate) fn category_name_pub(&self, c: CategoryId) -> Option<String> {
        self.model.categories.get(&c).map(|x| x.name.0.clone())
    }

    // -- pure state logic (unit-tested; no rendering) ----------------------

    /// The numeric value map for a measure (input cells, or the derived
    /// snapshot projected to numbers for the grid).
    fn values_for(&self, measure: MeasureId) -> HashMap<CoordKey, f64> {
        let is_derived = self.model.measures.get(&measure).map(|m| m.is_derived());
        match is_derived {
            Some(true) => self
                .snapshot
                .get(&measure)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_num().map(|n| (k.clone(), n)))
                        .collect()
                })
                .unwrap_or_default(),
            _ => self
                .model
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

    /// The display text for a derived cell (booleans as true/false, errors as
    /// `#ERR`) via `CellValue`'s `Display`.
    fn derived_cell_text(&self, measure: MeasureId, key: &CoordKey) -> Option<String> {
        self.snapshot
            .get(&measure)
            .and_then(|m| m.get(key))
            .map(|v| v.to_string())
    }

    /// Set an input cell and push the edit through the live engine, refreshing
    /// the snapshot. Returns an error string on failure (e.g. derived cell).
    pub fn set_cell(
        &mut self,
        measure: MeasureId,
        coord: CoordKey,
        value: f64,
    ) -> Result<(), String> {
        if self
            .model
            .measures
            .get(&measure)
            .map(|m| m.is_derived())
            .unwrap_or(false)
        {
            return Err("derived cells are computed, not editable".into());
        }
        self.model
            .set_input(measure, decode(&coord), Value::Number(value));
        if let Some(engine) = &mut self.engine {
            self.snapshot = engine
                .set(measure, coord, value)
                .map_err(|e| e.to_string())?;
        }
        self.save();
        Ok(())
    }

    // -- pivot / page state (pure; unit-tested without egui) ---------------

    /// Reset the pivot state to the selected measure's natural order when the
    /// selection has changed. Called each frame before rendering the grid.
    fn sync_axis_state(&mut self) {
        if self.axis_for != self.selected {
            self.axis_for = self.selected;
            self.axis_order = natural_axis_order(&self.model, self.selected);
            self.n_rows = 1.min(self.axis_order.len());
            self.n_cols = 1.min(self.axis_order.len().saturating_sub(self.n_rows));
            self.page_idx = vec![0; self.page_cats().len()];
            self.filters.clear();
            self.cursor_row = 0;
            self.cursor_col = 0;
        } else if self.page_idx.len() != self.page_cats().len() {
            // Keep page_idx sized to the current page-dimension count.
            let n = self.page_cats().len();
            self.page_idx.resize(n, 0);
        }
        self.clamp_cursor();
    }

    /// Resolved axes for the current pivot state: (row cat, col cat, pinned
    /// page dims as (category, item)). Mirrors the grid's cell keying. Page
    /// items are the selected index for each page dimension (clamped).
    /// Test-only: rendering and the chart use the stacked (`_cats`) form.
    #[cfg(test)]
    fn resolved_axes(
        &self,
    ) -> (
        Option<CategoryId>,
        Option<CategoryId>,
        Vec<(CategoryId, ItemId)>,
    ) {
        let row_cat = self.axis_order.first().copied();
        let col_cat = self.axis_order.get(self.n_rows).copied();
        (row_cat, col_cat, self.pinned_pages())
    }

    /// The categories stacked on the ROW axis (outer→inner), the COLUMN axis,
    /// and the remaining PAGE categories, derived from `axis_order` + the
    /// `n_rows`/`n_cols` split. Categories that fall off the current measure's
    /// dimension set are naturally absent from `axis_order`.
    fn row_cats(&self) -> Vec<CategoryId> {
        self.axis_order.iter().take(self.n_rows).copied().collect()
    }
    fn col_cats(&self) -> Vec<CategoryId> {
        self.axis_order
            .iter()
            .skip(self.n_rows)
            .take(self.n_cols)
            .copied()
            .collect()
    }
    fn page_cats(&self) -> Vec<CategoryId> {
        self.axis_order
            .iter()
            .skip(self.n_rows + self.n_cols)
            .copied()
            .collect()
    }

    /// The pinned (category, item) for each page dimension, from `page_idx`.
    fn pinned_pages(&self) -> Vec<(CategoryId, ItemId)> {
        let mut pinned = Vec::new();
        for (pi, c) in self.page_cats().iter().enumerate() {
            let its = self.sorted_items(*c);
            if its.is_empty() {
                continue;
            }
            let sel = self
                .page_idx
                .get(pi)
                .copied()
                .unwrap_or(0)
                .min(its.len() - 1);
            pinned.push((*c, its[sel].0));
        }
        pinned
    }

    /// The Cartesian product of `cats`' filtered items, outer category first.
    /// Each returned element is one axis line: a tuple of `(ItemId, name)` in
    /// `cats` order. An empty `cats` yields a single empty tuple (a 1-line
    /// axis, i.e. a scalar in that direction). Any empty category collapses the
    /// product to nothing (no lines).
    fn axis_tuples(&self, cats: &[CategoryId]) -> Vec<Vec<(ItemId, String)>> {
        let mut out: Vec<Vec<(ItemId, String)>> = vec![Vec::new()];
        for c in cats {
            let items = self.sorted_items(*c);
            if items.is_empty() {
                return Vec::new();
            }
            let mut next = Vec::with_capacity(out.len() * items.len());
            for prefix in &out {
                for it in &items {
                    let mut t = prefix.clone();
                    t.push(it.clone());
                    next.push(t);
                }
            }
            out = next;
        }
        out
    }

    /// The item lists (sorted, filtered) for each of `cats`, in order. Used to
    /// virtualize the row axis: with these lists we can compute the total row
    /// count as a product of lengths and decode the i-th row tuple on demand
    /// (`nth_tuple`) without materializing the whole Cartesian product.
    fn axis_item_lists(&self, cats: &[CategoryId]) -> Vec<Vec<(ItemId, String)>> {
        cats.iter().map(|c| self.sorted_items(*c)).collect()
    }

    /// A category's items sorted by id, honoring the active filters (shared by
    /// paging and grid rendering). A category without a filter shows all items;
    /// filtering is presentation only — it never touches model data.
    fn sorted_items(&self, c: CategoryId) -> Vec<(ItemId, String)> {
        let keep = |id: ItemId| match self.filters.iter().find(|f| f.category == c) {
            Some(f) => f.items.contains(&id),
            None => true,
        };
        let mut v: Vec<(ItemId, String)> = self
            .model
            .categories
            .get(&c)
            .map(|cat| {
                cat.items
                    .iter()
                    .filter(|id| keep(**id))
                    .filter_map(|id| self.model.items.get(id).map(|it| (*id, it.name.0.clone())))
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|(id, _)| id.0);
        v
    }

    /// Move `category` to `axis`, appending it as the innermost entry of that
    /// axis (so categories *stack*: dropping a second category on Rows nests it
    /// under the first). Removes it from its previous axis. No-op if the
    /// category is not among the current measure's dimensions. Pivoting is
    /// formula-free re-projection (the Improv/Quantrix signature move).
    pub fn set_axis(&mut self, category: CategoryId, axis: Axis) {
        if !self.axis_order.contains(&category) {
            return;
        }
        let (mut rows, mut cols, mut pages) = (self.row_cats(), self.col_cats(), self.page_cats());
        for v in [&mut rows, &mut cols, &mut pages] {
            v.retain(|c| *c != category);
        }
        match axis {
            Axis::Rows => rows.push(category),
            Axis::Columns => cols.push(category),
            Axis::Pages => pages.push(category),
        }
        self.rebuild_axis_order(rows, cols, pages);
        self.clamp_cursor();
    }

    /// Flatten the three axis groups back into `axis_order` + `n_rows`/`n_cols`,
    /// and resize `page_idx` to the new page count.
    fn rebuild_axis_order(
        &mut self,
        rows: Vec<CategoryId>,
        cols: Vec<CategoryId>,
        pages: Vec<CategoryId>,
    ) {
        self.n_rows = rows.len();
        self.n_cols = cols.len();
        self.axis_order = rows;
        self.axis_order.extend(cols);
        self.axis_order.extend(pages);
        self.page_idx = vec![0; self.page_cats().len()];
    }

    /// Pivot: swap the entire row stack with the entire column stack
    /// (Rows ↔ Columns), keeping pages put. For the classic one-per-axis case
    /// this is the familiar row/column swap; with stacked categories it swaps
    /// the two groups. No-op if there is nothing on either of rows/columns.
    pub fn pivot_rotate(&mut self) {
        let rows = self.row_cats();
        let cols = self.col_cats();
        if rows.is_empty() && cols.is_empty() {
            return;
        }
        let pages = self.page_cats();
        // Swap: old columns become rows, old rows become columns.
        self.rebuild_axis_order(cols, rows, pages);
        self.clamp_cursor();
    }

    /// Set the pinned item index for page dimension `dim_index` (position among
    /// the page dims, i.e. `axis_order[2 + dim_index]`), clamped to the
    /// dimension's item count. No-op if out of range.
    pub fn set_page(&mut self, dim_index: usize, item_index: usize) {
        let pages = self.page_cats();
        let Some(cat) = pages.get(dim_index).copied() else {
            return;
        };
        let count = self.sorted_items(cat).len();
        if count == 0 {
            return;
        }
        if self.page_idx.len() != pages.len() {
            self.page_idx.resize(pages.len(), 0);
        }
        if let Some(slot) = self.page_idx.get_mut(dim_index) {
            *slot = item_index.min(count - 1);
        }
    }

    // -- views & filters (pure; unit-tested without egui) ------------------

    /// Build a `View` capturing the current layout: selected measure, axis
    /// order, pinned page items, and active filters. Presentation only.
    pub fn build_view(&self, id: ViewId, name: &str) -> Option<View> {
        let measure = self.selected?;
        Some(View {
            id,
            name: Name(name.to_string()),
            measure,
            axis_order: self.axis_order.clone(),
            n_rows: self.n_rows,
            n_cols: self.n_cols,
            page_items: self.pinned_pages(),
            filters: self.filters.clone(),
        })
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
    /// model, and autosave. Returns the id (None if no measure is selected).
    pub fn save_view(&mut self, name: &str) -> Option<ViewId> {
        let name = name.trim();
        if name.is_empty() {
            self.status = "view name is required".into();
            return None;
        }
        let id = self.next_view_id();
        let view = self.build_view(id, name)?;
        self.model.add_view(view);
        self.save();
        self.status = format!("saved view '{name}'");
        Some(id)
    }

    /// Apply a saved `View` to the live layout: select its measure, restore the
    /// axis order, page items, and filters, then re-render. Presentation only
    /// — measures and data untouched. No-op if the measure is gone.
    pub fn apply_view(&mut self, view: &View) {
        if !self.model.measures.contains_key(&view.measure) {
            self.status = "view's measure no longer exists".into();
            return;
        }
        self.selected = Some(view.measure);
        // Pin selection so sync_axis_state does not reset what we set below.
        self.axis_for = Some(view.measure);
        self.axis_order = if view.axis_order.is_empty() {
            natural_axis_order(&self.model, self.selected)
        } else {
            view.axis_order.clone()
        };
        // Restore the axis split, clamped to the actual axis_order length.
        let len = self.axis_order.len();
        self.n_rows = view.n_rows.min(len);
        self.n_cols = view.n_cols.min(len.saturating_sub(self.n_rows));
        self.filters = view.filters.clone();
        // Restore page selections positionally by page dimension.
        let page_cats = self.page_cats();
        self.page_idx = vec![0; page_cats.len()];
        for (pi, cat) in page_cats.iter().enumerate() {
            if let Some((_, it)) = view.page_items.iter().find(|(c, _)| c == cat) {
                if let Some(idx) = self.sorted_items(*cat).iter().position(|(id, _)| id == it) {
                    if let Some(slot) = self.page_idx.get_mut(pi) {
                        *slot = idx;
                    }
                }
            }
        }
        self.editing = None;
        self.clamp_cursor();
        self.status = format!("view: {}", view.name.0);
    }

    /// Toggle whether `item` of `category` is shown. On first toggle the filter
    /// starts from all items minus this one; toggling so all items are kept
    /// drops the filter. Presentation only.
    pub fn toggle_filter_item(&mut self, category: CategoryId, item: ItemId) {
        let all: Vec<ItemId> = self
            .model
            .categories
            .get(&category)
            .map(|c| c.items.clone())
            .unwrap_or_default();
        match self.filters.iter().position(|f| f.category == category) {
            None => {
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
                if f.items.len() == all.len() && all.iter().all(|x| f.items.contains(x)) {
                    self.filters.remove(i);
                }
            }
        }
        self.clamp_cursor();
    }

    /// Clear all active filters, showing every item again.
    pub fn clear_filters(&mut self) {
        self.filters.clear();
        self.clamp_cursor();
    }

    /// Rebuild the live engine after a structural change (formula edit / new
    /// derived measure) and refresh the snapshot.
    fn rebuild_engine(&mut self) {
        let (engine, snapshot) = build_engine(&self.model);
        self.engine = engine;
        self.snapshot = snapshot;
    }

    // -- keyboard cell cursor (pure; unit-tested without egui) -------------

    /// The current grid's (row_count, col_count) for the selected measure and
    /// pivot. Both are >= 1 (a missing axis renders one synthetic row/column,
    /// matching `render_grid`).
    fn grid_dims(&self) -> (usize, usize) {
        let rows = product_len(&self.axis_item_lists(&self.row_cats())).max(1);
        let cols = product_len(&self.axis_item_lists(&self.col_cats())).max(1);
        (rows, cols)
    }

    /// Move the cursor by `(drow, dcol)`, clamped to the current grid (never
    /// out of range). Mirrors the TUI's `move_cursor`.
    pub fn move_cursor(&mut self, drow: isize, dcol: isize) {
        let (rows, cols) = self.grid_dims();
        let max_row = rows.saturating_sub(1) as isize;
        let max_col = cols.saturating_sub(1) as isize;
        self.cursor_row = (self.cursor_row as isize + drow).clamp(0, max_row) as usize;
        self.cursor_col = (self.cursor_col as isize + dcol).clamp(0, max_col) as usize;
    }

    /// Clamp the cursor into the current grid (called after a pivot / measure
    /// switch that may have shrunk it).
    fn clamp_cursor(&mut self) {
        let (rows, cols) = self.grid_dims();
        self.cursor_row = self.cursor_row.min(rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(cols.saturating_sub(1));
    }

    /// The `CoordKey` of the cell under the cursor, given the current pivot.
    pub fn cursor_key(&self) -> CoordKey {
        let row_cats = self.row_cats();
        let col_cats = self.col_cats();
        let row_lists = self.axis_item_lists(&row_cats);
        let col_lists = self.axis_item_lists(&col_cats);
        // Decode only the cursor's row/col line (never the whole product).
        let row_tuple = if row_lists.is_empty() || product_len(&row_lists) == 0 {
            Vec::new()
        } else {
            nth_tuple(&row_lists, self.cursor_row.min(product_len(&row_lists) - 1))
        };
        let col_tuple = if col_lists.is_empty() || product_len(&col_lists) == 0 {
            Vec::new()
        } else {
            nth_tuple(&col_lists, self.cursor_col.min(product_len(&col_lists) - 1))
        };
        cell_key_multi(
            &row_cats,
            &row_tuple,
            &col_cats,
            &col_tuple,
            &self.pinned_pages(),
        )
    }

    /// True if the cursor cell is an editable input cell (i.e. the selected
    /// measure is an input measure). Derived measures are read-only.
    pub fn cursor_is_editable(&self) -> bool {
        self.selected
            .and_then(|m| self.model.measures.get(&m))
            .map(|m| !m.is_derived())
            .unwrap_or(false)
    }

    /// Begin editing the cursor cell if it is editable, seeding the buffer with
    /// the current value. On a derived cell, sets the status message instead.
    /// Mirrors the TUI's `begin_edit`.
    fn begin_edit_cursor(&mut self) {
        let Some(measure) = self.selected else {
            return;
        };
        if !self.cursor_is_editable() {
            self.status = "derived cells are computed, not editable".into();
            return;
        }
        let key = self.cursor_key();
        let seed = self
            .values_for(measure)
            .get(&key)
            .map(|v| format!("{v}"))
            .unwrap_or_default();
        self.editing = Some((measure, key));
        self.edit_buf = seed;
    }

    /// Parse `text` as the RHS expression for an existing measure and make it
    /// derived (replacing any prior formula/input kind). Rebuilds the engine
    /// (structure changed), refreshes the snapshot, and autosaves. On parse
    /// error the model is left unchanged and the error is returned.
    pub fn commit_formula(&mut self, measure: MeasureId, text: &str) -> Result<(), String> {
        let formula = parser::parse_expr(&self.model, text).map_err(|e| e.to_string())?;
        let m = self
            .model
            .measures
            .get_mut(&measure)
            .ok_or_else(|| format!("no measure with id {}", measure.0))?;
        m.kind = MeasureKind::Derived(formula);
        self.rebuild_engine();
        self.save();
        Ok(())
    }

    /// Create a new derived measure named `name` with RHS `text`. Categories
    /// are inferred as the union of the referenced measures' categories (same
    /// rule as the CLI's `add-derived`). Rebuilds the engine and autosaves. On
    /// parse error (or a duplicate name) the model is unchanged.
    pub fn add_derived_measure(&mut self, name: &str, text: &str) -> Result<MeasureId, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("measure name is required".into());
        }
        if self.model.measure_by_name(name).is_some() {
            return Err(format!("a measure named {name:?} already exists"));
        }
        let formula = parser::parse_expr(&self.model, text).map_err(|e| e.to_string())?;

        // Infer categories: union of referenced measures' categories.
        let mut cats: Vec<CategoryId> = Vec::new();
        for m in formula.referenced_measures() {
            if let Some(measure) = self.model.measures.get(&m) {
                for c in &measure.categories {
                    if !cats.contains(c) {
                        cats.push(*c);
                    }
                }
            }
        }
        cats.sort_by_key(|c| c.0);

        let id = MeasureId(self.next_measure_id());
        self.model.add_measure(Measure {
            id,
            name: Name(name.to_string()),
            value_type: ValueType::Number,
            categories: cats,
            kind: MeasureKind::Derived(formula),
            description: None,
        });
        self.rebuild_engine();
        self.save();
        Ok(id)
    }

    /// The smallest unused measure id (>= 1).
    fn next_measure_id(&self) -> u32 {
        self.model
            .measures
            .keys()
            .map(|m| m.0)
            .max()
            .map(|m| m + 1)
            .unwrap_or(1)
    }

    /// Autosave the model to the store when `db` is set. In-memory (`""`)
    /// models skip saving. Save failures land in the status line, never panic.
    fn save(&mut self) {
        if self.db.is_empty() {
            return;
        }
        match ModelStore::open(&self.db).and_then(|mut s| s.save_model(&self.model).map(|_| ())) {
            Ok(()) => {}
            Err(e) => self.status = format!("save failed: {e}"),
        }
    }

    /// Read-only inspector facts for `measure` (see `inspector` panel).
    fn inspector_data(&self, measure: MeasureId) -> Option<InspectorData> {
        let m = self.model.measures.get(&measure)?;
        let dimensions = m
            .categories
            .iter()
            .map(|c| {
                self.model
                    .categories
                    .get(c)
                    .map(|cat| cat.name.0.clone())
                    .unwrap_or_else(|| format!("category {}", c.0))
            })
            .collect();
        let (dependencies, formula_english) = match &m.kind {
            MeasureKind::Derived(f) => {
                let mut deps: Vec<String> = f
                    .referenced_measures()
                    .into_iter()
                    .map(|id| {
                        self.model
                            .measures
                            .get(&id)
                            .map(|dm| dm.name.0.clone())
                            .unwrap_or_else(|| format!("measure {}", id.0))
                    })
                    .collect();
                deps.dedup();
                (
                    deps,
                    Some(describe_formula(&NlContext::new(&self.model), f)),
                )
            }
            MeasureKind::Input => (Vec::new(), None),
        };
        let error_cells = self
            .snapshot
            .get(&measure)
            .map(|m| {
                m.values()
                    .filter(|v| matches!(v, CellValue::Err(_)))
                    .count()
            })
            .unwrap_or(0);
        Some(InspectorData {
            id: measure,
            name: m.name.0.clone(),
            is_derived: m.is_derived(),
            value_type: m.value_type,
            dimensions,
            dependencies,
            formula_english,
            error_cells,
        })
    }
}

/// Read-only facts about a measure, assembled for the inspector panel.
#[derive(Debug, PartialEq)]
struct InspectorData {
    id: MeasureId,
    name: String,
    is_derived: bool,
    value_type: ValueType,
    dimensions: Vec<String>,
    dependencies: Vec<String>,
    formula_english: Option<String>,
    error_cells: usize,
}

/// Build a live engine over all derived measures in `model`, plus its initial
/// snapshot. Falls back to no engine (inputs still render) on build failure.
fn build_engine(model: &Model) -> (Option<Engine>, HashMap<MeasureId, MeasureValues>) {
    let derived: Vec<MeasureId> = model
        .measures
        .values()
        .filter(|m| m.is_derived())
        .map(|m| m.id)
        .collect();
    if derived.is_empty() {
        return (None, HashMap::new());
    }
    match Engine::new(model, &derived) {
        Ok((e, snap)) => (Some(e), snap),
        Err(e) => {
            eprintln!("improv-gui: engine build failed: {e}");
            (None, HashMap::new())
        }
    }
}

fn decode(k: &CoordKey) -> improv_core_model::Coordinate {
    improv_core_model::Coordinate::from_pairs(k.iter().map(|(c, i)| (CategoryId(*c), ItemId(*i))))
}

fn pick_default_measure(model: &Model) -> Option<MeasureId> {
    let mut ids: Vec<MeasureId> = model.measures.keys().copied().collect();
    ids.sort_by_key(|m| m.0);
    ids.iter()
        .find(|m| {
            model
                .measures
                .get(m)
                .map(|x| x.is_derived())
                .unwrap_or(false)
        })
        .copied()
        .or_else(|| ids.first().copied())
}

/// The measure's categories in natural (declared) order (empty if none/absent).
fn natural_axis_order(model: &Model, measure: Option<MeasureId>) -> Vec<CategoryId> {
    measure
        .and_then(|m| model.measures.get(&m))
        .map(|m| m.categories.clone())
        .unwrap_or_default()
}

impl eframe::App for ImprovApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_axis_state();
        self.formula_bar(ctx);
        self.tool_palette(ctx);
        self.explorer_panel(ctx);
        self.inspector_panel(ctx);
        self.formula_panel(ctx);
        self.chart_panel(ctx);
        self.grid_panel(ctx);
    }
}

impl ImprovApp {
    /// A NeXTSTEP-style **tool palette**: a narrow left column of beveled
    /// buttons for the common operations (pivot, chart, save model, save view).
    /// Always visible, like the tear-off palettes in NeXTSTEP apps.
    fn tool_palette(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("tools")
            .resizable(false)
            .exact_width(44.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    let btn = |ui: &mut egui::Ui, glyph: &str, tip: &str| {
                        ui.add_sized([32.0, 28.0], egui::Button::new(glyph))
                            .on_hover_text(tip)
                            .clicked()
                    };
                    if btn(ui, "↻", "Pivot (rotate axes)") {
                        self.pivot_rotate();
                    }
                    if btn(ui, "☉", "Toggle chart") {
                        self.show_chart = !self.show_chart;
                    }
                    ui.add_space(6.0);
                    if btn(ui, "▤", "Save view") {
                        // Save under the current name box, or a generated name.
                        let name = if self.view_name.trim().is_empty() {
                            format!("view {}", self.model.views.len() + 1)
                        } else {
                            self.view_name.clone()
                        };
                        if self.save_view(&name).is_some() {
                            self.view_name.clear();
                            self.status = format!("saved view '{name}'");
                        }
                    }
                    if btn(ui, "⬇", "Save model to store") {
                        self.save();
                    }
                });
            });
    }

    /// Left: model explorer grouped into Categories (with their items) and
    /// Measures (input vs derived). Clicking a measure selects it.
    fn explorer_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("explorer")
            .resizable(true)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.heading("Model");
                ui.separator();

                egui::CollapsingHeader::new("Categories")
                    .default_open(true)
                    .show(ui, |ui| {
                        let mut cats: Vec<CategoryId> =
                            self.model.categories.keys().copied().collect();
                        cats.sort_by_key(|c| c.0);
                        for cid in cats {
                            let cat = &self.model.categories[&cid];
                            egui::CollapsingHeader::new(&cat.name.0)
                                .id_salt(("cat", cid.0))
                                .show(ui, |ui| {
                                    let mut items = cat.items.clone();
                                    items.sort_by_key(|i| i.0);
                                    for iid in items {
                                        if let Some(it) = self.model.items.get(&iid) {
                                            ui.label(&it.name.0);
                                        }
                                    }
                                });
                        }
                    });

                egui::CollapsingHeader::new("Measures")
                    .default_open(true)
                    .show(ui, |ui| {
                        let mut ids: Vec<MeasureId> = self.model.measures.keys().copied().collect();
                        ids.sort_by_key(|m| m.0);
                        for id in ids {
                            let m = &self.model.measures[&id];
                            let tag = if m.is_derived() { "= " } else { "· " };
                            let label = format!("{tag}{}", m.name.0);
                            if ui
                                .selectable_label(self.selected == Some(id), label)
                                .clicked()
                            {
                                self.selected = Some(id);
                                self.editing = None;
                            }
                        }
                    });

                self.views_section(ui);
            });
    }

    /// Views section in the explorer: a name field + "Save view" button that
    /// captures the current layout, and a list of saved views (click to load).
    fn views_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Views")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.view_name);
                    if ui.button("Save view").clicked() {
                        let name = self.view_name.clone();
                        if self.save_view(&name).is_some() {
                            self.view_name.clear();
                        }
                    }
                });
                let mut ids: Vec<ViewId> = self.model.views.keys().copied().collect();
                if ids.is_empty() {
                    ui.weak("(no saved views)");
                }
                ids.sort_by_key(|v| v.0);
                let mut load: Option<View> = None;
                for id in ids {
                    let v = &self.model.views[&id];
                    if ui.button(&v.name.0).clicked() {
                        load = Some(v.clone());
                    }
                }
                if let Some(v) = load {
                    self.apply_view(&v);
                }
            });
    }

    /// Right: inspector for the selected measure.
    fn inspector_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.heading("Inspector");
                ui.separator();
                let data = self.selected.and_then(|m| self.inspector_data(m));
                match data {
                    None => {
                        ui.label("No measure selected.");
                    }
                    Some(d) => {
                        ui.label(format!("id: {}", d.id.0));
                        ui.label(format!("name: {}", d.name));
                        ui.label(format!(
                            "kind: {}",
                            if d.is_derived { "derived" } else { "input" }
                        ));
                        ui.label(format!("value type: {:?}", d.value_type));
                        ui.label(format!(
                            "dimensions: {}",
                            if d.dimensions.is_empty() {
                                "(scalar)".to_string()
                            } else {
                                d.dimensions.join(" x ")
                            }
                        ));
                        if d.is_derived {
                            ui.separator();
                            ui.label(format!("depends on: {}", d.dependencies.join(", ")));
                            if let Some(eng) = &d.formula_english {
                                ui.label(format!("formula: {eng}"));
                            }
                            if d.error_cells > 0 {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 60, 60),
                                    format!("{} cell(s) have errors (#ERR)", d.error_cells),
                                );
                            }
                        }
                    }
                }
            });
    }

    /// Bottom: formula editor for the selected derived measure, plus a form to
    /// add a new derived measure.
    /// The Lotus Improv **formula bar**: a compact single-line bar across the
    /// top that shows and edits the *selected measure's* formula. Input
    /// measures show a hint. Committing re-typechecks and rebuilds the engine.
    fn formula_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("formula_bar").show(ctx, |ui| {
            // Reload the buffer when the selection changes.
            if self.formula_for != self.selected {
                self.formula_for = self.selected;
                self.formula_buf = self
                    .selected
                    .and_then(|m| self.model.measures.get(&m))
                    .and_then(|m| match &m.kind {
                        MeasureKind::Derived(f) => {
                            Some(describe_formula(&NlContext::new(&self.model), f))
                        }
                        MeasureKind::Input => None,
                    })
                    .unwrap_or_default();
            }
            ui.horizontal(|ui| match self.selected {
                Some(mid)
                    if self.model.measures.get(&mid).map(|m| m.is_derived()) == Some(true) =>
                {
                    ui.strong(format!("{} =", self.model.measures[&mid].name.0));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.formula_buf)
                            .desired_width(f32::INFINITY)
                            .hint_text("e.g. Price * Quantity"),
                    );
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Commit").clicked() || enter {
                        let text = self.formula_buf.clone();
                        match self.commit_formula(mid, &text) {
                            Ok(()) => self.status = "formula updated".into(),
                            Err(e) => self.status = format!("formula error: {e}"),
                        }
                    }
                }
                Some(mid) => {
                    ui.strong(format!("{} ", self.model.measures[&mid].name.0));
                    ui.weak("(input measure — edit cells in the grid)");
                }
                None => {
                    ui.weak("No measure selected.");
                }
            });
        });
    }

    /// Bottom panel: the "new derived measure" definition form + status line.
    /// (The selected measure's formula is edited in the top formula bar.)
    fn formula_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("definitions")
            .resizable(true)
            .default_height(80.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("New derived measure:");
                    ui.label("name");
                    ui.text_edit_singleline(&mut self.new_name);
                    ui.label("=");
                    ui.text_edit_singleline(&mut self.new_formula);
                    if ui.button("Add").clicked() {
                        let (name, text) = (self.new_name.clone(), self.new_formula.clone());
                        match self.add_derived_measure(&name, &text) {
                            Ok(id) => {
                                self.status = format!("added derived measure {}", id.0);
                                self.selected = Some(id);
                                self.new_name.clear();
                                self.new_formula.clear();
                            }
                            Err(e) => self.status = format!("add failed: {e}"),
                        }
                    }
                });
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            });
    }

    /// Bottom-right chart panel (shown only when the "Chart" toggle is on): a
    /// read-only bar chart of the selected measure with a bar/line toggle.
    fn chart_panel(&mut self, ctx: &egui::Context) {
        if !self.show_chart {
            return;
        }
        egui::SidePanel::right("chart")
            .resizable(true)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Chart");
                    ui.checkbox(&mut self.chart_line, "line");
                });
                ui.separator();
                let data = self.chart_series();
                crate::chart::render_chart(ui, &data, self.chart_line);
            });
    }

    /// Center: the axis shelf (drag/reassign categories) + page selectors +
    /// the pivot grid for the selected measure.
    fn grid_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| match self.selected {
            None => {
                ui.label("No measures. Open a model store with `improv-gui <db>`.");
            }
            Some(mid) => {
                let name = self.model.measures[&mid].name.0.clone();
                ui.horizontal(|ui| {
                    ui.heading(&name);
                    if ui
                        .selectable_label(self.show_chart, "Chart")
                        .on_hover_text("toggle a read-only chart of this measure")
                        .clicked()
                    {
                        self.show_chart = !self.show_chart;
                    }
                });
                self.margin_tiles(ui);
                self.page_selectors(ui);
                self.filter_shelf(ui);
                ui.separator();
                self.render_grid(ui, mid);
            }
        });
    }

    /// The signature Lotus Improv pivot gesture: category **tiles at the grid
    /// margins**. The top margin holds the *Columns* category tile, the left
    /// margin the *Rows* tile, and a strip holds the *Pages* tiles. Each tile
    /// is a drag source; each margin is a drop zone — drag a tile from one
    /// margin to another to re-pivot, exactly as in NeXTSTEP Improv, without
    /// touching any formula. A small `↻` on each tile is a mouse-only fallback
    /// that cycles rows→cols→pages.
    fn margin_tiles(&mut self, ui: &mut egui::Ui) {
        let cat_name = |app: &ImprovApp, c: CategoryId| {
            app.model
                .categories
                .get(&c)
                .map(|x| x.name.0.clone())
                .unwrap_or_else(|| format!("category {}", c.0))
        };
        let row_stack = self.row_cats();
        let col_stack = self.col_cats();
        let page_cats: Vec<CategoryId> = self.page_cats();
        let mut moves: Vec<(CategoryId, Axis)> = Vec::new();

        // A draggable category tile: raised beveled face with the category name.
        let tile = |ui: &mut egui::Ui, app: &ImprovApp, c: CategoryId, from: Axis| {
            let id = egui::Id::new(("tile", c.0));
            ui.dnd_drag_source(id, c, |ui| {
                egui::Frame::default()
                    .fill(crate::theme::NEXT_LIGHT)
                    .stroke(egui::Stroke::new(1.0_f32, crate::theme::BEVEL_SHADOW))
                    .inner_margin(egui::Margin::symmetric(8.0, 3.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(egui::RichText::new(cat_name(app, c)).strong());
                            let next = match from {
                                Axis::Rows => Axis::Columns,
                                Axis::Columns => Axis::Pages,
                                Axis::Pages => Axis::Rows,
                            };
                            if ui
                                .small_button("↻")
                                .on_hover_text("move to next axis")
                                .clicked()
                            {
                                // recorded below via the returned move
                                ui.data_mut(|d| d.insert_temp(id, next));
                            }
                        });
                    });
            });
            // Pull any cycle request recorded on this tile's id.
            if let Some(next) = ui.data(|d| d.get_temp::<Axis>(id)) {
                ui.data_mut(|d| d.remove::<Axis>(id));
                Some((c, next))
            } else {
                None
            }
        };

        // A margin drop zone with a NeXT-groove frame + axis label.
        let margin = |ui: &mut egui::Ui,
                      app: &ImprovApp,
                      label: &str,
                      axis: Axis,
                      cats: &[CategoryId],
                      moves: &mut Vec<(CategoryId, Axis)>| {
            ui.vertical(|ui| {
                ui.small(egui::RichText::new(label).weak());
                let frame = egui::Frame::default()
                    .fill(crate::theme::NEXT_GRAY)
                    .inner_margin(3.0)
                    .stroke(egui::Stroke::new(1.0_f32, crate::theme::NEXT_DARK));
                let (_, dropped) = ui.dnd_drop_zone::<CategoryId, ()>(frame, |ui| {
                    ui.set_min_size(egui::vec2(120.0, 28.0));
                    ui.horizontal_wrapped(|ui| {
                        if cats.is_empty() {
                            ui.weak("(drop here)");
                        }
                        for c in cats {
                            if let Some(m) = tile(ui, app, *c, axis) {
                                moves.push(m);
                            }
                        }
                    });
                });
                if let Some(c) = dropped {
                    moves.push((*c, axis));
                }
            });
        };

        ui.horizontal(|ui| {
            margin(ui, self, "↓ Columns", Axis::Columns, &col_stack, &mut moves);
            margin(ui, self, "→ Rows", Axis::Rows, &row_stack, &mut moves);
            margin(ui, self, "Pages", Axis::Pages, &page_cats, &mut moves);
            if ui.button("Pivot").on_hover_text("rotate axes").clicked() {
                self.pivot_rotate();
            }
        });

        for (c, axis) in moves {
            self.set_axis(c, axis);
        }
    }

    /// Page selectors: for each page (extra) dimension, a ` <label> [i/n] < > `
    /// control that pins which item the grid slices to. Mirrors the TUI paging.
    fn page_selectors(&mut self, ui: &mut egui::Ui) {
        let page_cats: Vec<CategoryId> = self.page_cats();
        if page_cats.is_empty() {
            return;
        }
        let mut set: Option<(usize, usize)> = None;
        ui.horizontal(|ui| {
            for (i, c) in page_cats.iter().enumerate() {
                let items = self.sorted_items(*c);
                if items.is_empty() {
                    continue;
                }
                let cur = self
                    .page_idx
                    .get(i)
                    .copied()
                    .unwrap_or(0)
                    .min(items.len() - 1);
                let cname = self
                    .model
                    .categories
                    .get(c)
                    .map(|x| x.name.0.clone())
                    .unwrap_or_default();
                ui.group(|ui| {
                    ui.label(&cname);
                    if ui.small_button("<").clicked() {
                        let prev = (cur + items.len() - 1) % items.len();
                        set = Some((i, prev));
                    }
                    ui.label(format!("{}  [{}/{}]", items[cur].1, cur + 1, items.len()));
                    if ui.small_button(">").clicked() {
                        set = Some((i, (cur + 1) % items.len()));
                    }
                });
            }
        });
        if let Some((dim, idx)) = set {
            self.set_page(dim, idx);
        }
    }

    /// Filter shelf: for each category on an axis, a collapsing checkbox list of
    /// its items. Unchecking an item hides it from the grid (presentation
    /// only); a "Clear filters" button restores all. Mirrors the TUI's f/F.
    fn filter_shelf(&mut self, ui: &mut egui::Ui) {
        let cats: Vec<CategoryId> = self.axis_order.clone();
        if cats.is_empty() {
            return;
        }
        let mut toggles: Vec<(CategoryId, ItemId)> = Vec::new();
        let mut clear = false;
        ui.collapsing("Filters", |ui| {
            for c in &cats {
                let cname = self
                    .model
                    .categories
                    .get(c)
                    .map(|x| x.name.0.clone())
                    .unwrap_or_default();
                // Full (unfiltered) item list so hidden items can be re-shown.
                let mut items: Vec<(ItemId, String)> = self
                    .model
                    .categories
                    .get(c)
                    .map(|cat| {
                        cat.items
                            .iter()
                            .filter_map(|id| {
                                self.model.items.get(id).map(|it| (*id, it.name.0.clone()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                items.sort_by_key(|(id, _)| id.0);
                let f = self.filters.iter().find(|f| f.category == *c);
                egui::CollapsingHeader::new(&cname)
                    .id_salt(("filter", c.0))
                    .show(ui, |ui| {
                        for (id, name) in &items {
                            let shown = match f {
                                Some(f) => f.items.contains(id),
                                None => true,
                            };
                            let mut checked = shown;
                            if ui.checkbox(&mut checked, name).changed() {
                                toggles.push((*c, *id));
                            }
                        }
                    });
            }
            if !self.filters.is_empty() && ui.button("Clear filters").clicked() {
                clear = true;
            }
        });
        for (c, i) in toggles {
            self.toggle_filter_item(c, i);
        }
        if clear {
            self.clear_filters();
        }
    }

    /// Handle keyboard navigation for the grid. Arrow keys (and h/j/k/l) move
    /// the cursor; Enter/F2 begin editing the cursor cell; `[`/`]` and
    /// PageUp/PageDown page the first page dimension; `n`/`N` cycle the
    /// selected measure. Swallowed while a cell text field is open (so typing
    /// a value doesn't also move the cursor). `n`/`N` (not Tab) drive measure
    /// cycling because egui reserves Tab for widget focus.
    fn handle_grid_keys(&mut self, ui: &egui::Ui) {
        // While editing a cell, let the text field own the keyboard (Enter/Esc
        // are handled in the cell rendering below).
        if self.editing.is_some() {
            return;
        }
        use egui::Key;
        let k = |key: Key| ui.input(|i| i.key_pressed(key));

        if k(Key::ArrowUp) || k(Key::K) {
            self.move_cursor(-1, 0);
        }
        if k(Key::ArrowDown) || k(Key::J) {
            self.move_cursor(1, 0);
        }
        if k(Key::ArrowLeft) || k(Key::H) {
            self.move_cursor(0, -1);
        }
        if k(Key::ArrowRight) || k(Key::L) {
            self.move_cursor(0, 1);
        }
        if k(Key::Enter) || k(Key::F2) {
            self.begin_edit_cursor();
        }
        // Page the first page dimension, if any.
        if k(Key::CloseBracket) || k(Key::PageDown) {
            self.page_first(1);
        }
        if k(Key::OpenBracket) || k(Key::PageUp) {
            self.page_first(-1);
        }
        // Cycle measures with n / N (Tab is taken by egui focus).
        if k(Key::N) {
            let shift = ui.input(|i| i.modifiers.shift);
            self.cycle_measure(if shift { -1 } else { 1 });
        }
    }

    /// Cycle the first page dimension by `delta` (wrapping) via `set_page`.
    fn page_first(&mut self, delta: isize) {
        let Some(cat) = self.page_cats().first().copied() else {
            return;
        };
        let count = self.sorted_items(cat).len();
        if count == 0 {
            return;
        }
        let cur = self.page_idx.first().copied().unwrap_or(0).min(count - 1);
        let next = (cur as isize + delta).rem_euclid(count as isize) as usize;
        self.set_page(0, next);
    }

    /// Cycle the selected measure by `delta` (wrapping) in id order.
    fn cycle_measure(&mut self, delta: isize) {
        let mut ids: Vec<MeasureId> = self.model.measures.keys().copied().collect();
        if ids.is_empty() {
            return;
        }
        ids.sort_by_key(|m| m.0);
        let cur = self
            .selected
            .and_then(|s| ids.iter().position(|m| *m == s))
            .unwrap_or(0);
        let next = (cur as isize + delta).rem_euclid(ids.len() as isize) as usize;
        self.selected = Some(ids[next]);
        self.editing = None;
    }

    /// Render `measure` as a 2-D pivot grid using the current axis order: the
    /// category at axis index 0 on rows, index 1 on columns, the rest pinned to
    /// their selected page item. Input cells are editable; derived read-only.
    fn render_grid(&mut self, ui: &mut egui::Ui, measure: MeasureId) {
        self.handle_grid_keys(ui);
        let cursor = (self.cursor_row, self.cursor_col);
        let is_derived = self
            .model
            .measures
            .get(&measure)
            .map(|m| m.is_derived())
            .unwrap_or(false);
        let values = self.values_for(measure);

        // Cartesian product of stacked categories per axis. Columns are
        // materialized up front (they become egui table columns, which the
        // TableBuilder needs before the body, and are few in practice). Rows
        // are VIRTUALIZED: we hold only the per-category item lists and decode
        // the i-th row tuple on demand (see `nth_tuple`), so a grid with
        // millions of row lines never allocates them all.
        let row_cats = self.row_cats();
        let col_cats = self.col_cats();
        let pinned = self.pinned_pages();
        let row_lists = self.axis_item_lists(&row_cats);
        // Total row lines: product of the row categories' filtered item counts
        // (1 when there are no row categories -> a single synthetic row; 0 if
        // any row category filtered to empty -> also render one blank line).
        let total_rows = product_len(&row_lists).max(1);
        let col_lines = {
            let t = self.axis_tuples(&col_cats);
            if t.is_empty() {
                vec![Vec::new()]
            } else {
                t
            }
        };
        let n_row_stub = row_cats.len().max(1); // stub columns (one per row cat)
        let n_col_hdr = col_cats.len().max(1); // header rows (one per col cat)

        // Decode the i-th row tuple on demand (empty when there are no row
        // categories -> the single synthetic row).
        let row_tuple_at = |lists: &[Vec<(ItemId, String)>], i: usize| -> Vec<(ItemId, String)> {
            if lists.is_empty() {
                Vec::new()
            } else {
                nth_tuple(lists, i)
            }
        };

        // Edits collected during rendering, applied after the table closure so
        // we don't borrow `self` mutably inside it.
        let mut commit: Option<(CoordKey, String)> = None;
        let mut clicked_derived = false;
        let mut cancel = false;

        // Chiseled header cell (raised bevel) matching NeXTSTEP Improv.
        fn header_cell(ui: &mut egui::Ui, text: &str) {
            egui::Frame::default()
                .fill(crate::theme::NEXT_LIGHT)
                .stroke(egui::Stroke::new(1.0_f32, crate::theme::BEVEL_SHADOW))
                .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text).strong()).truncate());
                });
        }
        let cat_name = |app: &ImprovApp, c: CategoryId| {
            app.model
                .categories
                .get(&c)
                .map(|x| x.name.0.clone())
                .unwrap_or_default()
        };

        use egui_extras::{Column, TableBuilder};
        let mut table = TableBuilder::new(ui).striped(true);
        // One stub column per stacked row category, then one column per col line.
        for _ in 0..n_row_stub {
            table = table.column(Column::auto().resizable(true));
        }
        for _ in &col_lines {
            table = table.column(Column::auto().resizable(true));
        }

        table
            .header(22.0 * n_col_hdr as f32, |mut header| {
                // Corner stub: the row category names stacked, then a header
                // block spanning the column-category rows. With egui_extras we
                // render the stacked column-category labels inside one tall
                // header cell per column line (outer→inner, top→bottom).
                for (si, rc) in row_cats.iter().enumerate() {
                    let _ = si;
                    header.col(|ui| {
                        header_cell(ui, &cat_name(self, *rc));
                    });
                }
                if row_cats.is_empty() {
                    header.col(|ui| {
                        header_cell(ui, "");
                    });
                }
                for line in &col_lines {
                    header.col(|ui| {
                        ui.vertical(|ui| {
                            if line.is_empty() {
                                header_cell(ui, "");
                            }
                            for (it, name) in line {
                                let _ = it;
                                header_cell(ui, name);
                            }
                        });
                    });
                }
            })
            .body(|body| {
                // Virtualized rows: `body.rows` only invokes the closure for the
                // rows currently visible in the viewport. Because rows are not
                // built contiguously, group outlining can't rely on a running
                // `prev` tracker — for row `ri` we decode row `ri-1`'s tuple on
                // demand and blank an outer stub cell when it matches.
                body.rows(20.0, total_rows, |mut row| {
                    let ri = row.index();
                    let row_line = row_tuple_at(&row_lists, ri);
                    let prev_line = if ri > 0 {
                        Some(row_tuple_at(&row_lists, ri - 1))
                    } else {
                        None
                    };
                    // Stub columns: one per stacked row category.
                    for si in 0..n_row_stub {
                        let cell = row_line.get(si);
                        row.col(|ui| {
                            match cell {
                                Some((_id, name)) => {
                                    // Show the label on the innermost stub always,
                                    // and on an outer stub only when this level or
                                    // any enclosing outer level changed from the
                                    // row above — group outlining.
                                    let inner = si + 1 == n_row_stub;
                                    let changed = match &prev_line {
                                        None => true,
                                        Some(p) => (0..=si).any(|k| {
                                            row_line.get(k).map(|(id, _)| id)
                                                != p.get(k).map(|(id, _)| id)
                                        }),
                                    };
                                    if inner || changed {
                                        header_cell(ui, name);
                                    } else {
                                        header_cell(ui, "");
                                    }
                                }
                                None => header_cell(ui, ""),
                            }
                        });
                    }
                    for (ci, col_line) in col_lines.iter().enumerate() {
                        let key =
                            cell_key_multi(&row_cats, &row_line, &col_cats, col_line, &pinned);
                        let is_cursor = cursor == (ri, ci);
                        row.col(|ui| {
                            let mut frame = egui::Frame::default();
                            if is_cursor {
                                frame = frame.fill(ui.visuals().selection.bg_fill).stroke(
                                    egui::Stroke::new(1.0_f32, ui.visuals().selection.stroke.color),
                                );
                            }
                            frame.show(ui, |ui| {
                                if is_derived {
                                    let text =
                                        self.derived_cell_text(measure, &key).unwrap_or_default();
                                    if ui.label(text).clicked() {
                                        self.cursor_row = ri;
                                        self.cursor_col = ci;
                                        clicked_derived = true;
                                    }
                                } else if self.editing.as_ref() == Some(&(measure, key.clone())) {
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut self.edit_buf)
                                            .desired_width(f32::INFINITY),
                                    );
                                    resp.request_focus();
                                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                    if esc {
                                        cancel = true;
                                    } else if resp.lost_focus() || enter {
                                        commit = Some((key.clone(), self.edit_buf.clone()));
                                    }
                                } else {
                                    let text = values
                                        .get(&key)
                                        .map(|v| format!("{v}"))
                                        .unwrap_or_default();
                                    if ui.button(text).clicked() {
                                        self.cursor_row = ri;
                                        self.cursor_col = ci;
                                        self.editing = Some((measure, key.clone()));
                                        self.edit_buf = values
                                            .get(&key)
                                            .map(|v| format!("{v}"))
                                            .unwrap_or_default();
                                    }
                                }
                            });
                        });
                    }
                });
            });

        if cancel {
            self.editing = None;
            self.status = "edit cancelled".into();
        }
        if clicked_derived {
            self.status = "derived cells are computed, not editable".into();
        }
        if let Some((key, text)) = commit {
            self.editing = None;
            let trimmed = text.trim();
            if trimmed.is_empty() {
                self.status = "empty cell not set".into();
            } else {
                match trimmed.parse::<f64>() {
                    Ok(v) => match self.set_cell(measure, key, v) {
                        Ok(()) => self.status = "cell updated".into(),
                        Err(e) => self.status = format!("edit error: {e}"),
                    },
                    Err(_) => self.status = format!("bad number: {trimmed:?}"),
                }
            }
        }
    }
}

/// The sorted `CoordKey` for a cell when categories are STACKED on each axis:
/// `row_cats[i]` binds to `row_tuple[i]`, `col_cats[j]` to `col_tuple[j]`, plus
/// the pinned page dims. Tuples come from `axis_tuples` so they align with the
/// category list. The general form of `cell_key`.
fn cell_key_multi(
    row_cats: &[CategoryId],
    row_tuple: &[(ItemId, String)],
    col_cats: &[CategoryId],
    col_tuple: &[(ItemId, String)],
    pinned: &[(CategoryId, ItemId)],
) -> CoordKey {
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for (c, (it, _)) in row_cats.iter().zip(row_tuple.iter()) {
        pairs.push((c.0, it.0));
    }
    for (c, (it, _)) in col_cats.iter().zip(col_tuple.iter()) {
        pairs.push((c.0, it.0));
    }
    for (c, i) in pinned {
        pairs.push((c.0, i.0));
    }
    pairs.sort();
    pairs
}

/// The number of axis lines the Cartesian product of `lists` produces: the
/// product of each list's length (empty `lists` -> 1, the scalar axis; any
/// empty list -> 0, no lines). Mirrors `axis_tuples(..).len()` but is O(k) in
/// the number of categories, never materializing the product.
fn product_len(lists: &[Vec<(ItemId, String)>]) -> usize {
    lists.iter().map(|l| l.len()).product()
}

/// The i-th line of the Cartesian product of `lists` (outer category first),
/// by mixed-radix decoding of `i` across the list lengths — the inner (last)
/// category is the least-significant digit, matching `axis_tuples`' ordering
/// (which increments the inner category fastest). Returns the bound
/// `(ItemId, name)` per category. `i` must be `< product_len(lists)`.
///
/// This is what lets the grid virtualize rows: instead of holding every row
/// tuple in a Vec, we decode line `i` (and, for group outlining, line `i-1`)
/// only when that row is actually painted.
fn nth_tuple(lists: &[Vec<(ItemId, String)>], mut i: usize) -> Vec<(ItemId, String)> {
    let mut out: Vec<(ItemId, String)> = vec![(ItemId(0), String::new()); lists.len()];
    for (d, list) in lists.iter().enumerate().rev() {
        let radix = list.len();
        debug_assert!(radix > 0, "empty category has no lines");
        out[d] = list[i % radix].clone();
        i /= radix;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{
        BinaryOp, DimensionSpec, Expr, Formula, Measure, MeasureKind, Name, ValueType,
    };

    fn revenue_model() -> Model {
        let mut m = Model::new();
        let (t, p) = (CategoryId(1), CategoryId(2));
        m.add_category(t, "Time");
        m.add_category(p, "Product");
        m.add_item(ItemId(10), t, "2025");
        m.add_item(ItemId(20), p, "WidgetA");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
        m.set_input(MeasureId(100), c(&[(p, ItemId(20))]), Value::Number(10.0));
        m.set_input(
            MeasureId(101),
            c(&[(t, ItemId(10)), (p, ItemId(20))]),
            Value::Number(7.0),
        );
        m
    }

    /// A revenue model without the Revenue derived measure (inputs only), for
    /// exercising the formula-commit / add-derived flows.
    fn inputs_only_model() -> Model {
        let mut m = revenue_model();
        m.measures.remove(&MeasureId(102));
        m
    }

    #[test]
    fn loads_and_computes_derived() {
        let app = build_app(revenue_model());
        // Revenue is derived and selected by default.
        assert_eq!(app.selected, Some(MeasureId(102)));
        let vals = app.values_for(MeasureId(102));
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&70.0)); // 10 * 7
    }

    #[test]
    fn editing_recomputes_derived() {
        let mut app = build_app(revenue_model());
        // Set Quantity[2025,WidgetA] = 9 -> Revenue = 90.
        let mut qkey = vec![(1u32, 10u32), (2u32, 20u32)];
        qkey.sort();
        app.set_cell(MeasureId(101), qkey.clone(), 9.0).unwrap();
        let rev = app.values_for(MeasureId(102));
        assert_eq!(rev.get(&qkey), Some(&90.0));
    }

    #[test]
    fn editing_derived_is_rejected() {
        let mut app = build_app(revenue_model());
        let key = vec![(1u32, 10u32), (2u32, 20u32)];
        assert!(app.set_cell(MeasureId(102), key, 1.0).is_err());
    }

    #[test]
    fn commit_formula_creates_and_recomputes() {
        // Turn an input measure into a derived one via a formula string, then
        // confirm the snapshot recomputes.
        let mut app = build_app(inputs_only_model());
        // Add a fresh target measure to hold the formula.
        let rev = app
            .add_derived_measure("Revenue", "Price * Quantity")
            .expect("add derived");
        let vals = app.values_for(rev);
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&70.0)); // 10 * 7
                                                 // The measure is derived and its categories are the union {Time, Product}.
        let m = &app.model.measures[&rev];
        assert!(m.is_derived());
        assert_eq!(m.categories, vec![CategoryId(1), CategoryId(2)]);
    }

    #[test]
    fn commit_formula_updates_existing_derived() {
        let mut app = build_app(revenue_model());
        // Redefine Revenue = Price + Quantity -> 10 + 7 = 17.
        app.commit_formula(MeasureId(102), "Price + Quantity")
            .expect("commit");
        let vals = app.values_for(MeasureId(102));
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&17.0));
    }

    #[test]
    fn bad_formula_leaves_model_unchanged() {
        let mut app = build_app(revenue_model());
        let before = app.model.clone();
        let err = app.commit_formula(MeasureId(102), "Price * Widgets");
        assert!(err.is_err());
        assert_eq!(app.model, before, "model unchanged on parse error");
        // Snapshot still computes the original Revenue.
        let vals = app.values_for(MeasureId(102));
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&70.0));
    }

    #[test]
    fn bad_add_derived_leaves_model_unchanged() {
        let mut app = build_app(revenue_model());
        let before = app.model.clone();
        assert!(app
            .add_derived_measure("Junk", "does not parse [[")
            .is_err());
        assert!(app.add_derived_measure("Revenue", "Price").is_err()); // dup name
        assert_eq!(app.model, before);
    }

    #[test]
    fn inspector_data_is_correct() {
        let app = build_app(revenue_model());
        let d = app.inspector_data(MeasureId(102)).expect("data");
        assert_eq!(d.name, "Revenue");
        assert!(d.is_derived);
        assert_eq!(
            d.dimensions,
            vec!["Time".to_string(), "Product".to_string()]
        );
        let mut deps = d.dependencies.clone();
        deps.sort();
        assert_eq!(deps, vec!["Price".to_string(), "Quantity".to_string()]);
        assert!(d.formula_english.is_some());
        assert_eq!(d.error_cells, 0);

        // Input measure: no deps, no formula.
        let di = app.inspector_data(MeasureId(100)).expect("data");
        assert!(!di.is_derived);
        assert!(di.dependencies.is_empty());
        assert!(di.formula_english.is_none());
        assert_eq!(di.dimensions, vec!["Product".to_string()]);
    }

    /// Build an app directly from a model (bypassing the store) for tests.
    fn build_app(model: Model) -> ImprovApp {
        let (engine, snapshot) = build_engine(&model);
        let selected = pick_default_measure(&model);
        let axis_order = natural_axis_order(&model, selected);
        let page_idx = vec![0; axis_order.len().saturating_sub(2)];
        ImprovApp {
            db: String::new(),
            model,
            engine,
            snapshot,
            selected,
            status: String::new(),
            editing: None,
            edit_buf: String::new(),
            formula_buf: String::new(),
            formula_for: None,
            new_name: String::new(),
            new_formula: String::new(),
            axis_order,
            n_rows: 1,
            n_cols: 1,
            page_idx,
            axis_for: selected,
            filters: Vec::new(),
            view_name: String::new(),
            cursor_row: 0,
            cursor_col: 0,
            show_chart: false,
            chart_line: false,
        }
    }

    // A 3-D input measure Sales[Time, Product, Region] for paging tests
    // (mirrors the TUI's paging fixture).
    fn sales_3d_model() -> Model {
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
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
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
        m
    }

    #[test]
    fn set_axis_moves_category_from_columns_to_rows() {
        // Quantity[Time, Product]: natural rows=Time(1), cols=Product(2).
        let mut app = build_app(revenue_model());
        app.selected = Some(MeasureId(101));
        app.sync_axis_state();
        let (r, c, _) = app.resolved_axes();
        assert_eq!(r, Some(CategoryId(1))); // Time on rows
        assert_eq!(c, Some(CategoryId(2))); // Product on cols

        // Dragging Product onto Rows STACKS it under Time (both on rows), which
        // is the Improv semantics; columns become empty.
        app.set_axis(CategoryId(2), Axis::Rows);
        assert_eq!(app.row_cats(), vec![CategoryId(1), CategoryId(2)]);
        assert!(app.col_cats().is_empty());
        let (r, c, _) = app.resolved_axes();
        assert_eq!(r, Some(CategoryId(1)), "primary row is still Time");
        assert_eq!(c, None, "no column category after stacking both on rows");
    }

    #[test]
    fn stacked_rows_form_cartesian_product_with_correct_keys() {
        // Quantity[Time, Product] with BOTH categories stacked on rows.
        let mut app = build_app(revenue_model());
        app.selected = Some(MeasureId(101));
        app.sync_axis_state();
        app.set_axis(CategoryId(2), Axis::Rows); // rows = [Time, Product]
        assert_eq!(app.row_cats(), vec![CategoryId(1), CategoryId(2)]);

        // The row axis is the product of Time items x Product items.
        let tuples = app.axis_tuples(&app.row_cats());
        // revenue_model has Time={2025(10)}, Product={WidgetA(20)} (1x1) here,
        // so a single tuple binding both categories.
        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].len(), 2, "tuple binds both stacked categories");
        // The cell key for that row (no columns) binds Time AND Product, sorted.
        let key = cell_key_multi(&app.row_cats(), &tuples[0], &[], &[], &[]);
        assert_eq!(key, vec![(1, 10), (2, 20)]);
        // Grid dims: 1 row line, 1 col line (no col categories).
        assert_eq!(app.grid_dims(), (1, 1));
    }

    #[test]
    fn nth_tuple_decodes_mixed_radix_and_matches_axis_tuples() {
        // Row cats Time(4 items) x Region(2 items): 8 lines, inner (Region)
        // varies fastest — same order as axis_tuples.
        let time: Vec<(ItemId, String)> =
            (0..4).map(|k| (ItemId(10 + k), format!("T{k}"))).collect();
        let region: Vec<(ItemId, String)> =
            (0..2).map(|k| (ItemId(30 + k), format!("R{k}"))).collect();
        let lists = vec![time.clone(), region.clone()];
        assert_eq!(product_len(&lists), 8);

        // line 0 = (T0, R0); line 1 = (T0, R1); line 7 = (T3, R1).
        assert_eq!(
            nth_tuple(&lists, 0),
            vec![time[0].clone(), region[0].clone()]
        );
        assert_eq!(
            nth_tuple(&lists, 1),
            vec![time[0].clone(), region[1].clone()]
        );
        assert_eq!(
            nth_tuple(&lists, 7),
            vec![time[3].clone(), region[1].clone()]
        );

        // Iterating 0..total reproduces the full Cartesian product built by the
        // reference product-builder (same ordering as axis_tuples).
        let mut reference: Vec<Vec<(ItemId, String)>> = vec![Vec::new()];
        for list in &lists {
            let mut next = Vec::new();
            for prefix in &reference {
                for it in list {
                    let mut t = prefix.clone();
                    t.push(it.clone());
                    next.push(t);
                }
            }
            reference = next;
        }
        let decoded: Vec<_> = (0..product_len(&lists))
            .map(|i| nth_tuple(&lists, i))
            .collect();
        assert_eq!(decoded, reference);
    }

    #[test]
    fn nth_tuple_matches_apps_axis_tuples() {
        // Cross-check against the app's own axis_tuples on the 2x2 model.
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        app.set_axis(CategoryId(2), Axis::Rows); // rows = [Time, Product] stacked
        let cats = app.row_cats();
        let lists = app.axis_item_lists(&cats);
        let want = app.axis_tuples(&cats);
        assert_eq!(product_len(&lists), want.len());
        for (i, w) in want.iter().enumerate() {
            assert_eq!(nth_tuple(&lists, i), *w, "line {i}");
        }
    }

    #[test]
    fn large_grid_row_count_is_correct_and_does_not_panic() {
        // A synthetic model with a category of a few thousand items so the row
        // product is large; the pure helpers must give the right count and
        // decode any line without materializing the whole product.
        let mut m = Model::new();
        let (big, small) = (CategoryId(1), CategoryId(2));
        m.add_category(big, "Big");
        m.add_category(small, "Small");
        let n_big = 5_000u32;
        for k in 0..n_big {
            m.add_item(ItemId(1_000 + k), big, format!("b{k}"));
        }
        for k in 0..3u32 {
            m.add_item(ItemId(10 + k), small, format!("s{k}"));
        }
        m.add_measure(Measure {
            id: MeasureId(300),
            name: Name("M".into()),
            value_type: ValueType::Number,
            categories: vec![big, small],
            kind: MeasureKind::Input,
            description: None,
        });
        let mut app = build_app(m);
        app.selected = Some(MeasureId(300));
        app.sync_axis_state();
        // Stack both categories on rows -> 5000 * 3 = 15000 row lines.
        app.set_axis(small, Axis::Rows);
        assert_eq!(app.row_cats(), vec![big, small]);

        let lists = app.axis_item_lists(&app.row_cats());
        let total = product_len(&lists);
        assert_eq!(total, (n_big as usize) * 3);
        // grid_dims reports the same total (no column category -> 1 col).
        assert_eq!(app.grid_dims(), (total, 1));

        // Decode the first, a middle, and the last line without panic.
        let first = nth_tuple(&lists, 0);
        assert_eq!(first[0].0, ItemId(1_000));
        assert_eq!(first[1].0, ItemId(10));
        let last = nth_tuple(&lists, total - 1);
        assert_eq!(last[0].0, ItemId(1_000 + n_big - 1));
        assert_eq!(last[1].0, ItemId(12));
        // A cursor deep in the grid resolves its key without materializing rows.
        app.cursor_row = total - 1;
        app.cursor_col = 0;
        let key = app.cursor_key();
        let mut want = vec![(big.0, 1_000 + n_big - 1), (small.0, 12)];
        want.sort();
        assert_eq!(key, want);
    }

    #[test]
    fn pivot_rotate_swaps_axes_and_back() {
        let mut app = build_app(revenue_model());
        app.selected = Some(MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        let (r0, c0, _) = app.resolved_axes();
        assert_eq!((r0, c0), (Some(CategoryId(1)), Some(CategoryId(2))));

        app.pivot_rotate();
        let (r1, c1, _) = app.resolved_axes();
        assert_eq!((r1, c1), (Some(CategoryId(2)), Some(CategoryId(1))));

        app.pivot_rotate(); // back to start for a 2-D measure
        let (r2, c2, _) = app.resolved_axes();
        assert_eq!((r2, c2), (Some(CategoryId(1)), Some(CategoryId(2))));
    }

    #[test]
    fn set_page_changes_pinned_item_and_cell_value() {
        let mut app = build_app(sales_3d_model());
        app.selected = Some(MeasureId(200));
        app.sync_axis_state();
        // One page dim (Region), pinned to North (index 0) by default.
        let (_, _, pinned) = app.resolved_axes();
        assert_eq!(pinned, vec![(CategoryId(3), ItemId(30))]); // North

        // Cell [2025, WidgetA, North] = 100.
        let vals = app.values_for(MeasureId(200));
        let mut north = vec![(1u32, 10u32), (2, 20), (3, 30)];
        north.sort();
        assert_eq!(vals.get(&north), Some(&100.0));

        // Page to South (index 1): pinned item and the visible value change.
        app.set_page(0, 1);
        let (_, _, pinned) = app.resolved_axes();
        assert_eq!(pinned, vec![(CategoryId(3), ItemId(31))]); // South
        let mut south = vec![(1u32, 10u32), (2, 20), (3, 31)];
        south.sort();
        assert_eq!(vals.get(&south), Some(&250.0));
    }

    #[test]
    fn switching_measure_resets_axis_order() {
        let mut app = build_app(sales_3d_model());
        app.selected = Some(MeasureId(200));
        app.sync_axis_state();
        // Pivot away from natural order.
        app.pivot_rotate();
        assert_ne!(
            app.axis_order,
            vec![CategoryId(1), CategoryId(2), CategoryId(3)]
        );

        // Add a 1-D measure and select it: axis order resets to its natural order.
        app.model.add_measure(Measure {
            id: MeasureId(201),
            name: Name("Tax".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(2)],
            kind: MeasureKind::Input,
            description: None,
        });
        app.selected = Some(MeasureId(201));
        app.sync_axis_state();
        assert_eq!(app.axis_order, vec![CategoryId(2)]);
        assert!(app.page_idx.is_empty());

        // Back to Sales: natural order restored (not the pivoted one).
        app.selected = Some(MeasureId(200));
        app.sync_axis_state();
        assert_eq!(
            app.axis_order,
            vec![CategoryId(1), CategoryId(2), CategoryId(3)]
        );
    }

    /// A 2x2 input measure Quantity[Time(2025,2026), Product(WidgetA,WidgetB)]
    /// plus Revenue = Price * Quantity, for cursor navigation/edit tests.
    fn grid_2x2_model() -> Model {
        let mut m = Model::new();
        let (t, p) = (CategoryId(1), CategoryId(2));
        m.add_category(t, "Time");
        m.add_category(p, "Product");
        m.add_item(ItemId(10), t, "2025");
        m.add_item(ItemId(11), t, "2026");
        m.add_item(ItemId(20), p, "WidgetA");
        m.add_item(ItemId(21), p, "WidgetB");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
        m.set_input(MeasureId(100), c(&[(p, ItemId(20))]), Value::Number(10.0));
        m.set_input(MeasureId(100), c(&[(p, ItemId(21))]), Value::Number(20.0));
        for (ti, pi, q) in [
            (ItemId(10), ItemId(20), 100.0),
            (ItemId(10), ItemId(21), 50.0),
            (ItemId(11), ItemId(20), 120.0),
            (ItemId(11), ItemId(21), 80.0),
        ] {
            m.set_input(MeasureId(101), c(&[(t, ti), (p, pi)]), Value::Number(q));
        }
        m
    }

    #[test]
    fn cursor_clamps_at_all_edges_and_after_pivot_shrink() {
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(101)); // Quantity[Time, Product], 2x2
        app.sync_axis_state();

        // Off the top-left: clamps to (0, 0).
        app.move_cursor(-5, -5);
        assert_eq!((app.cursor_row, app.cursor_col), (0, 0));
        // Off the bottom-right: clamps to (1, 1).
        app.move_cursor(100, 100);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));

        // Switch to Price[Product]: 2 rows, 1 synthetic col -> column re-clamps.
        app.selected = Some(MeasureId(100));
        app.sync_axis_state(); // resets cursor to (0,0) on measure switch
        app.move_cursor(5, 5);
        assert_eq!(app.cursor_col, 0, "single-column grid clamps col to 0");
        assert_eq!(app.cursor_row, 1, "two rows -> max row 1");

        // Sales 3-D: put the cursor at the far corner, then pivot to a shape
        // where the cursor would be out of range; clamp_cursor must fix it.
        let mut app = build_app(sales_3d_model());
        app.selected = Some(MeasureId(200));
        app.sync_axis_state();
        // Region on cols has 2 items; move to the far cell.
        app.pivot_rotate(); // rows=Product, cols=Region (2 cols)
        app.move_cursor(100, 100);
        let (r, c) = (app.cursor_row, app.cursor_col);
        let (rows, cols) = app.grid_dims();
        assert!(r < rows && c < cols, "cursor within {rows}x{cols}");
        // Pivot again (rows=Region -> single-item axes elsewhere) and confirm
        // the cursor never goes out of range.
        app.pivot_rotate();
        let (rows, cols) = app.grid_dims();
        assert!(app.cursor_row < rows && app.cursor_col < cols);
    }

    #[test]
    fn cursor_maps_to_expected_coord_key() {
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        // [1,1] = Quantity[2026, WidgetB] = Time(11), Product(21).
        app.cursor_row = 1;
        app.cursor_col = 1;
        let mut expect = vec![(1u32, 11u32), (2, 21)];
        expect.sort();
        assert_eq!(app.cursor_key(), expect);

        // [0,1] = Quantity[2025, WidgetB] = Time(10), Product(21).
        app.cursor_row = 0;
        app.cursor_col = 1;
        let mut expect = vec![(1u32, 10u32), (2, 21)];
        expect.sort();
        assert_eq!(app.cursor_key(), expect);
    }

    #[test]
    fn move_then_edit_routes_through_set_cell_and_recomputes() {
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        assert!(app.cursor_is_editable());

        // Move to Quantity[2025, WidgetA] = [0,0], set it to 200.
        app.cursor_row = 0;
        app.cursor_col = 0;
        let key = app.cursor_key();
        app.set_cell(MeasureId(101), key, 200.0).unwrap();

        // Revenue[2025, WidgetA] = Price(10) * 200 = 2000 in the snapshot.
        let mut rkey = vec![(1u32, 10u32), (2, 20)];
        rkey.sort();
        let rev = app.values_for(MeasureId(102));
        assert_eq!(rev.get(&rkey), Some(&2000.0));

        // Derived measure: cursor cell is not editable (status set, not enter).
        app.selected = Some(MeasureId(102));
        app.sync_axis_state();
        assert!(!app.cursor_is_editable());
        app.begin_edit_cursor();
        assert!(app.editing.is_none());
        assert_eq!(app.status, "derived cells are computed, not editable");
    }

    #[test]
    fn filter_hides_an_item_from_the_rendered_rows() {
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        // Rows = Time (2025, 2026); hide 2026 (ItemId 11).
        let (row_cat, _, _) = app.resolved_axes();
        assert_eq!(row_cat, Some(CategoryId(1)));
        assert_eq!(app.sorted_items(CategoryId(1)).len(), 2);
        app.toggle_filter_item(CategoryId(1), ItemId(11));
        let rows: Vec<ItemId> = app
            .sorted_items(CategoryId(1))
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(rows, vec![ItemId(10)], "2026 filtered out of the row axis");
        // Columns (unfiltered Product) still show both.
        assert_eq!(app.sorted_items(CategoryId(2)).len(), 2);
        // Re-showing 2026 drops the filter (all items kept).
        app.toggle_filter_item(CategoryId(1), ItemId(11));
        assert!(app.filters.is_empty());
        assert_eq!(app.sorted_items(CategoryId(1)).len(), 2);
    }

    #[test]
    fn save_view_captures_measure_axis_order_and_filters() {
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        app.pivot_rotate(); // rows=Product, cols=Time
        app.toggle_filter_item(CategoryId(2), ItemId(21)); // hide WidgetB

        let id = app.save_view("L1").expect("saved");
        let v = app.model.views.get(&id).expect("view stored");
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
        let mut src = build_app(grid_2x2_model());
        src.selected = Some(MeasureId(101));
        src.sync_axis_state();
        src.pivot_rotate(); // rows=Product, cols=Time
        src.toggle_filter_item(CategoryId(2), ItemId(21)); // hide WidgetB
        let v = src.build_view(ViewId(1), "L1").expect("view");

        let mut dst = build_app(grid_2x2_model());
        // dst starts on Revenue (derived) with natural axes and no filters.
        assert_eq!(dst.selected, Some(MeasureId(102)));
        dst.apply_view(&v);
        dst.sync_axis_state(); // must not clobber the applied layout

        assert_eq!(dst.selected, Some(MeasureId(101)));
        assert_eq!(dst.axis_order, vec![CategoryId(2), CategoryId(1)]);
        assert_eq!(dst.filters, v.filters);
        let (row_cat, col_cat, _) = dst.resolved_axes();
        assert_eq!(row_cat, Some(CategoryId(2))); // Product on rows
        assert_eq!(col_cat, Some(CategoryId(1))); // Time on cols
                                                  // Filter reflected: only WidgetA on the (row) Product axis.
        let rows: Vec<ItemId> = dst
            .sorted_items(CategoryId(2))
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(rows, vec![ItemId(20)]);
    }

    #[test]
    fn apply_view_restores_page_item() {
        let mut src = build_app(sales_3d_model());
        src.selected = Some(MeasureId(200));
        src.sync_axis_state();
        // Page Region (dim 0) to South (index 1).
        src.set_page(0, 1);
        let (_, _, pinned) = src.resolved_axes();
        assert_eq!(pinned, vec![(CategoryId(3), ItemId(31))]); // South
        let v = src.build_view(ViewId(1), "south").expect("view");
        assert_eq!(v.page_items, vec![(CategoryId(3), ItemId(31))]);

        let mut dst = build_app(sales_3d_model());
        dst.selected = Some(MeasureId(200));
        dst.sync_axis_state();
        let (_, _, pinned) = dst.resolved_axes();
        assert_eq!(pinned, vec![(CategoryId(3), ItemId(30))]); // North default
        dst.apply_view(&v);
        let (_, _, pinned) = dst.resolved_axes();
        assert_eq!(
            pinned,
            vec![(CategoryId(3), ItemId(31))],
            "page item restored"
        );
    }

    // -- chart_series (pure; no egui) --------------------------------------

    #[test]
    fn chart_series_yields_labels_and_oracle_values() {
        // Revenue[Time, Product], natural axes: rows=Time -> x, cols=Product ->
        // one series each. Oracle: WidgetA = 1000/1200, WidgetB = 1000/1600.
        let app = build_app(grid_2x2_model());
        assert_eq!(app.selected, Some(MeasureId(102))); // Revenue selected
        let d = app.chart_series();
        assert_eq!(d.x_title, "Time");
        assert_eq!(d.x_labels, vec!["2025".to_string(), "2026".to_string()]);
        assert_eq!(d.series.len(), 2);
        assert_eq!(d.series[0].name, "WidgetA");
        assert_eq!(d.series[0].points, vec![Some(1000.0), Some(1200.0)]);
        assert_eq!(d.series[1].name, "WidgetB");
        assert_eq!(d.series[1].points, vec![Some(1000.0), Some(1600.0)]);
        // y-range spans 0..1600 (0 always included).
        assert_eq!(d.y_range(), (0.0, 1600.0));
    }

    #[test]
    fn chart_series_stacked_rows_join_tuple_labels_and_match_cell_keys() {
        // Revenue[Time, Product]: stack BOTH categories on rows, leaving no
        // column category. x-labels become the joined row tuples
        // ("2025 / WidgetA", ...) and there is one unnamed series whose values
        // match cell_key_multi lookups.
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(102)); // Revenue
        app.sync_axis_state();
        app.set_axis(CategoryId(2), Axis::Rows); // rows = [Time, Product]
        assert_eq!(app.row_cats(), vec![CategoryId(1), CategoryId(2)]);
        assert!(app.col_cats().is_empty());

        let d = app.chart_series();
        assert_eq!(d.x_title, "Time / Product");
        assert_eq!(
            d.x_labels,
            vec![
                "2025 / WidgetA".to_string(),
                "2025 / WidgetB".to_string(),
                "2026 / WidgetA".to_string(),
                "2026 / WidgetB".to_string(),
            ]
        );
        // One series (no column categories), unnamed.
        assert_eq!(d.series.len(), 1);
        assert_eq!(d.series[0].name, "");

        // Each point matches a direct cell_key_multi lookup on the same tuples.
        let values = app.values_for(MeasureId(102));
        let row_tuples = app.axis_tuples(&app.row_cats());
        let want: Vec<Option<f64>> = row_tuples
            .iter()
            .map(|t| {
                let key = cell_key_multi(&app.row_cats(), t, &[], &[], &[]);
                values.get(&key).copied()
            })
            .collect();
        assert_eq!(d.series[0].points, want);
        // Oracle: WidgetA prices 10/20; Quantities 100,50,120,80 ->
        // 1000, 1000, 1200, 1600.
        assert_eq!(
            d.series[0].points,
            vec![Some(1000.0), Some(1000.0), Some(1200.0), Some(1600.0)]
        );
    }

    #[test]
    fn chart_series_stacked_columns_form_one_series_per_col_tuple() {
        // Revenue[Time, Product]: rows=Time, columns stack both? Instead stack
        // Product on rows AND keep Time on cols to exercise a multi-tuple
        // column axis. Here: rows=Product (2), cols=Time (2) -> 2 x-labels,
        // 2 series named by the (single-item) column tuples.
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(102));
        app.sync_axis_state();
        app.pivot_rotate(); // rows=Product, cols=Time
        assert_eq!(app.row_cats(), vec![CategoryId(2)]);
        assert_eq!(app.col_cats(), vec![CategoryId(1)]);

        let d = app.chart_series();
        assert_eq!(d.x_title, "Product");
        assert_eq!(
            d.x_labels,
            vec!["WidgetA".to_string(), "WidgetB".to_string()]
        );
        // One series per Time item (single-element column tuples).
        assert_eq!(d.series.len(), 2);
        assert_eq!(d.series[0].name, "2025");
        assert_eq!(d.series[1].name, "2026");
        // 2025: WidgetA=1000, WidgetB=1000; 2026: WidgetA=1200, WidgetB=1600.
        assert_eq!(d.series[0].points, vec![Some(1000.0), Some(1000.0)]);
        assert_eq!(d.series[1].points, vec![Some(1200.0), Some(1600.0)]);
    }

    #[test]
    fn chart_series_1d_single_series() {
        // Price[Product]: 1-D grid -> a single unnamed series over Product.
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(100)); // Price[Product]
        app.sync_axis_state();
        let d = app.chart_series();
        assert_eq!(d.x_title, "Product");
        assert_eq!(
            d.x_labels,
            vec!["WidgetA".to_string(), "WidgetB".to_string()]
        );
        assert_eq!(d.series.len(), 1);
        assert_eq!(d.series[0].name, "");
        assert_eq!(d.series[0].points, vec![Some(10.0), Some(20.0)]);
    }

    #[test]
    fn chart_filter_removes_a_bar() {
        // Hide 2026 on the Time (x) axis: its label and points drop out.
        let mut app = build_app(grid_2x2_model());
        app.selected = Some(MeasureId(102));
        app.sync_axis_state();
        app.toggle_filter_item(CategoryId(1), ItemId(11)); // hide 2026
        let d = app.chart_series();
        assert_eq!(d.x_labels, vec!["2025".to_string()], "2026 filtered out");
        assert_eq!(d.series[0].points, vec![Some(1000.0)]); // WidgetA
        assert_eq!(d.series[1].points, vec![Some(1000.0)]); // WidgetB
    }

    #[test]
    fn chart_non_numeric_cell_is_a_gap_not_a_panic() {
        // Overwrite one Revenue-input cell so a derived cell errors, and set a
        // Text input on another measure: both surface as gaps, not panics.
        let mut app = build_app(grid_2x2_model());
        // Make Quantity[2026, WidgetA] a Text value -> Revenue[2026, WidgetA]
        // becomes an Error (type mismatch), which values_for skips (gap).
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
        app.model.set_input(
            MeasureId(101),
            c(&[(CategoryId(1), ItemId(11)), (CategoryId(2), ItemId(20))]),
            Value::Text("oops".into()),
        );
        app.rebuild_engine();
        app.selected = Some(MeasureId(102));
        app.sync_axis_state();
        let d = app.chart_series(); // must not panic
                                    // WidgetA series: 2025 numeric, 2026 is a gap (None).
        assert_eq!(d.series[0].name, "WidgetA");
        assert_eq!(d.series[0].points, vec![Some(1000.0), None]);
        // WidgetB series unaffected.
        assert_eq!(d.series[1].points, vec![Some(1000.0), Some(1600.0)]);
        // y-range still valid, 0 included.
        let (lo, hi) = d.y_range();
        assert_eq!(lo, 0.0);
        assert!(hi >= 1600.0);
    }
}
