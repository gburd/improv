//! The window: a COSMIC application over one model. It holds a copy of the
//! model for drawing and talks to the model service (a thread) for
//! everything else, so nothing it does waits on a load, an engine build,
//! a recalculation or a save.

use std::sync::{mpsc, Arc};

use cosmic::app::{Core, Task};
use cosmic::iced::{Length, Subscription};
use cosmic::widget;
use cosmic::{executor, ApplicationExt, Element};

use improv_core_model::{Locale, MeasureId, Model};
use improv_engine::session::Snapshot;

use crate::pivot::{Grid, Layout};
use crate::service::{self, LocalService, ModelService, Request, Update};
use crate::sheet::{self, Cell, Sheet, SheetProgram};

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
    _service: mpsc::Sender<Request>,
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
            _service: service::spawn(service, tx),
            updates: UpdateRx(Arc::new(std::sync::Mutex::new(Some(rx)))),
            model: Arc::new(Model::new()),
            snapshot: Snapshot::default(),
            measures: Vec::new(),
            layout: None,
            grid: None,
            origin: Cell::default(),
            cursor: Cell::default(),
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
            Update::Failed(e) => self.status = e,
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
        Subscription::run_with(self.updates.clone(), updates)
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
            Message::Sheet(sheet::Event::Clicked(c) | sheet::Event::Activated(c)) => {
                self.cursor = c;
            }
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
            (Some(l), Some(g)) => widget::canvas(SheetProgram {
                sheet: Sheet {
                    model: &self.model,
                    snapshot: &self.snapshot,
                    measure: l.measure,
                    grid: g,
                    format: self.model.measures.get(&l.measure).and_then(|m| m.format),
                    locale: &self.locale,
                    origin: self.origin,
                    cursor: self.cursor,
                },
                on_event: Message::Sheet,
            })
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
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
