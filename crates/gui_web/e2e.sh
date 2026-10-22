#!/usr/bin/env bash
# End-to-end check of the web spike: build a model with the CLI, serve it
# with improv-server, host the page, and drive it in headless Chromium.
#   e2e.sh <improv> <improv-server> <screenshot.png>
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
I=$1 S=$2 SHOT=$3
d=$(mktemp -d -p "$HOME" gw-e2e.XXXX)
q() { "$I" "$@" > /dev/null; }
db=$d/m.db
q init "$db"
q add-category "$db" 1 Time
id=10
for y in 2024 2025; do for k in 1 2 3 4; do q add-item "$db" $id 1 "${y}Q$k"; id=$((id+1)); done; done
q add-category "$db" 2 Product
q add-item "$db" 20 2 Widget; q add-item "$db" 21 2 Gadget; q add-item "$db" 22 2 Gizmo
q add-measure "$db" 100 Price number input Product
q add-measure "$db" 101 Quantity number input Time Product
for p in Widget:12 Gadget:30 Gizmo:5; do q set "$db" 100 "${p#*:}" --at "Product=${p%:*}"; done
qty=100
for t in 2024Q1 2024Q2 2024Q3 2024Q4 2025Q1 2025Q2 2025Q3 2025Q4; do
  for p in Widget Gadget Gizmo; do q set "$db" 101 $qty --at "Time=$t,Product=$p"; qty=$((qty+7)); done
done
q define "$db" 102 "Revenue = Price * Quantity"

"$S" "$db" 127.0.0.1:38731 > "$d/server.log" 2>&1 &
spid=$!
python3 "$here/serve.py" 38732 http://127.0.0.1:38731 &
hpid=$!
trap 'kill $spid $hpid 2>/dev/null; find "$d" -delete' EXIT
sleep 1.5
timeout 120 python3 "$here/drive.py" http://127.0.0.1:38732/ "$SHOT" ${PROBE:+--probe}
echo "the store after the edit:"
"$I" show "$db" 101 | grep -F 'Time=2024Q1' | grep -F 'Product=Widget'
