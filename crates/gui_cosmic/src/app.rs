//! The window: a COSMIC application over one model. It holds a copy of the
//! model for drawing and talks to the model service (a thread) for
//! everything else, so nothing it does waits on a load, an engine build,
//! a recalculation or a save.

use std::sync::{mpsc, Arc};

use cosmic::app::{Core, Task};
use cosmic::iced::keyboard::{self, key::Named, Key, Modifiers};
use cosmic::iced::widget::canvas;
use cosmic::iced::{Length, Size, Subscription};
use cosmic::widget;
use cosmic::{executor, ApplicationExt, Element};

use improv_core_model::{Locale, MeasureId, Model};
use improv_engine::session::Snapshot;

use crate::nav::{self, Extent, Move};
use crate::pivot::{self, Axis, Grid, Layout};
use crate::service::{self, CellEdit, LocalService, ModelService, Request, Update};
use crate::sheet::{self, Cell, Sheet, SheetProgram};

/// The formula editor's state.
#[derive(Debug, Clone, PartialEq)]
pub struct FormulaEdit {
    pub measure: MeasureId,
    pub text: String,
    /// Why the last commit was refused, located in `text`.
    pub error: Option<improv_client::diagnostics::Diagnostic>,
}

/// The formula editor's widget id.
static FORMULA: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("improv-formula-editor"));

/// The cell editor's widget id (to focus it when it opens).
static EDITOR: std::sync::LazyLock<cosmic::widget::Id> =
    std::sync::LazyLock::new(|| cosmic::widget::Id::new("improv-cell-editor"));

/// The reverse-DNS application id: the desktop entry, the icon and the
/// Wayland app id all use it.
pub const APP_ID: &str = "me.burd.Improv";

/// What `improv-cosmic` starts with.
pub struct Flags {
    /// The store: a file, `postgres://…?model=`, or `""` for in memory.
    pub target: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    /// The model service said something.
    Service(Update),
    /// A measure was picked in the measure list.
    ShowMeasure(usize),
    Sheet(sheet::Event),
    /// A key the focused widgets did not take, and the text it typed.
    Key(Key, Modifiers, Option<String>),
    /// The cell editor's text changed.
    EditText(String),
    /// Enter in the cell editor: commit it.
    EditCommit,
    /// A formula in the list was picked to edit.
    EditFormula(usize),
    /// The formula editor's text changed.
    FormulaText(String),
    /// Enter in the formula editor: commit it.
    FormulaCommit,
    /// Tab in the formula editor, or a click on a candidate: accept it.
    Complete(usize),
    /// A category tile was picked up (the pivot gesture starts).
    Grab(improv_core_model::CategoryId),
    /// The pointer is over a tile strip, at a tile position in it.
    Hover(Axis, usize),
    /// The button was released over a strip: drop a held tile where it
    /// hovers.
    Drop,
    /// The button was released anywhere: a held tile not dropped is let go.
    Release,
    /// Page `i` shows item `j` of its category.
    Page(usize, usize),
    /// Swap rows and columns.
    Transpose,
    /// Saved view `i` (in id order) was picked: show it.
    OpenView(usize),
    /// The new view's name, as typed.
    ViewName(String),
    /// Save the current layout as a view with the typed name.
    SaveView,
    /// Show the grid as a chart, or back.
    ToggleChart,
    /// Capture the window to [`SHOT_ENV`]'s file, then quit.
    Shoot,
    Shot(cosmic::iced::window::Screenshot),
}

/// Set to a `.png` path, the window captures itself there once the model
/// has loaded and quits: the screenshot check, and a capture on desktops
/// with no screenshot portal.
pub const SHOT_ENV: &str = "IMPROV_COSMIC_SHOT";

/// The window state.
pub struct App {
    core: Core,
    target: String,
    /// The service's request channel: dropping it ends the service.
    service: mpsc::Sender<Request>,
    /// The cell being edited and its text, while the editor is open.
    editing: Option<(Cell, String)>,
    /// The name typed for a view to save.
    view_name: String,
    /// The measure shows as a chart instead of a grid.
    chart: bool,
    /// The category tile being dragged, and the strip and slot it is over.
    held: Option<(improv_core_model::CategoryId, Option<(Axis, usize)>)>,
    /// The formula being edited: its measure, its text, and where the last
    /// commit of it failed (located for the underline).
    formula: Option<FormulaEdit>,
    /// Edits sent to the service and not yet answered (the status line says
    /// so; the grid shows a cell's new value when the service has it).
    pending: usize,
    /// The service's update stream, until the subscription takes it.
    updates: UpdateRx,
    model: Arc<Model>,
    snapshot: Snapshot,
    /// Measures in id order (the measure list).
    measures: Vec<MeasureId>,
    layout: Option<Layout>,
    grid: Option<Grid>,
    origin: Cell,
    cursor: Cell,
    /// Where a Shift+move or Shift+click range started.
    anchor: Option<Cell>,
    /// The grid canvas's size, as it last reported it.
    size: Size,
    locale: Locale,
    status: String,
}

impl App {
    /// A window over `service`, not yet started.
    pub fn with_service(core: Core, target: String, service: Box<dyn ModelService>) -> App {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        App {
            core,
            target,
            service: service::spawn(service, tx),
            editing: None,
            view_name: String::new(),
            chart: false,
            held: None,
            formula: None,
            pending: 0,
            updates: UpdateRx(Arc::new(std::sync::Mutex::new(Some(rx)))),
            model: Arc::new(Model::new()),
            snapshot: Snapshot::default(),
            measures: Vec::new(),
            layout: None,
            grid: None,
            origin: Cell::default(),
            cursor: Cell::default(),
            anchor: None,
            size: Size::ZERO,
            locale: Locale::from_env(),
            status: "Loading…".into(),
        }
    }

    /// Show `measure` in its natural layout.
    fn show(&mut self, measure: MeasureId) {
        self.layout = Layout::natural(&self.model, measure);
        self.relayout();
    }

    fn relayout(&mut self) {
        self.grid = self.layout.as_ref().map(|l| Grid::new(&self.model, l));
        self.origin = Cell::default();
        self.cursor = Cell::default();
        self.anchor = None;
    }

    /// The grid's and the view's extent.
    fn extent(&self) -> Extent {
        let Some(g) = &self.grid else {
            return Extent::default();
        };
        let (view_rows, view_cols) = sheet::whole(self.size, g.rows.cats.len(), g.cols.cats.len());
        Extent {
            rows: g.rows.len(),
            cols: g.cols.len(),
            view_rows,
            view_cols,
        }
    }

    /// Move the cursor (extending the selection with `extend`), and scroll
    /// the view after it.
    pub fn go(&mut self, m: Move, extend: bool) {
        let e = self.extent();
        if extend {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = nav::step(self.cursor, m, e);
        self.origin = nav::follow(self.origin, self.cursor, e);
    }

    /// The selected range, inclusive.
    pub fn selection(&self) -> (Cell, Cell) {
        nav::range(self.anchor, self.cursor)
    }

    /// What a key does in the grid, if anything.
    fn key(&mut self, key: &Key, mods: Modifiers, text: Option<&str>) -> Task<Message> {
        if self.editing.is_some() || self.formula.is_some() {
            // An editor has the keyboard (Escape: [`Self::on_escape`]).
            return Task::none();
        }
        match key {
            Key::Named(Named::Enter | Named::F2) => return self.begin_edit(None),
            Key::Named(Named::Delete | Named::Backspace) => {
                self.editing = Some((self.cursor, String::new()));
                self.commit_edit();
                return Task::none();
            }
            _ => {}
        }
        // A printable key starts an edit holding it (not with Ctrl or Alt:
        // those are shortcuts).
        if let Some(t) = text.filter(|t| !t.is_empty() && t.chars().all(|c| !c.is_control())) {
            if !mods.control() && !mods.alt() && !mods.logo() {
                return self.begin_edit(Some(t.to_string()));
            }
        }
        let m = match key {
            Key::Named(Named::ArrowUp) => Move::Up,
            Key::Named(Named::ArrowDown) => Move::Down,
            Key::Named(Named::ArrowLeft) => Move::Left,
            Key::Named(Named::ArrowRight) | Key::Named(Named::Tab) => Move::Right,
            Key::Named(Named::PageUp) => Move::PageUp,
            Key::Named(Named::PageDown) => Move::PageDown,
            Key::Named(Named::Home) if mods.control() => Move::First,
            Key::Named(Named::End) if mods.control() => Move::Last,
            Key::Named(Named::Home) => Move::Home,
            Key::Named(Named::End) => Move::End,
            _ => return Task::none(),
        };
        self.go(m, mods.shift());
        Task::none()
    }

    /// Apply what the service said.
    pub fn apply(&mut self, u: Update) {
        match u {
            Update::Opened(model, snapshot) => {
                self.model = model;
                self.snapshot = snapshot;
                self.measures = self.model.measures.keys().copied().collect();
                self.measures.sort_by_key(|m| m.0);
                self.status = format!("{} measures", self.measures.len());
                if let Some(m) = self.measures.first().copied() {
                    self.show(m);
                }
            }
            Update::Edited(cells, changes, unsaved) => {
                self.pending = self.pending.saturating_sub(1);
                let model = Arc::make_mut(&mut self.model);
                for c in cells.iter() {
                    let at = improv_engine::decode_coord(&c.at);
                    match &c.value {
                        Some(v) => model.set_input(c.measure, at, v.clone()),
                        None => drop(model.inputs.remove(&(c.measure, at))),
                    }
                }
                service::apply_changes(&mut self.snapshot, &changes);
                self.status = match unsaved {
                    None => "saved".into(),
                    Some(e) => e,
                };
            }
            Update::Restructured(structure, snapshot, unsaved) => {
                self.pending = self.pending.saturating_sub(1);
                let model = Arc::make_mut(&mut self.model);
                let inputs = std::mem::take(&mut model.inputs);
                *model = (*structure).clone();
                model.inputs = inputs;
                self.snapshot = snapshot;
                self.formula = None;
                self.status = unsaved.unwrap_or_else(|| "recalculated".into());
            }
            Update::Failed(e) => {
                self.pending = self.pending.saturating_sub(1);
                // A refused formula is located in its text, and stays open.
                if let Some(f) = &mut self.formula {
                    f.error = Some(crate::formulas::build_error(&self.model, &f.text, &e));
                }
                self.status = e;
            }
        }
    }

    /// Pivot: move `cat` onto `axis` at slot `at`. The layout changes, not
    /// the formulas; a move that would empty rows or columns while a page
    /// could fill them is refused, and says so.
    pub fn pivot(&mut self, cat: improv_core_model::CategoryId, axis: Axis, at: usize) {
        let Some(l) = &mut self.layout else {
            return;
        };
        if l.place(&self.model, cat, axis, at) {
            self.relayout_keeping_layout();
            self.status.clear();
        } else {
            self.status = "rows and columns each need a category while one is on the pages".into();
        }
    }

    /// The grid again for the current layout (after a pivot or a page).
    fn relayout_keeping_layout(&mut self) {
        self.grid = self.layout.as_ref().map(|l| Grid::new(&self.model, l));
        let e = self.extent();
        self.cursor = Cell {
            row: self.cursor.row.min(e.rows.saturating_sub(1)),
            col: self.cursor.col.min(e.cols.saturating_sub(1)),
        };
        self.origin = nav::follow(Cell::default(), self.cursor, e);
        self.anchor = None;
    }

    /// Save the layout as a view named as typed: a new view, or the one by
    /// that name replaced. A structural change, so the service saves it.
    pub fn save_view(&mut self) {
        let name = self.view_name.trim().to_string();
        let Some(l) = &self.layout else {
            return;
        };
        if name.is_empty() {
            self.status = "name the view first".into();
            return;
        }
        let id = self
            .model
            .views
            .values()
            .find(|v| v.name.0 == name)
            .map_or_else(|| pivot::next_view_id(&self.model), |v| v.id);
        let mut s = crate::formulas::structure(&self.model);
        s.add_view(l.to_view(id, &name));
        self.pending += 1;
        self.status = format!("saving view '{name}'…");
        if self
            .service
            .send(Request::Restructure(Box::new(s)))
            .is_err()
        {
            self.pending -= 1;
            self.status = "the model thread stopped: the view is not saved".into();
        } else {
            self.view_name.clear();
        }
    }

    /// Open the formula editor on row `i` of the formula list.
    pub fn edit_formula(&mut self, i: usize) -> Task<Message> {
        let Some(row) = crate::formulas::rows(&self.model, &self.locale)
            .into_iter()
            .nth(i)
        else {
            return Task::none();
        };
        self.editing = None;
        self.formula = Some(FormulaEdit {
            measure: row.measure,
            text: row.text,
            error: None,
        });
        widget::text_input::focus(FORMULA.clone())
    }

    /// The completions for the formula being typed (its caret at the end).
    ///
    /// shortcut: the caret is taken to be at the end (libcosmic's text
    /// input does not report it); completing mid-text needs a caret query.
    pub fn completions(&self) -> Vec<improv_client::completion::Candidate> {
        match &self.formula {
            Some(f) => improv_client::completion::offer(&self.model, &f.text, f.text.len()),
            None => Vec::new(),
        }
    }

    /// Accept completion `i` at the end of the formula's text.
    fn accept(&mut self, i: usize) {
        let cands = self.completions();
        if let (Some(c), Some(f)) = (cands.get(i), &mut self.formula) {
            let (text, _) = improv_client::completion::accept(&f.text, f.text.len(), c);
            f.text = text;
        }
    }

    /// Commit the formula editor: parse, then ask the service to rebuild
    /// with it. A parse error stays in the editor, located; a build error
    /// comes back from the service ([`Update::Failed`]) and is located too.
    pub fn commit_formula(&mut self) {
        let Some(f) = &mut self.formula else {
            return;
        };
        let parsed = crate::formulas::parse(&self.model, &self.locale, &f.text)
            .map_err(|d| (d.message.clone(), d))
            .and_then(|formula| {
                crate::formulas::with_formula(&self.model, f.measure, formula)
                    .map_err(|e| (e.clone(), improv_client::diagnostics::Diagnostic::plain(e)))
            });
        match parsed {
            Ok(structure) => {
                f.error = None;
                self.pending += 1;
                self.status = "recalculating…".into();
                if self
                    .service
                    .send(Request::Restructure(Box::new(structure)))
                    .is_err()
                {
                    self.pending -= 1;
                    self.status = "the model thread stopped: this formula is not applied".into();
                }
            }
            Err((msg, d)) => {
                f.error = Some(d);
                self.status = msg;
            }
        }
    }

    /// The cell under the cursor, if it is an input cell of the shown
    /// measure (derived cells are computed, not entered): its key and type.
    fn entry_cell(&self) -> Result<(MeasureId, improv_engine::CoordKey), String> {
        let (Some(l), Some(g)) = (&self.layout, &self.grid) else {
            return Err("no measure is shown".into());
        };
        let m = self
            .model
            .measures
            .get(&l.measure)
            .ok_or("no measure is shown")?;
        if m.is_derived() {
            return Err(format!(
                "{} is computed by its formula, not entered",
                m.name.0
            ));
        }
        let k = g
            .key(self.cursor.row, self.cursor.col)
            .ok_or("no cell here")?;
        Ok((l.measure, k))
    }

    /// Open the editor on the cursor's cell, holding `text` (what was
    /// typed) or, with `None`, the cell's value.
    pub fn begin_edit(&mut self, text: Option<String>) -> Task<Message> {
        let (measure, k) = match self.entry_cell() {
            Ok(c) => c,
            Err(e) => {
                self.status = e;
                return Task::none();
            }
        };
        let text = text.unwrap_or_else(|| {
            sheet::value(&self.model, &self.snapshot, measure, &k)
                .map(|v| v.to_string())
                .unwrap_or_default()
        });
        self.anchor = None;
        self.editing = Some((self.cursor, text));
        cosmic::widget::text_input::focus(EDITOR.clone())
    }

    /// Commit the editor: parse its text by the measure's type and send the
    /// edit. A text that does not parse keeps the editor open and says why.
    pub fn commit_edit(&mut self) {
        let Some((at, text)) = self.editing.take() else {
            return;
        };
        let cursor = std::mem::replace(&mut self.cursor, at);
        let cell = self.entry_cell();
        self.cursor = cursor;
        let (measure, k) = match cell {
            Ok(c) => c,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let ty = self.model.measures[&measure].value_type;
        match crate::entry::parse(&self.model, &self.locale, ty, &text) {
            Ok(value) => {
                self.pending += 1;
                self.status = "saving…".into();
                let edit = CellEdit {
                    measure,
                    at: k,
                    value,
                };
                if self.service.send(Request::Edit(vec![edit])).is_err() {
                    self.pending -= 1;
                    self.status = "the model thread stopped: this edit is not applied".into();
                }
            }
            Err(e) => {
                self.status = e;
                self.editing = Some((at, text));
            }
        }
    }

    /// The task that follows an update: a capture once the model is shown,
    /// when one was asked for.
    fn after(&self, opened: bool) -> Task<Message> {
        if opened && std::env::var_os(SHOT_ENV).is_some() {
            // Let a frame draw first.
            return cosmic::task::future(async {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                Message::Shoot
            });
        }
        Task::none()
    }

    fn title(&self) -> String {
        let name = if self.target.is_empty() {
            "Untitled".to_string()
        } else {
            std::path::Path::new(&self.target)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.target.clone())
        };
        format!("{name} — Improv")
    }

    fn measure_name(&self, m: MeasureId) -> String {
        self.model
            .measures
            .get(&m)
            .map(|x| x.name.0.clone())
            .unwrap_or_default()
    }
}

/// The service's updates as a stream the runtime polls (the subscription
/// runs once, so it takes the receiver).
fn updates(rx: &UpdateRx) -> impl futures::Stream<Item = Message> {
    use futures::StreamExt as _;
    let rx = rx.0.lock().ok().and_then(|mut r| r.take());
    futures::stream::iter(rx).flatten().map(Message::Service)
}

/// The receiver, shared with the subscription (which runs once).
#[derive(Clone)]
struct UpdateRx(Arc<std::sync::Mutex<Option<futures::channel::mpsc::UnboundedReceiver<Update>>>>);

impl std::hash::Hash for UpdateRx {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        "improv-cosmic-updates".hash(state);
    }
}

impl cosmic::Application for App {
    type Executor = executor::Default;
    type Flags = Flags;
    type Message = Message;
    const APP_ID: &'static str = APP_ID;

    fn core(&self) -> &Core {
        &self.core
    }

    fn core_mut(&mut self) -> &mut Core {
        &mut self.core
    }

    fn init(core: Core, flags: Flags) -> (Self, Task<Message>) {
        let service = Box::new(LocalService::new(flags.target.clone()));
        let mut app = App::with_service(core, flags.target, service);
        let title = app.title();
        app.set_header_title(title.clone());
        let task = match app.core.main_window_id() {
            Some(id) => app.set_window_title(title, id),
            None => Task::none(),
        };
        (app, task)
    }

    fn subscription(&self) -> Subscription<Message> {
        Subscription::batch([
            Subscription::run_with(self.updates.clone(), updates),
            // A button released anywhere ends a tile drag (a strip's own
            // release has dropped it there first).
            cosmic::iced::event::listen_with(|e, _, _| match e {
                cosmic::iced::Event::Mouse(cosmic::iced::mouse::Event::ButtonReleased(
                    cosmic::iced::mouse::Button::Left,
                )) => Some(Message::Release),
                _ => None,
            }),
            // Keys no widget took (a focused text field keeps its own).
            keyboard::listen().filter_map(|e| match e {
                keyboard::Event::KeyPressed {
                    key,
                    modifiers,
                    text,
                    ..
                } => Some(Message::Key(key, modifiers, text.map(|t| t.to_string()))),
                _ => None,
            }),
        ])
    }

    /// Escape (libcosmic routes it here, focused editor or not): close the
    /// cell editor unsaved, else drop the range.
    fn on_escape(&mut self) -> Task<Message> {
        if self.editing.take().is_some() || self.formula.take().is_some() {
            self.status.clear();
        } else {
            self.anchor = None;
        }
        Task::none()
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Service(u) => {
                let opened = matches!(u, Update::Opened(..));
                self.apply(u);
                return self.after(opened);
            }
            Message::Shoot => {
                if let Some(id) = self.core.main_window_id() {
                    return cosmic::iced::window::screenshot(id).map(|s| Message::Shot(s).into());
                }
            }
            Message::Shot(shot) => {
                if let Some(path) = std::env::var_os(SHOT_ENV) {
                    if let Err(e) = crate::shot::write_png(&shot, std::path::Path::new(&path)) {
                        eprintln!("improv-cosmic: {e}");
                    }
                }
                return cosmic::iced::exit();
            }
            Message::ShowMeasure(i) => {
                if let Some(m) = self.measures.get(i).copied() {
                    self.show(m);
                }
            }
            Message::Sheet(sheet::Event::Scrolled(to)) => self.origin = to,
            Message::Sheet(sheet::Event::Clicked(c)) => {
                if self.editing.is_some() {
                    self.commit_edit();
                }
                self.cursor = c;
                self.anchor = None;
            }
            Message::Sheet(sheet::Event::Activated(c)) => {
                self.cursor = c;
                return self.begin_edit(None);
            }
            Message::EditText(t) => {
                if let Some((_, text)) = &mut self.editing {
                    *text = t;
                }
            }
            Message::EditFormula(i) => return self.edit_formula(i),
            Message::Grab(c) => self.held = Some((c, None)),
            Message::Hover(axis, at) => {
                if let Some((_, over)) = &mut self.held {
                    *over = Some((axis, at));
                }
            }
            Message::Drop => {
                if let Some((c, Some((axis, at)))) = self.held.take() {
                    self.pivot(c, axis, at);
                }
                self.held = None;
            }
            Message::Release => self.held = None,
            Message::OpenView(i) => {
                let id = pivot::views(&self.model).get(i).map(|(id, _)| *id);
                match id
                    .and_then(|id| self.model.views.get(&id))
                    .and_then(|v| Layout::from_view(&self.model, v))
                {
                    Some(l) => {
                        self.layout = Some(l);
                        self.relayout_keeping_layout();
                        self.cursor = Cell::default();
                        self.origin = Cell::default();
                    }
                    None => self.status = "that view's measure no longer exists".into(),
                }
            }
            Message::ViewName(n) => self.view_name = n,
            Message::SaveView => self.save_view(),
            Message::ToggleChart => self.chart = !self.chart,
            Message::Page(i, j) => {
                if let Some(l) = &mut self.layout {
                    let cat = l.page_cats().get(i).copied();
                    let item = cat.and_then(|c| pivot::items(&self.model, c).get(j).copied());
                    if let Some(item) = item {
                        l.set_page(&self.model, i, item);
                        self.relayout_keeping_layout();
                    }
                }
            }
            Message::Transpose => {
                if let Some(l) = &mut self.layout {
                    l.transpose();
                    self.relayout_keeping_layout();
                }
            }
            Message::FormulaText(t) => {
                if let Some(f) = &mut self.formula {
                    f.text = t;
                    f.error = None;
                }
            }
            Message::FormulaCommit => {
                // Enter with a list showing accepts the first candidate.
                if !self.completions().is_empty() {
                    self.accept(0);
                } else {
                    self.commit_formula();
                }
            }
            Message::Complete(i) => {
                self.accept(i);
                return widget::text_input::focus(FORMULA.clone());
            }
            Message::EditCommit => {
                self.commit_edit();
                if self.editing.is_none() {
                    self.go(Move::Down, false);
                }
            }
            Message::Sheet(sheet::Event::Extended(c)) => {
                self.anchor.get_or_insert(self.cursor);
                self.cursor = c;
            }
            Message::Sheet(sheet::Event::Resized(size)) => self.size = size,
            Message::Key(key, mods, text) => return self.key(&key, mods, text.as_deref()),
        }
        Task::none()
    }

    fn view(&self) -> Element<'_, Message> {
        let names: Vec<String> = self
            .measures
            .iter()
            .map(|m| self.measure_name(*m))
            .collect();
        let shown = self
            .layout
            .as_ref()
            .and_then(|l| self.measures.iter().position(|m| *m == l.measure));
        let views: Vec<String> = pivot::views(&self.model)
            .into_iter()
            .map(|(_, n)| n)
            .collect();
        let bar = widget::row::with_capacity(6)
            .push(widget::dropdown(names, shown, Message::ShowMeasure))
            .push(widget::dropdown(views, None, Message::OpenView).placeholder("Views"))
            .push(
                widget::text_input::text_input("New view", self.view_name.as_str())
                    .on_input(Message::ViewName)
                    .on_submit(|_| Message::SaveView)
                    .width(Length::Fixed(160.0)),
            )
            .push(widget::button::standard("Save view").on_press(Message::SaveView))
            .push(
                widget::button::standard(if self.chart { "Grid" } else { "Chart" })
                    .on_press(Message::ToggleChart),
            )
            .push(widget::space::horizontal())
            .push(widget::text::caption(self.status.clone()))
            .spacing(cosmic::theme::spacing().space_s)
            .align_y(cosmic::iced::Alignment::Center);
        let body: Element<'_, Message> = match (&self.layout, &self.grid) {
            (Some(l), Some(g)) if self.chart => {
                let data = crate::chart::data(&self.model, &self.snapshot, l.measure, g);
                let note = data.truncated.then(|| {
                    format!(
                        "the first {} rows and {} columns",
                        crate::chart::MAX_GROUPS,
                        crate::chart::MAX_SERIES
                    )
                });
                let mut col = widget::column::with_capacity(2);
                if let Some(n) = note {
                    col = col.push(widget::text::caption(n));
                }
                col.push(
                    widget::canvas(ChartView(data))
                        .width(Length::Fill)
                        .height(Length::Fill),
                )
                .into()
            }
            (Some(l), Some(g)) => {
                let grid = widget::canvas(SheetProgram {
                    sheet: Sheet {
                        model: &self.model,
                        snapshot: &self.snapshot,
                        measure: l.measure,
                        grid: g,
                        format: self.model.measures.get(&l.measure).and_then(|m| m.format),
                        locale: &self.locale,
                        origin: self.origin,
                        cursor: self.cursor,
                        selection: self.selection(),
                        size: self.size,
                        editing: self.editing.as_ref().map(|(c, _)| *c),
                    },
                    on_event: Message::Sheet,
                })
                .width(Length::Fill)
                .height(Length::Fill);
                match &self.editing {
                    // The editor sits on its cell, while the cell is in view.
                    Some((at, text)) => match sheet::cell_rect(g, self.origin, *at, self.size) {
                        Some(r) => cosmic::iced::widget::stack![
                            grid,
                            cosmic::iced::widget::pin(
                                widget::text_input::text_input("", text.as_str())
                                    .id(EDITOR.clone())
                                    .on_input(Message::EditText)
                                    .on_submit(|_| Message::EditCommit)
                                    .width(Length::Fixed(r.width))
                            )
                            .position(r.position())
                        ]
                        .into(),
                        None => grid.into(),
                    },
                    None => grid.into(),
                }
            }
            _ => widget::container(widget::text::body(self.status.clone()))
                .center(Length::Fill)
                .into(),
        };
        widget::column::with_capacity(4)
            .push(bar)
            .push(self.tiles())
            .push(body)
            .push(self.formula_pane())
            .spacing(cosmic::theme::spacing().space_s)
            .padding(cosmic::theme::spacing().space_s)
            .into()
    }
}

impl App {
    /// The category tiles, in three strips: Rows, Columns and Pages. Press
    /// a tile and release it over a strip to move it there (before the tile
    /// under the pointer, or at the end): Improv's pivot. A page tile picks
    /// the item its page shows.
    fn tiles(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing();
        let Some(l) = &self.layout else {
            return widget::space::horizontal().into();
        };
        let held = self.held.map(|(c, _)| c);
        let over = self.held.and_then(|(_, o)| o);
        let strip = |axis: Axis, title: &'static str, cats: &[improv_core_model::CategoryId]| {
            let mut r = widget::row::with_capacity(cats.len() + 2)
                .spacing(space.space_xxs)
                .align_y(cosmic::iced::Alignment::Center)
                .push(widget::text::caption(title));
            for (i, c) in cats.iter().enumerate() {
                let name = pivot::category_name(&self.model, *c);
                let tile: Element<'_, Message> = if axis == Axis::Pages {
                    let items = pivot::items(&self.model, *c);
                    let names: Vec<String> = items
                        .iter()
                        .map(|it| pivot::item_name(&self.model, *it))
                        .collect();
                    let sel = l
                        .page_items
                        .get(i)
                        .and_then(|p| items.iter().position(|it| it == p));
                    widget::row::with_capacity(2)
                        .spacing(space.space_xxs)
                        .align_y(cosmic::iced::Alignment::Center)
                        .push(tile_button(name, held == Some(*c), Message::Grab(*c)))
                        .push(widget::dropdown(names, sel, move |j| Message::Page(i, j)))
                        .into()
                } else {
                    tile_button(name, held == Some(*c), Message::Grab(*c))
                };
                let marker = over == Some((axis, i));
                r = r.push(
                    widget::mouse_area(widget::container(tile).padding(if marker {
                        [0, 0, 0, 4]
                    } else {
                        [0, 0, 0, 0]
                    }))
                    .on_enter(Message::Hover(axis, i))
                    .on_release(Message::Drop),
                );
            }
            // The rest of the strip: drop at its end.
            r = r.push(
                widget::mouse_area(
                    widget::container(widget::space::horizontal())
                        .height(Length::Fixed(32.0))
                        .width(Length::Fill),
                )
                .on_enter(Message::Hover(axis, cats.len()))
                .on_release(Message::Drop),
            );
            widget::container(r)
                .padding(space.space_xxs)
                .class(cosmic::theme::Container::Card)
                .width(Length::Fill)
        };
        widget::column::with_capacity(2)
            .spacing(space.space_xxs)
            .push(
                widget::row::with_capacity(3)
                    .spacing(space.space_xxs)
                    .push(strip(Axis::Rows, "Rows", l.row_cats()))
                    .push(strip(Axis::Cols, "Columns", l.col_cats()))
                    .push(widget::button::standard("Swap").on_press(Message::Transpose)),
            )
            .push(strip(Axis::Pages, "Pages", l.page_cats()))
            .into()
    }

    /// The formula list under the grid, Improv's numbered `Name = formula`
    /// rows: a click opens a row in the editor; a refused formula shows its
    /// error (the span, the message and the hint) under it, and the names
    /// that complete the word at its end show as buttons (Enter takes the
    /// first).
    fn formula_pane(&self) -> Element<'_, Message> {
        let space = cosmic::theme::spacing();
        let mut list = widget::column::with_capacity(4).spacing(space.space_xxs);
        for (i, row) in crate::formulas::rows(&self.model, &self.locale)
            .into_iter()
            .enumerate()
        {
            let label = format!("{}. {} =", i + 1, row.target);
            let line: Element<'_, Message> = match &self.formula {
                Some(f) if f.measure == row.measure => {
                    let mut col = widget::column::with_capacity(4).spacing(space.space_xxs);
                    col = col.push(
                        widget::row::with_capacity(2)
                            .push(widget::text::body(label))
                            .push(
                                widget::text_input::text_input("", f.text.as_str())
                                    .id(FORMULA.clone())
                                    .on_input(Message::FormulaText)
                                    .on_submit(|_| Message::FormulaCommit)
                                    .on_tab(Message::Complete(0)),
                            )
                            .spacing(space.space_xs)
                            .align_y(cosmic::iced::Alignment::Center),
                    );
                    if let Some(d) = &f.error {
                        col = col.push(error_line(&f.text, d));
                    }
                    let cands = self.completions();
                    if !cands.is_empty() {
                        let mut r =
                            widget::row::with_capacity(cands.len()).spacing(space.space_xxs);
                        for (j, c) in cands.iter().enumerate() {
                            r = r.push(
                                widget::button::text(format!("{}  {}", c.hint(), c.doc()))
                                    .on_press(Message::Complete(j)),
                            );
                        }
                        col = col.push(r);
                    }
                    col.into()
                }
                _ => widget::button::custom(
                    widget::row::with_capacity(2)
                        .push(widget::text::body(label))
                        .push(widget::text::monotext(row.text))
                        .spacing(space.space_xs),
                )
                .class(cosmic::theme::Button::Text)
                .on_press(Message::EditFormula(i))
                .into(),
            };
            list = list.push(line);
        }
        widget::container(list).padding(space.space_xs).into()
    }
}

/// A chart that owns its data (the canvas is built each view).
struct ChartView(crate::chart::Data);

impl canvas::Program<Message, cosmic::Theme> for ChartView {
    type State = ();

    fn draw(
        &self,
        state: &(),
        renderer: &cosmic::Renderer,
        theme: &cosmic::Theme,
        bounds: cosmic::iced::Rectangle,
        cursor: cosmic::iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        <crate::chart::Chart<'_> as canvas::Program<Message, cosmic::Theme>>::draw(
            &crate::chart::Chart { data: &self.0 },
            state,
            renderer,
            theme,
            bounds,
            cursor,
        )
    }
}

/// A category tile: a pill in the button colors, the accent while it is
/// held. Not a button: a button takes the press, and the pivot needs it.
fn tile_button<'a>(name: String, held: bool, grab: Message) -> Element<'a, Message> {
    let pill = widget::container(widget::text::body(name))
        .padding([4, 12])
        .class(cosmic::theme::Container::custom(
            move |t: &cosmic::Theme| {
                let c = t.cosmic();
                let (bg, fg) = if held {
                    (c.accent_color(), c.on_accent_color())
                } else {
                    (c.button_bg_color(), c.on_bg_color())
                };
                cosmic::iced::widget::container::Style {
                    background: Some(cosmic::iced::Color::from(bg).into()),
                    text_color: Some(fg.into()),
                    border: cosmic::iced::Border {
                        radius: c.corner_radii.radius_xl.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }
            },
        ));
    widget::mouse_area(pill)
        .on_press(grab)
        .interaction(cosmic::iced::mouse::Interaction::Grab)
        .into()
}

/// A formula error: the text with the span marked under it (`^^^`), the
/// message, and the hint.
fn error_line<'a>(text: &str, d: &improv_client::diagnostics::Diagnostic) -> Element<'a, Message> {
    let mut col = widget::column::with_capacity(3);
    if let Some(r) = &d.span {
        col = col.push(widget::text::monotext(crate::formulas::caret_line(
            text,
            r.clone(),
        )));
    }
    col = col.push(widget::text::body(d.message.clone()).class(cosmic::theme::Text::Accent));
    if let Some(h) = &d.hint {
        col = col.push(widget::text::caption(h.clone()));
    }
    col.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::tests::revenue_model;
    use improv_core_model::{CategoryId, ItemId, Measure, MeasureKind, Name, ValueType};

    /// A window over `model`, opened as the service would open it, with a
    /// view of `rows` × `cols` whole cells.
    pub(crate) fn opened(model: Model, rows: usize, cols: usize) -> App {
        let mut app = App::with_service(Core::default(), String::new(), Box::new(Idle));
        app.apply(Update::Opened(Arc::new(model), Snapshot::default()));
        let (cats_r, cats_c) = app.grid.as_ref().map_or((1, 1), |g| {
            (g.rows.cats.len().max(1), g.cols.cats.len().max(1))
        });
        app.size = Size::new(
            sheet::HEAD_W * cats_r as f32 + sheet::COL_W * cols as f32,
            sheet::ROW_H * cats_c as f32 + sheet::ROW_H * rows as f32,
        );
        app
    }

    /// A service that never answers: tests feed the window updates.
    struct Idle;

    impl ModelService for Idle {
        fn open(&mut self) -> Result<(Model, Snapshot), String> {
            Err("idle".into())
        }
        fn edit(&mut self, _: Vec<CellEdit>) -> Result<service::Edited, String> {
            Err("idle".into())
        }
        fn restructure(&mut self, _: Model) -> Result<(Snapshot, Option<String>), String> {
            Err("idle".into())
        }
    }

    /// One input measure over A × B, `n` items each.
    fn square(n: u32) -> Model {
        let mut m = Model::new();
        let (a, b) = (CategoryId(1), CategoryId(2));
        m.add_category(a, "A");
        m.add_category(b, "B");
        for i in 0..n {
            m.add_item(ItemId(10_000 + i), a, format!("a{i}"));
            m.add_item(ItemId(20_000 + i), b, format!("b{i}"));
        }
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("V".into()),
            value_type: ValueType::Number,
            categories: vec![a, b],
            kind: MeasureKind::Input,
            description: None,
            format: None,
        });
        m
    }

    #[test]
    fn opening_shows_the_first_measure() {
        let app = opened(revenue_model(), 10, 5);
        assert_eq!(app.layout.as_ref().map(|l| l.measure), Some(MeasureId(100)));
        assert_eq!(app.status, "3 measures");
        assert_eq!(app.title(), "Untitled — Improv");
    }

    #[test]
    fn keys_move_the_cursor_and_the_view_follows_over_a_million_cells() {
        // 1000 × 1000 = 10^6 cells.
        let mut app = opened(square(1000), 20, 5);
        let ctrl = Modifiers::CTRL;
        let _ = app.key(&Key::Named(Named::End), ctrl, None);
        assert_eq!(app.cursor, Cell { row: 999, col: 999 });
        assert_eq!(app.origin, Cell { row: 980, col: 995 });
        let _ = app.key(&Key::Named(Named::Home), ctrl, None);
        assert_eq!((app.cursor, app.origin), (Cell::default(), Cell::default()));
        let _ = app.key(&Key::Named(Named::PageDown), Modifiers::empty(), None);
        assert_eq!(app.cursor.row, 20);
        assert_eq!(app.origin.row, 1);
        // An unbound key does nothing.
        let _ = app.key(&Key::Named(Named::Insert), Modifiers::empty(), None);
        assert_eq!(app.cursor.row, 20);
    }

    #[test]
    fn shift_extends_a_range_and_a_plain_move_drops_it() {
        let mut app = opened(square(10), 20, 5);
        let _ = app.key(&Key::Named(Named::ArrowDown), Modifiers::SHIFT, None);
        let _ = app.key(&Key::Named(Named::ArrowRight), Modifiers::SHIFT, None);
        assert_eq!(app.selection(), (Cell::default(), Cell { row: 1, col: 1 }));
        let _ = cosmic::Application::update(
            &mut app,
            Message::Sheet(sheet::Event::Extended(Cell { row: 3, col: 2 })),
        );
        // The anchor stays where Shift started.
        assert_eq!(app.selection(), (Cell::default(), Cell { row: 3, col: 2 }));
        let _ = app.key(&Key::Named(Named::ArrowUp), Modifiers::empty(), None);
        assert_eq!(
            app.selection(),
            (Cell { row: 2, col: 2 }, Cell { row: 2, col: 2 })
        );
    }

    /// A window over `model` whose service is a real one, in memory, on its
    /// thread: tests send edits and wait for the answer.
    fn live(model: Model) -> (App, futures::channel::mpsc::UnboundedReceiver<Update>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let svc = service::tests::memory(model);
        let mut app = App::with_service(Core::default(), String::new(), Box::new(Idle));
        app.service = service::spawn(Box::new(svc), tx);
        let opened = next(&mut rx);
        app.apply(opened);
        app.size = Size::new(800.0, 600.0);
        (app, rx)
    }

    fn next(rx: &mut futures::channel::mpsc::UnboundedReceiver<Update>) -> Update {
        futures::executor::block_on(futures::StreamExt::next(rx)).expect("an update")
    }

    fn typed(app: &mut App, s: &str) {
        let _ = app.key(&Key::Character(s.into()), Modifiers::empty(), Some(s));
    }

    #[test]
    fn typing_on_a_cell_edits_it_and_the_derived_cells_follow() {
        let (mut app, mut rx) = live(revenue_model());
        // Quantity (100) at Time=2025 (row 0), Product=WidgetA (col 0).
        typed(&mut app, "7");
        assert_eq!(app.editing, Some((Cell::default(), "7".into())));
        let _ = cosmic::Application::update(&mut app, Message::EditText("75".into()));
        let _ = cosmic::Application::update(&mut app, Message::EditCommit);
        assert!(app.editing.is_none());
        assert_eq!(app.cursor, Cell { row: 1, col: 0 }, "Enter moves down");
        assert_eq!(app.pending, 1);
        let u = next(&mut rx);
        app.apply(u);
        assert_eq!(app.pending, 0);
        assert_eq!(app.status, "saved");
        let k: improv_engine::CoordKey = [(1, 10), (2, 20)].into_iter().collect();
        let rev = sheet::value(&app.model, &app.snapshot, MeasureId(102), &k);
        assert_eq!(
            rev,
            Some(improv_engine::CellValue::num(750.0)),
            "Revenue = 10 x 75"
        );
        let q = sheet::value(&app.model, &app.snapshot, MeasureId(100), &k);
        assert_eq!(q, Some(improv_engine::CellValue::num(75.0)));
    }

    #[test]
    fn a_bad_entry_keeps_the_editor_and_says_why() {
        let (mut app, _rx) = live(revenue_model());
        typed(&mut app, "x");
        let _ = cosmic::Application::update(&mut app, Message::EditCommit);
        assert!(app.status.contains("not a number"), "{}", app.status);
        assert_eq!(app.editing, Some((Cell::default(), "x".into())));
        // Escape closes it unsaved.
        let _ = cosmic::Application::on_escape(&mut app);
        assert!(app.editing.is_none());
        assert_eq!(app.pending, 0);
    }

    #[test]
    fn delete_clears_a_cell_and_derived_cells_refuse_entry() {
        let (mut app, mut rx) = live(revenue_model());
        let _ = app.key(&Key::Named(Named::Delete), Modifiers::empty(), None);
        let u = next(&mut rx);
        app.apply(u);
        let k: improv_engine::CoordKey = [(1, 10), (2, 20)].into_iter().collect();
        assert_eq!(
            sheet::value(&app.model, &app.snapshot, MeasureId(100), &k),
            None
        );
        // Revenue is derived: Enter refuses to open an editor.
        app.show(MeasureId(102));
        let _ = app.key(&Key::Named(Named::Enter), Modifiers::empty(), None);
        assert!(app.editing.is_none());
        assert!(app.status.contains("computed"), "{}", app.status);
    }

    #[test]
    fn ctrl_keys_are_not_typed_into_a_cell() {
        let (mut app, _rx) = live(revenue_model());
        let _ = app.key(&Key::Character("c".into()), Modifiers::CTRL, Some("c"));
        assert!(app.editing.is_none());
    }

    fn send(app: &mut App, m: Message) {
        let _ = cosmic::Application::update(app, m);
    }

    #[test]
    fn a_formula_edit_recomputes_and_lists_as_typed() {
        let (mut app, mut rx) = live(revenue_model());
        send(&mut app, Message::EditFormula(0));
        assert_eq!(
            app.formula.as_ref().map(|f| f.text.as_str()),
            Some("Price * Quantity")
        );
        send(
            &mut app,
            Message::FormulaText("Price * Quantity * 2".into()),
        );
        send(&mut app, Message::FormulaCommit);
        let u = next(&mut rx);
        app.apply(u);
        assert!(app.formula.is_none(), "closed once applied");
        assert_eq!(app.status, "recalculated");
        let k: improv_engine::CoordKey = [(1, 10), (2, 20)].into_iter().collect();
        let rev = sheet::value(&app.model, &app.snapshot, MeasureId(102), &k);
        assert_eq!(rev, Some(improv_engine::CellValue::num(2000.0)));
        let rows = crate::formulas::rows(&app.model, &app.locale);
        assert_eq!(rows[0].text, "Price * Quantity * 2");
    }

    #[test]
    fn a_misspelled_name_is_underlined_and_nothing_is_sent() {
        let (mut app, _rx) = live(revenue_model());
        send(&mut app, Message::EditFormula(0));
        send(&mut app, Message::FormulaText("Price * Quantty".into()));
        // No completion for a misspelling, so Enter commits (and refuses).
        assert!(app.completions().is_empty());
        send(&mut app, Message::FormulaCommit);
        let f = app.formula.as_ref().expect("still open");
        let d = f.error.as_ref().expect("located");
        assert_eq!(d.span, Some(8..15));
        assert_eq!(app.pending, 0, "nothing sent");
    }

    #[test]
    fn a_cycle_comes_back_located_and_the_editor_stays() {
        let (mut app, mut rx) = live(revenue_model());
        send(&mut app, Message::EditFormula(0));
        send(&mut app, Message::FormulaText("Revenue + 1".into()));
        send(&mut app, Message::FormulaCommit);
        let u = next(&mut rx);
        assert!(matches!(u, Update::Failed(_)), "{u:?}");
        app.apply(u);
        let f = app.formula.as_ref().expect("still open");
        assert!(f.error.is_some());
        assert_eq!(app.pending, 0);
    }

    #[test]
    fn completion_offers_names_and_enter_takes_the_first() {
        let (mut app, _rx) = live(revenue_model());
        send(&mut app, Message::EditFormula(0));
        send(&mut app, Message::FormulaText("Price * Qua".into()));
        let c = app.completions();
        assert_eq!(c.first().map(|c| c.insert.as_str()), Some("Quantity"));
        send(&mut app, Message::FormulaCommit);
        assert_eq!(app.formula.as_ref().unwrap().text, "Price * Quantity");
        assert_eq!(app.pending, 0, "Enter completed, it did not commit");
        // A function completes to its call.
        send(&mut app, Message::FormulaText("ROU".into()));
        send(&mut app, Message::Complete(0));
        assert!(
            app.formula.as_ref().unwrap().text.starts_with("ROUND"),
            "{:?}",
            app.formula
        );
        let _ = cosmic::Application::on_escape(&mut app);
        assert!(app.formula.is_none());
    }

    #[test]
    fn dragging_a_tile_pivots_without_touching_formulas() {
        let (mut app, _rx) = live(revenue_model());
        // Quantity: Time on rows, Product on columns.
        app.show(MeasureId(100));
        let before = app.model.measures.clone();
        // Drag Product (2) to the start of Rows.
        send(&mut app, Message::Grab(CategoryId(2)));
        send(&mut app, Message::Hover(Axis::Rows, 0));
        send(&mut app, Message::Drop);
        let l = app.layout.as_ref().unwrap();
        assert_eq!(l.row_cats(), &[CategoryId(2), CategoryId(1)]);
        assert!(l.col_cats().is_empty());
        assert_eq!(app.grid.as_ref().unwrap().rows.len(), 4, "2 x 2 rows");
        assert_eq!(app.model.measures, before, "no formula touched");
        assert!(app.held.is_none());
        // Release outside any strip: nothing moves.
        send(&mut app, Message::Grab(CategoryId(1)));
        send(&mut app, Message::Release);
        assert!(app.held.is_none());
        assert_eq!(app.layout.as_ref().unwrap().row_cats().len(), 2);
    }

    #[test]
    fn a_page_shows_the_item_picked_and_an_empty_axis_is_refused() {
        // A third category, on the pages.
        let mut m = revenue_model();
        m.add_category(CategoryId(3), "Scenario");
        m.add_item(ItemId(30), CategoryId(3), "Plan");
        m.add_item(ItemId(31), CategoryId(3), "Actual");
        if let Some(q) = m.measures.get_mut(&MeasureId(100)) {
            q.categories.push(CategoryId(3));
        }
        let (mut app, _rx) = live(m);
        app.show(MeasureId(100));
        assert_eq!(app.layout.as_ref().unwrap().page_items, vec![ItemId(30)]);
        send(&mut app, Message::Page(0, 1));
        assert_eq!(app.layout.as_ref().unwrap().page_items, vec![ItemId(31)]);
        let k = app.grid.as_ref().unwrap().key(0, 0).unwrap();
        assert_eq!(k.as_slice(), &[(1, 10), (2, 20), (3, 31)]);
        // Time to the pages would leave the rows empty: refused.
        send(&mut app, Message::Grab(CategoryId(1)));
        send(&mut app, Message::Hover(Axis::Pages, 1));
        send(&mut app, Message::Drop);
        assert_eq!(app.layout.as_ref().unwrap().row_cats(), &[CategoryId(1)]);
        assert!(app.status.contains("need a category"), "{}", app.status);
        send(&mut app, Message::Transpose);
        assert_eq!(app.layout.as_ref().unwrap().row_cats(), &[CategoryId(2)]);
    }

    #[test]
    fn a_saved_view_reopens_its_layout() {
        let (mut app, mut rx) = live(revenue_model());
        app.show(MeasureId(100));
        send(&mut app, Message::Transpose);
        let pivoted = app.layout.clone();
        send(&mut app, Message::SaveView);
        assert_eq!(app.status, "name the view first");
        send(&mut app, Message::ViewName("By product".into()));
        send(&mut app, Message::SaveView);
        assert_eq!(app.pending, 1);
        let u = next(&mut rx);
        app.apply(u);
        assert_eq!(
            pivot::views(&app.model),
            vec![(improv_core_model::ViewId(1), "By product".into())]
        );
        // Elsewhere, then back through the view.
        app.show(MeasureId(102));
        send(&mut app, Message::OpenView(0));
        assert_eq!(app.layout, pivoted);
        // Saving the same name replaces it.
        send(&mut app, Message::ViewName("By product".into()));
        send(&mut app, Message::SaveView);
        let u = next(&mut rx);
        app.apply(u);
        assert_eq!(app.model.views.len(), 1);
    }

    /// A view the CLI saved (`improv save-view … --rows Product,Time
    /// --cols Scenario`, the GUI screenshot's fixture) opens as saved.
    #[test]
    fn a_view_from_another_front_end_opens() {
        let mut m = revenue_model();
        m.add_view(improv_core_model::View {
            id: improv_core_model::ViewId(1),
            name: improv_core_model::Name("Revenue by product".into()),
            measure: MeasureId(102),
            axis_order: vec![CategoryId(2), CategoryId(1)],
            n_rows: 1,
            n_cols: 1,
            page_items: vec![],
            filters: vec![],
            rect: Default::default(),
            placements: vec![],
        });
        let mut app = opened(m, 10, 5);
        send(&mut app, Message::OpenView(0));
        let l = app.layout.as_ref().unwrap();
        assert_eq!(
            (l.measure, l.row_cats(), l.col_cats()),
            (MeasureId(102), &[CategoryId(2)][..], &[CategoryId(1)][..])
        );
    }

    #[test]
    fn a_failed_open_says_why() {
        let mut app = App::with_service(Core::default(), String::new(), Box::new(Idle));
        app.apply(Update::Failed("no such store".into()));
        assert_eq!(app.status, "no such store");
        assert!(app.grid.is_none());
    }
}
