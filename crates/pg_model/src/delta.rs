//! Trigger-mode planning (L1): what an input change must recompute, and
//! in-memory what-if edits.
//!
//! [`plan`] is the decision: recompute the changed inputs' [`dependents`],
//! restricted to the touched coordinates ([`Scope`]) when every measure they
//! read is element-wise (no aggregation, BY/OVER/EXCEPT projection or
//! external call), else in full.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use improv_core_model::{
    CategoryId, Coordinate, Expr, ItemId, MeasureId, MeasureKind, Model, Value, ValueType,
};
use improv_engine::compiler::scalar_arity;

use crate::{quote_ident, Error, Result, ResultRows};

/// Postgres's identifier limit (`NAMEDATALEN - 1`).
const MAX_IDENT: usize = 63;

/// The derived measures to recompute when `roots` change: the derived roots
/// themselves and every derived measure that transitively reads a root,
/// sorted by id.
// ponytail: fixpoint over all measures, O(measures × depth); build a reverse
// index if models reach thousands of measures.
pub fn dependents(model: &Model, roots: &[MeasureId]) -> Vec<MeasureId> {
    let mut hit: BTreeSet<MeasureId> = roots.iter().copied().collect();
    loop {
        let before = hit.len();
        for m in model.measures.values() {
            if let MeasureKind::Derived(f) = &m.kind {
                if !hit.contains(&m.id) && f.referenced_measures().iter().any(|r| hit.contains(r)) {
                    hit.insert(m.id);
                }
            }
        }
        if hit.len() == before {
            break;
        }
    }
    hit.into_iter()
        .filter(|id| model.measures.get(id).is_some_and(|m| !m.is_input()))
        .collect()
}

/// The coordinates a statement touched, over one relation's dimension
/// categories.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Scope {
    /// The touched categories.
    pub cats: Vec<CategoryId>,
    /// Touched coordinates: one item per [`Scope::cats`] entry.
    pub keys: BTreeSet<Vec<ItemId>>,
}

impl Scope {
    /// From transition-table rows (item names in `cats` order). A row with a
    /// NULL or never-minted value is skipped: no cell can have it.
    pub fn from_rows(model: &Model, cats: &[CategoryId], rows: &[Vec<Option<String>>]) -> Scope {
        let index: HashMap<(CategoryId, &str), ItemId> = model
            .items
            .iter()
            .map(|(id, i)| ((i.category, i.name.0.as_str()), *id))
            .collect();
        let keys = rows
            .iter()
            .filter(|r| r.len() == cats.len())
            .filter_map(|r| {
                cats.iter()
                    .zip(r)
                    .map(|(c, v)| index.get(&(*c, v.as_deref()?)).copied())
                    .collect()
            })
            .collect();
        Scope {
            cats: cats.to_vec(),
            keys,
        }
    }

    /// For a measure over `cats`: `(scope index, measure position)` of each
    /// touched category it has, and the touched keys projected onto them.
    fn project(&self, cats: &[CategoryId]) -> (Vec<(usize, usize)>, HashSet<Vec<ItemId>>) {
        let shared: Vec<(usize, usize)> = self
            .cats
            .iter()
            .enumerate()
            .filter_map(|(i, c)| cats.iter().position(|m| m == c).map(|p| (i, p)))
            .collect();
        let keys = self
            .keys
            .iter()
            .filter_map(|k| shared.iter().map(|&(i, _)| k.get(i).copied()).collect())
            .collect();
        (shared, keys)
    }

    /// Drop the input cells no in-scope output cell reads: those whose
    /// projection onto the touched categories was not touched. Valid only
    /// for an element-wise [`Plan`].
    pub fn restrict_inputs(&self, model: &mut Model) {
        let projections: HashMap<MeasureId, (Vec<CategoryId>, HashSet<Vec<ItemId>>)> = model
            .measures
            .values()
            .map(|m| {
                let (shared, keys) = self.project(&m.categories);
                let cats = shared.iter().map(|&(i, _)| self.cats[i]).collect();
                (m.id, (cats, keys))
            })
            .collect();
        model.inputs.retain(|(mid, coord), _| {
            projections.get(mid).is_none_or(|(cats, keys)| {
                cats.iter()
                    .map(|c| coord.dims.get(c).copied())
                    .collect::<Option<Vec<_>>>()
                    .is_some_and(|k| keys.contains(&k))
            })
        });
    }

    /// Keep only `measure`'s result rows in scope.
    pub fn retain_rows(
        &self,
        model: &Model,
        measure: MeasureId,
        rows: &mut ResultRows,
    ) -> Result<()> {
        let m = model
            .measures
            .get(&measure)
            .ok_or_else(|| Error::Catalog(format!("unknown measure {}", measure.0)))?;
        let (shared, keys) = self.project(&m.categories);
        let name = |id: &ItemId| model.items.get(id).map(|i| i.name.0.as_str());
        let names: HashSet<Vec<&str>> = keys
            .iter()
            .filter_map(|k| k.iter().map(name).collect())
            .collect();
        rows.retain(|key, _| {
            shared
                .iter()
                .map(|&(_, p)| key.get(p).map(String::as_str))
                .collect::<Option<Vec<_>>>()
                .is_some_and(|k| names.contains(&k))
        });
        Ok(())
    }
}

/// What a change to some input measures must recompute.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Derived measures to recompute, sorted.
    pub targets: Vec<MeasureId>,
    /// `Some`: only cells in this scope can change, so inputs and results
    /// may be restricted to it. `None`: recompute the targets in full.
    pub scope: Option<Scope>,
}

/// Plan the recompute after a statement changed `changed` input measures at
/// `touched` coordinates: their [`dependents`], restricted to `touched` when
/// every measure those read is element-wise (an output cell then depends
/// only on its inputs' cells at its own coordinate, projected or
/// broadcast), else in full. Nothing touched, nothing to do.
pub fn plan(model: &Model, changed: &[MeasureId], touched: Scope) -> Plan {
    if touched.keys.is_empty() {
        return Plan {
            targets: Vec::new(),
            scope: None,
        };
    }
    let targets = dependents(model, changed);
    let elementwise = model
        .measure_dependency_closure(&targets)
        .iter()
        .all(|id| elementwise(model, *id));
    Plan {
        targets,
        scope: elementwise.then_some(touched),
    }
}

/// An input, or a formula of operators and scalar functions over plain
/// references to measures whose categories are all `id`'s own.
fn elementwise(model: &Model, id: MeasureId) -> bool {
    if model.external_calls.contains_key(&id) {
        return false;
    }
    match model.measures.get(&id).map(|m| (&m.kind, &m.categories)) {
        Some((MeasureKind::Input, _)) => true,
        Some((MeasureKind::Derived(f), cats)) => expr_elementwise(model, cats, &f.expr),
        None => false,
    }
}

fn expr_elementwise(model: &Model, cats: &[CategoryId], e: &Expr) -> bool {
    match e {
        Expr::Literal(_) => true,
        Expr::Ref(r, spec) => {
            spec.by.is_empty()
                && spec.over.is_empty()
                && spec.except.is_empty()
                && model
                    .measures
                    .get(r)
                    .is_some_and(|m| m.categories.iter().all(|c| cats.contains(c)))
        }
        Expr::UnaryOp(_, a) => expr_elementwise(model, cats, a),
        Expr::BinaryOp(_, a, b) => {
            expr_elementwise(model, cats, a) && expr_elementwise(model, cats, b)
        }
        Expr::Call(f, args) => {
            scalar_arity(*f).is_some() && args.iter().all(|a| expr_elementwise(model, cats, a))
        }
    }
}

/// Measure ids by name (case-insensitive), in order.
pub fn measure_ids(model: &Model, names: &[String]) -> Result<Vec<MeasureId>> {
    names
        .iter()
        .map(|n| {
            find_measure(model, n).ok_or_else(|| Error::Catalog(format!("unknown measure {n:?}")))
        })
        .collect()
}

fn find_measure(model: &Model, name: &str) -> Option<MeasureId> {
    model
        .measures
        .values()
        .find(|m| m.name.0.eq_ignore_ascii_case(name))
        .map(|m| m.id)
}

/// Apply what-if `edits` to `model`'s input cells in memory and return the
/// edited measures. `edits` is a JSON array of `{"measure": name, "coord":
/// {category: item}, "value": number | null}` (measure and category names
/// case-insensitive; `null` removes the cell).
pub fn apply_edits(model: &mut Model, edits: &str) -> Result<Vec<MeasureId>> {
    use serde_json::Value as J;
    let bad = |what: String| Error::Catalog(format!("what_if edit: {what}"));
    let json: J = serde_json::from_str(edits).map_err(|e| bad(e.to_string()))?;
    let list = json
        .as_array()
        .ok_or_else(|| bad("edits must be a JSON array".into()))?;
    let mut changed = BTreeSet::new();
    for e in list {
        let name = e
            .get("measure")
            .and_then(J::as_str)
            .ok_or_else(|| bad(format!("{e} has no \"measure\" name")))?;
        let id =
            find_measure(model, name).ok_or_else(|| bad(format!("unknown measure {name:?}")))?;
        let m = &model.measures[&id];
        if m.value_type != ValueType::Number || !m.is_input() {
            return Err(bad(format!("{name} is not a number input measure")));
        }
        let want: BTreeSet<CategoryId> = m.categories.iter().copied().collect();
        let coord = e
            .get("coord")
            .and_then(J::as_object)
            .ok_or_else(|| bad(format!("{e} has no \"coord\" object")))?;
        let mut dims = BTreeMap::new();
        for (cat_name, item) in coord {
            let (cat, _) = model
                .categories
                .iter()
                .find(|(_, c)| c.name.0.eq_ignore_ascii_case(cat_name))
                .ok_or_else(|| bad(format!("unknown category {cat_name:?}")))?;
            let item = item
                .as_str()
                .ok_or_else(|| bad(format!("{item} is not an item name")))?;
            let (it, _) = model
                .items
                .iter()
                .find(|(_, i)| i.category == *cat && i.name.0 == item)
                .ok_or_else(|| bad(format!("{cat_name} has no item {item:?}")))?;
            dims.insert(*cat, *it);
        }
        if !dims.keys().eq(want.iter()) {
            return Err(bad(format!("coord must name exactly {name}'s categories")));
        }
        let coord = Coordinate { dims };
        match e.get("value") {
            Some(J::Number(n)) => {
                let v = n
                    .as_f64()
                    .ok_or_else(|| bad(format!("{n} is out of range")))?;
                model.set_input(id, coord, Value::Number(v));
            }
            Some(J::Null) => {
                model.inputs.remove(&(id, coord));
            }
            _ => return Err(bad(format!("{e} needs a number or null \"value\""))),
        }
        changed.insert(id);
    }
    Ok(changed.into_iter().collect())
}

/// The cells of `new` that differ from `old` (`Some`), and the cells of
/// `old` that `new` lacks (`None`), in key order.
pub fn cell_changes(old: &ResultRows, new: &ResultRows) -> Vec<(Vec<String>, Option<f64>)> {
    let mut out: Vec<(Vec<String>, Option<f64>)> = new
        .iter()
        .filter(|(k, v)| old.get(*k).map(|o| o.to_bits()) != Some(v.to_bits()))
        .map(|(k, v)| (k.clone(), Some(*v)))
        .collect();
    out.extend(
        old.keys()
            .filter(|k| !new.contains_key(*k))
            .map(|k| (k.clone(), None)),
    );
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// A result key (item names in `measure`'s category order) as
/// `{"<category>": "<item>"}`.
pub fn coord_json(model: &Model, measure: MeasureId, key: &[String]) -> Result<serde_json::Value> {
    let m = model
        .measures
        .get(&measure)
        .ok_or_else(|| Error::Catalog(format!("unknown measure {}", measure.0)))?;
    let mut obj = serde_json::Map::new();
    for (c, item) in m.categories.iter().zip(key) {
        let cat = model
            .categories
            .get(c)
            .ok_or_else(|| Error::Catalog(format!("unknown category {}", c.0)))?;
        obj.insert(cat.name.0.clone(), item.clone().into());
    }
    Ok(obj.into())
}

/// The sync trigger for `op` (`ins` / `upd` / `del`):
/// `improv_<lower(model)>_sync_<op>`, which must fit an identifier.
pub fn sync_trigger_name(model: &str, op: &str) -> Result<String> {
    let name = format!("improv_{}_sync_{op}", model.to_ascii_lowercase());
    if name.len() > MAX_IDENT {
        return Err(Error::Catalog(format!(
            "model name {model:?} is too long for trigger mode (at most {} bytes)",
            MAX_IDENT + model.len() - name.len()
        )));
    }
    Ok(name)
}

/// A SQL string literal.
pub fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `SELECT` of the distinct dimension values (as text) in a statement's
/// transition tables, given the names of those present; `None` when there
/// is nothing to read.
pub fn touched_sql(old: Option<&str>, new: Option<&str>, cols: &[String]) -> Option<String> {
    if cols.is_empty() {
        return None;
    }
    let dims: Vec<String> = cols
        .iter()
        .map(|c| format!("{}::text", quote_ident(c)))
        .collect();
    let parts: Vec<String> = [old, new]
        .into_iter()
        .flatten()
        .map(|t| format!("SELECT {} FROM {}", dims.join(", "), quote_ident(t)))
        .collect();
    (!parts.is_empty()).then(|| parts.join(" UNION "))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::{build_model, define, result_rows, Catalog, CategoryRow, ItemRow, MeasureRow};
    use improv_engine::dataflow::evaluate;

    const PRICE: MeasureId = MeasureId(1);
    const QTY: MeasureId = MeasureId(2);
    const COST: MeasureId = MeasureId(3);
    const T: CategoryId = CategoryId(1);
    const P: CategoryId = CategoryId(2);
    const ALL: [&str; 4] = [
        "Revenue = Price * Quantity",
        "Total = SUM(Revenue OVER Time)",
        "Margin = Price - Cost",
        "Overhead = Cost * 2",
    ];

    fn input(id: u32, name: &str, cats: &[u32]) -> MeasureRow {
        MeasureRow {
            id,
            name: name.into(),
            value_type: "number".into(),
            kind: "input".into(),
            formula: None,
            description: None,
            categories: cats.to_vec(),
        }
    }

    fn coord(pairs: &[(u32, u32)]) -> Coordinate {
        Coordinate::from_pairs(pairs.iter().map(|&(c, i)| (CategoryId(c), ItemId(i))))
    }

    /// Price, Quantity [Time, Product] and Cost [Product] over
    /// {Q1, Q2} × {Widget, Gadget} (items 1..4), every input cell set, plus
    /// `defs` as measures 10, 11, ...
    fn model(defs: &[&str]) -> Model {
        let mut cat = Catalog::default();
        cat.categories.extend([
            CategoryRow {
                id: 1,
                name: "Time".into(),
            },
            CategoryRow {
                id: 2,
                name: "Product".into(),
            },
        ]);
        cat.items.extend(
            [
                (1, 1, "Q1"),
                (2, 1, "Q2"),
                (3, 2, "Widget"),
                (4, 2, "Gadget"),
            ]
            .map(|(id, category_id, n)| ItemRow {
                id,
                category_id,
                name: n.into(),
            }),
        );
        cat.measures.extend([
            input(1, "Price", &[1, 2]),
            input(2, "Quantity", &[1, 2]),
            input(3, "Cost", &[2]),
        ]);
        for (id, d) in (10..).zip(defs) {
            let row = define(&build_model(&cat).unwrap(), d, id).unwrap();
            cat.measures.push(row);
        }
        let mut m = build_model(&cat).unwrap();
        for p in [3, 4] {
            m.set_input(COST, coord(&[(2, p)]), Value::Number(f64::from(p)));
            for t in [1, 2] {
                let c = coord(&[(1, t), (2, p)]);
                m.set_input(PRICE, c.clone(), Value::Number(f64::from(10 * p)));
                m.set_input(QTY, c, Value::Number(f64::from(100 * t + p)));
            }
        }
        m
    }

    fn ids(v: &[u32]) -> Vec<MeasureId> {
        v.iter().map(|&i| MeasureId(i)).collect()
    }

    fn touched(m: &Model, cats: &[CategoryId], rows: &[&[Option<&str>]]) -> Scope {
        let rows: Vec<Vec<Option<String>>> = rows
            .iter()
            .map(|r| r.iter().map(|v| v.map(String::from)).collect())
            .collect();
        Scope::from_rows(m, cats, &rows)
    }

    #[test]
    fn dependents_follow_formulas_transitively() {
        let m = model(&ALL);
        assert_eq!(dependents(&m, &[PRICE]), ids(&[10, 11, 12]));
        assert_eq!(dependents(&m, &[QTY]), ids(&[10, 11]));
        assert_eq!(dependents(&m, &[COST]), ids(&[12, 13]));
        assert_eq!(
            dependents(&m, &[MeasureId(10)]),
            ids(&[10, 11]),
            "a derived root too"
        );
        assert!(dependents(&m, &[]).is_empty());
    }

    #[test]
    fn plan_restricts_only_element_wise_chains() {
        let m = model(&ALL);
        let s = touched(&m, &[T, P], &[&[Some("Q1"), Some("Widget")]]);
        // Cost feeds Margin (broadcast, larger operand left) and Overhead.
        let p = plan(&m, &[COST], s.clone());
        assert_eq!(p.targets, ids(&[12, 13]));
        assert_eq!(p.scope.as_ref(), Some(&s));
        // Price also feeds Total, an aggregation: recompute in full.
        let p = plan(&m, &[PRICE], s.clone());
        assert_eq!(p.targets, ids(&[10, 11, 12]));
        assert_eq!(p.scope, None);
        // A derived measure reading Total is not element-wise either.
        let m2 = model(&[
            "Revenue = Price * Quantity",
            "Total = SUM(Revenue OVER Time)",
            "Share = Total * 2",
        ]);
        assert_eq!(plan(&m2, &[MeasureId(11)], s.clone()).scope, None);
        // Scalar functions are element-wise.
        let m3 = model(&["Spread = ABS(Price - Cost)", "Neg = -Spread"]);
        assert_eq!(plan(&m3, &[PRICE], s.clone()).scope, Some(s));
        // Nothing touched: nothing to do.
        let p = plan(&m, &[PRICE], Scope::default());
        assert!(p.targets.is_empty());
    }

    #[test]
    fn scope_skips_null_unknown_and_short_rows() {
        let m = model(&[]);
        let s = touched(
            &m,
            &[T, P],
            &[
                &[Some("Q1"), Some("Widget")],
                &[None, Some("Gadget")],
                &[Some("Q9"), Some("Widget")],
                &[Some("Q1")],
            ],
        );
        assert_eq!(s.keys, BTreeSet::from([vec![ItemId(1), ItemId(3)]]));
    }

    #[test]
    fn retain_rows_projects_the_scope() {
        let m = model(&[]);
        let all: ResultRows = [
            ("Q1", "Widget"),
            ("Q1", "Gadget"),
            ("Q2", "Widget"),
            ("Q2", "Gadget"),
        ]
        .into_iter()
        .map(|(t, p)| (vec![t.to_string(), p.to_string()], 1.0))
        .collect();
        let kept = |s: Scope| {
            let mut rows = all.clone();
            s.retain_rows(&m, PRICE, &mut rows).unwrap();
            rows.keys().map(|k| k.join("/")).collect::<Vec<_>>()
        };
        assert_eq!(
            kept(touched(&m, &[P], &[&[Some("Widget")]])),
            ["Q1/Widget", "Q2/Widget"]
        );
        assert_eq!(
            kept(touched(&m, &[T, P], &[&[Some("Q2"), Some("Gadget")]])),
            ["Q2/Gadget"]
        );
        let mut rows = all.clone();
        assert!(Scope::default()
            .retain_rows(&m, MeasureId(99), &mut rows)
            .is_err());
    }

    #[test]
    fn restricted_inputs_compute_the_in_scope_cells() {
        let full_model = model(&[
            "Revenue = Price * Quantity",
            "Margin = Price - Cost",
            "Net = Revenue - Cost",
        ]);
        let targets = ids(&[10, 11, 12]);
        let full = evaluate(&full_model, &targets).unwrap();
        for s in [
            touched(&full_model, &[T, P], &[&[Some("Q2"), Some("Gadget")]]),
            touched(&full_model, &[P], &[&[Some("Widget")]]),
        ] {
            let mut m = full_model.clone();
            s.restrict_inputs(&mut m);
            assert!(m.inputs.len() < full_model.inputs.len());
            let part = evaluate(&m, &targets).unwrap();
            for t in &targets {
                let mut want = result_rows(&full_model, *t, &full[t]).unwrap();
                s.retain_rows(&full_model, *t, &mut want).unwrap();
                let mut got = result_rows(&m, *t, &part[t]).unwrap();
                s.retain_rows(&m, *t, &mut got).unwrap();
                assert!(!want.is_empty());
                assert_eq!(got, want, "measure {t:?}");
            }
        }
    }

    #[test]
    fn edits_set_and_clear_input_cells() {
        let mut m = model(&["Revenue = Price * Quantity"]);
        let changed = apply_edits(
            &mut m,
            r#"[{"measure": "price", "coord": {"Time": "Q1", "product": "Widget"}, "value": 11},
                {"measure": "Cost", "coord": {"Product": "Gadget"}, "value": null}]"#,
        )
        .unwrap();
        assert_eq!(changed, vec![PRICE, COST]);
        assert_eq!(
            m.input(PRICE, &coord(&[(1, 1), (2, 3)])),
            Some(&Value::Number(11.0))
        );
        assert_eq!(m.input(COST, &coord(&[(2, 4)])), None);
        for (edits, err) in [
            ("{", "what_if edit"),
            ("{}", "array"),
            (r#"[{"coord": {}}]"#, "\"measure\""),
            (r#"[{"measure": "Nope"}]"#, "unknown measure"),
            (r#"[{"measure": "Revenue"}]"#, "not a number input"),
            (r#"[{"measure": "Cost", "value": 1}]"#, "\"coord\""),
            (
                r#"[{"measure": "Cost", "coord": {"Shape": "Round"}}]"#,
                "unknown category",
            ),
            (
                r#"[{"measure": "Cost", "coord": {"Product": 3}}]"#,
                "not an item name",
            ),
            (
                r#"[{"measure": "Cost", "coord": {"Product": "Nope"}}]"#,
                "no item",
            ),
            (
                r#"[{"measure": "Cost", "coord": {"Time": "Q1"}}]"#,
                "exactly",
            ),
            (
                r#"[{"measure": "Cost", "coord": {"Product": "Widget"}, "value": "x"}]"#,
                "value",
            ),
            (
                r#"[{"measure": "Cost", "coord": {"Product": "Widget"}}]"#,
                "value",
            ),
        ] {
            let e = apply_edits(&mut m.clone(), edits).unwrap_err().to_string();
            assert!(e.contains(err), "{edits}: {e}");
        }
    }

    #[test]
    fn changes_and_coord_json() {
        let rows = |v: &[(&str, f64)]| -> ResultRows {
            v.iter().map(|(k, x)| (vec![k.to_string()], *x)).collect()
        };
        let old = rows(&[("a", 1.0), ("b", 2.0), ("c", 3.0)]);
        let new = rows(&[("a", 1.0), ("b", 5.0), ("d", 4.0)]);
        assert_eq!(
            cell_changes(&old, &new),
            vec![
                (vec!["b".to_string()], Some(5.0)),
                (vec!["c".to_string()], None),
                (vec!["d".to_string()], Some(4.0)),
            ]
        );
        let m = model(&[]);
        assert_eq!(
            coord_json(&m, PRICE, &["Q1".into(), "Widget".into()]).unwrap(),
            serde_json::json!({"Time": "Q1", "Product": "Widget"})
        );
        assert!(coord_json(&m, MeasureId(99), &[]).is_err());
    }

    #[test]
    fn names_and_sql() {
        assert_eq!(
            sync_trigger_name("Plan", "upd").unwrap(),
            "improv_plan_sync_upd"
        );
        assert!(sync_trigger_name(&"m".repeat(47), "ins").is_ok());
        let e = sync_trigger_name(&"m".repeat(48), "ins").unwrap_err();
        assert!(e.to_string().contains("at most 47 bytes"), "{e}");
        assert_eq!(quote_literal("it's"), "'it''s'");
        let cols = ["quarter".to_string(), "pro\"duct".to_string()];
        assert_eq!(
            touched_sql(Some("improv_old"), Some("improv_new"), &cols).unwrap(),
            "SELECT \"quarter\"::text, \"pro\"\"duct\"::text FROM \"improv_old\" UNION \
             SELECT \"quarter\"::text, \"pro\"\"duct\"::text FROM \"improv_new\""
        );
        assert_eq!(
            touched_sql(None, Some("n"), &cols[..1]).unwrap(),
            "SELECT \"quarter\"::text FROM \"n\""
        );
        assert_eq!(touched_sql(None, None, &cols), None);
        assert_eq!(touched_sql(Some("o"), None, &[]), None);
        let m = model(&[]);
        assert_eq!(
            measure_ids(&m, &["COST".into(), "price".into()]).unwrap(),
            vec![COST, PRICE]
        );
        assert!(measure_ids(&m, &["nope".into()]).is_err());
    }
}
