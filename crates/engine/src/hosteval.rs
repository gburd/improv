//! Host-side evaluation of a FIXPOINT's step.
//!
//! A FIXPOINT's step, `body(r)`, is a compiled plan like any measure's. Fed
//! through the dataflow, a round re-joins every edge an iterate cell touches,
//! and in PageRank every cell moves every round: a round is a full pass at
//! dataflow cost (arrangement merges, history replay, allocation). The worker
//! instead evaluates the same plan in memory. The parts that do not read r,
//! and the join indexes over them, are evaluated once per solve; a subtree
//! the plan repeats is evaluated once per round. A reduction lays its rows
//! out once (which cell each row value reads, in the order its kernel sees
//! them); while its collections keep their coordinates, which an iterate
//! does from round to round, a round only refills the values and calls the
//! kernel.
//!
//! Only the plan's structure lives here: alignment, a reduction's rows,
//! RELABEL, FILL and COALESCE, IF's branches. Every cell operation is the
//! dataflow builder's own (`apply_unary`, `apply_binary`, `call_with`, the
//! `reduce` kernels), and a reduction sees its rows in the order the
//! dataflow's `reduce` sorts them, so a round gives the dataflow's values bit
//! for bit. [`covers`] says which plans it takes; the rest (`PREV`,
//! recurrences) stay on the dataflow.

use crate::builtins::err;
use crate::dataflow::{
    agg_key, agg_order, agg_rows, apply_binary, apply_unary, call_with, grid_keys, literal_value,
    project, remap_key, remap_table, shared_cats, union_keys, where_row, windows, AggRow, AggSpec,
};
use crate::plan::{PlanNode, PlanNodeKind as K};
use crate::reduce::{self, two_level, Rows};
use crate::session::MeasureValues;
use crate::{CellValue, CoordKey};
use improv_core_model::{CategoryId, FuncId, ItemId, MeasureId};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

/// A collection: a multiset of cells (a cell may repeat, as a dataflow
/// collection's multiplicity).
type Cells = Vec<(CoordKey, CellValue)>;
/// The positions of a collection's cells by their projection onto join
/// categories.
type Index = HashMap<CoordKey, Vec<u32>>;
/// A plan node, by address (only compared: the plan is shared and never
/// changes, so its nodes do not move).
type Node = *const PlanNode;
/// A reduction row in layout: (item positions, item), value sources.
type SrcRow = AggRow<Src>;

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

/// `n` and the nodes under it, parents first.
fn preorder<'a>(n: &'a PlanNode, out: &mut Vec<&'a PlanNode>) {
    out.push(n);
    for k in kids(n) {
        preorder(k, out);
    }
}

/// Where a reduction row's value comes from: `Src(j, i)` is cell `i` of the
/// reduction's `j`-th collection, or, for `j` = [`OWN`], value `i` of the
/// reduction's own values (a `WHERE` condition's error). Eight bytes: a row
/// value per edge of a graph is held for the solve.
#[derive(Clone, Copy, Debug)]
struct Src(u32, u32);

/// [`Src`]'s collection index for a reduction's own values.
const OWN: u32 = u32::MAX;

fn value(kids: &[Rc<Cells>], own: &[CellValue], s: &Src) -> CellValue {
    match s.0 {
        OWN => own[s.1 as usize].clone(),
        j => kids[j as usize][s.1 as usize].1.clone(),
    }
}

/// A reduction's rows laid out on its collections' coordinates: which cell
/// each row value reads, per group, in the order the kernel sees them.
struct Shape {
    /// The collections it was laid out on (or last refilled from).
    kids: Vec<Rc<Cells>>,
    /// The values its [`OWN`] sources read.
    own: Vec<CellValue>,
    groups: Vec<Group>,
    /// One group's rows as the kernel takes them, refilled group by group
    /// (its row vectors kept, so a round allocates nothing).
    scratch: Rows,
}

/// One group of a [`Shape`], flat: a `Rows` held per group had two
/// vectors per row, 48 bytes of headers and two allocations per edge of a
/// graph. Per row: its item positions (`dims` each), its item, its value
/// sources (`width` each: the inputs', then the joined parameters') and its
/// input values (`n_in` each, refilled from the collections that changed).
struct Group {
    key: CoordKey,
    pos: Vec<u32>,
    items: Vec<u32>,
    srcs: Vec<Src>,
    vals: Vec<CellValue>,
    width: usize,
}

/// `rows` sized to `n`, keeping the row vectors it drops for later.
fn resize<T>(rows: &mut Vec<Vec<T>>, n: usize) {
    if rows.len() > n {
        rows.truncate(n);
    } else {
        rows.resize_with(n, Vec::new);
    }
}

impl Shape {
    /// Whether `kids` have the coordinates, in order, it was laid out on
    /// (and the `WHERE` condition, at `pred`, its values too).
    fn fits(&self, kids: &[Rc<Cells>], pred: Option<usize>) -> bool {
        let same_keys =
            |a: &Cells, b: &Cells| a.len() == b.len() && a.iter().zip(b).all(|(c, d)| c.0 == d.0);
        self.kids.len() == kids.len()
            && self.kids.iter().zip(kids).enumerate().all(|(j, (a, b))| {
                Rc::ptr_eq(a, b)
                    || if pred == Some(j) {
                        a == b
                    } else {
                        same_keys(a, b)
                    }
            })
    }

    /// Refill every group's rows from `kids` (only the values read from a
    /// collection `stale` marks) and reduce it into `out`.
    fn fill(
        &mut self,
        a: &AggSpec<'_>,
        consts: &[Option<CellValue>],
        stale: &[bool],
        out: &mut Cells,
    ) {
        let (n_in, dims, cells) = (a.ins.len(), a.order.len(), into_cells(a));
        let (kids, own, rs) = (&self.kids, &self.own, &mut self.scratch);
        for g in &mut self.groups {
            let srcs = g.srcs.chunks(g.width);
            for (row, srcs) in g.vals.chunks_mut(n_in.max(1)).zip(srcs) {
                for (v, s) in row.iter_mut().zip(srcs) {
                    if s.0 != OWN && stale[s.0 as usize] {
                        *v = value(kids, own, s);
                    }
                }
            }
            let n = g.items.len();
            resize(&mut rs.pos, n);
            resize(&mut rs.vals, n);
            let pos = g.pos.chunks(dims.max(1));
            let vals = g.vals.chunks(n_in.max(1));
            for ((p, v), (gp, gv)) in rs.pos.iter_mut().zip(&mut rs.vals).zip(pos.zip(vals)) {
                p.clear();
                p.extend_from_slice(&gp[..dims]);
                v.clear();
                v.extend_from_slice(&gv[..n_in]);
            }
            // The joined parameters are the first row's (see `agg_rows`).
            let mut joined = g.srcs[n_in..g.width].iter();
            let params: Vec<CellValue> = consts
                .iter()
                .map(|c| match c {
                    Some(v) => v.clone(),
                    None => joined
                        .next()
                        .map_or(CellValue::Err(err::NA), |s| value(kids, own, s)),
                })
                .collect();
            emit(a, cells.as_deref(), &g.key, rs, &params, &g.items, out);
        }
    }
}

/// An array reduction's landing cells (see the dataflow's `build_aggregate`).
fn into_cells(a: &AggSpec<'_>) -> Option<Vec<CoordKey>> {
    (!a.into.is_empty()).then(|| grid_keys(a.into))
}

/// Group `g`'s results, as the dataflow's `build_aggregate` gives them.
fn emit(
    a: &AggSpec<'_>,
    cells: Option<&[CoordKey]>,
    g: &CoordKey,
    rs: &Rows,
    params: &[CellValue],
    items: &[u32],
    out: &mut Cells,
) {
    if let Some(cells) = cells {
        for (cell, v) in cells.iter().zip(reduce::array(a.func, rs, params)) {
            let mut k = g.clone();
            k.extend(cell.iter().copied());
            k.sort_unstable();
            out.push((k, v));
        }
    } else if a.window.is_none() && !a.per_item {
        let v = if a.two_level() {
            two_level::reduce(a.func, rs, items)
        } else {
            reduce::reduce(a.func, rs, params)
        };
        out.push((g.clone(), v));
    } else {
        let outs = match a.window {
            None => reduce::per_item(a.func, rs, params),
            Some(w) => windows(a.func, rs, params, w),
        };
        let cat0 = a.order.first().map_or(0, |(c, _)| c.0);
        for (item, v) in items.iter().zip(outs) {
            let mut k = g.clone();
            k.push((cat0, *item));
            k.sort_unstable();
            out.push((k, v));
        }
    }
}

/// A reduction's rows by group, groups in key order, each group's rows in
/// item order.
fn group_rows(a: &AggSpec<'_>, rows: Vec<(CoordKey, Vec<Src>)>) -> Vec<(CoordKey, Vec<SrcRow>)> {
    let order = agg_order(a.order);
    let gb: Vec<u32> = a.group_by.iter().map(|c| c.0).collect();
    let mut groups: HashMap<CoordKey, Vec<SrcRow>> = HashMap::new();
    for (k, srcs) in rows {
        let (g, row) = agg_key(&k, srcs, &order, &gb);
        groups.entry(g).or_default().push(row);
    }
    let mut groups: Vec<_> = groups.into_iter().collect();
    groups.sort_unstable_by(|x, y| x.0.cmp(&y.0));
    for (_, rows) in &mut groups {
        rows.sort_by(|x, y| x.0.cmp(&y.0));
    }
    groups
}

/// A FIXPOINT step's plan, evaluated round after round at one iterate after
/// another, with the measures it reads (but r) fixed between refreshes.
pub(crate) struct Host {
    plan: Rc<PlanNode>,
    r: MeasureId,
    env: HashMap<MeasureId, Arc<MeasureValues>>,
    /// Each node's first structurally equal node, by which it is cached.
    canon: HashMap<Node, Node>,
    /// The nodes that read r, evaluated every round.
    live: HashSet<Node>,
    /// The other nodes' cells, evaluated once per refresh.
    fixed: HashMap<Node, Rc<Cells>>,
    /// Join indexes over the other nodes, by node and join categories.
    index: HashMap<(Node, Vec<u32>), Rc<Index>>,
    /// This round's cells of the nodes that read r.
    round: HashMap<Node, Rc<Cells>>,
    /// Each reduction's row layout.
    shapes: HashMap<Node, Shape>,
    /// r's coordinates, in the order a round reads them.
    r_keys: Vec<CoordKey>,
    /// Each other measure's cells as last read, whose order a refresh
    /// keeps while the coordinates stay: an edit then leaves the layouts
    /// fitting (a map's iteration order moves when cells are re-set).
    env_cells: HashMap<MeasureId, Rc<Cells>>,
}

impl Host {
    /// `plan` (covered: [`covers`]) reading the iterate as `r`, and no other
    /// measure's cells until [`Host::refresh`].
    pub(crate) fn new(plan: PlanNode, r: MeasureId) -> Self {
        let plan = Rc::new(plan);
        let mut canon = HashMap::new();
        let mut all = Vec::new();
        preorder(&plan, &mut all);
        for (i, &n) in all.iter().enumerate() {
            let first = all[..i].iter().copied().find(|&m| m == n).unwrap_or(n);
            canon.insert(n as Node, first as Node);
        }
        let mut live = HashSet::new();
        mark(&plan, r, &mut live);
        Host {
            plan,
            r,
            env: HashMap::new(),
            canon,
            live,
            fixed: HashMap::new(),
            index: HashMap::new(),
            round: HashMap::new(),
            shapes: HashMap::new(),
            r_keys: Vec::new(),
            env_cells: HashMap::new(),
        }
    }

    /// Let go of the measures [`Host::refresh`] read, keeping the cells
    /// taken from them: while it holds a snapshot's map, the worker's next
    /// change to that measure copies all of it (copy-on-write). The next
    /// refresh then evaluates afresh.
    pub(crate) fn release(&mut self) {
        self.env.clear();
        self.fixed.clear();
        self.index.clear();
    }

    /// Read every measure but r from `env` (one it lacks has no cells).
    /// The row layouts stay: they are checked against the cells each round.
    pub(crate) fn refresh(&mut self, env: HashMap<MeasureId, Arc<MeasureValues>>) {
        let same = env.len() == self.env.len()
            && env
                .iter()
                .all(|(m, v)| self.env.get(m).is_some_and(|w| Arc::ptr_eq(v, w)));
        if !same {
            self.fixed.clear();
            self.index.clear();
        }
        self.env = env;
    }

    /// The step at iterate `x`: one value per cell (of several, the
    /// greatest, as the engine's snapshot keeps the last of a sorted batch).
    pub(crate) fn step(&mut self, x: &MeasureValues) -> MeasureValues {
        self.round.clear();
        let plan = Rc::clone(&self.plan);
        let cells = self.eval(&plan, x);
        self.round.clear();
        let mut out = MeasureValues::with_capacity(cells.len());
        for (k, v) in cells.iter() {
            match out.get(k) {
                Some(w) if w >= v => {}
                _ => {
                    out.insert(k.clone(), v.clone());
                }
            }
        }
        out
    }

    fn node(&self, n: &PlanNode) -> Node {
        self.canon[&(n as Node)]
    }

    fn eval(&mut self, n: &PlanNode, x: &MeasureValues) -> Rc<Cells> {
        let p = self.node(n);
        let live = self.live.contains(&p);
        let memo = if live { &self.round } else { &self.fixed };
        if let Some(c) = memo.get(&p) {
            return Rc::clone(c);
        }
        let c = Rc::new(self.compute(n, x));
        if let K::InputMeasure(m) = n.kind {
            if m != self.r {
                self.env_cells.insert(m, Rc::clone(&c));
            }
        }
        let memo = if live {
            &mut self.round
        } else {
            &mut self.fixed
        };
        memo.insert(p, Rc::clone(&c));
        c
    }

    fn compute(&mut self, n: &PlanNode, x: &MeasureValues) -> Cells {
        match &n.kind {
            K::InputMeasure(m) if *m == self.r => {
                // r in a fixed coordinate order, so the layouts fit.
                let same =
                    self.r_keys.len() == x.len() && self.r_keys.iter().all(|k| x.contains_key(k));
                if !same {
                    self.r_keys = x.keys().cloned().collect();
                    self.r_keys.sort_unstable();
                }
                self.r_keys
                    .iter()
                    .map(|k| (k.clone(), x[k].clone()))
                    .collect()
            }
            K::InputMeasure(m) => {
                let Some(v) = self.env.get(m) else {
                    return Vec::new();
                };
                let old = self.env_cells.get(m).filter(|o| o.len() == v.len());
                let kept: Option<Cells> = old.and_then(|o| {
                    o.iter()
                        .map(|(k, _)| Some((k.clone(), v.get(k)?.clone())))
                        .collect()
                });
                kept.unwrap_or_else(|| v.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            }
            K::Literal(v) => {
                let v = CellValue::from_model_value(v).expect("covers checks literals");
                vec![(CoordKey::new(), v)]
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
                    (Some(l), Some(r)) => vec![(CoordKey::new(), apply_binary(op, &l, &r))],
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
                flat,
            } => self.aggregate(
                n,
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
                x,
            ),
            K::Items(cat, vals) => vals
                .iter()
                .filter_map(|(i, v)| {
                    Some((
                        smallvec::smallvec![(cat.0, i.0)],
                        CellValue::from_model_value(v)?,
                    ))
                })
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

    /// The dataflow's `align_with`, by position: every pair of a row of `l`
    /// and a cell `i` of node `n` (its cells `c`) that agree on `keys`, at
    /// the union coordinate. A node that does not read r keeps its index.
    fn align_idx<V: Clone>(
        &mut self,
        l: &[(CoordKey, V)],
        n: &PlanNode,
        c: &Cells,
        keys: &[CategoryId],
    ) -> Vec<(CoordKey, (V, u32))> {
        let keys: Vec<u32> = keys.iter().map(|c| c.0).collect();
        let p = self.node(n);
        let idx = if self.live.contains(&p) {
            Rc::new(index(c, &keys))
        } else {
            let e = self.index.entry((p, keys.clone()));
            Rc::clone(e.or_insert_with(|| Rc::new(index(c, &keys))))
        };
        let mut out = Vec::new();
        for (lk, lv) in l {
            if let Some(rs) = idx.get(&project(lk, &keys)) {
                for &i in rs {
                    out.push((union_keys(lk, &c[i as usize].0), (lv.clone(), i)));
                }
            }
        }
        out
    }

    /// [`Host::align_idx`] with the cells' values.
    fn align<V: Clone>(
        &mut self,
        l: &[(CoordKey, V)],
        n: &PlanNode,
        c: &Cells,
        keys: &[CategoryId],
    ) -> Vec<(CoordKey, (V, CellValue))> {
        self.align_idx(l, n, c, keys)
            .into_iter()
            .map(|(k, (v, i))| (k, (v, c[i as usize].1.clone())))
            .collect()
    }

    /// The dataflow's `build_aggregate`. Its rows are laid out once and
    /// refilled while its collections keep their coordinates; a group with
    /// two rows at one item (which the kernel sees in value order) is
    /// sorted by value every round.
    fn aggregate(&mut self, n: &PlanNode, a: AggSpec<'_>, x: &MeasureValues) -> Cells {
        let joined = a.params.iter().filter(|q| literal_value(q).is_none());
        let nodes: Vec<&PlanNode> = a.ins.iter().chain(a.pred).chain(joined).collect();
        let kids: Vec<Rc<Cells>> = nodes.iter().map(|q| self.eval(q, x)).collect();
        let consts: Vec<Option<CellValue>> = a.params.iter().map(literal_value).collect();
        let (p, pred) = (self.node(n), a.pred.map(|_| a.ins.len()));
        let mut out = Vec::new();
        if let Some(s) = self.shapes.get_mut(&p).filter(|s| s.fits(&kids, pred)) {
            let stale: Vec<bool> = s
                .kids
                .iter()
                .zip(&kids)
                .map(|(a, b)| !Rc::ptr_eq(a, b))
                .collect();
            s.kids = kids;
            s.fill(&a, &consts, &stale, &mut out);
            return out;
        }
        let (rows, own) = self.tagged_rows(&a, &nodes, &kids);
        let groups = group_rows(&a, rows);
        let ties = groups
            .iter()
            .any(|(_, rows)| rows.windows(2).any(|w| w[0].0 == w[1].0));
        let n_in = a.ins.len();
        let capacity: usize = a.order.iter().map(|(_, items)| items.len()).product();
        let dims = a.order.len();
        if ties {
            let cells = into_cells(&a);
            for (g, rows) in groups {
                let mut vals: Vec<AggRow> = rows
                    .into_iter()
                    .map(|(pi, srcs)| (pi, srcs.iter().map(|s| value(&kids, &own, s)).collect()))
                    .collect();
                vals.sort();
                let mut input: Vec<(&AggRow, isize)> = Vec::new();
                for row in &vals {
                    match input.last_mut() {
                        Some((r, m)) if *r == row => *m += 1,
                        _ => input.push((row, 1)),
                    }
                }
                let (rs, params, items) = agg_rows(&input, n_in, &consts, capacity);
                emit(&a, cells.as_deref(), &g, &rs, &params, &items, &mut out);
            }
            return out;
        }
        let groups = groups
            .into_iter()
            .map(|(key, rows)| {
                let width = rows.first().map_or(n_in, |r| r.1.len());
                let mut g = Group {
                    key,
                    pos: Vec::with_capacity(rows.len() * dims),
                    items: Vec::with_capacity(rows.len()),
                    srcs: Vec::with_capacity(rows.len() * width),
                    vals: Vec::with_capacity(rows.len() * n_in),
                    width,
                };
                for ((pos, item), srcs) in rows {
                    debug_assert_eq!(pos.len(), dims);
                    g.pos.extend(pos);
                    g.vals
                        .extend(srcs[..n_in].iter().map(|s| value(&kids, &own, s)));
                    g.items.push(item);
                    g.srcs.extend(srcs);
                }
                g
            })
            .collect();
        let scratch = Rows {
            pos: Vec::new(),
            vals: Vec::new(),
            capacity,
        };
        let mut s = Shape {
            kids,
            own,
            groups,
            scratch,
        };
        let fresh = vec![false; s.kids.len()];
        s.fill(&a, &consts, &fresh, &mut out);
        self.shapes.insert(p, s);
        out
    }

    /// A reduction's rows, as the dataflow builds them, with each value as
    /// its source: `nodes` are the inputs, the condition and the joined
    /// parameters, `kids` their cells. Also the values its [`OWN`] sources
    /// read.
    #[allow(clippy::type_complexity)]
    fn tagged_rows(
        &mut self,
        a: &AggSpec<'_>,
        nodes: &[&PlanNode],
        kids: &[Rc<Cells>],
    ) -> (Vec<(CoordKey, Vec<Src>)>, Vec<CellValue>) {
        let with_order = |n: &PlanNode| -> Vec<CategoryId> {
            let order = a.order.iter().map(|(c, _)| *c);
            n.ty.dim.categories.iter().copied().chain(order).collect()
        };
        let n_in = a.ins.len();
        let mut full = with_order(nodes[0]);
        let mut rows: Vec<(CoordKey, Vec<Src>)> = kids[0]
            .iter()
            .enumerate()
            .map(|(i, (k, _))| (k.clone(), vec![Src(0, i as u32)]))
            .collect();
        let push = |j: usize, rows: Vec<(CoordKey, (Vec<Src>, u32))>| {
            rows.into_iter()
                .map(|(k, (mut vs, i))| {
                    vs.push(Src(j as u32, i));
                    (k, vs)
                })
                .collect()
        };
        for j in 1..n_in {
            let mine = with_order(nodes[j]);
            let shared: Vec<CategoryId> =
                mine.iter().copied().filter(|c| full.contains(c)).collect();
            let more: Vec<CategoryId> = mine.into_iter().filter(|c| !full.contains(c)).collect();
            full.extend(more);
            rows = push(j, self.align_idx(&rows, nodes[j], &kids[j], &shared));
        }
        let mut j = n_in;
        let mut own = Vec::new();
        if let Some(p) = a.pred {
            let pc = &kids[j];
            let mut kept = Vec::new();
            for (k, (vs, i)) in self.align_idx(&rows, p, pc, &p.ty.dim.categories) {
                let t = pc[i as usize].1.clone();
                let at = Src(OWN, own.len() as u32);
                if let Some(vs) = where_row(vs, t.clone(), |_| at) {
                    if matches!(t, CellValue::Err(_)) {
                        own.push(t);
                    }
                    kept.push((k, vs));
                }
            }
            rows = kept;
            j += 1;
        }
        let mut row_dim = full;
        for (q, c) in nodes[j..].iter().zip(&kids[j..]) {
            let cats = &q.ty.dim.categories;
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
            rows = push(j, self.align_idx(&rows, q, c, &shared));
            j += 1;
        }
        (rows, own)
    }

    /// The dataflow's Coalesce: at each coordinate of the domain, the first
    /// argument's cell (in coordinate order).
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
        let mut out: Cells = best.into_iter().map(|(k, (_, v))| (k, v)).collect();
        out.sort_unstable();
        out
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
            return vec![(CoordKey::new(), call_with(func, &template, &[]))];
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
            acc = self
                .align(&acc, a, &c, &keys)
                .into_iter()
                .map(|(k, (mut vs, v))| {
                    vs.push(v);
                    (k, vs)
                })
                .collect();
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

/// The positions of `c`'s cells by their projection onto `keys`.
fn index(c: &Cells, keys: &[u32]) -> Index {
    let mut idx: Index = HashMap::new();
    for (i, (k, _)) in c.iter().enumerate() {
        idx.entry(project(k, keys)).or_default().push(i as u32);
    }
    idx
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
        let mut h = Host::new(plan.clone(), R);
        h.refresh(env);
        let host = h.step(&x);
        // A second round on the laid-out rows gives the same.
        assert_eq!(h.step(&x), host, "{text}: the refilled round");
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
        "LARGE(X OVER A, K)",
        "LARGE(R OVER A, K)",
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

    /// Hand-built `plan` as `T` over input `cells`, through the dataflow.
    fn run_dd(plan: PlanNode, cells: Vec<(MeasureId, MeasureValues)>) -> MeasureValues {
        let out = Arc::new(std::sync::Mutex::new(MeasureValues::new()));
        let out2 = Arc::clone(&out);
        timely::execute::execute_directly(move |worker| {
            let mut sessions = Vec::new();
            worker.dataflow::<u64, _, _>(|scope| {
                let mut inputs = HashMap::new();
                for (mid, _) in &cells {
                    let mut s = differential_dataflow::input::InputSession::new();
                    inputs.insert(*mid, s.to_collection(scope));
                    sessions.push(s);
                }
                for (_, coll) in crate::dataflow::build_derived(scope, &[(T, plan)], &inputs) {
                    let out = Arc::clone(&out2);
                    coll.inspect(move |((k, v), _, d)| {
                        if *d > 0 {
                            out.lock().unwrap().insert(k.clone(), v.clone());
                        }
                    });
                }
            });
            for ((_, cs), s) in cells.iter().zip(&mut sessions) {
                for (k, v) in cs {
                    s.insert((k.clone(), v.clone()));
                }
                s.advance_to(1);
                s.flush();
            }
        });
        let got = out.lock().unwrap().clone();
        got
    }

    /// Rows that share their item positions (R·Y over A × B reduced over A
    /// alone): the kernel sees them in value order, so the host sorts such
    /// groups every round, and gives the dataflow's cells.
    #[test]
    fn rows_at_one_item_reach_the_kernel_in_value_order() {
        use crate::typed::{Dim, TypeInfo};
        let ty = |cats: Vec<CategoryId>| TypeInfo {
            value_type: ValueType::Number,
            dim: Dim::of(cats),
        };
        let leaf = |m: MeasureId, cats: Vec<CategoryId>| PlanNode {
            kind: K::InputMeasure(m),
            ty: ty(cats),
        };
        let y = MeasureId(102);
        let wide = PlanNode {
            kind: K::Join {
                left: Box::new(leaf(R, vec![A])),
                right: Box::new(leaf(y, vec![A, B])),
                join_keys: vec![A],
            },
            ty: ty(vec![A, B]),
        };
        let product = PlanNode {
            kind: K::MapBinary(
                improv_core_model::BinaryOp::Mul,
                Box::new(wide),
                Box::new(leaf(y, vec![A, B])),
            ),
            ty: ty(vec![A, B]),
        };
        let items: Vec<ItemId> = (0..3).map(|i| ItemId(10 + i)).collect();
        let num = |x: f64| CellValue::num(x);
        let mut r = MeasureValues::new();
        let mut yv = MeasureValues::new();
        for (i, a) in items.iter().enumerate() {
            r.insert(CoordKey::from_slice(&[(A.0, a.0)]), num(i as f64 - 1.0));
            for j in 0..3u32 {
                // Equal values at one item too: a row with multiplicity 2.
                let v = f64::from((j * 7 + i as u32 * 3) % 4);
                yv.insert(CoordKey::from_slice(&[(A.0, a.0), (B.0, 30 + j)]), num(v));
            }
        }
        for name in ["FIRST", "LAST", "SUM", "TEXTJOIN"] {
            let def = improv_core_model::functions::by_name(name).next().unwrap();
            let params = if name == "TEXTJOIN" {
                let lit = |v: Value| PlanNode {
                    kind: K::Literal(v),
                    ty: ty(vec![]),
                };
                vec![lit(Value::Text("-".into())), lit(Value::Boolean(true))]
            } else {
                vec![]
            };
            let agg = PlanNode {
                kind: K::Aggregate {
                    inputs: vec![product.clone()],
                    params,
                    pred: None,
                    group_by: vec![],
                    func: def.id,
                    order: vec![(A, items.clone())],
                    window: None,
                    per_item: false,
                    into: vec![],
                    flat: false,
                },
                ty: ty(vec![]),
            };
            let want = run_dd(agg.clone(), vec![(R, r.clone()), (y, yv.clone())]);
            let mut h = Host::new(agg, R);
            h.refresh(HashMap::from([(y, Arc::new(yv.clone()))]));
            assert_eq!(h.step(&r), want, "{name}");
            assert_eq!(h.step(&r), want, "{name}, again");
            assert!(h.shapes.is_empty(), "{name}: tied rows are never laid out");
        }
    }
}
