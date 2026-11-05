#!/usr/bin/env bash
# Record the HTTP server movie: docs/demo/server-demo.cast and
# docs/src/img/server-demo.{gif,mp4,png}. Run from the repo root after
#   cargo build --release -p improv_cli -p improv_server
# (BIN= the directory with those binaries, default target/release).
# Needs python3 + pexpect, curl, jq, agg, gifsicle, ffmpeg and ImageMagick.
# Uses port 3071 on loopback. About 2 minutes.
set -euo pipefail
ROOT=$(pwd)
BIN=${BIN:-$ROOT/target/release}
W=$(mktemp -d)
trap 'pkill -f "improv-server sample.db 127.0.0.1:3071" || true; find "$W" -delete' EXIT
IMPROV=$BIN/improv bash docs/demo/sample-model.sh "$W/sample.db" >/dev/null

# One shell: the server and the change stream run in the background,
# their output interleaved with the requests that cause it.
cd "$W"
PATH=$BIN:$PATH python3 "$ROOT/docs/demo/record.py" "$ROOT/docs/demo/server-demo.cast" \
	--size 100x30 -- env PS1='$ ' bash --norc --noprofile <<'STEPS'
expect \$ $
sleep 1
pace 0.04 0.1
say Serve the sample model over HTTP, with one write token
type export TOKEN=demo-secret H="Authorization: Bearer demo-secret"
send \r
expect \$ $
type IMPROV_API_TOKEN=$TOKEN IMPROV_AUDIT_LOG=audit.log improv-server sample.db 127.0.0.1:3071 &
send \r
expect listening on
sleep 2
send \r
expect \$ $
say Without the token every route but /v1/health is refused
type curl -s localhost:3071/v1/structure; echo
send \r
expect \$ $
sleep 2
say /v1/structure: categories with their items, measures, formulas
type curl -s -H "$H" localhost:3071/v1/structure | jq -c '.categories[], .measures[0,3]'
send \r
expect \$ $
sleep 5
clear $ 
say A window of Revenue's cells, computed by the server: 2024Q1, North
type curl -s -H "$H" --json '{"at": {"1": [10], "3": [30]}}' \\
send \r
type   localhost:3071/v1/measures/200/window | jq -c '.cells[]'
send \r
expect \$ $
sleep 4
say Watch the change stream (Server-Sent Events) in the background
type curl -sN -H "$H" localhost:3071/v1/changes | sed -un 's/^data: //p' \\
send \r
type   | jq --unbuffered -c '.cells[]? // .' &
send \r
expect "version"
sleep 2.5
send \r
expect \$ $
say Edit one cell: Quantity at 2024Q1, Widget, North becomes 75
type curl -s -H "$H" --json '{"edits": [{"measure": 102,
send \r
type   "at": [[1, 10], [2, 20], [3, 30]], "value": 75}]}' localhost:3071/v1/cells \\
send \r
type   | jq -c .message; sleep 0.5
send \r
expect 204
sleep 1
send \r
expect \$ $
say The stream sent the input and each derived cell it moved
sleep 3
clear $ 
say Every write is audited: who, the route and the answer's status
type jq -c . audit.log
send \r
expect \$ $
sleep 4
STEPS

cd "$ROOT"
docs/demo/render.sh server-demo 95
