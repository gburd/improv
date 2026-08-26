//! Determinism oracle tests for the Improv engine.
//!
//! Part of the default suite (NOT ignored), kept fast with small models:
//!  * run-to-run byte-for-byte identity of the full result map;
//!  * insertion-order independence of `set_input` calls;
//!  * a proptest: for random small numeric Price/Quantity models,
//!    `Revenue[t,p] == price[p] * qty[t,p]` for every produced cell, and the
//!    produced key set is exactly the Cartesian product where both inputs exist.

use improv_core_model::{
    BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure, MeasureId,
    MeasureKind, Model, Name, Value, ValueType,
};
use improv_engine::dataflow::evaluate;
use improv_engine::CoordKey;
use proptest::prelude::*;
use std::collections::HashMap;

const TIME: CategoryId = CategoryId(1);
const PRODUCT: CategoryId = CategoryId(2);
const PRICE: MeasureId = MeasureId(100);
const QUANTITY: MeasureId = MeasureId(101);
const REVENUE: MeasureId = MeasureId(102);

fn time_item(t: u32) -> ItemId {
    ItemId(10 + t)
}
fn product_item(p: u32) -> ItemId {
    ItemId(1000 + p)
}

fn key(pairs: &[(u32, u32)]) -> CoordKey {
    let mut k: Vec<(u32, u32)> = pairs.to_vec();
    k.sort();
    k
}

/// Build the schema (categories, items, measures) for an `n_time x m_product`
/// Time x Product revenue model. Inputs are added separately so tests can vary
/// their insertion order.
fn build_schema(n_time: u32, m_product: u32) -> Model {
    let mut m = Model::new();
    m.add_category(TIME, "Time");
    m.add_category(PRODUCT, "Product");
    for t in 0..n_time {
        m.add_item(time_item(t), TIME, format!("t{t}"));
    }
    for p in 0..m_product {
        m.add_item(product_item(p), PRODUCT, format!("p{p}"));
    }
    m.add_measure(Measure {
        id: PRICE,
        name: Name("Price".into()),
        value_type: ValueType::Number,
        categories: vec![PRODUCT],
        kind: MeasureKind::Input,
        description: None,
    });
    m.add_measure(Measure {
        id: QUANTITY,
        name: Name("Quantity".into()),
        value_type: ValueType::Number,
        categories: vec![TIME, PRODUCT],
        kind: MeasureKind::Input,
        description: None,
    });
    m.add_measure(Measure {
        id: REVENUE,
        name: Name("Revenue".into()),
        value_type: ValueType::Number,
        categories: vec![TIME, PRODUCT],
        kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
            BinaryOp::Mul,
            Box::new(Expr::Ref(PRICE, DimensionSpec::default())),
            Box::new(Expr::Ref(QUANTITY, DimensionSpec::default())),
        ))),
        description: None,
    });
    m
}

/// A canonical small model with fully-populated inputs (integer-valued).
fn canonical_model(n_time: u32, m_product: u32) -> Model {
    let mut m = build_schema(n_time, m_product);
    for p in 0..m_product {
        m.set_input(
            PRICE,
            Coordinate::from_pairs([(PRODUCT, product_item(p))]),
            Value::Number((1 + p) as f64),
        );
    }
    for t in 0..n_time {
        for p in 0..m_product {
            m.set_input(
                QUANTITY,
                Coordinate::from_pairs([(TIME, time_item(t)), (PRODUCT, product_item(p))]),
                Value::Number((1 + t * 2 + p) as f64),
            );
        }
    }
    m
}

/// Compare two result maps for exact byte-for-byte f64 identity.
fn assert_maps_bit_identical(
    a: &HashMap<MeasureId, HashMap<CoordKey, improv_engine::CellValue>>,
    b: &HashMap<MeasureId, HashMap<CoordKey, improv_engine::CellValue>>,
) {
    let ka: std::collections::BTreeSet<_> = a.keys().collect();
    let kb: std::collections::BTreeSet<_> = b.keys().collect();
    assert_eq!(ka, kb, "same measure ids");
    for mid in ka {
        let ma = &a[mid];
        let mb = &b[mid];
        assert_eq!(ma.len(), mb.len(), "same cell count for {mid:?}");
        for (k, va) in ma {
            let vb = mb
                .get(k)
                .unwrap_or_else(|| panic!("missing key in run 2: {k:?}"));
            // `CellValue` is `Eq`/`Hash` (numbers carried as bit patterns), so
            // equality here is exact/bit-identical.
            assert_eq!(va, vb, "bit-identical value for {mid:?} at {k:?}");
        }
    }
}

#[test]
fn evaluate_is_deterministic_run_to_run() {
    let model = canonical_model(6, 5); // 30 cells; small and fast
    let first = evaluate(&model, &[REVENUE]).expect("evaluate run 0");
    for run in 1..20 {
        let again = evaluate(&model, &[REVENUE]).expect("evaluate run");
        assert_maps_bit_identical(&first, &again);
        let _ = run;
    }
}

#[test]
fn evaluate_is_insertion_order_independent() {
    // Same schema + same input values, inserted in two different orders.
    let n_time = 5;
    let m_product = 4;

    // Order A: products-then-time (row-major-ish), Price first.
    let mut a = build_schema(n_time, m_product);
    for p in 0..m_product {
        a.set_input(
            PRICE,
            Coordinate::from_pairs([(PRODUCT, product_item(p))]),
            Value::Number((3 + p) as f64),
        );
    }
    for t in 0..n_time {
        for p in 0..m_product {
            a.set_input(
                QUANTITY,
                Coordinate::from_pairs([(TIME, time_item(t)), (PRODUCT, product_item(p))]),
                Value::Number((1 + t + p * 2) as f64),
            );
        }
    }

    // Order B: quantity first, reversed nesting, Price last.
    let mut b = build_schema(n_time, m_product);
    for p in (0..m_product).rev() {
        for t in (0..n_time).rev() {
            b.set_input(
                QUANTITY,
                Coordinate::from_pairs([(TIME, time_item(t)), (PRODUCT, product_item(p))]),
                Value::Number((1 + t + p * 2) as f64),
            );
        }
    }
    for p in (0..m_product).rev() {
        b.set_input(
            PRICE,
            Coordinate::from_pairs([(PRODUCT, product_item(p))]),
            Value::Number((3 + p) as f64),
        );
    }

    let ra = evaluate(&a, &[REVENUE]).expect("evaluate A");
    let rb = evaluate(&b, &[REVENUE]).expect("evaluate B");
    assert_maps_bit_identical(&ra, &rb);
    // And a known spot-check: Revenue[t2,p1] = (3+1) * (1+2+1*2) = 4 * 5 = 20.
    let rev = ra.get(&REVENUE).unwrap();
    assert_eq!(
        rev.get(&key(&[
            (TIME.0, time_item(2).0),
            (PRODUCT.0, product_item(1).0)
        ]))
        .and_then(|v| v.as_num()),
        Some(20.0)
    );
}

proptest! {
    // Random small numeric models. Prices and quantities are integer-valued
    // f64 (no float-formatting noise). Some quantity cells may be absent; a
    // Revenue cell exists iff both Price[p] and Quantity[t,p] exist.
    #[test]
    fn revenue_equals_price_times_quantity(
        m_product in 1u32..5,
        n_time in 1u32..5,
        prices in prop::collection::vec(0i64..50, 1..5),
        // present[t][p] mask + qty value, flattened; regenerated per size below.
        qtys in prop::collection::vec((0i64..50, any::<bool>()), 1..25),
    ) {
        let m_product = m_product.min(prices.len() as u32);
        let mut model = build_schema(n_time, m_product);

        // Prices: one per product (all present).
        let mut price_map: HashMap<u32, f64> = HashMap::new();
        for p in 0..m_product {
            let v = prices[p as usize] as f64;
            price_map.insert(p, v);
            model.set_input(
                PRICE,
                Coordinate::from_pairs([(PRODUCT, product_item(p))]),
                Value::Number(v),
            );
        }

        // Quantities: present per the mask (index into flattened qtys, wrapping).
        let mut qty_map: HashMap<(u32, u32), f64> = HashMap::new();
        let mut idx = 0usize;
        for t in 0..n_time {
            for p in 0..m_product {
                let (val, present) = qtys[idx % qtys.len()];
                idx += 1;
                if present {
                    let v = val as f64;
                    qty_map.insert((t, p), v);
                    model.set_input(
                        QUANTITY,
                        Coordinate::from_pairs([
                            (TIME, time_item(t)),
                            (PRODUCT, product_item(p)),
                        ]),
                        Value::Number(v),
                    );
                }
            }
        }

        let out = evaluate(&model, &[REVENUE]).expect("evaluate");
        let rev = out.get(&REVENUE).cloned().unwrap_or_default();

        // Expected key set: exactly the (t,p) where both inputs exist.
        let mut expected: HashMap<CoordKey, f64> = HashMap::new();
        for (&(t, p), &q) in &qty_map {
            if let Some(&price) = price_map.get(&p) {
                let k = key(&[(TIME.0, time_item(t).0), (PRODUCT.0, product_item(p).0)]);
                expected.insert(k, price * q);
            }
        }

        // Same key set.
        let got_keys: std::collections::BTreeSet<_> = rev.keys().collect();
        let exp_keys: std::collections::BTreeSet<_> = expected.keys().collect();
        prop_assert_eq!(got_keys, exp_keys);

        // Same values (integer-valued products are exact in f64).
        for (k, e) in &expected {
            prop_assert_eq!(rev.get(k).and_then(|v| v.as_num()), Some(*e));
        }
    }
}
