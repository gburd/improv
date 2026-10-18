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
];
