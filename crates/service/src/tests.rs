//! The authority, end to end over a real store: the revenue model on disk,
//! every operation, the filters, undo per session and the change log.

use improv_client::wire::{
    CellEdit, DefineReq, EditReq, NameReq, Request, Response, UpdateMeasureReq, ViewReq, WindowReq,
    WireValue,
};
use improv_core_model::{
    BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, MatrixPlacement,
    Measure, MeasureId, MeasureKind, Model, Name, NumberFormat, Value, ValueType,
};

use crate::authority::Authority;
use crate::policy::Deny;
use crate::policy::{Action, Principal, Resource};

/// The canonical Time x Product revenue model.
pub(crate) fn revenue_model() -> Model {
    let mut m = Model::new();
    let (time, product) = (CategoryId(1), CategoryId(2));
    m.add_category(time, "Time");
    m.add_category(product, "Product");
    m.add_item(ItemId(10), time, "2025");
    m.add_item(ItemId(11), time, "2026");
    m.add_item(ItemId(20), product, "WidgetA");
    m.add_item(ItemId(21), product, "WidgetB");
    let input = |id, name: &str, cats| Measure {
        id: MeasureId(id),
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: cats,
        kind: MeasureKind::Input,
        description: None,
        format: None,
    };
    m.add_measure(input(100, "Price", vec![product]));
    m.add_measure(input(101, "Quantity", vec![time, product]));
    let mut revenue = input(102, "Revenue", vec![time, product]);
    revenue.kind = MeasureKind::Derived(Formula::new(Expr::BinaryOp(
        BinaryOp::Mul,
        Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
        Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
    )));
    m.add_measure(revenue);
    let at = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
    m.set_input(
        MeasureId(100),
        at(&[(product, ItemId(20))]),
        Value::Number(10.0),
    );
    m.set_input(
        MeasureId(100),
        at(&[(product, ItemId(21))]),
        Value::Number(20.0),
    );
    for (t, p, q) in [
        (10, 20, 100.0),
        (10, 21, 50.0),
        (11, 20, 120.0),
        (11, 21, 80.0),
    ] {
        m.set_input(
            MeasureId(101),
            at(&[(time, ItemId(t)), (product, ItemId(p))]),
            Value::Number(q),
        );
    }
    m
}

/// A fresh store path in this process's temp dir.
pub(crate) fn temp_path(tag: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!("improv-service-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(format!("{tag}-{}", N.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_file(&p);
    p.to_str().unwrap().to_owned()
}

fn seeded() -> (Authority, String) {
    let path = temp_path("m.db");
    improv_store::Store::open(&path)
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    (Authority::open(&path).unwrap(), path)
}

fn ann() -> Principal {
    Principal::user("ann")
}

fn done(a: &mut Authority, req: Request) -> improv_client::wire::Done {
    match a.serve(&ann(), "s1", req) {
        Ok(Response::Done(d)) => d,
        other => panic!("{other:?}"),
    }
}

fn refused(a: &mut Authority, req: Request) -> crate::authority::Refusal {
    a.serve(&ann(), "s1", req).expect_err("refused")
}

fn cell(a: &Authority, m: u32, at: &[(u32, u32)]) -> Option<WireValue> {
    let w = a.window(&ann(), m, &WindowReq::default()).unwrap();
    w.cells.into_iter().find(|c| c.at == at).map(|c| c.value)
}

fn set(m: u32, at: &[(u32, u32)], v: Option<f64>) -> CellEdit {
    CellEdit {
        measure: m,
        at: at.to_vec(),
        value: v.map(WireValue::Number),
    }
}

const A25: [(u32, u32); 2] = [(1, 10), (2, 20)];

#[test]
fn structure_and_windows_show_the_model() {
    let (a, _) = seeded();
    let s = a.structure(&ann());
    assert_eq!(s.categories.len(), 2);
    assert_eq!(s.categories[0].items[1].name, "2026");
    let rev = s.measure(102).unwrap();
    assert_eq!(rev.formula.as_deref(), Some("Price * Quantity"));
    assert!(!rev.editable && s.measure(101).unwrap().editable);
    assert_eq!(cell(&a, 102, &A25), Some(WireValue::Number(1000.0)));
    assert_eq!(cell(&a, 101, &A25), Some(WireValue::Number(100.0)));
    // A window: only 2026's row.
    let w = WindowReq {
        at: [(1, vec![11])].into(),
    };
    let cells = a.window(&ann(), 102, &w).unwrap().cells;
    assert_eq!(cells.len(), 2);
    assert!(cells.iter().all(|c| c.at[0] == (1, 11)));
    assert_eq!(a.window(&ann(), 999, &w).unwrap_err().status, 404);
    assert_eq!(s.version, a.version());
}

#[test]
fn an_edit_batch_recalculates_and_saves() {
    let (mut a, path) = seeded();
    let d = done(
        &mut a,
        Request::Edit(EditReq {
            edits: vec![set(101, &A25, Some(150.0)), set(100, &[(2, 21)], Some(1.0))],
        }),
    );
    assert_eq!(d.message, "2 cells set");
    assert_eq!(cell(&a, 102, &A25), Some(WireValue::Number(1500.0)));
    assert_eq!(
        cell(&a, 102, &[(1, 10), (2, 21)]),
        Some(WireValue::Number(50.0))
    );
    // In the store, for another reader.
    let stored = improv_store::Store::open(&path)
        .unwrap()
        .load_model()
        .unwrap();
    let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
    assert_eq!(
        stored.input(MeasureId(101), &at),
        Some(&Value::Number(150.0))
    );
    // Clearing a cell.
    done(
        &mut a,
        Request::Edit(EditReq {
            edits: vec![set(101, &A25, None)],
        }),
    );
    assert_eq!(cell(&a, 101, &A25), None);
    assert_eq!(cell(&a, 102, &A25), None);
}

#[test]
fn a_bad_edit_refuses_the_batch_and_names_it() {
    let (mut a, _) = seeded();
    let v = a.version();
    for (bad, status) in [
        (set(102, &A25, Some(1.0)), 400),
        (set(101, &[(1, 10)], Some(1.0)), 404),
        (set(101, &[(1, 10), (2, 99)], Some(1.0)), 404),
        (set(101, &[(1, 10), (1, 11)], Some(1.0)), 404),
        (set(999, &A25, Some(1.0)), 404),
        (
            CellEdit {
                value: Some(WireValue::Text("x".into())),
                ..set(101, &A25, None)
            },
            400,
        ),
    ] {
        let r = refused(
            &mut a,
            Request::Edit(EditReq {
                edits: vec![set(101, &A25, Some(7.0)), bad],
            }),
        );
        assert_eq!(
            (r.status, r.body.index),
            (status, Some(1)),
            "{}",
            r.body.error
        );
    }
    assert_eq!(
        cell(&a, 101, &A25),
        Some(WireValue::Number(100.0)),
        "none applied"
    );
    assert_eq!(a.version(), v);
    let many = EditReq {
        edits: vec![set(101, &A25, Some(1.0)); crate::authority::MAX_EDITS + 1],
    };
    assert_eq!(refused(&mut a, Request::Edit(many)).status, 400);
}

#[test]
fn an_outside_write_reloads_and_cell_edits_still_land() {
    let (mut a, path) = seeded();
    let mut other = improv_store::Store::open(&path).unwrap();
    let mut m = other.load_model().unwrap();
    let wb = Coordinate::from_pairs([(CategoryId(2), ItemId(21))]);
    m.set_input(MeasureId(100), wb, Value::Number(99.0));
    other.save_model(&m).unwrap();
    let before = a.loads;
    done(
        &mut a,
        Request::Edit(EditReq {
            edits: vec![set(101, &A25, Some(1.0))],
        }),
    );
    assert_eq!(a.loads, before + 1, "reloaded once");
    assert_eq!(
        cell(&a, 100, &[(2, 21)]),
        Some(WireValue::Number(99.0)),
        "theirs kept"
    );
    assert_eq!(
        cell(&a, 101, &A25),
        Some(WireValue::Number(1.0)),
        "ours landed"
    );
}

#[test]
fn formulas_define_redefine_and_locate_their_errors() {
    let (mut a, _) = seeded();
    let def = |t: &str| {
        Request::Define(DefineReq {
            definition: Some(t.into()),
            ..Default::default()
        })
    };
    let d = done(&mut a, def("Tax = Revenue * 0.1"));
    let tax = d.id.unwrap();
    assert_eq!(cell(&a, tax, &A25), Some(WireValue::Number(100.0)));
    // Redefining by name keeps the id.
    let d = done(&mut a, def("Tax = Revenue * 0.2"));
    assert_eq!(d.id, None);
    assert_eq!(cell(&a, tax, &A25), Some(WireValue::Number(200.0)));
    // A misspelled name: located after `Name = `, with a hint.
    let r = refused(&mut a, def("Bad = Revenu * 2"));
    let diag = r.body.diagnostic.expect("located");
    assert_eq!(diag.span, Some((6, 12)), "{}", r.body.error);
    assert!(diag.hint.unwrap().contains("Revenue"));
    // Through PUT, located in the formula alone.
    let r = refused(
        &mut a,
        Request::UpdateMeasure(
            tax,
            UpdateMeasureReq {
                formula: Some("Revenu + 1".into()),
                ..Default::default()
            },
        ),
    );
    assert_eq!(
        r.body.diagnostic.unwrap().span,
        Some((0, 6)),
        "{}",
        r.body.error
    );
    // A cycle is refused, and nothing changes.
    let r = refused(
        &mut a,
        Request::UpdateMeasure(
            102,
            UpdateMeasureReq {
                formula: Some(format!("Price * {}", "Tax")),
                ..Default::default()
            },
        ),
    );
    assert_eq!(r.status, 400, "{}", r.body.error);
    assert_eq!(cell(&a, 102, &A25), Some(WireValue::Number(1000.0)));
    // A LAMBDA, then a formula calling it.
    done(&mut a, def("Half = LAMBDA(x, x / 2)"));
    let d = done(&mut a, def("Mid = Half(Revenue)"));
    assert_eq!(
        cell(&a, d.id.unwrap(), &A25),
        Some(WireValue::Number(500.0))
    );
    assert_eq!(a.structure(&ann()).lambdas[0].name, "Half");
    // An input measure.
    let d = done(
        &mut a,
        Request::Define(DefineReq {
            name: Some("Notes".into()),
            categories: Some(vec![2]),
            value_type: Some(ValueType::Text),
            ..Default::default()
        }),
    );
    let s = a.structure(&ann());
    let notes = s.measure(d.id.unwrap()).unwrap();
    assert_eq!(
        (notes.categories.as_slice(), notes.value_type),
        (&[2][..], ValueType::Text)
    );
    for bad in [
        def("no equals sign"),
        def(" = 1"),
        def("Price2 = Price +"),
        Request::Define(DefineReq::default()),
        Request::Define(DefineReq {
            name: Some("Price".into()),
            ..Default::default()
        }),
        Request::Define(DefineReq {
            name: Some("X".into()),
            categories: Some(vec![9]),
            ..Default::default()
        }),
        Request::Define(DefineReq {
            name: Some("X".into()),
            categories: Some(vec![1, 1]),
            ..Default::default()
        }),
        def("Time = LAMBDA(x, x)"),
        def("Bad = LAMBDA(x"),
    ] {
        assert!(a.serve(&ann(), "s1", bad.clone()).is_err(), "{bad:?}");
    }
}

#[test]
fn measures_rename_format_and_delete() {
    let (mut a, _) = seeded();
    let put = |u| Request::UpdateMeasure(100, u);
    done(
        &mut a,
        put(UpdateMeasureReq {
            name: Some("Unit Price".into()),
            format: Some(Some(NumberFormat {
                decimals: Some(2),
                grouping: false,
            })),
            ..Default::default()
        }),
    );
    let s = a.structure(&ann());
    assert_eq!(s.measure(100).unwrap().name, "Unit Price");
    assert_eq!(
        s.measure(102).unwrap().formula.as_deref(),
        Some("'Unit Price' * Quantity")
    );
    assert_eq!(s.measure(100).unwrap().format.unwrap().decimals, Some(2));
    for bad in [
        put(UpdateMeasureReq::default()),
        put(UpdateMeasureReq {
            name: Some("Quantity".into()),
            ..Default::default()
        }),
        put(UpdateMeasureReq {
            name: Some(" ".into()),
            ..Default::default()
        }),
        Request::UpdateMeasure(999, UpdateMeasureReq::default()),
        Request::DeleteMeasure(100),
        Request::DeleteMeasure(999),
    ] {
        assert!(a.serve(&ann(), "s1", bad.clone()).is_err(), "{bad:?}");
    }
    let r = refused(&mut a, Request::DeleteMeasure(101));
    assert!(r.body.error.contains("Revenue"), "{}", r.body.error);
    done(&mut a, Request::DeleteMeasure(102));
    done(&mut a, Request::DeleteMeasure(101));
    assert!(a.structure(&ann()).measure(101).is_none());
    assert!(a.model().inputs.keys().all(|(m, _)| *m != MeasureId(101)));
}

#[test]
fn categories_and_items_add_rename_and_delete() {
    let (mut a, _) = seeded();
    let n = |s: &str| NameReq { name: s.into() };
    let region = done(&mut a, Request::AddCategory(n("Region"))).id.unwrap();
    let north = done(&mut a, Request::AddItem(region, n("North")))
        .id
        .unwrap();
    done(&mut a, Request::RenameItem(north, n("Nord")));
    done(&mut a, Request::RenameCategory(region, n("Area")));
    let s = a.structure(&ann());
    let area = s.category(region).unwrap();
    assert_eq!(
        (area.name.as_str(), area.items[0].name.as_str()),
        ("Area", "Nord")
    );
    let w26 = done(&mut a, Request::AddItem(1, n("2027"))).id.unwrap();
    for bad in [
        Request::AddCategory(n("Price")),
        Request::AddCategory(n("")),
        Request::AddItem(1, n("2025")),
        Request::AddItem(99, n("x")),
        Request::RenameItem(north, n("Nord")).clone(),
        Request::RenameItem(10, n("2026")),
        Request::RenameItem(999, n("x")),
        Request::RenameCategory(999, n("x")),
        Request::RenameCategory(1, n("Product")),
        Request::DeleteCategory(1),
        Request::DeleteCategory(999),
        Request::DeleteItem(999),
    ] {
        if matches!(&bad, Request::RenameItem(i, _) if *i == north) {
            continue;
        }
        assert!(a.serve(&ann(), "s1", bad.clone()).is_err(), "{bad:?}");
    }
    let r = refused(&mut a, Request::DeleteCategory(1));
    assert!(r.body.error.contains("Quantity"), "{}", r.body.error);
    // Deleting an item drops its cells.
    done(&mut a, Request::DeleteItem(10));
    assert_eq!(cell(&a, 101, &A25), None);
    assert_eq!(
        cell(&a, 101, &[(1, 11), (2, 20)]),
        Some(WireValue::Number(120.0))
    );
    done(&mut a, Request::DeleteItem(w26));
    done(&mut a, Request::DeleteCategory(region));
    assert!(a.structure(&ann()).category(region).is_none());
}

#[test]
fn views_save_update_and_delete() {
    let (mut a, _) = seeded();
    let mut p = MatrixPlacement::new(MeasureId(102));
    p.axis_order = vec![CategoryId(2), CategoryId(1)];
    let id = done(
        &mut a,
        Request::SaveView(ViewReq {
            name: Some("By product".into()),
            matrices: Some(vec![p.clone()]),
        }),
    )
    .id
    .unwrap();
    let s = a.structure(&ann());
    assert_eq!(s.views[0].axis_order, p.axis_order);
    done(
        &mut a,
        Request::UpdateView(
            id,
            ViewReq {
                name: Some("Products".into()),
                matrices: None,
            },
        ),
    );
    assert_eq!(a.structure(&ann()).views[0].name.0, "Products");
    for bad in [
        Request::SaveView(ViewReq::default()),
        Request::SaveView(ViewReq {
            name: Some("Products".into()),
            matrices: Some(vec![p.clone()]),
        }),
        Request::SaveView(ViewReq {
            name: Some("x".into()),
            matrices: Some(vec![]),
        }),
        Request::SaveView(ViewReq {
            name: Some("x".into()),
            matrices: Some(vec![MatrixPlacement::new(MeasureId(999))]),
        }),
        Request::UpdateView(99, ViewReq::default()),
        Request::DeleteView(99),
    ] {
        assert!(a.serve(&ann(), "s1", bad.clone()).is_err(), "{bad:?}");
    }
    done(&mut a, Request::DeleteView(id));
    assert!(a.structure(&ann()).views.is_empty());
}

#[test]
fn undo_and_redo_are_per_session() {
    let (mut a, _) = seeded();
    let edit = |v| {
        Request::Edit(EditReq {
            edits: vec![set(101, &A25, Some(v))],
        })
    };
    a.serve(&ann(), "s1", edit(1.0)).unwrap();
    a.serve(&ann(), "s2", edit(2.0)).unwrap();
    // s1 undoes its own edit: the cell goes back to what s1 replaced.
    a.serve(&ann(), "s1", Request::Undo).unwrap();
    assert_eq!(cell(&a, 101, &A25), Some(WireValue::Number(100.0)));
    a.serve(&ann(), "s1", Request::Redo).unwrap();
    assert_eq!(cell(&a, 101, &A25), Some(WireValue::Number(1.0)));
    assert!(
        a.serve(&ann(), "s1", Request::Redo).is_err(),
        "nothing to redo"
    );
    assert!(
        a.serve(&ann(), "s3", Request::Undo).is_err(),
        "nothing to undo"
    );
    // A structural step, and the cells it dropped come back.
    done(&mut a, Request::DeleteItem(11));
    assert_eq!(cell(&a, 101, &[(1, 11), (2, 20)]), None);
    done(&mut a, Request::Undo);
    assert_eq!(
        cell(&a, 101, &[(1, 11), (2, 20)]),
        Some(WireValue::Number(120.0))
    );
    assert_eq!(
        cell(&a, 102, &[(1, 11), (2, 20)]),
        Some(WireValue::Number(1200.0))
    );
    done(&mut a, Request::Redo);
    assert!(a.structure(&ann()).category(1).unwrap().items.len() == 1);
}

#[test]
fn the_change_log_resumes_and_says_reload() {
    let (mut a, _) = seeded();
    let v0 = a.version();
    assert_eq!(a.changes_since(&ann(), &v0), None, "nothing new");
    done(
        &mut a,
        Request::Edit(EditReq {
            edits: vec![set(101, &A25, Some(150.0))],
        }),
    );
    let v1 = a.version();
    assert_ne!(v0, v1);
    let c = a.changes_since(&ann(), &v0).unwrap();
    assert!(!c.reload);
    assert_eq!(c.version, v1);
    let changed: Vec<(u32, Option<WireValue>)> = c
        .cells
        .iter()
        .map(|x| (x.measure, x.value.clone()))
        .collect();
    assert!(changed.contains(&(101, Some(WireValue::Number(150.0)))));
    assert!(changed.contains(&(102, Some(WireValue::Number(1500.0)))));
    // Two edits: one event with both.
    done(
        &mut a,
        Request::Edit(EditReq {
            edits: vec![set(100, &[(2, 20)], Some(1.0))],
        }),
    );
    let c = a.changes_since(&ann(), &v0).unwrap();
    assert!(c.cells.iter().any(|x| x.measure == 100));
    assert!(c.cells.iter().any(|x| x.measure == 101));
    // A structural change, and an unknown version: reload.
    done(
        &mut a,
        Request::AddCategory(NameReq {
            name: "Region".into(),
        }),
    );
    assert!(a.changes_since(&ann(), &v0).unwrap().reload);
    assert!(a.changes_since(&ann(), "nonsense").unwrap().reload);
}

#[test]
fn a_hidden_item_measure_or_cell_is_absent_everywhere() {
    let path = temp_path("m.db");
    improv_store::Store::open(&path)
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    let hide = Deny(vec![
        ("ann".into(), Action::ReadItem, Resource::Item(ItemId(21))),
        (
            "ann".into(),
            Action::ReadMeasure,
            Resource::Measure(MeasureId(100)),
        ),
        (
            "ann".into(),
            Action::ReadCell,
            Resource::Cell(
                MeasureId(101),
                Coordinate::from_pairs([(CategoryId(1), ItemId(11)), (CategoryId(2), ItemId(20))]),
            ),
        ),
    ]);
    let mut a = Authority::with_policy(&path, Box::new(hide)).unwrap();
    let s = a.structure(&ann());
    let json = serde_json::to_string(&s).unwrap();
    assert!(
        !json.contains("WidgetB") && !json.contains("Price"),
        "{json}"
    );
    assert_eq!(s.measure(102).unwrap().formula, None, "it reads Price");
    // Bob, unrestricted, sees everything.
    let bob = Principal::user("bob");
    assert!(serde_json::to_string(&a.structure(&bob))
        .unwrap()
        .contains("WidgetB"));
    let all = a.window(&ann(), 101, &WindowReq::default()).unwrap();
    assert_eq!(all.cells.len(), 1, "2025 x WidgetA only: {all:?}");
    let asked = WindowReq {
        at: [(2, vec![21])].into(),
    };
    assert!(a.window(&ann(), 101, &asked).unwrap().cells.is_empty());
    // A hidden measure answers as a missing one, byte for byte.
    let hidden = a.window(&ann(), 100, &asked).unwrap_err();
    let missing = a.window(&ann(), 999, &asked).unwrap_err();
    assert_eq!(hidden.status, missing.status);
    assert_eq!(hidden.body.error.replace("100", "999"), missing.body.error);
    // Totals stay correct (computed over everything): Revenue[2025, A].
    assert_eq!(cell(&a, 102, &A25), Some(WireValue::Number(1000.0)));
    // An edit of a hidden cell is refused as one that does not exist.
    for at in [[(1, 11), (2, 20)], [(1, 10), (2, 21)]] {
        let r = a
            .serve(
                &ann(),
                "s",
                Request::Edit(EditReq {
                    edits: vec![set(101, &at, Some(1.0))],
                }),
            )
            .unwrap_err();
        let gone = a
            .serve(
                &ann(),
                "s",
                Request::Edit(EditReq {
                    edits: vec![set(101, &[(1, 10), (2, 99)], Some(1.0))],
                }),
            )
            .unwrap_err();
        assert_eq!((r.status, &r.body.error), (gone.status, &gone.body.error));
    }
    // Bob edits a cell ann cannot see: ann gets no event, bob does.
    let v0 = a.version();
    a.serve(
        &bob,
        "b",
        Request::Edit(EditReq {
            edits: vec![set(101, &[(1, 10), (2, 21)], Some(5.0))],
        }),
    )
    .unwrap();
    let bobs = a.changes_since(&bob, &v0).unwrap();
    assert!(bobs.cells.iter().any(|c| c.at == [(1, 10), (2, 21)]));
    assert_eq!(
        a.changes_since(&ann(), &v0),
        None,
        "nothing ann may see changed"
    );
    // An edit ann sees in part: only her part.
    let v1 = a.version();
    a.serve(
        &bob,
        "b",
        Request::Edit(EditReq {
            edits: vec![
                set(101, &A25, Some(7.0)),
                set(101, &[(1, 11), (2, 21)], Some(8.0)),
            ],
        }),
    )
    .unwrap();
    let hers = a.changes_since(&ann(), &v1).unwrap();
    assert!(
        hers.cells.iter().all(|c| !c.at.contains(&(2, 21))),
        "{hers:?}"
    );
    assert!(hers.cells.iter().any(|c| c.measure == 101 && c.at == A25));
    let refused_model = Deny(vec![("ann".into(), Action::ViewModel, Resource::Model)]);
    let mut a = Authority::with_policy(&path, Box::new(refused_model)).unwrap();
    assert_eq!(
        a.serve(&ann(), "s", Request::Structure).unwrap_err().status,
        403
    );
}

#[test]
fn the_local_service_answers_and_pushes_changes() {
    use improv_client::wire::{Event, ModelService};
    let (a, path) = seeded();
    let mut svc = crate::local::Local::new(a, ann());
    let t = svc.send(Request::Structure);
    assert!(matches!(&svc.poll()[..], [Event::Reply(n, Ok(Response::Structure(_)))] if *n == t));
    svc.send(Request::Edit(EditReq {
        edits: vec![set(101, &A25, Some(2.0))],
    }));
    let events = svc.poll();
    assert!(matches!(&events[0], Event::Reply(_, Ok(Response::Done(_)))));
    assert!(
        matches!(&events[1], Event::Changed(c) if !c.cells.is_empty()),
        "{events:?}"
    );
    assert!(svc.poll().is_empty());
    // Another program's save shows up as a reload.
    let mut other = improv_store::Store::open(&path).unwrap();
    let mut m = other.load_model().unwrap();
    m.add_category(CategoryId(9), "Region");
    other.save_model(&m).unwrap();
    assert!(matches!(&svc.poll()[..], [Event::Changed(c)] if c.reload));
}

#[test]
fn a_request_asks_the_policy_for_one_session_not_one_per_cell() {
    use crate::policy::{Access, Policy};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    struct Counting(Arc<AtomicUsize>, Arc<AtomicUsize>);
    struct Counted(Arc<AtomicUsize>);
    impl Policy for Counting {
        fn session(&self, _: &Model, _: u64, _: &Principal) -> Arc<dyn Access> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Arc::new(Counted(self.1.clone()))
        }
    }
    impl Access for Counted {
        fn allowed(&self, _: Action, _: &Resource) -> bool {
            self.0.fetch_add(1, Ordering::Relaxed);
            true
        }
        fn restricted(&self) -> bool {
            true
        }
    }
    let path = temp_path("m.db");
    improv_store::Store::open(&path)
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    let (sessions, checks) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let mut a = Authority::with_policy(&path, Box::new(Counting(sessions.clone(), checks.clone())))
        .unwrap();
    sessions.store(0, Ordering::Relaxed);
    a.serve(&ann(), "s", Request::Window(102, WindowReq::default()))
        .unwrap();
    assert_eq!(
        sessions.load(Ordering::Relaxed),
        2,
        "the model check and the window"
    );
    // Restricted: each of the 4 cells is one class lookup, plus its measure
    // and items, and the model.
    assert!(checks.load(Ordering::Relaxed) <= 16, "{checks:?}");
}

// --- Cedar (crate::cedar) ---

/// Store `text` (and `groups`) as the model's policies, as `improv policy
/// set` does, from another handle: another process, to the authority.
fn set_policies(path: &str, text: Option<&str>, groups: &str) {
    let mut s = improv_store::Store::open(path).unwrap();
    let doc = text.map(|t| {
        improv_policy::Policies::parse(t, groups).unwrap();
        (t, groups)
    });
    s.save_policies(doc).unwrap();
}

fn cedar(path: &str) -> Authority {
    let p = crate::cedar::CedarPolicy::new(path, improv_policy::Channel::Server);
    Authority::with_policy(path, Box::new(p)).unwrap()
}

/// The analyst template hides WidgetB from ann, through the authority:
/// structure, windows, edits and the change log, and totals over all data.
#[test]
fn cedar_policies_filter_every_answer() {
    let path = temp_path("m.db");
    improv_store::Store::open(&path)
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    let who = |w: &str| improv_policy::templates::Who::User(w.into());
    let r = improv_policy::templates::render;
    let text = [
        r("analyst", &who("ann"), &[], &[ItemId(21)]).unwrap(),
        r("editor", &who("ed"), &[MeasureId(101)], &[]).unwrap(),
    ]
    .join("\n");
    set_policies(&path, Some(&text), "");
    let mut a = cedar(&path);
    let s = serde_json::to_string(&a.structure(&ann())).unwrap();
    assert!(!s.contains("WidgetB") && s.contains("WidgetA"), "{s}");
    // Totals are over all data: Revenue's total over Product is not asked
    // here, but each visible cell is the full model's.
    assert_eq!(cell(&a, 102, &A25), Some(WireValue::Number(1000.0)));
    let w = a.window(&ann(), 101, &WindowReq::default()).unwrap();
    assert!(w.cells.iter().all(|c| !c.at.contains(&(2, 21))), "{w:?}");
    // Ann reads but may not edit; ed edits Quantity and nothing else.
    let edit = |m, at: &[(u32, u32)]| {
        Request::Edit(EditReq {
            edits: vec![set(m, at, Some(3.0))],
        })
    };
    let r = a.serve(&ann(), "s", edit(101, &A25)).unwrap_err();
    let gone = a
        .serve(&ann(), "s", edit(101, &[(1, 10), (2, 99)]))
        .unwrap_err();
    assert_eq!((r.status, &r.body.error), (gone.status, &gone.body.error));
    let ed = Principal::user("ed");
    a.serve(&ed, "e", edit(101, &A25)).unwrap();
    assert!(a.serve(&ed, "e", edit(100, &[(2, 20)])).is_err(), "Price");
    // Someone the policies do not name sees nothing.
    let eve = Principal::user("eve");
    assert_eq!(
        a.serve(&eve, "s", Request::Structure).unwrap_err().status,
        403
    );
}

/// Every way the policies change is seen by the authority's next request,
/// with no restart and nothing to remember: a set, a change, a change of
/// groups only, a clear, and policies that stop validating (fail closed).
#[test]
fn a_policy_change_is_seen_by_the_next_request() {
    let path = temp_path("m.db");
    improv_store::Store::open(&path)
        .unwrap()
        .save_model(&revenue_model())
        .unwrap();
    let mut a = cedar(&path);
    let sees = |a: &mut Authority, who: &str| {
        a.serve(&Principal::user(who), "s", Request::Structure)
            .is_ok()
    };
    assert!(sees(&mut a, "ann"), "no policies: open");
    let viewers = r#"permit (principal in Improv::Group::"viewers", action in Improv::Action::"Read", resource);"#;
    set_policies(&path, Some(viewers), r#"{"viewers": {"users": ["bob"]}}"#);
    assert!(!sees(&mut a, "ann") && sees(&mut a, "bob"), "set");
    set_policies(&path, Some(viewers), r#"{"viewers": {"users": ["ann"]}}"#);
    assert!(sees(&mut a, "ann") && !sees(&mut a, "bob"), "groups only");
    set_policies(&path, Some(r#"forbid (principal, action, resource);"#), "");
    assert!(!sees(&mut a, "ann"), "changed");
    set_policies(&path, None, "");
    assert!(sees(&mut a, "ann") && sees(&mut a, "bob"), "cleared");
    // Text that no longer validates (written behind the CLI's back): every
    // request is refused, not opened up.
    improv_store::Store::open(&path)
        .unwrap()
        .save_policies(Some(("permit (", "")))
        .unwrap();
    assert!(!sees(&mut a, "ann"), "fails closed");
    // A cell edit by an allowed writer does not reload the policies.
    set_policies(&path, Some("permit (principal, action, resource);"), "");
    let before = a.loads;
    a.serve(
        &ann(),
        "s",
        Request::Edit(EditReq {
            edits: vec![set(101, &A25, Some(4.0))],
        }),
    )
    .unwrap();
    assert!(sees(&mut a, "ann"));
    assert!(a.loads <= before + 1, "a policy set reloads at most once");
}

// --- Refusals, races, undo edges, views and the log (coverage of each path) ---

/// A store path holding `m`.
fn stored(m: &Model) -> String {
    let path = temp_path("m.db");
    improv_store::Store::open(&path)
        .unwrap()
        .save_model(m)
        .unwrap();
    path
}

fn edit1(m: u32, at: &[(u32, u32)], v: f64) -> Request {
    Request::Edit(EditReq {
        edits: vec![set(m, at, Some(v))],
    })
}

/// The status and the exact words of a refusal.
fn said(r: crate::authority::Refusal) -> (u16, String) {
    (r.status, r.body.error)
}

/// An open policy that, for user "racer", has another writer save the
/// model on each of its next `races` sessions. The authority asks for one
/// after its refresh and again for each check of an edit, so each race
/// lands between the authority's check and its save.
struct Racer {
    path: String,
    races: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl crate::policy::Policy for Racer {
    fn session(
        &self,
        _: &Model,
        _: u64,
        who: &Principal,
    ) -> std::sync::Arc<dyn crate::policy::Access> {
        use std::sync::atomic::Ordering;
        let left = self.races.load(Ordering::Relaxed);
        if who.user == "racer" && left > 0 {
            self.races.store(left - 1, Ordering::Relaxed);
            // Price[WidgetB] goes up by one, in another handle.
            let mut s = improv_store::Store::open(&self.path).unwrap();
            let mut m = s.load_model().unwrap();
            let at = Coordinate::from_pairs([(CategoryId(2), ItemId(21))]);
            let Some(Value::Number(v)) = m.input(MeasureId(100), &at).cloned() else {
                panic!("Price[WidgetB] is a number");
            };
            m.set_input(MeasureId(100), at, Value::Number(v + 1.0));
            s.save_model(&m).unwrap();
        }
        std::sync::Arc::new(crate::policy::Open)
    }
}

/// Every refusal names what it refuses with fixed words; a hidden thing and
/// a missing one get the same words.
#[test]
fn refusals_say_exactly_what_is_wrong() {
    let (mut a, _) = seeded();
    let n = |s: &str| NameReq { name: s.into() };
    for (req, status, words) in [
        (Request::DeleteMeasure(999), 404, "no measure 999"),
        (Request::RenameCategory(99, n("x")), 404, "no category 99"),
        (Request::DeleteCategory(99), 404, "no category 99"),
        (Request::AddItem(99, n("x")), 404, "no category 99"),
        (Request::RenameItem(999, n("x")), 404, "no item 999"),
        (Request::DeleteItem(999), 404, "no item 999"),
        (Request::UpdateView(9, ViewReq::default()), 404, "no view 9"),
        (Request::DeleteView(9), 404, "no view 9"),
        (
            Request::UpdateMeasure(999, UpdateMeasureReq::default()),
            404,
            "no measure 999",
        ),
        (Request::AddCategory(n(" ")), 400, "type a name"),
        (
            Request::AddCategory(n("Price")),
            400,
            "\"Price\" is already a category, measure or LAMBDA",
        ),
        (
            Request::RenameCategory(1, n("Product")),
            400,
            "\"Product\" is already a category, measure or LAMBDA",
        ),
        (
            Request::AddItem(1, n("2025")),
            400,
            "Time already has an item \"2025\"",
        ),
        (
            Request::RenameItem(10, n("2026")),
            400,
            "Time already has an item \"2026\"",
        ),
        (
            Request::DeleteMeasure(101),
            400,
            "cannot delete Quantity: Revenue still read it",
        ),
        (
            Request::DeleteCategory(1),
            400,
            "cannot delete Time: measures still use it (Quantity, Revenue)",
        ),
        (
            Request::SaveView(ViewReq::default()),
            400,
            "a view needs a name",
        ),
        (
            Request::SaveView(ViewReq {
                name: Some("v".into()),
                matrices: None,
            }),
            400,
            "a view needs at least one matrix",
        ),
        (
            Request::UpdateMeasure(100, UpdateMeasureReq::default()),
            400,
            "nothing to change: send formula, name or format",
        ),
        (
            Request::UpdateMeasure(
                100,
                UpdateMeasureReq {
                    name: Some(" ".into()),
                    ..Default::default()
                },
            ),
            400,
            "type a name",
        ),
        (
            Request::UpdateMeasure(
                100,
                UpdateMeasureReq {
                    name: Some("Time".into()),
                    ..Default::default()
                },
            ),
            400,
            "\"Time\" is already a category, measure or LAMBDA",
        ),
        (define("no equals sign"), 400, "type Name = formula"),
        (define(" = 1"), 400, "type Name = formula"),
        (
            Request::Define(DefineReq::default()),
            400,
            "send a definition (Name = formula) or a name",
        ),
        (
            Request::Define(DefineReq {
                name: Some("Time".into()),
                ..Default::default()
            }),
            400,
            "\"Time\" is already a category, measure or LAMBDA",
        ),
        (
            Request::Define(DefineReq {
                name: Some("X".into()),
                categories: Some(vec![9]),
                ..Default::default()
            }),
            404,
            "no category 9",
        ),
        (
            Request::Define(DefineReq {
                name: Some("X".into()),
                categories: Some(vec![1, 1]),
                ..Default::default()
            }),
            400,
            "category 1 is listed twice",
        ),
        (
            define("Time = LAMBDA(x, x)"),
            400,
            "parse error: Time is a built-in function; name the LAMBDA differently",
        ),
        (
            define("Quantity = LAMBDA(x, x)"),
            400,
            "\"Quantity\" is a measure or category; name the LAMBDA differently",
        ),
        (Request::Undo, 400, "nothing to undo"),
        (Request::Redo, 400, "nothing to redo"),
    ] {
        let r = refused(&mut a, req.clone());
        assert_eq!(said(r), (status, words.to_owned()), "{req:?}");
    }
    // A cell: hidden, missing, misplaced and unknown all read the same.
    let gone = (404, "no such cell in measure 101".to_owned());
    for at in [&[(1, 10)][..], &[(1, 10), (2, 99)], &[(1, 10), (1, 11)]] {
        let r = refused(&mut a, edit1(101, at, 1.0));
        assert_eq!(said(r), gone);
    }
    let r = refused(&mut a, edit1(102, &A25, 1.0));
    assert_eq!(
        said(r),
        (400, "Revenue is computed: edit its formula".into())
    );
    let text = Request::Edit(EditReq {
        edits: vec![CellEdit {
            value: Some(WireValue::Text("x".into())),
            ..set(101, &A25, None)
        }],
    });
    let r = refused(&mut a, text);
    assert_eq!(
        said(r),
        (400, "Quantity: this measure holds numbers".into())
    );
    let many = EditReq {
        edits: vec![set(101, &A25, Some(1.0)); crate::authority::MAX_EDITS + 1],
    };
    let r = refused(&mut a, Request::Edit(many));
    assert_eq!(
        said(r),
        (400, "10001 edits; at most 10000 in one batch".into())
    );
}

/// Every structural change needs its right (EditStructure, DefineMeasure or
/// ManageViews, on the model or on the thing changed): without it, 403 with
/// the same words. So does undoing a structural step after the right is
/// gone.
#[test]
fn structural_changes_need_their_rights() {
    let path = stored(&revenue_model());
    let deny = |a| ("ann".to_owned(), a, Resource::Model);
    let policy = Deny(vec![
        deny(Action::EditStructure),
        deny(Action::DefineMeasure),
        deny(Action::ManageViews),
        (
            "ann".into(),
            Action::DefineMeasure,
            Resource::Measure(MeasureId(102)),
        ),
        (
            "ann".into(),
            Action::EditStructure,
            Resource::Category(CategoryId(1)),
        ),
        (
            "ann".into(),
            Action::EditStructure,
            Resource::Item(ItemId(10)),
        ),
        (
            "ann".into(),
            Action::ManageViews,
            Resource::View(improv_core_model::ViewId(1)),
        ),
    ]);
    // A view to update and delete, saved by someone with the right.
    let mut a = Authority::with_policy(&path, Box::new(policy)).unwrap();
    let bob = Principal::user("bob");
    let view = Request::SaveView(ViewReq {
        name: Some("v".into()),
        matrices: Some(vec![MatrixPlacement::new(MeasureId(102))]),
    });
    let v = match a.serve(&bob, "b", view.clone()) {
        Ok(Response::Done(d)) => d.id.unwrap(),
        other => panic!("{other:?}"),
    };
    assert_eq!(v, 1);
    let n = |s: &str| NameReq { name: s.into() };
    let formula = |f: &str| UpdateMeasureReq {
        formula: Some(f.into()),
        ..Default::default()
    };
    for req in [
        Request::AddCategory(n("Region")),
        Request::RenameCategory(1, n("T")),
        Request::DeleteCategory(1),
        Request::AddItem(1, n("2027")),
        Request::RenameItem(10, n("x")),
        Request::DeleteItem(10),
        view,
        Request::UpdateView(v, ViewReq::default()),
        Request::DeleteView(v),
        Request::DeleteMeasure(102),
        Request::UpdateMeasure(102, formula("Price")),
        define("Tax = Revenue * 0.1"),
        define("Revenue = Price"),
        define("Half = LAMBDA(x, x / 2)"),
        Request::Define(DefineReq {
            name: Some("Notes".into()),
            ..Default::default()
        }),
    ] {
        let r = a.serve(&ann(), "s", req.clone()).unwrap_err();
        assert_eq!(said(r), (403, "not allowed".into()), "{req:?}");
    }
    // Undoing a structural step restores the whole structure, so it needs
    // the model-wide right: ann may delete an item (her right on it) but
    // not undo that.
    let path = stored(&revenue_model());
    let no_model = Deny(vec![("ann".into(), Action::EditStructure, Resource::Model)]);
    let mut a = Authority::with_policy(&path, Box::new(no_model)).unwrap();
    a.serve(&ann(), "s", Request::DeleteItem(11)).unwrap();
    let r = a.serve(&ann(), "s", Request::Undo).unwrap_err();
    assert_eq!(said(r), (403, "not allowed".into()));
}

/// Another writer moves the store between the authority's check and its
/// save of a cell batch: the authority reloads, checks again and retries,
/// keeping the other writer's change, up to three times; past that the
/// batch is refused as a conflict and none of it is applied. A structural
/// change that loses the race is a conflict at once.
#[test]
fn a_cell_batch_retries_a_racing_writer_three_times() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let path = stored(&revenue_model());
    let races = std::sync::Arc::new(AtomicUsize::new(0));
    let policy = Racer {
        path: path.clone(),
        races: races.clone(),
    };
    let mut a = Authority::with_policy(&path, Box::new(policy)).unwrap();
    let racer = Principal::user("racer");
    let price_b = |a: &Authority| cell(a, 100, &[(2, 21)]);
    // The races land in the sessions the authority asks for: one after its
    // refresh, then one per check of the batch. The first two beat the
    // first save, each later one the next: k races cost these reloads, and
    // the batch lands on the attempt after.
    let mut bumps = 0.0;
    for (k, reloads) in [(1, 1), (2, 1), (3, 2), (4, 3)] {
        races.store(k, Ordering::Relaxed);
        let loads = a.loads;
        let v = 1000.0 + k as f64;
        let d = match a.serve(&racer, "r", edit1(101, &A25, v)) {
            Ok(Response::Done(d)) => d,
            other => panic!("{k} races: {other:?}"),
        };
        bumps += k as f64;
        assert_eq!(d.message, "1 cell set");
        assert_eq!(a.loads, loads + reloads, "{k} races");
        assert_eq!(cell(&a, 101, &A25), Some(WireValue::Number(v)));
        assert_eq!(
            price_b(&a),
            Some(WireValue::Number(20.0 + bumps)),
            "theirs kept"
        );
    }
    // Five: three retries, then the refusal; nothing of the batch lands.
    races.store(5, Ordering::Relaxed);
    let r = a.serve(&racer, "r", edit1(101, &A25, 1.0)).unwrap_err();
    assert_eq!(r.body.index, None, "the batch, not one edit");
    assert_eq!(
        said(r),
        (
            409,
            "another writer changed the model (the store was changed by another writer \
             since it was loaded; reload and retry); reload and try again"
                .into()
        )
    );
    assert_eq!(cell(&a, 101, &A25), Some(WireValue::Number(1004.0)));
    let stored = improv_store::Store::open(&path)
        .unwrap()
        .load_model()
        .unwrap();
    let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
    assert_eq!(
        stored.input(MeasureId(101), &at),
        Some(&Value::Number(1004.0))
    );
    // The racer's undo stack holds only what landed.
    a.serve(&racer, "r", Request::Undo).unwrap();
    assert_eq!(cell(&a, 101, &A25), Some(WireValue::Number(1003.0)));
    // A structural change does not retry: the save is refused.
    races.store(1, Ordering::Relaxed);
    let add = Request::AddCategory(NameReq {
        name: "Region".into(),
    });
    let r = a.serve(&racer, "r", add.clone()).unwrap_err();
    assert_eq!(r.status, 409, "{}", r.body.error);
    assert!(a
        .structure(&racer)
        .categories
        .iter()
        .all(|c| c.name != "Region"));
    // The next request reloads and goes through.
    a.serve(&racer, "r", add).unwrap();
    assert!(a
        .structure(&racer)
        .categories
        .iter()
        .any(|c| c.name == "Region"));
}

/// A store that fails is a 500 with the store's words, never a panic: one
/// that cannot be opened, a save it refuses (the in-memory change is undone),
/// and a file damaged under a running authority. The handle is dropped and
/// the next request reopens it, so the authority recovers once the store
/// does.
#[test]
fn a_store_error_is_a_500_and_the_handle_reopens() {
    let dir = temp_path("dir");
    std::fs::create_dir_all(&dir).unwrap();
    let e = Authority::open(&dir)
        .err()
        .expect("a directory is no store");
    assert!(e.starts_with("mentat error:"), "{e}");
    std::fs::remove_dir(&dir).unwrap();

    // A save the store refuses (not a conflict): 500 with the store's words,
    // the batch undone in memory, the store handle dropped and reopened.
    let path = stored(&revenue_model());
    let mut a = Authority::open(&path).unwrap();
    let v = a.version();
    // Too many categories for one measure: the store's integrity guard.
    let cats: Vec<u32> = (1..=6000).collect();
    let mut huge = a.model().clone();
    for c in &cats[2..] {
        huge.add_category(CategoryId(*c), format!("C{c}"));
    }
    improv_store::Store::open(&path)
        .unwrap()
        .save_model(&huge)
        .unwrap();
    let r = a
        .serve(
            &ann(),
            "s",
            Request::Define(DefineReq {
                name: Some("Everything".into()),
                ..Default::default()
            }),
        )
        .unwrap_err();
    assert_eq!(r.status, 500, "{}", r.body.error);
    assert!(
        r.body.error.starts_with("data integrity: measure"),
        "{}",
        r.body.error
    );
    assert_ne!(a.version(), v, "it reloaded the outside write first");
    assert!(a
        .structure(&ann())
        .measures
        .iter()
        .all(|m| m.name != "Everything"));
    // The next request reopens the store and works.
    done(&mut a, edit1(101, &A25, 5.0));
    assert_eq!(cell(&a, 101, &A25), Some(WireValue::Number(5.0)));

    // The file overwritten under the open handle: the high-water read
    // fails, every request is a 500 (and the handle is dropped), and once
    // the file is back the next request reopens it and serves again.
    let good = std::fs::read(&path).unwrap();
    std::fs::write(&path, vec![0x5a; good.len()]).unwrap();
    for _ in 0..2 {
        let r = a.serve(&ann(), "s", Request::Structure).unwrap_err();
        assert_eq!(r.status, 500, "{}", r.body.error);
        assert!(
            r.body.error.starts_with("mentat error:"),
            "{}",
            r.body.error
        );
    }
    // (`good` is the database file without its write-ahead log: the last
    // edit need not come back with it.)
    std::fs::write(&path, &good).unwrap();
    assert!(matches!(
        a.serve(&ann(), "s", Request::Structure),
        Ok(Response::Structure(_))
    ));
    done(&mut a, edit1(101, &A25, 6.0));
    let back = improv_store::Store::open(&path)
        .unwrap()
        .load_model()
        .unwrap();
    let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
    assert_eq!(back.input(MeasureId(101), &at), Some(&Value::Number(6.0)));
}

/// Undo and redo at their edges: a new step empties redo; another
/// session's edit between is kept by a structural undo and written over by
/// a cell undo (it restores what this session replaced); an undo after
/// an outside reload still applies; an undo whose cell is gone is refused
/// with the edit's index and keeps nothing; and only the last 100 steps
/// are kept.
#[test]
fn undo_edges_reloads_other_sessions_and_depth() {
    let (mut a, path) = seeded();
    let q = |a: &Authority| cell(a, 101, &A25);
    // A new step after an undo empties redo.
    a.serve(&ann(), "s1", edit1(101, &A25, 1.0)).unwrap();
    a.serve(&ann(), "s1", Request::Undo).unwrap();
    a.serve(&ann(), "s1", edit1(101, &A25, 2.0)).unwrap();
    let r = a.serve(&ann(), "s1", Request::Redo).unwrap_err();
    assert_eq!(said(r), (400, "nothing to redo".into()));
    // Another session's edit of the same cell between: s1's undo writes
    // back what s1 replaced (100); its redo, what s1 set (2).
    a.serve(&ann(), "s2", edit1(101, &A25, 3.0)).unwrap();
    a.serve(&ann(), "s1", Request::Undo).unwrap();
    assert_eq!(q(&a), Some(WireValue::Number(100.0)));
    a.serve(&ann(), "s1", Request::Redo).unwrap();
    assert_eq!(q(&a), Some(WireValue::Number(2.0)));
    // After an outside write (a reload), the undo still applies, over the
    // reloaded model, and keeps the outside change.
    let mut other = improv_store::Store::open(&path).unwrap();
    let mut m = other.load_model().unwrap();
    let wb = Coordinate::from_pairs([(CategoryId(2), ItemId(21))]);
    m.set_input(MeasureId(100), wb, Value::Number(99.0));
    other.save_model(&m).unwrap();
    let loads = a.loads;
    a.serve(&ann(), "s1", Request::Undo).unwrap();
    assert_eq!(a.loads, loads + 1);
    assert_eq!(q(&a), Some(WireValue::Number(100.0)));
    assert_eq!(cell(&a, 100, &[(2, 21)]), Some(WireValue::Number(99.0)));
    // A cell step whose cell is gone: refused, naming the edit, and the
    // step is spent (the stack does not hand it out again).
    a.serve(
        &ann(),
        "s3",
        Request::Edit(EditReq {
            edits: vec![
                set(101, &[(1, 11), (2, 21)], Some(4.0)),
                set(101, &A25, Some(5.0)),
            ],
        }),
    )
    .unwrap();
    done(&mut a, Request::DeleteItem(10));
    let r = a.serve(&ann(), "s3", Request::Undo).unwrap_err();
    assert_eq!(r.body.index, Some(1), "{}", r.body.error);
    assert_eq!(said(r), (404, "no such cell in measure 101".into()));
    assert_eq!(
        cell(&a, 101, &[(1, 11), (2, 21)]),
        Some(WireValue::Number(4.0)),
        "none applied"
    );
    let r = a.serve(&ann(), "s3", Request::Undo).unwrap_err();
    assert_eq!(said(r), (400, "nothing to undo".into()));
    // A structural undo keeps another session's later cell edit.
    let (mut a, _) = seeded();
    a.serve(
        &ann(),
        "s1",
        Request::AddItem(
            1,
            NameReq {
                name: "2027".into(),
            },
        ),
    )
    .unwrap();
    a.serve(&ann(), "s2", edit1(101, &A25, 7.0)).unwrap();
    a.serve(&ann(), "s1", Request::Undo).unwrap();
    assert_eq!(a.structure(&ann()).category(1).unwrap().items.len(), 2);
    assert_eq!(q(&a), Some(WireValue::Number(7.0)), "s2's edit kept");
    // Depth: 101 steps keep the last 100.
    for i in 0..101 {
        a.serve(&ann(), "deep", edit1(101, &A25, f64::from(i)))
            .unwrap();
    }
    for _ in 0..100 {
        a.serve(&ann(), "deep", Request::Undo).unwrap();
    }
    assert_eq!(
        q(&a),
        Some(WireValue::Number(0.0)),
        "the first step's result"
    );
    let r = a.serve(&ann(), "deep", Request::Undo).unwrap_err();
    assert_eq!(said(r), (400, "nothing to undo".into()));
}

/// The change log: a client asking from before an outside write reloads
/// (the authority cannot say which cells it touched); one asking from after
/// it gets only the cells since; one asking from a version the log no
/// longer holds (more than 1024 changes ago) reloads too.
#[test]
fn changes_since_a_gap_or_a_reload() {
    let (mut a, path) = seeded();
    let v0 = a.version();
    assert!(!a.refresh().unwrap(), "nothing moved");
    a.serve(&ann(), "s", edit1(101, &A25, 1.0)).unwrap();
    let v1 = a.version();
    // An outside write, seen by the next refresh: logged as a reload.
    let mut other = improv_store::Store::open(&path).unwrap();
    let mut m = other.load_model().unwrap();
    let pa = Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
    m.set_input(MeasureId(100), pa, Value::Number(11.0));
    other.save_model(&m).unwrap();
    let probes = a.probes;
    assert!(a.refresh().unwrap(), "reloaded");
    assert_eq!(a.probes, probes + 1);
    let v2 = a.version();
    assert_ne!(v1, v2);
    assert_eq!(cell(&a, 100, &[(2, 20)]), Some(WireValue::Number(11.0)));
    for since in [&v0, &v1] {
        let c = a.changes_since(&ann(), since).unwrap();
        assert!(c.reload && c.cells.is_empty() && c.version == v2, "{c:?}");
    }
    assert_eq!(a.changes_since(&ann(), &v2), None);
    // From after the reload: just the cells since.
    a.serve(&ann(), "s", edit1(100, &[(2, 21)], 2.0)).unwrap();
    let c = a.changes_since(&ann(), &v2).unwrap();
    assert!(!c.reload);
    let mut got: Vec<(u32, Option<WireValue>)> = c
        .cells
        .iter()
        .map(|x| (x.measure, x.value.clone()))
        .collect();
    got.sort_by_key(|(m, _)| *m);
    assert_eq!(
        got,
        vec![
            (100, Some(WireValue::Number(2.0))),
            (102, Some(WireValue::Number(100.0))),
            (102, Some(WireValue::Number(160.0))),
        ]
    );
    // A gap: 1024 changes later the log no longer holds v3.
    let v3 = a.version();
    for i in 0..1024 {
        a.serve(&ann(), "s", edit1(101, &A25, f64::from(i)))
            .unwrap();
    }
    let c = a.changes_since(&ann(), &v3).unwrap();
    assert!(c.reload, "{} cells", c.cells.len());
    // One change later than that it still answers with cells.
    let v4 = a.version();
    a.serve(&ann(), "s", edit1(101, &A25, -1.0)).unwrap();
    assert!(!a.changes_since(&ann(), &v4).unwrap().reload);
}

/// A placement of `m` with a page item and a filter, as a GUI saves one.
fn placed(m: u32, page: (u32, u32), keep: &[u32]) -> MatrixPlacement {
    let mut p = MatrixPlacement::new(MeasureId(m));
    p.page_items = vec![(CategoryId(page.0), ItemId(page.1))];
    p.filters = vec![improv_core_model::Filter {
        category: CategoryId(2),
        items: keep.iter().copied().map(ItemId).collect(),
    }];
    p
}

/// Views: saved with several matrices, renamed (a name clash refused),
/// their matrices replaced, and shown with only what the caller sees; a
/// matrix naming a hidden measure or item is refused as a missing one.
/// Deleting a measure drops its matrices (and a view left with none);
/// deleting an item drops it from every page and filter.
#[test]
fn views_keep_what_is_visible_and_follow_deletes() {
    let path = stored(&revenue_model());
    let hide = Deny(vec![
        ("ann".into(), Action::ReadItem, Resource::Item(ItemId(21))),
        (
            "ann".into(),
            Action::ReadMeasure,
            Resource::Measure(MeasureId(100)),
        ),
    ]);
    let mut a = Authority::with_policy(&path, Box::new(hide)).unwrap();
    let bob = Principal::user("bob");
    let as_bob = |a: &mut Authority, req: Request| match a.serve(&bob, "b", req) {
        Ok(Response::Done(d)) => d,
        other => panic!("{other:?}"),
    };
    let save = |name: &str, ms: Vec<MatrixPlacement>| {
        Request::SaveView(ViewReq {
            name: Some(name.into()),
            matrices: Some(ms),
        })
    };
    let two = vec![placed(101, (1, 10), &[20, 21]), placed(100, (2, 21), &[21])];
    let v = as_bob(&mut a, save("Both", two.clone())).id.unwrap();
    let w = as_bob(
        &mut a,
        save("Price only", vec![MatrixPlacement::new(MeasureId(100))]),
    )
    .id
    .unwrap();
    // Bob sees both matrices; ann only Quantity's, without WidgetB, and
    // not the view of Price alone.
    let bobs = a.structure(&bob).views;
    assert_eq!((bobs.len(), bobs[0].matrices().len()), (2, 2));
    let hers = a.structure(&ann()).views;
    assert_eq!(hers.len(), 1, "{hers:?}");
    let ms = hers[0].matrices();
    assert_eq!(ms.len(), 1);
    assert_eq!(ms[0].measure, MeasureId(101));
    assert_eq!(ms[0].filters[0].items, vec![ItemId(20)]);
    // What ann may not see she may not name, and the words are a missing
    // thing's.
    for (ms, words) in [
        (vec![MatrixPlacement::new(MeasureId(100))], "no measure 100"),
        (vec![placed(101, (1, 10), &[21])], "no item 21"),
        (
            vec![
                MatrixPlacement::new(MeasureId(101)),
                placed(101, (2, 21), &[]),
            ],
            "no item 21",
        ),
    ] {
        let r = refused(&mut a, save("Mine", ms));
        assert_eq!(said(r), (404, words.into()));
    }
    // A view of Price alone is hidden from her, and so missing.
    let r = refused(&mut a, Request::DeleteView(w));
    assert_eq!(said(r), (404, format!("no view {w}")));
    // Renames: a clash refused, a blank name keeps the old one.
    let r = refused(
        &mut a,
        Request::UpdateView(
            v,
            ViewReq {
                name: Some("Price only".into()),
                matrices: None,
            },
        ),
    );
    assert_eq!(
        said(r),
        (400, "a view is already named \"Price only\"".into())
    );
    let r = refused(
        &mut a,
        save("Price only", vec![MatrixPlacement::new(MeasureId(101))]),
    );
    assert_eq!(
        said(r),
        (400, "a view is already named \"Price only\"".into())
    );
    let d = as_bob(
        &mut a,
        Request::UpdateView(
            v,
            ViewReq {
                name: Some(" ".into()),
                matrices: Some(vec![
                    placed(101, (1, 11), &[20, 21]),
                    placed(102, (1, 10), &[21]),
                    MatrixPlacement::new(MeasureId(100)),
                ]),
            },
        ),
    );
    assert_eq!(d.message, "saved view Both");
    // Deleting an item scrubs it from every page and filter.
    as_bob(&mut a, Request::DeleteItem(21));
    let view = &a.model().views[&improv_core_model::ViewId(v)];
    for p in view.matrices() {
        assert!(p.page_items.iter().all(|(_, i)| *i != ItemId(21)), "{p:?}");
        assert!(
            p.filters.iter().all(|f| !f.items.contains(&ItemId(21))),
            "{p:?}"
        );
    }
    as_bob(&mut a, Request::DeleteItem(11));
    let view = &a.model().views[&improv_core_model::ViewId(v)];
    assert!(view.matrices()[0].page_items.is_empty());
    // Deleting a measure drops its matrices, and a view left with none.
    as_bob(&mut a, Request::DeleteMeasure(102));
    as_bob(&mut a, Request::DeleteMeasure(100));
    let views = &a.model().views;
    assert!(
        !views.contains_key(&improv_core_model::ViewId(w)),
        "Price only is gone"
    );
    let left = views[&improv_core_model::ViewId(v)].matrices();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].measure, MeasureId(101));
}

/// The revenue model with a Region category, a SQL-filled measure, a
/// measure filled by an external function of Quantity, a scenario and a
/// formula that names Region.
fn rich_model() -> Model {
    use improv_core_model::{
        ExternalCall, ExternalFn, Language, RefreshPolicy, Scenario, SqlSource,
    };
    let mut m = revenue_model();
    m.add_category(CategoryId(3), "Region");
    m.add_item(ItemId(30), CategoryId(3), "North");
    let input = |id, name: &str, cats| Measure {
        id: MeasureId(id),
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: cats,
        kind: MeasureKind::Input,
        description: None,
        format: None,
    };
    m.add_measure(input(103, "Sales", vec![CategoryId(2)]));
    m.sql_sources.insert(
        MeasureId(103),
        SqlSource {
            query: "SELECT p, v FROM t".into(),
            dimension_columns: vec!["p".into()],
            value_column: "v".into(),
            refresh_policy: RefreshPolicy::Manual,
        },
    );
    m.add_measure(input(104, "Scored", vec![CategoryId(1), CategoryId(2)]));
    m.external_fns.insert(
        "score".into(),
        ExternalFn {
            name: "score".into(),
            language: Language::Python,
            body: "def score(q): return q".into(),
            arg_types: vec![ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        },
    );
    m.external_calls.insert(
        MeasureId(104),
        ExternalCall {
            func: "score".into(),
            arg_measures: vec![MeasureId(101)],
            refresh_policy: RefreshPolicy::Manual,
        },
    );
    let mut regions = input(105, "Regions", vec![]);
    regions.kind = MeasureKind::Derived(
        improv_core_model::parser::parse_expr(&m, "-@ITEMCOUNT(Region) + 1").unwrap(),
    );
    m.add_measure(regions);
    let a25 = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
    let a26 = Coordinate::from_pairs([(CategoryId(1), ItemId(11)), (CategoryId(2), ItemId(20))]);
    m.add_scenario(Scenario {
        id: improv_core_model::ScenarioId(1),
        name: Name("High".into()),
        overrides: [
            ((MeasureId(101), a25), Value::Number(500.0)),
            ((MeasureId(101), a26), Value::Number(600.0)),
            (
                (
                    MeasureId(100),
                    Coordinate::from_pairs([(CategoryId(2), ItemId(20))]),
                ),
                Value::Number(1.0),
            ),
        ]
        .into(),
    });
    m
}

/// Filled measures, a formula naming a category, and scenarios: what
/// each structural change does to them.
#[test]
fn sources_scenarios_and_category_users_follow_changes() {
    let path = stored(&rich_model());
    let mut a = Authority::open(&path).unwrap();
    let s = a.structure(&ann());
    for id in [103, 104, 105] {
        assert!(!s.measure(id).unwrap().editable, "{id}");
    }
    let caps = a.capabilities(&ann()).unwrap();
    for m in &caps.measures {
        let want = if [100, 101].contains(&m.id) {
            "all"
        } else {
            "none"
        };
        assert_eq!(m.cells, want, "{}", m.id);
    }
    assert_eq!(cell(&a, 105, &[]), Some(WireValue::Number(0.0)));
    // A filled measure's cells are its source's.
    for (m, at, name) in [(103, &[(2, 20)][..], "Sales"), (104, &A25[..], "Scored")] {
        let r = refused(&mut a, edit1(m, at, 1.0));
        assert_eq!(said(r), (400, format!("{name} is filled from its source")));
    }
    // An external function's argument is still read by it.
    let r = refused(&mut a, Request::DeleteMeasure(101));
    assert_eq!(
        said(r),
        (
            400,
            "cannot delete Quantity: Revenue, Scored still read it".into()
        )
    );
    // A formula naming Region keeps Region; once it is gone, Region goes.
    let r = refused(&mut a, Request::DeleteCategory(3));
    assert_eq!(
        said(r),
        (
            400,
            "cannot delete Region: measures still use it (Regions)".into()
        )
    );
    // A total over Time names Time only in its formula: it is a user too.
    done(&mut a, define("Total = SUM(Quantity OVER Time)"));
    let r = refused(&mut a, Request::DeleteCategory(1));
    assert_eq!(
        said(r),
        (
            400,
            "cannot delete Time: measures still use it (Quantity, Revenue, Scored, Total)".into()
        )
    );
    done(&mut a, Request::DeleteMeasure(106));
    // Deleting an item drops the scenario overrides at it.
    done(&mut a, Request::DeleteItem(10));
    let o = &a.model().scenarios[&improv_core_model::ScenarioId(1)].overrides;
    assert_eq!(o.len(), 2, "{o:?}");
    assert!(o
        .keys()
        .all(|(_, at)| at.dims.values().all(|i| *i != ItemId(10))));
    // Deleting a measure drops its source, call and overrides.
    done(&mut a, Request::DeleteMeasure(104));
    done(&mut a, Request::DeleteMeasure(103));
    done(&mut a, Request::DeleteMeasure(102));
    done(&mut a, Request::DeleteMeasure(101));
    let m = a.model();
    assert!(m.sql_sources.is_empty() && m.external_calls.is_empty());
    let o = &m.scenarios[&improv_core_model::ScenarioId(1)].overrides;
    assert!(o.keys().all(|(x, _)| *x == MeasureId(100)), "{o:?}");
    done(&mut a, Request::DeleteMeasure(105));
    done(&mut a, Request::DeleteCategory(3));
    // A category only a hidden measure uses: named without the measure.
    let path = stored(&rich_model());
    let hide = Deny(vec![(
        "ann".into(),
        Action::ReadMeasure,
        Resource::Measure(MeasureId(105)),
    )]);
    let mut a = Authority::with_policy(&path, Box::new(hide)).unwrap();
    let r = refused(&mut a, Request::DeleteCategory(3));
    assert_eq!(
        said(r),
        (400, "cannot delete Region: measures still use it".into())
    );
}

/// Defining and redefining: every refusal located in the text that was
/// sent, a LAMBDA redefined in place, a format only on numbers, and each
/// derived cell type on the wire.
#[test]
fn definitions_locate_errors_and_cells_keep_their_types() {
    let (mut a, _) = seeded();
    let formula = |f: &str| UpdateMeasureReq {
        formula: Some(f.into()),
        ..Default::default()
    };
    // A compile error after `Name = `: located at the measure it names.
    let r = refused(&mut a, define("Bad = Quantity[Time]"));
    let d = r.body.diagnostic.clone().expect("located");
    assert_eq!(d.span, Some((6, 14)), "{}", r.body.error);
    assert!(
        r.body.error.contains("leaves out category Product"),
        "{}",
        r.body.error
    );
    // Redefining by name: a parse error in it is located after `Name = `.
    let r = refused(&mut a, define("Revenue = Price *"));
    assert_eq!(r.body.diagnostic.unwrap().span, Some((16, 17)));
    // Through PUT: a compile error, then one that only the whole model
    // shows (a cycle), each located in the formula alone.
    let r = refused(
        &mut a,
        Request::UpdateMeasure(102, formula("Quantity[Time]")),
    );
    assert_eq!(r.body.diagnostic.unwrap().span, Some((0, 8)));
    let tax = done(&mut a, define("Tax = Revenue * 0.1")).id.unwrap();
    let r = refused(&mut a, Request::UpdateMeasure(102, formula("Tax + 1")));
    assert_eq!(r.status, 400, "{}", r.body.error);
    assert_eq!(
        r.body.diagnostic.unwrap().span,
        Some((0, 3)),
        "it names Tax"
    );
    // A formula past the depth limit.
    let deep = format!("{}1", "-".repeat(300));
    let r = refused(&mut a, Request::UpdateMeasure(102, formula(&deep)));
    assert!(
        r.body.error.contains("nests too deeply"),
        "{}",
        r.body.error
    );
    // A formula on an input makes it derived; English is understood.
    let d = done(
        &mut a,
        Request::UpdateMeasure(tax, formula("Revenue times 0.5")),
    );
    assert_eq!(d.message, "Tax: formula changed");
    assert_eq!(cell(&a, tax, &A25), Some(WireValue::Number(500.0)));
    // A LAMBDA redefined keeps its id.
    let half = done(&mut a, define("Half = LAMBDA(x, x / 2)")).id;
    let d = done(&mut a, define("Half = lambda (x, x / 4)"));
    assert_eq!((d.id, d.message.as_str()), (half, "redefined LAMBDA Half"));
    let r = refused(&mut a, define("Third = LAMBDA(x, x / 3)(1)"));
    assert!(r.body.diagnostic.is_some(), "{}", r.body.error);
    // Formats: set, cleared, refused where there are no numbers.
    let fmt = |f| UpdateMeasureReq {
        format: Some(f),
        ..Default::default()
    };
    let two = NumberFormat {
        decimals: Some(2),
        grouping: true,
    };
    done(&mut a, Request::UpdateMeasure(101, fmt(Some(two))));
    assert_eq!(a.structure(&ann()).measure(101).unwrap().format, Some(two));
    done(&mut a, Request::UpdateMeasure(101, fmt(None)));
    assert_eq!(a.structure(&ann()).measure(101).unwrap().format, None);
    let notes = done(
        &mut a,
        Request::Define(DefineReq {
            name: Some("Notes".into()),
            value_type: Some(ValueType::Text),
            ..Default::default()
        }),
    );
    let notes = notes.id.unwrap();
    assert_eq!(
        a.structure(&ann()).measure(notes).unwrap().categories,
        vec![1, 2],
        "all"
    );
    let r = refused(&mut a, Request::UpdateMeasure(notes, fmt(Some(two))));
    assert_eq!(said(r), (400, "Notes holds no numbers to format".into()));
    // Every type of derived cell, as the wire carries it.
    for (text, want) in [
        ("Big = Quantity > 100", WireValue::Bool(false)),
        ("Label = \"q\" & Quantity", WireValue::Text("q100".into())),
        (
            "When = #2025-01-31#",
            WireValue::Date {
                date: "2025-01-31T00:00:00+00:00".into(),
            },
        ),
        (
            "Oops = Quantity / 0",
            WireValue::Error {
                error: "#DIV/0!".into(),
            },
        ),
    ] {
        let id = done(&mut a, define(text)).id.unwrap();
        let at: &[(u32, u32)] = if text.starts_with("When") { &[] } else { &A25 };
        assert_eq!(cell(&a, id, at), Some(want), "{text}");
    }
}

/// A formula the DSL cannot spell is shown in English; a formula a GUI
/// disabled computes nothing; a model whose formulas no longer build is
/// still served, with each measure's error.
#[test]
fn formulas_show_as_english_disabled_or_broken() {
    use improv_core_model::DimensionSpec as D;
    let mut m = revenue_model();
    let mut x = m.measures[&MeasureId(102)].clone();
    x.id = MeasureId(103);
    x.name = Name("Odd".into());
    (x.value_type, x.categories) = (ValueType::Text, vec![]);
    // A backslash in a string: the DSL has no spelling for it.
    x.kind = MeasureKind::Derived(Formula::new(Expr::Literal(Value::Text("a\\b".into()))));
    m.add_measure(x);
    let mut off = m.measures[&MeasureId(102)].clone();
    off.id = MeasureId(104);
    off.name = Name("Off".into());
    off.description = Some("[improv:formula-disabled] for now".into());
    m.add_measure(off);
    let path = stored(&m);
    let a = Authority::open(&path).unwrap();
    let s = a.structure(&ann());
    let odd = s.measure(103).unwrap().formula.clone().unwrap();
    assert_eq!(
        odd,
        improv_nl_formula::describe_formula(
            &improv_nl_formula::NlContext::new(a.model()),
            match &a.model().measures[&MeasureId(103)].kind {
                MeasureKind::Derived(f) => f,
                MeasureKind::Input => unreachable!(),
            },
        )
    );
    assert_eq!(cell(&a, 103, &[]), Some(WireValue::Text("a\\b".into())));
    assert_eq!(cell(&a, 104, &A25), None, "disabled: no value");
    assert_eq!(cell(&a, 102, &A25), Some(WireValue::Number(1000.0)));
    // A formula that does not build: blank cells, and its error shown.
    let mut broken = revenue_model();
    if let MeasureKind::Derived(f) = &mut broken.measures.get_mut(&MeasureId(102)).unwrap().kind {
        f.expr = Expr::Ref(
            MeasureId(101),
            D {
                by: vec![CategoryId(1)],
                ..D::default()
            },
        );
    }
    let a = Authority::open(&stored(&broken)).unwrap();
    let rev = a.structure(&ann()).measure(102).unwrap().clone();
    let e = rev.error.unwrap();
    assert!(e.contains("leaves out category"), "{e}");
    assert_eq!(cell(&a, 102, &A25), None);
    assert_eq!(cell(&a, 101, &A25), Some(WireValue::Number(100.0)));
}

/// What a caller is told of a measure it may only partly use: cells
/// "some" when it may edit only part of an input's grid; a formula it
/// cannot see it may not replace; names clash with LAMBDAs as with
/// measures and categories; a window past the cap is refused, not cut.
#[test]
fn partial_rights_name_clashes_and_the_window_cap() {
    let path = stored(&revenue_model());
    let partial = Deny(vec![
        (
            "ann".into(),
            Action::EditCell,
            Resource::Cell(
                MeasureId(101),
                Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]),
            ),
        ),
        (
            "ann".into(),
            Action::ReadMeasure,
            Resource::Measure(MeasureId(100)),
        ),
    ]);
    let mut a = Authority::with_policy(&path, Box::new(partial)).unwrap();
    let caps = a.capabilities(&ann()).unwrap();
    let of = |id| caps.measures.iter().find(|m| m.id == id).unwrap();
    assert_eq!(
        (of(101).cells.as_str(), of(102).cells.as_str()),
        ("some", "none")
    );
    assert!(caps.measures.iter().all(|m| m.id != 100), "Price is hidden");
    // Revenue reads hidden Price: ann sees Revenue but not its formula,
    // and may not replace it; renaming it is fine.
    let r = refused(
        &mut a,
        Request::UpdateMeasure(
            102,
            UpdateMeasureReq {
                formula: Some("Quantity * 2".into()),
                ..Default::default()
            },
        ),
    );
    assert_eq!(said(r), (403, "not allowed".into()));
    let rename = |n: &str| {
        Request::UpdateMeasure(
            102,
            UpdateMeasureReq {
                name: Some(n.into()),
                ..Default::default()
            },
        )
    };
    done(&mut a, rename("Sales"));
    // A LAMBDA's name is taken, for a measure, a category and a rename.
    done(&mut a, define("Half = LAMBDA(x, x / 2)"));
    let clash = (
        400,
        "\"Half\" is already a category, measure or LAMBDA".to_owned(),
    );
    let n = NameReq {
        name: "Half".into(),
    };
    for req in [
        rename("Half"),
        Request::AddCategory(n.clone()),
        Request::Define(DefineReq {
            name: Some("Half".into()),
            ..Default::default()
        }),
        define("Half = Quantity * 2"),
    ] {
        assert_eq!(said(refused(&mut a, req.clone())), clash, "{req:?}");
    }
    // A category name is no LAMBDA's.
    done(
        &mut a,
        Request::AddCategory(NameReq {
            name: "Region".into(),
        }),
    );
    let r = refused(&mut a, define("Region = LAMBDA(x, x)"));
    assert_eq!(
        said(r),
        (
            400,
            "\"Region\" is a measure or category; name the LAMBDA differently".into()
        )
    );
    // A window larger than the cap: refused, saying the cap.
    let mut big = revenue_model();
    for i in 0..500 {
        big.add_item(ItemId(1000 + i), CategoryId(1), format!("T{i}"));
        big.add_item(ItemId(5000 + i), CategoryId(2), format!("P{i}"));
    }
    let a = Authority::open(&stored(&big)).unwrap();
    let r = a.window(&ann(), 101, &WindowReq::default()).unwrap_err();
    assert_eq!(
        said(r),
        (
            400,
            format!(
                "a window of 252004 cells; ask for at most {}",
                crate::authority::MAX_WINDOW
            )
        )
    );
    let row = WindowReq {
        at: [(1, vec![10])].into(),
    };
    assert_eq!(a.window(&ann(), 101, &row).unwrap().cells.len(), 2);
}

fn define(text: &str) -> Request {
    Request::Define(DefineReq {
        definition: Some(text.into()),
        ..Default::default()
    })
}

/// A formula that calls a named LAMBDA, or reads its categories' items
/// (POSITION, @ITEMCOUNT), builds: the structure reports no error for it.
/// The error is checked against the whole model, as the engine builds it.
#[test]
fn a_formula_using_a_lambda_or_a_category_has_no_error() {
    let (mut a, _) = seeded();
    done(&mut a, define("Half = LAMBDA(x, x / 2)"));
    let mid = done(&mut a, define("Mid = Half(Revenue)")).id.unwrap();
    let rank = done(&mut a, define("Rank = POSITION(Time)")).id.unwrap();
    let years = done(&mut a, define("Years = @ITEMCOUNT(Time)")).id.unwrap();
    let s = a.structure(&ann());
    for id in [mid, rank, years] {
        let x = s.measure(id).unwrap();
        assert_eq!(x.error, None, "{}", x.name);
    }
    assert_eq!(cell(&a, mid, &A25), Some(WireValue::Number(500.0)));
    assert_eq!(cell(&a, years, &[]), Some(WireValue::Number(2.0)));
}
