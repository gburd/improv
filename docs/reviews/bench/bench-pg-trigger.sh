#!/usr/bin/env bash
# pg_improv L1 benchmark sketch (L4 builds the full report): writer latency of
# a 1000-row UPDATE on a ~10k-input-cell model in manual vs trigger mode, and
# the cost of the manual-mode recalc that trigger mode replaces.
#
#   docs/reviews/bench/bench-pg-trigger.sh   # from the repo root, on a box
#                                            # with cargo-pgrx and PG 17
# Builds and installs pg_improv (release) into PG 17's system directories,
# runs a private throwaway cluster on $PORT under a fresh $DIR (left behind
# for inspection; delete it yourself), prints medians.
set -euo pipefail
PG=${PG:-/usr/lib/postgresql/17/bin}
DIR=${DIR:-$HOME/pg-improv-bench-$(date +%s)}
PORT=${PORT:-32299}
RUNS=${RUNS:-7}
here=$(cd "$(dirname "$0")/../../.." && pwd)

# Shared with `cargo pgrx test`, which installs into the same directories.
exec 9>"$HOME/.pgrx-test.lock"
flock 9
(cd "$here/crates/pg_improv" &&
  cargo pgrx install --release --pg-config "$PG/pg_config" >/dev/null)

if [ -e "$DIR" ]; then echo "$DIR exists; pick another DIR" >&2; exit 1; fi
"$PG/initdb" -D "$DIR" -A trust >/dev/null
"$PG/pg_ctl" -D "$DIR" -o "-p $PORT -k $DIR" -l "$DIR/log" -w start >/dev/null
trap '"$PG/pg_ctl" -D "$DIR" -m fast stop >/dev/null' EXIT
q() { "$PG/psql" -X -q -At -h "$DIR" -p "$PORT" -d postgres -v ON_ERROR_STOP=1 "$@"; }

# 50 quarters × 100 products = 5000 fact rows × (Price, Quantity) = 10k
# input cells; Revenue (5000 cells) and Total (100) derived.
q <<'SQL'
CREATE EXTENSION pg_improv;
CREATE TABLE sales (quarter text, product text, price float8, qty float8,
                    PRIMARY KEY (quarter, product));
INSERT INTO sales SELECT 'Q' || q, 'P' || lpad(p::text, 3, '0'), p, q
  FROM generate_series(1, 50) q, generate_series(1, 100) p;
SELECT improv.create_model('bench');
SELECT improv.bind_category('bench', 'Time', 'sales', 'quarter');
SELECT improv.bind_category('bench', 'Product', 'sales', 'product');
SELECT improv.bind_measure('bench', 'Price', 'sales', 'price');
SELECT improv.bind_measure('bench', 'Quantity', 'sales', 'qty');
SELECT improv.define('bench', 'Revenue = Price * Quantity');
SELECT improv.define('bench', 'Total = SUM(Revenue OVER Time)');
SELECT improv.recalc('bench');
CREATE FUNCTION ms(sql text) RETURNS float8 LANGUAGE plpgsql AS $$
DECLARE t timestamptz := clock_timestamp();
BEGIN EXECUTE sql; RETURN extract(epoch FROM clock_timestamp() - t) * 1000; END $$;
SQL

# Products P001..P020 × 50 quarters = 1000 rows per UPDATE; each statement
# is its own transaction. Median of $RUNS.
upd="UPDATE sales SET price = price + 1 WHERE product <= 'P020'"
median() {
  for _ in $(seq "$RUNS"); do q -c "SELECT ms(\$q\$$1\$q\$)"; done |
    sort -n | awk '{a[NR]=$1} END {printf "%8.1f ms\n", a[int((NR+1)/2)]}'
}
row() { printf '%-58s %s\n' "$1" "$(median "$2")"; }

q -c "SELECT improv.set_sync_mode('bench', 'manual')" >/dev/null
row "manual:  1000-row UPDATE (writer latency)" "$upd"
row "manual:  improv.recalc (full)" "SELECT improv.recalc('bench')"
row "manual:  improv.recalc(ARRAY['Price'])" "SELECT improv.recalc('bench', ARRAY['Price'])"
q -c "SELECT improv.set_sync_mode('bench', 'trigger')" >/dev/null
row "trigger: 1000-row UPDATE, closure has SUM (full closure)" "$upd"
# Element-wise only: drop Total so the plan is coordinate-restricted.
q -c "DELETE FROM improv.measure WHERE name = 'Total'"
row "trigger: 1000-row UPDATE, element-wise (restricted)" "$upd"
row "trigger: 1-row UPDATE, element-wise (restricted)" \
  "UPDATE sales SET price = price + 1 WHERE product = 'P001' AND quarter = 'Q1'"
q -c "SELECT count(*) || ' input cells' FROM sales, LATERAL (VALUES (price), (qty)) v"
