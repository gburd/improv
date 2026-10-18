#!/usr/bin/env bash
# pg_improv: trigger vs commit sync, in the writer's transaction.
#   txn100 = one transaction of 100 single-row updates, then COMMIT
#   single = one single-row UPDATE (autocommit)
# Wall time of the whole psql round trip, ms; median of REPS.
#   docs/reviews/bench/bench-pg-commit.sh   (needs pg_improv installed)
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=$(mktemp -d /var/tmp/pgi-bc.XXXX)
P=${PGI_PORT:-55436}
CELLS=${CELLS:-10000}
REPS=${REPS:-9}
cleanup() {
	"$PG_BIN/pg_ctl" -D "$D/data" stop -m immediate >/dev/null 2>&1 || true
	find "$D" -delete 2>/dev/null || true
}
trap cleanup EXIT
"$PG_BIN/initdb" -D "$D/data" -A trust -U improv >/dev/null
cat >>"$D/data/postgresql.conf" <<EOF
port = $P
listen_addresses = ''
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
shared_buffers = 1GB
EOF
"$PG_BIN/pg_ctl" -D "$D/data" -l "$D/log" -w start >/dev/null 3>&-
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv bench
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d bench -qAtX -v ON_ERROR_STOP=1 "$@"; }
q -c "CREATE EXTENSION pg_improv"
q -c "CREATE TABLE f (quarter text, product text, price float8, qty float8, PRIMARY KEY (quarter, product))"
q -c "INSERT INTO f SELECT 'Q'||q, 'P'||p, 10 + p % 7, 100 + q FROM generate_series(1,4) q, generate_series(1,$((CELLS / 4))) p"
for s in "create_model('b')" "set_sync_mode('b','manual')" "bind_category('b','Time','f','quarter')" \
	"bind_category('b','Product','f','product')" "bind_measure('b','Price','f','price')" \
	"bind_measure('b','Quantity','f','qty')" "define('b','Revenue = Price * Quantity')" \
	"define('b','Total = SUM(Revenue OVER Time)')"; do
	q -c "SELECT improv.$s" >/dev/null
done
{
	echo "BEGIN;"
	for i in $(seq 1 100); do echo "UPDATE f SET price = price + 1 WHERE quarter='Q1' AND product='P$i';"; done
	echo "COMMIT;"
} >"$D/txn100.sql"
echo "UPDATE f SET price = price + 1 WHERE quarter='Q2' AND product='P1';" >"$D/single.sql"
ms() { local t0=$(date +%s%N); q -f "$1" >/dev/null; echo $((($(date +%s%N) - t0) / 1000000)); }
med() { sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}'; }
echo "cells=$CELLS reps=$REPS (median wall ms)"
for mode in trigger commit; do
	q -c "SELECT improv.set_sync_mode('b','$mode')"
	for op in txn100 single; do
		ms "$D/$op.sql" >/dev/null # warm-up
		m=$(for _ in $(seq "$REPS"); do ms "$D/$op.sql"; done | med)
		echo "$mode $op ${m}ms"
	done
	want=$(q -c "SELECT sum(price * qty) FROM f WHERE quarter='Q1'")
	got=$(q -c "SELECT sum(value) FROM improv_b.revenue WHERE time='Q1'")
	[ "$want" = "$got" ] || { echo "FAIL: $mode results $got, want $want"; exit 1; }
done
