//! The authority: one model, its live engine, and every operation a GUI
//! asks of it, each checked by the [`Policy`] and answered with only what
//! the caller may see. `improv-server` serves it over HTTP; a local GUI
//! could drive it in process. Synchronous and blocking (the store and the
//! engine are): a server calls it on a blocking thread, one at a time.
//!
//! **State.** The model as last loaded or saved, the store's version it
//! is at, a live [`Engine`] over every enabled derived measure and its
//! snapshot, a log of recent changes (for `GET /v1/changes?since=`), and
//! per-session undo stacks.
//!
//! **Writes.** Every write is a load-check-save: when another writer moved
//! the store, the model is reloaded first (cell edits then go through,
//! checked against the fresh model: they never conflict); a structural
//! edit whose save the store refuses is a conflict.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use improv_client::diagnostics;
use improv_client::wire::{
    At, CategoryOut, CellEdit, CellOut, Cells as Window, ChangeEvent, ChangedCell, DefineReq,
    DiagnosticOut, Done, EditReq, Failure, ItemOut, LambdaOut, MeasureOut, NameReq, Request,
    Response, Structure, UpdateMeasureReq, WindowReq, WireValue,
};
use improv_client::wire::{Capabilities, MeasureCaps};
use improv_core_model::{
    parser, CategoryId, Coordinate, Expr, Filter, ItemId, MatrixPlacement, Measure, MeasureId,
    MeasureKind, Model, Name, Value, ValueErrorKind, ValueType, View, ViewId,
};
use improv_engine::session::{Edit, Engine, Snapshot};
use improv_engine::{decode_coord, encode_coord, CellValue, CoordKey};
use improv_nl_formula::{describe_formula, parse_nl_formula, NlContext};
use improv_store::{ChangeSet, Store, StoreError};

use std::sync::Arc;

use crate::policy::{Access, Action, Gate, Open, Policy, Principal, Resource};

/// Undo steps kept per session.
const UNDO_DEPTH: usize = 100;
/// Changes kept for clients that reconnect (`?since=`); past this a
/// client reloads.
const LOG_DEPTH: usize = 1024;
/// Most edits in one batch.
pub const MAX_EDITS: usize = 10_000;
/// Most cells one window returns.
pub const MAX_WINDOW: usize = 200_000;

/// A request refused: the HTTP status it maps to and the body.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub status: u16,
    pub body: Failure,
}

impl Refusal {
    fn new(status: u16, error: impl Into<String>) -> Self {
        Refusal {
            status,
            body: Failure {
                error: error.into(),
                ..Default::default()
            },
        }
    }
    fn bad(error: impl Into<String>) -> Self {
        Self::new(400, error)
    }
    /// A hidden thing and a missing one get exactly this (decision 6).
    fn not_found(what: &str, id: u32) -> Self {
        Self::new(404, format!("no {what} {id}"))
    }
    fn forbidden() -> Self {
        Self::new(403, "not allowed")
    }
    fn conflict(e: impl Into<String>) -> Self {
        Self::new(409, e)
    }
    fn internal(e: impl Into<String>) -> Self {
        Self::new(500, e)
    }
    fn at(mut self, index: usize) -> Self {
        self.body.index = Some(index);
        self
    }
    fn located(mut self, d: &diagnostics::Diagnostic) -> Self {
        self.body.diagnostic = Some(DiagnosticOut::from(d));
        self
    }
}

impl From<StoreError> for Refusal {
    fn from(e: StoreError) -> Self {
        match e {
            StoreError::Conflict(m) => Refusal::conflict(format!(
                "another writer changed the model ({m}); reload and try again"
            )),
            e => Refusal::internal(e.to_string()),
        }
    }
}

type Answer<T> = Result<T, Refusal>;

/// Whether the caller may edit all, some or none of input measure `id`'s
/// cells. Checks every cell of its grid: a session answers each from its
/// class cache, so this is a few lookups a cell.
// shortcut: walks the measure's whole grid; per class (the policy-named
// items) would be enough, if a huge grid makes capabilities slow.
fn edit_cells(g: &Gate, m: &Model, id: MeasureId) -> &'static str {
    let cats = &m.measures[&id].categories;
    let mut coords = vec![Coordinate::new()];
    for c in cats {
        let items = m
            .categories
            .get(c)
            .map(|x| x.items.clone())
            .unwrap_or_default();
        coords = coords
            .into_iter()
            .flat_map(|co| items.iter().map(move |i| co.clone().with(*c, *i)))
            .collect();
    }
    let visible: Vec<&Coordinate> = coords.iter().filter(|c| g.sees_cell(id, c)).collect();
    let editable = visible.iter().filter(|c| g.edits_cell(id, c)).count();
    match editable {
        0 => "none",
        n if n == visible.len() => "all",
        _ => "some",
    }
}

/// One undo step: the model before a change (without its cells for a
/// structural one), or the cells a batch replaced.
enum Step {
    /// The structure before (no cells), and the cells the change dropped.
    Structure(Box<Model>, Cells),
    /// The cells before a batch, and as it set them: undo writes the
    /// first, redo the second, whatever others wrote between.
    Cells(Cells, Cells),
}

type Cells = Vec<(MeasureId, Coordinate, Option<Value>)>;

/// One logged change: its version and the cells it changed (`None`: the
/// structure changed, every client reloads).
struct Logged {
    version: String,
    cells: Option<Vec<(MeasureId, CoordKey, Option<WireValue>)>>,
}

/// The authority over one model.
pub struct Authority {
    target: String,
    policy: Box<dyn Policy>,
    model: Model,
    /// The store's high-water `model` is at.
    tx: Option<i64>,
    /// A store handle kept for the high-water check and saves; reopened
    /// after an error.
    store: Option<Store>,
    engine: Option<Engine>,
    snapshot: Snapshot,
    log: VecDeque<Logged>,
    undo: HashMap<String, (Vec<Step>, Vec<Step>)>,
    /// Bumped whenever the structure changes (a load, a structural
    /// change): what a policy keys its sessions by.
    structure_version: u64,
    /// Loads performed, so tests can see the cache work.
    pub loads: u64,
    /// High-water checks performed ([`Authority::refresh`]), so tests can
    /// count store reads.
    pub probes: u64,
}

impl Authority {
    /// Open the model at `target` (a store path or a `postgres://` URL)
    /// with no policy.
    pub fn open(target: &str) -> Result<Self, String> {
        Self::with_policy(target, Box::new(Open))
    }

    pub fn with_policy(target: &str, policy: Box<dyn Policy>) -> Result<Self, String> {
        let mut a = Authority {
            target: target.to_owned(),
            policy,
            model: Model::new(),
            tx: None,
            store: None,
            engine: None,
            snapshot: Snapshot::default(),
            log: VecDeque::new(),
            undo: HashMap::new(),
            structure_version: 0,
            loads: 0,
            probes: 0,
        };
        a.reload().map_err(|r| r.body.error)?;
        a.push_log(Some(Vec::new()));
        Ok(a)
    }

    /// The model's version: opaque, compared for equality only.
    pub fn version(&self) -> String {
        self.tx.map_or_else(|| "0".into(), |t| t.to_string())
    }

    fn store(&mut self) -> Answer<&mut Store> {
        if self.store.is_none() {
            self.store = Some(Store::open(&self.target).map_err(Refusal::from)?);
        }
        Ok(self.store.as_mut().expect("opened above"))
    }

    /// Reload when another writer moved the store; a no-op otherwise.
    /// Another writer's change is logged as a reload: the authority does
    /// not know which cells it touched.
    pub fn refresh(&mut self) -> Answer<bool> {
        self.probes += 1;
        let hw = match self.store()?.tx_high_water() {
            Ok(hw) => hw,
            Err(e) => {
                self.store = None;
                return Err(e.into());
            }
        };
        if Some(hw) == self.tx {
            return Ok(false);
        }
        self.reload()?;
        self.push_log(None);
        Ok(true)
    }

    fn reload(&mut self) -> Answer<()> {
        let store = self.store()?;
        let model = store.load_model().map_err(Refusal::from)?;
        let tx = store.loaded_tx();
        self.loads += 1;
        self.structure_version += 1;
        self.model = model;
        self.tx = tx;
        self.rebuild()
    }

    /// A new engine over every enabled derived measure.
    /// A model whose formulas do not build (saved by an older release, or
    /// by hand) still serves: its derived cells are blank and each
    /// measure's `error` says why.
    fn rebuild(&mut self) -> Answer<()> {
        self.engine = None;
        self.snapshot = Snapshot::default();
        match build_engine(&self.model) {
            Ok((engine, snap)) => (self.engine, self.snapshot) = (engine, snap),
            Err(e) => eprintln!("{}: the formulas do not build: {e}", self.target),
        }
        Ok(())
    }

    fn push_log(&mut self, cells: Option<Vec<(MeasureId, CoordKey, Option<WireValue>)>>) {
        self.log.push_back(Logged {
            version: self.version(),
            cells,
        });
        while self.log.len() > LOG_DEPTH {
            self.log.pop_front();
        }
    }

    /// The model as it is now (for tests and the server's legacy routes).
    pub fn model(&self) -> &Model {
        &self.model
    }

    /// Serve one request for `who` in `session` (undo is per session).
    pub fn serve(&mut self, who: &Principal, session: &str, req: Request) -> Answer<Response> {
        self.refresh()?;
        if !self.gate(&self.access(who)).sees_model() {
            return Err(Refusal::forbidden());
        }
        Ok(match req {
            Request::Structure => Response::Structure(self.structure(who)),
            Request::Window(m, w) => Response::Cells(self.window(who, m, &w)?),
            Request::Edit(e) => Response::Done(self.edit(who, session, e)?),
            Request::Undo => Response::Done(self.undo(who, session, false)?),
            Request::Redo => Response::Done(self.undo(who, session, true)?),
            other => Response::Done(self.change_structure(who, session, other)?),
        })
    }

    /// `who`'s decisions over the current structure. The policy keeps one
    /// per (structure, principal); see [`Policy::session`].
    fn access(&self, who: &Principal) -> Arc<dyn Access> {
        self.policy
            .session(&self.model, self.structure_version, who)
    }

    fn gate<'a>(&'a self, access: &'a Arc<dyn Access>) -> Gate<'a> {
        Gate {
            access: &**access,
            model: &self.model,
        }
    }

    // -- reads ------------------------------------------------------------

    /// What `who` may see of the structure.
    pub fn structure(&self, who: &Principal) -> Structure {
        let access = self.access(who);
        let g = self.gate(&access);
        let m = &self.model;
        let mut cats: Vec<&improv_core_model::Category> = m.categories.values().collect();
        cats.sort_by_key(|c| c.id.0);
        let categories = cats
            .into_iter()
            .map(|c| CategoryOut {
                id: c.id.0,
                name: c.name.0.clone(),
                items: grid_items(m, c.id)
                    .into_iter()
                    .filter(|i| g.sees_item(*i))
                    .map(|i| ItemOut {
                        id: i.0,
                        name: m.items[&i].name.0.clone(),
                    })
                    .collect(),
            })
            .collect();
        let mut ids: Vec<MeasureId> = m.measures.keys().copied().collect();
        ids.sort_by_key(|i| i.0);
        let measures = ids
            .into_iter()
            .filter(|id| g.sees_measure(*id))
            .map(|id| self.measure_out(&g, id))
            .collect();
        let lambdas = m
            .lambdas
            .iter()
            .map(|(id, l)| LambdaOut {
                id: *id,
                name: l.name.0.clone(),
                params: l.params.clone(),
            })
            .collect();
        let mut views: Vec<View> = m
            .views
            .values()
            .filter_map(|v| visible_view(&g, v))
            .collect();
        views.sort_by_key(|v| v.id.0);
        Structure {
            version: self.version(),
            categories,
            measures,
            lambdas,
            views,
        }
    }

    fn measure_out(&self, g: &Gate, id: MeasureId) -> MeasureOut {
        let x = &self.model.measures[&id];
        let derived = match &x.kind {
            MeasureKind::Derived(f) => Some(f),
            MeasureKind::Input => None,
        };
        let shown = g.sees_formula(id);
        let filled =
            self.model.sql_sources.contains_key(&id) || self.model.external_calls.contains_key(&id);
        MeasureOut {
            id: id.0,
            name: x.name.0.clone(),
            value_type: x.value_type,
            kind: if derived.is_some() {
                "derived"
            } else {
                "input"
            }
            .into(),
            categories: x.categories.iter().map(|c| c.0).collect(),
            formula: derived
                .filter(|_| shown)
                .map(|f| formula_text(&self.model, f)),
            error: derived.filter(|_| shown).and_then(|f| {
                let ctx = improv_engine::compiler::CompileContext::new(&self.model.measures);
                improv_engine::compiler::compile_formula(&ctx, id, f)
                    .err()
                    .map(|e| e.to_string())
            }),
            format: x.format,
            // shortcut: input and not filled from a source; per-cell rights
            // come with the capabilities route (Cedar stage 2).
            editable: derived.is_none() && !filled,
        }
    }

    /// What `who` may do, for a GUI to grey out what it cannot (hints only:
    /// every request is checked again). The model-wide actions; for each
    /// visible measure whether it may be redefined and whether its cells
    /// may be edited (`some` when only some of them may); and the visible
    /// items that may be renamed or deleted. `None` when `who` may not open
    /// the model.
    pub fn capabilities(&mut self, who: &Principal) -> Answer<Capabilities> {
        self.refresh()?;
        let access = self.access(who);
        let g = self.gate(&access);
        if !g.sees_model() {
            return Err(Refusal::forbidden());
        }
        let m = &self.model;
        let model_wide = |a| g.allows(a, &Resource::Model);
        let mut ids: Vec<MeasureId> = m.measures.keys().copied().collect();
        ids.sort_by_key(|i| i.0);
        let measures = ids
            .into_iter()
            .filter(|id| g.sees_measure(*id))
            .map(|id| {
                let x = &m.measures[&id];
                let filled = m.sql_sources.contains_key(&id) || m.external_calls.contains_key(&id);
                let cells = match (x.is_input() && !filled, access.restricted()) {
                    (false, _) => "none",
                    (true, false) => "all",
                    (true, true) => edit_cells(&g, m, id),
                };
                MeasureCaps {
                    id: id.0,
                    define: g.allows(Action::DefineMeasure, &Resource::Measure(id)),
                    cells: cells.into(),
                }
            })
            .collect();
        let mut items: Vec<u32> = (m.items.keys())
            .filter(|i| g.sees_item(**i))
            .filter(|i| g.allows(Action::EditStructure, &Resource::Item(**i)))
            .map(|i| i.0)
            .collect();
        items.sort_unstable();
        Ok(Capabilities {
            version: self.version(),
            restricted: access.restricted(),
            admin: model_wide(Action::Admin),
            add_measure: model_wide(Action::DefineMeasure),
            edit_structure: model_wide(Action::EditStructure),
            manage_views: model_wide(Action::ManageViews),
            measures,
            edit_items: items,
        })
    }

    /// The visible cells of measure `id` at the items `w` names.
    pub fn window(&self, who: &Principal, id: u32, w: &WindowReq) -> Answer<Window> {
        let access = self.access(who);
        let g = self.gate(&access);
        let mid = MeasureId(id);
        if !g.sees_measure(mid) {
            return Err(Refusal::not_found("measure", id));
        }
        let x = &self.model.measures[&mid];
        // The allowed items per category of the measure, in its order.
        let mut lists = Vec::with_capacity(x.categories.len());
        for c in &x.categories {
            let all: BTreeSet<ItemId> = self
                .model
                .categories
                .get(c)
                .map(|k| k.items.iter().copied().collect())
                .unwrap_or_default();
            let want: BTreeSet<ItemId> = match w.at.get(&c.0) {
                Some(ids) => ids
                    .iter()
                    .map(|i| ItemId(*i))
                    .filter(|i| all.contains(i))
                    .collect(),
                None => all,
            };
            lists.push(
                want.into_iter()
                    .filter(|i| g.sees_item(*i))
                    .collect::<Vec<_>>(),
            );
        }
        let n: usize = lists.iter().map(Vec::len).product();
        if n > MAX_WINDOW {
            return Err(Refusal::bad(format!(
                "a window of {n} cells; ask for at most {MAX_WINDOW}"
            )));
        }
        let mut cells = Vec::new();
        let derived = x.is_derived();
        let values = self.snapshot.get(&mid);
        let mut at: Vec<(CategoryId, ItemId)> = Vec::with_capacity(lists.len());
        cartesian(&x.categories, &lists, &mut at, &mut |at| {
            let mut sorted = at.to_vec();
            sorted.sort();
            let key: CoordKey = sorted.iter().map(|(c, i)| (c.0, i.0)).collect();
            let value = if derived {
                values.and_then(|v| v.get(&key)).map(wire_of_cell)
            } else {
                self.model
                    .inputs
                    .get(&(mid, decode_coord(&key)))
                    .map(WireValue::from_value)
            };
            if let Some(value) = value.filter(|_| g.sees_cell(mid, &decode_coord(&key))) {
                cells.push(CellOut {
                    at: key.to_vec(),
                    value,
                });
            }
        });
        Ok(Window {
            version: self.version(),
            measure: id,
            cells,
        })
    }

    // -- cell edits -------------------------------------------------------

    /// Apply `req`'s edits together, or none: each must name a cell `who`
    /// may edit, with a value of its measure's type. A refusal names the
    /// edit's index; a hidden cell is refused as a missing one.
    fn edit(&mut self, who: &Principal, session: &str, req: EditReq) -> Answer<Done> {
        if req.edits.len() > MAX_EDITS {
            return Err(Refusal::bad(format!(
                "{} edits; at most {MAX_EDITS} in one batch",
                req.edits.len()
            )));
        }
        // Another writer between our check and our save: reload, check
        // again, retry. Cell edits never conflict (only structure does).
        let mut tries = 0;
        let (before, after) = loop {
            let mut checked = Vec::with_capacity(req.edits.len());
            for (i, e) in req.edits.iter().enumerate() {
                checked.push(self.check_edit(who, e).map_err(|r| r.at(i))?);
            }
            match self.apply_cells(checked.clone()) {
                Err(r) if r.status == 409 && tries < 3 => {
                    tries += 1;
                    self.refresh()?;
                }
                r => break (r?, checked),
            }
        };
        self.record(session, Step::Cells(before, after));
        Ok(Done {
            version: self.version(),
            message: format!("{} set", plural(req.edits.len(), "cell")),
            id: None,
        })
    }

    fn check_edit(
        &self,
        who: &Principal,
        e: &CellEdit,
    ) -> Answer<(MeasureId, Coordinate, Option<Value>)> {
        let access = self.access(who);
        let g = self.gate(&access);
        let mid = MeasureId(e.measure);
        let gone = || Refusal::not_found("such cell in measure", e.measure);
        let x = self
            .model
            .measures
            .get(&mid)
            .filter(|_| g.sees_measure(mid))
            .ok_or_else(gone)?;
        let at: Vec<(CategoryId, ItemId)> =
            e.at.iter()
                .map(|(c, i)| (CategoryId(*c), ItemId(*i)))
                .collect();
        let coord = Coordinate::from_pairs(at.iter().copied());
        let fits = coord.dims.len() == at.len()
            && coord.dims.len() == x.categories.len()
            && x.categories.iter().all(|c| {
                coord
                    .dims
                    .get(c)
                    .and_then(|i| self.model.items.get(i))
                    .is_some_and(|it| it.category == *c)
            });
        if !fits || !g.sees_cell(mid, &coord) {
            return Err(gone());
        }
        if !x.is_input() {
            return Err(Refusal::bad(format!(
                "{} is computed: edit its formula",
                x.name.0
            )));
        }
        if self.model.sql_sources.contains_key(&mid) || self.model.external_calls.contains_key(&mid)
        {
            return Err(Refusal::bad(format!(
                "{} is filled from its source",
                x.name.0
            )));
        }
        if !g.edits_cell(mid, &coord) {
            return Err(gone());
        }
        let value = e
            .value
            .as_ref()
            .map(|v| v.to_value(x.value_type))
            .transpose()
            .map_err(|m| Refusal::bad(format!("{}: {m}", x.name.0)))?;
        Ok((mid, coord, value))
    }

    /// Set the cells, save them, push them through the engine and log what
    /// changed. Returns the values they replaced.
    fn apply_cells(
        &mut self,
        edits: Vec<(MeasureId, Coordinate, Option<Value>)>,
    ) -> Answer<Vec<(MeasureId, Coordinate, Option<Value>)>> {
        let before = set_cells(&mut self.model, &edits);
        let changes = ChangeSet {
            cells: edits.iter().map(|(m, at, _)| (*m, at.clone())).collect(),
            structure: false,
        };
        let model = std::mem::take(&mut self.model);
        let saved = self.save(&model, &changes);
        self.model = model;
        if let Err(r) = saved {
            set_cells(&mut self.model, &before);
            return Err(r);
        }
        let mut changed: Vec<(MeasureId, CoordKey, Option<WireValue>)> = edits
            .iter()
            .map(|(m, at, v)| (*m, encode_coord(at), v.as_ref().map(WireValue::from_value)))
            .collect();
        let feed: Vec<Edit> = edits
            .iter()
            .map(|(m, at, v)| Edit {
                measure: *m,
                coord: encode_coord(at),
                value: v.as_ref().and_then(|v| {
                    improv_engine::session::input_cell(self.model.measures[m].value_type, v)
                }),
            })
            .collect();
        if let Some(engine) = &mut self.engine {
            // Drop our snapshot first, so the round updates it in place.
            self.snapshot = Snapshot::default();
            match engine
                .apply_delta(feed)
                .and_then(|d| Ok((d, engine.snapshot()?)))
            {
                Ok((delta, snap)) => {
                    self.snapshot = snap;
                    for (m, cells) in delta {
                        for c in cells {
                            changed.push((m, c.coord, c.new.as_ref().map(wire_of_cell)));
                        }
                    }
                }
                Err(e) => {
                    eprintln!("engine: {e}; rebuilding");
                    self.rebuild()?;
                    self.push_log(None);
                    return Ok(before);
                }
            }
        }
        self.push_log(Some(changed));
        Ok(before)
    }

    /// Save `model` over the store's current one. A conflict here (another
    /// writer between our check and our save) is refused; the caller's
    /// next request reloads.
    fn save(&mut self, model: &Model, changes: &ChangeSet) -> Answer<()> {
        let tx = self.tx;
        let store = self.store()?;
        store.set_loaded_tx(tx);
        let r = store.save_changes(model, changes);
        match r {
            Ok(()) => {
                self.tx = self.store.as_ref().and_then(Store::loaded_tx);
                Ok(())
            }
            Err(e) => {
                self.store = None;
                Err(e.into())
            }
        }
    }

    // -- structure --------------------------------------------------------

    fn change_structure(&mut self, who: &Principal, session: &str, req: Request) -> Answer<Done> {
        let (candidate, message, id) = self.plan(who, req)?;
        self.publish(session, candidate, message, id)
    }

    /// Adopt `candidate` (a model without cells): check that it builds,
    /// save it, rebuild the engine, record an undo step, log a reload.
    fn publish(
        &mut self,
        session: &str,
        candidate: Model,
        message: String,
        id: Option<u32>,
    ) -> Answer<Done> {
        let (previous, dropped) = self.adopt(candidate, Vec::new())?;
        self.record(session, Step::Structure(Box::new(previous), dropped));
        Ok(Done {
            version: self.version(),
            message,
            id,
        })
    }

    /// Swap in a structure: keep the cells it still has room for, add
    /// `restore` (the cells an undone change dropped) where none is set,
    /// check it builds, save it. Returns the structure it replaced and the
    /// cells this change dropped.
    fn adopt(&mut self, mut candidate: Model, restore: Cells) -> Answer<(Model, Cells)> {
        let fits = |c: &Model, m: &MeasureId, at: &Coordinate| {
            c.measures.get(m).is_some_and(|x| {
                x.is_input()
                    && at.dims.len() == x.categories.len()
                    && at.dims.iter().all(|(k, i)| {
                        x.categories.contains(k)
                            && c.items.get(i).is_some_and(|it| it.category == *k)
                    })
            })
        };
        let mut dropped = Vec::new();
        for ((m, at), v) in &self.model.inputs {
            if fits(&candidate, m, at) {
                candidate.inputs.insert((*m, at.clone()), v.clone());
            } else {
                dropped.push((*m, at.clone(), Some(v.clone())));
            }
        }
        for (m, at, v) in restore {
            if let (Some(v), true) = (v, fits(&candidate, &m, &at)) {
                candidate.inputs.entry((m, at)).or_insert(v);
            }
        }
        let (engine, snap) = build_engine(&candidate).map_err(Refusal::bad)?;
        let changes = ChangeSet::between(&self.model, &candidate);
        self.save(&candidate, &changes)?;
        let mut previous = std::mem::replace(&mut self.model, candidate);
        self.structure_version += 1;
        self.engine = engine;
        self.snapshot = snap;
        self.push_log(None);
        previous.inputs.clear();
        Ok((previous, dropped))
    }

    /// The structure a request asks for, checked; nothing changes yet.
    fn plan(&self, who: &Principal, req: Request) -> Answer<(Model, String, Option<u32>)> {
        let access = self.access(who);
        let g = self.gate(&access);
        let m = &self.model;
        let mut c = structure_of(m);
        let need = |a: Action, r: Resource| match g.allows(a, &r) {
            true => Ok(()),
            false => Err(Refusal::forbidden()),
        };
        let taken = |name: &str, me: Option<Resource>| {
            let clash = m
                .category_by_name(name)
                .is_some_and(|x| me != Some(Resource::Category(x.id)))
                || m.measure_by_name(name)
                    .is_some_and(|x| me != Some(Resource::Measure(x.id)))
                || m.lambda_by_name(name).is_some();
            match clash {
                true => Err(Refusal::bad(format!(
                    "{name:?} is already a category, measure or LAMBDA"
                ))),
                false => Ok(()),
            }
        };
        let name_of = |n: &NameReq| match n.name.trim() {
            "" => Err(Refusal::bad("type a name")),
            t => Ok(t.to_owned()),
        };
        Ok(match req {
            Request::Define(d) => return self.define(who, d),
            Request::UpdateMeasure(id, u) => return self.update_measure(who, id, u),
            Request::DeleteMeasure(id) => {
                let mid = MeasureId(id);
                if !g.sees_measure(mid) {
                    return Err(Refusal::not_found("measure", id));
                }
                need(Action::DefineMeasure, Resource::Measure(mid))?;
                let users: Vec<&str> = m
                    .measures
                    .values()
                    .filter(|x| x.id != mid)
                    .filter(|x| matches!(&x.kind, MeasureKind::Derived(f) if f.referenced_measures().contains(&mid))
                        || m.external_calls.get(&x.id).is_some_and(|k| k.arg_measures.contains(&mid)))
                    .map(|x| x.name.0.as_str())
                    .collect();
                if !users.is_empty() {
                    let mut users = users;
                    users.sort_unstable();
                    return Err(Refusal::bad(format!(
                        "cannot delete {}: {} still read it",
                        m.measures[&mid].name.0,
                        users.join(", ")
                    )));
                }
                let name = m.measures[&mid].name.0.clone();
                c.measures.remove(&mid);
                c.sql_sources.remove(&mid);
                c.external_calls.remove(&mid);
                for s in c.scenarios.values_mut() {
                    s.overrides.retain(|(x, _), _| *x != mid);
                }
                c.views = std::mem::take(&mut c.views)
                    .into_iter()
                    .filter_map(|(vid, v)| {
                        let mut left = v.matrices().into_iter().filter(|p| p.measure != mid);
                        let primary = left.next()?;
                        Some((
                            vid,
                            View::from_matrices(vid, v.name, primary, left.collect()),
                        ))
                    })
                    .collect();
                (c, format!("deleted measure {name}"), None)
            }
            Request::AddCategory(n) => {
                need(Action::EditStructure, Resource::Model)?;
                let name = name_of(&n)?;
                taken(&name, None)?;
                let id = next_id(m.categories.keys().map(|c| c.0));
                c.add_category(CategoryId(id), name.clone());
                (c, format!("added category {name}"), Some(id))
            }
            Request::RenameCategory(id, n) => {
                let cid = CategoryId(id);
                if !m.categories.contains_key(&cid) {
                    return Err(Refusal::not_found("category", id));
                }
                need(Action::EditStructure, Resource::Category(cid))?;
                let name = name_of(&n)?;
                taken(&name, Some(Resource::Category(cid)))?;
                let old = std::mem::replace(
                    &mut c.categories.get_mut(&cid).expect("checked").name,
                    Name(name.clone()),
                );
                (c, format!("renamed category {} to {name}", old.0), None)
            }
            Request::DeleteCategory(id) => {
                let cid = CategoryId(id);
                let Some(cat) = m.categories.get(&cid) else {
                    return Err(Refusal::not_found("category", id));
                };
                need(Action::EditStructure, Resource::Category(cid))?;
                // A hidden user would be named: say only that one exists.
                let used = m.measures.values().any(|x| {
                    x.categories.contains(&cid)
                        || matches!(&x.kind, MeasureKind::Derived(f) if names_category(&f.expr, cid))
                });
                if used {
                    let mut users: Vec<&str> = m
                        .measures
                        .values()
                        .filter(|x| g.sees_measure(x.id))
                        .filter(|x| x.categories.contains(&cid) || matches!(&x.kind, MeasureKind::Derived(f) if names_category(&f.expr, cid)))
                        .map(|x| x.name.0.as_str())
                        .collect();
                    users.sort_unstable();
                    return Err(Refusal::bad(format!(
                        "cannot delete {}: measures still use it{}",
                        cat.name.0,
                        if users.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", users.join(", "))
                        }
                    )));
                }
                for i in &cat.items {
                    c.items.remove(i);
                }
                c.categories.remove(&cid);
                (c, format!("deleted category {}", cat.name.0), None)
            }
            Request::AddItem(cat, n) => {
                let cid = CategoryId(cat);
                let Some(k) = m.categories.get(&cid) else {
                    return Err(Refusal::not_found("category", cat));
                };
                need(Action::EditStructure, Resource::Category(cid))?;
                let name = name_of(&n)?;
                if k.items
                    .iter()
                    .any(|i| m.items.get(i).is_some_and(|it| it.name.0 == name))
                {
                    return Err(Refusal::bad(format!(
                        "{} already has an item {name:?}",
                        k.name.0
                    )));
                }
                let id = next_id(m.items.keys().map(|i| i.0));
                c.add_item(ItemId(id), cid, name.clone());
                (c, format!("added {name} to {}", k.name.0), Some(id))
            }
            Request::RenameItem(id, n) => {
                let iid = ItemId(id);
                if !g.sees_item(iid) {
                    return Err(Refusal::not_found("item", id));
                }
                need(Action::EditStructure, Resource::Item(iid))?;
                let name = name_of(&n)?;
                let cat = m.items[&iid].category;
                if m.categories[&cat]
                    .items
                    .iter()
                    .any(|j| *j != iid && m.items.get(j).is_some_and(|it| it.name.0 == name))
                {
                    return Err(Refusal::bad(format!(
                        "{} already has an item {name:?}",
                        m.categories[&cat].name.0
                    )));
                }
                let old = std::mem::replace(
                    &mut c.items.get_mut(&iid).expect("checked").name,
                    Name(name.clone()),
                );
                (c, format!("renamed item {} to {name}", old.0), None)
            }
            Request::DeleteItem(id) => {
                let iid = ItemId(id);
                if !g.sees_item(iid) {
                    return Err(Refusal::not_found("item", id));
                }
                need(Action::EditStructure, Resource::Item(iid))?;
                let it = c.items.remove(&iid).expect("checked");
                if let Some(k) = c.categories.get_mut(&it.category) {
                    k.items.retain(|j| *j != iid);
                }
                for s in c.scenarios.values_mut() {
                    s.overrides
                        .retain(|(_, at), _| !at.dims.values().any(|j| *j == iid));
                }
                for v in c.views.values_mut() {
                    scrub_item(&mut v.filters, &mut v.page_items, iid);
                    for p in &mut v.placements {
                        scrub_item(&mut p.filters, &mut p.page_items, iid);
                    }
                }
                (c, format!("deleted item {}", it.name.0), None)
            }
            Request::SaveView(v) => {
                need(Action::ManageViews, Resource::Model)?;
                let name = v
                    .name
                    .as_deref()
                    .map(str::trim)
                    .unwrap_or_default()
                    .to_owned();
                if name.is_empty() {
                    return Err(Refusal::bad("a view needs a name"));
                }
                if m.view_by_name(&name).is_some() {
                    return Err(Refusal::bad(format!("a view is already named {name:?}")));
                }
                let id = next_id(m.views.keys().map(|v| v.0));
                let view =
                    self.view_of(&g, ViewId(id), name.clone(), v.matrices.unwrap_or_default())?;
                c.add_view(view);
                (c, format!("saved view {name}"), Some(id))
            }
            Request::UpdateView(id, v) => {
                let vid = ViewId(id);
                let Some(old) = m.views.get(&vid).filter(|x| visible_view(&g, x).is_some()) else {
                    return Err(Refusal::not_found("view", id));
                };
                need(Action::ManageViews, Resource::View(ViewId(id)))?;
                let name = v
                    .name
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or(&old.name.0)
                    .to_owned();
                if m.view_by_name(&name).is_some_and(|x| x.id != vid) {
                    return Err(Refusal::bad(format!("a view is already named {name:?}")));
                }
                let matrices = v.matrices.unwrap_or_else(|| old.matrices());
                let view = self.view_of(&g, vid, name.clone(), matrices)?;
                c.add_view(view);
                (c, format!("saved view {name}"), None)
            }
            Request::DeleteView(id) => {
                let vid = ViewId(id);
                let Some(old) = m.views.get(&vid).filter(|x| visible_view(&g, x).is_some()) else {
                    return Err(Refusal::not_found("view", id));
                };
                need(Action::ManageViews, Resource::View(ViewId(id)))?;
                c.views.remove(&vid);
                (c, format!("deleted view {}", old.name.0), None)
            }
            Request::Structure
            | Request::Window(..)
            | Request::Edit(_)
            | Request::Undo
            | Request::Redo => return Err(Refusal::bad("not a structural change")),
        })
    }

    /// A view of these matrices, each naming a visible measure and items.
    fn view_of(
        &self,
        g: &Gate,
        id: ViewId,
        name: String,
        matrices: Vec<MatrixPlacement>,
    ) -> Answer<View> {
        let mut ms = matrices.into_iter();
        let Some(primary) = ms.next() else {
            return Err(Refusal::bad("a view needs at least one matrix"));
        };
        let check = |p: &MatrixPlacement| -> Answer<()> {
            if !g.sees_measure(p.measure) {
                return Err(Refusal::not_found("measure", p.measure.0));
            }
            let items = p
                .page_items
                .iter()
                .map(|(_, i)| *i)
                .chain(p.filters.iter().flat_map(|f| f.items.iter().copied()));
            for i in items {
                if !g.sees_item(i) {
                    return Err(Refusal::not_found("item", i.0));
                }
            }
            Ok(())
        };
        check(&primary)?;
        let rest: Vec<MatrixPlacement> = ms.collect();
        for p in &rest {
            check(p)?;
        }
        Ok(View::from_matrices(id, Name(name), primary, rest))
    }

    /// `Name = formula` makes a derived measure (or redefines one);
    /// `Name = LAMBDA(…)` a named LAMBDA; `name` alone an input measure.
    fn define(&self, who: &Principal, d: DefineReq) -> Answer<(Model, String, Option<u32>)> {
        let access = self.access(who);
        let g = self.gate(&access);
        let m = &self.model;
        let mut c = structure_of(m);
        if let Some(text) = d.definition.as_deref() {
            let Some((lhs, rhs)) = text.split_once('=') else {
                return Err(Refusal::bad("type Name = formula"));
            };
            let name = lhs.trim();
            let at = text.len() - rhs.trim_start().len();
            let rhs = rhs.trim();
            if rhs
                .get(..6)
                .is_some_and(|h| h.eq_ignore_ascii_case("lambda"))
                && rhs[6..].trim_start().starts_with('(')
            {
                if !g.allows(Action::DefineMeasure, &Resource::Model) {
                    return Err(Refusal::forbidden());
                }
                let parsed = parser::parse_definition(m, text).map_err(|e| {
                    Refusal::bad(e.to_string()).located(&diagnostics::parse_error(m, text, &e))
                })?;
                let parser::Definition::Lambda(lt) = parsed else {
                    return Err(Refusal::bad("type Name = LAMBDA(parameters, body)"));
                };
                if m.measure_by_name(&lt.name.0).is_some()
                    || m.category_by_name(&lt.name.0).is_some()
                {
                    return Err(Refusal::bad(format!(
                        "{:?} is a measure or category; name the LAMBDA differently",
                        lt.name.0
                    )));
                }
                let old = m.lambda_by_name(&lt.name.0).map(|(id, _)| id);
                let id = old.unwrap_or_else(|| next_id(m.lambdas.keys().copied()));
                let lname = lt.name.0.clone();
                c.lambdas.insert(id, lt.lambda);
                let verb = if old.is_some() {
                    "redefined"
                } else {
                    "defined"
                };
                return Ok((c, format!("{verb} LAMBDA {lname}"), Some(id)));
            }
            if name.is_empty() {
                return Err(Refusal::bad("type Name = formula"));
            }
            if let Some(x) = m.measure_by_name(name).filter(|x| g.sees_measure(x.id)) {
                // A redefinition.
                return self
                    .update_measure(
                        who,
                        x.id.0,
                        UpdateMeasureReq {
                            formula: Some(rhs.to_owned()),
                            ..Default::default()
                        },
                    )
                    .map_err(|r| shift(r, at));
            }
            if !g.allows(Action::DefineMeasure, &Resource::Model) {
                return Err(Refusal::forbidden());
            }
            check_free(m, name)?;
            let id = MeasureId(next_id(m.measures.keys().map(|x| x.0)));
            let formula =
                parse_formula(&m.with_provisional(id, name), rhs).map_err(|r| shift(r, at))?;
            let (value_type, cats) = improv_engine::compiler::derived_shape(&c, id, name, &formula)
                .map_err(|e| {
                    let e = e.to_string();
                    Refusal::bad(e.clone())
                        .located(&diagnostics::compile_error(m, rhs, &e).shifted(at))
                })?;
            c.add_measure(Measure {
                id,
                name: Name(name.to_owned()),
                value_type,
                categories: cats,
                kind: MeasureKind::Derived(formula),
                description: None,
                format: None,
            });
            return Ok((c, format!("defined {name}"), Some(id.0)));
        }
        let name = d.name.as_deref().map(str::trim).unwrap_or_default();
        if name.is_empty() {
            return Err(Refusal::bad("send a definition (Name = formula) or a name"));
        }
        if !g.allows(Action::DefineMeasure, &Resource::Model) {
            return Err(Refusal::forbidden());
        }
        check_free(m, name)?;
        let categories = match d.categories {
            Some(ids) => {
                let mut out = Vec::new();
                for id in ids {
                    let cid = CategoryId(id);
                    if !m.categories.contains_key(&cid) {
                        return Err(Refusal::not_found("category", id));
                    }
                    if out.contains(&cid) {
                        return Err(Refusal::bad(format!("category {id} is listed twice")));
                    }
                    out.push(cid);
                }
                out
            }
            None => {
                let mut all: Vec<CategoryId> = m.categories.keys().copied().collect();
                all.sort_by_key(|c| c.0);
                all
            }
        };
        let id = next_id(m.measures.keys().map(|x| x.0));
        c.add_measure(Measure {
            id: MeasureId(id),
            name: Name(name.to_owned()),
            value_type: d.value_type.unwrap_or(ValueType::Number),
            categories,
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
        Ok((c, format!("added input measure {name}"), Some(id)))
    }

    fn update_measure(
        &self,
        who: &Principal,
        id: u32,
        u: UpdateMeasureReq,
    ) -> Answer<(Model, String, Option<u32>)> {
        let access = self.access(who);
        let g = self.gate(&access);
        let m = &self.model;
        let mid = MeasureId(id);
        if !g.sees_measure(mid) {
            return Err(Refusal::not_found("measure", id));
        }
        if !g.allows(Action::DefineMeasure, &Resource::Measure(mid)) {
            return Err(Refusal::forbidden());
        }
        let mut c = structure_of(m);
        let mut said = Vec::new();
        if let Some(name) = u.name.as_deref().map(str::trim) {
            if name.is_empty() {
                return Err(Refusal::bad("type a name"));
            }
            if m.measure_by_name(name).is_some_and(|x| x.id != mid)
                || m.category_by_name(name).is_some()
                || m.lambda_by_name(name).is_some()
            {
                return Err(Refusal::bad(format!(
                    "{name:?} is already a category, measure or LAMBDA"
                )));
            }
            c.measures.get_mut(&mid).expect("visible").name = Name(name.to_owned());
            said.push(format!("renamed to {name}"));
        }
        if let Some(text) = u.formula.as_deref() {
            if !g.sees_formula(mid) && m.measures[&mid].is_derived() {
                return Err(Refusal::forbidden());
            }
            let formula = parse_formula(&c, text)?;
            if let Err(e) = formula.expr.check_limits() {
                return Err(Refusal::bad(e));
            }
            let x = c.measures.get_mut(&mid).expect("visible");
            x.kind = MeasureKind::Derived(formula.clone());
            let name = x.name.0.clone();
            let (value_type, cats) =
                improv_engine::compiler::derived_shape(&c, mid, &name, &formula).map_err(|e| {
                    let e = e.to_string();
                    Refusal::bad(e.clone()).located(&diagnostics::compile_error(m, text, &e))
                })?;
            let x = c.measures.get_mut(&mid).expect("visible");
            x.value_type = value_type;
            x.categories = cats;
            if let Err(e) = check_builds(&c) {
                return Err(
                    Refusal::bad(e.clone()).located(&diagnostics::compile_error(m, text, &e))
                );
            }
            said.push("formula changed".into());
        }
        if let Some(f) = u.format {
            if m.measures[&mid].value_type != ValueType::Number {
                return Err(Refusal::bad(format!(
                    "{} holds no numbers to format",
                    m.measures[&mid].name.0
                )));
            }
            c.measures.get_mut(&mid).expect("visible").format = f;
            said.push(format!("format {}", f.unwrap_or_default()));
        }
        if said.is_empty() {
            return Err(Refusal::bad(
                "nothing to change: send formula, name or format",
            ));
        }
        Ok((
            c,
            format!("{}: {}", m.measures[&mid].name.0, said.join(", ")),
            None,
        ))
    }

    // -- undo ---------------------------------------------------------------

    fn record(&mut self, session: &str, step: Step) {
        let (undo, redo) = self.undo.entry(session.to_owned()).or_default();
        undo.push(step);
        if undo.len() > UNDO_DEPTH {
            undo.remove(0);
        }
        redo.clear();
    }

    /// Undo (or redo) `session`'s last step. A structure step restores the
    /// structure (refused if it no longer builds); a cell step writes the
    /// cells back as they were, each still checked for `who`.
    fn undo(&mut self, who: &Principal, session: &str, redo: bool) -> Answer<Done> {
        let stacks = self.undo.entry(session.to_owned()).or_default();
        let step = match redo {
            false => stacks.0.pop(),
            true => stacks.1.pop(),
        };
        let Some(step) = step else {
            return Err(Refusal::bad(if redo {
                "nothing to redo"
            } else {
                "nothing to undo"
            }));
        };
        let result = match step {
            Step::Structure(s, cells) => {
                let access = self.access(who);
                let g = self.gate(&access);
                if !g.allows(Action::DefineMeasure, &Resource::Model)
                    || !g.allows(Action::EditStructure, &Resource::Model)
                {
                    Err(Refusal::forbidden())
                } else {
                    self.adopt(*s, cells)
                        .map(|(prev, dropped)| Step::Structure(Box::new(prev), dropped))
                }
            }
            Step::Cells(before, after) => {
                let cells = if redo { &after } else { &before };
                let mut checked = Vec::with_capacity(cells.len());
                let mut refused = None;
                for (i, (m, at, v)) in cells.iter().enumerate() {
                    let e = CellEdit {
                        measure: m.0,
                        at: encode_coord(at).to_vec(),
                        value: v.as_ref().map(WireValue::from_value),
                    };
                    match self.check_edit(who, &e) {
                        Ok(x) => checked.push(x),
                        Err(r) => {
                            refused = Some(r.at(i));
                            break;
                        }
                    }
                }
                match refused {
                    Some(r) => Err(r),
                    None => self
                        .apply_cells(checked)
                        .map(|_| Step::Cells(before, after)),
                }
            }
        };
        let inverse = result?;
        let stacks = self.undo.get_mut(session).expect("made above");
        match redo {
            false => stacks.1.push(inverse),
            true => stacks.0.push(inverse),
        }
        Ok(Done {
            version: self.version(),
            message: if redo { "redone" } else { "undone" }.into(),
            id: None,
        })
    }

    // -- changes ----------------------------------------------------------

    /// What changed after version `since`, for `who`: the visible changed
    /// cells, or a reload when the structure changed or `since` is older
    /// than the log. `None` when nothing `who` may see changed.
    pub fn changes_since(&self, who: &Principal, since: &str) -> Option<ChangeEvent> {
        if since == self.version() {
            return None;
        }
        let reload = || {
            Some(ChangeEvent {
                version: self.version(),
                cells: Vec::new(),
                reload: true,
            })
        };
        let Some(start) = self.log.iter().position(|l| l.version == since) else {
            return reload();
        };
        let access = self.access(who);
        let g = self.gate(&access);
        let mut cells: BTreeMap<(u32, At), Option<WireValue>> = BTreeMap::new();
        for l in self.log.iter().skip(start + 1) {
            let Some(cs) = &l.cells else {
                return reload();
            };
            for (m, key, v) in cs {
                if g.sees_cell(*m, &decode_coord(key)) {
                    cells.insert((m.0, key.to_vec()), v.clone());
                }
            }
        }
        if cells.is_empty() {
            return None;
        }
        Some(ChangeEvent {
            version: self.version(),
            cells: cells
                .into_iter()
                .map(|((measure, at), value)| ChangedCell { measure, at, value })
                .collect(),
            reload: false,
        })
    }
}

/// `f`, as the formula editor shows it: the DSL it was typed in when that
/// prints back to the same formula, else controlled English.
pub fn formula_text(m: &Model, f: &improv_core_model::Formula) -> String {
    improv_client::dsl::formula(m, f)
        .filter(|t| parse_formula(m, t).ok().as_ref() == Some(f))
        .unwrap_or_else(|| describe_formula(&NlContext::new(m), f))
}

/// Parse formula text (the DSL, else controlled English), the error located
/// in `text`.
fn parse_formula(m: &Model, text: &str) -> Answer<improv_core_model::Formula> {
    parser::parse_expr(m, text)
        .or_else(|e| parse_nl_formula(&NlContext::new(m), text).map_err(|_| e))
        .map_err(|e| Refusal::bad(e.to_string()).located(&diagnostics::parse_error(m, text, &e)))
}

/// A refusal located in the formula after `Name = `: its span moved `by`.
fn shift(mut r: Refusal, by: usize) -> Refusal {
    if let Some(d) = &mut r.body.diagnostic {
        d.span = d.span.map(|(a, b)| (a + by, b + by));
    }
    r
}

fn check_free(m: &Model, name: &str) -> Answer<()> {
    if m.measure_by_name(name).is_some()
        || m.category_by_name(name).is_some()
        || m.lambda_by_name(name).is_some()
    {
        return Err(Refusal::bad(format!(
            "{name:?} is already a category, measure or LAMBDA"
        )));
    }
    Ok(())
}

/// Whether `m`'s structure builds an engine.
fn check_builds(m: &Model) -> Result<(), String> {
    build_engine(&structure_of(m)).map(drop)
}

/// A live engine over `m`'s enabled derived measures (a formula the
/// desktop GUI disabled computes nothing), or none when there are none.
fn build_engine(m: &Model) -> Result<(Option<Engine>, Snapshot), String> {
    const DISABLED: &str = "[improv:formula-disabled]";
    let disabled = |x: &Measure| {
        x.is_derived()
            && x.description
                .as_deref()
                .is_some_and(|d| d == DISABLED || d.starts_with(&format!("{DISABLED} ")))
    };
    let owned;
    let m = if m.measures.values().any(disabled) {
        let mut c = m.clone();
        for x in c.measures.values_mut() {
            if disabled(x) {
                x.kind = MeasureKind::Input;
            }
        }
        owned = c;
        &owned
    } else {
        m
    };
    let derived: Vec<MeasureId> = m
        .measures
        .values()
        .filter(|x| x.is_derived())
        .map(|x| x.id)
        .collect();
    if derived.is_empty() {
        return Ok((None, Snapshot::default()));
    }
    let (e, s) = Engine::new(m, &derived).map_err(|e| e.to_string())?;
    Ok((Some(e), s))
}

/// Set (or clear) each cell; returns what each held before.
fn set_cells(m: &mut Model, cells: &Cells) -> Cells {
    cells
        .iter()
        .map(|(id, at, v)| {
            let key = (*id, at.clone());
            let old = match v {
                Some(v) => m.inputs.insert(key, v.clone()),
                None => m.inputs.remove(&key),
            };
            (*id, at.clone(), old)
        })
        .collect()
}

/// The model without its cells.
fn structure_of(m: &Model) -> Model {
    let mut s = Model::new();
    s.categories.clone_from(&m.categories);
    s.items.clone_from(&m.items);
    s.measures.clone_from(&m.measures);
    s.sql_sources.clone_from(&m.sql_sources);
    s.views.clone_from(&m.views);
    s.external_fns.clone_from(&m.external_fns);
    s.external_calls.clone_from(&m.external_calls);
    s.scenarios.clone_from(&m.scenarios);
    s.lambdas.clone_from(&m.lambdas);
    s
}

/// A category's items in grid order: by id, as the desktop GUI lists them.
fn grid_items(m: &Model, c: CategoryId) -> Vec<ItemId> {
    let mut v: Vec<ItemId> = m
        .categories
        .get(&c)
        .map(|k| {
            k.items
                .iter()
                .copied()
                .filter(|i| m.items.contains_key(i))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by_key(|i| i.0);
    v
}

/// `v` with only what `g` may see: matrices of visible measures, filters
/// and pages of visible items. `None` when no matrix is left.
fn visible_view(g: &Gate, v: &View) -> Option<View> {
    let mut left = v
        .matrices()
        .into_iter()
        .filter(|p| g.sees_measure(p.measure))
        .map(|mut p| {
            p.page_items.retain(|(_, i)| g.sees_item(*i));
            for f in &mut p.filters {
                f.items.retain(|i| g.sees_item(*i));
            }
            p
        });
    let primary = left.next()?;
    Some(View::from_matrices(
        v.id,
        v.name.clone(),
        primary,
        left.collect(),
    ))
}

fn scrub_item(filters: &mut [Filter], pages: &mut Vec<(CategoryId, ItemId)>, i: ItemId) {
    for f in filters {
        f.items.retain(|j| *j != i);
    }
    pages.retain(|(_, j)| *j != i);
}

fn names_category(e: &Expr, c: CategoryId) -> bool {
    match e {
        Expr::Category(x) => *x == c,
        Expr::Ref(_, d) => d.by.contains(&c) || d.over.contains(&c) || d.except.contains(&c),
        Expr::UnaryOp(_, a) => names_category(a, c),
        Expr::BinaryOp(_, a, b) => names_category(a, c) || names_category(b, c),
        Expr::Call(_, a) => a.iter().any(|x| names_category(x, c)),
        Expr::Literal(_) => false,
    }
}

fn next_id(ids: impl Iterator<Item = u32>) -> u32 {
    ids.max().map_or(1, |m| m + 1)
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// One category and its item: a coordinate's part.
type Pair = (CategoryId, ItemId);

/// Every combination of one item per list, in row-major order.
fn cartesian(
    cats: &[CategoryId],
    lists: &[Vec<ItemId>],
    at: &mut Vec<(CategoryId, ItemId)>,
    f: &mut dyn FnMut(&[Pair]),
) {
    let k = at.len();
    if k == cats.len() {
        f(at);
        return;
    }
    for i in &lists[k] {
        at.push((cats[k], *i));
        cartesian(cats, lists, at, f);
        at.pop();
    }
}

/// An engine cell on the wire.
pub fn wire_of_cell(v: &CellValue) -> WireValue {
    match v {
        CellValue::Num(b) => WireValue::Number(f64::from_bits(*b)),
        CellValue::Bool(b) => WireValue::Bool(*b),
        CellValue::Text(t) => WireValue::Text(t.clone()),
        CellValue::Date(ms) => WireValue::Date {
            date: chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*ms)
                .map_or_else(|| ms.to_string(), |d| d.to_rfc3339()),
        },
        CellValue::Err(c) => WireValue::Error {
            error: ValueErrorKind::from_error_type(*c)
                .map_or("#ERR", |k| k.code())
                .into(),
        },
    }
}
