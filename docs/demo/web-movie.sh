#!/usr/bin/env bash
# Record the web GUI demo movie: docs/src/img/web-demo.{mp4,gif,png}.
#
#   docs/demo/web-movie.sh [OUTDIR]       (OUTDIR defaults to docs/src/img)
#
# web-stills.sh's model and server (a Time x Product model, served with a
# token on 127.0.0.1:38742), then docs/demo/web-movie.py drives two
# browser sessions in headless Chromium and saves each one's screencast:
# a login, a pivot and back, an edit in one browser reaching the other
# with the server's new Revenue, a refused formula with its error located,
# an undo, and the Model Browser. This script cuts the frames into a 15 fps
# movie: one session full size, or the two side by side while the edit
# travels, with a caption per beat in a band under the picture (Arimo, as
# the other movies).
#
# Needs what web-stills.sh needs (the wasm32 target, wasm-bindgen-cli at
# Cargo.lock's version, Chromium as CHROMIUM=, python3 with websocket-client
# as PYTHON=, curl, ImageMagick), plus gifsicle and an ffmpeg with drawtext:
# `nix shell nixpkgs#ffmpeg-full nixpkgs#gifsicle`. About a minute after the
# build; the movie is about 50 s.
set -euo pipefail

die() { echo "web-movie: $*" >&2; exit 1; }
for t in ffmpeg ffprobe gifsicle magick curl; do command -v "$t" >/dev/null || die "missing $t"; done
root=$(cd "$(dirname "$0")/../.." && pwd)
out=${1:-$root/docs/src/img}
mkdir -p "$out"
out=$(cd "$out" && pwd)
cd "$root"
font=$root/crates/gui/assets/fonts/Arimo-Regular.ttf
[[ -f $font ]] || die "missing $font"
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

# The model of web-stills.sh.
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

port=38742
token=web-movie-$RANDOM$RANDOM
IMPROV_API_TOKEN=$token "$S" "$db" "127.0.0.1:$port" --static "$target/gui_web" \
  >"$d/server.log" 2>&1 </dev/null &
pid=$!
for _ in $(seq 50); do
  curl -fs "http://127.0.0.1:$port/v1/health" >/dev/null && break
  sleep 0.1
done
curl -fs "http://127.0.0.1:$port/v1/health" >/dev/null || die "improv-server did not start: $(cat "$d/server.log")"

f=$d/frames
mkdir -p "$f"
SCRATCH=$d timeout 300 "${PYTHON:-python3}" docs/demo/web-movie.py \
  "http://127.0.0.1:$port/" "$token" "$f" || die "the browser run failed"

# --- Cut ---------------------------------------------------------------------
# Each session's frames, held until the next one, as a constant-rate video
# from the run's start (frame times in a.txt / b.txt).
W=1280 H=800 BAND=44
start=$(head -1 "$f/beats.txt" | cut -f1)
end=$(tail -1 "$f/beats.txt" | cut -f1)
for s in a b; do
  awk -v s="$s" -v t0="$start" -v t1="$end" -v dir="$f" '
    { n[NR] = $1; t[NR] = $2 }
    END {
      for (i = 1; i <= NR; i++) {
        a = (i == 1) ? t0 : t[i]; b = (i < NR) ? t[i + 1] : t1
        if (b <= t0) continue
        if (a < t0) a = t0
        if (b > t1) b = t1
        if (b <= a) continue
        printf "file \x27%s/%s-%d.jpg\x27\nduration %.3f\n", dir, s, n[i], b - a
        last = n[i]
      }
      printf "file \x27%s/%s-%d.jpg\x27\n", dir, s, last
    }' "$f/$s.txt" >"$f/$s.list"
  ffmpeg -loglevel error -y -f concat -safe 0 -i "$f/$s.list" -vf "fps=15,scale=$W:$H" \
    -c:v libx264 -preset veryfast -crf 16 -pix_fmt yuv444p "$f/$s.mkv"
done

# Per beat: A full size ("a"), or A and B side by side ("ab": each one's
# Revenue and Quantity windows, the region web-stills.sh crops), and the
# caption under it.
graph=$f/graph.txt
: >"$graph"
mapfile -t beats <"$f/beats.txt"
n=0
for ((i = 0; i + 1 < ${#beats[@]}; i++)); do
  IFS=$'\t' read -r t who text <<<"${beats[i]}"
  t1=$(cut -f1 <<<"${beats[i + 1]}")
  a=$(awk -v x="$t" -v s="$start" 'BEGIN { printf "%.3f", x - s }')
  b=$(awk -v x="$t1" -v s="$start" 'BEGIN { printf "%.3f", x - s }')
  printf '%s' "$text" >"$f/cap-$n.txt"
  cap="drawtext=fontfile=$font:textfile=$f/cap-$n.txt:fontsize=22:fontcolor=white:x=16:y=$((H + (BAND - 22) / 2))"
  if [[ $who == ab ]]; then
    {
      echo "[0:v]trim=$a:$b,setpts=PTS-STARTPTS,crop=452:402:171:207[a$n];"
      echo "[1:v]trim=$a:$b,setpts=PTS-STARTPTS,crop=452:402:171:207[b$n];"
      echo "[a$n][b$n]hstack,pad=916:402:0:0:color=0x333333,scale=1145:502,"
      echo "pad=$W:$((H + BAND)):(ow-iw)/2:(${H}-ih)/2:color=0x333333,$cap,setsar=1[v$n];"
    } >>"$graph"
  else
    echo "[0:v]trim=$a:$b,setpts=PTS-STARTPTS,pad=$W:$((H + BAND)):0:0:color=0x2a2a2a,$cap,setsar=1[v$n];" >>"$graph"
  fi
  n=$((n + 1))
done
for ((i = 0; i < n; i++)); do printf '[v%d]' "$i" >>"$graph"; done
echo "concat=n=$n:v=1:a=0[out]" >>"$graph"
ffmpeg -loglevel error -y -i "$f/a.mkv" -i "$f/b.mkv" -filter_complex_script "$graph" -map '[out]' \
  -c:v libx264 -preset veryslow -crf 29 -r 15 -pix_fmt yuv420p -movflags +faststart -an \
  "$out/web-demo.mp4"

# --- GIF and poster ----------------------------------------------------------
g='fps=8,scale=960:-1:flags=lanczos'
ffmpeg -loglevel error -y -i "$out/web-demo.mp4" -vf "$g,palettegen=max_colors=64:stats_mode=diff" "$d/pal.png"
ffmpeg -loglevel error -y -i "$out/web-demo.mp4" -i "$d/pal.png" \
  -lavfi "${g}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" "$d/raw.gif"
gifsicle -O3 --lossy=30 --colors 64 "$d/raw.gif" -o "$out/web-demo.gif"
# The poster: the two browsers, just after the edit arrived.
poster=$(awk -F'\t' -v s="$start" '$2 == "ab" { t = $1 } END { printf "%.2f", t - s + 2 }' "$f/beats.txt")
ffmpeg -loglevel error -y -ss "$poster" -i "$out/web-demo.mp4" -frames:v 1 "$d/poster.png"
magick "$d/poster.png" -strip -colors 64 -define png:compression-level=9 "$out/web-demo.png"
printf 'duration %.1fs\n' "$(ffprobe -v error -show_entries format=duration -of csv=p=0 "$out/web-demo.mp4")"
awk -F'\t' -v s="$start" '{ printf "%6.2f %-3s %s\n", $1 - s, $2, $3 }' "$f/beats.txt"
ls -l "$out"/web-demo.mp4 "$out"/web-demo.gif "$out"/web-demo.png
