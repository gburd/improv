#!/usr/bin/env bash
# Record the CLI terminal movie: docs/demo/cli-demo.cast and
# docs/src/img/cli-demo.{gif,mp4,png}. Run from the repo root after
#   cargo build --release -p improv_cli
# (BIN= the directory with improv, default target/release).
# Needs python3 + pexpect, agg, gifsicle, ffmpeg and ImageMagick. About 3
# minutes.
set -euo pipefail
ROOT=$(pwd)
BIN=${BIN:-$ROOT/target/release}
W=$(mktemp -d)
trap 'find "$W" -delete' EXIT

# `clear` steps wipe the screen between steps (the prompt redrawn).
cd "$W"
PATH=$BIN:$PATH python3 "$ROOT/docs/demo/record.py" "$ROOT/docs/demo/cli-demo.cast" \
	--size 100x26 -- env PS1='$ ' bash --norc --noprofile <<'STEPS'
expect \$ $
sleep 1
pace 0.05 0.12
say improv builds a model one command at a time, each saved to the file
type improv init m.db
send \r
expect \$ $
say Two categories with two items each, by id
type improv add-category m.db 1 Time && improv add-item m.db 10 1 Q1 && improv add-item m.db 11 1 Q2
send \r
expect \$ $
type improv add-category m.db 2 Product && improv add-item m.db 20 2 Widget
send \r
expect \$ $
type improv add-item m.db 21 2 Gadget
send \r
expect \$ $
sleep 1
clear $ 
say Input measures: Price by Product, Quantity by Time and Product
type improv add-measure m.db 100 Price number input Product
send \r
expect \$ $
type improv add-measure m.db 101 Quantity number input Time Product
send \r
expect \$ $
say Some cells, by name (Gadget's Q1 quantity stays empty)
type improv set m.db Price 10 --at Product=Widget && improv set m.db Price 25 --at Product=Gadget
send \r
expect \$ $
type improv set m.db Quantity 100 --at Time=Q1,Product=Widget
send \r
expect \$ $
type improv set m.db Quantity 120 --at Time=Q2,Product=Widget
send \r
expect \$ $
type improv set m.db Quantity 40 --at Time=Q2,Product=Gadget
send \r
expect \$ $
sleep 1
clear $ 
say Formulas name measures and categories, not cells
type improv define m.db 102 "Revenue = Price * Quantity"
send \r
expect \$ $
type improv define m.db 103 "Total = SUM(Revenue OVER Time)"
send \r
expect \$ $
say eval computes a measure; Price is the same in every quarter
type improv eval m.db Revenue
send \r
expect \$ $
sleep 3
say A number format: two decimals, grouped thousands
type improv format m.db Total 2 grouped && improv eval m.db Total
send \r
expect \$ $
sleep 3.5
clear $ 
say A policy template: ann reads everything except Gadget
type improv policy template m.db analyst --user ann --item Product=Gadget > ann.cedar
send \r
expect \$ $
type improv policy set m.db ann.cedar
send \r
expect \$ $
sleep 1.5
say check asks one question and names the policies that decided
type improv policy check m.db --user ann ReadCell --measure Revenue --at Time=Q2 Product=Gadget
send \r
expect \$ $
sleep 4
type improv policy check m.db --user ann ReadItem --item Product=Widget
send \r
expect \$ $
sleep 3.5
clear $ 
say Export a measure's cells as CSV, for anything downstream
type improv export-csv m.db revenue.csv Revenue && cat revenue.csv
send \r
expect \$ $
sleep 4
STEPS

cd "$ROOT"
docs/demo/render.sh cli-demo 142
