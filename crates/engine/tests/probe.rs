//! Scale probes from the v0.10.0 benchmark (`docs/reviews/2026-10-20-v0.10.0-benchmark.md`
//! §3c and §3d), rebuilt so a fix can be measured on the same shapes.
//! Every case is `#[ignore]`d; sizes come from the environment:
//!
//! ```text
//! PROBE_N=1000000 PROBE_FORMULA='SUM(X OVER W)' \
//!   cargo test --release -p improv_engine --test probe wide -- --ignored --nocapture
//! PROBE_N=20000 PROBE_SHAPE=fan \
//!   cargo test --release -p improv_engine --test probe measures -- --ignored --nocapture
//! PROBE_TIME=5000 PROBE_PRODUCT=1000 \
//!   cargo test --release -p improv_engine --test probe grid -- --ignored --nocapture
//! ```

use improv_core_model::{
    parse_formula, Category, CategoryId, Coordinate, Item, ItemId, Measure, MeasureId, MeasureKind,
    Model, Name, Value, ValueType,
};
use improv_engine::dataflow::evaluate;
use improv_engine::session::Engine;
use improv_engine::CoordKey;
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
            .set(X, CoordKey::from_slice(&[(W, it.0)]), 10.0 + e as f64)
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
            .set(
                X,
                CoordKey::from_slice(&[(W, last.0)]),
                0.5 * (e + 1) as f64,
            )
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
/// `split` is a fan whose measures but the first read another input `Z`,
/// so the edit reaches one measure.
#[test]
#[ignore = "probe; run with --ignored --nocapture"]
fn measures() {
    let n = env_or("PROBE_N", 2_000);
    let items = env_or("PROBE_ITEMS", 100);
    let edits = env_or("PROBE_EDITS", 20);
    let shape_name = std::env::var("PROBE_SHAPE").unwrap_or_else(|_| "fan".into());
    let chain = shape_name == "chain";
    let split = shape_name == "split";
    let mut m = Model::new();
    let c = category(&mut m, W, "C", 1_000_000, items);
    input(&mut m, X.0, "X", vec![CategoryId(W)]);
    input(&mut m, Y.0, "Z", vec![CategoryId(W)]);
    for (i, it) in c.iter().enumerate() {
        for id in [X, Y] {
            m.set_input(
                id,
                Coordinate::from_pairs([(CategoryId(W), *it)]),
                Value::Number(i as f64),
            );
        }
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
        } else if split && k > 1 {
            format!("M{k} = Z + {k}")
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
            .set(X, CoordKey::from_slice(&[(W, c[0].0)]), 1000.0 + e as f64)
            .expect("set");
        lat.push(t.elapsed());
        let (last, want) = if split {
            (s.get(&targets[0]).unwrap(), 1001.0 + e as f64)
        } else {
            (
                s.get(targets.last().unwrap()).unwrap(),
                1000.0 + e as f64 + n as f64,
            )
        };
        assert_eq!(
            last.get([(W, c[0].0)].as_slice()).and_then(|v| v.as_num()),
            Some(want)
        );
    }
    eprintln!(
        "[measures] {} n={n} items={items}: parse+shape {shape:?}, evaluate {eval:?}, \
         Engine::new {new:?}; root edit {}",
        shape_name,
        stats(lat)
    );
}

/// The stress grid (`tests/stress.rs`): `Revenue = Price * Quantity` over
/// `PROBE_TIME` x `PROBE_PRODUCT` cells (1000 x 1000), with the live heap
/// after each phase: the model, `evaluate` (its result dropped), and
/// `Engine::new` (snapshot held), to attribute bytes per cell.
#[test]
#[ignore = "probe; run with --ignored --nocapture"]
fn grid() {
    let (nt, np) = (env_or("PROBE_TIME", 1000), env_or("PROBE_PRODUCT", 1000));
    let (time, product) = (CategoryId(1), CategoryId(2));
    let (price, qty) = (MeasureId(100), MeasureId(101));
    let rev = MeasureId(102);
    let cells = nt * np;
    let h0 = heap_mb();
    let mut m = Model::new();
    let ts = category(&mut m, 1, "Time", 1, nt);
    let ps = category(&mut m, 2, "Product", 2_000_000, np);
    input(&mut m, price.0, "Price", vec![product]);
    input(&mut m, qty.0, "Quantity", vec![time, product]);
    for (p, it) in ps.iter().enumerate() {
        let at = Coordinate::from_pairs([(product, *it)]);
        m.set_input(price, at, Value::Number((1 + p) as f64));
    }
    for (t, ti) in ts.iter().enumerate() {
        for (p, pi) in ps.iter().enumerate() {
            let at = Coordinate::from_pairs([(time, *ti), (product, *pi)]);
            m.set_input(qty, at, Value::Number((1 + t + p) as f64));
        }
    }
    define(&mut m, rev.0, "Revenue = Price * Quantity");
    let h1 = heap_mb();
    let t1 = Instant::now();
    let fresh = evaluate(&m, &[rev]).expect("evaluate");
    let eval = t1.elapsed();
    assert_eq!(fresh[&rev].len(), cells);
    let h2 = heap_mb();
    drop(fresh);
    let h3 = heap_mb();
    let t2 = Instant::now();
    let (mut eng, snap) = Engine::new(&m, &[rev]).expect("engine");
    let new = t2.elapsed();
    assert_eq!(snap[&rev].len(), cells);
    let h4 = heap_mb();
    drop(snap);
    let h5 = heap_mb();
    let mut lat = Vec::new();
    for e in 0..env_or("PROBE_EDITS", 10) {
        let at: CoordKey = [(1, ts[e % nt].0), (2, ps[e % np].0)].as_slice().into();
        let t = Instant::now();
        drop(eng.set(qty, at, 7.0 + e as f64).expect("set"));
        lat.push(t.elapsed());
    }
    let per = |a: f64, b: f64| (b - a) * 1024.0 * 1024.0 / cells as f64;
    eprintln!(
        "[grid] {nt}x{np} = {cells} cells: evaluate {eval:?}, Engine::new {new:?}, edit {}; \
         live heap {h0:.0} -> model {h1:.0} -> evaluate result {h2:.0} -> dropped {h3:.0} \
         -> engine+snapshot {h4:.0} -> engine {h5:.0} MB; B/cell: model {:.0}, \
         evaluate result {:.0}, engine {:.0}, snapshot {:.0}; RSS {:.0} MB",
        stats(lat),
        per(h0, h1),
        per(h3, h2),
        per(h3, h5),
        per(h5, h4),
        rss_mb()
    );
}

/// Live heap in MB (glibc's `mallinfo2`: allocated bytes, not what the
/// allocator keeps), so each phase's own allocations are measured.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn heap_mb() -> f64 {
    #[repr(C)]
    struct MallInfo2 {
        arena: usize,
        ordblks: usize,
        smblks: usize,
        hblks: usize,
        hblkhd: usize,
        usmblks: usize,
        fsmblks: usize,
        uordblks: usize,
        fordblks: usize,
        keepcost: usize,
    }
    extern "C" {
        fn mallinfo2() -> MallInfo2;
    }
    // SAFETY: mallinfo2 takes no arguments and returns a plain struct.
    let m = unsafe { mallinfo2() };
    (m.uordblks + m.hblkhd) as f64 / (1024.0 * 1024.0)
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn heap_mb() -> f64 {
    0.0
}

/// Current RSS in MB (glibc may keep freed memory, so a drop is a floor).
fn rss_mb() -> f64 {
    let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    s.lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.trim().trim_end_matches(" kB").trim().parse::<f64>().ok())
        .map_or(0.0, |kb| kb / 1024.0)
}

/// §2: the PageRank FIXPOINT of `tests/fixpoint.rs` on `PROBE_N` nodes and
/// `PROBE_EDGES` edges (item ids far apart, so any n works), with RSS
/// after each phase, to attribute memory per edge.
#[test]
#[ignore = "probe; run with --ignored --nocapture"]
fn pagerank() {
    let n = env_or("PROBE_N", 10_000);
    let edges = env_or("PROBE_EDGES", 100 * n);
    let (src, dst) = (CategoryId(1), CategoryId(2));
    let w = MeasureId(100);
    let (r0, h0) = (rss_mb(), heap_mb());
    let t0 = Instant::now();
    let mut m = Model::new();
    // Two roles of one Person: RELABEL maps the items by name.
    m.add_category(src, "Src");
    m.add_category(dst, "Dst");
    for i in 0..n as u32 {
        m.add_item(ItemId(1_000_000 + i), src, format!("p{i}"));
        m.add_item(ItemId(100_000_000 + i), dst, format!("p{i}"));
    }
    input(&mut m, w.0, "W", vec![src, dst]);
    let mut seed = 7u64;
    let dangling = n / 20;
    let mut seen = std::collections::HashSet::new();
    while seen.len() < edges {
        let s = dangling + (lcg(&mut seed) * (n - dangling) as f64) as usize;
        let d = (lcg(&mut seed) * n as f64) as usize;
        if s != d && seen.insert((s, d)) {
            let x = 1.0 + (lcg(&mut seed) * 9.0).floor();
            let at = Coordinate::from_pairs([
                (src, ItemId(1_000_000 + s as u32)),
                (dst, ItemId(100_000_000 + d as u32)),
            ]);
            m.set_input(w, at, Value::Number(x));
        }
    }
    drop(seen);
    define(&mut m, 200, "Out = SUM(W OVER Dst)");
    define(&mut m, 201, "M = W / Out");
    define(&mut m, 202, "Dangling = FILL(0 * Out, 1)");
    define(
        &mut m,
        203,
        "PR = FIXPOINT(SEQUENCE(Src, 1 / @ITEMCOUNT(Src), 0), \
         LAMBDA(r, RELABEL((1 - 0.85) / @ITEMCOUNT(Src) \
         + 0.85 * (FILL(SUMPRODUCT(r, M OVER Src), 0) \
         + SUMPRODUCT(r, Dangling OVER Src) / @ITEMCOUNT(Src)), Dst, Src)), \
         200, 0.0000000000001)",
    );
    let build = t0.elapsed();
    let (r1, h1) = (rss_mb(), heap_mb());
    let t1 = Instant::now();
    // PROBE_TARGET=200 (Out), 201 (M), 202 (Dangling): a part only.
    let target = MeasureId(env_or("PROBE_TARGET", 203) as u32);
    let (mut eng, snap) = Engine::new(&m, &[target]).expect("engine");
    let new = t1.elapsed();
    let (r2, h2) = (rss_mb(), heap_mb());
    if target.0 == 203 {
        assert_eq!(snap[&target].len(), n);
    }
    drop(snap);
    let at: CoordKey = [(1, 1_000_000 + dangling as u32), (2, 100_000_000)]
        .as_slice()
        .into();
    let t2 = Instant::now();
    let s = eng.set(w, at, 25.0).expect("set");
    let edit = t2.elapsed();
    drop(s);
    let (r3, h3) = (rss_mb(), heap_mb());
    let per = |a: f64, b: f64| (b - a) * 1024.0 * 1024.0 / edges as f64;
    eprintln!(
        "[pagerank] n={n} edges={edges}: model {build:?} ({:.0} B/edge), \
         Engine::new {new:?} ({:.0} B/edge), edit {edit:?} ({:.0} B/edge more); \
         RSS {r0:.0} -> {r1:.0} -> {r2:.0} -> {r3:.0} MB; \
         live heap {h0:.0} -> {h1:.0} -> {h2:.0} -> {h3:.0} MB \
         (model {:.0}, engine {:.0}, edit {:.0} B/edge)",
        per(r0, r1),
        per(r1, r2),
        per(r2, r3),
        per(h0, h1),
        per(h1, h2),
        per(h2, h3)
    );
}
