//! Typed expression layer: the formula AST enriched with inferred value type
//! and dimension (the set of categories a subexpression ranges over).
//!
//! Produced by the compiler's first phase (`compiler::infer`). See
//! AGENT_STEERING.md / IMPROV.txt "Formula compiler".

use improv_core_model::{BinaryOp, CategoryId, FuncId, UnaryOp, Value, ValueType};

/// The dimensionality of a (sub)expression: the categories it is indexed by,
/// kept sorted and deduplicated (canonical form).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dim {
    pub categories: Vec<CategoryId>,
}

impl Dim {
    pub fn scalar() -> Self {
        Dim {
            categories: Vec::new(),
        }
    }

    pub fn of(mut categories: Vec<CategoryId>) -> Self {
        categories.sort();
        categories.dedup();
        Dim { categories }
    }

    pub fn is_scalar(&self) -> bool {
        self.categories.is_empty()
    }

    /// True if `self`'s categories are a subset of `other`'s (broadcastable
    /// into `other`).
    pub fn is_subset_of(&self, other: &Dim) -> bool {
        self.categories.iter().all(|c| other.categories.contains(c))
    }

    /// Union of two dimensions (the result dim when broadcasting).
    pub fn union(&self, other: &Dim) -> Dim {
        let mut cats = self.categories.clone();
        cats.extend(other.categories.iter().copied());
        Dim::of(cats)
    }
}

/// A value type paired with its dimension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeInfo {
    pub value_type: ValueType,
    pub dim: Dim,
}

/// The dimension-projection applied to a measure reference, resolved to the
/// concrete category sets the compiler will keep / aggregate / drop.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ResolvedDimSpec {
    pub by: Vec<CategoryId>,
    pub over: Vec<CategoryId>,
    pub except: Vec<CategoryId>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TypedExprKind {
    Literal(Value),
    Ref(improv_core_model::MeasureId, ResolvedDimSpec),
    UnaryOp(UnaryOp, Box<TypedExpr>),
    BinaryOp(BinaryOp, Box<TypedExpr>, Box<TypedExpr>),
    Call(FuncId, Vec<TypedExpr>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedExpr {
    pub kind: TypedExprKind,
    pub ty: TypeInfo,
}
