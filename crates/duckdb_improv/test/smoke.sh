#!/usr/bin/env bash
# Smoke test for the improv DuckDB extension against a standalone DuckDB CLI.
# Asserts on output, so it fails loudly when the logic breaks. Run it with
# every DuckDB the binary claims to load into: the v1.5.6 floor and v2.0.
#
# Usage:
#   DUCKDB=/path/to/duckdb crates/duckdb_improv/test/smoke.sh
# EXT defaults to build/release/improv.duckdb_extension (`make release`).
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXT="${EXT:-$HERE/build/release/improv.duckdb_extension}"
DUCKDB="${DUCKDB:-duckdb}"
DB="$(mktemp -u "${TMPDIR:-/tmp}/improv_smoke.XXXXXX.duckdb")"
trap 'rm -f "$DB" "$DB".wal' EXIT

[ -f "$EXT" ] || { echo "FAIL: $EXT not found (run 'make release' first)"; exit 1; }
"$DUCKDB" -c "SELECT 'duckdb ' || library_version FROM pragma_version()" -noheader -list

run() { "$DUCKDB" -unsigned -noheader -list "$DB" -c "LOAD '$EXT';" -c "$1" 2>&1; }

out="$("$DUCKDB" -unsigned -noheader -list "$DB" <<SQL
LOAD '$EXT';
-- improv_calc: math, financial, text, dates, logic, NULL handling.
SELECT 'math=' || improv_calc('ROUND(SQRT(2) * 100, 3)');
SELECT 'pmt=' || improv_calc('ROUND(PMT(rate/12, n, -pv), 2)', json_object('rate', 0.06, 'n', 360, 'pv', 200000));
SELECT 'fv=' || improv_calc('ROUND(FV(0.05/12, 120, -100), 2)');
SELECT 'rate=' || improv_calc('ROUND(RATE(360, -1199.1, 200000) * 12, 4)');
SELECT 'text=' || improv_calc('LEN(TRIM(s))', json_object('s', '  improv  '));
SELECT 'date=' || improv_calc('DAYS(DATE(2024, 3, 1), DATE(2024, 2, 1))');
SELECT 'serial=' || improv_calc('DATE(2024, 1, 31)');
SELECT 'if=' || improv_calc('IF(x > 10, x / 2, x * 2)', json_object('x', 4));
SELECT 'tif=' || improv_calc_text('IF(x > 10, "big", "small")', json_object('x', 4));
SELECT 'bool=' || improv_calc('AND(TRUE, NOT(FALSE))');
SELECT 'err=' || coalesce(improv_calc('1/0')::VARCHAR, 'NULL');
SELECT 'textval=' || coalesce(improv_calc('"abc"')::VARCHAR, 'NULL');
SELECT 'nullformula=' || coalesce(improv_calc(NULL)::VARCHAR, 'NULL');
SELECT 'nullinput=' || coalesce(improv_calc('x + 1', '{"x": null}')::VARCHAR, 'NULL');
SELECT 'lazy=' || improv_calc('IF(TRUE, 5, x)', '{"x": null}');
SELECT 'rows=' || sum(improv_calc('x * x', json_object('x', i))) FROM range(10000) t(i);
-- improv_calc_text: any type, errors rendered.
SELECT 'ttext=' || improv_calc_text('UPPER("ada") & "!"');
SELECT 'tdiv=' || improv_calc_text('1/0');
SELECT 'tnum=' || improv_calc_text('SQRT(-1)');
SELECT 'tna=' || improv_calc_text('NA()');
SELECT 'tdate=' || improv_calc_text('TEXT(EDATE(DATE(2024, 1, 31), 1), "yyyy-mm-dd")');
-- improv_functions: the registry.
SELECT 'nfun=' || count(*) FROM improv_functions();
SELECT 'pmtsig=' || signature || '|' || category || '|' || returns FROM improv_functions() WHERE name = 'PMT';
SELECT 'search=' || name FROM improv_functions(query := 'pmt') LIMIT 1;
-- improv(source, model) over a real table: Time x Product revenue. 2025's
-- WidgetA quantity comes as two rows (70 + 50) that \`sum\` adds.
CREATE TABLE sales(year INTEGER, product VARCHAR, price DECIMAL(10,2), qty INTEGER);
INSERT INTO sales VALUES
  (2024, 'WidgetA', 10, 100), (2024, 'WidgetB', 20, 50),
  (2025, 'WidgetA', 11, 70), (2025, 'WidgetA', 11, 50), (2025, 'WidgetB', 20, 40);
CREATE MACRO revenue_model() AS 'category Time = year
category Product = product
input Price = price max
input Quantity = qty sum
Revenue = Price * Quantity
Share = Revenue / SUM(Revenue OVER Product)
Growth = Revenue - PREV(Revenue, Time, 1, 0)';
SELECT 'cell=' || measure || '[' || "Time" || ',' || "Product" || ']=' || value_text
  FROM improv('sales', revenue_model());
SELECT 'ncells=' || count(*) FROM improv('sales', revenue_model());
-- A SELECT source, and a measure over fewer categories (NULL Product).
SELECT 'select=' || "Time" || ',' || coalesce("Product", 'NULL') || '=' || value
  FROM improv('SELECT * FROM sales WHERE year = 2025', 'category Time = year
category Product = product
input Quantity = qty sum
Total = SUM(Quantity OVER Product)');
-- Joins like any table.
SELECT 'join=' || string_agg(i."Product" || ':' || i.value_text, ',' ORDER BY i."Product")
  FROM improv('sales', revenue_model()) i JOIN (VALUES ('WidgetA')) p(name) ON p.name = i."Product"
 WHERE i.measure = 'Share' AND i."Time" = '2025';
SQL
)" || { echo "$out"; echo "FAIL: the smoke script errored"; exit 1; }

echo "$out"
expect() { grep -qxF -- "$1" <<<"$out" || { echo "FAIL: expected line '$1'"; exit 1; }; }
expect "math=141.421"
expect "pmt=1199.1"
expect "fv=15528.23"
expect "rate=0.06"
expect "text=6.0"
expect "date=29.0"
expect "serial=45322.0"
expect "if=8.0"
expect "bool=1.0"
expect "err=NULL"
expect "textval=NULL"
expect "nullformula=NULL"
expect "nullinput=NULL"
expect "lazy=5.0"
expect "rows=333283335000.0"
expect "ttext=ADA!"
expect "tif=small"
expect "tdiv=#DIV/0!"
expect "tnum=#NUM!"
expect "tna=#N/A"
expect "tdate=2024-02-29"
expect "nfun=674"
expect "pmtsig=PMT(rate, nper, pv, [fv], [type])|financial|number"
expect "search=PMT"
expect "cell=Revenue[2024,WidgetA]=1000"
expect "cell=Revenue[2024,WidgetB]=1000"
expect "cell=Revenue[2025,WidgetA]=1320"
expect "cell=Revenue[2025,WidgetB]=800"
expect "cell=Share[2024,WidgetA]=0.5"
expect "cell=Share[2025,WidgetA]=0.622641509433962"
expect "cell=Share[2025,WidgetB]=0.377358490566038"
expect "cell=Growth[2024,WidgetA]=1000"
expect "cell=Growth[2025,WidgetA]=320"
expect "cell=Growth[2025,WidgetB]=-200"
expect "ncells=12"
expect "select=2025,NULL=160.0"
expect "join=WidgetA:0.622641509433962"

# Error paths: each fails the query with a recognisable message.
expect_err() {
  local sql="$1" pat="$2" o
  if o="$(run "$sql")"; then echo "FAIL: expected an error for: $sql"; echo "$o"; exit 1; fi
  grep -qF -- "$pat" <<<"$o" || { echo "FAIL: error for '$sql' lacks '$pat':"; echo "$o"; exit 1; }
  echo "ok (error): $pat"
}
expect_err "SELECT improv_calc('1 +');" "improv_calc: parse error"
expect_err "SELECT improv_calc('y', json_object('x', 1));" "unknown"
expect_err "SELECT improv_calc('1', '{nope');" "not valid JSON"
expect_err "SELECT * FROM improv('sales', 'category Time = year
this line is wrong
Q = 1');" "model line 2: expected \`category Name = column\`"
expect_err "SELECT * FROM improv('sales', 'category Time = nope
Q = 1');" 'model line 1: the source has no column "nope"'
expect_err "SELECT * FROM improv('sales', 'category Time = year
input Quantity = qty
Q = Quantity');" "model line 2: Quantity: two source rows have the coordinate (2024); add an aggregation"
expect_err "SELECT * FROM improv('sales', 'category Time = year
input Quantity = qty sum
Q = Quantity * Nope');" "model line 3: definition: parse error at"
expect_err "SELECT * FROM improv('no_such', 'category T = y
Q = 1');" 'improv: cannot read source "no_such"'

echo "PASS: improv DuckDB extension smoke test"
