#!/usr/bin/env bash
# pg_improv access policies: reading a 1M-row result table (1000 x 1000
# items) as a restricted role (row-level security, the improv_access qual)
# against the owner (no qual). Needs pg_improv installed into $PG_BIN's
# Postgres (a release build: cargo pgrx install --release).
#   docs/reviews/bench/rls_1m.sh            # every case, 7 runs each
#   CASES="mixed" REPS=3 docs/reviews/bench/rls_1m.sh
# Cases (each a policy set for vic; alice owns the model):
#   mixed    permit Read; forbid ReadItem on row r0 for everyone; 98 forbids
#            for other users that name r1..r98 (99 named Row items)
#   uniform  permit Read, nothing else
#   cells    mixed, plus a forbid of ReadCell on the cells of r1 x c1 (two
#            named items together: not decidable per category)
#   denied   permit Read, forbid ReadMeasure on W: no row, decided once
#   hidden1  permit Read, forbid ReadItem on r0 (one hidden name)
# Each case runs twice: "stale" after an item is renamed and back (the
# result rows are not vouched for: the per-row check, rule 3) and "clean"
# after a recalc vouches for them (rule 0 or 2 where the policies allow). Prints the median ms of
# `SELECT count(*), sum(value) FROM improv_b.w` for alice and vic, vic's
# cell_rule, and with PLANS=1 the plan's shape for vic.
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
REPS=${REPS:-7}
CASES=${CASES:-mixed uniform hidden1 cells denied}
D=$(mktemp -d /var/tmp/pgi-rls.XXXX)
P=${PGI_PORT:-55437}
cleanup() {
	"$PG_BIN/pg_ctl" -D "$D/data" stop -m immediate >/dev/null 2>&1 || true
	find "$D" -delete 2>/dev/null || true
}
trap cleanup EXIT
"$PG_BIN/initdb" -D "$D/data" -A trust -U postgres >/dev/null
cat >>"$D/data/postgresql.conf" <<EOF
port = $P
listen_addresses = ''
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
shared_buffers = 1GB
max_parallel_workers_per_gather = 0
EOF
"$PG_BIN/pg_ctl" -D "$D/data" -l "$D/log" -w start >/dev/null 3>&-
"$PG_BIN/createdb" -h "$D" -p "$P" -U postgres bench
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U "${U:-postgres}" -d bench -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
q "CREATE EXTENSION pg_improv"
q "CREATE ROLE alice LOGIN; CREATE ROLE vic LOGIN; GRANT improv_user TO alice, vic"
q "CREATE TABLE f (r text, c text, v float8, PRIMARY KEY (r, c));
   INSERT INTO f SELECT 'r'||i, 'c'||j, i*j FROM generate_series(0,999) i, generate_series(0,999) j;
   GRANT SELECT ON f TO alice"
for s in "create_model('b')" "set_sync_mode('b','manual')" "bind_category('b','Row','f','r')" \
	"bind_category('b','Col','f','c')" "bind_measure('b','V','f','v')" "define('b','W = V * 2')"; do
	U=alice q "SELECT improv.$s" >/dev/null
done
U=alice q "SET improv.max_cells = 3000000; SELECT improv.recalc('b')" >/dev/null
U=alice q "GRANT USAGE ON SCHEMA improv_b TO vic; GRANT SELECT ON improv_b.w TO vic"
q "VACUUM ANALYZE improv_b.w"
R0=$(U=alice q "SELECT id FROM improv.item WHERE name = 'r0'")
WID=$(U=alice q "SELECT id FROM improv.measure WHERE name = 'W'")
C1=$(U=alice q "SELECT id FROM improv.item WHERE name = 'c1'")
SQL="SELECT count(*), sum(value) FROM improv_b.w"
ms() { # role -> wall ms of one run of $SQL
	local t0 t1
	t0=$(date +%s%N)
	U=$1 q "$SQL" >/dev/null
	t1=$(date +%s%N)
	echo $(((t1 - t0) / 1000000))
}
med() { sort -n | awk '{a[NR]=$1} END {print a[int((NR+1)/2)]}'; }
policy() {
	local pol='@id("v") permit (principal == Improv::User::"vic", action in Improv::Action::"Read", resource);'
	[ "$1" = uniform ] && { echo "$pol"; return; }
	[ "$1" = hidden1 ] && { echo "$pol
forbid (principal, action == Improv::Action::\"ReadItem\", resource == Improv::Item::\"$R0\");"; return; }
	[ "$1" = denied ] && { echo "$pol
forbid (principal, action == Improv::Action::\"ReadMeasure\", resource == Improv::Measure::\"$WID\");"; return; }
	pol="$pol
forbid (principal, action == Improv::Action::\"ReadItem\", resource == Improv::Item::\"$R0\");"
	for i in $(seq 0 97); do pol="$pol
forbid (principal == Improv::User::\"u$i\", action, resource in Improv::Item::\"$((R0 + 1 + i))\");"; done
	[ "$1" = cells ] && pol="$pol
forbid (principal, action == Improv::Action::\"ReadCell\", resource) when { resource in Improv::Item::\"$((R0 + 1))\" && resource in Improv::Item::\"$C1\" };"
	echo "$pol"
}
want_rows() { case $1 in uniform) echo 1000000 ;; mixed | hidden1) echo 999000 ;; cells) echo 998999 ;; denied) echo 0 ;; esac; }
MID=$(U=alice q "SELECT id FROM improv.model WHERE name = 'b'")
echo "case state owner_ms restricted_ms ratio rule rows"
for c in $CASES; do
	U=alice q "SELECT improv.set_policies('b', \$\$$(policy "$c")\$\$)" >/dev/null
	for state in stale clean; do
		case $state in
		# An item renamed and back: the rows are no longer vouched for.
		stale) U=alice q "UPDATE improv.item SET name = 'x' WHERE name = 'c999'; UPDATE improv.item SET name = 'c999' WHERE name = 'x'" ;;
		clean) U=alice q "SET improv.max_cells = 3000000; SELECT improv.recalc('b')" >/dev/null ;;
		esac
		got=$(U=vic q "SELECT count(*) FROM improv_b.w")
		[ "$got" = "$(want_rows "$c")" ] || { echo "FAIL: $c: vic reads $got rows, want $(want_rows "$c")"; exit 1; }
		rule=$(U=vic q "SELECT improv.cell_rule($MID, $WID)")
		for u in alice vic; do ms $u >/dev/null; done # warm-up
		a=$(for _ in $(seq "$REPS"); do ms alice; done | med)
		v=$(for _ in $(seq "$REPS"); do ms vic; done | med)
		echo "$c $state $a $v $(awk -v a="$a" -v v="$v" 'BEGIN {printf "%.2f", v / a}') $rule $got"
		[ -z "${PLANS:-}" ] || U=vic q "EXPLAIN (COSTS OFF) $SQL" | sed 's/^/    /'
	done
done
