//! The tutorial pages are tests: `docs/tutorials/mdtest.py` runs each
//! page's `sh` blocks in order and checks every `text` block against the
//! output of the block before it. A page edit that breaks a step, or a CLI
//! change that alters an output the page shows, fails here.

use std::path::Path;
use std::process::Command;

#[test]
fn tutorial_budget_runs_as_written() {
    // mdtest.py needs bash and python3; the Windows runners have neither
    // under those names.
    if cfg!(windows) || Command::new("python3").arg("-V").output().is_err() {
        println!("skipped: needs bash and python3");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bin = Path::new(env!("CARGO_BIN_EXE_improv")).parent().unwrap();
    let dir = std::env::temp_dir().join(format!("improv-tutorial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new("python3")
        .arg(root.join("docs/tutorials/mdtest.py"))
        .arg(root.join("docs/src/tutorial-budget.md"))
        .arg("--dir")
        .arg(&dir)
        .env("PATH", path)
        .env("IMPROV_LOCALE", "en")
        .output()
        .expect("run mdtest.py");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
