//! Live incremental edit API.
//!
//! `evaluate()` runs the whole model once. `Engine` instead builds the
//! differential-dataflow graph **once** and keeps it alive on a dedicated
//! worker thread, so subsequent input-cell edits propagate as *deltas* —
//! only affected coordinates (and their downstream measures) recompute. This
//! is the "edits → deltas → recompute without full rebuild" API the engine
//! steering calls for (see AGENT_ENGINE_STEERING §6.2).
//!
//! Scope: the measure *structure* (which measures exist, their formulas and
//! dimensions) is fixed at `Engine::new` time. Editing **input cell values**
//! is incremental. Adding/removing measures or changing a formula requires a
//! new `Engine` (a structural rebuild) — callers rebuild on structural change,
//! which is cheap relative to a data-edit-heavy session.
//!
//! Threading: differential dataflow's worker and `InputSession`s are not
//! `Send`-friendly to hold across calls, so the worker lives on its own thread
//! and communicates over channels. `Engine` is the `Send` handle.

use crate::plan::{PlanNode, PlanNodeKind};
use crate::{encode_coord, CellValue, CoordKey};
use improv_core_model::{MeasureId, MeasureKind, Model, Value, ValueType};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::dataflow::{build_derived, compile_plans, EngineError};
use differential_dataflow::input::InputSession;
use timely::dataflow::operators::probe::Handle as ProbeHandle;

/// A single input-cell edit: set `measure[coord]` to `value` (or remove it when
/// `value` is `None`).
#[derive(Debug, Clone)]
pub struct Edit {
    pub measure: MeasureId,
    pub coord: CoordKey,
    pub value: Option<CellValue>,
}

/// The engine cell an input of type `ty` holding `val` contributes: `None`
/// for a value of another type (text in a number measure, an error), which is
/// no cell. One rule for the seed and for callers' edits, so an edit and a
/// rebuild always agree.
pub fn input_cell(ty: ValueType, val: &Value) -> Option<CellValue> {
    let typed = matches!(
        (ty, val),
        (ValueType::Number, Value::Number(_))
            | (ValueType::Boolean, Value::Boolean(_))
            | (ValueType::Text, Value::Text(_))
            | (ValueType::DateTime, Value::DateTime(_))
    );
    CellValue::from_model_value(val).filter(|_| typed)
}

/// A snapshot of a computed derived measure after an edit round.
pub type MeasureValues = HashMap<CoordKey, CellValue>;

/// Every tracked derived measure's values after an edit round.
///
/// Copy-on-write: cloning a `Snapshot` is a reference-count bump, and the
/// engine shares unchanged measures between successive snapshots. An edit
/// round copies the outer map (one `Arc` per measure) and only the measures
/// whose cells changed, and only if a caller still holds the previous
/// snapshot; drop the old one first and an edit costs just its delta.
// ponytail: a changed measure is copied whole while the previous snapshot is
// alive, so one huge measure still costs O(its cells) per edit for callers
// that keep the old snapshot; a persistent map (`im`) per measure lifts that.
pub type Snapshot = Arc<HashMap<MeasureId, Arc<MeasureValues>>>;

enum Cmd {
    /// Apply a batch of edits, advance time, and return the new full snapshot
    /// of every tracked derived measure.
    Edit(Vec<Edit>, Sender<Snapshot>),
    Shutdown,
}

/// A live, incremental evaluation handle over a fixed model structure.
pub struct Engine {
    tx: Sender<Cmd>,
    worker: Option<JoinHandle<()>>,
    /// The derived measures this engine tracks (its `evaluate` targets).
    targets: Vec<MeasureId>,
    /// Current known value of each input cell, so `set` can emit the correct
    /// retract+assert delta for a cardinality-one cell.
    current: HashMap<(MeasureId, CoordKey), CellValue>,
    /// The compiled plans (a FIXPOINT as its first-iterate and step parts),
    /// to tell whether a model with more items still compiles to the same
    /// graph ([`Engine::serves`]).
    plans: Vec<(MeasureId, PlanNode)>,
}

impl Engine {
    /// Build a live engine for `model`, tracking `targets` (derived measures).
    /// Seeds the graph with the model's current input cells and returns a handle
    /// plus the initial snapshot.
    pub fn new(model: &Model, targets: &[MeasureId]) -> Result<(Engine, Snapshot), EngineError> {
        Self::build(model, targets, true)
    }

    /// [`Engine::new`]; with `host_steps` false every FIXPOINT step is a
    /// dataflow round (the path a body the host evaluator does not cover
    /// takes), so tests can hold the two paths to the same answer.
    fn build(
        model: &Model,
        targets: &[MeasureId],
        host_steps: bool,
    ) -> Result<(Engine, Snapshot), EngineError> {
        // Compile plans up front (outside the worker), so a compile error is
        // reported synchronously rather than on the worker thread.
        let plans = compile_plans(model, targets)?;
        let (plans, mut fixes) = split_fixpoints(model, plans)?;
        if !host_steps {
            fixes.iter_mut().for_each(|f| f.host = None);
        }

        // Which input measures need sessions? Every input measure referenced by
        // any tracked plan, plus any that already has cells. We over-approximate
        // with "all input measures in the model" — extra empty sessions are
        // cheap and keep later edits to any input measure valid.
        let mut input_ids: Vec<MeasureId> = model
            .measures
            .iter()
            .filter(|(_, m)| matches!(m.kind, MeasureKind::Input))
            .map(|(id, _)| *id)
            .collect();
        // A FIXPOINT's iterate and its solution ride on sessions of their own.
        input_ids.extend(fixes.iter().flat_map(|f| [f.r, f.id]));

        // Seed edits: the model's current input cells of their measure's
        // type (a mistyped value, e.g. text in a number measure, is no cell).
        let mut seed: Vec<(MeasureId, CoordKey, CellValue)> = Vec::new();
        let mut current: HashMap<(MeasureId, CoordKey), CellValue> = HashMap::new();
        for ((mid, coord), val) in &model.inputs {
            let ty = model.measures.get(mid).map(|m| m.value_type);
            if let Some(cv) = ty.and_then(|t| input_cell(t, val)) {
                let key = encode_coord(coord);
                seed.push((*mid, key.clone(), cv.clone()));
                current.insert((*mid, key), cv);
            }
        }

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Cmd>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let targets_owned = targets.to_vec();
        let plans_owned = plans.clone();
        let seed_owned = seed;

        let worker = std::thread::spawn(move || {
            worker_loop(input_ids, plans_owned, fixes, seed_owned, cmd_rx, ready_tx);
        });

        // Wait for the worker to build the graph and apply the seed.
        ready_rx
            .recv()
            .map_err(|_| EngineError::Unsupported("engine worker failed to start".into()))?;

        let mut engine = Engine {
            tx: cmd_tx,
            worker: Some(worker),
            targets: targets_owned,
            current,
            plans,
        };

        // Fetch the initial snapshot with an empty edit round.
        let snapshot = engine.apply(Vec::new())?;
        Ok((engine, snapshot))
    }

    /// Set an input cell to `value` and return the recomputed snapshot of all
    /// tracked derived measures. Incremental: only affected coordinates flow.
    pub fn set(
        &mut self,
        measure: MeasureId,
        coord: CoordKey,
        value: f64,
    ) -> Result<Snapshot, EngineError> {
        self.apply(vec![Edit {
            measure,
            coord,
            value: Some(CellValue::num(value)),
        }])
    }

    /// Remove an input cell and return the recomputed snapshot.
    pub fn clear(&mut self, measure: MeasureId, coord: CoordKey) -> Result<Snapshot, EngineError> {
        self.apply(vec![Edit {
            measure,
            coord,
            value: None,
        }])
    }

    /// Apply a batch of edits atomically (one time step) and return the new
    /// snapshot of all tracked derived measures. An edit's coordinate may list
    /// its categories in any order: it is sorted into the engine's key form,
    /// which the key kernels (`improv_verified::keys`) require.
    pub fn apply(&mut self, mut edits: Vec<Edit>) -> Result<Snapshot, EngineError> {
        for e in &mut edits {
            e.coord.sort_unstable();
        }
        // Update our shadow of current input values so retract deltas are exact.
        for e in &edits {
            let k = (e.measure, e.coord.clone());
            match &e.value {
                Some(v) => {
                    self.current.insert(k, v.clone());
                }
                None => {
                    self.current.remove(&k);
                }
            }
        }
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.tx
            .send(Cmd::Edit(edits, reply_tx))
            .map_err(|_| EngineError::Unsupported("engine worker is gone".into()))?;
        reply_rx
            .recv()
            .map_err(|_| EngineError::Unsupported("engine worker dropped the reply".into()))
    }

    /// The derived measures this engine tracks.
    pub fn targets(&self) -> &[MeasureId] {
        &self.targets
    }

    /// Whether this engine computes `model` as a new engine would, so a
    /// model that differs from the one it was built on only in input cells
    /// and in new items can be fed to it as edits. False when a plan reads
    /// the item list of a category that gained items: `POSITION`, `@ITEM`,
    /// `@ITEMCOUNT`, `PREV`/`NEXT`, recurrences, windows, per-item and
    /// `INTO` reductions, `FILL` (its grid), `RELABEL`/`SWAP` (their item
    /// maps), and every
    /// reduction other than `SUM` … `XOR`, `COUNT`, `COUNTA` and
    /// `COUNTUNIQUE` (which read only their rows' values: a new item
    /// reaches them as data, sorted last as an unknown item sorts). New
    /// items must have ids above every item of their category and come
    /// after them in its item order (append-only minting does both).
    pub fn serves(&self, model: &Model) -> Result<bool, EngineError> {
        let (now, _) = split_fixpoints(model, compile_plans(model, &self.targets)?)?;
        Ok(now.len() == self.plans.len()
            && now
                .iter()
                .zip(&self.plans)
                .all(|((a, p), (b, q))| a == b && item_blind(p) == item_blind(q)))
    }
}

/// `p` without the item order of the reductions that read only their rows'
/// values (see [`Engine::serves`]).
fn item_blind(p: &PlanNode) -> PlanNode {
    let mut p = p.clone();
    blind(&mut p);
    p
}

fn blind(p: &mut PlanNode) {
    use crate::plan::PlanNodeKind as K;
    match &mut p.kind {
        K::MapUnary(_, a) => blind(a),
        K::MapBinary(_, a, b)
        | K::Join {
            left: a, right: b, ..
        } => {
            blind(a);
            blind(b);
        }
        K::Aggregate {
            inputs,
            params,
            pred,
            func,
            order,
            window,
            per_item,
            into,
            ..
        } => {
            inputs.iter_mut().chain(params.iter_mut()).for_each(blind);
            if let Some(x) = pred {
                blind(x);
            }
            // SUM … XOR, COUNT, COUNTA, COUNTUNIQUE: values only, no
            // positions or capacity.
            let values_only = matches!(func.0, 1..=9 | 1005 | 1006 | 1100);
            if values_only && window.is_none() && !*per_item && into.is_empty() {
                order.clear();
            }
        }
        K::Recurrence { refs, .. } => refs.iter_mut().for_each(|(_, x)| blind(x)),
        K::Shift { input, default, .. } => {
            blind(input);
            if let Some(d) = default {
                blind(d);
            }
        }
        K::FuncCall { args, .. } => args.iter_mut().for_each(blind),
        // COALESCE's domain is its arguments' cells; FILL's grid lists items.
        K::Coalesce { args, .. } => args.iter_mut().for_each(blind),
        K::Remap { input, .. } => blind(input),
        K::If {
            test,
            then,
            otherwise,
            ..
        } => {
            blind(test);
            blind(then);
            if let Some(o) = otherwise {
                blind(o);
            }
        }
        // Leaves, and nodes whose item lists always count.
        _ => {}
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

/// The first engine-internal measure id of a FIXPOINT's parts (three per
/// FIXPOINT), between the LAMBDA placeholders and the recurrence slots.
const FIX_BASE: u32 = u32::MAX - 4000;
/// How many FIXPOINT measures one engine solves.
const FIX_LIMIT: u32 = 900;

/// A FIXPOINT measure, solved host-side: the dataflow computes its first
/// iterate (`init`); the worker iterates, and publishes the result on
/// session `id`, the measure's own, which the measures that read it see as
/// an input. A step is `host`, the body's plan evaluated in memory
/// ([`crate::hosteval`]), when it covers the body; else the dataflow's
/// `body`, reading the iterate from session `r`.
struct Fix {
    id: MeasureId,
    r: MeasureId,
    init: MeasureId,
    body: MeasureId,
    max_iter: u32,
    tolerance: f64,
    host: Option<PlanNode>,
    /// The measures the body reads, but r.
    reads: Vec<MeasureId>,
}

/// Replace each FIXPOINT plan by its first-iterate and step plans, in place
/// (so the build order holds), and describe it.
fn split_fixpoints(model: &Model, plans: Plans) -> Result<(Plans, Vec<Fix>), EngineError> {
    let ctx = crate::compiler::CompileContext::for_model(model);
    let (mut out, mut fixes) = (Vec::new(), Vec::new());
    for (mid, plan) in plans {
        let PlanNodeKind::Fixpoint {
            init,
            body,
            max_iter,
            tolerance,
        } = &plan.kind
        else {
            out.push((mid, plan));
            continue;
        };
        let n = fixes.len() as u32;
        if n == FIX_LIMIT {
            return Err(EngineError::Unsupported(format!(
                "a model may have at most {FIX_LIMIT} FIXPOINT measures"
            )));
        }
        let part = |j: u32| MeasureId(FIX_BASE + 3 * n + j);
        let (r, i, b) = (part(0), part(1), part(2));
        let (ip, bp, _) = crate::compiler::fixpoint_parts(&ctx, mid, init, body, r)?;
        let host = crate::hosteval::covers(&bp, r, &|_| true).then(|| bp.clone());
        let mut reads = HashSet::new();
        crate::hosteval::reads(&bp, &mut reads);
        reads.remove(&r);
        out.push((i, ip));
        out.push((b, bp));
        fixes.push(Fix {
            id: mid,
            r,
            init: i,
            body: b,
            max_iter: *max_iter,
            tolerance: *tolerance,
            host,
            reads: reads.into_iter().collect(),
        });
    }
    Ok((out, fixes))
}

type Sessions = HashMap<MeasureId, InputSession<u64, (CoordKey, CellValue), isize>>;
type Plans = Vec<(MeasureId, PlanNode)>;

/// What a FIXPOINT's sessions hold: the iterate last fed to `r`, and the
/// published result (and whether it converged).
#[derive(Default)]
struct FixState {
    r: MeasureValues,
    result: Arc<MeasureValues>,
    converged: bool,
}

/// Turn session `s`'s cells from `old` into `new`, as deltas.
fn replace(
    s: &mut InputSession<u64, (CoordKey, CellValue), isize>,
    old: &MeasureValues,
    new: &MeasureValues,
) {
    for (k, v) in old {
        if new.get(k) != Some(v) {
            s.remove((k.clone(), v.clone()));
        }
    }
    for (k, v) in new {
        if old.get(k) != Some(v) {
            s.insert((k.clone(), v.clone()));
        }
    }
}

/// Σ|a − b| over the cells, summed in key order (so a run decides the same
/// way every time). A cell only one side has, or a non-number that differs,
/// is an infinite change.
fn distance(a: &MeasureValues, b: &MeasureValues) -> f64 {
    if b.keys().any(|k| !a.contains_key(k)) {
        return f64::INFINITY;
    }
    let mut d: Vec<(&CoordKey, f64)> = a
        .iter()
        .map(|(k, x)| {
            let y = b.get(k);
            let step = match (x.as_num(), y.and_then(CellValue::as_num)) {
                _ if y == Some(x) => 0.0,
                (Some(x), Some(y)) => (x - y).abs(),
                _ => f64::INFINITY,
            };
            (k, step)
        })
        .collect();
    d.sort_by(|p, q| p.0.cmp(q.0));
    d.iter().map(|(_, x)| x).sum()
}

/// The worker's view of the model for a FIXPOINT solve: the input cells per
/// measure, and the measures changed since the last solve.
struct World<'w> {
    inputs: &'w HashMap<MeasureId, Arc<MeasureValues>>,
    dirty: &'w Mutex<HashSet<MeasureId>>,
}

/// Solve every FIXPOINT, in build order (one may read another), and publish
/// each result on its measure's session. A round is one step: the body's
/// plan evaluated host-side at the iterate (with what it reads but r fixed
/// for the solve), or, for a body the host evaluator does not cover, one
/// engine round: feed the iterate, read the step. `warm` (after an edit):
/// restart from the previous result if it converged, on the cells the first
/// iterate has; if that does not converge within max_iter rounds, start over
/// from the first iterate, so a result cut off at max_iter is always a full
/// recompute's. An edit that leaves a FIXPOINT's first iterate and what its
/// step reads as they were leaves its result as it was, unsolved.
// shortcut: a host round is a full pass over the edges (every PageRank cell
// moves every round, so a delta step would touch them all anyway), about
// 1/30 of a dataflow round's cost; a sparse step (delta-PageRank) would pay
// only when few cells still move.
#[allow(clippy::too_many_arguments, reason = "the worker's state, passed down")]
fn solve_fixpoints(
    fixes: &[Fix],
    state: &mut [FixState],
    warm: bool,
    sessions: &mut Sessions,
    t: &mut u64,
    run: &mut dyn FnMut(u64, &mut Sessions),
    snapshot: &Mutex<Snapshot>,
    first: &mut HashMap<MeasureId, Arc<MeasureValues>>,
    world: &World<'_>,
) {
    let get = |id: MeasureId| {
        snapshot
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .unwrap_or_default()
    };
    for (f, st) in fixes.iter().zip(state.iter_mut()) {
        let init = get(f.init);
        // The step's output is body(the last iterate fed), which is the
        // result, as long as nothing the FIXPOINT reads has changed.
        let same = |st: &FixState| match f.host {
            Some(_) => {
                let dirty = world.dirty.lock().unwrap();
                !f.reads.iter().any(|m| dirty.contains(m))
            }
            None => *get(f.body) == *st.result,
        };
        if warm && first.get(&f.id) == Some(&init) && same(st) {
            continue;
        }
        let mut host = f.host.as_ref().map(|plan| {
            let snap = snapshot.lock().unwrap();
            let env = f.reads.iter().filter_map(|m| {
                let vals = world.inputs.get(m).or_else(|| snap.get(m))?;
                Some((*m, Arc::clone(vals)))
            });
            crate::hosteval::Host::new(plan, f.r, env.collect())
        });
        let mut solve = |start: MeasureValues, st: &mut FixState| {
            let mut x = start;
            for _ in 0..f.max_iter {
                let y = match host.as_mut() {
                    Some(h) => h.step(&x),
                    None => {
                        let s = sessions
                            .get_mut(&f.r)
                            .expect("a FIXPOINT's iterate session");
                        replace(s, &st.r, &x);
                        st.r.clone_from(&x);
                        *t += 1;
                        run(*t, sessions);
                        (*get(f.body)).clone()
                    }
                };
                let d = distance(&x, &y);
                x = y;
                if d <= f.tolerance {
                    return (x, true);
                }
            }
            (x, false)
        };
        let mut out = None;
        if warm && st.converged {
            let start = init
                .iter()
                .map(|(k, v)| (k.clone(), st.result.get(k).unwrap_or(v).clone()))
                .collect();
            out = Some(solve(start, st)).filter(|(_, ok)| *ok);
        }
        let (x, converged) = out.unwrap_or_else(|| solve((*init).clone(), st));
        drop(host);
        if x != *st.result {
            world.dirty.lock().unwrap().insert(f.id);
        }
        let s = sessions
            .get_mut(&f.id)
            .expect("a FIXPOINT's result session");
        replace(s, &st.result, &x);
        st.result = Arc::new(x);
        st.converged = converged;
        first.insert(f.id, init);
        *t += 1;
        run(*t, sessions);
        let mut snap = snapshot.lock().unwrap();
        Arc::make_mut(&mut snap).insert(f.id, Arc::clone(&st.result));
    }
}

/// The worker thread: builds the dataflow once, applies the seed, then serves
/// edit rounds. Each round advances time by 1 and reports the changed outputs;
/// we keep a running snapshot per measure so we can return a full picture.
fn worker_loop(
    input_ids: Vec<MeasureId>,
    plans: Vec<(MeasureId, PlanNode)>,
    fixes: Vec<Fix>,
    seed: Vec<(MeasureId, CoordKey, CellValue)>,
    cmd_rx: Receiver<Cmd>,
    ready_tx: Sender<()>,
) {
    // The accumulated snapshot, shared with the dataflow's inspect closure.
    // `Arc<Mutex<..>>` (not Rc/RefCell) so the timely closure is Send + Sync.
    // The snapshot itself is copy-on-write (see [`Snapshot`]): replies hand
    // out an `Arc` clone, and `Arc::make_mut` below copies only what a caller
    // still shares and this round changes.
    let snapshot: Arc<Mutex<Snapshot>> = Arc::new(Mutex::new(Snapshot::default()));
    // The measures whose cells changed since the last edit round began.
    let dirty: Arc<Mutex<HashSet<MeasureId>>> = Arc::default();
    // The command receiver is `!Sync`; wrap it so the (Sync-bounded) timely
    // closure can hold it, and take it out once inside (single worker).
    let cmd_rx = Arc::new(Mutex::new(Some(cmd_rx)));
    let ready_tx = Arc::new(Mutex::new(Some(ready_tx)));

    timely::execute::execute_directly(move |worker| {
        let cmd_rx = cmd_rx
            .lock()
            .unwrap()
            .take()
            .expect("worker_loop runs a single timely worker");
        let ready_tx = ready_tx
            .lock()
            .unwrap()
            .take()
            .expect("worker_loop runs a single timely worker");
        let mut sessions: Sessions = HashMap::new();
        let snap = snapshot.clone();
        let mut probe = ProbeHandle::new();
        // A FIXPOINT's parts are the engine's own: not edited, not reported.
        let internal: HashSet<MeasureId> = fixes
            .iter()
            .flat_map(|f| [f.id, f.r, f.init, f.body])
            .collect();
        let mut fix_state: Vec<FixState> = fixes.iter().map(|_| FixState::default()).collect();
        let mut first = HashMap::new();
        // A body the host evaluates is no dataflow of its own.
        let mut fixes = fixes;
        let mut plans = plans;
        plans.retain(|(m, _)| !fixes.iter().any(|f| f.body == *m && f.host.is_some()));
        let mark = dirty.clone();

        let built: HashSet<MeasureId> = worker.dataflow::<u64, _, _>(|scope| {
            let mut input_colls = HashMap::new();
            for id in &input_ids {
                let mut s = InputSession::new();
                let coll = s.to_collection(scope);
                sessions.insert(*id, s);
                input_colls.insert(*id, coll);
            }

            let mut built = HashSet::new();
            for (mid, reduced) in build_derived(scope, &plans, &input_colls) {
                built.insert(mid);
                let snap = snap.clone();
                let mark = mark.clone();
                reduced
                    .inspect(move |((k, val), _t, diff)| {
                        // Maintain the running snapshot from the delta
                        // stream: a +1 sets the value, a -1 removes it. A
                        // changed cell's `+new` may arrive before its `-old`
                        // (updates are ordered by value), so a retraction
                        // only removes the value it retracts.
                        mark.lock().unwrap().insert(mid);
                        let mut s = snap.lock().unwrap();
                        let m = Arc::make_mut(Arc::make_mut(&mut s).entry(mid).or_default());
                        if *diff > 0 {
                            m.insert(k.clone(), val.clone());
                        } else if *diff < 0 && m.get(k) == Some(val) {
                            m.remove(k);
                        }
                    })
                    .probe_with(&mut probe);
            }
            built
        });
        // A body reading a measure with no collection would not build as a
        // dataflow either: it takes the dataflow path, with no step cells.
        for f in &mut fixes {
            let has = |m: &MeasureId| sessions.contains_key(m) || built.contains(m);
            if !f.reads.iter().all(has) {
                f.host = None;
            }
        }

        // Step the worker until the output frontier has passed `t`. (Advancing
        // an InputSession sets its time synchronously, so we cannot gate on the
        // session time; we gate on the dataflow probe instead.)
        let mut run_to = |t: u64, sessions: &mut Sessions| {
            for s in sessions.values_mut() {
                s.advance_to(t);
                s.flush();
            }
            while probe.less_than(&t) {
                worker.step();
            }
        };

        // Apply the seed at time 0, then advance to 1 and run. We keep each
        // input cell's value so a "set" is a proper retract(old)+assert(new)
        // on the cardinality-one cell, and a FIXPOINT step reads them.
        let mut inputs: HashMap<MeasureId, Arc<MeasureValues>> = HashMap::new();
        for (mid, coord, v) in &seed {
            if let Some(session) = sessions.get_mut(mid) {
                session.insert((coord.clone(), v.clone()));
                Arc::make_mut(inputs.entry(*mid).or_default()).insert(coord.clone(), v.clone());
            }
        }
        let mut t: u64 = 1;
        run_to(t, &mut sessions);
        solve_fixpoints(
            &fixes,
            &mut fix_state,
            false,
            &mut sessions,
            &mut t,
            &mut run_to,
            &snapshot,
            &mut first,
            &World {
                inputs: &inputs,
                dirty: &dirty,
            },
        );
        // What a reply shows: every measure but a FIXPOINT's parts.
        let reply_snapshot = |snapshot: &Mutex<Snapshot>| {
            let s = snapshot.lock().unwrap();
            if fixes.is_empty() {
                return Arc::clone(&s);
            }
            let mut m = (**s).clone();
            for f in &fixes {
                m.remove(&f.init);
                m.remove(&f.body);
            }
            Arc::new(m)
        };
        // Signal that startup is complete.
        let _ = ready_tx.send(());

        // Serve edit rounds.
        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                Cmd::Edit(edits, reply) => {
                    dirty.lock().unwrap().clear();
                    for e in edits.iter().filter(|e| !internal.contains(&e.measure)) {
                        if let Some(session) = sessions.get_mut(&e.measure) {
                            let cells = Arc::make_mut(inputs.entry(e.measure).or_default());
                            if let Some(old_val) = cells.remove(&e.coord) {
                                session.remove((e.coord.clone(), old_val));
                            }
                            if let Some(cv) = &e.value {
                                session.insert((e.coord.clone(), cv.clone()));
                                cells.insert(e.coord.clone(), cv.clone());
                            }
                            dirty.lock().unwrap().insert(e.measure);
                        }
                    }
                    t += 1;
                    run_to(t, &mut sessions);
                    solve_fixpoints(
                        &fixes,
                        &mut fix_state,
                        true,
                        &mut sessions,
                        &mut t,
                        &mut run_to,
                        &snapshot,
                        &mut first,
                        &World {
                            inputs: &inputs,
                            dirty: &dirty,
                        },
                    );
                    let _ = reply.send(reply_snapshot(&snapshot));
                }
                Cmd::Shutdown => break,
            }
        }
    });
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::{
        BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure, Name,
        Value, ValueType,
    };

    fn revenue_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(21), product, "WidgetB");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });
        let c = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
        m.set_input(
            MeasureId(100),
            c(&[(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m.set_input(
            MeasureId(100),
            c(&[(product, ItemId(21))]),
            Value::Number(20.0),
        );
        m.set_input(
            MeasureId(101),
            c(&[(time, ItemId(10)), (product, ItemId(20))]),
            Value::Number(100.0),
        );
        m.set_input(
            MeasureId(101),
            c(&[(time, ItemId(10)), (product, ItemId(21))]),
            Value::Number(50.0),
        );
        m
    }

    /// A new item reaches a value-only reduction as data: the engine serves
    /// the grown model and computes what a new engine computes. One that a
    /// plan enumerates (`POSITION`, `PREV`) does not.
    #[test]
    fn new_items_are_served_unless_a_plan_reads_their_category() {
        use improv_core_model::parse_definition;
        let (time, product) = (CategoryId(1), CategoryId(2));
        let define = |m: &mut Model, id: u32, text: &str| {
            let improv_core_model::Definition::Formula(f) = parse_definition(m, text).unwrap()
            else {
                panic!("{text}")
            };
            let name = f.target.0.clone();
            let (value_type, categories) =
                crate::compiler::derived_shape(m, MeasureId(id), &name, &f.formula).unwrap();
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name),
                value_type,
                categories,
                kind: MeasureKind::Derived(f.formula),
                description: None,
            });
        };
        let mut m = revenue_model();
        define(&mut m, 103, "Total = SUM(Revenue OVER Product)");
        define(&mut m, 104, "Count = COUNT(Quantity OVER Time, Product)");
        let targets = [MeasureId(102), MeasureId(103), MeasureId(104)];
        let (mut e, _) = Engine::new(&m, &targets).unwrap();
        assert!(e.serves(&m).unwrap());
        let mut grown = m.clone();
        grown.add_item(ItemId(22), product, "WidgetC");
        grown.add_item(ItemId(11), time, "2026");
        assert!(e.serves(&grown).unwrap());
        let c = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
        let edits = [
            (MeasureId(100), c(&[(product, ItemId(22))]), 5.0),
            (
                MeasureId(101),
                c(&[(time, ItemId(10)), (product, ItemId(22))]),
                7.0,
            ),
            (
                MeasureId(101),
                c(&[(time, ItemId(11)), (product, ItemId(22))]),
                3.0,
            ),
        ];
        for (mid, coord, v) in &edits {
            grown.set_input(*mid, coord.clone(), Value::Number(*v));
        }
        let snap = e
            .apply(
                edits
                    .iter()
                    .map(|(mid, coord, v)| Edit {
                        measure: *mid,
                        coord: encode_coord(coord),
                        value: Some(CellValue::num(*v)),
                    })
                    .collect(),
            )
            .unwrap();
        let fresh = crate::dataflow::evaluate(&grown, &targets).unwrap();
        for t in targets {
            assert_eq!(
                snap.get(&t).map(|m| (**m).clone()),
                fresh.get(&t).cloned(),
                "{t:?}"
            );
        }
        for text in ["Pos = POSITION(Time)", "Last = PREV(Quantity, Time)"] {
            let mut m = revenue_model();
            define(&mut m, 105, text);
            let (e, _) = Engine::new(&m, &[MeasureId(105)]).unwrap();
            let mut grown = m.clone();
            grown.add_item(ItemId(11), time, "2026");
            assert!(!e.serves(&grown).unwrap(), "{text}");
            grown = m.clone();
            grown.add_item(ItemId(22), product, "WidgetC");
            assert!(
                e.serves(&grown).unwrap(),
                "{text} over Time ignores Product"
            );
        }
        // FILL fills every item combination: a new item of either category
        // is a new grid row, so neither is served.
        let mut m = revenue_model();
        define(&mut m, 105, "Filled = FILL(Quantity, 0)");
        let (e, _) = Engine::new(&m, &[MeasureId(105)]).unwrap();
        for (id, cat) in [(11, time), (22, product)] {
            let mut grown = m.clone();
            grown.add_item(ItemId(id), cat, "new");
            assert!(!e.serves(&grown).unwrap(), "FILL with a new item {id}");
        }
        // A FIXPOINT is served when its parts are: here its step reads
        // @ITEMCOUNT(Product), so a new product is not served.
        let mut m = revenue_model();
        define(
            &mut m,
            105,
            "Fix = FIXPOINT(Price, LAMBDA(r, r / @ITEMCOUNT(Product) + 1))",
        );
        let (e, _) = Engine::new(&m, &[MeasureId(105)]).unwrap();
        let mut grown = m.clone();
        grown.add_item(ItemId(11), time, "2026");
        assert!(e.serves(&grown).unwrap());
        grown.add_item(ItemId(22), product, "WidgetC");
        assert!(!e.serves(&grown).unwrap());
        // COALESCE (no grid) and COUNTUNIQUE read only cells.
        let mut m = revenue_model();
        define(&mut m, 105, "Either = COALESCE(Quantity, Price)");
        define(&mut m, 106, "Kinds = COUNTUNIQUE(Quantity OVER Product)");
        let (e, _) = Engine::new(&m, &[MeasureId(105), MeasureId(106)]).unwrap();
        let mut grown = m.clone();
        grown.add_item(ItemId(22), product, "WidgetC");
        grown.add_item(ItemId(11), time, "2026");
        assert!(e.serves(&grown).unwrap());
    }

    /// Copy-on-write: an edit replaces only the changed measure's `Arc`;
    /// an untouched measure is the same allocation in both snapshots, and an
    /// old snapshot keeps its values.
    #[test]
    fn snapshot_shares_unchanged_measures() {
        let mut model = revenue_model();
        model.add_measure(Measure {
            id: MeasureId(103),
            name: Name("Double".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(1), CategoryId(2)],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
                Box::new(Expr::Literal(Value::Number(2.0))),
            ))),
            description: None,
        });
        let (mut engine, before) =
            Engine::new(&model, &[MeasureId(102), MeasureId(103)]).expect("engine");
        // Price feeds Revenue only.
        let after = engine
            .set(MeasureId(100), key(&[(2, 20)]), 15.0)
            .expect("set");
        assert!(Arc::ptr_eq(
            &before[&MeasureId(103)],
            &after[&MeasureId(103)]
        ));
        assert!(!Arc::ptr_eq(
            &before[&MeasureId(102)],
            &after[&MeasureId(102)]
        ));
        let rev = |s: &Snapshot| s[&MeasureId(102)][&key(&[(1, 10), (2, 20)])].as_num();
        assert_eq!(rev(&before), Some(1000.0), "old snapshot unchanged");
        assert_eq!(rev(&after), Some(1500.0));
    }

    /// Typed inputs seed and edit like numbers: a text input feeding a
    /// derived copy follows a text edit.
    #[test]
    fn typed_inputs_seed_and_edit() {
        let mut model = revenue_model();
        model.add_measure(Measure {
            id: MeasureId(103),
            name: Name("Kind".into()),
            value_type: ValueType::Text,
            categories: vec![CategoryId(2)],
            kind: MeasureKind::Input,
            description: None,
        });
        model.add_measure(Measure {
            id: MeasureId(104),
            name: Name("Label".into()),
            value_type: ValueType::Text,
            categories: vec![CategoryId(2)],
            kind: MeasureKind::Derived(Formula::new(Expr::Ref(
                MeasureId(103),
                DimensionSpec::default(),
            ))),
            description: None,
        });
        model.set_input(
            MeasureId(103),
            Coordinate::from_pairs([(CategoryId(2), ItemId(20))]),
            Value::Text("a".into()),
        );
        // A number in the text measure is mistyped: no cell.
        model.set_input(
            MeasureId(103),
            Coordinate::from_pairs([(CategoryId(2), ItemId(21))]),
            Value::Number(1.0),
        );
        let (mut engine, before) = Engine::new(&model, &[MeasureId(104)]).expect("engine");
        assert_eq!(before[&MeasureId(104)].len(), 1);
        let label = |s: &Snapshot| s[&MeasureId(104)].get(&key(&[(2, 20)])).cloned();
        assert_eq!(label(&before), Some(CellValue::Text("a".into())));
        let edit = |value| Edit {
            measure: MeasureId(103),
            coord: key(&[(2, 20)]),
            value,
        };
        let after = engine
            .apply(vec![edit(Some(CellValue::Text("b".into())))])
            .expect("edit");
        assert_eq!(label(&after), Some(CellValue::Text("b".into())));
        let cleared = engine.apply(vec![edit(None)]).expect("clear");
        assert_eq!(label(&cleared), None);
    }

    fn key(pairs: &[(u32, u32)]) -> CoordKey {
        let mut k: Vec<(u32, u32)> = pairs.to_vec();
        k.sort();
        k
    }

    #[test]
    fn initial_snapshot_matches_evaluate() {
        let model = revenue_model();
        let (_engine, snap) = Engine::new(&model, &[MeasureId(102)]).expect("engine");
        let rev = snap.get(&MeasureId(102)).expect("revenue");
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 20)])).and_then(|v| v.as_num()),
            Some(1000.0)
        ); // 10*100
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 21)])).and_then(|v| v.as_num()),
            Some(1000.0)
        ); // 20*50
        assert_eq!(rev.len(), 2);
    }

    #[test]
    fn edit_updates_only_affected_cell() {
        let model = revenue_model();
        let (mut engine, _snap) = Engine::new(&model, &[MeasureId(102)]).expect("engine");

        // Change Quantity[2025,WidgetA] 100 -> 120: Revenue there 1000 -> 1200,
        // the other cell (WidgetB) is unchanged.
        let snap = engine
            .set(MeasureId(101), key(&[(1, 10), (2, 20)]), 120.0)
            .expect("edit");
        let rev = snap.get(&MeasureId(102)).expect("revenue");
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 20)])).and_then(|v| v.as_num()),
            Some(1200.0)
        );
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 21)])).and_then(|v| v.as_num()),
            Some(1000.0)
        );
    }

    /// IF switches branches incrementally: raising Price past the test's
    /// threshold retracts the else-cells and asserts the then-cells.
    #[test]
    fn if_switches_branch_incrementally() {
        let mut model = revenue_model();
        let f =
            improv_core_model::parse_formula(&model, "Pick = IF(Price > 15, Revenue, -1)").unwrap();
        model.add_measure(Measure {
            id: MeasureId(103),
            name: f.target,
            value_type: ValueType::Number,
            categories: vec![CategoryId(1), CategoryId(2)],
            kind: MeasureKind::Derived(f.formula),
            description: None,
        });
        let (mut engine, snap) = Engine::new(&model, &[MeasureId(103)]).expect("engine");
        let pick = |s: &Snapshot, t, p| s[&MeasureId(103)].get(&key(&[(1, t), (2, p)])).cloned();
        assert_eq!(pick(&snap, 10, 20), Some(CellValue::num(-1.0)));
        let snap = engine
            .set(MeasureId(100), key(&[(2, 20)]), 16.0)
            .expect("edit");
        assert_eq!(pick(&snap, 10, 20), Some(CellValue::num(1600.0)));
        assert_eq!(pick(&snap, 10, 21), Some(CellValue::num(1000.0)));
        assert_eq!(snap[&MeasureId(103)].len(), 2, "no stale else-cell");
        let snap = engine
            .set(MeasureId(100), key(&[(2, 20)]), 1.0)
            .expect("edit");
        assert_eq!(pick(&snap, 10, 20), Some(CellValue::num(-1.0)));
    }

    #[test]
    fn edit_price_broadcasts() {
        let model = revenue_model();
        let (mut engine, _snap) = Engine::new(&model, &[MeasureId(102)]).expect("engine");
        // Price[WidgetA] 10 -> 15 broadcasts to Revenue[*, WidgetA].
        let snap = engine
            .set(MeasureId(100), key(&[(2, 20)]), 15.0)
            .expect("edit");
        let rev = snap.get(&MeasureId(102)).expect("revenue");
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 20)])).and_then(|v| v.as_num()),
            Some(1500.0)
        ); // 15*100
    }

    #[test]
    fn edit_that_lowers_a_value_keeps_the_cell() {
        let model = revenue_model();
        let (mut engine, _snap) = Engine::new(&model, &[MeasureId(102)]).expect("engine");
        // Price[WidgetA] 10 -> 5: Revenue[2025,A] 1000 -> 500. The `+500`
        // update sorts before the `-1000` one; the cell must survive.
        let snap = engine
            .set(MeasureId(100), key(&[(2, 20)]), 5.0)
            .expect("edit");
        let rev = snap.get(&MeasureId(102)).expect("revenue");
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 20)])).and_then(|v| v.as_num()),
            Some(500.0)
        );
        assert_eq!(rev.len(), 2);
    }

    #[test]
    fn adding_a_cell_creates_a_result() {
        let model = revenue_model();
        let (mut engine, _snap) = Engine::new(&model, &[MeasureId(102)]).expect("engine");
        // Add Quantity[2025, WidgetB] was already present; add a NEW coordinate
        // isn't possible without the item, but re-setting an existing cell and
        // clearing works. Clear Quantity[2025,WidgetA] -> Revenue there vanishes.
        let snap = engine
            .clear(MeasureId(101), key(&[(1, 10), (2, 20)]))
            .expect("clear");
        let rev = snap.get(&MeasureId(102)).expect("revenue");
        assert_eq!(rev.get(&key(&[(1, 10), (2, 20)])), None);
        // The other cell survives.
        assert_eq!(
            rev.get(&key(&[(1, 10), (2, 21)])).and_then(|v| v.as_num()),
            Some(1000.0)
        );
    }

    #[test]
    fn edits_to_untracked_measures_and_new_cells() {
        let mut model = revenue_model();
        // A non-numeric input and an orphan cell (no such measure) aren't seeded.
        model.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(CategoryId(2), ItemId(22))]),
            Value::Text("n/a".into()),
        );
        model.set_input(MeasureId(999), Coordinate::new(), Value::Number(1.0));
        let (mut engine, snap) = Engine::new(&model, &[MeasureId(102)]).expect("engine");
        assert_eq!(engine.targets(), [MeasureId(102)]);
        assert_eq!(snap[&MeasureId(102)].len(), 2);
        // An edit to a measure with no input session changes nothing.
        let s = engine.set(MeasureId(999), key(&[]), 5.0).expect("set");
        assert_eq!(s[&MeasureId(102)], snap[&MeasureId(102)]);
        // A brand-new Price cell (no previous value) has no Quantity yet...
        let s = engine
            .set(MeasureId(100), key(&[(2, 22)]), 7.0)
            .expect("set");
        assert_eq!(s[&MeasureId(102)].len(), 2);
        // ...until a matching Quantity arrives.
        let s = engine
            .set(MeasureId(101), key(&[(1, 10), (2, 22)]), 3.0)
            .expect("set");
        assert_eq!(
            s[&MeasureId(102)]
                .get(&key(&[(1, 10), (2, 22)]))
                .and_then(|v| v.as_num()),
            Some(21.0)
        );
    }

    /// Define derived measure `id` from `text`, with the engine's shape.
    fn define_text(m: &mut Model, id: u32, text: &str) {
        let f = improv_core_model::parse_formula(m, text).unwrap_or_else(|e| panic!("{e}"));
        let (value_type, categories) =
            crate::compiler::derived_shape(m, MeasureId(id), &f.target.0, &f.formula).unwrap();
        m.add_measure(Measure {
            id: MeasureId(id),
            name: f.target,
            value_type,
            categories,
            kind: MeasureKind::Derived(f.formula),
            description: None,
        });
    }

    /// One role pair Src/Dst of n people, inputs X[Src] = 1, 2, … and a
    /// pseudo-random weighted graph W[Src, Dst] (person 0 dangling).
    fn graph_model(n: u32) -> Model {
        let mut m = Model::new();
        let (s, d) = (CategoryId(1), CategoryId(2));
        m.add_category(s, "Src");
        m.add_category(d, "Dst");
        for i in 0..n {
            m.add_item(ItemId(100 + i), s, format!("p{i}"));
            m.add_item(ItemId(200 + i), d, format!("p{i}"));
        }
        for (id, name, cats) in [(10, "X", vec![s]), (11, "W", vec![s, d])] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: cats,
                kind: MeasureKind::Input,
                description: None,
            });
        }
        let mut seed = 7u64;
        for i in 0..n {
            let at = Coordinate::from_pairs([(s, ItemId(100 + i))]);
            m.set_input(MeasureId(10), at, Value::Number(f64::from(i) + 1.0));
            for j in 0..n {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                if i > 0 && i != j && (seed >> 33).is_multiple_of(3) {
                    let at = Coordinate::from_pairs([(s, ItemId(100 + i)), (d, ItemId(200 + j))]);
                    let w = 1.0 + ((seed >> 40) % 9) as f64;
                    m.set_input(MeasureId(11), at, Value::Number(w));
                }
            }
        }
        m
    }

    /// The fixpoint suite's FIXPOINTs, solved with host steps and with
    /// dataflow steps: the same snapshot bit for bit, fresh and after an
    /// edit and a clear. A body with an uncovered node (PREV is a shift)
    /// solves on the dataflow either way, to the hand-computed answer.
    #[test]
    fn host_and_dataflow_steps_agree_bit_for_bit() {
        let mut m = graph_model(12);
        for (id, text) in [
            (300, "Out = SUM(W OVER Dst)"),
            (301, "M = W / Out"),
            (302, "Dangling = FILL(0 * Out, 1)"),
            (
                303,
                "PR = FIXPOINT(SEQUENCE(Src, 1 / @ITEMCOUNT(Src), 0), \
                 LAMBDA(r, RELABEL((1 - 0.85) / @ITEMCOUNT(Src) \
                 + 0.85 * (FILL(SUMPRODUCT(r, M OVER Src), 0) \
                 + SUMPRODUCT(r, Dangling OVER Src) / @ITEMCOUNT(Src)), Dst, Src)), \
                 200, 0.0000000000001)",
            ),
            (304, "Two = FIXPOINT(X, LAMBDA(r, r / 2 + X))"),
            (305, "Cut = FIXPOINT(X, LAMBDA(r, r / 2 + X), 3)"),
            (306, "Grow = FIXPOINT(X, LAMBDA(r, r * 2), 40, 0)"),
            (307, "Zero = FIXPOINT(X, LAMBDA(r, r * 2), 0)"),
            (308, "Half = Two / 2"),
            (309, "Again = FIXPOINT(Two, LAMBDA(r, (r + Half) / 2))"),
            (310, "Root = FIXPOINT(1, LAMBDA(r, (r + 2 / r) / 2))"),
            (311, "Wide = FIXPOINT(0, LAMBDA(r, r / 2 + X))"),
            (
                312,
                "Back = FIXPOINT(X, LAMBDA(r, PREV(r, Src, 1, 0) / 2 + X))",
            ),
        ] {
            define_text(&mut m, id, text);
        }
        let ids: Vec<MeasureId> = (300..=312).map(MeasureId).collect();
        let (_, fixes) = split_fixpoints(&m, compile_plans(&m, &ids).unwrap()).unwrap();
        let hosted: Vec<u32> = fixes
            .iter()
            .filter(|f| f.host.is_some())
            .map(|f| f.id.0)
            .collect();
        assert_eq!(hosted, [303, 304, 305, 306, 307, 309, 310, 311]);
        let (mut host, a) = Engine::build(&m, &ids, true).unwrap();
        let (mut dd, b) = Engine::build(&m, &ids, false).unwrap();
        assert_eq!(a, b);
        let back = |s: &Snapshot| {
            (0..12)
                .map(|i| s[&MeasureId(312)][&vec![(1, 100 + i)]].as_num().unwrap())
                .collect::<Vec<_>>()
        };
        // r_i = r_(i-1) / 2 + X_i, from r_0 = X_0 = 1: exact in 12 rounds.
        let mut want = vec![0.0; 12];
        for i in 0..12 {
            want[i] = f64::from(i as u32 + 1) + if i > 0 { want[i - 1] / 2.0 } else { 0.0 };
        }
        assert_eq!(back(&a), want);
        let pr: f64 = a[&MeasureId(303)]
            .values()
            .filter_map(CellValue::as_num)
            .sum();
        assert!((pr - 1.0).abs() < 1e-12);
        for e in [
            (MeasureId(11), vec![(1, 103), (2, 200)], Some(30.0)),
            (MeasureId(10), vec![(1, 105)], Some(-4.0)),
            (MeasureId(10), vec![(1, 102)], None),
        ] {
            let edit = vec![Edit {
                measure: e.0,
                coord: e.1.clone(),
                value: e.2.map(CellValue::num),
            }];
            let (a, b) = (host.apply(edit.clone()).unwrap(), dd.apply(edit).unwrap());
            assert_eq!(a, b, "after {e:?}");
        }
    }
}
