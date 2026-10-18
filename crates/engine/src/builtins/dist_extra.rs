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
    while b - a > 1.0 {
        let m = ((a + b) / 2.0).floor();
        if ok(m) {
            b = m;
        } else {
            a = m;
        }
    }
    b
}

/// The binomial distribution of n trials of success probability p.
fn binom(x: f64, a: &[f64], op: Op) -> f64 {
    let (n, p) = (a[0], a[1]);
    if !(0.0..=1.0).contains(&p) || n < 0.0 || nonint(n) {
        return NAN;
    }
    let n = n.round();
    let cdf = |k: f64, lower: bool| {
        if lower {
            st::binom_range(n, p, 0.0, k)
        } else if k >= n {
            0.0
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
    if (q1 - q2) * 32.0 > q1 + q2 || !x2.is_finite() {
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
    if top <= 0.0 || h.is_infinite() {
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
}
