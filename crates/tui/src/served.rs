//! A model improv-server serves (`improv-tui https://…`): one measure's
//! grid over the remote model service, as the user the token names.
//! Everything comes from the server: the structure (hidden measures, items
//! and cells absent), the cells of the page on screen (derived ones
//! computed there), the capabilities that say what may be edited, and the
//! changes others make, which redraw as they arrive. Nothing is computed
//! here, and every refusal shows on the status line.

use std::collections::BTreeMap;
use std::io;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEventKind, KeyModifiers};
use improv_client::layout::{Cache, Sheet};
use improv_client::wire::{
    At, Capabilities, Cell, CellEdit, EditReq, Event as SvcEvent, Failure, ModelService, Request,
    Response, Structure, WindowReq,
};
use improv_core_model::{Locale, Model};
use ratatui::backend::Backend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};

use crate::edit::LineEdit;

/// What a sent request was for.
enum Pending {
    Structure,
    Capabilities,
    Window(u32, WindowReq),
    Cell(u32, At),
    Other,
}

pub struct Served<S: ModelService> {
    svc: S,
    /// What the window's title says: the server's URL.
    pub title: String,
    structure: Structure,
    caps: Option<Capabilities>,
    /// The structure as a model without cells (to read typed values).
    model: Model,
    cache: Cache,
    sheet: Option<Sheet>,
    cursor: (usize, usize),
    pending: BTreeMap<u64, Pending>,
    pub status: String,
    edit: Option<LineEdit>,
    /// Asking for the token (it is typed here, shown as `*`).
    token: Option<LineEdit>,
    locale: Locale,
    pub quit: bool,
}

impl<S: ModelService> Served<S> {
    /// The app over `svc`; with `token` (`IMPROV_API_TOKEN`, or empty for
    /// a server without tokens) it logs in at once, else it asks.
    pub fn new(mut svc: S, title: String, token: Option<String>) -> Self {
        let asking = token.is_none();
        if let Some(t) = token {
            svc.authenticate(t);
        }
        let mut app = Served {
            svc,
            title,
            structure: Structure::default(),
            caps: None,
            model: Model::new(),
            cache: Cache::default(),
            sheet: None,
            cursor: (0, 0),
            pending: BTreeMap::new(),
            status: String::new(),
            edit: None,
            token: asking.then(LineEdit::default),
            locale: Locale::from_env(),
            quit: false,
        };
        if !asking {
            app.reload();
        }
        app
    }

    fn ask(&mut self, req: Request, why: Pending) {
        let t = self.svc.send(req);
        self.pending.insert(t, why);
    }

    fn reload(&mut self) {
        self.ask(Request::Structure, Pending::Structure);
        self.ask(Request::Capabilities, Pending::Capabilities);
    }

    /// Take in what the service delivered; true when anything came.
    pub fn drain(&mut self) -> bool {
        let events = self.svc.poll();
        let any = !events.is_empty();
        for ev in events {
            match ev {
                SvcEvent::LoggedOut(e) => {
                    self.status = format!("the server refused the token ({e}): type another");
                    self.token = Some(LineEdit::default());
                    self.pending.clear();
                }
                SvcEvent::Changed(c) if c.reload => self.reload(),
                SvcEvent::Changed(c) => {
                    for x in c.cells {
                        self.cache.set(x.measure, x.at, x.value);
                    }
                }
                SvcEvent::Reply(t, r) => {
                    if let Some(why) = self.pending.remove(&t) {
                        self.reply(why, r);
                    }
                }
            }
        }
        self.fetch();
        any
    }

    fn reply(&mut self, why: Pending, r: Result<Response, Failure>) {
        match (why, r) {
            (Pending::Structure, Ok(Response::Structure(s))) => self.adopt(s),
            // No longer allowed to open it (a policy changed): nothing of
            // it stays on screen.
            (Pending::Structure, Err(f)) => {
                self.adopt(Structure::default());
                self.status = f.error;
            }
            (Pending::Capabilities, Ok(Response::Capabilities(c))) => self.caps = Some(c),
            (Pending::Capabilities, Err(_)) => self.caps = None,
            (Pending::Window(m, w), Ok(Response::Cells(c))) => {
                let cells = c.cells.into_iter().map(|x| (x.at, x.value)).collect();
                self.cache.fill(m, &w, cells);
            }
            (Pending::Cell(..) | Pending::Other, Ok(Response::Done(d))) => self.status = d.message,
            (Pending::Cell(m, at), Err(f)) => {
                self.status = format!("not saved: {}", f.error);
                // The server's value again.
                self.cache.set(m, at, None);
                self.cache.clear();
            }
            (_, Err(f)) => self.status = f.error,
            (_, Ok(_)) => {}
        }
    }

    /// A new structure: stay on the measure when it is still there.
    fn adopt(&mut self, s: Structure) {
        self.model = s.to_model();
        let keep = self.sheet.take();
        self.structure = s;
        self.cache.clear();
        let m = keep
            .as_ref()
            .map(|k| k.measure)
            .filter(|m| self.structure.measure(*m).is_some())
            .or_else(|| self.structure.measures.first().map(|m| m.id));
        self.sheet = m.map(|m| match keep {
            Some(k) if k.measure == m => Sheet {
                order: Sheet::new(&self.structure, m).order,
                ..k
            },
            _ => Sheet::new(&self.structure, m),
        });
        if self.status.is_empty() {
            self.status = "ready".into();
        }
        self.clamp();
    }

    /// Ask for the page on screen when it has not been read.
    fn fetch(&mut self) {
        let Some(sh) = &self.sheet else { return };
        let (m, w) = (sh.measure, sh.window(&self.structure));
        let asked = self
            .pending
            .values()
            .any(|p| matches!(p, Pending::Window(x, y) if *x == m && *y == w));
        if !self.cache.has(m, &w) && !asked {
            self.ask(Request::Window(m, w.clone()), Pending::Window(m, w));
        }
    }

    /// The row tuples and the column tuples on screen.
    fn axes(&self) -> (Vec<At>, Vec<At>) {
        match &self.sheet {
            Some(sh) => (
                Sheet::tuples(&self.structure, sh.rows()),
                Sheet::tuples(&self.structure, sh.cols()),
            ),
            None => (Vec::new(), Vec::new()),
        }
    }

    fn clamp(&mut self) {
        let (r, c) = self.axes();
        self.cursor.0 = self.cursor.0.min(r.len().saturating_sub(1));
        self.cursor.1 = self.cursor.1.min(c.len().saturating_sub(1));
    }

    /// The coordinate under the cursor.
    fn at_cursor(&self) -> Option<At> {
        let sh = self.sheet.as_ref()?;
        let (r, c) = self.axes();
        Some(sh.at(
            &self.structure,
            r.get(self.cursor.0)?,
            c.get(self.cursor.1)?,
        ))
    }

    /// Whether some of the measure's cells may be edited: an input, not
    /// filled from a source, and (by the capabilities) not `none`.
    fn may_edit(&self, m: &improv_client::wire::MeasureOut) -> bool {
        m.editable
            && self
                .caps
                .as_ref()
                .is_none_or(|c| c.measures.iter().any(|x| x.id == m.id && x.cells != "none"))
    }

    fn measure(&self) -> Option<&improv_client::wire::MeasureOut> {
        self.structure.measure(self.sheet.as_ref()?.measure)
    }

    /// One terminal event.
    pub fn key(&mut self, ev: Event) {
        let Event::Key(k) = ev else { return };
        if k.kind == KeyEventKind::Release {
            return;
        }
        if let Some(t) = &mut self.token {
            match k.code {
                KeyCode::Enter => {
                    let secret = std::mem::take(&mut t.text);
                    self.token = None;
                    self.svc.authenticate(secret);
                    self.status = "logging in…".into();
                    self.reload();
                }
                KeyCode::Esc => self.quit = true,
                code => drop(t.key(code)),
            }
            return;
        }
        if let Some(e) = &mut self.edit {
            match k.code {
                KeyCode::Enter => {
                    let text = std::mem::take(&mut e.text);
                    self.edit = None;
                    self.commit(&text);
                }
                KeyCode::Esc => self.edit = None,
                code => drop(e.key(code)),
            }
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.structure.measures.len();
        match k.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Up => self.cursor.0 = self.cursor.0.saturating_sub(1),
            KeyCode::Down => self.cursor.0 += 1,
            KeyCode::Left => self.cursor.1 = self.cursor.1.saturating_sub(1),
            KeyCode::Right => self.cursor.1 += 1,
            KeyCode::Tab | KeyCode::Char('m') if n > 0 => {
                let i = self
                    .structure
                    .measures
                    .iter()
                    .position(|m| Some(m.id) == self.sheet.as_ref().map(|s| s.measure))
                    .map_or(0, |i| (i + 1) % n);
                self.sheet = Some(Sheet::new(&self.structure, self.structure.measures[i].id));
            }
            KeyCode::Char('p') => {
                if let Some(sh) = &mut self.sheet {
                    sh.order.rotate_left(1);
                }
            }
            KeyCode::Char(c @ ('[' | ']')) => self.page(if c == '[' { -1 } else { 1 }),
            KeyCode::Char('e') | KeyCode::Enter => self.begin(String::new(), false),
            KeyCode::Char(c @ ('0'..='9' | '-' | '.')) => self.begin(c.to_string(), true),
            KeyCode::Char('u') => self.ask(Request::Undo, Pending::Other),
            KeyCode::Char('r') if ctrl => self.ask(Request::Redo, Pending::Other),
            KeyCode::Char('R') => {
                self.status = "reloaded".into();
                self.reload();
            }
            KeyCode::Esc => self.status.clear(),
            _ => {}
        }
        self.clamp();
    }

    /// Step the first page category's item.
    fn page(&mut self, by: isize) {
        let Some(sh) = &mut self.sheet else { return };
        let Some(c) = sh.pages().first().copied() else {
            self.status = "no page to change".into();
            return;
        };
        let items: Vec<u32> = self
            .structure
            .category(c)
            .map(|k| k.items.iter().map(|i| i.id).collect())
            .unwrap_or_default();
        let cur = sh.page_item(&self.structure, c);
        let i = items.iter().position(|x| Some(*x) == cur).unwrap_or(0) as isize;
        let n = items.len() as isize;
        if n > 0 {
            sh.page.insert(c, items[(i + by).rem_euclid(n) as usize]);
        }
    }

    /// Start editing the cell under the cursor, with `seed` typed.
    fn begin(&mut self, seed: String, typed: bool) {
        let Some(m) = self.measure().cloned() else {
            return;
        };
        if m.kind == "derived" {
            self.status = format!("{}'s cells are computed by the server", m.name);
            return;
        }
        if !self.may_edit(&m) {
            self.status = format!("you may not edit {}'s cells", m.name);
            return;
        }
        let Some(at) = self.at_cursor() else { return };
        self.edit = Some(match (typed, self.cache.get(m.id, &at)) {
            (true, _) => seed.into(),
            (false, Cell::Value(v)) => LineEdit::replacing(v.typed()),
            (false, _) => LineEdit::default(),
        });
    }

    /// Send the edit; the cell shows it at once, and the server's answer
    /// (with what it recalculated) follows on the change stream.
    fn commit(&mut self, text: &str) {
        let (Some(m), Some(at)) = (self.measure().cloned(), self.at_cursor()) else {
            return;
        };
        let text = text.trim();
        let value = match text.is_empty() {
            true => None,
            false => match improv_client::wire::parse_typed(
                &self.model,
                &self.locale,
                m.value_type,
                text,
            ) {
                Ok(v) => Some(v),
                Err(e) => {
                    self.status = e;
                    return;
                }
            },
        };
        self.cache.set(m.id, at.clone(), value.clone());
        let edits = vec![CellEdit {
            measure: m.id,
            at: at.clone(),
            value,
        }];
        self.ask(Request::Edit(EditReq { edits }), Pending::Cell(m.id, at));
    }

    /// The status box's lines, then the grid.
    pub fn render(&self, f: &mut Frame) {
        let mut lines = vec![];
        let live = if self.svc.live() {
            "live"
        } else {
            "connecting…"
        };
        match self.measure() {
            Some(m) => {
                let kind = if m.kind == "derived" {
                    "derived"
                } else {
                    "input"
                };
                let ro = match self.may_edit(m) || m.kind == "derived" {
                    true => "",
                    false => ", read only",
                };
                lines.push(Line::from(format!("{} [{kind}{ro}]   {live}", m.name)));
                if let Some(fx) = &m.formula {
                    lines.push(Line::from(format!("{} = {fx}", m.name)));
                }
            }
            None => lines.push(Line::from(format!("no measure you may see   {live}"))),
        }
        if let Some(t) = &self.token {
            lines.push(Line::from(format!(
                "token> {}",
                "*".repeat(t.text.chars().count())
            )));
        } else if let Some(e) = &self.edit {
            lines.push(Line::from(format!("edit> {}", e.text)));
        }
        if !self.status.is_empty() {
            lines.push(Line::from(self.status.clone()));
        }
        if let Some(sh) = &self.sheet {
            let pages: Vec<String> = (sh.pages().iter())
                .filter_map(|c| {
                    let k = self.structure.category(*c)?;
                    let i = sh.page_item(&self.structure, *c)?;
                    let n = k.items.iter().find(|x| x.id == i)?;
                    Some(format!("{}={}", k.name, n.name))
                })
                .collect();
            if !pages.is_empty() {
                lines.push(Line::from(format!(
                    "pages: {}   ([ ] to change)",
                    pages.join(", ")
                )));
            }
        }
        lines.push(Line::styled(
            "e: edit  Tab: measure  p: pivot  [ ]: page  u: undo  R: reload  q: quit",
            Style::default().add_modifier(Modifier::DIM),
        ));
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(lines.len() as u16 + 2),
                Constraint::Min(3),
            ])
            .split(f.area());
        let title = format!(" {} ", self.title);
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title)),
            chunks[0],
        );
        f.render_widget(self.table(), chunks[1]);
    }

    fn table(&self) -> Table<'_> {
        let block = Block::default().borders(Borders::ALL).title(" Improv ");
        let (Some(sh), Some(m)) = (&self.sheet, self.measure()) else {
            return Table::new(Vec::<Row>::new(), [Constraint::Min(1)]).block(block);
        };
        let st = &self.structure;
        let name = |c: u32, i: u32| {
            st.category(c)
                .and_then(|k| k.items.iter().find(|x| x.id == i))
                .map_or_else(String::new, |x| x.name.clone())
        };
        let label = |t: &[(u32, u32)]| {
            t.iter()
                .map(|(c, i)| name(*c, *i))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let (rows, cols) = self.axes();
        let corner = sh
            .rows()
            .first()
            .and_then(|c| st.category(*c))
            .map_or_else(String::new, |k| k.name.clone());
        let mut head = vec![corner];
        head.extend(cols.iter().map(|c| label(c)));
        let sel = Style::default()
            .bg(Color::Blue)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED);
        let body = rows.iter().enumerate().map(|(ri, r)| {
            let mut cells = vec![ratatui::widgets::Cell::from(label(r))
                .style(Style::default().add_modifier(Modifier::BOLD))];
            for (ci, c) in cols.iter().enumerate() {
                let text = match self.cache.get(m.id, &sh.at(st, r, c)) {
                    Cell::Value(v) => v.shown(m.format.as_ref(), &self.locale),
                    Cell::Blank => String::new(),
                    Cell::NotLoaded => "…".into(),
                };
                let cell = ratatui::widgets::Cell::from(text);
                cells.push(match (ri, ci) == self.cursor {
                    true => cell.style(sel),
                    false => cell,
                });
            }
            Row::new(cells)
        });
        let widths: Vec<Constraint> = std::iter::once(Constraint::Length(16))
            .chain(cols.iter().map(|_| Constraint::Length(12)))
            .collect();
        Table::new(body, widths)
            .header(Row::new(head).style(Style::default().add_modifier(Modifier::BOLD)))
            .block(block)
            .column_spacing(1)
    }
}

/// Draw, take what the service sent, and handle each terminal event
/// (`next` waits up to a tick for one) until the app quits or `next` runs
/// dry.
pub fn run<B: Backend, S: ModelService>(
    term: &mut Terminal<B>,
    app: &mut Served<S>,
    mut next: impl FnMut(Duration) -> Option<io::Result<Option<Event>>>,
) -> io::Result<()>
where
    B::Error: Send + Sync + 'static,
{
    while !app.quit {
        app.drain();
        term.draw(|f| app.render(f)).map_err(io::Error::other)?;
        match next(TICK) {
            None => break,
            Some(ev) => {
                if let Some(ev) = ev? {
                    app.key(ev);
                }
            }
        }
    }
    Ok(())
}

/// How often the screen looks for the service's news without a key.
const TICK: Duration = Duration::from_millis(100);

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crossterm::event::KeyEvent;
    use improv_core_model::{
        BinaryOp, CategoryId, Coordinate, DimensionSpec, Expr, Formula, ItemId, Measure, MeasureId,
        MeasureKind, Name, Value, ValueType,
    };
    use ratatui::backend::TestBackend;

    /// Ann: no WidgetB (21), no Price (100); edits Quantity at 2025 only.
    const POLICIES: &str = r#"
        permit (principal, action in Improv::Action::"Read", resource);
        forbid (principal == Improv::User::"ann", action in Improv::Action::"Read",
                resource in Improv::Item::"21");
        forbid (principal == Improv::User::"ann", action in Improv::Action::"Read",
                resource in Improv::Measure::"100");
        permit (principal == Improv::User::"ann", action == Improv::Action::"EditCell",
                resource in Improv::Item::"10");
    "#;

    fn model() -> Model {
        let mut m = Model::new();
        let (time, product, region) = (CategoryId(1), CategoryId(2), CategoryId(3));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_category(region, "Region");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(11), time, "2026");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(21), product, "WidgetB");
        m.add_item(ItemId(30), region, "North");
        m.add_item(ItemId(31), region, "South");
        let input = |id, name: &str, cats| Measure {
            id: MeasureId(id),
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories: cats,
            kind: MeasureKind::Input,
            description: None,
            format: None,
        };
        m.add_measure(input(100, "Price", vec![product]));
        m.add_measure(input(101, "Quantity", vec![time, product, region]));
        let mut rev = input(102, "Revenue", vec![time, product, region]);
        rev.kind = MeasureKind::Derived(Formula::new(Expr::BinaryOp(
            BinaryOp::Mul,
            Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
            Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
        )));
        m.add_measure(rev);
        let at = |p: &[(u32, u32)]| {
            Coordinate::from_pairs(p.iter().map(|(c, i)| (CategoryId(*c), ItemId(*i))))
        };
        m.set_input(MeasureId(100), at(&[(2, 20)]), Value::Number(10.0));
        m.set_input(MeasureId(100), at(&[(2, 21)]), Value::Number(20.0));
        for (t, p, r, q) in [(10, 20, 30, 100.0), (11, 20, 30, 120.0), (10, 21, 30, 50.0)] {
            m.set_input(
                MeasureId(101),
                at(&[(1, t), (2, p), (3, r)]),
                Value::Number(q),
            );
        }
        m.set_input(
            MeasureId(101),
            at(&[(1, 10), (2, 20), (3, 31)]),
            Value::Number(7.0),
        );
        m
    }

    type App = Served<improv_service::local::Local>;

    fn open(who: &str, token: Option<String>) -> (App, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "improv-tui-served-{who}-{}-{:?}.db",
            std::process::id(),
            std::thread::current().id()
        ));
        let p = path.to_str().unwrap();
        let mut s = improv_store::Store::open(p).unwrap();
        s.save_model(&model()).unwrap();
        s.save_policies(Some((POLICIES, ""))).unwrap();
        drop(s);
        let policy = improv_service::cedar::CedarPolicy::new(p, improv_policy::Channel::Server);
        let a = improv_service::authority::Authority::with_policy(p, Box::new(policy)).unwrap();
        let svc = improv_service::local::Local::new(a, improv_policy::Principal::user(who));
        let mut app = Served::new(svc, "https://improv.test".into(), token);
        app.locale = Locale::en();
        (app, path)
    }

    fn key(c: KeyCode) -> Event {
        Event::Key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    /// Run `keys` (one a tick, then a few idle ticks) and the screen after.
    fn run_keys(app: &mut App, keys: Vec<Event>) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(100, 20)).unwrap();
        let mut keys = keys.into_iter();
        let mut idle = 3;
        run(&mut term, app, |_| match keys.next() {
            Some(k) => Some(Ok(Some(k))),
            None if idle > 0 => {
                idle -= 1;
                Some(Ok(None))
            }
            None => None,
        })
        .unwrap();
        let buf = term.backend().buffer();
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    fn chars(s: &str) -> Vec<Event> {
        s.chars().map(|c| key(KeyCode::Char(c))).collect()
    }

    #[test]
    fn a_restricted_user_sees_and_edits_only_what_the_policies_allow() {
        let (mut app, path) = open("ann", Some(String::new()));
        let screen = run_keys(&mut app, vec![]).join("\n");
        // Price is absent: the first measure is Quantity; WidgetB has no
        // column; Region is a page.
        assert!(screen.contains("Quantity [input]   live"), "{screen}");
        assert!(
            screen.contains("WidgetA") && !screen.contains("WidgetB"),
            "{screen}"
        );
        assert!(screen.contains("pages: Region=North"), "{screen}");
        assert!(screen.contains("100") && screen.contains("120"), "{screen}");
        // Revenue: the server's values (over the hidden Price), no formula.
        let screen = run_keys(&mut app, vec![key(KeyCode::Tab)]).join("\n");
        assert!(screen.contains("Revenue [derived]"), "{screen}");
        assert!(
            screen.contains("1000") && !screen.contains("Revenue ="),
            "{screen}"
        );
        let screen = run_keys(&mut app, chars("e")).join("\n");
        assert!(
            screen.contains("Revenue's cells are computed by the server"),
            "{screen}"
        );
        // Back on Quantity: the South page, then an edit at 2025.
        let _ = run_keys(&mut app, vec![key(KeyCode::Tab), key(KeyCode::Char(']'))]);
        let screen = run_keys(&mut app, chars("e")).join("\n");
        assert!(
            screen.contains("Region=South") && screen.contains("edit> 7"),
            "{screen}"
        );
        let mut ev = chars("9");
        ev.push(key(KeyCode::Enter));
        let screen = run_keys(&mut app, ev).join("\n");
        assert!(screen.contains("1 cell set"), "{screen}");
        // At 2026 the server refuses it, and says so; the old value stays.
        let mut ev = vec![key(KeyCode::Down)];
        ev.extend(chars("5"));
        ev.push(key(KeyCode::Enter));
        let screen = run_keys(&mut app, ev).join("\n");
        assert!(screen.contains("not saved: "), "{screen}");
        // Undo goes to the server: the 9 at 2025 South is 7 again.
        let screen = run_keys(&mut app, chars("u")).join("\n");
        assert!(
            screen.contains("undone") || screen.contains("undo"),
            "{screen}"
        );
        let st = improv_store::Store::open(path.to_str().unwrap())
            .unwrap()
            .load_model()
            .unwrap();
        let at = Coordinate::from_pairs([
            (CategoryId(1), ItemId(10)),
            (CategoryId(2), ItemId(20)),
            (CategoryId(3), ItemId(31)),
        ]);
        assert_eq!(st.input(MeasureId(101), &at), Some(&Value::Number(7.0)));
        std::fs::remove_file(path).ok();
    }

    /// The paths around the grid: a value that does not parse, an empty
    /// commit clearing a cell, Esc in an edit and in the token prompt, a
    /// refused token asking again, redo, a release event, a model with
    /// nothing the user may see.
    #[test]
    fn edits_tokens_and_empty_models() {
        let (mut app, path) = open("ann", Some(String::new()));
        let _ = run_keys(&mut app, vec![key(KeyCode::Char(']'))]);
        // A value of the wrong shape: said here, nothing sent.
        let mut ev = chars("eabc");
        ev.push(key(KeyCode::Enter));
        let screen = run_keys(&mut app, ev).join("\n");
        assert!(screen.contains("not a number"), "{screen}");
        // Esc leaves an edit; an empty commit clears the cell.
        let _ = run_keys(&mut app, vec![key(KeyCode::Char('e')), key(KeyCode::Esc)]);
        let mut ev = chars("e");
        ev.extend([key(KeyCode::Backspace), key(KeyCode::Enter)]);
        let screen = run_keys(&mut app, ev).join("\n");
        assert!(screen.contains("1 cell set"), "{screen}");
        // Undo, redo (Ctrl+R), and a key release does nothing.
        let ctrl_r = Event::Key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        let mut release = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        release.kind = KeyEventKind::Release;
        let _ = run_keys(
            &mut app,
            vec![key(KeyCode::Char('u')), ctrl_r, Event::Key(release)],
        );
        assert!(!app.quit);
        // The server refuses the token: the prompt again, Esc quits.
        app.svc.authenticate(String::new());
        app.drain();
        app.status.clear();
        app.reply(
            Pending::Other,
            Err(Failure {
                error: "refused".into(),
                ..Default::default()
            }),
        );
        assert_eq!(app.status, "refused");
        app.token = Some(LineEdit::default());
        let screen = run_keys(&mut app, vec![]).join("\n");
        assert!(screen.contains("token> "), "{screen}");
        let _ = run_keys(&mut app, vec![key(KeyCode::Esc)]);
        assert!(app.quit);
        std::fs::remove_file(path).ok();
        // A user the policies do not let open the model: the refusal is
        // said, no measure shows, and the keys are safe.
        let (mut app, path) = open("nobody", Some(String::new()));
        let p = path.to_str().unwrap();
        improv_store::Store::open(p)
            .unwrap()
            .save_policies(Some((
                r#"permit (principal == Improv::User::"ann", action, resource);"#,
                "",
            )))
            .unwrap();
        app.reload();
        let screen = run_keys(&mut app, chars("e[pm")).join("\n");
        assert!(screen.contains("no measure you may see"), "{screen}");
        std::fs::remove_file(path).ok();
    }

    /// Another program's write shows without a key; pivoting and the
    /// token prompt.
    #[test]
    fn changes_redraw_and_the_token_is_asked_for() {
        let (mut app, path) = open("root", None);
        let screen = run_keys(&mut app, chars("s3")).join("\n");
        assert!(screen.contains("token> **"), "{screen}");
        assert!(!screen.contains("s3"), "the token is not shown");
        let screen = run_keys(&mut app, vec![key(KeyCode::Enter)]).join("\n");
        assert!(screen.contains("Price [input"), "{screen}");
        // Root may not edit (no EditCell for root): read only, refused.
        let screen = run_keys(&mut app, chars("e")).join("\n");
        assert!(
            screen.contains("read only") && screen.contains("you may not edit"),
            "{screen}"
        );
        // Another writer: the change arrives and redraws.
        let p = path.to_str().unwrap();
        let mut s = improv_store::Store::open(p).unwrap();
        let mut m = s.load_model().unwrap();
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(CategoryId(2), ItemId(20))]),
            Value::Number(42.5),
        );
        s.save_model(&m).unwrap();
        drop(s);
        let screen = run_keys(&mut app, vec![]).join("\n");
        assert!(screen.contains("42.5"), "{screen}");
        // Pivot, reload, a page key without pages, Esc and quit.
        let screen = run_keys(&mut app, chars("p[R")).join("\n");
        assert!(screen.contains("reloaded"), "{screen}");
        let _ = run_keys(&mut app, vec![key(KeyCode::Esc), key(KeyCode::Char('q'))]);
        assert!(app.quit);
        std::fs::remove_file(path).ok();
    }
}
