#!/usr/bin/env bash
# Take the web GUI's stills for docs/src/web-gui.md, in one run:
#
#   docs/demo/web-stills.sh [OUTDIR]       (OUTDIR defaults to docs/src/img)
#
# Builds the web GUI (crates/gui_web/build.sh --release), improv and
# improv-server, makes a Time x Product model with the CLI, serves it with
# a token on 127.0.0.1:38741, and drives two browser sessions in headless
# Chromium (docs/demo/web-stills.py) to write web-login.png,
# web-worksheet.png, web-live.png and web-browser.png. Takes under a
# minute after the build.
#
# Needs what scripts/web-e2e.sh needs: the wasm32-unknown-unknown target,
# wasm-bindgen-cli at Cargo.lock's version, Chromium (CHROMIUM=...,
# default chromium), python3 with websocket-client (PYTHON=..., default
# python3), curl and ImageMagick.
set -euo pipefail

die() { echo "web-stills: $*" >&2; exit 1; }
root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$root/docs/src/img}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$root"
target=${CARGO_TARGET_DIR:-$root/target}
crates/gui_web/build.sh --release "$target/gui_web" >/dev/null
cargo build --locked --release -q -p improv_cli -p improv_server
I=$target/release/improv S=$target/release/improv-server

d=$(mktemp -d)
pid=''
cleanup() {
  if [[ -n $pid ]]; then
    kill "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  rm -rf "$d"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

q() { "$I" "$@" >/dev/null; }
db=$d/sales.db
q init "$db"
q add-category "$db" 1 Time
id=10
for y in 2025 2026; do for k in 1 2 3 4; do q add-item "$db" $id 1 "${y}Q$k"; id=$((id + 1)); done; done
q add-category "$db" 2 Product
q add-item "$db" 20 2 Widget; q add-item "$db" 21 2 Gadget; q add-item "$db" 22 2 Gizmo
q add-measure "$db" 100 Price number input Product
q add-measure "$db" 101 Quantity number input Time Product
for p in Widget:12 Gadget:30 Gizmo:5; do q set "$db" 100 "${p#*:}" --at "Product=${p%:*}"; done
qty=100
for t in 2025Q1 2025Q2 2025Q3 2025Q4 2026Q1 2026Q2 2026Q3 2026Q4; do
  for p in Widget Gadget Gizmo; do q set "$db" 101 $qty --at "Time=$t,Product=$p"; qty=$((qty + 7)); done
done
q define "$db" 102 "Revenue = Price * Quantity"

port=38741
token=web-stills-$RANDOM$RANDOM
IMPROV_API_TOKEN=$token "$S" "$db" "127.0.0.1:$port" --static "$target/gui_web" \
  >"$d/server.log" 2>&1 </dev/null &
pid=$!
for _ in $(seq 50); do
  curl -fs "http://127.0.0.1:$port/v1/health" >/dev/null && break
  sleep 0.1
done
curl -fs "http://127.0.0.1:$port/v1/health" >/dev/null || die "improv-server did not start: $(cat "$d/server.log")"

SCRATCH=$d timeout 180 "${PYTHON:-python3}" docs/demo/web-stills.py \
  "http://127.0.0.1:$port/" "$token" "$d" ||
  { cp "$d"/*.png "$out/" 2>/dev/null; die "the browser run failed (its screenshots are in $out)"; }

# The live edit: each session's Quantity and Revenue windows, A left, B
# right, each under a label; the login: its panel.
font=$root/crates/gui/assets/fonts/Arimo-Bold.ttf
for s in a b; do
  label=$([[ $s == a ]] && echo 'Session A: types 250 into 2025Q2 Widget' || echo 'Session B: the edit and the new Revenue')
  magick "$d/web-live-$s.png" -gravity northwest -crop 452x402+171+207 +repage \
    -background '#333333' -splice 0x28 -fill white -font "$font" -pointsize 15 \
    -gravity north -annotate +0+5 "$label" "$d/live-$s.png"
done
magick "$d/live-a.png" "$d/live-b.png" -background '#333333' -gravity northwest \
  -splice 12x0 +append -gravity northwest -chop 12x0 "$d/web-live.png"
magick "$d/web-login.png" -gravity northwest -crop 360x170+460+272 +repage "$d/web-login.png"
for f in web-login web-worksheet web-live web-browser; do
  magick "$d/$f.png" -strip -colors 64 -define png:compression-level=9 "$out/$f.png"
done
ls -l "$out"/web-{login,worksheet,live,browser}.png
