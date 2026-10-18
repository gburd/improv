//! Located formula errors, terminal-free: the span of text a
//! [`ParseError`] is about, its message, and a hint for the common cases
//! (a misspelled name, the wrong number of arguments, an unclosed `(`).

use crate::complete::source_name;
use improv_core_model::{functions, Model, ParseError};
use std::ops::Range;

/// A parse error located in the formula text.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    /// Byte range of the offending token, if the error has a position.
    pub span: Option<Range<usize>>,
    pub message: String,
    pub hint: Option<String>,
}

/// Locate `err` (from parsing `text`) and add a hint where one helps.
pub fn diagnose(model: &Model, text: &str, err: &ParseError) -> Diagnostic {
    let mut span = err.position.and_then(|p| token_span(text, p));
    let msg = &err.message;
    let mut hint = None;
    if let Some(name) = ["unknown measure: ", "unknown category: "]
        .iter()
        .find_map(|p| msg.strip_prefix(p))
    {
        let call = span
            .as_ref()
            .is_some_and(|s| text[s.end..].trim_start().starts_with('('));
        hint = did_you_mean(model, name, call, msg.starts_with("unknown category"));
    } else if let Some((name, _)) = msg
        .split_once(" takes ")
        .filter(|_| msg.contains("argument"))
    {
        if let Some(d) = functions::by_name(name).next() {
            hint = Some(format!("usage: {}", d.hint()));
            span = call_name(text, name, err.position.unwrap_or(text.len())).or(span);
        }
    } else if msg.starts_with("expected ')'") || msg == "unexpected end of input" {
        if let Some(open) = unclosed_paren(text) {
            span = Some(open..open + 1);
            hint = Some("this '(' is never closed: add ')'".into());
        }
    }
    Diagnostic {
        span,
        message: msg.clone(),
        hint,
    }
}

/// The token starting at byte `pos`: a quoted name, a word, a number, or
/// one character. Past the end, the last character.
fn token_span(text: &str, pos: usize) -> Option<Range<usize>> {
    let pos = if pos >= text.len() {
        text.char_indices().next_back()?.0
    } else {
        pos
    };
    let rest = text.get(pos..)?;
    let first = rest.chars().next()?;
    let len = if first == '\'' {
        // To the closing quote ('' is an escaped quote), else the end.
        let b = rest.as_bytes();
        let mut i = 1;
        while i < b.len() {
            if b[i] == b'\'' {
                if b.get(i + 1) == Some(&b'\'') {
                    i += 2;
                    continue;
                }
                i += 1;
                break;
            }
            i += 1;
        }
        i.min(b.len())
    } else if first.is_alphanumeric() || first == '_' || first == '@' {
        rest.char_indices()
            .skip(1)
            .find(|&(_, c)| !(c.is_alphanumeric() || c == '_' || c == '.'))
            .map_or(rest.len(), |(i, _)| i)
    } else {
        first.len_utf8()
    };
    Some(pos..pos + len)
}

/// The last `name(` before `before`, as the range of the name.
fn call_name(text: &str, name: &str, before: usize) -> Option<Range<usize>> {
    let lower = text.to_lowercase();
    let n = name.to_lowercase();
    if lower.len() != text.len() {
        return None; // offsets would not line up
    }
    let mut end = before.min(text.len());
    while let Some(i) = lower[..end].rfind(&n) {
        let after = i + n.len();
        let starts_word = lower[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_' || c == '.'));
        if starts_word && text[after..].trim_start().starts_with('(') {
            return Some(i..after);
        }
        end = i;
    }
    None
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

/// Levenshtein distance between two strings, by chars.
fn distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diag = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let next = (diag + usize::from(ca != *cb))
                .min(row[j] + 1)
                .min(row[j + 1] + 1);
            diag = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

/// "did you mean X?" for the names closest to `name` by edit distance:
/// functions when it is called, else categories (and measures, unless only
/// a category fits there).
fn did_you_mean(model: &Model, name: &str, call: bool, category: bool) -> Option<String> {
    let names: Vec<String> = if call {
        functions::all()
            .flat_map(|d| std::iter::once(d.name).chain(d.aliases.iter().copied()))
            .map(str::to_string)
            .collect()
    } else {
        let cats = model.categories.values().map(|c| &c.name.0);
        let measures = model.measures.values().map(|m| &m.name.0);
        let measures = measures.filter(|_| !category);
        cats.chain(measures).map(|n| source_name(n)).collect()
    };
    let want = name.to_lowercase();
    let limit = (want.chars().count() / 3).max(1);
    let mut close: Vec<(usize, String)> = names
        .into_iter()
        .map(|n| {
            let bare = n.trim_matches('\'').to_lowercase();
            (distance(&want, &bare), n)
        })
        .filter(|(d, _)| *d <= limit)
        .collect();
    close.sort();
    close.dedup();
    let best: Vec<String> = close.into_iter().take(3).map(|(_, n)| n).collect();
    (!best.is_empty()).then(|| format!("did you mean {}?", best.join(" or ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::revenue_model;
    use improv_core_model::parser::parse_expr;

    fn diag(text: &str) -> Diagnostic {
        let m = revenue_model();
        let err = parse_expr(&m, text).expect_err(text);
        diagnose(&m, text, &err)
    }

    fn spanned(text: &str) -> (&str, Option<String>) {
        let d = diag(text);
        (&text[d.span.expect("located")], d.hint)
    }

    #[test]
    fn unknown_names_suggest_the_closest() {
        assert_eq!(
            spanned("Price * Quantty"),
            ("Quantty", Some("did you mean Quantity?".into()))
        );
        // A misspelled function: the parser's message names the suggestion
        // (`unknown function: PMTT; did you mean PMT?`), so no second hint.
        let d = diag("PMTT(1, 2, 3)");
        assert_eq!(&"PMTT(1, 2, 3)"[d.span.expect("located")], "PMTT");
        assert!(d.message.contains("did you mean PMT"), "{}", d.message);
        assert_eq!(d.hint, None);
        assert_eq!(
            spanned("SUM(Price OVER Tyme)"),
            ("Tyme", Some("did you mean Time?".into()))
        );
        // Nothing close: no hint.
        assert_eq!(spanned("Price * Zebra"), ("Zebra", None));
    }

    #[test]
    fn wrong_argument_counts_show_the_signature() {
        assert_eq!(
            spanned("1 + PMT(1) + 2"),
            (
                "PMT",
                Some("usage: PMT(rate, nper, pv, [fv], [type])".into())
            )
        );
        assert_eq!(spanned("abs(1, 2)").1, Some("usage: ABS(number)".into()));
    }

    #[test]
    fn unclosed_parens_point_at_the_open_one() {
        let hint = Some("this '(' is never closed: add ')'".into());
        let d = diag("PMT(1, (2 + 3)");
        assert_eq!((d.span, d.hint), (Some(3..4), hint.clone()));
        assert_eq!(diag("(Price + 1").span, Some(0..1));
        assert_eq!(diag("ABS(Price +").hint, hint);
        // `)` inside a string does not close anything.
        assert_eq!(unclosed_paren("LEN(\")\""), Some(3));
        assert_eq!(unclosed_paren("'a(b' + 1"), None);
        // Balanced but missing a comma: the parser's own position.
        let d = diag("PMT(1 2)");
        assert_eq!((d.span, d.hint), (Some(6..7), None));
    }

    #[test]
    fn tokens_and_edges() {
        assert_eq!(token_span("'Unit Pr", 0), Some(0..8));
        assert_eq!(token_span("'it''s' + 1", 0), Some(0..7));
        assert_eq!(token_span("1 + 22.5", 4), Some(4..8));
        assert_eq!(token_span("Price $ 2", 6), Some(6..7));
        assert_eq!(token_span("Price +", 99), Some(6..7));
        assert_eq!(token_span("", 0), None);
        assert_eq!(token_span("é", 1), None, "not a char boundary");
        assert_eq!(call_name("ABSX(1) + abs(2)", "ABS", 16), Some(10..13));
        assert_eq!(call_name("İ abs(1)", "ABS", 9), None);
        assert_eq!(distance("kitten", "sitting"), 3);
        assert_eq!(distance("", "abc"), 3);
        // No position: just the message.
        let m = revenue_model();
        let e = ParseError {
            message: "empty expression".into(),
            position: None,
        };
        assert_eq!(diagnose(&m, "", &e).span, None);
    }
}
