#!/usr/bin/env bash
# asciinema demo: build a small model, then flow in new numbers and watch the
# derived measures recompute. Recorded to docs/demo/recalc-demo.cast.
#   docs/demo/recalc-demo.sh
#
# To (re)record and render the embedded SVG:
#   asciinema rec --overwrite -c "bash docs/demo/recalc-demo.sh" docs/demo/recalc-demo.cast
#   asciinema convert -f asciicast-v2 docs/demo/recalc-demo.cast /tmp/recalc-v2.cast
#   npx --yes svg-term-cli --in /tmp/recalc-v2.cast --out docs/img/recalc-demo.svg \
#       --window --width 80 --height 34
set -euo pipefail

IMPROV="${IMPROV:-cargo run --quiet -p improv_cli --}"
DB="$(mktemp -u /tmp/improv-recalc-XXXX.db)"
trap 'rm -f "$DB"' EXIT

run()  { printf '\033[1;32m$ improv %s\033[0m\n' "$*"; $IMPROV "$@"; }
note() { printf '\033[1;36m# %s\033[0m\n' "$*"; }
pause(){ sleep "${1:-1.1}"; }

note "Model: Revenue = Price x Quantity over Time x Product"; pause
run init "$DB"; pause 0.5
run add-category "$DB" 1 Time
run add-item "$DB" 10 1 Jan; run add-item "$DB" 11 1 Feb
run add-category "$DB" 2 Product
run add-item "$DB" 20 2 WidgetA; run add-item "$DB" 21 2 WidgetB
pause 0.6

run add-measure "$DB" 100 Price number input Product
run add-measure "$DB" 101 Quantity number input Time Product
run set "$DB" 100 10 --at Product=WidgetA
run set "$DB" 100 20 --at Product=WidgetB
run set "$DB" 101 100 --at Time=Jan,Product=WidgetA
run set "$DB" 101 50  --at Time=Jan,Product=WidgetB
run set "$DB" 101 120 --at Time=Feb,Product=WidgetA
run set "$DB" 101 60  --at Time=Feb,Product=WidgetB
pause 0.6

note "Derived measures — formulas over names, incrementally recalculated"; pause
run define "$DB" 200 "Revenue = Price * Quantity"
run define "$DB" 201 "TotalByProduct = SUM(Revenue OVER Time)"
run eval "$DB" 200; pause 1.2
run eval "$DB" 201; pause 1.4

note "Now flow in new numbers — watch Revenue + the aggregate recompute"; pause 1.2
for q in 150 200 275; do
  note "Quantity[Feb, WidgetA] <- $q"
  run set "$DB" 101 "$q" --at Time=Feb,Product=WidgetA
  run eval "$DB" 200 | grep -E "Feb.*WidgetA|cells"
  run eval "$DB" 201 | grep -E "WidgetA|cells"
  pause 1.3
done

note "Every edit propagates as a delta through differential dataflow —"
note "only the touched cells (and their dependents) recompute."
pause 1.4
