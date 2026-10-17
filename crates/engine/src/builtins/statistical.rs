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
fn ln_gamma(x: f64) -> f64 {
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
fn ln_beta(a: f64, b: f64) -> f64 {
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
fn gamma_p(a: f64, x: f64) -> f64 {
    incomplete_gamma(a, x, true)
}

/// The regularized upper incomplete gamma Q(a, x), a > 0.
fn gamma_q(a: f64, x: f64) -> f64 {
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

/// The regularized incomplete beta I_x(a, b).
fn beta_reg(a: f64, b: f64, x: f64) -> f64 {
    sb::checked_beta_reg(a, b, x.clamp(0.0, 1.0)).unwrap_or(f64::NAN)
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
/// by doubling, then bisects until it cannot shrink (about 1100 steps at
/// worst, for a quantile among the subnormals).
fn solve(g: impl Fn(f64) -> f64, target: f64, mut lo: f64, mut hi: f64) -> f64 {
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
    for _ in 0..2200 {
        let mid = lo + (hi - lo) / 2.0;
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

/// The gamma(a, 1) quantile for left tail p (= 1 − q): solves the tail
/// that is under ½ so neither saturates.
fn gamma_inv(a: f64, p: f64, q: f64) -> f64 {
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
    if p <= 0.5 {
        solve(|x| beta_reg(a, b, x), p, 0.0, 1.0)
    } else {
        // 1 − I_x(a, b) = I_{1−x}(b, a): solve for y = 1 − x.
        1.0 - solve(|y| beta_reg(b, a, y), 1.0 - p, 0.0, 1.0)
    }
}

fn norm_cdf(z: f64) -> f64 {
    0.5 * libm::erfc(-z / SQRT_2)
}

fn norm_pdf(z: f64) -> f64 {
    FRAC_1_SQRT_2PI * (-z * z / 2.0).exp()
}

/// Φ⁻¹(p), p ∈ (0, 1): statrs's estimate, then Newton steps on Φ. The
/// upper half mirrors the lower (1 − p is exact for p ≥ ½), so Φ is only
/// ever evaluated in the left tail, where it keeps its relative precision.
fn norm_inv(p: f64) -> f64 {
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

fn f_cdf(x: f64, d1: f64, d2: f64) -> f64 {
    beta_reg(d1 / 2.0, d2 / 2.0, d1 * x / (d1 * x + d2))
}

fn f_sf(x: f64, d1: f64, d2: f64) -> f64 {
    beta_reg(d2 / 2.0, d1 / 2.0, d2 / (d2 + d1 * x))
}

fn f_pdf(x: f64, d1: f64, d2: f64) -> f64 {
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
fn f_inv(d1: f64, d2: f64, p: f64, q: f64) -> f64 {
    if q <= 0.0 {
        f64::INFINITY
    } else if p <= 0.5 {
        solve(|x| f_cdf(x, d1, d2), p, 0.0, 1.0)
    } else {
        solve(|x| -f_sf(x, d1, d2), -q, 0.0, 1.0)
    }
}

/// P(0 < T < t) for t ≥ 0: the central mass, precise near the median.
fn t_central(t: f64, df: f64) -> f64 {
    let z = t * t;
    0.5 * beta_reg(0.5, df / 2.0, z / (df + z))
}

/// Student's t right tail P(T > t).
fn t_sf(t: f64, df: f64) -> f64 {
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

fn t_pdf(t: f64, df: f64) -> f64 {
    (-(df + 1.0) / 2.0 * (t * t / df).ln_1p() - 0.5 * df.ln() - ln_beta(0.5, df / 2.0)).exp()
}

/// The t ≥ 0 with right tail q ∈ [0, ½] (infinite at 0).
fn t_upper_inv(q: f64, df: f64) -> f64 {
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
fn binom_range(n: f64, p: f64, lo: f64, hi: f64) -> f64 {
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
    let (mut lo, mut hi) = (0.0, n);
    while lo < hi {
        let mid = ((lo + hi) / 2.0).floor();
        if binom_range(n, p, 0.0, mid) >= alpha {
            hi = mid;
        } else {
            lo = mid + 1.0;
        }
    }
    lo
}

/// C(n, k) as a product while it is small, else through ln Γ.
fn ln_choose(n: f64, k: f64) -> f64 {
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
fn hypgeom_pmf(k: f64, n: f64, big_k: f64, big_n: f64) -> f64 {
    (ln_choose(big_k, k) + ln_choose(big_n - big_k, n - k) - ln_choose(big_n, n)).exp()
}

/// P(X ≤ k): sums the tail on k's side of the mode, from k outward (the
/// terms fall away from the mode, so the sum stops once they stop counting).
fn hypgeom_cdf(k: f64, n: f64, big_k: f64, big_n: f64) -> f64 {
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
fn negbinom_pmf(f: f64, s: f64, p: f64) -> f64 {
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
fn poisson(k: f64, lambda: f64, cumulative: bool) -> f64 {
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
        assert!(solve(|_| 0.0, 1.0, 0.0, 1.0).is_nan());
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
}
