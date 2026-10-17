//! `pg_improv`: Improv multidimensional models over Postgres data.
//!
//! A thin shell: SPI reads the catalog and the bound fact tables, the
//! Postgres-free [`improv_pg_model`] crate turns them into an Improv model and
//! the engine's results into SQL, and SPI writes the result tables. The engine
//! runs on the backend thread (`timely::execute_directly`); no Postgres API is
//! ever called from another thread.

use std::error::Error;

use improv_core_model::{CategoryId, ItemId, MeasureId, Model, ValueType};
use improv_engine::CellValue;
use improv_pg_model as pm;
use pgrx::datum::DatumWithOid;
use pgrx::prelude::*;
use pgrx::{GucContext, GucFlags, GucRegistry, GucSetting, PgRelation};

::pgrx::pg_module_magic!();

mod language;
mod trigger;
mod worker;

/// Errors returned to SQL as `ERROR`s.
type Res<T> = Result<T, Box<dyn Error>>;

/// `improv.max_cells`: the most bound input cells one model may hold.
static MAX_CELLS: GucSetting<i32> = GucSetting::<i32>::new(5_000_000);

/// `improv.raise_errors`: whether a `LANGUAGE improv` function whose result
/// is an error value (`#DIV/0!`, …) raises it instead of returning NULL.
pub(crate) static RAISE_ERRORS: GucSetting<bool> = GucSetting::<bool>::new(false);

#[allow(non_snake_case)]
#[pg_guard]
pub extern "C-unwind" fn _PG_init() {
    GucRegistry::define_int_guc(
        c"improv.max_cells",
        c"Maximum bound input cells per Improv model.",
        c"improv.recalc refuses to build an engine over a model with more bound input cells.",
        &MAX_CELLS,
        0,
        i32::MAX,
        GucContext::Userset,
        GucFlags::default(),
    );
    GucRegistry::define_bool_guc(
        c"improv.raise_errors",
        c"Raise a LANGUAGE improv error value instead of returning NULL.",
        c"When on, a LANGUAGE improv function whose result is an error value (#DIV/0!, #NUM!, #N/A, ...) raises an ERROR naming it; when off it returns NULL.",
        &RAISE_ERRORS,
        GucContext::Userset,
        GucFlags::default(),
    );
    worker::init();
}

extension_sql!(
    r#"
-- The shared catalog: a compatible superset of improv_storage_pg::CATALOG_SQL
-- (same tables, columns and constraints; extra: improv.category_binding).
CREATE TABLE improv.model (
    id        bigserial PRIMARY KEY,
    name      text NOT NULL UNIQUE,
    sync_mode text NOT NULL DEFAULT 'logical'
              CHECK (sync_mode IN ('logical', 'trigger', 'manual')),
    slot_name text,
    created   timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE improv.category (
    model_id bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id       bigint NOT NULL,
    name     text NOT NULL,
    PRIMARY KEY (model_id, id)
);
CREATE TABLE improv.item (
    model_id    bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id          bigint NOT NULL,
    category_id bigint NOT NULL,
    name        text NOT NULL,
    PRIMARY KEY (model_id, id)
);
CREATE TABLE improv.measure (
    model_id    bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id          bigint NOT NULL,
    name        text NOT NULL,
    value_type  text NOT NULL,
    kind        text NOT NULL CHECK (kind IN ('input', 'derived')),
    formula     text,
    description text,
    categories  bigint[] NOT NULL DEFAULT '{}',
    PRIMARY KEY (model_id, id)
);
CREATE TABLE improv.binding (
    model_id   bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    measure_id bigint NOT NULL,
    rel        regclass NOT NULL,
    value_col  text NOT NULL,
    dim_cols   jsonb NOT NULL,
    PRIMARY KEY (model_id, measure_id)
);
CREATE TABLE improv.cell (
    model_id   bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    measure_id bigint NOT NULL,
    coord      jsonb NOT NULL,
    value      jsonb NOT NULL,
    PRIMARY KEY (model_id, measure_id, coord)
);
CREATE TABLE improv.view (
    model_id bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id       bigint NOT NULL,
    name     text NOT NULL,
    layout   jsonb NOT NULL,
    PRIMARY KEY (model_id, id)
);
CREATE TABLE improv.meta (
    model_id bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    blob     jsonb NOT NULL
);
CREATE TABLE improv.model_version (
    model_id bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    tx       bigint NOT NULL DEFAULT 0
);

-- Extension-only: which column of which relation supplies a bound category.
CREATE TABLE improv.category_binding (
    model_id    bigint NOT NULL,
    category_id bigint NOT NULL,
    rel         regclass NOT NULL,
    col         text NOT NULL,
    PRIMARY KEY (model_id, category_id, rel),
    FOREIGN KEY (model_id, category_id) REFERENCES improv.category ON DELETE CASCADE
);
"#,
    name = "catalog",
    bootstrap
);

extension_sql!(
    r#"
-- Shared with storage_pg (design §9.2). Here plain CREATE: an extension owns
-- these, and IF NOT EXISTS / OR REPLACE could adopt a hostile role's object
-- (pgspot PS014/PS002). storage_pg keeps the idempotent forms.
ALTER TABLE improv.binding ADD COLUMN IF NOT EXISTS agg text
  CHECK (agg IN ('sum', 'avg', 'min', 'max', 'count'));
CREATE UNIQUE INDEX item_name_uniq ON improv.item (model_id, category_id, name);
CREATE FUNCTION improv.mint_items(p_model bigint, p_cats bigint[], p_names text[])
RETURNS TABLE (category_id bigint, name text, id bigint) LANGUAGE plpgsql
SET search_path = pg_catalog, pg_temp AS $$
BEGIN
  PERFORM 1 FROM improv.model m WHERE m.id = p_model FOR UPDATE;
  INSERT INTO improv.item (model_id, id, category_id, name)
  SELECT p_model,
         (SELECT coalesce(max(i.id), 0) FROM improv.item i WHERE i.model_id = p_model)
           + row_number() OVER (ORDER BY s.c, s.n),
         s.c, s.n
  FROM (SELECT DISTINCT u.c, u.n FROM unnest(p_cats, p_names) AS u(c, n)
        WHERE NOT EXISTS (SELECT 1 FROM improv.item i
                          WHERE i.model_id = p_model AND i.category_id = u.c AND i.name = u.n)) s
  ON CONFLICT DO NOTHING;
  RETURN QUERY
    SELECT i.category_id, i.name, i.id FROM improv.item i
    JOIN (SELECT DISTINCT u.c, u.n FROM unnest(p_cats, p_names) AS u(c, n)) w
      ON i.model_id = p_model AND i.category_id = w.c AND i.name = w.n;
END $$;
"#,
    name = "phase2_catalog",
    requires = ["catalog"]
);

// The catalog's rows are the user's models, not the extension's: without
// this, pg_dump writes only the extension's DDL and a restore loses every
// model (its tables come back empty from CREATE EXTENSION). The sequence
// behind improv.model.id is dumped too, so restored ids keep minting.
extension_sql!(
    r#"
SELECT pg_catalog.pg_extension_config_dump('improv.model', '');
SELECT pg_catalog.pg_extension_config_dump('improv.model_id_seq', '');
SELECT pg_catalog.pg_extension_config_dump('improv.category', '');
SELECT pg_catalog.pg_extension_config_dump('improv.item', '');
SELECT pg_catalog.pg_extension_config_dump('improv.measure', '');
SELECT pg_catalog.pg_extension_config_dump('improv.binding', '');
SELECT pg_catalog.pg_extension_config_dump('improv.category_binding', '');
SELECT pg_catalog.pg_extension_config_dump('improv.cell', '');
SELECT pg_catalog.pg_extension_config_dump('improv.view', '');
SELECT pg_catalog.pg_extension_config_dump('improv.meta', '');
SELECT pg_catalog.pg_extension_config_dump('improv.model_version', '');
"#,
    name = "dump_catalog",
    requires = ["phase2_catalog"]
);

/// The id and stored name of model `name`, row-locked so concurrent
/// structural changes and recalcs of one model serialize.
fn lock_model(name: &str) -> Res<(i64, String)> {
    // `update`, not `select`: pgrx runs a `select` read-only until the
    // transaction has an xid, and Postgres rejects a read-only FOR UPDATE.
    Spi::connect_mut(|c| {
        let t = c.update(
            "SELECT id, name FROM improv.model WHERE name = $1 FOR UPDATE",
            None,
            &[name.into()],
        )?;
        match t.into_iter().next() {
            Some(r) => Ok((req::<i64>(&r, 1)?, req::<String>(&r, 2)?)),
            None => Err(format!("improv model {name:?} does not exist").into()),
        }
    })
}

/// Column `i` of `r`, which must not be NULL.
fn req<T: IntoDatum + FromDatum>(r: &pgrx::spi::SpiHeapTupleData, i: usize) -> Res<T> {
    Ok(r.get::<T>(i)?
        .ok_or_else(|| format!("unexpected NULL in column {i}"))?)
}

/// A catalog id as the engine's `u32`.
fn id32(v: i64) -> Res<u32> {
    Ok(u32::try_from(v).map_err(|_| format!("catalog id {v} is out of range"))?)
}

/// Mint ids for the dimension values in `bound` that `m` has no item for,
/// through `improv.mint_items` (the one minter, shared with the GUI's store),
/// and add them to `m`. Minting is append-only: no `model_version` bump.
fn mint_items(mid: i64, m: &mut Model, bound: &[pm::BoundRows]) -> Res<()> {
    let new = pm::unknown_items(m, bound)?;
    if new.is_empty() {
        return Ok(());
    }
    let (cats, names): (Vec<i64>, Vec<String>) =
        new.into_iter().map(|(c, n)| (i64::from(c.0), n)).unzip();
    Spi::connect_mut(|c| {
        let t = c.update(
            "SELECT category_id, name, id FROM improv.mint_items($1, $2::int8[], $3::text[])",
            None,
            &[mid.into(), cats.into(), names.into()],
        )?;
        for r in t {
            let cat = CategoryId(id32(req(&r, 1)?)?);
            m.add_item(ItemId(id32(req(&r, 3)?)?), cat, req::<String>(&r, 2)?);
        }
        Ok(())
    })
}

/// Bump `improv.model_version`: the optimistic-concurrency token the GUI's
/// store checks before every save. Every structural change calls this.
fn bump_version(model_id: i64) -> Res<()> {
    Spi::run_with_args(
        "UPDATE improv.model_version SET tx = tx + 1 WHERE model_id = $1",
        &[model_id.into()],
    )?;
    Ok(())
}

/// Error unless `rel` has a live column `col`.
fn check_column(rel: &PgRelation, col: &str) -> Res<()> {
    let found = Spi::get_one_with_args::<bool>(
        "SELECT EXISTS (SELECT 1 FROM pg_attribute WHERE attrelid = $1 AND attname = $2 \
         AND attnum > 0 AND NOT attisdropped)",
        &[rel.oid().into(), col.into()],
    )?;
    if found == Some(true) {
        Ok(())
    } else {
        Err(format!("relation {} has no column {col:?}", rel.name()).into())
    }
}

/// `rel`'s relkind as a char (`r` table, `p` partitioned, `v` view, `m`
/// materialized view, `f` foreign table).
pub(crate) fn relkind(rel: pg_sys::Oid) -> Res<char> {
    let k = Spi::get_one_with_args::<String>(
        "SELECT relkind::text FROM pg_class WHERE oid = $1",
        &[rel.into()],
    )?;
    Ok(k.and_then(|k| k.chars().next()).unwrap_or('?'))
}

/// Logical and trigger sync read a bound relation's row changes (logical
/// decoding or statement triggers' transition tables), which only tables
/// have. Name the problem instead of letting Postgres refuse an
/// `ALTER TABLE … REPLICA IDENTITY` or a transition-table trigger.
pub(crate) fn check_syncable(rel_name: &str, kind: char, mode: &str) -> Res<()> {
    if mode == "manual" || matches!(kind, 'r' | 'p') {
        return Ok(());
    }
    let what = match kind {
        'v' => "a view",
        'm' => "a materialized view",
        'f' => "a foreign table",
        _ => "not a table",
    };
    Err(format!(
        "{rel_name} is {what}: {mode} sync follows a table's row changes, so it can bind only \
         tables. Bind a table, or put the model in manual mode \
         (improv.set_sync_mode(model, 'manual')) and refresh with improv.recalc()"
    )
    .into())
}

/// `rel.col`'s type: its `pg_type.typname` and `format_type`.
fn column_type(rel: &PgRelation, col: &str) -> Res<(String, String)> {
    Spi::connect(|c| -> Res<_> {
        let r = c
            .select(
                "SELECT t.typname::text, format_type(a.atttypid, a.atttypmod) \
                 FROM pg_attribute a JOIN pg_type t ON t.oid = a.atttypid \
                 WHERE a.attrelid = $1 AND a.attname = $2 AND a.attnum > 0 \
                   AND NOT a.attisdropped",
                None,
                &[rel.oid().into(), col.into()],
            )?
            .next()
            .ok_or_else(|| format!("relation {} has no column {col:?}", rel.name()))?;
        Ok((req(&r, 1)?, req(&r, 2)?))
    })
}

/// Column `i` of `r`, read as [`pm::select_bound_sql`] /
/// [`pm::select_result_sql`] select a `vt` value (`None`: NULL).
fn get_value(r: &pgrx::spi::SpiHeapTupleData, i: usize, vt: ValueType) -> Res<Option<CellValue>> {
    Ok(match vt {
        ValueType::Number => r.get::<f64>(i)?.map(CellValue::num),
        ValueType::Boolean => r.get::<bool>(i)?.map(CellValue::Bool),
        ValueType::Text => r.get::<String>(i)?.map(CellValue::Text),
        ValueType::DateTime => r.get::<i64>(i)?.map(CellValue::Date),
        ValueType::Enum => return Err("enum measures are not supported".into()),
    })
}

/// Create result table `table` of model `model` for a `vt` measure (and the
/// result schema) if missing; refuse an existing one whose `value` column
/// holds another type.
fn ensure_table(model: &str, table: &str, dims: &[String], vt: ValueType) -> Res<()> {
    // Checked first: `IF NOT EXISTS` would NOTICE on every statement.
    let found = Spi::get_one_with_args::<String>(
        "SELECT (SELECT format_type(atttypid, atttypmod) FROM pg_attribute \
         WHERE attrelid = to_regclass($1) AND attname = 'value' AND NOT attisdropped)",
        &[table.into()],
    )?;
    if let Some(t) = found {
        return Ok(pm::check_value_column(table, vt, &t)?);
    }
    let schema = pm::result_schema(model);
    let have = Spi::get_one_with_args::<bool>(
        "SELECT to_regnamespace(quote_ident($1)) IS NOT NULL",
        &[schema.as_str().into()],
    )?;
    if have != Some(true) {
        Spi::run(&format!("CREATE SCHEMA {}", pm::quote_ident(&schema)))?;
    }
    Spi::run(&pm::create_table_sql(table, dims, vt)?)?;
    Ok(())
}

/// Run the statements that turn result rows `old` of a `vt` measure into
/// `new`; returns the rows changed.
fn write_results(
    table: &str,
    dims: &[String],
    vt: ValueType,
    old: &pm::ResultRows,
    new: &pm::ResultRows,
) -> Res<i64> {
    let mut changed = 0;
    for st in pm::diff_statements(table, dims, vt, old, new)? {
        let mut args: Vec<DatumWithOid> = st.dims.iter().map(|d| d.clone().into()).collect();
        match &st.values {
            Some(pm::Values::Number(v)) => args.push(v.clone().into()),
            Some(pm::Values::Text(v)) => args.push(v.clone().into()),
            None => {}
        }
        Spi::run_with_args(&st.sql, &args)?;
        changed += st.rows() as i64;
    }
    Ok(changed)
}

/// The next free id in an `improv.*` table keyed by `(model_id, id)`.
fn next_id(table: &str, model_id: i64) -> Res<i64> {
    let sql = format!("SELECT coalesce(max(id), 0) + 1 FROM improv.{table} WHERE model_id = $1");
    Ok(Spi::get_one_with_args::<i64>(&sql, &[model_id.into()])?.unwrap_or(1))
}

fn read_catalog(model_id: i64) -> Res<pm::Catalog> {
    Spi::connect(|c| {
        let mut cat = pm::Catalog::default();
        let args = [model_id.into()];
        for r in c.select(
            "SELECT id, name FROM improv.category WHERE model_id = $1",
            None,
            &args,
        )? {
            cat.categories.push(pm::CategoryRow {
                id: id32(req(&r, 1)?)?,
                name: req(&r, 2)?,
            });
        }
        for r in c.select(
            "SELECT id, category_id, name FROM improv.item WHERE model_id = $1",
            None,
            &args,
        )? {
            cat.items.push(pm::ItemRow {
                id: id32(req(&r, 1)?)?,
                category_id: id32(req(&r, 2)?)?,
                name: req(&r, 3)?,
            });
        }
        for r in c.select(
            "SELECT id, name, value_type, kind, formula, description, categories \
             FROM improv.measure WHERE model_id = $1",
            None,
            &args,
        )? {
            cat.measures.push(pm::MeasureRow {
                id: id32(req(&r, 1)?)?,
                name: req(&r, 2)?,
                value_type: req(&r, 3)?,
                kind: req(&r, 4)?,
                formula: r.get(5)?,
                description: r.get(6)?,
                categories: req::<Vec<i64>>(&r, 7)?
                    .into_iter()
                    .map(id32)
                    .collect::<Res<_>>()?,
            });
        }
        Ok(cat)
    })
}

/// Create a model and its result schema `improv_<lower(name)>`.
#[pg_extern(security_invoker)]
fn create_model(name: &str) -> Res<i64> {
    pm::validate_name("model", name)?;
    let id = Spi::get_one_with_args::<i64>(
        "INSERT INTO improv.model (name) VALUES ($1) RETURNING id",
        &[name.into()],
    )?
    .ok_or("INSERT returned no id")?;
    Spi::run_with_args(
        // Born at tx 1: creation is the model's first structural change.
        "INSERT INTO improv.model_version (model_id, tx) VALUES ($1, 1)",
        &[id.into()],
    )?;
    Spi::run(&format!(
        "CREATE SCHEMA {}",
        pm::quote_ident(&pm::result_schema(name))
    ))?;
    Ok(id)
}

/// Bind category `category` (created if missing) to column `col` of `rel`.
#[pg_extern(security_invoker)]
fn bind_category(model: &str, category: &str, rel: PgRelation, col: &str) -> Res<()> {
    pm::validate_name("category", category)?;
    let (mid, _) = lock_model(model)?;
    check_column(&rel, col)?;
    let existing = Spi::get_one_with_args::<i64>(
        // A scalar subquery: always one row (`get_one` errors on zero rows).
        "SELECT (SELECT id FROM improv.category WHERE model_id = $1 AND lower(name) = lower($2))",
        &[mid.into(), category.into()],
    )?;
    let cid = match existing {
        Some(cid) => cid,
        None => {
            let cid = next_id("category", mid)?;
            Spi::run_with_args(
                "INSERT INTO improv.category (model_id, id, name) VALUES ($1, $2, $3)",
                &[mid.into(), cid.into(), category.into()],
            )?;
            cid
        }
    };
    Spi::run_with_args(
        "INSERT INTO improv.category_binding (model_id, category_id, rel, col) \
         VALUES ($1, $2, $3::oid::regclass, $4) \
         ON CONFLICT (model_id, category_id, rel) DO UPDATE SET col = EXCLUDED.col",
        &[mid.into(), cid.into(), rel.oid().into(), col.into()],
    )?;
    bump_version(mid)
}

/// Bind input measure `measure` to `rel.value_col`. Its dimensions are the
/// categories already bound on `rel`.
#[pg_extern(security_invoker)]
fn bind_measure(
    model: &str,
    measure: &str,
    rel: PgRelation,
    value_col: &str,
    agg: default!(Option<&str>, "NULL"),
) -> Res<()> {
    pm::validate_name("measure", measure)?;
    let (mid, _) = lock_model(model)?;
    let mode = Spi::get_one_with_args::<String>(
        "SELECT sync_mode FROM improv.model WHERE id = $1",
        &[mid.into()],
    )?
    .unwrap_or_default();
    check_syncable(rel.name(), relkind(rel.oid())?, &mode)?;
    let (typname, shown) = column_type(&rel, value_col)?;
    let vt = pm::value_type_name(pm::bound_value_type(&typname, &shown, agg)?);
    let taken = Spi::get_one_with_args::<bool>(
        "SELECT EXISTS (SELECT 1 FROM improv.measure WHERE model_id = $1 AND lower(name) = lower($2))",
        &[mid.into(), measure.into()],
    )?;
    if taken == Some(true) {
        return Err(format!("measure {measure} already exists").into());
    }
    let cats = Spi::get_one_with_args::<Vec<i64>>(
        "SELECT array_agg(category_id ORDER BY category_id) FROM improv.category_binding \
         WHERE model_id = $1 AND rel = $2::oid::regclass",
        &[mid.into(), rel.oid().into()],
    )?
    .ok_or_else(|| format!("bind a category on {} before its measures", rel.name()))?;
    let id = next_id("measure", mid)?;
    Spi::run_with_args(
        "INSERT INTO improv.measure (model_id, id, name, value_type, kind, categories) \
         VALUES ($1, $2, $3, $5, 'input', $4)",
        &[
            mid.into(),
            id.into(),
            measure.into(),
            cats.into(),
            vt.into(),
        ],
    )?;
    Spi::run_with_args(
        "INSERT INTO improv.binding (model_id, measure_id, rel, value_col, dim_cols, agg) \
         SELECT $1, $2, $3::oid::regclass, $4, jsonb_object_agg(c.name, b.col), $5 \
         FROM improv.category_binding b JOIN improv.category c \
           ON c.model_id = b.model_id AND c.id = b.category_id \
         WHERE b.model_id = $1 AND b.rel = $3::oid::regclass",
        &[
            mid.into(),
            id.into(),
            rel.oid().into(),
            value_col.into(),
            agg.into(),
        ],
    )?;
    // `ALTER TABLE … REPLICA IDENTITY` (in enable) refuses while this
    // function's own PgRelation keeps the table open.
    drop(rel);
    let mode = Spi::get_one_with_args::<String>(
        "SELECT sync_mode FROM improv.model WHERE id = $1",
        &[mid.into()],
    )?;
    if mode.as_deref() == Some("logical") {
        let name = Spi::get_one_with_args::<String>(
            "SELECT name FROM improv.model WHERE id = $1",
            &[mid.into()],
        )?
        .unwrap_or_default();
        worker::enable(mid, &name)?;
    }
    bump_version(mid)
}

/// Define a derived measure from `Target = expr` and create its result table.
#[pg_extern(security_invoker)]
fn define(model: &str, definition: &str) -> Res<i64> {
    let (mid, model_name) = lock_model(model)?;
    let mut cat = read_catalog(mid)?;
    let id = next_id("measure", mid)?;
    let row = pm::define(&pm::build_model(&cat)?, definition, id32(id)?)?;
    let cats: Vec<i64> = row.categories.iter().map(|&c| i64::from(c)).collect();
    Spi::run_with_args(
        "INSERT INTO improv.measure \
         (model_id, id, name, value_type, kind, formula, description, categories) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        &[
            mid.into(),
            id.into(),
            row.name.as_str().into(),
            row.value_type.as_str().into(),
            row.kind.as_str().into(),
            row.formula.clone().into(),
            row.description.clone().into(),
            cats.into(),
        ],
    )?;
    let table = pm::result_table(&model_name, &row.name);
    cat.measures.push(row);
    let m = pm::build_model(&cat)?;
    let target = MeasureId(id32(id)?);
    let dims = pm::dim_columns(&m, target)?;
    ensure_table(&model_name, &table, &dims, m.measures[&target].value_type)?;
    bump_version(mid)?;
    Ok(id)
}

/// Recompute every derived measure of `model` now, in the caller's
/// transaction, and return the number of result rows changed.
#[pg_extern(security_invoker)]
fn recalc(model: &str) -> Res<i64> {
    let (mid, name) = lock_model(model)?;
    trigger::recompute_all(mid, &name)
}

/// A result table's current rows.
fn read_results(table: &str, dims: &[String], vt: ValueType) -> Res<pm::ResultRows> {
    let sql = pm::select_result_sql(table, dims, vt)?;
    Spi::connect(|c| {
        let mut rows = pm::ResultRows::new();
        for r in c.select(&sql, None, &[])? {
            let key = (1..=dims.len())
                .map(|i| req::<String>(&r, i))
                .collect::<Result<Vec<_>, _>>()?;
            // A NULL value is left out, so the diff rewrites it.
            if let Some(v) = get_value(&r, dims.len() + 1, vt)? {
                rows.insert(key, v);
            }
        }
        Ok(rows)
    })
}

/// Drop a model: its catalog rows and its result schema.
#[pg_extern(security_invoker)]
fn drop_model(name: &str) -> Res<()> {
    let (mid, stored) = lock_model(name)?;
    // The version row goes with the model; bumping first still row-locks it,
    // so a GUI save in flight (it holds `FOR UPDATE` on it) is serialized.
    bump_version(mid)?;
    trigger::remove_triggers(&stored)?;
    worker::disable(mid, &stored)?;
    Spi::run(&format!(
        "DROP SCHEMA IF EXISTS {} CASCADE",
        pm::quote_ident(&pm::result_schema(&stored))
    ))?;
    Spi::run_with_args("DELETE FROM improv.model WHERE id = $1", &[mid.into()])?;
    Ok(())
}

#[cfg(any(test, feature = "pg_test"))]
#[pg_schema]
mod tests {
    use pgrx::prelude::*;

    /// Q1..Q4 × Widget/Gadget: Widget price 10, qty 100·q; Gadget price 20,
    /// qty 30·q. Revenue[q,Widget] = 1000·q, Revenue[q,Gadget] = 600·q;
    /// Total[Widget] = 10000, Total[Gadget] = 6000.
    fn setup_plan() {
        for sql in [
            "CREATE TABLE sales (quarter text, product text, price float8, qty float8)",
            "INSERT INTO sales
               SELECT 'Q' || q, p, CASE p WHEN 'Widget' THEN 10 ELSE 20 END,
                      q * CASE p WHEN 'Widget' THEN 100 ELSE 30 END
               FROM generate_series(1, 4) q, unnest(ARRAY['Widget', 'Gadget']) p",
            "SELECT improv.create_model('plan')",
            "SELECT improv.bind_category('plan', 'Time', 'sales', 'quarter')",
            "SELECT improv.bind_category('plan', 'Product', 'sales', 'product')",
            "SELECT improv.bind_measure('plan', 'Price', 'sales', 'price')",
            "SELECT improv.bind_measure('plan', 'Quantity', 'sales', 'qty')",
            "SELECT improv.define('plan', 'Revenue = Price * Quantity')",
            "SELECT improv.define('plan', 'Total = SUM(Revenue OVER Time)')",
        ] {
            Spi::run(sql).unwrap();
        }
    }

    fn recalc() -> i64 {
        Spi::get_one::<i64>("SELECT improv.recalc('plan')")
            .unwrap()
            .unwrap()
    }

    fn revenue(q: &str, p: &str) -> Option<f64> {
        Spi::get_one_with_args::<f64>(
            "SELECT (SELECT value FROM improv_plan.revenue WHERE \"time\" = $1 AND product = $2)",
            &[q.into(), p.into()],
        )
        .unwrap()
    }

    fn total(p: &str) -> Option<f64> {
        Spi::get_one_with_args::<f64>(
            "SELECT (SELECT value FROM improv_plan.total WHERE product = $1)",
            &[p.into()],
        )
        .unwrap()
    }

    fn count(table: &str) -> i64 {
        Spi::get_one::<i64>(&format!("SELECT count(*) FROM {table}"))
            .unwrap()
            .unwrap()
    }

    fn by_product(table: &str, p: &str) -> Option<f64> {
        Spi::get_one_with_args::<f64>(
            &format!("SELECT (SELECT value FROM improv_plan.{table} WHERE product = $1)"),
            &[p.into()],
        )
        .unwrap()
    }

    /// G4: `agg` folds duplicate rows per coordinate, in `recalc` and in
    /// trigger mode.
    #[pg_test]
    fn aggregated_bindings_fold_duplicate_rows() {
        for sql in [
            "CREATE TABLE orders (product text, amount float8)",
            "INSERT INTO orders VALUES ('Widget', 10), ('Widget', 30), ('Gadget', 5)",
            "SELECT improv.create_model('plan')",
            "SELECT improv.bind_category('plan', 'Product', 'orders', 'product')",
            "SELECT improv.bind_measure('plan', 'Amount', 'orders', 'amount', agg => 'sum')",
            "SELECT improv.bind_measure('plan', 'Mean', 'orders', 'amount', agg => 'avg')",
            "SELECT improv.define('plan', 'Total = Amount * 1')",
            "SELECT improv.define('plan', 'Average = Mean * 1')",
        ] {
            Spi::run(sql).unwrap();
        }
        assert_eq!(recalc(), 4);
        assert_eq!(by_product("total", "Widget"), Some(40.0));
        assert_eq!(by_product("average", "Widget"), Some(20.0));
        assert_eq!(by_product("total", "Gadget"), Some(5.0));
        assert_eq!(
            Spi::get_one::<String>("SELECT agg FROM improv.binding WHERE measure_id = 2"),
            Ok(Some("avg".into()))
        );

        Spi::run("SELECT improv.set_sync_mode('plan', 'trigger')").unwrap();
        Spi::run("INSERT INTO orders VALUES ('Widget', 50)").unwrap();
        assert_eq!(by_product("total", "Widget"), Some(90.0));
        assert_eq!(by_product("average", "Widget"), Some(30.0));
        Spi::run("UPDATE orders SET amount = 7 WHERE product = 'Gadget'").unwrap();
        assert_eq!(by_product("total", "Gadget"), Some(7.0));
        Spi::run("DELETE FROM orders WHERE amount = 10").unwrap();
        assert_eq!(by_product("total", "Widget"), Some(80.0));
        assert_eq!(by_product("average", "Widget"), Some(40.0));
    }

    fn text(sql: &str) -> String {
        Spi::get_one::<String>(&format!("SELECT ({sql})::text"))
            .unwrap()
            .unwrap_or_default()
    }

    /// G5: a text input, a derived Boolean and a date input passed through
    /// a derived measure get typed result tables, in `recalc` and trigger
    /// mode.
    #[pg_test]
    fn typed_measures_get_typed_result_tables() {
        for sql in [
            "CREATE TABLE products (product text, category varchar(10), launched date, price numeric)",
            "INSERT INTO products VALUES ('Widget', 'tools', '2026-01-02', 10), \
                                         ('Gadget', 'toys', '2025-06-30', 20)",
            "SELECT improv.create_model('plan')",
            "SELECT improv.bind_category('plan', 'Product', 'products', 'product')",
            "SELECT improv.bind_measure('plan', 'Category', 'products', 'category')",
            "SELECT improv.bind_measure('plan', 'Launched', 'products', 'launched')",
            "SELECT improv.bind_measure('plan', 'Price', 'products', 'price')",
            "SELECT improv.define('plan', 'Pricey = Price > 15')",
            "SELECT improv.define('plan', 'Kind = Category')",
            "SELECT improv.define('plan', 'Launch = Launched')",
        ] {
            Spi::run(sql).unwrap();
        }
        assert_eq!(
            text(
                "SELECT string_agg(name || ':' || value_type, ' ' ORDER BY id) FROM improv.measure"
            ),
            "Category:text Launched:datetime Price:number Pricey:boolean Kind:text Launch:datetime"
        );
        assert_eq!(
            text(
                "SELECT string_agg(attrelid::regclass || '=' || format_type(atttypid, atttypmod), ' ' \
                 ORDER BY attrelid::regclass::text) FROM pg_attribute \
                 WHERE attname = 'value' AND attrelid::regclass::text LIKE 'improv_plan.%'"
            ),
            "improv_plan.kind=text improv_plan.launch=timestamp with time zone \
             improv_plan.pricey=boolean"
        );
        assert_eq!(recalc(), 6);
        let rows = |t: &str| {
            text(&format!(
                "SELECT string_agg(product || '=' || value, ' ' ORDER BY product) FROM improv_plan.{t}"
            ))
        };
        assert_eq!(rows("pricey"), "Gadget=true Widget=false");
        assert_eq!(rows("kind"), "Gadget=toys Widget=tools");
        assert_eq!(
            text(
                "SELECT string_agg(product || '=' || (value = launched::timestamptz), ' ' \
                 ORDER BY product) FROM improv_plan.launch JOIN products USING (product)"
            ),
            "Gadget=true Widget=true"
        );
        assert_eq!(recalc(), 0, "typed values diff as unchanged");
        // what_if takes and reports typed values as JSON.
        assert_eq!(
            text(
                r#"SELECT string_agg(measure || '=' || value::text, ' ' ORDER BY measure)
                   FROM improv.what_if('plan', '[
                     {"measure": "Category", "coord": {"Product": "Widget"}, "value": "gear"},
                     {"measure": "Launched", "coord": {"Product": "Widget"},
                      "value": "2026-03-04T05:06:07.008+01:00"},
                     {"measure": "Price", "coord": {"Product": "Widget"}, "value": 16}]')"#
            ),
            r#"Kind="gear" Launch="2026-03-04T04:06:07.008+00:00" Pricey=true"#
        );

        Spi::run("SELECT improv.set_sync_mode('plan', 'trigger')").unwrap();
        Spi::run("UPDATE products SET price = 30, category = 'gear' WHERE product = 'Widget'")
            .unwrap();
        assert_eq!(rows("pricey"), "Gadget=true Widget=true");
        assert_eq!(rows("kind"), "Gadget=toys Widget=gear");
    }

    #[pg_test(error = "catalog: cannot bind a measure to a column of type jsonb")]
    fn unsupported_column_types_are_refused() {
        for sql in [
            "CREATE TABLE docs (product text, body jsonb)",
            "SELECT improv.create_model('plan')",
            "SELECT improv.bind_category('plan', 'Product', 'docs', 'product')",
            "SELECT improv.bind_measure('plan', 'Body', 'docs', 'body')",
        ] {
            Spi::run(sql).unwrap();
        }
    }

    #[pg_test]
    fn recalc_oracle_and_incremental_update() {
        setup_plan();
        assert_eq!(recalc(), 10, "8 revenue + 2 total rows");
        for q in 1..=4 {
            let t = format!("Q{q}");
            assert_eq!(revenue(&t, "Widget"), Some(1000.0 * q as f64));
            assert_eq!(revenue(&t, "Gadget"), Some(600.0 * q as f64));
        }
        assert_eq!(total("Widget"), Some(10000.0));
        assert_eq!(total("Gadget"), Some(6000.0));
        assert_eq!(recalc(), 0, "nothing changed");

        Spi::run("UPDATE sales SET price = 12 WHERE quarter = 'Q2' AND product = 'Widget'")
            .unwrap();
        assert_eq!(recalc(), 2, "one revenue row + one total row");
        assert_eq!(revenue("Q2", "Widget"), Some(2400.0));
        assert_eq!(revenue("Q1", "Widget"), Some(1000.0));
        assert_eq!(total("Widget"), Some(10400.0));
        assert_eq!(total("Gadget"), Some(6000.0));

        // A vanished fact row deletes its result row.
        Spi::run("DELETE FROM sales WHERE quarter = 'Q4' AND product = 'Gadget'").unwrap();
        assert_eq!(
            recalc(),
            2,
            "one revenue row deleted + Gadget total updated"
        );
        assert_eq!(revenue("Q4", "Gadget"), None);
        assert_eq!(total("Gadget"), Some(3600.0));
        assert_eq!(count("improv_plan.revenue"), 7);
    }

    #[pg_test]
    fn rolled_back_recalc_leaves_results_unchanged() {
        setup_plan();
        assert_eq!(recalc(), 10);
        // The DO block's exception handler is a subtransaction: the UPDATE
        // and the recalc's writes inside it are rolled back together.
        Spi::run(
            "DO $$ BEGIN
               UPDATE sales SET price = 99 WHERE quarter = 'Q1' AND product = 'Widget';
               PERFORM improv.recalc('plan');
               IF (SELECT value FROM improv_plan.revenue
                   WHERE \"time\" = 'Q1' AND product = 'Widget') <> 9900 THEN
                 RAISE EXCEPTION 'recalc did not apply inside the subtransaction';
               END IF;
               RAISE EXCEPTION USING ERRCODE = 'P0099', MESSAGE = 'roll back';
             EXCEPTION WHEN SQLSTATE 'P0099' THEN NULL;
             END $$",
        )
        .unwrap();
        assert_eq!(revenue("Q1", "Widget"), Some(1000.0));
        assert_eq!(total("Widget"), Some(10000.0));
        assert_eq!(recalc(), 0, "results still match the (unchanged) source");
    }

    #[pg_test]
    fn catalog_rows_and_stable_items() {
        setup_plan();
        recalc();
        let items = || {
            Spi::get_one::<String>(
                "SELECT string_agg(id || ':' || name, ',' ORDER BY id) FROM improv.item",
            )
            .unwrap()
            .unwrap()
        };
        let before = items();
        assert_eq!(before, "1:Q1,2:Q2,3:Q3,4:Q4,5:Gadget,6:Widget");
        Spi::run("INSERT INTO sales VALUES ('Q1', 'Doohickey', 1, 1)").unwrap();
        recalc();
        assert_eq!(items(), format!("{before},7:Doohickey"));
        assert_eq!(
            Spi::get_one::<String>(
                "SELECT dim_cols::text FROM improv.binding WHERE measure_id = 1"
            )
            .unwrap()
            .unwrap(),
            r#"{"Time": "quarter", "Product": "product"}"#
        );
        assert_eq!(
            Spi::get_one::<String>(
                "SELECT string_agg(name || '=' || categories::text, ' ' ORDER BY id) \
                 FROM improv.measure"
            )
            .unwrap()
            .unwrap(),
            "Price={1,2} Quantity={1,2} Revenue={1,2} Total={2}"
        );
        // create + 2 bind_category + 2 bind_measure + 2 define = 7; the two
        // recalcs above minted items through improv.mint_items, which is
        // append-only and does not bump the version.
        assert_eq!(count("improv.model_version WHERE tx = 7"), 1);
        recalc();
        assert_eq!(count("improv.model_version WHERE tx = 7"), 1);
        assert_eq!(count("improv.item"), 7, "item_name_uniq: nothing re-minted");
    }

    /// The extension and the GUI's store mint through the same function, so
    /// they agree on ids whichever mints a value first.
    #[pg_test]
    fn extension_and_store_agree_on_item_ids() {
        setup_plan();
        recalc();
        // storage_pg's statement, verbatim, for a value no recalc saw yet.
        let store = |cat: i64, name: &str| {
            Spi::get_one_with_args::<i64>(
                "SELECT id FROM improv.mint_items(\
                   (SELECT id FROM improv.model), $1::int8[], $2::text[])",
                &[vec![cat].into(), vec![name.to_string()].into()],
            )
            .unwrap()
            .unwrap()
        };
        let doohickey = store(2, "Doohickey");
        assert_eq!(doohickey, 7);
        Spi::run("INSERT INTO sales VALUES ('Q5', 'Doohickey', 1, 1)").unwrap();
        recalc();
        assert_eq!(
            item_id("Doohickey"),
            doohickey,
            "recalc reused the store's id"
        );
        assert_eq!(
            store(1, "Q5"),
            item_id("Q5"),
            "the store reuses recalc's id"
        );
        assert_eq!(item_id("Q5"), 8);
        assert_eq!(count("improv.item"), 8);
    }

    #[pg_test]
    fn errors_are_reported() {
        assert!(crate::create_model("bad-name").is_err());
        setup_plan();
        let err = |r: Result<i64, Box<dyn std::error::Error>>| r.unwrap_err().to_string();
        assert!(err(crate::define("plan", "X = CALL(f, Price)")).contains("not supported"));
        assert!(err(crate::define("nope", "X = Price * 2")).contains("does not exist"));

        Spi::run("SET improv.max_cells = 15").unwrap();
        let e = crate::recalc("plan").unwrap_err().to_string();
        assert_eq!(e, "16 bound input cells exceed improv.max_cells = 15");
        Spi::run("RESET improv.max_cells").unwrap();

        Spi::run("INSERT INTO sales VALUES ('Q1', 'Widget', 1, 1)").unwrap();
        let e = crate::recalc("plan").unwrap_err().to_string();
        assert!(e.starts_with("duplicate coordinate for measure"), "{e}");
    }

    /// A model shaped like the GUI's store writes it: an unbound input
    /// measure with `improv.cell` rows, and a derived measure inserted
    /// directly (no result table yet) — recalc handles both.
    #[pg_test]
    fn gui_shaped_catalog_rows() {
        setup_plan();
        recalc();
        let gadget = item_id("Gadget");
        let widget = item_id("Widget");
        Spi::run(&format!(
            "INSERT INTO improv.measure (model_id, id, name, value_type, kind, categories)
               SELECT id, 10, 'Discount', 'number', 'input', '{{2}}' FROM improv.model;
             INSERT INTO improv.cell (model_id, measure_id, coord, value)
               SELECT id, 10, '{{\"2\": {widget}}}', '100' FROM improv.model;
             INSERT INTO improv.measure
               (model_id, id, name, value_type, kind, formula, description, categories)
               SELECT id, 11, 'Net', 'number', 'derived', 'Net = Discount - Revenue',
                      'revenue after discount', '{{1,2}}' FROM improv.model"
        ))
        .unwrap();
        // Smaller dim on the left: improv_engine drops broadcast results
        // when the larger operand is on the left (reported upstream).
        assert_eq!(recalc(), 4, "Net exists only where Discount does (Widget)");
        assert_eq!(
            Spi::get_one::<f64>(
                "SELECT value FROM improv_plan.net WHERE \"time\" = 'Q3' AND product = 'Widget'"
            )
            .unwrap(),
            Some(-2900.0)
        );
        assert_eq!(count(&format!("improv.item WHERE id = {gadget}")), 1);
    }

    fn item_id(name: &str) -> i64 {
        Spi::get_one_with_args::<i64>("SELECT id FROM improv.item WHERE name = $1", &[name.into()])
            .unwrap()
            .unwrap()
    }

    /// The extension's catalog is a superset of the one the GUI's store
    /// (`improv_storage_pg::CATALOG_SQL`) creates: every column it declares
    /// exists here with the same type, nullability and default.
    #[pg_test]
    fn catalog_is_a_superset_of_storage_pg() {
        let src = include_str!("../../storage_pg/src/lib.rs");
        let needle = "pub const CATALOG_SQL: &str = r#\"";
        let start = src.find(needle).unwrap() + needle.len();
        let sql = &src[start..start + src[start..].find("\"#;").unwrap()];
        Spi::run(&sql.replace("improv.", "improv_ref.").replace(
            "CREATE SCHEMA IF NOT EXISTS improv;",
            "CREATE SCHEMA improv_ref;",
        ))
        .unwrap();
        let missing = Spi::get_one::<String>(
            "SELECT string_agg(r.table_name || '.' || r.column_name, ', ')
             FROM information_schema.columns r
             LEFT JOIN information_schema.columns e
               ON e.table_schema = 'improv' AND e.table_name = r.table_name
              AND e.column_name = r.column_name AND e.data_type = r.data_type
              AND e.is_nullable = r.is_nullable
              AND coalesce(replace(e.column_default, 'improv.', ''), '')
                = coalesce(replace(r.column_default, 'improv_ref.', ''), '')
             WHERE r.table_schema = 'improv_ref' AND e.column_name IS NULL",
        )
        .unwrap();
        assert_eq!(
            missing, None,
            "storage_pg columns missing or different here"
        );
        assert!(count("information_schema.columns WHERE table_schema = 'improv_ref'") > 30);
        // ...and every index and function it declares, with the same definition.
        let missing = Spi::get_one::<String>(
            "SELECT string_agg(r.def, ', ') FROM (
               SELECT replace(indexdef, 'improv_ref.', 'improv.') def
                 FROM pg_indexes WHERE schemaname = 'improv_ref'
               UNION ALL
               SELECT replace(pg_get_functiondef(p.oid), 'improv_ref.', 'improv.')
                 FROM pg_proc p WHERE p.pronamespace = 'improv_ref'::regnamespace) r
             WHERE r.def NOT IN (
               SELECT indexdef FROM pg_indexes WHERE schemaname = 'improv'
               UNION ALL
               SELECT pg_get_functiondef(p.oid)
                 FROM pg_proc p WHERE p.pronamespace = 'improv'::regnamespace)",
        )
        .unwrap();
        assert_eq!(missing, None, "storage_pg indexes or functions differ here");
        assert_eq!(count("pg_indexes WHERE indexname = 'item_name_uniq'"), 2);
    }

    #[pg_test]
    fn drop_model_removes_everything() {
        setup_plan();
        recalc();
        Spi::run("SELECT improv.drop_model('plan')").unwrap();
        assert_eq!(count("improv.model"), 0);
        assert_eq!(count("improv.measure"), 0);
        assert_eq!(count("pg_namespace WHERE nspname = 'improv_plan'"), 0);
    }

    /// Binding a measure in the default (logical) mode creates the
    /// publication over the bound table (the worker later gives a PK-less
    /// table `REPLICA IDENTITY FULL`) and records sync state; `status` reports it;
    /// `drop_model` removes the publication and queues the slot for the
    /// worker to drop.
    #[pg_test]
    fn logical_mode_publication_lifecycle() {
        setup_plan();
        let pubs = Spi::get_one::<Vec<String>>(
            "SELECT array_agg(tablename::text) FROM pg_publication_tables WHERE pubname = 'improv_plan'",
        )
        .unwrap()
        .unwrap();
        assert_eq!(pubs, vec!["sales".to_string()]);
        let (mode, slot) =
            Spi::get_two::<String, String>("SELECT sync_mode, slot FROM improv.status('plan')")
                .unwrap();
        assert_eq!(mode.as_deref(), Some("logical"));
        assert_eq!(slot.as_deref(), Some("improv_plan"));
        Spi::run("SELECT improv.drop_model('plan')").unwrap();
        let left = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_publication WHERE pubname = 'improv_plan'",
        )
        .unwrap();
        assert_eq!(left, Some(0));
        let queued = Spi::get_one::<i64>(
            "SELECT count(*) FROM improv.slot_to_drop WHERE slot = 'improv_plan'",
        )
        .unwrap();
        assert_eq!(queued, Some(1));
    }

    /// Leaving logical mode drops the publication; returning recreates it.
    #[pg_test]
    fn sync_mode_switch_manages_the_publication() {
        setup_plan();
        let count = || {
            Spi::get_one::<i64>("SELECT count(*) FROM pg_publication WHERE pubname = 'improv_plan'")
                .unwrap()
                .unwrap()
        };
        assert_eq!(count(), 1);
        Spi::run("SELECT improv.set_sync_mode('plan', 'manual')").unwrap();
        assert_eq!(count(), 0);
        Spi::run("SELECT improv.set_sync_mode('plan', 'logical')").unwrap();
        assert_eq!(count(), 1);
    }

    /// The sync worker is registered when preloaded and connected to
    /// `improv.database` (left at its default, `postgres`, in the test
    /// cluster so pgrx can still drop and recreate its own test database).
    #[pg_test]
    fn sync_worker_is_running() {
        let n = Spi::get_one::<i64>(
            "SELECT count(*) FROM pg_stat_activity \
             WHERE backend_type = 'pg_improv sync' AND datname = 'postgres'",
        )
        .unwrap();
        assert_eq!(n, Some(1));
    }
}

/// Required by `cargo pgrx test`.
#[cfg(test)]
pub mod pg_test {
    pub fn setup(_options: Vec<&str>) {}

    #[must_use]
    pub fn postgresql_conf_options() -> Vec<&'static str> {
        // Preloaded, so the L2 sync worker runs in the test cluster too
        // (its polls find no logical-mode model with a slot and do nothing).
        vec![
            "shared_preload_libraries = 'pg_improv'",
            "wal_level = logical",
            "max_replication_slots = 10",
            "max_wal_senders = 10",
        ]
    }
}
