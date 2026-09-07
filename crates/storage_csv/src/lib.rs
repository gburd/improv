//! CSV/TSV connectivity (Phase A, post-v0.5.0 plan): import a CSV/TSV file into
//! an Improv `Model`, and export a measure's cells back to CSV/TSV.
//!
//! Mirrors `improv_storage_sql`'s shape (see its module docs): a column→model
//! mapping (dimension columns → categories/items, one value column → a
//! measure) turns external tabular data into ordinary input cells. CSV has no
//! live "refresh" concept (no persistent connection to re-query) — import is a
//! one-shot static load; re-running it against a changed file re-populates the
//! measure the same way a fresh `import_csv` call would.
//!
//! * **Storage separation.** CSV is a data *source/sink* only; the canonical
//!   model lives in Mentat. Import produces ordinary categories/items/measures/
//!   input cells — the engine gains no CSV-specific path.
//! * **Testability.** Every file-based entry point (`import_csv`,
//!   `export_measure_csv`) has an `io::Read`/`io::Write` sibling
//!   (`import_csv_from`, `export_measure_csv_to`) so tests exercise the real
//!   logic against an in-memory buffer, no tempfiles required.

use improv_core_model::{CategoryId, Coordinate, MeasureId, Model, Value, ValueType};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum CsvError {
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("csv error: {0}")]
    Csv(#[from] csv::Error),
    #[error("row {row}: {message}")]
    Row { row: usize, message: String },
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, CsvError>;

/// A column reference: either a header name (requires `has_header`) or a
/// 0-based column index. Supporting both lets headerless files work too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColumnRef {
    Name(String),
    Index(usize),
}

/// A CSV/TSV column that becomes a category: its distinct values become items.
pub struct DimensionMapping {
    pub column: ColumnRef,
    pub category_id: CategoryId,
    pub category_name: String,
}

/// How to map a CSV/TSV file's columns onto model elements. Each `dimensions`
/// entry names a column that becomes a **category**; `value_column` names the
/// column that becomes the imported measure's value. A row therefore
/// contributes one input cell: `measure[dim=val, …] = value_column`.
pub struct ImportSpec {
    /// File to read (ignored by `import_csv_from`, which takes a reader).
    pub path: PathBuf,
    /// Field delimiter: `b','` for CSV, `b'\t'` for TSV.
    pub delimiter: u8,
    /// Whether the file's first row is a header (enables `ColumnRef::Name`).
    pub has_header: bool,
    /// The measure to create/populate for the value column.
    pub measure_id: MeasureId,
    pub measure_name: String,
    /// The declared type of the value column. `Number` parses strictly
    /// (a non-numeric value errors); `Boolean`/`Text` never error on the value
    /// column — a `Boolean` column that fails to match true/false falls back
    /// to `Text` for that cell.
    pub value_type: ValueType,
    pub value_column: ColumnRef,
    pub dimensions: Vec<DimensionMapping>,
    /// Base id for minting item ids; new items get sequential ids from here.
    pub item_id_base: u32,
}

/// Import a CSV/TSV file into `model` per `spec`. Returns the number of cells
/// imported. See [`import_csv_from`] for the reader-based (testable) version.
pub fn import_csv(model: &mut Model, spec: &ImportSpec) -> Result<usize> {
    let file = std::fs::File::open(&spec.path)?;
    import_csv_from(file, model, spec)
}

/// Import CSV/TSV data read from `reader` into `model` per `spec`
/// (`spec.path` is ignored). Distinct dimension-column values become items,
/// interned by name — an item is reused when one already exists with that
/// name in the target category. Returns the number of cells imported.
pub fn import_csv_from<R: io::Read>(
    reader: R,
    model: &mut Model,
    spec: &ImportSpec,
) -> Result<usize> {
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(spec.delimiter)
        .has_headers(spec.has_header)
        .from_reader(reader);

    let headers = if spec.has_header {
        Some(rdr.headers()?.clone())
    } else {
        None
    };

    // Ensure the categories exist.
    improv_data_source::ensure_categories(
        model,
        spec.dimensions
            .iter()
            .map(|d| (d.category_id, d.category_name.as_str())),
    );
    // Create the measure (input, over the mapped categories).
    let cats: Vec<CategoryId> = spec.dimensions.iter().map(|d| d.category_id).collect();
    improv_data_source::add_input_measure(
        model,
        spec.measure_id,
        &spec.measure_name,
        spec.value_type,
        cats,
        Some(format!("imported from CSV: {}", spec.path.display())),
    );

    // Resolve column refs to indices up front; error clearly if a named
    // column doesn't exist (or a name is used with no header row).
    let dim_idx: Vec<(usize, CategoryId)> = spec
        .dimensions
        .iter()
        .map(|d| resolve_col(headers.as_ref(), &d.column).map(|i| (i, d.category_id)))
        .collect::<Result<_>>()?;
    let val_idx = resolve_col(headers.as_ref(), &spec.value_column)?;
    let max_idx = dim_idx
        .iter()
        .map(|(i, _)| *i)
        .chain(std::iter::once(val_idx))
        .max()
        .unwrap_or(0);

    // Per-category interner: item name -> ItemId. Seeded with the model's
    // EXISTING items (by category + name) so re-import reuses items instead of
    // minting duplicates with the same name.
    let mut interner = improv_data_source::ItemInterner::seeded_from(model, spec.item_id_base);

    let mut cells: Vec<(Coordinate, Value)> = Vec::new();
    for result in rdr.records() {
        let record = result?;
        let row_no = record
            .position()
            .map(|p| p.line() as usize)
            .unwrap_or(cells.len() + 1);
        if record.len() <= max_idx {
            return Err(CsvError::Row {
                row: row_no,
                message: format!(
                    "expected at least {} column(s), got {}",
                    max_idx + 1,
                    record.len()
                ),
            });
        }
        let mut coord = Coordinate::new();
        for (ci, cat) in &dim_idx {
            let key = record.get(*ci).unwrap_or("");
            let item = interner.intern(*cat, key);
            coord = coord.with(*cat, item);
        }
        let raw = record.get(val_idx).unwrap_or("");
        let value = if spec.value_type == ValueType::Number {
            let n: f64 = raw.trim().parse().map_err(|_| CsvError::Row {
                row: row_no,
                message: format!("value column is not numeric: {raw:?}"),
            })?;
            Value::Number(n)
        } else {
            parse_flexible(spec.value_type, raw)
        };
        cells.push((coord, value));
    }

    // Register interned items on their categories, then set the input cells.
    interner.register(model);
    let count = cells.len();
    for (coord, value) in cells {
        model.set_input(spec.measure_id, coord, value);
    }
    Ok(count)
}

/// Parse a value column string for a non-`Number` declared type. `Boolean`
/// matches "true"/"false" case-insensitively; anything else (including any
/// string under a `Text`-declared column) passes through as `Text`. Never
/// errors — only a `Number`-declared column enforces a numeric parse.
fn parse_flexible(value_type: ValueType, raw: &str) -> Value {
    match value_type {
        ValueType::Boolean => match raw.trim().to_ascii_lowercase().as_str() {
            "true" => Value::Boolean(true),
            "false" => Value::Boolean(false),
            _ => Value::Text(raw.to_string()),
        },
        _ => Value::Text(raw.to_string()),
    }
}

/// Resolve a `ColumnRef` to a 0-based column index.
fn resolve_col(headers: Option<&csv::StringRecord>, col: &ColumnRef) -> Result<usize> {
    match col {
        ColumnRef::Index(i) => Ok(*i),
        ColumnRef::Name(name) => {
            let h = headers.ok_or_else(|| {
                CsvError::Other(format!(
                    "column '{name}' needs a header row (has_header is false)"
                ))
            })?;
            h.iter()
                .position(|c| c == name)
                .ok_or_else(|| CsvError::Other(format!("no column named '{name}' in header")))
        }
    }
}

/// Write a measure's input cells to a CSV/TSV file: one column per dimension
/// category (item name) plus a value column, header row first. Returns the
/// number of rows written. See [`export_measure_csv_to`] for the writer-based
/// (testable) version.
pub fn export_measure_csv(
    model: &Model,
    measure: MeasureId,
    path: &Path,
    delimiter: u8,
) -> Result<usize> {
    let file = std::fs::File::create(path)?;
    export_measure_csv_to(model, measure, file, delimiter)
}

/// Write a measure's input cells to `writer` as CSV/TSV (`delimiter`).
/// Rows are written in coordinate order, so output is deterministic.
pub fn export_measure_csv_to<W: io::Write>(
    model: &Model,
    measure: MeasureId,
    writer: W,
    delimiter: u8,
) -> Result<usize> {
    let m = model
        .measures
        .get(&measure)
        .ok_or_else(|| CsvError::Other(format!("no measure {measure:?}")))?;

    let dim_cols: Vec<(CategoryId, String)> = m
        .categories
        .iter()
        .map(|c| {
            let name = model
                .categories
                .get(c)
                .map(|x| x.name.0.clone())
                .unwrap_or_else(|| format!("cat{}", c.0));
            (*c, name)
        })
        .collect();

    let mut wtr = csv::WriterBuilder::new()
        .delimiter(delimiter)
        .from_writer(writer);
    let mut header: Vec<String> = dim_cols.iter().map(|(_, n)| n.clone()).collect();
    header.push(m.name.0.clone());
    wtr.write_record(&header)?;

    let mut rows: Vec<(&Coordinate, &Value)> = model
        .inputs
        .iter()
        .filter(|((mid, _), _)| *mid == measure)
        .map(|((_, c), v)| (c, v))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));

    let mut count = 0usize;
    for (coord, val) in rows {
        let value_str = match val {
            Value::Number(n) => n.to_string(),
            Value::Boolean(b) => b.to_string(),
            Value::Text(s) => s.clone(),
            Value::DateTime(dt) => dt.to_rfc3339(),
            Value::Enum(e) => e.to_string(),
            Value::Error(_) => continue, // no meaningful cell text to export
        };
        let mut row: Vec<String> = Vec::with_capacity(dim_cols.len() + 1);
        for (cat, _) in &dim_cols {
            let name = coord
                .get(*cat)
                .and_then(|i| model.items.get(&i))
                .map(|it| it.name.0.clone())
                .unwrap_or_default();
            row.push(name);
        }
        row.push(value_str);
        wtr.write_record(&row)?;
        count += 1;
    }
    wtr.flush()?;
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::ItemId;
    use std::io::Cursor;

    const SALES_CSV: &str = "time,product,revenue\n\
2025,WidgetA,1000\n\
2025,WidgetB,500\n\
2026,WidgetA,1200\n";

    fn revenue_spec() -> ImportSpec {
        ImportSpec {
            path: PathBuf::new(),
            delimiter: b',',
            has_header: true,
            measure_id: MeasureId(100),
            measure_name: "Revenue".into(),
            value_type: ValueType::Number,
            value_column: ColumnRef::Name("revenue".into()),
            dimensions: vec![
                DimensionMapping {
                    column: ColumnRef::Name("time".into()),
                    category_id: CategoryId(1),
                    category_name: "Time".into(),
                },
                DimensionMapping {
                    column: ColumnRef::Name("product".into()),
                    category_id: CategoryId(2),
                    category_name: "Product".into(),
                },
            ],
            item_id_base: 1000,
        }
    }

    fn item_id(model: &Model, cat: CategoryId, name: &str) -> ItemId {
        model
            .items
            .values()
            .find(|i| i.category == cat && i.name.0 == name)
            .unwrap()
            .id
    }

    #[test]
    fn import_maps_columns_to_model() {
        let mut model = Model::new();
        let n = import_csv_from(Cursor::new(SALES_CSV), &mut model, &revenue_spec()).unwrap();
        assert_eq!(n, 3, "three rows imported");

        assert_eq!(model.category_by_name("Time").unwrap().items.len(), 2);
        assert_eq!(model.category_by_name("Product").unwrap().items.len(), 2);

        let m = model.measure_by_name("Revenue").unwrap();
        assert!(m.is_input());
        assert_eq!(m.categories.len(), 2);
        assert_eq!(model.inputs.len(), 3);

        let time = model.category_by_name("Time").unwrap().id;
        let product = model.category_by_name("Product").unwrap().id;
        let coord = Coordinate::from_pairs([
            (time, item_id(&model, time, "2025")),
            (product, item_id(&model, product, "WidgetA")),
        ]);
        assert_eq!(
            model.input(MeasureId(100), &coord),
            Some(&Value::Number(1000.0))
        );
        let coord2 = Coordinate::from_pairs([
            (time, item_id(&model, time, "2025")),
            (product, item_id(&model, product, "WidgetB")),
        ]);
        assert_eq!(
            model.input(MeasureId(100), &coord2),
            Some(&Value::Number(500.0))
        );
    }

    #[test]
    fn import_then_export_round_trips_into_fresh_model() {
        let mut model = Model::new();
        import_csv_from(Cursor::new(SALES_CSV), &mut model, &revenue_spec()).unwrap();

        let mut buf: Vec<u8> = Vec::new();
        let n = export_measure_csv_to(&model, MeasureId(100), &mut buf, b',').unwrap();
        assert_eq!(n, 3);

        // The exported header uses the model's category/measure names
        // ("Time", "Product", "Revenue"), not the original CSV's lowercase
        // column names, so the re-import spec matches those.
        let mut reimport_spec = revenue_spec();
        reimport_spec.value_column = ColumnRef::Name("Revenue".into());
        reimport_spec.dimensions[0].column = ColumnRef::Name("Time".into());
        reimport_spec.dimensions[1].column = ColumnRef::Name("Product".into());

        let mut model2 = Model::new();
        let n2 = import_csv_from(Cursor::new(buf), &mut model2, &reimport_spec).unwrap();
        assert_eq!(n2, 3);

        // Same multiset of values in both models.
        let vals = |m: &Model| {
            let mut v: Vec<f64> = m.inputs.values().filter_map(|x| x.as_number()).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v
        };
        assert_eq!(vals(&model), vals(&model2));
        assert_eq!(model.inputs.len(), model2.inputs.len());
    }

    #[test]
    fn tsv_import_works_identically() {
        let tsv = "time\tproduct\trevenue\n2025\tWidgetA\t1000\n2026\tWidgetA\t1200\n";
        let mut spec = revenue_spec();
        spec.delimiter = b'\t';
        let mut model = Model::new();
        let n = import_csv_from(Cursor::new(tsv), &mut model, &spec).unwrap();
        assert_eq!(n, 2);
        assert_eq!(model.category_by_name("Time").unwrap().items.len(), 2);
    }

    #[test]
    fn malformed_row_errors_with_row_number() {
        let bad = "time,product,revenue\n2025,WidgetA,1000\n2026,OnlyOneField\n";
        let mut model = Model::new();
        let err = import_csv_from(Cursor::new(bad), &mut model, &revenue_spec()).unwrap_err();
        let msg = err.to_string();
        // The csv crate's own record-length check fires first (with line info).
        assert!(msg.contains('3') || msg.contains("line"), "got: {msg}");
    }

    #[test]
    fn short_row_within_declared_width_errors_with_row_number() {
        // 3-column header, but the value column is requested at an index
        // beyond a well-formed-but-narrow row (flexible-length off by design,
        // but exercise the explicit bounds check too via column indices).
        let mut spec = revenue_spec();
        spec.has_header = false;
        spec.dimensions[0].column = ColumnRef::Index(0);
        spec.dimensions[1].column = ColumnRef::Index(1);
        spec.value_column = ColumnRef::Index(5); // out of range for a 3-col row
        let mut model = Model::new();
        let err =
            import_csv_from(Cursor::new("2025,WidgetA,1000\n"), &mut model, &spec).unwrap_err();
        match err {
            CsvError::Row { row, message } => {
                assert_eq!(row, 1);
                assert!(message.contains("column"), "got: {message}");
            }
            other => panic!("expected a Row error, got {other:?}"),
        }
    }

    #[test]
    fn non_numeric_value_errors_for_number_type() {
        let bad = "time,product,revenue\n2025,WidgetA,notanumber\n";
        let mut model = Model::new();
        let err = import_csv_from(Cursor::new(bad), &mut model, &revenue_spec()).unwrap_err();
        match err {
            CsvError::Row { row, message } => {
                assert_eq!(row, 2);
                assert!(message.contains("numeric"), "got: {message}");
            }
            other => panic!("expected a Row error, got {other:?}"),
        }
    }

    #[test]
    fn boolean_and_text_columns_round_trip() {
        let csv_text = "product,active\nWidgetA,true\nWidgetB,FALSE\n";
        let mut spec = revenue_spec();
        spec.value_type = ValueType::Boolean;
        spec.value_column = ColumnRef::Name("active".into());
        spec.dimensions = vec![DimensionMapping {
            column: ColumnRef::Name("product".into()),
            category_id: CategoryId(2),
            category_name: "Product".into(),
        }];
        spec.measure_name = "Active".into();

        let mut model = Model::new();
        let n = import_csv_from(Cursor::new(csv_text), &mut model, &spec).unwrap();
        assert_eq!(n, 2);
        let product = model.category_by_name("Product").unwrap().id;
        let coord_a = Coordinate::from_pairs([(product, item_id(&model, product, "WidgetA"))]);
        let coord_b = Coordinate::from_pairs([(product, item_id(&model, product, "WidgetB"))]);
        assert_eq!(
            model.input(MeasureId(100), &coord_a),
            Some(&Value::Boolean(true))
        );
        assert_eq!(
            model.input(MeasureId(100), &coord_b),
            Some(&Value::Boolean(false))
        );

        // Export + re-import round-trips the booleans as "true"/"false".
        let mut buf = Vec::new();
        export_measure_csv_to(&model, MeasureId(100), &mut buf, b',').unwrap();
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("true"));
        assert!(text.contains("false"));

        // A Text-typed column passes values through verbatim, never erroring.
        let mut tspec = spec;
        tspec.value_type = ValueType::Text;
        tspec.measure_id = MeasureId(101);
        tspec.measure_name = "Note".into();
        let text_csv = "product,active\nWidgetA,hello world\nWidgetB,42\n";
        let mut model2 = Model::new();
        import_csv_from(Cursor::new(text_csv), &mut model2, &tspec).unwrap();
        let coord_a2 = Coordinate::from_pairs([(product, item_id(&model2, product, "WidgetA"))]);
        assert_eq!(
            model2.input(MeasureId(101), &coord_a2),
            Some(&Value::Text("hello world".to_string()))
        );
    }

    #[test]
    fn missing_column_errors_clearly() {
        let mut model = Model::new();
        let mut spec = revenue_spec();
        spec.value_column = ColumnRef::Name("nope".into());
        assert!(import_csv_from(Cursor::new(SALES_CSV), &mut model, &spec).is_err());
    }

    #[test]
    fn export_missing_measure_errors() {
        let model = Model::new();
        let mut buf = Vec::new();
        assert!(export_measure_csv_to(&model, MeasureId(999), &mut buf, b',').is_err());
    }
}
