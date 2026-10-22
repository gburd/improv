//! `fetch` over the server's JSON API, and the page hooks a test reads.

use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Send a request; `done` gets the body, or the error (an HTTP error carries
/// the server's JSON message).
pub fn fetch(
    method: &'static str,
    url: String,
    body: Option<String>,
    token: Option<String>,
    done: impl FnOnce(Result<String, String>) + 'static,
) {
    wasm_bindgen_futures::spawn_local(async move {
        let r = text(method, &url, body, token).await;
        done(r.map_err(|e| e.as_string().unwrap_or_else(|| format!("{e:?}"))));
    });
}

async fn text(
    method: &str,
    url: &str,
    body: Option<String>,
    token: Option<String>,
) -> Result<String, JsValue> {
    let init = web_sys::RequestInit::new();
    init.set_method(method);
    if let Some(b) = &body {
        init.set_body(&JsValue::from_str(b));
    }
    let req = web_sys::Request::new_with_str_and_init(url, &init)?;
    req.headers().set("content-type", "application/json")?;
    if let Some(t) = token {
        req.headers().set("authorization", &format!("Bearer {t}"))?;
    }
    let window = web_sys::window().ok_or("no window")?;
    let resp: web_sys::Response = JsFuture::from(window.fetch_with_request(&req))
        .await?
        .dyn_into()?;
    let text = JsFuture::from(resp.text()?)
        .await?
        .as_string()
        .unwrap_or_default();
    if !resp.ok() {
        return Err(JsValue::from_str(&format!(
            "HTTP {}: {text}",
            resp.status()
        )));
    }
    Ok(text)
}

/// Publish the app's state for a headless test: the page title, and each
/// visible cell's position and text as JSON on `<body data-cells>`.
pub fn expose(title: &str, cells: &str) {
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else {
        return;
    };
    if doc.title() != title {
        doc.set_title(title);
    }
    if let Some(body) = doc.body() {
        if body.get_attribute("data-cells").as_deref() != Some(cells) {
            let _ = body.set_attribute("data-cells", cells);
        }
    }
}
