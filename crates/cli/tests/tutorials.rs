//! The guide's CLI pages are tests: `docs/tutorials/mdtest.py` runs each
//! page's `sh` blocks in order and checks every `text` block against the
//! output of the block before it. A page edit that breaks a step, or a CLI
//! change that alters an output the page shows, fails here.

use std::path::Path;
use std::process::Command;

/// Run `page` (from the heading line `from` to the heading line `to`, when
/// given) in a fresh directory with this build's `improv` first on PATH.
fn run_page(page: &str, range: &[&str]) {
    // mdtest.py needs bash and python3; the Windows runners have neither
    // under those names.
    if cfg!(windows) || Command::new("python3").arg("-V").output().is_err() {
        println!("skipped: needs bash and python3");
        return;
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let bin = Path::new(env!("CARGO_BIN_EXE_improv")).parent().unwrap();
    let dir = std::env::temp_dir().join(format!(
        "improv-tutorial-{}-{}",
        std::process::id(),
        page.replace(['/', '.'], "-")
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new("python3")
        .arg(root.join("docs/tutorials/mdtest.py"))
        .arg(root.join(page))
        .arg("--dir")
        .arg(&dir)
        .args(range)
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

#[test]
fn tutorial_budget_runs_as_written() {
    run_page("docs/src/tutorial-budget.md", &[]);
}

/// Getting Started's quick start (its build steps before it need cargo).
#[test]
fn getting_started_quick_start_runs_as_written() {
    run_page(
        "docs/src/getting-started.md",
        &["--from", "## Quick start", "--to", "## Beyond the CLI"],
    );
}
