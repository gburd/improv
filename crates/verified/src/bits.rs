//! The 48-bit operations of `BITAND`, `BITOR`, `BITXOR`, `BITLSHIFT` and
//! `BITRSHIFT`. Operands are below 2⁴⁸ (the engine checks that) and shifts
//! are at most 53 places.

#[allow(unused_imports)]
use vstd::arithmetic::power2::*;
#[allow(unused_imports)]
use vstd::bits::*;
#[allow(unused_imports)]
use vstd::prelude::*;

verus! {

/// 2⁴⁸, the bit functions' exclusive operand bound.
pub const LIMIT: u64 = 0x1_0000_0000_0000;

/// `a & b`: below 2⁴⁸ and at most either operand.
pub fn and(a: u64, b: u64) -> (r: u64)
    requires
        a < LIMIT,
        b < LIMIT,
    ensures
        r == a & b,
        r < LIMIT,
        r <= a,
        r <= b,
{
    assert(a & b <= a && a & b <= b) by (bit_vector);
    a & b
}

/// `a | b`: below 2⁴⁸ and at least either operand.
pub fn or(a: u64, b: u64) -> (r: u64)
    requires
        a < LIMIT,
        b < LIMIT,
    ensures
        r == a | b,
        r < LIMIT,
        r >= a,
        r >= b,
{
    assert(a | b < 0x1_0000_0000_0000 && a | b >= a && a | b >= b) by (bit_vector)
        requires
            a < 0x1_0000_0000_0000,
            b < 0x1_0000_0000_0000,
    ;
    a | b
}

/// `a ^ b`: below 2⁴⁸.
pub fn xor(a: u64, b: u64) -> (r: u64)
    requires
        a < LIMIT,
        b < LIMIT,
    ensures
        r == a ^ b,
        r < LIMIT,
{
    assert(a ^ b < 0x1_0000_0000_0000) by (bit_vector)
        requires
            a < 0x1_0000_0000_0000,
            b < 0x1_0000_0000_0000,
    ;
    a ^ b
}

/// `x · 2ⁿ`, exactly: below 2¹⁰¹, so it never overflows the `u128`.
pub fn shl(x: u64, n: u32) -> (r: u128)
    requires
        x < LIMIT,
        n <= 53,
    ensures
        r == x * pow2(n as nat),
        r < 0x20_0000_0000_0000_0000_0000_0000,
{
    proof {
        lemma2_to64_rest();
        if n < 53 {
            lemma_pow2_strictly_increases(n as nat, 53);
        }
        let p = pow2(n as nat);
        assert(x * p < 0x20_0000_0000_0000_0000_0000_0000) by (nonlinear_arith)
            requires
                x < 0x1_0000_0000_0000,
                p <= 0x20_0000_0000_0000,
        ;
        lemma_u64_pow2_no_overflow(n as nat);
        lemma_u64_shl_is_mul(1, n as u64);
    }
    let p = 1u64 << (n as u64);
    (x as u128) * (p as u128)
}

/// `x / 2ⁿ` (rounded down): at most `x`.
pub fn shr(x: u64, n: u32) -> (r: u64)
    requires
        n <= 53,
    ensures
        r as nat == x as nat / pow2(n as nat),
        r <= x,
{
    proof {
        lemma_u64_shr_is_div(x, n as u64);
    }
    let s = n as u64;
    assert(x >> s <= x) by (bit_vector);
    x >> s
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_ops() {
        assert_eq!(and(0b1100, 0b1010), 0b1000);
        assert_eq!(or(0b1100, 0b1010), 0b1110);
        assert_eq!(xor(0b1100, 0b1010), 0b0110);
        assert_eq!(shl(LIMIT - 1, 53), u128::from(LIMIT - 1) << 53);
        assert_eq!(shr(LIMIT - 1, 47), 1);
        assert_eq!(shr(5, 53), 0);
    }
}
