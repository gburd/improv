//! Persist an Improv `Model` in Postgres, in the `improv.*` catalog that the
//! `pg_improv` extension computes over (design:
//! `docs/reviews/2026-10-14-pg-improv-design.md` §2, §5).
//!
//! [`PgModelStore`] mirrors `improv_storage_mentat::ModelStore` method for
//! method. The model is stored normalized: one row per category, item,
//! measure and view, input cells either in their *bound* fact table (an
//! `improv.binding` row maps a measure to `SELECT dims…, value FROM rel`) or,
//! when unbound, in `improv.cell`. `improv.model_version` is the optimistic
//! concurrency token.
//!
//! Encoding (v1; the extension must read the same):
//! - `measure.formula`: written as serde JSON of the `Formula` AST (the DSL
//!   printer lives in the GUI crate); read as JSON or, failing that, as DSL
//!   text (`Target = expr`, what the extension's `improv.define` writes);
//! - `measure.value_type`: `number | boolean | text | datetime | enum`;
//! - `cell.coord`: `{"<category id>": <item id>, …}`;
//! - cell values: plain JSON scalars (number, bool, string; datetimes as
//!   RFC 3339 strings);
//! - `lambda.definition`: written as serde JSON of the `NamedLambda`; read as
//!   JSON or as `Name = LAMBDA(…)` text (what `improv.define` writes);
//! - `meta.blob`: `{external_fns, external_calls, scenarios, sql_sources}`.
//!   Before `improv.lambda` it also held `lambdas`, still read for a model
//!   with no `improv.lambda` row.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

use improv_core_model::{
    parse_definition, parse_expr, CategoryId, Coordinate, Definition, ExternalCall, ExternalFn,
    Formula, ItemId, Measure, MeasureId, MeasureKind, Model, Name, Scenario, ScenarioId, SqlSource,
    Value, ValueType,
};
use postgres::config::{Host, SslMode};
use postgres::{Client, Config, GenericClient, IsolationLevel, NoTls};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

#[cfg(test)]
mod tests;

/// The catalog this client depends on (design §2), created when the
/// `pg_improv` extension is not installed so a plain Postgres works too.
///
/// Kept in this ONE place so it can be reconciled with the extension's own
/// catalog SQL; nothing else in this crate issues DDL against `improv.*`.
pub const CATALOG_SQL: &str = r#"
CREATE SCHEMA IF NOT EXISTS improv;
CREATE TABLE IF NOT EXISTS improv.model (
    id        bigserial PRIMARY KEY,
    name      text NOT NULL UNIQUE,
    sync_mode text NOT NULL DEFAULT 'logical'
              CHECK (sync_mode IN ('logical', 'trigger', 'commit', 'manual')),
    slot_name text,
    created   timestamptz NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS improv.category (
    model_id bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id       bigint NOT NULL,
    name     text NOT NULL,
    PRIMARY KEY (model_id, id)
);
CREATE TABLE IF NOT EXISTS improv.item (
    model_id    bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id          bigint NOT NULL,
    category_id bigint NOT NULL,
    name        text NOT NULL,
    PRIMARY KEY (model_id, id)
);
CREATE TABLE IF NOT EXISTS improv.measure (
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
CREATE TABLE IF NOT EXISTS improv.binding (
    model_id   bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    measure_id bigint NOT NULL,
    rel        regclass NOT NULL,
    value_col  text NOT NULL,
    dim_cols   jsonb NOT NULL,
    PRIMARY KEY (model_id, measure_id)
);
CREATE TABLE IF NOT EXISTS improv.cell (
    model_id   bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    measure_id bigint NOT NULL,
    coord      jsonb NOT NULL,
    value      jsonb NOT NULL,
    PRIMARY KEY (model_id, measure_id, coord)
);
CREATE TABLE IF NOT EXISTS improv.view (
    model_id bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id       bigint NOT NULL,
    name     text NOT NULL,
    layout   jsonb NOT NULL,
    PRIMARY KEY (model_id, id)
);
CREATE TABLE IF NOT EXISTS improv.meta (
    model_id bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    blob     jsonb NOT NULL
);
CREATE TABLE IF NOT EXISTS improv.model_version (
    model_id bigint PRIMARY KEY REFERENCES improv.model(id) ON DELETE CASCADE,
    tx       bigint NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS improv.lambda (
    model_id   bigint NOT NULL REFERENCES improv.model(id) ON DELETE CASCADE,
    id         bigint NOT NULL,
    name       text NOT NULL,
    definition text NOT NULL,
    PRIMARY KEY (model_id, id)
);
-- Shared with the pg_improv extension (design §9.2): the same DDL in
-- idempotent form (this runs against an existing database); the extension
-- uses plain CREATE. Both pin mint_items' search_path.
ALTER TABLE improv.binding ADD COLUMN IF NOT EXISTS agg text
  CHECK (agg IN ('sum', 'avg', 'min', 'max', 'count'));
CREATE UNIQUE INDEX IF NOT EXISTS item_name_uniq ON improv.item (model_id, category_id, name);
CREATE OR REPLACE FUNCTION improv.mint_items(p_model bigint, p_cats bigint[], p_names text[])
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
"#;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Never produced by this backend; present so the variants match
    /// `improv_storage_mentat::StoreError`.
    #[error("mentat error: {0}")]
    Mentat(String),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("data integrity: {0}")]
    Integrity(String),
    /// Another writer bumped `improv.model_version` since this handle loaded.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("postgres error: {0}")]
    Pg(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

impl From<postgres::Error> for StoreError {
    fn from(e: postgres::Error) -> Self {
        StoreError::Pg(match e.as_db_error() {
            Some(db) => db.message().to_string(),
            None => e.to_string(),
        })
    }
}

const CONFLICT_MSG: &str =
    "the model was changed by another writer since it was loaded; reload and retry";

/// Split `postgres://…?model=<name>[&…]` into the connection string without
/// the `model` parameter and the model name (required).
pub fn split_model_url(url: &str) -> Result<(String, String)> {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let mut model = None;
    let mut rest = Vec::new();
    for p in query.split('&').filter(|p| !p.is_empty()) {
        match p.strip_prefix("model=") {
            Some(m) => model = Some(m.to_string()),
            None => rest.push(p),
        }
    }
    let model = model.filter(|m| !m.is_empty()).ok_or_else(|| {
        StoreError::Pg(
            "a postgres model URL needs ?model=<name> (e.g. postgres://me@db/sales?model=plan)"
                .into(),
        )
    })?;
    let conn = if rest.is_empty() {
        base.to_string()
    } else {
        format!("{base}?{}", rest.join("&"))
    };
    Ok((conn, model))
}

/// Refuse to send credentials in cleartext off-host, or to ask for TLS this
/// build lacks (same policy as `improv_storage_sql::connect_postgres`).
fn check_transport(config: &Config) -> Result<()> {
    let no_tls = |why: &str| {
        Err(StoreError::Pg(format!(
            "TLS isn't built into this build of Improv; {why}"
        )))
    };
    let mode = config.get_ssl_mode();
    if mode == SslMode::Require {
        return no_tls("sslmode=require cannot be honoured (use an SSH tunnel to a loopback port)");
    }
    let local = config.get_hosts().iter().all(|h| match h {
        Host::Tcp(name) => {
            name.eq_ignore_ascii_case("localhost")
                || name
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        #[cfg(unix)]
        Host::Unix(_) => true,
    }) && config.get_hostaddrs().iter().all(|ip| ip.is_loopback());
    if !local && mode != SslMode::Disable {
        return no_tls(
            "refusing a non-loopback host without an explicit sslmode=disable \
             (the password would cross the network in cleartext)",
        );
    }
    Ok(())
}

/// Quote an identifier the way `quote_ident` does (always quoted).
fn ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn id32(v: i64) -> Result<u32> {
    u32::try_from(v).map_err(|_| StoreError::Integrity(format!("id {v} is out of range")))
}

fn vt_name(vt: ValueType) -> &'static str {
    match vt {
        ValueType::Number => "number",
        ValueType::Boolean => "boolean",
        ValueType::Text => "text",
        ValueType::DateTime => "datetime",
        ValueType::Enum => "enum",
    }
}

fn vt_parse(s: &str) -> Result<ValueType> {
    Ok(match s {
        "number" => ValueType::Number,
        "boolean" => ValueType::Boolean,
        "text" => ValueType::Text,
        "datetime" => ValueType::DateTime,
        "enum" => ValueType::Enum,
        other => {
            return Err(StoreError::Integrity(format!(
                "unknown value type {other:?}"
            )))
        }
    })
}

/// A cell value as the JSON scalar stored in `improv.cell.value` and fed to
/// a bound table through `jsonb_populate_recordset`.
fn value_to_json(v: &Value) -> Result<serde_json::Value> {
    Ok(match v {
        // Integral numbers as JSON integers, so an `int` fact column accepts them.
        Value::Number(n) if n.fract() == 0.0 && n.abs() < 9.0e15 => (*n as i64).into(),
        Value::Number(n) => serde_json::json!(n),
        Value::Boolean(b) => (*b).into(),
        Value::Text(s) => s.as_str().into(),
        Value::DateTime(d) => d.to_rfc3339().into(),
        Value::Enum(e) => (*e).into(),
        Value::Error(e) => {
            return Err(StoreError::Integrity(format!(
                "an error value cannot be stored as an input ({e:?})"
            )))
        }
    })
}

fn json_to_value(vt: ValueType, j: &serde_json::Value) -> Result<Value> {
    use serde_json::Value as J;
    Ok(match (vt, j) {
        (ValueType::Number, J::Number(n)) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
        (ValueType::Enum, J::Number(n)) if n.as_u64().is_some_and(|n| n <= u32::MAX as u64) => {
            Value::Enum(n.as_u64().unwrap_or_default() as u32)
        }
        (ValueType::Boolean, J::Bool(b)) => Value::Boolean(*b),
        (ValueType::DateTime, J::String(s)) => match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(dt) => Value::DateTime(dt.with_timezone(&chrono::Utc)),
            Err(_) => Value::Text(s.clone()),
        },
        (ValueType::Text, J::String(s)) => Value::Text(s.clone()),
        (ValueType::Text, other) => Value::Text(other.to_string()),
        _ => {
            return Err(StoreError::Integrity(format!(
                "{j} is not a {} value",
                vt_name(vt)
            )))
        }
    })
}

/// The SQL aggregate a `binding.agg` names.
fn agg_fn(agg: &str) -> Result<&'static str> {
    ["sum", "avg", "min", "max", "count"]
        .into_iter()
        .find(|a| *a == agg)
        .ok_or_else(|| StoreError::Integrity(format!("unknown binding agg {agg:?}")))
}

fn value_type_of(model: &Model, m: MeasureId) -> Result<ValueType> {
    model
        .measures
        .get(&m)
        .map(|m| m.value_type)
        .ok_or_else(|| StoreError::Integrity(format!("cell for unknown measure {}", m.0)))
}

/// A formula stored as DSL text: `Target = expr` (the extension's form) or a
/// bare `expr`.
fn parse_formula_text(
    model: &Model,
    text: &str,
) -> std::result::Result<Formula, improv_core_model::ParseError> {
    match parse_definition(model, text) {
        Ok(Definition::Formula(ft)) => Ok(ft.formula),
        _ => parse_expr(model, text),
    }
}

/// The non-structural parts of a model, as `improv.meta.blob`.
#[derive(serde::Deserialize, Default)]
struct Meta {
    #[serde(default)]
    external_fns: HashMap<String, ExternalFn>,
    #[serde(default)]
    external_calls: HashMap<MeasureId, ExternalCall>,
    #[serde(default)]
    scenarios: HashMap<ScenarioId, Scenario>,
    #[serde(default)]
    sql_sources: HashMap<MeasureId, SqlSource>,
    #[serde(default)]
    lambdas: std::collections::BTreeMap<u32, improv_core_model::NamedLambda>,
}

/// One `improv.binding` row, resolved against the model.
struct Binding {
    measure: MeasureId,
    /// `"schema"."table"`, quoted.
    table: String,
    value_col: String,
    /// `(category, column)`, in `dim_cols` key order.
    dims: Vec<(CategoryId, String)>,
    /// `sum | avg | min | max | count`: rows per coordinate are aggregated
    /// on load and the measure is read-only.
    agg: Option<String>,
}

fn bindings(c: &mut impl GenericClient, model: &Model, mid: i64) -> Result<Vec<Binding>> {
    let rows = c.query(
        "SELECT b.measure_id, n.nspname::text, c.relname::text, b.value_col, b.dim_cols::text, \
                b.agg \
         FROM improv.binding b JOIN pg_class c ON c.oid = b.rel \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE b.model_id = $1 ORDER BY b.measure_id",
        &[&mid],
    )?;
    rows.iter()
        .map(|r| {
            let cols: BTreeMap<String, String> = serde_json::from_str(r.get(4))?;
            let dims = cols
                .into_iter()
                .map(|(cat, col)| {
                    model
                        .category_by_name(&cat)
                        .map(|c| (c.id, col))
                        .ok_or_else(|| {
                            StoreError::Integrity(format!("binding names unknown category {cat:?}"))
                        })
                })
                .collect::<Result<_>>()?;
            Ok(Binding {
                measure: MeasureId(id32(r.get(0))?),
                table: format!("{}.{}", ident(r.get(1)), ident(r.get(2))),
                value_col: r.get(3),
                dims,
                agg: r.get(5),
            })
        })
        .collect()
}

fn version(c: &mut impl GenericClient, mid: i64, lock: bool) -> Result<i64> {
    let sql = if lock {
        "SELECT tx FROM improv.model_version WHERE model_id = $1 FOR UPDATE"
    } else {
        "SELECT tx FROM improv.model_version WHERE model_id = $1"
    };
    Ok(c.query_one(sql, &[&mid])?.get(0))
}

/// What a [`PgModelStore`] last loaded or saved: the base `save_model` diffs
/// against, so a save writes only what this client changed. Carry it across
/// handles with [`PgModelStore::set_snapshot`], as with `loaded_tx`.
#[derive(Debug, Clone)]
pub struct PgSnapshot {
    /// Bound input cells, as read from their fact tables.
    bound: HashMap<(MeasureId, Coordinate), Value>,
    /// Unbound input cells, as read from `improv.cell`.
    cells: HashMap<(MeasureId, Coordinate), Value>,
    /// Row ids per catalog table (`category`, `item`, `measure`, `view`,
    /// `lambda`).
    ids: HashMap<&'static str, HashSet<i64>>,
    /// The named LAMBDAs, to write only the changed ones.
    lambdas: BTreeMap<u32, improv_core_model::NamedLambda>,
    /// Item names, to address a removed bound cell whose item is gone.
    names: HashMap<ItemId, String>,
}

impl PgSnapshot {
    /// `model` as stored; `cells` is its unbound part.
    fn new(model: &Model, cells: HashMap<(MeasureId, Coordinate), Value>) -> Self {
        let bound = model
            .inputs
            .iter()
            .filter(|(k, _)| !cells.contains_key(*k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let ids = HashMap::from([
            (
                "category",
                model.categories.keys().map(|i| i64::from(i.0)).collect(),
            ),
            ("item", model.items.keys().map(|i| i64::from(i.0)).collect()),
            (
                "measure",
                model.measures.keys().map(|i| i64::from(i.0)).collect(),
            ),
            ("view", model.views.keys().map(|i| i64::from(i.0)).collect()),
            (
                "lambda",
                model.lambdas.keys().map(|i| i64::from(*i)).collect(),
            ),
        ]);
        let names = model
            .items
            .values()
            .map(|i| (i.id, i.name.0.clone()))
            .collect();
        PgSnapshot {
            bound,
            cells,
            ids,
            lambdas: model.lambdas.clone(),
            names,
        }
    }
}

/// Delete the `improv.<table>` rows the model dropped: with a snapshot
/// (`loaded`), only those it held, so rows another client added since
/// survive; without one, every row the model lacks.
fn delete_dropped(
    tx: &mut impl GenericClient,
    mid: i64,
    table: &str,
    keep: &[i64],
    loaded: Option<&HashSet<i64>>,
) -> Result<()> {
    let Some(loaded) = loaded else {
        tx.execute(
            &format!("DELETE FROM improv.{table} WHERE model_id = $1 AND id <> ALL($2)"),
            &[&mid, &keep],
        )?;
        return Ok(());
    };
    let keep: HashSet<&i64> = keep.iter().collect();
    let gone: Vec<i64> = loaded
        .iter()
        .filter(|i| !keep.contains(i))
        .copied()
        .collect();
    if !gone.is_empty() {
        tx.execute(
            &format!("DELETE FROM improv.{table} WHERE model_id = $1 AND id = ANY($2)"),
            &[&mid, &gone],
        )?;
    }
    Ok(())
}

/// Write binding `b`'s input cells to its fact table: values only, never DDL
/// or a row delete (a row may hold other measures).
///
/// With a snapshot (`base`) only the cells added, changed or removed since it
/// are written, each guarded by its loaded value: `UPDATE … SET value = <new>
/// WHERE <dims> AND value IS NOT DISTINCT FROM <loaded>` (a removed cell's
/// new value is NULL; an added cell's loaded one is NULL), and an added cell
/// no row holds is `INSERT`ed. A guard matching no row (or several) means the
/// cell changed in the database since the load: [`StoreError::Conflict`].
/// Without a snapshot every cell is written, unchecked.
/// `jsonb_populate_record` casts each JSON value to its column's type.
fn write_bound(
    tx: &mut impl GenericClient,
    model: &Model,
    b: &Binding,
    base: Option<&PgSnapshot>,
) -> Result<()> {
    // Deleting a measure must not wipe its column.
    let Some(measure) = model.measures.get(&b.measure) else {
        return Ok(());
    };
    let snap = base.map(|s| &s.bound);
    // An aggregated measure is never written; without a snapshot there is
    // no telling what changed, so nothing is checked either.
    if b.agg.is_some() && snap.is_none() {
        return Ok(());
    }
    // (coordinate, value now, value as loaded); `None`: no cell.
    let mut diff = Vec::new();
    for (k, v) in &model.inputs {
        if k.0 != b.measure {
            continue;
        }
        let old = snap.and_then(|s| s.get(k));
        if snap.is_none() || old != Some(v) {
            diff.push((&k.1, Some(v), old));
        }
    }
    for (k, v) in snap.into_iter().flatten() {
        if k.0 == b.measure && !model.inputs.contains_key(k) {
            diff.push((&k.1, None, Some(v)));
        }
    }
    if diff.is_empty() {
        return Ok(());
    }
    if b.agg.is_some() {
        return Err(StoreError::Integrity(format!(
            "aggregated measure {} is read-only",
            measure.name.0
        )));
    }
    let json = |v: Option<&Value>| v.map_or(Ok(serde_json::Value::Null), value_to_json);
    let (mut xs, mut labels) = (Vec::new(), Vec::new());
    for (coord, new, old) in &diff {
        let mut n = serde_json::Map::new();
        let mut names = Vec::new();
        for (cat, col) in &b.dims {
            let name = coord
                .get(*cat)
                .and_then(|i| {
                    let gone = || base.and_then(|s| s.names.get(&i)).map(String::as_str);
                    model.items.get(&i).map(|i| i.name.0.as_str()).or_else(gone)
                })
                .ok_or_else(|| {
                    StoreError::Integrity(format!(
                        "a cell of measure {} lacks a {} item",
                        b.measure.0, col
                    ))
                })?;
            n.insert(col.clone(), name.into());
            names.push(name);
        }
        n.insert(b.value_col.clone(), json(*new)?);
        let mut x = serde_json::Map::from_iter([("n".to_string(), n.into())]);
        if base.is_some() {
            let o = serde_json::Map::from_iter([(b.value_col.clone(), json(*old)?)]);
            x.insert("o".into(), o.into());
        }
        xs.push(serde_json::Value::Object(x));
        labels.push(names.join(", "));
    }

    let (t, v) = (&b.table, ident(&b.value_col));
    let on = b
        .dims
        .iter()
        .map(|(_, c)| format!("t.{0} = n.{0}", ident(c)))
        .chain(std::iter::once("true".to_string()))
        .collect::<Vec<_>>()
        .join(" AND ");
    let src = format!(
        "jsonb_array_elements($1::text::jsonb) WITH ORDINALITY e(x, i), \
         LATERAL jsonb_populate_record(NULL::{t}, e.x->'n') n"
    );
    let guard = match base {
        Some(_) => {
            format!("t.{v} IS NOT DISTINCT FROM (jsonb_populate_record(NULL::{t}, e.x->'o')).{v}")
        }
        None => format!("t.{v} IS DISTINCT FROM n.{v}"),
    };
    // Rows each cell's UPDATE matched, by its 1-based ordinal.
    let mut hits = vec![0; diff.len()];
    for r in tx.query(
        &format!("UPDATE {t} t SET {v} = n.{v} FROM {src} WHERE {on} AND {guard} RETURNING e.i"),
        &[&serde_json::to_string(&xs)?],
    )? {
        hits[r.get::<_, i64>(0) as usize - 1] += 1;
    }

    // Added cells the UPDATE missed: INSERT where no row has their
    // dimensions. The data-modifying CTE runs once and the outer SELECT sees
    // the table as before it: the added cells whose row already existed.
    let added: Vec<usize> = (0..diff.len())
        .filter(|&i| hits[i] == 0 && (base.is_none() || diff[i].2.is_none()))
        .collect();
    let mut taken = vec![false; diff.len()];
    if !added.is_empty() {
        let cols: Vec<String> = b
            .dims
            .iter()
            .map(|(_, c)| ident(c))
            .chain(std::iter::once(v.clone()))
            .collect();
        let ncols: Vec<String> = cols.iter().map(|c| format!("n.{c}")).collect();
        let sub: Vec<&serde_json::Value> = added.iter().map(|&i| &xs[i]).collect();
        for r in tx.query(
            &format!(
                "WITH ins AS (INSERT INTO {t} ({}) SELECT {} FROM {src} \
                 WHERE NOT EXISTS (SELECT 1 FROM {t} t WHERE {on})) \
                 SELECT e.i FROM {src} WHERE EXISTS (SELECT 1 FROM {t} t WHERE {on})",
                cols.join(", "),
                ncols.join(", ")
            ),
            &[&serde_json::to_string(&sub)?],
        )? {
            taken[added[r.get::<_, i64>(0) as usize - 1]] = true;
        }
    }
    if base.is_none() {
        return Ok(());
    }
    let lost = (0..diff.len())
        .find(|&i| hits[i] > 1 || (hits[i] == 0 && (diff[i].2.is_some() || taken[i])));
    match lost {
        Some(i) => Err(StoreError::Conflict(format!(
            "bound cell {}[{}] was changed in the database since it was loaded; \
             reload and retry",
            measure.name.0, labels[i]
        ))),
        None => Ok(()),
    }
}

/// A model store over one named model in a Postgres database.
pub struct PgModelStore {
    client: Client,
    model_id: i64,
    /// `improv.model_version.tx` as of this handle's last load or save;
    /// `save_model` refuses to commit if it moved. `None`: unchecked.
    loaded_tx: Option<i64>,
    /// What this handle last loaded or saved; `save_model` writes the diff
    /// against it. `None`: write everything, unchecked.
    loaded: Option<PgSnapshot>,
}

impl PgModelStore {
    /// Connect to `postgres://…?model=<name>` and open (or create) that model.
    /// Creates the `improv.*` catalog ([`CATALOG_SQL`]) if it is missing.
    pub fn open(url: &str) -> Result<Self> {
        let (conn, name) = split_model_url(url)?;
        let config: Config = conn
            .parse()
            .map_err(|_| StoreError::Pg("invalid postgres connection string (redacted)".into()))?;
        check_transport(&config)?;
        // A non-database connect error may echo the DSN; keep only its kind.
        let mut client = config.connect(NoTls).map_err(|e| {
            StoreError::Pg(match e.as_db_error() {
                Some(db) => format!("postgres connect failed: {}", db.message()),
                None => "postgres connect failed (connection error)".into(),
            })
        })?;
        // The newest catalog objects: an older catalog is upgraded in place
        // (CATALOG_SQL is idempotent).
        const INSTALLED: &str =
            "SELECT to_regprocedure('improv.mint_items(bigint, bigint[], text[])') IS NOT NULL \
             AND to_regclass('improv.lambda') IS NOT NULL";
        if !client.query_one(INSTALLED, &[])?.get::<_, bool>(0) {
            // A session lock, so the check below starts a new transaction
            // and sees a catalog another client installed meanwhile: its
            // DDL rerun against that client's writes would deadlock.
            client.execute("SELECT pg_advisory_lock(hashtext('improv.catalog'))", &[])?;
            if !client.query_one(INSTALLED, &[])?.get::<_, bool>(0) {
                client.batch_execute(&format!("BEGIN; {CATALOG_SQL} COMMIT;"))?;
            }
            client.execute("SELECT pg_advisory_unlock(hashtext('improv.catalog'))", &[])?;
        }
        let model_id: i64 = client
            .query_one(
                "WITH ins AS (INSERT INTO improv.model(name) VALUES ($1) \
                 ON CONFLICT (name) DO NOTHING RETURNING id) \
                 SELECT id FROM ins UNION ALL SELECT id FROM improv.model WHERE name = $1",
                &[&name],
            )?
            .get(0);
        client.execute(
            "INSERT INTO improv.model_version(model_id) VALUES ($1) ON CONFLICT DO NOTHING",
            &[&model_id],
        )?;
        Ok(PgModelStore {
            client,
            model_id,
            loaded_tx: None,
            loaded: None,
        })
    }

    /// The model's current `improv.model_version.tx`, read live.
    pub fn tx_high_water(&mut self) -> Result<i64> {
        version(&mut self.client, self.model_id, false)
    }

    /// The version this handle last loaded or saved at (`None` if neither).
    pub fn loaded_tx(&self) -> Option<i64> {
        self.loaded_tx
    }

    /// Adopt `tx` as the base `save_model` checks against; `None` saves
    /// unconditionally.
    pub fn set_loaded_tx(&mut self, tx: Option<i64>) {
        self.loaded_tx = tx;
    }

    /// What this handle last loaded or saved: the base `save_model` diffs
    /// against (`None` if neither).
    pub fn snapshot(&self) -> Option<&PgSnapshot> {
        self.loaded.as_ref()
    }

    /// Adopt `snapshot` (e.g. from the handle that loaded) as the save base;
    /// `None` makes `save_model` write the whole model, unchecked.
    pub fn set_snapshot(&mut self, snapshot: Option<PgSnapshot>) {
        self.loaded = snapshot;
    }

    /// Re-read the model after a [`StoreError::Conflict`].
    pub fn reload_model(&mut self) -> Result<Model> {
        self.load_model()
    }

    /// Read the whole model (structure, unbound cells, bound fact tables) from
    /// one consistent snapshot.
    pub fn load_model(&mut self) -> Result<Model> {
        self.load(None)
    }

    /// Like [`Self::load_model`], but only the cells of `measure_ids` and
    /// their dependency closure. Saving the result through this handle
    /// writes only what changed against what it loaded, so the cells it did
    /// not load are left alone.
    pub fn load_partial(&mut self, measure_ids: &[MeasureId]) -> Result<Model> {
        self.load(Some(measure_ids))
    }

    fn load(&mut self, roots: Option<&[MeasureId]>) -> Result<Model> {
        let mid = self.model_id;
        let mut tx = self
            .client
            .build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()?;
        let tx_now = version(&mut tx, mid, false)?;
        let mut model = Model::new();
        for r in tx.query(
            "SELECT id, name FROM improv.category WHERE model_id = $1 ORDER BY id",
            &[&mid],
        )? {
            model.add_category(CategoryId(id32(r.get(0))?), r.get::<_, String>(1));
        }
        for r in tx.query(
            "SELECT id, category_id, name FROM improv.item WHERE model_id = $1 ORDER BY id",
            &[&mid],
        )? {
            let (id, cat) = (ItemId(id32(r.get(0))?), CategoryId(id32(r.get(1))?));
            model.add_item(id, cat, r.get::<_, String>(2));
        }
        let mut dsl = Vec::new();
        for r in tx.query(
            "SELECT id, name, value_type, kind, formula, description, categories \
             FROM improv.measure WHERE model_id = $1 ORDER BY id",
            &[&mid],
        )? {
            let id = MeasureId(id32(r.get(0))?);
            let kind = match r.get::<_, &str>(3) {
                "derived" => {
                    let f: Option<String> = r.get(4);
                    let f = f.ok_or_else(|| {
                        StoreError::Integrity(format!("derived measure {} has no formula", id.0))
                    })?;
                    match serde_json::from_str(&f) {
                        Ok(f) => MeasureKind::Derived(f),
                        // DSL text (what `improv.define` writes): parsed once
                        // every measure name is known.
                        Err(_) => {
                            dsl.push((id, f));
                            MeasureKind::Input
                        }
                    }
                }
                _ => MeasureKind::Input,
            };
            let categories = r
                .get::<_, Vec<i64>>(6)
                .into_iter()
                .map(|c| id32(c).map(CategoryId))
                .collect::<Result<_>>()?;
            model.add_measure(Measure {
                id,
                name: Name(r.get(1)),
                value_type: vt_parse(r.get(2))?,
                categories,
                kind,
                description: r.get(5),
            });
        }
        if let Some(r) = tx.query_opt(
            "SELECT blob::text FROM improv.meta WHERE model_id = $1",
            &[&mid],
        )? {
            let meta: Meta = serde_json::from_str(r.get(0))?;
            model.external_fns = meta.external_fns;
            model.external_calls = meta.external_calls;
            model.scenarios = meta.scenarios;
            model.sql_sources = meta.sql_sources;
            // Saved before improv.lambda existed; the table's rows win.
            model.lambdas = meta.lambdas;
        }
        let lambdas = tx
            .query(
                "SELECT id, definition FROM improv.lambda WHERE model_id = $1 ORDER BY id",
                &[&mid],
            )?
            .iter()
            .map(|r| Ok((id32(r.get(0))?, r.get::<_, String>(1))))
            .collect::<Result<Vec<_>>>()?;
        // The legacy blob's lambdas are not stored rows: the next save
        // writes them to improv.lambda.
        let legacy = lambdas.is_empty() && !model.lambdas.is_empty();
        if !lambdas.is_empty() {
            model.lambdas.clear();
            model
                .load_lambdas(&lambdas)
                .map_err(StoreError::Integrity)?;
        }
        // Text formulas last: they may call any lambda.
        for (id, text) in dsl {
            let f = parse_formula_text(&model, &text).map_err(|e| {
                StoreError::Integrity(format!("measure {}: bad formula {text:?}: {e}", id.0))
            })?;
            if let Some(m) = model.measures.get_mut(&id) {
                m.kind = MeasureKind::Derived(f);
            }
        }
        for r in tx.query(
            "SELECT layout::text FROM improv.view WHERE model_id = $1",
            &[&mid],
        )? {
            model.add_view(serde_json::from_str(r.get(0))?);
        }

        // Cells: only the closure's, filtered in the database.
        let keep: Option<HashSet<MeasureId>> = roots.map(|r| model.measure_dependency_closure(r));
        let filter: Option<Vec<i64>> = keep
            .as_ref()
            .map(|k| k.iter().map(|m| i64::from(m.0)).collect());
        for r in tx.query(
            "SELECT measure_id, coord::text, value::text FROM improv.cell \
             WHERE model_id = $1 AND ($2::int8[] IS NULL OR measure_id = ANY($2))",
            &[&mid, &filter],
        )? {
            let m = MeasureId(id32(r.get(0))?);
            let vt = value_type_of(&model, m)?;
            let dims: BTreeMap<CategoryId, ItemId> = serde_json::from_str(r.get(1))?;
            let v = json_to_value(vt, &serde_json::from_str(r.get(2))?)?;
            model.set_input(m, Coordinate { dims }, v);
        }
        // The unbound cells as read; the rest of `model.inputs` is bound.
        let cells = model.inputs.clone();

        // Bound cells: dimension values map to items by name. Values with no
        // item are minted by `improv.mint_items` (the one minter, shared with
        // the pg_improv extension) once this snapshot closes: it must see
        // concurrent mints, so it runs read-committed, not in this snapshot.
        let mut names: HashMap<(CategoryId, String), ItemId> = model
            .items
            .values()
            .map(|i| ((i.category, i.name.0.clone()), i.id))
            .collect();
        let bound = bindings(&mut tx, &model, mid)?;
        let mut rows = Vec::new();
        for (bi, b) in bound.iter().enumerate() {
            if keep.as_ref().is_some_and(|k| !k.contains(&b.measure)) {
                continue;
            }
            let vt = value_type_of(&model, b.measure)?;
            let mut cols: Vec<String> = b
                .dims
                .iter()
                .map(|(_, c)| format!("{}::text", ident(c)))
                .collect();
            let mut v = ident(&b.value_col);
            if let Some(a) = b.agg.as_deref() {
                v = format!("{}({v})", agg_fn(a)?);
            }
            // A date/timestamp column as timestamptz: its JSON is RFC 3339.
            if vt == ValueType::DateTime {
                v = format!("({v})::timestamptz");
            }
            cols.push(format!("to_jsonb({v})::text"));
            let sql = match b.agg {
                None => format!("SELECT {} FROM {}", cols.join(", "), b.table),
                Some(_) => {
                    let by: Vec<String> = (1..cols.len()).map(|i| i.to_string()).collect();
                    format!(
                        "SELECT {} FROM {} GROUP BY {}",
                        cols.join(", "),
                        b.table,
                        by.join(", ")
                    )
                }
            };
            'row: for r in tx.query(&sql, &[])? {
                // NULL value: no cell. NULL dimension: the row is ignored.
                let Some(v) = r.get::<_, Option<&str>>(b.dims.len()) else {
                    continue;
                };
                let mut dims = Vec::with_capacity(b.dims.len());
                for (i, (cat, _)) in b.dims.iter().enumerate() {
                    let Some(name) = r.get::<_, Option<String>>(i) else {
                        continue 'row;
                    };
                    dims.push((*cat, name));
                }
                rows.push((bi, dims, json_to_value(vt, &serde_json::from_str(v)?)?));
            }
        }
        tx.commit()?;
        let new: BTreeSet<&(CategoryId, String)> = rows
            .iter()
            .flat_map(|(_, dims, _)| dims)
            .filter(|k| !names.contains_key(*k))
            .collect();
        if !new.is_empty() {
            let (cats, new): (Vec<i64>, Vec<&str>) = new
                .into_iter()
                .map(|(c, n)| (i64::from(c.0), n.as_str()))
                .unzip();
            for r in self.client.query(
                "SELECT category_id, name, id FROM improv.mint_items($1, $2::int8[], $3::text[])",
                &[&mid, &cats, &new],
            )? {
                let (cat, id) = (CategoryId(id32(r.get(0))?), ItemId(id32(r.get(2))?));
                let name: String = r.get(1);
                model.add_item(id, cat, name.clone());
                names.insert((cat, name), id);
            }
        }
        for (bi, dims, v) in rows {
            let b = &bound[bi];
            let mut coord = Coordinate::new();
            for k in &dims {
                let item = names.get(k).ok_or_else(|| {
                    StoreError::Integrity(format!("improv.mint_items returned no id for {:?}", k.1))
                })?;
                coord = coord.with(k.0, *item);
            }
            if model.inputs.insert((b.measure, coord), v).is_some() {
                return Err(StoreError::Integrity(format!(
                    "{} has two rows for one coordinate of measure {}",
                    b.table, b.measure.0
                )));
            }
        }
        self.loaded_tx = Some(tx_now);
        let mut snap = PgSnapshot::new(&model, cells);
        if legacy {
            snap.lambdas.clear();
            snap.ids.insert("lambda", HashSet::new());
        }
        self.loaded = Some(snap);
        Ok(model)
    }

    /// Persist the model in ONE transaction and bump `improv.model_version`.
    ///
    /// The save is a diff against this handle's snapshot (what it last loaded
    /// or saved): structure rows are upserted, and only rows the snapshot
    /// held but the model dropped are deleted (rows another client added
    /// survive); input cells added, changed or removed since are written,
    /// bound ones to their fact table under a per-cell optimistic guard
    /// (a removed one's value column is set to NULL, the row kept), unbound
    /// ones to `improv.cell`. Without a snapshot ([`Self::set_snapshot`]) the
    /// whole model is written unchecked and rows it lacks are deleted.
    ///
    /// Optimistic concurrency: fails with [`StoreError::Conflict`], writing
    /// nothing, if the version moved since this handle loaded or saved, or a
    /// bound cell it writes was changed in the database since.
    pub fn save_model(&mut self, model: &Model) -> Result<()> {
        let mid = self.model_id;
        let mut tx = self.client.transaction()?;
        let cur = version(&mut tx, mid, true)?;
        if self.loaded_tx.is_some_and(|l| l != cur) {
            return Err(StoreError::Conflict(CONFLICT_MSG.into()));
        }

        let (ids, names): (Vec<i64>, Vec<&str>) = model
            .categories
            .values()
            .map(|c| (i64::from(c.id.0), c.name.0.as_str()))
            .unzip();
        tx.execute(
            "INSERT INTO improv.category(model_id, id, name) \
             SELECT $1, * FROM unnest($2::int8[], $3::text[]) \
             ON CONFLICT (model_id, id) DO UPDATE SET name = EXCLUDED.name",
            &[&mid, &ids, &names],
        )?;
        let cat_ids = ids;

        let mut ids = Vec::new();
        let mut cats = Vec::new();
        let mut names = Vec::new();
        for i in model.items.values() {
            ids.push(i64::from(i.id.0));
            cats.push(i64::from(i.category.0));
            names.push(i.name.0.as_str());
        }
        tx.execute(
            "INSERT INTO improv.item(model_id, id, category_id, name) \
             SELECT $1, * FROM unnest($2::int8[], $3::int8[], $4::text[]) \
             ON CONFLICT (model_id, id) \
             DO UPDATE SET category_id = EXCLUDED.category_id, name = EXCLUDED.name",
            &[&mid, &ids, &cats, &names],
        )?;
        let item_ids = ids;

        let upsert = tx.prepare(
            "INSERT INTO improv.measure \
             (model_id, id, name, value_type, kind, formula, description, categories) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (model_id, id) DO UPDATE SET name = EXCLUDED.name, \
             value_type = EXCLUDED.value_type, kind = EXCLUDED.kind, \
             formula = EXCLUDED.formula, description = EXCLUDED.description, \
             categories = EXCLUDED.categories",
        )?;
        let mut measure_ids = Vec::new();
        for m in model.measures.values() {
            let (kind, formula) = match &m.kind {
                MeasureKind::Input => ("input", None),
                MeasureKind::Derived(f) => ("derived", Some(serde_json::to_string(f)?)),
            };
            let cats: Vec<i64> = m.categories.iter().map(|c| i64::from(c.0)).collect();
            let id = i64::from(m.id.0);
            tx.execute(
                &upsert,
                &[
                    &mid,
                    &id,
                    &m.name.0,
                    &vt_name(m.value_type),
                    &kind,
                    &formula,
                    &m.description,
                    &cats,
                ],
            )?;
            measure_ids.push(id);
        }
        // Children before parents, should the catalog grow foreign keys.
        let base = self.loaded.as_ref();
        for (table, keep) in [
            ("measure", &measure_ids),
            ("item", &item_ids),
            ("category", &cat_ids),
        ] {
            delete_dropped(&mut tx, mid, table, keep, base.map(|s| &s.ids[table]))?;
        }

        // Bound cells: values only, never DDL (see `write_bound`).
        let bound = bindings(&mut tx, model, mid)?;
        for b in &bound {
            write_bound(&mut tx, model, b, base)?;
        }

        // Unbound cells: with a snapshot, upsert those added or changed since
        // and delete those removed; without, improv.cell ends up holding
        // exactly the model's.
        let bound: HashSet<MeasureId> = bound.iter().map(|b| b.measure).collect();
        let cells: HashMap<(MeasureId, Coordinate), Value> = model
            .inputs
            .iter()
            .filter(|((m, _), _)| !bound.contains(m))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let (mut ms, mut coords, mut vals) = (Vec::new(), Vec::new(), Vec::new());
        for (k @ (m, coord), v) in &cells {
            if base.is_some_and(|s| s.cells.get(k) == Some(v)) {
                continue;
            }
            ms.push(i64::from(m.0));
            coords.push(serde_json::to_string(&coord.dims)?);
            vals.push(value_to_json(v)?.to_string());
        }
        if let Some(s) = base {
            let (mut gm, mut gk) = (Vec::new(), Vec::new());
            for (m, coord) in s.cells.keys().filter(|k| !cells.contains_key(*k)) {
                gm.push(i64::from(m.0));
                gk.push(serde_json::to_string(&coord.dims)?);
            }
            if !gm.is_empty() {
                tx.execute(
                    "DELETE FROM improv.cell c USING unnest($2::int8[], $3::text[]) u(m, k) \
                     WHERE c.model_id = $1 AND c.measure_id = u.m AND c.coord = u.k::jsonb",
                    &[&mid, &gm, &gk],
                )?;
            }
        } else {
            tx.execute(
                "DELETE FROM improv.cell c WHERE c.model_id = $1 AND NOT EXISTS \
                 (SELECT 1 FROM unnest($2::int8[], $3::text[]) u(m, k) \
                  WHERE c.measure_id = u.m AND c.coord = u.k::jsonb)",
                &[&mid, &ms, &coords],
            )?;
        }
        if !ms.is_empty() {
            tx.execute(
                "INSERT INTO improv.cell(model_id, measure_id, coord, value) \
                 SELECT $1, m, k::jsonb, v::jsonb FROM unnest($2::int8[], $3::text[], $4::text[]) u(m, k, v) \
                 ON CONFLICT (model_id, measure_id, coord) DO UPDATE SET value = EXCLUDED.value \
                 WHERE improv.cell.value IS DISTINCT FROM EXCLUDED.value",
                &[&mid, &ms, &coords, &vals],
            )?;
        }

        let (mut ids, mut names, mut layouts) = (Vec::new(), Vec::new(), Vec::new());
        for v in model.views.values() {
            ids.push(i64::from(v.id.0));
            names.push(v.name.0.as_str());
            layouts.push(serde_json::to_string(v)?);
        }
        tx.execute(
            "INSERT INTO improv.view(model_id, id, name, layout) \
             SELECT $1, i, n, l::jsonb FROM unnest($2::int8[], $3::text[], $4::text[]) u(i, n, l) \
             ON CONFLICT (model_id, id) DO UPDATE SET name = EXCLUDED.name, layout = EXCLUDED.layout",
            &[&mid, &ids, &names, &layouts],
        )?;
        delete_dropped(&mut tx, mid, "view", &ids, base.map(|s| &s.ids["view"]))?;

        // Lambdas: one improv.lambda row each, as JSON. Only those this
        // client changed are written, so a row improv.define wrote as text
        // keeps its text.
        let stored = base.map(|s| &s.lambdas);
        let ids: Vec<i64> = model.lambdas.keys().map(|i| i64::from(*i)).collect();
        let (mut up, mut names, mut defs) = (Vec::new(), Vec::new(), Vec::new());
        for (id, l) in &model.lambdas {
            if stored.is_some_and(|s| s.get(id) == Some(l)) {
                continue;
            }
            up.push(i64::from(*id));
            names.push(l.name.0.as_str());
            defs.push(serde_json::to_string(l)?);
        }
        tx.execute(
            "INSERT INTO improv.lambda(model_id, id, name, definition) \
             SELECT $1, i, n, d FROM unnest($2::int8[], $3::text[], $4::text[]) u(i, n, d) \
             ON CONFLICT (model_id, id) DO UPDATE SET name = EXCLUDED.name, \
             definition = EXCLUDED.definition",
            &[&mid, &up, &names, &defs],
        )?;
        delete_dropped(&mut tx, mid, "lambda", &ids, base.map(|s| &s.ids["lambda"]))?;

        let blob = serde_json::json!({
            "external_fns": model.external_fns,
            "external_calls": model.external_calls,
            "scenarios": model.scenarios,
            "sql_sources": model.sql_sources,
        })
        .to_string();
        tx.execute(
            "INSERT INTO improv.meta(model_id, blob) VALUES ($1, $2::text::jsonb) \
             ON CONFLICT (model_id) DO UPDATE SET blob = EXCLUDED.blob",
            &[&mid, &blob],
        )?;

        let new_tx: i64 = tx
            .query_one(
                "UPDATE improv.model_version SET tx = tx + 1 WHERE model_id = $1 RETURNING tx",
                &[&mid],
            )?
            .get(0);
        tx.commit()?;
        self.loaded_tx = Some(new_tx);
        self.loaded = Some(PgSnapshot::new(model, cells));
        Ok(())
    }
}
