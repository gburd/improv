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

pub mod pgoutput;

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use improv_core_model::parser::{self, Definition};
use improv_core_model::{
    CategoryId, Coordinate, Formula, ItemId, Measure, MeasureId, MeasureKind, Model, Name, Value,
    ValueType,
};
use improv_data_source::ItemInterner;
use improv_engine::compiler::{infer, CompileContext};
use improv_engine::{CellValue, CoordKey};

mod delta;
pub use delta::{
    apply_edits, cell_changes, coord_json, dependents, measure_ids, plan, quote_literal, slot_name,
    sync_trigger_name, touched_sql, Plan, Scope,
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
}

/// Parse a `Target = expr` definition, rejecting the `SQL(...)`/`CALL(...)`
/// source forms (external code never runs inside Postgres in v1).
fn parse_formula(model: &Model, text: &str) -> Result<(String, Formula)> {
    match parser::parse_definition(model, text).map_err(|e| Error::Definition(e.to_string()))? {
        Definition::Formula(ft) => Ok((ft.target.0, ft.formula)),
        _ => Err(Error::Definition(
            "SQL(...) and CALL(...) definitions are not supported inside Postgres".into(),
        )),
    }
}

/// Build a [`Model`] (structure only, no input cells) from catalog rows.
/// Input measures are added first, then derived measures in id order, so a
/// text formula may reference any input and any earlier derived measure.
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
    rows.sort_by_key(|r| (r.is_derived(), r.id));
    for r in rows {
        let kind = match (r.kind.as_str(), &r.formula) {
            ("input", _) => MeasureKind::Input,
            ("derived", Some(text)) => match serde_json::from_str::<Formula>(text) {
                Ok(f) => MeasureKind::Derived(f),
                Err(_) => {
                    let (target, formula) = parse_formula(&model, text)?;
                    if target != r.name {
                        return Err(Error::Catalog(format!(
                            "measure {} has a formula for {target}",
                            r.name
                        )));
                    }
                    MeasureKind::Derived(formula)
                }
            },
            (kind, _) => {
                return Err(Error::Catalog(format!(
                    "measure {} has kind {kind:?} and formula {:?}",
                    r.name, r.formula
                )))
            }
        };
        model.add_measure(Measure {
            id: MeasureId(r.id),
            name: Name(r.name.clone()),
            value_type: value_type_parse(&r.value_type)?,
            categories: r.categories.iter().map(|&c| CategoryId(c)).collect(),
            kind,
            description: r.description.clone(),
        });
    }
    Ok(model)
}

/// Validate a `Target = expr` definition against `model` and return the
/// catalog row for the new derived measure `id`. Its categories are the
/// engine-inferred result dimensions (an `OVER` aggregation drops the
/// aggregated category), which is exactly the result table's shape.
pub fn define(model: &Model, text: &str, id: u32) -> Result<MeasureRow> {
    let (target, formula) = parse_formula(model, text)?;
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
    let typed = infer(
        &CompileContext::new(&model.measures),
        MeasureId(id),
        &formula.expr,
    )
    .map_err(|e| Error::Definition(e.to_string()))?;
    if typed.ty.value_type != ValueType::Number {
        return Err(Error::Definition(format!(
            "{target} is {}; only number results are supported yet",
            value_type_name(typed.ty.value_type)
        )));
    }
    if typed.ty.dim.categories.is_empty() {
        return Err(Error::Definition(format!(
            "{target} has no dimensions; dimensionless results are not supported yet"
        )));
    }
    Ok(MeasureRow {
        id,
        name: target,
        value_type: value_type_name(typed.ty.value_type).into(),
        kind: "derived".into(),
        formula: Some(text.trim().to_string()),
        description: None,
        categories: typed.ty.dim.categories.iter().map(|c| c.0).collect(),
    })
}

/// The rows of one bound input measure, as read from its fact table: one
/// `Option<String>` per dimension (in the measure's category order) and the
/// value (`None` = SQL NULL).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BoundRows {
    /// The input measure the rows feed.
    pub measure_id: u32,
    /// `(dimension values, value)` per fact row.
    pub rows: Vec<(Vec<Option<String>>, Option<f64>)>,
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
        if m.value_type != ValueType::Number {
            return Err(Error::Catalog(format!(
                "{} is bound but is {}; only number bindings are supported yet",
                m.name.0,
                value_type_name(m.value_type)
            )));
        }
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

    let mut cells: BTreeMap<(MeasureId, Coordinate), f64> = BTreeMap::new();
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
                    e.insert(*v);
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
    for ((mid, coord), v) in cells {
        model.set_input(mid, coord, Value::Number(v));
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
        let v = match (vt, &json) {
            (ValueType::Number, J::Number(n)) => n.as_f64().map(Value::Number),
            (ValueType::Enum, J::Number(n)) => n
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .map(Value::Enum),
            (ValueType::Boolean, J::Bool(b)) => Some(Value::Boolean(*b)),
            (ValueType::Text, J::String(s)) => Some(Value::Text(s.clone())),
            // ponytail: datetime cells are not decoded yet (needs chrono);
            // add when a datetime measure feeds a derived one.
            _ => None,
        }
        .ok_or_else(|| bad(&format!("{json} is not a {} value", value_type_name(vt))))?;
        model.set_input(MeasureId(*mid), Coordinate { dims }, v);
    }
    Ok(cells.len())
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
pub type ResultRows = BTreeMap<Vec<String>, f64>;

/// Name the engine's cells for `measure` in its dimension order.
// ponytail: non-numeric cells (text/bool/error) are dropped because v1 result
// columns are double precision; add a typed value column when non-numeric
// derived measures are supported.
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
        let Some(v) = val.as_num() else { continue };
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
        out.insert(names, v);
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
/// measure defined through the GUI gets its table on the first recalc).
pub fn create_table_sql(table: &str, dim_cols: &[String]) -> String {
    let cols: Vec<String> = dim_cols
        .iter()
        .map(|c| format!("{} text NOT NULL", quote_ident(c)))
        .collect();
    format!(
        "CREATE TABLE IF NOT EXISTS {table} ({}, value double precision, PRIMARY KEY ({}))",
        cols.join(", "),
        quoted_list(dim_cols)
    )
}

/// `SELECT` of a result table's current rows (dims…, value).
pub fn select_result_sql(table: &str, dim_cols: &[String]) -> String {
    format!("SELECT {}, value FROM {table}", quoted_list(dim_cols))
}

/// `SELECT` of a binding's fact rows (dims as text…, value as float8).
/// `rel` is a `regclass::text`, which Postgres has already quoted.
pub fn select_bound_sql(rel: &str, dim_cols: &[String], value_col: &str) -> String {
    let dims: Vec<String> = dim_cols
        .iter()
        .map(|c| format!("{}::text", quote_ident(c)))
        .collect();
    format!(
        "SELECT {}, {}::float8 FROM {rel}",
        dims.join(", "),
        quote_ident(value_col)
    )
}

fn quoted_list(cols: &[String]) -> String {
    cols.iter()
        .map(|c| quote_ident(c))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One SQL statement with column-major array parameters: `$1..$n` are the
/// `text[]` dimension columns of [`Statement::dims`], then (upserts only) a
/// `float8[]` of [`Statement::values`].
#[derive(Debug, Clone, PartialEq)]
pub struct Statement {
    /// The SQL text.
    pub sql: String,
    /// One `text[]` parameter per dimension column.
    pub dims: Vec<Vec<String>>,
    /// The `float8[]` value parameter (upserts only).
    pub values: Option<Vec<f64>>,
}

impl Statement {
    /// Result rows this statement changes.
    pub fn rows(&self) -> usize {
        self.dims.first().map_or(0, Vec::len)
    }
}

/// The statements that turn `old` (the table's rows) into `new` (the engine's
/// rows): at most one batched upsert of new/changed rows and one batched
/// delete of vanished rows. Unchanged rows (bit-identical value) are skipped.
pub fn diff_statements(
    table: &str,
    dim_cols: &[String],
    old: &ResultRows,
    new: &ResultRows,
) -> Vec<Statement> {
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

    let changed: Vec<(&Vec<String>, f64)> = new
        .iter()
        .filter(|(k, v)| old.get(*k).map(|o| o.to_bits()) != Some(v.to_bits()))
        .map(|(k, v)| (k, *v))
        .collect();
    if !changed.is_empty() {
        out.push(Statement {
            sql: format!(
                "INSERT INTO {table} ({cols}, value) SELECT * FROM {} \
                 ON CONFLICT ({cols}) DO UPDATE SET value = EXCLUDED.value",
                unnest(&format!(", ${}::float8[]", n + 1))
            ),
            dims: columns(&mut changed.iter().map(|(k, _)| *k)),
            values: Some(changed.iter().map(|(_, v)| *v).collect()),
        });
    }

    let gone: Vec<&Vec<String>> = old.keys().filter(|k| !new.contains_key(*k)).collect();
    if !gone.is_empty() {
        out.push(Statement {
            sql: format!(
                "DELETE FROM {table} WHERE ({cols}) IN (SELECT * FROM {})",
                unnest("")
            ),
            dims: columns(&mut gone.into_iter()),
            values: None,
        });
    }
    out
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
        }
    }

    fn row(t: &str, p: &str, v: f64) -> (Vec<Option<String>>, Option<f64>) {
        (vec![Some(s(t)), Some(s(p))], Some(v))
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
            create_table_sql("t", &cols),
            "CREATE TABLE IF NOT EXISTS t (\"time\" text NOT NULL, \"product\" text NOT NULL, \
             value double precision, PRIMARY KEY (\"time\", \"product\"))"
        );
        assert_eq!(
            select_result_sql("t", &cols),
            "SELECT \"time\", \"product\", value FROM t"
        );
        assert_eq!(
            select_bound_sql("public.sales", &cols, "price"),
            "SELECT \"time\"::text, \"product\"::text, \"price\"::float8 FROM public.sales"
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
        assert!(matches!(err("Two = 2"), Error::Definition(m) if m.contains("dimension")));
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
        assert!(
            matches!(err("Big = Price > 2"), Error::Definition(m) if m.contains("only number"))
        );
        assert!(matches!(err("garbage"), Error::Definition(_)));
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
        let text_bound = BoundRows {
            measure_id: 1,
            rows: vec![],
        };
        assert!(matches!(
            load_inputs(&mut model, &[text_bound]),
            Err(Error::Catalog(m)) if m.contains("number bindings")
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
            rows: vec![(vec![Some(s("Q1"))], Some(1.0))],
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
        assert_eq!(model.inputs[&(MeasureId(7), coord)], Value::Enum(3));

        for bad in [
            cell(9, "1"),
            cell(1, "\"x\""),
            cell(8, "\"2026-01-01T00:00:00Z\""),
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
                (vec![None, Some(s("Widget"))], Some(1.0)),
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
        assert_eq!(rev[&vec![s("Q3"), s("Widget")]], 3000.0);
        assert_eq!(rev[&vec![s("Q4"), s("Gadget")]], 4000.0);
        let total = result_rows(&model, MeasureId(4), &out[&MeasureId(4)]).unwrap();
        // Widget: 10*(100+200+300+400) ; Gadget: 20*(50+100+150+200)
        assert_eq!(
            total,
            BTreeMap::from([(vec![s("Gadget")], 10000.0), (vec![s("Widget")], 10000.0)])
        );

        let cols = dim_columns(&model, MeasureId(3)).unwrap();
        let table = result_table("plan", "Revenue");
        let fresh = diff_statements(&table, &cols, &ResultRows::new(), &rev);
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].rows(), 8);
        assert!(fresh[0].sql.starts_with(
            "INSERT INTO \"improv_plan\".\"revenue\" (\"time\", \"product\", value) \
             SELECT * FROM unnest($1::text[], $2::text[], $3::float8[]) ON CONFLICT"
        ));
        assert!(diff_statements(&table, &cols, &rev, &rev).is_empty());
    }

    #[test]
    fn diff_upserts_changes_and_deletes_vanished() {
        let cols = vec![s("product")];
        let old = ResultRows::from([
            (vec![s("a")], 1.0),
            (vec![s("b")], 2.0),
            (vec![s("c")], 3.0),
        ]);
        let new = ResultRows::from([
            (vec![s("a")], 1.0),
            (vec![s("b")], 2.5),
            (vec![s("d")], 4.0),
        ]);
        let st = diff_statements("t", &cols, &old, &new);
        assert_eq!(st.len(), 2);
        assert_eq!(st[0].dims, vec![vec![s("b"), s("d")]]);
        assert_eq!(st[0].values, Some(vec![2.5, 4.0]));
        assert_eq!(
            st[1].sql,
            "DELETE FROM t WHERE (\"product\") IN (SELECT * FROM unnest($1::text[]))"
        );
        assert_eq!(st[1].dims, vec![vec![s("c")]]);
        assert_eq!(st[1].values, None);
        assert_eq!(st.iter().map(Statement::rows).sum::<usize>(), 3);
        assert_eq!(
            Statement {
                sql: s(""),
                dims: vec![],
                values: None
            }
            .rows(),
            0
        );
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
            (vec![(1, 1), (2, 2)], CellValue::num(7.0)),
            (vec![(1, 1), (2, 2), (3, 3)], CellValue::Text(s("x"))),
        ]);
        let rows = result_rows(&model, MeasureId(1), &cells).unwrap();
        assert_eq!(rows, ResultRows::from([(vec![s("Q1"), s("Widget")], 7.0)]));
        let foreign = HashMap::from([(vec![(1, 1)], CellValue::num(1.0))]);
        assert!(result_rows(&model, MeasureId(1), &foreign).is_err());
    }
}
