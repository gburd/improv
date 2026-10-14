#!/usr/bin/env bash
# L4 benchmark: pg_improv's three sync paths on one cluster.
#   writer latency  = time for the writer's UPDATE+COMMIT to return
#   freshness       = commit -> derived result visible to another session
# Modes: logical (default; worker applies after commit), trigger (in the
# writer's txn), manual + explicit improv.recalc() in the writer's txn.
# Sizes: cells = rows in the bound fact table (one input cell per measure).
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=${PGI_DATA:-$HOME/pgi-bench}
P=${PGI_PORT:-55434}
SIZES=${SIZES:-"10000 100000"}
REPS=${REPS:-15}
OUT=${OUT:-$HOME/pgi-bench.csv}

cleanup() {
	"$PG_BIN/pg_ctl" -D "$D" stop -m fast >/dev/null 2>&1 || true
	if [ -d "$D" ]; then find "$D" -mindepth 1 -delete && rmdir "$D"; fi
}
trap cleanup EXIT
cleanup
"$PG_BIN/initdb" -D "$D" -A trust -U improv >/dev/null
cat >>"$D/postgresql.conf" <<EOF
port = $P
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'bench'
improv.poll_ms = 20
improv.max_cells = 50000000
wal_level = logical
max_replication_slots = 10
max_wal_senders = 10
shared_buffers = 2GB
synchronous_commit = on
EOF
"$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv bench
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d bench -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
q "CREATE EXTENSION pg_improv"
ms() { echo $(( ($(date +%s%N) - $1) / 1000000 )); }
echo "mode,cells,op,writer_ms,fresh_ms" >"$OUT"

setup() { # mode cells
	local mode=$1 n=$2 products=$(( $2 / 4 ))
	q "SELECT improv.drop_model('b')" >/dev/null 2>&1 || true
	q "DROP TABLE IF EXISTS f"
	q "CREATE TABLE f (quarter text, product text, price float8, qty float8, PRIMARY KEY (quarter, product))"
	q "INSERT INTO f SELECT 'Q'||q, 'P'||p, 10 + p % 7, 100 + q FROM generate_series(1,4) q, generate_series(1,$products) p"
	q "SELECT improv.create_model('b')"
	[ "$mode" = logical ] || q "UPDATE improv.model SET sync_mode = 'manual' WHERE name = 'b'"
	q "SELECT improv.bind_category('b','Time','f','quarter')"
	q "SELECT improv.bind_category('b','Product','f','product')"
	q "SELECT improv.bind_measure('b','Price','f','price')"
	q "SELECT improv.bind_measure('b','Quantity','f','qty')"
	q "SELECT improv.define('b','Revenue = Price * Quantity')"
	q "SELECT improv.define('b','Total = SUM(Revenue OVER Time)')"
	case $mode in
	trigger) q "SELECT improv.set_sync_mode('b','trigger')" ;;
	manual) q "SELECT improv.recalc('b')" >/dev/null ;;
	logical) # wait for the worker's slot + catch-up
		for _ in $(seq 1 600); do
			[ "$(q "SELECT count(*) FROM improv_b.revenue")" = "$n" ] && break
			sleep 0.1
		done ;;
	esac
}

REV="SELECT value FROM improv_b.revenue WHERE time='Q1' AND product='P1'"
measure() { # mode cells op i
	local mode=$1 n=$2 op=$3 i=$4 price=$((1000 + $4)) want t0 w
	local upd="UPDATE f SET price = $price WHERE quarter='Q1' AND product='P1'"
	[ "$op" = bulk1k ] && upd="UPDATE f SET qty = qty + 1 WHERE quarter='Q2' AND product <= 'P1999'; $upd"
	want=$(( price * 101 ))
	t0=$(date +%s%N)
	case $mode in
	manual) q "BEGIN; $upd; SELECT improv.recalc('b'); COMMIT;" >/dev/null ;;
	*) q "BEGIN; $upd; COMMIT;" ;;
	esac
	w=$(ms "$t0")
	for _ in $(seq 1 3000); do
		[ "$(q "$REV")" = "$want" ] && break
		sleep 0.005
	done
	echo "$mode,$n,$op,$w,$(ms "$t0")" >>"$OUT"
}

for n in $SIZES; do
	for mode in logical trigger manual; do
		echo "== $mode @ $n cells"
		setup "$mode" "$n"
		for op in single bulk1k; do
			for i in $(seq 1 "$REPS"); do measure "$mode" "$n" "$op" "$i"; done
		done
	done
done

python3 - "$OUT" <<'PY'
import csv, sys, statistics as st, collections
d = collections.defaultdict(list)
for r in csv.DictReader(open(sys.argv[1])):
    d[(int(r["cells"]), r["op"], r["mode"])].append((int(r["writer_ms"]), int(r["fresh_ms"])))
print(f"{'cells':>7} {'op':8} {'mode':8} {'writer p50':>11} {'writer p95':>11} {'fresh p50':>10} {'fresh p95':>10}")
for k in sorted(d):
    w = sorted(x[0] for x in d[k]); f = sorted(x[1] for x in d[k])
    p = lambda a, q: a[min(len(a) - 1, int(round(q * (len(a) - 1))))]
    print(f"{k[0]:>7} {k[1]:8} {k[2]:8} {p(w,.5):>9}ms {p(w,.95):>9}ms {p(f,.5):>8}ms {p(f,.95):>8}ms")
PY
