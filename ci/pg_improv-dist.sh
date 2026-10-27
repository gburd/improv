#!/usr/bin/env bash
# pg_improv's release tarball for one Postgres major, built on EL8 so the
# .so needs at most glibc 2.28. Run it in the manylinux_2_28 container:
#
#   ci/manylinux.sh ci/pg_improv-dist.sh <PGMAJOR> <out-dir>
#
# Builds with `cargo pgrx package` against yum.postgresql.org's
# postgresql<N>-devel (/usr/pgsql-<N>/bin/pg_config) and AppStream's
# libclang, then lays the files out as Debian's PGDG paths, which
# ci/packages.sh expects (and moves to /usr/pgsql-<N> for the .rpm):
#   <out-dir>/pg_improv-<ver>-pg<N>-x86_64-linux.tar.gz
#     ./usr/lib/postgresql/<N>/lib/pg_improv.so
#     ./usr/share/postgresql/<N>/extension/pg_improv.control, *.sql
# Postgres' module ABI is the major's, not the distribution's: the same .so
# loads into apt.postgresql.org's server (ci/install-test.sh checks both).
set -euo pipefail
pg=${1:?usage: ci/pg_improv-dist.sh <PGMAJOR> <out-dir>} out=${2:?usage: ci/pg_improv-dist.sh <PGMAJOR> <out-dir>}
root=$(cd "$(dirname "$0")/.." && pwd)
crate=$root/crates/pg_improv
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' "$crate/Cargo.toml" | head -1)
pgc=/usr/pgsql-$pg/bin/pg_config
if [ ! -x "$pgc" ]; then
	dnf -q -y install https://download.postgresql.org/pub/repos/yum/reporpms/EL-8-x86_64/pgdg-redhat-repo-latest.noarch.rpm
	dnf -q -y module disable postgresql
	# postgresql<N>-devel needs perl-IPC-Run, from powertools; cargo-pgrx
	# needs openssl-devel.
	dnf -q -y --enablerepo=powertools install "postgresql$pg-devel" clang clang-devel openssl-devel
fi
command -v cargo-pgrx >/dev/null || cargo install cargo-pgrx --version 0.17.0 --locked
mkdir -p "$out"
out=$(cd "$out" && pwd)
work=$(mktemp -d)
trap 'find "$work" -delete' EXIT
cd "$crate"
cargo pgrx init "--pg$pg" "$pgc"
cargo pgrx package --no-default-features --features "pg$pg" --pg-config "$pgc" --out-dir "$work/pgrx"
lib=$work/pgrx$("$pgc" --pkglibdir) ext=$work/pgrx$("$pgc" --sharedir)/extension
dlib=$work/dist/usr/lib/postgresql/$pg/lib dext=$work/dist/usr/share/postgresql/$pg/extension
mkdir -p "$dlib" "$dext"
cp -p "$lib"/* "$dlib/"
cp -p "$ext"/* "$dext/"
# Everything pgrx wrote is in one of the two directories.
test "$(find "$work/pgrx" -type f | wc -l)" = "$(find "$work/dist" -type f | wc -l)"
test -f "$dlib/pg_improv.so"
test -f "$dext/pg_improv--$ver.sql"
"$root/ci/glibc-floor.sh" 2.28 3.4.25 "$dlib/pg_improv.so"
tar czf "$out/pg_improv-$ver-pg$pg-x86_64-linux.tar.gz" -C "$work/dist" .
tar tzf "$out/pg_improv-$ver-pg$pg-x86_64-linux.tar.gz"
