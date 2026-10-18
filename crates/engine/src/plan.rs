//! The operator plan: a lowered, dimension-explicit representation of a formula
//! that maps directly onto differential-dataflow operators.
//!
//! Produced by the compiler's second phase (`compiler::build_plan`) and
//! consumed by the dataflow builder (`dataflow::build_collection`).

use crate::typed::TypeInfo;
use improv_core_model::{BinaryOp, CategoryId, FuncId, ItemId, MeasureId, UnaryOp, Value};

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
    /// Reduce `inputs` (aligned on their full coordinate; `pred` filters
    /// them) down to `group_by`. `params` broadcast one value per group.
    /// Rows reach the reduction in item order along `order` (the reduced
    /// categories). With a `window` (one category, kept), one result per
    /// item, each over its window; a [`improv_core_model::functions::Kind::PerItem`]
    /// function also keeps the category.
    Aggregate {
        inputs: Vec<PlanNode>,
        params: Vec<PlanNode>,
        pred: Option<Box<PlanNode>>,
        group_by: Vec<CategoryId>,
        func: FuncId,
        order: Vec<(CategoryId, Vec<ItemId>)>,
        window: Option<crate::typed::Window>,
        per_item: bool,
        /// An array function's landing categories and their items: results
        /// fill them row-major (the last category varies fastest); results
        /// past the last cell are dropped. Empty: not a list.
        into: Vec<(CategoryId, Vec<ItemId>)>,
    },
    /// A static per-item collection of a category: one cell per item.
    Items(CategoryId, Vec<(ItemId, Value)>),
    /// A recurrence along `cat`: per group of the other categories, items in
    /// `order`, each cell is `body` evaluated (by the scalar evaluator) with
    /// `refs` at its coordinate and each slot's placeholder bound to this
    /// measure's own value `back` items earlier (or the slot's default).
    Recurrence {
        body: improv_core_model::Expr,
        refs: Vec<(MeasureId, PlanNode)>,
        slots: Vec<(MeasureId, i64, Option<improv_core_model::Expr>)>,
        cat: CategoryId,
        order: Vec<ItemId>,
    },
    /// `input` moved `offset` items along `cat` (in `order`); where no cell
    /// lands, `default` (when given) at the input's own coordinates.
    Shift {
        input: Box<PlanNode>,
        cat: CategoryId,
        offset: i64,
        order: Vec<ItemId>,
        default: Option<Box<PlanNode>>,
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
