//! Evaluation of the P7 distributions (ids 1550–1749): Gnumeric's extra
//! distributions and its R-style kernels `R.D*`, `R.P*`, `R.Q*`.
//!
//! Gnumeric 1.12.57 is the reference: its argument order, defaults and
//! domain rules (a NaN from its kernel is `#NUM!` here). The kernels are
//! written from the textbook formulas, reusing `statistical`'s Γ, B, the
//! incomplete functions and Φ.
//!
//! The R-style functions take R's flags: `give_log` (a density's logarithm),
//! `lower_tail` (P(X ≤ x), or P(X > x) when FALSE) and `log_p` (the
//! probability as a logarithm, in or out). Each family is one kernel
//! answering an [`Op`]; the id is `1600 + 3 × family + {0: D, 1: P, 2: Q}`.

use super::statistical as st;
use super::{b, domain, err, n, ok_num, R};
use crate::CellValue;
use std::f64::consts::{FRAC_1_SQRT_2, PI};

const INF: f64 = f64::INFINITY;
const NEG_INF: f64 = f64::NEG_INFINITY;
const NAN: f64 = f64::NAN;

/// ln √(2π).
const LN_SQRT_2PI: f64 = 0.918_938_533_204_672_7;

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
        1554 => ok_num(landau(x(0)?)),
        1560 => ok_num(norm_range(x(0)?, x(1)?)),
        1561 => ok_num(owen_t(x(0)?, x(1)?)),
        1562 => {
            let (h, k, rho) = (x(0)?, x(1)?, x(2)?);
            domain(rho.abs() < 1.0)?;
            ok_num(biv_norm(h, k, rho))
        }
        1600..=1656 => r_style(id, a),
        1657 | 1658 => tukey(id, a),
        _ => Err(err::NAME),
    }
}

// --- R-style kernels --------------------------------------------------------

/// What an R-style call asks of its family's kernel.
#[derive(Clone, Copy, Debug)]
enum Op {
    /// The density (or mass) at x; its logarithm if `true` (`give_log`).
    D(bool),
    /// P(X ≤ x) if `lower`, else P(X > x); its logarithm if `log`.
    P { lower: bool, log: bool },
    /// The quantile of a probability.
    Q(Tail),
}

/// A quantile's probability argument as R passes it: of the lower tail or
/// the upper, maybe as a logarithm.
#[derive(Clone, Copy, Debug)]
struct Tail {
    p: f64,
    lower: bool,
    log: bool,
}

impl Tail {
    /// In range: a probability, or a logarithm of one (≤ 0).
    fn valid(self) -> bool {
        if self.log {
            self.p <= 0.0
        } else {
            (0.0..=1.0).contains(&self.p)
        }
    }

    /// The lower-tail probability is exactly 0 (the quantile is the bottom
    /// of the support).
    fn at_bottom(self) -> bool {
        let none = if self.log { NEG_INF } else { 0.0 };
        let all = if self.log { 0.0 } else { 1.0 };
        self.p == if self.lower { none } else { all }
    }

    /// The lower-tail probability is exactly 1 (the top of the support).
    fn at_top(self) -> bool {
        let none = if self.log { NEG_INF } else { 0.0 };
        let all = if self.log { 0.0 } else { 1.0 };
        self.p == if self.lower { all } else { none }
    }

    /// The probability of the lower tail (`of_lower`) or of the upper.
    fn prob(self, of_lower: bool) -> f64 {
        match (self.log, self.lower == of_lower) {
            (false, true) => self.p,
            (false, false) => 1.0 - self.p,
            (true, true) => self.p.exp(),
            (true, false) => -self.p.exp_m1(),
        }
    }

    /// The logarithm of the lower tail's probability (`of_lower`) or of the
    /// upper's.
    fn ln(self, of_lower: bool) -> f64 {
        match (self.log, self.lower == of_lower) {
            (false, true) => self.p.ln(),
            (false, false) => (-self.p).ln_1p(),
            (true, true) => self.p,
            (true, false) => ln_1m_exp(self.p),
        }
    }
}

/// ln(1 − e^x) for x ≤ 0, precise at both ends (Gnumeric's `swap_log_tail`).
fn ln_1m_exp(x: f64) -> f64 {
    if x > -std::f64::consts::LN_2 {
        (-x.exp_m1()).ln()
    } else {
        (-x.exp()).ln_1p()
    }
}

/// A CDF result from its two tails, as R's flags ask for it (the
/// logarithm of a tail over ½ through the other, so it keeps its digits).
fn tails(lower: bool, log: bool, below: f64, above: f64) -> f64 {
    let (v, other) = if lower {
        (below, above)
    } else {
        (above, below)
    };
    if !log {
        v
    } else if v > 0.5 {
        (-other).ln_1p()
    } else {
        v.ln()
    }
}

/// A density from its logarithm, as `give_log` asks.
fn dens(log: bool, ln: f64) -> f64 {
    if log {
        ln
    } else {
        ln.exp()
    }
}

/// A quantile at the ends of the probability range (`bottom` and `top` of
/// the support), or NaN for a probability out of range; None inside.
fn q_ends(t: Tail, bottom: f64, top: f64) -> Option<f64> {
    if !t.valid() {
        Some(NAN)
    } else if t.at_bottom() {
        Some(bottom)
    } else if t.at_top() {
        Some(top)
    } else {
        None
    }
}

/// A quantile's lower- and upper-tail probabilities, for the inversions
/// that solve the smaller tail.
// ponytail: a log_p so negative that its tail underflows (below about −745)
// is NaN here; a log-space inversion would extend the range.
fn split(t: Tail) -> Option<(f64, f64)> {
    let (below, above) = (t.prob(true), t.prob(false));
    (below > 0.0 && above > 0.0).then_some((below, above))
}

/// A CDF result from the logarithm `ln` of one tail (the lower if
/// `of_lower`), as R's flags ask for it.
fn from_ln(ln: f64, of_lower: bool, lower: bool, log: bool) -> f64 {
    match (lower == of_lower, log) {
        (true, true) => ln,
        (true, false) => ln.exp(),
        (false, true) => ln_1m_exp(ln),
        (false, false) => -ln.exp_m1(),
    }
}

/// A family's kernel: the observation (or probability) x, the parameters,
/// and what to compute. NaN is a domain error.
type Kernel = fn(f64, &[f64], Op) -> f64;

/// The R-style families by index: the number of parameters and the kernel.
fn r_family(family: u32) -> Option<(usize, Kernel)> {
    Some(match family {
        0 => (2, beta),
        1 => (2, binom),
        2 => (2, cauchy),
        3 => (1, chisq),
        4 => (1, exp),
        5 => (2, f),
        6 => (2, gamma),
        7 => (1, geom),
        8 => (2, gumbel),
        9 => (3, hyper),
        10 => (2, lnorm),
        11 => (2, nbinom),
        12 => (2, norm),
        13 => (1, pois),
        14 => (1, rayleigh),
        15 => (3, snorm),
        16 => (2, skew_t),
        17 => (1, t),
        18 => (2, weibull),
        _ => return None,
    })
}

/// An `R.D*`, `R.P*` or `R.Q*` call: x (or p), the family's parameters,
/// then its optional flags.
fn r_style(id: u32, a: &[CellValue]) -> R {
    let (family, kind) = ((id - 1600) / 3, (id - 1600) % 3);
    let Some((k, kernel)) = r_family(family) else {
        return Err(err::NAME);
    };
    let v = n(a, 0)?;
    let mut p = [0.0; 3];
    for (i, slot) in p.iter_mut().enumerate().take(k) {
        *slot = n(a, i + 1)?;
    }
    let flag = |i: usize, default: bool| if a.len() > i { b(a, i) } else { Ok(default) };
    let op = match kind {
        0 => Op::D(flag(k + 1, false)?),
        1 => Op::P {
            lower: flag(k + 1, true)?,
            log: flag(k + 2, false)?,
        },
        _ => Op::Q(Tail {
            p: v,
            lower: flag(k + 1, true)?,
            log: flag(k + 2, false)?,
        }),
    };
    ok_num(kernel(v, &p[..k], op))
}

/// The density's zero, as `give_log` asks.
fn d0(log: bool) -> f64 {
    if log {
        NEG_INF
    } else {
        0.0
    }
}

fn norm(x: f64, a: &[f64], op: Op) -> f64 {
    let (mu, sd) = (a[0], a[1]);
    match op {
        Op::D(log) => {
            if sd < 0.0 {
                return NAN;
            }
            let z = (x - mu).abs() / sd;
            if z.is_infinite() {
                d0(log)
            } else if log {
                -(LN_SQRT_2PI + 0.5 * z * z + sd.ln())
            } else {
                st::norm_pdf(z) / sd
            }
        }
        Op::P { lower, log } => {
            if sd < 0.0 {
                return NAN;
            }
            // σ = 0 is a point mass at μ.
            let z = if sd == 0.0 {
                if x < mu {
                    NEG_INF
                } else {
                    INF
                }
            } else {
                (x - mu) / sd
            };
            norm_p(if lower { z } else { -z }, log)
        }
        Op::Q(t) => {
            if !t.valid() {
                NAN
            } else if t.at_bottom() {
                NEG_INF
            } else if t.at_top() {
                INF
            } else if sd < 0.0 {
                NAN
            } else if sd == 0.0 {
                mu
            } else {
                mu + sd * norm_q(t)
            }
        }
    }
}

/// Φ(z), or ln Φ(z) where Φ itself underflows (z < −37.5) by the
/// continued fraction of Mills' ratio.
fn norm_p(z: f64, log: bool) -> f64 {
    if !log {
        st::norm_cdf(z)
    } else if z > 0.0 {
        (-st::norm_cdf(-z)).ln_1p()
    } else if z > -37.5 {
        st::norm_cdf(z).ln()
    } else {
        // Φ(z) = φ(z) / (t + 1/(t + 2/(t + 3/(t + …)))), t = −z; 40 terms
        // are exact to the last bit for t ≥ 37.5.
        let t = -z;
        let mut f = t;
        for k in (1..=40).rev() {
            f = t + f64::from(k) / f;
        }
        -0.5 * z * z - LN_SQRT_2PI - f.ln()
    }
}

/// The standard normal quantile of a probability strictly inside (0, 1):
/// Φ⁻¹ of the smaller tail, or, where that tail underflows (a log_p below
/// about −690), the root of ln Φ.
fn norm_q(t: Tail) -> f64 {
    let (below, above) = (t.prob(true), t.prob(false));
    if (1e-300..=0.5).contains(&below) {
        st::norm_inv(below)
    } else if (1e-300..=0.5).contains(&above) {
        -st::norm_inv(above)
    } else {
        let tiny_below = below < above;
        let ln = t.ln(tiny_below);
        let y = st::solve(|y| -norm_p(-y, true), -ln, 0.0, 1.0);
        if tiny_below {
            -y
        } else {
            y
        }
    }
}

fn lnorm(x: f64, a: &[f64], op: Op) -> f64 {
    let (mu, sd) = (a[0], a[1]);
    match op {
        Op::Q(_) => norm(x, a, op).exp(),
        _ if sd <= 0.0 => NAN,
        // P(X ≤ x) = 0 for x ≤ 0. (Gnumeric's plnorm returns 0 for the
        // upper tail too; R and Improv give 1.)
        Op::P { lower, log } if x <= 0.0 => tails(lower, log, 0.0, 1.0),
        Op::D(log) if x <= 0.0 => d0(log),
        Op::D(log) => {
            let z = (x.ln() - mu) / sd;
            if log {
                -(LN_SQRT_2PI + 0.5 * z * z + sd.ln() + x.ln())
            } else {
                st::norm_pdf(z) / (sd * x)
            }
        }
        Op::P { .. } => norm(x.ln(), a, op),
    }
}

fn exp(x: f64, a: &[f64], op: Op) -> f64 {
    let s = a[0];
    match op {
        Op::D(_) if s <= 0.0 => NAN,
        Op::D(log) if x < 0.0 => d0(log),
        Op::D(true) => -x / s - s.ln(),
        Op::D(false) => (-x / s).exp() / s,
        // A scale of 0 is a point mass at 0.
        _ if s < 0.0 => NAN,
        Op::P { lower, log } if x <= 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } => from_ln(-(x / s), false, lower, log),
        Op::Q(t) if !t.valid() => NAN,
        Op::Q(t) if t.at_bottom() => 0.0,
        Op::Q(t) => -s * t.ln(false),
    }
}

fn weibull(x: f64, a: &[f64], op: Op) -> f64 {
    let (k, s) = (a[0], a[1]);
    if k <= 0.0 || s <= 0.0 {
        return NAN;
    }
    match op {
        Op::D(log) if x < 0.0 => d0(log),
        Op::D(_) if x == 0.0 && k < 1.0 => INF,
        Op::D(log) => {
            let t1 = (x / s).powf(k - 1.0);
            let t2 = t1 * (x / s);
            if log {
                -t2 + (k * t1 / s).ln()
            } else {
                k * t1 * (-t2).exp() / s
            }
        }
        Op::P { lower, log } if x <= 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } => from_ln(-(x / s).powf(k), false, lower, log),
        Op::Q(t) if !t.valid() => NAN,
        Op::Q(t) if t.at_bottom() => 0.0,
        Op::Q(t) => s * (-t.ln(false)).powf(1.0 / k),
    }
}

fn rayleigh(x: f64, a: &[f64], op: Op) -> f64 {
    let s = a[0];
    if s <= 0.0 {
        return NAN;
    }
    let u = x / s;
    match op {
        Op::D(log) if x <= 0.0 => d0(log),
        Op::D(true) => u.ln() - s.ln() - 0.5 * u * u,
        Op::D(false) => rayleigh_pdf(x, s),
        Op::P { lower, log } if x <= 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } => from_ln(-0.5 * u * u, false, lower, log),
        Op::Q(t) if !t.valid() => NAN,
        Op::Q(t) if t.at_bottom() => 0.0,
        Op::Q(t) => s * (-2.0 * t.ln(false)).sqrt(),
    }
}

fn gumbel(x: f64, a: &[f64], op: Op) -> f64 {
    let (mu, beta) = (a[0], a[1]);
    if beta <= 0.0 {
        return NAN;
    }
    let z = (x - mu) / beta;
    match op {
        Op::D(log) => {
            let lp = -(z + (-z).exp());
            if log {
                lp - beta.ln()
            } else {
                lp.exp() / beta
            }
        }
        Op::P { lower, log } => from_ln(-(-z).exp(), true, lower, log),
        Op::Q(t) if !t.valid() => NAN,
        Op::Q(t) => mu - beta * (-t.ln(true)).ln(),
    }
}

fn cauchy(x: f64, a: &[f64], op: Op) -> f64 {
    let (loc, s) = (a[0], a[1]);
    match op {
        _ if s < 0.0 => NAN,
        // A scale of 0 is allowed only by the quantile (it is the location).
        Op::D(_) | Op::P { .. } if s == 0.0 => NAN,
        Op::D(log) => {
            let y = (x - loc) / s;
            let d = PI * s * (1.0 + y * y);
            if log {
                -d.ln()
            } else {
                1.0 / d
            }
        }
        Op::P { lower, log } => {
            let z = (x - loc) / s;
            let z = if lower { z } else { -z };
            if z.abs() > 1.0 {
                let y = (1.0 / z).atan() / PI;
                match (z > 0.0, log) {
                    (true, true) => (-y).ln_1p(),
                    (true, false) => 1.0 - y,
                    (false, true) => (-y).ln(),
                    (false, false) => -y,
                }
            } else {
                let v = 0.5 + z.atan() / PI;
                if log {
                    v.ln()
                } else {
                    v
                }
            }
        }
        Op::Q(t) if !t.valid() => NAN,
        Op::Q(t) => {
            let below = t.prob(true);
            if below <= 0.5 {
                loc - s * cot_pi(below)
            } else {
                loc + s * cot_pi(t.prob(false))
            }
        }
    }
}

/// The gamma distribution of shape k ≥ 0 and scale s > 0 (k = 0 is a point
/// mass at 0, as R has it).
fn gamma(x: f64, a: &[f64], op: Op) -> f64 {
    let (k, s) = (a[0], a[1]);
    if k < 0.0 || s <= 0.0 {
        return NAN;
    }
    match op {
        Op::D(log) if x < 0.0 => d0(log),
        Op::D(log) if k == 0.0 => {
            if x == 0.0 {
                INF
            } else {
                d0(log)
            }
        }
        Op::D(log) if x == 0.0 => {
            if k < 1.0 {
                INF
            } else if k > 1.0 {
                d0(log)
            } else {
                dens(log, -s.ln())
            }
        }
        Op::D(log) => {
            let r = x / s;
            dens(log, (k - 1.0) * r.ln() - r - s.ln() - st::ln_gamma(k))
        }
        Op::P { lower, log } if x <= 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } if k == 0.0 => tails(lower, log, 1.0, 0.0),
        Op::P { lower, log } => tails(lower, log, st::gamma_p(k, x / s), st::gamma_q(k, x / s)),
        Op::Q(_) if k == 0.0 => NAN,
        Op::Q(t) => q_ends(t, 0.0, INF).unwrap_or_else(|| {
            split(t).map_or(NAN, |(below, above)| s * st::gamma_inv(k, below, above))
        }),
    }
}

/// Chi-square of df ≥ 0 degrees of freedom: gamma(df/2, 2).
fn chisq(x: f64, a: &[f64], op: Op) -> f64 {
    gamma(x, &[a[0] / 2.0, 2.0], op)
}

/// The beta distribution of shapes p, q > 0. (R also allows a shape of 0, a
/// point mass; Gnumeric extrapolates its CDF there. Both are `#NUM!` here.)
fn beta(x: f64, a: &[f64], op: Op) -> f64 {
    let (p, q) = (a[0], a[1]);
    if !(p > 0.0 && q > 0.0) {
        return NAN;
    }
    match op {
        Op::D(log) if !(0.0..=1.0).contains(&x) => d0(log),
        Op::D(log) if x == 0.0 || x == 1.0 => {
            // At an end the density is 0, infinite, or (a shape of 1)
            // 1/B(1, other) = other.
            let (own, other) = if x == 0.0 { (p, q) } else { (q, p) };
            if own > 1.0 {
                d0(log)
            } else if own < 1.0 {
                INF
            } else {
                dens(log, other.ln())
            }
        }
        Op::D(log) => dens(
            log,
            (p - 1.0) * x.ln() + (q - 1.0) * (-x).ln_1p() - st::ln_beta(p, q),
        ),
        Op::P { lower, log } if x <= 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } if x >= 1.0 => tails(lower, log, 1.0, 0.0),
        Op::P { lower, log } => tails(
            lower,
            log,
            st::beta_reg(p, q, x),
            st::beta_reg(q, p, 1.0 - x),
        ),
        Op::Q(t) => q_ends(t, 0.0, 1.0).unwrap_or_else(|| {
            split(t).map_or(NAN, |(below, above)| {
                if below <= 0.5 {
                    st::solve(|x| st::beta_reg(p, q, x), below, 0.0, 1.0)
                } else {
                    // 1 − I_x(p, q) = I_{1−x}(q, p): solve for y = 1 − x.
                    1.0 - st::solve(|y| st::beta_reg(q, p, y), above, 0.0, 1.0)
                }
            })
        }),
    }
}

/// The F distribution of m, n > 0 degrees of freedom.
fn f(x: f64, a: &[f64], op: Op) -> f64 {
    let (m, n) = (a[0], a[1]);
    if !(m > 0.0 && n > 0.0) {
        return NAN;
    }
    match op {
        Op::D(log) if x < 0.0 => d0(log),
        // At 0 the density is 0, 1 or infinite as m is over, at or under 2.
        Op::D(log) if x == 0.0 => {
            if m > 2.0 {
                d0(log)
            } else if m == 2.0 {
                dens(log, 0.0)
            } else {
                INF
            }
        }
        Op::D(log) => {
            let (h1, h2) = (m / 2.0, n / 2.0);
            dens(
                log,
                h1 * (m / n).ln() + (h1 - 1.0) * x.ln()
                    - (h1 + h2) * (m * x / n).ln_1p()
                    - st::ln_beta(h1, h2),
            )
        }
        Op::P { lower, log } if x <= 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } => tails(lower, log, st::f_cdf(x, m, n), st::f_sf(x, m, n)),
        Op::Q(t) => q_ends(t, 0.0, INF).unwrap_or_else(|| {
            split(t).map_or(NAN, |(below, above)| st::f_inv(m, n, below, above))
        }),
    }
}

/// Student's t of n > 0 degrees of freedom.
fn t(x: f64, a: &[f64], op: Op) -> f64 {
    let n = a[0];
    if n.is_nan() || n <= 0.0 {
        return NAN;
    }
    match op {
        Op::D(log) => dens(
            log,
            -(n + 1.0) / 2.0 * (x * x / n).ln_1p() - 0.5 * n.ln() - st::ln_beta(0.5, n / 2.0),
        ),
        Op::P { lower, log } => tails(lower, log, st::t_sf(-x, n), st::t_sf(x, n)),
        Op::Q(t) => q_ends(t, NEG_INF, INF).unwrap_or_else(|| {
            split(t).map_or(NAN, |(below, above)| {
                // Solve the smaller tail; + 0 turns the median's −0 into 0.
                if below <= 0.5 {
                    -st::t_upper_inv(below, n) + 0.0
                } else {
                    st::t_upper_inv(above, n)
                }
            })
        }),
    }
}

// --- skew distributions ----------------------------------------------------

/// The z where an increasing g on the real line reaches `target`: grows a
/// bracket from [−1, 1], then bisects to the last bit.
fn root(g: impl Fn(f64) -> f64, target: f64) -> f64 {
    let (mut lo, mut hi) = (-1.0, 1.0);
    while g(lo) >= target {
        (hi, lo) = (lo, 2.0 * lo);
        if lo.is_infinite() {
            return NAN;
        }
    }
    while g(hi) < target {
        (lo, hi) = (hi, 2.0 * hi);
        if hi.is_infinite() {
            return NAN;
        }
    }
    st::solve(g, target, lo, hi)
}

/// The quantile of a continuous distribution on the real line from its
/// lower-tail CDF: solves the smaller tail (the upper tail of shape s is the
/// lower tail of −x under −s, so `cdf(−z, true)` is P(Z > z)).
fn invert(t: Tail, cdf: impl Fn(f64, bool) -> f64) -> f64 {
    q_ends(t, NEG_INF, INF).unwrap_or_else(|| {
        split(t).map_or(NAN, |(below, above)| {
            if below <= 0.5 {
                root(|z| cdf(z, true), below)
            } else {
                root(|z| -cdf(z, false), -above)
            }
        })
    })
}

/// The skew-normal distribution of shape α, location ξ and scale ω > 0:
/// density 2/ω φ(z) Φ(αz), z = (x − ξ)/ω.
fn snorm(x: f64, a: &[f64], op: Op) -> f64 {
    let (shape, loc, scale) = (a[0], a[1], a[2]);
    if scale.is_nan() || scale <= 0.0 || shape.is_nan() || loc.is_nan() {
        return NAN;
    }
    let z = (x - loc) / scale;
    // The lower tail, or (upper) the upper one: P(Z > z | α) = P(Z ≤ −z | −α).
    let cdf = |z: f64, lower: bool| {
        if lower {
            sn_cdf(z, shape)
        } else {
            sn_cdf(-z, -shape)
        }
    };
    match op {
        Op::D(log) => dens(
            log,
            std::f64::consts::LN_2 - LN_SQRT_2PI - 0.5 * z * z - scale.ln()
                + norm_p(shape * z, true),
        ),
        Op::P { lower, log } => tails(lower, log, cdf(z, true), cdf(z, false)),
        Op::Q(t) => loc + scale * invert(t, cdf),
    }
}

/// The standard skew-normal P(Z ≤ h) of shape α: Φ(h) − 2T(h, α), or, in the
/// far left tail (α > 0, α|h| ≥ 1) where that difference cancels, the tail
/// integral (1/π) ∫_α^∞ e^(−h²(1+x²)/2) / (1 + x²) dx.
// ponytail: h ≈ 0 with α huge still cancels (about log10 α digits); the tail
// integral with geometric pieces would cover it.
fn sn_cdf(h: f64, alpha: f64) -> f64 {
    if alpha == 0.0 {
        return st::norm_cdf(h);
    }
    let g = -h;
    if h < 0.0 && alpha > 0.0 && alpha * g >= 1.0 {
        // With x = t/g, t = t0 + s: the integrand falls like e^(−t0 s), so
        // pieces 1/t0 wide reach e^(−40) at s = √(t0² + 80) − t0, written
        // 80 / (√(t0² + 80) + t0) so a huge t0 neither cancels nor overflows
        // (end · t0 < 40: at most 40 pieces).
        let t0 = alpha * g;
        let end = 80.0 / (t0.hypot(80f64.sqrt()) + t0);
        let pieces = (end * t0).ceil();
        let w = end / pieces;
        let f = |s: f64| (-t0 * s - 0.5 * s * s).exp() / (g * g + (t0 + s) * (t0 + s));
        let mut sum = 0.0;
        let mut i = 0.0;
        while i < pieces {
            sum += gauss_legendre(&f, i * w, (i + 1.0) * w);
            i += 1.0;
        }
        return g * (-0.5 * g * g * (1.0 + alpha * alpha)).exp() / PI * sum;
    }
    (st::norm_cdf(h) - 2.0 * owen_t(h, alpha)).clamp(0.0, 1.0)
}

/// The skew-t distribution of n > 0 degrees of freedom and shape λ (Azzalini
/// and Capitanio): density 2 t_n(x) T_{n+1}(λx √((n+1)/(x²+n))). Its CDF
/// needs a whole n (the recurrence of Jamalizadeh, Khosravi and
/// Balakrishnan, 2009, as Gnumeric uses it).
fn skew_t(x: f64, a: &[f64], op: Op) -> f64 {
    let (n, shape) = (a[0], a[1]);
    if n.is_nan() || n <= 0.0 || shape.is_nan() {
        return NAN;
    }
    if shape == 0.0 {
        return t(x, &a[..1], op);
    }
    let cdf = |z: f64, lower: bool| {
        if lower {
            st_cdf(z, n, shape)
        } else {
            st_cdf(-z, n, -shape)
        }
    };
    match op {
        Op::D(log) => {
            let lower_log = Op::P {
                lower: true,
                log: true,
            };
            let u = shape * x * ((n + 1.0) / (x * x + n)).sqrt();
            dens(
                log,
                std::f64::consts::LN_2 + t(x, &[n], Op::D(true)) + t(u, &[n + 1.0], lower_log),
            )
        }
        _ if nonint(n) => NAN,
        Op::P { lower, log } => tails(lower, log, cdf(x, true), cdf(x, false)),
        Op::Q(t) => invert(t, cdf),
    }
}

/// The skew-t P(X ≤ x) for whole n: each step of the recurrence takes n down
/// by 2, to the closed forms at n = 1 and 2.
// ponytail: n/2 steps, so past n = 1e4 it falls back on the skew-normal
// limit (error O(1/n)); Gnumeric does so past n = 100.
fn st_cdf(x: f64, n: f64, shape: f64) -> f64 {
    if n > 1e4 {
        return sn_cdf(x, shape);
    }
    let (mut x, mut n, mut p) = (x, n.round(), 0.0);
    while n > 2.0 {
        let v = n - 1.0;
        // ln[Γ(v/2) / (√π Γ((v+1)/2))] + ((v−1)/2) ln(v+1) − (v/2) ln(v+1+x²).
        let lb = st::ln_beta(v / 2.0, 0.5)
            - PI.ln()
            - 0.5 * (v + 1.0).ln()
            - v / 2.0 * (x * x / (v + 1.0)).ln_1p();
        let a = v + 1.0 + x * x;
        p += x * lb.exp() * st::t_sf(-(v.sqrt() * shape * x / a.sqrt()), v);
        n -= 2.0;
        x *= ((v - 1.0) / (v + 1.0)).sqrt();
    }
    p += if n == 1.0 {
        (x.atan() + (shape / ((1.0 + shape * shape) * (1.0 + x * x)).sqrt()).acos()) / PI
    } else {
        let f = x / (2.0 + x * x).sqrt();
        -(atan_less_half_pi(shape) + f * atan_less_half_pi(-shape * f)) / PI
    };
    p.clamp(0.0, 1.0)
}

/// atan(x) − π/2, without cancellation for large x.
fn atan_less_half_pi(x: f64) -> f64 {
    if x > 0.0 {
        (-1.0 / x).atan()
    } else {
        x.atan() - std::f64::consts::FRAC_PI_2
    }
}

// --- the studentized range ----------------------------------------------------

/// `R.PTUKEY(x, nmeans, df, [nranges], …)` and `R.QTUKEY(p, …)`: the largest
/// of `nranges` studentized ranges of `nmeans` means with `df` degrees of
/// freedom (Copenhaver and Holland's double integral, as R computes it).
fn tukey(id: u32, a: &[CellValue]) -> R {
    let (v, k, df) = (n(a, 0)?, n(a, 1)?, n(a, 2)?);
    let r = if a.len() > 3 { n(a, 3)? } else { 1.0 };
    let flag = |i: usize, default: bool| if a.len() > i { b(a, i) } else { Ok(default) };
    let (lower, log) = (flag(4, true)?, flag(5, false)?);
    if id == 1657 && v <= 0.0 {
        // Below the support, before the parameters are checked (R's order).
        return ok_num(tails(lower, log, 0.0, 1.0));
    }
    domain(k >= 2.0 && df >= 2.0 && r >= 1.0)?;
    let p = |q: f64| tukey_p(q, k, df, r);
    ok_num(if id == 1657 {
        let below = p(v);
        tails(lower, log, below, 1.0 - below)
    } else {
        let t = Tail { p: v, lower, log };
        q_ends(t, 0.0, INF)
            .unwrap_or_else(|| split(t).map_or(NAN, |(below, _)| st::solve(p, below, 0.0, 1.0)))
    })
}

/// P(Q ≤ q): ∫₀^∞ f(s) W(qs)^r ds, s the ratio of the sample standard
/// deviation to σ (a scaled chi of df degrees of freedom, density f) and W
/// the CDF of the range of k standard normals. Past 25000 degrees of
/// freedom s is taken as 1 (R's rule).
fn tukey_p(q: f64, k: f64, df: f64, r: f64) -> f64 {
    if q <= 0.0 {
        return 0.0;
    }
    if q.is_infinite() {
        return 1.0;
    }
    if df > 25000.0 {
        return range_cdf(q, k).powf(r);
    }
    // ln f(s) = ln c + (df − 1) ln s − (df/2) s², c = 2 (df/2)^(df/2) / Γ(df/2).
    let h = df / 2.0;
    let ln_c = std::f64::consts::LN_2 + h * h.ln() - st::ln_gamma(h);
    let ln_f = |s: f64| ln_c + (df - 1.0) * s.ln() - h * s * s;
    // (Gauss–Legendre nodes are interior to each piece, and every piece
    // lies in [0, ∞): s > 0 here.)
    let f = |s: f64| ln_f(s).exp() * range_cdf(q * s, k).powf(r);
    // Outward from the density's mode in pieces no wider than its spread.
    // To the right W(qs) rises and f falls, so the terms rise and then fall
    // away; to the left both fall, and the pieces halve toward 0, where W
    // makes its rise (on the scale 1/q). A piece adding nothing ends a side.
    let mode = ((df - 1.0) / df).sqrt();
    let w = 0.5f64.min(1.0 / (2.0 * df).sqrt());
    let mut sum = 0.0;
    let mut s = mode;
    loop {
        let piece = gauss_legendre(&f, s, s + w);
        sum += piece;
        s += w;
        if (piece <= sum * f64::EPSILON / 2.0 && sum > 0.0) || ln_f(s) < -745.0 {
            break;
        }
    }
    let mut s = mode;
    while s > f64::MIN_POSITIVE {
        let lo = s - w.min(s / 2.0);
        let piece = gauss_legendre(&f, lo, s);
        sum += piece;
        s = lo;
        if piece <= sum * f64::EPSILON / 2.0 {
            break;
        }
    }
    sum.min(1.0)
}

/// P(range of k standard normals ≤ w), in Hartley's form:
/// (2Φ(w/2) − 1)^k + 2k ∫_{w/2}^∞ φ(v) (Φ(v) − Φ(v − w))^(k−1) dv. The
/// integrand falls monotonically, so a piece adding nothing ends it.
fn range_cdf(w: f64, k: f64) -> f64 {
    if w <= 0.0 {
        return 0.0;
    }
    let half = w / 2.0;
    let first = if half > 1.0 {
        (k * (-2.0 * st::norm_cdf(-half)).ln_1p()).exp()
    } else {
        libm::erf(half * FRAC_1_SQRT_2).powf(k)
    };
    if first >= 1.0 {
        return 1.0;
    }
    let f = |v: f64| st::norm_pdf(v) * norm_range(v - w, v).powf(k - 1.0);
    let width = (3.0 / k.ln_1p()).min(1.0);
    let mut total = first;
    let mut v = half;
    while v < half + 40.0 {
        let piece = 2.0 * k * gauss_legendre(&f, v, v + width);
        total += piece;
        if total >= 1.0 {
            return 1.0;
        }
        if piece <= total * f64::EPSILON / 2.0 {
            break;
        }
        v += width;
    }
    total
}

// --- discrete families ------------------------------------------------------

/// Not an integer, to R's tolerance (1e-7 relative).
fn nonint(x: f64) -> bool {
    (x - x.round()).abs() > 1e-7 * x.abs().max(1.0)
}

/// A mass function's value at x (`mass` of the rounded x), or 0 off the
/// integers and below `lo`.
// ponytail: give_log is the log of the mass, so it is −∞ where the mass
// underflows (R keeps the log finite there); a log-space mass would fix it.
fn mass(log: bool, x: f64, lo: f64, mass: impl Fn(f64) -> f64) -> f64 {
    if x < lo || nonint(x) {
        d0(log)
    } else {
        dens(log, mass(x.round()).ln())
    }
}

/// The smallest integer k ∈ [lo, hi] with P(X ≤ k) ≥ p, where `cdf(k,
/// lower)` gives P(X ≤ k) (or P(X > k)). An upper-tail p is matched on the
/// upper tail, so it keeps its digits; a fuzz of 64 ε holds a p that rounds
/// onto a step to that step (R's rule).
fn disc_q(t: Tail, lo: f64, hi: f64, cdf: impl Fn(f64, bool) -> f64) -> f64 {
    if let Some(v) = q_ends(t, lo, hi) {
        return v;
    }
    let p = t.prob(t.lower);
    let fuzz = 64.0 * f64::EPSILON;
    let ok = |k: f64| {
        if t.lower {
            cdf(k, true) >= p * (1.0 - fuzz)
        } else {
            cdf(k, false) <= p * (1.0 + fuzz)
        }
    };
    if ok(lo) {
        return lo;
    }
    // Invariant: !ok(a) and ok(b) (or b = hi, the top of the support).
    let (mut a, mut b) = (lo, hi);
    if b.is_infinite() {
        b = lo + 1.0;
        while !ok(b) {
            a = b;
            b = lo + 2.0 * (b - lo);
            if b > 1e15 {
                return NAN;
            }
        }
    }
    st::int_search(a, b, ok)
}

/// The binomial distribution of n trials of success probability p.
fn binom(x: f64, a: &[f64], op: Op) -> f64 {
    let (n, p) = (a[0], a[1]);
    if !(0.0..=1.0).contains(&p) || n < 0.0 || nonint(n) {
        return NAN;
    }
    let n = n.round();
    let cdf = |k: f64, lower: bool| {
        // k < n here: the CDF answers k ≥ n itself, and the quantile search
        // never asks above its top, n.
        if lower {
            st::binom_range(n, p, 0.0, k)
        } else {
            st::binom_range(n, p, k + 1.0, n)
        }
    };
    match op {
        Op::D(log) if x > n => d0(log),
        Op::D(log) => mass(log, x, 0.0, |k| st::binom_range(n, p, k, k)),
        Op::P { lower, log } if x < 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } if x >= n => tails(lower, log, 1.0, 0.0),
        Op::P { lower, log } => {
            let k = fake_floor(x);
            tails(lower, log, cdf(k, true), cdf(k, false))
        }
        Op::Q(t) => disc_q(t, 0.0, n, cdf),
    }
}

/// The geometric distribution: failures before the first success, of
/// success probability p ∈ (0, 1].
fn geom(x: f64, a: &[f64], op: Op) -> f64 {
    let p = a[0];
    if !(p > 0.0 && p <= 1.0) {
        return NAN;
    }
    match op {
        Op::D(log) => mass(log, x, 0.0, |k| geom_pmf(k, p)),
        Op::P { lower, log } if x < 0.0 => tails(lower, log, 0.0, 1.0),
        // ln P(X > k) = (k + 1) ln(1 − p).
        Op::P { lower, log } => from_ln((fake_floor(x) + 1.0) * (-p).ln_1p(), false, lower, log),
        Op::Q(t) if !t.valid() => NAN,
        Op::Q(_) if p == 1.0 => 0.0,
        Op::Q(t) => q_ends(t, 0.0, INF).unwrap_or_else(|| {
            // The fuzz keeps a quantile that lands on an integer there.
            (t.ln(false) / (-p).ln_1p() - 1.0 - 1e-12).ceil().max(0.0)
        }),
    }
}

/// The negative binomial distribution: failures before the n-th success, of
/// success probability p ∈ (0, 1] (n ≥ 0 need not be an integer).
fn nbinom(x: f64, a: &[f64], op: Op) -> f64 {
    let (n, p) = (a[0], a[1]);
    if !(p > 0.0 && p <= 1.0) || n < 0.0 {
        return NAN;
    }
    // n = 0 is a point mass at 0.
    let cdf = |k: f64, lower: bool| match (n == 0.0, lower) {
        (true, _) => f64::from(u8::from(lower)),
        (false, true) => st::beta_reg(n, k + 1.0, p),
        (false, false) => st::beta_reg(k + 1.0, n, 1.0 - p),
    };
    match op {
        Op::D(log) => mass(log, x, 0.0, |k| {
            if n == 0.0 {
                f64::from(u8::from(k == 0.0))
            } else {
                st::negbinom_pmf(k, n, p)
            }
        }),
        Op::P { lower, log } if x < 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } => {
            let k = fake_floor(x);
            tails(lower, log, cdf(k, true), cdf(k, false))
        }
        // Gnumeric returns 0 for a point mass before it checks p.
        Op::Q(_) if p == 1.0 || n == 0.0 => 0.0,
        Op::Q(t) => disc_q(t, 0.0, INF, cdf),
    }
}

/// The Poisson distribution of mean λ ≥ 0.
fn pois(x: f64, a: &[f64], op: Op) -> f64 {
    let lambda = a[0];
    if lambda.is_nan() || lambda < 0.0 {
        return NAN;
    }
    let cdf = |k: f64, lower: bool| match (lambda == 0.0, lower) {
        (true, _) => f64::from(u8::from(lower)),
        (false, true) => st::gamma_q(k + 1.0, lambda),
        (false, false) => st::gamma_p(k + 1.0, lambda),
    };
    match op {
        Op::D(log) => mass(log, x, 0.0, |k| st::poisson(k, lambda, false)),
        Op::P { lower, log } if x < 0.0 => tails(lower, log, 0.0, 1.0),
        Op::P { lower, log } => {
            let k = fake_floor(x);
            tails(lower, log, cdf(k, true), cdf(k, false))
        }
        Op::Q(t) => disc_q(t, 0.0, INF, cdf),
    }
}

/// The hypergeometric distribution: red balls among n drawn from r red and
/// b black. The density wants whole parameters; the CDF and quantile round
/// them (Gnumeric's rules).
fn hyper(x: f64, a: &[f64], op: Op) -> f64 {
    let (r, b, n) = (a[0], a[1], a[2]);
    if matches!(op, Op::D(_)) && (nonint(r) || nonint(b) || nonint(n)) {
        return NAN;
    }
    let (r, b, n) = (r.round(), b.round(), n.round());
    if r < 0.0 || b < 0.0 || n < 0.0 || n > r + b || (r + b).is_infinite() {
        return NAN;
    }
    let (lo, hi) = ((n - b).max(0.0), n.min(r));
    // P(X ≤ k) for k red; P(X > k) is P(n − k − 1 or fewer black).
    let le = |k: f64, red: f64, black: f64| {
        let (lo, hi) = ((n - black).max(0.0), n.min(red));
        if k < lo {
            0.0
        } else if k >= hi {
            1.0
        } else {
            st::hypgeom_cdf(k, n, red, red + black)
        }
    };
    let cdf = |k: f64, lower: bool| {
        if lower {
            le(k, r, b)
        } else {
            le(n - k - 1.0, b, r)
        }
    };
    match op {
        Op::D(log) if x > hi => d0(log),
        Op::D(log) => mass(log, x, lo, |k| st::hypgeom_pmf(k, n, r, r + b)),
        Op::P { lower, log } => {
            let k = fake_floor(x);
            tails(lower, log, cdf(k, true), cdf(k, false))
        }
        Op::Q(t) => disc_q(t, lo, hi, cdf),
    }
}

/// cot(πp) for p ∈ [0, ½], exactly 0 at ½ (tan(π(½ − p)) where ½ − p is
/// exact).
fn cot_pi(p: f64) -> f64 {
    if p > 0.25 {
        (PI * (0.5 - p)).tan()
    } else {
        1.0 / (PI * p).tan()
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
    if (q1 - q2) * 32.0 > q1 + q2 {
        return q1 - q2;
    }
    // The tails nearly cancel, so the interval is narrow (the density falls
    // by at most e^(-1/32) across it): Gauss–Legendre integrates it exactly.
    gauss_legendre(&st::norm_pdf, x1, x2)
}

/// The Landau density, by Kölbig and Schorr's rational approximations
/// (CERNLIB's DENLAN, "A program package for the Landau distribution",
/// Comp. Phys. Comm. 31 (1984) 97), as Gnumeric has it: about 9 digits.
fn landau(v: f64) -> f64 {
    // A ratio of quartics in t, p and q low order first.
    let ratio = |p: [f64; 5], q: [f64; 5], t: f64| {
        let poly = |c: [f64; 5]| c[0] + (c[1] + (c[2] + (c[3] + c[4] * t) * t) * t) * t;
        poly(p) / poly(q)
    };
    if v < -5.5 {
        let u = (v + 1.0).exp();
        let a = [0.416_666_666_7e-1, -0.199_652_777_8e-1, 0.270_953_896_6e-1];
        0.398_942_280_3 * ((-1.0 / u).exp() / u.sqrt()) * (1.0 + (a[0] + (a[1] + a[2] * u) * u) * u)
    } else if v < -1.0 {
        let u = (-v - 1.0).exp();
        (-u).exp()
            * u.sqrt()
            * ratio(
                [
                    0.425_989_487_5,
                    -0.124_976_255,
                    0.398_424_37e-1,
                    -0.629_828_763_5e-2,
                    0.151_116_225_3e-2,
                ],
                [
                    1.0,
                    -0.338_826_062_9,
                    0.959_439_332_3e-1,
                    -0.160_804_228_3e-1,
                    0.377_894_206_3e-2,
                ],
                v,
            )
    } else if v < 1.0 {
        ratio(
            [
                0.178_854_160_9,
                0.117_395_740_3,
                0.148_885_051_8e-1,
                -0.139_498_941_1e-2,
                0.128_361_721_1e-3,
            ],
            [
                1.0,
                0.742_879_508_2,
                0.315_393_296_1,
                0.669_421_954_8e-1,
                0.879_060_971_4e-2,
            ],
            v,
        )
    } else if v < 5.0 {
        ratio(
            [
                0.178_854_450_3,
                0.935_916_166_2e-1,
                0.632_538_765_4e-2,
                0.661_166_731_9e-4,
                -0.203_104_910_1e-5,
            ],
            [
                1.0,
                0.609_780_992_1,
                0.256_061_666_5,
                0.474_672_238_4e-1,
                0.695_730_167_5e-2,
            ],
            v,
        )
    } else if v < 300.0 {
        let u = 1.0 / v;
        let (p, q) = if v < 12.0 {
            (
                [
                    0.987_405_440_7,
                    0.118_672_327_3e3,
                    0.849_279_436e3,
                    -0.743_779_244_4e3,
                    0.427_026_218_6e3,
                ],
                [
                    1.0,
                    0.106_861_596_1e3,
                    0.337_649_621_4e3,
                    0.201_671_238_9e4,
                    0.159_706_351_1e4,
                ],
            )
        } else if v < 50.0 {
            (
                [
                    0.100_367_507_4e1,
                    0.167_570_243_4e3,
                    0.478_971_128_9e4,
                    0.212_178_676_7e5,
                    -0.223_249_491e5,
                ],
                [
                    1.0,
                    0.156_942_453_7e3,
                    0.374_531_048_8e4,
                    0.983_469_887_6e4,
                    0.669_242_835_7e5,
                ],
            )
        } else {
            (
                [
                    0.100_082_761_9e1,
                    0.664_914_313_6e3,
                    0.629_729_266_5e5,
                    0.475_554_699_8e6,
                    -0.574_360_910_9e7,
                ],
                [
                    1.0,
                    0.651_410_109_8e3,
                    0.569_747_333_3e5,
                    0.165_917_472_5e6,
                    -0.281_575_993_9e7,
                ],
            )
        };
        u * u * ratio(p, q, u)
    } else {
        let u = 1.0 / (v - v * v.ln() / (v + 1.0));
        u * u * (1.0 + (-0.184_556_867e1 + -0.428_464_074_3e1 * u) * u)
    }
}

/// Owen's T function T(h, a) = (1/2π) ∫₀ᵃ exp(−h²(1 + x²)/2) / (1 + x²) dx:
/// even in h, odd in a; for |a| > 1 through Owen's identity
/// T(h, a) = ½Q(h)(1 − Q(ah)) + ½Q(ah)(1 − Q(h)) − T(ah, 1/a) (Q the normal
/// right tail), so the integral only ever runs over a ≤ 1.
fn owen_t(h: f64, a: f64) -> f64 {
    if h.is_nan() || a.is_nan() {
        return NAN;
    }
    let (h, aa) = (h.abs(), a.abs());
    let v = if aa == 0.0 {
        0.0
    } else if h == 0.0 {
        aa.atan() / (2.0 * PI)
    } else if aa <= 1.0 {
        owen_t_quad(h, aa)
    } else {
        let (q1, q2) = (st::norm_cdf(-h), st::norm_cdf(-aa * h));
        0.5 * q1 * (1.0 - q2) + 0.5 * q2 * (1.0 - q1) - owen_t_quad(aa * h, 1.0 / aa)
    };
    if a < 0.0 {
        -v
    } else {
        v
    }
}

/// T(h, a) for h > 0, 0 ≤ a ≤ 1: e^(−h²/2)/(2π) ∫₀ᵃ e^(−h²x²/2)/(1 + x²) dx
/// by Gauss–Legendre on pieces no wider than ½ or 1/h (each piece is then
/// smooth on its scale, so 16 points reach the last bit). Past x = 40/h the
/// integrand is below e^(−800).
fn owen_t_quad(h: f64, a: f64) -> f64 {
    let top = a.min(40.0 / h);
    if h.is_infinite() {
        return 0.0;
    }
    let pieces = (top * h.max(2.0)).ceil();
    let w = top / pieces;
    let f = |x: f64| (-0.5 * h * h * x * x).exp() / (1.0 + x * x);
    let mut sum = 0.0;
    let mut i = 0.0;
    while i < pieces {
        sum += gauss_legendre(&f, i * w, (i + 1.0) * w);
        i += 1.0;
    }
    (-0.5 * h * h).exp() / (2.0 * PI) * sum
}

/// ∫ f over [lo, hi] by 16-point Gauss–Legendre.
fn gauss_legendre(f: &impl Fn(f64) -> f64, lo: f64, hi: f64) -> f64 {
    let (c, r) = ((lo + hi) / 2.0, (hi - lo) / 2.0);
    r * GL16
        .iter()
        .map(|&(t, w)| w * (f(c - r * t) + f(c + r * t)))
        .sum::<f64>()
}

/// The standard bivariate normal P(X ≤ h, Y ≤ k) of correlation |ρ| < 1, by
/// Owen's formula ½Φ(h) + ½Φ(k) − T(h, a_h) − T(k, a_k) − β, a_h =
/// (k − ρh)/(h√(1 − ρ²)) and a_k likewise, β = ½ when h and k fall on
/// opposite sides of 0 (Gnumeric uses Drezner's 5-point rule, good to about
/// 1e-8).
fn biv_norm(h: f64, k: f64, rho: f64) -> f64 {
    if h == 0.0 && k == 0.0 {
        return 0.25 + rho.asin() / (2.0 * PI);
    }
    let s = (1.0 - rho * rho).sqrt();
    let beta = if h * k > 0.0 || (h * k == 0.0 && h + k >= 0.0) {
        0.0
    } else {
        0.5
    };
    let v = 0.5 * (st::norm_cdf(h) + st::norm_cdf(k))
        - owen_t(h, (k - rho * h) / (h * s))
        - owen_t(k, (h - rho * k) / (k * s))
        - beta;
    v.clamp(0.0, 1.0)
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
#[cfg_attr(coverage_nightly, coverage(off))]
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

    /// R.QBINOM and R.PBINOM with 1e8 and 1e16 trials (Gnumeric 1.12.57
    /// 3999999974309727; the mpmath sum 0.5000434313342542). They were
    /// 5.7 sd low and negative before beta_reg handled huge shapes.
    #[test]
    fn binomial_with_huge_n() {
        assert_eq!(val(1605, &[0.3, 1e16, 0.4]), 3_999_999_974_309_727.0);
        assert!((val(1604, &[4e7, 1e8, 0.4]) - 0.500_043_431_334_254_2).abs() < 1e-11);
    }

    /// The R-style kernels' and the extra distributions' branches in this
    /// module's own binary: (formula over literals, Gnumeric 1.12.57's
    /// result) through `eval`. Three rows differ from Gnumeric and say so:
    /// R's upper tail of plnorm, a zero beta shape, and the bivariate
    /// normal, which matches mpmath where Gnumeric's approximation is off
    /// by 1e-7.
    #[test]
    fn kernels_match_gnumeric() {
        let rows: &[(&str, Result<f64, u8>)] = &[
            ("R.DNORM(1,0,1,TRUE)", Ok(-1.4189385332046727)),
            ("R.DNORM(1,0,-1)", Err(err::NUM)),
            ("R.DNORM(1E+308,0,1E-308)", Ok(0.0)),
            ("R.PNORM(1,0,-1)", Err(err::NUM)),
            ("R.PNORM(-1,0,0)", Ok(0.0)),
            ("R.PNORM(1,0,0)", Ok(1.0)),
            ("R.PNORM(1,0,1,FALSE)", Ok(0.15865525393145705)),
            ("R.PNORM(-40,0,1,TRUE,TRUE)", Ok(-804.6084420137538)),
            ("R.PNORM(1,0,1,TRUE,TRUE)", Ok(-0.17275377902344988)),
            ("R.QNORM(2,0,1)", Err(err::NUM)),
            ("R.QNORM(0.3,0,-1)", Err(err::NUM)),
            ("R.QNORM(0.3,1,0)", Ok(1.0)),
            ("R.QNORM(0,0,1)", Err(err::NUM)),
            ("R.QNORM(1,0,1)", Err(err::NUM)),
            ("R.QNORM(-800,0,1,TRUE,TRUE)", Ok(-39.88469483825662)),
            ("R.QNORM(-800,0,1,FALSE,TRUE)", Ok(39.88469483825662)),
            ("R.QNORM(1E-310,0,1)", Ok(-37.66306033194952)),
            ("R.QNORM(0.7,0,1)", Ok(0.5244005127080407)),
            ("R.DLNORM(0,0,1)", Ok(0.0)),
            ("R.DLNORM(-1,0,1,TRUE)", Err(err::NUM)),
            ("R.DLNORM(2,0,1,TRUE)", Ok(-1.8523122207237188)),
            ("R.PLNORM(0,0,1)", Ok(0.0)),
            ("R.PLNORM(-1,0,1,FALSE)", Ok(1.0)), // R (Gnumeric's plnorm: 0)
            ("R.PLNORM(2,0,0)", Err(err::NUM)),
            ("R.QLNORM(0.3,0,1)", Ok(0.5919101006095542)),
            ("R.DEXP(-1,2)", Ok(0.0)),
            ("R.DEXP(1,2,TRUE)", Ok(-1.1931471805599454)),
            ("R.DEXP(1,0)", Err(err::NUM)),
            ("R.PEXP(-1,2)", Ok(0.0)),
            ("R.PEXP(1,-2)", Err(err::NUM)),
            ("R.PEXP(1,2,FALSE,TRUE)", Ok(-0.5)),
            ("R.QEXP(2,2)", Err(err::NUM)),
            ("R.QEXP(0,2)", Ok(0.0)),
            ("R.QEXP(0.3,2)", Ok(0.7133498878774647)),
            ("R.DWEIBULL(1,0,2)", Err(err::NUM)),
            ("R.DWEIBULL(-1,2,2)", Ok(0.0)),
            ("R.DWEIBULL(0,0.5,2)", Err(err::NUM)),
            ("R.DWEIBULL(1,2,2,TRUE)", Ok(-0.9431471805599453)),
            ("R.PWEIBULL(-1,2,2)", Ok(0.0)),
            ("R.PWEIBULL(1,2,2,FALSE,TRUE)", Ok(-0.25)),
            ("R.QWEIBULL(2,2,2)", Err(err::NUM)),
            ("R.QWEIBULL(0,2,2)", Ok(0.0)),
            ("R.QWEIBULL(0.3,2,2)", Ok(1.1944453841657765)),
            ("R.DRAYLEIGH(1,0)", Err(err::NUM)),
            ("R.DRAYLEIGH(0,1)", Ok(0.0)),
            ("R.DRAYLEIGH(1,1,TRUE)", Ok(-0.4999999999999999)),
            ("R.PRAYLEIGH(-1,1)", Ok(0.0)),
            ("R.PRAYLEIGH(1,1,FALSE)", Ok(0.6065306597126335)),
            ("R.QRAYLEIGH(2,1)", Err(err::NUM)),
            ("R.QRAYLEIGH(0,1)", Ok(0.0)),
            ("R.QRAYLEIGH(0.3,1)", Ok(0.8446004309005914)),
            ("R.DGUMBEL(1,0,0)", Err(err::NUM)),
            ("R.DGUMBEL(1,0,1,TRUE)", Ok(-1.3678794411714423)),
            ("R.PGUMBEL(1,0,1,FALSE)", Ok(0.30779937244465366)),
            ("R.QGUMBEL(2,0,1)", Err(err::NUM)),
            ("R.QGUMBEL(0.3,0,1)", Ok(-0.18562675886236574)),
            ("R.DCAUCHY(1,0,-1)", Err(err::NUM)),
            ("R.DCAUCHY(1,0,0)", Err(err::NUM)),
            ("R.DCAUCHY(1,0,1,TRUE)", Ok(-1.8378770664093453)),
            ("R.PCAUCHY(1,0,1,FALSE)", Ok(0.25)),
            ("R.PCAUCHY(3,0,1,TRUE,TRUE)", Ok(-0.10804899566097342)),
            ("R.PCAUCHY(-3,0,1,TRUE,TRUE)", Ok(-2.278708595290299)),
            ("R.PCAUCHY(0.5,0,1,TRUE,TRUE)", Ok(-0.4345073545177282)),
            ("R.QCAUCHY(2,0,1)", Err(err::NUM)),
            ("R.QCAUCHY(0.3,0,1)", Ok(-0.7265425280053609)),
            ("R.QCAUCHY(0.7,0,1)", Ok(0.7265425280053608)),
            ("R.QCAUCHY(0.3,1,0)", Ok(1.0)),
            ("R.DGAMMA(1,-1,2)", Err(err::NUM)),
            ("R.DGAMMA(-1,2,2)", Ok(0.0)),
            ("R.DGAMMA(0,0,2)", Err(err::NUM)),
            ("R.DGAMMA(1,0,2)", Ok(0.0)),
            ("R.DGAMMA(0,0.5,2)", Err(err::NUM)),
            ("R.DGAMMA(0,2,2)", Ok(0.0)),
            ("R.DGAMMA(0,1,2)", Ok(0.5)),
            ("R.DGAMMA(0,1,2,TRUE)", Ok(-std::f64::consts::LN_2)),
            ("R.PGAMMA(-1,2,2)", Ok(0.0)),
            ("R.PGAMMA(1,0,2)", Ok(1.0)),
            ("R.PGAMMA(1,2,2,FALSE)", Ok(0.9097959895689501)),
            ("R.QGAMMA(0.3,0,2)", Err(err::NUM)),
            ("R.QGAMMA(0.3,2,2)", Ok(2.194698421406983)),
            ("R.QGAMMA(0,2,2)", Ok(0.0)),
            ("R.QGAMMA(1,2,2)", Err(err::NUM)),
            ("R.QGAMMA(2,2,2)", Err(err::NUM)),
            ("R.DCHISQ(1,3)", Ok(0.24197072451914337)),
            ("R.QCHISQ(0.3,3)", Ok(1.4236522430352796)),
            ("R.DBETA(0.5,0,2)", Err(err::NUM)), // a 0 shape is #NUM! here (Gnumeric: 0)
            ("R.DBETA(1.5,2,2)", Ok(0.0)),
            ("R.DBETA(0,2,2)", Ok(0.0)),
            ("R.DBETA(0,0.5,2)", Err(err::NUM)),
            ("R.DBETA(0,1,2)", Ok(2.0)),
            ("R.DBETA(1,2,1,TRUE)", Ok(std::f64::consts::LN_2)),
            ("R.DBETA(0.3,2,3,TRUE)", Ok(0.5675839575845996)),
            ("R.PBETA(-1,2,3)", Ok(0.0)),
            ("R.PBETA(2,2,3)", Ok(1.0)),
            ("R.PBETA(0.3,2,3,FALSE)", Ok(0.6517)),
            ("R.QBETA(0.3,2,3)", Ok(0.2723839420751053)),
            ("R.QBETA(0.8,2,3)", Ok(0.5824535745243333)),
            ("R.QBETA(0,2,3)", Ok(0.0)),
            ("R.QBETA(1,2,3)", Ok(1.0)),
            ("R.DF(1,0,3)", Err(err::NUM)),
            ("R.DF(-1,2,3)", Ok(0.0)),
            ("R.DF(0,3,3)", Ok(0.0)),
            ("R.DF(0,2,3)", Ok(1.0)),
            ("R.DF(0,1,3)", Err(err::NUM)),
            ("R.DF(0,2,3,TRUE)", Ok(0.0)),
            ("R.DF(1,2,3,TRUE)", Ok(-1.2770640594149767)),
            ("R.PF(-1,2,3)", Ok(0.0)),
            ("R.PF(1,2,3,FALSE)", Ok(0.46475800154489)),
            ("R.QF(0.3,2,3)", Ok(0.4026514323055729)),
            ("R.QF(0,2,3)", Ok(0.0)),
            ("R.QF(1,2,3)", Err(err::NUM)),
            ("R.DT(1,0)", Err(err::NUM)),
            ("R.DT(1,3,TRUE)", Ok(-1.5762529945270716)),
            ("R.PT(1,3,FALSE)", Ok(0.19550110947788535)),
            ("R.QT(0.3,3)", Ok(-0.5843897274398187)),
            ("R.QT(0.7,3)", Ok(0.5843897274398185)),
            ("R.QT(0,3)", Err(err::NUM)),
            ("R.QT(0.5,3)", Ok(0.0)),
            ("R.DSNORM(1,2,0,-1)", Err(err::NUM)),
            ("R.DSNORM(1,2,0,1,TRUE)", Ok(-0.7488042619736909)),
            ("R.PSNORM(1,2,0,1,FALSE)", Ok(0.31559162791762524)),
            ("R.QSNORM(0.3,2,0,1)", Ok(0.313129446355111)),
            ("R.QSNORM(0.8,2,0,1)", Ok(1.2808674600387895)),
            ("R.DST(1,0,2)", Err(err::NUM)),
            ("R.DST(1,3,2,TRUE)", Ok(-0.9429176692860615)),
            ("R.PST(1,3,2,FALSE)", Ok(0.38008611774816325)),
            ("R.PST(1,3.5,2)", Err(err::NUM)),
            ("R.PST(-1,1,2)", Ok(0.032047108424487485)),
            ("R.PST(1,5,2)", Ok(0.6435166060836603)),
            ("R.QST(0.3,3,2)", Ok(0.33499000050476596)),
            ("R.DBINOM(3,10,1.4)", Err(err::NUM)),
            ("R.DBINOM(3,-10,0.4)", Err(err::NUM)),
            ("R.DBINOM(3,10.5,0.4)", Err(err::NUM)),
            ("R.DBINOM(11,10,0.4)", Ok(0.0)),
            ("R.DBINOM(3,10,0.4,TRUE)", Ok(-1.537159819202354)),
            ("R.PBINOM(-1,10,0.4)", Ok(0.0)),
            ("R.PBINOM(10,10,0.4)", Ok(1.0)),
            ("R.PBINOM(12,10,0.4,FALSE)", Ok(0.0)),
            ("R.QBINOM(0.3,10,0.4)", Ok(3.0)),
            ("R.QBINOM(0,10,0.4)", Ok(0.0)),
            ("R.QBINOM(1,10,0.4)", Ok(10.0)),
            ("R.DGEOM(-1,0.4)", Ok(0.0)),
            ("R.DGEOM(2,0.4,TRUE)", Ok(-1.9379419794061366)),
            ("R.PGEOM(-1,0.4)", Ok(0.0)),
            ("R.PGEOM(2,0.4,FALSE)", Ok(0.21599999999999997)),
            ("R.QGEOM(0.3,0.4)", Ok(0.0)),
            ("R.QGEOM(0.3,1)", Ok(0.0)),
            ("R.DNBINOM(3,4,0)", Err(err::NUM)),
            ("R.DNBINOM(3,-4,0.4)", Err(err::NUM)),
            ("R.DNBINOM(3,4,1.4)", Err(err::NUM)),
            ("R.PNBINOM(-1,4,0.4)", Ok(0.0)),
            ("R.PNBINOM(3,4,0.4,FALSE)", Ok(0.710208)),
            ("R.QNBINOM(0.3,4,0.4)", Ok(4.0)),
            ("R.QNBINOM(0.3,4,1)", Ok(0.0)),
            ("R.QNBINOM(0.3,0,0.4)", Ok(0.0)),
            ("R.DPOIS(3,-1)", Err(err::NUM)),
            ("R.DPOIS(3,2.5,TRUE)", Ok(-1.5428872736055899)),
            ("R.PPOIS(-1,2.5)", Ok(0.0)),
            ("R.PPOIS(3,2.5,FALSE)", Ok(0.2424238668669341)),
            ("R.QPOIS(0.3,2.5)", Ok(2.0)),
            ("R.QPOIS(0.3,0)", Ok(0.0)),
            ("R.DHYPER(2,5,4,3.5)", Err(err::NUM)),
            ("R.DHYPER(2,5,4,3,TRUE)", Ok(-0.7419373447293776)),
            ("R.DHYPER(2,-5,4,3)", Err(err::NUM)),
            ("R.DHYPER(2,5,4,10)", Err(err::NUM)),
            ("R.DHYPER(4,5,4,3)", Ok(0.0)),
            ("R.PHYPER(4,5,4,3)", Ok(1.0)),
            ("R.PHYPER(-1,5,4,3)", Ok(0.0)),
            ("R.QHYPER(0.3,5,4,3)", Ok(1.0)),
            ("R.PTUKEY(3,3,10)", Ok(0.8650165848104364)),
            ("R.PTUKEY(0,3,10)", Ok(0.0)),
            ("R.PTUKEY(30,3,10)", Ok(0.9999999966875545)),
            ("R.PTUKEY(3,3,10,1,FALSE)", Ok(0.13498341518956358)),
            ("R.PTUKEY(3,3,10,1,TRUE,TRUE)", Ok(-0.14500659904280505)),
            ("R.PTUKEY(3,3,1E+20)", Ok(0.914457428345042)),
            ("R.QTUKEY(0.3,3,10)", Ok(1.1589854329626572)),
            ("R.QTUKEY(0.99,3,10)", Ok(5.27016153706883)),
            ("CUM_BIV_NORM_DIST(0,0,0.5)", Ok(0.33333333333333337)), // mpmath (Gnumeric 0.333333338344917)
            ("CUM_BIV_NORM_DIST(1,0,0.5)", Ok(0.4687429526451681)), // mpmath (Gnumeric 0.4687430517060412)
            ("CUM_BIV_NORM_DIST(0,-1,0.5)", Ok(0.12739820657662526)), // mpmath (Gnumeric 0.12739830563749827)
            ("CUM_BIV_NORM_DIST(-1,1,0.5)", Ok(0.15487295185860273)), // mpmath (Gnumeric 0.15487307859862898)
            ("OWENT(0.5,2)", Ok(0.1415806036539784)),
            ("OWENT(0.5,0)", Ok(0.0)),
            ("OWENT(0,2)", Ok(0.17620819117478337)),
            ("OWENT(3,5)", Ok(0.0006749490158150473)),
            ("OWENT(-0.5,-2)", Ok(-0.1415806036539784)),
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
            let got = call(def.id.0, &a).and_then(|v| match v {
                CellValue::Err(e) => Err(e),
                v => Ok(v.as_num().unwrap()),
            });
            let same = match (got, want) {
                (Ok(g), Ok(w)) => g == *w || (g - w).abs() <= 1e-12 * w.abs(),
                (g, w) => g == *w,
            };
            assert!(same, "{formula} = {got:?}, want {want:?}");
        }
    }

    /// The branches of the quantile search, the skew-t recurrence, Tukey's
    /// range integral and a few domain checks the table misses.
    #[test]
    fn search_and_integral_edges() {
        let near = |got: f64, want: f64| {
            assert!((got - want).abs() <= 1e-12 * want.abs(), "{got} vs {want}")
        };
        // root and disc_q are generic: one closure type is one
        // instantiation, so each is driven through a single closure whose
        // behaviour a Cell switches. root: a bracket grown downward to a
        // target, upward, and neither way (no crossing: NaN).
        let mode = std::cell::Cell::new(0u8);
        let g = |z: f64| match mode.get() {
            0 => z,
            1 => 1.0,
            _ => 0.0,
        };
        assert_eq!(root(g, -5.0), -5.0);
        assert_eq!(root(g, 5.0), 5.0);
        mode.set(1);
        assert!(root(g, 0.5).is_nan());
        mode.set(2);
        assert!(root(g, 0.5).is_nan());
        // disc_q: below the support's bottom, a bounded search, an
        // unbounded one that finds its step, and one that never does.
        let step = std::cell::Cell::new(7.0);
        let cdf = |k: f64, lower: bool| {
            let below = if k >= step.get() { 1.0 } else { 0.0 };
            if lower {
                below
            } else {
                1.0 - below
            }
        };
        let t = tail(0.5, true, false);
        // At the ends of the probability range: the support's ends.
        assert_eq!(disc_q(tail(0.0, true, false), 0.0, 10.0, cdf), 0.0);
        assert_eq!(disc_q(t, 0.0, 10.0, cdf), 7.0);
        assert_eq!(disc_q(t, 0.0, INF, cdf), 7.0);
        step.set(-1.0);
        assert_eq!(disc_q(t, 0.0, INF, cdf), 0.0);
        step.set(INF);
        assert!(disc_q(t, 0.0, INF, cdf).is_nan());
        // R.QSNORM in the far lower tail goes through the tail integral and
        // root's downward bracket: mpmath (60 digits, on the same tail
        // integral) -16.51323725063124 (Gnumeric -4.0028, a failed search);
        // and high in the upper tail with a large shape (mpmath
        // 5.3267238864, Gnumeric -7.4e184).
        assert!((val(1647, &[1e-300, 2.0, 0.0, 1.0]) + 16.513_237_250_631_24).abs() < 1e-12);
        assert!((val(1647, &[0.999_999_9, 50.0, 0.0, 1.0]) - 5.326_723_886_384_496).abs() < 1e-8);
        // R.PST for an even and an odd whole n (Gnumeric).
        near(val(1649, &[1.0, 2.0, 2.0]), 0.593_768_262_346_985_1);
        near(val(1649, &[1.0, 1.0, 2.0]), 0.532_047_108_424_487_4);
        // R.PTUKEY near 0 and near 1, with 2 and with 1.5 degrees of
        // freedom, with 100 groups; W(q) reaching 1 (Gnumeric).
        near(val(1657, &[0.0001, 3.0, 10.0]), 2.756_644_472_514_328e-9);
        near(val(1657, &[100.0, 3.0, 10.0]), 0.999_999_999_999_978_4);
        near(val(1657, &[3.0, 3.0, 2.0]), 0.711_650_017_234_975);
        near(val(1657, &[3.0, 100.0, 10.0]), 0.009_458_845_407_880_362);
        assert_eq!(range_cdf(80.0, 3.0), 1.0);
        assert!((tukey_p(3.0, 3.0, 1.5, 1.0) - 0.664_624_268_956_452_8).abs() < 1e-12);
        // Domain checks: a zero Cauchy scale outside the quantile, a gamma
        // scale of 0 or negative shape, a fractional hypergeometric size,
        // SNORM.DIST.RANGE with the ends reversed (Gnumeric).
        assert_eq!(
            call(1607, &nums(&[1.0, 0.0, 0.0])),
            Ok(CellValue::Err(err::NUM))
        );
        assert_eq!(
            call(1618, &nums(&[1.0, 2.0, 0.0])),
            Ok(CellValue::Err(err::NUM))
        );
        assert_eq!(
            call(1619, &nums(&[1.0, -1.0, 2.0])),
            Ok(CellValue::Err(err::NUM))
        );
        assert_eq!(
            call(1627, &nums(&[2.0, 5.5, 4.0, 3.0])),
            Ok(CellValue::Err(err::NUM))
        );
        assert_eq!(
            call(1627, &nums(&[2.0, 5.0, 4.5, 3.0])),
            Ok(CellValue::Err(err::NUM))
        );
        near(val(1560, &[2.0, 1.0]), -0.135_905_121_983_277_84);
        // R.PTUKEY of a q so small the integrand is 0 everywhere: the right
        // side runs until the density underflows, and with 1 degree of
        // freedom (mode 0) the left side never starts (Gnumeric 0).
        assert_eq!(tukey_p(1e-300, 3.0, 10.0, 1.0), 0.0);
        assert_eq!(tukey_p(1e-300, 3.0, 1.0, 1.0), 0.0);
        assert!(range_cdf(f64::NAN, 3.0).is_nan());
        assert!(geom(1.0, &[f64::NAN], Op::D(false)).is_nan());
        assert!(geom(1.0, &[1.5], Op::D(false)).is_nan());
        // The bivariate normal on h·k > 0, h·k = 0 with h + k ≥ 0 and < 0,
        // h·k < 0, and both negative (mpmath).
        near(val(1562, &[1.0, -1.0, 0.5]), 0.154_872_951_858_602_78);
        near(val(1562, &[0.0, 1.0, 0.5]), 0.468_742_952_645_168_1);
        near(val(1562, &[0.0, -1.0, 0.5]), 0.127_398_206_576_625_12);
        near(val(1562, &[1.0, 1.0, 0.3]), 0.728_147_340_652_689_8);
        near(val(1562, &[0.0, 0.5, 0.3]), 0.388_355_154_325_802_7);
        near(val(1562, &[-0.5, 0.0, 0.3]), 0.196_892_693_051_789_58);
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
        assert_eq!(r_style(1657, &nums(&[0.5, 2.0, 3.0])), Err(err::NAME));
    }

    #[test]
    fn flags_must_be_booleans() {
        let mut a = nums(&[1.0, 0.0, 1.0]);
        a.push(CellValue::Text("x".into()));
        assert_eq!(call(1636, &a), Err(err::VALUE));
        assert_eq!(call(1637, &a), Err(err::VALUE));
        a.insert(3, CellValue::Bool(true));
        assert_eq!(call(1638, &a), Err(err::VALUE));
    }

    fn tail(p: f64, lower: bool, log: bool) -> Tail {
        Tail { p, lower, log }
    }

    #[test]
    fn tails_convert_both_ways() {
        let t = tail(0.25f64.ln(), false, true);
        assert!(t.valid() && !tail(0.1, true, true).valid());
        assert!(!tail(-0.1, true, false).valid());
        assert!((t.prob(true) - 0.75).abs() < 1e-15);
        assert!((t.prob(false) - 0.25).abs() < 1e-15);
        assert!((t.ln(true) - 0.75f64.ln()).abs() < 1e-15);
        assert!(tail(NEG_INF, false, true).at_top());
        assert!(tail(0.0, false, true).at_bottom());
        assert!((ln_1m_exp(-10.0) - (-(-10f64).exp()).ln_1p()).abs() < 1e-18);
        assert!((ln_1m_exp(-0.1) - (-(-0.1f64).exp_m1()).ln()).abs() < 1e-15);
        for (ln, of_lower, lower, log) in [
            (-0.5, true, true, true),
            (-0.5, true, false, false),
            (-0.5, false, true, true),
        ] {
            let p = (-0.5f64).exp();
            let want = if lower == of_lower { p } else { 1.0 - p };
            let got = from_ln(ln, of_lower, lower, log);
            let got = if log { got.exp() } else { got };
            assert!((got - want).abs() < 1e-15, "{of_lower} {lower} {log}");
        }
    }

    #[test]
    fn normal_quantiles_reach_the_far_tails() {
        // The upper tail's p, and a lower tail that underflows (ln Φ root).
        let q = |p: f64, lower: bool, log: bool| norm_q(tail(p, lower, log));
        assert!((q(0.3, false, false) - 0.524_400_512_708_041).abs() < 1e-14);
        let z = q(-1000.0, false, true);
        assert!((z - 44.615_747_731_969_4).abs() < 1e-12, "{z}");
        assert!((norm_p(-z, true) + 1000.0).abs() < 1e-9);
        assert!((norm_p(1.0, true) - 0.841_344_746_068_542_9f64.ln()).abs() < 1e-15);
        let n = |x: f64, a: &[f64], op: Op| norm(x, a, op);
        assert_eq!(n(0.5, &[0.0, 1.0], Op::Q(tail(0.0, true, false))), NEG_INF);
        assert_eq!(n(1.0, &[1.0, 1.0], Op::Q(tail(1.0, true, false))), INF);
        assert!(n(0.5, &[0.0, -1.0], Op::Q(tail(0.5, true, false))).is_nan());
        assert_eq!(n(INF, &[0.0, 1.0], Op::D(true)), NEG_INF);
        assert!(n(0.0, &[0.0, -1.0], Op::D(false)).is_nan());
    }

    #[test]
    fn incomplete_function_edges() {
        let up = Op::P {
            lower: false,
            log: false,
        };
        let q = |p: f64| Op::Q(tail(p, true, false));
        // A shape of 0 is a point mass at 0.
        assert_eq!(gamma(0.0, &[0.0, 1.0], Op::D(false)), INF);
        assert_eq!(gamma(0.0, &[1.0, 2.0], Op::D(true)), -(2f64.ln()));
        assert_eq!(gamma(1.0, &[0.0, 1.0], up), 0.0);
        assert!(gamma(1.0, &[0.0, 1.0], q(0.5)).is_nan());
        // A tail that underflows cannot be inverted.
        assert!(gamma(1.0, &[2.0, 1.0], Op::Q(tail(-800.0, true, true))).is_nan());
        assert!(beta(0.5, &[2.0, 3.0], Op::D(false)).is_finite());
        assert_eq!(beta(0.0, &[0.5, 3.0], Op::D(false)), INF);
        assert_eq!(beta(1.0, &[2.0, 3.0], Op::D(true)), NEG_INF);
        assert_eq!(beta(-1.0, &[2.0, 3.0], up), 1.0);
        assert!((beta(0.3, &[2.0, 3.0], q(0.2)) - 0.0).abs() > 0.0);
        assert!(beta(0.0, &[2.0, 3.0], Op::Q(tail(-800.0, true, true))).is_nan());
        assert_eq!(f(-1.0, &[3.0, 7.0], Op::D(false)), 0.0);
        assert_eq!(f(0.0, &[3.0, 7.0], Op::D(true)), NEG_INF);
        assert!(f(1.0, &[0.0, 7.0], Op::D(false)).is_nan());
        assert!(f(1.0, &[3.0, 7.0], Op::Q(tail(-800.0, true, true))).is_nan());
        assert!(t(1.0, &[0.0], Op::D(false)).is_nan());
        assert!(t(1.0, &[3.0], Op::Q(tail(-800.0, true, true))).is_nan());
        assert_eq!(t(0.0, &[3.0], q(0.5)).to_bits(), 0f64.to_bits());
    }

    #[test]
    fn skew_and_tukey_edges() {
        let lower = Op::P {
            lower: true,
            log: false,
        };
        // A shape of 0 is the symmetric distribution.
        assert_eq!(sn_cdf(0.7, 0.0), st::norm_cdf(0.7));
        assert_eq!(skew_t(0.7, &[3.0, 0.0], lower), t(0.7, &[3.0], lower));
        assert!(snorm(1.0, &[1.0, 0.0, 0.0], lower).is_nan());
        assert!(skew_t(1.0, &[f64::NAN, 1.0], lower).is_nan());
        // Large n falls back on the skew-normal limit.
        assert_eq!(st_cdf(1.0, 2e4, 2.0), sn_cdf(1.0, 2.0));
        assert!(atan_less_half_pi(-1.0) < -2.3);
        // A bracket that runs off the reals.
        assert!(root(|_| 0.0, 1.0).is_nan());
        assert!(root(|_| 1.0, 0.0).is_nan());
        assert!(snorm(0.0, &[1.0, 0.0, 1.0], Op::Q(tail(-800.0, true, true))).is_nan());
        // The range of k normals: certain past the far tail.
        assert_eq!(range_cdf(0.0, 3.0), 0.0);
        assert_eq!(range_cdf(80.0, 3.0), 1.0);
        assert_eq!(tukey_p(f64::INFINITY, 3.0, 10.0, 1.0), 1.0);
        assert_eq!(tukey_p(0.0, 3.0, 10.0, 1.0), 0.0);
        assert!((tukey_p(4.0, 4.0, 3e4, 1.0) - range_cdf(4.0, 4.0)).abs() < 1e-16);
        let mut a = nums(&[0.5, 4.0, 10.0, 1.0]);
        a.push(CellValue::Text("x".into()));
        assert_eq!(call(1657, &a), Err(err::VALUE));
        let deep = nums(&[-800.0, 4.0, 10.0, 1.0, 1.0, 1.0]);
        assert_eq!(call(1658, &deep), Ok(CellValue::Err(err::NUM)));
    }

    /// Each family rejects each bad parameter (the goldens hit one each).
    #[test]
    fn each_parameter_is_checked() {
        let d = Op::D(false);
        let nan = f64::NAN;
        for (family, params) in [
            (0, &[2.0, 0.0][..]),
            (5, &[3.0, -1.0]),
            (17, &[nan]),
            (15, &[nan, 0.0, 1.0]),
            (15, &[1.0, nan, 1.0]),
            (15, &[1.0, 0.0, nan]),
            (16, &[3.0, nan]),
            (1, &[-1.0, 0.5]),
            (1, &[2.5, 0.5]),
            (9, &[3.0, 2.5, 3.0]),
            (9, &[3.0, 2.0, 3.5]),
            (9, &[3.0, -2.0, 1.0]),
            (9, &[3.0, 2.0, -1.0]),
            (18, &[2.0, 0.0]),
        ] {
            let (np, kernel) = r_family(family).unwrap();
            assert_eq!(np, params.len());
            assert!(kernel(1.0, params, d).is_nan(), "{family} {params:?}");
        }
        assert!(norm_range(1.0, f64::NAN).is_nan());
        assert!(owen_t(0.5, f64::NAN).is_nan());
        // The point mass σ = 0 sits at μ.
        let lower = Op::P {
            lower: true,
            log: false,
        };
        assert_eq!(norm(-1.0, &[0.0, 0.0], lower), 0.0);
        assert_eq!(weibull(-1.0, &[2.0, 1.0], Op::D(false)), 0.0);
        assert_eq!(weibull(0.0, &[2.0, 1.0], Op::D(false)), 0.0);
        // The skew-normal's far-tail integral needs α > 0 and α|h| ≥ 1.
        assert_eq!(
            sn_cdf(-0.5, 1.5),
            st::norm_cdf(-0.5) - 2.0 * owen_t(-0.5, 1.5)
        );
        assert_eq!(
            sn_cdf(-2.0, -1.0),
            (st::norm_cdf(-2.0) - 2.0 * owen_t(-2.0, -1.0)).clamp(0.0, 1.0)
        );
        // The Tukey integrals' cut-offs: a vanishing density, a range already
        // certain, and the far tail of the range integral.
        assert!(tukey_p(0.5, 3.0, 2.0, 1.0) > 0.0);
        assert!(range_cdf(0.001, 2.0) > 0.0);
        assert!((range_cdf(12.0, 1e6) - 0.999_990_397_184_811).abs() < 1e-12);
    }

    #[test]
    fn owen_t_and_bivariate_edges() {
        assert!(owen_t(f64::NAN, 0.5).is_nan());
        assert_eq!(owen_t(INF, 0.5), 0.0);
        assert_eq!(owen_t_quad(1.0, 0.0), 0.0);
        // T(h, 1) = ½Φ(h)(1 − Φ(h)).
        let p = st::norm_cdf(1.3);
        assert!((owen_t(1.3, 1.0) - 0.5 * p * (1.0 - p)).abs() < 1e-16);
        // Independence: the product of the margins.
        let want = st::norm_cdf(0.4) * st::norm_cdf(-0.7);
        assert!((biv_norm(0.4, -0.7, 0.0) - want).abs() < 1e-16);
        assert!((biv_norm(0.0, -0.7, 0.2) - biv_norm(-0.7, 0.0, 0.2)).abs() < 1e-16);
        assert_eq!(call(1562, &nums(&[0.1, 0.2, 1.0])), Err(err::NUM));
    }

    #[test]
    fn discrete_edges() {
        let q = |p: f64, lower: bool| Op::Q(tail(p, lower, false));
        let up = Op::P {
            lower: false,
            log: false,
        };
        assert!(nonint(2.5) && !nonint(3.0 + 1e-9));
        // The bracket doubles out to a far quantile, and gives up past 1e15.
        assert_eq!(pois(0.0, &[200.0], q(0.999, true)), 245.0);
        assert!(disc_q(tail(0.5, true, false), 0.0, INF, |_, l| f64::from(
            u8::from(!l)
        ))
        .is_nan());
        assert_eq!(pois(0.0, &[0.0], q(0.5, false)), 0.0);
        assert_eq!(pois(1.0, &[0.0], up), 0.0);
        assert!(pois(1.0, &[f64::NAN], up).is_nan());
        assert_eq!(nbinom(2.0, &[0.0, 0.5], up), 0.0);
        assert_eq!(nbinom(0.0, &[0.0, 0.5], Op::D(false)), 1.0);
        assert!(nbinom(1.0, &[3.0, 1.5], up).is_nan());
        assert_eq!(
            binom(2.0, &[10.0, 0.3], Op::D(true)).exp(),
            binom(2.0, &[10.0, 0.3], Op::D(false))
        );
        assert_eq!(binom(10.0, &[10.0, 0.3], up), 0.0);
        assert_eq!(binom(0.0, &[10.0, 0.3], q(0.0, false)), 10.0);
        assert!(geom(1.0, &[0.3], Op::Q(tail(1.5, true, false))).is_nan());
        assert_eq!(geom(1.0, &[0.3], q(0.0, true)), 0.0);
        // The hypergeometric upper tail, on both sides of the support.
        assert_eq!(hyper(-1.0, &[3.0, 2.0, 3.0], up), 1.0);
        assert_eq!(hyper(3.0, &[3.0, 2.0, 3.0], up), 0.0);
        assert!((hyper(1.0, &[3.0, 2.0, 3.0], up) - 0.7).abs() < 1e-15);
        assert!(hyper(1.0, &[3.0, f64::INFINITY, 3.0], up).is_nan());
        assert_eq!(hyper(0.0, &[3.0, 2.0, 3.0], Op::D(false)), 0.0);
    }

    #[test]
    fn closed_form_edges() {
        let lower = Op::P {
            lower: true,
            log: false,
        };
        assert!(lnorm(1.0, &[0.0, -1.0], lower).is_nan());
        assert_eq!(lnorm(0.0, &[0.0, 1.0], Op::D(true)), NEG_INF);
        assert_eq!(exp(-1.0, &[1.0], Op::D(true)), NEG_INF);
        assert!((exp(1.0, &[2.0], Op::D(true)) - (-0.5 - 2f64.ln())).abs() < 1e-15);
        assert!(exp(0.5, &[-1.0], Op::Q(tail(1.5, true, false))).is_nan());
        assert_eq!(weibull(0.0, &[0.5, 1.0], Op::D(false)), INF);
        assert!(weibull(0.5, &[2.0, 1.0], Op::Q(tail(2.0, true, false))).is_nan());
        assert_eq!(rayleigh(0.0, &[1.0], Op::Q(tail(0.0, true, false))), 0.0);
        assert_eq!(rayleigh(-1.0, &[1.0], lower), 0.0);
        assert!(gumbel(1.0, &[0.0, 1.0], Op::Q(tail(0.5, true, true))).is_nan());
        assert!(cauchy(1.0, &[0.0, -1.0], lower).is_nan());
        assert!(cauchy(1.0, &[0.0, 1.0], Op::Q(tail(2.0, true, false))).is_nan());
        assert_eq!(cot_pi(0.5), 0.0);
        let up_log = Op::P {
            lower: true,
            log: true,
        };
        assert!((cauchy(0.5, &[0.0, 1.0], up_log) - (0.5 + 0.5f64.atan() / PI).ln()).abs() < 1e-15);
        assert!(
            (cauchy(3.0, &[0.0, 1.0], up_log) - (1.0 - (1.0f64 / 3.0).atan() / PI).ln()).abs()
                < 1e-15
        );
    }

    /// The skew-normal CDF far in the left tail (shape · |z| beyond 1e154)
    /// computed an infinite piece count and never returned (R.PSNORM,
    /// R.QSNORM, and R.PST past 1e4 degrees of freedom).
    #[test]
    fn skew_normal_far_tail_finishes() {
        let t0 = std::time::Instant::now();
        let (t, f) = (CellValue::Bool(true), CellValue::Bool(false));
        let mut a = nums(&[2.0, -1.208, -1.7e-106, 4.1e-289]);
        a.extend([t.clone(), f]);
        let _ = call(1646, &a);
        assert_eq!(val(1646, &[-1e200, 1e200, 0.0, 1.0]), 0.0);
        assert_eq!(sn_cdf(-1e200, 1e200), 0.0);
        let _ = call(1647, &nums(&[5.5e-186, 4.0e242, -0.5, 4.0e242]));
        let mut a = nums(&[-1.0, 1.07e212, 4.9e307]);
        a.extend([t.clone(), t]);
        let _ = call(1649, &a);
        assert!(t0.elapsed() < std::time::Duration::from_secs(1));
    }

    /// The discrete quantiles past 2⁵³ return: with a and b adjacent floats
    /// 2 apart, the midpoint is one of them and the bisection never ended
    /// (Gnumeric's R.QBINOM(0.5, 9.007199254740994E15, 0.5) is
    /// 4503599627370498).
    #[test]
    fn discrete_quantile_past_2_pow_53() {
        let t0 = std::time::Instant::now();
        let v = val(1605, &[0.5, 9_007_199_254_740_994.0, 0.5]);
        assert!((v / 4_503_599_627_370_498.0 - 1.0).abs() < 1e-14, "{v}");
        let cdf = |k: f64, lower: bool| {
            let p = if k >= 9_007_199_254_740_994.0 {
                1.0
            } else {
                0.0
            };
            if lower {
                p
            } else {
                1.0 - p
            }
        };
        let t = tail(0.5, true, false);
        assert_eq!(
            disc_q(t, 9_007_199_254_740_992.0, 9_007_199_254_740_996.0, cdf),
            9_007_199_254_740_994.0
        );
        assert!(t0.elapsed() < std::time::Duration::from_secs(1));
    }
}
