#!/usr/bin/env bash
# Build the web GUI into DIR (default: target/gui_web): index.html, the
# wasm and its JS glue. Then serve it: improv-server MODEL --static DIR.
#   build.sh [--release] [DIR]
# Needs the wasm32-unknown-unknown target and wasm-bindgen-cli at the
# version Cargo.lock pins (cargo install wasm-bindgen-cli --version V).
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
profile=debug
args=()
if [[ ${1:-} == --release ]]; then
	profile=release
	args+=(--release)
	shift
fi
out=${1:-$root/target/gui_web}
target=${CARGO_TARGET_DIR:-$root/target}
want=$(awk '/^name = "wasm-bindgen"$/ { getline; gsub(/[^0-9.]/, ""); print }' "$root/Cargo.lock")
have=$(wasm-bindgen --version 2>/dev/null | awk '{ print $2 }')
if [[ $have != "$want" ]]; then
	echo "build.sh: need wasm-bindgen $want (have ${have:-none}):" >&2
	echo "  cargo install wasm-bindgen-cli --version $want --locked" >&2
	exit 1
fi
cargo build --locked -p improv_gui_web --target wasm32-unknown-unknown "${args[@]}"
mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" \
	"$target/wasm32-unknown-unknown/$profile/improv_gui_web.wasm"
cp "$here/web/index.html" "$out/"
ls -l "$out"/*.wasm
