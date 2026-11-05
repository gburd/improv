#!/usr/bin/env bash
# Take the desktop GUI's stills for docs/src/gui-tour.md, in one run:
#
#   docs/demo/gui-stills.sh [OUTDIR]       (OUTDIR defaults to docs/src/img)
#
# Builds the 3-D sample model (docs/demo/sample-model.sh) plus two saved
# views, runs improv-gui on a headless 1280x800 Xvfb, drives it with
# xdotool and writes, from xwd captures:
#   gui-pivot-grid.png    the Revenue worksheet, Region paged
#   gui-inspector.png     the Inspector's Grid page beside the worksheet
#   gui-format-panel.png  the Format panel's Number page, and its result
#   gui-graphs.png        the color graph as Bar and as Line
#   gui-print.png         the Print panel and Page Layout
#   gui-context-help.png  Point for Help on a category tile
#   gui-function-help.png the Functions window, searching "pmt"
#   gui-autocomplete.png  completing PM in the formula pane
#   gui-signature.png     inside PMT: the current argument marked
#   gui-stacked-axes.png  Region stacked under Time on the rows
#   gui-selection.png     an outer item selecting its group
#   gui-workspace.png     the whole workspace
# Takes about a minute after the build. Needs Xvfb, xdotool, xwd and
# ImageMagick (the dev shell has them). The layout is deterministic at the
# fixed geometry, so the coordinates below are absolute screen pixels;
# GUI_STILLS_BEATS=1 also writes OUTDIR/gs-beat-<n>.png at each step, to
# re-derive them when the layout changes.
#
# Environment:
#   IMPROV_BIN  directory holding improv and improv-gui (default: a release
#               build of this checkout)
set -euo pipefail

die() { echo "gui-stills: $*" >&2; exit 1; }
for t in Xvfb xdotool magick xwd; do
  command -v "$t" >/dev/null || die "missing $t"
done

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$root/docs/src/img}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$root"

if [[ -z ${IMPROV_BIN:-} ]]; then
  cargo build --quiet --release -p improv_cli -p improv_gui
  IMPROV_BIN=${CARGO_TARGET_DIR:-$root/target}/release
fi

W=1280 H=800
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

# --- Model (opened by a relative path, so the Print panel offers demo.pdf) --
IMPROV=$IMPROV_BIN/improv bash docs/demo/sample-model.sh "$tmp/demo.db" >/dev/null
"$IMPROV_BIN/improv" save-view "$tmp/demo.db" 1 "Revenue" Revenue --rows Time --cols Product >/dev/null
"$IMPROV_BIN/improv" save-view "$tmp/demo.db" 2 "Quantity" Quantity --rows Time --cols Product >/dev/null

# --- Display + GUI -----------------------------------------------------------
Xvfb -displayfd 3 -screen 0 "${W}x${H}x24" -nolisten tcp 3>"$tmp/display" \
  >"$tmp/xvfb.log" 2>&1 </dev/null &
xvfb_pid=$!
deadline=$((SECONDS + 60))
until [[ -s $tmp/display ]]; do
  kill -0 "$xvfb_pid" 2>/dev/null || die "Xvfb exited: $(cat "$tmp/xvfb.log")"
  ((SECONDS < deadline)) || die "timed out waiting for Xvfb"
  sleep 0.1
done
DISPLAY=:$(<"$tmp/display")
export DISPLAY

(cd "$tmp" && WAYLAND_DISPLAY='' LIBGL_ALWAYS_SOFTWARE=1 exec "$IMPROV_BIN/improv-gui" demo.db) \
  >"$tmp/gui.log" 2>&1 </dev/null &
gui_pid=$!
win=''
until [[ -n $win ]]; do
  kill -0 "$gui_pid" 2>/dev/null || die "improv-gui exited: $(cat "$tmp/gui.log")"
  ((SECONDS < deadline)) || die "timed out waiting for the Improv window"
  sleep 0.2
  win=$(xdotool search --name Improv 2>/dev/null | head -n1 || true)
done
until ! xdotool getwindowname "$win" | grep -q 'Loading…'; do
  ((SECONDS < deadline)) || die "timed out waiting for the model to load"
  sleep 0.2
done
xdotool windowmove "$win" 0 0 windowsize "$win" "$W" "$H"
sleep 3
xdotool windowfocus "$win" # no window manager: keys go where focus is

# --- Helpers -----------------------------------------------------------------
beat=0
step() {
  beat=$((beat + 1))
  [[ -z ${GUI_STILLS_BEATS:-} ]] || xwd -root -silent | magick xwd:- -strip "$out/gs-beat-$beat.png"
}
click() { xdotool mousemove "$1" "$2"; sleep 0.2; xdotool click 1; sleep 0.8; }
drag() { # x0 y0 x1 y1
  local i
  xdotool mousemove "$1" "$2"; sleep 0.2
  xdotool mousedown 1; sleep 0.15
  for i in $(seq 1 15); do
    xdotool mousemove $(($1 + ($3 - $1) * i / 15)) $(($2 + ($4 - $2) * i / 15))
    sleep 0.03
  done
  sleep 0.2; xdotool mouseup 1; sleep 0.8
}
menu() { click 30 "$1"; click 160 "$2"; } # a main-menu row, then its submenu row
away() { xdotool mousemove 1000 560; sleep 0.6; } # empty workspace: no tooltip
# grab FILE CROP [color]: an 8-bit grayscale PNG, or 64 colors for a graph.
grab() {
  local q=(-colorspace Gray -colors 32)
  [[ -z ${3:-} ]] || q=(-colors 64)
  xwd -root -silent | magick xwd:- -crop "$2" +repage -strip "${q[@]}" \
    -define png:compression-level=9 "$out/$1"
}
WS=362x476+131+0 # the Revenue worksheet window, once made taller

# --- The worksheet -----------------------------------------------------------
drag 300 385 300 470 # a taller Revenue window
drag 300 355 300 220 # and a taller formula pane: all five formulas show
away; step
grab gui-pivot-grid.png "$WS"

# --- Inspector: lines in the cell area only, moved beside the worksheet ------
menu 75 196 # Edit > Inspector…
drag 1100 259 600 259
click 526 357 # Lines In: Cell Area
click 748 385 # OK
away; step
grab gui-inspector.png 650x476+131+0
click 674 357; click 748 385 # back to Both
click 760 259                 # close

# --- Format panel: two decimals with commas, then undone --------------------
menu 95 116 # Format > Format Panel…
drag 868 259 638 259
click 582 318 # Specific
click 620 340; xdotool key ctrl+a; xdotool type 2; sleep 0.3
click 516 363 # Commas
click 749 415 # Set
away; step
grab gui-format-panel.png 650x476+131+0
click 762 259 # close
click 300 160; xdotool key ctrl+z; sleep 1

# --- Graphs: Bar, then Line ---------------------------------------------------
menu 155 176 # Graph > New Graph
away; step
grab gs-bar.png 398x317+875+454 color
click 960 490; click 940 517 # Type: Line
away; step
grab gs-line.png 398x317+875+454 color
magick "$out/gs-bar.png" "$out/gs-line.png" +append -strip -colors 64 \
  -define png:compression-level=9 "$out/gui-graphs.png"
rm -f "$out/gs-bar.png" "$out/gs-line.png"
click 1260 465 # close

# --- Print panel and Page Layout ----------------------------------------------
click 30 175 # Print…
drag 640 280 1103 230 # under where Page Layout opens, right-aligned
menu 95 296 # Format > Page Layout…
click 1138 122 # Landscape
away; step
grab gui-print.png 344x362+931+59
click 1260 72; click 1261 230 # close both

# --- Context help: Point for Help, then a click on the Product tile ----------
menu 35 96 # Info > Point for Help
click 461 33
drag 750 621 750 40 # the Help window up beside the worksheet
away; step
grab gui-context-help.png 790x476+131+0
click 908 40 # close

# --- Functions window ---------------------------------------------------------
menu 215 236 # Help > Functions…
xdotool type --delay 60 pmt; sleep 0.8
away; step
grab gui-function-help.png 421x441+852+32
click 1260 43 # close

# --- Autocomplete, then the signature inside the call ------------------------
click 300 456; click 300 456 # the Name = formula line
xdotool type --delay 60 'Payment = PM'; sleep 1; step
grab gui-autocomplete.png 520x180+131+370
xdotool key Tab; sleep 0.4; xdotool type --delay 60 '0.05, '; sleep 1; step
grab gui-signature.png 520x180+131+370
xdotool key ctrl+a BackSpace Escape; sleep 0.5

# --- Stacked rows, then an outer item's group -------------------------------
click 205 101 # the cursor back on 2024Q1 Widget
drag 161 33 240 279 # Region from the pages onto the rows
away; step
grab gui-stacked-axes.png "$WS"
click 158 117 # 2024Q2: its North and South rows
away; step
grab gui-selection.png "$WS"

# --- The whole workspace: Quantity beside Revenue, and a graph ---------------
drag 202 279 165 33 # Region back to the pages
click 205 101       # the cursor on 2024Q1 Widget
menu 135 156        # View > New View
click 48 750        # browser: Quantity
click 205 101       # Revenue is key again
menu 155 176        # Graph > New Graph
click 960 490; click 940 541 # Type: Bar again
away; step
grab gui-workspace.png "${W}x${H}+0+0" color

ls -l "$out"/gui-{pivot-grid,inspector,format-panel,graphs,print,context-help,function-help,autocomplete,signature,stacked-axes,selection,workspace}.png
