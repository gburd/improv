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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn mem() -> duckdb::Connection {
        duckdb::Connection::open_in_memory().unwrap()
    }

    #[test]
    fn duck_maps_every_value_kind() {
        let t = duck_query(
            &mem(),
            "SELECT NULL AS n, true AS b, 1::TINYINT AS ti, 2::SMALLINT AS si, \
             3::INTEGER AS i, 4::BIGINT AS bi, 5::HUGEINT AS hi, 6::UTINYINT AS uti, \
             7::USMALLINT AS usi, 8::UINTEGER AS ui, 9::UBIGINT AS ubi, \
             1.5::FLOAT AS f, 2.5::DOUBLE AS d, 3.25::DECIMAL(5,2) AS dec, \
             'x' AS t, DATE '2025-01-02' AS dt",
        )
        .unwrap();
        assert_eq!(t.columns.len(), 16);
        let r = &t.rows[0];
        assert_eq!(
            r[..15],
            [
                Cell::Null,
                Cell::Bool(true),
                Cell::Int(1),
                Cell::Int(2),
                Cell::Int(3),
                Cell::Int(4),
                Cell::Float(5.0),
                Cell::Int(6),
                Cell::Int(7),
                Cell::Int(8),
                Cell::Int(9),
                Cell::Float(1.5),
                Cell::Float(2.5),
                Cell::Float(3.25),
                Cell::Text("x".into()),
            ]
        );
        // Dates are stringified defensively, not an error.
        assert!(matches!(&r[15], Cell::Text(s) if !s.is_empty()));
    }

    #[test]
    fn empty_result_keeps_column_names() {
        let t = duck_query(&mem(), "SELECT 1 AS a, 'q' AS b WHERE false").unwrap();
        assert_eq!(t.columns, ["a", "b"]);
        assert!(t.rows.is_empty());
    }

    #[test]
    fn execute_binds_every_cell_kind_and_reports_errors() {
        let c = mem();
        duck_execute(
            &c,
            "CREATE TABLE t (a INTEGER, b BIGINT, c DOUBLE, d VARCHAR, e BOOLEAN)",
            &[],
        )
        .unwrap();
        duck_execute(
            &c,
            "INSERT INTO t VALUES (?, ?, ?, ?, ?)",
            &[
                Cell::Null,
                Cell::Int(2),
                Cell::Float(0.5),
                Cell::Text("s".into()),
                Cell::Bool(false),
            ],
        )
        .unwrap();
        let t = duck_query(&c, "SELECT * FROM t").unwrap();
        assert_eq!(
            t.rows,
            [[
                Cell::Null,
                Cell::Int(2),
                Cell::Float(0.5),
                Cell::Text("s".into()),
                Cell::Bool(false)
            ]]
        );
        let err = duck_execute(&c, "INSERT INTO nope VALUES (1)", &[]).unwrap_err();
        assert!(err.to_string().contains("duckdb execute failed"), "{err}");
        let err = duck_query(&c, "FROBNICATE everything").err().unwrap();
        assert!(err.to_string().contains("duckdb prepare failed"), "{err}");
        // The failed read rolled back; the connection still works.
        assert_eq!(
            duck_query(&c, "SELECT count(*) AS n FROM t")
                .unwrap()
                .rows
                .len(),
            1
        );
    }

    #[test]
    fn connect_failure_is_a_connect_error() {
        let Err(err) = connect_duckdb("/nonexistent-dir-improv/x/y.duckdb") else {
            panic!("opened a path under a missing directory");
        };
        assert!(err.to_string().contains("duckdb connect failed"), "{err}");
    }
}
