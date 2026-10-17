//! The reductions: what an aggregation (`SUM(x OVER T)`, `CORREL(x, y OVER
//! T)`, `NPV(rate, cf OVER T)`) computes from one group's rows.
//!
//! The dataflow hands a group over as [`Rows`]: every row's position along
//! each reduced category (so rows arrive in item order: the first row is the
//! first item that has a cell) and its values, one per reduced measure in
//! argument order. Parameters (`rate`, `k`, `guess`) have one value per group.
//! The scalar evaluator never reduces (a scalar formula has no categories).
//!
//! Families add their reductions in `reduce/<family>.rs`, dispatched by id
//! range like `builtins`: P4 statistics own 1000–1099 and 1100–1149, P5
//! financial 1150–1199.

use crate::builtins::{self, err, text_of};
use crate::CellValue;
use improv_core_model::FuncId;

mod financial;

/// One group's rows, in item order.
#[derive(Debug, Clone, PartialEq)]
pub struct Rows {
    /// Per row, the position of its item along each reduced category
    /// (0-based, in the category's item order).
    pub pos: Vec<Vec<u32>>,
    /// Per row, the values of the reduced measures (argument order).
    pub vals: Vec<Vec<CellValue>>,
    /// How many cells the reduced categories could hold (the product of
    /// their item counts), for `COUNTBLANK`.
    pub capacity: usize,
}

impl Rows {
    /// The `i`-th reduced measure's values, in item order.
    pub fn col(&self, i: usize) -> impl Iterator<Item = &CellValue> + '_ {
        self.vals.iter().filter_map(move |r| r.get(i))
    }

    /// The first error among the values (in item order), if any.
    pub fn first_error(&self) -> Option<CellValue> {
        self.vals
            .iter()
            .flatten()
            .find(|v| matches!(v, CellValue::Err(_)))
            .cloned()
    }

    /// The number of rows.
    pub fn len(&self) -> usize {
        self.vals.len()
    }

    /// Whether there are no rows.
    pub fn is_empty(&self) -> bool {
        self.vals.is_empty()
    }
}

/// Reduce one group: aggregation `func` over `rows` with `params`.
pub fn reduce(func: FuncId, rows: &Rows, params: &[CellValue]) -> CellValue {
    if let Some(bad) = params.iter().find(|v| matches!(v, CellValue::Err(_))) {
        return bad.clone();
    }
    match func.0 {
        1..=9 => core(func.0, rows),
        906..=909 => ordered(func.0, rows, params),
        1150..=1199 => financial::reduce(func.0, rows, params),
        _ => CellValue::Err(err::NAME),
    }
}

/// One result per row (a [`improv_core_model::functions::Kind::PerItem`]
/// function such as `RANK`): the `i`-th result belongs to the `i`-th row.
pub fn per_item(func: FuncId, rows: &Rows, params: &[CellValue]) -> Vec<CellValue> {
    if let Some(bad) = params.iter().find(|v| matches!(v, CellValue::Err(_))) {
        return vec![bad.clone(); rows.len()];
    }
    // Each family adds its per-item reductions here by id (RANK, TREND, …).
    let _ = func;
    vec![CellValue::Err(err::NAME); rows.len()]
}

/// SUM … XOR through their scalar twins (an error in the group is the
/// result: the first in item order).
fn core(id: u32, rows: &Rows) -> CellValue {
    if let Some(bad) = rows.first_error() {
        return bad;
    }
    let twin = match id {
        1 => 22,
        2 => 23,
        3 => 20,
        4 => 21,
        5 => 24,
        6 => 25,
        7 => 26,
        8 => 27,
        _ => 28,
    };
    let vals: Vec<CellValue> = rows.col(0).cloned().collect();
    builtins::eval(FuncId(twin), &vals)
}

/// FIRST, LAST, S, TEXTJOIN: they read the rows in item order.
fn ordered(id: u32, rows: &Rows, params: &[CellValue]) -> CellValue {
    match id {
        906 | 908 => {
            let Some(v) = rows.col(0).next().cloned() else {
                return CellValue::Err(err::NA);
            };
            if id == 908 {
                return match v {
                    CellValue::Text(_) | CellValue::Err(_) => v,
                    _ => CellValue::Text(String::new()),
                };
            }
            v
        }
        907 => rows
            .col(0)
            .last()
            .cloned()
            .unwrap_or(CellValue::Err(err::NA)),
        _ => {
            // TEXTJOIN(sep, skip_empty, x OVER T): params are (sep, skip).
            let sep = match params.first().map(text_of) {
                Some(Ok(s)) => s,
                Some(Err(c)) => return CellValue::Err(c),
                None => String::new(),
            };
            let skip = matches!(params.get(1), Some(CellValue::Bool(true)));
            let mut parts = Vec::new();
            for v in rows.col(0) {
                match text_of(v) {
                    Ok(s) if skip && s.is_empty() => {}
                    Ok(s) => parts.push(s),
                    Err(c) => return CellValue::Err(c),
                }
            }
            let joined = parts.join(&sep);
            if joined.chars().count() > 32_767 {
                return CellValue::Err(err::VALUE);
            }
            CellValue::Text(joined)
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn rows(vals: Vec<CellValue>) -> Rows {
        Rows {
            pos: (0..vals.len() as u32).map(|i| vec![i]).collect(),
            vals: vals.into_iter().map(|v| vec![v]).collect(),
            capacity: 4,
        }
    }

    #[test]
    fn core_and_ordered() {
        let n = CellValue::num;
        let r = rows(vec![n(4.0), n(1.0), n(7.0)]);
        assert_eq!(reduce(FuncId(1), &r, &[]), n(12.0));
        assert_eq!(reduce(FuncId(906), &r, &[]), n(4.0));
        assert_eq!(reduce(FuncId(907), &r, &[]), n(7.0));
        let t = rows(vec![
            CellValue::Text("a".into()),
            CellValue::Text(String::new()),
            n(2.0),
        ]);
        let sep = CellValue::Text(", ".into());
        assert_eq!(
            reduce(FuncId(909), &t, &[sep.clone(), CellValue::Bool(true)]),
            CellValue::Text("a, 2".into())
        );
        assert_eq!(
            reduce(FuncId(909), &t, &[sep, CellValue::Bool(false)]),
            CellValue::Text("a, , 2".into())
        );
        assert_eq!(reduce(FuncId(908), &r, &[]), CellValue::Text(String::new()));
        assert_eq!(reduce(FuncId(908), &t, &[]), CellValue::Text("a".into()));
        let e = rows(vec![n(1.0), CellValue::Err(err::NA)]);
        assert_eq!(reduce(FuncId(1), &e, &[]), CellValue::Err(err::NA));
        assert_eq!(
            reduce(FuncId(1), &r, &[CellValue::Err(err::DIV0)]),
            CellValue::Err(err::DIV0)
        );
        assert_eq!(reduce(FuncId(999), &r, &[]), CellValue::Err(err::NAME));
        assert_eq!(
            per_item(FuncId(999), &r, &[]),
            vec![CellValue::Err(err::NAME); 3]
        );
        assert_eq!(
            per_item(FuncId(999), &r, &[CellValue::Err(err::NA)]),
            vec![CellValue::Err(err::NA); 3]
        );
        assert_eq!(r.len(), 3);
        assert!(!r.is_empty());
        // TEXTJOIN: an error value or separator, and an over-long result.
        let te = rows(vec![CellValue::Text("a".into()), CellValue::Err(err::NUM)]);
        assert_eq!(
            reduce(FuncId(909), &te, &[CellValue::Text(",".into())]),
            CellValue::Err(err::NUM)
        );
        assert_eq!(reduce(FuncId(909), &t, &[]), CellValue::Text("a2".into()));
        let long = rows(vec![CellValue::Text("x".repeat(20_000)); 2]);
        assert_eq!(
            reduce(FuncId(909), &long, &[CellValue::Text(String::new())]),
            CellValue::Err(err::VALUE)
        );
        assert_eq!(reduce(FuncId(908), &e, &[]), CellValue::Text(String::new()));
        let er = rows(vec![CellValue::Err(err::NA)]);
        assert_eq!(reduce(FuncId(908), &er, &[]), CellValue::Err(err::NA));
        assert_eq!(e.first_error(), Some(CellValue::Err(err::NA)));
        let empty = rows(vec![]);
        assert_eq!(reduce(FuncId(906), &empty, &[]), CellValue::Err(err::NA));
        assert_eq!(reduce(FuncId(907), &empty, &[]), CellValue::Err(err::NA));
    }
}
