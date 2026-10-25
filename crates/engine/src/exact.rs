//! Exact float summation (`SUM`, `AVERAGE`): a fixed-point
//! superaccumulator that holds the sum of any finite floats without
//! rounding, rounded once at the end (to nearest, ties to even). The result
//! is the exact sum's nearest float whatever the order or grouping of the
//! terms, so a flat sum, a blocked sum ([`crate::reduce::two_level`]) and a
//! sum of partial sums ([`Exact`]) agree bit for bit.
//!
//! The accumulator is an integer in units of 2⁻¹⁰⁷⁴ (the least subnormal)
//! held as signed 32-bit digits in `i64`s, so a term adds to three digits
//! with no carry; carries run once every [`CARRY_EVERY`] terms and at the
//! end. Infinities and NaNs are flags, giving what IEEE addition gives.

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// The accumulator's digits: digit `i` weighs 2^(32 i) units. A finite
/// float is below 2²⁰⁹⁸ units (66 digits); the last digit takes the carries.
const DIGITS: usize = 67;
/// Terms between carry passes. A term adds under 2³² to a digit, and a
/// carry pass leaves each digit under 2³¹, so a digit stays under 2⁵³.
const CARRY_EVERY: u32 = 1 << 20;
/// A term other than -0.0 was added: a zero sum is then +0.0 (IEEE
/// addition gives -0.0 only for -0.0 + -0.0).
const NOT_NEG_ZERO: u8 = 1;
const POS_INF: u8 = 2;
const NEG_INF: u8 = 4;
const NAN: u8 = 8;

/// A running exact sum.
#[derive(Clone)]
pub(crate) struct ExactSum {
    d: [i64; DIGITS],
    /// The digits that may be nonzero: `lo..hi`.
    lo: usize,
    hi: usize,
    /// Terms since the last carry pass.
    terms: u32,
    flags: u8,
}

impl Default for ExactSum {
    fn default() -> Self {
        ExactSum {
            d: [0; DIGITS],
            lo: DIGITS,
            hi: 0,
            terms: 0,
            flags: 0,
        }
    }
}

impl ExactSum {
    /// Add `x`.
    pub(crate) fn add(&mut self, x: f64) {
        let bits = x.to_bits();
        if bits != (-0.0f64).to_bits() {
            self.flags |= NOT_NEG_ZERO;
        }
        let (e, frac) = ((bits >> 52) & 0x7ff, bits & ((1 << 52) - 1));
        let neg = bits >> 63 == 1;
        if e == 0x7ff {
            self.flags |= match (frac, neg) {
                (0, false) => POS_INF,
                (0, true) => NEG_INF,
                _ => NAN,
            };
            return;
        }
        // |x| = m · 2^s units.
        let (m, s) = if e == 0 {
            (frac, 0)
        } else {
            (frac | 1 << 52, e - 1)
        };
        if m == 0 {
            return;
        }
        let (i, v) = ((s / 32) as usize, u128::from(m) << (s % 32));
        for (j, w) in [v as u32, (v >> 32) as u32, (v >> 64) as u32]
            .into_iter()
            .enumerate()
        {
            let w = i64::from(w);
            self.d[i + j] += if neg { -w } else { w };
        }
        self.touch(i, i + 3);
    }

    /// Add a partial sum.
    pub(crate) fn add_exact(&mut self, x: &Exact) {
        self.flags |= x.flags;
        if x.d.is_empty() {
            return;
        }
        let lo = usize::from(x.lo);
        for (j, w) in x.d.iter().enumerate() {
            self.d[lo + j] += i64::from(*w);
        }
        self.touch(lo, lo + x.d.len());
    }

    fn touch(&mut self, lo: usize, hi: usize) {
        (self.lo, self.hi) = (self.lo.min(lo), self.hi.max(hi));
        self.terms += 1;
        if self.terms == CARRY_EVERY {
            self.carry();
        }
    }

    /// Bring every digit into [-2³¹, 2³¹), balanced (so the top nonzero
    /// digit's sign is the sum's). The last digit keeps its carries.
    fn carry(&mut self) {
        self.terms = 0;
        let (mut i, mut c) = (self.lo, 0);
        while i < self.hi || c != 0 {
            let x = self.d[i] + c;
            c = if i + 1 == DIGITS {
                0
            } else {
                (x + (1 << 31)) >> 32
            };
            self.d[i] = x - (c << 32);
            i += 1;
        }
        self.hi = self.hi.max(i);
    }

    /// The sum so far, exactly.
    pub(crate) fn exact(&mut self) -> Exact {
        self.carry();
        let (mut lo, mut hi) = (self.lo, self.hi);
        while lo < hi && self.d[lo] == 0 {
            lo += 1;
        }
        while hi > lo && self.d[hi - 1] == 0 {
            hi -= 1;
        }
        if lo >= hi {
            lo = 0;
            hi = 0;
        }
        Exact {
            lo: lo as u8,
            // Only the carry digit can leave i32, past 2^45 times the
            // largest float: the sum is infinite either way.
            d: self.d[lo..hi]
                .iter()
                .map(|w| (*w).clamp(i32::MIN.into(), i32::MAX.into()) as i32)
                .collect(),
            flags: self.flags,
        }
    }

    /// The sum so far, rounded once.
    pub(crate) fn value(&mut self) -> f64 {
        self.exact().value()
    }
}

/// An exact sum over a sliding window: terms leave as well as join, and
/// its value is the window's terms' [`ExactSum`] value, bit for bit (the
/// digits are exact integers, and the flags are counted, not or-ed).
#[derive(Default)]
pub(crate) struct WindowSum {
    /// The finite terms' digits; a term leaves by adding its negation.
    sum: ExactSum,
    /// The terms other than -0.0, the +∞s, the -∞s, the NaNs.
    n: [usize; 4],
}

impl WindowSum {
    /// `x` joins the window.
    pub(crate) fn add(&mut self, x: f64) {
        self.step(x, true);
    }

    /// `x`, an earlier term, leaves the window.
    pub(crate) fn remove(&mut self, x: f64) {
        self.step(x, false);
    }

    fn step(&mut self, x: f64, join: bool) {
        let kinds = [
            x.to_bits() != (-0.0f64).to_bits(),
            x == f64::INFINITY,
            x == f64::NEG_INFINITY,
            x.is_nan(),
        ];
        for (n, k) in self.n.iter_mut().zip(kinds) {
            if k && join {
                *n += 1;
            } else if k {
                *n -= 1;
            }
        }
        if x.is_finite() {
            self.sum.add(if join { x } else { -x });
        }
    }

    /// The window's sum, rounded once.
    pub(crate) fn value(&mut self) -> f64 {
        let mut e = self.sum.exact();
        e.flags = [NOT_NEG_ZERO, POS_INF, NEG_INF, NAN]
            .into_iter()
            .zip(self.n)
            .filter(|(_, n)| *n > 0)
            .fold(0, |f, (bit, _)| f | bit);
        e.value()
    }
}

/// An exact sum, compact and canonical (equal sums are equal values): its
/// balanced digits from the lowest nonzero one up. A sum of floats of
/// similar size needs two to four.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub(crate) struct Exact {
    lo: u8,
    d: SmallVec<[i32; 4]>,
    flags: u8,
}

impl Exact {
    /// The sum rounded to the nearest float, ties to even, as IEEE addition
    /// treats infinities and NaNs; infinite beyond the largest float.
    pub(crate) fn value(&self) -> f64 {
        let f = self.flags;
        if f & NAN != 0 || f & (POS_INF | NEG_INF) == POS_INF | NEG_INF {
            return f64::NAN;
        }
        if f & POS_INF != 0 {
            return f64::INFINITY;
        }
        if f & NEG_INF != 0 {
            return f64::NEG_INFINITY;
        }
        let Some(&top) = self.d.last() else {
            return if f & NOT_NEG_ZERO != 0 { 0.0 } else { -0.0 };
        };
        // The magnitude's digits (balanced, so a run of them has the sign
        // of its top nonzero digit).
        let sign: i128 = if top < 0 { -1 } else { 1 };
        let dig = |i: usize| sign * i128::from(self.d[i]);
        // The top three digits as one integer w (at least 2⁶² when there
        // are three) and the rest r, |r| < 2^(32 base) units: the magnitude
        // is w·2^(32 base) + r, so its floor in those units is w, or w - 1
        // when r < 0, and r ≠ 0 is a sticky bit.
        let n = self.d.len();
        let base = n.saturating_sub(3);
        let w = (base..n).rev().fold(0i128, |w, i| (w << 32) + dig(i));
        let r = (0..base).rev().map(dig).find(|x| *x != 0).unwrap_or(0);
        let w = (w - i128::from(r < 0)) as u128;
        // Its top 64 bits, any bit below folded into the lowest (so the one
        // conversion to f64 rounds correctly).
        let shift = (128 - w.leading_zeros()).saturating_sub(64);
        let below = w & ((1 << shift) - 1) != 0 || r != 0;
        let x = ((w >> shift) as u64 | u64::from(below)) as f64;
        // In -1074..=1006: at most 67 digits.
        let p = shift as i32 + 32 * (i32::from(self.lo) + base as i32) - 1074;
        let mag = if p < -1022 {
            // Only an exact sum is this small (below 2^64 units), so x is
            // exact and so is the scaling.
            x * pow2(-1022) * pow2(p + 1022)
        } else {
            x * pow2(p)
        };
        if sign < 0 {
            -mag
        } else {
            mag
        }
    }
}

/// 2^`p`, for `p` in -1022..=1023.
fn pow2(p: i32) -> f64 {
    f64::from_bits(((p + 1023) as u64) << 52)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sum(xs: &[f64]) -> f64 {
        let mut a = ExactSum::default();
        xs.iter().for_each(|x| a.add(*x));
        a.value()
    }

    /// `xs` cut where `cuts` say, each part summed and the parts summed.
    fn blocked(xs: &[f64], cuts: &[usize]) -> f64 {
        let mut at: Vec<usize> = cuts.iter().map(|c| c % (xs.len() + 1)).collect();
        at.extend([0, xs.len()]);
        at.sort_unstable();
        let mut total = ExactSum::default();
        for w in at.windows(2) {
            let mut part = ExactSum::default();
            xs[w[0]..w[1]].iter().for_each(|x| part.add(*x));
            total.add_exact(&part.exact());
        }
        total.value()
    }

    #[test]
    fn rounds_once_to_nearest_even() {
        let p53 = 2f64.powi(53);
        // 0.1 + 0.2 + 0.3: a left fold gives 0.6000000000000001.
        assert_eq!(sum(&[0.1, 0.2, 0.3]), 0.6);
        assert_eq!(sum(&[1e16, 1.0, -1e16]), 1.0);
        assert_eq!(sum(&[1.0, 1e100, 1.0, -1e100]), 2.0);
        // A tie rounds to even; anything past it, up; anything short, down.
        assert_eq!(sum(&[p53, 1.0]), p53);
        assert_eq!(sum(&[p53, 3.0]), p53 + 4.0);
        assert_eq!(sum(&[p53, 1.0, 2f64.powi(-60)]), p53 + 2.0);
        assert_eq!(sum(&[p53, 1.0, -(2f64.powi(-60))]), p53);
        assert_eq!(sum(&[-p53, -1.0, 2f64.powi(-60)]), -p53);
        // Many digits apart: the far bits are only a sticky bit.
        assert_eq!(sum(&[1e300, 1e-300, -1e300]), 1e-300);
        assert_eq!(sum(&[1e300, 1.0, 1e-300]), 1e300);
        // Subnormals are exact.
        let tiny = f64::from_bits(1);
        assert_eq!(sum(&[tiny, tiny, tiny]), f64::from_bits(3));
        assert_eq!(
            sum(&[f64::MIN_POSITIVE, -tiny]),
            f64::from_bits((1 << 52) - 1)
        );
        assert_eq!(sum(&[2.5, -tiny]), 2.5);
    }

    #[test]
    fn zeros_overflow_and_non_finite_terms() {
        assert_eq!(sum(&[]).to_bits(), (-0.0f64).to_bits());
        assert_eq!(sum(&[-0.0, -0.0]).to_bits(), (-0.0f64).to_bits());
        assert_eq!(sum(&[-0.0, 0.0]).to_bits(), 0);
        assert_eq!(sum(&[1.5, -1.5]).to_bits(), 0);
        // An intermediate overflow is none; a final one is.
        assert_eq!(sum(&[f64::MAX, f64::MAX, -f64::MAX]), f64::MAX);
        assert_eq!(sum(&[f64::MAX, f64::MAX]), f64::INFINITY);
        assert_eq!(sum(&[-f64::MAX, -f64::MAX]), f64::NEG_INFINITY);
        // As IEEE addition: an infinity wins, two opposite ones are NaN.
        assert_eq!(sum(&[1.0, f64::INFINITY]), f64::INFINITY);
        assert_eq!(sum(&[f64::NEG_INFINITY, 1.0]), f64::NEG_INFINITY);
        assert!(sum(&[f64::INFINITY, f64::NEG_INFINITY]).is_nan());
        assert!(sum(&[f64::NAN, 1.0]).is_nan());
        assert!(blocked(&[1.0, f64::NAN], &[1]).is_nan());
    }

    #[test]
    fn carries_keep_up_with_many_terms() {
        // Past a carry pass, with digits pushed both ways and far apart.
        let n = CARRY_EVERY as usize + 10;
        let big = 2f64.powi(1000);
        let xs: Vec<f64> = (0..n)
            .map(|i| match i % 4 {
                0 | 2 => big,
                1 => -0.75 * big - 2f64.powi(-1000),
                _ => -0.75 * big + 2f64.powi(-1000),
            })
            .collect();
        assert_eq!(sum(&xs), (n / 2) as f64 * 0.25 * big);
        let ones = vec![1.0 + f64::EPSILON; n];
        assert_eq!(sum(&ones), n as f64 * (1.0 + f64::EPSILON));
        // A partial sum's digits are canonical.
        let mut a = ExactSum::default();
        a.add(-1.0);
        a.add(2f64.powi(-1000));
        let mut b = ExactSum::default();
        b.add(2f64.powi(-1000));
        b.add(-1.0);
        let e = a.exact();
        assert_eq!(e, b.exact());
        assert_eq!(e.value(), -1.0);
        assert_eq!(ExactSum::default().exact(), Exact::default());
    }

    /// Floats m · 2^e with e in -60..=0: their sum in units of 2⁻⁶⁰ is an
    /// exact i128, and an i128 → f64 cast rounds it once.
    fn reference(xs: &[f64]) -> f64 {
        let units: i128 = xs.iter().map(|x| (x * 2f64.powi(60)) as i128).sum();
        units as f64 * 2f64.powi(-60)
    }

    fn scaled() -> impl Strategy<Value = f64> {
        (any::<i64>(), -60i32..=0).prop_map(|(m, e)| (m >> 11) as f64 * 2f64.powi(e))
    }

    /// Any finite float, scaled so 200 of them cannot overflow.
    fn wide() -> impl Strategy<Value = f64> {
        any::<u64>().prop_map(|b| {
            let x = f64::from_bits(b);
            if x.is_finite() {
                x / 4096.0
            } else {
                1.0
            }
        })
    }

    proptest! {
        #[test]
        fn the_sum_is_the_exact_sums_nearest_float(xs in prop::collection::vec(scaled(), 1..200)) {
            prop_assert_eq!(sum(&xs).to_bits(), reference(&xs).to_bits());
        }

        /// A window slid over the terms sums what a fresh sum of the
        /// window's terms does, bit for bit.
        #[test]
        fn a_sliding_sum_is_a_fresh_sum(
            xs in prop::collection::vec(prop_oneof![
                scaled(),
                wide(),
                prop::sample::select(vec![
                    0.0, -0.0, f64::INFINITY, f64::NEG_INFINITY, f64::NAN, f64::MAX,
                ]),
            ], 0..60),
            width in 1usize..8,
        ) {
            let mut w = WindowSum::default();
            for (i, x) in xs.iter().enumerate() {
                w.add(*x);
                if i >= width {
                    w.remove(xs[i - width]);
                }
                let lo = (i + 1).saturating_sub(width);
                prop_assert_eq!(w.value().to_bits(), sum(&xs[lo..=i]).to_bits());
            }
        }

        #[test]
        fn order_and_grouping_do_not_matter(
            xs in prop::collection::vec(prop_oneof![scaled(), wide()], 0..200),
            cuts in prop::collection::vec(any::<usize>(), 0..8),
        ) {
            let flat = sum(&xs).to_bits();
            let mut rev = xs.clone();
            rev.reverse();
            prop_assert_eq!(sum(&rev).to_bits(), flat);
            prop_assert_eq!(blocked(&xs, &cuts).to_bits(), flat);
        }
    }
}
