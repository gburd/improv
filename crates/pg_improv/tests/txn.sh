#!/usr/bin/env bash
# pg_improv transactional semantics and packaging checks, end to end:
#   1. CREATE EXTENSION works WITHOUT shared_preload_libraries (trigger and
#      manual modes, recalc), and every object it creates lives in `improv`
#   2. trigger mode: results are in the writer's transaction (read your own
#      writes), invisible to other sessions until commit, gone on ROLLBACK,
#      and undone with a rolled-back savepoint
#   3. logical mode (preloaded): an aborted transaction and a rolled-back
#      savepoint never reach the results; the committed part does
#   3b. commit mode (preloaded; refused without): results move at COMMIT, in
#      the writer's transaction; a read before COMMIT sees the old ones
#      (improv.recalc gives read-your-writes); ROLLBACK, an error and a
#      rolled-back savepoint leave no results; a failing pre-commit
#      recompute aborts the COMMIT with a clear message; with
#      improv.commit_capture = 'rows' the same holds (the capture triggers
#      see only what commits) and the trigger-mode triggers are not used
#   4. pg_dump / pg_restore of a database with a model: results and the
#      model survive, and recalc / the sync worker carry on
#   4b. a model in each sync mode, over tables that load after the catalog
#      (schema public), restored by a plain pg_restore (no
#      --single-transaction) and by psql -f of a plain dump: both succeed
#      (a commit-mode recompute is skipped while its result tables have no
#      primary key yet) and every model follows writes afterwards
# Usage: tests/txn.sh   (needs pg_improv installed into $PG_BIN's install)
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=$(mktemp -d /var/tmp/pgi-txn.XXXX)
P=${PGPORT_TXN:-55470}
stop() {
	"$PG_BIN/pg_ctl" -D "$D/data" stop -m fast >/dev/null 2>&1 || true
	find "$D" -delete 2>/dev/null || true
}
trap stop EXIT
start() { # extra postgresql.conf lines
	"$PG_BIN/pg_ctl" -D "$D/data" stop -m fast >/dev/null 2>&1 || true
	cat >"$D/data/postgresql.auto.conf" <<-EOF
		port = $P
		listen_addresses = ''
		unix_socket_directories = '$D'
		$1
	EOF
	"$PG_BIN/pg_ctl" -D "$D/data" -l "$D/log" -w start >/dev/null 3>&-
}
"$PG_BIN/initdb" -D "$D/data" -A trust -U improv >/dev/null
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d "${DB:-t}" -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
fail() { echo "FAIL: $*"; tail -20 "$D/log"; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got '$1', want '$2'"; }

# ---- 1. no preload ----------------------------------------------------------
start ""
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv t
q "CREATE EXTENSION pg_improv"
outside=$(q "SELECT count(*) FROM pg_depend d JOIN pg_extension e ON e.oid = d.refobjid
             AND e.extname = 'pg_improv' AND d.deptype = 'e'
             WHERE (d.classid = 'pg_proc'::regclass AND (SELECT pronamespace FROM pg_proc WHERE oid = d.objid) <> 'improv'::regnamespace)
                OR (d.classid = 'pg_class'::regclass AND (SELECT relnamespace FROM pg_class WHERE oid = d.objid) <> 'improv'::regnamespace)
                OR (d.classid = 'pg_type'::regclass AND (SELECT typnamespace FROM pg_type WHERE oid = d.objid) <> 'improv'::regnamespace)")
eq "$outside" 0 "extension objects outside schema improv"
eq "$(q "SELECT count(*) FROM pg_proc WHERE pronamespace = 'improv'::regnamespace")" 21 "functions in improv"
q "CREATE TABLE sales (quarter text, product text, price float8, qty float8, PRIMARY KEY (quarter, product))"
q "INSERT INTO sales VALUES ('Q1','Widget',10,100), ('Q1','Gadget',20,30)"
for s in "create_model('plan')" "bind_category('plan','Time','sales','quarter')" \
	"bind_category('plan','Product','sales','product')" "bind_measure('plan','Price','sales','price')" \
	"bind_measure('plan','Quantity','sales','qty')" "define('plan','Revenue = Price * Quantity')" \
	"set_sync_mode('plan','trigger')"; do
	q "SELECT improv.$s" >/dev/null
done
REV="SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Widget'"
eq "$(q "$REV")" 1000 "no-preload trigger mode, initial"
err=$(q "SELECT improv.set_sync_mode('plan','commit')" 2>&1) && fail "commit mode accepted without the preload"
case $err in *shared_preload_libraries*) ;; *) fail "commit mode without the preload: $err" ;; esac
echo "ok: CREATE EXTENSION without the preload; all objects in schema improv; trigger mode computes; commit mode refused"

# ---- 2. trigger mode is transactional ---------------------------------------
own=$(q "BEGIN; UPDATE sales SET price = 11 WHERE quarter='Q1' AND product='Widget'; $REV; ROLLBACK;")
eq "$own" 1100 "trigger mode: the writer reads its own results"
eq "$(q "$REV")" 1000 "trigger mode: ROLLBACK undoes the results"
# Another session must not see the uncommitted results.
"$PG_BIN/psql" -h "$D" -p "$P" -U improv -d t -qAtX -v ON_ERROR_STOP=1 \
	-c "BEGIN; UPDATE sales SET price = 12 WHERE quarter='Q1' AND product='Widget'; SELECT pg_sleep(2); COMMIT;" >/dev/null &
sleep 1
eq "$(q "$REV")" 1000 "trigger mode: other sessions see the old results before commit"
wait
eq "$(q "$REV")" 1200 "trigger mode: results visible after commit"
sp=$(q "BEGIN; UPDATE sales SET qty = 1 WHERE quarter='Q1' AND product='Gadget';
        SAVEPOINT s; UPDATE sales SET price = 99 WHERE quarter='Q1' AND product='Widget'; ROLLBACK TO s;
        COMMIT; $REV; SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Gadget';" | tr '\n' ' ')
eq "$sp" "1200 20 " "trigger mode: a rolled-back savepoint's results are undone, the rest commits"
q "BEGIN; UPDATE sales SET price = 0 WHERE quarter='Q1' AND product='Widget'; SELECT 1/0;" 2>/dev/null && fail "1/0 succeeded"
eq "$(q "$REV")" 1200 "trigger mode: an aborted (errored) transaction leaves no results"
echo "ok: trigger mode: read-your-writes, isolation until commit, ROLLBACK/savepoint/error undo the results"

# ---- 3. logical mode: aborts never propagate --------------------------------
start "shared_preload_libraries = 'pg_improv'
wal_level = logical
improv.database = 't'
improv.poll_ms = 50"
q "SELECT improv.set_sync_mode('plan','logical')"
wait_rev() { # want label
	local v=""
	for _ in $(seq 200); do
		v=$(q "$REV")
		[ "$v" = "$1" ] && return 0
		sleep 0.05
	done
	fail "$2: got '$v', want '$1'"
}
q "UPDATE sales SET price = 13 WHERE quarter='Q1' AND product='Widget'"
wait_rev 1300 "logical: committed update"
q "BEGIN; UPDATE sales SET price = 77 WHERE quarter='Q1' AND product='Widget'; ROLLBACK;"
q "BEGIN; UPDATE sales SET price = 88 WHERE quarter='Q1' AND product='Widget'; SELECT 1/0;" 2>/dev/null || true
q "BEGIN; SAVEPOINT s; UPDATE sales SET price = 66 WHERE quarter='Q1' AND product='Widget'; ROLLBACK TO s;
   UPDATE sales SET qty = 200 WHERE quarter='Q1' AND product='Widget'; COMMIT;"
wait_rev 2600 "logical: the committed part of a transaction with a rolled-back savepoint"
sleep 0.5
eq "$(q "$REV")" 2600 "logical: aborted transactions never applied"
echo "ok: logical mode: ROLLBACK, errors and rolled-back savepoints never reach the results"

# ---- 3b. commit mode: one recompute at COMMIT -------------------------------
q "SELECT improv.set_sync_mode('plan','commit')"
eq "$(q "SELECT sync_mode FROM improv.status('plan')")" commit "commit mode: status"
eq "$(q "SELECT count(*) FROM pg_trigger WHERE tgname LIKE 'improv_plan_sync_%'")" 0 "commit mode: no triggers"
eq "$(q "$REV")" 2600 "commit mode: entering it leaves the results consistent"
q "UPDATE sales SET price = 15 WHERE quarter='Q1' AND product='Widget'"
eq "$(q "$REV")" 3000 "commit mode: committed update"
rb=$(q "BEGIN; UPDATE sales SET price = 16 WHERE quarter='Q1' AND product='Widget'; $REV;
        SELECT improv.recalc('plan') > 0; $REV; ROLLBACK;" | tr '\n' ' ')
eq "$rb" "3000 t 3200 " "commit mode: a read before COMMIT sees the old results; recalc() reads your writes"
eq "$(q "$REV")" 3000 "commit mode: ROLLBACK leaves the results"
q "BEGIN; UPDATE sales SET price = 0 WHERE quarter='Q1' AND product='Widget'; SELECT 1/0;" 2>/dev/null && fail "1/0 succeeded"
eq "$(q "$REV")" 3000 "commit mode: an errored transaction leaves the results"
sp=$(q "BEGIN; UPDATE sales SET qty = 2 WHERE quarter='Q1' AND product='Gadget';
        SAVEPOINT s; UPDATE sales SET price = 99 WHERE quarter='Q1' AND product='Widget'; ROLLBACK TO s;
        COMMIT; $REV; SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Gadget';" | tr '\n' ' ')
eq "$sp" "3000 40 " "commit mode: a rolled-back savepoint is undone, the rest recomputes at COMMIT"
# A recompute that fails at COMMIT rolls the transaction back, saying why.
err=$(q "BEGIN; SET LOCAL improv.max_cells = 1; UPDATE sales SET price = 17 WHERE quarter='Q1' AND product='Widget'; COMMIT;" 2>&1) \
	&& fail "commit mode: an over-budget COMMIT succeeded"
case $err in *"could not recompute before COMMIT"*"rolled back"*) ;; *) fail "commit mode: pre-commit error: $err" ;; esac
eq "$(q "SELECT price FROM sales WHERE quarter='Q1' AND product='Widget'")" 15 "commit mode: the failed COMMIT rolled the write back"
# Two-phase commit recomputes at PREPARE.
start "shared_preload_libraries = 'pg_improv'
wal_level = logical
improv.database = 't'
improv.poll_ms = 50
max_prepared_transactions = 2"
q "BEGIN; UPDATE sales SET price = 18 WHERE quarter='Q1' AND product='Widget'; PREPARE TRANSACTION 'p1';"
eq "$(q "$REV")" 3000 "commit mode: prepared, not yet committed"
q "COMMIT PREPARED 'p1'"
eq "$(q "$REV")" 3600 "commit mode: COMMIT PREPARED"
# Another session's mode switch reaches a live session's cache of the
# commit-bound relations (each -c is its own transaction, one session).
W1="UPDATE sales SET price = %s WHERE quarter='Q1' AND product='Widget'"
live() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d t -qAtX -v ON_ERROR_STOP=1 \
	-c "$(printf "$W1" "$1")" -c "SELECT pg_sleep(2)" -c "$(printf "$W1" "$2")" >/dev/null; }
live 19 21 &
sleep 1
eq "$(q "$REV")" 3800 "commit mode: a live session's commit"
q "SELECT improv.set_sync_mode('plan','manual')"
wait
eq "$(q "$REV")" 3800 "a live session sees the switch to manual"
live 22 23 &
sleep 1
eq "$(q "$REV")" 3800 "manual: no sync"
q "SELECT improv.set_sync_mode('plan','commit')"
eq "$(q "$REV")" 4400 "re-entering commit mode recomputes"
wait
eq "$(q "$REV")" 4600 "a live session sees the switch to commit"
# The row capture: same semantics, a scoped recompute at COMMIT.
q "SET improv.commit_capture = 'rows'; SELECT improv.set_sync_mode('plan','commit')" >/dev/null
eq "$(q "SELECT count(*) FROM pg_trigger WHERE tgname LIKE 'improv_plan_cap_%'")" 4 "capture: four triggers"
eq "$(q "SELECT count(*) FROM pg_trigger WHERE tgname LIKE 'improv_plan_sync_%'")" 0 "capture: no sync triggers"
cp=$(q "BEGIN; UPDATE sales SET price = 24 WHERE quarter='Q1' AND product='Widget'; $REV; COMMIT; $REV" | tr '\n' ' ')
eq "$cp" "4600 4800 " "capture: old results before COMMIT, new after"
q "BEGIN; UPDATE sales SET price = 25 WHERE quarter='Q1' AND product='Widget'; ROLLBACK;"
q "BEGIN; SAVEPOINT s; UPDATE sales SET price = 26 WHERE quarter='Q1' AND product='Widget'; ROLLBACK TO s;
   UPDATE sales SET qty = 300 WHERE quarter='Q1' AND product='Gadget'; COMMIT;"
eq "$(q "$REV")" 4800 "capture: ROLLBACK and a rolled-back savepoint leave the results"
eq "$(q "SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Gadget'")" 6000 "capture: the committed part"
q "BEGIN; UPDATE sales SET price = 27 WHERE quarter='Q1' AND product='Widget'; PREPARE TRANSACTION 'p2';"
q "COMMIT PREPARED 'p2'"
eq "$(q "$REV")" 5400 "capture: COMMIT PREPARED"
q "SELECT improv.set_sync_mode('plan','logical')"
eq "$(q "SELECT count(*) FROM pg_trigger WHERE tgname LIKE 'improv_plan_cap_%'")" 0 "capture: triggers gone"
wait_rev 5400 "back to logical"
echo "ok: commit mode: results at COMMIT/PREPARE; old results before it; ROLLBACK/error/savepoint undo; a failed recompute aborts the COMMIT; the same with commit_capture = rows"

# ---- 4. dump / restore -------------------------------------------------------
# Dump the logical-mode database and restore it into a fresh cluster whose
# worker serves it: the model, its sync mode and its results come back, and
# the restored worker creates its own slot and follows new writes.
q "SELECT improv.define('plan','Twice = LAMBDA(x, x * 2)')" >/dev/null
"$PG_BIN/pg_dump" -h "$D" -p "$P" -U improv -Fc t >"$D/t.dump"
# The dump is taken; this cluster is discarded, so stop it immediately: a
# fast shutdown on a loaded CI runner once outlasted pg_ctl's 60 s wait.
"$PG_BIN/pg_ctl" -D "$D/data" stop -m immediate >/dev/null
find "$D/data" -delete
"$PG_BIN/initdb" -D "$D/data" -A trust -U improv >/dev/null
start "shared_preload_libraries = 'pg_improv'
wal_level = logical
improv.database = 't'
improv.poll_ms = 50"
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv t
"$PG_BIN/pg_restore" -h "$D" -p "$P" -U improv -d t --exit-on-error "$D/t.dump"
eq "$(q "$REV")" 5400 "restored results"
eq "$(q "SELECT count(*) FROM improv.measure m JOIN improv.model o ON o.id = m.model_id WHERE o.name = 'plan'")" 3 "restored model measures"
eq "$(q "SELECT count(*) FROM improv.lambda")" 1 "restored lambdas"
eq "$(q "SELECT sync_mode FROM improv.model WHERE name = 'plan'")" logical "restored sync mode"
q "UPDATE sales SET price = 31 WHERE quarter='Q1' AND product='Widget'"
wait_rev 6200 "the restored model follows new writes (new slot)"
eq "$(q "SELECT slot IS NOT NULL AND last_error IS NULL FROM improv.status('plan')")" t "restored status"
q "SELECT improv.define('plan','Double = Twice(Revenue)')" >/dev/null
q "SELECT improv.create_model('fresh')" >/dev/null
eq "$(q "SELECT count(*) FROM improv.model")" 2 "ids keep minting after restore"
echo "ok: pg_dump / pg_restore keeps models, sync mode and results; the restored worker resumes"

# ---- 4b. every sync mode through a plain restore ----------------------------
# pg_dump orders table data by schema, then name, and adds primary keys after
# all of it: public.* rows load after improv.* and improv_*, each COPY in its
# own transaction, while the result tables have no key yet. W reads f_* and
# g_*: when f_*'s COPY commits g_* is still empty, so a recompute would move
# W and upsert into a result table with no key to upsert on.
DB=s q "SELECT 1" >/dev/null 2>&1 || "$PG_BIN/createdb" -h "$D" -p "$P" -U improv s
DB=s q "CREATE EXTENSION IF NOT EXISTS pg_improv"
for m in lg tg cm mn; do
	DB=s q "CREATE TABLE public.f_$m (k text PRIMARY KEY, v float8)"
	DB=s q "CREATE TABLE public.g_$m (k text PRIMARY KEY, u float8)"
	DB=s q "INSERT INTO public.f_$m VALUES ('a', 1), ('b', 2); INSERT INTO public.g_$m VALUES ('a', 5), ('b', 5)"
	for s in "create_model('$m')" "set_sync_mode('$m','manual')" "bind_category('$m','K','public.f_$m','k')" \
		"bind_measure('$m','V','public.f_$m','v')" "bind_category('$m','K','public.g_$m','k')" \
		"bind_measure('$m','U','public.g_$m','u')" "define('$m','W = V - U')"; do
		DB=s q "SELECT improv.$s" >/dev/null
	done
done
for x in "tg trigger" "cm commit"; do DB=s q "SELECT improv.set_sync_mode('${x% *}','${x#* }')"; done
# No worker serves s, where logical mode is refused: lg is made logical as a
# dump restored here would leave it.
DB=s q "UPDATE improv.model SET sync_mode = 'logical' WHERE name = 'lg'"
DB=s q "INSERT INTO improv.sync_state (model_id, slot, publication) SELECT id, 'improv_db', 'improv_db' FROM improv.model WHERE name = 'lg'"
DB=s q "SELECT improv.sync_publication()" >/dev/null
# (The worker serves database t, not s: recalc the logical model too.)
for m in mn lg; do DB=s q "SELECT improv.recalc('$m')" >/dev/null; done
W="SELECT string_agg(k || '=' || value, ',' ORDER BY k) FROM"
for m in lg tg cm mn; do
	for _ in $(seq 200); do [ "$(DB=s q "$W improv_$m.w")" = "a=-4,b=-3" ] && break; sleep 0.05; done
	eq "$(DB=s q "$W improv_$m.w")" "a=-4,b=-3" "$m before the dump"
done
"$PG_BIN/pg_dump" -h "$D" -p "$P" -U improv -Fc s >"$D/s.dump"
"$PG_BIN/pg_dump" -h "$D" -p "$P" -U improv -Fp s >"$D/s.sql"
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv r1
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv r2
"$PG_BIN/pg_restore" -h "$D" -p "$P" -U improv -d r1 --exit-on-error "$D/s.dump" 2>"$D/r1.err" ||
	fail "plain pg_restore of every sync mode: $(cat "$D/r1.err")"
"$PG_BIN/psql" -h "$D" -p "$P" -U improv -d r2 -qX -v ON_ERROR_STOP=1 -f "$D/s.sql" >/dev/null 2>"$D/r2.err" ||
	fail "psql -f of a plain dump of every sync mode: $(cat "$D/r2.err")"

for db in r1 r2; do
	for m in lg tg cm mn; do
		eq "$(DB=$db q "$W improv_$m.w")" "a=-4,b=-3" "$db: $m's restored results"
		eq "$(DB=$db q "SELECT sync_mode FROM improv.model WHERE name = '$m'")" \
			"$(DB=s q "SELECT sync_mode FROM improv.model WHERE name = '$m'")" "$db: $m's sync mode"
		DB=$db q "UPDATE public.f_$m SET v = 3 WHERE k = 'a'"
	done
	DB=$db q "SELECT improv.recalc('mn')" >/dev/null
	eq "$(DB=$db q "$W improv_tg.w")" "a=-2,b=-3" "$db: trigger mode follows a write"
	eq "$(DB=$db q "$W improv_cm.w")" "a=-2,b=-3" "$db: commit mode follows a write"
	eq "$(DB=$db q "$W improv_mn.w")" "a=-2,b=-3" "$db: manual mode recalcs"
done
# The worker serves database t only: a logical model in r1 or r2 is checked
# by a recalc; the restored model in t above shows the worker resumes.
for db in r1 r2; do
	DB=$db q "SELECT improv.recalc('lg')" >/dev/null
	eq "$(DB=$db q "$W improv_lg.w")" "a=-2,b=-3" "$db: the logical model recalcs"
done
echo "ok: a plain pg_restore and psql -f restore every sync mode; each follows writes"
echo PASS
