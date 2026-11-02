#!/usr/bin/env bash
# J3 qualification (docs/reviews/2026-10-21-road-to-1.0.md) of a release
# candidate, part 1: the v0.10.0 benchmark's engine, Mentat and pg_improv
# cases, sequential, each step timed with /usr/bin/time -v. Part 2 is
# j3b.sh. Run on a quiet, dedicated machine (the 0.11.0 and 0.12.0 runs:
# EC2 c7i.12xlarge, Debian 13, PGDG PG 17 and 18; see their qualification
# docs in docs/reviews/).
#
#   docs/reviews/bench/j3/j3.sh <rev>
#
# J3_TREE (default ~/j3-tree) is a clone of the repository checked out at
# <rev>; J3_OUT (default ~/j3-out) gets <step>.log and summary.txt. Step 5
# installs the candidate's pg_improv into the system PG 17 and 18 with
# `cargo pgrx install --sudo`: nothing else may be using those installs (a
# long-running cluster on them crashes when its pg_improv.so is replaced).
set -uo pipefail
rev=$1
# shellcheck source=/dev/null
. ~/.cargo/env
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-40} CARGO_TERM_COLOR=never
O=${J3_OUT:-$HOME/j3-out}; mkdir -p $O; S=$O/summary.txt
say() { echo "$(date -u +%FT%TZ) $*" | tee -a $S; }
T=${J3_TREE:-$HOME/j3-tree}
[ -d $T ] || git clone -q https://codeberg.org/gregburd/improv.git $T
cd $T && git fetch -q origin && git checkout -q "$rev" && git submodule update -q --init
say "tree $(git describe --tags --always) $(git rev-parse HEAD); $(rustc --version)"

# 1. Release build of the CLI and the heavy test binaries.
cargo build -q --release -p improv_cli > $O/build.log 2>&1 || { say "build FAIL"; exit 1; }
cargo test -q --release -p improv_engine --test fixpoint --test stress --test probe --no-run >> $O/build.log 2>&1 || { say "test build FAIL"; exit 1; }
say "built"

run() { # name timeout cmd...
	local n=$1 to=$2; shift 2
	timeout -s KILL "$to" /usr/bin/time -v "$@" > $O/$n.log 2>&1
	local rc=$?
	say "$n rc=$rc wall=$(grep 'Elapsed (wall' $O/$n.log | awk '{print $NF}') peak_kb=$(grep 'Maximum resident' $O/$n.log | awk '{print $NF}') $(grep -E '^test result' $O/$n.log | head -1)"
}
# 2. FIXPOINT at 1.5k/100k and 10k/1M (the tagged cases).
run fixpoint 1800 cargo test -q --release -p improv_engine --test fixpoint -- --ignored --nocapture --test-threads 1
# 3. The stress grid (1M, 2M cells) and the probes (the v0.10.0 benchmark's shapes).
run stress-1m 1800 cargo test -q --release -p improv_engine --test stress scale_evaluate_1m -- --ignored --nocapture --exact
run stress-2m 1800 cargo test -q --release -p improv_engine --test stress scale_evaluate_2m -- --ignored --nocapture --exact
for f in "SUM(X OVER W)" "RANK(X OVER W)" "SORTBY(X, X OVER W, -1 INTO K)"; do
	tag=$(echo "$f" | cut -c1-4 | tr -cd 'A-Z')
	PROBE_N=1000000 PROBE_FORMULA="$f" run probe-wide-$tag 1800 cargo test -q --release -p improv_engine --test probe wide -- --ignored --nocapture
done
PROBE_N=20000 PROBE_SHAPE=fan run probe-fan-20k 1800 cargo test -q --release -p improv_engine --test probe measures -- --ignored --nocapture
PROBE_N=20000 PROBE_SHAPE=chain run probe-chain-20k 1800 cargo test -q --release -p improv_engine --test probe measures -- --ignored --nocapture
PROBE_TIME=1000 PROBE_PRODUCT=1000 run probe-grid-1m 1800 cargo test -q --release -p improv_engine --test probe grid -- --ignored --nocapture

# 4. The 1M-row import into Mentat, then load, eval and a one-cell save.
W=$O/csv; mkdir -p $W; I=$T/target/release/improv
[ -f $W/sales1m.csv ] || awk 'BEGIN { srand(7); print "t,p,r"; for (i = 0; i < 1000000; i++) printf "t%d,p%d,%.2f\n", int(i / 1000), i % 1000, rand() * 100 }' > $W/sales1m.csv
find $W -maxdepth 1 -name 'm.db*' -delete
$I init $W/m.db > /dev/null
run import-1m 1800 $I import-csv $W/m.db $W/sales1m.csv 100 Qty r t:1:Time p:2:Product
$I define $W/m.db 102 "Revenue = Qty * 2" > /dev/null
run eval-1m 900 $I eval $W/m.db 102
run list-1m 900 $I list $W/m.db
run set-1m 900 $I set $W/m.db 100 7 --at Time=t0,Product=p0
say "db $(du -b $W/m.db | cut -f1) bytes"

# 5. pg_improv scale.sh at full size on PG 17 and 18 (release install into the system PG).
cd $T/crates/pg_improv || exit 1
for pg in 17 18; do
	cargo pgrx install --release --sudo -c /usr/lib/postgresql/$pg/bin/pg_config --no-default-features --features pg$pg > $O/install-$pg.log 2>&1 || { say "pg$pg install FAIL"; continue; }
	d=$O/pg$pg; find $d -delete 2>/dev/null
	PG_BIN=/usr/lib/postgresql/$pg/bin PGI_DATA=$d PGI_PORT=$((56400 + pg)) ROWS=2500000 BATCHES=1000 \
		run scale-pg$pg 7200 bash tests/scale.sh
	grep -E "^ *(loaded|ok:|FAIL|refresh|recalc|initial|batch)" $O/scale-pg$pg.log | head -20 >> $S
done
say DONE
