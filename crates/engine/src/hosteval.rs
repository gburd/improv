//! Host-side evaluation of a FIXPOINT's step.
//!
//! A FIXPOINT's step, `body(r)`, is a compiled plan like any measure's. Fed
//! through the dataflow, a round re-joins every edge an iterate cell touches,
//! and in PageRank every cell moves every round: a round is a full pass at
//! dataflow cost (arrangement merges, history replay, allocation). The worker
//! instead evaluates the same plan in memory, the parts that do not read r
//! (and the join indexes over them) once per solve and the rest per round.
//!
//! Only the plan's structure lives here: alignment, a reduction's rows,
//! RELABEL, FILL and COALESCE, IF's branches. Every cell operation is the
//! dataflow builder's own (`apply_unary`, `apply_binary`, `call_with`, the
//! `reduce` kernels), and a reduction sees its rows in the order the
//! dataflow's `reduce` sorts them, so a round gives the dataflow's values bit
//! for bit. [`covers`] says which plans it takes; the rest (`PREV`,
//! recurrences) stay on the dataflow.

use crate::dataflow::{
    agg_key, agg_order, agg_rows, apply_binary, apply_unary, call_with, grid_keys, literal_value,
    project, remap_key, remap_table, shared_cats, union_keys, where_row, windows, AggRow, AggSpec,
};
use crate::plan::{PlanNode, PlanNodeKind as K};
use crate::reduce;
use crate::session::MeasureValues;
use crate::{CellValue, CoordKey};
use improv_core_model::{CategoryId, FuncId, ItemId, MeasureId};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// A collection: a multiset of cells (a cell may repeat, as a dataflow
/// collection's multiplicity).
type Cells = Vec<(CoordKey, CellValue)>;
/// A collection's cells by their projection onto join categories.
type Index = HashMap<CoordKey, Vec<(CoordKey, CellValue)>>;
/// A plan node, by address (the plan outlives a solve).
type Node = *const PlanNode;

/// The plan nodes `n` reads, for the kinds [`covers`] takes.
fn kids(n: &PlanNode) -> Vec<&PlanNode> {
    match &n.kind {
        K::MapUnary(_, a) | K::Remap { input: a, .. } => vec![a],
        K::MapBinary(_, a, b)
        | K::Join {
            left: a, right: b, ..
        } => vec![a, b],
        K::Aggregate {
            inputs,
            params,
            pred,
            ..
        } => inputs.iter().chain(params).chain(pred.as_deref()).collect(),
        K::Coalesce { args, .. } | K::FuncCall { args, .. } => args.iter().collect(),
        K::If {
            test,
            then,
            otherwise,
            ..
        } => [Some(&**test), Some(&**then), otherwise.as_deref()]
            .into_iter()
            .flatten()
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether [`Host`] evaluates `plan`: every node of a kind it takes, every
/// literal a cell value, and every measure read either `r` or one `has` (a
/// collection the dataflow built), so the dataflow would build it too.
pub(crate) fn covers(plan: &PlanNode, r: MeasureId, has: &dyn Fn(MeasureId) -> bool) -> bool {
    let me = match &plan.kind {
        K::InputMeasure(m) => *m == r || has(*m),
        K::Literal(v) => CellValue::from_model_value(v).is_some(),
        K::Aggregate { inputs, .. } => !inputs.is_empty(),
        K::Recurrence { .. } | K::Shift { .. } | K::Fixpoint { .. } => false,
        _ => true,
    };
    me && kids(plan).into_iter().all(|k| covers(k, r, has))
}

/// Add the measures `plan` reads to `out`.
pub(crate) fn reads(plan: &PlanNode, out: &mut HashSet<MeasureId>) {
    if let K::InputMeasure(m) = plan.kind {
        out.insert(m);
    }
    for k in kids(plan) {
        reads(k, out);
    }
}

/// Mark the nodes of `n` that read `r`; whether `n` does.
fn mark(n: &PlanNode, r: MeasureId, live: &mut HashSet<Node>) -> bool {
    let mut me = matches!(n.kind, K::InputMeasure(m) if m == r);
    for k in kids(n) {
        me |= mark(k, r, live);
    }
    if me {
        live.insert(n as Node);
    }
    me
}

/// A FIXPOINT step's plan, evaluated round after round at one iterate after
/// another, with the measures it reads (but r) fixed for the solve.
pub(crate) struct Host<'p> {
    plan: &'p PlanNode,
    r: MeasureId,
    env: HashMap<MeasureId, Arc<MeasureValues>>,
    /// The nodes that read r, evaluated every round.
    live: HashSet<Node>,
    /// The other nodes' cells, evaluated once.
    fixed: HashMap<Node, Rc<Cells>>,
    /// Join indexes over the other nodes, by node and join categories.
    index: HashMap<(Node, Vec<u32>), Rc<Index>>,
}

impl<'p> Host<'p> {
    /// `plan` (covered: [`covers`]) reading the iterate as `r` and every
    /// other measure from `env` (a measure `env` lacks has no cells).
    pub(crate) fn new(
        plan: &'p PlanNode,
        r: MeasureId,
        env: HashMap<MeasureId, Arc<MeasureValues>>,
    ) -> Self {
        let mut live = HashSet::new();
        mark(plan, r, &mut live);
        Host {
            plan,
            r,
            env,
            live,
            fixed: HashMap::new(),
            index: HashMap::new(),
        }
    }

    /// The step at iterate `x`: one value per cell (of several, the
    /// greatest, as the engine's snapshot keeps the last of a sorted batch).
    pub(crate) fn step(&mut self, x: &MeasureValues) -> MeasureValues {
        let mut out = MeasureValues::new();
        for (k, v) in self.eval(self.plan, x).iter() {
            match out.get(k) {
                Some(w) if w >= v => {}
                _ => {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        out
    }

    fn eval(&mut self, n: &PlanNode, x: &MeasureValues) -> Rc<Cells> {
        let p = n as Node;
        if self.live.contains(&p) {
            return Rc::new(self.compute(n, x));
        }
        if let Some(c) = self.fixed.get(&p) {
            return Rc::clone(c);
        }
        let c = Rc::new(self.compute(n, x));
        self.fixed.insert(p, Rc::clone(&c));
        c
    }

    fn compute(&mut self, n: &PlanNode, x: &MeasureValues) -> Cells {
        match &n.kind {
            K::InputMeasure(m) => {
                let vals = if *m == self.r {
                    Some(x)
                } else {
                    self.env.get(m).map(|a| &**a)
                };
                vals.map_or_else(Vec::new, |v| {
                    v.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
                })
            }
            K::Literal(v) => {
                let v = CellValue::from_model_value(v).expect("covers checks literals");
                vec![(Vec::new(), v)]
            }
            K::MapUnary(op, a) => self
                .eval(a, x)
                .iter()
                .map(|(k, v)| (k.clone(), apply_unary(*op, v)))
                .collect(),
            K::MapBinary(op, a, b) => {
                let op = *op;
                match (literal_value(a), literal_value(b)) {
                    (Some(l), None) => self
                        .eval(b, x)
                        .iter()
                        .map(|(k, v)| (k.clone(), apply_binary(op, &l, v)))
                        .collect(),
                    (None, Some(r)) => self
                        .eval(a, x)
                        .iter()
                        .map(|(k, v)| (k.clone(), apply_binary(op, v, &r)))
                        .collect(),
                    (Some(l), Some(r)) => vec![(Vec::new(), apply_binary(op, &l, &r))],
                    (None, None) => {
                        let (l, r) = (self.eval(a, x), self.eval(b, x));
                        self.align(&l, b, &r, &shared_cats(a, b))
                            .into_iter()
                            .map(|(k, (u, v))| (k, apply_binary(op, &u, &v)))
                            .collect()
                    }
                }
            }
            K::Join {
                left,
                right,
                join_keys,
            } => {
                let (l, r) = (self.eval(left, x), self.eval(right, x));
                self.align(&l, right, &r, join_keys)
                    .into_iter()
                    .map(|(k, (u, _))| (k, u))
                    .collect()
            }
            K::Aggregate {
                inputs: ins,
                params,
                pred,
                group_by,
                func,
                order,
                window,
                per_item,
                into,
            } => self.aggregate(
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
                },
                x,
            ),
            K::Items(cat, vals) => vals
                .iter()
                .filter_map(|(i, v)| Some((vec![(cat.0, i.0)], CellValue::from_model_value(v)?)))
                .collect(),
            K::Coalesce { args, grid } => self.coalesce(n, args, grid.as_deref(), x),
            K::Remap { input, maps } => {
                let (from, to) = remap_table(maps);
                self.eval(input, x)
                    .iter()
                    .filter_map(|(k, v)| Some((remap_key(k, &from, &to)?, v.clone())))
                    .collect()
            }
            K::FuncCall { func, args } => self.call(*func, args, x),
            K::If {
                test,
                then,
                otherwise,
                spanning,
            } => {
                let t = self.eval(test, x);
                let a = self.eval(then, x);
                let b = otherwise.as_deref().map(|o| (o, self.eval(o, x)));
                // The test over the result's categories (see the dataflow's If).
                let dom: Cells = match spanning {
                    None => (*t).clone(),
                    Some(i) => {
                        let (node, coll) = match (*i, &b) {
                            (0, _) | (_, None) => (&**then, &a),
                            (_, Some((o, c))) => (*o, c),
                        };
                        self.align(&t, node, coll, &shared_cats(test, node))
                            .into_iter()
                            .map(|(k, (tv, _))| (k, tv))
                            .collect()
                    }
                };
                let mut out = self.pick(true, &dom, then, &a);
                if let Some((o, c)) = &b {
                    out.extend(self.pick(false, &dom, o, c));
                }
                out.extend(
                    dom.into_iter()
                        .filter(|(_, v)| matches!(v, CellValue::Err(_))),
                );
                out
            }
            K::Recurrence { .. } | K::Shift { .. } | K::Fixpoint { .. } => {
                unreachable!("covers keeps these on the dataflow")
            }
        }
    }

    /// The dataflow's `align_with`: every pair of a row of `l` and a cell of
    /// node `n` (its cells `c`) that agree on `keys`, at the union coordinate.
    /// A node that does not read r keeps its index for the solve.
    fn align<V: Clone>(
        &mut self,
        l: &[(CoordKey, V)],
        n: &PlanNode,
        c: &Cells,
        keys: &[CategoryId],
    ) -> Vec<(CoordKey, (V, CellValue))> {
        let keys: Vec<u32> = keys.iter().map(|c| c.0).collect();
        let p = n as Node;
        let idx = if self.live.contains(&p) {
            Rc::new(index(c, &keys))
        } else {
            let e = self.index.entry((p, keys.clone()));
            Rc::clone(e.or_insert_with(|| Rc::new(index(c, &keys))))
        };
        let mut out = Vec::new();
        for (lk, lv) in l {
            if let Some(rs) = idx.get(&project(lk, &keys)) {
                for (rk, rv) in rs {
                    out.push((union_keys(lk, rk), (lv.clone(), rv.clone())));
                }
            }
        }
        out
    }

    /// The dataflow's `build_aggregate`, its rows grouped and sorted as its
    /// `reduce` sees them.
    fn aggregate(&mut self, a: AggSpec<'_>, x: &MeasureValues) -> Cells {
        let (first, rest) = a.ins.split_first().expect("covers checks inputs");
        let with_order = |n: &PlanNode| -> Vec<CategoryId> {
            let order = a.order.iter().map(|(c, _)| *c);
            n.ty.dim.categories.iter().copied().chain(order).collect()
        };
        let mut full = with_order(first);
        let mut rows: Vec<(CoordKey, Vec<CellValue>)> = self
            .eval(first, x)
            .iter()
            .map(|(k, v)| (k.clone(), vec![v.clone()]))
            .collect();
        for n in rest {
            let c = self.eval(n, x);
            let mine = with_order(n);
            let shared: Vec<CategoryId> =
                mine.iter().copied().filter(|c| full.contains(c)).collect();
            let more: Vec<CategoryId> = mine.into_iter().filter(|c| !full.contains(c)).collect();
            full.extend(more);
            rows = push_each(self.align(&rows, n, &c, &shared));
        }
        if let Some(p) = a.pred {
            let pc = self.eval(p, x);
            rows = self
                .align(&rows, p, &pc, &p.ty.dim.categories)
                .into_iter()
                .filter_map(|(k, (vs, t))| where_row(vs, t).map(|vs| (k, vs)))
                .collect();
        }
        let mut consts = Vec::new();
        let mut row_dim = full;
        for p in a.params {
            if let Some(v) = literal_value(p) {
                consts.push(Some(v));
                continue;
            }
            consts.push(None);
            let pc = self.eval(p, x);
            let cats = &p.ty.dim.categories;
            let shared: Vec<CategoryId> = cats
                .iter()
                .copied()
                .filter(|c| row_dim.contains(c))
                .collect();
            for c in cats {
                if !row_dim.contains(c) {
                    row_dim.push(*c);
                }
            }
            rows = push_each(self.align(&rows, p, &pc, &shared));
        }
        let order = agg_order(a.order);
        let capacity: usize = a.order.iter().map(|(_, items)| items.len()).product();
        let gb: Vec<u32> = a.group_by.iter().map(|c| c.0).collect();
        let mut groups: HashMap<CoordKey, Vec<AggRow>> = HashMap::new();
        for (k, vs) in rows {
            let (g, row) = agg_key(&k, vs, &order, &gb);
            groups.entry(g).or_default().push(row);
        }
        let cells = (!a.into.is_empty()).then(|| grid_keys(a.into));
        let cat0 = a.order.first().map_or(0, |(c, _)| c.0);
        let n_in = a.ins.len();
        let mut out = Vec::new();
        for (g, mut group) in groups {
            group.sort();
            let mut input: Vec<(&AggRow, isize)> = Vec::new();
            for row in &group {
                match input.last_mut() {
                    Some((r, m)) if *r == row => *m += 1,
                    _ => input.push((row, 1)),
                }
            }
            let (rs, params, items) = agg_rows(&input, n_in, &consts, capacity);
            if let Some(cells) = &cells {
                for (cell, v) in cells.iter().zip(reduce::array(a.func, &rs, &params)) {
                    let mut k = g.clone();
                    k.extend(cell.iter().copied());
                    k.sort_unstable();
                    out.push((k, v));
                }
            } else if a.window.is_none() && !a.per_item {
                out.push((g, reduce::reduce(a.func, &rs, &params)));
            } else {
                let outs = match a.window {
                    None => reduce::per_item(a.func, &rs, &params),
                    Some(w) => windows(a.func, &rs, &params, w),
                };
                for (item, v) in items.into_iter().zip(outs) {
                    let mut k = g.clone();
                    k.push((cat0, item));
                    k.sort_unstable();
                    out.push((k, v));
                }
            }
        }
        out
    }

    /// The dataflow's Coalesce: at each coordinate of the domain, the first
    /// argument's cell.
    fn coalesce(
        &mut self,
        n: &PlanNode,
        args: &[PlanNode],
        grid: Option<&[(CategoryId, Vec<ItemId>)]>,
        x: &MeasureValues,
    ) -> Cells {
        let spans = |a: &PlanNode| a.ty.dim == n.ty.dim;
        let colls: Vec<Rc<Cells>> = args.iter().map(|a| self.eval(a, x)).collect();
        let keys = match grid {
            Some(g) => grid_keys(g),
            None => {
                let mut ks: Vec<CoordKey> = args
                    .iter()
                    .zip(&colls)
                    .filter(|(a, _)| spans(a))
                    .flat_map(|(_, c)| c.iter().map(|(k, _)| k.clone()))
                    .collect();
                ks.sort_unstable();
                ks.dedup();
                ks
            }
        };
        let domain: Cells = keys
            .into_iter()
            .map(|k| (k, CellValue::Bool(true)))
            .collect();
        // Per coordinate the lowest (position, value), as the sorted reduce.
        let mut best: HashMap<CoordKey, (u32, CellValue)> = HashMap::new();
        let mut offer = |k: CoordKey, t: (u32, CellValue)| match best.get(&k) {
            Some(b) if *b <= t => {}
            _ => {
                best.insert(k, t);
            }
        };
        for (i, (a, c)) in args.iter().zip(&colls).enumerate() {
            let i = i as u32;
            if spans(a) {
                for (k, v) in c.iter() {
                    offer(k.clone(), (i, v.clone()));
                }
            } else {
                for (k, (_, v)) in self.align(&domain, a, c, &a.ty.dim.categories) {
                    offer(k, (i, v));
                }
            }
        }
        best.into_iter().map(|(k, (_, v))| (k, v)).collect()
    }

    /// The dataflow's FuncCall: the collection arguments aligned into one
    /// row per coordinate, the function applied to each.
    fn call(&mut self, func: FuncId, args: &[PlanNode], x: &MeasureValues) -> Cells {
        let template: Vec<Option<CellValue>> = args.iter().map(literal_value).collect();
        let colls: Vec<&PlanNode> = args
            .iter()
            .zip(&template)
            .filter(|(_, t)| t.is_none())
            .map(|(a, _)| a)
            .collect();
        let Some((first, rest)) = colls.split_first() else {
            return vec![(Vec::new(), call_with(func, &template, &[]))];
        };
        let mut acc: Vec<(CoordKey, Vec<CellValue>)> = self
            .eval(first, x)
            .iter()
            .map(|(k, v)| (k.clone(), vec![v.clone()]))
            .collect();
        let mut dim = first.ty.dim.clone();
        for a in rest {
            let c = self.eval(a, x);
            let keys: Vec<CategoryId> = dim
                .categories
                .iter()
                .copied()
                .filter(|c| a.ty.dim.categories.contains(c))
                .collect();
            acc = push_each(self.align(&acc, a, &c, &keys));
            dim = dim.union(&a.ty.dim);
        }
        acc.into_iter()
            .map(|(k, vs)| (k, call_with(func, &template, &vs)))
            .collect()
    }

    /// IF's branch `node` (cells `c`) where the test (over `dom`) is `want`.
    fn pick(&mut self, want: bool, dom: &Cells, node: &PlanNode, c: &Cells) -> Cells {
        let sel: Cells = dom
            .iter()
            .filter(|(_, v)| *v == CellValue::Bool(want))
            .cloned()
            .collect();
        self.align(&sel, node, c, &node.ty.dim.categories)
            .into_iter()
            .map(|(k, (_, v))| (k, v))
            .collect()
    }
}

/// `c`'s cells by their projection onto `keys`.
fn index(c: &Cells, keys: &[u32]) -> Index {
    let mut idx: Index = HashMap::new();
    for (k, v) in c {
        idx.entry(project(k, keys))
            .or_default()
            .push((k.clone(), v.clone()));
    }
    idx
}

/// Aligned rows with the joined value appended to each row's values.
fn push_each(
    rows: Vec<(CoordKey, (Vec<CellValue>, CellValue))>,
) -> Vec<(CoordKey, Vec<CellValue>)> {
    rows.into_iter()
        .map(|(k, (mut vs, v))| {
            vs.push(v);
            (k, vs)
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! The host step against the dataflow: a formula reading an input `R`
    //! as the iterate, evaluated both ways on random small models, must
    //! agree bit for bit.
    use super::*;
    use crate::dataflow::{compile_plans, evaluate};
    use hegel::generators as gs;
    use hegel::{HealthCheck, TestCase};
    use improv_core_model::{
        parse_formula, Coordinate, Measure, MeasureKind, Model, Name, Value, ValueType,
    };

    const A: CategoryId = CategoryId(1);
    const B: CategoryId = CategoryId(2);
    const R: MeasureId = MeasureId(100);
    const T: MeasureId = MeasureId(200);

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

    fn define(m: &mut Model, id: u32, text: &str) -> bool {
        let Ok(f) = parse_formula(m, text) else {
            return false;
        };
        let Ok((vt, cats)) =
            crate::compiler::derived_shape(m, MeasureId(id), &f.target.0, &f.formula)
        else {
            return false;
        };
        m.add_measure(Measure {
            id: MeasureId(id),
            name: f.target,
            value_type: vt,
            categories: cats,
            kind: MeasureKind::Derived(f.formula),
            description: None,
        });
        compile_plans(m, &[MeasureId(id)]).is_ok()
    }

    /// A over n items, B its role (the same names) plus `extra` items of
    /// its own; inputs R[A] (the iterate), X[A], Y[A, B], Z[B], K[B], each
    /// cell present or not by `cell`; D = Y * 2 - 3.
    fn model(n: usize, extra: usize, cell: &mut dyn FnMut() -> Option<f64>) -> Model {
        let mut m = Model::new();
        m.add_category(A, "A");
        m.add_category(B, "B");
        for i in 0..n {
            m.add_item(ItemId(10 + i as u32), A, format!("p{i}"));
            m.add_item(ItemId(30 + i as u32), B, format!("p{i}"));
        }
        for i in 0..extra {
            m.add_item(ItemId(60 + i as u32), B, format!("q{i}"));
        }
        let a: Vec<ItemId> = (0..n).map(|i| ItemId(10 + i as u32)).collect();
        let b: Vec<ItemId> = (0..n + extra)
            .map(|i| ItemId(if i < n { 30 + i } else { 60 + i - n } as u32))
            .collect();
        input(&mut m, 100, "R", vec![A]);
        input(&mut m, 101, "X", vec![A]);
        input(&mut m, 102, "Y", vec![A, B]);
        input(&mut m, 103, "Z", vec![B]);
        input(&mut m, 104, "K", vec![B]);
        let mut put = |m: &mut Model, id: u32, pairs: Vec<(CategoryId, ItemId)>| {
            if let Some(x) = cell() {
                m.set_input(
                    MeasureId(id),
                    Coordinate::from_pairs(pairs),
                    Value::Number(x),
                );
            }
        };
        for &i in &a {
            put(&mut m, 100, vec![(A, i)]);
            put(&mut m, 101, vec![(A, i)]);
            for &j in &b {
                put(&mut m, 102, vec![(A, i), (B, j)]);
            }
        }
        for &j in &b {
            put(&mut m, 103, vec![(B, j)]);
            put(&mut m, 104, vec![(B, j)]);
        }
        assert!(define(&mut m, 150, "D = Y * 2 - 3"));
        m
    }

    /// `T = text` both ways, on `m`: `None` if it does not compile.
    fn both(mut m: Model, text: &str) -> Option<(MeasureValues, MeasureValues)> {
        if !define(&mut m, T.0, &format!("T = {text}")) {
            return None;
        }
        let plans = compile_plans(&m, &[T]).unwrap();
        let ids: Vec<MeasureId> = plans.iter().map(|(id, _)| *id).collect();
        let mut dd = evaluate(&m, &ids).unwrap();
        let mut env: HashMap<MeasureId, Arc<MeasureValues>> = HashMap::new();
        for ((mid, c), v) in &m.inputs {
            if let Some(cv) = crate::session::input_cell(ValueType::Number, v) {
                Arc::make_mut(env.entry(*mid).or_default()).insert(crate::encode_coord(c), cv);
            }
        }
        for (id, vals) in &dd {
            if *id != T {
                env.insert(*id, Arc::new(vals.clone()));
            }
        }
        let (_, plan) = plans.last().unwrap();
        let has = |id: MeasureId| env.contains_key(&id) || ids.contains(&id) || id.0 < 200;
        assert!(covers(plan, R, &has), "{text}");
        let x = env.get(&R).map(|a| (**a).clone()).unwrap_or_default();
        let host = Host::new(plan, R, env).step(&x);
        Some((host, dd.remove(&T).unwrap_or_default()))
    }

    fn agree(m: Model, text: &str) -> bool {
        let Some((host, dd)) = both(m, text) else {
            return false;
        };
        assert_eq!(host, dd, "{text}");
        true
    }

    /// The plan kinds the host takes, each in a formula the dataflow runs
    /// the same way; every one compiles.
    const CASES: &[&str] = &[
        "R",
        "R * 2 + X",
        "2 * R",
        "R - 1",
        "1 + 2",
        "R + Y",
        "-R",
        "ABS(R - X)",
        "ROUND(R / 3, 2)",
        "MAX(R, X)",
        "MAX(R, Y, 1)",
        "PI()",
        "SUM(R OVER A)",
        "SUMPRODUCT(R, Y OVER A)",
        "SUMPRODUCT(R, X OVER A)",
        "SUMPRODUCT(R, D OVER A)",
        "MAX(Y OVER B)",
        "FIRST(Y OVER A)",
        "RANK(R OVER A)",
        "SUM(Y OVER A WHERE Y > 2)",
        "SUMPRODUCT(R, Y OVER A WHERE R > 0.3)",
        "SUM(R OVER A[FIRST..THIS])",
        "LARGE(Y OVER A, 2)",
        "LARGE(Y OVER A, K)",
        "SORT(R OVER A, -1 INTO B)",
        "FILL(SUMPRODUCT(R, Y OVER A), 0)",
        "RELABEL(FILL(SUMPRODUCT(R, Y OVER A), 0), B, A)",
        "RELABEL(R, A, B)",
        "COALESCE(R, X, 0)",
        "COALESCE(Y, R)",
        "IF(R > 1, R, X)",
        "IF(R > 1, Y)",
        "IF(Y > 1, R, Y)",
        "IF(R > 1, Y, R)",
        "IF(R / 0 > 1, 1, 2)",
        "SEQUENCE(A, 1, 0.5) * R",
        "POSITION(A) + R",
        "R / @ITEMCOUNT(A) + 1",
        "(1 - 0.85) / @ITEMCOUNT(A) + 0.85 * RELABEL(FILL(SUMPRODUCT(R, D OVER A), 0) \
         + SUMPRODUCT(R, X OVER A) / @ITEMCOUNT(A), B, A)",
    ];

    #[test]
    fn every_case_agrees_with_the_dataflow() {
        let mut seed = 1u64;
        let mut cell = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let r = (seed >> 33) % 10;
            (r != 0).then(|| r as f64 / 2.0 - 1.5)
        };
        let m = model(4, 1, &mut cell);
        let bad: Vec<&str> = CASES
            .iter()
            .copied()
            .filter(|text| !agree(m.clone(), text))
            .collect();
        assert!(bad.is_empty(), "do not compile: {bad:?}");
    }

    /// The measures and parts a body is built from.
    const LEAVES: &[&str] = &[
        "R",
        "X",
        "Y",
        "Z",
        "D",
        "2",
        "0.5",
        "0",
        "SUM(R OVER A)",
        "SUMPRODUCT(R, Y OVER A)",
        "SUMPRODUCT(R, X OVER A)",
        "MAX(Y OVER B)",
        "FIRST(Y OVER A)",
        "RANK(R OVER A)",
        "SUM(Y OVER A WHERE Y > 2)",
        "SUMPRODUCT(R, Y OVER A WHERE R > 0.3)",
        "SUM(R OVER A[FIRST..THIS])",
        "LARGE(Y OVER A, K)",
        "SEQUENCE(A, 1, 0.5)",
        "@ITEMCOUNT(A)",
    ];

    fn expr(tc: &TestCase, depth: u32) -> String {
        let pick =
            tc.draw(
                gs::integers::<u32>()
                    .min_value(0)
                    .max_value(if depth == 0 { 0 } else { 11 }),
            );
        let sub = |tc: &TestCase| expr(tc, depth - 1);
        match pick {
            0 => tc.draw(gs::sampled_from(LEAVES.to_vec())).to_string(),
            1..=4 => {
                let op = tc.draw(gs::sampled_from(vec!["+", "-", "*", "/", ">"]));
                format!("({} {op} {})", sub(tc), sub(tc))
            }
            5 => format!("ABS({})", sub(tc)),
            6 => format!("MAX({}, {})", sub(tc), sub(tc)),
            7 => format!("IF({} > 1, {}, {})", sub(tc), sub(tc), sub(tc)),
            8 => format!("FILL({}, 0)", sub(tc)),
            9 => format!("COALESCE({}, {})", sub(tc), sub(tc)),
            10 => format!("RELABEL({}, B, A)", sub(tc)),
            _ => format!("RELABEL({}, A, B)", sub(tc)),
        }
    }

    /// Random bodies on random models: whatever compiles, the host step
    /// gives the dataflow's cells, bit for bit.
    #[hegel::test(test_cases = 300, suppress_health_check = HealthCheck::all())]
    fn random_bodies_agree_with_the_dataflow(tc: TestCase) {
        let n = tc.draw(gs::integers::<usize>().min_value(1).max_value(4));
        let extra = tc.draw(gs::integers::<usize>().min_value(0).max_value(1));
        let vals: Vec<Option<i32>> = tc.draw(
            gs::vecs(gs::optional(
                gs::integers::<i32>().min_value(-4).max_value(8),
            ))
            .min_size(64)
            .max_size(64),
        );
        let mut it = vals.into_iter().cycle();
        let mut cell = move || it.next().flatten().map(|v| f64::from(v) / 2.0);
        let m = model(n, extra, &mut cell);
        let text = expr(&tc, 3);
        tc.assume(agree(m, &text));
    }

    /// What the host does not take: PREV (a Shift), a recurrence, a
    /// measure the dataflow has no collection for.
    #[test]
    fn covers_leaves_shifts_and_unknown_measures_to_the_dataflow() {
        let mut m = model(3, 0, &mut || Some(1.0));
        assert!(define(&mut m, 201, "S = PREV(R, A, 1, 0)"));
        assert!(define(&mut m, 202, "U = R + 1"));
        let plans = compile_plans(&m, &[MeasureId(201), MeasureId(202)]).unwrap();
        let plan = |id: u32| &plans.iter().find(|(m, _)| m.0 == id).unwrap().1;
        let all = |_: MeasureId| true;
        assert!(!covers(plan(201), R, &all));
        assert!(covers(plan(202), R, &all));
        assert!(!covers(plan(202), MeasureId(7), &|m| m != R));
        let mut seen = HashSet::new();
        reads(plan(202), &mut seen);
        assert_eq!(seen, HashSet::from([R]));
    }
}
