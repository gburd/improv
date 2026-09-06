//! Fuzz `parse_expr`/`parse_formula`/`parse_definition` with arbitrary UTF-8
//! against a fixed fixture model.
//! Property: they never panic. Ok or Err are both acceptable.
#![no_main]

use improv_core_model::parser::{parse_definition, parse_expr, parse_formula};
use improv_core_model::{CategoryId, Measure, MeasureId, MeasureKind, Model, Name, ValueType};
use libfuzzer_sys::fuzz_target;

fn fixture() -> Model {
    let mut m = Model::new();
    m.add_category(CategoryId(1), "Time");
    m.add_category(CategoryId(2), "Product");
    for (id, name, cats) in [
        (100u32, "Price", vec![CategoryId(2)]),
        (101, "Quantity", vec![CategoryId(1), CategoryId(2)]),
        (
            102,
            "Revenue",
            vec![CategoryId(1), CategoryId(2)],
        ),
    ] {
        m.add_measure(Measure {
            id: MeasureId(id),
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: cats,
            kind: MeasureKind::Input,
            description: None,
        });
    }
    m
}

fuzz_target!(|data: &str| {
    let model = fixture();
    // Only the parsers are under test; discard the results. Must never panic.
    let _ = parse_expr(&model, data);
    let _ = parse_formula(&model, data);
    let _ = parse_definition(&model, data);
});
