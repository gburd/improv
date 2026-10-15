#!/usr/bin/env bash
# Record the GUI demo movie: docs/src/img/gui-demo.{gif,mp4}.
#
#   docs/demo/gui-movie.sh [OUTDIR]       (OUTDIR defaults to docs/src/img)
#
# Builds the 3-D sample model (docs/demo/sample-model.sh: Time x Product x
# Region; Price, UnitCost, Quantity; Revenue, COGS, Margin) plus two saved
# views, runs improv-gui on a headless 1280x800 Xvfb, records it with
# ffmpeg x11grab while xdotool drives the beats, then encodes a GIF
# (palettegen/paletteuse + gifsicle) and an H.264 MP4. The movie keeps the
# whole screen: the status bar's Sum sits at its right edge.
#
# Needs Xvfb, xdotool, ffmpeg, gifsicle, ImageMagick. The layout is
# deterministic at the fixed geometry, so the click coordinates below are
# absolute screen pixels; re-derive them (GUI_MOVIE_SHOTS=1 writes a PNG per
# beat) if the GUI layout changes.
#
# Environment:
#   IMPROV_BIN  directory holding improv and improv-gui (default: a release
#               build of this checkout)
#   GUI_MOVIE_SHOTS=1  also write OUTDIR/gm-beat-<n>.png at each beat
#
# Besides the movie it writes three stills into OUTDIR: gui-workspace.png
# (the whole workspace, Region stacked on rows), gui-pivot-grid.png (the
# Revenue worksheet, Time x Product with Region paged) and
# gui-stacked-axes.png (the same worksheet with Region stacked on rows).
set -euo pipefail

die() { echo "gui-movie: $*" >&2; exit 1; }
for t in Xvfb xdotool ffmpeg gifsicle magick xwd; do
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
Xvfb -displayfd 3 -screen 0 "${W}x${H}x24" -nolisten tcp 3>"$tmp/display" 2>"$tmp/xvfb.log" &
xvfb_pid=$!
deadline=$((SECONDS + 60))
until [[ -s $tmp/display ]]; do
  kill -0 "$xvfb_pid" 2>/dev/null || die "Xvfb exited: $(cat "$tmp/xvfb.log")"
  ((SECONDS < deadline)) || die "timed out waiting for Xvfb"
  sleep 0.1
done
DISPLAY=:$(<"$tmp/display")
export DISPLAY

WAYLAND_DISPLAY='' LIBGL_ALWAYS_SOFTWARE=1 "$IMPROV_BIN/improv-gui" "$db" >"$tmp/gui.log" 2>&1 &
gui_pid=$!
win=''
until [[ -n $win ]]; do
  kill -0 "$gui_pid" 2>/dev/null || die "improv-gui exited: $(cat "$tmp/gui.log")"
  ((SECONDS < deadline)) || die "timed out waiting for the Improv window"
  sleep 0.2
  win=$(xdotool search --name Improv 2>/dev/null | head -n1 || true)
done
xdotool windowmove "$win" 0 0 windowsize "$win" "$W" "$H"
sleep 4
xdotool windowfocus "$win" # no window manager: keys go where focus is

# --- Helpers -----------------------------------------------------------------
beat=0
shot() {
  beat=$((beat + 1))
  [[ -z ${GUI_MOVIE_SHOTS:-} ]] || xwd -root -silent | magick xwd:- -strip "$out/gm-beat-$beat.png"
}
# Smooth pointer glide from the current position to (x, y).
glide() {
  local x0 y0 i n=${3:-12}
  eval "$(xdotool getmouselocation --shell)"
  x0=$X y0=$Y
  for ((i = 1; i <= n; i++)); do
    xdotool mousemove $((x0 + ($1 - x0) * i / n)) $((y0 + ($2 - y0) * i / n))
    sleep 0.03
  done
}
click() { glide "$1" "$2"; sleep 0.2; xdotool click 1; }
dclick() { glide "$1" "$2"; sleep 0.2; xdotool click --repeat 2 --delay 80 1; }
drag() { # x0 y0 x1 y1
  glide "$1" "$2"; sleep 0.2
  xdotool mousedown 1; sleep 0.15
  glide "$3" "$4" 20; sleep 0.2
  xdotool mouseup 1
}
type_slow() { xdotool type --delay 90 "$1"; }
grab() { # file [crop]
  xwd -root -silent | magick xwd:- ${2:+-crop "$2" +repage} -strip \
    -define png:compression-level=9 "$out/$1"
}
REV_WIN=364x474+130+0 # the Revenue worksheet window, once resized

# --- Stage (before recording) -----------------------------------------------
# A second worksheet on Quantity, dragged by its title bar to the right; the
# Revenue window made taller and its formula pane opened up.
click 50 118; sleep 0.6; click 160 160; sleep 1     # Worksheet > Add Matrix
click 48 750; sleep 1                               # browser: Quantity
drag 340 40 790 12; sleep 1                         # move it right
click 300 200; sleep 0.5                            # focus Revenue
drag 430 386 430 470; sleep 1                       # taller window
drag 300 353 300 210; sleep 1                       # taller formula pane
glide 760 560; sleep 1.5
grab gui-pivot-grid.png "$REV_WIN"

# --- Record ------------------------------------------------------------------
ffmpeg -loglevel error -y -f x11grab -framerate 15 -draw_mouse 1 \
  -video_size "${W}x${H}" -i "$DISPLAY" -c:v libx264 -preset veryfast -crf 18 \
  -pix_fmt yuv444p "$tmp/raw.mkv" </dev/null &
rec_pid=$!
sleep 1

# (a) the workspace settles.
sleep 3; shot
glide 300 200 25; sleep 0.8

# (b) Region from the page gutter onto the rows strip: stacked row headers.
drag 158 34 240 282; glide 420 230 10; sleep 1; shot
grab gui-workspace.png 980x800+0+0
grab gui-stacked-axes.png "$REV_WIN"
sleep 2
drag 202 282 165 34; sleep 2.5; shot              # and back to pages

# (c) another Region item from the page drop-down.
click 185 57; sleep 1.2; click 164 106; sleep 2.5; shot

# (d) Quantity, same page; edit 2024Q1 Widget and Revenue follows at once.
click 660 57; sleep 1; click 642 106; sleep 1.5
dclick 690 101; sleep 0.8
xdotool key ctrl+a; sleep 0.3; type_slow 90; sleep 0.6
xdotool key Return; sleep 0.5
glide 205 101 20; sleep 3; shot

# (e) drag-select a range; the status bar shows its Sum.
drag 200 101 290 161; sleep 1
glide 1180 760 30; sleep 3; shot

# (f) a new formula on the pane's last line.
click 230 455; sleep 0.6
type_slow 'Bonus = Revenue * 0.05'; sleep 0.8
xdotool key Return; sleep 1
glide 300 420 15; sleep 3.5; shot

sleep 1
kill -INT "$rec_pid"
wait "$rec_pid" || true
rec_pid=''

# --- Encode ------------------------------------------------------------------
dur=$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$tmp/raw.mkv")
vf='fps=10,scale=960:-1:flags=lanczos'
ffmpeg -loglevel error -y -i "$tmp/raw.mkv" -vf "$vf,palettegen=stats_mode=diff" "$tmp/pal.png"
ffmpeg -loglevel error -y -i "$tmp/raw.mkv" -i "$tmp/pal.png" \
  -lavfi "${vf}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" \
  "$tmp/raw.gif"
gifsicle -O3 --lossy=40 "$tmp/raw.gif" -o "$out/gui-demo.gif"
ffmpeg -loglevel error -y -i "$tmp/raw.mkv" -c:v libx264 -crf 28 -preset slow \
  -pix_fmt yuv420p -movflags +faststart -an "$out/gui-demo.mp4"
printf 'duration %.1fs\n' "$dur"
ls -l "$out"/gui-demo.gif "$out"/gui-demo.mp4 "$out"/gui-workspace.png \
  "$out"/gui-pivot-grid.png "$out"/gui-stacked-axes.png
