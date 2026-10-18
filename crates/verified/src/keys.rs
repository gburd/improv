//! Coordinate-key projection and union: the key arithmetic of every
//! dimensional join.
//!
//! A coordinate key is a `(category, item)` list sorted by category with no
//! category twice (`sorted`), as every engine `CoordKey` is.

#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

/// Strictly increasing categories: sorted, and no category twice.
pub open spec fn sorted(k: Seq<(u32, u32)>) -> bool {
    forall|i: int, j: int| 0 <= i < j < k.len() ==> (#[trigger] k[i]).0 < (#[trigger] k[j]).0
}

/// The pair's category is one of `keep`.
pub open spec fn kept(keep: Seq<u32>) -> spec_fn((u32, u32)) -> bool {
    |p: (u32, u32)| keep.contains(p.0)
}

/// Some pair of `k` has category `c`.
pub open spec fn has_cat(k: Seq<(u32, u32)>, c: u32) -> bool {
    exists|i: int| 0 <= i < k.len() && (#[trigger] k[i]).0 == c
}

/// `a` and `b` give the same item to every category they share.
pub open spec fn agree(a: Seq<(u32, u32)>, b: Seq<(u32, u32)>) -> bool {
    forall|i: int, j: int|
        0 <= i < a.len() && 0 <= j < b.len() && (#[trigger] a[i]).0 == (#[trigger] b[j]).0 ==> a[i]
            == b[j]
}

/// Pushing keeps what was there and adds `x`.
proof fn lemma_push_contains(r0: Seq<(u32, u32)>, x: (u32, u32))
    ensures
        r0.push(x).contains(x),
        forall|y: (u32, u32)| r0.contains(y) ==> #[trigger] r0.push(x).contains(y),
{
    assert(r0.push(x)[r0.len() as int] == x);
    assert forall|y: (u32, u32)| r0.contains(y) implies #[trigger] r0.push(x).contains(y) by {
        let t = choose|t: int| 0 <= t < r0.len() && r0[t] == y;
        assert(r0.push(x)[t] == y);
    }
}

/// `c` is one of `keep` (linear: `keep` is a handful of categories).
#[inline]
fn has(keep: &[u32], c: u32) -> (b: bool)
    ensures
        b == keep@.contains(c),
{
    let mut i = 0;
    while i < keep.len()
        invariant
            0 <= i <= keep.len(),
            forall|m: int| 0 <= m < i ==> keep@[m] != c,
        decreases keep.len() - i,
    {
        if keep[i] == c {
            assert(keep@[i as int] == c);
            return true;
        }
        i += 1;
    }
    false
}

/// `k` restricted to the categories in `keep`: exactly the pairs whose
/// category is kept, in their order (a filter), so a sorted key projects to a
/// sorted key.
#[inline]
pub fn project(k: &[(u32, u32)], keep: &[u32]) -> (r: Vec<(u32, u32)>)
    ensures
        r@ == k@.filter(kept(keep@)),
        sorted(k@) ==> sorted(r@),
{
    let mut r: Vec<(u32, u32)> = Vec::with_capacity(k.len());
    let mut i = 0;
    while i < k.len()
        invariant
            0 <= i <= k.len(),
            r@ == k@.subrange(0, i as int).filter(kept(keep@)),
            sorted(k@) ==> sorted(r@),
            sorted(k@) ==> forall|t: int, m: int|
                0 <= t < r.len() && i <= m < k.len() ==> (#[trigger] r@[t]).0 < (#[trigger] k@[m]).0,
        decreases k.len() - i,
    {
        let x = k[i];
        let keep_it = has(keep, x.0);
        proof {
            assert(k@.subrange(0, i + 1) =~= k@.subrange(0, i as int).push(x));
            k@.subrange(0, i as int).lemma_filter_push(x, kept(keep@));
        }
        if keep_it {
            r.push(x);
        }
        i += 1;
    }
    assert(k@.subrange(0, k.len() as int) =~= k@);
    r
}

/// The union of two sorted keys, by merge: sorted (so no category twice);
/// every pair of `a`; every pair of `b` whose category `a` lacks (all of `b`
/// when the two agree on their shared categories, as join partners do); and
/// nothing else. On a shared category `a`'s item wins.
#[inline]
pub fn union_keys(a: &[(u32, u32)], b: &[(u32, u32)]) -> (r: Vec<(u32, u32)>)
    requires
        sorted(a@),
        sorted(b@),
    ensures
        sorted(r@),
        forall|x: (u32, u32)| a@.contains(x) ==> r@.contains(x),
        forall|x: (u32, u32)| #[trigger] b@.contains(x) ==> r@.contains(x) || has_cat(a@, x.0),
        agree(a@, b@) ==> forall|x: (u32, u32)| b@.contains(x) ==> r@.contains(x),
        forall|x: (u32, u32)| r@.contains(x) ==> a@.contains(x) || b@.contains(x),
{
    broadcast use vstd::seq_lib::lemma_seq_contains_after_push;

    let mut r: Vec<(u32, u32)> = Vec::with_capacity(a.len().saturating_add(b.len()));
    let mut i = 0;
    let mut j = 0;
    while i < a.len() || j < b.len()
        invariant
            0 <= i <= a.len(),
            0 <= j <= b.len(),
            sorted(a@),
            sorted(b@),
            sorted(r@),
            forall|t: int, m: int|
                0 <= t < r.len() && i <= m < a.len() ==> (#[trigger] r@[t]).0 < (#[trigger] a@[m]).0,
            forall|t: int, m: int|
                0 <= t < r.len() && j <= m < b.len() ==> (#[trigger] r@[t]).0 < (#[trigger] b@[m]).0,
            forall|m: int| 0 <= m < i ==> r@.contains(#[trigger] a@[m]),
            forall|m: int| 0 <= m < j ==> r@.contains(#[trigger] b@[m]) || has_cat(a@, b@[m].0),
            forall|x: (u32, u32)| r@.contains(x) ==> a@.contains(x) || b@.contains(x),
        decreases a.len() - i + b.len() - j,
    {
        if j >= b.len() || (i < a.len() && a[i].0 < b[j].0) {
            let x = a[i];
            assert(a@.contains(x) && x == a@[i as int]);
            let ghost r0 = r@;
            r.push(x);
            proof {
                lemma_push_contains(r0, x);
            }
            i += 1;
        } else if i >= a.len() || b[j].0 < a[i].0 {
            let x = b[j];
            assert(b@.contains(x) && x == b@[j as int]);
            let ghost r0 = r@;
            r.push(x);
            proof {
                lemma_push_contains(r0, x);
            }
            j += 1;
        } else {
            let x = a[i];
            assert(a@.contains(x) && x == a@[i as int]);
            assert(has_cat(a@, b@[j as int].0));
            let ghost r0 = r@;
            r.push(x);
            proof {
                lemma_push_contains(r0, x);
            }
            i += 1;
            j += 1;
        }
    }
    proof {
        assert forall|x: (u32, u32)| a@.contains(x) implies r@.contains(x) by {
            let m = choose|m: int| 0 <= m < a.len() && a@[m] == x;
            assert(r@.contains(a@[m]));
        }
        assert forall|x: (u32, u32)| #[trigger] b@.contains(x) implies r@.contains(x) || has_cat(
            a@,
            x.0,
        ) by {
            let m = choose|m: int| 0 <= m < b.len() && b@[m] == x;
            assert(r@.contains(b@[m]) || has_cat(a@, b@[m].0));
        }
        if agree(a@, b@) {
            assert forall|x: (u32, u32)| b@.contains(x) implies r@.contains(x) by {
                let m = choose|m: int| 0 <= m < b.len() && b@[m] == x;
                if !r@.contains(b@[m]) {
                    let t = choose|t: int| 0 <= t < a.len() && (#[trigger] a@[t]).0 == b@[m].0;
                    assert(a@[t] == b@[m]);
                    assert(r@.contains(a@[t]));
                }
            }
        }
    }
    r
}

} // verus!

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn project_filters_in_order() {
        let k = [(1, 10), (2, 20), (3, 30)];
        assert_eq!(project(&k, &[3, 1]), vec![(1, 10), (3, 30)]);
        assert_eq!(project(&k, &[]), vec![]);
        assert_eq!(project(&k, &[9]), vec![]);
    }

    #[test]
    fn union_merges_and_left_wins() {
        let a = [(1, 10), (3, 30)];
        let b = [(2, 20), (3, 31), (4, 40)];
        assert_eq!(union_keys(&a, &b), vec![(1, 10), (2, 20), (3, 30), (4, 40)]);
        assert_eq!(union_keys(&[], &b), b.to_vec());
        assert_eq!(union_keys(&a, &[]), a.to_vec());
    }
}
