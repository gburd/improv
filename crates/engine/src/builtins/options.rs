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
        1752 | 1755 => 0,
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
}
