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
    Principal("ann".into())
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
                vec![(CategoryId(1), ItemId(11)), (CategoryId(2), ItemId(20))],
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
    let bob = Principal("bob".into());
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
