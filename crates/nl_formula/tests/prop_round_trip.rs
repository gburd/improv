//! Property test for the CNL round trip: `parse(describe(f)) == f`.
//!
//! The generator produces ONLY shapes the controlled grammar supports and
//! round-trips exactly:
//!   * measure refs with `over` / `by` dimension phrases (no `except`, which
//!     the surface never emits),
//!   * single-level SUM/AVG/MIN/MAX aggregation of one measure ref,
//!   * arithmetic (+ - * /) built with the SAME precedence and associativity
//!     the parser uses, so the flat (paren-free) description re-parses to the
//!     identical tree.
//!
//! Fixture model: Time x Product x Region with three measures. Category / item
//! names are single words with no keyword collisions, so tokenization is exact.

use improv_core_model::{
    BinaryOp, CategoryId, DimensionSpec, Expr, Formula, Measure, MeasureId, MeasureKind, Model,
    Name, ValueType,
};
use improv_nl_formula::{
    describe_formula, parse_nl_formula, NlContext, FUNC_AVERAGE, FUNC_MAX, FUNC_MIN, FUNC_SUM,
};
use proptest::prelude::*;

const TIME: CategoryId = CategoryId(1);
const PRODUCT: CategoryId = CategoryId(2);
const REGION: CategoryId = CategoryId(3);
const PRICE: MeasureId = MeasureId(100);
const QUANTITY: MeasureId = MeasureId(101);
const REVENUE: MeasureId = MeasureId(102);

fn fixture() -> Model {
    let mut m = Model::new();
    m.add_category(TIME, "Time");
    m.add_category(PRODUCT, "Product");
    m.add_category(REGION, "Region");
    for (id, name, cats) in [
        (PRICE, "Price", vec![PRODUCT]),
        (QUANTITY, "Quantity", vec![TIME, PRODUCT]),
        (REVENUE, "Revenue", vec![TIME, PRODUCT, REGION]),
    ] {
        m.add_measure(Measure {
            id,
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: cats,
            kind: MeasureKind::Input,
            description: None,
        });
    }
    m
}

/// A single measure ref with optional (over, by) dimension phrases.
/// `over` and `by` category sets are disjoint so the describe/parse pair is
/// unambiguous; each is a subset of the fixture categories in canonical order.
fn arb_ref() -> impl Strategy<Value = Expr> {
    let mids = prop::sample::select(vec![PRICE, QUANTITY, REVENUE]);
    // Choose a subset of {Time, Product, Region} for `over`, and a disjoint
    // subset for `by`. describe emits categories in the order they appear in
    // the Vec, and parse reads them back in the same order, so any order works.
    let cats = vec![TIME, PRODUCT, REGION];
    (
        mids,
        prop::sample::subsequence(cats.clone(), 0..=cats.len()),
    )
        .prop_flat_map(move |(mid, over)| {
            let remaining: Vec<CategoryId> = vec![TIME, PRODUCT, REGION]
                .into_iter()
                .filter(|c| !over.contains(c))
                .collect();
            let rlen = remaining.len();
            (
                Just(mid),
                Just(over),
                prop::sample::subsequence(remaining, 0..=rlen),
            )
                .prop_map(|(mid, over, by)| {
                    Expr::Ref(
                        mid,
                        DimensionSpec {
                            over,
                            by,
                            except: vec![],
                        },
                    )
                })
        })
}

/// An aggregation call over a single measure ref.
fn arb_agg() -> impl Strategy<Value = Expr> {
    let funcs = prop::sample::select(vec![FUNC_SUM, FUNC_AVERAGE, FUNC_MIN, FUNC_MAX]);
    (funcs, arb_ref()).prop_map(|(f, r)| Expr::Call(f, vec![r]))
}

/// A "factor": a ref or an aggregation (the parser's `factor`, minus literals
/// and parens which are outside this round-trip's scope).
fn arb_factor() -> impl Strategy<Value = Expr> {
    prop_oneof![arb_ref(), arb_agg()]
}

/// A left-associative arithmetic expression matching the parser's grammar:
/// term = factor { (* | /) factor }, expr = term { (+ | -) term }.
/// Building the tree left-assoc with correct precedence guarantees the flat,
/// paren-free description re-parses to the identical tree.
fn arb_expr() -> impl Strategy<Value = Expr> {
    // A term is a non-empty run of factors joined by * / (left-assoc).
    // Boxed so it is Clone-able and can be reused on both sides of the expr.
    let term = (
        arb_factor(),
        prop::collection::vec(
            (
                prop::sample::select(vec![BinaryOp::Mul, BinaryOp::Div]),
                arb_factor(),
            ),
            0..3,
        ),
    )
        .prop_map(|(first, rest)| {
            rest.into_iter().fold(first, |acc, (op, rhs)| {
                Expr::BinaryOp(op, Box::new(acc), Box::new(rhs))
            })
        })
        .boxed();
    // An expr is a non-empty run of terms joined by + - (left-assoc).
    (
        term.clone(),
        prop::collection::vec(
            (
                prop::sample::select(vec![BinaryOp::Add, BinaryOp::Sub]),
                term,
            ),
            0..3,
        ),
    )
        .prop_map(|(first, rest)| {
            rest.into_iter().fold(first, |acc, (op, rhs)| {
                Expr::BinaryOp(op, Box::new(acc), Box::new(rhs))
            })
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// parse(describe(f)) reproduces f exactly for supported shapes.
    #[test]
    fn cnl_round_trip(expr in arb_expr()) {
        let m = fixture();
        let ctx = NlContext::new(&m);
        let f = Formula::new(expr.clone());
        let text = describe_formula(&ctx, &f);
        let reparsed = parse_nl_formula(&ctx, &text)
            .unwrap_or_else(|e| panic!("reparse of {text:?} failed: {e}"));
        prop_assert_eq!(reparsed.expr, expr, "round trip via {:?}", text);
    }
}
