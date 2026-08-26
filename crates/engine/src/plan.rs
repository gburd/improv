//! The operator plan: a lowered, dimension-explicit representation of a formula
//! that maps directly onto differential-dataflow operators.
//!
//! Produced by the compiler's second phase (`compiler::build_plan`) and
//! consumed by the dataflow builder (`dataflow::build_collection`).

use crate::typed::TypeInfo;
use improv_core_model::{BinaryOp, CategoryId, FuncId, MeasureId, UnaryOp, Value};

#[derive(Debug, Clone, PartialEq)]
pub enum PlanNodeKind {
    /// The base collection for an input measure.
    InputMeasure(MeasureId),
    /// A constant, broadcast over the target dimension.
    Literal(Value),
    MapUnary(UnaryOp, Box<PlanNode>),
    /// Element-wise binary op. Operands are assumed dimension-aligned (the
    /// compiler inserts `Join` nodes to align them first).
    MapBinary(BinaryOp, Box<PlanNode>, Box<PlanNode>),
    /// Align two collections on shared categories (broadcast the smaller over
    /// the larger). Result dimension is the union.
    Join {
        left: Box<PlanNode>,
        right: Box<PlanNode>,
        /// Categories both sides are joined on.
        join_keys: Vec<CategoryId>,
    },
    /// Aggregate `input` down to `group_by`, collapsing all other categories.
    Aggregate {
        input: Box<PlanNode>,
        group_by: Vec<CategoryId>,
        func: FuncId,
    },
    /// A (non-aggregating) function call over its argument plans.
    FuncCall {
        func: FuncId,
        args: Vec<PlanNode>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanNode {
    pub kind: PlanNodeKind,
    pub ty: TypeInfo,
}
