#!/usr/bin/env bash
# The pg_improv CI job for one Postgres major, as GitHub, Forgejo and a
# local run all execute it:
#
#   ci/pg_improv.sh <PGMAJOR>      e.g. ci/pg_improv.sh 18
#
# Run as root inside a Debian container (rust:*-trixie) or on a box that
# already has the PGDG packages. It installs Postgres <PGMAJOR> from PGDG,
# builds pg_improv for it with pgrx, and runs, in order:
#   fmt + clippy -D warnings, the #[pg_test] suite, pgspot on the generated
#   and update SQL, tests/txn.sh (no-preload CREATE EXTENSION, the schema
#   check, transactional semantics, dump/restore), tests/logical.sh (the
#   sync worker end to end), and on PG 17 also tests/upgrade.sh (ALTER
#   EXTENSION UPDATE from each earlier release) and the PGXN dist build.
# Steps that need a non-root user (pgrx refuses initdb as root) run as `pgi`.
set -euo pipefail
pg=${1:?usage: ci/pg_improv.sh <PGMAJOR>}
root=$(cd "$(dirname "$0")/.." && pwd)
crate="$root/crates/pg_improv"
PG_CONFIG=/usr/lib/postgresql/$pg/bin/pg_config
export DEBIAN_FRONTEND=noninteractive

step() { echo; echo "::group::$*" 2>/dev/null || true; echo "== $*"; }
end() { echo "::endgroup::" 2>/dev/null || true; }

if [ ! -x "$PG_CONFIG" ]; then
	step "install Postgres $pg (PGDG) and build deps"
	apt-get update -qq
	apt-get install -y -qq --no-install-recommends postgresql-common ca-certificates curl gnupg >/dev/null
	/usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y >/dev/null
	apt-get install -y -qq --no-install-recommends "postgresql-$pg" "postgresql-server-dev-$pg" \
		libclang-dev clang sudo zip python3 python3-venv git >/dev/null
	end
fi

step "cargo-pgrx, pgspot"
rustup component add rustfmt clippy >/dev/null 2>&1 || true
command -v cargo-pgrx >/dev/null || cargo install cargo-pgrx --version 0.17.0 --locked
if ! command -v pgspot >/dev/null; then
	python3 -m venv /opt/pgspot && /opt/pgspot/bin/pip install -q pgspot==0.9.2
	export PATH="/opt/pgspot/bin:$PATH"
fi
id pgi >/dev/null 2>&1 || useradd -m pgi
chmod -R a+w "/usr/share/postgresql/$pg/extension" "/usr/lib/postgresql/$pg/lib"
chown -R pgi "$root" "${CARGO_HOME:-$HOME/.cargo}"
git config --global --add safe.directory "$root"
run() { sudo -u pgi env PATH="$PATH" CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}" \
	RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}" CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-}" "$@"; }
run sh -c "git config --global --add safe.directory '$root'"
run cargo pgrx init "--pg$pg" "$PG_CONFIG" >/dev/null
end

cd "$crate"
feat=(--no-default-features --features "pg$pg")
step "fmt + clippy (pg$pg)"
cargo fmt --check
run cargo clippy "${feat[@]}" --all-targets -- -D warnings
end

step "pg_test (pg$pg)"
run cargo pgrx test "pg$pg" "${feat[@]}"
end

step "pgspot"
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
run cargo pgrx schema "pg$pg" "${feat[@]}" --out "/tmp/pg_improv--$ver.sql" >/dev/null
# PS001/PS016 (unqualified operators and built-in calls in CHECK constraints and
# defaults) are noise for objects created in the extension's own schema.
pgspot --ignore PS001 --ignore PS016 "/tmp/pg_improv--$ver.sql" sql/pg_improv--*--*.sql
end

step "install, tests/txn.sh (pg$pg)"
run cargo pgrx install --release "${feat[@]}" --pg-config "$PG_CONFIG"
run env PG_BIN="/usr/lib/postgresql/$pg/bin" ./tests/txn.sh
end

step "tests/logical.sh (pg$pg)"
run env PG_BIN="/usr/lib/postgresql/$pg/bin" N_SINGLE=10 N_BULK=3 ./tests/logical.sh
end

if [ "$pg" = 17 ]; then
	step "tests/upgrade.sh: ALTER EXTENSION UPDATE from earlier releases"
	run env PG_CONFIG="$PG_CONFIG" ./tests/upgrade.sh
	end
	step "PGXN dist"
	run "$root/ci/pgxn-dist.sh" "pg$pg"
	end
fi
echo "pg_improv CI on PG $pg: PASS"
