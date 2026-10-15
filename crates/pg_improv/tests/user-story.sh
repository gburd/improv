#!/usr/bin/env bash
# The pg_improv user story, end to end:
#   1. Postgres with pg_improv preloaded (logical mode default)
#   2. a model defined in SQL over a fact table
#   3. a committed UPDATE flows to the result tables via logical decoding
#   4. improv-gui opens the same model from postgres:// and shows it
set -euo pipefail
B=/usr/lib/postgresql/17/bin
D=$HOME/pgi-story
P=55440
OUT=${1:-$HOME/pgi-story.png}
stop() {
	"$B/pg_ctl" -D "$D" stop -m fast >/dev/null 2>&1 || true
	if [ -d "$D" ]; then find "$D" -mindepth 1 -delete && rmdir "$D"; fi
}
trap stop EXIT
stop
"$B/initdb" -D "$D" -A trust -U improv >/dev/null
cat >>"$D/postgresql.conf" <<EOF
port = $P
listen_addresses = '127.0.0.1'
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'sales'
improv.poll_ms = 100
wal_level = logical
EOF
"$B/pg_ctl" -D "$D" -l "$D/log" -w start >/dev/null
"$B/createdb" -h "$D" -p $P -U improv sales
q() { "$B/psql" -h "$D" -p $P -U improv -d sales -qAtX -v ON_ERROR_STOP=1 -c "$1"; }

echo "1. CREATE EXTENSION pg_improv"
q "CREATE EXTENSION pg_improv"
q "CREATE TABLE sales (quarter text, product text, price float8, qty float8, PRIMARY KEY (quarter, product))"
q "INSERT INTO sales SELECT 'Q'||q, p, CASE p WHEN 'Widget' THEN 10 ELSE 20 END,
     CASE p WHEN 'Widget' THEN 100*q ELSE 30*q END
   FROM generate_series(1,4) q, unnest(ARRAY['Widget','Gadget']) p"

echo "2. define the model in SQL"
for s in "create_model('plan')" \
	"bind_category('plan','Time','sales','quarter')" \
	"bind_category('plan','Product','sales','product')" \
	"bind_measure('plan','Price','sales','price')" \
	"bind_measure('plan','Quantity','sales','qty')" \
	"define('plan','Revenue = Price * Quantity')" \
	"define('plan','Total = SUM(Revenue OVER Time)')"; do
	q "SELECT improv.$s" >/dev/null
done
for _ in $(seq 1 100); do [ "$(q "SELECT count(*) FROM improv_plan.revenue")" = 8 ] && break; sleep 0.1; done
q "SELECT product, value FROM improv_plan.total ORDER BY 1" | sed 's/^/   total: /'

echo "3. UPDATE + COMMIT; the worker applies it"
q "UPDATE sales SET price = 12 WHERE quarter='Q1' AND product='Widget'"
for _ in $(seq 1 100); do [ "$(q "SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Widget'")" = 1200 ] && break; sleep 0.1; done
q "SELECT time, product, value FROM improv_plan.revenue WHERE product='Widget' ORDER BY 1" | sed 's/^/   revenue: /'
q "SELECT sync_mode, slot, rows_applied, last_error IS NULL FROM improv.status('plan')" | sed 's/^/   status: /'

URL="postgres://improv@127.0.0.1:$P/sales?sslmode=disable&model=plan"
echo "4. improv list $URL"
"$HOME/improv/target/release/improv" list "$URL" | sed -n '1,12p' | sed 's/^/   /'

echo "5. improv-gui $URL (headless screenshot)"
DISP=:$(( 90 + RANDOM % 9 ))
Xvfb "$DISP" -screen 0 1400x900x24 >/dev/null 2>&1 &
XP=$!
sleep 1
DISPLAY=$DISP WAYLAND_DISPLAY= LIBGL_ALWAYS_SOFTWARE=1 "$HOME/improv/target/release/improv-gui" "$URL" >"$D/gui.log" 2>&1 &
GP=$!
for _ in $(seq 1 60); do DISPLAY=$DISP xdotool search --name Improv >/dev/null 2>&1 && break; sleep 0.5; done
W=$(DISPLAY=$DISP xdotool search --name Improv | head -1)
DISPLAY=$DISP xdotool windowsize "$W" 1400 900
sleep 3
DISPLAY=$DISP xwd -root -silent | magick xwd:- -strip "$OUT"
kill $GP $XP 2>/dev/null || true
echo "   screenshot: $OUT"
echo "PASS"
