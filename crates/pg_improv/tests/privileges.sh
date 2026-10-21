#!/usr/bin/env bash
# pg_improv privileges, end to end (needs pg_improv installed into
# $PG_BIN's install):
#   1. a model acts as its owner: with a row-level security policy that lets
#      the owner (alice, not a superuser) see half of a bound table's rows,
#      the results aggregate only that half, in every sync mode (trigger,
#      commit, logical, manual + recalc), whoever writes the table (a
#      superuser, or bob, who has no privilege on schema improv at all)
#   2. revoking alice's SELECT makes the next logical apply fail with a
#      clear last_error instead of reading the table as the worker's
#      superuser; granting it back resumes the model
# Usage: tests/privileges.sh
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=$(mktemp -d /var/tmp/pgi-priv.XXXX)
P=${PGPORT_PRIV:-55490}
stop() {
	[ "${KEEP:-}" ] && { cat "$D/log" >&2; }
	"$PG_BIN/pg_ctl" -D "$D/data" stop -m fast >/dev/null 2>&1 || true
	find "$D" -delete 2>/dev/null || true
}
trap stop EXIT
"$PG_BIN/initdb" -D "$D/data" -A trust -U postgres >/dev/null
cat >"$D/data/postgresql.auto.conf" <<-EOF
	port = $P
	listen_addresses = ''
	unix_socket_directories = '$D'
	shared_preload_libraries = 'pg_improv'
	wal_level = logical
	improv.database = 't'
	improv.poll_ms = 50
EOF
"$PG_BIN/pg_ctl" -D "$D/data" -l "$D/log" -w start >/dev/null 3>&-
"$PG_BIN/createdb" -h "$D" -p "$P" -U postgres t
# q as the superuser; as ROLE (alice, bob) with U=ROLE.
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U "${U:-postgres}" -d t -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
fail() { echo "FAIL: $*"; tail -20 "$D/log"; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got '$1', want '$2'"; }
wait_for() { # sql want what
	local v=""
	for _ in $(seq 200); do v=$(q "$1"); [ "$v" = "$2" ] && return 0; sleep 0.05; done
	fail "$3: got '$v', want '$2'"
}

q "CREATE EXTENSION pg_improv"
q "CREATE ROLE alice LOGIN; CREATE ROLE bob LOGIN; GRANT USAGE ON SCHEMA improv TO alice"
q "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA improv TO alice"
q "GRANT USAGE ON ALL SEQUENCES IN SCHEMA improv TO alice; GRANT CREATE ON DATABASE t TO alice"
# Region north is alice's half: 1 + 2 + 3 = 6 of a total 6 + 40 = 46.
q "CREATE TABLE sales (region text, product text, amount float8, PRIMARY KEY (region, product))"
q "INSERT INTO sales VALUES ('north','a',1), ('north','b',2), ('north','c',3), ('south','a',10), ('south','b',30)"
q "ALTER TABLE sales ENABLE ROW LEVEL SECURITY"
q "CREATE POLICY north ON sales FOR SELECT TO alice USING (region = 'north')"
q "CREATE POLICY bob_writes ON sales TO bob USING (true)"
q "GRANT SELECT, TRIGGER ON sales TO alice; GRANT ALL ON sales TO bob"
for s in "create_model('m')" "bind_category('m','Region','sales','region')" \
	"bind_category('m','Product','sales','product')" "bind_measure('m','Amount','sales','amount')" \
	"define('m','Total = SUM(Amount OVER Region, Product)')"; do
	U=alice q "SELECT improv.$s" >/dev/null
done
eq "$(q "SELECT owner FROM improv.model WHERE name = 'm'")" alice "the owner"
TOTAL="SELECT value FROM improv_m.total"
eq "$(U=alice q "SELECT count(*) FROM sales")" 3 "alice sees half the rows"

# ---- 1. every sync mode aggregates what the owner can see ------------------
U=alice q "SELECT improv.set_sync_mode('m','manual')"
q "SELECT improv.recalc('m')" >/dev/null
eq "$(q "$TOTAL")" 6 "manual: a superuser's recalc computes as the owner"
q "UPDATE sales SET amount = 4 WHERE region = 'north' AND product = 'c'"
eq "$(q "$TOTAL")" 6 "manual: no sync"
U=alice q "SELECT improv.recalc('m')" >/dev/null
eq "$(q "$TOTAL")" 7 "manual: the owner's recalc"

U=alice q "SELECT improv.set_sync_mode('m','trigger')"
eq "$(q "$TOTAL")" 7 "trigger: entering the mode"
q "UPDATE sales SET amount = 50 WHERE region = 'south' AND product = 'a'"
eq "$(q "$TOTAL")" 7 "trigger: a superuser's write to the other half"
U=bob q "UPDATE sales SET amount = 5 WHERE region = 'north' AND product = 'c'"
eq "$(q "$TOTAL")" 8 "trigger: a write by bob, who has no privilege on improv"

U=alice q "SELECT improv.set_sync_mode('m','commit')"
eq "$(q "$TOTAL")" 8 "commit: entering the mode"
U=bob q "BEGIN; UPDATE sales SET amount = 6 WHERE region = 'north' AND product = 'c'; UPDATE sales SET amount = 60 WHERE region = 'south' AND product = 'a'; COMMIT"
eq "$(q "$TOTAL")" 9 "commit: bob's commit"
q "INSERT INTO sales VALUES ('south','c',100)"
eq "$(q "$TOTAL")" 9 "commit: a superuser's commit to the other half"

U=alice q "SELECT improv.set_sync_mode('m','logical')"
wait_for "SELECT last_applied_at IS NOT NULL FROM improv.status('m')" t "logical: caught up"
eq "$(q "$TOTAL")" 9 "logical: the catch-up rebuild"
U=bob q "UPDATE sales SET amount = 7 WHERE region = 'north' AND product = 'c'"
wait_for "$TOTAL" 10 "logical: bob's write"
q "UPDATE sales SET amount = 70 WHERE region = 'south' AND product = 'a'"
q "INSERT INTO sales VALUES ('north','d',1)"
wait_for "$TOTAL" 11 "logical: a superuser's writes to both halves"
eq "$(q "SELECT last_error IS NULL FROM improv.status('m')")" t "logical: no error"
echo "ok: results aggregate the owner's rows in manual, trigger, commit and logical modes"

# ---- 2. a revoked owner fails, not reads as superuser ----------------------
q "REVOKE SELECT ON sales FROM alice"
q "UPDATE sales SET amount = 8 WHERE region = 'north' AND product = 'c'"
wait_for "SELECT last_error IS NOT NULL FROM improv.status('m')" t "logical: the revoked owner's apply fails"
err=$(q "SELECT last_error FROM improv.status('m')")
case $err in *"alice"*"may not read"*) ;; *) fail "last_error does not name the privilege: $err" ;; esac
eq "$(q "$TOTAL")" 11 "logical: nothing applied without the privilege"
echo "   last_error: $err"
q "GRANT SELECT ON sales TO alice"
wait_for "$TOTAL" 12 "logical: the grant back resumes the model"
wait_for "SELECT last_error IS NULL FROM improv.status('m')" t "logical: the error clears"
echo "ok: revoking the owner's SELECT fails the apply with a clear last_error"
echo PASS
