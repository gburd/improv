//! G1: map decoded `pgoutput` row changes onto live-engine edits.
//!
//! The logical-decoding worker keeps a live [`improv_engine::session::Engine`]
//! per model. [`Mapper::map`] turns one batch of committed row changes into
//! the input-cell [`Edit`]s it makes (one per touched cell, its final value),
//! or says the batch cannot be applied cell by cell ([`Rebuild`]) and the
//! caller must reload the model: two rows on one coordinate of a binding
//! without `agg`, a value the loader would not read, or an old row whose
//! values were not replicated. A TRUNCATE removes every cell its
//! relation's bindings feed. Dimension values with no item yet are listed
//! by [`Mapper::unknown`]: the caller mints them (`improv.mint_items`) and
//! adds them ([`Mapper::add_item`]) before mapping.
//!
//! Aggregated bindings (`improv.binding.agg`) keep per-coordinate fold state
//! (`Fold`): the row counts, an exact total for `sum` and `avg`, and for
//! `min`, `max` and `count_distinct` each distinct value's row count, so
//! deleting one of two rows that share a value keeps it. [`Mapper::seed`]
//! loads that state at a rebuild; each change then removes its old row from
//! its coordinate's fold and adds its new row to its (maybe other)
//! coordinate's, and each touched cell takes its fold's value.
//!
//! Values are the `pgoutput` text of the column, read by the measure's type
//! as the loader's `::float8`/`::bool`/`::text`/`timestamptz` casts read the
//! row. A date or timestamp without a zone is taken as UTC: the sync worker
//! runs with `TimeZone = UTC` and `DateStyle = ISO`.
//!
//! Replay: a rebuild reads the bound tables as they are *now*, which may
//! already include transactions the slot has not yet delivered. Those are
//! re-applied afterwards. Each change to a binding without `agg` is a
//! per-cell "remove" or "set", so re-applying converges (the last change to
//! a cell wins); the one check that is not idempotent, an insert onto an
//! occupied coordinate, overwrites instead of rebuilding for transactions
//! that ended by `replay_until`. A fold is not idempotent (a re-applied
//! insert counts twice), so each aggregated binding's seed query also
//! returns the snapshot it read with ([`Snap`], `pg_current_snapshot()` in
//! the same statement), and [`Mapper::map`] skips exactly the transactions
//! that snapshot saw.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::sync::Arc;

use improv_core_model::{CategoryId, Coordinate, ItemId, MeasureId, Model, Value, ValueType};
use improv_engine::session::Edit;
use improv_engine::{encode_coord, CellValue};

use crate::pgoutput::{Datum, Message, Relation, Tuple, Txn};

/// How the rows on one coordinate fold into its cell (`improv.binding.agg`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agg {
    /// `sum(v)`.
    Sum,
    /// `avg(v)`.
    Avg,
    /// `min(v)`.
    Min,
    /// `max(v)`.
    Max,
    /// `count(v)`.
    Count,
    /// `count(DISTINCT v)`.
    CountDistinct,
}

impl Agg {
    /// The aggregate a `binding.agg` names; `None` for anything else.
    pub fn parse(s: &str) -> Option<Agg> {
        Some(match s {
            "sum" => Agg::Sum,
            "avg" => Agg::Avg,
            "min" => Agg::Min,
            "max" => Agg::Max,
            "count" => Agg::Count,
            "count_distinct" => Agg::CountDistinct,
            _ => return None,
        })
    }
}

/// How one input measure is bound to the columns of one relation.
#[derive(Debug, Clone, PartialEq)]
pub struct Binding {
    /// The bound input measure.
    pub measure: MeasureId,
    /// The value column.
    pub value_col: String,
    /// One `(category, column)` per dimension, in the measure's category order.
    pub dims: Vec<(CategoryId, String)>,
    /// Several rows may share a coordinate, folded by this aggregate
    /// (`improv.binding.agg`).
    pub agg: Option<Agg>,
}

/// Compares two texts as Postgres does under one collation.
pub type TextCmp = dyn Fn(&str, &str) -> Ordering + Send + Sync;

/// How a binding's text values compare: its value column's collation, as
/// the caller reads it from Postgres ([`Mapper::collate`]).
#[derive(Clone)]
pub enum Collation {
    /// A deterministic collation: two texts are equal only when their bytes
    /// are, and they order by this compare (Postgres's `varstr_cmp` under
    /// the collation; byte order for `C` and `POSIX`).
    Deterministic(Arc<TextCmp>),
    /// A nondeterministic collation: texts with different bytes may be
    /// equal, which only Postgres can tell, so a text `min` or `max` and a
    /// `count_distinct` rebuild on every change.
    Nondeterministic,
}

impl fmt::Debug for Collation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Collation::Deterministic(_) => "Deterministic",
            Collation::Nondeterministic => "Nondeterministic",
        })
    }
}

/// Each cell a batch touched, with its value before the batch.
type Touched = BTreeMap<(MeasureId, Coordinate), Option<Value>>;

/// Item ids per category, by name.
type Items = HashMap<CategoryId, HashMap<String, ItemId>>;

/// One row of [`seed_sql`]: dimension names, a value's text, how many rows
/// with a value it stands for, and how many rows in all.
pub type FoldRow = (Vec<Option<String>>, Option<String>, i64, i64);

/// A Postgres snapshot (`pg_current_snapshot()`): which transactions a
/// query saw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snap {
    xmin: u64,
    xmax: u64,
    xip: BTreeSet<u64>,
}

impl Snap {
    /// From `pg_snapshot` text, `xmin:xmax:xip,…`.
    pub fn parse(s: &str) -> Option<Snap> {
        let mut it = s.split(':');
        let xmin = it.next()?.parse().ok()?;
        let xmax = it.next()?.parse().ok()?;
        let xip = match it.next()? {
            "" => BTreeSet::new(),
            l => l
                .split(',')
                .map(|x| x.parse().ok())
                .collect::<Option<_>>()?,
        };
        it.next().is_none().then_some(Snap { xmin, xmax, xip })
    }

    /// Whether committed transaction `xid` (the 32 bits `pgoutput` sends,
    /// within 2³¹ of the snapshot) was visible to the snapshot.
    pub fn sees(&self, xid: u32) -> bool {
        let d = i64::from(xid.wrapping_sub(self.xmax as u32) as i32);
        let full = self.xmax as i64 + d;
        full < self.xmin as i64 || (full < self.xmax as i64 && !self.xip.contains(&(full as u64)))
    }
}

/// The `SELECT` that loads an `agg` binding's fold state for
/// [`Mapper::seed`]: per coordinate (dimension columns as text, `rel`
/// already quoted), the value text the fold keeps, `count(value_col)` and
/// `count(*)` ([`FoldRow`]), then a NULL. For `count` no value; for `sum` and
/// `avg` the finite values' sum and, in groups of their own, the infinities
/// and NaNs (and for an `avg` of `numeric`, one group per scale); for `min`,
/// `max` and `count_distinct` one group per value text.
/// One more row has NULL dimensions and, last, the statement's snapshot.
pub fn seed_sql(rel: &str, dim_cols: &[String], value_col: &str, agg: Agg) -> String {
    let nulls = vec!["NULL"; dim_cols.len()].join(", ");
    format!(
        "({}) UNION ALL SELECT {nulls}, NULL, 0, 0, pg_current_snapshot()::text",
        group_sql(rel, dim_cols, value_col, agg)
    )
}

fn group_sql(rel: &str, dim_cols: &[String], value_col: &str, agg: Agg) -> String {
    let dims: Vec<String> = dim_cols
        .iter()
        .map(|c| format!("{}::text", crate::quote_ident(c)))
        .collect();
    let by: Vec<String> = (1..=dims.len()).map(|i| i.to_string()).collect();
    let (dims, by) = (dims.join(", "), by.join(", "));
    let v = crate::quote_ident(value_col);
    match agg {
        Agg::Count => {
            format!(
                "SELECT {dims}, NULL::text, count({v}), count(*), NULL::text FROM {rel} \
                 GROUP BY {by}"
            )
        }
        Agg::Sum => format!(
            "SELECT {dims}, coalesce(improv_special, sum({v})::text), count({v}), count(*), \
                    NULL::text \
             FROM (SELECT *, CASE WHEN {v}::text IN ('Infinity', '-Infinity', 'NaN') \
                   THEN {v}::text END AS improv_special FROM {rel}) t \
             GROUP BY {by}, improv_special"
        ),
        // A numeric's average keeps its values' largest scale: one group per
        // scale, so the fold knows each scale's rows ([`Total::count`]).
        Agg::Avg => format!(
            "SELECT {dims}, coalesce(improv_special, sum({v})::text), count({v}), count(*), \
                    NULL::text \
             FROM (SELECT *, CASE WHEN {v}::text IN ('Infinity', '-Infinity', 'NaN') \
                   THEN {v}::text END AS improv_special, \
                   CASE WHEN pg_typeof({v}) = 'numeric'::regtype \
                   THEN scale({v}::numeric) END AS improv_scale FROM {rel}) t \
             GROUP BY {by}, improv_special, improv_scale"
        ),
        Agg::Min | Agg::Max | Agg::CountDistinct => format!(
            "SELECT {dims}, {v}::text, count({v}), count(*), NULL::text FROM {rel} \
             GROUP BY {by}, {}",
            dim_cols.len() + 1
        ),
    }
}

/// Split one decoded batch by model: each model gets, in commit order, the
/// transactions that changed a relation it binds (`models` per relation
/// oid), holding only those changes. A `TRUNCATE` of several relations goes
/// whole to each model that binds one of them ([`Mapper::map`] skips the
/// rest). Models no change reaches are absent.
pub fn route(txns: &[Txn], models: &HashMap<u32, Vec<i64>>) -> BTreeMap<i64, Vec<Txn>> {
    let mut out: BTreeMap<i64, Vec<Txn>> = BTreeMap::new();
    for t in txns {
        let mut per: BTreeMap<i64, Vec<Message>> = BTreeMap::new();
        for c in &t.changes {
            let rels: &[u32] = match c {
                Message::Insert { rel, .. }
                | Message::Update { rel, .. }
                | Message::Delete { rel, .. } => std::slice::from_ref(rel),
                Message::Truncate { rels, .. } => rels,
                _ => &[],
            };
            let hit: BTreeSet<i64> = rels
                .iter()
                .flat_map(|r| models.get(r).into_iter().flatten().copied())
                .collect();
            for m in hit {
                per.entry(m).or_default().push(c.clone());
            }
        }
        for (m, changes) in per {
            out.entry(m).or_default().push(Txn {
                xid: t.xid,
                end_lsn: t.end_lsn,
                changes,
            });
        }
    }
    out
}

/// Why a batch cannot be applied incrementally; the caller rebuilds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rebuild(pub &'static str);

/// Maps row changes of bound relations to input-cell edits.
#[derive(Debug, Clone, Default)]
pub struct Mapper {
    /// Bindings per relation oid.
    bindings: HashMap<u32, Vec<Binding>>,
    /// Item ids per category, by name.
    items: Items,
    /// Aggregated bindings' fold state, per measure and coordinate.
    folds: HashMap<MeasureId, HashMap<Coordinate, Fold>>,
    /// Per aggregated measure, the snapshot its fold was seeded with.
    seeded: HashMap<MeasureId, Snap>,
    /// Per aggregated measure over a text column, its collation.
    collations: HashMap<MeasureId, Collation>,
}

impl Mapper {
    /// A mapper for `model` (its items name the dimension values) with
    /// `bindings` per relation oid. Aggregated bindings start with no rows:
    /// [`Mapper::seed`] them.
    pub fn new(model: &Model, bindings: HashMap<u32, Vec<Binding>>) -> Self {
        let mut items: Items = HashMap::new();
        for i in model.items.values() {
            items
                .entry(i.category)
                .or_default()
                .insert(i.name.0.clone(), i.id);
        }
        Mapper {
            bindings,
            items,
            folds: HashMap::new(),
            seeded: HashMap::new(),
            collations: HashMap::new(),
        }
    }

    /// Compare aggregated measure `measure`'s text values under `c`, its
    /// value column's collation; call it before [`Mapper::seed`]. Without
    /// one, a text `min` or `max` rebuilds on every change and
    /// `count_distinct` finds two texts equal when their bytes are.
    pub fn collate(&mut self, measure: MeasureId, c: Collation) {
        self.collations.insert(measure, c);
    }

    /// The aggregated bindings, with their relation oids: each needs a
    /// [`Mapper::seed`].
    pub fn aggregated(&self) -> Vec<(u32, Binding)> {
        let mut v: Vec<(u32, Binding)> = self
            .bindings
            .iter()
            .flat_map(|(r, bs)| bs.iter().map(|b| (*r, b.clone())))
            .filter(|(_, b)| b.agg.is_some())
            .collect();
        v.sort_by_key(|(_, b)| b.measure);
        v
    }

    /// An estimate of the heap the fold state holds, in bytes: each
    /// measure's map of coordinates (its buckets, a control byte each), each
    /// coordinate's B-tree, the values a `min`, `max` or `count_distinct`
    /// keeps (their B-tree and texts), and a float sum's exact accumulator.
    /// Within 5 % of glibc's live heap at 300k coordinates. The model's
    /// cells are the caller's to count.
    pub fn fold_bytes(&self) -> usize {
        let entry = size_of::<(Coordinate, Fold)>() + 1;
        let tree = |len: usize, kv: usize| len.div_ceil(7) * chunk(16 + 11 * kv);
        let exact = chunk(size_of::<Exact>());
        let fold = |(c, f): (&Coordinate, &Fold)| {
            tree(c.dims.len(), size_of::<(CategoryId, ItemId)>())
                + tree(f.values.len(), size_of::<(Key, i64)>())
                + f.values.keys().map(Key::bytes).sum::<usize>()
                + f.total.float.as_ref().map_or(0, |_| exact)
        };
        self.folds
            .values()
            .map(|m| buckets(m.capacity()) * entry + m.iter().map(fold).sum::<usize>())
            .sum()
    }

    /// Whether relation `oid` feeds the model.
    pub fn binds(&self, oid: u32) -> bool {
        self.bindings.contains_key(&oid)
    }

    /// The dimension values in `txns`' rows of bound relations (old rows
    /// and new, as the loader's `::text` reads them) that have no item yet,
    /// sorted. As the loader does, a row's non-NULL values count even when
    /// another of its dimensions is NULL.
    pub fn unknown(
        &self,
        rels: &HashMap<u32, Relation>,
        txns: &[Txn],
    ) -> Vec<(CategoryId, String)> {
        let mut out = BTreeSet::new();
        for change in txns.iter().flat_map(|t| &t.changes) {
            let (rel, tuples) = match change {
                Message::Insert { rel, new } => (rel, [None, Some(new)]),
                Message::Update { rel, old, new } => (rel, [old.as_ref(), Some(new)]),
                Message::Delete { rel, old } => (rel, [Some(old), None]),
                _ => continue,
            };
            let (Some(bs), Some(r)) = (self.bindings.get(rel), rels.get(rel)) else {
                continue;
            };
            for (b, t) in bs
                .iter()
                .flat_map(|b| tuples.iter().flatten().map(move |t| (b, t)))
            {
                for (cat, col) in &b.dims {
                    let Ok(Some(name)) = dim_name(r, col, t) else {
                        continue;
                    };
                    if !self.items.get(cat).is_some_and(|m| m.contains_key(&name)) {
                        out.insert((*cat, name));
                    }
                }
            }
        }
        out.into_iter().collect()
    }

    /// Add item `id`, named `name`, of category `cat` (minted by the caller).
    pub fn add_item(&mut self, cat: CategoryId, name: &str, id: ItemId) {
        self.items
            .entry(cat)
            .or_default()
            .insert(name.to_string(), id);
    }

    /// Load aggregated measure `measure`'s fold state from the rows of its
    /// [`seed_sql`] (`snap` the snapshot they were read with), its value
    /// column being of Postgres type `oid`, and set `model`'s cells for it
    /// to the folds'. A text `min` or `max` without a collation, and a text
    /// `min`, `max` or `count_distinct` under a nondeterministic one, keep
    /// no state (their changes rebuild) and leave the model's cells as they
    /// are.
    pub fn seed(
        &mut self,
        model: &mut Model,
        measure: MeasureId,
        oid: u32,
        rows: &[FoldRow],
        snap: Snap,
    ) -> Result<(), Rebuild> {
        let b = self
            .bindings
            .values()
            .flatten()
            .find(|b| b.measure == measure)
            .ok_or(Rebuild("unknown binding"))?;
        let agg = b.agg.ok_or(Rebuild("not an aggregated binding"))?;
        let vt = value_type(model, measure)?;
        let coll = self.collations.get(&measure);
        if unfoldable(agg, vt, coll).is_some() {
            return Ok(());
        }
        let cmp = text_cmp(coll);
        self.seeded.insert(measure, snap);
        let folds = self.folds.entry(measure).or_default();
        for (dims, v, n, all) in rows {
            let names: Vec<Option<&str>> = dims.iter().map(Option::as_deref).collect();
            let Some(c) = coord_of(&self.items, b, &names)? else {
                continue;
            };
            let f = folds.entry(c).or_default();
            f.rows += all - n;
            let ok = match v {
                None => f.add(&Part::Count, *n),
                Some(s) => match part_of(agg, vt, oid, s)? {
                    // A group's finite sum stands for its `n` rows at once.
                    Part::Total(x) if x.is_finite() => {
                        f.total.count(x, n - 1);
                        f.add(&Part::Total(x), 1) && f.add(&Part::Count, n - 1)
                    }
                    p => f.add(&p, *n),
                },
            };
            if !ok {
                return Err(Rebuild("a fold went out of step"));
            }
        }
        model.inputs.retain(|(m, _), _| *m != measure);
        for (c, f) in folds.iter_mut() {
            if let Some(v) = f.cell(agg, oid, cmp) {
                model.set_input(measure, c.clone(), v);
            }
        }
        Ok(())
    }

    /// Apply the changes of `txns` (tuples of the relations in `rels`) to
    /// `model`'s input cells, as the loader would see the rows, and return
    /// one edit per cell whose value the batch changed, with its final value
    /// (a cell set back to its old value makes none). Changes to relations
    /// without a binding are ignored. Transactions that ended at or before
    /// `replay_until` may already be in `model` (see the module docs). On
    /// `Err` the model's inputs and this mapper's folds are half-updated:
    /// discard both.
    pub fn map(
        &mut self,
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
                self.change(model, rels, change, (replay, txn.xid), &mut touched)?;
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
        &mut self,
        model: &mut Model,
        rels: &HashMap<u32, Relation>,
        change: &Message,
        (replay, xid): (bool, u32),
        touched: &mut Touched,
    ) -> Result<(), Rebuild> {
        // `old`: None = no old row (insert); Some(None) = an update that
        // did not send its old row (the replica identity is unchanged).
        let (rel, old, new) = match change {
            Message::Insert { rel, new } => (*rel, None, Some(new)),
            Message::Update { rel, old, new } => (*rel, Some(old.as_ref()), Some(new)),
            Message::Delete { rel, old } => (*rel, Some(Some(old)), None),
            Message::Truncate { rels, .. } => {
                for r in rels {
                    self.truncate(model, *r, (replay, xid), touched)?;
                }
                return Ok(());
            }
            _ => return Ok(()),
        };
        let Some(bindings) = self.bindings.get(&rel) else {
            return Ok(());
        };
        let r = rels.get(&rel).ok_or(Rebuild("undescribed relation"))?;
        for b in bindings {
            if b.agg.is_none() {
                row(&self.items, model, r, b, (old, new), replay, touched)?;
            } else if !self.loaded(model, b, xid)? {
                let folds = self.folds.entry(b.measure).or_default();
                let cmp = text_cmp(self.collations.get(&b.measure));
                fold_row(&self.items, (folds, cmp), model, r, b, (old, new), touched)?;
            }
        }
        Ok(())
    }

    /// Whether aggregated binding `b`'s seed already saw transaction `xid`
    /// (see the module docs).
    fn loaded(&self, model: &Model, b: &Binding, xid: u32) -> Result<bool, Rebuild> {
        match self.seeded.get(&b.measure) {
            Some(s) => Ok(s.sees(xid)),
            None => Err(b
                .agg
                .and_then(|a| unfoldable(a, vt_of(model, b), self.collations.get(&b.measure)))
                .unwrap_or(Rebuild("fold not seeded"))),
        }
    }

    /// TRUNCATE of relation `rel`: every cell its bindings feed goes. A
    /// replayed one (see the module docs) of a binding without `agg` rebuilds.
    fn truncate(
        &mut self,
        model: &mut Model,
        rel: u32,
        (replay, xid): (bool, u32),
        touched: &mut Touched,
    ) -> Result<(), Rebuild> {
        for b in self.bindings.get(&rel).into_iter().flatten() {
            if b.agg.is_some() {
                if self.loaded(model, b, xid)? {
                    continue;
                }
                self.folds.remove(&b.measure);
            } else if replay {
                // Already in the load, and rows inserted after it may be
                // in a later batch: reload rather than empty the cells.
                return Err(Rebuild("truncate"));
            }
            let gone: Vec<(MeasureId, Coordinate)> = model
                .inputs
                .keys()
                .filter(|(m, _)| *m == b.measure)
                .cloned()
                .collect();
            for k in gone {
                let before = model.inputs.remove(&k);
                touched.entry(k).or_insert(before);
            }
        }
        Ok(())
    }
}

fn vt_of(model: &Model, b: &Binding) -> ValueType {
    model
        .measures
        .get(&b.measure)
        .map_or(ValueType::Number, |m| m.value_type)
}

fn value_type(model: &Model, measure: MeasureId) -> Result<ValueType, Rebuild> {
    Ok(model
        .measures
        .get(&measure)
        .ok_or(Rebuild("unknown measure"))?
        .value_type)
}

/// One row change through a binding without `agg`: retract the old row's
/// cell, then assert the new row's.
fn row(
    items: &Items,
    model: &mut Model,
    r: &Relation,
    b: &Binding,
    (old, new): (Option<Option<&Tuple>>, Option<&Tuple>),
    replay: bool,
    touched: &mut Touched,
) -> Result<(), Rebuild> {
    let vt = value_type(model, b.measure)?;
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
        if let Some(c) = coord(items, r, b, t)? {
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
        if let (Some(c), Some(v)) = (coord(items, r, b, t)?, value(r, b, t, vt)?) {
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

/// One row change through an aggregated binding: take the old row out of
/// its coordinate's fold, put the new row into its (maybe other)
/// coordinate's, and set each touched cell to its fold's value.
fn fold_row(
    items: &Items,
    (folds, cmp): (&mut HashMap<Coordinate, Fold>, Option<&TextCmp>),
    model: &mut Model,
    r: &Relation,
    b: &Binding,
    (old, new): (Option<Option<&Tuple>>, Option<&Tuple>),
    touched: &mut Touched,
) -> Result<(), Rebuild> {
    let agg = b.agg.ok_or(Rebuild("not an aggregated binding"))?;
    let vt = value_type(model, b.measure)?;
    // Only a FULL old row carries the old value (the worker gives the
    // relations of aggregated bindings REPLICA IDENTITY FULL).
    let old = match old {
        None => None,
        Some(Some(t)) if r.replica_identity == b'f' => Some(t),
        Some(_) => return Err(Rebuild("old row not replicated")),
    };
    for (t, k) in [(old, -1), (new, 1)] {
        let Some(t) = t else { continue };
        let Some(c) = coord(items, r, b, t)? else {
            continue;
        };
        let p = part(agg, r, b, t, vt)?;
        let f = folds.entry(c.clone()).or_default();
        if !f.add(&p, k) {
            return Err(Rebuild("a fold went out of step"));
        }
        if k > 0 {
            f.offer(&p, agg, cmp);
        }
        let cell = f.cell(agg, value_oid(r, b), cmp);
        if f.rows == 0 {
            folds.remove(&c);
        }
        let key = (b.measure, c);
        let before = match cell {
            Some(v) => model.inputs.insert(key.clone(), v),
            None => model.inputs.remove(&key),
        };
        touched.entry(key).or_insert(before);
    }
    Ok(())
}

/// Why aggregate `agg` of a `vt` measure under collation `c` keeps no fold
/// (its changes rebuild): a text `min` or `max` orders by its collation, so
/// it needs one; a nondeterministic collation's equality is Postgres's alone.
fn unfoldable(agg: Agg, vt: ValueType, c: Option<&Collation>) -> Option<Rebuild> {
    match (agg, c) {
        (Agg::Min | Agg::Max | Agg::CountDistinct, Some(Collation::Nondeterministic)) => {
            Some(Rebuild("nondeterministic collation"))
        }
        (Agg::Min | Agg::Max, None) if vt == ValueType::Text => Some(Rebuild("min or max of text")),
        _ => None,
    }
}

/// The text compare of collation `c`, when it is deterministic.
fn text_cmp(c: Option<&Collation>) -> Option<&TextCmp> {
    match c {
        Some(Collation::Deterministic(cmp)) => Some(&**cmp),
        _ => None,
    }
}

/// Whether value `a` is a better `min` (or `max`) than `b`: texts by `cmp`,
/// the rest by their key order.
fn beats(agg: Agg, cmp: &TextCmp, a: &Key, b: &Key) -> bool {
    let o = match (a, b) {
        (Key::Text(a), Key::Text(b)) => cmp(a, b),
        _ => a.cmp(b),
    };
    if agg == Agg::Max {
        o.is_gt()
    } else {
        o.is_lt()
    }
}

/// The coordinate of tuple `t`; `None` when a dimension is NULL (the
/// loader skips such rows).
fn coord(
    items: &Items,
    r: &Relation,
    b: &Binding,
    t: &Tuple,
) -> Result<Option<Coordinate>, Rebuild> {
    let mut names = Vec::with_capacity(b.dims.len());
    for (_, col) in &b.dims {
        names.push(dim_name(r, col, t)?);
    }
    let names: Vec<Option<&str>> = names.iter().map(Option::as_deref).collect();
    coord_of(items, b, &names)
}

/// Dimension column `col` of tuple `t` as the loader's `col::text` reads
/// it: a `bool` as `true`/`false` (`pgoutput` sends `t`/`f`), `char(n)`
/// without its padding, anything else as `pgoutput` sends it. `None` for
/// NULL.
fn dim_name(r: &Relation, col: &str, t: &Tuple) -> Result<Option<String>, Rebuild> {
    let s = match r.get(t, col) {
        None => return Err(Rebuild("bound column missing")),
        Some(Datum::Null) => return Ok(None),
        Some(Datum::Unchanged) => return Err(Rebuild("unchanged TOAST value")),
        Some(Datum::Text(s)) => s,
    };
    let oid = r
        .columns
        .iter()
        .find(|c| c.name == col)
        .map_or(0, |c| c.type_oid);
    Ok(Some(match (oid, s.as_str()) {
        (BOOL, "t") => "true".into(),
        (BOOL, "f") => "false".into(),
        (BPCHAR, s) => s.trim_end_matches(' ').into(),
        (_, s) => s.into(),
    }))
}

/// The coordinate of dimension values `names` (in `b.dims` order); `None`
/// when one is NULL.
fn coord_of(
    items: &Items,
    b: &Binding,
    names: &[Option<&str>],
) -> Result<Option<Coordinate>, Rebuild> {
    let mut pairs = Vec::with_capacity(b.dims.len());
    for ((cat, _), name) in b.dims.iter().zip(names) {
        let Some(name) = name else { return Ok(None) };
        let item = items
            .get(cat)
            .and_then(|m| m.get(*name))
            .ok_or(Rebuild("new item"))?;
        pairs.push((*cat, *item));
    }
    Ok(Some(Coordinate::from_pairs(pairs)))
}

/// `bpchar`'s oid: its text keeps the padding `::text` strips.
const BPCHAR: u32 = 1042;
const BOOL: u32 = 16;
const FLOAT4: u32 = 700;
const FLOAT8: u32 = 701;
const NUMERIC: u32 = 1700;

/// The text of tuple `t`'s value column (`None` = SQL NULL).
fn text<'t>(r: &Relation, b: &Binding, t: &'t Tuple) -> Result<Option<&'t str>, Rebuild> {
    match r.get(t, &b.value_col) {
        None => Err(Rebuild("bound column missing")),
        Some(Datum::Null) => Ok(None),
        Some(Datum::Unchanged) => Err(Rebuild("unchanged TOAST value")),
        Some(Datum::Text(s)) => Ok(Some(s)),
    }
}

/// The Postgres type of `b`'s value column in `r`.
fn value_oid(r: &Relation, b: &Binding) -> u32 {
    r.columns
        .iter()
        .find(|c| c.name == b.value_col)
        .map_or(0, |c| c.type_oid)
}

/// The value of tuple `t` as a `vt` (`None` = SQL NULL).
fn value(r: &Relation, b: &Binding, t: &Tuple, vt: ValueType) -> Result<Option<Value>, Rebuild> {
    let Some(s) = text(r, b, t)? else {
        return Ok(None);
    };
    parse(vt, s, value_oid(r, b) == BPCHAR).map(Some)
}

/// Value text `s` as a `vt` (`bpchar`: padded `char(n)` text).
fn parse(vt: ValueType, s: &str, bpchar: bool) -> Result<Value, Rebuild> {
    let v = match vt {
        ValueType::Number => s.parse().ok().map(Value::Number),
        // pgoutput sends t/f; a fold seed reads bool::text, true/false.
        ValueType::Boolean => match s {
            "t" | "true" => Some(Value::Boolean(true)),
            "f" | "false" => Some(Value::Boolean(false)),
            _ => None,
        },
        ValueType::Text => Some(Value::Text(
            if bpchar { s.trim_end_matches(' ') } else { s }.to_string(),
        )),
        ValueType::DateTime => datetime(s).map(Value::DateTime),
        ValueType::Enum => None,
    };
    v.ok_or(Rebuild("value the loader would not read"))
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

/// One coordinate's rows, as its aggregate needs them.
#[derive(Debug, Clone, Default, PartialEq)]
struct Fold {
    /// Rows, NULL values included: a `count` over rows that are all NULL
    /// is 0, a cell.
    rows: i64,
    /// Rows with a value.
    n: i64,
    /// `sum`, `avg`: the values' total.
    total: Total,
    /// `min`, `max`: each value's row count, in value order (a text's by
    /// its bytes); `count_distinct`: each distinct value's.
    values: BTreeMap<Key, i64>,
    /// A text `min` or `max` under a collation: its value, when known.
    best: Option<Key>,
}

/// What one row adds to a fold.
#[derive(Debug, Clone, PartialEq)]
enum Part {
    /// A NULL value.
    Null,
    /// A value only its presence matters for (`count`).
    Count,
    /// A value to total (`sum`, `avg`).
    Total(Num),
    /// A value to keep (`min`, `max`, `count_distinct`).
    Key(Key),
}

impl Fold {
    /// Add (`k > 0`) or remove (`k < 0`) `k` rows of `p`. `false` when that
    /// removes rows the fold does not have.
    fn add(&mut self, p: &Part, k: i64) -> bool {
        self.rows += k;
        match p {
            Part::Null => return self.rows >= 0,
            Part::Count => {}
            Part::Total(x) => self.total.add(*x, k),
            Part::Key(key) => {
                let c = self.values.entry(key.clone()).or_default();
                *c += k;
                let now = *c;
                if now == 0 {
                    self.values.remove(key);
                } else if now < 0 {
                    return false;
                }
            }
        }
        self.n += k;
        self.rows >= 0 && self.n >= 0
    }

    /// A text `min` or `max` under compare `cmp`: `p`, just added, may be its
    /// new value. (A removed one is noticed by [`Fold::cell`].)
    fn offer(&mut self, p: &Part, agg: Agg, cmp: Option<&TextCmp>) {
        if let (Part::Key(k), Some(cmp), Some(best)) = (p, cmp, &self.best) {
            if beats(agg, cmp, k, best) {
                self.best = Some(k.clone());
            }
        }
    }

    /// The cell the rows make, as Postgres's aggregate computes it over a
    /// value column of type `oid` (texts ordered by `cmp`, their collation's
    /// compare, when there is one).
    fn cell(&mut self, agg: Agg, oid: u32, cmp: Option<&TextCmp>) -> Option<Value> {
        if self.rows <= 0 {
            return None;
        }
        let num = |x: f64| Some(Value::Number(x));
        match agg {
            Agg::Count => num(self.n as f64),
            Agg::CountDistinct => num(self.values.len() as f64),
            // Over NULLs only, the other aggregates are NULL: no cell.
            _ if self.n <= 0 => None,
            Agg::Sum => num(self.total.value()),
            Agg::Avg => num(self.total.avg(self.n, matches!(oid, FLOAT4 | FLOAT8))),
            Agg::Min | Agg::Max => match cmp {
                Some(cmp) => {
                    // Rescan only when the value went (or was never found).
                    if !self
                        .best
                        .as_ref()
                        .is_some_and(|b| self.values.contains_key(b))
                    {
                        self.best = self
                            .values
                            .keys()
                            .reduce(|b, k| if beats(agg, cmp, k, b) { k } else { b })
                            .cloned();
                    }
                    self.best.as_ref().and_then(Key::value)
                }
                None if agg == Agg::Min => self.values.keys().next().and_then(Key::value),
                None => self.values.keys().next_back().and_then(Key::value),
            },
        }
    }
}

/// What a row whose value has text `s` (of Postgres type `oid`) adds to an
/// `agg` fold of a `vt` measure.
fn part_of(agg: Agg, vt: ValueType, oid: u32, s: &str) -> Result<Part, Rebuild> {
    Ok(match agg {
        Agg::Count => Part::Count,
        Agg::CountDistinct => Part::Key(Key::Text(distinct_text(oid, s))),
        Agg::Sum | Agg::Avg => Part::Total(Num::parse(s, matches!(oid, FLOAT4 | FLOAT8))?),
        Agg::Min | Agg::Max => Part::Key(Key::order(parse(vt, s, oid == BPCHAR)?)?),
    })
}

/// [`part_of`] tuple `t`'s value.
fn part(agg: Agg, r: &Relation, b: &Binding, t: &Tuple, vt: ValueType) -> Result<Part, Rebuild> {
    match text(r, b, t)? {
        None => Ok(Part::Null),
        Some(s) => part_of(agg, vt, value_oid(r, b), s),
    }
}

/// Value text `s` of type `oid`, normalized so that two values Postgres
/// finds equal have the same text: `char(n)` without its padding, a bool as
/// `t` or `f` (pgoutput's; `bool::text` is `true`), `numeric` without
/// trailing fractional zeros (`1.50` = `1.5`), and float `-0` as `0`. Other
/// types compare as their text: exact under a deterministic collation, the
/// only kind that folds ([`unfoldable`]).
fn distinct_text(oid: u32, s: &str) -> String {
    match oid {
        BPCHAR => s.trim_end_matches(' ').to_string(),
        BOOL => s.get(..1).unwrap_or(s).to_string(),
        NUMERIC if s.contains('.') => s.trim_end_matches('0').trim_end_matches('.').to_string(),
        FLOAT4 | FLOAT8 if s == "-0" => "0".to_string(),
        _ => s.to_string(),
    }
}

/// A value as `min`, `max` and `count_distinct` order and compare it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    /// A number by its bits made totally ordered ([`ord_bits`]).
    Num(i64),
    Bool(bool),
    /// Milliseconds since the epoch.
    Date(i64),
    /// A text (`min`, `max`: ordered by its collation, [`Fold::cell`]), or
    /// for `count_distinct` the normalized text ([`distinct_text`]).
    Text(String),
}

/// An estimate of the heap `model`'s input cells hold, in bytes: its map
/// (buckets and control bytes), each coordinate's B-tree and each text.
/// Within 5 % of glibc's live heap at 300k cells. It walks the cells.
pub fn inputs_bytes(model: &Model) -> usize {
    let entry = size_of::<((MeasureId, Coordinate), Value)>() + 1;
    let tree = |len: usize| len.div_ceil(7) * chunk(16 + 11 * size_of::<(CategoryId, ItemId)>());
    let cell = |((_, c), v): (&(MeasureId, Coordinate), &Value)| {
        tree(c.dims.len())
            + match v {
                Value::Text(s) if s.capacity() > 0 => chunk(s.capacity()),
                _ => 0,
            }
    };
    buckets(model.inputs.capacity()) * entry + model.inputs.iter().map(cell).sum::<usize>()
}

/// The bytes glibc's malloc takes for an `n`-byte allocation.
fn chunk(n: usize) -> usize {
    (n + 8).next_multiple_of(16).max(32)
}

/// The buckets of a hashbrown table that holds `capacity` entries (7/8 of
/// a power of two, or 3 of 4 when small).
fn buckets(capacity: usize) -> usize {
    match capacity {
        0 => 0,
        1..=3 => 4,
        4..=7 => 8,
        c => (c * 8 / 7).next_power_of_two(),
    }
}

impl Key {
    /// Its text's heap bytes.
    fn bytes(&self) -> usize {
        match self {
            Key::Text(s) if s.capacity() > 0 => chunk(s.capacity()),
            _ => 0,
        }
    }

    /// `v` in `min`/`max` order.
    fn order(v: Value) -> Result<Key, Rebuild> {
        Ok(match v {
            // + 0.0: -0 and 0 are one key, as float8 finds them equal.
            Value::Number(x) => Key::Num(ord_bits(x + 0.0)),
            Value::Boolean(b) => Key::Bool(b),
            Value::DateTime(d) => Key::Date(d.timestamp_millis()),
            Value::Text(s) => Key::Text(s),
            _ => return Err(Rebuild("value the loader would not read")),
        })
    }

    fn value(&self) -> Option<Value> {
        Some(match self {
            Key::Num(k) => Value::Number(f64::from_bits(ord_bits_inverse(*k))),
            Key::Bool(b) => Value::Boolean(*b),
            Key::Date(ms) => Value::DateTime(chrono::DateTime::from_timestamp_millis(*ms)?),
            Key::Text(s) => Value::Text(s.clone()),
        })
    }
}

/// `x`'s bits as an `i64` whose order is `x`'s (`f64::total_cmp`).
fn ord_bits(x: f64) -> i64 {
    let b = x.to_bits() as i64;
    b ^ ((((b >> 63) as u64) >> 1) as i64)
}

/// The inverse of [`ord_bits`] (it is its own inverse).
fn ord_bits_inverse(k: i64) -> u64 {
    (k ^ ((((k >> 63) as u64) >> 1) as i64)) as u64
}

/// A summed number: exact nanos when its text is a plain decimal with at
/// most 9 decimals (every integer, most `numeric`s), else a float (every
/// value of a float column, whose float8s Postgres sums).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Num {
    /// Nanos, and the text's decimal places.
    Nanos(i128, u8),
    Float(f64),
}

const NANOS: i128 = 1_000_000_000;

impl Num {
    fn is_finite(self) -> bool {
        match self {
            Num::Nanos(..) => true,
            Num::Float(f) => f.is_finite(),
        }
    }

    fn parse(s: &str, float: bool) -> Result<Num, Rebuild> {
        if let Some(n) = nanos(s).filter(|_| !float) {
            let scale = s.split_once('.').map_or(0, |(_, f)| f.len());
            return Ok(Num::Nanos(n, scale as u8));
        }
        s.parse()
            .map(Num::Float)
            .map_err(|_| Rebuild("value the loader would not read"))
    }
}

/// Plain decimal text (`-12.5`, at most 20 integer digits and 9 decimals)
/// as integer nanos.
fn nanos(s: &str) -> Option<i128> {
    let (neg, body) = match s.strip_prefix('-') {
        Some(b) => (true, b),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (int, frac) = body.split_once('.').unwrap_or((body, ""));
    let digits = |d: &str| d.bytes().all(|c| c.is_ascii_digit());
    if (int.is_empty() && frac.is_empty())
        || int.len() > 20
        || frac.len() > 9
        || !digits(int)
        || !digits(frac)
    {
        return None;
    }
    let int: i128 = if int.is_empty() { 0 } else { int.parse().ok()? };
    let x = int * NANOS + format!("{frac:0<9}").parse::<i128>().ok()?;
    Some(if neg { -x } else { x })
}

/// Postgres's numeric `x / n` for `x` = `nanos` / 10⁹ of display scale
/// `scale` and a count `n` > 0, cast to float8: the quotient rounded (half
/// away from zero) to `select_div_scale`'s result scale, at least 16
/// significant digits and at least `scale` decimals, then read as float8
/// text is.
fn numeric_div(nanos: i128, scale: i32, n: i64) -> f64 {
    // The weight (base-10000 digit position) and first base-10000 digit of
    // a numeric, as numeric.c normalizes it.
    fn weight(int: i128, frac9: i128) -> (i32, i128) {
        if int > 0 {
            let w = (int.ilog10() / 4) as i32;
            (w, int / 10_000i128.pow(w as u32))
        } else {
            let f12 = frac9 * 1000;
            [
                (-1, f12 / 100_000_000),
                (-2, f12 / 10_000 % 10_000),
                (-3, f12 % 10_000),
            ]
            .into_iter()
            .find(|(_, g)| *g > 0)
            .unwrap_or((0, 0))
        }
    }
    let a = nanos.unsigned_abs() as i128;
    let (w1, d1) = weight(a / NANOS, a % NANOS);
    let (w2, d2) = weight(i128::from(n), 0);
    let qweight = w1 - w2 - i32::from(d1 <= d2);
    let rscale = (16 - 4 * qweight).max(scale).clamp(0, 1000);
    let d = NANOS * i128::from(n);
    let (mut int, mut rem) = (a / d, a % d);
    let mut digits = Vec::with_capacity(rscale as usize);
    for _ in 0..rscale {
        rem *= 10;
        digits.push((rem / d) as u8);
        rem %= d;
    }
    if rem * 10 / d >= 5 {
        let mut carry = true;
        for x in digits.iter_mut().rev() {
            *x = (*x + 1) % 10;
            if *x != 0 {
                carry = false;
                break;
            }
        }
        if carry {
            int += 1;
        }
    }
    // The quotient exceeds 10^(4·qweight) and rscale keeps 16 digits below
    // that: a nonzero sum never rounds to zero, so its sign stands.
    let frac: String = digits.iter().map(|x| char::from(b'0' + x)).collect();
    let sign = if nanos < 0 { "-" } else { "" };
    format!("{sign}{int}.{frac}0").parse().unwrap_or(f64::NAN)
}

/// A running total that removing a value undoes exactly: decimal values in
/// integer nanos (so `sum(int)` and `sum(numeric)` come out as Postgres's
/// exact sum rounded once to `float8`), other finite values exactly in an
/// [`Exact`], and the infinities and NaNs counted. So the total is the same
/// whatever the order of the changes that made it.
///
/// Two documented limits (docs/src/postgres.md): a float column's total is
/// its float8s' exact sum rounded once, where Postgres adds them in scan
/// order, so a fresh `sum` can differ in the last bits (as Postgres's own
/// does across scan orders); a `numeric` value with more than 9 decimals
/// rounds to float8 before it is summed.
#[derive(Debug, Clone, Default, PartialEq)]
struct Total {
    nanos: i128,
    /// Rows summed in nanos, per decimal places (a numeric sum's scale is
    /// its values' largest).
    scales: [i64; 10],
    /// The floats' exact sum (boxed: most totals have none).
    float: Option<Box<Exact>>,
    /// Rows summed as floats.
    floats: i64,
    /// Rows of +∞, -∞ and NaN.
    special: [i64; 3],
}

impl Total {
    /// Count `k` more rows of `x`'s kind without adding to the sum: a seed
    /// group's sum stands for all of its rows.
    fn count(&mut self, x: Num, k: i64) {
        match x {
            Num::Nanos(_, scale) => self.scales[usize::from(scale)] += k,
            Num::Float(_) => self.floats += k,
        }
    }

    fn add(&mut self, x: Num, k: i64) {
        match x {
            Num::Nanos(n, scale) => {
                self.nanos += n * i128::from(k);
                self.scales[usize::from(scale)] += k;
            }
            Num::Float(f) if f.is_nan() => self.special[2] += k,
            Num::Float(f) if f == f64::INFINITY => self.special[0] += k,
            Num::Float(f) if f == f64::NEG_INFINITY => self.special[1] += k,
            Num::Float(f) => {
                self.float.get_or_insert_default().add(f, k);
                self.floats += k;
            }
        }
    }

    /// The mean of `n` rows: in float8 for a `float` column, else as
    /// Postgres's numeric `sum / count` ([`numeric_div`]) cast to float8.
    fn avg(&self, n: i64, float: bool) -> f64 {
        let special = self.special.iter().any(|k| *k > 0);
        if float || special || self.floats > 0 {
            return self.value() / n as f64;
        }
        let scale = self.scales.iter().rposition(|k| *k > 0).unwrap_or(0);
        numeric_div(self.nanos, scale as i32, n)
    }

    fn value(&self) -> f64 {
        match self.special {
            [_, _, nan] if nan > 0 => f64::NAN,
            [p, m, _] if p > 0 && m > 0 => f64::NAN,
            [p, _, _] if p > 0 => f64::INFINITY,
            [_, m, _] if m > 0 => f64::NEG_INFINITY,
            _ => {
                let (q, r) = (self.nanos / NANOS, (self.nanos % NANOS).abs());
                let sign = if self.nanos < 0 && q == 0 { "-" } else { "" };
                // Decimal text to float8, as Postgres's numeric_float8 does.
                let exact: f64 = format!("{sign}{q}.{r:09}").parse().unwrap_or(f64::NAN);
                match &self.float {
                    Some(f) => exact + f.value(),
                    None => exact,
                }
            }
        }
    }
}

/// The exact sum of finite floats, `k` times each: a two's-complement
/// integer in units of 2⁻¹⁰⁷⁴ (the least subnormal) over [`Exact::LIMBS`]
/// 64-bit limbs, room for 2²⁰⁵ times the largest float.
#[derive(Debug, Clone, PartialEq)]
struct Exact([u64; Exact::LIMBS]);

impl Default for Exact {
    fn default() -> Self {
        Exact([0; Exact::LIMBS])
    }
}

impl Exact {
    const LIMBS: usize = 36;

    /// Add `k` times finite `x`.
    fn add(&mut self, x: f64, k: i64) {
        let bits = x.to_bits();
        let (e, frac) = ((bits >> 52) & 0x7ff, bits & ((1 << 52) - 1));
        // x = m · 2^shift units.
        let (m, shift) = if e == 0 {
            (frac, 0)
        } else {
            (frac | 1 << 52, e - 1)
        };
        let v = u128::from(m) * u128::from(k.unsigned_abs());
        let (at, off) = ((shift / 64) as usize, (shift % 64) as u32);
        let (lo, hi) = (v as u64, (v >> 64) as u64);
        let words = if off == 0 {
            [lo, hi, 0]
        } else {
            [lo << off, hi << off | lo >> (64 - off), hi >> (64 - off)]
        };
        let neg = (bits >> 63 == 1) != (k < 0);
        let mut carry = false;
        for (i, limb) in self.0.iter_mut().enumerate().skip(at) {
            let w = words.get(i - at).copied().unwrap_or(0);
            if w == 0 && !carry {
                if i - at >= 2 {
                    break;
                }
                continue;
            }
            let (s, c1) = if neg {
                limb.overflowing_sub(w)
            } else {
                limb.overflowing_add(w)
            };
            let (s, c2) = if neg {
                s.overflowing_sub(u64::from(carry))
            } else {
                s.overflowing_add(u64::from(carry))
            };
            *limb = s;
            carry = c1 || c2;
        }
    }

    /// The sum, rounded once to the nearest float (ties to even).
    fn value(&self) -> f64 {
        let neg = self.0[Self::LIMBS - 1] >> 63 == 1;
        let mut mag = self.0;
        if neg {
            // Two's-complement negate.
            let mut carry = true;
            for l in &mut mag {
                let (s, c) = (!*l).overflowing_add(u64::from(carry));
                *l = s;
                carry = c;
            }
        }
        let sign = if neg { -1.0 } else { 1.0 };
        let Some(top) = mag.iter().rposition(|l| *l != 0) else {
            return 0.0;
        };
        // Its bit length, and its top 64 bits with any bit below kept in
        // the last (a sticky bit, so the one rounding is correct).
        let len = 64 * top as i32 + 64 - mag[top].leading_zeros() as i32;
        if len <= 64 {
            // An exact multiple of 2^-1074: a subnormal or a normal float.
            return sign * mag[0] as f64 * f64::from_bits(1);
        }
        let get = |bit: i32| -> u64 {
            let (i, o) = ((bit / 64) as usize, bit % 64);
            let w = mag[i] >> o;
            if o == 0 || i + 1 > top {
                w
            } else {
                w | mag[i + 1] << (64 - o)
            }
        };
        let low = len - 64;
        let sticky = mag[..(low / 64) as usize].iter().any(|l| *l != 0)
            || mag[(low / 64) as usize] & ((1u64 << (low % 64)) - 1) != 0;
        let x = (get(low) | u64::from(sticky)) as f64;
        sign * pow2_scale(x, low - 1074)
    }
}

/// `x` (a normal float below 2⁶⁴) times 2^`p`, exact unless it overflows.
fn pow2_scale(x: f64, p: i32) -> f64 {
    let pow2 = |p: i32| f64::from_bits(((p + 1023) as u64) << 52);
    if p > 1023 {
        f64::INFINITY
    } else if p < -1022 {
        x * pow2(-1022) * pow2(p + 1022)
    } else {
        x * pow2(p)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    /// `route` gives each model only the transactions and changes on its
    /// relations: two models on one table both get it, a model on two tables
    /// gets both, an unbound relation reaches nobody.
    #[test]
    fn route_splits_a_batch_by_model() {
        use crate::pgoutput::Tuple;
        let ins = |rel| Message::Insert {
            rel,
            new: Tuple::default(),
        };
        let txns = vec![
            Txn {
                xid: 7,
                end_lsn: 70,
                changes: vec![ins(1), ins(2), ins(9)],
            },
            Txn {
                xid: 8,
                end_lsn: 80,
                changes: vec![Message::Truncate {
                    options: 0,
                    rels: vec![2, 3],
                }],
            },
            Txn {
                xid: 9,
                end_lsn: 90,
                changes: vec![ins(9)],
            },
        ];
        // 1 -> models 10 and 11; 2 -> 11; 3 -> 11 and 12.
        let models = HashMap::from([(1, vec![10, 11]), (2, vec![11]), (3, vec![11, 12])]);
        let r = route(&txns, &models);
        assert_eq!(r.keys().copied().collect::<Vec<_>>(), vec![10, 11, 12]);
        assert_eq!(r[&10].len(), 1);
        assert_eq!(r[&10][0].changes, vec![ins(1)]);
        assert_eq!((r[&10][0].xid, r[&10][0].end_lsn), (7, 70));
        assert_eq!(r[&11].len(), 2);
        assert_eq!(r[&11][0].changes, vec![ins(1), ins(2)]);
        // A truncate of two of its relations reaches model 11 once.
        assert_eq!(r[&11][1].changes, txns[1].changes);
        assert_eq!(r[&12][0].changes, txns[1].changes);
        assert_eq!(r[&12][0].end_lsn, 80);
        assert!(route(&txns, &HashMap::new()).is_empty());
        let begin = Message::Begin {
            final_lsn: 0,
            commit_ts: 0,
            xid: 1,
        };
        let odd = vec![Txn {
            xid: 1,
            end_lsn: 1,
            changes: vec![begin],
        }];
        assert!(route(&odd, &models).is_empty());
    }

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

    /// Time {Q1=11, Q2=12} × Product {P1=21}; input Price (5).
    fn catalog() -> Catalog {
        Catalog {
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
        }
    }

    /// [`catalog`]'s model with Price[Q1,P1] = 1.
    fn model() -> Model {
        let mut m = build_model(&catalog()).unwrap();
        m.set_input(MeasureId(5), coordinate(11, 21), Value::Number(1.0));
        m
    }

    fn coordinate(q: u32, p: u32) -> Coordinate {
        Coordinate::from_pairs([(CategoryId(1), ItemId(q)), (CategoryId(2), ItemId(p))])
    }

    fn key(q: u32, p: u32) -> CoordKey {
        CoordKey::from_slice(&[(1, q), (2, p)])
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

    fn mapper(m: &Model, agg: Option<Agg>) -> Mapper {
        let b = Binding {
            measure: MeasureId(5),
            value_col: "price".into(),
            dims: vec![
                (CategoryId(1), "quarter".into()),
                (CategoryId(2), "product".into()),
            ],
            agg,
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
        run_with(rels(b'f', &[]), None, changes)
    }

    fn txn(changes: Vec<Message>) -> Vec<Txn> {
        vec![Txn {
            xid: 1,
            end_lsn: 10,
            changes,
        }]
    }

    fn run_with(rels: HashMap<u32, Relation>, agg: Option<Agg>, changes: Vec<Message>) -> Out {
        let mut m = model();
        let edits = mapper(&m, agg).map(&mut m, &rels, &txn(changes), 0)?;
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
        let edits = mapper(&m, None).map(&mut m, &r, &changes, 0).unwrap();
        assert!(edits.is_empty());
    }

    #[test]
    fn what_cannot_be_applied_cell_by_cell_rebuilds() {
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
        let mut no_price = rels(b'f', &[]);
        no_price.get_mut(&F).unwrap().columns[2].name = "cost".into();
        assert_eq!(
            run_with(no_price, None, vec![ins(full("Q2", "1"))]),
            Err(Rebuild("bound column missing"))
        );
        let mut m = model();
        let changes = txn(vec![ins(full("Q2", "1"))]);
        assert_eq!(
            mapper(&m, None)
                .map(&mut m, &HashMap::new(), &changes, 0)
                .err(),
            Some(Rebuild("undescribed relation"))
        );
        let mut gone = model();
        let mut m = mapper(&gone, None);
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
        let edits = mapper(&m, None).map(&mut m, &r, &txn(vec![ins(full("Q2", price))]), 0)?;
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
        for (vt, s) in [(Boolean, "yes"), (DateTime, "infinity"), (Enum, "1")] {
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
            run_with(pk(), None, vec![del(key_only.clone())]),
            Ok(vec![(key(11, 21), None)])
        );
        assert_eq!(
            run_with(pk(), None, vec![upd(None, full("Q1", "9"))]),
            Ok(vec![(key(11, 21), Some(9.0))])
        );
        assert_eq!(
            run_with(pk(), None, vec![upd(Some(key_only), full("Q2", "9"))]),
            Ok(vec![(key(11, 21), None), (key(12, 21), Some(9.0))])
        );
        // A key that is not the dimensions cannot name the old cell.
        let id_key = || rels(b'd', &["note"]);
        assert_eq!(
            run_with(id_key(), None, vec![ins(full("Q2", "1"))]),
            Ok(vec![(key(12, 21), Some(1.0))])
        );
        assert_eq!(
            run_with(id_key(), None, vec![upd(None, full("Q1", "9"))]),
            Err(Rebuild("old row not replicated"))
        );
        assert_eq!(
            run_with(id_key(), None, vec![del(full("Q1", "1"))]),
            Err(Rebuild("old row not replicated"))
        );
    }

    #[test]
    fn replayed_transactions_converge() {
        // Price[Q1,P1] = 1 is already loaded; the slot re-delivers its insert
        // and a later update, then a fresh insert onto the same cell.
        let mut m = model();
        let mut mp = mapper(&m, None);
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

    #[test]
    fn new_dimension_values_are_listed_then_minted() {
        let mut m = model();
        let mut mp = mapper(&m, None);
        let r = rels(b'f', &[]);
        let batch = txn(vec![
            ins(full("Q3", "1")),
            ins(row(Some("Q4"), None, Some("1"))),
            upd(
                Some(full("Q1", "1")),
                row(Some("Q1"), Some("P2"), Some("2")),
            ),
            del(full("Q3", "1")),
            ins(full("Q1", "9")),
        ]);
        assert_eq!(
            mp.unknown(&r, &batch),
            [
                (CategoryId(1), "Q3".to_string()),
                (CategoryId(1), "Q4".into()),
                (CategoryId(2), "P2".into())
            ]
        );
        assert_eq!(
            mp.map(&mut m, &r, &batch, 0).err(),
            Some(Rebuild("new item"))
        );
        let mut m = model();
        for (cat, name, id) in [(1, "Q3", 31), (1, "Q4", 32), (2, "P2", 33)] {
            m.add_item(ItemId(id), CategoryId(cat), name);
            mp.add_item(CategoryId(cat), name, ItemId(id));
        }
        assert!(mp.unknown(&r, &batch).is_empty());
        let edits = mp.map(&mut m, &r, &batch, 0).unwrap();
        let got: Vec<_> = edits
            .iter()
            .map(|e| (e.coord.clone(), e.value.clone()))
            .collect();
        assert_eq!(
            got,
            [
                (key(11, 21), Some(CellValue::num(9.0))),
                (key(11, 33), Some(CellValue::num(2.0))),
            ]
        );
    }

    #[test]
    fn dimension_names_read_as_the_loader_casts_them() {
        let mut r = rels(b'f', &[]);
        r.get_mut(&F).unwrap().columns[0].type_oid = BOOL;
        r.get_mut(&F).unwrap().columns[1].type_oid = BPCHAR;
        let f = &r[&F];
        let t = row(Some("t"), Some("P1  "), Some("1"));
        assert_eq!(dim_name(f, "quarter", &t), Ok(Some("true".into())));
        assert_eq!(dim_name(f, "product", &t), Ok(Some("P1".into())));
        let t = row(Some("f"), None, Some("1"));
        assert_eq!(dim_name(f, "quarter", &t), Ok(Some("false".into())));
        assert_eq!(dim_name(f, "product", &t), Ok(None));
        assert_eq!(
            dim_name(f, "nope", &t),
            Err(Rebuild("bound column missing"))
        );
    }

    #[test]
    fn truncate_removes_the_relations_cells() {
        let truncate = || Message::Truncate {
            options: 0,
            rels: vec![OTHER, F],
        };
        assert_eq!(run(vec![truncate()]), Ok(vec![(key(11, 21), None)]));
        // Rows inserted after it in the batch stay.
        assert_eq!(
            run(vec![truncate(), ins(full("Q2", "3"))]),
            Ok(vec![(key(11, 21), None), (key(12, 21), Some(3.0))])
        );
        // An aggregated binding's folds start over.
        let seed = [("Q1", Some("1")), ("Q1", Some("2"))];
        assert_eq!(
            fold(Agg::Count, &seed, vec![truncate(), ins(full("Q1", "5"))]),
            nums(&[("Q1", 1.0)])
        );
    }

    /// Rows `(quarter, price)` of product P1.
    type Rows = Vec<(String, Option<String>)>;

    fn rows_of(rows: &[(&str, Option<&str>)]) -> Rows {
        rows.iter()
            .map(|(q, v)| (q.to_string(), v.map(str::to_string)))
            .collect()
    }

    /// `rows` as [`seed_sql`]'s query returns them for `agg`.
    fn seed_rows(agg: Agg, rows: &Rows) -> Vec<FoldRow> {
        // (quarter, group value) -> (rows with a value, all rows, their sum)
        let mut g: BTreeMap<(String, Option<String>), (i64, i64, f64)> = BTreeMap::new();
        for (q, v) in rows {
            let special = |s: &String| s.parse::<f64>().is_ok_and(|x| !x.is_finite());
            let group = match agg {
                Agg::Count => None,
                Agg::Sum | Agg::Avg => v.clone().filter(special),
                _ => v.clone(),
            };
            let e = g.entry((q.clone(), group)).or_default();
            e.1 += 1;
            if let Some(s) = v {
                e.0 += 1;
                e.2 += s.parse::<f64>().unwrap_or(0.0);
            }
        }
        g.into_iter()
            .map(|((q, group), (n, all, sum))| {
                let v = match agg {
                    Agg::Count => None,
                    Agg::Sum | Agg::Avg => group.or((n > 0).then(|| sum.to_string())),
                    _ => group,
                };
                (vec![Some(q), Some("P1".into())], v, n, all)
            })
            .collect()
    }

    /// What Postgres's `agg … GROUP BY` makes of `rows`, per quarter.
    fn expected(agg: Agg, rows: &Rows) -> BTreeMap<String, Value> {
        let mut by: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (q, v) in rows {
            let present = by.entry(q).or_default();
            present.extend(v.as_deref());
        }
        by.into_iter()
            .filter_map(|(q, present)| {
                let xs = || present.iter().map(|s| s.parse::<f64>().unwrap());
                let n = present.len() as f64;
                let v = match agg {
                    Agg::Count => Some(n),
                    Agg::CountDistinct => {
                        Some(present.iter().collect::<BTreeSet<_>>().len() as f64)
                    }
                    _ if present.is_empty() => None,
                    Agg::Sum => Some(xs().sum()),
                    Agg::Avg => Some(xs().sum::<f64>() / n),
                    // Postgres orders NaN above every number.
                    Agg::Min => xs().min_by(f64::total_cmp),
                    Agg::Max => xs().max_by(f64::total_cmp),
                };
                v.map(|v| (q.to_string(), Value::Number(v)))
            })
            .collect()
    }

    fn quarter(c: &Coordinate) -> String {
        match c.dims[&CategoryId(1)] {
            ItemId(11) => "Q1".into(),
            _ => "Q2".into(),
        }
    }

    /// The model's Price cells by quarter.
    fn cells(m: &Model) -> BTreeMap<String, Value> {
        m.inputs
            .iter()
            .map(|((_, c), v)| (quarter(c), v.clone()))
            .collect()
    }

    fn nums(pairs: &[(&str, f64)]) -> Result<BTreeMap<String, Value>, Rebuild> {
        Ok(pairs
            .iter()
            .map(|(q, x)| (q.to_string(), Value::Number(*x)))
            .collect())
    }

    /// Fold `changes` into Price bound through `agg`, its model and folds
    /// loaded from `seed`; the resulting cells by quarter.
    fn fold(
        agg: Agg,
        seed: &[(&str, Option<&str>)],
        changes: Vec<Message>,
    ) -> Result<BTreeMap<String, Value>, Rebuild> {
        let rows = rows_of(seed);
        let mut m = model();
        let mut mp = mapper(&m, Some(agg));
        mp.seed(
            &mut m,
            MeasureId(5),
            25,
            &seed_rows(agg, &rows),
            Snap::default(),
        )?;
        assert_eq!(cells(&m), expected(agg, &rows), "{agg:?} seed");
        mp.map(&mut m, &rels(b'f', &[]), &txn(changes), 0)?;
        Ok(cells(&m))
    }

    #[test]
    fn aggregated_bindings_fold_rows_incrementally() {
        // An insert, a delete, and an update that moves a row to Q2.
        assert_eq!(
            fold(
                Agg::Sum,
                &[("Q1", Some("1")), ("Q1", Some("2"))],
                vec![
                    ins(full("Q1", "4")),
                    del(full("Q1", "1")),
                    upd(Some(full("Q1", "2")), full("Q2", "2")),
                ]
            ),
            nums(&[("Q1", 4.0), ("Q2", 2.0)])
        );
        // count counts values; a coordinate of NULLs only counts 0.
        let null_q2 = || ins(row(Some("Q2"), Some("P1"), None));
        assert_eq!(
            fold(
                Agg::Count,
                &[("Q1", Some("1"))],
                vec![null_q2(), ins(full("Q1", "1"))]
            ),
            nums(&[("Q1", 2.0), ("Q2", 0.0)])
        );
        // sum over NULLs only is NULL: no cell.
        assert_eq!(fold(Agg::Sum, &[], vec![null_q2()]), nums(&[]));
        assert_eq!(
            fold(
                Agg::Avg,
                &[("Q1", Some("1")), ("Q1", Some("2"))],
                vec![ins(full("Q1", "6"))]
            ),
            nums(&[("Q1", 3.0)])
        );
        // min and max keep each value's row count.
        let threes = [("Q1", Some("3")), ("Q1", Some("3")), ("Q1", Some("5"))];
        assert_eq!(
            fold(Agg::Min, &threes, vec![del(full("Q1", "3"))]),
            nums(&[("Q1", 3.0)])
        );
        assert_eq!(
            fold(
                Agg::Min,
                &threes,
                vec![del(full("Q1", "3")), del(full("Q1", "3"))]
            ),
            nums(&[("Q1", 5.0)])
        );
        assert_eq!(
            fold(Agg::Max, &threes, vec![del(full("Q1", "5"))]),
            nums(&[("Q1", 3.0)])
        );
        // Deleting one of two rows that share a key keeps the distinct count.
        let shas = [("Q1", Some("a1")), ("Q1", Some("a1")), ("Q1", Some("a2"))];
        assert_eq!(
            fold(Agg::CountDistinct, &shas, vec![del(full("Q1", "a1"))]),
            nums(&[("Q1", 2.0)])
        );
        assert_eq!(
            fold(
                Agg::CountDistinct,
                &shas,
                vec![del(full("Q1", "a1")), del(full("Q1", "a1"))]
            ),
            nums(&[("Q1", 1.0)])
        );
        // The coordinate's last row takes its cell with it.
        assert_eq!(
            fold(Agg::Count, &[("Q1", Some("1"))], vec![del(full("Q1", "1"))]),
            nums(&[])
        );
    }

    #[test]
    fn what_a_fold_cannot_apply_rebuilds() {
        let step = Some(Rebuild("a fold went out of step"));
        assert_eq!(fold(Agg::Sum, &[], vec![del(full("Q1", "1"))]).err(), step);
        assert_eq!(
            fold(
                Agg::CountDistinct,
                &[("Q1", Some("a1"))],
                vec![del(full("Q1", "a2"))]
            )
            .err(),
            step
        );
        assert_eq!(
            fold(Agg::Sum, &[], vec![ins(full("Q1", "x"))]).err(),
            Some(Rebuild("value the loader would not read"))
        );
        assert_eq!(
            fold(Agg::Count, &[("Q9", Some("1"))], vec![]).err(),
            Some(Rebuild("new item"))
        );
        // Only a FULL old row carries the old value.
        let mut m = model();
        let mut mp = mapper(&m, Some(Agg::Sum));
        assert_eq!(
            mp.map(
                &mut m,
                &rels(b'f', &[]),
                &txn(vec![ins(full("Q2", "1"))]),
                0
            )
            .err(),
            Some(Rebuild("fold not seeded"))
        );
        mp.seed(&mut m, MeasureId(5), 25, &[], Snap::default())
            .unwrap();
        let pk = rels(b'd', &["quarter", "product"]);
        assert_eq!(
            mp.map(&mut m, &pk, &txn(vec![del(full("Q1", "1"))]), 0)
                .err(),
            Some(Rebuild("old row not replicated"))
        );
        assert!(mp
            .map(&mut m, &pk, &txn(vec![ins(full("Q2", "1"))]), 0)
            .is_ok());
        // Text min/max without a collation to order by: it always rebuilds.
        let mut m = model();
        m.measures.get_mut(&MeasureId(5)).unwrap().value_type = ValueType::Text;
        let mut mp = mapper(&m, Some(Agg::Max));
        let b = seed_rows(Agg::Max, &rows_of(&[("Q1", Some("b"))]));
        mp.seed(&mut m, MeasureId(5), 25, &b, Snap::default())
            .unwrap();
        assert_eq!(
            mp.map(
                &mut m,
                &rels(b'f', &[]),
                &txn(vec![ins(full("Q1", "a"))]),
                0
            )
            .err(),
            Some(Rebuild("min or max of text"))
        );
        assert_eq!(
            mp.seed(&mut m, MeasureId(6), 25, &[], Snap::default()),
            Err(Rebuild("unknown binding"))
        );
        let mut plain = mapper(&m, None);
        assert_eq!(
            plain.seed(&mut m, MeasureId(5), 25, &[], Snap::default()),
            Err(Rebuild("not an aggregated binding"))
        );
    }

    /// A case-insensitive order, as a stand-in for a linguistic collation:
    /// `a` < `B` < `c`, where byte order has `B` < `a` < `c`.
    fn folded() -> Collation {
        Collation::Deterministic(Arc::new(|a: &str, b: &str| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        }))
    }

    /// Price as text bound through `agg` under collation `c`, seeded with
    /// `seed`, after `changes`: its cells by quarter.
    fn text_fold(
        agg: Agg,
        c: Option<Collation>,
        seed: &[(&str, Option<&str>)],
        changes: Vec<Message>,
    ) -> Result<BTreeMap<String, Value>, Rebuild> {
        let mut m = model();
        m.measures.get_mut(&MeasureId(5)).unwrap().value_type = ValueType::Text;
        let mut mp = mapper(&m, Some(agg));
        if let Some(c) = c {
            mp.collate(MeasureId(5), c);
        }
        let rows = seed_rows(agg, &rows_of(seed));
        mp.seed(&mut m, MeasureId(5), 25, &rows, Snap::default())?;
        mp.map(&mut m, &rels(b'f', &[]), &txn(changes), 0)?;
        Ok(cells(&m))
    }

    #[test]
    fn text_min_and_max_fold_under_their_collation() {
        let q1 = |v| full("Q1", v);
        let text = |pairs: &[(&str, &str)]| {
            values(
                &pairs
                    .iter()
                    .map(|(q, s)| (*q, Value::Text(s.to_string())))
                    .collect::<Vec<_>>(),
            )
        };
        let seed = [("Q1", Some("c")), ("Q1", Some("B")), ("Q2", Some("a"))];
        // Under the collation, not byte order (which would give `B`, `c`).
        let ins_a = vec![ins(q1("a"))];
        assert_eq!(
            text_fold(Agg::Min, Some(folded()), &seed, ins_a.clone()),
            text(&[("Q1", "a"), ("Q2", "a")])
        );
        assert_eq!(
            text_fold(Agg::Max, Some(folded()), &seed, vec![]),
            text(&[("Q1", "c"), ("Q2", "a")])
        );
        // The extreme goes: the fold rescans for the next.
        assert_eq!(
            text_fold(Agg::Max, Some(folded()), &seed, vec![del(q1("c"))]),
            text(&[("Q1", "B"), ("Q2", "a")])
        );
        assert_eq!(
            text_fold(
                Agg::Min,
                Some(folded()),
                &seed,
                vec![del(q1("B")), del(q1("c"))]
            ),
            text(&[("Q2", "a")])
        );
        // Byte order (`C`) is a collation too.
        let bytes = Collation::Deterministic(Arc::new(|a: &str, b: &str| a.cmp(b)));
        assert_eq!(
            text_fold(Agg::Min, Some(bytes), &seed, ins_a.clone()),
            text(&[("Q1", "B"), ("Q2", "a")])
        );
        // No collation: rebuild. A nondeterministic one: rebuild, also for
        // count_distinct, whose equality only Postgres knows.
        assert_eq!(
            text_fold(Agg::Min, None, &seed, ins_a.clone()),
            Err(Rebuild("min or max of text"))
        );
        for agg in [Agg::Min, Agg::Max, Agg::CountDistinct] {
            assert_eq!(
                text_fold(agg, Some(Collation::Nondeterministic), &seed, ins_a.clone()),
                Err(Rebuild("nondeterministic collation")),
                "{agg:?}"
            );
        }
        assert_eq!(format!("{:?}", folded()), "Deterministic");
        assert_eq!(
            format!("{:?}", Collation::Nondeterministic),
            "Nondeterministic"
        );
    }

    #[test]
    fn collated_folds_equal_a_fresh_aggregate_after_random_batches() {
        const VALUES: [Option<&str>; 7] = [
            Some("a"),
            Some("B"),
            Some("b"),
            Some("c"),
            Some("ä"),
            Some(""),
            None,
        ];
        let Collation::Deterministic(cmp) = folded() else {
            unreachable!()
        };
        let tuple = |(q, v): &(String, Option<String>)| row(Some(q), Some("P1"), v.as_deref());
        for agg in [Agg::Min, Agg::Max] {
            let mut rng = Rng(0x9e37_79b9_7f4a_7c15 + agg as u64);
            let pick = |rng: &mut Rng| {
                (
                    ["Q1", "Q2"][rng.below(2)].to_string(),
                    VALUES[rng.below(VALUES.len())].map(str::to_string),
                )
            };
            let mut rows: Rows = (0..4).map(|_| pick(&mut rng)).collect();
            let mut m = model();
            m.measures.get_mut(&MeasureId(5)).unwrap().value_type = ValueType::Text;
            let mut mp = mapper(&m, Some(agg));
            mp.collate(MeasureId(5), folded());
            mp.seed(
                &mut m,
                MeasureId(5),
                25,
                &seed_rows(agg, &rows),
                Snap::default(),
            )
            .unwrap();
            for batch in 0..300u32 {
                let mut changes = Vec::new();
                for _ in 0..1 + rng.below(3) {
                    let new = pick(&mut rng);
                    match rng.below(3) {
                        0 => {
                            changes.push(ins(tuple(&new)));
                            rows.push(new);
                        }
                        _ if rows.is_empty() => {}
                        1 => changes.push(del(tuple(&rows.swap_remove(rng.below(rows.len()))))),
                        _ => {
                            let at = rng.below(rows.len());
                            changes.push(upd(Some(tuple(&rows[at])), tuple(&new)));
                            rows[at] = new;
                        }
                    }
                }
                let t = Txn {
                    xid: batch,
                    end_lsn: 100 + u64::from(batch),
                    changes,
                };
                mp.map(&mut m, &rels(b'f', &[]), &[t], 0).unwrap();
                let mut want: BTreeMap<String, Value> = BTreeMap::new();
                for (q, v) in &rows {
                    let Some(v) = v else { continue };
                    let better = |w: &Value| match w {
                        Value::Text(w) => {
                            let o = cmp(v, w);
                            if agg == Agg::Min {
                                o.is_lt()
                            } else {
                                o.is_gt()
                            }
                        }
                        _ => false,
                    };
                    if want.get(q).is_none_or(better) {
                        want.insert(q.clone(), Value::Text(v.clone()));
                    }
                }
                assert_eq!(cells(&m), want, "{agg:?}, batch {batch}");
            }
        }
    }

    #[test]
    fn float_totals_are_exact_and_order_free() {
        let float = |s| Num::parse(s, true).unwrap();
        // A float column's values sum as float8s, even plain decimals.
        assert_eq!(float("0.1"), Num::Float(0.1));
        let mut t = Total::default();
        for s in ["0.1", "0.2", "0.3"] {
            t.add(float(s), 1);
        }
        // The float8s' exact sum, rounded once: 0.6. Postgres adds them in
        // scan order, 0.6000000000000001: the documented limit.
        assert_eq!(t.value(), 0.6);
        assert_eq!(0.1 + 0.2 + 0.3, 0.6000000000000001);
        // Cancellation leaves no residue: Postgres's sum(1e16, 1, -1e16) is
        // 0 in one order and 1 in another; the exact sum is 1.
        let mut t = Total::default();
        for (s, k) in [("1e16", 1), ("1", 1), ("-1e16", 1)] {
            t.add(float(s), k);
        }
        assert_eq!(t.value(), 1.0);
        t.add(float("1"), -1);
        assert_eq!(t.value(), 0.0);
        // Order-free: the same values in any order (and with removals) give
        // the same bits. Against an exact reference: the values are integers
        // times 2^-40 spread over 2^60, so an i128 sum of them is exact.
        let mut rng = Rng(42);
        let xs: Vec<f64> = (0..2000)
            .map(|_| {
                let m = rng.below(1 << 30) as f64 - (1 << 29) as f64;
                m * 2f64.powi(rng.below(60) as i32 - 40)
            })
            .collect();
        let mut a = Total::default();
        let mut b = Total::default();
        for x in &xs {
            a.add(Num::Float(*x), 1);
        }
        for x in xs.iter().rev() {
            b.add(Num::Float(*x), 2);
            b.add(Num::Float(*x), -1);
        }
        assert_eq!(a.value().to_bits(), b.value().to_bits());
        let units: i128 = xs.iter().map(|x| (x * 2f64.powi(40)) as i128).sum();
        assert!(xs.iter().all(|x| (x * 2f64.powi(40)).fract() == 0.0));
        assert_eq!(a.value(), units as f64 / 2f64.powi(40));
        // Extremes: subnormals, the largest float, overflow to infinity.
        let tiny = f64::from_bits(1);
        let mut e = Exact::default();
        e.add(tiny, 3);
        assert_eq!(e.value(), 3.0 * tiny);
        e.add(tiny, -5);
        assert_eq!(e.value(), -2.0 * tiny);
        let mut e = Exact::default();
        e.add(f64::MAX, 1);
        assert_eq!(e.value(), f64::MAX);
        e.add(f64::MAX, 1);
        assert_eq!(e.value(), f64::INFINITY);
        e.add(f64::MAX, -3);
        assert_eq!(e.value(), -f64::MAX);
        let mut e = Exact::default();
        e.add(-0.0, 1);
        assert_eq!(e.value(), 0.0);
        // Rounds once, to nearest even: 2^53 + 1 is a tie, 2^53 + 1 + 2^-60 not.
        let mut e = Exact::default();
        e.add(2f64.powi(53), 1);
        e.add(1.0, 1);
        assert_eq!(e.value(), 2f64.powi(53));
        e.add(2f64.powi(-60), 1);
        assert_eq!(e.value(), 2f64.powi(53) + 2.0);
        assert_eq!(pow2_scale(1.5, -1074), f64::from_bits(2));
        // A numeric column's exponent-form and long values sum as floats too.
        assert_eq!(Num::parse("1e2", false), Ok(Num::Float(100.0)));
        assert_eq!(
            Num::parse("0.1234567891", false),
            Ok(Num::Float(0.1234567891))
        );
    }

    #[test]
    fn a_numeric_average_keeps_its_scale_after_deletes() {
        // Seeded per scale: avg(10^8.123456789, 10^8) over 2 rows, then the
        // 9-decimal row goes; avg(100000000) is 100000000.00000000 (scale
        // 8 from 16 significant digits), the same float either way, but a
        // 2-row group at scale 9 must still count 1 row at scale 9.
        let numeric = (ValueType::Number, NUMERIC);
        let seed = [
            group(Some("Q1"), Some("200000000.000000000"), 2, 2),
            group(Some("Q1"), Some("100000000.123456789"), 1, 1),
        ];
        let q1 = |v| full("Q1", v);
        // Postgres: avg of 100000000.000000000 (x2) and 100000000.123456789
        // = 100000000.041152263 (scale 9).
        assert_eq!(
            fold_as(Agg::Avg, numeric, &seed, vec![]),
            nums(&[("Q1", 100_000_000.041_152_27)])
        );
        // Delete the scale-9 row: the two 100000000.000000000 rows keep
        // scale 9; avg = 100000000.000000000.
        let mut m = model();
        let mut mp = mapper(&m, Some(Agg::Avg));
        mp.seed(&mut m, MeasureId(5), NUMERIC, &seed, Snap::default())
            .unwrap();
        mp.map(
            &mut m,
            &rels(b'f', &[]),
            &txn(vec![del(q1("100000000.123456789"))]),
            0,
        )
        .unwrap();
        let f = &mp.folds[&MeasureId(5)][&coordinate(11, 21)];
        assert_eq!(f.total.scales[9], 2);
        assert_eq!(f.n, 2);
    }

    #[test]
    fn loaded_changes_to_aggregated_bindings_are_skipped() {
        let mut m = model();
        let mut mp = mapper(&m, Some(Agg::Sum));
        let b = seed_rows(Agg::Sum, &rows_of(&[("Q1", Some("1")), ("Q1", Some("4"))]));
        // The seed saw transactions 3 and 5, not 4 (running) or 6 and later.
        mp.seed(&mut m, MeasureId(5), 25, &b, Snap::parse("4:6:4").unwrap())
            .unwrap();
        let t = |xid, v| Txn {
            xid,
            end_lsn: 10,
            changes: vec![ins(full("Q1", v))],
        };
        let batch = [t(3, "1"), t(5, "4"), t(4, "2"), t(6, "8")];
        let edits = mp.map(&mut m, &rels(b'f', &[]), &batch, 99).unwrap();
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].value, Some(CellValue::num(15.0)));
        let truncate = Txn {
            xid: 3,
            end_lsn: 10,
            changes: vec![Message::Truncate {
                options: 0,
                rels: vec![F],
            }],
        };
        assert!(mp
            .map(&mut m, &rels(b'f', &[]), &[truncate], 0)
            .unwrap()
            .is_empty());
    }

    /// xorshift64: deterministic test randomness.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 % n as u64) as usize
        }
    }

    /// Random INSERT, UPDATE (moving rows between coordinates) and DELETE
    /// batches: after each, the model's cells and the edits' running result
    /// equal what the aggregate gives over the rows now in the table.
    #[test]
    fn folds_equal_a_fresh_aggregate_after_random_batches() {
        const VALUES: [Option<&str>; 9] = [
            Some("Infinity"),
            Some("NaN"),
            Some("1"),
            Some("2"),
            Some("2.5"),
            Some("-3"),
            Some("0.25"),
            Some("10"),
            None,
        ];
        let tuple = |(q, v): &(String, Option<String>)| row(Some(q), Some("P1"), v.as_deref());
        let r = rels(b'f', &[]);
        for (i, agg) in [
            Agg::Sum,
            Agg::Avg,
            Agg::Min,
            Agg::Max,
            Agg::Count,
            Agg::CountDistinct,
        ]
        .into_iter()
        .enumerate()
        {
            let mut rng = Rng(0x2545_f491_4f6c_dd1d + i as u64);
            let pick = |rng: &mut Rng| {
                (
                    ["Q1", "Q2"][rng.below(2)].to_string(),
                    VALUES[rng.below(VALUES.len())].map(str::to_string),
                )
            };
            let mut rows: Rows = (0..6).map(|_| pick(&mut rng)).collect();
            let mut m = model();
            let mut mp = mapper(&m, Some(agg));
            mp.seed(
                &mut m,
                MeasureId(5),
                25,
                &seed_rows(agg, &rows),
                Snap::default(),
            )
            .unwrap();
            assert_eq!(
                format!("{:?}", cells(&m)),
                format!("{:?}", expected(agg, &rows)),
                "{agg:?} seed"
            );
            // As text: NaN equals NaN (whatever its bits).
            let shown = |c: &BTreeMap<String, Value>| format!("{c:?}");
            let num = |v: &CellValue| Value::Number(v.as_num().unwrap());
            let mut engine = cells(&m);
            for batch in 0..300u32 {
                let mut changes = Vec::new();
                for _ in 0..1 + rng.below(4) {
                    let new = pick(&mut rng);
                    match rng.below(3) {
                        0 => {
                            changes.push(ins(tuple(&new)));
                            rows.push(new);
                        }
                        _ if rows.is_empty() => {}
                        1 => {
                            let old = rows.swap_remove(rng.below(rows.len()));
                            changes.push(del(tuple(&old)));
                        }
                        _ => {
                            let at = rng.below(rows.len());
                            changes.push(upd(Some(tuple(&rows[at])), tuple(&new)));
                            rows[at] = new;
                        }
                    }
                }
                let t = Txn {
                    xid: batch,
                    end_lsn: 100 + u64::from(batch),
                    changes,
                };
                for e in mp.map(&mut m, &r, &[t], 0).unwrap() {
                    let q = if e.coord == key(11, 21) { "Q1" } else { "Q2" };
                    match e.value {
                        Some(v) => engine.insert(q.into(), num(&v)),
                        None => engine.remove(q),
                    };
                }
                let want = expected(agg, &rows);
                assert_eq!(shown(&cells(&m)), shown(&want), "{agg:?}, batch {batch}");
                assert_eq!(shown(&engine), shown(&want), "{agg:?} edits, batch {batch}");
            }
        }
    }

    #[test]
    fn totals_are_exact_for_decimals_and_follow_float8_otherwise() {
        assert_eq!(nanos("-12.5"), Some(-12_500_000_000));
        assert_eq!(nanos("+0.000000001"), Some(1));
        assert_eq!(nanos(".5"), Some(500_000_000));
        assert_eq!(nanos("7"), Some(7 * NANOS));
        // Past 20 integer digits a value sums as a float.
        assert_eq!(nanos("123456789012345678901"), None);
        assert_eq!(
            Num::parse("123456789012345678901", false),
            Ok(Num::Float(1.2345678901234568e20))
        );
        for s in ["1e5", "1.0000000001", "", "-", ".", "NaN", "1.2.3", "1-2"] {
            assert_eq!(nanos(s), None, "{s}");
        }
        let num = |s| Num::parse(s, false).unwrap();
        let mut t = Total::default();
        t.add(num("0.1"), 1);
        t.add(num("0.2"), 1);
        // Postgres sums an int or numeric column exactly, then casts.
        assert_eq!(t.value(), 0.3);
        t.add(num("1e300"), 1);
        assert_eq!(t.value(), 1e300);
        t.add(num("1e300"), -1);
        assert_eq!(t.value(), 0.3);
        t.add(num("Infinity"), 1);
        assert_eq!(t.value(), f64::INFINITY);
        t.add(num("-Infinity"), 1);
        assert!(t.value().is_nan());
        t.add(num("Infinity"), -1);
        assert_eq!(t.value(), f64::NEG_INFINITY);
        t.add(num("-Infinity"), -1);
        t.add(num("NaN"), 1);
        assert!(t.value().is_nan());
        assert_eq!(
            Num::parse("abc", false),
            Err(Rebuild("value the loader would not read"))
        );
        let mut t = Total::default();
        t.add(num("-0.25"), 2);
        assert_eq!(t.value(), -0.5);
    }

    #[test]
    fn averages_divide_as_postgres_numeric_does() {
        // (sum, its scale, count) -> Postgres's (sum::numeric / count)::float8
        let cases: [(&str, i32, i64, f64); 13] = [
            ("11", 0, 9, 1.222222222222222),
            ("10", 0, 3, 3.3333333333333335),
            ("4", 0, 3, 1.3333333333333333),
            ("1", 0, 7, 0.14285714285714285),
            ("12345", 0, 7, 1763.5714285714287),
            ("98765432", 0, 3, 32921810.666666668),
            ("-11", 0, 9, -1.222222222222222),
            ("1.50", 2, 7, 0.21428571428571427),
            ("0.0001", 4, 1_000_000, 1e-10),
            ("-0.000000001", 9, 3, -3.333333333333333e-10),
            ("17", 0, 6, 2.8333333333333335),
            ("0", 0, 5, 0.0),
            // Rounding carries through every digit into the integer part.
            (
                "8999999999999999999.999999999",
                9,
                9_000_000_000_000_000_000,
                1.0,
            ),
        ];
        for (s, scale, n, want) in cases {
            let got = numeric_div(nanos(s).unwrap(), scale, n);
            assert_eq!(got.to_bits(), want.to_bits(), "{s} / {n}: {got} != {want}");
        }
        let num = |s| Num::parse(s, false).unwrap();
        let mut t = Total::default();
        for s in ["1.50", "2.25", "-0.10"] {
            t.add(num(s), 1);
        }
        assert_eq!(t.avg(3, false), 1.2166666666666666);
        t.add(num("2.25"), -1);
        t.add(num("1.50"), -1);
        assert_eq!(t.avg(1, false), -0.1);
        // A float8 column averages in float8.
        let mut t = Total::default();
        t.add(num("11"), 1);
        assert_eq!(t.avg(9, true), 11.0 / 9.0);
        // So does a numeric column once a value was summed as a float.
        let mut t = Total::default();
        t.add(num("1e2"), 1);
        t.add(num("2"), 1);
        assert_eq!(t.avg(2, false), 51.0);
    }

    #[test]
    fn keys_order_and_compare_as_postgres_does() {
        let xs = [
            f64::NEG_INFINITY,
            -2.5,
            -0.0,
            0.0,
            1e-300,
            3.0,
            f64::INFINITY,
        ];
        let ks: Vec<i64> = xs.iter().map(|x| ord_bits(*x)).collect();
        assert!(ks.windows(2).all(|w| w[0] < w[1]), "{ks:?}");
        assert!(ord_bits(f64::NAN) > ord_bits(f64::INFINITY));
        for x in xs {
            assert_eq!(Key::Num(ord_bits(x)).value(), Some(Value::Number(x)));
        }
        let day = Key::order(Value::DateTime(
            chrono::DateTime::from_timestamp_millis(86_400_000).unwrap(),
        ));
        assert_eq!(day, Ok(Key::Date(86_400_000)));
        assert!(Key::Bool(false) < Key::Bool(true));
        assert_eq!(
            Key::order(Value::Text("a".into())),
            Ok(Key::Text("a".into()))
        );
        assert_eq!(
            Key::order(Value::Enum(1)),
            Err(Rebuild("value the loader would not read"))
        );
        assert_eq!(distinct_text(BPCHAR, "ab  "), "ab");
        assert_eq!(distinct_text(NUMERIC, "1.50"), "1.5");
        assert_eq!(distinct_text(NUMERIC, "2.000"), "2");
        assert_eq!(distinct_text(NUMERIC, "100"), "100");
        assert_eq!(distinct_text(FLOAT8, "-0"), "0");
        assert_eq!(distinct_text(FLOAT4, "-0"), "0");
        assert_eq!(distinct_text(FLOAT8, "-1"), "-1");
        assert_eq!(distinct_text(FLOAT4, "1.5"), "1.5");
        assert_eq!(
            Key::order(Value::Number(-0.0)),
            Key::order(Value::Number(0.0))
        );
        assert_eq!(Key::Text("a".into()).value(), Some(Value::Text("a".into())));
        assert_eq!(distinct_text(25, "a  "), "a  ");
        assert_eq!(distinct_text(BOOL, "true"), distinct_text(BOOL, "t"));
        assert_eq!(
            parse(ValueType::Boolean, "false", false),
            Ok(Value::Boolean(false))
        );
        assert_eq!(Agg::parse("count_distinct"), Some(Agg::CountDistinct));
        for a in ["sum", "avg", "min", "max", "count"] {
            assert!(Agg::parse(a).is_some(), "{a}");
        }
        assert_eq!(Agg::parse("median"), None);
    }

    #[test]
    fn snapshots_say_which_transactions_they_saw() {
        let s = Snap::parse("100:105:101,103").unwrap();
        let seen: Vec<u32> = (95..110).filter(|x| s.sees(*x)).collect();
        assert_eq!(seen, [95, 96, 97, 98, 99, 100, 102, 104]);
        // pgoutput's 32 bits, across an epoch boundary.
        let s = Snap::parse("4294967300:4294967302:").unwrap();
        assert!(s.sees(4_294_967_295) && s.sees(4) && s.sees(5) && !s.sees(6));
        for bad in ["", "1", "1:2", "1:2:x", "1:2:3:4"] {
            assert_eq!(Snap::parse(bad), None, "{bad}");
        }
        assert_eq!(
            seed_sql("s", &["a".into()], "v", Agg::Count),
            "(SELECT \"a\"::text, NULL::text, count(\"v\"), count(*), NULL::text FROM s \
             GROUP BY 1) UNION ALL SELECT NULL, NULL, 0, 0, pg_current_snapshot()::text"
        );
        assert_eq!(
            seed_sql("s", &["a".into()], "v", Agg::Sum),
            "(SELECT \"a\"::text, coalesce(improv_special, sum(\"v\")::text), count(\"v\"), \
             count(*), NULL::text FROM (SELECT *, CASE WHEN \"v\"::text IN ('Infinity', \
             '-Infinity', 'NaN') THEN \"v\"::text END AS improv_special FROM s) t \
             GROUP BY 1, improv_special) UNION ALL SELECT NULL, NULL, 0, 0, \
             pg_current_snapshot()::text"
        );
        for agg in [Agg::Min, Agg::Max, Agg::CountDistinct] {
            assert_eq!(
                seed_sql("s", &["a".into()], "v", agg),
                "(SELECT \"a\"::text, \"v\"::text, count(\"v\"), count(*), NULL::text FROM s \
                 GROUP BY 1, 2) UNION ALL SELECT NULL, NULL, 0, 0, pg_current_snapshot()::text"
            );
        }
    }

    #[test]
    fn a_mapper_lists_its_aggregated_bindings() {
        let plain = Binding {
            measure: MeasureId(5),
            value_col: "price".into(),
            dims: vec![],
            agg: None,
        };
        let summed = |id| Binding {
            measure: MeasureId(id),
            agg: Some(Agg::Sum),
            ..plain.clone()
        };
        let mp = Mapper::new(
            &model(),
            HashMap::from([
                (F, vec![plain.clone(), summed(9)]),
                (OTHER, vec![summed(6)]),
            ]),
        );
        assert_eq!(mp.aggregated(), [(OTHER, summed(6)), (F, summed(9))]);
        assert!(mp.binds(F) && mp.binds(OTHER) && !mp.binds(3));
    }

    /// One [`seed_sql`] row of Price over (`q`, P1).
    fn group(q: Option<&str>, v: Option<&str>, n: i64, all: i64) -> FoldRow {
        (
            vec![q.map(str::to_string), Some("P1".into())],
            v.map(str::to_string),
            n,
            all,
        )
    }

    /// Price as a `vt` in a value column of type `oid`, bound through
    /// `agg` and seeded with `seed`, after `changes`: its cells by quarter.
    fn fold_as(
        agg: Agg,
        (vt, oid): (ValueType, u32),
        seed: &[FoldRow],
        changes: Vec<Message>,
    ) -> Result<BTreeMap<String, Value>, Rebuild> {
        let mut m = model();
        m.measures.get_mut(&MeasureId(5)).unwrap().value_type = vt;
        let mut r = rels(b'f', &[]);
        r.get_mut(&F).unwrap().columns[2].type_oid = oid;
        let mut mp = mapper(&m, Some(agg));
        mp.seed(&mut m, MeasureId(5), oid, seed, Snap::default())?;
        mp.map(&mut m, &r, &txn(changes), 0)?;
        Ok(cells(&m))
    }

    fn values(pairs: &[(&str, Value)]) -> Result<BTreeMap<String, Value>, Rebuild> {
        Ok(pairs
            .iter()
            .map(|(q, v)| (q.to_string(), v.clone()))
            .collect())
    }

    #[test]
    fn min_and_max_fall_back_when_their_extreme_goes() {
        let num = (ValueType::Number, FLOAT8);
        let q1 = |v| full("Q1", v);
        // NaN orders above every number: it is the max until it goes.
        let nan = [
            group(Some("Q1"), Some("3"), 1, 1),
            group(Some("Q1"), Some("NaN"), 1, 1),
        ];
        let max = |changes| fold_as(Agg::Max, num, &nan, changes).unwrap();
        assert!(matches!(max(vec![])["Q1"], Value::Number(x) if x.is_nan()));
        assert_eq!(Ok(max(vec![del(q1("NaN"))])), nums(&[("Q1", 3.0)]));
        let min = fold_as(Agg::Min, num, &nan, vec![del(q1("3"))]).unwrap();
        assert!(matches!(min["Q1"], Value::Number(x) if x.is_nan()));
        // An UPDATE moves the max to Q2; Q1 falls back to its next value.
        let five = [
            group(Some("Q1"), Some("3"), 1, 1),
            group(Some("Q1"), Some("5"), 2, 2),
        ];
        let moved = upd(Some(q1("5")), full("Q2", "5"));
        assert_eq!(
            fold_as(Agg::Max, num, &five, vec![moved.clone()]),
            nums(&[("Q1", 5.0), ("Q2", 5.0)])
        );
        assert_eq!(
            fold_as(Agg::Max, num, &five, vec![moved.clone(), moved]),
            nums(&[("Q1", 3.0), ("Q2", 5.0)])
        );
        // A bool's seed text is true/false and pgoutput's t/f: one value.
        let bool = (ValueType::Boolean, BOOL);
        let tf = [
            group(Some("Q1"), Some("false"), 1, 1),
            group(Some("Q1"), Some("true"), 2, 2),
        ];
        let b = |x| Value::Boolean(x);
        assert_eq!(
            fold_as(Agg::Max, bool, &tf, vec![del(q1("t"))]),
            values(&[("Q1", b(true))])
        );
        assert_eq!(
            fold_as(Agg::Max, bool, &tf, vec![del(q1("t")), del(q1("t"))]),
            values(&[("Q1", b(false))])
        );
        assert_eq!(
            fold_as(Agg::Min, bool, &tf, vec![del(q1("f"))]),
            values(&[("Q1", b(true))])
        );
        let distinct = (ValueType::Number, BOOL);
        assert_eq!(
            fold_as(Agg::CountDistinct, distinct, &tf, vec![del(q1("t"))]),
            nums(&[("Q1", 2.0)])
        );
        assert_eq!(
            fold_as(
                Agg::CountDistinct,
                distinct,
                &tf,
                vec![del(q1("t")), del(q1("t"))]
            ),
            nums(&[("Q1", 1.0)])
        );
        // Dates.
        let date = (ValueType::DateTime, 1082);
        let days = [
            group(Some("Q1"), Some("2026-01-01"), 1, 1),
            group(Some("Q1"), Some("2026-03-01"), 1, 1),
        ];
        let day = |s| Value::DateTime(datetime(s).unwrap());
        assert_eq!(
            fold_as(Agg::Max, date, &days, vec![del(q1("2026-03-01"))]),
            values(&[("Q1", day("2026-01-01"))])
        );
        assert_eq!(
            fold_as(Agg::Min, date, &days, vec![del(q1("2026-01-01"))]),
            values(&[("Q1", day("2026-03-01"))])
        );
        // count_distinct finds float -0 and 0 equal, as Postgres does.
        let zero = [group(Some("Q1"), Some("0"), 1, 1)];
        assert_eq!(
            fold_as(Agg::CountDistinct, num, &zero, vec![ins(q1("-0"))]),
            nums(&[("Q1", 1.0)])
        );
    }

    #[test]
    fn sums_and_averages_seed_from_groups() {
        // Postgres: avg(1.50, 2.25, -0.10) and avg(2.25, -0.10).
        let numeric = (ValueType::Number, NUMERIC);
        let q1 = |v| full("Q1", v);
        let two = [group(Some("Q1"), Some("3.75"), 2, 2)];
        assert_eq!(
            fold_as(Agg::Avg, numeric, &two, vec![ins(q1("-0.10"))]),
            nums(&[("Q1", 1.2166666666666666)])
        );
        assert_eq!(
            fold_as(
                Agg::Avg,
                numeric,
                &two,
                vec![ins(q1("-0.10")), del(q1("1.50"))]
            ),
            nums(&[("Q1", 1.075)])
        );
        // Postgres: avg(0.1, 0.2, 0.05) = 0.11666666666666666667.
        assert_eq!(
            fold_as(
                Agg::Avg,
                numeric,
                &[group(Some("Q1"), Some("0.3"), 2, 2)],
                vec![ins(q1("0.05"))]
            ),
            nums(&[("Q1", 0.11666666666666667)])
        );
        // A NaN group of its own makes the sum NaN until its row goes.
        let num = (ValueType::Number, FLOAT8);
        let nan = [
            group(Some("Q1"), Some("3"), 1, 1),
            group(Some("Q1"), Some("NaN"), 1, 1),
        ];
        let sum = fold_as(Agg::Sum, num, &nan, vec![]).unwrap();
        assert!(matches!(sum["Q1"], Value::Number(x) if x.is_nan()));
        assert_eq!(
            fold_as(Agg::Sum, num, &nan, vec![del(q1("NaN"))]),
            nums(&[("Q1", 3.0)])
        );
    }

    #[test]
    fn seeds_skip_null_dimensions_and_refuse_bad_counts() {
        let num = (ValueType::Number, FLOAT8);
        // The loader skips a NULL dimension; a sum over NULLs only is no cell,
        // a count over them is 0.
        let seed = [group(None, Some("7"), 1, 1), group(Some("Q2"), None, 0, 2)];
        assert_eq!(fold_as(Agg::Sum, num, &seed, vec![]), nums(&[]));
        assert_eq!(
            fold_as(Agg::Count, num, &seed, vec![]),
            nums(&[("Q2", 0.0)])
        );
        let null = || del(row(Some("Q2"), Some("P1"), None));
        assert_eq!(
            fold_as(Agg::Count, num, &seed, vec![null()]),
            nums(&[("Q2", 0.0)])
        );
        assert_eq!(
            fold_as(Agg::Count, num, &seed, vec![null(), null()]),
            nums(&[])
        );
        // count(*) below count(value) cannot be: the seed is out of step.
        let step = Err(Rebuild("a fold went out of step"));
        assert_eq!(
            fold_as(Agg::Count, num, &[group(Some("Q1"), None, 0, -1)], vec![]),
            step
        );
        assert_eq!(
            fold_as(Agg::Sum, num, &[group(Some("Q1"), Some("3"), 2, 0)], vec![]),
            step
        );
    }

    #[test]
    fn rows_a_change_cannot_place_are_skipped_or_rebuild() {
        // A NULL dimension is no coordinate: not a plain binding's old row,
        // nor an aggregated binding's row.
        assert_eq!(run(vec![del(row(None, Some("P1"), Some("1")))]), Ok(vec![]));
        assert_eq!(
            fold(
                Agg::Sum,
                &[("Q1", Some("1"))],
                vec![ins(row(None, Some("P1"), Some("5")))]
            ),
            nums(&[("Q1", 1.0)])
        );
        // A FULL identity sends every old row; an update without one rebuilds.
        assert_eq!(
            run(vec![upd(None, full("Q1", "2"))]),
            Err(Rebuild("old row not replicated"))
        );
        let mut t = full("Q2", "1");
        t[0] = Datum::Unchanged;
        assert_eq!(run(vec![ins(t)]), Err(Rebuild("unchanged TOAST value")));
        // Messages other than row changes change nothing.
        let origin = Message::Origin {
            lsn: 1,
            name: "o".into(),
        };
        assert_eq!(run(vec![origin.clone()]), Ok(vec![]));
        // Nor do they, or unbound relations, have unknown dimension values.
        let unbound = Message::Insert {
            rel: OTHER,
            new: full("Q9", "1"),
        };
        let r = rels(b'f', &[]);
        assert!(mapper(&model(), None)
            .unknown(&r, &txn(vec![origin, unbound]))
            .is_empty());
        // A replayed TRUNCATE of a plain binding's relation rebuilds.
        let mut m = model();
        let truncate = txn(vec![Message::Truncate {
            options: 0,
            rels: vec![F],
        }]);
        assert_eq!(
            mapper(&m, None).map(&mut m, &r, &truncate, 10).err(),
            Some(Rebuild("truncate"))
        );
    }

    /// New dimension values without a rebuild: minted into the mapper and
    /// the model, the batch's edits bring an engine to a fresh evaluation
    /// when its plans serve the grown model ([`Engine::serves`]); a plan
    /// that enumerates the grown category does not.
    ///
    /// [`Engine::serves`]: improv_engine::session::Engine::serves
    #[test]
    fn new_items_feed_an_engine_that_serves_the_grown_model() {
        use improv_engine::session::Engine;
        let derived = |id, name: &str, text: &str, categories| MeasureRow {
            id,
            name: name.into(),
            value_type: "number".into(),
            kind: "derived".into(),
            formula: Some(text.into()),
            description: None,
            categories,
        };
        let mut cat = catalog();
        cat.measures
            .push(derived(6, "Total", "Total = SUM(Price OVER Time)", vec![2]));
        cat.measures
            .push(derived(7, "Pos", "Pos = POSITION(Time)", vec![1]));
        let mut m = build_model(&cat).unwrap();
        m.set_input(MeasureId(5), coordinate(11, 21), Value::Number(1.0));
        let (mut total, _) = Engine::new(&m, &[MeasureId(6)]).unwrap();
        let (pos, _) = Engine::new(&m, &[MeasureId(7)]).unwrap();
        let mut mp = mapper(&m, None);
        let r = rels(b'f', &[]);
        let batch = txn(vec![ins(full("Q3", "4")), ins(full("Q2", "2"))]);
        assert_eq!(mp.unknown(&r, &batch), [(CategoryId(1), "Q3".to_string())]);
        mp.add_item(CategoryId(1), "Q3", ItemId(13));
        m.add_item(ItemId(13), CategoryId(1), "Q3");
        assert!(total.serves(&m).unwrap());
        assert!(!pos.serves(&m).unwrap());
        let snap = total.apply(mp.map(&mut m, &r, &batch, 0).unwrap()).unwrap();
        let fresh = improv_engine::dataflow::evaluate(&m, &[MeasureId(6)]).unwrap();
        let total = &fresh[&MeasureId(6)];
        assert_eq!(total[[(2, 21)].as_slice()], CellValue::num(7.0));
        assert_eq!(
            snap.get(&MeasureId(6)).map(|v| (**v).clone()),
            Some(total.clone())
        );
    }

    /// The estimates count what the state holds: nothing before a seed, a
    /// coordinate's map entry and B-tree after, each kept value, and a
    /// float sum's accumulator.
    #[test]
    fn memory_estimates_follow_the_state() {
        let m = model();
        let mp = mapper(&m, Some(Agg::Sum));
        assert_eq!(mp.fold_bytes(), 0);
        let one = inputs_bytes(&m);
        let entry = size_of::<((MeasureId, Coordinate), Value)>() + 1;
        assert_eq!(one, 4 * entry + chunk(16 + 11 * 8));
        let mut m = model();
        m.set_input(
            MeasureId(5),
            coordinate(12, 21),
            Value::Text("x".repeat(40)),
        );
        assert_eq!(inputs_bytes(&m), one + chunk(16 + 11 * 8) + chunk(40));
        let seeded = |agg, v: &str, oid| {
            let mut m = model();
            let mut mp = mapper(&m, Some(agg));
            let rows = vec![(
                vec![Some("Q1".to_string()), Some("P1".to_string())],
                Some(v.to_string()),
                2,
                2,
            )];
            mp.seed(&mut m, MeasureId(5), oid, &rows, Snap::default())
                .unwrap();
            mp.fold_bytes()
        };
        let sum = seeded(Agg::Sum, "1", 25);
        assert_eq!(
            sum,
            4 * (size_of::<(Coordinate, Fold)>() + 1) + chunk(16 + 11 * 8)
        );
        assert_eq!(
            seeded(Agg::Sum, "1.5", 701),
            sum + chunk(size_of::<Exact>())
        );
        let distinct = seeded(Agg::CountDistinct, &"k".repeat(20), 25);
        assert_eq!(
            distinct,
            sum + chunk(16 + 11 * size_of::<(Key, i64)>()) + chunk(20)
        );
        assert_eq!(
            (buckets(0), buckets(3), buckets(4), buckets(14)),
            (0, 4, 8, 16)
        );
    }
}
