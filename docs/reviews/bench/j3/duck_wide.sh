#!/usr/bin/env bash
# duck_wide.sh: improv() over N distinct coordinates (no fold): one category of N items,
# and a 2-category N = a x b grid. Shows the engine and the pg_model::build_model path.
set -uo pipefail
EXT=${J3_TREE:-$HOME/j3-tree}/build/release/extension/improv/improv.duckdb_extension
W=${J3_OUT:-$HOME/j3-out}/duck
DB=$W/wide.duckdb
V1=${DUCKDB_V1:-$HOME/dcli/v1.5.6/duckdb}
mkdir -p $W
find $W -maxdepth 1 -name "wide.duckdb*" -delete
$V1 $DB -c "
CREATE TABLE w1 AS SELECT 'i' || lpad(g::varchar, 8, '0') AS item, (g * 7919 % 1000003)::double AS x FROM range(1, 2500001) t(g);
CREATE TABLE g2 AS SELECT 't' || (g // 1000) AS t, 'p' || (g % 1000) AS p, (g % 97)::double AS q FROM range(0, 2500000) t(g);"
run() { # label table rows model
	local label=$1 tbl=$2 n=$3 model=$4
	timeout -s KILL 900 /usr/bin/time -f "%e s wall %M kB" $V1 -unsigned -noheader -list $DB -c "LOAD '$EXT';" \
		-c ".timer on" -c "SELECT measure, count(*), round(sum(value), 1) FROM improv('(SELECT * FROM $tbl LIMIT $n)', '$model') GROUP BY 1 ORDER BY 1;" \
		> $W/out.txt 2> $W/err.txt
	echo "$label n=$n: rc=$? $(tr '\n' ' ' < $W/out.txt | sed 's/Run Time (s): //g' | cut -c1-200) | $(tail -n 1 $W/err.txt)"
}
G='category T = t
category P = p
input Q = q
R = Q * 2'
for n in 250000 1000000 2500000; do run "grid T x P (1000 P per T), R = Q * 2" g2 $n "$G"; done
O='category I = item
input X = x
Y = X * 2'
for n in 10000 100000 300000 1000000; do run "one category of n items, Y = X * 2" w1 $n "$O"; done
echo DONE
