//! The statistical reductions (P4): descriptive and conditional statistics,
//! ranks, regression and the classical tests, all `F(x OVER T, …)`.
//!
//! The array results (`FREQUENCY`, `LINEST`, `LOGEST`) are in `array`.

use super::{alias, f, over, FnDef, Kind, Over, Ret};
use crate::value::ValueType;

use super::Arg::{Any as A, Boolean as B, Number as N};
use super::Category::{Math as M, Statistical as St};
use Kind::{Aggregate as Ag, PerItem as P};

const NUM: Ret = Ret::Fixed(ValueType::Number);

/// `SUMIFS(sum_x OVER T, c1, k1, c2, k2, …)`: argument 0 and every criteria
/// range (1, 3, 5, …) are reduced; Excel allows 127 pairs.
const IFS: [usize; 128] = {
    let mut a = [0; 128];
    let mut i = 1;
    while i < 128 {
        a[i] = 2 * i - 1;
        i += 1;
    }
    a
};

/// One measure reduced, the other arguments parameters.
#[cfg_attr(coverage_nightly, coverage(off))]
const fn r(
    id: u32,
    name: &'static str,
    cat: super::Category,
    params: &'static [super::Arg],
    required: usize,
    doc: &'static str,
) -> FnDef {
    f(id, name, Ag, cat, params, required, false, NUM, doc)
}

#[rustfmt::skip]
pub(super) static STATISTICAL_REDUCE: &[FnDef] = &[
    // --- sums (§5.4) ---
    r(1000, "SUMA", M, &[A], 1, "the sum, counting TRUE as 1 and text as 0"),
    over(f(1043, "SUMPRODUCT", Ag, M, &[N], 1, true, NUM, "Σ xᵢ·yᵢ·… over the rows where every measure has a cell"), Over::Every(0, 1)),
    over(r(1044, "SUMX2MY2", M, &[N, N], 2, "Σ(x² − y²)"), Over::At(&[0, 1])),
    over(r(1045, "SUMX2PY2", M, &[N, N], 2, "Σ(x² + y²)"), Over::At(&[0, 1])),
    over(r(1046, "SUMXMY2", M, &[N, N], 2, "Σ(x − y)²"), Over::At(&[0, 1])),
    over(r(1047, "SERIESSUM", M, &[N, N, N, N], 4, "Σ cᵢ·x^(n + i·m), the coefficients c in item order (i from 0)"), Over::At(&[3])),
    over(r(1048, "POLYNOMIAL", M, &[N, N], 2, "Σ cᵢ·xⁱ, the coefficients c in item order (the constant c₀ first)"), Over::At(&[1])),
    over(r(1049, "PERCENTOF", M, &[N, N], 2, "Σ subset / Σ all"), Over::At(&[0, 1])),
    // --- means, counts and conditional reductions ---
    r(1001, "AVERAGEA", St, &[A], 1, "the mean, counting TRUE as 1 and text and FALSE as 0"),
    over(r(1002, "AVERAGEIF", St, &[A, A, N], 2, "the mean of avg_x (else x) where x meets the criterion"), Over::At(&[0, 2])),
    over(f(1003, "AVERAGEIFS", Ag, St, &[N, A, A], 3, true, NUM, "the mean of x where every criteria measure meets its criterion"), Over::At(&IFS)),
    r(1004, "AVEDEV", St, &[N], 1, "the mean absolute deviation from the mean"),
    alias(r(1005, "COUNT", St, &[A], 1, "the number of numbers (and dates)"), &["DCOUNT"]),
    alias(r(1006, "COUNTA", St, &[A], 1, "the number of cells (errors included)"), &["DCOUNTA"]),
    r(1007, "COUNTBLANK", St, &[A], 1, "the number of items with no cell (or empty text)"),
    r(1008, "COUNTIF", St, &[A, A], 2, "the number of cells meeting the criterion (\">5\", \"<>x\", \"a*\", 3)"),
    over(f(1009, "COUNTIFS", Ag, St, &[A, A], 2, true, NUM, "the number of rows where every criteria measure meets its criterion"), Over::Every(0, 2)),
    over(r(1010, "SUMIF", St, &[A, A, N], 2, "the sum of sum_x (else x) where x meets the criterion"), Over::At(&[0, 2])),
    over(f(1011, "SUMIFS", Ag, St, &[N, A, A], 3, true, NUM, "the sum of x where every criteria measure meets its criterion"), Over::At(&IFS)),
    r(1012, "MAXA", St, &[A], 1, "the largest value, counting TRUE as 1 and text and FALSE as 0"),
    over(f(1013, "MAXIFS", Ag, St, &[N, A, A], 3, true, NUM, "the largest x where every criteria measure meets its criterion (0 if none)"), Over::At(&IFS)),
    r(1014, "MINA", St, &[A], 1, "the smallest value, counting TRUE as 1 and text and FALSE as 0"),
    over(f(1015, "MINIFS", Ag, St, &[N, A, A], 3, true, NUM, "the smallest x where every criteria measure meets its criterion (0 if none)"), Over::At(&IFS)),
    // --- location and spread ---
    r(1016, "MEDIAN", St, &[N], 1, "the median"),
    alias(r(1017, "MODE", St, &[N], 1, "the most frequent value (the first in item order on a tie; #N/A if none repeats)"), &["MODE.SNGL"]),
    r(1018, "GEOMEAN", St, &[N], 1, "the geometric mean (#NUM! for a value ≤ 0)"),
    r(1019, "HARMEAN", St, &[N], 1, "the harmonic mean (#NUM! for a value ≤ 0)"),
    r(1020, "TRIMMEAN", St, &[N, N], 2, "the mean without the fraction of values at the tails (rounded down to an even count)"),
    r(1021, "DEVSQ", St, &[N], 1, "Σ(x − mean)²"),
    alias(r(1022, "STDEV", St, &[N], 1, "the sample standard deviation"), &["STDEV.S", "DSTDEV"]),
    alias(r(1023, "STDEVP", St, &[N], 1, "the population standard deviation"), &["STDEV.P", "DSTDEVP"]),
    r(1024, "STDEVA", St, &[A], 1, "the sample standard deviation, counting TRUE as 1 and text and FALSE as 0"),
    r(1025, "STDEVPA", St, &[A], 1, "the population standard deviation, counting TRUE as 1 and text and FALSE as 0"),
    alias(r(1026, "VAR", St, &[N], 1, "the sample variance"), &["VAR.S", "DVAR"]),
    alias(r(1027, "VARP", St, &[N], 1, "the population variance"), &["VAR.P", "DVARP"]),
    r(1028, "VARA", St, &[A], 1, "the sample variance, counting TRUE as 1 and text and FALSE as 0"),
    r(1029, "VARPA", St, &[A], 1, "the population variance, counting TRUE as 1 and text and FALSE as 0"),
    r(1030, "KURT", St, &[N], 1, "the sample excess kurtosis"),
    r(1031, "KURTP", St, &[N], 1, "the population excess kurtosis"),
    r(1032, "SKEW", St, &[N], 1, "the sample skewness"),
    alias(r(1033, "SKEWP", St, &[N], 1, "the population skewness"), &["SKEW.P"]),
    // --- order statistics ---
    r(1034, "LARGE", St, &[N, N], 2, "the k-th largest value"),
    r(1035, "SMALL", St, &[N, N], 2, "the k-th smallest value"),
    alias(r(1036, "PERCENTILE", St, &[N, N], 2, "the p-th percentile, interpolated, 0 ≤ p ≤ 1"), &["PERCENTILE.INC"]),
    r(1037, "PERCENTILE.EXC", St, &[N, N], 2, "the p-th percentile, exclusive of 0 and 1"),
    alias(r(1038, "QUARTILE", St, &[N, N], 2, "the q-th quartile (0 min … 4 max)"), &["QUARTILE.INC"]),
    r(1039, "QUARTILE.EXC", St, &[N, N], 2, "the q-th quartile (1 … 3), exclusive"),
    over(r(1040, "PROB", St, &[N, N, N, N], 3, "Σ p over the values x in [low, high] (x = low without high); #NUM! unless every 0 < p ≤ 1 and Σ p = 1"), Over::At(&[0, 1])),
    over(r(1041, "SUBTOTAL", St, &[N, N], 2, "the reduction a code picks: 1 AVERAGE, 2 COUNT, 3 COUNTA, 4 MAX, 5 MIN, 6 PRODUCT, 7 STDEV, 8 STDEVP, 9 SUM, 10 VAR, 11 VARP (101–111 the same)"), Over::At(&[1])),
    over(r(1042, "AGGREGATE", St, &[N, N, N, N], 3, "SUBTOTAL's codes and 12 MEDIAN, 13 MODE, 14 LARGE, 15 SMALL, 16 PERCENTILE, 17 QUARTILE, 18 PERCENTILE.EXC, 19 QUARTILE.EXC; options 2, 3, 6, 7 skip errors"), Over::At(&[2])),
    // --- per item: the result keeps the OVER category ---
    alias(f(1050, "RANK", P, St, &[N, N], 1, false, NUM, "each item's rank among the values (order 0, the default, ranks the largest 1)"), &["RANK.EQ"]),
    f(1051, "RANK.AVG", P, St, &[N, N], 1, false, NUM, "each item's rank, ties sharing their mean rank"),
    alias(f(1052, "PERCENTRANK", P, St, &[N, N, N], 1, false, NUM, "each item's (or v's) rank as a fraction 0 … 1, to digits (3) significant digits"), &["PERCENTRANK.INC"]),
    f(1053, "PERCENTRANK.EXC", P, St, &[N, N, N], 1, false, NUM, "each item's (or v's) rank as a fraction, exclusive of 0 and 1"),
    over(f(1054, "TREND", P, St, &[N, N, B], 2, false, NUM, "each item's value on the least-squares line of y on x, at its own x (through 0 if const is FALSE)"), Over::At(&[0, 1])),
    over(f(1055, "GROWTH", P, St, &[N, N, B], 2, false, NUM, "each item's value on the least-squares curve y = b·mˣ (fitting ln y; b = 1 if const is FALSE)"), Over::At(&[0, 1])),
    // --- regression ---
    alias(over(r(1060, "CORREL", St, &[N, N], 2, "the Pearson correlation of x and y"), Over::At(&[0, 1])), &["PEARSON"]),
    over(r(1061, "RSQ", St, &[N, N], 2, "the square of the Pearson correlation"), Over::At(&[0, 1])),
    alias(over(r(1062, "COVAR", St, &[N, N], 2, "the population covariance"), Over::At(&[0, 1])), &["COVARIANCE.P"]),
    over(r(1063, "COVARIANCE.S", St, &[N, N], 2, "the sample covariance"), Over::At(&[0, 1])),
    over(r(1064, "SLOPE", St, &[N, N], 2, "the least-squares slope of y on x"), Over::At(&[0, 1])),
    over(r(1065, "INTERCEPT", St, &[N, N], 2, "the least-squares intercept of y on x"), Over::At(&[0, 1])),
    over(r(1066, "STEYX", St, &[N, N], 2, "the standard error of the predicted y"), Over::At(&[0, 1])),
    alias(over(r(1067, "FORECAST", St, &[N, N, N], 3, "the least-squares line of y on x at x0"), Over::At(&[1, 2])), &["FORECAST.LINEAR"]),
    over(r(1068, "EXPA", St, &[N, N, B], 2, "A of the fit y = A·e^(Bx): least squares of ln y weighted by y (simple = TRUE: unweighted, as LOGEST)"), Over::At(&[0, 1])),
    over(r(1069, "EXPB", St, &[N, N, B], 2, "B of the fit y = A·e^(Bx)"), Over::At(&[0, 1])),
    over(r(1070, "EXPFORECAST", St, &[N, N, N, B], 3, "the fit y = A·e^(Bx) at x0"), Over::At(&[1, 2])),
    // --- tests ---
    alias(over(r(1080, "CHITEST", St, &[N, N], 2, "the χ² test p-value of actual against expected"), Over::At(&[0, 1])), &["CHISQ.TEST"]),
    alias(over(r(1081, "FTEST", St, &[N, N], 2, "the two-tailed F-test p-value that a and b have the same variance"), Over::At(&[0, 1])), &["F.TEST"]),
    alias(over(r(1082, "TTEST", St, &[N, N, N, N], 4, "the t-test p-value (tails 1 or 2; type 1 paired, 2 equal variance, 3 unequal)"), Over::At(&[0, 1])), &["T.TEST"]),
    alias(r(1083, "ZTEST", St, &[N, N, N], 2, "the one-tailed z-test p-value of the mean against μ0 (σ defaults to the sample's)"), &["Z.TEST"]),
    alias(r(1084, "AUTOCORREL", St, &[N], 1, "the lag-1 autocorrelation in item order (GSL)"), &["GSL_STATS_LAG1_AUTOCORRELATION"]),
];
