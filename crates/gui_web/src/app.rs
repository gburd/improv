//! The web worksheet app: NeXT-look worksheet windows over a
//! [`ModelService`] (the remote one, in the browser). Everything shown
//! comes from the authority: the structure, each window's cells (derived
//! ones computed there), and the changes it pushes. Nothing is computed
//! here.

use std::collections::BTreeMap;

use egui::{vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind};
use improv_client::diagnostics::Diagnostic;
use improv_client::wire::{
    Cell, CellEdit, DefineReq, EditReq, Event, ModelService, NameReq, Request, Response, Structure,
    UpdateMeasureReq, ViewReq,
};
use improv_core_model::{Locale, Model, NumberFormat};

use crate::layout::{Axis, Cache, Sheet};
use crate::menu::{self, Command};
use crate::theme::{BLACK, DARK_GRAY, LIGHT_GRAY, WHITE};

const ROW_H: f32 = 20.0;
const COL_W: f32 = 90.0;
const HEAD_W: f32 = 100.0;
const TILE_H: f32 = 17.0;
/// Room the main menu takes at the workspace's left.
const MENU_CLEAR: f32 = menu::MENU_W + 12.0;

/// What a test reads off the page (`<body data-…>`): a canvas has no DOM,
/// so the app publishes where its cells, tiles, drop zones and formula
/// line are, and the formula error it shows.
#[derive(Default)]
pub struct Probe {
    pub cells: Vec<serde_json::Value>,
    pub tiles: Vec<serde_json::Value>,
    pub zones: Vec<serde_json::Value>,
    /// Page item fields, and the items of an open one's list.
    pub pages: Vec<serde_json::Value>,
    pub pageitems: Vec<serde_json::Value>,
    /// The model browser's measure links, and the button that opens it.
    pub links: Vec<serde_json::Value>,
    pub browserbutton: Option<serde_json::Value>,
    pub newformula: Option<serde_json::Value>,
    pub formulaerror: Option<serde_json::Value>,
    /// The change stream is connected.
    pub live: bool,
}

/// What a grid's frame asks of its window: a cell's edit to commit (an
/// empty coordinate: cancelled), an edit to start (its text, and whether
/// to select it), and where the cursor went (and whether it takes the
/// focus next frame).
struct GridOut<'a> {
    commit: &'a mut Option<(improv_client::wire::At, String)>,
    start_edit: &'a mut Option<(improv_client::wire::At, String, bool)>,
    cursor: &'a mut Option<(usize, usize)>,
    focus: &'a mut bool,
}

/// What a sent request was for, so its reply lands in the right place.
enum Pending {
    Structure,
    Window(u32, improv_client::wire::WindowReq),
    /// A change from the formula pane, a cell edit, a rename…: its status.
    Change(What),
}

#[derive(Clone)]
enum What {
    Cell(u32, improv_client::wire::At),
    Formula(Option<u32>),
    Other,
}

/// One worksheet window.
struct Win {
    uid: u64,
    sheet: Sheet,
    rect: Rect,
    open: bool,
    /// The cell being edited, its text, and whether to select it all.
    editing: Option<(improv_client::wire::At, String, bool)>,
    /// The keyboard cursor (row, column), and whether it takes the focus
    /// back next frame (an edit ended).
    cursor: (usize, usize),
    refocus: bool,
}

pub struct WebApp<S: ModelService> {
    svc: S,
    structure: Structure,
    /// The structure as a model without cells (for autocomplete and
    /// located errors).
    model: Model,
    cache: Cache,
    wins: Vec<Win>,
    next_uid: u64,
    pending: BTreeMap<u64, Pending>,
    status: String,
    menu: menu::MenuState,
    locale: Locale,
    /// Logged in: the token was accepted (or none was needed).
    login: Login,
    /// The formula pane: the selected measure's formula text and its error,
    /// and the new-formula line and its error.
    formula: (Option<u32>, String, Option<Diagnostic>),
    new_formula: (String, Option<Diagnostic>),
    /// A one-line prompt (rename, new item, save view) and what it is for.
    prompt: Option<(Prompt, String)>,
    current_view: Option<u32>,
    /// The key worksheet's uid: menus act on it, and only it shows the
    /// formula pane (one per app, as Improv's).
    key: Option<u64>,
    browser: bool,
    loaded: bool,
}

#[derive(Clone, Copy, PartialEq)]
enum Login {
    /// Asking for a token, with what went wrong last time.
    Asking,
    Trying,
    In,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Prompt {
    AddCategory,
    AddItem(u32),
    AddInput,
    RenameMeasure(u32),
    RenameCategory(u32),
    RenameItem(u32),
    SaveView,
}

impl<S: ModelService> WebApp<S> {
    pub fn new(ctx: &egui::Context, mut svc: S, token: Option<String>) -> Self {
        crate::theme::apply(ctx);
        let login = match token {
            Some(t) => {
                svc.authenticate(t);
                Login::Trying
            }
            None => Login::Asking,
        };
        let mut app = WebApp {
            svc,
            structure: Structure::default(),
            model: Model::new(),
            cache: Cache::default(),
            wins: Vec::new(),
            next_uid: 1,
            pending: BTreeMap::new(),
            status: String::new(),
            menu: menu::MenuState::default(),
            locale: Locale::en(),
            login,
            formula: (None, String::new(), None),
            new_formula: (String::new(), None),
            prompt: None,
            current_view: None,
            key: None,
            browser: false,
            loaded: false,
        };
        if login == Login::Trying {
            app.ask(Request::Structure, Pending::Structure);
        }
        app
    }

    fn ask(&mut self, req: Request, why: Pending) {
        let t = self.svc.send(req);
        self.pending.insert(t, why);
    }

    fn change(&mut self, req: Request, what: What) {
        self.ask(req, Pending::Change(what));
    }

    /// Take in what the service delivered.
    fn drain(&mut self, ctx: &egui::Context) {
        let events = self.svc.poll();
        if !events.is_empty() {
            // A reply can change a panel's size: let the layout settle.
            ctx.request_repaint();
        }
        for ev in events {
            match ev {
                Event::LoggedOut(e) => {
                    self.login = Login::Asking;
                    self.status = match e.contains("missing") {
                        true => "This server needs an access token.".into(),
                        false => format!("That token was refused ({e})."),
                    };
                    self.pending.clear();
                }
                Event::Changed(c) => {
                    if c.reload {
                        self.reload();
                    } else {
                        for x in c.cells {
                            self.cache.set(x.measure, x.at, x.value);
                        }
                    }
                }
                Event::Reply(t, r) => {
                    let Some(why) = self.pending.remove(&t) else {
                        continue;
                    };
                    self.reply(why, r);
                }
            }
        }
    }

    fn reply(&mut self, why: Pending, r: Result<Response, improv_client::wire::Failure>) {
        match (why, r) {
            (Pending::Structure, Ok(Response::Structure(s))) => {
                self.login = Login::In;
                self.adopt(s);
            }
            (Pending::Window(m, w), Ok(Response::Cells(c))) => {
                let cells = c.cells.into_iter().map(|x| (x.at, x.value)).collect();
                self.cache.fill(m, &w, cells);
            }
            (Pending::Change(what), Ok(Response::Done(d))) => {
                self.status = d.message;
                match what {
                    What::Formula(..) => {
                        self.new_formula = (String::new(), None);
                        self.formula.2 = None;
                    }
                    What::Cell(..) | What::Other => {}
                }
            }
            (Pending::Change(What::Formula(on)), Err(f)) => {
                self.status = format!("formula error: {}", f.error);
                let d = f
                    .diagnostic
                    .as_ref()
                    .map_or_else(|| Diagnostic::plain(f.error.clone()), Diagnostic::from);
                match on {
                    Some(_) => self.formula.2 = Some(d),
                    None => self.new_formula.1 = Some(d),
                }
            }
            (Pending::Change(What::Cell(m, at)), Err(f)) => {
                self.status = format!("not saved: {}", f.error);
                // Show the authority's value again.
                self.cache.set(m, at, None);
                self.reload_windows();
            }
            (_, Err(f)) => self.status = f.error,
            (_, Ok(_)) => {}
        }
    }

    /// A new structure: keep the windows whose measures are still there.
    fn adopt(&mut self, s: Structure) {
        self.model = s.to_model();
        self.structure = s;
        self.cache.clear();
        let st = &self.structure;
        self.wins.retain(|w| st.measure(w.sheet.measure).is_some());
        for w in &mut self.wins {
            let keep = w.sheet.clone();
            w.sheet = Sheet::new(st, keep.measure);
            for c in keep.order.iter().rev() {
                if let Some(a) = keep.axis_of(*c) {
                    w.sheet.move_to(*c, a);
                }
            }
            w.sheet.page = keep.page;
        }
        if !self.loaded {
            self.loaded = true;
            if let Some(v) = self.structure.views.first().cloned() {
                self.open_view(v.id.0);
            } else {
                let first: Vec<u32> = self
                    .structure
                    .measures
                    .iter()
                    .map(|m| m.id)
                    .take(3)
                    .collect();
                for m in first {
                    self.open_sheet(m);
                }
            }
        }
        if self.wins.is_empty() {
            if let Some(m) = self.structure.measures.first().map(|m| m.id) {
                self.open_sheet(m);
            }
        }
        self.status = "ready".into();
    }

    fn reload(&mut self) {
        self.ask(Request::Structure, Pending::Structure);
    }

    fn reload_windows(&mut self) {
        self.cache.clear();
    }

    fn open_sheet(&mut self, measure: u32) {
        let n = self.wins.len() as f32;
        let sheet = Sheet::new(&self.structure, measure);
        self.push_win(sheet, n);
    }

    fn push_win(&mut self, sheet: Sheet, n: f32) {
        // Wide enough for its columns, a cascade down and to the right of
        // the menu, so each title bar and its tiles stay in view.
        let cols = Sheet::tuples(&self.structure, sheet.cols()).len().max(1) as f32;
        let w = (HEAD_W + COL_W * cols + 40.0).clamp(320.0, 720.0);
        let at = egui::pos2(MENU_CLEAR + 40.0 * n, 8.0 + 200.0 * n);
        self.wins.push(Win {
            uid: self.next_uid,
            sheet,
            rect: Rect::from_min_size(at, vec2(w, 200.0)),
            open: true,
            editing: None,
            cursor: (0, 0),
            refocus: false,
        });
        self.next_uid += 1;
    }

    fn open_view(&mut self, id: u32) {
        let Some(v) = self.structure.views.iter().find(|v| v.id.0 == id).cloned() else {
            return;
        };
        self.wins.clear();
        for (i, p) in v.matrices().iter().enumerate() {
            let sheet = Sheet::from_placement(&self.structure, p);
            self.push_win(sheet, i as f32);
            if p.rect.w > 0.0 && p.rect.h > 0.0 {
                let w = self.wins.last_mut().expect("pushed");
                w.rect =
                    Rect::from_min_size(egui::pos2(p.rect.x, p.rect.y), vec2(p.rect.w, p.rect.h));
            }
        }
        self.current_view = Some(id);
        self.status = format!("view: {}", v.name.0);
    }

    /// Ask for each open window's cells it has not read yet.
    fn fetch_windows(&mut self) {
        let want: Vec<(u32, improv_client::wire::WindowReq)> = self
            .wins
            .iter()
            .filter(|w| w.open)
            .map(|w| (w.sheet.measure, w.sheet.window(&self.structure)))
            .collect();
        for (m, w) in want {
            let asked = self
                .pending
                .values()
                .any(|p| matches!(p, Pending::Window(x, y) if *x == m && *y == w));
            if !self.cache.has(m, &w) && !asked {
                self.ask(Request::Window(m, w.clone()), Pending::Window(m, w));
            }
        }
    }

    /// The key worksheet (the last one clicked), else the newest.
    fn focused(&self) -> Option<&Win> {
        let key = self.key;
        self.wins
            .iter()
            .find(|w| w.open && Some(w.uid) == key)
            .or_else(|| self.wins.iter().rev().find(|w| w.open))
    }

    // -- frame ------------------------------------------------------------

    pub fn frame(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        self.drain(ctx);
        if self.login != Login::In {
            self.login_panel(root);
            return;
        }
        self.fetch_windows();
        self.keys(ctx);
        let mut browser_button = None;
        egui::Panel::bottom("status")
            .frame(egui::Frame::NONE.fill(LIGHT_GRAY).inner_margin(4.0))
            .show(root, |ui| {
                ui.horizontal(|ui| {
                    // A screen reader hears it change (a polite live region).
                    let r = ui.label(&self.status);
                    ui.ctx().accesskit_node_builder(r.id, |n| {
                        n.set_role(egui::accesskit::Role::Status);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let r = ui.small_button("Model Browser");
                        if r.clicked() {
                            self.browser = !self.browser;
                        }
                        browser_button = Some(r.rect.center());
                        // Whether changes are arriving live.
                        ui.label(if self.svc.live() {
                            "live"
                        } else {
                            "connecting…"
                        });
                    });
                });
            });
        let ws = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(DARK_GRAY))
            .show(root, |ui| ui.max_rect())
            .inner;
        let mut probe = Probe {
            browserbutton: browser_button.map(|p| serde_json::json!({"x": p.x, "y": p.y})),
            ..Probe::default()
        };
        for i in 0..self.wins.len() {
            self.window(ctx, ws, i, &mut probe);
        }
        self.wins.retain(|w| w.open);
        if self.browser {
            self.model_browser(ctx, ws, &mut probe);
        }
        self.prompt_panel(ctx, ws);
        self.menu_bar(ctx, ws);
        probe.formulaerror = self
            .new_formula
            .1
            .as_ref()
            .or(self.formula.2.as_ref())
            .map(|d| serde_json::json!({"message": d.message, "span": d.span.as_ref().map(|r| [r.start, r.end]), "hint": d.hint}));
        probe.live = self.svc.live();
        crate::expose(&format!("improv-web: {}", self.status), &probe);
    }

    fn login_panel(&mut self, root: &mut egui::Ui) {
        let ctx = &root.ctx().clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(DARK_GRAY))
            .show(root, |ui| {
                let at = Rect::from_center_size(ui.max_rect().center(), vec2(320.0, 130.0));
                let id = egui::Id::unique("login_token");
                let mut token: String = ctx.data_mut(|d| d.get_temp(id)).unwrap_or_default();
                let mut go = false;
                let _ = crate::nxwin::NxWindow::new("login", "Improv - Log In")
                    .rect(at)
                    .resizable(false)
                    .show(ctx, |ui| {
                        crate::theme::content(ui);
                        let label = ui.label("The server's access token:");
                        let r = ui
                            .add(
                                egui::TextEdit::singleline(&mut token)
                                    .password(true)
                                    .id(egui::Id::unique("token_edit"))
                                    .desired_width(f32::INFINITY),
                            )
                            .labelled_by(label.id);
                        // Before asking for the focus back: a singleline edit
                        // gives it up on Enter.
                        let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                        go = enter && (r.lost_focus() || r.has_focus());
                        if self.login == Login::Asking && !r.has_focus() && !go {
                            r.request_focus();
                        }
                        ui.horizontal(|ui| {
                            go |= ui.button("Log In").clicked();
                            if self.login == Login::Trying {
                                ui.label("…");
                            }
                        });
                        if !self.status.is_empty() {
                            ui.label(&self.status);
                        }
                    });
                if go {
                    self.svc.authenticate(std::mem::take(&mut token));
                    self.login = Login::Trying;
                    self.status.clear();
                    self.reload();
                }
                ctx.data_mut(|d| d.insert_temp(id, token));
            });
        crate::expose("improv-web: log in", &Probe::default());
    }

    fn keys(&mut self, ctx: &egui::Context) {
        // A text edit has its own Ctrl+Z; a focused cell or tile does not.
        if ctx.text_edit_focused() {
            return;
        }
        let cmd = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                    ..
                } if modifiers.command => menu::key_command(*key, modifiers.shift),
                _ => None,
            })
        });
        if let Some(c) = cmd {
            self.run(c);
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context, ws: Rect) {
        let views: Vec<(String, bool)> = self
            .structure
            .views
            .iter()
            .map(|v| (v.name.0.clone(), Some(v.id.0) == self.current_view))
            .collect();
        let selected = self.focused().map(|w| w.sheet.measure);
        let numeric = selected
            .and_then(|m| self.structure.measure(m))
            .is_some_and(|m| m.value_type == improv_core_model::ValueType::Number);
        let format = selected
            .and_then(|m| self.structure.measure(m))
            .and_then(|m| m.format);
        let has_focus = selected.is_some();
        let mut state = self.menu;
        let picked = menu::show(
            ctx,
            &mut state,
            ws.min + vec2(4.0, 4.0),
            &views,
            |c| match c {
                Command::Undo | Command::Redo | Command::ModelBrowser | Command::AddCategory => {
                    true
                }
                Command::AddInputMeasure | Command::SaveView | Command::Keys => true,
                Command::OpenView(_) | Command::FunctionHelp | Command::UserGuide => true,
                Command::AddMatrix | Command::PivotRotate | Command::Rename | Command::AddItem => {
                    has_focus
                }
                Command::RemoveMatrix => self.wins.len() > 1,
                Command::Delete => has_focus,
                Command::Decimals(_) | Command::Grouping => numeric,
                _ => false,
            },
            |c| match c {
                Command::Decimals(d) => numeric && format.unwrap_or_default().decimals == d,
                Command::Grouping => format.is_some_and(|f| f.grouping),
                _ => false,
            },
        );
        self.menu = state;
        if let Some(c) = picked {
            self.run(c);
        }
    }

    fn run(&mut self, c: Command) {
        let focused = self.focused().map(|w| (w.uid, w.sheet.measure));
        match c {
            Command::Undo => self.change(Request::Undo, What::Other),
            Command::Redo => self.change(Request::Redo, What::Other),
            Command::ModelBrowser => self.browser = !self.browser,
            Command::AddCategory => self.prompt = Some((Prompt::AddCategory, String::new())),
            Command::AddInputMeasure => self.prompt = Some((Prompt::AddInput, String::new())),
            Command::SaveView => self.prompt = Some((Prompt::SaveView, String::new())),
            Command::AddItem => {
                if let Some(c) = self.focused().and_then(|w| w.sheet.rows().first().copied()) {
                    self.prompt = Some((Prompt::AddItem(c), String::new()));
                }
            }
            Command::Rename => {
                if let Some((_, m)) = focused {
                    let name = self
                        .structure
                        .measure(m)
                        .map(|x| x.name.clone())
                        .unwrap_or_default();
                    self.prompt = Some((Prompt::RenameMeasure(m), name));
                }
            }
            Command::Delete => {
                if let Some((_, m)) = focused {
                    self.change(Request::DeleteMeasure(m), What::Other);
                }
            }
            Command::AddMatrix => {
                if let Some((_, m)) = focused {
                    self.open_sheet(m);
                }
            }
            Command::RemoveMatrix => {
                if let Some(w) = self.wins.iter_mut().rev().find(|w| w.open) {
                    w.open = false;
                }
            }
            Command::PivotRotate => {
                if let Some(w) = self.wins.iter_mut().rev().find(|w| w.open) {
                    // Swap the row and column stacks.
                    let (r, k): (Vec<u32>, Vec<u32>) =
                        (w.sheet.rows().into(), w.sheet.cols().into());
                    for c in &r {
                        w.sheet.move_to(*c, Axis::Columns);
                    }
                    for c in &k {
                        w.sheet.move_to(*c, Axis::Rows);
                    }
                }
            }
            Command::OpenView(i) => {
                if let Some(id) = self.structure.views.get(i).map(|v| v.id.0) {
                    self.open_view(id);
                }
            }
            Command::Decimals(d) => self.format(|f| f.decimals = d),
            Command::Grouping => self.format(|f| f.grouping = !f.grouping),
            Command::UserGuide => {
                if let Some(w) = web_sys::window() {
                    let _ = w.open_with_url("https://gburd.github.io/improv/web-gui.html");
                }
            }
            _ => self.status = "not in the web GUI yet".into(),
        }
    }

    fn format(&mut self, f: impl FnOnce(&mut NumberFormat)) {
        let Some(m) = self.focused().map(|w| w.sheet.measure) else {
            return;
        };
        let mut fmt = self
            .structure
            .measure(m)
            .and_then(|x| x.format)
            .unwrap_or_default();
        f(&mut fmt);
        let fmt = (fmt != NumberFormat::default()).then_some(fmt);
        self.change(
            Request::UpdateMeasure(
                m,
                UpdateMeasureReq {
                    format: Some(fmt),
                    ..Default::default()
                },
            ),
            What::Other,
        );
    }

    // -- a worksheet window ------------------------------------------------

    fn window(&mut self, ctx: &egui::Context, ws: Rect, i: usize, probe: &mut Probe) {
        let (uid, rect, measure) = (
            self.wins[i].uid,
            self.wins[i].rect,
            self.wins[i].sheet.measure,
        );
        let Some(m) = self.structure.measure(measure).cloned() else {
            return;
        };
        let page_names: Vec<String> = self.wins[i]
            .sheet
            .pages()
            .iter()
            .filter_map(|c| {
                let item = self.wins[i].sheet.page_item(&self.structure, *c)?;
                self.structure
                    .category(*c)?
                    .items
                    .iter()
                    .find(|x| x.id == item)
                    .map(|x| x.name.clone())
            })
            .collect();
        let title = match page_names.is_empty() {
            true => format!("{} - Worksheet", m.name),
            false => format!("{}: {} - Worksheet", m.name, page_names.join(", ")),
        };
        let mut open = true;
        let mut moves: Vec<(u32, Axis)> = Vec::new();
        let mut page_pick: Option<(u32, u32)> = None;
        let mut commit: Option<(improv_client::wire::At, String)> = None;
        let mut start_edit: Option<(improv_client::wire::At, String, bool)> = None;
        let mut cursor = None;
        let mut focus_cursor = false;
        let mut formula_commit: Option<(u32, String)> = None;
        let mut new_commit: Option<String> = None;
        let id = crate::nxwin::NxWindow::new(("ws", uid), "").id();
        let is_key = self.focused().map(|w| w.uid) == Some(uid);
        let shown = crate::nxwin::NxWindow::new(("ws", uid), title)
            .rect(rect)
            .bounds(ws)
            .min_size(vec2(300.0, 160.0))
            .closable(&mut open)
            .show(ctx, |ui| {
                // The formula pane at the bottom, in the key window.
                if is_key {
                    egui::Panel::bottom(egui::Id::unique(("formulas", uid)))
                        .frame(egui::Frame::NONE.fill(WHITE).inner_margin(4.0))
                        .show(ui, |ui| {
                            crate::theme::content(ui);
                            self.formula_pane(
                                ui,
                                uid,
                                &m,
                                &mut formula_commit,
                                &mut new_commit,
                                probe,
                            );
                        });
                }
                self.gutters(ui, i, &mut moves, &mut page_pick, probe);
                let mut out = GridOut {
                    commit: &mut commit,
                    start_edit: &mut start_edit,
                    cursor: &mut cursor,
                    focus: &mut focus_cursor,
                };
                self.grid(ui, i, &m, probe, &mut out);
            });
        if shown.is_some() {
            if let Some(r) = crate::nxwin::stored_rect(ctx, id) {
                self.wins[i].rect = r;
            }
            if crate::nxwin::is_key(ctx, id) && self.key != Some(uid) {
                // The formula pane moves to this window: start it clean.
                self.key = Some(uid);
                self.formula = (None, String::new(), None);
                self.new_formula = (String::new(), None);
            }
        }
        let w = &mut self.wins[i];
        w.open = open;
        for (c, a) in moves {
            w.sheet.move_to(c, a);
        }
        if let Some((c, item)) = page_pick {
            w.sheet.page.insert(c, item);
        }
        if let Some(c) = cursor {
            w.cursor = c;
        }
        w.refocus = focus_cursor;
        if let Some((at, text, select)) = start_edit {
            w.editing = Some((at, text, select));
        }
        if let Some((at, text)) = commit {
            w.editing = None;
            // The cell has the keyboard again.
            w.refocus = true;
            self.commit_cell(&m, at, &text);
        }
        if let Some((id, text)) = formula_commit {
            self.change(
                Request::UpdateMeasure(
                    id,
                    UpdateMeasureReq {
                        formula: Some(text),
                        ..Default::default()
                    },
                ),
                What::Formula(Some(id)),
            );
        }
        if let Some(text) = new_commit {
            self.change(
                Request::Define(DefineReq {
                    definition: Some(text),
                    ..Default::default()
                }),
                What::Formula(None),
            );
        }
    }

    /// The row, column and page gutters: a tile per category, dragged
    /// between them to pivot; a page tile's item picked from a list.
    fn gutters(
        &self,
        ui: &mut egui::Ui,
        i: usize,
        moves: &mut Vec<(u32, Axis)>,
        pick: &mut Option<(u32, u32)>,
        probe: &mut Probe,
    ) {
        let mname = self
            .structure
            .measure(self.wins[i].sheet.measure)
            .map(|m| m.name.clone())
            .unwrap_or_default();
        let sheet = &self.wins[i].sheet;
        let uid = self.wins[i].uid;
        ui.horizontal(|ui| {
            for (axis, cats, label) in [
                (Axis::Pages, sheet.pages(), "Pages"),
                (Axis::Columns, sheet.cols(), "Columns"),
                (Axis::Rows, sheet.rows(), "Rows"),
            ] {
                let frame = egui::Frame::NONE
                    .fill(LIGHT_GRAY)
                    .stroke(Stroke::new(1.0, DARK_GRAY))
                    .inner_margin(2.0);
                let (zone, dropped) = ui.dnd_drop_zone::<u32, ()>(frame, |ui| {
                    ui.set_min_size(vec2(90.0, TILE_H));
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(label).color(DARK_GRAY).size(10.0));
                        for c in cats {
                            let r = self.tile(ui, uid, *c, axis, moves);
                            probe.tiles.push(serde_json::json!({
                                "measure": mname, "axis": format!("{axis:?}"),
                                "name": self.structure.category(*c).map(|k| k.name.clone()),
                                "x": r.center().x, "y": r.center().y,
                            }));
                            if axis == Axis::Pages {
                                let cur = sheet.page_item(&self.structure, *c);
                                let items = self
                                    .structure
                                    .category(*c)
                                    .map(|k| k.items.clone())
                                    .unwrap_or_default();
                                let name = items
                                    .iter()
                                    .find(|x| Some(x.id) == cur)
                                    .map(|x| x.name.clone())
                                    .unwrap_or_default();
                                let cat = self
                                    .structure
                                    .category(*c)
                                    .map(|k| k.name.clone())
                                    .unwrap_or_default();
                                let r = ui.name_inputs(format!("{cat} page"), |ui| {
                                    egui::ComboBox::from_id_salt(("page", uid, *c))
                                    .selected_text(name)
                                    .show_ui(ui, |ui| {
                                        for it in &items {
                                            let r = ui.selectable_label(Some(it.id) == cur, &it.name);
                                            probe.pageitems.push(serde_json::json!({
                                                "name": it.name, "x": r.rect.center().x, "y": r.rect.center().y,
                                            }));
                                            if r.clicked() {
                                                *pick = Some((*c, it.id));
                                            }
                                        }
                                    })
                                });
                                probe.pages.push(serde_json::json!({
                                    "measure": mname, "x": r.response.rect.center().x, "y": r.response.rect.center().y,
                                }));
                            }
                        }
                    });
                });
                probe.zones.push(serde_json::json!({
                    "measure": mname, "axis": format!("{axis:?}"),
                    "x": zone.response.rect.right() - 12.0, "y": zone.response.rect.center().y,
                }));
                if let Some(c) = dropped {
                    moves.push((*c, axis));
                }
            }
        });
    }

    fn tile(
        &self,
        ui: &mut egui::Ui,
        uid: u64,
        c: u32,
        from: Axis,
        moves: &mut Vec<(u32, Axis)>,
    ) -> Rect {
        let name = self
            .structure
            .category(c)
            .map_or_else(|| c.to_string(), |k| k.name.clone());
        let resp = ui
            .dnd_drag_source(egui::Id::unique(("tile", uid, c)), c, |ui| {
                egui::Frame::NONE
                    .fill(DARK_GRAY)
                    .stroke(Stroke::new(1.0, BLACK))
                    .inner_margin(egui::Margin::symmetric(6, 0))
                    .show(ui, |ui| {
                        ui.set_height(TILE_H - 2.0);
                        ui.add(
                            egui::Label::new(egui::RichText::new(&name).color(WHITE))
                                .selectable(false),
                        );
                    });
            })
            .response;
        // A secondary click opens the menu (the drag-free way to pivot).
        // Interacted after the drag source, it would sit on top and take
        // the drag, so only while a secondary press is under it.
        let secondary = ui.input(|i| i.pointer.secondary_down() || i.pointer.secondary_released());
        let resp = match secondary && resp.layer_id == ui.layer_id() {
            true => ui.interact(resp.rect, resp.id.with("menu"), Sense::click()) | resp,
            false => resp,
        };
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                true,
                format!("{name} tile, on {from:?}: Enter opens its menu"),
            )
        });
        let menu = |ui: &mut egui::Ui, moves: &mut Vec<(u32, Axis)>, focus: bool| {
            let mut first = focus;
            for to in [Axis::Rows, Axis::Columns, Axis::Pages] {
                if to == from {
                    continue;
                }
                let b = ui.button(format!("Move to {to:?}"));
                if std::mem::take(&mut first) {
                    b.request_focus();
                }
                if b.clicked() {
                    moves.push((c, to));
                    ui.close();
                }
            }
        };
        resp.context_menu(|ui| menu(ui, moves, false));
        // The same menu from the keyboard (Enter) or a screen reader's
        // activation, under the tile; closed, the tile has the focus back.
        let kb = resp.id.with("keys");
        let mut open = ui.data(|d| d.get_temp::<bool>(kb)).unwrap_or(false);
        // (`has_focus` outside `input`: both take egui's context lock.)
        let focused = resp.has_focus();
        let asked = ui.input(|i| {
            (focused && i.key_pressed(egui::Key::Enter))
                || i.has_accesskit_action_request(resp.id, egui::accesskit::Action::Click)
        });
        let opened = asked && !open;
        open |= asked;
        let was = open;
        egui::Popup::menu(&resp)
            .id(kb)
            .open_bool(&mut open)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| menu(ui, moves, opened));
        if was && !open {
            resp.request_focus();
        }
        ui.data_mut(|d| d.insert_temp(kb, open));
        resp.rect
    }

    /// The grid: column headers, row headers, and the cells on this page,
    /// marked up for a screen reader as a grid (rows of row and column
    /// headers and cells, with their indices). One cell, the cursor, is a
    /// Tab stop: the arrows, Home and End move it, Enter or typing edits
    /// it, Enter commits and Esc cancels.
    fn grid(
        &self,
        ui: &mut egui::Ui,
        i: usize,
        m: &improv_client::wire::MeasureOut,
        probe: &mut Probe,
        out: &mut GridOut,
    ) {
        let w = &self.wins[i];
        let st = &self.structure;
        let rows = Sheet::tuples(st, w.sheet.rows());
        let cols = Sheet::tuples(st, w.sheet.cols());
        let name = |c: u32, it: u32| {
            st.category(c)
                .and_then(|k| k.items.iter().find(|x| x.id == it))
                .map_or_else(String::new, |x| x.name.clone())
        };
        let label = |t: &[(u32, u32)]| {
            t.iter()
                .map(|(c, i)| name(*c, *i))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let size = vec2(
            HEAD_W + COL_W * cols.len() as f32,
            ROW_H * (rows.len() + 1) as f32,
        );
        let font = FontId::proportional(12.0);
        let cursor = (
            w.cursor.0.min(rows.len().saturating_sub(1)),
            w.cursor.1.min(cols.len().saturating_sub(1)),
        );
        // No taller than what the formula pane leaves (egui's default
        // minimum would run it into the pane).
        egui::ScrollArea::both()
            .id_salt(("grid", w.uid))
            .auto_shrink(false)
            .min_scrolled_height(0.0)
            .max_height(ui.available_height().max(0.0))
            .show(ui, |ui| {
                crate::theme::content(ui);
                let (area, _) = ui.allocate_exact_size(size, Sense::hover());
                // The grid's accessibility nodes: a Ui per grid and row
                // (their nodes hold the headers' and cells'); nothing drawn.
                let mut grid = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt(("axgrid", w.uid))
                        .max_rect(area)
                        .accessibility_role(egui::accesskit::Role::Grid)
                        .accessibility_label(&m.name),
                );
                grid.ctx().accesskit_node_builder(grid.unique_id(), |n| {
                    n.set_row_count(rows.len() + 1);
                    n.set_column_count(cols.len() + 1);
                });
                let row_ui = |grid: &mut egui::Ui, r: usize| {
                    let rui = grid.new_child(
                        egui::UiBuilder::new()
                            .id_salt(("axrow", r))
                            .max_rect(area)
                            .accessibility_role(egui::accesskit::Role::Row),
                    );
                    rui.ctx()
                        .accesskit_node_builder(rui.unique_id(), |n| n.set_row_index(r));
                    rui
                };
                // A header: hover-only, named, at its column.
                let ax_head = |ui: &egui::Ui, r: Rect, id: egui::Id, role, name: &str, col| {
                    let h = ui.interact(r, id, Sense::hover());
                    ui.ctx().accesskit_node_builder(h.id, |n| {
                        n.set_role(role);
                        n.set_label(name);
                        n.set_column_index(col);
                    });
                };
                let clip = ui.clip_rect();
                let p = ui.painter().clone();
                let cell = |r: usize, c: usize| {
                    let x = if c == 0 {
                        0.0
                    } else {
                        HEAD_W + COL_W * (c - 1) as f32
                    };
                    let wd = if c == 0 { HEAD_W } else { COL_W };
                    Rect::from_min_size(area.min + vec2(x, ROW_H * r as f32), vec2(wd, ROW_H))
                };
                let head = |r: Rect, t: &str, left: bool| {
                    p.rect_filled(r, 0.0, LIGHT_GRAY);
                    p.rect_stroke(r, 0.0, Stroke::new(1.0, BLACK), StrokeKind::Inside);
                    let (pos, al) = match left {
                        true => (r.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER),
                        false => (r.center(), Align2::CENTER_CENTER),
                    };
                    p.text(pos, al, t, font.clone(), BLACK);
                };
                head(cell(0, 0), "", true);
                let col_names: Vec<String> = cols.iter().map(|c| label(c)).collect();
                let top = row_ui(&mut grid, 0);
                let ch = egui::accesskit::Role::ColumnHeader;
                ax_head(&top, cell(0, 0), top.unique_id().with(0), ch, "", 0);
                for (ci, col) in col_names.iter().enumerate() {
                    head(cell(0, ci + 1), col, false);
                    ax_head(
                        &top,
                        cell(0, ci + 1),
                        top.unique_id().with(ci + 1),
                        ch,
                        col,
                        ci + 1,
                    );
                }
                drop(top);
                for (ri, row) in rows.iter().enumerate() {
                    let row_name = label(row);
                    head(cell(ri + 1, 0), &row_name, true);
                    // Rows scrolled off have no nodes (the grid's row count
                    // says how many there are), except the cursor's.
                    if !clip.intersects(cell(ri + 1, 0).union(cell(ri + 1, cols.len())))
                        && ri != cursor.0
                    {
                        continue;
                    }
                    let mut ui = row_ui(&mut grid, ri + 1);
                    let rh = egui::accesskit::Role::RowHeader;
                    ax_head(
                        &ui,
                        cell(ri + 1, 0),
                        ui.unique_id().with(0),
                        rh,
                        &row_name,
                        0,
                    );
                    for (ci, col) in cols.iter().enumerate() {
                        let r = cell(ri + 1, ci + 1);
                        let mine = (ri, ci) == cursor;
                        // The cursor cell stays, scrolled off or not: it is
                        // the grid's Tab stop.
                        if !clip.intersects(r) && !mine {
                            continue;
                        }
                        let at = w.sheet.at(st, row, col);
                        let (text, typed) = match self.cache.get(m.id, &at) {
                            Cell::Value(v) => (v.shown(m.format.as_ref(), &self.locale), v.typed()),
                            Cell::Blank => (String::new(), String::new()),
                            Cell::NotLoaded => ("…".into(), String::new()),
                        };
                        probe.cells.push(serde_json::json!({
                            "measure": m.name, "at": at, "text": text,
                            "x": r.center().x, "y": r.center().y,
                        }));
                        if let Some((_, seed, select)) =
                            w.editing.as_ref().filter(|(a, ..)| *a == at)
                        {
                            let id = egui::Id::unique(("edit", w.uid));
                            // The text lives in egui's memory while the
                            // editor is open; `seed` (the old value) only
                            // starts it.
                            let key = id.with(("buf", &at));
                            let mut buf: String = ui
                                .ctx()
                                .data_mut(|d| d.get_temp(key))
                                .unwrap_or_else(|| seed.clone());
                            let resp =
                                ui.put(
                                    r,
                                    egui::TextEdit::singleline(&mut buf)
                                        .id(id)
                                        .align(egui::Align2::RIGHT_CENTER),
                                )
                                .accessible_name(
                                    crate::layout::cell_name(&row_name, &col_names[ci], "editing"),
                                );
                            if !resp.has_focus() {
                                resp.request_focus();
                            }
                            if *select
                                && ui
                                    .ctx()
                                    .data(|d| d.get_temp::<bool>(key.with("sel")))
                                    .is_none()
                            {
                                ui.ctx().data_mut(|d| d.insert_temp(key.with("sel"), true));
                                let mut s =
                                    egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
                                let all = egui::text::CCursorRange::two(
                                    egui::text::CCursor::new(0),
                                    egui::text::CCursor::new(buf.chars().count()),
                                );
                                s.cursor.set_char_range(Some(all));
                                s.store(ui.ctx(), id);
                            }
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                            if enter || esc {
                                ui.ctx().data_mut(|d| {
                                    d.remove::<String>(key);
                                    d.remove::<bool>(key.with("sel"));
                                });
                                *out.commit = Some(if enter {
                                    (at.clone(), buf)
                                } else {
                                    (Vec::new(), String::new())
                                });
                            } else {
                                ui.ctx().data_mut(|d| d.insert_temp(key, buf));
                            }
                            continue;
                        }
                        p.rect_filled(r, 0.0, WHITE);
                        p.rect_stroke(
                            r,
                            0.0,
                            Stroke::new(1.0, Color32::from_gray(0xAA)),
                            StrokeKind::Inside,
                        );
                        p.text(
                            r.right_center() - vec2(4.0, 0.0),
                            Align2::RIGHT_CENTER,
                            &text,
                            font.clone(),
                            BLACK,
                        );
                        // Only the cursor cell is a Tab stop; a screen
                        // reader still reads every cell in the grid.
                        let sense = match mine {
                            true => Sense::click(),
                            false => Sense::CLICK,
                        };
                        let resp = ui.interact(r, egui::Id::unique(("cell", w.uid, &at)), sense);
                        let name = crate::layout::cell_name(&row_name, &col_names[ci], &text);
                        resp.widget_info(|| {
                            egui::WidgetInfo::labeled(egui::Role::GridCell, true, &name)
                        });
                        ui.ctx().accesskit_node_builder(resp.id, |n| {
                            n.set_column_index(ci + 1);
                            // Every cell takes a screen reader's focus (the
                            // request moves the cursor there).
                            n.add_action(egui::accesskit::Action::Focus);
                            if !m.editable {
                                n.set_read_only();
                            }
                        });
                        let (sr_click, sr_focus) = ui.input(|i| {
                            use egui::accesskit::Action;
                            (
                                i.has_accesskit_action_request(resp.id, Action::Click),
                                i.has_accesskit_action_request(resp.id, Action::Focus),
                            )
                        });
                        if resp.clicked() || resp.has_focus() {
                            *out.cursor = Some((ri, ci));
                        }
                        if sr_focus && !mine {
                            *out.cursor = Some((ri, ci));
                            *out.focus = true;
                        }
                        if mine && w.refocus {
                            resp.request_focus();
                            resp.scroll_to_me(None);
                        }
                        if resp.has_focus() {
                            if resp.gained_focus() {
                                // Another frame, so the cell holds the
                                // arrows (its lock filter) before the next key.
                                ui.ctx().request_repaint();
                            }
                            // The cursor shows where the keyboard is.
                            p.rect_stroke(r, 0.0, Stroke::new(2.0, BLACK), StrokeKind::Inside);
                            let cell_id = |r: usize, c: usize| {
                                let at = w.sheet.at(st, &rows[r], &cols[c]);
                                egui::Id::unique(("cell", w.uid, &at))
                            };
                            self.cell_keys(
                                &ui,
                                &resp,
                                (ri, ci),
                                (rows.len(), cols.len()),
                                cell_id,
                                out,
                            );
                            if m.editable {
                                // Enter edits the value; typing replaces it.
                                let (enter, typed_text) = ui.input(|i| {
                                    let t: String = i
                                        .events
                                        .iter()
                                        .filter_map(|e| match e {
                                            egui::Event::Text(t) => Some(t.as_str()),
                                            _ => None,
                                        })
                                        .collect();
                                    (i.key_pressed(egui::Key::Enter), t)
                                });
                                if enter {
                                    *out.start_edit = Some((at.clone(), typed.clone(), true));
                                } else if !typed_text.trim().is_empty() {
                                    *out.start_edit = Some((at.clone(), typed_text, false));
                                }
                            }
                        }
                        if m.editable && (resp.double_clicked() || sr_click) {
                            *out.start_edit = Some((at, typed, true));
                        }
                    }
                }
            });
    }

    /// Move the cursor with the arrows, Home and End.
    fn cell_keys(
        &self,
        ui: &egui::Ui,
        resp: &egui::Response,
        at: (usize, usize),
        size: (usize, usize),
        cell_id: impl Fn(usize, usize) -> egui::Id,
        out: &mut GridOut,
    ) {
        use crate::layout::Step;
        // The cell keeps the arrows (egui would move the focus by them).
        ui.memory_mut(|m| {
            m.set_focus_lock_filter(
                resp.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
        let keys = [
            (egui::Key::ArrowUp, Step::Up),
            (egui::Key::ArrowDown, Step::Down),
            (egui::Key::ArrowLeft, Step::Left),
            (egui::Key::ArrowRight, Step::Right),
            (egui::Key::Home, Step::Home),
            (egui::Key::End, Step::End),
        ];
        let pressed = ui.input(|i| {
            keys.iter()
                .find(|(k, _)| i.key_pressed(*k))
                .map(|(_, s)| (*s, i.modifiers.command))
        });
        if let Some(to) = pressed
            .and_then(|(k, ctrl)| crate::layout::step(at, k, ctrl, size.0, size.1))
            .filter(|to| *to != at)
        {
            *out.cursor = Some(to);
            // Straight to the new cell, so no frame has no focus (a screen
            // reader would announce the canvas); next frame scrolls to it.
            ui.memory_mut(|m| m.request_focus(cell_id(to.0, to.1)));
            *out.focus = true;
        }
    }

    fn commit_cell(
        &mut self,
        m: &improv_client::wire::MeasureOut,
        at: improv_client::wire::At,
        text: &str,
    ) {
        if at.is_empty() {
            return; // Esc
        }
        let text = text.trim();
        let value = if text.is_empty() {
            None
        } else {
            match improv_client::wire::parse_typed(&self.model, &self.locale, m.value_type, text) {
                Ok(v) => Some(v),
                Err(e) => {
                    self.status = e;
                    return;
                }
            }
        };
        // Shown at once; the authority's answer (and its recalculation)
        // follows on the change stream.
        self.cache.set(m.id, at.clone(), value.clone());
        self.change(
            Request::Edit(EditReq {
                edits: vec![CellEdit {
                    measure: m.id,
                    at: at.clone(),
                    value,
                }],
            }),
            What::Cell(m.id, at),
        );
    }

    /// The formula pane: every formula, the selected one editable with
    /// autocomplete and its error located, and a line for a new one.
    fn formula_pane(
        &mut self,
        ui: &mut egui::Ui,
        uid: u64,
        m: &improv_client::wire::MeasureOut,
        commit: &mut Option<(u32, String)>,
        new: &mut Option<String>,
        probe: &mut Probe,
    ) {
        let derived: Vec<improv_client::wire::MeasureOut> = self
            .structure
            .measures
            .iter()
            .filter(|x| x.kind == "derived")
            .cloned()
            .collect();
        // shortcut: every formula, unscrolled; a model with dozens of
        // formulas wants the desktop's scrolled list.
        ui.vertical(|ui| {
            crate::theme::content(ui);
            for (n, x) in derived.iter().enumerate() {
                let mine = self.formula.0 == Some(x.id);
                let text = x.formula.clone().unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.label(format!("{}.", n + 1));
                    if mine {
                        let id = egui::Id::unique(("formula_edit", uid));
                        let span = self.formula.2.as_ref().and_then(|d| d.span.clone());
                        ui.label(format!("{} =", x.name));
                        let resp = crate::edit::assisted(
                            ui,
                            &self.model,
                            &mut self.formula.1,
                            id,
                            "",
                            span,
                        )
                        .accessible_name(format!("Formula for {}", x.name));
                        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            *commit = Some((x.id, self.formula.1.clone()));
                        }
                    } else {
                        let r = ui.selectable_label(x.id == m.id, format!("{} = {text}", x.name));
                        if r.clicked() {
                            self.formula = (Some(x.id), text.clone(), None);
                        }
                    }
                });
                if mine {
                    if let Some(d) = &self.formula.2 {
                        crate::edit::error_lines(ui, d);
                    }
                }
                if let Some(e) = &x.error {
                    ui.colored_label(BLACK, format!("⚠ {e}"));
                }
            }
        });
        let id = egui::Id::unique(("formula_new", uid));
        let span = self.new_formula.1.as_ref().and_then(|d| d.span.clone());
        let resp = crate::edit::assisted(
            ui,
            &self.model,
            &mut self.new_formula.0,
            id,
            "Name = formula",
            span,
        );
        if probe.newformula.is_none() {
            probe.newformula =
                Some(serde_json::json!({"x": resp.rect.center().x, "y": resp.rect.center().y}));
        }
        if resp.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && !self.new_formula.0.trim().is_empty()
        {
            *new = Some(self.new_formula.0.clone());
        }
        if let Some(d) = &self.new_formula.1 {
            crate::edit::error_lines(ui, d);
        }
    }

    fn model_browser(&mut self, ctx: &egui::Context, ws: Rect, probe: &mut Probe) {
        let mut probe_links = Vec::new();
        let mut open = true;
        let mut act: Option<Request> = None;
        let mut open_measure = None;
        let mut prompt = None;
        let at = Rect::from_min_size(
            egui::pos2(ws.max.x - 260.0, ws.min.y + 8.0),
            vec2(250.0, 320.0),
        );
        let _ = crate::nxwin::NxWindow::new("browser", "Model Browser")
            .rect(at)
            .bounds(ws)
            .closable(&mut open)
            .show(ctx, |ui| {
                crate::theme::content(ui);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.strong("Measures");
                    for m in &self.structure.measures {
                        ui.horizontal(|ui| {
                            let r = ui.link(&m.name);
                            probe_links.push(serde_json::json!({
                                "name": m.name, "x": r.rect.center().x, "y": r.rect.center().y,
                            }));
                            if r.clicked() {
                                open_measure = Some(m.id);
                            }
                            let b = ui
                                .small_button("rename")
                                .accessible_name(format!("Rename {}", m.name));
                            if b.clicked() {
                                prompt = Some((Prompt::RenameMeasure(m.id), m.name.clone()));
                            }
                        });
                    }
                    ui.strong("Categories");
                    for c in &self.structure.categories {
                        ui.horizontal(|ui| {
                            ui.label(&c.name);
                            let b = ui
                                .small_button("rename")
                                .accessible_name(format!("Rename {}", c.name));
                            if b.clicked() {
                                prompt = Some((Prompt::RenameCategory(c.id), c.name.clone()));
                            }
                            let b = ui
                                .small_button("+ item")
                                .accessible_name(format!("Add an item to {}", c.name));
                            if b.clicked() {
                                prompt = Some((Prompt::AddItem(c.id), String::new()));
                            }
                            let b = ui
                                .small_button("delete")
                                .accessible_name(format!("Delete {}", c.name));
                            if b.clicked() {
                                act = Some(Request::DeleteCategory(c.id));
                            }
                        });
                        for it in &c.items {
                            ui.horizontal(|ui| {
                                ui.add_space(16.0);
                                ui.label(&it.name);
                                let b = ui
                                    .small_button("rename")
                                    .accessible_name(format!("Rename {}", it.name));
                                if b.clicked() {
                                    prompt = Some((Prompt::RenameItem(it.id), it.name.clone()));
                                }
                                let b = ui
                                    .small_button("delete")
                                    .accessible_name(format!("Delete {}", it.name));
                                if b.clicked() {
                                    act = Some(Request::DeleteItem(it.id));
                                }
                            });
                        }
                    }
                });
            });
        self.browser = open;
        probe.links = probe_links;
        if let Some(m) = open_measure {
            self.open_sheet(m);
        }
        if let Some(r) = act {
            self.change(r, What::Other);
        }
        if prompt.is_some() {
            self.prompt = prompt;
        }
    }

    fn prompt_panel(&mut self, ctx: &egui::Context, ws: Rect) {
        let Some((what, mut text)) = self.prompt.clone() else {
            return;
        };
        let title = match what {
            Prompt::AddCategory => "New Category",
            Prompt::AddItem(_) => "New Item",
            Prompt::AddInput => "New Input Measure",
            Prompt::RenameMeasure(_) | Prompt::RenameCategory(_) | Prompt::RenameItem(_) => {
                "Rename"
            }
            Prompt::SaveView => "Save View",
        };
        let at = Rect::from_center_size(ws.center(), vec2(300.0, 90.0));
        let (mut go, mut cancel) = (false, false);
        let _ = crate::nxwin::NxWindow::new("prompt", title)
            .rect(at)
            .resizable(false)
            .show(ctx, |ui| {
                crate::theme::content(ui);
                let r = ui
                    .add(
                        egui::TextEdit::singleline(&mut text)
                            .id(egui::Id::unique("prompt_edit"))
                            .desired_width(f32::INFINITY),
                    )
                    .accessible_name(title);
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                go = enter && (r.lost_focus() || r.has_focus());
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                cancel = ui.input(|i| i.key_pressed(egui::Key::Escape));
                ui.horizontal(|ui| {
                    go |= ui.button("OK").clicked();
                    cancel |= ui.button("Cancel").clicked();
                });
            });
        if cancel {
            self.prompt = None;
            return;
        }
        if !go {
            self.prompt = Some((what, text));
            return;
        }
        self.prompt = None;
        let name = NameReq { name: text.clone() };
        let req = match what {
            Prompt::AddCategory => Request::AddCategory(name),
            Prompt::AddItem(c) => Request::AddItem(c, name),
            Prompt::AddInput => Request::Define(DefineReq {
                name: Some(text),
                ..Default::default()
            }),
            Prompt::RenameMeasure(m) => Request::UpdateMeasure(
                m,
                UpdateMeasureReq {
                    name: Some(text),
                    ..Default::default()
                },
            ),
            Prompt::RenameCategory(c) => Request::RenameCategory(c, name),
            Prompt::RenameItem(i) => Request::RenameItem(i, name),
            Prompt::SaveView => Request::SaveView(ViewReq {
                name: Some(text),
                matrices: Some(
                    self.wins
                        .iter()
                        .map(|w| {
                            let mut p = w.sheet.to_placement();
                            p.rect = improv_core_model::CanvasRect {
                                x: w.rect.min.x,
                                y: w.rect.min.y,
                                w: w.rect.width(),
                                h: w.rect.height(),
                            };
                            p
                        })
                        .collect(),
                ),
            }),
        };
        self.change(req, What::Other);
    }
}
