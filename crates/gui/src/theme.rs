//! The NeXTSTEP egui theme for the Improv GUI.
//!
//! Lotus Improv ran on NeXTSTEP's 2-bit grayscale display: four grays and no
//! color at all. This module builds an [`egui::Style`] from exactly those four
//! levels (plus the scroller trough), with square corners and 1 px strokes.
//! Roles follow `docs/reviews/2026-10-03-nextstep-visual-spec.md` §1. It is
//! purely cosmetic: no behavior depends on it.

use egui::{Color32, Rounding, Shadow, Stroke, Style, Visuals};

/// Key-window title bars, grid rules, text, bevel dark edges.
pub const BLACK: Color32 = Color32::from_gray(0x00);
/// Desktop, category tile faces, non-key title bars, disabled / weak text.
pub const DARK_GRAY: Color32 = Color32::from_gray(0x55);
/// Window and panel background, control faces, the selection fill.
pub const LIGHT_GRAY: Color32 = Color32::from_gray(0xAA);
/// Grid cells, text fields, pressed faces, bevel light edges.
pub const WHITE: Color32 = Color32::from_gray(0xFF);
/// The scroller trough (the one value outside the four levels).
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

/// Build the NeXTSTEP [`Style`]. Applied once at startup via
/// [`egui::Context::set_style`].
pub fn next_style() -> Style {
    let mut style = Style::default();
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
}
