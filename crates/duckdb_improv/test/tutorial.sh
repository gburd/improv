#!/usr/bin/env bash
# Runs the guide's DuckDB tutorial (docs/src/tutorial-duckdb.md) as
# written: docs/tutorials/mdtest.py feeds every ```sql block to the DuckDB
# CLI, in order, against one scratch database (each block after a LOAD of
# the local build), and checks each ```text block against the output.
#
# Usage:
#   DUCKDB=/path/to/duckdb crates/duckdb_improv/test/tutorial.sh
# EXT defaults to build/release/improv.duckdb_extension (`make release`).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
EXT="${EXT:-$HERE/build/release/improv.duckdb_extension}"
DUCKDB="${DUCKDB:-duckdb}"
DIR="$(mktemp -d "${TMPDIR:-/tmp}/improv_tutorial.XXXXXX")"
trap 'find "$DIR" -mindepth 1 -delete; rmdir "$DIR"' EXIT

[ -f "$EXT" ] || { echo "FAIL: $EXT not found (run 'make release' first)"; exit 1; }
"$DUCKDB" -c "SELECT 'duckdb ' || library_version FROM pragma_version()" -noheader -list
python3 "$ROOT/docs/tutorials/mdtest.py" "$ROOT/docs/src/tutorial-duckdb.md" --dir "$DIR" \
	--sql "'$DUCKDB' -unsigned -noheader -list -bail tutorial.duckdb" \
	--sql-prelude "LOAD '$EXT';
"
