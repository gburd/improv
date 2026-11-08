//! The Postgres transport, shared by every Improv connection to Postgres
//! (this crate's model store and `improv_storage_sql`'s data sources):
//! `sslmode`, `sslrootcert`, certificate verification, and TLS by rustls on
//! the ring provider. Design: `docs/reviews/2026-11-02-cedar-policies.md`,
//! "TLS".
//!
//! | Host \ `sslmode` | `disable` | `prefer` (default) | `require`, `verify-full` | `verify-ca` |
//! |---|---|---|---|---|
//! | Unix socket | cleartext | cleartext | cleartext | cleartext |
//! | loopback (`localhost`, 127.0.0.1, ::1) | cleartext | cleartext | TLS, name checked | TLS, chain only |
//! | anything else | cleartext | TLS, name checked | TLS, name checked | TLS, chain only |
//!
//! TLS always verifies the server's certificate: against `sslrootcert=FILE`
//! when given (that CA only), else against the operating system's roots
//! (`rustls-platform-verifier`). So `require` behaves as `verify-full`,
//! which is stricter than libpq. `verify-ca` skips the name check, and so
//! needs `sslrootcert=FILE`: with public roots it would accept any site's
//! certificate. Off the host, `prefer` never falls back to cleartext: a
//! server without TLS is refused before the startup message (user,
//! database) is sent, and only an explicit `sslmode=disable` sends
//! cleartext. Over TLS, SCRAM uses channel binding when the server offers
//! it (`channel_binding=prefer`, the driver's default).

use postgres::config::{Host, SslMode as PgSslMode};
use postgres::{Client, Config, NoTls};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::WebPkiSupportedAlgorithms;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use rustls_platform_verifier::BuilderVerifierExt;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Why [`connect`] failed. Every message is safe to show: none carries the
/// connection string or anything from it but host names and file paths.
#[derive(Debug)]
pub enum ConnectError {
    /// Refused before anything was sent: the connection string, its TLS
    /// parameters or the certificates it names.
    Refused(String),
    /// The TLS handshake failed, or the server offers no TLS; the startup
    /// message (user, database) and the password were not sent.
    Tls(String),
    /// Any other error from the driver: callers redact it.
    Driver(postgres::Error),
}

/// Connect with the transport the string's host and `sslmode` call for (the
/// table above). `conn` is a libpq keyword/value string or a `postgres://`
/// URL; Improv reads `sslmode` and `sslrootcert` itself and passes the rest
/// to the driver.
pub fn connect(conn: &str) -> Result<Client, ConnectError> {
    let invalid = || ConnectError::Refused("invalid postgres connection string (redacted)".into());
    let keys = ["sslmode", "sslrootcert", "sslcert", "sslkey"];
    let (rest, taken) = take_params(conn, &keys).ok_or_else(invalid)?;
    let (mut mode, mut root) = (SslMode::Prefer, RootCert::System);
    for (key, value) in taken {
        match key.as_str() {
            "sslmode" => mode = value.parse().map_err(ConnectError::Refused)?,
            "sslrootcert" => root = RootCert::from(value),
            _ => {
                return Err(ConnectError::Refused(format!(
                    "{key}: client certificates are not supported yet"
                )))
            }
        }
    }
    let mut config: Config = rest.parse().map_err(|_| invalid())?;
    match transport(mode, hosts(&config), &root).map_err(ConnectError::Refused)? {
        Transport::Plain => {
            config.ssl_mode(PgSslMode::Disable);
            config.connect(NoTls).map_err(ConnectError::Driver)
        }
        Transport::Tls { check_name } => {
            if config.get_hosts().is_empty() {
                return Err(ConnectError::Refused(
                    "TLS needs host=NAME, the name the server's certificate is checked \
                     against (hostaddr= alone is not enough)"
                        .into(),
                ));
            }
            let tls = client_config(check_name, &root).map_err(ConnectError::Refused)?;
            // Require: no cleartext fallback when the server answers "no TLS".
            config.ssl_mode(PgSslMode::Require);
            config
                .connect(tokio_postgres_rustls::MakeRustlsConnect::new(tls))
                .map_err(|e| match tls_failure(&e) {
                    Some(why) => ConnectError::Tls(why),
                    None => ConnectError::Driver(e),
                })
        }
    }
}

/// `sslmode`, as Improv reads it (the driver knows only the first three).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SslMode {
    Disable,
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl std::str::FromStr for SslMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "disable" => Self::Disable,
            "prefer" => Self::Prefer,
            "require" => Self::Require,
            "verify-ca" => Self::VerifyCa,
            "verify-full" => Self::VerifyFull,
            // libpq's `allow` tries cleartext first, which Improv never does.
            _ => {
                return Err(format!(
                    "unsupported sslmode {s:?} (use disable, prefer, require, verify-ca or \
                     verify-full)"
                ))
            }
        })
    }
}

/// What `sslrootcert` names: a PEM file of CAs, or (unset, empty or
/// `system`, as in libpq 16) the operating system's roots.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RootCert {
    System,
    File(PathBuf),
}

impl From<String> for RootCert {
    fn from(v: String) -> Self {
        match v.as_str() {
            "" | "system" => Self::System,
            _ => Self::File(v.into()),
        }
    }
}

/// Where the hosts are, the farthest of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Hosts {
    /// Unix sockets only.
    Sockets,
    /// Loopback (and maybe sockets).
    Loopback,
    /// At least one host off this machine.
    Remote,
}

fn hosts(config: &Config) -> Hosts {
    let tcp = |loopback: bool| {
        if loopback {
            Hosts::Loopback
        } else {
            Hosts::Remote
        }
    };
    let names = config.get_hosts().iter().map(|h| match h {
        Host::Tcp(name) => tcp(name.eq_ignore_ascii_case("localhost")
            || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())),
        #[cfg(unix)]
        Host::Unix(_) => Hosts::Sockets,
    });
    let addrs = config
        .get_hostaddrs()
        .iter()
        .map(|ip| tcp(ip.is_loopback()));
    names.chain(addrs).max().unwrap_or(Hosts::Sockets)
}

/// What a connection does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Transport {
    Plain,
    Tls { check_name: bool },
}

/// The table in the module docs.
fn transport(mode: SslMode, hosts: Hosts, root: &RootCert) -> Result<Transport, String> {
    let check_name = match (hosts, mode) {
        // libpq does not use TLS on a Unix socket either: nothing crosses a network.
        (Hosts::Sockets, _) | (_, SslMode::Disable) | (Hosts::Loopback, SslMode::Prefer) => {
            return Ok(Transport::Plain)
        }
        (_, SslMode::VerifyCa) => false,
        _ => true,
    };
    if !check_name && *root == RootCert::System {
        return Err(
            "sslmode=verify-ca needs sslrootcert=FILE: against the operating \
                    system's roots, a certificate for any name would pass (use verify-full)"
                .into(),
        );
    }
    Ok(Transport::Tls { check_name })
}

/// Remove the parameters named `keys` from a keyword/value string or a URL,
/// returning the rest (unchanged otherwise) and the removed ones, decoded,
/// in order. `None`: the string does not parse.
pub(crate) fn take_params(conn: &str, keys: &[&str]) -> Option<(String, Vec<(String, String)>)> {
    let mut taken = Vec::new();
    let url = ["postgres://", "postgresql://"]
        .iter()
        .find_map(|p| conn.strip_prefix(p));
    let Some(after) = url else {
        let mut kept = Vec::new();
        for (key, value, span) in keywords(conn)? {
            if keys.contains(&key) {
                taken.push((key.to_string(), value));
            } else {
                kept.push(&conn[span]);
            }
        }
        return Some((kept.join(" "), taken));
    };
    // As the driver reads a URL: user info runs to the first '@', and the
    // query starts at the first '?' after it.
    let from = conn.len() - after.len() + after.find('@').map_or(0, |at| at + 1);
    let Some(q) = conn[from..].find('?').map(|q| from + q) else {
        return Some((conn.to_string(), taken));
    };
    let decode = |s: &str| {
        percent_encoding::percent_decode_str(s)
            .decode_utf8()
            .ok()
            .map(|s| s.into_owned())
    };
    let mut kept = Vec::new();
    for p in conn[q + 1..].split('&') {
        let (key, value) = p.split_once('=').unwrap_or((p, ""));
        let key = decode(key)?;
        if keys.contains(&key.as_str()) {
            taken.push((key, decode(value)?));
        } else {
            kept.push(p);
        }
    }
    let base = &conn[..q];
    Some(match kept.is_empty() {
        true => (base.to_string(), taken),
        false => (format!("{base}?{}", kept.join("&")), taken),
    })
}

/// libpq's keyword/value grammar: `key = value`, a value plain or in single
/// quotes, a backslash escaping the next character. Each parameter with its
/// span in `conn`.
fn keywords(conn: &str) -> Option<Vec<(&str, String, std::ops::Range<usize>)>> {
    let b = conn.as_bytes();
    let mut i = 0;
    let skip_ws = |i: &mut usize| {
        while b.get(*i).is_some_and(u8::is_ascii_whitespace) {
            *i += 1;
        }
    };
    let mut out = Vec::new();
    loop {
        skip_ws(&mut i);
        if i == b.len() {
            return Some(out);
        }
        let start = i;
        while b
            .get(i)
            .is_some_and(|c| !c.is_ascii_whitespace() && *c != b'=')
        {
            i += 1;
        }
        let key = &conn[start..i];
        skip_ws(&mut i);
        if b.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        skip_ws(&mut i);
        let quoted = b.get(i) == Some(&b'\'');
        i += usize::from(quoted);
        let mut value = Vec::new();
        loop {
            match b.get(i) {
                None if quoted => return None,
                None => break,
                Some(b'\'') if quoted => {
                    i += 1;
                    break;
                }
                Some(c) if !quoted && c.is_ascii_whitespace() => break,
                Some(b'\\') => {
                    i += 1;
                    value.push(*b.get(i)?);
                }
                Some(&c) => value.push(c),
            }
            i += 1;
        }
        out.push((key, String::from_utf8(value).ok()?, start..i));
    }
}

fn client_config(check_name: bool, root: &RootCert) -> Result<ClientConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let algs = provider.signature_verification_algorithms;
    let builder = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let builder = match root {
        RootCert::System => builder.with_platform_verifier().map_err(|e| {
            format!(
                "cannot load the operating system's CA certificates ({e}); name the CA with \
                 sslrootcert=FILE"
            )
        })?,
        RootCert::File(path) if check_name => builder.with_root_certificates(load_roots(path)?),
        RootCert::File(path) => builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(ChainOnly {
                roots: Arc::new(load_roots(path)?),
                algs,
            })),
    };
    let mut config = builder.with_no_client_auth();
    // What libpq 17 sends; needed for sslnegotiation=direct.
    config.alpn_protocols = vec![b"postgresql".to_vec()];
    Ok(config)
}

fn load_roots(path: &Path) -> Result<RootCertStore, String> {
    let fail = |e: &dyn std::fmt::Display| format!("sslrootcert {}: {e}", path.display());
    let mut roots = RootCertStore::empty();
    for cert in CertificateDer::pem_file_iter(path).map_err(|e| fail(&e))? {
        roots
            .add(cert.map_err(|e| fail(&e))?)
            .map_err(|e| fail(&e))?;
    }
    if roots.is_empty() {
        return Err(fail(&"no certificate in it"));
    }
    Ok(roots)
}

/// `verify-ca`: the chain must lead to a CA of `sslrootcert`; the name is
/// not checked. The signatures are checked as for `verify-full`.
#[derive(Debug)]
struct ChainOnly {
    roots: Arc<RootCertStore>,
    algs: WebPkiSupportedAlgorithms,
}

impl ServerCertVerifier for ChainOnly {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let cert = rustls::server::ParsedCertificate::try_from(end_entity)?;
        rustls::client::verify_server_cert_signed_by_trust_anchor(
            &cert,
            &self.roots,
            intermediates,
            now,
            self.algs.all,
        )?;
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.algs)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.algs)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.algs.supported_schemes()
    }
}

/// Why a TLS handshake failed: the certificate problem, or the server's
/// refusal of TLS. `None`: not a TLS failure.
fn tls_failure(e: &postgres::Error) -> Option<String> {
    use std::error::Error as _;
    let cause = e.source()?;
    let rustls = cause
        .downcast_ref::<std::io::Error>()
        .and_then(|io| io.get_ref()?.downcast_ref::<rustls::Error>());
    if let Some(tls) = rustls {
        return Some(describe(tls));
    }
    // The driver's error kinds are private: its TLS errors read so.
    (e.to_string() == "error performing TLS handshake").then(|| match cause.to_string().as_str() {
        "server does not support TLS" => "the server does not offer TLS, and Improv does not \
                                          fall back to cleartext off the host"
            .into(),
        other => format!("TLS handshake failed: {other}"),
    })
}

fn describe(e: &rustls::Error) -> String {
    use rustls::CertificateError as C;
    let rustls::Error::InvalidCertificate(c) = e else {
        return format!("TLS handshake failed: {e}");
    };
    let what = match c {
        C::UnknownIssuer => "is not signed by a CA Improv trusts (sslrootcert=FILE names the \
                             CA; without it, the operating system's roots)"
            .into(),
        C::NotValidForNameContext {
            expected,
            presented,
        } => format!(
            "is not valid for {:?} (it names {})",
            expected.to_str(),
            match presented.is_empty() {
                true => "no host".into(),
                false => presented.join(", "),
            }
        ),
        C::NotValidForName => "is not valid for this host name".into(),
        C::Expired | C::ExpiredContext { .. } => "has expired".into(),
        C::NotValidYet | C::NotValidYetContext { .. } => "is not valid yet".into(),
        C::Revoked => "has been revoked".into(),
        other => format!("was refused: {other}"),
    };
    format!("the server's certificate {what}")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn sslmode_parses_the_five_modes_and_nothing_else() {
        for (s, m) in [
            ("disable", SslMode::Disable),
            ("prefer", SslMode::Prefer),
            ("require", SslMode::Require),
            ("verify-ca", SslMode::VerifyCa),
            ("verify-full", SslMode::VerifyFull),
        ] {
            assert_eq!(s.parse::<SslMode>(), Ok(m));
        }
        for bad in ["allow", "", "Require", "verify_full"] {
            let e = bad.parse::<SslMode>().unwrap_err();
            assert!(e.contains("unsupported sslmode"), "{bad}: {e}");
        }
        assert_eq!(RootCert::from(String::new()), RootCert::System);
        assert_eq!(RootCert::from("system".to_string()), RootCert::System);
        assert_eq!(
            RootCert::from("/ca.pem".to_string()),
            RootCert::File("/ca.pem".into())
        );
    }

    #[test]
    fn take_params_strips_keywords_and_url_parameters() {
        let keys = &["sslmode", "sslrootcert"];
        let take = |s: &str| take_params(s, keys).unwrap();
        assert_eq!(
            take("host=h sslmode=verify-full  user=u sslrootcert='/a b/c\\'d.pem' x = 1"),
            (
                "host=h user=u x = 1".to_string(),
                vec![
                    ("sslmode".into(), "verify-full".into()),
                    ("sslrootcert".into(), "/a b/c'd.pem".into())
                ]
            )
        );
        assert_eq!(take(""), (String::new(), vec![]));
        assert_eq!(take("host=h"), ("host=h".to_string(), vec![]));
        assert_eq!(
            take("postgres://u:p%3F@h/db?sslmode=require&connect_timeout=5&sslrootcert=%2Fca.pem"),
            (
                "postgres://u:p%3F@h/db?connect_timeout=5".to_string(),
                vec![
                    ("sslmode".into(), "require".into()),
                    ("sslrootcert".into(), "/ca.pem".into())
                ]
            )
        );
        // A '?' in the password is before the '@': not the query.
        assert_eq!(
            take("postgresql://u:a?b@h/db?sslmode=disable"),
            (
                "postgresql://u:a?b@h/db".to_string(),
                vec![("sslmode".into(), "disable".into())]
            )
        );
        assert_eq!(take("postgres://h/db").0, "postgres://h/db");
        assert_eq!(take("postgres://h/db?x").0, "postgres://h/db?x");
        // Values of other parameters are the driver's to decode (and refuse).
        assert_eq!(take("postgres://h?k=%ff").0, "postgres://h?k=%ff");
        for bad in [
            "host",
            "host='unterminated",
            "a=\\",
            "postgres://h?sslmode=%ff",
            "postgres://h?%ff=1",
        ] {
            assert!(take_params(bad, keys).is_none(), "{bad}");
        }
    }

    fn cfg(s: &str) -> Config {
        s.parse().unwrap()
    }

    #[test]
    fn hosts_are_the_farthest_of_names_and_addresses() {
        for (s, want) in [
            ("host=localhost", Hosts::Loopback),
            ("host=LocalHost", Hosts::Loopback),
            ("host=127.0.0.1", Hosts::Loopback),
            ("host=::1", Hosts::Loopback),
            ("hostaddr=127.0.0.1", Hosts::Loopback),
            ("host=db.example.com", Hosts::Remote),
            ("host=10.0.0.5", Hosts::Remote),
            ("host=localhost,db.example.com", Hosts::Remote),
            ("host=localhost hostaddr=10.0.0.5", Hosts::Remote),
            ("host=db.example.com hostaddr=127.0.0.1", Hosts::Remote),
            ("user=u", Hosts::Sockets),
        ] {
            assert_eq!(hosts(&cfg(s)), want, "{s}");
        }
        #[cfg(unix)]
        {
            assert_eq!(hosts(&cfg("host=/run/postgresql")), Hosts::Sockets);
            assert_eq!(
                hosts(&cfg("host=/run/postgresql,localhost")),
                Hosts::Loopback
            );
        }
    }

    /// Every host kind against every sslmode, with and without sslrootcert.
    #[test]
    fn transport_decision_table() {
        use SslMode::*;
        use Transport::*;
        let full = Ok(Tls { check_name: true });
        let ca = Ok(Tls { check_name: false });
        let refused = Err(());
        #[rustfmt::skip]
        let table = [
            //                 disable    prefer       require      verify-ca  verify-full
            (Hosts::Sockets,  [Ok(Plain), Ok(Plain),   Ok(Plain),   Ok(Plain), Ok(Plain)], [Ok(Plain), Ok(Plain), Ok(Plain), Ok(Plain), Ok(Plain)]),
            (Hosts::Loopback, [Ok(Plain), Ok(Plain),   full,        ca,        full],      [Ok(Plain), Ok(Plain), full, refused, full]),
            (Hosts::Remote,   [Ok(Plain), full,        full,        ca,        full],      [Ok(Plain), full, full, refused, full]),
        ];
        let file = RootCert::File("ca.pem".into());
        for (hosts, with_file, system) in table {
            for (i, mode) in [Disable, Prefer, Require, VerifyCa, VerifyFull]
                .into_iter()
                .enumerate()
            {
                let got = |root| transport(mode, hosts, root).map_err(|_| ());
                assert_eq!(
                    got(&file),
                    with_file[i],
                    "{hosts:?} {mode:?} sslrootcert=FILE"
                );
                assert_eq!(
                    got(&RootCert::System),
                    system[i],
                    "{hosts:?} {mode:?} system roots"
                );
            }
        }
        let e = transport(VerifyCa, Hosts::Remote, &RootCert::System).unwrap_err();
        assert!(e.contains("verify-ca needs sslrootcert"), "{e}");
    }

    /// A CA and a server certificate for `names`, signed by it.
    pub(crate) struct Pki {
        pub ca_pem: String,
        cert: CertificateDer<'static>,
        key: rustls::pki_types::PrivateKeyDer<'static>,
    }

    impl Pki {
        pub(crate) fn new(names: &[&str]) -> Self {
            Self::with(names, |_| {})
        }

        fn with(names: &[&str], edit: impl FnOnce(&mut rcgen::CertificateParams)) -> Self {
            let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
            ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
            // A name of its own: a second CA with the same name would read
            // as a bad signature, not an unknown issuer.
            static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            ca.distinguished_name
                .push(rcgen::DnType::CommonName, format!("Improv test CA {n}"));
            let ca_key = rcgen::KeyPair::generate().unwrap();
            let ca = rcgen::CertifiedIssuer::self_signed(ca, ca_key).unwrap();
            let mut leaf = rcgen::CertificateParams::new(
                names.iter().map(|n| n.to_string()).collect::<Vec<_>>(),
            )
            .unwrap();
            edit(&mut leaf);
            let key = rcgen::KeyPair::generate().unwrap();
            let cert = leaf.signed_by(&key, &ca).unwrap();
            Pki {
                ca_pem: ca.pem(),
                cert: cert.der().clone(),
                key: rustls::pki_types::PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
            }
        }

        /// Write the CA's PEM to a file of its own; returns its path.
        pub(crate) fn ca_file(&self, tag: &str) -> PathBuf {
            let p = std::env::temp_dir().join(format!(
                "improv-tls-{tag}-{}-{:?}.pem",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::write(&p, &self.ca_pem).unwrap();
            p
        }
    }

    /// What a fake Postgres saw: whether it answered the SSLRequest with
    /// TLS, and the bytes it read after the handshake (or the refusal).
    struct Seen {
        tls_done: bool,
        after: usize,
    }

    /// A server on 127.0.0.1 for one connection: it reads the SSLRequest,
    /// answers 'S' and serves `pki`'s certificate (or 'N' without one), and
    /// counts what arrives next.
    fn fake_server(pki: Option<&Pki>) -> (u16, std::thread::JoinHandle<Seen>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let tls = pki.map(|p| {
            Arc::new(
                rustls::ServerConfig::builder_with_provider(Arc::new(
                    rustls::crypto::ring::default_provider(),
                ))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_no_client_auth()
                .with_single_cert(vec![p.cert.clone()], p.key.clone_key())
                .unwrap(),
            )
        });
        let h = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut req = [0u8; 8];
            s.read_exact(&mut req).unwrap();
            assert_eq!(req, [0, 0, 0, 8, 4, 210, 22, 47], "an SSLRequest");
            let mut rest = Vec::new();
            let Some(tls) = tls else {
                s.write_all(b"N").unwrap();
                let _ = s.read_to_end(&mut rest);
                return Seen {
                    tls_done: false,
                    after: rest.len(),
                };
            };
            s.write_all(b"S").unwrap();
            let mut conn = rustls::ServerConnection::new(tls).unwrap();
            let tls_done = conn.complete_io(&mut s).is_ok() && !conn.is_handshaking();
            // One read: the startup message, then hang up (the client waits
            // for an answer, so reading to EOF would wait out the timeout).
            let mut buf = [0u8; 512];
            let after = match tls_done {
                true => rustls::StreamOwned::new(conn, s)
                    .read(&mut buf)
                    .unwrap_or(0),
                false => 0,
            };
            Seen { tls_done, after }
        });
        (port, h)
    }

    fn connect_err(conn: &str) -> ConnectError {
        match connect(conn) {
            Ok(_) => panic!("connected to a fake server"),
            Err(e) => e,
        }
    }

    fn tls_err(e: ConnectError) -> String {
        match e {
            ConnectError::Tls(m) => m,
            ConnectError::Refused(m) => panic!("refused before connecting: {m}"),
            ConnectError::Driver(e) => panic!("not a TLS error: {e}"),
        }
    }

    /// Off the host (by name; the address is loopback), `prefer` asks for
    /// TLS, and a server that answers "no TLS" is refused before the
    /// startup message: no user, database or password is sent.
    #[test]
    fn prefer_off_the_host_never_falls_back_to_cleartext() {
        for mode in [
            "",
            "sslmode=prefer",
            "sslmode=require",
            "sslmode=verify-full",
        ] {
            let (port, server) = fake_server(None);
            let e = tls_err(connect_err(&format!(
                "host=db.example.com hostaddr=127.0.0.1 port={port} user=u password=hunter2 {mode}"
            )));
            assert!(e.contains("does not offer TLS"), "{mode}: {e}");
            assert!(!e.contains("hunter2"), "{e}");
            assert_eq!(
                server.join().unwrap().after,
                0,
                "{mode}: sent past the refusal"
            );
        }
    }

    #[test]
    fn verify_full_checks_the_ca_and_the_name() {
        let pki = Pki::new(&["db.improv.test"]);
        let ca = pki.ca_file("full");
        let at = |port: u16, name: &str, extra: &str| {
            format!(
                "host={name} hostaddr=127.0.0.1 port={port} user=u password=hunter2 \
                 sslrootcert={} {extra}",
                ca.display()
            )
        };
        // Right CA, right name: the handshake completes; the fake server
        // then hangs up, which the driver reports as its own error.
        for mode in ["sslmode=prefer", "sslmode=require", "sslmode=verify-full"] {
            let (port, server) = fake_server(Some(&pki));
            match connect_err(&at(port, "db.improv.test", mode)) {
                ConnectError::Driver(_) => {}
                ConnectError::Tls(m) | ConnectError::Refused(m) => panic!("{mode}: {m}"),
            }
            let seen = server.join().unwrap();
            assert!(
                seen.tls_done && seen.after > 0,
                "{mode}: no startup over TLS"
            );
        }
        // The wrong name.
        let (port, server) = fake_server(Some(&pki));
        let e = tls_err(connect_err(&at(
            port,
            "other.improv.test",
            "sslmode=verify-full",
        )));
        assert!(
            e.contains("not valid for \"other.improv.test\"") && e.contains("db.improv.test"),
            "{e}"
        );
        assert!(!e.contains("hunter2"), "{e}");
        assert_eq!(server.join().unwrap().after, 0);
        // verify-ca accepts the wrong name, not a wrong CA.
        let (port, server) = fake_server(Some(&pki));
        assert!(matches!(
            connect_err(&at(port, "other.improv.test", "sslmode=verify-ca")),
            ConnectError::Driver(_)
        ));
        assert!(server.join().unwrap().tls_done);
        std::fs::remove_file(ca).ok();
    }

    #[test]
    fn an_unknown_ca_is_refused() {
        let pki = Pki::new(&["db.improv.test"]);
        let other = Pki::new(&["db.improv.test"]).ca_file("other");
        for mode in ["verify-full", "verify-ca"] {
            let (port, server) = fake_server(Some(&pki));
            let e = tls_err(connect_err(&format!(
                "host=db.improv.test hostaddr=127.0.0.1 port={port} user=u \
                 sslmode={mode} sslrootcert={}",
                other.display()
            )));
            assert!(
                e.contains("not signed by a CA Improv trusts"),
                "{mode}: {e}"
            );
            assert_eq!(server.join().unwrap().after, 0);
        }
        // Nor do the operating system's roots know it (or none load).
        let (port, server) = fake_server(Some(&pki));
        match connect_err(&format!(
            "host=db.improv.test hostaddr=127.0.0.1 port={port} user=u"
        )) {
            // macOS and Windows report it in their own terms.
            ConnectError::Tls(m) => assert!(m.starts_with("the server's certificate"), "{m}"),
            ConnectError::Refused(m) => assert!(m.contains("operating system's CA"), "{m}"),
            ConnectError::Driver(e) => panic!("{e}"),
        }
        drop(server);
        std::fs::remove_file(other).ok();
    }

    #[test]
    fn an_expired_certificate_is_refused() {
        let pki = Pki::with(&["db.improv.test"], |p| {
            p.not_before = rcgen::date_time_ymd(2000, 1, 1);
            p.not_after = rcgen::date_time_ymd(2001, 1, 1);
        });
        let ca = pki.ca_file("expired");
        let (port, server) = fake_server(Some(&pki));
        let e = tls_err(connect_err(&format!(
            "host=db.improv.test hostaddr=127.0.0.1 port={port} user=u sslrootcert={}",
            ca.display()
        )));
        assert_eq!(e, "the server's certificate has expired");
        server.join().unwrap();
        std::fs::remove_file(ca).ok();
    }

    #[test]
    fn bad_parameters_are_refused_before_connecting() {
        let refused = |s: &str| match connect(s) {
            Err(ConnectError::Refused(m)) => m,
            Err(e) => panic!("{s}: not refused: {e:?}"),
            Ok(_) => panic!("{s}: connected"),
        };
        assert!(refused("host=h sslmode=allow").contains("unsupported sslmode"));
        assert!(refused("host=h password=hunter2 nonsense").contains("invalid"));
        assert!(!refused("host=h password=hunter2 nonsense").contains("hunter2"));
        assert!(refused("host=h unknown=1").contains("invalid"));
        assert!(refused("host=h sslkey=k.pem").starts_with("sslkey: client certificates"));
        assert!(refused("host=db.example.com sslmode=verify-ca").contains("sslrootcert"));
        assert!(refused("hostaddr=10.0.0.5 sslmode=require").contains("needs host=NAME"));
        let missing = refused("host=db.example.com sslrootcert=/nonexistent/ca.pem");
        assert!(
            missing.starts_with("sslrootcert /nonexistent/ca.pem: "),
            "{missing}"
        );
        let empty =
            std::env::temp_dir().join(format!("improv-tls-empty-{}.pem", std::process::id()));
        std::fs::write(&empty, "not a certificate\n").unwrap();
        let e = refused(&format!(
            "host=db.example.com sslrootcert={}",
            empty.display()
        ));
        assert!(e.ends_with("no certificate in it"), "{e}");
        std::fs::remove_file(empty).ok();
    }

    #[test]
    fn describe_names_the_certificate_problem() {
        use rustls::CertificateError as C;
        let d = |c: C| describe(&rustls::Error::InvalidCertificate(c));
        assert_eq!(
            d(C::NotValidForName),
            "the server's certificate is not valid for this host name"
        );
        assert_eq!(
            d(C::NotValidYet),
            "the server's certificate is not valid yet"
        );
        assert_eq!(d(C::Revoked), "the server's certificate has been revoked");
        assert_eq!(
            d(C::BadSignature),
            "the server's certificate was refused: BadSignature"
        );
        let none = d(C::NotValidForNameContext {
            expected: ServerName::try_from("h.test").unwrap().to_owned(),
            presented: vec![],
        });
        assert_eq!(
            none,
            "the server's certificate is not valid for \"h.test\" (it names no host)"
        );
        assert_eq!(
            describe(&rustls::Error::NoCertificatesPresented),
            "TLS handshake failed: peer sent no certificates"
        );
    }
}
