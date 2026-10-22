# improv_gui_web: the web GUI spike (I1)

**A spike, not a product.** It lives only on the branch `work/web-spike`.
It is its own Cargo workspace, so the root build, `Cargo.lock` and CI never
see it. The decision it supports is
[`docs/reviews/2026-10-22-web-gui.md`](../../docs/reviews/2026-10-22-web-gui.md).

The desktop GUI's NeXTSTEP worksheet, drawn by egui/eframe 0.29 into a
WebGL canvas. The model, the engine and the store stay in `improv-server`,
reached over its JSON API (`GET /model`, `GET /measures`,
`GET /measures/:id/values`, `POST /measures/:id/cells`). The theme and
the window chrome are `crates/gui/src/theme.rs` and `nxwin.rs`, compiled
unchanged through `#[path]`. Only the worksheet (`src/app.rs`) and the
`fetch` client (`src/net.rs`) are new.

It opens the server's model with one worksheet window per measure, and
pages any category beyond two at its first item. The windows scroll. A
double-click edits an input cell, and Enter writes it. Every measure is
then re-read, so derived cells show the server's recalculated values.

## Build and run

Needs the `wasm32-unknown-unknown` target and `wasm-bindgen-cli` 0.2.127,
the version the lock pins:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.127 --locked
./build.sh --release                 # -> web/improv_gui_web{.js,_bg.wasm}
improv-server m.db 127.0.0.1:3000 &
./serve.py 8080 http://127.0.0.1:3000   # web/ plus /api/* -> the server
# open http://127.0.0.1:8080/  (#token=… sends a bearer token)
```

`serve.py` keeps the page and the API on one origin. improv-server neither
serves static files nor sends CORS headers, so the spike needs it.

## The end-to-end check

`./e2e.sh <improv> <improv-server> <out.png>` builds a Time × Product model
with the CLI and serves it. It then drives the page in headless Chromium
over the DevTools protocol (`drive.py`, needs `python3-websocket`):

1. load (about 0.7 s);
2. scroll the Revenue window to its last row;
3. double-click Quantity[2024Q1, Widget], type 150, press Enter;
4. wait for Revenue[2024Q1, Widget] to become Price × 150 = 1800;
5. screenshot, and check the store holds 150.

The app publishes its visible cells (position and text) on
`<body data-cells>` and its status in the page title. That is how the
check reads a canvas.

`./build.sh --features probe` adds `?probe`: it runs the engine
(`improv_engine::dataflow::evaluate`) in the browser. It panics with `time
not implemented on this platform`, because timely's `Worker::new` calls
`std::time::Instant::now`. The engine compiles to wasm32, but it does not
run there unpatched.
