//! Postgres-free core of the `pg_improv` extension.
//!
//! The extension shell reads rows over SPI, hands them to this crate as plain
//! Rust values, and runs the SQL this crate returns. Everything decidable
//! without a backend lives here:
//!
//! * identifier validation and quoting ([`validate_name`], [`quote_ident`]);
//! * catalog rows → [`Model`] ([`build_model`]) and new derived measures
//!   ([`define`]);
//! * bound fact-table rows → input cells with stable item minting
//!   ([`load_inputs`]), and unbound `improv.cell` rows ([`load_cells`]);
//! * engine results → upsert/delete statements ([`result_rows`],
//!   [`diff_statements`]).
//!
//! The catalog encoding is shared with `improv_storage_pg` (the GUI's store):
//! `measure.formula` is `Target = expr` text (JSON `Formula` is also read),
//! `measure.value_type` is `number | boolean | text | datetime | enum`, a
//! measure's `categories` keep their stored order, and `cell.coord` is
//! `{"<category id>": <item id>}`.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod live;
pub mod pgoutput;

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use improv_core_model::parser::{self, Definition};
use improv_core_model::{
    CategoryId, Coordinate, Expr, Formula, ItemId, Measure, MeasureId, MeasureKind, Model, Name,
    Value, ValueType,
};
use improv_data_source::ItemInterner;
use improv_engine::{CellValue, CoordKey};

mod delta;
pub use delta::{
    apply_edits, cell_changes, coord_json, dependents, edit_rows, measure_ids, native_scope_type,
    parse_edits, plan, quote_literal, scopable, scope_filter, scoped_rel_sql, scoped_result_sql,
    scoped_tids_sql, set_edits, slot_name, sync_trigger_name, tid_rel_sql, touched_sql, value_json,
    Edit, Plan, Scope,
};

/// Longest accepted model/category/measure name. Postgres truncates
/// identifiers at 63 bytes and the result schema is `improv_<model>`.
pub const MAX_NAME_LEN: usize = 56;

/// Everything that can go wrong in the pure core.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// A model, category or measure name is not a plain identifier.
    #[error("invalid {what} name {name:?}: must match ^[A-Za-z][A-Za-z0-9_]*$ (at most 56 chars, not \"value\")")]
    InvalidName {
        /// Which kind of name (`model`, `category`, `measure`).
        what: &'static str,
        /// The rejected name.
        name: String,
    },
    /// A definition could not be parsed, typed, or is not supported in v1.
    #[error("definition: {0}")]
    Definition(String),
    /// Two bound rows map to the same input cell.
    #[error("duplicate coordinate for measure {measure}: {coord:?}")]
    DuplicateCoordinate {
        /// The measure's name.
        measure: String,
        /// The coordinate's item names, in dimension order.
        coord: Vec<String>,
    },
    /// The bound input exceeds the `improv.max_cells` budget.
    #[error("{cells} bound input cells exceed improv.max_cells = {max}")]
    TooManyCells {
        /// Input cells the model would hold.
        cells: usize,
        /// The configured budget.
        max: usize,
    },
    /// The catalog is internally inconsistent.
    #[error("catalog: {0}")]
    Catalog(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Accept `^[A-Za-z][A-Za-z0-9_]*$`, at most [`MAX_NAME_LEN`] chars, and not
/// `value` (any case: it is the result tables' value column).
pub fn validate_name(what: &'static str, name: &str) -> Result<()> {
    let mut chars = name.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && name.len() <= MAX_NAME_LEN
        && !name.eq_ignore_ascii_case("value");
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidName {
            what,
            name: name.to_string(),
        })
    }
}

/// Quote an identifier exactly like Postgres' `quote_ident` does when quoting
/// is needed (always quoting is equivalent and simpler).
pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// The result schema of a model: `improv_<lower(model)>`.
pub fn result_schema(model: &str) -> String {
    format!("improv_{}", model.to_ascii_lowercase())
}

/// The quoted, schema-qualified result table of a derived measure.
pub fn result_table(model: &str, measure: &str) -> String {
    format!(
        "{}.{}",
        quote_ident(&result_schema(model)),
        quote_ident(&measure.to_ascii_lowercase())
    )
}

/// `improv.category` row.
#[derive(Debug, Clone, PartialEq)]
pub struct CategoryRow {
    /// Category id (unique within the model).
    pub id: u32,
    /// Category name.
    pub name: String,
}

/// `improv.item` row.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemRow {
    /// Item id (unique within the model, never reused).
    pub id: u32,
    /// Owning category id.
    pub category_id: u32,
    /// Item name (the bound column's value as text).
    pub name: String,
}

/// `improv.measure` row.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasureRow {
    /// Measure id (unique within the model).
    pub id: u32,
    /// Measure name.
    pub name: String,
    /// `number | boolean | text | datetime | enum`.
    pub value_type: String,
    /// `input | derived`.
    pub kind: String,
    /// A derived measure's `Target = expr` text (or JSON `Formula`).
    pub formula: Option<String>,
    /// Free-text description.
    pub description: Option<String>,
    /// Dimension category ids, in stored order.
    pub categories: Vec<u32>,
}

impl MeasureRow {
    /// An input measure row of type `number`.
    pub fn input(id: u32, name: &str, categories: Vec<u32>) -> Self {
        MeasureRow {
            id,
            name: name.to_string(),
            value_type: "number".into(),
            kind: "input".into(),
            formula: None,
            description: None,
            categories,
        }
    }

    /// Whether this row is a derived measure.
    pub fn is_derived(&self) -> bool {
        self.kind == "derived"
    }
}

/// The catalog spelling of a value type.
pub fn value_type_name(vt: ValueType) -> &'static str {
    match vt {
        ValueType::Number => "number",
        ValueType::Boolean => "boolean",
        ValueType::Text => "text",
        ValueType::DateTime => "datetime",
        ValueType::Enum => "enum",
    }
}

fn value_type_parse(s: &str) -> Result<ValueType> {
    Ok(match s {
        "number" => ValueType::Number,
        "boolean" => ValueType::Boolean,
        "text" => ValueType::Text,
        "datetime" => ValueType::DateTime,
        "enum" => ValueType::Enum,
        other => return Err(Error::Catalog(format!("unknown value type {other:?}"))),
    })
}

/// One model's catalog rows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    /// `improv.category` rows.
    pub categories: Vec<CategoryRow>,
    /// `improv.item` rows.
    pub items: Vec<ItemRow>,
    /// `improv.measure` rows.
    pub measures: Vec<MeasureRow>,
    /// `improv.lambda` rows.
    pub lambdas: Vec<LambdaRow>,
}

/// `improv.lambda` row: a named LAMBDA.
#[derive(Debug, Clone, PartialEq)]
pub struct LambdaRow {
    /// Lambda id (unique within the model; calls store it).
    pub id: u32,
    /// Its name, as in the definition.
    pub name: String,
    /// `Name = LAMBDA(…)` text (what `improv.define` writes) or a JSON
    /// `NamedLambda` (what the GUI's store writes).
    pub definition: String,
}

/// Parse a `Target = expr` definition, rejecting the `SQL(...)`/`CALL(...)`
/// source forms (external code never runs inside Postgres in v1).
fn parse_formula(model: &Model, text: &str) -> Result<(String, Formula)> {
    match parser::parse_definition(model, text).map_err(|e| Error::Definition(e.to_string()))? {
        Definition::Formula(ft) => Ok((ft.target.0, ft.formula)),
        Definition::Lambda(lt) => Err(Error::Definition(format!(
            "{} = LAMBDA(…) defines a named LAMBDA, not a measure",
            lt.name.0
        ))),
        _ => Err(Error::Definition(
            "SQL(...) and CALL(...) definitions are not supported inside Postgres".into(),
        )),
    }
}

/// Build a [`Model`] (structure only, no input cells) from catalog rows.
/// Every measure and named LAMBDA is added before any formula is parsed, so
/// a text formula may reference any measure, whatever its id (a
/// redefinition may read a measure defined after it), and call any lambda.
pub fn build_model(catalog: &Catalog) -> Result<Model> {
    let mut model = Model::new();
    for c in &catalog.categories {
        model.add_category(CategoryId(c.id), c.name.clone());
    }
    for i in &catalog.items {
        if !model.categories.contains_key(&CategoryId(i.category_id)) {
            return Err(Error::Catalog(format!(
                "item {} references unknown category {}",
                i.id, i.category_id
            )));
        }
        model.add_item(ItemId(i.id), CategoryId(i.category_id), i.name.clone());
    }
    let mut rows: Vec<&MeasureRow> = catalog.measures.iter().collect();
    rows.sort_by_key(|r| r.id);
    for r in &rows {
        if !matches!(
            (r.kind.as_str(), &r.formula),
            ("input", _) | ("derived", Some(_))
        ) {
            return Err(Error::Catalog(format!(
                "measure {} has kind {:?} and formula {:?}",
                r.name, r.kind, r.formula
            )));
        }
        // A derived measure is a placeholder until its formula is parsed.
        model.add_measure(Measure {
            id: MeasureId(r.id),
            name: Name(r.name.clone()),
            value_type: value_type_parse(&r.value_type)?,
            categories: r.categories.iter().map(|&c| CategoryId(c)).collect(),
            kind: MeasureKind::Input,
            description: r.description.clone(),
        });
    }
    let lambdas: Vec<(u32, String)> = catalog
        .lambdas
        .iter()
        .map(|l| (l.id, l.definition.clone()))
        .collect();
    model.load_lambdas(&lambdas).map_err(Error::Catalog)?;
    for l in &catalog.lambdas {
        if !model.lambdas[&l.id].name.0.eq_ignore_ascii_case(&l.name) {
            return Err(Error::Catalog(format!(
                "lambda {} is named {} but defines {}",
                l.id, l.name, model.lambdas[&l.id].name.0
            )));
        }
    }
    for r in rows {
        let Some(text) = r.formula.as_deref().filter(|_| r.is_derived()) else {
            continue;
        };
        let formula = match serde_json::from_str::<Formula>(text) {
            Ok(f) => f,
            Err(_) => {
                let (target, formula) = parse_formula(&model, text)?;
                if target != r.name {
                    return Err(Error::Catalog(format!(
                        "measure {} has a formula for {target}",
                        r.name
                    )));
                }
                formula
            }
        };
        // Added above, as a placeholder.
        model
            .measures
            .entry(MeasureId(r.id))
            .and_modify(|m| m.kind = MeasureKind::Derived(formula));
    }
    Ok(model)
}

/// What [`define_measure`] found to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Defined {
    /// A new derived measure: insert the row and create its result table.
    New(MeasureRow),
    /// The measure `id` already has this formula: change nothing.
    Unchanged(u32),
    /// A new formula for an existing derived measure: update its row, then
    /// recompute it and its dependents.
    Replaced {
        /// The updated row (id, name and description kept).
        row: MeasureRow,
        /// Its value type or dimensions changed: recreate its result table.
        reshaped: bool,
    },
    /// A named LAMBDA ([`define_lambda`]): insert or update its row, then
    /// recompute `callers` and their dependents.
    Lambda {
        /// The new or updated row.
        row: LambdaRow,
        /// The derived measures that call it, directly or through other
        /// lambdas (none for a new lambda).
        callers: Vec<MeasureId>,
    },
}

/// Whether `text` is a `Name = LAMBDA(…)` definition.
fn is_lambda(text: &str) -> bool {
    text.split_once('=').is_some_and(|(_, rhs)| {
        let r = rhs.trim_start();
        r.get(..6).is_some_and(|h| h.eq_ignore_ascii_case("lambda"))
            && r[6..].trim_start().starts_with('(')
    })
}

/// A `Name = LAMBDA(…)` definition for `model`: `None` when `text` defines a
/// measure, else a new named LAMBDA ([`Defined::Lambda`]), or with `replace`
/// [`Defined::Unchanged`] when it has this definition already, else its
/// redefinition, with the measures that call it to recompute. Its body may
/// reduce over a category parameter (`LAMBDA(x, c, MAX(x OVER c))`). A name
/// a measure has is an error, and so is a redefinition that would call a
/// lambda calling it, or break or change the type or dimensions of a
/// measure that calls it (or depends on one that does): each error names
/// the dependent.
pub fn define_lambda(model: &Model, text: &str, replace: bool) -> Result<Option<Defined>> {
    if !is_lambda(text) {
        return Ok(None);
    }
    let lt = match parser::parse_definition(model, text)
        .map_err(|e| Error::Definition(e.to_string()))?
    {
        Definition::Lambda(lt) => lt,
        _ => {
            return Err(Error::Definition(format!(
                "{text:?} is not Name = LAMBDA(…)"
            )))
        }
    };
    let name = lt.name.0.as_str();
    if let Some(m) = model
        .measures
        .values()
        .find(|m| m.name.0.eq_ignore_ascii_case(name))
    {
        return Err(Error::Definition(format!(
            "{} is a measure; name the LAMBDA differently",
            m.name.0
        )));
    }
    let row = |id| LambdaRow {
        id,
        name: name.to_string(),
        definition: text.trim().to_string(),
    };
    match model.lambda_by_name(name) {
        None => Ok(Some(Defined::Lambda {
            row: row(model.lambdas.keys().max().map_or(1, |m| m + 1)),
            callers: Vec::new(),
        })),
        Some((_, old)) if !replace => Err(Error::Definition(format!(
            "LAMBDA {} already exists (improv.define(model, definition, replace => true) \
             redefines it)",
            old.name.0
        ))),
        Some((id, old)) if old.params == lt.lambda.params && old.body == lt.lambda.body => {
            Ok(Some(Defined::Unchanged(id)))
        }
        Some((id, old)) => {
            let callers = replace_lambda(model, id, &old.name.0, lt.lambda)?;
            Ok(Some(Defined::Lambda {
                row: row(id),
                callers,
            }))
        }
    }
}

/// Check that lambda `id` (`name`) may become `new`, and return the derived
/// measures that call it.
fn replace_lambda(
    model: &Model,
    id: u32,
    name: &str,
    new: improv_core_model::NamedLambda,
) -> Result<Vec<MeasureId>> {
    let (lambdas, callers) = model.lambda_users(id);
    let lambda_name = |i: &u32| model.lambdas[i].name.0.clone();
    // `new` may not call a lambda that calls it (directly or not): a cycle.
    for l in &lambdas {
        if calls_named(&new.body, *l) {
            return Err(Error::Definition(format!(
                "{name} cannot call {}: {} calls {name}",
                lambda_name(l),
                lambda_name(l)
            )));
        }
    }
    let mut next = model.clone();
    next.lambdas.insert(id, new);
    for l in &lambdas {
        next.expand_lambdas(&next.lambdas[l].body).map_err(|e| {
            Error::Definition(format!(
                "redefining {name} breaks LAMBDA {}, which calls it: {e}",
                lambda_name(l)
            ))
        })?;
    }
    keep_shapes(&next, name, &dependents(&next, &callers))?;
    Ok(callers)
}

/// Check that each derived measure of `ids`, which depend on `name`, still
/// compiles in `next` to the type and dimensions it has (its result table).
/// An input keeps its shape.
fn keep_shapes(next: &Model, name: &str, ids: &[MeasureId]) -> Result<()> {
    for d in ids {
        let m = &next.measures[d];
        let MeasureKind::Derived(f) = &m.kind else {
            continue;
        };
        let shape =
            improv_engine::compiler::derived_shape(next, *d, &m.name.0, f).map_err(|e| {
                Error::Definition(format!(
                    "redefining {name} breaks {}, which depends on it: {e}",
                    m.name.0
                ))
            })?;
        if shape != (m.value_type, m.categories.clone()) {
            return Err(Error::Definition(format!(
                "redefining {name} would change the type or dimensions of {}, which depends \
                 on it: redefine or drop {} first",
                m.name.0, m.name.0
            )));
        }
    }
    Ok(())
}

/// Whether `e` calls named LAMBDA `id` directly.
fn calls_named(e: &Expr, id: u32) -> bool {
    match e {
        Expr::Call(f, a) => {
            improv_core_model::functions::named_lambda_of(*f) == Some(id)
                || a.iter().any(|x| calls_named(x, id))
        }
        Expr::UnaryOp(_, a) => calls_named(a, id),
        Expr::BinaryOp(_, a, b) => calls_named(a, id) || calls_named(b, id),
        Expr::Literal(_) | Expr::Ref(..) | Expr::Category(_) => false,
    }
}

/// The named LAMBDA called `name` (any case) when nothing calls it, `None`
/// when there is none: dropping a lambda a measure or another lambda still
/// calls is an error naming them.
pub fn drop_lambda(model: &Model, name: &str) -> Result<Option<u32>> {
    let Some((id, l)) = model.lambda_by_name(name) else {
        return Ok(None);
    };
    let (lambdas, measures) = model.lambda_users(id);
    let mut users: Vec<&str> = measures
        .iter()
        .filter_map(|m| model.measures.get(m).map(|m| m.name.0.as_str()))
        .chain(lambdas.iter().map(|i| model.lambdas[i].name.0.as_str()))
        .collect();
    users.sort_unstable();
    if users.is_empty() {
        return Ok(Some(id));
    }
    Err(Error::Definition(format!(
        "cannot drop {}: {} still {} it (drop or redefine {} first)",
        l.name.0,
        users.join(", "),
        if users.len() == 1 { "calls" } else { "call" },
        if users.len() == 1 { "it" } else { "them" }
    )))
}

/// Validate a `Target = expr` definition against `model` and return the
/// catalog row for the new derived measure `id`. Its categories are the
/// engine-inferred result dimensions (an `OVER` aggregation drops the
/// aggregated category), which is exactly the result table's shape. An
/// existing target is an error ([`define_measure`] can replace it).
pub fn define(model: &Model, text: &str, id: u32) -> Result<MeasureRow> {
    match target(model, text) {
        (name, None) if model.lambda_by_name(name).is_some() => Err(Error::Definition(format!(
            "{name} is a named LAMBDA; name the measure differently"
        ))),
        (_, None) => new_measure(model, text, id),
        (_, Some(m)) => Err(Error::Definition(format!(
            "measure {} already exists (improv.define(model, definition, replace => true) \
             redefines it)",
            m.name.0
        ))),
    }
}

/// The target name of definition `text`, and the measure of that name (any
/// case) in `model`.
fn target<'a>(model: &'a Model, text: &'a str) -> (&'a str, Option<&'a Measure>) {
    let name = text.split('=').next().unwrap_or("").trim();
    let m = model
        .measures
        .values()
        .find(|m| m.name.0.eq_ignore_ascii_case(name));
    (name, m)
}

/// [`define`], or with `replace` a redefinition of an existing derived
/// measure: [`Defined::Unchanged`] when it parses to the formula it has, else
/// [`Defined::Replaced`]. A replacement is refused when its formula would read
/// a measure that depends on the target (a cycle), when it changes the
/// target's value type while another measure reads it, and when it breaks or
/// reshapes a dependent; each error names the dependent.
pub fn define_measure(model: &Model, text: &str, id: u32, replace: bool) -> Result<Defined> {
    match target(model, text) {
        (_, Some(m)) if replace => replace_measure(model, text, m),
        _ => define(model, text, id).map(Defined::New),
    }
}

fn new_measure(model: &Model, text: &str, id: u32) -> Result<MeasureRow> {
    // A new measure may refer to itself (a PREV recurrence): parse with its
    // name provisionally known.
    let name = text.split('=').next().unwrap_or("").trim();
    let (target, formula) = parse_formula(&model.with_provisional(MeasureId(id), name), text)?;
    validate_name("measure", &target)?;
    if model
        .measures
        .values()
        .any(|m| m.name.0.eq_ignore_ascii_case(&target))
    {
        return Err(Error::Definition(format!(
            "measure {target} already exists"
        )));
    }
    let (value_type, categories) =
        improv_engine::compiler::derived_shape(model, MeasureId(id), &target, &formula)
            .map_err(|e| Error::Definition(e.to_string()))?;
    if value_type == ValueType::Enum {
        return Err(no_enum_result(&target));
    }
    Ok(MeasureRow {
        id,
        name: target,
        value_type: value_type_name(value_type).into(),
        kind: "derived".into(),
        formula: Some(text.trim().to_string()),
        description: None,
        categories: categories.iter().map(|c| c.0).collect(),
    })
}

fn no_enum_result(name: &str) -> Error {
    Error::Definition(format!(
        "{name} is an enum; enum results are not supported yet"
    ))
}

fn replace_measure(model: &Model, text: &str, m: &Measure) -> Result<Defined> {
    let MeasureKind::Derived(old) = &m.kind else {
        return Err(Error::Definition(format!(
            "{} is an input measure; only a derived measure can be redefined",
            m.name.0
        )));
    };
    let (_, formula) = parse_formula(model, text)?;
    if formula == *old {
        return Ok(Defined::Unchanged(m.id.0));
    }
    let name_of = |id: &MeasureId| model.measures.get(id).map_or("?", |m| m.name.0.as_str());
    let downstream: Vec<MeasureId> = dependents(model, &[m.id])
        .into_iter()
        .filter(|d| *d != m.id)
        .collect();
    if let Some(d) = formula
        .referenced_measures()
        .iter()
        .find(|r| downstream.contains(r))
    {
        return Err(Error::Definition(format!(
            "{} cannot read {}: {} depends on {}",
            m.name.0,
            name_of(d),
            name_of(d),
            m.name.0
        )));
    }
    let (value_type, categories) =
        improv_engine::compiler::derived_shape(model, m.id, &m.name.0, &formula)
            .map_err(|e| Error::Definition(e.to_string()))?;
    if value_type == ValueType::Enum {
        return Err(no_enum_result(&m.name.0));
    }
    let readers = referencing(model, m.id);
    if value_type != m.value_type && !readers.is_empty() {
        return Err(Error::Definition(format!(
            "redefining {} would change its type from {} to {}, but {} {} it: \
             redefine or drop {} first",
            m.name.0,
            value_type_name(m.value_type),
            value_type_name(value_type),
            readers.join(", "),
            if readers.len() == 1 { "reads" } else { "read" },
            readers.join(", ")
        )));
    }
    // Every dependent must keep its type and dimensions (its result table).
    let mut next = model.clone();
    next.add_measure(Measure {
        kind: MeasureKind::Derived(formula),
        value_type,
        categories: categories.clone(),
        ..m.clone()
    });
    keep_shapes(&next, &m.name.0, &downstream)?;
    Ok(Defined::Replaced {
        reshaped: value_type != m.value_type || categories != m.categories,
        row: MeasureRow {
            id: m.id.0,
            name: m.name.0.clone(),
            value_type: value_type_name(value_type).into(),
            kind: "derived".into(),
            formula: Some(text.trim().to_string()),
            description: m.description.clone(),
            categories: categories.iter().map(|c| c.0).collect(),
        },
    })
}

/// The other derived measures whose formula reads `id` directly, by name,
/// sorted.
pub fn referencing(model: &Model, id: MeasureId) -> Vec<String> {
    let mut names: Vec<String> = model
        .measures
        .values()
        .filter(|m| m.id != id)
        .filter(
            |m| matches!(&m.kind, MeasureKind::Derived(f) if f.referenced_measures().contains(&id)),
        )
        .map(|m| m.name.0.clone())
        .collect();
    names.sort();
    names
}

/// The measure called `name` (any case), when no other measure reads it:
/// dropping a measure a dependent still references is an error naming it.
pub fn drop_target(model: &Model, name: &str) -> Result<MeasureId> {
    let m = model
        .measures
        .values()
        .find(|m| m.name.0.eq_ignore_ascii_case(name))
        .ok_or_else(|| Error::Definition(format!("measure {name} does not exist")))?;
    let readers = referencing(model, m.id);
    if readers.is_empty() {
        return Ok(m.id);
    }
    Err(Error::Definition(format!(
        "cannot drop {}: {} still {} it (drop or redefine {} first)",
        m.name.0,
        readers.join(", "),
        if readers.len() == 1 {
            "references"
        } else {
            "reference"
        },
        if readers.len() == 1 { "it" } else { "them" }
    )))
}

/// The rows of one bound input measure, as read from its fact table: one
/// `Option<String>` per dimension (in the measure's category order) and the
/// value as the measure's type reads it (`None` = SQL NULL).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoundRows {
    /// The input measure the rows feed.
    pub measure_id: u32,
    /// `(dimension values, value)` per fact row.
    pub rows: Vec<(Vec<Option<String>>, Option<CellValue>)>,
}

/// What [`load_inputs`] did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Loaded {
    /// Items minted in memory for values the model had no item for. A
    /// persisting caller mints through `improv.mint_items` first (see
    /// [`unknown_items`]), so this is empty for it.
    pub new_items: Vec<ItemRow>,
    /// Input cells now in the model.
    pub cells: usize,
    /// Rows ignored because a dimension value was NULL.
    pub null_dim_rows: usize,
}

/// A bound measure's id, name and categories.
type Shaped = (MeasureId, String, Vec<CategoryId>);

/// The distinct non-NULL dimension values per category.
type Distinct<'a> = BTreeMap<CategoryId, BTreeSet<&'a str>>;

/// Validate `bound` against `model`; per measure its shape, and the distinct
/// non-NULL dimension values per category.
fn shape<'a>(model: &Model, bound: &'a [BoundRows]) -> Result<(Vec<Shaped>, Distinct<'a>)> {
    let mut shaped = Vec::with_capacity(bound.len());
    let mut distinct = Distinct::new();
    for b in bound {
        let m = model
            .measures
            .get(&MeasureId(b.measure_id))
            .ok_or_else(|| {
                Error::Catalog(format!("binding for unknown measure {}", b.measure_id))
            })?;
        for (dims, _) in &b.rows {
            if dims.len() != m.categories.len() {
                return Err(Error::Catalog(format!(
                    "{} has {} dimensions but a bound row has {}",
                    m.name.0,
                    m.categories.len(),
                    dims.len()
                )));
            }
            for (c, v) in m.categories.iter().zip(dims) {
                if let Some(v) = v {
                    distinct.entry(*c).or_default().insert(v);
                }
            }
        }
        shaped.push((
            MeasureId(b.measure_id),
            m.name.0.clone(),
            m.categories.clone(),
        ));
    }
    Ok((shaped, distinct))
}

/// The `(category, name)` pairs `bound` uses that `model` has no item for,
/// sorted. A persisting caller mints them through `improv.mint_items` and
/// adds the returned items before [`load_inputs`], so it mints nothing.
pub fn unknown_items(model: &Model, bound: &[BoundRows]) -> Result<Vec<(CategoryId, String)>> {
    let known: BTreeSet<(CategoryId, &str)> = model
        .items
        .values()
        .map(|i| (i.category, i.name.0.as_str()))
        .collect();
    let (_, distinct) = shape(model, bound)?;
    Ok(distinct
        .into_iter()
        .flat_map(|(c, vs)| vs.into_iter().map(move |v| (c, v)))
        .filter(|k| !known.contains(k))
        .map(|(c, v)| (c, v.to_string()))
        .collect())
}

/// Turn bound fact rows into input cells. Existing items keep their ids; new
/// distinct values are minted above the model's highest item id, in lexical
/// order per category, so the same data always mints the same ids. A NULL
/// value is no cell; a NULL dimension skips (and counts) the row; two rows on
/// one coordinate are an error.
pub fn load_inputs(model: &mut Model, bound: &[BoundRows]) -> Result<Loaded> {
    let known: BTreeSet<ItemId> = model.items.keys().copied().collect();
    let (shaped, distinct) = shape(model, bound)?;

    let mut interner = ItemInterner::seeded_from(model, 1);
    for (c, values) in &distinct {
        for v in values {
            interner.intern(*c, v);
        }
    }

    // A map of the model's own kind, sized once: moved in whole below, so
    // the cells are never held twice (2.5M of them were ~1 GB at the peak).
    let rows = bound.iter().map(|b| b.rows.len()).sum();
    let mut cells: HashMap<(MeasureId, Coordinate), Value> = HashMap::with_capacity(rows);
    let mut null_dim_rows = 0;
    for (b, (mid, name, cats)) in bound.iter().zip(&shaped) {
        for (dims, value) in &b.rows {
            let Some(names) = dims
                .iter()
                .map(Option::as_deref)
                .collect::<Option<Vec<_>>>()
            else {
                null_dim_rows += 1;
                continue;
            };
            let Some(v) = value else { continue };
            let v = to_value(v)
                .ok_or_else(|| Error::Catalog(format!("{name}: {v} is out of range")))?;
            let coord = Coordinate::from_pairs(
                cats.iter()
                    .zip(&names)
                    .map(|(c, n)| (*c, interner.intern(*c, n))),
            );
            match cells.entry((*mid, coord)) {
                Entry::Occupied(_) => {
                    return Err(Error::DuplicateCoordinate {
                        measure: name.clone(),
                        coord: names.iter().map(|n| n.to_string()).collect(),
                    })
                }
                Entry::Vacant(e) => {
                    e.insert(v);
                }
            }
        }
    }

    interner.register(model);
    let mut new_items: Vec<ItemRow> = model
        .items
        .values()
        .filter(|i| !known.contains(&i.id))
        .map(|i| ItemRow {
            id: i.id.0,
            category_id: i.category.0,
            name: i.name.0.clone(),
        })
        .collect();
    new_items.sort_by_key(|i| i.id);
    let n = cells.len();
    if model.inputs.is_empty() {
        model.inputs = cells;
    } else {
        model.inputs.extend(cells);
    }
    Ok(Loaded {
        new_items,
        cells: n,
        null_dim_rows,
    })
}

/// Add unbound input cells (`improv.cell` rows: measure id, `coord` JSON
/// `{"<category id>": <item id>}`, `value` JSON scalar). Returns the count.
pub fn load_cells(model: &mut Model, cells: &[(u32, String, String)]) -> Result<usize> {
    use serde_json::Value as J;
    for (mid, coord, value) in cells {
        let bad = |what: &str| Error::Catalog(format!("improv.cell {mid} {coord}: {what}"));
        let vt = model
            .measures
            .get(&MeasureId(*mid))
            .ok_or_else(|| bad("unknown measure"))?
            .value_type;
        let dims: BTreeMap<CategoryId, ItemId> =
            serde_json::from_str(coord).map_err(|e| bad(&e.to_string()))?;
        let json: J = serde_json::from_str(value).map_err(|e| bad(&e.to_string()))?;
        let v = json_value(vt, &json)
            .ok_or_else(|| bad(&format!("{json} is not a {} value", value_type_name(vt))))?;
        model.set_input(MeasureId(*mid), Coordinate { dims }, v);
    }
    Ok(cells.len())
}

/// A JSON scalar as a `vt` value: a number, an enum ordinal, a bool, a
/// string, or a datetime as an RFC 3339 string. `None` when it is not one.
pub(crate) fn json_value(vt: ValueType, json: &serde_json::Value) -> Option<Value> {
    use serde_json::Value as J;
    match (vt, json) {
        (ValueType::Number, J::Number(n)) => n.as_f64().map(Value::Number),
        (ValueType::Enum, J::Number(n)) => n
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Value::Enum),
        (ValueType::Boolean, J::Bool(b)) => Some(Value::Boolean(*b)),
        (ValueType::Text, J::String(s)) => Some(Value::Text(s.clone())),
        (ValueType::DateTime, J::String(s)) => chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| Value::DateTime(d.with_timezone(&chrono::Utc))),
        _ => None,
    }
}

/// Refuse a model whose input exceeds the `improv.max_cells` budget.
pub fn check_cell_budget(cells: usize, max: usize) -> Result<()> {
    if cells > max {
        Err(Error::TooManyCells { cells, max })
    } else {
        Ok(())
    }
}

/// A result table's contents: item names in dimension order → value.
pub type ResultRows = BTreeMap<Vec<String>, CellValue>;

/// An engine cell as a model value (`None`: an error, or a timestamp chrono
/// cannot hold).
fn to_value(v: &CellValue) -> Option<Value> {
    Some(match v {
        CellValue::Num(b) => Value::Number(f64::from_bits(*b)),
        CellValue::Bool(b) => Value::Boolean(*b),
        CellValue::Text(t) => Value::Text(t.clone()),
        CellValue::Date(ms) => Value::DateTime(chrono::DateTime::from_timestamp_millis(*ms)?),
        CellValue::Err(_) => return None,
    })
}

/// Whether `v` is a value of type `vt`.
fn is_a(vt: ValueType, v: &CellValue) -> bool {
    matches!(
        (vt, v),
        (ValueType::Number, CellValue::Num(_))
            | (ValueType::Boolean, CellValue::Bool(_))
            | (ValueType::Text, CellValue::Text(_))
            | (ValueType::DateTime, CellValue::Date(_))
    )
}

fn no_enum() -> Error {
    Error::Catalog("enum measures have no Postgres column type yet".into())
}

/// The value type of an input measure bound to a column of Postgres type
/// `typname` (`pg_type.typname`; `shown` is its `format_type`, for errors),
/// read through `agg`.
pub fn bound_value_type(typname: &str, shown: &str, agg: Option<&str>) -> Result<ValueType> {
    let vt = match typname {
        "float4" | "float8" | "numeric" | "int2" | "int4" | "int8" => ValueType::Number,
        "bool" => ValueType::Boolean,
        "text" | "varchar" | "bpchar" => ValueType::Text,
        "date" | "timestamp" | "timestamptz" => ValueType::DateTime,
        _ => {
            return Err(Error::Catalog(format!(
                "cannot bind a measure to a column of type {shown}"
            )))
        }
    };
    match agg.map(agg_sql).transpose()? {
        Some("count" | "count_distinct") => Ok(ValueType::Number),
        Some(a @ ("sum" | "avg")) if vt != ValueType::Number => Err(Error::Catalog(format!(
            "agg {a} needs a number column, not {shown}"
        ))),
        _ => Ok(vt),
    }
}

/// A result table's `value` column type for `vt` (as `format_type` names it).
pub fn sql_type(vt: ValueType) -> Result<&'static str> {
    Ok(match vt {
        ValueType::Number => "double precision",
        ValueType::Boolean => "boolean",
        ValueType::Text => "text",
        ValueType::DateTime => "timestamp with time zone",
        ValueType::Enum => return Err(no_enum()),
    })
}

/// SQL reading expression `e` as a `vt` value: `float8`, `bool`, `text`, or
/// a datetime as `int8` epoch milliseconds (the engine's resolution).
fn read_expr(vt: ValueType, e: &str) -> Result<String> {
    Ok(match vt {
        ValueType::Number => format!("{e}::float8"),
        ValueType::Boolean => format!("{e}::bool"),
        ValueType::Text => format!("{e}::text"),
        ValueType::DateTime => format!("(extract(epoch from {e}::timestamptz) * 1000)::int8"),
        ValueType::Enum => return Err(no_enum()),
    })
}

/// Refuse a result table whose `value` column (of type `found`, as
/// `format_type` names it) does not hold `vt`.
pub fn check_value_column(table: &str, vt: ValueType, found: &str) -> Result<()> {
    if found == sql_type(vt)? {
        return Ok(());
    }
    Err(Error::Catalog(format!(
        "{table}.value is {found} but the measure is {}: redefine the measure \
         (improv.define(model, definition, replace => true)) to recreate its table",
        value_type_name(vt)
    )))
}

/// Name the engine's cells for `measure` in its dimension order. Cells that
/// are not of the measure's type (errors) are left out.
pub fn result_rows(
    model: &Model,
    measure: MeasureId,
    cells: &HashMap<CoordKey, CellValue>,
) -> Result<ResultRows> {
    let m = model
        .measures
        .get(&measure)
        .ok_or_else(|| Error::Catalog(format!("unknown measure {}", measure.0)))?;
    let mut out = ResultRows::new();
    for (key, val) in cells {
        if !is_a(m.value_type, val) {
            continue;
        }
        let names = m
            .categories
            .iter()
            .map(|c| {
                key.iter()
                    .find(|(kc, _)| *kc == c.0)
                    .and_then(|(_, i)| model.items.get(&ItemId(*i)))
                    .map(|it| it.name.0.clone())
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                Error::Catalog(format!(
                    "engine cell {key:?} does not match {}'s dimensions",
                    m.name.0
                ))
            })?;
        out.insert(names, val.clone());
    }
    Ok(out)
}

/// The lower-cased result-column names of a measure's dimensions.
pub fn dim_columns(model: &Model, measure: MeasureId) -> Result<Vec<String>> {
    let m = model
        .measures
        .get(&measure)
        .ok_or_else(|| Error::Catalog(format!("unknown measure {}", measure.0)))?;
    m.categories
        .iter()
        .map(|c| {
            model
                .categories
                .get(c)
                .map(|cat| cat.name.0.to_ascii_lowercase())
                .ok_or_else(|| Error::Catalog(format!("unknown category {}", c.0)))
        })
        .collect()
}

/// `CREATE TABLE IF NOT EXISTS` for a derived measure's result table (a
/// measure defined through the GUI gets its table on the first recalc). A
/// dimensionless measure's table is just `value`, holding at most one row.
pub fn create_table_sql(table: &str, dim_cols: &[String], vt: ValueType) -> Result<String> {
    let vt = sql_type(vt)?;
    if dim_cols.is_empty() {
        return Ok(format!("CREATE TABLE IF NOT EXISTS {table} (value {vt})"));
    }
    let cols: Vec<String> = dim_cols
        .iter()
        .map(|c| format!("{} text NOT NULL", quote_ident(c)))
        .collect();
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {table} ({}, value {vt}, PRIMARY KEY ({}))",
        cols.join(", "),
        quoted_list(dim_cols)
    ))
}

/// `SELECT` of a result table's current rows (dims…, value read as `vt`,
/// see [`select_bound_sql`]).
pub fn select_result_sql(table: &str, dim_cols: &[String], vt: ValueType) -> Result<String> {
    let mut cols: Vec<String> = dim_cols.iter().map(|c| quote_ident(c)).collect();
    cols.push(read_expr(vt, "value")?);
    Ok(format!("SELECT {} FROM {table}", cols.join(", ")))
}

/// The canonical name of a `binding.agg` (`sum | avg | min | max | count |
/// count_distinct`); an error for anything else.
pub fn agg_sql(agg: &str) -> Result<&'static str> {
    Ok(match agg {
        "sum" => "sum",
        "avg" => "avg",
        "min" => "min",
        "max" => "max",
        "count" => "count",
        "count_distinct" => "count_distinct",
        other => {
            return Err(Error::Catalog(format!(
                "agg must be sum, avg, min, max, count or count_distinct, not {other:?}"
            )))
        }
    })
}

/// The SQL aggregate call of `binding.agg` `agg` over the (quoted) value
/// expression `v`: `sum(v)`, …, `count(v)` for `count_distinct` too, whose
/// rows [`select_bound_sql`] has already made distinct.
pub fn agg_call(agg: &str, v: &str) -> Result<String> {
    Ok(match agg_sql(agg)? {
        "count_distinct" => format!("count({v})"),
        a => format!("{a}({v})"),
    })
}

/// `SELECT` of a binding's fact rows: dims as text…, then the value read as
/// `vt` (`float8`, `bool`, `text`, or a datetime as `int8` epoch
/// milliseconds). With `agg`, one row per coordinate, its rows' values
/// aggregated (a `bool`'s `min` and `max` as `bool_and` and `bool_or`: SQL
/// has no `max(bool)`). `rel` is a `regclass::text`, which Postgres has
/// already quoted.
///
/// `count_distinct` is `count(v)` over `SELECT DISTINCT dims::text, v`, two
/// hash aggregates: `count(DISTINCT v)` sorts every row by its text keys,
/// about 5x slower at 2.5M rows. It is equal: both group by the dims' text
/// and use `v`'s equality, `DISTINCT` keeps one NULL that `count` skips, and
/// a group of only NULLs still counts 0.
pub fn select_bound_sql(
    rel: &str,
    dim_cols: &[String],
    value_col: &str,
    agg: Option<&str>,
    vt: ValueType,
) -> Result<String> {
    let dims: Vec<String> = dim_cols
        .iter()
        .map(|c| format!("{}::text", quote_ident(c)))
        .collect();
    let v = quote_ident(value_col);
    Ok(match agg {
        None => format!(
            "SELECT {}, {} FROM {rel}",
            dims.join(", "),
            read_expr(vt, &v)?
        ),
        Some(a) => {
            let by: Vec<String> = (1..=dims.len()).map(|i| i.to_string()).collect();
            let (call, dims, rel) = match (agg_sql(a)?, vt) {
                ("min", ValueType::Boolean) => (format!("bool_and({v})"), dims, rel.to_string()),
                ("max", ValueType::Boolean) => (format!("bool_or({v})"), dims, rel.to_string()),
                ("count_distinct", _) => {
                    let names: Vec<String> = (1..=dims.len()).map(|i| format!("d{i}")).collect();
                    let inner: Vec<String> = dims
                        .iter()
                        .zip(&names)
                        .map(|(d, n)| format!("{d} AS {n}"))
                        .collect();
                    let rel = format!(
                        "(SELECT DISTINCT {}, {v} AS v FROM {rel}) improv_distinct",
                        inner.join(", ")
                    );
                    (agg_call(a, "v")?, names, rel)
                }
                _ => (agg_call(a, &v)?, dims, rel.to_string()),
            };
            format!(
                "SELECT {}, {} FROM {rel} GROUP BY {}",
                dims.join(", "),
                read_expr(vt, &call)?,
                by.join(", ")
            )
        }
    })
}

fn quoted_list(cols: &[String]) -> String {
    cols.iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One SQL statement with column-major array parameters: `$1..$n` are the
/// `text[]` dimension columns of [`Statement::dims`], then (upserts only)
/// [`Statement::values`].
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    /// The SQL text.
    pub sql: String,
    /// One `text[]` parameter per dimension column.
    pub dims: Vec<Vec<String>>,
    /// The value parameter (upserts only).
    pub values: Option<Values>,
}

/// An upsert's value parameter: `float8[]` for numbers, else `text[]` the
/// SQL casts to the column type.
#[derive(Debug, Clone, PartialEq)]
pub enum Values {
    /// A `float8[]`.
    Number(Vec<f64>),
    /// A `text[]` (booleans as `true`/`false`, datetimes as RFC 3339).
    Text(Vec<String>),
}

/// `v` as the text Postgres reads back as a `vt` value.
fn sql_text(vt: ValueType, v: &CellValue) -> Option<String> {
    match (vt, v) {
        (ValueType::Boolean, CellValue::Bool(b)) => Some(b.to_string()),
        (ValueType::Text, CellValue::Text(t)) => Some(t.clone()),
        (ValueType::DateTime, CellValue::Date(ms)) => {
            chrono::DateTime::from_timestamp_millis(*ms).map(|d| d.to_rfc3339())
        }
        _ => None,
    }
}

impl Statement {
    /// Result rows this statement changes.
    pub fn rows(&self) -> usize {
        match (self.dims.first(), &self.values) {
            (Some(d), _) => d.len(),
            (None, Some(Values::Number(v))) => v.len(),
            (None, Some(Values::Text(v))) => v.len(),
            // A dimensionless table's delete: its one row.
            (None, None) => 1,
        }
    }
}

/// The statements that turn `old` (the table's rows) into `new` (the engine's
/// rows) of a `vt` measure: at most one batched upsert of new/changed rows
/// and one batched delete of vanished rows. Unchanged rows (bit-identical
/// value) are skipped; a value that is not a `vt` is an error. With no
/// dimension columns (a dimensionless measure's one-row table) the upsert
/// replaces the table's row and the delete empties it.
pub fn diff_statements(
    table: &str,
    dim_cols: &[String],
    vt: ValueType,
    old: &ResultRows,
    new: &ResultRows,
) -> Result<Vec<Statement>> {
    let n = dim_cols.len();
    let columns = |keys: &mut dyn Iterator<Item = &Vec<String>>| {
        let mut cols = vec![Vec::new(); n];
        for k in keys {
            for (col, v) in cols.iter_mut().zip(k) {
                col.push(v.clone());
            }
        }
        cols
    };
    let unnest = |extra: &str| {
        let params: Vec<String> = (1..=n).map(|i| format!("${i}::text[]")).collect();
        format!("unnest({}{extra})", params.join(", "))
    };
    let cols = quoted_list(dim_cols);
    let mut out = Vec::new();

    let changed: Vec<(&Vec<String>, &CellValue)> = new
        .iter()
        .filter(|(k, v)| old.get(*k) != Some(*v))
        .collect();
    if !changed.is_empty() {
        let bad = |v: &CellValue| {
            Error::Catalog(format!("{table}: {v} is not a {}", value_type_name(vt)))
        };
        let (values, param) = match vt {
            ValueType::Number => (
                Values::Number(
                    changed
                        .iter()
                        .map(|(_, v)| v.as_num().ok_or_else(|| bad(v)))
                        .collect::<Result<_>>()?,
                ),
                "float8[]".to_string(),
            ),
            _ => (
                Values::Text(
                    changed
                        .iter()
                        .map(|(_, v)| sql_text(vt, v).ok_or_else(|| bad(v)))
                        .collect::<Result<_>>()?,
                ),
                format!("text[]::{}[]", sql_type(vt)?),
            ),
        };
        let sql = if n == 0 {
            format!(
                "WITH gone AS (DELETE FROM {table}) \
                 INSERT INTO {table} (value) SELECT * FROM unnest($1::{param})"
            )
        } else {
            format!(
                "INSERT INTO {table} ({cols}, value) SELECT * FROM {} \
                 ON CONFLICT ({cols}) DO UPDATE SET value = EXCLUDED.value",
                unnest(&format!(", ${}::{param}", n + 1))
            )
        };
        out.push(Statement {
            sql,
            dims: columns(&mut changed.iter().map(|(k, _)| *k)),
            values: Some(values),
        });
    }

    let gone: Vec<&Vec<String>> = old.keys().filter(|k| !new.contains_key(*k)).collect();
    if !gone.is_empty() {
        let sql = if n == 0 {
            format!("DELETE FROM {table}")
        } else {
            format!(
                "DELETE FROM {table} WHERE ({cols}) IN (SELECT * FROM {})",
                unnest("")
            )
        };
        out.push(Statement {
            sql,
            dims: columns(&mut gone.into_iter()),
            values: None,
        });
    }
    Ok(out)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    /// Time(1) × Product(2), inputs Price[Time,Product](1), Quantity[Time,Product](2).
    fn catalog() -> Catalog {
        Catalog {
            categories: vec![
                CategoryRow {
                    id: 1,
                    name: s("Time"),
                },
                CategoryRow {
                    id: 2,
                    name: s("Product"),
                },
            ],
            items: vec![],
            measures: vec![
                MeasureRow::input(1, "Price", vec![1, 2]),
                MeasureRow::input(2, "Quantity", vec![1, 2]),
            ],
            lambdas: vec![],
        }
    }

    /// P3-3: named LAMBDAs stored as rows, defined (with a category
    /// parameter), called by measures, and dropped once nothing calls them.
    #[test]
    fn named_lambdas_in_the_catalog() {
        let mut cat = catalog();
        let model = build_model(&cat).unwrap();
        let text = "LOGNORM = LAMBDA(x, c, LN1P(x) / LN1P(MAX(x OVER c)))";
        let Ok(Some(Defined::Lambda { row, callers })) = define_lambda(&model, text, false) else {
            panic!()
        };
        assert_eq!((row.id, row.name.as_str(), callers), (1, "LOGNORM", vec![]));
        cat.lambdas.push(row);
        let model = build_model(&cat).unwrap();
        cat.measures
            .push(define(&model, "PriceN = LOGNORM(Price, Time)", 3).unwrap());
        cat.measures
            .push(define(&model, "Qn = LOGNORM(Quantity, Product) * 2", 4).unwrap());
        let mut model = build_model(&cat).unwrap();
        assert_eq!(
            model.measures[&MeasureId(3)].categories,
            vec![CategoryId(1), CategoryId(2)]
        );
        load_inputs(&mut model, &sales()).unwrap();
        let out = improv_engine::dataflow::evaluate(&model, &[MeasureId(3), MeasureId(4)]).unwrap();
        let rows = result_rows(&model, MeasureId(4), &out[&MeasureId(4)]).unwrap();
        // Q2: Widget 200, Gadget 100; the max over Product is 200.
        let want = 2.0 * 100f64.ln_1p() / 200f64.ln_1p();
        assert!(
            matches!(rows[&vec![s("Q2"), s("Gadget")]], CellValue::Num(v) if (f64::from_bits(v) - want).abs() < 1e-12),
            "{rows:?}"
        );

        let def = |t: &str, replace| define_lambda(&model, t, replace);
        assert_eq!(def("Price = Quantity * 2", true), Ok(None));
        assert_eq!(def(text, true), Ok(Some(Defined::Unchanged(1))));
        assert_eq!(
            def("lognorm = LAMBDA( x,c, LN1P(x)/LN1P(MAX(x OVER c)) )", true),
            Ok(Some(Defined::Unchanged(1)))
        );
        let e = |r: Result<Option<Defined>>| r.unwrap_err().to_string();
        assert!(e(def(text, false)).contains("already exists"));
        let msg = e(def("quantity = LAMBDA(x, x)", false));
        assert!(msg.contains("Quantity is a measure"), "{msg}");
        assert!(e(def("X = LAMBDA(x, x + Price)", false)).contains("only its parameters"));
        assert!(matches!(
            def("Half = LAMBDA(x, x / 2)", false),
            Ok(Some(Defined::Lambda { row, .. })) if row.id == 2
        ));
        let err = define_measure(&model, "lognorm = Price * 2", 9, false).unwrap_err();
        assert!(err.to_string().contains("is a named LAMBDA"), "{err}");

        // Redefining: the callers to recompute; refusals name the dependent.
        let twice = "LOGNORM = LAMBDA(x, c, 2 * LN1P(x) / LN1P(MAX(x OVER c)))";
        assert!(matches!(
            def(twice, true),
            Ok(Some(Defined::Lambda { row, callers }))
                if row.id == 1 && row.definition == twice
                    && callers == vec![MeasureId(3), MeasureId(4)]
        ));
        let msg = e(def("LOGNORM = LAMBDA(x, c, x > MAX(x OVER c))", true));
        assert!(
            msg.contains("change the type or dimensions of PriceN"),
            "{msg}"
        );
        let msg = e(def("LOGNORM = LAMBDA(x, LN1P(x))", true));
        assert!(
            msg.contains("breaks PriceN") && msg.contains("takes 1"),
            "{msg}"
        );
        let msg = e(def("LOGNORM = LAMBDA(x, c, MAX(x OVER c))", true));
        assert!(
            msg.contains("change the type or dimensions of PriceN"),
            "{msg}"
        );
        let mut chain = cat.clone();
        chain.lambdas.push(LambdaRow {
            id: 2,
            name: s("Outer"),
            definition: s("Outer = LAMBDA(v, LOGNORM(v, Time) + 1)"),
        });
        chain.lambdas.push(LambdaRow {
            id: 3,
            name: s("Again"),
            definition: s("Again = LAMBDA(v, Outer(v))"),
        });
        let chained = build_model(&chain).unwrap();
        let msg = define_lambda(&chained, "LOGNORM = LAMBDA(x, c, Outer(x))", true)
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("LOGNORM cannot call Outer: Outer calls LOGNORM"),
            "{msg}"
        );
        // Found anywhere in the body: under an operator, or an argument.
        for body in ["-Again(x)", "Again(x) + 1", "1 + Again(x)", "ABS(Again(x))"] {
            let t = format!("LOGNORM = LAMBDA(x, c, {body})");
            let msg = define_lambda(&chained, &t, true).unwrap_err().to_string();
            assert!(msg.contains("cannot call Again"), "{body}: {msg}");
        }
        let e = drop_lambda(&chained, "Outer").unwrap_err().to_string();
        assert_eq!(
            e,
            "definition: cannot drop Outer: Again still calls it (drop or redefine it first)"
        );
        let msg = define_lambda(&chained, "LOGNORM = LAMBDA(x, x)", true)
            .unwrap_err()
            .to_string();
        assert!(msg.contains("breaks LAMBDA Outer, which calls it"), "{msg}");

        let e = drop_lambda(&model, "LogNorm").unwrap_err().to_string();
        assert!(
            e.contains("cannot drop LOGNORM: PriceN, Qn still call it"),
            "{e}"
        );
        assert_eq!(drop_lambda(&model, "PriceN"), Ok(None));
        cat.measures.truncate(2);
        assert_eq!(
            drop_lambda(&build_model(&cat).unwrap(), "lognorm"),
            Ok(Some(1))
        );

        // A stored row naming another lambda than it defines is refused.
        cat.lambdas[0].name = s("Other");
        let e = build_model(&cat).unwrap_err().to_string();
        assert!(
            e.contains("lambda 1 is named Other but defines LOGNORM"),
            "{e}"
        );
        cat.lambdas[0].definition = s("LOGNORM = LAMBDA(x, Nope(x))");
        assert!(build_model(&cat)
            .unwrap_err()
            .to_string()
            .contains("lambda 1:"));
    }

    fn row(t: &str, p: &str, v: f64) -> (Vec<Option<String>>, Option<CellValue>) {
        (vec![Some(s(t)), Some(s(p))], Some(CellValue::num(v)))
    }

    fn n(v: f64) -> CellValue {
        CellValue::num(v)
    }

    /// The canonical 4 quarters × 2 products fact table, price/qty columns.
    fn sales() -> Vec<BoundRows> {
        let mut price = BoundRows {
            measure_id: 1,
            rows: vec![],
        };
        let mut qty = BoundRows {
            measure_id: 2,
            rows: vec![],
        };
        for (q, t) in ["Q1", "Q2", "Q3", "Q4"].iter().enumerate() {
            let q = q as f64 + 1.0;
            price.rows.push(row(t, "Widget", 10.0));
            price.rows.push(row(t, "Gadget", 20.0));
            qty.rows.push(row(t, "Widget", 100.0 * q));
            qty.rows.push(row(t, "Gadget", 50.0 * q));
        }
        vec![price, qty]
    }

    #[test]
    fn name_validation() {
        for ok in ["plan", "Revenue", "a1_b", "X"] {
            validate_name("model", ok).unwrap();
        }
        let long = "a".repeat(MAX_NAME_LEN + 1);
        for bad in [
            "",
            "1abc",
            "_a",
            "a-b",
            "a b",
            "a\"b",
            "Value",
            "réal",
            long.as_str(),
        ] {
            assert!(
                matches!(validate_name("model", bad), Err(Error::InvalidName { .. })),
                "{bad:?} accepted"
            );
        }
        validate_name("model", &"a".repeat(MAX_NAME_LEN)).unwrap();
    }

    #[test]
    fn identifiers_are_quoted() {
        assert_eq!(quote_ident("a\"b"), "\"a\"\"b\"");
        assert_eq!(result_schema("Plan"), "improv_plan");
        assert_eq!(
            result_table("Plan", "Revenue"),
            "\"improv_plan\".\"revenue\""
        );
        let cols = vec![s("time"), s("product")];
        assert_eq!(
            create_table_sql("t", &cols, ValueType::Number).unwrap(),
            "CREATE TABLE IF NOT EXISTS t (\"time\" text NOT NULL, \"product\" text NOT NULL, \
             value double precision, PRIMARY KEY (\"time\", \"product\"))"
        );
        assert!(create_table_sql("t", &cols, ValueType::Boolean)
            .unwrap()
            .contains("value boolean,"));
        assert!(create_table_sql("t", &cols, ValueType::Enum).is_err());
        assert_eq!(
            select_result_sql("t", &cols, ValueType::Number).unwrap(),
            "SELECT \"time\", \"product\", value::float8 FROM t"
        );
        assert_eq!(
            select_result_sql("t", &cols, ValueType::DateTime).unwrap(),
            "SELECT \"time\", \"product\", \
             (extract(epoch from value::timestamptz) * 1000)::int8 FROM t"
        );
        let num = ValueType::Number;
        assert_eq!(
            select_bound_sql("public.sales", &cols, "price", None, num).unwrap(),
            "SELECT \"time\"::text, \"product\"::text, \"price\"::float8 FROM public.sales"
        );
        assert_eq!(
            select_bound_sql("s", &cols, "price", Some("avg"), num).unwrap(),
            "SELECT \"time\"::text, \"product\"::text, avg(\"price\")::float8 FROM s GROUP BY 1, 2"
        );
        // Postgres has no max(bool) or min(bool): bool_or and bool_and are.
        let bool = ValueType::Boolean;
        assert_eq!(
            select_bound_sql("s", &cols, "ok", Some("max"), bool).unwrap(),
            "SELECT \"time\"::text, \"product\"::text, bool_or(\"ok\")::bool FROM s GROUP BY 1, 2"
        );
        assert_eq!(
            select_bound_sql("s", &cols, "ok", Some("min"), bool).unwrap(),
            "SELECT \"time\"::text, \"product\"::text, bool_and(\"ok\")::bool FROM s GROUP BY 1, 2"
        );
        assert_eq!(
            select_bound_sql("s", &cols, "ok", Some("count"), num).unwrap(),
            "SELECT \"time\"::text, \"product\"::text, count(\"ok\")::float8 FROM s GROUP BY 1, 2"
        );
        assert_eq!(
            select_bound_sql("s", &cols, "price", Some("min"), num).unwrap(),
            "SELECT \"time\"::text, \"product\"::text, min(\"price\")::float8 FROM s GROUP BY 1, 2"
        );
        assert!(select_bound_sql("s", &cols, "price", Some("median; --"), num).is_err());
        assert_eq!(agg_sql("count").unwrap(), "count");
        assert_eq!(
            select_bound_sql("s", &cols, "sha", Some("count_distinct"), num).unwrap(),
            "SELECT d1, d2, count(v)::float8 FROM (SELECT DISTINCT \"time\"::text AS d1, \
             \"product\"::text AS d2, \"sha\" AS v FROM s) improv_distinct GROUP BY 1, 2"
        );
    }

    #[test]
    fn catalog_to_model_and_define() {
        let mut cat = catalog();
        let model = build_model(&cat).unwrap();
        assert_eq!(model.categories.len(), 2);
        assert!(matches!(
            model.measure_by_name("Price").unwrap().kind,
            MeasureKind::Input
        ));

        let rev = define(&model, " Revenue = Price * Quantity ", 3).unwrap();
        assert_eq!(rev.formula.as_deref(), Some("Revenue = Price * Quantity"));
        assert_eq!(rev.categories, vec![1, 2]);
        cat.measures.push(rev);
        let model = build_model(&cat).unwrap();
        let total = define(&model, "Total = SUM(Revenue OVER Time)", 4).unwrap();
        assert_eq!(total.categories, vec![2], "OVER Time drops Time");
        cat.measures.push(total);

        // Derived rows load after inputs regardless of row order.
        cat.measures.reverse();
        let model = build_model(&cat).unwrap();
        assert!(matches!(
            model.measure_by_name("Total").unwrap().kind,
            MeasureKind::Derived(_)
        ));
        assert_eq!(
            dim_columns(&model, MeasureId(3)).unwrap(),
            vec![s("time"), s("product")]
        );
    }

    #[test]
    fn define_rejections() {
        let model = build_model(&catalog()).unwrap();
        let err = |t: &str| define(&model, t, 9).unwrap_err();
        assert!(
            matches!(err("Price = Quantity * 2"), Error::Definition(m) if m.contains("exists"))
        );
        assert!(matches!(err("Bad = Nope * 2"), Error::Definition(_)));
        // A dimensionless result (P3-2): a one-row table.
        assert!(define(&model, "Two = 2", 9).unwrap().categories.is_empty());
        assert!(
            matches!(err("X = SQL(\"select 1\")"), Error::Definition(m) if m.contains("not supported"))
        );
        assert!(
            matches!(err("X = CALL(f, Price)"), Error::Definition(m) if m.contains("not supported"))
        );
        assert!(matches!(
            err("value = Price * 2"),
            Error::InvalidName { .. }
        ));
        assert!(define(&model, "Big = Price > 2", 9).unwrap().value_type == "boolean");
        assert!(matches!(err("garbage"), Error::Definition(_)));
    }

    /// P1-1: replace and drop decisions, each refusal naming the dependent.
    #[test]
    fn redefine_and_drop() {
        let mut cat = catalog();
        let model = build_model(&cat).unwrap();
        cat.measures
            .push(define(&model, "Revenue = Price * Quantity", 3).unwrap());
        let model = build_model(&cat).unwrap();
        cat.measures
            .push(define(&model, "Total = SUM(Revenue OVER Time)", 4).unwrap());
        let model = build_model(&cat).unwrap();
        let def = |t: &str| define_measure(&model, t, 9, true);
        let err = |t: &str| match def(t) {
            Err(Error::Definition(m)) => m,
            other => panic!("{t}: {other:?}"),
        };
        assert_eq!(
            def("Total = SUM(Revenue OVER Time)"),
            Ok(Defined::Unchanged(4))
        );
        assert_eq!(
            def("total=SUM( Revenue OVER Time )"),
            Ok(Defined::Unchanged(4))
        );
        let Ok(Defined::Replaced { row, reshaped }) = def("Revenue = Price * Quantity * 2") else {
            panic!()
        };
        assert_eq!((row.id, row.name.as_str(), reshaped), (3, "Revenue", false));
        assert_eq!(
            row.formula.as_deref(),
            Some("Revenue = Price * Quantity * 2")
        );
        let Ok(Defined::Replaced { reshaped, row }) = def("Total = SUM(Revenue OVER Product)")
        else {
            panic!()
        };
        assert!(reshaped, "Total moves from Product to Time");
        assert_eq!(row.categories, vec![1]);
        assert!(err("Revenue = Price > 2").contains("from number to boolean, but Total reads it"));
        // Nothing reads Total: it may change its type.
        let Ok(Defined::Replaced { row, reshaped }) = def("Total = SUM(Revenue OVER Time) > 1")
        else {
            panic!()
        };
        assert_eq!((row.value_type.as_str(), reshaped), ("boolean", true));
        assert!(err("Revenue = Total * 2").contains("Total depends on Revenue"));
        // Total still sums Revenue[Product] over Time, so it keeps its shape.
        assert!(matches!(
            def("Revenue = SUM(Price OVER Time)"),
            Ok(Defined::Replaced { reshaped: true, .. })
        ));
        let mut wider = cat.clone();
        wider
            .measures
            .push(define(&model, "Double = Revenue * 2", 5).unwrap());
        let wide = build_model(&wider).unwrap();
        let e = define_measure(&wide, "Revenue = SUM(Price OVER Time)", 9, true).unwrap_err();
        assert!(e.to_string().contains("dimensions of Double"), "{e}");
        assert!(err("Price = Quantity").contains("input measure"));
        assert!(matches!(def("New = Price"), Ok(Defined::New(r)) if r.id == 9));
        assert!(define_measure(&model, "Total = Revenue", 9, false)
            .is_err_and(|e| e.to_string().contains("replace => true")));

        assert_eq!(referencing(&model, MeasureId(3)), vec![s("Total")]);
        assert_eq!(drop_target(&model, "total"), Ok(MeasureId(4)));
        let e = drop_target(&model, "Price").unwrap_err().to_string();
        assert!(
            e.contains("cannot drop Price: Revenue still references it"),
            "{e}"
        );
        assert!(drop_target(&model, "Nope").is_err());

        // A formula may read a measure with a higher id (a redefinition did).
        cat.measures[2].formula = Some(s("Revenue = Total * 1"));
        cat.measures[3].formula = Some(s("Total = SUM(Price OVER Time)"));
        let model = build_model(&cat).unwrap();
        assert_eq!(
            model.measures[&MeasureId(3)].kind,
            MeasureKind::Derived(
                parser::parse_formula(&model, "Revenue = Total * 1")
                    .unwrap()
                    .formula
            )
        );
    }

    #[test]
    fn bad_catalogs() {
        let mut cat = catalog();
        cat.items.push(ItemRow {
            id: 1,
            category_id: 7,
            name: s("x"),
        });
        assert!(matches!(build_model(&cat), Err(Error::Catalog(_))));

        let mut cat = catalog();
        let mut bad = MeasureRow::input(3, "Revenue", vec![1, 2]);
        bad.kind = s("derived");
        bad.formula = Some(s("Other = Price * 2"));
        cat.measures.push(bad.clone());
        assert!(matches!(build_model(&cat), Err(Error::Catalog(_))));
        bad.formula = None;
        cat.measures[2] = bad.clone();
        assert!(matches!(build_model(&cat), Err(Error::Catalog(m)) if m.contains("kind")));
        bad.kind = s("input");
        bad.value_type = s("money");
        cat.measures[2] = bad;
        assert!(matches!(build_model(&cat), Err(Error::Catalog(m)) if m.contains("money")));

        let mut cat = catalog();
        cat.measures[0].value_type = s("text");
        let mut model = build_model(&cat).unwrap();
        let errs = BoundRows {
            measure_id: 1,
            rows: vec![(vec![Some(s("Q1")), Some(s("W"))], Some(CellValue::Err(0)))],
        };
        assert!(matches!(
            load_inputs(&mut model, &[errs]),
            Err(Error::Catalog(m)) if m.contains("out of range")
        ));

        let mut model = build_model(&catalog()).unwrap();
        let unknown = BoundRows {
            measure_id: 99,
            rows: vec![],
        };
        assert!(matches!(
            load_inputs(&mut model, &[unknown]),
            Err(Error::Catalog(_))
        ));
        let short = BoundRows {
            measure_id: 1,
            rows: vec![(vec![Some(s("Q1"))], Some(n(1.0)))],
        };
        assert!(matches!(
            load_inputs(&mut model, &[short]),
            Err(Error::Catalog(_))
        ));
        assert!(dim_columns(&model, MeasureId(99)).is_err());
        assert!(result_rows(&model, MeasureId(99), &HashMap::new()).is_err());
    }

    #[test]
    fn catalog_contract_with_storage_pg() {
        // Stored category order is kept (not re-sorted); description and
        // value type are honoured; a JSON formula (what storage_pg wrote
        // before it switched to text) is read too.
        let mut cat = catalog();
        cat.measures[0].categories = vec![2, 1];
        cat.measures[0].description = Some(s("list price"));
        let model = build_model(&cat).unwrap();
        let price = model.measure_by_name("Price").unwrap();
        assert_eq!(price.categories, vec![CategoryId(2), CategoryId(1)]);
        assert_eq!(price.description.as_deref(), Some("list price"));
        assert_eq!(
            dim_columns(&model, MeasureId(1)).unwrap(),
            vec![s("product"), s("time")]
        );

        let rev = define(&model, "Revenue = Price * Quantity", 3).unwrap();
        let mut with_text = cat.clone();
        with_text.measures.push(rev.clone());
        let parsed = build_model(&with_text).unwrap().measures[&MeasureId(3)]
            .kind
            .clone();
        let MeasureKind::Derived(f) = parsed else {
            panic!("derived")
        };
        let mut json = rev;
        json.formula = Some(serde_json::to_string(&f).unwrap());
        cat.measures.push(json);
        let model = build_model(&cat).unwrap();
        assert_eq!(model.measures[&MeasureId(3)].kind, MeasureKind::Derived(f));
        assert_eq!(value_type_name(ValueType::Enum), "enum");
        assert_eq!(value_type_name(ValueType::Text), "text");
    }

    /// The definitions `define` and `define_lambda` refuse, each naming why.
    #[test]
    fn definition_refusals_name_their_cause() {
        let mut cat = catalog();
        let model = build_model(&cat).unwrap();
        let err = |r: Result<MeasureRow>| r.unwrap_err().to_string();
        let msg = err(define(&model, "Half = LAMBDA(x, x / 2)", 9));
        assert!(
            msg.contains("defines a named LAMBDA, not a measure"),
            "{msg}"
        );
        // A quoted target is found only once parsed.
        let msg = err(define(&model, "'Price' = 1", 9));
        assert_eq!(msg, "definition: measure Price already exists");
        let msg = err(define(&model, "X = Price + \"a\"", 9));
        assert!(msg.contains("type mismatch"), "{msg}");
        // Looks like a lambda (`… = LAMBDA(`), but its target is quoted text.
        let msg = define_lambda(&model, "'x = LAMBDA(' = 1", false)
            .unwrap_err()
            .to_string();
        assert!(msg.contains("is not Name = LAMBDA(…)"), "{msg}");
        // A derived measure may not have an enum result (yet).
        cat.measures[0].value_type = s("enum");
        cat.measures
            .push(define(&model, "D = Quantity * 2", 3).unwrap());
        let model = build_model(&cat).unwrap();
        let msg = err(define(&model, "E = Price", 9));
        assert!(msg.contains("E is an enum; enum results"), "{msg}");
        let redefine = |t: &str| define_measure(&model, t, 9, true).unwrap_err().to_string();
        assert!(redefine("D = Price").contains("D is an enum"));
        assert!(redefine("D = Quantity + \"a\"").contains("type mismatch"));
    }

    #[test]
    fn inputs_keep_their_shape() {
        let model = build_model(&catalog()).unwrap();
        assert_eq!(
            keep_shapes(&model, "X", &[MeasureId(1), MeasureId(2)]),
            Ok(())
        );
    }

    /// A redefinition is checked against every dependent, also one that
    /// depends on it only through another measure.
    #[test]
    fn redefinitions_keep_every_dependent_valid() {
        let mut cat = catalog();
        cat.measures.push(MeasureRow::input(5, "Cost", vec![1, 2]));
        let model = build_model(&cat).unwrap();
        cat.measures
            .push(define(&model, "D = Quantity * 2", 3).unwrap());
        let model = build_model(&cat).unwrap();
        cat.measures
            .push(define(&model, "Prev = PREV(D, Time)", 4).unwrap());
        let model = build_model(&cat).unwrap();
        let redefine = |t: &str| define_measure(&model, t, 9, true);
        let msg = redefine("D = SUM(Quantity OVER Time)")
            .unwrap_err()
            .to_string();
        assert!(
            msg.contains("redefining D breaks Prev, which depends on it")
                && msg.contains("does not range over Time"),
            "{msg}"
        );
        assert!(matches!(
            redefine("D = Cost * 2"),
            Ok(Defined::Replaced {
                reshaped: false,
                ..
            })
        ));
        // Two readers are named together.
        cat.measures.push(define(&model, "Neg = -D", 6).unwrap());
        let model = build_model(&cat).unwrap();
        let msg = define_measure(&model, "D = Quantity > 1", 9, true)
            .unwrap_err()
            .to_string();
        assert!(msg.contains("but Neg, Prev read it"), "{msg}");
        let msg = drop_target(&model, "d").unwrap_err().to_string();
        assert_eq!(
            msg,
            "definition: cannot drop D: Neg, Prev still reference it (drop or redefine them first)"
        );
    }

    #[test]
    fn unbound_cells_from_improv_cell() {
        let mut cat = catalog();
        cat.items = vec![
            ItemRow {
                id: 1,
                category_id: 1,
                name: s("Q1"),
            },
            ItemRow {
                id: 2,
                category_id: 2,
                name: s("Widget"),
            },
        ];
        for (id, vt) in [(5, "boolean"), (6, "text"), (7, "enum"), (8, "datetime")] {
            let mut m = MeasureRow::input(id, &format!("M{id}"), vec![1, 2]);
            m.value_type = s(vt);
            cat.measures.push(m);
        }
        let mut model = build_model(&cat).unwrap();
        let c = s(r#"{"1": 1, "2": 2}"#);
        let cell = |m: u32, v: &str| (m, c.clone(), s(v));
        let n = load_cells(
            &mut model,
            &[
                cell(1, "4.5"),
                cell(5, "true"),
                cell(6, "\"x\""),
                cell(7, "3"),
            ],
        )
        .unwrap();
        assert_eq!(n, 4);
        let coord =
            Coordinate::from_pairs([(CategoryId(1), ItemId(1)), (CategoryId(2), ItemId(2))]);
        assert_eq!(
            model.inputs[&(MeasureId(1), coord.clone())],
            Value::Number(4.5)
        );
        assert_eq!(model.inputs[&(MeasureId(7), coord.clone())], Value::Enum(3));
        load_cells(&mut model, &[cell(8, "\"2026-01-01T00:00:00Z\"")]).unwrap();
        assert!(matches!(
            model.inputs[&(MeasureId(8), coord)],
            Value::DateTime(_)
        ));

        for bad in [
            cell(9, "1"),
            cell(1, "\"x\""),
            cell(8, "\"yesterday\""),
            (1, s("[1]"), s("1")),
            (1, c.clone(), s("nope")),
        ] {
            assert!(matches!(
                load_cells(&mut model, &[bad]),
                Err(Error::Catalog(_))
            ));
        }
    }

    #[test]
    fn dim_columns_unknown_category() {
        let mut cat = catalog();
        cat.measures[0].categories = vec![1, 5];
        let model = build_model(&cat).unwrap();
        assert!(matches!(
            dim_columns(&model, MeasureId(1)),
            Err(Error::Catalog(_))
        ));
    }

    #[test]
    fn rows_to_cells_mint_stable_items() {
        let mut cat = catalog();
        let mut model = build_model(&cat).unwrap();
        let loaded = load_inputs(&mut model, &sales()).unwrap();
        assert_eq!(loaded.cells, 16);
        assert_eq!(loaded.null_dim_rows, 0);
        // Lexical per category, categories in id order: Q1..Q4, then Gadget, Widget.
        let names: Vec<(u32, &str)> = loaded
            .new_items
            .iter()
            .map(|i| (i.id, i.name.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                (1, "Q1"),
                (2, "Q2"),
                (3, "Q3"),
                (4, "Q4"),
                (5, "Gadget"),
                (6, "Widget")
            ]
        );

        // Persist, reload, load again plus a new product: old ids hold, the
        // new value mints above them, nothing old is re-minted.
        cat.items = loaded.new_items.clone();
        let mut model = build_model(&cat).unwrap();
        let mut more = sales();
        more[0].rows.push(row("Q1", "Doohickey", 5.0));
        assert_eq!(
            unknown_items(&model, &more).unwrap(),
            vec![(CategoryId(2), s("Doohickey"))]
        );
        let again = load_inputs(&mut model, &more).unwrap();
        assert_eq!(again.cells, 17);
        assert_eq!(
            again.new_items,
            vec![ItemRow {
                id: 7,
                category_id: 2,
                name: s("Doohickey")
            }]
        );
        let widget = model.items.values().find(|i| i.name.0 == "Widget").unwrap();
        assert_eq!(widget.id, ItemId(6));
    }

    #[test]
    fn nulls_and_duplicates() {
        let mut model = build_model(&catalog()).unwrap();
        let b = BoundRows {
            measure_id: 1,
            rows: vec![
                (vec![None, Some(s("Widget"))], Some(n(1.0))),
                (vec![Some(s("Q1")), Some(s("Widget"))], None),
                row("Q2", "Widget", 3.0),
            ],
        };
        let loaded = load_inputs(&mut model, &[b]).unwrap();
        assert_eq!((loaded.cells, loaded.null_dim_rows), (1, 1));

        let mut model = build_model(&catalog()).unwrap();
        let dup = BoundRows {
            measure_id: 1,
            rows: vec![row("Q1", "Widget", 1.0), row("Q1", "Widget", 2.0)],
        };
        assert_eq!(
            load_inputs(&mut model, &[dup]).unwrap_err(),
            Error::DuplicateCoordinate {
                measure: s("Price"),
                coord: vec![s("Q1"), s("Widget")]
            }
        );
    }

    #[test]
    fn cell_budget() {
        check_cell_budget(5, 5).unwrap();
        assert_eq!(
            check_cell_budget(6, 5).unwrap_err().to_string(),
            "6 bound input cells exceed improv.max_cells = 5"
        );
    }

    /// The full pure path: catalog → model → inputs → engine → named rows →
    /// statements, with the canonical oracle values.
    #[test]
    fn engine_results_to_statements() {
        let mut cat = catalog();
        let model = build_model(&cat).unwrap();
        cat.measures
            .push(define(&model, "Revenue = Price * Quantity", 3).unwrap());
        let model = build_model(&cat).unwrap();
        cat.measures
            .push(define(&model, "Total = SUM(Revenue OVER Time)", 4).unwrap());
        let mut model = build_model(&cat).unwrap();
        load_inputs(&mut model, &sales()).unwrap();

        let out = improv_engine::dataflow::evaluate(&model, &[MeasureId(3), MeasureId(4)]).unwrap();
        let rev = result_rows(&model, MeasureId(3), &out[&MeasureId(3)]).unwrap();
        assert_eq!(rev.len(), 8);
        assert_eq!(rev[&vec![s("Q3"), s("Widget")]], n(3000.0));
        assert_eq!(rev[&vec![s("Q4"), s("Gadget")]], n(4000.0));
        let total = result_rows(&model, MeasureId(4), &out[&MeasureId(4)]).unwrap();
        // Widget: 10*(100+200+300+400) ; Gadget: 20*(50+100+150+200)
        assert_eq!(
            total,
            BTreeMap::from([
                (vec![s("Gadget")], n(10000.0)),
                (vec![s("Widget")], n(10000.0))
            ])
        );

        let cols = dim_columns(&model, MeasureId(3)).unwrap();
        let table = result_table("plan", "Revenue");
        let num = ValueType::Number;
        let fresh = diff_statements(&table, &cols, num, &ResultRows::new(), &rev).unwrap();
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].rows(), 8);
        assert!(fresh[0].sql.starts_with(
            "INSERT INTO \"improv_plan\".\"revenue\" (\"time\", \"product\", value) \
             SELECT * FROM unnest($1::text[], $2::text[], $3::float8[]) ON CONFLICT"
        ));
        assert!(diff_statements(&table, &cols, num, &rev, &rev)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn diff_upserts_changes_and_deletes_vanished() {
        let cols = vec![s("product")];
        let rows = |v: &[(&str, f64)]| -> ResultRows {
            v.iter().map(|(k, x)| (vec![s(k)], n(*x))).collect()
        };
        let old = rows(&[("a", 1.0), ("b", 2.0), ("c", 3.0)]);
        let new = rows(&[("a", 1.0), ("b", 2.5), ("d", 4.0)]);
        let st = diff_statements("t", &cols, ValueType::Number, &old, &new).unwrap();
        assert_eq!(st.len(), 2);
        assert_eq!(st[0].dims, vec![vec![s("b"), s("d")]]);
        assert_eq!(st[0].values, Some(Values::Number(vec![2.5, 4.0])));
        assert_eq!(
            st[1].sql,
            "DELETE FROM t WHERE (\"product\") IN (SELECT * FROM unnest($1::text[]))"
        );
        assert_eq!(st[1].dims, vec![vec![s("c")]]);
        assert_eq!(st[1].values, None);
        assert_eq!(st.iter().map(Statement::rows).sum::<usize>(), 3);
        // No dimension columns and no values: a dimensionless table's delete.
        assert_eq!(
            Statement {
                sql: s(""),
                dims: vec![],
                values: None
            }
            .rows(),
            1
        );
    }

    /// P3-2: a measure with no categories gets a one-row table of just
    /// `value`; its diff replaces or deletes that row.
    #[test]
    fn dimensionless_results() {
        let mut cat = catalog();
        let model = build_model(&cat).unwrap();
        let row = define(&model, "MaxQ = MAX(Quantity OVER Time, Product)", 3).unwrap();
        assert!(row.categories.is_empty());
        cat.measures.push(row);
        let mut model = build_model(&cat).unwrap();
        load_inputs(&mut model, &sales()).unwrap();
        let out = improv_engine::dataflow::evaluate(&model, &[MeasureId(3)]).unwrap();
        let rows = result_rows(&model, MeasureId(3), &out[&MeasureId(3)]).unwrap();
        assert_eq!(rows, BTreeMap::from([(vec![], n(400.0))]));
        let cols = dim_columns(&model, MeasureId(3)).unwrap();
        assert!(cols.is_empty());
        assert_eq!(
            create_table_sql("t", &cols, ValueType::Number).unwrap(),
            "CREATE TABLE IF NOT EXISTS t (value double precision)"
        );
        assert_eq!(
            select_result_sql("t", &cols, ValueType::Number).unwrap(),
            "SELECT value::float8 FROM t"
        );
        let num = ValueType::Number;
        let none = ResultRows::new();
        let st = diff_statements("t", &cols, num, &none, &rows).unwrap();
        assert_eq!(st.len(), 1);
        assert_eq!(
            st[0].sql,
            "WITH gone AS (DELETE FROM t) INSERT INTO t (value) SELECT * FROM unnest($1::float8[])"
        );
        assert_eq!((st[0].rows(), st[0].dims.len()), (1, 0));
        assert!(diff_statements("t", &cols, num, &rows, &rows)
            .unwrap()
            .is_empty());
        let st = diff_statements("t", &cols, num, &rows, &none).unwrap();
        assert_eq!((st[0].sql.as_str(), st[0].rows()), ("DELETE FROM t", 1));
        let text = ValueType::Text;
        let one = BTreeMap::from([(vec![], CellValue::Text(s("x")))]);
        let st = diff_statements("t", &cols, text, &none, &one).unwrap();
        assert!(
            st[0].sql.ends_with("unnest($1::text[]::text[])"),
            "{}",
            st[0].sql
        );
        assert_eq!(st[0].rows(), 1);
    }

    #[test]
    fn result_rows_skips_non_numeric_and_rejects_foreign_coords() {
        let mut cat = catalog();
        cat.items = vec![
            ItemRow {
                id: 1,
                category_id: 1,
                name: s("Q1"),
            },
            ItemRow {
                id: 2,
                category_id: 2,
                name: s("Widget"),
            },
        ];
        let model = build_model(&cat).unwrap();
        let cells = HashMap::from([
            (CoordKey::from_slice(&[(1, 1), (2, 2)]), CellValue::num(7.0)),
            (
                CoordKey::from_slice(&[(1, 1), (2, 2), (3, 3)]),
                CellValue::Text(s("x")),
            ),
        ]);
        let rows = result_rows(&model, MeasureId(1), &cells).unwrap();
        assert_eq!(
            rows,
            ResultRows::from([(vec![s("Q1"), s("Widget")], n(7.0))])
        );
        let foreign = HashMap::from([(CoordKey::from_slice(&[(1, 1)]), CellValue::num(1.0))]);
        assert!(result_rows(&model, MeasureId(1), &foreign).is_err());
    }

    /// G5: column types map to value types; typed rows load, and typed
    /// results diff into `text[]` upserts cast to the column type.
    #[test]
    fn typed_bindings_and_results() {
        let vt = |t: &str, agg| bound_value_type(t, t, agg);
        for (t, want) in [
            ("numeric", ValueType::Number),
            ("int8", ValueType::Number),
            ("bool", ValueType::Boolean),
            ("varchar", ValueType::Text),
            ("bpchar", ValueType::Text),
            ("date", ValueType::DateTime),
            ("timestamptz", ValueType::DateTime),
        ] {
            assert_eq!(vt(t, None).unwrap(), want, "{t}");
        }
        assert_eq!(vt("text", Some("count")).unwrap(), ValueType::Number);
        assert_eq!(
            vt("text", Some("count_distinct")).unwrap(),
            ValueType::Number
        );
        assert_eq!(vt("bool", Some("max")).unwrap(), ValueType::Boolean);
        assert!(matches!(vt("jsonb", None), Err(Error::Catalog(m)) if m.contains("type jsonb")));
        assert!(matches!(vt("text", Some("sum")), Err(Error::Catalog(m)) if m.contains("number")));
        assert!(matches!(vt("bool", Some("avg")), Err(Error::Catalog(m)) if m.contains("agg avg")));
        assert_eq!(vt("int4", Some("sum")).unwrap(), ValueType::Number);
        assert_eq!(vt("numeric", Some("avg")).unwrap(), ValueType::Number);
        assert!(vt("int4", Some("median")).is_err());
        assert_eq!(sql_type(ValueType::Text).unwrap(), "text");
        assert!(sql_type(ValueType::Enum).is_err());
        let cols = vec![s("p")];
        assert_eq!(
            select_bound_sql("s", &cols, "v", None, ValueType::Text).unwrap(),
            "SELECT \"p\"::text, \"v\"::text FROM s"
        );
        assert!(select_bound_sql("s", &cols, "v", None, ValueType::Enum).is_err());
        check_value_column("t", ValueType::Boolean, "boolean").unwrap();
        assert!(matches!(
            check_value_column("t", ValueType::Boolean, "double precision"),
            Err(Error::Catalog(m)) if m.contains("replace => true")
        ));

        let mut cat = catalog();
        cat.measures[0].value_type = s("text");
        cat.measures[1].value_type = s("datetime");
        let mut model = build_model(&cat).unwrap();
        let at = |v: CellValue| vec![(vec![Some(s("Q1")), Some(s("Widget"))], Some(v))];
        let bound = [
            BoundRows {
                measure_id: 1,
                rows: at(CellValue::Text(s("tools"))),
            },
            BoundRows {
                measure_id: 2,
                rows: at(CellValue::Date(86_400_000)),
            },
        ];
        load_inputs(&mut model, &bound).unwrap();
        let got: BTreeSet<String> = model.inputs.values().map(|v| format!("{v:?}")).collect();
        assert!(got.contains("Text(\"tools\")"), "{got:?}");
        assert!(
            got.iter().any(|v| v.starts_with("DateTime(1970-01-02")),
            "{got:?}"
        );
        let huge = [BoundRows {
            measure_id: 2,
            rows: at(CellValue::Date(i64::MAX)),
        }];
        assert!(load_inputs(&mut model, &huge).is_err());
        cat.measures[1].value_type = s("boolean");
        let mut model = build_model(&cat).unwrap();
        let flag = [BoundRows {
            measure_id: 2,
            rows: at(CellValue::Bool(true)),
        }];
        load_inputs(&mut model, &flag).unwrap();
        assert_eq!(
            model.inputs.values().collect::<Vec<_>>(),
            [&Value::Boolean(true)]
        );

        let cols = vec![s("product")];
        let one = |v: CellValue| ResultRows::from([(vec![s("a")], v)]);
        let st = diff_statements(
            "t",
            &cols,
            ValueType::Boolean,
            &ResultRows::new(),
            &one(CellValue::Bool(true)),
        )
        .unwrap();
        assert!(st[0].sql.contains("$2::text[]::boolean[]"), "{}", st[0].sql);
        assert_eq!(st[0].values, Some(Values::Text(vec![s("true")])));
        let dt = ValueType::DateTime;
        let st =
            diff_statements("t", &cols, dt, &ResultRows::new(), &one(CellValue::Date(0))).unwrap();
        assert_eq!(
            st[0].values,
            Some(Values::Text(vec![s("1970-01-01T00:00:00+00:00")]))
        );
        let text = one(CellValue::Text(s("x")));
        assert!(diff_statements("t", &cols, ValueType::Number, &ResultRows::new(), &text).is_err());
        assert!(diff_statements("t", &cols, dt, &ResultRows::new(), &text).is_err());
    }
}
