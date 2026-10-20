//! Scale probes from the v0.10.0 benchmark (`docs/reviews/2026-10-20-v0.10.0-benchmark.md`
//! §3c and §3d), rebuilt so a fix can be measured on the same shapes.
//! Every case is `#[ignore]`d; sizes come from the environment:
//!
//! ```text
//! PROBE_N=1000000 PROBE_FORMULA='SUM(X OVER W)' \
//!   cargo test --release -p improv_engine --test probe wide -- --ignored --nocapture
//! PROBE_N=20000 PROBE_SHAPE=fan \
//!   cargo test --release -p improv_engine --test probe measures -- --ignored --nocapture
//! ```

use improv_core_model::{
    parse_formula, Category, CategoryId, Coordinate, Item, ItemId, Measure, MeasureId, MeasureKind,
    Model, Name, Value, ValueType,
};
use improv_engine::dataflow::evaluate;
use improv_engine::session::Engine;
use std::time::{Duration, Instant};

fn env_or(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A category built through the struct fields (`add_item` is O(n²)).
fn category(m: &mut Model, id: u32, name: &str, base: u32, n: usize) -> Vec<ItemId> {
    let items: Vec<ItemId> = (0..n as u32).map(|i| ItemId(base + i)).collect();
    for (i, it) in items.iter().enumerate() {
        m.items.insert(
            *it,
            Item {
                id: *it,
                category: CategoryId(id),
                name: Name(format!("{name}{i}")),
            },
        );
    }
    m.categories.insert(
        CategoryId(id),
        Category {
            id: CategoryId(id),
            name: Name(name.into()),
            items: items.clone(),
        },
    );
    items
}

fn input(m: &mut Model, id: u32, name: &str, cats: Vec<CategoryId>) {
    m.add_measure(Measure {
        id: MeasureId(id),
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: cats,
        kind: MeasureKind::Input,
        description: None,
    });
}

fn define(m: &mut Model, id: u32, text: &str) {
    let f = parse_formula(m, text).unwrap_or_else(|e| panic!("{text}: {e}"));
    let (vt, cats) =
        improv_engine::compiler::derived_shape(m, MeasureId(id), &f.target.0, &f.formula)
            .unwrap_or_else(|e| panic!("{text}: {e}"));
    m.add_measure(Measure {
        id: MeasureId(id),
        name: f.target,
        value_type: vt,
        categories: cats,
        kind: MeasureKind::Derived(f.formula),
        description: None,
    });
}

/// Deterministic values in [0, 1).
fn lcg(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*seed >> 11) as f64 / (1u64 << 53) as f64
}

fn stats(mut v: Vec<Duration>) -> String {
    v.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1e3;
    format!(
        "{:.2} / {:.2} / {:.2} ms",
        ms(v[0]),
        ms(v[v.len() / 2]),
        ms(v[v.len() - 1])
    )
}

const W: u32 = 1;
const K: u32 = 2;
const X: MeasureId = MeasureId(100);
const Y: MeasureId = MeasureId(101);

/// §3d: one category W of `PROBE_N` items, random `X[W]`, `Y = PROBE_FORMULA`.
#[test]
#[ignore = "probe; run with --ignored --nocapture"]
fn wide() {
    let n = env_or("PROBE_N", 100_000);
    let edits = env_or("PROBE_EDITS", 10);
    let formula = std::env::var("PROBE_FORMULA").unwrap_or_else(|_| "SUM(X OVER W)".to_string());
    let t0 = Instant::now();
    let mut m = Model::new();
    let w = category(&mut m, W, "W", 1_000_000, n);
    if formula.contains("INTO K") {
        category(&mut m, K, "K", 100_000_000, n);
    }
    input(&mut m, X.0, "X", vec![CategoryId(W)]);
    let mut seed = 42;
    for it in &w {
        m.set_input(
            X,
            Coordinate::from_pairs([(CategoryId(W), *it)]),
            Value::Number(lcg(&mut seed)),
        );
    }
    define(&mut m, Y.0, &format!("Y = {formula}"));
    let build = t0.elapsed();
    let t1 = Instant::now();
    let fresh = evaluate(&m, &[Y]).expect("evaluate");
    let eval = t1.elapsed();
    let t2 = Instant::now();
    let (mut engine, snap) = Engine::new(&m, &[Y]).expect("engine");
    let new = t2.elapsed();
    assert_eq!(
        snap.get(&Y).map(|c| c.len()),
        fresh.get(&Y).map(|c| c.len()),
        "engine == evaluate (cells)"
    );
    drop(fresh);
    drop(snap);
    // New max: a middle item above every other value.
    let mut maxes = Vec::new();
    for e in 0..edits {
        let it = w[n / 2 + e % (n / 2)];
        let t = Instant::now();
        let s = engine
            .set(X, vec![(W, it.0)], 10.0 + e as f64)
            .expect("set");
        maxes.push(t.elapsed());
        drop(s);
    }
    // Tail +0.5.
    let mut tails = Vec::new();
    let last = *w.last().unwrap();
    for e in 0..edits {
        let t = Instant::now();
        let s = engine
            .set(X, vec![(W, last.0)], 0.5 * (e + 1) as f64)
            .expect("set");
        tails.push(t.elapsed());
        drop(s);
    }
    eprintln!(
        "[wide] {formula} n={n}: build {build:?}, evaluate {eval:?}, Engine::new {new:?}; \
         edit new max {}; edit tail {}",
        stats(maxes),
        stats(tails)
    );
}

/// §3c: one category of `PROBE_ITEMS` items (100), `PROBE_N` measures in a
/// `fan` (`Mk = X + k`) or `chain` (`Mk = Mk-1 + 1`); edit one root cell.
#[test]
#[ignore = "probe; run with --ignored --nocapture"]
fn measures() {
    let n = env_or("PROBE_N", 2_000);
    let items = env_or("PROBE_ITEMS", 100);
    let edits = env_or("PROBE_EDITS", 20);
    let chain = std::env::var("PROBE_SHAPE").is_ok_and(|s| s == "chain");
    let mut m = Model::new();
    let c = category(&mut m, W, "C", 1_000_000, items);
    input(&mut m, X.0, "X", vec![CategoryId(W)]);
    for (i, it) in c.iter().enumerate() {
        m.set_input(
            X,
            Coordinate::from_pairs([(CategoryId(W), *it)]),
            Value::Number(i as f64),
        );
    }
    let t0 = Instant::now();
    let mut targets = Vec::new();
    for k in 1..=n {
        let text = if chain {
            let prev = if k == 1 {
                "X".to_string()
            } else {
                format!("M{}", k - 1)
            };
            format!("M{k} = {prev} + 1")
        } else {
            format!("M{k} = X + {k}")
        };
        define(&mut m, 1000 + k as u32, &text);
        targets.push(MeasureId(1000 + k as u32));
    }
    let shape = t0.elapsed();
    let t1 = Instant::now();
    let fresh = evaluate(&m, &targets).expect("evaluate");
    let eval = t1.elapsed();
    drop(fresh);
    let t2 = Instant::now();
    let (mut engine, snap) = Engine::new(&m, &targets).expect("engine");
    let new = t2.elapsed();
    drop(snap);
    let mut lat = Vec::new();
    for e in 0..edits {
        let t = Instant::now();
        let s = engine
            .set(X, vec![(W, c[0].0)], 1000.0 + e as f64)
            .expect("set");
        lat.push(t.elapsed());
        let last = s.get(targets.last().unwrap()).unwrap();
        let want = 1000.0 + e as f64 + n as f64;
        assert_eq!(
            last.get(&vec![(W, c[0].0)]).and_then(|v| v.as_num()),
            Some(want)
        );
    }
    eprintln!(
        "[measures] {} n={n} items={items}: parse+shape {shape:?}, evaluate {eval:?}, \
         Engine::new {new:?}; root edit {}",
        if chain { "chain" } else { "fan" },
        stats(lat)
    );
}
