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
    s(1784, "OPT_COMPLEX_CHOOSER", C, &[N, N, N, N, N, N, N, N, N], V::Number, "complex chooser option: OPT_COMPLEX_CHOOSER(spot, strike_call, strike_put, time, time_call, time_put, rate, cost_of_carry, volatility)"),
    s(1785, "OPT_ON_OPTIONS", C, &[T, N, N, N, N, N, N, N, N], V::Number, "option on an option (\"cc\", \"cp\", \"pc\", \"pp\"): OPT_ON_OPTIONS(type, spot, strike1, strike2, time1, time2, rate, cost_of_carry, volatility)"),
    s(1786, "OPT_EXTENDIBLE_WRITER", C, &[T, N, N, N, N, N, N, N, N], V::Number, "writer-extendible option: OPT_EXTENDIBLE_WRITER(call_put, spot, strike1, strike2, time1, time2, rate, cost_of_carry, volatility)"),
    s(1790, "OPT_2_ASSET_CORRELATION", C, &[T, N, N, N, N, N, N, N, N, N, N, N], V::Number, "two-asset correlation option: OPT_2_ASSET_CORRELATION(call_put, spot1, spot2, strike1, strike2, time, cost_of_carry1, cost_of_carry2, rate, volatility1, volatility2, rho)"),
    s(1791, "OPT_EURO_EXCHANGE", C, &[N, N, N, N, N, N, N, N, N, N, N], V::Number, "European option to exchange asset 2 for asset 1: OPT_EURO_EXCHANGE(spot1, spot2, qty1, qty2, time, rate, cost_of_carry1, cost_of_carry2, volatility1, volatility2, rho)"),
    s(1792, "OPT_AMER_EXCHANGE", C, &[N, N, N, N, N, N, N, N, N, N, N], V::Number, "American option to exchange asset 2 for asset 1: OPT_AMER_EXCHANGE(spot1, spot2, qty1, qty2, time, rate, cost_of_carry1, cost_of_carry2, volatility1, volatility2, rho)"),
    s(1793, "OPT_SPREAD_APPROX", C, &[T, N, N, N, N, N, N, N, N], V::Number, "Kirk's futures spread option approximation: OPT_SPREAD_APPROX(call_put, fut_price1, fut_price2, strike, time, rate, volatility1, volatility2, rho)"),
    s(1795, "OPT_FIXED_STRK_LKBK", C, &[T, N, N, N, N, N, N, N, N], V::Number, "fixed-strike lookback: OPT_FIXED_STRK_LKBK(call_put, spot, spot_min, spot_max, strike, time, rate, cost_of_carry, volatility)"),
    s(1796, "OPT_FLOAT_STRK_LKBK", C, &[T, N, N, N, N, N, N, N], V::Number, "floating-strike lookback: OPT_FLOAT_STRK_LKBK(call_put, spot, spot_min, spot_max, time, rate, cost_of_carry, volatility)"),
    // --- LibreOffice's barrier pricing ---
    so(1800, "OPT_BARRIER", C, &[N, N, N, N, N, N, N, N, N, T, T, T, T], 12, V::Number, "barrier option: OPT_BARRIER(spot, vol, r, rf, maturity, strike, lower, upper, rebate, put_call, in_out, continuous_or_end, [greek])"),
    so(1801, "OPT_TOUCH", C, &[N, N, N, N, N, N, N, T, T, T, T], 10, V::Number, "one/no-touch option: OPT_TOUCH(spot, vol, r, rf, maturity, lower, upper, foreign_domestic, in_out, continuous_or_end, [greek])"),
    s(1802, "OPT_PROB_HIT", C, &[N, N, N, N, N, N], V::Number, "probability of touching a barrier: OPT_PROB_HIT(spot, vol, mu, maturity, lower, upper)"),
    so(1803, "OPT_PROB_INMONEY", C, &[N, N, N, N, N, N, N, T], 6, V::Number, "probability of ending between the barriers (and in the money): OPT_PROB_INMONEY(spot, vol, mu, maturity, lower, upper, [strike], [put_call])"),
];
