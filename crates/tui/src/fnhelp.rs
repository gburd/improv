//! Function help overlay state (`?` / F1): a search line over the built-in
//! functions, the ranked results, and the selection in them. Pure state,
//! terminal-free; `ui` draws it and `main` feeds it keys.

use improv_core_model::functions::{self, FnDef};

/// The open help overlay.
pub struct FnHelp {
    /// The search line, filtered through [`functions::search`].
    pub query: String,
    /// Functions matching `query`, best first.
    pub results: Vec<&'static FnDef>,
    /// Index of the selected result.
    pub selected: usize,
    /// First visible result row; the renderer scrolls it to the selection.
    pub top: usize,
    /// Rows the result list showed last render; PgUp/PgDn move by this.
    pub page: usize,
}

impl FnHelp {
    /// An overlay listing every function, the first one selected.
    pub fn new() -> FnHelp {
        let mut h = FnHelp {
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            top: 0,
            page: 10,
        };
        h.refilter();
        h
    }

    /// Type a character into the search line.
    pub fn push(&mut self, c: char) {
        self.query.push(c);
        self.refilter();
    }

    /// Delete the last search character.
    pub fn pop(&mut self) {
        self.query.pop();
        self.refilter();
    }

    fn refilter(&mut self) {
        self.results = functions::search(&self.query);
        self.selected = 0;
        self.top = 0;
    }

    /// Move the selection by `delta` rows, clamped to the results.
    pub fn step(&mut self, delta: isize) {
        let max = self.results.len().saturating_sub(1) as isize;
        self.selected = (self.selected as isize + delta).clamp(0, max) as usize;
    }

    /// Move the selection a page up (`-1`) or down (`1`).
    pub fn page_by(&mut self, dir: isize) {
        self.step(dir * self.page.max(1) as isize);
    }

    /// The selected function, if any result matched.
    pub fn current(&self) -> Option<&'static FnDef> {
        self.results.get(self.selected).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_filters_and_ranks() {
        let mut h = FnHelp::new();
        assert!(h.results.len() > 100, "empty query lists everything");
        h.page = 5;
        h.page_by(1);
        assert_eq!(h.selected, 5);
        for c in "pmt".chars() {
            h.push(c);
        }
        assert_eq!(h.selected, 0, "typing resets the selection");
        assert_eq!(h.current().unwrap().name, "PMT", "exact name first");
        h.pop();
        assert_eq!(h.query, "pm");
        h.query.clear();
        h.push('z');
        h.push('z');
        h.push('z');
        h.push('q');
        assert!(h.results.is_empty());
        assert!(h.current().is_none());
        h.step(1);
        assert_eq!(h.selected, 0, "no results: nothing to move to");
    }

    #[test]
    fn selection_clamps_to_the_list() {
        let mut h = FnHelp::new();
        h.step(-3);
        assert_eq!(h.selected, 0);
        h.page = 0; // a zero-row render still pages by one
        h.page_by(1);
        assert_eq!(h.selected, 1);
        h.step(isize::MAX / 2);
        assert_eq!(h.selected, h.results.len() - 1);
    }
}
