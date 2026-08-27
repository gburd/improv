#!/usr/bin/env bash
# Deterministic end-to-end CLI demo used for the README asciinema cast.
# Run from the repo root: docs/demo/cli-demo.sh
set -euo pipefail

IMPROV="${IMPROV:-cargo run --quiet -p improv_cli --}"
DB="$(mktemp -u /tmp/improv-demo-XXXX.db)"
trap 'rm -f "$DB"' EXIT

run() { echo "\$ improv $*"; $IMPROV "$@"; echo; }

echo "# Improv — model Revenue = Price * Quantity over Time x Product"
echo

run init "$DB"
run add-category "$DB" 1 Time
run add-category "$DB" 2 Product
run add-item "$DB" 10 1 2025
run add-item "$DB" 11 1 2026
run add-item "$DB" 20 2 WidgetA
run add-item "$DB" 21 2 WidgetB

run add-measure "$DB" 100 Price number input Product
run add-measure "$DB" 101 Quantity number input Time Product

run set "$DB" 100 10 --at Product=WidgetA
run set "$DB" 100 20 --at Product=WidgetB
run set "$DB" 101 100 --at Time=2025,Product=WidgetA
run set "$DB" 101 100 --at Time=2025,Product=WidgetB
run set "$DB" 101 120 --at Time=2026,Product=WidgetA
run set "$DB" 101 80  --at Time=2026,Product=WidgetB

echo "# Derived measure: Price is broadcast over Time."
run define "$DB" 102 "Revenue = Price * Quantity"
run eval "$DB" 102

echo "# Aggregate Revenue over Time -> Revenue[Product]."
run define "$DB" 103 "TotalRevenue = SUM(Revenue OVER Time)"
run eval "$DB" 103

echo "# External (Python) function, host-side, off the deterministic engine path."
run register-ext "$DB" hypot 2 "result = (args[0]**2 + args[1]**2) ** 0.5"
run define "$DB" 200 "H = CALL(hypot, Price, Price)"
run refresh-ext "$DB" 200
run show "$DB" 200
