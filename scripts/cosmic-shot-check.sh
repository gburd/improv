#!/usr/bin/env bash
# The COSMIC GUI's screenshot regression, as scripts/gui-shot-check.sh is the
# egui GUI's: build improv and improv-cosmic, make the fixed demo model,
# render it under Xvfb with the window's own capture (IMPROV_COSMIC_SHOT),
# and count the pixels that differ from the golden by more than 2%.
#
#   scripts/cosmic-shot-check.sh [--update]
#
# Fails above COSMIC_SHOT_MAX_AE (default 200) and leaves the capture and the
# highlighted difference in target/cosmic-shot/. --update rewrites the golden
# (docs/reviews/golden/cosmic-1200x800.png). Needs Xvfb, magick, and what
# crates/gui_cosmic builds with (libxkbcommon, libwayland); the dev shell has
# them. COSMIC's dark theme is libcosmic's default with no COSMIC config, so
# the run uses an empty XDG_CONFIG_HOME: the user's own theme never leaks in.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
golden=$root/docs/reviews/golden/cosmic-1200x800.png
out=$root/target/cosmic-shot
max=${COSMIC_SHOT_MAX_AE:-200}
case "${1:-}" in
-h | --help)
	sed -n '2,15p' "$0" | sed 's/^# \{0,1\}//'
	exit 0
	;;
--update | '') ;;
*) echo "cosmic-shot-check: unknown option $1" >&2 && exit 2 ;;
esac
die() { echo "cosmic-shot-check: $*" >&2; exit 1; }
for t in Xvfb magick; do command -v $t >/dev/null || die "needs $t (the dev shell has it)"; done

mkdir -p "$out"
tmp=$(mktemp -d)
xvfb=''
cleanup() {
	[[ -z $xvfb ]] || kill "$xvfb" 2>/dev/null || true
	wait 2>/dev/null || true
	rm -rf "$tmp"
}
trap cleanup EXIT

target=${CARGO_TARGET_DIR:-$root/target}
(cd "$root" && cargo build --quiet --locked -p improv_cli)
(cd "$root/crates/gui_cosmic" && cargo build --quiet --locked)

# The demo model of scripts/gui-shot.sh.
db=$tmp/demo.db
im() { "$target/debug/improv" "$@" >/dev/null; }
im init "$db"
im add-category "$db" 1 Time
im add-category "$db" 2 Product
im add-category "$db" 3 Scenario
quarters=(Q1 Q2 Q3 Q4) products=(Widget Gadget) scenarios=(Plan Actual)
for i in 0 1 2 3; do im add-item "$db" $((10 + i)) 1 "${quarters[i]}"; done
for i in 0 1; do im add-item "$db" $((20 + i)) 2 "${products[i]}"; done
for i in 0 1; do im add-item "$db" $((30 + i)) 3 "${scenarios[i]}"; done
im add-measure "$db" 100 Price number input Time Product Scenario
im add-measure "$db" 101 Quantity number input Time Product Scenario
im add-derived "$db" 102 Revenue "Price * Quantity"
im add-derived "$db" 103 Total "SUM(Revenue OVER Time)"
for q in 0 1 2 3; do
	for p in 0 1; do
		for s in 0 1; do
			at="Time=${quarters[q]},Product=${products[p]},Scenario=${scenarios[s]}"
			im set "$db" 100 $((10 + 10 * p + q + s)) --at "$at"
			im set "$db" 101 $((100 - 50 * p + 10 * q - 5 * s)) --at "$at"
		done
	done
done

Xvfb -displayfd 3 -screen 0 1280x860x24 -nolisten tcp 3>"$tmp/display" 2>"$tmp/xvfb.log" &
xvfb=$!
for _ in $(seq 1 100); do [[ -s $tmp/display ]] && break; sleep 0.1; done
[[ -s $tmp/display ]] || die "Xvfb did not start: $(cat "$tmp/xvfb.log")"
mkdir -p "$tmp/config"
DISPLAY=:$(<"$tmp/display") WAYLAND_DISPLAY='' XDG_CONFIG_HOME=$tmp/config \
	IMPROV_COSMIC_SHOT=$out/cosmic.png timeout "${COSMIC_SHOT_TIMEOUT:-60}" \
	"$target/debug/improv-cosmic" "$db" >"$tmp/run.log" 2>&1 ||
	die "improv-cosmic failed: $(tail -5 "$tmp/run.log")"
[[ -s $out/cosmic.png ]] || die "no capture: $(tail -5 "$tmp/run.log")"

if [[ ${1:-} == --update ]]; then
	magick "$out/cosmic.png" -strip "$golden"
	echo "updated $golden"
	exit 0
fi
ae=$(magick compare -metric AE -fuzz 2% "$out/cosmic.png" "$golden" "$out/diff.png" 2>&1) || true
ae=${ae%% *}
[[ $ae =~ ^[0-9]+(\.[0-9]+)?(e\+?[0-9]+)?$ ]] || die "compare failed: $ae"
echo "cosmic-shot-check: $ae pixels differ (max $max); diff in $out/diff.png"
awk -v ae="$ae" -v max="$max" 'BEGIN { exit !(ae + 0 <= max + 0) }'
