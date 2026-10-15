#!/usr/bin/env bash
# End-to-end test of pg_improv's logical-decoding sync (L2), with real
# commits. Needs: cargo-pgrx installed and pg_improv installed into the PG
# found by PG_BIN (default: Debian's PG 17). Uses its own data dir and port.
#
#   crates/pg_improv/tests/logical.sh            # run, print latency table
#
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=${PGI_DATA:-$HOME/pgi-logical}
P=${PGI_PORT:-55433}
N_SINGLE=${N_SINGLE:-50}
N_BULK=${N_BULK:-20}

cleanup() {
	"$PG_BIN/pg_ctl" -D "$D" stop -m fast >/dev/null 2>&1 || true
	if [ -d "$D" ]; then find "$D" -mindepth 1 -delete && rmdir "$D"; fi
}
trap '[ -n "${KEEP:-}" ] || cleanup' EXIT
cleanup

"$PG_BIN/initdb" -D "$D" -A trust -U improv >/dev/null
cat >>"$D/postgresql.conf" <<EOF
port = $P
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'improv_e2e'
improv.poll_ms = 50
wal_level = logical
max_replication_slots = 10
max_wal_senders = 10
EOF
"$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d improv_e2e -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv improv_e2e

q "CREATE EXTENSION pg_improv"
q "CREATE TABLE sales (quarter text, product text, price float8, qty float8)"
q "INSERT INTO sales SELECT 'Q'||q, p, CASE p WHEN 'Widget' THEN 10 ELSE 20 END,
     CASE p WHEN 'Widget' THEN 100*q ELSE 30*q END
   FROM generate_series(1,4) q, unnest(ARRAY['Widget','Gadget']) p"
q "SELECT improv.create_model('plan')"
q "SELECT improv.bind_category('plan','Time','sales','quarter')"
q "SELECT improv.bind_category('plan','Product','sales','product')"
q "SELECT improv.bind_measure('plan','Price','sales','price')"
q "SELECT improv.bind_measure('plan','Quantity','sales','qty')"
q "SELECT improv.define('plan','Revenue = Price * Quantity')"
q "SELECT improv.define('plan','Total = SUM(Revenue OVER Time)')"

# wait until VALUE (a SQL scalar) equals WANT, or fail after 10 s
wait_for() { # sql want label
	local t0=$(date +%s%N) v=""
	for _ in $(seq 1 400); do
		v=$(q "$1" || true)
		if [ "$v" = "$2" ]; then
			echo $(( ($(date +%s%N) - t0) / 1000000 ))
			return 0
		fi
		sleep 0.025
	done
	echo "TIMEOUT waiting for $3: got '$v' want '$2'" >&2
	tail -20 "$D/log" >&2
	exit 1
}

REV="SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Widget'"
TOT="SELECT value FROM improv_plan.total WHERE product='Widget'"
# 1. the worker creates the slot and catches up (no explicit recalc)
wait_for "$REV" 1000 "initial catch-up" >/dev/null
wait_for "$TOT" 10000 "initial total" >/dev/null
echo "ok: worker created slot + caught up (Revenue[Q1,Widget]=1000, Total[Widget]=10000)"
q "SELECT slot, lag_bytes IS NOT NULL FROM improv.status('plan')" | sed 's/^/   status: /'

# 2. a committed UPDATE propagates
q "UPDATE sales SET price = 11 WHERE quarter='Q1' AND product='Widget'"
wait_for "$REV" 1100 "update Q1 Widget" >/dev/null
wait_for "$TOT" 10100 "total after update" >/dev/null
echo "ok: committed UPDATE propagated (1100, total 10100)"

# 3. a rolled-back UPDATE never appears
q "BEGIN; UPDATE sales SET price = 999 WHERE quarter='Q1' AND product='Widget'; ROLLBACK;"
sleep 0.5
[ "$(q "$REV")" = "1100" ] || { echo "FAIL: rollback leaked"; exit 1; }
echo "ok: rolled-back UPDATE did not propagate"

# 4. INSERT of a new item and DELETE
q "INSERT INTO sales VALUES ('Q1','Gizmo',5,10)"
wait_for "SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Gizmo'" 50 "insert Gizmo" >/dev/null
q "DELETE FROM sales WHERE product='Gizmo'"
wait_for "SELECT count(*) FROM improv_plan.revenue WHERE product='Gizmo'" 0 "delete Gizmo" >/dev/null
echo "ok: INSERT (new item) and DELETE propagated"

# 5. kill the worker mid-stream; commits while it is down are not lost
q "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'" >/dev/null
q "UPDATE sales SET price = 12 WHERE quarter='Q1' AND product='Widget'"
q "UPDATE sales SET qty = 200 WHERE quarter='Q1' AND product='Widget'"
wait_for "$REV" 2400 "after worker restart" >/dev/null
wait_for "$TOT" $((2400 + 2000 + 3000 + 4000)) "total after restart" >/dev/null
echo "ok: worker killed, restarted, applied both commits made while down (2400)"
q "SELECT rows_applied > 0, last_error IS NULL FROM improv.status('plan')" | sed 's/^/   status: /'

# 6. latency: commit -> result visible
lat() { # n stmt-producer label
	local n=$1 label=$3 i want out=()
	for i in $(seq 1 "$n"); do
		want=$($2 "$i")
		out+=("$(wait_for "$REV" "$want" "$label $i")")
	done
	printf '%s\n' "${out[@]}" | sort -n | awk -v l="$label" -v n="$n" '{a[NR]=$1} END {
		printf "   %-34s n=%-3d p50=%4d ms  p95=%4d ms  max=%4d ms\n", l, n, a[int(NR*0.5+0.5)], a[int(NR*0.95+0.5)], a[NR]}'
}
single() { q "UPDATE sales SET price = $((100 + $1)) WHERE quarter='Q1' AND product='Widget'"; echo $(( (100 + $1) * 200 )); }
q "CREATE TABLE big AS SELECT 'Q'||(g%4+1) quarter, 'P'||g product, 1.0::float8 price, 1.0::float8 qty FROM generate_series(1,2000) g"
q "INSERT INTO sales SELECT * FROM big"
wait_for "SELECT count(*) FROM improv_plan.revenue" 2008 "bulk load" >/dev/null
bulk() { q "UPDATE sales SET qty = qty + 1 WHERE product LIKE 'P%' AND product <= 'P1999'; UPDATE sales SET price = $((300 + $1)) WHERE quarter='Q1' AND product='Widget'"; echo $(( (300 + $1) * 200 )); }
echo "latency (commit -> derived result visible; improv.poll_ms=50; incremental apply):"
lat "$N_SINGLE" single "1-row UPDATE (~2k-cell model)"
lat "$N_BULK" bulk "~2k-row UPDATE (~2k-cell model)"

# 7. a structural change (a new derived measure) rebuilds the live state
q "SELECT improv.define('plan','Double = Revenue * 2')"
q "UPDATE sales SET price = 7 WHERE quarter='Q1' AND product='Widget'"
wait_for "SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'" 2800 "new measure" >/dev/null
echo "ok: a measure defined while live is computed on the next change (2800)"

# 8. incremental at 100k cells: a one-cell UPDATE rewrites exactly the
#    affected result rows (Revenue[Q1,P1] and Total[P1]) and does not rebuild
q "CREATE TABLE wide (quarter text, product text, price float8, qty float8)"
q "INSERT INTO wide SELECT 'Q'||q, 'P'||p, 1, 1 FROM generate_series(1,4) q, generate_series(1,25000) p"
q "SELECT improv.create_model('wide')"
q "SELECT improv.bind_category('wide','Time','wide','quarter')"
q "SELECT improv.bind_category('wide','Product','wide','product')"
q "SELECT improv.bind_measure('wide','Price','wide','price')"
q "SELECT improv.bind_measure('wide','Quantity','wide','qty')"
q "SELECT improv.define('wide','Revenue = Price * Quantity')"
q "SELECT improv.define('wide','Total = SUM(Revenue OVER Time)')"
WREV="SELECT value FROM improv_wide.revenue WHERE time='Q1' AND product='P1'"
q "UPDATE wide SET price = 2 WHERE quarter='Q1' AND product='P1'" # warm the live state
wait_for "$WREV" 2 "wide warm-up" >/dev/null
wait_for "SELECT count(*) FROM improv_wide.revenue" 100000 "wide catch-up" >/dev/null
xmax() { q "SELECT max(xmin::text::bigint) FROM improv_wide.$1"; }
rx=$(xmax revenue) tx=$(xmax total) logpos=$(wc -c <"$D/log")
q "UPDATE wide SET price = 3 WHERE quarter='Q1' AND product='P1'"
wait_for "$WREV" 3 "wide one-cell update" >/dev/null
wait_for "SELECT value FROM improv_wide.total WHERE product='P1'" 6 "wide total" >/dev/null
nr=$(q "SELECT count(*) FROM improv_wide.revenue WHERE xmin::text::bigint > $rx")
nt=$(q "SELECT count(*) FROM improv_wide.total WHERE xmin::text::bigint > $tx")
[ "$nr $nt" = "1 1" ] || { echo "FAIL: one-cell update rewrote $nr revenue + $nt total rows"; exit 1; }
if tail -c +$((logpos + 1)) "$D/log" | grep -q "model wide: rebuilding"; then
	echo "FAIL: one-cell update rebuilt the model"; exit 1
fi
echo "ok: 100k-cell model, one-cell UPDATE rewrote 1 revenue + 1 total row, no rebuild"

# 9. TRUNCATE rebuilds: every result row goes
q "TRUNCATE wide"
wait_for "SELECT count(*) FROM improv_wide.revenue" 0 "truncate" >/dev/null
echo "ok: TRUNCATE emptied the results"

# 10. an aggregated binding (agg => 'sum') over duplicate rows: an UPDATE of
#     one duplicate re-sums its coordinate
q "CREATE TABLE orders (product text, amount float8)"
q "INSERT INTO orders VALUES ('Widget',10), ('Widget',30), ('Gadget',5)"
q "SELECT improv.create_model('agg')"
q "SELECT improv.bind_category('agg','Product','orders','product')"
q "SELECT improv.bind_measure('agg','Amount','orders','amount', agg => 'sum')"
q "SELECT improv.define('agg','Total = Amount * 1')"
AT="SELECT value FROM improv_agg.total WHERE product='Widget'"
wait_for "$AT" 40 "aggregated catch-up" >/dev/null
q "UPDATE orders SET amount = 50 WHERE product='Widget' AND amount = 30"
wait_for "$AT" 60 "aggregated update" >/dev/null
echo "ok: agg => 'sum' over duplicate rows; UPDATE of one duplicate re-summed (60)"

# 11. typed measures: a numeric input feeds a derived boolean
q "CREATE TABLE goods (product text PRIMARY KEY, price numeric)"
q "INSERT INTO goods VALUES ('Widget', 10)"
q "SELECT improv.create_model('typed')"
q "SELECT improv.bind_category('typed','Product','goods','product')"
q "SELECT improv.bind_measure('typed','Price','goods','price')"
q "SELECT improv.define('typed','Pricey = Price > 15')"
PR="SELECT value FROM improv_typed.pricey WHERE product='Widget'"
wait_for "$PR" f "typed catch-up" >/dev/null
q "UPDATE goods SET price = 20"
wait_for "$PR" t "typed update" >/dev/null
echo "ok: a derived boolean follows a committed UPDATE (f -> t)"
echo "PASS"
