//! Improv core model: categories, items, measures, coordinates, formulas.
//!
//! GUI-free, storage-free. This is the multidimensional "cube": each measure is
//! a tensor indexed by a subset of categories; a coordinate names one cell.
//!
//! Structure follows the project steering doc (AGENT_STEERING.md / IMPROV.txt).

pub mod extfn_def;
pub mod formula;
pub mod ids;
pub mod parser;
pub mod value;

pub use extfn_def::{ExternalFn, Language};
pub use formula::{BinaryOp, DimensionSpec, Expr, Formula, FuncId, UnaryOp};
pub use ids::{CategoryId, ItemId, MeasureId, Name, ViewId};
pub use parser::{parse_expr, parse_formula, FormulaText, ParseError};
pub use value::{Value, ValueError, ValueErrorKind, ValueType};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

/// A dimension of the model (e.g. Time, Product, Region).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: CategoryId,
    pub name: Name,
    pub items: Vec<ItemId>,
}

/// A member of a category (e.g. 2025, "Widget A").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub category: CategoryId,
    pub name: Name,
}

/// A coordinate maps categories to items: one cell of a measure's tensor.
///
/// `BTreeMap` gives a stable, ordered key (important for hashing/serialization
/// and, later, for deriving a fixed differential-dataflow key).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Coordinate {
    pub dims: BTreeMap<CategoryId, ItemId>,
}

impl Coordinate {
    pub fn new() -> Self {
        Coordinate {
            dims: BTreeMap::new(),
        }
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (CategoryId, ItemId)>) -> Self {
        Coordinate {
            dims: pairs.into_iter().collect(),
        }
    }

    pub fn get(&self, cat: CategoryId) -> Option<ItemId> {
        self.dims.get(&cat).copied()
    }

    pub fn with(mut self, cat: CategoryId, item: ItemId) -> Self {
        self.dims.insert(cat, item);
        self
    }

    /// The set of categories this coordinate is defined over.
    pub fn categories(&self) -> impl Iterator<Item = CategoryId> + '_ {
        self.dims.keys().copied()
    }
}

impl Default for Coordinate {
    fn default() -> Self {
        Self::new()
    }
}

/// A measure is either raw input data or derived from a formula.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MeasureKind {
    Input,
    Derived(Formula),
}

/// A named variable defined over one or more categories, e.g. `Revenue[Time, Product]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub id: MeasureId,
    pub name: Name,
    pub value_type: ValueType,
    /// Categories this measure is indexed by (its tensor dimensions).
    pub categories: Vec<CategoryId>,
    pub kind: MeasureKind,
    pub description: Option<String>,
}

impl Measure {
    pub fn is_input(&self) -> bool {
        matches!(self.kind, MeasureKind::Input)
    }
    pub fn is_derived(&self) -> bool {
        matches!(self.kind, MeasureKind::Derived(_))
    }
}

/// The whole model: the multidimensional cube plus its raw input data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub categories: HashMap<CategoryId, Category>,
    pub items: HashMap<ItemId, Item>,
    pub measures: HashMap<MeasureId, Measure>,
    /// Raw input data: `(measure, coordinate) -> value`. Only `Input` measures
    /// have entries here; derived measures are computed by the engine.
    ///
    /// Serialized as a sequence because JSON object keys must be strings and
    /// this map is keyed by a `(MeasureId, Coordinate)` tuple.
    #[serde(with = "inputs_as_seq")]
    pub inputs: HashMap<(MeasureId, Coordinate), Value>,
    /// Metadata marking which input measures are backed by an external SQL
    /// query (Phase 7). The measure's `kind` stays `Input` — to the engine an
    /// SQL measure is an ordinary input whose cells are (re)populated by
    /// refreshing the query, so the deterministic core gains no SQL path. This
    /// map records *how* to refresh; the actual query run lives in
    /// `improv_storage_sql`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sql_sources: HashMap<MeasureId, SqlSource>,
    /// Saved views: named pivot layouts over the model (parity with
    /// Improv/Quantrix "multiple views per model"). A view is presentation, not
    /// modeling — it does not change measures or data.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub views: HashMap<ViewId, View>,
    /// External-language function definitions (Phase 6) available to formulas
    /// via `CALL(name, ...)`, keyed by name. Plain data; the `improv_extfn`
    /// crate evaluates them. Marked non-deterministic-source-adjacent: the
    /// engine treats a call as pure per the author's purity assertion.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_fns: HashMap<String, ExternalFn>,
    /// Measures computed by calling an external function per coordinate over
    /// argument measures (Phase 6). The measure's `kind` stays `Input`; a
    /// host-side refresh (in `improv_engine::external`) runs the function and
    /// populates its cells — the engine's dataflow gains no external-call path,
    /// so its determinism is preserved.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_calls: HashMap<MeasureId, ExternalCall>,
}

/// A measure defined as `func(arg_measures...)` where `func` is a registered
/// external function. Evaluated host-side per coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalCall {
    /// Name of the external function (a key in `Model.external_fns`).
    pub func: String,
    /// The measures supplying the function's positional arguments, aligned to
    /// the function's declared `arg_types`.
    pub arg_measures: Vec<MeasureId>,
}

/// A saved pivot layout: which measure, how its categories are placed on axes,
/// which page items are pinned, and any per-category filters. Reusable across
/// sessions; the interfaces load a view to reproduce a layout without touching
/// formulas or data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct View {
    pub id: ViewId,
    pub name: Name,
    pub measure: MeasureId,
    /// A permutation of the measure's categories: index 0 is on rows, 1 on
    /// columns, the rest are pages. Empty = use the measure's natural order.
    #[serde(default)]
    pub axis_order: Vec<CategoryId>,
    /// For each page (extra) dimension, the pinned item.
    #[serde(default)]
    pub page_items: Vec<(CategoryId, ItemId)>,
    /// Per-category filters: restrict a dimension to this subset of items.
    /// A category absent here is unfiltered (all items shown).
    #[serde(default)]
    pub filters: Vec<Filter>,
}

/// Restrict a category to a subset of its items in a view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub category: CategoryId,
    /// Items to KEEP. An empty list means "no items pass" (an explicit empty
    /// filter); omit the `Filter` entirely to show all items.
    pub items: Vec<ItemId>,
}

impl View {
    /// Does `item` in `category` pass this view's filters? Categories without a
    /// filter always pass.
    pub fn allows(&self, category: CategoryId, item: ItemId) -> bool {
        match self.filters.iter().find(|f| f.category == category) {
            Some(f) => f.items.contains(&item),
            None => true,
        }
    }
}

/// How to refresh an SQL-backed input measure: the query and how its columns
/// map to the measure's dimensions + value. A live-query measure marked here
/// is nondeterministic (its data comes from outside); determinism tests treat
/// only measures WITHOUT an entry here as pure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlSource {
    /// The `SELECT` to run on refresh.
    pub query: String,
    /// Result column names, in the same order as the measure's `categories`,
    /// whose distinct values are the items of each dimension.
    pub dimension_columns: Vec<String>,
    /// Result column holding the numeric value.
    pub value_column: String,
}

/// serde adapter: (de)serialize the tuple-keyed `inputs` map as a `Vec` of
/// `(key, value)` pairs so it survives JSON (and any string-keyed format).
mod inputs_as_seq {
    use super::{Coordinate, MeasureId, Value};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashMap;

    type Map = HashMap<(MeasureId, Coordinate), Value>;

    pub fn serialize<S: Serializer>(map: &Map, s: S) -> Result<S::Ok, S::Error> {
        let v: Vec<(&(MeasureId, Coordinate), &Value)> = map.iter().collect();
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Map, D::Error> {
        let v: Vec<((MeasureId, Coordinate), Value)> = Vec::deserialize(d)?;
        Ok(v.into_iter().collect())
    }
}

impl Model {
    pub fn new() -> Self {
        Model::default()
    }

    // --- builders (return the new id for convenience) ---

    pub fn add_category(&mut self, id: CategoryId, name: impl Into<String>) {
        self.categories.insert(
            id,
            Category {
                id,
                name: Name(name.into()),
                items: Vec::new(),
            },
        );
    }

    pub fn add_item(&mut self, id: ItemId, category: CategoryId, name: impl Into<String>) {
        self.items.insert(
            id,
            Item {
                id,
                category,
                name: Name(name.into()),
            },
        );
        if let Some(c) = self.categories.get_mut(&category) {
            if !c.items.contains(&id) {
                c.items.push(id);
            }
        }
    }

    pub fn add_measure(&mut self, m: Measure) {
        self.measures.insert(m.id, m);
    }

    pub fn set_input(&mut self, measure: MeasureId, coord: Coordinate, value: Value) {
        self.inputs.insert((measure, coord), value);
    }

    pub fn input(&self, measure: MeasureId, coord: &Coordinate) -> Option<&Value> {
        self.inputs.get(&(measure, coord.clone()))
    }

    /// Look up a measure by its human name.
    pub fn measure_by_name(&self, name: &str) -> Option<&Measure> {
        self.measures.values().find(|m| m.name.0 == name)
    }

    /// Look up a category by its human name.
    pub fn category_by_name(&self, name: &str) -> Option<&Category> {
        self.categories.values().find(|c| c.name.0 == name)
    }

    /// Save (or replace) a view.
    pub fn add_view(&mut self, v: View) {
        self.views.insert(v.id, v);
    }

    /// Look up a view by its human name.
    pub fn view_by_name(&self, name: &str) -> Option<&View> {
        self.views.values().find(|v| v.name.0 == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A tiny Time x Product revenue model, matching the steering doc's example.
    fn time_product_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(11), time, "2026");
        m.add_item(ItemId(20), product, "Widget A");
        m.add_item(ItemId(21), product, "Widget B");

        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: Some("Unit price per product".into()),
        });
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m
    }

    #[test]
    fn coordinate_is_order_independent() {
        let a = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
        let b = Coordinate::from_pairs([(CategoryId(2), ItemId(20)), (CategoryId(1), ItemId(10))]);
        assert_eq!(a, b, "BTreeMap key is insertion-order independent");
        assert_eq!(a.get(CategoryId(1)), Some(ItemId(10)));
    }

    #[test]
    fn model_build_and_lookup() {
        let m = time_product_model();
        assert_eq!(m.category_by_name("Time").unwrap().items.len(), 2);
        assert!(m.measure_by_name("Price").unwrap().is_input());
        let coord = Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
        assert_eq!(m.input(MeasureId(100), &coord), Some(&Value::Number(10.0)));
    }

    #[test]
    fn model_round_trips_through_json() {
        let m = time_product_model();
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Model = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back, "model survives a JSON round trip");
    }

    #[test]
    fn views_and_filters() {
        let mut m = time_product_model();
        let (time, product) = (CategoryId(1), CategoryId(2));
        // A view of Price pivoted with Product on rows, filtered to WidgetA.
        m.add_view(View {
            id: ViewId(1),
            name: Name("By Product".into()),
            measure: MeasureId(100),
            axis_order: vec![product, time],
            page_items: vec![],
            filters: vec![Filter {
                category: product,
                items: vec![ItemId(20)], // WidgetA only
            }],
        });
        let v = m.view_by_name("By Product").expect("view");
        assert_eq!(v.axis_order, vec![product, time]);
        // Filter: WidgetA passes, WidgetB does not; unfiltered Time passes all.
        assert!(v.allows(product, ItemId(20)));
        assert!(!v.allows(product, ItemId(21)));
        assert!(v.allows(time, ItemId(10)));

        // Views survive a JSON round trip.
        let json = serde_json::to_string(&m).unwrap();
        let back: Model = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
