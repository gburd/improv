#!/usr/bin/env bash
# The DuckDB extension CI job, as GitHub, Forgejo and a local run all
# execute it (crates/duckdb_improv, its own Cargo workspace):
#
#   ci/duckdb_improv.sh <cli-dir>    e.g. ci/duckdb_improv.sh "$HOME/duckdb-cli"
#
# fmt + clippy -D warnings; `make configure release test_release` (the
# SQLLogicTest file under Python duckdb 1.5.6); the same file and the
# concurrency test under Python duckdb 2.0.0; test/smoke.sh under the DuckDB
# v1.5.6 and v2.0.0 CLIs (ci/duckdb-clis.sh fetches them into <cli-dir>);
# the guide's DuckDB tutorial (test/tutorial.sh) under both CLIs; and
# improv(...) against `improv eval` cell by cell.
# One release binary must pass under both DuckDB versions (README "Pins").
#
# Needs: cargo, python3 with venv, curl, unzip, git.
set -euo pipefail
cli=${1:?usage: ci/duckdb_improv.sh <cli-dir>}
root=$(cd "$(dirname "$0")/.." && pwd)
crate="$root/crates/duckdb_improv"
ext="$crate/build/release/improv.duckdb_extension"

step() { echo; echo "::group::$*" 2>/dev/null || true; echo "== $*"; }
end() { echo "::endgroup::" 2>/dev/null || true; }

step "DuckDB CLIs"
"$root/ci/duckdb-clis.sh" "$cli"
end

cd "$crate"
step "fmt + clippy"
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
end

step "build (release) + SQLLogicTest under duckdb 1.5.6"
make configure release test_release
end

step "SQLLogicTest + concurrency under Python duckdb 2.0.0"
v20="$crate/configure/venv-v200"
if [ ! -x "$v20/bin/python3" ]; then
	python3 -m venv "$v20"
	"$v20/bin/pip" install -q "duckdb==2.0.0" packaging \
		"git+https://github.com/duckdb/duckdb-sqllogictest-python@2ac8dbc012ddbd96a57dca37784fd8ee3c0eb021"
fi
"$v20/bin/python3" -m duckdb_sqllogictest --test-dir test/sql --external-extension "$ext"
"$v20/bin/python3" test/concurrency.py "$ext"
configure/venv/bin/python3 test/concurrency.py "$ext"
end

for v in v1.5.6 v2.0.0; do
	step "smoke.sh under the DuckDB $v CLI"
	EXT="$ext" DUCKDB="$cli/$v/duckdb" bash test/smoke.sh
	end
done

for v in v1.5.6 v2.0.0; do
	step "tutorial.sh (the guide's DuckDB tutorial) under the DuckDB $v CLI"
	EXT="$ext" DUCKDB="$cli/$v/duckdb" bash test/tutorial.sh
	end
done

step "improv(...) matches \`improv eval\`"
(cd "$root" && cargo build -q -p improv_cli)
EXT="$ext" DUCKDB="$cli/v1.5.6/duckdb" IMPROV="$root/target/debug/improv" bash test/compare_cli.sh
end
