#!/usr/bin/env bash
# Upgrade test: a database on an older pg_improv release moves to this tree
# with ALTER EXTENSION pg_improv UPDATE, and keeps working.
#   tests/upgrade.sh [FROM_TAG ...]     (default: every v* tag that ships an
#                                         update script to this version)
# For each FROM tag: build and install that release, create a logical-mode
# model with data, then install this tree over it (both versions' SQL side
# by side; one .so, the new one), restart, ALTER EXTENSION ... UPDATE, and
# check the version, the results, that the worker still follows writes, that
# a define() still works, that pg_dump now keeps the model, and that the
# model then moves to commit sync (0.10.0) and recomputes at COMMIT. From
# before 0.11.0: the per-model slot and publication give way to the shared
# improv_db, and the model stays in sync through the switch.
set -euo pipefail
here=$(cd "$(dirname "$0")/.." && pwd)
root=$(git -C "$here" rev-parse --show-toplevel)
PG_CONFIG=${PG_CONFIG:-/usr/lib/postgresql/17/bin/pg_config}
PG_BIN=$("$PG_CONFIG" --bindir)
PGV=$("$PG_CONFIG" --version | sed -E 's/[^0-9]*([0-9]+).*/\1/')
to=$(sed -n 's/^version = "\(.*\)"/\1/p' "$here/Cargo.toml" | head -1)
if [ $# -eq 0 ]; then
	set -- $(for f in "$here"/sql/pg_improv--*--"$to".sql; do
		v=${f##*/pg_improv--}; echo "v${v%%--*}"; done)
fi
P=${PGPORT_UP:-55480}
W=$(mktemp -d /var/tmp/pgi-up.XXXX)
stop() {
	"$PG_BIN/pg_ctl" -D "$W/data" stop -m fast >/dev/null 2>&1 || true
	git -C "$root" worktree remove --force "$W/src" >/dev/null 2>&1 || true
	find "$W" -delete 2>/dev/null || true
}
trap stop EXIT
q() { "$PG_BIN/psql" -h "$W" -p "$P" -U improv -d t -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
fail() { echo "FAIL ($from): $*"; tail -15 "$W/log"; exit 1; }
eq() { [ "$1" = "$2" ] || fail "$3: got '$1', want '$2'"; }
REV="SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Widget'"
wait_rev() {
	local v=""
	for _ in $(seq 200); do v=$(q "$REV"); [ "$v" = "$1" ] && return 0; sleep 0.05; done
	fail "$2: got '$v', want '$1'"
}
install() { # crate dir
	(cd "$1" && cargo pgrx install --release --pg-config "$PG_CONFIG" --features "pg$PGV" --no-default-features >/dev/null)
}

for from in "$@"; do
	echo "== $from -> $to (PG $PGV)"
	git -C "$root" worktree add -q --detach "$W/src" "$from"
	CARGO_TARGET_DIR="$W/target-old" install "$W/src/crates/pg_improv"
	git -C "$root" worktree remove --force "$W/src"
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
	eq "$(q "SELECT extversion FROM pg_extension WHERE extname = 'pg_improv'")" "${from#v}" "old version installed"
	q "CREATE TABLE sales (quarter text, product text, price float8, qty float8, PRIMARY KEY (quarter, product))"
	q "INSERT INTO sales VALUES ('Q1','Widget',10,100), ('Q1','Gadget',20,30), ('Q2','Widget',10,200)"
	for s in "create_model('plan')" "bind_category('plan','Time','sales','quarter')" \
		"bind_category('plan','Product','sales','product')" "bind_measure('plan','Price','sales','price')" \
		"bind_measure('plan','Quantity','sales','qty')" "define('plan','Revenue = Price * Quantity')" \
		"define('plan','Total = SUM(Revenue OVER Time)')"; do
		q "SELECT improv.$s" >/dev/null
	done
	q "UPDATE sales SET price = 11 WHERE quarter='Q1' AND product='Widget'"
	wait_rev 1100 "old version computes"
	# As if alice, not a superuser, had made the model: from 0.11.0 the worker
	# reads sales and writes the results as her. (sales is improv.sales: the
	# superuser is named improv, and search_path starts with "\$user".)
	q "CREATE ROLE alice; GRANT USAGE ON SCHEMA improv TO alice; GRANT SELECT ON sales TO alice"
	q "ALTER SCHEMA improv_plan OWNER TO alice"

	# The new release over the old: SQL scripts side by side, the new .so.
	install "$here"
	"$PG_BIN/pg_ctl" -D "$W/data" -l "$W/log" -w restart >/dev/null 3>&-
	# A write the old worker may not have applied yet: the update must not lose it.
	q "UPDATE sales SET qty = 250 WHERE quarter='Q1' AND product='Widget'"
	q "ALTER EXTENSION pg_improv UPDATE"
	eq "$(q "SELECT extversion FROM pg_extension WHERE extname = 'pg_improv'")" "$to" "updated version"
	# 0.11.0: each model acts as its owner, its result schema's owner before.
	eq "$(q "SELECT owner::text FROM improv.model WHERE name = 'plan'")" alice "the update records the owner"
	eq "$(q "SELECT string_agg(DISTINCT tableowner, ',') FROM pg_tables WHERE schemaname = 'improv_plan'")" alice "the owner owns the result tables"
	# 0.11.0: row-level security on every catalog table keyed by model_id, and
	# the owner trigger, as a fresh CREATE EXTENSION makes them.
	eq "$(q "SELECT count(*) FROM pg_class c WHERE c.relnamespace = 'improv'::regnamespace AND c.relkind = 'r'
	         AND (c.relname = 'model' OR EXISTS (SELECT 1 FROM pg_attribute a WHERE a.attrelid = c.oid AND a.attname = 'model_id'))
	         AND NOT (c.relrowsecurity AND EXISTS (SELECT 1 FROM pg_policy p WHERE p.polrelid = c.oid AND p.polname = 'write'))")" 0 \
		"every catalog table has row-level security after the update"
	eq "$(q "SELECT count(*) FROM pg_trigger WHERE tgname = 'check_owner' AND tgrelid = 'improv.model'::regclass")" 1 "the owner trigger after the update"
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
	echo "ok: $from -> $to: results kept, worker follows writes, define works, pg_dump keeps the model, commit mode works"
	"$PG_BIN/pg_ctl" -D "$W/data" stop -m immediate >/dev/null  # discarded next
	find "$W/data" -delete
done
echo PASS
