//! improv-server over a model in a DuckDB file (`duckdb:FILE?model=NAME`):
//! the server holds the file, so another writer (this test's process, or a
//! second server) is refused by name; two callers under the model's Cedar
//! policies edit it live, and one's edit reaches the other's change stream
//! filtered for them; after the server is killed the file opens with every
//! edit in it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use improv_core_model::{
    parse_expr, CategoryId, Coordinate, ItemId, Measure, MeasureId, MeasureKind, Model, Name,
    Value, ValueType,
};
use serde_json::{json, Value as Json};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("improv-srv-duck-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Time {2025, 2026} x Product {WidgetA, WidgetB}: Price, Quantity and
/// Revenue = Price * Quantity.
fn model() -> Model {
    let mut m = Model::new();
    let (t, p) = (CategoryId(1), CategoryId(2));
    m.add_category(t, "Time");
    m.add_category(p, "Product");
    m.add_item(ItemId(10), t, "2025");
    m.add_item(ItemId(11), t, "2026");
    m.add_item(ItemId(20), p, "WidgetA");
    m.add_item(ItemId(21), p, "WidgetB");
    let measure = |id, name: &str, cats: Vec<CategoryId>, kind| Measure {
        id: MeasureId(id),
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: cats,
        kind,
        description: None,
        format: None,
    };
    m.add_measure(measure(100, "Price", vec![p], MeasureKind::Input));
    m.add_measure(measure(101, "Quantity", vec![t, p], MeasureKind::Input));
    let rev = parse_expr(&m, "Price * Quantity").unwrap();
    m.add_measure(measure(
        102,
        "Revenue",
        vec![t, p],
        MeasureKind::Derived(rev),
    ));
    for (item, price) in [(20, 10.0), (21, 20.0)] {
        let at = Coordinate::from_pairs([(p, ItemId(item))]);
        m.set_input(MeasureId(100), at, Value::Number(price));
    }
    for (year, item, q) in [
        (10, 20, 100.0),
        (10, 21, 50.0),
        (11, 20, 120.0),
        (11, 21, 80.0),
    ] {
        let at = Coordinate::from_pairs([(t, ItemId(year)), (p, ItemId(item))]);
        m.set_input(MeasureId(101), at, Value::Number(q));
    }
    m
}

/// A running improv-server; killed (not stopped) when dropped.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Start the server on `target` with tokens `ann` and `bob`: `Ok` once it
/// listens, `Err((exit code, stderr))` when it exits first.
fn start(dir: &Path, target: &str) -> Result<Server, (i32, String)> {
    let tokens = dir.join("tokens");
    std::fs::write(&tokens, "ann:write:ann-secret-1\nbob:write:bob-secret-2\n").unwrap();
    // A free port: bind one, let it go, hand it over.
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_improv-server"))
        .arg(target)
        .arg(format!("127.0.0.1:{port}"))
        .env("IMPROV_API_TOKEN_FILE", &tokens)
        .env_remove("IMPROV_API_TOKEN")
        .env_remove("IMPROV_API_TOKENS")
        .env_remove("IMPROV_API_TOKENS_RO")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start improv-server");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = String::new();
    for line in lines.by_ref().map_while(Result::ok) {
        if line.starts_with("improv-server listening on") {
            // Keep the pipe drained, or a chatty server would block.
            std::thread::spawn(move || lines.for_each(drop));
            return Ok(Server { child, port });
        }
        seen.push_str(&line);
        seen.push('\n');
    }
    let status = child.wait().unwrap();
    Err((status.code().unwrap_or(-1), seen))
}

/// One HTTP/1.1 request; the status and the body.
fn http(port: u16, method: &str, path: &str, token: &str, body: Option<Json>) -> (u16, String) {
    let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let body = body.map(|b| b.to_string()).unwrap_or_default();
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\
         X-Improv-Session: s1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out[9..12].parse().unwrap();
    let body = out.split_once("\r\n\r\n").map_or("", |(_, b)| b).to_owned();
    (status, body)
}

/// A `GET /v1/changes` stream, read an event at a time.
struct Changes {
    s: TcpStream,
    buf: String,
}

impl Changes {
    fn open(port: u16, token: &str) -> Self {
        let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(
            s,
            "GET /v1/changes HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n"
        )
        .unwrap();
        Changes {
            s,
            buf: String::new(),
        }
    }

    /// The next event's data (the chunked framing's size lines are not
    /// `data:` lines, so they are skipped).
    fn next(&mut self) -> Json {
        loop {
            if let Some(i) = self.buf.find("data:") {
                if let Some(end) = self.buf[i..].find('\n') {
                    let line: String = self.buf.drain(..i + end + 1).collect();
                    let data = line[i + 5..].trim();
                    return serde_json::from_str(data).unwrap();
                }
            }
            let mut chunk = [0u8; 4096];
            let n = self.s.read(&mut chunk).expect("an event within 10 s");
            assert!(n > 0, "the stream closed: {}", self.buf);
            self.buf.push_str(std::str::from_utf8(&chunk[..n]).unwrap());
        }
    }
}

const ANN: &str = "ann-secret-1";
const BOB: &str = "bob-secret-2";

#[test]
fn the_server_holds_a_duckdb_model_and_serves_it_live() {
    let dir = scratch("live");
    let file = dir.join("plans.duckdb");
    let target = format!("duckdb:{}?model=plan", file.display());
    // Ann may not see WidgetB (item 21); bob administers the model.
    {
        let mut s = improv_store::Store::open(&target).unwrap();
        s.save_model(&model()).unwrap();
        let who = |n: &str| improv_policy::templates::Who::User(n.into());
        let r = improv_policy::templates::render;
        let text = [
            r("analyst", &who("ann"), &[], &[ItemId(21)]).unwrap(),
            r("admin", &who("bob"), &[], &[]).unwrap(),
        ]
        .join("\n");
        improv_policy::Policies::parse(&text, "").unwrap();
        s.save_policies(Some((&text, ""))).unwrap();
    }
    let server = start(&dir, &target).unwrap_or_else(|e| panic!("{e:?}"));

    // Another writer is refused, naming the holder, and writes nothing.
    let e = improv_store::Store::open(&target)
        .err()
        .unwrap()
        .to_string();
    assert!(
        e.contains(&format!("{} is open in another program", file.display()))
            && e.contains("change it through the server"),
        "{e}"
    );
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        assert!(e.contains("improv-server"), "{e}");
    }
    // So is a second server, before it listens.
    let (code, err) = start(&dir, &target).err().expect("a second server");
    assert!(
        code == 2 && err.contains("is open in another program"),
        "{err}"
    );

    // Two sessions, each on its change stream.
    let (port, mut ann, mut bob) = (
        server.port,
        Changes::open(server.port, ANN),
        Changes::open(server.port, BOB),
    );
    let v0 = ann.next()["version"].as_str().unwrap().to_owned();
    assert_eq!(bob.next()["version"], v0.as_str());
    let (st, body) = http(port, "GET", "/v1/structure", ANN, None);
    assert_eq!(st, 200, "{body}");
    assert!(
        body.contains("WidgetA") && !body.contains("WidgetB"),
        "{body}"
    );

    // Bob edits a cell each may see and one only he may.
    let edit = json!({"edits": [
        {"measure": 101, "at": [[1, 10], [2, 20]], "value": 7},
        {"measure": 101, "at": [[1, 10], [2, 21]], "value": 8}]});
    let (st, body) = http(port, "POST", "/v1/cells", BOB, Some(edit));
    assert_eq!(st, 200, "{body}");
    let (a, b) = (ann.next().to_string(), bob.next().to_string());
    assert!(a.contains("[1,10],[2,20]") && !a.contains("21"), "ann: {a}");
    assert!(
        b.contains("[1,10],[2,20]") && b.contains("[1,10],[2,21]"),
        "bob: {b}"
    );
    assert_ne!(
        json!(v0),
        serde_json::from_str::<Json>(&b).unwrap()["version"]
    );

    // Ann may not edit; WidgetB is missing to her, not forbidden.
    let mine = json!({"edits": [{"measure": 101, "at": [[1, 11], [2, 20]], "value": 1}]});
    assert_eq!(http(port, "POST", "/v1/cells", ANN, Some(mine)).0, 404);
    let hidden = json!({"edits": [{"measure": 101, "at": [[1, 11], [2, 21]], "value": 1}]});
    assert_eq!(http(port, "POST", "/v1/cells", ANN, Some(hidden)).0, 404);

    // Killed, not stopped: the file opens with bob's edit and nothing of ann's.
    drop(server);
    let saved = improv_store::Store::open(&target)
        .unwrap()
        .load_model()
        .unwrap();
    let q = |y, i| {
        let at = Coordinate::from_pairs([(CategoryId(1), ItemId(y)), (CategoryId(2), ItemId(i))]);
        saved.input(MeasureId(101), &at).cloned()
    };
    assert_eq!(q(10, 20), Some(Value::Number(7.0)));
    assert_eq!(q(10, 21), Some(Value::Number(8.0)));
    assert_eq!(q(11, 20), Some(Value::Number(120.0)));
    assert_eq!(q(11, 21), Some(Value::Number(80.0)));
    std::fs::remove_dir_all(&dir).ok();
}
