#!/usr/bin/env bash
# Record the COSMIC GUI demo movie: docs/src/img/cosmic-demo.{mp4,gif,png}.
#
#   docs/demo/cosmic-movie.sh [OUTDIR]     (OUTDIR defaults to docs/src/img)
#
# gui-movie.sh's method for improv-cosmic: the sample model
# (docs/demo/sample-model.sh) on a headless 1280x800 Xvfb, recorded by
# ffmpeg x11grab while xdotool drives the beats, with a caption per beat in
# a band under the screen. The theme is libcosmic's default dark one: an
# empty XDG_CONFIG_HOME keeps the user's own out, as
# scripts/cosmic-shot-check.sh does.
#
# Needs Xvfb, xdotool, ImageMagick and xwd (the dev shell), gifsicle and an
# ffmpeg with x11grab and drawtext (`nix shell nixpkgs#ffmpeg-full
# nixpkgs#gifsicle`). The click coordinates are absolute screen pixels at
# the fixed geometry; COSMIC_MOVIE_SHOTS=1 writes OUTDIR/cm-beat-<n>.png at
# each beat to re-derive them after a layout change.
#
# Environment:
#   IMPROV_BIN     directory holding improv (default: a release build here)
#   IMPROV_COSMIC  the improv-cosmic binary (default: a release build of
#                  crates/gui_cosmic, its own workspace)
set -euo pipefail

die() { echo "cosmic-movie: $*" >&2; exit 1; }
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
  cargo build --quiet --release -p improv_cli
  IMPROV_BIN=${CARGO_TARGET_DIR:-$root/target}/release
fi
if [[ -z ${IMPROV_COSMIC:-} ]]; then
  (cd crates/gui_cosmic && cargo build --quiet --locked --release)
  IMPROV_COSMIC=${CARGO_TARGET_DIR:-$root/crates/gui_cosmic/target}/release/improv-cosmic
fi

W=1280 H=800 BAND=44
tmp=$(mktemp -d)
xvfb_pid='' app_pid='' rec_pid=''
cleanup() {
  [[ -z $rec_pid ]] || kill -INT "$rec_pid" 2>/dev/null || true
  [[ -z $app_pid ]] || kill "$app_pid" 2>/dev/null || true
  [[ -z $xvfb_pid ]] || kill "$xvfb_pid" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$tmp"
}
trap cleanup EXIT
trap 'exit 1' INT TERM

# --- Model, display, window --------------------------------------------------
db=$tmp/sample.db
IMPROV=$IMPROV_BIN/improv bash docs/demo/sample-model.sh "$db" >/dev/null

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

mkdir -p "$tmp/config"
WAYLAND_DISPLAY='' XDG_CONFIG_HOME=$tmp/config "$IMPROV_COSMIC" "$db" \
  >"$tmp/app.log" 2>&1 </dev/null &
app_pid=$!
win=''
until [[ -n $win ]]; do
  kill -0 "$app_pid" 2>/dev/null || die "improv-cosmic exited: $(cat "$tmp/app.log")"
  ((SECONDS < deadline)) || die "timed out waiting for the Improv window"
  sleep 0.2
  win=$(xdotool search --name Improv 2>/dev/null | head -n1 || true)
done
xdotool windowmove "$win" 0 0 windowsize "$win" "$W" "$H"
sleep 3
xdotool windowfocus "$win" # no window manager: keys go where focus is

# --- Helpers -----------------------------------------------------------------
n=0
shot() {
  n=$((n + 1))
  [[ -z ${COSMIC_MOVIE_SHOTS:-} ]] || xwd -root -silent | magick xwd:- -strip "$out/cm-beat-$n.png"
}
glide() { # to (x, y), smoothly
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
measure() { click 512 81; click 512 "$1"; } # the header bar's measure list, then a row
type_slow() { xdotool type --delay 120 "$1"; }
t0='' caps=$tmp/captions
beat() { # its caption shows from now until the next beat
  printf '%s\t%s\n' "$(awk -v a="$(date +%s.%N)" -v b="$t0" 'BEGIN { printf "%.2f", a - b }')" "$1" >>"$caps"
}

# --- Stage: Revenue shown, the pointer out of the way --------------------------
measure 230
glide 900 450
sleep 2

# --- Record ------------------------------------------------------------------
ffmpeg -loglevel error -y -f x11grab -framerate 15 -draw_mouse 1 \
  -video_size "${W}x${H}" -i "$DISPLAY" -c:v libx264 -preset veryfast -crf 18 \
  -pix_fmt yuv444p "$tmp/raw.mkv" </dev/null &
rec_pid=$!
sleep 0.5
t0=$(date +%s.%N)

beat 'improv-cosmic: the same model, a native COSMIC application'
sleep 4; shot

beat 'Pivot: drag the Region tile from the pages onto the rows'
drag 100 195 200 140; glide 900 450 16; sleep 3; shot

beat 'A row per quarter and region. Drag it back to the pages'
drag 160 139 100 195; glide 900 450 16; sleep 2; shot

beat 'Swap exchanges rows and columns; no formula changes'
click 1222 131; glide 900 450 16; sleep 3
click 1222 131; glide 900 450 16; sleep 1; shot

beat 'Show Quantity, an input, and edit 2024Q2 Gadget from 30 to 90'
measure 194; sleep 1
dclick 300 305; sleep 0.5
xdotool key ctrl+a; sleep 0.3; type_slow 90; sleep 0.8
xdotool key Return; sleep 1.5; shot

beat 'Revenue follows: 2024Q2 Gadget is now 25 x 90 = 2250'
measure 230; glide 300 305 16; sleep 4; shot

beat 'Edit a formula; names and functions complete as you type'
click 75 677 # on the line's text: its button is only as wide as the text
xdotool key ctrl+a; sleep 0.3; type_slow 'Revenue - CO'; sleep 3; shot

beat 'A wrong name is refused and located, with a suggestion'
type_slow 'GZ'; sleep 0.5; xdotool key Return; sleep 4; shot

beat 'Fixed, it recalculates'
xdotool key ctrl+a; sleep 0.3; type_slow 'Revenue - COGS'; sleep 0.5
xdotool key Return; glide 900 450 16; sleep 3; shot

beat 'Chart shows the measure as bars, in the theme colors'
click 994 81; glide 900 450 16; sleep 4; shot
click 994 81; sleep 1

sleep 1
kill -INT "$rec_pid"
wait "$rec_pid" || true
rec_pid=''

# --- Captions ----------------------------------------------------------------
dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$tmp/raw.mkv")
vf="pad=$W:$((H + BAND)):0:0:color=0x2a2a2a"
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
  -crf 29 -r 15 -pix_fmt yuv420p -movflags +faststart -an "$out/cosmic-demo.mp4"
g='fps=8,scale=960:-1:flags=lanczos'
ffmpeg -loglevel error -y -i "$out/cosmic-demo.mp4" -vf "$g,palettegen=max_colors=64:stats_mode=diff" "$tmp/pal.png"
ffmpeg -loglevel error -y -i "$out/cosmic-demo.mp4" -i "$tmp/pal.png" \
  -lavfi "${g}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" "$tmp/raw.gif"
gifsicle -O3 --lossy=30 --colors 64 "$tmp/raw.gif" -o "$out/cosmic-demo.gif"
# The poster: the chart.
poster=$(awk -F'\t' 'END { print $1 + 2 }' "$caps")
ffmpeg -loglevel error -y -ss "$poster" -i "$out/cosmic-demo.mp4" -frames:v 1 "$tmp/poster.png"
magick "$tmp/poster.png" -strip -colors 64 -define png:compression-level=9 "$out/cosmic-demo.png"
printf 'duration %.1fs\n' "$dur"
cat "$caps"
ls -l "$out"/cosmic-demo.mp4 "$out"/cosmic-demo.gif "$out"/cosmic-demo.png
