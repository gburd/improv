//! The financial functions (see the functions plan): time value of money,
//! depreciation, bonds and securities. Ids 700–799 are the scalars; the
//! ordered reductions (NPV, IRR, …) are not here.

use super::{alias, s, so, Arg, Category, FnDef};
use crate::value::ValueType;

use Arg::{Boolean as B, DateLike as D, Number as N};
use Category::Financial as C;
use ValueType as V;

#[rustfmt::skip]
pub(super) static FINANCIAL: &[FnDef] = &[
    // --- time value of money (cash paid out is negative) ---
    so(700, "FV", C, &[N, N, N, N, N], 3, V::Number, "future value: FV(rate, nper, pmt, [pv], [type])"),
    so(701, "PV", C, &[N, N, N, N, N], 3, V::Number, "present value: PV(rate, nper, pmt, [fv], [type])"),
    so(702, "PMT", C, &[N, N, N, N, N], 3, V::Number, "payment per period: PMT(rate, nper, pv, [fv], [type])"),
    so(703, "IPMT", C, &[N, N, N, N, N, N], 4, V::Number, "interest part of payment per: IPMT(rate, per, nper, pv, [fv], [type])"),
    so(704, "PPMT", C, &[N, N, N, N, N, N], 4, V::Number, "principal part of payment per: PPMT(rate, per, nper, pv, [fv], [type])"),
    so(705, "NPER", C, &[N, N, N, N, N], 3, V::Number, "number of periods: NPER(rate, pmt, pv, [fv], [type])"),
    so(706, "RATE", C, &[N, N, N, N, N, N], 3, V::Number, "rate per period (iterative): RATE(nper, pmt, pv, [fv], [type], [guess])"),
    alias(s(707, "CUMIPMT", C, &[N, N, N, N, N, N], V::Number, "interest paid from period start to end: CUMIPMT(rate, nper, pv, start_period, end_period, type)"), &["CUMIPMT_ADD"]),
    alias(s(708, "CUMPRINC", C, &[N, N, N, N, N, N], V::Number, "principal paid from period start to end: CUMPRINC(rate, nper, pv, start_period, end_period, type)"), &["CUMPRINC_ADD"]),
    s(709, "ISPMT", C, &[N, N, N, N], V::Number, "interest of period per with straight-line principal: ISPMT(rate, per, nper, pv)"),
    alias(s(710, "EFFECT", C, &[N, N], V::Number, "effective annual rate of a nominal rate compounded npery times"), &["EFFECT_ADD"]),
    alias(s(711, "NOMINAL", C, &[N, N], V::Number, "nominal annual rate of an effective rate compounded npery times"), &["NOMINAL_ADD"]),
    alias(s(712, "PDURATION", C, &[N, N, N], V::Number, "periods for pv to grow to fv at rate: PDURATION(rate, pv, fv)"), &["G_DURATION"]),
    s(713, "RRI", C, &[N, N, N], V::Number, "equivalent growth rate per period: RRI(nper, pv, fv)"),
    s(714, "DOLLARDE", C, &[N, N], V::Number, "a fractional dollar price (1.02 with fraction 16 is 1 and 2/16) as a decimal"),
    s(715, "DOLLARFR", C, &[N, N], V::Number, "a decimal dollar price as a fraction (in the integer part's units)"),
    // --- depreciation ---
    s(720, "SLN", C, &[N, N, N], V::Number, "straight-line depreciation per period: SLN(cost, salvage, life)"),
    s(721, "SYD", C, &[N, N, N, N], V::Number, "sum-of-years'-digits depreciation: SYD(cost, salvage, life, per)"),
    so(722, "DB", C, &[N, N, N, N, N], 4, V::Number, "fixed declining balance: DB(cost, salvage, life, period, [month=12])"),
    so(723, "DDB", C, &[N, N, N, N, N], 4, V::Number, "double declining balance: DDB(cost, salvage, life, period, [factor=2])"),
    so(724, "VDB", C, &[N, N, N, N, N, N, B], 5, V::Number, "declining balance between two periods: VDB(cost, salvage, life, start_period, end_period, [factor=2], [no_switch])"),
    so(725, "AMORDEGRC", C, &[N, D, D, N, N, N, N], 6, V::Number, "French degressive depreciation: AMORDEGRC(cost, date_purchased, first_period, salvage, period, rate, [basis])"),
    so(726, "AMORLINC", C, &[N, D, D, N, N, N, N], 6, V::Number, "French linear depreciation: AMORLINC(cost, date_purchased, first_period, salvage, period, rate, [basis])"),
    // --- bonds and securities (basis: 0 US 30/360, 1 actual/actual, 2 actual/360, 3 actual/365, 4 European 30/360) ---
    so(740, "ACCRINT", C, &[D, D, D, N, N, N, N, B], 6, V::Number, "accrued interest of a periodic-coupon security: ACCRINT(issue, first_interest, settlement, rate, par, frequency, [basis], [calc_method])"),
    so(741, "ACCRINTM", C, &[D, D, N, N, N], 3, V::Number, "accrued interest paid at maturity: ACCRINTM(issue, settlement, rate, [par=1000], [basis])"),
    so(742, "COUPDAYBS", C, &[D, D, N, N], 3, V::Number, "days from the coupon period's start to settlement"),
    so(743, "COUPDAYS", C, &[D, D, N, N], 3, V::Number, "days in the coupon period holding settlement"),
    so(744, "COUPDAYSNC", C, &[D, D, N, N], 3, V::Number, "days from settlement to the next coupon"),
    so(745, "COUPNCD", C, &[D, D, N, N], 3, V::DateTime, "the next coupon date after settlement"),
    so(746, "COUPNUM", C, &[D, D, N, N], 3, V::Number, "coupons payable between settlement and maturity"),
    so(747, "COUPPCD", C, &[D, D, N, N], 3, V::DateTime, "the coupon date on or before settlement"),
    so(748, "DISC", C, &[D, D, N, N, N], 4, V::Number, "discount rate: DISC(settlement, maturity, pr, redemption, [basis])"),
    alias(so(749, "DURATION", C, &[D, D, N, N, N, N], 5, V::Number, "Macaulay duration: DURATION(settlement, maturity, coupon, yld, frequency, [basis])"), &["BONDDURATION"]),
    alias(so(750, "MDURATION", C, &[D, D, N, N, N, N], 5, V::Number, "modified duration: MDURATION(settlement, maturity, coupon, yld, frequency, [basis])"), &["BONDMDURATION"]),
    so(751, "INTRATE", C, &[D, D, N, N, N], 4, V::Number, "interest rate of a fully invested security: INTRATE(settlement, maturity, investment, redemption, [basis])"),
    so(752, "ODDFPRICE", C, &[D, D, D, D, N, N, N, N, N], 8, V::Number, "price with an odd first period: ODDFPRICE(settlement, maturity, issue, first_coupon, rate, yld, redemption, frequency, [basis])"),
    so(753, "ODDFYIELD", C, &[D, D, D, D, N, N, N, N, N], 8, V::Number, "yield with an odd first period: ODDFYIELD(settlement, maturity, issue, first_coupon, rate, pr, redemption, frequency, [basis])"),
    so(754, "ODDLPRICE", C, &[D, D, D, N, N, N, N, N], 7, V::Number, "price with an odd last period: ODDLPRICE(settlement, maturity, last_interest, rate, yld, redemption, frequency, [basis])"),
    so(755, "ODDLYIELD", C, &[D, D, D, N, N, N, N, N], 7, V::Number, "yield with an odd last period: ODDLYIELD(settlement, maturity, last_interest, rate, pr, redemption, frequency, [basis])"),
    so(756, "PRICE", C, &[D, D, N, N, N, N, N], 6, V::Number, "price per 100 face value: PRICE(settlement, maturity, rate, yld, redemption, frequency, [basis])"),
    so(757, "PRICEDISC", C, &[D, D, N, N, N], 4, V::Number, "price of a discounted security: PRICEDISC(settlement, maturity, discount, redemption, [basis])"),
    so(758, "PRICEMAT", C, &[D, D, D, N, N, N], 5, V::Number, "price of a security paying interest at maturity: PRICEMAT(settlement, maturity, issue, rate, yld, [basis])"),
    so(759, "RECEIVED", C, &[D, D, N, N, N], 4, V::Number, "amount at maturity of a fully invested security: RECEIVED(settlement, maturity, investment, discount, [basis])"),
    s(760, "TBILLEQ", C, &[D, D, N], V::Number, "bond-equivalent yield of a Treasury bill: TBILLEQ(settlement, maturity, discount)"),
    s(761, "TBILLPRICE", C, &[D, D, N], V::Number, "price per 100 face of a Treasury bill: TBILLPRICE(settlement, maturity, discount)"),
    s(762, "TBILLYIELD", C, &[D, D, N], V::Number, "yield of a Treasury bill: TBILLYIELD(settlement, maturity, pr)"),
    so(763, "YIELD", C, &[D, D, N, N, N, N, N], 6, V::Number, "yield of a periodic-coupon security (iterative): YIELD(settlement, maturity, rate, pr, redemption, frequency, [basis])"),
    so(764, "YIELDDISC", C, &[D, D, N, N, N], 4, V::Number, "annual yield of a discounted security: YIELDDISC(settlement, maturity, pr, redemption, [basis])"),
    so(765, "YIELDMAT", C, &[D, D, D, N, N, N], 5, V::Number, "annual yield of a security paying interest at maturity: YIELDMAT(settlement, maturity, issue, rate, pr, [basis])"),
];
