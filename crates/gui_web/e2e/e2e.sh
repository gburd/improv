#!/usr/bin/env bash
# The web GUI end to end: build a Time x Product model with the CLI, serve
# it and the web build with improv-server, and drive two browser sessions
# in headless Chromium (drive.py).
#   e2e.sh <improv> <improv-server> <web-dist-dir> <out-dir>
# Needs chromium (CHROMIUM=…) and python3 with websocket-client.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
I=$1 S=$2 DIST=$3 OUT=$4
mkdir -p "$OUT"
d=$(mktemp -d -p "${SCRATCH:-${TMPDIR:-/tmp}}" improv-web-e2e.XXXX)
q() { "$I" "$@" > /dev/null; }
db=$d/m.db
q init "$db"
q add-category "$db" 1 Time
id=10
for y in 2025 2026; do for k in 1 2 3 4; do q add-item "$db" $id 1 "${y}Q$k"; id=$((id+1)); done; done
q add-category "$db" 2 Product
q add-item "$db" 20 2 Widget; q add-item "$db" 21 2 Gadget; q add-item "$db" 22 2 Gizmo
q add-measure "$db" 100 Price number input Product
q add-measure "$db" 101 Quantity number input Time Product
for p in Widget:12 Gadget:30 Gizmo:5; do q set "$db" 100 "${p#*:}" --at "Product=${p%:*}"; done
qty=100
for t in 2025Q1 2025Q2 2025Q3 2025Q4 2026Q1 2026Q2 2026Q3 2026Q4; do
  for p in Widget Gadget Gizmo; do q set "$db" 101 $qty --at "Time=$t,Product=$p"; qty=$((qty+7)); done
done
q define "$db" 102 "Revenue = Price * Quantity"

port=${PORT:-38731}
token=web-e2e-$RANDOM$RANDOM
serve() {
  IMPROV_API_TOKEN=$token "$S" "$db" "127.0.0.1:$port" --static "$DIST" >> "$d/server.log" 2>&1 &
  echo $! > "$d/pid"
  for _ in $(seq 50); do curl -fs "http://127.0.0.1:$port/v1/health" > /dev/null && return; sleep 0.1; done
  echo "e2e: improv-server did not start" >&2; cat "$d/server.log" >&2; return 1
}
# Stop the server and reap it. `kill -0` alone loops forever on a zombie,
# which a container's init may never reap: `wait` reaps it when it is ours,
# and `ps` tells a zombie (Z) from a live process otherwise; 10 s at most.
stop() {
  local p; p=$(cat "$d/pid")
  kill "$p" 2>/dev/null || true
  wait "$p" 2>/dev/null || true
  for _ in $(seq 200); do
    kill -0 "$p" 2>/dev/null || return 0
    [ "$(ps -o stat= -p "$p" 2>/dev/null | cut -c1)" = Z ] && return 0
    sleep 0.05
  done
  kill -9 "$p" 2>/dev/null || true
}
trap 'stop; find "$d" -delete' EXIT
serve
export -f serve stop
export token db port S DIST d
# Another program adds Region and a measure over it while the pages are open.
ADD_REGION="$(printf '%q ' "$I") add-category $db 3 Region >/dev/null
  $(printf '%q ' "$I") add-item $db 30 3 North >/dev/null
  $(printf '%q ' "$I") add-item $db 31 3 South >/dev/null
  $(printf '%q ' "$I") add-measure $db 103 Sales number input Time Product Region >/dev/null
  $(printf '%q ' "$I") set $db 103 9 --at Time=2025Q1,Product=Widget,Region=South >/dev/null"
export ADD_REGION
RESTART='stop; sleep 1; serve' timeout 180 python3 "$here/drive.py" "http://127.0.0.1:$port/" "$token" "$OUT"
echo "the store after the edits:"
"$I" show "$db" 101 | grep -F 'Time=2025Q1' | grep -F 'Product=Widget'
