#!/usr/bin/env bash
# End-to-end test of pg_improv's logical-decoding sync (L2), with real
# commits. Section 13 covers the shared slot: routing by table (two models
# on one table, a model on two), models added, redefined and dropped while
# a writer streams, and a failing model that holds no one back. Section 20
# stops the server with -m fast, idle and mid-batch. Section 21 applies
# models in parallel (B1): a light model's change is visible beside four
# slow ones; a kill -9 and a fast stop mid-round lose nothing. Needs: cargo-pgrx installed and pg_improv installed into the PG
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

# 4. INSERT of a new item and DELETE: the worker mints the item and applies
#    the row without a rebuild (no formula enumerates Product's items)
logpos=$(wc -c <"$D/log")
q "INSERT INTO sales VALUES ('Q1','Gizmo',5,10)"
wait_for "SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Gizmo'" 50 "insert Gizmo" >/dev/null
wait_for "SELECT value FROM improv_plan.total WHERE product='Gizmo'" 50 "total Gizmo" >/dev/null
q "DELETE FROM sales WHERE product='Gizmo'"
wait_for "SELECT count(*) FROM improv_plan.revenue WHERE product='Gizmo'" 0 "delete Gizmo" >/dev/null
[ "$(q "SELECT count(*) FROM improv.item i JOIN improv.model m ON m.id = i.model_id WHERE m.name = 'plan' AND i.name = 'Gizmo'")" = 1 ] \
	|| { echo "FAIL: Gizmo was not minted once"; exit 1; }
if tail -c +$((logpos + 1)) "$D/log" | grep -q "model plan: rebuilding"; then
	echo "FAIL: a new item rebuilt the model"; exit 1
fi
echo "ok: INSERT (new item, minted without a rebuild) and DELETE propagated"

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

# 7. a structural change rebuilds the live state, even with no data change:
#    a measure defined after the worker caught up is computed on its own
#    (a person typing define() then SELECT must not see an empty table)
q "SELECT improv.define('plan','Double = Revenue * 2')"
want=$(( $(q "$REV") * 2 ))
wait_for "SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'" "$want" "defined, no data change" >/dev/null
q "UPDATE sales SET price = 7 WHERE quarter='Q1' AND product='Widget'"
wait_for "SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'" 2800 "new measure" >/dev/null
echo "ok: a measure defined while live is computed at once ($want), then follows changes (2800)"

# 7b. a dimensionless measure (P3-2): a one-row table of just `value`,
#     applied by the live engine like any other result
q "SELECT improv.define('plan','MaxRev = MAX(Revenue OVER Time, Product)')"
want=$(q "SELECT max(price * qty) FROM sales")
wait_for "SELECT value FROM improv_plan.maxrev" "$want" "dimensionless, defined" >/dev/null
q "UPDATE sales SET qty = 10000 WHERE quarter='Q3' AND product='Gadget'"
wait_for "SELECT value FROM improv_plan.maxrev" 200000 "dimensionless, updated" >/dev/null
[ "$(q "SELECT count(*) FROM improv_plan.maxrev")" = 1 ] || { echo "FAIL: maxrev has more than one row"; exit 1; }
q "UPDATE sales SET qty = 90 WHERE quarter='Q3' AND product='Gadget'"
echo "ok: a dimensionless measure keeps one row and follows changes"

# 7c. redefine (P1-1): the same formula is a no-op; a new one is applied in
#     the definer's transaction, and the worker's rebuild rewrites nothing
ver() { q "SELECT tx FROM improv.model_version v JOIN improv.model m ON m.id = v.model_id WHERE m.name = 'plan'"; }
dx() { q "SELECT max(xmin::text::bigint) FROM improv_plan.double"; }
v0=$(ver)
q "SELECT improv.define('plan','Double = Revenue * 2', replace => true)" >/dev/null
[ "$(ver)" = "$v0" ] || { echo "FAIL: an unchanged redefine bumped model_version"; exit 1; }
logpos=$(wc -c <"$D/log")
got=$(q "SELECT improv.define('plan','Double = Revenue * 3', replace => true);
         SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'" | tail -1)
[ "$got" = 4200 ] || { echo "FAIL: redefine not applied in its transaction: $got"; exit 1; }
x0=$(dx)
for _ in $(seq 400); do tail -c +$((logpos + 1)) "$D/log" | grep -q "model plan: rebuilding (model changed)" && break; sleep 0.025; done
tail -c +$((logpos + 1)) "$D/log" | grep -q "model plan: rebuilding (model changed)" || { echo "FAIL: the worker did not see the redefine"; exit 1; }
q "UPDATE sales SET price = 8 WHERE quarter='Q1' AND product='Widget'"
wait_for "SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'" 4800 "redefined, then a write" >/dev/null
n=$(q "SELECT count(*) FROM improv_plan.double WHERE xmin::text::bigint > $x0")
[ "$n" = 1 ] || { echo "FAIL: after a redefine the worker rewrote $n double rows (want 1, the write)"; exit 1; }
q "SELECT improv.define('plan','Double = Revenue * 2', replace => true)" >/dev/null
echo "ok: redefine is a no-op when unchanged, applied in its transaction, and the worker rewrites nothing"

# 7d. a named LAMBDA (P3-3) with a category parameter, called by two
#     measures; redefining it rewrites both in the definer's transaction,
#     and the worker's rebuild rewrites nothing
q "SELECT improv.define('plan','Share = LAMBDA(x, c, x / MAX(x OVER c))')" >/dev/null
q "SELECT improv.define('plan','RevShare = Share(Revenue, Time)')" >/dev/null
q "SELECT improv.define('plan','QtyShare = Share(Quantity, Product)')" >/dev/null
wait_for "SELECT count(*) FROM improv_plan.qtyshare WHERE value = 1" 4 "lambda callers computed" >/dev/null
sx() { q "SELECT max(xmin::text::bigint) FROM (SELECT xmin FROM improv_plan.revshare UNION ALL SELECT xmin FROM improv_plan.qtyshare) t"; }
logpos=$(wc -c <"$D/log")
got=$(q "SELECT improv.define('plan','Share = LAMBDA(x, c, 100 * x / MAX(x OVER c))', replace => true);
         SELECT count(*) FROM improv_plan.qtyshare WHERE value = 100" | tail -1)
[ "$got" = 4 ] || { echo "FAIL: lambda redefine not applied in its transaction: $got"; exit 1; }
x0=$(sx)
for _ in $(seq 400); do tail -c +$((logpos + 1)) "$D/log" | grep -q "model plan: rebuilding (model changed)" && break; sleep 0.025; done
tail -c +$((logpos + 1)) "$D/log" | grep -q "model plan: rebuilding (model changed)" || { echo "FAIL: the worker did not see the lambda redefine"; exit 1; }
sleep 0.5
n=$(q "SELECT count(*) FROM (SELECT xmin FROM improv_plan.revshare UNION ALL SELECT xmin FROM improv_plan.qtyshare) t WHERE xmin::text::bigint > $x0")
[ "$n" = 0 ] || { echo "FAIL: after a lambda redefine the worker rewrote $n rows"; exit 1; }
for m in RevShare QtyShare; do q "SELECT improv.drop_measure('plan','$m')"; done
q "SELECT improv.drop_measure('plan','Share')"
echo "ok: a named LAMBDA's callers are rewritten in the redefiner's transaction; the worker rewrites nothing"

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

# 9. TRUNCATE: every result row goes, without a rebuild
logpos=$(wc -c <"$D/log")
q "TRUNCATE wide"
wait_for "SELECT count(*) FROM improv_wide.revenue" 0 "truncate" >/dev/null
if tail -c +$((logpos + 1)) "$D/log" | grep -q "model wide: rebuilding"; then
	echo "FAIL: TRUNCATE rebuilt the model"; exit 1
fi
echo "ok: TRUNCATE emptied the results (no rebuild)"

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

# 10b. every aggregate, maintained incrementally: random INSERT, UPDATE
#      (moving rows between coordinates) and DELETE batches, then each
#      result equals a fresh SQL aggregate, with no rebuild. Rows share
#      keys (sha) within a coordinate, so count_distinct sees duplicates.
NP=${FOLD_PERSONS:-5} NY=${FOLD_YEARS:-3}
q "CREATE TABLE facts (id int PRIMARY KEY, person text, year int, sha text, n numeric)"
q "INSERT INTO facts SELECT g, 'p'||(g%5), 2020+g%3, 's'||(g%40), g%7 FROM generate_series(1,300) g"
q "SELECT improv.create_model('folds')"
q "SELECT improv.bind_category('folds','Person','facts','person')"
q "SELECT improv.bind_category('folds','Year','facts','year')"
AGGS="sum avg min max count count_distinct"
for a in $AGGS; do
	col=n; [ "$a" = count_distinct ] && col=sha
	q "SELECT improv.bind_measure('folds','F_$a','facts','$col', agg => '$a')"
	q "SELECT improv.define('folds','R_$a = F_$a * 1')"
done
# a marker write, visible once the worker has applied everything above
q "INSERT INTO facts VALUES (0, 'p0', 2020, 'warm', 100)"
wait_for "SELECT value FROM improv_folds.r_max WHERE person='p0' AND year='2020'" 100 "folds catch-up" >/dev/null
logpos=$(wc -c <"$D/log")
counters() { q "SELECT incremental_applies || ' ' || rebuilds FROM improv.status('folds')"; }
read -r inc0 reb0 <<<"$(counters)"
for i in $(seq 1 20); do
	# one more person and year than the load saw: new items, minted live
	q "INSERT INTO facts SELECT 1000*$i + g, 'p'||(g%($NP+1)), 2020+g%($NY+1), 'n'||(g%3), g%9 FROM generate_series(1, 5) g;
	   UPDATE facts SET person = 'p'||((id + $i) % $NP), n = n + 1
	     WHERE id IN (SELECT id FROM facts ORDER BY md5(id||'u'||$i) LIMIT 3);
	   DELETE FROM facts WHERE id IN (SELECT id FROM facts ORDER BY md5(id||'d'||$i) LIMIT 4)"
done
# rows where the result differs from a fresh SQL aggregate (or one lacks a row)
mismatch() { # agg
	local e="$1(n)"
	[ "$1" = count_distinct ] && e="count(DISTINCT sha)"
	echo "SELECT count(*) FROM (SELECT person, year::text AS year, ($e)::float8 AS v
	        FROM facts GROUP BY 1, 2) w
	      FULL JOIN improv_folds.r_$1 r USING (person, year) WHERE w.v IS DISTINCT FROM r.value"
}
for a in $AGGS; do
	wait_for "$(mismatch $a)" 0 "agg $a equals a fresh aggregate" >/dev/null
done
if tail -c +$((logpos + 1)) "$D/log" | grep -q "model folds: rebuilding"; then
	echo "FAIL: an aggregated batch rebuilt the model:"
	tail -c +$((logpos + 1)) "$D/log" | grep "model folds: rebuilding"; exit 1
fi
read -r inc1 reb1 <<<"$(counters)"
[ "$reb1" = "$reb0" ] && [ "$inc1" -gt "$inc0" ] \
	|| { echo "FAIL: improv.status counters went $inc0/$reb0 -> $inc1/$reb1 (want more applies, no rebuild)"; exit 1; }
echo "   status: incremental_applies $inc0 -> $inc1, rebuilds $reb0 -> $reb1"
[ "$(q "SELECT count(*) FROM improv_folds.r_count WHERE person = 'p$NP' OR year = '$((2020 + NY))'")" -gt 0 ] \
	|| { echo "FAIL: the batches brought no new item"; exit 1; }
echo "ok: $AGGS follow 20 random INSERT/UPDATE/DELETE batches (new items too) with no rebuild"

# 10c. a formula that enumerates a category's items (POSITION) rebuilds when
#      the category gains one, and is right afterwards
q "CREATE TABLE ranked (product text PRIMARY KEY, v float8)"
q "INSERT INTO ranked VALUES ('A', 1), ('C', 3)"
q "SELECT improv.create_model('pos')"
q "SELECT improv.bind_category('pos','Product','ranked','product')"
q "SELECT improv.bind_measure('pos','V','ranked','v')"
q "SELECT improv.define('pos','Pos = POSITION(Product) + 0 * V')"
q "UPDATE ranked SET v = 2 WHERE product = 'A'" # warm the live state
wait_for "SELECT value FROM improv_pos.pos WHERE product='C'" 2 "pos catch-up" >/dev/null
logpos=$(wc -c <"$D/log")
q "INSERT INTO ranked VALUES ('B', 2)"
wait_for "SELECT value FROM improv_pos.pos WHERE product='B'" 3 "pos of a new item" >/dev/null
tail -c +$((logpos + 1)) "$D/log" | grep -q "model pos: rebuilding (new item in a category a formula enumerates)" \
	|| { echo "FAIL: a new item under POSITION did not rebuild"; exit 1; }
echo "ok: a new item in a category POSITION enumerates rebuilds (B is item 3)"

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

# 12. text, bool and date inputs apply cell by cell: an UPDATE rewrites one
#     row of each result and does not rebuild
q "CREATE TABLE tags (product text PRIMARY KEY, kind text, active bool, due date)"
q "INSERT INTO tags SELECT 'P'||p, 'a', true, date '2024-01-01' FROM generate_series(1,3) p"
q "SELECT improv.create_model('tagged')"
q "SELECT improv.bind_category('tagged','Product','tags','product')"
q "SELECT improv.bind_measure('tagged','Kind','tags','kind')"
q "SELECT improv.bind_measure('tagged','Active','tags','active')"
q "SELECT improv.bind_measure('tagged','Due','tags','due')"
q "SELECT improv.define('tagged','Label = Kind')"
q "SELECT improv.define('tagged','Live = Active')"
q "SELECT improv.define('tagged','Deadline = Due')"
TL="SELECT value FROM improv_tagged.label WHERE product='P2'"
q "UPDATE tags SET kind = 'b' WHERE product = 'P1'" # warm the live state
wait_for "SELECT value FROM improv_tagged.label WHERE product='P1'" b "typed warm-up" >/dev/null
wait_for "SELECT count(*) FROM improv_tagged.deadline" 3 "typed catch-up" >/dev/null
txm() { q "SELECT max(xmin::text::bigint) FROM improv_tagged.$1"; }
lx=$(txm label) vx=$(txm live) dx=$(txm deadline) logpos=$(wc -c <"$D/log")
q "UPDATE tags SET kind = 'z', active = false, due = date '2024-02-29' WHERE product = 'P2'"
wait_for "$TL" z "text update" >/dev/null
wait_for "SELECT value FROM improv_tagged.live WHERE product='P2'" f "bool update" >/dev/null
wait_for "SELECT value::date FROM improv_tagged.deadline WHERE product='P2'" 2024-02-29 "date update" >/dev/null
n=$(q "SELECT (SELECT count(*) FROM improv_tagged.label WHERE xmin::text::bigint > $lx) || ' ' ||
              (SELECT count(*) FROM improv_tagged.live WHERE xmin::text::bigint > $vx) || ' ' ||
              (SELECT count(*) FROM improv_tagged.deadline WHERE xmin::text::bigint > $dx)")
[ "$n" = "1 1 1" ] || { echo "FAIL: typed update rewrote $n label/live/deadline rows"; exit 1; }
if tail -c +$((logpos + 1)) "$D/log" | grep -q "model tagged: rebuilding"; then
	echo "FAIL: a typed update rebuilt the model"; exit 1
fi
echo "ok: text/bool/date UPDATE applied cell by cell (1 row per result, no rebuild)"
# 13. one shared slot (0.11.0): routing by relation, and per-model state.
#     m_a and m_b both bind ev (two models on one table); m_c binds ev and
#     ev2 (a model on two tables). Then a writer streams into ev while a
#     model is added, one is rebuilt, one is dropped, and one fails.
q "CREATE TABLE ev (id int PRIMARY KEY, k text, n float8)"
q "CREATE TABLE ev2 (k text PRIMARY KEY, w float8)"
q "INSERT INTO ev SELECT g, 'k' || g % 3, 1 FROM generate_series(1, 30) g"
q "INSERT INTO ev2 VALUES ('k0', 10), ('k1', 20), ('k2', 30)"
mk() { # model on ev: T = sum(n) per k
	q "SELECT improv.create_model('$1')" >/dev/null
	q "SELECT improv.bind_category('$1','K','ev','k')" >/dev/null
	q "SELECT improv.bind_measure('$1','N','ev','n', agg => 'sum')" >/dev/null
	q "SELECT improv.define('$1','T = N * 1')" >/dev/null
}
mk m_a; mk m_b; mk m_c
q "SELECT improv.bind_category('m_c','K','ev2','k')" >/dev/null
q "SELECT improv.bind_measure('m_c','W','ev2','w')" >/dev/null
q "SELECT improv.define('m_c','TW = N * W')" >/dev/null
eq13() { [ "$1" = "$2" ] || { echo "FAIL: $3: got '$1', want '$2'"; tail -20 "$D/log"; exit 1; }; }
SUMK="SELECT string_agg(k || '=' || s, ',' ORDER BY k) FROM (SELECT k, sum(n) s FROM ev GROUP BY k) x"
T="string_agg(k || '=' || value, ',' ORDER BY k)"
for m in m_a m_b m_c; do wait_for "SELECT $T FROM improv_$m.t" "$(q "$SUMK")" "$m caught up" >/dev/null; done
wait_for "SELECT count(*) FROM improv_m_c.tw" 3 "m_c's TW" >/dev/null
eq13 "$(q "SELECT count(*) FROM pg_replication_slots")" 1 "one slot for every model"
eq13 "$(q "SELECT string_agg(DISTINCT slot, ',') FROM improv.sync_state")" improv_db "every model on the shared slot"
eq13 "$(q "SELECT count(*) FROM pg_publication_tables WHERE pubname = 'improv_db' AND tablename IN ('ev', 'ev2')")" 2 \
	"the publication holds each bound table once"
# routing: a write to ev2 reaches m_c only (m_a and m_b apply nothing)
cnt() { q "SELECT incremental_applies + rebuilds FROM improv.status('$1')"; }
a0=$(cnt m_a) b0=$(cnt m_b)
q "UPDATE ev2 SET w = 11 WHERE k = 'k0'"
wait_for "SELECT value FROM improv_m_c.tw WHERE k = 'k0'" 110 "m_c follows ev2" >/dev/null
eq13 "$(cnt m_a) $(cnt m_b)" "$a0 $b0" "a write to ev2 does not reach m_a or m_b"
# a writer: one row per 20 ms into ev, until told to stop
( i=1000; while [ ! -f "$D/stop13" ]; do q "INSERT INTO ev VALUES ($i, 'k' || $i % 3, 1)" 2>/dev/null; i=$((i + 1)); sleep 0.02; done ) &
writer=$!
sleep 0.5
mk m_d                                                        # added mid-stream
q "SELECT improv.define('m_b','T = N * 1 + 0', replace => true)" >/dev/null  # rebuilt
q "SELECT improv.drop_model('m_a')"                           # dropped
# a failing model: its result table refuses the worker's writes
q "CREATE TABLE evx (id int PRIMARY KEY, k text, n float8)"
q "INSERT INTO evx SELECT g, 'x' || g, 1 FROM generate_series(1, 5) g"
q "SELECT improv.create_model('m_e')" >/dev/null
q "SELECT improv.set_sync_mode('m_e','manual')" >/dev/null
q "SELECT improv.bind_category('m_e','K','evx','k')" >/dev/null
q "SELECT improv.bind_measure('m_e','N','evx','n')" >/dev/null
q "SELECT improv.define('m_e','T = N * 1')" >/dev/null
q "ALTER TABLE improv_m_e.t ADD CONSTRAINT refuse CHECK (value < 0) NOT VALID"
q "SELECT improv.set_sync_mode('m_e','logical')" >/dev/null
wait_for "SELECT last_error IS NOT NULL FROM improv.status('m_e')" t "m_e fails" >/dev/null
echo "   m_e: $(q "SELECT left(last_error, 120) FROM improv.status('m_e')")"
sleep 1
touch "$D/stop13"; wait "$writer"
want=$(q "$SUMK")
for m in m_b m_c m_d; do
	wait_for "SELECT $T FROM improv_$m.t" "$want" "$m exact after streaming past the failing model" >/dev/null
done
# a marker routed to m_c only: once it shows, the slot is past it, though m_e still fails
mark=$(q "SELECT pg_current_wal_insert_lsn()")
q "UPDATE ev2 SET w = 12 WHERE k = 'k0'"
wait_for "SELECT value FROM improv_m_c.tw WHERE k = 'k0'" "$(q "SELECT sum(n) * 12 FROM ev WHERE k = 'k0'")" "m_c marker" >/dev/null
wait_for "SELECT confirmed_flush_lsn > '$mark' FROM pg_replication_slots WHERE slot_name = 'improv_db'" t \
	"the failing model does not hold the slot back" >/dev/null
eq13 "$(q "SELECT last_error IS NOT NULL FROM improv.status('m_e')")" t "m_e still failing"
eq13 "$(q "SELECT count(*) FROM improv.model WHERE name = 'm_a'")" 0 "m_a dropped"
eq13 "$(q "SELECT count(*) FROM pg_replication_slots")" 1 "still one slot"
q "ALTER TABLE improv_m_e.t DROP CONSTRAINT refuse"
wait_for "SELECT count(*) FROM improv_m_e.t" 5 "m_e recovers (retried with backoff)" >/dev/null
wait_for "SELECT last_error IS NULL FROM improv.status('m_e')" t "m_e's error clears" >/dev/null
q "INSERT INTO evx VALUES (6, 'x6', 1)"
wait_for "SELECT count(*) FROM improv_m_e.t" 6 "m_e follows writes again" >/dev/null
echo "ok: one slot for every model; routing by table; models added, rebuilt and dropped mid-stream stay exact; a failing model holds no one back and recovers"

# 14. an invalidated slot (D11): with the worker away (improv.database points
#     elsewhere), the slot falls past max_slot_wal_keep_size and is lost, and
#     status says so. Back, the worker drops it, makes a new one, logs that
#     once, and rebuilds every model: the results equal a fresh recalc.
restart() { "$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w restart -m fast >/dev/null 3>&-; }
eq14() { [ "$1" = "$2" ] || { echo "FAIL: $3: got '$1', want '$2'"; tail -20 "$D/log"; exit 1; }; }
q "ALTER SYSTEM SET improv.database = 'postgres'"
q "ALTER SYSTEM SET max_slot_wal_keep_size = '1MB'"
restart
q "UPDATE sales SET price = 9 WHERE quarter='Q1' AND product='Widget'" # while the worker is away
q "CREATE TABLE ballast (b text)"
SLOTST="SELECT wal_status FROM pg_replication_slots WHERE slot_name = 'improv_db'"
for _ in $(seq 20); do
	[ "$(q "$SLOTST")" = lost ] && break
	q "INSERT INTO ballast SELECT repeat('x', 1000) FROM generate_series(1, 4000)"
	q "SELECT pg_switch_wal()" >/dev/null
	q "CHECKPOINT"
done
eq14 "$(q "$SLOTST")" lost "the slot is lost"
eq14 "$(q "SELECT last_error LIKE '%invalidated%' FROM improv.status('plan')")" t "status reports the lost slot"
logpos=$(wc -c <"$D/log")
q "ALTER SYSTEM RESET improv.database"
q "ALTER SYSTEM RESET max_slot_wal_keep_size"
restart
SYNCED="SELECT bool_and(last_applied_at IS NOT NULL AND last_error IS NULL) FROM improv.sync_state"
wait_for "$SYNCED" t "every model rebuilt after the lost slot" >/dev/null
wait_for "$REV" "$(q "SELECT price * qty FROM sales WHERE quarter='Q1' AND product='Widget'")" "the write made while away" >/dev/null
eq14 "$(q "$SLOTST")" reserved "a new slot"
eq14 "$(q "SELECT sum(improv.recalc(name)) FROM improv.model WHERE sync_mode = 'logical'")" 0 \
	"every model equals a fresh recalc"
q "UPDATE sales SET price = 10 WHERE quarter='Q1' AND product='Widget'"
wait_for "$REV" "$(q "SELECT price * qty FROM sales WHERE quarter='Q1' AND product='Widget'")" "a write after the new slot" >/dev/null
eq14 "$(tail -c +$((logpos + 1)) "$D/log" | grep -c "slot improv_db was invalidated")" 1 "the invalidation logged once"
eq14 "$(tail -c +$((logpos + 1)) "$D/log" | grep -c "can no longer")" 0 "no peek of the invalidated slot"
echo "ok: a lost slot is reported, dropped, made again and logged once; every model rebuilds and equals a fresh recalc"

# 14b. improv.max_slot_lag (D7): a slot that falls that far behind (here a
#      slow worker: poll_ms = 60000) is dropped and remade, and every model
#      rebuilds, rather than retain more WAL
q "ALTER SYSTEM SET improv.max_slot_lag = '2MB'"
q "ALTER SYSTEM SET improv.poll_ms = 60000"
q "SELECT pg_reload_conf()" >/dev/null
sleep 1
logpos=$(wc -c <"$D/log")
q "UPDATE sales SET price = 14 WHERE quarter='Q1' AND product='Widget'"
q "INSERT INTO ballast SELECT repeat('y', 1000) FROM generate_series(1, 4000)"
q "ALTER SYSTEM RESET improv.poll_ms"
q "SELECT pg_reload_conf()" >/dev/null
wait_for "$REV" "$(q "SELECT price * qty FROM sales WHERE quarter='Q1' AND product='Widget'")" "the write past max_slot_lag" >/dev/null
wait_for "$SYNCED" t "every model rebuilt after max_slot_lag" >/dev/null
eq14 "$(tail -c +$((logpos + 1)) "$D/log" | grep -c "slot improv_db was invalidated (improv.max_slot_lag exceeded")" 1 \
	"the max_slot_lag drop logged once"
eq14 "$(q "SELECT sum(improv.recalc(name)) FROM improv.model WHERE sync_mode = 'logical'")" 0 \
	"every model equals a fresh recalc after max_slot_lag"
q "ALTER SYSTEM RESET improv.max_slot_lag"
q "SELECT pg_reload_conf()" >/dev/null
echo "ok: past improv.max_slot_lag the slot is dropped and remade, and every model rebuilds"

# 14c. max_replication_slots exhausted (D7): the worker cannot make its slot.
#      status shows why, the worker logs it once and keeps running (no crash
#      loop), and once a slot is free it makes one and every model rebuilds
q "ALTER SYSTEM SET improv.database = 'postgres'"
restart
q "SELECT pg_drop_replication_slot('improv_db')"
q "ALTER SYSTEM SET max_replication_slots = 1"
restart
q "SELECT 1 FROM pg_create_physical_replication_slot('hog')" >/dev/null
q "UPDATE sales SET price = 15 WHERE quarter='Q1' AND product='Widget'"
logpos=$(wc -c <"$D/log")
q "ALTER SYSTEM RESET improv.database"
restart
wait_for "SELECT coalesce(last_error, '') LIKE '%replication slots are in use%' FROM improv.status('plan')" t \
	"status reports the exhausted slots" >/dev/null
pid=$(q "SELECT pid FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'")
sleep 1
eq14 "$(q "SELECT pid FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'")" "$pid" "the worker keeps running"
eq14 "$(tail -c +$((logpos + 1)) "$D/log" | grep -c "pg_improv sync: creating the slot improv_db")" 1 "the failure logged once"
q "SELECT pg_drop_replication_slot('hog')"
wait_for "$SYNCED" t "every model rebuilt once a slot is free" >/dev/null
wait_for "$REV" "$(q "SELECT price * qty FROM sales WHERE quarter='Q1' AND product='Widget'")" "the write made while no slot was free" >/dev/null
q "ALTER SYSTEM RESET max_replication_slots"
restart
echo "ok: max_replication_slots exhausted: status shows it, logged once, no crash loop; a free slot recovers every model"

# 15. an idle worker is quiet (D8): at poll_ms = 50 an idle window (60 s by
#     default) adds no "logical decoding" line to the server log, as every
#     decode used to; writes after it are still applied, and log none either
IDLE_S=${IDLE_S:-60}
REVQ="SELECT price * qty FROM sales WHERE quarter='Q1' AND product='Widget'"
q "UPDATE sales SET price = 13 WHERE quarter='Q1' AND product='Widget'"
wait_for "$REV" "$(q "$REVQ")" "a write before the idle window" >/dev/null
# autovacuum off for the window: an ANALYZE writes WAL of its own
q "ALTER SYSTEM SET autovacuum = off"
q "SELECT pg_reload_conf()" >/dev/null
sleep 1
logpos=$(wc -c <"$D/log")
wal0=$(q "SELECT pg_current_wal_insert_lsn()")
sleep "$IDLE_S"
decoding() { tail -c +$((logpos + 1)) "$D/log" | grep -c "logical decoding" || true; }
eq14 "$(decoding)" 0 "decoding lines logged over ${IDLE_S}s idle"
# an idle worker writes no WAL (it wrote two commit records a poll, which it
# then decoded); a bgwriter snapshot or a timed checkpoint is under 4 kB
walmoved=$(q "SELECT pg_current_wal_insert_lsn() - '$wal0'")
[ "$walmoved" -lt 4096 ] || { echo "FAIL: ${IDLE_S}s idle wrote $walmoved bytes of WAL"; exit 1; }
q "ALTER SYSTEM RESET autovacuum"
q "SELECT pg_reload_conf()" >/dev/null
for i in $(seq 1 10); do
	q "UPDATE sales SET price = $((20 + i)) WHERE quarter='Q1' AND product='Widget'"
	wait_for "$REV" "$(q "$REVQ")" "write $i after the idle window" >/dev/null
done
eq14 "$(decoding)" 0 "decoding lines logged by 10 applied writes"
echo "ok: ${IDLE_S}s idle at poll_ms 50 wrote $walmoved bytes of WAL and logged no decoding line; 10 later writes applied, quietly"

# 16. a configuration reload reaches the worker (D9): ALTER SYSTEM SET
#     improv.poll_ms then pg_reload_conf() changes the poll interval at once
logpos=$(wc -c <"$D/log")
q "ALTER SYSTEM SET improv.poll_ms = 60000"
q "SELECT pg_reload_conf()" >/dev/null
for _ in $(seq 200); do
	tail -c +$((logpos + 1)) "$D/log" | grep -q "reloaded the configuration (poll_ms = 60000" && break
	sleep 0.025
done
tail -c +$((logpos + 1)) "$D/log" | grep -q "reloaded the configuration (poll_ms = 60000" \
	|| { echo "FAIL: the worker did not reload its configuration"; tail -20 "$D/log"; exit 1; }
sleep 0.5 # let the poll the reload woke finish
before=$(q "$REV")
q "UPDATE sales SET price = 41 WHERE quarter='Q1' AND product='Widget'"
sleep 2
eq14 "$(q "$REV")" "$before" "at poll_ms 60000 a write waits for the next poll"
q "ALTER SYSTEM RESET improv.poll_ms"
q "SELECT pg_reload_conf()" >/dev/null
wait_for "$REV" "$(q "$REVQ")" "the reload back to poll_ms 50" >/dev/null
echo "ok: pg_reload_conf() changed improv.poll_ms without a restart (60000, then back to 50)"

# 17. a database the worker does not serve (D12): logical mode is refused
#     there, with a hint, and status reports no worker rather than reading
#     the served database's slot of the same name
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv improv_other
q2() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d improv_other -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
q2 "CREATE EXTENSION pg_improv"
q2 "CREATE TABLE sales (quarter text, product text, price float8)"
q2 "INSERT INTO sales VALUES ('Q1', 'Widget', 10)"
q2 "SELECT improv.create_model('o')" >/dev/null
q2 "SELECT improv.bind_category('o','Time','sales','quarter')"
err=$(q2 "SELECT improv.bind_measure('o','Price','sales','price')" 2>&1) && { echo "FAIL: bind_measure of a logical model outside improv.database"; exit 1; }
case $err in *"serves database \"improv_e2e\""*HINT*improv.database*) ;; *) echo "FAIL: bind_measure outside improv.database: $err"; exit 1 ;; esac
q2 "SELECT improv.set_sync_mode('o','trigger')"
q2 "SELECT improv.bind_measure('o','Price','sales','price')"
q2 "SELECT improv.define('o','Double = Price * 2')"
q2 "SELECT improv.recalc('o')" >/dev/null
eq14 "$(q2 "SELECT value FROM improv_o.double")" 20 "trigger mode outside improv.database"
err=$(q2 "SELECT improv.set_sync_mode('o','logical')" 2>&1) && { echo "FAIL: logical mode outside improv.database"; exit 1; }
case $err in *HINT*improv.database*) ;; *) echo "FAIL: set_sync_mode outside improv.database: $err"; exit 1 ;; esac
# a model left logical by a restored dump: its sync state names improv_db,
# the slot of the served database
q2 "UPDATE improv.model SET sync_mode = 'logical' WHERE name = 'o'"
q2 "INSERT INTO improv.sync_state (model_id, slot, publication) SELECT id, 'improv_db', 'improv_db' FROM improv.model WHERE name = 'o'"
eq14 "$(q2 "SELECT (lag_bytes IS NULL) || ' ' || last_error FROM improv.status('o')")" \
	"true no sync worker serves this database (improv.database = improv_e2e)" "status outside improv.database"
eq14 "$(q "SELECT last_error IS NULL FROM improv.status('plan')")" t "status in the served database"
"$PG_BIN/dropdb" -h "$D" -p "$P" -U improv improv_other
echo "ok: outside improv.database logical mode is refused with a hint, and status reports that no worker serves it"

# 18. DROP EXTENSION ... CASCADE with logical models (D13): the worker drops
#     the slot and publication it left, which would retain WAL forever, and
#     logs that once; the result schemas (user data) stay
logpos=$(wc -c <"$D/log")
q "DROP EXTENSION pg_improv CASCADE" 2>/dev/null
wait_for "SELECT count(*) FROM pg_replication_slots" 0 "the orphaned slot dropped" >/dev/null
wait_for "SELECT count(*) FROM pg_publication" 0 "the orphaned publication dropped" >/dev/null
sleep 0.5
eq14 "$(tail -c +$((logpos + 1)) "$D/log" | grep -c "pg_improv is not installed in this database: dropped the slot and publication improv_db")" 1 \
	"the orphan drop logged once"
eq14 "$(q "SELECT count(*) FROM pg_namespace WHERE nspname = 'improv_plan'")" 1 "the result schema kept"
q "CREATE EXTENSION pg_improv"
eq14 "$(q "SELECT count(*) FROM pg_replication_slots")" 0 "no slot without a logical model"
echo "ok: DROP EXTENSION CASCADE: the worker dropped the orphaned slot and publication (logged once); results kept"

# 19. partitioned tables (D3): a range-partitioned and a list-partitioned
#     bound table follow INSERT, UPDATE (across partitions too) and DELETE,
#     and attaching, detaching and truncating a partition while a writer
#     streams; each result then equals a fresh SQL aggregate and recalc
logpos19=$(wc -c <"$D/log")
q "CREATE TABLE pr (id int, d date, k text, n float8) PARTITION BY RANGE (d)"
q "CREATE TABLE pr_2024 PARTITION OF pr FOR VALUES FROM ('2024-01-01') TO ('2025-01-01')"
q "CREATE TABLE pr_2025 PARTITION OF pr FOR VALUES FROM ('2025-01-01') TO ('2026-01-01')"
q "CREATE TABLE pl (id int, region text, k text, n float8, PRIMARY KEY (region, id)) PARTITION BY LIST (region)"
q "CREATE TABLE pl_east PARTITION OF pl FOR VALUES IN ('east')"
q "CREATE TABLE pl_west PARTITION OF pl FOR VALUES IN ('west')"
q "INSERT INTO pr SELECT g, date '2024-01-01' + g * 3, 'k' || g % 3, g FROM generate_series(1, 200) g"
q "INSERT INTO pl SELECT g, CASE WHEN g % 2 = 0 THEN 'east' ELSE 'west' END, 'k' || g % 3, g FROM generate_series(1, 100) g"
for t in pr pl; do
	q "SELECT improv.create_model('m_$t')" >/dev/null
	q "SELECT improv.bind_category('m_$t','K','$t','k')" >/dev/null
	q "SELECT improv.bind_measure('m_$t','N','$t','n', agg => 'sum')" >/dev/null
	q "SELECT improv.define('m_$t','T = N * 1')" >/dev/null
done
eq14 "$(q "SELECT pubviaroot FROM pg_publication WHERE pubname = 'improv_db'")" t "publish_via_partition_root"
eq14 "$(q "SELECT string_agg(relname || '=' || relreplident::text, ',' ORDER BY relname) FROM pg_class WHERE relname ~ '^pr(_|$)'")" \
	"pr=f,pr_2024=f,pr_2025=f" "replica identity FULL on the root and each partition"
psum() { echo "SELECT coalesce(string_agg(k || '=' || s, ',' ORDER BY k), '') FROM (SELECT k, sum(n) s FROM $1 GROUP BY k) x"; }
pres() { echo "SELECT coalesce(string_agg(k || '=' || value, ',' ORDER BY k), '') FROM improv_m_$1.t"; }
peq() { for t in pr pl; do wait_for "$(pres $t)" "$(q "$(psum $t)")" "m_$t $1" >/dev/null; done; }
peq "caught up"
q "INSERT INTO pr VALUES (1001, '2025-06-01', 'k0', 1000)"
q "UPDATE pr SET n = n + 1 WHERE id % 10 = 0"
q "UPDATE pr SET d = '2025-02-02' WHERE id = 5" # moves a row from pr_2024 to pr_2025
q "DELETE FROM pr WHERE id = 7"
q "INSERT INTO pl VALUES (1001, 'east', 'k1', 500)"
q "UPDATE pl SET region = 'east' WHERE id = 3" # moves a row from pl_west to pl_east
q "DELETE FROM pl WHERE id = 4"
peq "after row changes"
# a writer into both, while partitions come and go (stopped before each
# comparison: the expected sums are read once)
writer19() { # first id
	rm -f "$D/stop19"
	( i=$1; while [ ! -f "$D/stop19" ]; do
		q "INSERT INTO pr VALUES ($i, date '2025-01-01' + $i % 300, 'k' || $i % 3, 1); INSERT INTO pl VALUES ($i, 'east', 'k' || $i % 3, 1)" 2>/dev/null
		i=$((i + 1)); sleep 0.02; done ) &
	writer=$!
	sleep 0.5
}
stop19() { sleep 0.5; touch "$D/stop19"; wait "$writer"; }
writer19 2000
q "CREATE TABLE pr_2026 (LIKE pr)"
q "INSERT INTO pr_2026 SELECT g, date '2026-01-01' + g, 'k' || g % 3, 100 FROM generate_series(1, 20) g"
q "ALTER TABLE pr ATTACH PARTITION pr_2026 FOR VALUES FROM ('2026-01-01') TO ('2027-01-01')"
q "CREATE TABLE pl_north (LIKE pl INCLUDING ALL)"
q "INSERT INTO pl_north SELECT g, 'north', 'k' || g % 3, 100 FROM generate_series(1, 10) g"
q "ALTER TABLE pl ATTACH PARTITION pl_north FOR VALUES IN ('north')"
stop19
peq "after attaching a partition"
writer19 5000
# the worker gave the attached PK-less partition REPLICA IDENTITY FULL, so
# an UPDATE through the root reaches it (Postgres refuses one otherwise)
q "UPDATE pr SET n = n + 1 WHERE d >= '2026-01-01' AND id <= 5"
q "INSERT INTO pr VALUES (3001, '2026-06-01', 'k2', 7)"
q "UPDATE pl SET n = n + 1 WHERE region = 'north'"
q "ALTER TABLE pr DETACH PARTITION pr_2024"
q "ALTER TABLE pl DETACH PARTITION pl_west"
sleep 0.5
q "TRUNCATE pl_north"
stop19
peq "after detaching and truncating a partition"
q "UPDATE pr_2024 SET n = 0" # detached: no longer the model's
q "INSERT INTO pr VALUES (3002, '2025-03-01', 'k1', 9)"
peq "after writes to the detached partition"
eq14 "$(q "SELECT improv.recalc('m_pr') + improv.recalc('m_pl')")" 0 "the partitioned models equal a fresh recalc"
eq14 "$(q "SELECT count(*) FROM improv.sync_state WHERE last_error IS NOT NULL")" 0 "no model failed"
for t in pr pl; do
	[ "$(tail -c +$((logpos19 + 1)) "$D/log" | grep -c "model m_$t: rebuilding (partitions changed)")" -ge 2 ] \
		|| { echo "FAIL: m_$t did not rebuild on ATTACH and DETACH"; exit 1; }
done
echo "ok: range- and list-partitioned tables follow row changes, cross-partition moves, ATTACH, DETACH and TRUNCATE of a partition"

# 20. a fast shutdown stops the worker (D14): SIGTERM ended it only between
#     polls, so `pg_ctl stop -m fast` hung while a poll ran on. It must stop
#     within 10 s with the worker idle, and in the middle of a batch (a
#     300k-cell model applying 300k changed rows); after each restart the
#     results catch up.
stop20() { # label
	local t0; t0=$(date +%s%N)
	"$PG_BIN/pg_ctl" -D "$D" stop -m fast -t 10 >/dev/null 2>&1 \
		|| { echo "FAIL: pg_ctl stop -m fast with the worker $1 did not stop in 10 s"; tail -20 "$D/log"; exit 1; }
	echo "ok: pg_ctl stop -m fast with the worker $1: stopped in $(( ($(date +%s%N) - t0) / 1000000 )) ms"
	"$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null 3>&-
}
q "CREATE TABLE sd (k text, v float8)"
q "INSERT INTO sd VALUES ('a', 1)"
q "SELECT improv.create_model('m_sd')" >/dev/null
q "SELECT improv.bind_category('m_sd', 'K', 'sd', 'k')" >/dev/null
q "SELECT improv.bind_measure('m_sd', 'V', 'sd', 'v')" >/dev/null
q "SELECT improv.define('m_sd', 'W = V * 2')" >/dev/null
SDQ="SELECT count(*) || ' ' || sum(value) FROM improv_m_sd.w"
wait_for "$SDQ" "1 2" "m_sd caught up" >/dev/null
sleep 0.5
stop20 idle
wait_for "$SDQ" "1 2" "m_sd after the idle stop" >/dev/null
q "INSERT INTO sd SELECT 'k' || g, g FROM generate_series(1, 300000) g"
sleep 0.3
stop20 "mid-batch"
want20=$(q "SELECT count(*) || ' ' || sum(v) * 2 FROM sd")
for _ in $(seq 1 6); do [ "$(q "$SDQ")" = "$want20" ] && break; sleep 5; done
wait_for "$SDQ" "$want20" "m_sd after the mid-batch stop" >/dev/null
echo "ok: after both stops the worker caught up ($want20)"
# the worker logs a standby snapshot (a running-xacts record) after each
# batch it applies, so the slot's restart point keeps up and a poll decodes
# new WAL only. PG 15 has no pg_log_standby_snapshot(): there it logged
# none, the restart point stayed ~140 MB back, and every peek decoded all
# of it (0.67 s). Three batches in a few seconds: the bgwriter (every 15 s)
# and a checkpoint log at most two records of their own.
w0=$(q "SELECT pg_current_wal_flush_lsn()")
for i in 1 2 3; do
	q "UPDATE sd SET v = v + 1 WHERE k = 'a'"
	wait_for "SELECT value FROM improv_m_sd.w WHERE k = 'a'" $((2 * (1 + i))) "m_sd batch $i" >/dev/null
done
sleep 0.3
snaps=$("$PG_BIN/pg_waldump" -p "$D/pg_wal" -s "$w0" -e "$(q "SELECT pg_current_wal_flush_lsn()")" -r Standby 2>/dev/null \
	| grep -c RUNNING_XACTS || true)
[ "$snaps" -ge 3 ] || { echo "FAIL: 3 applied batches logged $snaps standby snapshots"; exit 1; }
echo "ok: 3 applied batches logged $snaps standby snapshots, so the slot's restart point keeps up"

# 21. parallel apply (B1): four models whose applies take about a second
#     each (a FIXPOINT that never converges runs all its rounds), kept busy
#     by a writer, and a fifth, light model. The light model's change must
#     be visible in < LIGHT_MS (200) at the median, and never wait behind
#     a slow round (< 1 s). Then the worker is killed (-9) and stopped
#     (-m fast) while slow rounds run: the stop takes < 10 s, the slot
#     still reaches the end of the WAL, and every model equals a fresh
#     recalc without a failure.
LIGHT_MS=${LIGHT_MS:-200}
for m in s1 s2 s3 s4; do
	q "CREATE TABLE t_$m (item text PRIMARY KEY, x float8)"
	q "INSERT INTO t_$m SELECT 'i' || g, g FROM generate_series(1, 2000) g"
	q "SELECT improv.create_model('m_$m')" >/dev/null
	q "SELECT improv.bind_category('m_$m', 'Item', 't_$m', 'item')" >/dev/null
	q "SELECT improv.bind_measure('m_$m', 'X', 't_$m', 'x')" >/dev/null
	q "SELECT improv.define('m_$m', 'F = FIXPOINT(0, LAMBDA(r, X - r), 1451, 0)')" >/dev/null
done
q "CREATE TABLE t_light (k text PRIMARY KEY, v float8)"
q "INSERT INTO t_light VALUES ('a', 0)"
q "SELECT improv.create_model('m_light')" >/dev/null
q "SELECT improv.bind_category('m_light', 'K', 't_light', 'k')" >/dev/null
q "SELECT improv.bind_measure('m_light', 'V', 't_light', 'v')" >/dev/null
q "SELECT improv.define('m_light', 'W = V * 2')" >/dev/null
SLOW="SELECT string_agg(value::text, ',' ORDER BY m) FROM (SELECT 1 m, value FROM improv_m_s1.f WHERE item = 'i1' \
  UNION ALL SELECT 2, value FROM improv_m_s2.f WHERE item = 'i1' UNION ALL SELECT 3, value FROM improv_m_s3.f WHERE item = 'i1' \
  UNION ALL SELECT 4, value FROM improv_m_s4.f WHERE item = 'i1') s"
WANTSLOW="SELECT string_agg(x::text, ',' ORDER BY t) FROM (SELECT 1 t, x FROM t_s1 WHERE item = 'i1' \
  UNION ALL SELECT 2, x FROM t_s2 WHERE item = 'i1' UNION ALL SELECT 3, x FROM t_s3 WHERE item = 'i1' \
  UNION ALL SELECT 4, x FROM t_s4 WHERE item = 'i1') s"
LIGHT="SELECT value FROM improv_m_light.w WHERE k = 'a'"
wait_for "$SLOW" "1,1,1,1" "the slow models caught up" >/dev/null
wait_for "$LIGHT" 0 "the light model caught up" >/dev/null
q "UPDATE t_s1 SET x = x + 1 WHERE item = 'i1'"
slow_ms=$(wait_for "SELECT value FROM improv_m_s1.f WHERE item = 'i1'" 2 "one slow apply")
writer21() {
	rm -f "$D/stop21"
	# (also stops when the data dir goes: a failed run must not leave it)
	( while [ -d "$D" ] && [ ! -f "$D/stop21" ]; do
		q "UPDATE t_s1 SET x = x + 1 WHERE item = 'i1'; UPDATE t_s2 SET x = x + 1 WHERE item = 'i1';
		   UPDATE t_s3 SET x = x + 1 WHERE item = 'i1'; UPDATE t_s4 SET x = x + 1 WHERE item = 'i1'" 2>/dev/null || true
		sleep 0.2; done ) &
	writer=$!
	sleep 2 # every slow model busy
}
writer21
lat21=()
for i in $(seq 1 10); do
	q "UPDATE t_light SET v = $i"
	lat21+=("$(wait_for "$LIGHT" $((2 * i)) "light change $i")")
	sleep 0.3
done
read -r med21 max21 < <(printf '%s\n' "${lat21[@]}" | sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)], a[NR]}')
[ "$med21" -lt "$LIGHT_MS" ] || { echo "FAIL: the light model's changes took ${lat21[*]} ms (median $med21) beside four slow models"; tail -20 "$D/log"; exit 1; }
[ "$max21" -lt 1000 ] || { echo "FAIL: a light change waited $max21 ms, behind a slow round"; tail -20 "$D/log"; exit 1; }
echo "ok: beside four models applying for ${slow_ms} ms each, a light model's change is visible in $med21 ms (median of 10; max $max21)"
worker21() { q "SELECT pid FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'" 2>/dev/null || true; }
# a backend killed -9 makes the postmaster restart the cluster: a crash
old21=$(worker21)
kill -9 "$old21"
for _ in $(seq 1 120); do new21=$(worker21); [ -n "$new21" ] && [ "$new21" != "$old21" ] && break; sleep 0.25; done
[ -n "$new21" ] && [ "$new21" != "$old21" ] || { echo "FAIL: the worker did not come back after kill -9"; tail -20 "$D/log"; exit 1; }
# a restarted worker rebuilds every model first, one after the other
wait_long() { for _ in $(seq 1 6); do [ "$(q "$1" 2>/dev/null)" = "$2" ] && break; sleep 5; done; wait_for "$@" >/dev/null; }
q "UPDATE t_light SET v = 100"
wait_long "$LIGHT" 200 "the light model after kill -9"
sleep 2 # slow rounds running again
stop20 "in a slow round"
q "UPDATE t_light SET v = 101"
wait_long "$LIGHT" 202 "the light model after the fast stop"
sleep 1
touch "$D/stop21"; wait "$writer"
end21=$(q "SELECT pg_current_wal_insert_lsn()")
wait_long "$SLOW" "$(q "$WANTSLOW")" "the slow models after the writer"
wait_for "SELECT confirmed_flush_lsn >= '$end21' FROM pg_replication_slots WHERE slot_name = 'improv_db'" t "the slot past the last write" >/dev/null
eq14 "$(q "SELECT sum(improv.recalc(name)) FROM improv.model WHERE name ~ '^m_(s[1-4]|light)$'")" 0 \
	"every model equals a fresh recalc after kill -9 and a fast stop"
eq14 "$(q "SELECT count(*) FROM improv.sync_state WHERE last_error IS NOT NULL")" 0 "no model failed"
echo "ok: killed (-9) and stopped (-m fast) mid-round, the worker caught up; the slot reached the end; every model equals a fresh recalc"

# 22. an idle worker keeps its model listing: with no transaction since its
#     last listing it does not list the models again (it did every poll,
#     the idle cost at 1,000 models); each kind of change it lists for is
#     still seen at once: define (model_version), a new model, a drop, a
#     switch out of logical mode, a partition attached, and a data write.
q "ALTER SYSTEM SET log_min_messages = debug1"
q "SELECT pg_reload_conf()" >/dev/null
q "ALTER SYSTEM SET autovacuum = off" # an autovacuum or ANALYZE takes an xid
q "SELECT pg_reload_conf()" >/dev/null
sleep 1
lists() { tail -c +$((logpos22 + 1)) "$D/log" | grep -c "pg_improv sync: listed" || true; }
logpos22=$(wc -c <"$D/log")
sleep 5
idle22=$(lists)
# 5 s at poll_ms 50 is about 100 polls; a bgwriter snapshot takes no xid
[ "$idle22" -le 2 ] || { echo "FAIL: 5 s idle listed the models $idle22 times"; exit 1; }
q "CREATE TABLE t22 (k text PRIMARY KEY, v float8)"
q "INSERT INTO t22 VALUES ('a', 1), ('b', 2)"
q "SELECT improv.create_model('m22')" >/dev/null
q "SELECT improv.bind_category('m22', 'K', 't22', 'k')" >/dev/null
q "SELECT improv.bind_measure('m22', 'V', 't22', 'v')" >/dev/null
q "SELECT improv.define('m22', 'W = V * 2')" >/dev/null
W22="SELECT string_agg(k || '=' || value, ',' ORDER BY k) FROM improv_m22.w"
wait_for "$W22" "a=2,b=4" "a new model, while the worker keeps its listing" >/dev/null
q "SELECT improv.define('m22', 'W = V * 3', replace => true)" >/dev/null
wait_for "$W22" "a=3,b=6" "define on a listed model" >/dev/null
q "UPDATE t22 SET v = 10 WHERE k = 'a'"
wait_for "$W22" "a=30,b=6" "a data write" >/dev/null
q "SELECT improv.set_sync_mode('m22', 'manual')" >/dev/null
q "UPDATE t22 SET v = 20 WHERE k = 'a'"
sleep 1
eq14 "$(q "$W22")" "a=30,b=6" "a model switched to manual is no longer applied"
q "SELECT improv.set_sync_mode('m22', 'logical')" >/dev/null
wait_for "$W22" "a=60,b=6" "the same model back in logical mode" >/dev/null
q "CREATE TABLE p22 (k text, v float8) PARTITION BY LIST (k)"
q "CREATE TABLE p22_a PARTITION OF p22 FOR VALUES IN ('a')"
q "INSERT INTO p22 VALUES ('a', 1)"
q "SELECT improv.create_model('mp22')" >/dev/null
q "SELECT improv.bind_category('mp22', 'K', 'p22', 'k')" >/dev/null
q "SELECT improv.bind_measure('mp22', 'V', 'p22', 'v', agg => 'sum')" >/dev/null
q "SELECT improv.define('mp22', 'T = V * 1')" >/dev/null
P22="SELECT string_agg(k || '=' || value, ',' ORDER BY k) FROM improv_mp22.t"
wait_for "$P22" "a=1" "a partitioned model" >/dev/null
q "CREATE TABLE p22_b (k text, v float8)"
q "INSERT INTO p22_b VALUES ('b', 5)"
q "ALTER TABLE p22 ATTACH PARTITION p22_b FOR VALUES IN ('b')"
wait_for "$P22" "a=1,b=5" "a partition attached with rows" >/dev/null
q "SELECT improv.drop_model('m22')" >/dev/null
wait_for "SELECT count(*) FROM improv.sync_state s JOIN improv.model m ON m.id = s.model_id WHERE m.name = 'm22'" 0 "a dropped model" >/dev/null
sleep 1
logpos22=$(wc -c <"$D/log")
sleep 5
idle22b=$(lists)
[ "$idle22b" -le 2 ] || { echo "FAIL: 5 s idle after the changes listed the models $idle22b times"; exit 1; }
eq14 "$(q "SELECT count(*) FROM improv.sync_state WHERE last_error IS NOT NULL")" 0 "no model failed"
q "ALTER SYSTEM RESET log_min_messages"
q "ALTER SYSTEM RESET autovacuum"
q "SELECT pg_reload_conf()" >/dev/null
echo "ok: idle, the worker listed its models $idle22 and $idle22b times in 5 s (about 100 polls); define, a new model, manual and back, a partition attached and a drop were each applied at once"
echo "PASS"
