//! Evaluation of the P7 option pricing functions (ids 1750–1849): Gnumeric's
//! `fn-derivatives` set (Haug's closed forms, approximations and a binomial
//! tree) here, LibreOffice's barrier pricing in [`barrier`].
//!
//! Cost of carry `b` follows Haug: `r` for a stock, `r − q` with a dividend
//! yield, `0` for a future, `r − rf` for a currency.

mod barrier;

use super::statistical::{norm_cdf as nc, norm_pdf as np};
use super::{err, n, ok_num, t, R};
use crate::CellValue;
use Side::{Call, Put};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Side {
    Call,
    Put,
}

impl Side {
    /// +1 for a call, −1 for a put: a put's formula is a call's with the
    /// signs of the payoff and of the normal arguments flipped.
    pub(super) fn z(self) -> f64 {
        match self {
            Call => 1.0,
            Put => -1.0,
        }
    }
}

/// Gnumeric's call/put flag: text starting with `c` or `p` (any case).
fn side(a: &[CellValue], i: usize) -> Result<Side, u8> {
    match t(a, i)?.chars().next() {
        Some('c' | 'C') => Ok(Call),
        Some('p' | 'P') => Ok(Put),
        _ => Err(err::NUM),
    }
}

/// Evaluate function `id` (already type- and arity-checked).
pub(super) fn eval(id: u32, a: &[CellValue]) -> R {
    if id >= 1800 {
        return barrier::eval(id, a);
    }
    // The flags lead; every later argument is a number (converted first, so
    // a non-number is #VALUE! before a bad flag is #NUM!, as in Gnumeric).
    let lead = match id {
        1752 | 1755 | 1783 | 1772 | 1784 | 1791 | 1792 => 0,
        1773 => 2,
        _ => 1,
    };
    let xs = (lead..a.len())
        .map(|i| n(a, i))
        .collect::<Result<Vec<f64>, u8>>()?;
    // Missing (optional) arguments read as 0, the default cost of carry.
    let x = |i: usize| xs.get(i).copied().unwrap_or(0.0);
    let p = || side(a, 0);
    let v = match id {
        1750 => bs(p()?, x(0), x(1), x(2), x(3), x(4), x(5)),
        1751 => bs_delta(p()?, x(0), x(1), x(2), x(3), x(4), x(5)),
        1752 => bs_gamma(x(0), x(1), x(2), x(3), x(4), x(5)),
        1753 => bs_rho(p()?, x(0), x(1), x(2), x(3), x(4), x(5)),
        1754 => bs_theta(p()?, x(0), x(1), x(2), x(3), x(4), x(5)),
        1755 => bs_vega(x(0), x(1), x(2), x(3), x(4), x(5)),
        1756 => bs_carry(p()?, x(0), x(1), x(2), x(3), x(4), x(5)),
        // Garman–Kohlhagen: Black–Scholes with b = r − rf.
        1760 => bs(p()?, x(0), x(1), x(2), x(3), x(5), x(3) - x(4)),
        1761 => french(p()?, x(0), x(1), x(2), x(3), x(4), x(5), x(6)),
        1762 => jump_diff(p()?, x(0), x(1), x(2), x(3), x(4), x(5), x(6)),
        1763 => miltersen_schwartz(p()?, &xs),
        1780 => (-x(6) * x(2)).exp() * bs(p()?, x(0), x(1), x(2), x(3), x(4), x(5)),
        1781 => forward_start(p()?, x(0), x(1), x(2), x(3), x(4), x(5), x(6)),
        1782 => time_switch(p()?, &xs)?,
        1783 => simple_chooser(x(0), x(1), x(2), x(3), x(4), x(5), x(6)),
        1770 => baw(p()?, x(0), x(1), x(2), x(3), x(4), x(5)),
        1771 => bjer_stens(p()?, x(0), x(1), x(2), x(3), x(4), x(5)),
        1772 => rgw(x(0), x(1), x(2), x(3), x(4), x(5), x(6)),
        1773 => {
            let amer = match t(a, 0)?.chars().next() {
                Some('a' | 'A') => true,
                Some('e' | 'E') => false,
                _ => return Err(err::NUM),
            };
            binomial(amer, side(a, 1)?, x(0), x(1), x(2), x(3), x(4), x(5), x(6))?
        }
        1784 => complex_chooser(&xs),
        1785 => on_options(&t(a, 0)?, &xs)?,
        1786 => extendible_writer(p()?, &xs),
        1790 => two_asset(p()?, &xs),
        1791 => {
            let (s1, s2, q1, q2, tm, r, b1, b2) = (x(0), x(1), x(2), x(3), x(4), x(5), x(6), x(7));
            let v = exchange_vol(x(8), x(9), x(10));
            let d1 = d1(q1 * s1, q2 * s2, tm, v, b1 - b2);
            q1 * s1 * ((b1 - r) * tm).exp() * nc(d1)
                - q2 * s2 * ((b2 - r) * tm).exp() * nc(d1 - v * tm.sqrt())
        }
        1792 => {
            let v = exchange_vol(x(8), x(9), x(10));
            bjer_stens(
                Call,
                x(2) * x(0),
                x(3) * x(1),
                x(4),
                x(5) - x(7),
                v,
                x(6) - x(7),
            )
        }
        1793 => {
            // Kirk: the spread f1 − f2 − k as an option on f1 / (f2 + k).
            let (f1, f2, k, tm, r, v1, v2, rho) = (x(0), x(1), x(2), x(3), x(4), x(5), x(6), x(7));
            let w = v2 * f2 / (f2 + k);
            let v = (v1 * v1 + w * w - 2.0 * rho * v1 * w).sqrt();
            bs(p()?, f1 / (f2 + k), 1.0, tm, r, v, 0.0) * (f2 + k)
        }
        1795 => fixed_lookback(side(a, 0).map_err(|_| err::VALUE)?, &xs),
        1796 => float_lookback(p()?, &xs),
        _ => return Err(err::NAME),
    };
    ok_num(v)
}

/// Black–Scholes d₁.
fn d1(s: f64, x: f64, t: f64, v: f64, b: f64) -> f64 {
    ((s / x).ln() + (b + v * v / 2.0) * t) / (v * t.sqrt())
}

/// The generalized Black–Scholes price.
fn bs(side: Side, s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    let z = side.z();
    let d1 = d1(s, x, t, v, b);
    let d2 = d1 - v * t.sqrt();
    z * (s * ((b - r) * t).exp() * nc(z * d1) - x * (-r * t).exp() * nc(z * d2))
}

fn bs_delta(side: Side, s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    let n1 = nc(d1(s, x, t, v, b));
    ((b - r) * t).exp() * if side == Call { n1 } else { n1 - 1.0 }
}

fn bs_gamma(s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    ((b - r) * t).exp() * np(d1(s, x, t, v, b)) / (s * v * t.sqrt())
}

fn bs_vega(s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    s * ((b - r) * t).exp() * np(d1(s, x, t, v, b)) * t.sqrt()
}

fn bs_theta(side: Side, s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    let z = side.z();
    let d1 = d1(s, x, t, v, b);
    let d2 = d1 - v * t.sqrt();
    let carry = s * ((b - r) * t).exp();
    -carry * np(d1) * v / (2.0 * t.sqrt())
        - z * (b - r) * carry * nc(z * d1)
        - z * r * x * (-r * t).exp() * nc(z * d2)
}

fn bs_rho(side: Side, s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    if b == 0.0 {
        // An option on a future: only the discounting depends on r.
        return -t * bs(side, s, x, t, r, v, b);
    }
    let z = side.z();
    let d2 = d1(s, x, t, v, b) - v * t.sqrt();
    z * t * x * (-r * t).exp() * nc(z * d2)
}

fn bs_carry(side: Side, s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    let z = side.z();
    z * t * s * ((b - r) * t).exp() * nc(z * d1(s, x, t, v, b))
}

/// French (1984): volatility accrues over trading time `tt`, carry and
/// discounting over calendar time `ct`.
#[allow(clippy::too_many_arguments)]
fn french(side: Side, s: f64, x: f64, tt: f64, ct: f64, r: f64, v: f64, b: f64) -> f64 {
    let z = side.z();
    let d1 = ((s / x).ln() + b * ct + v * v / 2.0 * tt) / (v * tt.sqrt());
    let d2 = d1 - v * tt.sqrt();
    z * (s * ((b - r) * ct).exp() * nc(z * d1) - x * (-r * ct).exp() * nc(z * d2))
}

/// Merton (1976) jump diffusion: a Poisson mixture of Black–Scholes prices,
/// summed over 0–10 jumps as Gnumeric does. `gamma` is the share of the
/// variance due to jumps, `lambda` the jumps per year.
#[allow(clippy::too_many_arguments)]
fn jump_diff(side: Side, s: f64, x: f64, t: f64, r: f64, v: f64, lambda: f64, gamma: f64) -> f64 {
    let jump_var = gamma * v * v / lambda;
    let diffusion_var = v * v - lambda * jump_var;
    let mut weight = (-lambda * t).exp(); // e^(−λt) (λt)^i / i!
    let mut sum = 0.0;
    for i in 0..=10 {
        let vi = (diffusion_var + jump_var * (f64::from(i) / t)).sqrt();
        sum += weight * bs(side, s, x, t, r, vi, r);
        weight *= lambda * t / f64::from(i + 1);
    }
    sum
}

/// Miltersen–Schwartz (1998): an option on a commodity future with
/// stochastic convenience yield and interest rates.
fn miltersen_schwartz(side: Side, x: &[f64]) -> f64 {
    let [pt, ft, k, t1, t2, vs, ve, vf, rse, rsf, ref_, ke, kf] = take(x);
    // ∫ e^(−κ(t2−u)) du over [0, t1].
    let h = |kappa: f64| (-kappa * t2).exp() * ((kappa * t1).exp() - 1.0) / kappa;
    let var = vs * vs * t1
        + 2.0 * vs * (vf * rsf / kf * (t1 - h(kf)) - ve * rse / ke * (t1 - h(ke)))
        + ve * ve / (ke * ke) * (t1 + h(2.0 * ke) - 2.0 * h(ke))
        + vf * vf / (kf * kf) * (t1 + h(2.0 * kf) - 2.0 * h(kf))
        - 2.0 * ve * vf * ref_ / (ke * kf) * (t1 - h(ke) - h(kf) + h(ke + kf));
    let q = (1.0 - (-kf * t1).exp()) / kf;
    let vxz = vf / kf
        * (vs * rsf * (t1 - q)
            + vf / kf
                * (t1 - h(kf) - q
                    + (-kf * t2).exp() * ((kf * t1).exp() - (-kf * t1).exp()) / (2.0 * kf))
            - ve * ref_ / ke
                * (t1 - h(ke) - q
                    + (-ke * t2).exp() * ((ke * t1).exp() - (-kf * t1).exp()) / (ke + kf)));
    let vz = var.sqrt();
    let d1 = ((ft / k).ln() - vxz + var / 2.0) / vz;
    let d2 = ((ft / k).ln() - vxz - var / 2.0) / vz;
    let z = side.z();
    pt * z * (ft * (-vxz).exp() * nc(z * d1) - k * nc(z * d2))
}

/// The first `N` arguments as an array (missing ones are 0).
fn take<const N: usize>(x: &[f64]) -> [f64; N] {
    std::array::from_fn(|i| x.get(i).copied().unwrap_or(0.0))
}

/// A forward start option: struck at `alpha` times the spot at `t1`.
#[allow(clippy::too_many_arguments)]
fn forward_start(side: Side, s: f64, alpha: f64, t1: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    s * ((b - r) * t1).exp() * bs(side, 1.0, alpha, t - t1, r, v, b)
}

/// A discrete time switch option: `a` per period of `dt` the spot ends on the
/// money side of `x`, `m` periods already earned.
fn time_switch(side: Side, x: &[f64]) -> Result<f64, u8> {
    let [s, k, a, t, m, dt, r, b, v] = take(x);
    // dt = 0 is no periods (Gnumeric's loop is empty there, so 0).
    let periods = if (t / dt).is_finite() { t / dt } else { 0.0 };
    // ponytail: capped at 1e7 periods (Gnumeric loops on); a closed form
    // for the sum would lift it.
    if periods.abs() >= 1e7 {
        return Err(err::NUM);
    }
    let z = side.z();
    let mut sum = 0.0;
    for i in 1..periods.trunc() as i64 {
        let ti = i as f64 * dt;
        let d = ((s / k).ln() + (b - v * v / 2.0) * ti) / (v * ti.sqrt());
        sum += nc(z * d) * dt;
    }
    Ok(a * (-r * t).exp() * sum + dt * a * (-r * t).exp() * m)
}

/// A simple chooser: at `t1` the holder picks a call or a put expiring `t2`.
fn simple_chooser(s: f64, x: f64, t1: f64, t2: f64, r: f64, b: f64, v: f64) -> f64 {
    let d = d1(s, x, t2, v, b);
    let y = ((s / x).ln() + b * t2 + v * v * t1 / 2.0) / (v * t1.sqrt());
    let carry = s * ((b - r) * t2).exp();
    let disc = x * (-r * t2).exp();
    carry * nc(d) - disc * nc(d - v * t2.sqrt()) - carry * nc(-y) + disc * nc(-y + v * t1.sqrt())
}

/// Gauss–Legendre nodes (negative half) and weights for 6, 12 and 20 points.
const GL: [(&[f64], &[f64]); 3] = [
    (
        &[-0.932469514203152, -0.6612093864662645, -0.2386191860831969],
        &[0.17132449237917036, 0.3607615730481386, 0.46791393457269104],
    ),
    (
        &[
            -0.9815606342467192,
            -0.9041172563704749,
            -0.7699026741943047,
            -0.5873179542866175,
            -0.3678314989981802,
            -0.1252334085114689,
        ],
        &[
            0.04717533638651183,
            0.10693932599531843,
            0.16007832854334622,
            0.20316742672306592,
            0.2334925365383548,
            0.24914704581340277,
        ],
    ),
    (
        &[
            -0.9931285991850949,
            -0.9639719272779138,
            -0.912234428251326,
            -0.8391169718222188,
            -0.7463319064601508,
            -0.636053680726515,
            -0.5108670019508271,
            -0.37370608871541955,
            -0.22778585114164507,
            -0.07652652113349734,
        ],
        &[
            0.017614007139152118,
            0.04060142980038694,
            0.06267204833410907,
            0.08327674157670475,
            0.10193011981724044,
            0.11819453196151841,
            0.13168863844917664,
            0.14209610931838204,
            0.14917298647260374,
            0.15275338713072584,
        ],
    ),
];

/// Newton's method from `x0` on `f` (value, slope): the root once a step is
/// below 1e-12 relative, NaN if 100 steps do not get there.
fn newton(x0: f64, f: impl Fn(f64) -> (f64, f64)) -> f64 {
    let mut x = x0;
    for _ in 0..100 {
        let (y, dy) = f(x);
        let step = y / dy;
        x -= step;
        if step.is_nan() || step.abs() <= 1e-12 * x.abs() {
            return x;
        }
    }
    f64::NAN
}

/// Barone-Adesi–Whaley (1987) quadratic approximation of an American option.
fn baw(side: Side, s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    if side == Call && b >= r {
        // Never optimal to exercise early.
        return bs(Call, s, x, t, r, v, b);
    }
    let z = side.z();
    let (v2, st) = (v * v, t.sqrt());
    let nn = 2.0 * b / v2;
    let k = 2.0 * r / (v2 * (1.0 - (-r * t).exp()));
    let q = (-(nn - 1.0) + z * ((nn - 1.0) * (nn - 1.0) + 4.0 * k).sqrt()) / 2.0;
    let carry = ((b - r) * t).exp();
    // The critical price: exercising (z(S − x)) is worth the continuation
    // value bs + z(1 − e^((b−r)t) N(z d1)) S / q.
    let rhs = |si: f64| {
        let d1 = d1(si, x, t, v, b);
        let value = bs(side, si, x, t, r, v, b) + z * (1.0 - carry * nc(z * d1)) * si / q;
        let slope = z * carry * nc(z * d1) + z * (1.0 - carry * nc(z * d1)) / q
            - carry * np(d1) / (v * st * q);
        (value, slope)
    };
    // Seed: the perpetual option's critical price, damped toward x.
    let m = 2.0 * r / v2;
    let q_inf = (-(nn - 1.0) + z * ((nn - 1.0) * (nn - 1.0) + 4.0 * m).sqrt()) / 2.0;
    let s_inf = x / (1.0 - 1.0 / q_inf);
    let h = -(b * t + z * 2.0 * v * st) * x / (s_inf - x);
    let seed = x + (s_inf - x) * (1.0 - h.exp());
    let sk = newton(seed, |si| {
        let (value, slope) = rhs(si);
        (z * (si - x) - value, z - slope)
    });
    if z * (sk - s) > 0.0 {
        let a = z * (sk / q) * (1.0 - carry * nc(z * d1(sk, x, t, v, b)));
        bs(side, s, x, t, r, v, b) + a * (s / sk).powf(q)
    } else if sk.is_nan() {
        f64::NAN
    } else {
        z * (s - x)
    }
}

/// Bjerksund–Stensland (1993) American approximation; a put through their
/// put–call transformation.
fn bjer_stens(side: Side, s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    match side {
        Call => bjer_stens_call(s, x, t, r, v, b),
        Put => bjer_stens_call(x, s, t, r - b, v, -b),
    }
}

fn bjer_stens_call(s: f64, x: f64, t: f64, r: f64, v: f64, b: f64) -> f64 {
    if b >= r {
        return bs(Call, s, x, t, r, v, b);
    }
    let v2 = v * v;
    let beta = (0.5 - b / v2) + ((b / v2 - 0.5).powi(2) + 2.0 * r / v2).sqrt();
    let b_inf = beta / (beta - 1.0) * x;
    let b0 = x.max(r / (r - b) * x);
    let ht = -(b * t + 2.0 * v * t.sqrt()) * b0 / (b_inf - b0);
    let i = b0 + (b_inf - b0) * (1.0 - ht.exp());
    if s >= i {
        return s - x;
    }
    let phi = |gamma: f64, h: f64| {
        let lambda = (-r + gamma * b + 0.5 * gamma * (gamma - 1.0) * v2) * t;
        let vt = v * t.sqrt();
        let d = -((s / h).ln() + (b + (gamma - 0.5) * v2) * t) / vt;
        let kappa = 2.0 * b / v2 + (2.0 * gamma - 1.0);
        lambda.exp()
            * s.powf(gamma)
            * (nc(d) - (i / s).powf(kappa) * nc(d - 2.0 * (i / s).ln() / vt))
    };
    let alpha = (i - x) * i.powf(-beta);
    alpha * s.powf(beta) - alpha * phi(beta, i) + phi(1.0, i) - phi(1.0, x) - x * phi(0.0, i)
        + x * phi(0.0, x)
}

/// Roll–Geske–Whaley: an American call on a stock paying one known dividend
/// `d` at `t1`. As Gnumeric, the Black–Scholes legs carry b = 0.
fn rgw(s: f64, x: f64, t1: f64, t2: f64, r: f64, d: f64, v: f64) -> f64 {
    if s.is_nan() || s <= 0.0 {
        return f64::NAN;
    }
    let sx = s - d * (-r * t1).exp();
    let european = || bs(Call, sx, x, t2, r, v, 0.0);
    if d <= x * (1.0 - (-r * (t2 - t1)).exp()) {
        // Not optimal to exercise before the dividend.
        return european();
    }
    // The ex-dividend price i at which the call is worth i + d − x: bisect.
    let gap = |i: f64| bs(Call, i, x, t2 - t1, r, v, 0.0) - i - d + x;
    let mut hi = s;
    while gap(hi) > 0.0 && hi < 1e8 {
        hi *= 2.0;
    }
    if hi > 1e8 {
        return european();
    }
    let (mut lo, mut i) = (0.0, hi / 2.0);
    let mut g = gap(i);
    while g.abs() > 1e-5 && hi - lo > 1e-5 {
        if g < 0.0 {
            hi = i;
        } else {
            lo = i;
        }
        i = (hi + lo) / 2.0;
        g = gap(i);
    }
    let a1 = d1(sx, x, t2, v, r);
    let a2 = a1 - v * t2.sqrt();
    let b1 = d1(sx, i, t1, v, r);
    let b2 = b1 - v * t1.sqrt();
    let rho = -(t1 / t2).sqrt();
    sx * nc(b1) + sx * bvn(a1, -b1, rho)
        - x * (-r * t2).exp() * bvn(a2, -b2, rho)
        - (x - d) * (-r * t1).exp() * nc(b2)
}

/// Cox–Ross–Rubinstein binomial tree with `steps` steps (0–100000, as Gnumeric).
#[allow(clippy::too_many_arguments)]
fn binomial(
    amer: bool,
    side: Side,
    steps: f64,
    s: f64,
    x: f64,
    t: f64,
    r: f64,
    v: f64,
    b: f64,
) -> Result<f64, u8> {
    let steps = steps.floor();
    if !(0.0..=100_000.0).contains(&steps) {
        return Err(err::NUM);
    }
    let (z, n) = (side.z(), steps as usize);
    let dt = t / steps;
    let u = (v * dt.sqrt()).exp();
    let d = 1.0 / u;
    let p = ((b * dt).exp() - d) / (u - d);
    let df = (-r * dt).exp();
    let pow = |base: f64| -> Vec<f64> { (0..=n).map(|i| base.powf(i as f64)).collect() };
    let (up, down) = (pow(u), pow(d));
    // Node i of step j has i up-moves.
    let payoff = |i: usize, j: usize| z * (s * up[i] * down[j - i] - x);
    let mut value: Vec<f64> = (0..=n).map(|i| payoff(i, n).max(0.0)).collect();
    for j in (0..n).rev() {
        for i in 0..=j {
            let hold = (p * value[i + 1] + (1.0 - p) * value[i]) * df;
            value[i] = if amer { payoff(i, j).max(hold) } else { hold };
        }
    }
    Ok(value[0])
}

/// The bivariate normal CDF P(X ≤ a, Y ≤ b) at correlation `rho`: Genz's
/// (2004) form of Drezner–Wesolowsky, about 1e-15 absolute. NaN unless
/// |rho| ≤ 1.
// ponytail: private copy until the distribution family's CUM_BIV_NORM_DIST
// lands; the lead dedupes.
fn bvn(a: f64, b: f64, rho: f64) -> f64 {
    if rho.is_nan() || rho.abs() > 1.0 || a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    // Genz's BVNU computes P(X > h, Y > k).
    let (h, mut k) = (-a, -b);
    let mut hk = h * k;
    let (xs, ws) = GL[if rho.abs() < 0.3 {
        0
    } else if rho.abs() < 0.75 {
        1
    } else {
        2
    }];
    let tau = std::f64::consts::TAU;
    if rho.abs() < 0.925 {
        let hs = (h * h + k * k) / 2.0;
        let asr = rho.asin();
        let mut sum = 0.0;
        for (x, w) in xs.iter().zip(ws) {
            for sn in [(asr * (x + 1.0) / 2.0).sin(), (asr * (1.0 - x) / 2.0).sin()] {
                sum += w * ((sn * hk - hs) / (1.0 - sn * sn)).exp();
            }
        }
        return sum * asr / (2.0 * tau) + nc(-h) * nc(-k);
    }
    if rho < 0.0 {
        k = -k;
        hk = -hk;
    }
    let mut bvn = 0.0;
    if rho.abs() < 1.0 {
        let a2 = (1.0 - rho) * (1.0 + rho);
        let mut aa = a2.sqrt();
        let bs = (h - k) * (h - k);
        let c = (4.0 - hk) / 8.0;
        let d = (12.0 - hk) / 16.0;
        bvn = aa
            * (-(bs / a2 + hk) / 2.0).exp()
            * (1.0 - c * (bs - a2) * (1.0 - d * bs / 5.0) / 3.0 + c * d * a2 * a2 / 5.0);
        if hk > -160.0 {
            let bb = bs.sqrt();
            bvn -= (-hk / 2.0).exp()
                * tau.sqrt()
                * nc(-bb / aa)
                * bb
                * (1.0 - c * bs * (1.0 - d * bs / 5.0) / 3.0);
        }
        aa /= 2.0;
        for (x, w) in xs.iter().zip(ws) {
            let xs = (aa * (x + 1.0)).powi(2);
            let rs = (1.0 - xs).sqrt();
            bvn += aa
                * w
                * ((-bs / (2.0 * xs) - hk / (1.0 + rs)).exp() / rs
                    - (-(bs / xs + hk) / 2.0).exp() * (1.0 + c * xs * (1.0 + d * xs)));
            let xs = a2 * (1.0 - x).powi(2) / 4.0;
            let rs = (1.0 - xs).sqrt();
            bvn += aa
                * w
                * (-(bs / xs + hk) / 2.0).exp()
                * ((-hk * (1.0 - rs) / (2.0 * (1.0 + rs))).exp() / rs
                    - (1.0 + c * xs * (1.0 + d * xs)));
        }
        bvn = -bvn / tau;
    }
    if rho > 0.0 {
        bvn + nc(-h.max(k))
    } else {
        let mut p = -bvn;
        if k > h {
            p += if h < 0.0 {
                nc(k) - nc(h)
            } else {
                nc(-h) - nc(-k)
            };
        }
        p
    }
}

/// A complex chooser: at `t` pick a call (strike `xc`, expiry `tc`) or a put
/// (`xp`, `tp`).
fn complex_chooser(x: &[f64]) -> f64 {
    let [s, xc, xp, t, tc, tp, r, b, v] = take(x);
    // The spot at which the call and the put are worth the same at t.
    let i = newton(s, |si| {
        let gap = bs(Call, si, xc, tc - t, r, v, b) - bs(Put, si, xp, tp - t, r, v, b);
        let slope =
            bs_delta(Call, si, xc, tc - t, r, v, b) - bs_delta(Put, si, xp, tp - t, r, v, b);
        (gap, slope)
    });
    let d1 = d1(s, i, t, v, b);
    let d2 = d1 - v * t.sqrt();
    let y1 = self::d1(s, xc, tc, v, b);
    let y2 = self::d1(s, xp, tp, v, b);
    let (rho1, rho2) = ((t / tc).sqrt(), (t / tp).sqrt());
    s * ((b - r) * tc).exp() * bvn(d1, y1, rho1)
        - xc * (-r * tc).exp() * bvn(d2, y1 - v * tc.sqrt(), rho1)
        - s * ((b - r) * tp).exp() * bvn(-d1, -y2, rho2)
        + xp * (-r * tp).exp() * bvn(-d2, -y2 + v * tp.sqrt(), rho2)
}

/// An option (expiring `t1`, strike `x1`) on an option (expiring `t2`,
/// strike `x2`); `kind` is `cc`, `cp`, `pc` or `pp` (outer, inner).
fn on_options(kind: &str, x: &[f64]) -> Result<f64, u8> {
    let [s, x1, x2, t1, t2, r, b, v] = take(x);
    let inner = match kind {
        "cc" | "pc" => Call,
        "cp" | "pp" => Put,
        _ => return Err(err::VALUE),
    };
    // The spot at t1 where the inner option is worth x2.
    let i = newton(x1, |si| {
        (
            bs(inner, si, x1, t2 - t1, r, v, b) - x2,
            bs_delta(inner, si, x1, t2 - t1, r, v, b),
        )
    });
    let rho = (t1 / t2).sqrt();
    let y1 = d1(s, i, t1, v, b);
    let y2 = y1 - v * t1.sqrt();
    let z1 = d1(s, x1, t2, v, b);
    let z2 = z1 - v * t2.sqrt();
    let carry = s * ((b - r) * t2).exp();
    let disc1 = x1 * (-r * t2).exp();
    let disc2 = x2 * (-r * t1).exp();
    Ok(match kind {
        "cc" => carry * bvn(z1, y1, rho) - disc1 * bvn(z2, y2, rho) - disc2 * nc(y2),
        "pc" => disc1 * bvn(z2, -y2, -rho) - carry * bvn(z1, -y1, -rho) + disc2 * nc(-y2),
        "cp" => disc1 * bvn(-z2, -y2, rho) - carry * bvn(-z1, -y1, rho) - disc2 * nc(-y2),
        _ => carry * bvn(-z1, y1, -rho) - disc1 * bvn(-z2, y2, -rho) + disc2 * nc(y2),
    })
}

/// A writer-extendible option: struck `x1` at `t1`, extended to `t2` with
/// strike `x2` if out of the money.
fn extendible_writer(side: Side, x: &[f64]) -> f64 {
    let [s, x1, x2, t1, t2, r, b, v] = take(x);
    let z = side.z();
    let rho = (t1 / t2).sqrt();
    let z1 = d1(s, x2, t2, v, b);
    let z2 = d1(s, x1, t1, v, b);
    let (w1, w2) = ((v * v * t1).sqrt(), (v * v * t2).sqrt());
    bs(side, s, x1, t1, r, v, b)
        + z * (s * ((b - r) * t2).exp() * bvn(z * z1, -z * z2, -rho)
            - x2 * (-r * t2).exp() * bvn(z * (z1 - w2), z * (w1 - z2), -rho))
}

/// A two-asset correlation option: pays on asset 2 if asset 1 also ends in
/// the money.
fn two_asset(side: Side, x: &[f64]) -> f64 {
    let [s1, s2, x1, x2, t, b1, b2, r, v1, v2, rho] = take(x);
    let z = side.z();
    let st = t.sqrt();
    let y1 = ((s1 / x1).ln() + (b1 - v1 * v1 / 2.0) * t) / (v1 * st);
    let y2 = ((s2 / x2).ln() + (b2 - v2 * v2 / 2.0) * t) / (v2 * st);
    z * (s2 * ((b2 - r) * t).exp() * bvn(z * (y2 + v2 * st), z * (y1 + rho * v2 * st), rho)
        - x2 * (-r * t).exp() * bvn(z * y2, z * y1, rho))
}

/// The volatility of the ratio of two correlated assets.
fn exchange_vol(v1: f64, v2: f64, rho: f64) -> f64 {
    (v1 * v1 + v2 * v2 - 2.0 * rho * v1 * v2).sqrt()
}

/// Goldman–Sosin–Gatto floating-strike lookback: the strike is the minimum
/// (call) or maximum (put) price seen.
fn float_lookback(side: Side, x: &[f64]) -> f64 {
    let [s, s_min, s_max, t, r, b, v] = take(x);
    let z = side.z();
    let m = if side == Call { s_min } else { s_max };
    let a1 = d1(s, m, t, v, b);
    let a2 = a1 - v * t.sqrt();
    let k = -2.0 * b / (v * v);
    z * (s * ((b - r) * t).exp() * nc(z * a1) - m * (-r * t).exp() * nc(z * a2))
        + (-r * t).exp() * (v * v) / (2.0 * b)
            * s
            * z
            * ((s / m).powf(k) * nc(z * (-a1 + 2.0 * b / v * t.sqrt()))
                - (b * t).exp() * nc(-z * a1))
}

/// Conze–Viswanathan fixed-strike lookback: pays the maximum (call) or
/// minimum (put) price seen against strike `x`.
fn fixed_lookback(side: Side, x: &[f64]) -> f64 {
    let [s, s_min, s_max, k, t, r, b, v] = take(x);
    let z = side.z();
    let m = if side == Call { s_max } else { s_min };
    // Struck out of the money of the extreme so far: the extreme is k;
    // otherwise the extreme is m and m − k is already locked in.
    let (strike, locked) = if z * (k - m) > 0.0 {
        (k, 0.0)
    } else {
        (m, z * (m - k))
    };
    let d = d1(s, strike, t, v, b);
    let disc = (-r * t).exp();
    let p = -2.0 * b / (v * v);
    disc * locked
        + z * (s * ((b - r) * t).exp() * nc(z * d) - strike * disc * nc(z * (d - v * t.sqrt())))
        + s * disc * (v * v) / (2.0 * b)
            * z
            * (-(s / strike).powf(p) * nc(z * (d - 2.0 * b / v * t.sqrt()))
                + (b * t).exp() * nc(z * d))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::super::eval as call;
    use super::*;
    use improv_core_model::FuncId;

    fn f(id: u32, flag: &str, args: &[f64]) -> CellValue {
        let mut a = vec![CellValue::Text(flag.into())];
        a.extend(args.iter().map(|x| CellValue::num(*x)));
        call(FuncId(id), &a)
    }

    #[test]
    fn flags_and_unknown_ids() {
        let args = [100.0, 100.0, 0.5, 0.1, 0.25];
        assert_eq!(f(1750, "", &args), CellValue::Err(err::NUM));
        assert_eq!(f(1750, "P", &args), f(1750, "put", &args));
        assert_eq!(eval(1799, &[]), Err(err::NAME));
        assert_eq!(
            eval(1849, &[CellValue::num(1.0), CellValue::num(1.0)]),
            Err(err::NAME)
        );
    }

    #[test]
    fn bivariate_normal_matches_mpmath() {
        // Every branch: |rho| < 0.3, < 0.75, < 0.925, ≥ 0.925 of each sign,
        // |rho| = 1, and the NaN guard (mpmath quadrature, 30 digits).
        for (a, b, rho, want) in [
            (0.3, -0.2, 0.5, 0.336_198_437_015_518_77),
            (-1.5, 0.7, -0.2, 0.041_733_836_760_915_086),
            (0.7, 0.3, 0.2, 0.492_803_890_857_567_8),
            (1.2, 0.4, 0.6, 0.628_591_928_753_999_5),
            (-1.0, -0.5, 0.8, 0.135_080_483_795_528_95),
            (-0.5, -0.9, 0.95, 0.179_197_362_921_036_1),
            (2.0, -1.0, -0.97, 0.135_905_277_400_074_52),
            (-0.5, 1.0, -0.95, 0.152_205_184_284_615),
            (-2.5, -2.5, 0.999, 0.005_897_075_498_495_631),
        ] {
            let got = bvn(a, b, rho);
            assert!(
                (got - want).abs() < 1e-14,
                "bvn({a}, {b}, {rho}) = {got}, want {want}"
            );
        }
        assert!((bvn(0.4, -0.3, 1.0) - nc(-0.3)).abs() < 1e-15);
        assert!((bvn(0.4, -0.3, -1.0) - (nc(0.4) - nc(0.3))).abs() < 1e-15);
        assert_eq!(bvn(1.0, -1.0, -1.0), 0.0);
        assert!(bvn(0.0, 0.0, 1.5).is_nan());
    }

    #[test]
    fn newton_gives_up() {
        assert!(newton(1.0, |_| (1.0, 1.0)).is_nan());
    }

    #[test]
    fn time_switch_caps_the_period_count() {
        let args = [100.0, 110.0, 5.0, 1.0, 0.0, 1e-8, 0.06, 0.06, 0.26];
        assert_eq!(f(1782, "c", &args), CellValue::Err(err::NUM));
    }
}
