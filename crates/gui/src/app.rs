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
    parser, CategoryId, ItemId, Measure, MeasureId, MeasureKind, Model, Name, Value, ValueType,
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
    /// Selected item index for each page (extra) dimension, positionally by
    /// page dim (i.e. `axis_order[2 + i]`). Reset on measure switch.
    page_idx: Vec<usize>,
    /// The measure `axis_order`/`page_idx` currently describe (so we reset the
    /// pivot state when the selection changes).
    axis_for: Option<MeasureId>,

    /// Keyboard cell cursor into the current grid (row/col indices), clamped to
    /// the grid's dimensions. Reset when the selected measure or pivot changes.
    cursor_row: usize,
    cursor_col: usize,
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
            page_idx: Vec::new(),
            axis_for: selected,
            cursor_row: 0,
            cursor_col: 0,
        })
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
            self.page_idx = vec![0; self.axis_order.len().saturating_sub(2)];
            self.cursor_row = 0;
            self.cursor_col = 0;
        } else if self.page_idx.len() != self.axis_order.len().saturating_sub(2) {
            // Keep page_idx sized to the current page-dimension count.
            self.page_idx
                .resize(self.axis_order.len().saturating_sub(2), 0);
        }
        self.clamp_cursor();
    }

    /// Resolved axes for the current pivot state: (row cat, col cat, pinned
    /// page dims as (category, item)). Mirrors the grid's cell keying. Page
    /// items are the selected index for each page dimension (clamped).
    fn resolved_axes(
        &self,
    ) -> (
        Option<CategoryId>,
        Option<CategoryId>,
        Vec<(CategoryId, ItemId)>,
    ) {
        let row_cat = self.axis_order.first().copied();
        let col_cat = self.axis_order.get(1).copied();
        let mut pinned = Vec::new();
        for (pi, c) in self.axis_order.iter().skip(2).enumerate() {
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
        (row_cat, col_cat, pinned)
    }

    /// A category's items sorted by id (shared by paging and grid rendering).
    fn sorted_items(&self, c: CategoryId) -> Vec<(ItemId, String)> {
        let mut v: Vec<(ItemId, String)> = self
            .model
            .categories
            .get(&c)
            .map(|cat| {
                cat.items
                    .iter()
                    .filter_map(|id| self.model.items.get(id).map(|it| (*id, it.name.0.clone())))
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|(id, _)| id.0);
        v
    }

    /// Move `category` to `axis`. Rows/Columns take the single slot at index
    /// 0/1 (swapping out whatever was there, which drops to pages); Pages sends
    /// it to the end. No-op if the category is not in the current axis order.
    /// Pivoting is formula-free re-projection (Improv/Quantrix signature move).
    pub fn set_axis(&mut self, category: CategoryId, axis: Axis) {
        let Some(cur) = self.axis_order.iter().position(|c| *c == category) else {
            return;
        };
        self.axis_order.remove(cur);
        match axis {
            Axis::Rows => self.axis_order.insert(0, category),
            Axis::Columns => {
                let at = 1.min(self.axis_order.len());
                self.axis_order.insert(at, category);
            }
            Axis::Pages => self.axis_order.push(category),
        }
        self.page_idx = vec![0; self.axis_order.len().saturating_sub(2)];
        self.clamp_cursor();
    }

    /// Pivot: rotate the axis order left (rows->pages, cols->rows, first
    /// page->cols). Repeatedly cycles which category sits on each axis. No-op
    /// for < 2 categories. Mirrors the TUI's `pivot()`.
    pub fn pivot_rotate(&mut self) {
        if self.axis_order.len() < 2 {
            return;
        }
        self.axis_order.rotate_left(1);
        self.page_idx = vec![0; self.axis_order.len().saturating_sub(2)];
        self.clamp_cursor();
    }

    /// Set the pinned item index for page dimension `dim_index` (position among
    /// the page dims, i.e. `axis_order[2 + dim_index]`), clamped to the
    /// dimension's item count. No-op if out of range.
    pub fn set_page(&mut self, dim_index: usize, item_index: usize) {
        let Some(cat) = self.axis_order.get(2 + dim_index).copied() else {
            return;
        };
        let count = self.sorted_items(cat).len();
        if count == 0 {
            return;
        }
        if self.page_idx.len() != self.axis_order.len().saturating_sub(2) {
            self.page_idx
                .resize(self.axis_order.len().saturating_sub(2), 0);
        }
        if let Some(slot) = self.page_idx.get_mut(dim_index) {
            *slot = item_index.min(count - 1);
        }
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
        let (row_cat, col_cat, _) = self.resolved_axes();
        let rows = row_cat
            .map(|c| self.sorted_items(c).len().max(1))
            .unwrap_or(1);
        let cols = col_cat
            .map(|c| self.sorted_items(c).len().max(1))
            .unwrap_or(1);
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
        let (row_cat, col_cat, pinned) = self.resolved_axes();
        let rid = row_cat
            .and_then(|c| self.sorted_items(c).get(self.cursor_row).map(|(id, _)| *id))
            .unwrap_or(ItemId(0));
        let cid = col_cat
            .and_then(|c| self.sorted_items(c).get(self.cursor_col).map(|(id, _)| *id))
            .unwrap_or(ItemId(0));
        cell_key(row_cat, col_cat, rid, cid, &pinned)
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
        self.explorer_panel(ctx);
        self.inspector_panel(ctx);
        self.formula_panel(ctx);
        self.grid_panel(ctx);
    }
}

impl ImprovApp {
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
    fn formula_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("formula")
            .resizable(true)
            .default_height(140.0)
            .show(ctx, |ui| {
                ui.heading("Formula editor");
                ui.separator();

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

                match self.selected {
                    Some(mid) if self.model.measures.get(&mid).map(|m| m.is_derived()) == Some(true) => {
                        ui.label(format!(
                            "Editing formula for '{}'. Enter a DSL expression (e.g. Price * Quantity).",
                            self.model.measures[&mid].name.0
                        ));
                        ui.text_edit_multiline(&mut self.formula_buf);
                        if ui.button("Commit formula").clicked() {
                            let text = self.formula_buf.clone();
                            match self.commit_formula(mid, &text) {
                                Ok(()) => self.status = "formula updated".into(),
                                Err(e) => self.status = format!("formula error: {e}"),
                            }
                        }
                    }
                    Some(_) => {
                        ui.label("Selected measure is an input; edit its cells in the grid.");
                    }
                    None => {}
                }

                ui.separator();
                ui.label("New derived measure:");
                ui.horizontal(|ui| {
                    ui.label("name");
                    ui.text_edit_singleline(&mut self.new_name);
                });
                ui.horizontal(|ui| {
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

    /// Center: the axis shelf (drag/reassign categories) + page selectors +
    /// the pivot grid for the selected measure.
    fn grid_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| match self.selected {
            None => {
                ui.label("No measures. Open a model store with `improv-gui <db>`.");
            }
            Some(mid) => {
                let name = self.model.measures[&mid].name.0.clone();
                ui.heading(&name);
                self.axis_shelf(ui);
                self.page_selectors(ui);
                ui.separator();
                self.render_grid(ui, mid);
            }
        });
    }

    /// The pivot "axis shelf": three drop zones (Rows / Columns / Pages) each
    /// showing the category chips assigned there. Categories are dragged
    /// between zones (egui 0.29 `dnd_drag_source`/`dnd_drop_zone`); each chip
    /// also has a "->" button that cycles rows->cols->pages for mouse-only use.
    fn axis_shelf(&mut self, ui: &mut egui::Ui) {
        let cat_name = |app: &ImprovApp, c: CategoryId| {
            app.model
                .categories
                .get(&c)
                .map(|x| x.name.0.clone())
                .unwrap_or_else(|| format!("category {}", c.0))
        };
        // Snapshot the per-axis assignment for this frame.
        let row_cat = self.axis_order.first().copied();
        let col_cat = self.axis_order.get(1).copied();
        let page_cats: Vec<CategoryId> = self.axis_order.iter().skip(2).copied().collect();

        // Mutations requested this frame (applied after the borrow ends).
        let mut moves: Vec<(CategoryId, Axis)> = Vec::new();

        let zone = |ui: &mut egui::Ui,
                    app: &ImprovApp,
                    label: &str,
                    axis: Axis,
                    cats: &[CategoryId],
                    moves: &mut Vec<(CategoryId, Axis)>| {
            ui.vertical(|ui| {
                ui.strong(label);
                let frame = egui::Frame::default()
                    .inner_margin(4.0)
                    .stroke(ui.visuals().widgets.noninteractive.bg_stroke);
                let (_, dropped) = ui.dnd_drop_zone::<CategoryId, ()>(frame, |ui| {
                    ui.set_min_size(egui::vec2(120.0, 26.0));
                    ui.horizontal_wrapped(|ui| {
                        if cats.is_empty() {
                            ui.weak("(empty)");
                        }
                        for c in cats {
                            let id = egui::Id::new(("chip", c.0));
                            ui.dnd_drag_source(id, *c, |ui| {
                                ui.label(egui::RichText::new(cat_name(app, *c)).strong());
                            });
                            // Mouse-only fallback: cycle rows->cols->pages.
                            let next = match axis {
                                Axis::Rows => Axis::Columns,
                                Axis::Columns => Axis::Pages,
                                Axis::Pages => Axis::Rows,
                            };
                            if ui.small_button("->").clicked() {
                                moves.push((*c, next));
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
            zone(
                ui,
                self,
                "Rows",
                Axis::Rows,
                &row_cat.into_iter().collect::<Vec<_>>(),
                &mut moves,
            );
            zone(
                ui,
                self,
                "Columns",
                Axis::Columns,
                &col_cat.into_iter().collect::<Vec<_>>(),
                &mut moves,
            );
            zone(ui, self, "Pages", Axis::Pages, &page_cats, &mut moves);
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
        let page_cats: Vec<CategoryId> = self.axis_order.iter().skip(2).copied().collect();
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
        let Some(cat) = self.axis_order.get(2).copied() else {
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
        let (row_cat, col_cat, pinned) = self.resolved_axes();
        let rows = row_cat
            .map(|c| self.sorted_items(c))
            .unwrap_or_else(|| vec![(ItemId(0), String::new())]);
        let cols = col_cat
            .map(|c| self.sorted_items(c))
            .unwrap_or_else(|| vec![(ItemId(0), String::new())]);

        // Edits collected during rendering, applied after the table closure so
        // we don't borrow `self` mutably inside it.
        let mut commit: Option<(CoordKey, String)> = None;
        let mut clicked_derived = false;
        let mut cancel = false;

        use egui_extras::{Column, TableBuilder};
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().resizable(true));
        for _ in &cols {
            table = table.column(Column::auto().resizable(true));
        }
        table
            .header(20.0, |mut header| {
                header.col(|ui| {
                    ui.strong("");
                });
                for (_, cname) in &cols {
                    header.col(|ui| {
                        ui.strong(cname);
                    });
                }
            })
            .body(|mut body| {
                for (ri, (rid, rname)) in rows.iter().enumerate() {
                    body.row(20.0, |mut row| {
                        row.col(|ui| {
                            ui.strong(rname);
                        });
                        for (ci, (cid, _)) in cols.iter().enumerate() {
                            let key = cell_key(row_cat, col_cat, *rid, *cid, &pinned);
                            let is_cursor = cursor == (ri, ci);
                            row.col(|ui| {
                                // Highlight the cursor cell with a tinted frame.
                                let mut frame = egui::Frame::default();
                                if is_cursor {
                                    frame = frame.fill(ui.visuals().selection.bg_fill).stroke(
                                        egui::Stroke::new(
                                            1.0_f32,
                                            ui.visuals().selection.stroke.color,
                                        ),
                                    );
                                }
                                frame.show(ui, |ui| {
                                    if is_derived {
                                        let text = self
                                            .derived_cell_text(measure, &key)
                                            .unwrap_or_default();
                                        if ui.label(text).clicked() {
                                            self.cursor_row = ri;
                                            self.cursor_col = ci;
                                            clicked_derived = true;
                                        }
                                    } else if self.editing.as_ref() == Some(&(measure, key.clone()))
                                    {
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
                }
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

/// The sorted `CoordKey` for a cell, given the row/col categories, this cell's
/// row/col items, and the pinned extra dims.
fn cell_key(
    row_cat: Option<CategoryId>,
    col_cat: Option<CategoryId>,
    rid: ItemId,
    cid: ItemId,
    pinned: &[(CategoryId, ItemId)],
) -> CoordKey {
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    if let Some(c) = row_cat {
        pairs.push((c.0, rid.0));
    }
    if let Some(c) = col_cat {
        pairs.push((c.0, cid.0));
    }
    for (c, i) in pinned {
        pairs.push((c.0, i.0));
    }
    pairs.sort();
    pairs
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
            page_idx,
            axis_for: selected,
            cursor_row: 0,
            cursor_col: 0,
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

        // Move Product (currently on columns) to rows.
        app.set_axis(CategoryId(2), Axis::Rows);
        let (r, c, _) = app.resolved_axes();
        assert_eq!(r, Some(CategoryId(2)), "Product now on rows");
        assert_eq!(c, Some(CategoryId(1)), "Time bumped to columns");
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
}
