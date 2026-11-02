//! The model service: everything the window asks of a model, wherever the
//! model lives. [`LocalService`] is a store this process opens (a Mentat
//! file, `postgres://…?model=`) and an engine it runs; a remote service
//! (`improv-server` sessions) implements the same trait later and the
//! window does not change.
//!
//! A service runs on its own thread ([`spawn`]), never the UI's: loading,
//! building the engine, a recalculation and a save all take as long as they
//! take, and the window keeps drawing. The window sends [`Request`]s and
//! receives [`Update`]s; it keeps its own copy of the model for drawing and
//! applies what the updates say, so the service is the authority (as a
//! server would be).

use std::sync::{mpsc, Arc};

use improv_core_model::{Coordinate, MeasureId, Model, Value};
use improv_engine::session::{input_cell, Changes, Edit, Engine, Snapshot};
use improv_engine::{decode_coord, CoordKey};
use improv_store::{ChangeSet, SaveToken, Store};

/// One input cell to set (`Some`) or clear (`None`).
#[derive(Debug, Clone, PartialEq)]
pub struct CellEdit {
    pub measure: MeasureId,
    pub at: CoordKey,
    pub value: Option<Value>,
}

/// An edit round the service applied: the cells, what they changed, and
/// whether the save of them failed (`Some(why)`).
pub type Edited = (Vec<CellEdit>, Changes, Option<String>);

/// What a model service does. Every method runs on the service's thread.
pub trait ModelService: Send {
    /// Load the model and compute every derived measure.
    fn open(&mut self) -> Result<(Model, Snapshot), String>;
    /// Set or clear input cells and recompute, all or none (`Err`: nothing
    /// changed), then persist. Returns the cells as now held, what the
    /// recomputation changed, and the save's outcome: a failed save keeps
    /// the edit, unsaved.
    fn edit(&mut self, edits: Vec<CellEdit>) -> Result<Edited, String>;
    /// Replace everything but the input cells (a formula, a name, a view),
    /// rebuild the engine, and persist. A structure that does not build is
    /// refused and nothing changes (`Err`). Returns the new snapshot, and
    /// the save's outcome.
    fn restructure(&mut self, structure: Model) -> Result<(Snapshot, Option<String>), String>;
}

/// What the window asks its service to do.
#[derive(Debug)]
pub enum Request {
    Edit(Vec<CellEdit>),
    Restructure(Box<Model>),
}

/// What the service tells the window.
#[derive(Debug, Clone)]
pub enum Update {
    /// The model loaded and computed.
    Opened(Arc<Model>, Snapshot),
    /// Cells were set ([`ModelService::edit`]), what they changed, and why
    /// they are not saved, if they are not.
    Edited(Arc<Vec<CellEdit>>, Arc<Changes>, Option<String>),
    /// The structure was replaced (the cells are as they were): the new
    /// structure, its snapshot, and why it is not saved, if it is not.
    Restructured(Arc<Model>, Snapshot, Option<String>),
    /// A request failed; nothing changed.
    Failed(String),
}

/// Run `service` on a thread of its own (with the engine's stack: formulas
/// compile recursively). It opens the model at once, then serves requests
/// until the window drops its sender.
pub fn spawn(
    mut service: Box<dyn ModelService>,
    updates: futures::channel::mpsc::UnboundedSender<Update>,
) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    let failed = updates.clone();
    let spawned = std::thread::Builder::new()
        .name("improv-cosmic-model".into())
        .stack_size(improv_engine::dataflow::ENGINE_STACK)
        .spawn(move || {
            let send = |u| {
                let _ = updates.unbounded_send(u);
            };
            match service.open() {
                Ok((m, s)) => send(Update::Opened(Arc::new(m), s)),
                Err(e) => return send(Update::Failed(e)),
            }
            // Serve until the window drops its sender.
            for req in rx {
                send(match req {
                    Request::Edit(edits) => match service.edit(edits) {
                        Ok((cells, changes, unsaved)) => {
                            Update::Edited(Arc::new(cells), Arc::new(changes), unsaved)
                        }
                        Err(e) => Update::Failed(e),
                    },
                    Request::Restructure(m) => {
                        let shown = Arc::new((*m).clone());
                        match service.restructure(*m) {
                            Ok((snap, unsaved)) => Update::Restructured(shown, snap, unsaved),
                            Err(e) => Update::Failed(e),
                        }
                    }
                });
            }
        });
    if let Err(e) = spawned {
        let _ = failed.unbounded_send(Update::Failed(format!(
            "the model thread did not start: {e}"
        )));
    }
    tx
}

/// A model in a store this process opens, computed by an engine it runs.
pub struct LocalService {
    /// The store's target (`""` is a model in memory only).
    target: String,
    model: Model,
    engine: Option<Engine>,
    token: SaveToken,
    /// Changes a save failed to write, for the next save.
    unsaved: ChangeSet,
}

impl LocalService {
    pub fn new(target: impl Into<String>) -> LocalService {
        LocalService {
            target: target.into(),
            model: Model::new(),
            engine: None,
            token: SaveToken::default(),
            unsaved: ChangeSet::default(),
        }
    }

    /// Persist `changes`, reopening the store over the token from the last
    /// load or save (as `crates/gui` does, so no handle is held between).
    fn save(&mut self, changes: &ChangeSet) -> Result<(), String> {
        if self.target.is_empty() {
            return Ok(());
        }
        let mut store = Store::open(&self.target).map_err(|e| e.to_string())?;
        store.adopt_save_token(std::mem::take(&mut self.token));
        let r = store.save_changes(&self.model, changes);
        self.token = store.take_save_token();
        r.map_err(|e| format!("not saved: {e}"))
    }

    /// Build the engine over every derived measure, and its first snapshot.
    fn build(&mut self) -> Result<Snapshot, String> {
        let mut derived: Vec<MeasureId> = self
            .model
            .measures
            .values()
            .filter(|m| m.is_derived())
            .map(|m| m.id)
            .collect();
        derived.sort_by_key(|m| m.0);
        self.engine = None;
        if derived.is_empty() {
            return Ok(Snapshot::default());
        }
        let (e, s) = Engine::new(&self.model, &derived).map_err(|e| e.to_string())?;
        self.engine = Some(e);
        Ok(s)
    }
}

impl ModelService for LocalService {
    fn open(&mut self) -> Result<(Model, Snapshot), String> {
        if !self.target.is_empty() {
            let mut store = Store::open(&self.target).map_err(|e| e.to_string())?;
            self.model = store.load_model().map_err(|e| e.to_string())?;
            self.token = store.take_save_token();
        }
        let snapshot = self.build()?;
        Ok((self.model.clone(), snapshot))
    }

    fn edit(&mut self, edits: Vec<CellEdit>) -> Result<Edited, String> {
        // Check every edit first, so a refused one changes nothing.
        for e in &edits {
            let m = self
                .model
                .measures
                .get(&e.measure)
                .ok_or_else(|| format!("no measure {}", e.measure.0))?;
            if m.is_derived() {
                return Err(format!("{} is computed, not entered", m.name.0));
            }
            if let Some(v) = &e.value {
                if v.type_of() != Some(m.value_type) {
                    return Err(format!("{} holds {:?} values", m.name.0, m.value_type));
                }
            }
        }
        let mut engine_edits = Vec::with_capacity(edits.len());
        let mut changes = ChangeSet::default();
        let mut prior = Vec::with_capacity(edits.len());
        for e in &edits {
            let ty = self.model.measures[&e.measure].value_type;
            let at: Coordinate = decode_coord(&e.at);
            let old = match &e.value {
                Some(v) => self.model.inputs.insert((e.measure, at.clone()), v.clone()),
                None => self.model.inputs.remove(&(e.measure, at.clone())),
            };
            prior.push((e.measure, at.clone(), old));
            engine_edits.push(Edit {
                measure: e.measure,
                coord: e.at.clone(),
                value: e.value.as_ref().and_then(|v| input_cell(ty, v)),
            });
            changes.cells.push((e.measure, at));
        }
        let delta = match &mut self.engine {
            Some(engine) => match engine.apply_delta(engine_edits) {
                Ok(d) => d,
                Err(err) => {
                    // Put the cells back, and an engine that agrees with them.
                    for (m, at, old) in prior.into_iter().rev() {
                        match old {
                            Some(v) => drop(self.model.inputs.insert((m, at), v)),
                            None => drop(self.model.inputs.remove(&(m, at))),
                        }
                    }
                    let _ = self.build();
                    return Err(err.to_string());
                }
            },
            None => Changes::default(),
        };
        // A failed save keeps the edit (in the model and the engine) and
        // says so; its cells join the next save, which writes them.
        self.unsaved.add(changes);
        let unsaved = match self.save(&self.unsaved.clone()) {
            Ok(()) => {
                self.unsaved = ChangeSet::default();
                None
            }
            Err(e) => Some(e),
        };
        Ok((edits, delta, unsaved))
    }

    fn restructure(&mut self, mut structure: Model) -> Result<(Snapshot, Option<String>), String> {
        structure.inputs = std::mem::take(&mut self.model.inputs);
        let old = std::mem::replace(&mut self.model, structure);
        match self.build() {
            Ok(snap) => {
                self.unsaved.structure = true;
                let unsaved = match self.save(&self.unsaved.clone()) {
                    Ok(()) => {
                        self.unsaved = ChangeSet::default();
                        None
                    }
                    Err(e) => Some(e),
                };
                Ok((snap, unsaved))
            }
            Err(e) => {
                // Put the old structure back, with the cells, and its engine.
                let inputs = std::mem::take(&mut self.model.inputs);
                self.model = old;
                self.model.inputs = inputs;
                let _ = self.build();
                Err(e)
            }
        }
    }
}

/// Apply what an edit round changed to a snapshot the window holds.
pub fn apply_changes(snapshot: &mut Snapshot, changes: &Changes) {
    let s = Arc::make_mut(snapshot);
    for (m, cells) in changes {
        let values = Arc::make_mut(s.entry(*m).or_default());
        for c in cells {
            match &c.new {
                Some(v) => drop(values.insert(c.coord.clone(), v.clone())),
                None => drop(values.remove(&c.coord)),
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use improv_core_model::{
        BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure,
        MeasureKind, Name, Value, ValueType,
    };
    use improv_engine::CellValue;
    use std::collections::HashMap;

    /// Time × Product: Price[Product] × Quantity[Time, Product] = Revenue.
    pub(crate) fn revenue_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(11), time, "2026");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(21), product, "WidgetB");
        let measure = |id, name: &str, cats, kind| Measure {
            id: MeasureId(id),
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: cats,
            kind,
            description: None,
            format: None,
        };
        m.add_measure(measure(
            100,
            "Quantity",
            vec![time, product],
            MeasureKind::Input,
        ));
        m.add_measure(measure(101, "Price", vec![product], MeasureKind::Input));
        m.add_measure(measure(
            102,
            "Revenue",
            vec![time, product],
            MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
            ))),
        ));
        let at = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
        m.set_input(
            MeasureId(101),
            at(&[(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m.set_input(
            MeasureId(101),
            at(&[(product, ItemId(21))]),
            Value::Number(20.0),
        );
        for (t, p, q) in [
            (10, 20, 100.0),
            (10, 21, 50.0),
            (11, 20, 120.0),
            (11, 21, 80.0),
        ] {
            m.set_input(
                MeasureId(100),
                at(&[(time, ItemId(t)), (product, ItemId(p))]),
                Value::Number(q),
            );
        }
        m
    }

    /// A service over `m`, in memory only.
    pub(crate) fn memory(m: Model) -> LocalService {
        let mut s = LocalService::new("");
        s.model = m;
        s
    }

    #[test]
    fn the_thread_opens_the_model_and_computes_it() {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let _req = spawn(Box::new(memory(revenue_model())), tx);
        let first = futures::executor::block_on(futures::StreamExt::next(&mut rx));
        let Some(Update::Opened(m, snap)) = first else {
            panic!("not opened: {first:?}")
        };
        assert_eq!(m.measures.len(), 3);
        let k: improv_engine::CoordKey = [(1, 10), (2, 20)].into_iter().collect();
        assert_eq!(snap[&MeasureId(102)].get(&k), Some(&CellValue::num(1000.0)));
    }

    fn key(p: &[(u32, u32)]) -> CoordKey {
        p.iter().copied().collect()
    }

    fn set(m: u32, at: &[(u32, u32)], v: Option<Value>) -> CellEdit {
        CellEdit {
            measure: MeasureId(m),
            at: key(at),
            value: v,
        }
    }

    #[test]
    fn an_edit_is_saved_and_reads_back_from_the_store() {
        let path = std::env::temp_dir().join(format!("improv-cosmic-{}.db", std::process::id()));
        let db = path.to_string_lossy().into_owned();
        let mut store = Store::open(&db).unwrap();
        store.save_model(&revenue_model()).unwrap();
        drop(store);
        let mut s = LocalService::new(db.clone());
        let _ = s.open().unwrap();
        let (cells, changes, unsaved) = s
            .edit(vec![set(
                100,
                &[(1, 10), (2, 20)],
                Some(Value::Number(7.0)),
            )])
            .unwrap();
        assert_eq!((cells.len(), unsaved), (1, None));
        let rev = &changes[&MeasureId(102)];
        assert_eq!(rev[0].new, Some(CellValue::num(70.0)));
        let back = Store::open(&db).unwrap().load_model().unwrap();
        let at = decode_coord(&key(&[(1, 10), (2, 20)]));
        assert_eq!(back.input(MeasureId(100), &at), Some(&Value::Number(7.0)));
        for f in [
            path.clone(),
            path.with_extension("db-wal"),
            path.with_extension("db-shm"),
        ] {
            let _ = std::fs::remove_file(f);
        }
    }

    #[test]
    fn a_refused_edit_changes_nothing_of_its_batch() {
        let mut s = memory(revenue_model());
        let _ = s.open().unwrap();
        let before = s.model.inputs.clone();
        // The first is fine, the second is a derived cell: neither applies.
        let r = s.edit(vec![
            set(100, &[(1, 10), (2, 20)], Some(Value::Number(1.0))),
            set(102, &[(1, 10), (2, 20)], Some(Value::Number(1.0))),
        ]);
        assert!(r.unwrap_err().contains("computed"));
        assert_eq!(s.model.inputs, before);
        let typed = s.edit(vec![set(
            100,
            &[(1, 10), (2, 20)],
            Some(Value::Text("x".into())),
        )]);
        assert!(typed.unwrap_err().contains("Number"));
        assert!(s
            .edit(vec![set(999, &[], None)])
            .unwrap_err()
            .contains("no measure"));
    }

    #[test]
    fn clearing_a_cell_retracts_what_it_fed() {
        let mut s = memory(revenue_model());
        let _ = s.open().unwrap();
        let (_, changes, _) = s.edit(vec![set(100, &[(1, 10), (2, 20)], None)]).unwrap();
        assert_eq!(changes[&MeasureId(102)][0].new, None);
        let k = key(&[(1, 10), (2, 20)]);
        let before = HashMap::from([(k.clone(), CellValue::num(1000.0))]);
        let mut snap: Snapshot = Arc::new(HashMap::from([(MeasureId(102), Arc::new(before))]));
        apply_changes(&mut snap, &changes);
        assert_eq!(snap[&MeasureId(102)].get(&k), None);
    }

    /// Stress (`--ignored`): one cell edit at 10^6 cells, with a derived
    /// measure over them, in memory: the engine's incremental round only.
    #[test]
    #[ignore = "stress: 10^6 cells"]
    fn an_edit_at_a_million_cells_is_incremental() {
        let mut m = Model::new();
        let (a, b) = (CategoryId(1), CategoryId(2));
        m.add_category(a, "A");
        m.add_category(b, "B");
        for i in 0..1000 {
            m.add_item(ItemId(10_000 + i), a, format!("a{i}"));
            m.add_item(ItemId(20_000 + i), b, format!("b{i}"));
        }
        let measure = |id, kind| Measure {
            id: MeasureId(id),
            name: Name(format!("m{id}")),
            value_type: ValueType::Number,
            categories: vec![a, b],
            kind,
            description: None,
            format: None,
        };
        m.add_measure(measure(1, MeasureKind::Input));
        let double = Expr::BinaryOp(
            BinaryOp::Mul,
            Box::new(Expr::Ref(MeasureId(1), DimensionSpec::default())),
            Box::new(Expr::Literal(Value::Number(2.0))),
        );
        m.add_measure(measure(2, MeasureKind::Derived(Formula::new(double))));
        for i in 0..1000 {
            for j in 0..1000 {
                let at = Coordinate::from_pairs([(a, ItemId(10_000 + i)), (b, ItemId(20_000 + j))]);
                m.set_input(MeasureId(1), at, Value::Number(f64::from(i + j)));
            }
        }
        let mut s = memory(m);
        let t = std::time::Instant::now();
        let _ = s.open().unwrap();
        println!("open (engine build) at 10^6: {:?}", t.elapsed());
        let t = std::time::Instant::now();
        let (_, changes, _) = s
            .edit(vec![set(
                1,
                &[(1, 10_500), (2, 20_500)],
                Some(Value::Number(1.0)),
            )])
            .unwrap();
        let took = t.elapsed();
        println!("one edit at 10^6: {took:?}");
        assert_eq!(changes[&MeasureId(2)][0].new, Some(CellValue::num(2.0)));
        assert!(took < std::time::Duration::from_millis(500), "{took:?}");
    }

    #[test]
    fn a_new_formula_recomputes_and_a_bad_one_changes_nothing() {
        let mut s = memory(revenue_model());
        let _ = s.open().unwrap();
        let mut good = crate::formulas::structure(&s.model);
        let plus = Expr::BinaryOp(
            BinaryOp::Add,
            Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
        );
        good.measures.get_mut(&MeasureId(102)).unwrap().kind =
            MeasureKind::Derived(Formula::new(plus));
        let (snap, unsaved) = s.restructure(good).unwrap();
        assert_eq!(unsaved, None);
        let k = key(&[(1, 10), (2, 20)]);
        assert_eq!(
            snap[&MeasureId(102)].get(&k),
            Some(&CellValue::num(110.0)),
            "10 + 100"
        );
        assert_eq!(s.model.inputs.len(), 6, "cells kept");
        // Revenue = Revenue: a cycle is refused, and Revenue is still 10 + 100.
        let mut bad = crate::formulas::structure(&s.model);
        bad.measures.get_mut(&MeasureId(102)).unwrap().kind = MeasureKind::Derived(Formula::new(
            Expr::Ref(MeasureId(102), DimensionSpec::default()),
        ));
        assert!(s.restructure(bad).is_err());
        assert_eq!(s.model.inputs.len(), 6);
        let (_, changes, _) = s
            .edit(vec![set(
                100,
                &[(1, 10), (2, 20)],
                Some(Value::Number(1.0)),
            )])
            .unwrap();
        assert_eq!(
            changes[&MeasureId(102)][0].new,
            Some(CellValue::num(11.0)),
            "old engine kept"
        );
    }

    #[test]
    fn a_missing_store_fails_to_open() {
        let mut s = LocalService::new("postgres://nobody@127.0.0.1:1/x?model=m");
        assert!(s.open().is_err());
    }
}
