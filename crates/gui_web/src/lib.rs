//! Improv's GUI in a browser (road-to-1.0 I2): the desktop GUI's NeXT
//! worksheet, drawn by egui into a WebGL canvas. The model, the engine and
//! the store stay in `improv-server`; the browser draws what the server
//! sends over its `/v1` API and the change stream, and computes nothing.
//!
//! The look is the desktop's own code: `crates/gui`'s `theme`, `nxwin`,
//! `menu` and `formula_highlight`, compiled unchanged through `#[path]`
//! (they name each other as `crate::theme`, so they sit at the root), and
//! `improv_client`'s autocomplete and located errors.
//!
//! Off wasm32 the crate is its pure layout code alone, so the workspace
//! builds and tests it on every platform.

pub mod layout;

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

#[cfg(target_arch = "wasm32")]
mod app;
#[cfg(target_arch = "wasm32")]
mod edit;
#[cfg(target_arch = "wasm32")]
mod net;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub(crate) use web::expose;
