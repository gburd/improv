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
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

use improv_core_model::{
    Category, CategoryId, Item, ItemId, Measure, MeasureId, MeasureKind, Model, Name, ValueType,
};
use std::collections::{HashMap, HashSet};

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
        format: None,
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
    /// Seed the interner from `model`'s existing items. Minted ids start at
    /// `item_id_base`, but never below `max(existing item id) + 1`: on a repeat
    /// import the caller passes the same fixed base (the GUI wizard always
    /// passes 1_000_000), so allocating straight from the base would re-mint an
    /// id a *differently named* item already owns — `register` then keeps the
    /// old name and the new row's value silently lands under the wrong label.
    /// Allocating above the model's maximum guarantees freshness while the base
    /// still acts as the floor for a fresh import.
    pub fn seeded_from(model: &Model, item_id_base: u32) -> Self {
        let mut map: HashMap<CategoryId, HashMap<String, ItemId>> = HashMap::new();
        let mut max_id = 0u32;
        for it in model.items.values() {
            map.entry(it.category)
                .or_default()
                .insert(it.name.0.clone(), it.id);
            max_id = max_id.max(it.id.0);
        }
        Self {
            next_item: item_id_base.max(max_id.saturating_add(1)),
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
            // The category's list as a set, built once: a `Vec::contains`
            // per item made a wide category O(n²).
            let mut listed: HashSet<ItemId> = model
                .categories
                .get(cat)
                .map(|c| c.items.iter().copied().collect())
                .unwrap_or_default();
            for (name, id) in items {
                model.items.entry(*id).or_insert_with(|| Item {
                    id: *id,
                    category: *cat,
                    name: Name(name.clone()),
                });
                if let Some(c) = model.categories.get_mut(cat) {
                    if listed.insert(*id) {
                        c.items.push(*id);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::{Coordinate, ItemId};

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
    fn register_lists_each_item_once_and_skips_unknown_categories() {
        let mut model = Model::new();
        let (cat, gone) = (CategoryId(1), CategoryId(9));
        ensure_categories(&mut model, [(cat, "Time")]);
        model.items.insert(
            ItemId(1),
            Item {
                id: ItemId(1),
                category: cat,
                name: Name("2025".into()),
            },
        );
        model
            .categories
            .get_mut(&cat)
            .unwrap()
            .items
            .push(ItemId(1));
        let mut interner = ItemInterner::seeded_from(&model, 10);
        interner.intern(cat, "2025");
        let (a, b) = (interner.intern(cat, "2026"), interner.intern(cat, "2027"));
        let orphan = interner.intern(gone, "x");
        interner.register(&mut model);
        interner.register(&mut model); // a second register adds nothing
        let mut listed = model.categories[&cat].items.clone();
        assert_eq!(listed[0], ItemId(1), "the existing item keeps its place");
        listed.sort();
        assert_eq!(listed, [ItemId(1), a, b]);
        // No category: the item is recorded, no category appears.
        assert_eq!(model.items[&orphan].category, gone);
        assert!(!model.categories.contains_key(&gone));
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
    fn mints_above_existing_ids_even_when_base_is_lower() {
        // A prior import left item 1_000_000 ⇒ re-seeding with the same fixed
        // base must not hand that id to a different name.
        let mut model = Model::new();
        let cat = CategoryId(1);
        ensure_categories(&mut model, [(cat, "Time")]);
        model.items.insert(
            ItemId(1_000_000),
            Item {
                id: ItemId(1_000_000),
                category: cat,
                name: Name("2025".into()),
            },
        );

        let mut interner = ItemInterner::seeded_from(&model, 1_000_000);
        let fresh = interner.intern(cat, "2026");
        assert_ne!(fresh, ItemId(1_000_000));
        interner.register(&mut model);
        assert_eq!(model.items.get(&ItemId(1_000_000)).unwrap().name.0, "2025");
        assert_eq!(model.items.get(&fresh).unwrap().name.0, "2026");
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

    #[test]
    fn register_skips_known_items_and_unknown_categories() {
        let mut model = Model::new();
        model.add_category(CategoryId(1), "Time");
        model.add_item(ItemId(5), CategoryId(1), "2025");
        let mut interner = ItemInterner::seeded_from(&model, 100);
        // Seeded: re-interning "2025" reuses id 5, already on the category.
        assert_eq!(interner.intern(CategoryId(1), "2025"), ItemId(5));
        let orphan = interner.intern(CategoryId(9), "x");
        interner.register(&mut model);
        assert_eq!(model.categories[&CategoryId(1)].items, [ItemId(5)]);
        // An item for a category the model lacks is stored, attached to nothing.
        assert_eq!(model.items[&orphan].category, CategoryId(9));
        assert!(!model.categories.contains_key(&CategoryId(9)));
    }
}
