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
// Declarations only (the clap derive).
#[cfg_attr(coverage_nightly, coverage(off))]
mod args;
mod chart;
mod completion;
mod csv_wizard;
mod diagnostics;
mod formula_highlight;
mod function_help;
mod menu;
mod nxwin;
mod theme;

/// Tests run egui frames by hand and never paint them; egui 0.37 asserts
/// (in debug builds) that a frame's texture deltas were handled. These run a
/// frame and drop its deltas, so a test may drop or keep the output.
#[cfg(test)]
pub(crate) trait TestFrames {
    fn test_pass(&self, raw: egui::RawInput, f: impl FnMut(&egui::Context)) -> egui::FullOutput;
    fn test_ui(&self, raw: egui::RawInput, f: impl FnMut(&mut egui::Ui)) -> egui::FullOutput;
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl TestFrames for egui::Context {
    fn test_pass(&self, raw: egui::RawInput, f: impl FnMut(&egui::Context)) -> egui::FullOutput {
        let mut out = self.run_pass(raw, f);
        out.textures_delta.clear();
        out
    }
    fn test_ui(&self, raw: egui::RawInput, f: impl FnMut(&mut egui::Ui)) -> egui::FullOutput {
        let mut out = self.run_ui(raw, f);
        out.textures_delta.clear();
        out
    }
}

use app::ImprovApp;
use clap::Parser;

fn main() -> eframe::Result<()> {
    // No model is "": an in-memory scratch model.
    let db = args::Cli::parse().model.unwrap_or_default();

    // The store opens here (a bad path fails before any window); its model
    // loads in the background, behind a "Loading…" desk.
    let app = match ImprovApp::open(&db) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("improv-gui: {e}");
            std::process::exit(1);
        }
    };

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(app.native_title())
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
