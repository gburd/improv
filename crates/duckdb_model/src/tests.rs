use super::calc::{self, Cache};
use super::*;

fn col(name: &str, ty: &str) -> Column {
    Column {
        name: name.into(),
        ty: ty.into(),
    }
}

fn line_of(e: Error) -> usize {
    match e {
        Error::Line { line, .. } => line,
        other => panic!("expected a line error, got {other}"),
    }
}

const REVENUE: &str = "
# Time x Product revenue
category Time = year
category Product = product
input Price = price
input Quantity = qty sum
Revenue = Price * Quantity
Share = Revenue / SUM(Revenue OVER Product)
Growth = Revenue - PREV(Revenue, Time, 1, 0)
";

fn columns() -> Vec<Column> {
    vec![
        col("year", "INTEGER"),
        col("product", "VARCHAR"),
        col("price", "DECIMAL(10,2)"),
        col("qty", "BIGINT"),
    ]
}

#[test]
fn spec_lines() {
    let s = parse_spec(REVENUE).unwrap();
    assert_eq!(
        s.categories,
        [
            CategorySpec {
                name: "Time".into(),
                column: "year".into(),
                line: 3
            },
            CategorySpec {
                name: "Product".into(),
                column: "product".into(),
                line: 4
            },
        ]
    );
    assert_eq!(s.inputs[0].agg, None);
    assert_eq!(s.inputs[1].agg.as_deref(), Some("sum"));
    assert_eq!(
        s.derived[2].text,
        "Growth = Revenue - PREV(Revenue, Time, 1, 0)"
    );
    assert_eq!(s.derived[2].line, 9);
    // Keywords are case-insensitive; aggregations too; `--` comments.
    let s = parse_spec("-- c\nCATEGORY T = t\nInput X = x AVG\nY = X").unwrap();
    assert_eq!(s.inputs[0].agg.as_deref(), Some("avg"));
    // A measure may be named after a keyword: `input = ...` is a derived line.
    let s = parse_spec("category T = t\ninput = 1 + 1").unwrap();
    assert_eq!(s.derived[0].text, "input = 1 + 1");
}

#[test]
fn quoted_columns() {
    let s = parse_spec("category T = \"fiscal \"\"year\"\"\"\ninput V = \"the value\" max\nW = V")
        .unwrap();
    assert_eq!(s.categories[0].column, "fiscal \"year\"");
    assert_eq!(s.inputs[0].column, "the value");
    assert_eq!(s.inputs[0].agg.as_deref(), Some("max"));
    for bad in ["category T = \"open", "category T = \"\""] {
        let e = parse_spec(&format!("{bad}\nW = 1")).unwrap_err();
        assert!(e.to_string().contains("quoted column"), "{e}");
    }
}

#[test]
fn spec_errors_name_the_line() {
    let cases = [
        (
            "category T = t\nnonsense here\nW = 1",
            2,
            "expected `category",
        ),
        (
            "category T = t\ncategory 9x = u\nW = 1",
            2,
            "invalid category name",
        ),
        (
            "category T = t\ncategory Measure = v\nW = 1",
            2,
            "result column",
        ),
        (
            "category T = t\ncategory value_text = v\nW = 1",
            2,
            "result column",
        ),
        (
            "category T = t\ncategory t = u\nW = 1",
            2,
            "already declared on line 1",
        ),
        ("category T = t extra\nW = 1", 1, "unexpected \"extra\""),
        (
            "category T = t\ninput X = x median\nW = 1",
            2,
            "agg must be",
        ),
        (
            "category T = t\ninput X = x sum more\nW = 1",
            2,
            "unexpected",
        ),
        ("category T =\nW = 1", 1, "expected a column name"),
        ("category T = t\nW = 1\nW = 2", 3, "already declared"),
    ];
    for (text, line, want) in cases {
        let e = parse_spec(text).unwrap_err();
        assert!(e.to_string().contains(want), "{text:?}: {e}");
        assert!(
            e.to_string()
                .starts_with(&format!("improv: model line {line}:")),
            "{e}"
        );
    }
    let e = parse_spec("W = 1").unwrap_err();
    assert!(e.to_string().contains("no category"), "{e}");
    let e = parse_spec("category T = t\ninput X = x").unwrap_err();
    assert!(e.to_string().contains("no derived measure"), "{e}");
}

#[test]
fn binding() {
    let s = parse_spec(REVENUE).unwrap();
    let b = bind(&s, &columns()).unwrap();
    assert_eq!(b.dims, ["year", "product"]);
    assert_eq!(
        b.inputs,
        [
            ("price".to_string(), ValueType::Number),
            ("qty".to_string(), ValueType::Number)
        ]
    );
    // Case-insensitive: the source's spelling wins.
    let s = parse_spec("category T = YEAR\ninput F = Flag\nW = F").unwrap();
    let b = bind(&s, &[col("year", "INTEGER"), col("flag", "BOOLEAN")]).unwrap();
    assert_eq!(b.dims, ["year"]);
    assert_eq!(b.inputs, [("flag".to_string(), ValueType::Boolean)]);
    let s = parse_spec("category T = y\ninput N = name count\ninput S = name\nW = N").unwrap();
    let b = bind(&s, &[col("y", "DATE"), col("name", "VARCHAR")]).unwrap();
    assert_eq!(
        b.inputs,
        [
            ("name".to_string(), ValueType::Number),
            ("name".to_string(), ValueType::Text)
        ]
    );
}

#[test]
fn binding_errors() {
    let s = parse_spec(REVENUE).unwrap();
    let e = bind(&s, &columns()[1..]).unwrap_err();
    assert_eq!(line_of(e.clone()), 3);
    assert!(
        e.to_string()
            .contains("no column \"year\" (it has: product, price, qty)"),
        "{e}"
    );
    let s = parse_spec("category T = t\ninput X = x\nW = X").unwrap();
    let e = bind(&s, &[col("t", "INTEGER")]).unwrap_err();
    assert_eq!(line_of(e), 2);
    let e = bind(&s, &[col("t", "INTEGER"), col("x", "BLOB")]).unwrap_err();
    assert!(
        e.to_string().contains("model line 2: X:") && e.to_string().contains("BLOB"),
        "{e}"
    );
    let s = parse_spec("category T = t\ninput X = x sum\nW = X").unwrap();
    let e = bind(&s, &[col("t", "INTEGER"), col("x", "VARCHAR")]).unwrap_err();
    assert!(e.to_string().contains("needs a number column"), "{e}");
}

#[test]
fn sql() {
    assert_eq!(source_sql("sales"), "sales");
    assert_eq!(source_sql(" main.\"Sales\" ; "), "main.\"Sales\"");
    for q in [
        "SELECT * FROM t",
        "with x as (select 1) select * from x",
        "FROM t",
        "VALUES (1)",
    ] {
        assert_eq!(source_sql(q), format!("({q}) AS improv_source"));
    }
    assert_eq!(source_sql("(SELECT 1);"), "((SELECT 1)) AS improv_source");
    assert_eq!(describe_sql("t"), "DESCRIBE SELECT * FROM t");
    assert_eq!(
        items_sql("t", "year"),
        "SELECT CAST(\"year\" AS VARCHAR) FROM t WHERE \"year\" IS NOT NULL \
         GROUP BY \"year\" ORDER BY \"year\""
    );
    let s = parse_spec(REVENUE).unwrap();
    let b = bind(&s, &columns()).unwrap();
    assert_eq!(
        input_sql("t", &s, &b, 0).unwrap(),
        "SELECT \"year\"::text, \"product\"::text, \"price\"::float8 FROM t"
    );
    assert_eq!(
        input_sql("t", &s, &b, 1).unwrap(),
        "SELECT \"year\"::text, \"product\"::text, sum(\"qty\")::float8 FROM t GROUP BY 1, 2"
    );
    // An enum input has no SQL reading; reported on its line.
    let mut b = b;
    b.inputs[0].1 = ValueType::Enum;
    assert_eq!(line_of(input_sql("t", &s, &b, 0).unwrap_err()), 5);
}

/// The canonical Time x Product oracle (values checked by hand).
fn oracle() -> Vec<Row> {
    let s = parse_spec(REVENUE).unwrap();
    let b = bind(&s, &columns()).unwrap();
    let items = vec![
        vec!["2024".to_string(), "2025".to_string()],
        vec!["WidgetA".to_string(), "WidgetB".to_string()],
    ];
    let row = |t: &str, p: &str, v: f64| {
        (
            vec![Some(t.to_string()), Some(p.to_string())],
            Some(CellValue::num(v)),
        )
    };
    let price = vec![
        row("2024", "WidgetA", 10.0),
        row("2024", "WidgetB", 20.0),
        row("2025", "WidgetA", 11.0),
        row("2025", "WidgetB", 20.0),
    ];
    let qty = vec![
        row("2024", "WidgetA", 100.0),
        row("2024", "WidgetB", 50.0),
        row("2025", "WidgetA", 120.0),
        row("2025", "WidgetB", 40.0),
    ];
    run(&s, &b, &items, vec![price, qty]).unwrap()
}

#[test]
fn time_by_product_oracle() {
    let rows = oracle();
    let got: Vec<(String, Vec<Option<String>>, String)> = rows
        .iter()
        .map(|r| (r.measure.clone(), r.dims.clone(), r.text.clone()))
        .collect();
    let d = |t: &str, p: &str| vec![Some(t.to_string()), Some(p.to_string())];
    let want = vec![
        ("Revenue", d("2024", "WidgetA"), "1000"),
        ("Revenue", d("2024", "WidgetB"), "1000"),
        ("Revenue", d("2025", "WidgetA"), "1320"),
        ("Revenue", d("2025", "WidgetB"), "800"),
        ("Share", d("2024", "WidgetA"), "0.5"),
        ("Share", d("2024", "WidgetB"), "0.5"),
        ("Share", d("2025", "WidgetA"), "0.622641509433962"),
        ("Share", d("2025", "WidgetB"), "0.377358490566038"),
        ("Growth", d("2024", "WidgetA"), "1000"),
        ("Growth", d("2024", "WidgetB"), "1000"),
        ("Growth", d("2025", "WidgetA"), "320"),
        ("Growth", d("2025", "WidgetB"), "-200"),
    ];
    let want: Vec<_> = want
        .into_iter()
        .map(|(m, d, v)| (m.to_string(), d, v.to_string()))
        .collect();
    assert_eq!(got, want);
    assert_eq!(rows[6].value, Some(1320.0 / 2120.0));
    // Deterministic run to run.
    assert_eq!(oracle(), rows);
}

#[test]
fn measures_over_fewer_categories_and_text_values() {
    let s = parse_spec(
        "category Time = t\ncategory Product = p\ninput Q = q\ninput Name = n\n\
         Total = SUM(Q OVER Product)\nLabel = Name & \"!\"\nBad = Q / 0",
    )
    .unwrap();
    let b = bind(
        &s,
        &[
            col("t", "INTEGER"),
            col("p", "VARCHAR"),
            col("q", "DOUBLE"),
            col("n", "VARCHAR"),
        ],
    )
    .unwrap();
    let items = vec![
        vec!["1".to_string()],
        vec!["a".to_string(), "b".to_string()],
    ];
    let dims = |p: &str| vec![Some("1".to_string()), Some(p.to_string())];
    let q = vec![
        (dims("a"), Some(CellValue::num(2.0))),
        (dims("b"), Some(CellValue::num(3.0))),
        // NULL dimension: skipped; NULL value: no cell.
        (vec![None, Some("a".into())], Some(CellValue::num(9.0))),
    ];
    let n = vec![
        (dims("a"), Some(CellValue::Text("x".into()))),
        (dims("b"), None),
    ];
    let rows = run(&s, &b, &items, vec![q, n]).unwrap();
    let total: Vec<_> = rows.iter().filter(|r| r.measure == "Total").collect();
    assert_eq!(total.len(), 1);
    assert_eq!(total[0].dims, [Some("1".to_string()), None]);
    assert_eq!(total[0].value, Some(5.0));
    let label: Vec<_> = rows.iter().filter(|r| r.measure == "Label").collect();
    assert_eq!(label.len(), 1);
    assert_eq!((label[0].value, label[0].text.as_str()), (None, "x!"));
    let bad: Vec<_> = rows.iter().filter(|r| r.measure == "Bad").collect();
    assert_eq!(bad.len(), 2);
    assert_eq!((bad[0].value, bad[0].text.as_str()), (None, "#DIV/0!"));
}

#[test]
fn run_errors() {
    let s = parse_spec("category T = t\ninput X = x\nW = X + Nope").unwrap();
    let b = bind(&s, &[col("t", "INTEGER"), col("x", "DOUBLE")]).unwrap();
    let items = vec![vec!["1".to_string()]];
    let e = run(&s, &b, &items, vec![vec![]]).unwrap_err();
    assert_eq!(line_of(e.clone()), 3);
    assert!(e.to_string().contains("unknown measure: Nope"), "{e}");
    // Two rows on one coordinate without an aggregation.
    let s = parse_spec("category T = t\ninput X = x\nW = X").unwrap();
    let one = || (vec![Some("1".to_string())], Some(CellValue::num(1.0)));
    let e = run(&s, &b, &items, vec![vec![one(), one()]]).unwrap_err();
    assert_eq!(line_of(e.clone()), 2);
    assert!(
        e.to_string().contains("coordinate (1); add an aggregation"),
        "{e}"
    );
    // A row of the wrong width is the caller's bug, reported as a model error.
    let e = run(&s, &b, &items, vec![vec![(vec![], None)]]).unwrap_err();
    assert!(matches!(e, Error::Model(_)), "{e}");
    // A dimensionless result is refused by `define`.
    let s = parse_spec("category T = t\ninput X = x\nW = 1").unwrap();
    let e = run(&s, &b, &items, vec![vec![]]).unwrap_err();
    assert!(e.to_string().contains("no dimensions"), "{e}");
    // An enum-typed input is refused when the model is built.
    let mut b2 = b.clone();
    b2.inputs[0].1 = ValueType::Enum;
    let s = parse_spec("category T = t\ninput X = x\nW = X").unwrap();
    assert!(run(&s, &b2, &items, vec![vec![]]).is_err());
}

#[test]
fn recurrence_along_a_category() {
    // A running total: PREV of itself, defaulting to 0 before the first item.
    let s = parse_spec("category T = t\ninput X = x\nW = PREV(W, T, 1, 0) + X").unwrap();
    let b = bind(&s, &[col("t", "INTEGER"), col("x", "DOUBLE")]).unwrap();
    let items = vec![vec!["1".to_string(), "2".to_string(), "3".to_string()]];
    let x = |t: &str, v: f64| (vec![Some(t.to_string())], Some(CellValue::num(v)));
    let rows = run(
        &s,
        &b,
        &items,
        vec![vec![x("1", 1.0), x("2", 2.0), x("3", 4.0)]],
    )
    .unwrap();
    let v: Vec<_> = rows.iter().map(|r| r.value).collect();
    assert_eq!(v, [Some(1.0), Some(3.0), Some(7.0)]);
}

#[test]
fn functions_registry() {
    let all = function_rows(None);
    assert_eq!(all.len(), functions::all().count());
    let pmt = all.iter().find(|f| f.name == "PMT").unwrap();
    assert_eq!(pmt.category, "financial");
    assert_eq!(pmt.returns, "number");
    assert!(
        pmt.signature.starts_with("PMT(rate, nper, pv"),
        "{}",
        pmt.signature
    );
    assert!(all.iter().any(|f| f.aliases.is_some()));
    for r in &all {
        assert!(
            !r.description.is_empty() && !r.category.is_empty(),
            "{}",
            r.name
        );
    }
    let hits = function_rows(Some("pmt"));
    assert_eq!(hits[0].name, "PMT");
    assert!(function_rows(Some("zzzz-no-such")).is_empty());
    // Every category and return spelling is reachable.
    let cats: std::collections::BTreeSet<_> = all.iter().map(|f| f.category).collect();
    assert_eq!(cats.len(), 11, "{cats:?}");
    let rets: std::collections::BTreeSet<_> = all.iter().map(|f| f.returns).collect();
    assert_eq!(rets.len(), 5, "{rets:?}");
}

fn text(c: &Cache, f: &str, inputs: Option<&str>) -> Option<String> {
    c.calc(f, inputs).unwrap().map(|v| v.to_string())
}

#[test]
fn calc_values() {
    let c = Cache::default();
    assert_eq!(text(&c, "1 + 2 * 3", None).as_deref(), Some("7"));
    assert_eq!(
        text(&c, "=ROUND(2.675, 2)", Some("")).as_deref(),
        Some("2.68")
    );
    let pmt = c
        .calc(
            "ROUND(PMT(rate/12, n, -pv), 2)",
            Some(r#"{"rate": 0.06, "n": 360, "pv": 200000}"#),
        )
        .unwrap()
        .unwrap();
    assert_eq!(calc::number(&pmt), Some(1199.1));
    assert_eq!(
        text(&c, r#"UPPER(name) & "!""#, Some(r#"{"name": "ada"}"#)).as_deref(),
        Some("ADA!")
    );
    assert_eq!(
        text(&c, "IF(flag, 1, 2)", Some(r#"{"flag": false}"#)).as_deref(),
        Some("2")
    );
    assert_eq!(text(&c, "1/0", None).as_deref(), Some("#DIV/0!"));
    // A missing input: NULL out where the result depends on it.
    assert_eq!(text(&c, "x + 1", Some(r#"{"x": null}"#)), None);
    assert_eq!(
        text(&c, "IF(TRUE, 5, x)", Some(r#"{"x": null}"#)).as_deref(),
        Some("5")
    );
}

#[test]
fn calc_errors() {
    let c = Cache::default();
    for (f, inputs, want) in [
        ("1 +", None, "improv_calc: parse error"),
        ("y + 1", Some(r#"{"x": 1}"#), "unknown"),
        ("SUM(x OVER T)", Some(r#"{"x": 1}"#), "improv_calc"),
        ("1", Some("{nope"), "not valid JSON"),
        ("1", Some("[1, 2]"), "must be a JSON object"),
        ("1", Some(r#"{"x": [1]}"#), "array or object"),
        ("LEN(1, 2, 3)", None, "improv_calc"),
    ] {
        let e = c.calc(f, inputs).unwrap_err();
        assert!(e.contains(want), "{f}: {e}");
    }
    assert!(c.is_empty(), "compile errors are not cached");
}

#[test]
fn calc_cache() {
    let c = Cache::new(2);
    let x = |v: i32| format!(r#"{{"x": {v}}}"#);
    assert_eq!(text(&c, "x * 2", Some(&x(1))).as_deref(), Some("2"));
    assert_eq!(text(&c, "x * 2", Some(&x(5))).as_deref(), Some("10"));
    assert_eq!(c.len(), 1, "same formula and input shape: one entry");
    // Same name, other type: a separate compile.
    assert_eq!(
        text(&c, "x & \"\"", Some(r#"{"x": "a"}"#)).as_deref(),
        Some("a")
    );
    assert_eq!(c.len(), 2);
    // Full: cleared, then the new one is cached.
    assert_eq!(text(&c, "x + 1", Some(&x(1))).as_deref(), Some("2"));
    assert_eq!(c.len(), 1);
}

#[test]
fn calc_numbers() {
    assert_eq!(calc::number(&CellValue::num(1.5)), Some(1.5));
    assert_eq!(calc::number(&CellValue::Bool(true)), Some(1.0));
    assert_eq!(calc::number(&CellValue::Date(0)), Some(25_569.0));
    assert_eq!(calc::number(&CellValue::Text("a".into())), None);
    assert_eq!(calc::number(&CellValue::Err(1)), None);
    // A date-typed input is declared as a date.
    let inputs = vec![("d".to_string(), Some(CellValue::Date(86_400_000)))];
    let c = calc::compile("YEAR(d)", &inputs).unwrap();
    assert_eq!(c.eval(&inputs).unwrap(), Some(CellValue::num(1970.0)));
}

#[test]
fn calc_threads() {
    let c = std::sync::Arc::new(Cache::default());
    let handles: Vec<_> = (0..8)
        .map(|t| {
            let c = c.clone();
            std::thread::spawn(move || {
                for i in 0..200 {
                    let v = c
                        .calc("x * 2 + t", Some(&format!(r#"{{"x": {i}, "t": {t}}}"#)))
                        .unwrap()
                        .unwrap();
                    assert_eq!(calc::number(&v), Some(f64::from(i * 2 + t)));
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(c.len(), 1);
}
