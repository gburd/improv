//! FIXPOINT end to end: the customer's PageRank (P2-1), damping 0.85, with
//! the dangling mass spread uniformly, on a random 1,500-node, 100,000-edge
//! weighted graph, against a plain-Rust power iteration. Then one edge
//! weight changes, and the live engine's warm-started result must equal a
//! fresh engine's on the edited model. Also the small cases: convergence,
//! the max_iter cut-off (the last iterate, never a hang), the defaults and
//! the compile errors.

use improv_core_model::{
    parse_formula, CategoryId, Coordinate, ItemId, Measure, MeasureId, MeasureKind, Model, Name,
    Value, ValueType,
};
use improv_engine::dataflow::evaluate;
use improv_engine::session::Engine;
use improv_engine::{CellValue, CoordKey};
use std::collections::{BTreeMap, HashMap};

const SRC: CategoryId = CategoryId(1);
const DST: CategoryId = CategoryId(2);
const W: MeasureId = MeasureId(100);

/// A tiny deterministic generator (splitmix64), so the graph is the same
/// every run.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn src(i: usize) -> ItemId {
    ItemId(10_000 + i as u32)
}
fn dst(i: usize) -> ItemId {
    ItemId(20_000 + i as u32)
}

/// Src and Dst are two roles of Person (same item names); W[Src, Dst] is
/// the edge weight. `edges` distinct pairs, no self-loops; the first
/// `n / 20` nodes have no out-edges (dangling).
fn graph(n: usize, edges: usize, seed: u64) -> (Model, BTreeMap<(usize, usize), f64>) {
    let mut m = Model::new();
    m.add_category(SRC, "Src");
    m.add_category(DST, "Dst");
    for i in 0..n {
        m.add_item(src(i), SRC, format!("p{i}"));
        m.add_item(dst(i), DST, format!("p{i}"));
    }
    m.add_measure(Measure {
        id: W,
        name: Name("W".into()),
        value_type: ValueType::Number,
        categories: vec![SRC, DST],
        kind: MeasureKind::Input,
        description: None,
    });
    let mut rng = Rng(seed);
    let dangling = n / 20;
    let mut w = BTreeMap::new();
    while w.len() < edges {
        let s = dangling + rng.below((n - dangling) as u64) as usize;
        let d = rng.below(n as u64) as usize;
        if s != d {
            w.insert((s, d), 1.0 + rng.below(9) as f64);
        }
    }
    for (&(s, d), &x) in &w {
        m.set_input(W, at(s, d), Value::Number(x));
    }
    (m, w)
}

fn at(s: usize, d: usize) -> Coordinate {
    Coordinate::from_pairs([(SRC, src(s)), (DST, dst(d))])
}

/// Add derived measure `id` from `text`, with the engine's shape.
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

/// The customer's PageRank, as Improv formulas. M is the column-stochastic
/// transition weight W / out-weight; a node with no out-edges is dangling,
/// and its rank is spread over all N nodes.
fn pagerank(m: &mut Model) -> MeasureId {
    define(m, 200, "Out = SUM(W OVER Dst)");
    define(m, 201, "M = W / Out");
    // 1 where a node has no out-edge (no Out cell), else 0.
    define(m, 202, "Dangling = FILL(0 * Out, 1)");
    define(
        m,
        203,
        "PR = FIXPOINT(SEQUENCE(Src, 1 / @ITEMCOUNT(Src), 0), \
         LAMBDA(r, RELABEL((1 - 0.85) / @ITEMCOUNT(Src) \
         + 0.85 * (FILL(SUMPRODUCT(r, M OVER Src), 0) \
         + SUMPRODUCT(r, Dangling OVER Src) / @ITEMCOUNT(Src)), Dst, Src)), \
         200, 0.0000000000001)",
    );
    MeasureId(203)
}

/// Plain power iteration: r' = (1 − d)/N + d (Mᵀr + dangling/N), from
/// r = 1/N, until Σ|Δ| < 1e-15 (or 1000 rounds).
fn reference(n: usize, w: &BTreeMap<(usize, usize), f64>) -> Vec<f64> {
    let d = 0.85;
    let mut out = vec![0.0; n];
    for (&(s, _), &x) in w {
        out[s] += x;
    }
    let mut r = vec![1.0 / n as f64; n];
    for _ in 0..1000 {
        let dangling: f64 = (0..n).filter(|&i| out[i] == 0.0).map(|i| r[i]).sum();
        let mut next = vec![(1.0 - d) / n as f64 + d * dangling / n as f64; n];
        for (&(s, t), &x) in w {
            next[t] += d * r[s] * x / out[s];
        }
        let delta: f64 = r.iter().zip(&next).map(|(a, b)| (a - b).abs()).sum();
        r = next;
        if delta < 1e-15 {
            break;
        }
    }
    r
}

fn ranks(n: usize, vals: &HashMap<CoordKey, CellValue>) -> Vec<f64> {
    (0..n)
        .map(|i| {
            vals.get(&vec![(SRC.0, src(i).0)])
                .and_then(CellValue::as_num)
                .unwrap_or_else(|| panic!("no rank for p{i}"))
        })
        .collect()
}

fn l1(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum()
}

/// The acceptance case: 1,500 nodes, 100,000 edges (about 15 s in release,
/// three minutes in debug, so it is a heavy test).
#[test]
#[ignore = "heavy; cargo test --release -p improv_engine --test fixpoint -- --ignored"]
fn pagerank_1500_nodes_100k_edges() {
    pagerank_case(1500, 100_000);
}

#[test]
fn pagerank_matches_a_reference_and_follows_an_edge_edit() {
    pagerank_case(200, 4000);
}

fn pagerank_case(n: usize, edges: usize) {
    let (mut m, mut w) = graph(n, edges, 7);
    let pr = pagerank(&mut m);
    let t = std::time::Instant::now();
    let (mut eng, snap) = Engine::new(&m, &[pr]).unwrap();
    let built = t.elapsed();
    let got = ranks(n, &snap[&pr]);
    let want = reference(n, &w);
    let err = l1(&got, &want);
    eprintln!("PageRank {n} x {edges}: L1 {err:.3e}, {built:?}");
    assert!(err < 1e-9, "L1 distance to the reference: {err:e}");
    assert!((got.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    assert_eq!(snap.len(), 4, "only the model's measures are reported");

    // One edge weight changes: the warm-started result equals the reference
    // on the edited graph and a fresh engine's.
    let (&(s, d), &x) = w.iter().nth(edges / 3).unwrap();
    w.insert((s, d), x + 25.0);
    let key = improv_engine::encode_coord(&at(s, d));
    let t = std::time::Instant::now();
    let snap = eng.set(W, key, x + 25.0).unwrap();
    let edited = t.elapsed();
    let warm = ranks(n, &snap[&pr]);
    let err = l1(&warm, &reference(n, &w));
    eprintln!("after one edge edit: L1 {err:.3e}, {edited:?}");
    assert!(err < 1e-9, "L1 after the edit: {err:e}");
    assert!(l1(&warm, &got) > 1e-6, "the edit moved the ranks");
    m.set_input(W, at(s, d), Value::Number(x + 25.0));
    let fresh = evaluate(&m, &[pr]).unwrap();
    assert!(l1(&warm, &ranks(n, &fresh[&pr])) < 1e-11);
}

/// A small model: Person p1..p3, X over Person.
fn small() -> Model {
    let mut m = Model::new();
    m.add_category(SRC, "Person");
    for i in 0..3 {
        m.add_item(src(i), SRC, format!("p{i}"));
    }
    m.add_measure(Measure {
        id: W,
        name: Name("X".into()),
        value_type: ValueType::Number,
        categories: vec![SRC],
        kind: MeasureKind::Input,
        description: None,
    });
    for i in 0..3 {
        m.set_input(
            W,
            Coordinate::from_pairs([(SRC, src(i))]),
            Value::Number(i as f64 + 1.0),
        );
    }
    m
}

#[test]
fn fixpoint_converges_stops_at_max_iter_and_feeds_other_measures() {
    let mut m = small();
    // r = r / 2 + X converges to 2X; Σ|Δ| halves each round.
    define(&mut m, 200, "Two = FIXPOINT(X, LAMBDA(r, r / 2 + X))");
    // Three rounds: X, 1.5X, 1.75X, 1.875X; the last iterate.
    define(&mut m, 201, "Cut = FIXPOINT(X, LAMBDA(r, r / 2 + X), 3)");
    // Never converges (doubles); max_iter 0 is init itself.
    define(&mut m, 202, "Grow = FIXPOINT(X, LAMBDA(r, r * 2), 40, 0)");
    define(&mut m, 203, "Zero = FIXPOINT(X, LAMBDA(r, r * 2), 0)");
    // A measure that reads a FIXPOINT, and a FIXPOINT that reads one.
    define(&mut m, 204, "Half = Two / 2");
    define(
        &mut m,
        205,
        "Again = FIXPOINT(Two, LAMBDA(r, (r + Half) / 2))",
    );
    // A scalar fixed point: √2 by Newton.
    define(
        &mut m,
        206,
        "Root = FIXPOINT(1, LAMBDA(r, (r + 2 / r) / 2))",
    );
    // A scalar start broadcast onto the categories the body adds.
    define(&mut m, 207, "Wide = FIXPOINT(0, LAMBDA(r, r / 2 + X))");
    let ids: Vec<MeasureId> = (200..=207).map(MeasureId).collect();
    let out = evaluate(&m, &ids).unwrap();
    let col = |id: u32| {
        (0..3)
            .map(|i| {
                out[&MeasureId(id)][&vec![(SRC.0, src(i).0)]]
                    .as_num()
                    .unwrap()
            })
            .collect::<Vec<_>>()
    };
    for (got, want) in col(200).iter().zip([2.0, 4.0, 6.0]) {
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }
    assert_eq!(col(201), [1.875, 3.75, 5.625]);
    assert_eq!(
        col(202),
        [2f64.powi(40), 2f64.powi(41), 3.0 * 2f64.powi(40)]
    );
    assert_eq!(col(203), [1.0, 2.0, 3.0]);
    for (got, want) in col(205).iter().zip([1.0, 2.0, 3.0]) {
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }
    for (got, want) in col(207).iter().zip([2.0, 4.0, 6.0]) {
        assert!((got - want).abs() < 1e-9, "{got} vs {want}");
    }
    let root = out[&MeasureId(206)][&Vec::new()].as_num().unwrap();
    assert!((root - 2f64.sqrt()).abs() < 1e-15);

    // Live: an edit re-solves, warm, and agrees with a fresh evaluation.
    let (mut eng, _) = Engine::new(&m, &ids).unwrap();
    let k1 = vec![(SRC.0, src(1).0)];
    let snap = eng.set(W, k1.clone(), 10.0).unwrap();
    m.set_input(
        W,
        Coordinate::from_pairs([(SRC, src(1))]),
        Value::Number(10.0),
    );
    let fresh = evaluate(&m, &ids).unwrap();
    for id in &ids {
        for (k, v) in &fresh[id] {
            let (a, b) = (v.as_num().unwrap(), snap[id][k].as_num().unwrap());
            assert!(
                (a - b).abs() <= 1e-9 * a.abs().max(1.0),
                "{id:?} {k:?}: {a} vs {b}"
            );
        }
        assert_eq!(fresh[id].len(), snap[id].len(), "{id:?}");
    }
    // Clearing a cell drops it from every FIXPOINT (init has no cell there).
    let snap = eng.clear(W, k1.clone()).unwrap();
    assert!(!snap[&MeasureId(200)].contains_key(&k1));
    assert_eq!(snap[&MeasureId(200)].len(), 2);
    // An engine-internal id cannot be edited from outside.
    assert!(eng.set(MeasureId(u32::MAX - 4000), k1, 1.0).is_ok());
}

#[test]
fn fixpoint_compile_rules() {
    let mut m = small();
    let shape = |m: &Model, text: &str| {
        let f = parse_formula(m, text).unwrap_or_else(|e| panic!("{text}: {e}"));
        improv_engine::compiler::derived_shape(m, MeasureId(300), &f.target.0, &f.formula)
            .map_err(|e| e.to_string())
    };
    assert_eq!(
        shape(&m, "Y = FIXPOINT(X, LAMBDA(r, r))"),
        Ok((ValueType::Number, vec![SRC]))
    );
    // A scalar init over a body that ranges over Person: r ranges over Person.
    assert_eq!(
        shape(&m, "Y = FIXPOINT(0, LAMBDA(r, r / 2 + X))"),
        Ok((ValueType::Number, vec![SRC]))
    );
    for (text, why) in [
        ("Y = 1 + FIXPOINT(X, LAMBDA(r, r))", "whole formula"),
        (
            "Y = FIXPOINT(1, LAMBDA(r, SUMPRODUCT(r, X OVER Person)))",
            "SEQUENCE(T, x, 0)",
        ),
        (
            "Y = FIXPOINT(X, LAMBDA(r, SUM(r OVER Person)))",
            "keep the iterate's categories",
        ),
        (
            "Y = FIXPOINT(X, LAMBDA(r, r > 1))",
            "initial value is Number",
        ),
        ("Y = FIXPOINT(X, LAMBDA(r, r), X)", "constant number"),
        ("Y = FIXPOINT(X, LAMBDA(r, r), 1.5)", "whole number"),
        ("Y = FIXPOINT(X, LAMBDA(r, r), 20000)", "whole number"),
        ("Y = FIXPOINT(X, LAMBDA(r, r), 5, 0 - 1)", "0 or more"),
        ("Y = FIXPOINT(X, LAMBDA(r, r), 5, \"x\")", "constant number"),
    ] {
        let e = shape(&m, text).unwrap_err();
        assert!(e.contains(why), "{text}: {e}");
    }
    // It may not read its own measure.
    define(&mut m, 300, "Y = FIXPOINT(X, LAMBDA(r, r))");
    let f = parse_formula(&m, "Y = FIXPOINT(X, LAMBDA(r, r + Y))").unwrap();
    let e = improv_engine::compiler::derived_shape(&m, MeasureId(300), "Y", &f.formula)
        .unwrap_err()
        .to_string();
    assert!(e.contains("may not read its own measure"), "{e}");
}
