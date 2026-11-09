//! A worksheet's pivot, without egui: which categories are on rows,
//! columns and pages, which page item is picked, and the cells that
//! layout puts on screen. Pure, so it is tested natively.

use std::collections::{BTreeMap, HashMap};

use improv_client::wire::{At, Structure, WindowReq, WireValue};

/// Where a category sits in a worksheet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Rows,
    Columns,
    Pages,
}

/// One worksheet: a measure and its own pivot.
#[derive(Debug, Clone, PartialEq)]
pub struct Sheet {
    pub measure: u32,
    /// A permutation of the measure's categories: rows, then columns,
    /// then pages.
    pub order: Vec<u32>,
    pub n_rows: usize,
    pub n_cols: usize,
    /// The picked item per page category (absent: its first).
    pub page: BTreeMap<u32, u32>,
}

impl Sheet {
    /// A sheet of `measure` in its natural order: its first category on
    /// rows, the second on columns, the rest pages.
    pub fn new(s: &Structure, measure: u32) -> Self {
        let order = s
            .measure(measure)
            .map(|m| m.categories.clone())
            .unwrap_or_default();
        let n_rows = order.len().min(1);
        let n_cols = (order.len() - n_rows).min(1);
        Sheet {
            measure,
            order,
            n_rows,
            n_cols,
            page: BTreeMap::new(),
        }
    }

    /// From a saved view's matrix (its order is kept only when it is a
    /// permutation of the measure's categories).
    pub fn from_placement(s: &Structure, p: &improv_core_model::MatrixPlacement) -> Self {
        let mut sheet = Sheet::new(s, p.measure.0);
        let order: Vec<u32> = p.axis_order.iter().map(|c| c.0).collect();
        let mut a = order.clone();
        let mut b = sheet.order.clone();
        a.sort_unstable();
        b.sort_unstable();
        if !order.is_empty() && a == b {
            sheet.order = order;
            sheet.n_rows = p.n_rows.min(sheet.order.len());
            sheet.n_cols = p.n_cols.min(sheet.order.len() - sheet.n_rows);
        }
        for (c, i) in &p.page_items {
            sheet.page.insert(c.0, i.0);
        }
        sheet
    }

    /// The sheet as a saved view's matrix.
    pub fn to_placement(&self) -> improv_core_model::MatrixPlacement {
        use improv_core_model::{CategoryId, ItemId, MatrixPlacement, MeasureId};
        let mut p = MatrixPlacement::new(MeasureId(self.measure));
        p.axis_order = self.order.iter().map(|c| CategoryId(*c)).collect();
        p.n_rows = self.n_rows;
        p.n_cols = self.n_cols;
        p.page_items = self
            .pages()
            .iter()
            .filter_map(|c| self.page.get(c).map(|i| (CategoryId(*c), ItemId(*i))))
            .collect();
        p
    }

    pub fn rows(&self) -> &[u32] {
        &self.order[..self.n_rows]
    }
    pub fn cols(&self) -> &[u32] {
        &self.order[self.n_rows..self.n_rows + self.n_cols]
    }
    pub fn pages(&self) -> &[u32] {
        &self.order[self.n_rows + self.n_cols..]
    }

    pub fn axis_of(&self, c: u32) -> Option<Axis> {
        let i = self.order.iter().position(|x| *x == c)?;
        Some(if i < self.n_rows {
            Axis::Rows
        } else if i < self.n_rows + self.n_cols {
            Axis::Columns
        } else {
            Axis::Pages
        })
    }

    /// Drop category `c` on `to`: it becomes that axis's innermost (last)
    /// category. Pivoting changes only the layout, never the model.
    pub fn move_to(&mut self, c: u32, to: Axis) {
        if self.axis_of(c).is_none() {
            return;
        }
        let (mut r, mut k, mut p): (Vec<u32>, Vec<u32>, Vec<u32>) =
            (self.rows().into(), self.cols().into(), self.pages().into());
        for v in [&mut r, &mut k, &mut p] {
            v.retain(|x| *x != c);
        }
        match to {
            Axis::Rows => r.push(c),
            Axis::Columns => k.push(c),
            Axis::Pages => p.push(c),
        }
        self.n_rows = r.len();
        self.n_cols = k.len();
        self.order = r.into_iter().chain(k).chain(p).collect();
    }

    /// The page item picked for page category `c`: the one chosen, while
    /// it is still visible, else the first visible item.
    pub fn page_item(&self, s: &Structure, c: u32) -> Option<u32> {
        let items = &s.category(c)?.items;
        self.page
            .get(&c)
            .filter(|i| items.iter().any(|x| x.id == **i))
            .copied()
            .or_else(|| items.first().map(|i| i.id))
    }

    /// The tuples down one axis: every combination of the axis's
    /// categories' visible items, outer category first. An axis with no
    /// category has one empty tuple.
    pub fn tuples(s: &Structure, cats: &[u32]) -> Vec<Vec<(u32, u32)>> {
        let mut out = vec![Vec::new()];
        for c in cats {
            let items: Vec<u32> = s
                .category(*c)
                .map(|k| k.items.iter().map(|i| i.id).collect())
                .unwrap_or_default();
            out = out
                .into_iter()
                .flat_map(|t| {
                    items.iter().map(move |i| {
                        let mut t = t.clone();
                        t.push((*c, *i));
                        t
                    })
                })
                .collect();
        }
        out
    }

    /// The window to ask the server for: this page, every row and column.
    pub fn window(&self, s: &Structure) -> WindowReq {
        let at = self
            .pages()
            .iter()
            .filter_map(|c| self.page_item(s, *c).map(|i| (*c, vec![i])))
            .collect();
        WindowReq { at }
    }

    /// The coordinate of the cell at `row` x `col` on this page.
    pub fn at(&self, s: &Structure, row: &[(u32, u32)], col: &[(u32, u32)]) -> At {
        let mut at: Vec<(u32, u32)> = row.iter().chain(col).copied().collect();
        at.extend(
            self.pages()
                .iter()
                .filter_map(|c| self.page_item(s, *c).map(|i| (*c, i))),
        );
        at.sort_unstable();
        at
    }
}

/// The cells a client has read, per measure: what each window returned,
/// and which page it covered (for Blank versus NotLoaded).
#[derive(Debug, Default)]
pub struct Cache {
    cells: HashMap<u32, HashMap<At, WireValue>>,
    /// Per measure, the windows read: their page items.
    loaded: HashMap<u32, Vec<BTreeMap<u32, Vec<u32>>>>,
}

impl Cache {
    /// Take in a window's answer.
    pub fn fill(&mut self, measure: u32, w: &WindowReq, cells: Vec<(At, WireValue)>) {
        let m = self.cells.entry(measure).or_default();
        // The window's old cells go first: a cell absent now is blank.
        m.retain(|at, _| !covers(&w.at, at));
        m.extend(cells);
        let l = self.loaded.entry(measure).or_default();
        if !l.contains(&w.at) {
            l.push(w.at.clone());
        }
    }

    /// Set or clear one cell (a change event, or an edit made here).
    pub fn set(&mut self, measure: u32, at: At, v: Option<WireValue>) {
        let m = self.cells.entry(measure).or_default();
        match v {
            Some(v) => drop(m.insert(at, v)),
            None => drop(m.remove(&at)),
        }
    }

    /// What is known of a cell.
    pub fn get(&self, measure: u32, at: &At) -> improv_client::wire::Cell<'_> {
        use improv_client::wire::Cell;
        if let Some(v) = self.cells.get(&measure).and_then(|m| m.get(at)) {
            return Cell::Value(v);
        }
        let read = self
            .loaded
            .get(&measure)
            .is_some_and(|ws| ws.iter().any(|w| covers(w, at)));
        if read {
            Cell::Blank
        } else {
            Cell::NotLoaded
        }
    }

    /// Whether `measure`'s window `w` has been read.
    pub fn has(&self, measure: u32, w: &WindowReq) -> bool {
        self.loaded.get(&measure).is_some_and(|l| l.contains(&w.at))
    }

    /// Forget everything (a reload).
    pub fn clear(&mut self) {
        self.cells.clear();
        self.loaded.clear();
    }
}

/// The keyboard cursor in a grid of `rows` x `cols` cells: where `key`
/// moves it from `at` (row, column), or `None` when the key does not move
/// it. Home and End go to the row's ends; with `ctrl`, to the grid's
/// corners. A cursor left outside a smaller grid is brought back in.
pub fn step(
    at: (usize, usize),
    key: Step,
    ctrl: bool,
    rows: usize,
    cols: usize,
) -> Option<(usize, usize)> {
    if rows == 0 || cols == 0 {
        return None;
    }
    let (r, c) = (at.0.min(rows - 1), at.1.min(cols - 1));
    Some(match key {
        Step::Up => (r.saturating_sub(1), c),
        Step::Down => ((r + 1).min(rows - 1), c),
        Step::Left => (r, c.saturating_sub(1)),
        Step::Right => (r, (c + 1).min(cols - 1)),
        Step::Home if ctrl => (0, 0),
        Step::End if ctrl => (rows - 1, cols - 1),
        Step::Home => (r, 0),
        Step::End => (r, cols - 1),
    })
}

/// A key that moves a grid's cursor (egui-free, so [`step`] tests natively).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

/// What a screen reader hears for a cell: its row and column headers, then
/// its value (as on the desktop, H2), or "empty".
pub fn cell_name(row: &str, col: &str, text: &str) -> String {
    let place: Vec<&str> = [row, col].into_iter().filter(|s| !s.is_empty()).collect();
    let v = if text.is_empty() { "empty" } else { text };
    match place.is_empty() {
        true => v.to_owned(),
        false => format!("{}: {v}", place.join(", ")),
    }
}

/// Whether a window (items per category; a category left out: any) holds
/// the cell at `at`.
fn covers(w: &BTreeMap<u32, Vec<u32>>, at: &At) -> bool {
    w.iter()
        .all(|(c, items)| at.iter().any(|(k, i)| k == c && items.contains(i)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_client::wire::{CategoryOut, Cell, ItemOut, MeasureOut};
    use improv_core_model::ValueType;

    fn structure() -> Structure {
        let cat = |id, name: &str, items: &[(u32, &str)]| CategoryOut {
            id,
            name: name.into(),
            items: items
                .iter()
                .map(|(i, n)| ItemOut {
                    id: *i,
                    name: (*n).into(),
                })
                .collect(),
        };
        Structure {
            version: "1".into(),
            categories: vec![
                cat(1, "Time", &[(10, "2025"), (11, "2026")]),
                cat(2, "Product", &[(20, "A"), (21, "B")]),
                cat(3, "Region", &[(30, "N"), (31, "S")]),
            ],
            measures: vec![MeasureOut {
                id: 100,
                name: "Sales".into(),
                value_type: ValueType::Number,
                kind: "input".into(),
                categories: vec![1, 2, 3],
                formula: None,
                error: None,
                format: None,
                editable: true,
            }],
            lambdas: vec![],
            views: vec![],
        }
    }

    #[test]
    fn a_sheet_pivots_by_moving_categories() {
        let s = structure();
        let mut sh = Sheet::new(&s, 100);
        assert_eq!(
            (sh.rows(), sh.cols(), sh.pages()),
            (&[1][..], &[2][..], &[3][..])
        );
        sh.move_to(3, Axis::Rows);
        assert_eq!(
            (sh.rows(), sh.cols(), sh.pages()),
            (&[1, 3][..], &[2][..], &[][..])
        );
        assert_eq!(Sheet::tuples(&s, sh.rows()).len(), 4);
        sh.move_to(1, Axis::Pages);
        assert_eq!(
            (sh.rows(), sh.cols(), sh.pages()),
            (&[3][..], &[2][..], &[1][..])
        );
        assert_eq!(sh.axis_of(1), Some(Axis::Pages));
        sh.move_to(9, Axis::Rows);
        assert_eq!(sh.order.len(), 3, "an unknown category is ignored");
        // The page: its first item, or the one picked.
        assert_eq!(sh.page_item(&s, 1), Some(10));
        sh.page.insert(1, 11);
        assert_eq!(sh.page_item(&s, 1), Some(11));
        sh.page.insert(1, 99);
        assert_eq!(sh.page_item(&s, 1), Some(10), "a vanished pick falls back");
        sh.page.insert(1, 11);
        assert_eq!(sh.window(&s).at, [(1, vec![11])].into());
        assert_eq!(
            sh.at(&s, &[(3, 30)], &[(2, 21)]),
            vec![(1, 11), (2, 21), (3, 30)]
        );
        // Through a saved view and back.
        let p = sh.to_placement();
        assert_eq!(Sheet::from_placement(&s, &p), sh);
        let mut bad = p.clone();
        bad.axis_order.pop();
        assert_eq!(Sheet::from_placement(&s, &bad).order, vec![1, 2, 3]);
        assert_eq!(Sheet::tuples(&s, &[]), vec![Vec::new()]);
    }

    #[test]
    fn a_category_moves_onto_and_off_the_column_axis() {
        let s = structure();
        let mut sh = Sheet::new(&s, 100);
        assert_eq!(sh.axis_of(2), Some(Axis::Columns));
        assert_eq!(sh.axis_of(9), None);
        sh.move_to(3, Axis::Columns);
        assert_eq!(
            (sh.rows(), sh.cols(), sh.pages()),
            (&[1][..], &[2, 3][..], &[][..])
        );
        assert_eq!(sh.axis_of(3), Some(Axis::Columns));
        assert_eq!(Sheet::tuples(&s, sh.cols()).len(), 4);
        // The cell's coordinate is sorted, with no page to add.
        assert_eq!(
            sh.at(&s, &[(1, 11)], &[(2, 20), (3, 31)]),
            vec![(1, 11), (2, 20), (3, 31)]
        );
        assert!(sh.window(&s).at.is_empty());
        sh.move_to(2, Axis::Rows);
        assert_eq!(
            (sh.rows(), sh.cols(), sh.pages()),
            (&[1, 2][..], &[3][..], &[][..])
        );
        // A saved view keeps the columns; an empty saved order is the
        // measure's natural one.
        let p = sh.to_placement();
        assert_eq!((p.n_rows, p.n_cols), (2, 1));
        assert_eq!(Sheet::from_placement(&s, &p), sh);
        let mut empty = p.clone();
        empty.axis_order.clear();
        assert_eq!(Sheet::from_placement(&s, &empty), Sheet::new(&s, 100));
    }

    #[test]
    fn the_cursor_moves_and_stays_in_the_grid() {
        use Step::*;
        assert_eq!(step((0, 0), Down, false, 3, 2), Some((1, 0)));
        assert_eq!(step((0, 0), Up, false, 3, 2), Some((0, 0)), "the top edge");
        assert_eq!(step((2, 1), Down, false, 3, 2), Some((2, 1)), "the bottom");
        assert_eq!(step((1, 0), Right, false, 3, 2), Some((1, 1)));
        assert_eq!(step((1, 1), Right, false, 3, 2), Some((1, 1)));
        assert_eq!(step((1, 1), Left, false, 3, 2), Some((1, 0)));
        assert_eq!(step((1, 0), Left, false, 3, 2), Some((1, 0)));
        assert_eq!(step((1, 0), End, false, 3, 2), Some((1, 1)));
        assert_eq!(step((1, 1), Home, false, 3, 2), Some((1, 0)));
        assert_eq!(step((1, 1), Home, true, 3, 2), Some((0, 0)));
        assert_eq!(step((0, 0), End, true, 3, 2), Some((2, 1)));
        // A pivot shrank the grid: the cursor comes back inside.
        assert_eq!(step((9, 9), Up, false, 3, 2), Some((1, 1)));
        assert_eq!(step((0, 0), Down, false, 0, 2), None, "no rows");
        assert_eq!(step((0, 0), Down, false, 2, 0), None, "no columns");
    }

    #[test]
    fn a_cell_is_named_by_its_headers_and_value() {
        assert_eq!(cell_name("2025Q1", "Widget", "100"), "2025Q1, Widget: 100");
        assert_eq!(cell_name("2025Q1", "Widget", ""), "2025Q1, Widget: empty");
        // A one-dimensional measure has no column header.
        assert_eq!(cell_name("Widget", "", "12"), "Widget: 12");
        assert_eq!(cell_name("", "", "7"), "7");
    }

    #[test]
    fn the_cache_tells_blank_from_not_loaded() {
        let mut c = Cache::default();
        let w = WindowReq {
            at: [(3, vec![30])].into(),
        };
        let a = vec![(1, 10), (2, 20), (3, 30)];
        let b = vec![(1, 10), (2, 21), (3, 30)];
        let other_page = vec![(1, 10), (2, 20), (3, 31)];
        assert_eq!(c.get(100, &a), Cell::NotLoaded);
        c.fill(100, &w, vec![(a.clone(), WireValue::Number(5.0))]);
        assert!(c.has(100, &w));
        assert_eq!(c.get(100, &a), Cell::Value(&WireValue::Number(5.0)));
        assert_eq!(c.get(100, &b), Cell::Blank);
        assert_eq!(c.get(100, &other_page), Cell::NotLoaded);
        c.set(100, b.clone(), Some(WireValue::Number(1.0)));
        assert_eq!(c.get(100, &b), Cell::Value(&WireValue::Number(1.0)));
        // A re-read window replaces its cells.
        c.fill(100, &w, vec![]);
        assert_eq!(c.get(100, &a), Cell::Blank);
        c.set(100, a.clone(), None);
        c.clear();
        assert_eq!(c.get(100, &a), Cell::NotLoaded);
    }
}
