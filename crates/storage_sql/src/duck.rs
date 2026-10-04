//! DuckDB backend (embedded, via the `duckdb` crate with the `bundled`
//! feature so no system libduckdb is required).
//!
//! DuckDB is file/embedded like SQLite: no out-of-band secret. Its Rust API
//! mirrors rusqlite (prepared statements, `?` placeholders, typed value refs),
//! so this module reads the same way as the SQLite path in `backend.rs`.
//! Values are always bound as parameters; only validated identifiers reach a
//! query string (see `ident`/`sanitize_ident` in `lib.rs`).

use crate::backend::{Backend, Cell, Table};
use crate::{Result, SqlError};

/// Open a DuckDB database at `path` (or `:memory:`). Embedded, no secret.
///
/// External access is disabled (`enable_external_access = false`), so a stored
/// query cannot read host files (`read_csv('/etc/passwd')`), fetch URLs or
/// `ATTACH` other databases. The database stays read-write because the same
/// handle serves `export_measure`; refresh queries are confined by the
/// read-only transaction in `duck_query` instead of `access_mode=READ_ONLY`.
pub fn connect_duckdb(path: &str) -> Result<Backend> {
    let err = |e: duckdb::Error| SqlError::Connect(format!("duckdb connect failed: {e}"));
    let config = duckdb::Config::default()
        .enable_external_access(false)
        .and_then(|c| c.enable_autoload_extension(false))
        .map_err(err)?;
    let conn = duckdb::Connection::open_with_flags(path, config).map_err(err)?;
    Ok(Backend::Duckdb(conn))
}

/// Run `sql` inside a DuckDB `READ ONLY` transaction, so it cannot write
/// whatever statement type it turns out to be.
pub(crate) fn duck_query(conn: &duckdb::Connection, sql: &str) -> Result<Table> {
    let txn = |s: &str| {
        conn.execute_batch(s)
            .map_err(|e| SqlError::Import(format!("duckdb {s} failed: {e}")))
    };
    txn("BEGIN TRANSACTION READ ONLY")?;
    let out = duck_query_inner(conn, sql);
    match out {
        Ok(_) => txn("COMMIT")?,
        Err(_) => {
            let _ = txn("ROLLBACK");
        }
    }
    out
}

fn duck_query_inner(conn: &duckdb::Connection, sql: &str) -> Result<Table> {
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| SqlError::Import(format!("duckdb prepare failed: {e}")))?;
    let mut rows_iter = stmt
        .query([])
        .map_err(|e| SqlError::Import(format!("duckdb query failed: {e}")))?;

    let mut rows = Vec::new();
    let mut columns: Option<Vec<String>> = None;
    while let Some(row) = rows_iter
        .next()
        .map_err(|e| SqlError::Import(format!("duckdb row failed: {e}")))?
    {
        // Column names are available once we have a statement/row; capture once.
        if columns.is_none() {
            let stmt_ref = row.as_ref();
            columns = Some(
                stmt_ref
                    .column_names()
                    .into_iter()
                    .map(|s| s.to_string())
                    .collect(),
            );
        }
        let ncols = columns.as_ref().map(|c| c.len()).unwrap_or(0);
        let mut cells = Vec::with_capacity(ncols);
        for i in 0..ncols {
            let vr = row
                .get_ref(i)
                .map_err(|e| SqlError::Import(format!("duckdb column {i} failed: {e}")))?;
            cells.push(duck_cell(vr)?);
        }
        rows.push(cells);
    }

    // Empty result still needs column names: fall back to the statement metadata.
    let columns = match columns {
        Some(c) => c,
        None => stmt
            .column_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
    };
    Ok(Table { columns, rows })
}

/// Map a DuckDB value ref to a [`Cell`]. Integer family → Int, float/decimal
/// family → Float, text → Text, boolean → Bool, null → Null.
fn duck_cell(vr: duckdb::types::ValueRef<'_>) -> Result<Cell> {
    use duckdb::types::ValueRef as V;
    Ok(match vr {
        V::Null => Cell::Null,
        V::Boolean(b) => Cell::Bool(b),
        V::TinyInt(v) => Cell::Int(v as i64),
        V::SmallInt(v) => Cell::Int(v as i64),
        V::Int(v) => Cell::Int(v as i64),
        V::BigInt(v) => Cell::Int(v),
        V::HugeInt(v) => Cell::Float(v as f64),
        V::UTinyInt(v) => Cell::Int(v as i64),
        V::USmallInt(v) => Cell::Int(v as i64),
        V::UInt(v) => Cell::Int(v as i64),
        V::UBigInt(v) => Cell::Int(v as i64),
        V::Float(v) => Cell::Float(v as f64),
        V::Double(v) => Cell::Float(v),
        V::Decimal(v) => Cell::Float(v.to_string().parse::<f64>().unwrap_or(0.0)),
        V::Text(t) => Cell::Text(String::from_utf8_lossy(t).into_owned()),
        other => {
            // Dates/timestamps/blobs/etc.: stringify defensively rather than fail.
            Cell::Text(format!("{other:?}"))
        }
    })
}

pub(crate) fn duck_execute(conn: &duckdb::Connection, sql: &str, params: &[Cell]) -> Result<()> {
    let bound: Vec<duckdb::types::Value> = params.iter().map(cell_to_duck).collect();
    conn.execute(sql, duckdb::params_from_iter(bound))
        .map_err(|e| SqlError::Export(format!("duckdb execute failed: {e}")))?;
    Ok(())
}

fn cell_to_duck(c: &Cell) -> duckdb::types::Value {
    use duckdb::types::Value as V;
    match c {
        Cell::Null => V::Null,
        Cell::Int(i) => V::BigInt(*i),
        Cell::Float(f) => V::Double(*f),
        Cell::Text(t) => V::Text(t.clone()),
        Cell::Bool(b) => V::Boolean(*b),
    }
}
