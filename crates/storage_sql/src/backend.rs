//! Backend abstraction over SQL drivers.
//!
//! Both drivers produce a common [`Table`] of [`Cell`]s, so the column→model
//! mapping in `lib.rs` is backend-agnostic. Data values always cross the
//! boundary as typed cells; only validated identifiers ever reach a query
//! string (see `ident`/`sanitize_ident`).

use crate::{Result, SqlError};
use improv_core_model::Value;

/// A single result-cell value, normalized across drivers.
#[derive(Debug, Clone, PartialEq)]
pub enum Cell {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
}

impl Cell {
    /// Stringify a dimension cell (used to intern items by name).
    pub fn as_text(&self) -> String {
        match self {
            Cell::Null => String::new(),
            Cell::Int(i) => i.to_string(),
            Cell::Float(f) => f.to_string(),
            Cell::Text(t) => t.clone(),
            Cell::Bool(b) => b.to_string(),
        }
    }

    /// Coerce a value cell to the numeric measure value.
    pub fn as_number(&self) -> Result<f64> {
        match self {
            Cell::Int(i) => Ok(*i as f64),
            Cell::Float(f) => Ok(*f),
            Cell::Bool(b) => Ok(if *b { 1.0 } else { 0.0 }),
            Cell::Text(t) => t
                .parse::<f64>()
                .map_err(|_| SqlError::Import(format!("value {t:?} is not numeric"))),
            Cell::Null => Err(SqlError::Import("value column is NULL".into())),
        }
    }

    /// Map a cell to a model [`Value`] (Number/Bool/Text), shared across
    /// backends. Ints/Floats → Number, Bool → Boolean, Text → Text, Null → Text("").
    pub fn to_value(&self) -> Value {
        match self {
            Cell::Null => Value::Text(String::new()),
            Cell::Int(i) => Value::Number(*i as f64),
            Cell::Float(f) => Value::Number(*f),
            Cell::Text(t) => Value::Text(t.clone()),
            Cell::Bool(b) => Value::Boolean(*b),
        }
    }
}

/// A materialized query result: column names + row-major cells.
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
}

impl Table {
    /// Index of a named column, or a clear import error if absent.
    pub fn column_index(&self, name: &str) -> Result<usize> {
        self.columns
            .iter()
            .position(|c| c == name)
            .ok_or_else(|| SqlError::Import(format!("no column '{name}' in query result")))
    }
}

/// The operations `import_query`/`export_measure`/`refresh_sql_measure` need
/// from a connection. Implemented for `&mut Backend` (SQLite or Postgres) and,
/// for backward compatibility with SQLite-only callers, `&rusqlite::Connection`.
pub trait SqlConn {
    fn query(&mut self, sql: &str) -> Result<Table>;
    fn execute(&mut self, sql: &str, params: &[Cell]) -> Result<()>;
    /// Positional placeholder for the `n`-th (1-based) parameter.
    fn placeholder(&self, n: usize) -> String;
    /// SQL type keyword for a floating value column in `CREATE TABLE`.
    fn real_type(&self) -> &'static str;
}

impl SqlConn for &mut Backend {
    fn query(&mut self, sql: &str) -> Result<Table> {
        Backend::query(self, sql)
    }
    fn execute(&mut self, sql: &str, params: &[Cell]) -> Result<()> {
        Backend::execute(self, sql, params)
    }
    fn placeholder(&self, n: usize) -> String {
        Backend::placeholder(self, n)
    }
    fn real_type(&self) -> &'static str {
        Backend::real_type(self)
    }
}

impl SqlConn for &rusqlite::Connection {
    fn query(&mut self, sql: &str) -> Result<Table> {
        sqlite_query(self, sql)
    }
    fn execute(&mut self, sql: &str, params: &[Cell]) -> Result<()> {
        let bound: Vec<rusqlite::types::Value> = params.iter().map(cell_to_rusqlite).collect();
        rusqlite::Connection::execute(self, sql, rusqlite::params_from_iter(bound))?;
        Ok(())
    }
    fn placeholder(&self, _n: usize) -> String {
        "?".to_string()
    }
    fn real_type(&self) -> &'static str {
        "REAL"
    }
}

/// An open connection to one of the supported SQL backends. Data values are
/// always passed to `execute` as [`Cell`] parameters (never interpolated).
pub enum Backend {
    Sqlite(rusqlite::Connection),
    Postgres(postgres::Client),
}

impl Backend {
    /// Run a `SELECT` and materialize its rows.
    pub fn query(&mut self, sql: &str) -> Result<Table> {
        match self {
            Backend::Sqlite(conn) => sqlite_query(conn, sql),
            Backend::Postgres(client) => crate::pg::pg_query(client, sql),
        }
    }

    /// Run a statement, binding `params` positionally as data.
    pub fn execute(&mut self, sql: &str, params: &[Cell]) -> Result<()> {
        match self {
            Backend::Sqlite(conn) => {
                let bound: Vec<rusqlite::types::Value> =
                    params.iter().map(cell_to_rusqlite).collect();
                conn.execute(sql, rusqlite::params_from_iter(bound))?;
                Ok(())
            }
            Backend::Postgres(client) => crate::pg::pg_execute(client, sql, params),
        }
    }

    /// Positional placeholder for the `n`-th (1-based) parameter, per driver:
    /// `?` for SQLite, `$n` for Postgres.
    pub fn placeholder(&self, n: usize) -> String {
        match self {
            Backend::Sqlite(_) => "?".to_string(),
            Backend::Postgres(_) => format!("${n}"),
        }
    }

    /// SQL type keyword for a floating value column in a `CREATE TABLE`.
    pub fn real_type(&self) -> &'static str {
        match self {
            Backend::Sqlite(_) => "REAL",
            Backend::Postgres(_) => "DOUBLE PRECISION",
        }
    }
}

fn cell_to_rusqlite(c: &Cell) -> rusqlite::types::Value {
    use rusqlite::types::Value as V;
    match c {
        Cell::Null => V::Null,
        Cell::Int(i) => V::Integer(*i),
        Cell::Float(f) => V::Real(*f),
        Cell::Text(t) => V::Text(t.clone()),
        Cell::Bool(b) => V::Integer(*b as i64),
    }
}

fn sqlite_query(conn: &rusqlite::Connection, sql: &str) -> Result<Table> {
    use rusqlite::types::ValueRef;
    let mut stmt = conn.prepare(sql)?;
    let columns: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();
    let ncols = columns.len();
    let mut rows_iter = stmt.query([])?;
    let mut rows = Vec::new();
    while let Some(row) = rows_iter.next()? {
        let mut cells = Vec::with_capacity(ncols);
        for i in 0..ncols {
            cells.push(match row.get_ref(i)? {
                ValueRef::Null => Cell::Null,
                ValueRef::Integer(v) => Cell::Int(v),
                ValueRef::Real(v) => Cell::Float(v),
                ValueRef::Text(t) => Cell::Text(String::from_utf8_lossy(t).into_owned()),
                ValueRef::Blob(_) => return Err(SqlError::Import("blob cell value".into())),
            });
        }
        rows.push(cells);
    }
    Ok(Table { columns, rows })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_to_value_maps_types() {
        assert_eq!(Cell::Int(3).to_value(), Value::Number(3.0));
        assert_eq!(Cell::Float(2.5).to_value(), Value::Number(2.5));
        assert_eq!(Cell::Bool(true).to_value(), Value::Boolean(true));
        assert_eq!(Cell::Text("x".into()).to_value(), Value::Text("x".into()));
        assert_eq!(Cell::Null.to_value(), Value::Text(String::new()));
    }

    #[test]
    fn cell_as_number_coerces() {
        assert_eq!(Cell::Int(7).as_number().unwrap(), 7.0);
        assert_eq!(Cell::Text("1.5".into()).as_number().unwrap(), 1.5);
        assert!(Cell::Text("nope".into()).as_number().is_err());
        assert!(Cell::Null.as_number().is_err());
    }
}
