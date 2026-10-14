#![cfg_attr(coverage_nightly, coverage(off))]

use super::*;
use improv_core_model::{parse_expr, Filter, RefreshPolicy, View, ViewId};

const T: CategoryId = CategoryId(1);
const P: CategoryId = CategoryId(2);
const S: CategoryId = CategoryId(3);

/// Live: needs `IMPROV_TEST_PG` (a libpq DSN to a scratch database). Returns
/// a URL for a fresh, empty model named `name`.
fn live(name: &str) -> Option<String> {
    let Ok(dsn) = std::env::var("IMPROV_TEST_PG") else {
        println!("skipped: IMPROV_TEST_PG not set");
        return None;
    };
    let name = format!("t_{name}_{}", std::process::id());
    let url = format!("{dsn}?model={name}");
    let s = PgModelStore::open(&url).expect("open IMPROV_TEST_PG");
    let mut c = s.client;
    c.execute("DELETE FROM improv.model WHERE name = $1", &[&name])
        .unwrap();
    Some(url)
}

fn client() -> Client {
    let dsn = std::env::var("IMPROV_TEST_PG").unwrap();
    dsn.parse::<Config>().unwrap().connect(NoTls).unwrap()
}

fn measure(id: u32, name: &str, cats: &[CategoryId], kind: MeasureKind) -> Measure {
    Measure {
        id: MeasureId(id),
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: cats.to_vec(),
        kind,
        description: None,
    }
}

/// The 3-D demo: Time x Product x Scenario; Price, Quantity inputs; Revenue,
/// Total derived; two views; a meta blob (external fn + call + scenario +
/// sql source).
fn sample() -> Model {
    let mut m = Model::new();
    m.add_category(T, "Time");
    m.add_category(P, "Product");
    m.add_category(S, "Scenario");
    for (i, q) in ["Q1", "Q2", "Q3", "Q4"].iter().enumerate() {
        m.add_item(ItemId(10 + i as u32), T, *q);
    }
    m.add_item(ItemId(20), P, "Widget");
    m.add_item(ItemId(21), P, "Gadget");
    m.add_item(ItemId(30), S, "Plan");
    m.add_item(ItemId(31), S, "Actual");
    let mut price = measure(100, "Price", &[T, P, S], MeasureKind::Input);
    price.description = Some("Unit price".into());
    m.add_measure(price);
    m.add_measure(measure(101, "Quantity", &[T, P, S], MeasureKind::Input));
    let rev = parse_expr(&m, "Price * Quantity").unwrap();
    m.add_measure(measure(
        102,
        "Revenue",
        &[T, P, S],
        MeasureKind::Derived(rev),
    ));
    let total = parse_expr(&m, "SUM(Revenue OVER Time)").unwrap();
    m.add_measure(measure(103, "Total", &[P, S], MeasureKind::Derived(total)));
    let mut flag = measure(104, "Launched", &[P], MeasureKind::Input);
    flag.value_type = ValueType::Boolean;
    m.add_measure(flag);
    let mut note = measure(105, "Note", &[P], MeasureKind::Input);
    note.value_type = ValueType::Text;
    m.add_measure(note);
    let mut when = measure(106, "Ship", &[P], MeasureKind::Input);
    when.value_type = ValueType::DateTime;
    m.add_measure(when);
    let mut tier = measure(107, "Tier", &[P], MeasureKind::Input);
    tier.value_type = ValueType::Enum;
    m.add_measure(tier);
    for q in 0..4 {
        for p in 0..2 {
            for s in 0..2 {
                let c = Coordinate::from_pairs([
                    (T, ItemId(10 + q)),
                    (P, ItemId(20 + p)),
                    (S, ItemId(30 + s)),
                ]);
                m.set_input(
                    MeasureId(100),
                    c.clone(),
                    Value::Number(f64::from(10 + 10 * p + q + s)),
                );
                m.set_input(
                    MeasureId(101),
                    c,
                    Value::Number(f64::from(100 - 50 * p + 10 * q - 5 * s)),
                );
            }
        }
    }
    let w = Coordinate::from_pairs([(P, ItemId(20))]);
    m.set_input(MeasureId(104), w.clone(), Value::Boolean(true));
    m.set_input(MeasureId(105), w.clone(), Value::Text("hot".into()));
    let dt = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z").unwrap();
    m.set_input(
        MeasureId(106),
        w.clone(),
        Value::DateTime(dt.with_timezone(&chrono::Utc)),
    );
    m.set_input(MeasureId(107), w.clone(), Value::Enum(2));
    for (id, name, mid) in [(1, "Revenue by quarter", 102), (2, "Price plan", 100)] {
        m.add_view(View {
            id: ViewId(id),
            name: Name(name.into()),
            measure: MeasureId(mid),
            axis_order: vec![T, P, S],
            n_rows: 1,
            n_cols: 1,
            page_items: vec![(S, ItemId(30))],
            filters: vec![Filter {
                category: P,
                items: vec![ItemId(20)],
            }],
            rect: Default::default(),
            placements: vec![],
        });
    }
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
    m
}

#[test]
fn save_load_round_trips_the_3d_model() {
    let Some(url) = live("roundtrip") else { return };
    let mut s = PgModelStore::open(&url).unwrap();
    assert_eq!(s.loaded_tx(), None);
    let m = sample();
    s.save_model(&m).unwrap();
    assert_eq!(s.loaded_tx(), Some(1));
    assert_eq!(s.tx_high_water().unwrap(), 1);
    let loaded = PgModelStore::open(&url).unwrap().load_model().unwrap();
    assert_eq!(loaded, m);

    // Re-save after edits: deleted rows go, version bumps, still equal.
    let mut m2 = loaded;
    m2.measures.remove(&MeasureId(107));
    m2.inputs.retain(|(mid, _), _| mid.0 != 107);
    m2.views.remove(&ViewId(2));
    m2.items.remove(&ItemId(31));
    m2.categories
        .get_mut(&S)
        .unwrap()
        .items
        .retain(|i| i.0 != 31);
    m2.inputs.retain(|(_, c), _| c.get(S) != Some(ItemId(31)));
    m2.scenarios.clear();
    let mut s = PgModelStore::open(&url).unwrap();
    s.load_model().unwrap();
    s.save_model(&m2).unwrap();
    assert_eq!(s.loaded_tx(), Some(2));
    assert_eq!(s.reload_model().unwrap(), m2);
}

#[test]
fn load_partial_loads_only_the_closure_cells() {
    let Some(url) = live("partial") else { return };
    let mut s = PgModelStore::open(&url).unwrap();
    s.save_model(&sample()).unwrap();
    let p = s.load_partial(&[MeasureId(102)]).unwrap();
    assert_eq!(p.measures.len(), 8);
    assert!(p.inputs.keys().all(|(m, _)| m.0 == 100 || m.0 == 101));
    assert_eq!(p.inputs.len(), 32);
    assert!(s.load_partial(&[]).unwrap().inputs.is_empty());
}

#[test]
fn concurrent_writers_conflict() {
    let Some(url) = live("conflict") else { return };
    let mut a = PgModelStore::open(&url).unwrap();
    let mut b = PgModelStore::open(&url).unwrap();
    let m = a.load_model().unwrap();
    b.load_model().unwrap();
    a.save_model(&m).unwrap();
    let e = b.save_model(&m).unwrap_err();
    assert!(matches!(e, StoreError::Conflict(_)), "{e}");
    assert!(e.to_string().contains("reload and retry"));
    // Reload and retry wins; carrying a token across handles works too.
    b.reload_model().unwrap();
    b.save_model(&m).unwrap();
    let mut c = PgModelStore::open(&url).unwrap();
    c.set_loaded_tx(a.loaded_tx());
    assert!(matches!(c.save_model(&m), Err(StoreError::Conflict(_))));
    c.set_loaded_tx(None);
    c.save_model(&m).unwrap();
}

#[test]
fn unbound_cells_live_in_improv_cell() {
    let Some(url) = live("cells") else { return };
    let mut s = PgModelStore::open(&url).unwrap();
    let mut m = sample();
    s.save_model(&m).unwrap();
    let n = |s: &mut PgModelStore| -> i64 {
        s.client
            .query_one(
                "SELECT count(*) FROM improv.cell WHERE model_id = $1",
                &[&s.model_id],
            )
            .unwrap()
            .get(0)
    };
    assert_eq!(n(&mut s), m.inputs.len() as i64);
    let c = Coordinate::from_pairs([(P, ItemId(20))]);
    m.inputs.remove(&(MeasureId(104), c.clone()));
    m.set_input(MeasureId(105), c, Value::Text("cold".into()));
    s.save_model(&m).unwrap();
    assert_eq!(n(&mut s), m.inputs.len() as i64);
    assert_eq!(s.load_model().unwrap().inputs, m.inputs);
}

/// A model over a `sales` fact table: Price and Quantity bound to columns.
fn bound_model(url: &str, table: &str) -> PgModelStore {
    let mut c = client();
    c.batch_execute(&format!(
        "DROP TABLE IF EXISTS {table}; \
         CREATE TABLE {table} (quarter text, product text, price float8, qty int, \
                               PRIMARY KEY (quarter, product)); \
         INSERT INTO {table} VALUES ('Q1','Widget',10,100), ('Q2','Widget',11,110), \
                                    ('Q1','Gadget',20,NULL)"
    ))
    .unwrap();
    let mut s = PgModelStore::open(url).unwrap();
    let mut m = Model::new();
    m.add_category(T, "Time");
    m.add_category(P, "Product");
    m.add_item(ItemId(20), P, "Widget");
    m.add_measure(measure(100, "Price", &[T, P], MeasureKind::Input));
    m.add_measure(measure(101, "Quantity", &[T, P], MeasureKind::Input));
    let rev = parse_expr(&m, "Price * Quantity").unwrap();
    m.add_measure(measure(102, "Revenue", &[T, P], MeasureKind::Derived(rev)));
    s.save_model(&m).unwrap();
    for (mid, col) in [(100i64, "price"), (101, "qty")] {
        s.client
            .execute(
                "INSERT INTO improv.binding(model_id, measure_id, rel, value_col, dim_cols) \
                 VALUES ($1, $2, $3::text::regclass, $4, \
                         '{\"Time\":\"quarter\",\"Product\":\"product\"}')",
                &[&s.model_id, &mid, &table, &col],
            )
            .unwrap();
    }
    s
}

#[test]
fn bound_measures_read_and_write_the_fact_table() {
    let Some(url) = live("bound") else { return };
    let table = format!("sales_{}", std::process::id());
    let mut s = bound_model(&url, &table);
    let mut m = s.load_model().unwrap();
    let item = |m: &Model, c: CategoryId, n: &str| {
        m.items
            .values()
            .find(|i| i.category == c && i.name.0 == n)
            .unwrap()
            .id
    };
    // Widget kept its id; Q1, Q2, Gadget were minted from column values.
    assert_eq!(item(&m, P, "Widget"), ItemId(20));
    let (q1, q2, w, g) = (
        item(&m, T, "Q1"),
        item(&m, T, "Q2"),
        ItemId(20),
        item(&m, P, "Gadget"),
    );
    let at = |t, p| Coordinate::from_pairs([(T, t), (P, p)]);
    assert_eq!(m.inputs[&(MeasureId(100), at(q1, w))], Value::Number(10.0));
    assert_eq!(m.inputs[&(MeasureId(101), at(q2, w))], Value::Number(110.0));
    assert_eq!(m.inputs[&(MeasureId(100), at(q1, g))], Value::Number(20.0));
    // NULL value: no cell.
    assert!(!m.inputs.contains_key(&(MeasureId(101), at(q1, g))));
    assert_eq!(m.inputs.len(), 5);

    // A GUI-style edit + a new cell, then save: the fact table changes.
    m.set_input(MeasureId(100), at(q1, w), Value::Number(12.5));
    m.set_input(MeasureId(101), at(q1, g), Value::Number(7.0));
    m.add_item(ItemId(500), T, "Q3");
    m.set_input(MeasureId(100), at(ItemId(500), w), Value::Number(13.0));
    s.save_model(&m).unwrap();
    let rows: Vec<(String, String, Option<f64>, Option<i32>)> = client()
        .query(&format!("SELECT * FROM {table} ORDER BY 1, 2"), &[])
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect();
    let row = |q: &str, p: &str, pr, qt| (q.to_string(), p.to_string(), pr, qt);
    assert_eq!(
        rows,
        [
            row("Q1", "Gadget", Some(20.0), Some(7)),
            row("Q1", "Widget", Some(12.5), Some(100)),
            row("Q2", "Widget", Some(11.0), Some(110)),
            row("Q3", "Widget", Some(13.0), None),
        ]
    );
    // Bound cells never land in improv.cell; minted items persist.
    let n: i64 = client()
        .query_one(
            "SELECT count(*) FROM improv.cell WHERE model_id = $1",
            &[&s.model_id],
        )
        .unwrap()
        .get(0);
    assert_eq!(n, 0);
    let again = s.load_model().unwrap();
    assert_eq!(again.inputs, m.inputs);
    assert_eq!(item(&again, T, "Q1"), q1);

    // NULL dimension: the row is ignored. Duplicate coordinate: an error.
    let mut c = client();
    c.batch_execute(&format!(
        "ALTER TABLE {table} DROP CONSTRAINT {table}_pkey; \
         ALTER TABLE {table} ALTER quarter DROP NOT NULL; \
         INSERT INTO {table} VALUES (NULL, 'Widget', 1, 1)"
    ))
    .unwrap();
    assert_eq!(s.load_model().unwrap().inputs, m.inputs);
    c.batch_execute(&format!(
        "INSERT INTO {table} VALUES ('Q1', 'Widget', 1, 1)"
    ))
    .unwrap();
    let e = s.load_model().unwrap_err();
    assert!(matches!(e, StoreError::Integrity(_)), "{e}");
    // A cell missing a bound dimension cannot be written.
    let mut bad = m.clone();
    bad.set_input(
        MeasureId(100),
        Coordinate::from_pairs([(T, q1)]),
        Value::Number(1.0),
    );
    bad.inputs.retain(|(mid, _), _| mid.0 == 100);
    s.set_loaded_tx(None);
    assert!(matches!(s.save_model(&bad), Err(StoreError::Integrity(_))));
    // A binding naming a category the model lacks.
    c.execute(
        "UPDATE improv.binding SET dim_cols = '{\"Nope\":\"quarter\"}' WHERE model_id = $1",
        &[&s.model_id],
    )
    .unwrap();
    assert!(matches!(s.load_model(), Err(StoreError::Integrity(_))));
    c.batch_execute(&format!("DROP TABLE {table} CASCADE"))
        .unwrap();
}

#[test]
fn corrupt_rows_are_integrity_errors() {
    let Some(url) = live("corrupt") else { return };
    let mut s = PgModelStore::open(&url).unwrap();
    s.save_model(&sample()).unwrap();
    let mid = s.model_id;
    let mut c = client();
    for (fix, undo) in [
        (
            "UPDATE improv.measure SET formula = NULL WHERE model_id = $1 AND id = 102",
            "DELETE FROM improv.measure WHERE model_id = $1 AND id = 102",
        ),
        (
            "UPDATE improv.measure SET value_type = 'blob' WHERE model_id = $1 AND id = 100",
            "UPDATE improv.measure SET value_type = 'number' WHERE model_id = $1 AND id = 100",
        ),
        (
            "UPDATE improv.category SET id = 5000000000 WHERE model_id = $1 AND id = 1",
            "DELETE FROM improv.category WHERE model_id = $1 AND id = 5000000000",
        ),
        (
            "UPDATE improv.cell SET value = '\"x\"' WHERE model_id = $1 AND measure_id = 100",
            "DELETE FROM improv.cell WHERE model_id = $1 AND measure_id = 100",
        ),
        (
            "UPDATE improv.cell SET measure_id = 999 WHERE model_id = $1 AND measure_id = 101",
            "DELETE FROM improv.cell WHERE model_id = $1 AND measure_id = 999",
        ),
    ] {
        c.execute(fix, &[&mid]).unwrap();
        let e = s.load_model().unwrap_err();
        assert!(matches!(e, StoreError::Integrity(_)), "{fix}: {e}");
        c.execute(undo, &[&mid]).unwrap();
    }
    s.load_model().unwrap();
    // A meta blob of the wrong shape is a serde error.
    c.execute(
        "UPDATE improv.meta SET blob = '\"x\"' WHERE model_id = $1",
        &[&mid],
    )
    .unwrap();
    assert!(matches!(s.load_model(), Err(StoreError::Serde(_))));
}

#[test]
fn error_values_are_refused() {
    let Some(url) = live("errval") else { return };
    let mut s = PgModelStore::open(&url).unwrap();
    let mut m = sample();
    m.set_input(
        MeasureId(100),
        Coordinate::from_pairs([(T, ItemId(10)), (P, ItemId(20)), (S, ItemId(30))]),
        Value::Error(improv_core_model::ValueError::new(
            improv_core_model::ValueErrorKind::TypeMismatch,
        )),
    );
    assert!(matches!(s.save_model(&m), Err(StoreError::Integrity(_))));
    // Rolled back: nothing was written.
    assert_eq!(s.tx_high_water().unwrap(), 0);
}

#[test]
fn url_needs_a_model_and_keeps_other_params() {
    let (c, m) = split_model_url("postgres://u@h/db?sslmode=disable&model=plan&x=1").unwrap();
    assert_eq!(
        (c.as_str(), m.as_str()),
        ("postgres://u@h/db?sslmode=disable&x=1", "plan")
    );
    let (c, m) = split_model_url("postgres://u@h/db?model=plan").unwrap();
    assert_eq!((c.as_str(), m.as_str()), ("postgres://u@h/db", "plan"));
    for bad in [
        "postgres://u@h/db",
        "postgres://u@h/db?model=",
        "postgres://h/db?x=1",
    ] {
        assert!(
            matches!(split_model_url(bad), Err(StoreError::Pg(_))),
            "{bad}"
        );
        assert!(PgModelStore::open(bad).is_err());
    }
}

#[test]
fn transport_policy_matches_storage_sql() {
    for bad in [
        "postgres://u@db.example.com/x?model=m",
        "postgres://u@localhost/x?sslmode=require&model=m",
        "not a url ===?model=m",
    ] {
        let e = PgModelStore::open(bad).err().unwrap().to_string();
        assert!(e.contains("TLS") || e.contains("invalid"), "{bad}: {e}");
    }
    let ok: Config = "host=localhost hostaddr=127.0.0.1 user=u".parse().unwrap();
    assert!(check_transport(&ok).is_ok());
    let off: Config = "host=db.example.com sslmode=disable user=u"
        .parse()
        .unwrap();
    assert!(check_transport(&off).is_ok());
    // Nothing listens on port 1: a connect error, with no DSN echoed.
    let e = PgModelStore::open("postgres://u:secret@127.0.0.1:1/x?model=m")
        .err()
        .unwrap()
        .to_string();
    assert!(e.contains("connect failed") && !e.contains("secret"), "{e}");
}

#[test]
fn value_codecs() {
    assert_eq!(ident("a\"b"), "\"a\"\"b\"");
    assert!(id32(-1).is_err());
    for vt in [
        ValueType::Number,
        ValueType::Boolean,
        ValueType::Text,
        ValueType::DateTime,
        ValueType::Enum,
    ] {
        assert_eq!(vt_parse(vt_name(vt)).unwrap(), vt);
    }
    use serde_json::json as j;
    assert_eq!(
        json_to_value(ValueType::Text, &j!(3)).unwrap(),
        Value::Text("3".into())
    );
    assert_eq!(
        json_to_value(ValueType::DateTime, &j!("soon")).unwrap(),
        Value::Text("soon".into())
    );
    assert!(json_to_value(ValueType::Enum, &j!(-1)).is_err());
    assert!(json_to_value(ValueType::Boolean, &j!(1)).is_err());
    assert_eq!(
        StoreError::Mentat("m".into()).to_string(),
        "mentat error: m"
    );
}
