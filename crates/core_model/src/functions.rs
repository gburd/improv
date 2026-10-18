//! The built-in function registry: ONE table for every function the formula
//! language knows, read by the parser (names, aliases, arity), the compiler
//! (argument and result types, kind), the GUI and CNL (spellings), and the
//! generated function reference (`docs/src/functions.md`).
//!
//! Evaluation lives in the engine (`improv_engine::builtins`); a test there
//! walks [`all`] and fails if any evaluable id has no implementation.
//!
//! A [`FuncId`] is persisted in every stored formula, so ids are stable
//! forever: never renumber or reuse one. Aliases resolve to their canonical
//! id at parse time, so stored formulas never hold an alias.

use crate::formula::FuncId;
use crate::value::ValueType;

/// How a function is compiled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Element-wise over its arguments' aligned cells (`ABS`, `LEFT`, n-ary
    /// `SUM(a, b)`).
    Scalar,
    /// Collapses the `OVER` categories: `SUM(x OVER Time)`. The arguments at
    /// [`FnDef::over`] positions are measures reduced together (joined on
    /// their full coordinate); the others are parameters, one value per
    /// group (`LARGE(x OVER T, 2)`, `NPV(rate, cf OVER T)`).
    Aggregate,
    /// Like [`Kind::Aggregate`] but one result per item of the (single)
    /// `OVER` category, which the result keeps: `RANK(x OVER T)`.
    PerItem,
    /// A list of results laid along the `INTO` category's items (the k-th
    /// result at its k-th item): `SORT(x OVER T INTO Rank)`.
    Array,
    /// Evaluates only the arguments it needs (`IF`, `IFERROR`, …); the compiler
    /// lowers it specially.
    Lazy,
}

/// An argument's type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arg {
    Number,
    Text,
    Boolean,
    /// A date and time.
    Date,
    /// A number or a date (`YEAR`, `EDATE`, the bond functions): dates as
    /// themselves, numbers as spreadsheet serial dates.
    DateLike,
    /// Any value type (`CONCAT`, `ISERROR`).
    Any,
    /// A category name, not a value (`POSITION(Time)`).
    Category,
}

/// What a function returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ret {
    Fixed(ValueType),
    /// The unified type of the value arguments (`IF`'s branches); the compiler
    /// computes it per [`Kind::Lazy`] function.
    Unify,
    /// The type of argument `i` (`XLOOKUP`'s values, not its keys).
    Arg(usize),
}

/// A function's documentation category (groups the reference page).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    Aggregation,
    Logical,
    Information,
    Math,
    Rounding,
    Text,
    DateTime,
    Statistical,
    Financial,
    Dimensional,
    Engineering,
}

/// Which arguments of an aggregation are reduced over the `OVER` categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Over {
    /// These positions (`CORREL(x, y OVER T)` is `[0, 1]`).
    At(&'static [usize]),
    /// From `start`, every `step`-th argument (`COUNTIFS(c1 OVER T, k1, c2,
    /// k2, …)` is `Every(0, 2)`).
    Every(usize, usize),
    /// These positions, then from `start` every `step`-th (`SORTBY(x, by1
    /// OVER T, o1, by2, o2, …)` is `AtEvery(&[0, 1], 3, 2)`).
    AtEvery(&'static [usize], usize, usize),
    /// The leading arguments that are measure references, at least `min`
    /// (`LINEST(y, x1, x2 OVER T, const, stats)`); the parser decides and
    /// records it on the references' `over`, so later readers ask
    /// [`FnDef::reduced`] with the argument.
    Leading(usize),
}

impl Over {
    /// Whether argument `i` is reduced.
    pub fn contains(&self, i: usize) -> bool {
        match self {
            Over::At(v) => v.contains(&i),
            Over::Every(start, step) => i >= *start && (i - start).is_multiple_of(*step),
            Over::AtEvery(v, start, step) => {
                v.contains(&i) || (i >= *start && (i - start).is_multiple_of(*step))
            }
            Over::Leading(min) => i < *min,
        }
    }
}

/// One built-in function.
#[derive(Debug, Clone, Copy)]
pub struct FnDef {
    pub id: FuncId,
    /// Canonical (Excel) spelling.
    pub name: &'static str,
    /// Other spellings that mean exactly this function.
    pub aliases: &'static [&'static str],
    pub kind: Kind,
    pub category: Category,
    /// Leading required arguments, then `optional` optional ones; with
    /// `variadic`, the last parameter repeats (at least once).
    pub params: &'static [Arg],
    pub required: usize,
    pub variadic: bool,
    pub ret: Ret,
    /// One-line description for the reference page.
    pub doc: &'static str,
    /// For [`Kind::Aggregate`] / [`Kind::PerItem`]: the reduced arguments.
    pub over: Over,
}

impl FnDef {
    /// Whether `n` arguments is a valid call.
    pub fn accepts(&self, n: usize) -> bool {
        n >= self.required && (self.variadic || n <= self.params.len())
    }

    /// The expected type of argument `i` (variadic tails repeat the last).
    pub fn param(&self, i: usize) -> Arg {
        self.params
            .get(i)
            .or(self.params.last())
            .copied()
            .unwrap_or(Arg::Any)
    }

    /// Whether argument `i` (as parsed, `arg`) is reduced over the OVER
    /// categories. For [`Over::Leading`] the parser marked the reduced
    /// references with their `over`.
    pub fn reduced(&self, i: usize, arg: &crate::formula::Expr) -> bool {
        match self.over {
            Over::Leading(_) => {
                matches!(arg, crate::formula::Expr::Ref(_, s) if !s.over.is_empty())
            }
            o => o.contains(i),
        }
    }

    /// The parameters' names, for signature help and ghost text: from the
    /// table in `functions/params.rs`, else those the description spells
    /// (`PMT(rate, nper, pv, [fv], [type])`), else each parameter's type
    /// (`number`). Optional ones are `[bracketed]`, a variadic tail is `…`,
    /// a reduced one is `x OVER T`, and a list result ends in `INTO K`.
    pub fn param_names(&self) -> Vec<String> {
        self.named_params().unwrap_or_else(|| self.type_names())
    }

    /// The names from the table or the description, if either has them.
    fn named_params(&self) -> Option<Vec<String>> {
        // An aggregation and its scalar twin share a name: the entry
        // with an OVER is the aggregation's.
        let mut rows = params::PARAM_NAMES.iter().filter(|(n, _)| *n == self.name);
        rows.clone()
            .find(|(_, p)| p.contains(" OVER ") == self.kind.reduces())
            .or_else(|| rows.next())
            .map(|(_, p)| split_params(p))
            .or_else(|| spelled_params(self.name, self.doc))
    }

    /// The fallback: each parameter's type, as [`Self::signature`] lists them.
    fn type_names(&self) -> Vec<String> {
        let mut sig = self.signature();
        sig.truncate(sig.len() - 1);
        let inner = &sig[self.name.len() + 1..];
        if inner.is_empty() {
            return Vec::new();
        }
        inner.split(", ").map(str::to_string).collect()
    }

    /// `PMT(rate, nper, pv, [fv], [type])`: the name and [`Self::param_names`].
    pub fn hint(&self) -> String {
        format!("{}({})", self.name, self.param_names().join(", "))
    }

    /// `ABS(x)`-style signature for docs and error messages.
    pub fn signature(&self) -> String {
        let names: Vec<String> = self
            .params
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let t = match a {
                    Arg::Number => "number",
                    Arg::Text => "text",
                    Arg::Boolean => "boolean",
                    Arg::Date => "date",
                    Arg::DateLike => "date",
                    Arg::Any => "value",
                    Arg::Category => "category",
                };
                let t = if self.kind.reduces() && self.over.contains(i) {
                    format!("{t} OVER category")
                } else {
                    t.to_string()
                };
                if i >= self.required {
                    format!("[{t}]")
                } else {
                    t
                }
            })
            .collect();
        let mut s = names.join(", ");
        if self.variadic {
            s.push_str(", …");
        }
        format!("{}({s})", self.name)
    }
}

use Arg::{Any as A, Boolean as B, Number as N, Text as T};
use Category as C;
use ValueType as V;

#[allow(
    clippy::too_many_arguments,
    reason = "a table row: one argument per FnDef field"
)]
// Evaluated at compile time (they build the static table), so no test
// can execute them: excluded from the runtime coverage report.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) const fn f(
    id: u32,
    name: &'static str,
    kind: Kind,
    category: Category,
    params: &'static [Arg],
    required: usize,
    variadic: bool,
    ret: Ret,
    doc: &'static str,
) -> FnDef {
    FnDef {
        id: FuncId(id),
        name,
        aliases: &[],
        kind,
        category,
        params,
        required,
        variadic,
        ret,
        doc,
        over: Over::At(&[0]),
    }
}

/// `d` with its reduced arguments.
#[cfg_attr(coverage_nightly, coverage(off))]
pub const fn over(mut d: FnDef, over: Over) -> FnDef {
    d.over = over;
    d
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) const fn alias(mut d: FnDef, aliases: &'static [&'static str]) -> FnDef {
    d.aliases = aliases;
    d
}

/// A scalar of `params` (all required) returning `ret`.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) const fn s(
    id: u32,
    name: &'static str,
    cat: Category,
    params: &'static [Arg],
    ret: ValueType,
    doc: &'static str,
) -> FnDef {
    f(
        id,
        name,
        Kind::Scalar,
        cat,
        params,
        params.len(),
        false,
        Ret::Fixed(ret),
        doc,
    )
}

/// A scalar with `required` of `params` mandatory.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) const fn so(
    id: u32,
    name: &'static str,
    cat: Category,
    params: &'static [Arg],
    required: usize,
    ret: ValueType,
    doc: &'static str,
) -> FnDef {
    f(
        id,
        name,
        Kind::Scalar,
        cat,
        params,
        required,
        false,
        Ret::Fixed(ret),
        doc,
    )
}

/// A variadic scalar: one or more `arg`s.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) const fn sv(
    id: u32,
    name: &'static str,
    cat: Category,
    arg: &'static [Arg],
    ret: ValueType,
    doc: &'static str,
) -> FnDef {
    f(
        id,
        name,
        Kind::Scalar,
        cat,
        arg,
        1,
        true,
        Ret::Fixed(ret),
        doc,
    )
}

#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) const fn agg(
    id: u32,
    name: &'static str,
    arg: &'static [Arg],
    ret: ValueType,
    doc: &'static str,
) -> FnDef {
    f(
        id,
        name,
        Kind::Aggregate,
        C::Aggregation,
        arg,
        1,
        false,
        Ret::Fixed(ret),
        doc,
    )
}

/// The P1 core: aggregations, logical, information, math, rounding, text.
/// Order is documentation order within a category.
#[rustfmt::skip]
pub static CORE: &[FnDef] = &[
    // --- aggregations: F(x OVER Cat) ---
    alias(agg(1, "SUM", &[N], V::Number, "the sum over a category"), &["DSUM"]),
    alias(agg(2, "AVERAGE", &[N], V::Number, "the mean over a category"), &["AVG", "DAVERAGE"]),
    alias(agg(3, "MIN", &[N], V::Number, "the smallest value over a category"), &["DMIN"]),
    alias(agg(4, "MAX", &[N], V::Number, "the largest value over a category"), &["DMAX"]),
    alias(agg(5, "PRODUCT", &[N], V::Number, "the product over a category"), &["DPRODUCT"]),
    agg(6, "SUMSQ", &[N], V::Number, "the sum of squares over a category"),
    agg(7, "AND", &[B], V::Boolean, "whether every value over a category is true"),
    agg(8, "OR", &[B], V::Boolean, "whether any value over a category is true"),
    agg(9, "XOR", &[B], V::Boolean, "whether an odd number of values over a category are true"),
    // --- scalar twins of the aggregations: F(a, b, …) ---
    sv(22, "SUM", C::Math, &[N], V::Number, "the sum of the arguments"),
    sv(23, "AVERAGE", C::Math, &[N], V::Number, "the mean of the arguments"),
    alias(sv(20, "MIN", C::Math, &[N], V::Number, "the smallest argument"), &["MIN2"]),
    alias(sv(21, "MAX", C::Math, &[N], V::Number, "the largest argument"), &["MAX2"]),
    sv(24, "PRODUCT", C::Math, &[N], V::Number, "the product of the arguments"),
    sv(25, "SUMSQ", C::Math, &[N], V::Number, "the sum of the squares of the arguments"),
    sv(26, "AND", C::Logical, &[B], V::Boolean, "whether every argument is true"),
    sv(27, "OR", C::Logical, &[B], V::Boolean, "whether any argument is true"),
    sv(28, "XOR", C::Logical, &[B], V::Boolean, "whether an odd number of arguments are true"),
    // --- logical ---
    f(30, "IF", Kind::Lazy, C::Logical, &[B, A, A], 2, false, Ret::Unify, "then where the test is true, else (or no cell) where it is false"),
    f(31, "IFS", Kind::Lazy, C::Logical, &[B, A], 2, true, Ret::Unify, "the value paired with the first true test"),
    f(32, "SWITCH", Kind::Lazy, C::Logical, &[A, A, A], 3, true, Ret::Unify, "the result paired with the first value equal to the expression, else the default"),
    f(33, "CHOOSE", Kind::Lazy, C::Logical, &[N, A], 2, true, Ret::Unify, "the index-th value (1-based)"),
    f(34, "IFERROR", Kind::Lazy, C::Logical, &[A, A], 2, false, Ret::Unify, "the value, or the alternative where it is an error"),
    f(35, "IFNA", Kind::Lazy, C::Logical, &[A, A], 2, false, Ret::Unify, "the value, or the alternative where it is #N/A"),
    f(36, "CLEARERROR", Kind::Lazy, C::Logical, &[A, A], 1, false, Ret::Unify, "the value, or the alternative (or no cell) where it is an error"),
    f(37, "CLEARZERO", Kind::Lazy, C::Logical, &[N, N], 1, false, Ret::Fixed(V::Number), "the value, or the alternative (or no cell) where it is zero"),
    so(38, "BETWEEN", C::Logical, &[N, N, N, B], 3, V::Boolean, "whether low ≤ x ≤ high (strict when inclusive is FALSE)"),
    s(39, "TRUE", C::Logical, &[], V::Boolean, "TRUE"),
    s(40, "FALSE", C::Logical, &[], V::Boolean, "FALSE"),
    // --- information / errors ---
    s(50, "ISERROR", C::Information, &[A], V::Boolean, "whether the value is any error"),
    s(51, "ISERR", C::Information, &[A], V::Boolean, "whether the value is an error other than #N/A"),
    s(52, "ISNA", C::Information, &[A], V::Boolean, "whether the value is #N/A"),
    s(53, "NA", C::Information, &[], V::Number, "the #N/A error"),
    s(54, "ERROR", C::Information, &[T], V::Number, "the error a code names (\"#DIV/0!\")"),
    s(55, "ERROR.TYPE", C::Information, &[A], V::Number, "1 #NULL!, 2 #DIV/0!, 3 #VALUE!, 4 #REF!, 5 #NAME?, 6 #NUM!, 7 #N/A; #N/A for a non-error"),
    // --- math ---
    s(10, "ABS", C::Math, &[N], V::Number, "absolute value"),
    s(15, "NEG", C::Math, &[N], V::Number, "negation"),
    s(100, "SIGN", C::Math, &[N], V::Number, "−1, 0 or 1"),
    s(14, "SQRT", C::Math, &[N], V::Number, "square root (#NUM! below zero)"),
    s(101, "SQRTPI", C::Math, &[N], V::Number, "√(x·π)"),
    s(102, "EXP", C::Math, &[N], V::Number, "e to the power x"),
    s(103, "EXPM1", C::Math, &[N], V::Number, "eˣ − 1, accurate near 0"),
    s(104, "LN", C::Math, &[N], V::Number, "natural logarithm"),
    s(105, "LN1P", C::Math, &[N], V::Number, "ln(1 + x), accurate near 0"),
    so(106, "LOG", C::Math, &[N, N], 1, V::Number, "logarithm to a base (default 10)"),
    s(107, "LOG10", C::Math, &[N], V::Number, "base-10 logarithm"),
    s(108, "LOG2", C::Math, &[N], V::Number, "base-2 logarithm"),
    s(109, "POWER", C::Math, &[N, N], V::Number, "base to the power exponent (same as ^)"),
    s(110, "MOD", C::Math, &[N, N], V::Number, "remainder, with the divisor's sign"),
    s(111, "QUOTIENT", C::Math, &[N, N], V::Number, "integer part of a quotient"),
    s(112, "PI", C::Math, &[], V::Number, "π"),
    s(113, "DEGREES", C::Math, &[N], V::Number, "radians to degrees"),
    s(114, "RADIANS", C::Math, &[N], V::Number, "degrees to radians"),
    s(115, "SIN", C::Math, &[N], V::Number, "sine (radians)"),
    s(116, "COS", C::Math, &[N], V::Number, "cosine"),
    s(117, "TAN", C::Math, &[N], V::Number, "tangent"),
    s(118, "ASIN", C::Math, &[N], V::Number, "arcsine"),
    s(119, "ACOS", C::Math, &[N], V::Number, "arccosine"),
    s(120, "ATAN", C::Math, &[N], V::Number, "arctangent"),
    s(121, "ATAN2", C::Math, &[N, N], V::Number, "angle of the point (x, y)"),
    s(122, "SINH", C::Math, &[N], V::Number, "hyperbolic sine"),
    s(123, "COSH", C::Math, &[N], V::Number, "hyperbolic cosine"),
    s(124, "TANH", C::Math, &[N], V::Number, "hyperbolic tangent"),
    s(125, "ASINH", C::Math, &[N], V::Number, "inverse hyperbolic sine"),
    s(126, "ACOSH", C::Math, &[N], V::Number, "inverse hyperbolic cosine"),
    s(127, "ATANH", C::Math, &[N], V::Number, "inverse hyperbolic tangent"),
    s(128, "SEC", C::Math, &[N], V::Number, "secant"),
    s(129, "CSC", C::Math, &[N], V::Number, "cosecant"),
    s(130, "COT", C::Math, &[N], V::Number, "cotangent"),
    s(131, "SECH", C::Math, &[N], V::Number, "hyperbolic secant"),
    s(132, "CSCH", C::Math, &[N], V::Number, "hyperbolic cosecant"),
    s(133, "COTH", C::Math, &[N], V::Number, "hyperbolic cotangent"),
    s(134, "ACOT", C::Math, &[N], V::Number, "inverse cotangent"),
    s(135, "ACOTH", C::Math, &[N], V::Number, "inverse hyperbolic cotangent"),
    s(136, "SINPI", C::Math, &[N], V::Number, "sin(πx), exact at integers"),
    s(137, "COSPI", C::Math, &[N], V::Number, "cos(πx), exact at integers"),
    s(138, "TANPI", C::Math, &[N], V::Number, "tan(πx), exact at integers"),
    s(139, "COTPI", C::Math, &[N], V::Number, "cot(πx), exact at integers"),
    sv(140, "HYPOT", C::Math, &[N], V::Number, "√(Σ xᵢ²) without overflow"),
    s(141, "FACT", C::Math, &[N], V::Number, "factorial"),
    s(142, "FACTDOUBLE", C::Math, &[N], V::Number, "double factorial n!!"),
    s(143, "COMBIN", C::Math, &[N, N], V::Number, "combinations of n things k at a time"),
    s(144, "COMBINA", C::Math, &[N, N], V::Number, "combinations with repetition"),
    s(145, "PERMUT", C::Math, &[N, N], V::Number, "permutations of n things k at a time"),
    s(146, "PERMUTATIONA", C::Math, &[N, N], V::Number, "permutations with repetition"),
    sv(147, "GCD", C::Math, &[N], V::Number, "greatest common divisor"),
    sv(148, "LCM", C::Math, &[N], V::Number, "least common multiple"),
    sv(149, "MULTINOMIAL", C::Math, &[N], V::Number, "(Σ n)! / Π n!"),
    sv(150, "RAWSUBTRACT", C::Math, &[N], V::Number, "the first argument minus the rest"),
    sv(151, "INVSUMINV", C::Math, &[N], V::Number, "1 / Σ(1/xᵢ)"),
    // --- rounding ---
    so(11, "ROUND", C::Rounding, &[N, N], 1, V::Number, "to digits decimal places, halves away from zero"),
    so(200, "ROUNDUP", C::Rounding, &[N, N], 1, V::Number, "away from zero to digits places"),
    so(201, "ROUNDDOWN", C::Rounding, &[N, N], 1, V::Number, "toward zero to digits places"),
    s(202, "ROUNDSIG", C::Rounding, &[N, N], V::Number, "to digits significant digits"),
    so(203, "TRUNC", C::Rounding, &[N, N], 1, V::Number, "truncate to digits places"),
    s(204, "INT", C::Rounding, &[N], V::Number, "down to an integer"),
    so(12, "FLOOR", C::Rounding, &[N, N], 1, V::Number, "down to a multiple of significance (default 1)"),
    s(13, "CEIL", C::Rounding, &[N], V::Number, "up to an integer"),
    so(205, "CEILING", C::Rounding, &[N, N], 1, V::Number, "up to a multiple of significance (default 1)"),
    so(206, "FLOOR.MATH", C::Rounding, &[N, N, N], 1, V::Number, "down to a multiple; mode ≠ 0 rounds negatives toward zero"),
    so(207, "CEILING.MATH", C::Rounding, &[N, N, N], 1, V::Number, "up to a multiple; mode ≠ 0 rounds negatives away from zero"),
    so(208, "FLOOR.PRECISE", C::Rounding, &[N, N], 1, V::Number, "down to a multiple, ignoring the multiple's sign"),
    so(209, "CEILING.PRECISE", C::Rounding, &[N, N], 1, V::Number, "up to a multiple, ignoring the multiple's sign"),
    so(210, "ISO.CEILING", C::Rounding, &[N, N], 1, V::Number, "same as CEILING.PRECISE"),
    s(211, "MROUND", C::Rounding, &[N, N], V::Number, "to the nearest multiple"),
    s(212, "EVEN", C::Rounding, &[N], V::Number, "away from zero to an even integer"),
    s(213, "ODD", C::Rounding, &[N], V::Number, "away from zero to an odd integer"),
    // --- text ---
    s(300, "CHAR", C::Text, &[N], V::Text, "the character with code n (1–255)"),
    s(301, "UNICHAR", C::Text, &[N], V::Text, "the Unicode character n"),
    s(302, "CODE", C::Text, &[T], V::Number, "the code of the first character"),
    s(303, "UNICODE", C::Text, &[T], V::Number, "the code point of the first character"),
    s(304, "CLEAN", C::Text, &[T], V::Text, "remove non-printing characters"),
    sv(305, "CONCAT", C::Text, &[A], V::Text, "join values as text (same as &)"),
    sv(306, "CONCATENATE", C::Text, &[A], V::Text, "join values as text"),
    s(307, "EXACT", C::Text, &[T, T], V::Boolean, "case-sensitive equality"),
    so(308, "FIND", C::Text, &[T, T, N], 2, V::Number, "1-based position of find in text, case-sensitive"),
    so(309, "SEARCH", C::Text, &[T, T, N], 2, V::Number, "1-based position, case-insensitive, with * and ? wildcards"),
    so(310, "LEFT", C::Text, &[T, N], 1, V::Text, "the first n characters (default 1)"),
    so(311, "RIGHT", C::Text, &[T, N], 1, V::Text, "the last n characters (default 1)"),
    s(312, "MID", C::Text, &[T, N, N], V::Text, "n characters from a 1-based start"),
    s(313, "LEN", C::Text, &[T], V::Number, "the number of characters"),
    s(314, "LOWER", C::Text, &[T], V::Text, "lowercase"),
    s(315, "UPPER", C::Text, &[T], V::Text, "uppercase"),
    s(316, "PROPER", C::Text, &[T], V::Text, "capitalize each word"),
    s(317, "REPLACE", C::Text, &[T, N, N, T], V::Text, "replace n characters from a 1-based start"),
    s(318, "REPT", C::Text, &[T, N], V::Text, "repeat text n times"),
    so(319, "SUBSTITUTE", C::Text, &[T, T, T, N], 3, V::Text, "replace every (or the instance-th) occurrence of old"),
    s(320, "TRIM", C::Text, &[T], V::Text, "remove leading, trailing and repeated spaces"),
    s(321, "VALUE", C::Text, &[T], V::Number, "text as a number"),
    so(322, "TEXTBEFORE", C::Text, &[T, T, N, N, N, A], 2, V::Text, "text before the instance-th delimiter"),
    so(323, "TEXTAFTER", C::Text, &[T, T, N, N, N, A], 2, V::Text, "text after the instance-th delimiter"),
    s(324, "TEXTBETWEEN", C::Text, &[T, T, T], V::Text, "text between the first start delimiter and the next end delimiter"),
    s(325, "COUNTMATCHES", C::Text, &[T, T], V::Number, "how many times search occurs in text"),
];

/// The reserved id of a `WHERE` clause: an aggregation's trailing argument
/// `Call(WHERE, [predicate])` (`SUM(x OVER T WHERE x > 0)`). Not a callable
/// function, so not in the registry.
pub const WHERE: FuncId = FuncId(990);
/// The reserved id of a window: an aggregation's trailing argument
/// `Call(WINDOW, [from, to])` (`SUM(x OVER T[FIRST..THIS])`). Each bound is a
/// number (an offset from this item; `0` is THIS) or the text `FIRST` /
/// `LAST`.
pub const WINDOW: FuncId = FuncId(991);
/// The reserved id of an `INTO Category` clause: an array function's
/// trailing argument `Call(INTO, [Category(K)])` (`SORT(x OVER T INTO K)`).
pub const INTO: FuncId = FuncId(992);

/// The first function id of a named LAMBDA call: lambda `id` is called as
/// `FuncId(NAMED_LAMBDA_BASE + id)`. Above every built-in.
pub const NAMED_LAMBDA_BASE: u32 = 1_000_000;

/// The function id a call of named LAMBDA `id` is stored as.
pub fn named_lambda(id: u32) -> FuncId {
    FuncId(NAMED_LAMBDA_BASE.saturating_add(id))
}

/// The named LAMBDA a function id calls, if it is one.
pub fn named_lambda_of(f: FuncId) -> Option<u32> {
    f.0.checked_sub(NAMED_LAMBDA_BASE)
}

/// The reference standing for parameter `i` in a named LAMBDA's body.
pub fn lambda_param(i: usize) -> crate::MeasureId {
    crate::MeasureId(LAMBDA_PARAMS - i as u32)
}

/// The highest parameter placeholder id (below the fold placeholders).
const LAMBDA_PARAMS: u32 = LAMBDA_SLOTS - 2048;

/// The parameter index of a placeholder id (see [`lambda_param`]).
pub fn lambda_param_of(id: crate::MeasureId) -> Option<usize> {
    let d = LAMBDA_PARAMS.checked_sub(id.0)?;
    (d < 256).then_some(d as usize)
}

/// `POSITION(T)`: an item's 1-based position along its category.
pub const POSITION: FuncId = FuncId(900);

/// The placeholder measure id for parameter `i` (0 the accumulator, 1 the
/// value) of the `k`-th `REDUCE`/`SCAN` lambda in a formula. Placeholders
/// sit at the top of the id space, above any real measure.
pub fn lambda_slot(k: u32, i: u32) -> crate::MeasureId {
    crate::MeasureId(LAMBDA_SLOTS - 2 * k - i)
}

/// The highest placeholder id; ids above `LAMBDA_SLOTS - 2048` are reserved.
pub const LAMBDA_SLOTS: u32 = u32::MAX - 4096;

/// The `(k, i)` of a placeholder id (see [`lambda_slot`]).
pub fn lambda_slot_of(id: crate::MeasureId) -> Option<(u32, u32)> {
    let d = LAMBDA_SLOTS.checked_sub(id.0)?;
    (d < 2048).then_some((d / 2, d % 2))
}

mod analysis;
mod array;
mod date;
mod dimensional;
mod dist_extra;
mod engineering;
mod financial;
mod financial_reduce;
mod lookup;
mod math_extra;
mod options;
mod params;
mod statistical;
mod statistical_reduce;

/// Every family table, in documentation order.
static FAMILIES: &[&[FnDef]] = &[
    CORE,
    date::DATE,
    dimensional::DIMENSIONAL,
    array::ARRAY,
    statistical::STATISTICAL,
    statistical_reduce::STATISTICAL_REDUCE,
    financial::FINANCIAL,
    financial_reduce::FINANCIAL_REDUCE,
    lookup::LOOKUP,
    engineering::ENGINEERING,
    math_extra::MATH_EXTRA,
    dist_extra::DIST_EXTRA,
    options::OPTIONS,
    analysis::ANALYSIS,
];

/// Every built-in function.
pub fn all() -> impl Iterator<Item = &'static FnDef> {
    FAMILIES.iter().flat_map(|f| f.iter())
}

/// The function with id `id`.
pub fn by_id(id: FuncId) -> Option<&'static FnDef> {
    all().find(|d| d.id == id)
}

/// The functions a name (canonical or alias, case-insensitive) can mean: an
/// aggregation and its scalar twin share a name (`SUM(x OVER T)` vs
/// `SUM(a, b)`), so a name maps to at most one of each [`Kind`].
pub fn by_name(name: &str) -> impl Iterator<Item = &'static FnDef> + '_ {
    all().filter(move |d| {
        d.name.eq_ignore_ascii_case(name) || d.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    })
}

impl Kind {
    /// Whether this kind reduces over `OVER` categories.
    pub fn reduces(self) -> bool {
        matches!(self, Kind::Aggregate | Kind::PerItem | Kind::Array)
    }
}

/// `x, y OVER R, C, [z] INTO K` as its arguments: the categories listed
/// after an OVER or INTO (capitalized) stay with their argument.
fn split_params(list: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in list.split(", ") {
        match out.last_mut() {
            Some(last)
                if (last.contains(" OVER ") || last.contains(" INTO "))
                    && p.starts_with(|c: char| c.is_ascii_uppercase()) =>
            {
                last.push_str(", ");
                last.push_str(p);
            }
            _ => out.push(p.to_string()),
        }
    }
    out
}

/// The parameter list a description spells as `NAME(a, b, [c])`, if it
/// does (and the names look like names, not an example call).
fn spelled_params(name: &str, doc: &str) -> Option<Vec<String>> {
    let at = doc.find(&format!("{name}("))? + name.len() + 1;
    let mut depth = 0usize;
    let mut end = None;
    for (i, c) in doc[at..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' if depth == 0 => {
                end = Some(at + i);
                break;
            }
            ')' => depth -= 1,
            _ => {}
        }
    }
    let inner = &doc[at..end?];
    let parts: Vec<String> = split_params(inner)
        .iter()
        .map(|p| p.trim().to_string())
        .collect();
    let word = |p: &str| {
        // The name before any OVER / INTO categories.
        let p = p.split(" OVER ").next().unwrap_or(p);
        let p = p.split(" INTO ").next().unwrap_or(p);
        let p = p.trim_start_matches('[').trim_end_matches(']');
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_alphanumeric() || "_ …=.-".contains(c))
            && !p.starts_with(|c: char| c.is_ascii_digit() || c == '-')
    };
    (!inner.is_empty() && parts.iter().all(|p| word(p))).then_some(parts)
}

/// Functions matching `query`, best first: an exact name or alias, then a
/// prefix, then a word of the name or description. Case-insensitive; an
/// empty query lists everything by name. The help browser's search and
/// the formula editor's completion both use this.
pub fn search(query: &str) -> Vec<&'static FnDef> {
    let q = query.trim().to_ascii_uppercase();
    let mut hits: Vec<(u8, &'static FnDef)> = all()
        .filter_map(|d| {
            let names = std::iter::once(d.name).chain(d.aliases.iter().copied());
            let rank = if q.is_empty() {
                Some(3)
            } else if names.clone().any(|n| n == q) {
                Some(0)
            } else if names.clone().any(|n| n.starts_with(&q)) {
                Some(1)
            } else if names.clone().any(|n| n.contains(&q)) {
                Some(2)
            } else if d.doc.to_ascii_uppercase().contains(&q) {
                Some(4)
            } else {
                None
            };
            rank.map(|r| (r, d))
        })
        .collect();
    hits.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.name.cmp(b.1.name))
            .then(a.1.id.cmp(&b.1.id))
    });
    // One row per name: an aggregation and its scalar twin share a name.
    let mut seen = std::collections::HashSet::new();
    hits.into_iter()
        .map(|(_, d)| d)
        .filter(|d| seen.insert(d.name))
        .collect()
}

/// Up to `n` function names closest to `word` by edit distance (a typo's
/// likely meaning), nearest first; only those within a third of its
/// length, at least 2.
pub fn closest(word: &str, n: usize) -> Vec<&'static str> {
    let w = word.to_ascii_uppercase();
    let limit = (w.chars().count() / 3).max(2);
    let mut scored: Vec<(usize, &'static str)> = all()
        .flat_map(|d| std::iter::once(d.name).chain(d.aliases.iter().copied()))
        .map(|name| (edit_distance(&w, name), name))
        .filter(|(dist, _)| *dist <= limit)
        .collect();
    scored.sort();
    scored.dedup_by(|a, b| a.1 == b.1);
    scored.into_iter().take(n).map(|(_, name)| name).collect()
}

/// Levenshtein distance over chars.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

/// Function names (canonical and aliases) starting with `prefix`
/// (case-insensitive), canonical first, for completion.
pub fn complete(prefix: &str) -> Vec<(&'static str, &'static FnDef)> {
    let p = prefix.to_ascii_uppercase();
    if p.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<(&'static str, &'static FnDef)> = Vec::new();
    for d in all() {
        for n in std::iter::once(d.name).chain(d.aliases.iter().copied()) {
            if n.starts_with(&p) && !out.iter().any(|(m, _)| *m == n) {
                out.push((n, d));
            }
        }
    }
    out.sort_by(|a, b| a.0.len().cmp(&b.0.len()).then(a.0.cmp(b.0)));
    out
}

/// Where the caret sits in a call: the innermost unclosed `NAME(` before
/// byte offset `caret` in `text`, and the 0-based argument it is in.
/// Strings and nested parentheses are skipped. Signature help uses it.
pub fn call_at(text: &str, caret: usize) -> Option<(&'static FnDef, usize)> {
    let before = text.get(..caret)?;
    // (name end, argument index) per open parenthesis.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut in_str = false;
    for (i, c) in before.char_indices() {
        match c {
            '"' => in_str = !in_str,
            _ if in_str => {}
            '(' => stack.push((i, 0)),
            ')' => {
                stack.pop();
            }
            ',' | ';' => {
                if let Some(top) = stack.last_mut() {
                    top.1 += 1;
                }
            }
            _ => {}
        }
    }
    let (open, arg) = *stack.last()?;
    let head = before[..open].trim_end();
    let start = head
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '@'))
        .map_or(0, |i| {
            i + head[i..].chars().next().map_or(1, char::len_utf8)
        });
    let name = &head[start..];
    let def = by_name(name).next()?;
    Some((def, arg))
}

/// The aggregation (or per-item reduction) a name means, if any.
pub fn aggregation(name: &str) -> Option<&'static FnDef> {
    by_name(name).find(|d| d.kind.reduces())
}

/// The non-aggregating function a name means, if any.
pub fn callable(name: &str) -> Option<&'static FnDef> {
    by_name(name).find(|d| !d.kind.reduces())
}

/// Whether `id` reduces over `OVER` categories.
pub fn is_aggregation(id: FuncId) -> bool {
    by_id(id).is_some_and(|d| d.kind.reduces())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    /// Search ranks exact names, then prefixes, then substrings, then the
    /// descriptions; completion lists prefixes, shortest first; call_at
    /// finds the innermost open call and the argument the caret is in.
    #[test]
    fn search_complete_and_call_at() {
        let names = |q: &str| search(q).iter().map(|d| d.name).take(3).collect::<Vec<_>>();
        assert_eq!(names("pmt")[0], "PMT");
        assert_eq!(names("ROUNDU"), ["ROUNDUP"]);
        assert!(
            search("payment").iter().any(|d| d.name == "PMT"),
            "found by description"
        );
        assert_eq!(
            search("").len(),
            search("")
                .iter()
                .map(|d| d.name)
                .collect::<HashSet<_>>()
                .len()
        );
        assert!(search("zzzz-not-a-function").is_empty());
        assert!(search("DSUM").iter().any(|d| d.name == "SUM"), "by alias");
        let c: Vec<&str> = complete("roun").iter().map(|(n, _)| *n).collect();
        assert_eq!(c[0], "ROUND");
        assert!(c.contains(&"ROUNDUP"));
        assert!(complete("").is_empty());
        assert!(complete("DSU")
            .iter()
            .any(|(n, d)| *n == "DSUM" && d.name == "SUM"));
        let at = |t: &str| call_at(t, t.len()).map(|(d, i)| (d.name, i));
        assert_eq!(at("PMT("), Some(("PMT", 0)));
        assert_eq!(at("PMT(0.05, 12, "), Some(("PMT", 2)));
        assert_eq!(at("ROUND(PMT(0.05, 12, 100), "), Some(("ROUND", 1)));
        assert_eq!(
            at("ROUND(LEFT(\"a,b\", "),
            Some(("LEFT", 1)),
            "commas in strings skipped"
        );
        assert_eq!(
            at("NORM.S.DIST(1; "),
            Some(("NORM.S.DIST", 1)),
            "dotted name, ; separator"
        );
        assert_eq!(at("PMT(1)"), None, "closed");
        assert_eq!(at("(1 + "), None, "not a call");
        assert_eq!(at("Nope("), None);
        assert!(call_at("PMT(", 99).is_none());
        // Hints: spelled in the description, from the table, or the types.
        let hint = |n: &str| by_name(n).next().unwrap().hint();
        assert_eq!(hint("PMT"), "PMT(rate, nper, pv, [fv], [type])");
        assert_eq!(
            hint("IF"),
            "IF(logical_test, value_if_true, [value_if_false])"
        );
        assert_eq!(callable("SUM").unwrap().hint(), "SUM(number1, …)");
        assert_eq!(aggregation("SUM").unwrap().hint(), "SUM(values OVER T)");
        assert_eq!(
            hint("MINVERSE"),
            "MINVERSE(A OVER R, C INTO R2, C2)",
            "categories stay with their argument"
        );
        assert_eq!(
            spelled_params("X", "X(a, b OVER T, [c] INTO K)").map(|v| v.len()),
            Some(3)
        );
        assert_eq!(hint("PI"), "PI()");
        assert_eq!(closest("PMTT", 3)[0], "PMT");
        assert_eq!(closest("xlookp", 1), ["XLOOKUP"]);
        assert!(closest("qqqqqqqqqqqq", 3).is_empty());
        assert_eq!(edit_distance("kitten", "sitting"), 3);
        assert_eq!(edit_distance("", "ab"), 2);
        assert!(
            spelled_params("X", "X(1, 2)").is_none(),
            "an example call is not names"
        );
        assert!(spelled_params("X", "no call here").is_none());
        assert!(spelled_params("X", "X(a, (b)").is_none(), "unclosed");
    }

    /// Dump every hint (run with `--ignored --nocapture` to review them):
    /// `T` where the names fall back to the types, else `N`.
    #[test]
    #[ignore]
    fn dump_hints() {
        for d in super::all() {
            let generic = !d.params.is_empty() && d.named_params().is_none();
            println!("{} {}", if generic { "T" } else { "N" }, d.hint());
        }
    }

    /// Every function with parameters names them (in the table or its
    /// description), one name per parameter: optional ones bracketed, a
    /// variadic tail marked by one `…` item (`value1, …`, `x1, … OVER T`).
    #[test]
    fn every_function_names_its_parameters() {
        for d in super::all() {
            let names = d.param_names();
            assert!(
                d.params.is_empty() || d.named_params().is_some(),
                "{} falls back to its types: add it to functions/params.rs",
                d.hint()
            );
            let tail = names.iter().filter(|p| p.starts_with('…')).count();
            assert_eq!(
                tail,
                usize::from(d.variadic),
                "{}: one … iff variadic",
                d.hint()
            );
            // SWITCH's [default] follows its repeated value/result pairs.
            let extra = usize::from(d.name == "SWITCH");
            assert_eq!(
                names.len() - tail,
                d.params.len() + extra,
                "{}: one name per parameter",
                d.hint()
            );
            for (i, p) in names.iter().filter(|p| !p.starts_with('…')).enumerate() {
                assert_eq!(p.starts_with('['), i >= d.required, "{}: {p}", d.hint());
            }
            if d.kind.reduces() {
                assert!(
                    names.iter().any(|p| p.contains(" OVER ")),
                    "{}: which argument is reduced",
                    d.hint()
                );
            }
            assert_eq!(
                names.last().is_some_and(|p| p.contains(" INTO ")),
                d.kind == Kind::Array,
                "{}: a list result ends in INTO",
                d.hint()
            );
        }
    }

    use super::*;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn ids_are_unique() {
        let mut seen = HashSet::new();
        for d in all() {
            assert!(seen.insert(d.id), "{} reuses id {:?}", d.name, d.id);
        }
    }

    /// Each spelling names at most one aggregation and one other function.
    #[test]
    fn names_are_unambiguous() {
        let mut seen: HashMap<(String, bool), &str> = HashMap::new();
        for d in all() {
            for n in std::iter::once(&d.name).chain(d.aliases) {
                let k = (n.to_ascii_uppercase(), d.kind.reduces());
                if let Some(prev) = seen.insert(k, d.name) {
                    panic!("{n} names both {prev} and {}", d.name);
                }
            }
        }
    }

    /// Persisted ids keep their meaning (stored formulas depend on these).
    #[test]
    fn frozen_ids() {
        for (id, name) in [
            (1, "SUM"),
            (2, "AVERAGE"),
            (3, "MIN"),
            (4, "MAX"),
            (10, "ABS"),
            (11, "ROUND"),
            (12, "FLOOR"),
            (13, "CEIL"),
            (14, "SQRT"),
            (15, "NEG"),
            (20, "MIN"),
            (21, "MAX"),
        ] {
            assert_eq!(by_id(FuncId(id)).map(|d| d.name), Some(name), "{id}");
        }
    }

    #[test]
    fn lookups() {
        assert_eq!(aggregation("avg").map(|d| d.id), Some(FuncId(2)));
        assert_eq!(callable("sum").map(|d| d.id), Some(FuncId(22)));
        assert_eq!(callable("min2").map(|d| d.id), Some(FuncId(20)));
        assert_eq!(callable("Floor.Math").map(|d| d.id), Some(FuncId(206)));
        assert!(callable("nope").is_none());
        assert!(is_aggregation(FuncId(7)) && !is_aggregation(FuncId(26)));
        let round = by_id(FuncId(11)).unwrap();
        assert!(round.accepts(1) && round.accepts(2) && !round.accepts(3) && !round.accepts(0));
        let sum = callable("SUM").unwrap();
        assert!(sum.accepts(5) && !sum.accepts(0));
        assert_eq!(sum.param(4), Arg::Number);
        assert_eq!(round.signature(), "ROUND(number, [number])");
        assert_eq!(sum.signature(), "SUM(number, …)");
        assert_eq!(by_id(FuncId(39)).unwrap().param(0), Arg::Any);
    }
}

/// The function reference page (`docs/src/functions.md`), generated from
/// [`all`] so the docs cannot drift from the code. A test compares it
/// with the committed file; `UPDATE_DOCS=1 cargo test -p improv_core_model`
/// rewrites it.
pub fn reference_markdown() -> String {
    let mut out = String::from(
        "# Function Reference\n\n\
         <!-- Generated from crates/core_model/src/functions.rs by\n     \
         `UPDATE_DOCS=1 cargo test -p improv_core_model`. Do not edit. -->\n\n\
         Every built-in function, by category. Names are case-insensitive;\n\
         `[x]` is optional and `…` repeats the last argument. Positions are\n\
         1-based. An error argument (`#DIV/0!`, `#N/A`, …) makes the result\n\
         that error, except in the functions that test for errors. See\n\
         [The Formula Language](./formulas.md) for operators, types and\n\
         dimensions.\n",
    );
    let cats = [
        (Category::Aggregation, "Aggregations", "Written `F(Measure OVER Category)`: collapse one category. Each also has a scalar form over its arguments, listed under its category."),
        (Category::Logical, "Logical", "`IF` and the functions built on it evaluate only the branch they choose, so an error or missing cell in the other branch does not matter."),
        (Category::Information, "Information and errors", ""),
        (Category::Math, "Math and trigonometry", "Angles are in radians."),
        (Category::Rounding, "Rounding", "Rounding is decimal: `ROUND(2.675, 2)` is 2.68."),
        (Category::Text, "Text", "Lengths and positions count characters, not bytes."),
        (Category::DateTime, "Date and time", "Dates are date-time values (UTC, millisecond precision). A number given where a date is wanted is a spreadsheet serial date (1 = 1900-01-01)."),
        (Category::Dimensional, "Dimensional", "Functions of a cell's position in a category, and the ordered (previous/next, running) forms."),
        (Category::Statistical, "Statistical", ""),
        (Category::Financial, "Financial", "Rates are per period; cash flows are in item order along their `OVER` category."),
        (Category::Engineering, "Engineering", "Complex numbers are text, `\"3+4i\"` (suffix `i` or `j`); base conversions use 10-digit two's complement for negatives."),
    ];
    for (cat, title, intro) in cats {
        if all().all(|d| d.category != cat) {
            continue;
        }
        out.push_str(&format!("\n## {title}\n\n"));
        if !intro.is_empty() {
            out.push_str(intro);
            out.push_str("\n\n");
        }
        out.push_str("| Function | Returns | Description |\n|---|---|---|\n");
        for d in all().filter(|d| d.category == cat) {
            let sig = if d.kind.reduces() && d.params.len() == 1 {
                format!("{}(Measure OVER Category)", d.name)
            } else {
                d.signature()
            };
            let ret = match d.ret {
                Ret::Fixed(ValueType::Number) => "number",
                Ret::Fixed(ValueType::Boolean) => "boolean",
                Ret::Fixed(ValueType::Text) => "text",
                Ret::Fixed(ValueType::DateTime) => "date",
                Ret::Fixed(_) => "value",
                Ret::Unify => "the branches' type",
                Ret::Arg(_) => "the values' type",
            };
            let mut doc = d.doc.to_string();
            if !d.aliases.is_empty() {
                doc.push_str(&format!(" (also `{}`)", d.aliases.join("`, `")));
            }
            out.push_str(&format!("| `{sig}` | {ret} | {doc} |\n"));
        }
    }
    out
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod docs_test {
    #[test]
    fn reference_page_is_current() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/src/functions.md");
        let want = super::reference_markdown();
        if std::env::var_os("UPDATE_DOCS").is_some() {
            std::fs::write(path, &want).unwrap();
        }
        // A Windows checkout may have turned LF into CRLF.
        let have = std::fs::read_to_string(path)
            .unwrap_or_default()
            .replace("\r\n", "\n");
        assert!(
            have == want,
            "docs/src/functions.md is stale: run UPDATE_DOCS=1 cargo test -p improv_core_model"
        );
    }
}
