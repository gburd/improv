//! NeXTSTEP window chrome for in-app floating windows (spec §3, §4.3, §5).
//!
//! [`NxWindow`] wraps an [`egui::Window`] that has no egui title bar or frame,
//! and paints the NeXT chrome itself: a 1 px black border, a 23 px title bar
//! (black when key, dark gray otherwise), a beveled close box, a bottom resize
//! bar split into thirds, and a light-gray body. Every piece is a painter
//! shape; the only glyphs are the title text.
//!
//! The *key* window is the most recently clicked `NxWindow`, tracked in egui
//! memory; see [`is_key`]. Closing the key window hands the key to the next
//! window shown.

use crate::theme::{BLACK, DARK_GRAY, LIGHT_GRAY, WHITE};
use egui::{
    pos2, vec2, Align2, Color32, Context, FontFamily, FontId, Frame, Id, InnerResponse, LayerId,
    Order, Painter, Pos2, Rect, Sense, Stroke, Ui, UiBuilder, Vec2,
};

// Geometry, in points (spec §3).
const BORDER: f32 = 1.0;
const TITLE_HEIGHT: f32 = 23.0;
const CLOSE_SIZE: f32 = 15.0;
const CLOSE_INSET: f32 = 4.0;
const RESIZE_HEIGHT: f32 = 9.0;
const MIN_SIZE: Vec2 = vec2(100.0, 60.0);
const DEFAULT_RECT: Rect = Rect {
    min: pos2(32.0, 32.0),
    max: pos2(352.0, 272.0),
};

/// Title bar fill and text colors: black with white text for the key window,
/// dark gray with light-gray text otherwise (spec §1).
pub fn title_colors(key: bool) -> (Color32, Color32) {
    if key {
        (BLACK, WHITE)
    } else {
        (DARK_GRAY, LIGHT_GRAY)
    }
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
    Id::new("nxwin::key")
}

/// A floating window with NeXTSTEP chrome. Build with [`NxWindow::new`],
/// configure, then call [`NxWindow::show`] every frame.
#[must_use = "call .show(ctx, ...)"]
pub struct NxWindow<'open> {
    id: Id,
    title: String,
    open: Option<&'open mut bool>,
    default_rect: Rect,
    resizable: bool,
}

impl<'open> NxWindow<'open> {
    /// A window identified by `id_source` (salted, see [`Self::id`]), showing
    /// `title` centred in its title bar. Resizable by default, not closable.
    pub fn new(id_source: impl std::hash::Hash, title: impl Into<String>) -> Self {
        Self {
            id: Id::new(("nxwin", id_source)),
            title: title.into(),
            open: None,
            default_rect: DEFAULT_RECT,
            resizable: true,
        }
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
    pub fn default_rect(mut self, rect: Rect) -> Self {
        self.default_rect = rect;
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
            resizable,
        } = self;
        if matches!(open, Some(false)) {
            return None;
        }
        let closable = open.is_some();
        let state_id = id.with("nxwin_rect");
        let mut rect = ctx
            .data(|d| d.get_temp::<Rect>(state_id))
            .unwrap_or(default_rect);

        // Apply this frame's drags before placing the window, as egui's own
        // `Area` does, so the chrome tracks the pointer with no frame delay.
        if let Some(r) = ctx.read_response(id.with("title")) {
            rect = rect.translate(r.drag_delta());
        }
        let screen = ctx.screen_rect();
        if let Some(r) = ctx
            .read_response(id.with("nx_resize"))
            .filter(|r| r.dragged())
        {
            // NeXT: the left third drags the bottom-left corner, the middle
            // the bottom edge, the right third the bottom-right corner. The
            // third is latched at press, since the bar moves under the pointer.
            let zone_id = id.with("resize_zone");
            let zone = if r.drag_started() {
                let x = r
                    .interact_pointer_pos()
                    .map_or(0.0, |p| p.x - r.rect.left());
                let zone = if x * 3.0 < r.rect.width() {
                    0
                } else if x * 3.0 < r.rect.width() * 2.0 {
                    1
                } else {
                    2
                };
                ctx.data_mut(|d| d.insert_temp(zone_id, zone));
                zone
            } else {
                ctx.data(|d| d.get_temp::<u8>(zone_id)).unwrap_or(1)
            };
            // Clamp the dragged edge to the screen (never `f32::clamp`, which
            // panics when the screen is smaller than MIN_SIZE).
            let d = r.drag_delta();
            match zone {
                0 => {
                    rect.min.x = (rect.min.x + d.x)
                        .max(screen.min.x)
                        .min(rect.max.x - MIN_SIZE.x);
                }
                2 => {
                    rect.max.x = (rect.max.x + d.x)
                        .min(screen.max.x)
                        .max(rect.min.x + MIN_SIZE.x);
                }
                _ => {}
            }
            rect.max.y = (rect.max.y + d.y)
                .min(screen.max.y)
                .max(rect.min.y + MIN_SIZE.y);
        }

        // Keep the window on screen. Done here rather than by egui, whose first
        // (sizing) pass clamps against a guessed 600x400 size.
        let max = (screen.max - rect.size()).max(screen.min);
        rect = rect.translate(rect.min.clamp(screen.min, max) - rect.min);
        ctx.data_mut(|d| d.insert_temp(state_id, rect));

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
        });
        let key = is_key(ctx, id);

        let mut close_clicked = false;
        let shown = egui::Window::new(title.as_str())
            .id(id)
            .title_bar(false)
            .frame(Frame::none())
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
                let l = layout(outer, closable, resizable);
                let p = ui.painter().clone();
                let (fill, text) = title_colors(key);

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
                let _ = ui.interact(l.title, id.with("title"), Sense::drag());

                // Registered after the title so it sits on top for hit-testing.
                if let Some(close) = l.close {
                    bevel(&p, close);
                    let x = close.shrink(4.0);
                    let stroke = Stroke::new(1.0_f32, BLACK);
                    p.line_segment([x.left_top(), x.right_bottom()], stroke);
                    p.line_segment([x.right_top(), x.left_bottom()], stroke);
                    close_clicked = ui
                        .interact(close, id.with("close"), Sense::click())
                        .clicked();
                }

                if let Some(bar) = l.resize {
                    bevel(&p, bar);
                    for f in [1.0 / 3.0, 2.0 / 3.0] {
                        let x = (bar.left() + bar.width() * f).round();
                        p.rect_filled(
                            Rect::from_x_y_ranges(x..=x + 1.0, bar.y_range()),
                            0.0,
                            BLACK,
                        );
                        p.rect_filled(
                            Rect::from_x_y_ranges(x + 1.0..=x + 2.0, bar.y_range()),
                            0.0,
                            WHITE,
                        );
                    }
                    let _ = ui.interact(bar, id.with("nx_resize"), Sense::drag());
                }

                let mut body = ui.new_child(UiBuilder::new().max_rect(l.body));
                body.set_clip_rect(l.body.intersect(ui.clip_rect()));
                let inner = add_contents(&mut body);

                p.rect_stroke(outer.shrink(BORDER / 2.0), 0.0, Stroke::new(BORDER, BLACK));
                // Size the window to the chrome, not to whatever the body holds.
                ui.advance_cursor_after_rect(outer);
                inner
            })?;

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
    body: Rect,
    resize: Option<Rect>,
}

fn layout(outer: Rect, closable: bool, resizable: bool) -> Layout {
    let inner = outer.shrink(BORDER);
    let title = Rect::from_min_size(inner.min, vec2(inner.width(), TITLE_HEIGHT));
    let close = closable.then(|| {
        Rect::from_min_size(
            pos2(
                title.right() - CLOSE_INSET - CLOSE_SIZE,
                title.center().y - CLOSE_SIZE / 2.0,
            ),
            Vec2::splat(CLOSE_SIZE),
        )
    });
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
        body,
        resize,
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
mod tests {
    use super::*;
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
            self.ctx.run(raw, ui).shapes
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

    /// (a) The painted title bar is 23 px high and the body Ui starts below it.
    #[test]
    fn title_bar_is_23px_and_body_sits_below() {
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
        assert_eq!(title.height(), 23.0);
        assert_eq!(body.top(), title.bottom());
        assert_eq!((body.left(), body.right()), (title.left(), title.right()));
        assert_eq!(
            body.bottom(),
            R.bottom() - 1.0 - 9.0,
            "resize bar below the body"
        );
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
        let close = layout(R, true, true).close.unwrap();
        assert_eq!(close.size(), vec2(15.0, 15.0));
        assert_eq!(close.right(), R.right() - 1.0 - 4.0);
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
        layout(r, false, true).title
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
        assert_eq!(fill_at(&shapes, title(b)), Some(DARK_GRAY));

        h.frame(click(b.center()), &mut ui);
        shapes = h.frame(Vec::new(), &mut ui);
        assert!(is_key(&h.ctx, wid("b")));
        assert!(!is_key(&h.ctx, wid("a")));
        assert_eq!(fill_at(&shapes, title(b)), Some(BLACK));
        assert_eq!(fill_at(&shapes, title(a)), Some(DARK_GRAY));
        assert_eq!(text_color(&shapes, "Bravo"), Some(WHITE));
        assert_eq!(text_color(&shapes, "Alpha"), Some(LIGHT_GRAY));
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
            click(layout(R, true, true).close.unwrap().center()),
            &mut ui,
        );
        h.frame(Vec::new(), &mut ui);
        assert!(!open);
        assert!(is_key(&h.ctx, wid("b")));
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
            rect = NxWindow::new("a", "Alpha")
                .default_rect(R)
                .resizable(resizable)
                .show(ctx, |_| ())
                .unwrap()
                .response
                .rect;
        };
        for _ in 0..3 {
            h.frame(Vec::new(), &mut ui);
        }
        let from = pos2(x, R.bottom() - 1.0 - 4.5);
        drag(&mut h, from, from + delta, &mut ui);
        rect
    }

    /// (e) The resize bar resizes only when resizable; the left third drags
    /// the bottom-left corner, the right third the bottom-right.
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

    /// The middle third drags only the bottom edge.
    #[test]
    fn middle_third_resizes_height_only() {
        let mid = drag_resize_bar(true, R.center().x, vec2(40.0, 30.0));
        assert_eq!(mid, Rect::from_min_max(R.min, R.max + vec2(0.0, 30.0)));
    }

    /// Dragging the bar past the screen bottom stops the edge there instead
    /// of shoving the window up.
    #[test]
    fn resize_past_screen_bottom_clamps_the_edge() {
        let r = drag_resize_bar(true, R.center().x, vec2(0.0, 400.0));
        assert_eq!(r.min.y, R.min.y);
        assert_eq!(r.max.y, SCREEN.max.y);
    }
}
