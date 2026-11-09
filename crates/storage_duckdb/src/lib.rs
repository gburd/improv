//! Persist an Improv `Model` in a DuckDB file, in an `improv.*` catalog
//! shaped like the Postgres one (`improv_storage_pg`).
//!
//! [`DuckModelStore`] has the methods of `improv_storage_mentat::ModelStore`
//! and `improv_storage_pg::PgModelStore`; `improv_store::Store` opens it for
//! a `duckdb:<file>?model=<name>` target. One file holds any number of
//! models, each a row of `improv.model`. `improv.model_version` is the
//! optimistic-concurrency token and the store's high-water mark.
//!
//! **One writer.** DuckDB lets one process at a time open a file for
//! writing (it takes a lock on the file), and within a process two DuckDB
//! instances on one file would corrupt it. So every handle in a process
//! shares one instance per file ([`DuckModelStore::open`] keeps a registry),
//! and a second process is refused with a message that names the holder.
//! improv-server holds the file for as long as it runs; edit a served
//! model through the server.
//!
//! Encoding (the catalog is [`CATALOG_SQL`]):
//! - `measure.formula`: JSON of the `Formula` AST; `measure.categories`:
//!   `BIGINT[]` in the measure's order; `measure.format`: JSON of the
//!   `NumberFormat`, NULL for General;
//! - `cell.coord`: `{"<category id>": <item id>, …}` (the Postgres form);
//!   a cell's value is in exactly one of `value_number` (numbers, enums),
//!   `value_boolean` and `value_text` (text; datetimes as RFC 3339);
//! - `view.layout`, `lambda.definition`: JSON of the `View` and the
//!   `NamedLambda`; `meta.blob`: `{external_fns, external_calls,
//!   scenarios, sql_sources}`;
//! - `policy`: the Cedar text and groups JSON, with their version (a NULL
//!   text is "cleared").
//!
//! The cell tables have no primary key: DuckDB's index made a rewrite of a
//! million cells 30 times slower (24 s against 0.8 s). The store keeps a
//! cell unique by deleting its row before it writes one.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

use duckdb::{params, types::Value as DValue, Connection, OptionalExt};
use improv_core_model::{
    CategoryId, Coordinate, ExternalCall, ExternalFn, Formula, ItemId, Measure, MeasureId,
    MeasureKind, Model, Name, Scenario, ScenarioId, SqlSource, Value, ValueType,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

#[cfg(test)]
mod tests;

/// The catalog, created in a new file. Every statement is idempotent.
pub const CATALOG_SQL: &str = r#"
CREATE SCHEMA IF NOT EXISTS improv;
CREATE TABLE IF NOT EXISTS improv.catalog (version INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS improv.model (
    id   BIGINT PRIMARY KEY,
    name VARCHAR NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS improv.model_version (
    model_id BIGINT PRIMARY KEY,
    tx       BIGINT NOT NULL
);
CREATE TABLE IF NOT EXISTS improv.category (
    model_id BIGINT NOT NULL,
    id       BIGINT NOT NULL,
    name     VARCHAR NOT NULL
);
CREATE TABLE IF NOT EXISTS improv.item (
    model_id    BIGINT NOT NULL,
    id          BIGINT NOT NULL,
    category_id BIGINT NOT NULL,
    name        VARCHAR NOT NULL
);
CREATE TABLE IF NOT EXISTS improv.measure (
    model_id    BIGINT NOT NULL,
    id          BIGINT NOT NULL,
    name        VARCHAR NOT NULL,
    value_type  VARCHAR NOT NULL,
    kind        VARCHAR NOT NULL,
    formula     VARCHAR,
    description VARCHAR,
    categories  BIGINT[] NOT NULL,
    format      VARCHAR
);
CREATE TABLE IF NOT EXISTS improv.view (
    model_id BIGINT NOT NULL,
    id       BIGINT NOT NULL,
    name     VARCHAR NOT NULL,
    layout   VARCHAR NOT NULL
);
CREATE TABLE IF NOT EXISTS improv.lambda (
    model_id   BIGINT NOT NULL,
    id         BIGINT NOT NULL,
    name       VARCHAR NOT NULL,
    definition VARCHAR NOT NULL
);
CREATE TABLE IF NOT EXISTS improv.meta (
    model_id BIGINT PRIMARY KEY,
    blob     VARCHAR NOT NULL
);
CREATE TABLE IF NOT EXISTS improv.cell (
    model_id      BIGINT NOT NULL,
    measure_id    BIGINT NOT NULL,
    coord         VARCHAR NOT NULL,
    value_number  DOUBLE,
    value_boolean BOOLEAN,
    value_text    VARCHAR
);
CREATE TABLE IF NOT EXISTS improv.policy (
    model_id BIGINT PRIMARY KEY,
    version  BIGINT NOT NULL,
    text     VARCHAR,
    groups   VARCHAR
);
"#;

/// The catalog version this release writes; a file at a newer one is
/// refused rather than misread.
pub const CATALOG_VERSION: i32 = 1;

/// A model's stored access policies: Cedar text, the groups as JSON, and
/// how many times they were set or cleared.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPolicies {
    pub version: i64,
    pub text: String,
    pub groups: String,
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("data integrity: {0}")]
    Integrity(String),
    /// Another writer moved `improv.model_version` since this handle
    /// loaded, or committed first.
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("duckdb error: {0}")]
    Duck(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

const CONFLICT_MSG: &str =
    "the model was changed by another writer since it was loaded; reload and retry";

impl From<duckdb::Error> for StoreError {
    fn from(e: duckdb::Error) -> Self {
        let msg = e.to_string();
        // Two transactions wrote one row: DuckDB fails the second.
        if msg.contains("TransactionContext Error") || msg.contains("write-write conflict") {
            return StoreError::Conflict(CONFLICT_MSG.into());
        }
        StoreError::Duck(msg)
    }
}

/// Split `duckdb:<file>?model=<name>` into the file and the model name
/// (required, as for a Postgres model).
pub fn split_target(target: &str) -> Result<(&str, &str)> {
    let usage = |why: &str| {
        StoreError::Duck(format!(
            "{why}: a DuckDB model is duckdb:<file>?model=<name> \
             (e.g. duckdb:/srv/plans.duckdb?model=plan)"
        ))
    };
    let rest = target
        .strip_prefix("duckdb:")
        .ok_or_else(|| usage("not a duckdb: target"))?;
    let (path, query) = rest.rsplit_once('?').unwrap_or((rest, ""));
    let mut model = None;
    for p in query.split('&').filter(|p| !p.is_empty()) {
        match p.strip_prefix("model=") {
            Some(m) => model = Some(m),
            None => return Err(usage(&format!("unknown parameter {p:?}"))),
        }
    }
    if path.is_empty() {
        return Err(usage("no file"));
    }
    match model.filter(|m| !m.is_empty()) {
        Some(m) => Ok((path, m)),
        None => Err(usage("no ?model=")),
    }
}

/// The open DuckDB instances of this process: canonical file, the root
/// connection that keeps it open, and the handles using it.
type Registry = Vec<(PathBuf, Connection, usize)>;
static OPEN: Mutex<Registry> = Mutex::new(Vec::new());

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    OPEN.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A handle's share of its file's instance: the last one to go closes it,
/// under the registry lock, so a new instance never opens beside a closing
/// one (DuckDB checkpoints on close).
struct Lease(PathBuf);

impl Drop for Lease {
    fn drop(&mut self) {
        let mut open = registry();
        if let Some(i) = open.iter().position(|(p, _, _)| *p == self.0) {
            open[i].2 -= 1;
            if open[i].2 == 0 {
                drop(open.swap_remove(i));
            }
        }
    }
}

/// `path`, absolute and with its links resolved, so two spellings of one
/// file share an instance. A file not made yet: its directory's.
fn canonical(path: &Path) -> Result<PathBuf> {
    if let Ok(p) = path.canonicalize() {
        return Ok(p);
    }
    let name = path
        .file_name()
        .ok_or_else(|| StoreError::Duck(format!("{}: not a file name", path.display())))?;
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let dir = dir
        .canonicalize()
        .map_err(|e| StoreError::Duck(format!("cannot open {}: {e}", path.display())))?;
    Ok(dir.join(name))
}

/// What a failed open says. DuckDB's lock error names the program holding
/// the file; the rest says what to do.
fn open_error(path: &str, e: duckdb::Error) -> StoreError {
    let msg = e.to_string();
    // Unix: "Could not set lock on file …: Conflicting lock is held in
    // <program> (PID n) by user u. See also …". Windows: "Cannot open file
    // …: <why>\nFile is already open in \n<program> (PID n)".
    let holder = if let Some((_, h)) = msg.split_once("Conflicting lock is held in ") {
        h.split(". ").next().unwrap_or(h).trim()
    } else if let Some((_, h)) = msg.split_once("File is already open in") {
        h.trim()
    } else if msg.contains("Could not set lock") || msg.contains("used by another process") {
        "another program"
    } else {
        return StoreError::Duck(format!("cannot open {path}: {msg}"));
    };
    StoreError::Duck(format!(
        "{path} is open in another program ({holder}). DuckDB lets one program at a time \
         open a file for writing: while improv-server serves this model, change it through \
         the server, or stop the server first"
    ))
}

/// Open the instance of `path` (or share the open one) and its catalog.
/// Runs under the registry lock.
fn connect(open: &mut Registry, path: &str) -> Result<(Connection, PathBuf)> {
    let key = canonical(Path::new(path))?;
    if let Some(e) = open.iter_mut().find(|(p, _, _)| *p == key) {
        let conn = e.1.try_clone()?;
        e.2 += 1;
        return Ok((conn, key));
    }
    // A model store runs no SQL from the model: nothing outside the file.
    let config = duckdb::Config::default()
        .enable_external_access(false)
        .and_then(|c| c.enable_autoload_extension(false))?;
    let root = Connection::open_with_flags(path, config).map_err(|e| open_error(path, e))?;
    root.execute_batch(&format!("BEGIN; {CATALOG_SQL} COMMIT;"))?;
    let version: Option<i32> =
        root.query_row("SELECT max(version) FROM improv.catalog", [], |r| r.get(0))?;
    match version {
        None => {
            root.execute("INSERT INTO improv.catalog VALUES (?)", [CATALOG_VERSION])?;
        }
        Some(v) if v > CATALOG_VERSION => {
            return Err(StoreError::Integrity(format!(
                "{path} holds an Improv catalog of version {v}, newer than this release \
                 reads ({CATALOG_VERSION}): open it with a newer Improv"
            )))
        }
        Some(_) => {}
    }
    let conn = root.try_clone()?;
    open.push((key.clone(), root, 1));
    Ok((conn, key))
}

/// The id of model `name`, made (with its version row) when it is new.
fn model_id(conn: &Connection, name: &str) -> Result<i64> {
    let find = || {
        conn.query_row("SELECT id FROM improv.model WHERE name = ?", [name], |r| {
            r.get(0)
        })
        .optional()
    };
    if let Some(id) = find()? {
        return Ok(id);
    }
    let tx = conn.unchecked_transaction()?;
    let id: i64 = tx.query_row(
        "INSERT INTO improv.model SELECT coalesce(max(id), 0) + 1, ? FROM improv.model \
         RETURNING id",
        [name],
        |r| r.get(0),
    )?;
    tx.execute("INSERT INTO improv.model_version VALUES (?, 0)", [id])?;
    tx.commit()?;
    Ok(id)
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

/// A cell's value as its three typed columns.
type Typed = (Option<f64>, Option<bool>, Option<String>);

fn typed(v: &Value) -> Result<Typed> {
    Ok(match v {
        Value::Number(n) => (Some(*n), None, None),
        Value::Enum(e) => (Some(f64::from(*e)), None, None),
        Value::Boolean(b) => (None, Some(*b), None),
        Value::Text(s) => (None, None, Some(s.clone())),
        Value::DateTime(d) => (None, None, Some(d.to_rfc3339())),
        Value::Error(e) => {
            return Err(StoreError::Integrity(format!(
                "an error value cannot be stored as an input ({e:?})"
            )))
        }
    })
}

/// The value of typed columns, read as the measure's type where it tells
/// (an enum, a datetime), else as the column's.
fn untyped(vt: ValueType, (n, b, t): Typed) -> Result<Value> {
    Ok(match (n, b, t) {
        (Some(n), None, None) if vt == ValueType::Enum => Value::Enum(n as u32),
        (Some(n), None, None) => Value::Number(n),
        (None, Some(b), None) => Value::Boolean(b),
        (None, None, Some(t)) if vt == ValueType::DateTime => {
            match chrono::DateTime::parse_from_rfc3339(&t) {
                Ok(d) => Value::DateTime(d.with_timezone(&chrono::Utc)),
                Err(_) => Value::Text(t),
            }
        }
        (None, None, Some(t)) => Value::Text(t),
        _ => {
            return Err(StoreError::Integrity(
                "a cell must hold exactly one of value_number, value_boolean, value_text".into(),
            ))
        }
    })
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

/// A list of ids, bound as `?::BIGINT[]`.
fn id_list(ids: impl IntoIterator<Item = i64>) -> DValue {
    DValue::List(ids.into_iter().map(DValue::BigInt).collect())
}

/// Every row of `sql` over `p`, mapped by `f`.
fn rows<T>(
    c: &Connection,
    sql: &str,
    p: &[&dyn duckdb::ToSql],
    mut f: impl FnMut(&duckdb::Row) -> Result<T>,
) -> Result<Vec<T>> {
    let mut st = c.prepare(sql)?;
    let mut rs = st.query(p)?;
    let mut out = Vec::new();
    while let Some(r) = rs.next()? {
        out.push(f(r)?);
    }
    Ok(out)
}

fn version(c: &Connection, mid: i64) -> Result<i64> {
    Ok(c.query_row(
        "SELECT tx FROM improv.model_version WHERE model_id = ?",
        [mid],
        |r| r.get(0),
    )?)
}

/// Move the version on; the row's write conflicts with any other save's.
fn bump(c: &Connection, mid: i64) -> Result<i64> {
    Ok(c.query_row(
        "UPDATE improv.model_version SET tx = tx + 1 WHERE model_id = ? RETURNING tx",
        [mid],
        |r| r.get(0),
    )?)
}

/// Delete the `improv.cell` rows of `keys`.
fn delete_cells<'a>(
    c: &Connection,
    mid: i64,
    keys: impl Iterator<Item = &'a (MeasureId, Coordinate)>,
) -> Result<()> {
    let (mut ms, mut ks) = (Vec::new(), Vec::new());
    for (m, at) in keys {
        ms.push(DValue::BigInt(i64::from(m.0)));
        ks.push(DValue::Text(serde_json::to_string(&at.dims)?));
    }
    if !ms.is_empty() {
        c.execute(
            "DELETE FROM improv.cell c USING \
             (SELECT unnest(?::BIGINT[]) AS m, unnest(?::VARCHAR[]) AS k) u \
             WHERE c.model_id = ? AND c.measure_id = u.m AND c.coord = u.k",
            params![DValue::List(ms), DValue::List(ks), mid],
        )?;
    }
    Ok(())
}

/// Append the cells of `keys` that `model` holds.
fn append_cells<'a>(
    c: &Connection,
    mid: i64,
    model: &Model,
    keys: impl Iterator<Item = &'a (MeasureId, Coordinate)>,
) -> Result<()> {
    let mut ap = c.appender_to_db("cell", "improv")?;
    for k in keys {
        let Some(v) = model.inputs.get(k) else {
            continue;
        };
        let (n, b, t) = typed(v)?;
        let at = serde_json::to_string(&k.1.dims)?;
        ap.append_row(params![mid, i64::from(k.0 .0), at, n, b, t])?;
    }
    ap.flush()?;
    Ok(())
}

/// Replace every catalog row of the model but its cells.
fn write_structure(c: &Connection, mid: i64, model: &Model) -> Result<()> {
    for t in ["category", "item", "measure", "view", "lambda"] {
        c.execute(&format!("DELETE FROM improv.{t} WHERE model_id = ?"), [mid])?;
    }
    let mut ap = c.appender_to_db("category", "improv")?;
    for k in model.categories.values() {
        ap.append_row(params![mid, i64::from(k.id.0), k.name.0])?;
    }
    ap.flush()?;
    let mut ap = c.appender_to_db("item", "improv")?;
    for i in model.items.values() {
        ap.append_row(params![
            mid,
            i64::from(i.id.0),
            i64::from(i.category.0),
            i.name.0
        ])?;
    }
    ap.flush()?;
    let mut st =
        c.prepare("INSERT INTO improv.measure VALUES (?, ?, ?, ?, ?, ?, ?, ?::BIGINT[], ?)")?;
    for m in model.measures.values() {
        let (kind, formula) = match &m.kind {
            MeasureKind::Input => ("input", None),
            MeasureKind::Derived(f) => ("derived", Some(improv_core_model::json::to_string(f)?)),
        };
        let format = m.format.as_ref().map(serde_json::to_string).transpose()?;
        let cats = id_list(m.categories.iter().map(|c| i64::from(c.0)));
        st.execute(params![
            mid,
            i64::from(m.id.0),
            m.name.0,
            vt_name(m.value_type),
            kind,
            formula,
            m.description,
            cats,
            format
        ])?;
    }
    let mut ap = c.appender_to_db("view", "improv")?;
    for v in model.views.values() {
        let layout = serde_json::to_string(v)?;
        ap.append_row(params![mid, i64::from(v.id.0), v.name.0, layout])?;
    }
    ap.flush()?;
    let mut ap = c.appender_to_db("lambda", "improv")?;
    for (id, l) in &model.lambdas {
        let def = improv_core_model::json::to_string(l)?;
        ap.append_row(params![mid, i64::from(*id), l.name.0, def])?;
    }
    ap.flush()?;
    let meta = serde_json::json!({
        "external_fns": model.external_fns,
        "external_calls": model.external_calls,
        "scenarios": model.scenarios,
        "sql_sources": model.sql_sources,
    });
    c.execute(
        "INSERT OR REPLACE INTO improv.meta VALUES (?, ?)",
        params![mid, meta.to_string()],
    )?;
    Ok(())
}

/// A model store over one named model in a DuckDB file.
pub struct DuckModelStore {
    /// This handle's connection to the file's shared instance. Declared
    /// before `_lease`, so it closes first.
    conn: Connection,
    model_id: i64,
    /// `improv.model_version.tx` as of this handle's last load or save;
    /// a save refuses to commit if it moved. `None`: unchecked.
    loaded_tx: Option<i64>,
    /// The measures whose cells the last load read: `None` after a full
    /// load, the dependency closure after [`DuckModelStore::load_partial`].
    /// A full save rewrites only these measures' cells.
    loaded_scope: Option<HashSet<MeasureId>>,
    _lease: Lease,
}

impl DuckModelStore {
    /// Open (or create) the model of `duckdb:<file>?model=<name>`, creating
    /// the file and its catalog ([`CATALOG_SQL`]) when they are missing.
    /// Fails, naming the holder, when another program has the file open.
    pub fn open(target: &str) -> Result<Self> {
        let (path, name) = split_target(target)?;
        let mut open = registry();
        let (conn, key) = connect(&mut open, path)?;
        let lease = Lease(key);
        // The model row is made under the lock too: no other handle races it.
        let made = model_id(&conn, name);
        drop(open);
        Ok(DuckModelStore {
            model_id: made?,
            conn,
            loaded_tx: None,
            loaded_scope: None,
            _lease: lease,
        })
    }

    /// The model's current `improv.model_version.tx`, read live.
    pub fn tx_high_water(&mut self) -> Result<i64> {
        version(&self.conn, self.model_id)
    }

    /// The version this handle last loaded or saved at (`None` if neither).
    pub fn loaded_tx(&self) -> Option<i64> {
        self.loaded_tx
    }

    /// Adopt `tx` as the version a save checks against; `None` saves
    /// unconditionally.
    pub fn set_loaded_tx(&mut self, tx: Option<i64>) {
        self.loaded_tx = tx;
    }

    /// Re-read the model after a [`StoreError::Conflict`].
    pub fn reload_model(&mut self) -> Result<Model> {
        self.load_model()
    }

    /// Read the whole model, in one transaction.
    pub fn load_model(&mut self) -> Result<Model> {
        self.load(None)
    }

    /// Like [`Self::load_model`], but only the cells of `measure_ids` and
    /// their dependency closure. A full save through this handle then
    /// rewrites only those measures' cells.
    pub fn load_partial(&mut self, measure_ids: &[MeasureId]) -> Result<Model> {
        self.load(Some(measure_ids))
    }

    fn load(&mut self, roots: Option<&[MeasureId]>) -> Result<Model> {
        let mid = self.model_id;
        let tx = self.conn.transaction()?;
        let now = version(&tx, mid)?;
        let mut model = Model::new();
        let p: &[&dyn duckdb::ToSql] = &[&mid];
        for (id, name) in rows(
            &tx,
            "SELECT id, name FROM improv.category WHERE model_id = ? ORDER BY id",
            p,
            |r| Ok((id32(r.get(0)?)?, r.get::<_, String>(1)?)),
        )? {
            model.add_category(CategoryId(id), name);
        }
        for (id, cat, name) in rows(
            &tx,
            "SELECT id, category_id, name FROM improv.item WHERE model_id = ? ORDER BY id",
            p,
            |r| Ok((id32(r.get(0)?)?, id32(r.get(1)?)?, r.get::<_, String>(2)?)),
        )? {
            model.add_item(ItemId(id), CategoryId(cat), name);
        }
        let mut unreadable = Vec::new();
        let measures = rows(
            &tx,
            "SELECT id, name, value_type, kind, formula, description, categories, format \
             FROM improv.measure WHERE model_id = ? ORDER BY id",
            p,
            |r| {
                let id = MeasureId(id32(r.get(0)?)?);
                let kind = match (
                    r.get::<_, String>(3)?.as_str(),
                    r.get::<_, Option<String>>(4)?,
                ) {
                    ("input", _) => MeasureKind::Input,
                    ("derived", Some(f)) => match Formula::from_json(&f)? {
                        Some(f) => MeasureKind::Derived(f),
                        None => {
                            unreadable.push(id);
                            MeasureKind::Input
                        }
                    },
                    (k, f) => {
                        return Err(StoreError::Integrity(format!(
                            "measure {} has kind {k:?} and formula {f:?}",
                            id.0
                        )))
                    }
                };
                let DValue::List(cats) = r.get(6)? else {
                    return Err(StoreError::Integrity(format!(
                        "measure {}: categories is not a list",
                        id.0
                    )));
                };
                let categories = cats
                    .into_iter()
                    .map(|c| match c {
                        DValue::BigInt(c) => id32(c).map(CategoryId),
                        c => Err(StoreError::Integrity(format!("category id {c:?}"))),
                    })
                    .collect::<Result<_>>()?;
                Ok(Measure {
                    id,
                    name: Name(r.get(1)?),
                    value_type: vt_parse(&r.get::<_, String>(2)?)?,
                    categories,
                    kind,
                    description: r.get(5)?,
                    // Presentation only: one this release cannot read is General.
                    format: r
                        .get::<_, Option<String>>(7)?
                        .and_then(|f| serde_json::from_str(&f).ok()),
                })
            },
        )?;
        for m in measures {
            model.add_measure(m);
        }
        // Saved before the formula limits: the model still opens.
        for id in unreadable {
            if let Some(m) = model.measures.get_mut(&id) {
                m.mark_unreadable("its stored formula nests too deeply to load");
            }
        }
        let meta: Option<String> = tx
            .query_row(
                "SELECT blob FROM improv.meta WHERE model_id = ?",
                [mid],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(blob) = meta {
            let meta: Meta = improv_core_model::json::from_str(&blob)?;
            model.external_fns = meta.external_fns;
            model.external_calls = meta.external_calls;
            model.scenarios = meta.scenarios;
            model.sql_sources = meta.sql_sources;
        }
        let lambdas = rows(
            &tx,
            "SELECT id, definition FROM improv.lambda WHERE model_id = ? ORDER BY id",
            p,
            |r| Ok((id32(r.get(0)?)?, r.get::<_, String>(1)?)),
        )?;
        model
            .load_lambdas(&lambdas)
            .map_err(StoreError::Integrity)?;
        for layout in rows(
            &tx,
            "SELECT layout FROM improv.view WHERE model_id = ? ORDER BY id",
            p,
            |r| Ok(r.get::<_, String>(0)?),
        )? {
            model.add_view(serde_json::from_str(&layout)?);
        }

        let scope = roots.map(|r| model.measure_dependency_closure(r));
        let all = "SELECT measure_id, coord, value_number, value_boolean, value_text \
                   FROM improv.cell WHERE model_id = ?";
        let only = format!("{all} AND measure_id IN (SELECT unnest(?::BIGINT[]))");
        let list = scope
            .as_ref()
            .map(|s| id_list(s.iter().map(|m| i64::from(m.0))));
        let cells = match &list {
            None => rows(&tx, all, p, cell_row)?,
            Some(l) => rows(&tx, &only, params![mid, l], cell_row)?,
        };
        tx.commit()?;
        for (m, dims, v) in cells {
            let vt = model
                .measures
                .get(&m)
                .map(|m| m.value_type)
                .ok_or_else(|| StoreError::Integrity(format!("cell of unknown measure {}", m.0)))?;
            model.set_input(m, Coordinate { dims }, untyped(vt, v)?);
        }
        self.loaded_tx = Some(now);
        self.loaded_scope = scope;
        Ok(model)
    }

    /// Persist the whole model in one transaction and move the version on:
    /// every catalog row is replaced, and the cells of every measure this
    /// handle loaded (all of them, unless it loaded partially) are
    /// rewritten. Refused with [`StoreError::Conflict`], writing nothing,
    /// if another writer saved since this handle loaded or saved.
    pub fn save_model(&mut self, model: &Model) -> Result<()> {
        self.save(model, None, true)
    }

    /// [`Self::save_changes`] of cells alone. A cell of a measure the file
    /// does not hold is a [`StoreError::Integrity`] error.
    pub fn save_cells(&mut self, model: &Model, cells: &[(MeasureId, Coordinate)]) -> Result<()> {
        self.save(model, Some(cells), false)
    }

    /// Persist what changed since this handle's last load or save: each of
    /// `cells` gets its value in `model.inputs`, or is removed if the model
    /// has none; with `structure`, every catalog row is replaced and a
    /// dropped measure's cells deleted. No other cell is read or written.
    /// The same transaction and conflict check as [`Self::save_model`].
    pub fn save_changes(
        &mut self,
        model: &Model,
        cells: &[(MeasureId, Coordinate)],
        structure: bool,
    ) -> Result<()> {
        self.save(model, Some(cells), structure)
    }

    fn save(
        &mut self,
        model: &Model,
        only: Option<&[(MeasureId, Coordinate)]>,
        structure: bool,
    ) -> Result<()> {
        let mid = self.model_id;
        let tx = self.conn.transaction()?;
        if let Some(l) = self.loaded_tx {
            if version(&tx, mid)? != l {
                return Err(StoreError::Conflict(CONFLICT_MSG.into()));
            }
        }
        // First: a concurrent save fails here, before either writes more.
        let new_tx = bump(&tx, mid)?;
        if structure {
            write_structure(&tx, mid, model)?;
        }
        let measures = || id_list(model.measures.keys().map(|m| i64::from(m.0)));
        match only {
            Some(cells) => {
                let keys: BTreeSet<&(MeasureId, Coordinate)> = cells.iter().collect();
                // The measures of the cells it sets (clearing one is a delete).
                let named: BTreeSet<i64> = keys
                    .iter()
                    .filter(|k| model.inputs.contains_key(*k))
                    .map(|k| i64::from(k.0 .0))
                    .collect();
                let held: HashSet<i64> = rows(
                    &tx,
                    "SELECT id FROM improv.measure WHERE model_id = ? \
                     AND id IN (SELECT unnest(?::BIGINT[]))",
                    params![mid, id_list(named.iter().copied())],
                    |r| Ok(r.get(0)?),
                )?
                .into_iter()
                .collect();
                if let Some(m) = named.iter().find(|m| !held.contains(m)) {
                    return Err(StoreError::Integrity(format!(
                        "a cell of measure {m}, which is not saved; save the model first"
                    )));
                }
                delete_cells(&tx, mid, keys.iter().copied())?;
                append_cells(&tx, mid, model, keys.into_iter())?;
                if structure {
                    tx.execute(
                        "DELETE FROM improv.cell WHERE model_id = ? \
                         AND measure_id NOT IN (SELECT unnest(?::BIGINT[]))",
                        params![mid, measures()],
                    )?;
                }
            }
            None => {
                match &self.loaded_scope {
                    None => {
                        tx.execute("DELETE FROM improv.cell WHERE model_id = ?", [mid])?;
                    }
                    // A partial load's: its measures' cells, and the dropped
                    // measures'; any other cell only where the model has one.
                    Some(scope) => {
                        tx.execute(
                            "DELETE FROM improv.cell WHERE model_id = ? AND \
                             (measure_id IN (SELECT unnest(?::BIGINT[])) \
                              OR measure_id NOT IN (SELECT unnest(?::BIGINT[])))",
                            params![
                                mid,
                                id_list(scope.iter().map(|m| i64::from(m.0))),
                                measures()
                            ],
                        )?;
                        let outside = model.inputs.keys().filter(|k| !scope.contains(&k.0));
                        delete_cells(&tx, mid, outside)?;
                    }
                }
                append_cells(&tx, mid, model, model.inputs.keys())?;
            }
        }
        tx.commit()?;
        self.loaded_tx = Some(new_tx);
        Ok(())
    }

    /// The stored access policies, `None` when none are set.
    pub fn load_policies(&mut self) -> Result<Option<StoredPolicies>> {
        Ok(self
            .conn
            .query_row(
                "SELECT version, text, coalesce(groups, '') FROM improv.policy \
                 WHERE model_id = ? AND text IS NOT NULL",
                [self.model_id],
                |r| {
                    Ok(StoredPolicies {
                        version: r.get(0)?,
                        text: r.get(1)?,
                        groups: r.get(2)?,
                    })
                },
            )
            .optional()?)
    }

    /// The policies' version: how many times they were set or cleared (0:
    /// never).
    pub fn policy_version(&mut self) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT coalesce(max(version), 0) FROM improv.policy WHERE model_id = ?",
            [self.model_id],
            |r| r.get(0),
        )?)
    }

    /// Set the access policies (text and groups JSON, as checked by the
    /// caller), or clear them with `None`, and move both their version and
    /// the model's on (so a change feed reloads). Returns the new policy
    /// version.
    pub fn save_policies(&mut self, policies: Option<(&str, &str)>) -> Result<i64> {
        let mid = self.model_id;
        let tx = self.conn.transaction()?;
        let version: i64 = tx.query_row(
            "SELECT coalesce(max(version), 0) + 1 FROM improv.policy WHERE model_id = ?",
            [mid],
            |r| r.get(0),
        )?;
        let (text, groups) = policies.unzip();
        tx.execute(
            "INSERT OR REPLACE INTO improv.policy VALUES (?, ?, ?, ?)",
            params![mid, version, text, groups],
        )?;
        let new_tx = bump(&tx, mid)?;
        tx.commit()?;
        // Our own write: a save from this handle's current load stays unconflicted.
        if self.loaded_tx == Some(new_tx - 1) {
            self.loaded_tx = Some(new_tx);
        }
        Ok(version)
    }
}

/// One `improv.cell` row: measure, coordinate, typed value.
fn cell_row(r: &duckdb::Row) -> Result<(MeasureId, BTreeMap<CategoryId, ItemId>, Typed)> {
    let m = MeasureId(id32(r.get(0)?)?);
    let dims = serde_json::from_str(&r.get::<_, String>(1)?)?;
    Ok((m, dims, (r.get(2)?, r.get(3)?, r.get(4)?)))
}
