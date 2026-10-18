//! Base conversion between integers and digit vectors (most significant
//! first): `BASE`, and the 10-digit two's complement of `DEC2BIN` / `BIN2DEC`
//! and the rest of the radix 2 / 8 / 16 family.

#[allow(unused_imports)]
use vstd::arithmetic::div_mod::*;
#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

/// `r` to the `e`.
pub open spec fn pw(r: nat, e: nat) -> nat
    decreases e,
{
    if e == 0 {
        1
    } else {
        r * pw(r, (e - 1) as nat)
    }
}

/// The value of digits `ds` (most significant first) in radix `r` (Horner).
pub open spec fn value(ds: Seq<u8>, r: nat) -> nat
    decreases ds.len(),
{
    if ds.len() == 0 {
        0
    } else {
        value(ds.drop_last(), r) * r + ds.last() as nat
    }
}

/// Every digit is below `r`.
pub open spec fn digits_below(ds: Seq<u8>, r: nat) -> bool {
    forall|i: int| 0 <= i < ds.len() ==> (#[trigger] ds[i] as nat) < r
}

/// The radixes of the two's-complement functions.
pub open spec fn tc_radix(r: u32) -> bool {
    r == 2 || r == 8 || r == 16
}

/// What 10-digit two's-complement digits in radix `r` mean: nothing if there
/// are more than 10 or a digit is not below `r`; ten digits led by one of at
/// least `r / 2` are negative (the value less `r¹⁰`); otherwise the value.
pub open spec fn decode_spec(ds: Seq<u8>, r: nat) -> Option<int> {
    if ds.len() > 10 || !digits_below(ds, r) {
        None
    } else if ds.len() == 10 && ds[0] as nat >= r / 2 {
        Some(value(ds, r) - pw(r, 10))
    } else {
        Some(value(ds, r) as int)
    }
}

proof fn lemma_pw_pos(r: nat, e: nat)
    requires
        r >= 1,
    ensures
        pw(r, e) >= 1,
    decreases e,
{
    if e > 0 {
        lemma_pw_pos(r, (e - 1) as nat);
        let p = pw(r, (e - 1) as nat);
        assert(r * p >= 1) by (nonlinear_arith)
            requires
                r >= 1,
                p >= 1,
        ;
    }
}

proof fn lemma_pw_mono(r: nat, a: nat, b: nat)
    requires
        r >= 1,
        a <= b,
    ensures
        pw(r, a) <= pw(r, b),
    decreases b,
{
    if a < b {
        lemma_pw_mono(r, a, (b - 1) as nat);
        let p = pw(r, (b - 1) as nat);
        assert(p <= r * p) by (nonlinear_arith)
            requires
                r >= 1,
        ;
    }
}

/// Digits below `r` are worth less than `r` to their count.
proof fn lemma_value_bound(ds: Seq<u8>, r: nat)
    requires
        digits_below(ds, r),
    ensures
        value(ds, r) < pw(r, ds.len()),
    decreases ds.len(),
{
    if ds.len() > 0 {
        let p = ds.drop_last();
        assert(digits_below(p, r)) by {
            assert forall|i: int| 0 <= i < p.len() implies (#[trigger] p[i] as nat) < r by {
                assert(p[i] == ds[i]);
            }
        }
        lemma_value_bound(p, r);
        let v = value(p, r);
        let d = ds.last() as nat;
        assert(d < r);
        let q = pw(r, p.len());
        assert(pw(r, ds.len()) == r * q);
        assert(value(ds, r) == v * r + d);
        assert(v * r + d < r * q) by (nonlinear_arith)
            requires
                v < q,
                d < r,
        ;
    }
}

/// A leading digit is worth `d · r^len(ds)`.
proof fn lemma_value_prepend(d: u8, ds: Seq<u8>, r: nat)
    ensures
        value(seq![d] + ds, r) == d as nat * pw(r, ds.len()) + value(ds, r),
    decreases ds.len(),
{
    let s = seq![d] + ds;
    let dn = d as nat;
    if ds.len() == 0 {
        assert(s.drop_last() =~= Seq::<u8>::empty());
        assert(s.last() == d);
        let z = value(s.drop_last(), r);
        assert(z == 0);
        assert(value(s, r) == z * r + dn);
        assert(pw(r, 0) == 1);
        assert(z * r + dn == dn * 1 + 0) by (nonlinear_arith)
            requires
                z == 0,
        ;
    } else {
        let t = ds.drop_last();
        assert(s.drop_last() =~= seq![d] + t);
        assert(s.last() == ds.last());
        lemma_value_prepend(d, t, r);
        let p = pw(r, t.len());
        let w = value(t, r);
        let l = ds.last() as nat;
        assert(pw(r, ds.len()) == r * p);
        assert(value(ds, r) == w * r + l);
        assert(value(s, r) == value(seq![d] + t, r) * r + l);
        assert((dn * p + w) * r + l == dn * (r * p) + (w * r + l)) by (nonlinear_arith);
    }
}

/// The leading digit pins the value between `d · r^(n−1)` and
/// `(d + 1) · r^(n−1)`.
proof fn lemma_value_first(ds: Seq<u8>, r: nat)
    requires
        ds.len() >= 1,
        digits_below(ds, r),
    ensures
        ds[0] as nat * pw(r, (ds.len() - 1) as nat) <= value(ds, r),
        value(ds, r) < (ds[0] as nat + 1) * pw(r, (ds.len() - 1) as nat),
{
    let rest = ds.drop_first();
    assert(ds =~= seq![ds[0]] + rest);
    lemma_value_prepend(ds[0], rest, r);
    assert(digits_below(rest, r)) by {
        assert forall|i: int| 0 <= i < rest.len() implies (#[trigger] rest[i] as nat) < r by {
            assert(rest[i] == ds[i + 1]);
        }
    }
    lemma_value_bound(rest, r);
    let p = pw(r, rest.len());
    let d = ds[0] as nat;
    let w = value(rest, r);
    assert(d * p + w < (d + 1) * p) by (nonlinear_arith)
        requires
            w < p,
    ;
}

/// `u` in radix `r` (2…36): its digits, most significant first, without
/// leading zeros (`0` is one digit). Each digit is below `r` and they are
/// worth exactly `u`.
pub fn to_digits(u: u64, r: u32) -> (ds: Vec<u8>)
    requires
        2 <= r <= 36,
    ensures
        value(ds@, r as nat) == u,
        digits_below(ds@, r as nat),
        ds.len() >= 1,
        ds.len() == 1 || ds@[0] != 0,
{
    let rr = r as u64;
    let mut ds: Vec<u8> = Vec::new();
    let mut x = u;
    proof {
        let w = value(ds@, r as nat);
        assert(w == 0);
        assert(pw(r as nat, 0) == 1);
        assert(u as nat == x as nat * 1 + w);
    }
    loop
        invariant_except_break
            ds.len() == 0 || x != 0,
        invariant
            2 <= r <= 36,
            rr == r,
            u as nat == x as nat * pw(r as nat, ds.len() as nat) + value(ds@, r as nat),
            digits_below(ds@, r as nat),
        ensures
            value(ds@, r as nat) == u,
            ds.len() >= 1,
            ds.len() == 1 || ds@[0] != 0,
        decreases x,
    {
        let ghost x0 = x;
        let ghost ds0 = ds@;
        let d = (x % rr) as u8;
        x /= rr;
        ds.insert(0, d);
        proof {
            let rn = r as nat;
            let p = pw(rn, ds0.len());
            let v0 = value(ds0, rn);
            lemma_fundamental_div_mod(x0 as int, rr as int);
            lemma_mod_bound(x0 as int, rr as int);
            assert(d as int == x0 as int % rr as int);
            assert(x as int == x0 as int / rr as int);
            assert(ds@ =~= seq![d] + ds0);
            lemma_value_prepend(d, ds0, rn);
            assert(pw(rn, ds@.len()) == rn * p);
            assert(u as nat == x as nat * (rn * p) + (d as nat * p + v0)) by (nonlinear_arith)
                requires
                    x0 as int == rn as int * x as int + d as int,
                    u as nat == x0 as nat * p + v0,
            ;
            assert forall|i: int| 0 <= i < ds@.len() implies (#[trigger] ds@[i] as nat) < rn by {
                if i > 0 {
                    assert(ds@[i] == ds0[i - 1]);
                }
            }
            if x0 > 0 {
                lemma_div_decreases(x0 as int, rr as int);
            }
            if x == 0 {
                assert(x0 as int == d as int) by (nonlinear_arith)
                    requires
                        x0 as int == rn as int * x as int + d as int,
                        x == 0,
                ;
                let w = value(ds@, rn);
                let q = pw(rn, ds@.len());
                assert(u as nat == w) by (nonlinear_arith)
                    requires
                        u as nat == x as nat * q + w,
                        x == 0,
                ;
            }
        }
        if x == 0 {
            break;
        }
    }
    ds
}

/// `r¹⁰` for a two's-complement radix.
fn full10(r: u32) -> (f: i64)
    requires
        tc_radix(r),
    ensures
        f == pw(r as nat, 10),
        f <= 0x100_0000_0000,
{
    if r == 2 {
        assert(pw(2, 10) == 1024) by (compute);
        1024
    } else if r == 8 {
        assert(pw(8, 10) == 0x4000_0000) by (compute);
        0x4000_0000
    } else {
        assert(pw(16, 10) == 0x100_0000_0000) by (compute);
        0x100_0000_0000
    }
}

/// `v` as 10-digit two's complement in radix 2, 8 or 16: `None` exactly when
/// `v` is outside `[−r¹⁰/2, r¹⁰/2)`. The digits are those of `v` (`v + r¹⁰`
/// if negative) without leading zeros: at most 10, each below `r`, exactly 10
/// for a negative `v`, and [`decode`] maps them back to `v`.
pub fn encode(v: i64, r: u32) -> (res: Option<Vec<u8>>)
    requires
        tc_radix(r),
    ensures
        res is None <==> !(-(pw(r as nat, 10) as int / 2) <= v < pw(r as nat, 10) as int / 2),
        res matches Some(ds) ==> {
            &&& digits_below(ds@, r as nat)
            &&& 1 <= ds.len() <= 10
            &&& (v < 0 ==> ds.len() == 10)
            &&& decode_spec(ds@, r as nat) == Some(v as int)
        },
{
    let full = full10(r);
    let half = full / 2;
    if v < -half || v >= half {
        return None;
    }
    let u: u64 = if v < 0 {
        (v + full) as u64
    } else {
        v as u64
    };
    let ds = to_digits(u, r);
    proof {
        let rn = r as nat;
        let f = pw(rn, 10);
        let p9 = pw(rn, 9);
        let h = rn / 2;
        let n = ds.len();
        let w = value(ds@, rn);
        let d0 = ds@[0] as nat;
        assert(w == u);
        lemma_value_first(ds@, rn);
        lemma_value_bound(ds@, rn);
        lemma_pw_pos(rn, 9);
        assert(f == rn * p9);
        assert(rn == 2 * h && h >= 1);
        assert(f / 2 == h * p9) by (nonlinear_arith)
            requires
                f == rn * p9,
                rn == 2 * h,
        ;
        assert(h * p9 >= p9) by (nonlinear_arith)
            requires
                h >= 1,
        ;
        // At most 10 digits: a leading digit ≥ 1 makes u ≥ r^(n−1) ≥ r¹⁰.
        if n > 10 {
            lemma_pw_mono(rn, 10, (n - 1) as nat);
            let p_top = pw(rn, (n - 1) as nat);
            assert(d0 * p_top >= p_top) by (nonlinear_arith)
                requires
                    d0 >= 1,
            ;
        }
        if v >= 0 {
            if n == 10 && d0 >= h {
                assert(d0 * p9 >= h * p9) by (nonlinear_arith)
                    requires
                        d0 >= h,
                ;
            }
        } else {
            // u ≥ r¹⁰/2 ≥ r⁹: 10 digits, led by one ≥ r/2.
            if n <= 9 {
                lemma_pw_mono(rn, n as nat, 9);
            }
            if n == 10 && d0 + 1 <= h {
                assert((d0 + 1) * p9 <= h * p9) by (nonlinear_arith)
                    requires
                        d0 + 1 <= h,
                ;
            }
        }
    }
    Some(ds)
}

/// The integer 10-digit two's-complement digits `ds` in radix 2, 8 or 16
/// stand for (`decode_spec`): `None` for more than 10 digits or a digit not
/// below `r`.
pub fn decode(ds: &[u8], r: u32) -> (res: Option<i64>)
    requires
        tc_radix(r),
    ensures
        res is None <==> decode_spec(ds@, r as nat) is None,
        res matches Some(v) ==> decode_spec(ds@, r as nat) == Some(v as int),
        ds.len() > 10 ==> res is None,
        (exists|i: int| 0 <= i < ds.len() && ds[i] as nat >= r) ==> res is None,
{
    if ds.len() > 10 {
        return None;
    }
    let full = full10(r);
    let mut v: i64 = 0;
    let mut i = 0;
    while i < ds.len()
        invariant
            tc_radix(r),
            ds.len() <= 10,
            0 <= i <= ds.len(),
            full == pw(r as nat, 10),
            full <= 0x100_0000_0000,
            digits_below(ds@.subrange(0, i as int), r as nat),
            v == value(ds@.subrange(0, i as int), r as nat),
        decreases ds.len() - i,
    {
        let d = ds[i];
        if d as u32 >= r {
            assert(!digits_below(ds@, r as nat)) by {
                assert(ds@[i as int] as nat >= r);
            }
            return None;
        }
        proof {
            let p = ds@.subrange(0, i + 1);
            let p0 = ds@.subrange(0, i as int);
            assert(p.drop_last() =~= p0);
            assert(p.last() == d);
            assert(digits_below(p, r as nat)) by {
                assert forall|k: int| 0 <= k < p.len() implies (#[trigger] p[k] as nat) < r by {
                    if k < i {
                        assert(p[k] == p0[k]);
                    }
                }
            }
            lemma_value_bound(p, r as nat);
            lemma_pw_mono(r as nat, (i + 1) as nat, 10);
            assert(v * r + d == value(p, r as nat));
        }
        v = v * (r as i64) + d as i64;
        i += 1;
    }
    assert(ds@.subrange(0, ds.len() as int) =~= ds@);
    if ds.len() == 10 && ds[0] as u32 >= r / 2 {
        v -= full;
    }
    Some(v)
}

/// The round trip, as one statement: for every `v` in range, [`encode`]
/// succeeds and [`decode`] of its digits gives `v` back.
pub fn round_trip(v: i64, r: u32) -> (ok: bool)
    requires
        tc_radix(r),
        -(pw(r as nat, 10) as int / 2) <= v < pw(r as nat, 10) as int / 2,
    ensures
        ok,
{
    match encode(v, r) {
        Some(ds) => decode(&ds, r) == Some(v),
        None => false,
    }
}

} // verus!

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn digits_of_zero_and_more() {
        assert_eq!(to_digits(0, 2), vec![0]);
        assert_eq!(to_digits(10, 2), vec![1, 0, 1, 0]);
        assert_eq!(to_digits(255, 16), vec![15, 15]);
        assert_eq!(to_digits(35, 36), vec![35]);
    }

    #[test]
    fn twos_complement_round_trips() {
        for r in [2u32, 8, 16] {
            let half = i64::from(r).pow(10) / 2;
            for v in [-half, -1, 0, 1, half - 1] {
                let ds = encode(v, r).unwrap();
                assert_eq!(decode(&ds, r), Some(v), "{v} in radix {r}");
                assert!(round_trip(v, r));
            }
            assert_eq!(encode(half, r), None);
            assert_eq!(encode(-half - 1, r), None);
        }
        assert_eq!(encode(-1, 2), Some(vec![1; 10]));
        assert_eq!(decode(&[1; 11], 2), None);
        assert_eq!(decode(&[2], 2), None);
        assert_eq!(decode(&[], 16), Some(0));
    }

    fn text(ds: &[u8]) -> String {
        ds.iter()
            .map(|&d| char::from_digit(u32::from(d), 16).unwrap())
            .collect()
    }

    /// Every 10-digit binary value, and a stride through octal and hex,
    /// against `format!` on `v` (`v + r¹⁰` when negative).
    #[test]
    fn encode_matches_format() {
        for (r, step) in [(2u32, 1i64), (8, 9_973), (16, 7_919_993)] {
            let full = i64::from(r).pow(10);
            let mut v = -full / 2;
            while v < full / 2 {
                let u = if v < 0 { v + full } else { v };
                let want = match r {
                    2 => format!("{u:b}"),
                    8 => format!("{u:o}"),
                    _ => format!("{u:x}"),
                };
                let ds = encode(v, r).unwrap();
                assert_eq!(text(&ds), want, "{v} in radix {r}");
                assert_eq!(decode(&ds, r), Some(v));
                v += step;
            }
        }
        for u in (0..1_000_000u64).chain([u64::MAX, (1 << 53) - 1]) {
            assert_eq!(text(&to_digits(u, 16)), format!("{u:x}"));
        }
    }
}
