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
#      may set any owner
#   4. only those who may act as a model's owner change the model: bob may
#      not bind a table into alice's model, redefine a result she shared
#      with him, recalc it, or change its catalog rows by hand (row-level
#      security on improv.*), nor read its items or input cells, nor
#      what_if it; alice and a member of alice with SET may; a SECURITY
#      DEFINER wrapper alice owns gives bob what_if on her model and nothing
#      else; a writer who is not the owner still drives trigger, commit and
#      logical sync
#   5. a restore of the catalog by the models' owner or a superuser loads
#      it; one by a role that may not own them is refused; DROP EXTENSION
#      keeps the roles, and a new CREATE EXTENSION uses them
#   6. improv.models: every model, with may_change for the caller
#   7. the README's privilege matrix, cell by cell: each action on one model
#      as a superuser, an improv_admin, the owner, a member of the owner with
#      SET, an improv_user who is not the owner, a writer of a bound table,
#      and a role with no grant (PUBLIC); improv_admin enrolls builders
# The builders are members of improv_user (made by CREATE EXTENSION), not
# granted the catalog by hand; they have no CREATE on the database.
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
q "CREATE ROLE alice LOGIN; CREATE ROLE bob LOGIN; GRANT improv_user TO alice"
eq "$(q "SELECT has_database_privilege('alice', 't', 'CREATE')")" f "alice has no CREATE on the database"
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
q "GRANT improv_user TO bob"
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
# alice may not take team's model: row-level security hides its row from
# her UPDATE (0 rows), and the trigger would refuse it if it did not.
eq "$(U=alice q "WITH u AS (UPDATE improv.model SET owner = 'alice' WHERE name = 's' RETURNING 1) SELECT count(*) FROM u")" 0 \
	"alice takes team's model"
eq "$(q "SELECT owner FROM improv.model WHERE name = 's'")" team "the owner after alice's attempt"
q "UPDATE improv.model SET owner = 'alice' WHERE name = 's'"
eq "$(q "SELECT owner FROM improv.model WHERE name = 's'")" alice "a superuser sets any owner"
echo "ok: only a superuser or a member of a role may make it a model's owner (UPDATE and INSERT)"

# ---- 4. only those who may act as the owner change a model ----------------
# alice owns model p over pub, and shares its result with bob. priv is a
# table alice reads and bob does not: bob must not get it through p.
q "CREATE TABLE pub (k text PRIMARY KEY, v float8); INSERT INTO pub VALUES ('a', 1)"
q "CREATE TABLE priv (k text PRIMARY KEY, v float8); INSERT INTO priv VALUES ('a', 42)"
q "GRANT SELECT, TRIGGER ON pub, priv TO alice; GRANT ALL ON pub TO bob"
for s in "create_model('p')" "set_sync_mode('p','manual')" "bind_category('p','K','pub','k')" \
	"bind_measure('p','V','pub','v')" "define('p','W = V * 1')" "recalc('p')"; do
	U=alice q "SELECT improv.$s" >/dev/null
done
U=alice q "GRANT USAGE ON SCHEMA improv_p TO bob; GRANT SELECT ON improv_p.w TO bob"
W="SELECT string_agg(k || '=' || value, ',' ORDER BY k) FROM improv_p.w"
eq "$(U=bob q "$W")" a=1 "bob reads alice's shared result"
denied() { # role sql what
	local err
	err=$(U=$1 q "$2" 2>&1) && fail "$3: allowed"
	case $err in *"permission denied for improv model \"p\""*|*"row-level security"*|*"0 rows"*) ;;
		*) fail "$3: $err" ;; esac
}
for s in "bind_category('p','K','priv','k')" "bind_measure('p','S','priv','v')" \
	"define('p','W = V * 100', replace => true)" "recalc('p')" "set_sync_mode('p','trigger')" \
	"drop_measure('p','W')" "drop_model('p')"; do
	denied bob "SELECT improv.$s" "bob: $s on alice's model"
done
# By hand: RLS hides alice's rows from bob's UPDATE and DELETE (0 rows) and
# refuses what he inserts.
for t in "INSERT INTO improv.category_binding SELECT model_id, category_id, 'priv'::regclass, col FROM improv.category_binding WHERE model_id = (SELECT id FROM improv.model WHERE name = 'p')" \
	"INSERT INTO improv.cell (model_id, measure_id, coord, value) SELECT id, 1, '{}', '1' FROM improv.model WHERE name = 'p'"; do
	denied bob "$t" "bob inserts into alice's catalog"
done
eq "$(U=bob q "WITH u AS (UPDATE improv.measure SET formula = 'V * 100' WHERE model_id = (SELECT id FROM improv.model WHERE name = 'p') RETURNING 1) SELECT count(*) FROM u")" 0 "bob's UPDATE of alice's formulas"
eq "$(U=bob q "WITH u AS (UPDATE improv.binding SET rel = 'priv' WHERE model_id = (SELECT id FROM improv.model WHERE name = 'p') RETURNING 1) SELECT count(*) FROM u")" 0 "bob's UPDATE of alice's bindings"
eq "$(U=bob q "WITH u AS (DELETE FROM improv.measure WHERE model_id = (SELECT id FROM improv.model WHERE name = 'p') RETURNING 1) SELECT count(*) FROM u")" 0 "bob's DELETE of alice's measures"
eq "$(U=bob q "WITH u AS (UPDATE improv.model SET sync_mode = 'trigger' WHERE name = 'p' RETURNING 1) SELECT count(*) FROM u")" 0 "bob's UPDATE of alice's model row"
q "SELECT improv.recalc('p')" >/dev/null
eq "$(U=bob q "$W")" a=1 "priv never reached alice's shared result"
eq "$(q "SELECT count(*) FROM improv.binding b JOIN improv.model m ON m.id = b.model_id WHERE m.name = 'p' AND b.rel = 'priv'::regclass")" 0 "no binding of priv"
# The catalog's names and formulas stay readable (status reads them); input
# cells are data, readable only by those who may act as the owner.
eq "$(U=bob q "SELECT formula FROM improv.measure m JOIN improv.model o ON o.id = m.model_id WHERE o.name = 'p' AND m.name = 'W'")" "W = V * 1" "bob reads alice's formulas"
eq "$(U=bob q "SELECT sync_mode FROM improv.status('p')")" manual "bob reads alice's model status"
# The GUI's Postgres store (improv CLI and improv-gui postgres://…?model=)
# refuses a model its role may not act as, rather than load it empty.
if [ -n "${IMPROV:-}" ]; then
	url="postgres:///t?host=$D&port=$P&user=bob&model=p"
	err=$("$IMPROV" list "$url" 2>&1) && fail "bob opened alice's model through the store"
	case $err in *"permission denied for improv model \"p\""*) ;; *) fail "the store, as bob: $err" ;; esac
	"$IMPROV" list "postgres:///t?host=$D&port=$P&user=alice&model=p" | grep -q "W" ||
		fail "alice opens her model through the store"
	echo "ok: the Postgres model store opens a model only for those who may act as its owner"
fi
q "INSERT INTO improv.cell (model_id, measure_id, coord, value) SELECT id, 1, '{}', '7' FROM improv.model WHERE name = 'p'"
CELLS="SELECT count(*) FROM improv.cell c JOIN improv.model m ON m.id = c.model_id WHERE m.name = 'p'"
ITEMS="SELECT count(*) FROM improv.item i JOIN improv.model m ON m.id = i.model_id WHERE m.name = 'p'"
eq "$(U=alice q "$CELLS")" 1 "alice reads her cells"
eq "$(U=bob q "$CELLS")" 0 "bob does not read alice's cells"
q "DELETE FROM improv.cell WHERE model_id = (SELECT id FROM improv.model WHERE name = 'p')"
# Items are the bound tables' keys: data too.
eq "$(U=alice q "$ITEMS")" 1 "alice reads her items"
eq "$(U=bob q "$ITEMS")" 0 "bob does not read alice's items"
# Nor learns them through the minter: its insert is refused before any conflict check.
denied bob "SELECT * FROM improv.mint_items((SELECT id FROM improv.model WHERE name = 'p'), ARRAY[1]::int8[], ARRAY['a'])" \
	"bob mints into alice's model"
# what_if evaluates the owner's data: only those who may act as the owner.
WI="SELECT string_agg(measure || ' ' || coord::text || ' ' || value::text, '; ' ORDER BY measure) FROM"
EDIT="'[{\"measure\": \"V\", \"coord\": {\"K\": \"a\"}, \"value\": 5}]'"
eq "$(U=alice q "$WI improv.what_if('p', $EDIT)")" 'W {"K": "a"} 5' "alice's what_if"
err=$(U=bob q "$WI improv.what_if('p', $EDIT)" 2>&1) && fail "bob's what_if on alice's model: allowed"
case $err in *'permission denied for improv model "p": what_if needs to act as its owner, alice (see Privileges)'*) ;;
	*) fail "bob's what_if on alice's model: $err" ;; esac
echo "ok: bob may not bind, define, recalc, re-mode or drop alice's model, nor change its catalog rows by hand, read its items or cells, or what_if it"
# alice herself, and carol, a member of alice (with SET), change it.
U=alice q "SELECT improv.bind_category('p','K','priv','k')"
U=alice q "SELECT improv.bind_measure('p','S','priv','v')"
U=alice q "SELECT improv.define('p','T = S * 1')" >/dev/null
U=alice q "SELECT improv.recalc('p')" >/dev/null
eq "$(q "SELECT value FROM improv_p.t")" 42 "alice binds a table she reads into her model"
PG16=$(q "SELECT current_setting('server_version_num')::int >= 160000")
# carol is granted alice as Privileges' option (a) says: SET without INHERIT
# on PG 16+, so she reads nothing of alice's herself.
if [ "$PG16" = t ]; then q "CREATE ROLE carol LOGIN; GRANT alice TO carol WITH INHERIT FALSE, SET TRUE"
else q "CREATE ROLE carol LOGIN; GRANT alice TO carol"; fi
q "GRANT improv_user TO carol"
[ "$PG16" = t ] && { U=carol q "SELECT count(*) FROM pub" >/dev/null 2>&1 && fail "carol (INHERIT FALSE) reads pub"; }
eq "$(U=carol q "$WI improv.what_if('p', $EDIT)")" 'W {"K": "a"} 5' "carol's what_if, as alice"
U=carol q "SELECT improv.define('p','U = V * 2')" >/dev/null
U=carol q "SELECT improv.recalc('p')" >/dev/null
eq "$(q "SELECT value FROM improv_p.u")" 2 "carol, a member of alice, changes alice's model"
if [ "$PG16" = t ]; then
	# A member without SET may not act as alice (PG 16+, as ALTER ... OWNER TO).
	q "CREATE ROLE dave LOGIN; GRANT alice TO dave WITH SET FALSE, INHERIT FALSE; GRANT improv_user TO dave"
	denied dave "SELECT improv.define('p','X = V * 3')" "dave, a member of alice without SET"
	U=dave q "$WI improv.what_if('p', $EDIT)" >/dev/null 2>&1 && fail "dave, a member of alice without SET, runs what_if"
	echo "ok: alice and carol (a member of alice with SET) change her model and what_if it; dave (a member without SET) may not"
else
	echo "ok: alice and carol (a member of alice) change her model and what_if it"
fi
# Privileges' option (b): alice wraps what_if on p, and grants EXECUTE on it.
# (The README's function text, for model p and analyst bob.)
# (the wrapper's own schema: building models needs no CREATE on the database)
q "GRANT CREATE ON DATABASE t TO alice"
U=alice q "CREATE SCHEMA alice_api"
U=alice q "CREATE FUNCTION alice_api.what_if_p(edits jsonb)
RETURNS TABLE (measure text, coord jsonb, value jsonb)
LANGUAGE sql SECURITY DEFINER SET search_path = pg_catalog, pg_temp
AS \$\$ SELECT w.measure, w.coord, w.value FROM improv.what_if('p', edits) AS w \$\$;
REVOKE ALL ON FUNCTION alice_api.what_if_p(jsonb) FROM PUBLIC;
GRANT USAGE ON SCHEMA alice_api TO bob;
GRANT EXECUTE ON FUNCTION alice_api.what_if_p(jsonb) TO bob"
eq "$(U=bob q "$WI alice_api.what_if_p($EDIT)")" 'U {"K": "a"} 10; W {"K": "a"} 5' "bob's what_if through alice's wrapper"
eq "$(U=bob q "$ITEMS")" 0 "bob still does not read alice's items"
eq "$(U=bob q "$CELLS")" 0 "bob still does not read alice's cells"
U=bob q "$WI improv.what_if('p', $EDIT)" >/dev/null 2>&1 && fail "bob's what_if on alice's model, past the wrapper"
# erin has EXECUTE on the wrapper and nothing on schema improv at all.
q "CREATE ROLE erin LOGIN"
U=alice q "GRANT USAGE ON SCHEMA alice_api TO erin; GRANT EXECUTE ON FUNCTION alice_api.what_if_p(jsonb) TO erin"
eq "$(U=erin q "$WI alice_api.what_if_p($EDIT)")" 'U {"K": "a"} 10; W {"K": "a"} 5' "erin's what_if through alice's wrapper"
U=erin q "SELECT count(*) FROM improv.item" >/dev/null 2>&1 && fail "erin reads improv.item"
U=alice q "REVOKE EXECUTE ON FUNCTION alice_api.what_if_p(jsonb) FROM bob"
U=bob q "$WI alice_api.what_if_p($EDIT)" >/dev/null 2>&1 && fail "bob's what_if through the wrapper after REVOKE"
echo "ok: a SECURITY DEFINER wrapper alice owns gives bob and erin what_if on her model p, and nothing else"
# A writer who is not the owner (bob) still drives every sync mode: the
# trigger and the commit hook read the catalog as the bootstrap superuser,
# the worker is one, and the model reads pub as alice.
TOT="SELECT value FROM improv_p.w WHERE k = 'a'"
U=alice q "SELECT improv.set_sync_mode('p','trigger')"
U=bob q "UPDATE pub SET v = 2 WHERE k = 'a'"
eq "$(q "$TOT")" 2 "trigger: bob's write"
U=alice q "SELECT improv.set_sync_mode('p','commit')"
U=bob q "UPDATE pub SET v = 3 WHERE k = 'a'"
eq "$(q "$TOT")" 3 "commit: bob's write"
U=alice q "SELECT improv.set_sync_mode('p','logical')"
U=bob q "UPDATE pub SET v = 4 WHERE k = 'a'"
wait_for "$TOT" 4 "logical: bob's write"
eq "$(q "SELECT last_error IS NULL FROM improv.status('p')")" t "logical: no error"
echo "ok: bob's writes to alice's bound table drive trigger, commit and logical sync"
U=alice q "SELECT improv.drop_model('p')"

# ---- 5. a restore ------------------------------------------------------------
# A restore re-inserts every improv.model row with the trigger on (pg_restore
# without --disable-triggers). CREATE EXTENSION needs a superuser, so it is
# made first and the catalog restored as data: by the models' owner it loads,
# by a role that may not own them it is refused, by a superuser it loads.
q "UPDATE improv.model SET owner = 'bob' WHERE name = 's'"
q "SELECT improv.drop_model('m')" >/dev/null
# A superuser restores a plain dump (row_security = off, COPY); a role under
# the catalog's row-level security needs one made with --enable-row-security
# --inserts (COPY FROM does not support row-level security).
"$PG_BIN/pg_dump" -h "$D" -p "$P" -U postgres --data-only -t 'improv.*' t >"$D/cat-postgres.sql"
grep -q "^COPY improv.model " "$D/cat-postgres.sql" || fail "the catalog dump has no models"
"$PG_BIN/pg_dump" -h "$D" -p "$P" -U postgres --data-only -t 'improv.*' --enable-row-security --inserts t >"$D/cat-rls.sql"
cp "$D/cat-rls.sql" "$D/cat-bob.sql"; cp "$D/cat-rls.sql" "$D/cat-alice.sql"
restore() { # role db
	"$PG_BIN/psql" -h "$D" -p "$P" -U "$1" -d "$2" -qX -v ON_ERROR_STOP=1 -f "$D/cat-$1.sql" >/dev/null 2>"$D/$2.err"
}
models() { "$PG_BIN/psql" -h "$D" -p "$P" -U postgres -d "$1" -qAtX -c "SELECT string_agg(name || ':' || owner, ',') FROM improv.model"; }
for r in bob alice postgres; do
	"$PG_BIN/createdb" -h "$D" -p "$P" -U postgres "r_$r"
	# (the catalog names bound tables by regclass: the pre-data comes first)
	"$PG_BIN/psql" -h "$D" -p "$P" -U postgres -d "r_$r" -qAtX -v ON_ERROR_STOP=1 -c "CREATE EXTENSION pg_improv" \
		-c "CREATE TABLE secret (k text PRIMARY KEY, v float8)" \
		-c "GRANT UPDATE ON SEQUENCE improv.model_id_seq TO $r" # the dump's setval
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
# Roles are the cluster's: DROP EXTENSION keeps them (and their members),
# and CREATE EXTENSION again grants improv_user the new catalog.
qr() { "$PG_BIN/psql" -h "$D" -p "$P" -U "${U:-postgres}" -d r_postgres -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
qr "DROP EXTENSION pg_improv CASCADE"
eq "$(qr "SELECT string_agg(rolname, ',' ORDER BY rolname) FROM pg_roles WHERE rolname LIKE 'improv\_%'")" improv_admin,improv_user \
	"DROP EXTENSION keeps the roles"
qr "CREATE EXTENSION pg_improv"
U=bob qr "SELECT improv.create_model('again')" >/dev/null
eq "$(qr "SELECT owner FROM improv.model WHERE name = 'again'")" bob "an improv_user builds after DROP and CREATE EXTENSION"
echo "ok: DROP EXTENSION keeps improv_user and improv_admin; CREATE EXTENSION again grants improv_user its catalog"

# ---- 6. improv.models ----------------------------------------------------------
# Every model, and whether the caller may act on it; read access follows
# improv.model's (a role with no grant on improv reads nothing).
q "CREATE ROLE adm LOGIN; GRANT improv_admin TO adm; CREATE ROLE nob LOGIN"
for s in "create_model('x')" "set_sync_mode('x','manual')"; do U=alice q "SELECT improv.$s" >/dev/null; done
M="SELECT string_agg(name || ':' || owner || ':' || sync_mode || ':' || may_change || ':' || result_schema, ',' ORDER BY name) FROM improv.models WHERE name IN ('x', 's')"
eq "$(q "$M")" "s:bob:manual:true:improv_s,x:alice:manual:true:improv_x" "improv.models as a superuser"
eq "$(U=adm q "$M")" "s:bob:manual:false:improv_s,x:alice:manual:false:improv_x" "improv.models as improv_admin"
eq "$(U=alice q "$M")" "s:bob:manual:false:improv_s,x:alice:manual:true:improv_x" "improv.models as the owner"
eq "$(U=carol q "$M")" "s:bob:manual:false:improv_s,x:alice:manual:true:improv_x" "improv.models as a member of the owner"
eq "$(U=bob q "$M")" "s:bob:manual:true:improv_s,x:alice:manual:false:improv_x" "improv.models as bob"
U=nob q "SELECT count(*) FROM improv.models" >/dev/null 2>&1 && fail "a role with no grant reads improv.models"
echo "ok: improv.models lists every model and whether the caller may change it"

# ---- 7. the privilege matrix ---------------------------------------------------
# Model x (section 6), alice's, now over xs, in manual mode. Each action runs as each role:
#   postgres  a superuser
#   adm       a member of improv_admin
#   alice     the owner
#   carol     a member of alice with SET (INHERIT FALSE on PG 16+), an improv_user
#   bob       an improv_user, not the owner
#   wri       may write xs, no grant on improv
#   nob       no grant at all: what PUBLIC may do
# y = allowed; n = refused ("permission denied ...") or, for a read of the
# catalog, no rows (row-level security).
q "CREATE ROLE wri LOGIN"
q "CREATE TABLE xs (k text PRIMARY KEY, v float8); INSERT INTO xs VALUES ('a', 1)"
q "GRANT SELECT ON xs TO alice; GRANT SELECT, INSERT, UPDATE, DELETE ON xs TO wri"
for s in "bind_category('x','K','xs','k')" "bind_measure('x','V','xs','v')" "define('x','W = V * 1')" "recalc('x')"; do
	U=alice q "SELECT improv.$s" >/dev/null
done
ROLES="postgres adm alice carol bob wri nob"
act() { # action role
	case $1 in
	create_model) echo "SELECT improv.create_model('c_$2')" ;;
	bind) echo "SELECT improv.bind_measure('x','B_$2','xs','v')" ;;
	define) echo "SELECT improv.define('x','D_$2 = V * 2')" ;;
	recalc) echo "SELECT improv.recalc('x')" ;;
	what_if) echo "SELECT count(*) FROM improv.what_if('x', $EDIT)" ;;
	set_sync_mode) echo "SELECT improv.set_sync_mode('x','manual')" ;;
	drop) echo "SELECT improv.drop_model('z_$2')" ;;
	names) echo "SELECT count(*) FROM improv.measure m JOIN improv.model o ON o.id = m.model_id WHERE o.name = 'x'" ;;
	items) echo "SELECT count(*) FROM improv.item i JOIN improv.model o ON o.id = i.model_id WHERE o.name = 'x'" ;;
	results) echo "SELECT count(*) FROM improv_x.w" ;;
	status) echo "SELECT sync_mode FROM improv.status('x')" ;;
	esac
}
try() { # action role -> y or n
	local out
	if ! out=$(U=$2 q "$(act "$1" "$2")" 2>&1); then
		case $out in *"permission denied"*|*"row-level security"*) echo n; return ;; esac
		fail "$1 as $2: $out"
	fi
	case $1 in names|items) [ "$out" -gt 0 ] && echo y || echo n ;; *) echo y ;; esac
}
# Results: carol inherits nothing of alice on PG 16+ (she reads them after SET ROLE alice).
if [ "$PG16" = t ]; then CAROL_READS=n; else CAROL_READS=y; fi
#                  postgres adm alice carol bob wri nob
want_create_model="y y y y y n n"
want_bind="y n y y n n n"
want_define="y n y y n n n"
want_recalc="y n y y n n n"
want_what_if="y n y y n n n"
want_set_sync_mode="y n y y n n n"
want_drop="y n y y n n n"
want_names="y y y y y n n"
want_items="y n y y n n n"
want_results="y n y $CAROL_READS n n n"
want_status="y y y y y n n"
for a in create_model bind define recalc what_if set_sync_mode drop names items results status; do
	got=""
	for r in $ROLES; do
		[ "$a" = drop ] && U=alice q "SELECT improv.create_model('z_$r')" >/dev/null
		got="$got $(try $a $r)"
		[ "$a" = drop ] && q "SELECT improv.drop_model(name) FROM improv.model WHERE name = 'z_$r'" >/dev/null
	done
	want=want_$a
	eq "${got# }" "${!want}" "the matrix row $a ($ROLES)"
	printf '   %-14s %s\n' "$a" "${got# }"
done
q "SELECT improv.drop_model(name) FROM improv.model WHERE name LIKE 'c\_%'" >/dev/null
eq "$(U=carol q "SET ROLE alice; SELECT count(*) FROM improv_x.w")" 1 "carol reads the results after SET ROLE alice"
U=alice q "GRANT USAGE ON SCHEMA improv_x TO bob; GRANT SELECT ON improv_x.w TO bob"
eq "$(U=bob q "SELECT count(*) FROM improv_x.w")" 1 "bob reads the results once alice grants them"
# improv_admin enrolls builders; an improv_user may not.
q "CREATE ROLE newb LOGIN"
U=bob q "GRANT improv_user TO newb" >/dev/null 2>&1 && fail "bob enrolled a builder"
U=adm q "GRANT improv_user TO newb"
U=newb q "SELECT improv.create_model('n1')" >/dev/null
U=adm q "REVOKE improv_user FROM newb"
U=newb q "SELECT improv.create_model('n2')" >/dev/null 2>&1 && fail "newb builds after the REVOKE"
echo "ok: the privilege matrix, cell by cell; improv_admin enrolls builders"
echo PASS
