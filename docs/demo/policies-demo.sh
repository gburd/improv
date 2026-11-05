#!/usr/bin/env bash
# Record the access-policies movie: docs/demo/policies-demo.cast and
# docs/src/img/policies-demo.{gif,mp4,png}. One command, from the repo root:
#   docs/demo/policies-demo.sh
# It records in podman (docs/demo/pg-container.sh: rust:1.97-trixie, PGDG
# Postgres 17, pg_improv installed from this checkout), then renders on
# the host (docs/demo/render.sh: agg, gifsicle, ffmpeg, ImageMagick).
# About 4 minutes, plus the container's first build (about 10).
set -euo pipefail
if [ -z "${IMPROV_DEMO_IN_CONTAINER:-}" ]; then
	docs/demo/pg-container.sh docs/demo/policies-demo.sh
	# Three refusals are the point; anything else is a failed recording.
	n=$(grep -o 'ERROR: ' docs/demo/policies-demo.cast | wc -l)
	if [ "$n" != 3 ]; then
		echo "policies-demo.cast shows $n errors, not the 3 refusals" >&2
		exit 1
	fi
	exec docs/demo/render.sh policies-demo 79
fi

# In the container: a scratch cluster, removed on exit. improv (a superuser)
# owns the model; ann is the restricted role the movie creates.
ROOT=$(pwd)
D=$(mktemp -d /tmp/pgi-pol.XXXX)
export PGHOST=$D PGPORT=55443 PGUSER=improv
export PAGER=cat PSQLRC=/dev/null PSQL_HISTORY=/dev/null
export PATH=$PGBIN:$PATH
trap 'pg_ctl -D "$D/data" stop -m fast >/dev/null 2>&1 || true; find "$D" -delete' EXIT
initdb -D "$D/data" -A trust -U improv >/dev/null
cat >>"$D/data/postgresql.conf" <<CONF
port = $PGPORT
listen_addresses = ''
unix_socket_directories = '$D'
shared_preload_libraries = 'pg_improv'
improv.database = 'sales'
improv.poll_ms = 100
wal_level = logical
CONF
pg_ctl -D "$D/data" -l "$D/log" -w start >/dev/null 3>&-
createdb sales
# The model, off camera: pg-demo.sh shows how it is made.
psql -qX -v ON_ERROR_STOP=1 sales >/dev/null <<'SQL'
CREATE EXTENSION pg_improv;
CREATE TABLE sales (quarter text, product text, price float8, qty float8,
  PRIMARY KEY (quarter, product));
INSERT INTO sales VALUES ('Q1','Widget',10,100), ('Q2','Widget',10,200),
  ('Q1','Gadget',20,30), ('Q2','Gadget',20,60), ('Q1','Falcon',50,2), ('Q2','Falcon',50,4);
SELECT improv.create_model('plan');
SELECT improv.bind_category('plan', 'Time', 'sales', 'quarter'),
  improv.bind_category('plan', 'Product', 'sales', 'product'),
  improv.bind_measure('plan', 'Price', 'sales', 'price'),
  improv.bind_measure('plan', 'Quantity', 'sales', 'qty');
SELECT improv.define('plan', 'Revenue = Price * Quantity');
SQL
for _ in $(seq 100); do
	[ "$(psql -qAtX sales -c 'SELECT count(*) FROM improv_plan.revenue')" = 6 ] && break
	sleep 0.1
done

python3 "$ROOT/docs/demo/record.py" "$ROOT/docs/demo/policies-demo.cast" --size 100x30 \
	-- psql sales <<'STEPS'
expect sales=# $
comment --
pace 0.03 0.07
say As improv, the owner: the plan model over a sales table
type SELECT id, name FROM improv.items('plan');
send \r
expect sales=# $
sleep 2.5
type SELECT * FROM improv_plan.revenue ORDER BY product, time;
send \r
expect sales=# $
sleep 3
clear sales=# 
say Policies in Cedar: ann reads all but Falcon (item 3), edits Quantity
type SELECT improv.set_policies('plan', $$
send \r
type   @id("ann-reads") permit (principal == Improv::User::"ann",
send \r
type     action in Improv::Action::"Read", resource);
send \r
type   @id("no-falcon") forbid (principal,
send \r
type     action in Improv::Action::"Read", resource in Improv::Item::"3");
send \r
type   @id("ann-edits") permit (principal == Improv::User::"ann",
send \r
type     action == Improv::Action::"EditCell", resource in Improv::Measure::"2");
send \r
type $$);
send \r
expect sales=# $
sleep 2.5
say ann may call the filtered functions, and read the Revenue table
type CREATE ROLE ann LOGIN; GRANT improv_reader TO ann;
send \r
expect sales=# $
type GRANT USAGE ON SCHEMA improv_plan TO ann;
send \r
expect sales=# $
type GRANT SELECT ON improv_plan.revenue TO ann;
send \r
expect sales=# $
sleep 1.5
clear sales=# 
say Now as ann: the same SELECT, filtered by row-level security
type SET ROLE ann;
send \r
expect sales=> $
type SELECT * FROM improv_plan.revenue ORDER BY product, time;
send \r
expect sales=> $
sleep 3.5
say improv.cells lists only the cells ann may read
type SELECT * FROM improv.cells('plan', 'Quantity');
send \r
expect sales=> $
sleep 3.5
clear sales=> 
say A hidden item answers exactly as one that does not exist
type SELECT improv.set_cells('plan', '[{"measure": "Quantity",
send \r
type   "coord": {"Time": "Q1", "Product": "Falcon"}, "value": 5}]');
send \r
expect sales=> $
sleep 2
type SELECT improv.set_cells('plan', '[{"measure": "Quantity",
send \r
type   "coord": {"Time": "Q1", "Product": "Nope"}, "value": 5}]');
send \r
expect sales=> $
sleep 3
say May ann edit a Price cell? explain_access asks the policies
type SELECT action, resource, allowed FROM improv.explain_access('plan', 'EditCell',
send \r
type   '{"measure": "Price", "at": {"Time": "Q1", "Product": "Widget"}}');
send \r
expect sales=> $
sleep 3
type SELECT improv.set_cells('plan', '[{"measure": "Price",
send \r
type   "coord": {"Time": "Q1", "Product": "Widget"}, "value": 12}]');
send \r
expect sales=> $
sleep 3
clear sales=> 
say A Quantity cell is allowed: written to sales as the owner
type SELECT improv.set_cells('plan', '[{"measure": "Quantity",
send \r
type   "coord": {"Time": "Q1", "Product": "Widget"}, "value": 150}]');
send \r
expect sales=> $
until [ "$(psql -qAtX sales -c "SELECT value FROM improv_plan.revenue WHERE time = 'Q1' AND product = 'Widget'")" = 1500 ]
sleep 1
type SELECT * FROM improv_plan.revenue ORDER BY product, time;
send \r
expect sales=> $
sleep 3.5
clear sales=> 
say Back as the owner, who may ask about ann and see which policies decided
type RESET ROLE;
send \r
expect sales=# $
type SELECT step, action, resource, allowed, policies FROM improv.explain_access('plan',
send \r
type   'ReadCell', '{"measure": "Revenue", "at": {"Time": "Q1", "Product": "Falcon"}}', 'ann');
send \r
expect sales=# $
sleep 4
say Every change made through the policies is audited
type SELECT at::time(0), who, action, resource FROM improv.policy_audit;
send \r
expect sales=# $
sleep 5
send \\q\r
sleep 0.5
STEPS
