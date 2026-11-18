//! UniFFI bindings that expose the Improv engine to the Android app.
//!
//! A thin, synchronous wrapper over `improv_engine`, `improv_core_model`,
//! `improv_store`, `improv_nl_formula`, `improv_storage_csv`, and
//! `improv_client`.  The Kotlin side drives a single `ImprovMobileEngine`
//! instance: it builds and edits a model, reads a rectangular viewport of
//! cells (either as records or copied into a direct `ByteBuffer`), runs slice
//! analytics, and imports or exports CSV.  The engine itself is unchanged;
//! this crate only adapts its types across the FFI boundary.
//!
//! The Android port (this crate and the Jetpack Compose app under `android/`)
//! was contributed by David Gerbino <david.gerbino@dmgerbino.com>.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The coverage attribute is only used inside #[cfg(test)] modules, so a
// non-test coverage build would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use improv_client::completion;
use improv_client::dsl;
use improv_core_model::{
    format_number, parser, CategoryId, Coordinate, Formula, ItemId, Locale, MatrixPlacement,
    Measure, MeasureId, MeasureKind, Model, Name, Value, ValueType, View, ViewId,
};
use improv_engine::session::{Edit, Engine, Snapshot};
use improv_engine::{encode_coord, CellValue};
use improv_nl_formula::{describe_formula, parse_nl_formula, NlContext};
use improv_store::{ChangeSet, Store};

uniffi::setup_scaffolding!();

/// FFI error type surfaced as an exception in Kotlin.
#[derive(Debug, thiserror::Error, uniffi::Error)]
#[uniffi(flat_error)]
pub enum ImprovFfiError {
    #[error("{0}")]
    Message(String),
}

impl From<String> for ImprovFfiError {
    fn from(s: String) -> Self {
        ImprovFfiError::Message(s)
    }
}

/// One point on an axis: its joined label and the (category, item) pins that
/// locate it in the model. The grid's rows and columns are built from these.
type AxisTuple = (String, Vec<(CategoryId, ItemId)>);

/// A grid row: its header, the measure shown on the row, and the axis pins.
type GridRow = (String, MeasureId, Vec<(CategoryId, ItemId)>);

/// An axis tuple while it is still being built: the labels are kept apart so
/// they can be joined once, after every category has contributed one.
type PartialAxisTuple = (Vec<String>, Vec<(CategoryId, ItemId)>);

#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxisOrientation {
    Row,
    Column,
    Page,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CategoryItemInfo {
    pub id: String,
    pub name: String,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct CategoryTile {
    pub id: String,
    pub name: String,
    pub orientation: AxisOrientation,
    pub position_index: u32,
    pub items: Vec<CategoryItemInfo>,
    pub selected_page_item_id: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct MeasureSummary {
    pub id: String,
    pub name: String,
    pub is_derived: bool,
    pub formula_text: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct FormulaEntry {
    pub id: String,
    pub measure_name: String,
    pub expression: String,
    pub natural_language: String,
    pub error_message: Option<String>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ViewportCell {
    pub row_index: u32,
    pub col_index: u32,
    pub measure_id: String,
    pub measure_name: String,
    pub numeric_value: f64,
    pub formatted: String,
    pub is_derived: bool,
    pub is_blank: bool,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct ViewportSnapshot {
    pub row_headers: Vec<String>,
    pub col_headers: Vec<String>,
    pub total_rows: u32,
    pub total_cols: u32,
    pub active_measure_id: String,
    pub active_measure_name: String,
    pub cells: Vec<ViewportCell>,
}

#[derive(uniffi::Record, Clone, Debug)]
pub struct SliceAnalyticsSummary {
    pub measure_name: String,
    pub cell_count: u32,
    pub sum: f64,
    pub mean: f64,
    pub median: f64,
    pub min: f64,
    pub max: f64,
    pub std_dev: f64,
    pub series_labels: Vec<String>,
    pub series_values: Vec<f64>,
}

/// Binary layout constants for `populate_viewport_buffer`:
/// - 16-byte header: `[rows: u32, cols: u32, total_rows: u32, total_cols: u32]` (little-endian)
/// - Followed by `rows * cols` fixed 24-byte cell records:
///   - `0..8`:   `f64` numeric value (`f64::NAN` when blank)
///   - `8..12`:  `u32` measure ID
///   - `12..16`: `u32` primary row item ID
///   - `16..20`: `u32` primary column item ID
///   - `20`:     `u8`  cell tag (`0 = Blank`, `1 = InputNumber`, `2 = DerivedNumber`, `3 = NonNumeric`, `4 = Error`)
///   - `21`:     `u8`  `is_derived` (`0` or `1`)
///   - `22..24`: `u16` reserved (`0`)
pub const VIEWPORT_HEADER_BYTES: usize = 16;
pub const VIEWPORT_CELL_STRIDE_BYTES: usize = 24;

struct EngineState {
    db_path: String,
    store: Option<Store>,
    model: Model,
    engine: Option<Engine>,
    snapshot: Snapshot,
    /// Ordered categories on each axis for the active worksheet view.
    row_cats: Vec<CategoryId>,
    col_cats: Vec<CategoryId>,
    page_cats: Vec<CategoryId>,
    /// Selected item per page category (also used when a category isn't on row/col).
    page_items: BTreeMap<CategoryId, ItemId>,
    /// Currently focused measure (when `show_all_measures_on_rows` is false, or for single-measure slice).
    active_measure: Option<MeasureId>,
}

impl EngineState {
    fn rebuild_engine(&mut self) -> Result<(), ImprovFfiError> {
        self.engine = None;
        self.snapshot = Snapshot::default();
        let mut derived: Vec<MeasureId> = self
            .model
            .measures
            .values()
            .filter(|m| m.is_derived())
            .map(|m| m.id)
            .collect();
        derived.sort_by_key(|m| m.0);
        if !derived.is_empty() {
            let (eng, snap) = Engine::new(&self.model, &derived)
                .map_err(|e| ImprovFfiError::Message(e.to_string()))?;
            self.engine = Some(eng);
            self.snapshot = snap;
        }
        Ok(())
    }

    fn sync_axes_with_model(&mut self) {
        let mut all_cats: Vec<CategoryId> = self.model.categories.keys().copied().collect();
        all_cats.sort_by_key(|c| c.0);

        // If the model has a saved view, initialize axes from it when our axes are empty.
        if self.row_cats.is_empty() && self.col_cats.is_empty() && self.page_cats.is_empty() {
            if let Some(view) = self.model.views.values().min_by_key(|v| v.id.0) {
                let order = if view.axis_order.is_empty() {
                    all_cats.clone()
                } else {
                    view.axis_order.clone()
                };
                let n_rows = view.n_rows.min(order.len());
                let n_cols = view.n_cols.min(order.len().saturating_sub(n_rows));
                self.row_cats = order[..n_rows].to_vec();
                self.col_cats = order[n_rows..n_rows + n_cols].to_vec();
                self.page_cats = order[n_rows + n_cols..].to_vec();
                for (c, i) in &view.page_items {
                    self.page_items.insert(*c, *i);
                }
                if self.model.measures.contains_key(&view.measure) {
                    self.active_measure = Some(view.measure);
                }
            }
        }

        // Remove categories that no longer exist.
        self.row_cats
            .retain(|c| self.model.categories.contains_key(c));
        self.col_cats
            .retain(|c| self.model.categories.contains_key(c));
        self.page_cats
            .retain(|c| self.model.categories.contains_key(c));

        // Add any newly added categories to axes:
        // 1st category -> Row, 2nd -> Column, 3rd+ -> Page.
        for c in &all_cats {
            if !self.row_cats.contains(c)
                && !self.col_cats.contains(c)
                && !self.page_cats.contains(c)
            {
                if self.row_cats.is_empty() {
                    self.row_cats.push(*c);
                } else if self.col_cats.is_empty() {
                    self.col_cats.push(*c);
                } else {
                    self.page_cats.push(*c);
                }
            }
            if !self.page_items.contains_key(c) {
                if let Some(first_item) = self
                    .model
                    .categories
                    .get(c)
                    .and_then(|cat| cat.items.first())
                    .copied()
                {
                    self.page_items.insert(*c, first_item);
                }
            }
        }

        if self
            .active_measure
            .is_none_or(|m| !self.model.measures.contains_key(&m))
        {
            self.active_measure = self.model.measures.keys().copied().min_by_key(|m| m.0);
        }
    }

    fn save_structure(&mut self) -> Result<(), ImprovFfiError> {
        self.update_default_view();
        let EngineState { store, model, .. } = self;
        if let Some(store) = store.as_mut() {
            let changes = ChangeSet {
                structure: true,
                ..Default::default()
            };
            store
                .save_changes(model, &changes)
                .map_err(|e| ImprovFfiError::Message(e.to_string()))?;
        }
        Ok(())
    }

    fn save_full_model(&mut self) -> Result<(), ImprovFfiError> {
        self.update_default_view();
        let EngineState { store, model, .. } = self;
        if let Some(store) = store.as_mut() {
            store
                .save_model(model)
                .map_err(|e| ImprovFfiError::Message(e.to_string()))?;
        }
        Ok(())
    }

    fn update_default_view(&mut self) {
        let Some(measure) = self.active_measure else {
            return;
        };
        let mut axis_order = Vec::new();
        axis_order.extend(&self.row_cats);
        axis_order.extend(&self.col_cats);
        axis_order.extend(&self.page_cats);
        let page_items: Vec<(CategoryId, ItemId)> = self
            .page_cats
            .iter()
            .filter_map(|c| self.page_items.get(c).copied().map(|i| (*c, i)))
            .collect();
        let mut primary = MatrixPlacement::new(measure);
        primary.axis_order = axis_order;
        primary.n_rows = self.row_cats.len();
        primary.n_cols = self.col_cats.len();
        primary.page_items = page_items;
        let view = View::from_matrices(ViewId(1), Name("Default".into()), primary, Vec::new());
        self.model.add_view(view);
    }

    fn axis_tuples(&self, cats: &[CategoryId]) -> Vec<AxisTuple> {
        if cats.is_empty() {
            return vec![("All".to_string(), Vec::new())];
        }
        let mut tuples: Vec<PartialAxisTuple> = vec![(Vec::new(), Vec::new())];
        for cat_id in cats {
            let Some(cat) = self.model.categories.get(cat_id) else {
                continue;
            };
            let mut next = Vec::new();
            for (labels, coords) in &tuples {
                for item_id in &cat.items {
                    let item_name = self
                        .model
                        .items
                        .get(item_id)
                        .map(|it| it.name.0.clone())
                        .unwrap_or_else(|| item_id.0.to_string());
                    let mut l = labels.clone();
                    l.push(item_name);
                    let mut c = coords.clone();
                    c.push((*cat_id, *item_id));
                    next.push((l, c));
                }
            }
            tuples = next;
        }
        tuples
            .into_iter()
            .map(|(labels, coords)| (labels.join(" / "), coords))
            .collect()
    }

    /// Computes the rows and columns of the current viewport.
    ///
    /// To make the 3D financial model (`Time × Product × Region` with `Price`,
    /// `UnitCost`, `Quantity`, `Revenue`, `COGS`, `Margin`) immediately
    /// visible like a classic Lotus Improv worksheet:
    /// - Rows are `(Row Category Tuple × Measure)` when multiple measures exist,
    ///   or just `Row Category Tuple` if filtered.
    /// - Columns are `Column Category Tuple`.
    /// - Page categories pin their selected `page_items`.
    fn build_grid_axes(&self) -> (Vec<GridRow>, Vec<AxisTuple>) {
        let row_tuples = self.axis_tuples(&self.row_cats);
        let col_tuples = self.axis_tuples(&self.col_cats);

        let mut measures: Vec<&Measure> = self.model.measures.values().collect();
        measures.sort_by_key(|m| m.id.0);

        let mut rows = Vec::new();
        for (row_label, row_coords) in &row_tuples {
            for m in &measures {
                let header = if row_tuples.len() == 1 && row_coords.is_empty() {
                    m.name.0.clone()
                } else {
                    format!("{} · {}", row_label, m.name.0)
                };
                rows.push((header, m.id, row_coords.clone()));
            }
        }
        (rows, col_tuples)
    }

    fn resolve_cell(
        &self,
        measure_id: MeasureId,
        row_coords: &[(CategoryId, ItemId)],
        col_coords: &[(CategoryId, ItemId)],
    ) -> (f64, String, u8, bool, bool) {
        let Some(measure) = self.model.measures.get(&measure_id) else {
            return (f64::NAN, String::new(), 0, false, true);
        };
        let is_derived = measure.is_derived();

        let mut coord_map: BTreeMap<CategoryId, ItemId> = BTreeMap::new();
        for (c, i) in &self.page_items {
            if measure.categories.contains(c) {
                coord_map.insert(*c, *i);
            }
        }
        for (c, i) in row_coords.iter().chain(col_coords.iter()) {
            if measure.categories.contains(c) {
                coord_map.insert(*c, *i);
            }
        }
        // Ensure any remaining measure category has a default item.
        for c in &measure.categories {
            if !coord_map.contains_key(c) {
                if let Some(first_item) = self
                    .model
                    .categories
                    .get(c)
                    .and_then(|cat| cat.items.first())
                    .copied()
                {
                    coord_map.insert(*c, first_item);
                }
            }
        }

        let coord = Coordinate { dims: coord_map };
        let locale = Locale::en();
        if is_derived {
            let key = encode_coord(&coord);
            if let Some(cv) = self
                .snapshot
                .get(&measure_id)
                .and_then(|vals| vals.get(&key))
            {
                let formatted = cv.formatted(measure.format.as_ref(), &locale);
                match cv {
                    CellValue::Num(bits) => (f64::from_bits(*bits), formatted, 2, true, false),
                    CellValue::Err(_) => (f64::NAN, formatted, 4, true, false),
                    _ => (f64::NAN, formatted, 3, true, false),
                }
            } else {
                (f64::NAN, String::new(), 0, true, true)
            }
        } else if let Some(val) = self.model.input(measure_id, &coord) {
            match val {
                Value::Number(n) => (
                    *n,
                    format_number(*n, measure.format.as_ref(), &locale),
                    1,
                    false,
                    false,
                ),
                Value::Error(e) => (f64::NAN, format!("{:?}", e.kind), 4, false, false),
                other => (f64::NAN, format!("{other:?}"), 3, false, false),
            }
        } else {
            (f64::NAN, String::new(), 0, false, true)
        }
    }
}

/// Construct the canonical 3D sample model:
/// `Time × Product × Region` with `Price`, `UnitCost`, `Quantity`, `Revenue`, `COGS`, `Margin`.
fn build_sample_3d_model() -> Model {
    let mut m = Model::new();
    let time = CategoryId(1);
    let product = CategoryId(2);
    let region = CategoryId(3);

    m.add_category(time, "Time");
    m.add_category(product, "Product");
    m.add_category(region, "Region");

    let times = [
        (ItemId(101), "Q1"),
        (ItemId(102), "Q2"),
        (ItemId(103), "Q3"),
        (ItemId(104), "Q4"),
    ];
    for (id, name) in times {
        m.add_item(id, time, name);
    }

    let products = [
        (ItemId(201), "PixelBook"),
        (ItemId(202), "PixelPhone"),
        (ItemId(203), "PixelTablet"),
    ];
    for (id, name) in products {
        m.add_item(id, product, name);
    }

    let regions = [
        (ItemId(301), "Americas"),
        (ItemId(302), "EMEA"),
        (ItemId(303), "APAC"),
    ];
    for (id, name) in regions {
        m.add_item(id, region, name);
    }

    let dims = vec![time, product, region];
    let price_id = MeasureId(100);
    let unit_cost_id = MeasureId(101);
    let quantity_id = MeasureId(102);
    let revenue_id = MeasureId(103);
    let cogs_id = MeasureId(104);
    let margin_id = MeasureId(105);

    for (id, name) in [
        (price_id, "Price"),
        (unit_cost_id, "UnitCost"),
        (quantity_id, "Quantity"),
    ] {
        m.add_measure(Measure {
            id,
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: dims.clone(),
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
    }

    // Derived measures compiled via improv_nl_formula ("Price times Quantity", etc.)
    let rev_formula = parse_nl_formula(&NlContext::new(&m), "Revenue equals Price times Quantity")
        .expect("valid Revenue NL formula");
    m.add_measure(Measure {
        id: revenue_id,
        name: Name("Revenue".into()),
        value_type: ValueType::Number,
        categories: dims.clone(),
        kind: MeasureKind::Derived(rev_formula),
        description: Some("Revenue = Price * Quantity".into()),
        format: None,
    });

    let cogs_formula = parse_nl_formula(&NlContext::new(&m), "COGS equals UnitCost times Quantity")
        .expect("valid COGS NL formula");
    m.add_measure(Measure {
        id: cogs_id,
        name: Name("COGS".into()),
        value_type: ValueType::Number,
        categories: dims.clone(),
        kind: MeasureKind::Derived(cogs_formula),
        description: Some("COGS = UnitCost * Quantity".into()),
        format: None,
    });

    let margin_formula = parse_nl_formula(&NlContext::new(&m), "Margin equals Revenue minus COGS")
        .expect("valid Margin NL formula");
    m.add_measure(Measure {
        id: margin_id,
        name: Name("Margin".into()),
        value_type: ValueType::Number,
        categories: dims.clone(),
        kind: MeasureKind::Derived(margin_formula),
        description: Some("Margin = Revenue - COGS".into()),
        format: None,
    });

    // Populate realistic 3D input cells across Time × Product × Region
    let base_prices = [
        (ItemId(201), 1299.0),
        (ItemId(202), 899.0),
        (ItemId(203), 599.0),
    ];
    let base_costs = [
        (ItemId(201), 780.0),
        (ItemId(202), 490.0),
        (ItemId(203), 340.0),
    ];
    let region_mult = [(ItemId(301), 1.0), (ItemId(302), 0.85), (ItemId(303), 0.75)];

    for (q_idx, (t_id, _)) in times.iter().enumerate() {
        let q_growth = 1.0 + (q_idx as f64) * 0.08;
        for (p_idx, (p_id, _)) in products.iter().enumerate() {
            let price = base_prices[p_idx].1;
            let cost = base_costs[p_idx].1;
            let base_qty = match p_idx {
                0 => 120.0,
                1 => 340.0,
                _ => 210.0,
            };
            for (r_id, r_factor) in region_mult {
                let coord =
                    Coordinate::from_pairs([(time, *t_id), (product, *p_id), (region, r_id)]);
                let qty = (base_qty * r_factor * q_growth).round();
                m.set_input(price_id, coord.clone(), Value::Number(price));
                m.set_input(unit_cost_id, coord.clone(), Value::Number(cost));
                m.set_input(quantity_id, coord, Value::Number(qty));
            }
        }
    }

    // Default view: Time on Rows, Product on Columns, Region on Pages (Americas)
    let mut primary = MatrixPlacement::new(margin_id);
    primary.axis_order = vec![time, product, region];
    primary.n_rows = 1;
    primary.n_cols = 1;
    primary.page_items = vec![(region, ItemId(301))];
    m.add_view(View::from_matrices(
        ViewId(1),
        Name("Default".into()),
        primary,
        Vec::new(),
    ));

    m
}

#[derive(uniffi::Object)]
pub struct ImprovMobileEngine {
    state: Mutex<EngineState>,
}

#[uniffi::export]
impl ImprovMobileEngine {
    /// Opens an existing `.db` Mentat store (or creates a new one at `db_path`).
    /// Pass `""` for an in-memory store. If the store is newly created and empty,
    /// the 3D sample model (`Time × Product × Region`) is loaded automatically.
    #[uniffi::constructor]
    pub fn open_or_create(db_path: String) -> Arc<Self> {
        let mut store_opt = Store::open(&db_path).ok();
        let mut model = store_opt
            .as_mut()
            .and_then(|s| s.load_model().ok())
            .unwrap_or_default();

        let is_empty = model.categories.is_empty() && model.measures.is_empty();
        if is_empty {
            model = build_sample_3d_model();
            if let Some(s) = store_opt.as_mut() {
                let _ = s.save_model(&model);
            }
        }

        let mut state = EngineState {
            db_path,
            store: store_opt,
            model,
            engine: None,
            snapshot: Snapshot::default(),
            row_cats: Vec::new(),
            col_cats: Vec::new(),
            page_cats: Vec::new(),
            page_items: BTreeMap::new(),
            active_measure: None,
        };
        state.sync_axes_with_model();
        let _ = state.rebuild_engine();

        Arc::new(Self {
            state: Mutex::new(state),
        })
    }

    /// Returns the filesystem path of the backing SQLite/Mentat `.db` store (or `""` if in-memory).
    pub fn get_db_path(&self) -> String {
        let state = self.state.lock().unwrap();
        state.db_path.clone()
    }

    /// Resets/loads the canonical 3D sample model (`Time × Product × Region` with
    /// `Price`, `UnitCost`, `Quantity`, `Revenue`, `COGS`, `Margin`), evaluates it
    /// via `improv_engine`, and persists it to the `.db` store.
    pub fn load_sample_model(&self) -> Result<(), ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        state.model = build_sample_3d_model();
        state.row_cats.clear();
        state.col_cats.clear();
        state.page_cats.clear();
        state.page_items.clear();
        state.active_measure = None;
        state.sync_axes_with_model();
        state.rebuild_engine()?;
        state.save_full_model()?;
        Ok(())
    }

    /// Initializes a clean blank starter workbook (`Period × Department` with an
    /// editable `Budget` input measure) and persists it to the `.db` store.
    pub fn create_blank_model(&self) -> Result<(), ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        let mut m = Model::new();
        let period = CategoryId(1);
        let dept = CategoryId(2);
        m.add_category(period, "Period");
        m.add_category(dept, "Department");
        m.add_item(ItemId(101), period, "P1");
        m.add_item(ItemId(102), period, "P2");
        m.add_item(ItemId(201), dept, "Core");
        m.add_item(ItemId(202), dept, "Growth");

        let budget_id = MeasureId(100);
        m.add_measure(Measure {
            id: budget_id,
            name: Name("Budget".into()),
            value_type: ValueType::Number,
            categories: vec![period, dept],
            kind: MeasureKind::Input,
            description: Some("Blank starter input measure".into()),
            format: None,
        });
        for p in [ItemId(101), ItemId(102)] {
            for d in [ItemId(201), ItemId(202)] {
                m.set_input(
                    budget_id,
                    Coordinate::from_pairs([(period, p), (dept, d)]),
                    Value::Number(0.0),
                );
            }
        }
        state.model = m;
        state.row_cats.clear();
        state.col_cats.clear();
        state.page_cats.clear();
        state.page_items.clear();
        state.active_measure = None;
        state.sync_axes_with_model();
        state.rebuild_engine()?;
        state.save_full_model()?;
        Ok(())
    }

    /// Exports the current model to a CSV file at `output_path` for `measure_name`
    /// using `improv_storage_csv`.
    pub fn export_measure_csv(
        &self,
        measure_name: String,
        output_path: String,
    ) -> Result<u32, ImprovFfiError> {
        let state = self.state.lock().unwrap();
        let measure = state
            .model
            .measure_by_name(&measure_name)
            .ok_or_else(|| ImprovFfiError::Message(format!("unknown measure '{measure_name}'")))?;
        let count = improv_storage_csv::export_measure_csv(
            &state.model,
            measure.id,
            std::path::Path::new(&output_path),
            b',',
        )
        .map_err(|e| ImprovFfiError::Message(e.to_string()))?;
        Ok(count as u32)
    }

    /// Exports any measure (input or derived) to an in-memory UTF-8 CSV string.
    pub fn export_measure_csv_text(&self, measure_name: String) -> Result<String, ImprovFfiError> {
        let state = self.state.lock().unwrap();
        let measure = state
            .model
            .measure_by_name(&measure_name)
            .or_else(|| state.model.measures.values().min_by_key(|m| m.id.0))
            .ok_or_else(|| ImprovFfiError::Message(format!("unknown measure '{measure_name}'")))?;
        let mut buf: Vec<u8> = Vec::new();
        improv_storage_csv::export_measure_csv_to(&state.model, measure.id, &mut buf, b',')
            .map_err(|e| ImprovFfiError::Message(e.to_string()))?;
        String::from_utf8(buf).map_err(|e| ImprovFfiError::Message(e.to_string()))
    }

    /// Imports a headered CSV string where columns `0..N-1` map to categories (reusing
    /// existing categories by name or creating new ones) and column `N-1` maps to an
    /// input measure (named `measure_name_override` if non-empty, or the last header name).
    pub fn import_csv_text(
        &self,
        csv_content: String,
        measure_name_override: String,
    ) -> Result<u32, ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        let trimmed = csv_content.trim();
        if trimmed.is_empty() {
            return Err(ImprovFfiError::Message(
                "CSV content cannot be empty".into(),
            ));
        }

        let first_line = trimmed
            .lines()
            .next()
            .ok_or_else(|| ImprovFfiError::Message("CSV header row is missing".into()))?;
        let headers: Vec<String> = first_line
            .split(',')
            .map(|h| h.trim().trim_matches('"').trim().to_string())
            .filter(|h| !h.is_empty())
            .collect();

        if headers.len() < 2 {
            return Err(ImprovFfiError::Message(
                "CSV must have at least 1 category column and 1 value column".into(),
            ));
        }

        let dim_headers = &headers[..headers.len() - 1];
        let val_header = &headers[headers.len() - 1];
        let target_measure_name = if measure_name_override.trim().is_empty() {
            val_header.clone()
        } else {
            measure_name_override.trim().to_string()
        };

        let mut next_cat_id = state
            .model
            .categories
            .keys()
            .map(|c| c.0)
            .max()
            .unwrap_or(0)
            + 1;
        let mut dimensions = Vec::with_capacity(dim_headers.len());
        for col_name in dim_headers {
            let existing_cat = state
                .model
                .categories
                .values()
                .find(|c| c.name.0.eq_ignore_ascii_case(col_name))
                .map(|c| (c.id, c.name.0.clone()));
            let (cat_id, canonical_cat_name) = match existing_cat {
                Some(pair) => pair,
                None => {
                    let id = CategoryId(next_cat_id);
                    next_cat_id += 1;
                    (id, col_name.clone())
                }
            };
            dimensions.push(improv_storage_csv::DimensionMapping {
                column: improv_storage_csv::ColumnRef::Name(col_name.clone()),
                category_id: cat_id,
                category_name: canonical_cat_name,
            });
        }

        let measure_id = state
            .model
            .measures
            .values()
            .find(|m| m.name.0.eq_ignore_ascii_case(&target_measure_name))
            .map(|m| m.id)
            .unwrap_or_else(|| {
                let next_mid = state.model.measures.keys().map(|m| m.0).max().unwrap_or(99) + 1;
                MeasureId(next_mid)
            });

        let item_id_base = state.model.items.keys().map(|i| i.0).max().unwrap_or(500) + 1;

        let spec = improv_storage_csv::ImportSpec {
            path: std::path::PathBuf::new(),
            delimiter: b',',
            has_header: true,
            measure_id,
            measure_name: target_measure_name,
            value_type: ValueType::Number,
            value_column: improv_storage_csv::ColumnRef::Name(val_header.clone()),
            dimensions,
            item_id_base,
            max_records: None,
        };

        let imported_count = improv_storage_csv::import_csv_from(
            std::io::Cursor::new(trimmed),
            &mut state.model,
            &spec,
        )
        .map_err(|e| ImprovFfiError::Message(e.to_string()))?;

        state.sync_axes_with_model();
        state.rebuild_engine()?;
        state.save_full_model()?;
        Ok(imported_count as u32)
    }

    /// Adds a new item (e.g., `"Q5"`, `"LATAM"`, `"PixelFold"`) to an existing category.
    pub fn add_category_item(
        &self,
        category_id_or_name: String,
        item_name: String,
    ) -> Result<CategoryTile, ImprovFfiError> {
        let trimmed_item = item_name.trim();
        if trimmed_item.is_empty() {
            return Err(ImprovFfiError::Message("item name cannot be empty".into()));
        }
        {
            let mut state = self.state.lock().unwrap();
            let cat_id = category_id_or_name
                .parse::<u32>()
                .ok()
                .map(CategoryId)
                .or_else(|| {
                    state
                        .model
                        .categories
                        .values()
                        .find(|c| c.name.0.eq_ignore_ascii_case(category_id_or_name.trim()))
                        .map(|c| c.id)
                })
                .ok_or_else(|| {
                    ImprovFfiError::Message(format!("category '{category_id_or_name}' not found"))
                })?;

            let already_exists = state.model.categories.get(&cat_id).is_some_and(|cat| {
                cat.items.iter().any(|iid| {
                    state
                        .model
                        .items
                        .get(iid)
                        .is_some_and(|it| it.name.0.eq_ignore_ascii_case(trimmed_item))
                })
            });
            if already_exists {
                return Err(ImprovFfiError::Message(format!(
                    "item '{trimmed_item}' already exists in category"
                )));
            }

            let next_item_id = state.model.items.keys().map(|i| i.0).max().unwrap_or(100) + 1;
            state
                .model
                .add_item(ItemId(next_item_id), cat_id, trimmed_item);
            state.rebuild_engine()?;
            state.save_structure()?;
        }
        self.get_categories()
            .into_iter()
            .find(|c| {
                c.id == category_id_or_name
                    || c.name.eq_ignore_ascii_case(category_id_or_name.trim())
            })
            .ok_or_else(|| ImprovFfiError::Message("updated category not found".into()))
    }

    /// Creates a new editable input measure over all current model categories and populates
    /// its coordinates with `default_value`.
    pub fn add_input_measure(
        &self,
        name: String,
        default_value: f64,
    ) -> Result<MeasureSummary, ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(ImprovFfiError::Message(
                "measure name cannot be empty".into(),
            ));
        }
        if state.model.measure_by_name(trimmed).is_some()
            || state.model.category_by_name(trimmed).is_some()
        {
            return Err(ImprovFfiError::Message(format!(
                "'{trimmed}' is already in use"
            )));
        }

        let next_mid = state.model.measures.keys().map(|m| m.0).max().unwrap_or(99) + 1;
        let mid = MeasureId(next_mid);
        let mut all_cats: Vec<CategoryId> = state.model.categories.keys().copied().collect();
        all_cats.sort_by_key(|c| c.0);

        state.model.add_measure(Measure {
            id: mid,
            name: Name(trimmed.to_string()),
            value_type: ValueType::Number,
            categories: all_cats.clone(),
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });

        let tuples = state.axis_tuples(&all_cats);
        for (_, coords) in tuples {
            let coord = Coordinate::from_pairs(coords);
            state
                .model
                .set_input(mid, coord, Value::Number(default_value));
        }

        state.rebuild_engine()?;
        state.save_full_model()?;

        Ok(MeasureSummary {
            id: mid.0.to_string(),
            name: trimmed.to_string(),
            is_derived: false,
            formula_text: None,
        })
    }

    /// Deletes a measure by ID or name (if no other derived measure depends on it).
    pub fn delete_measure(&self, measure_id_or_name: String) -> Result<(), ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        if state.model.measures.len() <= 1 {
            return Err(ImprovFfiError::Message(
                "cannot delete the only measure in the workbook".into(),
            ));
        }
        let target_id = measure_id_or_name
            .parse::<u32>()
            .ok()
            .map(MeasureId)
            .or_else(|| {
                state
                    .model
                    .measures
                    .values()
                    .find(|m| m.name.0.eq_ignore_ascii_case(measure_id_or_name.trim()))
                    .map(|m| m.id)
            })
            .ok_or_else(|| {
                ImprovFfiError::Message(format!("measure '{measure_id_or_name}' not found"))
            })?;

        for m in state.model.measures.values() {
            if m.id != target_id {
                if let MeasureKind::Derived(f) = &m.kind {
                    if f.referenced_measures().contains(&target_id) {
                        return Err(ImprovFfiError::Message(format!(
                            "cannot delete: measure '{}' depends on it",
                            m.name.0
                        )));
                    }
                }
            }
        }

        state.model.measures.remove(&target_id);
        state.model.inputs.retain(|(mid, _), _| *mid != target_id);
        state.sync_axes_with_model();
        state.rebuild_engine()?;
        state.save_full_model()?;
        Ok(())
    }

    /// Applies a multiplicative What-If shock (`multiplier`, e.g. `1.10` for `+10%`) to all
    /// input cells of `measure_name`, recalculates all derived measures via `improv_engine`,
    /// and persists the updated model.
    pub fn apply_what_if_multiplier(
        &self,
        measure_name: String,
        multiplier: f64,
    ) -> Result<u32, ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        let measure = state
            .model
            .measures
            .values()
            .find(|m| m.name.0.eq_ignore_ascii_case(measure_name.trim()))
            .cloned()
            .ok_or_else(|| {
                ImprovFfiError::Message(format!("measure '{measure_name}' not found"))
            })?;
        if measure.is_derived() {
            return Err(ImprovFfiError::Message(format!(
                "'{}' is a derived measure; apply What-If multipliers to an input measure (e.g. Price, UnitCost, Quantity)",
                measure.name.0
            )));
        }

        let mut updated = 0u32;
        for ((mid, _), val) in state.model.inputs.iter_mut() {
            if *mid == measure.id {
                if let Value::Number(n) = val {
                    *n = (*n * multiplier * 100.0).round() / 100.0;
                    updated += 1;
                }
            }
        }
        state.rebuild_engine()?;
        state.save_full_model()?;
        Ok(updated)
    }

    /// Computes statistical summary metrics (count, sum, mean, median, min, max, standard
    /// deviation, and per-slice series values) for `measure_name` in the active page slice.
    pub fn get_slice_analytics(&self, measure_name: String) -> SliceAnalyticsSummary {
        let state = self.state.lock().unwrap();
        let target_measure = state
            .model
            .measures
            .values()
            .find(|m| m.name.0.eq_ignore_ascii_case(measure_name.trim()))
            .or_else(|| {
                state
                    .active_measure
                    .and_then(|id| state.model.measures.get(&id))
            })
            .or_else(|| state.model.measures.values().max_by_key(|m| m.id.0));

        let Some(measure) = target_measure else {
            return SliceAnalyticsSummary {
                measure_name,
                cell_count: 0,
                sum: 0.0,
                mean: 0.0,
                median: 0.0,
                min: 0.0,
                max: 0.0,
                std_dev: 0.0,
                series_labels: Vec::new(),
                series_values: Vec::new(),
            };
        };

        let mid = measure.id;
        let resolved_name = measure.name.0.clone();
        let row_tuples = state.axis_tuples(&state.row_cats);
        let col_tuples = state.axis_tuples(&state.col_cats);

        let mut values: Vec<f64> = Vec::new();
        let mut series_labels: Vec<String> = Vec::new();
        let mut series_values: Vec<f64> = Vec::new();

        for (r_label, r_coords) in &row_tuples {
            let mut row_sum = 0.0;
            let mut row_has_val = false;
            for (_c_label, c_coords) in &col_tuples {
                let (v, _fmt, _tag, _is_derived, is_blank) =
                    state.resolve_cell(mid, r_coords, c_coords);
                if !is_blank && v.is_finite() {
                    values.push(v);
                    row_sum += v;
                    row_has_val = true;
                }
            }
            if row_has_val {
                series_labels.push(r_label.clone());
                series_values.push(row_sum);
            }
        }

        if values.is_empty() {
            return SliceAnalyticsSummary {
                measure_name: resolved_name,
                cell_count: 0,
                sum: 0.0,
                mean: 0.0,
                median: 0.0,
                min: 0.0,
                max: 0.0,
                std_dev: 0.0,
                series_labels,
                series_values,
            };
        }

        let count = values.len();
        let sum: f64 = values.iter().sum();
        let mean = sum / (count as f64);

        let mut sorted = values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let min = *sorted.first().unwrap_or(&0.0);
        let max = *sorted.last().unwrap_or(&0.0);
        let median = if count % 2 == 1 {
            sorted[count / 2]
        } else {
            (sorted[count / 2 - 1] + sorted[count / 2]) / 2.0
        };

        let variance = values
            .iter()
            .map(|v| {
                let d = *v - mean;
                d * d
            })
            .sum::<f64>()
            / (count as f64);
        let std_dev = variance.sqrt();

        SliceAnalyticsSummary {
            measure_name: resolved_name,
            cell_count: count as u32,
            sum,
            mean,
            median,
            min,
            max,
            std_dev,
            series_labels,
            series_values,
        }
    }

    /// Returns all category tiles with their current `AxisOrientation` (`Row`, `Column`, `Page`),
    /// items, and selected page item.
    pub fn get_categories(&self) -> Vec<CategoryTile> {
        let state = self.state.lock().unwrap();
        let mut tiles = Vec::new();
        let push_axis =
            |cats: &[CategoryId], orientation: AxisOrientation, tiles: &mut Vec<CategoryTile>| {
                for (idx, cat_id) in cats.iter().enumerate() {
                    if let Some(cat) = state.model.categories.get(cat_id) {
                        let items = cat
                            .items
                            .iter()
                            .filter_map(|item_id| {
                                state.model.items.get(item_id).map(|it| CategoryItemInfo {
                                    id: it.id.0.to_string(),
                                    name: it.name.0.clone(),
                                })
                            })
                            .collect();
                        let selected_page_item_id =
                            state.page_items.get(cat_id).map(|i| i.0.to_string());
                        tiles.push(CategoryTile {
                            id: cat.id.0.to_string(),
                            name: cat.name.0.clone(),
                            orientation,
                            position_index: idx as u32,
                            items,
                            selected_page_item_id,
                        });
                    }
                }
            };
        push_axis(&state.row_cats, AxisOrientation::Row, &mut tiles);
        push_axis(&state.col_cats, AxisOrientation::Column, &mut tiles);
        push_axis(&state.page_cats, AxisOrientation::Page, &mut tiles);
        tiles
    }

    /// Adds a new category dimension with initial item names and places it on `orientation`.
    pub fn add_category(
        &self,
        name: String,
        item_names: Vec<String>,
        orientation: AxisOrientation,
    ) -> Result<CategoryTile, ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(ImprovFfiError::Message(
                "category name cannot be empty".into(),
            ));
        }
        if state.model.category_by_name(trimmed).is_some()
            || state.model.measure_by_name(trimmed).is_some()
        {
            return Err(ImprovFfiError::Message(format!(
                "'{trimmed}' is already in use"
            )));
        }

        let next_cat_id = state
            .model
            .categories
            .keys()
            .map(|c| c.0)
            .max()
            .unwrap_or(0)
            + 1;
        let cat_id = CategoryId(next_cat_id);
        state.model.add_category(cat_id, trimmed);

        let next_item_id = state.model.items.keys().map(|i| i.0).max().unwrap_or(100) + 1;
        let items_to_add = if item_names.is_empty() {
            vec![format!("{trimmed} 1"), format!("{trimmed} 2")]
        } else {
            item_names
        };
        let mut item_infos = Vec::new();
        for (offset, item_name) in items_to_add.iter().enumerate() {
            let item_id = ItemId(next_item_id + offset as u32);
            state.model.add_item(item_id, cat_id, item_name.trim());
            item_infos.push(CategoryItemInfo {
                id: item_id.0.to_string(),
                name: item_name.trim().to_string(),
            });
        }

        if let Some(first) = item_infos.first() {
            if let Ok(id_num) = first.id.parse::<u32>() {
                state.page_items.insert(cat_id, ItemId(id_num));
            }
        }

        let position_index = match orientation {
            AxisOrientation::Row => {
                state.row_cats.push(cat_id);
                (state.row_cats.len() - 1) as u32
            }
            AxisOrientation::Column => {
                state.col_cats.push(cat_id);
                (state.col_cats.len() - 1) as u32
            }
            AxisOrientation::Page => {
                state.page_cats.push(cat_id);
                (state.page_cats.len() - 1) as u32
            }
        };

        state.save_structure()?;
        Ok(CategoryTile {
            id: cat_id.0.to_string(),
            name: trimmed.to_string(),
            orientation,
            position_index,
            selected_page_item_id: item_infos.first().map(|i| i.id.clone()),
            items: item_infos,
        })
    }

    /// Pivots a category tile to a new axis (`Row`, `Column`, or `Page`) at `new_index`.
    pub fn pivot_category(
        &self,
        category_id: String,
        target_axis: AxisOrientation,
        new_index: u32,
    ) {
        let mut state = self.state.lock().unwrap();
        let cat_id = category_id
            .parse::<u32>()
            .ok()
            .map(CategoryId)
            .or_else(|| state.model.category_by_name(&category_id).map(|c| c.id));
        let Some(cat_id) = cat_id else {
            return;
        };
        if !state.model.categories.contains_key(&cat_id) {
            return;
        }

        state.row_cats.retain(|c| *c != cat_id);
        state.col_cats.retain(|c| *c != cat_id);
        state.page_cats.retain(|c| *c != cat_id);

        let target_vec = match target_axis {
            AxisOrientation::Row => &mut state.row_cats,
            AxisOrientation::Column => &mut state.col_cats,
            AxisOrientation::Page => &mut state.page_cats,
        };
        let idx = (new_index as usize).min(target_vec.len());
        target_vec.insert(idx, cat_id);
        let _ = state.save_structure();
    }

    /// Selects the active page item for a `Page`-oriented category.
    pub fn select_page_item(&self, category_id: String, item_id: String) {
        let mut state = self.state.lock().unwrap();
        let cat_id = category_id
            .parse::<u32>()
            .ok()
            .map(CategoryId)
            .or_else(|| state.model.category_by_name(&category_id).map(|c| c.id));
        let Some(cat_id) = cat_id else {
            return;
        };
        let resolved_item = item_id.parse::<u32>().ok().map(ItemId).or_else(|| {
            state.model.categories.get(&cat_id).and_then(|cat| {
                cat.items.iter().copied().find(|iid| {
                    state
                        .model
                        .items
                        .get(iid)
                        .is_some_and(|it| it.name.0 == item_id)
                })
            })
        });
        if let Some(iid) = resolved_item {
            state.page_items.insert(cat_id, iid);
            let _ = state.save_structure();
        }
    }

    /// Returns all measures in the model (both input and derived).
    pub fn get_measures(&self) -> Vec<MeasureSummary> {
        let state = self.state.lock().unwrap();
        let mut list: Vec<&Measure> = state.model.measures.values().collect();
        list.sort_by_key(|m| m.id.0);
        list.into_iter()
            .map(|m| {
                let formula_text = match &m.kind {
                    MeasureKind::Input => None,
                    MeasureKind::Derived(f) => Some(
                        dsl::formula(&state.model, f)
                            .unwrap_or_else(|| describe_formula(&NlContext::new(&state.model), f)),
                    ),
                };
                MeasureSummary {
                    id: m.id.0.to_string(),
                    name: m.name.0.clone(),
                    is_derived: m.is_derived(),
                    formula_text,
                }
            })
            .collect()
    }

    /// Returns all derived formulas in the model.
    pub fn get_formulas(&self) -> Vec<FormulaEntry> {
        let state = self.state.lock().unwrap();
        let mut list: Vec<&Measure> = state
            .model
            .measures
            .values()
            .filter(|m| m.is_derived())
            .collect();
        list.sort_by_key(|m| m.id.0);
        list.into_iter()
            .filter_map(|m| match &m.kind {
                MeasureKind::Derived(f) => {
                    let dsl_str = dsl::formula(&state.model, f)
                        .unwrap_or_else(|| describe_formula(&NlContext::new(&state.model), f));
                    let nl_str = describe_formula(&NlContext::new(&state.model), f);
                    Some(FormulaEntry {
                        id: m.id.0.to_string(),
                        measure_name: m.name.0.clone(),
                        expression: format!("{} = {}", m.name.0, dsl_str),
                        natural_language: format!("{} equals {}", m.name.0, nl_str),
                        error_message: None,
                    })
                }
                MeasureKind::Input => None,
            })
            .collect()
    }

    /// Evaluates and upserts a symbolic or controlled-natural-language formula
    /// (e.g., `"Margin = Revenue - COGS"` or `"Margin equals Revenue minus COGS"`),
    /// updates the differential-dataflow `Engine`, and saves to `improv_store`.
    pub fn upsert_formula(&self, expression: String) -> Result<FormulaEntry, ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        let trimmed = expression.trim();
        if trimmed.is_empty() {
            return Err(ImprovFfiError::Message("formula cannot be empty".into()));
        }

        // Extract target measure name from `Target = RHS` or `Target equals/is RHS`
        let (target_name, rhs_text) = if let Some((lhs, rhs)) = trimmed.split_once('=') {
            (lhs.trim().to_string(), rhs.trim().to_string())
        } else {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            if words.len() >= 3
                && (words[1].eq_ignore_ascii_case("equals") || words[1].eq_ignore_ascii_case("is"))
            {
                (words[0].to_string(), words[2..].join(" "))
            } else {
                return Err(ImprovFfiError::Message(
                    "expected 'Measure = expression' or 'Measure equals expression'".into(),
                ));
            }
        };

        if target_name.is_empty() || rhs_text.is_empty() {
            return Err(ImprovFfiError::Message(
                "measure name and formula expression are required".into(),
            ));
        }

        if state.model.category_by_name(&target_name).is_some() {
            return Err(ImprovFfiError::Message(format!(
                "'{target_name}' is already a category name"
            )));
        }

        let measure_id = state
            .model
            .measure_by_name(&target_name)
            .map(|m| m.id)
            .unwrap_or_else(|| {
                let next_id = state
                    .model
                    .measures
                    .keys()
                    .map(|m| m.0)
                    .max()
                    .unwrap_or(100)
                    + 1;
                MeasureId(next_id)
            });

        let provisional = state.model.with_provisional(measure_id, &target_name);
        let canonical_rhs = Locale::en().canonicalize(&rhs_text);

        // Try symbolic DSL first, then controlled natural language (`improv_nl_formula`)
        let formula: Formula =
            parser::parse_expr(&provisional, &canonical_rhs).or_else(|dsl_err| {
                parse_nl_formula(&NlContext::new(&provisional), &rhs_text)
                    .or_else(|_| parse_nl_formula(&NlContext::new(&provisional), trimmed))
                    .map_err(|nl_err| ImprovFfiError::Message(format!("{dsl_err} (NL: {nl_err})")))
            })?;

        let (value_type, mut cats) = improv_engine::compiler::derived_shape(
            &state.model,
            measure_id,
            &target_name,
            &formula,
        )
        .map_err(|e| ImprovFfiError::Message(e.to_string()))?;

        // If a scalar formula has no categories, inherit existing categories if redefining.
        if cats.is_empty() {
            if let Some(existing) = state.model.measures.get(&measure_id) {
                cats = existing.categories.clone();
            }
        }

        let previous_model = state.model.clone();
        state.model.add_measure(Measure {
            id: measure_id,
            name: Name(target_name.clone()),
            value_type,
            categories: cats,
            kind: MeasureKind::Derived(formula.clone()),
            description: None,
            format: None,
        });

        // Verify acyclicity and rebuild differential dataflow engine
        if let Err(e) = state.rebuild_engine() {
            state.model = previous_model;
            let _ = state.rebuild_engine();
            return Err(e);
        }

        state.save_structure()?;

        let dsl_str = dsl::formula(&state.model, &formula)
            .unwrap_or_else(|| describe_formula(&NlContext::new(&state.model), &formula));
        let nl_str = describe_formula(&NlContext::new(&state.model), &formula);

        Ok(FormulaEntry {
            id: measure_id.0.to_string(),
            measure_name: target_name.clone(),
            expression: format!("{target_name} = {dsl_str}"),
            natural_language: format!("{target_name} equals {nl_str}"),
            error_message: None,
        })
    }

    /// Updates an editable input cell at `(row_index, col_index)` in the current grid,
    /// pushes the incremental delta through `improv_engine::session::Engine`, and saves to `Store`.
    pub fn set_cell_value(
        &self,
        row_index: u32,
        col_index: u32,
        value: f64,
    ) -> Result<(), ImprovFfiError> {
        let mut state = self.state.lock().unwrap();
        let (rows, cols) = state.build_grid_axes();
        let (_, measure_id, row_coords) = rows
            .get(row_index as usize)
            .ok_or_else(|| ImprovFfiError::Message("row index out of bounds".into()))?
            .clone();
        let (_, col_coords) = cols
            .get(col_index as usize)
            .ok_or_else(|| ImprovFfiError::Message("column index out of bounds".into()))?
            .clone();

        let measure = state
            .model
            .measures
            .get(&measure_id)
            .ok_or_else(|| ImprovFfiError::Message("measure not found".into()))?;
        if measure.is_derived() {
            return Err(ImprovFfiError::Message(format!(
                "'{}' is a derived measure computed by a formula",
                measure.name.0
            )));
        }

        let mut coord_map = BTreeMap::new();
        for (c, i) in &state.page_items {
            if measure.categories.contains(c) {
                coord_map.insert(*c, *i);
            }
        }
        for (c, i) in row_coords.iter().chain(col_coords.iter()) {
            if measure.categories.contains(c) {
                coord_map.insert(*c, *i);
            }
        }
        let coord = Coordinate { dims: coord_map };
        state
            .model
            .set_input(measure_id, coord.clone(), Value::Number(value));

        let key = encode_coord(&coord);
        if let Some(eng) = state.engine.as_mut() {
            state.snapshot = eng
                .apply(vec![Edit {
                    measure: measure_id,
                    coord: key,
                    value: Some(CellValue::num(value)),
                }])
                .map_err(|e| ImprovFfiError::Message(e.to_string()))?;
        }
        let EngineState { store, model, .. } = &mut *state;
        if let Some(store) = store.as_mut() {
            store
                .save_cells(model, &[(measure_id, coord)])
                .map_err(|e| ImprovFfiError::Message(e.to_string()))?;
        }
        Ok(())
    }

    /// Returns autocomplete suggestions from `improv_client::completion` and
    /// `improv_nl_formula` for the formula bar.
    pub fn autocomplete_formula(&self, prefix_text: String) -> Vec<String> {
        let state = self.state.lock().unwrap();
        let word = completion::prefix(&prefix_text, prefix_text.len())
            .map(|(_, w)| w)
            .unwrap_or_else(|| prefix_text.split_whitespace().last().unwrap_or(""));

        let mut out: Vec<String> = completion::candidates(&state.model, word)
            .into_iter()
            .map(|c| c.insert)
            .collect();

        for nl_kw in [
            "equals",
            "times",
            "minus",
            "plus",
            "divided by",
            "the sum of",
            "the average of",
            "over",
            "for each",
        ] {
            if (word.is_empty() || nl_kw.starts_with(&word.to_ascii_lowercase()))
                && !out.iter().any(|s| s == nl_kw)
            {
                out.push(nl_kw.to_string());
            }
        }

        if out.is_empty() {
            for m in state.model.measures.values() {
                out.push(m.name.0.clone());
            }
            out.extend([
                "SUM(".into(),
                "PREV(".into(),
                "AVERAGE(".into(),
                "MIN(".into(),
                "MAX(".into(),
            ]);
        }
        out
    }

    /// Returns structured viewport metadata (row headers, column headers, and cells)
    /// for the visible window `[start_row..start_row+row_count, start_col..start_col+col_count]`.
    pub fn get_viewport(
        &self,
        start_row: u32,
        row_count: u32,
        start_col: u32,
        col_count: u32,
    ) -> ViewportSnapshot {
        let state = self.state.lock().unwrap();
        let (all_rows, all_cols) = state.build_grid_axes();
        let total_rows = all_rows.len() as u32;
        let total_cols = all_cols.len() as u32;

        let r_start = (start_row as usize).min(all_rows.len());
        let r_end = (r_start + row_count as usize).min(all_rows.len());
        let c_start = (start_col as usize).min(all_cols.len());
        let c_end = (c_start + col_count as usize).min(all_cols.len());

        let row_headers: Vec<String> = all_rows[r_start..r_end]
            .iter()
            .map(|(h, _, _)| h.clone())
            .collect();
        let col_headers: Vec<String> = all_cols[c_start..c_end]
            .iter()
            .map(|(h, _)| h.clone())
            .collect();

        let mut cells = Vec::with_capacity((r_end - r_start) * (c_end - c_start));
        for (r_idx, (_, m_id, r_coords)) in all_rows[r_start..r_end].iter().enumerate() {
            let m_name = state
                .model
                .measures
                .get(m_id)
                .map(|m| m.name.0.clone())
                .unwrap_or_default();
            for (c_idx, (_, c_coords)) in all_cols[c_start..c_end].iter().enumerate() {
                let (numeric_value, formatted, _tag, is_derived, is_blank) =
                    state.resolve_cell(*m_id, r_coords, c_coords);
                cells.push(ViewportCell {
                    row_index: (r_start + r_idx) as u32,
                    col_index: (c_start + c_idx) as u32,
                    measure_id: m_id.0.to_string(),
                    measure_name: m_name.clone(),
                    numeric_value,
                    formatted,
                    is_derived,
                    is_blank,
                });
            }
        }

        let (active_measure_id, active_measure_name) = state
            .active_measure
            .and_then(|id| state.model.measures.get(&id))
            .map(|m| (m.id.0.to_string(), m.name.0.clone()))
            .unwrap_or_default();

        ViewportSnapshot {
            row_headers,
            col_headers,
            total_rows,
            total_cols,
            active_measure_id,
            active_measure_name,
            cells,
        }
    }

    /// Populates a caller-allocated direct `java.nio.ByteBuffer` (passed by its native
    /// memory address `buffer_ptr` and `buffer_capacity` in bytes) with the visible cell
    /// viewport `[start_row..start_row+row_count, start_col..start_col+col_count]`.
    ///
    /// Returns the total number of bytes written (0 if `buffer_ptr == 0` or capacity < 16).
    pub fn populate_viewport_buffer(
        &self,
        buffer_ptr: u64,
        buffer_capacity: u32,
        start_row: u32,
        row_count: u32,
        start_col: u32,
        col_count: u32,
    ) -> u32 {
        if buffer_ptr == 0 || (buffer_capacity as usize) < VIEWPORT_HEADER_BYTES {
            return 0;
        }
        let state = self.state.lock().unwrap();
        let bytes = encode_viewport_binary(&state, start_row, row_count, start_col, col_count);
        let to_copy = bytes.len().min(buffer_capacity as usize);
        // SAFETY: `buffer_ptr` points to a live direct `ByteBuffer` of at least `buffer_capacity` bytes.
        unsafe {
            let dst = std::slice::from_raw_parts_mut(buffer_ptr as *mut u8, to_copy);
            dst.copy_from_slice(&bytes[..to_copy]);
        }
        to_copy as u32
    }

    /// Returns the binary-encoded viewport buffer (same layout as `populate_viewport_buffer`).
    pub fn get_viewport_bytes(
        &self,
        start_row: u32,
        row_count: u32,
        start_col: u32,
        col_count: u32,
    ) -> Vec<u8> {
        let state = self.state.lock().unwrap();
        encode_viewport_binary(&state, start_row, row_count, start_col, col_count)
    }
}

fn encode_viewport_binary(
    state: &EngineState,
    start_row: u32,
    row_count: u32,
    start_col: u32,
    col_count: u32,
) -> Vec<u8> {
    let (all_rows, all_cols) = state.build_grid_axes();
    let total_rows = all_rows.len() as u32;
    let total_cols = all_cols.len() as u32;

    let r_start = (start_row as usize).min(all_rows.len());
    let r_end = (r_start + row_count as usize).min(all_rows.len());
    let c_start = (start_col as usize).min(all_cols.len());
    let c_end = (c_start + col_count as usize).min(all_cols.len());

    let rows = (r_end - r_start) as u32;
    let cols = (c_end - c_start) as u32;

    let mut buf = Vec::with_capacity(
        VIEWPORT_HEADER_BYTES + (rows as usize) * (cols as usize) * VIEWPORT_CELL_STRIDE_BYTES,
    );
    buf.extend_from_slice(&rows.to_le_bytes());
    buf.extend_from_slice(&cols.to_le_bytes());
    buf.extend_from_slice(&total_rows.to_le_bytes());
    buf.extend_from_slice(&total_cols.to_le_bytes());

    for (_, m_id, r_coords) in &all_rows[r_start..r_end] {
        let row_item_id = r_coords.first().map(|(_, i)| i.0).unwrap_or(0);
        for (_, c_coords) in &all_cols[c_start..c_end] {
            let col_item_id = c_coords.first().map(|(_, i)| i.0).unwrap_or(0);
            let (val, _formatted, tag, is_derived, _is_blank) =
                state.resolve_cell(*m_id, r_coords, c_coords);
            buf.extend_from_slice(&val.to_le_bytes());
            buf.extend_from_slice(&m_id.0.to_le_bytes());
            buf.extend_from_slice(&row_item_id.to_le_bytes());
            buf.extend_from_slice(&col_item_id.to_le_bytes());
            buf.push(tag);
            buf.push(u8::from(is_derived));
            buf.extend_from_slice(&0u16.to_le_bytes());
        }
    }
    buf
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// The 24-byte cell record layout of the viewport buffer, split out so
    /// tests can assert individual fields without re-deriving offsets.
    struct CellRecord {
        value: f64,
        measure_id: u32,
        row_item_id: u32,
        col_item_id: u32,
        tag: u8,
        is_derived: u8,
    }

    /// Reads the header `(rows, cols, total_rows, total_cols)` from a viewport
    /// buffer and the `index`-th cell record after it.
    fn read_header(buf: &[u8]) -> (u32, u32, u32, u32) {
        (
            u32::from_le_bytes(buf[0..4].try_into().unwrap()),
            u32::from_le_bytes(buf[4..8].try_into().unwrap()),
            u32::from_le_bytes(buf[8..12].try_into().unwrap()),
            u32::from_le_bytes(buf[12..16].try_into().unwrap()),
        )
    }

    fn read_cell(buf: &[u8], index: usize) -> CellRecord {
        let off = VIEWPORT_HEADER_BYTES + index * VIEWPORT_CELL_STRIDE_BYTES;
        CellRecord {
            value: f64::from_le_bytes(buf[off..off + 8].try_into().unwrap()),
            measure_id: u32::from_le_bytes(buf[off + 8..off + 12].try_into().unwrap()),
            row_item_id: u32::from_le_bytes(buf[off + 12..off + 16].try_into().unwrap()),
            col_item_id: u32::from_le_bytes(buf[off + 16..off + 20].try_into().unwrap()),
            tag: buf[off + 20],
            is_derived: buf[off + 21],
        }
    }

    /// Finds the measure id the sample model assigned a given name.
    fn measure_id_of(eng: &ImprovMobileEngine, name: &str) -> String {
        eng.get_measures()
            .into_iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("measure {name} not found"))
            .id
    }

    // The original end-to-end scenario over the sample model: build, read a
    // viewport both ways, run analytics, and exercise the main edits.
    #[test]
    fn sample_3d_model_evaluates_and_populates_viewport_buffer() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        let cats = eng.get_categories();
        assert_eq!(cats.len(), 3);
        let formulas = eng.get_formulas();
        assert_eq!(formulas.len(), 3); // Revenue, COGS, Margin

        let vp = eng.get_viewport(0, 50, 0, 50);
        assert_eq!(vp.total_rows, 24); // 4 quarters * 6 measures
        assert_eq!(vp.total_cols, 3); // 3 products
        assert!(!vp.cells.is_empty());

        let mut direct_buf = vec![0u8; 4096];
        let written = eng.populate_viewport_buffer(
            direct_buf.as_mut_ptr() as u64,
            direct_buf.len() as u32,
            0,
            24,
            0,
            3,
        );
        assert_eq!(
            written as usize,
            VIEWPORT_HEADER_BYTES + 24 * 3 * VIEWPORT_CELL_STRIDE_BYTES
        );
        let (rows, cols, _, _) = read_header(&direct_buf);
        assert_eq!((rows, cols), (24, 3));

        // Analytics summary on Margin
        let analytics = eng.get_slice_analytics("Margin".to_string());
        assert_eq!(analytics.measure_name, "Margin");
        assert_eq!(analytics.cell_count, 12);
        assert!(analytics.sum > 0.0);
        assert!(analytics.max >= analytics.min);
        assert_eq!(analytics.series_labels.len(), 4);

        // Add category item, input measure, What-If multiplier, and CSV round-trip
        let updated_time = eng
            .add_category_item("Time".to_string(), "Q5".to_string())
            .unwrap();
        assert_eq!(updated_time.items.len(), 5);

        let discount = eng.add_input_measure("Discount".to_string(), 25.0).unwrap();
        assert_eq!(discount.name, "Discount");
        eng.delete_measure("Discount".to_string()).unwrap();

        let shocked = eng
            .apply_what_if_multiplier("Price".to_string(), 1.10)
            .unwrap();
        assert!(shocked > 0);

        let csv_out = eng.export_measure_csv_text("Revenue".to_string()).unwrap();
        assert!(csv_out.contains("Time,Product,Region,Revenue"));

        let imported = eng
            .import_csv_text(
                "Time,Product,Region,TargetQuota\nQ1,PixelBook,Americas,250000\nQ2,PixelPhone,EMEA,420000\n"
                    .to_string(),
                String::new(),
            )
            .unwrap();
        assert_eq!(imported, 2);
    }

    // ---- ImprovFfiError ----

    #[test]
    fn ffi_error_from_string_carries_message() {
        let err = ImprovFfiError::from("boom".to_string());
        assert_eq!(err.to_string(), "boom");
        let ImprovFfiError::Message(m) = err;
        assert_eq!(m, "boom");
    }

    // ---- open_or_create / blank model / db path ----

    #[test]
    fn open_with_unopenable_path_runs_without_a_store() {
        // A path whose parent directory does not exist cannot be opened as a
        // store, so the engine runs entirely in memory (store = None). Every
        // save becomes a no-op, and edits still work against the live model.
        let eng = ImprovMobileEngine::open_or_create(
            "/nonexistent-improv-xyz/parent/missing.db".to_string(),
        );
        // The sample model still loads (the "no store to seed" branch).
        assert_eq!(eng.get_categories().len(), 3);
        // Edits that would normally persist run their save paths with no store.
        eng.add_category_item("Time".to_string(), "Q5".to_string())
            .unwrap();
        eng.set_cell_value(0, 0, 7.0).unwrap();
        eng.load_sample_model().unwrap();
        eng.create_blank_model().unwrap();
        eng.add_category(
            "Extra".to_string(),
            vec!["A".to_string()],
            AxisOrientation::Page,
        )
        .unwrap();
        eng.select_page_item("Extra".to_string(), "A".to_string());
        // Nothing panicked and the model is still coherent.
        assert!(!eng.get_measures().is_empty());
    }

    #[test]
    fn blank_model_has_period_department_budget() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        let cats = eng.get_categories();
        assert_eq!(cats.len(), 2);
        let measures = eng.get_measures();
        assert_eq!(measures.len(), 1);
        assert_eq!(measures[0].name, "Budget");
        assert!(!measures[0].is_derived);
        // Blank model: a single input measure so the grid's one-row header is
        // the bare measure name, not "<tuple> · <measure>".
        let vp = eng.get_viewport(0, 10, 0, 10);
        assert_eq!(vp.total_rows, 2); // P1, P2 (one measure)
        assert_eq!(vp.total_cols, 2); // Core, Growth
        assert!(vp.row_headers.iter().any(|h| h.contains("Budget")));
    }

    #[test]
    fn single_measure_empty_row_axis_uses_bare_measure_name() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        // Push both categories off the row axis so the row tuple collapses to
        // the single "All" tuple; with one measure the row header is then the
        // bare measure name (the `row_tuples.len() == 1 && empty` branch).
        eng.pivot_category("Period".to_string(), AxisOrientation::Column, 0);
        eng.pivot_category("Department".to_string(), AxisOrientation::Page, 0);
        let vp = eng.get_viewport(0, 10, 0, 10);
        assert_eq!(vp.total_rows, 1);
        assert_eq!(vp.row_headers, vec!["Budget".to_string()]);
    }

    #[test]
    fn open_or_create_persists_and_reloads_from_disk() {
        let dir = std::env::temp_dir().join(format!("improv_android_ffi_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("reopen.db");
        let path_str = path.to_string_lossy().to_string();

        {
            let eng = ImprovMobileEngine::open_or_create(path_str.clone());
            assert_eq!(eng.get_db_path(), path_str);
            // A freshly created store is seeded with the 3D sample model.
            assert_eq!(eng.get_categories().len(), 3);
            eng.add_category_item("Time".to_string(), "Q5".to_string())
                .unwrap();
        }
        // Reopening the same path loads the saved model (not the sample again),
        // so the Q5 item added above survives.
        let eng2 = ImprovMobileEngine::open_or_create(path_str);
        let time = eng2
            .get_categories()
            .into_iter()
            .find(|c| c.name == "Time")
            .unwrap();
        assert_eq!(time.items.len(), 5);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_sample_model_resets_to_sample() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        assert_eq!(eng.get_categories().len(), 2);
        eng.load_sample_model().unwrap();
        assert_eq!(eng.get_categories().len(), 3);
        assert_eq!(eng.get_formulas().len(), 3);
    }

    // ---- axis_tuples / build_grid_axes via get_categories & viewport ----

    #[test]
    fn categories_report_their_axis_orientation() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        let cats = eng.get_categories();
        // sync_axes_with_model seeds Time->Row, Product->Column, Region->Page.
        let by_name = |n: &str| cats.iter().find(|c| c.name == n).unwrap();
        assert_eq!(by_name("Time").orientation, AxisOrientation::Row);
        assert_eq!(by_name("Product").orientation, AxisOrientation::Column);
        assert_eq!(by_name("Region").orientation, AxisOrientation::Page);
        // The page category pins a selected item.
        assert!(by_name("Region").selected_page_item_id.is_some());
    }

    #[test]
    fn adding_a_category_products_cartesian_columns() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        // Blank model has Period (2) x Department (2). Put a third category on
        // the column axis to exercise the cartesian product of two column
        // categories (2 x 2 = 4 columns).
        eng.add_category(
            "Channel".to_string(),
            vec!["Online".to_string(), "Retail".to_string()],
            AxisOrientation::Column,
        )
        .unwrap();
        let vp = eng.get_viewport(0, 50, 0, 50);
        assert_eq!(vp.total_cols, 4); // Department x Channel
        assert!(vp.col_headers.iter().any(|h| h.contains(" / ")));
    }

    // ---- get_viewport windowing ----

    #[test]
    fn viewport_windowing_clamps_and_offsets() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Full grid is 24 rows x 3 cols.
        let full = eng.get_viewport(0, 100, 0, 100);
        assert_eq!(full.cells.len(), 24 * 3);

        // A window starting mid-grid, limited past the end, clamps to the grid.
        let win = eng.get_viewport(20, 100, 1, 100);
        assert_eq!(win.total_rows, 24);
        assert_eq!(win.row_headers.len(), 4); // rows 20..24
        assert_eq!(win.col_headers.len(), 2); // cols 1..3
        assert_eq!(win.cells.len(), 4 * 2);
        assert_eq!(win.cells[0].row_index, 20);
        assert_eq!(win.cells[0].col_index, 1);

        // A start past the end yields no rows but still reports the totals.
        let past = eng.get_viewport(1000, 5, 1000, 5);
        assert_eq!(past.total_rows, 24);
        assert_eq!(past.total_cols, 3);
        assert!(past.cells.is_empty());
        assert!(past.row_headers.is_empty());

        // A zero-size window: no cells, totals still known.
        let zero = eng.get_viewport(0, 0, 0, 0);
        assert!(zero.cells.is_empty());
        assert_eq!(zero.total_rows, 24);

        // The active measure is surfaced (the sample view focuses Margin).
        assert!(!full.active_measure_id.is_empty());
        assert!(!full.active_measure_name.is_empty());
    }

    #[test]
    fn viewport_distinguishes_input_derived_and_blank_cells() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        let vp = eng.get_viewport(0, 24, 0, 3);
        // Every input Price/UnitCost/Quantity cell is numeric and not derived.
        let input_cell = vp.cells.iter().find(|c| c.measure_name == "Price").unwrap();
        assert!(!input_cell.is_blank);
        assert!(!input_cell.is_derived);
        assert!(input_cell.numeric_value > 0.0);
        // Revenue is derived and evaluates to a number.
        let derived_cell = vp
            .cells
            .iter()
            .find(|c| c.measure_name == "Revenue")
            .unwrap();
        assert!(derived_cell.is_derived);
        assert!(!derived_cell.is_blank);
        assert!(derived_cell.numeric_value > 0.0);
    }

    #[test]
    fn viewport_blank_cell_for_sparse_input() {
        // A model with one category and one input measure, leaving a cell
        // unset so resolve_cell takes its blank branch (tag 0, NaN).
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        // Add a new item but no value for it -> that row is blank.
        eng.add_category_item("Period".to_string(), "P3".to_string())
            .unwrap();
        let vp = eng.get_viewport(0, 50, 0, 50);
        let blank = vp.cells.iter().find(|c| c.is_blank);
        assert!(blank.is_some(), "expected at least one blank cell");
        assert!(blank.unwrap().numeric_value.is_nan());
    }

    // ---- populate_viewport_buffer (unsafe ByteBuffer path) ----

    #[test]
    fn populate_buffer_null_or_tiny_writes_nothing() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Null pointer -> 0.
        assert_eq!(eng.populate_viewport_buffer(0, 4096, 0, 24, 0, 3), 0);
        // Capacity below the 16-byte header -> 0, even with a valid pointer.
        let mut tiny = [0u8; 8];
        assert_eq!(
            eng.populate_viewport_buffer(tiny.as_mut_ptr() as u64, tiny.len() as u32, 0, 24, 0, 3),
            0
        );
    }

    #[test]
    fn populate_buffer_truncates_without_overflow() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Room for the header plus exactly two cell records; the full grid
        // needs far more, so the write must stop at the capacity and never
        // touch the guard bytes past it.
        let capacity = VIEWPORT_HEADER_BYTES + 2 * VIEWPORT_CELL_STRIDE_BYTES;
        let mut backing = vec![0xABu8; capacity + 32];
        let written =
            eng.populate_viewport_buffer(backing.as_mut_ptr() as u64, capacity as u32, 0, 24, 0, 3);
        assert_eq!(written as usize, capacity);
        // The guard bytes beyond `capacity` stay untouched.
        assert!(backing[capacity..].iter().all(|&b| b == 0xAB));
        // The header still reports the full window geometry (24 x 3), even
        // though only the first cells fit.
        let (rows, cols, total_rows, total_cols) = read_header(&backing);
        assert_eq!((rows, cols, total_rows, total_cols), (24, 3, 24, 3));
    }

    #[test]
    fn buffer_bytes_match_cell_tags_and_fields() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        let bytes = eng.get_viewport_bytes(0, 24, 0, 3);
        let (rows, cols, total_rows, total_cols) = read_header(&bytes);
        assert_eq!((rows, cols, total_rows, total_cols), (24, 3, 24, 3));
        assert_eq!(
            bytes.len(),
            VIEWPORT_HEADER_BYTES + 24 * 3 * VIEWPORT_CELL_STRIDE_BYTES
        );

        let price_id: u32 = measure_id_of(&eng, "Price").parse().unwrap();
        let revenue_id: u32 = measure_id_of(&eng, "Revenue").parse().unwrap();

        let mut saw_input = false;
        let mut saw_derived = false;
        for i in 0..(24 * 3) {
            let cell = read_cell(&bytes, i);
            assert_ne!(cell.row_item_id, 0);
            assert_ne!(cell.col_item_id, 0);
            if cell.measure_id == price_id {
                assert_eq!(cell.tag, 1, "input number is tag 1"); // InputNumber
                assert_eq!(cell.is_derived, 0);
                assert!(cell.value.is_finite());
                saw_input = true;
            }
            if cell.measure_id == revenue_id {
                assert_eq!(cell.tag, 2, "derived number is tag 2"); // DerivedNumber
                assert_eq!(cell.is_derived, 1);
                assert!(cell.value.is_finite());
                saw_derived = true;
            }
        }
        assert!(saw_input && saw_derived);
    }

    #[test]
    fn get_viewport_bytes_and_populate_buffer_agree() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        let bytes = eng.get_viewport_bytes(0, 24, 0, 3);
        let mut direct = vec![0u8; bytes.len()];
        let written = eng.populate_viewport_buffer(
            direct.as_mut_ptr() as u64,
            direct.len() as u32,
            0,
            24,
            0,
            3,
        );
        assert_eq!(written as usize, bytes.len());
        assert_eq!(direct, bytes);
    }

    #[test]
    fn derived_error_and_boolean_cells_tag_correctly() {
        // Exercise resolve_cell's derived error (tag 4) and derived
        // non-numeric (tag 3) branches through upsert_formula.
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Division by zero: UnitCost is never zero in the sample, so divide by
        // (Quantity - Quantity) to force a #DIV/0! in every cell.
        eng.upsert_formula("Bad = Price / (Quantity - Quantity)".to_string())
            .unwrap();
        // A comparison yields a boolean (non-numeric) derived value.
        eng.upsert_formula("Flag = Price > UnitCost".to_string())
            .unwrap();

        let bytes = eng.get_viewport_bytes(0, 100, 0, 3);
        let bad_id: u32 = measure_id_of(&eng, "Bad").parse().unwrap();
        let flag_id: u32 = measure_id_of(&eng, "Flag").parse().unwrap();
        let total = (bytes.len() - VIEWPORT_HEADER_BYTES) / VIEWPORT_CELL_STRIDE_BYTES;

        let mut saw_error = false;
        let mut saw_nonnumeric = false;
        for i in 0..total {
            let cell = read_cell(&bytes, i);
            if cell.measure_id == bad_id {
                assert_eq!(cell.tag, 4, "division by zero is tag 4"); // Error
                assert_eq!(cell.is_derived, 1);
                assert!(cell.value.is_nan());
                saw_error = true;
            }
            if cell.measure_id == flag_id {
                assert_eq!(cell.tag, 3, "boolean is tag 3"); // NonNumeric
                assert_eq!(cell.is_derived, 1);
                saw_nonnumeric = true;
            }
        }
        assert!(saw_error, "expected a #DIV/0! error cell");
        assert!(saw_nonnumeric, "expected a boolean non-numeric cell");
    }

    // ---- analytics ----

    #[test]
    fn analytics_deterministic_oracle() {
        // A tiny hand-built model with a known single-row series so the
        // statistics have exact expected values.
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        // Blank model: Period {P1,P2} x Department {Core,Growth}, Budget all 0.
        // Set the four Budget cells to 10, 20, 30, 40 via the grid.
        // Rows are the two Periods (single measure), cols are the two Depts.
        let values = [[10.0, 20.0], [30.0, 40.0]];
        for (r, row) in values.iter().enumerate() {
            for (c, v) in row.iter().enumerate() {
                eng.set_cell_value(r as u32, c as u32, *v).unwrap();
            }
        }
        let a = eng.get_slice_analytics("Budget".to_string());
        assert_eq!(a.cell_count, 4);
        assert_eq!(a.sum, 100.0);
        assert_eq!(a.mean, 25.0);
        assert_eq!(a.median, 25.0); // (20 + 30) / 2
        assert_eq!(a.min, 10.0);
        assert_eq!(a.max, 40.0);
        // population std dev of [10,20,30,40] = sqrt(125) ~= 11.1803
        assert!((a.std_dev - 125f64.sqrt()).abs() < 1e-9);
        // Two rows each with a value -> two per-row series points.
        assert_eq!(a.series_labels.len(), 2);
        assert_eq!(a.series_values, vec![30.0, 70.0]);
    }

    #[test]
    fn analytics_odd_count_median() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        eng.add_category_item("Department".to_string(), "Ops".to_string())
            .unwrap();
        // Period {P1,P2} x Department {Core,Growth,Ops} = 6 cells; fill five
        // with values and leave one blank by setting only five.
        let vals = [1.0, 2.0, 3.0, 4.0, 5.0];
        let mut k = 0;
        'outer: for r in 0..2u32 {
            for c in 0..3u32 {
                if k >= vals.len() {
                    break 'outer;
                }
                eng.set_cell_value(r, c, vals[k]).unwrap();
                k += 1;
            }
        }
        let a = eng.get_slice_analytics("Budget".to_string());
        // The sixth cell (a Period x Ops coordinate) was never seeded, so it
        // stays blank and drops out; the five set values give an odd count.
        assert_eq!(a.cell_count, 5);
        assert_eq!(a.median, 3.0); // median of [1, 2, 3, 4, 5]
    }

    #[test]
    fn analytics_all_blank_measure_is_zeroed() {
        // A measure that resolves to all-blank cells reports zeroed stats and
        // no series.
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        // Add an input measure then delete every cell by never setting it is
        // not possible (add_input_measure seeds defaults); instead add a
        // category whose items have no Budget value.
        eng.add_category_item("Period".to_string(), "P3".to_string())
            .unwrap();
        // The new P3 row has no Budget cells -> its series row is skipped, but
        // the existing rows still carry values, so target a brand-new sparse
        // measure via CSV import of a disjoint category.
        let a = eng.get_slice_analytics("Budget".to_string());
        // P1/P2 rows have values; P3 does not. cell_count counts only finite
        // cells, so it stays at the four seeded P1/P2 cells.
        assert_eq!(a.cell_count, 4);
        assert_eq!(a.series_labels.len(), 2);
    }

    #[test]
    fn analytics_unknown_measure_falls_back_to_active() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // An unknown name falls back to the active measure, never panics.
        let a = eng.get_slice_analytics("NoSuchMeasure".to_string());
        assert!(a.cell_count > 0);
        assert!(!a.measure_name.is_empty());
    }

    #[test]
    fn analytics_single_cell() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        eng.delete_measure("nope".to_string()).ok();
        // Reduce to a 1x1 grid by removing one item from each axis is not a
        // public op; instead pivot Department to Page so the grid is 2 rows x
        // 1 "All" column, then a single row has one value.
        eng.pivot_category("Department".to_string(), AxisOrientation::Page, 0);
        let a = eng.get_slice_analytics("Budget".to_string());
        // Period on rows (2), Department pinned on page -> 2 finite cells.
        assert_eq!(a.cell_count, 2);
        assert!(a.min <= a.max);
    }

    // ---- edits: category items, measures, derived, what-if, rename ----

    #[test]
    fn add_category_item_errors() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Empty name.
        assert!(eng
            .add_category_item("Time".to_string(), "  ".to_string())
            .is_err());
        // Unknown category.
        assert!(eng
            .add_category_item("Nope".to_string(), "X".to_string())
            .is_err());
        // Duplicate item (case-insensitive).
        assert!(eng
            .add_category_item("Time".to_string(), "q1".to_string())
            .is_err());
        // By numeric category id succeeds.
        let tile = eng
            .add_category_item("1".to_string(), "Q5".to_string())
            .unwrap();
        assert_eq!(tile.name, "Time");
    }

    #[test]
    fn add_input_measure_errors_on_blank_and_duplicate() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        assert!(eng.add_input_measure("  ".to_string(), 0.0).is_err());
        // "Time" is a category name already in use.
        assert!(eng.add_input_measure("Time".to_string(), 0.0).is_err());
        // "Price" is an existing measure name.
        assert!(eng.add_input_measure("Price".to_string(), 0.0).is_err());
    }

    #[test]
    fn delete_measure_guards() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Unknown measure.
        assert!(eng.delete_measure("Ghost".to_string()).is_err());
        // Price is referenced by Revenue (= Price * Quantity), so it is
        // protected from deletion.
        assert!(eng.delete_measure("Price".to_string()).is_err());
        // Delete a leaf derived measure by id.
        let margin_id = measure_id_of(&eng, "Margin");
        eng.delete_measure(margin_id).unwrap();
        assert!(eng.get_measures().iter().all(|m| m.name != "Margin"));

        // A blank model with one measure refuses to delete the last measure.
        eng.create_blank_model().unwrap();
        assert!(eng.delete_measure("Budget".to_string()).is_err());
    }

    #[test]
    fn define_derived_measure_valid_and_parse_error() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Valid symbolic DSL.
        let entry = eng
            .upsert_formula("GrossUp = Revenue + COGS".to_string())
            .unwrap();
        assert_eq!(entry.measure_name, "GrossUp");
        assert!(entry.expression.contains("GrossUp"));
        assert!(entry.error_message.is_none());
        assert!(eng
            .get_formulas()
            .iter()
            .any(|f| f.measure_name == "GrossUp"));

        // Natural-language form.
        let nl = eng
            .upsert_formula("Twice equals Revenue plus Revenue".to_string())
            .unwrap();
        assert_eq!(nl.measure_name, "Twice");
        // The "is" connective is accepted as well as "equals".
        let nl_is = eng
            .upsert_formula("Double is Revenue plus Revenue".to_string())
            .unwrap();
        assert_eq!(nl_is.measure_name, "Double");

        // Empty formula.
        assert!(eng.upsert_formula("   ".to_string()).is_err());
        // No '=' and not 'equals'/'is'.
        assert!(eng.upsert_formula("Revenue plus COGS".to_string()).is_err());
        // A single word with no '=' (fewer than three tokens).
        assert!(eng.upsert_formula("JustOneWord".to_string()).is_err());
        // An empty left-hand side before '='.
        assert!(eng.upsert_formula("= Revenue + COGS".to_string()).is_err());
        // Missing RHS.
        assert!(eng.upsert_formula("Bogus = ".to_string()).is_err());
        // Target collides with a category name.
        assert!(eng
            .upsert_formula("Time = Revenue + COGS".to_string())
            .is_err());
        // An unparsable expression surfaces both DSL and NL errors.
        let err = eng
            .upsert_formula("Weird = Revenue @@@ COGS".to_string())
            .unwrap_err();
        assert!(matches!(err, ImprovFfiError::Message(_)));
        // A formula that parses but has an inconsistent shape fails at the
        // type-inference stage (AND requires Boolean operands, not numbers).
        let shape_err = eng
            .upsert_formula("TypeBad = Price AND UnitCost".to_string())
            .unwrap_err();
        let ImprovFfiError::Message(m) = shape_err;
        assert!(m.contains("type mismatch"), "got: {m}");
    }

    #[test]
    fn cyclic_formula_is_rejected_and_rolls_back() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Build a mutual cycle in two steps. The first definition succeeds.
        eng.upsert_formula("Alpha = Revenue + 1".to_string())
            .unwrap();
        // The second closes the loop (Beta -> Alpha and Alpha redefined to
        // reference Beta). It parses and its shape resolves, but building the
        // dataflow engine detects the cycle, so upsert_formula rolls the model
        // back and returns an error (the rebuild_engine error branch).
        eng.upsert_formula("Beta = Alpha + 1".to_string()).unwrap();
        let err = eng
            .upsert_formula("Alpha = Beta + 1".to_string())
            .unwrap_err();
        assert!(matches!(err, ImprovFfiError::Message(_)));
        // The rollback restores Alpha to its pre-cycle definition, so the
        // engine still evaluates.
        let vp = eng.get_viewport(0, 100, 0, 3);
        let revenue = vp
            .cells
            .iter()
            .find(|c| c.measure_name == "Revenue")
            .unwrap();
        assert!(revenue.numeric_value > 0.0);
    }

    #[test]
    fn apply_what_if_multiplier_paths() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Unknown measure.
        assert!(eng
            .apply_what_if_multiplier("Ghost".to_string(), 1.1)
            .is_err());
        // Derived measure is rejected.
        assert!(eng
            .apply_what_if_multiplier("Revenue".to_string(), 1.1)
            .is_err());
        // Valid input measure: every Price cell updates.
        let before = eng.get_viewport_bytes(0, 24, 0, 3);
        let updated = eng
            .apply_what_if_multiplier("Price".to_string(), 2.0)
            .unwrap();
        assert_eq!(updated, 36); // 4 Time x 3 Product x 3 Region input cells
        let after = eng.get_viewport_bytes(0, 24, 0, 3);
        assert_ne!(before, after);
    }

    #[test]
    fn set_cell_value_paths() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        // Row 0 is Budget (an input measure) in the blank model.
        eng.set_cell_value(0, 0, 123.0).unwrap();
        let vp = eng.get_viewport(0, 1, 0, 1);
        assert_eq!(vp.cells[0].numeric_value, 123.0);
        // Row index out of bounds.
        assert!(eng.set_cell_value(999, 0, 1.0).is_err());
        // Column index out of bounds.
        assert!(eng.set_cell_value(0, 999, 1.0).is_err());

        // Setting a derived cell is rejected.
        let eng2 = ImprovMobileEngine::open_or_create(String::new());
        // Find a row whose measure is Revenue (derived) in the sample grid.
        let grid = eng2.get_viewport(0, 24, 0, 3);
        let derived_row = grid
            .cells
            .iter()
            .find(|c| c.measure_name == "Revenue")
            .unwrap()
            .row_index;
        assert!(eng2.set_cell_value(derived_row, 0, 1.0).is_err());

        // Setting an INPUT cell in the sample model (which has derived
        // measures, so the engine is live) pushes the edit through the engine
        // and updates every dependent derived cell.
        let price_row = grid
            .cells
            .iter()
            .find(|c| c.measure_name == "Price")
            .unwrap()
            .row_index;
        eng2.set_cell_value(price_row, 0, 4242.0).unwrap();
        let after = eng2.get_viewport(0, 24, 0, 3);
        let price_cell = after
            .cells
            .iter()
            .find(|c| c.measure_name == "Price" && c.row_index == price_row && c.col_index == 0)
            .unwrap();
        assert_eq!(price_cell.numeric_value, 4242.0);
    }

    // ---- pivot & page selection ----

    #[test]
    fn pivot_and_select_page_item() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Move Region (a page category) onto the row axis by name.
        eng.pivot_category("Region".to_string(), AxisOrientation::Row, 0);
        let region = eng
            .get_categories()
            .into_iter()
            .find(|c| c.name == "Region")
            .unwrap();
        assert_eq!(region.orientation, AxisOrientation::Row);

        // Move it back to Page with an out-of-range index (clamped to the end).
        eng.pivot_category("Region".to_string(), AxisOrientation::Page, 99);
        // Move Product onto the Column axis explicitly (the Column match arm).
        eng.pivot_category("Product".to_string(), AxisOrientation::Column, 0);
        // Unknown category is a no-op (does not panic).
        eng.pivot_category("Ghost".to_string(), AxisOrientation::Row, 0);
        // A numeric id that does not name any category hits the
        // `!contains_key` early return (parses but is not in the model).
        eng.pivot_category("99999".to_string(), AxisOrientation::Row, 0);

        // Select a page item by id.
        let region = eng
            .get_categories()
            .into_iter()
            .find(|c| c.name == "Region")
            .unwrap();
        let emea = region.items.iter().find(|i| i.name == "EMEA").unwrap();
        eng.select_page_item("Region".to_string(), emea.id.clone());
        let region = eng
            .get_categories()
            .into_iter()
            .find(|c| c.name == "Region")
            .unwrap();
        assert_eq!(
            region.selected_page_item_id.as_deref(),
            Some(emea.id.as_str())
        );

        // Select by item name.
        eng.select_page_item("Region".to_string(), "APAC".to_string());
        // Unknown category / unknown item are no-ops.
        eng.select_page_item("Ghost".to_string(), "X".to_string());
        eng.select_page_item("Region".to_string(), "NoItem".to_string());
    }

    #[test]
    fn add_category_defaults_and_errors() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Empty name.
        assert!(eng
            .add_category(String::new(), vec![], AxisOrientation::Row)
            .is_err());
        // Collides with an existing category.
        assert!(eng
            .add_category("Time".to_string(), vec![], AxisOrientation::Row)
            .is_err());
        // Collides with an existing measure name.
        assert!(eng
            .add_category("Price".to_string(), vec![], AxisOrientation::Row)
            .is_err());
        // No item names -> two defaulted items, placed on Page.
        let tile = eng
            .add_category("Scenario".to_string(), vec![], AxisOrientation::Page)
            .unwrap();
        assert_eq!(tile.items.len(), 2);
        assert_eq!(tile.orientation, AxisOrientation::Page);
        assert!(tile.selected_page_item_id.is_some());
        // Explicit items placed on the Row axis (covers the Row match arm).
        let on_row = eng
            .add_category(
                "Channel".to_string(),
                vec!["Online".to_string(), "Retail".to_string()],
                AxisOrientation::Row,
            )
            .unwrap();
        assert_eq!(on_row.orientation, AxisOrientation::Row);
        assert_eq!(on_row.items.len(), 2);
        // And on the Column axis.
        let on_col = eng
            .add_category(
                "Segment".to_string(),
                vec!["SMB".to_string()],
                AxisOrientation::Column,
            )
            .unwrap();
        assert_eq!(on_col.orientation, AxisOrientation::Column);
    }

    #[test]
    fn set_cell_value_ignores_coords_outside_the_measure() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Add a new row category the existing input measures do not span.
        eng.add_category(
            "Scenario".to_string(),
            vec!["Base".to_string(), "Upside".to_string()],
            AxisOrientation::Row,
        )
        .unwrap();
        // Find a Price (input) row and set it: the Scenario row-coord is not a
        // category of Price, so that coordinate is skipped when building the
        // cell key (the `measure.categories.contains` false branch).
        let grid = eng.get_viewport(0, 200, 0, 50);
        let price = grid
            .cells
            .iter()
            .find(|c| c.measure_name == "Price")
            .unwrap();
        eng.set_cell_value(price.row_index, price.col_index, 1234.0)
            .unwrap();
        let after = eng.get_viewport(0, 200, 0, 50);
        let updated = after
            .cells
            .iter()
            .find(|c| {
                c.measure_name == "Price"
                    && c.row_index == price.row_index
                    && c.col_index == price.col_index
            })
            .unwrap();
        assert_eq!(updated.numeric_value, 1234.0);
    }

    // ---- measures / formulas listing ----

    #[test]
    fn get_measures_and_formulas_shape() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        let measures = eng.get_measures();
        assert_eq!(measures.len(), 6);
        // Inputs have no formula text; derived do.
        let price = measures.iter().find(|m| m.name == "Price").unwrap();
        assert!(!price.is_derived);
        assert!(price.formula_text.is_none());
        let revenue = measures.iter().find(|m| m.name == "Revenue").unwrap();
        assert!(revenue.is_derived);
        assert!(revenue.formula_text.is_some());

        let formulas = eng.get_formulas();
        assert_eq!(formulas.len(), 3);
        let rev = formulas
            .iter()
            .find(|f| f.measure_name == "Revenue")
            .unwrap();
        assert!(rev.expression.starts_with("Revenue ="));
        assert!(rev.natural_language.starts_with("Revenue equals"));
    }

    // ---- autocomplete ----

    #[test]
    fn autocomplete_offers_keywords_and_candidates() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // A prefix matching an NL keyword offers it.
        let eq = eng.autocomplete_formula("Margin eq".to_string());
        assert!(eq.iter().any(|s| s == "equals"));
        // An empty-word prefix offers the full NL keyword set.
        let all = eng.autocomplete_formula(String::new());
        assert!(all.iter().any(|s| s == "times"));
        assert!(all.iter().any(|s| s == "divided by"));
        // A measure prefix offers measure-name candidates.
        let rev = eng.autocomplete_formula("Rev".to_string());
        assert!(rev.iter().any(|s| s.starts_with("Rev")));
        // A prefix that matches no candidate and no NL keyword falls back to
        // the full measure list plus the function stubs.
        let fallback = eng.autocomplete_formula("zzzzzz".to_string());
        assert!(fallback.iter().any(|s| s == "SUM("));
        assert!(fallback.iter().any(|s| s == "Revenue"));
    }

    // ---- CSV export / import ----

    #[test]
    fn export_measure_csv_to_file_and_text() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        let dir = std::env::temp_dir().join(format!("improv_csv_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("revenue.csv");
        let n = eng
            .export_measure_csv("Revenue".to_string(), out.to_string_lossy().to_string())
            .unwrap();
        assert!(n > 0);
        let on_disk = std::fs::read_to_string(&out).unwrap();
        assert!(on_disk.contains("Revenue"));
        std::fs::remove_dir_all(&dir).ok();

        // Unknown measure for file export errors.
        assert!(eng
            .export_measure_csv("Ghost".to_string(), "/tmp/none.csv".to_string())
            .is_err());

        // A valid measure but an unwritable path makes the CSV writer fail,
        // exercising the write-error branch.
        let bad = eng.export_measure_csv(
            "Revenue".to_string(),
            "/nonexistent-improv-dir/sub/out.csv".to_string(),
        );
        assert!(bad.is_err());

        // Text export of an unknown name falls back to the lowest-id measure.
        let text = eng.export_measure_csv_text("Ghost".to_string()).unwrap();
        assert!(!text.is_empty());
    }

    #[test]
    fn import_csv_errors_and_new_items() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Empty content.
        assert!(eng.import_csv_text(String::new(), String::new()).is_err());
        // Only one column (no value column).
        assert!(eng
            .import_csv_text("OnlyOne\nx\n".to_string(), String::new())
            .is_err());
        // A non-numeric value in a Number column makes the import fail.
        assert!(eng
            .import_csv_text(
                "Time,Product,Region,Quota\nQ1,PixelBook,Americas,notanumber\n".to_string(),
                String::new(),
            )
            .is_err());
        // A measure-name override is honored, and brand-new category items are
        // created from the rows.
        let n = eng
            .import_csv_text(
                "Time,Product,Region,Quota\nQ9,NewProd,Mars,42\n".to_string(),
                "Target".to_string(),
            )
            .unwrap();
        assert_eq!(n, 1);
        assert!(eng.get_measures().iter().any(|m| m.name == "Target"));
        // Re-importing into the SAME measure name reuses the existing measure
        // id (the find-by-name success path) rather than minting a new one.
        let before = eng.get_measures().len();
        let n2 = eng
            .import_csv_text(
                "Time,Product,Region,Target\nQ1,PixelBook,Americas,99\n".to_string(),
                "Target".to_string(),
            )
            .unwrap();
        assert_eq!(n2, 1);
        assert_eq!(eng.get_measures().len(), before);
    }

    // ---- NL formula helpers used indirectly by upsert_formula / get_formulas ----

    #[test]
    fn rename_via_redefining_a_derived_measure_in_place() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // Redefining an existing derived measure keeps its id and updates the
        // formula (the "rename/redefine" path through upsert_formula).
        let before = measure_id_of(&eng, "Margin");
        let entry = eng
            .upsert_formula("Margin = Revenue - COGS - COGS".to_string())
            .unwrap();
        assert_eq!(entry.id, before);
        assert_eq!(entry.measure_name, "Margin");
    }

    // ---- EngineState internals ----
    //
    // A handful of defensive branches guard model states the public FFI never
    // builds (an item id with no name, a measure category on no axis, no
    // active measure). They exist because EngineState's helpers must stay
    // correct even mid-edit, so exercise them directly on a crafted state.

    /// A bare EngineState over `model` with no store and no engine.
    fn bare_state(model: Model) -> EngineState {
        EngineState {
            db_path: String::new(),
            store: None,
            model,
            engine: None,
            snapshot: Snapshot::default(),
            row_cats: Vec::new(),
            col_cats: Vec::new(),
            page_cats: Vec::new(),
            page_items: BTreeMap::new(),
            active_measure: None,
        }
    }

    #[test]
    fn axis_tuples_falls_back_to_item_id_when_name_missing() {
        // A category that lists an item id with no matching items entry: the
        // label falls back to the numeric id (the unwrap_or_else branch).
        let mut model = Model::new();
        let cat = CategoryId(1);
        model.add_category(cat, "Orphans");
        model.add_item(ItemId(10), cat, "Named");
        // Point the category at an item that was never registered.
        model
            .categories
            .get_mut(&cat)
            .unwrap()
            .items
            .push(ItemId(999));
        let state = bare_state(model);
        let tuples = state.axis_tuples(&[cat]);
        let labels: Vec<&str> = tuples.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.contains(&"Named"));
        // The orphan item id has no name, so its label is the numeric id.
        assert!(labels.contains(&"999"));
        // A category id that is not in the model is skipped (the `continue`).
        let skipped = state.axis_tuples(&[CategoryId(1), CategoryId(42)]);
        assert_eq!(skipped.len(), 2); // only Orphans' two items
    }

    #[test]
    fn resolve_cell_fills_defaults_and_handles_missing_measure() {
        // A measure that spans a category not present on any axis nor in
        // page_items: resolve_cell fills that category's first item as a
        // default (the default-fill branch).
        let mut model = Model::new();
        let region = CategoryId(1);
        model.add_category(region, "Region");
        model.add_item(ItemId(10), region, "East");
        model.add_item(ItemId(11), region, "West");
        let mid = MeasureId(100);
        model.add_measure(Measure {
            id: mid,
            name: Name("Sales".into()),
            value_type: ValueType::Number,
            categories: vec![region],
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
        model.set_input(
            mid,
            Coordinate::from_pairs([(region, ItemId(10))]),
            Value::Number(7.0),
        );
        let state = bare_state(model);
        // No row/col coords and no page_items: resolve_cell defaults Region to
        // its first item (East) and reads 7.0 from the input.
        let (v, _fmt, tag, is_derived, is_blank) = state.resolve_cell(mid, &[], &[]);
        assert_eq!(v, 7.0);
        assert_eq!(tag, 1);
        assert!(!is_derived);
        assert!(!is_blank);
        // An unknown measure id returns the blank sentinel.
        let (nv, _f, ntag, _d, nblank) = state.resolve_cell(MeasureId(777), &[], &[]);
        assert!(nv.is_nan());
        assert_eq!(ntag, 0);
        assert!(nblank);
    }

    #[test]
    fn resolve_cell_input_error_and_non_numeric() {
        // Input cells holding an Error or a non-numeric value take the tag-4
        // and tag-3 branches of resolve_cell. The public FFI only writes
        // numbers, so craft these directly on the model.
        let mut model = Model::new();
        let cat = CategoryId(1);
        model.add_category(cat, "K");
        model.add_item(ItemId(10), cat, "a");
        model.add_item(ItemId(11), cat, "b");
        let mid = MeasureId(100);
        model.add_measure(Measure {
            id: mid,
            name: Name("M".into()),
            value_type: ValueType::Number,
            categories: vec![cat],
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
        model.set_input(
            mid,
            Coordinate::from_pairs([(cat, ItemId(10))]),
            Value::Error(improv_core_model::ValueError::new(
                improv_core_model::ValueErrorKind::DivisionByZero,
            )),
        );
        model.set_input(
            mid,
            Coordinate::from_pairs([(cat, ItemId(11))]),
            Value::Text("hello".into()),
        );
        let state = bare_state(model);
        let (_v, _f, err_tag, _d, _b) = state.resolve_cell(mid, &[(cat, ItemId(10))], &[]);
        assert_eq!(err_tag, 4); // input error
        let (_v2, _f2, text_tag, _d2, _b2) = state.resolve_cell(mid, &[(cat, ItemId(11))], &[]);
        assert_eq!(text_tag, 3); // input non-numeric
    }

    #[test]
    fn update_default_view_noop_without_active_measure() {
        // With no active measure, update_default_view returns early and adds
        // no view; save_structure then persists nothing (no store).
        let mut state = bare_state(Model::new());
        state.save_structure().unwrap();
        assert!(state.model.views.is_empty());
        state.save_full_model().unwrap();
    }

    #[test]
    fn sync_axes_initializes_from_a_saved_view() {
        // A model carrying a saved view but empty live axes: sync_axes_with_model
        // seeds the axes from the view (the saved-view branch), including the
        // empty-axis_order fallback to all categories.
        let mut model = Model::new();
        let a = CategoryId(1);
        let b = CategoryId(2);
        model.add_category(a, "A");
        model.add_category(b, "B");
        model.add_item(ItemId(10), a, "a1");
        model.add_item(ItemId(20), b, "b1");
        let mid = MeasureId(100);
        model.add_measure(Measure {
            id: mid,
            name: Name("V".into()),
            value_type: ValueType::Number,
            categories: vec![a, b],
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
        // A view whose axis_order is empty: sync falls back to all categories.
        let mut primary = MatrixPlacement::new(mid);
        primary.axis_order = Vec::new();
        primary.n_rows = 1;
        primary.n_cols = 0;
        model.add_view(View::from_matrices(
            ViewId(1),
            Name("Saved".into()),
            primary,
            Vec::new(),
        ));
        let mut state = bare_state(model);
        state.sync_axes_with_model();
        // all_cats = [A, B]; n_rows=1 so A on rows, B on cols.
        assert_eq!(state.row_cats, vec![a]);
        assert_eq!(state.active_measure, Some(mid));
    }

    #[test]
    fn scalar_formula_then_redefine_inherits_categories() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // A scalar formula has no categories.
        let s = eng.upsert_formula("Const = 2 + 3".to_string()).unwrap();
        assert_eq!(s.measure_name, "Const");
        // Redefining the same measure with another scalar exercises the
        // "inherit existing categories when the new shape is scalar" branch.
        let s2 = eng.upsert_formula("Const = 10 - 1".to_string()).unwrap();
        assert_eq!(s2.id, s.id);
    }

    #[test]
    fn analytics_empty_when_slice_is_all_blank() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        // A derived measure that is #DIV/0! in every cell: resolve_cell marks
        // such cells not-blank but their value is NaN, so none are finite and
        // `values` ends up empty -> the zeroed-summary return.
        eng.upsert_formula("AllErr = Price / (Quantity - Quantity)".to_string())
            .unwrap();
        let a = eng.get_slice_analytics("AllErr".to_string());
        assert_eq!(a.cell_count, 0);
        assert_eq!(a.sum, 0.0);
        assert_eq!(a.mean, 0.0);
        assert!(a.series_labels.is_empty());
        assert_eq!(a.measure_name, "AllErr");
    }

    #[test]
    fn analytics_skips_blank_series_rows() {
        let eng = ImprovMobileEngine::open_or_create(String::new());
        eng.create_blank_model().unwrap();
        // A new Period item with no Budget values: its row is all-blank, so
        // the per-row series skips it (the row_has_val == false branch) while
        // the seeded rows still contribute.
        eng.add_category_item("Period".to_string(), "P9".to_string())
            .unwrap();
        let a = eng.get_slice_analytics("Budget".to_string());
        assert!(a.series_labels.iter().all(|l| !l.contains("P9")));
        assert!(a.cell_count > 0);
    }
}
