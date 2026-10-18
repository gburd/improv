//! Evaluation of the P7 option pricing functions (ids 1750–1849): Gnumeric's
//! `fn-derivatives` set (Haug's closed forms, approximations and a binomial
//! tree).
//!
//! Cost of carry `b` follows Haug: `r` for a stock, `r − q` with a dividend
//! yield, `0` for a future, `r − rf` for a currency.

use super::statistical::{norm_cdf as nc, norm_pdf as np};
use super::{err, n, ok_num, t, R};
use crate::CellValue;
use Side::{Call, Put};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Call,
    Put,
}

impl Side {
    /// +1 for a call, −1 for a put: a put's formula is a call's with the
    /// signs of the payoff and of the normal arguments flipped.
    fn z(self) -> f64 {
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
    // The flags lead; every later argument is a number (converted first, so
    // a non-number is #VALUE! before a bad flag is #NUM!, as in Gnumeric).
    let lead = match id {
        1752 | 1755 | 1783 => 0,
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
        assert_eq!(eval(1849, &[]), Err(err::NAME));
    }

    #[test]
    fn time_switch_caps_the_period_count() {
        let args = [100.0, 110.0, 5.0, 1.0, 0.0, 1e-8, 0.06, 0.06, 0.26];
        assert_eq!(f(1782, "c", &args), CellValue::Err(err::NUM));
    }
}
