//! The [`ModelService`] over `improv-server`'s `/v1` API, for a native front
//! end (the desktop GUI and the TUI): requests over HTTP/1.1, one at a
//! time and in order, on a thread of their own; the change stream
//! (`GET /v1/changes`, Server-Sent Events) on another, resumed with
//! `?since=` after a dropped connection. The browser's twin is
//! `crates/gui_web/src/net.rs`.
//!
//! **Transport.** `https://` always, verified: against the operating
//! system's roots (`rustls-platform-verifier`), or only the CA in
//! `?sslrootcert=FILE` when the URL names one. `http://` only to a
//! loopback address (`localhost`, `127.0.0.0/8`, `::1`), where nothing
//! crosses a network; anywhere else it is refused, so a bearer token never
//! travels in cleartext. Redirects are not followed (the token stays with
//! the host it was given for), a URL with a user name or password is
//! refused, and an `http://` URL ignores the `*_PROXY` variables.
//!
//! **The token** is a bearer token held in memory ([`ModelService::authenticate`]);
//! never in a URL, a file or the error text. A server without tokens takes
//! an empty one.

use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crate::wire::{take_events, Event, Failure, ModelService, Request};

/// Whether `target` names an improv-server (`http://` or `https://`)
/// rather than a store file or a `postgres://` URL.
pub fn is_server(target: &str) -> bool {
    let t = target.to_ascii_lowercase();
    t.starts_with("https://") || t.starts_with("http://")
}

/// Where a server is, parsed from its URL: `https://host[:port][/prefix]`,
/// with `?sslrootcert=FILE` to trust that CA only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// `scheme://host[:port][/prefix]`, without a trailing `/`: the API is
    /// at `{base}/v1/…`.
    base: String,
    host: String,
    tls: bool,
    ca: Option<std::path::PathBuf>,
}

impl Target {
    /// Parse a server URL; the error says what is wrong with it (and never
    /// repeats a password it held).
    pub fn parse(url: &str) -> Result<Target, String> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or("a server URL starts with https:// (or http:// on this machine)")?;
        let tls = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            _ => return Err(format!("{scheme}:// is not a server URL (use https://)")),
        };
        if rest.contains('#') {
            return Err("a server URL has no #fragment".into());
        }
        let (rest, query) = rest.split_once('?').unwrap_or((rest, ""));
        let (authority, path) = rest.split_at(rest.find('/').unwrap_or(rest.len()));
        if authority.contains('@') {
            return Err(
                "a server URL carries no user name or password: give the token in \
                 IMPROV_API_TOKEN, or when asked"
                    .into(),
            );
        }
        let host = host_of(authority).ok_or_else(|| format!("no host in {scheme}://{rest}"))?;
        if !tls && !is_loopback(&host) {
            return Err(format!(
                "refusing http://{authority}: a token would cross the network in \
                 cleartext. Use https:// (http:// is only for this machine: localhost, \
                 127.0.0.1, ::1)"
            ));
        }
        let mut ca = None;
        for p in query.split('&').filter(|p| !p.is_empty()) {
            match p.split_once('=') {
                Some(("sslrootcert", v)) if !v.is_empty() => {
                    ca = Some(std::path::PathBuf::from(decode(v)?));
                }
                _ => {
                    let key = p.split_once('=').map_or(p, |(k, _)| k);
                    return Err(format!(
                        "unknown server URL parameter {key:?} (the one it takes is \
                         sslrootcert=FILE)"
                    ));
                }
            }
        }
        if ca.is_some() && !tls {
            return Err("sslrootcert= needs an https:// URL".into());
        }
        Ok(Target {
            base: format!(
                "{}://{authority}{}",
                scheme.to_ascii_lowercase(),
                path.trim_end_matches('/')
            ),
            host,
            tls,
            ca,
        })
    }

    /// The URL without its parameters, for a title bar.
    pub fn base(&self) -> &str {
        &self.base
    }

    /// The host, as the certificate must name it.
    pub fn host(&self) -> &str {
        &self.host
    }

    fn agent(&self, stream: bool) -> Result<ureq::Agent, String> {
        use ureq::tls::{Certificate, RootCerts, TlsConfig};
        let roots = match &self.ca {
            None => RootCerts::PlatformVerifier,
            Some(f) => {
                let pem =
                    std::fs::read(f).map_err(|e| format!("sslrootcert {}: {e}", f.display()))?;
                let certs: Vec<Certificate<'static>> = ureq::tls::parse_pem(&pem)
                    .filter_map(|i| match i {
                        Ok(ureq::tls::PemItem::Certificate(c)) => Some(c),
                        _ => None,
                    })
                    .collect();
                if certs.is_empty() {
                    return Err(format!(
                        "sslrootcert {}: no certificate in it (it wants PEM)",
                        f.display()
                    ));
                }
                RootCerts::new_with_certs(&certs)
            }
        };
        let tls = TlsConfig::builder()
            .root_certs(roots)
            .unversioned_rustls_crypto_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .build();
        let b = ureq::Agent::config_builder()
            .tls_config(tls)
            .https_only(self.tls)
            .http_status_as_error(false)
            .max_redirects(0)
            .user_agent(concat!("improv/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(CONNECT_TIMEOUT));
        // A proxy from the environment would see an http:// request, token
        // and all; over https:// it sees only a tunnel.
        let b = if self.tls { b } else { b.proxy(None) };
        // The stream waits for events as long as the server stays up; a
        // read that hears nothing, not even its keep-alive, gives up.
        let b = match stream {
            true => b.timeout_per_read(Some(STREAM_SILENCE)),
            false => b.timeout_global(Some(REQUEST_TIMEOUT)),
        };
        Ok(b.build().new_agent())
    }
}

/// The host of `host[:port]` or `[v6]:port`, without brackets.
fn host_of(authority: &str) -> Option<String> {
    let host = match authority.strip_prefix('[') {
        Some(v6) => v6.split_once(']')?.0,
        None => authority.rsplit_once(':').map_or(authority, |(h, _)| h),
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// Percent-decode a URL parameter's value.
fn decode(s: &str) -> Result<String, String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hex = s
                    .get(i + 1..i + 3)
                    .and_then(|h| u8::from_str_radix(h, 16).ok());
                out.push(hex.ok_or_else(|| format!("bad %-escape in {s:?}"))?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| format!("{s:?} is not UTF-8"))
}

/// Percent-encode a query value (a version is opaque: anything may be in it).
fn encode(s: &str) -> String {
    s.bytes()
        .map(|c| match c {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(c).to_string()
            }
            _ => format!("%{c:02X}"),
        })
        .collect()
}

/// How long to wait to connect, and for a whole request.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// The server sends a keep-alive every 15 s: three missed is a dead link.
const STREAM_SILENCE: Duration = Duration::from_secs(45);
/// The most a reply may be: a window of 200,000 cells is about 10 MB.
const MAX_REPLY: u64 = 128 << 20;
/// The first and the longest wait before reconnecting the stream.
const BACKOFF: (Duration, Duration) = (Duration::from_millis(250), Duration::from_secs(10));

/// What the threads share with the front end.
struct Shared {
    target: Target,
    token: Mutex<Option<String>>,
    session: String,
    out: Mutex<Sender<Event>>,
    live: AtomicBool,
    /// Bumped by each login and by drop: an older stream stops.
    epoch: AtomicU64,
    wake: Box<dyn Fn() + Send + Sync>,
}

impl Shared {
    fn push(&self, ev: Event) {
        let _ = self
            .out
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .send(ev);
        (self.wake)();
    }

    fn token(&self) -> Option<String> {
        self.token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// The remote model service. Replies and changes arrive on background
/// threads; `wake` (a GUI's repaint) is called after each.
pub struct Remote {
    shared: Arc<Shared>,
    events: Receiver<Event>,
    jobs: Sender<(u64, Request)>,
    next: u64,
}

impl Remote {
    /// A service over the server at `target`. Nothing is sent until the
    /// first request or [`ModelService::authenticate`] (which starts the
    /// change stream).
    pub fn new(target: Target, wake: impl Fn() + Send + Sync + 'static) -> Result<Remote, String> {
        let agent = target.agent(false)?;
        let (out, events) = mpsc::channel();
        let shared = Arc::new(Shared {
            target,
            token: Mutex::new(None),
            session: session_id(),
            out: Mutex::new(out),
            live: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
            wake: Box::new(wake),
        });
        let (jobs, todo) = mpsc::channel::<(u64, Request)>();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("improv-remote".into())
            .spawn(move || {
                for (ticket, req) in todo {
                    let ev = match call(&agent, &s, &req) {
                        Ok(text) => Event::Reply(ticket, req.parse_reply(&text).map_err(plain)),
                        Err((401, f)) => Event::LoggedOut(f.error),
                        Err((_, f)) => Event::Reply(ticket, Err(f)),
                    };
                    s.push(ev);
                }
            })
            .map_err(|e| format!("cannot start the request thread: {e}"))?;
        Ok(Remote {
            shared,
            events,
            jobs,
            next: 0,
        })
    }

    /// Where it connects.
    pub fn target(&self) -> &Target {
        &self.shared.target
    }

    /// Start (or restart) the change stream.
    fn stream(&self) {
        let mine = self.shared.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let s = self.shared.clone();
        let agent = match s.target.agent(true) {
            Ok(a) => a,
            Err(e) => return s.push(Event::LoggedOut(e)),
        };
        let started = std::thread::Builder::new()
            .name("improv-changes".into())
            .spawn(move || follow(&agent, &s, mine));
        if let Err(e) = started {
            self.shared
                .push(Event::LoggedOut(format!("cannot follow changes: {e}")));
        }
    }
}

impl Drop for Remote {
    fn drop(&mut self) {
        // Stops the stream at its next read (a keep-alive at the latest);
        // the request thread ends when `jobs` goes.
        self.shared.epoch.fetch_add(1, Ordering::SeqCst);
    }
}

impl ModelService for Remote {
    fn send(&mut self, req: Request) -> u64 {
        self.next += 1;
        if self.jobs.send((self.next, req)).is_err() {
            let f = plain("the request thread stopped: reopen the model".into());
            self.shared.push(Event::Reply(self.next, Err(f)));
        }
        self.next
    }

    fn poll(&mut self) -> Vec<Event> {
        self.events.try_iter().collect()
    }

    fn authenticate(&mut self, secret: String) {
        *self
            .shared
            .token
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = (!secret.is_empty()).then_some(secret);
        self.stream();
    }

    fn live(&self) -> bool {
        self.shared.live.load(Ordering::SeqCst)
    }
}

fn plain(error: String) -> Failure {
    Failure {
        error,
        ..Default::default()
    }
}

/// A random id for this service's undo session.
fn session_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos()),
    );
    h.write_u32(std::process::id());
    format!("{:016x}", h.finish())
}

/// A request to `path` with the token and the session.
fn request<B>(s: &Shared, b: ureq::RequestBuilder<B>) -> ureq::RequestBuilder<B> {
    let b = b.header("x-improv-session", &s.session);
    match s.token() {
        Some(t) => b.header("authorization", format!("Bearer {t}")),
        None => b,
    }
}

/// One request: the body of a 2xx reply, or the status and the refusal.
fn call(agent: &ureq::Agent, s: &Shared, req: &Request) -> Result<String, (u16, Failure)> {
    let (method, path, body) = req.http();
    let url = format!("{}{path}", s.target.base);
    let r = match (method, body) {
        ("GET", _) => request(s, agent.get(&url)).call(),
        ("DELETE", _) => request(s, agent.delete(&url)).call(),
        (m, b) => {
            let b = b.map_or_else(String::new, |b| b.to_string());
            let builder = match m {
                "PUT" => agent.put(&url),
                _ => agent.post(&url),
            };
            request(s, builder)
                .header("content-type", "application/json")
                .send(b)
        }
    };
    let mut resp = r.map_err(|e| (0, plain(unreachable(&s.target, &e))))?;
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .with_config()
        .limit(MAX_REPLY)
        .read_to_string()
        .map_err(|e| (status, plain(unreachable(&s.target, &e))))?;
    if (200..300).contains(&status) {
        return Ok(text);
    }
    let f = serde_json::from_str::<Failure>(&text).unwrap_or_else(|_| {
        plain(match (status, text.trim()) {
            (300..=399, _) => format!(
                "HTTP {status}: the server redirects, and Improv does not follow it (give \
                 the URL it names)"
            ),
            (_, "") => format!("HTTP {status}"),
            (_, t) => format!("HTTP {status}: {}", t.chars().take(200).collect::<String>()),
        })
    });
    Err((status, f))
}

/// What went wrong reaching the server, in words: a certificate problem
/// named, else ureq's own.
fn unreachable(t: &Target, e: &ureq::Error) -> String {
    let tls = match e {
        ureq::Error::Rustls(r) => Some(r),
        ureq::Error::Io(io) => io.get_ref().and_then(|x| x.downcast_ref::<rustls::Error>()),
        _ => None,
    };
    match tls {
        Some(r) => format!("{}: {}", t.base, describe(r)),
        None => format!("cannot reach {}: {e}", t.base),
    }
}

/// A TLS failure in words (as `improv_storage_pg::tls` says it).
fn describe(e: &rustls::Error) -> String {
    use rustls::CertificateError as C;
    let rustls::Error::InvalidCertificate(c) = e else {
        return format!("TLS handshake failed: {e}");
    };
    let what = match c {
        C::UnknownIssuer => "is not signed by a CA Improv trusts (sslrootcert=FILE in the \
                             URL names the CA; without it, the operating system's roots)"
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

/// Why a stream ended.
#[derive(Debug, PartialEq)]
enum Gone {
    /// The token was refused: ask for it again.
    Auth(String),
    /// Anything else: reconnect.
    Net(String),
}

/// The change stream's thread: read it, and reconnect (resuming at the
/// last version seen) until a newer login or drop takes over.
fn follow(agent: &ureq::Agent, s: &Shared, mine: u64) {
    let (mut seen, mut wait) = (None, BACKOFF.0);
    while s.epoch.load(Ordering::SeqCst) == mine {
        let r = read_stream(agent, s, mine, &mut seen);
        s.live.store(false, Ordering::SeqCst);
        (s.wake)();
        match r {
            Err(Gone::Auth(e)) => {
                if s.epoch.load(Ordering::SeqCst) == mine {
                    s.push(Event::LoggedOut(e));
                }
                return;
            }
            // A clean read resets the wait; a failure doubles it.
            Ok(()) => wait = BACKOFF.0,
            Err(Gone::Net(_)) => wait = (wait * 2).min(BACKOFF.1),
        }
        // In slices, so a newer login is not kept waiting.
        let mut left = wait;
        while !left.is_zero() && s.epoch.load(Ordering::SeqCst) == mine {
            let step = left.min(Duration::from_millis(50));
            std::thread::sleep(step);
            left -= step;
        }
    }
}

/// Read `GET /v1/changes` until it ends, pushing each event.
fn read_stream(
    agent: &ureq::Agent,
    s: &Shared,
    mine: u64,
    seen: &mut Option<String>,
) -> Result<(), Gone> {
    let url = match seen {
        Some(v) => format!("{}/v1/changes?since={}", s.target.base, encode(v)),
        None => format!("{}/v1/changes", s.target.base),
    };
    let net = |e: ureq::Error| Gone::Net(unreachable(&s.target, &e));
    let mut resp = request(s, agent.get(&url))
        .header("accept", "text/event-stream")
        .call()
        .map_err(net)?;
    match resp.status().as_u16() {
        401 => {
            // The server's words, as a refused request gives them.
            let text = resp
                .body_mut()
                .with_config()
                .limit(1 << 16)
                .read_to_string();
            let f = text
                .ok()
                .and_then(|t| serde_json::from_str::<Failure>(&t).ok());
            return Err(Gone::Auth(
                f.map_or_else(|| "the token was refused".into(), |f| f.error),
            ));
        }
        200..=299 => {}
        n => return Err(Gone::Net(format!("HTTP {n}"))),
    }
    s.live.store(true, Ordering::SeqCst);
    (s.wake)();
    let mut body = resp.into_body().into_reader();
    let (mut buf, mut chunk) = (String::new(), vec![0u8; 16 << 10]);
    let mut pending: Vec<u8> = Vec::new();
    while s.epoch.load(Ordering::SeqCst) == mine {
        let n = body
            .read(&mut chunk)
            .map_err(|e| Gone::Net(e.to_string()))?;
        if n == 0 {
            return Ok(());
        }
        // A character split across reads waits for its other half.
        pending.extend_from_slice(&chunk[..n]);
        let ok = match std::str::from_utf8(&pending) {
            Ok(t) => t.len(),
            Err(e) => e.valid_up_to(),
        };
        buf.push_str(std::str::from_utf8(&pending[..ok]).unwrap_or_default());
        pending.drain(..ok);
        if buf.len() > MAX_REPLY as usize {
            return Err(Gone::Net("a change event too large".into()));
        }
        for ev in take_events(&mut buf) {
            *seen = Some(ev.version.clone());
            s.push(Event::Changed(ev));
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;

    #[test]
    fn server_urls_are_parsed_and_cleartext_stays_on_this_machine() {
        assert!(is_server("https://h") && is_server("HTTP://localhost:1"));
        assert!(!is_server("postgres://h/db?model=m") && !is_server("m.db"));
        let t =
            Target::parse("https://Example.com:8443/improv/?sslrootcert=%2Ftmp%2Fca.pem").unwrap();
        assert_eq!(t.base(), "https://Example.com:8443/improv");
        assert_eq!(t.host(), "example.com");
        assert_eq!(t.ca.as_deref(), Some(std::path::Path::new("/tmp/ca.pem")));
        for ok in [
            "http://localhost:3000",
            "http://127.0.0.1",
            "http://[::1]:9/",
            "http://127.9.9.9:1",
        ] {
            let t = Target::parse(ok).unwrap();
            assert!(!t.tls && !t.base().ends_with('/'), "{ok}");
        }
        assert_eq!(
            Target::parse("https://[2001:db8::1]:3000").unwrap().host(),
            "2001:db8::1"
        );
        for (bad, words) in [
            ("http://example.com", "cleartext"),
            ("http://10.0.0.1:3000", "cleartext"),
            ("ftp://h", "not a server URL"),
            ("h:3000", "starts with https://"),
            ("https://ann:pw@h", "no user name or password"),
            ("https://h/#x", "fragment"),
            (
                "https://h?model=m",
                "unknown server URL parameter \"model\"",
            ),
            ("https://h?sslrootcert=", "unknown server URL parameter"),
            ("https://h?sslrootcert=%zz", "bad %-escape"),
            ("https://h?sslrootcert=%ff", "not UTF-8"),
            ("http://localhost?sslrootcert=ca.pem", "needs an https://"),
            ("https://:3000", "no host"),
            ("https://[::1", "no host"),
        ] {
            let e = Target::parse(bad).unwrap_err();
            assert!(e.contains(words), "{bad}: {e}");
            assert!(!e.contains("pw"), "{e}");
        }
        assert_eq!(decode("a+b%20c").unwrap(), "a b c");
        assert_eq!(encode("1 2/é"), "1%202%2F%C3%A9");
        assert_ne!(session_id(), session_id());
    }

    #[test]
    fn a_missing_or_empty_ca_file_is_refused_by_name() {
        let t = Target::parse("https://h?sslrootcert=/nonexistent/ca.pem").unwrap();
        let e = Remote::new(t, || {}).err().unwrap();
        assert!(e.starts_with("sslrootcert /nonexistent/ca.pem: "), "{e}");
        let f = std::env::temp_dir().join(format!("improv-empty-ca-{}.pem", std::process::id()));
        std::fs::write(&f, "not a certificate").unwrap();
        let url = format!("https://h?sslrootcert={}", f.display());
        let e = Remote::new(Target::parse(&url).unwrap(), || {})
            .err()
            .unwrap();
        assert!(e.contains("no certificate in it"), "{e}");
        std::fs::remove_file(f).ok();
    }

    /// A one-request HTTP server on loopback: answers each connection with
    /// `reply`, and hands back what each request said.
    fn fake(replies: Vec<String>) -> (String, Receiver<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for (reply, conn) in replies.into_iter().zip(l.incoming()) {
                let mut c = conn.unwrap();
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut head = String::new();
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    if r.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0; len];
                r.read_exact(&mut body).unwrap();
                head.push_str(&String::from_utf8_lossy(&body));
                let _ = tx.send(head);
                let _ = c.write_all(reply.as_bytes());
            }
        });
        (url, rx)
    }

    fn http(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn wait(r: &mut Remote, n: usize) -> Vec<Event> {
        let mut out = Vec::new();
        for _ in 0..400 {
            out.extend(r.poll());
            if out.len() >= n {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        out
    }

    /// Each request carries the token and the session; a 2xx reply parses,
    /// a refusal is the server's own words, a 401 logs out, a reply that
    /// is not JSON says its status.
    #[test]
    fn replies_refusals_and_logouts_come_back_in_order() {
        let (url, said) = fake(vec![
            http("200 OK", r#"{"version":"7","message":"1 cell set"}"#),
            http(
                "403 Forbidden",
                r#"{"error":"you may not edit that cell","index":0}"#,
            ),
            http("401 Unauthorized", r#"{"error":"invalid bearer token"}"#),
            http("502 Bad Gateway", "upstream down"),
            http("302 Found", ""),
            http("500 Internal Server Error", ""),
            http("200 OK", "not json"),
        ]);
        let woken = Arc::new(AtomicU64::new(0));
        let w = woken.clone();
        let mut r = Remote::new(Target::parse(&url).unwrap(), move || {
            w.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
        *r.shared.token.lock().unwrap() = Some("s3cret".into());
        assert!(!r.live());
        assert_eq!(r.target().base(), url);
        let edit = Request::Edit(crate::wire::EditReq::default());
        let tickets: Vec<u64> = (0..7)
            .map(|i| match i {
                3 => r.send(Request::Structure),
                4 => r.send(Request::DeleteMeasure(1)),
                5 => r.send(Request::UpdateMeasure(1, Default::default())),
                _ => r.send(edit.clone()),
            })
            .collect();
        let evs = wait(&mut r, 7);
        let head = said.recv().unwrap();
        assert!(head.starts_with("POST /v1/cells HTTP/1.1"), "{head}");
        let lower = head.to_ascii_lowercase();
        assert!(lower.contains("authorization: bearer s3cret"), "{head}");
        assert!(lower.contains(&format!("x-improv-session: {}", r.shared.session)));
        assert!(
            lower.contains("content-type: application/json") && head.ends_with("{\"edits\":[]}")
        );
        match &evs[0] {
            Event::Reply(t, Ok(crate::wire::Response::Done(d))) => {
                assert_eq!((*t, d.message.as_str()), (tickets[0], "1 cell set"))
            }
            e => panic!("{e:?}"),
        }
        assert!(matches!(&evs[1], Event::Reply(_, Err(f))
            if f.error == "you may not edit that cell" && f.index == Some(0)));
        assert_eq!(evs[2], Event::LoggedOut("invalid bearer token".into()));
        assert!(matches!(&evs[3], Event::Reply(_, Err(f)) if f.error == "HTTP 502: upstream down"));
        assert!(matches!(&evs[4], Event::Reply(_, Err(f)) if f.error.contains("does not follow")));
        assert!(matches!(&evs[5], Event::Reply(_, Err(f)) if f.error == "HTTP 500"));
        assert!(matches!(&evs[6], Event::Reply(_, Err(f)) if f.error.starts_with("bad reply: ")));
        for want in [
            "GET /v1/structure",
            "DELETE /v1/measures/1",
            "PUT /v1/measures/1",
        ] {
            let h = (0..4)
                .map(|_| said.recv().unwrap())
                .find(|h| h.starts_with(want));
            assert!(h.is_some(), "{want}");
        }
        assert!(woken.load(Ordering::SeqCst) >= 7);
    }

    #[test]
    fn an_unreachable_server_is_named() {
        // A port nobody listens on.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", l.local_addr().unwrap());
        drop(l);
        let mut r = Remote::new(Target::parse(&url).unwrap(), || {}).unwrap();
        r.send(Request::Structure);
        let evs = wait(&mut r, 1);
        assert!(
            matches!(&evs[0], Event::Reply(1, Err(f)) if f.error.starts_with(&format!("cannot reach {url}: "))),
            "{evs:?}"
        );
    }

    /// The stream: events pushed as they come (split across reads, a
    /// character split too), a reconnect resumes at the last version, and
    /// a 401 logs out and stops.
    #[test]
    fn the_change_stream_follows_resumes_and_logs_out() {
        let sse = |body: &str| {
            format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n{body}"
            )
        };
        let (url, said) = fake(vec![
            sse(": hi\n\nid: 5\ndata: {\"version\":\"5\"}\n\nid: 6\ndata: {\"version\":\"6\",\"cells\":[{\"measure\":1,\"at\":[[1,2]],\"value\":\"é\"}]}\n\n"),
            http("503 Service Unavailable", ""),
            sse("data: {\"version\":\"7\",\"reload\":true}\n\n"),
            http("401 Unauthorized", r#"{"error":"invalid bearer token"}"#),
        ]);
        let mut r = Remote::new(Target::parse(&url).unwrap(), || {}).unwrap();
        r.authenticate(String::new());
        let evs = wait(&mut r, 4);
        assert!(
            matches!(&evs[0], Event::Changed(c) if c.version == "5"),
            "{evs:?}"
        );
        assert!(
            matches!(&evs[1], Event::Changed(c) if c.cells[0].value == Some(crate::wire::WireValue::Text("é".into())))
        );
        assert!(matches!(&evs[2], Event::Changed(c) if c.reload && c.version == "7"));
        assert_eq!(evs[3], Event::LoggedOut("invalid bearer token".into()));
        let heads: Vec<String> = (0..4).map(|_| said.recv().unwrap()).collect();
        assert!(
            heads[0].starts_with("GET /v1/changes HTTP/1.1"),
            "{}",
            heads[0]
        );
        assert!(
            !heads[0].to_ascii_lowercase().contains("authorization"),
            "no token: none sent"
        );
        assert!(
            heads[1].starts_with("GET /v1/changes?since=6 HTTP/1.1"),
            "{}",
            heads[1]
        );
        assert!(
            heads[3].starts_with("GET /v1/changes?since=7 HTTP/1.1"),
            "{}",
            heads[3]
        );
        assert!(!r.live());
    }

    #[test]
    fn a_stopped_request_thread_is_a_refusal() {
        let mut r = Remote::new(Target::parse("http://127.0.0.1:9").unwrap(), || {}).unwrap();
        let (dead, _) = mpsc::channel();
        r.jobs = dead;
        drop(std::mem::replace(&mut r.jobs, mpsc::channel().0));
        let t = r.send(Request::Structure);
        let evs = wait(&mut r, 1);
        assert!(
            matches!(&evs[0], Event::Reply(x, Err(f)) if *x == t && f.error.contains("stopped"))
        );
    }

    #[test]
    fn certificate_errors_are_described() {
        use rustls::CertificateError as C;
        let d = |c| describe(&rustls::Error::InvalidCertificate(c));
        assert!(d(C::UnknownIssuer).contains("sslrootcert=FILE"));
        assert!(d(C::NotValidForName).contains("not valid for this host"));
        assert!(d(C::Expired).ends_with("has expired"));
        assert!(d(C::NotValidYet).ends_with("not valid yet"));
        assert!(d(C::Revoked).ends_with("revoked"));
        assert!(d(C::BadSignature).contains("was refused"));
        assert!(describe(&rustls::Error::DecryptError).starts_with("TLS handshake failed"));
        let t = Target::parse("https://h").unwrap();
        let io = std::io::Error::other(rustls::Error::InvalidCertificate(C::Expired));
        assert_eq!(
            unreachable(&t, &ureq::Error::Io(io)),
            "https://h: the server's certificate has expired"
        );
        let e = ureq::Error::Rustls(rustls::Error::InvalidCertificate(C::Revoked));
        assert!(unreachable(&t, &e).ends_with("has been revoked"));
    }
}
