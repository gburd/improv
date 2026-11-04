#!/usr/bin/env bash
# pg_improv access policies (Cedar), end to end (needs pg_improv installed
# into $PG_BIN's install):
#   1. without policies a model is as in 0.12: the filtered functions are
#      the owner's, the result tables follow their grants
#   2. set_policies validates (a bad document, a Cell literal, a non-Admin
#      setter are refused), stores and audits; only those who may act as
#      the owner, or whom the policies allow Admin, read them
#   3. a restricted role sees only what its policy allows through every
#      read path: psql on the result tables (row-level security, in every
#      sync mode, after recomputes and on new measures), the filtered
#      functions (categories, items, measures, cells), and the catalog
#      (improv.measure and the like show it nothing); totals stay
#   4. hidden is missing: a hidden measure's and item's errors are
#      byte-for-byte those of ones that do not exist; a hidden measure
#      cannot be read through a new definition
#   5. edits and defines: allowed ones work (as the owner, recomputed) and
#      are audited; denied ones are refused, write nothing, and are logged
#   6. the owner is never locked out: a forbid-all document leaves the
#      owner (and a superuser) everything; an Admin may grant Admin but not
#      take the owner's
#   7. a change of the policies, the groups or a role's memberships in one
#      session applies from the next statement of another
#   8. explain_access names the deciding policies for an admin, says only
#      allow or deny to the role itself
#   9. clearing the policies restores 0.12's behavior
#  10. pg_dump keeps the policies; a restore enforces them
#  11. the result tables' qual decides once per statement (InitPlans), and
#      checks rows only when it must
# Usage: tests/policies.sh
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=$(mktemp -d /var/tmp/pgi-pol.XXXX)
P=${PGPORT_POL:-55495}
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
	track_functions = 'all'
EOF
"$PG_BIN/pg_ctl" -D "$D/data" -l "$D/log" -w start >/dev/null 3>&-
"$PG_BIN/createdb" -h "$D" -p "$P" -U postgres t
# q as the superuser; as ROLE with U=ROLE. e: the error text of a statement that must fail.
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U "${U:-postgres}" -d "${DB:-t}" -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
e() { # the whole message, its lines joined by spaces
	local out
	if out=$(q "$1" 2>&1); then echo "UNEXPECTED SUCCESS: $out"; return; fi
	awk '/^ERROR:  /{sub(/^ERROR:  /, ""); p=1} /^(CONTEXT|DETAIL|HINT):|^LINE [0-9]+:/{p=0} p' <<<"$out" | paste -sd' '
}
fail() { echo "FAIL: $*"; tail -20 "$D/log"; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got '$1', want '$2'"; }
has() { case $1 in *"$2"*) ;; *) fail "$3: '$1' lacks '$2'" ;; esac; }
lacks() { case $1 in *"$2"*) fail "$3: '$1' has '$2'" ;; esac; }
wait_for() { # sql want what
	local v=""
	for _ in $(seq 200); do v=$(q "$1"); [ "$v" = "$2" ] && return 0; sleep 0.05; done
	fail "$3: got '$v', want '$2'"
}

q "CREATE EXTENSION pg_improv"
# alice owns the model; ann (role staff) and ed are analysts, builders of
# their own models (improv_user); nob has no policy; carol may act as alice.
# Calling the functions takes only USAGE on improv (nob, carol).
q "CREATE ROLE alice LOGIN; CREATE ROLE ann LOGIN; CREATE ROLE ed LOGIN; CREATE ROLE nob LOGIN;
   CREATE ROLE carol LOGIN; CREATE ROLE staff; CREATE ROLE fin;
   GRANT improv_user TO alice, ann, ed; GRANT staff TO ann; GRANT alice TO carol;
   GRANT USAGE ON SCHEMA improv TO nob, carol"
q "CREATE TABLE sales (quarter text, product text, price float8, qty float8, PRIMARY KEY (quarter, product))"
q "INSERT INTO sales VALUES ('Q1','Widget',10,100), ('Q1','Gadget',20,30), ('Q1','Falcon',50,2),
   ('Q2','Widget',10,200), ('Q2','Gadget',20,60), ('Q2','Falcon',50,4)"
q "GRANT SELECT, INSERT, UPDATE, TRIGGER ON sales TO alice"
for s in "create_model('m')" "bind_category('m','Time','sales','quarter')" \
	"bind_category('m','Product','sales','product')" "bind_measure('m','Price','sales','price')" \
	"bind_measure('m','Quantity','sales','qty')" "define('m','Revenue = Price * Quantity')" \
	"define('m','Total = SUM(Revenue OVER Time)')" "define('m','Grand = SUM(Revenue OVER Time, Product)')" \
	"set_sync_mode('m','trigger')"; do
	U=alice q "SELECT improv.$s" >/dev/null
done
# The results are granted, as an owner shares them today.
U=alice q "GRANT USAGE ON SCHEMA improv_m TO ann, ed, nob; GRANT SELECT ON ALL TABLES IN SCHEMA improv_m TO ann, ed, nob"
idof() { U=alice q "SELECT id FROM improv.$1 WHERE name = '$2'"; }
PRICE=$(idof measure Price) FALCON=$(idof item Falcon)
ROWS="SELECT string_agg(time || '/' || product || '=' || value, ' ' ORDER BY time, product) FROM improv_m.revenue"

# ---- 1. no policies: 0.12 ----------------------------------------------------
eq "$(U=ann q "$ROWS")" "Q1/Falcon=100 Q1/Gadget=600 Q1/Widget=1000 Q2/Falcon=200 Q2/Gadget=1200 Q2/Widget=2000" \
	"without policies a grantee reads every result row"
eq "$(U=ann e "SELECT * FROM improv.measures('m')")" \
	'permission denied for improv model "m": it has no access policies, so only its owner, alice, or a role that may act as it may use it' \
	"without policies the filtered functions are the owner's"
eq "$(U=alice q "SELECT count(*) FROM improv.measures('m')")" 5 "the owner's measures"
eq "$(U=carol q "SELECT count(*) FROM improv.items('m')")" 5 "a member of the owner's items"
eq "$(U=alice q "SELECT count(*) FROM improv.policies('m')")" 0 "no policies yet"
echo "ok: without policies the model is as in 0.12"

# ---- 2. set_policies ----------------------------------------------------------
POL="@id(\"staff\") permit (principal in Improv::Role::\"staff\", action in Improv::Action::\"Read\", resource);
@id(\"no-falcon\") forbid (principal == Improv::User::\"ann\", action == Improv::Action::\"ReadItem\", resource == Improv::Item::\"$FALCON\");
@id(\"ed\") permit (principal == Improv::User::\"ed\", action, resource) unless { action == Improv::Action::\"Admin\" };
@id(\"ed-no-price\") forbid (principal == Improv::User::\"ed\", action in Improv::Action::\"Read\", resource in Improv::Measure::\"$PRICE\");"
eq "$(U=ann e "SELECT improv.set_policies('m', \$\$$POL\$\$)")" \
	'permission denied for improv model "m": only its owner, alice, a role that may act as it, or a role its policies allow Admin may set its policies' \
	"a role that may not act as the owner may not set policies"
has "$(U=alice e "SELECT improv.set_policies('m', 'permit (principal, action, resource')")" "are not valid" "a syntax error is refused"
has "$(U=alice e "SELECT improv.set_policies('m', 'permit (principal, action, resource == Improv::Cell::\"1:2\");')")" \
	"names an Improv::Cell" "a Cell literal is refused"
has "$(U=alice e "SELECT improv.set_policies('m', 'permit (principal, action, resource);', '{\"g\": {\"groups\": [\"h\"]}}')")" \
	"which is not defined" "bad groups are refused"
eq "$(U=alice q "SELECT improv.set_policies('m', \$\$$POL\$\$)")" 1 "the owner sets policies: version 1"
eq "$(U=alice q "SELECT version || ' ' || set_by FROM improv.policies('m')")" "1 alice" "improv.policies"
eq "$(U=carol q "SELECT version FROM improv.policies('m')")" 1 "a role that may act as the owner reads them"
has "$(U=ann e "SELECT * FROM improv.policies('m')")" "may read its policies" "ann may not read them"
eq "$(U=ann q "SELECT count(*) FROM improv.policy")" 0 "nor read improv.policy"
has "$(U=ann e "INSERT INTO improv.policy VALUES (1, 9, 'x', '', 'ann', now())")" "permission denied" "nor write it"
eq "$(U=alice q "SELECT action || ':' || who || ':' || (detail->>'version') FROM improv.policy_audit")" "set_policies:alice:1" \
	"the set is audited"
echo "ok: set_policies validates, stores and audits; only the owner's side reads the policies"

# ---- 3. every read path -------------------------------------------------------
# ann: all but Falcon. ed: all but Price. nob: nothing.
eq "$(U=ann q "$ROWS")" "Q1/Gadget=600 Q1/Widget=1000 Q2/Gadget=1200 Q2/Widget=2000" "ann's result rows: no Falcon"
eq "$(U=ann q "SELECT string_agg(product || '=' || value, ' ' ORDER BY product) FROM improv_m.total")" "Gadget=1800 Widget=3000" \
	"ann's totals: no Falcon row"
eq "$(U=ann q "SELECT value FROM improv_m.grand")" 5100 "the grand total is over every item, Falcon included"
eq "$(U=nob q "SELECT count(*) FROM improv_m.revenue")" 0 "nob, with no policy, reads no result row"
eq "$(U=alice q "SELECT count(*) FROM improv_m.revenue")" 6 "the owner reads every row"
eq "$(U=carol q "SET ROLE alice; SELECT count(*) FROM improv_m.revenue")" 6 "SET ROLE to the owner reads every row"
eq "$(q "SELECT count(*) FROM improv_m.revenue")" 6 "a superuser reads every row"
eq "$(U=ann q "SELECT string_agg(name, ',' ORDER BY id) FROM improv.items('m')")" "Q1,Q2,Gadget,Widget" "ann's items"
eq "$(U=ann q "SELECT string_agg(name, ',' ORDER BY id) FROM improv.categories('m')")" "Time,Product" "ann's categories"
eq "$(U=ed q "SELECT string_agg(name || ':' || coalesce(formula, '-'), ',' ORDER BY id) FROM improv.measures('m')")" \
	"Quantity:-,Revenue:-,Total:Total = SUM(Revenue OVER Time),Grand:Grand = SUM(Revenue OVER Time, Product)" \
	"ed's measures: no Price, and no formula that reads it"
eq "$(U=ann q "SELECT string_agg(coord->>'Product' || '=' || value, ',' ORDER BY coord->>'Time', coord->>'Product') FROM improv.cells('m','Quantity') WHERE coord->>'Time' = 'Q1'")" \
	"Gadget=30,Widget=100" "ann's input cells: no Falcon"
eq "$(U=ann q "SELECT count(*) FROM improv.cells('m','Revenue')")" 4 "ann's derived cells"
eq "$(U=ed q "SELECT count(*) FROM improv.cells('m','Revenue')")" 6 "ed's derived cells: Revenue reads Price, and is shown"
for t in measure binding category view lambda item cell; do
	eq "$(U=ann q "SELECT count(*) FROM improv.$t")" 0 "ann reads nothing of improv.$t"
done
eq "$(U=alice q "SELECT count(*) FROM improv.measure")" 5 "the owner reads improv.measure"
has "$(U=nob e "SELECT * FROM improv.measures('m')")" "do not let you open it" "nob may not open the model"
# Every sync mode keeps the filter: the results are rewritten by recomputes.
FALCON_Q1="SELECT value FROM improv_m.revenue WHERE product = 'Falcon' AND time = 'Q1'"
for mode in commit logical manual trigger; do
	U=alice q "SELECT improv.set_sync_mode('m','$mode')" >/dev/null
	q "UPDATE sales SET qty = qty + 1 WHERE product = 'Falcon' AND quarter = 'Q1'"
	if [ "$mode" = manual ]; then U=alice q "SELECT improv.recalc('m')" >/dev/null; fi
	wait_for "$FALCON_Q1" "$(q "SELECT price * qty FROM sales WHERE product = 'Falcon' AND quarter = 'Q1'")" "$mode mode computes"
	eq "$(U=ann q "SELECT count(*) FROM improv_m.revenue WHERE product = 'Falcon'")" 0 "$mode mode: ann still reads no Falcon row"
	eq "$(U=ann q "SELECT count(*) FROM improv_m.revenue")" 4 "$mode mode: ann's rows"
done
# A measure defined after the policies is filtered from its first row.
U=alice q "SELECT improv.define('m','Double = Revenue * 2')" >/dev/null
U=alice q "SELECT improv.recalc('m')" >/dev/null
U=alice q "GRANT SELECT ON improv_m.double TO ann"
eq "$(U=ann q "SELECT count(*) FROM improv_m.double WHERE product = 'Falcon'")" 0 "a new result table is filtered"
eq "$(U=ann q "SELECT count(*) FROM improv_m.double")" 4 "a new result table's visible rows"
echo "ok: a restricted role sees only what its policy allows, through the result tables, the functions and the catalog, in every sync mode"

# ---- 4. hidden is missing -----------------------------------------------------
eq "$(U=ed e "SELECT * FROM improv.cells('m','Price')" | sed 's/Price/X/')" \
	"$(U=ed e "SELECT * FROM improv.cells('m','Nope')" | sed 's/Nope/X/')" "a hidden measure's cells answer as a missing one's"
eq "$(U=ed e "SELECT improv.define('m','A = Price * 2')" | sed 's/Price/X/')" \
	"$(U=ed e "SELECT improv.define('m','A = Nope * 2')" | sed 's/Nope/X/')" "a definition cannot read a hidden measure"
EDIT() { echo "SELECT improv.set_cells('m', '[{\"measure\": \"$1\", \"coord\": {\"Time\": \"Q1\", \"Product\": \"$2\"}, \"value\": 5}]')"; }
eq "$(U=ann e "$(EDIT Quantity Falcon)" | sed 's/Falcon/X/')" "$(U=ann e "$(EDIT Quantity Nope)" | sed 's/Nope/X/')" \
	"an edit of a hidden item answers as one of a missing item"
eq "$(U=ann e "SELECT improv.drop_measure('m','Secret')")" "$(U=ed e "SELECT improv.drop_measure('m','Price')" | sed 's/Price/Secret/')" \
	"dropping a hidden measure answers as dropping a missing one"
lacks "$(U=ann q "SELECT string_agg(name, ',') FROM improv.items('m')")" Falcon "ann's items"
echo "ok: hidden is missing"

# ---- 5. edits and defines -----------------------------------------------------
before=$(q "SELECT qty FROM sales WHERE quarter = 'Q1' AND product = 'Widget'")
has "$(U=ann e "$(EDIT Quantity Widget)")" 'may not edit cell Quantity' "ann may read but not edit"
eq "$(q "SELECT qty FROM sales WHERE quarter = 'Q1' AND product = 'Widget'")" "$before" "a refused edit writes nothing"
grep -q '"audit":"deny","who":"ann","model":"m","action":"EditCell"' "$D/log" || fail "a refused edit is logged"
eq "$(U=ed q "$(EDIT Quantity Widget)")" 1 "ed edits a cell"
eq "$(q "SELECT qty FROM sales WHERE quarter = 'Q1' AND product = 'Widget'")" 5 "ed's edit is in the bound table"
eq "$(U=ann q "SELECT value FROM improv_m.revenue WHERE time = 'Q1' AND product = 'Widget'")" 50 "and recomputed"
eq "$(U=ed e "$(EDIT Price Widget)" | sed 's/Price/X/')" "$(U=ed e "$(EDIT Nope Widget)" | sed 's/Nope/X/')" \
	"ed's edit of the hidden Price answers as one of a missing measure"
has "$(U=ed e "SELECT improv.set_cells('m', '[{\"measure\": \"Revenue\", \"coord\": {\"Time\": \"Q1\", \"Product\": \"Widget\"}, \"value\": 1}]')")" \
	"not an input measure" "nobody edits a derived cell"
eq "$(U=ed q "SELECT improv.define('m','Triple = Revenue * 3')")" "$(U=alice q "SELECT id FROM improv.measure WHERE name = 'Triple'")" "ed defines"
eq "$(q "SELECT tableowner FROM pg_tables WHERE schemaname = 'improv_m' AND tablename = 'triple'")" alice "as the owner"
eq "$(q "SELECT relrowsecurity FROM pg_class WHERE oid = 'improv_m.triple'::regclass")" t "its results are filtered"
eq "$(U=ed q "SELECT improv.define('m','Triple = Revenue * 4', replace => true)")" "$(U=alice q "SELECT id FROM improv.measure WHERE name = 'Triple'")" "ed redefines"
U=ed q "SELECT improv.drop_measure('m','Triple')" >/dev/null
# A hidden dependent is never named: Secret reads Quantity and is hidden from ed.
U=alice q "SELECT improv.define('m','Secret = Quantity * 7')" >/dev/null
U=alice q "SELECT improv.set_policies('m', \$\$$POL
@id(\"ed-no-secret\") forbid (principal == Improv::User::\"ed\", action in Improv::Action::\"Read\", resource in Improv::Measure::\"$(idof measure Secret)\");\$\$)" >/dev/null
out=$(U=ed e "SELECT improv.drop_measure('m','Quantity')")
lacks "$out" Secret "dropping a measure a hidden one reads"
has "$out" "may not read" "says why without the name"
U=alice q "SELECT improv.drop_measure('m','Secret')"
U=alice q "SELECT improv.set_policies('m', \$\$$POL\$\$)" >/dev/null
has "$(U=ann e "SELECT improv.define('m','Mine = Revenue * 2')")" "may not define Mine" "ann may not define"
has "$(U=ann e "SELECT improv.define('m','Double = Revenue * 3', replace => true)")" "may not define Double" "nor redefine"
eq "$(U=alice q "SELECT count(*) FROM improv.measure WHERE name = 'Mine'")" 0 "a refused define writes nothing"
grep -q '"who":"ann","model":"m","action":"DefineMeasure","resource":"Mine"' "$D/log" || fail "a refused define is logged"
has "$(U=ann e "SELECT improv.bind_measure('m','B','sales','qty')")" "permission denied" "a restricted role binds nothing"
has "$(U=ed e "SELECT improv.recalc('m')")" "permission denied" "nor recalcs"
eq "$(U=alice q "SELECT string_agg(action || ':' || who || ':' || resource, ',' ORDER BY at) FROM improv.policy_audit WHERE who = 'ed'")" \
	"EditCell:ed:1 cells,DefineMeasure:ed:Triple,DefineMeasure:ed:Triple,DefineMeasure:ed:Triple" "ed's changes are audited"
echo "ok: allowed edits and defines run as the owner and are audited; denied ones write nothing and are logged"

# ---- 6. the owner is never locked out -----------------------------------------
# set: the next version, which every set and clear moves on by one.
ver() { U=alice q "SELECT version FROM improv.policies('m')"; }
next() { echo $(($(ver) + 1)); }
want=$(next)
eq "$(U=alice q "SELECT improv.set_policies('m', 'forbid (principal, action, resource);')")" "$want" "a forbid-all document"
eq "$(U=alice q "SELECT count(*) FROM improv_m.revenue")" 6 "the owner reads every row"
eq "$(U=alice q "SELECT count(*) FROM improv.measures('m')")" 6 "and every measure"
want=$(next)
eq "$(U=carol q "SET ROLE alice; SELECT improv.set_policies('m', \$\$$POL\$\$)")" "$want" "and may still set the policies"
eq "$(q "SELECT count(*) FROM improv_m.revenue")" 6 "a superuser too"
# An Admin may grant Admin; the owner stays unrestricted whatever it writes.
ADMIN="$POL
@id(\"fin-admin\") permit (principal in Improv::Role::\"fin\", action, resource);"
U=alice q "SELECT improv.set_policies('m', \$\$$ADMIN\$\$)" >/dev/null
q "CREATE ROLE fay LOGIN; GRANT fin TO fay; GRANT USAGE ON SCHEMA improv TO fay"
want=$(next)
eq "$(U=fay q "SELECT improv.set_policies('m', \$\$$ADMIN
forbid (principal == Improv::User::\"alice\", action, resource);\$\$)")" "$want" "an Admin sets policies"
eq "$(U=alice q "SELECT count(*) FROM improv_m.revenue")" 6 "a forbid naming the owner does not restrict her"
want=$(next)
eq "$(U=alice q "SELECT improv.set_policies('m', \$\$$POL\$\$)")" "$want" "the owner takes the policies back"
has "$(U=fay e "SELECT improv.set_policies('m', NULL)")" "do not let you open it" "fay is no longer Admin"
echo "ok: the owner is never locked out; an Admin may grant Admin"

# ---- 7. changes apply from the next statement of another session ---------------
# One psql session for ann, fed through a FIFO, against changes made in others.
mkfifo "$D/in"
"$PG_BIN/psql" -h "$D" -p "$P" -U ann -d t -qAtX <"$D/in" >"$D/out" 2>&1 &
exec 3>"$D/in"
ask() { # sql -> the answer of ann's long-lived session
	local n
	n=$(wc -l <"$D/out")
	echo "$1" >&3
	echo "SELECT 'mark';" >&3
	for _ in $(seq 400); do
		[ "$(tail -1 "$D/out")" = mark ] && [ "$(wc -l <"$D/out")" -gt "$n" ] && break
		sleep 0.025
	done
	sed -n "$((n + 1)),\$p" "$D/out" | sed '$d' | tr '\n' ' ' | sed 's/ $//'
}
eq "$(ask "SELECT count(*) FROM improv_m.revenue;")" 4 "ann's session: 4 rows"
U=alice q "SELECT improv.set_policies('m', \$\$$(sed 's/"no-falcon") forbid/"no-falcon") permit/' <<<"$POL")\$\$)" >/dev/null
eq "$(ask "SELECT count(*) FROM improv_m.revenue;")" 6 "the next statement sees a new policy"
U=alice q "SELECT improv.set_policies('m', \$\$$POL\$\$)" >/dev/null
eq "$(ask "SELECT count(*) FROM improv_m.revenue;")" 4 "and the next its replacement"
q "REVOKE staff FROM ann"
eq "$(ask "SELECT count(*) FROM improv_m.revenue;")" 0 "a revoked role applies at the next statement"
q "GRANT staff TO ann"
eq "$(ask "SELECT count(*) FROM improv_m.revenue;")" 4 "and a granted one"
U=alice q "SELECT improv.set_policies('m', 'permit (principal in Improv::Group::\"g\", action in Improv::Action::\"Read\", resource);', '{\"g\": {\"users\": [\"ann\"]}}')" >/dev/null
eq "$(ask "SELECT count(*) FROM improv_m.revenue;")" 6 "a group membership applies at the next statement"
U=alice q "SELECT improv.set_policies('m', 'permit (principal in Improv::Group::\"g\", action in Improv::Action::\"Read\", resource);', '{\"g\": {\"users\": []}}')" >/dev/null
eq "$(ask "SELECT count(*) FROM improv_m.revenue;")" 0 "and its removal"
eq "$(ask "BEGIN ISOLATION LEVEL REPEATABLE READ; SELECT count(*) FROM improv_m.revenue;")" 0 "a snapshot"
U=alice q "SELECT improv.set_policies('m', \$\$$POL\$\$)" >/dev/null
eq "$(ask "SELECT count(*) FROM improv_m.revenue; COMMIT;")" 0 "a REPEATABLE READ transaction keeps its snapshot's policies"
eq "$(ask "SELECT count(*) FROM improv_m.revenue;")" 4 "the next transaction sees the new ones"
exec 3>&-
wait
echo "ok: a change of policies, groups or roles applies from the next statement of another session"

# ---- 8. explain_access ---------------------------------------------------------
CELL='{"measure": "Revenue", "at": {"Time": "Q1", "Product": "Falcon"}}'
eq "$(U=alice q "SELECT string_agg(step || ':' || action || ':' || allowed || ':' || array_to_string(policies, '+'), ' ' ORDER BY step) FROM improv.explain_access('m', 'ReadCell', '$CELL', 'ann')")" \
	"1:ViewModel:true:staff 2:ReadMeasure:true:staff 3:ReadItem:true:staff 4:ReadItem:false:no-falcon" \
	"the owner sees why ann may not read a cell"
eq "$(U=ann q "SELECT allowed || ':' || cardinality(policies) FROM improv.explain_access('m', 'ReadCell', '$CELL')")" "false:0" \
	"ann learns only that she may not"
eq "$(U=ann q "SELECT allowed FROM improv.explain_access('m', 'ReadCell', '{\"measure\": \"Revenue\", \"at\": {\"Time\": \"Q1\", \"Product\": \"Widget\"}}')")" t \
	"and that she may"
has "$(U=ann e "SELECT * FROM improv.explain_access('m', 'ReadCell', '$CELL', 'ed')")" "another role" "ann may not ask about ed"
eq "$(U=alice q "SELECT allowed || ':' || array_to_string(errors, '') FROM improv.explain_access('m', 'Admin')")" \
	"true:may act as the model's owner: not restricted" "the owner is not restricted"
echo "ok: explain_access"

# ---- 9. clear ------------------------------------------------------------------
want=$(next)
eq "$(U=alice q "SELECT improv.set_policies('m', NULL)")" "$want" "a clear moves the version on"
eq "$(U=ann q "SELECT count(*) FROM improv_m.revenue")" 6 "cleared: the grants apply again"
eq "$(q "SELECT count(*) FROM pg_class c WHERE c.relnamespace = 'improv_m'::regnamespace AND c.relrowsecurity")" 0 \
	"cleared: no result table keeps row-level security"
eq "$(U=ann q "SELECT count(*) FROM improv.measure")" 6 "cleared: the catalog is readable again"
eq "$(U=alice q "SELECT version || ':' || (policies IS NULL) FROM improv.policies('m')")" "$want:true" "a cleared row"
eq "$(U=alice q "SELECT action FROM improv.policy_audit ORDER BY at DESC LIMIT 1")" clear_policies "the clear is audited"
echo "ok: clearing restores 0.12's behavior"

# ---- 10. pg_dump keeps the policies --------------------------------------------
U=alice q "SELECT improv.set_policies('m', \$\$$POL\$\$)" >/dev/null
"$PG_BIN/pg_dump" -h "$D" -p "$P" -U postgres -Fc t >"$D/t.dump"
"$PG_BIN/createdb" -h "$D" -p "$P" -U postgres r
"$PG_BIN/pg_restore" -h "$D" -p "$P" -U postgres -d r "$D/t.dump" 2>"$D/restore.err" || fail "restore: $(cat "$D/restore.err")"
eq "$(DB=r q "SELECT version FROM improv.policy")" "$(ver)" "the policies survive a dump and restore"
eq "$(DB=r U=ann q "SELECT count(*) FROM improv_m.revenue")" 4 "and are enforced"
eq "$(DB=r q "SELECT count(*) > 0 FROM improv.policy_audit")" t "the audit trail survives"
echo "ok: pg_dump keeps the policies and their audit trail"

# ---- 11. the qual's plan -------------------------------------------------------
# The decision is two InitPlans; rows are checked only under rule 3.
PLAN=$(U=ann q "EXPLAIN (COSTS OFF) SELECT count(*) FROM improv_m.revenue")
has "$PLAN" "InitPlan" "the qual decides in an InitPlan"
lacks "$PLAN" "cell_visible" "and never calls cell_visible a row"
MID=$(U=alice q "SELECT id FROM improv.model WHERE name = 'm'")
eq "$(U=ann q "SELECT improv.cell_rule($MID, $(idof measure Revenue))")" 3 "ann, with a hidden item, is checked per row"
eq "$(U=ed q "SELECT improv.cell_rule($MID, $PRICE)")" 1 "ed, with Price forbidden, reads no row of it: decided once"
eq "$(U=alice q "SELECT improv.cell_rule($MID, $PRICE)")" 0 "the owner: every row"
# How often a read calls the per-row check (this transaction's count).
calls() { # role -> row_visible calls of one read of revenue
	U=$1 q "SELECT count(*) FROM improv_m.revenue;
	  SELECT coalesce(pg_stat_get_xact_function_calls('improv.row_visible(bigint, bigint, \"any\")'::regprocedure), 0)" | tail -1
}
eq "$(calls nob)" 0 "nob (no policy) reads no row, and no row is checked"
eq "$(calls ann)" 6 "ann's rows are checked one by one"
echo "ok: the qual decides once a statement"
echo PASS
