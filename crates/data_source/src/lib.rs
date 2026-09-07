//! Shared "data source" bookkeeping for Improv's import/export backends
//! (`improv_storage_sql`, `improv_storage_csv`, and future formats).
//!
//! Post-v0.5.0 Phase D: `improv_storage_sql::import_query` and
//! `improv_storage_csv::import_csv_from` independently reinvented the same
//! ~40-line block — ensure the mapped categories exist, create the input
//! measure, intern dimension values into item ids (reusing existing items by
//! name on re-import), then register the interned items on the model. That
//! block is genuinely format-agnostic: it only touches `CategoryId` +
//! `MeasureId` + names, never a source-specific column reference or row type.
//! This crate extracts it so a third tabular format (Parquet, JSON-lines, …)
//! reuses it instead of copying it a third time.
//!
//! What deliberately did NOT move: `ImportSpec`/`DimensionMapping` in each
//! backend keep their own shape (SQL's `column` is a plain query-result name
//! plus a `refresh_policy`; CSV's is a name-or-index `ColumnRef` plus a
//! `value_type`) — unifying those would touch the backends' public field
//! types, which ripples into `improv_cli`'s call sites (out of scope, and not
//! worth it: the two shapes differ for real reasons, not by accident). There
//! is also no shared `DataSource` trait: SQL's source is an open connection,
//! CSV's is a path/reader, and nothing in the workspace calls either backend
//! generically — a trait with no generic caller is indirection, not reuse.

use improv_core_model::{
    Category, CategoryId, Item, ItemId, Measure, MeasureId, MeasureKind, Model, Name, ValueType,
};
use std::collections::HashMap;

/// Ensure a category exists for each `(id, name)` pair. Idempotent: an
/// existing category (e.g. from a prior import) is left untouched.
pub fn ensure_categories<'a>(
    model: &mut Model,
    dims: impl IntoIterator<Item = (CategoryId, &'a str)>,
) {
    for (id, name) in dims {
        model.categories.entry(id).or_insert_with(|| Category {
            id,
            name: Name(name.to_string()),
            items: Vec::new(),
        });
    }
}

/// Create (or overwrite) the input measure an import populates.
pub fn add_input_measure(
    model: &mut Model,
    measure_id: MeasureId,
    measure_name: &str,
    value_type: ValueType,
    categories: Vec<CategoryId>,
    description: Option<String>,
) {
    model.add_measure(Measure {
        id: measure_id,
        name: Name(measure_name.to_string()),
        value_type,
        categories,
        kind: MeasureKind::Input,
        description,
    });
}

/// Per-category `name -> ItemId` interner used while importing rows: dimension
/// values are looked up (reusing an existing item of that name) or minted a
/// fresh sequential id. Seed from the model's current items so re-import /
/// refresh reuses items instead of creating duplicates with the same name.
#[derive(Debug, Default)]
pub struct ItemInterner {
    next_item: u32,
    map: HashMap<CategoryId, HashMap<String, ItemId>>,
}

impl ItemInterner {
    /// Seed the interner from `model`'s existing items; new ids start at
    /// `item_id_base`.
    pub fn seeded_from(model: &Model, item_id_base: u32) -> Self {
        let mut map: HashMap<CategoryId, HashMap<String, ItemId>> = HashMap::new();
        for it in model.items.values() {
            map.entry(it.category)
                .or_default()
                .insert(it.name.0.clone(), it.id);
        }
        Self {
            next_item: item_id_base,
            map,
        }
    }

    /// Look up (or mint) the item id for `name` under `category`.
    pub fn intern(&mut self, category: CategoryId, name: &str) -> ItemId {
        if let Some(id) = self.map.entry(category).or_default().get(name) {
            return *id;
        }
        let id = ItemId(self.next_item);
        self.next_item += 1;
        self.map
            .get_mut(&category)
            .expect("just inserted by or_default above")
            .insert(name.to_string(), id);
        id
    }

    /// Register every interned item into `model`'s `items`/`categories` maps.
    /// Call once, after all rows are interned.
    pub fn register(&self, model: &mut Model) {
        for (cat, items) in &self.map {
            for (name, id) in items {
                model.items.entry(*id).or_insert_with(|| Item {
                    id: *id,
                    category: *cat,
                    name: Name(name.clone()),
                });
                if let Some(c) = model.categories.get_mut(cat) {
                    if !c.items.contains(id) {
                        c.items.push(*id);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::Coordinate;

    #[test]
    fn ensure_categories_is_idempotent_and_preserves_existing_items() {
        let mut model = Model::new();
        let cat = CategoryId(1);
        ensure_categories(&mut model, [(cat, "Time")]);
        model
            .categories
            .get_mut(&cat)
            .unwrap()
            .items
            .push(ItemId(1));
        // Calling again must not reset the category (or its items).
        ensure_categories(&mut model, [(cat, "Time")]);
        assert_eq!(model.categories.get(&cat).unwrap().items, vec![ItemId(1)]);
    }

    #[test]
    fn interner_reuses_existing_items_by_name_and_mints_new_ones() {
        let mut model = Model::new();
        let cat = CategoryId(1);
        ensure_categories(&mut model, [(cat, "Time")]);
        model.items.insert(
            ItemId(1),
            Item {
                id: ItemId(1),
                category: cat,
                name: Name("2025".into()),
            },
        );

        let mut interner = ItemInterner::seeded_from(&model, 1000);
        let existing = interner.intern(cat, "2025");
        assert_eq!(existing, ItemId(1), "reuses the pre-existing item");
        let fresh1 = interner.intern(cat, "2026");
        let fresh2 = interner.intern(cat, "2026");
        assert_eq!(fresh1, fresh2, "same name interns to the same id");
        assert_ne!(fresh1, ItemId(1));

        interner.register(&mut model);
        assert_eq!(model.categories.get(&cat).unwrap().items.len(), 2);
        assert!(model.items.contains_key(&fresh1));
    }

    #[test]
    fn add_input_measure_creates_an_input_measure_over_the_categories() {
        let mut model = Model::new();
        let cat = CategoryId(1);
        add_input_measure(
            &mut model,
            MeasureId(1),
            "Revenue",
            ValueType::Number,
            vec![cat],
            Some("test".into()),
        );
        let m = model.measures.get(&MeasureId(1)).unwrap();
        assert!(m.is_input());
        assert_eq!(m.categories, vec![cat]);
        // Sanity: a coordinate over that category can key an input cell.
        let coord = Coordinate::new().with(cat, ItemId(1));
        model.set_input(
            MeasureId(1),
            coord.clone(),
            improv_core_model::Value::Number(1.0),
        );
        assert_eq!(
            model.input(MeasureId(1), &coord),
            Some(&improv_core_model::Value::Number(1.0))
        );
    }
}
