//! An edit's coordinate may list its categories in any order (the server's
//! `/eval` takes them as JSON): the engine treats it as the sorted key, which
//! the proven key merge (`improv_verified::keys::union_keys`) requires.

use improv_core_model::{
    BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure, MeasureId,
    MeasureKind, Model, Name, Value, ValueType,
};
use improv_engine::session::Engine;

#[test]
fn unsorted_edit_coord_is_the_sorted_key() {
    let (time, product) = (CategoryId(1), CategoryId(2));
    let (t1, p1) = (ItemId(10), ItemId(20));
    let (price, qty, revenue) = (MeasureId(1), MeasureId(2), MeasureId(3));
    let mut m = Model::new();
    m.add_category(time, "Time");
    m.add_category(product, "Product");
    m.add_item(t1, time, "t1".to_string());
    m.add_item(p1, product, "p1".to_string());
    let measure = |id, name: &str, categories, kind| Measure {
        id,
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories,
        kind,
        description: None,
    };
    m.add_measure(measure(price, "Price", vec![product], MeasureKind::Input));
    m.add_measure(measure(qty, "Qty", vec![time, product], MeasureKind::Input));
    let mul = Expr::BinaryOp(
        BinaryOp::Mul,
        Box::new(Expr::Ref(qty, DimensionSpec::default())),
        Box::new(Expr::Ref(price, DimensionSpec::default())),
    );
    m.add_measure(measure(
        revenue,
        "Revenue",
        vec![time, product],
        MeasureKind::Derived(Formula::new(mul)),
    ));
    m.set_input(
        price,
        Coordinate::from_pairs([(product, p1)]),
        Value::Number(3.0),
    );
    let (mut engine, _) = Engine::new(&m, &[revenue]).unwrap();
    // Qty[t1, p1] = 5, its key given Product first.
    let snap = engine
        .set(qty, [(2, 20), (1, 10)].as_slice().into(), 5.0)
        .unwrap();
    let rev = snap.get(&revenue).unwrap();
    assert_eq!(
        rev.get([(1, 10), (2, 20)].as_slice())
            .and_then(|v| v.as_num()),
        Some(15.0)
    );
    assert_eq!(rev.len(), 1);
}
