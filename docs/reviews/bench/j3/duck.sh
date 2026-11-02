#!/usr/bin/env bash
# duck.sh: improv(source, model) over a 2.5M-row DuckDB table (and 250k, 1M), in both CLIs.
set -uo pipefail
EXT=${J3_TREE:-$HOME/j3-tree}/build/release/extension/improv/improv.duckdb_extension
W=${J3_OUT:-$HOME/j3-out}/duck
mkdir -p $W
DB=$W/facts.duckdb
V1=${DUCKDB_V1:-$HOME/dcli/v1.5.6/duckdb}
V2=${DUCKDB_V2:-$HOME/dcli/v2.0.0-alpha46057/duckdb}
find $W -maxdepth 1 -name "facts.duckdb*" -delete
# the scale.sh shape: person x year x release x scope, 2.5M rows -> 44,750 coordinates
$V1 $DB -c "
CREATE TABLE contribution_rows AS
SELECT g AS id, 'p' || (g // 3 * 7919 % 1790) AS person, 2000 + b // 12 AS year, 'r' || (b // 3 % 4) AS release,
       (['commit', 'mail', 'review'])[1 + b % 3] AS scope, md5((g // 3)::varchar) AS commit_sha, (g % 17) * 1.25 AS amount
FROM (SELECT g, ((g // 3 * 7919 % 1790) * 37 + g // 3 // 1790 % 25 * 7) % 180 AS b FROM range(1, 2500001) t(g));
CREATE TABLE r250k AS SELECT * FROM contribution_rows WHERE id <= 250000;
CREATE TABLE r1m AS SELECT * FROM contribution_rows WHERE id <= 1000000;
SELECT count(*), count(DISTINCT (person, year, release, scope)) FROM contribution_rows;"
MODEL='category Person = person
category Year = year
category Release = release
category Scope = scope
input N = id count
input Amount = amount sum
Contributions = N * 1
Share = Amount / SUM(Amount OVER Person)'
for cli in $V1 $V2; do
	echo "== $($cli -c "SELECT library_version FROM pragma_version()" -noheader -list)"
	for t in r250k r1m contribution_rows; do
		for rep in 1 2; do
			/usr/bin/time -f "%e s wall %M kB" $cli -unsigned -noheader -list $DB -c "LOAD '$EXT';" \
				-c ".timer on" -c "SELECT measure, count(*), round(sum(value), 3) FROM improv('$t', '$MODEL') GROUP BY 1 ORDER BY 1;" \
				> $W/out.txt 2> $W/err.txt
			echo "$t rep $rep: $(tr '\n' ' ' < $W/out.txt | sed 's/Run Time (s): //g') | $(tail -n 1 $W/err.txt)"
		done
	done
	# reference: the same GROUP BY in plain DuckDB
	$cli -noheader -list $DB -c ".timer on" -c "SELECT count(*) FROM (SELECT person, year, release, scope, count(id), sum(amount) FROM contribution_rows GROUP BY ALL);" 2>&1 | tr '\n' ' '; echo " <- plain GROUP BY"
done
echo DONE
