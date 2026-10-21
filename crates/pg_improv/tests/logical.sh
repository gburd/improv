#!/usr/bin/env bash
# End-to-end test of pg_improv's logical-decoding sync (L2), with real
# commits. Section 13 covers the shared slot: routing by table (two models
# on one table, a model on two), models added, redefined and dropped while
# a writer streams, and a failing model that holds no one back. Needs: cargo-pgrx installed and pg_improv installed into the PG
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
echo "PASS"
