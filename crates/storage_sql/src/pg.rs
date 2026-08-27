//! Postgres backend (synchronous `postgres` crate).
//!
//! Credentials are **out of band**: `connect_postgres` takes a full connection
//! string that the caller assembled at connect time (see `conn::open`, which
//! injects the password from an env var). This module never logs a connection
//! string — a URI can carry a secret, so errors are redacted.

use crate::backend::{Backend, Cell, Table};
use crate::{Result, SqlError};
use postgres::types::Type;
use postgres::{Client, NoTls};

/// Open a Postgres connection from a full libpq/URI connection string.
///
/// The string may embed a password; **do not log it**. Prefer building it via
/// `conn::open`, which pulls the secret from the environment. On failure the
/// error is redacted so no secret leaks into logs.
pub fn connect_postgres(conn_str: &str) -> Result<Backend> {
    let client = Client::connect(conn_str, NoTls)
        .map_err(|e| SqlError::Connect(format!("postgres connect failed: {}", redact_pg(e))))?;
    Ok(Backend::Postgres(client))
}

/// Best-effort scrub of a driver error so a password embedded in a connection
/// string never reaches logs. We keep only the error's category, not its text.
fn redact_pg(e: postgres::Error) -> String {
    // postgres::Error's Display can echo back the DSN on some failures; return a
    // fixed string rather than risk leaking it.
    if e.as_db_error().is_some() {
        "database error (redacted)".to_string()
    } else {
        "connection error (redacted)".to_string()
    }
}

pub(crate) fn pg_query(client: &mut Client, sql: &str) -> Result<Table> {
    let rows = client
        .query(sql, &[])
        .map_err(|e| SqlError::Import(format!("postgres query failed: {}", redact_pg(e))))?;

    // Column names come from the first row; an empty result still needs them, so
    // fall back to a prepared statement's column metadata when there are no rows.
    let columns: Vec<String> = if let Some(r0) = rows.first() {
        r0.columns().iter().map(|c| c.name().to_string()).collect()
    } else {
        let stmt = client
            .prepare(sql)
            .map_err(|e| SqlError::Import(format!("postgres prepare failed: {}", redact_pg(e))))?;
        stmt.columns()
            .iter()
            .map(|c| c.name().to_string())
            .collect()
    };

    let mut out_rows = Vec::with_capacity(rows.len());
    for row in &rows {
        let mut cells = Vec::with_capacity(row.len());
        for i in 0..row.len() {
            cells.push(pg_cell(row, i)?);
        }
        out_rows.push(cells);
    }
    Ok(Table {
        columns,
        rows: out_rows,
    })
}

/// Map one Postgres cell to a [`Cell`], dispatching on the column's SQL type.
/// Numeric-family types → Int/Float, bool → Bool, everything textual → Text.
fn pg_cell(row: &postgres::Row, i: usize) -> Result<Cell> {
    let ty = row.columns()[i].type_();
    let mapped = match *ty {
        Type::BOOL => row.try_get::<_, Option<bool>>(i).map(|o| match o {
            Some(b) => Cell::Bool(b),
            None => Cell::Null,
        }),
        Type::INT2 => row
            .try_get::<_, Option<i16>>(i)
            .map(|o| o.map(|v| Cell::Int(v as i64)).unwrap_or(Cell::Null)),
        Type::INT4 => row
            .try_get::<_, Option<i32>>(i)
            .map(|o| o.map(|v| Cell::Int(v as i64)).unwrap_or(Cell::Null)),
        Type::INT8 => row
            .try_get::<_, Option<i64>>(i)
            .map(|o| o.map(Cell::Int).unwrap_or(Cell::Null)),
        Type::FLOAT4 => row
            .try_get::<_, Option<f32>>(i)
            .map(|o| o.map(|v| Cell::Float(v as f64)).unwrap_or(Cell::Null)),
        Type::FLOAT8 => row
            .try_get::<_, Option<f64>>(i)
            .map(|o| o.map(Cell::Float).unwrap_or(Cell::Null)),
        // NUMERIC has no native Rust mapping without extra crates; pg can cast it
        // to text on the way out. Read it as a string and parse to Float.
        Type::NUMERIC => row.try_get::<_, Option<String>>(i).map(|o| match o {
            Some(s) => s.parse::<f64>().map(Cell::Float).unwrap_or(Cell::Text(s)),
            None => Cell::Null,
        }),
        // Text-family (TEXT/VARCHAR/BPCHAR/NAME/…): read as String.
        _ => row
            .try_get::<_, Option<String>>(i)
            .map(|o| o.map(Cell::Text).unwrap_or(Cell::Null)),
    };
    mapped.map_err(|e| {
        SqlError::Import(format!(
            "postgres column {i} ({ty}) unmapped: {}",
            redact_str(&e.to_string())
        ))
    })
}

pub(crate) fn pg_execute(client: &mut Client, sql: &str, params: &[Cell]) -> Result<()> {
    // Bind each Cell as a &dyn ToSql. Build owned adapters, then borrow them.
    let owned: Vec<PgParam> = params.iter().map(PgParam::from_cell).collect();
    let bound: Vec<&(dyn postgres::types::ToSql + Sync)> = owned
        .iter()
        .map(|p| p as &(dyn postgres::types::ToSql + Sync))
        .collect();
    client
        .execute(sql, &bound)
        .map_err(|e| SqlError::Export(format!("postgres execute failed: {}", redact_pg(e))))?;
    Ok(())
}

/// Owning parameter adapter so we can hand `&dyn ToSql` to the driver.
#[derive(Debug)]
enum PgParam {
    Null,
    Int(i64),
    Float(f64),
    Text(String),
    Bool(bool),
}

impl PgParam {
    fn from_cell(c: &Cell) -> PgParam {
        match c {
            Cell::Null => PgParam::Null,
            Cell::Int(i) => PgParam::Int(*i),
            Cell::Float(f) => PgParam::Float(*f),
            Cell::Text(t) => PgParam::Text(t.clone()),
            Cell::Bool(b) => PgParam::Bool(*b),
        }
    }
}

impl postgres::types::ToSql for PgParam {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut postgres::types::private::BytesMut,
    ) -> std::result::Result<postgres::types::IsNull, Box<dyn std::error::Error + Sync + Send>>
    {
        match self {
            PgParam::Null => Ok(postgres::types::IsNull::Yes),
            PgParam::Int(i) => i.to_sql(ty, out),
            PgParam::Float(f) => f.to_sql(ty, out),
            PgParam::Text(t) => t.to_sql(ty, out),
            PgParam::Bool(b) => b.to_sql(ty, out),
        }
    }

    fn accepts(_ty: &Type) -> bool {
        // Accept whatever the target column is; the concrete to_sql above will
        // error if the runtime type truly can't encode.
        true
    }

    postgres::types::to_sql_checked!();
}

/// Redact anything that looks like it could carry a secret from a free-form
/// string before it reaches an error/log. Cheap and conservative.
fn redact_str(s: &str) -> String {
    if s.contains("password") || s.contains("://") {
        "(redacted)".to_string()
    } else {
        s.to_string()
    }
}
