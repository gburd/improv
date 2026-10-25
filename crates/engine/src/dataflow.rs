//! Build a differential-dataflow graph from compiled `PlanNode`s and run a
//! whole `Model` to a set of computed derived-measure values.
//!
//! This connects the formula compiler (`compiler`) to the DD substrate proven
//! by the spike in `lib.rs`. Encoding is the same the spike validated:
//!   * DD key   = coordinate `CoordKey` = `SmallVec<[(u32,u32); 3]>` (inline
//!     up to three categories)
//!   * DD value = `CellValue` (a number as `f64::to_bits()`, or a boolean,
//!     text, date or error)
//!   * diff `R` = `isize`; a `reduce` collapses a measure to one value per
//!     key unless its plan already gives one (`functional`).
//!
//! Scope (v1, numeric): InputMeasure, Literal, MapUnary(Neg), MapBinary
//! (arithmetic + comparison/logical encoded as 1.0/0.0), Join (dimension
//! alignment), Aggregate (SUM/AVG/MIN/MAX), and scalar FuncCall built-ins
//! (ABS/ROUND/FLOOR/CEIL/SQRT/NEG, MIN2/MAX2). Non-numeric derived values
//! (Text/Boolean stored as such) and Date values are carried in `CellValue`;
//! comparison/logical results live in the numeric lane as 1.0/0.0.

use crate::builtins::{self, err};
use crate::compiler::{compile_formula, CompileContext};
use crate::plan::{PlanNode, PlanNodeKind};
use crate::reduce::{self, two_level, Rows};
use crate::{encode_coord, CellValue, CoordKey};
use differential_dataflow::input::InputSession;
use differential_dataflow::operators::{Join as _, Reduce, Threshold};
use differential_dataflow::AsCollection;
use differential_dataflow::Collection;
use improv_core_model::{
    BinaryOp, CategoryId, Formula, FuncId, ItemId, MeasureId, MeasureKind, Model, UnaryOp,
};
#[cfg(test)]
use std::collections::BTreeMap;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use timely::dataflow::operators::ToStream;
use timely::dataflow::Scope;

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("compile error: {0}")]
    Compile(#[from] crate::compiler::CompileError),
    #[error("unsupported plan node in v1 dataflow builder: {0}")]
    Unsupported(String),
    /// Formulas that depend on each other in a loop, named in order:
    /// `A1 → B1 → A1`.
    #[error("formula cycle: {0}")]
    Cycle(String),
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
                    let empty = CoordKey::new();
                    let t = <G::Timestamp as timely::progress::Timestamp>::minimum();
                    Ok(vec![((empty, v), t, 1isize)]
                        .to_stream(scope)
                        .as_collection())
                }
                (None, None) => {
                    // The compiler's broadcast: `Join{a, b}` op `b`. One join
                    // of a and b gives both values; re-joining the Join's
                    // rows with b on the full key would arrange both again.
                    if let Some((a, keys)) = broadcast_of(left, right) {
                        let l = build_coll(a, scope, inputs, derived)?;
                        let r = build_coll(right, scope, inputs, derived)?;
                        return Ok(align(&l, &r, keys.to_vec())
                            .map(move |(k, (a, b))| (k, apply_binary(op, &a, &b))));
                    }
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
            inputs: ins,
            params,
            pred,
            group_by,
            func,
            order,
            window,
            per_item,
            into,
            flat,
        } => build_aggregate(
            scope,
            inputs,
            derived,
            AggSpec {
                ins,
                params,
                pred: pred.as_deref(),
                group_by,
                func: *func,
                order,
                window: *window,
                per_item: *per_item,
                into,
                flat: *flat,
            },
        ),

        PlanNodeKind::Items(cat, vals) => {
            let t = <G::Timestamp as timely::progress::Timestamp>::minimum();
            let rows: Vec<_> = vals
                .iter()
                .filter_map(|(item, v)| {
                    let cv = CellValue::from_model_value(v)?;
                    Some((
                        (smallvec::smallvec![(cat.0, item.0)], cv),
                        t.clone(),
                        1isize,
                    ))
                })
                .collect();
            Ok(rows.to_stream(scope).as_collection())
        }

        PlanNodeKind::Coalesce { args, grid } => {
            let colls = args
                .iter()
                .map(|a| build_coll(a, scope, inputs, derived))
                .collect::<Result<Vec<_>, _>>()?;
            let spans = |a: &PlanNode| a.ty.dim == node.ty.dim;
            // The domain: every item combination of the grid, or the cells of
            // the arguments over all the result's categories.
            let domain = match grid {
                Some(g) => {
                    let t = <G::Timestamp as timely::progress::Timestamp>::minimum();
                    grid_keys(g)
                        .into_iter()
                        .map(|k| (k, t.clone(), 1isize))
                        .collect::<Vec<_>>()
                        .to_stream(scope)
                        .as_collection()
                }
                None => {
                    let mut keys = None::<Collection<G, CoordKey, isize>>;
                    for (_, c) in args.iter().zip(&colls).filter(|(a, _)| spans(a)) {
                        let k = c.map(|(k, _)| k);
                        keys = Some(keys.map_or(k.clone(), |acc| acc.concat(&k)));
                    }
                    keys.expect("the widest argument spans the result")
                        .distinct()
                }
            };
            let domain = domain.map(|k| (k, CellValue::Bool(true)));
            // Each argument's cells on the domain, tagged with its position;
            // per coordinate the lowest position wins.
            let mut tagged = None::<Collection<G, (CoordKey, (u32, CellValue)), isize>>;
            for (i, (a, c)) in args.iter().zip(&colls).enumerate() {
                let i = i as u32;
                let t = if spans(a) {
                    c.map(move |(k, v)| (k, (i, v)))
                } else {
                    align(&domain, c, a.ty.dim.categories.clone())
                        .map(move |(k, (_, v))| (k, (i, v)))
                };
                tagged = Some(tagged.map_or(t.clone(), |acc| acc.concat(&t)));
            }
            let tagged = tagged.expect("COALESCE has arguments");
            Ok(tagged.reduce(|_k, input, output| {
                // Values arrive sorted, so the first is the lowest position.
                if let Some(((_, v), _)) = input.iter().find(|(_, m)| *m > 0) {
                    output.push((v.clone(), 1isize));
                }
            }))
        }

        PlanNodeKind::Fixpoint { .. } => Err(EngineError::Unsupported(
            "FIXPOINT is solved by the engine, as a measure's whole formula".into(),
        )),

        PlanNodeKind::Remap { input, maps } => {
            let x = build_coll(input, scope, inputs, derived)?;
            let (from, to) = remap_table(maps);
            Ok(x.flat_map(move |(k, v)| Some((remap_key(&k, &from, &to)?, v))))
        }

        PlanNodeKind::Recurrence {
            body,
            refs,
            slots,
            cat,
            order,
        } => {
            // Rows: every read measure aligned on the coordinate.
            let ids: Vec<MeasureId> = refs.iter().map(|(id, _)| *id).collect();
            let (_, p0) = &refs[0];
            let mut rows = build_coll(p0, scope, inputs, derived)?.map(|(k, v)| (k, vec![v]));
            let mut dim = p0.ty.dim.clone();
            for (_, p) in &refs[1..] {
                let c = build_coll(p, scope, inputs, derived)?;
                let keys: Vec<CategoryId> = dim
                    .categories
                    .iter()
                    .copied()
                    .filter(|c| p.ty.dim.categories.contains(c))
                    .collect();
                rows = align_with(&rows, &c, keys).map(|(k, (mut vs, v))| {
                    vs.push(v);
                    (k, vs)
                });
                dim = dim.union(&p.ty.dim);
            }
            let pos: HashMap<u32, u32> = order
                .iter()
                .enumerate()
                .map(|(i, it)| (it.0, i as u32))
                .collect();
            let c = cat.0;
            let keyed = rows.map(move |(k, vs)| {
                let item = k.iter().find(|(cc, _)| *cc == c).map_or(0, |(_, i)| *i);
                let p = pos.get(&item).copied().unwrap_or(u32::MAX / 2 + item);
                let group: CoordKey = k.into_iter().filter(|(cc, _)| *cc != c).collect();
                (group, ((p, item), vs))
            });
            let (body, slots) = (body.clone(), slots.clone());
            let scan = keyed.reduce(
                move |_g, input, output: &mut Vec<((u32, CellValue), isize)>| {
                    let mut done: HashMap<i64, CellValue> = HashMap::new();
                    for (((p, item), vs), mult) in input {
                        if *mult <= 0 {
                            continue;
                        }
                        let mut env: HashMap<MeasureId, CellValue> =
                            ids.iter().copied().zip(vs.iter().cloned()).collect();
                        for (slot, back, default) in &slots {
                            let earlier =
                                done.get(&(i64::from(*p) - back)).cloned().or_else(|| {
                                    default
                                        .as_ref()
                                        .and_then(|d| crate::scalar::eval(d, &env).ok().flatten())
                                });
                            if let Some(v) = earlier {
                                env.insert(*slot, v);
                            }
                        }
                        if let Ok(Some(v)) = crate::scalar::eval(&body, &env) {
                            done.insert(i64::from(*p), v.clone());
                            output.push(((*item, v), 1isize));
                        }
                    }
                },
            );
            Ok(scan.map(move |(g, (item, v))| {
                let mut k = g;
                k.push((c, item));
                k.sort_unstable();
                (k, v)
            }))
        }

        PlanNodeKind::Shift {
            input,
            cat,
            offset,
            order,
            default,
        } => {
            let x = build_coll(input, scope, inputs, derived)?;
            let pos: HashMap<u32, usize> =
                order.iter().enumerate().map(|(i, it)| (it.0, i)).collect();
            let (cat, offset, order) = (cat.0, *offset, order.clone());
            // Each cell moves to the item `-offset` away: the value at item i
            // is the input's value at item i + offset.
            let moved = x.flat_map(move |(k, v)| {
                let mut k2 = k.clone();
                let slot = k2.iter_mut().find(|(c, _)| *c == cat)?;
                let p = *pos.get(&slot.1)? as i64 - offset;
                let it = order.get(usize::try_from(p).ok()?)?;
                slot.1 = it.0;
                Some((k2, v))
            });
            let Some(d) = default else {
                return Ok(moved);
            };
            // Where nothing moved in, the default, at the input's own cells.
            let dflt = build_coll(d, scope, inputs, derived)?;
            let own = x.map(|(k, _)| k);
            let landed = moved.map(|(k, _)| k);
            let holes = own
                .concat(&landed.negate())
                .threshold(|_, m| if *m > 0 { 1 } else { 0 });
            let holes = holes.map(|k| (k, CellValue::Bool(true)));
            let filled =
                align(&holes, &dflt, d.ty.dim.categories.clone()).map(|(k, (_, v))| (k, v));
            Ok(moved.concat(&filled))
        }

        PlanNodeKind::Literal(v) => {
            // A standalone literal is a *scalar* measure: one value at the empty
            // coordinate, which broadcasts (via Join) into any formula that
            // references it. Build a constant single-cell collection.
            let cv = CellValue::from_model_value(v)
                .ok_or_else(|| EngineError::Unsupported("literal has no cell value".into()))?;
            let empty = CoordKey::new();
            let t = <G::Timestamp as timely::progress::Timestamp>::minimum();
            Ok(vec![((empty, cv), t, 1isize)]
                .to_stream(scope)
                .as_collection())
        }
        PlanNodeKind::FuncCall { func, args } => {
            let func = *func;
            // Literal arguments fold into the map; the others are aligned by
            // successive broadcast joins into one row of values per coordinate.
            let template: Vec<Option<CellValue>> = args.iter().map(literal_value).collect();
            let colls: Vec<(usize, &PlanNode)> = args
                .iter()
                .enumerate()
                .filter(|(i, _)| template[*i].is_none())
                .collect();
            let fill = move |vals: &[CellValue]| call_with(func, &template, vals);
            let Some(((_, first), rest)) = colls.split_first() else {
                // No collection argument: a constant at the empty coordinate.
                return Ok(constant(scope, fill(&[])));
            };
            let mut acc = build_coll(first, scope, inputs, derived)?.map(|(k, v)| (k, vec![v]));
            let mut dim = first.ty.dim.clone();
            for (_, a) in rest {
                let c = build_coll(a, scope, inputs, derived)?;
                let keys: Vec<CategoryId> = dim
                    .categories
                    .iter()
                    .copied()
                    .filter(|c| a.ty.dim.categories.contains(c))
                    .collect();
                acc = align_with(&acc, &c, keys).map(|(k, (mut vs, v))| {
                    vs.push(v);
                    (k, vs)
                });
                dim = dim.union(&a.ty.dim);
            }
            Ok(acc.map(move |(k, vs)| (k, fill(&vs))))
        }

        PlanNodeKind::If {
            test,
            then,
            otherwise,
            spanning,
        } => {
            let t = build_coll(test, scope, inputs, derived)?;
            let a = build_coll(then, scope, inputs, derived)?;
            let b = otherwise
                .as_ref()
                .map(|o| build_coll(o, scope, inputs, derived))
                .transpose()?;
            // The test over the result's categories: joined with the spanning
            // branch when the test lacks some of them.
            let dom = match spanning {
                None => t,
                Some(i) => {
                    let (node, coll) = if *i == 0 {
                        (then.as_ref(), &a)
                    } else {
                        let node = otherwise.as_deref().expect("spanning else exists");
                        (node, b.as_ref().expect("spanning else exists"))
                    };
                    align(&t, coll, shared_cats(test, node)).map(|(k, (tv, _))| (k, tv))
                }
            };
            let pick = |want: bool, dom: &Coll<G>, branch: &Coll<G>, node: &PlanNode| {
                let keys = node.ty.dim.categories.clone();
                let sel = dom.filter(move |(_, v)| *v == CellValue::Bool(want));
                // The branch's categories are a subset of the domain's, so the
                // joined coordinate is the domain's.
                align(&sel, branch, keys).map(|(k, (_, v))| (k, v))
            };
            let mut out = pick(true, &dom, &a, then);
            if let (Some(b), Some(node)) = (&b, otherwise) {
                out = out.concat(&pick(false, &dom, b, node));
            }
            Ok(out.concat(&dom.filter(|(_, v)| matches!(v, CellValue::Err(_)))))
        }
    }
}

/// A reduction row's item positions, one per reduced category (inline up
/// to four).
pub(crate) type Pos = smallvec::SmallVec<[u32; 4]>;
/// A reduction row's values: the reduced measures', then the joined
/// parameters' (inline for one, the one-input reduction).
pub(crate) type Vals<V = CellValue> = smallvec::SmallVec<[V; 1]>;

/// A reduction row as the `reduce` sees it: (item positions along the
/// reduced categories, the first reduced category's item), then the values.
/// Inline for the common shape (up to four reduced categories, one value):
/// 64 bytes and no allocation, where two `Vec`s took 56 and two more. Its
/// order is the `Vec`s' (`SmallVec` compares as a slice), so the kernels
/// see rows in the same order.
pub(crate) type AggRow<V = CellValue> = ((Pos, u32), Vals<V>);

/// The pieces of an `Aggregate` plan node, for [`build_aggregate`].
pub(crate) struct AggSpec<'p> {
    pub(crate) ins: &'p [PlanNode],
    pub(crate) params: &'p [PlanNode],
    pub(crate) pred: Option<&'p PlanNode>,
    pub(crate) group_by: &'p [CategoryId],
    pub(crate) func: FuncId,
    pub(crate) order: &'p [(CategoryId, Vec<ItemId>)],
    pub(crate) window: Option<crate::typed::Window>,
    pub(crate) per_item: bool,
    pub(crate) into: &'p [(CategoryId, Vec<ItemId>)],
    pub(crate) flat: bool,
}

impl AggSpec<'_> {
    /// Whether it reduces in two levels ([`two_level::applies`], unless
    /// marked flat).
    pub(crate) fn two_level(&self) -> bool {
        !self.flat
            && self.window.is_none()
            && !self.per_item
            && two_level::applies(self.func, self.ins.len(), self.params.len(), self.order)
    }
}

/// Each reduced category's item positions, for [`agg_key`].
pub(crate) type AggOrder = Vec<(u32, HashMap<u32, u32>)>;

/// The item positions along `order`'s categories.
pub(crate) fn agg_order(order: &[(CategoryId, Vec<ItemId>)]) -> AggOrder {
    order
        .iter()
        .map(|(c, items)| {
            let pos = items
                .iter()
                .enumerate()
                .map(|(i, it)| (it.0, i as u32))
                .collect();
            (c.0, pos)
        })
        .collect()
}

/// A reduction row at `k`, values `vs`: its group key (`k` on `gb`) and the
/// row as the reduction sees it (its item positions along `order`, its item
/// of the first reduced category, the values).
pub(crate) fn agg_key<V>(
    k: &CoordKey,
    vs: Vals<V>,
    order: &AggOrder,
    gb: &[u32],
) -> (CoordKey, AggRow<V>) {
    let pos: Pos = order
        .iter()
        .map(|(c, m)| {
            let item = k.iter().find(|(cc, _)| cc == c).map_or(0, |(_, i)| *i);
            // Unknown items (no model order) sort by id after known ones.
            m.get(&item).copied().unwrap_or(u32::MAX / 2 + item)
        })
        .collect();
    let item = order
        .first()
        .and_then(|(c, _)| k.iter().find(|(cc, _)| cc == c))
        .map_or(0, |(_, i)| *i);
    (project(k, gb), ((pos, item), vs))
}

/// A group's reduction input (sorted rows, with multiplicities) as the
/// kernels take it: the rows, the parameters (`consts`, the joined ones
/// filled from the first row), and each row's item.
pub(crate) fn agg_rows(
    input: &[(&AggRow, isize)],
    n_in: usize,
    consts: &[Option<CellValue>],
    capacity: usize,
) -> (Rows, Vec<CellValue>, Vec<u32>) {
    let mut rs = Rows {
        pos: Vec::new(),
        vals: Vec::new(),
        capacity,
    };
    let mut params: Vec<CellValue> = Vec::new();
    let mut items = Vec::new();
    for ((pk, vs), mult) in input {
        for _ in 0..(*mult).max(0) {
            rs.pos.push(pk.0.to_vec());
            rs.vals.push(vs[..n_in].to_vec());
            items.push(pk.1);
            if params.is_empty() {
                // Joined parameters follow the inputs, in order.
                let mut joined = vs[n_in..].iter();
                params = consts
                    .iter()
                    .map(|c| match c {
                        Some(v) => v.clone(),
                        None => joined.next().cloned().unwrap_or(CellValue::Err(err::NA)),
                    })
                    .collect();
            }
        }
    }
    if params.is_empty() {
        params = consts
            .iter()
            .map(|c| c.clone().unwrap_or(CellValue::Err(err::NA)))
            .collect();
    }
    (rs, params, items)
}

/// A reduction row under a `WHERE` condition `t`: kept where it is true,
/// every value the error (as `err` makes it a value) where it is an error,
/// else dropped.
pub(crate) fn where_row<V: Clone>(
    vs: Vals<V>,
    t: CellValue,
    err: impl Fn(CellValue) -> V,
) -> Option<Vals<V>> {
    match t {
        CellValue::Bool(true) => Some(vs),
        CellValue::Err(_) => Some(smallvec::smallvec![err(t); vs.len()]),
        _ => None,
    }
}

/// Every item combination of `grid`, row-major (the last category fastest).
pub(crate) fn grid_keys(grid: &[(CategoryId, Vec<ItemId>)]) -> Vec<CoordKey> {
    grid.iter().fold(vec![CoordKey::new()], |acc, (c, items)| {
        acc.iter()
            .flat_map(|pre: &CoordKey| {
                items.iter().map(move |i| {
                    let mut k = pre.clone();
                    k.push((c.0, i.0));
                    k
                })
            })
            .collect()
    })
}

/// A RELABEL's maps as the renamed categories and each renamed item's target.
pub(crate) type RemapTable = (Vec<u32>, HashMap<(u32, u32), (u32, u32)>);

/// [`RemapTable`] of `maps`.
pub(crate) fn remap_table(maps: &crate::plan::Remaps) -> RemapTable {
    let to = maps
        .iter()
        .flat_map(|(from, to, items)| items.iter().map(|(i, j)| ((from.0, i.0), (to.0, j.0))))
        .collect();
    (maps.iter().map(|(c, _, _)| c.0).collect(), to)
}

/// `k` renamed by a [`RemapTable`]; `None` where a renamed item has no target.
pub(crate) fn remap_key(
    k: &CoordKey,
    from: &[u32],
    to: &HashMap<(u32, u32), (u32, u32)>,
) -> Option<CoordKey> {
    let mut k2 = CoordKey::with_capacity(k.len());
    for &(c, i) in k {
        if from.contains(&c) {
            k2.push(*to.get(&(c, i))?);
        } else {
            k2.push((c, i));
        }
    }
    k2.sort_unstable();
    Some(k2)
}

/// A scalar call: `template`'s literals with `vals` in the other slots.
pub(crate) fn call_with(
    func: FuncId,
    template: &[Option<CellValue>],
    vals: &[CellValue],
) -> CellValue {
    let mut it = vals.iter();
    let full: Vec<CellValue> = template
        .iter()
        .map(|t| match t {
            Some(v) => v.clone(),
            None => it.next().expect("one value per collection arg").clone(),
        })
        .collect();
    builtins::eval(func, &full)
}

/// A reduction's rows: the reduced measures aligned on their coordinate
/// (and filtered by `pred`), parameters appended, re-keyed to the group,
/// each row carrying its item positions so the reduction sees item order.
fn build_aggregate<G: Scope>(
    scope: &mut G,
    inputs: &HashMap<MeasureId, Coll<G>>,
    derived: &HashMap<MeasureId, Coll<G>>,
    a: AggSpec<'_>,
) -> Result<Coll<G>, EngineError>
where
    G::Timestamp: differential_dataflow::lattice::Lattice + timely::progress::Timestamp,
{
    let (first, rest) = a
        .ins
        .split_first()
        .ok_or_else(|| EngineError::Unsupported("aggregation with no input".into()))?;
    let full: Vec<CategoryId> = first
        .ty
        .dim
        .categories
        .iter()
        .chain(a.order.iter().map(|(c, _)| c))
        .copied()
        .collect();
    let mut rows =
        build_coll(first, scope, inputs, derived)?.map(|(k, v)| (k, Vals::from_buf([v])));
    // The reduced measures join on the categories they share; a category
    // only some of them range over broadcasts (the rows gain it).
    let mut full = full;
    for n in rest {
        let c = build_coll(n, scope, inputs, derived)?;
        let mine: Vec<CategoryId> =
            n.ty.dim
                .categories
                .iter()
                .chain(a.order.iter().map(|(c, _)| c))
                .copied()
                .collect();
        let shared = mine.iter().copied().filter(|c| full.contains(c)).collect();
        full.extend(
            mine.into_iter()
                .filter(|c| !full.contains(c))
                .collect::<Vec<_>>(),
        );
        rows = align_with(&rows, &c, shared).map(|(k, (mut vs, v))| {
            vs.push(v);
            (k, vs)
        });
    }
    let n_in = a.ins.len();
    if let Some(p) = a.pred {
        let pc = build_coll(p, scope, inputs, derived)?;
        rows = align_with(&rows, &pc, p.ty.dim.categories.clone())
            .flat_map(|(k, (vs, t))| where_row(vs, t, |t| t).map(|vs| (k, vs)));
    }
    // Parameters: literals fold; the rest join per row on the categories they
    // share with the rows so far. Categories only a parameter has join in
    // (broadcast): each of its cells sees every row (a lookup).
    // ponytail: a lookup costs |table| × |parameter cells| rows; index the
    // table by key if lookups over large tables ever matter.
    let mut consts: Vec<Option<CellValue>> = Vec::new();
    let mut row_dim: Vec<CategoryId> = full.clone();
    for p in a.params {
        match literal_value(p) {
            Some(v) => consts.push(Some(v)),
            None => {
                consts.push(None);
                let pc = build_coll(p, scope, inputs, derived)?;
                let shared: Vec<CategoryId> =
                    p.ty.dim
                        .categories
                        .iter()
                        .copied()
                        .filter(|c| row_dim.contains(c))
                        .collect();
                for c in &p.ty.dim.categories {
                    if !row_dim.contains(c) {
                        row_dim.push(*c);
                    }
                }
                rows = align_with(&rows, &pc, shared).map(|(k, (mut vs, v))| {
                    vs.push(v);
                    (k, vs)
                });
            }
        }
    }
    let order = agg_order(a.order);
    let capacity: usize = a.order.iter().map(|(_, items)| items.len()).product();
    let gb: Vec<u32> = a.group_by.iter().map(|c| c.0).collect();
    // (group key, ((positions, item of the first reduced category), values))
    let keyed = rows.map(move |(k, vs)| agg_key(&k, vs, &order, &gb));
    let func = a.func;
    let to_rows = move |input: &[(&AggRow, isize)]| agg_rows(input, n_in, &consts, capacity);
    let cat0 = a.order.first().map(|(c, _)| c.0);
    if !a.into.is_empty() {
        // A list of results laid over the landing categories row-major: the
        // k-th result at the k-th cell of their product (last fastest).
        let cells = grid_keys(a.into);
        let lists = keyed.reduce(
            move |_k, input, output: &mut Vec<((CoordKey, CellValue), isize)>| {
                let (rs, params, _) = to_rows(input);
                for (cell, v) in cells.iter().zip(reduce::array(func, &rs, &params)) {
                    output.push(((cell.clone(), v), 1isize));
                }
            },
        );
        return Ok(lists.map(move |(gk, (cell, v))| {
            let mut key = gk;
            key.extend(cell);
            key.sort_unstable();
            (key, v)
        }));
    }
    if a.two_level() {
        // Each block of a group reduces to a part, the parts to the result:
        // an edit re-reduces its block and the group's parts.
        let parts = keyed
            .map(|(g, row)| ((g, two_level::block(row.0 .1)), row))
            .reduce(move |_k, input, output| {
                let (rs, _, _) = to_rows(input);
                if !rs.is_empty() {
                    output.push((two_level::part(func, &rs), 1isize));
                }
            });
        return Ok(parts
            .map(|((g, b), p)| (g, (b, p)))
            .reduce(move |_k, input, output| {
                let parts = input.iter().map(|((_, p), _)| p);
                let v = two_level::finish(func, &two_level::merge(func, parts));
                output.push((v, 1isize));
            }));
    }
    if a.window.is_none() && !a.per_item {
        return Ok(keyed.reduce(move |_k, input, output| {
            let (rs, params, _) = to_rows(input);
            if !rs.is_empty() {
                output.push((reduce::reduce(func, &rs, &params), 1isize));
            }
        }));
    }
    // One result per item of the (single) reduced category.
    let window = a.window;
    let per = keyed.reduce(
        move |_k, input, output: &mut Vec<((u32, CellValue), isize)>| {
            let (rs, params, items) = to_rows(input);
            let outs = match window {
                None => reduce::per_item(func, &rs, &params),
                Some(w) => reduce::window::windows(func, &rs, &params, w),
            };
            for (item, v) in items.into_iter().zip(outs) {
                output.push(((item, v), 1isize));
            }
        },
    );
    let cat0 = cat0.unwrap_or(0);
    Ok(per.map(move |(gk, (item, v))| {
        let mut k = gk;
        k.push((cat0, item));
        k.sort_unstable();
        (k, v)
    }))
}

/// A constant at the empty coordinate (broadcasts into any join).
fn constant<G: Scope>(scope: &mut G, v: CellValue) -> Coll<G>
where
    G::Timestamp: differential_dataflow::lattice::Lattice + timely::progress::Timestamp,
{
    let empty = CoordKey::new();
    let t = <G::Timestamp as timely::progress::Timestamp>::minimum();
    vec![((empty, v), t, 1isize)]
        .to_stream(scope)
        .as_collection()
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
    align_with(l, r, keys)
}

/// [`align`] for any left value type (n-ary calls carry a `Vec` of values).
#[allow(clippy::type_complexity)]
fn align_with<G: Scope, V>(
    l: &Collection<G, (CoordKey, V), isize>,
    r: &Coll<G>,
    keys: Vec<CategoryId>,
) -> Collection<G, (CoordKey, (V, CellValue)), isize>
where
    G::Timestamp: differential_dataflow::lattice::Lattice,
    V: differential_dataflow::ExchangeData + std::hash::Hash,
{
    let keys: Vec<u32> = keys.iter().map(|c| c.0).collect();
    let keys2 = keys.clone();
    let l = l.map(move |(k, v)| (project(&k, &keys), (k, v)));
    let r = r.map(move |(k, v)| (project(&k, &keys2), (k, v)));
    l.join(&r)
        .map(|(_jk, ((lk, lv), (rk, rv)))| (union_keys(&lk, &rk), (lv, rv)))
}

/// `left` op `right` where `left` is `Join{a, right, keys}` (the compiler's
/// `split_join`): `a` and the join keys. Each side holds one value per key,
/// so the Join's row at a coordinate meets exactly one cell of `right`
/// there, the one it was joined with: `a` joined with `right` is the same.
pub(crate) fn broadcast_of<'p>(
    left: &'p PlanNode,
    right: &PlanNode,
) -> Option<(&'p PlanNode, &'p [CategoryId])> {
    match &left.kind {
        PlanNodeKind::Join {
            left: a,
            right: b,
            join_keys,
        } if **b == *right => Some((a, join_keys)),
        _ => None,
    }
}

/// The categories both plan nodes range over: their broadcast join key.
pub(crate) fn shared_cats(a: &PlanNode, b: &PlanNode) -> Vec<CategoryId> {
    a.ty.dim
        .categories
        .iter()
        .copied()
        .filter(|c| b.ty.dim.categories.contains(c))
        .collect()
}

/// If a plan node is a literal, return its `CellValue` (for folding a constant
/// operand into a `MapBinary` map rather than materializing a collection).
pub(crate) fn literal_value(node: &PlanNode) -> Option<CellValue> {
    match &node.kind {
        PlanNodeKind::Literal(v) => CellValue::from_model_value(v),
        _ => None,
    }
}

pub(crate) fn apply_binary(op: BinaryOp, a: &CellValue, b: &CellValue) -> CellValue {
    // An error operand propagates (the left one first).
    if let CellValue::Err(_) = a {
        return a.clone();
    }
    if let CellValue::Err(_) = b {
        return b.clone();
    }
    // Numbers compare as numbers, not bit patterns: -0 = 0 (NaN = NaN is
    // FALSE, as `<=` already says). Other values compare exactly.
    let same = match (a.as_num(), b.as_num()) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    };
    match op {
        BinaryOp::Eq => return CellValue::Bool(same),
        BinaryOp::Ne => return CellValue::Bool(!same),
        BinaryOp::And => return CellValue::Bool(truthy(a) && truthy(b)),
        BinaryOp::Or => return CellValue::Bool(truthy(a) || truthy(b)),
        BinaryOp::Concat => {
            return match (builtins::text_of(a), builtins::text_of(b)) {
                (Ok(x), Ok(y)) => builtins::ok_text(x + &y).unwrap_or_else(CellValue::Err),
                (Err(c), _) | (_, Err(c)) => CellValue::Err(c),
            }
        }
        _ => {}
    }
    // Ordering compares like values: numbers, text (by code point), dates,
    // booleans (FALSE < TRUE).
    let ord = match (a, b) {
        (CellValue::Text(x), CellValue::Text(y)) => Some(x.cmp(y)),
        (CellValue::Date(x), CellValue::Date(y)) => Some(x.cmp(y)),
        (CellValue::Bool(x), CellValue::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    };
    // Dates: ± days, and the difference of two dates in days.
    const DAY_MS: f64 = 86_400_000.0;
    match (op, a, b) {
        (BinaryOp::Add | BinaryOp::Sub, CellValue::Date(d), CellValue::Num(_)) => {
            let days = b.as_num().unwrap_or(f64::NAN);
            let days = if op == BinaryOp::Sub { -days } else { days };
            return builtins::date_plus_days(*d, days);
        }
        (BinaryOp::Add, CellValue::Num(_), CellValue::Date(d)) => {
            return builtins::date_plus_days(*d, a.as_num().unwrap_or(f64::NAN));
        }
        (BinaryOp::Sub, CellValue::Date(x), CellValue::Date(y)) => {
            return CellValue::num((x - y) as f64 / DAY_MS);
        }
        _ => {}
    }
    let (Some(x), Some(y)) = (a.as_num(), b.as_num()) else {
        return match (op, ord) {
            (BinaryOp::Lt, Some(o)) => CellValue::Bool(o.is_lt()),
            (BinaryOp::Le, Some(o)) => CellValue::Bool(o.is_le()),
            (BinaryOp::Gt, Some(o)) => CellValue::Bool(o.is_gt()),
            (BinaryOp::Ge, Some(o)) => CellValue::Bool(o.is_ge()),
            _ => CellValue::Err(err::VALUE),
        };
    };
    match op {
        BinaryOp::Add => builtins::num(x + y),
        BinaryOp::Sub => builtins::num(x - y),
        BinaryOp::Mul => builtins::num(x * y),
        BinaryOp::Div if y == 0.0 => CellValue::Err(err::DIV0),
        BinaryOp::Div => builtins::num(x / y),
        BinaryOp::Pow => builtins::power(x, y).unwrap_or_else(CellValue::Err),
        BinaryOp::Lt => CellValue::Bool(x < y),
        BinaryOp::Le => CellValue::Bool(x <= y),
        BinaryOp::Gt => CellValue::Bool(x > y),
        BinaryOp::Ge => CellValue::Bool(x >= y),
        BinaryOp::Eq | BinaryOp::Ne | BinaryOp::And | BinaryOp::Or | BinaryOp::Concat => {
            unreachable!("handled above")
        }
    }
}

pub(crate) fn apply_unary(op: UnaryOp, v: &CellValue) -> CellValue {
    match (op, v) {
        (_, CellValue::Err(_)) => v.clone(),
        (UnaryOp::Neg, _) => match v.as_num() {
            Some(x) => builtins::num(-x),
            None => CellValue::Err(err::VALUE),
        },
        (UnaryOp::Not, _) => CellValue::Bool(!truthy(v)),
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

/// Project a coordinate key down to a subset of category ids (proven in
/// [`improv_verified::keys::project`]).
pub(crate) fn project(k: &CoordKey, keep: &[u32]) -> CoordKey {
    crate::inline_key(improv_verified::keys::project(k, keep))
}

/// Merge two sorted coordinate keys (right's extra dims added to left; shared
/// dims agree, they were the join key). A proven merge:
/// [`improv_verified::keys::union_keys`].
pub(crate) fn union_keys(a: &CoordKey, b: &CoordKey) -> CoordKey {
    crate::inline_key(improv_verified::keys::union_keys(a, b))
}

/// Compute the derived measures transitively needed by `targets`, in
/// dependency order (a measure comes after every derived measure it
/// references). Rejects a cycle, naming it.
pub fn derived_build_order<'m>(
    model: &'m Model,
    targets: &[MeasureId],
) -> Result<Vec<(MeasureId, &'m Formula)>, EngineError> {
    // Derived measures are the graph's nodes. A measure's references to itself
    // are a recurrence (`Bal = PREV(Bal, Time, 1, Open) + Flow`), checked by
    // the compiler, not a cycle; refs to inputs or unknown measures (which the
    // compiler reports) are no edges.
    let derived: Vec<(MeasureId, &'m Formula)> = model
        .measures
        .iter()
        .filter_map(|(id, m)| match &m.kind {
            MeasureKind::Derived(f) => Some((*id, f)),
            MeasureKind::Input => None,
        })
        .collect();
    let index: HashMap<MeasureId, usize> = derived
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (*id, i))
        .collect();
    let graph: Vec<Vec<usize>> = derived
        .iter()
        .map(|(id, f)| {
            f.referenced_measures()
                .iter()
                .filter(|d| *d != id)
                .filter_map(|d| index.get(d).copied())
                .collect()
        })
        .collect();
    let roots: Vec<usize> = targets
        .iter()
        .filter_map(|t| index.get(t).copied())
        .collect();
    // Proven: the reachable nodes, each once, after their dependencies; an
    // error exactly when a cycle is reachable.
    match improv_verified::graph::build_order(&graph, &roots) {
        Ok(order) => Ok(order.into_iter().map(|i| derived[i]).collect()),
        Err(i) => Err(EngineError::Cycle(cycle_names(model, &derived, &graph, i))),
    }
}

/// The cycle through `start` (a node `build_order` found on one) as measure
/// names, from its lowest measure id back to itself: `A1 → B1 → A1`.
fn cycle_names(
    model: &Model,
    derived: &[(MeasureId, &Formula)],
    graph: &[Vec<usize>],
    start: usize,
) -> String {
    // Breadth-first from `start` until an edge leads back to it.
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut queue = std::collections::VecDeque::from([start]);
    let mut last = start;
    'search: while let Some(u) = queue.pop_front() {
        for &v in &graph[u] {
            if v == start {
                last = u;
                break 'search;
            }
            if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(v) {
                e.insert(u);
                queue.push_back(v);
            }
        }
    }
    let mut ids = vec![derived[last].0];
    let mut at = last;
    while let Some(&p) = parent.get(&at) {
        ids.push(derived[p].0);
        at = p;
    }
    ids.reverse();
    let low = (0..ids.len()).min_by_key(|&k| ids[k].0).unwrap_or(0);
    ids.rotate_left(low);
    ids.push(ids[0]);
    ids.iter()
        .map(|id| model.measures[id].name.to_string())
        .collect::<Vec<_>>()
        .join(" → ")
}

/// Compile every derived measure `targets` transitively needs, in build order.
pub(crate) fn compile_plans(
    model: &Model,
    targets: &[MeasureId],
) -> Result<Vec<(MeasureId, PlanNode)>, EngineError> {
    let ctx = CompileContext::for_model(model);
    derived_build_order(model, targets)?
        .into_iter()
        .map(|(m, f)| Ok((m, compile_formula(&ctx, m, f)?)))
        .collect()
}

/// Mark the two-level reductions of `plans` whose groups are sparse as
/// flat: where the measures they read have at most [`two_level::FLAT_MAX`]
/// cells per group on average, blocks would hold a row or two each, and
/// their parts' two arrangements cost more per row than they save (in
/// PageRank's `SUM(W OVER Dst)`, 160 B per edge). Exact sums, the counts
/// and MIN/MAX give the same bits either way; PRODUCT and SUMSQ are never
/// marked. A guess from the cells at build time: it only moves cost.
pub(crate) fn mark_sparse(model: &Model, plans: &mut [(MeasureId, PlanNode)]) {
    let mut cells: HashMap<MeasureId, usize> = HashMap::new();
    for (m, _) in model.inputs.keys() {
        *cells.entry(*m).or_default() += 1;
    }
    let items = |c: &CategoryId| model.categories.get(c).map_or(1, |c| c.items.len().max(1));
    fn walk(p: &mut PlanNode, f: &mut dyn FnMut(&mut PlanNode)) {
        f(p);
        match &mut p.kind {
            PlanNodeKind::MapUnary(_, a) | PlanNodeKind::Remap { input: a, .. } => walk(a, f),
            PlanNodeKind::MapBinary(_, a, b)
            | PlanNodeKind::Join {
                left: a, right: b, ..
            } => {
                walk(a, f);
                walk(b, f);
            }
            PlanNodeKind::If {
                test,
                then,
                otherwise,
                ..
            } => {
                walk(test, f);
                walk(then, f);
                if let Some(o) = otherwise {
                    walk(o, f);
                }
            }
            PlanNodeKind::FuncCall { args, .. } | PlanNodeKind::Coalesce { args, .. } => {
                args.iter_mut().for_each(|a| walk(a, f));
            }
            PlanNodeKind::Aggregate {
                inputs,
                params,
                pred,
                ..
            } => {
                inputs
                    .iter_mut()
                    .chain(params.iter_mut())
                    .for_each(|a| walk(a, f));
                if let Some(x) = pred {
                    walk(x, f);
                }
            }
            _ => {}
        }
    }
    // A derived measure's cells: at most its inputs' (an estimate).
    let mut est: HashMap<MeasureId, usize> = cells.clone();
    for (mid, plan) in plans.iter_mut() {
        let mut reads = HashSet::new();
        crate::hosteval::reads(plan, &mut reads);
        let n = reads
            .iter()
            .filter_map(|m| est.get(m))
            .max()
            .copied()
            .unwrap_or(0);
        walk(plan, &mut |p| {
            let PlanNodeKind::Aggregate {
                inputs,
                params,
                group_by,
                func,
                order,
                window: None,
                per_item: false,
                into,
                flat,
                ..
            } = &mut p.kind
            else {
                return;
            };
            if !into.is_empty() || matches!(func.0, 5 | 6) {
                return;
            }
            if !two_level::applies(*func, inputs.len(), params.len(), order) {
                return;
            }
            let mut read = HashSet::new();
            inputs
                .iter()
                .for_each(|i| crate::hosteval::reads(i, &mut read));
            let rows = read
                .iter()
                .filter_map(|m| est.get(m))
                .max()
                .copied()
                .unwrap_or(0);
            let groups: usize = group_by.iter().map(&items).product();
            *flat = rows <= groups.saturating_mul(two_level::FLAT_MAX);
        });
        est.insert(*mid, n);
    }
}

/// Whether `p`'s collection has one value per key, each once, given that
/// the measures in `f` do: a map or a join of such measures, or a
/// single-result reduction. Its collection needs no collapse. (A cell's
/// coordinate names each of its measure's categories, so a join row's
/// coordinate determines the two cells it joined.)
pub(crate) fn functional(p: &PlanNode, f: &HashSet<MeasureId>) -> bool {
    match &p.kind {
        PlanNodeKind::InputMeasure(m) => f.contains(m),
        PlanNodeKind::MapUnary(_, a) => functional(a, f),
        PlanNodeKind::MapBinary(_, a, b) => match (literal_value(a), literal_value(b)) {
            (Some(_), None) => functional(b, f),
            (None, Some(_)) => functional(a, f),
            (None, None) => functional(a, f) && functional(b, f),
            (Some(_), Some(_)) => false,
        },
        // A join's row is at the union of its sides' coordinates, which
        // projects back onto each side's one cell: one value per key.
        PlanNodeKind::Join { left, right, .. } => functional(left, f) && functional(right, f),
        // A single-result reduction pushes one value per group.
        PlanNodeKind::Aggregate {
            window: None,
            per_item: false,
            into,
            ..
        } => into.is_empty(),
        _ => false,
    }
}

/// Build each plan's collection, collapsed to one value per key, in order (so
/// a later plan can reference an earlier one). A plan that fails to build, such
/// as a ref to an input measure with no collection, is left out. Input
/// collections hold one value per key, so a [`functional`] plan's needs no
/// collapse: skipping it saves two arrangements per measure (the reduce's
/// input and output: at 5M cells of `Price * Quantity`, 1.2 GB), and a chain
/// of such measures settles in one scheduling step rather than one per link.
/// Its deltas within a round are then in no set order (a join's `-old` may
/// follow its `+new`); the live engine consolidates them per round.
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
    let mut flat: HashSet<MeasureId> = inputs.keys().copied().collect();
    for (mid, plan) in plans {
        let Ok(coll) = build_coll(plan, scope, inputs, &derived) else {
            continue;
        };
        if functional(plan, &flat) {
            flat.insert(*mid);
            derived.insert(*mid, coll.clone());
            built.push((*mid, coll));
            continue;
        }
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
    // On a thread with a known stack: the plan builders recurse on the
    // formula tree, and a caller's main thread may be 1 MiB (Windows).
    std::thread::scope(|s| {
        std::thread::Builder::new()
            .name("improv-evaluate".into())
            .stack_size(ENGINE_STACK)
            .spawn_scoped(s, || evaluate_here(model, targets))
            .expect("spawn the evaluate thread")
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p))
    })
}

/// The stack of every engine thread (`evaluate`'s and the live engine's
/// worker). The formula depth limit (40 for a new formula, 256 for one
/// stored as text) is sized against it.
pub const ENGINE_STACK: usize = 8 << 20;

fn evaluate_here(
    model: &Model,
    targets: &[MeasureId],
) -> Result<HashMap<MeasureId, HashMap<CoordKey, CellValue>>, EngineError> {
    // Compile every derived measure transitively needed by `targets`, in
    // dependency (topological) order.
    let mut plans = compile_plans(model, targets)?;
    mark_sparse(model, &mut plans);
    // A FIXPOINT is solved host-side, round by round, by the live engine.
    // (It feeds only the inputs of their measure's type, as edits do.)
    if plans
        .iter()
        .any(|(_, p)| matches!(p.kind, PlanNodeKind::Fixpoint { .. }))
    {
        let (_engine, snap) = crate::session::Engine::new(model, targets)?;
        return Ok(snap.iter().map(|(m, v)| (*m, (**v).clone())).collect());
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

    // Rows, not maps: a map grown while the dataflow's arrangements are
    // alive peaks at both plus its old and new table (at 5M cells, 1.1 GB);
    // built once the dataflow is gone, at its final size, it costs the rows.
    #[allow(clippy::type_complexity)]
    let results: Arc<Mutex<HashMap<MeasureId, Vec<(CoordKey, CellValue)>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let results2 = results.clone();

    // A session per input measure, with or without cells: an empty measure
    // is an empty collection (a lazy IF may never need it), not a missing one.
    let input_ids: Vec<MeasureId> = model
        .measures
        .values()
        .filter(|m| matches!(m.kind, MeasureKind::Input))
        .map(|m| m.id)
        .collect();
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
                            .push((k.clone(), val.clone()));
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

    let rows = std::mem::take(&mut *results.lock().unwrap());
    // A key seen twice keeps its last value, as a map's inserts did.
    Ok(rows
        .into_iter()
        .map(|(m, r)| (m, r.into_iter().collect()))
        .collect())
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
    /// side is on the left and whether or not a `Join` feeds the map; the
    /// compiler accepts it (they share P), and refuses disjoint operands.
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
        // The compiler accepts the shared-category join too (a matrix
        // product's first step) and agrees with the hand-built plans.
        let m = bcast_model(&[1, 2], &a, &[2, 3], &b, BinaryOp::Mul, true);
        let got: BTreeMap<CoordKey, f64> = evaluate(&m, &[MeasureId(102)]).unwrap()
            [&MeasureId(102)]
            .iter()
            .map(|(k, v)| (k.clone(), v.as_num().unwrap()))
            .collect();
        assert_eq!(got, want);
        // Disjoint categories are still refused.
        let m = bcast_model(
            &[1],
            &[(vec![10], 1.0)],
            &[3],
            &[(vec![30], 2.0)],
            BinaryOp::Mul,
            true,
        );
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
            // Refused only when neither is a subset and they share nothing.
            let subset = |x: u8, y: u8| x & y == x;
            if !subset(a_mask, b_mask) && !subset(b_mask, a_mask) && a_mask & b_mask == 0 {
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

    /// Add derived `name = text` over `cats` to `m` as measure `id`.
    fn define(m: &mut Model, id: u32, cats: Vec<CategoryId>, text: &str) {
        let f = improv_core_model::parse_formula(m, text).expect("parses");
        let vt = crate::compiler::infer(
            &CompileContext::for_model(m),
            MeasureId(id),
            &f.formula.expr,
        )
        .expect("compiles")
        .ty
        .value_type;
        m.add_measure(Measure {
            id: MeasureId(id),
            name: f.target,
            value_type: vt,
            categories: cats,
            kind: MeasureKind::Derived(f.formula),
            description: None,
        });
    }

    /// The customer's case (P1-2): Person has four items, Authored a cell
    /// for two, Vol for all four. COALESCE and FILL give all four cells;
    /// IFNA and an IF on ISNUMBER keep Authored's two.
    fn people() -> Model {
        let mut m = Model::new();
        let (person, year) = (CategoryId(1), CategoryId(2));
        m.add_category(person, "Person");
        m.add_category(year, "Year");
        for i in 1..=4 {
            m.add_item(ItemId(10 + i), person, format!("p{i}"));
        }
        m.add_item(ItemId(21), year, "2025");
        m.add_item(ItemId(22), year, "2026");
        for (id, name, cats) in [
            (100, "Authored", vec![person]),
            (101, "Vol", vec![person]),
            (102, "Mail", vec![person, year]),
        ] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: cats,
                kind: MeasureKind::Input,
                description: None,
            });
        }
        let at = |p: u32| Coordinate::from_pairs([(person, ItemId(10 + p))]);
        m.set_input(MeasureId(100), at(1), Value::Number(300.0));
        m.set_input(MeasureId(100), at(2), Value::Number(2.0));
        for p in 1..=4 {
            m.set_input(MeasureId(101), at(p), Value::Number(f64::from(p) * 10.0));
        }
        let py =
            |p: u32, y: u32| Coordinate::from_pairs([(person, ItemId(10 + p)), (year, ItemId(y))]);
        m.set_input(MeasureId(102), py(1, 21), Value::Number(5.0));
        m.set_input(MeasureId(102), py(3, 22), Value::Number(7.0));
        m
    }

    #[test]
    fn coalesce_and_fill_give_every_person_a_cell() {
        let (person, year) = (CategoryId(1), CategoryId(2));
        let mut m = people();
        define(&mut m, 200, vec![person], "C = COALESCE(Authored, Vol*0)");
        define(&mut m, 201, vec![person], "F = FILL(Authored, 0)");
        define(&mut m, 202, vec![person], "N = IFNA(Authored, 0)");
        define(
            &mut m,
            203,
            vec![person],
            "Merit = 0.3 * Vol + 0.25 * FILL(Authored, 0)",
        );
        // Three arguments, the first with no cell anywhere but p3's Vol.
        define(
            &mut m,
            204,
            vec![person],
            "D = COALESCE(Authored, IF(Vol > 25, Vol), -1)",
        );
        // A per-Person default broadcast under Person × Year: Mail's cells
        // only (Vol adds no Year).
        define(&mut m, 205, vec![person, year], "Y = COALESCE(Mail, Vol)");
        define(&mut m, 206, vec![person, year], "G = FILL(Mail, Vol)");
        define(&mut m, 207, vec![person, year], "Z = FILL(Mail, 0)");
        let out = evaluate(&m, &(200..=207).map(MeasureId).collect::<Vec<_>>()).unwrap();
        let col = |id: u32| {
            (1..=4)
                .map(|p| {
                    out[&MeasureId(id)]
                        .get(&key(&[(1, 10 + p)]))
                        .and_then(CellValue::as_num)
                })
                .collect::<Vec<_>>()
        };
        let all = |v: &[f64]| v.iter().map(|x| Some(*x)).collect::<Vec<_>>();
        assert_eq!(col(200), all(&[300.0, 2.0, 0.0, 0.0]));
        assert_eq!(col(201), all(&[300.0, 2.0, 0.0, 0.0]));
        assert_eq!(col(202), [Some(300.0), Some(2.0), None, None]);
        assert_eq!(col(203), all(&[78.0, 6.5, 9.0, 12.0]));
        assert_eq!(col(204), all(&[300.0, 2.0, 30.0, 40.0]));
        let py = |id: u32, p: u32, y: u32| {
            out[&MeasureId(id)]
                .get(&key(&[(1, 10 + p), (2, y)]))
                .and_then(CellValue::as_num)
        };
        assert_eq!(out[&MeasureId(205)].len(), 2);
        assert_eq!(py(205, 1, 21), Some(5.0));
        assert_eq!(out[&MeasureId(206)].len(), 8);
        assert_eq!(
            (py(206, 1, 21), py(206, 1, 22), py(206, 4, 21)),
            (Some(5.0), Some(10.0), Some(40.0))
        );
        assert_eq!(out[&MeasureId(207)].len(), 8);
        assert_eq!((py(207, 3, 22), py(207, 2, 21)), (Some(7.0), Some(0.0)));
    }

    #[test]
    fn coalesce_and_fill_type_and_dimension_rules() {
        let m = people();
        let ctx = CompileContext::for_model(&m);
        let ty = |text: &str| {
            let f = improv_core_model::parse_formula(&m, text).expect("parses");
            crate::compiler::infer(&ctx, MeasureId(300), &f.formula.expr).map(|t| t.ty)
        };
        // Error-only arguments take the others' type.
        assert_eq!(
            ty("X = COALESCE(Authored, NA())").unwrap().value_type,
            ValueType::Number
        );
        assert!(ty("X = COALESCE(Authored, \"none\")").is_err());
        // FILL's default must not range beyond x.
        assert!(ty("X = FILL(Authored, Mail)").is_err());
        // Categories must nest.
        let e = ty("X = COALESCE(SUM(Mail OVER Person), Authored)");
        assert!(
            matches!(e, Err(crate::compiler::CompileError::DimensionMismatch(_))),
            "{e:?}"
        );
        // FILL needs the model's categories.
        let f = improv_core_model::parse_formula(&m, "X = FILL(Authored, 0)").unwrap();
        let bare = CompileContext::new(&m.measures);
        assert!(crate::compiler::infer(&bare, MeasureId(300), &f.formula.expr).is_err());
        // Scalar: the first argument with a value.
        let env: HashMap<MeasureId, CellValue> = [(MeasureId(101), CellValue::num(4.0))].into();
        let c = improv_core_model::parse_formula(&m, "X = COALESCE(Authored, Vol, 1)").unwrap();
        assert_eq!(
            crate::scalar::eval(&c.formula.expr, &env),
            Ok(Some(CellValue::num(4.0)))
        );
    }

    /// The edit path: a cell appearing or going away in an earlier argument
    /// moves the winner, without a rebuild.
    #[test]
    fn coalesce_follows_edits() {
        let person = CategoryId(1);
        let mut m = people();
        define(&mut m, 200, vec![person], "F = FILL(Authored, 0)");
        let (mut eng, snap) = crate::session::Engine::new(&m, &[MeasureId(200)]).unwrap();
        let k3 = key(&[(1, 13)]);
        assert_eq!(snap[&MeasureId(200)].get(&k3), Some(&CellValue::num(0.0)));
        let snap = eng.set(MeasureId(100), k3.clone(), 9.0).unwrap();
        assert_eq!(snap[&MeasureId(200)].get(&k3), Some(&CellValue::num(9.0)));
        let snap = eng.clear(MeasureId(100), key(&[(1, 11)])).unwrap();
        assert_eq!(
            snap[&MeasureId(200)].get(&key(&[(1, 11)])),
            Some(&CellValue::num(0.0))
        );
        assert_eq!(snap[&MeasureId(200)].len(), 4);
        // A new Person item gets its filled cell. Items are fixed per engine
        // build, so this takes a new Engine (a structural rebuild).
        m.add_item(ItemId(15), person, "p5");
        let (_, snap) = crate::session::Engine::new(&m, &[MeasureId(200)]).unwrap();
        assert_eq!(
            snap[&MeasureId(200)].get(&key(&[(1, 15)])),
            Some(&CellValue::num(0.0))
        );
        assert_eq!(snap[&MeasureId(200)].len(), 5);
    }

    /// COUNTUNIQUE and RANK.DENSE live (P1-4, P3-1): deleting one of two
    /// rows that share a key leaves the distinct count as it was, deleting
    /// the other lowers it, and the dense ranks follow; the live engine and
    /// a fresh evaluation agree after each edit.
    #[test]
    fn countunique_and_dense_rank_follow_deletes() {
        let person = CategoryId(1);
        let mut m = people();
        // Vol: 10, 20, 30, 40; make p2 and p3 share the key 20.
        let at = |p: u32| Coordinate::from_pairs([(person, ItemId(10 + p))]);
        m.set_input(MeasureId(101), at(3), Value::Number(20.0));
        define(&mut m, 200, vec![], "U = COUNTUNIQUE(Vol OVER Person)");
        define(&mut m, 201, vec![person], "D = RANK.DENSE(Vol OVER Person)");
        let ids = [MeasureId(200), MeasureId(201)];
        let (mut eng, snap) = crate::session::Engine::new(&m, &ids).unwrap();
        let u = |s: &crate::session::Snapshot| s[&MeasureId(200)][&CoordKey::new()].clone();
        let d = |s: &crate::session::Snapshot| {
            (1..=4)
                .map(|p| s[&MeasureId(201)].get(&key(&[(1, 10 + p)])).cloned())
                .collect::<Vec<_>>()
        };
        let n = |x: f64| Some(CellValue::num(x));
        assert_eq!(u(&snap), CellValue::num(3.0));
        assert_eq!(d(&snap), [n(3.0), n(2.0), n(2.0), n(1.0)]);
        // Delete p2: 20 is still there (p3).
        let snap = eng.clear(MeasureId(101), key(&[(1, 12)])).unwrap();
        assert_eq!(u(&snap), CellValue::num(3.0));
        assert_eq!(d(&snap), [n(3.0), None, n(2.0), n(1.0)]);
        m.inputs.remove(&(MeasureId(101), at(2)));
        let fresh = evaluate(&m, &ids).unwrap();
        assert_eq!(
            fresh[&MeasureId(200)][&CoordKey::new()],
            CellValue::num(3.0)
        );
        // Delete p3: now 20 is gone.
        let snap = eng.clear(MeasureId(101), key(&[(1, 13)])).unwrap();
        assert_eq!(u(&snap), CellValue::num(2.0));
        assert_eq!(d(&snap), [n(2.0), None, None, n(1.0)]);
        // Insert a duplicate of 40: unchanged count, a shared rank.
        let snap = eng.set(MeasureId(101), key(&[(1, 12)]), 40.0).unwrap();
        assert_eq!(u(&snap), CellValue::num(2.0));
        assert_eq!(d(&snap), [n(2.0), n(1.0), None, n(1.0)]);

        // Random batches of sets and clears (values 0..4, so keys repeat):
        // the count is count(DISTINCT) of the cells after every batch.
        let mut cells: BTreeMap<u32, u64> = [(1, 10), (4, 40), (2, 40)].into();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rand = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for _ in 0..60 {
            let mut batch = Vec::new();
            for _ in 0..1 + rand(3) {
                let p = 1 + rand(4) as u32;
                let value = (rand(3) > 0).then(|| rand(4));
                match value {
                    Some(v) => cells.insert(p, v),
                    None => cells.remove(&p),
                };
                batch.push(crate::session::Edit {
                    measure: MeasureId(101),
                    coord: key(&[(1, 10 + p)]),
                    value: value.map(|v| CellValue::num(v as f64)),
                });
            }
            let snap = eng.apply(batch).unwrap();
            let distinct: std::collections::BTreeSet<_> = cells.values().collect();
            let want = (!cells.is_empty()).then(|| CellValue::num(distinct.len() as f64));
            assert_eq!(snap[&MeasureId(200)].get(&CoordKey::new()).cloned(), want);
        }
    }

    /// Two roles of Person, Src and Dst (items a, b, c; Dst also has z, no
    /// Src namesake), and Iter.
    fn roles() -> Model {
        let mut m = Model::new();
        let (src, dst, iter) = (CategoryId(1), CategoryId(2), CategoryId(3));
        m.add_category(src, "Src");
        m.add_category(dst, "Dst");
        m.add_category(iter, "Iter");
        for (i, n) in ["a", "b", "c"].iter().enumerate() {
            m.add_item(ItemId(10 + i as u32), src, *n);
        }
        // Dst lists its items in another order: matching is by name.
        for (id, n) in [(23, "z"), (22, "c"), (20, "a"), (21, "b")] {
            m.add_item(ItemId(id), dst, n);
        }
        m.add_item(ItemId(30), iter, "1");
        m.add_item(ItemId(31), iter, "2");
        for (id, name, cats) in [(100, "R", vec![dst, iter]), (101, "W", vec![src, dst])] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: cats,
                kind: MeasureKind::Input,
                description: None,
            });
        }
        let c2 = |a: (CategoryId, u32), b: (CategoryId, u32)| {
            Coordinate::from_pairs([(a.0, ItemId(a.1)), (b.0, ItemId(b.1))])
        };
        for (d, it, v) in [(20, 30, 1.0), (21, 30, 2.0), (23, 30, 9.0), (22, 31, 3.0)] {
            m.set_input(MeasureId(100), c2((dst, d), (iter, it)), Value::Number(v));
        }
        // a→b and b→a reciprocal, a→c one way, c→a with weight 0.
        for (s, d, v) in [(10, 21, 4.0), (11, 20, 1.0), (10, 22, 2.0), (12, 20, 0.0)] {
            m.set_input(MeasureId(101), c2((src, s), (dst, d)), Value::Number(v));
        }
        m
    }

    #[test]
    fn relabel_and_swap_map_items_by_name() {
        let (src, dst, iter) = (CategoryId(1), CategoryId(2), CategoryId(3));
        let mut m = roles();
        define(
            &mut m,
            200,
            vec![src, iter],
            "Rs = RELABEL(R[Dst, Iter], Dst, Src)",
        );
        define(
            &mut m,
            201,
            vec![src, dst],
            "Recip = W > 0 AND SWAP(W, Src, Dst) > 0",
        );
        define(&mut m, 202, vec![src, dst], "Back = SWAP(W, Dst, Src)");
        define(&mut m, 203, vec![iter], "Tot = SUM(Rs OVER Src)");
        let out = evaluate(&m, &(200..=203).map(MeasureId).collect::<Vec<_>>()).unwrap();
        let rs: BTreeMap<_, _> = out[&MeasureId(200)].iter().collect();
        // Same values, over Src; z (no Src namesake) is dropped.
        assert_eq!(
            rs,
            BTreeMap::from([
                (&key(&[(1, 10), (3, 30)]), &CellValue::num(1.0)),
                (&key(&[(1, 11), (3, 30)]), &CellValue::num(2.0)),
                (&key(&[(1, 12), (3, 31)]), &CellValue::num(3.0)),
            ])
        );
        let at =
            |id: u32, s: u32, d: u32| out[&MeasureId(id)].get(&key(&[(1, s), (2, d)])).cloned();
        let t = CellValue::Bool(true);
        let f = CellValue::Bool(false);
        // a→b with b→a: reciprocal. a→c has no c→a > 0; c→a has no a→c cell.
        assert_eq!(at(201, 10, 21), Some(t.clone()));
        assert_eq!(at(201, 11, 20), Some(t));
        assert_eq!(at(201, 10, 22), Some(f));
        assert_eq!(at(201, 12, 20), Some(CellValue::Bool(false)));
        assert_eq!(out[&MeasureId(201)].len(), 4);
        // SWAP is symmetric in its categories.
        assert_eq!(at(202, 11, 20), Some(CellValue::num(4.0)));
        assert_eq!(at(202, 12, 20), Some(CellValue::num(2.0)));
        assert_eq!(
            out[&MeasureId(203)].get(&key(&[(3, 30)])),
            Some(&CellValue::num(3.0))
        );
        // The rules.
        let ctx = CompileContext::for_model(&m);
        let ty = |text: &str| {
            let f = improv_core_model::parse_formula(&m, text).expect("parses");
            crate::compiler::infer(&ctx, MeasureId(300), &f.formula.expr)
        };
        assert_eq!(
            ty("X = RELABEL(R, Dst, Src)").unwrap().ty.dim,
            Dim::of(vec![src, iter])
        );
        assert_eq!(
            ty("X = SWAP(W, Src, Dst)").unwrap().ty.dim,
            Dim::of(vec![src, dst])
        );
        for (bad, why) in [
            ("X = RELABEL(R, Src, Dst)", "does not range over Src"),
            ("X = RELABEL(W, Src, Dst)", "already ranges over Dst"),
            ("X = SWAP(R, Dst, Src)", "does not range over Src"),
            ("X = SWAP(W, Src, Src)", "both categories"),
        ] {
            let e = ty(bad).unwrap_err().to_string();
            assert!(e.contains(why), "{bad}: {e}");
        }
        let f = improv_core_model::parse_formula(&m, "X = RELABEL(R, Dst, Src)").unwrap();
        let bare = CompileContext::new(&m.measures);
        assert!(crate::compiler::infer(&bare, MeasureId(300), &f.formula.expr).is_err());
    }

    /// Measures reduced together need only reduce the same categories; the
    /// rest broadcast: SUMPRODUCT(Rs, W OVER Src) with Rs over Src × Iter and
    /// W over Src × Dst is Σ_src Rs·W per Dst × Iter (a matrix-vector
    /// product, PageRank's step).
    #[test]
    fn reduced_measures_broadcast_their_other_categories() {
        let (src, dst, iter) = (CategoryId(1), CategoryId(2), CategoryId(3));
        let mut m = roles();
        define(
            &mut m,
            200,
            vec![src, iter],
            "Rs = RELABEL(R[Dst, Iter], Dst, Src)",
        );
        define(
            &mut m,
            201,
            vec![dst, iter],
            "P = SUMPRODUCT(Rs, W OVER Src)",
        );
        let out = evaluate(&m, &[MeasureId(201)]).unwrap();
        let p: BTreeMap<_, _> = out[&MeasureId(201)].iter().collect();
        let at = |d: u32, i: u32, v: f64| (key(&[(2, d), (3, i)]), CellValue::num(v));
        let want = [
            at(21, 30, 4.0),
            at(20, 30, 2.0),
            at(22, 30, 2.0),
            at(20, 31, 0.0),
        ];
        assert_eq!(p, want.iter().map(|(k, v)| (k, v)).collect());
        // Each still reduces the OVER categories it has; with none reduced,
        // the measures must agree (no silent cross product).
        let ctx = CompileContext::for_model(&m);
        let err = |text: &str| {
            let f = improv_core_model::parse_formula(&m, text).expect("parses");
            crate::compiler::infer(&ctx, MeasureId(300), &f.formula.expr)
                .unwrap_err()
                .to_string()
        };
        assert!(err("X = SUMPRODUCT(Rs, W OVER Iter)").contains("reduce the same categories"));
        // The customer's lookup: X over Dst x Iter, the key over Dst alone.
        assert_eq!(
            crate::compiler::infer(
                &ctx,
                MeasureId(300),
                &improv_core_model::parse_formula(&m, "X = SUMPRODUCT(R, W OVER Dst)")
                    .unwrap()
                    .formula
                    .expr
            )
            .unwrap()
            .ty
            .dim,
            Dim::of(vec![src, iter])
        );
        // XLOOKUP's return measure may range beyond its key: R over Dst × Iter
        // looked up by name along Dst, per Src.
        define(&mut m, 202, vec![src], "SrcName = @ITEM(Src)");
        define(&mut m, 203, vec![dst], "DstName = @ITEM(Dst)");
        define(
            &mut m,
            204,
            vec![src, iter],
            "L = XLOOKUP(SrcName, DstName OVER Dst, R)",
        );
        let out = evaluate(&m, &[MeasureId(204)]).unwrap();
        assert_eq!(
            out[&MeasureId(204)].get(&key(&[(1, 11), (3, 30)])),
            Some(&CellValue::num(2.0))
        );
        let _ = dst;
    }

    /// IF over dimensions: the test over Product, the branches over
    /// Time×Product. The then-branch's coordinates extend the test, an
    /// unselected branch's missing cell or error never matters, and
    /// IFERROR/CLEARZERO follow.
    #[test]
    fn if_family_over_dimensions() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        // Quantity for 2026/WidgetB is missing: no Revenue cell there.
        m.inputs.remove(&(
            MeasureId(101),
            Coordinate::from_pairs([(time, ItemId(11)), (product, ItemId(21))]),
        ));
        define(
            &mut m,
            200,
            vec![time, product],
            "Pick = IF(Price > 15, Revenue, 0)",
        );
        define(
            &mut m,
            201,
            vec![time, product],
            "Ratio = Revenue / (Price - 10)",
        );
        define(
            &mut m,
            202,
            vec![time, product],
            "Safe = IFERROR(Ratio, -1)",
        );
        define(
            &mut m,
            203,
            vec![time, product],
            "Lazy = IF(Price > 15, Ratio, 0)",
        );
        define(
            &mut m,
            204,
            vec![product],
            "Label = IF(Price > 15, \"dear\", \"cheap\")",
        );
        define(&mut m, 205, vec![time, product], "Kept = CLEARERROR(Ratio)");
        let out = evaluate(&m, &(200..=205).map(MeasureId).collect::<Vec<_>>()).unwrap();
        let at =
            |id: u32, t: u32, p: u32| out[&MeasureId(id)].get(&key(&[(1, t), (2, p)])).cloned();
        let n = CellValue::num;
        // WidgetA (price 10): else-branch 0 wherever Revenue exists.
        assert_eq!(at(200, 10, 20), Some(n(0.0)));
        assert_eq!(at(200, 11, 20), Some(n(0.0)));
        // WidgetB (price 20): Revenue, and no cell where Revenue is missing.
        assert_eq!(at(200, 10, 21), Some(n(1000.0)));
        assert_eq!(at(200, 11, 21), None);
        assert_eq!(out[&MeasureId(200)].len(), 3);
        // Ratio divides by zero for WidgetA; IFERROR replaces it.
        assert_eq!(at(201, 10, 20), Some(CellValue::Err(err::DIV0)));
        assert_eq!(at(202, 10, 20), Some(n(-1.0)));
        assert_eq!(at(202, 10, 21), Some(n(100.0)));
        // The error branch is never selected for WidgetA, so it never shows.
        assert_eq!(at(203, 10, 20), Some(n(0.0)));
        assert_eq!(at(203, 10, 21), Some(n(100.0)));
        // A text IF over Product only.
        let label = &out[&MeasureId(204)];
        assert_eq!(
            label.get(&key(&[(2, 20)])),
            Some(&CellValue::Text("cheap".into()))
        );
        assert_eq!(
            label.get(&key(&[(2, 21)])),
            Some(&CellValue::Text("dear".into()))
        );
        // CLEARERROR without an alternative drops the error cells.
        assert_eq!(at(205, 10, 20), None);
        assert_eq!(at(205, 10, 21), Some(n(100.0)));
    }

    /// The P3 dimensional forms end to end: positions and names, neighbours,
    /// running windows, several OVER categories, ALL EXCEPT, WHERE, and the
    /// ordered reductions.
    #[test]
    fn dimensional_forms() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        let tp = vec![time, product];
        define(&mut m, 200, tp.clone(), "Pos = Quantity * POSITION(Time)");
        define(&mut m, 201, vec![time], "Year = @ITEM(Time)");
        define(&mut m, 202, tp.clone(), "Before = PREV(Quantity, Time)");
        define(
            &mut m,
            203,
            tp.clone(),
            "Growth = Quantity - PREV(Quantity, Time, 1, 0)",
        );
        define(&mut m, 204, tp.clone(), "After = NEXT(Quantity, Time)");
        define(
            &mut m,
            205,
            tp.clone(),
            "Run = SUM(Quantity OVER Time[FIRST..THIS])",
        );
        define(
            &mut m,
            206,
            vec![],
            "All = SUM(Quantity OVER Time, Product)",
        );
        define(
            &mut m,
            207,
            vec![product],
            "Ex = SUM(Quantity OVER ALL EXCEPT Product)",
        );
        define(
            &mut m,
            208,
            vec![product],
            "Big = SUM(Quantity OVER Time WHERE Quantity > 90)",
        );
        define(&mut m, 209, vec![product], "F = FIRST(Quantity OVER Time)");
        define(&mut m, 210, vec![product], "L = LAST(Quantity OVER Time)");
        define(
            &mut m,
            211,
            vec![],
            "J = TEXTJOIN(\"-\", TRUE, Year OVER Time)",
        );
        define(&mut m, 212, vec![], "N = @ITEMCOUNT(Product)");
        define(
            &mut m,
            213,
            tp.clone(),
            "ToGo = SUM(Quantity OVER Time[THIS..LAST])",
        );
        define(
            &mut m,
            214,
            tp,
            "Prior = SUM(Quantity OVER Time[THIS-1..THIS-1])",
        );
        let out = evaluate(&m, &(200..=214).map(MeasureId).collect::<Vec<_>>()).unwrap();
        let at =
            |id: u32, t: u32, p: u32| out[&MeasureId(id)].get(&key(&[(1, t), (2, p)])).cloned();
        let one = |id: u32, k: &[(u32, u32)]| out[&MeasureId(id)].get(&key(k)).cloned();
        let n = CellValue::num;
        // Quantity: 2025 A 100, B 50; 2026 A 120, B 80.
        assert_eq!(at(200, 11, 20), Some(n(240.0)), "2026 is position 2");
        assert_eq!(one(201, &[(1, 11)]), Some(CellValue::Text("2026".into())));
        assert_eq!(at(202, 10, 20), None, "no year before 2025");
        assert_eq!(at(202, 11, 20), Some(n(100.0)));
        assert_eq!(
            at(203, 10, 20),
            Some(n(100.0)),
            "default 0 before the first year"
        );
        assert_eq!(at(203, 11, 21), Some(n(30.0)));
        assert_eq!(at(204, 10, 21), Some(n(80.0)));
        assert_eq!(at(204, 11, 21), None);
        assert_eq!(at(205, 10, 20), Some(n(100.0)));
        assert_eq!(at(205, 11, 20), Some(n(220.0)), "running total");
        assert_eq!(one(206, &[]), Some(n(350.0)));
        assert_eq!(one(207, &[(2, 20)]), Some(n(220.0)));
        assert_eq!(one(208, &[(2, 20)]), Some(n(220.0)));
        assert_eq!(one(208, &[(2, 21)]), None, "no row passes WHERE: no cell");
        assert_eq!(one(209, &[(2, 21)]), Some(n(50.0)));
        assert_eq!(one(210, &[(2, 21)]), Some(n(80.0)));
        assert_eq!(one(211, &[]), Some(CellValue::Text("2025-2026".into())));
        assert_eq!(one(212, &[]), Some(n(2.0)));
        assert_eq!(at(213, 10, 20), Some(n(220.0)), "to go");
        assert_eq!(at(213, 11, 20), Some(n(120.0)));
        assert_eq!(
            at(214, 10, 20),
            Some(CellValue::Err(err::NA)),
            "an empty window"
        );
        assert_eq!(at(214, 11, 20), Some(n(100.0)));
    }

    /// A recurrence: a balance carried forward along Time, per Product,
    /// opening from a per-product measure; it updates incrementally.
    #[test]
    fn recurrence_carries_a_balance_forward() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        m.add_item(ItemId(12), time, "2027");
        let set = |m: &mut Model, id: u32, pairs: &[(CategoryId, ItemId)], v: f64| {
            m.set_input(
                MeasureId(id),
                Coordinate::from_pairs(pairs.iter().copied()),
                Value::Number(v),
            )
        };
        set(
            &mut m,
            101,
            &[(time, ItemId(12)), (product, ItemId(20))],
            10.0,
        );
        // Open[Product] = Price; Bal = PREV(Bal, Time, 1, Open) + Quantity.
        m.add_measure(Measure {
            id: MeasureId(300),
            name: Name("Bal".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Input,
            description: None,
        });
        let f = improv_core_model::parse_formula(&m, "Bal = PREV(Bal, Time, 1, Price) + Quantity")
            .unwrap();
        m.measures.get_mut(&MeasureId(300)).unwrap().kind = MeasureKind::Derived(f.formula);
        let out = evaluate(&m, &[MeasureId(300)]).unwrap();
        let bal = &out[&MeasureId(300)];
        let at = |t: u32, p: u32| bal.get(&key(&[(1, t), (2, p)])).cloned();
        // WidgetA: price 10; 10+100 = 110, +120 = 230, +10 = 240.
        assert_eq!(at(10, 20), Some(CellValue::num(110.0)));
        assert_eq!(at(11, 20), Some(CellValue::num(230.0)));
        assert_eq!(at(12, 20), Some(CellValue::num(240.0)));
        // WidgetB: price 20; 70, 150; no 2027 Quantity, so no 2027 balance.
        assert_eq!(at(11, 21), Some(CellValue::num(150.0)));
        assert_eq!(at(12, 21), None);
        // Incremental: changing 2025's Quantity re-runs WidgetA's chain.
        let (mut engine, _) = crate::session::Engine::new(&m, &[MeasureId(300)]).unwrap();
        let snap = engine
            .set(MeasureId(101), key(&[(1, 10), (2, 20)]), 0.0)
            .unwrap();
        assert_eq!(
            snap[&MeasureId(300)][&key(&[(1, 12), (2, 20)])],
            CellValue::num(140.0)
        );
        // Not a recurrence: a self-reference outside PREV, or along a
        // category the measure lacks.
        let mut bad = m.clone();
        let f = improv_core_model::parse_formula(&bad, "Bal = Bal + 1").unwrap();
        bad.measures.get_mut(&MeasureId(300)).unwrap().kind = MeasureKind::Derived(f.formula);
        let e = evaluate(&bad, &[MeasureId(300)]).unwrap_err().to_string();
        assert!(e.contains("only PREV"), "{e}");
    }

    /// A recurrence across a gap (no cell for an item) restarts from the
    /// default; an error in the body flows on as that error; a recurrence
    /// with no default has no cells.
    #[test]
    fn recurrence_gaps_errors_and_no_default() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        m.add_item(ItemId(12), time, "2027");
        let q = |t: u32, p: u32| Coordinate::from_pairs([(time, ItemId(t)), (product, ItemId(p))]);
        // WidgetA: 2025 and 2027 only (2026 missing: a gap).
        m.inputs.remove(&(MeasureId(101), q(11, 20)));
        m.set_input(MeasureId(101), q(12, 20), Value::Number(5.0));
        for (id, name, text) in [
            (300, "Bal", "Bal = PREV(Bal, Time, 1, 1000) + Quantity"),
            (
                301,
                "Inv",
                "Inv = PREV(Inv, Time, 1, 0) + 1 / (Quantity - 50)",
            ),
            (302, "Bare", "Bare = PREV(Bare, Time) + Quantity"),
        ] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![time, product],
                kind: MeasureKind::Input,
                description: None,
            });
            let f = improv_core_model::parse_formula(&m, text).unwrap();
            m.measures.get_mut(&MeasureId(id)).unwrap().kind = MeasureKind::Derived(f.formula);
        }
        let out = evaluate(&m, &[MeasureId(300), MeasureId(301), MeasureId(302)]).unwrap();
        let at =
            |id: u32, t: u32, p: u32| out[&MeasureId(id)].get(&key(&[(1, t), (2, p)])).cloned();
        assert_eq!(at(300, 10, 20), Some(CellValue::num(1100.0)));
        assert_eq!(at(300, 11, 20), None, "no 2026 cell");
        assert_eq!(
            at(300, 12, 20),
            Some(CellValue::num(1005.0)),
            "restarts from the default"
        );
        // WidgetB 2025 Quantity is 50: 1 / 0 is #DIV/0!, carried forward.
        assert_eq!(at(301, 10, 21), Some(CellValue::Err(err::DIV0)));
        assert_eq!(at(301, 11, 21), Some(CellValue::Err(err::DIV0)));
        assert!(
            out.get(&MeasureId(302)).is_none_or(|c| c.is_empty()),
            "no default: no chain"
        );
    }

    /// Windows anchored at LAST and ending at FIRST, an empty WHERE group,
    /// and TEXTJOIN of booleans.
    #[test]
    fn window_bounds_and_edges() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        let tp = vec![time, product];
        define(
            &mut m,
            200,
            tp.clone(),
            "A = SUM(Quantity OVER Time[LAST..LAST])",
        );
        define(
            &mut m,
            201,
            tp.clone(),
            "B = SUM(Quantity OVER Time[FIRST..FIRST])",
        );
        define(&mut m, 202, tp, "C = MAX(Quantity OVER Time[THIS+1..LAST])");
        define(&mut m, 204, vec![time, product], "Big = Quantity > 90");
        define(
            &mut m,
            203,
            vec![product],
            "D = TEXTJOIN(\",\", FALSE, Big OVER Time)",
        );
        let out = evaluate(&m, &(200..=204).map(MeasureId).collect::<Vec<_>>()).unwrap();
        let at =
            |id: u32, t: u32, p: u32| out[&MeasureId(id)].get(&key(&[(1, t), (2, p)])).cloned();
        assert_eq!(at(200, 10, 20), Some(CellValue::num(120.0)));
        assert_eq!(at(201, 11, 20), Some(CellValue::num(100.0)));
        assert_eq!(at(202, 10, 20), Some(CellValue::num(120.0)));
        assert_eq!(at(202, 11, 20), Some(CellValue::Err(err::NA)));
        assert_eq!(
            out[&MeasureId(203)].get(&key(&[(2, 20)])),
            Some(&CellValue::Text("TRUE,TRUE".into()))
        );
    }

    /// LET and LAMBDA expand at parse time; SCAN and REDUCE are folds
    /// along a category (recurrences under the hood).
    #[test]
    fn let_lambda_map_scan_reduce() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        let tp = vec![time, product];
        define(
            &mut m,
            200,
            tp.clone(),
            "A = LET(q, Quantity, p, Price, q * p + q)",
        );
        define(
            &mut m,
            201,
            tp.clone(),
            "B = MAP(Quantity, LAMBDA(v, v * 2))",
        );
        define(
            &mut m,
            202,
            tp.clone(),
            "C = LAMBDA(a, b, a - b)(Quantity, Price)",
        );
        define(
            &mut m,
            203,
            tp.clone(),
            "D = SCAN(0, Quantity OVER Time, LAMBDA(acc, v, acc + v))",
        );
        define(
            &mut m,
            204,
            vec![product],
            "E = REDUCE(1, Quantity OVER Time, LAMBDA(acc, v, acc * v))",
        );
        let out = evaluate(&m, &(200..=204).map(MeasureId).collect::<Vec<_>>()).unwrap();
        let at = |id: u32, k: &[(u32, u32)]| out[&MeasureId(id)].get(&key(k)).cloned();
        let n = CellValue::num;
        assert_eq!(at(200, &[(1, 10), (2, 20)]), Some(n(1100.0)));
        assert_eq!(at(201, &[(1, 10), (2, 21)]), Some(n(100.0)));
        assert_eq!(at(202, &[(1, 11), (2, 20)]), Some(n(110.0)));
        assert_eq!(at(203, &[(1, 10), (2, 20)]), Some(n(100.0)));
        assert_eq!(at(203, &[(1, 11), (2, 20)]), Some(n(220.0)), "running sum");
        assert_eq!(at(204, &[(2, 21)]), Some(n(4000.0)), "50 * 80");
        // Parse errors: LET without a body, an uncalled LAMBDA, a fold
        // without OVER, a LAMBDA with the wrong arity.
        for text in [
            "X = LET(q, Quantity)",
            "X = LAMBDA(v, v + 1)",
            "X = SCAN(0, Quantity, LAMBDA(a, v, a + v))",
            "X = SCAN(0, Quantity OVER Time, LAMBDA(v, v))",
            "X = MAP(Quantity, LAMBDA(a, b, a + b))",
            "X = SCAN(0, Quantity OVER Time, 1)",
        ] {
            assert!(
                improv_core_model::parse_formula(&m, text).is_err(),
                "{text}"
            );
        }
    }

    /// Operands sharing a category join over it: the matrix product is
    /// `AB = A * B` then `SUM(AB OVER K)`.
    #[test]
    fn matrix_product_by_join() {
        let (r, k, c) = (CategoryId(1), CategoryId(2), CategoryId(3));
        let mut m = Model::default();
        for (cat, name, base) in [(r, "R", 10), (k, "K", 20), (c, "C", 30)] {
            m.add_category(cat, name);
            for i in 0..2 {
                m.add_item(ItemId(base + i), cat, format!("{name}{i}"));
            }
        }
        let input = |m: &mut Model, id: u32, name: &str, cats: Vec<CategoryId>| {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: cats,
                kind: MeasureKind::Input,
                description: None,
            });
        };
        input(&mut m, 100, "A", vec![r, k]);
        input(&mut m, 101, "B", vec![k, c]);
        // A = [[1, 2], [3, 4]], B = [[5, 6], [7, 8]].
        let at = |a: (CategoryId, u32), b: (CategoryId, u32)| {
            Coordinate::from_pairs([(a.0, ItemId(a.1)), (b.0, ItemId(b.1))])
        };
        for (i, j, v) in [(0, 0, 1.0), (0, 1, 2.0), (1, 0, 3.0), (1, 1, 4.0)] {
            m.set_input(
                MeasureId(100),
                at((r, 10 + i), (k, 20 + j)),
                Value::Number(v),
            );
        }
        for (i, j, v) in [(0, 0, 5.0), (0, 1, 6.0), (1, 0, 7.0), (1, 1, 8.0)] {
            m.set_input(
                MeasureId(101),
                at((k, 20 + i), (c, 30 + j)),
                Value::Number(v),
            );
        }
        define(&mut m, 200, vec![r, k, c], "AB = A * B");
        define(&mut m, 201, vec![r, c], "P = SUM(AB OVER K)");
        let out = evaluate(&m, &[MeasureId(201)]).unwrap();
        let p = |i: u32, j: u32| {
            out[&MeasureId(201)]
                .get(&key(&[(1, 10 + i), (3, 30 + j)]))
                .cloned()
        };
        // [[19, 22], [43, 50]].
        assert_eq!(p(0, 0), Some(CellValue::num(19.0)));
        assert_eq!(p(0, 1), Some(CellValue::num(22.0)));
        assert_eq!(p(1, 0), Some(CellValue::num(43.0)));
        assert_eq!(p(1, 1), Some(CellValue::num(50.0)));
    }

    /// Named LAMBDAs: defined once, called by name, expanded when compiled
    /// (so redefining one changes every use); their errors.
    #[test]
    fn named_lambdas() {
        use improv_core_model::{parse_definition, Definition};
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        let def = |m: &Model, t: &str| match parse_definition(m, t).unwrap() {
            Definition::Lambda(l) => l.lambda,
            other => panic!("{other:?}"),
        };
        let l = def(&m, "Margin = LAMBDA(p, q, (p - q) / p)");
        m.lambdas.insert(7, l);
        let l = def(&m, "Twice = LAMBDA(x, Margin(x, 1) * 2)");
        m.lambdas.insert(8, l);
        define(
            &mut m,
            200,
            vec![time, product],
            "A = Margin(Quantity, Price)",
        );
        define(&mut m, 201, vec![time, product], "B = Twice(Quantity)");
        let at = |m: &Model, id: u32| {
            let out = evaluate(m, &[MeasureId(id)]).unwrap();
            out[&MeasureId(id)].get(&key(&[(1, 10), (2, 20)])).cloned()
        };
        // Quantity 100, Price 10.
        assert_eq!(at(&m, 200), Some(CellValue::num(0.9)));
        assert_eq!(at(&m, 201), Some(CellValue::num(1.98)));
        // Redefine Margin: both uses follow.
        let l = def(&m, "Margin = LAMBDA(p, q, p - q)");
        m.lambdas.insert(7, l);
        assert_eq!(at(&m, 200), Some(CellValue::num(90.0)));
        assert_eq!(at(&m, 201), Some(CellValue::num(198.0)));
        // A missing lambda is an error, not a crash.
        let mut gone = m.clone();
        gone.lambdas.remove(&7);
        assert!(evaluate(&gone, &[MeasureId(200)]).is_err());
        for (text, why) in [
            ("X = LAMBDA(p, p + Price)", "reads only its parameters"),
            ("SUM = LAMBDA(p, p)", "built-in function"),
            ("X = LAMBDA(p, p)(1)", "not called here"),
            ("Margin = LAMBDA(p, Margin(p, 1))", "unknown"),
            (
                "X = LAMBDA(a, SCAN(0, a OVER Time, LAMBDA(s, v, s + v)))",
                "",
            ),
        ] {
            let e = parse_definition(&m, text).unwrap_err();
            assert!(e.message.contains(why), "{text}: {}", e.message);
        }
        for text in ["Y = Margin(Quantity)", "Y = Margin(Quantity, Price, 1)"] {
            let e = improv_core_model::parse_formula(&m, text).unwrap_err();
            assert!(e.message.contains("takes 2"), "{text}: {}", e.message);
        }
        // Expansion: a cycle (only by hand-editing) and arity errors.
        let mut cyc = m.clone();
        let body = Expr::Call(improv_core_model::functions::named_lambda(9), vec![]);
        cyc.lambdas.insert(
            9,
            improv_core_model::NamedLambda {
                name: "Loop".into(),
                params: vec![],
                body: body.clone(),
            },
        );
        assert!(cyc.expand_lambdas(&body).unwrap_err().contains("cycle"));
        let bad = Expr::Call(improv_core_model::functions::named_lambda(7), vec![]);
        assert!(m.expand_lambdas(&bad).unwrap_err().contains("takes 2"));
        assert!(m
            .expand_lambdas(&Expr::Call(
                improv_core_model::functions::named_lambda(99),
                vec![]
            ))
            .unwrap_err()
            .contains("does not exist"));
    }

    /// `INTO R, C` lays a list over two categories, row-major: SORT's
    /// four values over a 2 × 2 grid, and LINEST's statistics as Excel's
    /// 5 × 2 block.
    #[test]
    fn into_two_categories() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let (row, col) = (CategoryId(3), CategoryId(4));
        let mut m = revenue_model();
        m.add_category(row, "Row");
        m.add_category(col, "Col");
        for i in 0..5 {
            m.add_item(ItemId(30 + i), row, format!("r{i}"));
        }
        for i in 0..2 {
            m.add_item(ItemId(40 + i), col, format!("c{i}"));
        }
        define(
            &mut m,
            200,
            vec![row, col],
            "S = SORT(Quantity OVER Time, Product INTO Row, Col)",
        );
        define(
            &mut m,
            201,
            vec![time, product],
            "X = POSITION(Time) + Quantity * 0",
        );
        define(
            &mut m,
            202,
            vec![product, row, col],
            "L = LINEST(Quantity, X OVER Time, TRUE, TRUE INTO Row, Col)",
        );
        let out = evaluate(&m, &[MeasureId(200), MeasureId(202)]).unwrap();
        let at = |id: u32, k: &[(u32, u32)]| out[&MeasureId(id)].get(&key(k)).cloned();
        let n = CellValue::num;
        // 50, 80, 100, 120 row-major over r0..r1 × c0..c1.
        assert_eq!(at(200, &[(3, 30), (4, 40)]), Some(n(50.0)));
        assert_eq!(at(200, &[(3, 30), (4, 41)]), Some(n(80.0)));
        assert_eq!(at(200, &[(3, 31), (4, 40)]), Some(n(100.0)));
        assert_eq!(at(200, &[(3, 32), (4, 40)]), None, "four values only");
        // Widget A: y = 100, 120 over x = 1, 2: slope 20, intercept 80; r² 1.
        assert_eq!(at(202, &[(2, 20), (3, 30), (4, 40)]), Some(n(20.0)));
        assert_eq!(at(202, &[(2, 20), (3, 30), (4, 41)]), Some(n(80.0)));
        assert_eq!(at(202, &[(2, 20), (3, 32), (4, 40)]), Some(n(1.0)));
        assert_eq!(out[&MeasureId(202)].len(), 20, "two 5 × 2 blocks");
        // Two x's (Over::Leading): Quantity on X and X² over Time per
        // product, 3 coefficients into one row; the options after the x's.
        let mut m3 = m.clone();
        define(&mut m3, 203, vec![time, product], "X2 = X * X");
        define(
            &mut m3,
            204,
            vec![product, col],
            "Two = LINEST(Quantity, X, X2 OVER Time, FALSE INTO Col)",
        );
        let f = improv_core_model::parse_formula(
            &m3,
            "Q = LINEST(Quantity, X, X2 OVER Time, FALSE INTO Col)",
        )
        .unwrap();
        let Expr::Call(_, a) = &f.formula.expr else {
            panic!()
        };
        let def = improv_core_model::functions::by_id(FuncId(1257)).unwrap();
        assert_eq!(
            (0..a.len()).filter(|i| def.reduced(*i, &a[*i])).count(),
            3,
            "y, x1, x2 reduced; FALSE a parameter"
        );
        assert!(
            improv_core_model::parse_formula(&m3, "Q = LINEST(Quantity OVER Time INTO Col)")
                .is_err()
        );
        // The GUI printer and the parser agree on the form.
        let f = improv_core_model::parse_formula(&m, "Z = SORT(Quantity OVER Time INTO Row, Col)")
            .unwrap();
        let Expr::Call(_, args) = &f.formula.expr else {
            panic!()
        };
        assert_eq!(
            args.last(),
            Some(&Expr::Call(
                improv_core_model::functions::INTO,
                vec![Expr::Category(row), Expr::Category(col)]
            ))
        );
        for (text, why) in [
            (
                "X = SORT(Quantity OVER Time INTO Row, Product)",
                "does not range over",
            ),
            (
                "X = SORT(Quantity OVER Time INTO Row, Col, Row)",
                "malformed INTO",
            ),
        ] {
            let e = improv_core_model::parse_formula(&m, text)
                .map_err(|e| e.message)
                .and_then(|f| {
                    crate::compiler::infer(
                        &CompileContext::for_model(&m),
                        MeasureId(209),
                        &f.formula.expr,
                    )
                    .map(|_| ())
                    .map_err(|e| e.to_string())
                })
                .unwrap_err();
            assert!(e.contains(why), "{text}: {e}");
        }
    }

    /// SEQUENCE and MAKEARRAY are positions; SELECTLIST is SELECT with a
    /// lookup over its own category; SORTBY takes several keys.
    #[test]
    fn sequence_makearray_selectlist_sortby() {
        let (time, product, rank) = (CategoryId(1), CategoryId(2), CategoryId(3));
        let mut m = revenue_model();
        m.add_category(rank, "Rank");
        for i in 0..2 {
            m.add_item(ItemId(30 + i), rank, format!("#{}", i + 1));
        }
        define(&mut m, 200, vec![time], "S = SEQUENCE(Time, 10, 5)");
        define(
            &mut m,
            201,
            vec![time, product],
            "G = MAKEARRAY(Time, Product, LAMBDA(r, c, r * 10 + c))",
        );
        // Price spread over Time × Product (the shape of Quantity); each Rank
        // item looks up the product whose price is 10 * its position.
        define(
            &mut m,
            205,
            vec![time, product],
            "P2 = Price + Quantity * 0",
        );
        define(&mut m, 202, vec![rank], "Want = POSITION(Rank) * 10");
        define(
            &mut m,
            203,
            vec![time, rank],
            "L = SELECTLIST(Quantity, P2 OVER Product, Want)",
        );
        // Products by price descending: B then A (the second key never ties).
        define(
            &mut m,
            204,
            vec![time, rank],
            "O = SORTBY(Quantity, P2 OVER Product, -1, Quantity, 1 INTO Rank)",
        );
        let ids: Vec<MeasureId> = (200..=205).map(MeasureId).collect();
        let out = evaluate(&m, &ids).unwrap();
        let at = |id: u32, k: &[(u32, u32)]| out[&MeasureId(id)].get(&key(k)).cloned();
        let n = CellValue::num;
        assert_eq!(at(200, &[(1, 10)]), Some(n(10.0)));
        assert_eq!(at(200, &[(1, 11)]), Some(n(15.0)));
        assert_eq!(at(201, &[(1, 11), (2, 20)]), Some(n(21.0)));
        assert_eq!(
            at(203, &[(1, 10), (3, 30)]),
            Some(n(100.0)),
            "Price 10 is A"
        );
        assert_eq!(at(203, &[(1, 11), (3, 31)]), Some(n(80.0)), "Price 20 is B");
        assert_eq!(at(204, &[(1, 10), (3, 30)]), Some(n(50.0)));
        assert_eq!(at(204, &[(1, 10), (3, 31)]), Some(n(100.0)));
        // BYROW/BYCOL: one value per item of the other categories.
        define(
            &mut m,
            206,
            vec![time],
            "R = BYROW(Quantity OVER Product, LAMBDA(row, MAX(row) - MIN(row)))",
        );
        define(
            &mut m,
            207,
            vec![product],
            "C = BYCOL(Quantity OVER Time, LAMBDA(col, SUM(col)))",
        );
        let out = evaluate(&m, &[MeasureId(206), MeasureId(207)]).unwrap();
        assert_eq!(out[&MeasureId(206)].get(&key(&[(1, 10)])), Some(&n(50.0)));
        assert_eq!(out[&MeasureId(207)].get(&key(&[(2, 21)])), Some(&n(130.0)));
        for text in [
            "X = BYROW(Quantity OVER Product, LAMBDA(row, row * 2))",
            "X = BYROW(Quantity, LAMBDA(row, SUM(row)))",
            "X = BYROW(Quantity OVER Product, 1)",
            "X = BYROW(Quantity OVER Product LAMBDA(r, SUM(r)))",
            "X = BYROW(Quantity OVER Product, LAMBDA(r, SUM(r))",
            "X = SEQUENCE(Time, 1, 2, 3)",
            "X = SEQUENCE(Quantity)",
            "X = MAKEARRAY(Time, Product, 1)",
            "X = MAKEARRAY(Time Product, LAMBDA(r, c, r))",
            "X = MAKEARRAY(Time, Product LAMBDA(r, c, r))",
            "X = MAKEARRAY(Time, Product, LAMBDA(r, r))",
        ] {
            assert!(
                improv_core_model::parse_formula(&m, text).is_err(),
                "{text}"
            );
        }
    }

    /// Array results land along the INTO category: SORT and UNIQUE of each
    /// product's quantities over Time, into a Rank category.
    #[test]
    fn array_results_land_on_the_into_category() {
        let (time, product, rank) = (CategoryId(1), CategoryId(2), CategoryId(3));
        let mut m = revenue_model();
        m.add_category(rank, "Rank");
        for i in 0..3 {
            m.add_item(ItemId(30 + i), rank, format!("#{}", i + 1));
        }
        define(
            &mut m,
            200,
            vec![product, rank],
            "Hi = SORT(Quantity OVER Time, -1 INTO Rank)",
        );
        define(
            &mut m,
            201,
            vec![rank],
            "U = UNIQUE(Price OVER Product INTO Rank)",
        );
        let out = evaluate(&m, &[MeasureId(200), MeasureId(201)]).unwrap();
        let hi = &out[&MeasureId(200)];
        assert_eq!(
            hi.get(&key(&[(2, 20), (3, 30)])),
            Some(&CellValue::num(120.0))
        );
        assert_eq!(
            hi.get(&key(&[(2, 20), (3, 31)])),
            Some(&CellValue::num(100.0))
        );
        assert_eq!(
            hi.get(&key(&[(2, 20), (3, 32)])),
            None,
            "two values, three items"
        );
        assert_eq!(out[&MeasureId(201)].len(), 2);
        // INTO is required for a list result and refused for one value; it
        // must not be one of the measure's own categories.
        for (text, why) in [
            ("X = SORT(Quantity OVER Time)", "INTO"),
            ("X = SUM(Quantity OVER Time INTO Rank)", "INTO is for list"),
            (
                "X = SORT(Quantity OVER Time INTO Product)",
                "does not range over",
            ),
        ] {
            let f = improv_core_model::parse_formula(&m, text).unwrap();
            let e = crate::compiler::infer(
                &CompileContext::for_model(&m),
                MeasureId(300),
                &f.formula.expr,
            )
            .unwrap_err()
            .to_string();
            assert!(e.contains(why), "{text}: {e}");
        }
        let _ = time;
    }

    /// A parameter over a category the measure lacks broadcasts: each of its
    /// cells sees the whole table (the shape lookups need).
    #[test]
    fn parameters_beyond_the_table_broadcast() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        // Sep[Time] per year; J joins each product's quantities with it.
        define(&mut m, 199, vec![time], "Sep = @ITEM(Time)");
        define(
            &mut m,
            200,
            vec![time],
            "J = TEXTJOIN(Sep, TRUE, Price OVER Product)",
        );
        let out = evaluate(&m, &[MeasureId(200)]).unwrap();
        let j = &out[&MeasureId(200)];
        assert_eq!(
            j.get(&key(&[(1, 10)])),
            Some(&CellValue::Text("10202520".into()))
        );
        assert_eq!(
            j.get(&key(&[(1, 11)])),
            Some(&CellValue::Text("10202620".into()))
        );
        let _ = product;
    }

    /// A parameter that is a measure (one value per group) joins per row.
    #[test]
    fn measure_parameters_join_per_group() {
        let product = CategoryId(2);
        let mut m = revenue_model();
        define(
            &mut m,
            200,
            vec![product],
            "J = TEXTJOIN(@ITEM(Product), TRUE, Quantity OVER Time)",
        );
        let out = evaluate(&m, &[MeasureId(200)]).unwrap();
        assert_eq!(
            out[&MeasureId(200)].get(&key(&[(2, 20)])),
            Some(&CellValue::Text("100WidgetA120".into()))
        );
    }

    /// The else-branch spans (the then-branch is a literal), the test is an
    /// error somewhere, and an aggregation meets an error in its group.
    #[test]
    fn if_else_spans_and_errors_flow() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        // Price - 10 is 0 for WidgetA: Ratio is #DIV/0! there.
        define(&mut m, 200, vec![product], "Ratio = 1 / (Price - 10)");
        define(
            &mut m,
            201,
            vec![time, product],
            "Pick = IF(Ratio > 0, 0, Revenue)",
        );
        define(&mut m, 202, vec![], "Total = SUM(Ratio OVER Product)");
        define(
            &mut m,
            203,
            vec![time, product],
            "Gate = IF(Price > 15, Revenue)",
        );
        let out = evaluate(&m, &(200..=203).map(MeasureId).collect::<Vec<_>>()).unwrap();
        let at =
            |id: u32, t: u32, p: u32| out[&MeasureId(id)].get(&key(&[(1, t), (2, p)])).cloned();
        // WidgetA: the test is #DIV/0!, so each spanned cell is that error.
        assert_eq!(at(201, 10, 20), Some(CellValue::Err(err::DIV0)));
        assert_eq!(at(201, 11, 20), Some(CellValue::Err(err::DIV0)));
        // WidgetB: Ratio 0.1 > 0 selects the literal 0, at Revenue's cells.
        assert_eq!(at(201, 10, 21), Some(CellValue::num(0.0)));
        assert_eq!(out[&MeasureId(201)].len(), 4);
        assert_eq!(
            out[&MeasureId(202)][&CoordKey::new()],
            CellValue::Err(err::DIV0)
        );
        // No else-branch: a false test is no cell.
        assert_eq!(at(203, 10, 20), None);
        assert_eq!(at(203, 10, 21), Some(CellValue::num(1000.0)));
    }

    /// n-ary scalar calls align every collection argument and fold literals;
    /// an aggregation of booleans and a product reduce typed values.
    #[test]
    fn n_ary_calls_and_typed_aggregations() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut m = revenue_model();
        define(
            &mut m,
            200,
            vec![time, product],
            "S = SUM(Price, Quantity, 1, Price)",
        );
        define(
            &mut m,
            201,
            vec![product],
            "P = PRODUCT(Quantity OVER Time)",
        );
        define(&mut m, 202, vec![product], "Big = Price > 15");
        define(&mut m, 203, vec![], "AnyBig = OR(Big OVER Product)");
        define(&mut m, 204, vec![], "AllBig = AND(Big OVER Product)");
        define(&mut m, 205, vec![product], "R = ROUND(Price / 3, 2)");
        define(&mut m, 206, vec![], "Pi = PI()");
        let out = evaluate(&m, &(200..=206).map(MeasureId).collect::<Vec<_>>()).unwrap();
        let n = CellValue::num;
        assert_eq!(out[&MeasureId(200)][&key(&[(1, 10), (2, 20)])], n(121.0));
        assert_eq!(out[&MeasureId(201)][&key(&[(2, 21)])], n(4000.0));
        assert_eq!(
            out[&MeasureId(203)][&CoordKey::new()],
            CellValue::Bool(true)
        );
        assert_eq!(
            out[&MeasureId(204)][&CoordKey::new()],
            CellValue::Bool(false)
        );
        assert_eq!(out[&MeasureId(205)][&key(&[(2, 20)])], n(3.33));
        assert_eq!(
            out[&MeasureId(206)][&CoordKey::new()],
            n(std::f64::consts::PI)
        );
    }

    fn key(pairs: &[(u32, u32)]) -> CoordKey {
        let mut k: Vec<(u32, u32)> = pairs.to_vec();
        k.sort();
        k.into()
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
        let err = evaluate(&model, &[MeasureId(1)]).unwrap_err().to_string();
        assert_eq!(err, "formula cycle: A → B → A");
        // A longer loop is named whole, from its lowest id, whichever
        // measure the search starts at.
        let refs = |a| improv_core_model::Formula::new(Expr::Ref(a, DimensionSpec::default()));
        for (id, name, dep) in [(3, "C", 1), (1, "A", 2)] {
            model.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![],
                kind: MeasureKind::Derived(refs(MeasureId(dep))),
                description: None,
            });
        }
        model.measures.get_mut(&MeasureId(2)).unwrap().kind =
            MeasureKind::Derived(refs(MeasureId(3)));
        for start in 1..=3 {
            let err = evaluate(&model, &[MeasureId(start)]).unwrap_err();
            assert_eq!(err.to_string(), "formula cycle: A → B → C → A");
        }
    }

    /// A plan the compiler would never emit (an unknown function) still
    /// builds; its cells are `#NAME?`, and a ref to an absent measure is an
    /// error.
    #[test]
    fn hand_built_plans() {
        use crate::typed::{Dim, TypeInfo};
        let ty = TypeInfo {
            value_type: ValueType::Number,
            dim: Dim::scalar(),
        };
        let unknown = PlanNode {
            kind: PlanNodeKind::FuncCall {
                func: FuncId(999),
                args: vec![],
            },
            ty: ty.clone(),
        };
        let missing = PlanNode {
            kind: PlanNodeKind::InputMeasure(MeasureId(77)),
            ty,
        };
        let (ok, err) = timely::execute::execute_directly(move |worker| {
            worker.dataflow::<u64, _, _>(|scope| {
                let ok = build_coll(&unknown, scope, &HashMap::new(), &HashMap::new()).is_ok();
                let err = build_coll(&missing, scope, &HashMap::new(), &HashMap::new())
                    .err()
                    .map(|e| e.to_string());
                (ok, err)
            })
        });
        assert!(ok);
        assert!(err.unwrap().contains("no collection for measure"));
        assert_eq!(
            crate::builtins::eval(FuncId(999), &[]),
            CellValue::Err(err::NAME)
        );
    }

    #[test]
    fn cell_ops_cover_every_operator() {
        let n = CellValue::num;
        let b = CellValue::Bool;
        assert_eq!(apply_binary(BinaryOp::Add, &n(2.0), &n(3.0)), n(5.0));
        assert_eq!(apply_binary(BinaryOp::Div, &n(1.0), &n(4.0)), n(0.25));
        let div0 = CellValue::Err(err::DIV0);
        assert_eq!(apply_binary(BinaryOp::Div, &n(1.0), &n(0.0)), div0);
        assert_eq!(apply_binary(BinaryOp::Div, &n(0.0), &n(0.0)), div0);
        // Errors propagate, the left one first; a non-number is #VALUE!.
        let na = CellValue::Err(err::NA);
        assert_eq!(apply_binary(BinaryOp::Add, &na, &div0), na);
        assert_eq!(apply_binary(BinaryOp::Mul, &n(1.0), &div0), div0);
        assert_eq!(apply_binary(BinaryOp::Eq, &n(1.0), &na), na);
        assert_eq!(apply_binary(BinaryOp::And, &b(true), &na), na);
        let txt = |s: &str| CellValue::Text(s.into());
        assert_eq!(
            apply_binary(BinaryOp::Add, &txt("a"), &n(1.0)),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(apply_binary(BinaryOp::Lt, &txt("a"), &txt("b")), b(true));
        assert_eq!(apply_binary(BinaryOp::Ge, &txt("a"), &txt("b")), b(false));
        assert_eq!(apply_binary(BinaryOp::Le, &b(false), &b(true)), b(true));
        assert_eq!(
            apply_binary(BinaryOp::Gt, &CellValue::Date(2), &CellValue::Date(1)),
            b(true)
        );
        assert_eq!(
            apply_binary(BinaryOp::Lt, &txt("a"), &n(1.0)),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(
            apply_binary(BinaryOp::Mul, &n(1e308), &n(10.0)),
            CellValue::Err(err::NUM)
        );
        assert_eq!(apply_unary(UnaryOp::Neg, &na), na);
        assert_eq!(apply_binary(BinaryOp::Pow, &n(2.0), &n(10.0)), n(1024.0));
        let day = 86_400_000;
        let dt = CellValue::Date;
        assert_eq!(apply_binary(BinaryOp::Add, &dt(0), &n(2.0)), dt(2 * day));
        assert_eq!(apply_binary(BinaryOp::Add, &n(1.0), &dt(0)), dt(day));
        assert_eq!(
            apply_binary(BinaryOp::Sub, &dt(3 * day), &n(1.0)),
            dt(2 * day)
        );
        assert_eq!(apply_binary(BinaryOp::Sub, &dt(3 * day), &dt(day)), n(2.0));
        assert_eq!(
            apply_binary(BinaryOp::Mul, &dt(day), &n(2.0)),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(apply_binary(BinaryOp::Pow, &n(0.0), &n(0.0)), n(1.0));
        assert_eq!(apply_binary(BinaryOp::Pow, &n(0.0), &n(-1.0)), div0);
        assert_eq!(
            apply_binary(BinaryOp::Pow, &n(-8.0), &n(0.5)),
            CellValue::Err(err::NUM)
        );
        assert_eq!(
            apply_binary(BinaryOp::Concat, &txt("a"), &n(1.5)),
            txt("a1.5")
        );
        assert_eq!(
            apply_binary(BinaryOp::Concat, &b(true), &txt("!")),
            txt("TRUE!")
        );
        assert_eq!(apply_binary(BinaryOp::Concat, &txt("a"), &na), na);
        assert_eq!(
            apply_unary(UnaryOp::Neg, &txt("a")),
            CellValue::Err(err::VALUE)
        );
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
        // Numerically, not by bits: -0 = 0, and NaN is unequal to itself.
        assert_eq!(apply_binary(BinaryOp::Eq, &n(-0.0), &n(0.0)), b(true));
        assert_eq!(apply_binary(BinaryOp::Ne, &n(-0.0), &n(0.0)), b(false));
        assert_eq!(
            apply_binary(BinaryOp::Eq, &n(f64::NAN), &n(f64::NAN)),
            b(false)
        );
        assert_eq!(apply_binary(BinaryOp::Eq, &b(true), &n(1.0)), b(false));
        assert_eq!(apply_unary(UnaryOp::Neg, &n(2.0)), n(-2.0));
        assert_eq!(apply_unary(UnaryOp::Not, &b(true)), b(false));
        assert!(truthy(&n(1.0)) && !truthy(&n(0.0)));
        assert!(truthy(&CellValue::Text("x".into())) && !truthy(&CellValue::Text(String::new())));
        assert!(truthy(&CellValue::Date(1)) && !truthy(&CellValue::Date(0)));
        assert!(!truthy(&CellValue::Err(0)));
    }

    #[test]
    fn aggregates() {
        let rows = |vals: Vec<CellValue>| Rows {
            pos: (0..vals.len() as u32).map(|i| vec![i]).collect(),
            vals: vals.into_iter().map(|v| vec![v]).collect(),
            capacity: 3,
        };
        let r = rows(vec![
            CellValue::num(4.0),
            CellValue::num(1.0),
            CellValue::num(7.0),
        ]);
        assert_eq!(
            [1, 2, 3, 4, 5, 6].map(|f| reduce::reduce(FuncId(f), &r, &[])),
            [12.0, 4.0, 1.0, 7.0, 28.0, 66.0].map(CellValue::num)
        );
        let b = rows(vec![
            CellValue::Bool(true),
            CellValue::Bool(false),
            CellValue::Bool(true),
        ]);
        assert_eq!(
            [7, 8, 9].map(|f| reduce::reduce(FuncId(f), &b, &[])),
            [false, true, false].map(CellValue::Bool)
        );
        // An error in the group is the result: the first in item order.
        let e = rows(vec![
            CellValue::num(1.0),
            CellValue::Err(err::NA),
            CellValue::Err(err::DIV0),
        ]);
        assert_eq!(reduce::reduce(FuncId(1), &e, &[]), CellValue::Err(err::NA));
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
