#!/usr/bin/env bash
# Build the spike for the browser: cargo (wasm32) + wasm-bindgen into web/.
#   build.sh [--release] [--features probe]
set -euo pipefail
cd "$(dirname "$0")"
profile=debug
args=()
for a in "$@"; do
  [ "$a" = --release ] && profile=release
  args+=("$a")
done
cargo build --target wasm32-unknown-unknown "${args[@]}"
wasm-bindgen --target web --no-typescript --out-dir web \
  "target/wasm32-unknown-unknown/$profile/improv_gui_web.wasm"
ls -l web/improv_gui_web_bg.wasm
