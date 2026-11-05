#!/usr/bin/env bash
# Fetch the two DuckDB CLIs the improv DuckDB extension is tested against:
# the v1.5.6 release (its C API floor) and v2.0.0, the release the DuckDB
# Community Extensions registry builds against (duckdb/community-extensions
# .github/workflows/build.yml: duckdb_commit d54cbbe3).
#
#   ci/duckdb-clis.sh <dir>   ->  <dir>/v1.5.6/duckdb  <dir>/v2.0.0/duckdb
#
# Linux x86_64 only (the release asset names).
set -euo pipefail
dir=${1:?usage: ci/duckdb-clis.sh <dir>}
V156=v1.5.6
V20=v2.0.0
V20_COMMIT=d54cbbe3df17e8e5ce68312f483b4155836112aa
mkdir -p "$dir/$V156" "$dir/$V20"

if [ ! -x "$dir/$V156/duckdb" ]; then
	curl -fsSL "https://github.com/duckdb/duckdb/releases/download/$V156/duckdb_cli-linux-amd64.zip" \
		-o "$dir/$V156/cli.zip"
	unzip -qo "$dir/$V156/cli.zip" -d "$dir/$V156"
	rm -f "$dir/$V156/cli.zip"
fi

if [ ! -x "$dir/$V20/duckdb" ]; then
	curl -fsSL "https://github.com/duckdb/duckdb/releases/download/$V20/duckdb-cli-linux-amd64.tar.gz" |
		tar -xz -C "$dir/$V20"
fi

"$dir/$V156/duckdb" --version
"$dir/$V20/duckdb" --version
"$dir/$V20/duckdb" -noheader -list -c "SELECT source_id FROM pragma_version()" | grep -q "^${V20_COMMIT:0:10}$"
