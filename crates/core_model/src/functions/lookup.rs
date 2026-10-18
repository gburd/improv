//! The lookups and database functions (P6): reductions whose key is a
//! parameter. `XLOOKUP(Grade, GradeKey OVER Band, GradeRate)` searches the
//! table along Band for each cell of the key parameter; a key ranging over
//! categories the table lacks (Grade per Employee) gives a result over them.
//! Ids 1200–1249.

use super::{alias, f, over, Arg, Category, FnDef, Kind, Over, Ret};
use crate::value::ValueType;

use Arg::{Any as A, Boolean as B, Number as N};
use Category::{Dimensional as Dm, Statistical as St, Text as Tx};

const NUM: Ret = Ret::Fixed(ValueType::Number);

/// A one-result reduction (`Kind::Aggregate`).
#[allow(
    clippy::too_many_arguments,
    reason = "a table row: one argument per FnDef field"
)]
#[cfg_attr(coverage_nightly, coverage(off))]
const fn r(
    id: u32,
    name: &'static str,
    cat: Category,
    params: &'static [Arg],
    required: usize,
    ret: Ret,
    reduced: &'static [usize],
    doc: &'static str,
) -> FnDef {
    over(
        f(
            id,
            name,
            Kind::Aggregate,
            cat,
            params,
            required,
            false,
            ret,
            doc,
        ),
        Over::At(reduced),
    )
}

#[rustfmt::skip]
pub(super) static LOOKUP: &[FnDef] = &[
    // --- lookups (§5.22) ---
    r(1200, "XLOOKUP", Dm, &[A, A, A, A, N, N], 3, Ret::Arg(2), &[1, 2], "return_array at the item where lookup_array matches lookup_value (match_mode 0 exact, -1 next smaller, 1 next larger, 2 wildcard; search_mode 1 first to last, -1 last to first, ±2 the same as ±1); if_not_found (else #N/A) when none does"),
    r(1201, "XMATCH", Dm, &[A, A, N, N], 2, NUM, &[1], "the 1-based item position along the category where lookup_array matches lookup_value (XLOOKUP's modes); #N/A when none does"),
    r(1202, "MATCH", Dm, &[A, A, N], 2, NUM, &[1], "the 1-based item position of lookup_value: match_type 1 (default) the largest ≤ it, 0 the first equal (text with * and ? wildcards), -1 the smallest ≥ it; #N/A when none"),
    r(1203, "LOOKUP", Dm, &[A, A, A], 3, Ret::Arg(2), &[1, 2], "result_vector at the largest lookup_vector ≤ lookup_value (ascending); #N/A when none"),
    alias(r(1204, "INDEX", Dm, &[A, N], 2, Ret::Unify, &[0], "the value at the row_num-th item (1-based); #REF! out of range, #N/A where that item has no cell"), &["VALUEAT"]),
    // --- Quantrix selection (§5.26). SELECTLIST is SELECT with a lookup
    // measure over its own category (one result per lookup); SELECTS is a
    // list function (array.rs) ---
    alias(r(1210, "SELECT", Dm, &[A, A, A], 3, Ret::Unify, &[0, 1], "values at the first item (in item order) whose key equals lookup; #N/A when none (SELECTLIST: a lookup over its own category gives one result per lookup)"), &["SELECTLIST"]),
    r(1211, "SELECTBETWEEN", Dm, &[A, A, A, A, B], 4, Ret::Unify, &[0, 1], "values at the first item whose key is between low and high (strictly when inclusive is FALSE); #N/A when none"),
    r(1212, "SELECTGREATERTHAN", Dm, &[A, A, A, B], 3, Ret::Unify, &[0, 1], "values at the first item whose key is > lookup (≥ when inclusive is TRUE); #N/A when none"),
    r(1213, "SELECTLESSTHAN", Dm, &[A, A, A, B], 3, Ret::Unify, &[0, 1], "values at the first item whose key is < lookup (≤ when inclusive is TRUE); #N/A when none"),
    // --- database (§5.24): the rest are aliases of SUM, COUNT, … ---
    r(1220, "DGET", St, &[A], 1, Ret::Unify, &[0], "the single value (with WHERE, the single match); #NUM! when more than one (a WHERE that keeps none leaves no cell)"),
    // --- text of a list ---
    r(1230, "ARRAYTOTEXT", Tx, &[A, N], 1, Ret::Fixed(ValueType::Text), &[0], "the values as text in item order: format 0 \"1, 2, a\", format 1 {1,2,\"a\"}"),
];
