//! A pure scalar evaluator: one formula, one value per input, no dimensions.
//!
//! This is the engine's per-cell semantics without the dataflow around it. It
//! calls the very operators the dataflow builder maps over its collections
//! (`apply_binary`, `apply_unary`, `apply_scalar`), so a formula evaluated
//! here gives the bit-identical value the engine gives at a single coordinate.
//! `pg_improv`'s `LANGUAGE improv` evaluates SQL function bodies with it.
//!
//! Semantics:
//! * a `Ref` to a measure absent from `env` is a missing input: the result is
//!   `None` wherever it depends on it (the dataflow emits no cell there);
//! * division by zero and domain errors are `NaN` (the engine's convention);
//! * aggregations and dimension projections (`SUM(x OVER T)`, `x BY T`) need a
//!   model with categories and are rejected with [`ScalarError::NeedsDimensions`].

use crate::compiler::{is_aggregation, scalar_arity};
use crate::dataflow::{apply_binary, apply_scalar, apply_unary};
use crate::CellValue;
use improv_core_model::{Expr, FuncId, MeasureId};
use std::collections::HashMap;

/// Why a formula cannot be evaluated as a scalar.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScalarError {
    /// An aggregation or a dimension projection: these range over categories,
    /// and a scalar evaluation has none.
    #[error(
        "{0} needs dimensions; aggregate over categories in an Improv model, not a scalar formula"
    )]
    NeedsDimensions(String),
    /// A function id that is neither a scalar built-in nor an aggregation.
    #[error("unknown function {0:?}")]
    UnknownFunction(FuncId),
    /// A scalar built-in called with the wrong number of arguments.
    #[error("function {func:?} takes {want} argument(s), got {got}")]
    Arity {
        func: FuncId,
        want: usize,
        got: usize,
    },
}

/// Evaluate `expr` with each referenced measure's value taken from `env`.
///
/// `Ok(None)` means a referenced input is missing (SQL NULL in, NULL out).
pub fn eval(
    expr: &Expr,
    env: &HashMap<MeasureId, CellValue>,
) -> Result<Option<CellValue>, ScalarError> {
    check(expr)?;
    Ok(eval_checked(expr, env))
}

/// Reject what a scalar evaluation cannot do, before evaluating anything, so
/// the error does not depend on which inputs happen to be NULL.
pub fn check(expr: &Expr) -> Result<(), ScalarError> {
    match expr {
        Expr::Literal(_) => Ok(()),
        Expr::Ref(_, spec) => {
            if spec.by.is_empty() && spec.over.is_empty() && spec.except.is_empty() {
                Ok(())
            } else {
                Err(ScalarError::NeedsDimensions(
                    "a dimension projection (BY/OVER/EXCEPT)".into(),
                ))
            }
        }
        Expr::UnaryOp(_, a) => check(a),
        Expr::BinaryOp(_, a, b) => check(a).and_then(|()| check(b)),
        Expr::Call(func, args) => {
            if is_aggregation(*func) {
                return Err(ScalarError::NeedsDimensions(
                    "an aggregation (SUM/AVG/MIN/MAX)".into(),
                ));
            }
            let want = scalar_arity(*func).ok_or(ScalarError::UnknownFunction(*func))?;
            if args.len() != want {
                return Err(ScalarError::Arity {
                    func: *func,
                    want,
                    got: args.len(),
                });
            }
            args.iter().try_for_each(check)
        }
    }
}

fn eval_checked(expr: &Expr, env: &HashMap<MeasureId, CellValue>) -> Option<CellValue> {
    match expr {
        Expr::Literal(v) => CellValue::from_model_value(v),
        Expr::Ref(m, _) => env.get(m).cloned(),
        Expr::UnaryOp(op, a) => Some(apply_unary(*op, &eval_checked(a, env)?)),
        Expr::BinaryOp(op, a, b) => {
            let (a, b) = (eval_checked(a, env)?, eval_checked(b, env)?);
            Some(apply_binary(*op, &a, &b))
        }
        Expr::Call(func, args) => {
            let nums = args
                .iter()
                .map(|a| Some(eval_checked(a, env)?.as_num().unwrap_or(f64::NAN)))
                .collect::<Option<Vec<f64>>>()?;
            Some(CellValue::num(apply_scalar(*func, &nums)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{
        BinaryOp, CategoryId, DimensionSpec, Formula, Measure, MeasureKind, Model, Name, Value,
        ValueType,
    };
    use proptest::prelude::*;

    const A: MeasureId = MeasureId(1);
    const B: MeasureId = MeasureId(2);

    fn r(m: MeasureId) -> Expr {
        Expr::Ref(m, DimensionSpec::default())
    }
    fn n(x: f64) -> Expr {
        Expr::Literal(Value::Number(x))
    }
    fn t(s: &str) -> Expr {
        Expr::Literal(Value::Text(s.into()))
    }
    fn bin(op: BinaryOp, a: Expr, b: Expr) -> Expr {
        Expr::BinaryOp(op, Box::new(a), Box::new(b))
    }
    fn call(f: u32, args: Vec<Expr>) -> Expr {
        Expr::Call(FuncId(f), args)
    }
    fn env(a: f64, b: f64) -> HashMap<MeasureId, CellValue> {
        HashMap::from([(A, CellValue::num(a)), (B, CellValue::num(b))])
    }
    fn num(e: &Expr, a: f64, b: f64) -> f64 {
        eval(e, &env(a, b)).unwrap().unwrap().as_num().unwrap()
    }
    fn boolean(e: &Expr) -> bool {
        match eval(e, &env(2.0, 3.0)).unwrap().unwrap() {
            CellValue::Bool(b) => b,
            other => panic!("not a bool: {other:?}"),
        }
    }

    #[test]
    fn arithmetic() {
        assert_eq!(num(&bin(BinaryOp::Add, r(A), r(B)), 2.0, 3.0), 5.0);
        assert_eq!(num(&bin(BinaryOp::Sub, r(A), r(B)), 2.0, 3.0), -1.0);
        assert_eq!(num(&bin(BinaryOp::Mul, r(A), r(B)), 2.0, 3.0), 6.0);
        assert_eq!(num(&bin(BinaryOp::Div, r(A), r(B)), 3.0, 2.0), 1.5);
        let neg = Expr::UnaryOp(improv_core_model::UnaryOp::Neg, Box::new(r(A)));
        assert_eq!(num(&neg, 2.0, 0.0), -2.0);
    }

    #[test]
    fn division_by_zero_is_nan() {
        // Mirrors the dataflow: x / 0 is NaN (including 0 / 0), never ±inf.
        assert!(num(&bin(BinaryOp::Div, r(A), r(B)), 1.0, 0.0).is_nan());
        assert!(num(&bin(BinaryOp::Div, r(A), r(B)), 0.0, 0.0).is_nan());
        assert!(num(&bin(BinaryOp::Div, n(-1.0), n(0.0)), 0.0, 0.0).is_nan());
    }

    #[test]
    fn comparisons_and_logic() {
        assert!(boolean(&bin(BinaryOp::Lt, r(A), r(B))));
        assert!(boolean(&bin(BinaryOp::Le, r(A), r(A))));
        assert!(!boolean(&bin(BinaryOp::Gt, r(A), r(B))));
        assert!(boolean(&bin(BinaryOp::Ge, r(B), r(A))));
        assert!(boolean(&bin(BinaryOp::Eq, r(A), n(2.0))));
        assert!(boolean(&bin(BinaryOp::Ne, r(A), r(B))));
        assert!(boolean(&bin(BinaryOp::And, r(A), r(B))));
        assert!(!boolean(&bin(BinaryOp::And, r(A), n(0.0))));
        assert!(boolean(&bin(BinaryOp::Or, n(0.0), r(B))));
        let not = Expr::UnaryOp(improv_core_model::UnaryOp::Not, Box::new(n(0.0)));
        assert!(boolean(&not));
    }

    #[test]
    fn text_equality() {
        let e = HashMap::from([(A, CellValue::Text("x".into()))]);
        let eq = |rhs: &str| eval(&bin(BinaryOp::Eq, r(A), t(rhs)), &e).unwrap().unwrap();
        assert_eq!(eq("x"), CellValue::Bool(true));
        assert_eq!(eq("y"), CellValue::Bool(false));
    }

    #[test]
    fn scalar_builtins() {
        for (id, x, want) in [
            (10, -2.5, 2.5),
            (11, 2.5, 3.0),
            (12, 2.5, 2.0),
            (13, 2.1, 3.0),
            (14, 9.0, 3.0),
            (15, 4.0, -4.0),
        ] {
            assert_eq!(num(&call(id, vec![r(A)]), x, 0.0), want, "func {id}");
        }
        assert!(num(&call(14, vec![r(A)]), -1.0, 0.0).is_nan());
        assert_eq!(num(&call(20, vec![r(A), r(B)]), 1.0, 2.0), 1.0);
        assert_eq!(num(&call(21, vec![r(A), r(B)]), 1.0, 2.0), 2.0);
    }

    #[test]
    fn missing_input_propagates() {
        let only_a = HashMap::from([(A, CellValue::num(1.0))]);
        for e in [
            bin(BinaryOp::Add, r(A), r(B)),
            call(21, vec![r(A), r(B)]),
            Expr::UnaryOp(improv_core_model::UnaryOp::Neg, Box::new(r(B))),
            bin(BinaryOp::Or, n(1.0), r(B)), // no short-circuit, like the dataflow
        ] {
            assert_eq!(eval(&e, &only_a), Ok(None), "{e:?}");
        }
        assert_eq!(
            eval(&bin(BinaryOp::Add, r(A), n(1.0)), &only_a),
            Ok(Some(CellValue::num(2.0)))
        );
    }

    #[test]
    fn rejects_what_needs_dimensions() {
        let none = HashMap::new();
        for f in 1..=4 {
            assert!(matches!(
                eval(&call(f, vec![r(A)]), &none),
                Err(ScalarError::NeedsDimensions(_))
            ));
        }
        let cats = vec![CategoryId(1)];
        for spec in [
            DimensionSpec {
                by: cats.clone(),
                ..DimensionSpec::default()
            },
            DimensionSpec {
                over: cats.clone(),
                ..DimensionSpec::default()
            },
            DimensionSpec {
                except: cats.clone(),
                ..DimensionSpec::default()
            },
        ] {
            assert!(matches!(
                eval(&Expr::Ref(A, spec), &none),
                Err(ScalarError::NeedsDimensions(_))
            ));
        }
        // Rejected even when the inputs are missing (no NULL-dependent errors).
        let nested = bin(BinaryOp::Add, r(B), call(1, vec![r(A)]));
        assert!(eval(&nested, &none).is_err());
        assert_eq!(
            eval(&call(99, vec![]), &none),
            Err(ScalarError::UnknownFunction(FuncId(99)))
        );
        assert!(matches!(
            eval(&call(20, vec![r(A)]), &none),
            Err(ScalarError::Arity {
                want: 2,
                got: 1,
                ..
            })
        ));
    }

    /// Run `expr` through the dataflow engine on a 0-dimension model with
    /// inputs A and B (absent when `None`).
    fn via_dataflow(expr: &Expr, a: Option<f64>, b: Option<f64>) -> Option<CellValue> {
        let mut m = Model::new();
        let measure = |id, name: &str, kind| Measure {
            id,
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind,
            description: None,
        };
        m.add_measure(measure(A, "A", MeasureKind::Input));
        m.add_measure(measure(B, "B", MeasureKind::Input));
        let target = MeasureId(3);
        let derived = MeasureKind::Derived(Formula::new(expr.clone()));
        m.add_measure(measure(target, "R", derived));
        for (id, v) in [(A, a), (B, b)] {
            if let Some(v) = v {
                m.set_input(id, Default::default(), Value::Number(v));
            }
        }
        let out = crate::dataflow::evaluate(&m, &[target]).expect("dataflow evaluate");
        out.get(&target)
            .and_then(|cells| cells.get(&Vec::new()).cloned())
    }

    fn num_expr() -> impl Strategy<Value = Expr> {
        let leaf = prop_oneof![
            Just(r(A)),
            Just(r(B)),
            (-3i8..=3).prop_map(|x| n(f64::from(x) / 2.0)),
        ];
        leaf.prop_recursive(3, 12, 2, |inner| {
            let op = prop_oneof![
                Just(BinaryOp::Add),
                Just(BinaryOp::Sub),
                Just(BinaryOp::Mul),
                Just(BinaryOp::Div),
            ];
            prop_oneof![
                (op, inner.clone(), inner.clone()).prop_map(|(op, a, b)| bin(op, a, b)),
                (10u32..=15, inner.clone()).prop_map(|(f, a)| call(f, vec![a])),
                (20u32..=21, inner.clone(), inner).prop_map(|(f, a, b)| call(f, vec![a, b])),
            ]
        })
    }

    fn any_expr() -> impl Strategy<Value = Expr> {
        let cmp = prop_oneof![
            Just(BinaryOp::Lt),
            Just(BinaryOp::Le),
            Just(BinaryOp::Gt),
            Just(BinaryOp::Ge),
            Just(BinaryOp::Eq),
            Just(BinaryOp::Ne),
        ];
        prop_oneof![
            num_expr(),
            (cmp, num_expr(), num_expr()).prop_map(|(op, a, b)| bin(op, a, b)),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        /// The scalar evaluator and the dataflow engine agree, bit for bit,
        /// on every formula over two scalar inputs (including missing ones).
        #[test]
        fn matches_dataflow(
            expr in any_expr(),
            a in proptest::option::weighted(0.9, -4i8..=4),
            b in proptest::option::weighted(0.9, -4i8..=4),
        ) {
            let (a, b) = (a.map(f64::from), b.map(f64::from));
            let mut env = HashMap::new();
            for (id, v) in [(A, a), (B, b)] {
                if let Some(v) = v {
                    env.insert(id, CellValue::num(v));
                }
            }
            let scalar = eval(&expr, &env).expect("scalar eval");
            prop_assert_eq!(scalar, via_dataflow(&expr, a, b), "{:?}", expr);
        }
    }
}
