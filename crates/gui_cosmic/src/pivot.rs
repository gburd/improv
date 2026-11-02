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
}
