//! Formula autocomplete, window-free: the word being typed before the
//! caret, what may complete it (functions, measures, categories), the ghost
//! text painted after the caret, and the edit that accepting a candidate
//! makes. The formula editors draw the list, the ghost and the signature
//! tooltip from these.

use improv_core_model::functions::{self, FnDef};
use improv_core_model::Model;

/// At most this many candidates are listed.
pub const MAX_ITEMS: usize = 8;

/// What a candidate names.
#[derive(Debug, Clone, Copy)]
pub enum Kind {
    Function(&'static FnDef),
    Measure,
    Category,
    /// A named argument of the call the caret is in (`match_mode => `).
    Param,
}

/// One completion: the name shown, and what accepting it inserts.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// `PMT`, or the model's own spelling (`Unit Cost`).
    pub name: String,
    /// The source text: `PMT(`, `Price`, `'Unit Cost'`.
    pub insert: String,
    pub kind: Kind,
}

impl Candidate {
    /// A function by `name` (its canonical name or an alias).
    pub fn function(name: &str, def: &'static FnDef) -> Self {
        Candidate {
            name: name.to_string(),
            insert: format!("{name}("),
            kind: Kind::Function(def),
        }
    }

    /// `PMT(rate, nper, pv, [fv=0], [type=0])`, or a model name as it is.
    pub fn hint(&self) -> String {
        match self.kind {
            Kind::Function(d) => call_hint(&self.name, d),
            _ => self.name.clone(),
        }
    }

    /// The one-line description, or what a model name names.
    pub fn doc(&self) -> &'static str {
        match self.kind {
            Kind::Function(d) => short_doc(d),
            Kind::Measure => "measure",
            Kind::Category => "category",
            Kind::Param => "named argument",
        }
    }
}

/// `d`'s description without a `NAME(…)` call that spells its parameters
/// (`payment per period: PMT(rate, nper, …)`), which a hint already shows.
pub fn short_doc(d: &FnDef) -> &'static str {
    let doc = d.doc;
    let call = format!("{}(", d.name);
    match doc.find(&call) {
        Some(at) if doc[at..].ends_with(')') => doc[..at].trim_end_matches([' ', ':', ',']),
        _ => doc,
    }
}

/// `NAME(params)` under the name the user typed (an alias keeps its own).
fn call_hint(name: &str, d: &FnDef) -> String {
    format!("{name}({})", d.param_names().join(", "))
}

fn ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '.'
}

/// Whether byte `i` of `text` is inside a `"string"`.
fn in_string(text: &str, i: usize) -> bool {
    text[..i].matches('"').count() % 2 == 1
}

/// Byte offset where the word ending at `caret` starts: the identifier
/// run before it, or an open `'quoted name` (spaces and all).
pub fn word_start(text: &str, caret: usize) -> usize {
    let before = &text[..caret];
    if before.matches('\'').count() % 2 == 1 {
        if let Some(q) = before.rfind('\'') {
            return q;
        }
    }
    before
        .char_indices()
        .rev()
        .take_while(|&(_, c)| ident_char(c))
        .last()
        .map_or(caret, |(i, _)| i)
}

/// The word being typed before `caret`, if it can be completed: not
/// empty, starting with a letter, `_` or `'`, and not in a string.
pub fn prefix(text: &str, caret: usize) -> Option<(usize, &str)> {
    if !text.is_char_boundary(caret) {
        return None;
    }
    let start = word_start(text, caret);
    let word = &text[start..caret];
    let first = word.chars().next()?;
    let name = first.is_alphabetic() || first == '_' || first == '\'';
    (name && !in_string(text, start)).then_some((start, word))
}

/// What may complete `word`, case-insensitively: the model's measures,
/// then its categories (each sorted), then the functions, shortest first.
/// A `'quoted` word completes model names only.
pub fn candidates(model: &Model, word: &str) -> Vec<Candidate> {
    let quoted = word.strip_prefix('\'');
    let p = quoted.unwrap_or(word).to_lowercase();
    if p.is_empty() && quoted.is_none() {
        return Vec::new();
    }
    let named = |names: Vec<&str>, kind: Kind| {
        let mut names: Vec<&str> = names
            .into_iter()
            .filter(|n| n.to_lowercase().starts_with(&p))
            .collect();
        names.sort_unstable();
        names.dedup();
        names
            .into_iter()
            .filter_map(move |n| {
                Some(Candidate {
                    name: n.to_string(),
                    insert: crate::app::ident(n)?,
                    kind,
                })
            })
            .collect::<Vec<_>>()
    };
    let mut out = named(
        model.measures.values().map(|m| m.name.0.as_str()).collect(),
        Kind::Measure,
    );
    out.extend(named(
        model
            .categories
            .values()
            .map(|c| c.name.0.as_str())
            .collect(),
        Kind::Category,
    ));
    if quoted.is_none() {
        out.extend(
            functions::complete(word)
                .into_iter()
                .map(|(n, d)| Candidate::function(n, d)),
        );
    }
    out
}

/// The list to offer while typing at `caret`: [`candidates`] for the word
/// there, at most [`MAX_ITEMS`]. Empty once the word is exactly a measure
/// or category name, so Enter after `Price * Quantity` commits, and when the
/// caret is inside a word or right before a `(` (the call is written).
pub fn offer(model: &Model, text: &str, caret: usize) -> Vec<Candidate> {
    let Some((_, word)) = prefix(text, caret) else {
        return Vec::new();
    };
    if text[caret..]
        .chars()
        .next()
        .is_some_and(|c| c == '(' || ident_char(c))
    {
        return Vec::new();
    }
    let start = caret - word.len();
    let mut c: Vec<Candidate> = functions::complete_named(text, start, word)
        .into_iter()
        .map(|n| Candidate {
            name: n.clone(),
            insert: n,
            kind: Kind::Param,
        })
        .collect();
    c.extend(candidates(model, word));
    let whole = c
        .iter()
        .any(|c| !matches!(c.kind, Kind::Function(_)) && c.insert == word);
    if whole {
        return Vec::new();
    }
    c.truncate(MAX_ITEMS);
    c
}

/// The ghost text after the caret, which must end `text`. While
/// completing (`cand` is the selected candidate for the word there) it is
/// the rest of that name, and a function's `(params)`; else, inside a call,
/// the parameters from the current argument on: after `PMT(0.05, ` it is
/// `nper, pv, [fv=0], [type=0])`, after `PMT(0.05` it is `, nper, …)`.
pub fn ghost(text: &str, caret: usize, cand: Option<&Candidate>) -> Option<String> {
    if caret != text.len() {
        return None;
    }
    if let (Some(c), Some((_, word))) = (cand, prefix(text, caret)) {
        let full = match c.kind {
            Kind::Function(d) => call_hint(&c.name, d),
            _ => c.insert.clone(),
        };
        let head = full.get(..word.len())?;
        return head
            .eq_ignore_ascii_case(word)
            .then(|| full[word.len()..].to_string())
            .filter(|g| !g.is_empty());
    }
    let (def, arg) = functions::call_at(text, caret)?;
    let names = def.param_names();
    let typing = !text.trim_end().ends_with(['(', ',', ';']);
    let start = clamp_variadic(def, &names, arg + usize::from(typing));
    let rest = names.get(start..).unwrap_or_default();
    let mut out = rest.join(", ");
    if typing && !rest.is_empty() {
        out.insert_str(0, ", ");
    } else if text.ends_with([',', ';']) {
        out.insert(0, ' ');
    }
    out.push(')');
    Some(out)
}

/// Argument `i` of `def` as an index into its `names`: a variadic tail
/// repeats, so an index past it lands on the `…` (or the last name).
fn clamp_variadic(def: &FnDef, names: &[String], i: usize) -> usize {
    if !def.variadic || names.is_empty() {
        return i;
    }
    let tail = names
        .iter()
        .position(|n| n == "…")
        .unwrap_or(names.len() - 1);
    i.min(tail)
}

/// Replace the word at `caret` (through the end of the identifier the
/// caret is in) with `cand`: the new text and the caret after it. A
/// function lands as `NAME(` with the caret after the `(`, reusing a `(`
/// that already follows.
pub fn accept(text: &str, caret: usize, cand: &Candidate) -> (String, usize) {
    let start = word_start(text, caret);
    let end = caret
        + text[caret..]
            .char_indices()
            .find(|&(_, c)| !ident_char(c))
            .map_or(text.len() - caret, |(i, _)| i);
    let after = &text[end..];
    let reuse = matches!(cand.kind, Kind::Function(_)) && after.starts_with('(');
    let insert = if reuse { &cand.name } else { &cand.insert };
    let out = format!("{}{insert}{after}", &text[..start]);
    // Past a reused `(` too.
    (out, start + insert.len() + usize::from(reuse))
}

/// One editor's popup: the selected candidate, and whether Esc closed it
/// (it stays closed until the text changes).
#[derive(Debug, Clone, Default)]
pub struct Popup {
    /// The word the selection belongs to; a new word selects its first.
    word: String,
    pub sel: usize,
    /// The text Esc was pressed on.
    dismissed: Option<String>,
}

impl Popup {
    /// Follow the editor: `text`, the `word` at its caret, and `n` candidates
    /// for it. Returns whether the list shows.
    pub fn sync(&mut self, text: &str, word: &str, n: usize) -> bool {
        if self.word != word {
            word.clone_into(&mut self.word);
            self.sel = 0;
        }
        if self.dismissed.as_deref().is_some_and(|d| d != text) {
            self.dismissed = None;
        }
        self.sel = self.sel.min(n.saturating_sub(1));
        n > 0 && self.dismissed.is_none()
    }

    /// Esc: close it until `text` changes.
    pub fn dismiss(&mut self, text: &str) {
        self.dismissed = Some(text.to_string());
    }

    /// Up (`-1`) or Down (`1`) through `n` candidates, wrapping.
    pub fn step(&mut self, delta: isize, n: usize) {
        if n > 0 {
            self.sel = (self.sel as isize + delta).rem_euclid(n as isize) as usize;
        }
    }
}

/// `def`'s hint split around argument `arg`: (before, current, after), so
/// the current parameter can be drawn bold. No `arg` (or one past a
/// fixed parameter list) leaves `current` empty.
pub fn hint_parts(name: &str, def: &FnDef, arg: Option<usize>) -> (String, String, String) {
    let names = def.param_names();
    let Some(i) = arg
        .map(|a| clamp_variadic(def, &names, a))
        .filter(|&i| i < names.len())
    else {
        return (call_hint(name, def), String::new(), String::new());
    };
    let mut before = format!("{name}(");
    for n in &names[..i] {
        before.push_str(n);
        before.push_str(", ");
    }
    let mut after: String = names[i + 1..].iter().map(|n| format!(", {n}")).collect();
    after.push(')');
    (before, names[i].clone(), after)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::{CategoryId, Measure, MeasureId, MeasureKind, Name, ValueType};

    fn model() -> Model {
        let mut m = Model::new();
        m.add_category(CategoryId(1), "Time");
        m.add_category(CategoryId(2), "Product");
        for (id, name) in [(100, "Price"), (101, "Quantity"), (102, "Unit Cost")] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![CategoryId(1)],
                kind: MeasureKind::Input,
                description: None,
            });
        }
        m
    }

    fn names(c: &[Candidate]) -> Vec<&str> {
        c.iter().map(|c| c.insert.as_str()).collect()
    }

    fn func(name: &str) -> Candidate {
        let d = functions::by_name(name).next().expect(name);
        Candidate::function(name, d)
    }

    #[test]
    fn the_word_before_the_caret() {
        assert_eq!(prefix("Price * Qua", 11), Some((8, "Qua")));
        assert_eq!(prefix("FLOOR.MA", 8), Some((0, "FLOOR.MA")));
        assert_eq!(prefix("SUM(Pr", 6), Some((4, "Pr")));
        assert_eq!(prefix("1 + ", 4), None, "no word");
        assert_eq!(prefix("10", 2), None, "a number is no name");
        assert_eq!(prefix("\"Pri", 4), None, "inside a string");
        assert_eq!(prefix("'Unit Co", 8), Some((0, "'Unit Co")));
        assert_eq!(prefix("'Unit Cost' * Pr", 16), Some((14, "Pr")));
        assert_eq!(prefix("é", 2), Some((0, "é")));
        assert_eq!(prefix("é", 1), None, "not a char boundary");
        assert_eq!(word_start("Price", 0), 0);
    }

    #[test]
    fn candidates_are_measures_categories_then_functions() {
        let m = model();
        let c = candidates(&m, "pr");
        assert_eq!(&names(&c)[..2], ["Price", "Product"]);
        assert!(matches!(
            (c[0].kind, c[1].kind),
            (Kind::Measure, Kind::Category)
        ));
        assert!(names(&c).contains(&"PRODUCT("), "{:?}", names(&c));
        assert_eq!(names(&candidates(&m, "PM")), ["PMT("]);
        assert_eq!(names(&candidates(&m, "pm")), ["PMT("], "case-insensitive");
        assert_eq!(
            names(&candidates(&m, "un"))[0],
            "'Unit Cost'",
            "quoted to insert"
        );
        assert_eq!(names(&candidates(&m, "'unit")), ["'Unit Cost'"]);
        assert_eq!(names(&candidates(&m, "'")).len(), 5, "model names only");
        assert!(candidates(&m, "").is_empty());
        assert!(candidates(&m, "zzzz").is_empty());
        assert_eq!((c[0].hint().as_str(), c[0].doc()), ("Price", "measure"));
        assert_eq!(c[1].doc(), "category");
        assert!(
            func("PMT").doc().contains("payment"),
            "{}",
            func("PMT").doc()
        );
    }

    #[test]
    fn the_offer_is_capped_and_closes_on_a_whole_name() {
        let m = model();
        assert_eq!(offer(&m, "S", 1).len(), MAX_ITEMS);
        assert!(offer(&m, "Price", 5).is_empty(), "a whole measure name");
        assert!(offer(&m, "Time", 4).is_empty(), "a whole category name");
        assert!(!offer(&m, "time", 4).is_empty(), "names are case-sensitive");
        assert!(!offer(&m, "PMT", 3).is_empty(), "a function without (");
        assert!(offer(&m, "1 + ", 4).is_empty());
        assert!(offer(&m, "PM(1)", 2).is_empty(), "the call is written");
        assert!(offer(&m, "Prxx", 2).is_empty(), "inside a word");
        assert_eq!(
            offer(&m, "Pr * 2", 2)[0].insert,
            "Price",
            "before an operator"
        );
    }

    /// Where an argument starts in a call, the call's named arguments are
    /// offered first, from the current parameter on.
    #[test]
    fn named_arguments_are_offered_in_a_call() {
        let m = model();
        let o = |t: &str| names(&offer(&m, t, t.len())).join(" ");
        assert!(
            o("PMT(1, 2, 3, t").starts_with("type =>  Time"),
            "{}",
            o("PMT(1, 2, 3, t")
        );
        assert!(
            o("ROUND(Price, n").starts_with("num_digits => "),
            "{}",
            o("ROUND(Price, n")
        );
        assert!(o("ROUND(nu").starts_with("number => "), "{}", o("ROUND(nu"));
        assert!(!o("SUM(n").contains("=>"), "variadic: by position");
        assert!(!o("1 + t").contains("=>"), "not in a call");
        let c = &offer(&m, "PMT(1, 2, 3, t", 14)[0];
        assert_eq!((c.hint().as_str(), c.doc()), ("type => ", "named argument"));
        assert_eq!(
            accept("PMT(1, 2, 3, t", 14, c),
            ("PMT(1, 2, 3, type => ".to_string(), 21)
        );
        // Inside the named argument, the signature marks its parameter.
        let g = |t: &str| ghost(t, t.len(), None);
        assert_eq!(g("PMT(1, 2, 3, type => 1, ").as_deref(), Some(")"));
        assert_eq!(g("PMT(1, 2, 3, fv => ").as_deref(), Some(", [type=0])"));
    }

    #[test]
    fn ghost_while_completing() {
        let m = model();
        let top = |t: &str| offer(&m, t, t.len()).into_iter().next();
        let g = |t: &str| ghost(t, t.len(), top(t).as_ref());
        assert_eq!(
            g("PM").as_deref(),
            Some("T(rate, nper, pv, [fv=0], [type=0])")
        );
        assert_eq!(
            g("pm").as_deref(),
            Some("T(rate, nper, pv, [fv=0], [type=0])")
        );
        assert_eq!(g("Pri").as_deref(), Some("ce"));
        assert_eq!(g("1 + Qu").as_deref(), Some("antity"));
        assert_eq!(g("'Unit").as_deref(), Some(" Cost'"));
        // The quoted insert does not extend a bare prefix: no ghost.
        assert_eq!(g("Uni"), None);
        assert_eq!(g("PI").as_deref(), Some("()"));
        // Not at the end of the text: nothing.
        assert_eq!(ghost("PM + 1", 2, top("PM").as_ref()), None);
    }

    #[test]
    fn ghost_inside_a_call() {
        let g = |t: &str| ghost(t, t.len(), None);
        assert_eq!(
            g("PMT(").as_deref(),
            Some("rate, nper, pv, [fv=0], [type=0])")
        );
        assert_eq!(
            g("PMT(0.05, ").as_deref(),
            Some("nper, pv, [fv=0], [type=0])")
        );
        assert_eq!(
            g("PMT(0.05,").as_deref(),
            Some(" nper, pv, [fv=0], [type=0])")
        );
        assert_eq!(
            g("PMT(0.05").as_deref(),
            Some(", nper, pv, [fv=0], [type=0])")
        );
        // Into the optional ones, then past them all.
        assert_eq!(g("PMT(1, 2, 3, ").as_deref(), Some("[fv=0], [type=0])"));
        assert_eq!(g("PMT(1, 2, 3, 4, 5").as_deref(), Some(")"));
        // A variadic tail repeats.
        assert_eq!(g("CONCAT(").as_deref(), Some("text1, …)"));
        assert_eq!(g("CONCAT(a, b, ").as_deref(), Some("…)"));
        assert_eq!(g("CONCAT(a, b").as_deref(), Some(", …)"));
        // The innermost call; none outside one.
        assert_eq!(g("PMT(ABS(").as_deref(), Some("number)"));
        assert_eq!(
            g("PMT(ABS(1), ").as_deref(),
            Some("nper, pv, [fv=0], [type=0])")
        );
        assert_eq!(g("Price + 1"), None);
        assert_eq!(ghost("PMT(", 2, None), None, "caret not at the end");
    }

    #[test]
    fn accepting_replaces_the_word_and_places_the_caret() {
        let m = model();
        assert_eq!(accept("PM", 2, &func("PMT")), ("PMT(".into(), 4));
        assert_eq!(
            accept("1 + pm", 6, &func("PMT")),
            ("1 + PMT(".into(), 8),
            "the typed case is replaced"
        );
        // A `(` already there is reused.
        assert_eq!(accept("PM(1, 2)", 2, &func("PMT")), ("PMT(1, 2)".into(), 4));
        // The whole word under the caret goes.
        let price = &candidates(&m, "Pr")[0];
        assert_eq!(accept("Pr * 2", 2, price), ("Price * 2".into(), 5));
        assert_eq!(accept("Prxx * 2", 2, price), ("Price * 2".into(), 5));
        let unit = &candidates(&m, "'un")[0];
        assert_eq!(accept("1 + 'un", 7, unit), ("1 + 'Unit Cost'".into(), 15));
        assert_eq!(accept("1 + Un", 6, unit), ("1 + 'Unit Cost'".into(), 15));
    }

    #[test]
    fn short_docs_drop_a_spelled_call() {
        let d = |n: &str| functions::by_name(n).next().expect(n);
        assert_eq!(short_doc(d("PMT")), "payment per period");
        assert_eq!(short_doc(d("ABS")), d("ABS").doc, "nothing to drop");
    }

    #[test]
    fn the_popup_selects_wraps_and_stays_closed_until_an_edit() {
        let mut p = Popup::default();
        assert!(p.sync("PM", "PM", 3));
        p.step(1, 3);
        p.step(1, 3);
        assert_eq!(p.sel, 2);
        p.step(1, 3);
        assert_eq!(p.sel, 0, "wraps");
        p.step(-1, 3);
        assert_eq!(p.sel, 2);
        assert!(p.sync("PM", "PM", 2) && p.sel == 1, "clamped to the list");
        assert!(
            p.sync("PMT", "PMT", 2) && p.sel == 0,
            "a new word starts over"
        );
        p.dismiss("PMT");
        assert!(!p.sync("PMT", "PMT", 2), "Esc closed it");
        assert!(!p.sync("PMT ", "", 0), "nothing for a finished word");
        assert!(p.sync("PMTx", "PMTx", 1), "an edit reopens it");
        assert!(!p.sync("1", "", 0), "nothing to offer");
    }

    #[test]
    fn hint_parts_mark_the_current_argument() {
        let d = |n: &str| functions::by_name(n).next().expect(n);
        let parts = |n: &str, a| {
            let (b, c, e) = hint_parts(n, d(n), a);
            format!("{b}<{c}>{e}")
        };
        assert_eq!(
            parts("PMT", Some(0)),
            "PMT(<rate>, nper, pv, [fv=0], [type=0])"
        );
        assert_eq!(
            parts("PMT", Some(3)),
            "PMT(rate, nper, pv, <[fv=0]>, [type=0])"
        );
        assert_eq!(
            parts("PMT", Some(9)),
            "PMT(rate, nper, pv, [fv=0], [type=0])<>"
        );
        assert_eq!(
            parts("PMT", None),
            "PMT(rate, nper, pv, [fv=0], [type=0])<>"
        );
        assert_eq!(parts("CONCAT", Some(4)), "CONCAT(text1, <…>)");
        assert_eq!(parts("PI", Some(0)), "PI()<>");
    }
}
