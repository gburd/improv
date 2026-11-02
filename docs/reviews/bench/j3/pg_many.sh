#!/usr/bin/env bash
# pg_many.sh: NM small models in one database, logical mode. Worker memory, the
# initial catch-up, and per-model apply latency once all are live.
set -uo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/18/bin}
D=${PGI_DATA:-$HOME/pgi-many}
P=${PGI_PORT:-55441}
NM=${NM:-200}
cleanup() {
	"$PG_BIN/pg_ctl" -D "$D" stop -m fast >/dev/null 2>&1 || true
	if [ -d "$D" ]; then find "$D" -mindepth 1 -delete && rmdir "$D"; fi
}
trap '[ -n "${KEEP:-}" ] || cleanup' EXIT
cleanup
"$PG_BIN/initdb" -D "$D" -A trust -U improv >/dev/null
cat >>"$D/postgresql.conf" <<EOF
port = $P
listen_addresses = ''
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'many'
improv.poll_ms = 50
wal_level = logical
max_replication_slots = $((NM + 10))
max_wal_senders = 10
shared_buffers = '1GB'
max_wal_size = '8GB'
EOF
"$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null 3>&-
PM=$(head -1 "$D/postmaster.pid")
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv many
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d many -qAtX -v ON_ERROR_STOP=1 -c "$1" 2>&1 | grep -v NOTICE; }
now() { date +%s%N; }
ms() { echo $(( ($(now) - $1) / 1000000 )); }
log() { echo "$(date +%T) $*"; }
wpid() { local p="" i; for i in 1 2 3 4 5 6 7 8 9 10; do
	p=$(for x in $(pgrep -f "pg_improv sync"); do [ "$(ps -o ppid= -p $x | tr -d " ")" = "$PM" ] && echo $x; done | head -1)
	[ -n "$p" ] && break; sleep 0.3; done; echo "$p"; }
wrss() { local p; p=$(wpid); [ -n "$p" ] || { echo "no worker"; return; }
	awk '/^(VmRSS|RssAnon|RssShmem):/{printf "%s %d MB  ", $1, $2/1024}' /proc/$p/status; }
q "CREATE EXTENSION pg_improv"
log "worker before: $(wrss)"
t0=$(now)
for i in $(seq 1 "$NM"); do
	q "CREATE TABLE f$i (id int PRIMARY KEY, region text, month int, amount numeric);
	   INSERT INTO f$i SELECT g, 'r' || g % 10, 1 + g % 12, g % 97 FROM generate_series(1, 1000) g;
	   SELECT improv.create_model('m$i');
	   SELECT improv.set_sync_mode('m$i', 'manual');
	   SELECT improv.bind_category('m$i', 'Region', 'f$i', 'region');
	   SELECT improv.bind_category('m$i', 'Month', 'f$i', 'month');
	   SELECT improv.bind_measure('m$i', 'Amount', 'f$i', 'amount', agg => 'sum');
	   SELECT improv.define('m$i', 'Total = SUM(Amount OVER Month)');
	   SELECT improv.define('m$i', 'Share = Amount / Total');" >/dev/null
done
log "created $NM models (manual) in $(ms "$t0") ms; worker: $(wrss)"
t0=$(now)
for i in $(seq 1 "$NM"); do q "SELECT improv.set_sync_mode('m$i', 'logical')" >/dev/null; done
log "set_sync_mode logical x $NM in $(ms "$t0") ms"
# wait for every model's initial build
end=$(( $(date +%s) + 1800 ))
while [ "$(date +%s)" -lt "$end" ]; do
	n=$(q "SELECT count(*) FROM improv.model m, improv.status(m.name) s WHERE s.rebuilds >= 1")
	[ "$n" = "$NM" ] && break
	sleep 0.5
done
log "all $NM models built ($n) in $(ms "$t0") ms; worker: $(wrss)"
q "SELECT count(*) FILTER (WHERE last_error IS NOT NULL) FROM improv.sync_state" | sed 's/^/models with last_error: /'
# apply latency: one-row INSERT into model k, visible
lat=()
for k in 1 50 100 150 $NM 7 77 177; do
	want=$(q "SELECT sum(amount) + 5 FROM f$k WHERE region = 'r1'")
	t0=$(now)
	q "INSERT INTO f$k VALUES (100000, 'r1', 1, 5)"
	end=$(( $(date +%s) + 120 ))
	while [ "$(date +%s)" -lt "$end" ]; do
		v=$(q "SELECT value FROM improv_m$k.total WHERE region = 'r1'")
		[ "$v" = "$want" ] && break
		sleep 0.01
	done
	l=$(ms "$t0"); lat+=("$l")
	log "model m$k: one-row INSERT visible in $l ms (got $v want $want)"
	q "DELETE FROM f$k WHERE id = 100000" >/dev/null
done
# a write touching all models at once (one INSERT each, one transaction)
t0=$(now)
q "BEGIN; $(for i in $(seq 1 "$NM"); do echo "INSERT INTO f$i VALUES (200000, 'r2', 2, 1);"; done) COMMIT;" >/dev/null
end=$(( $(date +%s) + 600 ))
while [ "$(date +%s)" -lt "$end" ]; do
	v=$(q "SELECT value FROM improv_m$NM.total WHERE region = 'r2'")
	w=$(q "SELECT sum(amount) FROM f$NM WHERE region = 'r2'")
	[ "$v" = "$w" ] && break
	sleep 0.02
done
log "one transaction touching all $NM models: last model visible in $(ms "$t0") ms; worker: $(wrss)"
sleep 3
# idle poll cost: the worker's CPU over 10 s with nothing to do
p=$(wpid)
t1=$(awk '{print $14 + $15}' /proc/$p/stat); sleep 10; t2=$(awk '{print $14 + $15}' /proc/$p/stat)
log "idle worker CPU: $(( (t2 - t1) )) ticks in 10 s ($(( (t2 - t1) * 10 ))% of one core / 100)"
log "worker final: $(wrss)"
grep -c "pg_improv sync" "$D/log" | sed 's/^/server log lines mentioning the worker: /'
grep -i "error\|fatal\|panic" "$D/log" | head -5
log DONE
