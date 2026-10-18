//! Formula-edit assistance, terminal-free: Tab completion of the identifier
//! before the caret, and the ghost text of the parameters still to type.

use crate::edit::LineEdit;
use improv_core_model::{functions, Model};

/// Whether `c` can be part of a bare identifier (`FLOOR.MATH` included).
fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// Byte offset where the identifier ending at `caret` starts.
pub fn word_start(text: &str, caret: usize) -> usize {
    text[..caret]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| ident_char(c))
        .last()
        .map_or(caret, |(i, _)| i)
}

/// `name` as formula source: bare if it can be, else `'quoted'`.
fn source_name(name: &str) -> String {
    let mut chars = name.chars();
    let bare = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
        && !["NOT", "AND", "OR", "OVER", "TRUE", "FALSE"]
            .iter()
            .any(|k| name.eq_ignore_ascii_case(k));
    if bare {
        name.to_string()
    } else {
        format!("'{}'", name.replace('\'', "''"))
    }
}

/// What Tab may put in place of `prefix`: the model's measure and category
/// names (case-insensitive prefix match, sorted), then functions as `NAME(`.
pub fn candidates(model: &Model, prefix: &str) -> Vec<String> {
    if prefix.is_empty() {
        return Vec::new();
    }
    let p = prefix.to_lowercase();
    let mut names: Vec<&str> = model
        .measures
        .values()
        .map(|m| m.name.0.as_str())
        .chain(model.categories.values().map(|c| c.name.0.as_str()))
        .filter(|n| n.to_lowercase().starts_with(&p))
        .collect();
    names.sort_unstable();
    names.dedup();
    let mut out: Vec<String> = names.into_iter().map(source_name).collect();
    out.extend(
        functions::complete(prefix)
            .iter()
            .map(|(n, _)| format!("{n}(")),
    );
    out
}

/// The longest prefix all of `cands` share, compared case-insensitively,
/// spelled as the first one spells it.
fn common_prefix(cands: &[String]) -> &str {
    let first = &cands[0];
    let mut end = first.len();
    for c in &cands[1..] {
        end = first
            .char_indices()
            .zip(c.chars())
            .find(|((_, a), b)| !a.eq_ignore_ascii_case(b))
            .map_or(end.min(c.len()), |((i, _), _)| end.min(i));
    }
    &first[..end]
}

/// Repeated-Tab state: the candidates for the word at `start`, and which one
/// the line shows (`None` until the second Tab).
#[derive(Debug, Clone, PartialEq)]
pub struct Cycle {
    start: usize,
    cands: Vec<String>,
    shown: Option<usize>,
}

/// One Tab in `e`. A single candidate replaces the word; several extend it
/// to their common prefix, and each further Tab (with `cycle` kept from the
/// last one) puts the next candidate in. Returns the status line to show:
/// the candidates, or that there are none.
pub fn tab(model: &Model, e: &mut LineEdit, cycle: &mut Option<Cycle>) -> Option<String> {
    if let Some(c) = cycle {
        let i = c.shown.map_or(0, |i| (i + 1) % c.cands.len());
        c.shown = Some(i);
        e.replace(c.start, &c.cands[i]);
        return Some(listing(&c.cands));
    }
    let start = word_start(&e.text, e.caret);
    let prefix = &e.text[start..e.caret];
    if prefix.is_empty() {
        return None;
    }
    let cands = candidates(model, prefix);
    match cands.len() {
        0 => Some(format!("no completions for '{prefix}'")),
        1 => {
            e.replace(start, &cands[0]);
            None
        }
        _ => {
            let common = common_prefix(&cands);
            if common != prefix {
                e.replace(start, common);
            }
            let msg = listing(&cands);
            *cycle = Some(Cycle {
                start,
                cands,
                shown: None,
            });
            Some(msg)
        }
    }
}

fn listing(cands: &[String]) -> String {
    format!("Tab cycles: {}", cands.join("  "))
}

/// The parameters still to type, when the caret ends `text` inside a call:
/// `PMT(1, ` gives `nper, pv, [fv], [type])`, `PMT(1, 2` gives
/// `, pv, [fv], [type])`.
pub fn ghost(text: &str, caret: usize) -> Option<String> {
    if caret != text.len() {
        return None;
    }
    let (def, arg) = functions::call_at(text, caret)?;
    let names = def.param_names();
    let typing = !text.trim_end().ends_with(['(', ',', ';']);
    let rest = names.get(arg + usize::from(typing)..).unwrap_or_default();
    let mut out = rest.join(", ");
    if typing && !rest.is_empty() {
        out.insert_str(0, ", ");
    }
    out.push(')');
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::revenue_model;

    fn tab_in(model: &Model, text: &str, cycle: &mut Option<Cycle>) -> (String, Option<String>) {
        let mut e = LineEdit::from(text);
        let msg = tab(model, &mut e, cycle);
        (e.text, msg)
    }

    #[test]
    fn word_before_the_caret() {
        assert_eq!(word_start("Price * Qua", 11), 8);
        assert_eq!(word_start("FLOOR.MA", 8), 0);
        assert_eq!(word_start("1 + ", 4), 4);
        assert_eq!(word_start("é", 2), 0);
    }

    #[test]
    fn candidates_are_model_names_then_functions() {
        let m = revenue_model();
        let c = candidates(&m, "pr");
        assert_eq!(&c[..2], ["Price", "Product"]);
        assert!(c.contains(&"PRODUCT(".to_string()));
        assert!(candidates(&m, "").is_empty());
        assert_eq!(source_name("Unit Price"), "'Unit Price'");
        assert_eq!(source_name("it's"), "'it''s'");
        assert_eq!(source_name("Over"), "'Over'");
    }

    #[test]
    fn tab_completes_extends_and_cycles() {
        let m = revenue_model();
        let mut cy = None;
        // One match: a function completes with its parenthesis.
        assert_eq!(tab_in(&m, "1 + pm", &mut cy), ("1 + PMT(".into(), None));
        assert_eq!(tab_in(&m, "Price * Quan", &mut cy).0, "Price * Quantity");
        assert!(cy.is_none());
        // Nothing to complete, or nothing matches.
        assert_eq!(tab_in(&m, "1 + ", &mut cy), ("1 + ".into(), None));
        let (t, msg) = tab_in(&m, "zzq", &mut cy);
        assert_eq!(
            (t.as_str(), msg.unwrap()),
            ("zzq", "no completions for 'zzq'".into())
        );
        // Several: extend to the common prefix and list them.
        let mut e = LineEdit::from("sumx");
        let msg = tab(&m, &mut e, &mut cy).unwrap();
        assert_eq!(e.text, "SUMX");
        assert!(
            msg.contains("SUMX2MY2(") && msg.contains("SUMX2PY2("),
            "{msg}"
        );
        // Further Tabs cycle through them (shortest first), wrapping.
        let mut seen = Vec::new();
        for _ in 0..4 {
            tab(&m, &mut e, &mut cy);
            seen.push(e.text.clone());
        }
        assert_eq!(seen, ["SUMXMY2(", "SUMX2MY2(", "SUMX2PY2(", "SUMXMY2("]);
        // The common prefix is case-insensitive: "pr" matches Price and PROB(.
        let mut cy = None;
        assert_eq!(tab_in(&m, "pr", &mut cy).0, "Pr");
        assert_eq!(common_prefix(&["PMT(".into(), "pmtx".into()]), "PMT");
        assert_eq!(common_prefix(&["ab".into(), "abc".into()]), "ab");
    }

    #[test]
    fn ghost_text_is_the_parameters_left() {
        assert_eq!(ghost("PMT(", 4).unwrap(), "rate, nper, pv, [fv], [type])");
        assert_eq!(ghost("PMT(1, ", 7).unwrap(), "nper, pv, [fv], [type])");
        assert_eq!(ghost("PMT(1, 2", 8).unwrap(), ", pv, [fv], [type])");
        assert_eq!(ghost("PMT(1,2,3,4,5", 13).unwrap(), ")");
        assert_eq!(ghost("PI(", 3).unwrap(), ")");
        assert_eq!(ghost("PMT(1", 2), None, "caret not at the end");
        assert_eq!(ghost("Price", 5), None, "not in a call");
    }
}
