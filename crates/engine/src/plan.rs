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
    /// `IF(test, then, [otherwise])`, evaluated lazily: a cell where the test
    /// is true and `then` has one, where it is false and `otherwise` has one,
    /// and the test's error where it is an error. `spanning` names the branch
    /// (0 then, 1 otherwise) whose coordinates extend the test to the
    /// result's categories; `None` when the test already spans them.
    If {
        test: Box<PlanNode>,
        then: Box<PlanNode>,
        otherwise: Option<Box<PlanNode>>,
        spanning: Option<u8>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanNode {
    pub kind: PlanNodeKind,
    pub ty: TypeInfo,
}
