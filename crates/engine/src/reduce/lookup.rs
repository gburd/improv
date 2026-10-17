//! The lookups and database functions (P6, ids 1200–1249): reductions whose
//! key is a parameter. `XLOOKUP(key, keys OVER T, values)` reads one group's
//! rows (the table, in item order) and picks one; a key over categories the
//! table lacks gives one lookup per key cell (the dataflow broadcasts).
//!
//! A row exists only where every reduced measure has a cell, so an item with
//! a key but no value is not in the table. Keys compare within a kind:
//! numbers with numbers (dates as serials), text with text ignoring case,
//! booleans with booleans; an error key never matches. The approximate modes
//! assume nothing about order: `MATCH(k, keys OVER T, 1)` is the largest key
//! ≤ k (the last on a tie), which on ascending keys is Excel's answer.

use super::statistical::{like, number};
use super::Rows;
use crate::builtins::{b, err, fold_char, n, n_or, text_of};
use crate::CellValue;
use std::cmp::Ordering;

type R = Result<CellValue, u8>;

/// Evaluate lookup `id` over a group's rows with parameters `p`.
pub(super) fn reduce(id: u32, rows: &Rows, p: &[CellValue]) -> CellValue {
    // XLOOKUP reads its if_not_found (argument 4, parameter 1) only when
    // nothing matches, so an error there is not the result.
    let bad = p
        .iter()
        .enumerate()
        .find(|(i, v)| matches!(v, CellValue::Err(_)) && !(id == 1200 && *i == 1));
    if let Some((_, v)) = bad {
        return v.clone();
    }
    eval(id, rows, p).unwrap_or_else(CellValue::Err)
}

fn eval(id: u32, rows: &Rows, p: &[CellValue]) -> R {
    let cell = |r: usize, c: usize| rows.vals[r][c].clone();
    let fwd = || 0..rows.len();
    match id {
        // XLOOKUP(key, keys OVER T, values, [if_not_found], [match], [search])
        1200 => match xsearch(rows, 0, &p[0], p.get(2), p.get(3))? {
            Some(r) => Ok(cell(r, 1)),
            None => Ok(p.get(1).cloned().unwrap_or(CellValue::Err(err::NA))),
        },
        // XMATCH(key, keys OVER T, [match], [search])
        1201 => position(rows, xsearch(rows, 0, &p[0], p.get(1), p.get(2))?),
        // MATCH(key, keys OVER T, [type = 1])
        1202 => {
            let r = match n_or(p, 1, 1.0)?.trunc() {
                0.0 => fwd().find(|&r| equal(&rows.vals[r][0], &p[0], true)),
                1.0 => nearest(rows, 0, &p[0], Ordering::Less, fwd(), false),
                -1.0 => nearest(rows, 0, &p[0], Ordering::Greater, fwd(), false),
                _ => return Err(err::VALUE),
            };
            position(rows, r)
        }
        // LOOKUP(key, keys OVER T, values): the largest key ≤ key.
        1203 => nearest(rows, 0, &p[0], Ordering::Less, fwd(), false)
            .map(|r| cell(r, 1))
            .ok_or(err::NA),
        // INDEX(x OVER T, n): the value at the n-th item.
        1204 => index(rows, n(p, 0)?.trunc()),
        // SELECT(values, keys OVER T, lookup) and the range forms: the first
        // item, in item order, whose key qualifies.
        1210..=1213 => {
            let ok = |c: &CellValue, k: &CellValue, ord: Ordering, inclusive| {
                compare(c, k).is_some_and(|o| o == ord || (inclusive && o == Ordering::Equal))
            };
            let inclusive = |i: usize, default| match p.get(i) {
                Some(_) => b(p, i),
                None => Ok(default),
            };
            let pick: Box<dyn Fn(&CellValue) -> bool> = match id {
                1210 => Box::new(|c| equal(c, &p[0], false)),
                1211 => {
                    let inc = inclusive(2, true)?;
                    Box::new(move |c| {
                        ok(c, &p[0], Ordering::Greater, inc) && ok(c, &p[1], Ordering::Less, inc)
                    })
                }
                1212 => {
                    let inc = inclusive(1, false)?;
                    Box::new(move |c| ok(c, &p[0], Ordering::Greater, inc))
                }
                _ => {
                    let inc = inclusive(1, false)?;
                    Box::new(move |c| ok(c, &p[0], Ordering::Less, inc))
                }
            };
            fwd()
                .find(|&r| pick(&rows.vals[r][1]))
                .map(|r| cell(r, 0))
                .ok_or(err::NA)
        }
        // DGET(x OVER T WHERE pred): Excel's single-record rule.
        1220 => match (rows.first_error(), rows.len()) {
            (Some(e), _) => Ok(e),
            (None, 0) => Err(err::VALUE),
            (None, 1) => Ok(cell(0, 0)),
            _ => Err(err::NUM),
        },
        // ARRAYTOTEXT(x OVER T, [format = 0])
        1230 => array_to_text(rows, n_or(p, 0, 0.0)?),
        _ => Err(err::NAME),
    }
}

/// How `cell` compares with `key` when they are the same kind (numbers and
/// dates together, text ignoring case, booleans); `None` otherwise.
fn compare(cell: &CellValue, key: &CellValue) -> Option<Ordering> {
    let fold = |s: &str| s.chars().map(fold_char).collect::<String>();
    match (cell, key) {
        (CellValue::Text(a), CellValue::Text(k)) => Some(fold(a).cmp(&fold(k))),
        (CellValue::Bool(a), CellValue::Bool(k)) => Some(a.cmp(k)),
        _ => number(cell)?.partial_cmp(&number(key)?),
    }
}

/// Whether `cell` equals `key`; with `wild`, text keys take the wildcards
/// `*`, `?` and `~`.
fn equal(cell: &CellValue, key: &CellValue, wild: bool) -> bool {
    match (cell, key) {
        (CellValue::Text(t), CellValue::Text(pat)) if wild => like(pat, t),
        _ => compare(cell, key) == Some(Ordering::Equal),
    }
}

/// The row, scanning `order`, whose `col` cell is nearest `key` from one
/// side: `Less` the largest ≤ key, `Greater` the smallest ≥ key (an equal
/// key is nearest). On a tie, the first scanned, or the last.
fn nearest(
    rows: &Rows,
    col: usize,
    key: &CellValue,
    side: Ordering,
    order: impl Iterator<Item = usize>,
    first: bool,
) -> Option<usize> {
    let cell = |r: usize| &rows.vals[r][col];
    let mut best: Option<usize> = None;
    for r in order {
        if !compare(cell(r), key).is_some_and(|o| o != side.reverse()) {
            continue;
        }
        let better = best.is_none_or(|b| match compare(cell(r), cell(b)) {
            Some(Ordering::Equal) => !first,
            o => o == Some(side.reverse()),
        });
        if better {
            best = Some(r);
        }
    }
    best
}

/// XLOOKUP's and XMATCH's search: match mode 0 exact, -1 exact or next
/// smaller, 1 exact or next larger, 2 wildcard; search mode 1 first to last,
/// -1 last to first. The binary modes (±2) assume sorted keys in Excel; here
/// they are the linear search, which gives the same answer on sorted keys.
fn xsearch(
    rows: &Rows,
    col: usize,
    key: &CellValue,
    mode: Option<&CellValue>,
    search: Option<&CellValue>,
) -> Result<Option<usize>, u8> {
    let arg = |v: Option<&CellValue>, d: f64| match v {
        Some(v) => n(std::slice::from_ref(v), 0).map(f64::trunc),
        None => Ok(d),
    };
    let (mode, search) = (arg(mode, 0.0)?, arg(search, 1.0)?);
    if !matches!(search, -2.0 | -1.0 | 1.0 | 2.0) {
        return Err(err::VALUE);
    }
    let order: Vec<usize> = if search < 0.0 {
        (0..rows.len()).rev().collect()
    } else {
        (0..rows.len()).collect()
    };
    let mut it = order.into_iter();
    Ok(match mode {
        0.0 | 2.0 => it.find(|&r| equal(&rows.vals[r][col], key, mode == 2.0)),
        -1.0 => nearest(rows, col, key, Ordering::Less, it, true),
        1.0 => nearest(rows, col, key, Ordering::Greater, it, true),
        _ => return Err(err::VALUE),
    })
}

/// Row `r`'s 1-based item position along the (single) reduced category;
/// #N/A for no row.
fn position(rows: &Rows, r: Option<usize>) -> R {
    let r = r.ok_or(err::NA)?;
    match rows.pos[r][..] {
        [p] => Ok(CellValue::num(f64::from(p) + 1.0)),
        _ => Err(err::VALUE),
    }
}

/// `INDEX(x OVER T, n)`: #VALUE! below 1 (or over more than one category),
/// #REF! past T's last item, #N/A where the n-th item has no cell (Improv
/// has no blank-as-zero).
fn index(rows: &Rows, k: f64) -> R {
    if k < 1.0 || rows.pos.first().is_some_and(|p| p.len() != 1) {
        return Err(err::VALUE);
    }
    if k > rows.capacity as f64 {
        return Err(err::REF);
    }
    let at = k as u32 - 1;
    rows.pos
        .iter()
        .position(|p| p[0] == at)
        .map(|r| rows.vals[r][0].clone())
        .ok_or(err::NA)
}

/// `ARRAYTOTEXT`: format 0 joins the values with ", "; format 1 writes a
/// list literal, `{1,"a",TRUE}`, text quoted as `VALUETOTEXT` quotes it.
/// Errors show as their codes.
fn array_to_text(rows: &Rows, format: f64) -> R {
    let strict = match format {
        0.0 => false,
        1.0 => true,
        _ => return Err(err::VALUE),
    };
    let parts: Vec<String> = rows
        .col(0)
        .map(|v| match v {
            CellValue::Err(_) => v.to_string(),
            CellValue::Text(s) if strict => format!("\"{s}\""),
            _ => text_of(v).unwrap_or_default(),
        })
        .collect();
    Ok(CellValue::Text(if strict {
        format!("{{{}}}", parts.join(","))
    } else {
        parts.join(", ")
    }))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::functions::all;

    fn nv(x: f64) -> CellValue {
        CellValue::num(x)
    }
    fn tx(s: &str) -> CellValue {
        CellValue::Text(s.into())
    }
    fn e(c: u8) -> CellValue {
        CellValue::Err(c)
    }

    /// Rows at the given item positions, one measure per column.
    fn table(pos: &[u32], cols: &[Vec<CellValue>], capacity: usize) -> Rows {
        Rows {
            pos: pos.iter().map(|p| vec![*p]).collect(),
            vals: (0..pos.len())
                .map(|r| cols.iter().map(|c| c[r].clone()).collect())
                .collect(),
            capacity,
        }
    }

    /// Keys 3 1 2 1 at positions 0, 2, 3, 4 (position 1 has no cell).
    fn keys() -> Rows {
        let k = vec![nv(3.0), nv(1.0), nv(2.0), nv(1.0)];
        let v = vec![tx("a"), tx("b"), tx("c"), tx("d")];
        table(&[0, 2, 3, 4], &[k, v], 5)
    }

    #[test]
    fn every_lookup_is_implemented() {
        let r = keys();
        for d in all().filter(|d| (1200..=1249).contains(&d.id.0)) {
            let n_params = (0..d.required).filter(|i| !d.over.contains(*i)).count();
            let p = vec![nv(1.0); n_params];
            assert_ne!(reduce(d.id.0, &r, &p), e(err::NAME), "{}", d.name);
        }
        assert_eq!(reduce(1249, &r, &[]), e(err::NAME));
    }

    #[test]
    fn xlookup_modes_and_errors() {
        let r = keys();
        let x = |p: &[CellValue]| reduce(1200, &r, p);
        assert_eq!(x(&[nv(1.0)]), tx("b"));
        assert_eq!(x(&[nv(1.0), tx("-"), nv(0.0), nv(-1.0)]), tx("d"));
        assert_eq!(x(&[nv(9.0)]), e(err::NA));
        assert_eq!(x(&[nv(9.0), tx("none")]), tx("none"));
        // An error if_not_found is read only when nothing matches.
        assert_eq!(x(&[nv(1.0), e(err::DIV0)]), tx("b"));
        assert_eq!(x(&[nv(9.0), e(err::DIV0)]), e(err::DIV0));
        assert_eq!(x(&[e(err::NUM)]), e(err::NUM));
        // Next smaller / larger; ties take the first in search order.
        assert_eq!(x(&[nv(1.5), tx("-"), nv(-1.0)]), tx("b"));
        assert_eq!(x(&[nv(1.5), tx("-"), nv(-1.0), nv(-1.0)]), tx("d"));
        assert_eq!(x(&[nv(1.5), tx("-"), nv(1.0)]), tx("c"));
        assert_eq!(x(&[nv(0.5), tx("-"), nv(-1.0)]), tx("-"));
        // Binary search is the linear one.
        assert_eq!(x(&[nv(2.0), tx("-"), nv(0.0), nv(2.0)]), tx("c"));
        assert_eq!(x(&[nv(2.0), tx("-"), nv(0.0), nv(0.0)]), e(err::VALUE));
        assert_eq!(x(&[nv(2.0), tx("-"), nv(3.0)]), e(err::VALUE));
        assert_eq!(x(&[nv(2.0), tx("-"), tx("x")]), e(err::VALUE));
        // XMATCH gives the item position, not the row ordinal.
        assert_eq!(reduce(1201, &r, &[nv(2.0)]), nv(4.0));
        assert_eq!(reduce(1201, &r, &[nv(7.0)]), e(err::NA));
    }

    #[test]
    fn text_keys_and_wildcards() {
        let k = vec![tx("Apple"), tx("banana"), CellValue::Bool(true), e(err::NA)];
        let r = table(&[0, 1, 2, 3], &[k.clone(), k], 4);
        let x = |p: &[CellValue]| reduce(1201, &r, p);
        assert_eq!(x(&[tx("BANANA")]), nv(2.0));
        assert_eq!(x(&[tx("b*")]), e(err::NA));
        assert_eq!(x(&[tx("b*"), nv(2.0)]), nv(2.0));
        assert_eq!(x(&[CellValue::Bool(true)]), nv(3.0));
        assert_eq!(x(&[tx("apricot"), nv(-1.0)]), nv(1.0));
        // MATCH 0 takes wildcards; an error key never matches.
        assert_eq!(reduce(1202, &r, &[tx("?pple"), nv(0.0)]), nv(1.0));
        assert_eq!(reduce(1202, &r, &[nv(1.0), nv(0.0)]), e(err::NA));
        // SELECT compares exactly (no wildcards).
        assert_eq!(reduce(1210, &r, &[tx("b*")]), e(err::NA));
        assert_eq!(reduce(1210, &r, &[tx("apple")]), tx("Apple"));
    }

    #[test]
    fn match_and_lookup_nearest() {
        // Ascending with a tie: 1 2 2 3.
        let k = vec![nv(1.0), nv(2.0), nv(2.0), nv(3.0)];
        let v = vec![nv(10.0), nv(20.0), nv(30.0), nv(40.0)];
        let r = table(&[0, 1, 2, 3], &[k, v], 4);
        let m = |p: &[CellValue]| reduce(1202, &r, p);
        assert_eq!(m(&[nv(2.0)]), nv(3.0));
        assert_eq!(m(&[nv(2.5)]), nv(3.0));
        assert_eq!(m(&[nv(0.0)]), e(err::NA));
        assert_eq!(m(&[nv(2.0), nv(-1.0)]), nv(3.0));
        assert_eq!(m(&[nv(2.5), nv(-1.0)]), nv(4.0));
        assert_eq!(m(&[nv(2.0), nv(0.5)]), nv(2.0));
        assert_eq!(m(&[nv(2.0), nv(5.0)]), e(err::VALUE));
        assert_eq!(m(&[nv(2.0), tx("x")]), e(err::VALUE));
        assert_eq!(reduce(1203, &r, &[nv(2.0)]), nv(30.0));
        assert_eq!(reduce(1203, &r, &[nv(0.5)]), e(err::NA));
        // Over two categories a position is not defined.
        let two = Rows {
            pos: vec![vec![0, 0]],
            vals: vec![vec![nv(1.0)]],
            capacity: 4,
        };
        assert_eq!(reduce(1202, &two, &[nv(1.0)]), e(err::VALUE));
        assert_eq!(reduce(1204, &two, &[nv(1.0)]), e(err::VALUE));
    }

    #[test]
    fn index_rules() {
        let r = keys();
        let i = |k: f64| reduce(1204, &r, &[nv(k)]);
        assert_eq!(i(1.0), nv(3.0));
        assert_eq!(i(3.7), nv(1.0));
        assert_eq!(i(2.0), e(err::NA), "item 2 has no cell");
        assert_eq!(i(5.0), nv(1.0));
        assert_eq!(i(6.0), e(err::REF));
        assert_eq!(i(0.0), e(err::VALUE));
        assert_eq!(reduce(1204, &r, &[tx("x")]), e(err::VALUE));
    }

    #[test]
    fn select_family() {
        let vals = vec![tx("a"), tx("b"), tx("c"), tx("d")];
        let k = vec![nv(5.0), nv(10.0), nv(15.0), nv(10.0)];
        let r = table(&[0, 1, 2, 3], &[vals, k], 4);
        let t = CellValue::Bool(true);
        let f = CellValue::Bool(false);
        assert_eq!(reduce(1210, &r, &[nv(10.0)]), tx("b"));
        assert_eq!(reduce(1210, &r, &[nv(11.0)]), e(err::NA));
        assert_eq!(reduce(1211, &r, &[nv(10.0), nv(15.0)]), tx("b"));
        assert_eq!(
            reduce(1211, &r, &[nv(10.0), nv(15.0), f.clone()]),
            e(err::NA)
        );
        assert_eq!(reduce(1211, &r, &[nv(6.0), nv(20.0), f.clone()]), tx("b"));
        assert_eq!(reduce(1212, &r, &[nv(10.0)]), tx("c"));
        assert_eq!(reduce(1212, &r, &[nv(10.0), t.clone()]), tx("b"));
        assert_eq!(reduce(1213, &r, &[nv(10.0)]), tx("a"));
        assert_eq!(reduce(1213, &r, &[nv(5.0)]), e(err::NA));
        assert_eq!(reduce(1213, &r, &[nv(5.0), t]), tx("a"));
        assert_eq!(reduce(1212, &r, &[nv(1.0), tx("x")]), e(err::VALUE));
        assert_eq!(reduce(1210, &r, &[e(err::DIV0)]), e(err::DIV0));
    }

    #[test]
    fn dget_and_arraytotext() {
        let one = table(&[2], &[vec![nv(7.0)]], 3);
        assert_eq!(reduce(1220, &one, &[]), nv(7.0));
        let none = table(&[], &[vec![]], 3);
        assert_eq!(reduce(1220, &none, &[]), e(err::VALUE));
        let r = table(
            &[0, 1, 2, 3],
            &[vec![nv(1.5), tx("a\"b"), CellValue::Bool(true), e(err::NA)]],
            4,
        );
        assert_eq!(reduce(1220, &r, &[]), e(err::NA));
        let two = table(&[0, 1], &[vec![nv(1.0), nv(2.0)]], 2);
        assert_eq!(reduce(1220, &two, &[]), e(err::NUM));
        assert_eq!(reduce(1230, &r, &[]), tx("1.5, a\"b, TRUE, #N/A"));
        assert_eq!(reduce(1230, &r, &[nv(1.0)]), tx("{1.5,\"a\"b\",TRUE,#N/A}"));
        assert_eq!(reduce(1230, &r, &[nv(2.0)]), e(err::VALUE));
    }
}
