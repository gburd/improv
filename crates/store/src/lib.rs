//! One model store over either backend: an embedded Mentat file (a path) or
//! a Postgres-hosted model (`postgres://…?model=<name>`).
//!
//! The GUI, TUI, CLI and server open models through [`Store::open`]; every
//! method delegates to `improv_storage_mentat::ModelStore` or
//! `improv_storage_pg::PgModelStore`, which share one surface.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

use improv_core_model::{Coordinate, MeasureId, Model};
use improv_storage_mentat::ModelStore;
use improv_storage_pg::PgModelStore;

/// Either backend's error. Callers match [`StoreError::Conflict`] for the
/// "reload and retry" path; everything else is reported as text.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error(transparent)]
    Mentat(improv_storage_mentat::StoreError),
    #[error(transparent)]
    Pg(improv_storage_pg::StoreError),
    /// Another writer committed since this handle loaded (either backend).
    #[error("conflict: {0}")]
    Conflict(String),
}

impl From<improv_storage_mentat::StoreError> for StoreError {
    fn from(e: improv_storage_mentat::StoreError) -> Self {
        match e {
            improv_storage_mentat::StoreError::Conflict(m) => StoreError::Conflict(m),
            e => StoreError::Mentat(e),
        }
    }
}

impl From<improv_storage_pg::StoreError> for StoreError {
    fn from(e: improv_storage_pg::StoreError) -> Self {
        match e {
            improv_storage_pg::StoreError::Conflict(m) => StoreError::Conflict(m),
            e => StoreError::Pg(e),
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A model store: Mentat file or Postgres model.
pub enum Store {
    Mentat(ModelStore),
    Pg(PgModelStore),
}

/// True when `target` names a Postgres-hosted model rather than a file.
pub fn is_postgres(target: &str) -> bool {
    target.starts_with("postgres://") || target.starts_with("postgresql://")
}

macro_rules! delegate {
    ($self:ident, $s:ident => $e:expr) => {
        match $self {
            Store::Mentat($s) => Ok($e?),
            Store::Pg($s) => Ok($e?),
        }
    };
}

impl Store {
    /// Open `target`: a `postgres://` / `postgresql://` URL with `?model=`
    /// opens that Postgres model; anything else is a Mentat file path (`""`
    /// is in-memory).
    pub fn open(target: &str) -> Result<Self> {
        Ok(if is_postgres(target) {
            Store::Pg(PgModelStore::open(target)?)
        } else {
            Store::Mentat(ModelStore::open(target)?)
        })
    }

    pub fn load_model(&mut self) -> Result<Model> {
        delegate!(self, s => s.load_model())
    }

    pub fn load_partial(&mut self, measure_ids: &[MeasureId]) -> Result<Model> {
        delegate!(self, s => s.load_partial(measure_ids))
    }

    pub fn save_model(&mut self, model: &Model) -> Result<()> {
        delegate!(self, s => s.save_model(model))
    }

    /// Persist only `cells` (measure, coordinate) of `model`: each is set to
    /// its value in `model.inputs`, or removed if it has none. O(cells), not
    /// O(model). The caller names every cell it changed since this handle's
    /// last load or save and changed nothing else; any other edit (a
    /// category, an item, a measure, a view, a LAMBDA, meta) needs
    /// [`Store::save_model`]. The same conflict check as `save_model`: a
    /// concurrent writer is refused with [`StoreError::Conflict`].
    pub fn save_cells(&mut self, model: &Model, cells: &[(MeasureId, Coordinate)]) -> Result<()> {
        delegate!(self, s => s.save_cells(model, cells))
    }

    /// Persist `changes` of `model`: its cells, and with
    /// [`ChangeSet::structure`] everything else, without the full save's
    /// pass over every stored cell (Mentat) or every cell in memory
    /// (Postgres). The same conflict check as `save_model`.
    /// `changes` must name everything that changed since this handle's last
    /// load or save ([`ChangeSet::between`] does, for two models). A handle
    /// that has loaded or saved nothing (no [`SaveToken`]) has no base to
    /// change from, so it saves the whole model, as `save_model` does.
    pub fn save_changes(&mut self, model: &Model, changes: &ChangeSet) -> Result<()> {
        if self.loaded_tx().is_none() {
            return self.save_model(model);
        }
        delegate!(self, s => s.save_changes(model, &changes.cells, changes.structure))
    }

    pub fn reload_model(&mut self) -> Result<Model> {
        delegate!(self, s => s.reload_model())
    }

    pub fn tx_high_water(&mut self) -> Result<i64> {
        delegate!(self, s => s.tx_high_water())
    }

    pub fn loaded_tx(&self) -> Option<i64> {
        match self {
            Store::Mentat(s) => s.loaded_tx(),
            Store::Pg(s) => s.loaded_tx(),
        }
    }

    pub fn set_loaded_tx(&mut self, tx: Option<i64>) {
        match self {
            Store::Mentat(s) => s.set_loaded_tx(tx),
            Store::Pg(s) => s.set_loaded_tx(tx),
        }
    }

    /// Everything a later handle needs to save safely over what this one
    /// loaded or saved: the optimistic-concurrency token and (Postgres) the
    /// snapshot a save diffs against, so it writes only what changed.
    /// Callers that reopen the store per save carry this across handles.
    pub fn save_token(&self) -> SaveToken {
        match self {
            Store::Mentat(s) => SaveToken {
                tx: s.loaded_tx(),
                pg: None,
            },
            Store::Pg(s) => SaveToken {
                tx: s.loaded_tx(),
                pg: s.snapshot().cloned(),
            },
        }
    }

    /// [`Store::save_token`] without the copy: moves the Postgres snapshot
    /// out, so the handle then saves as if it had loaded nothing. For callers
    /// that reopen the store per save (the GUI, the TUI), with
    /// [`Store::adopt_save_token`].
    pub fn take_save_token(&mut self) -> SaveToken {
        match self {
            Store::Mentat(s) => SaveToken {
                tx: s.loaded_tx(),
                pg: None,
            },
            Store::Pg(s) => SaveToken {
                tx: s.loaded_tx(),
                pg: s.take_snapshot(),
            },
        }
    }

    /// [`Store::set_save_token`] by value, without copying the snapshot.
    pub fn adopt_save_token(&mut self, token: SaveToken) {
        match self {
            Store::Mentat(s) => s.set_loaded_tx(token.tx),
            Store::Pg(s) => {
                s.set_loaded_tx(token.tx);
                s.set_snapshot(token.pg);
            }
        }
    }

    /// Adopt a token from [`Store::save_token`] before saving.
    pub fn set_save_token(&mut self, token: &SaveToken) {
        match self {
            Store::Mentat(s) => s.set_loaded_tx(token.tx),
            Store::Pg(s) => {
                s.set_loaded_tx(token.tx);
                s.set_snapshot(token.pg.clone());
            }
        }
    }
}

/// What changed in a model since it was last loaded or saved, for
/// [`Store::save_changes`]. Front ends say what they edited, so `Model`
/// carries no change tracking.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChangeSet {
    /// Input cells set or cleared (repeats are harmless).
    pub cells: Vec<(MeasureId, Coordinate)>,
    /// Anything else changed: a category, item, measure, view, LAMBDA,
    /// scenario, external function or SQL source.
    pub structure: bool,
}

impl ChangeSet {
    /// One cell edit.
    pub fn cell(measure: MeasureId, at: Coordinate) -> Self {
        ChangeSet {
            cells: vec![(measure, at)],
            structure: false,
        }
    }

    /// What differs from `old` to `new`: O(cells) in memory, with no store
    /// access.
    pub fn between(old: &Model, new: &Model) -> Self {
        let mut cells: Vec<_> = new
            .inputs
            .iter()
            .filter(|(k, v)| old.inputs.get(*k) != Some(*v))
            .map(|(k, _)| k.clone())
            .collect();
        cells.extend(
            old.inputs
                .keys()
                .filter(|k| !new.inputs.contains_key(*k))
                .cloned(),
        );
        // Every saved field of `Model` but `inputs`: a new one goes here.
        let structure = old.categories != new.categories
            || old.items != new.items
            || old.measures != new.measures
            || old.sql_sources != new.sql_sources
            || old.views != new.views
            || old.external_fns != new.external_fns
            || old.external_calls != new.external_calls
            || old.scenarios != new.scenarios
            || old.lambdas != new.lambdas;
        ChangeSet { cells, structure }
    }

    /// Add `other`'s changes to these.
    pub fn add(&mut self, other: ChangeSet) {
        self.cells.extend(other.cells);
        self.structure |= other.structure;
    }

    /// Nothing changed.
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty() && !self.structure
    }
}

/// What a store handle last loaded or saved, carried to the next handle (see
/// [`Store::save_token`]). `Default` is "nothing loaded": the save writes the
/// whole model unchecked.
#[derive(Debug, Clone, Default)]
pub struct SaveToken {
    tx: Option<i64>,
    pg: Option<improv_storage_pg::PgSnapshot>,
}

impl SaveToken {
    /// The optimistic-concurrency high-water this token carries.
    pub fn tx(&self) -> Option<i64> {
        self.tx
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::{CategoryId, Coordinate, ItemId, Measure, MeasureKind, Name, Value};

    fn model() -> Model {
        let mut m = Model::new();
        m.add_category(CategoryId(1), "Time");
        m.add_item(ItemId(10), CategoryId(1), "Q1");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: improv_core_model::ValueType::Number,
            categories: vec![CategoryId(1)],
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(CategoryId(1), ItemId(10))]),
            Value::Number(3.0),
        );
        m
    }

    #[test]
    fn change_sets_name_what_differs() {
        let old = model();
        assert!(ChangeSet::between(&old, &old).is_empty());
        let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10))]);
        let mut new = old.clone();
        new.inputs.remove(&(MeasureId(100), at.clone()));
        let other = Coordinate::from_pairs([(CategoryId(1), ItemId(11))]);
        new.set_input(MeasureId(100), other.clone(), Value::Number(1.0));
        let mut c = ChangeSet::between(&old, &new);
        c.cells.sort();
        assert_eq!(
            c,
            ChangeSet {
                cells: vec![(MeasureId(100), at.clone()), (MeasureId(100), other)],
                structure: false
            }
        );
        let mut renamed = old.clone();
        renamed.categories.get_mut(&CategoryId(1)).unwrap().name = Name("T".into());
        let mut d = ChangeSet::cell(MeasureId(100), at);
        assert!(!d.is_empty());
        d.add(ChangeSet::between(&old, &renamed));
        assert!(d.structure && d.cells.len() == 1);
    }

    /// B12's numbers: on the store at `IMPROV_B12_STORE` (a path or a
    /// `postgres://…?model=` URL) with an input measure 100, the load, one
    /// cell's incremental save (three times), a structural save, and a
    /// full save. `cargo test --release -p improv_store b12_ -- --ignored
    /// --nocapture`.
    #[test]
    #[ignore = "a probe: needs a large store at IMPROV_B12_STORE"]
    fn b12_save_probe() {
        use std::time::Instant;
        let Ok(target) = std::env::var("IMPROV_B12_STORE") else {
            return;
        };
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
        let t = Instant::now();
        let mut s = Store::open(&target).unwrap();
        println!("B12 open: {:.0} ms", ms(t));
        let t = Instant::now();
        let mut m = s.load_model().unwrap();
        println!("B12 load ({} cells): {:.0} ms", m.inputs.len(), ms(t));
        let at = m
            .inputs
            .keys()
            .find(|k| k.0 == MeasureId(100))
            .expect("a cell of measure 100")
            .1
            .clone();
        for v in [5.0, 6.0, 7.0] {
            m.set_input(MeasureId(100), at.clone(), Value::Number(v));
            let t = Instant::now();
            s.save_changes(&m, &ChangeSet::cell(MeasureId(100), at.clone()))
                .unwrap();
            println!("B12 save one cell (save_changes): {:.1} ms", ms(t));
        }
        // Reopened per save, as the GUI does.
        let token = s.take_save_token();
        m.set_input(MeasureId(100), at.clone(), Value::Number(8.0));
        let t = Instant::now();
        let mut g = Store::open(&target).unwrap();
        g.adopt_save_token(token);
        g.save_changes(&m, &ChangeSet::cell(MeasureId(100), at.clone()))
            .unwrap();
        let _token = g.take_save_token();
        println!("B12 open + save one cell + token: {:.1} ms", ms(t));
        let structure = ChangeSet {
            structure: true,
            ..Default::default()
        };
        let t = Instant::now();
        g.adopt_save_token(_token);
        g.save_changes(&m, &structure).unwrap();
        println!("B12 structural save (save_changes): {:.0} ms", ms(t));
        let t = Instant::now();
        g.save_model(&m).unwrap();
        println!("B12 full save (save_model, unchanged): {:.0} ms", ms(t));
        assert_eq!(
            Store::open(&target)
                .unwrap()
                .load_model()
                .unwrap()
                .input(MeasureId(100), &at),
            Some(&Value::Number(8.0))
        );
    }

    /// A change to any saved field but the cells is structure.
    #[test]
    fn every_saved_field_is_structure() {
        use improv_core_model::*;
        let old = model();
        let edits: [fn(&mut Model); 9] = [
            |m| m.categories.get_mut(&CategoryId(1)).unwrap().name = Name("T".into()),
            |m| m.items.get_mut(&ItemId(10)).unwrap().name = Name("Q9".into()),
            |m| m.measures.get_mut(&MeasureId(100)).unwrap().description = Some("d".into()),
            |m| {
                let src = SqlSource {
                    query: "SELECT 1".into(),
                    dimension_columns: vec![],
                    value_column: "v".into(),
                    refresh_policy: RefreshPolicy::Manual,
                };
                m.sql_sources.insert(MeasureId(100), src);
            },
            |m| {
                let v = View::from_matrices(
                    ViewId(1),
                    Name("v".into()),
                    MatrixPlacement::new(MeasureId(100)),
                    vec![],
                );
                m.add_view(v);
            },
            |m| {
                let f = ExternalFn {
                    name: "f".into(),
                    language: Language::Python,
                    body: "1".into(),
                    arg_types: vec![],
                    return_type: ValueType::Number,
                    pure: true,
                };
                m.external_fns.insert("f".into(), f);
            },
            |m| {
                let c = ExternalCall {
                    func: "f".into(),
                    arg_measures: vec![],
                    refresh_policy: RefreshPolicy::Manual,
                };
                m.external_calls.insert(MeasureId(100), c);
            },
            |m| {
                let s = Scenario {
                    id: ScenarioId(1),
                    name: Name("s".into()),
                    overrides: Default::default(),
                };
                m.scenarios.insert(ScenarioId(1), s);
            },
            |m| {
                let l = NamedLambda {
                    name: Name("L".into()),
                    params: vec!["x".into()],
                    body: Expr::Literal(Value::Number(1.0)),
                };
                m.lambdas.insert(1, l);
            },
        ];
        for (i, edit) in edits.iter().enumerate() {
            let mut new = old.clone();
            edit(&mut new);
            assert_ne!(new, old, "edit {i} changed nothing");
            let c = ChangeSet::between(&old, &new);
            assert!(c.structure && c.cells.is_empty(), "edit {i}: {c:?}");
        }
    }

    /// Every delegating method, plus Conflict mapping, on one open store.
    fn exercise(target: &str) {
        let m = model();
        let mut a = Store::open(target).unwrap();
        let mut b = Store::open(target).unwrap();
        a.save_model(&m).unwrap();
        assert_eq!(a.load_model().unwrap(), m);
        assert_eq!(a.reload_model().unwrap(), m);
        assert_eq!(a.load_partial(&[MeasureId(100)]).unwrap(), m);
        assert_eq!(a.loaded_tx(), Some(a.tx_high_water().unwrap()));
        b.set_loaded_tx(Some(-1));
        let e = b.save_model(&m).unwrap_err();
        assert!(matches!(e, StoreError::Conflict(_)), "{e}");
        assert!(e.to_string().starts_with("conflict:"));

        // An incremental save, carried across handles by a moved token.
        let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10))]);
        let mut m2 = m.clone();
        m2.set_input(MeasureId(100), at.clone(), Value::Number(4.0));
        let mut c = Store::open(target).unwrap();
        c.adopt_save_token(a.take_save_token());
        c.save_cells(&m2, &[(MeasureId(100), at.clone())]).unwrap();
        let token = c.take_save_token();
        assert_eq!(token.tx(), Some(c.tx_high_water().unwrap()));
        assert_eq!(Store::open(target).unwrap().load_model().unwrap(), m2);
        let e = b
            .save_cells(&m, &[(MeasureId(100), at.clone())])
            .unwrap_err();
        assert!(matches!(e, StoreError::Conflict(_)), "{e}");

        // Change sets, cells only and with structure, either backend.
        let mut m3 = m2.clone();
        m3.inputs.remove(&(MeasureId(100), at.clone()));
        let mut d = Store::open(target).unwrap();
        d.adopt_save_token(token);
        d.save_changes(&m3, &ChangeSet::between(&m2, &m3)).unwrap();
        let mut m4 = m3.clone();
        m4.add_item(ItemId(11), CategoryId(1), "Q2");
        m4.set_input(MeasureId(100), at.clone(), Value::Number(2.0));
        d.save_changes(&m4, &ChangeSet::between(&m3, &m4)).unwrap();
        assert_eq!(Store::open(target).unwrap().load_model().unwrap(), m4);
        let e = b
            .save_changes(&m, &ChangeSet::cell(MeasureId(100), at))
            .unwrap_err();
        assert!(matches!(e, StoreError::Conflict(_)), "{e}");
        // No token: the change set is ignored and the whole model saved.
        let mut fresh = Store::open(target).unwrap();
        fresh.save_changes(&m, &ChangeSet::default()).unwrap();
        assert_eq!(Store::open(target).unwrap().load_model().unwrap(), m);
    }

    #[test]
    fn a_path_opens_mentat() {
        let dir = std::env::temp_dir().join(format!("improv_store_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("m.db");
        let db = db.to_str().unwrap();
        assert!(matches!(Store::open(db).unwrap(), Store::Mentat(_)));
        exercise(db);
        // A non-conflict backend error keeps its backend.
        let mut s = Store::open("").unwrap();
        let mut bad = model();
        bad.measures.get_mut(&MeasureId(100)).unwrap().categories =
            (0..6000).map(CategoryId).collect();
        let e = s.save_model(&bad).unwrap_err();
        assert!(matches!(e, StoreError::Mentat(_)), "{e}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn postgres_urls_dispatch_to_pg_and_need_a_model() {
        assert!(is_postgres("postgres://u@h/d") && is_postgres("postgresql://u@h/d"));
        assert!(!is_postgres("/tmp/postgres.db") && !is_postgres(""));
        for bad in ["postgres://u@localhost/d", "postgresql://u@localhost/d?x=1"] {
            let e = Store::open(bad).err().unwrap();
            assert!(matches!(e, StoreError::Pg(_)), "{e}");
            assert!(e.to_string().contains("?model="), "{e}");
        }
    }

    /// `IMPROV_TEST_PG` (a URL or a `key=value` DSN) as a `postgres://` URL.
    fn pg_url() -> Option<String> {
        let Ok(dsn) = std::env::var("IMPROV_TEST_PG") else {
            println!("skipped: IMPROV_TEST_PG not set");
            return None;
        };
        if is_postgres(&dsn) {
            return Some(dsn);
        }
        let kv: std::collections::HashMap<_, _> = dsn
            .split_whitespace()
            .filter_map(|p| p.split_once('='))
            .collect();
        let get = |k| kv.get(k).copied().unwrap_or_default();
        Some(format!(
            "postgres://{}:{}@{}/{}",
            get("user"),
            get("password"),
            get("host"),
            get("dbname")
        ))
    }

    #[test]
    fn live_postgres_store() {
        let Some(url) = pg_url() else { return };
        let url = format!("{url}?model=store_{}", std::process::id());
        assert!(matches!(Store::open(&url).unwrap(), Store::Pg(_)));
        exercise(&url);
    }
}
