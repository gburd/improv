# improv_gui_web: the GUI in a browser

The desktop GUI's NeXT worksheet, drawn by egui/eframe 0.37 into a WebGL
canvas, over `improv-server`'s `/v1` API (`improv_client::wire`). The
model, the engine and the store stay in the server; this crate draws and
sends edits. User guide: [`docs/src/web-gui.md`](../../docs/src/web-gui.md).

- `src/layout.rs`: a worksheet's pivot and the cell cache (value, blank,
  not loaded). Pure; built and tested on every target.
- `src/net.rs`: the remote `ModelService` (`fetch`, and the change stream
  read with `fetch`, with backoff and `?since=` resume).
- `src/app.rs`: the worksheet windows, gutters and tiles, grid, formula
  pane, menus, model browser and login.
- `src/edit.rs`: the formula edit with autocomplete.
- `crates/gui`'s `theme`, `nxwin`, `menu` and `formula_highlight` compile
  in unchanged through `#[path]`.

Off wasm32 the crate is `layout` alone, so `cargo build --workspace`
builds it everywhere.

```sh
./build.sh [--release] [DIR]      # DIR defaults to target/gui_web
improv-server model.db --static DIR
e2e/e2e.sh <improv> <improv-server> DIR <out-dir>   # headless Chromium
```
