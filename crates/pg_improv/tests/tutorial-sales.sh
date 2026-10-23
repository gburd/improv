#!/usr/bin/env bash
# Runs the guide's Postgres tutorial (docs/src/tutorial-postgres.md) as
# written: a scratch cluster configured with the page's ```ini block, then
# docs/tutorials/mdtest.py feeds every ```sql block to psql in order and
# checks each ```text block against the output.
#
# Needs pg_improv installed into the PG found by PG_BIN (default: Debian's
# PG 17), and python3. Uses its own data dir and port; KEEP=1 leaves the
# cluster running.
#
#   crates/pg_improv/tests/tutorial-sales.sh
#
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=${PGI_DATA:-$HOME/pgi-tutorial}
P=${PGI_PORT:-55470}
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
PAGE="$ROOT/docs/src/tutorial-postgres.md"

cleanup() {
	"$PG_BIN/pg_ctl" -D "$D" stop -m fast >/dev/null 2>&1 || true
	if [ -d "$D" ]; then find "$D" -mindepth 1 -delete && rmdir "$D"; fi
}
trap '[ -n "${KEEP:-}" ] || cleanup' EXIT
cleanup

"$PG_BIN/initdb" -D "$D" -A trust -U improv >/dev/null
{
	echo "port = $P"
	echo "unix_socket_directories = '$D'"
	# The page's server settings, as a reader copies them.
	awk '/^```ini$/ {on = 1; next} /^```$/ {on = 0} on' "$PAGE"
} >>"$D/postgresql.conf"
grep -q "^improv.database = 'shop'" "$D/postgresql.conf"
# 3>&- : do not hand the server a pipe the caller is waiting on
"$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null 3>&-
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv shop

if ! python3 "$ROOT/docs/tutorials/mdtest.py" "$PAGE" --dir "$D" \
	--sql "'$PG_BIN/psql' -h '$D' -p $P -U improv -d shop -qAtX -v ON_ERROR_STOP=1"; then
	tail -20 "$D/log" >&2
	exit 1
fi
