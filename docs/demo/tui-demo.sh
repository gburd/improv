#!/usr/bin/env bash
# Record the improv-tui terminal movie: docs/demo/tui-demo.cast and
# docs/src/img/tui-demo.{gif,mp4}. Run from the repo root after
#   cargo build --release -p improv_cli -p improv_tui
# (BIN= the directory with those binaries, default target/release).
# Needs python3 + pexpect, agg, gifsicle and ffmpeg. About 2 minutes.
set -euo pipefail
ROOT=$(pwd)
BIN=${BIN:-$ROOT/target/release}
W=$(mktemp -d)
trap 'find "$W" -delete' EXIT
IMPROV=$BIN/improv bash docs/demo/sample-model.sh "$W/sample.db" >/dev/null

# A clean bash with a short prompt, the release binaries first on PATH.
# Captions (in the bar under the TUI) say what the next keys do.
cd "$W"
PATH=$BIN:$PATH python3 "$ROOT/docs/demo/record.py" "$ROOT/docs/demo/tui-demo.cast" \
	--size 100x30 -- env PS1='$ ' bash --norc --noprofile <<'STEPS'
expect \$ $
sleep 1
say A sales model: Time x Product x Region, prices and quantities
type improv-tui sample.db
sleep 0.6
send \r
expect (│|\x1b\[2;2H)Price\b
caption It opens on Price, the first measure the model defines
sleep 2
caption Arrows move the cursor; Tab steps to the next measure
send \x1b[B
sleep 0.8
send \x1b[B
sleep 0.8
send \t
sleep 1.2
send \t
expect (│|\x1b\[2;2H)Quantity\b
sleep 2.5
caption Quantity is an input: e edits the cell, and 1,500 is a number
send \x1b[A
sleep 0.8
send \x1b[A
sleep 0.8
send e
sleep 1.2
send \x7f\x7f\x7f
sleep 0.5
type 1,500
sleep 1.2
send \r
sleep 2.5
caption Tab on to Revenue = Price * Quantity: already recomputed
send \t
expect (│|\x1b\[2;2H)Revenue\b
sleep 3.5
caption # sets the number format: two decimals, grouped thousands
send #
sleep 1.2
type 2 grouped
sleep 1
send \r
sleep 3.5
caption ] and [ page through Region; p swaps rows and columns
send ]
sleep 2.5
send [
sleep 2
send p
sleep 3.5
caption Shift+arrows select a range; its Sum shows in the status line
send \x1b[1;2B
sleep 0.8
send \x1b[1;2B
sleep 0.8
send \x1b[1;2C
sleep 3.5
caption S saves this layout as a view, named as you type
send S
sleep 1
type By product
sleep 1
send \r
sleep 3
caption q quits; every change was saved as it was made
send q
expect \$ $
caption
sleep 1
say Open it again: it opens on the saved view now
type improv-tui sample.db
sleep 0.6
send \r
expect (│|\x1b\[2;2H)Revenue\b
sleep 4
send q
expect \$ $
sleep 1.5
STEPS

cd "$ROOT"
docs/demo/render.sh tui-demo 58
