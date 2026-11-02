//! The function help browser (Help ▸ Functions…, F1), window-free: the
//! search, its results grouped by category, and the selection in them. The
//! app draws the window from a [`Browser`].

use improv_core_model::functions::{self, Category, FnDef};

/// The reference's title for a category.
pub fn category_title(c: Category) -> &'static str {
    match c {
        Category::Aggregation => "Aggregations",
        Category::Logical => "Logical",
        Category::Information => "Information and errors",
        Category::Math => "Math and trigonometry",
        Category::Rounding => "Rounding",
        Category::Text => "Text",
        Category::DateTime => "Date and time",
        Category::Statistical => "Statistical",
        Category::Financial => "Financial",
        Category::Dimensional => "Dimensional",
        Category::Engineering => "Engineering",
    }
}

/// Functions in one category, in result order.
pub type Group = (Category, Vec<&'static FnDef>);

/// [`functions::search`]'s results for `query`, grouped by category. With
/// a query the groups come in order of their best match; without one, in
/// category order, each sorted by name.
pub fn groups(query: &str) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for d in functions::search(query) {
        match out.iter_mut().find(|(c, _)| *c == d.category) {
            Some((_, fs)) => fs.push(d),
            None => out.push((d.category, vec![d])),
        }
    }
    if query.trim().is_empty() {
        out.sort_by_key(|(c, _)| *c);
    }
    out
}

/// `also AVG, MEAN` for a function with aliases.
pub fn aliases(d: &FnDef) -> Option<String> {
    (!d.aliases.is_empty()).then(|| format!("also {}", d.aliases.join(", ")))
}

/// Insert a call of `name` into `text` at byte `caret`: `NAME(`, the caret
/// after the `(`. A word typed before the caret that starts the name
/// (`pm` for PMT) is replaced, and a `(` already after the caret is reused.
pub fn insert_call(text: &str, caret: usize, name: &str) -> (String, usize) {
    let caret = caret.min(text.len());
    let caret = (0..=caret)
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let word = text[..caret]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| c.is_alphanumeric() || c == '_' || c == '.')
        .last()
        .map_or(caret, |(i, _)| i);
    let typed = &text[word..caret];
    let start = if name.len() >= typed.len() && name[..typed.len()].eq_ignore_ascii_case(typed) {
        word
    } else {
        caret
    };
    let after = &text[caret..];
    let paren = if after.starts_with('(') { "" } else { "(" };
    let out = format!("{}{name}{paren}{after}", &text[..start]);
    (out, start + name.len() + 1)
}

/// The browser's state: the search line, its grouped results, and the
/// selected function.
#[derive(Default)]
pub struct Browser {
    pub open: bool,
    pub query: String,
    pub selected: Option<&'static FnDef>,
    /// Give the search line focus next frame (just opened).
    pub focus_search: bool,
    /// The results, and the query they are for.
    results: Option<(String, Vec<Group>)>,
}

impl Browser {
    /// Open it, or close it if open.
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.focus_search = self.open;
    }

    /// The results for the current query, recomputed when it changed; a
    /// selection the new results lack moves to their first function.
    pub fn groups(&mut self) -> &[Group] {
        if self.results.as_ref().is_none_or(|(q, _)| *q != self.query) {
            let g = groups(&self.query);
            let kept = self
                .selected
                .filter(|s| g.iter().any(|(_, fs)| fs.iter().any(|d| d.id == s.id)));
            self.selected = kept.or_else(|| g.first().map(|(_, fs)| fs[0]));
            self.results = Some((self.query.clone(), g));
        }
        &self.results.as_ref().expect("just set").1
    }

    /// Move the selection `delta` functions along the list (clamped).
    pub fn step(&mut self, delta: isize) {
        let sel = self.selected.map(|s| s.id);
        let all: Vec<&'static FnDef> = self
            .groups()
            .iter()
            .flat_map(|(_, fs)| fs.iter().copied())
            .collect();
        let Some(last) = all.len().checked_sub(1) else {
            return;
        };
        let at = all.iter().position(|d| Some(d.id) == sel).unwrap_or(0);
        let to = at.saturating_add_signed(delta).min(last);
        self.selected = Some(all[to]);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn names(g: &[Group]) -> Vec<(Category, Vec<&str>)> {
        g.iter()
            .map(|(c, fs)| (*c, fs.iter().map(|d| d.name).collect()))
            .collect()
    }

    #[test]
    fn results_are_grouped_by_category() {
        let all = groups("");
        let cats: Vec<Category> = all.iter().map(|(c, _)| *c).collect();
        let mut sorted = cats.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(cats, sorted, "every category once, in order");
        let n: usize = all.iter().map(|(_, fs)| fs.len()).sum();
        assert_eq!(n, functions::search("").len(), "every function once");
        for (c, fs) in &all {
            assert!(fs.iter().all(|d| d.category == *c));
            assert!(fs.windows(2).all(|w| w[0].name <= w[1].name), "{c:?}");
        }
    }

    #[test]
    fn a_query_filters_and_its_best_match_leads() {
        let g = groups("pmt");
        let (cat, fs) = &names(&g)[0];
        assert_eq!(*cat, Category::Financial);
        assert_eq!(fs[0], "PMT", "the exact name first");
        assert!(fs.contains(&"IPMT") && fs.contains(&"PPMT"), "{fs:?}");
        // An alias finds its function.
        let g = groups("avg");
        assert_eq!(g[0].1[0].name, "AVERAGE");
        // The description is searched too.
        assert!(groups("payment")
            .iter()
            .any(|(_, fs)| fs.iter().any(|d| d.name == "PMT")));
        assert!(groups("no such thing at all").is_empty());
    }

    #[test]
    fn the_browser_keeps_or_moves_its_selection() {
        let mut b = Browser::default();
        b.toggle();
        assert!(b.open && b.focus_search);
        b.query = "pmt".into();
        assert_eq!(b.groups()[0].1[0].name, "PMT");
        assert_eq!(b.selected.map(|d| d.name), Some("PMT"));
        let second = b.groups()[0].1[1].name;
        b.step(1);
        assert_eq!(b.selected.map(|d| d.name), Some(second));
        b.step(-5);
        assert_eq!(b.selected.map(|d| d.name), Some("PMT"), "clamped");
        b.step(1);
        // Still a result: kept. Gone: the first result.
        b.query = second.to_lowercase();
        let _ = b.groups();
        assert_eq!(b.selected.map(|d| d.name), Some(second));
        b.query = "abs".into();
        assert_eq!(b.groups()[0].1[0].name, "ABS");
        assert_eq!(b.selected.map(|d| d.name), Some("ABS"));
        b.query = "zzzzzzzz".into();
        assert!(b.groups().is_empty());
        assert!(b.selected.is_none());
        b.step(1);
        assert!(b.selected.is_none());
        b.toggle();
        assert!(!b.open && !b.focus_search);
    }

    #[test]
    fn inserting_a_call_at_the_caret() {
        assert_eq!(insert_call("", 0, "PMT"), ("PMT(".into(), 4));
        assert_eq!(insert_call("1 + ", 4, "PMT"), ("1 + PMT(".into(), 8));
        assert_eq!(
            insert_call("1 +  * 2", 4, "ABS"),
            ("1 + ABS( * 2".into(), 8)
        );
        // A typed start of the name is replaced; another word is kept.
        assert_eq!(insert_call("1 + pm", 6, "PMT"), ("1 + PMT(".into(), 8));
        assert_eq!(insert_call("Price", 5, "ABS"), ("PriceABS(".into(), 9));
        // A `(` already there is reused; the caret goes past it.
        assert_eq!(insert_call("PM(1)", 2, "PMT"), ("PMT(1)".into(), 4));
        // A caret past the end or inside a char is pulled back.
        assert_eq!(insert_call("é", 1, "PI"), ("PI(é".into(), 3));
        assert_eq!(insert_call("x", 99, "PI"), ("xPI(".into(), 4));
    }

    #[test]
    fn titles_and_aliases() {
        assert_eq!(category_title(Category::DateTime), "Date and time");
        let avg = functions::by_name("AVERAGE").find(|d| !d.aliases.is_empty());
        assert!(aliases(avg.expect("AVERAGE has an alias")).is_some_and(|a| a.starts_with("also ")));
        let pmt = functions::by_name("PMT").next().expect("PMT");
        assert_eq!(aliases(pmt), None);
    }
}
