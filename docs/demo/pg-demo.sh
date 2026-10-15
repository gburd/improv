#!/usr/bin/env bash
# Record the pg_improv terminal movie: docs/demo/pg-demo.cast and
# docs/src/img/pg-demo.gif. Run from the repo root after
#   cargo build --release -p improv_cli -p improv_tui
#   (cd crates/pg_improv && cargo pgrx install --release \
#      --pg-config /usr/lib/postgresql/17/bin/pg_config)
# Needs python3 + pexpect, agg and gifsicle. Uses a scratch cluster in
# ~/pgi-demo on port 55441, removed on exit.
set -euo pipefail
ROOT=$(pwd)
BIN=${BIN:-$ROOT/target/release}
PGBIN=${PGBIN:-/usr/lib/postgresql/17/bin}
D=$HOME/pgi-demo
W=$(mktemp -d)
export PGHOST=127.0.0.1 PGPORT=55441 PGUSER=improv
export PAGER=cat PSQLRC=/dev/null PSQL_HISTORY=/dev/null
export PATH=$BIN:$PGBIN:$PATH

stop() {
	[ -n "${KEEP:-}" ] && return
	pg_ctl -D "$D" stop -m fast >/dev/null 2>&1 || true
	for d in "$D" "$W"; do
		if [ -d "$d" ]; then find "$d" -mindepth 1 -delete && rmdir "$d"; fi
	done
}
trap stop EXIT
stop
W=$(mktemp -d)
initdb -D "$D" -A trust -U improv >/dev/null
cat >>"$D/postgresql.conf" <<CONF
port = $PGPORT
listen_addresses = '127.0.0.1'
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'sales'
improv.poll_ms = 100
wal_level = logical
CONF
pg_ctl -D "$D" -l "$D/log" -w start >/dev/null 3>&-
createdb sales

# `until` lines poll (unrecorded) so each SELECT runs after the worker caught up.
python3 "$ROOT/docs/demo/record.py" "$ROOT/docs/demo/pg-demo.cast" \
	-- env PS1='$ ' bash --norc --noprofile <<'STEPS'
expect \$ $
sleep 0.8
type psql sales
send \r
expect sales=\*?# $
sleep 1
pace 0.012 0.03
type CREATE EXTENSION pg_improv;
send \r
expect sales=\*?# $
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
sleep 1
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
type SELECT * FROM improv_plan.total;
send \r
expect sales=\*?# $
sleep 2.5
type UPDATE sales SET qty = 300 WHERE quarter = 'Q2' AND product = 'Widget';
send \r
expect sales=\*?# $
until [ "$(psql -qAtX sales -c "SELECT value FROM improv_plan.total WHERE product = 'Widget'")" = 4000 ]
sleep 1
type SELECT * FROM improv_plan.total;
send \r
expect sales=\*?# $
sleep 2.5
type SELECT * FROM improv.status('plan') \\gx
send \r
expect sales=\*?# $
sleep 3
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
sleep 2.5
send \\q\r
expect \$ $
pace 0.03 0.08
type improv-tui 'postgres://improv@127.0.0.1:55441/sales?sslmode=disable&model=plan'
sleep 0.4
send \r
expect Revenue
sleep 2.5
send \t
sleep 0.4
send \t
sleep 0.4
send \t
expect \x1b\[2;2HQuantity
sleep 2
send e
sleep 1
send \x7f\x7f\x7f
type 150
sleep 0.8
send \r
sleep 1.5
send \t
expect \x1b\[2;2HRevenue
sleep 2.5
send q
expect \$ $
until [ "$(psql -qAtX sales -c "SELECT value FROM improv_plan.revenue WHERE time = 'Q1' AND product = 'Gadget'")" = 3000 ]
sleep 0.8
type psql sales -c "SELECT * FROM sales ORDER BY product, quarter"
send \r
expect \$ $
sleep 1.5
type psql sales -c "SELECT * FROM improv_plan.revenue ORDER BY product, time"
send \r
expect \$ $
sleep 3
STEPS

if grep -q ERROR docs/demo/pg-demo.cast; then
	echo "pg-demo.cast shows an ERROR" >&2
	exit 1
fi
# NeXT-ish grayscale: bg,fg, then the 16 ANSI colors (blue = cursor, cyan = range).
THEME=e6e6e6,1a1a1a,000000,8b1a1a,2d5a2d,6b5600,3c3c3c,5a3c5a,b4b4b4,d0d0d0,555555,aa3333,3a7a3a,8a7300,4a4a4a,7a4f7a,c8c8c8,ffffff
agg --idle-time-limit 1.5 --font-size 15 --theme "$THEME" docs/demo/pg-demo.cast "$W/pg-demo.gif"
gifsicle -O3 --lossy=30 --colors 64 "$W/pg-demo.gif" -o docs/src/img/pg-demo.gif
ls -l docs/src/img/pg-demo.gif
