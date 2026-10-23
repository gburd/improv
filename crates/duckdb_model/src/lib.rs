//! DuckDB-free core of the `improv` DuckDB extension (`crates/duckdb_improv`).
//!
//! The extension reads rows through DuckDB and hands them here as plain Rust
//! values, the same split as `improv_pg_model` / `pg_improv`:
//!
//! * [`calc`]: `improv_calc` / `improv_calc_text`, a scalar formula over
//!   named JSON inputs (compiled once, cached);
//! * [`function_rows`]: `improv_functions`, the built-in function registry;
//! * [`parse_spec`], [`bind`], the `*_sql` builders and [`cells`]:
//!   `improv(source, model)`, a small model spec over a table or query, the
//!   SQL that reads it, and the engine's cells in result order, which the
//!   extension writes out a chunk at a time ([`run`] collects them as rows).
//!
//! The model spec has one item per line (blank lines and `#` / `--`
//! comments are skipped):
//!
//! ```text
//! category <Name> = <column>
//! input    <Name> = <column> [sum|avg|min|max|count]
//! <Name> = <formula>
//! ```
//!
//! Every input ranges over every category. A category's items are its
//! column's distinct non-NULL values in the column's own order (numbers
//! numerically, dates chronologically), so `PREV` and `NEXT` follow it.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod calc;

use std::collections::HashMap;

use improv_core_model::functions::{self, Category, FnDef, Ret};
use improv_core_model::MeasureId;
/// Re-exported so the extension shell needs no direct engine dependency.
pub use improv_core_model::ValueType;
/// Re-exported so the extension shell needs no direct engine dependency.
pub use improv_engine::CellValue;
use improv_engine::CoordKey;
use improv_pg_model as pm;

/// Everything that can go wrong building or running a model.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Error {
    /// A problem with one line of the model spec.
    #[error("improv: model line {line}: {message}")]
    Line {
        /// 1-based line number in the spec.
        line: usize,
        /// What is wrong with it.
        message: String,
    },
    /// A problem with the model as a whole, or the engine's.
    #[error("improv: {0}")]
    Model(String),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

fn at(line: usize, message: impl ToString) -> Error {
    Error::Line {
        line,
        message: message.to_string(),
    }
}

/// A `category <Name> = <column>` line.
#[derive(Debug, Clone, PartialEq)]
pub struct CategorySpec {
    /// The category's name (also its result column).
    pub name: String,
    /// The source column its items come from.
    pub column: String,
    /// Its line in the spec.
    pub line: usize,
}

/// An `input <Name> = <column> [agg]` line.
#[derive(Debug, Clone, PartialEq)]
pub struct InputSpec {
    /// The input measure's name.
    pub name: String,
    /// The source column its values come from.
    pub column: String,
    /// `sum | avg | min | max | count`: rows on one coordinate are combined
    /// with it (without one, two such rows are an error).
    pub agg: Option<String>,
    /// Its line in the spec.
    pub line: usize,
}

/// A `<Name> = <formula>` line.
#[derive(Debug, Clone, PartialEq)]
pub struct DerivedSpec {
    /// The whole definition, `Name = formula`.
    pub text: String,
    /// Its line in the spec.
    pub line: usize,
}

/// A parsed model spec.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Spec {
    /// Categories in declaration order (the result's column order).
    pub categories: Vec<CategorySpec>,
    /// Input measures in declaration order.
    pub inputs: Vec<InputSpec>,
    /// Derived measures in declaration order (each may use the inputs, the
    /// derived measures before it, and itself through `PREV`).
    pub derived: Vec<DerivedSpec>,
}

const USAGE: &str =
    "expected `category Name = column`, `input Name = column [sum|avg|min|max|count]` or `Name = formula`";

/// Result columns besides the categories; a category may not take one.
const RESERVED: [&str; 3] = ["measure", "value", "value_text"];

/// Parse a model spec. Errors name the offending line.
pub fn parse_spec(text: &str) -> Result<Spec> {
    let mut spec = Spec::default();
    let mut seen: HashMap<String, usize> = HashMap::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') || l.starts_with("--") {
            continue;
        }
        let (word, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
        let rest = rest.trim_start();
        let keyword = word.to_ascii_lowercase();
        let declares = matches!(keyword.as_str(), "category" | "input") && !rest.starts_with('=');
        let (name, rhs) = if declares { rest } else { l }
            .split_once('=')
            .map(|(n, r)| (n.trim(), r.trim()))
            .ok_or_else(|| at(line, USAGE))?;
        let what = match (declares, keyword.as_str()) {
            (true, "category") => "category",
            _ => "measure",
        };
        pm::validate_name(what, name).map_err(|e| at(line, e))?;
        if what == "category" && RESERVED.iter().any(|r| name.eq_ignore_ascii_case(r)) {
            return Err(at(
                line,
                format!("{name} is a result column; name the category something else"),
            ));
        }
        if let Some(first) = seen.insert(name.to_ascii_lowercase(), line) {
            return Err(at(
                line,
                format!("{name} is already declared on line {first}"),
            ));
        }
        if !declares {
            spec.derived.push(DerivedSpec {
                text: l.to_string(),
                line,
            });
            continue;
        }
        let (column, tail) = parse_column(rhs).map_err(|m| at(line, m))?;
        let name = name.to_string();
        if what == "category" {
            if !tail.is_empty() {
                return Err(at(line, format!("unexpected {tail:?} after the column")));
            }
            spec.categories.push(CategorySpec { name, column, line });
            continue;
        }
        let agg = match tail.split_whitespace().collect::<Vec<_>>()[..] {
            [] => None,
            [a] => Some(pm::agg_sql(&a.to_ascii_lowercase()).map_err(|e| at(line, e))?),
            _ => return Err(at(line, format!("unexpected {tail:?} after the column"))),
        };
        spec.inputs.push(InputSpec {
            name,
            column,
            agg: agg.map(str::to_string),
            line,
        });
    }
    if spec.categories.is_empty() {
        return Err(Error::Model(
            "the model declares no category (add a line like `category Time = year`)".into(),
        ));
    }
    if spec.derived.is_empty() {
        return Err(Error::Model(
            "the model defines no derived measure (add a line like `Revenue = Price * Quantity`)"
                .into(),
        ));
    }
    Ok(spec)
}

/// A column name (bare, or `"double quoted"` with `""` for a quote) and the
/// rest of the line.
fn parse_column(s: &str) -> std::result::Result<(String, &str), String> {
    if let Some(body) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = body.char_indices();
        while let Some((i, c)) = chars.next() {
            if c != '"' {
                out.push(c);
            } else if body[i + 1..].starts_with('"') {
                out.push('"');
                chars.next();
            } else if out.is_empty() {
                break;
            } else {
                return Ok((out, body[i + 1..].trim()));
            }
        }
        return Err("unterminated or empty quoted column name".into());
    }
    match s.split_once(char::is_whitespace) {
        _ if s.is_empty() => Err("expected a column name after `=`".into()),
        Some((c, rest)) => Ok((c.to_string(), rest.trim())),
        None => Ok((s.to_string(), "")),
    }
}

/// A source column as `DESCRIBE` reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Its name.
    pub name: String,
    /// Its DuckDB type (`INTEGER`, `DECIMAL(10,2)`, `VARCHAR`, …).
    pub ty: String,
}

/// The spec bound to the source's columns.
#[derive(Debug, Clone, PartialEq)]
pub struct Bound {
    /// The source column of each category, in declaration order.
    pub dims: Vec<String>,
    /// The source column and value type of each input, in declaration order.
    pub inputs: Vec<(String, ValueType)>,
}

/// The PostgreSQL spelling [`pm::bound_value_type`] understands for a DuckDB
/// column type; others pass through and are refused there.
fn pg_type(ty: &str) -> &str {
    let base = ty.split('(').next().unwrap_or(ty).trim();
    match base {
        "TINYINT" | "SMALLINT" | "INTEGER" | "BIGINT" | "HUGEINT" | "UTINYINT" | "USMALLINT"
        | "UINTEGER" | "UBIGINT" | "UHUGEINT" | "FLOAT" | "DOUBLE" | "DECIMAL" => "float8",
        "BOOLEAN" => "bool",
        "VARCHAR" => "text",
        _ => ty,
    }
}

/// Resolve the spec's columns against the source (DuckDB names are
/// case-insensitive) and type its inputs: numbers, booleans and text; `sum`
/// and `avg` need a number, `count` is a number.
pub fn bind(spec: &Spec, columns: &[Column]) -> Result<Bound> {
    let find = |line: usize, want: &str| {
        columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(want))
            .ok_or_else(|| {
                let have: Vec<&str> = columns.iter().map(|c| c.name.as_str()).collect();
                at(
                    line,
                    format!(
                        "the source has no column {want:?} (it has: {})",
                        have.join(", ")
                    ),
                )
            })
    };
    let dims = spec
        .categories
        .iter()
        .map(|c| find(c.line, &c.column).map(|col| col.name.clone()))
        .collect::<Result<_>>()?;
    let inputs = spec
        .inputs
        .iter()
        .map(|i| {
            let col = find(i.line, &i.column)?;
            let vt = pm::bound_value_type(pg_type(&col.ty), &col.ty, i.agg.as_deref())
                .map_err(|e| at(i.line, format!("{}: {e}", i.name)))?;
            Ok((col.name.clone(), vt))
        })
        .collect::<Result<_>>()?;
    Ok(Bound { dims, inputs })
}

/// The source in a `FROM`: a query (`SELECT`, `WITH`, `FROM`, `VALUES`, or
/// anything parenthesized) as a subquery, else a table or view name as
/// written. A trailing `;` is dropped.
pub fn source_sql(source: &str) -> String {
    let s = source.trim().trim_end_matches(';').trim_end();
    let first = s
        .split(|c: char| !c.is_ascii_alphabetic())
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if s.starts_with('(') || matches!(first.as_str(), "select" | "with" | "from" | "values") {
        format!("({s}) AS improv_source")
    } else {
        s.to_string()
    }
}

/// The SQL listing the source's columns and types.
pub fn describe_sql(source: &str) -> String {
    format!("DESCRIBE SELECT * FROM {}", source_sql(source))
}

/// The SQL listing a category's items: its column's distinct non-NULL values
/// as text, in the column's own order.
pub fn items_sql(source: &str, column: &str) -> String {
    let c = pm::quote_ident(column);
    format!(
        "SELECT CAST({c} AS VARCHAR) FROM {} WHERE {c} IS NOT NULL GROUP BY {c} ORDER BY {c}",
        source_sql(source)
    )
}

/// The SQL reading input `i`'s rows: every category column as text, then the
/// value as a `DOUBLE`, `BOOLEAN` or `VARCHAR`, aggregated per coordinate
/// when the input has an aggregation.
pub fn input_sql(source: &str, spec: &Spec, bound: &Bound, i: usize) -> Result<String> {
    let (col, vt) = &bound.inputs[i];
    pm::select_bound_sql(
        &source_sql(source),
        &bound.dims,
        col,
        spec.inputs[i].agg.as_deref(),
        *vt,
    )
    .map_err(|e| at(spec.inputs[i].line, e))
}

/// One fact row: the category values (in declaration order) and the value.
pub type FactRow = (Vec<Option<String>>, Option<CellValue>);

/// One result row of `improv(source, model)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    /// The derived measure.
    pub measure: String,
    /// One item per category (declaration order); `None` where the measure
    /// does not range over that category.
    pub dims: Vec<Option<String>>,
    /// The value when it is a number.
    pub value: Option<f64>,
    /// The value as text: numbers to 15 significant digits, errors as their
    /// code (`#DIV/0!`).
    pub text: String,
}

/// [`cells`], collected as one [`Row`] per derived cell.
pub fn run(
    spec: &Spec,
    bound: &Bound,
    items: &[Vec<String>],
    facts: Vec<Vec<FactRow>>,
) -> Result<Vec<Row>> {
    let c = cells(spec, bound, items, facts)?;
    Ok(c.range(0..c.len())
        .map(|r| Row {
            measure: r.measure.to_string(),
            dims: (0..spec.categories.len())
                .map(|i| r.dim(i).map(str::to_string))
                .collect(),
            value: calc::number(r.value),
            text: r.value.to_string(),
        })
        .collect())
}

/// The derived cells of a model, in result order: measures in declaration
/// order, each one's cells in item order. Rows are read a range at a time
/// ([`Cells::range`]), so a caller writes them out without holding a copy
/// of every row.
#[derive(Debug)]
pub struct Cells {
    /// Per derived measure: its name and its cells, sorted.
    measures: Vec<(String, Vec<(CoordKey, CellValue)>)>,
    /// Row number of each measure's first cell.
    starts: Vec<usize>,
    /// Item names by item id - 1.
    names: Vec<String>,
}

/// One derived cell of [`Cells`].
#[derive(Debug, Clone, Copy)]
pub struct CellRow<'a> {
    /// The derived measure.
    pub measure: &'a str,
    /// The value (its text is its `Display`).
    pub value: &'a CellValue,
    key: &'a CoordKey,
    names: &'a [String],
}

impl CellRow<'_> {
    /// The item of the `i`-th category (declaration order); `None` where the
    /// measure does not range over it.
    pub fn dim(&self, i: usize) -> Option<&str> {
        item_of(self.key, i as u32 + 1).map(|id| self.names[id as usize - 1].as_str())
    }
}

impl Cells {
    /// The number of rows.
    pub fn len(&self) -> usize {
        self.starts.last().copied().unwrap_or(0)
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Rows `r` (clamped to [`Cells::len`]).
    pub fn range(&self, r: std::ops::Range<usize>) -> impl Iterator<Item = CellRow<'_>> + '_ {
        let end = r.end.min(self.len());
        let first = self.starts.partition_point(|s| *s <= r.start).max(1) - 1;
        self.measures[first.min(self.measures.len())..]
            .iter()
            .zip(&self.starts[first.min(self.measures.len())..])
            .flat_map(move |((name, cells), at)| {
                let lo = r.start.saturating_sub(*at).min(cells.len());
                let hi = end.saturating_sub(*at).min(cells.len());
                cells[lo..hi].iter().map(move |(key, value)| CellRow {
                    measure: name,
                    value,
                    key,
                    names: &self.names,
                })
            })
    }
}

/// Build the model from the spec, its categories' items (from
/// [`items_sql`], per category) and its inputs' rows (from [`input_sql`],
/// per input), and evaluate every derived measure.
pub fn cells(
    spec: &Spec,
    bound: &Bound,
    items: &[Vec<String>],
    facts: Vec<Vec<FactRow>>,
) -> Result<Cells> {
    let mut catalog = pm::Catalog::default();
    let mut next_item = 0;
    for (c, (cat, names)) in (1..).zip(spec.categories.iter().zip(items)) {
        catalog.categories.push(pm::CategoryRow {
            id: c,
            name: cat.name.clone(),
        });
        for name in names {
            next_item += 1;
            catalog.items.push(pm::ItemRow {
                id: next_item,
                category_id: c,
                name: name.clone(),
            });
        }
    }
    let cats: Vec<u32> = (1..=spec.categories.len() as u32).collect();
    for (m, (input, (_, vt))) in (1..).zip(spec.inputs.iter().zip(&bound.inputs)) {
        let mut row = pm::MeasureRow::input(m, &input.name, cats.clone());
        row.value_type = pm::value_type_name(*vt).into();
        catalog.measures.push(row);
    }
    let model_err = |e: pm::Error| Error::Model(e.to_string());
    let mut model = pm::build_model(&catalog).map_err(model_err)?;
    let mut targets = Vec::with_capacity(spec.derived.len());
    for (m, d) in (spec.inputs.len() as u32 + 1..).zip(&spec.derived) {
        let row = pm::define(&model, &d.text, m).map_err(|e| at(d.line, e))?;
        if row.categories.is_empty() {
            return Err(at(
                d.line,
                format!(
                    "{} has no dimensions; dimensionless results are not supported",
                    row.name
                ),
            ));
        }
        catalog.measures.push(row);
        model = pm::build_model(&catalog).map_err(|e| at(d.line, e))?;
        targets.push(MeasureId(m));
    }
    let bound_rows: Vec<pm::BoundRows> = (1..)
        .zip(facts)
        .map(|(measure_id, rows)| pm::BoundRows { measure_id, rows })
        .collect();
    pm::load_inputs(&mut model, &bound_rows).map_err(|e| match e {
        pm::Error::DuplicateCoordinate { measure, coord } => {
            let line = spec
                .inputs
                .iter()
                .find(|i| i.name == measure)
                .map_or(0, |i| i.line);
            at(
                line,
                format!(
                    "{measure}: two source rows have the coordinate ({}); add an aggregation \
                     (sum, avg, min, max, count or count_distinct) to the input line",
                    coord.join(", ")
                ),
            )
        }
        other => model_err(other),
    })?;
    drop(bound_rows);
    let mut out = improv_engine::dataflow::evaluate(&model, &targets)
        .map_err(|e| Error::Model(e.to_string()))?;
    let names: Vec<String> = targets
        .iter()
        .map(|t| model.measures[t].name.0.clone())
        .collect();
    drop(model);
    let mut measures = Vec::with_capacity(targets.len());
    let mut starts = vec![0];
    for (t, name) in targets.into_iter().zip(names) {
        let mut cells: Vec<(CoordKey, CellValue)> =
            out.remove(&t).unwrap_or_default().into_iter().collect();
        // Items are numbered in each category's order, so ids sort by it.
        // (A key per comparison, not a Vec: 2.5M cells sort in a fraction of
        // the time.)
        cells.sort_unstable_by(|(a, _), (b, _)| {
            let ids = |k| cats.iter().map(move |c| item_of(k, *c));
            ids(a).cmp(ids(b))
        });
        starts.push(starts[starts.len() - 1] + cells.len());
        measures.push((name, cells));
    }
    Ok(Cells {
        measures,
        starts,
        names: catalog.items.into_iter().map(|i| i.name).collect(),
    })
}

fn item_of(key: &CoordKey, cat: u32) -> Option<u32> {
    key.iter().find(|(c, _)| *c == cat).map(|(_, i)| *i)
}

/// One row of `improv_functions()`.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionRow {
    /// Canonical (Excel) name.
    pub name: &'static str,
    /// Reference-page category: `math`, `financial`, `text`, ….
    pub category: &'static str,
    /// `PMT(rate, nper, pv, [fv=0], [type=0])`.
    pub signature: String,
    /// `number | boolean | text | date | any`.
    pub returns: &'static str,
    /// One-line description.
    pub description: &'static str,
    /// Other spellings, comma-separated; `None` when there are none.
    pub aliases: Option<String>,
}

fn category_name(c: Category) -> &'static str {
    match c {
        Category::Aggregation => "aggregation",
        Category::Logical => "logical",
        Category::Information => "information",
        Category::Math => "math",
        Category::Rounding => "rounding",
        Category::Text => "text",
        Category::DateTime => "datetime",
        Category::Statistical => "statistical",
        Category::Financial => "financial",
        Category::Dimensional => "dimensional",
        Category::Engineering => "engineering",
    }
}

fn returns(r: Ret) -> &'static str {
    match r {
        Ret::Fixed(ValueType::Number) => "number",
        Ret::Fixed(ValueType::Boolean) => "boolean",
        Ret::Fixed(ValueType::Text) => "text",
        Ret::Fixed(ValueType::DateTime) => "date",
        Ret::Fixed(ValueType::Enum) | Ret::Unify | Ret::Arg(_) => "any",
    }
}

fn function_row(d: &FnDef) -> FunctionRow {
    FunctionRow {
        name: d.name,
        category: category_name(d.category),
        signature: d.hint(),
        returns: returns(d.ret),
        description: d.doc,
        aliases: (!d.aliases.is_empty()).then(|| d.aliases.join(", ")),
    }
}

/// The built-in functions in registry order; with `query`, those
/// [`functions::search`] finds, best match first.
pub fn function_rows(query: Option<&str>) -> Vec<FunctionRow> {
    match query {
        None => functions::all().map(function_row).collect(),
        Some(q) => functions::search(q).into_iter().map(function_row).collect(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
