//! Improv core model: categories, items, measures, coordinates, formulas.
//!
//! GUI-free, storage-free. This is the multidimensional "cube": each measure is
//! a tensor indexed by a subset of categories; a coordinate names one cell.
//!
//! Structure follows the project steering doc (AGENT_STEERING.md / IMPROV.txt).
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod extfn_def;
pub mod formula;
pub mod functions;
pub mod ids;
pub mod json;
pub mod locale;
pub mod parser;
pub mod schedule;
pub mod value;

pub use extfn_def::{ExternalFn, Language};
pub use formula::{
    grow_stack, BinaryOp, DimensionSpec, Expr, Formula, FuncId, UnaryOp, MAX_FORMULA_DEPTH,
    MAX_FORMULA_NODES, MAX_STORED_FORMULA_DEPTH,
};
pub use ids::{CategoryId, ItemId, MeasureId, Name, ScenarioId, ViewId};
pub use locale::Locale;
pub use parser::{
    parse_definition, parse_expr, parse_formula, parse_stored_definition, parse_stored_expr,
    Definition, FormulaText, LambdaText, ParseError,
};
pub use value::{Value, ValueError, ValueErrorKind, ValueType};

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A dimension of the model (e.g. Time, Product, Region).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    pub id: CategoryId,
    pub name: Name,
    pub items: Vec<ItemId>,
}

/// A member of a category (e.g. 2025, "Widget A").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub id: ItemId,
    pub category: CategoryId,
    pub name: Name,
}

/// A coordinate maps categories to items: one cell of a measure's tensor.
///
/// `BTreeMap` gives a stable, ordered key (important for hashing/serialization
/// and, later, for deriving a fixed differential-dataflow key).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Coordinate {
    pub dims: BTreeMap<CategoryId, ItemId>,
}

impl Coordinate {
    pub fn new() -> Self {
        Coordinate {
            dims: BTreeMap::new(),
        }
    }

    pub fn from_pairs(pairs: impl IntoIterator<Item = (CategoryId, ItemId)>) -> Self {
        Coordinate {
            dims: pairs.into_iter().collect(),
        }
    }

    pub fn get(&self, cat: CategoryId) -> Option<ItemId> {
        self.dims.get(&cat).copied()
    }

    pub fn with(mut self, cat: CategoryId, item: ItemId) -> Self {
        self.dims.insert(cat, item);
        self
    }

    /// The set of categories this coordinate is defined over.
    pub fn categories(&self) -> impl Iterator<Item = CategoryId> + '_ {
        self.dims.keys().copied()
    }
}

impl Default for Coordinate {
    fn default() -> Self {
        Self::new()
    }
}

/// A measure is either raw input data or derived from a formula.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum MeasureKind {
    Input,
    Derived(Formula),
}

/// A named variable defined over one or more categories, e.g. `Revenue[Time, Product]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Measure {
    pub id: MeasureId,
    pub name: Name,
    pub value_type: ValueType,
    /// Categories this measure is indexed by (its tensor dimensions).
    pub categories: Vec<CategoryId>,
    pub kind: MeasureKind,
    pub description: Option<String>,
    /// How its numbers are shown (H1-22); `None` is the General form.
    /// Absent in models saved before it existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<NumberFormat>,
}

/// How a number measure's cells are shown (H1-22): a fixed count of
/// decimal places, thousands separators, or both. Presentation only: the
/// values keep every bit, and formulas and exports see them unrounded.
/// Every field has a default, so a format saved by a release that added
/// fields (a percent or a currency form) still reads, without them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NumberFormat {
    /// Digits after the decimal separator; `None` keeps the General form's
    /// (as many as the value needs, at most 15 significant digits).
    #[serde(default)]
    pub decimals: Option<u8>,
    /// Separate thousands with the locale's group separator (`1,500`).
    #[serde(default)]
    pub grouping: bool,
}

impl NumberFormat {
    /// The most decimals a format may ask for: an `f64` carries 15
    /// significant digits.
    pub const MAX_DECIMALS: u8 = 15;

    /// Read a format as it is typed and as [`Display`](std::fmt::Display)
    /// writes it: `general` or a count of decimals (`2`, `2 decimals`),
    /// then `grouped` for thousands separators (`0 grouped`). Case and
    /// commas are ignored. `Ok(None)` is `general` alone: no format.
    pub fn parse(text: &str) -> Result<Option<NumberFormat>, String> {
        let usage = || {
            format!(
                "expected general or 0 to {} decimals, then grouped for thousands \
                 separators (as in: 2 grouped); got {text:?}",
                Self::MAX_DECIMALS
            )
        };
        let mut words = text
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|w| !w.is_empty())
            .map(str::to_ascii_lowercase);
        let first = words.next().ok_or_else(usage)?;
        let mut f = NumberFormat {
            decimals: match first.as_str() {
                "general" => None,
                n => Some(
                    n.parse::<u8>()
                        .ok()
                        .filter(|d| *d <= Self::MAX_DECIMALS)
                        .ok_or_else(usage)?,
                ),
            },
            grouping: false,
        };
        for w in words {
            match w.as_str() {
                "decimal" | "decimals" if f.decimals.is_some() => {}
                "grouped" => f.grouping = true,
                _ => return Err(usage()),
            }
        }
        Ok((f != NumberFormat::default()).then_some(f))
    }
}

impl std::fmt::Display for NumberFormat {
    /// `general`, `1 decimal` or `N decimals`, then ` grouped`: what
    /// [`NumberFormat::parse`] reads back.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.decimals {
            None => f.write_str("general")?,
            Some(1) => f.write_str("1 decimal")?,
            Some(d) => write!(f, "{d} decimals")?,
        }
        if self.grouping {
            f.write_str(" grouped")?;
        }
        Ok(())
    }
}

impl Measure {
    /// Load a derived measure whose stored formula cannot be used (`why`):
    /// one saved before [`MAX_FORMULA_DEPTH`] existed, too deep to read back
    /// or past the limits. Rather than refuse the whole model, the measure
    /// becomes `#REF!` at each of its cells, so its dependents compute as
    /// errors, and its description says why and how to fix it: redefine it
    /// (the store keeps the placeholder at the next save).
    pub fn mark_unreadable(&mut self, why: &str) {
        let ref_error = Expr::Call(
            FuncId(54), // ERROR("#REF!")
            vec![Expr::Literal(Value::Text("#REF!".into()))],
        );
        // `+ POSITION(c)` gives the error a cell at each item of c.
        let expr = self.categories.iter().fold(ref_error, |e, c| {
            let at = Expr::Call(functions::POSITION, vec![Expr::Category(*c)]);
            Expr::BinaryOp(BinaryOp::Add, Box::new(e), Box::new(at))
        });
        self.kind = MeasureKind::Derived(Formula::new(expr));
        let note = format!(
            "[unreadable formula] {} (measure {} {}); it is #REF! until redefined: \
             split it into intermediate measures, at most {MAX_FORMULA_DEPTH} levels deep.",
            why, self.id.0, self.name.0
        );
        self.description = Some(match self.description.take() {
            Some(d) => format!("{note} {d}"),
            None => note,
        });
    }

    pub fn is_input(&self) -> bool {
        matches!(self.kind, MeasureKind::Input)
    }
    pub fn is_derived(&self) -> bool {
        matches!(self.kind, MeasureKind::Derived(_))
    }
}

/// The whole model: the multidimensional cube plus its raw input data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub categories: HashMap<CategoryId, Category>,
    pub items: HashMap<ItemId, Item>,
    pub measures: HashMap<MeasureId, Measure>,
    /// Raw input data: `(measure, coordinate) -> value`. Only `Input` measures
    /// have entries here; derived measures are computed by the engine.
    ///
    /// Serialized as a sequence because JSON object keys must be strings and
    /// this map is keyed by a `(MeasureId, Coordinate)` tuple.
    #[serde(with = "inputs_as_seq")]
    pub inputs: HashMap<(MeasureId, Coordinate), Value>,
    /// Metadata marking which input measures are backed by an external SQL
    /// query (Phase 7). The measure's `kind` stays `Input` — to the engine an
    /// SQL measure is an ordinary input whose cells are (re)populated by
    /// refreshing the query, so the deterministic core gains no SQL path. This
    /// map records *how* to refresh; the actual query run lives in
    /// `improv_storage_sql`.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub sql_sources: HashMap<MeasureId, SqlSource>,
    /// Saved views: named pivot layouts over the model (parity with
    /// Improv/Quantrix "multiple views per model"). A view is presentation, not
    /// modeling — it does not change measures or data.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub views: HashMap<ViewId, View>,
    /// External-language function definitions (Phase 6) available to formulas
    /// via `CALL(name, ...)`, keyed by name. Plain data; the `improv_extfn`
    /// crate evaluates them. Marked non-deterministic-source-adjacent: the
    /// engine treats a call as pure per the author's purity assertion.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_fns: HashMap<String, ExternalFn>,
    /// Measures computed by calling an external function per coordinate over
    /// argument measures (Phase 6). The measure's `kind` stays `Input`; a
    /// host-side refresh (in `improv_engine::external`) runs the function and
    /// populates its cells — the engine's dataflow gains no external-call path,
    /// so its determinism is preserved.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub external_calls: HashMap<MeasureId, ExternalCall>,
    /// What-if **scenarios**: named sets of input-cell overrides layered on the
    /// base model. First-class in the Improv sense — a scenario changes only
    /// which *input* values are fed to the engine, so evaluating "under" a
    /// scenario stays fully deterministic (the engine sees ordinary inputs).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scenarios: HashMap<ScenarioId, Scenario>,
    /// Named LAMBDAs (Excel's Name Manager functions): `Margin = LAMBDA(p,
    /// c, (p - c) / p)`, called as `Margin(Price, Cost)` in any formula. A
    /// call is stored by the lambda's id ([`functions::named_lambda`]) and
    /// expanded when compiled, so editing the lambda changes every use.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub lambdas: BTreeMap<u32, NamedLambda>,
    /// Name → id hints for the `*_by_name` lookups. Not saved: built on the
    /// first lookup, so every load path gets them.
    #[serde(skip)]
    names: NameHints,
}

/// Name → id hints for [`Model::measure_by_name`] and
/// [`Model::category_by_name`], so a bulk define resolves names in O(1)
/// instead of scanning every measure. Built from the maps on the first
/// lookup; [`Model::add_measure`] and [`Model::add_category`] keep a built
/// one current. `measures` and `categories` are pub and can change behind
/// the hints' back, so a lookup trusts a hint only after checking that its
/// id still has that name, and scans when there is no hint or a stale one:
/// a hint saves time and never changes the answer. Ignored by `==`, so a
/// loaded model still equals the one saved.
#[derive(Clone, Default)]
struct NameHints(std::sync::OnceLock<Hints>);

#[derive(Clone, Default)]
struct Hints {
    measures: HashMap<String, MeasureId>,
    categories: HashMap<String, CategoryId>,
}

impl NameHints {
    fn get(&self, model: &Model) -> &Hints {
        self.0.get_or_init(|| Hints {
            measures: model
                .measures
                .values()
                .map(|m| (m.name.0.clone(), m.id))
                .collect(),
            categories: model
                .categories
                .values()
                .map(|c| (c.name.0.clone(), c.id))
                .collect(),
        })
    }
}

impl PartialEq for NameHints {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for NameHints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NameHints")
    }
}

/// A named LAMBDA: its parameters and its body, in which parameter `i` is
/// the reference [`functions::lambda_param`]`(i)`, or, where the body uses
/// it as a category (`MAX(x OVER c)`), the category
/// [`functions::lambda_category`]`(i)`. A body reads only its parameters
/// (no measures), so a call depends on just its arguments; it may call
/// lambdas defined before it, never itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NamedLambda {
    pub name: Name,
    pub params: Vec<String>,
    pub body: Expr,
}

/// How a named LAMBDA's body uses a parameter, which says what a call
/// passes for it ([`NamedLambda::param_kinds`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LambdaParam {
    /// A value: any expression.
    Value,
    /// Reduced over a category (`MAX(x OVER c)`): a measure reference.
    Reduced,
    /// A category (`OVER c`, `POSITION(c)`): a category name.
    Category,
}

impl NamedLambda {
    /// What each parameter takes, from how the body uses it. An error names
    /// a parameter used both as a value and as a category.
    pub fn param_kinds(&self) -> Result<Vec<LambdaParam>, String> {
        let mut uses = vec![ParamUse::default(); self.params.len()];
        param_uses(&self.body, &mut uses);
        self.params
            .iter()
            .zip(uses)
            .map(|(p, u)| match u {
                ParamUse {
                    value: true,
                    category: true,
                    ..
                } => Err(format!(
                    "LAMBDA parameter {p} is used both as a value and as a category"
                )),
                ParamUse { category: true, .. } => Ok(LambdaParam::Category),
                ParamUse { reduced: true, .. } => Ok(LambdaParam::Reduced),
                _ => Ok(LambdaParam::Value),
            })
            .collect()
    }
}

/// How a body uses one parameter (see [`NamedLambda::param_kinds`]).
#[derive(Debug, Clone, Copy, Default)]
struct ParamUse {
    value: bool,
    reduced: bool,
    category: bool,
}

fn param_uses(e: &Expr, uses: &mut [ParamUse]) {
    let cat = |c: &CategoryId, uses: &mut [ParamUse]| {
        if let Some(u) = functions::lambda_category_of(*c).and_then(|i| uses.get_mut(i)) {
            u.category = true;
        }
    };
    match e {
        Expr::Ref(id, s) => {
            for c in s.by.iter().chain(&s.over).chain(&s.except) {
                cat(c, uses);
            }
            if let Some(u) = functions::lambda_param_of(*id).and_then(|i| uses.get_mut(i)) {
                u.value = true;
                u.reduced |= *s != DimensionSpec::default();
            }
        }
        Expr::Category(c) => cat(c, uses),
        Expr::UnaryOp(_, a) => param_uses(a, uses),
        Expr::BinaryOp(_, a, b) => {
            param_uses(a, uses);
            param_uses(b, uses);
        }
        Expr::Call(_, args) => args.iter().for_each(|a| param_uses(a, uses)),
        Expr::Literal(_) => {}
    }
}

/// A what-if scenario: a named overlay of input-cell overrides on the base
/// model. Applying it produces a model whose `inputs` are the base inputs with
/// these overrides taking precedence; derived measures then recompute normally.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scenario {
    pub id: ScenarioId,
    pub name: Name,
    /// Input-cell overrides: `(measure, coordinate) -> value`. Only `Input`
    /// measures are meaningful here; an override on a derived measure is
    /// ignored by the engine (derived cells are computed).
    #[serde(with = "inputs_as_seq")]
    pub overrides: HashMap<(MeasureId, Coordinate), Value>,
}

/// A measure defined as `func(arg_measures...)` where `func` is a registered
/// external function. Evaluated host-side per coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExternalCall {
    /// Name of the external function (a key in `Model.external_fns`).
    pub func: String,
    /// The measures supplying the function's positional arguments, aligned to
    /// the function's declared `arg_types`.
    pub arg_measures: Vec<MeasureId>,
    /// When this measure should be refreshed (default `Manual`). Advisory
    /// metadata a scheduler consults; see `RefreshPolicy`.
    #[serde(default)]
    pub refresh_policy: RefreshPolicy,
}
/// Where a matrix sits on a view's canvas: position of its top-left corner and
/// its size, in abstract layout units (the GUI treats them as egui logical
/// points; nothing here depends on egui — `core_model` is GUI-free).
///
/// `Default` is a sane starting rectangle, not a zero one: a zero-sized matrix
/// is never what a caller wants, and a legacy view deserialized without
/// geometry has to land somewhere visible.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CanvasRect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Default for CanvasRect {
    fn default() -> Self {
        // ponytail: fixed default size; make it measure-aware (columns x rows)
        // if auto-layout of a freshly placed matrix matters.
        CanvasRect {
            x: 0.0,
            y: 0.0,
            w: 560.0,
            h: 420.0,
        }
    }
}

/// One matrix on a view's canvas: a measure, that matrix's own pivot layout,
/// and where it sits (`rect`). A view holds several of these — Quantrix's
/// free-form canvas, where each matrix is independently pivoted (see
/// `docs/reviews/2026-09-22-gui-reconstruction-plan.md` Step 3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatrixPlacement {
    pub measure: MeasureId,
    /// This matrix's axis permutation; see `View::axis_order`.
    #[serde(default)]
    pub axis_order: Vec<CategoryId>,
    /// Categories stacked on this matrix's row axis.
    #[serde(default = "one")]
    pub n_rows: usize,
    /// Categories stacked on this matrix's column axis.
    #[serde(default = "one")]
    pub n_cols: usize,
    /// Pinned page items for this matrix.
    #[serde(default)]
    pub page_items: Vec<(CategoryId, ItemId)>,
    /// This matrix's own filters.
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Canvas geometry.
    #[serde(default)]
    pub rect: CanvasRect,
}

impl MatrixPlacement {
    /// A placement of `measure` with default layout and geometry.
    pub fn new(measure: MeasureId) -> Self {
        MatrixPlacement {
            measure,
            axis_order: Vec::new(),
            n_rows: 1,
            n_cols: 1,
            page_items: Vec::new(),
            filters: Vec::new(),
            rect: CanvasRect::default(),
        }
    }

    /// Does `item` in `category` pass THIS matrix's filters? Categories without
    /// a filter always pass. Same rule as `View::allows`, applied per matrix.
    pub fn allows(&self, category: CategoryId, item: ItemId) -> bool {
        match self.filters.iter().find(|f| f.category == category) {
            Some(f) => f.items.contains(&item),
            None => true,
        }
    }
}

/// A saved canvas layout: one *primary* matrix described by the flat fields
/// below, plus any number of `placements` (additional matrices), each with its
/// own measure, pivot and geometry.
///
/// The flat fields (`measure`, `axis_order`, `n_rows`, `n_cols`, `page_items`,
/// `filters`) and `rect` ARE the primary matrix — not a cache of
/// `placements[0]`. `placements` holds only the *extra* matrices, so every
/// matrix is stored exactly once and there is no way for two copies of the same
/// matrix to disagree. Use `View::matrices()` to iterate all of them uniformly.
///
/// This is what keeps pre-canvas saved views loading unchanged: a view written
/// before canvases has no `placements`, defaults to none, and is therefore a
/// one-matrix canvas whose single matrix is exactly what it always was.
/// Reusable across sessions; loading a view reproduces a layout without
/// touching formulas or data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct View {
    pub id: ViewId,
    pub name: Name,
    /// The primary matrix's measure.
    pub measure: MeasureId,
    /// A permutation of the measure's categories: the first `n_rows` are
    /// stacked on rows (outer→inner), the next `n_cols` on columns, the rest
    /// are pages. Empty = use the measure's natural order. (Pre-stacking views
    /// have no `n_rows`/`n_cols`; they default to 1/1 = one category per axis.)
    #[serde(default)]
    pub axis_order: Vec<CategoryId>,
    /// How many leading `axis_order` categories are stacked on the row axis.
    #[serde(default = "one")]
    pub n_rows: usize,
    /// How many `axis_order` categories (after the rows) are stacked on the
    /// column axis.
    #[serde(default = "one")]
    pub n_cols: usize,
    /// For each page (extra) dimension, the pinned item.
    #[serde(default)]
    pub page_items: Vec<(CategoryId, ItemId)>,
    /// Per-category filters: restrict a dimension to this subset of items.
    /// A category absent here is unfiltered (all items shown).
    #[serde(default)]
    pub filters: Vec<Filter>,
    /// Where the primary matrix sits on the canvas. (Pre-canvas views have no
    /// geometry; they default to `CanvasRect::default()`.)
    #[serde(default)]
    pub rect: CanvasRect,
    /// Additional matrices on this canvas, beyond the primary one described by
    /// the fields above. Empty (the default, and what every pre-canvas saved
    /// view deserializes to) = a single-matrix view.
    #[serde(default)]
    pub placements: Vec<MatrixPlacement>,
}

/// Serde default for `n_rows`/`n_cols` on `View` and `MatrixPlacement`: one
/// category per axis.
fn one() -> usize {
    1
}

/// Restrict a category to a subset of its items in a view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub category: CategoryId,
    /// Items to KEEP. An empty list means "no items pass" (an explicit empty
    /// filter); omit the `Filter` entirely to show all items.
    pub items: Vec<ItemId>,
}

impl View {
    /// Build a view from its matrices: the first is the primary one (the flat
    /// fields), the rest become `placements`.
    pub fn from_matrices(
        id: ViewId,
        name: Name,
        primary: MatrixPlacement,
        extras: Vec<MatrixPlacement>,
    ) -> Self {
        View {
            id,
            name,
            measure: primary.measure,
            axis_order: primary.axis_order,
            n_rows: primary.n_rows,
            n_cols: primary.n_cols,
            page_items: primary.page_items,
            filters: primary.filters,
            rect: primary.rect,
            placements: extras,
        }
    }

    /// The primary matrix, as a `MatrixPlacement` (the flat fields viewed
    /// uniformly).
    pub fn primary(&self) -> MatrixPlacement {
        MatrixPlacement {
            measure: self.measure,
            axis_order: self.axis_order.clone(),
            n_rows: self.n_rows,
            n_cols: self.n_cols,
            page_items: self.page_items.clone(),
            filters: self.filters.clone(),
            rect: self.rect,
        }
    }

    /// Every matrix on this canvas, primary first. Always non-empty.
    pub fn matrices(&self) -> Vec<MatrixPlacement> {
        let mut v = Vec::with_capacity(1 + self.placements.len());
        v.push(self.primary());
        v.extend(self.placements.iter().cloned());
        v
    }

    /// Does `item` in `category` pass the PRIMARY matrix's filters? Categories
    /// without a filter always pass.
    ///
    /// Unchanged semantics from before canvases: filtering is per matrix, and
    /// this is the primary matrix's filter set (identical to the whole view's,
    /// for a single-matrix view). For another matrix use
    /// `MatrixPlacement::allows`.
    pub fn allows(&self, category: CategoryId, item: ItemId) -> bool {
        self.primary().allows(category, item)
    }
}

/// How to refresh an SQL-backed input measure: the query and how its columns
/// map to the measure's dimensions + value. A live-query measure marked here
/// is nondeterministic (its data comes from outside); determinism tests treat
/// only measures WITHOUT an entry here as pure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlSource {
    /// The `SELECT` to run on refresh.
    pub query: String,
    /// Result column names, in the same order as the measure's `categories`,
    /// whose distinct values are the items of each dimension.
    pub dimension_columns: Vec<String>,
    /// Result column holding the numeric value.
    pub value_column: String,
    /// When this measure should be refreshed. Defaults to `Manual` (only when
    /// asked). This is *policy metadata*; a caller (CLI `refresh-all`, a future
    /// scheduler) decides when to honor it — the deterministic engine core is
    /// untouched either way.
    #[serde(default)]
    pub refresh_policy: RefreshPolicy,
}

/// When an external-sourced measure (SQL or external-function) should refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RefreshPolicy {
    /// Only when explicitly asked (the safe default).
    #[default]
    Manual,
    /// Whenever the model is loaded/opened by a tool that honors the policy.
    OnLoad,
    /// At most every `secs` seconds (advisory; the honoring tool tracks time).
    Interval { secs: u64 },
}

/// serde adapter: (de)serialize the tuple-keyed `inputs` map as a `Vec` of
/// `(key, value)` pairs so it survives JSON (and any string-keyed format).
/// `body` with parameter `i` replaced by `args[i]`: a value parameter by the
/// argument; a reduced one (`x OVER c`) by the argument's measure reference,
/// carrying the body's `OVER`; a category placeholder by the argument's
/// category.
fn substitute_params(body: &Expr, args: &[Expr]) -> Result<Expr, String> {
    grow_stack(|| substitute_here(body, args))
}

fn substitute_here(body: &Expr, args: &[Expr]) -> Result<Expr, String> {
    let cat = |c: &CategoryId| match functions::lambda_category_of(*c) {
        None => Ok(*c),
        Some(i) => match args.get(i) {
            Some(Expr::Category(a)) => Ok(*a),
            _ => Err(format!("argument {} must be a category", i + 1)),
        },
    };
    let cats = |v: &[CategoryId]| v.iter().map(cat).collect::<Result<Vec<_>, String>>();
    let sub = |e: &Expr| substitute_params(e, args);
    Ok(match body {
        Expr::Ref(id, spec) => {
            let spec = DimensionSpec {
                by: cats(&spec.by)?,
                over: cats(&spec.over)?,
                except: cats(&spec.except)?,
            };
            match functions::lambda_param_of(*id).and_then(|i| Some((i, args.get(i)?))) {
                None => Expr::Ref(*id, spec),
                Some((_, a)) if spec == DimensionSpec::default() => a.clone(),
                Some((_, Expr::Ref(m, a))) => {
                    let or = |b: Vec<CategoryId>, a: &[CategoryId]| {
                        if b.is_empty() {
                            a.to_vec()
                        } else {
                            b
                        }
                    };
                    Expr::Ref(
                        *m,
                        DimensionSpec {
                            by: or(spec.by, &a.by),
                            over: or(spec.over, &a.over),
                            except: or(spec.except, &a.except),
                        },
                    )
                }
                Some((i, _)) => {
                    return Err(format!(
                        "argument {} is reduced over a category, so it must be a measure",
                        i + 1
                    ))
                }
            }
        }
        Expr::Category(c) => Expr::Category(cat(c)?),
        Expr::Call(f, a) => Expr::Call(*f, a.iter().map(sub).collect::<Result<_, _>>()?),
        Expr::UnaryOp(o, a) => Expr::UnaryOp(*o, Box::new(sub(a)?)),
        Expr::BinaryOp(o, a, b) => Expr::BinaryOp(*o, Box::new(sub(a)?), Box::new(sub(b)?)),
        Expr::Literal(_) => body.clone(),
    })
}

/// Whether `e` calls one of the named LAMBDAs `ids`.
fn calls_lambda(e: &Expr, ids: &BTreeSet<u32>) -> bool {
    match e {
        Expr::Call(f, a) => {
            functions::named_lambda_of(*f).is_some_and(|i| ids.contains(&i))
                || a.iter().any(|x| calls_lambda(x, ids))
        }
        Expr::UnaryOp(_, a) => calls_lambda(a, ids),
        Expr::BinaryOp(_, a, b) => calls_lambda(a, ids) || calls_lambda(b, ids),
        Expr::Literal(_) | Expr::Ref(..) | Expr::Category(_) => false,
    }
}

mod inputs_as_seq {
    use super::{Coordinate, MeasureId, Value};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    use std::collections::HashMap;

    type Map = HashMap<(MeasureId, Coordinate), Value>;

    pub fn serialize<S: Serializer>(map: &Map, s: S) -> Result<S::Ok, S::Error> {
        let v: Vec<(&(MeasureId, Coordinate), &Value)> = map.iter().collect();
        v.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Map, D::Error> {
        let v: Vec<((MeasureId, Coordinate), Value)> = Vec::deserialize(d)?;
        Ok(v.into_iter().collect())
    }
}

impl Model {
    pub fn new() -> Self {
        Model::default()
    }

    /// This model plus a provisional input measure `id` named `name` (when
    /// no measure has the name), so a definition of a new measure can refer
    /// to itself (`Bal = PREV(Bal, Time, 1, 0) + Flow`) while it is parsed.
    pub fn with_provisional(&self, id: MeasureId, name: &str) -> std::borrow::Cow<'_, Model> {
        if self.measure_by_name(name).is_some() {
            return std::borrow::Cow::Borrowed(self);
        }
        let mut m = self.clone();
        m.add_measure(Measure {
            id,
            name: Name(name.to_string()),
            value_type: ValueType::Number,
            categories: Vec::new(),
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
        std::borrow::Cow::Owned(m)
    }

    /// The named LAMBDA called `name` (case-insensitive), with its id.
    pub fn lambda_by_name(&self, name: &str) -> Option<(u32, &NamedLambda)> {
        self.lambdas
            .iter()
            .find(|(_, l)| l.name.0.eq_ignore_ascii_case(name))
            .map(|(id, l)| (*id, l))
    }

    /// What calls named LAMBDA `id`, directly or through other lambdas: the
    /// other lambdas' ids and the derived measures, each sorted.
    pub fn lambda_users(&self, id: u32) -> (Vec<u32>, Vec<MeasureId>) {
        let mut hit = BTreeSet::from([id]);
        loop {
            let before = hit.len();
            for (lid, l) in &self.lambdas {
                if !hit.contains(lid) && calls_lambda(&l.body, &hit) {
                    hit.insert(*lid);
                }
            }
            if hit.len() == before {
                break;
            }
        }
        let measures = self
            .measures
            .values()
            .filter(|m| matches!(&m.kind, MeasureKind::Derived(f) if calls_lambda(&f.expr, &hit)))
            .map(|m| m.id)
            .collect::<BTreeSet<_>>();
        hit.remove(&id);
        (hit.into_iter().collect(), measures.into_iter().collect())
    }

    /// `e` with every named-LAMBDA call replaced by the lambda's body, its
    /// parameters bound to the call's arguments (nested calls too). Errors
    /// name an unknown lambda or a wrong argument count.
    pub fn expand_lambdas(&self, e: &Expr) -> Result<Expr, String> {
        // The compiler expands stored formulas too, so this is the load
        // bound; the parser holds a new formula's expansion to the new one.
        let e = self.expand_at(e, 0)?;
        e.check_depth(MAX_STORED_FORMULA_DEPTH)?;
        Ok(e)
    }

    fn expand_at(&self, e: &Expr, depth: usize) -> Result<Expr, String> {
        grow_stack(|| self.expand_here(e, depth))
    }

    fn expand_here(&self, e: &Expr, depth: usize) -> Result<Expr, String> {
        // Lambdas only call earlier ones, so depth is bounded by their
        // number; the guard catches a hand-edited cycle.
        if depth > self.lambdas.len() + 1 {
            return Err("named LAMBDAs call each other in a cycle".into());
        }
        Ok(match e {
            Expr::Call(f, args) => {
                let args = args
                    .iter()
                    .map(|a| self.expand_at(a, depth))
                    .collect::<Result<Vec<_>, _>>()?;
                match functions::named_lambda_of(*f) {
                    None => Expr::Call(*f, args),
                    Some(id) => {
                        let l = self
                            .lambdas
                            .get(&id)
                            .ok_or_else(|| format!("named LAMBDA {id} does not exist"))?;
                        if l.params.len() != args.len() {
                            return Err(format!(
                                "{} takes {} argument(s), got {}",
                                l.name.0,
                                l.params.len(),
                                args.len()
                            ));
                        }
                        let body = substitute_params(&l.body, &args)
                            .map_err(|e| format!("{}: {e}", l.name.0))?;
                        // Lambdas calling lambdas can double per level: stop early.
                        body.check_depth(MAX_STORED_FORMULA_DEPTH)
                            .map_err(|e| format!("{}: {e}", l.name.0))?;
                        self.expand_at(&body, depth + 1)?
                    }
                }
            }
            Expr::UnaryOp(o, a) => Expr::UnaryOp(*o, Box::new(self.expand_at(a, depth)?)),
            Expr::BinaryOp(o, a, b) => Expr::BinaryOp(
                *o,
                Box::new(self.expand_at(a, depth)?),
                Box::new(self.expand_at(b, depth)?),
            ),
            other => other.clone(),
        })
    }

    /// Add the named LAMBDAs stored as `(id, definition)` rows, each a JSON
    /// [`NamedLambda`] or `Name = LAMBDA(…)` text. Text calls other lambdas
    /// by name, and a redefined lambda may call one with a higher id, so the
    /// text rows are parsed in passes until all are in. An error names the
    /// row that never parsed.
    pub fn load_lambdas(&mut self, rows: &[(u32, String)]) -> Result<(), String> {
        let mut todo = Vec::new();
        for (id, text) in rows {
            match json::from_str::<NamedLambda>(text) {
                Ok(l) => {
                    self.lambdas.insert(*id, l);
                }
                Err(_) => todo.push((*id, text)),
            }
        }
        while !todo.is_empty() {
            let before = todo.len();
            let mut err = None;
            todo.retain(|(id, text)| match parse_stored_definition(self, text) {
                Ok(Definition::Lambda(lt)) => {
                    self.lambdas.insert(*id, lt.lambda);
                    false
                }
                Ok(_) => {
                    err = Some(format!("lambda {id}: {text:?} is not a LAMBDA"));
                    true
                }
                Err(e) => {
                    err.get_or_insert(format!("lambda {id}: {e}"));
                    true
                }
            });
            if todo.len() == before {
                return Err(err.unwrap_or_default());
            }
        }
        Ok(())
    }

    // --- builders (return the new id for convenience) ---

    pub fn add_category(&mut self, id: CategoryId, name: impl Into<String>) {
        let name = name.into();
        if let Some(h) = self.names.0.get_mut() {
            h.categories.insert(name.clone(), id);
        }
        self.categories.insert(
            id,
            Category {
                id,
                name: Name(name),
                items: Vec::new(),
            },
        );
    }

    /// Add (or replace) item `id` in `category`, appending it to the
    /// category's item order when it is not there yet. Re-adding an id under
    /// a different category leaves it in the old category's list too.
    pub fn add_item(&mut self, id: ItemId, category: CategoryId, name: impl Into<String>) {
        let old = self.items.insert(
            id,
            Item {
                id,
                category,
                name: Name(name.into()),
            },
        );
        if let Some(c) = self.categories.get_mut(&category) {
            // Every listed id is in `items` (the model's invariant), so an id
            // new to `items` is in no list: only a re-add pays the O(n) scan
            // (a scan per insert made bulk loads O(n²)).
            if old.is_none() || !c.items.contains(&id) {
                c.items.push(id);
            }
        }
    }

    pub fn add_measure(&mut self, m: Measure) {
        if let Some(h) = self.names.0.get_mut() {
            h.measures.insert(m.name.0.clone(), m.id);
        }
        self.measures.insert(m.id, m);
    }

    pub fn set_input(&mut self, measure: MeasureId, coord: Coordinate, value: Value) {
        self.inputs.insert((measure, coord), value);
    }

    pub fn input(&self, measure: MeasureId, coord: &Coordinate) -> Option<&Value> {
        self.inputs.get(&(measure, coord.clone()))
    }

    /// The transitive closure of every measure `roots` depends on: for each
    /// derived measure, its formula's `referenced_measures()`; for an
    /// external-call measure (`Model.external_calls`), its `arg_measures`.
    /// Includes `roots` themselves. Storage-free, engine-free — a `Model`
    /// loader (e.g. `storage_mentat::ModelStore::load_partial`) uses this to
    /// load only the input cells an operation over `roots` actually needs,
    /// instead of the whole model (see
    /// `.agent/steering/AGENT_OUT_OF_CORE_DESIGN.md` §4).
    pub fn measure_dependency_closure(
        &self,
        roots: &[MeasureId],
    ) -> std::collections::HashSet<MeasureId> {
        let mut seen: std::collections::HashSet<MeasureId> = roots.iter().copied().collect();
        let mut frontier: Vec<MeasureId> = roots.to_vec();
        while let Some(m) = frontier.pop() {
            let mut deps: Vec<MeasureId> = Vec::new();
            if let Some(measure) = self.measures.get(&m) {
                if let MeasureKind::Derived(f) = &measure.kind {
                    deps.extend(f.referenced_measures());
                }
            }
            if let Some(call) = self.external_calls.get(&m) {
                deps.extend(call.arg_measures.iter().copied());
            }
            for d in deps {
                if seen.insert(d) {
                    frontier.push(d);
                }
            }
        }
        seen
    }

    /// Look up a measure by its human name.
    // shortcut: a name with no hint (absent, or put into the pub map
    // directly after the first lookup) still scans; make the maps private
    // and keep an exact index if misses show up in a profile.
    pub fn measure_by_name(&self, name: &str) -> Option<&Measure> {
        let hint = self.names.get(self).measures.get(name);
        hint.and_then(|id| self.measures.get(id))
            .filter(|m| m.name.0 == name)
            .or_else(|| self.measures.values().find(|m| m.name.0 == name))
    }

    /// Look up a category by its human name.
    pub fn category_by_name(&self, name: &str) -> Option<&Category> {
        let hint = self.names.get(self).categories.get(name);
        hint.and_then(|id| self.categories.get(id))
            .filter(|c| c.name.0 == name)
            .or_else(|| self.categories.values().find(|c| c.name.0 == name))
    }

    /// Save (or replace) a view.
    pub fn add_view(&mut self, v: View) {
        self.views.insert(v.id, v);
    }

    /// Look up a view by its human name.
    pub fn view_by_name(&self, name: &str) -> Option<&View> {
        self.views.values().find(|v| v.name.0 == name)
    }

    /// Register a what-if scenario.
    pub fn add_scenario(&mut self, s: Scenario) {
        self.scenarios.insert(s.id, s);
    }

    /// Look up a scenario by its human name.
    pub fn scenario_by_name(&self, name: &str) -> Option<&Scenario> {
        self.scenarios.values().find(|s| s.name.0 == name)
    }

    /// Produce a model with a scenario's input overrides applied on top of the
    /// base inputs (overrides win). Structure, formulas, and every other input
    /// are unchanged — only the overridden `(measure, coordinate)` cells differ,
    /// so the returned model evaluates deterministically like any other. The
    /// scenario set itself is cleared on the overlay (an overlay is a concrete
    /// what-if world, not a base to re-branch). Returns the base model unchanged
    /// if no scenario by that id exists.
    pub fn with_scenario(&self, id: ScenarioId) -> Model {
        let mut m = self.clone();
        m.scenarios.clear();
        if let Some(s) = self.scenarios.get(&id) {
            for (key, val) in &s.overrides {
                m.inputs.insert(key.clone(), val.clone());
            }
        }
        m
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// H1-22: a format reads as it is typed and as it prints; an old
    /// measure's JSON (no `format`) and a newer format's (more fields) load.
    #[test]
    fn number_formats_parse_print_and_load() {
        let f = |decimals, grouping| Some(NumberFormat { decimals, grouping });
        for (text, want) in [
            ("2", f(Some(2), false)),
            ("2 decimals", f(Some(2), false)),
            ("1 decimal", f(Some(1), false)),
            ("0 grouped", f(Some(0), true)),
            ("General, Grouped", f(None, true)),
            ("  15  ", f(Some(15), false)),
            ("general", None),
        ] {
            let got = NumberFormat::parse(text).unwrap();
            assert_eq!(got, want, "{text:?}");
            if let Some(g) = got {
                assert_eq!(NumberFormat::parse(&g.to_string()), Ok(Some(g)));
            }
        }
        for bad in ["", "16", "-1", "two", "general decimals", "2 grouped x"] {
            let e = NumberFormat::parse(bad).unwrap_err();
            assert!(e.contains("expected general or 0 to 15"), "{bad:?}: {e}");
        }
        assert_eq!(NumberFormat::default().to_string(), "general");
        assert_eq!(f(Some(2), true).unwrap().to_string(), "2 decimals grouped");

        let m = Measure {
            id: MeasureId(1),
            name: Name("X".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Input,
            description: None,
            format: None,
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("format"), "no format, no field: {json}");
        assert_eq!(serde_json::from_str::<Measure>(&json).unwrap(), m);
        let newer = json.replace(
            "\"description\":null",
            "\"description\":null,\"format\":{\"decimals\":2,\"percent\":true}",
        );
        let read: Measure = serde_json::from_str(&newer).unwrap();
        assert_eq!(read.format, f(Some(2), false));
        // A field a later release adds to a measure is ignored.
        let later = json.replace("\"description\":null", "\"description\":null,\"later\":1");
        assert_eq!(serde_json::from_str::<Measure>(&later).unwrap(), m);
    }

    #[test]
    fn an_unreadable_formula_becomes_ref_errors_over_its_categories() {
        let mut m = Measure {
            id: MeasureId(7),
            name: Name("Q".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(1), CategoryId(2)],
            kind: MeasureKind::Input,
            description: Some("mine".into()),
            format: None,
        };
        m.mark_unreadable("its stored formula nests too deeply to load");
        let MeasureKind::Derived(f) = &m.kind else {
            panic!("{m:?}")
        };
        let model = Model::new();
        let mut want = parse_expr(&model, "ERROR(\"#REF!\")").unwrap().expr;
        for c in [1, 2] {
            let at = Expr::Call(functions::POSITION, vec![Expr::Category(CategoryId(c))]);
            want = Expr::BinaryOp(BinaryOp::Add, Box::new(want), Box::new(at));
        }
        assert_eq!(f.expr, want);
        let d = m.description.as_deref().unwrap();
        assert!(
            d.starts_with(
                "[unreadable formula] its stored formula nests too deeply to load (measure 7 Q)"
            ),
            "{d}"
        );
        assert!(d.ends_with(" mine"), "{d}");
        // Without a description of its own, the note is all of it.
        m.description = None;
        m.mark_unreadable("x");
        assert!(m.description.unwrap().ends_with("levels deep."));
    }

    // A tiny Time x Product revenue model, matching the steering doc's example.
    fn time_product_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(11), time, "2026");
        m.add_item(ItemId(20), product, "Widget A");
        m.add_item(ItemId(21), product, "Widget B");

        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: Some("Unit price per product".into()),
            format: None,
        });
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m
    }

    #[test]
    fn coordinate_is_order_independent() {
        let a = Coordinate::from_pairs([(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))]);
        let b = Coordinate::from_pairs([(CategoryId(2), ItemId(20)), (CategoryId(1), ItemId(10))]);
        assert_eq!(a, b, "BTreeMap key is insertion-order independent");
        assert_eq!(a.get(CategoryId(1)), Some(ItemId(10)));
    }

    #[test]
    fn model_build_and_lookup() {
        let m = time_product_model();
        assert_eq!(m.category_by_name("Time").unwrap().items.len(), 2);
        assert!(m.measure_by_name("Price").unwrap().is_input());
        let coord = Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
        assert_eq!(m.input(MeasureId(100), &coord), Some(&Value::Number(10.0)));
    }

    #[test]
    fn name_lookups_survive_direct_edits_of_the_pub_maps() {
        let num = |id: u32, name: &str| Measure {
            id: MeasureId(id),
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: Vec::new(),
            kind: MeasureKind::Input,
            description: None,
            format: None,
        };
        let mut m = Model::new();
        // Inserted straight into the maps before any lookup: the hints are
        // built from the maps on the first lookup.
        m.measures.insert(MeasureId(1), num(1, "A"));
        m.categories.insert(
            CategoryId(1),
            Category {
                id: CategoryId(1),
                name: Name("T".into()),
                items: Vec::new(),
            },
        );
        assert_eq!(m.measure_by_name("A").unwrap().id, MeasureId(1));
        assert_eq!(m.category_by_name("T").unwrap().id, CategoryId(1));
        // Added through the builders after the hints exist: kept current.
        m.add_measure(num(2, "B"));
        m.add_category(CategoryId(2), "P");
        assert_eq!(m.measure_by_name("B").unwrap().id, MeasureId(2));
        assert_eq!(m.category_by_name("P").unwrap().id, CategoryId(2));
        // Inserted directly after the hints exist: found by the scan.
        m.measures.insert(MeasureId(3), num(3, "C"));
        assert_eq!(m.measure_by_name("C").unwrap().id, MeasureId(3));
        // Renamed in place: the stale hint is not trusted.
        m.measures.get_mut(&MeasureId(1)).unwrap().name = Name("A2".into());
        m.categories.get_mut(&CategoryId(1)).unwrap().name = Name("T2".into());
        assert!(m.measure_by_name("A").is_none());
        assert!(m.category_by_name("T").is_none());
        assert_eq!(m.measure_by_name("A2").unwrap().id, MeasureId(1));
        assert_eq!(m.category_by_name("T2").unwrap().id, CategoryId(1));
        // Removed, or replaced under the same id with another name.
        m.measures.remove(&MeasureId(2));
        assert!(m.measure_by_name("B").is_none());
        m.add_measure(num(3, "D"));
        assert!(m.measure_by_name("C").is_none());
        assert_eq!(m.measure_by_name("D").unwrap().id, MeasureId(3));
        // A clone keeps answering, the hints are not saved, and `==` and
        // `Debug` ignore them.
        let copy = m.clone();
        assert_eq!(copy.measure_by_name("D").unwrap().id, MeasureId(3));
        let json = serde_json::to_string(&m).unwrap();
        assert!(!json.contains("names"), "{json}");
        let back: Model = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.measure_by_name("A2").unwrap().id, MeasureId(1));
        assert!(format!("{m:?}").contains("names: NameHints"));
    }

    #[test]
    fn add_item_relists_nothing_and_a_move_stays_listed_in_the_old_category() {
        let mut m = Model::new();
        let (a, b) = (CategoryId(1), CategoryId(2));
        m.add_category(a, "A");
        m.add_category(b, "B");
        m.add_item(ItemId(7), a, "x");
        m.add_item(ItemId(8), a, "y");
        m.add_item(ItemId(7), a, "x2"); // a re-add renames, listed once
        assert_eq!(m.categories[&a].items, [ItemId(7), ItemId(8)]);
        assert_eq!(m.items[&ItemId(7)].name.0, "x2");
        // A move: the item belongs to `b` but stays in `a`'s list too.
        m.add_item(ItemId(7), b, "x3");
        assert_eq!(m.categories[&a].items, [ItemId(7), ItemId(8)]);
        assert_eq!(m.categories[&b].items, [ItemId(7)]);
        assert_eq!(m.items[&ItemId(7)].category, b);
        m.add_item(ItemId(7), b, "x4");
        assert_eq!(m.categories[&b].items, [ItemId(7)]);
        // An item whose list entry is missing is listed again on a re-add.
        m.categories.get_mut(&b).unwrap().items.clear();
        m.add_item(ItemId(7), b, "x5");
        assert_eq!(m.categories[&b].items, [ItemId(7)]);
        // An unknown category: the item is recorded, no list changes.
        m.add_item(ItemId(9), CategoryId(3), "z");
        assert_eq!(m.items[&ItemId(9)].category, CategoryId(3));
        assert_eq!(m.categories.len(), 2);
    }

    #[test]
    fn model_round_trips_through_json() {
        let m = time_product_model();
        let json = serde_json::to_string(&m).expect("serialize");
        let back: Model = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(m, back, "model survives a JSON round trip");
    }

    #[test]
    fn views_and_filters() {
        let mut m = time_product_model();
        let (time, product) = (CategoryId(1), CategoryId(2));
        // A view of Price pivoted with Product on rows, filtered to WidgetA.
        m.add_view(View {
            id: ViewId(1),
            name: Name("By Product".into()),
            measure: MeasureId(100),
            axis_order: vec![product, time],
            n_rows: 1,
            n_cols: 1,
            page_items: vec![],
            filters: vec![Filter {
                category: product,
                items: vec![ItemId(20)], // WidgetA only
            }],
            rect: Default::default(),
            placements: vec![],
        });
        let v = m.view_by_name("By Product").expect("view");
        assert_eq!(v.axis_order, vec![product, time]);
        // Filter: WidgetA passes, WidgetB does not; unfiltered Time passes all.
        assert!(v.allows(product, ItemId(20)));
        assert!(!v.allows(product, ItemId(21)));
        assert!(v.allows(time, ItemId(10)));

        // Views survive a JSON round trip.
        let json = serde_json::to_string(&m).unwrap();
        let back: Model = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn legacy_flat_view_json_loads_as_a_one_matrix_canvas() {
        // A view exactly as written to disk BEFORE canvases existed: no
        // `placements`, no `rect`. Literal legacy JSON on purpose -- building it
        // with today's `View` would test the new type against itself.
        let legacy = r#"{
            "id": 7,
            "name": "By Product",
            "measure": 100,
            "axis_order": [2, 1],
            "n_rows": 1,
            "n_cols": 1,
            "page_items": [[3, 30]],
            "filters": [{"category": 2, "items": [20]}]
        }"#;
        let v: View = serde_json::from_str(legacy).expect("legacy view deserializes");

        // Every flat field survived verbatim.
        assert_eq!(v.id, ViewId(7));
        assert_eq!(v.name, Name("By Product".into()));
        assert_eq!(v.measure, MeasureId(100));
        assert_eq!(v.axis_order, vec![CategoryId(2), CategoryId(1)]);
        assert_eq!((v.n_rows, v.n_cols), (1, 1));
        assert_eq!(v.page_items, vec![(CategoryId(3), ItemId(30))]);
        assert_eq!(
            v.filters,
            vec![Filter {
                category: CategoryId(2),
                items: vec![ItemId(20)]
            }]
        );

        // It is a ONE-matrix canvas, and that matrix mirrors the flat fields.
        assert!(v.placements.is_empty(), "no extra matrices");
        let ms = v.matrices();
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].measure, MeasureId(100));
        assert_eq!(ms[0].axis_order, v.axis_order);
        assert_eq!((ms[0].n_rows, ms[0].n_cols), (1, 1));
        assert_eq!(ms[0].page_items, v.page_items);
        assert_eq!(ms[0].filters, v.filters);
        assert_eq!(ms[0].rect, CanvasRect::default(), "geometry defaulted");

        // ...and it still FILTERS identically, via the view and the matrix.
        assert!(v.allows(CategoryId(2), ItemId(20)));
        assert!(!v.allows(CategoryId(2), ItemId(21)));
        assert!(v.allows(CategoryId(1), ItemId(10))); // unfiltered category
        assert!(!ms[0].allows(CategoryId(2), ItemId(21)));

        // A legacy view with NO n_rows/n_cols at all (pre-stacking) still
        // defaults to 1/1, as it did before this change.
        let prestacking = r#"{"id": 1, "name": "V", "measure": 100, "axis_order": [1, 2]}"#;
        let v2: View = serde_json::from_str(prestacking).expect("pre-stacking view");
        assert_eq!((v2.n_rows, v2.n_cols), (1, 1));
        assert_eq!(v2.matrices().len(), 1);
    }

    /// Stored lambdas load from JSON or text, a text one calling another
    /// whatever their ids; a row that never parses is an error naming it.
    #[test]
    fn load_lambdas_from_rows() {
        let mut m = time_product_model();
        let twice = NamedLambda {
            name: Name("Twice".into()),
            params: vec!["x".into()],
            body: Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(
                    functions::lambda_param(0),
                    DimensionSpec::default(),
                )),
                Box::new(Expr::Literal(Value::Number(2.0))),
            ),
        };
        m.load_lambdas(&[
            (1, "Big = LAMBDA(x, Norm(x, Time) > 1)".into()),
            (2, "Norm = LAMBDA(x, c, Twice(x) / MAX(x OVER c))".into()),
            (3, serde_json::to_string(&twice).unwrap()),
        ])
        .unwrap();
        assert_eq!(m.lambdas[&3], twice);
        assert_eq!(m.lambda_by_name("big").map(|(id, _)| id), Some(1));
        let norm = &m.lambdas[&2];
        assert_eq!(
            norm.param_kinds(),
            Ok(vec![LambdaParam::Reduced, LambdaParam::Category])
        );
        let Expr::BinaryOp(_, call, _) = &m.lambdas[&1].body else {
            panic!("{:?}", m.lambdas[&1].body)
        };
        assert!(matches!(&**call, Expr::Call(f, a)
            if *f == functions::named_lambda(2) && a[1] == Expr::Category(CategoryId(1))));
        // Big calls Norm, which calls Twice; a measure calls Big.
        m.add_measure(Measure {
            id: MeasureId(200),
            name: Name("Flag".into()),
            value_type: ValueType::Boolean,
            categories: vec![CategoryId(2)],
            kind: MeasureKind::Derived(Formula::new(Expr::Call(
                functions::named_lambda(1),
                vec![Expr::Ref(MeasureId(100), DimensionSpec::default())],
            ))),
            description: None,
            format: None,
        });
        assert_eq!(m.lambda_users(3), (vec![1, 2], vec![MeasureId(200)]));
        assert_eq!(m.lambda_users(1), (vec![], vec![MeasureId(200)]));
        assert_eq!(m.lambda_users(9), (vec![], vec![]));
        for (row, why) in [
            ("Bad = LAMBDA(x, Nope(x))", "unknown function: Nope"),
            ("Bad = Price * 2", "is not a LAMBDA"),
        ] {
            let e = m.clone().load_lambdas(&[(9, row.into())]).unwrap_err();
            assert!(e.starts_with("lambda 9: ") && e.contains(why), "{row}: {e}");
        }
    }

    #[test]
    fn multi_placement_view_round_trips_and_filters_per_matrix() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        let mut second = MatrixPlacement::new(MeasureId(101));
        second.axis_order = vec![time, product];
        second.n_rows = 2;
        second.n_cols = 0;
        second.rect = CanvasRect {
            x: 520.0,
            y: 48.5,
            w: 300.0,
            h: 200.0,
        };
        second.filters = vec![Filter {
            category: time,
            items: vec![ItemId(11)], // 2026 only, on THIS matrix
        }];

        let mut primary = MatrixPlacement::new(MeasureId(100));
        primary.axis_order = vec![product];
        primary.rect = CanvasRect {
            x: 10.0,
            y: 20.0,
            w: 400.0,
            h: 150.0,
        };
        primary.filters = vec![Filter {
            category: product,
            items: vec![ItemId(20)],
        }];

        let v = View::from_matrices(
            ViewId(3),
            Name("Canvas".into()),
            primary.clone(),
            vec![second.clone()],
        );
        assert_eq!(v.matrices(), vec![primary.clone(), second.clone()]);

        // Filters are per matrix: the view's `allows` is the PRIMARY matrix's.
        assert!(v.allows(product, ItemId(20)));
        assert!(!v.allows(product, ItemId(21)));
        assert!(v.allows(time, ItemId(10)), "primary has no Time filter");
        assert!(!second.allows(time, ItemId(10)), "but matrix 2 does");
        assert!(second.allows(time, ItemId(11)));

        // Serde round trip, geometry included.
        let json = serde_json::to_string(&v).unwrap();
        let back: View = serde_json::from_str(&json).unwrap();
        assert_eq!(back, v);
        assert_eq!(back.rect.x, 10.0);
        assert_eq!(back.placements[0].rect.y, 48.5);
        assert_eq!(back.placements[0].rect.w, 300.0);
        assert_eq!(
            (back.placements[0].n_rows, back.placements[0].n_cols),
            (2, 0)
        );

        // And inside a whole Model.
        let mut m = time_product_model();
        m.add_view(v.clone());
        let back: Model = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(back.views[&ViewId(3)].matrices().len(), 2);
    }

    #[test]
    fn scenario_overlay_overrides_inputs() {
        let mut m = time_product_model();
        let product = CategoryId(2);
        let a = Coordinate::from_pairs([(product, ItemId(20))]);
        // Base: Price[WidgetA] = 10.
        assert_eq!(m.input(MeasureId(100), &a), Some(&Value::Number(10.0)));

        // A what-if scenario raising WidgetA's price to 15.
        let mut overrides = std::collections::HashMap::new();
        overrides.insert((MeasureId(100), a.clone()), Value::Number(15.0));
        m.add_scenario(Scenario {
            id: ScenarioId(1),
            name: Name("Price hike".into()),
            overrides,
        });

        // Base model unchanged; overlay sees the override.
        assert_eq!(m.input(MeasureId(100), &a), Some(&Value::Number(10.0)));
        let s = m.scenario_by_name("Price hike").unwrap().id;
        let world = m.with_scenario(s);
        assert_eq!(world.input(MeasureId(100), &a), Some(&Value::Number(15.0)));
        // The overlay is a concrete world: no scenarios to re-branch.
        assert!(world.scenarios.is_empty());

        // Scenarios survive a JSON round trip.
        let json = serde_json::to_string(&m).unwrap();
        let back: Model = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn measure_dependency_closure_walks_formulas_and_external_calls() {
        // A -> B -> C chain via Derived formulas, plus D = CALL(f, C, E) (an
        // external-call measure whose "dependencies" are its arg_measures, not
        // an Expr). F is unrelated and must NOT appear in A's closure.
        let mut m = Model::new();
        let a = MeasureId(1);
        let b = MeasureId(2);
        let c = MeasureId(3);
        let d = MeasureId(4);
        let e = MeasureId(5);
        let f = MeasureId(6);
        let leaf = |id: MeasureId, kind: MeasureKind| Measure {
            id,
            name: Name(format!("M{}", id.0)),
            value_type: ValueType::Number,
            categories: vec![],
            kind,
            description: None,
            format: None,
        };
        m.add_measure(leaf(c, MeasureKind::Input));
        m.add_measure(leaf(e, MeasureKind::Input));
        m.add_measure(leaf(f, MeasureKind::Input));
        m.add_measure(leaf(
            b,
            MeasureKind::Derived(Formula::new(Expr::Ref(c, DimensionSpec::default()))),
        ));
        m.add_measure(leaf(
            a,
            MeasureKind::Derived(Formula::new(Expr::Ref(b, DimensionSpec::default()))),
        ));
        m.add_measure(leaf(d, MeasureKind::Input)); // external-call measures stay Input
        m.external_calls.insert(
            d,
            ExternalCall {
                func: "f".into(),
                arg_measures: vec![c, e],
                refresh_policy: Default::default(),
            },
        );

        let closure = m.measure_dependency_closure(&[a]);
        assert_eq!(closure, [a, b, c].into_iter().collect());

        let closure_d = m.measure_dependency_closure(&[d]);
        assert_eq!(closure_d, [d, c, e].into_iter().collect());

        // Multiple roots union their closures; unrelated F never appears.
        let closure_both = m.measure_dependency_closure(&[a, d]);
        assert_eq!(closure_both, [a, b, c, d, e].into_iter().collect());
        assert!(!closure_both.contains(&f));
    }
}
