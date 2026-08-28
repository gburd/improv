//! Stress tests for large Improv models exercising incremental recalculation.
//!
//! The heavy cases are `#[ignore]`d so the default `cargo test` stays fast.
//! Run them (with timings) via:
//!
//! ```text
//! cargo test -p improv_engine --test stress -- --ignored --nocapture
//! ```
//!
//! What they exercise:
//!  * `evaluate` a large `Revenue = Price * Quantity` grid once (correctness at
//!    scale: `Price*Quantity` is exact for the integer-valued inputs here);
//!  * `SUM(Revenue OVER Time)` aggregation at scale;
//!  * the live `session::Engine` incremental path: many single-cell edits, each
//!    recomputing only the affected coordinate (deltas, not full rebuild).
//!
//! Sizes are `n_time * n_product` cells. We cap the default heavy sizes at
//! ~1M cells (fits in CI RAM). True billion-cell scale needs out-of-core
//! storage (future work); these tests scale to what fits in memory.

use improv_core_model::{
    BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, FuncId, ItemId, Measure,
    MeasureId, MeasureKind, Model, Name, ValueType,
};
use improv_engine::dataflow::evaluate;
use improv_engine::session::Engine;
use improv_engine::{encode_coord, CoordKey};
use std::time::Instant;

const TIME: CategoryId = CategoryId(1);
const PRODUCT: CategoryId = CategoryId(2);
const REGION: CategoryId = CategoryId(3);
const PRICE: MeasureId = MeasureId(100);
const QUANTITY: MeasureId = MeasureId(101);
const REVENUE: MeasureId = MeasureId(102);
const REVENUE_BY_PRODUCT: MeasureId = MeasureId(103);
const SUM: FuncId = FuncId(1);

// Disjoint item-id ranges so distinct categories never share an item id (the
// CoordKey is (category_id, item_id) pairs, but keeping ranges disjoint also
// makes spot-check math obvious). All must fit in u32.
fn time_item(t: usize) -> ItemId {
    ItemId(1 + t as u32)
}
fn product_item(p: usize) -> ItemId {
    ItemId(2_000_000 + p as u32)
}
fn region_item(r: usize) -> ItemId {
    ItemId(3_000_000_000 + r as u32)
}

/// Sorted coord key, matching the engine's `CoordKey` encoding.
fn key(pairs: &[(u32, u32)]) -> CoordKey {
    let mut k: Vec<(u32, u32)> = pairs.to_vec();
    k.sort();
    k
}

/// Known input values (integer-valued f64, so `Price*Quantity` is exact).
fn price_of(p: usize) -> f64 {
    (1 + p) as f64
}
fn qty_of(t: usize, p: usize) -> f64 {
    (1 + t + p) as f64
}

/// Build a `Time x Product` revenue model: `Revenue = Price * Quantity`, with
/// `Price[Product]` and `Quantity[Time,Product]` fully populated. Same shape as
/// the determinism oracle, scaled to `n_time x n_product`.
fn grid_model(n_time: usize, n_product: usize) -> Model {
    let mut m = Model::new();
    m.add_category(TIME, "Time");
    m.add_category(PRODUCT, "Product");
    for t in 0..n_time {
        m.add_item(time_item(t), TIME, format!("t{t}"));
    }
    for p in 0..n_product {
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
    for p in 0..n_product {
        m.set_input(
            PRICE,
            Coordinate::from_pairs([(PRODUCT, product_item(p))]),
            improv_core_model::Value::Number(price_of(p)),
        );
    }
    for t in 0..n_time {
        for p in 0..n_product {
            m.set_input(
                QUANTITY,
                Coordinate::from_pairs([(TIME, time_item(t)), (PRODUCT, product_item(p))]),
                improv_core_model::Value::Number(qty_of(t, p)),
            );
        }
    }
    m
}

/// Add `RevenueByProduct[Product] = SUM(Revenue OVER Time)` to a grid model.
fn with_sum_over_time(mut m: Model) -> Model {
    let over_time = DimensionSpec {
        by: vec![PRODUCT],
        over: vec![TIME],
        except: vec![],
    };
    m.add_measure(Measure {
        id: REVENUE_BY_PRODUCT,
        name: Name("RevenueByProduct".into()),
        value_type: ValueType::Number,
        categories: vec![PRODUCT],
        kind: MeasureKind::Derived(Formula::new(Expr::Call(
            SUM,
            vec![Expr::Ref(REVENUE, over_time)],
        ))),
        description: None,
    });
    m
}

/// A 3-D variant: `Time x Product x Region` with the same
/// `Revenue = Price * Quantity` shape (Quantity carries all three dims).
fn grid_model_3d(n_time: usize, n_product: usize, n_region: usize) -> Model {
    let mut m = Model::new();
    m.add_category(TIME, "Time");
    m.add_category(PRODUCT, "Product");
    m.add_category(REGION, "Region");
    for t in 0..n_time {
        m.add_item(time_item(t), TIME, format!("t{t}"));
    }
    for p in 0..n_product {
        m.add_item(product_item(p), PRODUCT, format!("p{p}"));
    }
    for r in 0..n_region {
        m.add_item(region_item(r), REGION, format!("r{r}"));
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
        categories: vec![TIME, PRODUCT, REGION],
        kind: MeasureKind::Input,
        description: None,
    });
    m.add_measure(Measure {
        id: REVENUE,
        name: Name("Revenue".into()),
        value_type: ValueType::Number,
        categories: vec![TIME, PRODUCT, REGION],
        kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
            BinaryOp::Mul,
            Box::new(Expr::Ref(PRICE, DimensionSpec::default())),
            Box::new(Expr::Ref(QUANTITY, DimensionSpec::default())),
        ))),
        description: None,
    });
    for p in 0..n_product {
        m.set_input(
            PRICE,
            Coordinate::from_pairs([(PRODUCT, product_item(p))]),
            improv_core_model::Value::Number(price_of(p)),
        );
    }
    for t in 0..n_time {
        for p in 0..n_product {
            for r in 0..n_region {
                m.set_input(
                    QUANTITY,
                    Coordinate::from_pairs([
                        (TIME, time_item(t)),
                        (PRODUCT, product_item(p)),
                        (REGION, region_item(r)),
                    ]),
                    improv_core_model::Value::Number((1 + t + p + r) as f64),
                );
            }
        }
    }
    m
}

// --- Fast smoke test (runs by default) -------------------------------------

/// Small 20x20 grid through the SAME generator + eval path, exact values, so
/// the generator itself is always covered even when heavy tests are skipped.
#[test]
fn smoke_grid_20x20_exact() {
    let (n_time, n_product) = (20usize, 20usize);
    let model = grid_model(n_time, n_product);
    let out = evaluate(&model, &[REVENUE]).expect("evaluate");
    let rev = out.get(&REVENUE).expect("revenue computed");

    assert_eq!(rev.len(), n_time * n_product, "one cell per (t,p)");

    // Every cell exactly Price*Quantity.
    for t in 0..n_time {
        for p in 0..n_product {
            let k = key(&[(TIME.0, time_item(t).0), (PRODUCT.0, product_item(p).0)]);
            assert_eq!(
                rev.get(&k).and_then(|v| v.as_num()),
                Some(price_of(p) * qty_of(t, p)),
                "Revenue[t{t},p{p}]"
            );
        }
    }

    // SUM(Revenue OVER Time) aggregated to one cell per product.
    let out = evaluate(&with_sum_over_time(model), &[REVENUE_BY_PRODUCT]).expect("evaluate sum");
    let rbp = out.get(&REVENUE_BY_PRODUCT).expect("sum computed");
    assert_eq!(rbp.len(), n_product, "one aggregate per product");
    for p in 0..n_product {
        let expected: f64 = (0..n_time).map(|t| price_of(p) * qty_of(t, p)).sum();
        let k = key(&[(PRODUCT.0, product_item(p).0)]);
        assert_eq!(rbp.get(&k).and_then(|v| v.as_num()), Some(expected));
    }

    // Incremental smoke: one edit updates exactly the affected cell.
    let (mut engine, _snap) = Engine::new(&grid_model(n_time, n_product), &[REVENUE]).expect("eng");
    let coord = key(&[(TIME.0, time_item(3).0), (PRODUCT.0, product_item(4).0)]);
    let new_qty = 999.0;
    let snap = engine.set(QUANTITY, coord.clone(), new_qty).expect("set");
    let rev = snap.get(&REVENUE).expect("revenue");
    assert_eq!(
        rev.get(&coord).and_then(|v| v.as_num()),
        Some(price_of(4) * new_qty)
    );
}

// --- Heavy scale tests (ignored by default) --------------------------------

/// Default cap: ~1M cells fits comfortably in CI RAM. Billions of cells will
/// not fit in memory — that needs out-of-core storage (future work). Bump this
/// (and `SCALE_TIME`) locally to push further on a big-RAM host.
const SCALE_PRODUCT: usize = 1000;
const SCALE_TIME: usize = 1000; // 1000 * 1000 = 1,000,000 cells
                                // For >1M, e.g. 10M cells, try: SCALE_TIME = 10_000 (needs several GB RAM).

fn run_scale_case(n_time: usize, n_product: usize) {
    let cells = n_time * n_product;
    let t0 = Instant::now();
    let model = grid_model(n_time, n_product);
    let build = t0.elapsed();

    let t1 = Instant::now();
    let out = evaluate(&model, &[REVENUE]).expect("evaluate");
    let eval = t1.elapsed();

    let rev = out.get(&REVENUE).expect("revenue computed");
    assert_eq!(rev.len(), cells, "cell count == n_time*n_product");

    // Spot-check corners + center (exact, integer-valued products).
    for &(t, p) in &[
        (0, 0),
        (n_time - 1, n_product - 1),
        (n_time / 2, n_product / 2),
    ] {
        let k = key(&[(TIME.0, time_item(t).0), (PRODUCT.0, product_item(p).0)]);
        assert_eq!(
            rev.get(&k).and_then(|v| v.as_num()),
            Some(price_of(p) * qty_of(t, p)),
            "Revenue[t{t},p{p}] exact"
        );
    }
    eprintln!("[scale] {n_time}x{n_product} = {cells} cells: build {build:?}, evaluate {eval:?}");
}

#[test]
#[ignore = "heavy; run with --ignored --nocapture"]
fn scale_evaluate_100k() {
    run_scale_case(100, 1000); // 100k cells
}

#[test]
#[ignore = "heavy; run with --ignored --nocapture"]
fn scale_evaluate_1m() {
    run_scale_case(SCALE_TIME, SCALE_PRODUCT); // 1M cells by default
}

#[test]
#[ignore = "heavy; run with --ignored --nocapture"]
fn scale_sum_aggregation_1m() {
    let (n_time, n_product) = (SCALE_TIME, SCALE_PRODUCT);
    let cells = n_time * n_product;
    let model = with_sum_over_time(grid_model(n_time, n_product));

    let t0 = Instant::now();
    let out = evaluate(&model, &[REVENUE_BY_PRODUCT]).expect("evaluate sum");
    let eval = t0.elapsed();

    let rbp = out.get(&REVENUE_BY_PRODUCT).expect("sum computed");
    assert_eq!(rbp.len(), n_product, "one aggregate per product");

    // Spot-check one column's sum: SUM_t Price[p]*Quantity[t,p].
    let p = n_product / 2;
    let expected: f64 = (0..n_time).map(|t| price_of(p) * qty_of(t, p)).sum();
    let k = key(&[(PRODUCT.0, product_item(p).0)]);
    assert_eq!(rbp.get(&k).and_then(|v| v.as_num()), Some(expected));
    eprintln!(
        "[scale-sum] {n_time}x{n_product} = {cells} cells -> {n_product} aggregates: evaluate {eval:?}"
    );
}

/// 3-D grid at moderate scale (Time x Product x Region). Kept smaller because
/// the third dimension multiplies the cell count fast.
#[test]
#[ignore = "heavy; run with --ignored --nocapture"]
fn scale_evaluate_3d() {
    let (n_time, n_product, n_region) = (100, 100, 20); // 200k cells
    let cells = n_time * n_product * n_region;
    let model = grid_model_3d(n_time, n_product, n_region);

    let t0 = Instant::now();
    let out = evaluate(&model, &[REVENUE]).expect("evaluate 3d");
    let eval = t0.elapsed();

    let rev = out.get(&REVENUE).expect("revenue computed");
    assert_eq!(rev.len(), cells, "one cell per (t,p,r)");

    // Spot-check a corner: Revenue = Price[p] * (1+t+p+r).
    let (t, p, r) = (n_time - 1, n_product - 1, n_region - 1);
    let k = key(&[
        (TIME.0, time_item(t).0),
        (PRODUCT.0, product_item(p).0),
        (REGION.0, region_item(r).0),
    ]);
    assert_eq!(
        rev.get(&k).and_then(|v| v.as_num()),
        Some(price_of(p) * (1 + t + p + r) as f64)
    );
    eprintln!("[scale-3d] {n_time}x{n_product}x{n_region} = {cells} cells: evaluate {eval:?}");
}

// --- Incremental recalculation stress (ignored by default) -----------------

/// Build a large grid with the live `Engine`, then apply MANY single-cell edits
/// via `engine.set(..)`, checking a sample recompute to the exact new value.
/// This is the delta path: each edit recomputes only the affected coordinate,
/// not the whole model. Prints edits/sec.
///
/// Throughput note: `Engine::set` currently returns a *clone of the full
/// snapshot* every call, so per-edit cost is O(total cells) from the clone, not
/// from the (tiny) delta recompute. That clone, not the dataflow, dominates the
/// edits/sec here. A future delta-only return (changed cells only) would lift
/// throughput sharply; the grid/edit counts below are sized so the clone stays
/// affordable while still exercising thousands of real deltas.
#[test]
#[ignore = "heavy; run with --ignored --nocapture"]
fn incremental_many_edits() {
    // 100x100 = 10k cells, 5k edits: exercises thousands of deltas while the
    // per-edit full-snapshot clone (see note above) stays affordable. Bump both
    // to push harder once the engine can return delta-only snapshots.
    let (n_time, n_product) = (100usize, 100usize);
    const N_EDITS: usize = 5_000;

    let t0 = Instant::now();
    let (mut engine, snap) = Engine::new(&grid_model(n_time, n_product), &[REVENUE]).expect("eng");
    let build = t0.elapsed();
    assert_eq!(
        snap.get(&REVENUE).map(|m| m.len()),
        Some(n_time * n_product),
        "initial snapshot has every cell"
    );

    // Deterministic pseudo-random cell walk (no rng dep): a coprime stride over
    // the flattened (t,p) index space visits distinct cells.
    let total = n_time * n_product;
    let stride = 7919usize; // prime, coprime with total for our sizes
    let mut idx = 1usize;

    let t1 = Instant::now();
    let mut checks = 0usize;
    for e in 0..N_EDITS {
        idx = (idx + stride) % total;
        let t = idx / n_product;
        let p = idx % n_product;
        let coord = key(&[(TIME.0, time_item(t).0), (PRODUCT.0, product_item(p).0)]);
        let new_qty = (1_000_000 + e) as f64; // distinct, exact in f64
        let snap = engine.set(QUANTITY, coord.clone(), new_qty).expect("set");

        // Sample: verify ~1% of edits recomputed to the exact new value.
        if e % 100 == 0 {
            let rev = snap.get(&REVENUE).expect("revenue");
            assert_eq!(
                rev.get(&coord).and_then(|v| v.as_num()),
                Some(price_of(p) * new_qty),
                "edit {e}: Revenue[t{t},p{p}] = Price*newQty"
            );
            // The full grid is still present (edits update, never shrink it).
            assert_eq!(rev.len(), total, "cell count stable across edits");
            checks += 1;
        }
    }
    let edit_time = t1.elapsed();
    let per_sec = N_EDITS as f64 / edit_time.as_secs_f64();
    eprintln!(
        "[incremental] {n_time}x{n_product} = {total} cells, engine build {build:?}; \
         {N_EDITS} edits in {edit_time:?} = {per_sec:.0} edits/sec ({checks} sampled checks)"
    );
}

/// Sanity: the engine's edited cell agrees with a from-scratch `evaluate` of a
/// model carrying the same edit — incremental and batch paths must match.
#[test]
#[ignore = "heavy; run with --ignored --nocapture"]
fn incremental_matches_batch_after_edit() {
    let (n_time, n_product) = (50usize, 50usize);
    let (mut engine, _snap) = Engine::new(&grid_model(n_time, n_product), &[REVENUE]).expect("eng");

    let (t, p) = (10usize, 20usize);
    let coord = key(&[(TIME.0, time_item(t).0), (PRODUCT.0, product_item(p).0)]);
    let new_qty = 4242.0;
    let snap = engine.set(QUANTITY, coord.clone(), new_qty).expect("set");
    let incr = snap
        .get(&REVENUE)
        .and_then(|m| m.get(&coord))
        .and_then(|v| v.as_num());

    // Batch: same model with the one input changed.
    let mut model = grid_model(n_time, n_product);
    model.set_input(
        QUANTITY,
        Coordinate::from_pairs([(TIME, time_item(t)), (PRODUCT, product_item(p))]),
        improv_core_model::Value::Number(new_qty),
    );
    let out = evaluate(&model, &[REVENUE]).expect("evaluate");
    let batch = out
        .get(&REVENUE)
        .and_then(|m| {
            m.get(&encode_coord(&Coordinate::from_pairs([
                (TIME, time_item(t)),
                (PRODUCT, product_item(p)),
            ])))
        })
        .and_then(|v| v.as_num());

    assert_eq!(incr, batch, "incremental edit matches batch recompute");
    assert_eq!(incr, Some(price_of(p) * new_qty));
}
