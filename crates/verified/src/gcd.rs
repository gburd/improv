//! GCD and LCM (the spreadsheet `GCD` / `LCM` functions).

#[allow(unused_imports)]
use vstd::arithmetic::div_mod::*;
#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

/// `d` divides `n`: `n` is an integer multiple of `d`.
pub open spec fn divides(d: int, n: int) -> bool {
    exists|k: int| #[trigger] (d * k) == n
}

/// Euclid's algorithm, as the specification of the greatest common divisor.
pub open spec fn gcd_spec(a: nat, b: nat) -> nat
    decreases b,
{
    if b == 0 {
        a
    } else {
        gcd_spec(b, a % b)
    }
}

/// `gcd_spec(a, b)` divides both `a` and `b`.
pub proof fn lemma_gcd_divides(a: nat, b: nat)
    ensures
        divides(gcd_spec(a, b) as int, a as int),
        divides(gcd_spec(a, b) as int, b as int),
    decreases b,
{
    if b == 0 {
        let g = gcd_spec(a, b) as int;
        assert(g * 1 == a as int);
        assert(g * 0 == b as int);
    } else {
        lemma_gcd_divides(b, a % b);
        let g = gcd_spec(b, a % b) as int;
        let k1 = choose|k: int| #[trigger] (g * k) == b as int;
        let k2 = choose|k: int| #[trigger] (g * k) == (a % b) as int;
        lemma_fundamental_div_mod(a as int, b as int);
        let q = a as int / b as int;
        assert(g * (q * k1 + k2) == a as int) by (nonlinear_arith)
            requires
                g * k1 == b as int,
                g * k2 == (a % b) as int,
                a as int == b as int * q + (a % b) as int,
        ;
    }
}

/// Every common divisor of `a` and `b` divides `gcd_spec(a, b)`.
pub proof fn lemma_gcd_greatest(a: nat, b: nat, d: int)
    requires
        divides(d, a as int),
        divides(d, b as int),
    ensures
        divides(d, gcd_spec(a, b) as int),
    decreases b,
{
    if b != 0 {
        let ka = choose|k: int| #[trigger] (d * k) == a as int;
        let kb = choose|k: int| #[trigger] (d * k) == b as int;
        lemma_fundamental_div_mod(a as int, b as int);
        let q = a as int / b as int;
        assert(d * (ka - kb * q) == (a % b) as int) by (nonlinear_arith)
            requires
                d * ka == a as int,
                d * kb == b as int,
                a as int == b as int * q + (a % b) as int,
        ;
        lemma_gcd_greatest(b, a % b, d);
    }
}

/// The greatest common divisor of `a` and `b` (Euclid): it divides both, and
/// every common divisor divides it. `gcd(a, 0) == a`, so folding from 0 gives
/// the gcd of a list.
pub fn gcd(a: u64, b: u64) -> (g: u64)
    ensures
        g == gcd_spec(a as nat, b as nat),
        divides(g as int, a as int),
        divides(g as int, b as int),
        forall|d: int| divides(d, a as int) && divides(d, b as int) ==> #[trigger] divides(d, g as int),
        b == 0 ==> g == a,
{
    let mut x = a;
    let mut y = b;
    while y != 0
        invariant
            gcd_spec(x as nat, y as nat) == gcd_spec(a as nat, b as nat),
        decreases y,
    {
        let t = x % y;
        x = y;
        y = t;
    }
    proof {
        lemma_gcd_divides(a as nat, b as nat);
        assert forall|d: int| divides(d, a as int) && divides(d, b as int) implies #[trigger] divides(
            d,
            x as int,
        ) by {
            lemma_gcd_greatest(a as nat, b as nat, d);
        }
    }
    x
}

/// The least common multiple as the engine computes it: `a / gcd(a, b) * b`.
pub open spec fn lcm_spec(a: nat, b: nat) -> int {
    (a / gcd_spec(a, b)) as int * b as int
}

/// `lcm(a, b)` if it is below `cap`, else `None`. Positive `a` and `b`; the
/// product is formed in `u128`, so the overflow check is exact: `None` exactly
/// when the true lcm is at least `cap`. A result is a positive common multiple
/// of `a` and `b`.
pub fn lcm_below(a: u64, b: u64, cap: u64) -> (r: Option<u64>)
    requires
        a > 0,
        b > 0,
    ensures
        r is None <==> lcm_spec(a as nat, b as nat) >= cap,
        r matches Some(m) ==> m as int == lcm_spec(a as nat, b as nat) && m < cap && m > 0
            && divides(a as int, m as int) && divides(b as int, m as int),
{
    let g = gcd(a, b);
    proof {
        // g divides a > 0, so g > 0 and a == g * (a / g).
        let k = choose|k: int| #[trigger] (g as int * k) == a as int;
        assert(g > 0) by (nonlinear_arith)
            requires
                g as int * k == a as int,
                a > 0,
                g >= 0,
        ;
        lemma_div_multiples_vanish(k, g as int);
        assert(a as int / g as int == k);
    }
    let q = a / g;
    proof {
        assert(q as int * b as int <= u64::MAX as int * u64::MAX as int) by (nonlinear_arith)
            requires
                q <= u64::MAX,
                b <= u64::MAX,
        ;
    }
    let m = (q as u128) * (b as u128);
    if m < cap as u128 {
        proof {
            let k = choose|k: int| #[trigger] (g as int * k) == a as int;
            let kb = choose|k: int| #[trigger] (g as int * k) == b as int;
            assert(a as int * kb == m as int) by (nonlinear_arith)
                requires
                    g as int * k == a as int,
                    g as int * kb == b as int,
                    q as int == k,
                    m as int == q as int * b as int,
            ;
            assert(b as int * q as int == m as int) by (nonlinear_arith)
                requires
                    m as int == q as int * b as int,
            ;
            assert(m > 0) by (nonlinear_arith)
                requires
                    q as int == k,
                    g as int * k == a as int,
                    a > 0,
                    g > 0,
                    b > 0,
                    m as int == q as int * b as int,
            ;
        }
        Some(m as u64)
    } else {
        None
    }
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcd_matches_euclid() {
        assert_eq!(gcd(0, 0), 0);
        assert_eq!(gcd(12, 0), 12);
        assert_eq!(gcd(0, 12), 12);
        assert_eq!(gcd(12, 18), 6);
        assert_eq!(gcd(17, 5), 1);
    }

    #[test]
    fn lcm_caps_exactly() {
        assert_eq!(lcm_below(4, 6, 100), Some(12));
        assert_eq!(lcm_below(4, 6, 12), None);
        assert_eq!(lcm_below(4, 6, 13), Some(12));
        assert_eq!(lcm_below(u64::MAX, u64::MAX - 1, u64::MAX), None);
    }
}
