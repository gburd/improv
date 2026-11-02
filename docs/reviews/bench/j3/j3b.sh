#!/usr/bin/env bash
# J3 part 2 (after j3.sh, same J3_TREE and J3_OUT): many models (200 and
# 1,000) in one PG 18 database (pg_many.sh), then DuckDB's improv() over a
# folded 2.5M-row table and over distinct coordinates (duck.sh,
# duck_wide.sh). DUCKDB_V1 and DUCKDB_V2 are the v1.5.6 and v2.0 CLIs
# (ci/duckdb-clis.sh fetches and builds them).
set -uo pipefail
# shellcheck source=/dev/null
. ~/.cargo/env
here=$(cd "$(dirname "$0")" && pwd)
export J3_TREE=${J3_TREE:-$HOME/j3-tree}
O=${J3_OUT:-$HOME/j3-out}; export J3_OUT=$O
for nm in 200 1000; do NM=$nm PGI_DATA=$O/many PGI_PORT=56441 timeout -s KILL 2400 bash "$here/pg_many.sh" > $O/many-$nm.log 2>&1; echo "rc=$?" >> $O/many-$nm.log; done
cd "$J3_TREE" && git submodule update -q --init && make configure release > $O/duck-build.log 2>&1 || echo "duck build FAIL" >> $O/duck-build.log
bash "$here/duck.sh" > $O/duck.log 2>&1
bash "$here/duck_wide.sh" > $O/duck-wide.log 2>&1
echo DONE > $O/j3b.done
