//! Improv computation engine.
//!
//! Phase 1. Before building the full formula compiler on differential dataflow,
//! this module contains the **DD-viability spike** (see AGENT_STEERING.md
//! Constraint 1): proof that a `Coordinate`-keyed, `f64`-valued incremental
//! computation (`Revenue = Price * Quantity`) works on differential dataflow,
//! including a live delta after the initial run.
//!
//! Encoding at the dataflow boundary (timely 0.13's exchange bound is serde
//! `Serialize + Deserialize` plus `Ord + Hash` for keys; a `Coordinate`'s
//! `BTreeMap` and a raw `f64` are encoded rather than used directly):
//! * DD *key* = the coordinate as a sorted `Vec<(u32, u32)>` (category, item).
//!   Deterministic and `Ord + Hash`.
//! * cell *value* = [`CellValue`]: a number as `f64::to_bits()` (so it is
//!   `Eq + Hash`), or a boolean, text, date (epoch ms) or error.
//! * diff `R` stays `isize` multiplicity; a `reduce` collapses to one value/key.
//!
//! `core_model` types convert to/from these encodings at the edge.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod builtins;
pub mod compiler;
pub mod dataflow;
pub mod external;
pub mod plan;
pub mod reduce;
pub mod scalar;
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
            Value::Error(e) => Some(CellValue::Err(e.kind.error_type().unwrap_or(0))),
        }
    }
}

impl std::fmt::Display for CellValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CellValue::Num(bits) => f.write_str(&display_number(f64::from_bits(*bits))),
            CellValue::Bool(b) => write!(f, "{b}"),
            CellValue::Text(t) => write!(f, "{t}"),
            CellValue::Date(ms) => {
                // Render as an RFC3339-ish UTC timestamp.
                match chrono::DateTime::<chrono::Utc>::from_timestamp_millis(*ms) {
                    Some(dt) => write!(f, "{}", dt.to_rfc3339()),
                    None => write!(f, "date({ms})"),
                }
            }
            CellValue::Err(c) => f.write_str(
                improv_core_model::ValueErrorKind::from_error_type(*c).map_or("#ERR", |k| k.code()),
            ),
        }
    }
}

/// A number as people read it: at most 15 significant digits, the precision
/// an `f64` always carries through decimal, so binary noise like
/// `58.650000000000006` (from `1173 * 0.05`) shows as `58.65`. The value
/// itself is untouched; exports (CSV, SQL, Postgres) keep every bit.
pub fn display_number(n: f64) -> String {
    if !n.is_finite() || n == 0.0 {
        return if n == 0.0 { "0".into() } else { n.to_string() };
    }
    // `{:.14e}` rounds to 15 significant digits; parsing it back and printing
    // with `{}` gives the shortest decimal for that rounded value.
    let rounded: f64 = format!("{n:.14e}").parse().unwrap_or(n);
    rounded.to_string()
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
/// Exactly the pairs whose category is kept, in order (proven in
/// [`improv_verified::keys::project`]).
pub fn project_key(k: &CoordKey, keep: &[u32]) -> CoordKey {
    improv_verified::keys::project(k, keep)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn display_number_hides_binary_noise_not_digits() {
        use super::display_number as d;
        assert_eq!(d(1173.0 * 0.05), "58.65");
        assert_eq!(d(936.0 * 0.05), "46.8");
        assert_eq!(d(0.1 + 0.2), "0.3");
        assert_eq!(d(1000.0), "1000");
        assert_eq!(d(-2.5), "-2.5");
        assert_eq!(d(0.0), "0");
        assert_eq!(d(-0.0), "0");
        assert_eq!(d(1.0 / 3.0), "0.333333333333333");
        assert_eq!(d(123_456_789_012_345.0), "123456789012345");
        assert_eq!(d(1e300), 1e300.to_string(), "no exponent form, as before");
        assert_eq!(d(1.5e-7), "0.00000015");
        assert_eq!(d(f64::NAN), "NaN");
        assert_eq!(d(f64::INFINITY), "inf");
        assert_eq!(super::CellValue::num(1173.0 * 0.05).to_string(), "58.65");
    }

    use super::*;
    use improv_core_model::{ValueError, ValueErrorKind};

    #[test]
    fn cell_value_conversions_and_display() {
        assert_eq!(CellValue::from_bool_numeric(true), CellValue::num(1.0));
        assert_eq!(CellValue::from_bool_numeric(false), CellValue::num(0.0));
        assert_eq!(CellValue::Bool(true).as_num(), None);
        let cv = |v: Value| CellValue::from_model_value(&v).unwrap();
        assert_eq!(cv(Value::Boolean(true)), CellValue::Bool(true));
        assert_eq!(cv(Value::Text("t".into())), CellValue::Text("t".into()));
        assert_eq!(cv(Value::Enum(3)), CellValue::num(3.0));
        assert_eq!(
            cv(Value::Error(ValueError::new(ValueErrorKind::MissingInput))),
            CellValue::Err(0)
        );
        let shown: Vec<String> = [
            CellValue::num(1.5),
            CellValue::Bool(false),
            CellValue::Text("hi".into()),
            CellValue::Date(0),
            CellValue::Date(i64::MAX),
            CellValue::Err(1),
        ]
        .iter()
        .map(ToString::to_string)
        .collect();
        assert_eq!(
            shown[..4],
            ["1.5", "false", "hi", "1970-01-01T00:00:00+00:00"]
        );
        assert_eq!(shown[4], format!("date({})", i64::MAX));
        assert_eq!(shown[5], "#NULL!");
        assert_eq!(CellValue::Err(0).to_string(), "#ERR");
        assert_eq!(CellValue::Err(2).to_string(), "#DIV/0!");
        assert_eq!(
            cv(Value::Error(ValueError::new(ValueErrorKind::Na))),
            CellValue::Err(7)
        );
    }

    #[test]
    fn coord_keys_round_trip_and_project() {
        let c = Coordinate::from_pairs([(CategoryId(2), ItemId(20)), (CategoryId(1), ItemId(10))]);
        let k = encode_coord(&c);
        assert_eq!(k, vec![(1, 10), (2, 20)]);
        assert_eq!(decode_coord(&k), c);
        assert_eq!(project_key(&k, &[2]), vec![(2, 20)]);
    }
}
