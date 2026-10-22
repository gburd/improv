//! D5 goldens: `SUM` and `AVERAGE` of 10,000 fractional values are the
//! exact sum's nearest double (and that double / 10,000), in every
//! evaluation path: the two-level reduce of one large category (`evaluate`
//! and the live engine, before and after an edit) and the flat reduce over
//! two categories. The expected bits are from mpmath
//! (`scripts/goldens/sum_10k.py`); a left-to-right fold misses each by
//! 14 to 18 ulps.

use improv_core_model::{
    parse_formula, Category, CategoryId, Coordinate, Item, ItemId, Measure, MeasureId, MeasureKind,
    Model, Name, Value, ValueType,
};
use improv_engine::dataflow::evaluate;
use improv_engine::session::Engine;
use improv_engine::{CellValue, CoordKey};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug)]
enum Shape {
    Uniform,
    Signed,
    Mixed,
}

/// (shape, LCG seed, SUM bits, AVERAGE bits), from `scripts/goldens/sum_10k.py`.
const GOLDENS: [(Shape, u64, u64, u64); 3] = [
    // SUM 5970.168915251347, AVERAGE 0.5970168915251347; a left fold: 5970.168915251333
    (Shape::Uniform, 42, 0x40b7522b3e07a855, 0x3fe31ac32b085231),
    // SUM 88.8692045870742, AVERAGE 0.008886920458707419; a left fold: 88.86920458707431
    (Shape::Signed, 7, 0x405637a10c46c114, 0x3f82334e45dbc0fc),
    // SUM -3370982.755206487, AVERAGE -337.0982755206487; a left fold: -3370982.755206469
    (Shape::Mixed, 1234, 0xc149b7f360aa9b2e, 0xc0751192895a32f0),
];

const N: usize = 10_000;
const W: u32 = 1;
const A: u32 = 2;
const B: u32 = 3;
const X: MeasureId = MeasureId(10);
const XY: MeasureId = MeasureId(11);
const SUM: MeasureId = MeasureId(20);
const AVG: MeasureId = MeasureId(21);
const SUM2: MeasureId = MeasureId(22);
const AVG2: MeasureId = MeasureId(23);

/// The probe's LCG (tests/probe.rs), as the script reproduces it.
fn lcg(seed: &mut u64) -> f64 {
    *seed = seed
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    (*seed >> 11) as f64 / (1u64 << 53) as f64
}

fn values(shape: Shape, mut seed: u64) -> Vec<f64> {
    (0..N)
        .map(|_| {
            let u = lcg(&mut seed);
            match shape {
                Shape::Uniform => u + 0.1,
                Shape::Signed => u * 2.0 - 1.0,
                Shape::Mixed => {
                    let v = lcg(&mut seed);
                    let scale = [1.0, 10.0, 100.0, 1000.0, 1e4, 1e5, 1e6][(v * 7.0) as usize];
                    (u - 0.5) * scale
                }
            }
        })
        .collect()
}

fn category(m: &mut Model, id: u32, base: u32, n: usize) -> Vec<ItemId> {
    let items: Vec<ItemId> = (0..n as u32).map(|i| ItemId(base + i)).collect();
    for (i, it) in items.iter().enumerate() {
        let name = Name(format!("c{id}i{i}"));
        let (id, category) = (*it, CategoryId(id));
        m.items.insert(*it, Item { id, category, name });
    }
    let (cid, name) = (CategoryId(id), Name(format!("C{id}")));
    let items_c = items.clone();
    m.categories.insert(
        cid,
        Category {
            id: cid,
            name,
            items: items_c,
        },
    );
    items
}

fn measure(m: &mut Model, id: MeasureId, text: Option<&str>, cats: Vec<CategoryId>) {
    let (name, kind, value_type, categories) = match text {
        None => (
            Name(format!("X{}", id.0)),
            MeasureKind::Input,
            ValueType::Number,
            cats,
        ),
        Some(t) => {
            let f = parse_formula(m, t).unwrap_or_else(|e| panic!("{t}: {e}"));
            let (vt, cats) =
                improv_engine::compiler::derived_shape(m, id, &f.target.0, &f.formula).unwrap();
            (f.target, MeasureKind::Derived(f.formula), vt, cats)
        }
    };
    m.add_measure(Measure {
        id,
        name,
        value_type,
        categories,
        kind,
        description: None,
    });
}

/// X over one 10,000-item category (two-level) and X10 over 100 × 100
/// (flat), both holding `xs`.
fn model(xs: &[f64]) -> (Model, Vec<ItemId>) {
    let mut m = Model::new();
    let w = category(&mut m, W, 1_000_000, N);
    let a = category(&mut m, A, 2_000_000, 100);
    let b = category(&mut m, B, 3_000_000, 100);
    measure(&mut m, X, None, vec![CategoryId(W)]);
    measure(&mut m, XY, None, vec![CategoryId(A), CategoryId(B)]);
    for (i, x) in xs.iter().enumerate() {
        let one = Coordinate::from_pairs([(CategoryId(W), w[i])]);
        m.set_input(X, one, Value::Number(*x));
        let two =
            Coordinate::from_pairs([(CategoryId(A), a[i / 100]), (CategoryId(B), b[i % 100])]);
        m.set_input(XY, two, Value::Number(*x));
    }
    measure(&mut m, SUM, Some("S = SUM(X10 OVER C1)"), vec![]);
    measure(&mut m, AVG, Some("M = AVERAGE(X10 OVER C1)"), vec![]);
    measure(&mut m, SUM2, Some("S2 = SUM(X11 OVER C2, C3)"), vec![]);
    measure(&mut m, AVG2, Some("M2 = AVERAGE(X11 OVER C2, C3)"), vec![]);
    (m, w)
}

const ALL: [MeasureId; 4] = [SUM, AVG, SUM2, AVG2];

fn bits(cells: &HashMap<MeasureId, HashMap<CoordKey, CellValue>>, id: MeasureId) -> u64 {
    match cells[&id].get(&CoordKey::new()) {
        Some(CellValue::Num(b)) => *b,
        other => panic!("{id:?}: {other:?}"),
    }
}

#[test]
fn ten_thousand_fractional_values_sum_to_the_exact_sums_nearest_double() {
    for (shape, seed, sum, avg) in GOLDENS {
        let xs = values(shape, seed);
        let (mut m, w) = model(&xs);
        let fresh = evaluate(&m, &ALL).unwrap();
        for (id, want) in [(SUM, sum), (AVG, avg), (SUM2, sum), (AVG2, avg)] {
            assert_eq!(
                bits(&fresh, id),
                want,
                "{shape:?} {id:?}: {} != {}",
                f64::from_bits(bits(&fresh, id)),
                f64::from_bits(want)
            );
        }
        // The live engine starts at the same bits, and an edit to one block
        // gives what a fresh evaluation of the edited model gives.
        let (mut engine, snap) = Engine::new(&m, &[SUM, AVG]).unwrap();
        assert_eq!(
            snap[&SUM][&CoordKey::new()],
            CellValue::Num(sum),
            "{shape:?}"
        );
        assert_eq!(
            snap[&AVG][&CoordKey::new()],
            CellValue::Num(avg),
            "{shape:?}"
        );
        let at = w[N / 2];
        let snap = engine
            .set(X, CoordKey::from_slice(&[(W, at.0)]), 0.1)
            .unwrap();
        m.set_input(
            X,
            Coordinate::from_pairs([(CategoryId(W), at)]),
            Value::Number(0.1),
        );
        let fresh = evaluate(&m, &[SUM, AVG]).unwrap();
        for id in [SUM, AVG] {
            assert_eq!(
                snap[&id][&CoordKey::new()],
                fresh[&id][&CoordKey::new()],
                "{shape:?} {id:?} after an edit"
            );
        }
        // A left fold is measurably off, so the goldens pin the compensation.
        let naive: f64 = xs.iter().sum();
        assert_ne!(naive.to_bits(), sum, "{shape:?}");
    }
}
