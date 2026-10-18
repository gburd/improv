#!/usr/bin/env bash
# Fetch the two DuckDB CLIs the improv DuckDB extension is tested against:
# the v1.5.6 release (its C API floor) and v2.0.0-alpha46057, the build the
# DuckDB Community Extensions registry targets (duckdb/community-extensions
# .github/workflows/build.yml). That alpha has no CLI download, so it is
# built from source at its commit; CI caches the result by commit.
#
#   ci/duckdb-clis.sh <dir>   ->  <dir>/v1.5.6/duckdb  <dir>/v2.0.0-alpha46057/duckdb
#
# Linux x86_64 only (the release asset name; the source build is portable).
set -euo pipefail
dir=${1:?usage: ci/duckdb-clis.sh <dir>}
V156=v1.5.6
V20=v2.0.0-alpha46057
V20_COMMIT=b15358c371daf10dad799a43b6167f6ccdce84d8
mkdir -p "$dir/$V156" "$dir/$V20"

if [ ! -x "$dir/$V156/duckdb" ]; then
	curl -fsSL "https://github.com/duckdb/duckdb/releases/download/$V156/duckdb_cli-linux-amd64.zip" \
		-o "$dir/$V156/cli.zip"
	unzip -qo "$dir/$V156/cli.zip" -d "$dir/$V156"
	rm -f "$dir/$V156/cli.zip"
fi

if [ ! -x "$dir/$V20/duckdb" ]; then
	src="$dir/src"
	if [ ! -d "$src/.git" ]; then
		git init -q "$src"
		git -C "$src" remote add origin https://github.com/duckdb/duckdb
	fi
	git -C "$src" fetch -q --depth 1 origin "$V20_COMMIT"
	git -C "$src" checkout -q FETCH_HEAD
	# OVERRIDE_GIT_DESCRIBE: a shallow clone has no tags to describe; the
	# CLI must report the registry's version string.
	(cd "$src" && OVERRIDE_GIT_DESCRIBE="$V20" GEN=ninja CORE_EXTENSIONS="json;parquet" \
		make release -j"$(nproc)")
	cp "$src/build/release/duckdb" "$dir/$V20/duckdb"
fi

"$dir/$V156/duckdb" --version
"$dir/$V20/duckdb" --version
"$dir/$V20/duckdb" -noheader -list -c "SELECT source_id FROM pragma_version()" | grep -q "^${V20_COMMIT:0:10}$"
