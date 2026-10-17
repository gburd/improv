//! End-to-end: `save-view` writes a GUI-shaped `View` that `list` and `export` see.

use std::process::{Command, Output};

fn improv(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_improv"))
        .env("IMPROV_LOCALE", "en")
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
fn save_view_round_trips_and_bad_args_are_rejected() {
    let dir = std::env::temp_dir().join(format!("improv-save-view-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("m.db");
    let db = db.to_str().unwrap();
    ok(&["init", db]);
    ok(&["add-category", db, "1", "Time"]);
    ok(&["add-category", db, "2", "Product"]);
    ok(&["add-category", db, "3", "Scenario"]);
    for (id, cat, name) in [
        ("10", "1", "Q1"),
        ("20", "2", "Widget"),
        ("31", "3", "Actual"),
        ("30", "3", "Plan"),
    ] {
        ok(&["add-item", db, id, cat, name]);
    }
    ok(&[
        "add-measure",
        db,
        "100",
        "Units",
        "number",
        "input",
        "Time",
        "Product",
        "Scenario",
    ]);

    // Defaults: first category on rows, next on cols, the rest pages.
    ok(&["save-view", db, "1", "Default", "Units"]);
    // Explicit stacking: Product x Time on rows, Scenario on cols.
    ok(&[
        "save-view",
        db,
        "2",
        "By product",
        "100",
        "--rows",
        "Product,Time",
        "--cols",
        "Scenario",
    ]);

    let list = ok(&["list", db]);
    assert!(list.contains("views:\n  1 Default (measure Units, 1 matrices)\n  2 By product"));

    let model: serde_json::Value = serde_json::from_str(&ok(&["export", db])).unwrap();
    let view = |id: &str| model["views"][id].clone();
    let v1 = view("1");
    assert_eq!(v1["axis_order"], serde_json::json!([1, 2, 3]));
    assert_eq!(
        (v1["n_rows"].as_u64(), v1["n_cols"].as_u64()),
        (Some(1), Some(1))
    );
    // Page pinned to the lowest item id (Plan = 30), not insertion order.
    assert_eq!(v1["page_items"], serde_json::json!([[3, 30]]));
    let v2 = view("2");
    assert_eq!(v2["axis_order"], serde_json::json!([2, 1, 3]));
    assert_eq!(
        (v2["n_rows"].as_u64(), v2["n_cols"].as_u64()),
        (Some(2), Some(1))
    );
    assert_eq!(v2["page_items"], serde_json::json!([]));

    assert!(err(&["save-view", db, "3", " ", "Units"]).contains("name is required"));
    assert!(err(&["save-view", db, "3", "x", "Nope"]).contains("no measure"));
    assert!(
        err(&["save-view", db, "3", "x", "Units", "--rows", "Bogus"]).contains("not a category")
    );
    assert!(err(&[
        "save-view",
        db,
        "3",
        "x",
        "Units",
        "--rows",
        "Time",
        "--cols",
        "Time"
    ])
    .contains("used twice"));
    assert!(err(&["save-view", db, "3", "x", "Units", "--rows"]).contains("needs a value"));
    let _ = std::fs::remove_dir_all(&dir);
}
