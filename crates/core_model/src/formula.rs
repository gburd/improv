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
/// the CNL parser. Two things bound it (measured 2026-10-24, rustc 1.98):
///
/// * **JSON.** Stores keep a formula as serde_json, which refuses input
///   nested 128 arrays and objects deep. A call level costs 3 of them
///   (`{"Call":[id,[…]]}`), an operator level 2, and a leaf at most 4 (a
///   measure reference, `{"Ref":[id,{"by":[]…}]}`), so a tree d deep is at
///   most 3d + 1 levels. A stored formula adds 1 (`{"expr":…}`) and a model
///   written whole (`improv export`) 5: 3d + 6 ≤ 127 holds to d = 40. Nested
///   `ABS` reads back at 40 and fails at 42 (128 levels); a `+` chain fails
///   at 62.
/// * **Stack.** The engine walks the tree recursively. In a debug build on a
///   2 MiB thread, every shape (`+`, `*`, `-`, `ABS`, `IF`, `SWITCH`)
///   evaluates at depth 54 and overflows at 56: 14 levels of margin.
pub const MAX_FORMULA_DEPTH: usize = 40;

/// How deep a formula that a store kept as **text** may nest when it is
/// loaded (Postgres `improv.measure`, `LANGUAGE improv` bodies): text has no
/// JSON limit, so a formula saved before [`MAX_FORMULA_DEPTH`] existed, up to
/// this deep, keeps working. Named-LAMBDA expansion is held to it too. It is a
/// bound for **release** builds: on the engine's 2 MiB worker thread a
/// release build evaluates `+`, `*` and `-` chains to 800 levels and `ABS`,
/// `IF` and `SWITCH` nesting to 600 (each fails 50 later), more than twice
/// this. A debug build's worker overflows at 56, so debug tests stay under
/// that.
pub const MAX_STORED_FORMULA_DEPTH: usize = 256;

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
    /// than [`MAX_FORMULA_NODES`]: the limits on a new formula.
    pub fn check_limits(&self) -> Result<(), String> {
        self.check_depth(MAX_FORMULA_DEPTH)
    }

    /// An error when the tree is deeper than `max` or larger than
    /// [`MAX_FORMULA_NODES`].
    pub fn check_depth(&self, max: usize) -> Result<(), String> {
        let (depth, nodes) = self.depth_and_nodes();
        if nodes > MAX_FORMULA_NODES {
            return Err(too_large());
        }
        if depth > max {
            return Err(too_deep(depth, max));
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

/// Run `f`, one level of a recursion over a formula's tree, on a new stack
/// segment when this thread's stack is nearly used up (`stacker`). The
/// parser, the compiler and named-LAMBDA expansion recurse once a level (up
/// to 25 KB a level in a debug build) on their callers' threads, which can
/// be as small as 1 MiB (a Windows main thread).
pub fn grow_stack<R>(f: impl FnOnce() -> R) -> R {
    stacker::maybe_grow(256 << 10, 4 << 20, f)
}

/// How the two limit errors start (see [`crate::ParseError::is_limit`]).
pub(crate) const TOO_DEEP: &str = "formula nests too deeply";
pub(crate) const TOO_LARGE: &str = "formula is too large";

/// The error for a tree `depth` levels deep, past `max`.
pub(crate) fn too_deep(depth: usize, max: usize) -> String {
    format!("{TOO_DEEP} ({depth} levels; the limit is {max})")
}

/// The error for a tree past [`MAX_FORMULA_NODES`].
pub(crate) fn too_large() -> String {
    format!("{TOO_LARGE} (more than {MAX_FORMULA_NODES} terms once LET and LAMBDA are expanded)")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Formula {
    pub expr: Expr,
}

impl Formula {
    pub fn new(expr: Expr) -> Self {
        Formula { expr }
    }

    /// A formula as a store reads it back from its JSON ([`crate::json`]):
    /// `None` when it is deeper than [`MAX_STORED_FORMULA_DEPTH`], as one
    /// saved before [`MAX_FORMULA_DEPTH`] existed can be (see
    /// [`crate::Measure::mark_unreadable`]).
    pub fn from_json(json: &str) -> Result<Option<Formula>, serde_json::Error> {
        match crate::json::from_str::<Formula>(json) {
            // Past the load bound, as text-stored formulas are.
            Ok(f) if f.expr.check_depth(MAX_STORED_FORMULA_DEPTH).is_err() => Ok(None),
            Ok(f) => Ok(Some(f)),
            Err(e) if e.to_string().starts_with(crate::json::TOO_DEEP) => Ok(None),
            Err(e) => Err(e),
        }
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

    #[test]
    fn from_json_tells_a_too_deep_formula_from_a_bad_one() {
        let f = Formula::new(Expr::UnaryOp(
            UnaryOp::Neg,
            Box::new(Expr::Literal(Value::Number(1.0))),
        ));
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(Formula::from_json(&json).unwrap(), Some(f));
        // n nested negations: 2n + 3 levels of JSON, n + 1 of tree.
        let deep = |n: usize| {
            format!(
                "{{\"expr\":{}{{\"Literal\":{{\"Number\":1.0}}}}{}}}",
                "{\"UnaryOp\":[\"Neg\",".repeat(n),
                "]}".repeat(n)
            )
        };
        // Past serde_json's own limit (128), within ours.
        let f = Formula::from_json(&deep(70)).unwrap().unwrap();
        assert_eq!(f.expr.depth_and_nodes().0, 71);
        // Past the load bound, and past the JSON bound.
        assert_eq!(
            Formula::from_json(&deep(MAX_STORED_FORMULA_DEPTH)).unwrap(),
            None
        );
        assert_eq!(Formula::from_json(&deep(400)).unwrap(), None);
        assert!(Formula::from_json("not json").is_err());
    }
}
