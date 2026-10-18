//! Evaluation of the statistical functions (see `improv_core_model::functions`):
//! the special functions (Γ, B, ψ, erf, …) and the probability distributions.
//!
//! The kernels are `libm`'s Γ, ln Γ, erf and erfc (musl's, about an ulp) and
//! `statrs`'s incomplete gamma, incomplete beta and digamma. Every quantile inverts its CDF (the upper tail when
//! p > ½, so the tail keeps its precision) by bisection to the last bit, so
//! a quantile is as precise as the CDF it inverts. The discrete
//! distributions sum their mass functions term by term, as LibreOffice does.
//!
//! Precision ceiling: `statrs`'s incomplete beta stops after 140
//! continued-fraction terms and both incomplete functions go through
//! ln Γ, so shape parameters (degrees of freedom, α, β) above about 1e4 lose
//! digits.

use super::{b, domain, err, n, n_or, ok_num, R};
use crate::CellValue;
use statrs::function::{beta as sb, erf as serf, factorial, gamma as sg};
use std::f64::consts::SQRT_2;

/// 1/√(2π).
const FRAC_1_SQRT_2PI: f64 = 0.398_942_280_401_432_7;

/// Evaluate statistical function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    let x = |i: usize| n(a, i);
    // Integer-valued parameters are truncated, as Excel does.
    let int = |i: usize| n(a, i).map(f64::trunc);
    let flag = |i: usize| b(a, i);
    let opt_flag = |i: usize| if a.len() > i { b(a, i) } else { Ok(true) };
    match id {
        // --- special functions ---
        160 => {
            let v = x(0)?;
            domain(!is_nonpos_int(v))?;
            ok_num(gamma(v))
        }
        161 => {
            let v = x(0)?;
            domain(v > 0.0)?;
            ok_num(ln_gamma(v))
        }
        162 => ok_num(beta_fn(x(0)?, x(1)?)),
        163 => ok_num(ln_beta(x(0)?, x(1)?)),
        164 => ok_num(sg::digamma(x(0)?)),
        165 => {
            let (s, v) = (x(0)?, x(1)?);
            let (lower, reg, real) = (opt_flag(2)?, opt_flag(3)?, opt_flag(4)?);
            // Negative a or x make the function complex-valued (Gnumeric
            // returns its real or imaginary part); only the real domain is here.
            domain(s >= 0.0 && v >= 0.0)?;
            ok_num(if real { igamma(s, v, lower, reg) } else { 0.0 })
        }
        166 => ok_num(pochhammer(x(0)?, x(1)?)),
        167 => {
            let lo = libm::erf(x(0)?);
            ok_num(if a.len() > 1 {
                libm::erf(x(1)?) - lo
            } else {
                lo
            })
        }
        168 => ok_num(libm::erfc(x(0)?)),
        169 => ok_num(libm::erf(x(0)?)),
        // --- distributions ---
        500 | 501 => {
            // BETA.DIST(x, α, β, cumulative, [A], [B]); BETADIST has no flag.
            let at = if id == 500 { 4 } else { 3 };
            let cum = if id == 500 { flag(3)? } else { true };
            let (v, al, be) = (x(0)?, x(1)?, x(2)?);
            let (lo, hi) = (n_or(a, at, 0.0)?, n_or(a, at + 1, 1.0)?);
            domain(al > 0.0 && be > 0.0 && lo < hi && lo <= v && v <= hi)?;
            let w = hi - lo;
            let (z, zc) = ((v - lo) / w, (hi - v) / w);
            ok_num(if cum {
                beta_reg(al, be, z)
            } else {
                beta_pdf(z, zc, al, be) / w
            })
        }
        502 => {
            let (p, al, be) = (x(0)?, x(1)?, x(2)?);
            let (lo, hi) = (n_or(a, 3, 0.0)?, n_or(a, 4, 1.0)?);
            domain(p > 0.0 && p <= 1.0 && al > 0.0 && be > 0.0 && lo < hi)?;
            ok_num(lo + beta_inv(p, al, be) * (hi - lo))
        }
        503 => {
            let (k, trials, p, cum) = (int(0)?, int(1)?, x(2)?, flag(3)?);
            domain(0.0 <= k && k <= trials && (0.0..=1.0).contains(&p))?;
            ok_num(binom_range(trials, p, if cum { 0.0 } else { k }, k))
        }
        504 => {
            let (trials, p, alpha) = (int(0)?, x(1)?, x(2)?);
            domain(trials >= 0.0 && (0.0..=1.0).contains(&p) && (0.0..=1.0).contains(&alpha))?;
            ok_num(binom_inv(trials, p, alpha))
        }
        505 => {
            let (trials, p, s1) = (int(0)?, x(1)?, int(2)?);
            let s2 = if a.len() > 3 { int(3)? } else { s1 };
            domain((0.0..=1.0).contains(&p) && 0.0 <= s1 && s1 <= s2 && s2 <= trials)?;
            ok_num(binom_range(trials, p, s1, s2))
        }
        506 | 507 => {
            let (v, df) = (x(0)?, int(1)?);
            domain(v >= 0.0 && (1.0..=1e10).contains(&df))?;
            ok_num(match (id, id == 506 && flag(2)?) {
                (507, _) => gamma_q(df / 2.0, v / 2.0),
                (_, true) => gamma_p(df / 2.0, v / 2.0),
                _ => gamma_pdf(v, df / 2.0, 2.0)?,
            })
        }
        508 | 509 => {
            // CHISQ.INV takes the left tail, CHISQ.INV.RT the right one.
            let (p, df) = (x(0)?, int(1)?);
            domain((0.0..=1.0).contains(&p) && (1.0..=1e10).contains(&df))?;
            let (lt, rt) = if id == 508 {
                (p, 1.0 - p)
            } else {
                (1.0 - p, p)
            };
            ok_num(2.0 * gamma_inv(df / 2.0, lt, rt))
        }
        510 => {
            let (v, rate, cum) = (x(0)?, x(1)?, flag(2)?);
            domain(v >= 0.0 && rate > 0.0)?;
            ok_num(if cum {
                -(-rate * v).exp_m1()
            } else {
                rate * (-rate * v).exp()
            })
        }
        511 | 512 => {
            let (v, d1, d2) = (x(0)?, int(1)?, int(2)?);
            domain(v >= 0.0 && f_df(d1) && f_df(d2))?;
            ok_num(match (id, id == 511 && flag(3)?) {
                (512, _) => f_sf(v, d1, d2),
                (_, true) => f_cdf(v, d1, d2),
                _ => f_pdf(v, d1, d2),
            })
        }
        513 | 514 => {
            let (p, d1, d2) = (x(0)?, int(1)?, int(2)?);
            domain((0.0..=1.0).contains(&p) && f_df(d1) && f_df(d2))?;
            let (lt, rt) = if id == 513 {
                (p, 1.0 - p)
            } else {
                (1.0 - p, p)
            };
            ok_num(f_inv(d1, d2, lt, rt))
        }
        515 => {
            let (v, al, be, cum) = (x(0)?, x(1)?, x(2)?, flag(3)?);
            domain(v >= 0.0 && al > 0.0 && be > 0.0)?;
            ok_num(if cum {
                gamma_p(al, v / be)
            } else {
                gamma_pdf(v, al, be)?
            })
        }
        516 => {
            let (p, al, be) = (x(0)?, x(1)?, x(2)?);
            domain((0.0..=1.0).contains(&p) && al > 0.0 && be > 0.0)?;
            ok_num(be * gamma_inv(al, p, 1.0 - p))
        }
        517 => ok_num(0.5 * libm::erf(x(0)? / SQRT_2)),
        518 | 519 => {
            let (k, draws, succ, pop) = (int(0)?, int(1)?, int(2)?, int(3)?);
            let cum = id == 518 && flag(4)?;
            // Excel's constraints: a draw count the population allows.
            let least = (draws - pop + succ).max(0.0);
            domain(
                pop > 0.0
                    && 0.0 < draws
                    && draws <= pop
                    && 0.0 < succ
                    && succ <= pop
                    && least <= k
                    && k <= draws.min(succ),
            )?;
            ok_num(if cum {
                hypgeom_cdf(k, draws, succ, pop)
            } else {
                hypgeom_pmf(k, draws, succ, pop)
            })
        }
        520 | 521 => {
            let (v, mu, sd) = (x(0)?, x(1)?, x(2)?);
            let cum = id == 521 || flag(3)?;
            domain(v > 0.0 && sd > 0.0)?;
            let z = (v.ln() - mu) / sd;
            ok_num(if cum {
                norm_cdf(z)
            } else {
                norm_pdf(z) / (sd * v)
            })
        }
        522 => {
            let (p, mu, sd) = (x(0)?, x(1)?, x(2)?);
            domain(p > 0.0 && p < 1.0 && sd > 0.0)?;
            ok_num((mu + sd * norm_inv(p)).exp())
        }
        523 | 524 => {
            let (f, s, p) = (int(0)?, int(1)?, x(2)?);
            let cum = id == 523 && flag(3)?;
            domain(f >= 0.0 && s >= 1.0 && (0.0..=1.0).contains(&p))?;
            ok_num(if cum {
                beta_reg(s, f + 1.0, p)
            } else {
                negbinom_pmf(f, s, p)
            })
        }
        525 => {
            let (v, mu, sd, cum) = (x(0)?, x(1)?, x(2)?, flag(3)?);
            domain(sd > 0.0)?;
            let z = (v - mu) / sd;
            ok_num(if cum { norm_cdf(z) } else { norm_pdf(z) / sd })
        }
        526 => {
            let (p, mu, sd) = (x(0)?, x(1)?, x(2)?);
            domain(p > 0.0 && p < 1.0 && sd > 0.0)?;
            ok_num(mu + sd * norm_inv(p))
        }
        527 => {
            let z = x(0)?;
            ok_num(if flag(1)? { norm_cdf(z) } else { norm_pdf(z) })
        }
        528 => ok_num(norm_cdf(x(0)?)),
        529 => {
            let p = x(0)?;
            domain(p > 0.0 && p < 1.0)?;
            ok_num(norm_inv(p))
        }
        530 => ok_num(norm_pdf(x(0)?)),
        531 => {
            let (k, mean, cum) = (int(0)?, x(1)?, flag(2)?);
            domain(k >= 0.0 && mean >= 0.0)?;
            ok_num(poisson(k, mean, cum))
        }
        532 | 533 => {
            let (t, df) = (x(0)?, int(1)?);
            domain(df >= 1.0)?;
            ok_num(match (id, id == 532 && flag(2)?) {
                (533, _) => t_sf(t, df),
                (_, true) => t_sf(-t, df),
                _ => t_pdf(t, df),
            })
        }
        534 | 535 => {
            // T.DIST.2T(x, df); TDIST(x, df, tails) is its half when tails = 1.
            let (t, df) = (x(0)?, int(1)?);
            let tails = if id == 535 { int(2)? } else { 2.0 };
            domain(t >= 0.0 && df >= 1.0 && (tails == 1.0 || tails == 2.0))?;
            ok_num(tails * t_sf(t, df))
        }
        536 => {
            let (p, df) = (x(0)?, int(1)?);
            domain(p > 0.0 && p <= 1.0 && df >= 1.0)?;
            ok_num(if p < 0.5 {
                -t_upper_inv(p, df)
            } else {
                t_upper_inv(1.0 - p, df)
            })
        }
        537 => {
            let (p, df) = (x(0)?, int(1)?);
            domain(p > 0.0 && p <= 1.0 && df >= 1.0)?;
            ok_num(t_upper_inv(p / 2.0, df))
        }
        538 => {
            let (v, shape, scale, cum) = (x(0)?, x(1)?, x(2)?, flag(3)?);
            domain(v >= 0.0 && shape > 0.0 && scale > 0.0)?;
            let r = v / scale;
            let e = r.powf(shape);
            ok_num(if cum {
                -(-e).exp_m1()
            } else {
                shape / scale * r.powf(shape - 1.0) * (-e).exp()
            })
        }
        539 => {
            let v = x(0)?;
            domain(v > -1.0 && v < 1.0)?;
            ok_num(v.atanh())
        }
        540 => ok_num(x(0)?.tanh()),
        541 | 542 => {
            let (alpha, sd, size) = (x(0)?, x(1)?, int(2)?);
            domain(alpha > 0.0 && alpha < 1.0 && sd > 0.0 && size >= 1.0)?;
            let q = if id == 541 {
                -norm_inv(alpha / 2.0)
            } else if size == 1.0 {
                return Err(err::DIV0);
            } else {
                t_upper_inv(alpha / 2.0, size - 1.0)
            };
            ok_num(q * sd / size.sqrt())
        }
        543 => {
            let (v, mu, sd) = (x(0)?, x(1)?, x(2)?);
            domain(sd > 0.0)?;
            ok_num((v - mu) / sd)
        }
        // An unknown id is #NAME?.
        _ => Err(err::NAME),
    }
}

fn is_nonpos_int(v: f64) -> bool {
    v <= 0.0 && v == v.floor()
}

fn is_int(v: f64) -> bool {
    v == v.floor()
}

// --- special functions ----------------------------------------------------

/// Γ(x), exact at the positive integers.
fn gamma(x: f64) -> f64 {
    if is_int(x) && (1.0..=171.0).contains(&x) {
        factorial::factorial(x as u64 - 1)
    } else {
        libm::tgamma(x)
    }
}

/// ln |Γ(x)|.
pub(crate) fn ln_gamma(x: f64) -> f64 {
    libm::lgamma(x)
}

/// B(a, b) over the reals, as Gnumeric extends it: NaN where a or b is a
/// non-positive integer, 0 where only a + b is one (1/Γ(a + b) = 0).
fn beta_fn(a: f64, b: f64) -> f64 {
    if is_nonpos_int(a) || is_nonpos_int(b) {
        return f64::NAN;
    }
    if is_nonpos_int(a + b) {
        return 0.0;
    }
    if a > 0.0 && b > 0.0 && a + b > 171.0 {
        return ln_beta(a, b).exp();
    }
    gamma(a) * gamma(b) / gamma(a + b)
}

/// ln |B(a, b)|.
pub(crate) fn ln_beta(a: f64, b: f64) -> f64 {
    if a > 0.0 && b > 0.0 && a + b > 171.0 {
        ln_gamma(a) + ln_gamma(b) - ln_gamma(a + b)
    } else {
        beta_fn(a, b).abs().ln()
    }
}

/// The incomplete gamma function of a ≥ 0, x ≥ 0 (Gnumeric's IGAMMA, real
/// part): lower or upper, regularized or not.
fn igamma(a: f64, x: f64, lower: bool, regularized: bool) -> f64 {
    if a == 0.0 {
        // P(0, x) = 1; the unregularized form has a pole.
        return match (regularized, lower) {
            (false, _) => f64::NAN,
            (true, true) => 1.0,
            (true, false) => 0.0,
        };
    }
    let v = if lower { gamma_p(a, x) } else { gamma_q(a, x) };
    if regularized {
        v
    } else {
        v * gamma(a)
    }
}

/// Γ(x + n) / Γ(x), with Gnumeric's limits where Γ(x) has a pole.
fn pochhammer(x: f64, n: f64) -> f64 {
    if n == 0.0 {
        return 1.0;
    }
    if is_nonpos_int(x) {
        if !is_int(n) || (x == 0.0 && n > 0.0) {
            return 0.0;
        }
        if n > -x {
            return f64::NAN;
        }
    }
    if is_int(n) && n.abs() < 100.0 {
        if n < 0.0 {
            return 1.0 / pochhammer(x + n, -n);
        }
        return (0..n as u32).map(|i| x + f64::from(i)).product();
    }
    if is_nonpos_int(x + n) {
        return f64::NAN;
    }
    if x > 0.0 && x + n > 0.0 && x.max(x + n) > 171.0 {
        return (ln_gamma(x + n) - ln_gamma(x)).exp();
    }
    gamma(x + n) / gamma(x)
}

// --- continuous kernels ---------------------------------------------------

/// The regularized lower incomplete gamma P(a, x), a > 0.
pub(crate) fn gamma_p(a: f64, x: f64) -> f64 {
    incomplete_gamma(a, x, true)
}

/// The regularized upper incomplete gamma Q(a, x), a > 0.
pub(crate) fn gamma_q(a: f64, x: f64) -> f64 {
    incomplete_gamma(a, x, false)
}

fn incomplete_gamma(a: f64, x: f64, lower: bool) -> f64 {
    // statrs's series runs ~√a terms when x ≈ a, so a huge shape would hang.
    if !(a > 0.0 && a <= 1e12) || x.is_nan() {
        return f64::NAN;
    }
    if x <= 0.0 || x == f64::INFINITY {
        return if (x <= 0.0) == lower { 0.0 } else { 1.0 };
    }
    let r = if lower {
        sg::checked_gamma_lr(a, x)
    } else {
        sg::checked_gamma_ur(a, x)
    };
    r.unwrap_or(f64::NAN)
}

/// The regularized incomplete beta I_x(a, b). statrs for moderate shapes;
/// past 1000 its prefactor (ln Γ(a+b) − ln Γ(a) − ln Γ(b) + a ln x + b ln y,
/// terms of size a ln a that cancel to O(1)) loses about log10 a digits, and
/// its continued fraction stops at 140 terms while it needs about √(a + b)
/// (BINOM.DIST(4e7, 1e8, 0.4) came out −0.42). There the prefactor is
/// Loader's saddle-point form and the fraction runs to convergence.
pub(crate) fn beta_reg(a: f64, b: f64, x: f64) -> f64 {
    let x = x.clamp(0.0, 1.0);
    if a > 1000.0 && b > 1000.0 && x > 0.0 && x < 1.0 {
        return beta_reg_large(a, b, x);
    }
    sb::checked_beta_reg(a, b, x).unwrap_or(f64::NAN)
}

/// I_x(a, b) for a, b > 1000 and 0 < x < 1: the continued fraction (Numerical
/// Recipes' `betacf`, modified Lentz) on the side where it converges fast,
/// times x^a y^b / (a B(a, b)) computed without cancellation.
fn beta_reg_large(a: f64, b: f64, x: f64) -> f64 {
    let y = 1.0 - x;
    // Far from the mean the front underflows to 0 and the tail is 0: the
    // continued fraction (up to a million terms) is not needed.
    let tail = |a: f64, b: f64, x: f64, y: f64| {
        let front = ln_beta_front(a, b, x, y).exp();
        if front == 0.0 {
            0.0
        } else {
            front * beta_cf(a, b, x)
        }
    };
    // The mean test halved, so a + b past f64::MAX does not overflow.
    if x < (a / 2.0 + 0.5) / (a / 2.0 + b / 2.0 + 1.0) {
        tail(a, b, x, y).min(1.0)
    } else {
        (1.0 - tail(b, a, y, x)).max(0.0)
    }
}

/// ln(x^a y^b / (a B(a, b))) for a, b > 1: x·y·(a+b−1) times the binomial
/// mass of a − 1 in a + b − 2 trials, over a; the mass by Loader's form
/// (Stirling corrections and bd0, as R's `dbinom_raw`).
fn ln_beta_front(a: f64, b: f64, x: f64, y: f64) -> f64 {
    // n − k is b − 1 as given: (a + b − 2) − (a − 1) rounds to 0 when b is
    // below a's ulp, and bd0(0, ·) is a NaN. The √ term is taken in logs:
    // 2π·k·(n − k) overflows past 1e154.
    let (k, nk, n) = (a - 1.0, b - 1.0, a + b - 2.0);
    if n.is_infinite() {
        // a + b overflows: the same with n halved (bd0 is homogeneous,
        // bd0(k, n x) = 2 bd0(k/2, (n/2) x), and stirlerr(n) is 0).
        let (h, ln2) = (a / 2.0 + b / 2.0 - 1.0, std::f64::consts::LN_2);
        let ln_mass =
            -stirlerr(k) - stirlerr(nk) - 2.0 * (bd0(k / 2.0, h * x) + bd0(nk / 2.0, h * y))
                + 0.5 * (h.ln() + ln2 - std::f64::consts::TAU.ln() - k.ln() - nk.ln());
        return x.ln() + y.ln() + ln2 + (h + 0.5).ln() + ln_mass - a.ln();
    }
    let ln_mass = stirlerr(n) - stirlerr(k) - stirlerr(nk) - bd0(k, n * x) - bd0(nk, n * y)
        + 0.5 * (n.ln() - std::f64::consts::TAU.ln() - k.ln() - nk.ln());
    x.ln() + y.ln() + (a + b - 1.0).ln() + ln_mass - a.ln()
}

/// The incomplete beta's continued fraction (modified Lentz), to ε, for at
/// most a million terms (about √(a + b) are needed near the mean). Lentz's
/// usual guard against a zero denominator is left out: on this side of
/// the mean (x < (a+1)/(a+b+2)) the first is over 2/(a+b+2) and the rest
/// stay far from 0 (none below 5e-5 over 20000 random shapes 1e3–1e10).
fn beta_cf(a: f64, b: f64, x: f64) -> f64 {
    let (qab, qap, qam) = (a + b, a + 1.0, a - 1.0);
    let mut c = 1.0;
    let mut d = 1.0 / (1.0 - qab * x / qap);
    let mut h = d;
    for m in 1..1_000_000 {
        let (m, m2) = (f64::from(m), f64::from(2 * m));
        let aa = m * (b - m) * x / ((qam + m2) * (a + m2));
        d = 1.0 / (1.0 + aa * d);
        c = 1.0 + aa / c;
        h *= d * c;
        let aa = -(a + m) * (qab + m) * x / ((a + m2) * (qap + m2));
        d = 1.0 / (1.0 + aa * d);
        c = 1.0 + aa / c;
        let step = d * c;
        h *= step;
        if (step - 1.0).abs() < f64::EPSILON {
            break;
        }
    }
    h
}

/// ln(n!) − ln(√(2π n) (n/e)ⁿ) for n > 1000 (Stirling's series).
fn stirlerr(n: f64) -> f64 {
    let n2 = n * n;
    (1.0 / 12.0 - (1.0 / 360.0 - 1.0 / 1260.0 / n2) / n2) / n
}

/// c·ln(c/a) + a − c without cancellation when c ≈ a (Loader).
pub(crate) fn bd0(c: f64, a: f64) -> f64 {
    // Halved before adding: c + a overflows past f64::MAX / 2 (and 2c·v was
    // ∞ · 0 there).
    let (hc, ha) = (c / 2.0, a / 2.0);
    if (hc - ha).abs() < 0.1 * (hc + ha) {
        let v = (hc - ha) / (hc + ha);
        let mut s = (c - a) * v;
        let mut ej = c * (2.0 * v);
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

/// The beta density at z, with zc = 1 − z given separately for precision.
fn beta_pdf(z: f64, zc: f64, a: f64, b: f64) -> f64 {
    if (z == 0.0 && a == 1.0) || (zc == 0.0 && b == 1.0) {
        return 1.0 / beta_fn(a, b);
    }
    ((a - 1.0) * z.ln() + (b - 1.0) * zc.ln() - ln_beta(a, b)).exp()
}

/// The gamma density of shape a and scale s at x ≥ 0. At 0 it is infinite
/// for a < 1 (LibreOffice's #DIV/0!).
fn gamma_pdf(x: f64, a: f64, s: f64) -> Result<f64, u8> {
    if x == 0.0 {
        return if a < 1.0 {
            Err(err::DIV0)
        } else {
            Ok(f64::from(u8::from(a == 1.0)) / s)
        };
    }
    let r = x / s;
    Ok(((a - 1.0) * r.ln() - r - s.ln() - ln_gamma(a)).exp())
}

/// The x ≥ `lo` where the increasing `g` reaches `target`: grows the bracket
/// by doubling, then bisects in the floats' order until it cannot shrink
/// (at most 65 steps, even for a quantile among the subnormals).
pub(crate) fn solve(g: impl Fn(f64) -> f64, target: f64, mut lo: f64, mut hi: f64) -> f64 {
    if g(lo) >= target {
        return lo;
    }
    while g(hi) < target {
        lo = hi;
        hi *= 2.0;
        if hi.is_infinite() {
            return f64::NAN;
        }
    }
    loop {
        let mid = float_mid(lo, hi);
        if mid <= lo || mid >= hi {
            break;
        }
        if g(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// The middle of [lo, hi] in the floats' order (0 first when they straddle
/// it): bisecting with it reaches adjacent floats in at most 65 steps, where
/// halving the width takes up to 2100 (1e300 down to the subnormals).
pub(crate) fn float_mid(lo: f64, hi: f64) -> f64 {
    if lo < 0.0 && hi > 0.0 {
        return 0.0;
    }
    let m = f64::from_bits(((lo.abs().to_bits() as u128 + hi.abs().to_bits() as u128) / 2) as u64);
    if hi <= 0.0 {
        -m
    } else {
        m
    }
}

/// The smallest integer k in (a, b] with `ok(k)`, given !ok(a) and ok(b)
/// (or b the top of the support), for a monotone `ok`: bisects k + 1 in
/// the floats' order, so at most about 130 steps for any a and b (halving
/// [0, 1e300] took a thousand, and past 2⁵³ stepping to `mid + 1` never
/// ended).
pub(crate) fn int_search(mut a: f64, mut b: f64, ok: impl Fn(f64) -> bool) -> f64 {
    loop {
        let mut mid = (float_mid(a + 1.0, b + 1.0) - 1.0).floor();
        if mid <= a || mid >= b {
            mid = (a + (b - a) / 2.0).floor();
        }
        if mid <= a || mid >= b {
            return b;
        }
        if ok(mid) {
            b = mid;
        } else {
            a = mid;
        }
    }
}

/// The gamma(a, 1) quantile for left tail p (= 1 − q): solves the tail
/// that is under ½ so neither saturates.
pub(crate) fn gamma_inv(a: f64, p: f64, q: f64) -> f64 {
    if q <= 0.0 {
        f64::INFINITY
    } else if p <= 0.5 {
        solve(|x| gamma_p(a, x), p, 0.0, 1.0)
    } else {
        solve(|x| -gamma_q(a, x), -q, 0.0, 1.0)
    }
}

/// The beta(a, b) quantile of p ∈ (0, 1].
fn beta_inv(p: f64, a: f64, b: f64) -> f64 {
    if p == 1.0 {
        1.0
    } else {
        beta_q(a, b, p, 1.0 - p)
    }
}

/// The x with I_x(a, b) = p (= 1 − q), solved on the tail under ½ so it
/// keeps its digits. The upper tail is I_{1−x}(b, a) as a function of x:
/// solving for y = 1 − x and returning 1 − y lost every digit of a
/// quantile under ε (BETA.INV(0.6, 1e15, 1e300) is 9.98e-286, not 0).
pub(crate) fn beta_q(a: f64, b: f64, p: f64, q: f64) -> f64 {
    if p <= 0.5 {
        solve(|x| beta_reg(a, b, x), p, 0.0, 1.0)
    } else {
        // I_{1−x}(b, a) falls as x rises; 1 − x is exact from ½ up, and
        // below ½ the lower tail's own digits decide.
        let upper = |x: f64| {
            if x < 0.5 {
                1.0 - beta_reg(a, b, x)
            } else {
                beta_reg(b, a, 1.0 - x)
            }
        };
        solve(|x| -upper(x), -q, 0.0, 1.0)
    }
}

pub(crate) fn norm_cdf(z: f64) -> f64 {
    0.5 * libm::erfc(-z / SQRT_2)
}

pub(crate) fn norm_pdf(z: f64) -> f64 {
    FRAC_1_SQRT_2PI * (-z * z / 2.0).exp()
}

/// Φ⁻¹(p), p ∈ (0, 1): statrs's estimate, then Newton steps on Φ. The
/// upper half mirrors the lower (1 − p is exact for p ≥ ½), so Φ is only
/// ever evaluated in the left tail, where it keeps its relative precision.
pub(crate) fn norm_inv(p: f64) -> f64 {
    if p > 0.5 {
        return -norm_inv(1.0 - p);
    }
    let mut z = -SQRT_2 * serf::erfc_inv(2.0 * p);
    // The density stays positive (a subnormal at worst) for every p > 0.
    for _ in 0..2 {
        z -= (norm_cdf(z) - p) / norm_pdf(z);
    }
    z
}

/// Whether an F distribution's degrees of freedom are in Excel's range.
fn f_df(d: f64) -> bool {
    (1.0..1e10).contains(&d)
}

pub(crate) fn f_cdf(x: f64, d1: f64, d2: f64) -> f64 {
    beta_reg(d1 / 2.0, d2 / 2.0, d1 * x / (d1 * x + d2))
}

pub(crate) fn f_sf(x: f64, d1: f64, d2: f64) -> f64 {
    beta_reg(d2 / 2.0, d1 / 2.0, d2 / (d2 + d1 * x))
}

pub(crate) fn f_pdf(x: f64, d1: f64, d2: f64) -> f64 {
    if x == 0.0 {
        // The density at 0 is infinite, 1 or 0 as d1 is 1, 2 or more.
        return if d1 == 1.0 {
            f64::INFINITY
        } else {
            f64::from(u8::from(d1 == 2.0))
        };
    }
    let (h1, h2) = (d1 / 2.0, d2 / 2.0);
    (h1 * (d1 / d2).ln() + (h1 - 1.0) * x.ln()
        - (h1 + h2) * (d1 * x / d2).ln_1p()
        - ln_beta(h1, h2))
    .exp()
}

/// The F quantile for left tail p (= 1 − q).
pub(crate) fn f_inv(d1: f64, d2: f64, p: f64, q: f64) -> f64 {
    if q <= 0.0 {
        f64::INFINITY
    } else if p <= 0.5 {
        solve(|x| f_cdf(x, d1, d2), p, 0.0, 1.0)
    } else {
        solve(|x| -f_sf(x, d1, d2), -q, 0.0, 1.0)
    }
}

/// P(0 < T < t) for t ≥ 0: the central mass, precise near the median.
pub(crate) fn t_central(t: f64, df: f64) -> f64 {
    let z = t * t;
    0.5 * beta_reg(0.5, df / 2.0, z / (df + z))
}

/// Student's t right tail P(T > t).
pub(crate) fn t_sf(t: f64, df: f64) -> f64 {
    let z = t * t;
    if z < df {
        let c = t_central(t.abs(), df);
        return if t >= 0.0 { 0.5 - c } else { 0.5 + c };
    }
    let half = 0.5 * beta_reg(df / 2.0, 0.5, df / (df + z));
    if t >= 0.0 {
        half
    } else {
        1.0 - half
    }
}

pub(crate) fn t_pdf(t: f64, df: f64) -> f64 {
    (-(df + 1.0) / 2.0 * (t * t / df).ln_1p() - 0.5 * df.ln() - ln_beta(0.5, df / 2.0)).exp()
}

/// The t ≥ 0 with right tail q ∈ [0, ½] (infinite at 0).
pub(crate) fn t_upper_inv(q: f64, df: f64) -> f64 {
    if q <= 0.0 {
        return f64::INFINITY;
    }
    if q < 0.25 {
        solve(|t| -t_sf(t, df), -q, 0.0, 1.0)
    } else {
        // ½ − q is exact here; solving on the central mass keeps the digits.
        solve(|t| t_central(t, df), 0.5 - q, 0.0, 1.0)
    }
}

// --- discrete kernels -----------------------------------------------------

/// P(lo ≤ K ≤ hi) for K ~ Binomial(n, p), integers 0 ≤ lo ≤ hi ≤ n.
pub(crate) fn binom_range(n: f64, p: f64, lo: f64, hi: f64) -> f64 {
    if p == 0.0 {
        return f64::from(u8::from(lo == 0.0));
    }
    if p == 1.0 {
        return f64::from(u8::from(hi == n));
    }
    let q = 1.0 - p;
    let mut t = q.powf(n);
    if lo == hi && (t <= f64::MIN_POSITIVE || hi > 1e6) {
        // The mass alone, without a difference of tails.
        return beta_pdf(p, q, lo + 1.0, n - lo + 1.0) / (n + 1.0);
    }
    if t > f64::MIN_POSITIVE && hi <= 1e6 {
        // Sum the terms, each from the last: t_i = t_{i-1} (n-i+1)/i p/q.
        let mut i = 1.0;
        while i <= lo {
            t *= (n - i + 1.0) / i * p / q;
            i += 1.0;
        }
        let mut sum = t;
        while i <= hi {
            t *= (n - i + 1.0) / i * p / q;
            sum += t;
            i += 1.0;
        }
        return sum.min(1.0);
    }
    // P(K ≥ k) = I_p(k, n − k + 1).
    let at_least = |k: f64| {
        if k <= 0.0 {
            1.0
        } else {
            beta_reg(k, n - k + 1.0, p)
        }
    };
    let above = if hi >= n { 0.0 } else { at_least(hi + 1.0) };
    at_least(lo) - above
}

/// The smallest k with P(K ≤ k) ≥ alpha.
fn binom_inv(n: f64, p: f64, alpha: f64) -> f64 {
    if alpha == 1.0 {
        return if p == 0.0 { 0.0 } else { n };
    }
    let ok = |k: f64| binom_range(n, p, 0.0, k) >= alpha;
    if ok(0.0) {
        return 0.0;
    }
    int_search(0.0, n, ok)
}

/// C(n, k) as a product while it is small, else through ln Γ.
pub(crate) fn ln_choose(n: f64, k: f64) -> f64 {
    let k = k.min(n - k);
    if k <= 1000.0 {
        let mut c = 1.0;
        let mut i = 1.0;
        while i <= k {
            c = c * (n - k + i) / i;
            i += 1.0;
        }
        if c.is_finite() {
            return c.ln();
        }
    }
    ln_gamma(n + 1.0) - ln_gamma(k + 1.0) - ln_gamma(n - k + 1.0)
}

/// P(X = k) drawing n from N holding K successes.
pub(crate) fn hypgeom_pmf(k: f64, n: f64, big_k: f64, big_n: f64) -> f64 {
    (ln_choose(big_k, k) + ln_choose(big_n - big_k, n - k) - ln_choose(big_n, n)).exp()
}

/// P(X ≤ k): sums the tail on k's side of the mode, from k outward (the
/// terms fall away from the mode, so the sum stops once they stop counting).
pub(crate) fn hypgeom_cdf(k: f64, n: f64, big_k: f64, big_n: f64) -> f64 {
    let mode = ((n + 1.0) * (big_k + 1.0) / (big_n + 2.0)).floor();
    let (lo, hi) = ((n - big_n + big_k).max(0.0), n.min(big_k));
    // pmf(i + 1) / pmf(i).
    let up = |i: f64| (big_k - i) * (n - i) / ((i + 1.0) * (big_n - big_k - n + i + 1.0));
    let (mut i, below) = if k <= mode {
        (k, true)
    } else {
        (k + 1.0, false)
    };
    if !below && i > hi {
        return 1.0;
    }
    let mut t = hypgeom_pmf(i, n, big_k, big_n);
    let mut sum = 0.0;
    loop {
        sum += t;
        if t <= sum * 1e-17 || (below && i <= lo) || (!below && i >= hi) {
            break;
        }
        if below {
            i -= 1.0;
            t /= up(i);
        } else {
            t *= up(i);
            i += 1.0;
        }
    }
    if below {
        sum.min(1.0)
    } else {
        (1.0 - sum).max(0.0)
    }
}

/// P(F = f) for f failures before the s-th success.
pub(crate) fn negbinom_pmf(f: f64, s: f64, p: f64) -> f64 {
    let q = 1.0 - p;
    if f > 1e5 {
        return (ln_gamma(f + s) - ln_gamma(f + 1.0) - ln_gamma(s) + s * p.ln() + f * q.ln()).exp();
    }
    let mut v = p.powf(s);
    let mut i = 0.0;
    while i < f {
        v *= (i + s) / (i + 1.0) * q;
        i += 1.0;
    }
    v
}

/// The Poisson mass P(X = k) or distribution P(X ≤ k) at mean λ ≥ 0.
pub(crate) fn poisson(k: f64, lambda: f64, cumulative: bool) -> f64 {
    if lambda == 0.0 {
        return f64::from(u8::from(cumulative || k == 0.0));
    }
    if lambda > 712.0 || k > 1000.0 {
        // exp(−λ) underflows, or too many terms: the closed forms.
        return if cumulative {
            gamma_q(k + 1.0, lambda)
        } else {
            (k * lambda.ln() - lambda - ln_gamma(k + 1.0)).exp()
        };
    }
    if cumulative {
        let mut t = (-lambda).exp();
        let mut sum = t;
        let mut i = 1.0;
        while i <= k {
            t = t * lambda / i;
            sum += t;
            i += 1.0;
        }
        sum
    } else {
        let mut v = 1.0;
        let mut i = 0.0;
        while i < k {
            v *= lambda / (i + 1.0);
            i += 1.0;
        }
        v * (-lambda).exp()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn call(id: u32, args: &[f64]) -> CellValue {
        let a: Vec<CellValue> = args.iter().map(|x| CellValue::num(*x)).collect();
        eval(id, &a).unwrap_or_else(CellValue::Err)
    }

    fn num_of(v: CellValue) -> f64 {
        v.as_num().unwrap_or_else(|| panic!("not a number: {v:?}"))
    }

    fn close(got: f64, want: f64) -> bool {
        (got - want).abs() <= 1e-12 * want.abs().max(1e-300)
    }

    /// Each parameter's own domain check (the goldens hit the first failing
    /// one of each function; these hit the others).
    #[test]
    fn each_parameter_is_checked() {
        let t = CellValue::Bool(true);
        let nums = |v: &[f64]| v.iter().map(|x| CellValue::num(*x)).collect::<Vec<_>>();
        let with_flag = |v: &[f64], at: usize| {
            let mut a = nums(v);
            a.insert(at, t.clone());
            a
        };
        for (id, args) in [
            (500, with_flag(&[0.5, 2.0, 0.0], 3)),
            (502, nums(&[0.5, 0.0, 3.0])),
            (502, nums(&[0.5, 2.0, 0.0])),
            (502, nums(&[0.5, 2.0, 3.0, 1.0, 1.0])),
            (503, with_flag(&[-1.0, 10.0, 0.5], 3)),
            (505, nums(&[10.0, 1.5, 2.0])),
            (505, nums(&[10.0, 0.5, -1.0])),
            (516, nums(&[1.5, 2.0, 2.0])),
            (518, with_flag(&[0.0, 1.0, 1.0, 0.0], 4)),
            (518, with_flag(&[0.0, 0.0, 1.0, 5.0], 4)),
            (518, with_flag(&[0.0, 1.0, 0.0, 5.0], 4)),
            (518, with_flag(&[0.0, 1.0, 6.0, 5.0], 4)),
        ] {
            assert_eq!(
                eval(id, &args),
                Err(err::NUM),
                "{id} {args:?} should be #NUM!"
            );
        }
    }

    /// Every distribution's value and #NUM! guards in this module's own
    /// binary: (formula over literals, LibreOffice 25.2's result; Err:502
    /// read as #NUM!, as the goldens do), evaluated through `eval`. The
    /// rows marked Excel docs keep Excel's documented error where
    /// LibreOffice answers (scripts/goldens/statistical.py's `excel=`).
    #[test]
    fn distributions_match_libreoffice() {
        let rows: &[(&str, Result<f64, u8>)] = &[
            ("BETA.DIST(0.4,2,3,TRUE)", Ok(0.5247999999999999)),
            ("BETA.DIST(0.4,2,3,FALSE)", Ok(1.728)),
            ("BETA.DIST(3,2,3,TRUE,2,5)", Ok(0.4074074074074075)),
            ("BETA.DIST(0.4,0,3,TRUE)", Err(err::NUM)),
            ("BETA.DIST(0.4,2,0,TRUE)", Err(err::NUM)),
            ("BETA.DIST(0.4,2,3,TRUE,1,0)", Err(err::NUM)),
            ("BETA.DIST(0.4,2,3,TRUE,0.5,1)", Err(err::NUM)),
            ("BETA.DIST(1.4,2,3,TRUE)", Err(err::NUM)),
            ("ERF(0.5,1.5)", Ok(0.44560526866226424)),
            ("ERF(0.5)", Ok(0.5204998778130465)),
            ("BETA.INV(0.4,2,3)", Ok(0.32916650337840775)),
            ("BETA.INV(0,2,3)", Err(err::NUM)), // Excel docs (LibreOffice Ok(0.0))
            ("BETA.INV(1.2,2,3)", Err(err::NUM)),
            ("BETA.INV(0.4,0,3)", Err(err::NUM)),
            ("BETA.INV(0.4,2,0)", Err(err::NUM)),
            ("BETA.INV(0.4,2,3,1,1)", Err(err::NUM)),
            ("BINOM.DIST(3,10,0.4,TRUE)", Ok(0.3822806015999998)),
            ("BINOM.DIST(3,10,0.4,FALSE)", Ok(0.2149908479999999)),
            ("BINOM.DIST(-1,10,0.4,FALSE)", Err(err::NUM)),
            ("BINOM.DIST(11,10,0.4,FALSE)", Err(err::NUM)),
            ("BINOM.DIST(3,10,1.4,FALSE)", Err(err::NUM)),
            ("BINOM.INV(10,0.4,0.5)", Ok(4.0)),
            ("BINOM.INV(-1,0.4,0.5)", Err(err::NUM)),
            ("BINOM.INV(10,1.4,0.5)", Err(err::NUM)),
            ("BINOM.INV(10,0.4,1.5)", Err(err::NUM)),
            ("BINOM.DIST.RANGE(10,0.4,3)", Ok(0.2149908479999999)),
            ("BINOM.DIST.RANGE(10,0.4,3,5)", Ok(0.6664716287999998)),
            ("BINOM.DIST.RANGE(10,1.4,3)", Err(err::NUM)),
            ("BINOM.DIST.RANGE(10,0.4,-1)", Err(err::NUM)),
            ("BINOM.DIST.RANGE(10,0.4,5,3)", Err(err::NUM)),
            ("BINOM.DIST.RANGE(10,0.4,3,11)", Err(err::NUM)),
            ("CHISQ.DIST(2,3,TRUE)", Ok(0.42759329552912023)),
            ("CHISQ.DIST(2,3,FALSE)", Ok(0.20755374871029736)),
            ("CHISQ.DIST.RT(2,3)", Ok(0.5724067044708798)),
            ("CHISQ.DIST(-2,3,TRUE)", Err(err::NUM)),
            ("CHISQ.DIST(2,0,TRUE)", Err(err::NUM)),
            ("CHISQ.INV(0.4,3)", Ok(1.8691684033887161)),
            ("CHISQ.INV.RT(0.4,3)", Ok(2.946166073101949)),
            ("CHISQ.INV(1.4,3)", Err(err::NUM)),
            ("CHISQ.INV(0.4,0)", Err(err::NUM)),
            ("EXPON.DIST(2,0.5,TRUE)", Ok(0.6321205588285577)),
            ("EXPON.DIST(2,0.5,FALSE)", Ok(0.18393972058572117)),
            ("EXPON.DIST(-2,0.5,TRUE)", Err(err::NUM)), // Excel docs (LibreOffice Ok(0.0))
            ("EXPON.DIST(2,0,TRUE)", Err(err::NUM)),
            ("F.DIST(2,3,4,TRUE)", Ok(0.7436128024718239)),
            ("F.DIST(2,3,4,FALSE)", Ok(0.13942740046346697)),
            ("F.DIST.RT(2,3,4)", Ok(0.25638719752817607)),
            ("F.DIST(-2,3,4,TRUE)", Err(err::NUM)),
            ("F.DIST(2,0,4,TRUE)", Err(err::NUM)),
            ("F.DIST(2,3,0,TRUE)", Err(err::NUM)),
            ("F.INV(0.4,3,4)", Ok(0.6985553545555817)),
            ("F.INV.RT(0.4,3,4)", Ok(1.2605877191043346)),
            ("F.INV(1.4,3,4)", Err(err::NUM)),
            ("F.INV(0.4,0,4)", Err(err::NUM)),
            ("F.INV(0.4,3,0)", Err(err::NUM)),
            ("GAMMA.DIST(2,3,4,TRUE)", Ok(0.014387677966970689)),
            ("GAMMA.DIST(2,3,4,FALSE)", Ok(0.018954083116019795)),
            ("GAMMA.DIST(-2,3,4,TRUE)", Err(err::NUM)),
            ("GAMMA.DIST(2,0,4,TRUE)", Err(err::NUM)),
            ("GAMMA.DIST(2,3,0,TRUE)", Err(err::NUM)),
            ("GAMMA.INV(0.4,3,4)", Ok(9.140307616013523)),
            ("GAMMA.INV(1.4,3,4)", Err(err::NUM)),
            ("GAMMA.INV(0.4,0,4)", Err(err::NUM)),
            ("GAMMA.INV(0.4,3,0)", Err(err::NUM)),
            ("HYPGEOM.DIST(2,5,4,20,TRUE)", Ok(0.9680082559339525)),
            ("HYPGEOM.DIST(2,5,4,20,FALSE)", Ok(0.21671826625386992)),
            ("HYPGEOM.DIST(2,5,4,0,FALSE)", Err(err::NUM)),
            ("HYPGEOM.DIST(2,0,4,20,FALSE)", Err(err::NUM)),
            ("HYPGEOM.DIST(2,25,4,20,FALSE)", Err(err::NUM)),
            ("HYPGEOM.DIST(2,5,0,20,FALSE)", Err(err::NUM)), // Excel docs (LibreOffice Ok(0.0))
            ("HYPGEOM.DIST(2,5,25,20,FALSE)", Err(err::NUM)),
            ("HYPGEOM.DIST(0,18,4,20,FALSE)", Err(err::NUM)), // Excel docs (LibreOffice Ok(0.0))
            ("HYPGEOM.DIST(5,5,4,20,FALSE)", Err(err::NUM)),  // Excel docs (LibreOffice Ok(0.0))
            ("LOGNORM.DIST(2,0.5,1.2,TRUE)", Ok(0.5639359658118673)),
            ("LOGNORM.DIST(2,0.5,1.2,FALSE)", Ok(0.1640866421418861)),
            ("LOGNORM.DIST(-2,0.5,1.2,TRUE)", Err(err::NUM)), // Excel docs (LibreOffice Ok(0.0))
            ("LOGNORM.DIST(2,0.5,0,TRUE)", Err(err::NUM)),
            ("LOGNORM.INV(0.4,0.5,1.2)", Ok(1.2165068038981746)),
            ("LOGNORM.INV(0,0.5,1.2)", Err(err::NUM)),
            ("LOGNORM.INV(1,0.5,1.2)", Err(err::NUM)),
            ("LOGNORM.INV(0.4,0.5,0)", Err(err::NUM)),
            ("NEGBINOM.DIST(3,4,0.4,TRUE)", Ok(0.28979199999999994)),
            ("NEGBINOM.DIST(3,4,0.4,FALSE)", Ok(0.11059200000000002)),
            ("NEGBINOM.DIST(-1,4,0.4,FALSE)", Err(err::NUM)),
            ("NEGBINOM.DIST(3,0,0.4,FALSE)", Err(err::NUM)),
            ("NEGBINOM.DIST(3,4,1.4,FALSE)", Err(err::NUM)),
            ("NORM.DIST(2,1,3,TRUE)", Ok(0.6305586598182363)),
            ("NORM.DIST(2,1,3,FALSE)", Ok(0.12579440923099772)),
            ("NORM.DIST(2,1,0,TRUE)", Err(err::NUM)),
            ("NORM.INV(0.4,1,3)", Ok(0.23995869059260067)),
            ("NORM.INV(0,1,3)", Err(err::NUM)), // Excel docs (LibreOffice Err(err::VALUE))
            ("NORM.INV(1,1,3)", Err(err::NUM)), // Excel docs (LibreOffice Err(err::VALUE))
            ("NORM.INV(0.4,1,0)", Err(err::NUM)),
            ("NORM.S.DIST(0.5,TRUE)", Ok(0.6914624612740131)),
            ("NORM.S.DIST(0.5,FALSE)", Ok(0.3520653267642995)),
            ("NORM.S.INV(0.4)", Ok(-0.2533471031357998)),
            ("NORM.S.INV(0)", Err(err::NUM)), // Excel docs (LibreOffice Err(err::VALUE))
            ("NORM.S.INV(1)", Err(err::NUM)), // Excel docs (LibreOffice Err(err::VALUE))
            ("POISSON.DIST(3,2.5,TRUE)", Ok(0.757576133133066)),
            ("POISSON.DIST(3,2.5,FALSE)", Ok(0.21376301724973648)),
            ("POISSON.DIST(-1,2.5,TRUE)", Err(err::NUM)),
            ("POISSON.DIST(3,-2.5,TRUE)", Err(err::NUM)),
            ("T.DIST(0.5,3,TRUE)", Ok(0.6742760175759245)),
            ("T.DIST(0.5,3,FALSE)", Ok(0.3131809110088287)),
            ("T.DIST.RT(0.5,3)", Ok(0.3257239824240755)),
            ("T.DIST(0.5,0,TRUE)", Err(err::NUM)),
            ("T.DIST.2T(0.5,3)", Ok(0.651447964848151)),
            ("TDIST(0.5,3,1)", Ok(0.3257239824240755)),
            ("T.DIST.2T(-0.5,3)", Err(err::NUM)),
            ("T.DIST.2T(0.5,0)", Err(err::NUM)),
            ("TDIST(0.5,3,3)", Err(err::NUM)),
            ("T.INV(0.3,3)", Ok(-0.584389727439818)),
            ("T.INV(0.7,3)", Ok(0.584389727439818)),
            ("T.INV(0,3)", Err(err::NUM)),
            ("T.INV(1.2,3)", Err(err::NUM)),
            ("T.INV(0.3,0)", Err(err::NUM)),
            ("T.INV.2T(0.3,3)", Ok(1.2497781050332248)),
            ("T.INV.2T(0,3)", Err(err::NUM)),
            ("T.INV.2T(1.2,3)", Err(err::NUM)),
            ("T.INV.2T(1,3)", Ok(0.0)),
            ("T.INV.2T(0.3,0)", Err(err::NUM)),
            ("WEIBULL.DIST(2,1.5,3,TRUE)", Ok(0.41977020402532716)),
            ("WEIBULL.DIST(2,1.5,3,FALSE)", Ok(0.2368778222828562)),
            ("WEIBULL.DIST(-2,1.5,3,TRUE)", Err(err::NUM)),
            ("WEIBULL.DIST(2,0,3,TRUE)", Err(err::NUM)),
            ("WEIBULL.DIST(2,1.5,0,TRUE)", Err(err::NUM)),
            ("FISHER(0.5)", Ok(0.5493061443340548)),
            ("FISHER(1)", Err(err::NUM)),
            ("FISHER(-1)", Err(err::NUM)),
            ("CONFIDENCE.NORM(0.05,2,10)", Ok(1.2395900646091227)),
            ("CONFIDENCE.T(0.05,2,10)", Ok(1.4307138119413296)),
            ("CONFIDENCE.T(0.05,2,1)", Err(err::DIV0)),
            ("CONFIDENCE.NORM(0,2,10)", Err(err::NUM)),
            ("CONFIDENCE.NORM(1,2,10)", Err(err::NUM)),
            ("CONFIDENCE.NORM(0.05,0,10)", Err(err::NUM)),
            ("CONFIDENCE.NORM(0.05,2,0)", Err(err::NUM)),
            ("STANDARDIZE(2,1,3)", Ok(0.3333333333333333)),
            ("STANDARDIZE(2,1,0)", Err(err::NUM)), // Excel docs (LibreOffice Err(err::DIV0))
            ("GAMMA(-1)", Err(err::NUM)),
            ("GAMMA(0.5)", Ok(1.7724538509055159)),
            ("GAMMALN(0)", Err(err::NUM)),
            ("GAMMALN(2.5)", Ok(0.2846828704729192)),
        ];
        for (formula, want) in rows {
            let (name, args) = formula.trim_end_matches(')').split_once('(').unwrap();
            let def = improv_core_model::functions::by_name(name)
                .find(|d| d.kind == improv_core_model::functions::Kind::Scalar)
                .unwrap_or_else(|| panic!("{name}"));
            let a: Vec<CellValue> = args
                .split(',')
                .map(|s| match s {
                    "TRUE" => CellValue::Bool(true),
                    "FALSE" => CellValue::Bool(false),
                    _ => CellValue::num(s.parse().unwrap()),
                })
                .collect();
            let got = eval(def.id.0, &a).map(|v| v.as_num().unwrap());
            let same = match (got, want) {
                (Ok(g), Ok(w)) => close(g, *w),
                (g, w) => g == *w,
            };
            assert!(same, "{formula} = {got:?}, want {want:?}");
        }
    }

    /// The binomial CDF and I_x(a, b) with shapes past 1000, where statrs'
    /// prefactor cancels and its 140-term fraction stops short. References:
    /// P(K ≤ k) summed exactly in mpmath (40 digits) over ±45 sd; Gnumeric's
    /// BINOMDIST agrees to 16 digits, LibreOffice to about 8.
    #[test]
    fn binomial_and_beta_with_huge_shapes() {
        for (n, p, k, want) in [
            (1e8, 0.4, 4e7, 0.500_043_431_334_254_2),
            (1e9, 0.4, 4e8, 0.500_013_734_193_845_4),
            (1e10, 0.01, 1e8, 0.500_026_596_487_800_7),
        ] {
            let got = binom_range(n, p, 0.0, k);
            assert!(
                (got - want).abs() <= 1e-10 * want,
                "P(K ≤ {k} | {n}, {p}) = {got}, want {want}"
            );
            // BINOM.DIST through eval.
            let v = eval(
                503,
                &[
                    CellValue::num(k),
                    CellValue::num(n),
                    CellValue::num(p),
                    CellValue::Bool(true),
                ],
            );
            assert!((num_of(v.unwrap()) - want).abs() <= 1e-10 * want);
        }
        // Where statrs is still accurate (shapes just past 1000), the two
        // agree; both tails sum to 1.
        for (a, b, x) in [
            (1500.0, 2500.0, 0.37),
            (1500.0, 2500.0, 0.38),
            (3000.0, 1001.0, 0.75),
        ] {
            let old = sb::checked_beta_reg(a, b, x).unwrap();
            let new = beta_reg(a, b, x);
            assert!(
                (new - old).abs() <= 1e-10 * old.max(1e-300),
                "I_{x}({a}, {b}) = {new}, statrs {old}"
            );
            assert!((new + beta_reg(b, a, 1.0 - x) - 1.0).abs() < 1e-13);
        }
        // The large-shape path for both shapes past 1000, x inside (0, 1),
        // on each side of the mean (mpmath, 40 digits; statrs is 4e-13 off
        // here): I_0.4(2000, 3000) and I_0.39(2000, 3000).
        assert!((beta_reg(2000.0, 3000.0, 0.4) - 0.500_767_787_945_590_7).abs() < 1e-14);
        assert!((beta_reg(2000.0, 3000.0, 0.39) - 0.074_150_549_711_298_32).abs() < 1e-15);
        // The ends, and a shape at or under 1000, stay with statrs.
        assert_eq!(beta_reg(1500.0, 1500.0, 0.0), 0.0);
        assert_eq!(beta_reg(1500.0, 1500.0, 1.0), 1.0);
        for (a, b) in [(1500.0, 500.0), (500.0, 1500.0)] {
            assert_eq!(
                beta_reg(a, b, 0.6),
                sb::checked_beta_reg(a, b, 0.6).unwrap()
            );
        }
    }

    #[test]
    fn unknown_ids_are_name_errors() {
        assert_eq!(call(199, &[1.0]), CellValue::Err(err::NAME));
        assert_eq!(call(699, &[1.0]), CellValue::Err(err::NAME));
    }

    #[test]
    fn non_numbers_are_value_errors() {
        let t = CellValue::Text("x".into());
        assert_eq!(eval(530, std::slice::from_ref(&t)), Err(err::VALUE));
        assert_eq!(eval(527, &[CellValue::num(1.0), t]), Err(err::VALUE));
    }

    #[test]
    fn special_function_edges() {
        assert_eq!(num_of(call(160, &[5.0])), 24.0);
        assert_eq!(call(160, &[172.0]), CellValue::Err(err::NUM));
        assert_eq!(num_of(call(161, &[1.0])), 0.0);
        assert_eq!(num_of(call(161, &[2.0])), 0.0);
        // BETA with a + b past Γ's range goes through ln Γ.
        let want = (ln_gamma(100.5) + ln_gamma(80.25) - ln_gamma(180.75)).exp();
        assert!(close(num_of(call(162, &[100.5, 80.25])), want));
        assert!(close(
            num_of(call(163, &[100.5, 80.25])),
            ln_gamma(100.5) + ln_gamma(80.25) - ln_gamma(180.75)
        ));
        // IGAMMA at a = 0 and x = 0.
        let i = |args: &[CellValue]| eval(165, args).unwrap_or_else(CellValue::Err);
        let (z, one) = (CellValue::num(0.0), CellValue::num(1.0));
        let (t, f) = (CellValue::Bool(true), CellValue::Bool(false));
        assert_eq!(i(&[z.clone(), one.clone()]), CellValue::num(1.0));
        assert_eq!(i(&[z.clone(), one.clone(), f.clone()]), CellValue::num(0.0));
        assert_eq!(
            i(&[z.clone(), one.clone(), t.clone(), f.clone()]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(i(&[one.clone(), z.clone()]), CellValue::num(0.0));
        assert_eq!(i(&[one.clone(), z, f]), CellValue::num(1.0));
        // POCHHAMMER's large and pole branches.
        let want = (ln_gamma(300.5) - ln_gamma(200.0)).exp();
        assert!(close(num_of(call(166, &[200.0, 100.5])), want));
        assert_eq!(call(166, &[-1.5, -0.5]), CellValue::Err(err::NUM));
        assert_eq!(num_of(call(166, &[-3.0, 0.5])), 0.0);
        assert_eq!(num_of(call(166, &[0.0, 3.0])), 0.0);
        assert_eq!(num_of(call(166, &[0.0, -2.0])), 0.5);
        assert_eq!(call(166, &[-3.0, 5.0]), CellValue::Err(err::NUM));
        assert_eq!(num_of(call(166, &[-3.0, 2.0])), 6.0);
        assert_eq!(num_of(call(166, &[2.5, 0.0])), 1.0);
        // Non-integer n with x or x + n negative: the Γ ratio.
        assert!(close(
            num_of(call(166, &[-1.5, 0.25])),
            gamma(-1.25) / gamma(-1.5)
        ));
        assert!(close(
            num_of(call(166, &[0.5, -1.25])),
            gamma(-0.75) / gamma(0.5)
        ));
        // A large integer n is the Γ ratio too.
        assert!(close(
            num_of(call(166, &[1.5, 150.0])),
            gamma(151.5) / gamma(1.5)
        ));
    }

    #[test]
    fn kernel_edges() {
        assert!(incomplete_gamma(1e13, 1.0, true).is_nan());
        assert!(incomplete_gamma(1.0, f64::NAN, true).is_nan());
        assert_eq!(gamma_p(2.0, f64::INFINITY), 1.0);
        assert_eq!(gamma_q(2.0, f64::INFINITY), 0.0);
        // One g for every exit (solve is generic: one closure type is one
        // instantiation): at lo already, a target never reached (the
        // bracket doubles to infinity), and a step where bisection narrows
        // to adjacent doubles and stops.
        // (The step at c, for c = 2.5 the midpoint rounds onto hi, for
        // c = 0.3 onto lo.)
        let c = std::cell::Cell::new(2.5);
        let g = |x: f64| if x < c.get() { 0.0 } else { 1.0 };
        assert_eq!(solve(g, 0.0, 0.0, 1.0), 0.0);
        assert!(solve(g, 2.0, 0.0, 1.0).is_nan());
        let step = solve(g, 0.5, 0.0, 1.0);
        assert!((step - 2.5).abs() <= 4.0 * f64::EPSILON, "{step}");
        c.set(0.3);
        let step = solve(g, 0.5, 0.0, 1.0);
        assert!((step - 0.3).abs() <= f64::EPSILON, "{step}");
        // The binomial with p = 0: all its mass at 0.
        assert_eq!(binom_range(10.0, 0.0, 0.0, 3.0), 1.0);
        assert_eq!(binom_range(10.0, 0.0, 1.0, 3.0), 0.0);
        // Gnumeric's BETA: #NUM! at a non-positive integer a or b, 0 where
        // only a + b is one, and a negative fractional a (0.000883).
        assert_eq!(call(162, &[-1.0, 1.5]), CellValue::Err(err::NUM));
        assert_eq!(call(162, &[1.5, -1.0]), CellValue::Err(err::NUM));
        assert_eq!(call(162, &[-0.5, -0.5]), CellValue::num(0.0));
        assert!(close(
            num_of(call(162, &[-100.5, 100.0])),
            0.000_882_920_793_175_656_8
        ));
        // IGAMMA of a negative a or x is complex-valued: #NUM! here.
        assert_eq!(call(165, &[-2.0, 1.0]), CellValue::Err(err::NUM));
        assert_eq!(call(165, &[2.0, -1.0]), CellValue::Err(err::NUM));
        // IGAMMA's imaginary part over the real domain is 0 (Gnumeric).
        let im = eval(
            165,
            &[2.0, 3.0]
                .map(CellValue::num)
                .iter()
                .cloned()
                .chain([
                    CellValue::Bool(true),
                    CellValue::Bool(true),
                    CellValue::Bool(false),
                ])
                .collect::<Vec<_>>(),
        );
        assert_eq!(im, Ok(CellValue::num(0.0)));
        assert_eq!(gamma_inv(2.0, 1.0, 0.0), f64::INFINITY);
        assert_eq!(f_inv(2.0, 3.0, 1.0, 0.0), f64::INFINITY);
        assert_eq!(f_pdf(0.0, 1.0, 5.0), f64::INFINITY);
        assert_eq!(f_pdf(0.0, 2.0, 5.0), 1.0);
        assert_eq!(f_pdf(0.0, 3.0, 5.0), 0.0);
        assert_eq!(beta_pdf(0.0, 1.0, 1.0, 3.0), 3.0);
        assert_eq!(beta_pdf(1.0, 0.0, 2.0, 1.0), 2.0);
        assert_eq!(gamma_pdf(0.0, 0.5, 2.0), Err(err::DIV0));
        assert_eq!(gamma_pdf(0.0, 1.0, 2.0), Ok(0.5));
        assert_eq!(gamma_pdf(0.0, 3.0, 2.0), Ok(0.0));
        assert!(ln_beta(2.0, -0.5).is_finite());
        // ln |B(a, b)| with a negative argument either side (Gnumeric
        // BETALN 1.3862943611198906 = ln 4).
        assert!(close(ln_beta(-0.5, 2.0), 1.386_294_361_119_890_6));
        assert!(close(ln_beta(2.0, -0.5), 1.386_294_361_119_890_6));
        assert!(incomplete_gamma(0.0, 1.0, true).is_nan());
        assert_eq!(beta_pdf(0.0, 1.0, 2.0, 3.0), 0.0);
        assert_eq!(beta_pdf(1.0, 0.0, 2.0, 3.0), 0.0);
        // Φ⁻¹ of the smallest subnormal stays finite.
        let z = norm_inv(f64::from_bits(1));
        assert!(z.is_finite() && z < -38.0, "{z}");
        // ln C(n, k) past f64's range, by the product and by ln Γ.
        assert!(close(
            ln_choose(1e6, 1000.0),
            ln_gamma(1e6 + 1.0) - ln_gamma(1001.0) - ln_gamma(1e6 - 999.0)
        ));
        assert!(close(
            ln_choose(5000.0, 2500.0),
            ln_gamma(5001.0) - 2.0 * ln_gamma(2501.0)
        ));
    }

    #[test]
    fn binomial_large_n_uses_the_incomplete_beta() {
        // q^n underflows: P(K ≤ 5000) for n = 10000, p = ½ is
        // ½ + P(K = 5000)/2 ≈ ½ + 1/(2√(5000π)) (1 − 1/40000).
        let v = binom_range(10000.0, 0.5, 0.0, 5000.0);
        let want = 0.5 + (1.0 - 1.0 / 40000.0) / (2.0 * (5000.0 * std::f64::consts::PI).sqrt());
        assert!((v - want).abs() < 1e-9, "{v}");
        assert_eq!(binom_range(10000.0, 0.5, 0.0, 10000.0), 1.0);
        assert_eq!(binom_inv(10000.0, 0.5, 0.5), 5000.0);
        assert_eq!(binom_inv(10.0, 0.0, 1.0), 0.0);
        assert_eq!(binom_inv(10.0, 0.3, 1.0), 10.0);
        assert_eq!(binom_range(5.0, 1.0, 2.0, 5.0), 1.0);
        assert_eq!(binom_range(5.0, 1.0, 2.0, 4.0), 0.0);
        // A single mass where q^n underflows, or past a million terms.
        let pmf = binom_range(2000.0, 0.5, 1000.0, 1000.0);
        assert!((pmf - 0.017_839_011_145_854_32).abs() < 1e-14, "{pmf}");
        assert!(binom_range(2e6, 1e-7, 1.5e6, 1.5e6) < 1e-300);
        assert_eq!(binom_range(2e6, 1e-7, 0.0, 1.5e6), 1.0);
    }

    #[test]
    fn hypergeometric_cdf_above_the_mode() {
        // Above the mode it sums the upper tail: P(X ≤ 3) = 1 − P(X = 4).
        let want = 1.0 - hypgeom_pmf(4.0, 4.0, 8.0, 20.0);
        assert!(close(hypgeom_cdf(3.0, 4.0, 8.0, 20.0), want));
        let below: f64 = (0..=30)
            .map(|i| hypgeom_pmf(f64::from(i), 100.0, 400.0, 1000.0))
            .sum();
        assert!(close(hypgeom_cdf(30.0, 100.0, 400.0, 1000.0), below));
        let above: f64 = (61..=100)
            .map(|i| hypgeom_pmf(f64::from(i), 100.0, 400.0, 1000.0))
            .sum();
        assert!((hypgeom_cdf(60.0, 100.0, 400.0, 1000.0) - (1.0 - above)).abs() < 1e-15);
    }

    #[test]
    fn large_count_closed_forms() {
        // NEGBINOM with many failures, Poisson past 1000 terms or λ > 712.
        // C(f + s − 1, f) p^s q^f with f = 200001, s = 3.
        let c = 200_003.0 * 200_002.0 / 2.0;
        let want = c * 1e-15 * (200_001.0 * (-1e-5f64).ln_1p()).exp();
        let got = negbinom_pmf(200_001.0, 3.0, 1e-5);
        assert!((got - want).abs() < 1e-9 * want, "{got} {want}");
        let pmf = poisson(1500.0, 1500.0, false);
        assert!((pmf - 0.010_299_9).abs() < 1e-6, "{pmf}");
        let cdf = poisson(800.0, 800.0, true);
        assert!((cdf - 0.509_4).abs() < 1e-3, "{cdf}");
        let many = poisson(1001.0, 700.0, true);
        assert!(close(many, gamma_q(1002.0, 700.0)));
        assert_eq!(poisson(0.0, 0.0, false), 1.0);
        assert_eq!(poisson(3.0, 0.0, false), 0.0);
        assert_eq!(poisson(3.0, 0.0, true), 1.0);
    }

    #[test]
    fn quantiles_invert_their_distributions() {
        for &(p, a, b) in &[(0.01, 0.5, 3.0), (0.7, 2.0, 0.3), (0.999, 5.0, 5.0)] {
            let x = beta_inv(p, a, b);
            assert!((beta_reg(a, b, x) - p).abs() < 1e-14, "{p} {a} {b}");
        }
        for &(p, a) in &[(1e-9, 0.25), (0.3, 4.0), (0.9999, 30.0)] {
            let x = gamma_inv(a, p, 1.0 - p);
            assert!((gamma_p(a, x) - p).abs() < 1e-14, "{p} {a}");
        }
    }

    /// BINOM.INV past 2⁵³ trials returns (Gnumeric's CRITBINOM(1E16, 0.5,
    /// 0.5) is 5e15); its bisection stepped to mid + 1 = mid forever.
    #[test]
    fn binom_inv_with_huge_n() {
        let t0 = std::time::Instant::now();
        assert_eq!(call(504, &[1e16, 0.5, 0.5]), CellValue::num(5e15));
        assert_eq!(
            call(504, &[f64::MAX, f64::MIN_POSITIVE, 1e-300]),
            CellValue::num(0.0)
        );
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
        // Small n is unchanged (Excel's documented BINOM.INV(6, 0.5, 0.75)).
        assert_eq!(call(504, &[6.0, 0.5, 0.75]), CellValue::num(4.0));
        assert_eq!(call(504, &[6.0, 0.5, 0.0]), CellValue::num(0.0));
        assert_eq!(call(504, &[6.0, 0.5, 1.0]), CellValue::num(6.0));
    }

    /// The quantile bisections take a bounded number of steps whatever the
    /// bracket: BETA.INV with shapes 1e15 and 1.8e293 took 16 s (1100
    /// halvings, each a million-term continued fraction), BINOM.INV with
    /// 1e300 trials 12 s.
    #[test]
    fn bisections_take_few_steps() {
        let steps = std::cell::Cell::new(0);
        let g = |x: f64| {
            steps.set(steps.get() + 1);
            x
        };
        assert_eq!(solve(g, 1e-300, 0.0, 1.0), 1e-300);
        assert!(steps.get() < 70, "{} steps", steps.get());
        steps.set(0);
        assert_eq!(solve(g, 5e-324, 0.0, 1.0), 5e-324);
        assert!(steps.get() < 70, "{} steps", steps.get());
        steps.set(0);
        let ok = |k: f64| {
            steps.set(steps.get() + 1);
            k >= 3e299
        };
        assert_eq!(int_search(0.0, 1e300, ok), 3e299);
        assert!(steps.get() < 140, "{} steps", steps.get());
        steps.set(0);
        assert_eq!(int_search(-1.0, 1e300, |k| k >= 0.0), 0.0);
        assert_eq!(int_search(5.0, 9.0, |k| k >= 7.0), 7.0);
        assert_eq!(int_search(5.0, 6.0, |_| true), 6.0);
        // Past 2⁵³ the floats are 2 apart: nothing lies between these.
        let big = 9_007_199_254_740_994.0;
        assert_eq!(int_search(big, big + 2.0, |_| true), big + 2.0);
        assert_eq!(int_search(9e15, 9e15 + 2.0, |_| true), 9e15 + 1.0);
        assert_eq!(float_mid(-1.0, 1.0), 0.0);
        assert!(float_mid(-4.0, -1.0) < -1.0 && float_mid(-4.0, -1.0) > -4.0);
        assert_eq!(
            call(502, &[1.0, 1.797_693_134_862_314_3e293, 1e15]),
            CellValue::num(1.0)
        );
        assert_eq!(call(502, &[0.5, 2.0, 2.0]), CellValue::num(0.5));
    }

    /// I_x(a, b) with one shape below the other's ulp: the front took
    /// b − 1 as (a + b − 2) − (a − 1) = 0, its log was NaN, and the
    /// continued fraction ran a million terms on it. R.PBETA(0.5, 1e15,
    /// 1.34e154) was 0, not 1, and BETA.INV(0.5, 1e15, 1.34e154) bisected
    /// on those for 0.8 s, to 1.
    #[test]
    fn beta_far_from_the_mean_of_huge_shapes() {
        let t0 = std::time::Instant::now();
        let b = 1.340_780_792_994_259_7e154;
        assert_eq!(beta_reg(1e15, b, 0.5), 1.0);
        assert_eq!(beta_reg(b, 1e15, 0.5), 0.0);
        assert_eq!(beta_reg(1e15, b, 1e-100), 1.0);
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
        let q = call(502, &[0.5, 1e15, b]).as_num().unwrap();
        assert!((q / 7.458e-140 - 1.0).abs() < 1e-3, "{q}");
        // Above the median, a quantile under ε (Gnumeric 9.976418892862951e-286;
        // the median (a − 1/3)/b is 1e-285): it was 0.
        let q = call(502, &[0.6, 1e15, 1e300]).as_num().unwrap();
        assert!((q / 1e-285 - 1.0).abs() < 1e-5, "{q}");
        // Shapes whose sum overflows (Gnumeric's R.PBETA(0.5, 1E15,
        // 1.797E308) is 1; it was 0, and BETA.INV took 0.8 s on NaNs).
        let t0 = std::time::Instant::now();
        assert_eq!(beta_reg(1e15, f64::MAX, 0.5), 1.0);
        assert_eq!(beta_reg(f64::MAX, 1e15, 0.5), 0.0);
        assert_eq!(beta_reg(f64::MAX, f64::MAX, 0.25), 0.0);
        assert_eq!(beta_reg(f64::MAX, f64::MAX, 0.75), 1.0);
        let q = call(502, &[0.5, 1e15, 1e308]).as_num().unwrap();
        assert!((q / 1e-293 - 1.0).abs() < 1e-5, "{q}");
        assert!(t0.elapsed() < std::time::Duration::from_secs(10));
    }
}
