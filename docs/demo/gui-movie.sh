#!/usr/bin/env bash
# Record the GUI demo movie: docs/src/img/gui-demo.{mp4,gif,png}.
#
#   docs/demo/gui-movie.sh [OUTDIR]       (OUTDIR defaults to docs/src/img)
#
# Builds the 3-D sample model (docs/demo/sample-model.sh: Time x Product x
# Region; Price, UnitCost, Quantity; Revenue, COGS, Margin) plus two saved
# views, runs improv-gui on a headless 1280x800 Xvfb, records it with
# ffmpeg x11grab while xdotool drives the beats, then encodes an H.264 MP4
# (the one the guide plays), a GIF (palettegen/paletteuse + gifsicle, the
# fallback) and the MP4's poster. Each beat logs when it starts, and its
# caption is drawn in a band padded below the screen, where the GUI never
# draws, with ffmpeg drawtext in the GUI's own font (Arimo).
#
# Needs Xvfb, xdotool, ImageMagick and xwd (the dev shell has them), gifsicle,
# and an ffmpeg with x11grab and drawtext: the nixpkgs one, `nix shell
# nixpkgs#ffmpeg-full nixpkgs#gifsicle`. Takes about two minutes after the build; the
# movie is about 90 s. The layout is deterministic at the fixed geometry, so
# the click coordinates below are absolute screen pixels; re-derive them
# (GUI_MOVIE_SHOTS=1 writes OUTDIR/gm-beat-<n>.png at each beat) if the GUI
# layout changes. docs/demo/gui-stills.sh takes the guide's stills.
#
# Environment:
#   IMPROV_BIN  directory holding improv and improv-gui (default: a release
#               build of this checkout)
#   GUI_MOVIE_SHOTS=1  also write OUTDIR/gm-beat-<n>.png at each beat
set -euo pipefail

die() { echo "gui-movie: $*" >&2; exit 1; }
for t in Xvfb xdotool ffmpeg ffprobe gifsicle magick xwd; do
  command -v "$t" >/dev/null || die "missing $t"
done

root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$root/docs/src/img}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$root"
font=$root/crates/gui/assets/fonts/Arimo-Regular.ttf
[[ -f $font ]] || die "missing $font"

if [[ -z ${IMPROV_BIN:-} ]]; then
  cargo build --quiet --release -p improv_cli -p improv_gui
  IMPROV_BIN=${CARGO_TARGET_DIR:-$root/target}/release
fi

W=1280 H=800 BAND=44
tmp=$(mktemp -d)
xvfb_pid='' gui_pid='' rec_pid=''
cleanup() {
  [[ -z $rec_pid ]] || kill -INT "$rec_pid" 2>/dev/null || true
  [[ -z $gui_pid ]] || kill "$gui_pid" 2>/dev/null || true
  [[ -z $xvfb_pid ]] || kill "$xvfb_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

# --- Model -------------------------------------------------------------------
db=$tmp/demo.db
IMPROV=$IMPROV_BIN/improv bash docs/demo/sample-model.sh "$db" >/dev/null
"$IMPROV_BIN/improv" save-view "$db" 1 "Revenue" Revenue --rows Time --cols Product >/dev/null
"$IMPROV_BIN/improv" save-view "$db" 2 "Quantity" Quantity --rows Time --cols Product >/dev/null

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

WAYLAND_DISPLAY='' LIBGL_ALWAYS_SOFTWARE=1 "$IMPROV_BIN/improv-gui" "$db" \
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
n=0
shot() {
  n=$((n + 1))
  [[ -z ${GUI_MOVIE_SHOTS:-} ]] || xwd -root -silent | magick xwd:- -strip "$out/gm-beat-$n.png"
}
# Smooth pointer glide from the current position to (x, y).
glide() {
  local x0 y0 i n=${3:-14}
  eval "$(xdotool getmouselocation --shell)"
  x0=$X y0=$Y
  for ((i = 1; i <= n; i++)); do
    xdotool mousemove $((x0 + ($1 - x0) * i / n)) $((y0 + ($2 - y0) * i / n))
    sleep 0.035
  done
}
click() { glide "$1" "$2"; sleep 0.25; xdotool click 1; sleep 0.6; }
dclick() { glide "$1" "$2"; sleep 0.25; xdotool click --repeat 2 --delay 80 1; sleep 0.6; }
drag() { # x0 y0 x1 y1
  glide "$1" "$2"; sleep 0.3
  xdotool mousedown 1; sleep 0.2
  glide "$3" "$4" 24; sleep 0.3
  xdotool mouseup 1; sleep 0.6
}
menu() { click 30 "$1"; click 160 "$2"; } # a main-menu row, then its submenu row
type_slow() { xdotool type --delay 120 "$1"; }
# A beat: its caption shows from now until the next beat starts.
t0=''
caps=$tmp/captions
beat() {
  local now
  now=$(date +%s.%N)
  printf '%s\t%s\n' "$(awk -v a="$now" -v b="$t0" 'BEGIN { printf "%.2f", a - b }')" "$1" >>"$caps"
}

# --- Stage (before recording) ------------------------------------------------
# A taller Revenue window with all five formulas, and a Quantity window
# beside it (View > New View, then Quantity in the model browser).
drag 300 385 300 470
drag 300 355 300 220
menu 135 156
click 48 750
click 205 101 # Revenue is the key window again
glide 1000 600
sleep 5 # the status line clears

# --- Record ------------------------------------------------------------------
ffmpeg -loglevel error -y -f x11grab -framerate 15 -draw_mouse 1 \
  -video_size "${W}x${H}" -i "$DISPLAY" -c:v libx264 -preset veryfast -crf 18 \
  -pix_fmt yuv444p "$tmp/raw.mkv" </dev/null &
rec_pid=$!
sleep 0.5
t0=$(date +%s.%N)

beat 'Two worksheets on one model: Revenue (a formula) and Quantity (an input)'
sleep 4; shot

beat 'Pivot: drag the Region tile from the pages onto the rows'
glide 161 33 20; sleep 0.5
drag 161 33 240 279; glide 700 600 16; sleep 3; shot

beat 'Each quarter now holds a row per region. Drag it back to the pages'
sleep 2
drag 202 279 165 33; glide 700 600 16; sleep 2; shot

beat 'Page: the drop-down under the Region tile picks the region shown'
click 185 56; sleep 1; click 166 106; sleep 1
click 553 56; sleep 1; click 534 106; glide 700 600 16; sleep 2; shot

beat 'Edit an input: South 2024Q1 Widget Quantity, from 40 to 90'
dclick 575 101; sleep 0.6
xdotool key ctrl+a; sleep 0.4; type_slow 90; sleep 1
xdotool key Return; sleep 0.5

beat 'Revenue follows at once: only the changed cells recompute'
glide 205 101 20; sleep 4; shot

beat 'A new formula on the last line; the list completes names as you type'
click 300 456; click 300 456 # the first click makes the pane key
type_slow 'Bonus = ROU'; sleep 2.5; shot
xdotool key Tab; sleep 1

beat 'Tab takes ROUND(; the tip shows its arguments. Then Revenue'
type_slow 'Rev'; sleep 1.2
xdotool key Down; sleep 0.5; xdotool key Down; sleep 0.6; xdotool key Tab; sleep 0.6
type_slow ' * 0.05, 0)'; sleep 2; shot
xdotool key Return; sleep 0.5

beat 'Enter adds Bonus and shows it, recalculated'
sleep 3.5; shot

beat 'Graph > New Graph charts the worksheet in color, and follows it'
click 205 101 # the cursor back on a cell
menu 155 176; glide 1000 600 16; sleep 3; shot

beat 'Type picks one of seven graphs: here Line'
click 960 490; sleep 1; click 940 517; glide 1000 600 16; sleep 3; shot

beat 'Edit > Inspector sets how this worksheet draws: here, no grid lines'
menu 75 196; sleep 0.6
click 1026 318; sleep 0.8 # Show Grid Lines off
click 1248 385            # OK
glide 760 600 16; sleep 4; shot

sleep 1
kill -INT "$rec_pid"
wait "$rec_pid" || true
rec_pid=''

# --- Captions ----------------------------------------------------------------
dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$tmp/raw.mkv")
vf="pad=$W:$((H + BAND)):0:0:color=0x2a2a2a"
i=0
mapfile -t lines <"$caps"
for ((i = 0; i < ${#lines[@]}; i++)); do
  a=${lines[i]%%$'\t'*} text=${lines[i]#*$'\t'}
  if ((i + 1 < ${#lines[@]})); then b=${lines[i + 1]%%$'\t'*}; else b=$dur; fi
  printf '%s' "$text" >"$tmp/cap-$i.txt"
  vf+=",drawtext=fontfile=$font:textfile=$tmp/cap-$i.txt:fontsize=22:fontcolor=white"
  vf+=":x=16:y=$((H + (BAND - 22) / 2)):enable='between(t,$a,$b)'"
done

# --- Encode ------------------------------------------------------------------
ffmpeg -loglevel error -y -i "$tmp/raw.mkv" -vf "$vf" -c:v libx264 -preset veryslow \
  -crf 29 -r 15 -pix_fmt yuv420p -movflags +faststart -an "$out/gui-demo.mp4"
g='fps=8,scale=960:-1:flags=lanczos'
ffmpeg -loglevel error -y -i "$out/gui-demo.mp4" -vf "$g,palettegen=max_colors=64:stats_mode=diff" "$tmp/pal.png"
ffmpeg -loglevel error -y -i "$out/gui-demo.mp4" -i "$tmp/pal.png" \
  -lavfi "${g}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" "$tmp/raw.gif"
gifsicle -O3 --lossy=30 --colors 64 "$tmp/raw.gif" -o "$out/gui-demo.gif"
# The poster: the frame with the graph, before the Inspector's beat.
poster=$(awk -F'\t' 'NR == 11 { print $1 - 0.5 }' "$caps")
ffmpeg -loglevel error -y -ss "$poster" -i "$out/gui-demo.mp4" -frames:v 1 "$tmp/poster.png"
magick "$tmp/poster.png" -strip -colors 64 -define png:compression-level=9 "$out/gui-demo.png"
printf 'duration %.1fs\n' "$dur"
cat "$caps"
ls -l "$out"/gui-demo.mp4 "$out"/gui-demo.gif "$out"/gui-demo.png
