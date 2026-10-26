//! Coverage for the model's small helpers and the formula parser's error and
//! rarely-taken paths, through the public API.

use improv_core_model::{
    parse_definition, parse_formula, parser::scalar_func, BinaryOp, CategoryId, Coordinate,
    Definition, DimensionSpec, Expr, ExternalCall, ExternalFn, Formula, FuncId, ItemId, Language,
    Measure, MeasureId, MeasureKind, Model, Name, Scenario, ScenarioId, UnaryOp, Value, ValueError,
    ValueErrorKind, ValueType,
};

const TIME: CategoryId = CategoryId(1);
const PRICE: MeasureId = MeasureId(100);
const QTY: MeasureId = MeasureId(101);

fn measure(id: MeasureId, name: &str, kind: MeasureKind) -> Measure {
    Measure {
        id,
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: vec![TIME],
        kind,
        description: None,
        format: None,
    }
}

fn model() -> Model {
    let mut m = Model::new();
    m.add_category(TIME, "Time");
    m.add_measure(measure(PRICE, "Price", MeasureKind::Input));
    m.add_measure(measure(QTY, "Quantity", MeasureKind::Input));
    m
}

fn rhs(text: &str) -> Expr {
    parse_formula(&model(), &format!("X = {text}"))
        .unwrap_or_else(|e| panic!("{text}: {e}"))
        .formula
        .expr
}

fn err(text: &str) -> String {
    match parse_definition(&model(), text) {
        Ok(d) => panic!("{text:?} parsed: {d:?}"),
        Err(e) => e.to_string(),
    }
}

fn r(id: MeasureId) -> Expr {
    Expr::Ref(id, DimensionSpec::default())
}

#[test]
fn value_and_id_helpers() {
    let e = ValueError::new(ValueErrorKind::DivisionByZero);
    assert_eq!(e.source_measure, None);
    let from = ValueError::from_measure(ValueErrorKind::MissingInput, PRICE);
    assert_eq!(from.source_measure, Some(PRICE));
    let v: Value = from.into();
    assert!(v.is_error() && v.type_of().is_none() && v.as_number().is_none());
    assert_eq!(Value::Number(2.0).as_number(), Some(2.0));
    for (v, t) in [
        (Value::Number(1.0), ValueType::Number),
        (Value::Boolean(true), ValueType::Boolean),
        (Value::Text("t".into()), ValueType::Text),
        (
            Value::DateTime(chrono::DateTime::UNIX_EPOCH),
            ValueType::DateTime,
        ),
        (Value::Enum(1), ValueType::Enum),
    ] {
        assert_eq!(v.type_of(), Some(t));
        assert!(!v.is_error());
    }
    let n: Name = "Price".into();
    assert_eq!(n.to_string(), "Price");
    let f = ExternalFn {
        name: "f".into(),
        language: Language::Python,
        body: String::new(),
        arg_types: vec![ValueType::Number; 3],
        return_type: ValueType::Number,
        pure: true,
    };
    assert_eq!(f.arity(), 3);
}

#[test]
fn coordinates_measures_and_items() {
    let c = Coordinate::new()
        .with(TIME, ItemId(10))
        .with(CategoryId(2), ItemId(20));
    assert_eq!(c.categories().collect::<Vec<_>>(), [TIME, CategoryId(2)]);
    assert_eq!(c.get(TIME), Some(ItemId(10)));
    assert_eq!(Coordinate::default(), Coordinate::new());
    let derived = measure(
        MeasureId(9),
        "D",
        MeasureKind::Derived(Formula::new(r(PRICE))),
    );
    assert!(derived.is_derived() && !derived.is_input());
    let mut m = model();
    // Re-adding an item does not duplicate it; an item for an unknown category
    // is stored but attached to nothing.
    m.add_item(ItemId(10), TIME, "2025");
    m.add_item(ItemId(10), TIME, "2025");
    m.add_item(ItemId(99), CategoryId(77), "orphan");
    assert_eq!(m.categories[&TIME].items, [ItemId(10)]);
    assert!(m.items.contains_key(&ItemId(99)));
}

#[test]
fn dependency_closure_follows_formulas_and_external_calls() {
    let mut m = model();
    // A = Price + -Quantity ; B = CALL(f, A) ; C = SUM(B) with a literal arg.
    let a = MeasureId(1);
    let b = MeasureId(2);
    let c = MeasureId(3);
    let neg_qty = Expr::UnaryOp(UnaryOp::Neg, Box::new(r(QTY)));
    m.add_measure(measure(
        a,
        "A",
        MeasureKind::Derived(Formula::new(Expr::BinaryOp(
            BinaryOp::Add,
            Box::new(r(PRICE)),
            Box::new(neg_qty),
        ))),
    ));
    m.external_calls.insert(
        b,
        ExternalCall {
            func: "f".into(),
            arg_measures: vec![a],
            refresh_policy: Default::default(),
        },
    );
    m.add_measure(measure(
        c,
        "C",
        MeasureKind::Derived(Formula::new(Expr::Call(
            FuncId(1),
            vec![r(b), Expr::Literal(Value::Number(1.0)), r(a)],
        ))),
    ));
    let closure = m.measure_dependency_closure(&[c]);
    let mut got: Vec<_> = closure.into_iter().collect();
    got.sort();
    assert_eq!(got, [a, b, c, PRICE, QTY]);
}

#[test]
fn unknown_scenario_overlay_is_the_base_model() {
    let mut m = model();
    let coord = Coordinate::new().with(TIME, ItemId(10));
    m.set_input(PRICE, coord.clone(), Value::Number(1.0));
    m.add_scenario(Scenario {
        id: ScenarioId(1),
        name: Name("High".into()),
        overrides: [((PRICE, coord.clone()), Value::Number(2.0))].into(),
    });
    let base = m.with_scenario(ScenarioId(42));
    assert!(base.scenarios.is_empty());
    assert_eq!(base.input(PRICE, &coord), Some(&Value::Number(1.0)));
    assert_eq!(
        m.with_scenario(ScenarioId(1)).input(PRICE, &coord),
        Some(&Value::Number(2.0))
    );
}

#[test]
fn every_scalar_function_and_operator_parses() {
    for (name, id, arity) in [
        ("abs", 10, 1),
        ("ROUND", 11, 2),
        ("Floor", 12, 2),
        ("ceil", 13, 1),
        ("SQRT", 14, 1),
        ("neg", 15, 1),
        ("min2", 20, 1),
        ("MAX2", 21, 1),
        ("Floor.Math", 206, 3),
    ] {
        assert_eq!(scalar_func(name), Some((FuncId(id), arity)), "{name}");
    }
    assert_eq!(scalar_func("nope"), None);
    let bin = |op, l, rr| Expr::BinaryOp(op, Box::new(l), Box::new(rr));
    for (src, op) in [
        ("Price <= Quantity", BinaryOp::Le),
        ("Price >= Quantity", BinaryOp::Ge),
        ("Price / Quantity", BinaryOp::Div),
    ] {
        assert_eq!(rhs(src), bin(op, r(PRICE), r(QTY)), "{src}");
    }
    let t = Expr::Literal(Value::Boolean(true));
    let f = Expr::Literal(Value::Boolean(false));
    assert_eq!(
        rhs("TRUE or FALSE"),
        bin(BinaryOp::Or, t.clone(), f.clone())
    );
    assert_eq!(
        rhs("true OR false or true"),
        bin(BinaryOp::Or, bin(BinaryOp::Or, t.clone(), f), t)
    );
    assert_eq!(
        rhs("MAX2(Price, Quantity)"),
        Expr::Call(FuncId(21), vec![r(PRICE), r(QTY)])
    );
    for (agg, id) in [("AVG", 2), ("min", 3), ("Max", 4)] {
        let over = DimensionSpec {
            over: vec![TIME],
            ..DimensionSpec::default()
        };
        assert_eq!(
            rhs(&format!("{agg}(Price OVER Time)")),
            Expr::Call(FuncId(id), vec![Expr::Ref(PRICE, over)])
        );
    }
    // A function name not followed by "(" is just a (missing) measure name.
    assert!(err("X = abs + 1").contains("abs"));
    assert!(err("X = avg").contains("avg"));
    assert!(err("X = avg + 1").contains("avg"));
    assert!(err("X = _unit_cost").contains("_unit_cost"));
    assert_eq!(
        rhs("ABS(-Price)"),
        Expr::Call(
            FuncId(10),
            vec![Expr::UnaryOp(UnaryOp::Neg, Box::new(r(PRICE)))]
        )
    );
    let trailing = parse_formula(&model(), "X = Price Quantity").unwrap_err();
    assert!(trailing.to_string().contains("unexpected trailing input"));
}

#[test]
fn parse_errors_name_the_problem() {
    for (src, want) in [
        ("X = 1.2.3", "invalid number"),
        ("X = Price Quantity", "unexpected trailing input"),
        ("X = (Price", "')'"),
        ("X = ABS(Price", "')' to close function call"),
        (
            "X = MID(Price)",
            "MID takes MID(text, start_num, num_chars), got 1 argument(s)",
        ),
        ("X = ABS()", "ABS takes ABS(number), got 0 argument(s)"),
        ("X = SUM(Price Time)", "expected ')' to close function call"),
        ("X = SUM(Price OVER Time", "')' to close function call"),
        ("X = Price[Time", "']'"),
        ("X = + 1", "expected"),
        ("X[Time = 1", "unterminated '['"),
        ("1 = 2", "must start with a target"),
        ("X 2", "expected '='"),
        ("X = SQL(1)", "single quoted query string"),
        ("X = SQL(\"q\"", "')' to close SQL"),
        ("X = SQL(\"q\") 1", "trailing input after SQL"),
        ("X = CALL(1)", "function name then argument"),
        ("X = CALL(f, 1)", "argument measure name after ','"),
        ("X = CALL(f Price)", "expected ',' or ')'"),
        ("X = CALL(f, Price", "unterminated CALL"),
        ("X = CALL(f) Price", "trailing input after CALL"),
        ("X = Price )", "unexpected trailing input"),
    ] {
        let e = err(src);
        assert!(e.contains(want), "{src:?}: {e}");
    }
    // A position-less error renders without "at".
    match parse_formula(&model(), "") {
        Err(e) => assert_eq!(e.to_string(), "parse error: empty formula"),
        Ok(f) => panic!("parsed {f:?}"),
    }
}

#[test]
fn definition_forms() {
    let m = model();
    assert!(matches!(
        parse_definition(&m, "X = sql(\"select 1\")").unwrap(),
        Definition::Sql { query, .. } if query == "select 1"
    ));
    match parse_definition(&m, "X[Time] = call(f, Price, 'Quantity')").unwrap() {
        Definition::Call { func, args, .. } => {
            assert_eq!(func, "f");
            assert_eq!(args, [Name("Price".into()), Name("Quantity".into())]);
        }
        other => panic!("expected Call, got {other:?}"),
    }
    // `sql` / `call` not followed by "(" fall through to a formula.
    assert!(err("X = sql + 1").contains("sql"));
    assert!(err("X = call").contains("call"));
    match parse_definition(&m, "X = Price - 1").unwrap() {
        Definition::Formula(f) => assert_eq!(f.target, Name("X".into())),
        other => panic!("expected Formula, got {other:?}"),
    }
}
