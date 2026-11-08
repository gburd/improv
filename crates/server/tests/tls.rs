//! The server binary on a non-loopback address: with tokens it refuses
//! plain HTTP unless `IMPROV_ALLOW_PLAIN=1`, and serves with
//! `--tls-cert`/`--tls-key`.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("improv-srv-tls-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Start the server on 0.0.0.0:0 with a token; `Ok(first line)` once it
/// listens (then it is killed), `Err(stderr)` when it exits.
fn start(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> Result<String, (i32, String)> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_improv-server"));
    cmd.arg(dir.join("m.db"))
        .arg("0.0.0.0:0")
        .args(args)
        .env("IMPROV_API_TOKEN", "s3cret-tls-test")
        .env_remove("IMPROV_ALLOW_PLAIN")
        .env_remove("IMPROV_ALLOW_OPEN")
        .stderr(Stdio::piped())
        .stdout(Stdio::null())
        .stdin(Stdio::null());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("start improv-server");
    let mut seen = String::new();
    for line in BufReader::new(child.stderr.take().unwrap())
        .lines()
        .map_while(Result::ok)
    {
        if line.starts_with("improv-server listening on") {
            child.kill().ok();
            child.wait().ok();
            return Ok(line);
        }
        seen.push_str(&line);
        seen.push('\n');
    }
    let status = child.wait().unwrap();
    Err((status.code().unwrap_or(-1), seen))
}

#[test]
fn tokens_off_loopback_need_tls_or_the_opt_in() {
    let dir = scratch("plain");
    let (code, err) = start(&dir, &[], &[]).expect_err("served tokens over plain HTTP");
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("refusing to serve 0.0.0.0:")
            && err.contains(" over plain HTTP: ")
            && err.contains("--tls-cert and --tls-key")
            && err.contains("IMPROV_ALLOW_PLAIN=1"),
        "{err}"
    );
    assert!(!err.contains("s3cret"), "{err}");
    let line = start(&dir, &[], &[("IMPROV_ALLOW_PLAIN", "1")]).unwrap();
    assert_eq!(line, "improv-server listening on 0.0.0.0:0");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn tls_cert_and_key_serve_off_loopback() {
    let dir = scratch("tls");
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    std::fs::write(&cert, ck.cert.pem()).unwrap();
    std::fs::write(&key, ck.signing_key.serialize_pem()).unwrap();
    let (c, k) = (cert.to_str().unwrap(), key.to_str().unwrap());
    let line = start(&dir, &["--tls-cert", c, "--tls-key", k], &[]).unwrap();
    assert_eq!(line, "improv-server listening on 0.0.0.0:0 (TLS)");
    // A bad file stops it before it listens, naming the file.
    let (code, err) = start(&dir, &["--tls-cert", k, "--tls-key", k], &[]).unwrap_err();
    assert!(code == 2 && err.contains("no certificate in it"), "{err}");
    // One without the other is a usage error.
    let (code, err) = start(&dir, &["--tls-cert", c], &[]).unwrap_err();
    assert!(code == 2 && err.contains("--tls-key"), "{err}");
    std::fs::remove_dir_all(&dir).ok();
}
