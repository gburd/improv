//! Windowed reductions, `F(x OVER T[from..to])`: one result per row, the
//! reduction over the rows whose position along `T` lies in the row's window.
//!
//! Rows arrive in item order, so each row's window is a run of rows, and
//! both ends of the run move forward from one row to the next: two pointers
//! find every window in O(n). `SUM` … `XOR` then keep a running state as
//! the window slides (an exact sum with removal, a monotone deque for `MIN`
//! and `MAX`, counts for the logical ones; `PRODUCT` and `SUMSQ`, which fold
//! left to right, only over windows that start at `FIRST`), so a group costs
//! O(n). Any other reduction gets each window's rows. Every result is the one
//! [`scan`] gives, bit for bit: the reduction over the window's rows.

use super::Rows;
use crate::builtins::{self, err, num};
use crate::exact::WindowSum;
use crate::typed::{Bound, Window};
use crate::CellValue;
use improv_core_model::FuncId;
use std::collections::VecDeque;

/// One result per row: the reduction `func` over the rows whose position
/// (along the single reduced category) lies in the row's window `w`.
pub(crate) fn windows(func: FuncId, rs: &Rows, params: &[CellValue], w: Window) -> Vec<CellValue> {
    let pos: Vec<i64> = rs.pos.iter().map(|p| i64::from(p[0])).collect();
    if !pos.is_sorted() {
        return scan(func, rs, params, w);
    }
    let spans = spans(&pos, w);
    if let Some(out) = sliding(func.0, rs, params, w, &spans) {
        return out;
    }
    spans
        .iter()
        .map(|&(a, b)| {
            if a >= b {
                return CellValue::Err(err::NA);
            }
            let sub = Rows {
                pos: rs.pos[a..b].to_vec(),
                vals: rs.vals[a..b].to_vec(),
                capacity: rs.capacity,
            };
            super::reduce(func, &sub, params)
        })
        .collect()
}

/// Each row's window as the rows `a..b` (empty when `a >= b`), for rows
/// sorted by position. FIRST and LAST are the first and last rows'.
fn spans(pos: &[i64], w: Window) -> Vec<(usize, usize)> {
    let first = pos.first().copied().unwrap_or(0);
    let last = pos.last().copied().unwrap_or(0);
    let (mut a, mut b) = (0, 0);
    pos.iter()
        .map(|&me| {
            let at = |b: Bound| match b {
                Bound::First => first,
                Bound::Last => last,
                Bound::Offset(o) => me.saturating_add(o),
            };
            let (lo, hi) = (at(w.0), at(w.1));
            // Both bounds only grow from row to row.
            while a < pos.len() && pos[a] < lo {
                a += 1;
            }
            while b < pos.len() && pos[b] <= hi {
                b += 1;
            }
            (a, b)
        })
        .collect()
}

/// The definition: per row, pick the rows in its window and reduce them.
/// O(n²) per group; the reference [`windows`] is held to, and its path for
/// rows out of position order.
pub(crate) fn scan(func: FuncId, rs: &Rows, params: &[CellValue], w: Window) -> Vec<CellValue> {
    let positions: Vec<i64> = rs.pos.iter().map(|p| i64::from(p[0])).collect();
    // FIRST and LAST are the group's first and last items with a cell.
    let first = positions.iter().copied().min().unwrap_or(0);
    let last = positions.iter().copied().max().unwrap_or(0);
    (0..rs.len())
        .map(|i| {
            let me = positions[i];
            let at = |b: Bound| match b {
                Bound::First => first,
                Bound::Last => last,
                Bound::Offset(o) => me.saturating_add(o),
            };
            let (lo, hi) = (at(w.0), at(w.1));
            let pick: Vec<usize> = (0..rs.len())
                .filter(|j| (lo..=hi).contains(&positions[*j]))
                .collect();
            if pick.is_empty() {
                return CellValue::Err(err::NA);
            }
            let sub = Rows {
                pos: pick.iter().map(|j| rs.pos[*j].clone()).collect(),
                vals: pick.iter().map(|j| rs.vals[*j].clone()).collect(),
                capacity: rs.capacity,
            };
            super::reduce(func, &sub, params)
        })
        .collect()
}

/// `SUM` … `XOR` (ids 1–9) over every window in O(n), or `None` for a
/// reduction (or a window, for `PRODUCT` and `SUMSQ`) that does not slide.
/// As the reduction does: the window's first error is the result, then any
/// value its scalar twin cannot read is `#VALUE!`.
fn sliding(
    id: u32,
    rs: &Rows,
    params: &[CellValue],
    w: Window,
    spans: &[(usize, usize)],
) -> Option<Vec<CellValue>> {
    let folds = matches!(id, 5 | 6);
    if !(1..=9).contains(&id)
        || !params.is_empty()
        || rs.vals.iter().any(|v| v.len() != 1)
        || (folds && w.0 != Bound::First)
    {
        return None;
    }
    let logical = id >= 7;
    let errs = next(rs, |v| matches!(v[0], CellValue::Err(_)));
    let bad = next(rs, |v| {
        if logical {
            builtins::b(v, 0).is_err()
        } else {
            builtins::n(v, 0).is_err()
        }
    });
    let mut s = Slide {
        id,
        rs,
        sum: WindowSum::default(),
        best: VecDeque::new(),
        // The same start as the scalar twin's `product()` and `sum()`.
        fold: if id == 5 {
            std::iter::empty::<f64>().product()
        } else {
            std::iter::empty::<f64>().sum()
        },
        trues: 0,
    };
    let (mut ra, mut rb) = (0, 0);
    let out = spans
        .iter()
        .map(|&(a, b)| {
            // The state holds rows ra..rb; both ends only move forward.
            while rb < a.max(b) {
                s.add(rb);
                rb += 1;
            }
            while ra < a {
                s.remove(ra);
                ra += 1;
            }
            if a >= b {
                CellValue::Err(err::NA)
            } else if errs[a] < b {
                rs.vals[errs[a]][0].clone()
            } else if bad[a] < b {
                CellValue::Err(err::VALUE)
            } else {
                s.value(b - a)
            }
        })
        .collect();
    Some(out)
}

/// For each row, the first row at or after it whose values `p` holds for
/// (the row count if none), and one past the end for the empty window.
fn next(rs: &Rows, p: impl Fn(&[CellValue]) -> bool) -> Vec<usize> {
    let n = rs.len();
    let mut out = vec![n; n + 1];
    for j in (0..n).rev() {
        out[j] = if p(&rs.vals[j]) { j } else { out[j + 1] };
    }
    out
}

/// A sliding window's running state for `SUM` … `XOR`, over the rows'
/// readable values (a window holding any other is an error anyway).
struct Slide<'r> {
    id: u32,
    rs: &'r Rows,
    sum: WindowSum,
    /// `MIN`/`MAX`: the rows that may still be the window's extreme, in row
    /// order, their values strictly worse from front to back.
    best: VecDeque<usize>,
    /// `PRODUCT`/`SUMSQ`: the fold so far (their windows only grow).
    fold: f64,
    trues: usize,
}

impl Slide<'_> {
    fn x(&self, j: usize) -> Option<f64> {
        builtins::n(&self.rs.vals[j], 0).ok()
    }

    fn truth(&self, j: usize) -> bool {
        builtins::b(&self.rs.vals[j], 0) == Ok(true)
    }

    /// The extreme of two numbers as the twin folds them (`-0.0` below
    /// `0.0`; a NaN never wins).
    fn pick(&self, a: f64, x: f64) -> f64 {
        if self.id == 3 {
            builtins::min(a, x)
        } else {
            builtins::max(a, x)
        }
    }

    fn add(&mut self, j: usize) {
        match (self.id, self.x(j)) {
            (1 | 2, Some(x)) => self.sum.add(x),
            (3 | 4, Some(x)) if !x.is_nan() => {
                // A row no better than x leaves the window before x does.
                while let Some(&k) = self.best.back() {
                    let y = self.x(k).unwrap_or(f64::NAN);
                    if self.pick(y, x).to_bits() != x.to_bits() {
                        break;
                    }
                    self.best.pop_back();
                }
                self.best.push_back(j);
            }
            (5, Some(x)) => self.fold *= x,
            (6, Some(x)) => self.fold += x * x,
            (7..=9, _) if self.truth(j) => self.trues += 1,
            _ => {}
        }
    }

    fn remove(&mut self, j: usize) {
        match (self.id, self.x(j)) {
            (1 | 2, Some(x)) => self.sum.remove(x),
            (3 | 4, _) if self.best.front() == Some(&j) => {
                self.best.pop_front();
            }
            (7..=9, _) if self.truth(j) => self.trues -= 1,
            _ => {}
        }
    }

    /// The window's result, over its `len` rows (none an error or
    /// unreadable).
    fn value(&mut self, len: usize) -> CellValue {
        let best = self.best.front().and_then(|&k| self.x(k));
        match self.id {
            1 => num(self.sum.value()),
            2 => num(self.sum.value() / len as f64),
            3 => num(best.unwrap_or(f64::INFINITY)),
            4 => num(best.unwrap_or(f64::NEG_INFINITY)),
            5 | 6 => num(self.fold),
            7 => CellValue::Bool(self.trues == len),
            8 => CellValue::Bool(self.trues > 0),
            _ => CellValue::Bool(self.trues % 2 == 1),
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn value() -> impl Strategy<Value = CellValue> {
        let special = [
            0.0,
            -0.0,
            1.0,
            -1.0,
            0.5,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NAN,
            f64::MAX,
            -f64::MAX,
            1e300,
            -1e300,
            1e-300,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
        ];
        prop_oneof![
            6 => (-20i32..=20).prop_map(|i| CellValue::num(f64::from(i) / 4.0)),
            2 => proptest::sample::select(special.to_vec()).prop_map(CellValue::num),
            1 => any::<u64>().prop_map(CellValue::Num),
            1 => any::<bool>().prop_map(CellValue::Bool),
            1 => (-3i64..=3).prop_map(CellValue::Date),
            1 => proptest::sample::select(vec!["", "a"]).prop_map(|s| CellValue::Text(s.into())),
            1 => proptest::sample::select(vec![err::NA, err::DIV0]).prop_map(CellValue::Err),
        ]
    }

    fn bound() -> impl Strategy<Value = Bound> {
        prop_oneof![
            Just(Bound::First),
            Just(Bound::Last),
            (-4i64..=4).prop_map(Bound::Offset),
        ]
    }

    /// Rows in position order: steps of 0 (a repeated item), 1, 2, or a
    /// jump (an item the model does not order sorts far after the rest).
    fn rows() -> impl Strategy<Value = Rows> {
        let step = prop_oneof![4 => 0u32..=2, 1 => Just(1u32 << 30)];
        proptest::collection::vec((step, value()), 0..24).prop_map(|rows| {
            let mut at = 0u32;
            let (mut pos, mut vals) = (Vec::new(), Vec::new());
            for (step, v) in rows {
                at = at.saturating_add(step);
                pos.push(vec![at]);
                vals.push(vec![v]);
            }
            Rows {
                pos,
                vals,
                capacity: 64,
            }
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]

        /// The linear kernel gives the definition's results, bit for bit,
        /// for the sliding reductions and the rest alike.
        #[test]
        fn windows_are_the_scan(
            rs in rows(),
            func in proptest::sample::select(vec![
                1u32, 2, 3, 4, 5, 6, 7, 8, 9, 906, 907, 1005, 1006, 1016, 1034,
            ]),
            k in proptest::option::of(1i32..=3),
            from in bound(),
            to in bound(),
            shuffle in proptest::option::of(any::<usize>()),
        ) {
            // A parameter (LARGE's k; any other reduction with one runs
            // the general path).
            let params: Vec<CellValue> = k.map(|k| CellValue::num(f64::from(k))).into_iter().collect();
            let mut rs = rs;
            if let Some(r) = shuffle.filter(|_| rs.len() > 1) {
                // Out of position order: the scan's own path.
                let n = rs.len();
                rs.pos.rotate_left(r % n);
            }
            let w = (from, to);
            prop_assert_eq!(
                windows(FuncId(func), &rs, &params, w),
                scan(FuncId(func), &rs, &params, w)
            );
        }
    }

    #[test]
    fn running_sum_and_extremes() {
        let n = CellValue::num;
        let rs = Rows {
            pos: (0..5).map(|i| vec![i]).collect(),
            vals: [3.0, -1.0, 4.0, 1.0, 5.0].map(|x| vec![n(x)]).to_vec(),
            capacity: 5,
        };
        let w = |a, b| (a, b);
        assert_eq!(
            windows(FuncId(1), &rs, &[], w(Bound::First, Bound::Offset(0))),
            [3.0, 2.0, 6.0, 7.0, 12.0].map(n).to_vec()
        );
        assert_eq!(
            windows(FuncId(3), &rs, &[], w(Bound::Offset(-1), Bound::Offset(0))),
            [3.0, -1.0, -1.0, 1.0, 1.0].map(n).to_vec()
        );
        assert_eq!(
            windows(FuncId(4), &rs, &[], w(Bound::Offset(0), Bound::Last)),
            [5.0, 5.0, 5.0, 5.0, 5.0].map(n).to_vec()
        );
        // An empty window is #N/A.
        assert_eq!(
            windows(FuncId(1), &rs, &[], w(Bound::Offset(1), Bound::Offset(0))),
            vec![CellValue::Err(err::NA); 5]
        );
    }
}
