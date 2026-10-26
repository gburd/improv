//! `improv_calc(formula [, inputs])`: one scalar Improv formula over named
//! inputs, the engine's own per-cell operators ([`improv_engine::scalar`]),
//! as `LANGUAGE improv` evaluates a function body.
//!
//! Inputs are a JSON object. Each key is an input measure; its value's JSON
//! type gives the measure's type (number, boolean, text; a JSON `null` is a
//! missing input, which makes a result that depends on it NULL). Compiled
//! formulas are cached by formula text and input names *and types*, so a
//! column of rows with the same shape compiles once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use improv_core_model::{parser, Expr, Measure, MeasureId, MeasureKind, Model, Name, ValueType};
use improv_engine::compiler::{infer, CompileContext};
use improv_engine::{scalar, CellValue};

/// A formula compiled against its inputs' names and types.
#[derive(Debug)]
pub struct Compiled {
    /// Input `i` (in key order) is `MeasureId(i + 1)`.
    names: Vec<String>,
    expr: Expr,
}

/// An input value as the formula reads it; `None` is a missing input.
type Input = Option<CellValue>;

/// The inputs of one call, by name, in key order.
pub type Inputs = Vec<(String, Input)>;

/// Parse a JSON object of inputs. A SQL NULL `inputs` argument is no inputs.
/// Keys keep their order; a nested array or object is an error.
pub fn parse_inputs(json: Option<&str>) -> Result<Inputs, String> {
    use serde_json::Value as J;
    let Some(text) = json.map(str::trim).filter(|t| !t.is_empty()) else {
        return Ok(Vec::new());
    };
    let J::Object(map) = serde_json::from_str::<J>(text)
        .map_err(|e| format!("improv_calc: inputs are not valid JSON: {e}"))?
    else {
        return Err("improv_calc: inputs must be a JSON object like {\"rate\": 0.05}".into());
    };
    map.into_iter()
        .map(|(k, v)| {
            let v = match v {
                J::Null => None,
                J::Bool(b) => Some(CellValue::Bool(b)),
                J::Number(n) => Some(CellValue::num(n.as_f64().unwrap_or(f64::NAN))),
                J::String(s) => Some(CellValue::Text(s)),
                J::Array(_) | J::Object(_) => {
                    return Err(format!(
                        "improv_calc: input {k:?} is a JSON array or object; inputs are scalars"
                    ))
                }
            };
            Ok((k, v))
        })
        .collect()
}

/// The type an input is declared with: a missing one is a number (the
/// commonest case; `IF(ISBLANK(x), …)` still works on it).
fn input_type(v: &Input) -> ValueType {
    match v {
        Some(CellValue::Bool(_)) => ValueType::Boolean,
        Some(CellValue::Text(_)) => ValueType::Text,
        Some(CellValue::Date(_)) => ValueType::DateTime,
        _ => ValueType::Number,
    }
}

/// Compile `formula` with these inputs' names and types.
pub fn compile(formula: &str, inputs: &Inputs) -> Result<Compiled, String> {
    let mut model = Model::new();
    for (i, (name, v)) in inputs.iter().enumerate() {
        model.add_measure(Measure {
            id: MeasureId(i as u32 + 1),
            name: Name(name.clone()),
            value_type: input_type(v),
            categories: vec![],
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
    }
    let text = formula.trim();
    let text = text.strip_prefix('=').unwrap_or(text);
    let expr = parser::parse_expr(&model, text)
        .map_err(|e| format!("improv_calc: {e}"))?
        .expr;
    scalar::check(&expr).map_err(|e| format!("improv_calc: {e}"))?;
    infer(&CompileContext::new(&model.measures), MeasureId(0), &expr)
        .map_err(|e| format!("improv_calc: {e}"))?;
    Ok(Compiled {
        names: inputs.iter().map(|(n, _)| n.clone()).collect(),
        expr,
    })
}

impl Compiled {
    /// Evaluate on `inputs` (the shape it was compiled for). `Ok(None)`:
    /// the result depends on a missing input.
    pub fn eval(&self, inputs: &Inputs) -> Result<Option<CellValue>, String> {
        debug_assert_eq!(inputs.len(), self.names.len());
        let env: HashMap<MeasureId, CellValue> = (1..)
            .zip(inputs)
            .filter_map(|(i, (_, v))| v.clone().map(|v| (MeasureId(i), v)))
            .collect();
        scalar::eval(&self.expr, &env).map_err(|e| format!("improv_calc: {e}"))
    }
}

/// The cache key: formula text, then each input's name and type.
type Key = (String, Vec<(String, &'static str)>);

/// A bounded cache of compiled formulas, shared by every thread.
#[derive(Debug)]
pub struct Cache {
    max: usize,
    map: Mutex<HashMap<Key, Arc<Compiled>>>,
}

impl Default for Cache {
    fn default() -> Self {
        Cache::new(1024)
    }
}

impl Cache {
    /// A cache of at most `max` formulas.
    pub fn new(max: usize) -> Self {
        Cache {
            max,
            map: Mutex::new(HashMap::new()),
        }
    }

    /// Formulas cached now.
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether nothing is cached.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<Key, Arc<Compiled>>> {
        self.map.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The compiled `formula` for inputs of this shape, compiling it on a
    /// miss. A compile error is not cached.
    pub fn get(&self, formula: &str, inputs: &Inputs) -> Result<Arc<Compiled>, String> {
        let key: Key = (
            formula.to_string(),
            inputs
                .iter()
                .map(|(n, v)| (n.clone(), improv_pg_model::value_type_name(input_type(v))))
                .collect(),
        );
        if let Some(c) = self.lock().get(&key) {
            return Ok(c.clone());
        }
        // Compile outside the lock: other threads keep evaluating.
        let c = Arc::new(compile(formula, inputs)?);
        let mut map = self.lock();
        // ponytail: clear-all when full, not LRU; a working set larger than
        // `max` recompiles often. Make it LRU if that shows up in profiles.
        if map.len() >= self.max {
            map.clear();
        }
        map.insert(key, c.clone());
        Ok(c)
    }

    /// `improv_calc_text`'s value: any type as text, an error as its code;
    /// `None` is SQL NULL. A formula that does not compile is an `Err`.
    pub fn calc(&self, formula: &str, inputs: Option<&str>) -> Result<Option<CellValue>, String> {
        let inputs = parse_inputs(inputs)?;
        self.get(formula, &inputs)?.eval(&inputs)
    }
}

/// `improv_calc`'s value: a number, a boolean as 1 or 0, a date as its
/// spreadsheet serial (days since 1899-12-30); text and errors are NULL.
pub fn number(v: &CellValue) -> Option<f64> {
    const MS_PER_DAY: f64 = 86_400_000.0;
    const EPOCH_SERIAL: f64 = 25_569.0; // 1970-01-01
    match v {
        CellValue::Num(_) => v.as_num(),
        CellValue::Bool(b) => Some(f64::from(u8::from(*b))),
        CellValue::Date(ms) => Some(*ms as f64 / MS_PER_DAY + EPOCH_SERIAL),
        CellValue::Text(_) | CellValue::Err(_) => None,
    }
}
