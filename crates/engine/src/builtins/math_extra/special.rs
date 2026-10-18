//! LAMBERTW, REDUCEPI and the Erlang-B telephone-traffic functions.

use super::super::{domain, err, statistical::gamma_q};

// --- Lambert W ------------------------------------------------------------------

/// The Lambert W function on branch `k` (0 or −1) by Halley's iteration
/// from the usual starting points (the branch-point series near −1/e, the
/// asymptotic L₁ − L₂ + L₂/L₁ far from it).
pub(super) fn lambert_w(x: f64, k: f64) -> Result<f64, u8> {
    // 1/e rounds up, so x = −1/e as a double is already just past the branch
    // point; Gnumeric answers −1 there rather than #NUM!, and so does this.
    const INV_E: f64 = 0.367_879_441_171_442_33;
    domain((k == 0.0 || k == -1.0) && x >= -INV_E && x.is_finite())?;
    let lower = k == -1.0;
    if x == -INV_E {
        return Ok(-1.0);
    }
    domain(!lower || x < 0.0)?;
    if x == 0.0 {
        return Ok(0.0);
    }
    let p = (2.0 * (std::f64::consts::E * x + 1.0)).max(0.0).sqrt();
    let p = if lower { -p } else { p };
    let mut w = if x < -0.25 {
        -1.0 + p - p * p / 3.0 + 11.0 / 72.0 * p * p * p
    } else if lower {
        let (l1, l2) = ((-x).ln(), (-(-x).ln()).ln());
        l1 - l2 + l2 / l1
    } else if x < 3.0 {
        let l = x.ln_1p();
        l * (1.0 - (1.0 + l).ln() / (2.0 + l))
    } else {
        let (l1, l2) = (x.ln(), x.ln().ln());
        l1 - l2 + l2 / l1
    };
    let mut dw = f64::INFINITY;
    for _ in 0..40 {
        let ew = w.exp();
        let f = w * ew - x;
        dw = f / (ew * (w + 1.0) - (w + 2.0) * f / (2.0 * w + 2.0));
        let old = w;
        w -= dw;
        // Stay on the branch: W₀ ≥ −1 ≥ W₋₁.
        if (!lower && w <= -1.0) || (lower && w >= -1.0) {
            w = old + (-1.0 - old) * 15.0 / 16.0;
        }
        if dw.abs() <= 4.0 * f64::EPSILON * w.abs() {
            break;
        }
    }
    domain(dw.abs() <= 1e-12 * w.abs())?;
    Ok(w)
}

// --- REDUCEPI -----------------------------------------------------------------------

/// 1/(2π) in binary: bit i (from 1) of the fraction is bit 63 − (i−1) mod 64
/// of word (i−1)/64. 1408 bits: enough for x up to 2¹⁰²⁴ and e ≤ 7.
const INV_2PI: [u64; 22] = [
    0x28be_60db_9391_054a,
    0x7f09_d5f4_7d4d_3770,
    0x36d8_a566_4f10_e410,
    0x7f94_58ea_f7ae_f158,
    0x6dc9_1b8e_9093_74b8,
    0x0192_4bba_8274_6487,
    0x3f87_7ac7_2c4a_69cf,
    0xba20_8d7d_4bae_d121,
    0x3a67_1c09_ad17_df90,
    0x4e64_758e_60d4_ce7d,
    0x2721_17e2_ef7e_4a0e,
    0xc7fe_25ff_f781_6603,
    0xfbcb_c462_d682_9b47,
    0xdb4d_9fb3_c9f2_c26d,
    0xd3d1_8fd9_a797_fa8b,
    0x5d49_eeb1_faf9_7c5e,
    0xcf41_ce7d_e294_a4ba,
    0x9afe_d7ec_47e3_5742,
    0x1580_cc11_bf1e_daea,
    0xfc33_ef08_26bd_0d87,
    0x6a78_e458_57b9_86c2,
    0x1966_6157_c528_1a10,
];

/// The 64 bits of 1/(2π) at fraction positions p, p+1, …, p+63 (positions
/// below 1 are the zero integer part). `reduce_pi` reads at most position
/// 1227 (s ≤ 971 + 7 + 1, plus 185 + 63), inside the table's 1408.
fn bits64(p: i64) -> u64 {
    let bit = |i: i64| -> u64 {
        if i < 1 {
            return 0;
        }
        let i = (i - 1) as usize;
        (INV_2PI[i / 64] >> (63 - i % 64)) & 1
    };
    (0..64).fold(0, |acc, j| (acc << 1) | bit(p + j))
}

/// Gnumeric's REDUCEPI: x = xr + j·π/2ᵉ with |xr| ≤ π/2ᵉ⁺¹ and π exact
/// (Payne–Hanek: the bits of x·2ᵉ/π that matter, from 256 bits of 1/(2π)).
/// Returns xr and the e+1 low bits of j.
pub(super) fn reduce_pi(x: f64, e: i32) -> Result<(f64, i64), u8> {
    domain(x.is_finite())?;
    let mask = (1i64 << (e + 1)) - 1;
    let ax = x.abs();
    if ax < 1.5 * libm::ldexp(1.0, -e) {
        // Below π/2ᵉ⁺¹: j = 0 (this also covers 0 and the subnormals).
        return Ok((x, 0));
    }
    // |x| = m·2^k, m a 53-bit integer; N = x·2ᵉ/π = m · (2^(k+e+1)/(2π)).
    let bits = ax.to_bits();
    let m = (bits & ((1 << 52) - 1)) | (1 << 52);
    let k = ((bits >> 52) & 0x7ff) as i64 - 1075;
    let s = k + i64::from(e) + 1;
    // U·2⁻²⁴⁸ = (2^s/(2π)) mod 2⁸, to 248 fraction bits.
    let u = [
        bits64(s + 185),
        bits64(s + 121),
        bits64(s + 57),
        bits64(s - 7),
    ];
    let mut prod = [0u64; 4];
    let mut carry = 0u128;
    for (p, &w) in prod.iter_mut().zip(&u) {
        let t = u128::from(w) * u128::from(m) + carry;
        *p = t as u64;
        carry = t >> 64;
    }
    // Bits 248–255 are N mod 2⁸; below them the fraction f, as F·2⁻¹²⁰ +
    // L·2⁻²⁴⁸.
    let mut j = i64::from((prod[3] >> 56) as u8);
    let big_f = (u128::from(prod[3] & ((1 << 56) - 1)) << 64) | u128::from(prod[2]);
    let low = (u128::from(prod[1]) << 64) | u128::from(prod[0]);
    let (g, l, neg) = if big_f >= 1 << 119 {
        // f ≥ ½: round j up, xr = −(1 − f).
        j += 1;
        let borrow = u128::from(low != 0);
        ((1u128 << 120) - big_f - borrow, low.wrapping_neg(), true)
    } else {
        (big_f, low, false)
    };
    let hi = g as f64;
    let rem = g as i128 - hi as u128 as i128;
    let f_hi = libm::ldexp(hi, -120);
    let f_lo = libm::ldexp(rem as f64, -120) + libm::ldexp(l as f64, -248);
    // xr = f·π/2ᵉ in double-double.
    const PI_HI: f64 = std::f64::consts::PI;
    const PI_LO: f64 = 1.224_646_799_147_353_2e-16;
    let p = f_hi * PI_HI;
    let err = f_hi.mul_add(PI_HI, -p);
    let mut xr = libm::ldexp(p + (err + f_hi * PI_LO + f_lo * PI_HI), -e);
    if neg {
        xr = -xr;
    }
    if x < 0.0 {
        xr = -xr;
        j = -j;
    }
    Ok((xr, j & mask))
}

// --- Erlang B ------------------------------------------------------------------------

/// The largest circuit count: bounds every evaluation's cost.
const MAX_CIRCUITS: f64 = 1e6;

/// (B, 1 − B): the Erlang-B blocking probability of `a` erlangs offered to
/// `c` circuits, by Gnumeric's three methods (Erlang's recursion below 100
/// circuits, which truncates c; the series of c!/(k!·a^(c−k)) when
/// c < 0.9a; else the Poisson density over its distribution function).
fn erlang(a: f64, c: f64) -> Result<(f64, f64), u8> {
    if !(c >= 1.0 && a >= 0.0) {
        return Err(err::VALUE);
    }
    domain(c <= MAX_CIRCUITS && a.is_finite())?;
    if a == 0.0 {
        return Ok((0.0, 1.0));
    }
    if c < 100.0 {
        let mut g = 1.0;
        let mut i = 1.0;
        while i <= c {
            g = a * g / (i + a * g);
            i += 1.0;
        }
        return Ok((g, 1.0 - g));
    }
    // Gnumeric stops at n = 1, but the terms (ratio n/a < 1 and falling)
    // drop below ε·sum long before: from c ≥ 100 and a > c/0.9 (or a tail
    // that underflows), t(c−1)/t(1) ≤ (c−1)!/a^(c−2) < 1e-44. The loop
    // ends by n ≤ 0 regardless (a zero or negative term).
    let series = || {
        let (mut sum, mut term, mut n) = (0.0, 1.0, c);
        loop {
            term *= n / a;
            if term < f64::EPSILON * sum {
                break;
            }
            sum += term;
            n -= 1.0;
        }
        (1.0 / (1.0 + sum), sum / (1.0 + sum))
    };
    if c / a < 0.9 {
        return Ok(series());
    }
    // Finite: c + 1 ∈ [101, 10⁶ + 1] and a > 0 are inside gamma_q's domain.
    let q = gamma_q(c + 1.0, a);
    if q <= 1e-300 {
        // Both tails underflow (c just below a huge a): the series is short there.
        return Ok(series());
    }
    let log_b = ln_poisson(c, a) - q.ln();
    Ok((log_b.exp(), -log_b.exp_m1()))
}

/// ln of the Poisson density at c ≥ 100 (Loader's saddle-point form, as R's
/// `dpois_raw`): accurate where c ln a − a − ln c! would cancel.
fn ln_poisson(c: f64, a: f64) -> f64 {
    let stirlerr = {
        let c2 = c * c;
        (1.0 / 12.0 - (1.0 / 360.0 - 1.0 / 1260.0 / c2) / c2) / c
    };
    -stirlerr - bd0(c, a) - 0.5 * (std::f64::consts::TAU * c).ln()
}

/// c·ln(c/a) + a − c without cancellation when c ≈ a.
fn bd0(c: f64, a: f64) -> f64 {
    if (c - a).abs() < 0.1 * (c + a) {
        let v = (c - a) / (c + a);
        let mut s = (c - a) * v;
        let mut ej = 2.0 * c * v;
        let v2 = v * v;
        for j in 1..1000 {
            ej *= v2;
            let s1 = s + ej / f64::from(2 * j + 1);
            if s1 == s {
                break;
            }
            s = s1;
        }
        s
    } else {
        c * (c / a).ln() + a - c
    }
}

/// The x in [lo, hi] where the increasing `f` crosses 0, by bisection to
/// the last bit (at most 200 steps).
fn bisect(f: impl Fn(f64) -> f64, mut lo: f64, mut hi: f64) -> f64 {
    for _ in 0..200 {
        let mid = lo + (hi - lo) / 2.0;
        if mid <= lo || mid >= hi {
            break;
        }
        if f(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

pub(super) fn probblock(a: f64, c: f64) -> Result<f64, u8> {
    Ok(erlang(a, c)?.0)
}

/// The offered traffic that carries `t` erlangs on `c` circuits: the root
/// of a·(1 − B(a, c)) = t for a in [t, c] (#VALUE! if there is none).
pub(super) fn offtraf(t: f64, c: f64) -> Result<f64, u8> {
    let carried = |a: f64| erlang(a, c).map(|(_, nb)| a * nb - t);
    if !(c >= 1.0 && t >= 0.0) || carried(c)? < 0.0 {
        return Err(err::VALUE);
    }
    Ok(bisect(|a| carried(a).unwrap_or(f64::NAN), t, c))
}

/// The fewest circuits that carry `a` erlangs with blocking at most `gos`.
pub(super) fn dimcirc(a: f64, gos: f64) -> Result<f64, u8> {
    if !(gos > 0.0 && gos <= 1.0 && a >= 0.0) {
        return Err(err::VALUE);
    }
    let (mut lo, mut hi) = (1.0, 1.0);
    while erlang(a, hi)?.0 > gos {
        domain(hi < MAX_CIRCUITS)?;
        lo = hi;
        hi = (hi * 2.0).min(MAX_CIRCUITS);
    }
    while hi - lo > 1.5 {
        let mid = ((hi + lo) / 2.0 + 0.1).floor();
        if erlang(a, mid)?.0 > gos {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Ok(hi)
}

/// The traffic `c` circuits carry at blocking `gos`: the root of
/// B(a, c) = gos for a in [0, c/(1 − gos)].
pub(super) fn offcap(c: f64, gos: f64) -> Result<f64, u8> {
    if !(gos > 0.0 && gos < 1.0) {
        return Err(err::VALUE);
    }
    erlang(1.0, c)?;
    Ok(bisect(
        |a| erlang(a, c).map_or(f64::NAN, |(b, _)| b - gos),
        0.0,
        c / (1.0 - gos),
    ))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn lambert_w_satisfies_its_equation() {
        for x in [-0.367, -0.3, -0.1, 1e-300, 0.5, 2.5, 10.0, 1e10, 1e300] {
            let w = lambert_w(x, 0.0).unwrap();
            assert!(
                (w * w.exp() - x).abs() <= 1e-13 * x.abs().max(1e-300),
                "W0({x}) = {w}"
            );
        }
        for x in [-0.367, -0.3, -0.1, -1e-10, -1e-300] {
            let w = lambert_w(x, -1.0).unwrap();
            assert!(w < -1.0);
            assert!((w * w.exp() - x).abs() <= 1e-12 * x.abs(), "W-1({x}) = {w}");
        }
        assert_eq!(lambert_w(-0.367_879_441_171_442_33, -1.0), Ok(-1.0));
        assert_eq!(lambert_w(0.0, 0.0), Ok(0.0));
        assert_eq!(lambert_w(f64::INFINITY, 0.0), Err(err::NUM));
        // Gnumeric #NUM!: a branch other than 0 or -1, x below -1/e, W₋₁
        // of a positive x.
        assert_eq!(lambert_w(1.0, 2.0), Err(err::NUM));
        assert_eq!(lambert_w(-0.5, 0.0), Err(err::NUM));
        assert_eq!(lambert_w(1.0, -1.0), Err(err::NUM));
    }

    #[test]
    fn reduce_pi_agrees_with_the_simple_reduction() {
        // Below 2²⁰, x − j·π/2ᵉ in double-double is exact enough to compare.
        for &x in &[3.0, 10.0, -10.0, 100.5, 12345.678, -7.0e5, 1.0e6] {
            for e in -1..=7 {
                let step = std::f64::consts::PI / f64::from(1 << (e + 1)) * 2.0;
                let j = (x / step).round();
                let want = x - j * step;
                let (xr, q) = reduce_pi(x, e).unwrap();
                assert!((xr - want).abs() < 1e-9, "x={x} e={e}: {xr} vs {want}");
                let mask = (1i64 << (e + 1)) - 1;
                assert_eq!(q, (j as i64) & mask, "x={x} e={e}");
            }
        }
        assert_eq!(reduce_pi(f64::NAN, 0), Err(err::NUM));
        assert_eq!(reduce_pi(1e-310, 3), Ok((1e-310, 0)));
        // The largest double reduces without running off the table.
        assert!(reduce_pi(f64::MAX, 7).unwrap().0.abs() <= std::f64::consts::PI / 256.0);
    }

    /// Release-mode timing of the slowest inputs (`--ignored`).
    #[test]
    #[ignore]
    fn erlang_is_fast() {
        for (name, f) in [
            (
                "probblock big",
                Box::new(|| probblock(1.1e6, 1e6)) as Box<dyn Fn() -> Result<f64, u8>>,
            ),
            ("probblock tail", Box::new(|| probblock(1e6, 9e5))),
            ("dimcirc big", Box::new(|| dimcirc(9e5, 1e-3))),
            ("dimcirc huge", Box::new(|| dimcirc(1e9, 0.01))),
            ("offcap big", Box::new(|| offcap(1e6, 0.01))),
            ("offtraf big", Box::new(|| offtraf(9e5, 1e6))),
        ] {
            let t = std::time::Instant::now();
            let v = f();
            println!("{name}: {v:?} in {:?}", t.elapsed());
        }
    }

    #[test]
    fn erlang_branches() {
        // The series and the Poisson-ratio forms agree on integer c.
        let (a, c) = (1000.0, 950.0);
        let (b, nb) = erlang(a, c).unwrap();
        let (mut sum, mut term, mut n) = (0.0, 1.0, c);
        while n >= 1.0 {
            term *= n / a;
            sum += term;
            n -= 1.0;
        }
        assert!((b - 1.0 / (1.0 + sum)).abs() < 1e-13 * b, "{b}");
        assert!((b + nb - 1.0).abs() < 1e-15);
        // Underflowing tails fall back to the series.
        let (b, _) = erlang(1e6, 9e5).unwrap();
        assert!((b - 0.1).abs() < 1e-3, "{b}");
        assert_eq!(erlang(5.0, 0.5), Err(err::VALUE));
        assert_eq!(erlang(5.0, 2e6), Err(err::NUM));
        assert_eq!(erlang(0.0, 5.0), Ok((0.0, 1.0)));
        assert_eq!(dimcirc(1e9, 0.01), Err(err::NUM));
        assert_eq!(offcap(0.5, 0.1), Err(err::VALUE));
        // Against Gnumeric 1.12: negative traffic, too few circuits, a
        // carried load above the circuits, blocking outside (0, 1].
        assert_eq!(probblock(-1.0, 5.0), Err(err::VALUE));
        assert_eq!(probblock(5.0, 0.5), Err(err::VALUE));
        let near = |got: Result<f64, u8>, want: f64| {
            let g = got.unwrap();
            assert!((g - want).abs() <= 1e-12 * want, "{g} vs {want}");
        };
        near(probblock(1000.0, 100.0), 0.900_110_837_840_662_4);
        near(probblock(150.0, 100.5), 0.342_201_062_936_081_66);
        near(offtraf(8.0, 12.0), 8.598_753_503_809_851);
        for (t, c) in [(-1.0, 4.0), (2.0, 0.5), (5.0, 4.0)] {
            assert_eq!(offtraf(t, c), Err(err::VALUE), "OFFTRAF({t}, {c})");
        }
        assert_eq!(dimcirc(10.0, 0.01), Ok(18.0));
        assert_eq!(dimcirc(50.0, 0.05), Ok(56.0));
        assert_eq!(dimcirc(10.0, 1.0), Ok(1.0));
        assert_eq!(dimcirc(10.0, 0.0), Err(err::VALUE));
        assert_eq!(dimcirc(10.0, 1.5), Err(err::VALUE));
        // Negative traffic: #VALUE!, as PROBBLOCK and OFFTRAF have it
        // (Gnumeric 1.12 answers 1 for DIMCIRC alone).
        assert_eq!(dimcirc(-1.0, 0.1), Err(err::VALUE));
        assert_eq!(offcap(5.0, 0.0), Err(err::VALUE));
        assert_eq!(offcap(5.0, 1.0), Err(err::VALUE));
        assert!(bd0(100.0, 50.0) > 0.0);
    }
}
