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
use improv_core_model::functions::{self, Arg, Kind, Ret};
use improv_core_model::{
    BinaryOp, Category, CategoryId, DimensionSpec, Expr, Formula, FuncId, Item, ItemId, Measure,
    MeasureId, UnaryOp, ValueType,
};
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

/// Read-only context for compilation: the model's measures, and its
/// categories (their items' order and names) when the caller has a model.
pub struct CompileContext<'m> {
    pub measures: &'m HashMap<MeasureId, Measure>,
    /// `None` without a model: positional forms (`PREV`, `POSITION`,
    /// ordered reductions) then order items by id.
    pub categories: Option<&'m HashMap<CategoryId, Category>>,
    /// Item names, for `@ITEM`.
    pub items: Option<&'m HashMap<ItemId, Item>>,
}

impl<'m> CompileContext<'m> {
    /// A context with measures only (scalar formulas, type checks).
    pub fn new(measures: &'m HashMap<MeasureId, Measure>) -> Self {
        CompileContext {
            measures,
            categories: None,
            items: None,
        }
    }

    /// A context over a whole model.
    pub fn for_model(model: &'m improv_core_model::Model) -> Self {
        CompileContext {
            measures: &model.measures,
            categories: Some(&model.categories),
            items: Some(&model.items),
        }
    }

    /// Category `c`'s items in order (empty when unknown).
    pub fn items_of(&self, c: CategoryId) -> Vec<ItemId> {
        self.categories
            .and_then(|cs| cs.get(&c))
            .map(|c| c.items.clone())
            .unwrap_or_default()
    }

    /// The name of category `c`, for messages.
    pub fn category_name(&self, c: CategoryId) -> String {
        self.categories
            .and_then(|cs| cs.get(&c))
            .map_or_else(|| format!("{c:?}"), |c| c.name.0.clone())
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

/// Whether `f` is an aggregation (`SUM(x OVER T)`), per the registry.
pub(crate) fn is_aggregation(f: FuncId) -> bool {
    functions::is_aggregation(f)
}

/// The number of arguments a scalar (non-aggregating, non-lazy) built-in
/// takes when that number is fixed; `None` for an unknown id. Kept for
/// callers that only need "is this an element-wise built-in".
pub fn scalar_arity(f: FuncId) -> Option<usize> {
    functions::by_id(f)
        .filter(|d| d.kind == Kind::Scalar)
        .map(|d| d.params.len())
}

/// The lazy functions' ids (see `functions::FUNCTIONS`).
pub(crate) mod lazy {
    pub const IF: u32 = 30;
    pub const IFS: u32 = 31;
    pub const SWITCH: u32 = 32;
    pub const CHOOSE: u32 = 33;
    pub const IFERROR: u32 = 34;
    pub const IFNA: u32 = 35;
    pub const CLEARERROR: u32 = 36;
    pub const CLEARZERO: u32 = 37;
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
            let typed = infer_ref(ctx, *mid, spec)?;
            // Outside an aggregation nothing collapses the categories a
            // `[…]` list leaves out: the cells would keep them while the type
            // said otherwise. Refuse, and say how to collapse one.
            let m = &ctx.measures[mid];
            if let Some(c) = m
                .categories
                .iter()
                .find(|c| !typed.ty.dim.categories.contains(c))
            {
                return Err(CompileError::DimensionMismatch(format!(
                    "{0}[…] leaves out category {c:?}, which only an aggregation \
                     can collapse: write SUM({0} OVER <category>) (or AVG/MIN/MAX)",
                    m.name.0
                )));
            }
            Ok(typed)
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
            let def = functions::by_id(*func).ok_or(CompileError::UnknownFunction(*func))?;
            if !def.accepts(args.len()) {
                return Err(CompileError::Unsupported(format!(
                    "{} expects {}, got {} argument(s)",
                    def.name,
                    def.signature(),
                    args.len()
                )));
            }
            if def.kind == Kind::Lazy {
                return infer_lazy(ctx, target, def, args);
            }
            let typed_args: Vec<TypedExpr> = args
                .iter()
                .map(|a| infer(ctx, target, a))
                .collect::<Result<_>>()?;
            for (i, a) in typed_args.iter().enumerate() {
                check_arg(def, i, a)?;
            }
            let dim = union_dim(def.name, typed_args.iter().map(|a| &a.ty.dim))?;
            let Ret::Fixed(value_type) = def.ret else {
                unreachable!("only lazy functions unify their return type")
            };
            Ok(TypedExpr {
                kind: TypedExprKind::Call(*func, typed_args),
                ty: TypeInfo { value_type, dim },
            })
        }
        Expr::Call(func, args) => {
            // Aggregation: single arg, collapse its `over` categories.
            let arg = match args.as_slice() {
                // The aggregation collapses what the `[…]` list leaves out.
                [Expr::Ref(mid, spec)] => infer_ref(ctx, *mid, spec)?,
                [a] => infer(ctx, target, a)?,
                _ => {
                    return Err(CompileError::Unsupported(format!(
                        "aggregation {func:?} expects exactly 1 arg, got {}",
                        args.len()
                    )))
                }
            };
            let def = functions::by_id(*func).ok_or(CompileError::UnknownFunction(*func))?;
            check_arg(def, 0, &arg)?;
            let Ret::Fixed(value_type) = def.ret else {
                unreachable!("aggregations have a fixed result type")
            };
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
                    value_type,
                    dim: Dim::of(reduced),
                },
            })
        }
    }
}

/// Argument `i` of `def` must have the parameter's type (`Any` takes all).
fn check_arg(def: &functions::FnDef, i: usize, a: &TypedExpr) -> Result<()> {
    let want = match def.param(i) {
        Arg::Any => return Ok(()),
        Arg::Number => ValueType::Number,
        Arg::Text => ValueType::Text,
        Arg::Boolean => ValueType::Boolean,
        Arg::Date => ValueType::DateTime,
        // A date, or a number read as a spreadsheet serial date.
        Arg::DateLike if a.ty.value_type == ValueType::Number => return Ok(()),
        Arg::DateLike => ValueType::DateTime,
        Arg::Category => {
            return Err(CompileError::TypeMismatch(format!(
                "{} argument {} must be a category name ({})",
                def.name,
                i + 1,
                def.signature()
            )))
        }
    };
    // An error-only argument (`NA()`) fits any parameter.
    if a.ty.value_type == want || is_error_only(a) {
        return Ok(());
    }
    Err(CompileError::TypeMismatch(format!(
        "{} argument {} must be {want:?}, got {:?} ({})",
        def.name,
        i + 1,
        a.ty.value_type,
        def.signature()
    )))
}

/// The union of broadcastable dims: each must be a subset of the union so far
/// or contain it, so the dataflow join is well defined.
fn union_dim<'a>(what: &str, dims: impl IntoIterator<Item = &'a Dim>) -> Result<Dim> {
    let mut dim = Dim::scalar();
    for d in dims {
        if !(dim.is_subset_of(d) || d.is_subset_of(&dim)) {
            return Err(CompileError::DimensionMismatch(format!(
                "{what} arguments not broadcastable: {:?} vs {:?}",
                dim.categories, d.categories
            )));
        }
        dim = dim.union(d);
    }
    Ok(dim)
}

/// An expression that can only ever be an error (`NA()`, `ERROR("#NUM!")`):
/// its declared type unifies with anything.
fn is_error_only(e: &TypedExpr) -> bool {
    match &e.kind {
        TypedExprKind::Call(f, _) => matches!(f.0, 53 | 54),
        TypedExprKind::Literal(v) => v.is_error(),
        _ => false,
    }
}

fn call(id: u32, args: Vec<Expr>) -> Expr {
    Expr::Call(FuncId(id), args)
}

fn na() -> Expr {
    call(53, vec![])
}

fn value_error() -> Expr {
    call(
        54,
        vec![Expr::Literal(improv_core_model::Value::Text(
            "#VALUE!".into(),
        ))],
    )
}

/// Type a lazy function. `IF` is the primitive; the others are rewritten into
/// nested `IF`s first:
///
/// * `IFS(t1, v1, t2, v2, …)` → `IF(t1, v1, IF(t2, v2, … NA()))`
/// * `SWITCH(e, m1, r1, …, [d])` → `IF(e == m1, r1, … d or NA())`
/// * `CHOOSE(i, v1, …)` → `IF(INT(i) == 1, v1, … #VALUE!)`
/// * `IFERROR(v, a)` → `IF(ISERROR(v), a, v)`, `IFNA` with `ISNA`
/// * `CLEARERROR(v, [a])` → `IF(ISERROR(v), a, v)`, or no cell without `a`
/// * `CLEARZERO(v, [a])` → `IF(v == 0, a, v)`, or no cell without `a`
fn infer_lazy(
    ctx: &CompileContext,
    target: MeasureId,
    def: &functions::FnDef,
    args: &[Expr],
) -> Result<TypedExpr> {
    if def.id.0 == lazy::IF {
        return infer_if(ctx, target, args);
    }
    infer(ctx, target, &desugar_lazy(def.id, args)?)
}

/// The nested-`IF` form of a lazy call other than `IF` itself (see
/// [`infer_lazy`]). Shared with the scalar evaluator so both agree.
pub(crate) fn desugar_lazy(id: FuncId, args: &[Expr]) -> Result<Expr> {
    let if_ = |c: Expr, a: Expr, b: Option<Expr>| {
        let mut v = vec![c, a];
        v.extend(b);
        call(lazy::IF, v)
    };
    let not = |e: Expr| Expr::UnaryOp(UnaryOp::Not, Box::new(e));
    Ok(match id.0 {
        lazy::IFS => {
            if !args.len().is_multiple_of(2) {
                return Err(CompileError::Unsupported(
                    "IFS takes test/value pairs".into(),
                ));
            }
            args.chunks(2)
                .rev()
                .fold(na(), |acc, p| if_(p[0].clone(), p[1].clone(), Some(acc)))
        }
        lazy::SWITCH => {
            let (e, rest) = args.split_first().expect("SWITCH has an expression");
            let (pairs, default) = if rest.len() % 2 == 1 {
                (&rest[..rest.len() - 1], rest.last().cloned())
            } else {
                (rest, None)
            };
            pairs
                .chunks(2)
                .rev()
                .fold(default.unwrap_or_else(na), |acc, p| {
                    let test =
                        Expr::BinaryOp(BinaryOp::Eq, Box::new(e.clone()), Box::new(p[0].clone()));
                    if_(test, p[1].clone(), Some(acc))
                })
        }
        lazy::CHOOSE => {
            let (i, vals) = args.split_first().expect("CHOOSE has an index");
            let idx = call(204, vec![i.clone()]); // INT
            vals.iter()
                .enumerate()
                .rev()
                .fold(value_error(), |acc, (k, v)| {
                    let n = Expr::Literal(improv_core_model::Value::Number(k as f64 + 1.0));
                    let test = Expr::BinaryOp(BinaryOp::Eq, Box::new(idx.clone()), Box::new(n));
                    if_(test, v.clone(), Some(acc))
                })
        }
        lazy::IFERROR | lazy::IFNA => {
            let test = call(
                if id.0 == lazy::IFERROR { 50 } else { 52 },
                vec![args[0].clone()],
            );
            if_(test, args[1].clone(), Some(args[0].clone()))
        }
        lazy::CLEARERROR => {
            let test = call(50, vec![args[0].clone()]);
            match args.get(1) {
                Some(alt) => if_(test, alt.clone(), Some(args[0].clone())),
                None => if_(not(test), args[0].clone(), None),
            }
        }
        lazy::CLEARZERO => {
            let zero = Expr::Literal(improv_core_model::Value::Number(0.0));
            let test = Expr::BinaryOp(BinaryOp::Eq, Box::new(args[0].clone()), Box::new(zero));
            match args.get(1) {
                Some(alt) => if_(test, alt.clone(), Some(args[0].clone())),
                None => if_(not(test), args[0].clone(), None),
            }
        }
        _ => return Err(CompileError::UnknownFunction(id)),
    })
}

/// `IF(test, then, [else])`: the test must be Boolean; the branches unify
/// (an error-only branch takes the other's type). A cell exists where the
/// test does and the selected branch does; where the test is an error, the
/// result is that error. The result ranges over the union of the three
/// dims, and the test, joined with at least one branch, must span it (that
/// branch supplies the coordinates of the categories the test lacks).
fn infer_if(ctx: &CompileContext, target: MeasureId, args: &[Expr]) -> Result<TypedExpr> {
    let typed: Vec<TypedExpr> = args
        .iter()
        .map(|a| infer(ctx, target, a))
        .collect::<Result<_>>()?;
    let (test, branches) = typed.split_first().expect("IF has a test");
    if test.ty.value_type != ValueType::Boolean && !is_error_only(test) {
        return Err(CompileError::TypeMismatch(format!(
            "IF test must be Boolean, got {:?}",
            test.ty.value_type
        )));
    }
    let value_type = match branches {
        [a] => a.ty.value_type,
        [a, b] if is_error_only(a) => b.ty.value_type,
        [a, b] if is_error_only(b) || a.ty.value_type == b.ty.value_type => a.ty.value_type,
        [a, b] => {
            return Err(CompileError::TypeMismatch(format!(
                "IF branches must have the same type, got {:?} and {:?}",
                a.ty.value_type, b.ty.value_type
            )))
        }
        _ => unreachable!("arity checked by the registry"),
    };
    let dim = union_dim("IF", typed.iter().map(|a| &a.ty.dim))?;
    let spans = |b: &TypedExpr| test.ty.dim.union(&b.ty.dim) == dim;
    if test.ty.dim != dim && !branches.iter().any(spans) {
        return Err(CompileError::DimensionMismatch(format!(
            "IF ranges over {:?}, but neither the test with the then-branch nor with the \
             else-branch covers it",
            dim.categories
        )));
    }
    Ok(TypedExpr {
        kind: TypedExprKind::Call(FuncId(lazy::IF), typed),
        ty: TypeInfo { value_type, dim },
    })
}

/// A measure reference's type: the measure's categories, kept by `by`,
/// minus `except` and `over`. [`infer`] adds the bare-reference check.
fn infer_ref(ctx: &CompileContext, mid: MeasureId, spec: &DimensionSpec) -> Result<TypedExpr> {
    let measure = ctx
        .measures
        .get(&mid)
        .ok_or(CompileError::UnknownMeasure(mid))?;
    let result: Vec<_> = measure
        .categories
        .iter()
        .copied()
        .filter(|c| spec.by.is_empty() || spec.by.contains(c))
        .filter(|c| !spec.except.contains(c))
        .filter(|c| !spec.over.contains(c))
        .collect();
    Ok(TypedExpr {
        kind: TypedExprKind::Ref(
            mid,
            ResolvedDimSpec {
                by: spec.by.clone(),
                over: spec.over.clone(),
                except: spec.except.clone(),
            },
        ),
        ty: TypeInfo {
            value_type: measure.value_type,
            dim: Dim::of(result),
        },
    })
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
    use ValueType::{DateTime as D, Number as N};
    match (op, lt, rt) {
        // Date arithmetic, in days: date ± days is a date, date − date days.
        (Add, D, N) | (Add, N, D) | (Sub, D, N) => return Ok(D),
        (Sub, D, D) => return Ok(N),
        _ => {}
    }
    match op {
        Add | Sub | Mul | Div | Pow => {
            require(
                lt == ValueType::Number && rt == ValueType::Number,
                op,
                "Number",
            )?;
            Ok(ValueType::Number)
        }
        // `&` joins any two values as text.
        Concat => Ok(ValueType::Text),
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
            // A scalar literal operand broadcasts over the other operand and is
            // folded into the map by the dataflow builder — no Join needed.
            let is_scalar_lit =
                |e: &TypedExpr| matches!(e.kind, TypedExprKind::Literal(_)) && e.ty.dim.is_scalar();
            if l.ty.dim == r.ty.dim || is_scalar_lit(l) || is_scalar_lit(r) {
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
        TypedExprKind::Call(func, args) if func.0 == lazy::IF => {
            let mut plans = args
                .iter()
                .map(|a| build_plan(ctx, a))
                .collect::<Result<Vec<_>>>()?
                .into_iter();
            let test = plans.next().expect("IF has a test");
            let then = plans.next().expect("IF has a then-branch");
            let otherwise = plans.next();
            // The branch that, with the test, spans the result: its coordinates
            // fill in the categories the test lacks. None when the test spans.
            let spanning = if test.ty.dim == ty.dim {
                None
            } else if test.ty.dim.union(&then.ty.dim) == ty.dim {
                Some(0)
            } else {
                Some(1)
            };
            PlanNodeKind::If {
                test: Box::new(test),
                then: Box::new(then),
                otherwise: otherwise.map(Box::new),
                spanning,
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
#[cfg_attr(coverage_nightly, coverage(off))]
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

    // 3b. A bare `Quantity[Product]` would leave Time in the cells but not in
    // the type, so it is refused; a list naming every category is fine, and
    // inside an aggregation the list's left-out categories are collapsed.
    #[test]
    fn by_list_must_not_drop_categories_outside_an_aggregation() {
        let measures = fixture();
        let ctx = CompileContext::new(&measures);
        let by = |cats: Vec<CategoryId>| {
            Expr::Ref(
                QTY,
                DimensionSpec {
                    by: cats,
                    ..Default::default()
                },
            )
        };
        let err = infer(&ctx, MeasureId(200), &by(vec![PRODUCT])).unwrap_err();
        match err {
            CompileError::DimensionMismatch(msg) => {
                assert!(msg.contains("SUM(Quantity OVER"), "{msg}")
            }
            other => panic!("expected DimensionMismatch, got {other:?}"),
        }
        let full = infer(&ctx, MeasureId(200), &by(vec![TIME, PRODUCT])).expect("full list");
        assert_eq!(full.ty.dim, Dim::of(vec![TIME, PRODUCT]));
        let agg = Expr::Call(
            FuncId(1),
            vec![Expr::Ref(
                QTY,
                DimensionSpec {
                    by: vec![PRODUCT],
                    over: vec![TIME],
                    except: vec![],
                },
            )],
        );
        let typed = infer(&ctx, MeasureId(200), &agg).expect("aggregated");
        assert_eq!(typed.ty.dim, Dim::of(vec![PRODUCT]));
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

        // Unknown function id (in no registry entry).
        let unknown = Expr::Call(FuncId(999), vec![refr(PRICE)]);
        assert_eq!(
            infer(&ctx, MeasureId(200), &unknown),
            Err(CompileError::UnknownFunction(FuncId(999)))
        );
    }

    fn with_flag() -> (HashMap<MeasureId, Measure>, MeasureId) {
        let mut measures = fixture();
        let flag = MeasureId(103);
        measures.insert(
            flag,
            Measure {
                value_type: ValueType::Boolean,
                ..num_measure(flag, "Flag", vec![PRODUCT])
            },
        );
        (measures, flag)
    }

    fn bin(op: BinaryOp, l: Expr, r: Expr) -> Expr {
        Expr::BinaryOp(op, Box::new(l), Box::new(r))
    }

    /// Date arithmetic types: date ± number is a date, date − date a number,
    /// anything else with a date an error.
    #[test]
    fn date_arithmetic_types() {
        let mut measures = fixture();
        let when = MeasureId(110);
        measures.insert(
            when,
            Measure {
                value_type: ValueType::DateTime,
                ..num_measure(when, "When", vec![PRODUCT])
            },
        );
        let ctx = CompileContext::new(&measures);
        let ty = |e: &Expr| infer(&ctx, MeasureId(200), e).map(|t| t.ty.value_type);
        assert_eq!(
            ty(&bin(BinaryOp::Add, refr(when), lit(1.0))),
            Ok(ValueType::DateTime)
        );
        assert_eq!(
            ty(&bin(BinaryOp::Add, lit(1.0), refr(when))),
            Ok(ValueType::DateTime)
        );
        assert_eq!(
            ty(&bin(BinaryOp::Sub, refr(when), lit(1.0))),
            Ok(ValueType::DateTime)
        );
        assert_eq!(
            ty(&bin(BinaryOp::Sub, refr(when), refr(when))),
            Ok(ValueType::Number)
        );
        assert!(ty(&bin(BinaryOp::Mul, refr(when), lit(2.0))).is_err());
        assert!(ty(&bin(BinaryOp::Sub, lit(1.0), refr(when))).is_err());
    }

    /// IF's type and dimension rules, and the lazy desugarings' arity rules.
    #[test]
    fn if_family_type_and_dimension_rules() {
        let (measures, flag) = with_flag();
        let ctx = CompileContext::new(&measures);
        let infer_ = |e: &Expr| infer(&ctx, MeasureId(200), e);
        let call = |id: u32, args: Vec<Expr>| Expr::Call(FuncId(id), args);
        let na = || call(53, vec![]);
        let text = |s: &str| Expr::Literal(Value::Text(s.into()));
        // The test must be Boolean; NA() as a test is accepted (it errors).
        assert!(matches!(
            infer_(&call(30, vec![refr(PRICE), lit(1.0)])),
            Err(CompileError::TypeMismatch(_))
        ));
        assert!(infer_(&call(30, vec![na(), lit(1.0)])).is_ok());
        // Branches unify; an error-only branch takes the other's type.
        assert!(matches!(
            infer_(&call(30, vec![refr(flag), lit(1.0), text("x")])),
            Err(CompileError::TypeMismatch(_))
        ));
        let t = infer_(&call(30, vec![refr(flag), na(), text("x")])).unwrap();
        assert_eq!(t.ty.value_type, ValueType::Text);
        let t = infer_(&call(30, vec![refr(flag), text("x"), na()])).unwrap();
        assert_eq!(t.ty.value_type, ValueType::Text);
        // Spanning: Flag over Product, Quantity over Time×Product spans via
        // either branch; Headcount over Region cannot broadcast at all.
        let t = infer_(&call(30, vec![refr(flag), lit(0.0), refr(QTY)])).unwrap();
        assert_eq!(t.ty.dim, Dim::of(vec![TIME, PRODUCT]));
        let plan = build_plan(&ctx, &t).unwrap();
        assert!(matches!(
            plan.kind,
            PlanNodeKind::If {
                spanning: Some(1),
                ..
            }
        ));
        let t = infer_(&call(30, vec![refr(flag), refr(QTY), lit(0.0)])).unwrap();
        assert!(matches!(
            build_plan(&ctx, &t).unwrap().kind,
            PlanNodeKind::If {
                spanning: Some(0),
                ..
            }
        ));
        assert!(matches!(
            infer_(&call(30, vec![refr(flag), refr(HEADCOUNT), lit(0.0)])),
            Err(CompileError::DimensionMismatch(_))
        ));
        // A test over Time with branches over Time×Product and
        // Time×Product×Region spans through the wider (else) branch.
        let mut m2 = measures.clone();
        let wide = MeasureId(105);
        m2.insert(wide, num_measure(wide, "W", vec![TIME, PRODUCT, REGION]));
        let tflag = MeasureId(106);
        m2.insert(
            tflag,
            Measure {
                value_type: ValueType::Boolean,
                ..num_measure(tflag, "TF", vec![TIME])
            },
        );
        let ctx2 = CompileContext::new(&m2);
        let e = call(30, vec![refr(tflag), refr(QTY), refr(wide)]);
        let t = infer(&ctx2, MeasureId(200), &e).unwrap();
        assert!(matches!(
            build_plan(&ctx2, &t).unwrap().kind,
            PlanNodeKind::If {
                spanning: Some(1),
                ..
            }
        ));
        // IFS needs pairs; SWITCH with and without a default; arity errors.
        assert!(matches!(
            infer_(&call(31, vec![refr(flag), lit(1.0), refr(flag)])),
            Err(CompileError::Unsupported(_))
        ));
        assert!(infer_(&call(32, vec![refr(PRICE), lit(1.0), text("a"), text("z")])).is_ok());
        assert!(infer_(&call(32, vec![refr(PRICE), lit(1.0), text("a")])).is_ok());
        assert!(matches!(
            infer_(&call(312, vec![refr(PRICE)])),
            Err(CompileError::Unsupported(_))
        ));
        assert_eq!(
            infer_(&call(999, vec![])),
            Err(CompileError::UnknownFunction(FuncId(999)))
        );
        // A Text argument where a number is wanted; an error-only argument fits.
        assert!(matches!(
            infer_(&call(10, vec![text("x")])),
            Err(CompileError::TypeMismatch(_))
        ));
        assert!(infer_(&call(10, vec![na()])).is_ok());
        assert!(matches!(
            infer_(&call(22, vec![refr(PRICE), refr(HEADCOUNT)])),
            Err(CompileError::DimensionMismatch(_))
        ));
        // desugar_lazy on a non-lazy id.
        assert_eq!(
            desugar_lazy(FuncId(10), &[]),
            Err(CompileError::UnknownFunction(FuncId(10)))
        );
        // NOT on a number.
        assert!(matches!(
            infer_(&Expr::UnaryOp(UnaryOp::Not, Box::new(refr(PRICE)))),
            Err(CompileError::TypeMismatch(_))
        ));
    }

    fn lit(n: f64) -> Expr {
        Expr::Literal(Value::Number(n))
    }

    #[test]
    fn infer_rejects_ill_typed_and_unbroadcastable_exprs() {
        let (measures, flag) = with_flag();
        let ctx = CompileContext::new(&measures);
        let err = |e: &Expr| infer(&ctx, MeasureId(200), e).unwrap_err();
        let msg = |e: &Expr| err(e).to_string();

        let e = Expr::Literal(Value::Error(improv_core_model::ValueError::new(
            improv_core_model::ValueErrorKind::DivisionByZero,
        )));
        assert!(msg(&e).contains("error literal"));
        // Arithmetic needs Numbers; logic needs Booleans.
        assert!(msg(&bin(BinaryOp::Add, refr(flag), lit(1.0))).contains("requires Number"));
        assert!(msg(&bin(BinaryOp::And, refr(flag), refr(PRICE))).contains("requires Boolean"));
        assert!(msg(&bin(BinaryOp::Or, refr(PRICE), refr(flag))).contains("requires Boolean"));
        // Scalar call: a bad arg propagates; disjoint dims don't broadcast.
        assert_eq!(
            err(&Expr::Call(FuncId(10), vec![refr(MeasureId(999))])),
            CompileError::UnknownMeasure(MeasureId(999))
        );
        assert!(matches!(
            err(&Expr::Call(FuncId(20), vec![refr(PRICE), refr(HEADCOUNT)])),
            CompileError::DimensionMismatch(_)
        ));
        // Aggregations take exactly one Number arg.
        assert!(msg(&Expr::Call(FuncId(1), vec![refr(PRICE), refr(PRICE)])).contains("exactly 1"));
        assert!(msg(&Expr::Call(FuncId(1), vec![refr(flag)])).contains("must be Number"));
        // AND(flag OVER …) takes booleans; SUM(flag, …) does not.
        assert!(msg(&Expr::Call(FuncId(22), vec![refr(flag)]))
            .contains("SUM argument 1 must be Number"));
    }

    #[test]
    fn infer_accepts_logic_broadcast_calls_and_folds_literals() {
        let (measures, flag) = with_flag();
        let ctx = CompileContext::new(&measures);
        let ok = |e: &Expr| infer(&ctx, MeasureId(200), e).unwrap();

        let t = ok(&bin(BinaryOp::And, refr(flag), refr(flag)));
        assert_eq!(t.ty.value_type, ValueType::Boolean);
        // MIN2(Quantity[Time,Product], Price[Product]) broadcasts to the union.
        let t = ok(&Expr::Call(FuncId(20), vec![refr(QTY), refr(PRICE)]));
        assert_eq!(t.ty.dim, Dim::of(vec![TIME, PRODUCT]));
        // SUM over a non-ref arg has no `over`, so it collapses nothing.
        let t = ok(&Expr::Call(
            FuncId(1),
            vec![bin(BinaryOp::Mul, refr(PRICE), lit(2.0))],
        ));
        assert_eq!(t.ty.dim, Dim::of(vec![PRODUCT]));
        // A scalar literal on the left folds into the map: no Join.
        let typed = ok(&bin(BinaryOp::Mul, lit(2.0), refr(QTY)));
        match build_plan(&ctx, &typed).unwrap().kind {
            PlanNodeKind::MapBinary(BinaryOp::Mul, l, r) => {
                assert!(matches!(l.kind, PlanNodeKind::Literal(_)));
                assert_eq!(r.kind, PlanNodeKind::InputMeasure(QTY));
            }
            other => panic!("expected MapBinary, got {other:?}"),
        }
    }
}
