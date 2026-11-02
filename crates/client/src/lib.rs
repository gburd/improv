//! What every Improv front end shares, without the engine or a store: the
//! formula editor's located errors ([`diagnostics`]), autocomplete
//! ([`completion`]), the function help browser ([`function_help`]) and the
//! printer of formulas as the DSL they were typed in ([`dsl`]). All are pure
//! functions of a [`Model`](improv_core_model::Model), so they build for
//! `wasm32-unknown-unknown` and run in the browser as they do on the desktop
//! and in the server.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod completion;
pub mod diagnostics;
pub mod dsl;
pub mod function_help;

/// The paths the moved files still name (`crate::app::…`, from the GUI and
/// the TUI they came from). shortcut: a shim for the move commit alone,
/// removed by the next one.
#[doc(hidden)]
pub mod app {
    /// The TUI's app, named by `dsl`'s module doc.
    pub struct App;
    impl App {
        /// See `crates/tui`.
        pub fn formula_text(&self) {}
    }

    /// The GUI's quoting of a name in a formula (`crates/gui`'s `ident`).
    pub(crate) fn ident(name: &str) -> Option<String> {
        if is_bare_ident(name) {
            return Some(name.to_string());
        }
        // `''` names nothing (a parse error), so an empty name stays unspellable.
        if name.is_empty() {
            return None;
        }
        Some(format!("'{}'", name.replace('\'', "''")))
    }

    /// Whether `name` survives a round trip through the tokenizer as one *bare*
    /// identifier that the grammar will not mistake for a keyword.
    fn is_bare_ident(name: &str) -> bool {
        let mut chars = name.chars();
        let head_ok = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
        if !head_ok || !chars.all(|c| c.is_alphanumeric() || c == '_') {
            return false;
        }
        // A bare keyword is consumed as grammar, never as a name. (`SUM`/`AVG` and
        // the scalar-function names are NOT keywords here: the parser only treats
        // them as functions when directly followed by `(`, which a printed
        // measure reference never is.)
        const KEYWORDS: &[&str] = &["NOT", "AND", "OR", "OVER", "TRUE", "FALSE"];
        !KEYWORDS.iter().any(|k| name.eq_ignore_ascii_case(k))
    }

    #[cfg(test)]
    pub(crate) mod tests {
        use improv_core_model::{
            BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure,
            MeasureId, MeasureKind, Model, Name, Value, ValueType,
        };

        // Canonical Time x Product revenue model (mirrors engine's fixture).
        pub(crate) fn revenue_model() -> Model {
            let mut m = Model::new();
            let (time, product) = (CategoryId(1), CategoryId(2));
            m.add_category(time, "Time");
            m.add_category(product, "Product");
            m.add_item(ItemId(10), time, "2025");
            m.add_item(ItemId(11), time, "2026");
            m.add_item(ItemId(20), product, "WidgetA");
            m.add_item(ItemId(21), product, "WidgetB");

            m.add_measure(Measure {
                id: MeasureId(100),
                name: Name("Price".into()),
                value_type: ValueType::Number,
                categories: vec![product],
                kind: MeasureKind::Input,
                description: None,
                format: None,
            });
            m.add_measure(Measure {
                id: MeasureId(101),
                name: Name("Quantity".into()),
                value_type: ValueType::Number,
                categories: vec![time, product],
                kind: MeasureKind::Input,
                description: None,
                format: None,
            });
            m.add_measure(Measure {
                id: MeasureId(102),
                name: Name("Revenue".into()),
                value_type: ValueType::Number,
                categories: vec![time, product],
                kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                    BinaryOp::Mul,
                    Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                    Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
                ))),
                description: None,
                format: None,
            });

            let coord = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
            m.set_input(
                MeasureId(100),
                coord(&[(product, ItemId(20))]),
                Value::Number(10.0),
            );
            m.set_input(
                MeasureId(100),
                coord(&[(product, ItemId(21))]),
                Value::Number(20.0),
            );
            for (t, p, q) in [
                (ItemId(10), ItemId(20), 100.0),
                (ItemId(10), ItemId(21), 50.0),
                (ItemId(11), ItemId(20), 120.0),
                (ItemId(11), ItemId(21), 80.0),
            ] {
                m.set_input(
                    MeasureId(101),
                    coord(&[(time, t), (product, p)]),
                    Value::Number(q),
                );
            }
            m
        }
    }
}
