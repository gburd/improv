//! Persist an Improv `Model` to the embedded (SQLite-backed) Mentat store.
//!
//! The model is stored as datoms using the schema from IMPROV.txt
//! ("Mentat schema (steering version)"): categories, items, measures, and
//! input cells are first-class entities so the model is *queryable*, not an
//! opaque blob.
//!
//! Formulas and coordinates are stored as JSON strings (the AST/coordinate are
//! serialized with serde); everything else maps to native datom value types.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

use improv_core_model::{
    Category, CategoryId, Coordinate, Formula, Item, ItemId, Measure, MeasureId, MeasureKind,
    Model, Name, Value, ValueType,
};
use mentat::{InProgress, Store, TypedValue};
use std::collections::HashMap;

mod convert;
mod schema;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("mentat error: {0}")]
    Mentat(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("data integrity: {0}")]
    Integrity(String),
    /// Another writer committed to the store after this handle's model was
    /// loaded; saving would clobber (and retract) its changes.
    #[error("conflict: {0}")]
    Conflict(String),
}

/// The store's high-water mark: the newest transaction that changed a datom
/// other than `:db/txInstant` (entid 3, a Mentat bootstrap constant), read
/// from the raw transaction log. Read from SQLite rather than
/// `Store::last_tx_id`, whose cached partition map never sees another
/// process's commits. Skipping `txInstant` ignores the no-op schema transact
/// every [`ModelStore::open`] performs, so a reader merely opening the store
/// does not count as a writer. Scans the log backwards by rowid, so it touches
/// only the latest transaction's rows.
const TX_HIGH_WATER_SQL: &str = "SELECT coalesce((SELECT tx FROM timelined_transactions \
     WHERE timeline = 0 AND a <> 3 ORDER BY rowid DESC LIMIT 1), 0)";

const CONFLICT_MSG: &str =
    "the store was changed by another writer since it was loaded; reload and retry";

fn high_water(ip: &InProgress) -> Result<i64> {
    ip.transaction
        .query_row(TX_HIGH_WATER_SQL, [], |r| r.get(0))
        .map_err(|e| StoreError::Mentat(e.to_string()))
}

impl From<mentat::errors::MentatError> for StoreError {
    fn from(e: mentat::errors::MentatError) -> Self {
        StoreError::Mentat(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Upper bound on the number of datoms Improv puts in a single
/// `InProgress::transact` call.
///
/// Mentat's `insert_non_fts_searches` (`../mentat/db/src/db.rs`) splits one
/// transact's datoms into chunks of `SQLITE_LIMIT_VARIABLE_NUMBER /
/// bindings_per_statement` = `32766 / 6` = 5461, then asserts
/// `bindings_per_statement * count < max_vars`. For a *full* chunk that reads
/// `6 * 5461 < 32766`, which is false — so any transact carrying 5461 or more
/// datoms aborts the process with `Too many values: 6 * 5461 >= 32766`. It is a
/// hard `assert!`, not a `Result`, so it cannot be caught or retried; the only
/// fix on our side is to never hand Mentat that many datoms at once.
///
/// 2000 leaves ~2.7x headroom under 5461. The headroom is deliberate: a re-save
/// of an existing entity turns into a retraction plus an assertion for each
/// changed cardinality-one attribute, so the datom count Mentat actually sees
/// can exceed the count we emit.
const MAX_DATOMS_PER_TRANSACT: usize = 2000;

/// The datom count at which one transact aborts the process (see
/// `MAX_DATOMS_PER_TRANSACT`): `32766 / 6`.
///
/// Chunking keeps *groups* of entities under this, but it cannot help when ONE
/// entity is this wide by itself — an entity map is indivisible, so the widest
/// single entity is the hard floor on what a transact must carry. Improv has
/// exactly one entity kind whose width scales with model data:
/// `:measure/categories` is `:db.cardinality/many`, so a measure with N
/// categories emits N ref datoms (plus ~7 fixed attributes). `save_model`
/// therefore rejects an over-wide measure with `StoreError::Integrity` instead
/// of handing Mentat a transact that kills the process.
///
/// The ceiling applies *per attribute queue*, not per transact: Mentat's
/// transactor splits a transact's datoms into cardinality-many
/// (`SearchType::Exact`) and cardinality-one (`Inexact`) queues and calls
/// `insert_non_fts_searches` once per queue, each with its own chunking and its
/// own `assert!`. That is why a measure with 5460 categories saves even though
/// it emits 5467 datoms: 5460 land in the many-queue and 7 in the one-queue.
/// The binding constraint is the category count alone, so the guard checks that
/// and not the padded `datoms_per_entity` bound (checking the padded bound would
/// reject the 5454..=5460 category range, which is measurably fine today).
const MAX_DATOMS_PER_TRANSACT_QUEUE: usize = 32766 / 6;

/// Most `:measure/categories` refs one measure can carry and still be savable
/// (5460 — verified as the last good value by
/// `measure_at_the_single_entity_ceiling_round_trips`).
const MAX_CATEGORIES_PER_MEASURE: usize = MAX_DATOMS_PER_TRANSACT_QUEUE - 1;

/// Transact `entities` through `ip` in EDN vectors of at most
/// `MAX_DATOMS_PER_TRANSACT / datoms_per_entity` entities each, sharing the
/// underlying SQLite transaction with every other call made on the same
/// `InProgress` (a no-op if `entities` is empty).
///
/// `datoms_per_entity` is an *upper bound* on the attributes one element of
/// `entities` asserts (see `MAX_DATOMS_PER_TRANSACT` for why the bound matters).
///
/// Chunking happens *inside* the caller's `InProgress`: many `transact` calls,
/// still exactly one `commit()`, so `save_model` stays all-or-nothing — a
/// failure in chunk 7 of 24 rolls back chunks 1..7 along with every other step
/// of the save.
fn transact_group(
    ip: &mut InProgress<'_, '_>,
    datoms_per_entity: usize,
    entities: impl IntoIterator<Item = String>,
) -> Result<()> {
    let parts: Vec<String> = entities.into_iter().collect();
    // `.max(1)` only bites for an entity so wide it exceeds the whole budget by
    // itself; a single entity is indivisible, so one-per-transact is the best we
    // can do. That is NOT automatically under Mentat's real ceiling — an entity
    // wide enough to exceed MAX_DATOMS_PER_TRANSACT_QUEUE on its own still
    // aborts, which is why `save_model` guards the one entity kind whose width
    // is unbounded (`:measure/categories`) before it gets here.
    let per_chunk = (MAX_DATOMS_PER_TRANSACT / datoms_per_entity.max(1)).max(1);
    for batch in parts.chunks(per_chunk) {
        ip.transact(format!("[{}]", batch.join("\n")))?;
    }
    Ok(())
}

const VALUE_NUMBER: &str = "value-number";
const VALUE_BOOLEAN: &str = "value-boolean";
const VALUE_TEXT: &str = "value-text";

/// The `:cell/<attr>` a value of type `vt` is stored under (see
/// `convert::cell_edn`).
fn value_attr(vt: ValueType) -> &'static str {
    match vt {
        ValueType::Number | ValueType::Enum => VALUE_NUMBER,
        ValueType::Boolean => VALUE_BOOLEAN,
        ValueType::Text | ValueType::DateTime => VALUE_TEXT,
    }
}

/// A model store backed by embedded Mentat (a single SQLite file, or `""` for
/// in-memory).
pub struct ModelStore {
    store: Store,
    /// The high-water as of this handle's last load or save; `save_model`
    /// refuses to commit if the store has moved past it. `None`: unchecked.
    loaded_tx: Option<i64>,
    /// The measures whose cells the last load read: `None` after a full load
    /// (or none), the dependency closure after [`ModelStore::load_partial`].
    /// `save_model` only retracts stored cells of these measures, so saving a
    /// partial model leaves every other measure's cells alone.
    loaded_scope: Option<std::collections::HashSet<MeasureId>>,
    /// Test-only: how many `(measure, coord)` rows the last `load_cells` query
    /// *returned* (not how many survived a Rust-side filter). Lets a test prove
    /// the enumeration itself is bounded by `load_partial`'s closure rather
    /// than by the size of the whole store.
    #[cfg(test)]
    cell_rows_returned: usize,
}

impl ModelStore {
    /// Open (or create) a store at `path`. Use `""` for an in-memory database.
    pub fn open(path: &str) -> Result<Self> {
        let mut store = Store::open(path)?;
        store.transact(schema::SCHEMA_EDN)?;
        Ok(ModelStore {
            store,
            loaded_tx: None,
            loaded_scope: None,
            #[cfg(test)]
            cell_rows_returned: 0,
        })
    }

    /// Persist the entire model, atomically: either every category, item,
    /// measure, cell, view and meta-blob is durably saved, or (on any error)
    /// none of them are.
    ///
    /// Idempotent for identity-unique entities (categories/items/measures
    /// keyed by their id; cells keyed by measure+coord), so re-saving updates
    /// in place.
    ///
    /// Cells are diffed against the store, which ends up holding exactly
    /// `model.inputs`: unchanged cells add no datoms, and a stored cell the
    /// model lacks is retracted. After [`ModelStore::load_partial`] only the
    /// loaded closure's cells are diffed that way; cells of measures the load
    /// skipped are left as they are.
    ///
    /// Transacted in dependency order (categories, then items, then measures,
    /// then cells, then views, then meta) as several `InProgress::transact`
    /// calls sharing ONE underlying SQLite transaction (via
    /// `Store::begin_transaction`), so later lookup-refs resolve against
    /// earlier (still-uncommitted) writes in the same save. A single
    /// `ip.commit()` at the end makes the whole sequence all-or-nothing: if
    /// any step's EDN fails to transact, `?` returns before `commit()` runs
    /// and the dropped `InProgress` rolls back every prior step of this save.
    /// See `.agent/steering/AGENT_DATABASE_CONNECTIVITY.md` "Crash safety".
    ///
    /// Each step is further split into several `transact` calls of bounded
    /// datom count (see `MAX_DATOMS_PER_TRANSACT`), because Mentat aborts the
    /// process on a transact of 5461+ datoms. The extra calls are still inside
    /// the same single `InProgress`/`commit()`, so atomicity is unaffected.
    ///
    /// Optimistic concurrency: if this handle loaded (or saved) the model, the
    /// save fails with [`StoreError::Conflict`], writing nothing, when another
    /// writer has committed since. A handle that never loaded saves
    /// unconditionally.
    ///
    /// Chunking cannot rescue a *single* entity wider than that ceiling, so a
    /// measure with more than `MAX_CATEGORIES_PER_MEASURE` (5460) categories is
    /// rejected up front with `StoreError::Integrity` — a recoverable error
    /// instead of a killed process.
    pub fn save_model(&mut self, model: &Model) -> Result<()> {
        // IMMEDIATE: no other writer can commit between this check and ours.
        let mut ip = self.store.begin_transaction()?;
        if let Some(loaded) = self.loaded_tx {
            if high_water(&ip)? != loaded {
                return Err(StoreError::Conflict(CONFLICT_MSG.into()));
            }
        }

        // `datoms_per_entity` arguments below are upper bounds on the
        // attributes each `convert::*_edn` helper can emit for one entity.
        // :category/id + :category/name.
        transact_group(
            &mut ip,
            2,
            model.categories.values().map(convert::category_edn),
        )?;
        // :item/id + :item/name + :item/category.
        transact_group(&mut ip, 3, model.items.values().map(convert::item_edn))?;

        let mut measures = Vec::new();
        for m in model.measures.values() {
            measures.push(convert::measure_edn(m, model.sql_sources.get(&m.id))?);
        }
        // A measure is one indivisible entity, and `:measure/categories` is
        // cardinality-many, so its width is the only model-data-dependent width
        // in the schema. Chunking cannot split it: past the per-queue ceiling
        // Mentat aborts the process, so refuse the save with a recoverable error
        // instead (`ip` is dropped un-committed here, so nothing persists).
        //
        // NOTE (interacts with the missing-retraction bug): `save_model` never
        // retracts stale `:measure/categories` refs, so re-saving a measure whose
        // category set *changed* leaves the union on disk. A later `load_model`
        // hands back that union, which is how a model can grow past this limit
        // without any single in-memory save ever being that wide. The guard is
        // still sound, because Mentat's `assert!` counts only the datoms in the
        // transact being applied — on-disk accumulation for a cardinality-many
        // attribute adds no rows to the search tables (proven by
        // `churned_resave_reports_error_not_abort`). The consequence of the
        // retraction bug is that the *error* can appear on a re-save of a loaded
        // model whose author never built a measure that wide; fixing retraction
        // removes that surprise, not this guard.
        let widest_categories = model
            .measures
            .values()
            .map(|m| m.categories.len())
            .max()
            .unwrap_or(0);
        if widest_categories > MAX_CATEGORIES_PER_MEASURE {
            let worst = model
                .measures
                .values()
                .max_by_key(|m| m.categories.len())
                .expect("non-empty: widest_categories > 0");
            return Err(StoreError::Integrity(format!(
                "measure {} (\"{}\") has {} categories; a measure is a single \
                 indivisible entity and the store aborts on a transact carrying \
                 {} or more datoms for one attribute, so at most {} categories \
                 per measure can be saved",
                worst.id.0,
                worst.name.0,
                widest_categories,
                MAX_DATOMS_PER_TRANSACT_QUEUE,
                MAX_CATEGORIES_PER_MEASURE,
            )));
        }
        // id, name, value-type, kind, formula, description, sql-source, plus
        // one :measure/categories datom per category (cardinality-many); take
        // the widest measure in this model rather than guessing.
        transact_group(&mut ip, 7 + widest_categories, measures)?;

        // Diff the cells against what is stored, so the store's cells end up
        // exactly `model.inputs`: an unchanged cell is not re-asserted, a value
        // left under another type's attribute is retracted, and a stored cell
        // the model no longer has is retracted whole (it used to linger on
        // disk and come back on the next load).
        let mut stored = stored_cells(&ip)?;
        let mut cells = Vec::new();
        let mut retracts = Vec::new();
        for ((mid, coord), val) in model.inputs.iter() {
            let coord_json = serde_json::to_string(coord)?;
            let key = convert::cell_key(*mid, &coord_json);
            let (attr, v) = convert::cell_value_edn(val)?;
            if let Some(old) = stored.remove(&key) {
                let mut unchanged = false;
                for (a, ov) in &old.values {
                    if *a == attr {
                        // Cardinality-one: asserting the new value replaces it.
                        unchanged = *ov == v;
                    } else {
                        retracts.push(format!("[:db/retract {} :cell/{a} {ov}]", old.e));
                    }
                }
                if unchanged {
                    continue;
                }
            }
            cells.push(convert::cell_edn(*mid, &coord_json, &key, attr, &v));
        }
        for (key, old) in stored {
            // A partial load never saw this measure's cells: not ours to drop.
            if let Some(scope) = &self.loaded_scope {
                let mid = key.split_once("::").and_then(|(m, _)| m.parse().ok());
                if !mid.is_some_and(|m| scope.contains(&MeasureId(m))) {
                    continue;
                }
            }
            let e = old.e;
            retracts.push(format!(
                "[:db/retract {e} :cell/key {}]",
                convert::edn_str_pub(&key)
            ));
            retracts.push(format!("[:db/retract {e} :cell/measure {}]", old.measure));
            retracts.push(format!(
                "[:db/retract {e} :cell/coord {}]",
                convert::edn_str_pub(&old.coord)
            ));
            for (a, ov) in &old.values {
                retracts.push(format!("[:db/retract {e} :cell/{a} {ov}]"));
            }
        }
        // One datom per retraction.
        transact_group(&mut ip, 1, retracts)?;
        // :cell/key + :cell/measure + :cell/coord + one typed value attribute.
        transact_group(&mut ip, 4, cells)?;

        let mut views = Vec::new();
        for v in model.views.values() {
            let json = serde_json::to_string(v)?;
            views.push(format!(
                "{{:view/id {} :view/json {}}}",
                v.id.0,
                convert::edn_str_pub(&json)
            ));
        }
        // :view/id + :view/json.
        transact_group(&mut ip, 2, views)?;

        // Singleton meta: external function defs + external-call measures +
        // what-if scenarios, each as a JSON blob on one entity (only if any
        // are non-empty).
        if !model.external_fns.is_empty()
            || !model.external_calls.is_empty()
            || !model.scenarios.is_empty()
            || !model.lambdas.is_empty()
        {
            let fns_json = serde_json::to_string(&model.external_fns)?;
            let calls_json = serde_json::to_string(&model.external_calls)?;
            let scen_json = serde_json::to_string(&model.scenarios)?;
            let lambdas_json = serde_json::to_string(&model.lambdas)?;
            let edn = format!(
                "[{{:meta/singleton 1 :meta/external-fns {} :meta/external-calls {} :meta/scenarios {} :meta/lambdas {}}}]",
                convert::edn_str_pub(&fns_json),
                convert::edn_str_pub(&calls_json),
                convert::edn_str_pub(&scen_json),
                convert::edn_str_pub(&lambdas_json),
            );
            ip.transact(edn)?;
        }

        // Read before commit, under our write lock, so a writer racing in right
        // after the commit is never mistaken for our own transaction.
        let new_tx = high_water(&ip)?;
        ip.commit()?;
        self.loaded_tx = Some(new_tx);
        Ok(())
    }

    /// The store's latest transaction id, read live from SQLite (so it sees
    /// other processes' commits).
    pub fn tx_high_water(&mut self) -> Result<i64> {
        high_water(&self.store.begin_read()?.in_progress)
    }

    /// The high-water this handle last loaded or saved at (`None` if it has
    /// done neither). Callers that reopen the store for each save carry it
    /// across handles with [`ModelStore::set_loaded_tx`].
    pub fn loaded_tx(&self) -> Option<i64> {
        self.loaded_tx
    }

    /// Adopt `tx` (an earlier handle's [`ModelStore::loaded_tx`]) as the base
    /// `save_model` checks against. `None` saves unconditionally.
    pub fn set_loaded_tx(&mut self, tx: Option<i64>) {
        self.loaded_tx = tx;
    }

    /// Re-read the model after a [`StoreError::Conflict`]; same as
    /// [`ModelStore::load_model`].
    pub fn reload_model(&mut self) -> Result<Model> {
        self.load_model()
    }

    /// Reconstruct the model by querying the store.
    pub fn load_model(&mut self) -> Result<Model> {
        let tx = self.tx_high_water()?;
        let mut model = Model::new();
        self.load_categories(&mut model)?;
        self.load_items(&mut model)?;
        self.load_measures(&mut model)?;
        self.load_cells(&mut model, None)?;
        self.load_views(&mut model)?;
        self.load_meta(&mut model)?;
        self.loaded_tx = Some(tx);
        self.loaded_scope = None;
        Ok(model)
    }

    /// Load a model containing only the categories/items/measures/cells that
    /// an operation over `measure_ids` actually needs: `measure_ids` plus
    /// their transitive dependency closure (`Model::measure_dependency_closure`
    /// — every measure reachable via a `Derived` formula or an external-call's
    /// `arg_measures`). Views/scenarios/external-fn defs/meta are still loaded
    /// in full (they're small, bounded by model *shape* not data volume, not
    /// data-scale-sensitive like cells are).
    ///
    /// This is the "windowed" load recommended in
    /// `.agent/steering/AGENT_OUT_OF_CORE_DESIGN.md` §4: `engine::dataflow::
    /// evaluate`/`engine::session::Engine::new` need ZERO changes, because they
    /// already only touch whatever is in the `Model` they're handed — the gap
    /// was purely that `load_model` always loaded every cell of every measure
    /// regardless of what the caller needed. Categories/items/measures are
    /// still loaded in full too (their volume scales with model *shape*, which
    /// is the cheap part; only `load_cells`, whose volume scales with *data*,
    /// is filtered). A single operation whose formula-dependency closure is a
    /// bounded subset of a huge model can now avoid paying for every other
    /// measure's cells — it does NOT help an operation that touches the whole
    /// model by definition (e.g. a grand total over everything).
    ///
    /// The cell filter is applied *in the store*: `load_cells` binds `?mid` to
    /// exactly the closure's measure ids with `ground`, so the query returns
    /// rows only for those measures — no row is materialized or returned for a
    /// cell outside the closure. See `load_cells` for the one residual cost
    /// this does NOT remove (SQLite still scans the cell datom slices inside
    /// the query; only the returned row set is closure-bounded).
    ///
    /// Saving the result with [`ModelStore::save_model`] writes the closure's
    /// cells and leaves every other measure's cells alone.
    pub fn load_partial(&mut self, measure_ids: &[MeasureId]) -> Result<Model> {
        let tx = self.tx_high_water()?;
        let mut model = Model::new();
        self.load_categories(&mut model)?;
        self.load_items(&mut model)?;
        self.load_measures(&mut model)?;
        self.load_views(&mut model)?;
        self.load_meta(&mut model)?; // external_calls populated before the closure walk
        let closure = model.measure_dependency_closure(measure_ids);
        self.load_cells(&mut model, Some(&closure))?;
        self.loaded_tx = Some(tx);
        self.loaded_scope = Some(closure);
        Ok(model)
    }

    // --- load: query each entity kind ---

    fn load_categories(&mut self, model: &mut Model) -> Result<()> {
        let q = "[:find ?id ?name :where [?e :category/id ?id] [?e :category/name ?name]]";
        for row in self.rel(q)? {
            let id = CategoryId(convert::as_u32(&row[0])?);
            let name = convert::as_string(&row[1])?;
            model.categories.insert(
                id,
                Category {
                    id,
                    name: Name(name),
                    items: Vec::new(),
                },
            );
        }
        Ok(())
    }

    fn load_items(&mut self, model: &mut Model) -> Result<()> {
        let q = "[:find ?id ?cat ?name :where \
                  [?e :item/id ?id] \
                  [?e :item/category ?c] [?c :category/id ?cat] \
                  [?e :item/name ?name]]";
        for row in self.rel(q)? {
            let id = ItemId(convert::as_u32(&row[0])?);
            let category = CategoryId(convert::as_u32(&row[1])?);
            let name = convert::as_string(&row[2])?;
            model.items.insert(
                id,
                Item {
                    id,
                    category,
                    name: Name(name),
                },
            );
            if let Some(c) = model.categories.get_mut(&category) {
                c.items.push(id);
            }
        }
        Ok(())
    }

    fn load_measures(&mut self, model: &mut Model) -> Result<()> {
        // Required fields only; optional formula/description fetched per measure.
        let q = "[:find ?id ?name ?vt ?kind :where \
                  [?e :measure/id ?id] \
                  [?e :measure/name ?name] \
                  [?e :measure/value-type ?vt] \
                  [?e :measure/kind ?kind]]";
        for row in self.rel(q)? {
            let id = MeasureId(convert::as_u32(&row[0])?);
            let name = convert::as_string(&row[1])?;
            let value_type = convert::value_type_from_kw(&convert::as_string(&row[2])?)?;
            let kind_kw = convert::as_string(&row[3])?;

            let kind = match kind_kw.as_str() {
                "input" => MeasureKind::Input,
                "derived" => {
                    let formula_json = self
                        .scalar_string(&format!(
                            "[:find ?f . :where [?e :measure/id {}] [?e :measure/formula ?f]]",
                            id.0
                        ))?
                        .ok_or_else(|| {
                            StoreError::Integrity(format!(
                                "derived measure {} has no formula",
                                id.0
                            ))
                        })?;
                    let f: Formula = serde_json::from_str(&formula_json)?;
                    MeasureKind::Derived(f)
                }
                other => {
                    return Err(StoreError::Integrity(format!(
                        "unknown measure kind {other}"
                    )))
                }
            };

            let description = self.scalar_string(&format!(
                "[:find ?d . :where [?e :measure/id {}] [?e :measure/description ?d]]",
                id.0
            ))?;

            let categories = self.load_measure_categories(id)?;

            // SQL-source metadata (Phase 7), if this measure is SQL-backed.
            if let Some(json) = self.scalar_string(&format!(
                "[:find ?s . :where [?e :measure/id {}] [?e :measure/sql-source ?s]]",
                id.0
            ))? {
                let src: improv_core_model::SqlSource = serde_json::from_str(&json)?;
                model.sql_sources.insert(id, src);
            }

            model.measures.insert(
                id,
                Measure {
                    id,
                    name: Name(name),
                    value_type,
                    categories,
                    kind,
                    description,
                },
            );
        }
        Ok(())
    }

    fn load_measure_categories(&mut self, measure: MeasureId) -> Result<Vec<CategoryId>> {
        let q = format!(
            "[:find ?cat :where \
             [?e :measure/id {}] [?e :measure/categories ?c] [?c :category/id ?cat]]",
            measure.0
        );
        let mut cats = Vec::new();
        for row in self.rel(&q)? {
            cats.push(CategoryId(convert::as_u32(&row[0])?));
        }
        // `:measure/categories` is a set, so its query order is arbitrary.
        // Restore the saved order; a store written before
        // `:measure/category-order` existed falls back to id order, and an id
        // the order does not list (a stale ref) sorts after those it does.
        let order: Vec<u32> = match self.scalar_string(&format!(
            "[:find ?o . :where [?e :measure/id {}] [?e :measure/category-order ?o]]",
            measure.0
        ))? {
            Some(json) => serde_json::from_str(&json)?,
            None => Vec::new(),
        };
        cats.sort_by_key(|c| {
            (
                order.iter().position(|&o| o == c.0).unwrap_or(usize::MAX),
                c.0,
            )
        });
        Ok(cats)
    }

    fn load_cells(
        &mut self,
        model: &mut Model,
        filter: Option<&std::collections::HashSet<MeasureId>>,
    ) -> Result<()> {
        // One query per typed value attribute (three in all), each returning
        // (measure id, coord, value) rows, so the load is O(value types)
        // queries, not O(cells). It used to be one query for (mid, coord) and
        // then one `q_once` per cell for its value: 100k cells took ~290 s.
        //
        // The measure filter is applied IN the query, not in this loop: with a
        // `filter`, `?mid` is `ground`-bound to exactly the closure's measure
        // ids, so the store returns rows only for those measures. Nothing
        // proportional to the rest of the model's cells is materialized or
        // returned.
        //
        // Residual cost, stated plainly: the returned row set and all per-row
        // work are bounded by the closure, but Mentat plans either shape as a
        // scan of the `:cell/measure` / `:cell/coord` datom slices (`SEARCH ...
        // USING COVERING INDEX idx_datoms_aevt (a=?)`, verified with
        // `Store::q_explain`), so inside SQLite the query still walks every
        // cell datom. Making the scan itself seek-bounded needs a Mentat/schema
        // change (an AVET-indexed `:cell/measure` usable as a leading index
        // column), not a query rewrite.
        let ground = match filter {
            None => String::new(),
            Some(keep) if keep.is_empty() => {
                // `ground` rejects an empty collection, and an empty closure
                // needs no cells: skip the queries entirely.
                #[cfg(test)]
                {
                    self.cell_rows_returned = 0;
                }
                return Ok(());
            }
            Some(keep) => {
                // Sorted so the query text is stable/reproducible.
                let mut ids: Vec<u32> = keep.iter().map(|m| m.0).collect();
                ids.sort_unstable();
                let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
                format!("[(ground [{}]) [?mid ...]] ", ids.join(" "))
            }
        };
        #[cfg(test)]
        {
            self.cell_rows_returned = 0;
        }
        for attr in [VALUE_NUMBER, VALUE_BOOLEAN, VALUE_TEXT] {
            let q = format!(
                "[:find ?mid ?coord ?v :where {ground}\
                 [?m :measure/id ?mid] [?e :cell/measure ?m] \
                 [?e :cell/coord ?coord] [?e :cell/{attr} ?v]]"
            );
            let rows = self.rel(&q)?;
            #[cfg(test)]
            {
                self.cell_rows_returned += rows.len();
            }
            for row in rows {
                let mid = MeasureId(convert::as_u32(&row[0])?);
                let vt = model
                    .measures
                    .get(&mid)
                    .map(|m| m.value_type)
                    .ok_or_else(|| {
                        StoreError::Integrity(format!("cell for unknown measure {mid:?}"))
                    })?;
                let coord: Coordinate = serde_json::from_str(&convert::as_string(&row[1])?)?;
                let value = match (attr, vt) {
                    (VALUE_NUMBER, ValueType::Enum) => {
                        Value::Enum(convert::as_f64(&row[2])? as u32)
                    }
                    (VALUE_NUMBER, _) => Value::Number(convert::as_f64(&row[2])?),
                    (VALUE_BOOLEAN, _) => Value::Boolean(convert::as_bool(&row[2])?),
                    (_, ValueType::DateTime) => {
                        let text = convert::as_string(&row[2])?;
                        match chrono::DateTime::parse_from_rfc3339(&text) {
                            Ok(dt) => Value::DateTime(dt.with_timezone(&chrono::Utc)),
                            Err(_) => Value::Text(text),
                        }
                    }
                    _ => Value::Text(convert::as_string(&row[2])?),
                };
                // A cell carries one value attribute, but a store written
                // before re-saves retracted the old one may hold several: the
                // attribute matching the measure's declared type wins.
                let key = (mid, coord);
                if attr == value_attr(vt) || !model.inputs.contains_key(&key) {
                    model.inputs.insert(key, value);
                }
            }
        }
        Ok(())
    }

    fn load_views(&mut self, model: &mut Model) -> Result<()> {
        let q = "[:find ?json :where [?e :view/id _] [?e :view/json ?json]]";
        for row in self.rel(q)? {
            let json = convert::as_string(&row[0])?;
            let v: improv_core_model::View = serde_json::from_str(&json)?;
            model.views.insert(v.id, v);
        }
        Ok(())
    }

    fn load_meta(&mut self, model: &mut Model) -> Result<()> {
        if let Some(json) = self.scalar_string(
            "[:find ?j . :where [?e :meta/singleton 1] [?e :meta/external-fns ?j]]",
        )? {
            model.external_fns = serde_json::from_str(&json)?;
        }
        if let Some(json) = self.scalar_string(
            "[:find ?j . :where [?e :meta/singleton 1] [?e :meta/external-calls ?j]]",
        )? {
            model.external_calls = serde_json::from_str(&json)?;
        }
        if let Some(json) = self
            .scalar_string("[:find ?j . :where [?e :meta/singleton 1] [?e :meta/scenarios ?j]]")?
        {
            model.scenarios = serde_json::from_str(&json)?;
        }
        if let Some(json) =
            self.scalar_string("[:find ?j . :where [?e :meta/singleton 1] [?e :meta/lambdas ?j]]")?
        {
            model.lambdas = serde_json::from_str(&json)?;
        }
        Ok(())
    }

    fn scalar(&self, query: &str) -> Result<Option<TypedValue>> {
        use mentat::Queryable;
        let out = self.store.q_once(query, None)?;
        let s = out
            .results
            .into_scalar()
            .map_err(|e| StoreError::Mentat(e.to_string()))?;
        Ok(s.and_then(|b| b.into_scalar()))
    }

    fn scalar_string(&self, query: &str) -> Result<Option<String>> {
        match self.scalar(query)? {
            Some(tv) => Ok(Some(convert::as_string(&tv)?)),
            None => Ok(None),
        }
    }

    fn rel(&self, query: &str) -> Result<Vec<Vec<TypedValue>>> {
        rel_on(&self.store, query)
    }
}

/// A stored cell entity, as `save_model` needs it to diff or retract it.
struct StoredCell {
    e: i64,
    /// The `:cell/measure` ref's entity id.
    measure: i64,
    coord: String,
    /// (value attribute, EDN literal) for each typed value the entity holds.
    values: Vec<(&'static str, String)>,
}

/// Every stored cell, by `:cell/key`. Four queries, whatever the cell count.
fn stored_cells(q: &impl mentat::Queryable) -> Result<HashMap<String, StoredCell>> {
    let mut out = HashMap::new();
    let rows = rel_on(
        q,
        "[:find ?e ?key ?m ?coord :where \
         [?e :cell/key ?key] [?e :cell/measure ?m] [?e :cell/coord ?coord]]",
    )?;
    for row in rows {
        out.insert(
            convert::as_string(&row[1])?,
            StoredCell {
                e: convert::as_entid(&row[0])?,
                measure: convert::as_entid(&row[2])?,
                coord: convert::as_string(&row[3])?,
                values: Vec::new(),
            },
        );
    }
    for attr in [VALUE_NUMBER, VALUE_BOOLEAN, VALUE_TEXT] {
        let q_attr = format!("[:find ?key ?v :where [?e :cell/key ?key] [?e :cell/{attr} ?v]]");
        for row in rel_on(q, &q_attr)? {
            if let Some(c) = out.get_mut(&convert::as_string(&row[0])?) {
                c.values.push((attr, convert::stored_value_edn(&row[1])?));
            }
        }
    }
    Ok(out)
}

fn rel_on(q: &impl mentat::Queryable, query: &str) -> Result<Vec<Vec<TypedValue>>> {
    let out = q.q_once(query, None)?;
    let rel = out
        .results
        .into_rel()
        .map_err(|e| StoreError::Mentat(e.to_string()))?;
    Ok(rel
        .into_iter()
        .map(|row| {
            row.into_iter()
                .map(|b| b.into_scalar().expect("scalar binding"))
                .collect()
        })
        .collect())
}

// Silence unused-import warnings for HashMap in case the module trims; it's used
// transitively by Model. (Kept explicit for readability of the load path.)
#[allow(unused_imports)]
use HashMap as _HashMap;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::{BinaryOp, DimensionSpec, Expr, ItemId};

    fn sample_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(20), product, "Widget A");

        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: Some("Unit price".into()),
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
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m.add_view(improv_core_model::View {
            id: improv_core_model::ViewId(1),
            name: Name("Prices by product".into()),
            measure: MeasureId(100),
            axis_order: vec![CategoryId(2)],
            n_rows: 1,
            n_cols: 1,
            page_items: vec![],
            filters: vec![improv_core_model::Filter {
                category: CategoryId(2),
                items: vec![ItemId(20)],
            }],
            rect: Default::default(),
            placements: vec![],
        });

        // A named LAMBDA rides in the same blob.
        m.lambdas.insert(
            3,
            improv_core_model::NamedLambda {
                name: "Half".into(),
                params: vec!["x".into()],
                body: Expr::BinaryOp(
                    BinaryOp::Div,
                    Box::new(Expr::Ref(
                        improv_core_model::functions::lambda_param(0),
                        Default::default(),
                    )),
                    Box::new(Expr::Literal(Value::Number(2.0))),
                ),
            },
        );
        // Meta blob: an external function def, an external-call measure that
        // uses it, and a what-if scenario -- so `save_model`'s :meta/* step
        // (the last, easiest-to-miss step of the multi-step save) is non-empty
        // and covered by the round-trip.
        m.external_fns.insert(
            "double".into(),
            improv_core_model::ExternalFn {
                name: "double".into(),
                language: improv_core_model::Language::Pure,
                body: "return args[0] * 2".into(),
                arg_types: vec![ValueType::Number],
                return_type: ValueType::Number,
                pure: true,
            },
        );
        m.external_calls.insert(
            MeasureId(102),
            improv_core_model::ExternalCall {
                func: "double".into(),
                arg_measures: vec![MeasureId(100)],
                refresh_policy: improv_core_model::RefreshPolicy::Manual,
            },
        );
        m.add_scenario(improv_core_model::Scenario {
            id: improv_core_model::ScenarioId(1),
            name: Name("High price".into()),
            overrides: [(
                (
                    MeasureId(100),
                    Coordinate::from_pairs([(product, ItemId(20))]),
                ),
                Value::Number(99.0),
            )]
            .into_iter()
            .collect(),
        });
        m
    }

    #[test]
    fn save_then_load_round_trips() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let original = sample_model();
        store.save_model(&original).expect("save");
        let loaded = store.load_model().expect("load");

        assert_eq!(loaded.categories.len(), 2);
        assert_eq!(loaded.items.len(), 2);
        assert_eq!(loaded.measures.len(), 2);
        assert_eq!(loaded.inputs.len(), 1);

        // Derived measure formula survived the JSON-in-datom round trip.
        let rev = loaded.measure_by_name("Revenue").expect("revenue");
        assert!(rev.is_derived());
        assert_eq!(rev.categories.len(), 2);

        // Input cell value survived.
        let coord = Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
        assert_eq!(
            loaded.input(MeasureId(100), &coord),
            Some(&Value::Number(10.0))
        );

        // The saved view (layout + filter) survived the round trip.
        let v = loaded.view_by_name("Prices by product").expect("view");
        assert_eq!(v.measure, MeasureId(100));
        assert_eq!(v.axis_order, vec![CategoryId(2)]);
        assert!(v.allows(CategoryId(2), ItemId(20)));
        assert!(!v.allows(CategoryId(2), ItemId(21)));

        // The :meta/* blob (external fns/calls + scenarios) survived too --
        // this is the LAST step of `save_model` and the one most exposed by
        // the old multi-transact-call bug this fix closes (see the atomicity
        // test below).
        assert_eq!(loaded.external_fns.len(), 1);
        assert_eq!(loaded.lambdas, original.lambdas);
        assert_eq!(
            loaded.external_fns["double"],
            original.external_fns["double"]
        );
        assert_eq!(loaded.external_calls.len(), 1);
        assert_eq!(
            loaded.external_calls[&MeasureId(102)],
            original.external_calls[&MeasureId(102)]
        );
        assert_eq!(loaded.scenarios.len(), 1);
        let scenario = loaded.scenario_by_name("High price").expect("scenario");
        assert_eq!(
            scenario.overrides.get(&(MeasureId(100), coord.clone())),
            Some(&Value::Number(99.0))
        );

        // Full-model equality, modulo `measure.categories` / `category.items`
        // vector order (cardinality-many refs come back from the store in
        // query order, not necessarily insertion order): normalize those
        // before comparing so the assertion isn't flaky while still checking
        // every field of every entity.
        assert_eq!(sort_vecs(loaded), sort_vecs(original));
    }

    /// A three-category model whose measure lists its categories out of id
    /// order, `[3, 1, 2]`.
    fn unordered_categories_model() -> Model {
        let mut m = Model::new();
        for (id, name) in [(1, "Year"), (2, "Region"), (3, "Product")] {
            m.add_category(CategoryId(id), name);
        }
        m.add_measure(Measure {
            id: MeasureId(400),
            name: Name("Sales".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(3), CategoryId(1), CategoryId(2)],
            kind: MeasureKind::Input,
            description: None,
        });
        m
    }

    /// `:measure/categories` is a set; the order positional data (SQL/CSV
    /// columns, coordinates) is mapped by must survive save/load and
    /// load_partial anyway.
    #[test]
    fn measure_category_order_round_trips() {
        let want = vec![CategoryId(3), CategoryId(1), CategoryId(2)];
        let mut store = ModelStore::open("").expect("open in-memory");
        store
            .save_model(&unordered_categories_model())
            .expect("save");
        let loaded = store.load_model().expect("load");
        assert_eq!(loaded.measures[&MeasureId(400)].categories, want);
        let partial = store.load_partial(&[MeasureId(400)]).expect("partial");
        assert_eq!(partial.measures[&MeasureId(400)].categories, want);

        // A re-save with a new order replaces the old one.
        let mut m = loaded;
        m.measures
            .get_mut(&MeasureId(400))
            .unwrap()
            .categories
            .reverse();
        store.save_model(&m).expect("re-save");
        assert_eq!(
            store.load_model().expect("re-load").measures[&MeasureId(400)].categories,
            vec![CategoryId(2), CategoryId(1), CategoryId(3)]
        );
    }

    /// A store written before `:measure/category-order` existed (old schema,
    /// measure without the attribute) still opens -- installing the new
    /// schema over it is idempotent -- and loads its categories in id order.
    #[test]
    fn store_without_category_order_still_loads() {
        let dir = std::env::temp_dir().join(format!("improv-catorder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("old.db");
        let path = path.to_str().unwrap();

        let start = schema::SCHEMA_EDN
            .find(" ;; The order of :measure/categories")
            .expect("category-order block");
        let tail = ":db.cardinality/one}";
        let end = start + schema::SCHEMA_EDN[start..].find(tail).unwrap() + tail.len();
        let old_schema = format!(
            "{}{}",
            &schema::SCHEMA_EDN[..start],
            &schema::SCHEMA_EDN[end..]
        );
        assert!(!old_schema.contains("category-order"));
        {
            let mut old = Store::open(path).expect("open old");
            old.transact(&old_schema).expect("old schema");
            old.transact(
                r#"[{:category/id 1 :category/name "Year"}
                    {:category/id 2 :category/name "Region"}
                    {:category/id 3 :category/name "Product"}]"#,
            )
            .expect("old categories");
            old.transact(
                r#"[{:measure/id 400 :measure/name "Sales"
                     :measure/value-type "number" :measure/kind "input"
                     :measure/categories [(lookup-ref :category/id 3)
                                          (lookup-ref :category/id 1)
                                          (lookup-ref :category/id 2)]}]"#,
            )
            .expect("old measure");
        }

        // Opening twice installs the schema twice: both must succeed.
        drop(ModelStore::open(path).expect("open old store with new schema"));
        let mut store = ModelStore::open(path).expect("re-open");
        let loaded = store.load_model().expect("load old store");
        assert_eq!(
            loaded.measures[&MeasureId(400)].categories,
            vec![CategoryId(1), CategoryId(2), CategoryId(3)]
        );
        // Re-saving writes the order, after which it is preserved.
        let mut m = loaded;
        m.measures.get_mut(&MeasureId(400)).unwrap().categories =
            vec![CategoryId(3), CategoryId(1), CategoryId(2)];
        store.save_model(&m).expect("upgrade save");
        assert_eq!(
            store.load_model().expect("load").measures[&MeasureId(400)].categories,
            vec![CategoryId(3), CategoryId(1), CategoryId(2)]
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A view saved by a PRE-CANVAS build of Improv must still load. Those
    /// blobs are on users' disks; the only thing that ever read them is
    /// `serde_json::from_str::<View>` in `load_views`, so this drives the real
    /// store path with the real legacy bytes (transacted straight as a
    /// `:view/json` datom -- constructing it via today's `View` would test the
    /// new type against itself and prove nothing about old data).
    #[test]
    fn legacy_flat_view_blob_loads_from_the_store() {
        let mut store = ModelStore::open("").expect("open in-memory");
        // Structure first, so the loaded view's measure/categories exist.
        let mut m = sample_model();
        m.views.clear();
        store.save_model(&m).expect("save");

        // Byte-for-byte the shape `View` serialized to before canvases: no
        // `placements`, no `rect`.
        let legacy = r#"{"id":9,"name":"Legacy layout","measure":100,"axis_order":[2,1],"n_rows":1,"n_cols":1,"page_items":[],"filters":[{"category":2,"items":[20]}]}"#;
        store
            .store
            .transact(&format!(
                "[{{:view/id 9 :view/json {}}}]",
                convert::edn_str_pub(legacy)
            ))
            .expect("transact legacy view blob");

        let loaded = store.load_model().expect("load");
        let v = loaded.view_by_name("Legacy layout").expect("legacy view");
        assert_eq!(v.id, improv_core_model::ViewId(9));
        assert_eq!(v.measure, MeasureId(100));
        assert_eq!(v.axis_order, vec![CategoryId(2), CategoryId(1)]);
        assert_eq!((v.n_rows, v.n_cols), (1, 1));
        assert!(v.page_items.is_empty());
        // Behaves identically: filters still filter.
        assert!(v.allows(CategoryId(2), ItemId(20)));
        assert!(!v.allows(CategoryId(2), ItemId(21)));
        // ...as a one-matrix canvas with defaulted geometry.
        assert!(v.placements.is_empty());
        let ms = v.matrices();
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].measure, MeasureId(100));
        assert_eq!(ms[0].filters, v.filters);
        assert_eq!(ms[0].rect, improv_core_model::CanvasRect::default());

        // Re-saving the loaded model rewrites it in the NEW shape, and that
        // still loads to the same thing (migration is idempotent, not lossy).
        store.save_model(&loaded).expect("re-save");
        let again = store.load_model().expect("re-load");
        assert_eq!(again.views, loaded.views);
    }

    /// A multi-matrix canvas view survives a real store round trip, geometry
    /// included.
    ///
    /// Also confirms the change does NOT interact with `save_model`'s two datom
    /// limits: a view is `:view/id` + `:view/json` = 2 datoms no matter how many
    /// placements the JSON holds (placements make the *string* longer, never the
    /// datom count), and placements are not `:measure/categories`, so the
    /// 5460-categories-per-measure guard is untouched. The 24 placements here
    /// would blow a per-datom budget if either were false.
    #[test]
    fn multi_placement_view_round_trips_through_the_store() {
        use improv_core_model::{CanvasRect, MatrixPlacement, View, ViewId};

        let mut store = ModelStore::open("").expect("open in-memory");
        let mut m = sample_model();
        m.views.clear();

        let mut primary = MatrixPlacement::new(MeasureId(100));
        primary.axis_order = vec![CategoryId(2)];
        primary.rect = CanvasRect {
            x: 12.5,
            y: 24.0,
            w: 400.0,
            h: 180.0,
        };
        primary.filters = vec![improv_core_model::Filter {
            category: CategoryId(2),
            items: vec![ItemId(20)],
        }];

        let mut second = MatrixPlacement::new(MeasureId(102));
        second.axis_order = vec![CategoryId(1), CategoryId(2)];
        second.n_rows = 2;
        second.n_cols = 0;
        second.page_items = vec![(CategoryId(1), ItemId(10))];
        second.rect = CanvasRect {
            x: 440.0,
            y: 24.0,
            w: 320.25,
            h: 240.0,
        };

        let extras: Vec<MatrixPlacement> = std::iter::once(second.clone())
            .chain((0u8..23).map(|i| {
                let mut p = MatrixPlacement::new(MeasureId(100));
                p.rect = CanvasRect {
                    x: f32::from(i) * 10.0,
                    y: 600.0,
                    w: 100.0,
                    h: 80.0,
                };
                p
            }))
            .collect();
        let view = View::from_matrices(
            ViewId(5),
            Name("Canvas".into()),
            primary.clone(),
            extras.clone(),
        );
        m.add_view(view.clone());

        store.save_model(&m).expect("save multi-placement view");
        let loaded = store.load_model().expect("load");
        let v = loaded.view_by_name("Canvas").expect("canvas view");

        assert_eq!(v, &view, "whole view survived the store round trip");
        assert_eq!(v.matrices().len(), 1 + extras.len());
        // Geometry, to the float.
        assert_eq!(v.rect, primary.rect);
        assert_eq!(v.placements[0].rect, second.rect);
        assert_eq!(v.placements[0].rect.w, 320.25);
        // Per-matrix pivot and filters.
        assert_eq!((v.placements[0].n_rows, v.placements[0].n_cols), (2, 0));
        assert_eq!(v.placements[0].page_items, second.page_items);
        assert!(v.allows(CategoryId(2), ItemId(20)));
        assert!(!v.allows(CategoryId(2), ItemId(21)));
        assert!(
            v.placements[0].allows(CategoryId(2), ItemId(21)),
            "matrix 2 is unfiltered -- filters are per matrix"
        );
    }

    /// Prove `save_model`'s atomicity: a save that fails partway through must
    /// not leave a mixed old/new state.
    ///
    /// Before this fix, `save_model` issued one `Store::transact` call per
    /// step (categories, items, measures, cells, views, meta): six separate
    /// SQLite transactions, each committing independently (confirmed by
    /// reading `Store::transact` in `../mentat/src/store.rs`, which opens and
    /// commits its own `InProgress` per call -- see also
    /// `AGENT_DATABASE_CONNECTIVITY.md` "Crash safety"). A crash or error
    /// between two of those steps left the earlier steps durably committed
    /// and the later ones missing or stale: a real "atomic saves" gap per
    /// IMPROV.txt.
    ///
    /// After the fix, all six steps run through ONE `InProgress` (one open
    /// SQLite transaction, via `Store::begin_transaction`) and a single
    /// trailing `ip.commit()`. If any step's `?` returns early, the
    /// `InProgress` (and its inner `rusqlite::Transaction`) is dropped
    /// without being committed; `rusqlite::Transaction`'s `Drop` rolls the
    /// whole SQLite transaction back (default `DropBehavior::Rollback`), so
    /// nothing from a failed save -- not even its earlier steps -- reaches
    /// disk/the in-memory DB.
    ///
    /// To manufacture a real, publicly-reachable transact failure we set an
    /// input cell to `Value::Number(f64::NAN)`: Rust's `f64` `Display` prints
    /// `NaN` as the bare (unquoted) EDN token `NaN`, which Mentat's
    /// `:db.type/double` typechecking rejects with `BadValuePair` (verified
    /// directly against `../mentat`: `store.transact("[{:cell/value-number
    /// NaN}]")` fails). This is a real, reachable failure -- e.g. a formula
    /// producing `0.0/0.0` fed back in as an input -- not a contrived path.
    #[test]
    fn save_partway_failure_does_not_leave_a_partial_write() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let m1 = sample_model();
        store.save_model(&m1).expect("save m1");

        // M2: a modification of M1 that touches every step BEFORE the failing
        // `cells` step (a new category+item, a renamed measure) plus the
        // failing cell itself, and would ALSO touch steps after `cells` (a
        // new view, a new scenario) were the transact ever to get that far.
        // If atomicity holds, NONE of these changes appear after the failed
        // save -- not even the ones from steps that would have run first.
        let mut m2 = m1.clone();
        let extra_cat = CategoryId(3);
        m2.add_category(extra_cat, "Region");
        m2.add_item(ItemId(30), extra_cat, "EMEA");
        m2.measures.get_mut(&MeasureId(100)).unwrap().name = Name("Price (changed)".into());
        m2.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(CategoryId(2), ItemId(20))]),
            Value::Number(f64::NAN),
        );
        m2.add_view(improv_core_model::View {
            id: improv_core_model::ViewId(2),
            name: Name("Should never persist".into()),
            measure: MeasureId(100),
            axis_order: vec![],
            n_rows: 1,
            n_cols: 1,
            page_items: vec![],
            filters: vec![],
            rect: Default::default(),
            placements: vec![],
        });
        m2.add_scenario(improv_core_model::Scenario {
            id: improv_core_model::ScenarioId(2),
            name: Name("Should also never persist".into()),
            overrides: HashMap::new(),
        });

        let err = store
            .save_model(&m2)
            .expect_err("NaN cell must fail to transact");
        assert!(
            matches!(err, StoreError::Mentat(_)),
            "expected a Mentat transact error, got {err:?}"
        );

        // The store must be exactly M1 -- none of M2's changes, including the
        // ones from steps that ran before the failing `cells` step, survived.
        let after = store.load_model().expect("load after failed save");
        assert_eq!(sort_vecs(after), sort_vecs(m1));
    }

    /// Sort the order-insensitive `Vec` fields (`measure.categories`,
    /// `category.items`) so two models differing only in that ordering
    /// compare equal.
    fn temp_store_path(tag: &str) -> String {
        let p = std::env::temp_dir().join(format!("improv-lock-{tag}-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p.to_str().expect("utf-8 temp path").to_owned()
    }

    /// Two handles on one file stand in for two processes. B's save lands
    /// after A's load, so A's save must refuse rather than retract B's cell.
    #[test]
    fn stale_save_is_refused_and_keeps_the_other_writers_cell() {
        let path = temp_store_path("stale");
        ModelStore::open(&path)
            .and_then(|mut s| s.save_model(&sample_model()))
            .expect("seed");
        let mut a = ModelStore::open(&path).expect("open a");
        let mut b = ModelStore::open(&path).expect("open b");
        let model_a = a.load_model().expect("a loads");
        let mut model_b = b.load_model().expect("b loads");
        let b_coord = Coordinate::from_pairs([(CategoryId(2), ItemId(21))]);
        model_b.add_item(ItemId(21), CategoryId(2), "Widget B");
        model_b.set_input(MeasureId(100), b_coord.clone(), Value::Number(7.0));
        b.save_model(&model_b).expect("b saves");

        let err = a.save_model(&model_a).expect_err("a's save is stale");
        assert!(matches!(err, StoreError::Conflict(_)), "got {err:?}");

        let after = ModelStore::open(&path)
            .and_then(|mut s| s.load_model())
            .expect("reopen");
        assert_eq!(
            after.input(MeasureId(100), &b_coord),
            Some(&Value::Number(7.0))
        );
        let _ = std::fs::remove_file(&path);
    }

    /// A handle's own commit advances its base, so it can keep saving; and a
    /// handle that merely opened the store (a no-op schema transact) does not
    /// count as a writer.
    #[test]
    fn own_saves_and_bare_opens_do_not_conflict() {
        let path = temp_store_path("own");
        ModelStore::open(&path)
            .and_then(|mut s| s.save_model(&sample_model()))
            .expect("seed");
        let mut a = ModelStore::open(&path).expect("open a");
        let mut m = a.load_model().expect("load");
        a.save_model(&m).expect("first save");
        let _reader = ModelStore::open(&path).expect("another process opens");
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(CategoryId(2), ItemId(20))]),
            Value::Number(11.0),
        );
        a.save_model(&m).expect("second save");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn fresh_store_saves_without_a_prior_load() {
        let path = temp_store_path("fresh");
        let mut s = ModelStore::open(&path).expect("open");
        assert_eq!(s.loaded_tx(), None);
        s.save_model(&sample_model()).expect("unconditional save");
        assert_eq!(s.loaded_tx(), Some(s.tx_high_water().expect("high water")));
        let _ = std::fs::remove_file(&path);
    }

    fn sort_vecs(mut m: Model) -> Model {
        for c in m.categories.values_mut() {
            c.items.sort();
        }
        for measure in m.measures.values_mut() {
            measure.categories.sort();
        }
        m
    }

    #[test]
    fn load_partial_loads_only_the_dependency_closures_cells() {
        // Time x Product; Price (input), Quantity (input), Revenue = Price *
        // Quantity (derived) -- Revenue's closure is {Revenue, Price, Quantity}.
        // Unrelated (input) is a measure with its own cell that must NOT load.
        let mut store = ModelStore::open("").expect("open in-memory");
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(20), product, "WidgetA");

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
        m.add_measure(Measure {
            id: MeasureId(200),
            name: Name("Unrelated".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Input,
            description: None,
        });

        let price_coord = Coordinate::from_pairs([(product, ItemId(20))]);
        let qty_coord = Coordinate::from_pairs([(time, ItemId(10)), (product, ItemId(20))]);
        m.set_input(MeasureId(100), price_coord.clone(), Value::Number(10.0));
        m.set_input(MeasureId(101), qty_coord.clone(), Value::Number(5.0));
        m.set_input(MeasureId(200), Coordinate::new(), Value::Number(999.0));

        store.save_model(&m).expect("save");

        // load_partial(&[Revenue]) must load Price/Quantity's cells but NOT
        // Unrelated's, even though Unrelated's MEASURE metadata is still
        // present (categories/items/measures are always loaded in full --
        // only cell volume is filtered).
        let partial = store.load_partial(&[MeasureId(102)]).expect("load_partial");
        assert!(
            partial.measures.contains_key(&MeasureId(200)),
            "measure shape still loads"
        );
        assert_eq!(
            partial.input(MeasureId(100), &price_coord),
            Some(&Value::Number(10.0))
        );
        assert_eq!(
            partial.input(MeasureId(101), &qty_coord),
            Some(&Value::Number(5.0))
        );
        assert_eq!(
            partial.input(MeasureId(200), &Coordinate::new()),
            None,
            "Unrelated's cell must NOT be loaded -- it's outside Revenue's closure"
        );

        // The engine gets zero changes: evaluate() over the SAME partial model
        // (which is an ordinary Model) produces the same Revenue as a full load.
        let full = store.load_model().expect("load_model");
        let out_full =
            improv_engine::dataflow::evaluate(&full, &[MeasureId(102)]).expect("eval full");
        let out_partial =
            improv_engine::dataflow::evaluate(&partial, &[MeasureId(102)]).expect("eval partial");
        assert_eq!(out_full, out_partial);

        // Saving a partial model keeps the cells it never loaded, and still
        // diffs the closure's: an edit lands, a removed closure cell goes.
        let mut partial = store
            .load_partial(&[MeasureId(102)])
            .expect("reload partial");
        partial.set_input(MeasureId(100), price_coord.clone(), Value::Number(11.0));
        partial.inputs.remove(&(MeasureId(101), qty_coord.clone()));
        store.save_model(&partial).expect("save partial");
        let after = store.load_model().expect("load after");
        assert_eq!(
            after.input(MeasureId(200), &Coordinate::new()),
            Some(&Value::Number(999.0)),
            "a cell outside the closure survives a partial save"
        );
        assert_eq!(
            after.input(MeasureId(100), &price_coord),
            Some(&Value::Number(11.0))
        );
        assert_eq!(after.input(MeasureId(101), &qty_coord), None);
        // A full load lifts the scope again: dropping Unrelated now retracts it.
        let mut full = after;
        full.inputs.remove(&(MeasureId(200), Coordinate::new()));
        store.save_model(&full).expect("save full");
        let last = store.load_model().expect("load last");
        assert_eq!(last.input(MeasureId(200), &Coordinate::new()), None);
    }

    /// `load_partial`'s cell *enumeration* must be bounded by the closure, not
    /// by the size of the store: a measure outside the closure with hundreds of
    /// cells must not produce a single returned row.
    ///
    /// Checked via `ModelStore::cell_rows_returned` (test-only), which records
    /// how many rows the `load_cells` query RETURNED — before this fix the
    /// query was always the unconstrained "every (measure, coord) pair" one and
    /// non-closure measures were dropped by a `continue` in the Rust loop, so
    /// this counter would have read 402 (all cells) instead of 2, and both
    /// assertions below would fail.
    #[test]
    fn load_partial_enumeration_is_bounded_by_the_closure() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let mut m = Model::new();
        let cat = CategoryId(1);
        m.add_category(cat, "Thing");
        const BIG: u32 = 400;
        for i in 0..BIG {
            m.add_item(ItemId(1000 + i), cat, format!("item{i}"));
        }

        // Closure: Derived = Small * Small (so {Derived, Small}), 1 input cell.
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("Small".into()),
            value_type: ValueType::Number,
            categories: vec![cat],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(2),
            name: Name("Derived".into()),
            value_type: ValueType::Number,
            categories: vec![cat],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(1), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(1), DimensionSpec::default())),
            ))),
            description: None,
        });
        // Outside the closure, and much bigger than it.
        m.add_measure(Measure {
            id: MeasureId(3),
            name: Name("Huge".into()),
            value_type: ValueType::Number,
            categories: vec![cat],
            kind: MeasureKind::Input,
            description: None,
        });

        let small_coord = Coordinate::from_pairs([(cat, ItemId(1000))]);
        m.set_input(MeasureId(1), small_coord.clone(), Value::Number(3.0));
        m.set_input(MeasureId(2), small_coord.clone(), Value::Number(9.0));
        for i in 0..BIG {
            m.set_input(
                MeasureId(3),
                Coordinate::from_pairs([(cat, ItemId(1000 + i))]),
                Value::Number(f64::from(i)),
            );
        }
        store.save_model(&m).expect("save");

        let partial = store.load_partial(&[MeasureId(2)]).expect("load_partial");
        // The closure has 2 cells (Small's and Derived's own stored cell); the
        // store holds 2 + BIG. The query must have returned only the former.
        assert_eq!(
            store.cell_rows_returned,
            2,
            "load_partial must not enumerate out-of-closure rows (store holds {} cells)",
            2 + BIG
        );
        assert_eq!(partial.inputs.len(), 2);
        assert_eq!(
            partial.input(MeasureId(1), &small_coord),
            Some(&Value::Number(3.0))
        );

        // Empty closure: no cells, no query (a bare `ground []` is a Mentat
        // parse error, so this branch must short-circuit, not build a query).
        let none = store.load_partial(&[]).expect("load_partial(&[])");
        assert!(none.inputs.is_empty());
        assert_eq!(store.cell_rows_returned, 0);

        // load_model still loads EVERYTHING -- no accidental filtering.
        let full = store.load_model().expect("load_model");
        assert_eq!(store.cell_rows_returned as u32, 2 + BIG);
        assert_eq!(full.inputs.len() as u32, 2 + BIG);
        assert_eq!(
            full.input(
                MeasureId(3),
                &Coordinate::from_pairs([(cat, ItemId(1000 + BIG - 1))])
            ),
            Some(&Value::Number(f64::from(BIG - 1)))
        );
    }

    #[test]
    fn load_partial_with_no_dependencies_loads_only_its_own_cells() {
        // A root with no formula/arg_measures (an ordinary input measure) has a
        // closure of just itself.
        let mut store = ModelStore::open("").expect("open in-memory");
        let mut m = Model::new();
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("A".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(2),
            name: Name("B".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Input,
            description: None,
        });
        m.set_input(MeasureId(1), Coordinate::new(), Value::Number(1.0));
        m.set_input(MeasureId(2), Coordinate::new(), Value::Number(2.0));
        store.save_model(&m).expect("save");

        let partial = store.load_partial(&[MeasureId(1)]).expect("load_partial");
        assert_eq!(
            partial.input(MeasureId(1), &Coordinate::new()),
            Some(&Value::Number(1.0))
        );
        assert_eq!(partial.input(MeasureId(2), &Coordinate::new()), None);
    }

    const WIDE_CAT: CategoryId = CategoryId(1);

    /// A model with `n_cells` input cells over `n_cells` items of one category,
    /// so BOTH the items group and the cells group exceed a single safe
    /// transact.
    fn wide_model(n_cells: u32) -> Model {
        let mut m = Model::new();
        m.add_category(WIDE_CAT, "Thing");
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("Amount".into()),
            value_type: ValueType::Number,
            categories: vec![WIDE_CAT],
            kind: MeasureKind::Input,
            description: None,
        });
        for i in 0..n_cells {
            m.add_item(ItemId(1000 + i), WIDE_CAT, format!("item{i}"));
            m.set_input(
                MeasureId(1),
                Coordinate::from_pairs([(WIDE_CAT, ItemId(1000 + i))]),
                Value::Number(f64::from(i) * 1.5),
            );
        }
        m
    }

    fn assert_wide_round_trip(n_cells: u32) {
        let mut store = ModelStore::open("").expect("open in-memory");
        store.save_model(&wide_model(n_cells)).expect("save large");

        let loaded = store.load_model().expect("load large");
        assert_eq!(loaded.items.len() as u32, n_cells);
        assert_eq!(loaded.inputs.len() as u32, n_cells);
        // Spot-check the first, a middle, and the last cell's value.
        for i in [0, n_cells / 2, n_cells - 1] {
            assert_eq!(
                loaded.input(
                    MeasureId(1),
                    &Coordinate::from_pairs([(WIDE_CAT, ItemId(1000 + i))])
                ),
                Some(&Value::Number(f64::from(i) * 1.5)),
                "cell {i} of {n_cells}"
            );
        }
    }

    /// A model past Mentat's hard transact ceiling must save and round-trip.
    ///
    /// Mentat's `insert_non_fts_searches` `assert!`s that a single transact
    /// carries fewer than `32766 / 6` = 5461 datoms, and *aborts the process*
    /// otherwise (`Too many values: 6 * 5461 >= 32766`). Before the chunking
    /// fix `save_model` put every entity of a kind in ONE transact, so this
    /// test panicked rather than failed: 6000 cells (and 6000 items) are each
    /// well past that limit.
    #[test]
    fn save_and_load_model_past_mentats_transact_limit() {
        assert_wide_round_trip(6_000);
    }

    /// Same, but far enough past the boundary to prove *many* chunks work and
    /// not just the first one.
    #[test]
    fn save_and_load_model_spanning_many_chunks() {
        assert_wide_round_trip(12_000);
    }

    /// Chunking must not weaken atomicity: a failure in a *later* chunk of the
    /// cells group rolls back the earlier chunks of that same group, not just
    /// the earlier groups.
    #[test]
    fn a_failure_in_a_later_chunk_rolls_back_earlier_chunks() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let mut m = wide_model(6_000);
        // NaN prints as the bare EDN token `NaN`, which Mentat's
        // `:db.type/double` typecheck rejects -- same mechanism as
        // `save_partway_failure_does_not_leave_a_partial_write`. Which chunk it
        // lands in depends on HashMap iteration order, which is the point:
        // whichever chunk fails, everything before it must vanish.
        m.set_input(
            MeasureId(1),
            Coordinate::from_pairs([(WIDE_CAT, ItemId(1000))]),
            Value::Number(f64::NAN),
        );
        let err = store.save_model(&m).expect_err("NaN cell must fail");
        assert!(matches!(err, StoreError::Mentat(_)), "got {err:?}");

        // Nothing from the failed save survived: not the categories/items
        // transacted before the cells group, and not the cell chunks that
        // succeeded before the failing one.
        let after = store.load_model().expect("load after failed save");
        assert!(
            after.inputs.is_empty(),
            "{} cells leaked",
            after.inputs.len()
        );
        assert!(after.items.is_empty(), "{} items leaked", after.items.len());
        assert!(after.categories.is_empty());
    }

    /// A measure carrying `n` categories, numbered from `first_id`.
    fn measure_with_n_categories_from(n: usize, first_id: u32) -> Model {
        let mut m = Model::new();
        let mut cats = Vec::with_capacity(n);
        for i in 0..n {
            let c = CategoryId(first_id + i as u32);
            m.add_category(c, format!("cat{i}"));
            cats.push(c);
        }
        m.add_measure(Measure {
            id: MeasureId(7),
            name: Name("Very Wide".into()),
            value_type: ValueType::Number,
            categories: cats,
            kind: MeasureKind::Input,
            description: None,
        });
        m
    }

    fn measure_with_n_categories(n: usize) -> Model {
        measure_with_n_categories_from(n, 1)
    }

    /// One entity wider than the store's per-transact ceiling must come back as
    /// an error, not kill the process.
    ///
    /// Chunking (`MAX_DATOMS_PER_TRANSACT`) fixed groups of entities, but a
    /// measure is indivisible: `:measure/categories` is cardinality-many, so
    /// 5461+ categories on one measure put 5461+ datoms in a single
    /// `insert_non_fts_searches` call, which `assert!`s and *aborts*. Against
    /// the pre-guard code this exact input printed
    /// `Too many values: 6 * 5461 >= 32766` from `../mentat/db/src/db.rs:949`.
    #[test]
    fn measure_past_the_single_entity_ceiling_errors_instead_of_aborting() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let m = measure_with_n_categories(MAX_CATEGORIES_PER_MEASURE + 1);
        let err = store
            .save_model(&m)
            .expect_err("over-wide measure must fail");

        let msg = err.to_string();
        assert!(matches!(err, StoreError::Integrity(_)), "got {err:?}");
        // The diagnostic names the offending entity and the real limit.
        assert!(msg.contains("measure 7"), "{msg}");
        assert!(msg.contains("Very Wide"), "{msg}");
        assert!(msg.contains("5461"), "{msg}");
        assert!(
            msg.contains(&MAX_CATEGORIES_PER_MEASURE.to_string()),
            "{msg}"
        );

        // The refused save left nothing behind (the `InProgress` is dropped
        // before `commit`), so a later smaller save is unaffected.
        assert!(store.load_model().expect("load").measures.is_empty());
    }

    /// The last savable width still saves and round-trips — the guard must not
    /// be off by one. 5460 is measured, not assumed: 5461 aborts (see above).
    #[test]
    fn measure_at_the_single_entity_ceiling_round_trips() {
        assert_eq!(MAX_CATEGORIES_PER_MEASURE, 5460);
        let mut store = ModelStore::open("").expect("open in-memory");
        let m = measure_with_n_categories(MAX_CATEGORIES_PER_MEASURE);
        store.save_model(&m).expect("5460 categories must save");

        let loaded = store.load_model().expect("load");
        assert_eq!(
            loaded.measures[&MeasureId(7)].categories.len(),
            MAX_CATEGORIES_PER_MEASURE
        );
    }

    /// The guard stays sound despite the (separate, unfixed) missing-retraction
    /// bug: `:measure/categories` is cardinality-many and `save_model` never
    /// retracts stale refs, so re-saving a measure with a *different* category
    /// set accumulates the union on disk. Two 3000-category saves of disjoint
    /// sets each pass the guard, yet the store then holds 6000 refs — past the
    /// ceiling. Re-saving *that loaded model* must be a clean error, and a
    /// normal-width save over the accumulated entity must still succeed (proving
    /// the abort counts only the datoms of the transact being applied, so the
    /// in-memory budget the guard checks is the right thing to check).
    #[test]
    fn churned_resave_reports_error_not_abort() {
        let mut store = ModelStore::open("").expect("open in-memory");
        store
            .save_model(&measure_with_n_categories_from(3_000, 1))
            .expect("first save");
        store
            .save_model(&measure_with_n_categories_from(3_000, 10_000))
            .expect("churned re-save (both halves under the guard)");

        // Finding #8: the union accumulated, so the on-disk width now exceeds
        // what either in-memory save emitted.
        let loaded = store.load_model().expect("load");
        assert_eq!(loaded.measures[&MeasureId(7)].categories.len(), 6_000);

        // Re-saving the accumulated model is a recoverable error, not an abort.
        let err = store
            .save_model(&loaded)
            .expect_err("6000-ref re-save must fail");
        assert!(matches!(err, StoreError::Integrity(_)), "got {err:?}");

        // And a normal-width save over the 6000-ref on-disk entity still works:
        // the ceiling is per-transact, not cumulative.
        store
            .save_model(&measure_with_n_categories_from(3_000, 1))
            .expect("narrow save over a wide on-disk entity");
    }
    /// Strings containing newlines, tabs or CRs must survive save/reload.
    /// `edn_str` used to emit `\\n`/`\\t`/`\\r`, but Mentat's EDN reader drops the
    /// backslash rather than interpreting it, so `"a\\nb"` reloaded as `"anb"` --
    /// silent corruption of any description, name, or Text cell with whitespace
    /// control characters in it.
    #[test]
    fn control_characters_survive_a_round_trip() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let mut m = Model::new();
        let cat = CategoryId(1);
        m.add_category(cat, "Line\nBreak");
        m.add_item(ItemId(10), cat, "Tab\tItem");
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("Multi\nLine".into()),
            value_type: ValueType::Text,
            categories: vec![cat],
            kind: MeasureKind::Input,
            description: Some("first\nsecond\tthird\rfourth".into()),
        });
        m.set_input(
            MeasureId(1),
            Coordinate::from_pairs([(cat, ItemId(10))]),
            Value::Text("cell\nwith\ttabs".into()),
        );
        store.save_model(&m).expect("save");
        let back = store.load_model().expect("load");

        assert_eq!(back.categories[&cat].name.0, "Line\nBreak");
        assert_eq!(back.items[&ItemId(10)].name.0, "Tab\tItem");
        let got = &back.measures[&MeasureId(1)];
        assert_eq!(got.name.0, "Multi\nLine");
        assert_eq!(
            got.description.as_deref(),
            Some("first\nsecond\tthird\rfourth")
        );
        assert_eq!(
            back.input(MeasureId(1), &Coordinate::from_pairs([(cat, ItemId(10))])),
            Some(&Value::Text("cell\nwith\ttabs".into()))
        );
    }

    /// One measure per value type, two cells each: what `load_model` hands
    /// back must equal what was saved, type for type (P1: the per-type value
    /// queries must not mix attributes up or lose dates to text).
    fn mixed_type_model() -> Model {
        let mut m = Model::new();
        let cat = CategoryId(1);
        m.add_category(cat, "Thing");
        m.add_item(ItemId(10), cat, "a");
        m.add_item(ItemId(11), cat, "b");
        let dt = |s: &str| {
            Value::DateTime(
                chrono::DateTime::parse_from_rfc3339(s)
                    .unwrap()
                    .with_timezone(&chrono::Utc),
            )
        };
        let cols = [
            (ValueType::Number, Value::Number(1.5), Value::Number(-2.0)),
            (
                ValueType::Text,
                Value::Text("hi".into()),
                Value::Text(String::new()),
            ),
            (
                ValueType::Boolean,
                Value::Boolean(true),
                Value::Boolean(false),
            ),
            (
                ValueType::DateTime,
                dt("2026-01-02T03:04:05Z"),
                dt("1999-12-31T23:59:59.5Z"),
            ),
            (ValueType::Enum, Value::Enum(3), Value::Enum(0)),
        ];
        for (i, (vt, a, b)) in cols.into_iter().enumerate() {
            let mid = MeasureId(100 + i as u32);
            m.add_measure(Measure {
                id: mid,
                name: Name(format!("M{i}")),
                value_type: vt,
                categories: vec![cat],
                kind: MeasureKind::Input,
                description: None,
            });
            m.set_input(mid, Coordinate::from_pairs([(cat, ItemId(10))]), a);
            m.set_input(mid, Coordinate::from_pairs([(cat, ItemId(11))]), b);
        }
        m
    }

    #[test]
    fn load_model_returns_identical_cells_for_every_value_type() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let m = mixed_type_model();
        store.save_model(&m).expect("save");
        let loaded = store.load_model().expect("load");
        assert_eq!(loaded.inputs, m.inputs);
        let partial = store.load_partial(&[MeasureId(103)]).expect("partial");
        assert_eq!(partial.inputs.len(), 2);
        assert_eq!(
            partial.input(
                MeasureId(103),
                &Coordinate::from_pairs([(CategoryId(1), ItemId(10))])
            ),
            m.input(
                MeasureId(103),
                &Coordinate::from_pairs([(CategoryId(1), ItemId(10))])
            )
        );
    }

    /// P1 stress: a 100k-cell load must take seconds, not minutes (it was
    /// ~290 s with one query per cell). Release mode recommended:
    /// `cargo test --release -p improv_storage_mentat -- --ignored`.
    #[test]
    #[ignore = "stress: 100k cells; run with --ignored"]
    fn load_100k_cells_is_fast() {
        let mut store = ModelStore::open("").expect("open in-memory");
        store.save_model(&wide_model(100_000)).expect("save");
        let t = std::time::Instant::now();
        let loaded = store.load_model().expect("load");
        let took = t.elapsed();
        assert_eq!(loaded.inputs.len(), 100_000);
        // Target is 5 s in release; debug builds get headroom.
        let limit = if cfg!(debug_assertions) { 60 } else { 5 };
        eprintln!("100k-cell load_model: {took:?}");
        assert!(took.as_secs() < limit, "100k-cell load took {took:?}");
    }

    /// How many cell value datoms the store holds, across all three typed
    /// value attributes, and how many cell entities.
    fn cell_datom_counts(store: &ModelStore) -> (usize, usize) {
        let values = [VALUE_NUMBER, VALUE_BOOLEAN, VALUE_TEXT]
            .iter()
            .map(|a| {
                store
                    .rel(&format!("[:find ?e ?v :where [?e :cell/{a} ?v]]"))
                    .unwrap()
                    .len()
            })
            .sum();
        let entities = store
            .rel("[:find ?e :where [?e :cell/key _]]")
            .unwrap()
            .len();
        (values, entities)
    }

    /// P2: re-saving an unchanged model adds no cell datoms, a changed value
    /// replaces the old one, and a cleared cell is retracted instead of
    /// lingering on disk (and coming back on the next load).
    #[test]
    fn resave_keeps_cell_datoms_flat_and_retracts_stale_ones() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let mut m = mixed_type_model();
        let n = m.inputs.len();
        for _ in 0..3 {
            store.save_model(&m).expect("save");
            assert_eq!(cell_datom_counts(&store), (n, n));
        }

        // A changed value replaces the old one (cardinality-one).
        let a = Coordinate::from_pairs([(CategoryId(1), ItemId(10))]);
        m.set_input(MeasureId(100), a.clone(), Value::Number(42.0));
        store.save_model(&m).expect("save changed");
        assert_eq!(cell_datom_counts(&store), (n, n));
        assert_eq!(store.load_model().unwrap().inputs, m.inputs);

        // A cleared cell is gone from the store, not just from the model.
        m.inputs.remove(&(MeasureId(101), a.clone()));
        store.save_model(&m).expect("save cleared");
        assert_eq!(cell_datom_counts(&store), (n - 1, n - 1));
        assert_eq!(store.load_model().unwrap().inputs, m.inputs);

        // A cell whose measure changed type keeps one value attribute, the new
        // one: the old typed value is retracted.
        let mut loaded = store.load_model().unwrap();
        loaded.measures.get_mut(&MeasureId(100)).unwrap().value_type = ValueType::Text;
        loaded.set_input(MeasureId(100), a.clone(), Value::Text("now text".into()));
        let b = Coordinate::from_pairs([(CategoryId(1), ItemId(11))]);
        loaded.set_input(MeasureId(100), b, Value::Text("also".into()));
        store.save_model(&loaded).expect("save retyped");
        assert_eq!(cell_datom_counts(&store), (n - 1, n - 1));
        assert_eq!(store.load_model().unwrap().inputs, loaded.inputs);
    }

    /// Paths the main fixture skips: a SQL-sourced measure, a CALL blob with
    /// no function defs, a cell re-saved under another value type (the old
    /// attribute is retracted; a value whose type differs from the measure's
    /// still loads), `reload_model`, and an empty `load_partial` closure.
    #[test]
    fn sql_source_call_blob_and_retyped_cell_round_trip() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let product = CategoryId(2);
        let mut m = Model::new();
        m.add_category(product, "Product");
        m.add_item(ItemId(20), product, "Widget A");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.sql_sources.insert(
            MeasureId(100),
            improv_core_model::SqlSource {
                query: "SELECT p, v FROM t".into(),
                dimension_columns: vec!["p".into()],
                value_column: "v".into(),
                refresh_policy: Default::default(),
            },
        );
        m.external_calls.insert(
            MeasureId(100),
            improv_core_model::ExternalCall {
                func: "f".into(),
                arg_measures: vec![],
                refresh_policy: Default::default(),
            },
        );
        let coord = Coordinate::from_pairs([(product, ItemId(20))]);
        m.set_input(MeasureId(100), coord.clone(), Value::Number(1.0));
        store.save_model(&m).expect("save");
        m.set_input(MeasureId(100), coord.clone(), Value::Text("n/a".into()));
        store.save_model(&m).expect("re-save");

        let loaded = store.reload_model().expect("reload");
        assert_eq!(loaded.sql_sources, m.sql_sources);
        assert_eq!(loaded.external_calls, m.external_calls);
        assert_eq!(loaded.inputs, m.inputs);

        let partial = store.load_partial(&[]).expect("partial");
        assert!(partial.inputs.is_empty());
        assert_eq!(partial.measures.len(), 1);
    }

    /// A meta blob carrying only scenarios survives a round trip. A non-finite
    /// number has no EDN double spelling, so its save is a recoverable error
    /// (not an abort) that leaves the store untouched.
    #[test]
    fn scenarios_only_meta_round_trips_and_infinite_cell_is_refused() {
        let mut store = ModelStore::open("").expect("open in-memory");
        let mut m = Model::new();
        m.add_category(CategoryId(1), "Time");
        m.add_item(ItemId(10), CategoryId(1), "2025");
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("X".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(1)],
            kind: MeasureKind::Input,
            description: None,
        });
        let coord = Coordinate::from_pairs([(CategoryId(1), ItemId(10))]);
        m.set_input(MeasureId(1), coord.clone(), Value::Number(2.5));
        m.add_scenario(improv_core_model::Scenario {
            id: improv_core_model::ScenarioId(1),
            name: Name("Up".into()),
            overrides: [((MeasureId(1), coord.clone()), Value::Number(2.0))]
                .into_iter()
                .collect(),
        });
        store.save_model(&m).expect("save");
        let loaded = store.load_model().expect("load");
        assert_eq!(loaded.scenarios, m.scenarios);
        assert_eq!(loaded.inputs, m.inputs);

        let mut inf = m.clone();
        inf.set_input(MeasureId(1), coord, Value::Number(f64::INFINITY));
        let err = store.save_model(&inf).expect_err("inf has no EDN form");
        assert!(matches!(err, StoreError::Mentat(_)), "got {err:?}");
        assert_eq!(store.load_model().expect("reload").inputs, m.inputs);
    }
}
