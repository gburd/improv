//! Fuzz `parse_nl_formula` with arbitrary UTF-8 against a fixed fixture model.
//! Property: it never panics. Ok or Err are both acceptable.
#![no_main]

use improv_core_model::{CategoryId, Measure, MeasureId, MeasureKind, Model, Name, ValueType};
use improv_nl_formula::{parse_nl_formula, NlContext};
use libfuzzer_sys::fuzz_target;

fn fixture() -> Model {
    let mut m = Model::new();
    m.add_category(CategoryId(1), "Time");
    m.add_category(CategoryId(2), "Product");
    m.add_category(CategoryId(3), "Region");
    for (id, name, cats) in [
        (100u32, "Price", vec![CategoryId(2)]),
        (101, "Quantity", vec![CategoryId(1), CategoryId(2)]),
        (
            102,
            "Revenue",
            vec![CategoryId(1), CategoryId(2), CategoryId(3)],
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
    let ctx = NlContext::new(&model);
    // Only the parser is under test; discard the result. Must never panic.
    let _ = parse_nl_formula(&ctx, data);
});
