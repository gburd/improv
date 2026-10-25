#!/usr/bin/env bash
# Soak test (road-to-1.0 J4): logical mode under a steady write load (two
# writers, ~20 statements/s each, INSERT/UPDATE/DELETE on three tables),
# with a random disruption every EVENT_MIN to 3 x EVENT_MIN minutes (a
# worker restart, a server restart, a failover to the streaming standby
# and back on the next one, a redefinition), for SOAK_HOURS (default 72).
# Every CHECK_MIN minutes (default 60) it pauses the writers and checks:
#   - no drift: each logical model's result tables equal, within 5 min, a
#     fresh improv.recalc of its manual-mode twin over the same tables, and
#     one of them equals the SQL aggregate;
#   - the writers made progress since the last check;
#   - no memory growth: the sync worker's private memory (RssAnon) stays
#     under MAX_ANON_KB (default 4x the first check's, at least 64 MB);
#   - no WAL retention growth: the improv_db slot's lag behind the WAL head
#     stays under MAX_LAG_B (default 64 MB).
# One line per check goes to $BASE/soak.tsv; any failure stops the run.
#
#   PG_BIN=/usr/lib/postgresql/18/bin SOAK_HOURS=72 crates/pg_improv/tests/soak.sh
#
# Needs pg_improv installed into the PG found by PG_BIN (PG 17+ for the
# failover slot). A short run (SOAK_HOURS=0.25 CHECK_MIN=3) checks the harness.
set -Eeuo pipefail
# A failing command anywhere (functions too: -E) names itself in the log
# before set -e ends the run.
trap 'echo "$(date -u +%FT%TZ) FAIL: line $LINENO: $BASH_COMMAND exited $?" >>"$BASE/soak.log"' ERR
PG_BIN=${PG_BIN:-/usr/lib/postgresql/18/bin}
BASE=${PGI_DATA:-$HOME/pgi-soak}
P1=${PGI_PORT:-55460}
P2=$((P1 + 1))
HOURS=${SOAK_HOURS:-72}
CHECK_MIN=${CHECK_MIN:-60}
# A disruption every EVENT_MIN to 3 x EVENT_MIN minutes (default 5-15), from EVENTS.
EVENT_MIN=${EVENT_MIN:-5}
EVENTS=${EVENTS:-restart_worker restart_server failover redefine}
DB=soak
end=$(( $(date +%s) + $(awk "BEGIN { printf \"%d\", $HOURS * 3600 }") ))

log() { echo "$(date -u +%FT%TZ) $*" | tee -a "$BASE/soak.log"; }
fail() { log "FAIL: $*"; for d in a b; do echo "--- $d"; tail -30 "$BASE/$d/log" 2>/dev/null; done | tee -a "$BASE/soak.log"; touch "$BASE/stop"; exit 1; }

# Which node is the primary, a or b (they swap on each failover): in a file,
# so the background writers follow a failover too.
PRI=a
set_primary() { PRI=$1; echo "$1" >"$BASE/primary"; }
port() { [ "$1" = a ] && echo $P1 || echo $P2; }
other() { [ "$1" = a ] && echo b || echo a; }
q() { PRI=$(cat "$BASE/primary"); "$PG_BIN/psql" -h "$BASE/$PRI" -p "$(port $PRI)" -U improv -d $DB -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
qn() { "$PG_BIN/psql" -h "$BASE/$1" -p "$(port "$1")" -U improv -d $DB -qAtX -v ON_ERROR_STOP=1 -c "$2"; }

conf() { # node
	cat >>"$BASE/$1/postgresql.conf" <<-EOF
		port = $(port "$1")
		unix_socket_directories = '$BASE/$1'
		shared_preload_libraries = 'pg_improv'
		improv.database = '$DB'
		improv.poll_ms = 50
		wal_level = logical
		max_replication_slots = 10
		max_wal_senders = 10
		hot_standby_feedback = on
		sync_replication_slots = on
		max_slot_wal_keep_size = '20GB'
		logging_collector = off
	EOF
}

# A streaming standby of the primary, with a physical slot and slot sync.
standby() {
	local s; s=$(other $PRI)
	[ ! -d "$BASE/$s" ] || find "$BASE/$s" -delete
	q "SELECT pg_drop_replication_slot('standby1') FROM pg_replication_slots WHERE slot_name = 'standby1'" >/dev/null
	q "SELECT 1 FROM pg_create_physical_replication_slot('standby1')" >/dev/null
	"$PG_BIN/pg_basebackup" -h "$BASE/$PRI" -p "$(port $PRI)" -U improv -D "$BASE/$s" -R -X stream --checkpoint=fast
	# the base backup copied the primary's conf: this node's port and socket win
	cat >>"$BASE/$s/postgresql.conf" <<-EOF
		port = $(port "$s")
		unix_socket_directories = '$BASE/$s'
	EOF
	cat >>"$BASE/$s/postgresql.auto.conf" <<-EOF
		primary_conninfo = 'host=$BASE/$PRI port=$(port $PRI) user=improv dbname=$DB'
		primary_slot_name = 'standby1'
		synchronized_standby_slots = ''
	EOF
	"$PG_BIN/pg_ctl" -D "$BASE/$s" -l "$BASE/$s/log" -w start >/dev/null 3>&-
	q "ALTER SYSTEM SET synchronized_standby_slots = 'standby1'"
	q "SELECT pg_reload_conf()" >/dev/null
}

# The models: each logical model has a manual-mode twin over the same tables;
# a check recalcs the twin and compares the result tables.
#   ev    (2.5k-row-ish event log, keys in 3 x 20)  sum + count over K, Day
#   sales (100 products, price x qty)               element-wise Revenue, SUM over all
#   tags  (rows with a text tag)                    count_distinct
setup() {
	q "CREATE EXTENSION pg_improv"
	q "CREATE TABLE ev (id bigint PRIMARY KEY, k text NOT NULL, day int NOT NULL, n float8 NOT NULL)"
	q "INSERT INTO ev SELECT g, 'k' || g % 3, g % 20, (g % 7) * 0.5 FROM generate_series(1, 3000) g"
	q "CREATE TABLE sales (product text PRIMARY KEY, price float8 NOT NULL, qty float8 NOT NULL)"
	q "INSERT INTO sales SELECT 'p' || g, g % 13 + 1, g % 29 FROM generate_series(1, 100) g"
	q "CREATE TABLE tags (id bigint PRIMARY KEY, k text NOT NULL, tag text NOT NULL)"
	q "INSERT INTO tags SELECT g, 'k' || g % 3, 't' || g % 50 FROM generate_series(1, 2000) g"
	for m in ev evm; do
		for s in "create_model('$m')" "bind_category('$m','K','ev','k')" "bind_category('$m','Day','ev','day')" \
			"bind_measure('$m','N','ev','n', agg => 'sum')" "bind_measure('$m','C','ev','id', agg => 'count')" \
			"define('$m','Total = SUM(N OVER Day)')" "define('$m','Mean = N / C')"; do
			q "SELECT improv.$s" >/dev/null
		done
	done
	for m in sa sam; do
		for s in "create_model('$m')" "bind_category('$m','Product','sales','product')" \
			"bind_measure('$m','Price','sales','price')" "bind_measure('$m','Quantity','sales','qty')" \
			"define('$m','Revenue = Price * Quantity')" "define('$m','All = SUM(Revenue OVER Product)')"; do
			q "SELECT improv.$s" >/dev/null
		done
	done
	for m in tg tgm; do
		for s in "create_model('$m')" "bind_category('$m','K','tags','k')" \
			"bind_measure('$m','D','tags','tag', agg => 'count_distinct')" "define('$m','D2 = D * 2')"; do
			q "SELECT improv.$s" >/dev/null
		done
	done
	for m in evm sam tgm; do q "SELECT improv.set_sync_mode('$m', 'manual')" >/dev/null; done
}

# One string per model: every result table's rows, sorted.
results() { # model
	local m=$1 out="" t
	for t in $(q "SELECT table_name FROM information_schema.tables WHERE table_schema = 'improv_$m' ORDER BY 1"); do
		out="$out|$t:$(q "SELECT md5(coalesce(string_agg(x::text, ',' ORDER BY x::text), '')) FROM improv_$m.$t x")"
	done
	echo "$out"
}

writer() { # one writer: a mix of INSERT, UPDATE and DELETE on all three tables, ~20 statements/s
	local nid=$((1000000 * $1))
	while [ ! -f "$BASE/stop" ]; do
		if [ -f "$BASE/pause" ]; then sleep 0.2; continue; fi
		nid=$((nid + 1))
		case $((RANDOM % 6)) in
		0 | 1) q "INSERT INTO ev VALUES ($nid, 'k' || $((RANDOM % 3)), $((RANDOM % 20)), $((RANDOM % 9)) * 0.25)" ;;
		2) q "DELETE FROM ev WHERE id = (SELECT id FROM ev ORDER BY id OFFSET $((RANDOM % 2000)) LIMIT 1)" ;;
		3) q "UPDATE sales SET qty = qty + $((RANDOM % 5 - 2)), price = price * 1.01 WHERE product = 'p$((RANDOM % 100 + 1))'" ;;
		4) q "INSERT INTO tags VALUES ($nid, 'k' || $((RANDOM % 3)), 't' || $((RANDOM % 80)))" ;;
		5) q "BEGIN; UPDATE ev SET n = n + 0.5 WHERE id = (SELECT id FROM ev ORDER BY id DESC LIMIT 1 OFFSET $((RANDOM % 50))); DELETE FROM tags WHERE id = (SELECT min(id) FROM tags); COMMIT" ;;
		esac >/dev/null 2>&1 || true
		sleep 0.05
	done
}

# The sync worker of the current primary: retried, since right after a
# restart or promotion it may not have started yet. (`if`, not `[ … ] &&
# echo`: under set -e and pipefail a last non-matching pid fails the loop.)
worker_pid() {
	local pm x
	PRI=$(cat "$BASE/primary")
	for _ in $(seq 1 100); do
		pm=$(head -1 "$BASE/$PRI/postmaster.pid")
		for x in $(pgrep -f "pg_improv sync" || true); do
			if [ "$(ps -o ppid= -p "$x" | tr -d ' ')" = "$pm" ]; then echo "$x"; return 0; fi
		done
		sleep 0.1
	done
	fail "no sync worker under the primary ($PRI) after 10 s"
}

# Pause the writers, wait for every logical model to apply what was written,
# then compare each with its recalculated twin.
check() {
	# Writers paused (each finishes its statement within ~0.1 s): the twins'
	# recalc is the truth, and each live model must reach it within 5 min.
	touch "$BASE/pause"; sleep 2
	local t0; t0=$(date +%s)
	for pair in ev:evm sa:sam tg:tgm; do
		local live=${pair%%:*} twin=${pair##*:} a="" b
		q "SELECT improv.recalc('$twin')" >/dev/null
		b=$(results "$twin")
		[ "$b" != "" ] || fail "model $twin has no result tables"
		while [ $(( $(date +%s) - t0 )) -lt 300 ]; do
			a=$(results "$live")
			[ "$a" = "$b" ] && break
			sleep 0.5
		done
		[ "$a" = "$b" ] || fail "drift: $live ($a) differs from a fresh recalc of $twin ($b) after 5 min; status: $(q "SELECT row_to_json(s) FROM improv.status('$live') s")"
	done
	local caught=$(( $(date +%s) - t0 ))
	# and one against plain SQL
	local s1 s2
	s1=$(q "SELECT coalesce(string_agg(k || '=' || round(value::numeric, 6), ',' ORDER BY k), '') FROM improv_ev.total")
	s2=$(q "SELECT coalesce(string_agg(k || '=' || round(s::numeric, 6), ',' ORDER BY k), '') FROM (SELECT k, sum(n) s FROM ev GROUP BY k) x")
	[ "$s1" = "$s2" ] || fail "drift: improv_ev.total ($s1) differs from SQL ($s2)"
	local w anon est lag rows
	w=$(worker_pid)
	anon=$(awk '/^RssAnon:/ {print $2}' /proc/"$w"/status 2>/dev/null || echo 0)
	est=$(q "SELECT coalesce(sum(fold_bytes + engine_bytes), 0)::bigint / 1024 FROM improv.model m, improv.status(m.name) s WHERE m.sync_mode = 'logical'")
	lag=$(q "SELECT pg_wal_lsn_diff(pg_current_wal_lsn(), confirmed_flush_lsn) FROM pg_replication_slots WHERE slot_name = 'improv_db'")
	rows=$(q "SELECT (SELECT count(*) FROM ev) + (SELECT count(*) FROM tags)")
	# the writers are alive: the newest id moved since the last check
	local top; top=$(q "SELECT max(id) FROM ev")
	if [ -n "${last_top:-}" ] && [ "$top" = "$last_top" ]; then fail "the writers made no progress since the last check (max id $top)"; fi
	last_top=$top
	first_anon=${first_anon:-$anon}
	local cap=$(( first_anon * 4 > 65536 ? first_anon * 4 : 65536 ))
	[ "$anon" -le "${MAX_ANON_KB:-$cap}" ] || fail "worker memory grew: ${anon} kB, first check ${first_anon} kB"
	[ "${lag%.*}" -le "${MAX_LAG_B:-67108864}" ] || fail "the slot retains ${lag} B of WAL"
	printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$(date -u +%FT%TZ)" "$PRI" "$anon" "$est" "$lag" "$rows" "$events" >>"$BASE/soak.tsv"
	log "check ok: primary $PRI, caught up in ${caught} s, worker anon ${anon} kB (estimate ${est} kB), slot lag ${lag} B, ${rows} rows, events: $events"
	rm -f "$BASE/pause"
}

# Disruptions, one at random every 5-15 min.
restart_worker() { local w; w=$(worker_pid); [ -n "$w" ] && kill -TERM "$w"; }
restart_server() { "$PG_BIN/pg_ctl" -D "$BASE/$PRI" -m fast -w restart -l "$BASE/$PRI/log" >/dev/null 3>&-; }
failover() {
	local old=$PRI new; new=$(other $PRI)
	# let the synced slot catch up, then stop the primary and promote the standby
	sleep 2
	"$PG_BIN/pg_ctl" -D "$BASE/$old" -m fast -w stop >/dev/null
	"$PG_BIN/pg_ctl" -D "$BASE/$new" -w promote >/dev/null
	set_primary "$new"
	for _ in $(seq 1 200); do [ "$(q "SELECT pg_is_in_recovery()" 2>/dev/null)" = f ] && break; sleep 0.1; done
	q "ALTER SYSTEM SET synchronized_standby_slots = ''"
	q "SELECT pg_reload_conf()" >/dev/null
	# the old primary becomes the new standby, from a fresh base backup
	standby
}
redefine() { # a formula change on a live model and its twin (rebuilds the live one)
	local f; f=$([ $((RANDOM % 2)) = 0 ] && echo "Mean = N / C" || echo "Mean = N / C * 1")
	q "SELECT improv.define('ev', '$f', replace => true)" >/dev/null
	q "SELECT improv.define('evm', '$f', replace => true)" >/dev/null
}

mkdir -p "$BASE"
for d in a b; do [ ! -d "$BASE/$d" ] || find "$BASE/$d" -delete; done
rm -f "$BASE/stop" "$BASE/pause" "$BASE/soak.tsv"
set_primary a
trap 'touch "$BASE/stop"; wait; for d in a b; do "$PG_BIN/pg_ctl" -D "$BASE/$d" -m fast stop >/dev/null 2>&1 || true; done' EXIT
"$PG_BIN/initdb" -D "$BASE/a" -A trust -U improv >/dev/null
conf a
"$PG_BIN/pg_ctl" -D "$BASE/a" -l "$BASE/a/log" -w start >/dev/null 3>&-
"$PG_BIN/createdb" -h "$BASE/a" -p "$P1" -U improv $DB
setup
standby
printf 'time\tprimary\tworker_anon_kb\testimate_kb\tslot_lag_b\trows\tevents\n' >"$BASE/soak.tsv"
events=""
log "soak start: $HOURS h, checks every $CHECK_MIN min, PG $("$PG_BIN/postgres" --version)"
last_top=""
check
writer 1 & writer 2 &
next_check=$(( $(date +%s) + CHECK_MIN * 60 ))
next_event=$(( $(date +%s) + EVENT_MIN * 60 + RANDOM % (EVENT_MIN * 120) ))
events=""
while [ "$(date +%s)" -lt "$end" ]; do
	sleep 5
	now=$(date +%s)
	if [ "$now" -ge "$next_event" ]; then
		e=$(echo $EVENTS | tr ' ' '\n' | shuf -n 1)
		log "event: $e (primary $PRI)"
		$e || fail "event $e failed"
		events="$events $e"
		next_event=$(( now + EVENT_MIN * 60 + RANDOM % (EVENT_MIN * 120) ))
	fi
	if [ "$now" -ge "$next_check" ]; then
		check
		events=""
		next_check=$(( now + CHECK_MIN * 60 ))
	fi
done
check
log "PASS: $HOURS h"
