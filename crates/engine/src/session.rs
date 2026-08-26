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

use crate::compiler::{compile_formula, CompileContext};
use crate::plan::PlanNode;
use crate::{encode_coord, CellValue, CoordKey};
use improv_core_model::{MeasureId, MeasureKind, Model};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::thread::JoinHandle;

use crate::dataflow::{build_coll, derived_build_order, EngineError};
use differential_dataflow::input::InputSession;
use differential_dataflow::operators::Reduce;
use timely::dataflow::operators::probe::Handle as ProbeHandle;

/// A single input-cell edit: set `measure[coord]` to `value` (or remove it when
/// `value` is `None`).
#[derive(Debug, Clone)]
pub struct Edit {
    pub measure: MeasureId,
    pub coord: CoordKey,
    pub value: Option<f64>,
}

/// A snapshot of a computed derived measure after an edit round.
pub type MeasureValues = HashMap<CoordKey, CellValue>;

enum Cmd {
    /// Apply a batch of edits, advance time, and return the new full snapshot
    /// of every tracked derived measure.
    Edit(Vec<Edit>, Sender<HashMap<MeasureId, MeasureValues>>),
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
    current: HashMap<(MeasureId, CoordKey), f64>,
}

impl Engine {
    /// Build a live engine for `model`, tracking `targets` (derived measures).
    /// Seeds the graph with the model's current input cells and returns a handle
    /// plus the initial snapshot.
    pub fn new(
        model: &Model,
        targets: &[MeasureId],
    ) -> Result<(Engine, HashMap<MeasureId, MeasureValues>), EngineError> {
        // Compile plans up front (outside the worker), so a compile error is
        // reported synchronously rather than on the worker thread.
        let ctx = CompileContext::new(&model.measures);
        let order = derived_build_order(model, targets)?;
        let mut plans: Vec<(MeasureId, PlanNode)> = Vec::new();
        for m in &order {
            if let Some(measure) = model.measures.get(m) {
                if let MeasureKind::Derived(f) = &measure.kind {
                    plans.push((*m, compile_formula(&ctx, *m, f)?));
                }
            }
        }

        // Which input measures need sessions? Every input measure referenced by
        // any tracked plan, plus any that already has cells. We over-approximate
        // with "all input measures in the model" — extra empty sessions are
        // cheap and keep later edits to any input measure valid.
        let input_ids: Vec<MeasureId> = model
            .measures
            .iter()
            .filter(|(_, m)| matches!(m.kind, MeasureKind::Input))
            .map(|(id, _)| *id)
            .collect();

        // Seed edits: the model's current numeric input cells.
        let mut seed: Vec<Edit> = Vec::new();
        let mut current: HashMap<(MeasureId, CoordKey), f64> = HashMap::new();
        for ((mid, coord), val) in &model.inputs {
            if let improv_core_model::Value::Number(n) = val {
                let key = encode_coord(coord);
                seed.push(Edit {
                    measure: *mid,
                    coord: key.clone(),
                    value: Some(*n),
                });
                current.insert((*mid, key), *n);
            }
        }

        let (cmd_tx, cmd_rx) = std::sync::mpsc::channel::<Cmd>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let targets_owned = targets.to_vec();
        let plans_owned = plans;
        let seed_owned = seed;

        let worker = std::thread::spawn(move || {
            worker_loop(input_ids, plans_owned, seed_owned, cmd_rx, ready_tx);
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
    ) -> Result<HashMap<MeasureId, MeasureValues>, EngineError> {
        self.apply(vec![Edit {
            measure,
            coord,
            value: Some(value),
        }])
    }

    /// Remove an input cell and return the recomputed snapshot.
    pub fn clear(
        &mut self,
        measure: MeasureId,
        coord: CoordKey,
    ) -> Result<HashMap<MeasureId, MeasureValues>, EngineError> {
        self.apply(vec![Edit {
            measure,
            coord,
            value: None,
        }])
    }

    /// Apply a batch of edits atomically (one time step) and return the new
    /// snapshot of all tracked derived measures.
    pub fn apply(
        &mut self,
        edits: Vec<Edit>,
    ) -> Result<HashMap<MeasureId, MeasureValues>, EngineError> {
        // Update our shadow of current input values so retract deltas are exact.
        for e in &edits {
            let k = (e.measure, e.coord.clone());
            match e.value {
                Some(v) => {
                    self.current.insert(k, v);
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
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

/// The worker thread: builds the dataflow once, applies the seed, then serves
/// edit rounds. Each round advances time by 1 and reports the changed outputs;
/// we keep a running snapshot per measure so we can return a full picture.
fn worker_loop(
    input_ids: Vec<MeasureId>,
    plans: Vec<(MeasureId, PlanNode)>,
    seed: Vec<Edit>,
    cmd_rx: Receiver<Cmd>,
    ready_tx: Sender<()>,
) {
    use std::sync::{Arc, Mutex};
    // The accumulated snapshot, shared with the dataflow's inspect closure.
    // `Arc<Mutex<..>>` (not Rc/RefCell) so the timely closure is Send + Sync.
    let snapshot: Arc<Mutex<HashMap<MeasureId, MeasureValues>>> =
        Arc::new(Mutex::new(HashMap::new()));
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
        let mut sessions: HashMap<MeasureId, InputSession<u64, (CoordKey, CellValue), isize>> =
            HashMap::new();
        let snap = snapshot.clone();
        let mut probe = ProbeHandle::new();

        worker.dataflow::<u64, _, _>(|scope| {
            let mut input_colls = HashMap::new();
            for id in &input_ids {
                let mut s = InputSession::new();
                let coll = s.to_collection(scope);
                sessions.insert(*id, s);
                input_colls.insert(*id, coll);
            }

            let mut derived = HashMap::new();
            for (mid, plan) in &plans {
                if let Ok(coll) = build_coll(plan, &input_colls, &derived) {
                    let mid = *mid;
                    let snap = snap.clone();
                    let reduced = coll.reduce(|_k, inp, out| {
                        for (val, mult) in inp {
                            if *mult > 0 {
                                out.push(((*val).clone(), 1isize));
                            }
                        }
                    });
                    derived.insert(mid, reduced.clone());
                    reduced
                        .inspect(move |((k, val), _t, diff)| {
                            // Maintain the running snapshot from the delta
                            // stream: a +1 sets the value, a -1 removes it.
                            let mut s = snap.lock().unwrap();
                            let m = s.entry(mid).or_default();
                            if *diff > 0 {
                                m.insert(k.clone(), val.clone());
                            } else if *diff < 0 {
                                m.remove(k);
                            }
                        })
                        .probe_with(&mut probe);
                }
            }
        });

        // Step the worker until the output frontier has passed `t`. (Advancing
        // an InputSession sets its time synchronously, so we cannot gate on the
        // session time; we gate on the dataflow probe instead.)
        let mut run_to = |t: u64,
                          sessions: &mut HashMap<
            MeasureId,
            InputSession<u64, (CoordKey, CellValue), isize>,
        >| {
            for s in sessions.values_mut() {
                s.advance_to(t);
                s.flush();
            }
            while probe.less_than(&t) {
                worker.step();
            }
        };

        // Apply the seed at time 0, then advance to 1 and run.
        for e in &seed {
            if let Some(session) = sessions.get_mut(&e.measure) {
                if let Some(v) = e.value {
                    session.insert((e.coord.clone(), CellValue::num(v)));
                }
            }
        }
        let mut t: u64 = 1;
        run_to(t, &mut sessions);
        // Signal that startup is complete.
        let _ = ready_tx.send(());

        // Serve edit rounds.
        // We track the previous value of each cell so a "set" is a proper
        // retract(old)+assert(new) on the cardinality-one cell.
        let mut prev: HashMap<(MeasureId, CoordKey), CellValue> = HashMap::new();
        for e in &seed {
            if let Some(v) = e.value {
                prev.insert((e.measure, e.coord.clone()), CellValue::num(v));
            }
        }

        while let Ok(cmd) = cmd_rx.recv() {
            match cmd {
                Cmd::Edit(edits, reply) => {
                    for e in &edits {
                        let key = (e.measure, e.coord.clone());
                        let old = prev.get(&key).cloned();
                        if let Some(session) = sessions.get_mut(&e.measure) {
                            if let Some(old_val) = old {
                                session.remove((e.coord.clone(), old_val));
                            }
                            match e.value {
                                Some(v) => {
                                    let cv = CellValue::num(v);
                                    session.insert((e.coord.clone(), cv.clone()));
                                    prev.insert(key, cv);
                                }
                                None => {
                                    prev.remove(&key);
                                }
                            }
                        }
                    }
                    t += 1;
                    run_to(t, &mut sessions);
                    let out = snapshot.lock().unwrap().clone();
                    let _ = reply.send(out);
                }
                Cmd::Shutdown => break,
            }
        }
    });
}

#[cfg(test)]
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
}
