//! The dimension-aware formula AST.
//!
//! Formulas are defined over measure *names/ids* and categories, not cell
//! addresses. Dimension operations (BY / OVER / EXCEPT) are explicit so the
//! compiler can statically check dimension alignment.

use crate::ids::{CategoryId, MeasureId};
use crate::value::Value;
use serde::{Deserialize, Serialize};

/// Identifies a built-in or user-defined function (SUM, AVG, IF, ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FuncId(pub u32);

/// How a measure reference is projected/aggregated across dimensions.
///
/// * `by`     — categories to keep (group by).
/// * `over`   — categories to aggregate over (collapse).
/// * `except` — categories to drop.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DimensionSpec {
    pub by: Vec<CategoryId>,
    pub over: Vec<CategoryId>,
    pub except: Vec<CategoryId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BinaryOp {
    Add,
    Sub,
    Mul,
    Div,
    And,
    Or,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    /// `^`: power (right-associative; binds tighter than unary minus).
    Pow,
    /// `&`: join two values as text.
    Concat,
}

/// A formula expression. Broadcasting over shared dimensions is implicit at the
/// language level and made explicit by the compiler (Phase 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Literal(Value),
    /// Reference another measure, with an optional dimension projection.
    Ref(MeasureId, DimensionSpec),
    UnaryOp(UnaryOp, Box<Expr>),
    BinaryOp(BinaryOp, Box<Expr>, Box<Expr>),
    Call(FuncId, Vec<Expr>),
    /// A category named as a function argument (`POSITION(Time)`,
    /// `PREV(x, Time)`); not a value on its own.
    Category(CategoryId),
}

/// How deep a formula's expression tree may nest, a leaf counting as one
/// level (`Price + Price` is 2). Every way a formula enters checks it: the
/// parser (after `LET` and `LAMBDA` expansion), named-LAMBDA expansion, and
/// the CNL parser. Two things bound it:
///
/// * The store keeps a formula as serde_json, which refuses input nested past
///   128 arrays and objects. A call level costs 3 of them (`{"Call":[id,[…]]}`),
///   an operator level 2, the `Formula` wrapper 1 and a measure reference 4,
///   so a store row takes 3 × 41 + 2 = 125 levels at 41 nested calls. A
///   model exported whole (`improv export`) adds 4 more wrappers.
/// * The engine walks the tree recursively; a debug build overflows a
///   2 MiB thread at about 54 nested calls.
pub const MAX_FORMULA_DEPTH: usize = 40;

/// How many nodes a formula's tree may have once `LET`, `LAMBDA` and named
/// LAMBDAs are expanded: each copies its value at every use, so a chain of
/// `LET` bindings that each use the previous one twice doubles per binding.
pub const MAX_FORMULA_NODES: usize = 100_000;

/// How many levels a lazy call adds when the engine rewrites it into nested
/// `IF`s (`engine::compiler::desugar_lazy`), beyond its own: one `IF` per
/// case, plus the case's test (`==`, and `INT` for `CHOOSE`).
fn lazy_levels(f: FuncId, args: usize) -> usize {
    match f.0 {
        31 => (args / 2).saturating_sub(1), // IFS: one IF per test/value pair
        32 => args.saturating_sub(1) / 2,   // SWITCH: one IF per case, + the ==
        33 => args,                         // CHOOSE: one IF per value, + == and INT
        // CLEARERROR/CLEARZERO without an alternative: IF(NOT(test), v).
        36 | 37 if args < 2 => 2,
        34..=37 => 1, // IFERROR & co: IF(ISERROR(v), a, v)
        _ => 0,
    }
}

impl Expr {
    /// The tree's depth as the engine walks it (a leaf is 1; a `SWITCH`,
    /// `IFS` or `CHOOSE` counts the nested `IF`s it compiles to) and its
    /// node count. The walk uses its own stack, so any tree is safe, and
    /// stops once the count passes [`MAX_FORMULA_NODES`].
    pub fn depth_and_nodes(&self) -> (usize, usize) {
        let (mut depth, mut nodes) = (0, 0);
        let mut todo = vec![(self, 1)];
        while let Some((e, d)) = todo.pop() {
            nodes += 1;
            depth = depth.max(d);
            if nodes > MAX_FORMULA_NODES {
                break;
            }
            match e {
                Expr::Literal(_) | Expr::Ref(..) | Expr::Category(_) => {}
                Expr::UnaryOp(_, a) => todo.push((a, d + 1)),
                Expr::BinaryOp(_, a, b) => todo.extend([(&**a, d + 1), (&**b, d + 1)]),
                Expr::Call(f, args) => {
                    let below = d + 1 + lazy_levels(*f, args.len());
                    todo.extend(args.iter().map(|a| (a, below)));
                }
            }
        }
        (depth, nodes)
    }

    /// An error when the tree is deeper than [`MAX_FORMULA_DEPTH`] or larger
    /// than [`MAX_FORMULA_NODES`].
    pub fn check_limits(&self) -> Result<(), String> {
        let (depth, nodes) = self.depth_and_nodes();
        if nodes > MAX_FORMULA_NODES {
            return Err(too_large());
        }
        if depth > MAX_FORMULA_DEPTH {
            return Err(format!(
                "formula nests too deeply ({depth} levels; the limit is {MAX_FORMULA_DEPTH})"
            ));
        }
        Ok(())
    }

    /// Every measure this expression references, in traversal order (with
    /// duplicates). Used to build the dependency graph.
    pub fn referenced_measures(&self) -> Vec<MeasureId> {
        let mut acc = Vec::new();
        self.collect_refs(&mut acc);
        acc
    }

    fn collect_refs(&self, acc: &mut Vec<MeasureId>) {
        match self {
            Expr::Literal(_) | Expr::Category(_) => {}
            Expr::Ref(m, _) => acc.push(*m),
            Expr::UnaryOp(_, e) => e.collect_refs(acc),
            Expr::BinaryOp(_, l, r) => {
                l.collect_refs(acc);
                r.collect_refs(acc);
            }
            Expr::Call(_, args) => {
                for a in args {
                    a.collect_refs(acc);
                }
            }
        }
    }
}

/// The error for a tree past [`MAX_FORMULA_NODES`].
pub(crate) fn too_large() -> String {
    format!(
        "formula is too large (more than {MAX_FORMULA_NODES} terms once LET and LAMBDA \
         are expanded)"
    )
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Formula {
    pub expr: Expr,
}

impl Formula {
    pub fn new(expr: Expr) -> Self {
        Formula { expr }
    }

    pub fn referenced_measures(&self) -> Vec<MeasureId> {
        self.expr.referenced_measures()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn dependency_extraction() {
        // Revenue = Price[Product] * Quantity[Time, Product]
        let f = Formula::new(Expr::BinaryOp(
            BinaryOp::Mul,
            Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
            Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
        ));
        let mut deps = f.referenced_measures();
        deps.sort();
        assert_eq!(deps, vec![MeasureId(100), MeasureId(101)]);
    }
}
