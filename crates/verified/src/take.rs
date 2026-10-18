//! The index range of `TAKE` / `DROP` over a list.

#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

/// The `[from, to)` range `TAKE` (`drop` false) or `DROP` (`drop` true)
/// keeps of a list of `len` values, for a count of `m` (already capped at
/// `len`) from the front, or from the end when `from_end`.
pub open spec fn range_spec(len: nat, m: nat, from_end: bool, drop: bool) -> (nat, nat) {
    if !from_end && !drop {
        (0, m)
    } else if from_end && !drop {
        ((len - m) as nat, len)
    } else if !from_end && drop {
        (m, len)
    } else {
        (0, (len - m) as nat)
    }
}

/// `range_spec`: in bounds (`from ≤ to ≤ len`), `m` values for `TAKE` and
/// `len − m` for `DROP`.
pub fn range(len: usize, m: usize, from_end: bool, drop: bool) -> (r: (usize, usize))
    requires
        m <= len,
    ensures
        r.0 <= r.1 <= len,
        (r.0 as nat, r.1 as nat) == range_spec(len as nat, m as nat, from_end, drop),
        r.1 - r.0 == if drop { len - m } else { m as int },
{
    match (from_end, drop) {
        (false, false) => (0, m),
        (true, false) => (len - m, len),
        (false, true) => (m, len),
        (true, true) => (0, len - m),
    }
}

/// Index `i` is in the range `r`.
pub open spec fn within(r: (nat, nat), i: nat) -> bool {
    r.0 <= i < r.1
}

/// `TAKE` and `DROP` of the same count partition the list: every index is
/// in exactly one of the two ranges.
pub proof fn lemma_take_drop_partition(len: nat, m: nat, from_end: bool)
    requires
        m <= len,
    ensures
        forall|i: nat|
            i < len ==> (#[trigger] within(range_spec(len, m, from_end, false), i) != within(
                range_spec(len, m, from_end, true),
                i,
            )),
{
}

} // verus!

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn take_and_drop_partition() {
        for len in 0..6 {
            for m in 0..=len {
                for end in [false, true] {
                    let (a, b) = range(len, m, end, false);
                    let (c, d) = range(len, m, end, true);
                    assert_eq!(b - a, m);
                    let mut all: Vec<usize> = (a..b).chain(c..d).collect();
                    all.sort_unstable();
                    assert_eq!(all, (0..len).collect::<Vec<_>>());
                }
            }
        }
    }
}
