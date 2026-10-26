#!/usr/bin/env bash
# Upgrade test: a database on an older pg_improv release moves to this tree
# with ALTER EXTENSION pg_improv UPDATE, and keeps working.
#   tests/upgrade.sh [FROM_TAG ...]   (default: every released version, the
#                                       v* tags that ship pg_improv)
# Every released version must have a chain of update scripts in sql/ to
# this version, and ALTER EXTENSION UPDATE must find a path from each,
# whichever tags run here. For each FROM tag that
# builds for PG_CONFIG's major: take that release from the cache
# ($PGI_OLD_CACHE/<version>/<major>: its .so, .control and .sql) or build
# it into the cache, create one model in each of its sync modes with data,
# install this tree over it (both versions' SQL side by side; one .so, the
# new one), restart, ALTER EXTENSION ... UPDATE, and check: the update path
# Postgres picks, the version, every model's results, mode and owner, the
# 0.11.0 row-level security and owner trigger, that the worker still follows
# writes and the trigger, commit and manual models still recompute, that
# define() still works, that pg_dump now keeps the model, and that the model
# then moves to commit sync (0.10.0) and recomputes at COMMIT. From before
# 0.11.0: the per-model slot and publication give way to the shared
# improv_db, and the model stays in sync through the switch.
# Postgres runs from a private copy of PG_CONFIG's install, so the system's
# extension directories are never written. PGRX_LOCK=<file> runs each cargo
# pgrx under flock (a box shared with other pgrx users).
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(git -C "$here" rev-parse --show-toplevel)
PG_CONFIG=${PG_CONFIG:-/usr/lib/postgresql/17/bin/pg_config}
PGV=$("$PG_CONFIG" --version | sed -E 's/[^0-9]*([0-9]+).*/\1/')
CACHE=${PGI_OLD_CACHE:-$HOME/.cache/pgi-old}
to=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/Cargo.toml" | head -1)
version_of() { git -C "$root" show "$1:crates/pg_improv/Cargo.toml" | sed -n 's/^version = "\(.*\)"/\1/p' | head -1; }
older() { [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | head -1)" = "$1" ]; }
pgrx() { ${PGRX_LOCK:+flock -o "$PGRX_LOCK"} cargo pgrx "$@"; }
# pgrx-pg-sys builds for a major `cargo pgrx init` named, or for PGRX_PG_CONFIG_PATH.
# (Only when not named: a changed PGRX_PG_CONFIG_PATH rebuilds pgrx-pg-sys.)
grep -qs "^pg$PGV = " "${PGRX_HOME:-$HOME/.pgrx}/config.toml" || export PGRX_PG_CONFIG_PATH=$PG_CONFIG

released=$(for t in $(git -C "$root" tag -l 'v*' | sort -V); do
	git -C "$root" cat-file -e "$t:crates/pg_improv/pg_improv.control" 2>/dev/null || continue
	# (not `older … && echo`: when the newest tag is this version, the loop's
	# last status is 1 and set -e fails the assignment.)
	if older "$(version_of "$t")" "$to"; then echo "$t"; fi
done)
[ -n "$released" ] || { echo "FAIL: no released pg_improv tag (a clone without tags?)"; exit 1; }
# Every released version reaches this one through sql/'s update scripts.
for t in $released; do
	cur=$(version_of "$t") hops=$(version_of "$t")
	for _ in $(seq 50); do
		[ "$cur" = "$to" ] && break
		cur=$(find "$here/sql" -name "pg_improv--$cur--*.sql" | sed -E 's/.*--(.*)\.sql$/\1/' | sort -V | tail -1)
		[ -n "$cur" ] || { echo "FAIL: no update script from ${hops##*--} on the way from $t to $to"; exit 1; }
		hops="$hops--$cur"
	done
	[ "$cur" = "$to" ] || { echo "FAIL: the update scripts from $t do not reach $to ($hops)"; exit 1; }
	echo "update scripts: $t: $hops"
done
# shellcheck disable=SC2086
[ $# -gt 0 ] || set -- $released

P=${PGPORT_UP:-55480}
W=$(mktemp -d /var/tmp/pgi-up.XXXX)
stop() {
	if [ -d "$W/data" ]; then "$PG_BIN/pg_ctl" -D "$W/data" stop -m fast >/dev/null 2>&1 || true; fi
	find "$W" -delete 2>/dev/null || true
}
trap stop EXIT
# The private install: bindir, pkglibdir and sharedir at their places under
# $W/pg (Postgres finds its lib and share directories relative to its binary).
mkdir "$W/pg"
cp -a --parents "$("$PG_CONFIG" --bindir)" "$("$PG_CONFIG" --pkglibdir)" "$("$PG_CONFIG" --sharedir)" "$W/pg/"
PG_BIN=$W/pg$("$PG_CONFIG" --bindir)
LIB=$W/pg$("$PG_CONFIG" --pkglibdir)
EXT=$W/pg$("$PG_CONFIG" --sharedir)/extension
package() { # crate dir, output dir: the .so, .control and .sql files, flat
	local out=$W/pkg
	(cd "$1" && pgrx package --pg-config "$PG_CONFIG" --features "pg$PGV" --no-default-features --out-dir "$out" >/dev/null)
	mkdir -p "$2"
	find "$out" -type f -name 'pg_improv*' -exec cp {} "$2/" \;
	find "$out" -delete
}
use() { # a package directory: it alone in the private install
	find "$LIB" "$EXT" -maxdepth 1 -name 'pg_improv*' -delete
	cp "$1/pg_improv.so" "$LIB/"
	find "$1" -name 'pg_improv*' ! -name '*.so' -exec cp {} "$EXT/" \;
}
add() { # a package directory over the installed one (a new .so inode)
	find "$LIB" -maxdepth 1 -name pg_improv.so -delete
	cp "$1/pg_improv.so" "$LIB/"
	find "$1" -name 'pg_improv*' ! -name '*.so' -exec cp {} "$EXT/" \;
}
t0=$SECONDS
package "$here" "$W/new"
echo "built $to for PG $PGV in $((SECONDS - t0)) s"

q() { "$PG_BIN/psql" -h "$W" -p "$P" -U improv -d t -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
fail() { echo "FAIL ($from, PG $PGV): $*"; tail -15 "$W/log"; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got '$1', want '$2'"; }
REV="SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Widget'"
rev() { q "SELECT value FROM improv_plan_$1.revenue WHERE time='Q1' AND product='Widget'"; }
wait_rev() {
	local v=""
	for _ in $(seq 200); do v=$(q "$REV"); [ "$v" = "$1" ] && return 0; sleep 0.05; done
	fail "$2: got '$v', want '$1'"
}
# The models besides the logical plan, one per sync mode, all over sales2.
modes() { printf '%s\n' trigger manual; older "$1" 0.10.0 || echo commit; }
model() { # name, relation
	for s in "create_model('$1')" "bind_category('$1','Time','$2','quarter')" \
		"bind_category('$1','Product','$2','product')" "bind_measure('$1','Price','$2','price')" \
		"bind_measure('$1','Quantity','$2','qty')" "define('$1','Revenue = Price * Quantity')"; do
		q "SELECT improv.$s" >/dev/null
	done
}
results=()

for from in "$@"; do
	t0=$SECONDS
	fv=$(version_of "$from")
	if ! git -C "$root" show "$from:crates/pg_improv/Cargo.toml" | grep -q "^pg$PGV = "; then
		echo "== $from -> $to (PG $PGV): skipped, $from does not build for PG $PGV"
		results+=("$from: n/a (no PG $PGV build)")
		continue
	fi
	echo "== $from -> $to (PG $PGV)"
	old=$CACHE/$fv/$PGV
	if [ ! -f "$old/pg_improv.so" ]; then
		mkdir -p "$W/src"
		git -C "$root" archive "$from" | tar -x -C "$W/src"
		# A test fixture: no fat LTO, so a cold cache builds in minutes; and
		# no -Dwarnings, which a newer compiler may turn on old code.
		CARGO_TARGET_DIR="$W/target-old" CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
			RUSTFLAGS="" package "$W/src/crates/pg_improv" "$old.tmp"
		find "$W/src" -delete
		[ ! -d "$old" ] || find "$old" -delete
		mv "$old.tmp" "$old"
		echo "   built $from for PG $PGV into $old in $((SECONDS - t0)) s"
	fi
	use "$old"
	"$PG_BIN/initdb" -D "$W/data" -A trust -U improv >/dev/null
	cat >>"$W/data/postgresql.conf" <<-EOF
		port = $P
		listen_addresses = ''
		unix_socket_directories = '$W'
		shared_preload_libraries = 'pg_improv'
		wal_level = logical
		improv.database = 't'
		improv.poll_ms = 50
	EOF
	"$PG_BIN/pg_ctl" -D "$W/data" -l "$W/log" -w start >/dev/null 3>&-
	"$PG_BIN/createdb" -h "$W" -p "$P" -U improv t
	q "CREATE EXTENSION pg_improv"
	eq "$(q "SELECT extversion FROM pg_extension WHERE extname = 'pg_improv'")" "$fv" "old version installed"
	for s in sales sales2; do
		q "CREATE TABLE $s (quarter text, product text, price float8, qty float8, PRIMARY KEY (quarter, product))"
		q "INSERT INTO $s VALUES ('Q1','Widget',10,100), ('Q1','Gadget',20,30), ('Q2','Widget',10,200)"
	done
	model plan sales
	q "SELECT improv.define('plan','Total = SUM(Revenue OVER Time)')" >/dev/null
	for m in $(modes "$fv"); do
		model "plan_$m" sales2
		q "SELECT improv.set_sync_mode('plan_$m','$m')"
	done
	q "UPDATE sales SET price = 11 WHERE quarter='Q1' AND product='Widget'"
	q "UPDATE sales2 SET price = 11 WHERE quarter='Q1' AND product='Widget'"
	wait_rev 1100 "old version computes"
	q "SELECT improv.recalc('plan_manual')" >/dev/null
	for m in $(modes "$fv"); do eq "$(rev "$m")" 1100 "old version: the $m model computes"; done
	# As if alice, not a superuser, had made the model: from 0.11.0 the worker
	# reads sales and writes the results as her. (sales is improv.sales: the
	# superuser is named improv, and search_path starts with "\$user".)
	q "CREATE ROLE alice; GRANT USAGE ON SCHEMA improv TO alice; GRANT SELECT ON sales TO alice"
	q "ALTER SCHEMA improv_plan OWNER TO alice"
	# From 0.11.0 a model records its owner, its creator (improv here): make
	# it alice, with her result tables, as the 0.11.0 update would have.
	if ! older "$fv" 0.11.0; then
		q "UPDATE improv.model SET owner = 'alice' WHERE name = 'plan'"
		for t in revenue total; do q "ALTER TABLE improv_plan.$t OWNER TO alice"; done
	fi

	# The new release over the old: SQL scripts side by side, the new .so.
	"$PG_BIN/pg_ctl" -D "$W/data" -w stop -m fast >/dev/null
	add "$W/new"
	"$PG_BIN/pg_ctl" -D "$W/data" -l "$W/log" -w start >/dev/null 3>&-
	# A write the old worker may not have applied yet: the update must not lose it.
	q "UPDATE sales SET qty = 250 WHERE quarter='Q1' AND product='Widget'"
	path=$(q "SELECT path FROM pg_extension_update_paths('pg_improv') WHERE source = '$fv' AND target = '$to'")
	[ -n "$path" ] || fail "ALTER EXTENSION UPDATE finds no path from $fv to $to"
	echo "   update path: $path"
	for t in $released; do
		[ -n "$(q "SELECT path FROM pg_extension_update_paths('pg_improv') WHERE source = '$(version_of "$t")' AND target = '$to'")" ] ||
			fail "ALTER EXTENSION UPDATE finds no path from $t to $to"
	done
	q "ALTER EXTENSION pg_improv UPDATE"
	eq "$(q "SELECT extversion FROM pg_extension WHERE extname = 'pg_improv'")" "$to" "updated version"
	# 0.12.0: a measure's number format (H1-22), General (NULL) for every measure.
	eq "$(q "SELECT format_type(atttypid, atttypmod) FROM pg_attribute WHERE attrelid = 'improv.measure'::regclass AND attname = 'format'")" \
		jsonb "the format column after the update"
	eq "$(q "SELECT count(*) FROM improv.measure WHERE format IS NOT NULL")" 0 "every measure is General after the update"
	# 0.11.0: each model acts as its owner, its result schema's owner before.
	want=plan:alice:logical
	for m in $(modes "$fv" | LC_ALL=C sort); do want="$want,plan_$m:improv:$m"; done
	eq "$(q "SELECT string_agg(name || ':' || owner::text || ':' || sync_mode, ',' ORDER BY name COLLATE \"C\") FROM improv.model")" \
		"$want" "the update records each model's owner and keeps its sync mode"
	eq "$(q "SELECT string_agg(DISTINCT tableowner, ',') FROM pg_tables WHERE schemaname = 'improv_plan'")" alice "the owner owns the result tables"
	# 0.11.0: row-level security on every catalog table keyed by model_id, and
	# the owner trigger, as a fresh CREATE EXTENSION makes them.
	eq "$(q "SELECT count(*) FROM pg_class c WHERE c.relnamespace = 'improv'::regnamespace AND c.relkind = 'r'
	         AND (c.relname = 'model' OR EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attname = 'model_id'))
	         AND NOT (c.relrowsecurity AND EXISTS (SELECT 1 FROM pg_policy p WHERE p.polrelid = c.oid AND p.polname = 'write'))")" 0 \
		"every catalog table has row-level security after the update"
	# Items, input cells and state are data: readable only by those who may act as the owner.
	eq "$(q "SELECT string_agg(c.relname, ',' ORDER BY c.relname) FROM pg_policy p JOIN pg_class c ON c.oid = p.polrelid
	         WHERE c.relnamespace = 'improv'::regnamespace AND p.polname = 'read' AND pg_get_expr(p.polqual, p.polrelid) <> 'true'")" \
		cell,item,meta "the owner-only read policies after the update"
	eq "$(q "SELECT count(*) FROM pg_trigger WHERE tgname = 'check_owner' AND tgrelid = 'improv.model'::regclass")" 1 "the owner trigger after the update"
	for m in $(modes "$fv"); do eq "$(rev "$m")" 1100 "the $m model's results after the update"; done
	q "UPDATE sales2 SET qty = 250 WHERE quarter='Q1' AND product='Widget'"
	for m in $(modes "$fv"); do
		if [ "$m" = manual ]; then
			eq "$(rev "$m")" 1100 "the manual model waits for recalc after the update"
			q "SELECT improv.recalc('plan_manual')" >/dev/null
		fi
		eq "$(rev "$m")" 2750 "the $m model recomputes after the update"
	done
	# 0.11.0: the roles. The update makes them; a 0.10.0 model's owner keeps
	# her model once she is an improv_user (the update grants no one).
	eq "$(q "SELECT string_agg(rolname, ',' ORDER BY rolname) FROM pg_roles WHERE rolname LIKE 'improv\_%'")" improv_admin,improv_user "the roles after the update"
	eq "$(q "SELECT pg_has_role('alice', 'improv_user', 'MEMBER')")" f "the update enrolls no one"
	q "ALTER ROLE alice LOGIN; GRANT improv_user TO alice"
	qa() { "$PG_BIN/psql" -h "$W" -p "$P" -U alice -d t -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
	eq "$(qa "SELECT may_change FROM improv.models WHERE name = 'plan'")" t "alice may change her 0.10.0 model"
	eq "$(qa "SELECT count(*) > 0 FROM improv.item i JOIN improv.model m ON m.id = i.model_id WHERE m.name = 'plan'")" t "alice reads her items"
	qa "SELECT improv.define('plan','Alices = Revenue + 1')" >/dev/null
	eq "$(qa "SELECT sync_mode FROM improv.status('plan')")" logical "alice's status"
	qa "SELECT improv.drop_measure('plan','Alices')"
	wait_rev 2750 "a write around the update is applied"
	q "UPDATE sales SET qty = 300 WHERE quarter='Q1' AND product='Widget'"
	wait_rev 3300 "the worker follows writes after the update"
	# 0.11.0: one slot and publication per database; the per-model ones go.
	for _ in $(seq 200); do
		[ "$(q "SELECT string_agg(slot_name, ',' ORDER BY slot_name) FROM pg_replication_slots")" = improv_db ] && break
		sleep 0.05
	done
	eq "$(q "SELECT string_agg(slot_name, ',' ORDER BY slot_name) FROM pg_replication_slots")" improv_db "only the shared slot after the update"
	eq "$(q "SELECT string_agg(pubname, ',' ORDER BY pubname) FROM pg_publication")" improv_db "only the shared publication after the update"
	eq "$(q "SELECT slot || ' ' || (last_error IS NULL)::text FROM improv.status('plan')")" "improv_db true" "status names the shared slot"
	q "SELECT improv.define('plan','Double = Revenue * 2')" >/dev/null
	for _ in $(seq 200); do [ "$(q "SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'")" = 6600 ] && break; sleep 0.05; done
	eq "$(q "SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'")" 6600 "define after the update"
	# 0.10.0: define(…, replace => true) and drop_measure
	q "SELECT improv.set_sync_mode('plan','manual')"
	q "SELECT improv.define('plan','Double = Revenue * 3', replace => true)" >/dev/null
	eq "$(q "SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'")" 9900 "redefine after the update"
	q "SELECT improv.drop_measure('plan','Double')"
	eq "$(q "SELECT to_regclass('improv_plan.double') IS NULL")" t "drop_measure after the update"
	q "SELECT improv.define('plan','Double = Revenue * 2')" >/dev/null
	eq "$(q "SELECT count(*) FROM pg_proc WHERE proname = 'define' AND pronamespace = 'improv'::regnamespace")" 1 "one define after the update"
	# 0.10.0: named LAMBDAs (improv.lambda)
	q "SELECT improv.define('plan','Twice = LAMBDA(x, x * 2)')" >/dev/null
	q "SELECT improv.define('plan','Norm = LAMBDA(x, c, x / MAX(x OVER c))')" >/dev/null
	q "SELECT improv.define('plan','Twice2 = Twice(Revenue) + Norm(Revenue, Time)')" >/dev/null
	q "SELECT improv.recalc('plan')" >/dev/null
	eq "$(q "SELECT value FROM improv_plan.twice2 WHERE time='Q1' AND product='Widget'")" 6601 "a named LAMBDA after the update"
	q "SELECT improv.drop_measure('plan','Twice2')"
	q "SELECT improv.set_sync_mode('plan','logical')"
	cfg=$(q "SELECT count(*) FROM pg_extension, unnest(extconfig) WHERE extname = 'pg_improv'")
	[ "$cfg" -ge 13 ] || fail "the update registered only $cfg config tables for pg_dump"
	# (not `pg_dump | grep -q`: grep -q exits early, pg_dump gets SIGPIPE,
	# and pipefail turns a found match into a failure)
	"$PG_BIN/pg_dump" -h "$W" -p "$P" -U improv -Fp t >"$W/dump.sql"
	grep -q "COPY improv.measure" "$W/dump.sql" || fail "pg_dump omits the model"
	grep -q "COPY improv.lambda" "$W/dump.sql" || fail "pg_dump omits the lambdas"
	eq "$(q "SELECT array_to_string(proconfig, ',') FROM pg_proc WHERE oid = 'improv.mint_items(bigint,bigint[],text[])'::regprocedure")" \
		"search_path=pg_catalog, pg_temp" "mint_items search_path after the update"
	eq "$(q "SELECT rebuilds >= 0 AND incremental_applies >= 0 FROM improv.status('plan')")" t "status counters after the update"
	q "SELECT improv.set_sync_mode('plan','commit')"
	eq "$(q "SELECT sync_mode FROM improv.status('plan')")" commit "commit mode after the update"
	q "BEGIN; UPDATE sales SET qty = 400 WHERE quarter='Q1' AND product='Widget'; UPDATE sales SET price = 12 WHERE quarter='Q1' AND product='Widget'; COMMIT;"
	eq "$(q "$REV")" 4800 "commit mode recomputes at COMMIT after the update"
	eq "$(q "SELECT value FROM improv_plan.double WHERE time='Q1' AND product='Widget'")" 9600 "commit mode: the dependents too"
	echo "ok: $from -> $to (PG $PGV, $((SECONDS - t0)) s): models in $(modes "$fv" | tr '\n' ' ')logical kept their" \
		"results; owner and RLS migrated; the worker, define, pg_dump and commit mode work"
	results+=("$from: ok ($path, $((SECONDS - t0)) s)")
	"$PG_BIN/pg_ctl" -D "$W/data" stop -m immediate >/dev/null # discarded next
	find "$W/data" -delete
done
for r in "${results[@]}"; do echo "PG $PGV: $r"; done
echo PASS
