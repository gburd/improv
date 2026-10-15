//! Host-side evaluation of external-function measures (Phase 6).
//!
//! An external-function measure (`Model.external_calls[m] = ExternalCall {
//! func, arg_measures }`) is computed *outside* the differential-dataflow graph:
//! for each coordinate present in all argument measures, we gather the argument
//! values and invoke the registered external function via `improv_extfn`,
//! writing the result as an ordinary input cell.
//!
//! Doing this host-side (rather than inside a DD operator) keeps the engine's
//! dataflow pure and `Send`/deterministic — external calls, which shell out to
//! a runtime, never enter the hot recompute path. It mirrors how SQL live-query
//! measures refresh: the function's output becomes input cells, and the engine
//! then recomputes ordinary dependents normally. External calls are gated on an
//! explicit refresh (they are nondeterministic sources by nature).

use improv_core_model::{Coordinate, MeasureId, Model, Value};
use std::collections::BTreeMap;
use std::time::Duration;

/// Default wall-clock deadline for a single external evaluation.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum ExternalError {
    #[error("{0:?} is not an external-function measure")]
    NotExternal(MeasureId),
    #[error("external function {0:?} is not registered on the model")]
    UnknownFunction(String),
    #[error("external function {func:?} failed at {coord}: {msg}")]
    Eval {
        func: String,
        coord: String,
        msg: String,
    },
    #[error("external function {func:?} takes {expected} args but the measure supplies {got}")]
    ArgCountMismatch {
        func: String,
        expected: usize,
        got: usize,
    },
}

/// Recompute an external-function measure's cells by invoking its function over
/// its argument measures, coordinate by coordinate. Returns the number of cells
/// written. If the function fails at any coordinate (an exception, a timeout,
/// no usable sandbox), this returns [`ExternalError::Eval`] and leaves the
/// measure's previous cells untouched: the results are input cells, and an
/// error value cannot be stored as one.
///
/// The measure is populated over the set of coordinates for which *every*
/// argument measure has a value (the natural join of the arguments' cells).
pub fn refresh_external_measure(
    model: &mut Model,
    timeout: Duration,
    measure: MeasureId,
) -> Result<usize, ExternalError> {
    let call = model
        .external_calls
        .get(&measure)
        .cloned()
        .ok_or(ExternalError::NotExternal(measure))?;

    let func = model
        .external_fns
        .get(&call.func)
        .cloned()
        .ok_or_else(|| ExternalError::UnknownFunction(call.func.clone()))?;

    if func.arity() != call.arg_measures.len() {
        return Err(ExternalError::ArgCountMismatch {
            func: call.func.clone(),
            expected: func.arity(),
            got: call.arg_measures.len(),
        });
    }

    // Collect each argument measure's cells: coord -> value.
    let arg_cells: Vec<BTreeMap<Coordinate, Value>> = call
        .arg_measures
        .iter()
        .map(|mid| {
            model
                .inputs
                .iter()
                .filter(|((m, _), _)| m == mid)
                .map(|((_, coord), v)| (coord.clone(), v.clone()))
                .collect()
        })
        .collect();

    // The coordinates to evaluate: those present in every argument. Use the
    // first argument's coordinates as the candidate set (all args must agree on
    // the value at each coordinate for a meaningful call). For 0-arg functions
    // there is a single empty-coordinate cell.
    let coords: Vec<Coordinate> = match arg_cells.first() {
        Some(first) => first
            .keys()
            .filter(|c| arg_cells.iter().all(|m| m.contains_key(*c)))
            .cloned()
            .collect(),
        None => vec![Coordinate::new()],
    };

    // Evaluate everything first: a failure leaves the old cells in place.
    let mut results = Vec::with_capacity(coords.len());
    for coord in coords {
        let args: Vec<Value> = arg_cells
            .iter()
            .map(|m| m.get(&coord).cloned().unwrap_or(Value::Number(f64::NAN)))
            .collect();
        let v = improv_extfn::eval(&func, &args, timeout).map_err(|e| ExternalError::Eval {
            func: call.func.clone(),
            coord: coord_label(model, &coord),
            msg: e.to_string(),
        })?;
        results.push((coord, v));
    }
    model.inputs.retain(|(m, _), _| *m != measure);
    let count = results.len();
    for (coord, v) in results {
        model.set_input(measure, coord, v);
    }
    Ok(count)
}

/// `Product=WidgetA, Time=Q1` (item names, falling back to ids).
fn coord_label(model: &Model, coord: &Coordinate) -> String {
    if coord.dims.is_empty() {
        return "the scalar cell".into();
    }
    coord
        .dims
        .iter()
        .map(|(c, i)| {
            let cat = model
                .categories
                .get(c)
                .map_or(c.0.to_string(), |c| c.name.0.clone());
            let item = model
                .items
                .get(i)
                .map_or(i.0.to_string(), |i| i.name.0.clone());
            format!("{cat}={item}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use improv_core_model::{
        CategoryId, ExternalCall, ExternalFn, ItemId, Language, Measure, MeasureKind, Name,
        ValueType,
    };

    fn python_available() -> bool {
        // python3 must run through the host sandbox: Restricted fails closed
        // without a working bwrap, and the test then skips.
        let probe = ExternalFn {
            name: "probe".into(),
            language: Language::Python,
            body: "result = 1".into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        improv_extfn::eval(&probe, &[], std::time::Duration::from_secs(5)).is_ok()
    }

    fn base_model() -> Model {
        let mut m = Model::new();
        let product = CategoryId(2);
        m.add_category(product, "Product");
        m.add_item(ItemId(20), product, "A");
        m.add_item(ItemId(21), product, "B");
        // Two input measures over Product.
        for (id, name) in [(MeasureId(100), "X"), (MeasureId(101), "Y")] {
            m.add_measure(Measure {
                id,
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![product],
                kind: MeasureKind::Input,
                description: None,
            });
        }
        let c = |i| Coordinate::from_pairs([(product, ItemId(i))]);
        m.set_input(MeasureId(100), c(20), Value::Number(3.0));
        m.set_input(MeasureId(100), c(21), Value::Number(10.0));
        m.set_input(MeasureId(101), c(20), Value::Number(4.0));
        m.set_input(MeasureId(101), c(21), Value::Number(20.0));
        m
    }

    #[test]
    fn unknown_function_errors() {
        let mut m = base_model();
        m.external_calls.insert(
            MeasureId(200),
            ExternalCall {
                func: "nope".into(),
                arg_measures: vec![MeasureId(100)],
                refresh_policy: Default::default(),
            },
        );

        assert!(matches!(
            refresh_external_measure(&mut m, DEFAULT_TIMEOUT, MeasureId(200)),
            Err(ExternalError::UnknownFunction(_))
        ));
    }

    #[test]
    fn arity_mismatch_errors() {
        let mut m = base_model();
        m.external_fns.insert(
            "hypot".into(),
            ExternalFn {
                name: "hypot".into(),
                language: Language::Python,
                body: "result = (args[0]**2 + args[1]**2) ** 0.5".into(),
                arg_types: vec![ValueType::Number, ValueType::Number],
                return_type: ValueType::Number,
                pure: true,
            },
        );
        // Supplies one arg but the function takes two.
        m.external_calls.insert(
            MeasureId(200),
            ExternalCall {
                func: "hypot".into(),
                arg_measures: vec![MeasureId(100)],
                refresh_policy: Default::default(),
            },
        );

        assert!(matches!(
            refresh_external_measure(&mut m, DEFAULT_TIMEOUT, MeasureId(200)),
            Err(ExternalError::ArgCountMismatch { .. })
        ));
    }

    #[test]
    fn evaluates_hypot_over_coordinates() {
        if !python_available() {
            println!("skipped: python3 not found");
            return;
        }
        let mut m = base_model();
        let hypot = ExternalFn {
            name: "hypot".into(),
            language: Language::Python,
            body: "result = (args[0]**2 + args[1]**2) ** 0.5".into(),
            arg_types: vec![ValueType::Number, ValueType::Number],
            return_type: ValueType::Number,
            pure: true,
        };
        m.external_fns.insert("hypot".into(), hypot);
        m.add_measure(Measure {
            id: MeasureId(200),
            name: Name("H".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(2)],
            kind: MeasureKind::Input, // populated by refresh
            description: None,
        });
        m.external_calls.insert(
            MeasureId(200),
            ExternalCall {
                func: "hypot".into(),
                arg_measures: vec![MeasureId(100), MeasureId(101)],
                refresh_policy: Default::default(),
            },
        );

        let n = refresh_external_measure(&mut m, DEFAULT_TIMEOUT, MeasureId(200)).unwrap();
        assert_eq!(n, 2);
        // H[A] = hypot(3,4) = 5 ; H[B] = hypot(10,20) = ~22.36
        let a = Coordinate::from_pairs([(CategoryId(2), ItemId(20))]);
        assert_eq!(m.input(MeasureId(200), &a), Some(&Value::Number(5.0)));
        let b = Coordinate::from_pairs([(CategoryId(2), ItemId(21))]);
        match m.input(MeasureId(200), &b) {
            Some(Value::Number(v)) => assert!((v - 500.0_f64.sqrt()).abs() < 1e-9),
            other => panic!("expected number, got {other:?}"),
        }
    }

    #[test]
    fn zero_arg_function_fills_one_scalar_cell_and_a_failure_keeps_old_cells() {
        let mut m = base_model();
        let wasm = |name: &str, body: &str| ExternalFn {
            name: name.into(),
            language: Language::Wasm,
            body: body.into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        m.external_fns.insert(
            "seven".into(),
            wasm(
                "seven",
                r#"wat:(module (func (export "improv_call") (result f64) f64.const 7))"#,
            ),
        );
        m.external_fns
            .insert("bad".into(), wasm("bad", "wat:(module)"));
        for (mid, func) in [(MeasureId(300), "seven"), (MeasureId(301), "bad")] {
            m.external_calls.insert(
                mid,
                ExternalCall {
                    func: func.into(),
                    arg_measures: vec![],
                    refresh_policy: Default::default(),
                },
            );
        }
        assert_eq!(
            refresh_external_measure(&mut m, DEFAULT_TIMEOUT, MeasureId(300)).unwrap(),
            1
        );
        let here = Coordinate::new();
        assert_eq!(m.input(MeasureId(300), &here), Some(&Value::Number(7.0)));
        // A failing call is an error naming the function and cell, and the
        // measure keeps the cells it had (here: one left from earlier).
        m.set_input(MeasureId(301), here.clone(), Value::Number(1.0));
        let err = refresh_external_measure(&mut m, DEFAULT_TIMEOUT, MeasureId(301)).unwrap_err();
        assert!(
            matches!(&err, ExternalError::Eval { func, coord, .. } if func == "bad" && coord == "the scalar cell"),
            "{err}"
        );
        assert_eq!(m.input(MeasureId(301), &here), Some(&Value::Number(1.0)));
        assert!(matches!(
            refresh_external_measure(&mut m, DEFAULT_TIMEOUT, MeasureId(100)),
            Err(ExternalError::NotExternal(_))
        ));
    }
}
