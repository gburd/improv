#!/usr/bin/env bash
# The extension and the `improv` CLI must compute the same cells. Builds the
# Time x Product revenue model both ways from one data set (2025's WidgetA
# quantity as two rows the extension sums; the CLI gets the sum, 120) and
# compares every Revenue, Share and Growth cell's printed value.
#
# Usage:
#   DUCKDB=/path/to/duckdb IMPROV=/path/to/improv crates/duckdb_improv/test/compare_cli.sh
# IMPROV defaults to `cargo run -q -p improv_cli --` from the repo root.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
EXT="${EXT:-$HERE/build/release/improv.duckdb_extension}"
DUCKDB="${DUCKDB:-duckdb}"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/improv_cmp.XXXXXX")"
trap 'find "$WORK" -delete' EXIT
if [ -n "${IMPROV:-}" ]; then cli() { "$IMPROV" "$@"; }
else cli() { (cd "$ROOT" && cargo run -q -p improv_cli -- "$@"); }; fi

# --- the extension ----------------------------------------------------------
"$DUCKDB" -unsigned -noheader -csv "$WORK/x.duckdb" > "$WORK/ext.csv" <<SQL
LOAD '$EXT';
CREATE TABLE sales(year INTEGER, product VARCHAR, price DOUBLE, qty INTEGER);
INSERT INTO sales VALUES
  (2024, 'WidgetA', 10, 100), (2024, 'WidgetB', 20, 50),
  (2025, 'WidgetA', 11, 70), (2025, 'WidgetA', 11, 50), (2025, 'WidgetB', 20, 40);
SELECT measure, "Time", "Product", value_text FROM improv('sales', 'category Time = year
category Product = product
input Price = price max
input Quantity = qty sum
Revenue = Price * Quantity
Share = Revenue / SUM(Revenue OVER Product)
Growth = Revenue - PREV(Revenue, Time, 1, 0)') ORDER BY ALL;
SQL

# --- the CLI ----------------------------------------------------------------
M="$WORK/m.db"
cli init "$M" >/dev/null
cli add-category "$M" 1 Time >/dev/null
cli add-category "$M" 2 Product >/dev/null
cli add-item "$M" 10 1 2024 >/dev/null
cli add-item "$M" 11 1 2025 >/dev/null
cli add-item "$M" 20 2 WidgetA >/dev/null
cli add-item "$M" 21 2 WidgetB >/dev/null
cli add-measure "$M" 100 Price number input Time Product >/dev/null
cli add-measure "$M" 101 Quantity number input Time Product >/dev/null
while read -r t p price qty; do
  cli set "$M" 100 "$price" --at "Time=$t,Product=$p" >/dev/null
  cli set "$M" 101 "$qty" --at "Time=$t,Product=$p" >/dev/null
done <<'DATA'
2024 WidgetA 10 100
2024 WidgetB 20 50
2025 WidgetA 11 120
2025 WidgetB 20 40
DATA
cli define "$M" 102 "Revenue = Price * Quantity" >/dev/null
cli define "$M" 103 "Share = Revenue / SUM(Revenue OVER Product)" >/dev/null
cli define "$M" 104 "Growth = Revenue - PREV(Revenue, Time, 1, 0)" >/dev/null
for id in 102 103 104; do cli eval "$M" "$id"; done \
  | sed -n 's/^  \([A-Za-z]*\)\[Time=\([^,]*\), Product=\([^]]*\)\] = \(.*\)$/\1,\2,\3,\4/p' \
  | sort > "$WORK/cli.csv"

sort "$WORK/ext.csv" > "$WORK/ext.sorted"
n="$(wc -l < "$WORK/cli.csv")"
if [ "$n" -ne 12 ] || ! diff -u "$WORK/cli.csv" "$WORK/ext.sorted"; then
  echo "FAIL: the extension and 'improv eval' disagree ($n CLI cells)"
  exit 1
fi
cat "$WORK/ext.sorted"
echo "PASS: improv(...) matches 'improv eval' on all $n cells"
