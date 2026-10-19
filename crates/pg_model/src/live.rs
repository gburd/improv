//! G1: map decoded `pgoutput` row changes onto live-engine edits.
//!
//! The logical-decoding worker keeps a live [`improv_engine::session::Engine`]
//! per model. [`Mapper::map`] turns one batch of committed row changes into
//! the input-cell [`Edit`]s it makes (one per touched cell, its final value),
//! or says the batch cannot be applied cell by cell ([`Rebuild`]) and the
//! caller must reload the model: a TRUNCATE, a dimension value with no item
//! yet, two rows on one coordinate, an aggregated binding, a value the
//! loader would not read, or an old row whose dimension values were not
//! replicated.
//!
//! Values are the `pgoutput` text of the column, read by the measure's type
//! as the loader's `::float8`/`::bool`/`::text`/`timestamptz` casts read the
//! row. A date or timestamp without a zone is taken as UTC: the sync worker
//! runs with `TimeZone = UTC` and `DateStyle = ISO`.
//!
//! Replay: a rebuild reads the bound tables as they are *now*, which may
//! already include transactions the slot has not yet delivered. Those are
//! re-applied afterwards. Each change is a per-cell "remove" or "set", so
//! re-applying converges (the last change to a cell wins); the one check
//! that is not idempotent, an insert onto an occupied coordinate, overwrites
//! instead of rebuilding for transactions that ended by `replay_until`.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use improv_core_model::{CategoryId, Coordinate, ItemId, MeasureId, Model, Value, ValueType};
use improv_engine::session::Edit;
use improv_engine::{encode_coord, CellValue};

use crate::pgoutput::{Datum, Message, Relation, Tuple, Txn};

/// How one input measure is bound to the columns of one relation.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    /// The bound input measure.
    pub measure: MeasureId,
    /// The value column.
    pub value_col: String,
    /// One `(category, column)` per dimension, in the measure's category order.
    pub dims: Vec<(CategoryId, String)>,
    /// Several rows may share a coordinate (`improv.binding.agg`): a change
    /// to such a binding always rebuilds.
    pub aggregated: bool,
}

/// Each cell a batch touched, with its value before the batch.
type Touched = BTreeMap<(MeasureId, Coordinate), Option<Value>>;

/// Why a batch cannot be applied incrementally; the caller rebuilds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rebuild(pub &'static str);

/// Maps row changes of bound relations to input-cell edits.
#[derive(Debug, Clone, Default)]
pub struct Mapper {
    /// Bindings per relation oid.
    bindings: HashMap<u32, Vec<Binding>>,
    /// Item ids per category, by name.
    items: HashMap<CategoryId, HashMap<String, ItemId>>,
}

impl Mapper {
    /// A mapper for `model` (its items name the dimension values) with
    /// `bindings` per relation oid.
    pub fn new(model: &Model, bindings: HashMap<u32, Vec<Binding>>) -> Self {
        let mut items: HashMap<CategoryId, HashMap<String, ItemId>> = HashMap::new();
        for i in model.items.values() {
            items
                .entry(i.category)
                .or_default()
                .insert(i.name.0.clone(), i.id);
        }
        Mapper { bindings, items }
    }

    /// Whether relation `oid` feeds the model.
    pub fn binds(&self, oid: u32) -> bool {
        self.bindings.contains_key(&oid)
    }

    /// Apply the changes of `txns` (tuples of the relations in `rels`) to
    /// `model`'s input cells, as the loader would see the rows, and return
    /// one edit per cell whose value the batch changed, with its final value
    /// (a cell set back to its old value makes none). Changes to relations
    /// without a binding are ignored. Transactions that ended at or before
    /// `replay_until` may already be in `model` (see the module docs). On
    /// `Err` the model's inputs are half-updated: discard the model.
    pub fn map(
        &self,
        model: &mut Model,
        rels: &HashMap<u32, Relation>,
        txns: &[Txn],
        replay_until: u64,
    ) -> Result<Vec<Edit>, Rebuild> {
        // Each touched cell, with its value before the batch.
        let mut touched = BTreeMap::new();
        for txn in txns {
            let replay = txn.end_lsn <= replay_until;
            for change in &txn.changes {
                self.change(model, rels, change, replay, &mut touched)?;
            }
        }
        // As engine values: numbers compare by bits, like the engine's.
        let cell = |v: Option<&Value>| v.and_then(CellValue::from_model_value);
        Ok(touched
            .into_iter()
            .filter_map(|((measure, c), before): (_, Option<Value>)| {
                let now = cell(model.inputs.get(&(measure, c.clone())));
                (now != cell(before.as_ref())).then(|| Edit {
                    value: now,
                    coord: encode_coord(&c),
                    measure,
                })
            })
            .collect())
    }

    fn change(
        &self,
        model: &mut Model,
        rels: &HashMap<u32, Relation>,
        change: &Message,
        replay: bool,
        touched: &mut Touched,
    ) -> Result<(), Rebuild> {
        // `old`: None = no old row (insert); Some(None) = an update that
        // did not send its old row (the replica identity is unchanged).
        let (rel, old, new) = match change {
            Message::Insert { rel, new } => (*rel, None, Some(new)),
            Message::Update { rel, old, new } => (*rel, Some(old.as_ref()), Some(new)),
            Message::Delete { rel, old } => (*rel, Some(Some(old)), None),
            Message::Truncate { rels, .. } if rels.iter().any(|r| self.binds(*r)) => {
                return Err(Rebuild("truncate"))
            }
            _ => return Ok(()),
        };
        let Some(bindings) = self.bindings.get(&rel) else {
            return Ok(());
        };
        let r = rels.get(&rel).ok_or(Rebuild("undescribed relation"))?;
        for b in bindings {
            self.row(model, r, b, (old, new), replay, touched)?;
        }
        Ok(())
    }

    /// One row change through one binding: retract the old row's cell, then
    /// assert the new row's.
    fn row(
        &self,
        model: &mut Model,
        r: &Relation,
        b: &Binding,
        (old, new): (Option<Option<&Tuple>>, Option<&Tuple>),
        replay: bool,
        touched: &mut Touched,
    ) -> Result<(), Rebuild> {
        if b.aggregated {
            return Err(Rebuild("aggregated binding"));
        }
        let vt = model
            .measures
            .get(&b.measure)
            .ok_or(Rebuild("unknown measure"))?
            .value_type;
        // REPLICA IDENTITY FULL sends the whole old row. A key identity sends
        // only the key (and nothing when it is unchanged); usable only when
        // the key is exactly the dimension columns, so the old dimension
        // values are known and the coordinate names one row.
        let keyed = || {
            let keys: BTreeSet<&str> = r
                .columns
                .iter()
                .filter(|c| c.key)
                .map(|c| c.name.as_str())
                .collect();
            keys == b.dims.iter().map(|(_, c)| c.as_str()).collect()
        };
        let old = match old {
            None => None,
            Some(Some(t)) if r.replica_identity == b'f' => Some((t, true)),
            Some(t) if r.replica_identity != b'f' && keyed() => {
                Some((t.or(new).ok_or(Rebuild("no row"))?, false))
            }
            Some(_) => return Err(Rebuild("old row not replicated")),
        };
        if let Some((t, value_known)) = old {
            if let Some(c) = self.coord(r, b, t)? {
                // A NULL-valued row was no cell (and may share its coordinate
                // with one that is).
                if !value_known || value(r, b, t, vt)?.is_some() {
                    let k = (b.measure, c);
                    let before = model.inputs.remove(&k);
                    touched.entry(k).or_insert(before);
                }
            }
        }
        if let Some(t) = new {
            if let (Some(c), Some(v)) = (self.coord(r, b, t)?, value(r, b, t, vt)?) {
                let k = (b.measure, c);
                if !replay && model.inputs.contains_key(&k) {
                    return Err(Rebuild("two rows on one coordinate"));
                }
                let before = model.inputs.insert(k.clone(), v);
                touched.entry(k).or_insert(before);
            }
        }
        Ok(())
    }

    /// The coordinate of tuple `t`; `None` when a dimension is NULL (the
    /// loader skips such rows).
    fn coord(&self, r: &Relation, b: &Binding, t: &Tuple) -> Result<Option<Coordinate>, Rebuild> {
        let mut pairs = Vec::with_capacity(b.dims.len());
        for (cat, col) in &b.dims {
            let name = match r.get(t, col) {
                None => return Err(Rebuild("bound column missing")),
                Some(Datum::Null) => return Ok(None),
                Some(Datum::Unchanged) => return Err(Rebuild("unchanged TOAST value")),
                Some(Datum::Text(s)) => s,
            };
            let item = self
                .items
                .get(cat)
                .and_then(|m| m.get(name))
                .ok_or(Rebuild("new item"))?;
            pairs.push((*cat, *item));
        }
        Ok(Some(Coordinate::from_pairs(pairs)))
    }
}

/// `bpchar`'s oid: its text keeps the padding `::text` strips.
const BPCHAR: u32 = 1042;

/// The value of tuple `t` as a `vt` (`None` = SQL NULL).
fn value(r: &Relation, b: &Binding, t: &Tuple, vt: ValueType) -> Result<Option<Value>, Rebuild> {
    let s = match r.get(t, &b.value_col) {
        None => return Err(Rebuild("bound column missing")),
        Some(Datum::Null) => return Ok(None),
        Some(Datum::Unchanged) => return Err(Rebuild("unchanged TOAST value")),
        Some(Datum::Text(s)) => s,
    };
    let v = match vt {
        ValueType::Number => s.parse().ok().map(Value::Number),
        ValueType::Boolean => match s.as_str() {
            "t" => Some(Value::Boolean(true)),
            "f" => Some(Value::Boolean(false)),
            _ => None,
        },
        ValueType::Text => {
            let bpchar = r
                .columns
                .iter()
                .any(|c| c.name == b.value_col && c.type_oid == BPCHAR);
            Some(Value::Text(
                if bpchar { s.trim_end_matches(' ') } else { s }.to_string(),
            ))
        }
        ValueType::DateTime => datetime(s).map(Value::DateTime),
        ValueType::Enum => None,
    };
    v.map(Some)
        .ok_or(Rebuild("value the loader would not read"))
}

/// ISO `timestamptz`, `timestamp` or `date` text (the last two in UTC) at
/// the millisecond, rounded half away from zero as the loader's
/// `(extract(epoch from v) * 1000)::int8` rounds. `None` for anything else
/// (`infinity`, BC dates), which the loader cannot read either.
fn datetime(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{DateTime, NaiveDate, NaiveDateTime};
    let t = DateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f%#z")
        .map(|d| d.naive_utc())
        .or_else(|_| NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f"))
        .or_else(|_| {
            NaiveDate::parse_from_str(s, "%Y-%m-%d").map(|d| d.and_time(Default::default()))
        })
        .ok()?
        .and_utc();
    let us = t.timestamp_micros();
    let ms = us / 1000 + (us % 1000) / 500;
    DateTime::from_timestamp_millis(ms)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::pgoutput::Column;
    use crate::{build_model, Catalog, CategoryRow, ItemRow, MeasureRow};
    use improv_engine::CoordKey;

    const F: u32 = 7;
    const OTHER: u32 = 8;

    fn item(id: u32, category_id: u32, name: &str) -> ItemRow {
        ItemRow {
            id,
            category_id,
            name: name.into(),
        }
    }

    /// Time {Q1=11, Q2=12} × Product {P1=21}; input Price (5); Price[Q1,P1] = 1.
    fn model() -> Model {
        let mut m = build_model(&Catalog {
            categories: vec![
                CategoryRow {
                    id: 1,
                    name: "Time".into(),
                },
                CategoryRow {
                    id: 2,
                    name: "Product".into(),
                },
            ],
            items: vec![item(11, 1, "Q1"), item(12, 1, "Q2"), item(21, 2, "P1")],
            measures: vec![MeasureRow::input(5, "Price", vec![1, 2])],
            lambdas: vec![],
        })
        .unwrap();
        m.set_input(MeasureId(5), coordinate(11, 21), Value::Number(1.0));
        m
    }

    fn coordinate(q: u32, p: u32) -> Coordinate {
        Coordinate::from_pairs([(CategoryId(1), ItemId(q)), (CategoryId(2), ItemId(p))])
    }

    fn key(q: u32, p: u32) -> CoordKey {
        vec![(1, q), (2, p)]
    }

    /// Relation `f (quarter, product, price, note)` with `keys` as its key.
    fn rels(identity: u8, keys: &[&str]) -> HashMap<u32, Relation> {
        let col = |n: &str| Column {
            key: keys.contains(&n),
            name: n.into(),
            type_oid: 25,
            type_mod: -1,
        };
        let f = Relation {
            id: F,
            namespace: "public".into(),
            name: "f".into(),
            replica_identity: identity,
            columns: ["quarter", "product", "price", "note"]
                .into_iter()
                .map(col)
                .collect(),
        };
        HashMap::from([(F, f)])
    }

    fn mapper(m: &Model, aggregated: bool) -> Mapper {
        let b = Binding {
            measure: MeasureId(5),
            value_col: "price".into(),
            dims: vec![
                (CategoryId(1), "quarter".into()),
                (CategoryId(2), "product".into()),
            ],
            aggregated,
        };
        Mapper::new(m, HashMap::from([(F, vec![b])]))
    }

    fn row(q: Option<&str>, p: Option<&str>, v: Option<&str>) -> Tuple {
        [q, p, v, Some("x")]
            .into_iter()
            .map(|d| d.map_or(Datum::Null, |s| Datum::Text(s.into())))
            .collect()
    }

    fn full(q: &str, v: &str) -> Tuple {
        row(Some(q), Some("P1"), Some(v))
    }

    type Out = Result<Vec<(CoordKey, Option<f64>)>, Rebuild>;

    /// Map `changes` over a fresh model with a FULL-identity `f`.
    fn run(changes: Vec<Message>) -> Out {
        run_with(rels(b'f', &[]), false, changes)
    }

    fn txn(changes: Vec<Message>) -> Vec<Txn> {
        vec![Txn {
            xid: 1,
            end_lsn: 10,
            changes,
        }]
    }

    fn run_with(rels: HashMap<u32, Relation>, aggregated: bool, changes: Vec<Message>) -> Out {
        let mut m = model();
        let edits = mapper(&m, aggregated).map(&mut m, &rels, &txn(changes), 0)?;
        assert!(edits.iter().all(|e| e.measure == MeasureId(5)));
        Ok(edits
            .into_iter()
            .map(|e| (e.coord, e.value.map(|v| v.as_num().expect("a number"))))
            .collect())
    }

    fn ins(t: Tuple) -> Message {
        Message::Insert { rel: F, new: t }
    }

    fn upd(old: Option<Tuple>, new: Tuple) -> Message {
        Message::Update { rel: F, old, new }
    }

    fn del(t: Tuple) -> Message {
        Message::Delete { rel: F, old: t }
    }

    #[test]
    fn insert_update_delete_map_to_set_and_remove() {
        assert_eq!(
            run(vec![ins(full("Q2", "3"))]),
            Ok(vec![(key(12, 21), Some(3.0))])
        );
        assert_eq!(
            run(vec![upd(Some(full("Q1", "1")), full("Q1", "4.5"))]),
            Ok(vec![(key(11, 21), Some(4.5))])
        );
        assert_eq!(
            run(vec![del(full("Q1", "1"))]),
            Ok(vec![(key(11, 21), None)])
        );
    }

    #[test]
    fn a_dimension_change_moves_the_cell() {
        assert_eq!(
            run(vec![upd(Some(full("Q1", "1")), full("Q2", "1"))]),
            Ok(vec![(key(11, 21), None), (key(12, 21), Some(1.0))])
        );
    }

    #[test]
    fn a_batch_yields_each_cells_final_value() {
        assert_eq!(
            run(vec![
                ins(full("Q2", "3")),
                upd(Some(full("Q2", "3")), full("Q2", "5")),
                del(full("Q1", "1")),
                ins(full("Q1", "2")),
            ]),
            Ok(vec![(key(11, 21), Some(2.0)), (key(12, 21), Some(5.0))])
        );
        // Net no-ops make no edit.
        assert_eq!(
            run(vec![ins(full("Q2", "3")), del(full("Q2", "3"))]),
            Ok(vec![])
        );
        assert_eq!(
            run(vec![
                upd(Some(full("Q1", "1")), full("Q1", "2")),
                upd(Some(full("Q1", "2")), full("Q1", "1")),
            ]),
            Ok(vec![])
        );
    }

    #[test]
    fn nulls_follow_the_loader() {
        // A NULL dimension is no row; a NULL value is no cell.
        assert_eq!(run(vec![ins(row(None, Some("P1"), Some("3")))]), Ok(vec![]));
        assert_eq!(
            run(vec![ins(row(Some("Q2"), Some("P1"), None))]),
            Ok(vec![])
        );
        assert_eq!(
            run(vec![upd(
                Some(full("Q1", "1")),
                row(Some("Q1"), Some("P1"), None)
            )]),
            Ok(vec![(key(11, 21), None)])
        );
        assert_eq!(
            run(vec![upd(
                Some(full("Q1", "1")),
                row(None, Some("P1"), Some("1"))
            )]),
            Ok(vec![(key(11, 21), None)])
        );
        // Deleting a NULL-valued row on Price[Q1,P1] keeps the cell.
        assert_eq!(
            run(vec![del(row(Some("Q1"), Some("P1"), None))]),
            Ok(vec![])
        );
        assert_eq!(
            run(vec![upd(
                Some(row(Some("Q2"), Some("P1"), None)),
                full("Q2", "7")
            )]),
            Ok(vec![(key(12, 21), Some(7.0))])
        );
    }

    #[test]
    fn unbound_relations_and_columns_are_ignored() {
        let mut m = model();
        let mut r = rels(b'f', &[]);
        let mut other = r[&F].clone();
        other.id = OTHER;
        r.insert(OTHER, other);
        let changes = txn(vec![
            Message::Insert {
                rel: OTHER,
                new: full("Q2", "3"),
            },
            Message::Truncate {
                options: 0,
                rels: vec![OTHER],
            },
            // Only `note` changed: the cell keeps its value, no edit.
            upd(Some(full("Q1", "1")), full("Q1", "1")),
        ]);
        let edits = mapper(&m, false).map(&mut m, &r, &changes, 0).unwrap();
        assert!(edits.is_empty());
    }

    #[test]
    fn what_cannot_be_applied_cell_by_cell_rebuilds() {
        let truncate = Message::Truncate {
            options: 0,
            rels: vec![OTHER, F],
        };
        assert_eq!(run(vec![truncate]), Err(Rebuild("truncate")));
        assert_eq!(run(vec![ins(full("Q3", "1"))]), Err(Rebuild("new item")));
        assert_eq!(
            run(vec![ins(full("Q1", "2"))]),
            Err(Rebuild("two rows on one coordinate"))
        );
        assert_eq!(
            run(vec![ins(full("Q2", "abc"))]),
            Err(Rebuild("value the loader would not read"))
        );
        let mut t = full("Q2", "1");
        t[2] = Datum::Unchanged;
        assert_eq!(run(vec![ins(t)]), Err(Rebuild("unchanged TOAST value")));
        assert_eq!(
            run_with(rels(b'f', &[]), true, vec![ins(full("Q2", "1"))]),
            Err(Rebuild("aggregated binding"))
        );
        let mut no_price = rels(b'f', &[]);
        no_price.get_mut(&F).unwrap().columns[2].name = "cost".into();
        assert_eq!(
            run_with(no_price, false, vec![ins(full("Q2", "1"))]),
            Err(Rebuild("bound column missing"))
        );
        let mut m = model();
        let changes = txn(vec![ins(full("Q2", "1"))]);
        assert_eq!(
            mapper(&m, false)
                .map(&mut m, &HashMap::new(), &changes, 0)
                .err(),
            Some(Rebuild("undescribed relation"))
        );
        let mut gone = model();
        let m = mapper(&gone, false);
        gone.measures.clear();
        assert_eq!(
            m.map(&mut gone, &rels(b'f', &[]), &changes, 0).err(),
            Some(Rebuild("unknown measure"))
        );
    }

    /// Map one insert of `price` text onto Price[Q2,P1], with Price of type
    /// `vt` in a column of type `type_oid`.
    fn typed(vt: ValueType, type_oid: u32, price: &str) -> Result<Option<CellValue>, Rebuild> {
        let mut m = model();
        m.measures.get_mut(&MeasureId(5)).unwrap().value_type = vt;
        let mut r = rels(b'f', &[]);
        r.get_mut(&F).unwrap().columns[2].type_oid = type_oid;
        let edits = mapper(&m, false).map(&mut m, &r, &txn(vec![ins(full("Q2", price))]), 0)?;
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].coord, key(12, 21));
        Ok(edits[0].value.clone())
    }

    #[test]
    fn typed_bindings_read_values_as_the_loader_does() {
        use ValueType::{Boolean, DateTime, Enum, Text};
        let ok = |v| Ok(Some(v));
        assert_eq!(typed(Boolean, 16, "t"), ok(CellValue::Bool(true)));
        assert_eq!(typed(Boolean, 16, "f"), ok(CellValue::Bool(false)));
        assert_eq!(typed(Text, 25, "a b "), ok(CellValue::Text("a b ".into())));
        // `char(n)` pads; `::text` strips the padding.
        assert_eq!(
            typed(Text, BPCHAR, "ab  "),
            ok(CellValue::Text("ab".into()))
        );
        let day = 19_723 * 86_400_000; // 2024-01-01
        assert_eq!(
            typed(DateTime, 1082, "2024-01-01"),
            ok(CellValue::Date(day))
        );
        assert_eq!(
            typed(DateTime, 1114, "2024-01-01 00:00:01.5"),
            ok(CellValue::Date(day + 1500))
        );
        assert_eq!(
            typed(DateTime, 1184, "2024-01-01 05:30:00.0005+05:30"),
            ok(CellValue::Date(day + 1))
        );
        assert_eq!(
            typed(DateTime, 1184, "1969-12-31 23:59:59.9985+00"),
            ok(CellValue::Date(-2))
        );
        let no = Err(Rebuild("value the loader would not read"));
        for (vt, s) in [(Boolean, "true"), (DateTime, "infinity"), (Enum, "1")] {
            assert_eq!(typed(vt, 25, s), no, "{vt:?} {s}");
        }
    }

    #[test]
    fn non_finite_values_parse_like_float8() {
        assert_eq!(
            run(vec![ins(full("Q2", "-Infinity"))]),
            Ok(vec![(key(12, 21), Some(f64::NEG_INFINITY))])
        );
    }

    #[test]
    fn a_key_identity_works_only_when_the_key_is_the_dimensions() {
        let pk = || rels(b'd', &["quarter", "product"]);
        // The old row is key-only: its value is unknown, the coordinate unique.
        let key_only = row(Some("Q1"), Some("P1"), None);
        assert_eq!(
            run_with(pk(), false, vec![del(key_only.clone())]),
            Ok(vec![(key(11, 21), None)])
        );
        assert_eq!(
            run_with(pk(), false, vec![upd(None, full("Q1", "9"))]),
            Ok(vec![(key(11, 21), Some(9.0))])
        );
        assert_eq!(
            run_with(pk(), false, vec![upd(Some(key_only), full("Q2", "9"))]),
            Ok(vec![(key(11, 21), None), (key(12, 21), Some(9.0))])
        );
        // A key that is not the dimensions cannot name the old cell.
        let id_key = || rels(b'd', &["note"]);
        assert_eq!(
            run_with(id_key(), false, vec![ins(full("Q2", "1"))]),
            Ok(vec![(key(12, 21), Some(1.0))])
        );
        assert_eq!(
            run_with(id_key(), false, vec![upd(None, full("Q1", "9"))]),
            Err(Rebuild("old row not replicated"))
        );
        assert_eq!(
            run_with(id_key(), false, vec![del(full("Q1", "1"))]),
            Err(Rebuild("old row not replicated"))
        );
    }

    #[test]
    fn replayed_transactions_converge() {
        // Price[Q1,P1] = 1 is already loaded; the slot re-delivers its insert
        // and a later update, then a fresh insert onto the same cell.
        let mut m = model();
        let mp = mapper(&m, false);
        let r = rels(b'f', &[]);
        let replayed = vec![
            Txn {
                xid: 1,
                end_lsn: 10,
                changes: vec![ins(full("Q1", "1"))],
            },
            Txn {
                xid: 2,
                end_lsn: 20,
                changes: vec![upd(Some(full("Q1", "1")), full("Q1", "2"))],
            },
        ];
        let edits = mp.map(&mut m, &r, &replayed, 20).unwrap();
        assert_eq!(
            (edits.len(), edits[0].value.clone()),
            (1, Some(CellValue::num(2.0)))
        );
        assert!(mp.map(&mut m, &r, &replayed, 20).unwrap().is_empty());
        let later = txn(vec![ins(full("Q1", "3"))]);
        assert_eq!(
            mp.map(&mut m, &r, &later, 9).err(),
            Some(Rebuild("two rows on one coordinate"))
        );
    }
}
