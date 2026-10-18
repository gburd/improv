//! Improv desktop GUI (`improv-gui`) — Phase 5.
//!
//! An `egui`/`eframe` front-end: a *view* over the engine (see
//! `.agent/steering/AGENT_GUI_STEERING.md` §9). It loads a model from a store
//! (a Mentat file or a `postgres://…?model=` URL), renders a measure as a pivot grid, and (Phase 5 increments) offers a
//! model browser, formula editor, and Info panel.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

mod app;
mod chart;
mod completion;
mod csv_wizard;
mod diagnostics;
mod formula_highlight;
mod function_help;
mod menu;
mod nxwin;
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

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size(app::INITIAL_WINDOW)
            .with_min_inner_size(app::MIN_WINDOW),
        ..Default::default()
    };
    eframe::run_native(
        "Improv",
        native_options,
        Box::new(|cc| {
            // Apply the NeXTSTEP look-and-feel once at startup.
            theme::apply(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
}
