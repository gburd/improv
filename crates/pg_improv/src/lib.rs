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

mod commit;
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
    commit::init();
}

extension_sql!(
    r#"
-- The shared catalog: a compatible superset of improv_storage_pg::CATALOG_SQL
-- (same tables, columns and constraints; extra: improv.category_binding).
CREATE TABLE improv.model (
    id        bigserial PRIMARY KEY,
    name      text NOT NULL UNIQUE,
    sync_mode text NOT NULL DEFAULT 'logical'
              CHECK (sync_mode IN ('logical', 'trigger', 'commit', 'manual')),
    slot_name text,
    created   timestamptz NOT NULL DEFAULT now(),
    -- The role the model acts as: create_model's caller. Every read of its
    -- bound tables and every write of its results runs as this role.
    owner     regrole NOT NULL
              DEFAULT pg_catalog.to_regrole(pg_catalog.quote_ident(CURRENT_USER))
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
  CHECK (agg IN ('sum', 'avg', 'min', 'max', 'count', 'count_distinct'));
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

// Named LAMBDAs (P3-3): `Name = LAMBDA(…)` as given to `define`, called by
// formulas by id (`improv_core_model::functions::named_lambda`).
extension_sql!(
    r#"
CREATE TABLE improv.lambda (
    model_id   bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id         bigint NOT NULL,
    name       text NOT NULL,
    definition text NOT NULL,
    PRIMARY KEY (model_id, id)
);
"#,
    name = "lambda_catalog",
    requires = ["phase2_catalog"]
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
SELECT pg_catalog.pg_extension_config_dump('improv.lambda', '');
"#,
    name = "dump_catalog",
    requires = ["lambda_catalog"]
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
    for (cat, name, id) in mint(mid, pm::unknown_items(m, bound)?)? {
        m.add_item(id, cat, name);
    }
    Ok(())
}

/// Mint `new` `(category, name)` pairs through `improv.mint_items` (ids
/// above the model's highest, in category-then-name order; a name another
/// transaction minted first keeps its id). Returns each item.
fn mint(mid: i64, new: Vec<(CategoryId, String)>) -> Res<Vec<(CategoryId, String, ItemId)>> {
    if new.is_empty() {
        return Ok(Vec::new());
    }
    let (cats, names): (Vec<i64>, Vec<String>) =
        new.into_iter().map(|(c, n)| (i64::from(c.0), n)).unzip();
    Spi::connect_mut(|c| {
        let t = c.update(
            "SELECT category_id, name, id FROM improv.mint_items($1, $2::int8[], $3::text[]) \
             ORDER BY id",
            None,
            &[mid.into(), cats.into(), names.into()],
        )?;
        t.map(|r| {
            Ok((
                CategoryId(id32(req(&r, 1)?)?),
                req::<String>(&r, 2)?,
                ItemId(id32(req(&r, 3)?)?),
            ))
        })
        .collect()
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
        // Absent between installing a newer .so and ALTER EXTENSION UPDATE.
        let lambdas = c
            .select("SELECT to_regclass('improv.lambda') IS NOT NULL", None, &[])?
            .first()
            .get::<bool>(1)?
            == Some(true);
        if lambdas {
            for r in c.select(
                "SELECT id, name, definition FROM improv.lambda WHERE model_id = $1",
                None,
                &args,
            )? {
                cat.lambdas.push(pm::LambdaRow {
                    id: id32(req(&r, 1)?)?,
                    name: req(&r, 2)?,
                    definition: req(&r, 3)?,
                });
            }
        }
        Ok(cat)
    })
}

/// Create a model owned by the calling role (`improv.model.owner`) and its
/// result schema `improv_<lower(name)>`.
#[pg_extern(security_invoker)]
fn create_model(name: &str) -> Res<i64> {
    pm::validate_name("model", name)?;
    // SAFETY: a plain read of the backend's current user.
    let owner = unsafe { pg_sys::GetUserId() };
    let id = Spi::get_one_with_args::<i64>(
        "INSERT INTO improv.model (name, owner) VALUES ($1, $2::oid::regrole) RETURNING id",
        &[name.into(), owner.into()],
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
    if mode == "commit" {
        commit::guard(rel.oid(), rel.name())?;
    }
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
        worker::check_served(&name)?;
        worker::enable(mid, &name)?;
    }
    bump_version(mid)
}

/// Define a derived measure from `Target = expr` and create its result table,
/// or a named LAMBDA from `Name = LAMBDA(p, …, body)`, which formulas then
/// call by name. With `replace`, an existing derived measure (or lambda) gets
/// the new definition instead: the same one changes nothing (no version
/// bump, no row written); a new one recomputes the measure (the lambda's
/// callers) and their dependents at once, in the caller's transaction,
/// writing only the result rows whose value changed. Returns the measure's
/// (or the lambda's) id.
#[pg_extern(security_invoker)]
fn define(model: &str, definition: &str, replace: default!(bool, false)) -> Res<i64> {
    let (mid, model_name) = lock_model(model)?;
    let mut cat = read_catalog(mid)?;
    let built = pm::build_model(&cat)?;
    match pm::define_lambda(&built, definition, replace)? {
        None => {}
        Some(pm::Defined::Lambda { row, callers }) => {
            return store_lambda(mid, &model_name, &row, &callers);
        }
        Some(pm::Defined::Unchanged(id)) => return Ok(i64::from(id)),
        Some(other) => return Err(format!("unexpected {other:?}").into()),
    }
    let id = next_id("measure", mid)?;
    let row = match pm::define_measure(&built, definition, id32(id)?, replace)? {
        pm::Defined::New(row) => row,
        pm::Defined::Unchanged(id) => return Ok(i64::from(id)),
        pm::Defined::Replaced { row, reshaped } => {
            return redefine(mid, &model_name, &row, reshaped);
        }
        other => return Err(format!("unexpected {other:?}").into()),
    };
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

/// Insert (or update) named LAMBDA `row`, then recompute the measures that
/// call it, `callers`, and their dependents.
fn store_lambda(mid: i64, model: &str, row: &pm::LambdaRow, callers: &[MeasureId]) -> Res<i64> {
    let id = i64::from(row.id);
    Spi::run_with_args(
        "INSERT INTO improv.lambda (model_id, id, name, definition) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (model_id, id) DO UPDATE SET name = EXCLUDED.name, \
         definition = EXCLUDED.definition",
        &[
            mid.into(),
            id.into(),
            row.name.as_str().into(),
            row.definition.as_str().into(),
        ],
    )?;
    if !callers.is_empty() {
        let mut l = trigger::load(mid, model.to_string(), true)?;
        let targets = pm::dependents(&l.model, callers);
        trigger::recompute(
            &mut l,
            &pm::Plan {
                targets,
                scope: None,
            },
        )?;
    }
    bump_version(mid)?;
    Ok(id)
}

/// Store `row`'s new formula and recompute it and its dependents (a
/// `reshaped` measure's result table is recreated first).
fn redefine(mid: i64, model: &str, row: &pm::MeasureRow, reshaped: bool) -> Res<i64> {
    let id = i64::from(row.id);
    let cats: Vec<i64> = row.categories.iter().map(|&c| i64::from(c)).collect();
    Spi::run_with_args(
        "UPDATE improv.measure SET formula = $3, value_type = $4, categories = $5 \
         WHERE model_id = $1 AND id = $2",
        &[
            mid.into(),
            id.into(),
            row.formula.clone().into(),
            row.value_type.as_str().into(),
            cats.into(),
        ],
    )?;
    if reshaped {
        pm::validate_name("measure", &row.name)?;
        let table = pm::result_table(model, &row.name);
        Spi::run(&format!("DROP TABLE IF EXISTS {table}"))?;
    }
    recompute_from(mid, model, MeasureId(row.id))?;
    bump_version(mid)?;
    Ok(id)
}

/// Recompute `root` and every measure that depends on it, now, in the
/// caller's transaction (any sync mode). Returns the rows changed.
fn recompute_from(mid: i64, model: &str, root: MeasureId) -> Res<i64> {
    let mut l = trigger::load(mid, model.to_string(), true)?;
    let targets = pm::dependents(&l.model, &[root]);
    trigger::recompute(
        &mut l,
        &pm::Plan {
            targets,
            scope: None,
        },
    )
}

/// Drop measure `measure` from `model`: its catalog row, its binding and
/// stored cells (an input), or its result table (a derived measure). An
/// ERROR names the measures that still reference it. A named LAMBDA is
/// dropped the same way, by name, once no measure or lambda calls it.
#[pg_extern(security_invoker)]
fn drop_measure(model: &str, measure: &str) -> Res<()> {
    let (mid, model_name) = lock_model(model)?;
    let m = pm::build_model(&read_catalog(mid)?)?;
    if let Some(id) = pm::drop_lambda(&m, measure)? {
        Spi::run_with_args(
            "DELETE FROM improv.lambda WHERE model_id = $1 AND id = $2",
            &[mid.into(), i64::from(id).into()],
        )?;
        return bump_version(mid);
    }
    let target = pm::drop_target(&m, measure)?;
    let found = &m.measures[&target];
    let id = i64::from(target.0);
    if found.is_derived() {
        pm::validate_name("measure", &found.name.0)?;
        let table = pm::result_table(&model_name, &found.name.0);
        Spi::run(&format!("DROP TABLE IF EXISTS {table}"))?;
    }
    let unbinds = Spi::get_one_with_args::<i64>(
        "WITH b AS (DELETE FROM improv.binding WHERE model_id = $1 AND measure_id = $2 \
         RETURNING 1) SELECT count(*) FROM b",
        &[mid.into(), id.into()],
    )? == Some(1);
    for table in ["cell", "measure"] {
        let key = if table == "measure" {
            "id"
        } else {
            "measure_id"
        };
        Spi::run_with_args(
            &format!("DELETE FROM improv.{table} WHERE model_id = $1 AND {key} = $2"),
            &[mid.into(), id.into()],
        )?;
    }
    let mode = Spi::get_one_with_args::<String>(
        "SELECT sync_mode FROM improv.model WHERE id = $1",
        &[mid.into()],
    )?;
    if unbinds && mode.as_deref() == Some("logical") {
        // Publish only the relations still bound.
        worker::enable(mid, &model_name)?;
    }
    bump_version(mid)
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
        assert!(err(crate::define("plan", "X = CALL(f, Price)", false)).contains("not supported"));
        assert!(err(crate::define("nope", "X = Price * 2", false)).contains("does not exist"));

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
    fn create_model_records_its_owner() {
        setup_plan();
        let owner = Spi::get_one::<bool>(
            "SELECT owner::oid = (SELECT oid FROM pg_roles WHERE rolname = current_user) \
             FROM improv.model WHERE name = 'plan'",
        );
        assert_eq!(owner, Ok(Some(true)));
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

    /// `(table, key, ctid)` of every row of the plan's result tables: an
    /// UPDATE moves a row to a new ctid.
    fn ctids() -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        for t in ["revenue", "total", "double"] {
            let rows = Spi::connect(|c| {
                c.select(
                    &format!(
                        "SELECT (to_jsonb(r) - 'value')::text, ctid::text FROM improv_plan.{t} r"
                    ),
                    None,
                    &[],
                )
                .unwrap()
                .map(|r| {
                    (
                        r.get::<String>(1).unwrap().unwrap(),
                        r.get::<String>(2).unwrap().unwrap(),
                    )
                })
                .collect::<Vec<_>>()
            });
            for (k, c) in rows {
                out.insert(format!("{t} {k}"), c);
            }
        }
        out
    }

    fn version() -> i64 {
        Spi::get_one::<i64>("SELECT tx FROM improv.model_version")
            .unwrap()
            .unwrap()
    }

    /// P1-1 in sync mode `mode`: the same `define(…, replace => true)` is a
    /// no-op; a new formula rewrites only the rows whose value changed, at
    /// once, in this transaction.
    fn redefine_in(mode: &str) {
        setup_plan();
        Spi::run("SELECT improv.define('plan', 'Double = Revenue * 2')").unwrap();
        Spi::run_with_args("SELECT improv.set_sync_mode('plan', $1)", &[mode.into()]).unwrap();
        recalc();
        let (v0, rows0) = (version(), ctids());
        assert_eq!(rows0.len(), 8 + 2 + 8);
        let sql = Spi::get_one::<i64>(
            "SELECT improv.define('plan', 'Total = SUM(Revenue OVER Time)', replace => true)",
        );
        assert_eq!(sql, Ok(Some(4)));
        for same in [
            "Total = SUM(Revenue OVER Time)",
            "Total=SUM( Revenue  OVER Time )",
        ] {
            let id = crate::define("plan", same, true).unwrap();
            assert_eq!(id, 4);
            assert_eq!((version(), ctids()), (v0, rows0.clone()), "{mode}: a no-op");
        }
        // Only Gadget's price is above 15: its rows change, Widget's do not.
        crate::define(
            "plan",
            "Revenue = Price * Quantity * IF(Price > 15, 2, 1)",
            true,
        )
        .unwrap();
        assert!(version() > v0, "{mode}: a real change bumps the version");
        assert_eq!(revenue("Q2", "Gadget"), Some(2400.0), "{mode}");
        assert_eq!(revenue("Q2", "Widget"), Some(2000.0), "{mode}");
        assert_eq!(
            (total("Gadget"), total("Widget")),
            (Some(12000.0), Some(10000.0))
        );
        let rows1 = ctids();
        let moved: Vec<&String> = rows0
            .iter()
            .filter(|(k, c)| rows1.get(*k) != Some(*c))
            .map(|(k, _)| k)
            .collect();
        assert_eq!(moved.len(), 4 + 1 + 4, "{mode}: {moved:?}");
        assert!(
            moved.iter().all(|k| k.contains("Gadget")),
            "{mode}: {moved:?}"
        );
        assert_eq!(recalc(), 0, "{mode}: already current");
    }

    #[pg_test]
    fn redefine_manual() {
        redefine_in("manual");
    }

    #[pg_test]
    fn redefine_trigger() {
        redefine_in("trigger");
        // The new formula follows later writes.
        Spi::run("UPDATE sales SET qty = 1 WHERE quarter = 'Q1' AND product = 'Gadget'").unwrap();
        assert_eq!(revenue("Q1", "Gadget"), Some(40.0));
    }

    #[pg_test]
    fn redefine_commit() {
        redefine_in("commit");
        Spi::run("UPDATE sales SET qty = 1 WHERE quarter = 'Q1' AND product = 'Gadget'").unwrap();
        crate::commit::pre_commit();
        assert_eq!(revenue("Q1", "Gadget"), Some(40.0));
    }

    #[pg_test]
    fn redefine_logical() {
        // The worker sees nothing of this uncommitted transaction;
        // tests/logical.sh checks it rewrites nothing after the commit.
        redefine_in("logical");
    }

    /// P1-1 refusals name the dependent; drop_measure removes a measure's
    /// table or its binding.
    #[pg_test]
    fn redefine_and_drop_refusals() {
        setup_plan();
        Spi::run("SELECT improv.define('plan', 'Double = Revenue * 2')").unwrap();
        let err = |r: Result<i64, Box<dyn std::error::Error>>| r.unwrap_err().to_string();
        let e = err(crate::define("plan", "Revenue = Price > 15", true));
        assert!(
            e.contains("from number to boolean") && e.contains("Double, Total read it"),
            "{e}"
        );
        let e = err(crate::define("plan", "Revenue = Total * 1", true));
        assert!(e.contains("Total depends on Revenue"), "{e}");
        let e = err(crate::define(
            "plan",
            "Revenue = SUM(Price OVER Time)",
            true,
        ));
        assert!(e.contains("change the type or dimensions of Double"), "{e}");
        let e = err(crate::define("plan", "Price = Quantity * 2", true));
        assert!(e.contains("input measure"), "{e}");
        let e = err(crate::define("plan", "Revenue = Price", false));
        assert!(
            e.contains("already exists") && e.contains("replace => true"),
            "{e}"
        );
        // A replace of a new name defines it.
        assert_eq!(
            crate::define("plan", "Half = Revenue / 2", true).unwrap(),
            6
        );

        let drop = |m: &str| crate::drop_measure("plan", m).map_err(|e| e.to_string());
        let e = drop("Revenue").unwrap_err();
        assert!(e.contains("Double, Half, Total still reference it"), "{e}");
        assert!(drop("Price")
            .unwrap_err()
            .contains("Revenue still references it"));
        assert!(drop("Nope").unwrap_err().contains("does not exist"));
        for m in ["Double", "half", "Total"] {
            drop(m).unwrap();
        }
        assert_eq!(count("pg_tables WHERE schemaname = 'improv_plan'"), 1);
        // With no reader left, the type may change: the table is recreated.
        crate::define("plan", "Revenue = Price > 15", true).unwrap();
        recalc();
        assert_eq!(
            Spi::get_one::<bool>(
                "SELECT value FROM improv_plan.revenue WHERE \"time\" = 'Q1' AND product = 'Gadget'"
            ),
            Ok(Some(true))
        );
        drop("Revenue").unwrap();
        drop("Price").unwrap();
        assert_eq!(count("improv.binding"), 1, "Price's binding is gone");
        assert_eq!(count("improv.measure"), 1);
        assert_eq!(count("pg_tables WHERE schemaname = 'improv_plan'"), 0);
        // The publication follows the remaining binding; recalc still works.
        assert_eq!(recalc(), 0);
    }

    /// `contrib`: four people's contribution counts. Model `merit` binds
    /// Person and the four counts (Authored is missing for p4).
    fn setup_merit() {
        for sql in [
            "CREATE TABLE contrib (person text, authored float8, reviewed float8,
                                   mails float8, commits float8)",
            "INSERT INTO contrib VALUES ('p1', 300, 10, 50, 7), ('p2', 2, 40, 5, 70),
                                        ('p3', 0, 0, 500, 1), ('p4', NULL, 1, 1, 0)",
            "SELECT improv.create_model('merit')",
            "SELECT improv.bind_category('merit', 'Person', 'contrib', 'person')",
            "SELECT improv.bind_measure('merit', 'Authored', 'contrib', 'authored')",
            "SELECT improv.bind_measure('merit', 'Reviewed', 'contrib', 'reviewed')",
            "SELECT improv.bind_measure('merit', 'Mails', 'contrib', 'mails')",
            "SELECT improv.bind_measure('merit', 'Commits', 'contrib', 'commits')",
            "SELECT improv.set_sync_mode('merit', 'manual')",
        ] {
            Spi::run(sql).unwrap();
        }
    }

    /// Value of `improv_merit.<table>` for `person`.
    fn merit(table: &str, person: &str) -> Option<f64> {
        Spi::get_one_with_args::<f64>(
            &format!("SELECT (SELECT value FROM improv_merit.{table} WHERE person = $1)"),
            &[person.into()],
        )
        .unwrap()
    }

    const NORMS: [(&str, &str); 4] = [
        ("An", "Authored"),
        ("Rn", "Reviewed"),
        ("Mn", "Mails"),
        ("Cn", "Commits"),
    ];

    /// P3-3: `LOGNORM(Authored, Person)` defined once, a body that reduces
    /// over its category parameter, used by four measures; it cannot be
    /// dropped while they call it.
    #[pg_test]
    fn named_lambda_used_by_four_measures() {
        setup_merit();
        let lognorm = "LOGNORM = LAMBDA(x, c, LN1P(x) / LN1P(MAX(x OVER c)))";
        assert_eq!(crate::define("merit", lognorm, false).unwrap(), 1);
        assert_eq!(count("improv.lambda"), 1);
        for (m, input) in NORMS {
            crate::define("merit", &format!("{m} = LOGNORM({input}, Person)"), false).unwrap();
        }
        assert!(recalc_model("merit") > 0);
        let close = |a: Option<f64>, b: f64| a.is_some_and(|a| (a - b).abs() < 1e-12);
        assert!(close(merit("an", "p2"), 2f64.ln_1p() / 300f64.ln_1p()));
        assert!(close(merit("an", "p1"), 1.0));
        assert_eq!(merit("an", "p4"), None, "no Authored, no cell");
        assert!(close(merit("mn", "p1"), 50f64.ln_1p() / 500f64.ln_1p()));
        assert!(close(merit("cn", "p2"), 1.0));
        assert!(close(merit("rn", "p4"), 1f64.ln_1p() / 40f64.ln_1p()));
        // The model's lambdas are visible to the next define, and unchanged.
        let v0 = model_version("merit");
        assert_eq!(crate::define("merit", lognorm, true).unwrap(), 1);
        assert_eq!(model_version("merit"), v0, "an unchanged lambda is a no-op");
        let err = |r: Result<i64, Box<dyn std::error::Error>>| r.unwrap_err().to_string();
        assert!(err(crate::define("merit", lognorm, false)).contains("already exists"));
        assert!(err(crate::define("merit", "Mails = LAMBDA(x, x)", false)).contains("is a measure"));
        assert!(err(crate::define("merit", "LogNorm = Mails * 2", false)).contains("named LAMBDA"));

        let drop = |m: &str| crate::drop_measure("merit", m).map_err(|e| e.to_string());
        let e = drop("lognorm").unwrap_err();
        assert!(
            e.contains("cannot drop LOGNORM: An, Cn, Mn, Rn still call it"),
            "{e}"
        );
        for (m, _) in NORMS {
            drop(m).unwrap();
        }
        drop("LOGNORM").unwrap();
        assert_eq!(count("improv.lambda"), 0);
        assert!(model_version("merit") > v0);
    }

    /// `(table person, ctid)` of every row of the four norm tables.
    fn norm_ctids() -> std::collections::BTreeMap<String, String> {
        let mut out = std::collections::BTreeMap::new();
        for (m, _) in NORMS {
            let t = m.to_lowercase();
            let rows = Spi::connect(|c| {
                c.select(
                    &format!("SELECT person, ctid::text FROM improv_merit.{t}"),
                    None,
                    &[],
                )
                .unwrap()
                .map(|r| {
                    (
                        r.get::<String>(1).unwrap().unwrap(),
                        r.get::<String>(2).unwrap().unwrap(),
                    )
                })
                .collect::<Vec<_>>()
            });
            for (p, c) in rows {
                out.insert(format!("{t} {p}"), c);
            }
        }
        out
    }

    /// P3-3 with P1-1, in sync mode `mode`: redefining LOGNORM rewrites the
    /// four measures that call it at once, in this transaction, and only
    /// the rows whose value changed (a zero stays zero).
    fn lambda_redefine_in(mode: &str) {
        setup_merit();
        crate::define(
            "merit",
            "LOGNORM = LAMBDA(x, c, LN1P(x) / LN1P(MAX(x OVER c)))",
            false,
        )
        .unwrap();
        for (m, input) in NORMS {
            crate::define("merit", &format!("{m} = LOGNORM({input}, Person)"), false).unwrap();
        }
        Spi::run_with_args("SELECT improv.set_sync_mode('merit', $1)", &[mode.into()]).unwrap();
        recalc_model("merit");
        let (v0, rows0) = (model_version("merit"), norm_ctids());
        assert_eq!(rows0.len(), 3 + 4 + 4 + 4, "{mode}");
        let p2: Vec<Option<f64>> = NORMS
            .iter()
            .map(|(m, _)| merit(&m.to_lowercase(), "p2"))
            .collect();
        crate::define(
            "merit",
            "LOGNORM = LAMBDA(x, c, 2 * LN1P(x) / LN1P(MAX(x OVER c)))",
            true,
        )
        .unwrap();
        assert!(model_version("merit") > v0, "{mode}");
        // All four callers follow the new body.
        for ((m, _), was) in NORMS.iter().zip(&p2) {
            let now = merit(&m.to_lowercase(), "p2");
            assert_eq!(now, was.map(|v| 2.0 * v), "{mode} {m}");
        }
        let rows1 = norm_ctids();
        let kept: Vec<&String> = rows0
            .iter()
            .filter(|(k, c)| rows1.get(*k) == Some(*c))
            .map(|(k, _)| k)
            .collect();
        // p3 authored and reviewed nothing, p4 committed nothing: 2 * 0 is
        // still 0, so those rows are not rewritten.
        assert_eq!(kept, ["an p3", "cn p4", "rn p3"], "{mode}");
        assert_eq!(rows1.len(), rows0.len(), "{mode}");
        assert_eq!(recalc_model("merit"), 0, "{mode}: already current");
    }

    #[pg_test]
    fn lambda_redefine_manual() {
        lambda_redefine_in("manual");
    }

    #[pg_test]
    fn lambda_redefine_trigger() {
        lambda_redefine_in("trigger");
        // The new body follows later writes.
        Spi::run("UPDATE contrib SET authored = 3 WHERE person = 'p2'").unwrap();
        let want = 2.0 * 3f64.ln_1p() / 300f64.ln_1p();
        assert!(merit("an", "p2").is_some_and(|v| (v - want).abs() < 1e-12));
    }

    #[pg_test]
    fn lambda_redefine_commit() {
        lambda_redefine_in("commit");
        Spi::run("UPDATE contrib SET authored = 3 WHERE person = 'p2'").unwrap();
        crate::commit::pre_commit();
        let want = 2.0 * 3f64.ln_1p() / 300f64.ln_1p();
        assert!(merit("an", "p2").is_some_and(|v| (v - want).abs() < 1e-12));
    }

    #[pg_test]
    fn lambda_redefine_logical() {
        // tests/logical.sh checks the worker rewrites nothing after it.
        lambda_redefine_in("logical");
    }

    /// A lambda redefinition that would reshape a caller is an ERROR naming
    /// it, and changes nothing.
    #[pg_test]
    fn lambda_redefine_refusals() {
        setup_merit();
        crate::define("merit", "Ratio = LAMBDA(x, c, x / MAX(x OVER c))", false).unwrap();
        crate::define("merit", "Share = Ratio(Mails, Person)", false).unwrap();
        crate::define("merit", "Big = Share > 0.5", false).unwrap();
        let v0 = model_version("merit");
        let err = |t: &str| crate::define("merit", t, true).unwrap_err().to_string();
        let e = err("Ratio = LAMBDA(x, c, x > MAX(x OVER c))");
        assert!(e.contains("change the type or dimensions of Share"), "{e}");
        let e = err("Ratio = LAMBDA(x, c, SUM(x OVER c))");
        assert!(e.contains("change the type or dimensions of Share"), "{e}");
        let e = err("Ratio = LAMBDA(x, LN1P(x))");
        assert!(e.contains("breaks Share"), "{e}");
        assert_eq!(model_version("merit"), v0);
    }

    fn recalc_model(model: &str) -> i64 {
        Spi::get_one_with_args::<i64>("SELECT improv.recalc($1)", &[model.into()])
            .unwrap()
            .unwrap()
    }

    fn model_version(model: &str) -> i64 {
        Spi::get_one_with_args::<i64>(
            "SELECT tx FROM improv.model_version v JOIN improv.model m ON m.id = v.model_id \
             WHERE m.name = $1",
            &[model.into()],
        )
        .unwrap()
        .unwrap()
    }

    /// P3-2: a measure with no categories gets a one-row table of just
    /// `value`, kept current by recalc, trigger and commit mode, and
    /// reported by what_if.
    #[pg_test]
    fn dimensionless_measures_get_one_row_tables() {
        setup_plan();
        Spi::run("SELECT improv.define('plan', 'MaxQ = MAX(Quantity OVER Time, Product)')")
            .unwrap();
        let cols = || {
            Spi::get_one::<String>(
                "SELECT string_agg(attname::text, ',' ORDER BY attnum) FROM pg_attribute \
                 WHERE attrelid = 'improv_plan.maxq'::regclass AND attnum > 0",
            )
            .unwrap()
            .unwrap()
        };
        assert_eq!(cols(), "value");
        let max = || Spi::get_one::<f64>("SELECT (SELECT value FROM improv_plan.maxq)").unwrap();
        assert!(recalc() > 0);
        assert_eq!(max(), Some(400.0));
        assert_eq!(recalc(), 0, "unchanged: nothing rewritten");
        let got = Spi::get_one::<String>(
            r#"SELECT string_agg(measure || ' ' || coord::text || ' ' || value::text, '; ')
               FROM improv.what_if('plan', '[
                 {"measure": "Quantity", "coord": {"Time": "Q4", "Product": "Widget"}, "value": 900}]')
               WHERE measure = 'MaxQ'"#,
        )
        .unwrap();
        assert_eq!(got.as_deref(), Some("MaxQ {} 900"));
        Spi::run("SELECT improv.set_sync_mode('plan', 'trigger')").unwrap();
        Spi::run("UPDATE sales SET qty = 500 WHERE quarter = 'Q2' AND product = 'Gadget'").unwrap();
        assert_eq!(max(), Some(500.0));
        Spi::run("DELETE FROM sales").unwrap();
        assert_eq!(count("improv_plan.maxq"), 0, "no cell, no row");
        Spi::run("SELECT improv.set_sync_mode('plan', 'commit')").unwrap();
        Spi::run("INSERT INTO sales VALUES ('Q1', 'Widget', 1, 7)").unwrap();
        crate::commit::pre_commit();
        assert_eq!(max(), Some(7.0));
        assert_eq!(count("improv_plan.maxq"), 1);
    }

    /// The tables of the shared publication `improv_db`, sorted.
    fn published() -> Vec<String> {
        Spi::get_one::<Vec<String>>(
            "SELECT coalesce(array_agg(tablename::text ORDER BY tablename), '{}') \
             FROM pg_publication_tables WHERE pubname = 'improv_db'",
        )
        .unwrap()
        .unwrap()
    }

    /// Binding a measure in the default (logical) mode adds the bound table
    /// to the database's one publication, `improv_db` (the worker later
    /// gives a PK-less table `REPLICA IDENTITY FULL`), and records sync
    /// state; `status` reports the shared slot; `drop_model` takes the table
    /// out again and leaves no per-model publication or slot behind.
    #[pg_test]
    fn logical_mode_publication_lifecycle() {
        setup_plan();
        assert_eq!(published(), vec!["sales".to_string()]);
        let (mode, slot) =
            Spi::get_two::<String, String>("SELECT sync_mode, slot FROM improv.status('plan')")
                .unwrap();
        assert_eq!(mode.as_deref(), Some("logical"));
        assert_eq!(slot.as_deref(), Some("improv_db"));
        Spi::run("SELECT improv.drop_model('plan')").unwrap();
        assert!(published().is_empty());
        let other =
            Spi::get_one::<i64>("SELECT count(*) FROM pg_publication WHERE pubname <> 'improv_db'")
                .unwrap();
        assert_eq!(other, Some(0));
        assert_eq!(
            Spi::get_one::<i64>("SELECT count(*) FROM improv.slot_to_drop").unwrap(),
            Some(0)
        );
    }

    /// The shared publication is the union over the logical-mode models:
    /// two models on one table publish it once, a model on two tables
    /// publishes both, and a table leaves only when no logical model binds
    /// it (mode switch, drop_measure, drop_model).
    #[pg_test]
    fn shared_publication_is_the_union_of_logical_models() {
        setup_plan();
        for sql in [
            "CREATE TABLE costs (product text PRIMARY KEY, cost float8)",
            "SELECT improv.create_model('two')",
            "SELECT improv.bind_category('two', 'Product', 'sales', 'product')",
            "SELECT improv.bind_category('two', 'Product', 'costs', 'product')",
            "SELECT improv.bind_measure('two', 'Qty', 'sales', 'qty', agg => 'sum')",
            "SELECT improv.bind_measure('two', 'Cost', 'costs', 'cost')",
        ] {
            Spi::run(sql).unwrap();
        }
        let both = vec!["costs".to_string(), "sales".to_string()];
        assert_eq!(published(), both);
        Spi::run("SELECT improv.set_sync_mode('plan', 'manual')").unwrap();
        assert_eq!(published(), both, "two still binds sales");
        Spi::run("SELECT improv.drop_measure('two', 'Qty')").unwrap();
        assert_eq!(published(), vec!["costs".to_string()]);
        Spi::run("SELECT improv.set_sync_mode('plan', 'logical')").unwrap();
        assert_eq!(published(), both);
        Spi::run("SELECT improv.drop_model('two')").unwrap();
        assert_eq!(published(), vec!["sales".to_string()]);
        Spi::run("SELECT improv.set_sync_mode('plan', 'trigger')").unwrap();
        assert!(published().is_empty());
        // Idempotent, and repairs a publication changed by hand.
        Spi::run("ALTER PUBLICATION improv_db ADD TABLE costs").unwrap();
        Spi::run("SELECT improv.sync_publication()").unwrap();
        Spi::run("SELECT improv.sync_publication()").unwrap();
        assert!(published().is_empty());
        Spi::run("DROP PUBLICATION improv_db").unwrap();
        Spi::run("SELECT improv.set_sync_mode('plan', 'logical')").unwrap();
        assert_eq!(published(), vec!["sales".to_string()]);
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
