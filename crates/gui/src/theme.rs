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

/// Repaint a shown vertical scroller NeXT-style: a knob with a 1 px bevel
/// (white top-left, black bottom-right) above two grouped arrow buttons, each
/// click scrolling `line` points. `outer` is the area plus its bar as laid
/// out (`out.inner_rect` is the pre-shrink rect, so it can't be used).
// ponytail: egui still owns knob drags and trough clicks, mapped over the
// WHOLE trough, so the knob (drawn in the trough minus the arrows) trails the
// pointer by 2*ARROW/height; a hand-rolled scroller fixes it if it matters.
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
    let at = |y: f32| egui::remap_clamp(y, 0.0..=out.content_size.y.max(view), track.clone());
    let (mut y0, mut y1) = (at(out.state.offset.y), at(out.state.offset.y + view));
    let grow = (scroll.handle_min_length - (y1 - y0)).max(0.0) / 2.0;
    (y0, y1) = (
        (y0 - grow).max(*track.start()),
        (y1 + grow).min(*track.end()),
    );
    raised(
        p,
        p.round_rect_to_pixels(Rect::from_x_y_ranges(trough.x_range(), y0..=y1)),
    );
    let max = (out.content_size.y - view).max(0.0);
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
                egui::Sense::click(),
            )
            .clicked()
        {
            let mut state = out.state;
            state.offset.y = (state.offset.y + sign * line).clamp(0.0, max);
            state.store(ui.ctx(), out.id);
            ui.ctx().request_repaint();
        }
    }
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

    #[test]
    fn scrollers_are_solid_and_19_px() {
        let s = next_style().spacing.scroll;
        assert_eq!(s.bar_width, 19.0);
        assert!(!s.floating);
        assert!(!s.foreground_color);
    }
}
