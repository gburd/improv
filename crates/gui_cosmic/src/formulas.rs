//! The formula list and what its editor commits: every derived measure's
//! formula as editable text (the DSL as typed, else the controlled
//! English), and a typed formula read back, with errors located for the
//! underline. Pure; the located errors, the completion and the printer are
//! `improv_client`'s, shared with the other front ends.

use improv_client::diagnostics::{self, Diagnostic};
use improv_client::dsl;
use improv_core_model::{parser, Formula, Locale, MeasureId, MeasureKind, Model};
use improv_nl_formula::{describe_formula, parse_nl_formula, NlContext};

/// One row of the formula list.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub measure: MeasureId,
    /// The measure the formula defines (the row's `Name =`).
    pub target: String,
    /// The formula as the editor shows it.
    pub text: String,
}

/// Formula text in `locale` (decimal comma, `;`, translated names), or the
/// controlled English: canonical DSL is accepted in every locale.
pub fn parse(model: &Model, locale: &Locale, text: &str) -> Result<Formula, Diagnostic> {
    let canonical = locale.canonicalize(text);
    parser::parse_expr(model, &canonical).or_else(|e| {
        parse_nl_formula(&NlContext::new(model), text)
            .map_err(|_| diagnostics::parse_error(model, text, &e))
    })
}

/// `f` as text the editor shows and [`parse`] reads back to `f`: the DSL in
/// `locale` when it round-trips, else the controlled English.
pub fn source(model: &Model, locale: &Locale, f: &Formula) -> String {
    let english = describe_formula(&NlContext::new(model), f);
    dsl::formula(model, f)
        .map(|d| locale.localize(&d))
        .into_iter()
        .chain(std::iter::once(english.clone()))
        .find(|t| parse(model, locale, t).ok().as_ref() == Some(f))
        .unwrap_or(english)
}

/// Every formula, in measure id order.
pub fn rows(model: &Model, locale: &Locale) -> Vec<Row> {
    let mut out: Vec<Row> = model
        .measures
        .values()
        .filter_map(|m| match &m.kind {
            MeasureKind::Derived(f) => Some(Row {
                measure: m.id,
                target: m.name.0.clone(),
                text: source(model, locale, f),
            }),
            MeasureKind::Input => None,
        })
        .collect();
    out.sort_by_key(|r| r.measure.0);
    out
}

/// The structure (no input cells) with `measure`'s formula replaced by
/// `f`: what the service rebuilds and checks.
pub fn with_formula(model: &Model, measure: MeasureId, f: Formula) -> Result<Model, String> {
    let mut s = structure(model);
    let m = s
        .measures
        .get_mut(&measure)
        .ok_or_else(|| format!("no measure {}", measure.0))?;
    m.kind = MeasureKind::Derived(f);
    Ok(s)
}

/// Everything in `model` but its input cells (a field `Model` gains goes
/// here too, as in `improv_store::ChangeSet::between`).
pub fn structure(model: &Model) -> Model {
    let mut s = Model::new();
    s.categories.clone_from(&model.categories);
    s.items.clone_from(&model.items);
    s.measures.clone_from(&model.measures);
    s.sql_sources.clone_from(&model.sql_sources);
    s.views.clone_from(&model.views);
    s.external_fns.clone_from(&model.external_fns);
    s.external_calls.clone_from(&model.external_calls);
    s.scenarios.clone_from(&model.scenarios);
    s.lambdas.clone_from(&model.lambdas);
    s
}

/// A formula that parsed but did not build (a cycle, an unknown function's
/// arity, a dimension mismatch), located in `text` where it can be.
pub fn build_error(model: &Model, text: &str, message: &str) -> Diagnostic {
    diagnostics::compile_error(model, text, message)
}

/// `text` with a line of `^` under the bytes `span` covers, as a monospace
/// pair of lines: where the error is, without color.
pub fn caret_line(text: &str, span: std::ops::Range<usize>) -> String {
    let fix = |i: usize| {
        let mut i = i.min(text.len());
        while !text.is_char_boundary(i) {
            i -= 1;
        }
        i
    };
    let (a, b) = (fix(span.start), fix(span.end));
    let pad = text[..a].chars().count();
    let marks = text[a..b].chars().count().max(1);
    format!("{text}\n{}{}", " ".repeat(pad), "^".repeat(marks))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::tests::revenue_model;

    #[test]
    fn formulas_list_as_typed_and_read_back() {
        let m = revenue_model();
        let en = Locale::en();
        let rows = rows(&m, &en);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "Revenue");
        assert_eq!(rows[0].text, "Price * Quantity");
        let f = parse(&m, &en, &rows[0].text).unwrap();
        assert_eq!(MeasureKind::Derived(f), m.measures[&MeasureId(102)].kind);
    }

    #[test]
    fn english_and_the_locale_parse_too() {
        let m = revenue_model();
        assert!(parse(&m, &Locale::en(), "Price times Quantity").is_ok());
        let de = Locale::de();
        let f = parse(&m, &de, "Price * 1,5").unwrap();
        assert_eq!(source(&m, &de, &f), "Price * 1,5");
    }

    #[test]
    fn a_misspelled_name_is_located() {
        let m = revenue_model();
        let d = parse(&m, &Locale::en(), "Price * Quantty").unwrap_err();
        assert_eq!(d.span, Some(8..15), "{d:?}");
        assert!(
            d.hint.as_deref().unwrap_or("").contains("Quantity"),
            "{d:?}"
        );
    }

    #[test]
    fn the_caret_line_marks_the_span_by_chars() {
        assert_eq!(
            caret_line("Price * Quantty", 8..15),
            "Price * Quantty\n        ^^^^^^^"
        );
        // `x` is bytes 4..5 but the fourth char: three spaces, not four.
        assert_eq!(
            caret_line("é + x", 4..5),
            "é + x\n   ^",
            "by chars, not bytes"
        );
        assert_eq!(caret_line("a", 5..9), "a\n ^", "clamped");
    }

    #[test]
    fn a_new_formula_keeps_the_rest_of_the_structure() {
        let m = revenue_model();
        let f = parse(&m, &Locale::en(), "Price + Quantity").unwrap();
        let s = with_formula(&m, MeasureId(102), f.clone()).unwrap();
        assert!(s.inputs.is_empty());
        assert_eq!(s.categories, m.categories);
        assert_eq!(
            s.measures[&MeasureId(102)].kind,
            MeasureKind::Derived(f.clone())
        );
        assert!(with_formula(&m, MeasureId(9), f).is_err());
    }
}
