#!/usr/bin/env bash
# Build pg_improv's PGXN source distribution: pg_improv-<version>.zip.
#
#   ci/pgxn-dist.sh [PG_FEATURE]    (default pg17; run from anywhere)
#
# pg_improv is a pgrx (Rust) extension inside the Improv workspace and links
# six sibling crates, so the dist is a buildable subset of the repository,
# not a PGXS tree: `cargo pgrx install` from crates/pg_improv builds it.
#   1. reads the version from crates/pg_improv/Cargo.toml (the one source),
#   2. renders META.json (repo root) from crates/pg_improv/META.json.in,
#   3. generates the install script crates/pg_improv/sql/pg_improv--V.sql
#      with `cargo pgrx schema` (PGXN's provides.file points at it),
#   4. checks the generated SQL with pgspot when it is on PATH,
#   5. zips META.json, the licences and the crates pg_improv needs.
# The release workflow uploads the zip with pgxn-release; CI builds it on
# every push so a broken dist is caught before a tag.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
feature=${1:-pg17}
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' crates/pg_improv/Cargo.toml | head -1)
[ -n "$ver" ] || { echo "pgxn-dist: no version in crates/pg_improv/Cargo.toml" >&2; exit 1; }
dist="pg_improv-$ver"
echo "pgxn-dist: $dist ($feature)"

sed "s/@VERSION@/$ver/g" crates/pg_improv/META.json.in >META.json
python3 -c 'import json,sys; m=json.load(open("META.json")); assert m["version"]==sys.argv[1], m["version"]; assert m["provides"]["pg_improv"]["version"]==sys.argv[1]' "$ver"

sql="crates/pg_improv/sql/pg_improv--$ver.sql"
(cd crates/pg_improv && cargo pgrx schema "$feature" --out "sql/pg_improv--$ver.sql" >/dev/null)
[ -s "$sql" ] || { echo "pgxn-dist: $sql was not generated" >&2; exit 1; }
if command -v pgspot >/dev/null; then
	pgspot --ignore PS001 --ignore PS016 "$sql" crates/pg_improv/sql/pg_improv--*--"$ver".sql
fi

# The crates the extension builds from (cargo metadata: path dependencies).
crates=$(cd crates/pg_improv && cargo metadata --format-version 1 --no-deps 2>/dev/null >/dev/null &&
	cargo metadata --format-version 1 | python3 -c '
import json, os, sys
m = json.load(sys.stdin)
for p in m["packages"]:
    if p["source"] is None:
        print(os.path.relpath(os.path.dirname(p["manifest_path"]), sys.argv[1]))
' "$root" | sort -u)
stage=$(mktemp -d)
trap 'find "$stage" -delete' EXIT
mkdir -p "$stage/$dist"
# Root manifest (workspace deps/versions the crates inherit), licences, docs.
cp META.json Cargo.toml Cargo.lock LICENSE-APACHE LICENSE-MIT README.md CHANGELOG.md "$stage/$dist/"
for c in $crates; do
	mkdir -p "$stage/$dist/$c"
	(cd "$c" && git ls-files -z | xargs -0 -I{} cp --parents {} "$stage/$dist/$c/")
done
cp "$sql" "$stage/$dist/$sql"
# A trimmed workspace: only the crates in the dist (so `cargo pgrx install`
# from the dist resolves without the GUI, server, storage crates, ...).
python3 - "$stage/$dist/Cargo.toml" $crates <<'PY'
import re, sys
p, crates = sys.argv[1], [c for c in sys.argv[2:] if c != "crates/pg_improv"]
s = open(p).read()
members = "members = [\n" + "".join(f'    "{c}",\n' for c in crates) + "]"
s = re.sub(r"members = \[.*?\]", members, s, count=1, flags=re.S)
open(p, "w").write(s)
PY
test -f "$stage/$dist/crates/pg_improv/pg_improv.control"
test -f "$stage/$dist/$sql"
rm -f "$dist.zip"
(cd "$stage" && zip -qr "$root/$dist.zip" "$dist")
echo "pgxn-dist: wrote $dist.zip ($(du -h "$dist.zip" | cut -f1)): $(echo $crates | tr '\n' ' ')"
