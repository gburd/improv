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
use improv_core_model::{
    BinaryOp, CategoryId, Formula, FuncId, MeasureId, MeasureKind, Model, UnaryOp,
};
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
pub(crate) type Coll<G> = Collection<G, (CoordKey, CellValue), isize>;

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
                    Ok(align(&l, &r, shared_cats(left, right))
                        .map(move |(k, (a, b))| (k, apply_binary(op, &a, &b))))
                }
            }
        }

        PlanNodeKind::Join {
            left,
            right,
            join_keys,
        } => {
            // Broadcast the left value onto the union coordinate. A MapBinary
            // above re-aligns this with its right operand on their shared
            // categories, so it sees both values.
            let l = build_coll(left, scope, inputs, derived)?;
            let r = build_coll(right, scope, inputs, derived)?;
            Ok(align(&l, &r, join_keys.clone()).map(|(k, (lv, _rv))| (k, lv)))
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
                    // over the larger), then apply the function.
                    let l = build_coll(a, scope, inputs, derived)?;
                    let r = build_coll(b, scope, inputs, derived)?;
                    Ok(
                        align(&l, &r, shared_cats(a, b)).map(move |(merged, (lv, rv))| {
                            let out = apply_scalar(
                                func,
                                &[
                                    lv.as_num().unwrap_or(f64::NAN),
                                    rv.as_num().unwrap_or(f64::NAN),
                                ],
                            );
                            (merged, CellValue::num(out))
                        }),
                    )
                }
                _ => Err(EngineError::Unsupported(format!(
                    "scalar func arity {} unsupported",
                    args.len()
                ))),
            }
        }
    }
}

/// Broadcast-join two collections on `keys`, the categories both range over.
/// Every pair that agrees on `keys` yields one row at the union coordinate
/// carrying both values, so either side may have extra dimensions. This is the
/// single alignment primitive for `Join`, `MapBinary` and two-arg `FuncCall`.
#[allow(clippy::type_complexity)]
fn align<G: Scope>(
    l: &Coll<G>,
    r: &Coll<G>,
    keys: Vec<CategoryId>,
) -> Collection<G, (CoordKey, (CellValue, CellValue)), isize>
where
    G::Timestamp: differential_dataflow::lattice::Lattice,
{
    let keys2 = keys.clone();
    let l = l.map(move |(k, v)| (project(&k, &keys), (k, v)));
    let r = r.map(move |(k, v)| (project(&k, &keys2), (k, v)));
    l.join(&r)
        .map(|(_jk, ((lk, lv), (rk, rv)))| (union_keys(&lk, &rk), (lv, rv)))
}

/// The categories both plan nodes range over: their broadcast join key.
fn shared_cats(a: &PlanNode, b: &PlanNode) -> Vec<CategoryId> {
    a.ty.dim
        .categories
        .iter()
        .copied()
        .filter(|c| b.ty.dim.categories.contains(c))
        .collect()
}

/// If a plan node is a literal, return its `CellValue` (for folding a constant
/// operand into a `MapBinary` map rather than materializing a collection).
fn literal_value(node: &PlanNode) -> Option<CellValue> {
    match &node.kind {
        PlanNodeKind::Literal(v) => CellValue::from_model_value(v),
        _ => None,
    }
}

pub(crate) fn apply_binary(op: BinaryOp, a: &CellValue, b: &CellValue) -> CellValue {
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

pub(crate) fn apply_unary(op: UnaryOp, v: &CellValue) -> CellValue {
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
pub(crate) fn apply_scalar(func: FuncId, args: &[f64]) -> f64 {
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
pub(crate) fn derived_build_order<'m>(
    model: &'m Model,
    targets: &[MeasureId],
) -> Result<Vec<(MeasureId, &'m Formula)>, EngineError> {
    let mut order: Vec<(MeasureId, &'m Formula)> = Vec::new();
    let mut visited: std::collections::HashSet<MeasureId> = std::collections::HashSet::new();
    let mut on_stack: std::collections::HashSet<MeasureId> = std::collections::HashSet::new();

    fn visit<'m>(
        model: &'m Model,
        m: MeasureId,
        order: &mut Vec<(MeasureId, &'m Formula)>,
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
        order.push((m, formula));
        Ok(())
    }

    for t in targets {
        visit(model, *t, &mut order, &mut visited, &mut on_stack)?;
    }
    Ok(order)
}

/// Compile every derived measure `targets` transitively needs, in build order.
pub(crate) fn compile_plans(
    model: &Model,
    targets: &[MeasureId],
) -> Result<Vec<(MeasureId, PlanNode)>, EngineError> {
    let ctx = CompileContext::new(&model.measures);
    derived_build_order(model, targets)?
        .into_iter()
        .map(|(m, f)| Ok((m, compile_formula(&ctx, m, f)?)))
        .collect()
}

/// Build each plan's collection, collapsed to one value per key, in order (so
/// a later plan can reference an earlier one). A plan that fails to build, such
/// as a ref to an input measure with no collection, is left out.
pub(crate) fn build_derived<G: Scope>(
    scope: &mut G,
    plans: &[(MeasureId, PlanNode)],
    inputs: &HashMap<MeasureId, Coll<G>>,
) -> Vec<(MeasureId, Coll<G>)>
where
    G::Timestamp: differential_dataflow::lattice::Lattice + timely::progress::Timestamp,
{
    let mut derived = HashMap::new();
    let mut built = Vec::new();
    for (mid, plan) in plans {
        let Ok(coll) = build_coll(plan, scope, inputs, &derived) else {
            continue;
        };
        let reduced = coll.reduce(|_k, inp, out| {
            for (val, mult) in inp {
                if *mult > 0 {
                    out.push(((*val).clone(), 1isize));
                }
            }
        });
        derived.insert(*mid, reduced.clone());
        built.push((*mid, reduced));
    }
    built
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
    // Compile every derived measure transitively needed by `targets`, in
    // dependency (topological) order.
    let plans = compile_plans(model, targets)?;

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

            for (mid, reduced) in build_derived(scope, &plans_arc, &input_colls) {
                let res = res.clone();
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
        });

        // Feed all input cells at time 0, advance, and run to completion.
        for (id, cells) in &input_cells {
            let session = sessions.get_mut(id).expect("a session per input measure");
            for (k, v) in cells {
                session.insert((k.clone(), v.clone()));
            }
        }
        // Closing the sessions (on return) lets `execute_directly` run the
        // dataflow to completion before it returns.
        for session in sessions.values_mut() {
            session.advance_to(1);
            session.flush();
        }
    });

    let out = results.lock().unwrap().clone();
    Ok(out)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::decode_coord;
    use crate::typed::{Dim, TypeInfo};
    use improv_core_model::{
        Coordinate, DimensionSpec, Expr, ItemId, Measure, MeasureKind, Name, Value, ValueType,
    };

    /// Two numeric inputs A (`MeasureId(100)`) and B (`MeasureId(101)`) and a
    /// derived `R = A op B` (or `B op A` when `!a_left`) at `MeasureId(102)`.
    /// Category `c` has items `10*c + 0..3`; cells are `(item ids, value)` in
    /// the order of the measure's categories.
    fn bcast_model(
        a_cats: &[u32],
        a: &[(Vec<u32>, f64)],
        b_cats: &[u32],
        b: &[(Vec<u32>, f64)],
        op: BinaryOp,
        a_left: bool,
    ) -> Model {
        let mut m = Model::new();
        for c in 1..=3u32 {
            m.add_category(CategoryId(c), format!("C{c}"));
            for i in 0..3u32 {
                m.add_item(ItemId(10 * c + i), CategoryId(c), format!("I{c}_{i}"));
            }
        }
        let cats = |cs: &[u32]| cs.iter().map(|c| CategoryId(*c)).collect::<Vec<_>>();
        let mut union: Vec<u32> = a_cats.iter().chain(b_cats).copied().collect();
        union.sort_unstable();
        union.dedup();
        let input = |id: u32, name: &str, cs: &[u32]| Measure {
            id: MeasureId(id),
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: cats(cs),
            kind: MeasureKind::Input,
            description: None,
        };
        m.add_measure(input(100, "A", a_cats));
        m.add_measure(input(101, "B", b_cats));
        let r = |id: u32| Box::new(Expr::Ref(MeasureId(id), DimensionSpec::default()));
        let (l, rr) = if a_left {
            (r(100), r(101))
        } else {
            (r(101), r(100))
        };
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("R".into()),
            value_type: ValueType::Number,
            categories: cats(&union),
            kind: MeasureKind::Derived(improv_core_model::Formula::new(Expr::BinaryOp(op, l, rr))),
            description: None,
        });
        for (id, cs, cells) in [(100, a_cats, a), (101, b_cats, b)] {
            for (items, v) in cells {
                let coord = Coordinate::from_pairs(
                    cs.iter()
                        .zip(items)
                        .map(|(c, i)| (CategoryId(*c), ItemId(*i))),
                );
                m.set_input(MeasureId(id), coord, Value::Number(*v));
            }
        }
        m
    }

    /// Evaluate `R` (`MeasureId(102)`) to a `CoordKey -> f64` map.
    fn eval_r(m: &Model) -> BTreeMap<CoordKey, f64> {
        let out = evaluate(m, &[MeasureId(102)]).expect("evaluate");
        out.get(&MeasureId(102))
            .map(|r| {
                r.iter()
                    .map(|(k, v)| (k.clone(), v.as_num().unwrap()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Price[Time,Product] and Discount[Product]: (cats, cells) pairs.
    #[allow(clippy::type_complexity)]
    fn price_discount() -> (
        (Vec<u32>, Vec<(Vec<u32>, f64)>),
        (Vec<u32>, Vec<(Vec<u32>, f64)>),
    ) {
        (
            (
                vec![1, 2],
                vec![
                    (vec![10, 20], 100.0),
                    (vec![10, 21], 50.0),
                    (vec![11, 20], 120.0),
                ],
            ),
            (vec![2], vec![(vec![20], 10.0), (vec![21], 5.0)]),
        )
    }

    /// `Price op Discount` (larger on the left) and `Discount op Price`
    /// (larger on the right), exact values for every arithmetic operator.
    #[test]
    fn broadcast_larger_left_and_larger_right() {
        let ((pc, p), (dc, d)) = price_discount();
        let at = [
            key(&[(1, 10), (2, 20)]),
            key(&[(1, 10), (2, 21)]),
            key(&[(1, 11), (2, 20)]),
        ];
        let cases: [(BinaryOp, [f64; 3], [f64; 3]); 4] = [
            (BinaryOp::Add, [110.0, 55.0, 130.0], [110.0, 55.0, 130.0]),
            (BinaryOp::Sub, [90.0, 45.0, 110.0], [-90.0, -45.0, -110.0]),
            (
                BinaryOp::Mul,
                [1000.0, 250.0, 1200.0],
                [1000.0, 250.0, 1200.0],
            ),
            (
                BinaryOp::Div,
                [10.0, 10.0, 12.0],
                [10.0 / 100.0, 5.0 / 50.0, 10.0 / 120.0],
            ),
        ];
        for (op, larger_left, larger_right) in cases {
            for (a_left, want) in [(true, larger_left), (false, larger_right)] {
                let got = eval_r(&bcast_model(&pc, &p, &dc, &d, op, a_left));
                let want: BTreeMap<CoordKey, f64> = at.iter().cloned().zip(want).collect();
                assert_eq!(got, want, "{op:?}, price on the left: {a_left}");
            }
        }
    }

    /// Run hand-built plan `plan` as derived `MeasureId(102)` over `cells`.
    fn run_plan(
        plan: PlanNode,
        cells: Vec<(MeasureId, Vec<(CoordKey, f64)>)>,
    ) -> BTreeMap<CoordKey, f64> {
        let out = Arc::new(Mutex::new(BTreeMap::new()));
        let out2 = out.clone();
        timely::execute::execute_directly(move |worker| {
            let mut sessions = Vec::new();
            worker.dataflow::<u64, _, _>(|scope| {
                let mut inputs = HashMap::new();
                for (mid, _) in &cells {
                    let mut session = InputSession::new();
                    inputs.insert(*mid, session.to_collection(scope));
                    sessions.push(session);
                }
                for (_, coll) in build_derived(scope, &[(MeasureId(102), plan)], &inputs) {
                    let out = out2.clone();
                    coll.inspect(move |((k, v), _t, d)| {
                        if *d > 0 {
                            out.lock().unwrap().insert(k.clone(), v.as_num().unwrap());
                        }
                    });
                }
            });
            for ((_, cs), session) in cells.iter().zip(&mut sessions) {
                for (k, v) in cs {
                    session.insert((k.clone(), CellValue::num(*v)));
                }
                session.advance_to(1);
                session.flush();
            }
        });
        let got = out.lock().unwrap().clone();
        got
    }

    /// Both operands carry an extra dim the other lacks:
    /// `A[T,P] * B[P,R]` lands on `[T,P,R]` in the dataflow builder, whichever
    /// side is on the left and whether or not a `Join` feeds the map. (The
    /// compiler rejects such formulas: one dim must be a subset of the other.)
    #[test]
    fn broadcast_disjoint_extra_dims() {
        let a = vec![
            (vec![10, 20], 2.0),
            (vec![11, 20], 3.0),
            (vec![10, 21], 5.0),
        ];
        let b = vec![
            (vec![20, 30], 7.0),
            (vec![20, 31], 11.0),
            (vec![21, 30], 13.0),
        ];
        let want: BTreeMap<CoordKey, f64> = [
            (key(&[(1, 10), (2, 20), (3, 30)]), 14.0),
            (key(&[(1, 10), (2, 20), (3, 31)]), 22.0),
            (key(&[(1, 11), (2, 20), (3, 30)]), 21.0),
            (key(&[(1, 11), (2, 20), (3, 31)]), 33.0),
            (key(&[(1, 10), (2, 21), (3, 30)]), 65.0),
        ]
        .into_iter()
        .collect();
        let cells = |cs: [u32; 2], v: &[(Vec<u32>, f64)]| {
            let pairs = |items: &[u32]| {
                cs.iter()
                    .copied()
                    .zip(items.iter().copied())
                    .collect::<Vec<_>>()
            };
            v.iter()
                .map(|(items, x)| (key(&pairs(items)), *x))
                .collect::<Vec<_>>()
        };
        let input = vec![
            (MeasureId(100), cells([1, 2], &a)),
            (MeasureId(101), cells([2, 3], &b)),
        ];
        let ty = |cs: &[u32]| TypeInfo {
            value_type: ValueType::Number,
            dim: Dim::of(cs.iter().map(|c| CategoryId(*c)).collect()),
        };
        let leaf = |id: u32, cs: &[u32]| PlanNode {
            kind: PlanNodeKind::InputMeasure(MeasureId(id)),
            ty: ty(cs),
        };
        let mul = |l: PlanNode, r: PlanNode| PlanNode {
            kind: PlanNodeKind::MapBinary(BinaryOp::Mul, Box::new(l), Box::new(r)),
            ty: ty(&[1, 2, 3]),
        };
        let (pa, pb) = (leaf(100, &[1, 2]), leaf(101, &[2, 3]));
        let joined = PlanNode {
            kind: PlanNodeKind::Join {
                left: Box::new(pa.clone()),
                right: Box::new(pb.clone()),
                join_keys: vec![CategoryId(2)],
            },
            ty: ty(&[1, 2, 3]),
        };
        for plan in [
            mul(pa.clone(), pb.clone()),
            mul(pb.clone(), pa),
            mul(joined, pb),
        ] {
            assert_eq!(run_plan(plan.clone(), input.clone()), want, "{plan:?}");
        }
        let m = bcast_model(&[1, 2], &a, &[2, 3], &b, BinaryOp::Mul, true);
        assert!(matches!(
            evaluate(&m, &[MeasureId(102)]),
            Err(EngineError::Compile(
                crate::compiler::CompileError::DimensionMismatch(_)
            ))
        ));
    }

    /// `session::Engine` (incremental) agrees with `evaluate` on broadcasts,
    /// both at build time and after edits to either operand.
    #[test]
    fn broadcast_session_matches_evaluate() {
        let snap_r = |snap: &crate::session::Snapshot| -> BTreeMap<CoordKey, f64> {
            snap.get(&MeasureId(102))
                .map(|r| {
                    r.iter()
                        .map(|(k, v)| (k.clone(), v.as_num().unwrap()))
                        .collect()
                })
                .unwrap_or_default()
        };
        let ((pc, p), (dc, d)) = price_discount();
        for op in [BinaryOp::Add, BinaryOp::Sub, BinaryOp::Mul, BinaryOp::Div] {
            for a_left in [true, false] {
                let mut m = bcast_model(&pc, &p, &dc, &d, op, a_left);
                let (mut engine, snap) =
                    crate::session::Engine::new(&m, &[MeasureId(102)]).expect("engine");
                assert_eq!(snap_r(&snap), eval_r(&m), "initial {op:?} {a_left}");
                assert_eq!(snap_r(&snap).len(), 3);
                // Edit the smaller operand and add a cell to the larger one.
                engine
                    .set(MeasureId(101), key(&[(2, 21)]), 4.0)
                    .expect("edit");
                let snap = engine
                    .set(MeasureId(100), key(&[(1, 11), (2, 21)]), 7.0)
                    .expect("edit");
                let c = |pairs: &[(u32, u32)]| decode_coord(&key(pairs));
                m.set_input(MeasureId(101), c(&[(2, 21)]), Value::Number(4.0));
                m.set_input(MeasureId(100), c(&[(1, 11), (2, 21)]), Value::Number(7.0));
                assert_eq!(snap_r(&snap), eval_r(&m), "edited {op:?} {a_left}");
                if (op, a_left) == (BinaryOp::Sub, true) {
                    let want: BTreeMap<CoordKey, f64> = [
                        (key(&[(1, 10), (2, 20)]), 90.0),
                        (key(&[(1, 10), (2, 21)]), 46.0),
                        (key(&[(1, 11), (2, 20)]), 110.0),
                        (key(&[(1, 11), (2, 21)]), 3.0),
                    ]
                    .into_iter()
                    .collect();
                    assert_eq!(snap_r(&snap), want);
                }
            }
        }
    }

    /// All coordinates over `cats`, two items per category (`10c`, `10c+1`).
    fn coords(cats: &[u32]) -> Vec<Vec<u32>> {
        cats.iter().fold(vec![vec![]], |acc, c| {
            acc.into_iter()
                .flat_map(|pre| {
                    (0..2).map(move |i| {
                        let mut v = pre.clone();
                        v.push(10 * c + i);
                        v
                    })
                })
                .collect()
        })
    }

    proptest::proptest! {
        /// `evaluate(A op B)` equals a naive reference: iterate the union's
        /// Cartesian coordinates, look up each broadcast operand by projecting,
        /// and apply `op` where both exist. Non-broadcastable dims must error.
        #[test]
        fn broadcast_matches_naive_reference(
            a_mask in 0u8..8,
            b_mask in 0u8..8,
            a_vals in proptest::collection::vec(proptest::option::of(1i32..=9), 8),
            b_vals in proptest::collection::vec(proptest::option::of(-9i32..=-1), 8),
            op_ix in 0usize..4,
            a_left: bool,
        ) {
            let op = [BinaryOp::Add, BinaryOp::Sub, BinaryOp::Mul, BinaryOp::Div][op_ix];
            let cats = |mask: u8| (1..=3u32).filter(|c| mask & (1 << (c - 1)) != 0).collect::<Vec<_>>();
            let (ac, bc) = (cats(a_mask), cats(b_mask));
            let cells = |cs: &[u32], vals: &[Option<i32>]| -> Vec<(Vec<u32>, f64)> {
                coords(cs).into_iter().zip(vals).filter_map(|(c, v)| v.map(|v| (c, f64::from(v)))).collect()
            };
            let (a, b) = (cells(&ac, &a_vals), cells(&bc, &b_vals));
            let m = bcast_model(&ac, &a, &bc, &b, op, a_left);
            let subset = |x: u8, y: u8| x & y == x;
            if !subset(a_mask, b_mask) && !subset(b_mask, a_mask) {
                proptest::prop_assert!(evaluate(&m, &[MeasureId(102)]).is_err());
                return Ok(());
            }
            let union = cats(a_mask | b_mask);
            let lookup = |cs: &[u32], cells: &[(Vec<u32>, f64)], u: &[u32]| {
                let want: Vec<u32> = union.iter().zip(u).filter(|(c, _)| cs.contains(c)).map(|(_, i)| *i).collect();
                cells.iter().find(|(c, _)| *c == want).map(|(_, v)| *v)
            };
            let mut reference = BTreeMap::new();
            for u in coords(&union) {
                let (Some(x), Some(y)) = (lookup(&ac, &a, &u), lookup(&bc, &b, &u)) else { continue };
                let (x, y) = if a_left { (x, y) } else { (y, x) };
                let v = match op {
                    BinaryOp::Add => x + y,
                    BinaryOp::Sub => x - y,
                    BinaryOp::Mul => x * y,
                    _ => x / y,
                };
                let k: Vec<(u32, u32)> = union.iter().copied().zip(u).collect();
                reference.insert(key(&k), v);
            }
            proptest::prop_assert_eq!(eval_r(&m), reference);
        }
    }

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

    /// Run `build_coll` for a hand-built plan in a throwaway scope.
    fn build_err(plan: PlanNode) -> String {
        timely::execute::execute_directly(move |worker| {
            worker.dataflow::<u64, _, _>(|scope| {
                match build_coll(&plan, scope, &HashMap::new(), &HashMap::new()) {
                    Ok(_) => panic!("expected a build error"),
                    Err(e) => e.to_string(),
                }
            })
        })
    }

    #[test]
    fn malformed_func_call_plans_are_build_errors() {
        use crate::typed::{Dim, TypeInfo};
        let ty = TypeInfo {
            value_type: ValueType::Number,
            dim: Dim::scalar(),
        };
        let lit = PlanNode {
            kind: PlanNodeKind::Literal(Value::Number(1.0)),
            ty: ty.clone(),
        };
        let call = |func, args| PlanNode {
            kind: PlanNodeKind::FuncCall {
                func: FuncId(func),
                args,
            },
            ty: ty.clone(),
        };
        assert!(build_err(call(99, vec![])).contains("unknown scalar func"));
        assert!(build_err(call(10, vec![lit.clone(), lit])).contains("arity 1, got 2"));
    }

    #[test]
    fn cell_ops_cover_every_operator() {
        let n = CellValue::num;
        let b = CellValue::Bool;
        assert_eq!(apply_binary(BinaryOp::Add, &n(2.0), &n(3.0)), n(5.0));
        assert_eq!(apply_binary(BinaryOp::Div, &n(1.0), &n(4.0)), n(0.25));
        assert!(apply_binary(BinaryOp::Div, &n(1.0), &n(0.0))
            .as_num()
            .unwrap()
            .is_nan());
        for (x, y) in [(true, true), (true, false), (false, true), (false, false)] {
            assert_eq!(apply_binary(BinaryOp::And, &b(x), &b(y)), b(x && y));
            assert_eq!(apply_binary(BinaryOp::Or, &b(x), &b(y)), b(x || y));
        }
        for (op, want) in [
            (BinaryOp::Lt, true),
            (BinaryOp::Le, true),
            (BinaryOp::Gt, false),
            (BinaryOp::Ge, false),
            (BinaryOp::Eq, false),
            (BinaryOp::Ne, true),
        ] {
            assert_eq!(apply_binary(op, &n(1.0), &n(2.0)), b(want), "{op:?}");
        }
        assert_eq!(apply_unary(UnaryOp::Neg, &n(2.0)), n(-2.0));
        assert_eq!(apply_unary(UnaryOp::Not, &b(true)), b(false));
        assert!(truthy(&n(1.0)) && !truthy(&n(0.0)));
        assert!(truthy(&CellValue::Text("x".into())) && !truthy(&CellValue::Text(String::new())));
        assert!(truthy(&CellValue::Date(1)) && !truthy(&CellValue::Date(0)));
        assert!(!truthy(&CellValue::Err(0)));
    }

    #[test]
    fn scalar_funcs_and_aggregates() {
        for (id, x, want) in [
            (10, -2.5, 2.5),
            (11, 2.5, 3.0),
            (12, 2.5, 2.0),
            (13, 2.1, 3.0),
            (14, 9.0, 3.0),
            (15, 4.0, -4.0),
        ] {
            assert_eq!(apply_scalar(FuncId(id), &[x]), want, "func {id}");
        }
        assert!(apply_scalar(FuncId(14), &[-1.0]).is_nan());
        assert_eq!(apply_scalar(FuncId(20), &[1.0, 2.0]), 1.0);
        assert_eq!(apply_scalar(FuncId(21), &[1.0, 2.0]), 2.0);
        assert!(apply_scalar(FuncId(99), &[1.0]).is_nan());
        let v = [4.0, 1.0, 7.0];
        assert_eq!(
            [SUM, AVG, MIN, MAX].map(|f| aggregate(f, &v)),
            [12.0, 4.0, 1.0, 7.0]
        );
        assert!(aggregate(FuncId(99), &v).is_nan());
    }

    #[test]
    fn evaluate_unary_literal_operands_and_missing_inputs() {
        let mut model = revenue_model();
        let (time, product) = (CategoryId(1), CategoryId(2));
        let refr = |id| Expr::Ref(MeasureId(id), DimensionSpec::default());
        let lit = |n: f64| Expr::Literal(Value::Number(n));
        let bin = |op, l, r| Expr::BinaryOp(op, Box::new(l), Box::new(r));
        let mk = |id: u32, cats: Vec<CategoryId>, kind| Measure {
            id: MeasureId(id),
            name: Name(format!("M{id}")),
            value_type: ValueType::Number,
            categories: cats,
            kind,
            description: None,
        };
        let derived = |f| MeasureKind::Derived(improv_core_model::Formula::new(f));
        model.add_measure(mk(
            150,
            vec![product],
            derived(Expr::UnaryOp(UnaryOp::Neg, Box::new(refr(100)))),
        ));
        model.add_measure(mk(
            151,
            vec![product],
            derived(bin(BinaryOp::Mul, lit(2.0), refr(100))),
        ));
        model.add_measure(mk(
            152,
            vec![],
            derived(bin(BinaryOp::Add, lit(2.0), lit(3.0))),
        ));
        // A ref to an input measure with no cells has no collection: skipped.
        model.add_measure(mk(160, vec![time], MeasureKind::Input));
        model.add_measure(mk(153, vec![time], derived(refr(160))));
        let ids = [150, 151, 152, 153].map(MeasureId);
        let out = evaluate(&model, &ids).unwrap();
        let a = key(&[(2, 20)]);
        assert_eq!(out[&MeasureId(150)][&a], CellValue::num(-10.0));
        assert_eq!(out[&MeasureId(151)][&a], CellValue::num(20.0));
        assert_eq!(out[&MeasureId(152)][&key(&[])], CellValue::num(5.0));
        assert!(!out.contains_key(&MeasureId(153)));
        // An unknown target is ignored here (the compiler reports bad refs).
        assert!(evaluate(&model, &[MeasureId(999)]).unwrap().is_empty());
    }
}
