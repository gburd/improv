//! File-path entry points and the less common value kinds of CSV export.

use improv_core_model::{
    CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure, MeasureId, MeasureKind,
    Model, Name, Value, ValueError, ValueErrorKind, ValueType,
};
use improv_storage_csv::{
    export_measure_csv, export_measure_csv_with, import_csv, import_csv_from, ColumnRef,
    DimensionMapping, ExportSpec, ImportSpec,
};
use std::path::PathBuf;

const TIME: CategoryId = CategoryId(1);

fn spec(path: PathBuf, value_type: ValueType, value_column: ColumnRef) -> ImportSpec {
    ImportSpec {
        path,
        delimiter: b',',
        has_header: true,
        measure_id: MeasureId(100),
        measure_name: "V".into(),
        value_type,
        value_column,
        dimensions: vec![DimensionMapping {
            column: ColumnRef::Index(0),
            category_id: TIME,
            category_name: "Time".into(),
        }],
        item_id_base: 1000,
        max_records: None,
    }
}

fn measure(id: u32, value_type: ValueType, kind: MeasureKind) -> Measure {
    Measure {
        id: MeasureId(id),
        name: Name(format!("M{id}")),
        value_type,
        categories: vec![TIME],
        kind,
        description: None,
    }
}

fn export(model: &Model, id: u32) -> String {
    let mut out = Vec::new();
    export_measure_csv_with(model, MeasureId(id), &mut out, &ExportSpec::default()).unwrap();
    String::from_utf8(out).unwrap()
}

#[test]
fn file_round_trip_through_paths() {
    let dir = std::env::temp_dir().join(format!("improv_csv_cov_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let src = dir.join("in.csv");
    std::fs::write(&src, "t,v\n2025,1.5\n2026,2\n").unwrap();
    let mut model = Model::new();
    let n = import_csv(
        &mut model,
        &spec(src.clone(), ValueType::Number, ColumnRef::Index(1)),
    )
    .unwrap();
    assert_eq!(n, 2);
    let dst = dir.join("out.csv");
    assert_eq!(
        export_measure_csv(&model, MeasureId(100), &dst, b',').unwrap(),
        2
    );
    assert_eq!(
        std::fs::read_to_string(&dst).unwrap(),
        "Time,V\n2025,1.5\n2026,2\n"
    );
    // A missing file is an I/O error, not a panic.
    let missing = spec(dir.join("nope.csv"), ValueType::Number, ColumnRef::Index(1));
    assert!(import_csv(&mut model, &missing)
        .unwrap_err()
        .to_string()
        .contains("io error"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn named_column_without_header_and_boolean_fallback() {
    let mut s = spec(
        PathBuf::new(),
        ValueType::Boolean,
        ColumnRef::Name("v".into()),
    );
    s.has_header = false;
    let err = import_csv_from("2025,true\n".as_bytes(), &mut Model::new(), &s).unwrap_err();
    assert!(err.to_string().contains("needs a header row"), "{err}");
    // A Boolean column keeps a non-boolean string as Text.
    let s = spec(PathBuf::new(), ValueType::Boolean, ColumnRef::Index(1));
    let mut model = Model::new();
    import_csv_from("t,v\n2025,maybe\n2026,FALSE\n".as_bytes(), &mut model, &s).unwrap();
    let mut vals: Vec<_> = model.inputs.values().cloned().collect();
    vals.sort_by_key(|v| format!("{v:?}"));
    assert_eq!(vals, [Value::Boolean(false), Value::Text("maybe".into())]);
}

#[test]
fn export_input_datetime_enum_and_unknown_category() {
    let mut model = Model::new();
    model.add_category(TIME, "Time");
    model.add_item(ItemId(10), TIME, "2025");
    model.add_item(ItemId(11), TIME, "2026");
    model.add_item(ItemId(12), TIME, "2027");
    model.add_measure(measure(1, ValueType::DateTime, MeasureKind::Input));
    let at = |i| Coordinate::from_pairs([(TIME, ItemId(i))]);
    model.set_input(
        MeasureId(1),
        at(10),
        Value::DateTime(chrono::DateTime::UNIX_EPOCH),
    );
    model.set_input(MeasureId(1), at(11), Value::Enum(3));
    model.set_input(
        MeasureId(1),
        at(12),
        Value::Error(ValueError::new(ValueErrorKind::MissingInput)),
    );
    assert_eq!(
        export(&model, 1),
        "Time,M1\n2025,1970-01-01T00:00:00+00:00\n2026,3\n"
    );
    // A category missing from the model is named by its id.
    model.categories.remove(&TIME);
    assert!(export(&model, 1).starts_with("cat1,M1\n"));
}

#[test]
fn export_derived_text_bool_and_error_cells() {
    let mut model = Model::new();
    model.add_category(TIME, "Time");
    model.add_item(ItemId(10), TIME, "2025");
    let at = Coordinate::from_pairs([(TIME, ItemId(10))]);
    let refr = |id| {
        MeasureKind::Derived(Formula::new(Expr::Ref(
            MeasureId(id),
            DimensionSpec::default(),
        )))
    };
    for (input, derived, vt, v) in [
        (1, 11, ValueType::Text, Value::Text("=1+1".into())),
        (2, 12, ValueType::Boolean, Value::Boolean(true)),
        (
            3,
            13,
            ValueType::Number,
            Value::Error(ValueError::new(ValueErrorKind::DivisionByZero)),
        ),
    ] {
        model.add_measure(measure(input, vt, MeasureKind::Input));
        model.add_measure(measure(derived, vt, refr(input)));
        model.set_input(MeasureId(input), at.clone(), v);
    }
    assert_eq!(export(&model, 11), "Time,M11\n2025,'=1+1\n");
    assert_eq!(export(&model, 12), "Time,M12\n2025,true\n");
    assert_eq!(export(&model, 13), "Time,M13\n");
    // A derived measure that can't evaluate (self-cycle) is a clean error.
    model.add_measure(measure(14, ValueType::Number, refr(14)));
    let mut out = Vec::new();
    let err = export_measure_csv_with(&model, MeasureId(14), &mut out, &ExportSpec::default())
        .unwrap_err();
    assert!(err.to_string().contains("evaluating measure"), "{err}");
}
