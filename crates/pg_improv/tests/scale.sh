#!/usr/bin/env bash
# pg_improv P1-3/P1-4 acceptance: aggregated bindings and new items in the
# logical worker, at scale and under random batches. Needs pg_improv
# installed into PG_BIN's install (cargo pgrx install) with the preload.
#
#   tests/scale.sh                 # small: 50k fact rows, 100 random batches
#   ROWS=2500000 BATCHES=1000 tests/scale.sh     # the full acceptance run
#
# 1. Scale: ROWS contribution rows bound with agg => 'count' (and a
#    count_distinct over the commit sha) into PERSONS × 25 cells (~45,000)
#    of Person × Year × Release × Scope. Times a REFRESH of the equivalent
#    materialized view, recalc, and the worker's initial build, then
#    1,000-row INSERT batches (one bringing a new Person) from commit to
#    visible; improv.status must show no rebuild for them.
# 2. Correctness: BATCHES random INSERT / UPDATE (moving rows between
#    coordinates) / DELETE batches into a model with agg => 'count', 'sum'
#    and 'count_distinct'. Afterwards each result equals a fresh
#    improv.recalc of a copy of the model (count, sum) and count(DISTINCT …)
#    (count_distinct), with no rebuild; deleting one of two rows that share a
#    key keeps the distinct count.
# 3. Collations and floats: text min/max under an ICU collation and "C", and
#    a float8 sum, fold without a rebuild and equal a fresh recalc; a
#    nondeterministic collation's min and count_distinct rebuild instead.
set -euo pipefail
PG_BIN=${PG_BIN:-/usr/lib/postgresql/17/bin}
D=${PGI_DATA:-$HOME/pgi-scale}
P=${PGI_PORT:-55437}
ROWS=${ROWS:-50000}
BATCHES=${BATCHES:-100}
PERSONS=${PERSONS:-1790}

cleanup() {
	"$PG_BIN/pg_ctl" -D "$D" stop -m fast >/dev/null 2>&1 || true
	if [ -d "$D" ]; then find "$D" -mindepth 1 -delete && rmdir "$D"; fi
}
trap '[ -n "${KEEP:-}" ] || cleanup' EXIT
cleanup

# UTF8 (section 3 makes ICU collations): a container without locales
# would otherwise default to SQL_ASCII.
"$PG_BIN/initdb" -D "$D" -A trust -U improv -E UTF8 --locale=C >/dev/null
cat >>"$D/postgresql.conf" <<EOF
port = $P
listen_addresses = ''
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'scale'
improv.poll_ms = 50
wal_level = logical
max_replication_slots = 10
max_wal_senders = 10
shared_buffers = '2GB'
work_mem = '256MB'
maintenance_work_mem = '1GB'
max_wal_size = '8GB'
EOF
"$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null 3>&-
"$PG_BIN/createdb" -h "$D" -p "$P" -U improv scale
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d scale -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
fail() { echo "FAIL: $*"; tail -20 "$D/log"; exit 1; }
now() { date +%s%N; }
ms() { echo $(( ($(now) - $1) / 1000000 )); }
# wait until SQL scalar $1 equals $2 (timeout $3 s, default 600)
wait_for() {
	local v="" end=$(( $(date +%s) + ${3:-600} ))
	while [ "$(date +%s)" -lt "$end" ]; do
		v=$(q "$1" || true)
		[ "$v" = "$2" ] && return 0
		sleep 0.02
	done
	fail "timeout waiting for '$1' = '$2' (got '$v')"
}
counters() { q "SELECT incremental_applies || ' ' || rebuilds FROM improv.status('$1')"; }
q "CREATE EXTENSION pg_improv"

# ---- 1. scale --------------------------------------------------------------
# PERSONS persons, each contributing to 25 of 180 buckets (Year 2000..2014 ×
# Release r0..r3 × Scope commit/mail/review): PERSONS × 25 coordinates
# (44,750 for 1,790 persons). One commit credits its person under up to
# three roles, so three rows share each sha (count_distinct < count).
q "CREATE TABLE contribution_rows (id bigint PRIMARY KEY, person text, year int, release text,
     scope text, commit_sha text)"
# row g of commit c = g / 3: person c * 7919 % PERSONS, and bucket b, the
# (c / PERSONS % 25)-th of the person's 25 (all 25 once ROWS >= 75 PERSONS)
rows_sql() { # first last person-expr
	echo "SELECT g, $3, 2000 + b / 12, 'r' || b / 3 % 4, (ARRAY['commit', 'mail', 'review'])[1 + b % 3],
	        md5((g / 3)::text)
	      FROM (SELECT g, (g / 3 * 7919 % $PERSONS * 37 + g / 3 / $PERSONS % 25 * 7) % 180 AS b
	            FROM generate_series($1::int8, $2::int8) g) s"
}
t0=$(now)
q "INSERT INTO contribution_rows $(rows_sql 1 "$ROWS" "'p' || g / 3 * 7919 % $PERSONS")"
q "VACUUM ANALYZE contribution_rows"
echo "   loaded $ROWS fact rows in $(ms "$t0") ms"
x() { q "$1" >/dev/null; }
x "SELECT improv.create_model('contrib')"
x "SELECT improv.set_sync_mode('contrib', 'manual')"
for c in "Person person" "Year year" "Release release" "Scope scope"; do
	set -- $c
	x "SELECT improv.bind_category('contrib', '$1', 'contribution_rows', '$2')"
done
x "SELECT improv.bind_measure('contrib', 'N', 'contribution_rows', 'id', agg => 'count')"
x "SELECT improv.bind_measure('contrib', 'Commits', 'contribution_rows', 'commit_sha', agg => 'count_distinct')"
x "SELECT improv.define('contrib', 'Contributions = N * 1')"
x "SELECT improv.define('contrib', 'DistinctCommits = Commits * 1')"
cells=$(q "SELECT count(*) FROM (SELECT DISTINCT person, year, release, scope FROM contribution_rows) t")
echo "   $cells coordinates (Person × Year × Release × Scope)"
# the materialized view the model replaces, refreshed in plain SQL
q "CREATE MATERIALIZED VIEW mv AS SELECT person, year, release, scope, count(id) AS n,
     count(DISTINCT commit_sha) AS commits FROM contribution_rows GROUP BY 1, 2, 3, 4"
q "CREATE UNIQUE INDEX ON mv (person, year, release, scope)"
t0=$(now)
q "REFRESH MATERIALIZED VIEW CONCURRENTLY mv"
echo "   REFRESH MATERIALIZED VIEW CONCURRENTLY of the same GROUP BY: $(ms "$t0") ms"
t0=$(now)
q "SELECT improv.recalc('contrib')" >/dev/null
echo "   improv.recalc (load, compute, write $cells x 2 result rows): $(ms "$t0") ms"
# the sync worker's initial build: slot, load, fold seed, live engine
t0=$(now)
x "SELECT improv.set_sync_mode('contrib', 'logical')"
wait_for "SELECT rebuilds FROM improv.status('contrib')" 1 3600
echo "   initial build of the live state (worker: slot, load, fold seed, engine): $(ms "$t0") ms"
n=$(q "SELECT count(*) FROM improv_contrib.contributions c JOIN mv USING (person)
       WHERE c.year = mv.year::text AND c.release = mv.release AND c.scope = mv.scope
         AND c.value = mv.n")
[ "$n" = "$cells" ] || fail "$n of $cells cells equal the view"
read -r inc0 reb0 <<<"$(counters contrib)"
next=$((ROWS + 1))
batch() { # label new-person?
	local who="'p' || g / 3 * 7919 % $PERSONS" t0 want
	[ "$2" = new ] && who="CASE WHEN g = $next THEN 'newcomer' ELSE 'p' || g / 3 * 7919 % $PERSONS END"
	want=$(q "SELECT count(*) + 1000 FROM contribution_rows")
	t0=$(now)
	q "INSERT INTO contribution_rows $(rows_sql "$next" $((next + 999)) "$who")"
	wait_for "SELECT sum(value)::bigint FROM improv_contrib.contributions" "$want" 60
	local lat; lat=$(ms "$t0")
	next=$((next + 1000))
	echo "   1,000-row INSERT batch ($1): visible in $lat ms"
	echo "$lat" >>"$D/lat"
}
for i in 1 2 3 4 5; do batch "existing persons, $i" old; done
batch "brings a new Person item" new
[ "$(q "SELECT value FROM improv_contrib.contributions WHERE person = 'newcomer'")" = 1 ] \
	|| fail "the new person's cell is missing"
read -r inc1 reb1 <<<"$(counters contrib)"
[ "$reb1" = "$reb0" ] || fail "the INSERT batches rebuilt the model ($reb0 -> $reb1 rebuilds)"
[ "$inc1" -gt "$inc0" ] || fail "no incremental apply was counted"
max=$(sort -n "$D/lat" | tail -1)
[ "$max" -lt 5000 ] || fail "a batch took $max ms (want < 5000)"
echo "ok: scale: $ROWS rows, $cells cells; 1,000-row batches visible in <= $max ms;" \
	"status: incremental_applies $inc0 -> $inc1, rebuilds $reb0 -> $reb1"

# ---- 2. random batches ------------------------------------------------------
q "CREATE TABLE facts (id int PRIMARY KEY, person text, year int, sha text, amount numeric)"
q "INSERT INTO facts SELECT g, 'p' || g % 20, 2020 + g % 4, 's' || g % 300, (g % 17) * 1.25
   FROM generate_series(1, 2000) g"
for m in fresh live; do
	x "SELECT improv.create_model('$m')"
	[ $m = fresh ] && x "SELECT improv.set_sync_mode('fresh', 'manual')"
	x "SELECT improv.bind_category('$m', 'Person', 'facts', 'person')"
	x "SELECT improv.bind_category('$m', 'Year', 'facts', 'year')"
	x "SELECT improv.bind_measure('$m', 'N', 'facts', 'id', agg => 'count')"
	x "SELECT improv.bind_measure('$m', 'Amount', 'facts', 'amount', agg => 'sum')"
	x "SELECT improv.bind_measure('$m', 'Shas', 'facts', 'sha', agg => 'count_distinct')"
	for r in "RN = N * 1" "RAmount = Amount * 1" "RShas = Shas * 1"; do x "SELECT improv.define('$m', '$r')"; done
done
q "INSERT INTO facts VALUES (0, 'p0', 2020, 'warm', 0)"
wait_for "SELECT count(*) FROM improv_live.rn WHERE person = 'p0' AND year = '2020' AND value = (SELECT count(*) FROM facts WHERE person = 'p0' AND year = 2020)" 1
read -r inc0 reb0 <<<"$(counters live)"
t0=$(now)
for i in $(seq 1 "$BATCHES"); do
	case $((i % 3)) in
	0) q "INSERT INTO facts SELECT 100000 * $i + g, 'p' || (g * $i) % 23, 2020 + g % 5, 's' || (g * $i) % 320,
	        ((g + $i) % 13) * 0.5 FROM generate_series(1, 1 + $i % 7) g" ;;
	1) q "UPDATE facts SET person = 'p' || (id + $i) % 23, amount = amount + 0.25
	      WHERE id IN (SELECT id FROM facts ORDER BY md5(id || 'u' || $i) LIMIT 1 + $i % 5)" ;;
	2) q "DELETE FROM facts WHERE id IN (SELECT id FROM facts ORDER BY md5(id || 'd' || $i) LIMIT 1 + $i % 6)" ;;
	esac
done
echo "   $BATCHES random batches committed in $(ms "$t0") ms"
# the worker has caught up once a marker write is visible
q "INSERT INTO facts VALUES (-1, 'marker', 2020, 'm', 1)"
wait_for "SELECT count(*) FROM improv_live.rn WHERE person = 'marker'" 1 120
q "SELECT improv.recalc('fresh')" >/dev/null
for r in rn ramount rshas; do
	n=$(q "SELECT count(*) FROM improv_live.$r l FULL JOIN improv_fresh.$r f USING (person, year)
	       WHERE l.value IS DISTINCT FROM f.value")
	[ "$n" = 0 ] || fail "$r: $n cells differ from a fresh recalc"
done
n=$(q "SELECT count(*) FROM (SELECT person, year::text AS year, count(DISTINCT sha)::float8 AS v
       FROM facts GROUP BY 1, 2) w FULL JOIN improv_live.rshas r USING (person, year)
       WHERE w.v IS DISTINCT FROM r.value")
[ "$n" = 0 ] || fail "count_distinct: $n cells differ from count(DISTINCT sha)"
read -r inc1 reb1 <<<"$(counters live)"
[ "$reb1" = "$reb0" ] || fail "the random batches rebuilt the model ($reb0 -> $reb1 rebuilds)"
echo "ok: $BATCHES random INSERT/UPDATE/DELETE batches: count and sum equal a fresh recalc," \
	"count_distinct equals count(DISTINCT sha); incremental_applies $inc0 -> $inc1, rebuilds $reb0 -> $reb1"

# deleting one of two rows that share a key keeps the distinct count
q "INSERT INTO facts VALUES (-2, 'dup', 2020, 'shared', 1), (-3, 'dup', 2020, 'shared', 1), (-4, 'dup', 2020, 'other', 1)"
wait_for "SELECT value FROM improv_live.rshas WHERE person = 'dup'" 2
q "DELETE FROM facts WHERE id = -2"
wait_for "SELECT value FROM improv_live.rn WHERE person = 'dup'" 2
[ "$(q "SELECT value FROM improv_live.rshas WHERE person = 'dup'")" = 2 ] || fail "deleting one of two shared keys lowered the count"
q "DELETE FROM facts WHERE id = -3"
wait_for "SELECT value FROM improv_live.rshas WHERE person = 'dup'" 1
echo "ok: deleting one of two rows that share a key keeps count_distinct (2), the second lowers it (1)"

# ---- 3. collations and floats -----------------------------------------------
q "CREATE COLLATION IF NOT EXISTS improv_ci (provider = icu, locale = 'und-u-ks-level2', deterministic = false)"
q "CREATE TABLE words (id int PRIMARY KEY, g text, w text COLLATE \"en-x-icu\", c text COLLATE \"C\",
     x float8)"
q "INSERT INTO words SELECT i, 'g' || i % 7, (ARRAY['apple', 'Banana', 'cherry', 'Äpfel', 'banana', 'zebra',
     'Zulu'])[1 + i % 7], (ARRAY['b', 'A', 'c', 'a'])[1 + i % 4], i * 0.1 + 1e-12 * i
   FROM generate_series(1, 300) i"
for m in wfresh wlive; do
	x "SELECT improv.create_model('$m')"
	[ $m = wfresh ] && x "SELECT improv.set_sync_mode('wfresh', 'manual')"
	x "SELECT improv.bind_category('$m', 'G', 'words', 'g')"
	x "SELECT improv.bind_measure('$m', 'WMin', 'words', 'w', agg => 'min')"
	x "SELECT improv.bind_measure('$m', 'WMax', 'words', 'w', agg => 'max')"
	x "SELECT improv.bind_measure('$m', 'CMax', 'words', 'c', agg => 'max')"
	x "SELECT improv.bind_measure('$m', 'X', 'words', 'x', agg => 'sum')"
	for r in "RWMin = WMin" "RWMax = WMax" "RCMax = CMax" "RX = X * 1"; do x "SELECT improv.define('$m', '$r')"; done
done
q "INSERT INTO words VALUES (0, 'g0', 'warm', 'w', 0)"
wait_for "SELECT count(*) FROM improv_wlive.rx" 7
read -r inc0 reb0 <<<"$(counters wlive)"
for i in $(seq 1 60); do
	case $((i % 3)) in
	0) q "INSERT INTO words SELECT 1000 * $i + g, 'g' || (g * $i) % 8,
	        (ARRAY['apple', 'Banana', 'cherry', 'Äpfel', 'banana', 'zebra', 'Zulu', 'éclair'])[1 + (g + $i) % 8],
	        (ARRAY['b', 'A', 'c', 'a', 'B'])[1 + (g * $i) % 5], (g + $i) * 0.3e-3 FROM generate_series(1, 1 + $i % 4) g" ;;
	1) q "UPDATE words SET g = 'g' || (id + $i) % 8, x = x * 1.5 + 1e-9
	      WHERE id IN (SELECT id FROM words ORDER BY md5(id || 'u' || $i) LIMIT 1 + $i % 4)" ;;
	2) q "DELETE FROM words WHERE id IN (SELECT id FROM words ORDER BY md5(id || 'd' || $i) LIMIT 1 + $i % 5)" ;;
	esac
done
q "INSERT INTO words VALUES (-1, 'marker', 'm', 'm', 1)"
wait_for "SELECT count(*) FROM improv_wlive.rx WHERE g = 'marker'" 1 120
q "SELECT improv.recalc('wfresh')" >/dev/null
for r in rwmin rwmax rcmax; do
	n=$(q "SELECT count(*) FROM improv_wlive.$r l FULL JOIN improv_wfresh.$r f USING (g)
	       WHERE l.value IS DISTINCT FROM f.value")
	[ "$n" = 0 ] || fail "$r: $n cells differ from a fresh recalc"
done
# The fold's float8 sum is exact then rounded once; Postgres adds in scan
# order: equal to within a few ulps (the documented limit).
n=$(q "SELECT count(*) FROM improv_wlive.rx l FULL JOIN improv_wfresh.rx f USING (g)
       WHERE abs(l.value - f.value) > 1e-14 * greatest(abs(f.value), 1) OR (l.value IS NULL) <> (f.value IS NULL)")
[ "$n" = 0 ] || fail "rx: $n cells differ from a fresh recalc beyond rounding"
read -r inc1 reb1 <<<"$(counters wlive)"
[ "$reb1" = "$reb0" ] || fail "collated text min/max rebuilt the model ($reb0 -> $reb1 rebuilds)"
echo "ok: text min/max under en-x-icu and C, and a float8 sum, folded: equal to a fresh recalc;" \
	"rebuilds $reb0 -> $reb1"

# A nondeterministic collation: min and count_distinct rebuild on a change.
q "CREATE TABLE ci (id int PRIMARY KEY, g text, w text COLLATE improv_ci)"
q "INSERT INTO ci VALUES (1, 'g', 'Apple'), (2, 'g', 'apple'), (3, 'g', 'banana')"
x "SELECT improv.create_model('ci')"
x "SELECT improv.bind_category('ci', 'G', 'ci', 'g')"
x "SELECT improv.bind_measure('ci', 'W', 'ci', 'w', agg => 'count_distinct')"
x "SELECT improv.define('ci', 'RW = W * 1')"
wait_for "SELECT value FROM improv_ci.rw" 2
read -r inc0 reb0 <<<"$(counters ci)"
q "INSERT INTO ci VALUES (4, 'g', 'BANANA'), (5, 'g', 'cherry')"
wait_for "SELECT value FROM improv_ci.rw" 3
read -r inc1 reb1 <<<"$(counters ci)"
[ "$reb1" -gt "$reb0" ] || fail "a nondeterministic count_distinct did not rebuild"
echo "ok: count_distinct under a nondeterministic collation counts as Postgres does (rebuilds $reb0 -> $reb1)"
echo PASS
