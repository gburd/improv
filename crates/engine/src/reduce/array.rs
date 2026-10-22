//! The array reductions (P6): functions whose result is a list, laid along
//! the `INTO` category (`SORT(x OVER T INTO Rank)`: the smallest value at
//! Rank's first item). Ids 1250–1299.
//!
//! Excel's empty array (`#CALC!`) has no Improv error: an empty result
//! (`FILTER` with nothing included and no `if_empty`, `TAKE` of 0, `DROP` of
//! everything, `TEXTSPLIT` of nothing but delimiters) is `#N/A`.
//!
//! An error among the rows is the result (the first in item order; every
//! entry of `LINEST`'s and `LOGEST`'s block), except that `FILTER`,
//! `SORTBY`, `TAKE` and `DROP` move x's values as they are, errors
//! included; only an error in `FILTER`'s include is the result.

use super::Rows;
use crate::builtins::{b, err, fold_char, n, nums, t, text_of};
use crate::CellValue;
use std::collections::HashMap;

mod fit;

type List = Result<Vec<CellValue>, u8>;

/// Evaluate array function `id` over a group's rows.
pub(super) fn array(id: u32, rows: &Rows, params: &[CellValue]) -> Vec<CellValue> {
    let out = match id {
        1252 => filter(rows, params),
        1253 => sortby(rows, params),
        1254 | 1255 => take(rows, params, id == 1255),
        1261 => selects(rows, params),
        _ => match rows.first_error() {
            // LINEST's block has its shape whatever the values (as in
            // LibreOffice): every entry is the error.
            Some(bad) if matches!(id, 1257 | 1258) => return vec![bad; fit::len(params, xs(rows))],
            Some(bad) => return vec![bad],
            None => of_values(id, rows, params),
        },
    };
    out.unwrap_or_else(|c| vec![CellValue::Err(c)])
}

/// How many x columns a LINEST/LOGEST row carries (after y).
fn xs(rows: &Rows) -> usize {
    rows.vals.iter().map(Vec::len).max().unwrap_or(2).max(2) - 1
}

/// The functions of error-free rows.
fn of_values(id: u32, rows: &Rows, params: &[CellValue]) -> List {
    let vals: Vec<CellValue> = rows.col(0).cloned().collect();
    Ok(match id {
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
            // Text ignores case, as in Excel; the first spelling is kept.
            let key = |v: &CellValue| match v {
                CellValue::Text(s) => CellValue::Text(s.chars().map(fold_char).collect()),
                v => v.clone(),
            };
            let keys: Vec<CellValue> = vals.iter().map(key).collect();
            let once = matches!(params.first(), Some(CellValue::Bool(true)));
            let mut out: Vec<CellValue> = Vec::new();
            let mut seen: Vec<&CellValue> = Vec::new();
            for (v, k) in vals.iter().zip(&keys) {
                let n = keys.iter().filter(|w| *w == k).count();
                if !seen.contains(&k) && (!once || n == 1) {
                    out.push(v.clone());
                }
                seen.push(k);
            }
            if out.is_empty() {
                // Every value repeats (exactly_once): the empty list.
                return Err(err::NA);
            }
            out
        }
        1256 => mode_mult(&vals)?,
        1257 | 1258 => fit::linest(rows, params, id == 1258, xs(rows))?,
        1259 => textsplit(&vals, params)?,
        1260 => frequency(&vals, params)?,
        _ => return Err(err::NAME),
    })
}

/// FILTER(x, include OVER T, [if_empty]): the x values whose include is
/// TRUE, in item order.
fn filter(rows: &Rows, params: &[CellValue]) -> List {
    let mut out = Vec::new();
    for r in &rows.vals {
        let keep = match r.get(1) {
            Some(CellValue::Err(c)) => return Err(*c),
            Some(v) => b(std::slice::from_ref(v), 0)?,
            None => false,
        };
        if keep {
            out.extend(r.first().cloned());
        }
    }
    if out.is_empty() {
        return params.first().map(|v| vec![v.clone()]).ok_or(err::NA);
    }
    Ok(out)
}

/// SORTBY(x, by1 OVER T, [order1 = 1], [by2, order2], …): x in the
/// (stable) order of by1, ties broken by by2, …; a key descends when its
/// order is -1. Reduced columns are x, by1, by2, …; `params` the orders.
fn sortby(rows: &Rows, params: &[CellValue]) -> List {
    let keys = rows.vals.iter().map(Vec::len).max().unwrap_or(2).max(2) - 1;
    let desc = (0..keys)
        .map(|k| match params.get(k) {
            None => Ok(false),
            Some(_) => match n(params, k)? {
                1.0 => Ok(false),
                -1.0 => Ok(true),
                _ => Err(err::VALUE),
            },
        })
        .collect::<Result<Vec<bool>, u8>>()?;
    let mut rs: Vec<&Vec<CellValue>> = rows.vals.iter().filter(|r| r.len() > keys).collect();
    rs.sort_by(|a, b| {
        (0..keys)
            .map(|k| {
                let o = order(&a[k + 1], &b[k + 1]);
                if desc[k] {
                    o.reverse()
                } else {
                    o
                }
            })
            .find(|o| o.is_ne())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(rs.into_iter().map(|r| r[0].clone()).collect())
}

/// SELECTS(values, tests OVER T, condition): Quantrix's selection, the
/// values whose test meets the condition (a SUMIF-style criterion such as
/// `">5"` or `"a*"`), in item order; #N/A when none does.
fn selects(rows: &Rows, params: &[CellValue]) -> List {
    let c = super::statistical::Criterion::new(params.first().ok_or(err::VALUE)?);
    let out: Vec<CellValue> = rows
        .vals
        .iter()
        .filter(|r| r.get(1).is_some_and(|t| c.matches(t)))
        .filter_map(|r| r.first().cloned())
        .collect();
    if out.is_empty() {
        return Err(err::NA);
    }
    Ok(out)
}

/// TAKE(x OVER T, n): the first n values (the last -n when n < 0), in item
/// order; `drop` keeps the others (DROP).
fn take(rows: &Rows, params: &[CellValue], drop: bool) -> List {
    let vals: Vec<CellValue> = rows.col(0).cloned().collect();
    let len = vals.len();
    let k = n(params, 0)?.trunc();
    let m = k.abs().min(len as f64) as usize;
    // Proven in bounds; TAKE and DROP of one count partition the list.
    let (from, to) = improv_verified::take::range(len, m, k < 0.0, drop);
    if from == to {
        return Err(err::NA);
    }
    Ok(vals[from..to].to_vec())
}

/// MODE.MULT(x OVER T): every value of the highest count (at least two), in
/// order of first occurrence.
fn mode_mult(vals: &[CellValue]) -> List {
    let mut at: HashMap<u64, usize> = HashMap::new();
    let mut counts: Vec<(f64, usize)> = Vec::new();
    for x in vals.iter().filter_map(fit::number) {
        let x = x + 0.0; // -0 is 0
        let i = *at.entry(x.to_bits()).or_insert_with(|| {
            counts.push((x, 0));
            counts.len() - 1
        });
        counts[i].1 += 1;
    }
    let top = counts.iter().map(|c| c.1).max().unwrap_or(0);
    if top < 2 {
        return Err(err::NA);
    }
    Ok(counts
        .into_iter()
        .filter(|c| c.1 == top)
        .map(|c| CellValue::num(c.0))
        .collect())
}

/// TEXTSPLIT(t, col_delimiter, [row_delimiter], [ignore_empty = FALSE]):
/// the parts between delimiters (case-sensitive), every value's in item
/// order. A row_delimiter other than "" is not supported yet (#VALUE!).
fn textsplit(vals: &[CellValue], params: &[CellValue]) -> List {
    let delim = t(params, 0)?;
    if params.len() > 1 && !t(params, 1)?.is_empty() {
        return Err(err::VALUE);
    }
    let ignore = params.len() > 2 && b(params, 2)?;
    let mut out = Vec::new();
    for v in vals {
        let s = text_of(v)?;
        if s.is_empty() {
            return Err(err::VALUE);
        }
        let parts: Vec<&str> = if delim.is_empty() {
            vec![&s]
        } else {
            s.split(delim.as_str()).collect()
        };
        out.extend(
            parts
                .into_iter()
                .filter(|p| !ignore || !p.is_empty())
                .map(|p| CellValue::Text(p.into())),
        );
    }
    if out.is_empty() {
        return Err(err::NA);
    }
    Ok(out)
}

/// FREQUENCY(x OVER T, b1, b2, …): per bin, in the order given, the count
/// of values above the next lower bin and at most this one; then the count
/// above every bin. Of equal bins the last given takes the count (as in
/// LibreOffice).
fn frequency(vals: &[CellValue], params: &[CellValue]) -> List {
    let bins = nums(params)?;
    let mut ord: Vec<usize> = (0..bins.len()).collect();
    ord.sort_by(|&i, &j| {
        bins[i]
            .partial_cmp(&bins[j])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(j.cmp(&i))
    });
    let mut counts = vec![0_u32; bins.len() + 1];
    for x in vals.iter().filter_map(fit::number) {
        let k = ord.iter().find(|&&i| x <= bins[i]).copied();
        counts[k.unwrap_or(bins.len())] += 1;
    }
    Ok(counts
        .into_iter()
        .map(|c| CellValue::num(f64::from(c)))
        .collect())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn n(x: f64) -> CellValue {
        CellValue::num(x)
    }

    fn text(s: &str) -> CellValue {
        CellValue::Text(s.into())
    }

    pub(super) fn rows(vals: Vec<Vec<CellValue>>) -> Rows {
        Rows {
            pos: (0..vals.len() as u32).map(|i| vec![i]).collect(),
            vals,
            capacity: 8,
        }
    }

    fn col(v: &[CellValue]) -> Rows {
        rows(v.iter().map(|x| vec![x.clone()]).collect())
    }

    const NA: CellValue = CellValue::Err(err::NA);

    #[test]
    fn unknown_ids_and_errors() {
        assert_eq!(
            array(1299, &col(&[n(1.0)]), &[]),
            [CellValue::Err(err::NAME)]
        );
        // An error among the values is the result for the value functions...
        let bad = col(&[n(1.0), CellValue::Err(err::DIV0)]);
        assert_eq!(array(1256, &bad, &[]), [CellValue::Err(err::DIV0)]);
        let e = CellValue::Err(err::DIV0);
        assert_eq!(array(1257, &bad, &[]), [e.clone(), e.clone()]);
        let stats = [CellValue::Bool(true), CellValue::Bool(true)];
        assert_eq!(array(1258, &bad, &stats).len(), 10);
        // ...but TAKE moves it along.
        assert_eq!(array(1254, &bad, &[n(-1.0)]), [CellValue::Err(err::DIV0)]);
        assert_eq!(array(1255, &bad, &[n(1.0)]), [CellValue::Err(err::DIV0)]);
    }

    #[test]
    fn filter_branches() {
        let r = rows(vec![
            vec![n(1.0), CellValue::Bool(true)],
            vec![n(2.0), n(0.0)],
            vec![n(3.0), n(2.0)],
            vec![n(4.0)],
        ]);
        assert_eq!(array(1252, &r, &[]), [n(1.0), n(3.0)]);
        let none = rows(vec![vec![n(1.0), CellValue::Bool(false)]]);
        assert_eq!(array(1252, &none, &[]), [NA]);
        assert_eq!(array(1252, &none, &[text("-")]), [text("-")]);
        let bad = rows(vec![vec![n(1.0), text("yes")]]);
        assert_eq!(array(1252, &bad, &[]), [CellValue::Err(err::VALUE)]);
    }

    #[test]
    fn sortby_branches() {
        let r = rows(vec![
            vec![text("a"), n(2.0)],
            vec![text("b"), n(1.0)],
            vec![text("c"), n(2.0)],
            vec![text("d")],
        ]);
        assert_eq!(array(1253, &r, &[]), [text("b"), text("a"), text("c")]);
        assert_eq!(
            array(1253, &r, &[n(1.0)]),
            [text("b"), text("a"), text("c")]
        );
        // Descending keeps ties in item order.
        assert_eq!(
            array(1253, &r, &[n(-1.0)]),
            [text("a"), text("c"), text("b")]
        );
        assert_eq!(array(1253, &r, &[n(0.0)]), [CellValue::Err(err::VALUE)]);
        // Two keys: by1 ties broken by by2 (descending).
        let r2 = rows(vec![
            vec![text("a"), n(1.0), n(1.0)],
            vec![text("b"), n(0.0), n(5.0)],
            vec![text("c"), n(1.0), n(3.0)],
            vec![text("d"), n(0.0), n(2.0)],
        ]);
        assert_eq!(
            array(1253, &r2, &[n(1.0), n(-1.0)]),
            [text("b"), text("d"), text("c"), text("a")]
        );
        assert_eq!(
            array(1253, &r2, &[n(1.0), n(2.0)]),
            [CellValue::Err(err::VALUE)]
        );
        assert_eq!(
            array(1253, &r2, &[]),
            [text("d"), text("b"), text("a"), text("c")]
        );
        // SELECTS: the values whose test meets the condition.
        assert_eq!(array(1261, &r2, &[text(">0")]), [text("a"), text("c")]);
        assert_eq!(array(1261, &r2, &[n(9.0)]), [NA]);
        assert_eq!(array(1261, &r2, &[]), [CellValue::Err(err::VALUE)]);
        // An error key sorts last (after booleans).
        let e = rows(vec![
            vec![n(1.0), CellValue::Err(err::NA)],
            vec![n(2.0), CellValue::Bool(true)],
        ]);
        assert_eq!(array(1253, &e, &[]), [n(2.0), n(1.0)]);
        assert_eq!(array(1253, &r, &[text("x")]), [CellValue::Err(err::VALUE)]);
    }

    #[test]
    fn take_and_drop() {
        let r = col(&[n(1.0), n(2.0), n(3.0)]);
        let take = |k: f64| array(1254, &r, &[n(k)]);
        let drop = |k: f64| array(1255, &r, &[n(k)]);
        assert_eq!(take(2.9), [n(1.0), n(2.0)]);
        assert_eq!(take(-2.0), [n(2.0), n(3.0)]);
        assert_eq!(take(9.0), [n(1.0), n(2.0), n(3.0)]);
        assert_eq!(take(-0.5), [NA]);
        assert_eq!(drop(1.0), [n(2.0), n(3.0)]);
        assert_eq!(drop(-1.0), [n(1.0), n(2.0)]);
        assert_eq!(drop(0.0), [n(1.0), n(2.0), n(3.0)]);
        assert_eq!(drop(-3.0), [NA]);
        assert_eq!(array(1254, &r, &[text("x")]), [CellValue::Err(err::VALUE)]);
    }

    #[test]
    fn mode_mult_branches() {
        let r = col(&[n(2.0), n(-0.0), n(2.0), n(0.0), text("x"), n(5.0)]);
        assert_eq!(array(1256, &r, &[]), [n(2.0), n(0.0)]);
        assert_eq!(array(1256, &col(&[n(1.0), n(2.0)]), &[]), [NA]);
        assert_eq!(array(1256, &col(&[text("x")]), &[]), [NA]);
    }

    #[test]
    fn textsplit_branches() {
        let r = col(&[text("a,b,,c"), text(",x")]);
        let split = |p: &[CellValue]| array(1259, &r, p);
        assert_eq!(split(&[text(",")]), ["a", "b", "", "c", "", "x"].map(text));
        assert_eq!(
            split(&[text(","), text(""), CellValue::Bool(true)]),
            ["a", "b", "c", "x"].map(text)
        );
        assert_eq!(split(&[text("")]), [text("a,b,,c"), text(",x")]);
        // Rows are deferred: a row delimiter is refused.
        assert_eq!(split(&[text(","), text(";")]), [CellValue::Err(err::VALUE)]);
        assert_eq!(
            array(1259, &col(&[text("")]), &[text(",")]),
            [CellValue::Err(err::VALUE)]
        );
        assert_eq!(
            array(
                1259,
                &col(&[text(",,")]),
                &[text(","), text(""), CellValue::Bool(true)]
            ),
            [NA]
        );
        // Numbers split as their text.
        assert_eq!(
            array(1259, &col(&[n(1.5)]), &[text(".")]),
            [text("1"), text("5")]
        );
    }

    #[test]
    fn frequency_branches() {
        let r = col(&[n(1.0), n(2.0), n(3.0), n(4.0), n(5.0), n(6.0), text("x")]);
        let f = |bins: &[f64]| array(1260, &r, &bins.iter().map(|b| n(*b)).collect::<Vec<_>>());
        assert_eq!(f(&[4.0, 2.0]), [n(2.0), n(2.0), n(2.0)]);
        assert_eq!(f(&[2.0, 2.0, 4.0]), [n(0.0), n(2.0), n(2.0), n(2.0)]);
        assert_eq!(f(&[0.0]), [n(0.0), n(6.0)]);
        assert_eq!(array(1260, &r, &[text("x")]), [CellValue::Err(err::VALUE)]);
    }

    #[test]
    fn sort_and_unique() {
        let v = col(&[n(3.0), text("b"), n(1.0), n(3.0)]);
        let up = [n(1.0), n(3.0), n(3.0), text("b")];
        assert_eq!(array(1250, &v, &[]), up);
        assert_eq!(array(1250, &v, &[n(1.0)]), up);
        let mut down = up.clone();
        down.reverse();
        assert_eq!(array(1250, &v, &[n(-1.0)]), down);
        // UNIQUE: first occurrences in item order, or (exactly_once) only
        // the values that occur once.
        assert_eq!(array(1251, &v, &[]), [n(3.0), text("b"), n(1.0)]);
        let once = [CellValue::Bool(true)];
        assert_eq!(array(1251, &v, &once), [text("b"), n(1.0)]);
        // Text ignores case (Excel, LibreOffice, Gnumeric): the first
        // spelling stays, and with exactly_once "a" and "A" are a repeat.
        let v = col(&[text("Fig"), text("ä"), text("fig"), text("Ä"), text("x")]);
        assert_eq!(array(1251, &v, &[]), [text("Fig"), text("ä"), text("x")]);
        assert_eq!(array(1251, &v, &once), [text("x")]);
    }

    #[test]
    fn sort_order_ranks_kinds() {
        use std::cmp::Ordering::*;
        assert_eq!(order(&n(1.0), &text("a")), Less);
        assert_eq!(order(&CellValue::Bool(false), &CellValue::Bool(true)), Less);
        assert_eq!(order(&CellValue::Err(1), &CellValue::Bool(true)), Greater);
        assert_eq!(order(&CellValue::Date(5), &n(5.0)), Equal);
        assert_eq!(order(&text("B"), &text("a")), Greater);
        assert_eq!(order(&n(f64::NAN), &n(1.0)), Equal);
    }

    /// UNIQUE with exactly_once where every value repeats is the empty
    /// list, #N/A (LibreOffice's UNIQUE(…; FALSE; TRUE) too); it was an
    /// empty list, which lays nothing along INTO.
    #[test]
    fn unique_exactly_once_of_only_repeats() {
        let v = col(&[CellValue::Bool(false), CellValue::Bool(false)]);
        assert_eq!(array(1251, &v, &[CellValue::Bool(true)]), [NA]);
        assert_eq!(array(1251, &v, &[]), [CellValue::Bool(false)]);
    }
}
