//! P7: option pricing (Gnumeric's and LibreOffice's). Ids 1750–1849.
//!
//! The Gnumeric functions take `"c"`/`"p"` (call/put) as text, as Gnumeric
//! does; rates, volatilities and carry are annual fractions, times are years.

use super::{s, so, Arg, Category, FnDef};
use crate::value::ValueType;

use Arg::{Number as N, Text as T};
use Category::Financial as C;
use ValueType as V;

#[rustfmt::skip]
pub(super) static OPTIONS: &[FnDef] = &[
    // --- generalized Black–Scholes (cost_of_carry defaults to 0) ---
    so(1750, "OPT_BS", C, &[T, N, N, N, N, N, N], 6, V::Number, "Black-Scholes price of a European option: OPT_BS(call_put, spot, strike, time, rate, volatility, [cost_of_carry])"),
    so(1751, "OPT_BS_DELTA", C, &[T, N, N, N, N, N, N], 6, V::Number, "Black-Scholes delta: OPT_BS_DELTA(call_put, spot, strike, time, rate, volatility, [cost_of_carry])"),
    so(1752, "OPT_BS_GAMMA", C, &[N, N, N, N, N, N], 5, V::Number, "Black-Scholes gamma (calls and puts alike): OPT_BS_GAMMA(spot, strike, time, rate, volatility, [cost_of_carry])"),
    so(1753, "OPT_BS_RHO", C, &[T, N, N, N, N, N, N], 6, V::Number, "Black-Scholes rho: OPT_BS_RHO(call_put, spot, strike, time, rate, volatility, [cost_of_carry])"),
    so(1754, "OPT_BS_THETA", C, &[T, N, N, N, N, N, N], 6, V::Number, "Black-Scholes theta: OPT_BS_THETA(call_put, spot, strike, time, rate, volatility, [cost_of_carry])"),
    so(1755, "OPT_BS_VEGA", C, &[N, N, N, N, N, N], 5, V::Number, "Black-Scholes vega (calls and puts alike): OPT_BS_VEGA(spot, strike, time, rate, volatility, [cost_of_carry])"),
    so(1756, "OPT_BS_CARRYCOST", C, &[T, N, N, N, N, N, N], 6, V::Number, "Black-Scholes sensitivity to the cost of carry: OPT_BS_CARRYCOST(call_put, spot, strike, time, rate, volatility, [cost_of_carry])"),
    // --- European variants ---
    s(1760, "OPT_GARMAN_KOHLHAGEN", C, &[T, N, N, N, N, N, N], V::Number, "currency option: OPT_GARMAN_KOHLHAGEN(call_put, spot, strike, time, domestic_rate, foreign_rate, volatility)"),
    s(1761, "OPT_FRENCH", C, &[T, N, N, N, N, N, N, N], V::Number, "Black-Scholes with trading-day volatility: OPT_FRENCH(call_put, spot, strike, trading_time, calendar_time, rate, volatility, cost_of_carry)"),
    s(1762, "OPT_JUMP_DIFF", C, &[T, N, N, N, N, N, N, N], V::Number, "Merton jump diffusion: OPT_JUMP_DIFF(call_put, spot, strike, time, rate, volatility, lambda, gamma)"),
    s(1763, "OPT_MILTERSEN_SCHWARTZ", C, &[T, N, N, N, N, N, N, N, N, N, N, N, N, N], V::Number, "commodity futures option: OPT_MILTERSEN_SCHWARTZ(call_put, p_t, f_t, strike, t1, t2, v_s, v_e, v_f, rho_se, rho_sf, rho_ef, kappa_e, kappa_f)"),
    s(1780, "OPT_EXEC", C, &[T, N, N, N, N, N, N, N], V::Number, "executive stock option: OPT_EXEC(call_put, spot, strike, time, rate, volatility, cost_of_carry, lambda)"),
    s(1781, "OPT_FORWARD_START", C, &[T, N, N, N, N, N, N, N], V::Number, "forward start option: OPT_FORWARD_START(call_put, spot, alpha, time_start, time, rate, volatility, cost_of_carry)"),
    s(1782, "OPT_TIME_SWITCH", C, &[T, N, N, N, N, N, N, N, N, N], V::Number, "discrete time switch option: OPT_TIME_SWITCH(call_put, spot, strike, a, time, m, dt, rate, cost_of_carry, volatility)"),
    s(1783, "OPT_SIMPLE_CHOOSER", C, &[N, N, N, N, N, N, N], V::Number, "simple chooser option: OPT_SIMPLE_CHOOSER(spot, strike, time1, time2, rate, cost_of_carry, volatility)"),
    s(1770, "OPT_BAW_AMER", C, &[T, N, N, N, N, N, N], V::Number, "Barone-Adesi-Whaley American approximation: OPT_BAW_AMER(call_put, spot, strike, time, rate, volatility, cost_of_carry)"),
    s(1771, "OPT_BJER_STENS", C, &[T, N, N, N, N, N, N], V::Number, "Bjerksund-Stensland American approximation: OPT_BJER_STENS(call_put, spot, strike, time, rate, volatility, cost_of_carry)"),
    s(1772, "OPT_RGW", C, &[N, N, N, N, N, N, N], V::Number, "Roll-Geske-Whaley American call with one known dividend: OPT_RGW(spot, strike, time_payout, time_exp, rate, d, volatility)"),
    so(1773, "OPT_BINOMIAL", C, &[T, T, N, N, N, N, N, N, N], 8, V::Number, "Cox-Ross-Rubinstein binomial tree: OPT_BINOMIAL(amer_euro, call_put, steps, spot, strike, time, rate, volatility, [cost_of_carry])"),
];
