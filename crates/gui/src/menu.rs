//! The NeXTSTEP application menu (spec §4.1): a vertical floating panel with a
//! black title bar, its title at the left margin, and beveled items, with a
//! dark-gray border on its top and left sides only; submenus cascade to the
//! right.
//!
//! The menu knows nothing about the app. [`show`] draws it and returns the
//! [`Command`] the user picked, and the caller runs it. [`key_command`] maps the
//! key equivalents (Ctrl on Linux and Windows, Cmd on macOS) to the same
//! commands. Everything is painted with painter shapes and text.

use egui::{
    pos2, vec2, Align2, Area, Color32, Context, FontFamily, FontId, Id, Key, LayerId, Order,
    Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Vec2,
};

use crate::theme::{BLACK, BOLD, DARK_GRAY, LIGHT_GRAY, WHITE};

/// Something the user can ask for from the menu or a key equivalent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    InfoPanel,
    /// File ▸ New: ask for a path and start an empty model there.
    NewModel,
    /// File ▸ Open…: ask for a store path or `postgres://` URL and open it.
    OpenModel,
    /// Show or hide the model browser window.
    ModelBrowser,
    /// File ▸ Save; on an in-memory model it asks for a path, as Save As.
    SaveModel,
    /// File ▸ Save As…: ask for a path, write the model there, switch to it.
    SaveAs,
    /// Reload the model from the store, dropping in-memory state (the way
    /// out of a save conflict: another writer changed the model).
    RevertToSaved,
    Undo,
    Redo,
    /// Edit ▸ Copy: the focused worksheet's selection to the clipboard as
    /// tab-separated text (Command-c, which egui delivers as a copy event).
    Copy,
    /// Edit ▸ Paste: the clipboard's cells into the worksheet at the cursor
    /// (Command-v).
    Paste,
    /// Edit ▸ Select All (Command-a, reserved by NeXTSTEP): every cell of
    /// the focused worksheet.
    SelectAll,
    PivotRotate,
    AddMatrix,
    RemoveMatrix,
    SaveView,
    /// Type a new category's name (the model browser's add line).
    AddCategory,
    /// Type a new item into the focused worksheet's first category.
    AddItem,
    /// Type a new input measure's name (the model browser's add line).
    AddInputMeasure,
    /// Rename the model browser's picked category, item or measure, in place.
    Rename,
    /// Ask to delete the picked category, item or measure.
    Delete,
    /// Open the n-th entry of the `views` list passed to [`show`].
    OpenView(usize),
    /// Worksheet ▸ Recalculate (Improv's Command-r): recompute every
    /// formula, as the formula pane's Calc button does.
    Recalculate,
    /// Graph ▸ New Graph: chart the focused worksheet's selection in the
    /// graph window.
    NewGraph,
    ToggleCsv,
    /// Show or hide the function help browser (also F1).
    FunctionHelp,
    /// Show or hide the list of keyboard shortcuts.
    Keys,
    /// Open the published user guide in the browser.
    UserGuide,
    /// Format ▸ General / N Decimals: the selected measure's decimals
    /// (`None`: General) (H1-22).
    Decimals(Option<u8>),
    /// Format ▸ Thousands Separators: turn the selected measure's grouping
    /// on or off (H1-22).
    Grouping,
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
    /// Opens the View submenu: these rows, then the saved views the
    /// caller passes to [`show`].
    Views(&'static [Leaf]),
    /// Runs a command directly (`None`: disabled).
    Leaf(Option<Command>),
}

use Command as C;

/// The menu tree: Lotus Improv 1.0's own, from its main-menu nib (titles,
/// order and key equivalents; `docs/reviews/2026-11-02-nextstep-fidelity.md`
/// §3.1), holding the commands this app has. What Improv did not have (Undo,
/// Format's decimals, the CSV wizard, Help) sits at the end of the menu it
/// fits. Rows Improv had that are not built here are left out rather than
/// drawn disabled, except Print… and Services, which NeXT puts in every
/// main menu.
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
            ("Open…", Some('o'), Some(C::OpenModel)),
            ("New", Some('n'), Some(C::NewModel)),
            ("Save", Some('s'), Some(C::SaveModel)),
            ("Save As…", Some('S'), Some(C::SaveAs)),
            ("Revert to Saved", None, Some(C::RevertToSaved)),
            ("Model Browser…", None, Some(C::ModelBrowser)),
            ("Import/Export CSV…", None, Some(C::ToggleCsv)),
        ]),
    ),
    (
        "Edit",
        None,
        Top::Sub(&[
            ("Undo", Some('z'), Some(C::Undo)),
            ("Redo", Some('Z'), Some(C::Redo)),
            ("Copy", Some('c'), Some(C::Copy)),
            ("Paste", Some('v'), Some(C::Paste)),
            ("Select All", Some('a'), Some(C::SelectAll)),
            ("Add Category", None, Some(C::AddCategory)),
            ("Add Item", None, Some(C::AddItem)),
            ("Add Input Measure", None, Some(C::AddInputMeasure)),
            ("Rename", None, Some(C::Rename)),
            ("Delete…", None, Some(C::Delete)),
        ]),
    ),
    (
        "Format",
        None,
        Top::Sub(&[
            ("General", None, Some(C::Decimals(None))),
            ("0 Decimals", None, Some(C::Decimals(Some(0)))),
            ("1 Decimal", None, Some(C::Decimals(Some(1)))),
            ("2 Decimals", None, Some(C::Decimals(Some(2)))),
            ("3 Decimals", None, Some(C::Decimals(Some(3)))),
            ("4 Decimals", None, Some(C::Decimals(Some(4)))),
            ("Thousands Separators", None, Some(C::Grouping)),
        ]),
    ),
    (
        "Worksheet",
        None,
        Top::Sub(&[
            ("Recalculate", Some('r'), Some(C::Recalculate)),
            ("Save View", None, Some(C::SaveView)),
        ]),
    ),
    (
        "View",
        None,
        Top::Views(&[
            ("New View", Some('V'), Some(C::AddMatrix)),
            ("Close Window", None, Some(C::RemoveMatrix)),
            ("Swap Rows/Columns", Some('/'), Some(C::PivotRotate)),
        ]),
    ),
    (
        "Graph",
        None,
        Top::Sub(&[("New Graph", None, Some(C::NewGraph))]),
    ),
    ("Print…", Some('p'), Top::Leaf(None)),
    ("Services", None, Top::Sub(&[])),
    (
        "Help",
        None,
        Top::Sub(&[
            ("Functions…", None, Some(C::FunctionHelp)),
            ("Keys…", None, Some(C::Keys)),
            ("User Guide", None, Some(C::UserGuide)),
        ]),
    ),
    ("Hide", Some('h'), Top::Leaf(Some(C::Hide))),
    ("Quit", Some('q'), Top::Leaf(Some(C::Quit))),
];

const TITLE: &str = "Improv";
const NO_VIEWS: &str = "No Views";

// Geometry, in points (spec §3, §4.1).
const BORDER: f32 = 1.0;
/// As a window's (NeXTSTEP 2: 21 px).
const TITLE_HEIGHT: f32 = 21.0;
const ITEM_HEIGHT: f32 = 20.0;
const PAD: f32 = 6.0;
/// Room between a label and the right-hand marker, plus the marker itself.
const RIGHT_SLOT: f32 = 28.0;
const MIN_WIDTH: f32 = 96.0;
/// Room the main menu takes from the workspace's left edge: its ~110 px panel
/// plus its margin. A fresh worksheet window opens clear of it.
pub(crate) const MENU_W: f32 = 120.0;
const FONT_SIZE: f32 = 12.0;

/// Where the menu sits and which submenu is open. Lives on the app.
#[derive(Clone, Copy, Debug, Default)]
pub struct MenuState {
    /// Index into the top level of the open submenu.
    open: Option<usize>,
    /// Top-left corner of the menu; dragging the title bar moves it. `None`
    /// until first shown, which places it at [`show`]'s `home`.
    pos: Option<Pos2>,
}

/// Map a key equivalent (pressed with Ctrl/Cmd) to its command, read off the
/// [`MENU`] tree so the two can't disagree: a lowercase letter is the plain
/// chord, an uppercase one the chord with Shift. Y is a Redo alias.
pub fn key_command(key: Key, shift: bool) -> Option<Command> {
    if key == Key::Y {
        return Some(C::Redo);
    }
    // egui turns these chords into its own copy and paste events (with the
    // clipboard's text), which the worksheet takes; a key event for them
    // must not run the commands a second time.
    if !shift && matches!(key, Key::C | Key::V) {
        return None;
    }
    keyed_items()
        .find(|(k, _)| Key::from_name(&k.to_string()) == Some(key) && k.is_uppercase() == shift)
        .and_then(|(_, c)| c)
}

/// Every menu item with a key equivalent, top level and submenus.
fn keyed_items() -> impl Iterator<Item = (char, Option<Command>)> {
    key_equivalents().map(|(k, _, c)| (k, c))
}

/// Every menu item with a key equivalent, as `(key, "Menu: Item", command)`
/// (a top-level item is just its label): what Help ▸ Keys… lists. (The
/// docs' `▸` is not in the GUI's fonts.)
pub fn key_equivalents() -> impl Iterator<Item = (char, String, Option<Command>)> {
    MENU.iter().flat_map(|&(menu, key, ref top)| {
        let leaves: &[Leaf] = match top {
            Top::Sub(l) | Top::Views(l) => l,
            Top::Leaf(_) => &[],
        };
        let top = match top {
            Top::Leaf(c) => key.map(|k| (k, menu.to_owned(), *c)),
            _ => None,
        };
        top.into_iter().chain(
            leaves
                .iter()
                .filter_map(move |&(item, k, c)| k.map(|k| (k, format!("{menu}: {item}"), c))),
        )
    })
}

/// Draw the menu and return the command picked this frame, if any. `views`
/// fills the View submenu as `(name, is_current)`; picking the n-th returns
/// [`Command::OpenView`]`(n)`. `enabled` greys out commands that can't run now,
/// and `checked` puts a check on those in effect (the selected measure's
/// format). The menu's top-left starts at `home`; after that it stays where
/// it's dragged.
pub fn show(
    ctx: &Context,
    state: &mut MenuState,
    home: Pos2,
    views: &[(String, bool)],
    enabled: impl Fn(Command) -> bool,
    checked: impl Fn(Command) -> bool,
) -> Option<Command> {
    let id = menu_id();
    let mut pos = state.pos.unwrap_or(home);
    // Apply this frame's title drag before placing the menu, so it tracks the
    // pointer with no frame delay (as nxwin does).
    if let Some(r) = ctx.read_response(id.with("title")) {
        pos += r.drag_delta();
    }
    let screen = ctx.content_rect();
    let pos = pos.clamp(
        screen.min,
        (screen.max - vec2(MIN_WIDTH, TITLE_HEIGHT)).max(screen.min),
    );
    state.pos = Some(pos);

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
                Top::Views(_) => true,
                Top::Leaf(c) => c.is_some_and(&enabled),
            },
            open: state.open == Some(i),
        })
        .collect();
    let (menu_rect, items) = panel(ctx, id, pos, TITLE, &rows, Sense::drag());

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
            Top::Sub(l) => l
                .iter()
                .map(|&(a, k, c)| (a, k, c, c.is_some_and(&checked)))
                .collect(),
            Top::Views(l) => {
                let fixed = l.iter().map(|&(a, k, c)| (a, k, c, false));
                let list = views
                    .iter()
                    .enumerate()
                    .map(|(n, (name, cur))| (name.as_str(), None, Some(C::OpenView(n)), *cur));
                if views.is_empty() {
                    fixed.chain([(NO_VIEWS, None, None, false)]).collect()
                } else {
                    fixed.chain(list).collect()
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
        // Cascade from the right edge, title level with the item; from the
        // left edge when that runs off screen, and lifted to stay on it.
        let size = panel_size(ctx, MENU[i].0, &rows);
        let x = if menu_rect.right() + size.x > screen.right() {
            menu_rect.left() - size.x
        } else {
            menu_rect.right()
        };
        let y = (items[i].rect.top() - BORDER)
            .min(screen.bottom() - size.y)
            .max(screen.top());
        let at = pos2(x, y);
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

/// The menu panel's area id.
pub fn menu_id() -> Id {
    Id::unique("improv_menu")
}

fn sub_id(i: usize) -> Id {
    menu_id().with(("sub", i))
}

/// The area id of top-level item `i`'s submenu (a row is `.with(n)`).
#[cfg(test)]
pub fn sub_id_pub(i: usize) -> Id {
    sub_id(i)
}

/// How many rows the top level has (each takes the keyboard).
#[cfg(test)]
pub fn focusable_rows() -> usize {
    MENU.len()
}

/// The top-level index of `label`.
#[cfg(test)]
pub fn top_index(label: &str) -> usize {
    MENU.iter()
        .position(|m| m.0 == label)
        .expect("a top-level item")
}

/// The row of `item` in top-level `menu`'s submenu.
#[cfg(test)]
pub fn sub_row(menu: &str, item: &str) -> usize {
    match MENU[top_index(menu)].2 {
        Top::Sub(l) | Top::Views(l) => l.iter().position(|r| r.0 == item).expect("a submenu row"),
        _ => panic!("{menu} has no submenu"),
    }
}

/// The open submenu's top-level index.
#[cfg(test)]
pub fn open_submenu(s: &MenuState) -> Option<usize> {
    s.open
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

/// Outer size of a menu panel titled `title` holding `rows`.
fn panel_size(ctx: &Context, title: &str, rows: &[Row]) -> Vec2 {
    let text_width = |s: &str, f: FontId| {
        ctx.fonts_mut(|fonts| fonts.layout_no_wrap(s.to_owned(), f, BLACK).size().x)
    };
    let width = rows
        .iter()
        .map(|r| text_width(r.label, font()) + RIGHT_SLOT)
        .chain([text_width(title, bold())])
        .fold(MIN_WIDTH, f32::max)
        + 2.0 * (PAD + BORDER);
    let height = 2.0 * BORDER + TITLE_HEIGHT + ITEM_HEIGHT * rows.len() as f32;
    vec2(width.ceil(), height)
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
    let outer = Rect::from_min_size(pos, panel_size(ctx, title, rows));
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
            ui.interact(title_rect, id.with("title"), title_sense)
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, title));

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
                    // H2: painted rows tell AccessKit what they are.
                    resp.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            if row.sub {
                                egui::Role::ComboBox
                            } else {
                                egui::Role::Button
                            },
                            row.enabled,
                            row.label,
                        )
                    });
                    let lit = row.open || (row.enabled && resp.is_pointer_button_down_on());
                    paint_row(p, r, row, if lit { WHITE } else { LIGHT_GRAY });
                    resp
                })
                .collect();

            // NeXTSTEP 2: "menus have a dark gray border just on their top
            // and left sides; the border on the bottom and right is drawn
            // within the content area as part of the menu commands" (each
            // row's black bottom-right bevel edge, and the title's black).
            let edge = Stroke::new(BORDER, DARK_GRAY);
            let o = outer.shrink(BORDER / 2.0);
            p.line_segment([o.left_bottom(), o.left_top()], edge);
            p.line_segment([o.left_top(), o.right_top()], edge);
            let ink = Stroke::new(BORDER, BLACK);
            p.line_segment([o.right_top(), o.right_bottom()], ink);
            p.line_segment([o.left_bottom(), o.right_bottom()], ink);
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
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::TestFrames as _;
    use egui::{Event, Modifiers, PointerButton, RawInput};

    /// Headless frames of just the menu, with three views ("b" current) and
    /// Close Window (View) disabled.
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
            let _ = self.ctx.test_pass(raw, |ctx| {
                picked = show(
                    ctx,
                    state,
                    pos2(4.0, 4.0),
                    &views,
                    |c| c != C::RemoveMatrix && c != C::Decimals(Some(4)),
                    |c| c == C::Decimals(Some(2)),
                );
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
        assert_eq!(h.click_row(sub_id(view), 5), Some(C::OpenView(2)));
    }

    /// H1-22: Format sets the selected measure's decimals and grouping; a
    /// command that cannot run now draws disabled, and the format in effect
    /// is checked.
    #[test]
    fn format_submenu_picks_decimals_and_grouping() {
        let mut h = Harness::new();
        let f = top("Format");
        h.click_row(menu_id(), f);
        assert_eq!(h.state.open, Some(f), "Format is built now");
        assert_eq!(h.click_row(sub_id(f), 0), Some(C::Decimals(None)));
        for (row, d) in [(1, 0), (2, 1), (3, 2), (4, 3)] {
            h.click_row(menu_id(), f);
            assert_eq!(h.click_row(sub_id(f), row), Some(C::Decimals(Some(d))));
        }
        h.click_row(menu_id(), f);
        assert_eq!(h.click_row(sub_id(f), 5), None, "disabled here");
        assert_eq!(h.click_row(sub_id(f), 6), Some(C::Grouping));
        assert_eq!(sub_row("Format", "2 Decimals"), 3);
    }

    /// Improv's View menu leads with New View, then Swap Rows/Columns; the
    /// saved views follow. File ▸ Model Browser… shows the browser.
    #[test]
    fn view_and_file_rows_are_improvs() {
        let mut h = Harness::new();
        let view = top("View");
        h.click_row(menu_id(), view);
        assert_eq!(h.click_row(sub_id(view), 0), Some(C::AddMatrix));
        h.click_row(menu_id(), view);
        assert_eq!(h.click_row(sub_id(view), 2), Some(C::PivotRotate));
        let file = top("File");
        h.click_row(menu_id(), file);
        let mb = sub_row("File", "Model Browser…");
        assert_eq!(h.click_row(sub_id(file), mb), Some(C::ModelBrowser));
        let ws = top("Worksheet");
        h.click_row(menu_id(), ws);
        assert_eq!(h.click_row(sub_id(ws), 0), Some(C::Recalculate));
    }

    /// NeXTSTEP 2's menu: a 21 px black title bar, a dark-gray border on the
    /// top and left sides only (the bottom and right are the rows' black
    /// bevel edges).
    #[test]
    fn the_menu_border_is_dark_gray_on_top_and_left() {
        let ctx = Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = MenuState::default();
        let mut shapes = Vec::new();
        for t in 0..3 {
            let raw = RawInput {
                time: Some(f64::from(t) / 60.0),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                ..Default::default()
            };
            shapes = ctx
                .test_pass(raw, |ctx| {
                    show(ctx, &mut state, pos2(4.0, 4.0), &[], |_| true, |_| false);
                })
                .shapes;
        }
        let menu = ctx.memory(|m| m.area_rect(menu_id())).unwrap();
        let o = menu.shrink(BORDER / 2.0);
        let seg = |a: Pos2, b: Pos2| {
            shapes.iter().find_map(|c| match &c.shape {
                Shape::LineSegment { points, stroke } if *points == [a, b] => Some(stroke.color),
                _ => None,
            })
        };
        assert_eq!(seg(o.left_bottom(), o.left_top()), Some(DARK_GRAY), "left");
        assert_eq!(seg(o.left_top(), o.right_top()), Some(DARK_GRAY), "top");
        assert_eq!(seg(o.right_top(), o.right_bottom()), Some(BLACK), "right");
        let title = shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Rect(r) if r.fill == BLACK && r.rect.min == menu.min + vec2(1.0, 1.0) => {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("the black title bar");
        assert_eq!(title.height(), 21.0);
    }

    /// The top level is Improv 1.0's, in its order (from its main-menu nib),
    /// plus Help.
    #[test]
    fn the_top_level_is_improvs() {
        let labels: Vec<&str> = MENU.iter().map(|m| m.0).collect();
        assert_eq!(
            labels,
            [
                "Info",
                "File",
                "Edit",
                "Format",
                "Worksheet",
                "View",
                "Graph",
                "Print…",
                "Services",
                "Help",
                "Hide",
                "Quit"
            ]
        );
    }

    #[test]
    fn disabled_rows_return_nothing() {
        let mut h = Harness::new();
        assert_eq!(h.click_row(menu_id(), top("Print…")), None);
        assert_eq!(h.click_row(menu_id(), top("Services")), None);
        assert_eq!(h.state.open, None, "an empty submenu doesn't open");

        let view = top("View");
        h.click_row(menu_id(), view);
        assert_eq!(
            h.click_row(sub_id(view), 1),
            None,
            "Close Window is disabled"
        );
        assert_eq!(h.state.open, Some(view), "a disabled click leaves it open");
        let file = top("File");
        h.click_row(menu_id(), file);
        assert_eq!(h.click_row(sub_id(file), 3), Some(C::SaveAs));
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
        assert_eq!(h.state.pos, Some(pos2(104.0, 54.0)));
    }

    /// Dragged to the screen's bottom-right corner, the menu opens Worksheet
    /// to its left and lifts it, keeping the submenu on screen.
    #[test]
    fn submenus_stay_on_screen() {
        let mut h = Harness::new();
        let menu = h.rect(menu_id());
        h.state.pos = Some(pos2(800.0 - menu.width(), 450.0));
        h.frame(Vec::new());
        let ws = top("Worksheet");
        h.click_row(menu_id(), ws);
        assert_eq!(h.state.open, Some(ws));
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0));
        let sub = h.rect(sub_id(ws));
        assert!(screen.contains_rect(sub), "{sub:?} off {screen:?}");
        assert!(sub.right() <= h.rect(menu_id()).left() + 1.0, "{sub:?}");
    }

    /// The key table is the menu: every keyed item maps back to itself.
    #[test]
    fn key_command_agrees_with_every_keyed_menu_item() {
        let mut n = 0;
        for (k, c) in keyed_items() {
            let key = Key::from_name(&k.to_string()).expect("a key letter");
            // Copy and Paste arrive as egui's clipboard events instead.
            let want = if matches!(k, 'c' | 'v') { None } else { c };
            assert_eq!(key_command(key, k.is_uppercase()), want, "{k:?}");
            n += 1;
        }
        assert_eq!(n, 15, "o n s S z Z c v a r V / p h q");
    }

    #[test]
    fn key_equivalents_map_to_commands() {
        assert_eq!(key_command(Key::S, false), Some(C::SaveModel));
        assert_eq!(key_command(Key::Z, false), Some(C::Undo));
        assert_eq!(key_command(Key::Z, true), Some(C::Redo));
        assert_eq!(key_command(Key::Y, false), Some(C::Redo));
        assert_eq!(key_command(Key::Q, false), Some(C::Quit));
        assert_eq!(key_command(Key::H, false), Some(C::Hide));
        assert_eq!(key_command(Key::N, false), Some(C::NewModel));
        assert_eq!(key_command(Key::O, false), Some(C::OpenModel));
        // Improv's own: Save As, Recalculate, New View, Swap Rows/Columns.
        assert_eq!(key_command(Key::S, true), Some(C::SaveAs));
        assert_eq!(key_command(Key::R, false), Some(C::Recalculate));
        assert_eq!(key_command(Key::V, true), Some(C::AddMatrix));
        assert_eq!(key_command(Key::Slash, false), Some(C::PivotRotate));
        assert_eq!(
            key_command(Key::V, false),
            None,
            "Paste comes as egui's event"
        );
        assert_eq!(key_command(Key::C, false), None, "so does Copy");
        assert_eq!(key_command(Key::P, false), None, "Print is disabled");
        assert_eq!(key_command(Key::A, false), Some(C::SelectAll));
        assert_eq!(key_command(Key::B, false), None);
    }

    /// Every label and key letter renders from the installed fonts, not as the
    /// missing-glyph box. Replaces the old tool-palette glyph check.
    #[test]
    fn menu_glyphs_are_in_the_installed_fonts() {
        let h = Harness::new();
        let mut texts: Vec<String> = vec![NO_VIEWS.into()];
        for (label, key, top) in MENU {
            texts.push((*label).into());
            texts.extend(key.map(String::from));
            if let Top::Sub(leaves) | Top::Views(leaves) = top {
                for (label, key, _) in *leaves {
                    texts.push((*label).into());
                    texts.extend(key.map(String::from));
                }
            }
        }
        h.ctx.fonts_mut(|f| {
            assert!(f.has_glyphs(&bold(), TITLE));
            for t in &texts {
                assert!(f.has_glyphs(&font(), t), "missing glyph in {t:?}");
            }
        });
    }

    /// With no saved views the View submenu lists its own rows then a
    /// disabled "No Views" placeholder.
    #[test]
    fn view_submenu_without_views_shows_the_placeholder() {
        let ctx = Context::default();
        crate::theme::install_fonts(&ctx);
        let mut state = MenuState {
            open: Some(top("View")),
            pos: None,
        };
        let mut texts = Vec::new();
        for t in 0..3 {
            let raw = RawInput {
                time: Some(f64::from(t) / 60.0),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))),
                ..Default::default()
            };
            let out = ctx.test_pass(raw, |ctx| {
                let picked = show(ctx, &mut state, pos2(4.0, 4.0), &[], |_| true, |_| false);
                assert_eq!(picked, None);
            });
            texts = out
                .shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        assert!(texts.iter().any(|t| t == NO_VIEWS), "{texts:?}");
        assert!(texts.iter().any(|t| t == "New View"), "{texts:?}");
    }
}
