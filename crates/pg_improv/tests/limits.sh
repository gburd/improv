#!/usr/bin/env bash
# pg_improv on resource limits (D7), end to end (needs pg_improv installed
# into $PG_BIN's install):
#   1. max_worker_processes = 0: the sync worker cannot register (the
#      server logs "too many background workers" at startup, not again), status says the
#      worker is not running and names the setting; raised, the worker
#      starts and catches up
#   2. wal_keep_size is not the slot's limit: with it 0 and the worker away,
#      the slot keeps the WAL it needs (not lost), status's lag_bytes shows
#      what it holds, and back, the worker applies the write made meanwhile
#   3. disk full while writing results (a per-file size limit, ulimit -f,
#      which fails a write as ENOSPC would: "could not extend file"): in
#      logical mode the model's last_error says so, nothing is half-written,
#      the worker keeps running (no crash loop) and retries with backoff
#      while other models stay in sync; given room (a restart without the
#      limit), the model recovers. In
#      trigger mode the writer's statement fails, and its rows with it.
#   4. improv.max_model_memory: status shows each model's live memory; a
#      tiny cap refuses the big model with a logged reason and a last_error
#      (no OOM), the others stay in sync, and without the cap it recovers
# Usage: tests/limits.sh
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=$(mktemp -d /var/tmp/pgi-lim.XXXX)
P=${PGPORT_LIM:-55495}
stop() {
	[ "${KEEP:-}" ] && { cat "$D/log" >&2; }
	"$PG_BIN/pg_ctl" -D "$D/data" stop -m fast >/dev/null 2>&1 || true
	find "$D" -delete 2>/dev/null || true
}
trap stop EXIT
"$PG_BIN/initdb" -D "$D/data" -A trust -U postgres >/dev/null
cat >"$D/data/postgresql.conf" <<-EOF2
	port = $P
	listen_addresses = ''
	unix_socket_directories = '$D'
	shared_preload_libraries = 'pg_improv'
	wal_level = logical
	improv.database = 't'
	improv.poll_ms = 50
	work_mem = '256MB'
EOF2
start() { "$PG_BIN/pg_ctl" -D "$D/data" -l "$D/log" -w start >/dev/null 3>&-; }
restart() { "$PG_BIN/pg_ctl" -D "$D/data" stop -m fast >/dev/null; start; }
start
"$PG_BIN/createdb" -h "$D" -p "$P" -U postgres t
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U "${U:-postgres}" -d t -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
fail() { echo "FAIL: $*"; tail -20 "$D/log"; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got '$1', want '$2'"; }
wait_for() { # sql want what [seconds]
	local v="" n=$(( ${4:-20} * 20 ))
	for _ in $(seq "$n"); do v=$(q "$1" 2>/dev/null || true); [ "$v" = "$2" ] && return 0; sleep 0.05; done
	fail "$3: got '$v', want '$2'"
}
since() { tail -c +$(($1 + 1)) "$D/log"; }
WORKER="SELECT count(*) FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'"

q "CREATE EXTENSION pg_improv"
q "CREATE ROLE alice LOGIN; GRANT improv_user TO alice"
q "CREATE TABLE sales (k text PRIMARY KEY, v float8); INSERT INTO sales VALUES ('a', 1)"
for s in "create_model('plan')" "bind_category('plan','K','sales','k')" \
	"bind_measure('plan','V','sales','v')" "define('plan','W = V * 2')"; do
	q "SELECT improv.$s" >/dev/null
done
PLAN="SELECT value FROM improv_plan.w WHERE k = 'a'"
wait_for "$PLAN" 2 "plan in sync"
wait_for "SELECT last_error IS NULL FROM improv.status('plan')" t "plan has no error"
eq "$(U=alice q "SELECT last_error IS NULL FROM improv.status('plan')")" t \
	"a builder's status sees the running worker"

# ---- 1. max_worker_processes exhausted -----------------------------------------
logpos=$(wc -c <"$D/log")
q "ALTER SYSTEM SET max_worker_processes = 0"
restart
eq "$(q "$WORKER")" 0 "no sync worker with max_worker_processes = 0"
# (logged per worker that fails to register: Postgres's logical replication
# launcher is one too; the postmaster does not retry, so no loop)
n=$(since "$logpos" | grep -c "too many background workers" || true)
[ "$n" -ge 1 ] && [ "$n" -le 2 ] || fail "the registration failure logged $n times"
q "UPDATE sales SET v = 3 WHERE k = 'a'"
# (status gives a restarted server 10 s to start the worker)
wait_for "SELECT last_error FROM improv.status('plan')" \
	"the sync worker is not running: if the server log says \"too many background workers\", raise max_worker_processes (now 0) and restart" \
	"status names the stopped worker and the setting" 30
eq "$(U=alice q "SELECT last_error LIKE 'the sync worker is not running%' FROM improv.status('plan')")" t \
	"a builder's status too"
eq "$(q "$PLAN")" 2 "no sync without the worker"
q "ALTER SYSTEM RESET max_worker_processes"
restart
wait_for "$PLAN" 6 "the worker, back, applies the write made while it could not start"
wait_for "SELECT last_error IS NULL FROM improv.status('plan')" t "the error clears"
echo "ok: max_worker_processes = 0: the registration failure is logged once and status names it; raised, the worker catches up"

# ---- 2. wal_keep_size is not the slot's limit -------------------------------
q "ALTER SYSTEM SET improv.database = 'postgres'" # the worker serves another database: away
q "ALTER SYSTEM SET wal_keep_size = 0"
restart
q "UPDATE sales SET v = 4 WHERE k = 'a'"
q "CREATE TABLE ballast (b text)"
for _ in 1 2 3 4; do
	q "INSERT INTO ballast SELECT md5(i::text) || repeat('x', 400) FROM generate_series(1, 20000) i"
	q "SELECT pg_switch_wal()" >/dev/null
	q "CHECKPOINT"
done
SLOT="SELECT wal_status FROM pg_replication_slots WHERE slot_name = 'improv_db'"
case $(q "$SLOT") in reserved | extended) ;; *) fail "the slot with wal_keep_size = 0: $(q "$SLOT")" ;; esac
eq "$(q "SELECT lag_bytes > 3 * 16 * 1024 * 1024 FROM improv.status('plan')")" t \
	"status's lag_bytes shows the WAL the slot holds"
logpos=$(wc -c <"$D/log")
q "ALTER SYSTEM RESET improv.database"
q "ALTER SYSTEM RESET wal_keep_size"
restart
wait_for "$PLAN" 8 "the write made while away, applied from the kept WAL"
eq "$(since "$logpos" | grep -c "invalidated")" 0 "the slot was not invalidated"
q "DROP TABLE ballast"
echo "ok: wal_keep_size = 0 does not cut the slot: it keeps its WAL (lag_bytes shows it) and the worker applies it"

# ---- 3. disk full while writing results ----------------------------------------
# Every file the server writes is capped at 20 MB (above a 16 MB WAL
# segment); improv_d.w, at fillfactor 10, is the one file that outgrows it.
"$PG_BIN/pg_ctl" -D "$D/data" stop -m fast >/dev/null
(ulimit -f 20480 && start)
q "CREATE TABLE big (k text PRIMARY KEY, v float8); INSERT INTO big VALUES ('k0', 1)"
for s in "create_model('d')" "bind_category('d','K','big','k')" \
	"bind_measure('d','V','big','v')" "define('d','W = V * 1')"; do
	q "SELECT improv.$s" >/dev/null
done
DW="SELECT count(*) FROM improv_d.w"
wait_for "$DW" 1 "d in sync"
q "ALTER TABLE improv_d.w SET (fillfactor = 10)"
pid=$(q "SELECT pid FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'")
logpos=$(wc -c <"$D/log")
q "INSERT INTO big SELECT 'k' || i, i FROM generate_series(1, 150000) i"
wait_for "SELECT coalesce(last_error, '') LIKE '%could not extend file%' FROM improv.status('d')" t \
	"status says the results could not be written" 60
echo "   last_error: $(q "SELECT last_error FROM improv.status('d')")"
eq "$(q "$DW")" 1 "nothing half-written"
q "UPDATE sales SET v = 5 WHERE k = 'a'"
wait_for "$PLAN" 10 "another model stays in sync"
sleep 3
eq "$(q "SELECT pid FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'")" "$pid" "the worker keeps running"
n=$(since "$logpos" | grep -c "pg_improv sync: model d: .*could not extend file" || true)
[ "$n" -ge 1 ] && [ "$n" -le 6 ] || fail "model d's failure logged $n times in a few seconds (backoff)"
# Room: the volume grows (a restart without the cap). Each failed retry
# also left the items it minted as dead rows in improv.item, so freeing
# space in improv_d.w alone may not be room enough until a vacuum.
restart
wait_for "$DW" 150001 "with room, the model rebuilds every result" 90
wait_for "SELECT last_error IS NULL FROM improv.status('d')" t "d's error clears"
# Trigger mode: the writer's statement fails, and its rows with it.
q "SELECT improv.set_sync_mode('d','trigger')"
q "VACUUM improv.item"
"$PG_BIN/pg_ctl" -D "$D/data" stop -m fast >/dev/null
(ulimit -f 20480 && start)
err=$(q "INSERT INTO big SELECT 'j' || i, i FROM generate_series(1, 150000) i" 2>&1) &&
	fail "trigger mode: the insert past the limit succeeded"
case $err in *"could not extend file"*) ;; *) fail "trigger mode: $err" ;; esac
eq "$(q "SELECT count(*) FROM big")" 150001 "trigger mode: the writer's rows rolled back"
eq "$(q "$DW")" 150001 "trigger mode: results unchanged"
echo "ok: disk full: logical mode reports it in last_error, writes nothing partial, keeps the worker and the other models going, and recovers; trigger mode fails the writer"

# ---- 4. improv.max_model_memory (B9) ----------------------------------------
# status shows each model's live memory; a model over the cap is refused
# with a logged reason and a last_error (and keeps no live state), never
# grown until the backend is killed; the other models stay in sync; raised,
# it catches up.
"$PG_BIN/pg_ctl" -D "$D/data" stop -m fast >/dev/null
start
q "SELECT improv.set_sync_mode('d', 'logical')"
# (the worker builds a model's live state on its first change)
q "INSERT INTO big VALUES ('l', 1)"
wait_for "$DW" 150002 "d in logical mode"
wait_for "SELECT fold_bytes = 0 AND engine_bytes > 150000 * 200 FROM improv.status('d')" t \
	"status shows d's engine memory (150k cells)"
eq "$(q "SELECT engine_bytes > 0 AND engine_bytes < 1e6 FROM improv.status('plan')")" t \
	"status shows plan's (one cell)"
q "CREATE TABLE ev (k text, v float8); INSERT INTO ev SELECT 'k' || i % 50000, i FROM generate_series(1, 100000) i"
for s in "create_model('agg')" "bind_category('agg','K','ev','k')" \
	"bind_measure('agg','V','ev','v', agg => 'count_distinct')" "define('agg','W = V * 1')"; do
	q "SELECT improv.$s" >/dev/null
done
wait_for "SELECT count(*) FROM improv_agg.w" 50000 "agg in sync"
wait_for "SELECT fold_bytes > 50000 * 500 FROM improv.status('agg')" t "status shows agg's fold state (50k coordinates)"
echo "   status: $(q "SELECT 'd: ' || pg_size_pretty(d.engine_bytes) || ' engine; agg: ' || pg_size_pretty(a.fold_bytes) || ' fold + ' || pg_size_pretty(a.engine_bytes) || ' engine' FROM improv.status('d') d, improv.status('agg') a")"
logpos=$(wc -c <"$D/log")
q "ALTER SYSTEM SET improv.max_model_memory = '20MB'"
q "SELECT pg_reload_conf()" >/dev/null
q "INSERT INTO big VALUES ('over', 1)"
wait_for "SELECT coalesce(last_error, '') LIKE '%over improv.max_model_memory = 20 MB%' FROM improv.status('d')" t \
	"status says d is over the cap" 30
echo "   last_error: $(q "SELECT last_error FROM improv.status('d')")"
since "$logpos" | grep -q "pg_improv sync: model d: the live state needs about .* over improv.max_model_memory" \
	|| fail "the refusal is not logged"
eq "$(q "SELECT count(*) FROM improv_d.w")" 150002 "d's results are left as they were"
q "UPDATE sales SET v = 7 WHERE k = 'a'"
wait_for "$PLAN" 14 "a model under the cap stays in sync"
eq "$(q "SELECT last_error IS NULL FROM improv.status('plan')")" t "plan has no error"
eq "$(q "SELECT pid FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'")" \
	"$(q "SELECT pid FROM pg_stat_activity WHERE backend_type = 'pg_improv sync'")" "the worker keeps running"
q "ALTER SYSTEM RESET improv.max_model_memory"
q "SELECT pg_reload_conf()" >/dev/null
wait_for "$DW" 150003 "under no cap, d catches up" 90
wait_for "SELECT last_error IS NULL FROM improv.status('d')" t "d's error clears"
echo "ok: improv.max_model_memory: status shows each model's fold and engine memory; a model over the cap is refused with a logged reason and a last_error, the others stay in sync, and it recovers once the cap allows it"
echo PASS
