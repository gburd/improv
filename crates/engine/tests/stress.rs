//! Large-model stress tests for the Improv engine.
//!
//! These are all `#[ignore]` so the default `cargo test` stays fast. Run them
//! (with timing printed to stdout) via:
//!
//! ```text
//! cargo test -p improv_engine --test stress -- --ignored --nocapture
//! ```
//!
//! Sizes are constants near the top of the file so they are easy to scale.
//! The default ignored run builds a Time x Product model of `N_TIME * M_PRODUCT`
//! cells (~100k) and evaluates `Revenue = Price * Quantity` plus a rollup
//! `RevenueByProduct = SUM(Revenue OVER Time)`. Timing is printed but never
//! asserted (nondeterministic wall-clock).

use improv_core_model::{
    BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, FuncId, ItemId, Measure,
    MeasureId, MeasureKind, Model, Name, Value, ValueType,
};
use improv_engine::dataflow::evaluate;
use improv_engine::CoordKey;
use std::time::Instant;

// --- scale knobs -----------------------------------------------------------
// N_TIME * M_PRODUCT = number of Quantity/Revenue cells. ~100k by default.
// If a machine is too slow, dial these down to ~40k (e.g. 400 x 100).
const N_TIME: u32 = 500;
const M_PRODUCT: u32 = 200;

// Fixed category / measure ids.
const TIME: CategoryId = CategoryId(1);
const PRODUCT: CategoryId = CategoryId(2);
const PRICE: MeasureId = MeasureId(100);
const QUANTITY: MeasureId = MeasureId(101);
const REVENUE: MeasureId = MeasureId(102);
const REVENUE_BY_PRODUCT: MeasureId = MeasureId(103);

const SUM: FuncId = FuncId(1);

// Item id ranges (kept disjoint so a coordinate never collides).
const TIME_ITEM_BASE: u32 = 1_000_000;
const PRODUCT_ITEM_BASE: u32 = 2_000_000;

fn time_item(t: u32) -> ItemId {
    ItemId(TIME_ITEM_BASE + t)
}
fn product_item(p: u32) -> ItemId {
    ItemId(PRODUCT_ITEM_BASE + p)
}

/// Deterministic pseudo-values so spot checks have known answers without a big
/// oracle table. Integer-valued to avoid float noise.
fn price_of(p: u32) -> f64 {
    (1 + (p % 97)) as f64
}
fn qty_of(t: u32, p: u32) -> f64 {
    (1 + ((t * 31 + p * 7) % 101)) as f64
}

fn key(pairs: &[(u32, u32)]) -> CoordKey {
    let mut k: Vec<(u32, u32)> = pairs.to_vec();
    k.sort();
    k
}

/// Build a Time x Product model with Price[Product], Quantity[Time,Product],
/// and Revenue = Price * Quantity. `with_rollup` adds RevenueByProduct.
fn build_model(n_time: u32, m_product: u32, with_rollup: bool) -> Model {
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

    if with_rollup {
        m.add_measure(Measure {
            id: REVENUE_BY_PRODUCT,
            name: Name("RevenueByProduct".into()),
            value_type: ValueType::Number,
            categories: vec![PRODUCT],
            kind: MeasureKind::Derived(Formula::new(Expr::Call(
                SUM,
                vec![Expr::Ref(
                    REVENUE,
                    DimensionSpec {
                        by: vec![PRODUCT],
                        over: vec![TIME],
                        except: vec![],
                    },
                )],
            ))),
            description: None,
        });
    }

    for p in 0..m_product {
        m.set_input(
            PRICE,
            Coordinate::from_pairs([(PRODUCT, product_item(p))]),
            Value::Number(price_of(p)),
        );
    }
    for t in 0..n_time {
        for p in 0..m_product {
            m.set_input(
                QUANTITY,
                Coordinate::from_pairs([(TIME, time_item(t)), (PRODUCT, product_item(p))]),
                Value::Number(qty_of(t, p)),
            );
        }
    }
    m
}

#[test]
#[ignore = "stress: run with --ignored --nocapture"]
fn stress_revenue_large() {
    let cells = (N_TIME * M_PRODUCT) as usize;
    println!("[stress_revenue_large] building {N_TIME}x{M_PRODUCT} = {cells} cells");
    let t0 = Instant::now();
    let model = build_model(N_TIME, M_PRODUCT, false);
    println!("[stress_revenue_large] build took {:?}", t0.elapsed());

    let t1 = Instant::now();
    let out = evaluate(&model, &[REVENUE]).expect("evaluate revenue");
    let eval = t1.elapsed();
    let rev = out.get(&REVENUE).expect("revenue computed");

    println!(
        "[stress_revenue_large] evaluate took {:?} ({:.0} cells/sec)",
        eval,
        cells as f64 / eval.as_secs_f64()
    );

    assert_eq!(rev.len(), cells, "every Time x Product cell is produced");

    // Spot-check a few known cells.
    for &(t, p) in &[(0, 0), (1, 3), (N_TIME - 1, M_PRODUCT - 1), (250, 100)] {
        let expected = price_of(p) * qty_of(t, p);
        let got = rev.get(&key(&[
            (TIME.0, time_item(t).0),
            (PRODUCT.0, product_item(p).0),
        ]));
        assert_eq!(got, Some(&expected), "Revenue[t{t},p{p}]");
    }
}

#[test]
#[ignore = "stress: run with --ignored --nocapture"]
fn stress_multi_layer_rollup() {
    let cells = (N_TIME * M_PRODUCT) as usize;
    println!("[stress_multi_layer_rollup] building {N_TIME}x{M_PRODUCT} = {cells} cells + rollup");
    let model = build_model(N_TIME, M_PRODUCT, true);

    let t0 = Instant::now();
    let out = evaluate(&model, &[REVENUE_BY_PRODUCT]).expect("evaluate rollup");
    let eval = t0.elapsed();
    let rbp = out.get(&REVENUE_BY_PRODUCT).expect("rollup computed");

    println!(
        "[stress_multi_layer_rollup] evaluate took {:?} ({:.0} src-cells/sec)",
        eval,
        cells as f64 / eval.as_secs_f64()
    );

    assert_eq!(rbp.len(), M_PRODUCT as usize, "one result per product");

    // Spot-check a couple of known sums: SUM over all Time of price*qty.
    for &p in &[0u32, 7, M_PRODUCT - 1] {
        let expected: f64 = (0..N_TIME).map(|t| price_of(p) * qty_of(t, p)).sum();
        let got = rbp.get(&key(&[(PRODUCT.0, product_item(p).0)]));
        assert_eq!(got, Some(&expected), "RevenueByProduct[p{p}]");
    }
}
