//! End-to-end coverage of every `improv` subcommand through the real binary:
//! each command's success path (asserted on stdout or a store read-back) and
//! its argument / lookup error paths (asserted on stderr).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn improv(args: &[&str]) -> Output {
    // `IMPROV_TEST_STACK_KB`: run under that main-thread stack (`ulimit -s`).
    let mut cmd = match std::env::var("IMPROV_TEST_STACK_KB") {
        Ok(kb) if cfg!(unix) => {
            let mut c = Command::new("sh");
            c.arg("-c")
                .arg(format!("ulimit -s {kb} && exec \"$0\" \"$@\""))
                .arg(env!("CARGO_BIN_EXE_improv"));
            c
        }
        _ => Command::new(env!("CARGO_BIN_EXE_improv")),
    };
    cmd.env("IMPROV_LOCALE", "en")
        .args(args)
        .output()
        .expect("run improv")
}

fn improv_stdin(args: &[&str], input: &[u8]) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_improv"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn improv");
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().expect("wait improv")
}

/// Whether a `Restricted` Python body can run here: it needs python3 and a
/// working `bwrap` (the sandbox fails closed, e.g. in a container without
/// user namespaces). Tests that refresh a Python CALL measure skip otherwise.
fn python_sandbox() -> bool {
    use improv_core_model::{ExternalFn, Language, ValueType};
    let probe = ExternalFn {
        name: "probe".into(),
        language: Language::Python,
        body: "result = 1".into(),
        arg_types: vec![],
        return_type: ValueType::Number,
        pure: true,
    };
    let ok = improv_extfn::eval(&probe, &[], std::time::Duration::from_secs(5)).is_ok();
    if !ok {
        println!("skipped: python3 cannot run in the extfn sandbox here");
    }
    ok
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

fn fails_with(args: &[&str], needle: &str) {
    let e = err(args);
    assert!(e.contains(needle), "{args:?}: expected '{needle}' in: {e}");
}

fn stdout_has(out: &str, needle: &str) {
    assert!(out.contains(needle), "expected '{needle}' in:\n{out}");
}

/// A fresh, empty scratch directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("improv-cmd-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn file(dir: &Path, name: &str) -> String {
    dir.join(name).to_str().unwrap().to_owned()
}

/// Time{Q1,Q2} x Product{WidgetA,WidgetB}; Price[Product]=10 at WidgetA,
/// Quantity[Time,Product]=7 (Q1) and 3 (Q2) at WidgetA, and the derived
/// Revenue = Price * Quantity (= 70 and 30). Returns (dir, db).
fn model(name: &str) -> (PathBuf, String) {
    let dir = scratch(name);
    let db = file(&dir, "m.db");
    let d = db.as_str();
    ok(&["init", d]);
    ok(&["add-category", d, "1", "Time"]);
    ok(&["add-category", d, "2", "Product"]);
    ok(&["add-item", d, "10", "1", "Q1"]);
    ok(&["add-item", d, "11", "1", "Q2"]);
    ok(&["add-item", d, "20", "2", "WidgetA"]);
    ok(&["add-item", d, "21", "2", "WidgetB"]);
    ok(&[
        "add-measure",
        d,
        "100",
        "Price",
        "number",
        "input",
        "Product",
    ]);
    ok(&[
        "add-measure",
        d,
        "101",
        "Quantity",
        "number",
        "input",
        "Time",
        "Product",
    ]);
    ok(&["set", d, "100", "10", "--at", "Product=WidgetA"]);
    ok(&["set", d, "101", "7", "--at", "Time=Q1,Product=WidgetA"]);
    ok(&["set", d, "101", "3", "--at", "Time=Q2,Product=WidgetA"]);
    ok(&["add-derived", d, "102", "Revenue", "Price * Quantity"]);
    (dir, db)
}

fn load(db: &str) -> improv_core_model::Model {
    improv_store::Store::open(db).unwrap().load_model().unwrap()
}

/// `define` and `add-derived` used to save formulas that depend on each
/// other in a loop; every later `eval` of them then failed.
#[test]
fn a_formula_cycle_is_refused_and_named() {
    let (dir, db) = model("cycle");
    let d = db.as_str();
    ok(&["define", d, "300", "A1 = Quantity + 1"]);
    ok(&["define", d, "301", "B1 = A1 * 2"]);
    let before = ok(&["eval", d, "300"]);
    fails_with(
        &["define", d, "300", "A1 = B1 + 1"],
        "formula cycle: A1 → B1 → A1",
    );
    ok(&["define", d, "302", "C1 = B1 + 1"]);
    fails_with(
        &["add-derived", d, "300", "A1", "C1 + 1"],
        "formula cycle: A1 → C1 → B1 → A1",
    );
    // Nothing was saved: A1 still evaluates as before.
    assert_eq!(ok(&["eval", d, "300"]), before);
    // A measure's reference to itself is a recurrence, not a cycle.
    ok(&["define", d, "303", "Run = PREV(Run, Time, 1, 0) + Quantity"]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn help_prints_usage_for_every_spelling_and_no_args() {
    let cases: [&[&str]; 4] = [&[], &["help"], &["--help"], &["-h"]];
    for args in cases {
        let out = ok(args);
        stdout_has(&out, "Usage: improv [COMMAND]");
        stdout_has(&out, "serve-refresh");
    }
    // One command's help, three ways; it starts with the man page's NAME line.
    for args in [&["help", "set"][..], &["set", "--help"], &["set", "-h"]] {
        let out = ok(args);
        stdout_has(&out, "Set an input cell");
        stdout_has(
            &out,
            "Usage: improv set [OPTIONS] <db> <measure-id> <value>",
        );
    }
}

#[test]
fn version_prints_the_crate_version() {
    for flag in ["--version", "-V"] {
        assert_eq!(
            ok(&[flag]),
            format!("improv {}\n", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn unknown_command_is_a_usage_error() {
    let o = improv(&["frobnicate"]);
    assert_eq!(o.status.code(), Some(2));
    let e = String::from_utf8_lossy(&o.stderr);
    stdout_has(&e, "error: unknown command 'frobnicate'");
    stdout_has(&e, "Usage: improv [COMMAND]");
}

#[test]
fn init_creates_an_empty_store() {
    let dir = scratch("init");
    let db = file(&dir, "m.db");
    stdout_has(&ok(&["init", &db]), "initialized store at");
    assert_eq!(
        ok(&["list", &db]),
        "categories:\nitems:\nmeasures:\nviews:\ninputs:\n"
    );
    fails_with(&["init"], "missing argument <db>");
    fails_with(&["init", "/nonexistent-improv-dir/m.db"], "error:");
}

#[test]
fn add_category_and_item_and_their_errors() {
    let dir = scratch("cat-item");
    let db = file(&dir, "m.db");
    ok(&["init", &db]);
    assert_eq!(
        ok(&["add-category", &db, "1", "Time"]),
        "added category 1 'Time'\n"
    );
    assert_eq!(
        ok(&["add-item", &db, "10", "1", "Q1"]),
        "added item 10 'Q1' in category 1\n"
    );
    let list = ok(&["list", &db]);
    stdout_has(&list, "categories:\n  1 Time\n");
    stdout_has(&list, "items:\n  10 Q1 (category 1)\n");

    fails_with(
        &["add-category", &db, "x", "T"],
        "<id> must be a number, got 'x'",
    );
    fails_with(&["add-category", &db, "2"], "missing argument <name>");
    fails_with(&["add-item", &db, "11", "9", "Q2"], "no category with id 9");
    fails_with(
        &["add-item", &db, "11", "y", "Q2"],
        "<category-id> must be a number",
    );
    assert_eq!(load(&db).items.len(), 1, "failed adds must not write");
}

#[test]
fn add_measure_number_boolean_text_and_errors() {
    let (_dir, db) = model("add-measure");
    assert_eq!(
        ok(&["add-measure", &db, "103", "Flag", "boolean", "input"]),
        "added input measure 103 'Flag'\n"
    );
    ok(&["add-measure", &db, "104", "Note", "text", "input"]);
    let list = ok(&["list", &db]);
    stdout_has(&list, "  100 Price [input] Number\n");
    stdout_has(&list, "  102 Revenue [derived] Number\n");
    stdout_has(&list, "  103 Flag [input] Boolean\n");
    stdout_has(&list, "  104 Note [input] Text\n");

    fails_with(
        &["add-measure", &db, "105", "D", "date", "input"],
        "type must be number|boolean|text, got 'date'",
    );
    fails_with(
        &["add-measure", &db, "105", "D", "number", "derived"],
        "only 'input' measures are supported, got 'derived'",
    );
    fails_with(
        &["add-measure", &db, "105", "D", "number", "input", "Nope"],
        "unknown category 'Nope'",
    );
    fails_with(&["add-measure", &db, "105", "D"], "missing argument <type>");
}

#[test]
fn set_parses_by_declared_type_and_validates_the_coordinate() {
    let (_dir, db) = model("set");
    ok(&["add-measure", &db, "103", "Flag", "boolean", "input"]);
    ok(&["add-measure", &db, "104", "Note", "text", "input"]);
    assert_eq!(ok(&["set", &db, "103", "true"]), "set measure 103\n");
    ok(&["set", &db, "104", "hello"]);
    stdout_has(&ok(&["show", &db, "103"]), "103::{} = true");
    stdout_has(&ok(&["show", &db, "104"]), "104::{} = \"hello\"");

    fails_with(&["set", &db, "999", "1"], "no measure with id 999");
    fails_with(
        &["set", &db, "103", "yes"],
        "value must be true|false, got 'yes'",
    );
    fails_with(
        &["set", &db, "100", "ten", "--at", "Product=WidgetA"],
        "value must be a number, got 'ten'",
    );
    fails_with(
        &["set", &db, "100", "1", "--at", "Product=WidgetA,Bad"],
        "--at pair must be Cat=Item, got 'Bad'",
    );
    fails_with(
        &["set", &db, "100", "1", "--at", "Product=Nope"],
        "no item named 'Nope' in category 'Product'",
    );
    fails_with(&["set", &db, "100", "1", "--at"], "--at needs an argument");
    // Every declared dimension given, plus one the measure is not over.
    let e = err(&["set", &db, "100", "1", "--at", "Product=WidgetA,Time=Q1"]);
    stdout_has(&e, "not a dimension of it: Time");
    assert!(!e.contains("missing"), "{e}");
    fails_with(&["set", &db, "100"], "missing argument <value>");
}

#[test]
fn show_prints_one_measure_and_only_its_cells() {
    let (_dir, db) = model("show");
    let out = ok(&["show", &db, "101"]);
    stdout_has(&out, "measure 101 Quantity [input] Number\n  cells:\n");
    stdout_has(&out, "    101::{Time=Q1,Product=WidgetA} = 7\n");
    stdout_has(&out, "    101::{Time=Q2,Product=WidgetA} = 3\n");
    assert!(!out.contains("100::"), "{out}");
    stdout_has(
        &ok(&["show", &db, "102"]),
        "measure 102 Revenue [derived] Number",
    );
    fails_with(&["show", &db, "999"], "no measure with id 999");
    fails_with(&["show", &db, "x"], "<measure-id> must be a number");
}

#[test]
fn show_prints_a_measure_description() {
    let (_dir, db) = model("show-desc");
    {
        let mut store = improv_store::Store::open(&db).unwrap();
        let mut m = store.load_model().unwrap();
        m.measures
            .get_mut(&improv_core_model::MeasureId(100))
            .unwrap()
            .description = Some("unit price".into());
        store.save_model(&m).unwrap();
    }
    stdout_has(&ok(&["show", &db, "100"]), "  description: unit price\n");
}

#[test]
fn list_prints_every_section_sorted() {
    let (_dir, db) = model("list");
    let out = ok(&["list", &db]);
    stdout_has(
        &out,
        "measures:\n  100 Price [input] Number\n  101 Quantity [input] Number\n  102 Revenue [derived] Number\nviews:\ninputs:\n",
    );
    stdout_has(&out, "  100::{Product=WidgetA} = 10\n");
    stdout_has(&out, "items:\n  10 Q1 (category 1)\n  11 Q2 (category 1)\n");
    fails_with(&["list"], "missing argument <db>");
}

#[test]
fn export_prints_the_model_as_json() {
    let (_dir, db) = model("export");
    let out = ok(&["export", &db]);
    let json: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert!(json.is_object());
    stdout_has(&out, "\"Revenue\"");
    stdout_has(&out, "\"WidgetB\"");
    fails_with(&["export"], "missing argument <db>");
}

#[test]
fn add_derived_then_eval_prints_every_cell() {
    let (_dir, db) = model("eval");
    assert_eq!(
        ok(&["eval", &db, "102"]),
        "eval 102 'Revenue' (2 cells):\n  Revenue[Time=Q1, Product=WidgetA] = 70\n  Revenue[Time=Q2, Product=WidgetA] = 30\n"
    );
    fails_with(&["add-derived", &db, "103", "X", "Nope * 2"], "error:");
    fails_with(
        &["add-derived", &db, "103", "X"],
        "missing argument <formula>",
    );
    fails_with(&["eval", &db, "100"], "is an input measure");
    fails_with(&["eval", &db, "999"], "no measure with id 999");
    fails_with(&["eval", &db], "missing argument <measure-id>");
}

#[test]
fn eval_reports_an_engine_compile_error() {
    let (_dir, db) = model("eval-bad");
    ok(&["add-measure", &db, "105", "A", "number", "input", "Time"]);
    ok(&["add-measure", &db, "106", "B", "number", "input", "Product"]);
    // add-derived checks the formula now (its shape comes from the engine).
    fails_with(
        &["add-derived", &db, "107", "Bad", "A + B"],
        "neither operand dim",
    );
}

/// `define` makes a named LAMBDA; formulas call it by name and it persists.
#[test]
fn define_a_named_lambda() {
    let (_dir, db) = model("lambda");
    let out = ok(&["define", &db, "5", "Margin = LAMBDA(p, q, p - q / 2)"]);
    stdout_has(&out, "defined LAMBDA 5 'Margin'(p, q)");
    // Redefining keeps the id.
    let out = ok(&["define", &db, "6", "Margin = LAMBDA(p, q, p - q)"]);
    stdout_has(&out, "defined LAMBDA 5");
    ok(&["define", &db, "120", "Net = Margin(Price, 1)"]);
    let out = ok(&["eval", &db, "120"]);
    stdout_has(&out, "9");
    fails_with(
        &["define", &db, "7", "Quantity = LAMBDA(x, x)"],
        "is a measure",
    );
    fails_with(&["define", &db, "5", "Other = LAMBDA(x, x)"], "is taken");
    fails_with(
        &["define", &db, "8", "Bad = LAMBDA(x, x + Price)"],
        "only its parameters",
    );
    // A parameter used as a category takes a category at the call.
    ok(&["define", &db, "9", "Norm = LAMBDA(x, c, x / MAX(x OVER c))"]);
    ok(&["define", &db, "121", "Share = Norm(Quantity, Time)"]);
    let out = ok(&["eval", &db, "121"]);
    stdout_has(&out, "Share[Time=Q1, Product=WidgetA] = 1\n");
    stdout_has(&out, "Share[Time=Q2, Product=WidgetA] = 0.428");
    fails_with(
        &["define", &db, "122", "Bad = Norm(Quantity, Price)"],
        "unknown category: Price",
    );
}

#[test]
fn define_formula_and_its_errors() {
    let (_dir, db) = model("define");
    assert_eq!(
        ok(&["define", &db, "103", "Rev2 = Price * Quantity"]),
        "defined derived measure 103 'Rev2'\n"
    );
    stdout_has(
        &ok(&["eval", &db, "103"]),
        "Rev2[Time=Q1, Product=WidgetA] = 70",
    );
    fails_with(
        &["define", &db, "104", "S = SQL(\"SELECT 1\")"],
        "use 'import-sql'",
    );
    fails_with(
        &["define", &db, "104", "H = CALL(nofn, Price)"],
        "external function 'nofn' is not registered",
    );
    fails_with(&["define", &db, "104", "no equals here"], "error:");
    fails_with(&["define", &db, "104"], "missing argument <definition>");
    assert!(load(&db).measure_by_name("S").is_none());
}

#[test]
fn register_ext_define_call_and_refresh_ext() {
    if !python_sandbox() {
        return;
    }
    let (_dir, db) = model("ext");
    assert_eq!(
        ok(&["register-ext", &db, "dbl", "1", "result = args[0] * 2"]),
        "registered external function 'dbl' (arity 1)\n"
    );
    stdout_has(
        &ok(&[
            "define",
            &db,
            "200",
            "D = CALL(dbl, Price)",
            "--refresh",
            "on-load",
        ]),
        "defined external-call measure 200 'D' = dbl(...)",
    );
    assert_eq!(
        ok(&["refresh-ext", &db, "200"]),
        "refreshed external measure 200 (1 cells)\n"
    );
    stdout_has(&ok(&["show", &db, "200"]), "200::{Product=WidgetA} = 20");
    // No SQL-backed measures: refresh-all needs no source and prints no note.
    let o = improv(&["refresh-all", &db]);
    assert!(o.status.success());
    stdout_has(
        &String::from_utf8_lossy(&o.stdout),
        "refresh-all: 1 cells refreshed",
    );
    assert!(
        o.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );

    // Two arguments sharing a category: the call ranges over their union.
    ok(&[
        "register-ext",
        &db,
        "mul",
        "2",
        "result = args[0] * args[1]",
    ]);
    ok(&["define", &db, "201", "PQ = CALL(mul, Price, Quantity)"]);
    let m = load(&db);
    let pq = m.measure_by_name("PQ").unwrap();
    assert_eq!(pq.categories.len(), 2);

    fails_with(
        &["define", &db, "202", "E = CALL(dbl, Nope)"],
        "unknown argument measure 'Nope'",
    );
    fails_with(
        &[
            "define",
            &db,
            "202",
            "E = CALL(dbl, Price)",
            "--refresh",
            "hourly",
        ],
        "unknown refresh policy 'hourly'",
    );
    fails_with(&["refresh-ext", &db, "100"], "error:");
    fails_with(&["refresh-ext", &db], "missing argument <measure-id>");
    fails_with(
        &["register-ext", &db, "f", "two", "result = 1"],
        "<arity> must be a number",
    );
    fails_with(
        &["register-ext", &db, "f", "1"],
        "missing argument <python-body>",
    );
}

#[test]
fn scenario_overrides_apply_only_under_eval_scenario() {
    let (_dir, db) = model("scenario");
    assert_eq!(
        ok(&[
            "scenario",
            &db,
            "hi",
            "100",
            "20",
            "--at",
            "Product=WidgetA"
        ]),
        "scenario 'hi': override on measure 100 recorded\n"
    );
    stdout_has(
        &ok(&["eval", &db, "102", "--scenario", "hi"]),
        "Revenue[Time=Q1, Product=WidgetA] = 140",
    );
    stdout_has(
        &ok(&["eval", &db, "102"]),
        "Revenue[Time=Q1, Product=WidgetA] = 70",
    );
    // A second override on the same scenario updates it; a new name mints one.
    ok(&[
        "scenario",
        &db,
        "hi",
        "101",
        "1",
        "--at",
        "Time=Q1,Product=WidgetA",
    ]);
    ok(&["scenario", &db, "lo", "100", "1", "--at", "Product=WidgetA"]);
    stdout_has(
        &ok(&["eval", &db, "102", "--scenario", "hi"]),
        "Revenue[Time=Q1, Product=WidgetA] = 20",
    );
    let m = load(&db);
    assert_eq!(m.scenarios.len(), 2);
    assert_eq!(m.scenario_by_name("hi").unwrap().overrides.len(), 2);

    fails_with(
        &["scenario", &db, "hi", "999", "1"],
        "no measure with id 999",
    );
    fails_with(
        &["scenario", &db, "hi", "100", "x", "--at", "Product=WidgetA"],
        "value must be a number",
    );
    fails_with(&["scenario", &db, "hi", "100"], "missing argument <value>");
    fails_with(
        &["eval", &db, "102", "--scenario", "nope"],
        "no scenario named 'nope'",
    );
    fails_with(
        &["eval", &db, "102", "--scenario"],
        "--scenario needs a name",
    );
}

#[test]
fn save_view_success_and_errors() {
    let (_dir, db) = model("save-view");
    assert_eq!(
        ok(&[
            "save-view",
            &db,
            "1",
            "V",
            "Revenue",
            "--rows",
            "Product",
            "--cols",
            "Time"
        ]),
        "saved view 1 'V'\n"
    );
    stdout_has(
        &ok(&["list", &db]),
        "views:\n  1 V (measure Revenue, 1 matrices)\n",
    );
    fails_with(
        &["save-view", &db, "2", "  ", "102"],
        "view name is required",
    );
    fails_with(
        &["save-view", &db, "2", "V", "102", "--bogus"],
        "unexpected argument '--bogus'",
    );
    fails_with(
        &["save-view", &db, "2", "V", "102", "--rows"],
        "--rows needs a value",
    );
    fails_with(
        &["save-view", &db, "2", "V", "102", "--rows", "Nope"],
        "'Nope' is not a category of measure 102",
    );
    fails_with(
        &[
            "save-view",
            &db,
            "2",
            "V",
            "102",
            "--rows",
            "Time",
            "--cols",
            "Time",
        ],
        "category 1 is used twice",
    );
    fails_with(
        &["save-view", &db, "2", "V", "Zzz"],
        "no measure named or numbered 'Zzz'",
    );
    assert_eq!(load(&db).views.len(), 1);
}

#[test]
fn import_csv_then_export_csv_round_trips() {
    let (dir, db) = model("csv");
    let csv = file(&dir, "sales.csv");
    std::fs::write(&csv, "t,p,r\n2025,A,1000\n2025,B,500\n").unwrap();
    assert_eq!(
        ok(&[
            "import-csv",
            &db,
            &csv,
            "300",
            "Sales",
            "r",
            "t:5:Year",
            "p:6:Region"
        ]),
        "imported 2 cells into measure 300 'Sales'\n"
    );
    let shown = ok(&["show", &db, "300"]);
    assert!(
        shown.contains("{Year=2025,Region=A} = 1000")
            || shown.contains("{Region=A,Year=2025} = 1000"),
        "{shown}"
    );

    let out_csv = file(&dir, "out.csv");
    stdout_has(
        &ok(&["export-csv", &db, &out_csv, "300"]),
        "exported 2 rows from measure 300",
    );
    let text = std::fs::read_to_string(&out_csv).unwrap();
    // Column order follows the loaded measure's category order, which is not
    // stable across a store round-trip; check the content, not the order.
    let header = text.lines().next().unwrap();
    for col in ["Year", "Region", "Sales"] {
        assert!(header.split(',').any(|c| c == col), "{text}");
    }
    assert!(text.contains("1000") && text.contains("500"), "{text}");

    let out_tsv = file(&dir, "out.tsv");
    ok(&["export-csv", &db, &out_tsv, "300"]);
    assert!(std::fs::read_to_string(&out_tsv).unwrap().contains('\t'));
    let out_txt = file(&dir, "out.txt");
    ok(&["export-csv", &db, &out_txt, "300", "--tsv"]);
    assert!(std::fs::read_to_string(&out_txt).unwrap().contains('\t'));
}

#[test]
fn import_csv_tsv_and_headerless_forms() {
    let (dir, db) = model("csv-forms");
    let tsv = file(&dir, "s.tsv");
    std::fs::write(&tsv, "2025\tA\t7\n").unwrap();
    ok(&[
        "import-csv",
        &db,
        &tsv,
        "301",
        "T",
        "2",
        "0:7:Y",
        "1:8:R",
        "--no-header",
    ]);
    let shown = ok(&["show", &db, "301"]);
    assert!(
        shown.contains("{Y=2025,R=A} = 7") || shown.contains("{R=A,Y=2025} = 7"),
        "{shown}"
    );

    let txt = file(&dir, "s.txt");
    std::fs::write(&txt, "t\tr\n2026\t9\n").unwrap();
    ok(&["import-csv", &db, &txt, "302", "U", "r", "t:9:Z", "--tsv"]);
    stdout_has(&ok(&["show", &db, "302"]), "302::{Z=2026} = 9");

    // Headerless with a non-index column reference cannot resolve.
    fails_with(
        &[
            "import-csv",
            &db,
            &tsv,
            "303",
            "W",
            "r",
            "t:10:Q",
            "--no-header",
        ],
        "error:",
    );
}

#[test]
fn import_csv_and_export_csv_errors() {
    let (dir, db) = model("csv-err");
    let csv = file(&dir, "s.csv");
    std::fs::write(&csv, "t,r\n2025,1\n").unwrap();
    fails_with(
        &["import-csv", &db, &csv, "300", "S", "r", "t:1"],
        "dimension spec 't:1' must be <col>:<cat-id>:<cat-name>",
    );
    fails_with(
        &["import-csv", &db, &csv, "300", "S", "r"],
        "at least one dimension mapping is required",
    );
    fails_with(
        &["import-csv", &db, &csv, "300", "S", "r", "t:x:Year"],
        "<cat-id> must be a number",
    );
    let missing = file(&dir, "missing.csv");
    fails_with(
        &["import-csv", &db, &missing, "300", "S", "r", "t:5:Year"],
        "error:",
    );
    fails_with(
        &["import-csv", &db, &csv, "300", "S"],
        "missing argument <value-col>",
    );
    fails_with(
        &["export-csv", &db, "/nonexistent-improv-dir/o.csv", "100"],
        "error:",
    );
    fails_with(&["export-csv", &db, &csv], "missing argument <measure-id>");
    assert!(load(&db).measure_by_name("S").is_none());
}

fn sales_source(dir: &Path) -> String {
    let src = file(dir, "sales.sqlite");
    let conn = rusqlite::Connection::open(&src).unwrap();
    conn.execute_batch(
        "CREATE TABLE sales(t TEXT, p TEXT, r REAL);
         INSERT INTO sales VALUES ('2025','A',1000.0), ('2025','B',500.0);",
    )
    .unwrap();
    src
}

#[test]
fn import_sql_refresh_sql_and_export_sql() {
    let (dir, db) = model("sql");
    let src = sales_source(&dir);
    stdout_has(
        &ok(&[
            "import-sql",
            &db,
            &src,
            "400",
            "Rev",
            "SELECT p,r FROM sales",
            "r",
            "--refresh",
            "interval:60",
            "p:6:Region",
        ]),
        "imported 2 cells into SQL-backed measure 400 'Rev'",
    );
    assert_eq!(
        load(&db).sql_sources[&improv_core_model::MeasureId(400)].refresh_policy,
        improv_core_model::RefreshPolicy::Interval { secs: 60 }
    );
    rusqlite::Connection::open(&src)
        .unwrap()
        .execute("UPDATE sales SET r = 2000.0 WHERE p = 'A'", [])
        .unwrap();
    assert_eq!(
        ok(&["refresh-sql", &db, &src, "400"]),
        "refreshed measure 400 from SQL: 2 cells\n"
    );
    // Two dimensions: see `refresh_sql_keeps_a_two_dim_mapping_across_reload`.
    stdout_has(&ok(&["show", &db, "400"]), "400::{Region=A} = 2000");

    let target = file(&dir, "out.sqlite");
    assert_eq!(
        ok(&["export-sql", &db, &target, "400", "sales_out", "val"]),
        "exported 2 cells from measure 400 to table 'sales_out'\n"
    );
    let total: f64 = rusqlite::Connection::open(&target)
        .unwrap()
        .query_row("SELECT SUM(val) FROM sales_out", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 2500.0);
}

/// A 2-dim SQL measure whose dimension order is NOT ascending by category id
/// (Year=7 before Region=6) maps SQL columns to the same categories after a
/// store reload. The store used to hand the categories back sorted by id, so
/// `refresh-sql` wrote `{Year=A,Region=2025}`.
#[test]
fn refresh_sql_keeps_a_two_dim_mapping_across_reload() {
    let (dir, db) = model("sql-2dim");
    let src = sales_source(&dir);
    ok(&[
        "import-sql",
        &db,
        &src,
        "400",
        "Rev",
        "SELECT t,p,r FROM sales",
        "r",
        "t:7:Year",
        "p:6:Region",
    ]);
    let m400 = improv_core_model::MeasureId(400);
    let cats = |db: &str| load(db).measures[&m400].categories.clone();
    let want = vec![
        improv_core_model::CategoryId(7),
        improv_core_model::CategoryId(6),
    ];
    assert_eq!(cats(&db), want);
    rusqlite::Connection::open(&src)
        .unwrap()
        .execute("UPDATE sales SET r = 2000.0 WHERE p = 'A'", [])
        .unwrap();
    ok(&["refresh-sql", &db, &src, "400"]);
    assert_eq!(cats(&db), want);
    let shown = ok(&["show", &db, "400"]);
    stdout_has(&shown, "400::{Region=A,Year=2025} = 2000");
    stdout_has(&shown, "400::{Region=B,Year=2025} = 500");
    assert!(
        !shown.contains("Region=2025"),
        "swapped categories:\n{shown}"
    );
}

#[test]
fn sql_commands_report_their_errors() {
    let (dir, db) = model("sql-err");
    let src = sales_source(&dir);
    let q = "SELECT t,p,r FROM sales";
    fails_with(
        &["import-sql", &db, &src, "400", "R", q, "r", "t:5"],
        "must be <col>:<cat-id>:<cat-name>",
    );
    fails_with(
        &["import-sql", &db, &src, "400", "R", q, "r"],
        "at least one dimension mapping is required",
    );
    fails_with(
        &[
            "import-sql",
            &db,
            &src,
            "400",
            "R",
            "SELECT nope FROM nowhere",
            "r",
            "t:5:Y",
        ],
        "error:",
    );
    fails_with(
        &[
            "import-sql",
            &db,
            "/nonexistent-improv-dir/s.db",
            "400",
            "R",
            q,
            "r",
            "t:5:Y",
        ],
        "error:",
    );
    fails_with(
        &["import-sql", &db, &src, "400", "R", q],
        "missing argument <value-col>",
    );
    fails_with(&["refresh-sql", &db, &src, "100"], "error:");
    fails_with(
        &["refresh-sql", &db, "/nonexistent-improv-dir/s.db", "100"],
        "error:",
    );
    fails_with(&["refresh-sql", &db, &src], "missing argument <measure-id>");
    let target = file(&dir, "out.sqlite");
    fails_with(
        &["export-sql", &db, &target, "100", "bad name", "val"],
        "error:",
    );
    fails_with(
        &[
            "export-sql",
            &db,
            "/nonexistent-improv-dir/o.db",
            "100",
            "t",
            "v",
        ],
        "error:",
    );
    fails_with(
        &["export-sql", &db, &target, "100", "t"],
        "missing argument <value-col>",
    );
    assert!(load(&db).sql_sources.is_empty());
}

#[test]
fn refresh_all_refreshes_external_and_sql_measures() {
    if !python_sandbox() {
        return;
    }
    let (dir, db) = model("refresh-all");
    let src = sales_source(&dir);
    ok(&["register-ext", &db, "dbl", "1", "result = args[0] * 2"]);
    ok(&["define", &db, "200", "D = CALL(dbl, Price)"]);
    ok(&[
        "import-sql",
        &db,
        &src,
        "400",
        "Rev",
        "SELECT t,p,r FROM sales",
        "r",
        "t:5:Year",
        "p:6:Region",
    ]);

    let o = improv(&["refresh-all", &db]);
    assert!(o.status.success());
    let out = String::from_utf8_lossy(&o.stdout);
    stdout_has(&out, "refreshed external measure 200 (1 cells)");
    stdout_has(&out, "refresh-all: 1 cells refreshed");
    stdout_has(
        &String::from_utf8_lossy(&o.stderr),
        "1 SQL-backed measure(s) skipped",
    );

    let out = ok(&["refresh-all", &db, &src]);
    stdout_has(&out, "refreshed SQL measure 400 (2 cells)");
    stdout_has(&out, "refresh-all: 3 cells refreshed");
    stdout_has(&ok(&["show", &db, "200"]), "200::{Product=WidgetA} = 20");

    fails_with(
        &["refresh-all", &db, "/nonexistent-improv-dir/s.db"],
        "error:",
    );
    let empty = file(&dir, "empty.sqlite");
    fails_with(&["refresh-all", &db, &empty], "error:");
    // Re-registering with the wrong arity makes the CALL refresh fail.
    ok(&["register-ext", &db, "dbl", "2", "result = 0"]);
    fails_with(&["refresh-all", &db], "error:");
    fails_with(&["refresh-all"], "missing argument <db>");
}

#[test]
fn stream_applies_edits_and_prints_changed_cells() {
    let (_dir, db) = model("stream");
    let o = improv_stdin(
        &["stream", &db, "Revenue"],
        b"# a comment\n\nPrice 12 Product=WidgetA\nbogus a b c\n",
    );
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        "Revenue Time=Q1, Product=WidgetA = 84\nRevenue Time=Q2, Product=WidgetA = 36\n"
    );
    stdout_has(&String::from_utf8_lossy(&o.stderr), "stream: line 4:");
    stdout_has(&ok(&["show", &db, "100"]), "100::{Product=WidgetA} = 12");

    // An input target is warned about, not rejected.
    let o = improv_stdin(&["stream", &db, "102", "Price"], b"");
    assert!(o.status.success());
    stdout_has(
        &String::from_utf8_lossy(&o.stderr),
        "warning: target measure 100 'Price' is an input measure",
    );
}

#[test]
fn stream_errors() {
    let (_dir, db) = model("stream-err");
    fails_with(&["stream", &db], "stream needs at least one target measure");
    fails_with(
        &["stream", &db, "Nope"],
        "no measure named or numbered 'Nope'",
    );
    fails_with(&["stream"], "missing argument <db>");
    let o = improv_stdin(&["stream", &db, "Revenue"], b"Price \xff\xfe 1\n");
    assert!(!o.status.success());
    stdout_has(&String::from_utf8_lossy(&o.stderr), "UTF-8");

    ok(&["add-measure", &db, "105", "A", "number", "input", "Time"]);
    ok(&["add-measure", &db, "106", "B", "number", "input", "Product"]);
    fails_with(
        &["add-derived", &db, "107", "Bad", "A + B"],
        "neither operand dim",
    );
}

#[test]
fn serve_refresh_rejects_a_bad_tick() {
    let (_dir, db) = model("serve");
    fails_with(
        &["serve-refresh", &db, "--tick", "x"],
        "--tick SECS must be a number",
    );
    fails_with(
        &["serve-refresh", &db, "--tick"],
        "--tick needs a value in seconds",
    );
    fails_with(&["serve-refresh"], "missing argument <db>");
}

/// Make every later `load_model`/`load_partial` of `db` fail: overwrite the
/// stored external-functions JSON blob with garbage.
fn corrupt_meta(db: &str) {
    let n = rusqlite::Connection::open(db)
        .unwrap()
        .execute(
            "UPDATE datoms SET v = 'not json' \
             WHERE a = (SELECT e FROM idents WHERE v = ':meta/external-fns')",
            [],
        )
        .unwrap();
    assert_eq!(n, 1, "expected one external-fns blob to corrupt");
}

/// Make every later write of new data to `db` fail (reads, and the idempotent
/// schema transaction `open` runs, still succeed).
fn block_writes(db: &str) {
    rusqlite::Connection::open(db)
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER improv_test_block BEFORE INSERT ON datoms \
             WHEN NEW.a <> (SELECT e FROM idents WHERE v = ':db/txInstant') \
             BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;",
        )
        .unwrap();
}

/// C3: a 71-term chain was saved, and then no command could open the
/// model ("recursion limit exceeded"). C3b: the limit is 256, for every
/// shape; `IMPROV_TEST_STACK_KB` runs the commands under `ulimit -s` (CI's
/// Windows main thread is 1 MiB: `IMPROV_TEST_STACK_KB=1024`).
/// `improv list m.db | head -1`: the reader closes the pipe early, and the
/// CLI exits quietly instead of panicking.
#[cfg(unix)]
#[test]
fn a_closed_stdout_is_not_a_panic() {
    let (_dir, d) = model("pipe");
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_improv"))
        .args(["list", &d])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert!(
        matches!(out.status.code(), Some(0) | Some(141)),
        "{:?}",
        out.status
    );
}

/// H1-9 and H1-11: a reused id or name is refused (it silently renamed or
/// replaced), and a mistyped model path is an error, not a new empty model.
#[test]
fn add_refuses_a_taken_id_or_name_and_open_refuses_a_missing_model() {
    let dir = scratch("taken");
    let d = &file(&dir, "m.db");
    ok(&["init", d]);
    ok(&["add-category", d, "1", "Month"]);
    fails_with(&["add-category", d, "1", "Foo"], "category id 1 is in use");
    fails_with(
        &["add-category", d, "2", "Month"],
        "the name \"Month\" is taken by category 1",
    );
    ok(&["add-item", d, "10", "1", "Jan"]);
    fails_with(&["add-item", d, "10", "1", "Feb"], "item id 10 is in use");
    fails_with(
        &["add-item", d, "11", "1", "Jan"],
        "category 1 already has an item \"Jan\"",
    );
    ok(&["add-measure", d, "100", "Spent", "number", "input", "Month"]);
    fails_with(
        &["add-measure", d, "100", "Other", "number", "input"],
        "measure id 100 is in use",
    );
    fails_with(
        &["add-measure", d, "101", "Month", "number", "input"],
        "taken by category 1",
    );
    stdout_has(&ok(&["list", d]), "1 Month");
    let missing = file(&dir, "typo.db");
    fails_with(&["list", &missing], "no model at");
    assert!(
        !std::path::Path::new(&missing).exists(),
        "list created {missing}"
    );
}

#[test]
fn set_refuses_a_formula_measure() {
    let dir = scratch("set-derived");
    let d = &file(&dir, "m.db");
    ok(&["init", d]);
    ok(&["add-measure", d, "100", "P", "number", "input"]);
    ok(&["set", d, "100", "3"]);
    ok(&["add-derived", d, "101", "Q", "P * 2"]);
    fails_with(
        &["set", d, "101", "9"],
        "measure 101 Q is computed by its formula, not set",
    );
    stdout_has(&ok(&["eval", d, "101"]), "Q[] = 6");
}

#[test]
fn a_formula_too_deep_is_refused_and_an_old_one_does_not_lock_the_model() {
    let dir = scratch("depth");
    let db = file(&dir, "m.db");
    let d = db.as_str();
    ok(&["init", d]);
    ok(&["add-measure", d, "100", "Price", "number", "input"]);
    let chain = |n: usize| vec!["Price"; n].join(" + ");
    let abs = |n: usize| format!("{}Price{}", "ABS(".repeat(n - 1), ")".repeat(n - 1));
    let if_ = |n: usize| {
        format!(
            "{}Price{}",
            "IF(Price > 1, ".repeat(n - 2),
            ", Price)".repeat(n - 2)
        )
    };
    let over = "formula nests too deeply (257 levels; the limit is 256)";
    fails_with(&["add-derived", d, "101", "Q", &chain(300)], over);
    for f in [chain(257), abs(257), if_(257)] {
        fails_with(&["add-derived", d, "101", "Q", &f], over);
        fails_with(&["define", d, "101", &format!("Q = {f}")], over);
    }
    // At the limit: each shape saves, exports, imports into a new store and
    // evaluates (Price = 2).
    ok(&["set", d, "100", "2"]);
    for (id, name, f) in [("102", "R", abs(256)), ("103", "S", if_(256))] {
        ok(&["define", d, id, &format!("{name} = {f}")]);
        stdout_has(&ok(&["eval", d, id]), &format!("{name}[] = 2"));
    }
    ok(&["add-derived", d, "101", "Q", &chain(256)]);
    stdout_has(&ok(&["eval", d, "101"]), "Q[] = 512");
    stdout_has(&ok(&["list", d]), "101 Q [derived]");
    let json = ok(&["export", d]);
    let model: improv_core_model::Model = improv_core_model::json::from_str(&json).unwrap();
    let copy = file(&dir, "copy.db");
    improv_store::Store::open(&copy)
        .unwrap()
        .save_model(&model)
        .unwrap();
    stdout_has(&ok(&["eval", &copy, "101"]), "Q[] = 512");

    // A store written before the limit: Q's formula is a 300-term chain,
    // past the load bound (256).
    let mut e = r#"{"Ref":[100,{"by":[],"over":[],"except":[]}]}"#.to_string();
    for _ in 1..300 {
        e = format!(
            r#"{{"BinaryOp":["Add",{e},{{"Ref":[100,{{"by":[],"over":[],"except":[]}}]}}]}}"#
        );
    }
    // Q is the measure with id 101 (`:measure/id`).
    let n = rusqlite::Connection::open(d)
        .unwrap()
        .execute(
            "UPDATE datoms SET v = ?1 \
             WHERE a = (SELECT e FROM idents WHERE v = ':measure/formula') \
               AND e = (SELECT e FROM datoms WHERE v = 101 \
                 AND a = (SELECT e FROM idents WHERE v = ':measure/id'))",
            [format!(r#"{{"expr":{e}}}"#)],
        )
        .unwrap();
    assert_eq!(n, 1);
    stdout_has(&ok(&["list", d]), "101 Q [derived]");
    stdout_has(
        &ok(&["show", d, "101"]),
        "nests too deeply to load (measure 101 Q)",
    );
    stdout_has(&ok(&["eval", d, "101"]), "Q[] = #REF!");
    // Redefining it fixes it.
    ok(&["add-derived", d, "101", "Q", "Price * 2"]);
    stdout_has(&ok(&["eval", d, "101"]), "Q[] = 4");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn every_command_reports_a_store_load_failure() {
    let (dir, db) = model("load-fail");
    let src = sales_source(&dir);
    let csv = file(&dir, "s.csv");
    std::fs::write(&csv, "p,r\nA,1\n").unwrap();
    let out = file(&dir, "out.sqlite");
    let out_csv = file(&dir, "out.csv");
    ok(&["register-ext", &db, "dbl", "1", "result = args[0] * 2"]);
    corrupt_meta(&db);
    let d = db.as_str();
    let q = "SELECT p,r FROM sales";
    let cases: Vec<Vec<&str>> = vec![
        vec!["add-category", d, "3", "C"],
        vec!["add-item", d, "12", "1", "Q3"],
        vec!["add-measure", d, "105", "X", "number", "input"],
        vec!["add-derived", d, "106", "Y", "Price * 2"],
        vec!["define", d, "107", "Z = Price * 3"],
        vec!["register-ext", d, "f", "1", "result = 1"],
        vec!["refresh-ext", d, "200"],
        vec!["refresh-all", d],
        vec!["serve-refresh", d, "--tick", "1"],
        vec!["scenario", d, "s", "100", "1", "--at", "Product=WidgetA"],
        vec!["set", d, "100", "1", "--at", "Product=WidgetA"],
        vec!["list", d],
        vec!["save-view", d, "1", "V", "102"],
        vec!["show", d, "100"],
        vec!["eval", d, "102"],
        vec!["export", d],
        vec!["stream", d, "102"],
        vec!["import-sql", d, &src, "400", "R", q, "r", "p:6:Region"],
        vec!["refresh-sql", d, &src, "400"],
        vec!["export-sql", d, &out, "100", "t", "v"],
        vec!["import-csv", d, &csv, "300", "S", "r", "p:6:Region"],
        vec!["export-csv", d, &out_csv, "100"],
    ];
    for args in &cases {
        fails_with(args, "serialization error");
    }
}

#[test]
fn every_mutating_command_reports_a_store_save_failure() {
    let (dir, db) = model("save-fail");
    let src = sales_source(&dir);
    let csv = file(&dir, "s.csv");
    std::fs::write(&csv, "p,r\nA,1\n").unwrap();
    let d = db.as_str();
    ok(&["register-ext", d, "dbl", "1", "result = args[0] * 2"]);
    ok(&["define", d, "200", "D = CALL(dbl, Price)"]);
    ok(&[
        "import-sql",
        d,
        &src,
        "400",
        "Rev",
        "SELECT p,r FROM sales",
        "r",
        "p:6:Region",
    ]);
    rusqlite::Connection::open(&src)
        .unwrap()
        .execute("UPDATE sales SET r = 9.0", [])
        .unwrap();
    let before = load(d);
    block_writes(d);

    let q = "SELECT p,r FROM sales";
    let cases: Vec<Vec<&str>> = vec![
        vec!["add-category", d, "3", "C"],
        vec!["add-item", d, "12", "1", "Q3"],
        vec!["add-measure", d, "105", "X", "number", "input"],
        vec!["add-derived", d, "106", "Y", "Price * 2"],
        vec!["define", d, "107", "Z = Price * 3"],
        vec!["register-ext", d, "f", "1", "result = 1"],
        vec!["refresh-ext", d, "200"],
        vec!["refresh-all", d],
        vec!["scenario", d, "s", "100", "1", "--at", "Product=WidgetA"],
        vec!["set", d, "100", "99", "--at", "Product=WidgetA"],
        vec!["save-view", d, "1", "V", "102"],
        vec!["import-sql", d, &src, "401", "R2", q, "r", "p:7:Area"],
        vec!["refresh-sql", d, &src, "400"],
        vec!["import-csv", d, &csv, "300", "S", "r", "p:8:Zone"],
    ];
    // refresh-ext / refresh-all run the Python body before saving; without a
    // usable sandbox they fail there (covered by the engine), not at the save.
    let sandbox = python_sandbox();
    for args in cases
        .iter()
        .filter(|a| sandbox || !a[0].starts_with("refresh-"))
    {
        fails_with(args, "mentat error");
    }
    let o = improv_stdin(&["stream", d, "Revenue"], b"Price 55 Product=WidgetA\n");
    assert!(!o.status.success());
    stdout_has(&String::from_utf8_lossy(&o.stderr), "mentat error");

    // Nothing was written by any of the failed commands.
    let after = load(d);
    assert_eq!(after.categories.len(), before.categories.len());
    assert_eq!(after.items.len(), before.items.len());
    assert_eq!(after.measures.len(), before.measures.len());
    assert_eq!(after.inputs, before.inputs);
    assert!(after.views.is_empty() && after.scenarios.is_empty());
}

/// `IMPROV_TEST_PG` (a URL or a libpq `key=value` DSN) as a `postgres://`
/// URL for a fresh model, or `None` (test skipped) when it is unset.
fn pg_model_url(tag: &str) -> Option<String> {
    let Ok(dsn) = std::env::var("IMPROV_TEST_PG") else {
        println!("skipped: IMPROV_TEST_PG not set");
        return None;
    };
    let base = if dsn.starts_with("postgres") {
        dsn
    } else {
        let kv: std::collections::HashMap<_, _> = dsn
            .split_whitespace()
            .filter_map(|p| p.split_once('='))
            .collect();
        let get = |k| kv.get(k).copied().unwrap_or_default();
        format!(
            "postgres://{}:{}@{}/{}",
            get("user"),
            get("password"),
            get("host"),
            get("dbname")
        )
    };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    Some(format!("{base}?model=cli_{tag}_{nonce}"))
}

/// The CLI builds and lists a model hosted in Postgres, through the same
/// subcommands it uses for a Mentat file.
#[test]
fn list_a_postgres_model() {
    let Some(url) = pg_model_url("list") else {
        return;
    };
    let d = url.as_str();
    ok(&["init", d]);
    ok(&["add-category", d, "1", "Time"]);
    ok(&["add-item", d, "10", "1", "Q1"]);
    ok(&["add-measure", d, "100", "Price", "number", "input", "Time"]);
    ok(&["set", d, "100", "4", "--at", "Time=Q1"]);
    ok(&["add-derived", d, "102", "Double", "Price * 2"]);
    ok(&["save-view", d, "1", "Prices", "Price", "--rows", "Time"]);
    let out = ok(&["list", d]);
    for want in [
        "  1 Time",
        "  10 Q1 (category 1)",
        "  100 Price [input] Number",
        "  102 Double [derived] Number",
        "  1 Prices (measure Price, 1 matrices)",
        "  100::{Time=Q1} = 4",
    ] {
        assert!(out.contains(want), "missing {want:?} in:\n{out}");
    }
    assert!(err(&["list", "postgres://u@localhost/d"]).contains("?model="));
}

/// `IMPROV_LOCALE=de`: a formula typed with German names, decimal comma and
/// `;` separators is stored canonically.
#[test]
fn german_formula_entry() {
    let dir = std::env::temp_dir().join(format!("improv-de-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("de.db");
    let db = db.to_str().unwrap();
    let run = |locale: &str, args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_improv"))
            .env("IMPROV_LOCALE", locale)
            .args(args)
            .output()
            .expect("run improv");
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    run("en", &["init", db]);
    run("en", &["add-category", db, "1", "Time"]);
    run("en", &["add-item", db, "10", "1", "2025"]);
    run(
        "en",
        &["add-measure", db, "100", "Price", "number", "input", "Time"],
    );
    run("en", &["set", db, "100", "2.675", "--at", "Time=2025"]);
    run(
        "de",
        &[
            "define",
            db,
            "101",
            "R = WENN(Price > 1,5; RUNDEN(Price; 2); 0)",
        ],
    );
    // Evaluated in English: the stored formula is canonical.
    let out = run("en", &["eval", db, "101"]);
    assert!(out.contains("R[Time=2025] = 2.68"), "{out}");
    // German names are not English names: the same text fails in `en`.
    let bad = Command::new(env!("CARGO_BIN_EXE_improv"))
        .env("IMPROV_LOCALE", "en")
        .args(["define", db, "102", "S = RUNDEN(Price; 2)"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    std::fs::remove_file(db).ok();
}
