//! CSV/TSV import/export wizard: form state + pure spec-building for the GUI
//! panel (rendered by `ImprovApp::csv_import_ui`/`csv_export_ui` in `app.rs`).
//! Mirrors the CLI's `import-csv`/`export-csv` semantics (delimiter
//! auto-detected from the `.tsv` extension unless overridden; `item_id_base`
//! set high enough to avoid colliding with hand-assigned ids — see
//! `crates/cli/src/main.rs::cmd_import_csv`/`cmd_export_csv`) so the wizard
//! behaves exactly like `improv import-csv`/`export-csv`.
//!
//! Kept free of any `egui::Ui` parameter so `build_import_spec`/
//! `build_export_args`/`detect_delimiter` are unit-tested without a window;
//! the actual widgets live in `app.rs`, which owns `ImprovApp`'s state.

use std::path::PathBuf;

use improv_core_model::{CategoryId, MeasureId, ValueType};
use improv_storage_csv::{ColumnRef, DimensionMapping, ImportSpec};

/// Base id for minting item ids on import — high enough to avoid colliding
/// with hand-assigned ids (matches the CLI's `cmd_import_csv`).
const ITEM_ID_BASE: u32 = 1_000_000;

/// One dimension-mapping row in the import form: a CSV/TSV column, and the
/// category it becomes (id + name). All three fields are free text, parsed on
/// submit by `build_import_spec`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DimRow {
    pub column: String,
    pub category_id: String,
    pub category_name: String,
}

/// The import wizard's editable form state. Mirrors the CLI's `import-csv`
/// arguments (see `crates/cli/src/main.rs::cmd_import_csv`).
#[derive(Debug, Clone, PartialEq)]
pub struct ImportForm {
    pub path: String,
    /// Force TSV even if the path doesn't end in `.tsv` (checkbox).
    pub tsv: bool,
    pub has_header: bool,
    /// Blank = auto-assign the next free measure id.
    pub measure_id: String,
    pub measure_name: String,
    pub value_column: String,
    pub dimensions: Vec<DimRow>,
}

impl Default for ImportForm {
    fn default() -> Self {
        ImportForm {
            path: String::new(),
            tsv: false,
            has_header: true,
            measure_id: String::new(),
            measure_name: String::new(),
            value_column: String::new(),
            dimensions: vec![DimRow::default()],
        }
    }
}

/// The export wizard's form state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportForm {
    pub measure_id: Option<MeasureId>,
    pub path: String,
    pub tsv: bool,
}

/// The delimiter for `path`: TSV if `tsv_checked` or the path ends in `.tsv`,
/// else CSV. Matches the CLI's `--tsv`/extension convention exactly.
pub fn detect_delimiter(path: &str, tsv_checked: bool) -> u8 {
    if tsv_checked || path.ends_with(".tsv") {
        b'\t'
    } else {
        b','
    }
}

/// A column reference: a 0-based index when there's no header row, else a
/// header name (mirrors the CLI's `col_ref` closure).
fn column_ref(s: &str, has_header: bool) -> ColumnRef {
    if has_header {
        ColumnRef::Name(s.to_string())
    } else {
        s.parse::<usize>()
            .map(ColumnRef::Index)
            .unwrap_or_else(|_| ColumnRef::Name(s.to_string()))
    }
}

/// Build an `ImportSpec` from the wizard's form fields. Validates as the CLI
/// does: a non-empty path, a numeric measure id, a non-empty measure name and
/// value column, and at least one fully-filled dimension row (a blank spacer
/// row — all three fields empty — is skipped, not an error). Pure: no egui, no
/// I/O; `improv_storage_csv::import_csv` does the real work.
pub fn build_import_spec(form: &ImportForm) -> Result<ImportSpec, String> {
    let path_str = form.path.trim();
    if path_str.is_empty() {
        return Err("file path is required".into());
    }
    let measure_id = form
        .measure_id
        .trim()
        .parse::<u32>()
        .map_err(|_| format!("measure id must be a number, got '{}'", form.measure_id))?;
    let measure_name = form.measure_name.trim();
    if measure_name.is_empty() {
        return Err("measure name is required".into());
    }
    let value_col = form.value_column.trim();
    if value_col.is_empty() {
        return Err("value column is required".into());
    }

    let mut dimensions = Vec::new();
    for row in &form.dimensions {
        let (col, cid, cname) = (
            row.column.trim(),
            row.category_id.trim(),
            row.category_name.trim(),
        );
        if col.is_empty() && cid.is_empty() && cname.is_empty() {
            continue; // blank spacer row
        }
        if col.is_empty() || cid.is_empty() || cname.is_empty() {
            return Err(
                "each dimension row needs a column, a category id, and a category name".into(),
            );
        }
        let category_id = cid
            .parse::<u32>()
            .map_err(|_| format!("category id must be a number, got '{cid}'"))?;
        dimensions.push(DimensionMapping {
            column: column_ref(col, form.has_header),
            category_id: CategoryId(category_id),
            category_name: cname.to_string(),
        });
    }
    if dimensions.is_empty() {
        return Err("at least one dimension mapping is required".into());
    }

    Ok(ImportSpec {
        path: PathBuf::from(path_str),
        delimiter: detect_delimiter(path_str, form.tsv),
        has_header: form.has_header,
        measure_id: MeasureId(measure_id),
        measure_name: measure_name.to_string(),
        value_type: ValueType::Number,
        value_column: column_ref(value_col, form.has_header),
        dimensions,
        item_id_base: ITEM_ID_BASE,
    })
}

/// Resolve the export form into `(measure, path, delimiter)`. Pure: no egui,
/// no I/O; `improv_storage_csv::export_measure_csv` does the real work.
pub fn build_export_args(form: &ExportForm) -> Result<(MeasureId, PathBuf, u8), String> {
    let measure = form.measure_id.ok_or("select a measure to export")?;
    let path_str = form.path.trim();
    if path_str.is_empty() {
        return Err("target file path is required".into());
    }
    Ok((
        measure,
        PathBuf::from(path_str),
        detect_delimiter(path_str, form.tsv),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{Coordinate, ItemId, Measure, MeasureKind, Model, Name, Value};
    use improv_storage_csv::export_measure_csv;

    /// A unique path under the OS temp dir; removed by `Drop`.
    struct TempFile(PathBuf);
    impl TempFile {
        fn new(suffix: &str) -> Self {
            let p = std::env::temp_dir().join(format!(
                "improv_gui_csv_wizard_test_{}_{}{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                suffix
            ));
            TempFile(p)
        }
        fn path_str(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn dim(col: &str, id: &str, name: &str) -> DimRow {
        DimRow {
            column: col.into(),
            category_id: id.into(),
            category_name: name.into(),
        }
    }

    #[test]
    fn detect_delimiter_from_extension() {
        assert_eq!(detect_delimiter("sales.csv", false), b',');
        assert_eq!(detect_delimiter("sales.tsv", false), b'\t');
        assert_eq!(detect_delimiter("sales.csv", true), b'\t'); // checkbox overrides
        assert_eq!(detect_delimiter("no_extension", false), b',');
    }

    #[test]
    fn build_import_spec_end_to_end_via_real_tempfile() {
        let tmp = TempFile::new(".csv");
        std::fs::write(
            &tmp.0,
            "time,product,revenue\n2025,WidgetA,1000\n2025,WidgetB,500\n2026,WidgetA,1200\n",
        )
        .unwrap();

        let form = ImportForm {
            path: tmp.path_str(),
            tsv: false,
            has_header: true,
            measure_id: "100".into(),
            measure_name: "Revenue".into(),
            value_column: "revenue".into(),
            dimensions: vec![dim("time", "1", "Time"), dim("product", "2", "Product")],
        };
        let spec = build_import_spec(&form).expect("valid spec");
        assert_eq!(spec.delimiter, b',');

        let mut model = Model::new();
        let n = improv_storage_csv::import_csv(&mut model, &spec).expect("import");
        assert_eq!(n, 3);
        assert_eq!(model.category_by_name("Time").unwrap().items.len(), 2);
        assert_eq!(model.category_by_name("Product").unwrap().items.len(), 2);
        let m = model.measure_by_name("Revenue").unwrap();
        assert!(m.is_input());
        assert_eq!(model.inputs.len(), 3);
    }

    #[test]
    fn tsv_extension_is_auto_detected() {
        let tmp = TempFile::new(".tsv");
        std::fs::write(&tmp.0, "time\tproduct\trevenue\n2025\tWidgetA\t1000\n").unwrap();

        let form = ImportForm {
            path: tmp.path_str(),
            tsv: false, // no explicit override — extension alone should pick TSV
            has_header: true,
            measure_id: "1".into(),
            measure_name: "Revenue".into(),
            value_column: "revenue".into(),
            dimensions: vec![dim("time", "1", "Time"), dim("product", "2", "Product")],
        };
        let spec = build_import_spec(&form).expect("valid spec");
        assert_eq!(spec.delimiter, b'\t');

        let mut model = Model::new();
        let n = improv_storage_csv::import_csv(&mut model, &spec).expect("import");
        assert_eq!(n, 1);
    }

    #[test]
    fn export_then_reimport_round_trips() {
        let export_tmp = TempFile::new(".csv");

        // Build a small model with an input cell directly (bypassing the form).
        let mut model = Model::new();
        let (t, p) = (CategoryId(1), CategoryId(2));
        model.add_category(t, "Time");
        model.add_category(p, "Product");
        model.add_item(ItemId(10), t, "2025");
        model.add_item(ItemId(20), p, "WidgetA");
        model.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Input,
            description: None,
        });
        model.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(t, ItemId(10)), (p, ItemId(20))]),
            Value::Number(42.0),
        );

        let export_form = ExportForm {
            measure_id: Some(MeasureId(100)),
            path: export_tmp.path_str(),
            tsv: false,
        };
        let (measure, path, delimiter) = build_export_args(&export_form).expect("valid export");
        let exported = export_measure_csv(&model, measure, &path, delimiter).expect("export");
        assert_eq!(exported, 1);

        // Re-import into a fresh model using the exported header names.
        let import_form = ImportForm {
            path: export_tmp.path_str(),
            tsv: false,
            has_header: true,
            measure_id: "200".into(),
            measure_name: "Revenue2".into(),
            value_column: "Revenue".into(),
            dimensions: vec![dim("Time", "1", "Time"), dim("Product", "2", "Product")],
        };
        let spec = build_import_spec(&import_form).expect("valid spec");
        let mut model2 = Model::new();
        let n = improv_storage_csv::import_csv(&mut model2, &spec).expect("reimport");
        assert_eq!(n, 1);
        let vals: Vec<f64> = model2
            .inputs
            .values()
            .filter_map(|v| v.as_number())
            .collect();
        assert_eq!(vals, vec![42.0]);
    }

    #[test]
    fn malformed_spec_is_err_not_panic() {
        // Missing dimensions (a blank row is skipped, leaving none).
        let no_dims = ImportForm {
            path: "whatever.csv".into(),
            measure_id: "1".into(),
            measure_name: "M".into(),
            value_column: "v".into(),
            ..ImportForm::default()
        };
        assert!(build_import_spec(&no_dims).is_err());

        // Empty path.
        let no_path = ImportForm {
            measure_id: "1".into(),
            measure_name: "M".into(),
            value_column: "v".into(),
            dimensions: vec![dim("a", "1", "A")],
            ..ImportForm::default()
        };
        assert!(build_import_spec(&no_path).is_err());

        // Non-numeric measure id.
        let bad_id = ImportForm {
            path: "x.csv".into(),
            measure_id: "notanumber".into(),
            measure_name: "M".into(),
            value_column: "v".into(),
            dimensions: vec![dim("a", "1", "A")],
            ..ImportForm::default()
        };
        assert!(build_import_spec(&bad_id).is_err());

        // Partial dimension row (column set, category id/name blank).
        let partial_dim = ImportForm {
            path: "x.csv".into(),
            measure_id: "1".into(),
            measure_name: "M".into(),
            value_column: "v".into(),
            dimensions: vec![dim("a", "", "")],
            ..ImportForm::default()
        };
        assert!(build_import_spec(&partial_dim).is_err());

        // A well-formed spec against a nonexistent file: `import_csv` errors
        // (io::Error), never panics.
        let good_spec_bad_file = ImportForm {
            path: "/nonexistent/path/does/not/exist.csv".into(),
            measure_id: "1".into(),
            measure_name: "M".into(),
            value_column: "v".into(),
            dimensions: vec![dim("a", "1", "A")],
            ..ImportForm::default()
        };
        let spec = build_import_spec(&good_spec_bad_file).expect("spec itself is well-formed");
        let mut model = Model::new();
        assert!(improv_storage_csv::import_csv(&mut model, &spec).is_err());
    }
}
