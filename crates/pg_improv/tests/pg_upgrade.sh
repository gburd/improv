#!/usr/bin/env bash
# pg_upgrade of a cluster with Improv models, PG 17 -> 18 (D2). A PG 17
# cluster has a model in each sync mode (logical, trigger, commit, manual)
# and pg_improv installed in both majors. pg_upgrade --check, then the
# upgrade itself. PG 17+ carries the logical slot when it is caught up and
# the new cluster has wal_level = logical. Then every model is checked:
# its results equal a fresh recalc, and each mode still works (the logical
# model follows writes on the carried slot, without a new slot or a
# rebuild; the trigger and commit models recompute in the writer's
# transaction; the manual model refreshes by recalc).
#
#   crates/pg_improv/tests/pg_upgrade.sh
#
# Needs pg_improv installed into both OLD_BIN's and NEW_BIN's Postgres
# (cargo pgrx install --release -c <pg_config>, one per major).
set -euo pipefail
OLD_BIN=${OLD_BIN:-/usr/lib/postgresql/17/bin}
NEW_BIN=${NEW_BIN:-/usr/lib/postgresql/18/bin}
BASE=${PGI_DATA:-$HOME/pgi-pg-upgrade}
P=${PGI_PORT:-55450}
DB=improv_e2e

stop_all() {
	"$OLD_BIN/pg_ctl" -D "$BASE/old" stop -m fast >/dev/null 2>&1 || true
	"$NEW_BIN/pg_ctl" -D "$BASE/new" stop -m fast >/dev/null 2>&1 || true
	if [ -d "$BASE" ]; then find "$BASE" -mindepth 1 -delete && rmdir "$BASE"; fi
}
trap '[ -n "${KEEP:-}" ] || stop_all' EXIT
stop_all
mkdir -p "$BASE/work"

conf() { # datadir
	cat >>"$1/postgresql.conf" <<-EOF
		port = $P
		unix_socket_directories = '$BASE'
		shared_preload_libraries = 'pg_improv'
		improv.database = '$DB'
		improv.poll_ms = 50
		wal_level = logical
		max_replication_slots = 10
		max_wal_senders = 10
	EOF
}
q() { "$BIN/psql" -h "$BASE" -p "$P" -U improv -d $DB -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
fail() { echo "FAIL: $*"; tail -20 "$LOG"; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got '$1', want '$2'"; }
wait_for() { # sql want label
	local v=""
	for _ in $(seq 1 400); do
		v=$(q "$1" 2>/dev/null || true)
		[ "$v" = "$2" ] && return 0
		sleep 0.05
	done
	fail "timed out waiting for $3: got '$v', want '$2'"
}
MODES="logical trigger commit manual"
res() { echo "SELECT coalesce(string_agg(k || '=' || value, ',' ORDER BY k), '') FROM improv_m_$1.t"; }
want() { echo "SELECT coalesce(string_agg(k || '=' || s, ',' ORDER BY k), '') FROM (SELECT k, sum(n) * 2 s FROM f_$1 GROUP BY k) x"; }

# ---- the PG 17 cluster ------------------------------------------------------
BIN=$OLD_BIN LOG=$BASE/old.log
"$OLD_BIN/initdb" -D "$BASE/old" -A trust -U improv >/dev/null
conf "$BASE/old"
"$OLD_BIN/pg_ctl" -D "$BASE/old" -l "$LOG" -w start >/dev/null 3>&-
"$OLD_BIN/createdb" -h "$BASE" -p "$P" -U improv $DB
q "CREATE EXTENSION pg_improv"
for m in $MODES; do
	q "CREATE TABLE f_$m (id int PRIMARY KEY, k text, n float8)"
	q "INSERT INTO f_$m SELECT g, 'k' || g % 4, g FROM generate_series(1, 40) g"
	q "SELECT improv.create_model('m_$m')" >/dev/null
	[ $m = logical ] || q "SELECT improv.set_sync_mode('m_$m', 'manual')" >/dev/null
	q "SELECT improv.bind_category('m_$m','K','f_$m','k')" >/dev/null
	q "SELECT improv.bind_measure('m_$m','N','f_$m','n', agg => 'sum')" >/dev/null
	q "SELECT improv.define('m_$m','T = N * 2')" >/dev/null
	[ $m = logical ] || q "SELECT improv.set_sync_mode('m_$m', '$m')" >/dev/null
	[ $m = manual ] && q "SELECT improv.recalc('m_manual')" >/dev/null
done
for m in $MODES; do wait_for "$(res $m)" "$(q "$(want $m)")" "m_$m on PG 17"; done
q "UPDATE f_logical SET n = n + 1 WHERE id = 1"
wait_for "$(res logical)" "$(q "$(want logical)")" "m_logical follows a write on PG 17"
# pg_upgrade carries a slot only when no transaction follows its confirmed
# position (standby snapshots do not count). The worker confirms each batch
# it applies, and an idle worker writes no WAL, so a poll after the last
# write is applied is enough; pg_upgrade --check verifies it.
sleep 0.5
old_slot=$(q "SELECT confirmed_flush_lsn FROM pg_replication_slots WHERE slot_name = 'improv_db'")
echo "ok: PG 17 cluster with a model in each sync mode (slot at $old_slot)"
"$OLD_BIN/pg_ctl" -D "$BASE/old" stop -m fast >/dev/null

# ---- pg_upgrade to PG 18 ----------------------------------------------------
# PG 18's initdb enables data checksums by default; match the old cluster
checksums=$("$OLD_BIN/pg_controldata" "$BASE/old" | awk '/checksum version/ {print $NF}')
[ "$checksums" = 0 ] && nock=--no-data-checksums || nock=
"$NEW_BIN/initdb" -D "$BASE/new" -A trust -U improv $nock >/dev/null
conf "$BASE/new"
cd "$BASE/work"
"$NEW_BIN/pg_upgrade" -U improv -b "$OLD_BIN" -B "$NEW_BIN" -d "$BASE/old" -D "$BASE/new" \
	-p "$P" -P "$P" -s "$BASE" --check >"$BASE/check.log" 2>&1 || { cat "$BASE/check.log"; exit 1; }
"$NEW_BIN/pg_upgrade" -U improv -b "$OLD_BIN" -B "$NEW_BIN" -d "$BASE/old" -D "$BASE/new" \
	-p "$P" -P "$P" -s "$BASE" >"$BASE/upgrade.log" 2>&1 || { cat "$BASE/upgrade.log"; exit 1; }
cd - >/dev/null
echo "ok: pg_upgrade --check and pg_upgrade 17 -> 18"

# ---- PG 18 ------------------------------------------------------------------
BIN=$NEW_BIN LOG=$BASE/new.log
"$NEW_BIN/pg_ctl" -D "$BASE/new" -l "$LOG" -w start >/dev/null 3>&-
eq "$(q "SELECT current_setting('server_version_num')::int / 10000")" 18 "the new cluster is PG 18"
eq "$(q "SELECT count(*) FROM pg_replication_slots WHERE slot_name = 'improv_db' AND plugin = 'pgoutput'")" 1 \
	"pg_upgrade carried the improv_db slot"
for m in $MODES; do eq "$(q "$(res $m)")" "$(q "$(want $m)")" "m_$m's results after the upgrade"; done
logpos=0
# logical: the worker continues on the carried slot
q "UPDATE f_logical SET n = n + 10 WHERE id = 2"
q "INSERT INTO f_logical VALUES (100, 'k9', 5)"
wait_for "$(res logical)" "$(q "$(want logical)")" "m_logical follows writes on PG 18"
grep -q "created the slot improv_db" "$LOG" && fail "the worker made a new slot (the carried one was not used)"
eq "$(q "SELECT last_error IS NULL FROM improv.status('m_logical')")" t "m_logical's status"
# trigger and commit: results move in the writer's transaction
for m in trigger commit; do
	q "UPDATE f_$m SET n = n + 10 WHERE id = 2"
	eq "$(q "$(res $m)")" "$(q "$(want $m)")" "m_$m recomputes in the writer's transaction on PG 18"
done
# manual: a recalc refreshes it
q "UPDATE f_manual SET n = n + 10 WHERE id = 2"
q "SELECT improv.recalc('m_manual')" >/dev/null
eq "$(q "$(res manual)")" "$(q "$(want manual)")" "m_manual after recalc on PG 18"
eq "$(q "SELECT sum(improv.recalc(name)) FROM improv.model")" 0 "every model equals a fresh recalc"
q "SELECT improv.define('m_logical','U = N * 3')" >/dev/null
wait_for "SELECT count(*) FROM improv_m_logical.u" "$(q "SELECT count(DISTINCT k) FROM f_logical")" "a define after the upgrade"
echo "ok: PG 18: every model equals a fresh recalc; logical continues on the carried slot; trigger, commit and manual work"
echo "PASS"
