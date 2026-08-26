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
}

impl Expr {
    /// Every measure this expression references, in traversal order (with
    /// duplicates). Used to build the dependency graph.
    pub fn referenced_measures(&self) -> Vec<MeasureId> {
        let mut acc = Vec::new();
        self.collect_refs(&mut acc);
        acc
    }

    fn collect_refs(&self, acc: &mut Vec<MeasureId>) {
        match self {
            Expr::Literal(_) => {}
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
