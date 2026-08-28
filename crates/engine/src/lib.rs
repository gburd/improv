//! Improv computation engine.
//!
//! Phase 1. Before building the full formula compiler on differential dataflow,
//! this module contains the **DD-viability spike** (see AGENT_STEERING.md
//! Constraint 1): proof that a `Coordinate`-keyed, `f64`-valued incremental
//! computation (`Revenue = Price * Quantity`) works on differential dataflow,
//! including a live delta after the initial run.
//!
//! Key encoding decisions proven here (timely 0.12 requires `ExchangeData`,
//! i.e. `Abomonation`, on all dataflow data — `BTreeMap` and `f64` don't
//! satisfy that cleanly, so we encode at the dataflow boundary):
//! * DD *key* = the coordinate as a sorted `Vec<(u32, u32)>` (category, item).
//!   Deterministic, `Ord + Hash`, and Abomonation-friendly.
//! * numeric cell *value* = `f64::to_bits()` as `u64` in DD data position
//!   (primitive, exchange-safe); decoded with `f64::from_bits`.
//! * diff `R` stays `isize` multiplicity; a `reduce` collapses to one value/key.
//!
//! `core_model` types convert to/from these encodings at the edge.

pub mod compiler;
pub mod dataflow;
pub mod external;
pub mod plan;
pub mod session;
pub mod typed;

use improv_core_model::{CategoryId, Coordinate, ItemId, Value};

/// The dataflow encoding of a coordinate: sorted `(category_id, item_id)` pairs.
pub type CoordKey = Vec<(u32, u32)>;

/// The dataflow encoding of a cell value.
///
/// Differential-dataflow data must be `Ord + Eq + Hash + Clone + 'static` and
/// exchange-safe. `f64` is none of the first three, so numbers ride as their
/// bit pattern; the other variants make Text / Boolean / Error first-class in
/// the dataflow (the non-numeric value lane). Numbers stay the fast path.
#[derive(
    Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub enum CellValue {
    /// A number, as `f64::to_bits()` (total order via `f64::total_cmp` at the
    /// boundary; bit order is fine for equality/hashing/exchange).
    Num(u64),
    Bool(bool),
    Text(String),
    /// A UTC timestamp as milliseconds since the Unix epoch. `i64` keeps the
    /// value totally-ordered/`Hash`/`Ord` for the DD lane (unlike a chrono
    /// `DateTime`, which is fine but this avoids a chrono dep in the key type).
    Date(i64),
    /// An error value; the `u8` is the `ValueError` kind discriminant so error
    /// cells are distinguishable and propagate through operators.
    Err(u8),
}

impl CellValue {
    /// Wrap an `f64`.
    pub fn num(n: f64) -> Self {
        CellValue::Num(n.to_bits())
    }
    /// The number this holds, if it is `Num`.
    pub fn as_num(&self) -> Option<f64> {
        match self {
            CellValue::Num(b) => Some(f64::from_bits(*b)),
            _ => None,
        }
    }
    /// Encode a boolean as the numeric-lane 1.0/0.0 (back-compat with the
    /// comparison/logical convention) — used where downstream expects a number.
    pub fn from_bool_numeric(b: bool) -> Self {
        CellValue::num(if b { 1.0 } else { 0.0 })
    }
    /// Convert a model `Value` into a `CellValue` for feeding inputs.
    pub fn from_model_value(v: &Value) -> Option<CellValue> {
        match v {
            Value::Number(n) => Some(CellValue::num(*n)),
            Value::Boolean(b) => Some(CellValue::Bool(*b)),
            Value::Text(t) => Some(CellValue::Text(t.clone())),
            Value::Enum(e) => Some(CellValue::num(*e as f64)),
            Value::DateTime(dt) => Some(CellValue::Date(dt.timestamp_millis())),
            Value::Error(_) => Some(CellValue::Err(0)),
        }
    }
}

impl std::fmt::Display for CellValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CellValue::Num(bits) => write!(f, "{}", f64::from_bits(*bits)),
            CellValue::Bool(b) => write!(f, "{b}"),
            CellValue::Text(t) => write!(f, "{t}"),
            CellValue::Date(ms) => {
                // Render as an RFC3339-ish UTC timestamp.
                match chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*ms) {
                    Some(dt) => write!(f, "{}", dt.to_rfc3339()),
                    None => write!(f, "date({ms})"),
                }
            }
            CellValue::Err(_) => write!(f, "#ERR"),
        }
    }
}

/// Encode a `Coordinate` as its dataflow key (BTreeMap already iterates sorted).
pub fn encode_coord(c: &Coordinate) -> CoordKey {
    c.dims.iter().map(|(cat, it)| (cat.0, it.0)).collect()
}

/// Decode a dataflow key back into a `Coordinate`.
pub fn decode_coord(k: &CoordKey) -> Coordinate {
    Coordinate::from_pairs(k.iter().map(|(c, i)| (CategoryId(*c), ItemId(*i))))
}

/// Project a coord key down to a subset of category ids (join key / broadcast).
pub fn project_key(k: &CoordKey, keep: &[u32]) -> CoordKey {
    k.iter()
        .filter(|(cat, _)| keep.contains(cat))
        .copied()
        .collect()
}

#[cfg(test)]
mod spike {
    //! The DD-viability spike. If this compiles and passes, differential
    //! dataflow is a viable engine substrate for Improv's cube model.

    use super::*;
    use differential_dataflow::input::InputSession;
    use differential_dataflow::operators::{Join, Reduce};
    use std::sync::{Arc, Mutex};

    const TIME: u32 = 1;
    const PRODUCT: u32 = 2;

    fn ck(pairs: &[(u32, u32)]) -> CoordKey {
        let c = Coordinate::from_pairs(
            pairs
                .iter()
                .map(|(cat, it)| (CategoryId(*cat), ItemId(*it))),
        );
        encode_coord(&c)
    }

    /// Compute `Revenue[Time,Product] = Price[Product] * Quantity[Time,Product]`
    /// incrementally on DD, then apply a delta and confirm the affected cell
    /// updates while the unaffected cell does not recompute.
    #[test]
    fn dd_revenue_is_incremental() {
        let out: Arc<Mutex<Vec<(u64, CoordKey, f64)>>> = Arc::new(Mutex::new(Vec::new()));
        let out2 = out.clone();

        timely::execute::execute_directly(move |worker| {
            let mut price_in: InputSession<u64, (CoordKey, u64), isize> = InputSession::new();
            let mut qty_in: InputSession<u64, (CoordKey, u64), isize> = InputSession::new();
            let probe_out = out2.clone();

            worker.dataflow(|scope| {
                let price = price_in.to_collection(scope);
                let qty = qty_in.to_collection(scope);

                // Key Price by Product; carry the full coord + qty so we can
                // rebuild the Revenue key after the join broadcasts over Time.
                let price_by_product = price.map(|(k, v)| (project_key(&k, &[PRODUCT]), v));
                let qty_by_product = qty.map(|(k, v)| (project_key(&k, &[PRODUCT]), (k, v)));

                let revenue = qty_by_product
                    .join(&price_by_product)
                    .map(|(_p, ((full, qbits), pbits))| {
                        let rev = f64::from_bits(pbits) * f64::from_bits(qbits);
                        (full, rev.to_bits())
                    })
                    .reduce(|_key, input, output| {
                        for (bits, mult) in input {
                            if *mult > 0 {
                                output.push((**bits, 1isize));
                            }
                        }
                    });

                revenue.inspect(move |((k, bits), time, _diff)| {
                    probe_out
                        .lock()
                        .unwrap()
                        .push((*time, k.clone(), f64::from_bits(*bits)));
                });
            });

            // Round 0: initial data.
            price_in.advance_to(0);
            price_in.insert((ck(&[(PRODUCT, 20)]), 10.0f64.to_bits())); // Widget A = 10
            price_in.insert((ck(&[(PRODUCT, 21)]), 20.0f64.to_bits())); // Widget B = 20
            qty_in.advance_to(0);
            qty_in.insert((ck(&[(TIME, 10), (PRODUCT, 20)]), 100.0f64.to_bits()));
            qty_in.insert((ck(&[(TIME, 10), (PRODUCT, 21)]), 50.0f64.to_bits()));
            price_in.advance_to(1);
            qty_in.advance_to(1);
            price_in.flush();
            qty_in.flush();
            while price_in.time() < &1 || qty_in.time() < &1 {
                worker.step();
            }

            // Round 1: delta -- Quantity[2025,WidgetA] 100 -> 120.
            qty_in.remove((ck(&[(TIME, 10), (PRODUCT, 20)]), 100.0f64.to_bits()));
            qty_in.insert((ck(&[(TIME, 10), (PRODUCT, 20)]), 120.0f64.to_bits()));
            price_in.advance_to(2);
            qty_in.advance_to(2);
            price_in.flush();
            qty_in.flush();
            while qty_in.time() < &2 {
                worker.step();
            }
        });

        let events = out.lock().unwrap();
        let a = ck(&[(TIME, 10), (PRODUCT, 20)]);
        let b = ck(&[(TIME, 10), (PRODUCT, 21)]);

        assert_eq!(final_value(&events, &b), 1000.0, "B = 20*50, unchanged");
        assert_eq!(final_value(&events, &a), 1200.0, "A updated to 10*120");

        // Incrementality: B must not re-emit at round >= 2 (only A changed).
        let b_recomputed = events.iter().any(|(t, k, _)| *t >= 2 && k == &b);
        assert!(!b_recomputed, "unaffected cell B must not recompute");
    }

    fn final_value(events: &[(u64, CoordKey, f64)], target: &CoordKey) -> f64 {
        events
            .iter()
            .rfind(|(_, k, _)| k == target)
            .map(|(_, _, v)| *v)
            .unwrap_or(0.0)
    }

    #[test]
    fn date_value_enters_the_dd_lane() {
        // A model Date value maps to the Date(i64 millis) lane and displays as
        // an RFC3339 timestamp.
        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0).unwrap();
        let cv = CellValue::from_model_value(&Value::DateTime(dt)).expect("date -> cell");
        assert_eq!(cv, CellValue::Date(1_700_000_000_000));
        assert!(format!("{cv}").starts_with("2023-11-"));
    }
}
