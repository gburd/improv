//! The browser entry point.

use std::rc::Rc;

use wasm_bindgen::prelude::*;

use crate::{app, net};

/// Run by the generated JS glue. The token is asked for at login and held
/// in memory: never from the URL, a cookie or storage.
#[wasm_bindgen(start)]
pub fn start() {
    let Some(window) = web_sys::window() else {
        return;
    };
    let canvas = window
        .document()
        .and_then(|d| d.get_element_by_id("improv"))
        .and_then(|e| e.dyn_into::<web_sys::HtmlCanvasElement>().ok());
    let Some(canvas) = canvas else {
        expose(
            "improv-web: no <canvas id=\"improv\">",
            &app::Probe::default(),
        );
        return;
    };
    // One random session per page load: undo is per session.
    let session = format!("{:016x}", (js_sys::Math::random() * 2f64.powi(53)) as u64);
    wasm_bindgen_futures::spawn_local(async move {
        let mirror = canvas.clone();
        let started = eframe::WebRunner::new()
            .start(
                canvas,
                eframe::WebOptions::default(),
                Box::new(move |cc| {
                    crate::bridge::install(&cc.egui_ctx, &mirror);
                    let ctx = cc.egui_ctx.clone();
                    let repaint: Rc<dyn Fn()> = Rc::new(move || ctx.request_repaint());
                    let svc = net::Remote::new(session, repaint);
                    // A server without tokens takes an empty one.
                    let probe = Some(String::new());
                    Ok(Box::new(Shell(app::WebApp::new(&cc.egui_ctx, svc, probe))))
                }),
            )
            .await;
        if let Err(e) = started {
            expose(
                &format!("improv-web: failed to start: {e:?}"),
                &app::Probe::default(),
            );
        }
    });
}

struct Shell(app::WebApp<net::Remote>);

impl eframe::App for Shell {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.frame(ui);
    }
}

/// Publish the app's state for tests that drive a canvas: the page
/// title, and on `<body>` as JSON: `data-cells` (each visible cell's
/// position and text), `data-tiles`, `data-zones`, `data-newformula` and
/// `data-formulaerror`.
pub(crate) fn expose(title: &str, p: &app::Probe) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if doc.title() != title {
        doc.set_title(title);
    }
    let Some(body) = doc.body() else {
        return;
    };
    let arr = |v: &[serde_json::Value]| serde_json::Value::Array(v.to_vec()).to_string();
    let opt = |v: &Option<serde_json::Value>| v.clone().unwrap_or_default().to_string();
    for (k, v) in [
        ("data-cells", arr(&p.cells)),
        ("data-tiles", arr(&p.tiles)),
        ("data-zones", arr(&p.zones)),
        ("data-pages", arr(&p.pages)),
        ("data-pageitems", arr(&p.pageitems)),
        ("data-links", arr(&p.links)),
        ("data-browserbutton", opt(&p.browserbutton)),
        ("data-newformula", opt(&p.newformula)),
        ("data-formulaerror", opt(&p.formulaerror)),
        ("data-live", p.live.to_string()),
    ] {
        if body.get_attribute(k).as_deref() != Some(&v) {
            let _ = body.set_attribute(k, &v);
        }
    }
}
