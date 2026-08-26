//! Build a differential-dataflow graph from compiled `PlanNode`s and run a
//! whole `Model` to a set of computed derived-measure values.
//!
//! This connects the formula compiler (`compiler`) to the DD substrate proven
//! by the spike in `lib.rs`. Encoding is the same the spike validated:
//!   * DD key   = serialized coordinate `CoordKey` = `Vec<(u32,u32)>`
//!   * DD value = `f64::to_bits()` as `u64` (numeric measures only, v1)
//!   * diff `R` = `isize`; a final `reduce` collapses to one value per key.
//!
//! Scope (v1, numeric): InputMeasure, Literal, MapUnary(Neg), MapBinary
//! (arithmetic), Join (dimension alignment), Aggregate (SUM/AVG/MIN/MAX).
//! Non-numeric values and general FuncCall are future work.

use crate::compiler::{compile_formula, CompileContext};
use crate::plan::{PlanNode, PlanNodeKind};
use crate::{encode_coord, CoordKey};
use differential_dataflow::input::InputSession;
use differential_dataflow::operators::{Join as _, Reduce};
use differential_dataflow::Collection;
use improv_core_model::{
    BinaryOp, CategoryId, FuncId, MeasureId, MeasureKind, Model, UnaryOp, Value,
};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use timely::dataflow::Scope;

/// SUM/AVG/MIN/MAX func ids (must match the compiler's convention).
const SUM: FuncId = FuncId(1);
const AVG: FuncId = FuncId(2);
const MIN: FuncId = FuncId(3);
const MAX: FuncId = FuncId(4);

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("compile error: {0}")]
    Compile(#[from] crate::compiler::CompileError),
    #[error("unsupported plan node in v1 dataflow builder: {0}")]
    Unsupported(String),
}

/// A DD collection of `(coordinate, f64-bits)`.
type Coll<G> = Collection<G, (CoordKey, u64), isize>;

/// Recursively build a collection for a plan node.
///
/// `inputs` maps input-measure ids to their base DD collections. A `Ref` to a
/// *derived* measure resolves to its already-built collection in `derived`.
fn build<G: Scope>(
    node: &PlanNode,
    inputs: &HashMap<MeasureId, Coll<G>>,
    derived: &HashMap<MeasureId, Coll<G>>,
) -> Result<Coll<G>, EngineError>
where
    G::Timestamp: differential_dataflow::lattice::Lattice,
{
    match &node.kind {
        PlanNodeKind::InputMeasure(m) => inputs
            .get(m)
            .or_else(|| derived.get(m))
            .cloned()
            .ok_or_else(|| EngineError::Unsupported(format!("no collection for measure {m:?}"))),

        PlanNodeKind::MapUnary(op, child) => {
            let c = build(child, inputs, derived)?;
            let op = *op;
            Ok(c.map(move |(k, bits)| {
                let v = f64::from_bits(bits);
                let out = match op {
                    UnaryOp::Neg => -v,
                    // Boolean not on a numeric encoding: treat !=0 as true.
                    UnaryOp::Not => {
                        if v == 0.0 {
                            1.0
                        } else {
                            0.0
                        }
                    }
                };
                (k, out.to_bits())
            }))
        }

        PlanNodeKind::MapBinary(op, left, right) => {
            // Operands are dimension-aligned by an enclosing Join (or equal
            // dims). Align by key and apply the op element-wise.
            let l = build(left, inputs, derived)?;
            let r = build(right, inputs, derived)?;
            let op = *op;
            Ok(l.join(&r).map(move |(k, (lb, rb))| {
                let a = f64::from_bits(lb);
                let b = f64::from_bits(rb);
                (k, apply_binary(op, a, b).to_bits())
            }))
        }

        PlanNodeKind::Join {
            left,
            right,
            join_keys,
        } => {
            // Re-key both sides to the shared join categories, join, then
            // rebuild the union key. The union of the two full keys is the
            // result coordinate.
            let l = build(left, inputs, derived)?;
            let r = build(right, inputs, derived)?;
            let keys = join_keys.clone();
            let keys2 = keys.clone();
            let l_keyed = l.map(move |(k, v)| (project(&k, &keys), (k, v)));
            let r_keyed = r.map(move |(k, v)| (project(&k, &keys2), (k, v)));
            Ok(l_keyed.join(&r_keyed).map(|(_jk, ((lk, lv), (rk, _rv)))| {
                // Union the two coordinates (rk contributes its extra dims).
                let merged = union_keys(&lk, &rk);
                // Value carried is the left operand's; MapBinary above
                // re-fetches both operands, so a Join feeding a MapBinary
                // is handled by MapBinary. When a Join stands alone we keep
                // the left value.
                (merged, lv)
            }))
        }

        PlanNodeKind::Aggregate {
            input,
            group_by,
            func,
        } => {
            let c = build(input, inputs, derived)?;
            let gb = group_by.clone();
            let func = *func;
            // Re-key to the group-by coordinate, then reduce.
            let keyed = c.map(move |(k, v)| (project(&k, &gb), v));
            Ok(keyed.reduce(move |_key, input, output| {
                let vals: Vec<f64> = input
                    .iter()
                    .flat_map(|(bits, mult)| {
                        let n = (*mult).max(0) as usize;
                        std::iter::repeat_n(f64::from_bits(**bits), n)
                    })
                    .collect();
                if vals.is_empty() {
                    return;
                }
                let agg = aggregate(func, &vals);
                output.push((agg.to_bits(), 1isize));
            }))
        }

        PlanNodeKind::Literal(Value::Number(n)) => {
            // A scalar literal has no coordinate; emit under the empty key.
            // (Broadcasting a literal against a dimensioned operand is done by
            // the MapBinary join, which will find the empty-key literal only if
            // join keys are empty — adequate for v1 scalar-on-scalar.)
            let bits = n.to_bits();
            // We can't create a collection from thin air here without an input;
            // literals are only supported inside expressions the compiler has
            // already dimensioned. Signal unsupported standalone use.
            let _ = bits;
            Err(EngineError::Unsupported(
                "standalone numeric literal (needs a dimensioned context)".into(),
            ))
        }

        PlanNodeKind::Literal(_) => Err(EngineError::Unsupported("non-numeric literal".into())),
        PlanNodeKind::FuncCall { .. } => Err(EngineError::Unsupported("general FuncCall".into())),
    }
}

fn apply_binary(op: BinaryOp, a: f64, b: f64) -> f64 {
    match op {
        BinaryOp::Add => a + b,
        BinaryOp::Sub => a - b,
        BinaryOp::Mul => a * b,
        BinaryOp::Div => {
            if b == 0.0 {
                f64::NAN
            } else {
                a / b
            }
        }
        BinaryOp::And => bool_f64(a != 0.0 && b != 0.0),
        BinaryOp::Or => bool_f64(a != 0.0 || b != 0.0),
        BinaryOp::Eq => bool_f64(a == b),
        BinaryOp::Ne => bool_f64(a != b),
        BinaryOp::Lt => bool_f64(a < b),
        BinaryOp::Le => bool_f64(a <= b),
        BinaryOp::Gt => bool_f64(a > b),
        BinaryOp::Ge => bool_f64(a >= b),
    }
}

fn bool_f64(b: bool) -> f64 {
    if b {
        1.0
    } else {
        0.0
    }
}

fn aggregate(func: FuncId, vals: &[f64]) -> f64 {
    match func {
        SUM => vals.iter().sum(),
        AVG => vals.iter().sum::<f64>() / vals.len() as f64,
        MIN => vals.iter().cloned().fold(f64::INFINITY, f64::min),
        MAX => vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        _ => f64::NAN,
    }
}

/// Project a coordinate key down to a subset of category ids.
fn project(k: &CoordKey, keep: &[CategoryId]) -> CoordKey {
    let keep: Vec<u32> = keep.iter().map(|c| c.0).collect();
    k.iter()
        .filter(|(cat, _)| keep.contains(cat))
        .copied()
        .collect()
}

/// Merge two coordinate keys (right's extra dims added to left). Assumes shared
/// dims agree (they were the join key).
fn union_keys(a: &CoordKey, b: &CoordKey) -> CoordKey {
    let mut m: BTreeMap<u32, u32> = a.iter().copied().collect();
    for (c, i) in b {
        m.entry(*c).or_insert(*i);
    }
    m.into_iter().collect()
}

/// Run `model` to completion and return computed values for the requested
/// derived measures: `measure -> (coordinate-key -> value)`.
///
/// v1: numeric input + derived measures only. Derived measures must reference
/// only input measures (a single dataflow layer); multi-layer derivation is a
/// follow-up (topological build over `derived`).
pub fn evaluate(
    model: &Model,
    targets: &[MeasureId],
) -> Result<HashMap<MeasureId, HashMap<CoordKey, f64>>, EngineError> {
    // Compile each target's plan up front (outside the timely closure).
    let ctx = CompileContext::new(&model.measures);
    let mut plans: Vec<(MeasureId, PlanNode)> = Vec::new();
    for m in targets {
        if let Some(measure) = model.measures.get(m) {
            if let MeasureKind::Derived(f) = &measure.kind {
                plans.push((*m, compile_formula(&ctx, *m, f)?));
            }
        }
    }

    // Gather input cells to feed, grouped by measure.
    let mut input_cells: HashMap<MeasureId, Vec<(CoordKey, u64)>> = HashMap::new();
    for ((mid, coord), val) in &model.inputs {
        if let Value::Number(n) = val {
            input_cells
                .entry(*mid)
                .or_default()
                .push((encode_coord(coord), n.to_bits()));
        }
    }

    let results: Arc<Mutex<HashMap<MeasureId, HashMap<CoordKey, f64>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let results2 = results.clone();

    // Which input measures do we need sessions for? All that appear in inputs.
    let input_ids: Vec<MeasureId> = input_cells.keys().copied().collect();
    let plans_arc = plans.clone();

    timely::execute::execute_directly(move |worker| {
        let mut sessions: HashMap<MeasureId, InputSession<u64, (CoordKey, u64), isize>> =
            HashMap::new();
        let res = results2.clone();

        worker.dataflow::<u64, _, _>(|scope| {
            let mut input_colls: HashMap<MeasureId, Coll<_>> = HashMap::new();
            for id in &input_ids {
                let mut session = InputSession::new();
                let coll = session.to_collection(scope);
                sessions.insert(*id, session);
                input_colls.insert(*id, coll);
            }

            let derived: HashMap<MeasureId, Coll<_>> = HashMap::new();
            for (mid, plan) in &plans_arc {
                if let Ok(coll) = build(plan, &input_colls, &derived) {
                    let mid = *mid;
                    let res = res.clone();
                    // Collapse to one value per key, then capture.
                    coll.reduce(|_k, inp, out| {
                        for (bits, mult) in inp {
                            if *mult > 0 {
                                out.push((**bits, 1isize));
                            }
                        }
                    })
                    .inspect(move |((k, bits), _t, diff)| {
                        if *diff > 0 {
                            res.lock()
                                .unwrap()
                                .entry(mid)
                                .or_default()
                                .insert(k.clone(), f64::from_bits(*bits));
                        }
                    });
                }
            }
        });

        // Feed all input cells at time 0, advance, and run to completion.
        for (id, session) in sessions.iter_mut() {
            session.advance_to(0);
            if let Some(cells) = input_cells.get(id) {
                for (k, v) in cells {
                    session.insert((k.clone(), *v));
                }
            }
        }
        for session in sessions.values_mut() {
            session.advance_to(1);
            session.flush();
        }
        while sessions.values().any(|s| s.time() < &1) {
            worker.step();
        }
    });

    let out = results.lock().unwrap().clone();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{
        Coordinate, DimensionSpec, Expr, ItemId, Measure, MeasureKind, Name, ValueType,
    };

    // The canonical Time x Product revenue model with known results.
    fn revenue_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(11), time, "2026");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(21), product, "WidgetB");

        // Price[Product] (input)
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        // Quantity[Time,Product] (input)
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Input,
            description: None,
        });
        // Revenue[Time,Product] = Price[Product] * Quantity[Time,Product]
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Derived(improv_core_model::Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });

        let coord = |pairs: &[(CategoryId, ItemId)]| Coordinate::from_pairs(pairs.iter().copied());
        // Price
        m.set_input(
            MeasureId(100),
            coord(&[(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m.set_input(
            MeasureId(100),
            coord(&[(product, ItemId(21))]),
            Value::Number(20.0),
        );
        // Quantity
        m.set_input(
            MeasureId(101),
            coord(&[(time, ItemId(10)), (product, ItemId(20))]),
            Value::Number(100.0),
        );
        m.set_input(
            MeasureId(101),
            coord(&[(time, ItemId(10)), (product, ItemId(21))]),
            Value::Number(50.0),
        );
        m.set_input(
            MeasureId(101),
            coord(&[(time, ItemId(11)), (product, ItemId(20))]),
            Value::Number(120.0),
        );
        m.set_input(
            MeasureId(101),
            coord(&[(time, ItemId(11)), (product, ItemId(21))]),
            Value::Number(80.0),
        );
        m
    }

    fn key(pairs: &[(u32, u32)]) -> CoordKey {
        let mut k: Vec<(u32, u32)> = pairs.to_vec();
        k.sort();
        k
    }

    #[test]
    fn evaluate_revenue_matches_known_results() {
        let model = revenue_model();
        let out = evaluate(&model, &[MeasureId(102)]).expect("evaluate");
        let rev = out.get(&MeasureId(102)).expect("revenue computed");

        // Revenue[2025,A] = 10*100 = 1000
        assert_eq!(rev.get(&key(&[(1, 10), (2, 20)])), Some(&1000.0));
        // Revenue[2025,B] = 20*50 = 1000
        assert_eq!(rev.get(&key(&[(1, 10), (2, 21)])), Some(&1000.0));
        // Revenue[2026,A] = 10*120 = 1200
        assert_eq!(rev.get(&key(&[(1, 11), (2, 20)])), Some(&1200.0));
        // Revenue[2026,B] = 20*80 = 1600
        assert_eq!(rev.get(&key(&[(1, 11), (2, 21)])), Some(&1600.0));
        assert_eq!(rev.len(), 4);
    }
}
