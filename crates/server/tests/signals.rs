//! The server binary ends on SIGTERM (`docker stop`, systemd) instead of
//! waiting to be killed.

#[cfg(unix)]
#[test]
fn sigterm_stops_the_server() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let dir = std::env::temp_dir().join(format!("improv-sig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("m.db");
    let mut child = Command::new(env!("CARGO_BIN_EXE_improv-server"))
        .arg(&db)
        .arg("127.0.0.1:0")
        .env_remove("IMPROV_API_TOKEN")
        .env_remove("IMPROV_API_TOKENS")
        .env_remove("IMPROV_API_TOKEN_FILE")
        .stderr(Stdio::piped())
        .spawn()
        .expect("start improv-server");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    // Up and serving before the signal, so the handler is installed.
    assert!(
        lines
            .by_ref()
            .map_while(Result::ok)
            .any(|l| l.starts_with("improv-server listening on")),
        "the server did not start"
    );
    let sent = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let t = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        assert!(
            t.elapsed() < Duration::from_secs(10),
            "still running 10 s after SIGTERM"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "{status}");
    assert!(lines
        .map_while(Result::ok)
        .any(|l| l == "improv-server: shutting down"));
    std::fs::remove_dir_all(&dir).ok();
}
