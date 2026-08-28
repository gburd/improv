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
//! (arithmetic + comparison/logical encoded as 1.0/0.0), Join (dimension
//! alignment), Aggregate (SUM/AVG/MIN/MAX), and scalar FuncCall built-ins
//! (ABS/ROUND/FLOOR/CEIL/SQRT/NEG, MIN2/MAX2). Non-numeric derived values
//! (Text/Boolean stored as such) and Date values are carried in `CellValue`;
//! comparison/logical results live in the numeric lane as 1.0/0.0.

use crate::compiler::{compile_formula, scalar_arity, CompileContext};
use crate::plan::{PlanNode, PlanNodeKind};
use crate::{encode_coord, CellValue, CoordKey};
use differential_dataflow::input::InputSession;
use differential_dataflow::operators::{Join as _, Reduce};
use differential_dataflow::AsCollection;
use differential_dataflow::Collection;
use improv_core_model::{BinaryOp, CategoryId, FuncId, MeasureId, MeasureKind, Model, UnaryOp};
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use timely::dataflow::operators::ToStream;
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

/// A DD collection of `(coordinate, cell value)`.
type Coll<G> = Collection<G, (CoordKey, CellValue), isize>;

/// Recursively build a collection for a plan node.
///
/// `inputs` maps input-measure ids to their base DD collections. A `Ref` to a
/// *derived* measure resolves to its already-built collection in `derived`.
pub(crate) fn build_coll<G: Scope>(
    node: &PlanNode,
    scope: &mut G,
    inputs: &HashMap<MeasureId, Coll<G>>,
    derived: &HashMap<MeasureId, Coll<G>>,
) -> Result<Coll<G>, EngineError>
where
    G::Timestamp: differential_dataflow::lattice::Lattice + timely::progress::Timestamp,
{
    match &node.kind {
        PlanNodeKind::InputMeasure(m) => inputs
            .get(m)
            .or_else(|| derived.get(m))
            .cloned()
            .ok_or_else(|| EngineError::Unsupported(format!("no collection for measure {m:?}"))),

        PlanNodeKind::MapUnary(op, child) => {
            let c = build_coll(child, scope, inputs, derived)?;
            let op = *op;
            Ok(c.map(move |(k, v)| (k, apply_unary(op, &v))))
        }

        PlanNodeKind::MapBinary(op, left, right) => {
            // Operands are dimension-aligned by an enclosing Join (or equal
            // dims). A *literal* operand has no collection of its own; fold it
            // into the map closure so it broadcasts over the other operand.
            let op = *op;
            match (literal_value(left), literal_value(right)) {
                (Some(lit), None) => {
                    let r = build_coll(right, scope, inputs, derived)?;
                    Ok(r.map(move |(k, b)| (k, apply_binary(op, &lit, &b))))
                }
                (None, Some(lit)) => {
                    let l = build_coll(left, scope, inputs, derived)?;
                    Ok(l.map(move |(k, a)| (k, apply_binary(op, &a, &lit))))
                }
                (Some(a), Some(b)) => {
                    // Both literals: a scalar result at the empty coordinate.
                    let v = apply_binary(op, &a, &b);
                    let empty: CoordKey = Vec::new();
                    let t = <G::Timestamp as timely::progress::Timestamp>::minimum();
                    Ok(vec![((empty, v), t, 1isize)]
                        .to_stream(scope)
                        .as_collection())
                }
                (None, None) => {
                    let l = build_coll(left, scope, inputs, derived)?;
                    let r = build_coll(right, scope, inputs, derived)?;
                    Ok(l.join(&r)
                        .map(move |(k, (a, b))| (k, apply_binary(op, &a, &b))))
                }
            }
        }

        PlanNodeKind::Join {
            left,
            right,
            join_keys,
        } => {
            // Re-key both sides to the shared join categories, join, then
            // rebuild the union key. The union of the two full keys is the
            // result coordinate.
            let l = build_coll(left, scope, inputs, derived)?;
            let r = build_coll(right, scope, inputs, derived)?;
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
            let c = build_coll(input, scope, inputs, derived)?;
            let gb = group_by.clone();
            let func = *func;
            // Re-key to the group-by coordinate, then reduce.
            let keyed = c.map(move |(k, v)| (project(&k, &gb), v));
            Ok(keyed.reduce(move |_key, input, output| {
                let vals: Vec<f64> = input
                    .iter()
                    .flat_map(|(v, mult)| {
                        let n = (*mult).max(0) as usize;
                        let x = v.as_num().unwrap_or(f64::NAN);
                        std::iter::repeat_n(x, n)
                    })
                    .collect();
                if vals.is_empty() {
                    return;
                }
                let agg = aggregate(func, &vals);
                output.push((CellValue::num(agg), 1isize));
            }))
        }

        PlanNodeKind::Literal(v) => {
            // A standalone literal is a *scalar* measure: one value at the empty
            // coordinate, which broadcasts (via Join) into any formula that
            // references it. Build a constant single-cell collection.
            let cv = CellValue::from_model_value(v)
                .ok_or_else(|| EngineError::Unsupported("literal has no cell value".into()))?;
            let empty: CoordKey = Vec::new();
            let t = <G::Timestamp as timely::progress::Timestamp>::minimum();
            Ok(vec![((empty, cv), t, 1isize)]
                .to_stream(scope)
                .as_collection())
        }
        PlanNodeKind::FuncCall { func, args } => {
            let func = *func;
            let arity = scalar_arity(func)
                .ok_or_else(|| EngineError::Unsupported(format!("unknown scalar func {func:?}")))?;
            if args.len() != arity {
                return Err(EngineError::Unsupported(format!(
                    "scalar {func:?} arity {arity}, got {} args",
                    args.len()
                )));
            }
            match args.as_slice() {
                [a] => {
                    let c = build_coll(a, scope, inputs, derived)?;
                    Ok(c.map(move |(k, v)| {
                        (
                            k,
                            CellValue::num(apply_scalar(func, &[v.as_num().unwrap_or(f64::NAN)])),
                        )
                    }))
                }
                [a, b] => {
                    // Align by shared categories (broadcast the smaller dim
                    // over the larger), join, apply the function on both
                    // decoded values, rebuild the union coordinate.
                    let l = build_coll(a, scope, inputs, derived)?;
                    let r = build_coll(b, scope, inputs, derived)?;
                    let keys: Vec<CategoryId> =
                        a.ty.dim
                            .categories
                            .iter()
                            .copied()
                            .filter(|c| b.ty.dim.categories.contains(c))
                            .collect();
                    let keys2 = keys.clone();
                    let l_keyed = l.map(move |(k, v)| (project(&k, &keys), (k, v)));
                    let r_keyed = r.map(move |(k, v)| (project(&k, &keys2), (k, v)));
                    Ok(l_keyed
                        .join(&r_keyed)
                        .map(move |(_jk, ((lk, lv), (rk, rv)))| {
                            let merged = union_keys(&lk, &rk);
                            let out = apply_scalar(
                                func,
                                &[
                                    lv.as_num().unwrap_or(f64::NAN),
                                    rv.as_num().unwrap_or(f64::NAN),
                                ],
                            );
                            (merged, CellValue::num(out))
                        }))
                }
                _ => Err(EngineError::Unsupported(format!(
                    "scalar func arity {} unsupported",
                    args.len()
                ))),
            }
        }
    }
}

/// If a plan node is a literal, return its `CellValue` (for folding a constant
/// operand into a `MapBinary` map rather than materializing a collection).
fn literal_value(node: &PlanNode) -> Option<CellValue> {
    match &node.kind {
        PlanNodeKind::Literal(v) => CellValue::from_model_value(v),
        _ => None,
    }
}

fn apply_binary(op: BinaryOp, a: &CellValue, b: &CellValue) -> CellValue {
    // Equality/inequality work on any value type (Text, Bool, Num, Err);
    // everything else operates on the numeric lane.
    match op {
        BinaryOp::Eq => return CellValue::Bool(a == b),
        BinaryOp::Ne => return CellValue::Bool(a != b),
        _ => {}
    }
    let (x, y) = (
        a.as_num().unwrap_or(f64::NAN),
        b.as_num().unwrap_or(f64::NAN),
    );
    match op {
        BinaryOp::Add => CellValue::num(x + y),
        BinaryOp::Sub => CellValue::num(x - y),
        BinaryOp::Mul => CellValue::num(x * y),
        BinaryOp::Div => CellValue::num(if y == 0.0 { f64::NAN } else { x / y }),
        BinaryOp::And => CellValue::Bool(truthy(a) && truthy(b)),
        BinaryOp::Or => CellValue::Bool(truthy(a) || truthy(b)),
        BinaryOp::Lt => CellValue::Bool(x < y),
        BinaryOp::Le => CellValue::Bool(x <= y),
        BinaryOp::Gt => CellValue::Bool(x > y),
        BinaryOp::Ge => CellValue::Bool(x >= y),
        BinaryOp::Eq | BinaryOp::Ne => unreachable!("handled above"),
    }
}

fn apply_unary(op: UnaryOp, v: &CellValue) -> CellValue {
    match op {
        UnaryOp::Neg => CellValue::num(-v.as_num().unwrap_or(f64::NAN)),
        UnaryOp::Not => CellValue::Bool(!truthy(v)),
    }
}

/// Truthiness for logical ops: `Bool` directly, a nonzero `Num`, a non-empty
/// `Text`; errors are falsy.
fn truthy(v: &CellValue) -> bool {
    match v {
        CellValue::Bool(b) => *b,
        CellValue::Num(bits) => f64::from_bits(*bits) != 0.0,
        CellValue::Text(t) => !t.is_empty(),
        CellValue::Date(ms) => *ms != 0,
        CellValue::Err(_) => false,
    }
}

/// Evaluate a scalar built-in on its decoded f64 args. Ids match the compiler's
/// `scalar_arity` registry. Domain errors (SQRT of a negative) yield NaN,
/// consistent with the Div-by-zero -> NaN convention. Deterministic: pure f64
/// arithmetic, no ordering dependence.
fn apply_scalar(func: FuncId, args: &[f64]) -> f64 {
    match (func.0, args) {
        (10, [a]) => a.abs(),
        (11, [a]) => a.round(),
        (12, [a]) => a.floor(),
        (13, [a]) => a.ceil(),
        (14, [a]) => {
            if *a < 0.0 {
                f64::NAN
            } else {
                a.sqrt()
            }
        }
        (15, [a]) => -a,
        (20, [a, b]) => a.min(*b),
        (21, [a, b]) => a.max(*b),
        _ => f64::NAN,
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

/// Compute the derived measures transitively needed by `targets`, in
/// dependency order (a measure comes after every derived measure it
/// references). Rejects cycles.
pub(crate) fn derived_build_order(
    model: &Model,
    targets: &[MeasureId],
) -> Result<Vec<MeasureId>, EngineError> {
    let mut order: Vec<MeasureId> = Vec::new();
    let mut visited: std::collections::HashSet<MeasureId> = std::collections::HashSet::new();
    let mut on_stack: std::collections::HashSet<MeasureId> = std::collections::HashSet::new();

    fn visit(
        model: &Model,
        m: MeasureId,
        order: &mut Vec<MeasureId>,
        visited: &mut std::collections::HashSet<MeasureId>,
        on_stack: &mut std::collections::HashSet<MeasureId>,
    ) -> Result<(), EngineError> {
        if visited.contains(&m) {
            return Ok(());
        }
        let measure = match model.measures.get(&m) {
            Some(x) => x,
            None => return Ok(()), // unknown ref: the compiler will report it
        };
        // Only derived measures participate in the build order.
        let formula = match &measure.kind {
            MeasureKind::Derived(f) => f,
            MeasureKind::Input => {
                visited.insert(m);
                return Ok(());
            }
        };
        if !on_stack.insert(m) {
            return Err(EngineError::Unsupported(format!(
                "cyclic measure dependency at {m:?}"
            )));
        }
        for dep in formula.referenced_measures() {
            visit(model, dep, order, visited, on_stack)?;
        }
        on_stack.remove(&m);
        visited.insert(m);
        order.push(m);
        Ok(())
    }

    for t in targets {
        visit(model, *t, &mut order, &mut visited, &mut on_stack)?;
    }
    Ok(order)
}

/// Run `model` to completion and return computed values for the requested
/// derived measures: `measure -> (coordinate-key -> value)`.
///
/// Numeric input + derived measures. Supports **multi-layer** derivation: a
/// derived measure may reference other derived measures; they are built in
/// topological order so each layer feeds the next. Cyclic dependencies are
/// rejected.
pub fn evaluate(
    model: &Model,
    targets: &[MeasureId],
) -> Result<HashMap<MeasureId, HashMap<CoordKey, CellValue>>, EngineError> {
    let ctx = CompileContext::new(&model.measures);

    // Determine every derived measure transitively needed by `targets`, in
    // dependency (topological) order: a measure appears after all derived
    // measures it references.
    let order = derived_build_order(model, targets)?;

    // Compile each derived measure in that order.
    let mut plans: Vec<(MeasureId, PlanNode)> = Vec::new();
    for m in &order {
        if let Some(measure) = model.measures.get(m) {
            if let MeasureKind::Derived(f) = &measure.kind {
                plans.push((*m, compile_formula(&ctx, *m, f)?));
            }
        }
    }

    // Gather input cells to feed, grouped by measure. Every model value type
    // maps into the DD lane: Number, Boolean, Text, Enum, DateTime (as
    // CellValue::Date millis), and Error.
    let mut input_cells: HashMap<MeasureId, Vec<(CoordKey, CellValue)>> = HashMap::new();
    for ((mid, coord), val) in &model.inputs {
        if let Some(cv) = CellValue::from_model_value(val) {
            input_cells
                .entry(*mid)
                .or_default()
                .push((encode_coord(coord), cv));
        }
    }

    let results: Arc<Mutex<HashMap<MeasureId, HashMap<CoordKey, CellValue>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let results2 = results.clone();

    // Which input measures do we need sessions for? All that appear in inputs.
    let input_ids: Vec<MeasureId> = input_cells.keys().copied().collect();
    let plans_arc = plans.clone();

    timely::execute::execute_directly(move |worker| {
        let mut sessions: HashMap<MeasureId, InputSession<u64, (CoordKey, CellValue), isize>> =
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

            let mut derived: HashMap<MeasureId, Coll<_>> = HashMap::new();
            for (mid, plan) in &plans_arc {
                if let Ok(coll) = build_coll(plan, scope, &input_colls, &derived) {
                    let mid = *mid;
                    let res = res.clone();
                    // Collapse to one value per key.
                    let reduced = coll.reduce(|_k, inp, out| {
                        for (val, mult) in inp {
                            if *mult > 0 {
                                out.push(((*val).clone(), 1isize));
                            }
                        }
                    });
                    // Register this derived measure so later layers can
                    // reference it as an InputMeasure.
                    derived.insert(mid, reduced.clone());
                    reduced.inspect(move |((k, val), _t, diff)| {
                        if *diff > 0 {
                            res.lock()
                                .unwrap()
                                .entry(mid)
                                .or_default()
                                .insert(k.clone(), val.clone());
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
                    session.insert((k.clone(), v.clone()));
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
        Coordinate, DimensionSpec, Expr, ItemId, Measure, MeasureKind, Name, Value, ValueType,
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
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 20)])).and_then(|v| v.as_num()),
            Some(1000.0)
        );
        // Revenue[2025,B] = 20*50 = 1000
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 21)])).and_then(|v| v.as_num()),
            Some(1000.0)
        );
        // Revenue[2026,A] = 10*120 = 1200
        assert_eq!(
            rev.get(&key(&[(1, 11), (2, 20)])).and_then(|v| v.as_num()),
            Some(1200.0)
        );
        // Revenue[2026,B] = 20*80 = 1600
        assert_eq!(
            rev.get(&key(&[(1, 11), (2, 21)])).and_then(|v| v.as_num()),
            Some(1600.0)
        );
        assert_eq!(rev.len(), 4);
    }

    #[test]
    fn evaluate_multi_layer_derivation() {
        // RevenueByProduct[Product] = SUM(Revenue OVER Time), where Revenue is
        // itself a derived measure -> two dataflow layers.
        let mut model = revenue_model();
        let (time, product) = (CategoryId(1), CategoryId(2));
        let over_time = improv_core_model::DimensionSpec {
            by: vec![product],
            over: vec![time],
            except: vec![],
        };
        model.add_measure(Measure {
            id: MeasureId(103),
            name: Name("RevenueByProduct".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Derived(improv_core_model::Formula::new(Expr::Call(
                improv_core_model::FuncId(1), // SUM
                vec![Expr::Ref(MeasureId(102), over_time)],
            ))),
            description: None,
        });

        let out = evaluate(&model, &[MeasureId(103)]).expect("evaluate");
        let rbp = out.get(&MeasureId(103)).expect("RevenueByProduct computed");
        // WidgetA: 1000 + 1200 = 2200 ; WidgetB: 1000 + 1600 = 2600
        assert_eq!(
            rbp.get(&key(&[(2, 20)])).and_then(|v| v.as_num()),
            Some(2200.0)
        );
        assert_eq!(
            rbp.get(&key(&[(2, 21)])).and_then(|v| v.as_num()),
            Some(2600.0)
        );
        assert_eq!(rbp.len(), 2);
    }

    // Scalar FuncCall evaluation over the revenue model: ABS (unary) and MIN2
    // (2-arg join path). Comparison-as-numeric is checked separately.
    #[test]
    fn evaluate_scalar_funcs_and_comparison() {
        let mut model = revenue_model();
        let (time, product) = (CategoryId(1), CategoryId(2));
        let refr = |id| Expr::Ref(id, DimensionSpec::default());
        let mk = |id: u32, name: &str, cats: Vec<CategoryId>, f: Expr| Measure {
            id: MeasureId(id),
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: cats,
            kind: MeasureKind::Derived(improv_core_model::Formula::new(f)),
            description: None,
        };

        // Profit[Time,Product] = Price[Product] - Quantity[Time,Product]
        // (negative everywhere: 10-100, 20-50, ...).
        model.add_measure(mk(
            110,
            "Profit",
            vec![time, product],
            Expr::BinaryOp(
                BinaryOp::Sub,
                Box::new(refr(MeasureId(100))),
                Box::new(refr(MeasureId(101))),
            ),
        ));
        // AbsProfit = ABS(Profit)
        model.add_measure(mk(
            111,
            "AbsProfit",
            vec![time, product],
            Expr::Call(FuncId(10), vec![refr(MeasureId(110))]),
        ));
        // MinPQ[Time,Product] = MIN2(Price[Product], Quantity[Time,Product])
        model.add_measure(mk(
            113,
            "MinPQ",
            vec![time, product],
            Expr::Call(FuncId(20), vec![refr(MeasureId(100)), refr(MeasureId(101))]),
        ));

        let out = evaluate(&model, &[MeasureId(111), MeasureId(113)]).expect("evaluate");

        // AbsProfit = |Price - Quantity|.
        let ap = out.get(&MeasureId(111)).expect("AbsProfit");
        assert_eq!(
            ap.get(&key(&[(1, 10), (2, 20)])).and_then(|v| v.as_num()),
            Some(90.0)
        ); // |10-100|
        assert_eq!(
            ap.get(&key(&[(1, 10), (2, 21)])).and_then(|v| v.as_num()),
            Some(30.0)
        ); // |20-50|
        assert_eq!(
            ap.get(&key(&[(1, 11), (2, 20)])).and_then(|v| v.as_num()),
            Some(110.0)
        ); // |10-120|
        assert_eq!(
            ap.get(&key(&[(1, 11), (2, 21)])).and_then(|v| v.as_num()),
            Some(60.0)
        ); // |20-80|

        // MinPQ = min(Price, Quantity) = Price everywhere (Price<Quantity).
        let mp = out.get(&MeasureId(113)).expect("MinPQ");
        assert_eq!(
            mp.get(&key(&[(1, 10), (2, 20)])).and_then(|v| v.as_num()),
            Some(10.0)
        );
        assert_eq!(
            mp.get(&key(&[(1, 11), (2, 21)])).and_then(|v| v.as_num()),
            Some(20.0)
        );

        // Determinism: a second run yields identical results.
        let out2 = evaluate(&model, &[MeasureId(111), MeasureId(113)]).expect("evaluate");
        assert_eq!(out.get(&MeasureId(111)), out2.get(&MeasureId(111)));
        assert_eq!(out.get(&MeasureId(113)), out2.get(&MeasureId(113)));
    }

    // Comparison result encoded as 1.0/0.0 in the numeric lane:
    // Hot[Product] = Price > Threshold, an input Threshold[Product] = 15.
    #[test]
    fn evaluate_comparison_yields_boolean() {
        let mut model = revenue_model();
        let product = CategoryId(2);
        let refr = |id| Expr::Ref(id, DimensionSpec::default());

        // Threshold[Product] = 15 for both products (input).
        model.add_measure(Measure {
            id: MeasureId(120),
            name: Name("Threshold".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        let coord = |pairs: &[(CategoryId, ItemId)]| Coordinate::from_pairs(pairs.iter().copied());
        model.set_input(
            MeasureId(120),
            coord(&[(product, ItemId(20))]),
            Value::Number(15.0),
        );
        model.set_input(
            MeasureId(120),
            coord(&[(product, ItemId(21))]),
            Value::Number(15.0),
        );

        // Hot[Product] = Price > Threshold  (Price A=10 -> 0.0, B=20 -> 1.0).
        model.add_measure(Measure {
            id: MeasureId(121),
            name: Name("Hot".into()),
            value_type: ValueType::Boolean,
            categories: vec![product],
            kind: MeasureKind::Derived(improv_core_model::Formula::new(Expr::BinaryOp(
                BinaryOp::Gt,
                Box::new(refr(MeasureId(100))),
                Box::new(refr(MeasureId(120))),
            ))),
            description: None,
        });

        let out = evaluate(&model, &[MeasureId(121)]).expect("evaluate");
        let hot = out.get(&MeasureId(121)).expect("Hot");
        assert_eq!(
            hot.get(&key(&[(2, 20)])),
            Some(&CellValue::Bool(false)),
            "Price 10 not > 15"
        );
        assert_eq!(
            hot.get(&key(&[(2, 21)])),
            Some(&CellValue::Bool(true)),
            "Price 20 > 15"
        );
    }

    #[test]
    fn evaluate_comparison_against_literal_broadcasts() {
        // Expensive[Product] = Price > 15  (a *literal* RHS, no Threshold
        // measure) — exercises literal folding + broadcast in MapBinary.
        let mut model = Model::new();
        let product = CategoryId(2);
        model.add_category(product, "Product");
        model.add_item(ItemId(20), product, "WidgetA");
        model.add_item(ItemId(21), product, "WidgetB");
        model.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        model.add_measure(Measure {
            id: MeasureId(130),
            name: Name("Expensive".into()),
            value_type: ValueType::Boolean,
            categories: vec![product],
            kind: MeasureKind::Derived(improv_core_model::Formula::new(Expr::BinaryOp(
                BinaryOp::Gt,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Literal(Value::Number(15.0))),
            ))),
            description: None,
        });
        let c = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
        model.set_input(
            MeasureId(100),
            c(&[(product, ItemId(20))]),
            Value::Number(10.0),
        );
        model.set_input(
            MeasureId(100),
            c(&[(product, ItemId(21))]),
            Value::Number(20.0),
        );

        let out = evaluate(&model, &[MeasureId(130)]).expect("evaluate");
        let exp = out.get(&MeasureId(130)).expect("Expensive");
        assert_eq!(exp.get(&key(&[(2, 20)])), Some(&CellValue::Bool(false)));
        assert_eq!(exp.get(&key(&[(2, 21)])), Some(&CellValue::Bool(true)));
    }

    #[test]
    fn evaluate_standalone_literal_is_a_scalar_measure() {
        // TaxRate = 0.08  (a bare literal RHS, no measure ref) evaluates to a
        // single scalar cell at the empty coordinate.
        let mut model = Model::new();
        model.add_measure(Measure {
            id: MeasureId(140),
            name: Name("TaxRate".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Derived(improv_core_model::Formula::new(Expr::Literal(
                Value::Number(0.08),
            ))),
            description: None,
        });
        let out = evaluate(&model, &[MeasureId(140)]).expect("evaluate");
        let tr = out.get(&MeasureId(140)).expect("TaxRate");
        // One cell, at the empty coordinate.
        assert_eq!(tr.len(), 1);
        assert_eq!(tr.get(&key(&[])), Some(&CellValue::num(0.08)));
    }

    #[test]
    fn cyclic_dependency_is_rejected() {
        // A -> B -> A cycle must error, not loop forever.
        let mut model = Model::new();
        let mul = |a, b| {
            improv_core_model::Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(a, DimensionSpec::default())),
                Box::new(Expr::Ref(b, DimensionSpec::default())),
            ))
        };
        model.add_measure(Measure {
            id: MeasureId(1),
            name: Name("A".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Derived(mul(MeasureId(2), MeasureId(2))),
            description: None,
        });
        model.add_measure(Measure {
            id: MeasureId(2),
            name: Name("B".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Derived(mul(MeasureId(1), MeasureId(1))),
            description: None,
        });
        assert!(
            evaluate(&model, &[MeasureId(1)]).is_err(),
            "cycle must be rejected"
        );
    }
}
