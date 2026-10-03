//! End-to-end: every `--at` spelling writes the same cell; garbage is rejected.

use std::process::{Command, Output};

fn improv(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_improv"))
        .args(args)
        .output()
        .expect("run improv")
}

fn ok(args: &[&str]) -> String {
    let o = improv(args);
    assert!(
        o.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(args: &[&str]) -> String {
    let o = improv(args);
    assert!(!o.status.success(), "{args:?} unexpectedly succeeded");
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn set_at_forms_round_trip_and_garbage_is_rejected() {
    let dir = std::env::temp_dir().join(format!("improv-set-at-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("m.db");
    let db = db.to_str().unwrap();
    ok(&["init", db]);
    ok(&["add-category", db, "1", "Time"]);
    ok(&["add-category", db, "2", "Product"]);
    ok(&["add-item", db, "10", "1", "Q1"]);
    ok(&["add-item", db, "11", "1", "Q2"]);
    ok(&["add-item", db, "20", "2", "Widget"]);
    ok(&[
        "add-measure",
        db,
        "100",
        "Units",
        "number",
        "input",
        "Time",
        "Product",
    ]);

    ok(&["set", db, "100", "1", "--at", "Time=Q1,Product=Widget"]);
    ok(&[
        "set",
        db,
        "100",
        "2",
        "--at",
        "Time=Q2",
        "--at",
        "Product=Widget",
    ]);
    ok(&["set", db, "100", "3", "--at", "Time=Q1", "Product=Widget"]);
    let show = ok(&["show", db, "100"]);
    // The third write overwrote the first: same cell, not a new one.
    assert!(show.contains("{Time=Q1,Product=Widget} = 3"), "{show}");
    assert!(show.contains("{Time=Q2,Product=Widget} = 2"), "{show}");
    assert_eq!(show.matches("100::").count(), 2, "{show}");

    let e = err(&["set", db, "100", "9", "--at", "Time=Q1", "--at", "Time=Q2"]);
    assert!(e.contains("'Time' more than once"), "{e}");
    let e = err(&["set", db, "100", "9", "Time=Q1,Product=Widget"]);
    assert!(e.contains("did you mean '--at"), "{e}");
    err(&[
        "set",
        db,
        "100",
        "9",
        "--at",
        "Time=Q1",
        "Product=Widget",
        "junk",
    ]);

    // `scenario` shares the parser.
    ok(&[
        "scenario",
        db,
        "s",
        "100",
        "7",
        "--at",
        "Time=Q1",
        "Product=Widget",
    ]);

    std::fs::remove_dir_all(&dir).ok();
}
