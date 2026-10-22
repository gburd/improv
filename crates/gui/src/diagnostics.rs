//! Located formula errors, window-free: the byte span of the formula an
//! error is about, its message, and a "why" hint for the common cases (a
//! misspelled name, the wrong number of arguments, a `(` left open, an
//! aggregation without `OVER`). The formula editors underline the span and
//! show the message and the hint under the row.

use improv_core_model::{functions, Model, ParseError};
use std::ops::Range;

/// An error located in a formula's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    /// Byte range to underline, if the error could be placed.
    pub span: Option<Range<usize>>,
    pub message: String,
    /// Why, or what to do: shown dimmed under the message.
    pub hint: Option<String>,
}

impl Diagnostic {
    /// A message with no place and no hint.
    pub fn plain(message: impl Into<String>) -> Self {
        Diagnostic {
            span: None,
            message: message.into(),
            hint: None,
        }
    }

    /// The same diagnostic for text that starts `by` bytes later (the
    /// `Name = ` before a formula).
    pub fn shifted(mut self, by: usize) -> Self {
        self.span = self.span.map(|r| r.start + by..r.end + by);
        self
    }
}

/// Locate `err` (from parsing `text`) and add a hint where one helps.
pub fn parse_error(model: &Model, text: &str, err: &ParseError) -> Diagnostic {
    let msg = err.message.as_str();
    let mut span = err.position.and_then(|p| token_at(text, p));
    let mut hint = None;
    if let Some(name) = msg.strip_prefix("unknown measure: ") {
        hint = unknown_measure(model, name);
    } else if let Some(name) = msg.strip_prefix("unknown category: ") {
        hint = unknown_category(model, name);
    } else if let Some((name, given)) = arity(msg) {
        // The message already spells the parameters: say what is off.
        if let Some(d) = functions::by_name(name).next() {
            hint = given.parse::<usize>().ok().map(|n| arity_hint(d, n));
            span = call_name(text, name, err.position.unwrap_or(text.len())).or(span);
        }
    } else if msg.starts_with("expected ')'") || msg == "unexpected end of input" {
        if let Some(open) = unclosed_paren(text) {
            span = Some(open..open + 1);
            hint = Some("( opened here is never closed: add )".into());
        }
    } else if let Some(name) = msg.split_once(" aggregates: ").map(|(n, _)| n) {
        if let Some(d) = functions::aggregation(name) {
            hint = Some(format!(
                "{} reduces a measure over a category, so it needs OVER: {}",
                d.name,
                d.hint()
            ));
            span = call_name(text, name, err.position.unwrap_or(text.len())).or(span);
        }
    } else if msg.ends_with(" does not aggregate OVER a category") {
        hint = Some("only aggregations take OVER (SUM, AVERAGE, MIN, MAX, COUNT, …)".into());
    }
    Diagnostic {
        span,
        message: msg.to_string(),
        hint,
    }
}

/// A compile (type or dimension) error for `text`, which has no position:
/// underline the first function or measure the message names that also
/// appears in the text.
pub fn compile_error(model: &Model, text: &str, message: &str) -> Diagnostic {
    let message = message.strip_prefix("compile error: ").unwrap_or(message);
    Diagnostic {
        span: mentioned_span(model, text, message),
        message: message.to_string(),
        hint: None,
    }
}

/// What is off about calling `d` with `n` arguments.
fn arity_hint(d: &functions::FnDef, n: usize) -> String {
    let names = d.param_names();
    if n < d.required {
        let missing: Vec<&str> = names
            .iter()
            .skip(n)
            .take(d.required - n)
            .map(String::as_str)
            .collect();
        format!("missing {}: {n} given", missing.join(", "))
    } else {
        let at_most = names.len();
        format!("at most {at_most} argument(s): {n} given; remove the extra")
    }
}

/// `"PMT takes …, got 2 argument(s)"`: the function and the count given.
fn arity(msg: &str) -> Option<(&str, &str)> {
    let (name, rest) = msg.split_once(" takes ")?;
    let given = rest.rsplit_once(", got ")?.1.strip_suffix(" argument(s)")?;
    Some((name, given))
}

fn unknown_measure(model: &Model, name: &str) -> Option<String> {
    if model.category_by_name(name).is_some() {
        return Some(format!(
            "{name} is a category, not a measure: reduce over it, e.g. SUM(<measure> OVER {name})"
        ));
    }
    if let Some(d) = functions::by_name(name).next() {
        return Some(format!("{} is a function: {}", d.name, d.hint()));
    }
    let measures = model.measures.values().map(|m| m.name.0.as_str());
    let mut near = closest(name, measures);
    if near.is_empty() {
        near = functions::closest(name, 3)
            .into_iter()
            .map(|n| format!("{n}(…)"))
            .collect();
    }
    did_you_mean(near)
}

fn unknown_category(model: &Model, name: &str) -> Option<String> {
    if model.measure_by_name(name).is_some() {
        return Some(format!("{name} is a measure, not a category"));
    }
    did_you_mean(closest(
        name,
        model.categories.values().map(|c| c.name.0.as_str()),
    ))
}

fn did_you_mean(near: Vec<String>) -> Option<String> {
    (!near.is_empty()).then(|| format!("did you mean {}?", near.join(", ")))
}

/// Up to three of `names` nearest `word` by edit distance (ignoring case),
/// within a third of its length (at least 2), nearest first.
fn closest<'a>(word: &str, names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let w = word.to_lowercase();
    let limit = (w.chars().count() / 3).max(2);
    let mut scored: Vec<(usize, &str)> = names
        .map(|n| (functions::edit_distance(&w, &n.to_lowercase()), n))
        .filter(|&(d, _)| d <= limit)
        .collect();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    scored
        .into_iter()
        .take(3)
        .map(|(_, n)| n.to_string())
        .collect()
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// The token starting at byte `pos`: a `'quoted name'`, a word, or one
/// character. At or past the end, the last character.
pub fn token_at(text: &str, pos: usize) -> Option<Range<usize>> {
    let mut pos = pos.min(text.len());
    if pos == text.len() {
        pos = text.char_indices().next_back()?.0;
    }
    while !text.is_char_boundary(pos) {
        pos -= 1;
    }
    let rest = &text[pos..];
    let first = rest.chars().next()?;
    let len = if first == '\'' {
        // To the closing quote ('' is an escaped quote), else the end.
        let mut it = rest.char_indices().skip(1).peekable();
        let mut end = rest.len();
        while let Some((i, c)) = it.next() {
            if c == '\'' {
                if it.peek().is_some_and(|&(_, n)| n == '\'') {
                    it.next();
                    continue;
                }
                end = i + 1;
                break;
            }
        }
        end
    } else if ident_char(first) || first == '@' {
        rest.char_indices()
            .skip(1)
            .find(|&(_, c)| !ident_char(c))
            .map_or(rest.len(), |(i, _)| i)
    } else {
        first.len_utf8()
    };
    Some(pos..pos + len)
}

/// Whether byte `i` of `text` is inside a `"string"`.
fn in_string(text: &str, i: usize) -> bool {
    text[..i].matches('"').count() % 2 == 1
}

/// Every whole-word occurrence of `name` in `text` outside strings
/// (ignoring ASCII case when `nocase`).
fn occurrences<'a>(
    text: &'a str,
    name: &'a str,
    nocase: bool,
) -> impl Iterator<Item = Range<usize>> + 'a {
    text.char_indices().filter_map(move |(i, _)| {
        let s = text.get(i..i + name.len())?;
        let same = if nocase {
            s.eq_ignore_ascii_case(name)
        } else {
            s == name
        };
        let before = text[..i].chars().next_back().is_none_or(|c| !ident_char(c));
        let after = text[i + name.len()..]
            .chars()
            .next()
            .is_none_or(|c| !ident_char(c));
        (same && before && after && !in_string(text, i)).then_some(i..i + name.len())
    })
}

/// The span of the last `name(` call starting before byte `before`.
fn call_name(text: &str, name: &str, before: usize) -> Option<Range<usize>> {
    occurrences(text, name, true)
        .filter(|r| r.start < before.max(1) && text[r.end..].trim_start().starts_with('('))
        .last()
}

/// Byte offset of the innermost `(` left open at the end of `text`;
/// parentheses in strings and quoted names do not count.
fn unclosed_paren(text: &str) -> Option<usize> {
    let mut open = Vec::new();
    let mut quote = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => open.push(i),
            (None, ')') => {
                open.pop();
            }
            _ => {}
        }
    }
    open.pop()
}

/// The span in `text` of the first function or measure `message` names:
/// a measure by its exact name, a function by its upper-case name (as
/// messages print it) written as a call. Earliest in the message wins.
fn mentioned_span(model: &Model, text: &str, message: &str) -> Option<Range<usize>> {
    let mut named: Vec<(usize, Range<usize>)> = Vec::new();
    for m in model.measures.values() {
        let name = m.name.0.as_str();
        if name.is_empty() {
            continue;
        }
        let (Some(at), Some(r)) = (
            occurrences(message, name, false).next(),
            occurrences(text, name, false).next(),
        ) else {
            continue;
        };
        named.push((at.start, r));
    }
    for word in message.split(|c: char| !ident_char(c)) {
        // `word` is a slice of `message`: its offset is exact.
        let start = word.as_ptr() as usize - message.as_ptr() as usize;
        let upper = !word.is_empty() && word.chars().all(|c| !c.is_lowercase());
        if !upper || functions::by_name(word).next().is_none() {
            continue;
        }
        if let Some(r) =
            occurrences(text, word, true).find(|r| text[r.end..].trim_start().starts_with('('))
        {
            named.push((start, r));
        }
    }
    named.into_iter().min_by_key(|(at, _)| *at).map(|(_, r)| r)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::{parser, CategoryId, Measure, MeasureId, MeasureKind, Name, ValueType};

    fn model() -> Model {
        let mut m = Model::new();
        m.add_category(CategoryId(1), "Time");
        m.add_category(CategoryId(2), "Product");
        for (id, name) in [(100, "Price"), (101, "Quantity"), (102, "Unit Cost")] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![CategoryId(1), CategoryId(2)],
                kind: MeasureKind::Input,
                description: None,
            });
        }
        m
    }

    fn diag(text: &str) -> Diagnostic {
        let m = model();
        let e = parser::parse_expr(&m, text).expect_err(text);
        parse_error(&m, text, &e)
    }

    fn marked(text: &str, d: &Diagnostic) -> String {
        d.span
            .clone()
            .map(|r| text[r].to_string())
            .unwrap_or_default()
    }

    #[test]
    fn unknown_names_suggest_the_closest() {
        let t = "Prise * Quantity";
        let d = diag(t);
        assert_eq!(marked(t, &d), "Prise");
        assert_eq!(d.message, "unknown measure: Prise");
        assert_eq!(d.hint.as_deref(), Some("did you mean Price?"));

        let t = "SUM(Quantity OVER Tyme)";
        let d = diag(t);
        assert_eq!(marked(t, &d), "Tyme");
        assert_eq!(d.hint.as_deref(), Some("did you mean Time?"));

        // No close measure: the closest functions, as calls.
        let h = diag("Averag + 1").hint.unwrap();
        assert!(h.starts_with("did you mean AVERAGE(…)"), "{h}");
        // A function or a category where a measure belongs.
        let h = diag("ABS + 1").hint.unwrap();
        assert!(h.starts_with("ABS is a function: ABS("), "{h}");
        assert!(diag("Time + 1")
            .hint
            .unwrap()
            .contains("SUM(<measure> OVER Time)"));
        assert_eq!(
            diag("SUM(Quantity OVER Price)").hint.as_deref(),
            Some("Price is a measure, not a category")
        );
        // Nothing near: no hint.
        assert_eq!(diag("Zzzzzzzzzz + 1").hint, None);
    }

    /// The parser names the closest functions itself; no hint repeats it.
    #[test]
    fn unknown_function_is_not_suggested_twice() {
        let t = "PMTT(0.05, 10, 100)";
        let d = diag(t);
        assert_eq!(marked(t, &d), "PMTT");
        assert!(d.message.contains("did you mean PMT"), "{}", d.message);
        assert_eq!(d.hint, None);
    }

    #[test]
    fn arity_names_the_parameters_and_marks_the_call() {
        let t = "1 + PMT(0.05, 10)";
        let d = diag(t);
        assert!(d.message.contains("PMT(rate, nper, pv, [fv=0], [type=0])"));
        assert_eq!(d.hint.as_deref(), Some("missing pv: 2 given"));
        assert_eq!(marked(t, &d), "PMT");
        // The innermost of two calls by that name; too many arguments.
        let t = "ABS(1) + abs(1, 2)";
        assert_eq!(d_span(t), 9..12);
        assert_eq!(
            diag(t).hint.as_deref(),
            Some("at most 1 argument(s): 2 given; remove the extra")
        );
    }

    fn d_span(t: &str) -> Range<usize> {
        diag(t).span.expect("placed")
    }

    #[test]
    fn unclosed_paren_marks_the_open_one() {
        let t = "ABS((Price + 1)";
        let d = diag(t);
        assert_eq!(d.span, Some(3..4));
        assert_eq!(
            d.hint.as_deref(),
            Some("( opened here is never closed: add )")
        );
        // A ")" in a string does not close it.
        let t = "LEN(\")\"";
        assert_eq!(d_span(t), 3..4);
    }

    #[test]
    fn aggregation_without_over_shows_the_form() {
        let t = "IRR(Quantity)";
        let d = diag(t);
        assert_eq!(marked(t, &d), "IRR");
        let h = d.hint.unwrap();
        let irr = functions::aggregation("IRR")
            .expect("IRR aggregates")
            .hint();
        assert!(h.contains("needs OVER") && h.ends_with(&irr), "{h}");
        let d = diag("ABS(Quantity OVER Time)");
        assert!(d.hint.unwrap().starts_with("only aggregations take OVER"));
    }

    #[test]
    fn token_at_reads_quoted_names_words_and_the_end() {
        let t = "'Unit Cost' * FLOOR.MATH(x)";
        assert_eq!(token_at(t, 0), Some(0..11));
        assert_eq!(token_at(t, 14), Some(14..24));
        assert_eq!(token_at(t, 12), Some(12..13));
        assert_eq!(token_at(t, 99), Some(26..27));
        assert_eq!(token_at("'it''s", 0), Some(0..6));
        assert_eq!(token_at("é", 1), Some(0..2));
        assert_eq!(token_at("", 0), None);
    }

    /// Compile errors carry no position: the first function or measure
    /// the message names that the text contains is underlined.
    #[test]
    fn compile_errors_underline_what_they_name() {
        let m = model();
        let span = |text: &str, msg: &str| {
            let d = compile_error(&m, text, msg);
            assert!(!d.message.starts_with("compile error"));
            d.span.map(|r| text[r].to_string())
        };
        assert_eq!(
            span(
                "Price + SUM(Quantity OVER Product)",
                "dimension mismatch: Price ranges over [Time, Product] and Quantity over [Time]"
            )
            .as_deref(),
            Some("Price")
        );
        // A function named in upper case, written as a call; "Text" and
        // "type" are words, not the TEXT and TYPE functions.
        assert_eq!(
            span(
                "Quantity + len(Price)",
                "compile error: type mismatch: LEN argument 1 must be Text, got Number (LEN(text))"
            )
            .as_deref(),
            Some("len")
        );
        assert_eq!(
            span(
                "'Unit Cost' * 2",
                "Unit Cost is Number but its formula yields Text"
            )
            .as_deref(),
            Some("Unit Cost")
        );
        // A name only inside a string literal is not the text's.
        assert_eq!(span("\"Price\" & 1", "Price is Text"), None);
        assert_eq!(span("1 + 2", "IF test must be Boolean"), None);
    }

    #[test]
    fn shifted_moves_the_span() {
        let d = Diagnostic {
            span: Some(1..3),
            ..Diagnostic::plain("x")
        };
        assert_eq!(d.shifted(4).span, Some(5..7));
        assert_eq!(Diagnostic::plain("x").shifted(4).span, None);
    }
}
