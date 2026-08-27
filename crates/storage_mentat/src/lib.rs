//! Persist an Improv `Model` to the embedded (SQLite-backed) Mentat store.
//!
//! The model is stored as datoms using the schema from IMPROV.txt
//! ("Mentat schema (steering version)"): categories, items, measures, and
//! input cells are first-class entities so the model is *queryable*, not an
//! opaque blob.
//!
//! Formulas and coordinates are stored as JSON strings (the AST/coordinate are
//! serialized with serde); everything else maps to native datom value types.

use improv_core_model::{
    Category, CategoryId, Coordinate, Formula, Item, ItemId, Measure, MeasureId, MeasureKind,
    Model, Name, Value, ValueType,
};
use mentat::{Store, TypedValue};
use std::collections::HashMap;

mod convert;
mod schema;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("mentat error: {0}")]
    Mentat(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("data integrity: {0}")]
    Integrity(String),
}

impl From<mentat::errors::MentatError> for StoreError {
    fn from(e: mentat::errors::MentatError) -> Self {
        StoreError::Mentat(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A model store backed by embedded Mentat (a single SQLite file, or `""` for
/// in-memory).
pub struct ModelStore {
    store: Store,
}

impl ModelStore {
    /// Open (or create) a store at `path`. Use `""` for an in-memory database.
    pub fn open(path: &str) -> Result<Self> {
        let mut store = Store::open(path)?;
        store.transact(schema::SCHEMA_EDN)?;
        Ok(ModelStore { store })
    }

    /// Persist the entire model. Idempotent for identity-unique entities
    /// (categories/items/measures keyed by their id; cells keyed by
    /// measure+coord), so re-saving updates in place.
    ///
    /// Transacted in dependency order (categories, then items, then measures,
    /// then cells) as separate transactions so that lookup-refs resolve against
    /// already-committed data.
    pub fn save_model(&mut self, model: &Model) -> Result<()> {
        self.transact_group(model.categories.values().map(convert::category_edn))?;
        self.transact_group(model.items.values().map(convert::item_edn))?;

        let mut measures = Vec::new();
        for m in model.measures.values() {
            measures.push(convert::measure_edn(m, model.sql_sources.get(&m.id))?);
        }
        self.transact_group(measures)?;

        let mut cells = Vec::new();
        for ((mid, coord), val) in model.inputs.iter() {
            cells.push(convert::cell_edn(*mid, coord, val)?);
        }
        self.transact_group(cells)?;

        let mut views = Vec::new();
        for v in model.views.values() {
            let json = serde_json::to_string(v)?;
            views.push(format!(
                "{{:view/id {} :view/json {}}}",
                v.id.0,
                convert::edn_str_pub(&json)
            ));
        }
        self.transact_group(views)?;

        // Singleton meta: external function defs + external-call measures, each
        // as a JSON blob on one entity (only if non-empty).
        if !model.external_fns.is_empty() || !model.external_calls.is_empty() {
            let fns_json = serde_json::to_string(&model.external_fns)?;
            let calls_json = serde_json::to_string(&model.external_calls)?;
            let edn = format!(
                "[{{:meta/singleton 1 :meta/external-fns {} :meta/external-calls {}}}]",
                convert::edn_str_pub(&fns_json),
                convert::edn_str_pub(&calls_json),
            );
            self.store.transact(&edn)?;
        }
        Ok(())
    }

    fn transact_group(&mut self, entities: impl IntoIterator<Item = String>) -> Result<()> {
        let parts: Vec<String> = entities.into_iter().collect();
        if parts.is_empty() {
            return Ok(());
        }
        let edn = format!("[{}]", parts.join("\n"));
        self.store.transact(&edn)?;
        Ok(())
    }

    /// Reconstruct the model by querying the store.
    pub fn load_model(&mut self) -> Result<Model> {
        let mut model = Model::new();
        self.load_categories(&mut model)?;
        self.load_items(&mut model)?;
        self.load_measures(&mut model)?;
        self.load_cells(&mut model)?;
        self.load_views(&mut model)?;
        self.load_meta(&mut model)?;
        Ok(model)
    }

    // --- load: query each entity kind ---

    fn load_categories(&mut self, model: &mut Model) -> Result<()> {
        let q = "[:find ?id ?name :where [?e :category/id ?id] [?e :category/name ?name]]";
        for row in self.rel(q)? {
            let id = CategoryId(convert::as_u32(&row[0])?);
            let name = convert::as_string(&row[1])?;
            model.categories.insert(
                id,
                Category {
                    id,
                    name: Name(name),
                    items: Vec::new(),
                },
            );
        }
        Ok(())
    }

    fn load_items(&mut self, model: &mut Model) -> Result<()> {
        let q = "[:find ?id ?cat ?name :where \
                  [?e :item/id ?id] \
                  [?e :item/category ?c] [?c :category/id ?cat] \
                  [?e :item/name ?name]]";
        for row in self.rel(q)? {
            let id = ItemId(convert::as_u32(&row[0])?);
            let category = CategoryId(convert::as_u32(&row[1])?);
            let name = convert::as_string(&row[2])?;
            model.items.insert(
                id,
                Item {
                    id,
                    category,
                    name: Name(name),
                },
            );
            if let Some(c) = model.categories.get_mut(&category) {
                c.items.push(id);
            }
        }
        Ok(())
    }

    fn load_measures(&mut self, model: &mut Model) -> Result<()> {
        // Required fields only; optional formula/description fetched per measure.
        let q = "[:find ?id ?name ?vt ?kind :where \
                  [?e :measure/id ?id] \
                  [?e :measure/name ?name] \
                  [?e :measure/value-type ?vt] \
                  [?e :measure/kind ?kind]]";
        for row in self.rel(q)? {
            let id = MeasureId(convert::as_u32(&row[0])?);
            let name = convert::as_string(&row[1])?;
            let value_type = convert::value_type_from_kw(&convert::as_string(&row[2])?)?;
            let kind_kw = convert::as_string(&row[3])?;

            let kind = match kind_kw.as_str() {
                "input" => MeasureKind::Input,
                "derived" => {
                    let formula_json = self
                        .scalar_string(&format!(
                            "[:find ?f . :where [?e :measure/id {}] [?e :measure/formula ?f]]",
                            id.0
                        ))?
                        .ok_or_else(|| {
                            StoreError::Integrity(format!(
                                "derived measure {} has no formula",
                                id.0
                            ))
                        })?;
                    let f: Formula = serde_json::from_str(&formula_json)?;
                    MeasureKind::Derived(f)
                }
                other => {
                    return Err(StoreError::Integrity(format!(
                        "unknown measure kind {other}"
                    )))
                }
            };

            let description = self.scalar_string(&format!(
                "[:find ?d . :where [?e :measure/id {}] [?e :measure/description ?d]]",
                id.0
            ))?;

            let categories = self.load_measure_categories(id)?;

            // SQL-source metadata (Phase 7), if this measure is SQL-backed.
            if let Some(json) = self.scalar_string(&format!(
                "[:find ?s . :where [?e :measure/id {}] [?e :measure/sql-source ?s]]",
                id.0
            ))? {
                let src: improv_core_model::SqlSource = serde_json::from_str(&json)?;
                model.sql_sources.insert(id, src);
            }

            model.measures.insert(
                id,
                Measure {
                    id,
                    name: Name(name),
                    value_type,
                    categories,
                    kind,
                    description,
                },
            );
        }
        Ok(())
    }

    fn load_measure_categories(&mut self, measure: MeasureId) -> Result<Vec<CategoryId>> {
        let q = format!(
            "[:find ?cat :where \
             [?e :measure/id {}] [?e :measure/categories ?c] [?c :category/id ?cat]]",
            measure.0
        );
        let mut cats = Vec::new();
        for row in self.rel(&q)? {
            cats.push(CategoryId(convert::as_u32(&row[0])?));
        }
        Ok(cats)
    }

    fn load_cells(&mut self, model: &mut Model) -> Result<()> {
        // Get all cells' measure id + coord, then fetch the typed value by the
        // owning measure's declared type (avoids optional-attribute functions).
        let q = "[:find ?mid ?coord :where \
                  [?e :cell/measure ?m] [?m :measure/id ?mid] \
                  [?e :cell/coord ?coord]]";
        for row in self.rel(q)? {
            let mid = MeasureId(convert::as_u32(&row[0])?);
            let coord_json = convert::as_string(&row[1])?;
            let coord: Coordinate = serde_json::from_str(&coord_json)?;

            let vt = model
                .measures
                .get(&mid)
                .map(|m| m.value_type)
                .ok_or_else(|| {
                    StoreError::Integrity(format!("cell for unknown measure {mid:?}"))
                })?;

            // The cell entity is uniquely keyed; query its typed value column.
            let (col, ekey) = match vt {
                ValueType::Number | ValueType::Enum => ("value-number", "?v"),
                ValueType::Boolean => ("value-boolean", "?v"),
                ValueType::Text | ValueType::DateTime => ("value-text", "?v"),
            };
            let key = format!("{}::{}", mid.0, coord_json);
            let vq = format!(
                "[:find {ekey} . :where [?e :cell/key {}] [?e :cell/{col} ?v]]",
                convert::edn_str_pub(&key)
            );
            let val_tv = self
                .scalar(&vq)?
                .ok_or_else(|| StoreError::Integrity("cell missing value".into()))?;

            let value = match vt {
                ValueType::Number => Value::Number(convert::as_f64(&val_tv)?),
                ValueType::Enum => Value::Enum(convert::as_f64(&val_tv)? as u32),
                ValueType::Boolean => Value::Boolean(convert::as_bool(&val_tv)?),
                ValueType::Text | ValueType::DateTime => Value::Text(convert::as_string(&val_tv)?),
            };
            model.inputs.insert((mid, coord), value);
        }
        Ok(())
    }

    fn load_views(&mut self, model: &mut Model) -> Result<()> {
        let q = "[:find ?json :where [?e :view/id _] [?e :view/json ?json]]";
        for row in self.rel(q)? {
            let json = convert::as_string(&row[0])?;
            let v: improv_core_model::View = serde_json::from_str(&json)?;
            model.views.insert(v.id, v);
        }
        Ok(())
    }

    fn load_meta(&mut self, model: &mut Model) -> Result<()> {
        if let Some(json) = self.scalar_string(
            "[:find ?j . :where [?e :meta/singleton 1] [?e :meta/external-fns ?j]]",
        )? {
            model.external_fns = serde_json::from_str(&json)?;
        }
        if let Some(json) = self.scalar_string(
            "[:find ?j . :where [?e :meta/singleton 1] [?e :meta/external-calls ?j]]",
        )? {
            model.external_calls = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    fn scalar(&self, query: &str) -> Result<Option<TypedValue>> {
        use mentat::Queryable;
        let out = self.store.q_once(query, None)?;
        let s = out
            .results
            .into_scalar()
            .map_err(|e| StoreError::Mentat(e.to_string()))?;
        Ok(s.and_then(|b| b.into_scalar()))
    }

    fn scalar_string(&self, query: &str) -> Result<Option<String>> {
        match self.scalar(query)? {
            Some(tv) => Ok(Some(convert::as_string(&tv)?)),
            None => Ok(None),
        }
    }

    fn rel(&self, query: &str) -> Result<Vec<Vec<TypedValue>>> {
        use mentat::Queryable;
        let out = self.store.q_once(query, None)?;
        let rel = out
            .results
            .into_rel()
            .map_err(|e| StoreError::Mentat(e.to_string()))?;
        Ok(rel
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|b| b.into_scalar().expect("scalar binding"))
                    .collect()
            })
            .collect())
    }
}

// Silence unused-import warnings for HashMap in case the module trims; it's used
// transitively by Model. (Kept explicit for readability of the load path.)
#[allow(unused_imports)]
use HashMap as _HashMap;

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{BinaryOp, DimensionSpec, Expr, ItemId};

    fn sample_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(20), product, "Widget A");

        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: Some("Unit price".into()),
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
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m.add_view(improv_core_model::View {
            id: improv_core_model::ViewId(1),
            name: Name("Prices by product".into()),
            measure: MeasureId(100),
            axis_order: vec![CategoryId(2)],
            page_items: vec![],
            filters: vec![improv_core_model::Filter {
                category: CategoryId(2),
                items: vec![ItemId(20)],
            }],
        });
        m
    }

    #[test]
    fn save_then_load_round_trips() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let original = sample_model();
        store.save_model(&original).expect("save");
        let loaded = store.load_model().expect("load");

        assert_eq!(loaded.categories.len(), 2);
        assert_eq!(loaded.items.len(), 2);
        assert_eq!(loaded.measures.len(), 2);
        assert_eq!(loaded.inputs.len(), 1);

        // Derived measure formula survived the JSON-in-datom round trip.
        let rev = loaded.measure_by_name("Revenue").expect("revenue");
        assert!(rev.is_derived());
        assert_eq!(rev.categories.len(), 2);

        // Input cell value survived.
        let coord = Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
        assert_eq!(
            loaded.input(MeasureId(100), &coord),
            Some(&Value::Number(10.0))
        );

        // The saved view (layout + filter) survived the round trip.
        let v = loaded.view_by_name("Prices by product").expect("view");
        assert_eq!(v.measure, MeasureId(100));
        assert_eq!(v.axis_order, vec![CategoryId(2)]);
        assert!(v.allows(CategoryId(2), ItemId(20)));
        assert!(!v.allows(CategoryId(2), ItemId(21)));
    }
}
