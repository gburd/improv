#!/usr/bin/env bash
# Throughput of a LANGUAGE improv function vs plain SQL, PL/pgSQL and a SQL
# function, on a private cluster. Run under flock ~/.pgrx-test.lock after
# `cargo pgrx install --release` (the .so lives in the shared libdir).
set -euo pipefail
PG_BIN=/usr/lib/postgresql/17/bin
D=$HOME/pgi-plimprov
P=55491
N=${N:-1000000}
cleanup() {
	"$PG_BIN/pg_ctl" -D "$D" stop -m fast >/dev/null 2>&1 || true
	if [ -d "$D" ]; then find "$D" -mindepth 1 -delete && rmdir "$D"; fi
}
trap cleanup EXIT
cleanup
"$PG_BIN/initdb" -D "$D" -A trust -U improv >/dev/null
printf "port = $P\nunix_socket_directories = '$D'\njit = off\nshared_preload_libraries = 'pg_improv'\n" >>"$D/postgresql.conf"
"$PG_BIN/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null
q() { "$PG_BIN/psql" -h "$D" -p "$P" -U improv -d postgres -qAtX -v ON_ERROR_STOP=1 "$@"; }
q -c "CREATE EXTENSION pg_improv" -c "
CREATE FUNCTION m_improv(price float8, cost float8) RETURNS float8
  LANGUAGE improv IMMUTABLE AS \$\$ price - cost \$\$;
CREATE FUNCTION m_bonus(sales float8, target float8) RETURNS float8
  LANGUAGE improv IMMUTABLE AS \$\$
    Ratio = sales / target
    MAX2(Ratio - 1, 0) * 1000
  \$\$;
CREATE FUNCTION m_plpgsql(price float8, cost float8) RETURNS float8
  LANGUAGE plpgsql IMMUTABLE AS \$\$ BEGIN RETURN price - cost; END \$\$;
-- not inlinable (SET), so it is called per row like the others
CREATE FUNCTION m_sql(price float8, cost float8) RETURNS float8
  LANGUAGE sql IMMUTABLE SET search_path = public AS 'SELECT price - cost';"
best() { # median ms of 5 runs of sum(<expr>) over N float8 rows x
	for _ in 1 2 3 4 5; do
		q -c '\timing on' -c "SELECT sum($1) FROM (SELECT g::float8 AS x FROM generate_series(1, $N) g) s" |
			sed -n 's/^Time: \([0-9.]*\) ms.*/\1/p'
	done | sort -n | sed -n 3p
}
# Every call gets the same float8 arguments (x, x / 2); the baseline does the
# one subtraction inline, so the delta is the function call itself.
base=$(best "x - x / 2")
for e in "x - x / 2" "m_improv(x, x / 2)" "m_bonus(x, x / 2)" "m_plpgsql(x, x / 2)" "m_sql(x, x / 2)"; do
	t=$(best "$e")
	python3 -c "import sys; t,b,n=map(float,sys.argv[1:4]); print(f'{sys.argv[4]:<24} {t:8.1f} ms  {(t-b)*1000/n:6.3f} us/call over plain SQL')" "$t" "$base" "$N" "$e"
done
