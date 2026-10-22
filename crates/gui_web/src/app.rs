//! The web worksheet: one measure's cells, rows × columns, in an `NxWindow`.
//! Input cells edit in place (Enter writes them through
//! `POST /measures/:id/cells`); derived cells are read-only and come back
//! recomputed by the server's engine after each write.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use egui::{vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use serde::Deserialize;

use crate::net;
use crate::theme::{BLACK, DARK_GRAY, LIGHT_GRAY, WHITE};

const CELL: egui::Vec2 = vec2(96.0, 20.0);
const HEADER_W: f32 = 104.0;

/// `GET /measures` row.
#[derive(Deserialize, Clone)]
struct MeasureRow {
    id: u32,
    name: String,
    kind: String,
    categories: Vec<String>,
}

/// `GET /measures/:id/values` row: `[[category, item], ...]` and a number.
#[derive(Deserialize, Clone)]
struct CellRow {
    coord: Vec<[String; 2]>,
    value: f64,
}

/// The subset of `GET /model` the worksheet needs: ids and names.
#[derive(Deserialize, Default)]
struct ModelNames {
    categories: BTreeMap<String, CategoryRow>,
    items: BTreeMap<String, ItemRow>,
}

#[derive(Deserialize)]
struct CategoryRow {
    id: u32,
    name: String,
    items: Vec<u32>,
}

#[derive(Deserialize)]
struct ItemRow {
    name: String,
}

/// A category: its id and its items (id, name), in model order.
#[derive(Clone)]
struct Axis {
    name: String,
    id: u32,
    items: Vec<(u32, String)>,
}

/// What the network callbacks hand back to the next frame.
#[derive(Default)]
struct Inbox {
    model: Option<Result<ModelNames, String>>,
    measures: Option<Result<Vec<MeasureRow>, String>>,
    values: Vec<(u32, Result<Vec<CellRow>, String>)>,
    wrote: Option<Result<(), String>>,
}

pub struct WebApp {
    api: String,
    token: Option<String>,
    inbox: Rc<RefCell<Inbox>>,
    axes: Vec<Axis>,
    measures: Vec<MeasureRow>,
    values: BTreeMap<u32, BTreeMap<Vec<String>, f64>>,
    /// The cell being edited (measure, key), its text, and whether the
    /// text still needs selecting (the first frame of an edit).
    editing: Option<(u32, Vec<String>, String, bool)>,
    status: String,
    in_flight: usize,
}

impl WebApp {
    pub fn new(cc: &eframe::CreationContext<'_>, api: String, token: Option<String>) -> Self {
        crate::theme::apply(&cc.egui_ctx);
        let app = WebApp {
            api,
            token,
            inbox: Rc::default(),
            axes: Vec::new(),
            measures: Vec::new(),
            values: BTreeMap::new(),
            editing: None,
            status: "loading…".into(),
            in_flight: 0,
        };
        app.get_json("model", &cc.egui_ctx, |inbox, r| inbox.model = Some(r));
        app.get_json("measures", &cc.egui_ctx, |inbox, r| {
            inbox.measures = Some(r)
        });
        app
    }

    fn get_json<T: for<'de> Deserialize<'de> + 'static>(
        &self,
        path: &str,
        ctx: &egui::Context,
        put: impl FnOnce(&mut Inbox, Result<T, String>) + 'static,
    ) {
        let (inbox, ctx) = (self.inbox.clone(), ctx.clone());
        let url = format!("{}/{path}", self.api);
        net::fetch("GET", url, None, self.token.clone(), move |r| {
            let parsed = r.and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()));
            put(&mut inbox.borrow_mut(), parsed);
            ctx.request_repaint();
        });
    }

    fn load_values(&mut self, ctx: &egui::Context) {
        for m in self.measures.clone() {
            self.in_flight += 1;
            let id = m.id;
            self.get_json(&format!("measures/{id}/values"), ctx, move |inbox, r| {
                inbox.values.push((id, r));
            });
        }
    }

    /// The categories of `m`, in its order, with their items.
    fn axes_of(&self, m: &MeasureRow) -> Vec<Axis> {
        m.categories
            .iter()
            .filter_map(|c| self.axes.iter().find(|a| &a.name == c).cloned())
            .collect()
    }

    fn drain(&mut self, ctx: &egui::Context) {
        let inbox = std::mem::take(&mut *self.inbox.borrow_mut());
        if let Some(r) = inbox.model {
            match r {
                Ok(m) => {
                    let mut cats: Vec<&CategoryRow> = m.categories.values().collect();
                    cats.sort_by_key(|c| c.id);
                    self.axes = cats
                        .into_iter()
                        .map(|c| Axis {
                            name: c.name.clone(),
                            id: c.id,
                            items: c
                                .items
                                .iter()
                                .filter_map(|i| {
                                    m.items.get(&i.to_string()).map(|it| (*i, it.name.clone()))
                                })
                                .collect(),
                        })
                        .collect();
                }
                Err(e) => self.status = format!("model: {e}"),
            }
        }
        if let Some(r) = inbox.measures {
            match r {
                Ok(ms) => {
                    self.measures = ms;
                    self.load_values(ctx);
                }
                Err(e) => self.status = format!("measures: {e}"),
            }
        }
        for (id, r) in inbox.values {
            self.in_flight = self.in_flight.saturating_sub(1);
            match r {
                Ok(cells) => {
                    let map = cells
                        .into_iter()
                        .map(|c| {
                            let mut key: Vec<(String, String)> =
                                c.coord.into_iter().map(|[a, b]| (a, b)).collect();
                            key.sort();
                            (key.into_iter().map(|(_, i)| i).collect(), c.value)
                        })
                        .collect();
                    self.values.insert(id, map);
                }
                Err(e) => self.status = format!("values: {e}"),
            }
        }
        if let Some(r) = inbox.wrote {
            match r {
                Ok(()) => {
                    self.status = "saved; recalculated by the server".into();
                    self.load_values(ctx);
                }
                Err(e) => self.status = format!("write refused: {e}"),
            }
        }
        if self.in_flight == 0 && self.status == "loading…" && !self.measures.is_empty() {
            self.status = "ready".into();
        }
    }

    /// Write one input cell; on success every measure's values are re-read.
    fn write(&mut self, ctx: &egui::Context, m: &MeasureRow, items: &[String], text: &str) {
        let Ok(value) = text.trim().parse::<f64>() else {
            self.status = format!("not a number: {text:?}");
            return;
        };
        let mut dims = serde_json::Map::new();
        for a in self.axes_of(m) {
            let pos = sorted_index(&m.categories, &a.name);
            if let Some((id, _)) = a.items.iter().find(|(_, n)| Some(n) == items.get(pos)) {
                dims.insert(a.id.to_string(), (*id).into());
            }
        }
        let body = serde_json::json!({ "coord": { "dims": dims }, "value": value }).to_string();
        let (inbox, ctx) = (self.inbox.clone(), ctx.clone());
        let url = format!("{}/measures/{}/cells", self.api, m.id);
        self.status = "saving…".into();
        net::fetch("POST", url, Some(body), self.token.clone(), move |r| {
            inbox.borrow_mut().wrote = Some(r.map(|_| ()));
            ctx.request_repaint();
        });
    }
}

/// A cell's key lists items by category name order (the server sorts its
/// coordinate pairs); where category `c` falls in that order.
fn sorted_index(cats: &[String], c: &str) -> usize {
    let mut sorted: Vec<&String> = cats.iter().collect();
    sorted.sort();
    sorted.iter().position(|x| *x == c).unwrap_or(0)
}

fn fmt(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{v:.0}")
    } else {
        format!("{v:.2}")
    }
}

/// Visible rows of a worksheet window before it scrolls.
const VISIBLE_ROWS: usize = 5;

impl eframe::App for WebApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain(ctx);
        let mut exposed: Vec<serde_json::Value> = Vec::new();
        egui::TopBottomPanel::bottom("status")
            .frame(egui::Frame::none().fill(LIGHT_GRAY).inner_margin(4.0))
            .show(ctx, |ui| {
                ui.label(format!("Improv (web) over improv-server — {}", self.status));
            });
        let ws = egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(DARK_GRAY))
            .show(ctx, |_| {})
            .response
            .rect;
        let mut x = ws.min.x + 24.0;
        let mut commit = None;
        for m in self.measures.clone() {
            let axes = self.axes_of(&m);
            // Rows and columns are the last two categories; any before them
            // are pages, shown at their first item.
            let (pages, rows, cols) = match axes.as_slice() {
                [] => (&[][..], None, None),
                [r] => (&[][..], Some(r.clone()), None),
                [p @ .., r, c] => (p, Some(r.clone()), Some(c.clone())),
            };
            let pages: Vec<(String, String)> = pages
                .iter()
                .filter_map(|a| a.items.first().map(|(_, i)| (a.name.clone(), i.clone())))
                .collect();
            let one = || vec![(0, String::new())];
            let row_items = rows.as_ref().map_or_else(one, |a| a.items.clone());
            let col_items = cols.as_ref().map_or_else(one, |a| a.items.clone());
            let grid = vec2(
                HEADER_W + CELL.x * col_items.len() as f32,
                CELL.y * (row_items.len() + 1) as f32,
            );
            let view = vec2(
                grid.x + 16.0,
                CELL.y * (row_items.len().min(VISIBLE_ROWS) + 1) as f32 + 4.0,
            );
            let rect = Rect::from_min_size(
                egui::pos2(x, ws.min.y + 24.0),
                view + vec2(18.0, 23.0 + 9.0 + 18.0),
            );
            x = rect.max.x + 24.0;
            let kind = if m.kind == "input" {
                "input: double-click to edit"
            } else {
                "derived"
            };
            let page: Vec<&str> = pages.iter().map(|(_, i)| i.as_str()).collect();
            let kind = if page.is_empty() {
                kind.to_owned()
            } else {
                format!("{kind}; page {}", page.join(", "))
            };
            let values = self.values.get(&m.id).cloned().unwrap_or_default();
            let editing = &mut self.editing;
            let _ = crate::nxwin::NxWindow::new(("ws", m.id), format!("{} — {kind}", m.name))
                .rect(rect)
                .resizable(false)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt(("scroll", m.id))
                        .max_height(view.y)
                        .show(ui, |ui| {
                            // Inside the scroll area: the bar keeps the trough gray.
                            crate::theme::content(ui);
                            let (area, _) = ui.allocate_exact_size(grid, Sense::hover());
                            let clip = ui.clip_rect();
                            let origin = area.min;
                            let p = ui.painter().clone();
                            let font = FontId::proportional(12.0);
                            let at = |r: usize, c: usize| {
                                let (x, w) = if c == 0 {
                                    (0.0, HEADER_W)
                                } else {
                                    (HEADER_W + CELL.x * (c - 1) as f32, CELL.x)
                                };
                                Rect::from_min_size(
                                    origin + vec2(x, CELL.y * r as f32),
                                    vec2(w, CELL.y),
                                )
                            };
                            let header = |r: Rect, text: &str, left: bool| {
                                p.rect_filled(r, 0.0, LIGHT_GRAY);
                                p.rect_stroke(r, 0.0, Stroke::new(1.0_f32, BLACK));
                                let (pos, align) = if left {
                                    (r.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER)
                                } else {
                                    (r.center(), Align2::CENTER_CENTER)
                                };
                                p.text(pos, align, text, font.clone(), BLACK);
                            };
                            header(at(0, 0), "", true);
                            for (c, (_, name)) in col_items.iter().enumerate() {
                                header(at(0, c + 1), name, false);
                            }
                            for (ri, (_, rname)) in row_items.iter().enumerate() {
                                header(at(ri + 1, 0), rname, true);
                                for (ci, (_, cname)) in col_items.iter().enumerate() {
                                    let r = at(ri + 1, ci + 1);
                                    // The server's key order: items by category name.
                                    let mut pairs: Vec<(&str, &str)> = pages
                                        .iter()
                                        .map(|(c, i)| (c.as_str(), i.as_str()))
                                        .collect();
                                    if let Some(a) = &rows {
                                        pairs.push((&a.name, rname));
                                    }
                                    if let Some(a) = &cols {
                                        pairs.push((&a.name, cname));
                                    }
                                    pairs.sort();
                                    let key: Vec<String> =
                                        pairs.into_iter().map(|(_, i)| i.to_owned()).collect();
                                    let text =
                                        values.get(&key).map(|v| fmt(*v)).unwrap_or_default();
                                    if clip.contains_rect(r) {
                                        exposed.push(serde_json::json!({
                                            "measure": m.name, "key": key, "text": text,
                                            "x": r.center().x, "y": r.center().y,
                                        }));
                                    }
                                    if let Some((_, _, buf, select)) = editing
                                        .as_mut()
                                        .filter(|(id, k, ..)| *id == m.id && *k == key)
                                    {
                                        let id = egui::Id::new(("edit", m.id, &key));
                                        let resp = ui.put(
                                            r,
                                            egui::TextEdit::singleline(buf)
                                                .id(id)
                                                .horizontal_align(egui::Align::Max),
                                        );
                                        resp.request_focus();
                                        if std::mem::take(select) {
                                            let mut st = egui::TextEdit::load_state(ui.ctx(), id)
                                                .unwrap_or_default();
                                            let all = egui::text::CCursorRange::two(
                                                egui::text::CCursor::new(0),
                                                egui::text::CCursor::new(buf.chars().count()),
                                            );
                                            st.cursor.set_char_range(Some(all));
                                            st.store(ui.ctx(), id);
                                        }
                                        if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                            commit = Some((m.clone(), key.clone(), buf.clone()));
                                        } else if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                            *editing = None;
                                        }
                                        continue;
                                    }
                                    p.rect_filled(r, 0.0, WHITE);
                                    p.rect_stroke(
                                        r,
                                        0.0,
                                        Stroke::new(1.0_f32, Color32::from_gray(0xAA)),
                                    );
                                    p.text(
                                        r.right_center() - vec2(4.0, 0.0),
                                        Align2::RIGHT_CENTER,
                                        &text,
                                        font.clone(),
                                        BLACK,
                                    );
                                    let resp = ui.interact(
                                        r,
                                        egui::Id::new(("cell", m.id, &key)),
                                        Sense::click(),
                                    );
                                    if m.kind == "input" && resp.double_clicked() {
                                        *editing = Some((m.id, key.clone(), text.clone(), true));
                                    }
                                }
                            }
                        });
                });
        }
        if let Some((m, key, text)) = commit {
            self.editing = None;
            self.write(ctx, &m, &key, &text);
        }
        let title = format!("improv-web: {}", self.status);
        net::expose(&title, &serde_json::Value::Array(exposed).to_string());
    }
}
