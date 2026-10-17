//! Edge-case coverage for the CNL parser and describer: every operator
//! spelling, dimension phrase and error path, plus describe's fallbacks for
//! shapes outside the CNL surface.

use improv_core_model::{
    BinaryOp, CategoryId, DimensionSpec, Expr, Formula, FuncId, Measure, MeasureId, MeasureKind,
    Model, Name, UnaryOp, Value, ValueType,
};
use improv_nl_formula::{
    describe_formula, parse_nl_formula, NlContext, NlError, FUNC_AVERAGE, FUNC_MAX, FUNC_MIN,
    FUNC_SUM,
};

const TIME: CategoryId = CategoryId(1);
const PRODUCT: CategoryId = CategoryId(2);
const PRICE: MeasureId = MeasureId(100);
const QTY: MeasureId = MeasureId(101);

fn model() -> Model {
    let mut m = Model::new();
    m.add_category(TIME, "Time");
    m.add_category(PRODUCT, "Product");
    for (id, name) in [(PRICE, "Price"), (QTY, "Quantity")] {
        m.add_measure(Measure {
            id,
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: vec![TIME, PRODUCT],
            kind: MeasureKind::Input,
            description: None,
        });
    }
    m
}

fn parse(text: &str) -> Result<Expr, NlError> {
    let m = model();
    parse_nl_formula(&NlContext::new(&m), text).map(|f| f.expr)
}

fn r(id: MeasureId) -> Expr {
    Expr::Ref(id, DimensionSpec::default())
}

fn bin(op: BinaryOp, l: Expr, rhs: Expr) -> Expr {
    Expr::BinaryOp(op, Box::new(l), Box::new(rhs))
}

#[test]
fn every_operator_spelling_parses() {
    for (text, op) in [
        ("price plus quantity", BinaryOp::Add),
        ("price + quantity", BinaryOp::Add),
        ("price minus quantity", BinaryOp::Sub),
        ("price-quantity", BinaryOp::Sub),
        ("price times quantity", BinaryOp::Mul),
        ("price*quantity", BinaryOp::Mul),
        ("price divided by quantity", BinaryOp::Div),
        ("price / quantity", BinaryOp::Div),
    ] {
        assert_eq!(parse(text).unwrap(), bin(op, r(PRICE), r(QTY)), "{text}");
    }
}

#[test]
fn copula_parens_numbers_and_punctuation() {
    // "<subject> is ..." strips the subject; `.`/`,` are separators.
    assert_eq!(
        parse("Revenue is (Price plus 2) times Quantity.").unwrap(),
        bin(
            BinaryOp::Mul,
            bin(BinaryOp::Add, r(PRICE), Expr::Literal(Value::Number(2.0))),
            r(QTY)
        )
    );
    assert_eq!(
        parse("x equals 3").unwrap(),
        Expr::Literal(Value::Number(3.0))
    );
    // `.` is a word separator, so a decimal literal splits in two.
    assert_eq!(
        parse("price plus 2.5"),
        Err(NlError::Unexpected("5".into()))
    );
    // "divided" without "by" is not an operator: trailing junk.
    assert_eq!(
        parse("price divided quantity"),
        Err(NlError::Unexpected("divided".into()))
    );
}

#[test]
fn aggregations_and_dimension_phrases() {
    let spec = |over: Vec<CategoryId>, by: Vec<CategoryId>| DimensionSpec {
        by,
        over,
        except: vec![],
    };
    for (word, f) in [
        ("sum", FUNC_SUM),
        ("average", FUNC_AVERAGE),
        ("min", FUNC_MIN),
        ("max", FUNC_MAX),
    ] {
        let want = Expr::Call(f, vec![Expr::Ref(PRICE, spec(vec![TIME], vec![]))]);
        assert_eq!(
            parse(&format!("the {word} of price over time")).unwrap(),
            want
        );
        assert_eq!(parse(&format!("{word} of price over time")).unwrap(), want);
    }
    assert_eq!(
        parse("price by time and product").unwrap(),
        Expr::Ref(PRICE, spec(vec![], vec![TIME, PRODUCT]))
    );
    assert_eq!(
        parse("price over product for each time").unwrap(),
        Expr::Ref(PRICE, spec(vec![PRODUCT], vec![TIME]))
    );
    // "for" not followed by "each" ends the ref (then is trailing junk).
    assert_eq!(
        parse("price for time"),
        Err(NlError::Unexpected("for".into()))
    );
}

#[test]
fn parse_errors_are_specific() {
    for (text, want) in [
        ("", NlError::Empty),
        ("   ", NlError::Empty),
        ("price plus", NlError::UnexpectedEof),
        ("(price plus quantity", NlError::Unexpected("<eof>".into())),
        ("(price quantity)", NlError::Unexpected("quantity".into())),
        ("sum price", NlError::Unexpected("price".into())),
        ("sum of", NlError::UnexpectedEof),
        ("sum of over", NlError::Unexpected("over".into())),
        ("cost plus price", NlError::UnknownMeasure("cost".into())),
        ("price over", NlError::UnexpectedEof),
        (
            "price over region",
            NlError::UnknownCategory("region".into()),
        ),
        ("price over the", NlError::Unexpected("the".into())),
        ("price over time and", NlError::UnexpectedEof),
        ("the price", NlError::Unexpected("the".into())),
    ] {
        assert_eq!(parse(text), Err(want), "{text:?}");
    }
    assert_eq!(NlError::Empty.to_string(), "empty input");
}

#[test]
fn describe_renders_every_shape() {
    let m = model();
    let ctx = NlContext::new(&m);
    let d = |e: Expr| describe_formula(&ctx, &Formula::new(e));
    let words = [
        (BinaryOp::Add, "plus"),
        (BinaryOp::Sub, "minus"),
        (BinaryOp::Mul, "times"),
        (BinaryOp::Div, "divided by"),
        (BinaryOp::And, "and"),
        (BinaryOp::Or, "or"),
        (BinaryOp::Eq, "equals"),
        (BinaryOp::Ne, "does not equal"),
        (BinaryOp::Lt, "is less than"),
        (BinaryOp::Le, "is at most"),
        (BinaryOp::Gt, "is greater than"),
        (BinaryOp::Ge, "is at least"),
    ];
    for (op, word) in words {
        assert_eq!(
            d(bin(op, r(PRICE), r(QTY))),
            format!("Price {word} Quantity")
        );
    }
    assert_eq!(
        d(Expr::UnaryOp(UnaryOp::Neg, Box::new(r(PRICE)))),
        "negative Price"
    );
    assert_eq!(d(Expr::Literal(Value::Boolean(true))), "Boolean(true)");
    assert_eq!(d(Expr::Literal(Value::Number(2.5))), "2.5");
    assert_eq!(d(Expr::Literal(Value::Number(-3.0))), "-3");
    for (f, word) in [
        (FUNC_SUM, "sum"),
        (FUNC_AVERAGE, "average"),
        (FUNC_MIN, "min"),
        (FUNC_MAX, "max"),
    ] {
        assert_eq!(
            d(Expr::Call(f, vec![r(PRICE)])),
            format!("the {word} of Price")
        );
    }
    assert_eq!(d(Expr::Literal(Value::Number(f64::INFINITY))), "inf");
    assert_eq!(
        d(Expr::Call(FuncId(999), vec![r(PRICE), r(QTY)])),
        "function 999(Price, Quantity)"
    );
    // A registry function renders by name.
    assert_eq!(
        d(Expr::Call(
            FuncId(11),
            vec![r(PRICE), Expr::Literal(Value::Number(2.0))]
        )),
        "ROUND(Price, 2)"
    );
    // Unknown ids fall back to numeric names.
    let ghost = DimensionSpec {
        by: vec![CategoryId(9)],
        over: vec![CategoryId(8)],
        except: vec![],
    };
    assert_eq!(
        d(Expr::Ref(MeasureId(7), ghost)),
        "measure 7 over category 8 for each category 9"
    );
}
