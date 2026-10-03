//! The NeXTSTEP application menu (spec §4.1): a vertical floating panel with a
//! black title bar and beveled items, whose submenus cascade to the right.
//!
//! The menu knows nothing about the app. [`show`] draws it and returns the
//! [`Command`] the user picked, and the caller runs it. [`key_command`] maps the
//! key equivalents (Ctrl on Linux and Windows, Cmd on macOS) to the same
//! commands. Everything is painted with painter shapes and text.

use egui::{
    pos2, vec2, Align2, Area, Color32, Context, FontFamily, FontId, Id, Key, LayerId, Order,
    Painter, Pos2, Rect, Response, Sense, Shape, Stroke,
};

use crate::theme::{BLACK, BOLD, DARK_GRAY, LIGHT_GRAY, WHITE};

/// Something the user can ask for from the menu or a key equivalent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    InfoPanel,
    /// Show or hide the model browser window.
    ModelBrowser,
    SaveModel,
    Undo,
    Redo,
    PivotRotate,
    AddMatrix,
    RemoveMatrix,
    SaveView,
    /// Open the n-th entry of the `views` list passed to [`show`].
    OpenView(usize),
    ToggleChart,
    ToggleCsv,
    Hide,
    Quit,
}

/// A submenu row: label, key equivalent, command. `None` is not built yet and
/// draws disabled.
type Leaf = (&'static str, Option<char>, Option<Command>);

/// What a top-level item does.
enum Top {
    /// Opens a submenu. An empty one draws disabled.
    Sub(&'static [Leaf]),
    /// Opens the View submenu: Model Browser, then the views the caller
    /// passes to [`show`].
    Views,
    /// Runs a command directly (`None`: disabled).
    Leaf(Option<Command>),
}

use Command as C;

/// The menu tree. Top level per spec §4.1; submenus hold the commands the app
/// can already run, plus disabled placeholders for File.
const MENU: &[(&str, Option<char>, Top)] = &[
    (
        "Info",
        None,
        Top::Sub(&[("Info Panel…", None, Some(C::InfoPanel))]),
    ),
    (
        "File",
        None,
        Top::Sub(&[
            ("New", None, None),
            ("Open…", None, None),
            ("Save", Some('s'), Some(C::SaveModel)),
            ("Save As…", None, None),
            ("Import/Export CSV…", None, Some(C::ToggleCsv)),
            ("Close", None, None),
        ]),
    ),
    (
        "Edit",
        None,
        Top::Sub(&[
            ("Undo", Some('z'), Some(C::Undo)),
            ("Redo", Some('Z'), Some(C::Redo)),
        ]),
    ),
    ("Format", None, Top::Sub(&[])),
    (
        "Worksheet",
        None,
        Top::Sub(&[
            ("Pivot", None, Some(C::PivotRotate)),
            ("Add Matrix", None, Some(C::AddMatrix)),
            ("Remove Matrix", None, Some(C::RemoveMatrix)),
            ("Save View", None, Some(C::SaveView)),
        ]),
    ),
    ("View", None, Top::Views),
    (
        "Graph",
        None,
        Top::Sub(&[("Show Chart", None, Some(C::ToggleChart))]),
    ),
    ("Print…", Some('p'), Top::Leaf(None)),
    ("Services", None, Top::Sub(&[])),
    ("Hide", Some('h'), Top::Leaf(Some(C::Hide))),
    ("Quit", Some('q'), Top::Leaf(Some(C::Quit))),
];

const TITLE: &str = "Improv";
const NO_VIEWS: &str = "No Views";
const MODEL_BROWSER: &str = "Model Browser";

// Geometry, in points (spec §3, §4.1).
const BORDER: f32 = 1.0;
const TITLE_HEIGHT: f32 = 23.0;
const ITEM_HEIGHT: f32 = 20.0;
const PAD: f32 = 6.0;
/// Room between a label and the right-hand marker, plus the marker itself.
const RIGHT_SLOT: f32 = 28.0;
const MIN_WIDTH: f32 = 96.0;
const FONT_SIZE: f32 = 12.0;

/// Where the menu sits and which submenu is open. Lives on the app.
#[derive(Clone, Copy, Debug)]
pub struct MenuState {
    /// Index into the top level of the open submenu.
    open: Option<usize>,
    /// Top-left corner of the menu; dragging the title bar moves it.
    pos: Pos2,
}

impl Default for MenuState {
    fn default() -> Self {
        Self {
            open: None,
            pos: pos2(4.0, 4.0),
        }
    }
}

/// Map a key equivalent (pressed with Ctrl/Cmd) to its command.
pub fn key_command(key: Key, shift: bool) -> Option<Command> {
    Some(match (key, shift) {
        (Key::S, false) => C::SaveModel,
        (Key::Z, false) => C::Undo,
        (Key::Z, true) | (Key::Y, _) => C::Redo,
        (Key::Q, false) => C::Quit,
        (Key::H, false) => C::Hide,
        _ => return None,
    })
}

/// Draw the menu and return the command picked this frame, if any. `views`
/// fills the View submenu as `(name, is_current)`; picking the n-th returns
/// [`Command::OpenView`]`(n)`. `enabled` greys out commands that can't run now.
pub fn show(
    ctx: &Context,
    state: &mut MenuState,
    views: &[(String, bool)],
    enabled: impl Fn(Command) -> bool,
) -> Option<Command> {
    let id = menu_id();
    // Apply this frame's title drag before placing the menu, so it tracks the
    // pointer with no frame delay (as nxwin does).
    if let Some(r) = ctx.read_response(id.with("title")) {
        state.pos += r.drag_delta();
    }
    let screen = ctx.screen_rect();
    state.pos = state.pos.clamp(
        screen.min,
        (screen.max - vec2(MIN_WIDTH, TITLE_HEIGHT)).max(screen.min),
    );

    // A press anywhere but the menu or its open submenu closes the submenu.
    let pressed_at = ctx.input(|i| i.pointer.interact_pos().filter(|_| i.pointer.any_pressed()));
    if let Some(p) = pressed_at {
        let layer = ctx.layer_id_at(p);
        let ours = |id| layer == Some(LayerId::new(Order::Foreground, id));
        if !ours(id) && !state.open.is_some_and(|i| ours(sub_id(i))) {
            state.open = None;
        }
    }
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        state.open = None;
    }

    let rows: Vec<Row> = MENU
        .iter()
        .enumerate()
        .map(|(i, (label, key, top))| Row {
            label,
            key: *key,
            sub: !matches!(top, Top::Leaf(_)),
            check: false,
            enabled: match top {
                Top::Sub(items) => !items.is_empty(),
                Top::Views => true,
                Top::Leaf(c) => c.is_some_and(&enabled),
            },
            open: state.open == Some(i),
        })
        .collect();
    let (menu_rect, items) = panel(ctx, id, state.pos, TITLE, &rows, Sense::drag());

    let mut picked = None;
    for (i, r) in items.iter().enumerate() {
        if !(r.clicked() && rows[i].enabled) {
            continue;
        }
        match MENU[i].2 {
            Top::Leaf(c) => {
                picked = c;
                state.open = None;
            }
            _ => state.open = (state.open != Some(i)).then_some(i),
        }
    }

    if let Some(i) = state.open {
        let leaves: Vec<(&str, Option<char>, Option<Command>, bool)> = match MENU[i].2 {
            Top::Sub(l) => l.iter().map(|&(a, k, c)| (a, k, c, false)).collect(),
            Top::Views => {
                let browser = (MODEL_BROWSER, None, Some(C::ModelBrowser), false);
                let list = views
                    .iter()
                    .enumerate()
                    .map(|(n, (name, cur))| (name.as_str(), None, Some(C::OpenView(n)), *cur));
                if views.is_empty() {
                    vec![browser, (NO_VIEWS, None, None, false)]
                } else {
                    std::iter::once(browser).chain(list).collect()
                }
            }
            Top::Leaf(_) => Vec::new(),
        };
        let rows: Vec<Row> = leaves
            .iter()
            .map(|&(label, key, c, check)| Row {
                label,
                key,
                sub: false,
                check,
                enabled: c.is_some_and(&enabled),
                open: false,
            })
            .collect();
        // Cascade from the right edge, title level with the item.
        let at = pos2(menu_rect.right(), items[i].rect.top() - BORDER);
        let (_, sub) = panel(ctx, sub_id(i), at, MENU[i].0, &rows, Sense::hover());
        for (n, r) in sub.iter().enumerate() {
            if r.clicked() && rows[n].enabled {
                picked = leaves[n].2;
                state.open = None;
            }
        }
    }
    picked
}

fn menu_id() -> Id {
    Id::new("improv_menu")
}

fn sub_id(i: usize) -> Id {
    menu_id().with(("sub", i))
}

/// One row of a menu panel, as drawn.
struct Row<'a> {
    label: &'a str,
    key: Option<char>,
    /// Draw the submenu arrow.
    sub: bool,
    /// Draw a check mark (the current view).
    check: bool,
    enabled: bool,
    /// Its submenu is open: white face.
    open: bool,
}

fn font() -> FontId {
    FontId::proportional(FONT_SIZE)
}

fn bold() -> FontId {
    FontId::new(FONT_SIZE, FontFamily::Name(BOLD.into()))
}

/// Draw one menu panel (title bar plus rows) in its own foreground [`Area`] at
/// `pos`. Returns the panel's outer rect and each row's response.
fn panel(
    ctx: &Context,
    id: Id,
    pos: Pos2,
    title: &str,
    rows: &[Row],
    title_sense: Sense,
) -> (Rect, Vec<Response>) {
    let text_width = |s: &str, f: FontId| {
        ctx.fonts(|fonts| fonts.layout_no_wrap(s.to_owned(), f, BLACK).size().x)
    };
    let width = rows
        .iter()
        .map(|r| text_width(r.label, font()) + RIGHT_SLOT)
        .chain([text_width(title, bold())])
        .fold(MIN_WIDTH, f32::max)
        + 2.0 * (PAD + BORDER);
    let width = width.ceil();
    let height = 2.0 * BORDER + TITLE_HEIGHT + ITEM_HEIGHT * rows.len() as f32;
    let outer = Rect::from_min_size(pos, vec2(width, height));
    let inner = outer.shrink(BORDER);

    Area::new(id)
        .order(Order::Foreground)
        .fixed_pos(pos)
        .constrain(false)
        .fade_in(false)
        .show(ctx, |ui| {
            let p = ui.painter();
            let title_rect = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_HEIGHT));
            p.rect_filled(title_rect, 0.0, BLACK);
            p.text(
                pos2(title_rect.left() + PAD, title_rect.center().y),
                Align2::LEFT_CENTER,
                title,
                bold(),
                WHITE,
            );
            let _ = ui.interact(title_rect, id.with("title"), title_sense);

            let responses = rows
                .iter()
                .enumerate()
                .map(|(i, row)| {
                    let top = title_rect.bottom() + ITEM_HEIGHT * i as f32;
                    let r = Rect::from_min_size(
                        pos2(inner.left(), top),
                        vec2(inner.width(), ITEM_HEIGHT),
                    );
                    let resp = ui.interact(r, id.with(i), Sense::click());
                    let lit = row.open || (row.enabled && resp.is_pointer_button_down_on());
                    paint_row(p, r, row, if lit { WHITE } else { LIGHT_GRAY });
                    resp
                })
                .collect();

            p.rect_stroke(outer.shrink(BORDER / 2.0), 0.0, Stroke::new(BORDER, BLACK));
            ui.advance_cursor_after_rect(outer);
            (outer, responses)
        })
        .inner
}

/// Paint one beveled item: `face`, 1 px white top-left, 1 px black
/// bottom-right, label left and its marker right-aligned.
fn paint_row(p: &Painter, r: Rect, row: &Row, face: Color32) {
    let edge = |min: Pos2, max: Pos2, c: Color32| {
        p.rect_filled(Rect::from_min_max(min, max), 0.0, c);
    };
    p.rect_filled(r, 0.0, face);
    edge(r.min, pos2(r.max.x, r.min.y + 1.0), WHITE);
    edge(r.min, pos2(r.min.x + 1.0, r.max.y), WHITE);
    edge(pos2(r.min.x, r.max.y - 1.0), r.max, BLACK);
    edge(pos2(r.max.x - 1.0, r.min.y), r.max, BLACK);

    let ink = if row.enabled { BLACK } else { DARK_GRAY };
    let y = r.center().y;
    p.text(
        pos2(r.left() + PAD, y),
        Align2::LEFT_CENTER,
        row.label,
        font(),
        ink,
    );
    let right = r.right() - PAD;
    if let Some(k) = row.key {
        p.text(pos2(right, y), Align2::RIGHT_CENTER, k, font(), ink);
    } else if row.sub {
        // ponytail: a plain triangle stands in for NeXT's submenu arrow, which
        // no bundled font has; draw the real shape if the screenshots demand it.
        p.add(Shape::convex_polygon(
            vec![
                pos2(right - 5.0, y - 4.0),
                pos2(right, y),
                pos2(right - 5.0, y + 4.0),
            ],
            ink,
            Stroke::NONE,
        ));
    } else if row.check {
        let s = Stroke::new(1.5_f32, ink);
        p.line_segment([pos2(right - 9.0, y), pos2(right - 6.0, y + 3.5)], s);
        p.line_segment([pos2(right - 6.0, y + 3.5), pos2(right, y - 4.0)], s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, RawInput};

    /// Headless frames of just the menu, with three views ("b" current) and
    /// Remove Matrix disabled.
    struct Harness {
        ctx: Context,
        time: f64,
        state: MenuState,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = Context::default();
            crate::theme::install_fonts(&ctx);
            let mut h = Self {
                ctx,
                time: 0.0,
                state: MenuState::default(),
            };
            // The sizing pass, then a frame whose rows take clicks.
            h.frame(Vec::new());
            h.frame(Vec::new());
            h
        }

        fn frame(&mut self, events: Vec<Event>) -> Option<Command> {
            self.time += 1.0 / 60.0;
            let raw = RawInput {
                time: Some(self.time),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                events,
                ..Default::default()
            };
            let views = ["a", "b", "c"].map(|v| (v.to_owned(), v == "b"));
            let mut picked = None;
            let state = &mut self.state;
            let _ = self.ctx.run(raw, |ctx| {
                picked = show(ctx, state, &views, |c| c != C::RemoveMatrix);
            });
            picked
        }

        fn rect(&self, id: Id) -> Rect {
            self.ctx.memory(|m| m.area_rect(id)).expect("area shown")
        }

        /// Click row `n` of the panel `id`, then let a newly opened submenu
        /// finish its sizing pass and come live.
        fn click_row(&mut self, id: Id, n: usize) -> Option<Command> {
            let r = self.rect(id);
            let at = pos2(
                r.left() + 10.0,
                r.top() + BORDER + TITLE_HEIGHT + ITEM_HEIGHT * (n as f32 + 0.5),
            );
            let picked = self.frame(vec![
                Event::PointerMoved(at),
                button(at, true),
                button(at, false),
            ]);
            self.frame(Vec::new());
            self.frame(Vec::new());
            picked
        }
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn top(label: &str) -> usize {
        MENU.iter().position(|m| m.0 == label).unwrap()
    }

    #[test]
    fn edit_submenu_cascades_right_and_picks_undo() {
        let mut h = Harness::new();
        let edit = top("Edit");
        assert_eq!(h.click_row(menu_id(), edit), None);
        assert_eq!(h.state.open, Some(edit));
        let menu = h.rect(menu_id());
        let sub = h.rect(sub_id(edit));
        assert!(sub.left() >= menu.right() - 1.0, "{sub:?} vs {menu:?}");
        assert!((sub.top() - (menu.top() + TITLE_HEIGHT + ITEM_HEIGHT * edit as f32)).abs() < 1.0);

        assert_eq!(h.click_row(sub_id(edit), 0), Some(C::Undo));
        assert_eq!(h.state.open, None);
    }

    #[test]
    fn clicking_the_open_item_closes_it() {
        let mut h = Harness::new();
        let edit = top("Edit");
        h.click_row(menu_id(), edit);
        h.click_row(menu_id(), edit);
        assert_eq!(h.state.open, None);
    }

    #[test]
    fn top_level_leaf_and_view_rows_return_commands() {
        let mut h = Harness::new();
        assert_eq!(h.click_row(menu_id(), top("Quit")), Some(C::Quit));
        let view = top("View");
        h.click_row(menu_id(), view);
        assert_eq!(h.click_row(sub_id(view), 3), Some(C::OpenView(2)));
    }

    /// View ▸ Model Browser sits above the views list and dispatches.
    #[test]
    fn view_model_browser_dispatches() {
        let mut h = Harness::new();
        let view = top("View");
        h.click_row(menu_id(), view);
        assert_eq!(h.click_row(sub_id(view), 0), Some(C::ModelBrowser));
    }

    #[test]
    fn disabled_rows_return_nothing() {
        let mut h = Harness::new();
        assert_eq!(h.click_row(menu_id(), top("Print…")), None);
        assert_eq!(h.click_row(menu_id(), top("Format")), None);
        assert_eq!(h.state.open, None, "an empty submenu doesn't open");

        let ws = top("Worksheet");
        h.click_row(menu_id(), ws);
        assert_eq!(
            h.click_row(sub_id(ws), 2),
            None,
            "Remove Matrix is disabled"
        );
        assert_eq!(h.state.open, Some(ws), "a disabled click leaves it open");
        let file = top("File");
        h.click_row(menu_id(), file);
        assert_eq!(h.click_row(sub_id(file), 0), None, "New isn't built yet");
    }

    #[test]
    fn escape_or_a_click_elsewhere_closes_the_submenu() {
        let mut h = Harness::new();
        h.click_row(menu_id(), top("Edit"));
        h.frame(vec![Event::Key {
            key: Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
        assert_eq!(h.state.open, None);

        h.click_row(menu_id(), top("Edit"));
        let away = pos2(600.0, 500.0);
        h.frame(vec![Event::PointerMoved(away), button(away, true)]);
        assert_eq!(h.state.open, None);
    }

    #[test]
    fn title_drag_moves_the_menu() {
        let mut h = Harness::new();
        let from = pos2(30.0, 12.0);
        let to = from + vec2(100.0, 50.0);
        h.frame(vec![Event::PointerMoved(from)]);
        h.frame(vec![button(from, true)]);
        h.frame(vec![Event::PointerMoved(to)]);
        h.frame(vec![button(to, false)]);
        assert_eq!(h.state.pos, pos2(104.0, 54.0));
    }

    #[test]
    fn key_equivalents_map_to_commands() {
        assert_eq!(key_command(Key::S, false), Some(C::SaveModel));
        assert_eq!(key_command(Key::Z, false), Some(C::Undo));
        assert_eq!(key_command(Key::Z, true), Some(C::Redo));
        assert_eq!(key_command(Key::Y, false), Some(C::Redo));
        assert_eq!(key_command(Key::Q, false), Some(C::Quit));
        assert_eq!(key_command(Key::H, false), Some(C::Hide));
        assert_eq!(key_command(Key::S, true), None);
        assert_eq!(key_command(Key::A, false), None);
    }

    /// Every label and key letter renders from the installed fonts, not as the
    /// missing-glyph box. Replaces the old tool-palette glyph check.
    #[test]
    fn menu_glyphs_are_in_the_installed_fonts() {
        let h = Harness::new();
        let mut texts: Vec<String> = vec![NO_VIEWS.into(), MODEL_BROWSER.into()];
        for (label, key, top) in MENU {
            texts.push((*label).into());
            texts.extend(key.map(String::from));
            if let Top::Sub(leaves) = top {
                for (label, key, _) in *leaves {
                    texts.push((*label).into());
                    texts.extend(key.map(String::from));
                }
            }
        }
        h.ctx.fonts(|f| {
            assert!(f.has_glyphs(&bold(), TITLE));
            for t in &texts {
                assert!(f.has_glyphs(&font(), t), "missing glyph in {t:?}");
            }
        });
    }
}
