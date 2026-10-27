#!/usr/bin/env bash
# Fixed-geometry GUI screenshot beside the NeXT reference (review plan item D5).
set -euo pipefail

usage() {
	cat <<'EOF'
Usage: scripts/gui-shot.sh [-h|--help] [OUTDIR]

Builds improv and improv-gui (debug), creates a fixed demo model
(Time x Product x Scenario; Price, Quantity, Revenue, Total; two saved
views), runs the GUI
on a headless Xvfb and writes:
  OUTDIR/gui.png      the capture
  OUTDIR/compare.png  the capture beside docs/reviews/refs/improv-next-workspace-1.0.png
OUTDIR defaults to target/gui-shot/. Run from the dev shell (GUI libs on
LD_LIBRARY_PATH). Missing Xvfb/xwd/magick/xdotool come from `nix shell`.

Environment:
  GUI_SHOT_GEOMETRY  screen and window size, WxH (default 1400x900)
  GUI_SHOT_TIMEOUT   seconds to wait for the window (default 60)
  GUI_SHOT_SETTLE    seconds to let the GUI repaint after sizing (default 3)
  GUI_SHOT_DB        where to build the demo model (default: a temp Mentat
                     file); e.g. postgres://u:p@127.0.0.1/db?model=demo to
                     shoot a Postgres-hosted model (use a fresh model name)
EOF
}

die() {
	echo "gui-shot: $*" >&2
	exit 1
}

case "${1:-}" in -h | --help)
	usage
	exit 0
	;;
-*) die "unknown option $1 (see --help)" ;;
esac

# Tools: take what's on PATH, fetch the rest with nix shell (re-exec once).
missing=()
for t in Xvfb:xorg.xorgserver xwd:xorg.xwd magick:imagemagick xdotool:xdotool; do
	command -v "${t%%:*}" >/dev/null || missing+=("nixpkgs#${t#*:}")
done
if ((${#missing[@]})); then
	[[ -z ${GUI_SHOT_IN_NIX:-} ]] || die "still missing after nix shell: ${missing[*]}"
	GUI_SHOT_IN_NIX=1 exec nix shell "${missing[@]}" -c bash "$0" "$@"
fi

geom=${GUI_SHOT_GEOMETRY:-1400x900}
[[ $geom =~ ^([0-9]+)x([0-9]+)$ ]] || die "bad GUI_SHOT_GEOMETRY '$geom' (want WxH)"
w=${BASH_REMATCH[1]} h=${BASH_REMATCH[2]}

root=$(cd "$(dirname "$0")/.." && pwd)
out=${1:-$root/target/gui-shot}
mkdir -p "$out"
out=$(cd "$out" && pwd)
ref=$root/docs/reviews/refs/improv-next-workspace-1.0.png
cd "$root"

tmp=$(mktemp -d)
xvfb_pid='' gui_pid=''
cleanup() {
	[[ -z $gui_pid ]] || kill "$gui_pid" 2>/dev/null || true
	[[ -z $xvfb_pid ]] || kill "$xvfb_pid" 2>/dev/null || true
	wait 2>/dev/null || true
	rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

cargo build --quiet -p improv_cli -p improv_gui
bin=${CARGO_TARGET_DIR:-$root/target}/debug

# Demo model: fixed ids, fixed values.
db=${GUI_SHOT_DB:-$tmp/demo.db}
im() { "$bin/improv" "$@" >/dev/null; }
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

im save-view "$db" 1 "Revenue by quarter" Revenue --rows Time --cols Product
im save-view "$db" 2 "Price plan" Price --rows Product,Time --cols Scenario

# Xvfb picks a free display and reports it on fd 3 once it accepts clients.
Xvfb -displayfd 3 -screen 0 "${w}x${h}x24" -nolisten tcp 3>"$tmp/display" 2>"$tmp/xvfb.log" &
xvfb_pid=$!
deadline=$((SECONDS + ${GUI_SHOT_TIMEOUT:-60}))
until [[ -s $tmp/display ]]; do
	kill -0 "$xvfb_pid" 2>/dev/null || die "Xvfb exited: $(cat "$tmp/xvfb.log")"
	((SECONDS < deadline)) || die "timed out waiting for Xvfb"
	sleep 0.1
done
DISPLAY=:$(<"$tmp/display")
export DISPLAY

WAYLAND_DISPLAY='' LIBGL_ALWAYS_SOFTWARE=1 "$bin/improv-gui" "$db" >"$tmp/gui.log" 2>&1 &
gui_pid=$!
win=''
until [[ -n $win ]]; do
	kill -0 "$gui_pid" 2>/dev/null || die "improv-gui exited: $(cat "$tmp/gui.log")"
	((SECONDS < deadline)) || die "timed out waiting for the Improv window"
	sleep 0.2
	win=$(xdotool search --name Improv 2>/dev/null | head -n1 || true)
done
# The model loads behind the open window (its title ends "Loading…").
until ! xdotool getwindowname "$win" | grep -q 'Loading…'; do
	kill -0 "$gui_pid" 2>/dev/null || die "improv-gui exited: $(cat "$tmp/gui.log")"
	((SECONDS < deadline)) || die "timed out waiting for the model to load"
	sleep 0.2
done
xdotool windowmove "$win" 0 0 windowsize "$win" "$w" "$h"
sleep "${GUI_SHOT_SETTLE:-3}"
kill -0 "$gui_pid" 2>/dev/null || die "improv-gui exited: $(cat "$tmp/gui.log")"

xwd -root -silent | magick xwd:- -strip "$out/gui.png"
magick "$out/gui.png" \( "$ref" -resize "x$h" \) +append -strip "$out/compare.png"
echo "$out/gui.png"
echo "$out/compare.png"
