//! Access policies in the backend: a model's Cedar policies
//! (`improv_policy`), stored in `improv.policy` and enforced for every role
//! that may not act as the model's owner.
//!
//! - **Who.** The Cedar User is the current role (`SET ROLE` acts as that
//!   role, as privileges do), its Roles every role whose privileges it has
//!   (`pg_has_role(…, 'USAGE')`), its Groups the model's stored groups. A
//!   superuser and any role that may act as the owner
//!   (`improv.may_act_as`) bypass Cedar: nobody can lock the owner out.
//! - **Without policies** a model behaves as in 0.12: its owner acts on it,
//!   the filtered functions refuse everyone else, and the catalog and
//!   result tables keep their 0.12 rules.
//! - **With policies** the catalog rows that name measures, bindings,
//!   views and lambdas are the owner's (`improv.unrestricted`); everyone
//!   else reads the model through [`categories`], [`items`], [`measures`]
//!   and [`cells`], each filtered by Cedar, and the result tables carry a
//!   row-level security policy ([`cell_visible`]) that shows a role only
//!   the cells Cedar lets it read. Hidden is absent: a hidden measure or
//!   item answers as one that does not exist. [`set_cells`] and `define`
//!   let a role Cedar allows edit cells and define measures, as the owner.
//! - **Freshness.** A backend keeps one [`Session`] per model and role,
//!   checked once per statement against `improv.policy.version`,
//!   `improv.model_version` and the model's newest item, so a change made
//!   in another session applies from the next statement.
//! - **Audit.** Every change of the policies, and every change a role
//!   makes through Cedar, adds a row to `improv.policy_audit` in the
//!   change's own transaction. A refusal rolls its transaction back, so it
//!   is written to the server log instead (`pg_improv audit: deny …`).

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::hash::{BuildHasherDefault, Hasher};
use std::rc::Rc;
use std::str::FromStr;
use std::sync::Arc;

use improv_core_model::{parser, CategoryId, Coordinate, ItemId, MeasureId, Model};
use improv_engine::CellValue;
use improv_pg_model as pm;
use improv_policy::{Action, Channel, Policies, Principal, Resource, Session};
use pgrx::prelude::*;
use pgrx::{JsonB, PgLogLevel, PgMemoryContexts, PgSqlErrorCode, PgTriggerOperation};

use super::{owner, req, Res};

extension_sql!(
    r#"
-- A model's access policies (Cedar text and groups JSON, as
-- improv_policy::Policies::parse reads them). No row, or a NULL text: no
-- policies. version moves on with every set and clear; set_policies is the
-- only writer (improv_user may read the rows of the models it may act as
-- the owner of).
CREATE TABLE improv.policy (
    model_id bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    version  bigint NOT NULL,
    text     text,
    groups   text,
    set_by   text NOT NULL,
    set_at   timestamptz NOT NULL
);
-- Every change of a model's policies, and every change a role made through
-- them (set_cells, define, drop_measure). No foreign key: a dropped model's rows stay,
-- for superusers. A refusal is in the server log, not here.
CREATE TABLE improv.policy_audit (
    model_id bigint NOT NULL,
    model    text NOT NULL,
    at       timestamptz NOT NULL,
    who      text NOT NULL,
    action   text NOT NULL,
    resource text NOT NULL,
    detail   jsonb
);
CREATE INDEX policy_audit_model ON improv.policy_audit (model_id, at);
-- Read by those who may act as the owner; written by the extension alone.
ALTER TABLE improv.policy ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.policy FOR SELECT USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
CREATE POLICY write ON improv.policy USING (false);
ALTER TABLE improv.policy_audit ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.policy_audit FOR SELECT USING (improv.may_act_as((SELECT m.owner FROM improv.model m
  WHERE m.id OPERATOR(pg_catalog.=) model_id)));
CREATE POLICY write ON improv.policy_audit USING (false);
SELECT pg_catalog.pg_extension_config_dump('improv.policy', '');
SELECT pg_catalog.pg_extension_config_dump('improv.policy_audit', '');
-- With policies, the catalog rows that name a model's measures, bindings,
-- views, lambdas and sync state are its owner's: everyone else reads the
-- model through the filtered functions. Without, anyone may read them, as
-- in 0.12.
ALTER POLICY read ON improv.category USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.measure USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.binding USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.category_binding USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.view USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.lambda USING (improv.unrestricted(model_id));
ALTER POLICY read ON improv.sync_state USING (improv.unrestricted(model_id));
"#,
    name = "policy_catalog",
    requires = ["check_owner", unrestricted]
);

/// A trigger on `improv.item`: an item renamed or deleted (or the table
/// truncated) may leave result rows with a name the model no longer has,
/// so the model's stale generation moves on ([`clear_stale`]). Written as
/// the extension: a role that may change an item may not write
/// `improv.stale` itself.
#[pg_trigger]
fn item_changed<'a>(
    t: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, PgHeapTupleError> {
    let bumped = (|| -> Res<()> {
        let q = |n: Option<&str>| n.map(pm::quote_ident).ok_or("no transition table");
        // An INSERT adds names (no row names them yet): only the sessions
        // keyed on the items are stale, not the result rows.
        let (models, rows_stale) = match t.op()? {
            // Every model: TRUNCATE has no rows to say which.
            PgTriggerOperation::Truncate => ("SELECT id FROM improv.model".to_string(), 1),
            PgTriggerOperation::Insert => (
                format!(
                    "SELECT model_id FROM {}",
                    q(t.new_transition_table_name()?)?
                ),
                0,
            ),
            // An UPDATE that changed no item (a store's upsert) changes nothing.
            PgTriggerOperation::Update => (
                format!(
                    "SELECT model_id FROM (SELECT model_id, id, category_id, name FROM {} \
                     EXCEPT SELECT model_id, id, category_id, name FROM {}) x",
                    q(t.old_transition_table_name()?)?,
                    q(t.new_transition_table_name()?)?
                ),
                1,
            ),
            _ => (
                format!(
                    "SELECT model_id FROM {}",
                    q(t.old_transition_table_name()?)?
                ),
                1,
            ),
        };
        let sql = format!(
            "INSERT INTO improv.stale AS s (model_id, items) \
             SELECT DISTINCT m, 1 FROM ({models}) x(m) WHERE m IN (SELECT id FROM improv.model) \
             ON CONFLICT (model_id) DO UPDATE SET items = s.items + 1, gen = s.gen + {rows_stale}"
        );
        Spi::connect_mut(|c| {
            if !matches!(t.op()?, PgTriggerOperation::Truncate) {
                // SAFETY: the firing trigger's data, live for this call;
                // registering it exposes its transition tables.
                let rc = unsafe {
                    pg_sys::SPI_register_trigger_data(
                        std::ptr::from_ref(t.trigger_data()).cast_mut(),
                    )
                };
                if rc != pg_sys::SPI_OK_TD_REGISTER as i32 {
                    return Err(format!("SPI_register_trigger_data failed: {rc}").into());
                }
            }
            owner::as_catalog(|| Ok(c.update(&sql, None, &[]).map(drop)?))
        })
    })();
    if let Err(e) = bumped {
        error!("improv.item_changed: {e}");
    }
    Ok(None)
}

extension_sql!(
    r#"
-- Whether a model's result tables may hold rows whose item names the model
-- does not have: gen moves on with every item renamed or deleted
-- (improv.item_changed) and every result row written other than by a
-- rewrite of every result table (improv.result_written); clean_gen is the
-- gen as of the last such rewrite (src/policy.rs, clear_stale). Clean when
-- equal; a model with no row, or a NULL, is not. items moves on with every
-- item added, renamed or deleted: a backend's Cedar sessions are keyed on it.
-- Read and written by the extension alone (the result tables' qual reads
-- it as the extension).
CREATE TABLE improv.stale (
    model_id  bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    items     bigint NOT NULL DEFAULT 0,
    gen       bigint NOT NULL DEFAULT 1,
    clean_gen bigint,
    -- 0.15: moves on with every transaction that applies a model's inputs
    -- or writes its results (src/lib.rs, data_moved): the second number of
    -- its NOTIFY payload.
    data_gen  bigint NOT NULL DEFAULT 0
);
INSERT INTO improv.stale (model_id) SELECT id FROM improv.model;
ALTER TABLE improv.stale ENABLE ROW LEVEL SECURITY;
CREATE POLICY read ON improv.stale FOR SELECT USING (true);
CREATE POLICY write ON improv.stale USING (false);
CREATE TRIGGER stale_items AFTER UPDATE ON improv.item
  REFERENCING OLD TABLE AS improv_old NEW TABLE AS improv_new
  FOR EACH STATEMENT EXECUTE FUNCTION improv.item_changed();
CREATE TRIGGER stale_items_del AFTER DELETE ON improv.item REFERENCING OLD TABLE AS improv_old
  FOR EACH STATEMENT EXECUTE FUNCTION improv.item_changed();
CREATE TRIGGER stale_items_trn AFTER TRUNCATE ON improv.item
  FOR EACH STATEMENT EXECUTE FUNCTION improv.item_changed();
CREATE TRIGGER stale_items_ins AFTER INSERT ON improv.item REFERENCING NEW TABLE AS improv_new
  FOR EACH STATEMENT EXECUTE FUNCTION improv.item_changed();
-- Also under session_replication_role = replica (a logical subscriber, a
-- restore with triggers disabled): a missed rename would leak.
ALTER TABLE improv.item ENABLE ALWAYS TRIGGER stale_items, ENABLE ALWAYS TRIGGER stale_items_del,
  ENABLE ALWAYS TRIGGER stale_items_trn, ENABLE ALWAYS TRIGGER stale_items_ins;
"#,
    name = "stale",
    requires = ["policy_catalog", item_changed]
);

/// A statement trigger on a filtered result table (made by [`secure`]):
/// any write may name an item the model lacks (the owner's own `INSERT`,
/// or a recompute from a load that raced a rename), so the model is stale
/// until [`clear_stale`] checks it again. The model id is the trigger's
/// argument.
#[pg_trigger]
fn result_written<'a>(
    t: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, PgHeapTupleError> {
    let bumped = (|| -> Res<()> {
        let mid: i64 = (t.extra_args()?.first())
            .and_then(|a| a.parse().ok())
            .ok_or("the trigger has no model argument")?;
        bump_stale(mid)
    })();
    if let Err(e) = bumped {
        error!("improv.result_written: {e}");
    }
    Ok(None)
}

/// Mark model `mid` stale: its result rows are not vouched for.
fn bump_stale(mid: i64) -> Res<()> {
    owner::as_catalog(|| {
        Spi::connect_mut(|c| {
            c.update(
                "INSERT INTO improv.stale AS s (model_id) VALUES ($1) \
                 ON CONFLICT (model_id) DO UPDATE SET gen = s.gen + 1",
                None,
                &[mid.into()],
            )?;
            Ok(())
        })
    })
}

/// After a recompute of every derived measure of model `mid` (`model`,
/// named `name`), in this transaction: mark the model clean when no result
/// row names an item the model lacks. Reads the stale generation, then
/// checks each result table against the items in one query (one snapshot,
/// later than the read), then sets `clean_gen` only if `gen` is still what
/// it read: a compare-and-set on the row every rename, delete and result
/// write moves on, so a change committed after the check, or still in
/// flight, leaves the model stale. Not under REPEATABLE READ or
/// SERIALIZABLE, where the compare-and-set would fail its transaction on a
/// concurrent change rather than skip.
pub(crate) fn clear_stale(mid: i64, name: &str, model: &Model) -> Res<()> {
    // SAFETY: a plain read of the transaction's isolation level.
    if unsafe { pg_sys::XactIsoLevel } >= pg_sys::XACT_REPEATABLE_READ as i32 {
        return Ok(());
    }
    owner::as_catalog(|| {
        Spi::connect_mut(|c| {
            c.update(
                "INSERT INTO improv.stale (model_id) VALUES ($1) ON CONFLICT DO NOTHING",
                None,
                &[mid.into()],
            )?;
            let read = c
                .update(
                    "SELECT gen FROM improv.stale WHERE model_id = $1",
                    None,
                    &[mid.into()],
                )?
                .first()
                .get::<i64>(1)?;
            let Some(read) = read else { return Ok(()) };
            for m in model
                .measures
                .values()
                .filter(|m| m.is_derived() && !m.categories.is_empty())
            {
                pm::validate_name("measure", &m.name.0)?;
                let table = pm::result_table(name, &m.name.0);
                let dims = pm::dim_columns(model, m.id)?;
                let known: Vec<String> = (m.categories.iter().zip(&dims))
                    .map(|(cat, d)| {
                        format!(
                            "t.{} IN (SELECT i.name FROM improv.item i \
                             WHERE i.model_id = {mid} AND i.category_id = {})",
                            pm::quote_ident(d),
                            cat.0
                        )
                    })
                    .collect();
                let sql = format!(
                    "SELECT EXISTS (SELECT 1 FROM {table} t WHERE NOT ({})) \
                     FROM (SELECT pg_catalog.to_regclass($1) r) x WHERE x.r IS NOT NULL",
                    known.join(" AND ")
                );
                // A table not made yet holds no row.
                let exists = c
                    .update(
                        "SELECT pg_catalog.to_regclass($1) IS NOT NULL",
                        None,
                        &[table.as_str().into()],
                    )?
                    .first()
                    .get::<bool>(1)?;
                if exists != Some(true) {
                    continue;
                }
                let bad = c
                    .update(&sql, None, &[table.as_str().into()])?
                    .first()
                    .get::<bool>(1)?;
                if bad != Some(false) {
                    return Ok(());
                }
            }
            c.update(
                "UPDATE improv.stale SET clean_gen = $2 WHERE model_id = $1 AND gen = $2",
                None,
                &[mid.into(), read.into()],
            )?;
            Ok(())
        })
    })
}

/// How a role may act on a model, as of one statement.
#[derive(Clone)]
enum Access {
    /// The model has no policies, or the role may act as its owner.
    Owner,
    /// No policies, and the role may not act as the owner: 0.12's rules.
    NotOwner,
    /// Cedar decides.
    Policy(Rc<Restricted>),
    /// The stored policies cannot be used (they no longer validate, or the
    /// model does not build): everything is denied.
    Closed(String),
}

/// One role's Cedar session over one model, with the names a result row
/// and an edit use.
struct Restricted {
    session: Session,
    model: Model,
    /// Each category's items by name.
    names: HashMap<CategoryId, HashMap<String, ItemId>>,
    /// Per measure, what a row check of its result table looks up, built
    /// at its first row (see [`Rows`]).
    rows: RefCell<HashMap<MeasureId, Rc<Rows>>>,
    /// Per measure, its hide lists when its rows can be decided per
    /// category ([`Rows::hide_lists`]).
    fast: RefCell<HashMap<MeasureId, Option<Rc<HideLists>>>>,
    /// (newest item id, item generation) of the catalog this session was
    /// built from, when the build saw exactly that generation's items.
    items: Option<(i64, i64)>,
}

/// FNV-1a: a row check hashes two or three short item names, where
/// SipHash costs more than the rest of the check.
// shortcut: not DoS-resistant; the names are the model's own, written by
// its owner, so a hostile owner slows only its own readers.
#[derive(Default)]
struct Fnv(u64);

impl Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h = match self.0 {
            0 => 0xcbf2_9ce4_8422_2325,
            h => h,
        };
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }
}

/// Per dimension of a measure, the names of the items a role may not
/// read (rule 2 of [`cell_rule`]).
type HideLists = Vec<Vec<String>>;

/// A model's `improv.stale` state: clean, item generation, newest item.
type StaleRow = (Option<bool>, Option<i64>, Option<i64>);

/// One category's item names, as bytes, to (decision class, item).
type Names = HashMap<Box<[u8]>, (u32, ItemId), BuildHasherDefault<Fnv>>;

/// The most class combinations a [`Rows`] keeps a decision for; a row of
/// a combination past it asks the session (which caches it too, at the
/// cost of an allocation).
const DENSE: u64 = 1 << 16;

/// A row check of one measure's result table, for one session, with no
/// allocation per row: each dimension's item names, as bytes, to (decision
/// class, item), and a decision per combination of classes. Class 0 of a
/// dimension is its category's items no policy names, which Cedar decides
/// alike ([`Session::item_named`]); each named item is a class of its own.
/// A name the model lacks has no entry, and its row is hidden, as
/// [`cell_visible`] hides it.
struct Rows {
    measure: MeasureId,
    /// The measure's categories, in its order.
    cats: Vec<CategoryId>,
    /// Per category: item name to (class, item).
    dims: Vec<Names>,
    /// A class's weight in a combination's index (mixed radix; saturated
    /// past `u64`, which is past [`DENSE`]).
    strides: Vec<u64>,
    /// Per combination below [`DENSE`]: 0 not asked yet, 1 hidden, 2 shown.
    decided: Vec<Cell<u8>>,
    /// No row can be shown: a category of the measure has no item the role
    /// may read.
    none: bool,
}

impl Rows {
    fn new(r: &Restricted, m: MeasureId) -> Option<Rows> {
        let cats = r.model.measures.get(&m)?.categories.clone();
        let (mut dims, mut strides, mut none) = (Vec::new(), Vec::new(), false);
        let mut total = 1u64;
        for c in &cats {
            let mut items: Vec<_> = (r.model.items.values())
                .filter(|i| i.category == *c)
                .map(|i| (r.session.item_named(i.id), i))
                .collect();
            // The unnamed items first: one of them decides for all.
            items.sort_by_key(|(named, i)| (*named, i.id));
            none = none || !items.iter().any(|(_, i)| r.session.item_visible(i.id));
            let mut map = HashMap::default();
            let mut named = 0u32;
            for (is_named, i) in items {
                let class = match is_named {
                    true => {
                        named += 1;
                        named
                    }
                    false => 0,
                };
                map.insert(i.name.0.as_bytes().into(), (class, i.id));
            }
            dims.push(map);
            strides.push(total);
            total = total.saturating_mul(u64::from(named) + 1);
        }
        let decided = (0..total.min(DENSE)).map(|_| Cell::new(0)).collect();
        Some(Rows {
            measure: m,
            cats,
            dims,
            strides,
            decided,
            none,
        })
    }

    /// Whether the row whose `n` dimension values `key` gives is shown, as
    /// `session.cell_visible` decides its cell: false for a value the
    /// model has no item for, and for a row of another width.
    fn visible<'a>(&self, session: &Session, n: usize, key: impl Fn(usize) -> &'a [u8]) -> bool {
        if n != self.dims.len() {
            return false;
        }
        let mut at = 0u64;
        for (k, map) in self.dims.iter().enumerate() {
            let Some((class, _)) = map.get(key(k)) else {
                return false;
            };
            at = at.saturating_add(u64::from(*class).saturating_mul(self.strides[k]));
        }
        let slot = usize::try_from(at).ok().and_then(|i| self.decided.get(i));
        if let Some(d) = slot.map(Cell::get).filter(|d| *d != 0) {
            return d == 2;
        }
        // The first row of its combination: ask the session about it.
        let dims = (self.cats.iter().zip(&self.dims).enumerate())
            .filter_map(|(k, (c, map))| map.get(key(k)).map(|(_, i)| (*c, *i)))
            .collect();
        let shown = session.cell_visible(self.measure, &Coordinate { dims });
        if let Some(s) = slot {
            s.set(if shown { 2 } else { 1 });
        }
        shown
    }

    /// Whether no row of the table can be shown: `ViewModel` or
    /// `ReadMeasure` denied, a category with no item the role may read, or
    /// every combination of decision classes (at most [`PROVE`]) hidden.
    /// False when it cannot tell.
    fn none_shown(&self, session: &Session) -> bool {
        if self.none
            || !session.allowed(Action::ViewModel, &Resource::Model)
            || !session.measure_visible(self.measure)
        {
            return true;
        }
        let Some(reps) = self.representatives() else {
            return false;
        };
        let mut at = vec![0usize; reps.len()];
        loop {
            let dims = (self.cats.iter().zip(&reps).zip(&at))
                .map(|((c, r), k)| (*c, r[*k]))
                .collect();
            if session.cell_visible(self.measure, &Coordinate { dims }) {
                return false;
            }
            // The next combination (odometer order); done after the last.
            let mut k = 0;
            loop {
                if k == at.len() {
                    return true;
                }
                at[k] += 1;
                if at[k] < reps[k].len() {
                    break;
                }
                at[k] = 0;
                k += 1;
            }
        }
    }

    /// When a row is shown exactly when each of its items is readable (the
    /// measure readable, and every combination of decision classes, at most
    /// [`PROVE`], shown iff each class's items are readable): per
    /// dimension, the names of its items the role may not read, each list
    /// at most [`SLOTS`] long. `None` otherwise.
    fn hide_lists(&self, r: &Restricted) -> Option<HideLists> {
        let s = &r.session;
        if !s.allowed(Action::ViewModel, &Resource::Model) || !s.measure_visible(self.measure) {
            return None;
        }
        let reps = self.representatives()?;
        let mut at = vec![0usize; reps.len()];
        'combos: loop {
            let items: Vec<ItemId> = reps.iter().zip(&at).map(|(r, k)| r[*k]).collect();
            let dims = self
                .cats
                .iter()
                .copied()
                .zip(items.iter().copied())
                .collect();
            let each = items.iter().all(|i| s.item_visible(*i));
            if s.cell_visible(self.measure, &Coordinate { dims }) != each {
                return None;
            }
            for k in 0..at.len() {
                at[k] += 1;
                if at[k] < reps[k].len() {
                    continue 'combos;
                }
                at[k] = 0;
            }
            break;
        }
        let mut lists = Vec::with_capacity(self.dims.len());
        for (c, map) in self.cats.iter().zip(&self.dims) {
            let mut hidden: Vec<String> = (r.model.items.values())
                .filter(|i| i.category == *c && map.contains_key(i.name.0.as_bytes()))
                .filter(|i| !s.item_visible(i.id))
                .map(|i| i.name.0.clone())
                .take(SLOTS + 1)
                .collect();
            if hidden.len() > SLOTS {
                return None;
            }
            hidden.sort();
            lists.push(hidden);
        }
        Some(lists)
    }

    /// One item of each decision class, per dimension; `None` past
    /// [`PROVE`] combinations. A category with no item at all has none,
    /// and no row (see `none`).
    fn representatives(&self) -> Option<Vec<Vec<ItemId>>> {
        let mut total = 1u64;
        let mut out = Vec::with_capacity(self.dims.len());
        for map in &self.dims {
            let mut by: BTreeMap<u32, ItemId> = BTreeMap::new();
            for (class, i) in map.values() {
                let e = by.entry(*class).or_insert(*i);
                *e = (*e).min(*i);
            }
            total = total.saturating_mul(by.len() as u64);
            out.push(by.into_values().collect());
        }
        (total <= PROVE).then_some(out)
    }
}

impl Restricted {
    /// Measure `m`'s hide lists ([`Rows::hide_lists`]), kept per session.
    fn fast(&self, m: MeasureId) -> Option<Rc<HideLists>> {
        if let Some(f) = self.fast.borrow().get(&m) {
            return f.clone();
        }
        let f = self
            .rows(m)
            .and_then(|rows| rows.hide_lists(self))
            .map(Rc::new);
        self.fast.borrow_mut().insert(m, f.clone());
        f
    }

    /// The row check of measure `m`'s result table; `None` when the model
    /// has no such measure.
    fn rows(&self, m: MeasureId) -> Option<Rc<Rows>> {
        if let Some(r) = self.rows.borrow().get(&m) {
            return Some(r.clone());
        }
        let r = Rc::new(Rows::new(self, m)?);
        self.rows.borrow_mut().insert(m, r.clone());
        Some(r)
    }
    /// The coordinate of a result row: item names in `m`'s category order.
    fn coord(&self, m: MeasureId, key: &[&str]) -> Option<Coordinate> {
        let cats = &self.model.measures.get(&m)?.categories;
        if cats.len() != key.len() {
            return None;
        }
        let mut dims = BTreeMap::new();
        for (c, name) in cats.iter().zip(key) {
            dims.insert(*c, *self.names.get(c)?.get(*name)?);
        }
        Some(Coordinate { dims })
    }

    /// The names of every measure and item this role may not read, for
    /// [`Gate::scrub`].
    fn hidden(&self) -> Vec<String> {
        let measures = (self.model.measures.values())
            .filter(|m| !self.session.measure_visible(m.id))
            .map(|m| m.name.0.clone());
        let items = (self.model.items.values())
            .filter(|i| !self.session.item_visible(i.id))
            .map(|i| i.name.0.clone());
        measures.chain(items).collect()
    }

    /// The model as this role may name it: every category, the items and
    /// measures it may read, every LAMBDA. A name it may not read parses
    /// as one the model lacks.
    fn visible(&self) -> Model {
        let mut v = Model::new();
        for c in self.model.categories.values() {
            v.add_category(c.id, c.name.0.clone());
        }
        let mut items: Vec<_> = self.model.items.values().collect();
        items.sort_by_key(|i| i.id);
        for i in items
            .into_iter()
            .filter(|i| self.session.item_visible(i.id))
        {
            v.add_item(i.id, i.category, i.name.0.clone());
        }
        for m in self.model.measures.values() {
            if self.session.measure_visible(m.id) {
                v.add_measure(m.clone());
            }
        }
        v.lambdas.clone_from(&self.model.lambdas);
        v
    }
}

/// When a cached session was last checked: (transaction start, statement
/// start, this backend's own policy changes, transactions completed as of
/// the statement's snapshot, the transaction's command). A write in this
/// transaction moves the command on; one query's rows share all five.
type Stamp = (i64, i64, u64, u64, u32);

/// What a session was built from: (policy version, model version, newest
/// item id, item generation: `improv.stale.items`, which every item rename
/// and delete moves on) and the principal (its roles change with GRANT and
/// REVOKE).
type Key = ((i64, i64, i64, i64), Principal);

/// A backend's sessions: per (model, role), what it was checked against.
struct Cached {
    stamp: Stamp,
    key: Option<Key>,
    access: Access,
}

thread_local! {
    static CACHE: RefCell<HashMap<(i64, u32), Cached>> = RefCell::new(HashMap::new());
    /// Moved on by this backend's own `set_policies`, which the statement
    /// stamp would miss.
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

/// The stamp a cached session is fresh for: this statement, under this
/// snapshot. A commit by another session moves the snapshot's completion
/// count, so the next statement checks again.
fn stamp() -> Stamp {
    // SAFETY: plain reads of the backend's transaction state; the active
    // snapshot is read only while one is set.
    unsafe {
        let completed = match pg_sys::ActiveSnapshotSet() {
            true => (*pg_sys::GetActiveSnapshot()).snapXactCompletionCount,
            false => 0,
        };
        (
            pg_sys::GetCurrentTransactionStartTimestamp(),
            pg_sys::GetCurrentStatementStartTimestamp(),
            GENERATION.get(),
            completed,
            pg_sys::GetCurrentCommandId(false),
        )
    }
}

/// The current role.
fn me() -> pg_sys::Oid {
    // SAFETY: a plain read of the backend's current user.
    unsafe { pg_sys::GetUserId() }
}

fn role_name(role: pg_sys::Oid) -> String {
    // SAFETY: a syscache lookup; `noerr` returns NULL for a dropped role.
    let p = unsafe { pg_sys::GetUserNameFromId(role, true) };
    if p.is_null() {
        return format!("role {}", role.to_u32());
    }
    // SAFETY: a palloc'd C string from the line above.
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_string_lossy()
        .into_owned()
}

/// Column 1 of the first row of `sql`, read-only (a check in a row-level
/// security policy runs on standbys too) and as the extension, under its
/// pinned `search_path`: a check that runs in the caller's session must
/// not resolve an operator through the caller's schemas. `None` for no row
/// or NULL.
fn one<T: IntoDatum + FromDatum>(sql: &str, args: &[pgrx::datum::DatumWithOid]) -> Res<Option<T>> {
    owner::as_catalog(|| {
        Spi::connect(|c| {
            let t = c.select(sql, Some(1), args)?;
            match t.into_iter().next() {
                Some(r) => Ok(r.get::<T>(1)?),
                None => Ok(None),
            }
        })
    })
}

/// Whether `role` may act as `owner`: a superuser, or a member with SET
/// (any member on PG 15), as `improv.may_act_as` decides for the caller.
fn may_act_as(role: pg_sys::Oid, owner: Option<pg_sys::Oid>) -> Res<bool> {
    // SAFETY: a catalog lookup of an existing role.
    if unsafe { pg_sys::superuser_arg(role) } {
        return Ok(true);
    }
    let Some(owner) = owner else { return Ok(false) };
    Ok(one::<bool>(
        "SELECT pg_catalog.pg_has_role($1, $2, CASE WHEN \
         pg_catalog.current_setting('server_version_num')::int4 >= 160000 \
         THEN 'SET' ELSE 'MEMBER' END)",
        &[role.into(), owner.into()],
    )? == Some(true))
}

/// Model `mid`'s owner, read as the extension; `None` for a model without
/// one (before the 0.11.0 update) or one that does not exist.
fn owner_of(mid: i64) -> Res<Option<pg_sys::Oid>> {
    owner::as_catalog(|| {
        one::<pg_sys::Oid>(
            "SELECT (pg_catalog.to_jsonb(m) ->> 'owner')::pg_catalog.regrole::pg_catalog.oid \
             FROM improv.model m WHERE m.id = $1",
            &[mid.into()],
        )
    })
}

/// A model's row as the catalog has it: id, name, owner.
fn model_row(name: &str) -> Res<Option<(i64, String, Option<pg_sys::Oid>)>> {
    let id = owner::as_catalog(|| {
        one::<i64>(
            "SELECT id FROM improv.model WHERE name = $1",
            &[name.into()],
        )
    })?;
    match id {
        Some(id) => Ok(Some((id, name.to_string(), owner_of(id)?))),
        None => Ok(None),
    }
}

/// Whether the catalog has `improv.policy` (0.13 or later).
fn installed() -> Res<bool> {
    Ok(one::<bool>(
        "SELECT pg_catalog.to_regclass('improv.policy') IS NOT NULL",
        &[],
    )? == Some(true))
}

/// The stored policies of model `mid`: version, and text and groups when
/// set.
fn stored(mid: i64) -> Res<(i64, Option<(String, String)>)> {
    owner::as_catalog(|| {
        if !installed()? {
            return Ok((0, None));
        }
        Spi::connect(|c| {
            let t = c.select(
                "SELECT version, text, coalesce(groups, '') FROM improv.policy WHERE model_id = $1",
                None,
                &[mid.into()],
            )?;
            let Some(r) = t.into_iter().next() else {
                return Ok((0, None));
            };
            let v = req::<i64>(&r, 1)?;
            Ok(match r.get::<String>(2)? {
                Some(text) => (v, Some((text, req(&r, 3)?))),
                None => (v, None),
            })
        })
    })
}

/// How `role` may act on model `mid`, cached for the statement: a check
/// per row of a result table costs a few hash lookups.
fn access(mid: i64, role: pg_sys::Oid) -> Res<Access> {
    let now = stamp();
    let hit = CACHE.with(|c| {
        (c.borrow().get(&(mid, role.to_u32())))
            .filter(|x| x.stamp == now)
            .map(|x| x.access.clone())
    });
    if let Some(a) = hit {
        return Ok(a);
    }
    let owner = owner_of(mid)?;
    let (version, doc) = stored(mid)?;
    let Some((text, groups)) = doc else {
        let a = match may_act_as(role, owner)? {
            true => Access::Owner,
            false => Access::NotOwner,
        };
        remember(mid, role, now, None, a.clone());
        return Ok(a);
    };
    if may_act_as(role, owner)? {
        remember(mid, role, now, None, Access::Owner);
        return Ok(Access::Owner);
    }
    let versions = owner::as_catalog(|| {
        let tx = one::<i64>(
            "SELECT tx FROM improv.model_version WHERE model_id = $1",
            &[mid.into()],
        )?;
        let item = one::<i64>(
            "SELECT max(id) FROM improv.item WHERE model_id = $1",
            &[mid.into()],
        )?;
        Ok((version, tx.unwrap_or(0), item.unwrap_or(0), item_gen(mid)?))
    })?;
    let key = (versions, principal(role)?);
    let kept = CACHE.with(|c| {
        (c.borrow().get(&(mid, role.to_u32())))
            .filter(|x| x.key.as_ref() == Some(&key))
            .map(|x| x.access.clone())
    });
    let a = match kept {
        Some(a) => a,
        None => build(mid, &key, &text, &groups)?,
    };
    remember(mid, role, now, Some(key), a.clone());
    Ok(a)
}

fn remember(mid: i64, role: pg_sys::Oid, stamp: Stamp, key: Option<Key>, a: Access) {
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        // shortcut: drops every session past 256 (models x roles in one
        // backend); an LRU if a backend ever serves that many.
        if c.len() >= 256 {
            c.clear();
        }
        c.insert(
            (mid, role.to_u32()),
            Cached {
                stamp,
                key,
                access: a,
            },
        );
    });
}

/// `role` as a Cedar principal: its name, and every role whose privileges
/// it has.
fn principal(role: pg_sys::Oid) -> Res<Principal> {
    let roles = owner::as_catalog(|| {
        Spi::connect(|c| -> Res<Vec<String>> {
            c.select(
                "SELECT rolname::text FROM pg_catalog.pg_roles \
                 WHERE oid <> $1 AND pg_catalog.pg_has_role($1, oid, 'USAGE') ORDER BY 1",
                None,
                &[role.into()],
            )?
            .map(|r| req::<String>(&r, 1))
            .collect()
        })
    })?;
    Ok(Principal {
        user: role_name(role),
        roles,
    })
}

/// Model `mid`'s item generation (`improv.stale.items`; 0 without a row).
fn item_gen(mid: i64) -> Res<i64> {
    Ok(one::<i64>(
        "SELECT (SELECT items FROM improv.stale WHERE model_id = $1)",
        &[mid.into()],
    )?
    .unwrap_or(0))
}

/// A Cedar session for `key`'s principal over model `mid` and its stored
/// policies.
fn build(mid: i64, key: &Key, text: &str, groups: &str) -> Res<Access> {
    let who = &key.1;
    let policies = match Policies::parse(text, groups) {
        Ok(p) => Arc::new(p),
        Err(e) => {
            return Ok(Access::Closed(format!(
                "the stored policies do not validate: {e}"
            )))
        }
    };
    let model = match owner::as_catalog(|| Ok(pm::build_model(&super::read_catalog(mid)?)?)) {
        Ok(m) => m,
        Err(e) => return Ok(Access::Closed(format!("the model does not load: {e}"))),
    };
    let session = Session::new(Some(policies), &model, who, Channel::Postgres);
    let mut names: HashMap<CategoryId, HashMap<String, ItemId>> = HashMap::new();
    for i in model.items.values() {
        names
            .entry(i.category)
            .or_default()
            .insert(i.name.0.clone(), i.id);
    }
    // The catalog read above saw the items of the key's generation only if
    // no rename or delete committed while it ran: else this session may
    // not stand for the snapshot's items in `cell_rule`.
    let exact = owner::as_catalog(|| item_gen(mid))? == (key.0).3;
    Ok(Access::Policy(Rc::new(Restricted {
        session,
        model,
        names,
        rows: RefCell::default(),
        fast: RefCell::default(),
        items: exact.then_some(((key.0).2, (key.0).3)),
    })))
}

/// A model, and how the current role may act on it.
struct Found {
    mid: i64,
    name: String,
    owner: Option<pg_sys::Oid>,
    access: Access,
}

/// The model's access for the current role, by name; `None` when the
/// model does not exist.
fn lookup(name: &str) -> Res<Option<Found>> {
    let Some((mid, name, owner)) = model_row(name)? else {
        return Ok(None);
    };
    let access = access(mid, me())?;
    Ok(Some(Found {
        mid,
        name,
        owner,
        access,
    }))
}

fn missing(model: &str) -> Box<dyn std::error::Error> {
    format!("improv model {model:?} does not exist").into()
}

/// For a role the policies do not let open the model (`ViewModel`). Model
/// names are public (`improv.models`), so this says no more than they do.
fn unseen(model: &str) -> String {
    format!(
        "permission denied for improv model {model:?}: its access policies do not let you open it"
    )
}

fn not_owner(model: &str, owner: Option<pg_sys::Oid>) -> Box<dyn std::error::Error> {
    let owner = owner.map_or_else(|| "?".into(), role_name);
    format!(
        "permission denied for improv model {model:?}: it has no access policies, so only its \
         owner, {owner}, or a role that may act as it may use it"
    )
    .into()
}

/// What a filtered read works from: the model, and a Cedar session that
/// may open it (`None`: the owner, who sees everything).
struct Reader {
    mid: i64,
    name: String,
    owner: Option<pg_sys::Oid>,
    r: Option<Rc<Restricted>>,
}

/// The [`Reader`] for the current role; refuses a role that may neither
/// act as the owner nor open the model under its policies.
fn reader(model: &str) -> Res<Reader> {
    let f = lookup(model)?.ok_or_else(|| missing(model))?;
    let r = match f.access {
        Access::Owner => None,
        Access::NotOwner => return Err(not_owner(&f.name, f.owner)),
        Access::Closed(why) => return Err(closed(&f.name, &why)),
        Access::Policy(r) if r.session.allowed(Action::ViewModel, &Resource::Model) => Some(r),
        Access::Policy(_) => return Err(unseen(&f.name).into()),
    };
    Ok(Reader {
        mid: f.mid,
        name: f.name,
        owner: f.owner,
        r,
    })
}

fn closed(model: &str, why: &str) -> Box<dyn std::error::Error> {
    format!("improv model {model:?} refuses every request: {why}").into()
}

/// The model's structure (no cells), read as the extension.
fn structure(mid: i64) -> Res<Model> {
    owner::as_catalog(|| Ok(pm::build_model(&super::read_catalog(mid)?)?))
}

/// Row-level security on a restricted model's result tables: whether the
/// current role may read the cell of measure `measure` at `dims` (item
/// names in the measure's category order). True for a model without
/// policies and for a role that may act as its owner; false for a cell
/// Cedar hides, for a name the model has no item for, and for every cell
/// of a model whose policies cannot be used. The reference the result
/// tables' qual ([`cell_rule`], [`row_visible`]) is tested against, and
/// the qual of a measure with no dimensions.
#[pg_extern(stable, security_invoker)]
fn cell_visible(model: i64, measure: i64, dims: pgrx::Array<&str>) -> Res<bool> {
    Ok(match access(model, me())? {
        Access::Owner => true,
        // No policies: the result table has no row-level security of
        // pg_improv's, and this answers as the table's grants do.
        Access::NotOwner => true,
        Access::Closed(_) => false,
        Access::Policy(r) => {
            let key: Option<Vec<&str>> = dims.iter().collect();
            let m = MeasureId(super::id32(measure)?);
            match key.and_then(|k| r.coord(m, &k)) {
                Some(at) => r.session.cell_visible(m, &at),
                None => false,
            }
        }
    })
}

/// The most combinations of decision classes [`cell_rule`] asks the
/// session about to prove that no row of a table can be shown.
const PROVE: u64 = 1024;

/// The most hidden names per dimension the result tables' qual compares a
/// row with (rule 2); more, and its rows are checked one by one.
const SLOTS: usize = 2;

/// The result tables' qual, once per statement (an InitPlan): 0 when every
/// row is shown, 1 when none is, 2 when a row is shown exactly when none
/// of its items is one of [`cell_name`]'s hidden names, 3 when
/// [`row_visible`] decides each row. Each answer is exactly what
/// [`cell_visible`] would say of every row of the table:
///
/// - 0: a model without policies, a role that may act as the owner; or
///   rule 2 with no hidden name.
/// - 1: policies that cannot be used; `ViewModel` or `ReadMeasure`
///   denied; a category of the measure with no item the role may read; or
///   every combination of the items' decision classes denied.
/// - 2: only when the model's result rows name only its items: its stale
///   state, read in this statement's snapshot, says no item was renamed or
///   deleted, and no result row written but by a rewrite of every table,
///   since the last such rewrite ([`clear_stale`]); and this session was
///   built from that very catalog. Else a row whose name the model lacks,
///   which `cell_visible` hides, would be shown.
#[pg_extern(stable, security_invoker)]
fn cell_rule(model: i64, measure: i64) -> Res<i32> {
    Ok(match access(model, me())? {
        Access::Owner | Access::NotOwner => 0,
        Access::Closed(_) => 1,
        Access::Policy(r) => {
            let m = MeasureId(super::id32(measure)?);
            match r.rows(m) {
                None => 1,
                Some(rows) if rows.none_shown(&r.session) => 1,
                Some(_) => match r.fast(m) {
                    Some(lists) if clean(model, r.items)? => {
                        match lists.iter().all(Vec::is_empty) {
                            true => 0,
                            false => 2,
                        }
                    }
                    _ => 3,
                },
            }
        }
    })
}

/// The `slot`th (from 0) hidden name of dimension `dim` (from 0) of
/// measure `measure`'s rows, for rule 2 of [`cell_rule`]; NULL past the
/// last (a slot that hides nothing). NULL too whenever rule 2 does not
/// apply: the qual reads the names only under it.
#[pg_extern(stable, security_invoker)]
fn cell_name(model: i64, measure: i64, dim: i32, slot: i32) -> Res<Option<String>> {
    let Access::Policy(r) = access(model, me())? else {
        return Ok(None);
    };
    let m = MeasureId(super::id32(measure)?);
    let pick = |lists: &[Vec<String>]| {
        let (d, k) = (usize::try_from(dim).ok()?, usize::try_from(slot).ok()?);
        lists.get(d)?.get(k).cloned()
    };
    Ok(r.fast(m).and_then(|l| pick(&l)))
}

/// Whether model `mid`'s result rows name only its items, as of this
/// statement's snapshot: its `improv.stale` row is clean (`gen =
/// clean_gen`), and its item generation and newest item are a session's
/// `items` (so the session's names are the snapshot's). Read-only, in the
/// active snapshot, as the extension. False without a row, for NULLs, and
/// for a session built while its items changed.
fn clean(mid: i64, items: Option<(i64, i64)>) -> Res<bool> {
    let Some((newest, gen)) = items else {
        return Ok(false);
    };
    let row = owner::as_catalog(|| {
        let sql = c"SELECT s.gen OPERATOR(pg_catalog.=) s.clean_gen, s.items, \
            (SELECT pg_catalog.max(i.id) FROM improv.item i WHERE i.model_id OPERATOR(pg_catalog.=) s.model_id) \
            FROM improv.stale s WHERE s.model_id OPERATOR(pg_catalog.=) $1";
        snapshot_row(sql, mid)
    })?;
    Ok(matches!(row, Some((Some(true), Some(g), n)) if g == gen && n.unwrap_or(0) == newest))
}

/// One row of `sql` (bool, int8, int8; `$1` the model id) read in the
/// active snapshot: a read-only SPI execution takes no new snapshot, so
/// the qual's decision and the scan's rows come from one.
fn snapshot_row(sql: &std::ffi::CStr, mid: i64) -> Res<Option<StaleRow>> {
    Spi::connect(|_| {
        // SAFETY: inside an SPI connection; one int8 argument; the result
        // table is read before the connection closes.
        unsafe {
            let mut types = [pg_sys::INT8OID];
            let mut values = [pg_sys::Datum::from(mid)];
            let nulls = [b' ' as std::ffi::c_char];
            let rc = pg_sys::SPI_execute_with_args(
                sql.as_ptr(),
                1,
                types.as_mut_ptr(),
                values.as_mut_ptr(),
                nulls.as_ptr(),
                true,
                1,
            );
            if rc != pg_sys::SPI_OK_SELECT as i32 {
                return Err(format!("reading improv.stale: SPI error {rc}").into());
            }
            let t = pg_sys::SPI_tuptable;
            if t.is_null() || pg_sys::SPI_processed == 0 {
                return Ok(None);
            }
            let (tup, desc) = (*(*t).vals, (*t).tupdesc);
            let get = |k: i32| {
                let mut null = false;
                let d = pg_sys::SPI_getbinval(tup, desc, k, &mut null);
                (!null).then_some(d)
            };
            Ok(Some((
                get(1).map(|d| d.value() != 0),
                get(2).map(|d| d.value() as i64),
                get(3).map(|d| d.value() as i64),
            )))
        }
    })
}

/// What a [`row_visible`] call keeps for its query: the role it decided
/// for and how that role may act on the model, as of the query's first row
/// (as an InitPlan decides once).
struct RowCall {
    role: pg_sys::Oid,
    access: Access,
    rows: Option<Rc<Rows>>,
}

/// `improv.row_visible(model, measure, VARIADIC dims "any")`: the per-row
/// half of the result tables' qual, the answer of [`cell_visible`] without
/// building an array or a coordinate. The dimension values are `text`
/// arguments, in the measure's category order. The first call of a query
/// finds the session and the measure's [`Rows`] and keeps them in the
/// call's `fn_extra`; each row then hashes its names and reads one
/// decision. A value the model has no item for hides its row.
///
/// # Safety
/// Called by Postgres's fmgr with a valid `fcinfo`, as the SQL declares it
/// (STRICT: no argument is NULL).
#[pg_guard]
#[unsafe(no_mangle)]
pub unsafe extern "C-unwind" fn row_visible_wrapper(
    fcinfo: pg_sys::FunctionCallInfo,
) -> pg_sys::Datum {
    // SAFETY: fmgr passes a valid fcinfo with flinfo set; `nargs` arguments.
    let (flinfo, nargs) = unsafe {
        (
            (*fcinfo).flinfo,
            usize::try_from((*fcinfo).nargs).unwrap_or(0),
        )
    };
    // SAFETY: as above.
    let arg = |i: usize| unsafe { (*fcinfo).args.as_slice(nargs)[i].value };
    let role = me();
    // SAFETY: fn_extra is ours: null, or the RowCall set below, which lives
    // as long as fn_mcxt.
    let mut call = unsafe { (*flinfo).fn_extra.cast::<RowCall>() };
    if call.is_null() || unsafe { (*call).role } != role {
        // SAFETY: flinfo is the live call's; checking its argument types
        // before any is read as text.
        let ok = unsafe { !pg_sys::get_fn_expr_variadic(flinfo) }
            && nargs >= 3
            && (2..nargs).all(
                |i| unsafe { pg_sys::get_fn_expr_argtype(flinfo, i as i32) } == pg_sys::TEXTOID,
            );
        if !ok {
            error!(
                "improv.row_visible takes a model, a measure and the row's text dimension values"
            );
        }
        let (mid, measure) = (arg(0).value() as i64, arg(1).value() as i64);
        let found = (|| -> Res<RowCall> {
            let access = access(mid, role)?;
            let rows = match &access {
                Access::Policy(r) => r.rows(MeasureId(super::id32(measure)?)),
                _ => None,
            };
            Ok(RowCall { role, access, rows })
        })();
        let found = found.unwrap_or_else(|e| error!("{e}"));
        // SAFETY: fn_mcxt outlives every call through flinfo; the callback
        // drops the RowCall when it is reset or deleted.
        unsafe {
            if !call.is_null() {
                *call = found;
            } else {
                call = PgMemoryContexts::For((*flinfo).fn_mcxt).leak_and_drop_on_delete(found);
                (*flinfo).fn_extra = call.cast();
            }
        }
    }
    // SAFETY: set above, live for the query.
    let call = unsafe { &*call };
    let shown = match &call.access {
        Access::Owner | Access::NotOwner => true,
        Access::Closed(_) => false,
        Access::Policy(r) => match &call.rows {
            // SAFETY: arguments 2.. are non-NULL text (checked above).
            Some(rows) => {
                rows.visible(&r.session, nargs - 2, |k| unsafe { text_bytes(arg(k + 2)) })
            }
            None => false,
        },
    };
    pg_sys::Datum::from(shown)
}

/// A `text` datum's bytes, detoasted only when it is compressed or stored
/// out of line (a detoasted copy lives in the per-row memory context).
///
/// # Safety
/// `d` is a non-NULL `text` datum.
unsafe fn text_bytes<'a>(d: pg_sys::Datum) -> &'a [u8] {
    use pgrx::varlena as v;
    let p = d.cast_mut_ptr::<pg_sys::varlena>();
    // SAFETY: a valid varlena (the caller's promise).
    unsafe {
        let p = match v::varatt_is_1b_e(p) || v::varatt_is_4b_c(p) {
            true => pg_sys::pg_detoast_datum_packed(p),
            false => p,
        };
        v::varlena_to_byte_slice(p)
    }
}

/// Required by Postgres to call [`row_visible_wrapper`] (`PG_FUNCTION_INFO_V1`).
#[unsafe(no_mangle)]
#[doc(hidden)]
pub extern "C" fn pg_finfo_row_visible_wrapper() -> &'static pg_sys::Pg_finfo_record {
    const V1: pg_sys::Pg_finfo_record = pg_sys::Pg_finfo_record { api_version: 1 };
    &V1
}

extension_sql!(
    r#"
-- The per-row half of the result tables' qual (src/policy.rs, row_visible).
CREATE FUNCTION improv.row_visible(model bigint, measure bigint, VARIADIC dims "any")
  RETURNS bool STRICT STABLE SECURITY INVOKER LANGUAGE c
  AS 'MODULE_PATHNAME', 'row_visible_wrapper';
"#,
    name = "row_visible",
    requires = [cell_rule]
);

/// Row-level security on the catalog: whether the current role may read
/// model `model`'s catalog rows (measures, bindings, views, lambdas, sync
/// state). True when the model has no policies (0.12's rule) or the role
/// may act as its owner.
#[pg_extern(stable, security_invoker)]
fn unrestricted(model: i64) -> Res<bool> {
    Ok(matches!(
        access(model, me())?,
        Access::Owner | Access::NotOwner
    ))
}

/// The `improv_access` qual of measure `measure`'s result table (dimension
/// columns `dims`): [`cell_rule`] decides once per statement (InitPlans,
/// so a cursor keeps its first execution's answer); rule 2 compares each
/// row's names with at most [`SLOTS`] hidden names a dimension (more
/// InitPlans, [`cell_name`]); only rule 3 calls a function a row
/// ([`row_visible`]; [`cell_visible`] for a table without dimensions,
/// which has at most one row).
fn qual(mid: i64, measure: MeasureId, dims: &[String]) -> String {
    let m = measure.0;
    let rule = |n: u8| {
        format!("(SELECT improv.cell_rule({mid}::int8, {m}::int8) OPERATOR(pg_catalog.=) {n})")
    };
    let row = match dims.is_empty() {
        true => format!("improv.cell_visible({mid}::int8, {m}::int8, '{{}}'::text[])"),
        false => format!(
            "improv.row_visible({mid}::int8, {m}::int8, {})",
            dims.iter()
                .map(|d| pm::quote_ident(d))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    let mut slots = Vec::new();
    for (k, d) in dims.iter().enumerate() {
        for j in 0..SLOTS {
            // The columns are NOT NULL; an empty slot is NULL, hiding nothing.
            slots.push(format!(
                "{} IS DISTINCT FROM (SELECT improv.cell_name({mid}::int8, {m}::int8, {k}, {j}))",
                pm::quote_ident(d)
            ));
        }
    }
    match slots.is_empty() {
        true => format!("{} OR ({} AND {row})", rule(0), rule(3)),
        false => format!(
            "{} OR ({} AND {}) OR ({} AND {row})",
            rule(0),
            rule(2),
            slots.join(" AND "),
            rule(3)
        ),
    }
}

/// The row-level security policy that filters result table `table` of
/// measure `measure` (dimension columns `dims`), [`qual`]. Run as the
/// table's owner.
fn secure(mid: i64, measure: MeasureId, table: &str, dims: &[String]) -> Res<()> {
    let have = one::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_policy \
         WHERE polrelid = pg_catalog.to_regclass($1) AND polname = 'improv_access')",
        &[table.into()],
    )?;
    if have != Some(true) {
        Spi::run(&format!(
            "CREATE POLICY improv_access ON {table} FOR SELECT USING ({})",
            qual(mid, measure, dims)
        ))?;
    }
    // Kept when the policies are cleared, so writes stay tracked.
    let tracked = one::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_trigger \
         WHERE tgrelid = pg_catalog.to_regclass($1) AND tgname = 'improv_stale')",
        &[table.into()],
    )?;
    if tracked != Some(true) {
        Spi::run(&format!(
            "CREATE TRIGGER improv_stale AFTER INSERT OR UPDATE ON {table} \
             FOR EACH STATEMENT EXECUTE FUNCTION improv.result_written('{mid}')"
        ))?;
        // Under session_replication_role = replica too.
        Spi::run(&format!(
            "ALTER TABLE {table} ENABLE ALWAYS TRIGGER improv_stale"
        ))?;
        // Its rows were written untracked until now.
        bump_stale(mid)?;
    }
    // Skipped when on: the ALTER takes an ACCESS EXCLUSIVE lock.
    let on = one::<bool>(
        "SELECT relrowsecurity FROM pg_catalog.pg_class WHERE oid = pg_catalog.to_regclass($1)",
        &[table.into()],
    )?;
    if on != Some(true) {
        Spi::run(&format!("ALTER TABLE {table} ENABLE ROW LEVEL SECURITY"))?;
    }
    Ok(())
}

/// Undo [`secure`]: drop the policy, and turn row-level security off when
/// no other policy is left on the table.
fn unsecure(table: &str) -> Res<()> {
    let ours = one::<bool>(
        "SELECT bool_or(polname = 'improv_access') FROM pg_catalog.pg_policy \
         WHERE polrelid = pg_catalog.to_regclass($1)",
        &[table.into()],
    )?;
    let others = one::<bool>(
        "SELECT bool_or(polname <> 'improv_access') FROM pg_catalog.pg_policy \
         WHERE polrelid = pg_catalog.to_regclass($1)",
        &[table.into()],
    )?;
    if ours == Some(true) {
        Spi::run(&format!("DROP POLICY improv_access ON {table}"))?;
    }
    if others != Some(true) {
        Spi::run(&format!("ALTER TABLE {table} DISABLE ROW LEVEL SECURITY"))?;
    }
    Ok(())
}

/// For a result table just created (by its owner): filter it when the
/// model has policies.
pub(crate) fn secure_new(mid: i64, measure: MeasureId, table: &str, dims: &[String]) -> Res<()> {
    if stored(mid)?.1.is_some() {
        secure(mid, measure, table, dims)?;
    }
    Ok(())
}

/// Turn the result tables' row-level security on (`on`) or off for every
/// derived measure of model `mid`, as their owner.
fn secure_all(mid: i64, name: &str, on: bool) -> Res<()> {
    let model = structure(mid)?;
    // The owner owns them; a superuser runs as itself, so a model whose
    // owner is gone still has its tables secured.
    // SAFETY: a plain read of the backend's current user's attributes.
    let role = match unsafe { pg_sys::superuser() } {
        true => None,
        false => owner_of(mid)?,
    };
    owner::as_role(role, || {
        for m in model.measures.values().filter(|m| m.is_derived()) {
            pm::validate_name("measure", &m.name.0)?;
            let table = pm::result_table(name, &m.name.0);
            if !super::trigger::exists(&table)? {
                continue;
            }
            match on {
                true => secure(mid, m.id, &table, &pm::dim_columns(&model, m.id)?)?,
                false => unsecure(&table)?,
            }
        }
        Ok(())
    })
}

/// One row of `improv.policy_audit`, in the change's transaction.
fn audit(
    mid: i64,
    model: &str,
    action: &str,
    resource: &str,
    detail: serde_json::Value,
) -> Res<()> {
    let who = role_name(me());
    owner::as_catalog(|| {
        Spi::run_with_args(
            "INSERT INTO improv.policy_audit (model_id, model, at, who, action, resource, detail) \
             VALUES ($1, $2, pg_catalog.clock_timestamp(), $3, $4, $5, $6)",
            &[
                mid.into(),
                model.into(),
                who.as_str().into(),
                action.into(),
                resource.into(),
                JsonB(detail).into(),
            ],
        )?;
        Ok(())
    })
}

/// A refusal: written to the server log as one JSON object (its
/// transaction rolls back, so no audit row survives it), and returned as
/// the error.
fn deny(model: &str, action: &str, what: &str, msg: String) -> Box<dyn std::error::Error> {
    // Built by hand: one key order whatever serde_json's features.
    let j = |s: &str| serde_json::Value::from(s).to_string();
    let line = format!(
        "pg_improv audit: {{\"audit\":\"deny\",\"who\":{},\"model\":{},\"action\":{},\"resource\":{}}}",
        j(&role_name(me())),
        j(model),
        j(action),
        j(what)
    );
    pgrx::ereport!(
        PgLogLevel::LOG_SERVER_ONLY,
        PgSqlErrorCode::ERRCODE_SUCCESSFUL_COMPLETION,
        line
    );
    msg.into()
}

/// Store model `model`'s access policies: Cedar text and groups JSON,
/// validated as `improv policy set` validates them; `policies` NULL clears
/// them. Returns the new version. Only a role that may act as the owner, or
/// that the stored policies allow `Admin`, may; the owner is never
/// restricted, whatever the policies say.
#[pg_extern(security_invoker)]
fn set_policies(
    model: &str,
    policies: Option<&str>,
    groups: default!(Option<&str>, "NULL"),
) -> Res<i64> {
    let Found {
        mid,
        name,
        owner,
        access: a,
    } = lookup(model)?.ok_or_else(|| missing(model))?;
    let admin = match &a {
        Access::Owner => true,
        Access::Policy(r) => r.session.allowed(Action::Admin, &Resource::Model),
        Access::NotOwner => false,
        // Unusable stored policies: the owner (Access::Owner) replaces them.
        Access::Closed(why) => return Err(closed(&name, why)),
    };
    if !admin {
        if let Access::Policy(r) = &a {
            if !r.session.allowed(Action::ViewModel, &Resource::Model) {
                return Err(deny(&name, "Admin", "the model", unseen(&name)));
            }
        }
        let owner = owner.map_or_else(|| "?".into(), role_name);
        return Err(deny(
            &name,
            "Admin",
            "the model",
            format!(
                "permission denied for improv model {name:?}: only its owner, {owner}, a role \
                 that may act as it, or a role its policies allow Admin may set its policies"
            ),
        ));
    }
    let groups = groups.unwrap_or("");
    if let Some(text) = policies {
        let p = Policies::parse(text, groups)
            .map_err(|e| format!("the policies for improv model {name:?} are not valid:\n{e}"))?;
        for w in p.warnings() {
            pgrx::notice!("{w}");
        }
        if let Ok(m) = structure(mid) {
            for n in p.unknown_names(&m) {
                pgrx::notice!(
                    "the policies name {n}, which model {name} does not have (policies name \
                     measures, items, categories and views by id: see improv.measures and \
                     improv.items)"
                );
            }
        }
    }
    let who = role_name(me());
    // As the extension: an Admin the policies name may not write the
    // catalog itself.
    let version = owner::as_catalog(|| {
        // Serializes with structural changes and other sets.
        Spi::run_with_args(
            "SELECT 1 FROM improv.model WHERE id = $1 FOR UPDATE",
            &[mid.into()],
        )?;
        Ok(Spi::get_one_with_args::<i64>(
            "INSERT INTO improv.policy AS p (model_id, version, text, groups, set_by, set_at) \
             VALUES ($1, 1, $2, $3, $4, pg_catalog.now()) \
             ON CONFLICT (model_id) DO UPDATE SET version = p.version + 1, text = EXCLUDED.text, \
               groups = EXCLUDED.groups, set_by = EXCLUDED.set_by, set_at = EXCLUDED.set_at \
             RETURNING version",
            &[
                mid.into(),
                policies.into(),
                policies.map(|_| groups).into(),
                who.as_str().into(),
            ],
        )?
        .unwrap_or(0))
    })?;
    GENERATION.set(GENERATION.get() + 1);
    secure_all(mid, &name, policies.is_some())?;
    let (action, detail) = match policies {
        Some(t) => (
            "set_policies",
            serde_json::json!({"version": version, "bytes": t.len()}),
        ),
        None => ("clear_policies", serde_json::json!({"version": version})),
    };
    audit(mid, &name, action, "the model", detail)?;
    Ok(version)
}

/// Model `model`'s stored policies: no row when none were ever set; a NULL
/// `policies` after a clear. For a role that may act as the owner, or that
/// the policies allow `Admin`.
// pgrx derives SQL from the inline `name!` tuple, not a type alias.
#[allow(clippy::type_complexity)]
#[pg_extern(security_invoker)]
fn policies(
    model: &str,
) -> TableIterator<
    'static,
    (
        name!(version, i64),
        name!(policies, Option<String>),
        name!(groups, Option<String>),
        name!(set_by, String),
        name!(set_at, pgrx::datum::TimestampWithTimeZone),
    ),
> {
    match policies_rows(model) {
        Ok(rows) => TableIterator::new(rows),
        Err(e) => error!("{e}"),
    }
}

type PolicyRow = (
    i64,
    Option<String>,
    Option<String>,
    String,
    pgrx::datum::TimestampWithTimeZone,
);

fn policies_rows(model: &str) -> Res<Vec<PolicyRow>> {
    let Found {
        mid,
        name,
        owner,
        access: a,
    } = lookup(model)?.ok_or_else(|| missing(model))?;
    let admin = match &a {
        Access::Owner => true,
        Access::Policy(r) => r.session.allowed(Action::Admin, &Resource::Model),
        _ => false,
    };
    if !admin {
        if matches!(&a, Access::Policy(r) if !r.session.allowed(Action::ViewModel, &Resource::Model))
        {
            return Err(unseen(&name).into());
        }
        let owner = owner.map_or_else(|| "?".into(), role_name);
        return Err(format!(
            "permission denied for improv model {name:?}: only its owner, {owner}, a role that \
             may act as it, or a role its policies allow Admin may read its policies"
        )
        .into());
    }
    owner::as_catalog(|| {
        if !installed()? {
            return Ok(Vec::new());
        }
        Spi::connect(|c| {
            c.select(
                "SELECT version, text, groups, set_by, set_at FROM improv.policy \
                 WHERE model_id = $1",
                None,
                &[mid.into()],
            )?
            .map(|r| Ok((req(&r, 1)?, r.get(2)?, r.get(3)?, req(&r, 4)?, req(&r, 5)?)))
            .collect()
        })
    })
}

/// A resource named in JSON: `NULL` the model, `{"measure": M}`,
/// `{"measure": M, "at": {"Category": "Item", …}}` a cell,
/// `{"category": C}`, `{"category": C, "item": I}`, `{"view": V}`.
fn resource(m: &Model, spec: Option<&serde_json::Value>) -> Result<Resource, String> {
    use serde_json::Value as J;
    let Some(spec) = spec.filter(|s| !s.is_null()) else {
        return Ok(Resource::Model);
    };
    let o = spec
        .as_object()
        .ok_or("the resource is a JSON object, or NULL for the model")?;
    let s = |k: &str| o.get(k).and_then(J::as_str);
    let cat = |n: &str| {
        m.categories
            .values()
            .find(|c| c.name.0.eq_ignore_ascii_case(n))
            .map(|c| c.id)
            .ok_or_else(|| format!("no category {n:?}"))
    };
    let item = |c: CategoryId, n: &str| {
        m.items
            .values()
            .find(|i| i.category == c && i.name.0 == n)
            .map(|i| i.id)
            .ok_or_else(|| format!("no item {n:?}"))
    };
    if let Some(v) = s("view") {
        return m
            .views
            .values()
            .find(|x| x.name.0.eq_ignore_ascii_case(v))
            .map(|x| Resource::View(x.id))
            .ok_or_else(|| format!("no view {v:?}"));
    }
    if let Some(name) = s("measure") {
        let id = m
            .measures
            .values()
            .find(|x| x.name.0.eq_ignore_ascii_case(name))
            .map(|x| x.id)
            .ok_or_else(|| format!("no measure {name:?}"))?;
        let Some(at) = o.get("at") else {
            return Ok(Resource::Measure(id));
        };
        let at = at
            .as_object()
            .ok_or("\"at\" is an object of category: item")?;
        let mut dims = BTreeMap::new();
        for (c, i) in at {
            let c = cat(c)?;
            let i = i.as_str().ok_or("\"at\" names items by name")?;
            dims.insert(c, item(c, i)?);
        }
        return Ok(Resource::Cell(id, Coordinate { dims }));
    }
    if let Some(c) = s("category") {
        let c = cat(c)?;
        return match s("item") {
            Some(i) => Ok(Resource::Item(item(c, i)?)),
            None => Ok(Resource::Category(c)),
        };
    }
    Err("name a measure, a category (and item) or a view".into())
}

/// A resource as people read it: `measure Revenue (3)`.
fn describe(m: &Model, r: &Resource) -> String {
    let item = |i: &ItemId| {
        m.items.get(i).map_or_else(
            || format!("item {}", i.0),
            |x| {
                let c = m
                    .categories
                    .get(&x.category)
                    .map_or("?", |c| c.name.0.as_str());
                format!("{c}={}", x.name.0)
            },
        )
    };
    let measure = |id: &MeasureId| {
        m.measures.get(id).map_or_else(
            || format!("{}", id.0),
            |x| format!("{} ({})", x.name.0, id.0),
        )
    };
    match r {
        Resource::Model => "the model".into(),
        Resource::Category(c) => format!(
            "category {} ({})",
            m.categories.get(c).map_or("?", |c| c.name.0.as_str()),
            c.0
        ),
        Resource::Item(i) => format!("item {} ({})", item(i), i.0),
        Resource::Measure(id) => format!("measure {}", measure(id)),
        Resource::View(v) => format!(
            "view {} ({})",
            m.views.get(v).map_or("?", |v| v.name.0.as_str()),
            v.0
        ),
        Resource::Cell(id, at) => format!(
            "cell {}[{}]",
            measure(id),
            at.dims.values().map(item).collect::<Vec<_>>().join(", ")
        ),
    }
}

type ExplainRow = (i32, String, String, bool, Vec<String>, Vec<String>);

/// Why role `role` (the caller when NULL) may or may not do `action` on
/// `resource` of `model`: each question the composite rules ask, with the
/// policies that decided it, stopping at the first denial. Only a role
/// that may act as the owner, or that the policies allow `Admin`, may ask
/// about another role or see the steps of a denial; anyone else gets one
/// row, and a denial and a resource it cannot see answer alike.
// pgrx derives SQL from the inline `name!` tuple, not a type alias.
#[allow(clippy::type_complexity)]
#[pg_extern(security_invoker)]
fn explain_access(
    model: &str,
    action: &str,
    resource: default!(Option<JsonB>, "NULL"),
    role: default!(Option<&str>, "NULL"),
) -> TableIterator<
    'static,
    (
        name!(step, i32),
        name!(action, String),
        name!(resource, String),
        name!(allowed, bool),
        name!(policies, Vec<String>),
        name!(errors, Vec<String>),
    ),
> {
    match explain_rows(model, action, resource.as_ref().map(|j| &j.0), role) {
        Ok(rows) => TableIterator::new(rows),
        Err(e) => error!("{e}"),
    }
}

fn explain_rows(
    model: &str,
    action: &str,
    spec: Option<&serde_json::Value>,
    role: Option<&str>,
) -> Res<Vec<ExplainRow>> {
    let action = Action::from_str(action)?;
    let Found {
        mid,
        name,
        access: mine,
        ..
    } = lookup(model)?.ok_or_else(|| missing(model))?;
    let admin = match &mine {
        Access::Owner => true,
        Access::Policy(r) => r.session.allowed(Action::Admin, &Resource::Model),
        _ => false,
    };
    let me = me();
    let target = match role {
        None => me,
        Some(r) => {
            let oid = one::<pg_sys::Oid>(
                "SELECT pg_catalog.to_regrole(pg_catalog.quote_ident($1))::pg_catalog.oid",
                &[r.into()],
            )?
            .ok_or_else(|| format!("role {r:?} does not exist"))?;
            if oid != me && !admin {
                return Err(format!(
                    "permission denied for improv model {name:?}: only its owner, or a role its \
                     policies allow Admin, may explain another role's access"
                )
                .into());
            }
            oid
        }
    };
    let asked = spec.map_or_else(|| "the model".to_string(), |s| s.to_string());
    let one = |allowed: bool, errors: Vec<String>, what: String| {
        vec![(
            1,
            action.name().to_string(),
            what,
            allowed,
            Vec::new(),
            errors,
        )]
    };
    let a = access(mid, target)?;
    let r = match a {
        Access::Owner => {
            return Ok(one(
                true,
                vec!["may act as the model's owner: not restricted".into()],
                asked,
            ))
        }
        Access::NotOwner => {
            return Ok(one(
                false,
                vec!["the model has no access policies: only its owner acts on it".into()],
                asked,
            ))
        }
        Access::Closed(why) => return Ok(one(false, vec![why], asked)),
        Access::Policy(r) => r,
    };
    let full = || {
        let res = resource(&r.model, spec)?;
        let steps = r.session.explain(action, &res)?;
        Ok::<_, String>(
            steps
                .into_iter()
                .enumerate()
                .map(|(i, c)| {
                    (
                        i as i32 + 1,
                        c.action.name().to_string(),
                        describe(&r.model, &c.resource),
                        c.decision.allow,
                        c.decision.policies,
                        c.decision.errors,
                    )
                })
                .collect::<Vec<_>>(),
        )
    };
    if admin {
        return full().map_err(Into::into);
    }
    // Not an admin: its own answer, without the policies; a denial and a
    // resource it cannot see answer alike.
    let steps = full().unwrap_or_default();
    let allowed = !steps.is_empty() && steps.iter().all(|s| s.3);
    Ok(one(allowed, Vec::new(), asked))
}

/// Model `model`'s categories, for a role its policies let open it (and
/// for the owner): id and name.
#[pg_extern(security_invoker)]
fn categories(model: &str) -> TableIterator<'static, (name!(id, i64), name!(name, String))> {
    let rows = (|| -> Res<Vec<(i64, String)>> {
        let mid = reader(model)?.mid;
        let m = structure(mid)?;
        let mut cats: Vec<_> = m.categories.values().collect();
        cats.sort_by_key(|c| c.id);
        Ok(cats
            .into_iter()
            .map(|c| (i64::from(c.id.0), c.name.0.clone()))
            .collect())
    })();
    match rows {
        Ok(rows) => TableIterator::new(rows),
        Err(e) => error!("{e}"),
    }
}

/// Model `model`'s items that the caller may read (`ReadItem`), by
/// category and id.
#[pg_extern(security_invoker)]
fn items(
    model: &str,
) -> TableIterator<'static, (name!(category, String), name!(id, i64), name!(name, String))> {
    let rows = (|| -> Res<Vec<(String, i64, String)>> {
        let Reader { mid, r, .. } = reader(model)?;
        let m = match &r {
            Some(r) => r.model.clone(),
            None => structure(mid)?,
        };
        let mut items: Vec<_> = m
            .items
            .values()
            .filter(|i| r.as_ref().is_none_or(|r| r.session.item_visible(i.id)))
            .collect();
        items.sort_by_key(|i| (i.category, i.id));
        Ok(items
            .into_iter()
            .map(|i| {
                let c = m
                    .categories
                    .get(&i.category)
                    .map_or("", |c| c.name.0.as_str());
                (c.to_string(), i64::from(i.id.0), i.name.0.clone())
            })
            .collect())
    })();
    match rows {
        Ok(rows) => TableIterator::new(rows),
        Err(e) => error!("{e}"),
    }
}

type MeasureOut = (i64, String, String, String, Vec<String>, Option<String>);

/// Model `model`'s measures that the caller may read (`ReadMeasure`): kind
/// (`input` or `derived`), type, categories, and the formula when every
/// measure it reads is readable too (else NULL).
// pgrx derives SQL from the inline `name!` tuple, not a type alias.
#[allow(clippy::type_complexity)]
#[pg_extern(security_invoker)]
fn measures(
    model: &str,
) -> TableIterator<
    'static,
    (
        name!(id, i64),
        name!(name, String),
        name!(kind, String),
        name!(value_type, String),
        name!(categories, Vec<String>),
        name!(formula, Option<String>),
    ),
> {
    let rows = (|| -> Res<Vec<MeasureOut>> {
        let Reader { mid, r, .. } = reader(model)?;
        let m = structure(mid)?;
        let formulas: HashMap<i64, Option<String>> = owner::as_catalog(|| {
            Spi::connect(|c| {
                c.select(
                    "SELECT id, formula FROM improv.measure WHERE model_id = $1",
                    None,
                    &[mid.into()],
                )?
                .map(|row| Ok((req::<i64>(&row, 1)?, row.get::<String>(2)?)))
                .collect::<Res<_>>()
            })
        })?;
        let mut ms: Vec<_> = m
            .measures
            .values()
            .filter(|x| r.as_ref().is_none_or(|r| r.session.measure_visible(x.id)))
            .collect();
        ms.sort_by_key(|x| x.id);
        Ok(ms
            .into_iter()
            .map(|x| {
                let id = i64::from(x.id.0);
                let shown = r.as_ref().is_none_or(|r| r.session.formula_visible(x.id));
                let formula = formulas.get(&id).cloned().flatten().filter(|_| shown);
                (
                    id,
                    x.name.0.clone(),
                    if x.is_input() { "input" } else { "derived" }.to_string(),
                    pm::value_type_name(x.value_type).to_string(),
                    (x.categories.iter())
                        .map(|c| {
                            m.categories
                                .get(c)
                                .map_or_else(String::new, |c| c.name.0.clone())
                        })
                        .collect(),
                    formula,
                )
            })
            .collect())
    })();
    match rows {
        Ok(rows) => TableIterator::new(rows),
        Err(e) => error!("{e}"),
    }
}

// shortcut: an input measure's cells come from a load of every binding;
// load only that measure's binding once a model's other bindings are large.
/// The cells of measure `measure` of `model` that the caller may read: an
/// input's from its bound table or `improv.cell`, a derived measure's from
/// its result table, each read as the owner and filtered by Cedar. `coord`
/// is `{"Category": "Item", …}`; `value` JSON of the measure's type.
#[pg_extern(security_invoker)]
fn cells(
    model: &str,
    measure: &str,
) -> TableIterator<'static, (name!(coord, JsonB), name!(value, JsonB))> {
    match cell_rows(model, measure) {
        Ok(rows) => TableIterator::new(rows),
        Err(e) => error!("{e}"),
    }
}

fn cell_rows(model: &str, measure: &str) -> Res<Vec<(JsonB, JsonB)>> {
    let Reader {
        mid,
        name,
        owner,
        r,
    } = reader(model)?;
    let m = structure(mid)?;
    let x = m
        .measures
        .values()
        .find(|x| x.name.0.eq_ignore_ascii_case(measure))
        .filter(|x| r.as_ref().is_none_or(|r| r.session.measure_visible(x.id)))
        .ok_or_else(|| format!("improv model {name:?} has no measure {measure:?}"))?;
    let id = x.id;
    let visible = |at: &Coordinate| r.as_ref().is_none_or(|r| r.session.cell_visible(id, at));
    let mut out = Vec::new();
    if x.is_derived() {
        pm::validate_name("measure", &x.name.0)?;
        let table = pm::result_table(&name, &x.name.0);
        let dims = pm::dim_columns(&m, id)?;
        let vt = x.value_type;
        let rows = owner::as_role(owner, || {
            Ok(match super::trigger::exists(&table)? {
                true => super::read_results(&table, &dims, vt)?,
                false => Default::default(),
            })
        })?;
        let names: HashMap<CategoryId, HashMap<&str, ItemId>> = {
            let mut n: HashMap<CategoryId, HashMap<&str, ItemId>> = HashMap::new();
            for i in m.items.values() {
                n.entry(i.category)
                    .or_default()
                    .insert(i.name.0.as_str(), i.id);
            }
            n
        };
        for (key, v) in &rows {
            let at = (x.categories.iter().zip(key))
                .map(|(c, k)| Some((*c, *names.get(c)?.get(k.as_str())?)))
                .collect::<Option<BTreeMap<_, _>>>();
            if at.is_some_and(|dims| visible(&Coordinate { dims })) {
                out.push((
                    JsonB(pm::coord_json(&m, id, key)?),
                    JsonB(pm::value_json(v)),
                ));
            }
        }
        return Ok(out);
    }
    let loaded = owner::as_catalog(|| super::trigger::load(mid, name.clone(), false))?;
    let mut cells: Vec<_> = (loaded.model.inputs.iter())
        .filter(|((mm, at), _)| *mm == id && visible(at))
        .collect();
    cells.sort_by(|a, b| a.0 .1.dims.cmp(&b.0 .1.dims));
    for ((_, at), v) in cells {
        let mut coord = serde_json::Map::new();
        for (c, i) in &at.dims {
            let cat = loaded
                .model
                .categories
                .get(c)
                .map_or("", |c| c.name.0.as_str());
            let item = loaded.model.items.get(i).map_or("", |i| i.name.0.as_str());
            coord.insert(cat.to_string(), item.into());
        }
        let v =
            CellValue::from_model_value(v).map_or(serde_json::Value::Null, |v| pm::value_json(&v));
        out.push((JsonB(coord.into()), JsonB(v)));
    }
    Ok(out)
}

/// Set (or, with a `null` value, clear) input cells of `model`, as
/// `what_if`'s edits name them: `[{"measure": M, "coord": {"Category":
/// "Item", …}, "value": v | null}]`. Bound cells are written to their
/// table, unbound ones to `improv.cell`, as the owner; the edit is then
/// computed as the model's sync mode computes a write to its tables
/// (unbound cells' dependents are recomputed at once). Each cell must be
/// editable (`EditCell`, and readable); one that is not refuses the whole
/// call. Returns the cells written.
#[pg_extern(security_invoker)]
fn set_cells(model: &str, edits: JsonB) -> Res<i64> {
    let Found {
        mid,
        name,
        owner,
        access: a,
    } = lookup(model)?.ok_or_else(|| missing(model))?;
    let text = edits.0.to_string();
    let r = match a {
        Access::Owner => None,
        Access::NotOwner => return Err(not_owner(&name, owner)),
        Access::Closed(why) => return Err(closed(&name, &why)),
        Access::Policy(r) if r.session.allowed(Action::ViewModel, &Resource::Model) => Some(r),
        Access::Policy(_) => return Err(deny(&name, "EditCell", "a cell", unseen(&name))),
    };
    let full = structure(mid)?;
    // A restricted role names only what it may read.
    let named = r.as_ref().map_or_else(|| full.clone(), |r| r.visible());
    let parsed = pm::parse_edits(&named, &text)
        .map_err(|e| e.to_string().replacen("what_if edit", "edit", 1))?;
    if let Some(r) = &r {
        for (m, at, _) in &parsed {
            if !r.session.cell_editable(*m, at) {
                let what = describe(&full, &Resource::Cell(*m, at.clone()));
                return Err(deny(
                    &name,
                    "EditCell",
                    &what,
                    format!("permission denied for improv model {name:?}: may not edit {what}"),
                ));
            }
        }
    }
    let list = edits.0.as_array().cloned().unwrap_or_default();
    let n = parsed.len() as i64;
    // As the owner, found as the extension: a restricted role may not read
    // the owner's catalog row itself.
    let gate = Gate {
        mid,
        owner,
        hidden: r.as_ref().map(|r| r.hidden()).unwrap_or_default(),
    };
    let changed = owner::as_role(owner, || write_cells(mid, &name, &full, &parsed, &list))
        .map_err(|e| gate.scrub(e))?;
    if !changed.is_empty() {
        owner::as_role(owner, || {
            let mut l = super::trigger::load(mid, name.clone(), true)?;
            let targets = pm::dependents(&l.model, &changed);
            super::trigger::recompute(
                &mut l,
                &pm::Plan {
                    targets,
                    scope: None,
                },
            )?;
            super::bump_version(mid)
        })
        .map_err(|e| gate.scrub(e))?;
    }
    if n > 0 {
        let cells: Vec<String> = (parsed.iter())
            .map(|(m, at, _)| describe(&full, &Resource::Cell(*m, at.clone())))
            .collect();
        audit(
            mid,
            &name,
            "EditCell",
            &format!("{n} cells"),
            serde_json::json!({"cells": cells}),
        )?;
    }
    Ok(n)
}

/// Write `edits` (parsed from `raw`, in order): bound cells to their
/// tables, unbound ones to `improv.cell`. Returns the unbound measures
/// changed. Run as the owner.
fn write_cells(
    mid: i64,
    name: &str,
    m: &Model,
    edits: &[pm::Edit],
    raw: &[serde_json::Value],
) -> Res<Vec<MeasureId>> {
    let mut unbound = std::collections::BTreeSet::new();
    for ((id, at, _), e) in edits.iter().zip(raw) {
        let value = e.get("value").cloned().unwrap_or(serde_json::Value::Null);
        let binding = owner::as_catalog(|| {
            Spi::connect(|c| {
                let t = c.select(
                    "SELECT (SELECT format('%I.%I', n.nspname, c.relname) FROM pg_class c \
                       JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.oid = b.rel), \
                     b.value_col, b.agg, b.dim_cols::text \
                     FROM improv.binding b WHERE b.model_id = $1 AND b.measure_id = $2",
                    None,
                    &[mid.into(), i64::from(id.0).into()],
                )?;
                t.into_iter()
                    .next()
                    .map(|r| {
                        Ok((
                            req::<String>(&r, 1)?,
                            req::<String>(&r, 2)?,
                            r.get::<String>(3)?,
                            req::<String>(&r, 4)?,
                        ))
                    })
                    .transpose()
            })
        })?;
        let Some((rel, col, agg, dims)) = binding else {
            let coord = serde_json::to_string(&at.dims)?;
            owner::as_catalog(|| {
                if value.is_null() {
                    Spi::run_with_args(
                        "DELETE FROM improv.cell WHERE model_id = $1 AND measure_id = $2 \
                         AND coord = $3::text::jsonb",
                        &[mid.into(), i64::from(id.0).into(), coord.as_str().into()],
                    )?;
                } else {
                    Spi::run_with_args(
                        "INSERT INTO improv.cell (model_id, measure_id, coord, value) \
                         VALUES ($1, $2, $3::text::jsonb, $4::text::jsonb) \
                         ON CONFLICT (model_id, measure_id, coord) DO UPDATE SET value = EXCLUDED.value",
                        &[
                            mid.into(),
                            i64::from(id.0).into(),
                            coord.as_str().into(),
                            value.to_string().into(),
                        ],
                    )?;
                }
                Ok(())
            })?;
            unbound.insert(*id);
            continue;
        };
        if agg.is_some() {
            return Err(format!(
                "measure {} of improv model {name:?} is aggregated from its table: it is read-only",
                m.measures[id].name.0
            )
            .into());
        }
        let dims: BTreeMap<String, String> = serde_json::from_str(&dims)?;
        let mut row = serde_json::Map::new();
        let mut on = Vec::new();
        for (c, i) in &at.dims {
            let cat = &m.categories[c].name.0;
            let dc = dims.get(cat).ok_or_else(|| {
                format!("the binding of measure {} has no column for {cat}", id.0)
            })?;
            row.insert(dc.clone(), m.items[i].name.0.clone().into());
            on.push(format!("t.{0} = n.{0}", pm::quote_ident(dc)));
        }
        row.insert(col.clone(), value.clone());
        let v = pm::quote_ident(&col);
        let src = format!("jsonb_populate_record(NULL::{rel}, $1::text::jsonb) n");
        on.push("true".into());
        let hit = Spi::get_one_with_args::<i64>(
            &format!(
                "WITH u AS (UPDATE {rel} t SET {v} = n.{v} FROM {src} WHERE {} RETURNING 1) \
                 SELECT count(*) FROM u",
                on.join(" AND ")
            ),
            &[serde_json::Value::Object(row.clone()).to_string().into()],
        )?;
        if hit == Some(0) && !value.is_null() {
            let cols: Vec<String> = row.keys().map(|k| pm::quote_ident(k)).collect();
            let ncols: Vec<String> = cols.iter().map(|c| format!("n.{c}")).collect();
            Spi::run_with_args(
                &format!(
                    "INSERT INTO {rel} ({}) SELECT {} FROM {src}",
                    cols.join(", "),
                    ncols.join(", ")
                ),
                &[serde_json::Value::Object(row).to_string().into()],
            )?;
        }
    }
    Ok(unbound.into_iter().collect())
}

/// A change a restricted role may make, run as the owner: the model, its
/// owner, and the names it may not read.
pub(crate) struct Gate {
    pub(crate) mid: i64,
    pub(crate) owner: Option<pg_sys::Oid>,
    hidden: Vec<String>,
}

impl Gate {
    /// `e`, an error of the change run as the owner, unless it names a
    /// measure or item the role may not read (a dependent a drop or a
    /// redefinition would break): then a message that names none.
    pub(crate) fn scrub(&self, e: Box<dyn std::error::Error>) -> Box<dyn std::error::Error> {
        let msg = e.to_string();
        let words: Vec<&str> = msg
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|w| !w.is_empty())
            .collect();
        let named = |h: &String| words.iter().any(|w| w.eq_ignore_ascii_case(h));
        match self.hidden.iter().any(named) {
            true => "the change would break or reveal a part of the model you may not read".into(),
            false => e,
        }
    }
}

/// For `define` by a role that may not act as model `model`'s owner: when
/// the model has policies and they allow `DefineMeasure` on the model (a
/// new measure or LAMBDA) or on the measure (a redefinition), and the
/// definition reads only measures the role may read, the [`Gate`] (the
/// definition then runs as the owner). `None` for everyone `define`
/// already serves (the owner; anyone, for a model without policies, whom
/// its own check then refuses). An error, logged, for a refusal; a hidden
/// measure answers as one that does not exist.
pub(crate) fn define_gate(model: &str, definition: &str, replace: bool) -> Res<Option<Gate>> {
    let Some(Found {
        mid,
        name,
        owner,
        access: a,
    }) = lookup(model)?
    else {
        return Ok(None);
    };
    let r = match a {
        Access::Owner | Access::NotOwner => return Ok(None),
        Access::Closed(why) => return Err(closed(&name, &why)),
        Access::Policy(r) => r,
    };
    let (target, rhs) = definition.split_once('=').unwrap_or((definition, ""));
    let target = target.trim();
    if !r.session.allowed(Action::ViewModel, &Resource::Model) {
        return Err(deny(&name, "DefineMeasure", target, unseen(&name)));
    }
    let refused = |what: &str| {
        deny(
            &name,
            "DefineMeasure",
            what,
            format!("permission denied for improv model {name:?}: may not define {what}"),
        )
    };
    let named = (r.model.measures.values()).find(|x| x.name.0.eq_ignore_ascii_case(target));
    // A hidden measure's name is taken: refused, as the server refuses it.
    if named.is_some_and(|x| !r.session.measure_visible(x.id)) {
        return Err(refused(target));
    }
    let resource = match named {
        Some(x) if replace => Resource::Measure(x.id),
        _ => Resource::Model,
    };
    if !r.session.allowed(Action::DefineMeasure, &resource) {
        return Err(refused(target));
    }
    if let Some(x) = named.filter(|x| replace && x.is_derived()) {
        if !r.session.formula_visible(x.id) {
            return Err(refused(target));
        }
    }
    // A formula is parsed as the role sees the model, so it cannot read a
    // hidden measure: one answers as a measure the model lacks. A LAMBDA's
    // body reads only its parameters.
    let rhs = rhs.trim_start();
    let lambda = rhs
        .get(..6)
        .is_some_and(|h| h.eq_ignore_ascii_case("lambda"))
        && rhs[6..].trim_start().starts_with('(');
    if !lambda {
        let visible = r.visible();
        let probe = visible.with_provisional(MeasureId(u32::MAX), target);
        parser::parse_definition(&probe, definition).map_err(|e| format!("definition: {e}"))?;
    }
    // Redefining a LAMBDA changes every measure that calls it: each must
    // be one the role may redefine.
    if let Some((id, _)) = r.model.lambda_by_name(target).filter(|_| replace) {
        let (_, users) = r.model.lambda_users(id);
        if users.iter().any(|m| {
            !r.session.formula_visible(*m)
                || !r
                    .session
                    .allowed(Action::DefineMeasure, &Resource::Measure(*m))
        }) {
            return Err(refused(target));
        }
    }
    Ok(Some(Gate {
        mid,
        owner,
        hidden: r.hidden(),
    }))
}

/// For `drop_measure` by a role that may not act as the owner: when the
/// policies allow `DefineMeasure` on the measure (or, for a LAMBDA, on the
/// model), the [`Gate`]. As [`define_gate`] otherwise.
pub(crate) fn drop_gate(model: &str, measure: &str) -> Res<Option<Gate>> {
    let Some(Found {
        mid,
        name,
        owner,
        access: a,
    }) = lookup(model)?
    else {
        return Ok(None);
    };
    let r = match a {
        Access::Owner | Access::NotOwner => return Ok(None),
        Access::Closed(why) => return Err(closed(&name, &why)),
        Access::Policy(r) => r,
    };
    if !r.session.allowed(Action::ViewModel, &Resource::Model) {
        return Err(deny(&name, "DefineMeasure", measure, unseen(&name)));
    }
    let named = (r.model.measures.values()).find(|x| x.name.0.eq_ignore_ascii_case(measure));
    let resource = match named {
        Some(x) if r.session.measure_visible(x.id) => Resource::Measure(x.id),
        None if r.model.lambda_by_name(measure).is_some() => Resource::Model,
        // Hidden, or missing: the same answer.
        _ => return Err(format!("measure {measure} does not exist").into()),
    };
    if !r.session.allowed(Action::DefineMeasure, &resource) {
        return Err(deny(
            &name,
            "DefineMeasure",
            measure,
            format!("permission denied for improv model {name:?}: may not drop {measure}"),
        ));
    }
    Ok(Some(Gate {
        mid,
        owner,
        hidden: r.hidden(),
    }))
}

/// Audit a `define` that [`define_gate`] let through.
pub(crate) fn audit_define(mid: i64, model: &str, definition: &str, id: i64) -> Res<()> {
    audit(
        mid,
        model,
        "DefineMeasure",
        definition.split('=').next().unwrap_or("").trim(),
        serde_json::json!({"definition": definition, "id": id}),
    )
}

/// Audit a `drop_measure` that [`drop_gate`] let through.
pub(crate) fn audit_drop(mid: i64, model: &str, measure: &str) -> Res<()> {
    audit(
        mid,
        model,
        "DefineMeasure",
        measure,
        serde_json::json!({"dropped": measure}),
    )
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    fn setup() {
        for sql in [
            "CREATE TABLE s (k text PRIMARY KEY, v float8)",
            "INSERT INTO s VALUES ('a', 1), ('b', 2)",
            "SELECT improv.create_model('p')",
            "SELECT improv.bind_category('p', 'K', 's', 'k')",
            "SELECT improv.bind_measure('p', 'V', 's', 'v')",
            "SELECT improv.define('p', 'W = V * 2')",
            "SELECT improv.recalc('p')",
            "CREATE ROLE pol_reader",
            "GRANT improv_user TO pol_reader",
            "GRANT USAGE ON SCHEMA improv, improv_p TO pol_reader",
            "GRANT SELECT ON improv_p.w TO pol_reader",
        ] {
            Spi::run(sql).unwrap();
        }
    }

    fn as_reader(sql: &str) -> Option<i64> {
        Spi::run("SET LOCAL ROLE pol_reader").unwrap();
        let n = Spi::get_one::<i64>(sql).unwrap();
        Spi::run("RESET ROLE").unwrap();
        n
    }

    /// The result table is filtered only while the model has policies, and
    /// a role the policies do not name reads nothing.
    #[pg_test]
    fn result_tables_follow_the_policies() {
        setup();
        assert_eq!(as_reader("SELECT count(*) FROM improv_p.w"), Some(2));
        let item = Spi::get_one::<i64>("SELECT id FROM improv.item WHERE name = 'a'")
            .unwrap()
            .unwrap();
        let text = format!(
            "permit (principal == Improv::User::\"pol_reader\", action in Improv::Action::\"Read\", \
             resource); forbid (principal, action == Improv::Action::\"ReadItem\", \
             resource == Improv::Item::\"{item}\");"
        );
        let v = Spi::get_one_with_args::<i64>(
            "SELECT improv.set_policies('p', $1)",
            &[text.as_str().into()],
        )
        .unwrap();
        assert_eq!(v, Some(1));
        assert_eq!(as_reader("SELECT count(*) FROM improv_p.w"), Some(1));
        assert_eq!(
            as_reader("SELECT count(*) FROM improv.measure"),
            Some(0),
            "the catalog is the owner's"
        );
        Spi::run("SELECT improv.set_policies('p', NULL)").unwrap();
        assert_eq!(as_reader("SELECT count(*) FROM improv_p.w"), Some(2));
    }

    /// Stored policies that no longer validate (written around
    /// set_policies, by a superuser) fail closed for everyone but the owner.
    /// Sessions key on the version, which every write moves on.
    #[pg_test]
    fn unusable_policies_fail_closed() {
        setup();
        Spi::run("SELECT improv.set_policies('p', 'permit (principal, action, resource);')")
            .unwrap();
        assert_eq!(as_reader("SELECT count(*) FROM improv_p.w"), Some(2));
        Spi::run("ALTER TABLE improv.policy DISABLE ROW LEVEL SECURITY").unwrap();
        Spi::run(
            "UPDATE improv.policy SET version = version + 1, text = \
             'permit (principal, action, resource == Improv::Nope::\"x\");'",
        )
        .unwrap();
        assert_eq!(as_reader("SELECT count(*) FROM improv_p.w"), Some(0));
        assert_eq!(
            Spi::get_one::<i64>("SELECT count(*) FROM improv_p.w").unwrap(),
            Some(2),
            "the owner reads on"
        );
    }

    fn run(sql: &str) {
        Spi::run(sql).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    fn id(sql: &str) -> i64 {
        Spi::get_one::<i64>(sql).unwrap().unwrap()
    }

    /// One text answer of `sql`, as `role` (the superuser when `None`).
    fn text_as(role: Option<&str>, sql: &str) -> String {
        if let Some(r) = role {
            run(&format!("SET LOCAL ROLE {r}"));
        }
        let v = Spi::get_one::<String>(sql)
            .unwrap_or_else(|e| panic!("{sql}: {e}"))
            .unwrap_or_default();
        if role.is_some() {
            run("RESET ROLE");
        }
        v
    }

    /// The equivalence fixture: model `q` (manual) over R (r0..r5) x C
    /// (c0..c3), W = V * 2 (two dimensions), S = SUM(W OVER C) (one),
    /// T = SUM(W OVER R, C) (none). eq_a is a user, eq_b a member of
    /// eq_staff, eq_o may act as the owner (the test's superuser).
    struct Eq {
        mid: i64,
        w: i64,
        s: i64,
        t: i64,
        r: i64,
        c: i64,
        item: std::collections::HashMap<String, i64>,
    }

    const ROLES: [&str; 3] = ["eq_a", "eq_b", "eq_o"];

    fn eq_setup() -> Eq {
        for sql in [
            "CREATE TABLE eqf (r text, c text, v float8, PRIMARY KEY (r, c))",
            "INSERT INTO eqf SELECT 'r' || i, 'c' || j, i * 10 + j \
             FROM generate_series(0, 5) i, generate_series(0, 3) j",
            "SELECT improv.create_model('q')",
            "SELECT improv.set_sync_mode('q', 'manual')",
            "SELECT improv.bind_category('q', 'R', 'eqf', 'r')",
            "SELECT improv.bind_category('q', 'C', 'eqf', 'c')",
            "SELECT improv.bind_measure('q', 'V', 'eqf', 'v')",
            "SELECT improv.define('q', 'W = V * 2')",
            "SELECT improv.define('q', 'S = SUM(W OVER C)')",
            "SELECT improv.define('q', 'T = SUM(W OVER R, C)')",
            "SELECT improv.recalc('q')",
            "CREATE ROLE eq_a",
            "CREATE ROLE eq_b",
            "CREATE ROLE eq_o",
            "CREATE ROLE eq_staff",
            "GRANT eq_staff TO eq_b",
            "GRANT improv_user TO eq_a, eq_b, eq_o",
            "DO $$ BEGIN EXECUTE format('GRANT %I TO eq_o', current_user); END $$",
            "GRANT USAGE ON SCHEMA improv_q TO eq_a, eq_b, eq_o",
            "GRANT SELECT ON ALL TABLES IN SCHEMA improv_q TO eq_a, eq_b, eq_o",
        ] {
            run(sql);
        }
        let named = |t: &str, n: &str| id(&format!("SELECT id FROM improv.{t} WHERE name = '{n}'"));
        let item = (0..6)
            .map(|i| format!("r{i}"))
            .chain((0..4).map(|j| format!("c{j}")))
            .map(|n| {
                let i = named("item", &n);
                (n, i)
            })
            .collect();
        Eq {
            mid: id("SELECT id FROM improv.model WHERE name = 'q'"),
            w: named("measure", "W"),
            s: named("measure", "S"),
            t: named("measure", "T"),
            r: named("category", "R"),
            c: named("category", "C"),
            item,
        }
    }

    impl Eq {
        /// The rows each role reads through the qual, against the rows of
        /// a copy (no row-level security) the old per-row check shows.
        fn check(&self, what: &str) {
            for t in ["w", "s", "t"] {
                run(&format!("DROP TABLE IF EXISTS eq_{t}"));
                run(&format!(
                    "CREATE TABLE eq_{t} AS SELECT * FROM improv_q.{t}"
                ));
                run(&format!("GRANT SELECT ON eq_{t} TO PUBLIC"));
            }
            let tables = [
                ("w", self.w, "r || '/' || c", "ARRAY[r, c]"),
                ("s", self.s, "r", "ARRAY[r]"),
                ("t", self.t, "''", "'{}'::text[]"),
            ];
            for role in ROLES {
                for (t, m, key, dims) in tables {
                    let rows = |from: &str, filter: &str| {
                        format!(
                            "SELECT coalesce(string_agg({key} || '=' || value, ',' ORDER BY {key}), '') \
                             FROM {from} {filter}"
                        )
                    };
                    let qual = text_as(Some(role), &rows(&format!("improv_q.{t}"), ""));
                    let oracle = text_as(
                        Some(role),
                        &rows(
                            &format!("eq_{t}"),
                            &format!("WHERE improv.cell_visible({}, {m}, {dims})", self.mid),
                        ),
                    );
                    assert_eq!(qual, oracle, "{what}: {role} reading {t}");
                }
            }
        }

        fn item(&self, n: &str) -> i64 {
            self.item[n]
        }

        fn allow(&self) -> String {
            "permit (principal == Improv::User::\"eq_a\", action in Improv::Action::\"Read\", resource);\n\
             permit (principal in Improv::Role::\"eq_staff\", action in Improv::Action::\"Read\", resource);\n"
                .into()
        }

        /// The fixed matrix: (name, policies, groups).
        fn matrix(&self) -> Vec<(&'static str, String, String)> {
            let a = self.allow();
            let i = |n: &str| self.item(n);
            let none = String::new();
            vec![
                ("uniform allow, by user and by role", a.clone(), none.clone()),
                (
                    "allow by group",
                    "permit (principal in Improv::Group::\"g\", action in Improv::Action::\"Read\", resource);".into(),
                    r#"{"g": {"users": ["eq_a"]}}"#.into(),
                ),
                (
                    "uniform deny of cells",
                    format!("{a}forbid (principal, action == Improv::Action::\"ReadCell\", resource);"),
                    none.clone(),
                ),
                (
                    "a hidden named item",
                    format!(
                        "{a}forbid (principal, action == Improv::Action::\"ReadItem\", resource == Improv::Item::\"{}\");",
                        i("r1")
                    ),
                    none.clone(),
                ),
                (
                    "a hidden unnamed class",
                    format!(
                        "{a}forbid (principal, action == Improv::Action::\"ReadItem\", resource in Improv::Category::\"{}\") \
                         unless {{ resource == Improv::Item::\"{}\" }};",
                        self.r,
                        i("r2")
                    ),
                    none.clone(),
                ),
                (
                    "a category with nothing readable",
                    format!(
                        "{a}forbid (principal, action == Improv::Action::\"ReadItem\", resource in Improv::Category::\"{}\");",
                        self.c
                    ),
                    none.clone(),
                ),
                (
                    "a forbid on cells of two named items (not per category)",
                    format!(
                        "{a}forbid (principal, action == Improv::Action::\"ReadCell\", resource) \
                         when {{ resource in Improv::Item::\"{}\" && resource in Improv::Item::\"{}\" }};",
                        i("r1"),
                        i("c1")
                    ),
                    none.clone(),
                ),
                (
                    "ViewModel denied",
                    format!("{a}forbid (principal == Improv::User::\"eq_a\", action == Improv::Action::\"ViewModel\", resource);"),
                    none.clone(),
                ),
                (
                    "ReadMeasure denied",
                    format!(
                        "{a}forbid (principal, action == Improv::Action::\"ReadMeasure\", resource == Improv::Measure::\"{}\");",
                        self.w
                    ),
                    none.clone(),
                ),
                (
                    "mixed, as the benchmark",
                    format!(
                        "{a}forbid (principal, action == Improv::Action::\"ReadItem\", resource == Improv::Item::\"{}\");\n\
                         forbid (principal == Improv::User::\"u1\", action, resource in Improv::Item::\"{}\");\n\
                         forbid (principal == Improv::User::\"u2\", action, resource in Improv::Item::\"{}\");",
                        i("r0"),
                        i("r1"),
                        i("c2")
                    ),
                    none.clone(),
                ),
                (
                    "a policy for someone else",
                    "permit (principal == Improv::User::\"nobody\", action, resource);".into(),
                    none,
                ),
            ]
        }

        /// Random policy sets over the fixture's ids (a fixed seed, so a
        /// failure reproduces), each valid.
        fn random(&self, n: usize) -> Vec<(String, String)> {
            let mut x: u64 = 0x2545_f491_4f6c_dd1d;
            let mut next = |k: usize| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x % k as u64) as usize
            };
            let items: Vec<i64> = ["r0", "r1", "r2", "r3", "c0", "c1"]
                .iter()
                .map(|n| self.item(n))
                .collect();
            let mut out = Vec::new();
            while out.len() < n {
                let mut text = String::new();
                for k in 0..1 + next(4) {
                    let effect = if k == 0 && next(10) < 8 {
                        "permit"
                    } else {
                        ["permit", "forbid"][next(2)]
                    };
                    let who = [
                        "principal",
                        "principal == Improv::User::\"eq_a\"",
                        "principal in Improv::Role::\"eq_staff\"",
                        "principal in Improv::Group::\"g\"",
                    ][next(4)];
                    let (x1, x2) = (items[next(items.len())], items[next(items.len())]);
                    let cat = [self.r, self.c][next(2)];
                    let (action, resource, when): (String, String, String) = match next(9) {
                        0 => ("action in Improv::Action::\"Read\"".into(), "resource".into(), String::new()),
                        1 => ("action".into(), "resource".into(), String::new()),
                        2 => (
                            "action == Improv::Action::\"ReadItem\"".into(),
                            format!("resource == Improv::Item::\"{x1}\""),
                            String::new(),
                        ),
                        3 => (
                            "action == Improv::Action::\"ReadItem\"".into(),
                            format!("resource in Improv::Category::\"{cat}\""),
                            String::new(),
                        ),
                        4 => (
                            "action == Improv::Action::\"ReadCell\"".into(),
                            format!("resource in Improv::Item::\"{x1}\""),
                            String::new(),
                        ),
                        5 => (
                            "action == Improv::Action::\"ReadCell\"".into(),
                            "resource".into(),
                            format!(
                                " when {{ resource in Improv::Item::\"{x1}\" && resource in Improv::Item::\"{x2}\" }}"
                            ),
                        ),
                        6 => (
                            "action == Improv::Action::\"ReadMeasure\"".into(),
                            format!("resource == Improv::Measure::\"{}\"", [self.w, self.s, self.t][next(3)]),
                            String::new(),
                        ),
                        7 => ("action == Improv::Action::\"ViewModel\"".into(), "resource".into(), String::new()),
                        _ => ("action".into(), format!("resource in Improv::Item::\"{x1}\""), String::new()),
                    };
                    text += &format!("{effect} ({who}, {action}, {resource}){when};\n");
                }
                let groups = [
                    r#"{"g": {"users": ["eq_a"]}}"#,
                    r#"{"g": {"users": ["eq_b"]}}"#,
                    "",
                ][next(3)];
                if improv_policy::Policies::parse(&text, groups).is_ok() {
                    out.push((text, groups.to_string()));
                }
            }
            out
        }

        fn set(&self, text: &str, groups: &str) {
            Spi::run_with_args(
                "SELECT improv.set_policies('q', $1, $2)",
                &[text.into(), groups.into()],
            )
            .unwrap_or_else(|e| panic!("{text}: {e}"));
        }

        /// As a store's save does: a structure change moves the version on.
        fn bump(&self) {
            run(&format!(
                "UPDATE improv.model_version SET tx = tx + 1 WHERE model_id = {}",
                self.mid
            ));
        }
    }

    /// The result tables' qual (`cell_rule` once a statement, then
    /// `row_visible` a row) shows each role exactly the rows the per-row
    /// `cell_visible` shows, for a fixed matrix of policies and 40 random
    /// ones, with no policies, and as the model's tables go stale: rows
    /// whose names the model lacks, a forbidden item renamed (its old
    /// rows keep the old name), an item deleted, and both then recomputed.
    #[pg_test]
    fn the_qual_shows_what_cell_visible_shows() {
        let q = eq_setup();
        let mut sets: Vec<(String, String, String)> = (q.matrix().into_iter())
            .map(|(n, t, g)| (n.to_string(), t, g))
            .collect();
        sets.extend(
            (q.random(40).into_iter().enumerate()).map(|(i, (t, g))| (format!("random {i}"), t, g)),
        );
        let states: [(&str, &dyn Fn()); 6] = [
            ("fresh", &|| {}),
            ("clean: recomputed", &|| {
                run("SELECT improv.recalc('q')");
            }),
            ("rows with names the model lacks", &|| {
                run("INSERT INTO improv_q.w VALUES ('zz', 'c0', 1), ('r0', 'zz', 2)");
                run("INSERT INTO improv_q.s VALUES ('zz', 3)");
            }),
            ("a forbidden item renamed", &|| {
                run(&format!(
                    "UPDATE improv.item SET name = 'pub' WHERE id = {}",
                    q.item("r1")
                ));
                q.bump();
            }),
            ("an item deleted", &|| {
                run(&format!(
                    "DELETE FROM improv.item WHERE id = {}",
                    q.item("r3")
                ));
                q.bump();
            }),
            ("renamed and deleted, then recalc", &|| {
                run("SELECT improv.recalc('q')");
            }),
        ];
        for (state, change) in states {
            change();
            run("SELECT improv.set_policies('q', NULL)");
            q.check(&format!("{state}, no policies"));
            for (name, text, groups) in &sets {
                q.set(text, groups);
                q.check(&format!("{state}, {name}:\n{text}"));
            }
            // Not vacuous: the leak a name list would have. r1 is forbidden
            // by id; renamed, its stale rows still say r1 and stay hidden.
            q.set(&sets[3].1, "");
            let read = |sql: &str| text_as(Some("eq_a"), sql);
            let want = match state {
                // r3's rows name an item the model no longer has.
                "an item deleted" => "16:0",
                "renamed and deleted, then recalc" => "24:4",
                _ => "20:0",
            };
            // The fast path only when the rows are vouched for.
            let rule = text_as(
                Some("eq_a"),
                &format!("SELECT improv.cell_rule({}, {})::text", q.mid, q.w),
            );
            let fast = matches!(
                state,
                "clean: recomputed" | "renamed and deleted, then recalc"
            );
            assert_eq!(
                rule,
                if fast { "2" } else { "3" },
                "{state}: the rule for one hidden name"
            );
            assert_eq!(
                read("SELECT count(*) || ':' || count(*) FILTER (WHERE r = 'r1') FROM improv_q.w"),
                want,
                "{state}: r1's rows (a recalc mints a new, unforbidden r1)"
            );
        }
    }

    /// Stored policies that do not validate (here: a policy naming an
    /// attribute the schema does not declare, written around
    /// set_policies) deny every row through the qual whatever rule the
    /// valid policies gave: cell_rule says 1 and row_visible and
    /// cell_visible say false, for every policy of the matrix. The owner's
    /// side reads on.
    #[pg_test]
    fn unvalidated_policies_hide_every_row_for_every_rule() {
        let q = eq_setup();
        let bad = "permit (principal, action, resource) when { resource.nope == \"x\" };";
        assert!(improv_policy::Policies::parse(bad, "").is_err());
        run("ALTER TABLE improv.policy DISABLE ROW LEVEL SECURITY");
        let mut rules = std::collections::BTreeSet::new();
        for (name, text, groups) in q.matrix() {
            q.set(&text, &groups);
            // Clean, so the valid policies' rule may be 0 or 2.
            run("SELECT improv.recalc('q')");
            for role in ["eq_a", "eq_b"] {
                rules.insert(text_as(
                    Some(role),
                    &format!("SELECT improv.cell_rule({}, {})::text", q.mid, q.w),
                ));
            }
            Spi::run_with_args(
                "UPDATE improv.policy SET version = version + 1, text = text || $1",
                &[bad.into()],
            )
            .unwrap();
            for role in ["eq_a", "eq_b"] {
                for t in ["w", "s", "t"] {
                    let n = text_as(
                        Some(role),
                        &format!("SELECT count(*)::text FROM improv_q.{t}"),
                    );
                    assert_eq!(n, "0", "{name}: {role} reads {t}");
                }
                let probe = format!(
                    "SELECT improv.cell_rule({0}, {1}) || ':' || improv.row_visible({0}, {1}, 'r4'::text, 'c3'::text) \
                     || ':' || improv.cell_visible({0}, {1}, ARRAY['r4', 'c3'])",
                    q.mid, q.w
                );
                assert_eq!(
                    text_as(Some(role), &probe),
                    "1:false:false",
                    "{name}: {role}"
                );
            }
            assert_eq!(
                text_as(Some("eq_o"), "SELECT count(*)::text FROM improv_q.w"),
                "24",
                "{name}"
            );
            assert_eq!(
                text_as(None, "SELECT count(*)::text FROM improv_q.w"),
                "24",
                "{name}"
            );
        }
        let seen: Vec<String> = rules.into_iter().collect();
        assert_eq!(
            seen,
            ["0", "1", "2", "3"],
            "the valid policies covered every rule"
        );
    }

    /// cell_rule's answer for the matrix's plain cases.
    #[pg_test]
    fn cell_rule_decides_per_statement() {
        let q = eq_setup();
        let rule = |role: &str, m: i64| {
            text_as(
                Some(role),
                &format!("SELECT improv.cell_rule({}, {m})::text", q.mid),
            )
        };
        assert_eq!(rule("eq_a", q.w), "0", "no policies");
        let m = q.matrix();
        let case = |n: &str| m.iter().find(|c| c.0 == n).unwrap();
        for (n, role, measure, want) in [
            ("uniform allow, by user and by role", "eq_a", q.w, "3"),
            ("uniform allow, by user and by role", "eq_a", q.s, "3"),
            ("uniform allow, by user and by role", "eq_o", q.w, "0"),
            ("uniform deny of cells", "eq_a", q.w, "1"),
            ("a category with nothing readable", "eq_a", q.w, "1"),
            ("a category with nothing readable", "eq_a", q.s, "3"),
            ("ViewModel denied", "eq_a", q.w, "1"),
            ("ReadMeasure denied", "eq_b", q.w, "1"),
            ("ReadMeasure denied", "eq_b", q.s, "3"),
            ("a policy for someone else", "eq_b", q.w, "1"),
        ] {
            let c = case(n);
            q.set(&c.1, &c.2);
            assert_eq!(rule(role, measure), want, "{n}: {role}");
        }
        // Clean (a recalc after the policies were set): a uniform allow is
        // rule 0, one hidden name rule 2; with three, rows are checked.
        let (a, i) = (q.allow(), |n: &str| q.item(n));
        let forbid = |names: &[&str]| {
            let mut t = a.clone();
            for n in names {
                t += &format!(
                    "forbid (principal, action == Improv::Action::\"ReadItem\", resource == Improv::Item::\"{}\");\n",
                    i(n)
                );
            }
            t
        };
        for (names, want) in [
            (&[][..], "0"),
            (&["r1"][..], "2"),
            (&["r1", "c2"][..], "2"),
            (&["r1", "r2"][..], "2"),
            (&["r1", "r2", "r3"][..], "3"),
        ] {
            q.set(&forbid(names), "");
            run("SELECT improv.recalc('q')");
            assert_eq!(rule("eq_a", q.w), want, "clean, hidden {names:?}");
            run(&format!(
                "UPDATE improv.item SET name = name WHERE id = {}",
                i("r4")
            ));
            assert_eq!(
                rule("eq_a", q.w),
                want,
                "a no-op item UPDATE keeps it clean"
            );
            run(&format!(
                "UPDATE improv.item SET name = 'x' || name WHERE id = {}",
                i("r4")
            ));
            let stale = if want == "0" || want == "2" {
                "3"
            } else {
                want
            };
            assert_eq!(rule("eq_a", q.w), stale, "a rename: hidden {names:?}");
            run(&format!(
                "UPDATE improv.item SET name = substr(name, 2) WHERE id = {}",
                i("r4")
            ));
        }
        // A result row written by hand marks it stale too.
        q.set(&forbid(&["r1"]), "");
        run("SELECT improv.recalc('q')");
        assert_eq!(rule("eq_a", q.w), "2");
        run("INSERT INTO improv_q.w VALUES ('zz', 'c0', 1)");
        assert_eq!(rule("eq_a", q.w), "3", "a row written by hand");
        run("SELECT improv.recalc('q')");
        assert_eq!(
            rule("eq_a", q.w),
            "2",
            "a recalc vouches for the rows again"
        );
        // Without its improv.stale row a model is never clean.
        run("ALTER TABLE improv.stale DISABLE ROW LEVEL SECURITY");
        run(&format!(
            "DELETE FROM improv.stale WHERE model_id = {}",
            q.mid
        ));
        assert_eq!(rule("eq_a", q.w), "3", "no stale row");
        run(&format!(
            "INSERT INTO improv.stale (model_id, gen, clean_gen) VALUES ({}, 5, NULL)",
            q.mid
        ));
        assert_eq!(rule("eq_a", q.w), "3", "a NULL clean_gen");
    }
}
