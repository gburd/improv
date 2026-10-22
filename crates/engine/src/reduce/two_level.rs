//! The two-level reduce of the decomposable reductions (`SUM`, `AVERAGE`,
//! `MIN`, `MAX`, `PRODUCT`, `SUMSQ`, `AND`, `OR`, `XOR`, `COUNT`, `COUNTA`)
//! over one category of more than [`FLAT_MAX`] items: a group's rows split
//! into blocks of item ids (`id >> BLOCK_BITS`), each block reduces to a
//! [`Part`] in item order, and the parts merge in block order. An edit then
//! re-reduces one block and the merge, not the whole group.
//!
//! The counts, `AND`/`OR`/`XOR`, `MIN`/`MAX` and integer-valued sums give
//! what the flat fold gives. `SUM` and `AVERAGE` are compensated
//! ([`Neumaier`]) in each block and across the blocks, as the flat fold
//! is, so both are the exact sum's nearest float but for extreme
//! cancellation, and agree to within an ulp. `PRODUCT` and `SUMSQ` are a
//! fixed two-level fold: deterministic, the same in `evaluate`, the live
//! engine and the FIXPOINT host step, but not bit-equal to one
//! left-to-right fold. Over a category of up to [`FLAT_MAX`] items they
//! reduce flat, as before.
//!
//! Blocks are by item id, not position: an engine that takes a model's
//! appended items as data ([`crate::session::Engine::serves`]) does not know
//! their positions, but it blocks them as a fresh engine would.

use super::Rows;
use crate::builtins::{err, num, Neumaier};
use crate::CellValue;
use improv_core_model::{CategoryId, FuncId, ItemId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The most items a reduced category may have and still reduce flat.
pub(crate) const FLAT_MAX: usize = 1024;

/// A block spans `1 << BLOCK_BITS` item ids.
const BLOCK_BITS: u32 = 10;

/// The block of item `id`.
pub(crate) fn block(id: u32) -> u32 {
    id >> BLOCK_BITS
}

/// Whether an aggregation with a single result per group reduces in two
/// levels: a decomposable `func` of one input and no parameters, over one
/// category of more than [`FLAT_MAX`] items.
pub(crate) fn applies(
    func: FuncId,
    n_in: usize,
    n_params: usize,
    order: &[(CategoryId, Vec<ItemId>)],
) -> bool {
    matches!(func.0, 1..=9 | 1005 | 1006)
        && n_in == 1
        && n_params == 0
        && matches!(order, [(_, items)] if items.len() > FLAT_MAX)
}

/// A run of rows reduced: what merging needs to finish any of the
/// decomposable reductions.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Part {
    /// The rows.
    n: u64,
    /// The first error in item order: its position and code.
    err: Option<(u32, u8)>,
    /// A value the reduction cannot read (text for a number, a date for a
    /// boolean), which makes it `#VALUE!`.
    bad: bool,
    /// The numbers (`COUNT`), or the true values (`AND`, `OR`, `XOR`).
    k: u64,
    /// The floating-point accumulator, as bits.
    acc: u64,
    /// `SUM`, `AVERAGE`: the accumulator's [`Neumaier`] compensation, as bits.
    comp: u64,
}

/// The accumulator's start and step of `id`'s fold.
fn fold_of(id: u32) -> (f64, fn(f64, f64) -> f64) {
    match id {
        3 => (f64::INFINITY, f64::min),
        4 => (f64::NEG_INFINITY, f64::max),
        5 => (1.0, |a, x| a * x),
        6 => (-0.0, |a, x| a + x * x),
        // SUM and AVERAGE add through a Neumaier; the rest do not fold.
        _ => (-0.0, |a, _| a),
    }
}

/// `rows` (in item order) reduced to a [`Part`] for `func`.
pub(crate) fn part(func: FuncId, rows: &Rows) -> Part {
    let id = func.0;
    let (start, step) = fold_of(id);
    let mut acc = start;
    let mut sum = Neumaier::default();
    let mut step = |x: f64| match id {
        1 | 2 => sum.add(x),
        _ => acc = step(acc, x),
    };
    let mut p = Part {
        n: rows.len() as u64,
        err: None,
        bad: false,
        k: 0,
        acc: 0,
        comp: 0,
    };
    for (pos, row) in rows.pos.iter().zip(&rows.vals) {
        match (id, &row[0]) {
            (_, CellValue::Err(c)) => p.err = p.err.or(Some((pos[0], *c))),
            (1005, CellValue::Num(_) | CellValue::Date(_)) => p.k += 1,
            (1005 | 1006, _) => {}
            (7..=9, CellValue::Bool(b)) => p.k += u64::from(*b),
            (7..=9, CellValue::Num(x)) => p.k += u64::from(f64::from_bits(*x) != 0.0),
            (7..=9, _) | (_, CellValue::Text(_)) => p.bad = true,
            (_, CellValue::Num(x)) => step(f64::from_bits(*x)),
            (_, CellValue::Bool(b)) => step(f64::from(u8::from(*b))),
            (_, CellValue::Date(ms)) => step(*ms as f64),
        }
    }
    if matches!(id, 1 | 2) {
        (p.acc, p.comp) = (sum.s.to_bits(), sum.c.to_bits());
    } else {
        p.acc = acc.to_bits();
    }
    p
}

/// `parts`, in block order, merged into one.
pub(crate) fn merge<'a>(func: FuncId, parts: impl IntoIterator<Item = &'a Part>) -> Part {
    let (start, _) = fold_of(func.0);
    let combine: fn(f64, f64) -> f64 = match func.0 {
        3 => f64::min,
        4 => f64::max,
        5 => |a, x| a * x,
        _ => |a, x| a + x,
    };
    let mut acc = start;
    let mut sum = Neumaier::default();
    let mut out = Part {
        n: 0,
        err: None,
        bad: false,
        k: 0,
        acc: 0,
        comp: 0,
    };
    for p in parts {
        out.n += p.n;
        out.k += p.k;
        out.bad |= p.bad;
        out.err = match (out.err, p.err) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if matches!(func.0, 1 | 2) {
            // The blocks' sums compensated, and their lost parts added.
            sum.add(f64::from_bits(p.acc));
            sum.c += f64::from_bits(p.comp);
        } else {
            acc = combine(acc, f64::from_bits(p.acc));
        }
    }
    if matches!(func.0, 1 | 2) {
        (out.acc, out.comp) = (sum.s.to_bits(), sum.c.to_bits());
    } else {
        out.acc = acc.to_bits();
    }
    out
}

/// The result of `func` from its group's merged [`Part`].
pub(crate) fn finish(func: FuncId, p: &Part) -> CellValue {
    match func.0 {
        1005 => return CellValue::num(p.k as f64),
        1006 => return CellValue::num(p.n as f64),
        _ => {}
    }
    if let Some((_, c)) = p.err {
        return CellValue::Err(c);
    }
    if p.bad {
        return CellValue::Err(err::VALUE);
    }
    let acc = match func.0 {
        1 | 2 => Neumaier {
            s: f64::from_bits(p.acc),
            c: f64::from_bits(p.comp),
        }
        .value(),
        _ => f64::from_bits(p.acc),
    };
    match func.0 {
        2 => num(acc / p.n as f64),
        7 => CellValue::Bool(p.k == p.n),
        8 => CellValue::Bool(p.k > 0),
        9 => CellValue::Bool(p.k % 2 == 1),
        _ => num(acc),
    }
}

/// One group's two-level result from its rows (in item order) and each
/// row's item id: what the dataflow's two stages compute.
pub(crate) fn reduce(func: FuncId, rows: &Rows, items: &[u32]) -> CellValue {
    let mut blocks: BTreeMap<u32, Rows> = BTreeMap::new();
    for ((pos, vals), item) in rows.pos.iter().zip(&rows.vals).zip(items) {
        let b = blocks.entry(block(*item)).or_insert_with(|| Rows {
            pos: Vec::new(),
            vals: Vec::new(),
            capacity: rows.capacity,
        });
        b.pos.push(pos.clone());
        b.vals.push(vals.clone());
    }
    let parts: Vec<Part> = blocks.values().map(|b| part(func, b)).collect();
    finish(func, &merge(func, &parts))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::reduce;

    fn rows(vals: &[CellValue]) -> (Rows, Vec<u32>) {
        let n = vals.len() as u32;
        (
            Rows {
                pos: (0..n).map(|i| vec![i]).collect(),
                vals: vals.iter().map(|v| vec![v.clone()]).collect(),
                capacity: vals.len(),
            },
            // Ids 0, 256, 512, …: four rows per block.
            (0..n).map(|i| i * 256).collect(),
        )
    }

    const ALL: [u32; 11] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 1005, 1006];

    #[test]
    fn one_block_is_the_flat_fold() {
        let n = CellValue::num;
        let vals = [n(0.1), n(0.2), n(-0.3), CellValue::Bool(true), n(7.5)];
        let r = Rows {
            pos: (0..5).map(|i| vec![i]).collect(),
            vals: vals.iter().map(|v| vec![v.clone()]).collect(),
            capacity: 5,
        };
        for id in ALL {
            let f = FuncId(id);
            assert_eq!(
                reduce(f, &r, &[1, 2, 3, 4, 5]),
                reduce::reduce(f, &r, &[]),
                "{id}"
            );
        }
    }

    #[test]
    fn exact_reductions_match_the_flat_fold_across_blocks() {
        let n = CellValue::num;
        let ints: Vec<CellValue> = (0..40).map(|i| n(f64::from(i * 7 % 13) - 6.0)).collect();
        let (r, items) = rows(&ints);
        for id in [1, 2, 3, 4, 5, 6, 1005, 1006] {
            assert_eq!(
                reduce(FuncId(id), &r, &items),
                reduce::reduce(FuncId(id), &r, &[]),
                "{id}"
            );
        }
        let bools: Vec<CellValue> = (0..40)
            .map(|i| match i % 3 {
                0 => CellValue::Bool(true),
                1 => n(0.0),
                _ => n(2.0),
            })
            .collect();
        let (r, items) = rows(&bools);
        for id in [7, 8, 9] {
            assert_eq!(
                reduce(FuncId(id), &r, &items),
                reduce::reduce(FuncId(id), &r, &[]),
                "{id}"
            );
        }
        let all_true = rows(&vec![CellValue::Bool(true); 9]);
        assert_eq!(
            reduce(FuncId(7), &all_true.0, &all_true.1),
            CellValue::Bool(true)
        );
    }

    #[test]
    fn errors_text_and_dates() {
        let n = CellValue::num;
        // The first error by position, though a later block's error has a
        // lower code.
        let mut v: Vec<CellValue> = (0..12).map(|i| n(f64::from(i))).collect();
        v[5] = CellValue::Err(err::NUM);
        v[9] = CellValue::Err(err::DIV0);
        v[10] = CellValue::Text("t".into());
        let (r, items) = rows(&v);
        assert_eq!(reduce(FuncId(1), &r, &items), CellValue::Err(err::NUM));
        // COUNT and COUNTA ignore errors and text.
        assert_eq!(reduce(FuncId(1005), &r, &items), n(9.0));
        assert_eq!(reduce(FuncId(1006), &r, &items), n(12.0));
        // Text alone is #VALUE!, for numbers and booleans alike.
        let (r, items) = rows(&[n(1.0), n(2.0), n(3.0), n(4.0), CellValue::Text("x".into())]);
        assert_eq!(reduce(FuncId(1), &r, &items), CellValue::Err(err::VALUE));
        assert_eq!(reduce(FuncId(7), &r, &items), CellValue::Err(err::VALUE));
        // A date counts as a number, and as its milliseconds in a sum; it
        // is no boolean.
        let (r, items) = rows(&[n(1.0), n(2.0), n(3.0), n(4.0), CellValue::Date(5)]);
        assert_eq!(reduce(FuncId(1), &r, &items), n(15.0));
        assert_eq!(reduce(FuncId(1005), &r, &items), n(5.0));
        assert_eq!(reduce(FuncId(8), &r, &items), CellValue::Err(err::VALUE));
        // Overflow is #NUM!.
        let (r, items) = rows(&vec![n(f64::MAX); 6]);
        assert_eq!(reduce(FuncId(1), &r, &items), CellValue::Err(err::NUM));
    }

    #[test]
    fn floating_sums_are_compensated_in_and_across_blocks() {
        let n = CellValue::num;
        // 1 + 7e-16: a left fold drops every small term; the compensated
        // sums, flat and two-level, keep them.
        let mut v = vec![n(1.0)];
        v.extend((0..7).map(|_| n(1e-16)));
        let (r, items) = rows(&v);
        let two = reduce(FuncId(1), &r, &items).as_num().unwrap();
        let flat = reduce::reduce(FuncId(1), &r, &[]).as_num().unwrap();
        assert_eq!(v.iter().map(|c| c.as_num().unwrap()).sum::<f64>(), 1.0);
        assert_eq!(flat, 1.0000000000000007);
        assert_eq!(two, flat);
        assert_eq!(reduce(FuncId(2), &r, &items).as_num(), Some(flat / 8.0));
        // Cancellation across blocks: 1e16, 1, -1e16, 1 in four blocks.
        let (r, items) = rows(&[n(1e16), n(1.0), n(-1e16), n(1.0)]);
        assert_eq!(reduce(FuncId(1), &r, &items), n(2.0));
        assert_eq!(reduce::reduce(FuncId(1), &r, &[]), n(2.0));
        // An exact (integer) sum keeps the plain fold's bits, -0.0 included.
        let (r, items) = rows(&[n(-0.0), n(-0.0)]);
        let z = reduce(FuncId(1), &r, &items).as_num().unwrap();
        assert!(z == 0.0 && z.is_sign_negative());
        // SUMSQ stays a plain two-level fold: per block of four, then the
        // blocks in order.
        let sq: Vec<CellValue> = (0..8).map(|i| n(1.0 + f64::from(i) * 1e-8)).collect();
        let (r, items) = rows(&sq);
        let block = |b: &[CellValue]| {
            b.iter()
                .map(|c| c.as_num().unwrap())
                .fold(-0.0, |a, x| a + x * x)
        };
        assert_eq!(
            reduce(FuncId(6), &r, &items),
            n(-0.0 + block(&sq[..4]) + block(&sq[4..]))
        );
    }

    #[test]
    fn applies_only_to_large_single_category_reductions() {
        let big = vec![(CategoryId(1), vec![ItemId(0); FLAT_MAX + 1])];
        let small = vec![(CategoryId(1), vec![ItemId(0); FLAT_MAX])];
        assert!(applies(FuncId(1), 1, 0, &big));
        assert!(!applies(FuncId(1), 1, 0, &small));
        assert!(!applies(FuncId(1), 2, 0, &big));
        assert!(!applies(FuncId(1), 1, 1, &big));
        assert!(!applies(FuncId(1000), 1, 0, &big));
        let two = vec![big[0].clone(), big[0].clone()];
        assert!(!applies(FuncId(1), 1, 0, &two));
        assert_eq!(block(1023), 0);
        assert_eq!(block(1024), 1);
    }
}
