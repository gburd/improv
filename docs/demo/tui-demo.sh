#!/usr/bin/env bash
# Record the improv-tui terminal movie: docs/demo/tui-demo.cast and
# docs/src/img/tui-demo.gif. Run from the repo root after
#   cargo build --release -p improv_cli -p improv_tui
# Needs python3 + pexpect, agg and gifsicle.
set -euo pipefail
ROOT=$(pwd)
BIN=${BIN:-$ROOT/target/release}
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
IMPROV=$BIN/improv bash docs/demo/sample-model.sh "$W/sample.db" >/dev/null

# A clean bash with a short prompt, the release binaries first on PATH.
cd "$W"
PATH=$BIN:$PATH python3 "$ROOT/docs/demo/record.py" "$ROOT/docs/demo/tui-demo.cast" \
	-- env PS1='$ ' bash --norc --noprofile <<'STEPS'
expect \$ $
sleep 0.8
type improv-tui sample.db
sleep 0.5
send \r
expect Revenue
sleep 3
# The grid opens on Revenue; move the cursor around.
send \x1b[B
sleep 0.6
send \x1b[C
sleep 0.6
send \x1b[B
sleep 1
# Tab cycles the measures; seven steps reach the input Quantity.
send \t
sleep 0.3
send \t
sleep 0.3
send \t
sleep 0.3
send \t
sleep 0.3
send \t
sleep 0.3
send \t
sleep 0.3
send \t
expect \x1b\[2;2HQuantity
sleep 2.5
# Edit the cell: the buffer is seeded with the old value.
send e
sleep 1.2
send \x7f\x7f\x7f
sleep 0.4
type 500
sleep 1
send \r
sleep 2.5
# Tab wraps to Revenue = Price * Quantity, already recomputed.
send \t
expect \x1b\[2;2HRevenue
sleep 4
# Page through Region (South, back to North), then pivot the axes.
send ]
sleep 2.5
send [
sleep 2
send p
sleep 3.5
# Shift+arrows grow a range; its Sum shows in the status line.
send \x1b[A
sleep 0.4
send \x1b[A
sleep 0.4
send \x1b[D
sleep 0.8
send \x1b[1;2B
sleep 0.6
send \x1b[1;2B
sleep 0.6
send \x1b[1;2C
sleep 3
# Save the layout as a view, cycle views, quit (autosaves).
send S
sleep 2.5
send v
sleep 2.5
send q
expect \$ $
sleep 1
STEPS

cd "$ROOT"
# NeXT-ish grayscale: bg,fg, then the 16 ANSI colors (blue = cursor, cyan = range).
THEME=e6e6e6,1a1a1a,000000,8b1a1a,2d5a2d,6b5600,3c3c3c,5a3c5a,b4b4b4,d0d0d0,555555,aa3333,3a7a3a,8a7300,4a4a4a,7a4f7a,c8c8c8,ffffff
agg --idle-time-limit 1.5 --font-size 15 --theme "$THEME" \
	docs/demo/tui-demo.cast "$W/tui-demo.gif"
gifsicle -O3 --lossy=30 --colors 64 "$W/tui-demo.gif" -o docs/src/img/tui-demo.gif
ls -l docs/src/img/tui-demo.gif
