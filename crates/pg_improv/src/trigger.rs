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
    pub(crate) model: Model,
}

/// Load model `mid` (named `name`) with its bound and unbound input cells.
/// Items for new dimension values are minted through `improv.mint_items`
/// when `persist`, else kept in memory.
// ponytail: duplicates L0 `recalc`'s loading; fold `recalc(text)` onto
// `load` + `recompute` once L1 and L2 have merged.
pub(crate) fn load(mid: i64, name: String, persist: bool) -> Res<Loaded> {
    pm::validate_name("model", &name)?;
    let mut model = pm::build_model(&read_catalog(mid)?)?;
    // (measure id, relation, value column, agg, dimension columns)
    type Row = (i64, String, String, Option<String>, Vec<String>);
    let bindings: Vec<Row> = Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT b.measure_id, b.rel::text, b.value_col, b.agg, \
                    ARRAY(SELECT b.dim_cols ->> c.name \
                          FROM unnest(m.categories) WITH ORDINALITY u(cid, ord) \
                          JOIN improv.category c ON c.model_id = b.model_id AND c.id = u.cid \
                          ORDER BY u.ord) \
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
            ))
        })
        .collect::<Res<_>>()
    })?;
    let mut bound = Vec::with_capacity(bindings.len());
    for (measure_id, rel, value_col, agg, dims) in bindings {
        check_for_interrupts!();
        let measure = MeasureId(id32(measure_id)?);
        let vt = model
            .measures
            .get(&measure)
            .ok_or("binding for an unknown measure")?
            .value_type;
        let sql = pm::select_bound_sql(&rel, &dims, &value_col, agg.as_deref(), vt)?;
        let rows = Spi::connect(|c| {
            c.select(&sql, None, &[])?
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
    Ok(Loaded { name, model })
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
/// in `plan.scope`, when given). Returns the rows changed.
fn recompute(l: &mut Loaded, plan: &pm::Plan) -> Res<i64> {
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
        let mut old = read_results(&table, &dims, vt)?;
        if let Some(s) = &plan.scope {
            s.retain_rows(&l.model, target, &mut old)?;
            s.retain_rows(&l.model, target, &mut new)?;
        }
        changed += write_results(&table, &dims, vt, &old, &new)?;
    }
    Ok(changed)
}

/// Drop model `name`'s sync triggers from every relation.
pub(crate) fn remove_triggers(name: &str) -> Res<()> {
    let names = EVENTS
        .iter()
        .map(|(op, _, _)| pm::sync_trigger_name(name, op))
        .collect::<pm::Result<Vec<_>>>()?;
    let found: Vec<(String, String)> = Spi::connect(|c| -> Res<_> {
        c.select(
            "SELECT tgrelid::regclass::text, tgname::text FROM pg_trigger WHERE tgname = ANY($1)",
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
}

/// Set how `model`'s results follow its bound data: `trigger` installs
/// statement-level sync triggers on every bound relation (re-run after
/// binding a new relation) and recomputes once so the results start
/// consistent; `logical` and `manual` remove them.
#[pg_extern(security_invoker)]
fn set_sync_mode(model: &str, mode: &str) -> Res<()> {
    if !matches!(mode, "logical" | "trigger" | "manual") {
        return Err(format!("sync mode must be logical, trigger or manual, not {mode:?}").into());
    }
    let (mid, name) = lock_model(model)?;
    pm::validate_name("model", &name)?;
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
    if mode == "trigger" {
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
                Spi::run(&format!(
                    "CREATE TRIGGER {} AFTER {event} ON {rel} {refs} \
                     FOR EACH STATEMENT EXECUTE FUNCTION improv.on_change({})",
                    pm::quote_ident(&pm::sync_trigger_name(&name, op)?),
                    pm::quote_literal(&name)
                ))?;
            }
        }
        let mut l = load(mid, name, true)?;
        let targets = all_derived(&l.model);
        recompute(
            &mut l,
            &pm::Plan {
                targets,
                scope: None,
            },
        )?;
    }
    bump_version(mid)
}

/// The sync trigger: recompute the changed inputs' dependents in the
/// writer's transaction. Its one argument is the model name.
#[pg_trigger]
fn on_change<'a>(
    t: &'a PgTrigger<'a>,
) -> Result<Option<PgHeapTuple<'a, AllocatedByPostgres>>, PgHeapTupleError> {
    if let Err(e) = sync(t) {
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
    let (cats, cols): (Vec<i64>, Vec<String>) = Spi::connect(|c| -> Res<Vec<(i64, String)>> {
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
    .unzip();
    let rows = touched_rows(t, &cols)?;
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
    let changed = changed
        .into_iter()
        .map(|i| id32(i).map(MeasureId))
        .collect::<Res<Vec<_>>>()?;
    let mut l = load(mid, name, true)?;
    let plan = match rows {
        Some(rows) => {
            let cats = cats
                .into_iter()
                .map(|c| id32(c).map(CategoryId))
                .collect::<Res<Vec<_>>>()?;
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
fn touched_rows(t: &PgTrigger<'_>, cols: &[String]) -> Res<Option<Vec<Vec<Option<String>>>>> {
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
/// NULL: the cell would vanish). Writes nothing.
#[pg_extern(security_invoker)]
fn what_if(
    model: &str,
    edits: JsonB,
) -> TableIterator<
    'static,
    (
        name!(measure, String),
        name!(coord, JsonB),
        name!(value, Option<f64>),
    ),
> {
    // pgrx 0.17 cannot generate SQL for a `Result<TableIterator>`.
    match what_if_rows(model, &edits) {
        Ok(rows) => TableIterator::new(rows),
        Err(e) => error!("{e}"),
    }
}

/// One `what_if` result row.
type WhatIfRow = (String, JsonB, Option<f64>);

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
    let mut l = load(mid, name, false)?;
    let changed = pm::apply_edits(&mut l.model, &edits.0.to_string())?;
    let targets = pm::dependents(&l.model, &changed);
    let out = evaluate(&l.model, &targets)?;
    let empty = Default::default();
    let mut rows = Vec::new();
    for target in targets {
        // ponytail: `value` is float8, so only number measures are reported;
        // a jsonb value column would carry the other types.
        if l.model.measures[&target].value_type != ValueType::Number {
            continue;
        }
        let new = pm::result_rows(&l.model, target, out.get(&target).unwrap_or(&empty))?;
        let dims = pm::dim_columns(&l.model, target)?;
        let mname = l.model.measures[&target].name.0.clone();
        pm::validate_name("measure", &mname)?;
        let table = pm::result_table(&l.name, &mname);
        let old = if exists(&table)? {
            read_results(&table, &dims, ValueType::Number)?
        } else {
            Default::default()
        };
        for (key, v) in pm::cell_changes(&old, &new) {
            rows.push((
                mname.clone(),
                JsonB(pm::coord_json(&l.model, target, &key)?),
                v.and_then(|v| v.as_num()),
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

    /// Every entry point locks its model first; that must work as the first
    /// statement of a transaction, before anything has assigned an xid.
    #[pg_test]
    fn lock_model_works_before_any_write() {
        let e = crate::lock_model("nope").unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
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
        assert!(e.to_string().contains("logical, trigger or manual"), "{e}");
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
        // `error!` would abort the test's transaction: call the Rust side.
        let obj = Spi::get_one::<pgrx::JsonB>("SELECT '{}'::jsonb")
            .unwrap()
            .unwrap();
        let e = crate::trigger::what_if_rows("plan", &obj).unwrap_err();
        assert!(e.to_string().contains("array"), "{e}");
        let e = crate::trigger::what_if_rows("nope", &obj).unwrap_err();
        assert!(e.to_string().contains("does not exist"), "{e}");
    }
}
