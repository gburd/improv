//! Number theory (Gnumeric's `fn-numtheory`), bounded so every call stays
//! fast: n up to 2⁵² (Gnumeric's `1/DBL_EPSILON`), factored by trial
//! division then Pollard–Brent ρ with a deterministic Miller–Rabin test;
//! π(n) by Lucy Hedgehog's O(n^¾) sieve; the i-th prime from π below a
//! Dusart bound plus a short segmented sieve.

use super::super::{domain, err, ok_num, R};
use crate::CellValue;

/// Gnumeric's bound for exact integer work: 1/DBL_EPSILON = 2⁵².
const MAX_N: f64 = 4_503_599_627_370_496.0;
/// The i-th prime and π stop at the 10⁷-th prime (179 424 673), so π
/// costs about a millisecond. ponytail: Gnumeric's table goes to 10⁸
/// (π to 2 038 074 743), which costs 10 ms here; raise these with a
/// faster π (Meissel–Lehmer) if anyone needs it.
const MAX_I: f64 = 1e7;
const MAX_PI: f64 = 179_424_673.0;
/// ponytail: p(n) is exact in 256 bits only to about n = 4400 (p(4000) ≈
/// 1e66); a double holds p(n) to n ≈ 78 000, which needs a float
/// recurrence (Gnumeric 1.12.60+ goes to 10⁵ that way, in O(n²)).
const MAX_PARTITIONS: f64 = 4000.0;

/// Evaluate number-theory function `id` on the truncated argument `x`.
pub(super) fn eval(id: u32, x: f64) -> R {
    let pos = || -> Result<u64, u8> {
        domain((1.0..=MAX_N).contains(&x))?;
        Ok(x as u64)
    };
    match id {
        1460 => {
            domain(x <= MAX_N)?;
            Ok(CellValue::Bool(x >= 0.0 && is_prime(x as u64)))
        }
        1461 => {
            domain((1.0..=MAX_I).contains(&x))?;
            ok_num(ith_prime(x as u64) as f64)
        }
        1462 => {
            if x < 2.0 {
                return Err(err::VALUE);
            }
            domain(x <= MAX_N)?;
            ok_num(factor(x as u64)[0].0 as f64)
        }
        1463 => ok_num(fold(pos()?, |acc, _, e| acc * f64::from(e + 1))),
        1464 => ok_num(fold(pos()?, |acc, p, e| {
            // (p^(e+1) − 1)/(p − 1) < 2⁵³·p fits u128.
            let p = u128::from(p);
            acc * ((p.pow(e + 1) - 1) / (p - 1)) as f64
        })),
        1465 => ok_num(fold(pos()?, |acc, p, e| {
            acc * (p.pow(e - 1) * (p - 1)) as f64
        })),
        1466 => ok_num(fold(pos()?, |acc, _, e| if e > 1 { 0.0 } else { -acc })),
        1467 => ok_num(factor(pos()?).len() as f64),
        1468 => {
            domain(x <= MAX_PI)?;
            ok_num(if x < 2.0 {
                0.0
            } else {
                prime_pi(x as u64) as f64
            })
        }
        1469 => ok_num(fold(pos()?, |acc, p, _| acc * p as f64)),
        _ => {
            domain((0.0..=MAX_PARTITIONS).contains(&x))?;
            ok_num(partitions(x as usize))
        }
    }
}

/// Fold `f(acc, p, e)` from 1 over n's factorization pᵉ….
fn fold(n: u64, f: impl Fn(f64, u64, u32) -> f64) -> f64 {
    factor(n).iter().fold(1.0, |acc, &(p, e)| f(acc, p, e))
}

fn mul_mod(a: u64, b: u64, m: u64) -> u64 {
    (u128::from(a) * u128::from(b) % u128::from(m)) as u64
}

fn pow_mod(mut b: u64, mut e: u64, m: u64) -> u64 {
    let mut r = 1;
    while e > 0 {
        if e & 1 == 1 {
            r = mul_mod(r, b, m);
        }
        b = mul_mod(b, b, m);
        e >>= 1;
    }
    r
}

const SMALL: [u64; 12] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37];

/// Miller–Rabin with the first 12 prime bases: deterministic below 3.3·10²⁴.
fn is_prime(n: u64) -> bool {
    if n < 2 {
        return false;
    }
    if let Some(&p) = SMALL.iter().find(|&&p| n.is_multiple_of(p)) {
        return n == p;
    }
    let s = (n - 1).trailing_zeros();
    let d = (n - 1) >> s;
    'bases: for a in SMALL {
        let mut x = pow_mod(a, d, n);
        if x == 1 || x == n - 1 {
            continue;
        }
        for _ in 1..s {
            x = mul_mod(x, x, n);
            if x == n - 1 {
                continue 'bases;
            }
        }
        return false;
    }
    true
}

use improv_verified::gcd::gcd;

/// A proper factor of the odd composite n (Brent's ρ, batched gcds).
fn rho(n: u64) -> u64 {
    for c in 1.. {
        let f = |x: u64| (mul_mod(x, x, n) + c) % n;
        let (mut x, mut y, mut ys, mut q, mut g, mut r) = (0, 2, 2, 1, 1, 1u32);
        while g == 1 {
            x = y;
            for _ in 0..r {
                y = f(y);
            }
            let mut k = 0;
            while k < r && g == 1 {
                ys = y;
                for _ in 0..128.min(r - k) {
                    y = f(y);
                    q = mul_mod(q, x.abs_diff(y), n);
                }
                g = gcd(q, n);
                k += 128;
            }
            r *= 2;
        }
        if g == n {
            // The batch overshot: step again one at a time.
            loop {
                ys = f(ys);
                g = gcd(x.abs_diff(ys), n);
                if g > 1 {
                    break;
                }
            }
        }
        if g != n {
            return g;
        }
    }
    unreachable!("some c splits a composite")
}

/// n ≥ 1 as (prime, exponent) pairs, primes ascending.
fn factor(mut n: u64) -> Vec<(u64, u32)> {
    let mut ps = Vec::new();
    let mut p = 2;
    while p < 1000 && p * p <= n {
        while n.is_multiple_of(p) {
            ps.push(p);
            n /= p;
        }
        p += 1 + (p & 1);
    }
    let mut stack = vec![n];
    while let Some(m) = stack.pop() {
        if m == 1 {
        } else if is_prime(m) {
            ps.push(m);
        } else {
            let d = rho(m);
            stack.extend([d, m / d]);
        }
    }
    ps.sort_unstable();
    let mut out: Vec<(u64, u32)> = Vec::new();
    for p in ps {
        match out.last_mut() {
            Some((q, e)) if *q == p => *e += 1,
            _ => out.push((p, 1)),
        }
    }
    out
}

/// π(n) for 2 ≤ n < 2³² (Lucy Hedgehog): S(v) counts the survivors in
/// 2..=v, kept for v ≤ √n (`lo`) and v = n/i (`hi`), sieved prime by
/// prime. In u32, which halves the memory traffic and the division cost.
fn prime_pi(n: u64) -> u64 {
    let n = u32::try_from(n).expect("π's argument is below 2³²");
    let r = n.isqrt() as usize;
    let mut lo: Vec<u32> = (0..=r as u32).map(|v| v.saturating_sub(1)).collect();
    let mut hi: Vec<u32> = (0..=r as u32)
        .map(|i| (n / i.max(1)).saturating_sub(1))
        .collect();
    for p in 2..=r {
        if lo[p] == lo[p - 1] {
            continue;
        }
        let (sp, p2) = (lo[p - 1], (p * p) as u32);
        let top = r.min((n / p2) as usize);
        // i·p ≤ r: S(n/(i·p)) is hi[i·p]; beyond, it is lo[n/(i·p)].
        let split = top.min(r / p);
        for i in 1..=split {
            hi[i] -= hi[i * p] - sp;
        }
        for i in split + 1..=top {
            hi[i] -= lo[(n / (i * p) as u32) as usize] - sp;
        }
        for v in (p * p..=r).rev() {
            lo[v] -= lo[v / p] - sp;
        }
    }
    u64::from(hi[1])
}

/// Which of lo..lo+len are prime.
fn sieve(lo: u64, len: usize) -> Vec<bool> {
    let hi = lo + len as u64;
    let r = hi.isqrt() as usize + 1;
    let mut small = vec![true; r + 1];
    let mut out: Vec<bool> = (lo..hi).map(|v| v >= 2).collect();
    for p in 2..=r {
        if !small[p] {
            continue;
        }
        for m in (p * p..=r).step_by(p) {
            small[m] = false;
        }
        let p = p as u64;
        let start = (p * p).max(lo.div_ceil(p) * p);
        for m in (start..hi).step_by(p as usize) {
            out[(m - lo) as usize] = false;
        }
    }
    out
}

/// The i-th prime: π at a Dusart lower bound (p_i ≥ i(ln i + ln ln i − 1 +
/// (ln ln i − 2.1)/ln i) for i ≥ 2), then count up through 2¹⁶-wide
/// segments (the bound is within 0.2% of p_i).
fn ith_prime(i: u64) -> u64 {
    let mut x = if i < 6 {
        0
    } else {
        let (l, f) = ((i as f64).ln(), i as f64);
        let ll = l.ln();
        (f * (l + ll - 1.0 + (ll - 2.1) / l)) as u64 - 1
    };
    let mut count = if x < 2 { 0 } else { prime_pi(x) };
    loop {
        for (k, p) in sieve(x + 1, 1 << 16).into_iter().enumerate() {
            count += u64::from(p);
            if count == i {
                return x + 1 + k as u64;
            }
        }
        x += 1 << 16;
    }
}

/// p(n) by Euler's pentagonal recurrence in exact 256-bit integers
/// (wrapping: the true value fits, so modular sums are exact).
fn partitions(n: usize) -> f64 {
    let mut p = vec![[0u64; 4]; n + 1];
    p[0][0] = 1;
    for m in 1..=n {
        let mut acc = [0u64; 4];
        for k in 1.. {
            let g1 = k * (3 * k - 1) / 2;
            if g1 > m {
                break;
            }
            let neg = k % 2 == 0;
            wrap(&mut acc, &p[m - g1], neg);
            let g2 = k * (3 * k + 1) / 2;
            if g2 <= m {
                wrap(&mut acc, &p[m - g2], neg);
            }
        }
        p[m] = acc;
    }
    p[n].iter()
        .rev()
        .fold(0.0, |acc, &w| acc * 18_446_744_073_709_551_616.0 + w as f64)
}

/// acc ± v modulo 2²⁵⁶.
fn wrap(acc: &mut [u64; 4], v: &[u64; 4], neg: bool) {
    let mut carry = false;
    for (a, &w) in acc.iter_mut().zip(v) {
        let (s1, c1, s2, c2);
        if neg {
            (s1, c1) = a.overflowing_sub(w);
            (s2, c2) = s1.overflowing_sub(u64::from(carry));
        } else {
            (s1, c1) = a.overflowing_add(w);
            (s2, c2) = s1.overflowing_add(u64::from(carry));
        }
        *a = s2;
        carry = c1 || c2;
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn primes_agree_with_a_sieve() {
        let s = sieve(0, 20_000);
        let mut count = 0;
        for (v, &p) in s.iter().enumerate() {
            assert_eq!(is_prime(v as u64), p, "{v}");
            count += u64::from(p);
            if v >= 2 && v % 997 == 0 {
                assert_eq!(prime_pi(v as u64), count, "π({v})");
            }
            if p {
                assert_eq!(ith_prime(count), v as u64);
            }
        }
    }

    #[test]
    fn factoring_hard_cases() {
        // Two primes near 2²⁶ (ρ's worst case), a square, a prime power,
        // a Carmichael number, and 2⁵².
        let (p, q) = (67_108_859u64, 67_108_837u64);
        assert_eq!(factor(p * q), vec![(q, 1), (p, 1)]);
        assert_eq!(factor(p * p), vec![(p, 2)]);
        assert_eq!(factor(3u64.pow(30)), vec![(3, 30)]);
        assert_eq!(factor(561), vec![(3, 1), (11, 1), (17, 1)]);
        assert_eq!(factor(1 << 52), vec![(2, 52)]);
        assert_eq!(factor(1), vec![]);
        // Brent's batch overshoots to g = n and the one-at-a-time replay
        // takes several steps (1009·1049); every step of c = 1 cycles
        // without splitting, so c = 2 does it (1009·1709). Gnumeric 1.12
        // PFACTOR: 1009 for both.
        assert_eq!(factor(1009 * 1049), vec![(1009, 1), (1049, 1)]);
        assert_eq!(factor(1009 * 1709), vec![(1009, 1), (1709, 1)]);
        for n in 2..3000u64 {
            let back: u64 = factor(n).iter().map(|&(p, e)| p.pow(e)).product();
            assert_eq!(back, n);
        }
    }

    #[test]
    fn partitions_are_exact() {
        assert_eq!(partitions(0), 1.0);
        assert_eq!(partitions(100), 190_569_292.0);
        assert_eq!(partitions(200), 3_972_999_029_388.0);
    }
}
