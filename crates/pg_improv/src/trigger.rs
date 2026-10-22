//! L1: trigger-mode sync (`improv.set_sync_mode`, `improv.on_change`), the
//! measure-subset `improv.recalc(model, measures)`, and `improv.what_if`.
//!
//! In trigger mode every bound relation carries, per model, one
//! `AFTER … FOR EACH STATEMENT` trigger per event (Postgres allows transition
//! tables only on single-event triggers). The statement's transition tables
//! name the touched coordinates; [`pm::plan`] picks the derived measures to
//! recompute (the changed inputs' dependents) and whether the recompute may
//! be restricted to those coordinates. The recompute and its writes run in
//! the writer's transaction: results commit or roll back with the change.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use improv_core_model::{CategoryId, MeasureId, Model, ValueType};
use improv_engine::{CellValue, CoordKey};
use improv_pg_model as pm;
use pgrx::prelude::*;
use pgrx::JsonB;

use super::{
    bump_version, ensure_table, get_value, id32, lock_model, mint_items, read_catalog,
    read_results, req, write_results, Res, MAX_CELLS,
};

/// Per event: trigger-name suffix, event, transition tables.
const EVENTS: [(&str, &str, &str); 4] = [
    ("ins", "INSERT", "REFERENCING NEW TABLE AS improv_new"),
    (
        "upd",
        "UPDATE",
        "REFERENCING OLD TABLE AS improv_old NEW TABLE AS improv_new",
    ),
    ("del", "DELETE", "REFERENCING OLD TABLE AS improv_old"),
    ("trn", "TRUNCATE", ""),
];

/// A model with every input cell loaded.
pub(crate) struct Loaded {
    name: String,
    mid: i64,
    pub(crate) model: Model,
}

/// Load model `mid` (named `name`) with its bound and unbound input cells.
/// Items for new dimension values are minted through `improv.mint_items`
/// when `persist`, else kept in memory.
// ponytail: duplicates L0 `recalc`'s loading; fold `recalc(text)` onto
// `load` + `recompute` once L1 and L2 have merged.
pub(crate) fn load(mid: i64, name: String, persist: bool) -> Res<Loaded> {
    pm::validate_name("model", &name)?;
    load_scoped(
        name,
        pm::build_model(&read_catalog(mid)?)?,
        mid,
        persist,
        None,
    )
}

/// The touched rows of a trigger-mode statement: their categories and
/// dimension values (in that order).
type Touched<'a> = (&'a [CategoryId], &'a [Vec<Option<String>>]);

/// [`load`] onto `model` (the catalog's), reading each binding only at the
/// `touched` rows' coordinates when given ([`pm::scope_filter`]): an
/// aggregated binding then loads only those coordinates' groups. Valid only
/// when the recompute is restricted to them too ([`pm::scopable`]). Reads the bound
/// tables as the model's owner.
fn load_scoped(
    name: String,
    mut model: Model,
    mid: i64,
    persist: bool,
    touched: Option<Touched<'_>>,
) -> Res<Loaded> {
    let bindings: Vec<BindingRow> = Spi::connect(|c| -> Res<_> {
        c.select(
            // Qualified: the owner's reads run under a pinned search_path.
            "SELECT b.measure_id, \
                    (SELECT format('%I.%I', n.nspname, c.relname) FROM pg_class c \
                     JOIN pg_namespace n ON n.oid = c.relnamespace WHERE c.oid = b.rel), \
                    b.value_col, b.agg, \
                    ARRAY(SELECT b.dim_cols ->> c.name \
                          FROM unnest(m.categories) WITH ORDINALITY u(cid, ord) \
                          JOIN improv.category c ON c.model_id = b.model_id AND c.id = u.cid \
                          ORDER BY u.ord), \
                    ARRAY(SELECT (SELECT t.typname::text FROM pg_attribute a \
                                  JOIN pg_type t ON t.oid = a.atttypid \
                                  WHERE a.attrelid = b.rel AND a.attname = b.dim_cols ->> c.name \
                                    AND NOT a.attisdropped) \
                          FROM unnest(m.categories) WITH ORDINALITY u(cid, ord) \
                          JOIN improv.category c ON c.model_id = b.model_id AND c.id = u.cid \
                          ORDER BY u.ord), \
                    (SELECT c.relkind = 'r' AND NOT c.relhassubclass FROM pg_class c \
                     WHERE c.oid = b.rel) \
             FROM improv.binding b \
             JOIN improv.measure m ON m.model_id = b.model_id AND m.id = b.measure_id \
             WHERE b.model_id = $1 ORDER BY b.measure_id",
            None,
            &[mid.into()],
        )?
        .map(|r| {
            Ok((
                req(&r, 1)?,
                req(&r, 2)?,
                req(&r, 3)?,
                r.get(4)?,
                req(&r, 5)?,
                req(&r, 6)?,
                r.get(7)?.unwrap_or(false),
            ))
        })
        .collect::<Res<_>>()
    })?;
    let owner = super::owner::owner_of(mid)?;
    let bound = super::owner::as_role(owner, || read_bound(&model, bindings, touched))?;
    if persist {
        mint_items(mid, &mut model, &bound)?;
    }
    let loaded = pm::load_inputs(&mut model, &bound)?;
    let cells: Vec<(u32, String, String)> = Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT measure_id, coord::text, value::text FROM improv.cell c \
             WHERE model_id = $1 AND NOT EXISTS (SELECT 1 FROM improv.binding b \
               WHERE b.model_id = c.model_id AND b.measure_id = c.measure_id)",
            None,
            &[mid.into()],
        )?
        .map(|r| Ok((id32(req(&r, 1)?)?, req(&r, 2)?, req(&r, 3)?)))
        .collect()
    })?;
    let unbound = pm::load_cells(&mut model, &cells)?;
    pm::check_cell_budget(loaded.cells + unbound, MAX_CELLS.get().max(0) as usize)?;
    Ok(Loaded { name, mid, model })
}

/// One binding: (measure id, relation, value column, agg, dimension
/// columns, their types, whether the relation is a plain table without
/// inheritance children).
type BindingRow = (
    i64,
    String,
    String,
    Option<String>,
    Vec<String>,
    Vec<Option<String>>,
    bool,
);

/// A scope filter on one relation: the relation, its filtered columns and
/// their types, and the touched values (row-aligned, one list per column).
type Filter = (String, Vec<String>, Vec<Option<String>>, Vec<Vec<String>>);

/// Read each binding's rows (at the `touched` coordinates when given).
/// Bindings on one plain table under one filter find its touched rows in a
/// single scan ([`pm::scoped_tids_sql`]) and each reads them by `ctid`.
fn read_bound(
    model: &Model,
    bindings: Vec<BindingRow>,
    touched: Option<Touched<'_>>,
) -> Res<Vec<pm::BoundRows>> {
    let mut reads = Vec::with_capacity(bindings.len());
    let mut sharing: HashMap<Filter, usize> = HashMap::new();
    for (measure_id, rel, value_col, agg, dims, types, plain) in bindings {
        let measure = MeasureId(id32(measure_id)?);
        let m = model
            .measures
            .get(&measure)
            .ok_or("binding for an unknown measure")?;
        let filter = touched
            .and_then(|(cats, rows)| pm::scope_filter(cats, rows, &m.categories, &dims))
            .map(|(cols, values)| {
                let types = cols
                    .iter()
                    .map(|c| {
                        dims.iter()
                            .position(|d| d == c)
                            .and_then(|i| types.get(i).cloned().flatten())
                    })
                    .collect();
                (rel.clone(), cols, types, values)
            });
        if let (Some(f), true) = (&filter, plain) {
            *sharing.entry(f.clone()).or_default() += 1;
        }
        reads.push((measure, rel, value_col, agg, dims, m.value_type, filter));
    }
    let mut tids: HashMap<Filter, Option<String>> = HashMap::new();
    let mut bound = Vec::with_capacity(reads.len());
    for (measure, rel, value_col, agg, dims, vt, filter) in reads {
        check_for_interrupts!();
        // A text[] per filtered column, or the touched rows' ctids as text.
        let (rel, args): (String, Vec<pgrx::datum::DatumWithOid>) = match filter {
            None => (rel, Vec::new()),
            Some(f) => {
                let types: Vec<Option<&str>> = f.2.iter().map(Option::as_deref).collect();
                let tid = match tids.get(&f) {
                    Some(t) => t.clone(),
                    None if sharing.get(&f).is_some_and(|n| *n > 1) => {
                        let sql = pm::scoped_tids_sql(&f.0, &f.1, &types);
                        let args: Vec<_> = f.3.iter().cloned().map(Into::into).collect();
                        // Read-only (`select`, not `get_one`'s `update`), so
                        // the scan may run in parallel.
                        let t = Spi::connect(|c| c.select(&sql, None, &args)?.first().get_one())?;
                        tids.insert(f.clone(), t.clone());
                        t
                    }
                    None => None,
                };
                match tid {
                    Some(t) => (pm::tid_rel_sql(&f.0), vec![t.into()]),
                    None => (
                        pm::scoped_rel_sql(&f.0, &f.1, &types),
                        f.3.into_iter().map(Into::into).collect(),
                    ),
                }
            }
        };
        let sql = pm::select_bound_sql(&rel, &dims, &value_col, agg.as_deref(), vt)?;
        let rows = Spi::connect(|c| {
            c.select(&sql, None, &args)?
                .map(|r| {
                    let d = (1..=dims.len())
                        .map(|i| r.get::<String>(i))
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok((d, get_value(&r, dims.len() + 1, vt)?))
                })
                .collect::<Res<Vec<_>>>()
        })?;
        bound.push(pm::BoundRows {
            measure_id: measure.0,
            rows,
        });
    }
    Ok(bound)
}

/// Every derived measure, sorted.
pub(crate) fn all_derived(m: &Model) -> Vec<MeasureId> {
    let mut v: Vec<MeasureId> = m
        .measures
        .values()
        .filter(|m| !m.is_input())
        .map(|m| m.id)
        .collect();
    v.sort();
    v
}

/// Engine output: cells per derived measure.
type Cells = HashMap<MeasureId, HashMap<CoordKey, CellValue>>;

/// `dataflow::evaluate` on the backend thread, an engine panic as an error.
fn evaluate(m: &Model, targets: &[MeasureId]) -> Res<Cells> {
    check_for_interrupts!();
    match catch_unwind(AssertUnwindSafe(|| {
        improv_engine::dataflow::evaluate(m, targets)
    })) {
        Ok(r) => Ok(r?),
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(format!("improv engine panicked: {msg}").into())
        }
    }
}

/// Whether relation `table` exists.
pub(crate) fn exists(table: &str) -> Res<bool> {
    Ok(
        Spi::get_one_with_args::<bool>("SELECT to_regclass($1) IS NOT NULL", &[table.into()])?
            == Some(true),
    )
}

/// Recompute `plan.targets` and write their changed result rows (only rows
/// in `plan.scope`, when given), as the model's owner. Returns the rows
/// changed.
pub(crate) fn recompute(l: &mut Loaded, plan: &pm::Plan) -> Res<i64> {
    let owner = super::owner::owner_of(l.mid)?;
    super::owner::as_role(owner, || write_plan(l, plan))
}

fn write_plan(l: &mut Loaded, plan: &pm::Plan) -> Res<i64> {
    if plan.targets.is_empty() {
        return Ok(0);
    }
    if let Some(s) = &plan.scope {
        s.restrict_inputs(&mut l.model);
    }
    let out = evaluate(&l.model, &plan.targets)?;
    let empty = Default::default();
    let mut changed = 0i64;
    for &target in &plan.targets {
        check_for_interrupts!();
        let mut new = pm::result_rows(&l.model, target, out.get(&target).unwrap_or(&empty))?;
        let dims = pm::dim_columns(&l.model, target)?;
        let name = &l.model.measures[&target].name.0;
        pm::validate_name("measure", name)?;
        let table = pm::result_table(&l.name, name);
        let vt = l.model.measures[&target].value_type;
        ensure_table(&l.name, &table, &dims, vt)?;
        let filter = match &plan.scope {
            Some(s) => s.result_filter(&l.model, target)?,
            None => None,
        };
        let mut old = match filter {
            Some((positions, values)) => {
                let sql = pm::scoped_result_sql(&table, &dims, vt, &positions)?;
                read_rows(&sql, dims.len(), vt, values)?
            }
            None => read_results(&table, &dims, vt)?,
        };
        if let Some(s) = &plan.scope {
            s.retain_rows(&l.model, target, &mut old)?;
            s.retain_rows(&l.model, target, &mut new)?;
        }
        changed += write_results(&table, &dims, vt, &old, &new)?;
    }
    Ok(changed)
}

/// The result rows `sql` (a [`pm::scoped_result_sql`]) reads with one
/// `text[]` per filtered column in `values`.
fn read_rows(sql: &str, n: usize, vt: ValueType, values: Vec<Vec<String>>) -> Res<pm::ResultRows> {
    let args: Vec<pgrx::datum::DatumWithOid> = values.into_iter().map(Into::into).collect();
    Spi::connect(|c| {
        let mut rows = pm::ResultRows::new();
        for r in c.select(sql, None, &args)? {
            let key = (1..=n)
                .map(|i| req::<String>(&r, i))
                .collect::<Result<Vec<_>, _>>()?;
            if let Some(v) = get_value(&r, n + 1, vt)? {
                rows.insert(key, v);
            }
        }
        Ok(rows)
    })
}

/// The function behind trigger mode's sync triggers and commit mode's
/// capture triggers (`improv_<model>_sync_<op>` and `_cap_<op>`).
const TRIGGER_FNS: [(&str, &str); 2] = [
    ("sync", "improv.on_change"),
    ("cap", "improv.commit_capture"),
];

/// Model `model`'s `kind` (`sync` or `cap`) trigger for event `op`:
/// `improv_<model>_<kind>_<op>`, as long as [`pm::sync_trigger_name`]'s.
fn trigger_name(model: &str, kind: &str, op: &str) -> Res<String> {
    let name = pm::sync_trigger_name(model, op)?;
    Ok(name.replacen("_sync_", &format!("_{kind}_"), 1))
}

/// Drop model `name`'s sync and capture triggers from every relation. As
/// the extension: the TRIGGER privilege that let a model's owner create
/// them does not let it drop them (only the table's owner may), and these
/// are the model's own (named for it, calling `improv.on_change` or
/// `improv.commit_capture`).
pub(crate) fn remove_triggers(name: &str) -> Res<()> {
    let mut names = Vec::new();
    for (kind, _) in TRIGGER_FNS {
        for (op, _, _) in EVENTS {
            names.push(trigger_name(name, kind, op)?);
        }
    }
    super::owner::as_catalog(|| {
        let found: Vec<(String, String)> = Spi::connect(|c| -> Res<_> {
            c.select(
                "SELECT format('%I.%I', n.nspname, c.relname), t.tgname::text \
                 FROM pg_trigger t JOIN pg_class c ON c.oid = t.tgrelid \
                 JOIN pg_namespace n ON n.oid = c.relnamespace \
                 WHERE t.tgname = ANY($1) \
                   AND t.tgfoid IN ('improv.on_change'::regproc, 'improv.commit_capture'::regproc)",
                None,
                &[names.into()],
            )?
            .map(|r| Ok((req(&r, 1)?, req(&r, 2)?)))
            .collect()
        })?;
        for (rel, tg) in found {
            Spi::run(&format!("DROP TRIGGER {} ON {rel}", pm::quote_ident(&tg)))?;
        }
        Ok(())
    })
}

/// Set how `model`'s results follow its bound data: `trigger` installs
/// statement-level sync triggers on every bound relation (re-run after
/// binding a new relation); `commit` recomputes once per writing
/// transaction, before it commits (needs the library preloaded). Both
/// recompute once so the results start consistent. `logical` and `manual`
/// remove the triggers.
#[pg_extern(security_invoker)]
fn set_sync_mode(model: &str, mode: &str) -> Res<()> {
    if !matches!(mode, "logical" | "trigger" | "commit" | "manual") {
        return Err(
            format!("sync mode must be logical, trigger, commit or manual, not {mode:?}").into(),
        );
    }
    if mode == "commit" {
        super::commit::check_preloaded()?;
    }
    if mode == "logical" {
        super::worker::check_served(model)?;
    }
    let (mid, name) = lock_model(model)?;
    pm::validate_name("model", &name)?;
    let bound: Vec<(String, pg_sys::Oid)> = Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT DISTINCT rel::text, rel::oid FROM improv.binding WHERE model_id = $1",
            None,
            &[mid.into()],
        )?
        .map(|r| Ok((req(&r, 1)?, req(&r, 2)?)))
        .collect()
    })?;
    for (rel, oid) in &bound {
        crate::check_syncable(rel, crate::relkind(*oid)?, mode)?;
        if mode == "commit" {
            super::commit::guard(*oid, rel)?;
        }
    }
    remove_triggers(&name)?;
    Spi::run_with_args(
        "UPDATE improv.model SET sync_mode = $2 WHERE id = $1",
        &[mid.into(), mode.into()],
    )?;
    if mode == "logical" {
        super::worker::enable(mid, &name)?;
    } else {
        super::worker::disable(mid, &name)?;
    }
    let kind = match mode {
        "trigger" => Some(TRIGGER_FNS[0]),
        "commit" if super::commit::CAPTURE.get() == super::commit::Capture::Rows => {
            Some(TRIGGER_FNS[1])
        }
        _ => None,
    };
    if let Some((kind, func)) = kind {
        let rels: Vec<String> = Spi::connect(|c| -> Res<_> {
            c.select(
                "SELECT DISTINCT rel::text FROM improv.binding WHERE model_id = $1 ORDER BY 1",
                None,
                &[mid.into()],
            )?
            .map(|r| req(&r, 1))
            .collect()
        })?;
        for rel in rels {
            for (op, event, refs) in EVENTS {
                let tg = trigger_name(&name, kind, op)?;
                Spi::run(&format!(
                    "CREATE TRIGGER {} AFTER {event} ON {rel} {refs} \
                     FOR EACH STATEMENT EXECUTE FUNCTION {func}({})",
                    pm::quote_ident(&tg),
                    pm::quote_literal(&name)
                ))?;
            }
        }
    }
    if matches!(mode, "trigger" | "commit") {
        recompute_all(mid, &name)?;
    }
    bump_version(mid)
}

/// The sync trigger: recompute the changed inputs' dependents in the
/// writer's transaction, as the model's owner (the catalog as the extension:
/// the writer needs no privilege on `improv`). Its one argument is the model
/// name.
#[pg_trigger]
fn on_change<'a>(
    t: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, PgHeapTupleError> {
    if let Err(e) = super::owner::as_catalog(|| sync(t)) {
        error!("improv.on_change: {e}");
    }
    Ok(None)
}

fn sync(t: &PgTrigger<'_>) -> Res<()> {
    let model = t
        .extra_args()?
        .into_iter()
        .next()
        .ok_or("the trigger has no model argument")?;
    let rel = t.relid()?;
    let (mid, name) = lock_model(&model)?;
    let mode = Spi::get_one_with_args::<String>(
        "SELECT sync_mode FROM improv.model WHERE id = $1",
        &[mid.into()],
    )?;
    if mode.as_deref() != Some("trigger") {
        return Ok(());
    }
    let cols = touched_cols(mid, rel)?;
    let rows = touched_rows(t, &cols.1)?;
    sync_rows(mid, name, rel, cols.0, rows)
}

/// The dimension categories of relation `rel` in model `mid` that a bound
/// measure has, and their columns (in category order).
pub(crate) fn touched_cols(mid: i64, rel: pg_sys::Oid) -> Res<(Vec<i64>, Vec<String>)> {
    Ok(Spi::connect(|c| -> Res<Vec<(i64, String)>> {
        c.select(
            // Only categories a bound measure has: a column bound after its
            // measures has values no load ever minted.
            "SELECT cb.category_id, cb.col FROM improv.category_binding cb \
             WHERE cb.model_id = $1 AND cb.rel = $2::oid::regclass \
               AND cb.category_id IN (SELECT unnest(m.categories) FROM improv.binding b \
                 JOIN improv.measure m ON m.model_id = b.model_id AND m.id = b.measure_id \
                 WHERE b.model_id = $1 AND b.rel = $2::oid::regclass) \
             ORDER BY cb.category_id",
            None,
            &[mid.into(), rel.into()],
        )?
        .map(|r| Ok((req(&r, 1)?, req(&r, 2)?)))
        .collect()
    })?
    .into_iter()
    .unzip())
}

/// Recompute model `mid` (named `name`) after its relation `rel` changed
/// at the coordinates of `rows` (dimension values of categories `cats`, in
/// order; `None`: everywhere, as after a TRUNCATE), in the caller's
/// transaction: a statement's sync trigger, or commit mode's pre-commit
/// recompute with captured rows. Returns the result rows changed.
pub(crate) fn sync_rows(
    mid: i64,
    name: String,
    rel: pg_sys::Oid,
    cats: Vec<i64>,
    rows: Option<Vec<Vec<Option<String>>>>,
) -> Res<()> {
    if rows.as_ref().is_some_and(Vec::is_empty) {
        return Ok(());
    }
    let changed: Vec<i64> = Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT measure_id FROM improv.binding \
             WHERE model_id = $1 AND rel = $2::oid::regclass",
            None,
            &[mid.into(), rel.into()],
        )?
        .map(|r| req(&r, 1))
        .collect()
    })?;
    if changed.is_empty() {
        return Ok(()); // the relation's measures were all dropped
    }
    let changed = changed
        .into_iter()
        .map(|i| id32(i).map(MeasureId))
        .collect::<Res<Vec<_>>>()?;
    let cats = cats
        .into_iter()
        .map(|c| id32(c).map(CategoryId))
        .collect::<Res<Vec<_>>>()?;
    pm::validate_name("model", &name)?;
    let model = pm::build_model(&read_catalog(mid)?)?;
    // An element-wise recompute reads only the touched coordinates: load
    // only their rows (an aggregated binding, only their groups).
    let touched = rows
        .as_deref()
        .filter(|_| pm::scopable(&model, &changed))
        .map(|r| (cats.as_slice(), r));
    let mut l = load_scoped(name, model, mid, true, touched)?;
    let plan = match rows {
        Some(rows) => {
            let scope = pm::Scope::from_rows(&l.model, &cats, &rows);
            pm::plan(&l.model, &changed, scope)
        }
        // TRUNCATE: no transition tables.
        None => pm::Plan {
            targets: pm::dependents(&l.model, &changed),
            scope: None,
        },
    };
    recompute(&mut l, &plan)?;
    Ok(())
}

/// The dimension values (in `cols` order) in the firing statement's
/// transition tables; `None` when it has none (TRUNCATE).
pub(crate) fn touched_rows(
    t: &PgTrigger<'_>,
    cols: &[String],
) -> Res<Option<Vec<Vec<Option<String>>>>> {
    let Some(sql) = pm::touched_sql(
        t.old_transition_table_name()?,
        t.new_transition_table_name()?,
        cols,
    ) else {
        return Ok(None);
    };
    Spi::connect(|c| {
        // SAFETY: the firing trigger's data, live for this call; registering
        // it exposes its transition tables to this SPI connection's queries.
        let rc = unsafe {
            pg_sys::SPI_register_trigger_data(std::ptr::from_ref(t.trigger_data()).cast_mut())
        };
        if rc != pg_sys::SPI_OK_TD_REGISTER as i32 {
            return Err(format!("SPI_register_trigger_data failed: {rc}").into());
        }
        let rows = c
            .select(&sql, None, &[])?
            .map(|r| {
                (1..=cols.len())
                    .map(|i| r.get::<String>(i))
                    .collect::<Result<Vec<_>, _>>()
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Some(rows))
    })
}

/// Recompute the named measures and every measure that depends on them,
/// now, in the caller's transaction (all derived measures when `measures`
/// is NULL). Returns the number of result rows changed.
#[pg_extern(security_invoker, name = "recalc")]
fn recalc_measures(model: &str, measures: Option<Vec<String>>) -> Res<i64> {
    let (mid, name) = lock_model(model)?;
    let mut l = load(mid, name, true)?;
    let targets = match measures {
        Some(ms) => pm::dependents(&l.model, &pm::measure_ids(&l.model, &ms)?),
        None => all_derived(&l.model),
    };
    recompute(
        &mut l,
        &pm::Plan {
            targets,
            scope: None,
        },
    )
}

/// Recompute every derived measure of model `mid` (named `name`) in the
/// caller's transaction: the shared path behind `recalc(model, NULL)`, the
/// logical worker, and entering trigger mode. Returns the rows changed.
pub(crate) fn recompute_all(mid: i64, name: &str) -> Res<i64> {
    let mut l = load(mid, name.to_string(), true)?;
    let targets = all_derived(&l.model);
    recompute(
        &mut l,
        &pm::Plan {
            targets,
            scope: None,
        },
    )
}

/// Evaluate `model` with `edits` applied to its input cells in memory and
/// return the derived cells that differ from the stored results (`value`
/// NULL: the cell would vanish; else JSON of the measure's type, a datetime
/// as an RFC 3339 string). Writes nothing. Only a role that may act as the
/// model's owner may call it; it reads the bound and result tables as the
/// owner, as `recalc` does.
#[pg_extern(security_invoker)]
fn what_if(
    model: &str,
    edits: JsonB,
) -> TableIterator<
    'static,
    (
        name!(measure, String),
        name!(coord, JsonB),
        name!(value, Option<JsonB>),
    ),
> {
    // pgrx 0.17 cannot generate SQL for a `Result<TableIterator>`.
    match what_if_rows(model, &edits) {
        Ok(rows) => TableIterator::new(rows),
        Err(e) => error!("{e}"),
    }
}

/// One `what_if` result row.
type WhatIfRow = (String, JsonB, Option<JsonB>);

fn what_if_rows(model: &str, edits: &JsonB) -> Res<Vec<WhatIfRow>> {
    let (mid, name) = Spi::connect(|c| -> Res<(i64, String)> {
        let r = c
            .select(
                "SELECT id, name FROM improv.model WHERE name = $1",
                None,
                &[model.into()],
            )?
            .next()
            .ok_or_else(|| format!("improv model {model:?} does not exist"))?;
        Ok((req(&r, 1)?, req(&r, 2)?))
    })?;
    pm::validate_name("model", &name)?;
    // Its items, cells and tables are the owner's data: without this check a
    // caller would see none of its items (row-level security) and get
    // nothing, or a misleading delta, back.
    let owner = super::owner::owner_of(mid)?;
    if let Some(o) = owner {
        let (role, may) = Spi::get_two_with_args::<String, bool>(
            "SELECT r::text, improv.may_act_as(r) FROM (SELECT $1::regrole AS r) s",
            &[o.into()],
        )?;
        if may != Some(true) {
            return Err(format!(
                "permission denied for improv model {name:?}: what_if needs to act as its \
                 owner, {} (see Privileges)",
                role.unwrap_or_default()
            )
            .into());
        }
    }
    let text = edits.0.to_string();
    let model = pm::build_model(&read_catalog(mid)?)?;
    // Edits of one category set, under an element-wise chain, reach only
    // their own coordinates: load and compare only those, as trigger mode
    // does. (Edits naming an item not minted yet take the full load.)
    let scoped = pm::parse_edits(&model, &text).ok().and_then(|e| {
        let changed: std::collections::BTreeSet<MeasureId> = e.iter().map(|x| x.0).collect();
        let changed: Vec<MeasureId> = changed.into_iter().collect();
        let rows = pm::edit_rows(&model, &e).filter(|_| pm::scopable(&model, &changed))?;
        Some((e, rows))
    });
    let (l, plan) = match scoped {
        Some((e, (cats, rows))) => {
            let mut l = load_scoped(name, model, mid, false, Some((&cats, &rows)))?;
            let changed = pm::set_edits(&mut l.model, &e);
            let scope = pm::Scope::from_rows(&l.model, &cats, &rows);
            let plan = pm::Plan {
                targets: pm::dependents(&l.model, &changed),
                scope: Some(scope),
            };
            (l, plan)
        }
        None => {
            let mut l = load_scoped(name, model, mid, false, None)?;
            let changed = pm::apply_edits(&mut l.model, &text)?;
            let targets = pm::dependents(&l.model, &changed);
            (
                l,
                pm::Plan {
                    targets,
                    scope: None,
                },
            )
        }
    };
    let mut l = l;
    if let Some(s) = &plan.scope {
        s.restrict_inputs(&mut l.model);
    }
    let out = evaluate(&l.model, &plan.targets)?;
    let empty = Default::default();
    let mut rows = Vec::new();
    for &target in &plan.targets {
        let mut new = pm::result_rows(&l.model, target, out.get(&target).unwrap_or(&empty))?;
        let dims = pm::dim_columns(&l.model, target)?;
        let mname = l.model.measures[&target].name.0.clone();
        pm::validate_name("measure", &mname)?;
        let table = pm::result_table(&l.name, &mname);
        let vt = l.model.measures[&target].value_type;
        let mut old = super::owner::as_role(owner, || {
            Ok(if exists(&table)? {
                read_results(&table, &dims, vt)?
            } else {
                Default::default()
            })
        })?;
        if let Some(s) = &plan.scope {
            s.retain_rows(&l.model, target, &mut old)?;
            s.retain_rows(&l.model, target, &mut new)?;
        }
        for (key, v) in pm::cell_changes(&old, &new) {
            rows.push((
                mname.clone(),
                JsonB(pm::coord_json(&l.model, target, &key)?),
                v.map(|v| JsonB(pm::value_json(&v))),
            ));
        }
    }
    Ok(rows)
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    /// L0's plan (Q1..Q4 × Widget/Gadget; Widget price 10, qty 100·q; Gadget
    /// price 20, qty 30·q) plus `costs` (Widget 4, Gadget 5): Revenue and
    /// Total as in L0, Margin = Price - Cost (element-wise, larger operand
    /// left) and Overhead = Cost * 2 (reads only `costs`).
    fn setup(mode: &str) {
        for sql in [
            "CREATE TABLE sales (quarter text, product text, price float8, qty float8)",
            "INSERT INTO sales
               SELECT 'Q' || q, p, CASE p WHEN 'Widget' THEN 10 ELSE 20 END,
                      q * CASE p WHEN 'Widget' THEN 100 ELSE 30 END
               FROM generate_series(1, 4) q, unnest(ARRAY['Widget', 'Gadget']) p",
            "CREATE TABLE costs (product text, cost float8)",
            "INSERT INTO costs VALUES ('Widget', 4), ('Gadget', 5)",
            "SELECT improv.create_model('plan')",
            "SELECT improv.bind_category('plan', 'Time', 'sales', 'quarter')",
            "SELECT improv.bind_category('plan', 'Product', 'sales', 'product')",
            "SELECT improv.bind_category('plan', 'Product', 'costs', 'product')",
            "SELECT improv.bind_measure('plan', 'Price', 'sales', 'price')",
            "SELECT improv.bind_measure('plan', 'Quantity', 'sales', 'qty')",
            "SELECT improv.bind_measure('plan', 'Cost', 'costs', 'cost')",
            "SELECT improv.define('plan', 'Revenue = Price * Quantity')",
            "SELECT improv.define('plan', 'Total = SUM(Revenue OVER Time)')",
            "SELECT improv.define('plan', 'Margin = Price - Cost')",
            "SELECT improv.define('plan', 'Overhead = Cost * 2')",
        ] {
            Spi::run(sql).unwrap();
        }
        Spi::run_with_args("SELECT improv.set_sync_mode('plan', $1)", &[mode.into()]).unwrap();
    }

    fn text(sql: &str) -> String {
        Spi::get_one::<String>(&format!("SELECT ({sql})::text"))
            .unwrap()
            .unwrap_or_default()
    }

    /// `measure`'s result rows as `dim/dim=value` (dims in their column
    /// order: `time` sorts after `product`, so descending), in key order.
    fn rows(measure: &str, filter: &str) -> String {
        text(&format!(
            "SELECT string_agg(k || '=' || value, ' ' ORDER BY k) FROM (SELECT value, \
             array_to_string(ARRAY(SELECT e.value FROM jsonb_each_text(to_jsonb(r) - 'value') e \
             ORDER BY e.key DESC), '/') AS k FROM improv_plan.{measure} r WHERE {filter}) s"
        ))
    }

    fn cell(measure: &str, filter: &str) -> Option<f64> {
        Spi::get_one::<f64>(&format!(
            "SELECT (SELECT value FROM improv_plan.{measure} WHERE {filter})"
        ))
        .unwrap()
    }

    /// The ctids of `measure`'s rows matching `filter`: a rewritten row moves.
    fn ctids(measure: &str, filter: &str) -> String {
        text(&format!(
            "SELECT string_agg(ctid::text, ',' ORDER BY ctid) FROM improv_plan.{measure} \
             WHERE {filter}"
        ))
    }

    fn count(sql: &str) -> i64 {
        Spi::get_one::<i64>(&format!("SELECT count(*) FROM {sql}"))
            .unwrap()
            .unwrap()
    }

    /// Trigger mode reads an aggregated binding only at the touched
    /// coordinates (a GROUP BY under a WHERE on their dimension values) when
    /// the recompute is element-wise. The proof: an untouched coordinate
    /// whose `max(due)` is `infinity`, which a full load cannot read (no
    /// int8 epoch: it would ERROR), does not fail writes to the others.
    #[pg_test]
    fn trigger_mode_loads_only_the_touched_groups() {
        for sql in [
            "CREATE TABLE contrib (person text, year int, due date)",
            "INSERT INTO contrib VALUES ('ann', 2020, '2024-01-01'), ('ann', 2020, '2024-03-01'), \
             ('ann', 2021, '2023-01-01')",
            "SELECT improv.create_model('c')",
            "SELECT improv.bind_category('c', 'Person', 'contrib', 'person')",
            "SELECT improv.bind_category('c', 'Year', 'contrib', 'year')",
            "SELECT improv.bind_measure('c', 'Latest', 'contrib', 'due', agg => 'max')",
            "SELECT improv.define('c', 'Shown = Latest')",
            "SELECT improv.set_sync_mode('c', 'trigger')",
            // An unreadable group, slipped in behind the sync triggers.
            "ALTER TABLE contrib DISABLE TRIGGER USER",
            "INSERT INTO contrib VALUES ('bob', 1999, 'infinity')",
            "ALTER TABLE contrib ENABLE TRIGGER USER",
        ] {
            Spi::run(sql).unwrap();
        }
        let v = |filter: &str| {
            Spi::get_one::<String>(&format!(
                "SELECT (SELECT value::date::text FROM improv_c.shown WHERE {filter})"
            ))
            .unwrap()
        };
        let ann = |y: u32| v(&format!("person = 'ann' AND year = '{y}'"));
        Spi::run("INSERT INTO contrib VALUES ('ann', 2020, '2025-06-30')").unwrap();
        assert_eq!(ann(2020).as_deref(), Some("2025-06-30"));
        Spi::run("DELETE FROM contrib WHERE due = '2025-06-30'").unwrap();
        assert_eq!(ann(2020).as_deref(), Some("2024-03-01"));
        // A row moved to a new coordinate: both ends are recomputed.
        Spi::run("UPDATE contrib SET person = 'cat' WHERE year = 2021").unwrap();
        assert_eq!(
            v("person = 'cat' AND year = '2021'").as_deref(),
            Some("2023-01-01")
        );
        assert_eq!(ann(2021), None);
    }

    /// B3: two bindings on one table share one scan for the touched rows
    /// (read back by `ctid`), and a `numeric` or `int` dimension compares as
    /// itself: `1.50` and `1.5` are equal numbers but two items, and a write
    /// to one leaves the other's cell alone.
    #[pg_test]
    fn trigger_mode_reads_touched_rows_once_per_table() {
        for sql in [
            "CREATE TABLE f (k numeric, y int, a float8, b float8)",
            "INSERT INTO f VALUES (1.50, 2020, 1, 10), (1.5, 2020, 2, 20), (2, 2021, 3, 30)",
            "SELECT improv.create_model('n')",
            "SELECT improv.bind_category('n', 'K', 'f', 'k')",
            "SELECT improv.bind_category('n', 'Y', 'f', 'y')",
            "SELECT improv.bind_measure('n', 'A', 'f', 'a', agg => 'sum')",
            "SELECT improv.bind_measure('n', 'B', 'f', 'b', agg => 'sum')",
            "SELECT improv.define('n', 'RA = A * 1')",
            "SELECT improv.define('n', 'RB = B + A')",
            "SELECT improv.set_sync_mode('n', 'trigger')",
            "INSERT INTO f VALUES (1.5, 2020, 5, 50)",
        ] {
            Spi::run(sql).unwrap();
        }
        let all = |t: &str| {
            text(&format!(
                "SELECT string_agg(k || '/' || y || '=' || value, ' ' ORDER BY k, y) FROM improv_n.{t}"
            ))
        };
        assert_eq!(all("ra"), "1.5/2020=7 1.50/2020=1 2/2021=3");
        assert_eq!(all("rb"), "1.5/2020=77 1.50/2020=11 2/2021=33");
        Spi::run("UPDATE f SET a = a + 100 WHERE k = 1.50 AND k::text = '1.50'").unwrap();
        assert_eq!(all("ra"), "1.5/2020=7 1.50/2020=101 2/2021=3");
        Spi::run("DELETE FROM f WHERE y = 2021").unwrap();
        assert_eq!(all("rb"), "1.5/2020=77 1.50/2020=111");
    }

    /// P1-4: `agg => 'count_distinct'` counts each coordinate's distinct
    /// values, in the load and in trigger mode; deleting one of two rows
    /// that share a value keeps the count. NULLs are not counted, as in
    /// `count(DISTINCT)`: a coordinate of only NULLs counts 0.
    #[pg_test]
    fn count_distinct_bindings_count_distinct_values() {
        for sql in [
            "CREATE TABLE contrib (person text, sha text)",
            "INSERT INTO contrib VALUES ('ann', 'a1'), ('ann', 'a1'), ('ann', 'a2'), ('bob', 'b1'), \
             ('ann', NULL), ('ann', NULL), ('dan', NULL), (NULL, 'z1')",
            "SELECT improv.create_model('c')",
            "SELECT improv.bind_category('c', 'Person', 'contrib', 'person')",
            "SELECT improv.bind_measure('c', 'Commits', 'contrib', 'sha', agg => 'count_distinct')",
            "SELECT improv.define('c', 'Shown = Commits * 1')",
            "SELECT improv.set_sync_mode('c', 'trigger')",
        ] {
            Spi::run(sql).unwrap();
        }
        let n = |p: &str| {
            Spi::get_one::<f64>(&format!(
                "SELECT (SELECT value FROM improv_c.shown WHERE person = '{p}')"
            ))
            .unwrap()
        };
        assert_eq!(
            (n("ann"), n("bob"), n("dan")),
            (Some(2.0), Some(1.0), Some(0.0))
        );
        let same = "SELECT count(*) FROM (SELECT person, count(DISTINCT sha)::float8 AS v \
                    FROM contrib WHERE person IS NOT NULL GROUP BY 1) w \
                    FULL JOIN improv_c.shown s USING (person) WHERE w.v IS DISTINCT FROM s.value";
        assert_eq!(Spi::get_one::<i64>(same).unwrap(), Some(0));
        Spi::run(
            "DELETE FROM contrib WHERE ctid = (SELECT min(ctid) FROM contrib WHERE sha = 'a1')",
        )
        .unwrap();
        assert_eq!(n("ann"), Some(2.0));
        Spi::run("DELETE FROM contrib WHERE sha = 'a1'").unwrap();
        assert_eq!(n("ann"), Some(1.0));
        Spi::run("INSERT INTO contrib VALUES ('bob', 'b1'), ('bob', 'b2'), ('cat', 'c1')").unwrap();
        assert_eq!((n("bob"), n("cat")), (Some(2.0), Some(1.0)));
    }

    /// Every entry point locks its model first; that must work as the first
    /// statement of a transaction, before anything has assigned an xid.
    #[pg_test]
    fn lock_model_works_before_any_write() {
        let e = crate::lock_model("nope").unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
    }

    #[pg_test]
    fn views_bind_only_in_manual_mode() {
        Spi::run(
            "CREATE TABLE vt (g text PRIMARY KEY, v float8); \
             INSERT INTO vt VALUES ('a', 2); \
             CREATE VIEW vv AS SELECT * FROM vt; \
             SELECT improv.create_model('vm'); \
             SELECT improv.bind_category('vm', 'G', 'vv', 'g');",
        )
        .unwrap();
        // Logical is the default: a view is refused with a clear message
        // (the check bind_measure runs, before it writes anything).
        let oid = Spi::get_one::<pg_sys::Oid>("SELECT 'vv'::regclass::oid")
            .unwrap()
            .unwrap();
        let err = crate::check_syncable("vv", crate::relkind(oid).unwrap(), "logical").unwrap_err();
        assert!(err.to_string().contains("vv is a view"), "{err}");
        assert!(crate::check_syncable("vt", 'r', "logical").is_ok());
        Spi::run("SELECT improv.set_sync_mode('vm', 'manual')").unwrap();
        Spi::run("SELECT improv.bind_measure('vm', 'A', 'vv', 'v')").unwrap();
        Spi::run("SELECT improv.define('vm', 'B = A * 3')").unwrap();
        Spi::run("SELECT improv.recalc('vm')").unwrap();
        assert_eq!(
            Spi::get_one::<f64>("SELECT value FROM improv_vm.b WHERE g = 'a'").unwrap(),
            Some(6.0)
        );
        for mode in ["trigger", "logical"] {
            let err = crate::trigger::set_sync_mode("vm", mode).unwrap_err();
            assert!(err.to_string().contains("vv is a view"), "{mode}: {err}");
        }
    }

    #[pg_test]
    fn set_sync_mode_installs_and_removes_triggers() {
        setup("trigger");
        let triggers = || {
            text(
                "SELECT string_agg(tgrelid::regclass || '.' || tgname, ',' ORDER BY tgrelid::regclass::text, tgname) \
                 FROM pg_trigger WHERE tgname LIKE 'improv_plan_sync_%'",
            )
        };
        assert_eq!(
            triggers(),
            "costs.improv_plan_sync_del,costs.improv_plan_sync_ins,costs.improv_plan_sync_trn,\
             costs.improv_plan_sync_upd,sales.improv_plan_sync_del,sales.improv_plan_sync_ins,\
             sales.improv_plan_sync_trn,sales.improv_plan_sync_upd"
        );
        assert_eq!(text("SELECT sync_mode FROM improv.model"), "trigger");
        // Entering trigger mode recomputes, so results start consistent.
        assert_eq!(rows("total", "true"), "Gadget=6000 Widget=10000");
        assert_eq!(
            cell("margin", "\"time\" = 'Q1' AND product = 'Gadget'"),
            Some(15.0)
        );
        let version = || text("SELECT tx FROM improv.model_version");
        let before = version();
        Spi::run("SELECT improv.set_sync_mode('plan', 'manual')").unwrap();
        assert_eq!(triggers(), "");
        assert_ne!(version(), before, "a mode change bumps model_version");
        Spi::run("UPDATE sales SET price = 99").unwrap();
        assert_eq!(
            rows("total", "true"),
            "Gadget=6000 Widget=10000",
            "manual: no sync"
        );
        let e = crate::trigger::set_sync_mode("plan", "eager").unwrap_err();
        assert!(
            e.to_string().contains("logical, trigger, commit or manual"),
            "{e}"
        );
        // drop_model removes the triggers with the model.
        Spi::run("SELECT improv.set_sync_mode('plan', 'trigger')").unwrap();
        Spi::run("SELECT improv.drop_model('plan')").unwrap();
        assert_eq!(triggers(), "");
        Spi::run("UPDATE sales SET price = 1").unwrap();
    }

    #[pg_test]
    fn update_is_visible_in_the_writers_transaction() {
        setup("trigger");
        let overhead = ctids("overhead", "true");
        let gadget = ctids("revenue", "product = 'Gadget'");
        Spi::run("SET track_functions = 'all'").unwrap();
        // pg_test runs in one transaction: every read below precedes COMMIT.
        Spi::run("UPDATE sales SET price = 11 WHERE product = 'Widget'").unwrap();
        assert_eq!(
            rows("revenue", "product = 'Widget'"),
            "Q1/Widget=1100 Q2/Widget=2200 Q3/Widget=3300 Q4/Widget=4400"
        );
        assert_eq!(rows("total", "true"), "Gadget=6000 Widget=11000");
        assert_eq!(
            cell("margin", "\"time\" = 'Q3' AND product = 'Widget'"),
            Some(7.0)
        );
        // One 4-row UPDATE statement: one trigger firing.
        let calls = Spi::get_one::<i64>(
            "SELECT calls FROM pg_stat_xact_user_functions WHERE funcname = 'on_change'",
        )
        .unwrap();
        assert_eq!(calls, Some(1));
        // Overhead reads only `costs`, outside Price's closure: not rewritten.
        assert_eq!(ctids("overhead", "true"), overhead);
        // Gadget's Revenue was recomputed but is unchanged: not rewritten.
        assert_eq!(ctids("revenue", "product = 'Gadget'"), gadget);
    }

    #[pg_test]
    fn element_wise_change_rewrites_only_its_coordinates() {
        setup("trigger");
        let gadget = ctids("margin", "product = 'Gadget'");
        let revenue = ctids("revenue", "true");
        Spi::run("UPDATE costs SET cost = 1 WHERE product = 'Widget'").unwrap();
        // Cost feeds only Margin and Overhead, both element-wise: the plan
        // is restricted to Widget, and Gadget's rows must survive it.
        assert_eq!(
            rows("margin", "true"),
            "Q1/Gadget=15 Q1/Widget=9 Q2/Gadget=15 Q2/Widget=9 \
             Q3/Gadget=15 Q3/Widget=9 Q4/Gadget=15 Q4/Widget=9"
        );
        assert_eq!(rows("overhead", "true"), "Gadget=10 Widget=2");
        assert_eq!(ctids("margin", "product = 'Gadget'"), gadget);
        assert_eq!(ctids("revenue", "true"), revenue);
    }

    #[pg_test]
    fn rolled_back_change_reverts_results() {
        setup("trigger");
        Spi::run(
            "DO $$ BEGIN
               UPDATE sales SET price = 99 WHERE quarter = 'Q1' AND product = 'Widget';
               IF (SELECT value FROM improv_plan.revenue
                   WHERE \"time\" = 'Q1' AND product = 'Widget') <> 9900 THEN
                 RAISE EXCEPTION 'the trigger did not apply in the transaction';
               END IF;
               RAISE EXCEPTION USING ERRCODE = 'P0099', MESSAGE = 'roll back';
             EXCEPTION WHEN SQLSTATE 'P0099' THEN NULL;
             END $$",
        )
        .unwrap();
        assert_eq!(
            cell("revenue", "\"time\" = 'Q1' AND product = 'Widget'"),
            Some(1000.0)
        );
        assert_eq!(rows("total", "true"), "Gadget=6000 Widget=10000");
        assert_eq!(crate::trigger::recalc_measures("plan", None).unwrap(), 0);
    }

    #[pg_test]
    fn insert_delete_and_truncate_follow_the_source() {
        setup("trigger");
        Spi::run("INSERT INTO sales VALUES ('Q1', 'Doohickey', 3, 2), ('Q5', 'Widget', 10, 1)")
            .unwrap();
        assert_eq!(count("improv.item WHERE name IN ('Doohickey', 'Q5')"), 2);
        assert_eq!(
            cell("revenue", "\"time\" = 'Q1' AND product = 'Doohickey'"),
            Some(6.0)
        );
        assert_eq!(
            cell("revenue", "\"time\" = 'Q5' AND product = 'Widget'"),
            Some(10.0)
        );
        assert_eq!(
            rows("total", "true"),
            "Doohickey=6 Gadget=6000 Widget=10010"
        );
        // An element-wise insert, onto an item another table minted.
        Spi::run("INSERT INTO costs VALUES ('Doohickey', 1)").unwrap();
        assert_eq!(
            cell("margin", "\"time\" = 'Q1' AND product = 'Doohickey'"),
            Some(2.0)
        );
        assert_eq!(rows("overhead", "true"), "Doohickey=2 Gadget=10 Widget=8");

        Spi::run("DELETE FROM costs WHERE product = 'Widget'").unwrap();
        assert_eq!(count("improv_plan.margin WHERE product = 'Widget'"), 0);
        assert_eq!(count("improv_plan.margin WHERE product = 'Gadget'"), 4);
        assert_eq!(rows("overhead", "true"), "Doohickey=2 Gadget=10");
        Spi::run("DELETE FROM sales WHERE product = 'Gadget'").unwrap();
        assert_eq!(count("improv_plan.revenue WHERE product = 'Gadget'"), 0);
        assert_eq!(rows("total", "true"), "Doohickey=6 Widget=10010");
        assert_eq!(count("improv_plan.margin"), 1);
        // A no-op statement fires but changes nothing.
        Spi::run("DELETE FROM sales WHERE product = 'Nothing'").unwrap();
        Spi::run("TRUNCATE costs").unwrap();
        assert_eq!(count("improv_plan.overhead"), 0);
        assert_eq!(count("improv_plan.margin"), 0);
        assert_eq!(count("improv_plan.revenue"), 6);
    }

    /// Commit mode: run the pre-commit recompute as COMMIT would and return
    /// the model recomputes it ran (a pg_test never commits; tests/txn.sh
    /// commits for real).
    fn commit() -> u64 {
        let before = crate::commit::recomputes();
        crate::commit::pre_commit();
        crate::commit::recomputes() - before
    }

    #[pg_test]
    fn commit_mode_skips_result_tables_without_their_key() {
        setup("commit");
        commit();
        // As in a restore, before its post-data step adds the keys.
        Spi::run("ALTER TABLE improv_plan.revenue DROP CONSTRAINT revenue_pkey").unwrap();
        Spi::run("UPDATE sales SET qty = 1 WHERE product = 'Widget'").unwrap();
        assert_eq!(commit(), 0);
        assert_eq!(rows("total", "true"), "Gadget=6000 Widget=10000");
        Spi::run("ALTER TABLE improv_plan.revenue ADD PRIMARY KEY (\"time\", product)").unwrap();
        Spi::run("UPDATE sales SET qty = 1 WHERE product = 'Widget'").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(rows("total", "true"), "Gadget=6000 Widget=40");
    }

    #[pg_test]
    fn commit_mode_recomputes_once_per_transaction() {
        setup("commit");
        commit(); // setup's own writes
        assert_eq!(
            text("SELECT sync_mode FROM improv.status('plan')"),
            "commit"
        );
        assert_eq!(
            count("pg_trigger WHERE tgname LIKE 'improv_plan_sync_%'"),
            0
        );
        // Entering commit mode recomputes, so results start consistent.
        assert_eq!(rows("total", "true"), "Gadget=6000 Widget=10000");
        assert_eq!(
            cell("margin", "\"time\" = 'Q1' AND product = 'Gadget'"),
            Some(15.0)
        );
        // Statements do not recompute; the commit does, once.
        Spi::run("UPDATE sales SET price = 11 WHERE product = 'Widget'").unwrap();
        Spi::run("INSERT INTO sales VALUES ('Q5', 'Widget', 10, 1)").unwrap();
        Spi::run("DELETE FROM sales WHERE product = 'Gadget' AND quarter = 'Q4'").unwrap();
        Spi::run("UPDATE costs SET cost = 1 WHERE product = 'Widget'").unwrap();
        assert_eq!(
            rows("total", "true"),
            "Gadget=6000 Widget=10000",
            "read before the commit: the old results"
        );
        assert_eq!(commit(), 1);
        assert_eq!(rows("total", "true"), "Gadget=3600 Widget=11010");
        assert_eq!(
            cell("margin", "\"time\" = 'Q1' AND product = 'Widget'"),
            Some(10.0)
        );
        assert_eq!(rows("overhead", "true"), "Gadget=10 Widget=2");
        assert_eq!(commit(), 0, "nothing written since");
        // Neither a write to an unbound table nor one that changes no row.
        Spi::run("CREATE TABLE other (x int); INSERT INTO other VALUES (1)").unwrap();
        Spi::run("DELETE FROM sales WHERE product = 'Nothing'").unwrap();
        assert_eq!(commit(), 0);
        // A writable CTE counts, whatever its outer query returns.
        Spi::run("WITH u AS (UPDATE costs SET cost = 2 RETURNING 1) SELECT 1 WHERE false").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(rows("overhead", "true"), "Gadget=4 Widget=4");
        // Out of commit mode, writes are not followed.
        Spi::run("SELECT improv.set_sync_mode('plan', 'manual')").unwrap();
        Spi::run("UPDATE sales SET price = 1").unwrap();
        assert_eq!(commit(), 0);
    }

    #[pg_test]
    fn commit_mode_forgets_rolled_back_writes_and_follows_truncate() {
        setup("commit");
        commit();
        // ROLLBACK: the results never moved, and the next commit has
        // nothing to do.
        Spi::run("UPDATE costs SET cost = 9").unwrap();
        assert_eq!(rows("overhead", "true"), "Gadget=10 Widget=8");
        // SAFETY: the callback as Postgres calls it at ROLLBACK.
        unsafe {
            crate::commit::xact_callback(pg_sys::XactEvent::XACT_EVENT_ABORT, std::ptr::null_mut())
        };
        assert_eq!(commit(), 0);
        // (The data did not roll back: a pg_test is one transaction.)
        Spi::run("UPDATE costs SET cost = CASE product WHEN 'Widget' THEN 4 ELSE 5 END").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(rows("overhead", "true"), "Gadget=10 Widget=8");
        // A rolled-back savepoint's write does not count; a released one's does.
        Spi::run(
            "DO $$ BEGIN
               BEGIN UPDATE costs SET cost = 7; RAISE EXCEPTION 'undo';
               EXCEPTION WHEN raise_exception THEN NULL; END;
             END $$",
        )
        .unwrap();
        assert_eq!(commit(), 0, "rolled-back savepoint");
        Spi::run(
            "DO $$ BEGIN
               BEGIN UPDATE costs SET cost = 7 WHERE product = 'Widget';
               EXCEPTION WHEN others THEN NULL; END;
             END $$",
        )
        .unwrap();
        assert_eq!(commit(), 1, "released savepoint");
        assert_eq!(rows("overhead", "true"), "Gadget=10 Widget=14");
        // TRUNCATE has no executor write; the utility hook notes it.
        Spi::run("CREATE TABLE t2 (x int); TRUNCATE t2").unwrap();
        assert_eq!(commit(), 0, "an unbound table");
        Spi::run("TRUNCATE costs").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(count("improv_plan.overhead"), 0);
        assert_eq!(count("improv_plan.margin"), 0);
        assert_eq!(count("improv_plan.revenue"), 8);
    }

    /// B2: with `improv.commit_capture = 'rows'`, COMMIT recomputes only
    /// the captured coordinates (the results equal a full recompute's), and
    /// falls back to a full recompute when a write got past the capture or
    /// the change is not element-wise.
    #[pg_test]
    fn commit_mode_capture_recomputes_the_touched_rows() {
        let scoped = || crate::commit::scoped();
        setup("manual");
        Spi::run("SET improv.commit_capture = 'rows'").unwrap();
        Spi::run("SELECT improv.set_sync_mode('plan', 'commit')").unwrap();
        commit();
        assert_eq!(
            count("pg_trigger WHERE tgname LIKE 'improv_plan_cap_%'"),
            8,
            "four events on each of sales and costs"
        );
        assert_eq!(
            count("pg_trigger WHERE tgname LIKE 'improv_plan_sync_%'"),
            0
        );
        // Margin and Overhead read only Cost and Price, element-wise: an
        // UPDATE of costs is captured and scoped.
        let s0 = scoped();
        Spi::run("UPDATE costs SET cost = 1 WHERE product = 'Widget'").unwrap();
        Spi::run("INSERT INTO costs VALUES ('Doohickey', 3)").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(scoped(), s0 + 1);
        assert_eq!(rows("overhead", "true"), "Doohickey=6 Gadget=10 Widget=2");
        assert_eq!(
            cell("margin", "\"time\" = 'Q1' AND product = 'Widget'"),
            Some(9.0)
        );
        // Total sums Revenue over Time: not element-wise, so in full.
        Spi::run("UPDATE sales SET price = 11 WHERE product = 'Widget'").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(scoped(), s0 + 1);
        assert_eq!(rows("total", "true"), "Gadget=6000 Widget=11000");
        // A write with the capture disabled: in full, and still right.
        Spi::run("ALTER TABLE costs DISABLE TRIGGER USER").unwrap();
        Spi::run("UPDATE costs SET cost = 2 WHERE product = 'Gadget'").unwrap();
        Spi::run("ALTER TABLE costs ENABLE TRIGGER USER").unwrap();
        Spi::run("UPDATE costs SET cost = 3 WHERE product = 'Widget'").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(scoped(), s0 + 1, "a statement got past the capture");
        assert_eq!(rows("overhead", "true"), "Doohickey=6 Gadget=4 Widget=6");
        // A rolled-back savepoint's capture goes with it; TRUNCATE is in full.
        Spi::run(
            "DO $$ BEGIN
               BEGIN UPDATE costs SET cost = 50; RAISE EXCEPTION 'undo';
               EXCEPTION WHEN raise_exception THEN NULL; END;
             END $$",
        )
        .unwrap();
        assert_eq!(commit(), 0, "rolled-back savepoint");
        Spi::run("TRUNCATE costs").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(count("improv_plan.overhead"), 0);
        // Leaving commit mode removes the capture triggers.
        Spi::run("SELECT improv.set_sync_mode('plan', 'manual')").unwrap();
        assert_eq!(count("pg_trigger WHERE tgname LIKE 'improv_plan_cap_%'"), 0);
        assert_eq!(
            text("SELECT current_setting('improv.commit_capture')"),
            "rows"
        );
    }

    #[pg_test]
    fn commit_mode_follows_partitions() {
        Spi::run(
            "CREATE TABLE pt (k text, v float8) PARTITION BY LIST (k);
             CREATE TABLE pt_a PARTITION OF pt FOR VALUES IN ('a');
             CREATE TABLE pt_b PARTITION OF pt FOR VALUES IN ('b');
             INSERT INTO pt VALUES ('a', 1), ('b', 2);
             SELECT improv.create_model('pm');
             SELECT improv.set_sync_mode('pm', 'commit');
             SELECT improv.bind_category('pm', 'K', 'pt', 'k');
             SELECT improv.bind_measure('pm', 'V', 'pt', 'v');
             SELECT improv.define('pm', 'W = V * 10');",
        )
        .unwrap();
        commit();
        let w = |k: &str| {
            Spi::get_one_with_args::<f64>("SELECT value FROM improv_pm.w WHERE k = $1", &[k.into()])
                .unwrap()
        };
        assert_eq!(w("a"), Some(10.0));
        // A write to a partition of the bound table.
        Spi::run("UPDATE pt_b SET v = 5").unwrap();
        assert_eq!(commit(), 1);
        assert_eq!(w("b"), Some(50.0));
    }

    #[pg_test]
    fn recalc_named_measures_and_their_dependents() {
        setup("manual");
        assert_eq!(crate::recalc("plan").unwrap(), 20);
        Spi::run("UPDATE sales SET price = 11 WHERE product = 'Widget'").unwrap();
        Spi::run("UPDATE costs SET cost = 1").unwrap();
        // Cost's dependents: Margin (8 rows) and Overhead (2).
        let n = Spi::get_one::<i64>("SELECT improv.recalc('plan', ARRAY['cost'])").unwrap();
        assert_eq!(n, Some(10));
        assert_eq!(rows("overhead", "true"), "Gadget=2 Widget=2");
        assert_eq!(
            cell("margin", "\"time\" = 'Q1' AND product = 'Widget'"),
            Some(10.0)
        );
        assert_eq!(
            rows("total", "true"),
            "Gadget=6000 Widget=10000",
            "not Cost's dependent"
        );
        // A derived root: itself and its dependents.
        let n = Spi::get_one::<i64>("SELECT improv.recalc('plan', ARRAY['Revenue'])").unwrap();
        assert_eq!(n, Some(5));
        assert_eq!(rows("total", "true"), "Gadget=6000 Widget=11000");
        assert_eq!(
            Spi::get_one::<i64>("SELECT improv.recalc('plan', NULL)").unwrap(),
            Some(0)
        );
        let e = crate::trigger::recalc_measures("plan", Some(vec!["Nope".into()])).unwrap_err();
        assert!(e.to_string().contains("unknown measure"), "{e}");
    }

    #[pg_test]
    fn what_if_returns_deltas_and_writes_nothing() {
        setup("manual");
        crate::recalc("plan").unwrap();
        let snapshot = || {
            text(
                "SELECT (SELECT tx FROM improv.model_version) || ':' || \
                 (SELECT count(*) FROM improv.item) || ':' || \
                 (SELECT string_agg(ctid::text, ',') FROM improv_plan.revenue) || ':' || \
                 (SELECT string_agg(ctid::text, ',') FROM improv_plan.total)",
            )
        };
        let before = snapshot();
        let got = text(
            r#"SELECT string_agg(measure || ' ' || coord::text || ' ' || coalesce(value::text, 'NULL'),
                 '; ' ORDER BY measure, coord::text)
               FROM improv.what_if('plan', '[
                 {"measure": "Price", "coord": {"Time": "Q1", "Product": "Widget"}, "value": 11},
                 {"measure": "Cost", "coord": {"Product": "Gadget"}, "value": null}]')"#,
        );
        assert_eq!(
            got,
            "Margin {\"Time\": \"Q1\", \"Product\": \"Gadget\"} NULL; \
             Margin {\"Time\": \"Q1\", \"Product\": \"Widget\"} 7; \
             Margin {\"Time\": \"Q2\", \"Product\": \"Gadget\"} NULL; \
             Margin {\"Time\": \"Q3\", \"Product\": \"Gadget\"} NULL; \
             Margin {\"Time\": \"Q4\", \"Product\": \"Gadget\"} NULL; \
             Overhead {\"Product\": \"Gadget\"} NULL; \
             Revenue {\"Time\": \"Q1\", \"Product\": \"Widget\"} 1100; \
             Total {\"Product\": \"Widget\"} 10100"
        );
        assert_eq!(snapshot(), before, "what_if writes nothing");
        assert_eq!(count("improv.what_if('plan', '[]')"), 0);
        // Edits of one category set under an element-wise chain (Cost feeds
        // only Margin and Overhead) load and compare only their coordinates:
        // a stale result elsewhere is not reported, as nothing there changes.
        Spi::run("UPDATE improv_plan.overhead SET value = 999 WHERE product = 'Widget'").unwrap();
        let got = text(
            r#"SELECT string_agg(measure || ' ' || coord::text || ' ' || coalesce(value::text, 'NULL'),
                 '; ' ORDER BY measure, coord::text)
               FROM improv.what_if('plan', '[
                 {"measure": "Cost", "coord": {"Product": "Gadget"}, "value": 6}]')"#,
        );
        assert_eq!(
            got,
            "Margin {\"Time\": \"Q1\", \"Product\": \"Gadget\"} 14; \
             Margin {\"Time\": \"Q2\", \"Product\": \"Gadget\"} 14; \
             Margin {\"Time\": \"Q3\", \"Product\": \"Gadget\"} 14; \
             Margin {\"Time\": \"Q4\", \"Product\": \"Gadget\"} 14; \
             Overhead {\"Product\": \"Gadget\"} 12"
        );
        // Mixed category sets take the full load, which sees the stale row.
        let full = text(
            r#"SELECT string_agg(measure || ' ' || coord::text, '; ' ORDER BY measure, coord::text)
               FROM improv.what_if('plan', '[
                 {"measure": "Cost", "coord": {"Product": "Gadget"}, "value": 5},
                 {"measure": "Price", "coord": {"Time": "Q1", "Product": "Gadget"}, "value": 20}]')"#,
        );
        assert_eq!(full, "Overhead {\"Product\": \"Widget\"}");
        // `error!` would abort the test's transaction: call the Rust side.
        let obj = Spi::get_one::<pgrx::JsonB>("SELECT '{}'::jsonb")
            .unwrap()
            .unwrap();
        let e = crate::trigger::what_if_rows("plan", &obj).unwrap_err();
        assert!(e.to_string().contains("array"), "{e}");
        let e = crate::trigger::what_if_rows("nope", &obj).unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
        // A role that may not act as the owner is refused, not shown nothing.
        let owner = text("SELECT owner::text FROM improv.model WHERE name = 'plan'");
        Spi::run(
            "CREATE ROLE wi_other; GRANT USAGE ON SCHEMA improv TO wi_other; \
             GRANT SELECT ON ALL TABLES IN SCHEMA improv TO wi_other; SET ROLE wi_other",
        )
        .unwrap();
        let none = Spi::get_one::<pgrx::JsonB>("SELECT '[]'::jsonb")
            .unwrap()
            .unwrap();
        let e = crate::trigger::what_if_rows("plan", &none).unwrap_err();
        Spi::run("RESET ROLE").unwrap();
        assert_eq!(
            e.to_string(),
            format!(
                "permission denied for improv model \"plan\": what_if needs to act as its \
                 owner, {owner} (see Privileges)"
            )
        );
    }
}
