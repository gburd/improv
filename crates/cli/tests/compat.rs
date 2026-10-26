//! The CLI's argument syntax is a compatibility surface. Every invocation
//! here comes from `crates/cli/tests`, the man page EXAMPLES, `docs/src` or a
//! smoke script, and its stdout was recorded from the hand-written parser
//! (before the clap move). It must keep working, byte for byte. The failures
//! keep their message; a usage error (clap's) exits 2 where the hand parser
//! exited 1.
//!
//! `{d}` is a scratch directory, `{v}` the crate version.

use std::io::Write;
use std::process::{Command, Stdio};

/// (environment, arguments, stdin, expected stdout; `None` = success only).
type Case = (
    &'static [(&'static str, &'static str)],
    &'static [&'static str],
    Option<&'static str>,
    Option<&'static str>,
);

const EN: &[(&str, &str)] = &[("IMPROV_LOCALE", "en")];

const CASES: &[Case] = &[
    (EN, &[], None, None),
    (EN, &["help"], None, None),
    (EN, &["--help"], None, None),
    (EN, &["-h"], None, None),
    (EN, &["--version"], None, Some("improv {v}\n")),
    (EN, &["-V"], None, Some("improv {v}\n")),
    // man improv EXAMPLES; docs/src/getting-started.md; compare_cli.sh.
    (EN, &["init", "{d}/m.db"], None, Some("initialized store at '{d}/m.db'\n")),
    (EN, &["init", "{d}/m.db"], None, Some("initialized store at '{d}/m.db'\n")),
    (EN, &["add-category", "{d}/m.db", "1", "Time"], None, Some("added category 1 'Time'\n")),
    (EN, &["add-category", "{d}/m.db", "2", "Product"], None, Some("added category 2 'Product'\n")),
    (EN, &["add-item", "{d}/m.db", "10", "1", "2025"], None, Some("added item 10 '2025' in category 1\n")),
    (EN, &["add-item", "{d}/m.db", "11", "1", "2026"], None, Some("added item 11 '2026' in category 1\n")),
    (EN, &["add-item", "{d}/m.db", "20", "2", "WidgetA"], None, Some("added item 20 'WidgetA' in category 2\n")),
    (EN, &["add-item", "{d}/m.db", "21", "2", "WidgetB"], None, Some("added item 21 'WidgetB' in category 2\n")),
    (EN, &["add-measure", "{d}/m.db", "100", "Price", "number", "input", "Product"], None, Some("added input measure 100 'Price'\n")),
    (EN, &["add-measure", "{d}/m.db", "101", "Quantity", "number", "input", "Time", "Product"], None, Some("added input measure 101 'Quantity'\n")),
    (EN, &["add-measure", "{d}/m.db", "103", "Flag", "boolean", "input"], None, Some("added input measure 103 'Flag'\n")),
    (EN, &["add-measure", "{d}/m.db", "104", "Note", "text", "input"], None, Some("added input measure 104 'Note'\n")),
    (EN, &["set", "{d}/m.db", "100", "10", "--at", "Product=WidgetA"], None, Some("set measure 100\n")),
    (EN, &["set", "{d}/m.db", "100", "20", "--at", "Product=WidgetB"], None, Some("set measure 100\n")),
    (EN, &["set", "{d}/m.db", "101", "100", "--at", "Time=2025,Product=WidgetA"], None, Some("set measure 101\n")),
    (EN, &["set", "{d}/m.db", "101", "50", "--at", "Time=2025", "Product=WidgetB"], None, Some("set measure 101\n")),
    (EN, &["set", "{d}/m.db", "101", "120", "--at", "Time=2026", "--at", "Product=WidgetA"], None, Some("set measure 101\n")),
    (EN, &["set", "{d}/m.db", "101", "-80", "--at", "Time=2026,Product=WidgetB"], None, Some("set measure 101\n")),
    (EN, &["set", "{d}/m.db", "101", "80", "--at", "Time=2026,Product=WidgetB"], None, Some("set measure 101\n")),
    (EN, &["set", "{d}/m.db", "103", "true"], None, Some("set measure 103\n")),
    (EN, &["set", "{d}/m.db", "104", "hello world"], None, Some("set measure 104\n")),
    // H1-25: a measure by name.
    (EN, &["set", "{d}/m.db", "Price", "10", "--at", "Product=WidgetA"], None, Some("set measure 100\n")),
    (EN, &["list", "{d}/m.db"], None, Some("categories:\n  1 Time\n  2 Product\nitems:\n  10 2025 (category 1)\n  11 2026 (category 1)\n  20 WidgetA (category 2)\n  21 WidgetB (category 2)\nmeasures:\n  100 Price [input] Number\n  101 Quantity [input] Number\n  103 Flag [input] Boolean\n  104 Note [input] Text\nviews:\ninputs:\n  100::{Product=WidgetA} = 10\n  100::{Product=WidgetB} = 20\n  101::{Time=2025,Product=WidgetA} = 100\n  101::{Time=2025,Product=WidgetB} = 50\n  101::{Time=2026,Product=WidgetA} = 120\n  101::{Time=2026,Product=WidgetB} = 80\n  103::{} = true\n  104::{} = \"hello world\"\n")),
    (EN, &["save-view", "{d}/m.db", "1", "Quantity by year", "Quantity", "--rows", "Time", "--cols", "Product"], None, Some("saved view 1 'Quantity by year'\n")),
    (EN, &["save-view", "{d}/m.db", "2", "Prices", "100", "--rows", "Product"], None, Some("saved view 2 'Prices'\n")),
    (EN, &["save-view", "{d}/m.db", "3", "Q", "101"], None, Some("saved view 3 'Q'\n")),
    (EN, &["show", "{d}/m.db", "100"], None, Some("measure 100 Price [input] Number\n  cells:\n    100::{Product=WidgetA} = 10\n    100::{Product=WidgetB} = 20\n")),
    (EN, &["show", "{d}/m.db", "101"], None, Some("measure 101 Quantity [input] Number\n  cells:\n    101::{Time=2025,Product=WidgetA} = 100\n    101::{Time=2025,Product=WidgetB} = 50\n    101::{Time=2026,Product=WidgetA} = 120\n    101::{Time=2026,Product=WidgetB} = 80\n")),
    (EN, &["export", "{d}/m.db"], None, None),
    (EN, &["add-derived", "{d}/m.db", "102", "Revenue", "Price * Quantity"], None, Some("added derived measure 102 'Revenue'\n")),
    (EN, &["eval", "{d}/m.db", "102"], None, Some("eval 102 'Revenue' (4 cells):\n  Revenue[Time=2025, Product=WidgetA] = 1000\n  Revenue[Time=2025, Product=WidgetB] = 1000\n  Revenue[Time=2026, Product=WidgetA] = 1200\n  Revenue[Time=2026, Product=WidgetB] = 1600\n")),
    (EN, &["add-derived", "{d}/m.db", "105", "RevByProduct", "SUM(Revenue OVER Time)"], None, Some("added derived measure 105 'RevByProduct'\n")),
    (EN, &["eval", "{d}/m.db", "105"], None, Some("eval 105 'RevByProduct' (2 cells):\n  RevByProduct[Product=WidgetA] = 2200\n  RevByProduct[Product=WidgetB] = 2600\n")),
    (EN, &["add-derived", "{d}/m.db", "106", "Neg", "-Price"], None, Some("added derived measure 106 'Neg'\n")),
    (EN, &["eval", "{d}/m.db", "106"], None, Some("eval 106 'Neg' (2 cells):\n  Neg[Product=WidgetA] = -10\n  Neg[Product=WidgetB] = -20\n")),
    (EN, &["define", "{d}/m.db", "107", "Share = Revenue / SUM(Revenue OVER Product)"], None, Some("defined derived measure 107 'Share'\n")),
    (EN, &["define", "{d}/m.db", "108", "Growth = Revenue - PREV(Revenue, Time, 1, 0)"], None, Some("defined derived measure 108 'Growth'\n")),
    (EN, &["eval", "{d}/m.db", "108"], None, Some("eval 108 'Growth' (4 cells):\n  Growth[Time=2025, Product=WidgetA] = 1000\n  Growth[Time=2025, Product=WidgetB] = 1000\n  Growth[Time=2026, Product=WidgetA] = 200\n  Growth[Time=2026, Product=WidgetB] = 600\n")),
    (EN, &["define", "{d}/m.db", "1", "Margin = LAMBDA(price, cost, (price - cost) / price)"], None, Some("defined LAMBDA 1 'Margin'(price, cost)\n")),
    (EN, &["define", "{d}/m.db", "120", "Pct = Margin(Price, 5) * 100"], None, Some("defined derived measure 120 'Pct'\n")),
    (EN, &["eval", "{d}/m.db", "120"], None, Some("eval 120 'Pct' (2 cells):\n  Pct[Product=WidgetA] = 50\n  Pct[Product=WidgetB] = 75\n")),
    // IMPROV_LOCALE: German names, decimal comma, `;` separators.
    (&[("IMPROV_LOCALE", "de")], &["define", "{d}/m.db", "121", "Cap = WENN(Price > 1,5; RUNDEN(Price; 2); 0)"], None, Some("defined derived measure 121 'Cap'\n")),
    (&[("IMPROV_LOCALE", "de")], &["add-derived", "{d}/m.db", "122", "Half", "Price * 0,5"], None, Some("added derived measure 122 'Half'\n")),
    (EN, &["eval", "{d}/m.db", "122"], None, Some("eval 122 'Half' (2 cells):\n  Half[Product=WidgetA] = 5\n  Half[Product=WidgetB] = 10\n")),
    (EN, &["scenario", "{d}/m.db", "high", "100", "15", "--at", "Product=WidgetA"], None, Some("scenario 'high': override on measure 100 recorded\n")),
    (EN, &["scenario", "{d}/m.db", "high", "101", "1", "--at", "Time=2025", "Product=WidgetA"], None, Some("scenario 'high': override on measure 101 recorded\n")),
    (EN, &["eval", "{d}/m.db", "102", "--scenario", "high"], None, Some("eval 102 'Revenue' (4 cells):\n  Revenue[Time=2025, Product=WidgetA] = 15\n  Revenue[Time=2025, Product=WidgetB] = 1000\n  Revenue[Time=2026, Product=WidgetA] = 1800\n  Revenue[Time=2026, Product=WidgetB] = 1600\n")),
    (EN, &["stream", "{d}/m.db", "Revenue"], Some("Price 15 Product=WidgetA\n"), Some("Revenue Time=2025, Product=WidgetA = 1500\nRevenue Time=2026, Product=WidgetA = 1800\n")),
    (EN, &["stream", "{d}/m.db", "102", "Share"], Some("101 60 Time=2025,Product=WidgetB\n"), Some("Revenue Time=2025, Product=WidgetB = 1200\nShare Time=2025, Product=WidgetA = 0.555555555555556\nShare Time=2025, Product=WidgetB = 0.444444444444444\n")),
    (EN, &["register-ext", "{d}/m.db", "hypot", "2", "result = (args[0]**2 + args[1]**2) ** 0.5"], None, Some("registered external function 'hypot' (arity 2)\n")),
    (EN, &["define", "{d}/m.db", "200", "H = CALL(hypot, Price, Quantity)"], None, Some("defined external-call measure 200 'H' = hypot(...); run 'refresh-ext' to populate\n")),
    (EN, &["define", "{d}/m.db", "201", "H2 = CALL(hypot, Price, Quantity)", "--refresh", "interval:60"], None, Some("defined external-call measure 201 'H2' = hypot(...); run 'refresh-ext' to populate\n")),
    (EN, &["define", "{d}/m.db", "202", "H3 = CALL(hypot, Price, Quantity)", "--refresh", "on-load"], None, Some("defined external-call measure 202 'H3' = hypot(...); run 'refresh-ext' to populate\n")),
    (EN, &["export-csv", "{d}/m.db", "{d}/q.csv", "101"], None, Some("exported 4 rows from measure 101 to {d}/q.csv\n")),
    (EN, &["export-csv", "{d}/m.db", "{d}/q.tsv", "101"], None, Some("exported 4 rows from measure 101 to {d}/q.tsv\n")),
    (EN, &["export-csv", "{d}/m.db", "{d}/q.txt", "101", "--tsv"], None, Some("exported 4 rows from measure 101 to {d}/q.txt\n")),
    (EN, &["export-sql", "{d}/m.db", "{d}/out.sqlite", "101", "qty", "v"], None, Some("exported 4 cells from measure 101 to table 'qty'\n")),
    // man improv: import-sql / import-csv / refresh-sql / refresh-all.
    (EN, &["import-sql", "{d}/s.db", "{d}/sales.db", "100", "Revenue", "SELECT t,p,r FROM sales", "r", "t:1:Time", "p:2:Product"], None, Some("imported 3 cells into SQL-backed measure 100 'Revenue' (refresh with refresh-sql)\n")),
    (EN, &["import-sql", "{d}/s.db", "{d}/sales.db", "101", "Rev2", "SELECT t,p,r FROM sales", "r", "t:1:Time", "p:2:Product", "--refresh", "interval:30"], None, Some("imported 3 cells into SQL-backed measure 101 'Rev2' (refresh with refresh-sql)\n")),
    (EN, &["refresh-sql", "{d}/s.db", "{d}/sales.db", "100"], None, Some("refreshed measure 100 from SQL: 3 cells\n")),
    (EN, &["refresh-all", "{d}/s.db", "{d}/sales.db"], None, Some("refreshed SQL measure 100 (3 cells) [Some(Manual)]\nrefreshed SQL measure 101 (3 cells) [Some(Interval { secs: 30 })]\nrefresh-all: 6 cells refreshed\n")),
    (EN, &["refresh-all", "{d}/s.db"], None, Some("refresh-all: 0 cells refreshed\n")),
    (EN, &["show", "{d}/s.db", "100"], None, Some("measure 100 Revenue [input] Number\n  description: imported from SQL: SELECT t,p,r FROM sales\n  cells:\n    100::{Time=2025,Product=A} = 10\n    100::{Time=2025,Product=B} = 20\n    100::{Time=2026,Product=A} = 30\n")),
    (EN, &["import-csv", "{d}/c.db", "{d}/sales.csv", "100", "Revenue", "r", "t:1:Time", "p:2:Product"], None, Some("imported 2 cells into measure 100 'Revenue'\n")),
    (EN, &["import-csv", "{d}/c.db", "{d}/sales.tsv", "101", "R2", "r", "t:1:Time", "p:2:Product"], None, Some("imported 1 cells into measure 101 'R2'\n")),
    (EN, &["import-csv", "{d}/c.db", "{d}/raw.csv", "102", "R3", "2", "0:1:Time", "1:2:Product", "--no-header"], None, Some("imported 1 cells into measure 102 'R3'\n")),
    (EN, &["import-csv", "{d}/c.db", "{d}/raw.txt", "103", "R4", "2", "0:1:Time", "--tsv", "--no-header", "1:2:Product"], None, Some("imported 1 cells into measure 103 'R4'\n")),
    (EN, &["list", "{d}/c.db"], None, Some("categories:\n  1 Time\n  2 Product\nitems:\n  1000000 2025 (category 1)\n  1000001 A (category 2)\n  1000002 2026 (category 1)\n  1000003 B (category 2)\nmeasures:\n  100 Revenue [input] Number\n  101 R2 [input] Number\n  102 R3 [input] Number\n  103 R4 [input] Number\nviews:\ninputs:\n  100::{Time=2025,Product=A} = 10\n  100::{Time=2026,Product=B} = 20\n  101::{Time=2025,Product=A} = 1\n  102::{Time=2025,Product=A} = 5\n  103::{Time=2026,Product=B} = 6\n")),
];

/// Invocations that fail: the exit code (2 for a usage error, 1 for one
/// the command reports) and the text of the error.
const FAILS: &[(&[&str], i32, &str)] = &[
    (&["init"], 2, "missing argument <db>"),
    (
        &["add-category", "{d}/m.db", "2"],
        2,
        "missing argument <name>",
    ),
    (
        &["add-category", "{d}/m.db", "x", "T"],
        2,
        "<id> must be a number, got 'x'",
    ),
    (
        &["add-item", "{d}/m.db", "11", "y", "Q2"],
        2,
        "<category-id> must be a number",
    ),
    (&["set", "{d}/m.db", "100"], 2, "missing argument <value>"),
    (
        &["set", "{d}/m.db", "100", "1", "Product=WidgetA"],
        1,
        "did you mean '--at Product=WidgetA'?",
    ),
    (
        &["set", "{d}/m.db", "100", "1", "--at", "Product=WidgetA,Bad"],
        1,
        "--at pair must be Cat=Item, got 'Bad'",
    ),
    (
        &[
            "set",
            "{d}/m.db",
            "101",
            "1",
            "--at",
            "Time=2025",
            "--at",
            "Time=2026",
        ],
        1,
        "'Time' more than once",
    ),
    (
        &["show", "{d}/m.db", "x"],
        1,
        "no measure named 'x'; the measures are",
    ),
    (&["eval", "{d}/m.db"], 2, "missing argument <measure>"),
    (
        &["eval", "{d}/m.db", "102", "--scenario", "nope"],
        1,
        "no scenario named 'nope'",
    ),
    (
        &["register-ext", "{d}/m.db", "f", "x", "result = 1"],
        2,
        "<arity> must be a number",
    ),
    (
        &[
            "import-sql",
            "{d}/s.db",
            "{d}/sales.db",
            "109",
            "X",
            "SELECT 1",
            "r",
            "t:1",
        ],
        2,
        "dimension spec 't:1' must be <col>:<cat-id>:<cat-name>",
    ),
    (
        &["import-csv", "{d}/c.db", "{d}/sales.csv", "109", "X", "r"],
        2,
        "at least one dimension mapping is required",
    ),
    (
        &["stream", "{d}/m.db"],
        2,
        "stream needs at least one target measure",
    ),
];

fn scratch() -> String {
    let dir = std::env::temp_dir().join(format!("improv-compat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let conn = rusqlite::Connection::open(dir.join("sales.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE sales(t TEXT, p TEXT, r REAL);
         INSERT INTO sales VALUES ('2025','A',10),('2025','B',20),('2026','A',30);",
    )
    .unwrap();
    std::fs::write(dir.join("sales.csv"), "t,p,r\n2025,A,10\n2026,B,20\n").unwrap();
    std::fs::write(dir.join("sales.tsv"), "t\tp\tr\n2025\tA\t1\n").unwrap();
    std::fs::write(dir.join("raw.csv"), "2025,A,5\n").unwrap();
    std::fs::write(dir.join("raw.txt"), "2026\tB\t6\n").unwrap();
    dir.to_string_lossy().into_owned()
}

fn run(env: &[(&str, &str)], args: &[String], stdin: Option<&str>) -> std::process::Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_improv"))
        .envs(env.iter().copied())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn improv");
    let mut input = child.stdin.take().unwrap();
    input.write_all(stdin.unwrap_or("").as_bytes()).unwrap();
    drop(input);
    child.wait_with_output().expect("wait improv")
}

#[test]
fn todays_invocations_keep_working() {
    let d = scratch();
    let fill = |s: &str| {
        s.replace("{d}", &d)
            .replace("{v}", env!("CARGO_PKG_VERSION"))
    };
    for (env, args, stdin, want) in CASES {
        let args: Vec<String> = args.iter().map(|a| fill(a)).collect();
        let o = run(env, &args, *stdin);
        let out = String::from_utf8_lossy(&o.stdout);
        assert_eq!(
            o.status.code(),
            Some(0),
            "improv {args:?} failed: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        if let Some(want) = want {
            assert_eq!(out, fill(want), "improv {args:?}");
        }
    }
    for (args, code, needle) in FAILS {
        let args: Vec<String> = args.iter().map(|a| fill(a)).collect();
        let o = run(EN, &args, None);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(o.status.code(), Some(*code), "improv {args:?}: {err}");
        assert!(
            err.contains(needle),
            "improv {args:?}: want '{needle}' in: {err}"
        );
    }
    let _ = std::fs::remove_dir_all(&d);
}
