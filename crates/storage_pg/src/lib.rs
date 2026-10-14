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
//! - `meta.blob`: `{external_fns, external_calls, scenarios, sql_sources}`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

use improv_core_model::{
    parse_definition, parse_expr, CategoryId, Coordinate, Definition, ExternalCall, ExternalFn,
    Formula, ItemId, Measure, MeasureId, MeasureKind, Model, Name, Scenario, ScenarioId, SqlSource,
    Value, ValueType,
};
use postgres::config::{Host, SslMode};
use postgres::{Client, Config, GenericClient, IsolationLevel, NoTls};
use std::collections::{BTreeMap, HashMap, HashSet};

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
              CHECK (sync_mode IN ('logical', 'trigger', 'manual')),
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
}

/// One `improv.binding` row, resolved against the model.
struct Binding {
    measure: MeasureId,
    /// `"schema"."table"`, quoted.
    table: String,
    value_col: String,
    /// `(category, column)`, in `dim_cols` key order.
    dims: Vec<(CategoryId, String)>,
}

fn bindings(c: &mut impl GenericClient, model: &Model, mid: i64) -> Result<Vec<Binding>> {
    let rows = c.query(
        "SELECT b.measure_id, n.nspname::text, c.relname::text, b.value_col, b.dim_cols::text \
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

/// A model store over one named model in a Postgres database.
pub struct PgModelStore {
    client: Client,
    model_id: i64,
    /// `improv.model_version.tx` as of this handle's last load or save;
    /// `save_model` refuses to commit if it moved. `None`: unchecked.
    loaded_tx: Option<i64>,
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
        let installed: bool = client
            .query_one(
                "SELECT to_regclass('improv.model_version') IS NOT NULL",
                &[],
            )?
            .get(0);
        if !installed {
            let mut tx = client.transaction()?;
            tx.execute(
                "SELECT pg_advisory_xact_lock(hashtext('improv.catalog'))",
                &[],
            )?;
            tx.batch_execute(CATALOG_SQL)?;
            tx.commit()?;
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
    /// their dependency closure. Saving the result drops every other
    /// unbound cell, as with the Mentat store.
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
        if let Some(r) = tx.query_opt(
            "SELECT blob::text FROM improv.meta WHERE model_id = $1",
            &[&mid],
        )? {
            let meta: Meta = serde_json::from_str(r.get(0))?;
            model.external_fns = meta.external_fns;
            model.external_calls = meta.external_calls;
            model.scenarios = meta.scenarios;
            model.sql_sources = meta.sql_sources;
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

        // Bound cells: dimension values map to items by name; unseen values
        // mint a new item (persisted by the next save).
        let mut names: HashMap<(CategoryId, String), ItemId> = model
            .items
            .values()
            .map(|i| ((i.category, i.name.0.clone()), i.id))
            .collect();
        let mut next_item = model.items.keys().map(|i| i.0 + 1).max().unwrap_or(1);
        for b in bindings(&mut tx, &model, mid)? {
            if keep.as_ref().is_some_and(|k| !k.contains(&b.measure)) {
                continue;
            }
            let vt = value_type_of(&model, b.measure)?;
            let mut cols: Vec<String> = b
                .dims
                .iter()
                .map(|(_, c)| format!("{}::text", ident(c)))
                .collect();
            cols.push(format!("to_jsonb({})::text", ident(&b.value_col)));
            // Lexical by dimension values (design §8), so minted item ids
            // are the same on every load.
            let order: Vec<String> = (1..=b.dims.len()).map(|i| i.to_string()).collect();
            let mut sql = format!("SELECT {} FROM {}", cols.join(", "), b.table);
            if !order.is_empty() {
                sql += &format!(" ORDER BY {}", order.join(", "));
            }
            'row: for r in tx.query(&sql, &[])? {
                // NULL value: no cell. NULL dimension: the row is ignored.
                let Some(v) = r.get::<_, Option<&str>>(b.dims.len()) else {
                    continue;
                };
                let mut coord = Coordinate::new();
                for (i, (cat, _)) in b.dims.iter().enumerate() {
                    let Some(name) = r.get::<_, Option<String>>(i) else {
                        continue 'row;
                    };
                    let item = *names.entry((*cat, name.clone())).or_insert_with(|| {
                        next_item += 1;
                        ItemId(next_item - 1)
                    });
                    if !model.items.contains_key(&item) {
                        model.add_item(item, *cat, name);
                    }
                    coord = coord.with(*cat, item);
                }
                let v = json_to_value(vt, &serde_json::from_str(v)?)?;
                if model.inputs.insert((b.measure, coord), v).is_some() {
                    return Err(StoreError::Integrity(format!(
                        "{} has two rows for one coordinate of measure {}",
                        b.table, b.measure.0
                    )));
                }
            }
        }
        tx.commit()?;
        self.loaded_tx = Some(tx_now);
        Ok(model)
    }

    /// Persist the whole model in ONE transaction: structure rows are upserted
    /// (rows the model no longer has are deleted), bound input cells are
    /// written to their fact table (values only: `UPDATE` by dimensions,
    /// `INSERT` if absent), unbound ones to `improv.cell`, and
    /// `improv.model_version` is bumped.
    ///
    /// Optimistic concurrency: fails with [`StoreError::Conflict`], writing
    /// nothing, if the version moved since this handle loaded or saved.
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
        for (table, ids) in [
            ("measure", &measure_ids),
            ("item", &item_ids),
            ("category", &cat_ids),
        ] {
            tx.execute(
                &format!("DELETE FROM improv.{table} WHERE model_id = $1 AND id <> ALL($2)"),
                &[&mid, ids],
            )?;
        }

        // Bound cells: values only, never DDL. One UPDATE + one INSERT per
        // binding; jsonb_populate_recordset casts each JSON value to its
        // column's type.
        let bound = bindings(&mut tx, model, mid)?;
        for b in &bound {
            let mut rows = Vec::new();
            for ((m, coord), v) in &model.inputs {
                if *m != b.measure {
                    continue;
                }
                let mut row = serde_json::Map::new();
                for (cat, col) in &b.dims {
                    let item = coord
                        .get(*cat)
                        .and_then(|i| model.items.get(&i))
                        .ok_or_else(|| {
                            StoreError::Integrity(format!(
                                "a cell of measure {} lacks a {} item",
                                m.0, col
                            ))
                        })?;
                    row.insert(col.clone(), item.name.0.as_str().into());
                }
                row.insert(b.value_col.clone(), value_to_json(v)?);
                rows.push(serde_json::Value::Object(row));
            }
            let rows = serde_json::Value::Array(rows).to_string();
            let v = ident(&b.value_col);
            let on = b
                .dims
                .iter()
                .map(|(_, c)| format!("t.{0} = r.{0}", ident(c)))
                .chain(std::iter::once("true".to_string()))
                .collect::<Vec<_>>()
                .join(" AND ");
            let rs = format!(
                "jsonb_populate_recordset(NULL::{}, $1::text::jsonb) r",
                b.table
            );
            let t = &b.table;
            tx.execute(
                &format!(
                    "UPDATE {t} t SET {v} = r.{v} FROM {rs} \
                     WHERE {on} AND t.{v} IS DISTINCT FROM r.{v}"
                ),
                &[&rows],
            )?;
            let cols: Vec<String> = b
                .dims
                .iter()
                .map(|(_, c)| ident(c))
                .chain(std::iter::once(v.clone()))
                .collect();
            let rcols: Vec<String> = cols.iter().map(|c| format!("r.{c}")).collect();
            tx.execute(
                &format!(
                    "INSERT INTO {t} ({}) SELECT {} FROM {rs} \
                     WHERE NOT EXISTS (SELECT 1 FROM {t} t WHERE {on})",
                    cols.join(", "),
                    rcols.join(", ")
                ),
                &[&rows],
            )?;
        }

        // Unbound cells: improv.cell ends up holding exactly the model's.
        let bound: HashSet<MeasureId> = bound.iter().map(|b| b.measure).collect();
        let (mut ms, mut coords, mut vals) = (Vec::new(), Vec::new(), Vec::new());
        for ((m, coord), v) in &model.inputs {
            if bound.contains(m) {
                continue;
            }
            ms.push(i64::from(m.0));
            coords.push(serde_json::to_string(&coord.dims)?);
            vals.push(value_to_json(v)?.to_string());
        }
        tx.execute(
            "DELETE FROM improv.cell c WHERE c.model_id = $1 AND NOT EXISTS \
             (SELECT 1 FROM unnest($2::int8[], $3::text[]) u(m, k) \
              WHERE c.measure_id = u.m AND c.coord = u.k::jsonb)",
            &[&mid, &ms, &coords],
        )?;
        tx.execute(
            "INSERT INTO improv.cell(model_id, measure_id, coord, value) \
             SELECT $1, m, k::jsonb, v::jsonb FROM unnest($2::int8[], $3::text[], $4::text[]) u(m, k, v) \
             ON CONFLICT (model_id, measure_id, coord) DO UPDATE SET value = EXCLUDED.value \
             WHERE improv.cell.value IS DISTINCT FROM EXCLUDED.value",
            &[&mid, &ms, &coords, &vals],
        )?;

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
        tx.execute(
            "DELETE FROM improv.view WHERE model_id = $1 AND id <> ALL($2)",
            &[&mid, &ids],
        )?;

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
        Ok(())
    }
}
