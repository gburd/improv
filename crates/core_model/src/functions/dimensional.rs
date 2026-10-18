//! The dimensional functions: a cell's position in a category, its
//! neighbours along an ordered category, and the ordered reductions.
//! Item order is the category's item order.

use super::{alias, f, over, s, Arg, Category, FnDef, Kind, Over, Ret};
use crate::value::ValueType;

use Arg::{Any as A, Boolean as B, Category as K, Number as N, Text as T};
use Category::Dimensional as D;
use ValueType as V;

#[rustfmt::skip]
pub(super) static DIMENSIONAL: &[FnDef] = &[
    s(900, "POSITION", D, &[K], V::Number, "1-based position of this cell's item along the category"),
    s(901, "@ITEM", D, &[K], V::Text, "the name of this cell's item along the category"),
    s(902, "@ID", D, &[K], V::Text, "the item name (Improv item names are unique within a category)"),
    s(903, "@ITEMCOUNT", D, &[K], V::Number, "the number of items in the category"),
    f(904, "PREV", Kind::Lazy, D, &[A, K, N, A], 2, false, Ret::Unify, "the value at the n-th previous item along the category (n = 1); off the end, the default, else no cell"),
    f(905, "NEXT", Kind::Lazy, D, &[A, K, N, A], 2, false, Ret::Unify, "the value at the n-th next item along the category (n = 1); off the end, the default, else no cell"),
    over(f(906, "FIRST", Kind::Aggregate, Category::Aggregation, &[A], 1, false, Ret::Unify, "the value at the first item (in category order) that has one"), Over::At(&[0])),
    over(f(907, "LAST", Kind::Aggregate, Category::Aggregation, &[A], 1, false, Ret::Unify, "the value at the last item that has one"), Over::At(&[0])),
    over(f(908, "S", Kind::Aggregate, Category::Aggregation, &[A], 1, false, Ret::Fixed(V::Text), "the first value, as text (Quantrix)"), Over::At(&[0])),
    f(911, "SCAN", Kind::Lazy, Category::Logical, &[A, A, A], 3, false, Ret::Unify, "SCAN(init, x OVER T, LAMBDA(acc, v, body)): the running fold along T, one value per item"),
    f(912, "REDUCE", Kind::Lazy, Category::Logical, &[A, A, A], 3, false, Ret::Unify, "REDUCE(init, x OVER T, LAMBDA(acc, v, body)): the fold's last value"),
    f(913, "LET", Kind::Lazy, Category::Logical, &[A, A, A], 3, true, Ret::Unify, "LET(name, value, …, body): the body with each name bound to its value"),
    f(914, "LAMBDA", Kind::Lazy, Category::Logical, &[A, A], 2, true, Ret::Unify, "LAMBDA(p, …, body): a function of its parameters, called on the spot or passed to MAP, REDUCE or SCAN"),
    f(916, "SEQUENCE", Kind::Lazy, Category::Dimensional, &[A, A, A], 1, false, Ret::Unify, "SEQUENCE(T, [start = 1], [step = 1]): start, start + step, … along T's items, (POSITION(T) - 1) * step + start"),
    f(917, "MAKEARRAY", Kind::Lazy, Category::Logical, &[A, A, A], 3, false, Ret::Unify, "MAKEARRAY(R, C, LAMBDA(r, c, body)): the body over R × C, with r and c the item positions"),
    alias(f(918, "BYROW", Kind::Lazy, Category::Logical, &[A, A], 2, false, Ret::Unify, "BYROW(x OVER C, LAMBDA(row, body)): the body per item of x's other categories, row being the values along C (the body reduces it: SUM(row), …)"), &["BYCOL"]),
    f(915, "MAP", Kind::Lazy, Category::Logical, &[A, A], 2, true, Ret::Unify, "MAP(x, …, LAMBDA(p, …, body)): the body applied element by element"),
    over(f(909, "TEXTJOIN", Kind::Aggregate, Category::Aggregation, &[T, B, A], 3, false, Ret::Fixed(V::Text), "the values in item order joined by the delimiter (skipping empty text when ignore_empty is TRUE)"), Over::At(&[2])),
];
