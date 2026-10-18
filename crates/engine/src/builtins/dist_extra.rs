//! Evaluation of the P7 distributions (ids 1550–1749): Gnumeric's extra
//! distributions and its R-style kernels `R.D*`, `R.P*`, `R.Q*`.
//!
//! Gnumeric 1.12.57 is the reference: its argument order, defaults and
//! domain rules (a NaN from its kernel is `#NUM!` here). The kernels are
//! written from the textbook formulas, reusing `statistical`'s Γ, B, the
//! incomplete functions and Φ.

use super::{b, domain, err, n, ok_num, R};
use crate::CellValue;
use std::f64::consts::{FRAC_1_SQRT_2, PI};

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    let x = |i: usize| n(a, i);
    match id {
        1550 => {
            let (k, p) = (x(0)?, x(1)?);
            domain((0.0..=1.0).contains(&p) && (k == 0.0 || k == 1.0))?;
            ok_num(if k == 0.0 { 1.0 - p } else { p })
        }
        1551 => {
            let (v, s) = (x(0)?, x(1)?);
            domain(s > 0.0)?;
            // Gnumeric's "cumulative" CAUCHY is the right tail P(X > x).
            ok_num(if b(a, 2)? {
                cauchy_cdf(-v / s)
            } else {
                1.0 / (PI * s * (1.0 + (v / s) * (v / s)))
            })
        }
        1552 => {
            let (v, s, e) = (x(0)?, x(1)?, x(2)?);
            domain(e > 0.0)?;
            ok_num((-(v / s).abs().powf(e) - libm::lgamma(1.0 + 1.0 / e)).exp() / (2.0 * s))
        }
        1553 => {
            let (k, p) = (fake_floor(x(0)?), x(1)?);
            domain(p > 0.0 && p <= 1.0 && k >= 0.0)?;
            ok_num(if b(a, 2)? {
                geom_cdf(k, p, true)
            } else {
                geom_pmf(k, p)
            })
        }
        1555 => {
            let (v, s) = (x(0)?, x(1)?);
            domain(s > 0.0)?;
            ok_num((-v.abs() / s).exp() / (2.0 * s))
        }
        1556 => {
            let (v, s) = (x(0)?, x(1)?);
            domain(s > 0.0)?;
            let u = (-v.abs() / s).exp();
            ok_num(u / (s * (1.0 + u) * (1.0 + u)))
        }
        1557 => {
            let (v, e, s) = (x(0)?, x(1)?, x(2)?);
            domain(e > 0.0 && s > 0.0)?;
            ok_num(if v >= s {
                (e / s) / (v / s).powf(e + 1.0)
            } else {
                0.0
            })
        }
        1558 => {
            let (v, s) = (x(0)?, x(1)?);
            domain(s > 0.0)?;
            ok_num(rayleigh_pdf(v, s))
        }
        1559 => {
            let (v, lo, s) = (x(0)?, x(1)?, x(2)?);
            domain(s > 0.0)?;
            if v < lo {
                return ok_num(0.0);
            }
            let (u, w) = (v / s, lo / s);
            ok_num(u / s * ((w + u) * (w - u) / 2.0).exp())
        }
        1560 => ok_num(norm_range(x(0)?, x(1)?)),
        _ => Err(err::NAME),
    }
}

/// The floor of x, forgiving an x one ulp under an integer (Gnumeric's
/// `gnm_fake_floor`: 2.9999999999999996 counts as 3).
fn fake_floor(x: f64) -> f64 {
    if x == x.floor() {
        x
    } else {
        x.next_up().floor()
    }
}

/// The standard Cauchy CDF P(X ≤ z), keeping the far tails' precision.
fn cauchy_cdf(z: f64) -> f64 {
    if z.abs() > 1.0 {
        let y = (1.0 / z).atan() / PI;
        if z > 0.0 {
            1.0 - y
        } else {
            -y
        }
    } else {
        0.5 + z.atan() / PI
    }
}

/// P(K = k) for k failures before the first success, p ∈ (0, 1].
fn geom_pmf(k: f64, p: f64) -> f64 {
    if k == 0.0 {
        p
    } else {
        p * (k * (-p).ln_1p()).exp()
    }
}

/// P(K ≤ k) (or P(K > k)) for k ≥ 0 failures, p ∈ (0, 1].
fn geom_cdf(k: f64, p: f64, lower: bool) -> f64 {
    let ln_q = (k + 1.0) * (-p).ln_1p();
    if lower {
        -ln_q.exp_m1()
    } else {
        ln_q.exp()
    }
}

/// The Rayleigh density of scale s > 0 at x.
fn rayleigh_pdf(x: f64, s: f64) -> f64 {
    if x <= 0.0 {
        return 0.0;
    }
    let u = x / s;
    u / s * (-0.5 * u * u).exp()
}

/// Φ(x2) − Φ(x1) without cancellation: through erf when the interval spans
/// 0, by quadrature of the density when the two tails nearly agree.
fn norm_range(x1: f64, x2: f64) -> f64 {
    if x1.is_nan() || x2.is_nan() {
        return f64::NAN;
    }
    if x1 > x2 {
        return -norm_range(x2, x1);
    }
    if x1 == x2 {
        return 0.0;
    }
    if x1 <= 0.0 && x2 >= 0.0 {
        return (libm::erf(x2 * FRAC_1_SQRT_2) - libm::erf(x1 * FRAC_1_SQRT_2)) / 2.0;
    }
    if x2 < 0.0 {
        return norm_range(-x2, -x1);
    }
    // 0 < x1 < x2: a difference of right tails.
    let (q1, q2) = (
        super::statistical::norm_cdf(-x1),
        super::statistical::norm_cdf(-x2),
    );
    if (q1 - q2) * 32.0 > q1 + q2 || !x2.is_finite() {
        return q1 - q2;
    }
    // The tails nearly cancel, so the interval is narrow (the density falls
    // by at most e^(-1/32) across it): Gauss–Legendre integrates it exactly.
    let (c, h) = ((x1 + x2) / 2.0, (x2 - x1) / 2.0);
    let phi = |z: f64| (-z * z / 2.0).exp() / (2.0 * PI).sqrt();
    h * GL16
        .iter()
        .map(|&(t, w)| w * (phi(c - h * t) + phi(c + h * t)))
        .sum::<f64>()
}

/// The 16-point Gauss–Legendre nodes t > 0 and weights on [−1, 1] (each node
/// is used as ±t).
const GL16: [(f64, f64); 8] = [
    (0.989_400_934_991_649_9, 0.027_152_459_411_754_095),
    (0.944_575_023_073_232_6, 0.062_253_523_938_647_89),
    (0.865_631_202_387_831_8, 0.095_158_511_682_492_78),
    (0.755_404_408_355_003, 0.124_628_971_255_533_87),
    (0.617_876_244_402_643_7, 0.149_595_988_816_576_73),
    (0.458_016_777_657_227_4, 0.169_156_519_395_002_54),
    (0.281_603_550_779_258_9, 0.182_603_415_044_923_6),
    (0.095_012_509_837_637_44, 0.189_450_610_455_068_5),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: u32, args: &[CellValue]) -> R {
        eval(id, args)
    }

    fn nums(v: &[f64]) -> Vec<CellValue> {
        v.iter().map(|x| CellValue::num(*x)).collect()
    }

    fn val(id: u32, v: &[f64]) -> f64 {
        call(id, &nums(v)).unwrap().as_num().unwrap()
    }

    #[test]
    fn helpers_cover_their_edges() {
        assert_eq!(fake_floor(3.0), 3.0);
        assert_eq!(fake_floor(2.999_999_999_999_999_6), 3.0);
        assert_eq!(fake_floor(2.5), 2.0);
        assert!((cauchy_cdf(2.0) - (0.5 + 2f64.atan() / PI)).abs() < 1e-15);
        assert!((cauchy_cdf(-2.0) - (0.5 - 2f64.atan() / PI)).abs() < 1e-15);
        assert_eq!(geom_pmf(0.0, 0.3), 0.3);
        assert!((geom_cdf(2.0, 0.3, false) - 0.343).abs() < 1e-15);
        assert_eq!(rayleigh_pdf(-1.0, 1.0), 0.0);
        assert!(norm_range(f64::NAN, 1.0).is_nan());
        assert_eq!(norm_range(1.0, 1.0), 0.0);
        assert!((norm_range(-2.0, -1.0) - norm_range(1.0, 2.0)).abs() < 1e-16);
        assert!((norm_range(1.0, f64::INFINITY) - 0.158_655_253_931_457_05).abs() < 1e-16);
    }

    #[test]
    fn rayleigh_tail_below_its_limit_is_zero() {
        assert_eq!(val(1559, &[0.1, 0.3, 1.0]), 0.0);
        assert_eq!(val(1557, &[0.5, 1.0, 2.0]), 0.0);
    }

    #[test]
    fn unknown_ids_are_name_errors() {
        assert_eq!(call(1749, &nums(&[1.0])), Err(err::NAME));
    }
}
