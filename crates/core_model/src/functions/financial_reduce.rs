//! The financial reductions: cash flows over an ordered category, read in
//! item order (the first row is the first period).

use super::{f, over, FnDef, Kind, Over, Ret};
use crate::value::ValueType;

use super::Arg::{DateLike as DL, Number as N};
use super::Category::Financial as FI;

#[rustfmt::skip]
pub(super) static FINANCIAL_REDUCE: &[FnDef] = &[
    over(f(1150, "NPV", Kind::Aggregate, FI, &[N, N], 2, false, Ret::Fixed(ValueType::Number), "net present value of the flows in item order, the first discounted one period (a missing item is skipped, not zero): NPV(rate, values OVER T)"), Over::At(&[1])),
    over(f(1151, "XNPV", Kind::Aggregate, FI, &[N, N, DL], 3, false, Ret::Fixed(ValueType::Number), "net present value of flows at dates, discounted from the first date (365-day years): XNPV(rate, values, dates OVER T)"), Over::At(&[1, 2])),
    over(f(1152, "IRR", Kind::Aggregate, FI, &[N, N], 1, false, Ret::Fixed(ValueType::Number), "internal rate of return of the flows in item order (iterative): IRR(values OVER T, [guess=0.1])"), Over::At(&[0])),
    over(f(1153, "XIRR", Kind::Aggregate, FI, &[N, DL, N], 2, false, Ret::Fixed(ValueType::Number), "internal rate of return of flows at dates (iterative): XIRR(values, dates OVER T, [guess=0.1])"), Over::At(&[0, 1])),
    over(f(1154, "MIRR", Kind::Aggregate, FI, &[N, N, N], 3, false, Ret::Fixed(ValueType::Number), "modified internal rate of return, outflows financed and inflows reinvested: MIRR(values OVER T, finance_rate, reinvest_rate)"), Over::At(&[0])),
    over(f(1155, "FMRR", Kind::Aggregate, FI, &[N, N, N, N], 4, false, Ret::Fixed(ValueType::Number), "financial management rate of return (GNU Oleo): outflows pulled back at the safe rate, inflows of at least reinvest_min pushed forward at the reinvestment rate: FMRR(values OVER T, safe_rate, reinvest_rate, reinvest_min)"), Over::At(&[0])),
    over(f(1156, "FVSCHEDULE", Kind::Aggregate, FI, &[N, N], 2, false, Ret::Fixed(ValueType::Number), "principal compounded over the rates in item order: FVSCHEDULE(principal, schedule OVER T)"), Over::At(&[1])),
];
