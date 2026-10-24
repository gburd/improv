//! The NeXTSTEP egui theme for the Improv GUI.
//!
//! Lotus Improv ran on NeXTSTEP's 2-bit grayscale display: four grays and no
//! color at all. This module builds an [`egui::Style`] from exactly those four
//! levels (plus the scroller trough), with square corners and 1 px strokes.
//! Roles follow `docs/reviews/2026-10-03-nextstep-visual-spec.md` §1. Text is
//! the bundled Arimo (§2), a Helvetica-metric face, installed by
//! [`install_fonts`]. It is purely cosmetic: no behavior depends on it.

use egui::{
    pos2, vec2, Color32, FontData, FontDefinitions, FontFamily, FontId, Rect, Rounding, Shadow,
    Stroke, Style, TextStyle, Visuals,
};

/// Key-window title bars, grid rules, text, bevel dark edges.
pub const BLACK: Color32 = Color32::from_gray(0x00);
/// Desktop, category tile faces, non-key title bars, disabled / weak text.
pub const DARK_GRAY: Color32 = Color32::from_gray(0x55);
/// Window and panel background, control faces, the selection fill.
pub const LIGHT_GRAY: Color32 = Color32::from_gray(0xAA);
/// Grid cells, text fields, pressed faces, bevel light edges.
pub const WHITE: Color32 = Color32::from_gray(0xFF);
/// The scroller trough (the one value outside the four levels, spec §4.8).
pub const TROUGH: Color32 = Color32::from_gray(0x80);

/// Every color the theme may use. `TRANSPARENT` is "no stroke / no shadow".
#[cfg(test)]
const ALLOWED: [Color32; 6] = [
    BLACK,
    DARK_GRAY,
    LIGHT_GRAY,
    WHITE,
    TROUGH,
    Color32::TRANSPARENT,
];

/// The font name and family of the bold face, for window and menu titles.
pub const BOLD: &str = "bold";

/// NeXT's one text size: Helvetica 12 everywhere (§2).
const TEXT_SIZE: f32 = 12.0;

/// Apply the NeXTSTEP look to `ctx` at startup: pin the light theme slot
/// first, so a system theme change can't swap in egui's stock visuals, then
/// set [`next_style`] into that slot and [`install_fonts`].
pub fn apply(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.set_style(next_style());
    install_fonts(ctx);
}

/// Register the bundled Arimo with `ctx`: Regular first in the proportional
/// family, ahead of egui's default fonts (kept as glyph fallback), and Bold as
/// the named family [`BOLD`]. [`TextStyle::Heading`] is pointed at the bold
/// family here rather than in [`next_style`], because a style naming an
/// unregistered family panics when text is laid out; call this after
/// `set_style`. Monospace is left as egui's default.
pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    fonts.font_data.insert(
        "Arimo".into(),
        FontData::from_static(include_bytes!("../assets/fonts/Arimo-Regular.ttf")),
    );
    fonts.font_data.insert(
        BOLD.into(),
        FontData::from_static(include_bytes!("../assets/fonts/Arimo-Bold.ttf")),
    );
    let fallback = fonts.families[&FontFamily::Proportional].clone();
    fonts
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Arimo".into());
    fonts.families.insert(
        FontFamily::Name(BOLD.into()),
        std::iter::once(BOLD.to_string()).chain(fallback).collect(),
    );
    ctx.set_fonts(fonts);
    ctx.all_styles_mut(|s| {
        s.text_styles.insert(
            TextStyle::Heading,
            FontId::new(TEXT_SIZE, FontFamily::Name(BOLD.into())),
        );
    });
}

/// Show `area` NeXT-style: the bar always there, its trough [`TROUGH`],
/// with [`scroller_chrome`] on top (one line = one interact row).
/// egui paints the trough with `extreme_bg_color`, which is WHITE for text
/// edits, so it is swapped for the scroller only and restored for `add`.
pub fn next_scroll<R>(
    ui: &mut egui::Ui,
    area: egui::ScrollArea,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::scroll_area::ScrollAreaOutput<R> {
    let scoped = ui.scope(|ui| {
        ui.visuals_mut().extreme_bg_color = TROUGH;
        area.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| {
                content(ui);
                add(ui)
            })
    });
    let line = ui.spacing().interact_size.y + ui.spacing().item_spacing.y;
    // The scope holds just the area, so its rect is the area plus its bar.
    scroller_chrome(ui, &scoped.inner, scoped.response.rect, line);
    scoped.inner
}

/// §4.8 arrow buttons: 18 px squares.
pub const ARROW: f32 = 18.0;

/// The vertical trough at the right of a non-floating scroll area's `outer`
/// rect (area plus bar), then its up and down arrow buttons grouped at the
/// trough's bottom end.
pub fn scroller_rects(outer: Rect, bar_width: f32) -> [Rect; 3] {
    let trough = outer.with_min_x(outer.max.x - bar_width);
    let button = |n: f32| {
        Rect::from_min_size(
            pos2(trough.max.x - ARROW, trough.max.y - n * ARROW),
            vec2(ARROW, ARROW),
        )
    };
    [trough, button(2.0), button(1.0)]
}

/// Where a scroller knob spans along `track` for a `view` long window at
/// `offset` into `content`: grown to `min_len`, kept inside the track.
fn knob_span(
    offset: f32,
    view: f32,
    content: f32,
    track: std::ops::RangeInclusive<f32>,
    min_len: f32,
) -> (f32, f32) {
    let at = |v: f32| egui::remap_clamp(v, 0.0..=content.max(view), track.clone());
    let (a, b) = (at(offset), at(offset + view));
    let grow = (min_len - (b - a)).max(0.0) / 2.0;
    ((a - grow).max(*track.start()), (b + grow).min(*track.end()))
}

/// Drag `knob` (it senses clicks too, so the egui bar under it never jumps):
/// the scroll offset change for this frame's drag, mapping the knob's free
/// travel (`track_len` minus the knob) onto the `max` scrollable points.
/// `sense` for a scroller part: pointer only, never a Tab stop. The keyboard
/// scrolls the area itself (arrows, PageUp/PageDown), so the arrows and knob
/// would only add focus stops a screen reader announces with no meaning (H2).
fn pointer_only(sense: egui::Sense) -> egui::Sense {
    egui::Sense {
        focusable: false,
        ..sense
    }
}

fn knob_drag(
    ui: &egui::Ui,
    id: egui::Id,
    knob: Rect,
    axis: usize,
    track_len: f32,
    max: f32,
) -> f32 {
    let r = ui.interact(knob, id, pointer_only(egui::Sense::click_and_drag()));
    if !r.dragged() {
        return 0.0;
    }
    let free = (track_len - knob.size()[axis]).max(1.0);
    r.drag_delta()[axis] * max / free
}

/// Store `offset` (clamped to `0..=max`) on `axis` of `out`'s area if it
/// moved, and repaint so the area shows it.
fn scroll_to<R>(
    ui: &egui::Ui,
    out: &egui::scroll_area::ScrollAreaOutput<R>,
    axis: usize,
    offset: f32,
    max: f32,
) {
    let offset = offset.clamp(0.0, max);
    if offset != out.state.offset[axis] {
        let mut state = out.state;
        state.offset[axis] = offset;
        state.store(ui.ctx(), out.id);
        ui.ctx().request_repaint();
    }
}

/// Repaint a shown vertical scroller NeXT-style: a knob with a 1 px bevel
/// (white top-left, black bottom-right) above two grouped arrow buttons, each
/// click scrolling `line` points. `outer` is the area plus its bar as laid
/// out (`out.inner_rect` is the pre-shrink rect, so it can't be used). The
/// knob drags 1:1 with the pointer (ours, not egui's whole-trough mapping),
/// so it also works when the bar is drawn at a viewport edge egui's own bar
/// is scrolled away from (a grid wider than its window).
pub fn scroller_chrome<R>(
    ui: &egui::Ui,
    out: &egui::scroll_area::ScrollAreaOutput<R>,
    outer: Rect,
    line: f32,
) {
    let scroll = &ui.spacing().scroll;
    let [trough, up, down] = scroller_rects(outer, scroll.bar_width);
    let p = ui.painter();
    p.rect_filled(trough, Rounding::ZERO, TROUGH);
    let track = trough.min.y..=up.min.y.max(trough.min.y);
    let view = trough.height();
    let (y0, y1) = knob_span(
        out.state.offset.y,
        view,
        out.content_size.y,
        track.clone(),
        scroll.handle_min_length,
    );
    let knob = p.round_rect_to_pixels(Rect::from_x_y_ranges(trough.x_range(), y0..=y1));
    raised(p, knob);
    let max = (out.content_size.y - view).max(0.0);
    let mut offset = out.state.offset.y;
    for (rect, sign) in [(up, -1.0), (down, 1.0)] {
        raised(p, rect);
        let c = rect.center();
        let (tip, base) = (c.y + sign * 4.0, c.y - sign * 4.0);
        p.add(egui::Shape::convex_polygon(
            vec![pos2(c.x, tip), pos2(c.x + 5.0, base), pos2(c.x - 5.0, base)],
            BLACK,
            Stroke::NONE,
        ));
        if ui
            .interact(
                rect,
                out.id.with(("arrow", sign as i8)),
                pointer_only(egui::Sense::click()),
            )
            .clicked()
        {
            offset += sign * line;
        }
    }
    let track_len = track.end() - track.start();
    offset += knob_drag(ui, out.id.with("knob"), knob, 1, track_len, max);
    scroll_to(ui, out, 1, offset, max);
}

/// The §4.4 horizontal scroller in `bar` (the bottom strip, right of the row
/// tiles): its left and right arrow buttons grouped at the LEFT end, as in
/// the reference, then the knob's track.
pub fn hscroller_rects(bar: Rect) -> [Rect; 3] {
    let button = |n: f32| {
        Rect::from_min_size(
            pos2(bar.min.x + n * ARROW, bar.max.y - ARROW),
            vec2(ARROW, ARROW),
        )
    };
    let track = bar.with_min_x((bar.min.x + 2.0 * ARROW).min(bar.max.x));
    [button(0.0), button(1.0), track]
}

/// Draw and run a horizontal scroller in `bar` for `out` (an area whose own
/// bar is hidden): grouped arrows scroll `line` points per click, the beveled
/// knob drags, and a click in the trough beside the knob pages by a view.
pub fn hscroller_chrome<R>(
    ui: &egui::Ui,
    out: &egui::scroll_area::ScrollAreaOutput<R>,
    bar: Rect,
    line: f32,
) {
    let [left, right, track] = hscroller_rects(bar);
    let p = ui.painter();
    p.rect_filled(bar, Rounding::ZERO, TROUGH);
    let view = out.inner_rect.width();
    let max = (out.content_size.x - view).max(0.0);
    let (x0, x1) = knob_span(
        out.state.offset.x,
        view,
        out.content_size.x,
        track.min.x..=track.max.x,
        ui.spacing().scroll.handle_min_length,
    );
    let knob = p.round_rect_to_pixels(Rect::from_x_y_ranges(x0..=x1, bar.y_range()));
    let mut offset = out.state.offset.x;
    let trough = ui.interact(
        track,
        out.id.with("htrough"),
        pointer_only(egui::Sense::click()),
    );
    if let Some(at) = trough
        .clicked()
        .then(|| trough.interact_pointer_pos().or(trough.hover_pos()))
        .flatten()
    {
        if at.x < knob.min.x {
            offset -= view;
        } else if at.x > knob.max.x {
            offset += view;
        }
    }
    raised(p, knob);
    for (rect, sign) in [(left, -1.0), (right, 1.0)] {
        raised(p, rect);
        let c = rect.center();
        let (tip, base) = (c.x + sign * 4.0, c.x - sign * 4.0);
        p.add(egui::Shape::convex_polygon(
            vec![pos2(tip, c.y), pos2(base, c.y + 5.0), pos2(base, c.y - 5.0)],
            BLACK,
            Stroke::NONE,
        ));
        if ui
            .interact(
                rect,
                out.id.with(("harrow", sign as i8)),
                pointer_only(egui::Sense::click()),
            )
            .clicked()
        {
            offset += sign * line;
        }
    }
    offset += knob_drag(ui, out.id.with("hknob"), knob, 0, track.width(), max);
    scroll_to(ui, out, 0, offset, max);
}

/// A raised NeXT face: [`LIGHT_GRAY`] with a white top-left, black
/// bottom-right 1 px bevel.
fn raised(p: &egui::Painter, r: Rect) {
    let fill = |min, max, c| p.rect_filled(Rect::from_min_max(min, max), Rounding::ZERO, c);
    fill(r.min, r.max, LIGHT_GRAY);
    fill(r.min, pos2(r.max.x, r.min.y + 1.0), WHITE);
    fill(r.min, pos2(r.min.x + 1.0, r.max.y), WHITE);
    fill(pos2(r.min.x, r.max.y - 1.0), r.max, BLACK);
    fill(pos2(r.max.x - 1.0, r.min.y), r.max, BLACK);
}

/// Undo the trough color inside scrolled content, so text edits stay WHITE.
pub fn content(ui: &mut egui::Ui) {
    ui.visuals_mut().extreme_bg_color = WHITE;
}

/// Build the NeXTSTEP [`Style`]. Applied once at startup by [`apply`].
pub fn next_style() -> Style {
    let mut style = Style::default();
    for (ts, size) in [
        (TextStyle::Body, TEXT_SIZE),
        (TextStyle::Button, TEXT_SIZE),
        (TextStyle::Heading, TEXT_SIZE),
    ] {
        style
            .text_styles
            .insert(ts, FontId::new(size, FontFamily::Proportional));
    }
    let mut v = Visuals::light();
    let line = |c| Stroke::new(1.0_f32, c);

    v.override_text_color = Some(BLACK);
    v.window_fill = LIGHT_GRAY;
    v.panel_fill = LIGHT_GRAY;
    v.extreme_bg_color = WHITE; // text edits, scroll bar background
    v.faint_bg_color = WHITE; // striped rows
    v.code_bg_color = WHITE;
    v.hyperlink_color = BLACK;
    v.warn_fg_color = BLACK;
    v.error_fg_color = BLACK;
    v.window_rounding = Rounding::ZERO;
    v.menu_rounding = Rounding::ZERO;
    v.window_shadow = Shadow::NONE;
    v.popup_shadow = Shadow::NONE;
    v.window_stroke = line(BLACK);
    // Gray, never blue and never inverted; the black stroke keeps a selected
    // cell visible on a gray row.
    v.selection.bg_fill = LIGHT_GRAY;
    v.selection.stroke = line(BLACK);
    v.text_cursor.stroke = line(BLACK);

    // Idle and hovered controls are the same raised gray face: NeXT
    // highlights only on press, when the face turns white.
    let face = |fill| egui::style::WidgetVisuals {
        bg_fill: fill,
        weak_bg_fill: fill,
        bg_stroke: line(BLACK),
        fg_stroke: line(BLACK),
        rounding: Rounding::ZERO,
        expansion: 0.0,
    };
    let w = &mut v.widgets;
    w.inactive = face(LIGHT_GRAY);
    w.hovered = face(LIGHT_GRAY);
    w.active = face(WHITE);
    w.open = face(WHITE);
    // Labels, frames, separators. `weak_bg_fill` is egui's fade target, so
    // weak and disabled text come out exactly DARK_GRAY (black halfway to
    // LIGHT_GRAY).
    w.noninteractive = egui::style::WidgetVisuals {
        bg_stroke: line(DARK_GRAY),
        ..face(LIGHT_GRAY)
    };

    style.visuals = v;
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    // §3: 17 px rows, header bands and tiles.
    style.spacing.interact_size.y = 17.0;
    // §4.8: an always-there 19 px trough with a square knob; the bevel and
    // grouped arrows are painted over it by `scroller_chrome`.
    style.spacing.scroll = egui::style::ScrollStyle {
        floating: false,
        bar_width: 19.0,
        bar_inner_margin: 0.0,
        bar_outer_margin: 0.0,
        handle_min_length: 19.0,
        foreground_color: false,
        ..egui::style::ScrollStyle::solid()
    };
    style
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// Every `Color32` in `visuals`, found through its `Debug` form
    /// (`#RR_GG_BB_AA`), so a field added in a future egui is still checked.
    fn colors(visuals: &Visuals) -> Vec<Color32> {
        let dbg = format!("{visuals:?}");
        let hex = |s: &str| u8::from_str_radix(s, 16).expect("hex byte");
        dbg.match_indices('#')
            .filter_map(|(i, _)| dbg.get(i + 1..i + 12))
            .map(|c| {
                let [r, g, b, a] = [&c[0..2], &c[3..5], &c[6..8], &c[9..11]].map(hex);
                Color32::from_rgba_premultiplied(r, g, b, a)
            })
            .collect()
    }

    #[test]
    fn visuals_use_only_the_grayscale_palette() {
        let v = next_style().visuals;
        let found = colors(&v);
        assert!(found.len() > 20, "Debug scan found only {found:?}");
        for c in found {
            assert!(c.r() == c.g() && c.g() == c.b(), "{c:?} is not gray");
            assert!(ALLOWED.contains(&c), "{c:?} is not in the palette");
        }
        assert_eq!(v.selection.bg_fill, LIGHT_GRAY);
        assert_eq!(v.panel_fill, LIGHT_GRAY);
        assert_eq!(v.window_fill, LIGHT_GRAY);
        assert_eq!(v.extreme_bg_color, WHITE);
        assert_eq!(v.text_color(), BLACK);
        assert_eq!(v.weak_text_color(), DARK_GRAY);
    }

    #[test]
    fn install_fonts_puts_arimo_first_and_registers_bold() {
        let ctx = egui::Context::default();
        ctx.set_style(next_style());
        install_fonts(&ctx);
        // Fonts are swapped in at the start of a pass; laying out a heading
        // proves the bold family is bound (an unbound one panics).
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| ui.heading("Improv"));
        });
        ctx.fonts(|f| {
            let fonts = f.lock();
            let defs = fonts.fonts.definitions();
            let prop = &defs.families[&FontFamily::Proportional];
            assert_eq!(prop[0], "Arimo");
            assert!(prop.len() > 1, "egui's fonts must stay as fallback");
            assert_eq!(defs.families[&FontFamily::Name(BOLD.into())][0], BOLD);
        });
        let heading = &ctx.style().text_styles[&TextStyle::Heading];
        assert_eq!(*heading, FontId::new(12.0, FontFamily::Name(BOLD.into())));
        assert_eq!(ctx.style().text_styles[&TextStyle::Body].size, 12.0);
    }

    #[test]
    fn apply_pins_the_light_slot() {
        let ctx = egui::Context::default();
        apply(&ctx);
        // What a system ThemeChanged to light does.
        ctx.set_theme(egui::Theme::Light);
        assert_eq!(ctx.style().visuals.selection.bg_fill, LIGHT_GRAY);
    }

    #[test]
    fn corners_are_square_and_strokes_one_pixel() {
        let v = next_style().visuals;
        assert_eq!(v.window_rounding, Rounding::ZERO);
        assert_eq!(v.menu_rounding, Rounding::ZERO);
        let w = &v.widgets;
        for s in [w.noninteractive, w.inactive, w.hovered, w.active, w.open] {
            assert_eq!(s.rounding, Rounding::ZERO);
            assert_eq!(s.bg_stroke.width, 1.0);
            assert_eq!(s.expansion, 0.0);
        }
    }

    #[test]
    fn grouped_arrows_sit_at_the_trough_bottom_and_scroll() {
        let ctx = egui::Context::default();
        ctx.set_style(next_style());
        let frame = |events: Vec<egui::Event>| {
            let mut shown = None;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0))),
                events,
                ..Default::default()
            };
            let full = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let area = egui::ScrollArea::vertical().max_height(200.0);
                    let s = ui.scope(|ui| {
                        next_scroll(ui, area, |ui| {
                            for i in 0..50 {
                                ui.label(format!("row {i}"));
                            }
                        })
                    });
                    shown = Some((s.inner.id, s.response.rect));
                });
            });
            let (id, inner) = shown.expect("scroll area shown");
            (full, id, inner)
        };

        let (full, id, outer) = frame(Vec::new());
        let [trough, up, down] = scroller_rects(outer, 19.0);
        assert_eq!(trough.height(), 200.0, "the trough is the area's height");
        assert_eq!(down.max, trough.max, "down arrow ends the trough");
        assert_eq!(up.max.y, down.min.y, "up arrow sits right above it");
        assert_eq!((up.width(), up.height()), (ARROW, ARROW));
        for button in [up, down] {
            let painted = full.shapes.iter().any(|c| match &c.shape {
                egui::Shape::Path(p) => {
                    p.points.len() == 3 && p.points.iter().all(|q| button.contains(*q))
                }
                _ => false,
            });
            assert!(painted, "a triangle is painted in {button:?}");
        }
        // The up arrow's tip is its topmost point.
        let tips: Vec<f32> = full
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Path(p) if p.points.len() == 3 && up.contains(p.points[0]) => {
                    Some(p.points[0].y)
                }
                _ => None,
            })
            .collect();
        assert!(
            tips.first().is_some_and(|y| *y < up.center().y),
            "up points up"
        );

        let at = down.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let _ = frame(vec![
            egui::Event::PointerMoved(at),
            press(true),
            press(false),
        ]);
        let _ = frame(Vec::new());
        let offset = egui::scroll_area::State::load(&ctx, id)
            .expect("state")
            .offset
            .y;
        // Exactly one line (17 px row + 4 px spacing), not a trough jump.
        assert_eq!(offset, 21.0, "down arrow scrolled one line");
    }

    /// The horizontal scroller: arrows grouped at the bar's left end; the
    /// right arrow scrolls a line, the left one clamps at 0, a trough click
    /// right of the knob pages by a view, and the knob drags.
    #[test]
    fn horizontal_scroller_groups_arrows_left_and_scrolls() {
        let ctx = egui::Context::default();
        ctx.set_style(next_style());
        let bar = Rect::from_min_size(pos2(10.0, 260.0), vec2(300.0, 19.0));
        let frame = |events: Vec<egui::Event>| {
            let mut out = None;
            let raw = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(400.0, 300.0))),
                events,
                ..Default::default()
            };
            let full = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let o = egui::ScrollArea::horizontal()
                        .max_width(200.0)
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.allocate_exact_size(vec2(1000.0, 20.0), egui::Sense::hover())
                        });
                    hscroller_chrome(ui, &o, bar, 20.0);
                    out = Some((o.state.offset.x, o.inner_rect.width()));
                });
            });
            (full, out.unwrap())
        };
        let click = |at: egui::Pos2| {
            let b = |pressed| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frame(vec![egui::Event::PointerMoved(at)]);
            frame(vec![b(true)]);
            frame(vec![b(false)]);
            frame(Vec::new()).1
        };
        let (full, (x, view)) = frame(Vec::new());
        assert_eq!((x, view), (0.0, 200.0));
        let [left, right, track] = hscroller_rects(bar);
        assert_eq!(left.min, pos2(bar.min.x, bar.max.y - ARROW));
        assert_eq!(right.min.x, left.max.x, "the arrows are grouped");
        assert_eq!(track.min.x, right.max.x);
        let tips: Vec<egui::Pos2> = full
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Path(p) if p.points.len() == 3 => Some(p.points[0]),
                _ => None,
            })
            .collect();
        assert!(
            tips.iter()
                .any(|t| left.contains(*t) && t.x < left.center().x),
            "left points left"
        );
        assert!(
            tips.iter()
                .any(|t| right.contains(*t) && t.x > right.center().x),
            "right points right"
        );

        assert_eq!(click(left.center()).0, 0.0, "clamped at the start");
        assert_eq!(click(right.center()).0, 20.0, "one line right");
        assert_eq!(
            click(pos2(track.max.x - 2.0, track.center().y)).0,
            220.0,
            "a page right"
        );
        // Drag the knob 10 px: the free travel maps onto the 800 px scroll.
        let (x0, x1) = knob_span(220.0, 200.0, 1000.0, track.min.x..=track.max.x, 19.0);
        let from = pos2((x0 + x1) / 2.0, track.center().y);
        let to = from + vec2(10.0, 0.0);
        let b = |at, pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(vec![egui::Event::PointerMoved(from)]);
        frame(vec![b(from, true)]);
        frame(vec![egui::Event::PointerMoved(to)]);
        frame(vec![b(to, false)]);
        let after = frame(Vec::new()).1 .0;
        let free = track.width() - (x1 - x0);
        assert!(
            (after - (220.0 + 10.0 * 800.0 / free)).abs() < 0.5,
            "{after}"
        );
    }

    #[test]
    fn scrollers_are_solid_and_19_px() {
        let s = next_style().spacing.scroll;
        assert_eq!(s.bar_width, 19.0);
        assert!(!s.floating);
        assert!(!s.foreground_color);
    }
}
