//! The window: a COSMIC application over one model. It holds a copy of the
//! model for drawing and talks to the model service (a thread) for
//! everything else, so nothing it does waits on a load, an engine build,
//! a recalculation or a save.

use std::sync::{mpsc, Arc};

use cosmic::app::{Core, Task};
use cosmic::iced::keyboard::{self, key::Named, Key, Modifiers};
use cosmic::iced::{Length, Size, Subscription};
use cosmic::widget;
use cosmic::{executor, ApplicationExt, Element};

use improv_core_model::{Locale, MeasureId, Model};
use improv_engine::session::Snapshot;

use crate::nav::{self, Extent, Move};
use crate::pivot::{Grid, Layout};
use crate::service::{self, CellEdit, LocalService, ModelService, Request, Update};
use crate::sheet::{self, Cell, Sheet, SheetProgram};

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
        if self.editing.is_some() {
            // The editor has the keyboard (Escape: [`Self::on_escape`]).
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
            Update::Failed(e) => {
                self.pending = self.pending.saturating_sub(1);
                self.status = e;
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
        if self.editing.take().is_some() {
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
        let bar = widget::row::with_capacity(3)
            .push(widget::dropdown(names, shown, Message::ShowMeasure))
            .push(widget::space::horizontal())
            .push(widget::text::caption(self.status.clone()))
            .spacing(cosmic::theme::spacing().space_s)
            .align_y(cosmic::iced::Alignment::Center);
        let body: Element<'_, Message> = match (&self.layout, &self.grid) {
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
        widget::column::with_capacity(2)
            .push(bar)
            .push(body)
            .spacing(cosmic::theme::spacing().space_s)
            .padding(cosmic::theme::spacing().space_s)
            .into()
    }
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

    #[test]
    fn a_failed_open_says_why() {
        let mut app = App::with_service(Core::default(), String::new(), Box::new(Idle));
        app.apply(Update::Failed("no such store".into()));
        assert_eq!(app.status, "no such store");
        assert!(app.grid.is_none());
    }
}
