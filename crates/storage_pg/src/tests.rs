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
    m.lambdas.insert(
        3,
        improv_core_model::NamedLambda {
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

/// Two clients loading one bound model at once, with dimension values no
/// item exists for yet, get identical ids: `improv.mint_items` mints each
/// value once (`item_name_uniq` holds) and does not bump the version.
#[test]
fn concurrent_loads_mint_identical_item_ids() {
    let Some(url) = live("mint") else { return };
    let table = format!("sales_mint_{}", std::process::id());
    let mut s = bound_model(&url, &table);
    let mut c = client();
    c.execute(
        &format!("INSERT INTO {table} SELECT 'Q' || q, 'P' || p, 1, 1 FROM generate_series(3, 40) q, generate_series(1, 20) p"),
        &[],
    )
    .unwrap();
    let before = s.tx_high_water().unwrap();
    let ids = |m: &Model| -> Vec<(u32, u32, String)> {
        let mut v: Vec<_> = m
            .items
            .values()
            .map(|i| (i.id.0, i.category.0, i.name.0.clone()))
            .collect();
        v.sort();
        v
    };
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
    let loads: Vec<_> = (0..2)
        .map(|_| {
            let (url, barrier) = (url.clone(), barrier.clone());
            std::thread::spawn(move || {
                let mut s = PgModelStore::open(&url).unwrap();
                barrier.wait();
                s.load_model().unwrap()
            })
        })
        .collect();
    let models: Vec<Model> = loads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(ids(&models[0]), ids(&models[1]));
    assert_eq!(models[0].inputs, models[1].inputs);
    assert_eq!(
        models[0].items.len(),
        1 + 40 + 21,
        "Widget, Q1..Q40, Gadget + P1..P20"
    );
    let dup: i64 = c
        .query_one(
            "SELECT count(*) - count(DISTINCT (category_id, name)) FROM improv.item \
             WHERE model_id = $1",
            &[&s.model_id],
        )
        .unwrap()
        .get(0);
    assert_eq!(dup, 0);
    assert_eq!(
        ids(&s.load_model().unwrap()),
        ids(&models[0]),
        "a reload re-mints nothing"
    );
    assert_eq!(
        s.tx_high_water().unwrap(),
        before,
        "minting does not bump the version"
    );
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
    // Widget kept its id; Q1, Q2, Gadget were minted by improv.mint_items
    // from column values, ordered by (category id, name), after the highest
    // existing id.
    assert_eq!(item(&m, P, "Widget"), ItemId(20));
    assert_eq!(
        [item(&m, T, "Q1"), item(&m, T, "Q2"), item(&m, P, "Gadget")],
        [ItemId(21), ItemId(22), ItemId(23)]
    );
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
    // Formulas written as DSL text (by the extension) load too. (Restore the
    // rows the undo steps above deleted first.)
    s.set_loaded_tx(None);
    s.save_model(&sample()).unwrap();
    for text in ["Revenue = Price * Quantity", "Price * Quantity"] {
        c.execute(
            "UPDATE improv.measure SET formula = $2 WHERE model_id = $1 AND id = 102",
            &[&mid, &text],
        )
        .unwrap();
        assert_eq!(s.load_model().unwrap(), sample(), "{text}");
    }
    c.execute(
        "UPDATE improv.measure SET formula = 'Price * Nope' WHERE model_id = $1 AND id = 102",
        &[&mid],
    )
    .unwrap();
    assert!(matches!(s.load_model(), Err(StoreError::Integrity(_))));
    c.execute(
        "UPDATE improv.measure SET formula = 'Price' WHERE model_id = $1 AND id = 102",
        &[&mid],
    )
    .unwrap();
    // A meta blob of the wrong shape is a serde error.
    c.execute(
        "UPDATE improv.meta SET blob = '\"x\"' WHERE model_id = $1",
        &[&mid],
    )
    .unwrap();
    assert!(matches!(s.load_model(), Err(StoreError::Serde(_))));
}

/// Named LAMBDAs live in `improv.lambda` (JSON when this store writes them,
/// text when `improv.define` does); a model saved before the table keeps
/// its lambdas in the meta blob, and those still load.
#[test]
fn lambdas_live_in_improv_lambda() {
    let Some(url) = live("lambda") else { return };
    let mut s = PgModelStore::open(&url).unwrap();
    let mut m = sample();
    s.save_model(&m).unwrap();
    let mid = s.model_id;
    let mut c = client();
    let rows = |c: &mut Client| -> Vec<(i64, String, String)> {
        c.query(
            "SELECT id, name, definition FROM improv.lambda WHERE model_id = $1 ORDER BY id",
            &[&mid],
        )
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2)))
        .collect()
    };
    let json = serde_json::to_string(&m.lambdas[&3]).unwrap();
    assert_eq!(rows(&mut c), vec![(3, "Ident".into(), json.clone())]);
    let blob: String = c
        .query_one(
            "SELECT blob::text FROM improv.meta WHERE model_id = $1",
            &[&mid],
        )
        .unwrap()
        .get(0);
    assert!(!blob.contains("lambdas"), "{blob}");

    // improv.define writes text, with a category parameter; a formula
    // written as text calls it; a save that leaves it alone keeps the text.
    let text = "Norm = LAMBDA(x, c, x / MAX(x OVER c))";
    c.execute(
        "INSERT INTO improv.lambda VALUES ($1, 4, 'Norm', $2)",
        &[&mid, &text],
    )
    .unwrap();
    c.execute(
        "INSERT INTO improv.measure VALUES ($1, 120, 'Share', 'number', 'derived', \
         'Share = Norm(Quantity, Time)', NULL, '{1,2,3}')",
        &[&mid],
    )
    .unwrap();
    s.set_loaded_tx(None);
    let mut got = s.load_model().unwrap();
    assert_eq!(got.lambdas[&4].params, ["x", "c"]);
    let MeasureKind::Derived(f) = &got.measures[&MeasureId(120)].kind else {
        panic!()
    };
    assert_eq!(
        f.expr,
        parse_expr(&got, "Norm(Quantity, Time)").unwrap().expr
    );
    got.lambdas.remove(&3);
    s.save_model(&got).unwrap();
    assert_eq!(rows(&mut c), vec![(4, "Norm".into(), text.into())]);
    assert_eq!(s.reload_model().unwrap().lambdas, got.lambdas);

    // The meta blob of a model saved before improv.lambda.
    m.lambdas.insert(
        7,
        improv_core_model::NamedLambda {
            name: "Old".into(),
            params: vec!["x".into()],
            body: improv_core_model::Expr::Literal(Value::Number(1.0)),
        },
    );
    c.execute("DELETE FROM improv.lambda WHERE model_id = $1", &[&mid])
        .unwrap();
    c.execute(
        "DELETE FROM improv.measure WHERE model_id = $1 AND id = 120",
        &[&mid],
    )
    .unwrap();
    let old = serde_json::json!({ "lambdas": m.lambdas }).to_string();
    c.execute(
        "UPDATE improv.meta SET blob = $2::text::jsonb WHERE model_id = $1",
        &[&mid, &old],
    )
    .unwrap();
    s.set_loaded_tx(None);
    let legacy = s.load_model().unwrap();
    assert_eq!(legacy.lambdas, m.lambdas);
    // Saving moves them into the table.
    s.save_model(&legacy).unwrap();
    assert_eq!(rows(&mut c).len(), 2);
    assert_eq!(s.reload_model().unwrap().lambdas, m.lambdas);
    // A row that does not parse is an integrity error naming it.
    c.execute(
        "UPDATE improv.lambda SET definition = 'X = LAMBDA(x, Nope(x))' WHERE model_id = $1 AND id = 7",
        &[&mid],
    )
    .unwrap();
    let e = s.load_model().unwrap_err();
    assert!(
        matches!(&e, StoreError::Integrity(m) if m.contains("lambda 7")),
        "{e}"
    );
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
    assert!(json_to_value(ValueType::Enum, &j!(5_000_000_000u64)).is_err());
    assert_eq!(value_to_json(&Value::Number(3.0)).unwrap(), j!(3));
    assert_eq!(value_to_json(&Value::Number(1e20)).unwrap(), j!(1e20));
    assert_eq!(value_to_json(&Value::Number(0.5)).unwrap(), j!(0.5));
    assert!(json_to_value(ValueType::Boolean, &j!(1)).is_err());
    assert_eq!(
        StoreError::Mentat("m".into()).to_string(),
        "mentat error: m"
    );
}

/// The `bound_model` fact table's rows, sorted by dimensions.
type FactRow = (String, String, Option<f64>, Option<i32>);
fn fact_rows(table: &str) -> Vec<FactRow> {
    client()
        .query(&format!("SELECT * FROM {table} ORDER BY 1, 2"), &[])
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1), r.get(2), r.get(3)))
        .collect()
}

fn row(q: &str, p: &str, pr: Option<f64>, qt: Option<i32>) -> FactRow {
    (q.to_string(), p.to_string(), pr, qt)
}

/// Row versions: an unchanged xmin means the row was not written.
fn xmins(sql: &str) -> Vec<String> {
    client()
        .query(sql, &[])
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect()
}

/// The coordinate of the `bound_model` cell at quarter `q`, product `p`.
fn at(m: &Model, q: &str, p: &str) -> Coordinate {
    let id = |c: CategoryId, n: &str| {
        m.items
            .values()
            .find(|i| i.category == c && i.name.0 == n)
            .unwrap()
            .id
    };
    Coordinate::from_pairs([(T, id(T, q)), (P, id(P, p))])
}

const PRICE: MeasureId = MeasureId(100);
const QTY: MeasureId = MeasureId(101);

/// G4: an aggregated binding loads one aggregated cell per coordinate, and
/// a save that edits one is refused.
#[test]
fn aggregated_bindings_load_folded_and_are_read_only() {
    let Some(url) = live("g4_agg") else { return };
    let table = format!("g4_agg_{}", std::process::id());
    client()
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table}; \
             CREATE TABLE {table} (quarter text, product text, price float8); \
             INSERT INTO {table} VALUES ('Q1','Widget',10), ('Q1','Widget',30), ('Q2','Widget',5), \
               ('Q1','Widget',10), ('Q1','Widget',NULL), ('Q3','Widget',NULL)"
        ))
        .unwrap();
    let mut s = PgModelStore::open(&url).unwrap();
    let mut m = Model::new();
    m.add_category(T, "Time");
    m.add_category(P, "Product");
    m.add_measure(measure(100, "Price", &[T, P], MeasureKind::Input));
    m.add_measure(measure(101, "Prices", &[T, P], MeasureKind::Input));
    s.save_model(&m).unwrap();
    for (id, agg) in [(100, "sum"), (101, "count_distinct")] {
        s.client
            .execute(
                "INSERT INTO improv.binding(model_id, measure_id, rel, value_col, dim_cols, agg) \
                 VALUES ($1, $2, $3::text::regclass, 'price', \
                         '{\"Time\":\"quarter\",\"Product\":\"product\"}', $4)",
                &[&s.model_id, &i64::from(id), &table, &agg],
            )
            .unwrap();
    }
    let mut m = s.load_model().unwrap();
    let cell = |m: &Model, q| m.inputs.get(&(PRICE, at(m, q, "Widget"))).cloned();
    assert_eq!(cell(&m, "Q1"), Some(Value::Number(50.0)));
    assert_eq!(cell(&m, "Q2"), Some(Value::Number(5.0)));
    // count_distinct as count(DISTINCT): NULLs not counted, all-NULL is 0.
    let n = |m: &Model, q| m.inputs.get(&(QTY, at(m, q, "Widget"))).cloned();
    assert_eq!(n(&m, "Q1"), Some(Value::Number(2.0)));
    assert_eq!(n(&m, "Q3"), Some(Value::Number(0.0)));
    s.save_model(&m).unwrap();
    m.set_input(PRICE, at(&m, "Q1", "Widget"), Value::Number(1.0));
    let e = s.save_model(&m).unwrap_err();
    assert!(
        matches!(&e, StoreError::Integrity(msg) if msg == "aggregated measure Price is read-only"),
        "{e}"
    );
    client()
        .batch_execute(&format!("DROP TABLE {table}"))
        .unwrap();
}

/// G5: bool, text and date/timestamp columns load as their value types and a
/// GUI edit of a bool cell writes the bool column.
#[test]
fn typed_bound_columns_load_and_save() {
    let Some(url) = live("g5_types") else { return };
    let table = format!("g5_types_{}", std::process::id());
    client()
        .batch_execute(&format!(
            "DROP TABLE IF EXISTS {table}; \
             CREATE TABLE {table} (product text PRIMARY KEY, active bool, category varchar(20), \
                                   launched date, seen timestamptz); \
             INSERT INTO {table} VALUES ('Widget', true, 'tools', '2026-01-02', \
                                         '2026-03-04 05:06:07+00')"
        ))
        .unwrap();
    let mut s = PgModelStore::open(&url).unwrap();
    let mut m = Model::new();
    m.add_category(P, "Product");
    let typed = [
        (100, "Active", ValueType::Boolean, "active"),
        (101, "Category", ValueType::Text, "category"),
        (102, "Launched", ValueType::DateTime, "launched"),
        (103, "Seen", ValueType::DateTime, "seen"),
    ];
    for (id, name, vt, _) in typed {
        let mut me = measure(id, name, &[P], MeasureKind::Input);
        me.value_type = vt;
        m.add_measure(me);
    }
    s.save_model(&m).unwrap();
    for (id, _, _, col) in typed {
        s.client
            .execute(
                "INSERT INTO improv.binding(model_id, measure_id, rel, value_col, dim_cols) \
                 VALUES ($1, $2, $3::text::regclass, $4, '{\"Product\":\"product\"}')",
                &[&s.model_id, &i64::from(id), &table, &col],
            )
            .unwrap();
    }
    let mut m = s.load_model().unwrap();
    let w = Coordinate::from_pairs([(
        P,
        m.items.values().find(|i| i.name.0 == "Widget").unwrap().id,
    )]);
    let get = |m: &Model, id| m.inputs[&(MeasureId(id), w.clone())].clone();
    for (id, _, vt, _) in typed {
        assert_eq!(m.measures[&MeasureId(id)].value_type, vt);
    }
    assert_eq!(get(&m, 100), Value::Boolean(true));
    assert_eq!(get(&m, 101), Value::Text("tools".into()));
    let ts = |s: &str| {
        Value::DateTime(
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .with_timezone(&chrono::Utc),
        )
    };
    assert!(
        matches!(get(&m, 102), Value::DateTime(d) if d.date_naive().to_string() == "2026-01-02")
    );
    assert_eq!(get(&m, 103), ts("2026-03-04T05:06:07Z"));

    m.set_input(MeasureId(100), w.clone(), Value::Boolean(false));
    m.set_input(MeasureId(103), w.clone(), ts("2027-01-01T00:00:00Z"));
    s.save_model(&m).unwrap();
    let r = client()
        .query_one(
            &format!("SELECT active, category, launched::text, seen = '2027-01-01Z' FROM {table}"),
            &[],
        )
        .unwrap();
    assert!(!r.get::<_, bool>(0));
    assert_eq!(r.get::<_, String>(1), "tools");
    assert_eq!(r.get::<_, String>(2), "2026-01-02");
    assert!(r.get::<_, bool>(3));
    client()
        .batch_execute(&format!("DROP TABLE {table}"))
        .unwrap();
}

/// G2 (a): a SQL edit to a bound row survives a save that changed another
/// cell, here through a second handle carrying the loading one's snapshot.
#[test]
fn a_save_keeps_sql_edits_to_cells_it_did_not_change() {
    let Some(url) = live("g2_keep") else { return };
    let table = format!("g2_keep_{}", std::process::id());
    let mut s = bound_model(&url, &table);
    let mut m = s.load_model().unwrap();
    client()
        .execute(
            &format!("UPDATE {table} SET price = 99 WHERE quarter = 'Q2'"),
            &[],
        )
        .unwrap();
    m.set_input(PRICE, at(&m, "Q1", "Widget"), Value::Number(12.5));
    let mut h = PgModelStore::open(&url).unwrap();
    h.set_loaded_tx(s.loaded_tx());
    h.set_snapshot(s.snapshot().cloned());
    h.save_model(&m).unwrap();
    assert_eq!(
        fact_rows(&table),
        [
            row("Q1", "Gadget", Some(20.0), None),
            row("Q1", "Widget", Some(12.5), Some(100)),
            row("Q2", "Widget", Some(99.0), Some(110)),
        ]
    );
    // The snapshot is now the saved state: the next save is diffed from it.
    m.set_input(PRICE, at(&m, "Q1", "Widget"), Value::Number(13.0));
    h.save_model(&m).unwrap();
    assert_eq!(fact_rows(&table)[1].2, Some(13.0));
    client()
        .batch_execute(&format!("DROP TABLE {table}"))
        .unwrap();
}

/// G2 (b): editing a cell SQL changed since the load is a conflict, and the
/// whole save rolls back. The same for removing such a cell, and for adding
/// a cell whose row SQL has since filled.
#[test]
fn editing_a_cell_changed_in_sql_conflicts() {
    let Some(url) = live("g2_conflict") else {
        return;
    };
    let table = format!("g2_conflict_{}", std::process::id());
    let mut s = bound_model(&url, &table);
    let loaded = s.load_model().unwrap();
    let tx = s.loaded_tx();
    let snap = s.snapshot().cloned();
    let retry = |s: &mut PgModelStore, m: &Model| {
        s.set_loaded_tx(tx);
        s.set_snapshot(snap.clone());
        let e = s.save_model(m).unwrap_err();
        assert!(matches!(e, StoreError::Conflict(_)), "{e}");
        e.to_string()
    };
    client()
        .execute(
            &format!("UPDATE {table} SET qty = 5 WHERE quarter = 'Q1' AND product = 'Widget'"),
            &[],
        )
        .unwrap();
    let before = fact_rows(&table);

    // Price (written first) changes too: it must roll back with the rest.
    let mut m = loaded.clone();
    m.set_input(PRICE, at(&m, "Q2", "Widget"), Value::Number(1.0));
    m.set_input(QTY, at(&m, "Q1", "Widget"), Value::Number(7.0));
    let e = retry(&mut s, &m);
    assert!(
        e.contains("bound cell Quantity[Widget, Q1] was changed in the database"),
        "{e}"
    );
    assert!(e.contains("reload and retry"), "{e}");
    assert_eq!(fact_rows(&table), before);
    assert_eq!(s.tx_high_water().unwrap(), tx.unwrap());

    // Removing the SQL-changed cell conflicts as well.
    let mut m = loaded.clone();
    m.inputs.remove(&(QTY, at(&m, "Q1", "Widget")));
    retry(&mut s, &m);
    assert_eq!(fact_rows(&table), before);

    // An added cell whose row now holds a value: conflict; the INSERT of
    // another added cell in the same save rolls back.
    client()
        .execute(
            &format!("UPDATE {table} SET qty = 3 WHERE product = 'Gadget'"),
            &[],
        )
        .unwrap();
    let before = fact_rows(&table);
    let mut m = loaded.clone();
    m.set_input(QTY, at(&m, "Q1", "Gadget"), Value::Number(8.0));
    m.add_item(ItemId(500), T, "Q9");
    m.set_input(QTY, at(&m, "Q9", "Widget"), Value::Number(1.0));
    let e = retry(&mut s, &m);
    assert!(e.contains("Quantity[Gadget, Q1]"), "{e}");
    assert_eq!(fact_rows(&table), before);

    // After a reload the same edit goes through.
    let mut m = s.reload_model().unwrap();
    m.set_input(QTY, at(&m, "Q1", "Widget"), Value::Number(7.0));
    s.save_model(&m).unwrap();
    assert_eq!(fact_rows(&table)[1].3, Some(7));
    client()
        .batch_execute(&format!("DROP TABLE {table}"))
        .unwrap();
}

/// G2 (c): a removed bound cell NULLs its value column and keeps the row,
/// even when its item was deleted too.
#[test]
fn a_removed_bound_cell_nulls_only_its_column() {
    let Some(url) = live("g2_remove") else { return };
    let table = format!("g2_remove_{}", std::process::id());
    let mut s = bound_model(&url, &table);
    let mut m = s.load_model().unwrap();
    m.inputs.remove(&(PRICE, at(&m, "Q1", "Widget")));
    // Delete item Q2 with its cells.
    let q2 = at(&m, "Q2", "Widget").get(T).unwrap();
    m.inputs.retain(|(_, c), _| c.get(T) != Some(q2));
    m.items.remove(&q2);
    s.save_model(&m).unwrap();
    assert_eq!(
        fact_rows(&table),
        [
            row("Q1", "Gadget", Some(20.0), None),
            row("Q1", "Widget", None, Some(100)),
            row("Q2", "Widget", None, None),
        ]
    );
    assert_eq!(s.load_model().unwrap().inputs, m.inputs);
    client()
        .batch_execute(&format!("DROP TABLE {table}"))
        .unwrap();
}

/// G2 (d): an unchanged save writes no fact-table or `improv.cell` row.
#[test]
fn an_unchanged_save_writes_no_cells() {
    let Some(url) = live("g2_noop") else { return };
    let table = format!("g2_noop_{}", std::process::id());
    let mut s = bound_model(&url, &table);
    let mut m = s.load_model().unwrap();
    m.add_measure(measure(103, "Note", &[P], MeasureKind::Input));
    let w = Coordinate::from_pairs([(P, ItemId(20))]);
    m.set_input(MeasureId(103), w, Value::Number(1.0));
    s.save_model(&m).unwrap();
    let fact = format!("SELECT xmin::text FROM {table} ORDER BY quarter, product");
    let cell = format!(
        "SELECT xmin::text FROM improv.cell WHERE model_id = {} ORDER BY coord",
        s.model_id
    );
    let (f0, c0) = (xmins(&fact), xmins(&cell));
    assert_eq!((f0.len(), c0.len()), (3, 1));
    let fresh = s.load_model().unwrap();
    s.save_model(&fresh).unwrap();
    s.save_model(&m).unwrap();
    assert_eq!((xmins(&fact), xmins(&cell)), (f0, c0));
    client()
        .batch_execute(&format!("DROP TABLE {table}"))
        .unwrap();
}

/// G2 (e): rows another client added after the load survive a save (the
/// saver deletes only rows it loaded), and so does its unbound cell, while
/// what the saver itself dropped is deleted. Dropping a bound measure
/// leaves its column alone.
#[test]
fn rows_added_by_another_client_survive_a_save() {
    let Some(url) = live("g2_others") else { return };
    let table = format!("g2_others_{}", std::process::id());
    let mut s = bound_model(&url, &table);
    let mut m = s.load_model().unwrap();
    m.add_measure(measure(103, "Note", &[P], MeasureKind::Input));
    let (w, g) = (
        Coordinate::from_pairs([(P, ItemId(20))]),
        Coordinate::from_pairs([(P, ItemId(21))]),
    );
    m.set_input(MeasureId(103), w.clone(), Value::Number(1.0));
    m.set_input(MeasureId(103), g.clone(), Value::Number(2.0));
    let mut v = sample().views[&ViewId(1)].clone();
    v.measure = PRICE;
    m.add_view(v);
    s.save_model(&m).unwrap();
    let mid = s.model_id;
    client()
        .batch_execute(&format!(
            "INSERT INTO improv.category VALUES ({mid}, 9, 'Region'); \
             INSERT INTO improv.item VALUES ({mid}, 900, 9, 'East'); \
             INSERT INTO improv.measure(model_id, id, name, value_type, kind) \
                 VALUES ({mid}, 900, 'Extra', 'number', 'input'); \
             INSERT INTO improv.view VALUES ({mid}, 9, 'Theirs', \
                 (SELECT layout FROM improv.view WHERE model_id = {mid} AND id = 1)); \
             INSERT INTO improv.cell VALUES ({mid}, 900, '{{}}', '5')"
        ))
        .unwrap();
    m.inputs.remove(&(MeasureId(103), g));
    m.set_input(MeasureId(103), w, Value::Number(3.0));
    m.measures.remove(&MeasureId(102));
    m.views.clear();
    s.save_model(&m).unwrap();
    let ids = |t: &str| -> Vec<i64> {
        client()
            .query(
                &format!("SELECT id FROM improv.{t} WHERE model_id = $1 ORDER BY id"),
                &[&mid],
            )
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect()
    };
    assert_eq!(ids("category"), [1, 2, 9]);
    assert!(ids("item").contains(&900));
    assert_eq!(ids("measure"), [100, 101, 103, 900]);
    assert_eq!(ids("view"), [9]);
    let cells: Vec<(i64, String)> = client()
        .query(
            "SELECT measure_id, value::text FROM improv.cell WHERE model_id = $1 ORDER BY 1",
            &[&mid],
        )
        .unwrap()
        .iter()
        .map(|r| (r.get(0), r.get(1)))
        .collect();
    assert_eq!(cells, [(103, "3".to_string()), (900, "5".to_string())]);

    // Deleting a bound measure (and so its cells) keeps its column's values.
    let before = fact_rows(&table);
    m.inputs.retain(|(id, _), _| *id != QTY);
    m.measures.remove(&QTY);
    s.save_model(&m).unwrap();
    assert_eq!(fact_rows(&table), before);
    client()
        .batch_execute(&format!("DROP TABLE {table}"))
        .unwrap();
}

/// Without a snapshot a save writes every bound cell, unchecked (the
/// pre-G2 behaviour); a guard matching several rows is a conflict.
#[test]
fn snapshotless_saves_overwrite_and_duplicate_rows_conflict() {
    let Some(url) = live("g2_blind") else { return };
    let table = format!("g2_blind_{}", std::process::id());
    let mut s = bound_model(&url, &table);
    let mut m = s.load_model().unwrap();
    client()
        .execute(
            &format!("UPDATE {table} SET price = 99 WHERE quarter = 'Q2'"),
            &[],
        )
        .unwrap();
    m.add_item(ItemId(500), T, "Q9");
    m.set_input(PRICE, at(&m, "Q9", "Widget"), Value::Number(1.0));
    s.set_snapshot(None);
    s.save_model(&m).unwrap();
    assert_eq!(
        fact_rows(&table),
        [
            row("Q1", "Gadget", Some(20.0), None),
            row("Q1", "Widget", Some(10.0), Some(100)),
            row("Q2", "Widget", Some(11.0), Some(110)),
            row("Q9", "Widget", Some(1.0), None),
        ]
    );
    // The save took a snapshot; a duplicate row then makes the guard match
    // two rows.
    client()
        .batch_execute(&format!(
            "ALTER TABLE {table} DROP CONSTRAINT {table}_pkey; \
             INSERT INTO {table} VALUES ('Q9', 'Widget', 1, NULL)"
        ))
        .unwrap();
    m.set_input(PRICE, at(&m, "Q9", "Widget"), Value::Number(2.0));
    let e = s.save_model(&m).unwrap_err();
    assert!(e.to_string().contains("Price[Widget, Q9]"), "{e}");
    client()
        .batch_execute(&format!("DROP TABLE {table}"))
        .unwrap();
}

/// A binding's aggregate in SQL: `bool` min/max are `bool_and`/`bool_or`
/// (Postgres has no `max(bool)`), the rest by name, `count_distinct` as
/// `count(…)` (over rows the load made distinct); anything else is refused.
#[test]
fn agg_call_spells_each_aggregate() {
    use super::agg_call;
    assert_eq!(
        agg_call("max", "v", ValueType::Boolean).unwrap(),
        "bool_or(v)"
    );
    assert_eq!(
        agg_call("min", "v", ValueType::Boolean).unwrap(),
        "bool_and(v)"
    );
    assert_eq!(agg_call("max", "v", ValueType::Number).unwrap(), "max(v)");
    assert_eq!(agg_call("sum", "v", ValueType::Number).unwrap(), "sum(v)");
    assert_eq!(
        agg_call("count_distinct", "v", ValueType::Text).unwrap(),
        "count(v)"
    );
    assert!(agg_call("median", "v", ValueType::Number).is_err());
}
