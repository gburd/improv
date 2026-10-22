//! Spike for road-to-1.0 task I1: the desktop GUI's NeXTSTEP worksheet in a
//! browser. egui/eframe draws it into a WebGL canvas; the model, the engine
//! and the store stay in `improv-server`, reached over its JSON API.
//!
//! The theme and the window chrome are the desktop GUI's own files, compiled
//! unchanged (`#[path]`); only the worksheet and the HTTP client are new.

#[allow(dead_code)]
#[path = "../../gui/src/theme.rs"]
mod theme;

#[allow(dead_code)]
#[path = "../../gui/src/nxwin.rs"]
mod nxwin;

mod app;
mod net;

use wasm_bindgen::prelude::*;

/// Entry point, run by the generated JS glue. `?probe` runs the engine probe
/// instead of the app; `#token=…` is sent as the bearer token.
#[wasm_bindgen(start)]
pub fn start() {
    let window = web_sys::window().expect("a window");
    let location = window.location();
    #[cfg(feature = "probe")]
    if location.search().unwrap_or_default().contains("probe") {
        probe();
        return;
    }
    let token = location
        .hash()
        .unwrap_or_default()
        .strip_prefix("#token=")
        .map(str::to_owned);
    let canvas = window
        .document()
        .and_then(|d| d.get_element_by_id("improv"))
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok())
        .expect("a <canvas id=\"improv\">");
    wasm_bindgen_futures::spawn_local(async move {
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| Ok(Box::new(app::WebApp::new(cc, "api".into(), token)))),
            )
            .await;
        if let Err(e) = started {
            net::expose(&format!("improv: failed to start: {e:?}"), "{}");
        }
    });
}

/// Does the engine (timely + differential dataflow) run in the browser? Puts
/// the answer, or the panic, in the page title.
#[cfg(feature = "probe")]
fn probe() {
    use improv_core_model::{CategoryId, ItemId, Measure, MeasureId, MeasureKind, Model, Name};
    use improv_core_model::{Coordinate, Value, ValueType};
    std::panic::set_hook(Box::new(|info| {
        net::expose(&format!("probe panicked: {info}"), "{}");
    }));
    let mut m = Model::new();
    m.add_category(CategoryId(1), "Product");
    m.add_item(ItemId(10), CategoryId(1), "WidgetA");
    let input = |id, name: &str| Measure {
        id: MeasureId(id),
        name: Name(name.into()),
        value_type: ValueType::Number,
        categories: vec![CategoryId(1)],
        kind: MeasureKind::Input,
        description: None,
    };
    m.add_measure(input(100, "Price"));
    m.add_measure(input(101, "Quantity"));
    let at = Coordinate::from_pairs([(CategoryId(1), ItemId(10))]);
    m.set_input(MeasureId(100), at.clone(), Value::Number(10.0));
    m.set_input(MeasureId(101), at, Value::Number(7.0));
    let f = improv_core_model::parser::parse_expr(&m, "Price * Quantity").expect("parses");
    let mut revenue = input(102, "Revenue");
    revenue.kind = MeasureKind::Derived(f);
    m.add_measure(revenue);
    match improv_engine::dataflow::evaluate(&m, &[MeasureId(102)]) {
        Ok(out) => {
            let cells = out.get(&MeasureId(102)).map_or(0, |c| c.len());
            net::expose(&format!("probe ok: {cells} cell"), "{}");
        }
        Err(e) => net::expose(&format!("probe error: {e}"), "{}"),
    }
}
