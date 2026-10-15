//! Postgres backend (synchronous `postgres` crate).
//!
//! Credentials are **out of band**: `connect_postgres` takes a full connection
//! string that the caller assembled at connect time (see `conn::open`, which
//! injects the password from an env var). This module never logs a connection
//! string — a URI can carry a secret, so errors are redacted.

use crate::backend::{Backend, Cell, Table};
use crate::{Result, SqlError};
use postgres::config::{Host, SslMode};
use postgres::types::Type;
use postgres::{Client, Config, NoTls};

/// Open a Postgres connection from a full libpq/URI connection string.
///
/// The string may embed a password; **do not log it**. Prefer building it via
/// `conn::open`, which pulls the secret from the environment. On failure the
/// error is redacted so no secret leaks into logs.
///
/// **No TLS is built in.** To keep the password off the wire in cleartext the
/// connection is refused (fail closed, before any network I/O) unless every
/// host is loopback or a Unix socket, or the string explicitly says
/// `sslmode=disable`. `sslmode=require` is refused too, since it cannot be
/// honoured. Reach a remote server over an SSH tunnel or a local proxy.
pub fn connect_postgres(conn_str: &str) -> Result<Backend> {
    let config: Config = conn_str
        .parse()
        .map_err(|_| SqlError::Connect("invalid postgres connection string (redacted)".into()))?;
    check_pg_transport(&config)?;
    let client = config
        .connect(NoTls)
        .map_err(|e| SqlError::Connect(format!("postgres connect failed: {}", redact_pg(e))))?;
    Ok(Backend::Postgres(client))
}

/// Refuse a connection that would send credentials in cleartext off-host, or
/// that asks for TLS this build cannot provide. See [`connect_postgres`].
fn check_pg_transport(config: &Config) -> Result<()> {
    let no_tls = |why: &str| {
        Err(SqlError::Connect(format!(
            "postgres: TLS isn't built into this build of Improv; {why}"
        )))
    };
    let mode = config.get_ssl_mode();
    if mode == SslMode::Require {
        return no_tls("sslmode=require cannot be honoured (use an SSH tunnel to a loopback port)");
    }
    let host_is_local = |h: &Host| match h {
        Host::Tcp(name) => {
            name.eq_ignore_ascii_case("localhost")
                || name
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|ip| ip.is_loopback())
        }
        #[cfg(unix)]
        Host::Unix(_) => true,
    };
    let local = config.get_hosts().iter().all(host_is_local)
        && config.get_hostaddrs().iter().all(|ip| ip.is_loopback());
    if !local && mode != SslMode::Disable {
        return no_tls(
            "refusing a non-loopback host without an explicit sslmode=disable \
             (the password would cross the network in cleartext)",
        );
    }
    Ok(())
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
    // Run the query inside a READ ONLY transaction so even a data-modifying
    // CTE (`WITH x AS (DELETE ...) SELECT ...`) is refused by the server. The
    // transaction is rolled back on drop if anything below fails.
    let pg_err =
        |what: &str, e| SqlError::Import(format!("postgres {what} failed: {}", redact_pg(e)));
    let mut tx = client
        .build_transaction()
        .read_only(true)
        .start()
        .map_err(|e| pg_err("begin read only", e))?;
    // Prepare once: the statement carries the column names even for an empty
    // result.
    let stmt = tx.prepare(sql).map_err(|e| pg_err("prepare", e))?;
    let rows = tx.query(&stmt, &[]).map_err(|e| pg_err("query", e))?;
    let columns: Vec<String> = stmt
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();
    tx.commit().map_err(|e| pg_err("commit", e))?;

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
        // NUMERIC arrives in its binary wire form; decode it to Float.
        Type::NUMERIC => row
            .try_get::<_, Option<PgNumeric>>(i)
            .map(|o| o.map(|n| Cell::Float(n.0)).unwrap_or(Cell::Null)),
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

/// A Postgres `NUMERIC` read as `f64` (precision beyond f64 is lost, which is
/// what the model's numeric cells hold anyway).
struct PgNumeric(f64);

impl<'a> postgres::types::FromSql<'a> for PgNumeric {
    /// Binary layout: ndigits i16, weight i16, sign u16, dscale u16, then
    /// `ndigits` base-10000 digits (i16), the first worth `10000^weight`.
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> std::result::Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        let word =
            |k: usize| -> std::result::Result<i16, Box<dyn std::error::Error + Sync + Send>> {
                raw.get(2 * k..2 * k + 2)
                    .map(|b| i16::from_be_bytes([b[0], b[1]]))
                    .ok_or_else(|| "truncated numeric".into())
            };
        let (ndigits, weight, sign) = (word(0)?, word(1)?, word(2)? as u16);
        if sign == 0xC000 {
            return Ok(PgNumeric(f64::NAN));
        }
        let mut v = 0.0;
        for k in 0..ndigits.max(0) as usize {
            v += f64::from(word(4 + k)?) * 10000f64.powi(i32::from(weight) - k as i32);
        }
        Ok(PgNumeric(if sign == 0x4000 { -v } else { v }))
    }

    fn accepts(ty: &Type) -> bool {
        *ty == Type::NUMERIC
    }
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn transport(dsn: &str) -> Result<()> {
        check_pg_transport(&dsn.parse().unwrap())
    }

    #[test]
    fn transport_allows_loopback_and_sockets() {
        for ok in [
            "host=localhost user=u",
            "host=127.0.0.1 user=u",
            "host=::1 user=u",
            "postgresql://u@localhost:5433/db",
            "host=db.example.com user=u sslmode=disable",
        ] {
            assert!(transport(ok).is_ok(), "{ok:?}");
        }
        // A socket directory is a `Host::Unix` only on unix; elsewhere the
        // driver parses it as a TCP host name, which is not loopback.
        #[cfg(unix)]
        assert!(transport("host=/var/run/postgresql user=u").is_ok());
    }

    #[test]
    fn transport_refuses_cleartext_remote_and_unhonourable_tls() {
        for bad in [
            "host=db.example.com user=u",
            "host=10.0.0.5 user=u sslmode=prefer",
            "postgresql://u@db.example.com/db",
            "host=localhost,db.example.com user=u",
            "host=localhost hostaddr=10.0.0.5 user=u",
            "host=localhost user=u sslmode=require",
        ] {
            let err = transport(bad).unwrap_err().to_string();
            assert!(err.contains("TLS isn't built in"), "{bad:?}: {err}");
        }
        // Refused before any network I/O, through the public entry point.
        assert!(connect_postgres("host=db.example.com user=u").is_err());
        assert!(connect_postgres("not a dsn ===").is_err());
    }

    #[test]
    fn redact_str_scrubs_secrets() {
        assert_eq!(redact_str("password=x"), "(redacted)");
        assert_eq!(redact_str("postgres://u:p@h"), "(redacted)");
        assert_eq!(redact_str("plain"), "plain");
    }

    /// Live: needs `IMPROV_TEST_PG` (a libpq DSN to a scratch database).
    fn live() -> Option<Backend> {
        let Ok(dsn) = std::env::var("IMPROV_TEST_PG") else {
            println!("skipped: IMPROV_TEST_PG not set");
            return None;
        };
        Some(connect_postgres(&dsn).expect("connect IMPROV_TEST_PG"))
    }

    #[test]
    fn pg_maps_every_column_type() {
        let Some(mut be) = live() else { return };
        let t = be
            .query(
                "SELECT true AS b, 2::int2 AS s, 3::int4 AS i, 4::int8 AS l, \
                 1.5::float4 AS f, 2.5::float8 AS d, 3.25::numeric AS n, \
                 'x'::text AS t, NULL::int4 AS z, NULL::text AS zt",
            )
            .unwrap();
        assert_eq!(
            t.columns,
            ["b", "s", "i", "l", "f", "d", "n", "t", "z", "zt"]
        );
        assert_eq!(
            t.rows,
            vec![vec![
                Cell::Bool(true),
                Cell::Int(2),
                Cell::Int(3),
                Cell::Int(4),
                Cell::Float(1.5),
                Cell::Float(2.5),
                Cell::Float(3.25),
                Cell::Text("x".into()),
                Cell::Null,
                Cell::Null,
            ]]
        );
        // An empty result still reports its columns.
        let e = be.query("SELECT 1 AS a, 'q' AS b WHERE false").unwrap();
        assert_eq!(e.columns, ["a", "b"]);
        assert!(e.rows.is_empty());
        let n = be
            .query("SELECT -12345.678::numeric AS a, 0::numeric AS b, 'NaN'::numeric AS c")
            .unwrap();
        assert_eq!(n.rows[0][..2], [Cell::Float(-12345.678), Cell::Float(0.0)]);
        assert!(matches!(n.rows[0][2], Cell::Float(f) if f.is_nan()));
        // An unmappable type is an error, not a panic.
        assert!(be.query("SELECT now() AS ts").is_err());
        assert!(be.query("SELECT * FROM no_such_table").is_err());
    }

    #[test]
    fn pg_refresh_query_runs_read_only() {
        let Some(mut be) = live() else { return };
        be.execute("DROP TABLE IF EXISTS improv_pg_ro", &[])
            .unwrap();
        be.execute("CREATE TABLE improv_pg_ro (v int8)", &[])
            .unwrap();
        be.execute("INSERT INTO improv_pg_ro VALUES ($1)", &[Cell::Int(7)])
            .unwrap();
        // A data-modifying CTE passes the keyword check but the READ ONLY
        // transaction refuses it server-side.
        let cte = "WITH x AS (DELETE FROM improv_pg_ro RETURNING *) SELECT * FROM x";
        assert!(crate::check_read_only_sql(cte).is_ok());
        assert!(be.query(cte).is_err());
        assert!(be.query("DELETE FROM improv_pg_ro").is_err());
        // The connection is still usable and the row survived.
        let t = be.query("SELECT v FROM improv_pg_ro").unwrap();
        assert_eq!(t.rows, vec![vec![Cell::Int(7)]]);
        be.execute("DROP TABLE improv_pg_ro", &[]).unwrap();
    }

    #[test]
    fn numeric_decoder_rejects_truncated_input() {
        use postgres::types::FromSql;
        let err = PgNumeric::from_sql(&Type::NUMERIC, &[0, 1]).err().unwrap();
        assert_eq!(err.to_string(), "truncated numeric");
        // ndigits=1 but the digit word is missing.
        let err = PgNumeric::from_sql(&Type::NUMERIC, &[0, 1, 0, 0, 0, 0, 0, 0])
            .err()
            .unwrap();
        assert_eq!(err.to_string(), "truncated numeric");
        assert!(PgNumeric::accepts(&Type::NUMERIC) && !PgNumeric::accepts(&Type::TEXT));
    }

    /// Nothing listens on loopback port 1: a non-database (I/O) error, redacted.
    #[test]
    fn refused_connection_is_redacted() {
        let Err(err) = connect_postgres("host=127.0.0.1 port=1 user=u password=hunter2") else {
            panic!("connected to port 1");
        };
        let msg = err.to_string();
        assert!(msg.contains("connection error (redacted)"), "{msg}");
        assert!(!msg.contains("hunter2"), "{msg}");
    }

    #[test]
    fn pg_binds_null_and_bool_and_redacts_execute_errors() {
        let Some(mut be) = live() else { return };
        be.execute("DROP TABLE IF EXISTS improv_pg_params", &[])
            .unwrap();
        be.execute("CREATE TABLE improv_pg_params (a int8, b bool)", &[])
            .unwrap();
        be.execute(
            "INSERT INTO improv_pg_params VALUES ($1, $2), ($3, $4)",
            &[Cell::Null, Cell::Bool(true), Cell::Int(1), Cell::Null],
        )
        .unwrap();
        let t = be
            .query("SELECT a, b FROM improv_pg_params ORDER BY a NULLS FIRST")
            .unwrap();
        assert_eq!(
            t.rows,
            [[Cell::Null, Cell::Bool(true)], [Cell::Int(1), Cell::Null]]
        );
        let err = be
            .execute("INSERT INTO no_such_table VALUES (1)", &[])
            .unwrap_err();
        assert!(
            err.to_string().contains("database error (redacted)"),
            "{err}"
        );
        be.execute("DROP TABLE improv_pg_params", &[]).unwrap();
    }
}
