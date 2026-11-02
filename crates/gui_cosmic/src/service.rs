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

use improv_core_model::{MeasureId, Model};
use improv_engine::session::{Engine, Snapshot};
use improv_store::{SaveToken, Store};

/// What a model service does. Every method runs on the service's thread.
pub trait ModelService: Send {
    /// Load the model and compute every derived measure.
    fn open(&mut self) -> Result<(Model, Snapshot), String>;
}

/// What the window asks its service to do.
#[derive(Debug)]
pub enum Request {}

/// What the service tells the window.
#[derive(Debug, Clone)]
pub enum Update {
    /// The model loaded and computed.
    Opened(Arc<Model>, Snapshot),
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
            if let Ok(req) = rx.recv() {
                match req {}
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
}

impl LocalService {
    pub fn new(target: impl Into<String>) -> LocalService {
        LocalService {
            target: target.into(),
            model: Model::new(),
            engine: None,
            token: SaveToken::default(),
        }
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
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use improv_core_model::{
        BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure,
        MeasureKind, Name, Value, ValueType,
    };
    use improv_engine::CellValue;

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
    fn memory(m: Model) -> LocalService {
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

    #[test]
    fn a_missing_store_fails_to_open() {
        let mut s = LocalService::new("postgres://nobody@127.0.0.1:1/x?model=m");
        assert!(s.open().is_err());
    }
}
