//! The formula compiler: `core_model::Formula` -> `TypedExpr` -> `PlanNode`.
//!
//! Phase 1 of `AGENT_STEERING.md`. Two passes:
//!   1. `infer` — walk the AST, infer value type + dimension for each node,
//!      resolving measure references against the model's measure metadata.
//!      Catches structural errors (dimension mismatch) and type errors.
//!   2. `build_plan` — lower the typed tree to a `PlanNode` graph, inserting
//!      `Join` nodes to align operand dimensions and `Aggregate` nodes for
//!      aggregation functions.
//!
//! IMPLEMENTED. This file defines the public API contract and both passes.

use crate::plan::{PlanNode, PlanNodeKind};
use crate::typed::{Dim, ResolvedDimSpec, TypeInfo, TypedExpr, TypedExprKind};
use improv_core_model::{BinaryOp, Expr, Formula, FuncId, Measure, MeasureId, UnaryOp, ValueType};
use std::collections::HashMap;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum CompileError {
    #[error("unknown measure referenced in formula: {0:?}")]
    UnknownMeasure(MeasureId),
    #[error("unknown function: {0:?}")]
    UnknownFunction(improv_core_model::FuncId),
    #[error("type mismatch: {0}")]
    TypeMismatch(String),
    #[error("dimension mismatch: {0}")]
    DimensionMismatch(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, CompileError>;

/// Read-only context for compilation: the model's measures (and, later, a
/// function registry).
pub struct CompileContext<'m> {
    pub measures: &'m HashMap<MeasureId, Measure>,
}

impl<'m> CompileContext<'m> {
    pub fn new(measures: &'m HashMap<MeasureId, Measure>) -> Self {
        CompileContext { measures }
    }
}

/// Compile a formula for `target` measure into an operator plan.
pub fn compile_formula(
    ctx: &CompileContext,
    target: MeasureId,
    formula: &Formula,
) -> Result<PlanNode> {
    let typed = infer(ctx, target, &formula.expr)?;
    build_plan(ctx, &typed)
}

/// Aggregation func-id convention (v1 built-ins). An aggregation `Call` takes a
/// single arg (a measure ref) whose `ResolvedDimSpec.over` names the categories
/// to collapse; the result dim is the arg's dim with `over` removed. Non-listed
/// func ids are `UnknownFunction`.
const SUM: FuncId = FuncId(1);
const AVG: FuncId = FuncId(2);
const MIN: FuncId = FuncId(3);
const MAX: FuncId = FuncId(4);

fn is_aggregation(f: FuncId) -> bool {
    matches!(f, SUM | AVG | MIN | MAX)
}

/// Scalar (non-aggregating) built-in func-id registry. These ids start at 10 so
/// they never collide with the aggregation ids (1-4). Each takes `arity`
/// Number args and returns Number over the union of the args' dims. Evaluation
/// lives in `dataflow::apply_scalar`; the two tables must agree on ids/arity.
///
/// | id | name  | arity | meaning                          |
/// |----|-------|-------|----------------------------------|
/// | 10 | ABS   | 1     | absolute value                   |
/// | 11 | ROUND | 1     | round half-to-even (f64::round)  |
/// | 12 | FLOOR | 1     | floor                            |
/// | 13 | CEIL  | 1     | ceil                             |
/// | 14 | SQRT  | 1     | sqrt (NaN for negative input)    |
/// | 15 | NEG   | 1     | negate                           |
/// | 20 | MIN2  | 2     | min of two args                  |
/// | 21 | MAX2  | 2     | max of two args                  |
///
/// Returns the arity for a known scalar func id, or `None` if unknown.
pub fn scalar_arity(f: FuncId) -> Option<usize> {
    match f.0 {
        10..=15 => Some(1),
        20 | 21 => Some(2),
        _ => None,
    }
}

/// Pass 1: type + dimension inference.
#[allow(
    clippy::only_used_in_recursion,
    reason = "`target` is threaded for future error context and recursive calls"
)]
pub fn infer(ctx: &CompileContext, target: MeasureId, expr: &Expr) -> Result<TypedExpr> {
    match expr {
        Expr::Literal(v) => {
            let value_type = v.type_of().ok_or_else(|| {
                CompileError::TypeMismatch("error literal has no value type".into())
            })?;
            Ok(TypedExpr {
                kind: TypedExprKind::Literal(v.clone()),
                ty: TypeInfo {
                    value_type,
                    dim: Dim::scalar(),
                },
            })
        }
        Expr::Ref(mid, spec) => {
            let measure = ctx
                .measures
                .get(mid)
                .ok_or(CompileError::UnknownMeasure(*mid))?;

            // Base dim = measure's categories. `by` keeps only those; `except`
            // drops; `over` marks categories to aggregate away. All three are
            // removed-or-kept relative to the base set.
            let base = &measure.categories;
            let keep_by =
                |c: &improv_core_model::CategoryId| spec.by.is_empty() || spec.by.contains(c);
            let result: Vec<_> = base
                .iter()
                .copied()
                .filter(keep_by)
                .filter(|c| !spec.except.contains(c))
                .filter(|c| !spec.over.contains(c))
                .collect();

            let resolved = ResolvedDimSpec {
                by: spec.by.clone(),
                over: spec.over.clone(),
                except: spec.except.clone(),
            };
            Ok(TypedExpr {
                kind: TypedExprKind::Ref(*mid, resolved),
                ty: TypeInfo {
                    value_type: measure.value_type,
                    dim: Dim::of(result),
                },
            })
        }
        Expr::UnaryOp(op, e) => {
            let inner = infer(ctx, target, e)?;
            let want = match op {
                UnaryOp::Neg => ValueType::Number,
                UnaryOp::Not => ValueType::Boolean,
            };
            if inner.ty.value_type != want {
                return Err(CompileError::TypeMismatch(format!(
                    "unary {op:?} expects {want:?}, got {:?}",
                    inner.ty.value_type
                )));
            }
            let ty = inner.ty.clone();
            Ok(TypedExpr {
                kind: TypedExprKind::UnaryOp(*op, Box::new(inner)),
                ty,
            })
        }
        Expr::BinaryOp(op, l, r) => {
            let lt = infer(ctx, target, l)?;
            let rt = infer(ctx, target, r)?;
            let value_type = binop_result_type(*op, &lt, &rt)?;

            // Broadcastable: one dim must be a subset of the other.
            let dim = if lt.ty.dim.is_subset_of(&rt.ty.dim) {
                rt.ty.dim.clone()
            } else if rt.ty.dim.is_subset_of(&lt.ty.dim) {
                lt.ty.dim.clone()
            } else {
                return Err(CompileError::DimensionMismatch(format!(
                    "neither operand dim is a subset of the other: {:?} vs {:?}",
                    lt.ty.dim.categories, rt.ty.dim.categories
                )));
            };
            Ok(TypedExpr {
                kind: TypedExprKind::BinaryOp(*op, Box::new(lt), Box::new(rt)),
                ty: TypeInfo { value_type, dim },
            })
        }
        Expr::Call(func, args) if !is_aggregation(*func) => {
            // Scalar built-in: N Number args on broadcastable dims, result
            // Number over the union dim.
            let arity = scalar_arity(*func).ok_or(CompileError::UnknownFunction(*func))?;
            if args.len() != arity {
                return Err(CompileError::Unsupported(format!(
                    "scalar {func:?} expects {arity} arg(s), got {}",
                    args.len()
                )));
            }
            let typed_args: Vec<TypedExpr> = args
                .iter()
                .map(|a| infer(ctx, target, a))
                .collect::<Result<_>>()?;
            for a in &typed_args {
                if a.ty.value_type != ValueType::Number {
                    return Err(CompileError::TypeMismatch(format!(
                        "scalar {func:?} expects Number args, got {:?}",
                        a.ty.value_type
                    )));
                }
            }
            // Union dim; require pairwise broadcastability (one a subset of the
            // other) so the dataflow join is well defined.
            let mut dim = Dim::scalar();
            for a in &typed_args {
                if !(dim.is_subset_of(&a.ty.dim) || a.ty.dim.is_subset_of(&dim)) {
                    return Err(CompileError::DimensionMismatch(format!(
                        "scalar {func:?} args not broadcastable: {:?} vs {:?}",
                        dim.categories, a.ty.dim.categories
                    )));
                }
                dim = dim.union(&a.ty.dim);
            }
            Ok(TypedExpr {
                kind: TypedExprKind::Call(*func, typed_args),
                ty: TypeInfo {
                    value_type: ValueType::Number,
                    dim,
                },
            })
        }
        Expr::Call(func, args) => {
            // Aggregation: single arg, collapse its `over` categories.
            let arg = match args.as_slice() {
                [a] => infer(ctx, target, a)?,
                _ => {
                    return Err(CompileError::Unsupported(format!(
                        "aggregation {func:?} expects exactly 1 arg, got {}",
                        args.len()
                    )))
                }
            };
            if arg.ty.value_type != ValueType::Number {
                return Err(CompileError::TypeMismatch(format!(
                    "aggregation {func:?} expects Number, got {:?}",
                    arg.ty.value_type
                )));
            }
            let over = aggregation_over(&arg);
            let reduced: Vec<_> = arg
                .ty
                .dim
                .categories
                .iter()
                .copied()
                .filter(|c| !over.contains(c))
                .collect();
            Ok(TypedExpr {
                kind: TypedExprKind::Call(*func, vec![arg]),
                ty: TypeInfo {
                    value_type: ValueType::Number,
                    dim: Dim::of(reduced),
                },
            })
        }
    }
}

/// Categories an aggregation collapses: the arg ref's `over`, restricted to the
/// categories actually present in the arg's dim.
fn aggregation_over(arg: &TypedExpr) -> Vec<improv_core_model::CategoryId> {
    match &arg.kind {
        TypedExprKind::Ref(_, spec) => spec
            .over
            .iter()
            .copied()
            .filter(|c| arg.ty.dim.categories.contains(c))
            .collect(),
        _ => Vec::new(),
    }
}

fn binop_result_type(op: BinaryOp, l: &TypedExpr, r: &TypedExpr) -> Result<ValueType> {
    use BinaryOp::*;
    let (lt, rt) = (l.ty.value_type, r.ty.value_type);
    match op {
        Add | Sub | Mul | Div => {
            require(
                lt == ValueType::Number && rt == ValueType::Number,
                op,
                "Number",
            )?;
            Ok(ValueType::Number)
        }
        And | Or => {
            require(
                lt == ValueType::Boolean && rt == ValueType::Boolean,
                op,
                "Boolean",
            )?;
            Ok(ValueType::Boolean)
        }
        Eq | Ne | Lt | Le | Gt | Ge => {
            require(lt == rt, op, "matching operand types")?;
            Ok(ValueType::Boolean)
        }
    }
}

fn require(ok: bool, op: BinaryOp, want: &str) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(CompileError::TypeMismatch(format!(
            "{op:?} requires {want}"
        )))
    }
}

/// Pass 2: lower a typed expression to a plan.
#[allow(
    clippy::only_used_in_recursion,
    reason = "`ctx` kept in the signature for symmetry and future func-registry lookups"
)]
pub fn build_plan(ctx: &CompileContext, typed: &TypedExpr) -> Result<PlanNode> {
    let ty = typed.ty.clone();
    let kind = match &typed.kind {
        TypedExprKind::Literal(v) => PlanNodeKind::Literal(v.clone()),
        TypedExprKind::Ref(mid, _spec) => {
            // A bare ref lowers to its input collection. `infer` already removed
            // any `over` categories from the ref's dim, so a bare ref never
            // self-aggregates here; `Call` emits the Aggregate node.
            PlanNodeKind::InputMeasure(*mid)
        }
        TypedExprKind::UnaryOp(op, e) => PlanNodeKind::MapUnary(*op, Box::new(build_plan(ctx, e)?)),
        TypedExprKind::BinaryOp(op, l, r) => {
            let lp = build_plan(ctx, l)?;
            let rp = build_plan(ctx, r)?;
            if l.ty.dim == r.ty.dim {
                PlanNodeKind::MapBinary(*op, Box::new(lp), Box::new(rp))
            } else {
                // Dims differ: insert a Join on the shared (intersection)
                // categories to broadcast the two operands onto the union dim,
                // then map element-wise. The Join carries both operands; each
                // MapBinary child then ranges over the union (result) dim.
                let join_keys: Vec<_> =
                    l.ty.dim
                        .categories
                        .iter()
                        .copied()
                        .filter(|c| r.ty.dim.categories.contains(c))
                        .collect();
                let (lp_aligned, rp_aligned) = split_join(lp, rp, join_keys, &ty);
                PlanNodeKind::MapBinary(*op, Box::new(lp_aligned), Box::new(rp_aligned))
            }
        }
        TypedExprKind::Call(func, args) if !is_aggregation(*func) => {
            // Scalar built-in: lower each arg; the dataflow builder aligns
            // multi-arg calls by key (join) and applies the function.
            let arg_plans = args
                .iter()
                .map(|a| build_plan(ctx, a))
                .collect::<Result<Vec<_>>>()?;
            PlanNodeKind::FuncCall {
                func: *func,
                args: arg_plans,
            }
        }
        TypedExprKind::Call(func, args) => {
            let arg = &args[0];
            let input = build_plan(ctx, arg)?;
            PlanNodeKind::Aggregate {
                input: Box::new(input),
                group_by: typed.ty.dim.categories.clone(),
                func: *func,
            }
        }
    };
    Ok(PlanNode { kind, ty })
}

/// Build the aligned operand pair for a dimension-broadcasting MapBinary: a
/// `Join` node (aligned to the union dim) is the left operand; the right
/// operand references the same join by carrying the right side. Both children
/// carry the union `TypeInfo` so downstream sees a single aligned collection.
fn split_join(
    lp: PlanNode,
    rp: PlanNode,
    join_keys: Vec<improv_core_model::CategoryId>,
    union_ty: &TypeInfo,
) -> (PlanNode, PlanNode) {
    let right_leaf = rp.clone();
    let join = PlanNode {
        kind: PlanNodeKind::Join {
            left: Box::new(lp),
            right: Box::new(rp),
            join_keys,
        },
        ty: union_ty.clone(),
    };
    (join, right_leaf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{CategoryId, DimensionSpec, MeasureKind, Name, Value};

    const TIME: CategoryId = CategoryId(1);
    const PRODUCT: CategoryId = CategoryId(2);
    const REGION: CategoryId = CategoryId(3);
    const PRICE: MeasureId = MeasureId(100); // over [Product]
    const QTY: MeasureId = MeasureId(101); // over [Time, Product]
    const HEADCOUNT: MeasureId = MeasureId(102); // over [Region]

    fn num_measure(id: MeasureId, name: &str, cats: Vec<CategoryId>) -> Measure {
        Measure {
            id,
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: cats,
            kind: MeasureKind::Input,
            description: None,
        }
    }

    fn fixture() -> HashMap<MeasureId, Measure> {
        let mut m = HashMap::new();
        m.insert(PRICE, num_measure(PRICE, "Price", vec![PRODUCT]));
        m.insert(QTY, num_measure(QTY, "Quantity", vec![TIME, PRODUCT]));
        m.insert(HEADCOUNT, num_measure(HEADCOUNT, "Headcount", vec![REGION]));
        m
    }

    fn refr(id: MeasureId) -> Expr {
        Expr::Ref(id, DimensionSpec::default())
    }

    // 1. Revenue = Price[Product] * Quantity[Time,Product]
    #[test]
    fn revenue_mul_infers_union_dim_and_joins() {
        let measures = fixture();
        let ctx = CompileContext::new(&measures);
        let expr = Expr::BinaryOp(BinaryOp::Mul, Box::new(refr(PRICE)), Box::new(refr(QTY)));

        let typed = infer(&ctx, MeasureId(200), &expr).expect("infer");
        assert_eq!(typed.ty.value_type, ValueType::Number);
        assert_eq!(typed.ty.dim, Dim::of(vec![TIME, PRODUCT]), "union dim");

        let plan = build_plan(&ctx, &typed).expect("plan");
        match plan.kind {
            PlanNodeKind::MapBinary(BinaryOp::Mul, left, _right) => match left.kind {
                PlanNodeKind::Join { join_keys, .. } => {
                    assert_eq!(join_keys, vec![PRODUCT], "join on shared Product");
                }
                other => panic!("expected Join feeding MapBinary, got {other:?}"),
            },
            other => panic!("expected MapBinary(Mul), got {other:?}"),
        }
    }

    // 2. Disjoint dims (Product-only vs Region-only), neither a subset -> error.
    #[test]
    fn disjoint_dims_error() {
        let measures = fixture();
        let ctx = CompileContext::new(&measures);
        let expr = Expr::BinaryOp(
            BinaryOp::Add,
            Box::new(refr(PRICE)),     // [Product]
            Box::new(refr(HEADCOUNT)), // [Region]
        );
        let err = infer(&ctx, MeasureId(200), &expr).unwrap_err();
        assert!(matches!(err, CompileError::DimensionMismatch(_)), "{err:?}");
    }

    // 3. SUM(Revenue OVER Time): {Time,Product} -> {Product}, Aggregate plan.
    #[test]
    fn sum_over_time_reduces_dim() {
        let measures = fixture();
        let ctx = CompileContext::new(&measures);
        // Quantity ref aggregating OVER Time.
        let arg = Expr::Ref(
            QTY,
            DimensionSpec {
                over: vec![TIME],
                ..Default::default()
            },
        );
        let expr = Expr::Call(FuncId(1), vec![arg]); // SUM

        let typed = infer(&ctx, MeasureId(200), &expr).expect("infer");
        assert_eq!(typed.ty.dim, Dim::of(vec![PRODUCT]), "Time collapsed");

        let plan = build_plan(&ctx, &typed).expect("plan");
        match plan.kind {
            PlanNodeKind::Aggregate { group_by, func, .. } => {
                assert_eq!(group_by, vec![PRODUCT]);
                assert_eq!(func, FuncId(1));
            }
            other => panic!("expected Aggregate, got {other:?}"),
        }
    }

    // 4. Unknown measure ref errors with UnknownMeasure.
    #[test]
    fn unknown_measure_errors() {
        let measures = fixture();
        let ctx = CompileContext::new(&measures);
        let err = infer(&ctx, MeasureId(200), &refr(MeasureId(999))).unwrap_err();
        assert_eq!(err, CompileError::UnknownMeasure(MeasureId(999)));
    }

    // Extra coverage: literal, unary Neg, type mismatch, unknown func,
    // compile_formula end-to-end.
    #[test]
    fn literal_and_unary_and_type_errors() {
        let measures = fixture();
        let ctx = CompileContext::new(&measures);

        let lit = infer(&ctx, MeasureId(200), &Expr::Literal(Value::Number(3.0))).unwrap();
        assert_eq!(lit.ty.value_type, ValueType::Number);
        assert!(lit.ty.dim.is_scalar());
        assert!(matches!(
            build_plan(&ctx, &lit).unwrap().kind,
            PlanNodeKind::Literal(Value::Number(_))
        ));

        let neg = Expr::UnaryOp(UnaryOp::Neg, Box::new(refr(PRICE)));
        let t = infer(&ctx, MeasureId(200), &neg).unwrap();
        assert_eq!(t.ty.dim, Dim::of(vec![PRODUCT]));
        assert!(matches!(
            build_plan(&ctx, &t).unwrap().kind,
            PlanNodeKind::MapUnary(UnaryOp::Neg, _)
        ));

        // Not on a Number -> type mismatch.
        let bad = Expr::UnaryOp(UnaryOp::Not, Box::new(refr(PRICE)));
        assert!(matches!(
            infer(&ctx, MeasureId(200), &bad),
            Err(CompileError::TypeMismatch(_))
        ));

        // Unknown function id.
        let uf = Expr::Call(FuncId(999), vec![refr(PRICE)]);
        assert_eq!(
            infer(&ctx, MeasureId(200), &uf),
            Err(CompileError::UnknownFunction(FuncId(999)))
        );

        // End-to-end through compile_formula.
        let f = Formula::new(Expr::BinaryOp(
            BinaryOp::Mul,
            Box::new(refr(PRICE)),
            Box::new(refr(QTY)),
        ));
        let plan = compile_formula(&ctx, MeasureId(200), &f).unwrap();
        assert!(matches!(
            plan.kind,
            PlanNodeKind::MapBinary(BinaryOp::Mul, ..)
        ));
    }

    // Scalar FuncCall: ABS(Price) types as Number with the arg's dim and
    // lowers to PlanNodeKind::FuncCall.
    #[test]
    fn scalar_abs_infers_and_lowers() {
        let measures = fixture();
        let ctx = CompileContext::new(&measures);
        let expr = Expr::Call(FuncId(10), vec![refr(PRICE)]); // ABS(Price[Product])

        let typed = infer(&ctx, MeasureId(200), &expr).expect("infer");
        assert_eq!(typed.ty.value_type, ValueType::Number);
        assert_eq!(typed.ty.dim, Dim::of(vec![PRODUCT]), "dim follows the arg");

        let plan = build_plan(&ctx, &typed).expect("plan");
        match plan.kind {
            PlanNodeKind::FuncCall { func, args } => {
                assert_eq!(func, FuncId(10));
                assert_eq!(args.len(), 1);
                assert!(matches!(args[0].kind, PlanNodeKind::InputMeasure(PRICE)));
            }
            other => panic!("expected FuncCall, got {other:?}"),
        }
    }

    // 2-arg scalar MIN2 unions dims; wrong-arity and wrong-type error.
    #[test]
    fn scalar_min2_union_dim_and_errors() {
        let measures = fixture();
        let ctx = CompileContext::new(&measures);
        // MIN2(Price[Product], Quantity[Time,Product]) -> [Time,Product]
        let ok = Expr::Call(FuncId(20), vec![refr(PRICE), refr(QTY)]);
        let typed = infer(&ctx, MeasureId(200), &ok).expect("infer");
        assert_eq!(typed.ty.dim, Dim::of(vec![TIME, PRODUCT]), "union dim");

        // Wrong arity: ABS with 2 args.
        let bad_arity = Expr::Call(FuncId(10), vec![refr(PRICE), refr(QTY)]);
        assert!(matches!(
            infer(&ctx, MeasureId(200), &bad_arity),
            Err(CompileError::Unsupported(_))
        ));

        // Wrong type: ABS of a comparison (Boolean) result.
        let cmp = Expr::BinaryOp(
            BinaryOp::Gt,
            Box::new(refr(PRICE)),
            Box::new(Expr::Literal(Value::Number(1.0))),
        );
        let bad_type = Expr::Call(FuncId(10), vec![cmp]);
        assert!(matches!(
            infer(&ctx, MeasureId(200), &bad_type),
            Err(CompileError::TypeMismatch(_))
        ));

        // Unknown scalar id (not 10-15/20-21 and not an aggregation).
        let unknown = Expr::Call(FuncId(50), vec![refr(PRICE)]);
        assert_eq!(
            infer(&ctx, MeasureId(200), &unknown),
            Err(CompileError::UnknownFunction(FuncId(50)))
        );
    }
}
