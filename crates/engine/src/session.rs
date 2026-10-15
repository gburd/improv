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

use crate::plan::PlanNode;
use crate::{encode_coord, CellValue, CoordKey};
use improv_core_model::{MeasureId, MeasureKind, Model, Value, ValueType};
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
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
}

impl Engine {
    /// Build a live engine for `model`, tracking `targets` (derived measures).
    /// Seeds the graph with the model's current input cells and returns a handle
    /// plus the initial snapshot.
    pub fn new(model: &Model, targets: &[MeasureId]) -> Result<(Engine, Snapshot), EngineError> {
        // Compile plans up front (outside the worker), so a compile error is
        // reported synchronously rather than on the worker thread.
        let plans = compile_plans(model, targets)?;

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

        // Seed edits: the model's current input cells of their measure's
        // type (a mistyped value, e.g. text in a number measure, is no cell).
        let mut seed: Vec<(MeasureId, CoordKey, CellValue)> = Vec::new();
        let mut current: HashMap<(MeasureId, CoordKey), CellValue> = HashMap::new();
        for ((mid, coord), val) in &model.inputs {
            let typed = matches!(
                (model.measures.get(mid).map(|m| m.value_type), val),
                (Some(ValueType::Number), Value::Number(_))
                    | (Some(ValueType::Boolean), Value::Boolean(_))
                    | (Some(ValueType::Text), Value::Text(_))
                    | (Some(ValueType::DateTime), Value::DateTime(_))
            );
            if let Some(cv) = CellValue::from_model_value(val).filter(|_| typed) {
                let key = encode_coord(coord);
                seed.push((*mid, key.clone(), cv.clone()));
                current.insert((*mid, key), cv);
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
    /// snapshot of all tracked derived measures.
    pub fn apply(&mut self, edits: Vec<Edit>) -> Result<Snapshot, EngineError> {
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
    seed: Vec<(MeasureId, CoordKey, CellValue)>,
    cmd_rx: Receiver<Cmd>,
    ready_tx: Sender<()>,
) {
    use std::sync::Mutex;
    // The accumulated snapshot, shared with the dataflow's inspect closure.
    // `Arc<Mutex<..>>` (not Rc/RefCell) so the timely closure is Send + Sync.
    // The snapshot itself is copy-on-write (see [`Snapshot`]): replies hand
    // out an `Arc` clone, and `Arc::make_mut` below copies only what a caller
    // still shares and this round changes.
    let snapshot: Arc<Mutex<Snapshot>> = Arc::new(Mutex::new(Snapshot::default()));
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

            for (mid, reduced) in build_derived(scope, &plans, &input_colls) {
                let snap = snap.clone();
                reduced
                    .inspect(move |((k, val), _t, diff)| {
                        // Maintain the running snapshot from the delta
                        // stream: a +1 sets the value, a -1 removes it. A
                        // changed cell's `+new` may arrive before its `-old`
                        // (updates are ordered by value), so a retraction
                        // only removes the value it retracts.
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
        for (mid, coord, v) in &seed {
            if let Some(session) = sessions.get_mut(mid) {
                session.insert((coord.clone(), v.clone()));
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
        for (mid, coord, v) in &seed {
            prev.insert((*mid, coord.clone()), v.clone());
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
                            match &e.value {
                                Some(cv) => {
                                    session.insert((e.coord.clone(), cv.clone()));
                                    prev.insert(key, cv.clone());
                                }
                                None => {
                                    prev.remove(&key);
                                }
                            }
                        }
                    }
                    t += 1;
                    run_to(t, &mut sessions);
                    let out = Arc::clone(&snapshot.lock().unwrap());
                    let _ = reply.send(out);
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
}
