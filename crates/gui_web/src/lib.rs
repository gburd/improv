//! Improv's GUI in a browser (road-to-1.0 I2): the desktop GUI's NeXT
//! worksheet, drawn by egui into a WebGL canvas. The model, the engine and
//! the store stay in `improv-server`; the browser draws what the server
//! sends over its `/v1` API and the change stream, and computes nothing.
//!
//! The look is the desktop's own code: `crates/gui`'s `theme`, `nxwin`,
//! `menu` and `formula_highlight`, and the worksheet app itself
//! (`served.rs`, which improv-gui runs for a server model), compiled
//! unchanged through `#[path]` (they name each other as `crate::theme`,
//! so they sit at the root), and `improv_client`'s autocomplete and
//! located errors.
//!
//! Off wasm32 the crate is its pure part alone, the accessibility tree's
//! DOM model ([`a11y`]), so the workspace builds and tests it on every
//! platform. The pivot, cell cache and grid cursor are
//! `improv_client::layout`, tested natively.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod a11y;

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
#[path = "../../gui/src/theme.rs"]
mod theme;

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
#[path = "../../gui/src/nxwin.rs"]
mod nxwin;

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
#[path = "../../gui/src/menu.rs"]
mod menu;

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
#[path = "../../gui/src/formula_highlight.rs"]
mod formula_highlight;

// The worksheet app, shared with improv-gui's server models.
#[cfg(target_arch = "wasm32")]
#[path = "../../gui/src/served.rs"]
mod app;
#[cfg(target_arch = "wasm32")]
mod bridge;
#[cfg(target_arch = "wasm32")]
mod net;
#[cfg(target_arch = "wasm32")]
#[path = "../../gui/src/served_edit.rs"]
mod served_edit;
#[cfg(target_arch = "wasm32")]
mod web;
