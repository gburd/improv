//! The formula editors' text edit: highlighted, the error underlined, and
//! autocomplete from [`improv_client::completion`] (the desktop's), on
//! the structure the server sent.

use egui::{Key, Modifiers};
use improv_client::completion::{self, Popup};
use improv_client::diagnostics::Diagnostic;
use improv_core_model::Model;

/// Byte offset of the caret in text edit `id` over `text`.
fn caret(ctx: &egui::Context, id: egui::Id, text: &str) -> usize {
    egui::TextEdit::load_state(ctx, id)
        .and_then(|s| s.cursor.char_range())
        .map_or(text.len(), |r| {
            text.char_indices()
                .nth(r.primary.index.0)
                .map_or(text.len(), |(b, _)| b)
        })
}

fn set_caret(ctx: &egui::Context, id: egui::Id, text: &str, at: usize) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let c = egui::text::CCursor::new(text[..at.min(text.len())].chars().count());
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(c)));
    state.store(ctx, id);
}

/// A formula edit with autocomplete: Up/Down pick, Tab accepts, Esc
/// closes the list; the list shows under the edit.
pub fn assisted(
    ui: &mut egui::Ui,
    model: &Model,
    text: &mut String,
    id: egui::Id,
    hint: &str,
    span: Option<std::ops::Range<usize>>,
) -> egui::Response {
    let ctx = ui.ctx().clone();
    let pid = id.with("popup");
    let mut popup: Popup = ctx.data_mut(|d| d.get_temp(pid)).unwrap_or_default();
    let word =
        |t: &str, c: usize| completion::prefix(t, c).map_or(String::new(), |(_, w)| w.to_string());
    let mut accepted = false;
    // Tab accepts a completion while the list is open; else it moves on
    // (a keyboard user leaves the field with it).
    let mut listing = false;
    if ctx.memory(|m| m.has_focus(id)) {
        let at = caret(&ctx, id, text);
        let cands = completion::offer(model, text, at);
        if popup.sync(text, &word(text, at), cands.len()) {
            listing = true;
            let (up, down, tab, esc) = ui.input_mut(|i| {
                let mut k = |key| i.consume_key(Modifiers::NONE, key);
                (
                    k(Key::ArrowUp),
                    k(Key::ArrowDown),
                    k(Key::Tab),
                    k(Key::Escape),
                )
            });
            popup.step(isize::from(down) - isize::from(up), cands.len());
            if tab {
                let (t, c) = completion::accept(text, at, &cands[popup.sel]);
                *text = t;
                set_caret(&ctx, id, text, c);
                accepted = true;
            } else if esc {
                popup.dismiss(text);
            }
        }
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut layouter = move |ui: &egui::Ui, t: &dyn egui::TextBuffer, wrap: f32| {
        let mut job =
            crate::formula_highlight::highlight_formula(t.as_str(), font.clone(), span.clone());
        job.wrap.max_width = wrap;
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let out = egui::TextEdit::singleline(text)
        .id(id)
        .hint_text(hint)
        .lock_focus(listing)
        .desired_width(f32::INFINITY)
        .layouter(&mut layouter)
        .show(ui);
    let mut resp = out.response.response;
    if accepted {
        resp.request_focus();
        resp.mark_changed();
    }
    if resp.has_focus() {
        let at = caret(&ctx, id, text);
        let cands = completion::offer(model, text, at);
        if !cands.is_empty() && popup.sync(text, &word(text, at), cands.len()) {
            let below = resp.rect.left_bottom();
            egui::Area::new(id.with("list"))
                .order(egui::Order::Foreground)
                .fixed_pos(below)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        for (k, c) in cands.iter().enumerate() {
                            let line = format!("{}  {}", c.name, c.hint());
                            let _ = ui.selectable_label(k == popup.sel, line);
                        }
                    });
                });
        }
    }
    ctx.data_mut(|d| d.insert_temp(pid, popup));
    resp
}

/// A formula error under its edit: the message, and the hint dimmed.
pub fn error_lines(ui: &mut egui::Ui, d: &Diagnostic) {
    let r = ui.colored_label(
        crate::formula_highlight::ERROR_COLOR,
        format!("⚠ {}", d.message),
    );
    // Announced when it appears.
    ui.ctx().accesskit_node_builder(r.id, |n| {
        n.set_role(egui::accesskit::Role::Alert);
    });
    if let Some(h) = &d.hint {
        ui.label(egui::RichText::new(h).color(crate::theme::DARK_GRAY));
    }
}
