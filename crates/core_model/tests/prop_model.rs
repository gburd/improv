//! Property tests for the core model.
//!
//! Properties:
//! 1. A random Model (categories/items/measures/number inputs) survives a
//!    serde JSON round trip unchanged.
//! 2. A Coordinate built from pairs is order-independent, and `get` returns the
//!    inserted item for every category.
//! 3. Every finite number prints as a literal (`1e-10`, `6.02e23`) that
//!    parses back to the same `f64`, alone and inside an expression.

use improv_core_model::parser::number_literal;
use improv_core_model::{
    parse_expr, BinaryOp, CategoryId, Coordinate, Expr, Item, ItemId, Measure, MeasureId,
    MeasureKind, Model, Name, UnaryOp, Value, ValueType,
};
use proptest::prelude::*;

/// Strategy for a small Model with unique category / item / measure ids and
/// number-valued input cells. Kept small so JSON round trips stay cheap.
fn arb_model() -> impl Strategy<Value = Model> {
    // Unique category ids, each with a handful of unique item ids.
    let cats = prop::collection::hash_set(0u32..20, 1..5);
    cats.prop_flat_map(|cat_ids| {
        let cat_ids: Vec<u32> = cat_ids.into_iter().collect();
        // For each category, pick a distinct set of item ids (globally unique
        // by offsetting per category so items never collide across categories).
        let items_per_cat =
            prop::collection::vec(prop::collection::hash_set(0u32..8, 0..5), cat_ids.len());
        // Measures: unique ids, each indexed by a random subset of categories.
        let measures = prop::collection::hash_set(100u32..130, 0..4);
        (Just(cat_ids), items_per_cat, measures).prop_flat_map(|(cat_ids, items, measure_ids)| {
            let measure_ids: Vec<u32> = measure_ids.into_iter().collect();
            // Which categories each measure spans (subset of cat_ids), plus a
            // finite number value per (measure, coord) input.
            let per_measure = prop::collection::vec(
                (
                    prop::sample::subsequence(cat_ids.clone(), 0..=cat_ids.len()),
                    // Integer-valued f64: exactly representable, so JSON
                    // round-trips bit-for-bit (serde_json reformats fractional
                    // f64 lossily, which is out of scope here).
                    (-1_000_000i64..1_000_000).prop_map(|n| n as f64),
                ),
                measure_ids.len(),
            );
            (Just(cat_ids), Just(items), Just(measure_ids), per_measure).prop_map(
                |(cat_ids, items, measure_ids, per_measure)| {
                    let mut m = Model::new();
                    for &c in &cat_ids {
                        m.add_category(CategoryId(c), format!("Cat{c}"));
                    }
                    // Distinct item ids: encode as cat_index * 100 + local.
                    for (ci, item_set) in items.iter().enumerate() {
                        let cat = cat_ids[ci];
                        for &local in item_set {
                            let iid = (ci as u32) * 100 + local;
                            m.add_item(ItemId(iid), CategoryId(cat), format!("Item{iid}"));
                        }
                    }
                    for (mi, &mid) in measure_ids.iter().enumerate() {
                        let (cats, val) = &per_measure[mi];
                        m.add_measure(Measure {
                            id: MeasureId(mid),
                            name: Name(format!("M{mid}")),
                            value_type: ValueType::Number,
                            categories: cats.iter().map(|&c| CategoryId(c)).collect(),
                            kind: MeasureKind::Input,
                            description: None,
                        });
                        // One input at the coordinate using each spanned
                        // category's first known item (if any).
                        let coord = Coordinate::from_pairs(cats.iter().filter_map(|&c| {
                            let ci = cat_ids.iter().position(|&x| x == c).unwrap();
                            items[ci]
                                .iter()
                                .min()
                                .map(|&local| (CategoryId(c), ItemId((ci as u32) * 100 + local)))
                        }));
                        m.set_input(MeasureId(mid), coord, Value::Number(*val));
                    }
                    m
                },
            )
        })
    })
}

proptest! {
    #[test]
    fn model_json_round_trip(m in arb_model()) {
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Model = serde_json::from_str(&json).expect("deserialize");
        prop_assert_eq!(m, back);
    }

    /// Coordinate::from_pairs over UNIQUE categories is order-independent, and
    /// every inserted (cat, item) is retrievable via `get`. Unique categories
    /// avoid last-write-wins ambiguity, isolating the ordering property.
    #[test]
    fn coordinate_order_independent(
        cats in prop::collection::hash_set(0u32..50, 0..10),
        items in prop::collection::vec(0u32..50, 10),
    ) {
        let pairs: Vec<(u32, u32)> = cats.iter().copied().zip(items).collect();
        let forward = Coordinate::from_pairs(
            pairs.iter().map(|&(c, i)| (CategoryId(c), ItemId(i))),
        );
        let reversed = Coordinate::from_pairs(
            pairs.iter().rev().map(|&(c, i)| (CategoryId(c), ItemId(i))),
        );
        prop_assert_eq!(&forward, &reversed, "unique-category coord is order independent");
        for &(c, i) in &pairs {
            prop_assert_eq!(forward.get(CategoryId(c)), Some(ItemId(i)));
        }
        prop_assert_eq!(forward.dims.len(), pairs.len());
    }
}

proptest! {
    /// `add_item` lists exactly what the `Vec::contains` version listed, in
    /// the same order, through re-adds, moves and unknown categories.
    #[test]
    fn add_item_lists_like_the_scan(adds in prop::collection::vec((0u32..12, 0u32..4), 0..60)) {
        let mut m = Model::new();
        for c in 0..3 {
            m.add_category(CategoryId(c), format!("C{c}"));
        }
        let mut want: Vec<Vec<ItemId>> = vec![Vec::new(); 3];
        for (i, c) in adds {
            m.add_item(ItemId(i), CategoryId(c), format!("I{i}"));
            if let Some(l) = want.get_mut(c as usize) {
                if !l.contains(&ItemId(i)) {
                    l.push(ItemId(i));
                }
            }
        }
        for (c, l) in (0..).zip(&want) {
            prop_assert_eq!(&m.categories[&CategoryId(c)].items, l);
        }
    }
}

proptest! {
    /// parse(print(n)) == n for every finite number: whole, fractional,
    /// huge, tiny and subnormal. A negative one prints as a negation.
    #[test]
    fn number_literals_round_trip(
        n in prop_oneof![
            (0u32..100_000).prop_map(f64::from),
            prop::num::f64::NORMAL | prop::num::f64::SUBNORMAL | prop::num::f64::ZERO,
        ],
        e in -330i32..330,
    ) {
        let m = Model::new();
        let lit = |n: f64| Expr::Literal(Value::Number(n));
        let read = |text: &str| parse_expr(&m, text).map(|f| f.expr);
        let text = number_literal(n.abs()).expect("finite");
        let want = if n < 0.0 { Expr::UnaryOp(UnaryOp::Neg, Box::new(lit(-n))) } else { lit(n) };
        let src = if n < 0.0 { format!("-{text}") } else { text.clone() };
        prop_assert_eq!(read(&src), Ok(want));
        // In an expression, `+` after an exponent stays an operator.
        prop_assert_eq!(
            read(&format!("{text}+{text}")),
            Ok(Expr::BinaryOp(BinaryOp::Add, Box::new(lit(n.abs())), Box::new(lit(n.abs()))))
        );
        // A typed exponent reads as Rust's own parse (or overflows, an error).
        let typed = format!("7.5e{e}");
        let want: f64 = typed.parse().unwrap();
        match read(&typed) {
            Ok(got) => prop_assert_eq!(got, lit(want)),
            Err(err) => prop_assert!(want.is_infinite(), "{typed}: {err}"),
        }
    }
}

/// One edit of the measures, straight through the pub map or the builder.
#[derive(Debug, Clone)]
enum Edit {
    Add(u32, u32),
    Insert(u32, u32),
    Rename(u32, u32),
    Remove(u32),
    Lookup(u32),
}

fn arb_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        (0u32..6, 0u32..6).prop_map(|(i, n)| Edit::Add(i, n)),
        (0u32..6, 0u32..6).prop_map(|(i, n)| Edit::Insert(i, n)),
        (0u32..6, 0u32..6).prop_map(|(i, n)| Edit::Rename(i, n)),
        (0u32..6).prop_map(Edit::Remove),
        (0u32..6).prop_map(Edit::Lookup),
    ]
}

proptest! {
    /// Whatever the edits, `measure_by_name` answers like the scan it
    /// replaced: a measure of that name when there is one, else none.
    #[test]
    fn measure_by_name_agrees_with_a_scan(edits in prop::collection::vec(arb_edit(), 0..40)) {
        let measure = |id: u32, n: u32| Measure {
            id: MeasureId(id),
            name: Name(format!("N{n}")),
            value_type: ValueType::Number,
            categories: Vec::new(),
            kind: MeasureKind::Input,
            description: None,
        };
        let mut m = Model::new();
        for e in edits {
            match e {
                Edit::Add(i, n) => m.add_measure(measure(i, n)),
                Edit::Insert(i, n) => {
                    m.measures.insert(MeasureId(i), measure(i, n));
                }
                Edit::Rename(i, n) => {
                    if let Some(x) = m.measures.get_mut(&MeasureId(i)) {
                        x.name = Name(format!("N{n}"));
                    }
                }
                Edit::Remove(i) => {
                    m.measures.remove(&MeasureId(i));
                }
                Edit::Lookup(n) => {
                    m.measure_by_name(&format!("N{n}"));
                }
            }
            for n in 0..6 {
                let name = format!("N{n}");
                let found = m.measure_by_name(&name);
                let any = m.measures.values().any(|x| x.name.0 == name);
                prop_assert_eq!(found.is_some(), any);
                if let Some(x) = found {
                    prop_assert_eq!(&x.name.0, &name);
                    prop_assert_eq!(m.measures.get(&x.id), Some(x));
                }
            }
        }
    }
}

// Keep `Item` referenced so an unused-import lint never fires if the strategy
// changes; also a trivial structural check that add_item wires the category.
#[test]
fn add_item_registers_in_category() {
    let mut m = Model::new();
    m.add_category(CategoryId(1), "C");
    m.add_item(ItemId(5), CategoryId(1), "I");
    let _typecheck: Option<&Item> = m.items.get(&ItemId(5));
    assert!(m.categories[&CategoryId(1)].items.contains(&ItemId(5)));
}
