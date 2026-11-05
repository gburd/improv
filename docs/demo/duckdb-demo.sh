#!/usr/bin/env bash
# Record the DuckDB movie: docs/demo/duckdb-demo.cast and
# docs/src/img/duckdb-demo.{gif,mp4,png}. One command, from the repo root:
#   docs/demo/duckdb-demo.sh
# It runs the DuckDB tutorial's queries (docs/src/tutorial-duckdb.md) in the
# DuckDB v1.5.6 CLI, which loads the improv extension. Both are downloaded
# into DUCK (default ~/.cache/improv-demo/duckdb) and checked:
#   * the extension: the v0.12.2 GitHub Release's linux_amd64 build, checked
#     against the release's SHA256SUMS. crates/duckdb_improv is unchanged
#     since v0.12.2 (git log v0.12.2.. -- crates/duckdb_improv is empty);
#     after a change, build it instead (make release in crates/duckdb_improv)
#     and pass EXT=path/to/improv.duckdb_extension.
#   * the CLI: duckdb_cli-linux-amd64.zip of DuckDB v1.5.6, checked against
#     the digest GitHub lists for that release asset.
# Needs gh, curl, unzip, python3 + pexpect, agg, gifsicle, ffmpeg and
# ImageMagick, on x86_64 Linux. About 2 minutes.
set -euo pipefail
ROOT=$(pwd)
DUCK=${DUCK:-$HOME/.cache/improv-demo/duckdb}
mkdir -p "$DUCK"
if [ -z "${EXT:-}" ]; then
	EXT=$DUCK/improv-0.12.2-duckdb-linux_amd64/improv.duckdb_extension
	if [ ! -f "$EXT" ]; then
		(cd "$DUCK" && gh release download v0.12.2 -R gburd/improv --clobber \
			-p improv-0.12.2-duckdb-linux_amd64.tar.gz -p SHA256SUMS &&
			grep ' improv-0.12.2-duckdb-linux_amd64.tar.gz$' SHA256SUMS | sha256sum -c &&
			tar xzf improv-0.12.2-duckdb-linux_amd64.tar.gz)
	fi
fi
if [ ! -x "$DUCK/duckdb" ]; then
	want=$(gh api repos/duckdb/duckdb/releases/tags/v1.5.6 \
		-q '.assets[] | select(.name == "duckdb_cli-linux-amd64.zip") | .digest')
	curl -sSfL -o "$DUCK/cli.zip" \
		https://github.com/duckdb/duckdb/releases/download/v1.5.6/duckdb_cli-linux-amd64.zip
	echo "${want#sha256:}  $DUCK/cli.zip" | sha256sum -c
	(cd "$DUCK" && unzip -o -q cli.zip duckdb)
fi
"$DUCK/duckdb" --version | grep -q '^v1\.5\.6 '

W=$(mktemp -d)
trap 'find "$W" -delete' EXIT
cp "$EXT" "$W/improv.duckdb_extension"
cd "$W"
python3 "$ROOT/docs/demo/record.py" "$ROOT/docs/demo/duckdb-demo.cast" --size 100x30 \
	-- "$DUCK/duckdb" -unsigned tickets.duckdb <<'STEPS'
expect D $
sleep 1
comment --
pace 0.03 0.07
say DuckDB v1.5.6, started -unsigned: load the extension from a file
type LOAD './improv.duckdb_extension';
send \r
expect D $
say improv_calc: one formula in Improv's language, as a scalar function
type SELECT improv_calc('ROUND(PMT(rate/12, n, -pv), 2)',
send \r
type   json_object('rate', 0.06, 'n', 360, 'pv', 200000)) AS payment;
send \r
expect D $
sleep 3
clear D 
say The tutorial's tickets, written to a Parquet file
type CREATE TABLE tickets AS SELECT * FROM (VALUES
send \r
type   (1, DATE '2026-03-02', 'Billing', 1.5), (2, DATE '2026-03-03', 'Billing', 0.5),
send \r
type   (3, DATE '2026-03-03', 'Platform', 4.0), (4, DATE '2026-03-04', 'Platform', 2.0),
send \r
type   (5, DATE '2026-03-05', 'Platform', 3.0), (6, DATE '2026-03-09', 'Billing', 1.0),
send \r
type   (7, DATE '2026-03-10', 'Billing', 2.0), (8, DATE '2026-03-10', 'Billing', 1.5),
send \r
type   (9, DATE '2026-03-11', 'Platform', 6.0), (10, DATE '2026-03-12', 'Mobile', 2.5),
send \r
type   (11, DATE '2026-03-13', 'Mobile', 1.5)) t(id, opened, team, hours);
send \r
expect D $
type COPY tickets TO 'tickets.parquet' (FORMAT parquet); DROP TABLE tickets;
send \r
expect D $
sleep 1.5
say The model's source: a view of week, team, hours and a count of 1
type CREATE VIEW ticket_rows AS SELECT strftime(opened, '%G-W%V') AS week,
send \r
type   team, hours, 1 AS n FROM 'tickets.parquet';
send \r
expect D $
sleep 1.5
clear D 
say improv(source, model): the model is text; derived cells come back
type SELECT measure, "Week", "Team", value_text FROM improv('ticket_rows', '
send \r
type   category Week = week
send \r
type   category Team = team
send \r
type   input Hours = hours sum
send \r
type   WeekHours = SUM(Hours OVER Team)
send \r
type   Share = ROUND(Hours / WeekHours, 3)
send \r
type   Change = Hours - PREV(Hours, Week, 1, 0)') WHERE measure <> 'WeekHours';
send \r
expect D $
sleep 6
clear D 
say It is a table function, so the rest of SQL applies: PIVOT
type PIVOT (SELECT "Week", "Team", value FROM improv('ticket_rows', '
send \r
type   category Week = week
send \r
type   category Team = team
send \r
type   input Tickets = n sum
send \r
type   input Hours = hours sum
send \r
type   PerTicket = Hours / Tickets') WHERE measure = 'PerTicket')
send \r
type ON "Team" USING first(value) GROUP BY "Week" ORDER BY "Week";
send \r
expect D $
sleep 5
say The source can be a query: last week's tickets only
type SELECT "Team", value FROM improv(
send \r
type   'SELECT team, hours FROM ''tickets.parquet'' WHERE opened >= DATE ''2026-03-09''',
send \r
type   'category Team = team
send \r
type    input Hours = hours sum
send \r
type    Share = ROUND(Hours / SUM(Hours OVER Team), 2)') ORDER BY value DESC;
send \r
expect D $
sleep 5
send .quit\r
sleep 0.5
STEPS

cd "$ROOT"
if grep -q 'Error' docs/demo/duckdb-demo.cast; then
	echo "duckdb-demo.cast shows an error" >&2
	exit 1
fi
docs/demo/render.sh duckdb-demo 96
