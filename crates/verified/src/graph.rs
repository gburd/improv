//! The dependency build order: a depth-first topological sort that rejects
//! cycles, so building (and evaluating) the derived measures terminates.
//!
//! Nodes are indices `0..g.len()`; `g[u]` lists `u`'s dependencies, in the
//! order they are visited. The result is the DFS post-order from `roots`.

#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

broadcast use vstd::seq_lib::lemma_seq_contains_after_push;

/// `w` is one of `u`'s dependencies.
pub open spec fn edge(g: Seq<Vec<usize>>, u: usize, w: usize) -> bool {
    u < g.len() && g[u as int]@.contains(w)
}

/// Every dependency names a node.
pub open spec fn wf(g: Seq<Vec<usize>>) -> bool {
    forall|u: int, k: int| 0 <= u < g.len() && 0 <= k < g[u]@.len() ==> (#[trigger] g[u]@[k]) < g.len()
}

/// Each node of `p` depends on the one after it.
pub open spec fn is_path(g: Seq<Vec<usize>>, p: Seq<usize>) -> bool {
    &&& p.len() >= 1
    &&& forall|i: int| 0 <= i < p.len() - 1 ==> #[trigger] edge(g, p[i], p[i + 1])
}

/// `v` is reachable from `roots` along dependencies.
pub open spec fn reachable(g: Seq<Vec<usize>>, roots: Seq<usize>, v: usize) -> bool {
    exists|p: Seq<usize>| #[trigger] is_path(g, p) && roots.contains(p[0]) && p.last() == v
}

/// A cycle: a path whose last node depends on its first.
pub open spec fn is_cycle(g: Seq<Vec<usize>>, c: Seq<usize>) -> bool {
    is_path(g, c) && edge(g, c.last(), c[0])
}

/// Some cycle is reachable from `roots`.
pub open spec fn has_reachable_cycle(g: Seq<Vec<usize>>, roots: Seq<usize>) -> bool {
    exists|c: Seq<usize>| #[trigger] is_cycle(g, c) && reachable(g, roots, c[0])
}

/// No node twice, and every node after all of its dependencies.
pub open spec fn topological(g: Seq<Vec<usize>>, order: Seq<usize>) -> bool {
    &&& order.no_duplicates()
    &&& forall|i: int, w: usize|
        0 <= i < order.len() && #[trigger] edge(g, order[i], w) ==> order.subrange(0, i).contains(w)
}

/// How many nodes are unvisited (state 0): the DFS's termination measure.
pub open spec fn unseen(st: Seq<u8>) -> nat
    decreases st.len(),
{
    if st.len() == 0 {
        0
    } else {
        unseen(st.drop_last()) + if st.last() == 0 {
            1nat
        } else {
            0nat
        }
    }
}

/// Visiting only ever makes unvisited nodes visited: `unseen` cannot grow,
/// and shrinks when node `k` is newly visited.
proof fn lemma_unseen(a: Seq<u8>, b: Seq<u8>, k: int)
    requires
        a.len() == b.len(),
        forall|u: int| 0 <= u < a.len() && b[u] == 0 ==> a[u] == 0,
    ensures
        unseen(b) <= unseen(a),
        0 <= k < a.len() && a[k] == 0 && b[k] != 0 ==> unseen(b) < unseen(a),
    decreases a.len(),
{
    if a.len() > 0 {
        let (a0, b0) = (a.drop_last(), b.drop_last());
        assert forall|u: int| 0 <= u < a0.len() && b0[u] == 0 implies a0[u] == 0 by {
            assert(a0[u] == a[u] && b0[u] == b[u]);
        }
        lemma_unseen(a0, b0, k);
    }
}

/// The DFS state: `st[u]` is 0 unvisited, 1 on the stack, 2 done. Done nodes
/// are exactly `order`, which is topological and reachable; the stack nodes
/// are exactly `stack`, a path from a root.
pub open spec fn inv(
    g: Seq<Vec<usize>>,
    roots: Seq<usize>,
    st: Seq<u8>,
    order: Seq<usize>,
    stack: Seq<usize>,
) -> bool {
    &&& st.len() == g.len()
    &&& forall|u: int| 0 <= u < st.len() ==> #[trigger] st[u] <= 2
    &&& forall|u: usize| u < st.len() ==> (st[u as int] == 2 <==> #[trigger] order.contains(u))
    &&& forall|u: usize| u < st.len() ==> (st[u as int] == 1 <==> #[trigger] stack.contains(u))
    &&& forall|k: int| 0 <= k < order.len() ==> #[trigger] order[k] < g.len()
    &&& forall|k: int| 0 <= k < stack.len() ==> #[trigger] stack[k] < g.len()
    &&& topological(g, order)
    &&& forall|k: int| 0 <= k < order.len() ==> reachable(g, roots, #[trigger] order[k])
    &&& stack.len() > 0 ==> is_path(g, stack) && roots.contains(stack[0])
}

/// A stack node that a stack node depends on closes a reachable cycle.
proof fn lemma_back_edge(g: Seq<Vec<usize>>, roots: Seq<usize>, stk: Seq<usize>, w: usize)
    requires
        is_path(g, stk),
        roots.contains(stk[0]),
        stk.contains(w),
        edge(g, stk.last(), w),
    ensures
        has_reachable_cycle(g, roots),
{
    let k = choose|k: int| 0 <= k < stk.len() && stk[k] == w;
    let c = stk.subrange(k, stk.len() as int);
    assert forall|i: int| 0 <= i < c.len() - 1 implies #[trigger] edge(g, c[i], c[i + 1]) by {
        assert(edge(g, stk[k + i], stk[k + i + 1]));
    }
    assert(c.last() == stk.last());
    assert(is_cycle(g, c));
    let p = stk.subrange(0, k + 1);
    assert forall|i: int| 0 <= i < p.len() - 1 implies #[trigger] edge(g, p[i], p[i + 1]) by {
        assert(edge(g, stk[i], stk[i + 1]));
    }
    assert(is_path(g, p) && roots.contains(p[0]) && p.last() == c[0]);
}

/// The node at the end of a stack path is reachable.
proof fn lemma_stack_reachable(g: Seq<Vec<usize>>, roots: Seq<usize>, stk: Seq<usize>)
    requires
        is_path(g, stk),
        roots.contains(stk[0]),
    ensures
        reachable(g, roots, stk.last()),
{
}

fn visit(
    g: &Vec<Vec<usize>>,
    Ghost(roots): Ghost<Seq<usize>>,
    v: usize,
    st: &mut Vec<u8>,
    order: &mut Vec<usize>,
    Ghost(stack): Ghost<Seq<usize>>,
    Ghost(fuel): Ghost<nat>,
) -> (r: Result<(), usize>)
    requires
        wf(g@),
        v < g.len(),
        inv(g@, roots, old(st)@, old(order)@, stack),
        old(st)@[v as int] == 0,
        stack.len() == 0 ==> roots.contains(v),
        stack.len() > 0 ==> edge(g@, stack.last(), v),
        fuel == unseen(old(st)@),
    ensures
        r is Ok ==> {
            &&& inv(g@, roots, final(st)@, final(order)@, stack)
            &&& final(st)@[v as int] == 2
            &&& forall|u: int| 0 <= u < final(st)@.len() && final(st)@[u] == 0 ==> old(st)@[u] == 0
            &&& forall|u: int| 0 <= u < final(st)@.len() && old(st)@[u] == 2 ==> final(st)@[u] == 2
        },
        r is Err ==> has_reachable_cycle(g@, roots),
    decreases fuel,
{
    let ghost st0 = st@;
    let ghost stk = stack.push(v);
    st.set(v, 1);
    proof {
        lemma_unseen(st0, st@, v as int);
        assert(!stack.contains(v));
        assert forall|i: int| 0 <= i < stk.len() - 1 implies #[trigger] edge(g@, stk[i], stk[i + 1]) by {
            if i < stack.len() - 1 {
                assert(edge(g@, stack[i], stack[i + 1]));
            }
        }
        assert(stk[0] == if stack.len() == 0 { v } else { stack[0] });
        assert forall|u: usize| u < st.len() implies (st@[u as int] == 2 <==> #[trigger] order@.contains(u)) by {
            if u == v {
                assert(!order@.contains(v));
            }
        }
    }
    let n = g[v].len();
    let mut i = 0;
    while i < n
        invariant
            wf(g@),
            v < g.len(),
            n == g@[v as int]@.len(),
            0 <= i <= n,
            stk == stack.push(v),
            !stack.contains(v),
            is_path(g@, stk),
            roots.contains(stk[0]),
            inv(g@, roots, st@, order@, stk),
            st@[v as int] == 1,
            unseen(st@) < fuel,
            forall|k: int| 0 <= k < i ==> st@[#[trigger] g@[v as int]@[k] as int] == 2,
            forall|u: int| 0 <= u < st.len() && #[trigger] st@[u] == 0 ==> st0[u] == 0,
            forall|u: int| 0 <= u < st.len() && #[trigger] st0[u] == 2 ==> st@[u] == 2,
        decreases n - i,
    {
        let w = g[v][i];
        assert(edge(g@, v, w)) by {
            assert(g@[v as int]@[i as int] == w);
        }
        if st[w] == 1 {
            proof {
                assert(stk.contains(w));
                lemma_back_edge(g@, roots, stk, w);
            }
            return Err(w);
        }
        if st[w] == 0 {
            let ghost st1 = st@;
            let ghost fuel1 = unseen(st@);
            let r = visit(g, Ghost(roots), w, st, order, Ghost(stk), Ghost(fuel1));
            r?;
            proof {
                lemma_unseen(st1, st@, 0);
                assert(stk.contains(v));
                assert forall|k: int| 0 <= k < i implies st@[#[trigger] g@[v as int]@[k] as int] == 2 by {
                    assert(st1[g@[v as int]@[k] as int] == 2);
                }
                assert forall|u: int| 0 <= u < st.len() && #[trigger] st@[u] == 0 implies st0[u] == 0 by {
                    assert(st1[u] == 0);
                }
                assert forall|u: int| 0 <= u < st.len() && #[trigger] st0[u] == 2 implies st@[u] == 2 by {
                    assert(st1[u] == 2);
                }
            }
        }
        assert(st@[g@[v as int]@[i as int] as int] == 2);
        i += 1;
    }
    let ghost order0 = order@;
    let ghost st2 = st@;
    st.set(v, 2);
    order.push(v);
    proof {
        lemma_stack_reachable(g@, roots, stk);
        assert(!order0.contains(v));
        assert forall|k: int| 0 <= k < order@.len() implies reachable(g@, roots, #[trigger] order@[k]) by {
            if k < order0.len() {
                assert(order@[k] == order0[k]);
            }
        }
        assert forall|k: int| 0 <= k < order@.len() implies #[trigger] order@[k] < g.len() by {
            if k < order0.len() {
                assert(order@[k] == order0[k]);
            }
        }
        assert forall|u: usize| u < st.len() implies (st@[u as int] == 2 <==> #[trigger] order@.contains(u)) by {
            if u != v {
                assert(st@[u as int] == st2[u as int]);
            }
        }
        assert forall|u: usize| u < st.len() implies (st@[u as int] == 1 <==> #[trigger] stack.contains(u)) by {
            if u != v {
                assert(st@[u as int] == st2[u as int]);
                assert(stk.contains(u) <==> stack.contains(u));
            }
        }
        assert(order@.no_duplicates()) by {
            assert forall|a: int, b: int| 0 <= a < b < order@.len() implies order@[a] != order@[b] by {
                if b == order0.len() {
                    assert(order@[a] == order0[a]);
                    assert(order0.contains(order0[a]));
                }
            }
        }
        assert forall|j: int, x: usize|
            0 <= j < order@.len() && #[trigger] edge(g@, order@[j], x) implies order@.subrange(0, j).contains(x) by {
            if j < order0.len() {
                assert(order@[j] == order0[j]);
                assert(order@.subrange(0, j) =~= order0.subrange(0, j));
            } else {
                let k = choose|k: int| 0 <= k < g@[v as int]@.len() && g@[v as int]@[k] == x;
                assert(st2[x as int] == 2);
                assert(order@.subrange(0, j) =~= order0);
            }
        }
        if stack.len() > 0 {
            assert forall|i: int| 0 <= i < stack.len() - 1 implies #[trigger] edge(g@, stack[i], stack[i + 1]) by {
                assert(edge(g@, stk[i], stk[i + 1]));
            }
            assert(stack[0] == stk[0]);
        }
        assert forall|k: int| 0 <= k < stack.len() implies #[trigger] stack[k] < g.len() by {
            assert(stack[k] == stk[k]);
        }
    }
    Ok(())
}

/// A path from a node of a topological order stays in the order.
proof fn lemma_closed_path(g: Seq<Vec<usize>>, order: Seq<usize>, p: Seq<usize>, k: int)
    requires
        topological(g, order),
        is_path(g, p),
        order.contains(p[0]),
        0 <= k < p.len(),
    ensures
        order.contains(p[k]),
    decreases k,
{
    if k > 0 {
        lemma_closed_path(g, order, p, k - 1);
        let i = choose|i: int| 0 <= i < order.len() && order[i] == p[k - 1];
        let h = k - 1;
        assert(edge(g, p[h], p[h + 1]));
        assert(edge(g, order[i], p[k]));
        let j = choose|j: int| 0 <= j < i && order.subrange(0, i)[j] == p[k];
        assert(order[j] == p[k]);
    }
}

/// The position of `x` in `order`.
spec fn pos(order: Seq<usize>, x: usize) -> int {
    choose|i: int| 0 <= i < order.len() && order[i] == x
}

/// A dependency comes strictly earlier.
proof fn lemma_edge_pos(g: Seq<Vec<usize>>, order: Seq<usize>, a: usize, b: usize)
    requires
        topological(g, order),
        order.contains(a),
        edge(g, a, b),
    ensures
        order.contains(b),
        pos(order, b) < pos(order, a),
{
    let i = pos(order, a);
    assert(edge(g, order[i], b));
    let j = choose|j: int| 0 <= j < i && order.subrange(0, i)[j] == b;
    assert(order[j] == b);
    let pb = pos(order, b);
    assert(order[pb] == b);
    assert(pb == j);
}

/// Along a path in a topological order, positions strictly fall.
proof fn lemma_path_pos(g: Seq<Vec<usize>>, order: Seq<usize>, c: Seq<usize>, k: int)
    requires
        topological(g, order),
        is_path(g, c),
        order.contains(c[0]),
        0 < k < c.len(),
    ensures
        order.contains(c[k]),
        pos(order, c[k]) < pos(order, c[0]),
    decreases k,
{
    lemma_closed_path(g, order, c, k - 1);
    let h = k - 1;
    assert(edge(g, c[h], c[h + 1]));
    lemma_edge_pos(g, order, c[k - 1], c[k]);
    if k > 1 {
        lemma_path_pos(g, order, c, k - 1);
    }
}

/// A topological order holding every reachable node rules out a reachable
/// cycle.
proof fn lemma_no_cycle(g: Seq<Vec<usize>>, roots: Seq<usize>, order: Seq<usize>)
    requires
        topological(g, order),
        forall|v: usize| #[trigger] reachable(g, roots, v) ==> order.contains(v),
    ensures
        !has_reachable_cycle(g, roots),
{
    if has_reachable_cycle(g, roots) {
        let c = choose|c: Seq<usize>| #[trigger] is_cycle(g, c) && reachable(g, roots, c[0]);
        assert(order.contains(c[0]));
        if c.len() > 1 {
            lemma_path_pos(g, order, c, c.len() - 1);
        }
        lemma_edge_pos(g, order, c.last(), c[0]);
    }
}

/// The dependency build order of `roots` in graph `g`: `Ok(order)`, the DFS
/// post-order, holds exactly the nodes reachable from `roots`, each once and
/// after all of its dependencies; `Err(w)`, at a node `w` on a cycle,
/// exactly when a cycle is reachable from `roots`.
pub fn build_order(g: &Vec<Vec<usize>>, roots: &[usize]) -> (r: Result<Vec<usize>, usize>)
    requires
        wf(g@),
        forall|k: int| 0 <= k < roots.len() ==> #[trigger] roots[k] < g.len(),
    ensures
        r is Err <==> has_reachable_cycle(g@, roots@),
        r matches Ok(order) ==> {
            &&& topological(g@, order@)
            &&& forall|v: usize| v < g.len() ==> (#[trigger] order@.contains(v) <==> reachable(g@, roots@, v))
            &&& forall|k: int| 0 <= k < order.len() ==> #[trigger] order@[k] < g.len()
        },
{
    let n = g.len();
    let mut st: Vec<u8> = Vec::with_capacity(n);
    while st.len() < n
        invariant
            st.len() <= n,
            forall|u: int| 0 <= u < st.len() ==> st@[u] == 0,
        decreases n - st.len(),
    {
        st.push(0);
    }
    let mut order: Vec<usize> = Vec::new();
    let ghost empty = Seq::<usize>::empty();
    let mut t = 0;
    while t < roots.len()
        invariant
            wf(g@),
            n == g.len(),
            forall|k: int| 0 <= k < roots.len() ==> #[trigger] roots[k] < g.len(),
            0 <= t <= roots.len(),
            empty == Seq::<usize>::empty(),
            inv(g@, roots@, st@, order@, empty),
            forall|k: int| 0 <= k < t ==> st@[#[trigger] roots@[k] as int] == 2,
        decreases roots.len() - t,
    {
        let v = roots[t];
        if st[v] == 0 {
            assert(roots@.contains(v)) by {
                assert(roots@[t as int] == v);
            }
            let ghost fuel = unseen(st@);
            let r = visit(g, Ghost(roots@), v, &mut st, &mut order, Ghost(empty), Ghost(fuel));
            r?;
        } else {
            assert(!empty.contains(v));
        }
        assert(st@[roots@[t as int] as int] == 2);
        t += 1;
    }
    proof {
        assert forall|v: usize| #[trigger] reachable(g@, roots@, v) implies order@.contains(v) by {
            let p = choose|p: Seq<usize>| #[trigger] is_path(g@, p) && roots@.contains(p[0]) && p.last() == v;
            let k = choose|k: int| 0 <= k < roots.len() && roots@[k] == p[0];
            assert(st@[roots@[k] as int] == 2);
            lemma_closed_path(g@, order@, p, p.len() - 1);
        }
        lemma_no_cycle(g@, roots@, order@);
        assert forall|v: usize| v < g.len() implies (#[trigger] order@.contains(v) <==> reachable(g@, roots@, v)) by {
            if order@.contains(v) {
                let k = choose|k: int| 0 <= k < order.len() && order@[k] == v;
                assert(reachable(g@, roots@, order@[k]));
            }
        }
    }
    Ok(order)
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine's former `derived_build_order`, over indices: recursive DFS,
    /// error at the first dependency found on the stack.
    fn reference(g: &[Vec<usize>], roots: &[usize]) -> Result<Vec<usize>, usize> {
        fn go(
            g: &[Vec<usize>],
            v: usize,
            order: &mut Vec<usize>,
            done: &mut [bool],
            on: &mut [bool],
        ) -> Result<(), usize> {
            if done[v] {
                return Ok(());
            }
            if on[v] {
                return Err(v);
            }
            on[v] = true;
            for &w in &g[v] {
                go(g, w, order, done, on)?;
            }
            on[v] = false;
            done[v] = true;
            order.push(v);
            Ok(())
        }
        let (mut order, mut done, mut on) = (vec![], vec![false; g.len()], vec![false; g.len()]);
        for &r in roots {
            go(g, r, &mut order, &mut done, &mut on)?;
        }
        Ok(order)
    }

    #[test]
    fn orders_and_rejects_cycles() {
        let g = vec![vec![1, 2], vec![2], vec![], vec![0]];
        assert_eq!(build_order(&g, &[0]), Ok(vec![2, 1, 0]));
        assert_eq!(build_order(&g, &[3, 0]), Ok(vec![2, 1, 0, 3]));
        assert_eq!(build_order(&g, &[]), Ok(vec![]));
        let cyc = vec![vec![1], vec![0], vec![]];
        assert_eq!(build_order(&cyc, &[2]), Ok(vec![2]));
        assert_eq!(build_order(&cyc, &[0]), Err(0));
    }

    /// Random small graphs (with duplicate edges and cycles): the same order,
    /// and the same error node, as the reference.
    #[test]
    fn matches_reference_dfs() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |m: usize| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % m as u64) as usize
        };
        for _ in 0..20_000 {
            let n = 1 + next(8);
            let mut g: Vec<Vec<usize>> = vec![Vec::new(); n];
            for (v, deps) in g.iter_mut().enumerate() {
                for _ in 0..next(4) {
                    let w = next(n);
                    if w != v || next(4) == 0 {
                        deps.push(w);
                    }
                }
            }
            let roots: Vec<usize> = (0..next(4)).map(|_| next(n)).collect();
            assert_eq!(
                build_order(&g, &roots),
                reference(&g, &roots),
                "{g:?} {roots:?}"
            );
        }
    }
}
