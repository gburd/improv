//! Connection descriptors with **credentials handled out of band**.
//!
//! A [`Connection`] is a serializable descriptor (persisted by callers, e.g. as
//! datoms on the model). It NEVER stores a password. For Postgres it stores a
//! DSN *without* the password plus the NAME of an environment variable holding
//! the secret (default `PGPASSWORD`). [`open`] resolves the secret from the
//! environment at connect time and injects it, and never logs the assembled
//! string. A serde round-trip of a `Connection` therefore contains no secret.

use crate::backend::Backend;
use crate::{Result, SqlError};
use serde::{Deserialize, Serialize};

/// A saved connection. Serializable and secret-free by construction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub kind: ConnKind,
}

/// The backend + its *non-secret* locator. Secrets live in the environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ConnKind {
    /// SQLite: a filesystem path (or `:memory:`). No secret.
    Sqlite { path: String },
    /// DuckDB: a filesystem path (or `:memory:`). Embedded, no secret.
    Duckdb { path: String },
    /// Postgres: a DSN/URI **without** a password, plus the env var that holds
    /// the password. The secret is resolved at connect time, never stored.
    Postgres {
        /// libpq DSN or `postgres://user@host/db` — MUST NOT contain a password.
        uri: String,
        /// Name of the env var holding the password. Defaults to `PGPASSWORD`.
        #[serde(default = "default_password_env")]
        password_env: String,
    },
}

fn default_password_env() -> String {
    "PGPASSWORD".to_string()
}

/// Assemble a live [`Backend`] from a descriptor, pulling any secret from the
/// environment. The secret is never logged and never written back to the
/// descriptor.
pub fn open(c: &Connection) -> Result<Backend> {
    match &c.kind {
        ConnKind::Sqlite { path } => {
            let conn = if path == ":memory:" {
                rusqlite::Connection::open_in_memory()?
            } else {
                rusqlite::Connection::open(path)?
            };
            Ok(Backend::Sqlite(conn))
        }
        ConnKind::Duckdb { path } => crate::duck::connect_duckdb(path),
        ConnKind::Postgres { uri, password_env } => {
            // libpq-style: append `password=...` as a keyword only if the env var
            // is set. The `postgres` crate accepts both URI and keyword/value
            // DSNs; keyword form composes cleanly and keeps the secret out of the
            // stored `uri`.
            let dsn = match std::env::var(password_env) {
                Ok(pw) if !pw.is_empty() => format!("{uri} password={pw}"),
                _ => uri.clone(),
            };
            crate::pg::connect_postgres(&dsn)
        }
    }
}

/// Convenience: a Postgres descriptor naming an env var for its secret.
pub fn postgres(id: &str, name: &str, uri: &str, password_env: &str) -> Connection {
    Connection {
        id: id.to_string(),
        name: name.to_string(),
        kind: ConnKind::Postgres {
            uri: uri.to_string(),
            password_env: password_env.to_string(),
        },
    }
}

/// Convenience: a SQLite descriptor.
pub fn sqlite(id: &str, name: &str, path: &str) -> Connection {
    Connection {
        id: id.to_string(),
        name: name.to_string(),
        kind: ConnKind::Sqlite {
            path: path.to_string(),
        },
    }
}

/// Convenience: a DuckDB descriptor.
pub fn duckdb(id: &str, name: &str, path: &str) -> Connection {
    Connection {
        id: id.to_string(),
        name: name.to_string(),
        kind: ConnKind::Duckdb {
            path: path.to_string(),
        },
    }
}

// Reject descriptors that smell like they embed a secret, so a caller can't
// accidentally persist one. Not a security boundary (a URI is opaque), just a
// guard rail.
pub fn assert_no_inline_secret(c: &Connection) -> Result<()> {
    if let ConnKind::Postgres { uri, .. } = &c.kind {
        if uri.contains("password=")
            || (uri.contains("://") && uri.contains(':') && uri.contains('@'))
        {
            // `postgres://user:secret@host` embeds a password.
            if uri
                .split('@')
                .next()
                .map(|a| a.matches(':').count())
                .unwrap_or(0)
                > 1
            {
                return Err(SqlError::Connect(
                    "connection URI appears to embed a password; store it in an env var instead"
                        .into(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn postgres_descriptor_serde_has_no_secret() {
        // A descriptor names an env var; the actual password lives only in the
        // environment. Even if someone set PGPASSWORD, it must not serialize.
        let c = postgres(
            "c1",
            "SalesDB",
            "postgres://app@db.internal:5432/sales",
            "SALES_DB_PASSWORD",
        );
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("password="), "no inline password kv");
        assert!(json.contains("SALES_DB_PASSWORD"), "names the env var");
        // The env var NAME is fine to store; a value never appears.
        assert!(!json.to_lowercase().contains("secret"));

        // Round-trips identically (secret-free).
        let back: Connection = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn password_env_defaults_to_pgpassword() {
        let json = r#"{"id":"c2","name":"X","kind":{"type":"postgres","uri":"postgres://u@h/db"}}"#;
        let c: Connection = serde_json::from_str(json).unwrap();
        match c.kind {
            ConnKind::Postgres { password_env, .. } => assert_eq!(password_env, "PGPASSWORD"),
            _ => panic!("expected postgres"),
        }
    }

    #[test]
    fn duckdb_descriptor_serde_round_trips() {
        let c = duckdb("d1", "Analytics", "/tmp/warehouse.duckdb");
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains("duckdb"), "tagged as duckdb");
        assert!(json.contains("warehouse.duckdb"), "path preserved");
        let back: Connection = serde_json::from_str(&json).unwrap();
        assert_eq!(back, c);
        match back.kind {
            ConnKind::Duckdb { path } => assert_eq!(path, "/tmp/warehouse.duckdb"),
            _ => panic!("expected duckdb"),
        }
    }

    #[test]
    fn rejects_inline_password_uri() {
        let c = postgres("c3", "Bad", "postgres://u:hunter2@h/db", "PGPASSWORD");
        assert!(assert_no_inline_secret(&c).is_err());
        let ok = postgres("c4", "Good", "postgres://u@h/db", "PGPASSWORD");
        assert!(assert_no_inline_secret(&ok).is_ok());
    }

    #[test]
    fn open_dispatches_on_kind() {
        let mut mem = open(&sqlite("m", "mem", ":memory:")).unwrap();
        assert!(matches!(mem, Backend::Sqlite(_)));
        assert_eq!(
            mem.query("SELECT 1 AS one").unwrap().rows,
            [[crate::Cell::Int(1)]]
        );
        let duck = open(&duckdb("d", "duck", ":memory:")).unwrap();
        assert!(matches!(duck, Backend::Duckdb(_)));
    }

    /// The password is appended from the env var only when it is non-empty;
    /// either way a remote cleartext host is refused before any network I/O.
    #[test]
    fn open_postgres_resolves_password_env() {
        let var = format!("IMPROV_CONN_TEST_PW_{}", std::process::id());
        let remote = postgres("p", "remote", "host=db.example.com user=u", &var);
        std::env::set_var(&var, "s3cret");
        let with_pw = open(&remote).err().expect("refused").to_string();
        std::env::set_var(&var, "");
        let empty_pw = open(&remote).err().expect("refused").to_string();
        std::env::remove_var(&var);
        let unset = open(&remote).err().expect("refused").to_string();
        for e in [&with_pw, &empty_pw, &unset] {
            assert!(e.contains("TLS isn't built in"), "{e}");
            assert!(!e.contains("s3cret"), "{e}");
        }
    }

    #[test]
    fn inline_secret_guard_shapes() {
        // Non-Postgres descriptors never carry a secret.
        assert!(assert_no_inline_secret(&sqlite("s", "s", "/tmp/x.db")).is_ok());
        // A URI with user:password before `@` is refused.
        let uri = postgres("k", "k", "postgresql://u:p@h:5432/db", "PGPASSWORD");
        assert!(assert_no_inline_secret(&uri).is_err());
        // URI with a port but no password: one colon before `@` is fine.
        let port = postgres("u", "u", "postgres://u@h:5432/db", "PGPASSWORD");
        assert!(assert_no_inline_secret(&port).is_ok());
        // URI without `@` / keyword DSN without a password pass.
        let no_at = postgres("n", "n", "postgres://h:5432/db", "PGPASSWORD");
        assert!(assert_no_inline_secret(&no_at).is_ok());
        let plain = postgres("p", "p", "host=h user=u", "PGPASSWORD");
        assert!(assert_no_inline_secret(&plain).is_ok());
    }
}
