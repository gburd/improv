//! A NeXTSTEP-flavored egui theme for the Improv GUI.
//!
//! Lotus Improv ran on NeXTSTEP, whose UI was a light neutral gray with
//! beveled/embossed controls, black text, and a restrained accent. This module
//! builds an [`egui::Style`] that approximates that look-and-feel: the classic
//! `~0.66` gray desktop, chiseled button bevels (light top-left / dark
//! bottom-right via stroke + fill), square-ish corners, and a muted blue
//! selection. It is purely cosmetic — no behavior depends on it.

use egui::{Color32, Rounding, Stroke, Style, Visuals};

/// The canonical NeXTSTEP window gray (~2/3 white).
pub const NEXT_GRAY: Color32 = Color32::from_rgb(0xAA, 0xAA, 0xAA);
/// A slightly lighter gray for raised control faces.
pub const NEXT_LIGHT: Color32 = Color32::from_rgb(0xBE, 0xBE, 0xBE);
/// A darker gray for grooves / pressed faces.
pub const NEXT_DARK: Color32 = Color32::from_rgb(0x80, 0x80, 0x80);
/// Near-white bevel highlight (top-left edge).
pub const BEVEL_LIGHT: Color32 = Color32::from_rgb(0xF0, 0xF0, 0xF0);
/// Bevel shadow (bottom-right edge).
pub const BEVEL_SHADOW: Color32 = Color32::from_rgb(0x55, 0x55, 0x55);
/// The NeXT selection blue.
pub const NEXT_BLUE: Color32 = Color32::from_rgb(0x30, 0x50, 0x90);
/// Content panel white (cell grid, text fields).
pub const PAPER: Color32 = Color32::from_rgb(0xF2, 0xF2, 0xF2);

/// Build the NeXTSTEP-style [`Style`]. Applied once at startup via
/// [`egui::Context::set_style`].
pub fn next_style() -> Style {
    let mut style = Style::default();
    let mut v = Visuals::light();

    // Squared corners everywhere — NeXT controls were near-rectangular.
    let sharp = Rounding::same(1.0_f32);

    v.window_fill = NEXT_GRAY;
    v.panel_fill = NEXT_GRAY;
    v.extreme_bg_color = PAPER; // text edit / grid background
    v.faint_bg_color = Color32::from_rgb(0xC4, 0xC4, 0xC4); // striped rows
    v.window_rounding = sharp;
    v.window_stroke = Stroke::new(1.0_f32, BEVEL_SHADOW);
    v.selection.bg_fill = NEXT_BLUE;
    v.selection.stroke = Stroke::new(1.0_f32, BEVEL_LIGHT);
    v.hyperlink_color = NEXT_BLUE;
    v.override_text_color = Some(Color32::from_gray(0x10));

    let w = &mut v.widgets;
    // Non-interactive (labels, frames): flat gray, faint groove.
    w.noninteractive.bg_fill = NEXT_GRAY;
    w.noninteractive.weak_bg_fill = NEXT_GRAY;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, NEXT_DARK);
    w.noninteractive.rounding = sharp;
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, Color32::from_gray(0x10));

    // Idle controls: raised light face with a dark edge (chiseled).
    w.inactive.bg_fill = NEXT_LIGHT;
    w.inactive.weak_bg_fill = NEXT_LIGHT;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, BEVEL_SHADOW);
    w.inactive.rounding = sharp;
    w.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_gray(0x10));

    // Hovered: highlight bevel.
    w.hovered.bg_fill = BEVEL_LIGHT;
    w.hovered.weak_bg_fill = BEVEL_LIGHT;
    w.hovered.bg_stroke = Stroke::new(1.5_f32, BEVEL_SHADOW);
    w.hovered.rounding = sharp;
    w.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::BLACK);

    // Active/pressed: sunken dark face.
    w.active.bg_fill = NEXT_DARK;
    w.active.weak_bg_fill = NEXT_DARK;
    w.active.bg_stroke = Stroke::new(1.0_f32, BEVEL_LIGHT);
    w.active.rounding = sharp;
    w.active.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);

    // Open menus/combos.
    w.open.bg_fill = NEXT_LIGHT;
    w.open.bg_stroke = Stroke::new(1.0_f32, BEVEL_SHADOW);
    w.open.rounding = sharp;

    style.visuals = v;
    // Tighter, denser spacing like the original.
    style.spacing.item_spacing = egui::vec2(6.0, 4.0);
    style.spacing.button_padding = egui::vec2(8.0, 3.0);
    style
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_is_next_gray_and_squared() {
        let s = next_style();
        assert_eq!(s.visuals.panel_fill, NEXT_GRAY);
        assert_eq!(s.visuals.selection.bg_fill, NEXT_BLUE);
        // Squared (near-zero) corners on idle widgets.
        assert!(s.visuals.widgets.inactive.rounding.nw <= 1.0);
        // Raised face is lighter than the desktop gray.
        assert!(NEXT_LIGHT.r() > NEXT_GRAY.r());
    }
}
