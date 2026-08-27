//! SQL connectivity (Phase 7): import external SQL query results into an Improv
//! `Model`, and export a measure's cells back to a SQL table.
//!
//! Design invariants (see `.agent/steering/AGENT_DATABASE_CONNECTIVITY.md` §7):
//!
//! * **Storage separation.** External SQL is a data *source/sink*; the canonical
//!   model lives in Mentat. Import produces ordinary categories/items/measures/
//!   input cells — the engine gains no SQL-specific paths and stays deterministic.
//! * **Security.** Queries are run as-is against a caller-provided connection;
//!   values are read through rusqlite's typed API (no string interpolation of
//!   data). Credential/connection management is the caller's concern; this crate
//!   takes an open `rusqlite::Connection`.
//!
//! v1 targets **SQLite** (the reference driver, and what the embedded Mentat
//! store already uses). Other backends (Postgres, DuckDB, …) slot behind the
//! same column→model mapping later.

use improv_core_model::{
    CategoryId, Coordinate, ItemId, Measure, MeasureId, MeasureKind, Model, Name, Value, ValueType,
};
use rusqlite::Connection;
use std::collections::HashMap;

#[derive(Debug, thiserror::Error)]
pub enum SqlError {
    #[error("sql error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("import: {0}")]
    Import(String),
    #[error("export: {0}")]
    Export(String),
}

pub type Result<T> = std::result::Result<T, SqlError>;

/// How to map a query's result columns onto model elements.
///
/// Each `dimensions` entry names a result column that becomes a **category**
/// (its distinct values become items); `value_column` names the column that
/// becomes the imported measure's numeric value. A row therefore contributes
/// one input cell: `measure[dim=val, …] = value_column`.
pub struct ImportSpec {
    /// SQL `SELECT` to run against the connection.
    pub query: String,
    /// Result-column name → the category it maps to (name + id).
    pub dimensions: Vec<DimensionMapping>,
    /// The result column holding the numeric measure value.
    pub value_column: String,
    /// The measure to create/populate for the value column.
    pub measure_id: MeasureId,
    pub measure_name: String,
    /// Base id for minting item ids; items get sequential ids from here.
    pub item_id_base: u32,
}

pub struct DimensionMapping {
    pub column: String,
    pub category_id: CategoryId,
    pub category_name: String,
}

/// Import the result of `spec.query` into `model` as a new input measure over
/// the mapped dimension categories. Distinct dimension-column values become
/// items (interned by their string form). Returns the number of cells imported.
///
/// SQL data enters purely as input cells (a normal measure collection), so the
/// deterministic engine core is untouched.
pub fn import_query(conn: &Connection, model: &mut Model, spec: &ImportSpec) -> Result<usize> {
    // Ensure the categories exist.
    for d in &spec.dimensions {
        model
            .categories
            .entry(d.category_id)
            .or_insert_with(|| improv_core_model::Category {
                id: d.category_id,
                name: Name(d.category_name.clone()),
                items: Vec::new(),
            });
    }
    // Create the measure (input, over the mapped categories).
    let cats: Vec<CategoryId> = spec.dimensions.iter().map(|d| d.category_id).collect();
    model.add_measure(Measure {
        id: spec.measure_id,
        name: Name(spec.measure_name.clone()),
        value_type: ValueType::Number,
        categories: cats,
        kind: MeasureKind::Input,
        description: Some(format!("imported from SQL: {}", truncate(&spec.query, 80))),
    });

    // Per-category interner: dimension value string -> ItemId. Seed it with the
    // model's EXISTING items (by category + name) so re-import / refresh reuses
    // items instead of minting duplicates with the same name.
    let mut interners: HashMap<CategoryId, HashMap<String, ItemId>> = HashMap::new();
    for it in model.items.values() {
        interners
            .entry(it.category)
            .or_default()
            .insert(it.name.0.clone(), it.id);
    }
    let mut next_item = spec.item_id_base;

    let mut stmt = conn.prepare(&spec.query)?;
    let col_names: Vec<String> = stmt.column_names().iter().map(|s| s.to_string()).collect();

    // Resolve column indices up front; error clearly if a mapped column is absent.
    let idx = |name: &str| -> Result<usize> {
        col_names
            .iter()
            .position(|c| c == name)
            .ok_or_else(|| SqlError::Import(format!("no column '{name}' in query result")))
    };
    let dim_idx: Vec<(usize, CategoryId)> = spec
        .dimensions
        .iter()
        .map(|d| Ok((idx(&d.column)?, d.category_id)))
        .collect::<Result<_>>()?;
    let val_idx = idx(&spec.value_column)?;

    let mut rows = stmt.query([])?;
    let mut count = 0usize;
    // Collect (coord, value) first so we can mutate the model after the borrow.
    let mut cells: Vec<(Coordinate, f64)> = Vec::new();
    while let Some(row) = rows.next()? {
        let mut coord = Coordinate::new();
        for (ci, cat) in &dim_idx {
            let key = cell_text(row, *ci)?;
            let item = *interners
                .entry(*cat)
                .or_default()
                .entry(key.clone())
                .or_insert_with(|| {
                    let id = ItemId(next_item);
                    next_item += 1;
                    id
                });
            coord = coord.with(*cat, item);
        }
        let value: f64 = row.get(val_idx).map_err(|e| {
            SqlError::Import(format!(
                "value column '{}' is not numeric: {e}",
                spec.value_column
            ))
        })?;
        cells.push((coord, value));
        count += 1;
    }
    drop(rows);
    drop(stmt);

    // Register items on their categories and set the input cells.
    for (cat, items) in &interners {
        for (name, id) in items {
            model
                .items
                .entry(*id)
                .or_insert_with(|| improv_core_model::Item {
                    id: *id,
                    category: *cat,
                    name: Name(name.clone()),
                });
            if let Some(c) = model.categories.get_mut(cat) {
                if !c.items.contains(id) {
                    c.items.push(*id);
                }
            }
        }
    }
    for (coord, value) in cells {
        model.set_input(spec.measure_id, coord, Value::Number(value));
    }

    Ok(count)
}

/// Register an SQL-backed input measure (records its `SqlSource` on the model)
/// and populate it by running the query once. The measure's categories must
/// already exist or be creatable from the import; this is a convenience over
/// `import_query` that also marks the measure as SQL-backed so it can be
/// refreshed later.
pub fn add_sql_measure(conn: &Connection, model: &mut Model, spec: &ImportSpec) -> Result<usize> {
    let source = improv_core_model::SqlSource {
        query: spec.query.clone(),
        dimension_columns: spec.dimensions.iter().map(|d| d.column.clone()).collect(),
        value_column: spec.value_column.clone(),
    };
    let n = import_query(conn, model, spec)?;
    model.sql_sources.insert(spec.measure_id, source);
    Ok(n)
}

/// Refresh an SQL-backed measure: re-run its stored query and replace its input
/// cells with the fresh result. New dimension values become new items. The
/// measure must have a `SqlSource` (added via `add_sql_measure`).
///
/// This is the `SQL("...")` live-query refresh (Phase 7): SQL data re-enters as
/// ordinary input cells, so the engine recomputes dependents with no SQL path
/// of its own. It is nondeterministic by nature (external data), hence gated on
/// an explicit refresh rather than sitting on a pure evaluation path.
pub fn refresh_sql_measure(
    conn: &Connection,
    model: &mut Model,
    measure: MeasureId,
) -> Result<usize> {
    let source = model
        .sql_sources
        .get(&measure)
        .cloned()
        .ok_or_else(|| SqlError::Import(format!("{measure:?} is not an SQL-backed measure")))?;
    let m = model
        .measures
        .get(&measure)
        .ok_or_else(|| SqlError::Import(format!("no measure {measure:?}")))?;
    if source.dimension_columns.len() != m.categories.len() {
        return Err(SqlError::Import(format!(
            "SQL source for {measure:?} maps {} columns but the measure has {} categories",
            source.dimension_columns.len(),
            m.categories.len()
        )));
    }
    // Rebuild the import spec from the stored source + the measure's categories.
    let dimensions: Vec<DimensionMapping> = source
        .dimension_columns
        .iter()
        .zip(m.categories.iter())
        .map(|(col, cat)| DimensionMapping {
            column: col.clone(),
            category_id: *cat,
            category_name: model
                .categories
                .get(cat)
                .map(|c| c.name.0.clone())
                .unwrap_or_else(|| cat.0.to_string()),
        })
        .collect();
    let spec = ImportSpec {
        query: source.query.clone(),
        dimensions,
        value_column: source.value_column.clone(),
        measure_id: measure,
        measure_name: m.name.0.clone(),
        // Continue item ids above the current max to avoid collisions.
        item_id_base: model
            .items
            .keys()
            .map(|i| i.0)
            .max()
            .map(|m| m + 1)
            .unwrap_or(1_000_000),
    };
    // Clear this measure's existing cells (refresh replaces them). Items from
    // prior refreshes are left in place (harmless; may be re-referenced).
    model.inputs.retain(|(mid, _), _| *mid != measure);
    // import_query re-adds the measure; remove first so it isn't duplicated.
    model.measures.remove(&measure);
    let n = import_query(conn, model, &spec)?;
    Ok(n)
}

/// Write a measure's input cells to a SQL table: one column per dimension
/// category (item name), plus a value column. Creates the table if missing and
/// inserts one row per cell. Values are bound as parameters (no interpolation).
///
/// `table` and column names are validated as plain identifiers to avoid
/// injection through DDL (which cannot be parameterized).
pub fn export_measure(
    conn: &Connection,
    model: &Model,
    measure: MeasureId,
    table: &str,
    value_column: &str,
) -> Result<usize> {
    ident(table)?;
    ident(value_column)?;
    let m = model
        .measures
        .get(&measure)
        .ok_or_else(|| SqlError::Export(format!("no measure {measure:?}")))?;

    // Column per category, in the measure's declared order.
    let dim_cols: Vec<(CategoryId, String)> = m
        .categories
        .iter()
        .map(|c| {
            let name = model
                .categories
                .get(c)
                .map(|x| sanitize_ident(&x.name.0))
                .unwrap_or_else(|| format!("cat{}", c.0));
            (*c, name)
        })
        .collect();
    for (_, name) in &dim_cols {
        ident(name)?;
    }

    // CREATE TABLE (idempotent). Identifiers are validated; values are bound.
    let cols_ddl = dim_cols
        .iter()
        .map(|(_, n)| format!("{n} TEXT"))
        .chain(std::iter::once(format!("{value_column} REAL")))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(
        &format!("CREATE TABLE IF NOT EXISTS {table} ({cols_ddl})"),
        [],
    )?;

    let placeholders = std::iter::repeat_n("?", dim_cols.len() + 1)
        .collect::<Vec<_>>()
        .join(", ");
    let col_list = dim_cols
        .iter()
        .map(|(_, n)| n.clone())
        .chain(std::iter::once(value_column.to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    let insert = format!("INSERT INTO {table} ({col_list}) VALUES ({placeholders})");
    let mut stmt = conn.prepare(&insert)?;

    let mut count = 0usize;
    for ((mid, coord), val) in &model.inputs {
        if *mid != measure {
            continue;
        }
        let n = match val {
            Value::Number(n) => *n,
            _ => continue, // v1 exports numeric cells only
        };
        // Bind dimension item names then the value.
        let mut params: Vec<rusqlite::types::Value> = Vec::with_capacity(dim_cols.len() + 1);
        for (cat, _) in &dim_cols {
            let item_name = coord
                .get(*cat)
                .and_then(|i| model.items.get(&i))
                .map(|it| it.name.0.clone())
                .unwrap_or_default();
            params.push(rusqlite::types::Value::Text(item_name));
        }
        params.push(rusqlite::types::Value::Real(n));
        stmt.execute(rusqlite::params_from_iter(params))?;
        count += 1;
    }
    Ok(count)
}

// --- helpers ---

fn cell_text(row: &rusqlite::Row, idx: usize) -> Result<String> {
    use rusqlite::types::ValueRef;
    Ok(match row.get_ref(idx)? {
        ValueRef::Null => String::new(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
        ValueRef::Blob(_) => return Err(SqlError::Import("blob dimension value".into())),
    })
}

/// Validate a SQL identifier (table/column) used in DDL/DML that can't be
/// parameterized: ASCII alphanumeric + underscore, not starting with a digit.
fn ident(s: &str) -> Result<()> {
    let ok = !s.is_empty()
        && s.chars()
            .next()
            .map(|c| c.is_ascii_alphabetic() || c == '_')
            .unwrap_or(false)
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(SqlError::Export(format!("invalid SQL identifier: {s:?}")))
    }
}

/// Turn an arbitrary category/measure name into a safe identifier.
fn sanitize_ident(s: &str) -> String {
    let mut out: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if out
        .chars()
        .next()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(true)
    {
        out.insert(0, '_');
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        s.to_string()
    } else {
        format!("{}…", &s[..n])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE sales (time TEXT, product TEXT, revenue REAL);
             INSERT INTO sales VALUES ('2025','WidgetA', 1000.0);
             INSERT INTO sales VALUES ('2025','WidgetB', 500.0);
             INSERT INTO sales VALUES ('2026','WidgetA', 1200.0);",
        )
        .unwrap();
        conn
    }

    fn revenue_spec() -> ImportSpec {
        ImportSpec {
            query: "SELECT time, product, revenue FROM sales".into(),
            dimensions: vec![
                DimensionMapping {
                    column: "time".into(),
                    category_id: CategoryId(1),
                    category_name: "Time".into(),
                },
                DimensionMapping {
                    column: "product".into(),
                    category_id: CategoryId(2),
                    category_name: "Product".into(),
                },
            ],
            value_column: "revenue".into(),
            measure_id: MeasureId(100),
            measure_name: "Revenue".into(),
            item_id_base: 1000,
        }
    }

    #[test]
    fn import_maps_columns_to_model() {
        let conn = source_db();
        let mut model = Model::new();
        let n = import_query(&conn, &mut model, &revenue_spec()).unwrap();
        assert_eq!(n, 3, "three rows imported");

        // Categories + items created.
        assert_eq!(model.category_by_name("Time").unwrap().items.len(), 2); // 2025, 2026
        assert_eq!(model.category_by_name("Product").unwrap().items.len(), 2);

        // Measure created as input over both categories.
        let m = model.measure_by_name("Revenue").unwrap();
        assert!(m.is_input());
        assert_eq!(m.categories.len(), 2);
        assert_eq!(model.inputs.len(), 3);

        // A specific cell resolves: Revenue[2025, WidgetA] = 1000.
        let time = model.category_by_name("Time").unwrap().id;
        let product = model.category_by_name("Product").unwrap().id;
        let item = |cat, name: &str| {
            model
                .items
                .values()
                .find(|i| i.category == cat && i.name.0 == name)
                .unwrap()
                .id
        };
        let coord = Coordinate::from_pairs([
            (time, item(time, "2025")),
            (product, item(product, "WidgetA")),
        ]);
        assert_eq!(
            model.input(MeasureId(100), &coord),
            Some(&Value::Number(1000.0))
        );
    }

    #[test]
    fn import_then_export_round_trips_values() {
        let conn = source_db();
        let mut model = Model::new();
        import_query(&conn, &mut model, &revenue_spec()).unwrap();

        let out = Connection::open_in_memory().unwrap();
        let n = export_measure(&out, &model, MeasureId(100), "revenue_out", "revenue").unwrap();
        assert_eq!(n, 3);

        // Read the exported values back; the multiset of revenues matches.
        let mut stmt = out
            .prepare("SELECT revenue FROM revenue_out ORDER BY revenue")
            .unwrap();
        let vals: Vec<f64> = stmt
            .query_map([], |r| r.get::<_, f64>(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(vals, vec![500.0, 1000.0, 1200.0]);
    }

    #[test]
    fn missing_column_errors() {
        let conn = source_db();
        let mut model = Model::new();
        let mut spec = revenue_spec();
        spec.value_column = "nope".into();
        assert!(import_query(&conn, &mut model, &spec).is_err());
    }

    #[test]
    fn export_rejects_bad_identifier() {
        let model = {
            let conn = source_db();
            let mut m = Model::new();
            import_query(&conn, &mut m, &revenue_spec()).unwrap();
            m
        };
        let out = Connection::open_in_memory().unwrap();
        // A table name with a quote must be rejected, not interpolated.
        assert!(export_measure(&out, &model, MeasureId(100), "bad\"; DROP", "v").is_err());
    }

    #[test]
    fn sql_measure_refresh_picks_up_new_data() {
        let conn = source_db();
        let mut model = Model::new();
        // Register + populate an SQL-backed measure.
        let n = add_sql_measure(&conn, &mut model, &revenue_spec()).unwrap();
        assert_eq!(n, 3);
        assert!(model.sql_sources.contains_key(&MeasureId(100)));

        // The source data changes (a new row + a changed value).
        conn.execute_batch(
            "UPDATE sales SET revenue = 9999.0 WHERE time='2025' AND product='WidgetA';
             INSERT INTO sales VALUES ('2027','WidgetA', 1.0);",
        )
        .unwrap();

        // Refresh replaces this measure's cells with the fresh query result.
        let n2 = refresh_sql_measure(&conn, &mut model, MeasureId(100)).unwrap();
        assert_eq!(n2, 4, "now four rows");
        assert_eq!(model.inputs.len(), 4);

        // The changed cell reflects the new value.
        let time = model.category_by_name("Time").unwrap().id;
        let product = model.category_by_name("Product").unwrap().id;
        let item = |cat, name: &str| {
            model
                .items
                .values()
                .find(|i| i.category == cat && i.name.0 == name)
                .unwrap()
                .id
        };
        let coord = Coordinate::from_pairs([
            (time, item(time, "2025")),
            (product, item(product, "WidgetA")),
        ]);
        assert_eq!(
            model.input(MeasureId(100), &coord),
            Some(&Value::Number(9999.0))
        );
    }

    #[test]
    fn refresh_requires_a_registered_source() {
        let conn = source_db();
        let mut model = Model::new();
        // A plain import (no source registered) can't be refreshed.
        import_query(&conn, &mut model, &revenue_spec()).unwrap();
        assert!(refresh_sql_measure(&conn, &mut model, MeasureId(100)).is_err());
    }
}
