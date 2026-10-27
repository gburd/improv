//! The order-statistics operator (B5): `RANK` (`RANK.EQ`) and `SORTBY`,
//! kept per group as a sorted index, so an edit emits only the results it
//! changes.
//!
//! DD's `reduce` hands its closure a group's whole input and diffs the
//! whole output: one edit among a million items re-ranks a million. This
//! operator, modeled on DD's `count_total`, keeps its own state per group
//! instead. It stashes each time's updates under a retained capability and
//! processes the time once the input frontier has passed it: Improv's
//! epochs (`u64`) are totally ordered, so a time is complete then.
//!
//! Per group it keeps every row (by row, which is item order: the batch
//! kernel's input) and the rows with a sort key, in key order (`order`, a
//! `Vec` of handles: an insert moves at most 4 MB at a million rows, about
//! 0.2 ms). An edit retracts one row and inserts another.
//!
//! * `RANK(x OVER T, [order])`: a row's rank is one plus the number of keys
//!   before its own. A value moving from a to b changes the ranks of exactly
//!   the rows whose keys lie between a and b. They are found by index, and
//!   each emits its old and new rank: O(k log n) for the k rows it passes.
//! * `SORTBY(x, by1 OVER T, [order1], … INTO K)`: slot i holds the i-th row
//!   in key order (ties in item order: the kernel's stable sort). Only the
//!   slots between the first and the last position an edit touched change.
//!
//! What the index cannot order goes to the batch kernel, for the group's old
//! and new state, and the operator emits the difference: an error among
//! RANK's values (every item's result is the first error), and a NaN (the
//! kernels' comparisons are no total order on it). So does a large batch
//! (the first fill), where one sort is cheaper than many inserts. Every
//! result is the kernel's ([`super::per_item`], [`super::array`]), bit for
//! bit.

use super::Rows;
use crate::builtins::{err, n, n_or, num};
use crate::dataflow::AggRow;
use crate::{CellValue, CoordKey};
use differential_dataflow::consolidation::consolidate;
use differential_dataflow::hashable::Hashable;
use differential_dataflow::{AsCollection, Collection};
use improv_core_model::{CategoryId, FuncId, ItemId};
use smallvec::SmallVec;
use std::cmp::{Ordering, Reverse};
use std::collections::{BTreeMap, HashMap};
use timely::dataflow::channels::pact::Exchange;
use timely::dataflow::operators::{Capability, Operator};
use timely::dataflow::Scope;
use timely::order::TotalOrder;

/// One component of a sort key, in `array::order`'s order: numbers and
/// dates (dates as milliseconds), then text ignoring case, then booleans,
/// then errors (all equal).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    Num(u64),
    Text(String),
    Bool(bool),
    Err,
}

/// A key component, ascending or descending.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Dir {
    Up(Part),
    Down(Reverse<Part>),
}

type Key = SmallVec<[Dir; 1]>;

/// Results as `((item or slot, value), diff)`.
type Out = Vec<((u32, CellValue), isize)>;

/// What the operator computes.
#[derive(Clone, Debug)]
pub(crate) enum Spec {
    /// `RANK`: ascending (an order other than 0) or not; each result lands
    /// at its row's item of the reduced category `cat`.
    Rank { asc: bool, cat: u32 },
    /// `SORTBY`: per key, descending or not; the k-th result lands on
    /// `cells[k]`.
    SortBy {
        desc: Vec<bool>,
        cells: Vec<CoordKey>,
    },
}

impl Spec {
    /// The operator's spec for aggregation `func` over `n_in` reduced
    /// measures with literal `params`, or `None` where the batch kernel
    /// keeps it: another function, or parameters it rejects (the kernel
    /// makes those the whole result).
    pub(crate) fn of(
        func: FuncId,
        n_in: usize,
        params: &[CellValue],
        per_item: bool,
        cat: Option<u32>,
        into: &[(CategoryId, Vec<ItemId>)],
    ) -> Option<Spec> {
        if params.iter().any(|p| matches!(p, CellValue::Err(_))) {
            return None;
        }
        match func.0 {
            1050 if per_item && into.is_empty() && n_in == 1 => Some(Spec::Rank {
                asc: n_or(params, 0, 0.0).ok()? != 0.0,
                cat: cat?,
            }),
            1253 if !into.is_empty() && n_in >= 2 => {
                // `array::sortby`'s orders: 1 or -1, absent ascending.
                let desc = (0..n_in - 1)
                    .map(|k| match params.get(k) {
                        None => Some(false),
                        Some(_) => match n(params, k).ok()? {
                            1.0 => Some(false),
                            -1.0 => Some(true),
                            _ => None,
                        },
                    })
                    .collect::<Option<Vec<bool>>>()?;
                let cells = crate::dataflow::grid_keys(into);
                Some(Spec::SortBy { desc, cells })
            }
            _ => None,
        }
    }

    /// A row's sort key (`None`: it has none), and whether it is odd: a
    /// value the index cannot order, which sends the group to the kernel.
    fn key(&self, row: &AggRow) -> (Option<Key>, bool) {
        let vals = &row.1;
        match self {
            Spec::Rank { asc, .. } => match vals.first() {
                Some(CellValue::Err(_)) => (None, true),
                Some(v) => match super::statistical::number(v) {
                    Some(x) if x.is_nan() => (None, true),
                    // Rank 1 is the largest (or, ascending, the smallest).
                    Some(x) => (
                        Some(smallvec::smallvec![dir(!asc, Part::Num(enc(x)))]),
                        false,
                    ),
                    None => (None, false),
                },
                None => (None, false),
            },
            Spec::SortBy { desc, .. } => {
                let mut key = Key::new();
                for (k, d) in desc.iter().enumerate() {
                    let part = match vals.get(k + 1) {
                        Some(CellValue::Num(b)) if f64::from_bits(*b).is_nan() => {
                            return (None, true)
                        }
                        Some(CellValue::Num(b)) => Part::Num(enc(f64::from_bits(*b))),
                        Some(CellValue::Date(ms)) => Part::Num(enc(*ms as f64)),
                        Some(CellValue::Text(s)) => Part::Text(s.to_lowercase()),
                        Some(CellValue::Bool(b)) => Part::Bool(*b),
                        Some(CellValue::Err(_)) => Part::Err,
                        None => return (None, true),
                    };
                    key.push(dir(*d, part));
                }
                (Some(key), false)
            }
        }
    }
}

/// `p`, descending or not.
fn dir(desc: bool, p: Part) -> Dir {
    if desc {
        Dir::Down(Reverse(p))
    } else {
        Dir::Up(p)
    }
}

/// A non-NaN number as bits in numeric order, -0 as 0 (`<` and
/// `partial_cmp` take them as equal).
fn enc(x: f64) -> u64 {
    let b = (x + 0.0).to_bits();
    if b >> 63 == 1 {
        !b
    } else {
        b | 1 << 63
    }
}

/// The aggregation the operator computes for every group.
pub(crate) struct Order {
    pub(crate) func: FuncId,
    pub(crate) params: Vec<CellValue>,
    pub(crate) capacity: usize,
    pub(crate) spec: Spec,
}

impl Order {
    /// The cell result `i` (an item or a slot) of group `g` lands on.
    fn place(&self, g: &CoordKey, i: u32) -> CoordKey {
        let mut k = g.clone();
        match &self.spec {
            Spec::Rank { cat, .. } => k.push((*cat, i)),
            Spec::SortBy { cells, .. } => k.extend(cells[i as usize].iter().copied()),
        }
        k.sort_unstable();
        k
    }
}

/// A distinct row of a group, with its multiplicity.
#[derive(Debug)]
struct Entry {
    row: AggRow,
    key: Option<Key>,
    odd: bool,
    count: isize,
    /// Changed in the batch being applied.
    touched: bool,
}

/// One group's rows.
#[derive(Debug, Default)]
struct Group {
    by_row: BTreeMap<AggRow, u32>,
    slab: Vec<Entry>,
    free: Vec<u32>,
    /// A handle per copy (a positive count's worth) of each row with a key,
    /// in (key, row) order.
    order: Vec<u32>,
    /// Copies of odd rows.
    odd: usize,
}

/// `out` with every diff negated.
fn negate(mut out: Out) -> Out {
    out.iter_mut().for_each(|(_, r)| *r = -*r);
    out
}

/// A copy count: a row counts while its multiplicity is positive.
fn copies(c: isize) -> usize {
    c.max(0).unsigned_abs()
}

impl Group {
    fn entry(&self, h: u32) -> &Entry {
        &self.slab[h as usize]
    }

    fn key_of(&self, h: u32) -> &Key {
        self.entry(h).key.as_ref().expect("ordered rows have keys")
    }

    /// Order handles `a` and `b` by (key, row).
    fn cmp(&self, a: u32, b: u32) -> Ordering {
        let (x, y) = (self.entry(a), self.entry(b));
        (&x.key, &x.row).cmp(&(&y.key, &y.row))
    }

    /// The first position of the copies of `h` in `order` (or where they
    /// would go).
    fn at(&self, h: u32) -> usize {
        self.order
            .partition_point(|&g| self.cmp(g, h) == Ordering::Less)
    }

    /// The batch kernel's results over the whole group.
    fn full(&self, o: &Order) -> Out {
        let mut rs = Rows {
            pos: Vec::new(),
            vals: Vec::new(),
            capacity: o.capacity,
        };
        let mut items = Vec::new();
        for (row, &h) in &self.by_row {
            for _ in 0..copies(self.entry(h).count) {
                rs.pos.push(row.0 .0.to_vec());
                rs.vals.push(row.1.to_vec());
                items.push(row.0 .1);
            }
        }
        match &o.spec {
            Spec::Rank { .. } => items
                .into_iter()
                .zip(super::per_item(o.func, &rs, &o.params))
                .map(|r| (r, 1))
                .collect(),
            Spec::SortBy { cells, .. } => super::array(o.func, &rs, &o.params)
                .into_iter()
                .take(cells.len())
                .zip(0u32..)
                .map(|(v, i)| ((i, v), 1))
                .collect(),
        }
    }

    /// The group's results: from the index where no row is odd (in O(n)),
    /// else from the batch kernel.
    fn results(&self, o: &Order) -> Out {
        if self.odd > 0 {
            return self.full(o);
        }
        let mut out = Vec::with_capacity(self.order.len());
        match &o.spec {
            Spec::Rank { .. } => {
                // A tie run shares the rank of its first row.
                let mut start = 0;
                for (i, &h) in self.order.iter().enumerate() {
                    if i > 0 && self.key_of(h) != self.key_of(self.order[i - 1]) {
                        start = i;
                    }
                    out.push(((self.entry(h).row.0 .1, num(start as f64 + 1.0)), 1));
                }
                for &h in self.by_row.values() {
                    let e = self.entry(h);
                    if e.key.is_none() && e.count > 0 {
                        out.push(((e.row.0 .1, CellValue::Err(err::VALUE)), e.count));
                    }
                }
            }
            Spec::SortBy { cells, .. } => {
                for (i, &h) in self.order.iter().take(cells.len()).enumerate() {
                    out.push(((i as u32, self.entry(h).row.1[0].clone()), 1));
                }
            }
        }
        out
    }

    /// Apply one time's `changes` (distinct rows, in row order) and return
    /// how the group's results change.
    fn apply(&mut self, o: &Order, changes: Vec<(AggRow, isize)>) -> Out {
        let changes: Vec<(AggRow, isize, Option<Key>, bool)> = changes
            .into_iter()
            .map(|(row, d)| {
                let (key, odd) = o.spec.key(&row);
                (row, d, key, odd)
            })
            .collect();
        // Many changes (the first fill): one sort beats an insert each,
        // which moves a quarter of the index on average (at 1M rows, 1,000
        // inserts cost about what the sort does).
        let bulk = changes.len() > 64 && changes.len().saturating_mul(1024) > self.order.len();
        let whole = bulk || self.odd > 0 || changes.iter().any(|c| c.1 > 0 && c.3);
        let mut out = if whole {
            negate(self.results(o))
        } else {
            Vec::new()
        };
        let n_old = self.order.len();
        // SORTBY: where the changed rows were.
        let mut span = (usize::MAX, 0);
        if !whole && matches!(o.spec, Spec::SortBy { .. }) {
            for (row, ..) in &changes {
                if let Some(&h) = self.by_row.get(row) {
                    let p0 = copies(self.entry(h).count);
                    if p0 > 0 && self.entry(h).key.is_some() {
                        let p = self.at(h);
                        span = (span.0.min(p), span.1.max(p + p0 - 1));
                    }
                }
            }
        }
        let mut touched = Vec::with_capacity(changes.len());
        for (row, d, key, odd) in changes {
            let h = match self.by_row.get(&row) {
                Some(&h) => h,
                None => self.alloc(row, key, odd),
            };
            let e = &mut self.slab[h as usize];
            let (p0, p1) = (copies(e.count), copies(e.count + d));
            e.count += d;
            e.touched = true;
            let (keyed, odd) = (e.key.is_some(), e.odd);
            if odd {
                self.odd = self.odd + p1 - p0;
            }
            if !bulk && keyed && p0 != p1 {
                let i = self.at(h);
                if p1 > p0 {
                    self.order.splice(i..i, std::iter::repeat_n(h, p1 - p0));
                } else {
                    self.order.drain(i..i + (p0 - p1));
                }
            }
            touched.push((h, p0, p1));
        }
        if bulk {
            self.order.clear();
            for &h in self.by_row.values() {
                if self.entry(h).key.is_some() {
                    let c = copies(self.entry(h).count);
                    self.order.extend(std::iter::repeat_n(h, c));
                }
            }
            // Stable, and by_row is in row order: (key, row) order.
            let slab = &self.slab;
            self.order
                .sort_by(|&a, &b| slab[a as usize].key.cmp(&slab[b as usize].key));
        }
        if whole {
            out.extend(self.results(o));
        } else {
            match &o.spec {
                Spec::Rank { .. } => self.ranks(&touched, &mut out),
                Spec::SortBy { cells, .. } => {
                    self.slots(&touched, n_old, span, cells.len(), &mut out)
                }
            }
        }
        for &(h, ..) in &touched {
            let e = &mut self.slab[h as usize];
            e.touched = false;
            if e.count == 0 {
                self.by_row.remove(&e.row);
                self.free.push(h);
            }
        }
        consolidate(&mut out);
        out
    }

    /// A new entry for `row`, which the group did not have.
    fn alloc(&mut self, row: AggRow, key: Option<Key>, odd: bool) -> u32 {
        let e = Entry {
            row: row.clone(),
            key,
            odd,
            count: 0,
            touched: false,
        };
        let h = match self.free.pop() {
            Some(h) => {
                self.slab[h as usize] = e;
                h
            }
            None => {
                self.slab.push(e);
                (self.slab.len() - 1) as u32
            }
        };
        self.by_row.insert(row, h);
        h
    }

    /// RANK's changed results: the touched rows', and those of the rows
    /// whose count of keys before them changed (`d` ≠ 0).
    fn ranks(&self, touched: &[(u32, usize, usize)], out: &mut Out) {
        let rank = |before: isize| num(before as f64 + 1.0);
        // The net change in copies per key, in key order.
        let mut ds: Vec<(&Key, isize)> = touched
            .iter()
            .filter(|&&(_, p0, p1)| p0 != p1)
            .filter_map(|&(h, p0, p1)| {
                Some((self.entry(h).key.as_ref()?, p1 as isize - p0 as isize))
            })
            .collect();
        ds.sort_by(|a, b| a.0.cmp(b.0));
        let mut net: Vec<(&Key, isize)> = Vec::with_capacity(ds.len());
        for (k, d) in ds {
            match net.last_mut() {
                Some((k2, d2)) if *k2 == k => *d2 += d,
                _ => net.push((k, d)),
            }
        }
        net.retain(|(_, d)| *d != 0);
        // pre[j]: the change in the count of keys before any key in
        // (net[j], net[j + 1]].
        let pre: Vec<isize> = net
            .iter()
            .scan(0, |acc, (_, d)| {
                *acc += d;
                Some(*acc)
            })
            .collect();
        let after = |k: &Key| self.order.partition_point(|&h| self.key_of(h) <= k);
        for (j, &d) in pre.iter().enumerate() {
            if d == 0 {
                continue;
            }
            let lo = after(net[j].0);
            let hi = net.get(j + 1).map_or(self.order.len(), |(k, _)| after(k));
            let mut i = lo;
            while i < hi {
                let start = i;
                let k = self.key_of(self.order[i]);
                while i < hi && self.key_of(self.order[i]) == k {
                    i += 1;
                }
                let new = start as isize;
                for &h in &self.order[start..i] {
                    let e = self.entry(h);
                    if !e.touched {
                        out.push(((e.row.0 .1, rank(new - d)), -1));
                        out.push(((e.row.0 .1, rank(new)), 1));
                    }
                }
            }
        }
        for &(h, p0, p1) in touched {
            let e = self.entry(h);
            let item = e.row.0 .1;
            let (p0, p1) = (p0 as isize, p1 as isize);
            match &e.key {
                Some(k) => {
                    let new = self.order.partition_point(|&g| self.key_of(g) < k) as isize;
                    let below = net.partition_point(|(m, _)| *m < k);
                    let d = below.checked_sub(1).map_or(0, |b| pre[b]);
                    out.push(((item, rank(new - d)), -p0));
                    out.push(((item, rank(new)), p1));
                }
                None => out.push(((item, CellValue::Err(err::VALUE)), p1 - p0)),
            }
        }
    }

    /// SORTBY's changed slots: those between the first and the last
    /// position a touched row had (`span`, before) or has; to the end when
    /// the count changed. The old slots are the rows not touched, merged
    /// with the touched rows' old copies.
    fn slots(
        &self,
        touched: &[(u32, usize, usize)],
        n_old: usize,
        span: (usize, usize),
        cells: usize,
        out: &mut Out,
    ) {
        let n_new = self.order.len();
        let (mut lo, mut hi) = span;
        for &(h, _, p1) in touched {
            if p1 > 0 && self.entry(h).key.is_some() {
                let p = self.at(h);
                (lo, hi) = (lo.min(p), hi.max(p + p1 - 1));
            }
        }
        if lo == usize::MAX {
            return;
        }
        if n_old != n_new {
            hi = n_old.max(n_new) - 1;
        }
        let end = hi.min(cells.saturating_sub(1));
        let mut gone: Vec<(u32, usize)> = touched
            .iter()
            .filter(|&&(h, p0, _)| p0 > 0 && self.entry(h).key.is_some())
            .map(|&(h, p0, _)| (h, p0))
            .collect();
        gone.sort_by(|a, b| self.cmp(a.0, b.0));
        let val = |h: u32| self.entry(h).row.1[0].clone();
        let (mut j, mut r, mut used) = (lo, 0, 0);
        for i in lo..=end {
            let mut old = None;
            if i < n_old {
                while j < n_new && self.entry(self.order[j]).touched {
                    j += 1;
                }
                let from_gone = r < gone.len()
                    && (j >= n_new || self.cmp(gone[r].0, self.order[j]) == Ordering::Less);
                if from_gone {
                    old = Some(gone[r].0);
                    used += 1;
                    if used == gone[r].1 {
                        (r, used) = (r + 1, 0);
                    }
                } else {
                    old = Some(self.order[j]);
                    j += 1;
                }
            }
            let (ov, nv) = (old.map(val), (i < n_new).then(|| val(self.order[i])));
            if ov != nv {
                let slot = i as u32;
                out.extend(ov.map(|v| ((slot, v), -1)));
                out.extend(nv.map(|v| ((slot, v), 1)));
            }
        }
    }
}

/// The updates of one time, by group.
type Stash<T> = BTreeMap<T, (Capability<T>, Vec<((CoordKey, AggRow), isize)>)>;

/// `o` over each group of `rows` (group key, row), as the cells it lands
/// on: the same collection the `reduce` with the batch kernel gives.
pub(crate) fn order_stats<G>(
    rows: &Collection<G, (CoordKey, AggRow), isize>,
    o: Order,
) -> Collection<G, (CoordKey, CellValue), isize>
where
    G: Scope,
    G::Timestamp: TotalOrder,
{
    type Update<T> = ((CoordKey, AggRow), T, isize);
    let by_group = Exchange::new(|u: &Update<G::Timestamp>| (u.0).0.hashed());
    rows.inner
        .unary_frontier(by_group, "OrderStats", move |_, _| {
            let mut stash: Stash<G::Timestamp> = BTreeMap::new();
            let mut groups: HashMap<CoordKey, Group> = HashMap::new();
            let mut buf = Vec::new();
            move |input, output| {
                input.for_each(|cap, data| {
                    data.swap(&mut buf);
                    for (d, t, r) in buf.drain(..) {
                        let at = stash
                            .entry(t.clone())
                            .or_insert_with(|| (cap.delayed(&t), Vec::new()));
                        at.1.push((d, r));
                    }
                });
                // A totally ordered time the frontier has passed is complete.
                while let Some(t) = stash.keys().next() {
                    if input.frontier().less_equal(t) {
                        break;
                    }
                    let (t, (cap, mut ups)) = stash.pop_first().expect("a stashed time");
                    consolidate(&mut ups);
                    let mut out = Vec::new();
                    let mut ups = ups.into_iter().peekable();
                    while let Some(((g, row), r)) = ups.next() {
                        let mut changes = vec![(row, r)];
                        while let Some(((_, row), r)) = ups.next_if(|((g2, _), _)| *g2 == g) {
                            changes.push((row, r));
                        }
                        let group = groups.entry(g.clone()).or_default();
                        for ((i, v), r) in group.apply(&o, changes) {
                            out.push(((o.place(&g, i), v), t.clone(), r));
                        }
                        if group.by_row.is_empty() {
                            groups.remove(&g);
                        }
                    }
                    output.session(&cap).give_iterator(out.into_iter());
                }
            }
        })
        .as_collection()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    //! The operator against the batch kernel: random groups and random
    //! edit rounds (inserts, deletions, value changes; ties, -0, text,
    //! booleans, dates, errors, NaN; batches large enough for the bulk
    //! path), on one worker and on three. After every round its output
    //! must be the kernel's over each group's current rows, bit for bit.
    use super::*;
    use crate::dataflow::{Pos, Vals};
    use differential_dataflow::input::InputSession;
    use hegel::generators as gs;
    use hegel::{HealthCheck, TestCase};
    use std::sync::{Arc, Mutex};
    use timely::dataflow::operators::probe::Handle as ProbeHandle;
    use timely::dataflow::operators::{Inspect, Probe};

    /// A function, its parameters, the reduced columns per row, the
    /// landing cells (SORTBY).
    #[derive(Clone, Debug)]
    struct Case {
        func: FuncId,
        params: Vec<CellValue>,
        cols: usize,
        into: Vec<(CategoryId, Vec<ItemId>)>,
    }

    /// Per round, the edits: (group, item, new values or a deletion).
    type Rounds = Vec<Vec<(u32, u32, Option<Vec<CellValue>>)>>;

    const CAT: u32 = 7;

    fn row(item: u32, vals: &[CellValue]) -> AggRow {
        let pos: Pos = smallvec::smallvec![item];
        ((pos, 1000 + item), Vals::from_vec(vals.to_vec()))
    }

    fn group(g: u32) -> CoordKey {
        CoordKey::from_slice(&[(1, g)])
    }

    fn spec(c: &Case) -> Spec {
        Spec::of(
            c.func,
            c.cols,
            &c.params,
            c.into.is_empty(),
            Some(CAT),
            &c.into,
        )
        .expect("a routed case")
    }

    /// The kernel's cells over `state`.
    fn expected(
        c: &Case,
        state: &BTreeMap<(u32, u32), Vec<CellValue>>,
    ) -> HashMap<(CoordKey, CellValue), isize> {
        let o = Order {
            func: c.func,
            params: c.params.clone(),
            capacity: 0,
            spec: spec(c),
        };
        let mut out = HashMap::new();
        let gs: std::collections::BTreeSet<u32> = state.keys().map(|k| k.0).collect();
        for g in gs {
            let mut rs = Rows {
                pos: Vec::new(),
                vals: Vec::new(),
                capacity: 0,
            };
            let mut items = Vec::new();
            for ((_, i), v) in state.range((g, 0)..(g + 1, 0)) {
                rs.pos.push(vec![*i]);
                rs.vals.push(v.clone());
                items.push(1000 + *i);
            }
            let res: Vec<(u32, CellValue)> = match &o.spec {
                Spec::Rank { .. } => items
                    .into_iter()
                    .zip(super::super::per_item(c.func, &rs, &c.params))
                    .collect(),
                Spec::SortBy { cells, .. } => (0u32..)
                    .zip(super::super::array(c.func, &rs, &c.params))
                    .take(cells.len())
                    .collect(),
            };
            for (i, v) in res {
                *out.entry((o.place(&group(g), i), v)).or_insert(0) += 1;
            }
        }
        out
    }

    /// Run `rounds` through the operator on `workers` workers; the rounds
    /// whose output differs from the kernel's, with both.
    fn run(c: &Case, rounds: &Rounds, workers: usize) -> Vec<String> {
        let bad = Arc::new(Mutex::new(Vec::new()));
        let got = Arc::new(Mutex::new(HashMap::<(CoordKey, CellValue), isize>::new()));
        let (c2, r2, bad2) = (c.clone(), rounds.clone(), Arc::clone(&bad));
        let guards = timely::execute(timely::Config::process(workers), move |worker| {
            let sink = Arc::clone(&got);
            let probe = ProbeHandle::new();
            let mut input = InputSession::<u64, (CoordKey, AggRow), isize>::new();
            let o = Order {
                func: c2.func,
                params: c2.params.clone(),
                capacity: 0,
                spec: spec(&c2),
            };
            worker.dataflow::<u64, _, _>(|scope| {
                let rows = input.to_collection(scope);
                let out = Arc::clone(&sink);
                order_stats(&rows, o)
                    .inner
                    .inspect(move |(d, _, r)| {
                        let mut m = out.lock().unwrap();
                        let e = m.entry(d.clone()).or_insert(0);
                        *e += r;
                        if *e == 0 {
                            m.remove(d);
                        }
                    })
                    .probe_with(&probe);
            });
            let me = worker.index();
            let mut state: BTreeMap<(u32, u32), Vec<CellValue>> = BTreeMap::new();
            for (t, edits) in r2.iter().enumerate() {
                for (g, i, v) in edits {
                    let old = match v {
                        Some(v) => state.insert((*g, *i), v.clone()),
                        None => state.remove(&(*g, *i)),
                    };
                    if me == 0 {
                        if let Some(old) = old {
                            input.remove((group(*g), row(*i, &old)));
                        }
                        if let Some(v) = v {
                            input.insert((group(*g), row(*i, v)));
                        }
                    }
                }
                let t = t as u64 + 1;
                input.advance_to(t);
                input.flush();
                while probe.less_than(&t) {
                    worker.step();
                }
                if me == 0 {
                    let want = expected(&c2, &state);
                    let have = sink.lock().unwrap().clone();
                    // Every worker's output is in: the probe has passed t.
                    if have != want {
                        bad2.lock().unwrap().push(format!(
                            "round {t}: state {state:?}\n have {have:?}\n want {want:?}"
                        ));
                    }
                }
            }
        })
        .expect("timely");
        for w in guards.join() {
            w.expect("a worker");
        }
        let v = bad.lock().unwrap().clone();
        v
    }

    fn int(tc: &TestCase, lo: i64, hi: i64) -> i64 {
        tc.draw(gs::integers::<i64>().min_value(lo).max_value(hi))
    }

    /// A value, mostly small numbers (ties), sometimes anything else
    /// (`odd`: errors, and NaN where `nan`).
    fn value(tc: &TestCase, odd: bool) -> CellValue {
        let top = if odd { 29 } else { 25 };
        match int(tc, 0, top) {
            0..=17 => CellValue::num(int(tc, -3, 3) as f64 / 2.0),
            18 => CellValue::num(-0.0),
            19 => CellValue::num(f64::INFINITY),
            20 | 21 => CellValue::Text(
                tc.draw(gs::sampled_from(vec!["a", "B", "b", ""]))
                    .to_string(),
            ),
            22 => CellValue::Bool(tc.draw(gs::booleans())),
            23..=25 => CellValue::Date(int(tc, -2, 2) * 86_400_000),
            26 | 27 => CellValue::Err(tc.draw(gs::sampled_from(vec![err::NA, err::DIV0]))),
            _ => CellValue::num(f64::NAN),
        }
    }

    fn case(tc: &TestCase) -> Case {
        if tc.draw(gs::booleans()) {
            let params = match int(tc, 0, 3) {
                0 => vec![],
                1 => vec![CellValue::num(0.0)],
                2 => vec![CellValue::num(1.0)],
                _ => vec![CellValue::Bool(true)],
            };
            Case {
                func: FuncId(1050),
                params,
                cols: 1,
                into: vec![],
            }
        } else {
            let keys = int(tc, 1, 2) as usize;
            let given = int(tc, 0, keys as i64) as usize;
            let params = (0..given)
                .map(|_| CellValue::num(if tc.draw(gs::booleans()) { 1.0 } else { -1.0 }))
                .collect();
            let cells = int(tc, 1, 40) as u32;
            Case {
                func: FuncId(1253),
                params,
                cols: keys + 1,
                into: vec![(CategoryId(9), (0..cells).map(|i| ItemId(500 + i)).collect())],
            }
        }
    }

    fn rounds(tc: &TestCase, c: &Case) -> Rounds {
        let odd = int(tc, 0, 3) == 0;
        let groups = int(tc, 1, 2) as u32;
        let items = int(tc, 1, if tc.draw(gs::booleans()) { 12 } else { 90 }) as u32;
        let vals = |tc: &TestCase| (0..c.cols).map(|_| value(tc, odd)).collect::<Vec<_>>();
        let mut out = Vec::new();
        // The first fill: most items.
        let mut first = Vec::new();
        for g in 0..groups {
            for i in 0..items {
                if int(tc, 0, 4) > 0 {
                    first.push((g, i, Some(vals(tc))));
                }
            }
        }
        out.push(first);
        for _ in 0..int(tc, 1, 8) {
            let n = if int(tc, 0, 6) == 0 {
                int(tc, 1, 80)
            } else {
                int(tc, 1, 3)
            };
            let edits = (0..n)
                .map(|_| {
                    let (g, i) = (
                        int(tc, 0, groups as i64 - 1) as u32,
                        int(tc, 0, items as i64) as u32,
                    );
                    let v = (int(tc, 0, 4) > 0).then(|| vals(tc));
                    (g, i, v)
                })
                .collect();
            out.push(edits);
        }
        out
    }

    #[hegel::test(test_cases = 400, suppress_health_check = HealthCheck::all())]
    fn the_operator_is_the_kernel_after_every_round(tc: TestCase) {
        let c = case(&tc);
        let r = rounds(&tc, &c);
        let bad = run(&c, &r, 1);
        assert!(bad.is_empty(), "{c:?}\n{}", bad.join("\n"));
    }

    #[hegel::test(test_cases = 100, suppress_health_check = HealthCheck::all())]
    fn the_operator_is_the_kernel_on_three_workers(tc: TestCase) {
        let c = case(&tc);
        let r = rounds(&tc, &c);
        let bad = run(&c, &r, 3);
        assert!(bad.is_empty(), "{c:?}\n{}", bad.join("\n"));
    }

    /// A value moving k places re-ranks just the k rows it passes.
    #[test]
    fn a_move_emits_only_the_rows_it_passes() {
        let c = Case {
            func: FuncId(1050),
            params: vec![],
            cols: 1,
            into: vec![],
        };
        let o = Order {
            func: c.func,
            params: vec![],
            capacity: 0,
            spec: spec(&c),
        };
        let mut g = Group::default();
        let n = |x: f64| vec![CellValue::num(x)];
        let fill: Vec<(AggRow, isize)> = (0..200).map(|i| (row(i, &n(f64::from(i))), 1)).collect();
        assert_eq!(g.apply(&o, fill).len(), 200);
        // 50 moves to 46.5, below 47, 48 and 49.
        let out = g.apply(&o, vec![(row(50, &n(50.0)), -1), (row(50, &n(46.5)), 1)]);
        // Its own old and new rank, and the three rows it passed: two each.
        assert_eq!(out.len(), 2 + 3 * 2, "{out:?}");
        // A new maximum shifts every row below it, and nothing else runs.
        let out = g.apply(&o, vec![(row(3, &n(3.0)), -1), (row(3, &n(1e9)), 1)]);
        assert_eq!(out.len(), 2 + 196 * 2);
    }

    #[test]
    fn specs_route_only_what_the_operator_computes() {
        let into = vec![(CategoryId(9), vec![ItemId(1)])];
        let p = |v: &[f64]| v.iter().map(|x| CellValue::num(*x)).collect::<Vec<_>>();
        assert!(Spec::of(FuncId(1050), 1, &[], true, Some(1), &[]).is_some());
        assert!(Spec::of(FuncId(1050), 1, &[], true, None, &[]).is_none());
        assert!(Spec::of(
            FuncId(1050),
            1,
            &[CellValue::Text("x".into())],
            true,
            Some(1),
            &[]
        )
        .is_none());
        assert!(Spec::of(
            FuncId(1050),
            1,
            &[CellValue::Err(err::NA)],
            true,
            Some(1),
            &[]
        )
        .is_none());
        assert!(Spec::of(FuncId(1051), 1, &[], true, Some(1), &[]).is_none());
        assert!(Spec::of(FuncId(1253), 2, &p(&[-1.0]), false, None, &into).is_some());
        assert!(Spec::of(FuncId(1253), 2, &p(&[0.0]), false, None, &into).is_none());
        assert!(Spec::of(
            FuncId(1253),
            2,
            &[CellValue::Text("x".into())],
            false,
            None,
            &into
        )
        .is_none());
        assert!(Spec::of(FuncId(1253), 1, &[], false, None, &into).is_none());
        assert!(Spec::of(FuncId(1253), 2, &[], false, None, &[]).is_none());
        // A row with fewer columns than keys has no key: the kernel decides.
        let s = Spec::of(FuncId(1253), 3, &[], false, None, &into).unwrap();
        assert_eq!(s.key(&row(0, &[CellValue::num(1.0)])), (None, true));
        // Numbers order as numbers, -0 as 0.
        let xs = [
            f64::NEG_INFINITY,
            -2.0,
            -0.5,
            0.0,
            1e-300,
            3.0,
            f64::INFINITY,
        ];
        assert!(xs.windows(2).all(|w| enc(w[0]) < enc(w[1])));
        assert_eq!(enc(-0.0), enc(0.0));
    }
}
