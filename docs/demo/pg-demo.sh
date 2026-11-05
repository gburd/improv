#!/usr/bin/env bash
# Record the pg_improv terminal movie: docs/demo/pg-demo.cast and
# docs/src/img/pg-demo.{gif,mp4,png}. One command, from the repo root:
#   docs/demo/pg-demo.sh
# It records in podman (docs/demo/pg-container.sh: rust:1.97-trixie, PGDG
# Postgres 17, pg_improv installed from this checkout), then renders on
# the host (docs/demo/render.sh: agg, gifsicle, ffmpeg, ImageMagick).
# About 4 minutes, plus the container's first build (about 10).
set -euo pipefail
if [ -z "${IMPROV_DEMO_IN_CONTAINER:-}" ]; then
	docs/demo/pg-container.sh docs/demo/pg-demo.sh
	if grep -q ERROR docs/demo/pg-demo.cast; then
		echo "pg-demo.cast shows an ERROR" >&2
		exit 1
	fi
	exec docs/demo/render.sh pg-demo 75
fi

# In the container: a scratch cluster on port 55441, removed on exit.
ROOT=$(pwd)
D=$(mktemp -d /tmp/pgi-demo.XXXX)
export PGHOST=127.0.0.1 PGPORT=55441 PGUSER=improv
export PAGER=cat PSQLRC=/dev/null PSQL_HISTORY=/dev/null
export PATH=$BIN:$PGBIN:$PATH
trap 'pg_ctl -D "$D/data" stop -m fast >/dev/null 2>&1 || true; find "$D" -delete' EXIT
initdb -D "$D/data" -A trust -U improv >/dev/null
cat >>"$D/data/postgresql.conf" <<CONF
port = $PGPORT
listen_addresses = '127.0.0.1'
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'sales'
improv.poll_ms = 100
wal_level = logical
CONF
pg_ctl -D "$D/data" -l "$D/log" -w start >/dev/null 3>&-
createdb sales

# `until` lines poll (unrecorded) so each SELECT runs after the worker caught up.
cd "$D"
python3 "$ROOT/docs/demo/record.py" "$ROOT/docs/demo/pg-demo.cast" --size 100x30 \
	-- env PS1='$ ' bash --norc --noprofile <<'STEPS'
expect \$ $
sleep 1
say Postgres 17 with pg_improv preloaded, and an empty database: sales
type psql sales
send \r
expect sales=\*?# $
sleep 1
comment --
pace 0.03 0.07
say The extension: an improv schema of functions and catalogs
type CREATE EXTENSION pg_improv;
send \r
expect sales=\*?# $
sleep 1
say A table of your own: price and quantity by quarter and product
type CREATE TABLE sales (quarter text, product text, price float8, qty float8,
send \r
type   PRIMARY KEY (quarter, product));
send \r
expect sales=\*?# $
type INSERT INTO sales VALUES ('Q1','Widget',10,100), ('Q2','Widget',10,200),
send \r
type   ('Q1','Gadget',20,30), ('Q2','Gadget',20,60);
send \r
expect sales=\*?# $
sleep 1.5
say A model over it: columns become categories and input measures
type BEGIN;
send \r
expect sales=\*?# $
type SELECT improv.create_model('plan');
send \r
expect sales=\*?# $
type SELECT improv.bind_category('plan', 'Time', 'sales', 'quarter'),
send \r
type   improv.bind_category('plan', 'Product', 'sales', 'product'),
send \r
type   improv.bind_measure('plan', 'Price', 'sales', 'price'),
send \r
type   improv.bind_measure('plan', 'Quantity', 'sales', 'qty');
send \r
expect sales=\*?# $
sleep 1.5
say Formulas name measures and categories, not cells
type SELECT improv.define('plan', 'Revenue = Price * Quantity');
send \r
expect sales=\*?# $
type SELECT improv.define('plan', 'Total = SUM(Revenue OVER Time)');
send \r
expect sales=\*?# $
type COMMIT;
send \r
expect sales=\*?# $
until [ "$(psql -qAtX sales -c 'SELECT count(*) FROM improv_plan.total' 2>/dev/null)" = 2 ]
sleep 1
say Each derived measure is a table in schema improv_plan
type SELECT * FROM improv_plan.total;
send \r
expect sales=\*?# $
sleep 3
say Change the source table; a background worker updates the results
type UPDATE sales SET qty = 300 WHERE quarter = 'Q2' AND product = 'Widget';
send \r
expect sales=\*?# $
until [ "$(psql -qAtX sales -c "SELECT value FROM improv_plan.total WHERE product = 'Widget'")" = 4000 ]
sleep 1
type SELECT * FROM improv_plan.total;
send \r
expect sales=\*?# $
sleep 3
say status: the sync mode, and how the worker applied the change
type SELECT * FROM improv.status('plan') \\gx
send \r
expect sales=\*?# $
sleep 5
say A SQL function written in Improv's formula language
type CREATE FUNCTION bonus(sales float8, target float8) RETURNS float8
send \r
type   LANGUAGE improv AS $$ Ratio = sales / target
send \r
type   MAX2(Ratio - 1, 0) * 1000 $$;
send \r
expect sales=\*?# $
type SELECT bonus(150, 100);
send \r
expect sales=\*?# $
sleep 3
send \\q\r
expect \$ $
comment #
pace 0.06 0.16
say The TUI opens the same model by URL; its edits write to sales
type improv-tui 'postgres://improv@127.0.0.1:55441/sales?sslmode=disable&model=plan'
sleep 0.6
send \r
expect (│|\x1b\[2;2H)Price\b
caption Price, the first measure; Tab steps to Quantity, an input
sleep 1
send \t
expect (│|\x1b\[2;2H)Quantity\b
sleep 2
caption Edit Q1 Gadget's quantity: 30 becomes 150
sleep 0.5
send e
sleep 1
send \x7f\x7f\x7f
type 150
sleep 1
send \r
sleep 2.5
caption Tab to Revenue: Q1 Gadget is now 20 * 150
send \t
expect (│|\x1b\[2;2H)Revenue\b
sleep 4
caption q quits; the edit is already in the sales table
send q
expect \$ $
caption
until [ "$(psql -qAtX sales -c "SELECT value FROM improv_plan.revenue WHERE time = 'Q1' AND product = 'Gadget'")" = 3000 ]
sleep 0.8
say The edit is a row of sales, and Revenue follows it
type psql sales -c "SELECT * FROM sales ORDER BY product, quarter"
send \r
expect \$ $
sleep 2.5
type psql sales -c "SELECT * FROM improv_plan.revenue ORDER BY product, time"
send \r
expect \$ $
sleep 4
STEPS
