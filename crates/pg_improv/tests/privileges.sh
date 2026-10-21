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
#   3. only a superuser or a member of a role may make it a model's owner:
#      bob may not point his model at a superuser to read a table he
#      cannot, by UPDATE or by INSERT; he may hand it to a role he is a
#      member of; alice may not take a model she does not own; a superuser
#      may set any owner. A pg_restore by the models' owner or a superuser
#      passes; one by a role that may not own them is refused
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
# row_security = off (as pg_dump and pg_restore set it) errors where a policy
# applies, instead of filtering: the owner's recompute pins it on.
q "SET row_security = off; UPDATE sales SET amount = 5 WHERE region = 'north' AND product = 'c'"
eq "$(q "$TOTAL")" 8 "commit: a commit with row_security off"
q "UPDATE sales SET amount = 6 WHERE region = 'north' AND product = 'c'"

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

# ---- 3. who may set a model's owner ------------------------------------------
q "CREATE TABLE secret (k text PRIMARY KEY, v float8); INSERT INTO secret VALUES ('a', 42)"
# team is a role bob belongs to: what team reads, bob reads already.
q "CREATE ROLE team; GRANT team TO bob"
q "GRANT USAGE ON SCHEMA improv TO bob, team; GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA improv TO bob, team"
q "GRANT USAGE ON ALL SEQUENCES IN SCHEMA improv TO bob, team; GRANT CREATE ON DATABASE t TO bob, team"
refused() { # sql what
	local err
	err=$(U=bob q "$1" 2>&1) && fail "$2: allowed"
	case $err in *"only a superuser or a member of role"*"may make it a model's owner"*) ;;
		*) fail "$2: $err" ;; esac
}
U=bob q "SELECT v FROM secret" >/dev/null 2>&1 && fail "bob reads secret"
for s in "create_model('s')" "set_sync_mode('s','manual')" "bind_category('s','K','secret','k')" \
	"bind_measure('s','V','secret','v')" "define('s','W = V * 1')"; do
	U=bob q "SELECT improv.$s" >/dev/null
done
eq "$(q "SELECT owner FROM improv.model WHERE name = 's'")" bob "create_model's owner"
U=bob q "SELECT improv.recalc('s')" >/dev/null 2>&1 && fail "bob's model read secret as bob"
refused "UPDATE improv.model SET owner = 'postgres' WHERE name = 's'" "bob makes a superuser his model's owner"
refused "UPDATE improv.model SET owner = 'alice' WHERE name = 's'" "bob makes alice his model's owner"
refused "INSERT INTO improv.model (name, owner) VALUES ('s2', 'postgres')" "bob inserts a model a superuser owns"
eq "$(q "SELECT owner FROM improv.model WHERE name = 's'")" bob "the owner after the refusals"
U=bob q "SELECT improv.recalc('s')" >/dev/null 2>&1 && fail "secret read after the refusals"
eq "$(q "SELECT count(*) FROM improv_s.w")" 0 "secret never reached bob's results"
# bob may hand the model to team, whose member he is: it reads as team, so
# still not secret; once team may read secret (and so may bob), it does.
U=bob q "UPDATE improv.model SET owner = 'team' WHERE name = 's'"
eq "$(q "SELECT owner FROM improv.model WHERE name = 's'")" team "bob hands his model to his role team"
q "ALTER SCHEMA improv_s OWNER TO team; ALTER TABLE improv_s.w OWNER TO team"
U=bob q "SELECT improv.recalc('s')" >/dev/null 2>&1 && fail "the model read secret as team"
q "GRANT SELECT ON secret TO team"
U=bob q "SELECT improv.recalc('s')" >/dev/null
eq "$(q "SELECT value FROM improv_s.w")" 42 "a model owned by team reads as team"
err=$(U=alice q "UPDATE improv.model SET owner = 'alice' WHERE name = 's'" 2>&1) &&
	fail "alice took team's model"
case $err in *"only a superuser or a member of role team may change the owner of model s"*) ;;
	*) fail "alice takes team's model: $err" ;; esac
eq "$(q "SELECT owner FROM improv.model WHERE name = 's'")" team "the owner after alice's attempt"
q "UPDATE improv.model SET owner = 'alice' WHERE name = 's'"
eq "$(q "SELECT owner FROM improv.model WHERE name = 's'")" alice "a superuser sets any owner"
echo "ok: only a superuser or a member of a role may make it a model's owner (UPDATE and INSERT)"

# A restore re-inserts every improv.model row with the trigger on (pg_restore
# without --disable-triggers). CREATE EXTENSION needs a superuser, so it is
# made first and the catalog restored as data: by the models' owner it loads,
# by a role that may not own them it is refused, by a superuser it loads.
q "UPDATE improv.model SET owner = 'bob' WHERE name = 's'"
q "SELECT improv.drop_model('m')" >/dev/null
"$PG_BIN/pg_dump" -h "$D" -p "$P" -U postgres --data-only -t 'improv.*' t >"$D/cat.sql"
grep -q "^COPY improv.model " "$D/cat.sql" || fail "the catalog dump has no models"
restore() { # role db
	"$PG_BIN/psql" -h "$D" -p "$P" -U "$1" -d "$2" -qX -v ON_ERROR_STOP=1 -f "$D/cat.sql" >/dev/null 2>"$D/$2.err"
}
models() { "$PG_BIN/psql" -h "$D" -p "$P" -U postgres -d "$1" -qAtX -c "SELECT string_agg(name || ':' || owner, ',') FROM improv.model"; }
for r in bob alice postgres; do
	"$PG_BIN/createdb" -h "$D" -p "$P" -U postgres "r_$r"
	# (the catalog names bound tables by regclass: the pre-data comes first)
	"$PG_BIN/psql" -h "$D" -p "$P" -U postgres -d "r_$r" -qAtX -v ON_ERROR_STOP=1 -c "CREATE EXTENSION pg_improv" \
		-c "CREATE TABLE secret (k text PRIMARY KEY, v float8)" \
		-c "GRANT USAGE ON SCHEMA improv TO $r" \
		-c "GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA improv TO $r" \
		-c "GRANT USAGE, UPDATE ON ALL SEQUENCES IN SCHEMA improv TO $r"
done
restore bob r_bob || fail "the owner's restore: $(cat "$D/r_bob.err")"
eq "$(models r_bob)" s:bob "restored by its owner"
restore alice r_alice && fail "alice restored a model she may not own"
grep -q "only a superuser or a member of role bob may make it a model's owner" "$D/r_alice.err" ||
	fail "alice's restore: $(cat "$D/r_alice.err")"
eq "$(models r_alice)" "" "nothing restored by alice"
restore postgres r_postgres || fail "a superuser's restore: $(cat "$D/r_postgres.err")"
eq "$(models r_postgres)" s:bob "restored by a superuser"
echo "ok: a restore by the models' owner or a superuser loads them; one by a role that may not own them is refused"
echo PASS
