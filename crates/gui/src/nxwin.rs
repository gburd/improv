//! NeXTSTEP window chrome for in-app floating windows (spec §3, §4.3, §5).
//!
//! [`NxWindow`] wraps an [`egui::Window`] that has no egui title bar or frame,
//! and paints the NeXT chrome itself, to the NeXTSTEP 2 guidelines
//! (*NEXTstep Concepts*, Dec 1990, ch. 2; fidelity note §3.3): a 1 px black
//! border, a 21 px title bar (black when key, dark gray otherwise), a beveled
//! close box, an 8 px resize bar (a dark-gray row, a white row, six
//! light-gray rows) notched near each end, where it moves a corner, and a
//! light-gray body. Every piece is a painter shape; the only glyphs are the
//! title text.
//!
//! The *key* window is the most recently clicked `NxWindow`, tracked in egui
//! memory; see [`is_key`]. Closing the key window hands the key to the next
//! window shown.

use crate::theme::{BLACK, DARK_GRAY, LIGHT_GRAY, WHITE};
use egui::{
    pos2, vec2, Align2, Color32, Context, FontFamily, FontId, Frame, Id, InnerResponse, LayerId,
    Order, Painter, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2,
};

// Geometry, in points (NeXTSTEP 2: "The title bar is 21 pixels high";
// "the resize bar is eight pixels high").
const BORDER: f32 = 1.0;
const TITLE_HEIGHT: f32 = 21.0;
const CLOSE_SIZE: f32 = 15.0;
const CLOSE_INSET: f32 = 3.0;
const RESIZE_HEIGHT: f32 = 8.0;
/// How far in from each end of the resize bar its corner notch sits (about
/// the AppKit's, measured off the guidelines' Figure 2-9), at most a third
/// of the bar.
const CORNER_ZONE: f32 = 30.0;

/// The resize bar's corner zone width for a bar `width` wide.
fn corner_zone(width: f32) -> f32 {
    CORNER_ZONE.min(width / 3.0)
}
const MIN_SIZE: Vec2 = vec2(100.0, 60.0);
const DEFAULT_RECT: Rect = Rect {
    min: pos2(32.0, 32.0),
    max: pos2(352.0, 272.0),
};

/// A window's status, which its title bar shows (NeXTSTEP 2 guidelines,
/// "The Key Window" and "The Main Window").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Takes the keyboard: black title bar.
    Key,
    /// The standard window the user works in while a panel has the key:
    /// "the Application Kit highlights its title bar in dark gray".
    Main,
    /// Any other window: light gray.
    Other,
}

/// Title bar fill and text colors for a window's [`Status`]: black with
/// white text, dark gray with white text, light gray with black text.
pub fn title_colors(status: Status) -> (Color32, Color32) {
    match status {
        Status::Key => (BLACK, WHITE),
        Status::Main => (DARK_GRAY, WHITE),
        Status::Other => (LIGHT_GRAY, BLACK),
    }
}

/// Is the window with this id the main window: the standard window last
/// made key (a panel taking the key leaves it main)?
pub fn is_main(ctx: &Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<Id>(main_slot())) == Some(id)
}

/// Memory slot holding the [`Id`] of the main window.
fn main_slot() -> Id {
    Id::unique("nxwin::main")
}

/// Is the window with this id the key window (the most recently clicked
/// [`NxWindow`])? The id is [`NxWindow::id`].
pub fn is_key(ctx: &Context, id: Id) -> bool {
    ctx.data(|d| d.get_temp::<Id>(key_slot())) == Some(id)
}

/// Make the window with this id the key window, as a click on it would.
pub fn make_key(ctx: &Context, id: Id) {
    ctx.data_mut(|d| d.insert_temp(key_slot(), id));
}

/// Memory slot holding the [`Id`] of the key window.
fn key_slot() -> Id {
    Id::unique("nxwin::key")
}

/// The outer rect the window with this id ([`NxWindow::id`]) ended its last
/// [`NxWindow::show`] with: drags, resizes and clamps applied. `None` before
/// it is first shown.
pub fn stored_rect(ctx: &Context, id: Id) -> Option<Rect> {
    ctx.data(|d| d.get_temp::<Rect>(rect_slot(id)))
}

/// Memory slot holding a window's outer rect between frames.
fn rect_slot(id: Id) -> Id {
    id.with("nxwin_rect")
}

/// A floating window with NeXTSTEP chrome. Build with [`NxWindow::new`],
/// configure, then call [`NxWindow::show`] every frame.
#[must_use = "call .show(ctx, ...)"]
pub struct NxWindow<'open> {
    id: Id,
    title: String,
    open: Option<&'open mut bool>,
    default_rect: Rect,
    rect: Option<Rect>,
    min_size: Vec2,
    bounds: Option<Rect>,
    resizable: bool,
    standard: bool,
    edited: bool,
    miniaturize: Option<&'open mut bool>,
}

impl<'open> NxWindow<'open> {
    /// A window identified by `id_source` (salted, see [`Self::id`]), showing
    /// `title` centred in its title bar. Resizable by default, not closable.
    pub fn new(
        id_source: impl std::hash::Hash + std::fmt::Debug,
        title: impl Into<String>,
    ) -> Self {
        Self {
            id: Id::unique(("nxwin", id_source)),
            title: title.into(),
            open: None,
            default_rect: DEFAULT_RECT,
            rect: None,
            min_size: MIN_SIZE,
            bounds: None,
            resizable: true,
            standard: false,
            edited: false,
            miniaturize: None,
        }
    }

    /// A standard window (a document's: worksheet, graph), not a panel: it
    /// becomes the main window when it takes the key, and stays main, its
    /// title bar dark gray, while a panel has the key.
    pub fn standard(mut self) -> Self {
        self.standard = true;
        self
    }

    /// Draw the close box broken, NeXT's mark for a document "that the user
    /// has edited but hasn't saved".
    pub fn edited(mut self, edited: bool) -> Self {
        self.edited = edited;
        self
    }

    /// Show the miniaturize button at the left of the title bar; a click
    /// sets `clicked`.
    pub fn miniaturizable(mut self, clicked: &'open mut bool) -> Self {
        self.miniaturize = Some(clicked);
        self
    }

    /// The window's [`Id`]: its egui area id, and the id [`is_key`] takes.
    pub fn id(&self) -> Id {
        self.id
    }

    /// Show a close box; clicking it sets `open` to `false`. While `open` is
    /// `false` the window is not shown.
    pub fn closable(mut self, open: &'open mut bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Outer rect (border included) the first time the window is shown.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "the app's windows all own their rect (.rect)")
    )]
    pub fn default_rect(mut self, rect: Rect) -> Self {
        self.default_rect = rect;
        self
    }

    /// Outer rect for this frame, overriding the one stored from the last
    /// frame; this frame's drags apply on top of it. For callers that own the
    /// rect: feed back [`stored_rect`] after [`Self::show`].
    pub fn rect(mut self, rect: Rect) -> Self {
        self.rect = Some(rect);
        self
    }

    /// Smallest outer size: resizes stop here, and a smaller incoming rect is
    /// enlarged (keeping its top-left). Defaults to 100 x 60.
    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = min_size;
        self
    }

    /// Rect the window is kept inside, both when moved and when resized.
    /// Defaults to [`Context::content_rect`].
    pub fn bounds(mut self, bounds: Rect) -> Self {
        self.bounds = Some(bounds);
        self
    }

    /// Whether the bottom resize bar is shown and resizes the window.
    pub fn resizable(mut self, resizable: bool) -> Self {
        self.resizable = resizable;
        self
    }

    /// Show the window, laying `add_contents` out in the body. Returns `None`
    /// when the window is closed.
    pub fn show<R>(
        self,
        ctx: &Context,
        add_contents: impl FnOnce(&mut Ui) -> R,
    ) -> Option<InnerResponse<R>> {
        let Self {
            id,
            title,
            open,
            default_rect,
            rect,
            min_size,
            bounds,
            resizable,
            standard,
            edited,
            miniaturize,
        } = self;
        if matches!(open, Some(false)) {
            return None;
        }
        let closable = open.is_some();
        let mut rect = rect
            .or_else(|| stored_rect(ctx, id))
            .unwrap_or(default_rect);
        rect.max = rect.min + rect.size().max(min_size);

        // Apply this frame's drags before placing the window, as egui's own
        // `Area` does, so the chrome tracks the pointer with no frame delay.
        if let Some(r) = ctx.read_response(id.with("title")) {
            rect = rect.translate(r.drag_delta());
        }
        let screen = bounds.unwrap_or_else(|| ctx.content_rect());
        if let Some(r) = ctx
            .read_response(id.with("nx_resize"))
            .filter(|r| r.dragged())
        {
            // NeXT: left of the left notch drags the bottom-left corner,
            // right of the right notch the bottom-right corner, between them
            // the bottom edge. The zone is latched at press, since the bar
            // moves under the pointer.
            let zone_id = id.with("resize_zone");
            let zone = if r.drag_started() {
                let x = r
                    .interact_pointer_pos()
                    .map_or(0.0, |p| p.x - r.rect.left());
                let corner = corner_zone(r.rect.width());
                let zone = if x < corner {
                    0
                } else if x < r.rect.width() - corner {
                    1
                } else {
                    2
                };
                ctx.data_mut(|d| d.insert_temp(zone_id, zone));
                zone
            } else {
                ctx.data(|d| d.get_temp::<u8>(zone_id)).unwrap_or(1)
            };
            // Clamp the dragged edge to the bounds (never `f32::clamp`, which
            // panics when the bounds are smaller than `min_size`).
            let d = r.drag_delta();
            match zone {
                0 => {
                    rect.min.x = (rect.min.x + d.x)
                        .max(screen.min.x)
                        .min(rect.max.x - min_size.x);
                }
                2 => {
                    rect.max.x = (rect.max.x + d.x)
                        .min(screen.max.x)
                        .max(rect.min.x + min_size.x);
                }
                _ => {}
            }
            rect.max.y = (rect.max.y + d.y)
                .min(screen.max.y)
                .max(rect.min.y + min_size.y);
        }

        // Keep the window in bounds. Done here rather than by egui, whose first
        // (sizing) pass clamps against a guessed 600x400 size.
        let max = (screen.max - rect.size()).max(screen.min);
        rect = rect.translate(rect.min.clamp(screen.min, max) - rect.min);
        ctx.data_mut(|d| d.insert_temp(rect_slot(id), rect));

        // A press on this window's layer makes it key; the first window shown
        // is key until something is clicked.
        // ponytail: a window shown before the clicked one keeps the key look
        // for that one frame; a window registry would fix it if it shows.
        let layer = LayerId::new(Order::Middle, id);
        let pressed_here = ctx
            .input(|i| i.pointer.interact_pos().filter(|_| i.pointer.any_pressed()))
            .is_some_and(|p| ctx.layer_id_at(p) == Some(layer));
        ctx.data_mut(|d| {
            if pressed_here || d.get_temp::<Id>(key_slot()).is_none() {
                d.insert_temp(key_slot(), id);
            }
            if standard && d.get_temp::<Id>(key_slot()) == Some(id) {
                d.insert_temp(main_slot(), id);
            }
        });
        let status = if is_key(ctx, id) {
            Status::Key
        } else if standard && is_main(ctx, id) {
            Status::Main
        } else {
            Status::Other
        };

        let mut close_clicked = false;
        let mut mini_clicked = false;
        let has_mini = miniaturize.is_some();
        let shown = egui::Window::new(title.as_str())
            .id(id)
            .title_bar(false)
            .frame(Frame::NONE)
            .movable(false)
            .constrain(false)
            .resizable(false)
            .collapsible(false)
            .fade_in(false)
            .fade_out(false)
            .current_pos(rect.min)
            .fixed_size(rect.size())
            .show(ctx, |ui| {
                let outer = ui.max_rect();
                let l = layout(outer, closable, resizable, has_mini);
                let p = ui.painter().clone();
                let (fill, text) = title_colors(status);

                p.rect_filled(l.body, 0.0, LIGHT_GRAY);
                p.rect_filled(l.title, 0.0, fill);
                let title_painter =
                    p.with_clip_rect(l.title.shrink2(vec2(CLOSE_INSET * 2.0 + CLOSE_SIZE, 0.0)));
                title_painter.text(
                    l.title.center(),
                    Align2::CENTER_CENTER,
                    &title,
                    FontId::new(12.0, FontFamily::Name(crate::theme::BOLD.into())),
                    text,
                );
                // The drag is applied before layout, above; this registers it.
                ui.interact(l.title, id.with("title"), Sense::drag())
                    .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Label, true, &title));
                // H2: egui's whole-window widget (it brings a clicked window
                // to the front) is the window a screen reader announces.
                ctx.accesskit_node_builder(id.with("move"), |b| {
                    b.set_role(egui::accesskit::Role::Window);
                    b.set_label(title.as_str());
                });

                // Registered after the title so it sits on top for hit-testing.
                if let Some(close) = l.close {
                    bevel(&p, close);
                    close_glyph(&p, close.shrink(4.0), edited);
                    let r = ui.interact(close, id.with("close"), Sense::click());
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            true,
                            if edited {
                                format!("Close {title} (edited)")
                            } else {
                                format!("Close {title}")
                            },
                        )
                    });
                    close_clicked = r.clicked();
                }
                if let Some(mini) = l.mini {
                    bevel(&p, mini);
                    // NeXT's miniaturize glyph: a small outlined square.
                    let g = mini.shrink(4.0);
                    p.rect_stroke(
                        g,
                        0.0,
                        Stroke::new(1.0_f32, BLACK),
                        egui::StrokeKind::Inside,
                    );
                    // Pointer only, never a Tab stop: Enter on it would
                    // minimize the app; the keyboard has Hide (Command-h).
                    let r = ui.interact(mini, id.with("mini"), Sense::click() - Sense::FOCUSABLE);
                    r.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::Button,
                            true,
                            format!("Miniaturize {title}"),
                        )
                    });
                    mini_clicked = r.clicked();
                }

                if let Some(bar) = l.resize {
                    resize_bar(&p, bar);
                    let pointer_only = Sense::DRAG;
                    let _ = ui.interact(bar, id.with("nx_resize"), pointer_only);
                }

                let mut body = ui.new_child(UiBuilder::new().max_rect(l.body));
                body.set_clip_rect(l.body.intersect(ui.clip_rect()));
                let inner = add_contents(&mut body);

                p.rect_stroke(
                    outer.shrink(BORDER / 2.0),
                    0.0,
                    Stroke::new(BORDER, BLACK),
                    egui::StrokeKind::Middle,
                );
                // Size the window to the chrome, not to whatever the body holds.
                ui.advance_cursor_after_rect(outer);
                inner
            })?;

        if let (true, Some(m)) = (mini_clicked, miniaturize) {
            *m = true;
        }
        if let (true, Some(open)) = (close_clicked, open) {
            *open = false;
            // Hand the key to the next window shown.
            if is_key(ctx, id) {
                ctx.data_mut(|d| d.remove::<Id>(key_slot()));
            }
        }
        Some(InnerResponse {
            inner: shown.inner?,
            response: shown.response,
        })
    }
}

/// Where each piece of chrome sits inside a window's outer rect.
struct Layout {
    title: Rect,
    close: Option<Rect>,
    mini: Option<Rect>,
    body: Rect,
    resize: Option<Rect>,
}

fn layout(outer: Rect, closable: bool, resizable: bool, mini: bool) -> Layout {
    let inner = outer.shrink(BORDER);
    let title = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_HEIGHT));
    let button = |x: f32| {
        Rect::from_min_size(
            pos2(x, title.center().y - CLOSE_SIZE / 2.0),
            Vec2::splat(CLOSE_SIZE),
        )
    };
    let close = closable.then(|| button(title.right() - CLOSE_INSET - CLOSE_SIZE));
    // Miniaturize at the left, close at the right (NeXTSTEP 2, Figure 2-16).
    let mini = mini.then(|| button(title.left() + CLOSE_INSET));
    let resize = resizable.then(|| {
        Rect::from_min_max(
            pos2(inner.left(), inner.bottom() - RESIZE_HEIGHT),
            inner.max,
        )
    });
    let body = Rect::from_min_max(
        pos2(inner.left(), title.bottom()),
        pos2(inner.right(), resize.map_or(inner.bottom(), |r| r.top())),
    );
    Layout {
        title,
        close,
        mini,
        body,
        resize,
    }
}

/// The close box's X in `g`; broken (its middle left out) while the
/// window's document is edited and not saved.
fn close_glyph(p: &Painter, g: Rect, broken: bool) {
    let stroke = Stroke::new(1.0_f32, BLACK);
    let diagonals = [
        (g.left_top(), g.right_bottom()),
        (g.right_top(), g.left_bottom()),
    ];
    for (a, b) in diagonals {
        if broken {
            // A gap of 40% of the stroke at the crossing.
            let (m1, m2) = (a.lerp(b, 0.3), a.lerp(b, 0.7));
            p.line_segment([a, m1], stroke);
            p.line_segment([m2, b], stroke);
        } else {
            p.line_segment([a, b], stroke);
        }
    }
}

/// Paint the NeXTSTEP 2 resize bar over `r`: "The top row of pixels is
/// dark gray, the row beneath is white, and the remaining six rows are light
/// gray", with a notch (a black line, a white one right of it) where each
/// corner zone ends.
fn resize_bar(p: &Painter, r: Rect) {
    let row = |y: f32, h: f32, c: Color32| {
        p.rect_filled(
            Rect::from_x_y_ranges(r.x_range(), r.top() + y..=r.top() + y + h),
            0.0,
            c,
        );
    };
    row(0.0, 1.0, DARK_GRAY);
    row(1.0, 1.0, WHITE);
    row(2.0, r.height() - 2.0, LIGHT_GRAY);
    let corner = corner_zone(r.width());
    for x in [r.left() + corner, r.right() - corner - 2.0] {
        let x = x.round();
        let notch = r.top() + 1.0..=r.bottom();
        p.rect_filled(
            Rect::from_x_y_ranges(x..=x + 1.0, notch.clone()),
            0.0,
            BLACK,
        );
        p.rect_filled(Rect::from_x_y_ranges(x + 1.0..=x + 2.0, notch), 0.0, WHITE);
    }
}

/// Paint a NeXT raised bevel over `r`: light-gray face, white top-left edge,
/// dark gray then black bottom-right edge (spec §1).
fn bevel(p: &Painter, r: Rect) {
    let edge = |min: Pos2, max: Pos2, c: Color32| {
        p.rect_filled(Rect::from_min_max(min, max), 0.0, c);
    };
    p.rect_filled(r, 0.0, LIGHT_GRAY);
    edge(r.min, pos2(r.max.x, r.min.y + 1.0), WHITE);
    edge(r.min, pos2(r.min.x + 1.0, r.max.y), WHITE);
    edge(
        pos2(r.min.x + 1.0, r.max.y - 2.0),
        pos2(r.max.x - 1.0, r.max.y - 1.0),
        DARK_GRAY,
    );
    edge(
        pos2(r.max.x - 2.0, r.min.y + 1.0),
        pos2(r.max.x - 1.0, r.max.y - 1.0),
        DARK_GRAY,
    );
    edge(pos2(r.min.x, r.max.y - 1.0), r.max, BLACK);
    edge(pos2(r.max.x - 1.0, r.min.y), r.max, BLACK);
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[allow(unused_imports)]
    use crate::TestFrames as _;
    use egui::epaint::ClippedShape;
    use egui::{Event, Modifiers, PointerButton, RawInput, Shape};

    const SCREEN: Rect = Rect {
        min: Pos2::ZERO,
        max: pos2(800.0, 600.0),
    };

    /// The id [`NxWindow::new`] gives `id_source`.
    fn wid(id_source: &str) -> Id {
        NxWindow::new(id_source, "").id()
    }

    /// Headless frames at a fixed screen rect, like app.rs's `full_frame`.
    struct Harness {
        ctx: Context,
        time: f64,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = Context::default();
            crate::theme::install_fonts(&ctx);
            Self { ctx, time: 0.0 }
        }

        fn frame(&mut self, events: Vec<Event>, ui: impl FnMut(&Context)) -> Vec<ClippedShape> {
            self.time += 1.0 / 60.0;
            let raw = RawInput {
                time: Some(self.time),
                screen_rect: Some(SCREEN),
                events,
                ..Default::default()
            };
            self.ctx.test_pass(raw, ui).shapes
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

    fn click(at: Pos2) -> Vec<Event> {
        vec![Event::PointerMoved(at), button(at, true), button(at, false)]
    }

    /// Press at `from`, move to `to`, release, then settle: one frame each.
    fn drag(h: &mut Harness, from: Pos2, to: Pos2, mut ui: impl FnMut(&Context)) {
        h.frame(vec![Event::PointerMoved(from)], &mut ui);
        h.frame(vec![button(from, true)], &mut ui);
        h.frame(vec![Event::PointerMoved(to)], &mut ui);
        h.frame(vec![button(to, false)], &mut ui);
        h.frame(Vec::new(), &mut ui);
    }

    /// Fill of the rect shape painted exactly over `rect`.
    fn fill_at(shapes: &[ClippedShape], rect: Rect) -> Option<Color32> {
        shapes.iter().find_map(|c| match &c.shape {
            Shape::Rect(r) if r.rect == rect => Some(r.fill),
            _ => None,
        })
    }

    /// Color of the text shape reading `text`.
    fn text_color(shapes: &[ClippedShape], text: &str) -> Option<Color32> {
        shapes.iter().find_map(|c| match &c.shape {
            Shape::Text(t) if t.galley.text() == text => {
                Some(t.galley.job.sections[0].format.color)
            }
            _ => None,
        })
    }

    const R: Rect = Rect {
        min: pos2(100.0, 100.0),
        max: pos2(400.0, 300.0),
    };

    /// (a) The painted title bar is 21 px high (NeXTSTEP 2) and the body Ui
    /// starts below it.
    #[test]
    fn title_bar_is_21px_and_body_sits_below() {
        let mut h = Harness::new();
        let (mut outer, mut body) = (Rect::NOTHING, Rect::NOTHING);
        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = h.frame(Vec::new(), |ctx| {
                let ir = NxWindow::new("a", "Alpha")
                    .default_rect(R)
                    .show(ctx, |ui| ui.max_rect())
                    .unwrap();
                (outer, body) = (ir.response.rect, ir.inner);
            });
        }
        assert_eq!(outer, R);
        // The black fill starting just inside the 1 px border is the title.
        let title = shapes
            .iter()
            .find_map(|c| match &c.shape {
                Shape::Rect(r) if r.fill == BLACK && r.rect.min == R.min + vec2(1.0, 1.0) => {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("a black title bar inside the border");
        assert_eq!(title.width(), R.width() - 2.0);
        assert_eq!(title.height(), 21.0);
        assert_eq!(body.top(), title.bottom());
        assert_eq!((body.left(), body.right()), (title.left(), title.right()));
        assert_eq!(
            body.bottom(),
            R.bottom() - 1.0 - 8.0,
            "resize bar below the body"
        );
        // The bar's rows: dark gray, white, then light gray.
        let bar_top = body.bottom();
        let row = |y: f32| {
            shapes.iter().find_map(|c| match &c.shape {
                Shape::Rect(r) if r.rect.top() == y && r.rect.width() == R.width() - 2.0 => {
                    Some(r.fill)
                }
                _ => None,
            })
        };
        assert_eq!(row(bar_top), Some(DARK_GRAY));
        assert_eq!(row(bar_top + 1.0), Some(WHITE));
        assert_eq!(row(bar_top + 2.0), Some(LIGHT_GRAY));
    }

    /// (b) Clicking the close box clears the bool and hides the window.
    #[test]
    fn close_box_clears_open() {
        let mut h = Harness::new();
        let mut open = true;
        let mut shown = false;
        let mut ui = |ctx: &Context| {
            shown = NxWindow::new("a", "Alpha")
                .default_rect(R)
                .closable(&mut open)
                .show(ctx, |_| ())
                .is_some();
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let close = layout(R, true, true, false).close.unwrap();
        assert_eq!(close.size(), vec2(15.0, 15.0));
        // Centred in the 21 px bar: 3 px above and below the 15 px box.
        assert_eq!(close.top(), R.top() + 1.0 + 3.0);
        assert_eq!(close.right(), R.right() - 1.0 - 3.0);
        h.frame(click(close.center()), &mut ui);
        h.frame(Vec::new(), &mut ui);
        assert!(!shown);
        assert!(!open);
    }

    /// (c) Dragging the title bar moves the window by the drag delta.
    #[test]
    fn title_drag_moves_window() {
        let mut h = Harness::new();
        let mut rect = Rect::NOTHING;
        let mut ui = |ctx: &Context| {
            rect = NxWindow::new("a", "Alpha")
                .default_rect(R)
                .show(ctx, |_| ())
                .unwrap()
                .response
                .rect;
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let from = pos2(200.0, 112.0);
        drag(&mut h, from, from + vec2(60.0, 40.0), &mut ui);
        assert_eq!(rect, R.translate(vec2(60.0, 40.0)));
    }

    /// Show "a" (Alpha) at `a` then "b" (Bravo) at `b`, every frame.
    fn two_windows(a: Rect, b: Rect) -> impl FnMut(&Context) {
        move |ctx| {
            let _ = NxWindow::new("a", "Alpha")
                .default_rect(a)
                .show(ctx, |_| ());
            let _ = NxWindow::new("b", "Bravo")
                .default_rect(b)
                .show(ctx, |_| ());
        }
    }

    fn title(r: Rect) -> Rect {
        layout(r, false, true, false).title
    }

    /// A window's options, for [`show_chrome`].
    #[derive(Clone, Copy)]
    struct Chrome {
        standard: bool,
        edited: bool,
        resizable: bool,
        closable: bool,
        miniaturizable: bool,
        open: bool,
        mini: bool,
    }

    const PLAIN: Chrome = Chrome {
        standard: false,
        edited: false,
        resizable: true,
        closable: false,
        miniaturizable: false,
        open: true,
        mini: false,
    };

    /// Show window `name` (titled `name`) at `r` with options `c`; its outer
    /// rect, `None` once closed. One call site, so one `show` for them all.
    fn show_chrome(ctx: &Context, name: &str, r: Rect, c: &mut Chrome) -> Option<Rect> {
        let mut w = NxWindow::new(name, name)
            .default_rect(r)
            .resizable(c.resizable)
            .edited(c.edited);
        if c.standard {
            w = w.standard();
        }
        if c.closable {
            w = w.closable(&mut c.open);
        }
        if c.miniaturizable {
            w = w.miniaturizable(&mut c.mini);
        }
        w.show(ctx, |ui| ui.max_rect()).map(|ir| ir.response.rect)
    }

    /// (d) Clicking window B makes it key and A non-key, and the painted title
    /// fills and text colors follow.
    #[test]
    fn click_makes_window_key() {
        let mut h = Harness::new();
        let a = Rect::from_min_size(pos2(50.0, 50.0), vec2(200.0, 150.0));
        let b = a.translate(vec2(350.0, 0.0));
        let mut ui = two_windows(a, b);

        let mut shapes = Vec::new();
        for _ in 0..3 {
            shapes = h.frame(Vec::new(), &mut ui);
        }
        assert!(is_key(&h.ctx, wid("a")), "first window shown is key");
        assert_eq!(fill_at(&shapes, title(a)), Some(BLACK));
        assert_eq!(fill_at(&shapes, title(b)), Some(LIGHT_GRAY));

        h.frame(click(b.center()), &mut ui);
        shapes = h.frame(Vec::new(), &mut ui);
        assert!(is_key(&h.ctx, wid("b")));
        assert!(!is_key(&h.ctx, wid("a")));
        assert_eq!(fill_at(&shapes, title(b)), Some(BLACK));
        assert_eq!(fill_at(&shapes, title(a)), Some(LIGHT_GRAY));
        assert_eq!(text_color(&shapes, "Bravo"), Some(WHITE));
        assert_eq!(text_color(&shapes, "Alpha"), Some(BLACK));
    }

    /// NeXTSTEP 2's three title bars: the key window black; the standard
    /// window last made key, once a panel takes the key, dark gray (the
    /// main window); every other window light gray.
    #[test]
    fn a_panel_taking_the_key_leaves_the_document_main() {
        let mut h = Harness::new();
        let doc = Rect::from_min_size(pos2(50.0, 50.0), vec2(200.0, 150.0));
        let other = doc.translate(vec2(0.0, 250.0));
        let panel = doc.translate(vec2(350.0, 0.0));
        let mut ui = |ctx: &Context| {
            for (n, r) in [("Doc", doc), ("Other", other)] {
                let mut c = Chrome {
                    standard: true,
                    ..PLAIN
                };
                show_chrome(ctx, n, r, &mut c);
            }
            show_chrome(ctx, "Panel", panel, &mut { PLAIN });
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        h.frame(click(panel.center()), &mut ui);
        let shapes = h.frame(Vec::new(), &mut ui);
        assert!(is_key(&h.ctx, wid("Panel")));
        assert!(is_main(&h.ctx, wid("Doc")), "the first document stays main");
        assert_eq!(fill_at(&shapes, title(panel)), Some(BLACK));
        assert_eq!(fill_at(&shapes, title(doc)), Some(DARK_GRAY));
        assert_eq!(text_color(&shapes, "Doc"), Some(WHITE));
        assert_eq!(fill_at(&shapes, title(other)), Some(LIGHT_GRAY));
        // Clicking the other document makes it key and main.
        h.frame(click(other.center()), &mut ui);
        let shapes = h.frame(Vec::new(), &mut ui);
        assert!(is_main(&h.ctx, wid("Other")));
        assert_eq!(fill_at(&shapes, title(other)), Some(BLACK));
        assert_eq!(fill_at(&shapes, title(doc)), Some(LIGHT_GRAY));
        assert_eq!(fill_at(&shapes, title(panel)), Some(LIGHT_GRAY));
        assert_eq!(text_color(&shapes, "Panel"), Some(BLACK));
    }

    /// An edited window's close box is broken: four short strokes, not two
    /// long ones. The miniaturize button sits at the left and reports a
    /// click.
    #[test]
    fn the_close_box_breaks_when_edited_and_miniaturize_sits_left() {
        let segments = |edited: bool| {
            let mut h = Harness::new();
            let mut open = true;
            let mut shapes = Vec::new();
            for _ in 0..3 {
                shapes = h.frame(Vec::new(), |ctx| {
                    let _ = NxWindow::new("a", "Alpha")
                        .default_rect(R)
                        .closable(&mut open)
                        .edited(edited)
                        .show(ctx, |_| ());
                });
            }
            let close = layout(R, true, true, false).close.unwrap();
            shapes
                .iter()
                .filter(|c| {
                    matches!(&c.shape, Shape::LineSegment { points, .. }
                        if close.contains(points[0]) && close.contains(points[1]))
                })
                .count()
        };
        assert_eq!(segments(false), 2);
        assert_eq!(segments(true), 4);

        let mut h = Harness::new();
        let mut mini = false;
        let mut ui = |ctx: &Context| {
            let _ = NxWindow::new("a", "Alpha")
                .default_rect(R)
                .miniaturizable(&mut mini)
                .show(ctx, |_| ());
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let l = layout(R, false, true, true);
        let m = l.mini.unwrap();
        assert_eq!(m.left(), R.left() + 1.0 + CLOSE_INSET);
        assert!(l.close.is_none());
        h.frame(click(m.center()), &mut ui);
        assert!(mini);
    }

    /// A click where windows overlap keys the one on top, not the one shown
    /// last; clicking a window also raises it.
    #[test]
    fn overlap_click_keys_the_top_window() {
        let mut h = Harness::new();
        let a = R;
        let b = R.translate(vec2(150.0, 100.0));
        let mut ui = two_windows(a, b);
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let overlap = a.intersect(b).center();

        h.frame(click(overlap), &mut ui);
        assert!(is_key(&h.ctx, wid("b")), "B was shown last, so is on top");
        assert!(!is_key(&h.ctx, wid("a")));

        h.frame(click(a.min + vec2(20.0, 50.0)), &mut ui);
        assert!(is_key(&h.ctx, wid("a")));
        assert!(!is_key(&h.ctx, wid("b")));

        // A is now on top, though B is still shown after it.
        h.frame(click(overlap), &mut ui);
        assert!(is_key(&h.ctx, wid("a")));
    }

    /// The click frame paints both titles black: the clicked window takes the
    /// key after the earlier-shown one has already painted (one-frame lag).
    #[test]
    fn click_frame_paints_both_titles_black() {
        let mut h = Harness::new();
        let a = Rect::from_min_size(pos2(50.0, 50.0), vec2(200.0, 150.0));
        let b = a.translate(vec2(350.0, 0.0));
        let mut ui = two_windows(a, b);
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let shapes = h.frame(click(b.center()), &mut ui);
        assert_eq!(fill_at(&shapes, title(a)), Some(BLACK));
        assert_eq!(fill_at(&shapes, title(b)), Some(BLACK));
    }

    /// Closing the key window hands the key to the next window shown.
    #[test]
    fn closing_key_window_hands_key_on() {
        let mut h = Harness::new();
        let mut open = true;
        let b = R.translate(vec2(350.0, 0.0));
        let mut ui = |ctx: &Context| {
            let _ = NxWindow::new("a", "Alpha")
                .default_rect(R)
                .closable(&mut open)
                .show(ctx, |_| ());
            if !open {
                let _ = NxWindow::new("b", "Bravo")
                    .default_rect(b)
                    .show(ctx, |_| ());
            }
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        assert!(is_key(&h.ctx, wid("a")));
        h.frame(
            click(layout(R, true, true, false).close.unwrap().center()),
            &mut ui,
        );
        h.frame(Vec::new(), &mut ui);
        assert!(!open);
        assert!(is_key(&h.ctx, wid("b")));
    }

    /// The close and miniaturize buttons say what they do to a screen
    /// reader (an edited window's close box says so) and act on its Click:
    /// closing a background window leaves the key where it is.
    #[test]
    fn the_title_bar_buttons_act_for_a_screen_reader() {
        use egui::accesskit::{Action, ActionRequest, NodeId, TreeId};
        let mut h = Harness::new();
        h.ctx.enable_accesskit();
        let b = R.translate(vec2(350.0, 0.0));
        let mut alpha = Chrome {
            standard: true,
            edited: true,
            closable: true,
            miniaturizable: true,
            ..PLAIN
        };
        let mut bravo = Chrome {
            closable: true,
            resizable: false,
            ..PLAIN
        };
        let mut labels = Vec::new();
        let frame = |h: &mut Harness, events, a: &mut Chrome, b2: &mut Chrome| {
            let raw = RawInput {
                time: Some(h.time),
                screen_rect: Some(SCREEN),
                events,
                ..Default::default()
            };
            h.time += 1.0 / 60.0;
            let out = h.ctx.test_pass(raw, |ctx| {
                show_chrome(ctx, "Alpha", R, a);
                show_chrome(ctx, "Bravo", b, b2);
            });
            out.platform_output
                .accesskit_update
                .expect("AccessKit is on")
                .nodes
                .into_iter()
                .filter_map(|(_, n)| n.label().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        for _ in 0..3 {
            labels = frame(&mut h, Vec::new(), &mut alpha, &mut bravo);
        }
        for want in ["Close Alpha (edited)", "Miniaturize Alpha", "Close Bravo"] {
            assert!(labels.iter().any(|l| l == want), "{want} in {labels:?}");
        }
        assert!(is_key(&h.ctx, wid("Alpha")));
        let click = |id: Id| {
            Event::AccessKitActionRequest(ActionRequest {
                action: Action::Click,
                target_tree: TreeId::ROOT,
                target_node: NodeId::from(id.value()),
                data: None,
            })
        };
        frame(
            &mut h,
            vec![click(wid("Alpha").with("mini"))],
            &mut alpha,
            &mut bravo,
        );
        assert!(alpha.mini && alpha.open, "miniaturized, not closed");
        frame(
            &mut h,
            vec![click(wid("Bravo").with("close"))],
            &mut alpha,
            &mut bravo,
        );
        assert!(!bravo.open);
        assert!(is_key(&h.ctx, wid("Alpha")), "the key stays with Alpha");
        frame(
            &mut h,
            vec![click(wid("Alpha").with("close"))],
            &mut alpha,
            &mut bravo,
        );
        assert!(!alpha.open);
        assert!(
            !is_key(&h.ctx, wid("Alpha")),
            "a closed window gives up the key"
        );
    }

    /// A window placed off screen is moved back on, keeping its size.
    #[test]
    fn off_screen_window_is_clamped_on_screen() {
        let mut h = Harness::new();
        let mut rect = Rect::NOTHING;
        for _ in 0..3 {
            h.frame(Vec::new(), |ctx| {
                rect = NxWindow::new("a", "Alpha")
                    .default_rect(R.translate(vec2(1000.0, 1000.0)))
                    .show(ctx, |_| ())
                    .unwrap()
                    .response
                    .rect;
            });
        }
        assert_eq!(rect.max, SCREEN.max);
        assert_eq!(rect.size(), R.size());
    }

    /// Drag the bottom bar at `x` by `delta` and return the window rect.
    fn drag_resize_bar(resizable: bool, x: f32, delta: Vec2) -> Rect {
        let mut h = Harness::new();
        let mut rect = Rect::NOTHING;
        let mut ui = |ctx: &Context| {
            let mut c = Chrome { resizable, ..PLAIN };
            rect = show_chrome(ctx, "a", R, &mut c).unwrap();
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let from = pos2(x, R.bottom() - 1.0 - 4.0);
        drag(&mut h, from, from + delta, &mut ui);
        rect
    }

    /// (e) The resize bar resizes only when resizable; left of its left
    /// notch it drags the bottom-left corner, right of the right notch the
    /// bottom-right.
    #[test]
    fn resize_bar_resizes_only_when_resizable() {
        let d = vec2(40.0, 30.0);
        let right = drag_resize_bar(true, R.right() - 20.0, d);
        assert_eq!(right, Rect::from_min_max(R.min, R.max + d));
        let left = drag_resize_bar(true, R.left() + 20.0, d);
        assert_eq!(
            left,
            Rect::from_min_max(R.min + vec2(40.0, 0.0), R.max + vec2(0.0, 30.0))
        );
        assert_eq!(drag_resize_bar(false, R.right() - 20.0, d), R);
    }

    /// Between the notches the bar drags only the bottom edge, even well
    /// outside the middle third (the notches sit 30 px in, not at thirds).
    #[test]
    fn between_the_notches_resizes_height_only() {
        for x in [R.center().x, R.left() + 40.0, R.right() - 40.0] {
            let mid = drag_resize_bar(true, x, vec2(40.0, 30.0));
            assert_eq!(
                mid,
                Rect::from_min_max(R.min, R.max + vec2(0.0, 30.0)),
                "at {x}"
            );
        }
    }

    /// Dragging the bar past the screen bottom stops the edge there instead
    /// of shoving the window up.
    #[test]
    fn resize_past_screen_bottom_clamps_the_edge() {
        let r = drag_resize_bar(true, R.center().x, vec2(0.0, 400.0));
        assert_eq!(r.min.y, R.min.y);
        assert_eq!(r.max.y, SCREEN.max.y);
    }

    /// Show window a (default rect `R`) configured by `f`; its outer rect.
    fn show_a(ctx: &Context, f: impl FnOnce(NxWindow<'_>) -> NxWindow<'_>) -> Rect {
        f(NxWindow::new("a", "Alpha").default_rect(R))
            .show(ctx, |_| ())
            .unwrap()
            .response
            .rect
    }

    /// Dragging the left corner zone past `min_size` stops the left edge there,
    /// rather than at `MIN_SIZE` for the caller to enlarge (sliding it right).
    #[test]
    fn left_resize_stops_at_min_size() {
        let min = vec2(240.0, 180.0);
        let mut h = Harness::new();
        let mut ui = |ctx: &Context| {
            show_a(ctx, |w| w.min_size(min));
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let from = pos2(R.left() + 20.0, R.bottom() - 1.0 - 4.0);
        drag(&mut h, from, from + vec2(400.0, 0.0), &mut ui);
        let rect = stored_rect(&h.ctx, wid("a")).unwrap();
        assert_eq!(rect.max.x, R.max.x);
        assert_eq!(rect.width(), min.x);
    }

    /// `bounds` replaces the screen as the rect a dragged window is kept in.
    #[test]
    fn bounds_clamp_a_dragged_window() {
        let bounds = Rect::from_min_max(pos2(50.0, 60.0), pos2(600.0, 500.0));
        let mut h = Harness::new();
        let mut ui = |ctx: &Context| {
            show_a(ctx, |w| w.bounds(bounds));
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let from = title(R).center();
        drag(&mut h, from, from + vec2(500.0, 500.0), &mut ui);
        let rect = stored_rect(&h.ctx, wid("a")).unwrap();
        assert_eq!(rect.max, bounds.max);
        assert_eq!(rect.size(), R.size());
    }

    /// `rect` overrides the stored rect; a caller owning the rect reads it
    /// back with `stored_rect`, this frame's drag applied.
    #[test]
    fn owned_rect_overrides_and_reads_back() {
        let mut h = Harness::new();
        for _ in 0..3 {
            h.frame(Vec::new(), |ctx| {
                show_a(ctx, |w| w);
            });
        }
        assert_eq!(stored_rect(&h.ctx, wid("a")), Some(R));
        let start = R.translate(vec2(200.0, 150.0));
        let mut shown = Rect::NOTHING;
        h.frame(Vec::new(), |ctx| shown = show_a(ctx, |w| w.rect(start)));
        assert_eq!(shown, start, "rect() beats the stored R");
        let mut owned = start;
        let mut ui = |ctx: &Context| {
            shown = show_a(ctx, |w| w.rect(owned));
            owned = stored_rect(ctx, wid("a")).unwrap();
        };
        let from = title(start).center();
        let d = vec2(-60.0, -40.0);
        drag(&mut h, from, from + d, &mut ui);
        assert_eq!(owned, start.translate(d));
        assert_eq!(shown, owned);
    }

    /// An incoming rect smaller than `min_size` is enlarged, top-left kept.
    #[test]
    fn undersized_rect_is_enlarged() {
        let min = vec2(240.0, 180.0);
        let small = Rect::from_min_size(pos2(100.0, 100.0), vec2(50.0, 40.0));
        let mut h = Harness::new();
        let mut rect = Rect::NOTHING;
        for _ in 0..3 {
            h.frame(Vec::new(), |ctx| {
                rect = show_a(ctx, |w| w.rect(small).min_size(min));
            });
        }
        assert_eq!(rect, Rect::from_min_size(small.min, min));
        assert_eq!(stored_rect(&h.ctx, wid("a")), Some(rect));
    }
}
