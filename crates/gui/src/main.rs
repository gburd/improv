//! Improv desktop GUI (`improv-gui`) — Phase 5.
//!
//! An `egui`/`eframe` front-end: a *view* over the engine (see
//! `.agent/steering/AGENT_GUI_STEERING.md` §9). It loads a model from a Mentat
//! store, renders a measure as a pivot grid, and (Phase 5 increments) offers a
//! model explorer, formula editor, and inspector.

mod app;
mod chart;
mod theme;

use app::ImprovApp;

fn main() -> eframe::Result<()> {
    // Model store path from argv[1]; "" is an in-memory scratch model.
    let db = std::env::args().nth(1).unwrap_or_default();

    let app = match ImprovApp::load(&db) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("improv-gui: {e}");
            std::process::exit(1);
        }
    };

    let native_options = eframe::NativeOptions::default();
    eframe::run_native(
        "Improv",
        native_options,
        Box::new(|cc| {
            // Apply the NeXTSTEP look-and-feel once at startup.
            cc.egui_ctx.set_style(theme::next_style());
            Ok(Box::new(app))
        }),
    )
}
