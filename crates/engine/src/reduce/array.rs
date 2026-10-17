//! The array reductions (P6): functions whose result is a list, laid along
//! the `INTO` category (`SORT(x OVER T INTO Rank)`: the smallest value at
//! Rank's first item). Ids 1250–1299.

use super::Rows;
use crate::builtins::err;
use crate::CellValue;

/// Evaluate array function `id` over a group's rows.
pub(super) fn array(id: u32, rows: &Rows, params: &[CellValue]) -> Vec<CellValue> {
    if let Some(bad) = rows.first_error() {
        return vec![bad];
    }
    let vals: Vec<CellValue> = rows.col(0).cloned().collect();
    match id {
        // SORT(x OVER T, [order = 1]): ascending, or descending for -1.
        1250 => {
            let desc = matches!(params.first().and_then(CellValue::as_num), Some(o) if o < 0.0);
            let mut v = vals;
            v.sort_by(order);
            if desc {
                v.reverse();
            }
            v
        }
        // UNIQUE(x OVER T, [exactly_once = FALSE]): first occurrences in item
        // order (or only the values that occur once).
        1251 => {
            let once = matches!(params.first(), Some(CellValue::Bool(true)));
            let mut out: Vec<CellValue> = Vec::new();
            for v in &vals {
                let n = vals.iter().filter(|w| *w == v).count();
                if !out.contains(v) && (!once || n == 1) {
                    out.push(v.clone());
                }
            }
            out
        }
        _ => vec![CellValue::Err(err::NAME)],
    }
}

/// Spreadsheet sort order: numbers (by value), then text (case-insensitive),
/// then booleans (FALSE < TRUE); dates sort with numbers.
pub(super) fn order(a: &CellValue, b: &CellValue) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let rank = |v: &CellValue| match v {
        CellValue::Num(_) | CellValue::Date(_) => 0,
        CellValue::Text(_) => 1,
        CellValue::Bool(_) => 2,
        CellValue::Err(_) => 3,
    };
    let num = |v: &CellValue| match v {
        CellValue::Date(ms) => Some(*ms as f64),
        _ => v.as_num(),
    };
    rank(a).cmp(&rank(b)).then_with(|| match (a, b) {
        (CellValue::Text(x), CellValue::Text(y)) => x.to_lowercase().cmp(&y.to_lowercase()),
        (CellValue::Bool(x), CellValue::Bool(y)) => x.cmp(y),
        _ => num(a)
            .zip(num(b))
            .and_then(|(x, y)| x.partial_cmp(&y))
            .unwrap_or(Ordering::Equal),
    })
}
