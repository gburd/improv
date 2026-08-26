//! GUI application state and rendering.
//!
//! State: the loaded `Model`, a live `session::Engine` (built over all derived
//! measures), the current derived-measure snapshot, and the selected measure.
//! The panels (model explorer, pivot grid, formula editor, inspector) are
//! rendered each frame from this state; the fuller panels land as Phase 5
//! increments.

use std::collections::HashMap;

use improv_core_model::{CategoryId, ItemId, MeasureId, Model, Value};
use improv_engine::session::{Engine, MeasureValues};
use improv_engine::{encode_coord, CoordKey};
use improv_storage_mentat::ModelStore;

/// The running GUI application.
// `db`/`engine`/`set_cell` are wired by the grid-editing increment; kept now so
// the state shape is stable for the panels that follow.
#[allow(dead_code)]
pub struct ImprovApp {
    /// The store path (empty = in-memory scratch); used for saving.
    db: String,
    model: Model,
    /// Live incremental engine over all derived measures, plus its snapshot.
    engine: Option<Engine>,
    snapshot: HashMap<MeasureId, MeasureValues>,
    /// The measure currently shown in the grid.
    selected: Option<MeasureId>,
    status: String,
}

impl ImprovApp {
    /// Load a model from the store at `db` (`""` = fresh in-memory model) and
    /// build the live engine over its derived measures.
    pub fn load(db: &str) -> Result<ImprovApp, String> {
        let model = if db.is_empty() {
            Model::new()
        } else {
            let mut store = ModelStore::open(db).map_err(|e| e.to_string())?;
            store.load_model().map_err(|e| e.to_string())?
        };

        let derived: Vec<MeasureId> = model
            .measures
            .values()
            .filter(|m| m.is_derived())
            .map(|m| m.id)
            .collect();

        let (engine, snapshot) = if derived.is_empty() {
            (None, HashMap::new())
        } else {
            match Engine::new(&model, &derived) {
                Ok((e, snap)) => (Some(e), snap),
                Err(e) => (None, {
                    // Fall back to no live engine; the grid still shows inputs.
                    eprintln!("improv-gui: engine build failed: {e}");
                    HashMap::new()
                }),
            }
        };

        // Default selection: first derived measure, else first measure by id.
        let selected = pick_default_measure(&model);

        Ok(ImprovApp {
            db: db.to_string(),
            model,
            engine,
            snapshot,
            selected,
            status: String::new(),
        })
    }

    /// The numeric value map for a measure (input cells, or the derived
    /// snapshot projected to numbers for the grid).
    fn values_for(&self, measure: MeasureId) -> HashMap<CoordKey, f64> {
        let is_derived = self.model.measures.get(&measure).map(|m| m.is_derived());
        match is_derived {
            Some(true) => self
                .snapshot
                .get(&measure)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_num().map(|n| (k.clone(), n)))
                        .collect()
                })
                .unwrap_or_default(),
            _ => self
                .model
                .inputs
                .iter()
                .filter(|((mid, _), _)| *mid == measure)
                .filter_map(|((_, coord), v)| match v {
                    Value::Number(n) => Some((encode_coord(coord), *n)),
                    _ => None,
                })
                .collect(),
        }
    }

    /// Set an input cell and push the edit through the live engine, refreshing
    /// the snapshot. Returns an error string on failure (e.g. derived cell).
    #[allow(dead_code)]
    pub fn set_cell(
        &mut self,
        measure: MeasureId,
        coord: CoordKey,
        value: f64,
    ) -> Result<(), String> {
        if self
            .model
            .measures
            .get(&measure)
            .map(|m| m.is_derived())
            .unwrap_or(false)
        {
            return Err("derived cells are computed, not editable".into());
        }
        self.model
            .set_input(measure, decode(&coord), Value::Number(value));
        if let Some(engine) = &mut self.engine {
            self.snapshot = engine
                .set(measure, coord, value)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[allow(dead_code)]
fn decode(k: &CoordKey) -> improv_core_model::Coordinate {
    improv_core_model::Coordinate::from_pairs(k.iter().map(|(c, i)| (CategoryId(*c), ItemId(*i))))
}

fn pick_default_measure(model: &Model) -> Option<MeasureId> {
    let mut ids: Vec<MeasureId> = model.measures.keys().copied().collect();
    ids.sort_by_key(|m| m.0);
    ids.iter()
        .find(|m| {
            model
                .measures
                .get(m)
                .map(|x| x.is_derived())
                .unwrap_or(false)
        })
        .copied()
        .or_else(|| ids.first().copied())
}

impl eframe::App for ImprovApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Left: model explorer (measure list). Center: pivot grid.
        egui::SidePanel::left("explorer")
            .resizable(true)
            .default_width(200.0)
            .show(ctx, |ui| {
                ui.heading("Measures");
                ui.separator();
                let mut ids: Vec<MeasureId> = self.model.measures.keys().copied().collect();
                ids.sort_by_key(|m| m.0);
                for id in ids {
                    let m = &self.model.measures[&id];
                    let label = format!(
                        "{} {}{}",
                        id.0,
                        m.name.0,
                        if m.is_derived() { " (=)" } else { "" }
                    );
                    if ui
                        .selectable_label(self.selected == Some(id), label)
                        .clicked()
                    {
                        self.selected = Some(id);
                    }
                }
            });

        egui::CentralPanel::default().show(ctx, |ui| match self.selected {
            None => {
                ui.label("No measures. Open a model store with `improv-gui <db>`.");
            }
            Some(mid) => {
                let (name, cats) = {
                    let m = &self.model.measures[&mid];
                    (m.name.0.clone(), m.categories.clone())
                };
                ui.heading(&name);
                ui.label(format!(
                    "dimensions: {}",
                    cats.iter()
                        .map(|c| self.model.categories[c].name.0.clone())
                        .collect::<Vec<_>>()
                        .join(" x ")
                ));
                ui.separator();
                self.render_grid(ui, mid, &cats);
                if !self.status.is_empty() {
                    ui.separator();
                    ui.label(&self.status);
                }
            }
        });
    }
}

impl ImprovApp {
    /// Render `measure` as a 2-D pivot grid: first category on rows, second on
    /// columns, remaining dims pinned to their first item.
    fn render_grid(&mut self, ui: &mut egui::Ui, measure: MeasureId, cats: &[CategoryId]) {
        let values = self.values_for(measure);
        let items = |c: CategoryId| -> Vec<(ItemId, String)> {
            let mut v: Vec<(ItemId, String)> = self
                .model
                .categories
                .get(&c)
                .map(|cat| {
                    cat.items
                        .iter()
                        .filter_map(|id| {
                            self.model.items.get(id).map(|it| (*id, it.name.0.clone()))
                        })
                        .collect()
                })
                .unwrap_or_default();
            v.sort_by_key(|(id, _)| id.0);
            v
        };

        let row_cat = cats.first().copied();
        let col_cat = cats.get(1).copied();
        let rows = row_cat
            .map(items)
            .unwrap_or_else(|| vec![(ItemId(0), String::new())]);
        let cols = col_cat
            .map(items)
            .unwrap_or_else(|| vec![(ItemId(0), String::new())]);
        // Pin extra dims to their first item.
        let pinned: Vec<(CategoryId, ItemId)> = cats
            .iter()
            .skip(2)
            .filter_map(|c| items(*c).first().map(|(id, _)| (*c, *id)))
            .collect();

        use egui_extras::{Column, TableBuilder};
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().resizable(true));
        for _ in &cols {
            table = table.column(Column::auto().resizable(true));
        }
        table
            .header(20.0, |mut header| {
                header.col(|ui| {
                    ui.strong("");
                });
                for (_, cname) in &cols {
                    header.col(|ui| {
                        ui.strong(cname);
                    });
                }
            })
            .body(|mut body| {
                for (rid, rname) in &rows {
                    body.row(18.0, |mut row| {
                        row.col(|ui| {
                            ui.strong(rname);
                        });
                        for (cid, _) in &cols {
                            let key = cell_key(row_cat, col_cat, *rid, *cid, &pinned);
                            let text = values.get(&key).map(|v| format!("{v}")).unwrap_or_default();
                            row.col(|ui| {
                                ui.label(text);
                            });
                        }
                    });
                }
            });
    }
}

/// The sorted `CoordKey` for a cell, given the row/col categories, this cell's
/// row/col items, and the pinned extra dims.
fn cell_key(
    row_cat: Option<CategoryId>,
    col_cat: Option<CategoryId>,
    rid: ItemId,
    cid: ItemId,
    pinned: &[(CategoryId, ItemId)],
) -> CoordKey {
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    if let Some(c) = row_cat {
        pairs.push((c.0, rid.0));
    }
    if let Some(c) = col_cat {
        pairs.push((c.0, cid.0));
    }
    for (c, i) in pinned {
        pairs.push((c.0, i.0));
    }
    pairs.sort();
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{
        BinaryOp, DimensionSpec, Expr, Formula, Measure, MeasureKind, Name, ValueType,
    };

    fn revenue_model() -> Model {
        let mut m = Model::new();
        let (t, p) = (CategoryId(1), CategoryId(2));
        m.add_category(t, "Time");
        m.add_category(p, "Product");
        m.add_item(ItemId(10), t, "2025");
        m.add_item(ItemId(20), p, "WidgetA");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
        m.set_input(MeasureId(100), c(&[(p, ItemId(20))]), Value::Number(10.0));
        m.set_input(
            MeasureId(101),
            c(&[(t, ItemId(10)), (p, ItemId(20))]),
            Value::Number(7.0),
        );
        m
    }

    #[test]
    fn loads_and_computes_derived() {
        let app = build_app(revenue_model());
        // Revenue is derived and selected by default.
        assert_eq!(app.selected, Some(MeasureId(102)));
        let vals = app.values_for(MeasureId(102));
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&70.0)); // 10 * 7
    }

    #[test]
    fn editing_recomputes_derived() {
        let mut app = build_app(revenue_model());
        // Set Quantity[2025,WidgetA] = 9 -> Revenue = 90.
        let mut qkey = vec![(1u32, 10u32), (2u32, 20u32)];
        qkey.sort();
        app.set_cell(MeasureId(101), qkey.clone(), 9.0).unwrap();
        let rev = app.values_for(MeasureId(102));
        assert_eq!(rev.get(&qkey), Some(&90.0));
    }

    #[test]
    fn editing_derived_is_rejected() {
        let mut app = build_app(revenue_model());
        let key = vec![(1u32, 10u32), (2u32, 20u32)];
        assert!(app.set_cell(MeasureId(102), key, 1.0).is_err());
    }

    /// Build an app directly from a model (bypassing the store) for tests.
    fn build_app(model: Model) -> ImprovApp {
        let derived: Vec<MeasureId> = model
            .measures
            .values()
            .filter(|m| m.is_derived())
            .map(|m| m.id)
            .collect();
        let (engine, snapshot) = if derived.is_empty() {
            (None, HashMap::new())
        } else {
            let (e, s) = Engine::new(&model, &derived).expect("engine");
            (Some(e), s)
        };
        let selected = pick_default_measure(&model);
        ImprovApp {
            db: String::new(),
            model,
            engine,
            snapshot,
            selected,
            status: String::new(),
        }
    }
}
