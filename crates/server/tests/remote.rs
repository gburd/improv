//! The native remote model service (`improv_client::remote`) against a
//! real `improv-server` binary: over HTTPS with a private CA, as two
//! principals under stored Cedar policies, one of whom sees only part of
//! the model; its change stream, filtered per subscriber; and the refusals
//! a front end must show.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use improv_client::remote::{Remote, Target};
use improv_client::wire::{
    CellEdit, Cells, EditReq, Event, ModelService, Request, Response, Structure, WindowReq,
    WireValue,
};
use improv_core_model::{
    BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure, MeasureId,
    MeasureKind, Model, Name, Value, ValueType,
};

/// Ann may not see WidgetB (item 21) or Price (measure 100), and edits
/// nothing; Ed may read everything and edit Quantity (101).
const POLICIES: &str = r#"
    permit (principal, action in Improv::Action::"Read", resource)
      when { principal in Improv::Group::"staff" };
    forbid (principal == Improv::User::"ann", action in Improv::Action::"Read",
            resource in Improv::Item::"21");
    forbid (principal == Improv::User::"ann", action in Improv::Action::"Read",
            resource in Improv::Measure::"100");
    permit (principal == Improv::User::"ed", action == Improv::Action::"EditCell",
            resource in Improv::Measure::"101");
"#;
const GROUPS: &str = r#"{"staff": {"users": ["ann", "ed"]}}"#;

fn revenue_model() -> Model {
    let mut m = Model::new();
    let (time, product) = (CategoryId(1), CategoryId(2));
    m.add_category(time, "Time");
    m.add_category(product, "Product");
    m.add_item(ItemId(10), time, "2025");
    m.add_item(ItemId(11), time, "2026");
    m.add_item(ItemId(20), product, "WidgetA");
    m.add_item(ItemId(21), product, "WidgetB");
    let input = |id, name: &str, cats| Measure {
        id: MeasureId(id),
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: cats,
        kind: MeasureKind::Input,
        description: None,
        format: None,
    };
    m.add_measure(input(100, "Price", vec![product]));
    m.add_measure(input(101, "Quantity", vec![time, product]));
    let mut revenue = input(102, "Revenue", vec![time, product]);
    revenue.kind = MeasureKind::Derived(Formula::new(Expr::BinaryOp(
        BinaryOp::Mul,
        Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
        Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
    )));
    m.add_measure(revenue);
    let at = |p: &[(u32, u32)]| {
        Coordinate::from_pairs(p.iter().map(|(c, i)| (CategoryId(*c), ItemId(*i))))
    };
    m.set_input(MeasureId(100), at(&[(2, 20)]), Value::Number(10.0));
    m.set_input(MeasureId(100), at(&[(2, 21)]), Value::Number(20.0));
    for (t, p, q) in [
        (10, 20, 100.0),
        (10, 21, 50.0),
        (11, 20, 120.0),
        (11, 21, 80.0),
    ] {
        m.set_input(MeasureId(101), at(&[(1, t), (2, p)]), Value::Number(q));
    }
    m
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("improv-remote-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A CA and a certificate for 127.0.0.1 and localhost it signed: (CA PEM
/// file, cert chain file, key file).
fn tls_files(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let mut ca = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    // Each CA its own name: one of the same name would be found as the
    // issuer, and fail on its signature instead.
    ca.distinguished_name.push(
        rcgen::DnType::CommonName,
        format!("test CA {}", dir.display()),
    );
    ca.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let ca = rcgen::CertifiedIssuer::self_signed(ca, rcgen::KeyPair::generate().unwrap()).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let leaf = rcgen::CertificateParams::new(vec!["127.0.0.1".into(), "localhost".into()])
        .unwrap()
        .signed_by(&key, &ca)
        .unwrap();
    let files = (
        dir.join("ca.pem"),
        dir.join("cert.pem"),
        dir.join("key.pem"),
    );
    std::fs::write(&files.0, ca.pem()).unwrap();
    std::fs::write(&files.1, leaf.pem() + &ca.pem()).unwrap();
    std::fs::write(&files.2, key.serialize_pem()).unwrap();
    files
}

/// A running server; killed when dropped.
struct Server {
    child: Child,
    addr: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.child.kill().ok();
        self.child.wait().ok();
    }
}

/// Start improv-server on the model at `db` on a free loopback port, with
/// `ann`, `ed` and `owner` write tokens (secret = name + "-secret"),
/// `args` added.
fn serve(dir: &Path, db: &Path, args: &[&str]) -> Server {
    let tokens = dir.join("tokens");
    std::fs::write(
        &tokens,
        "ann:write:ann-secret\ned:write:ed-secret\nro:read:ro-secret\n",
    )
    .unwrap();
    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let addr = format!("127.0.0.1:{port}");
    let mut child = Command::new(env!("CARGO_BIN_EXE_improv-server"))
        .arg(db)
        .arg(&addr)
        .args(args)
        .env("IMPROV_API_TOKEN_FILE", &tokens)
        .env_remove("IMPROV_API_TOKEN")
        .env_remove("IMPROV_API_TOKENS")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .expect("start improv-server");
    let err = child.stderr.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            let _ = tx.send(line);
        }
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(l) if l.starts_with("improv-server listening on") => break,
            Ok(_) => {}
            Err(_) => panic!("improv-server did not start"),
        }
    }
    Server { child, addr }
}

fn connect(url: &str, token: &str) -> Remote {
    let mut r = Remote::new(Target::parse(url).unwrap(), || {}).unwrap();
    r.authenticate(token.into());
    r
}

/// Send `req` and wait for its reply, keeping the change events seen
/// meanwhile in `changes`.
fn ask(r: &mut Remote, req: Request, changes: &mut Vec<Event>) -> Event {
    let t = r.send(req);
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        for ev in r.poll() {
            match ev {
                Event::Reply(x, _) if x == t => return ev,
                Event::LoggedOut(_) => return ev,
                other => changes.push(other),
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no reply")
}

fn structure(r: &mut Remote) -> Structure {
    match ask(r, Request::Structure, &mut Vec::new()) {
        Event::Reply(_, Ok(Response::Structure(s))) => s,
        e => panic!("{e:?}"),
    }
}

fn window(r: &mut Remote, m: u32) -> Cells {
    match ask(r, Request::Window(m, WindowReq::default()), &mut Vec::new()) {
        Event::Reply(_, Ok(Response::Cells(c))) => c,
        e => panic!("{e:?}"),
    }
}

fn value(c: &Cells, at: &[(u32, u32)]) -> Option<WireValue> {
    c.cells.iter().find(|x| x.at == at).map(|x| x.value.clone())
}

/// Wait for a change event that `f` accepts.
fn changed(r: &mut Remote, mut f: impl FnMut(&improv_client::wire::ChangeEvent) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        for ev in r.poll() {
            if let Event::Changed(c) = &ev {
                if f(c) {
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no such change")
}

fn wait_live(r: &Remote) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !r.live() {
        assert!(Instant::now() < deadline, "the stream never connected");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_restricted_user_over_https_sees_only_what_the_policies_allow() {
    let dir = scratch("cedar");
    let db = dir.join("m.db");
    let mut s = improv_store::Store::open(db.to_str().unwrap()).unwrap();
    s.save_model(&revenue_model()).unwrap();
    s.save_policies(Some((POLICIES, GROUPS))).unwrap();
    drop(s);
    let (ca, cert, key) = tls_files(&dir);
    let srv = serve(
        &dir,
        &db,
        &[
            "--tls-cert",
            cert.to_str().unwrap(),
            "--tls-key",
            key.to_str().unwrap(),
        ],
    );
    let url = format!("https://{}?sslrootcert={}", srv.addr, ca.display());

    // Ann: Price and WidgetB are absent, not blank; Revenue (derived)
    // comes from the server, computed over the Price she cannot see.
    let mut ann = connect(&url, "ann-secret");
    let st = structure(&mut ann);
    let ids: Vec<u32> = st.measures.iter().map(|m| m.id).collect();
    assert_eq!(ids, [101, 102]);
    let products: Vec<&str> = st.categories[1]
        .items
        .iter()
        .map(|i| i.name.as_str())
        .collect();
    assert_eq!(products, ["WidgetA"]);
    assert_eq!(
        st.measure(102).unwrap().formula,
        None,
        "it reads hidden Price"
    );
    let rev = window(&mut ann, 102);
    assert_eq!(rev.cells.len(), 2, "WidgetB's cells are absent");
    assert_eq!(
        value(&rev, &[(1, 10), (2, 20)]),
        Some(WireValue::Number(1000.0))
    );
    // Capabilities grey out what she may not do.
    match ask(&mut ann, Request::Capabilities, &mut Vec::new()) {
        Event::Reply(_, Ok(Response::Capabilities(c))) => {
            assert!(c.restricted && !c.add_measure && !c.edit_structure && !c.admin);
            assert!(c.measures.iter().all(|m| m.cells == "none" && !m.define));
            assert!(c.measures.iter().all(|m| m.id != 100));
        }
        e => panic!("{e:?}"),
    }
    // A hidden measure is refused as a missing one, word for word.
    let hidden = ask(
        &mut ann,
        Request::Window(100, WindowReq::default()),
        &mut Vec::new(),
    );
    let missing = ask(
        &mut ann,
        Request::Window(999, WindowReq::default()),
        &mut Vec::new(),
    );
    let words = |e: &Event| match e {
        Event::Reply(_, Err(f)) => f.error.replace("100", "N").replace("999", "N"),
        e => panic!("{e:?}"),
    };
    assert_eq!(words(&hidden), words(&missing));
    // Her edit is refused, naming the edit.
    let edit = |v: f64| {
        Request::Edit(EditReq {
            edits: vec![CellEdit {
                measure: 101,
                at: vec![(1, 10), (2, 20)],
                value: Some(WireValue::Number(v)),
            }],
        })
    };
    match ask(&mut ann, edit(1.0), &mut Vec::new()) {
        Event::Reply(_, Err(f)) => assert_eq!(f.index, Some(0), "{f:?}"),
        e => panic!("{e:?}"),
    }

    // Ed edits Quantity; Ann's stream gets Revenue's new value at WidgetA,
    // computed by the server, and nothing of WidgetB.
    wait_live(&ann);
    let mut ed = connect(&url, "ed-secret");
    match ask(&mut ed, edit(150.0), &mut Vec::new()) {
        Event::Reply(_, Ok(Response::Done(d))) => assert_eq!(d.message, "1 cell set"),
        e => panic!("{e:?}"),
    }
    changed(&mut ann, |c| {
        assert!(c
            .cells
            .iter()
            .all(|x| x.measure != 100 && !x.at.contains(&(2, 21))));
        c.cells.iter().any(|x| {
            x.measure == 102
                && x.at == [(1, 10), (2, 20)]
                && x.value == Some(WireValue::Number(1500.0))
        })
    });
    // A change Ann can see none of sends her nothing: Ed's edit at
    // WidgetB, then one she sees; the first event she gets is the second.
    let at_b = Request::Edit(EditReq {
        edits: vec![CellEdit {
            measure: 101,
            at: vec![(1, 11), (2, 21)],
            value: Some(WireValue::Number(5.0)),
        }],
    });
    assert!(matches!(
        ask(&mut ed, at_b, &mut Vec::new()),
        Event::Reply(_, Ok(_))
    ));
    assert!(matches!(
        ask(&mut ed, edit(160.0), &mut Vec::new()),
        Event::Reply(_, Ok(_))
    ));
    let mut first = None;
    changed(&mut ann, |c| {
        first.get_or_insert_with(|| c.clone());
        true
    });
    let first = first.unwrap();
    assert!(
        first.cells.iter().all(|x| !x.at.contains(&(2, 21))),
        "{first:?}"
    );
    assert!(first
        .cells
        .iter()
        .any(|x| x.value == Some(WireValue::Number(1600.0))));

    // A read token cannot write; a wrong token logs out.
    let mut ro = connect(&url, "ro-secret");
    match ask(&mut ro, edit(1.0), &mut Vec::new()) {
        Event::Reply(_, Err(f)) => assert!(f.error.contains("write token"), "{f:?}"),
        e => panic!("{e:?}"),
    }
    let mut bad = connect(&url, "nope");
    assert_eq!(
        ask(&mut bad, Request::Structure, &mut Vec::new()),
        Event::LoggedOut("invalid bearer token".into())
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn certificates_are_verified_and_cleartext_is_refused() {
    let dir = scratch("verify");
    let db = dir.join("m.db");
    improv_store::Store::open(db.to_str().unwrap())
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    let (_, cert, key) = tls_files(&dir);
    let srv = serve(
        &dir,
        &db,
        &[
            "--tls-cert",
            cert.to_str().unwrap(),
            "--tls-key",
            key.to_str().unwrap(),
        ],
    );
    let refused = |url: String| match ask(
        &mut connect(&url, "ann-secret"),
        Request::Structure,
        &mut Vec::new(),
    ) {
        Event::Reply(_, Err(f)) => f.error,
        e => panic!("{e:?}"),
    };
    // The OS roots do not know this CA; another CA is not this one.
    let e = refused(format!("https://{}", srv.addr));
    assert!(e.contains("is not signed by a CA Improv trusts"), "{e}");
    let other = scratch("verify-other");
    let (other_ca, _, _) = tls_files(&other);
    let e = refused(format!(
        "https://{}?sslrootcert={}",
        srv.addr,
        other_ca.display()
    ));
    assert!(e.contains("is not signed by a CA Improv trusts"), "{e}");
    assert!(!e.contains("ann-secret"), "{e}");
    // Plain HTTP to a TLS port gets no answer it could parse.
    let e = refused(format!("http://{}", srv.addr));
    assert!(
        e.starts_with(&format!("cannot reach http://{}", srv.addr)),
        "{e}"
    );
    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&other).ok();
}

/// The stream survives a server restart: it reconnects with `since`, and a
/// gap the new process's log does not cover is a reload.
#[test]
fn the_stream_reconnects_after_a_restart() {
    let dir = scratch("restart");
    let db = dir.join("m.db");
    improv_store::Store::open(db.to_str().unwrap())
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    let srv = serve(&dir, &db, &[]);
    let url = format!("http://{}", srv.addr);
    let mut r = connect(&url, "ed-secret");
    wait_live(&r);
    let addr = srv.addr.clone();
    drop(srv);
    let deadline = Instant::now() + Duration::from_secs(30);
    while r.live() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    // Another writer meanwhile.
    let mut s = improv_store::Store::open(db.to_str().unwrap()).unwrap();
    let mut m = s.load_model().unwrap();
    m.set_input(
        MeasureId(100),
        Coordinate::from_pairs([(CategoryId(2), ItemId(20))]),
        Value::Number(11.0),
    );
    s.save_model(&m).unwrap();
    drop(s);
    // Back on the same port.
    let mut again = None;
    for _ in 0..50 {
        if std::net::TcpListener::bind(&addr).is_ok() {
            again = Some(serve_at(&dir, &db, &addr));
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _srv = again.expect("the port came back");
    changed(&mut r, |c| c.reload);
    let rev = window(&mut r, 102);
    assert_eq!(
        value(&rev, &[(1, 10), (2, 20)]),
        Some(WireValue::Number(1100.0))
    );
    std::fs::remove_dir_all(&dir).ok();
}

/// [`serve`] at a given address.
fn serve_at(dir: &Path, db: &Path, addr: &str) -> Server {
    let tokens = dir.join("tokens");
    let mut child = Command::new(env!("CARGO_BIN_EXE_improv-server"))
        .arg(db)
        .arg(addr)
        .env("IMPROV_API_TOKEN_FILE", &tokens)
        .env_remove("IMPROV_API_TOKEN")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    while let Some(Ok(l)) = lines.next() {
        if l.starts_with("improv-server listening on") {
            break;
        }
    }
    std::thread::spawn(move || lines.for_each(drop));
    Server {
        child,
        addr: addr.into(),
    }
}
