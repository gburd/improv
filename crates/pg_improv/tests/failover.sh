#!/usr/bin/env bash
# Physical standbys and failover (D1). A primary serves logical-mode models
# under a steady writer, and a streaming standby (pg_basebackup -R) follows
# it. Checks that:
#   - the result tables replicate as ordinary tables;
#   - no sync worker runs on the hot standby, and improv.status works there;
#   - after the primary stops and the standby is promoted, the worker starts
#     on the new primary, every model's results equal a fresh recalc and the
#     SQL aggregates, and the models keep following writes.
# Two runs: "plain" (no slot sync: the slot is gone after promotion, so the
# worker makes a new one and rebuilds each model) and, on PG 17+,
# "failover" (a failover slot plus sync_replication_slots: the worker
# continues on the synced slot and creates none).
#
#   PG_BIN=/usr/lib/postgresql/18/bin crates/pg_improv/tests/failover.sh
#
# Needs pg_improv installed into the PG found by PG_BIN (default: PG 17).
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
BASE=${PGI_DATA:-$HOME/pgi-failover}
P1=${PGI_PORT:-55440}
P2=$((P1 + 1))
PGV=$("$PG_BIN/postgres" --version | sed -E 's/[^0-9]*([0-9]+).*/\1/')
DB=improv_e2e

stop_all() {
	for d in "$BASE/a" "$BASE/b"; do "$PG_BIN/pg_ctl" -D "$d" stop -m immediate >/dev/null 2>&1 || true; done
	if [ -d "$BASE" ]; then find "$BASE" -mindepth 1 -delete && rmdir "$BASE"; fi
}
trap '[ -n "${KEEP:-}" ] || stop_all' EXIT

qa() { "$PG_BIN/psql" -h "$BASE/a" -p "$P1" -U improv -d $DB -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
qb() { "$PG_BIN/psql" -h "$BASE/b" -p "$P2" -U improv -d $DB -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
fail() { echo "FAIL ($mode): $*"; for d in a b; do echo "--- $d"; tail -15 "$BASE/$d/log" 2>/dev/null; done; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got '$1', want '$2'"; }
wait_for() { # query-fn sql want label
	local v=""
	for _ in $(seq 1 600); do
		v=$($1 "$2" 2>/dev/null || true)
		[ "$v" = "$3" ] && return 0
		sleep 0.05
	done
	fail "timed out waiting for $4: got '$v', want '$3'"
}

# each model's results, and the same from SQL
TOT="SELECT coalesce(string_agg(k || '=' || value, ',' ORDER BY k), '') FROM improv_fa.t"
SUMK="SELECT coalesce(string_agg(k || '=' || s, ',' ORDER BY k), '') FROM (SELECT k, sum(n) s FROM ev GROUP BY k) x"
REV="SELECT coalesce(string_agg(product || '=' || value, ',' ORDER BY product), '') FROM improv_fb.revenue"
REVQ="SELECT coalesce(string_agg(product || '=' || (price * qty), ',' ORDER BY product), '') FROM sales"

run() {
	mode=$1
	stop_all
	mkdir -p "$BASE"
	"$PG_BIN/initdb" -D "$BASE/a" -A trust -U improv >/dev/null
	cat >>"$BASE/a/postgresql.conf" <<-EOF
		port = $P1
		unix_socket_directories = '$BASE/a'
		shared_preload_libraries = 'pg_improv'
		improv.database = '$DB'
		improv.poll_ms = 50
		wal_level = logical
		max_replication_slots = 10
		max_wal_senders = 10
		hot_standby_feedback = on
	EOF
	"$PG_BIN/pg_ctl" -D "$BASE/a" -l "$BASE/a/log" -w start >/dev/null 3>&-
	"$PG_BIN/createdb" -h "$BASE/a" -p "$P1" -U improv $DB
	qa "CREATE EXTENSION pg_improv"
	qa "CREATE TABLE ev (id int PRIMARY KEY, k text, n float8)"
	qa "INSERT INTO ev SELECT g, 'k' || g % 3, 1 FROM generate_series(1, 30) g"
	qa "CREATE TABLE sales (product text PRIMARY KEY, price float8, qty float8)"
	qa "INSERT INTO sales VALUES ('Widget', 10, 100), ('Gadget', 20, 30)"
	for s in "create_model('fa')" "bind_category('fa','K','ev','k')" \
		"bind_measure('fa','N','ev','n', agg => 'sum')" "define('fa','T = N * 1')" \
		"create_model('fb')" "bind_category('fb','Product','sales','product')" \
		"bind_measure('fb','Price','sales','price')" "bind_measure('fb','Quantity','sales','qty')" \
		"define('fb','Revenue = Price * Quantity')"; do
		qa "SELECT improv.$s" >/dev/null
	done
	wait_for qa "$TOT" "$(qa "$SUMK")" "fa caught up on the primary"
	wait_for qa "$REV" "$(qa "$REVQ")" "fb caught up on the primary"
	if [ "$mode" = failover ]; then
		eq "$(qa "SELECT failover FROM pg_replication_slots WHERE slot_name = 'improv_db'")" t "improv_db is a failover slot"
		qa "SELECT 1 FROM pg_create_physical_replication_slot('standby1')" >/dev/null
		# logical decoding waits for the standby, so the synced slot is never
		# behind what the models applied
		qa "ALTER SYSTEM SET synchronized_standby_slots = 'standby1'"
		qa "SELECT pg_reload_conf()" >/dev/null
	fi

	# a writer on the primary, until it stops
	( i=1000; while [ ! -f "$BASE/stop" ]; do
		qa "INSERT INTO ev VALUES ($i, 'k' || $i % 3, 1); UPDATE sales SET qty = qty + 1 WHERE product = 'Widget'" 2>/dev/null || true
		i=$((i + 1)); sleep 0.02; done ) &
	writer=$!

	# the standby, from a base backup taken under the writer
	"$PG_BIN/pg_basebackup" -h "$BASE/a" -p "$P1" -U improv -D "$BASE/b" -R -X stream
	cat >>"$BASE/b/postgresql.conf" <<-EOF
		port = $P2
		unix_socket_directories = '$BASE/b'
	EOF
	if [ "$mode" = failover ]; then
		cat >>"$BASE/b/postgresql.auto.conf" <<-EOF
			primary_conninfo = 'host=$BASE/a port=$P1 user=improv dbname=$DB'
			primary_slot_name = 'standby1'
			sync_replication_slots = on
			# copied from the primary, where it names this standby's slot: on
			# the promoted standby it would name a slot that does not exist,
			# and logical decoding would wait for it forever
			synchronized_standby_slots = ''
		EOF
	fi
	"$PG_BIN/pg_ctl" -D "$BASE/b" -l "$BASE/b/log" -w start >/dev/null 3>&-
	eq "$(qb "SELECT pg_is_in_recovery()")" t "the standby is in recovery"
	sleep 1

	# the results replicate: pause the writer, let the worker apply its
	# writes, then the standby replays to the primary's position
	touch "$BASE/stop"; wait "$writer"; rm -f "$BASE/stop"
	wait_for qa "$TOT" "$(qa "$SUMK")" "fa follows the writer"
	wait_for qa "$REV" "$(qa "$REVQ")" "fb follows the writer"
	lsn=$(qa "SELECT pg_current_wal_lsn()")
	wait_for qb "SELECT pg_last_wal_replay_lsn() >= '$lsn'" t "the standby replays"
	eq "$(qb "$TOT")" "$(qa "$TOT")" "fa's results on the standby"
	eq "$(qb "$REV")" "$(qa "$REV")" "fb's results on the standby"
	eq "$(qb "SELECT count(*) FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'")" 0 \
		"no sync worker on the hot standby"
	eq "$(qb "SELECT sync_mode || ' ' || (lag_bytes IS NULL OR lag_bytes >= 0) FROM improv.status('fa')")" \
		"logical true" "improv.status on the standby"
	if [ "$mode" = failover ]; then
		wait_for qb "SELECT synced AND NOT temporary FROM pg_replication_slots WHERE slot_name = 'improv_db'" t \
			"improv_db synced to the standby"
		# the synced slot is past the writer's last batch
		qa "UPDATE sales SET price = 11 WHERE product = 'Widget'"
		wait_for qa "$REV" "$(qa "$REVQ")" "fb after a marker write"
		mark=$(qa "SELECT confirmed_flush_lsn FROM pg_replication_slots WHERE slot_name = 'improv_db'")
		wait_for qb "SELECT confirmed_flush_lsn >= '$mark' FROM pg_replication_slots WHERE slot_name = 'improv_db'" t \
			"the synced slot keeps up"
	fi
	echo "ok ($mode): results replicate to the standby; no worker there; status works there"

	# failover: the writer streams, the primary stops dead, the standby is
	# promoted, and the writer moves to it
	( i=5000; while [ ! -f "$BASE/stop" ]; do
		qa "INSERT INTO ev VALUES ($i, 'k' || $i % 3, 1)" 2>/dev/null || true
		i=$((i + 1)); sleep 0.02; done ) &
	writer=$!
	sleep 1
	"$PG_BIN/pg_ctl" -D "$BASE/a" stop -m immediate >/dev/null
	touch "$BASE/stop"; wait "$writer"; rm -f "$BASE/stop"
	logpos=$(wc -c <"$BASE/b/log")
	"$PG_BIN/pg_ctl" -D "$BASE/b" promote -w >/dev/null
	eq "$(qb "SELECT pg_is_in_recovery()")" f "the standby is promoted"
	wait_for qb "SELECT count(*) FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'" 1 \
		"the sync worker starts on the new primary"
	( i=9000; while [ ! -f "$BASE/stop" ]; do
		qb "INSERT INTO ev VALUES ($i, 'k' || $i % 3, 1)" 2>/dev/null || true
		i=$((i + 1)); sleep 0.02; done ) &
	writer=$!
	sleep 1
	qb "UPDATE sales SET price = 12 WHERE product = 'Gadget'"
	touch "$BASE/stop"; wait "$writer"; rm -f "$BASE/stop"
	wait_for qb "$TOT" "$(qb "$SUMK")" "fa on the new primary"
	wait_for qb "$REV" "$(qb "$REVQ")" "fb on the new primary"
	wait_for qb "SELECT count(*) FROM improv.sync_state WHERE last_error IS NULL AND last_applied_at IS NOT NULL" 2 \
		"both models synced on the new primary"
	eq "$(qb "SELECT improv.recalc('fa') + improv.recalc('fb')")" 0 "the results equal a fresh recalc"
	created=$(tail -c +$((logpos + 1)) "$BASE/b/log" | grep -c "created the slot improv_db" || true)
	if [ "$mode" = failover ]; then
		eq "$created" 0 "the worker kept the synced slot"
		eq "$(qb "SELECT count(*) FROM pg_replication_slots WHERE slot_name = 'improv_db' AND failover")" 1 \
			"the failover slot on the new primary"
	else
		eq "$created" 1 "the worker made a new slot"
	fi
	# and keeps following writes
	qb "UPDATE sales SET qty = 7 WHERE product = 'Gadget'"
	qb "DELETE FROM ev WHERE id < 10"
	wait_for qb "$REV" "$(qb "$REVQ")" "fb follows a write after promotion"
	wait_for qb "$TOT" "$(qb "$SUMK")" "fa follows a write after promotion"
	echo "ok ($mode): promoted; the worker $( [ "$mode" = failover ] && echo "kept the synced slot" || echo "made a new slot and rebuilt" ); results equal a fresh recalc and follow writes"
}

run plain
if [ "$PGV" -ge 17 ]; then run failover; else echo "skip (PG $PGV): failover slots need PG 17"; fi
echo "PASS"
