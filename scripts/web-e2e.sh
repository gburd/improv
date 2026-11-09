#!/usr/bin/env bash
# The web GUI end to end, in one command: build the web GUI, improv and
# improv-server (release), drive two browser sessions in headless Chromium
# (crates/gui_web/e2e), and compare the first frame against the golden.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
golden=$root/docs/reviews/golden/web-1280x713.png
out=$root/target/web-e2e
max=${WEB_SHOT_MAX_AE:-200}

case "${1:-}" in
-h | --help)
	cat <<USAGE
Usage: scripts/web-e2e.sh [--update]

Builds crates/gui_web (crates/gui_web/build.sh --release), improv and
improv-server, runs crates/gui_web/e2e/e2e.sh (two sessions: a live edit,
a server restart caught up, a pivot by drag, a located formula error,
undo, a page item, and a third session's accessibility tree: the grid's
cells and headers, Tab order, a menu row activated, a cell edited from the
keyboard), then counts the pixels of its first frame that differ
from docs/reviews/golden/web-1280x713.png by more than 2% (magick compare
-metric AE -fuzz 2%). Fails above WEB_SHOT_MAX_AE (default 200), leaving
the screenshots and the diff in target/web-e2e/.
  --update   overwrite the golden with the new capture instead
WEB_GOLDEN=0 skips the comparison (another Chromium build than the
nixpkgs one the golden was taken with).

Needs the wasm32-unknown-unknown target, wasm-bindgen-cli at Cargo.lock's
version, Chromium (CHROMIUM=..., default chromium), python3 with
websocket-client, and ImageMagick (the dev shell has it).
USAGE
	exit 0
	;;
--update | '') ;;
*) echo "web-e2e: unknown option $1" >&2 && exit 2 ;;
esac

cd "$root"
target=${CARGO_TARGET_DIR:-$root/target}
crates/gui_web/build.sh --release "$target/gui_web" > /dev/null
cargo build --locked --release -q -p improv_cli -p improv_server
mkdir -p "$out"
crates/gui_web/e2e/e2e.sh "$target/release/improv" "$target/release/improv-server" \
	"$target/gui_web" "$out"
if [[ ${WEB_GOLDEN:-1} == 0 ]]; then
	# For a Chromium build the golden was not taken with (it was taken
	# with nixpkgs' Chromium 156): another may rasterize differently.
	echo "web-e2e: passed; golden not compared (WEB_GOLDEN=0)"
	exit 0
fi
if [[ ${1:-} == --update ]]; then
	cp "$out/web-golden.png" "$golden"
	echo "updated $golden"
	exit 0
fi
# compare exits 1 when the images differ; the metric goes to stderr.
ae=$(magick compare -metric AE -fuzz 2% "$out/web-golden.png" "$golden" "$out/diff.png" 2>&1) || true
ae=${ae%% *}
[[ $ae =~ ^[0-9]+(\.[0-9]+)?(e\+?[0-9]+)?$ ]] || {
	echo "web-e2e: compare failed: $ae" >&2
	exit 1
}
echo "web-e2e: $ae pixels differ (max $max); diff in $out/diff.png"
awk -v ae="$ae" -v max="$max" 'BEGIN { exit !(ae + 0 <= max + 0) }'
