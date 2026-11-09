#![cfg_attr(coverage_nightly, coverage(off))]

use super::*;
use improv_core_model::{
    parse_expr, Filter, NamedLambda, NumberFormat, RefreshPolicy, View, ViewId,
};
use proptest::prelude::*;

const T: CategoryId = CategoryId(1);
const P: CategoryId = CategoryId(2);

/// A fresh directory for this test's files.
fn dir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("improv-duck-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn target(d: &Path, model: &str) -> String {
    format!("duckdb:{}?model={model}", d.join("m.duckdb").display())
}

fn measure(id: u32, name: &str, cats: &[CategoryId], kind: MeasureKind) -> Measure {
    Measure {
        id: MeasureId(id),
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: cats.to_vec(),
        kind,
        description: None,
        format: None,
    }
}

fn at(t: u32, p: u32) -> Coordinate {
    Coordinate::from_pairs([(T, ItemId(t)), (P, ItemId(p))])
}

/// Every part of a model a store keeps: categories (P before T, so a
/// measure's order is not the ids'), items, every value type, a derived
/// measure and a total, a number format, a description, views, a LAMBDA,
/// an external function and call, a scenario and a SQL source.
fn sample() -> Model {
    let mut m = Model::new();
    m.add_category(T, "Time");
    m.add_category(P, "Product");
    for (i, q) in ["Q1", "Q2", "Q3"].iter().enumerate() {
        m.add_item(ItemId(10 + i as u32), T, *q);
    }
    m.add_item(ItemId(20), P, "Widget");
    m.add_item(ItemId(21), P, "Gadget");
    let mut price = measure(100, "Price", &[P, T], MeasureKind::Input);
    price.description = Some("Unit price".into());
    price.format = Some(NumberFormat {
        decimals: Some(2),
        grouping: true,
    });
    m.add_measure(price);
    m.add_measure(measure(101, "Quantity", &[T, P], MeasureKind::Input));
    let rev = parse_expr(&m, "Price * Quantity").unwrap();
    m.add_measure(measure(102, "Revenue", &[T, P], MeasureKind::Derived(rev)));
    let total = parse_expr(&m, "SUM(Revenue OVER Time)").unwrap();
    m.add_measure(measure(103, "Total", &[P], MeasureKind::Derived(total)));
    for (id, vt) in [
        (104, ValueType::Boolean),
        (105, ValueType::Text),
        (106, ValueType::DateTime),
        (107, ValueType::Enum),
    ] {
        let mut x = measure(id, &format!("M{id}"), &[P], MeasureKind::Input);
        x.value_type = vt;
        m.add_measure(x);
    }
    for t in 10..13 {
        for p in 20..22 {
            m.set_input(
                MeasureId(100),
                at(t, p),
                Value::Number(f64::from(t + p) + 0.25),
            );
            m.set_input(MeasureId(101), at(t, p), Value::Number(f64::from(t * p)));
        }
    }
    let w = Coordinate::from_pairs([(P, ItemId(20))]);
    m.set_input(MeasureId(104), w.clone(), Value::Boolean(true));
    m.set_input(MeasureId(105), w.clone(), Value::Text("hot".into()));
    let dt = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z").unwrap();
    m.set_input(MeasureId(106), w.clone(), Value::DateTime(dt.into()));
    m.set_input(MeasureId(107), w.clone(), Value::Enum(2));
    m.add_view(View {
        id: ViewId(1),
        name: Name("Revenue by quarter".into()),
        measure: MeasureId(102),
        axis_order: vec![T, P],
        n_rows: 1,
        n_cols: 1,
        page_items: vec![],
        filters: vec![Filter {
            category: P,
            items: vec![ItemId(20)],
        }],
        rect: Default::default(),
        placements: vec![],
    });
    m.lambdas.insert(
        3,
        NamedLambda {
            name: "Ident".into(),
            params: vec!["x".into()],
            body: improv_core_model::Expr::Ref(
                improv_core_model::functions::lambda_param(0),
                Default::default(),
            ),
        },
    );
    m.external_fns.insert(
        "double".into(),
        ExternalFn {
            name: "double".into(),
            language: improv_core_model::Language::Pure,
            body: "return args[0] * 2".into(),
            arg_types: vec![ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        },
    );
    m.external_calls.insert(
        MeasureId(105),
        ExternalCall {
            func: "double".into(),
            arg_measures: vec![MeasureId(100)],
            refresh_policy: RefreshPolicy::Manual,
        },
    );
    m.add_scenario(Scenario {
        id: ScenarioId(1),
        name: Name("High price".into()),
        overrides: [((MeasureId(100), w), Value::Number(99.0))]
            .into_iter()
            .collect(),
    });
    m.sql_sources.insert(
        MeasureId(101),
        SqlSource {
            query: "SELECT 1".into(),
            dimension_columns: vec![],
            value_column: "v".into(),
            refresh_policy: RefreshPolicy::Manual,
        },
    );
    m
}

#[test]
fn targets_need_a_file_and_a_model() {
    assert_eq!(
        split_target("duckdb:/a/b.duckdb?model=plan").unwrap(),
        ("/a/b.duckdb", "plan")
    );
    assert_eq!(
        split_target("duckdb:rel.duckdb?model=x").unwrap(),
        ("rel.duckdb", "x")
    );
    for (bad, why) in [
        ("/a/b.duckdb?model=p", "not a duckdb: target"),
        ("duckdb:/a/b.duckdb", "no ?model="),
        ("duckdb:/a/b.duckdb?model=", "no ?model="),
        ("duckdb:?model=p", "no file"),
        ("duckdb:/a?model=p&x=1", "unknown parameter \"x=1\""),
    ] {
        let e = split_target(bad).unwrap_err().to_string();
        assert!(
            e.contains(why) && e.contains("duckdb:<file>?model=<name>"),
            "{bad}: {e}"
        );
    }
}

#[test]
fn save_load_round_trips_everything() {
    let d = dir("round");
    let m = sample();
    let mut s = DuckModelStore::open(&target(&d, "plan")).unwrap();
    assert_eq!(
        s.load_model().unwrap(),
        Model::new(),
        "a new model is empty"
    );
    s.save_model(&m).unwrap();
    assert_eq!(s.load_model().unwrap(), m);
    // Through another handle, and after the instance closed (every handle gone).
    let mut again = DuckModelStore::open(&target(&d, "plan")).unwrap();
    assert_eq!(again.load_model().unwrap(), m);
    drop((s, again));
    let mut cold = DuckModelStore::open(&target(&d, "plan")).unwrap();
    assert_eq!(cold.load_model().unwrap(), m);
    assert_eq!(cold.reload_model().unwrap(), m);
    // A measure's categories keep their order, not the ids'.
    let loaded = cold.load_model().unwrap();
    assert_eq!(loaded.measures[&MeasureId(100)].categories, [P, T]);
    // Another model in the same file is its own.
    let mut other = DuckModelStore::open(&target(&d, "other")).unwrap();
    assert_eq!(other.load_model().unwrap(), Model::new());
    assert_eq!(cold.load_model().unwrap(), m);
}

#[test]
fn versions_move_and_stale_handles_conflict() {
    let d = dir("conflict");
    let t = target(&d, "plan");
    let m = sample();
    let mut a = DuckModelStore::open(&t).unwrap();
    assert_eq!((a.loaded_tx(), a.tx_high_water().unwrap()), (None, 0));
    a.save_model(&m).unwrap();
    assert_eq!(a.loaded_tx(), Some(1));
    let mut b = DuckModelStore::open(&t).unwrap();
    b.load_model().unwrap();
    assert_eq!(b.loaded_tx(), Some(1));
    a.save_model(&m).unwrap();
    assert_eq!(a.tx_high_water().unwrap(), 2);
    for e in [
        b.save_model(&m).unwrap_err(),
        b.save_cells(&m, &[(MeasureId(100), at(10, 20))])
            .unwrap_err(),
        b.save_changes(&m, &[], true).unwrap_err(),
    ] {
        assert!(matches!(e, StoreError::Conflict(_)), "{e}");
    }
    assert_eq!(a.tx_high_water().unwrap(), 2, "nothing written");
    b.set_loaded_tx(None);
    b.save_model(&m).unwrap();
    assert_eq!(b.loaded_tx(), Some(3));
}

/// Two handles that loaded the same version save at once: one commits,
/// the other is a conflict (DuckDB's write-write check on the version
/// row), never a lost update.
#[test]
fn racing_saves_are_a_conflict() {
    let d = dir("race");
    let t = target(&d, "plan");
    let mut a = DuckModelStore::open(&t).unwrap();
    a.save_model(&sample()).unwrap();
    let mut b = DuckModelStore::open(&t).unwrap();
    b.load_model().unwrap();
    // b's transaction holds the version row; a's save conflicts with it.
    let tx = b.conn.transaction().unwrap();
    bump(&tx, b.model_id).unwrap();
    let e = a.save_model(&sample()).unwrap_err();
    assert!(matches!(e, StoreError::Conflict(_)), "{e}");
    tx.commit().unwrap();
    assert_eq!(a.tx_high_water().unwrap(), 2);
}

#[test]
fn incremental_saves_write_only_what_they_name() {
    let d = dir("cells");
    let t = target(&d, "plan");
    let m = sample();
    let mut s = DuckModelStore::open(&t).unwrap();
    s.save_model(&m).unwrap();
    let mut m2 = m.clone();
    m2.set_input(MeasureId(100), at(10, 20), Value::Number(7.5));
    m2.inputs.remove(&(MeasureId(101), at(10, 21)));
    m2.set_input(
        MeasureId(105),
        Coordinate::from_pairs([(P, ItemId(21))]),
        Value::Text("new".into()),
    );
    let changed = [
        (MeasureId(100), at(10, 20)),
        (MeasureId(101), at(10, 21)),
        (MeasureId(105), Coordinate::from_pairs([(P, ItemId(21))])),
        // An unchanged one: harmless.
        (MeasureId(100), at(11, 20)),
    ];
    s.save_cells(&m2, &changed).unwrap();
    assert_eq!(DuckModelStore::open(&t).unwrap().load_model().unwrap(), m2);
    // A cell the call does not name is left alone, even when the model differs.
    let mut m3 = m2.clone();
    m3.set_input(MeasureId(100), at(12, 21), Value::Number(-1.0));
    s.save_changes(&m3, &[], false).unwrap();
    assert_eq!(DuckModelStore::open(&t).unwrap().load_model().unwrap(), m2);
    // A structural change with cells: a new item and measure, a dropped one.
    let mut m4 = m2.clone();
    m4.add_item(ItemId(13), T, "Q4");
    m4.add_measure(measure(108, "Cost", &[T], MeasureKind::Input));
    let q4 = Coordinate::from_pairs([(T, ItemId(13))]);
    m4.set_input(MeasureId(108), q4.clone(), Value::Number(3.0));
    m4.measures.remove(&MeasureId(107));
    m4.inputs.retain(|k, _| k.0 != MeasureId(107));
    m4.lambdas.clear();
    s.save_changes(&m4, &[(MeasureId(108), q4.clone())], true)
        .unwrap();
    assert_eq!(DuckModelStore::open(&t).unwrap().load_model().unwrap(), m4);
    // A cell of a measure the file lacks.
    let mut m5 = m4.clone();
    m5.add_measure(measure(109, "New", &[T], MeasureKind::Input));
    m5.set_input(MeasureId(109), q4.clone(), Value::Number(1.0));
    let e = s.save_cells(&m5, &[(MeasureId(109), q4)]).unwrap_err();
    assert!(
        e.to_string().contains("measure 109, which is not saved"),
        "{e}"
    );
    // An error value is refused, writing nothing.
    let mut bad = m4.clone();
    bad.set_input(
        MeasureId(100),
        at(10, 20),
        Value::Error(improv_core_model::ValueError::new(
            improv_core_model::ValueErrorKind::TypeMismatch,
        )),
    );
    let e = s
        .save_cells(&bad, &[(MeasureId(100), at(10, 20))])
        .unwrap_err();
    assert!(e.to_string().contains("error value"), "{e}");
    assert_eq!(DuckModelStore::open(&t).unwrap().load_model().unwrap(), m4);
}

#[test]
fn partial_loads_save_only_their_closure() {
    let d = dir("partial");
    let t = target(&d, "plan");
    let m = sample();
    DuckModelStore::open(&t).unwrap().save_model(&m).unwrap();
    let mut s = DuckModelStore::open(&t).unwrap();
    let part = s.load_partial(&[MeasureId(102)]).unwrap();
    assert_eq!(part.measures, m.measures);
    assert!(
        part.inputs.keys().all(|k| k.0 .0 <= 101),
        "Revenue's closure only"
    );
    assert_eq!(part.inputs.len(), 12);
    // An empty closure loads no cells.
    let none = DuckModelStore::open(&t).unwrap().load_partial(&[]).unwrap();
    assert!(none.inputs.is_empty());
    // Saving the partial model keeps every other measure's cells; a cell
    // it set outside the closure is written.
    let mut edit = part.clone();
    edit.set_input(MeasureId(100), at(10, 20), Value::Number(1.0));
    let w = Coordinate::from_pairs([(P, ItemId(21))]);
    edit.set_input(MeasureId(107), w.clone(), Value::Enum(1));
    s.save_model(&edit).unwrap();
    let mut want = m.clone();
    want.set_input(MeasureId(100), at(10, 20), Value::Number(1.0));
    want.set_input(MeasureId(107), w, Value::Enum(1));
    assert_eq!(
        DuckModelStore::open(&t).unwrap().load_model().unwrap(),
        want
    );
}

#[test]
fn policies_are_kept_per_model_and_move_the_version() {
    let d = dir("policies");
    let mut s = DuckModelStore::open(&target(&d, "plan")).unwrap();
    assert_eq!(s.load_policies().unwrap(), None);
    assert_eq!(s.policy_version().unwrap(), 0);
    s.load_model().unwrap();
    let text = "permit (principal, action, resource);";
    assert_eq!(s.save_policies(Some((text, "{}"))).unwrap(), 1);
    let p = s.load_policies().unwrap().unwrap();
    assert_eq!(
        (p.version, p.text.as_str(), p.groups.as_str()),
        (1, text, "{}")
    );
    assert_eq!(s.tx_high_water().unwrap(), 1, "a change feed sees it");
    // This handle's own write: its next save is no conflict.
    s.save_model(&sample()).unwrap();
    assert_eq!(s.save_policies(None).unwrap(), 2);
    assert_eq!(s.load_policies().unwrap(), None);
    assert_eq!(s.policy_version().unwrap(), 2);
    let mut other = DuckModelStore::open(&target(&d, "other")).unwrap();
    assert_eq!(other.policy_version().unwrap(), 0);
}

/// While one process holds the file, another is refused with a message
/// that names the holder and says what to do (the test binary, run again
/// as that other process).
#[test]
fn a_second_program_is_refused_by_name() {
    let d = dir("lock");
    let t = target(&d, "plan");
    let _held = DuckModelStore::open(&t).unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tests::open_from_another_process",
            "--ignored",
            "--nocapture",
        ])
        .env("IMPROV_DUCK_TARGET", &t)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    let (_, e) = text
        .split_once("refused: ")
        .expect("the second open is refused");
    let path = d.join("m.duckdb");
    assert!(
        e.starts_with(&format!(
            "duckdb error: {} is open in another program (",
            path.display()
        )),
        "{e}"
    );
    // DuckDB names the holder on Linux and macOS (on Windows, when it can).
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        assert!(e.contains(&format!("(PID {})", std::process::id())), "{e}");
    }
    assert!(
        e.contains("change it through the server, or stop the server first"),
        "{e}"
    );
    let fail = |m: &str| {
        let e = duckdb::Error::DuckDBFailure(duckdb::ffi::Error::new(1), Some(m.into()));
        open_error("f.duckdb", e).to_string()
    };
    assert_eq!(
        fail("disk full"),
        "duckdb error: cannot open f.duckdb: disk full"
    );
    // Windows says it its own way; a lock without a holder is anonymous.
    let w = fail("IO Error: Cannot open file \"f.duckdb\": used\nFile is already open in \nC:\\improv-server.exe (PID 7)");
    assert!(
        w.contains("open in another program (C:\\improv-server.exe (PID 7))"),
        "{w}"
    );
    let anon = fail("IO Error: Could not set lock on file \"f.duckdb\": Resource busy");
    assert!(
        anon.contains("open in another program (another program)"),
        "{anon}"
    );
}

/// The other process of [`a_second_program_is_refused_by_name`].
#[test]
#[ignore = "run by a_second_program_is_refused_by_name"]
fn open_from_another_process() {
    let Ok(t) = std::env::var("IMPROV_DUCK_TARGET") else {
        return;
    };
    match DuckModelStore::open(&t) {
        Ok(_) => println!("opened"),
        Err(e) => println!("refused: {e}"),
    }
}

#[test]
fn bad_files_and_rows_are_reported() {
    let d = dir("bad");
    // Not a DuckDB file.
    let junk = d.join("junk.duckdb");
    std::fs::write(&junk, b"not a database at all, just text").unwrap();
    let e = DuckModelStore::open(&format!("duckdb:{}?model=m", junk.display()))
        .err()
        .unwrap();
    assert!(e.to_string().contains("cannot open"), "{e}");
    // A directory that does not exist.
    let e = DuckModelStore::open("duckdb:/nonexistent-improv/x/y.duckdb?model=m")
        .err()
        .unwrap();
    assert!(e.to_string().contains("cannot open"), "{e}");
    // A catalog from a newer release.
    let t = target(&d, "plan");
    {
        let s = DuckModelStore::open(&t).unwrap();
        s.conn
            .execute("INSERT INTO improv.catalog VALUES (99)", [])
            .unwrap();
    }
    let e = DuckModelStore::open(&t).err().unwrap();
    assert!(
        e.to_string()
            .contains("version 99, newer than this release"),
        "{e}"
    );
    let d = dir("rows");
    let t = target(&d, "plan");
    let mut s = DuckModelStore::open(&t).unwrap();
    s.save_model(&sample()).unwrap();
    // Each corruption, then a full save that repairs it.
    let corrupt = |s: &mut DuckModelStore, sql: &str, want: &str| {
        s.conn.execute_batch(sql).unwrap();
        let e = s.load_model().unwrap_err().to_string();
        assert!(e.contains(want), "{sql}: {e}");
        s.set_loaded_tx(None);
        s.save_model(&sample()).unwrap();
    };
    corrupt(
        &mut s,
        "UPDATE improv.measure SET value_type = 'money' WHERE id = 100",
        "unknown value type",
    );
    corrupt(
        &mut s,
        "UPDATE improv.measure SET kind = 'odd' WHERE id = 100",
        "has kind \"odd\"",
    );
    corrupt(
        &mut s,
        "UPDATE improv.measure SET formula = NULL WHERE id = 102",
        "has kind \"derived\"",
    );
    corrupt(
        &mut s,
        "UPDATE improv.measure SET formula = 'x' WHERE id = 102",
        "serialization",
    );
    corrupt(
        &mut s,
        "UPDATE improv.category SET id = -1 WHERE id = 1",
        "out of range",
    );
    corrupt(
        &mut s,
        "UPDATE improv.cell SET measure_id = 999 WHERE measure_id = 100",
        "unknown measure 999",
    );
    corrupt(
        &mut s,
        "UPDATE improv.cell SET value_text = 'x' WHERE measure_id = 100",
        "exactly one",
    );
    corrupt(
        &mut s,
        "UPDATE improv.lambda SET definition = 'nope'",
        "lambda 3",
    );
    // A datetime that does not parse loads as its text.
    let c = s.conn.try_clone().unwrap();
    c.execute(
        "UPDATE improv.cell SET value_text = 'soon' WHERE measure_id = 106",
        [],
    )
    .unwrap();
    let m = s.load_model().unwrap();
    let w = Coordinate::from_pairs([(P, ItemId(20))]);
    assert_eq!(
        m.input(MeasureId(106), &w),
        Some(&Value::Text("soon".into()))
    );
    // A format this release cannot read is General.
    c.execute("UPDATE improv.measure SET format = '{' WHERE id = 100", [])
        .unwrap();
    assert_eq!(
        s.load_model().unwrap().measures[&MeasureId(100)].format,
        None
    );
    // A formula past the load bound opens as #REF!.
    let deep = format!(
        "{{\"expr\":{}{{\"Literal\":{{\"Number\":1.0}}}}{}}}",
        "{\"UnaryOp\":[\"Neg\",".repeat(400),
        "]}".repeat(400)
    );
    c.execute(
        "UPDATE improv.measure SET formula = ? WHERE id = 102",
        [deep],
    )
    .unwrap();
    let m = s.load_model().unwrap();
    assert!(m.measures[&MeasureId(102)]
        .description
        .as_deref()
        .unwrap()
        .contains("unreadable"));
}

#[test]
fn canonical_paths_share_one_instance() {
    let d = dir("canon");
    let a = target(&d, "plan");
    let b = format!("duckdb:{}/./m.duckdb?model=plan", d.display());
    let mut s = DuckModelStore::open(&a).unwrap();
    s.save_model(&sample()).unwrap();
    let mut t = DuckModelStore::open(&b).unwrap();
    assert_eq!(t.load_model().unwrap(), sample());
    assert_eq!(
        registry()
            .iter()
            .filter(|(p, _, _)| p.starts_with(d.canonicalize().unwrap()))
            .count(),
        1
    );
    // A relative file name resolves against the working directory.
    assert_eq!(
        canonical(Path::new("x.duckdb")).unwrap(),
        std::env::current_dir()
            .unwrap()
            .canonicalize()
            .unwrap()
            .join("x.duckdb")
    );
    // Windows resolves `..` by the text before it looks at the disk, so
    // this path is the existing root there.
    #[cfg(not(windows))]
    assert!(
        canonical(Path::new("/nonexistent-improv/..")).is_err(),
        "no file name"
    );
    assert!(canonical(Path::new("/nonexistent-improv/x")).is_err());
}

#[test]
fn value_codecs() {
    for v in [
        Value::Number(1.5),
        Value::Boolean(false),
        Value::Text(String::new()),
        Value::Enum(7),
    ] {
        let vt = v.type_of().unwrap();
        assert_eq!(untyped(vt, typed(&v).unwrap()).unwrap(), v);
    }
    assert!(untyped(ValueType::Number, (None, None, None)).is_err());
    assert!(untyped(ValueType::Number, (Some(1.0), Some(true), None)).is_err());
    for vt in [
        ValueType::Number,
        ValueType::Boolean,
        ValueType::Text,
        ValueType::DateTime,
        ValueType::Enum,
    ] {
        assert_eq!(vt_parse(vt_name(vt)).unwrap(), vt);
    }
    assert!(id32(-1).is_err() && id32(1 << 33).is_err());
    let e: StoreError = duckdb::Error::QueryReturnedNoRows.into();
    assert!(matches!(e, StoreError::Duck(_)));
}

/// A model of `n` categories with items, input measures over subsets of
/// them, and a value of every storable type in each cell.
fn arb_model() -> impl Strategy<Value = Model> {
    let value = prop_oneof![
        any::<f64>()
            .prop_filter("finite", |f| f.is_finite())
            .prop_map(Value::Number),
        any::<bool>().prop_map(Value::Boolean),
        ".{0,12}".prop_map(Value::Text),
        any::<u32>().prop_map(Value::Enum),
        (0i64..4_000_000_000)
            .prop_map(|s| Value::DateTime(chrono::DateTime::from_timestamp(s, 0).unwrap())),
    ];
    (
        prop::collection::btree_set(1u32..30, 1..4),
        prop::collection::vec((1usize..4, any::<bool>()), 0..4),
        prop::collection::vec(value, 0..40),
        ".{0,10}",
    )
        .prop_map(|(cats, measures, values, name)| {
            let mut m = Model::new();
            let cats: Vec<u32> = cats.into_iter().collect();
            for &c in &cats {
                m.add_category(CategoryId(c), format!("{name}{c}"));
                for i in 0..3 {
                    m.add_item(ItemId(c * 10 + i), CategoryId(c), format!("i{i}"));
                }
            }
            for (k, (span, reversed)) in measures.iter().enumerate() {
                let mut over: Vec<CategoryId> =
                    cats.iter().take(*span).map(|&c| CategoryId(c)).collect();
                if *reversed {
                    over.reverse();
                }
                let id = MeasureId(100 + k as u32);
                let vt = values
                    .get(k)
                    .and_then(Value::type_of)
                    .unwrap_or(ValueType::Number);
                let mut x = measure(id.0, &format!("M{k}"), &over, MeasureKind::Input);
                x.value_type = vt;
                m.add_measure(x);
                for (j, v) in values
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| v.type_of() == Some(vt))
                {
                    let at = Coordinate::from_pairs(
                        over.iter().map(|c| (*c, ItemId(c.0 * 10 + (j as u32 % 3)))),
                    );
                    m.set_input(id, at, v.clone());
                }
            }
            m
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// save ↔ load: any model saved loads back equal, and an incremental
    /// save of the cells that differ between two models leaves the second.
    #[test]
    fn save_load_round_trips(a in arb_model(), b in arb_model()) {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let d = dir(&format!("prop{}", N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        let t = target(&d, "p");
        let mut s = DuckModelStore::open(&t).unwrap();
        s.save_model(&a).unwrap();
        prop_assert_eq!(&DuckModelStore::open(&t).unwrap().load_model().unwrap(), &a);
        let mut cells: Vec<_> = b.inputs.keys().chain(a.inputs.keys()).cloned().collect();
        cells.sort();
        cells.dedup();
        s.save_changes(&b, &cells, true).unwrap();
        prop_assert_eq!(&DuckModelStore::open(&t).unwrap().load_model().unwrap(), &b);
        drop(s);
        let _ = std::fs::remove_dir_all(&d);
    }
}
