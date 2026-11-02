//! The pivot layout of one measure: which categories run down the rows,
//! across the columns, and which are pinned pages, and the cell at any
//! (row, column) of the result. Pure: no toolkit, no engine, so it is
//! tested directly and the grid widget only paints what it answers.
//!
//! Lines are never materialized: row `i` of a stack of categories is decoded
//! from `i` by mixed radix (the inner category fastest), so a grid of a
//! million rows costs what the visible ones cost.

use improv_core_model::{CategoryId, ItemId, MeasureId, Model};
use improv_engine::CoordKey;

/// Where a measure's categories sit: the first `n_rows` of `order` are
/// stacked on rows (outer first), the next `n_cols` on columns, the rest
/// are pages pinned to `page_items`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub measure: MeasureId,
    pub order: Vec<CategoryId>,
    pub n_rows: usize,
    pub n_cols: usize,
    /// One item per page category, in `order`'s page order.
    pub page_items: Vec<ItemId>,
}

/// An axis or the pages: where a category tile can be dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    Rows,
    Cols,
    Pages,
}

impl Layout {
    /// The measure's natural layout: its first category on rows, the second
    /// on columns, the rest pages at their first item. `None` when the
    /// measure does not exist.
    pub fn natural(model: &Model, measure: MeasureId) -> Option<Layout> {
        let m = model.measures.get(&measure)?;
        let order = m.categories.clone();
        let n_rows = order.len().min(1);
        let n_cols = order.len().saturating_sub(1).min(1);
        let mut l = Layout {
            measure,
            order,
            n_rows,
            n_cols,
            page_items: Vec::new(),
        };
        l.repin(model);
        Some(l)
    }

    pub fn row_cats(&self) -> &[CategoryId] {
        &self.order[..self.n_rows]
    }

    pub fn col_cats(&self) -> &[CategoryId] {
        &self.order[self.n_rows..self.n_rows + self.n_cols]
    }

    pub fn page_cats(&self) -> &[CategoryId] {
        &self.order[self.n_rows + self.n_cols..]
    }

    /// Re-pin each page to its current item if it still has one, else to its
    /// category's first item.
    fn repin(&mut self, model: &Model) {
        let old: Vec<(CategoryId, ItemId)> = self
            .page_cats()
            .iter()
            .copied()
            .zip(self.page_items.iter().copied())
            .collect();
        self.page_items = self
            .page_cats()
            .iter()
            .map(|c| {
                let items = items(model, *c);
                old.iter()
                    .find(|(oc, i)| oc == c && items.contains(i))
                    .map(|(_, i)| *i)
                    .or_else(|| items.first().copied())
                    .unwrap_or(ItemId(0))
            })
            .collect();
    }

    /// Move `cat` onto `axis` at position `at` within it (clamped), the
    /// pivot gesture. Formulas are untouched; only the layout changes.
    /// Returns false (and changes nothing) for a category not in the
    /// layout, or a move that would leave rows or columns empty while
    /// another category sits on the pages.
    pub fn place(&mut self, model: &Model, cat: CategoryId, axis: Axis, at: usize) -> bool {
        if !self.order.contains(&cat) {
            return false;
        }
        let mut rows: Vec<CategoryId> = self.row_cats().to_vec();
        let mut cols: Vec<CategoryId> = self.col_cats().to_vec();
        let mut pages: Vec<CategoryId> = self.page_cats().to_vec();
        for v in [&mut rows, &mut cols, &mut pages] {
            v.retain(|c| *c != cat);
        }
        let dest = match axis {
            Axis::Rows => &mut rows,
            Axis::Cols => &mut cols,
            Axis::Pages => &mut pages,
        };
        dest.insert(at.min(dest.len()), cat);
        // A grid with an empty axis while a page could fill it is never
        // what a drop means: refuse it, as the NeXT GUI does.
        if (rows.is_empty() || cols.is_empty()) && !pages.is_empty() {
            return false;
        }
        self.n_rows = rows.len();
        self.n_cols = cols.len();
        self.order = rows.into_iter().chain(cols).chain(pages).collect();
        self.repin(model);
        true
    }

    /// Swap the row and column stacks.
    pub fn transpose(&mut self) {
        let rows = self.row_cats().to_vec();
        let cols = self.col_cats().to_vec();
        let pages = self.page_cats().to_vec();
        self.n_rows = cols.len();
        self.n_cols = rows.len();
        self.order = cols.into_iter().chain(rows).chain(pages).collect();
    }

    /// Pin page `i` to `item`, if `item` belongs to that page's category.
    pub fn set_page(&mut self, model: &Model, i: usize, item: ItemId) -> bool {
        let ok = self
            .page_cats()
            .get(i)
            .is_some_and(|c| items(model, *c).contains(&item));
        if ok {
            self.page_items[i] = item;
        }
        ok
    }
}

/// A category's items in declared order.
pub fn items(model: &Model, cat: CategoryId) -> Vec<ItemId> {
    model
        .categories
        .get(&cat)
        .map(|c| c.items.clone())
        .unwrap_or_default()
}

/// The name of an item, or its id when it has none.
pub fn item_name(model: &Model, item: ItemId) -> String {
    model
        .items
        .get(&item)
        .map(|i| i.name.0.clone())
        .unwrap_or_else(|| item.0.to_string())
}

/// The name of a category, or its id when it has none.
pub fn category_name(model: &Model, cat: CategoryId) -> String {
    model
        .categories
        .get(&cat)
        .map(|c| c.name.0.clone())
        .unwrap_or_else(|| cat.0.to_string())
}

/// One axis's stack of categories, resolved to item lists, for decoding
/// lines by index.
#[derive(Debug, Clone, Default)]
pub struct Stack {
    pub cats: Vec<CategoryId>,
    pub items: Vec<Vec<ItemId>>,
}

impl Stack {
    pub fn new(model: &Model, cats: &[CategoryId]) -> Stack {
        Stack {
            cats: cats.to_vec(),
            items: cats.iter().map(|c| items(model, *c)).collect(),
        }
    }

    /// How many lines the stack has: the product of its item counts (one
    /// line for an empty stack, the scalar axis; none if a category is
    /// empty).
    pub fn len(&self) -> usize {
        self.items.iter().map(Vec::len).product()
    }

    /// Line `i`'s item per category, outer first; `None` past the end.
    pub fn line(&self, mut i: usize) -> Option<Vec<ItemId>> {
        if i >= self.len() {
            return None;
        }
        let mut out = vec![ItemId(0); self.items.len()];
        for (d, list) in self.items.iter().enumerate().rev() {
            out[d] = list[i % list.len()];
            i /= list.len();
        }
        Some(out)
    }
}

/// A layout resolved against the model, ready to answer cell keys.
#[derive(Debug, Clone)]
pub struct Grid {
    pub rows: Stack,
    pub cols: Stack,
    pages: Vec<(CategoryId, ItemId)>,
}

impl Grid {
    pub fn new(model: &Model, layout: &Layout) -> Grid {
        Grid {
            rows: Stack::new(model, layout.row_cats()),
            cols: Stack::new(model, layout.col_cats()),
            pages: layout
                .page_cats()
                .iter()
                .copied()
                .zip(layout.page_items.iter().copied())
                .collect(),
        }
    }

    /// The engine key of the cell at (`row`, `col`): every category's item,
    /// sorted by category id. `None` outside the grid.
    pub fn key(&self, row: usize, col: usize) -> Option<CoordKey> {
        let r = self.rows.line(row)?;
        let c = self.cols.line(col)?;
        let mut k: CoordKey = self
            .rows
            .cats
            .iter()
            .zip(r)
            .chain(self.cols.cats.iter().zip(c))
            .map(|(cat, it)| (cat.0, it.0))
            .chain(self.pages.iter().map(|(c, i)| (c.0, i.0)))
            .collect();
        k.sort_unstable();
        Some(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::tests::revenue_model;

    #[test]
    fn natural_layout_puts_one_category_per_axis_and_pins_the_rest() {
        let m = revenue_model();
        let l = Layout::natural(&m, MeasureId(100)).unwrap();
        assert_eq!(l.row_cats(), &[CategoryId(1)]);
        assert_eq!(l.col_cats(), &[CategoryId(2)]);
        assert!(l.page_cats().is_empty());
        assert!(Layout::natural(&m, MeasureId(999)).is_none());
    }

    #[test]
    fn lines_decode_by_mixed_radix_inner_fastest() {
        let s = Stack {
            cats: vec![CategoryId(1), CategoryId(2)],
            items: vec![
                vec![ItemId(1), ItemId(2)],
                vec![ItemId(7), ItemId(8), ItemId(9)],
            ],
        };
        assert_eq!(s.len(), 6);
        assert_eq!(s.line(0), Some(vec![ItemId(1), ItemId(7)]));
        assert_eq!(s.line(4), Some(vec![ItemId(2), ItemId(8)]));
        assert_eq!(s.line(6), None);
        let scalar = Stack::default();
        assert_eq!(scalar.len(), 1);
        assert_eq!(scalar.line(0), Some(vec![]));
        let empty = Stack {
            cats: vec![CategoryId(1)],
            items: vec![vec![]],
        };
        assert_eq!(empty.len(), 0);
        assert_eq!(empty.line(0), None);
    }

    #[test]
    fn keys_are_sorted_and_cover_every_category() {
        let m = revenue_model();
        let l = Layout::natural(&m, MeasureId(100)).unwrap();
        let g = Grid::new(&m, &l);
        // Rows are Time (1), columns Product (2).
        let k = g.key(1, 0).unwrap();
        assert_eq!(k.as_slice(), &[(1, 11), (2, 20)]);
        assert!(g.key(9, 0).is_none());
    }

    #[test]
    fn placing_a_category_pivots_and_refuses_an_empty_axis() {
        let m = revenue_model();
        let mut l = Layout::natural(&m, MeasureId(100)).unwrap();
        // Stack Product under Time on rows: columns become the scalar axis.
        assert!(l.place(&m, CategoryId(2), Axis::Rows, 1));
        assert_eq!(l.row_cats(), &[CategoryId(1), CategoryId(2)]);
        assert!(l.col_cats().is_empty());
        // To the pages with rows still holding Time: columns empty while a
        // page exists is refused.
        assert!(!l.place(&m, CategoryId(2), Axis::Pages, 0));
        assert!(l.place(&m, CategoryId(2), Axis::Cols, 0));
        assert_eq!(l.col_cats(), &[CategoryId(2)]);
        assert!(!l.place(&m, CategoryId(77), Axis::Rows, 0));
    }

    #[test]
    fn pages_pin_to_an_item_of_their_category() {
        let m = revenue_model();
        let mut l = Layout::natural(&m, MeasureId(100)).unwrap();
        // Time to the pages beside a one-category grid is refused (rows
        // would empty), so move Product there instead after stacking.
        assert!(l.place(&m, CategoryId(1), Axis::Cols, 1));
        assert!(l.row_cats().is_empty());
        assert!(l.place(&m, CategoryId(2), Axis::Rows, 0));
        assert!(l.place(&m, CategoryId(1), Axis::Rows, 1));
        // Now rows = Product, Time; no columns, no pages.
        assert_eq!(l.page_items, Vec::<ItemId>::new());
        assert!(!l.set_page(&m, 0, ItemId(10)));
    }

    #[test]
    fn transposing_swaps_rows_and_columns() {
        let m = revenue_model();
        let mut l = Layout::natural(&m, MeasureId(100)).unwrap();
        l.transpose();
        assert_eq!(l.row_cats(), &[CategoryId(2)]);
        assert_eq!(l.col_cats(), &[CategoryId(1)]);
        assert_eq!(category_name(&m, CategoryId(2)), "Product");
        assert_eq!(category_name(&m, CategoryId(9)), "9");
    }
}
