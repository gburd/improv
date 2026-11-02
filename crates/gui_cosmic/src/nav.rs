//! Keyboard movement over the grid: where a key takes the cursor, and how
//! the view follows it. Pure; the window feeds it keys and the visible
//! size the canvas last reported.

use crate::sheet::Cell;

/// A cursor move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Move {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    /// The first column of the row (Home), or the first cell (Ctrl+Home).
    Home,
    End,
    First,
    Last,
}

/// The grid's extent and the view's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Extent {
    /// Data rows and columns in the grid.
    pub rows: usize,
    pub cols: usize,
    /// Whole data rows and columns that fit in the view (at least 1).
    pub view_rows: usize,
    pub view_cols: usize,
}

/// Where `m` takes `at`, clamped to the grid.
pub fn step(at: Cell, m: Move, e: Extent) -> Cell {
    let last_r = e.rows.saturating_sub(1);
    let last_c = e.cols.saturating_sub(1);
    let page = e.view_rows.max(1);
    let c = match m {
        Move::Up => Cell {
            row: at.row.saturating_sub(1),
            ..at
        },
        Move::Down => Cell {
            row: at.row + 1,
            ..at
        },
        Move::Left => Cell {
            col: at.col.saturating_sub(1),
            ..at
        },
        Move::Right => Cell {
            col: at.col + 1,
            ..at
        },
        Move::PageUp => Cell {
            row: at.row.saturating_sub(page),
            ..at
        },
        Move::PageDown => Cell {
            row: at.row.saturating_add(page),
            ..at
        },
        Move::Home => Cell { col: 0, ..at },
        Move::End => Cell { col: last_c, ..at },
        Move::First => Cell::default(),
        Move::Last => Cell {
            row: last_r,
            col: last_c,
        },
    };
    Cell {
        row: c.row.min(last_r),
        col: c.col.min(last_c),
    }
}

/// The view's top-left after the cursor moved to `cursor`: unchanged if the
/// cursor is in view, else the least scroll that brings it in.
pub fn follow(origin: Cell, cursor: Cell, e: Extent) -> Cell {
    let axis = |o: usize, c: usize, n: usize| {
        let n = n.max(1);
        if c < o {
            c
        } else if c >= o + n {
            c + 1 - n
        } else {
            o
        }
    };
    Cell {
        row: axis(origin.row, cursor.row, e.view_rows),
        col: axis(origin.col, cursor.col, e.view_cols),
    }
}

/// A selected range: the anchor (where Shift+move started) and the cursor,
/// inclusive, as (top-left, bottom-right).
pub fn range(anchor: Option<Cell>, cursor: Cell) -> (Cell, Cell) {
    let a = anchor.unwrap_or(cursor);
    (
        Cell {
            row: a.row.min(cursor.row),
            col: a.col.min(cursor.col),
        },
        Cell {
            row: a.row.max(cursor.row),
            col: a.col.max(cursor.col),
        },
    )
}

/// Whether `c` is in the range `r`.
pub fn contains(r: (Cell, Cell), c: Cell) -> bool {
    (r.0.row..=r.1.row).contains(&c.row) && (r.0.col..=r.1.col).contains(&c.col)
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: Extent = Extent {
        rows: 1_000_000,
        cols: 5,
        view_rows: 20,
        view_cols: 3,
    };

    fn cell(row: usize, col: usize) -> Cell {
        Cell { row, col }
    }

    #[test]
    fn moves_clamp_to_the_grid() {
        assert_eq!(step(cell(0, 0), Move::Up, E), cell(0, 0));
        assert_eq!(step(cell(0, 0), Move::Left, E), cell(0, 0));
        assert_eq!(step(cell(0, 4), Move::Right, E), cell(0, 4));
        assert_eq!(step(cell(5, 2), Move::PageDown, E), cell(25, 2));
        assert_eq!(step(cell(5, 2), Move::PageUp, E), cell(0, 2));
        assert_eq!(step(cell(5, 2), Move::End, E), cell(5, 4));
        assert_eq!(step(cell(5, 2), Move::Home, E), cell(5, 0));
        assert_eq!(step(cell(5, 2), Move::Last, E), cell(999_999, 4));
        assert_eq!(step(cell(5, 2), Move::First, E), cell(0, 0));
        let empty = Extent::default();
        assert_eq!(step(cell(0, 0), Move::Down, empty), cell(0, 0));
    }

    #[test]
    fn the_view_follows_the_cursor_with_the_least_scroll() {
        // In view: no scroll.
        assert_eq!(follow(cell(10, 0), cell(15, 2), E), cell(10, 0));
        // Below: the cursor becomes the last visible row.
        assert_eq!(follow(cell(10, 0), cell(40, 2), E), cell(21, 0));
        // Above and left: it becomes the first.
        assert_eq!(follow(cell(10, 2), cell(3, 0), E), cell(3, 0));
        // Right: the last visible column.
        assert_eq!(follow(cell(0, 0), cell(0, 4), E), cell(0, 2));
        // To the last of a million rows.
        let end = step(cell(0, 0), Move::Last, E);
        assert_eq!(follow(cell(0, 0), end, E), cell(999_980, 2));
    }

    #[test]
    fn a_range_spans_anchor_and_cursor_either_way() {
        let r = range(Some(cell(5, 3)), cell(2, 1));
        assert_eq!(r, (cell(2, 1), cell(5, 3)));
        assert!(contains(r, cell(4, 2)));
        assert!(!contains(r, cell(6, 2)));
        assert_eq!(range(None, cell(1, 1)), (cell(1, 1), cell(1, 1)));
    }
}
