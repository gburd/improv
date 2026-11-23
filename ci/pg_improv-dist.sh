#!/usr/bin/env bash
# pg_improv's release tarball for one Postgres major, built on EL8 (glibc
# 2.28) for PG 15 to 18, or EL9 (glibc 2.34) for PG 19, which PGDG ships
# for EL9+ only. Run it in the matching manylinux container
# (MANYLINUX_IMAGE selects it; the default is manylinux_2_28):
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
	# PGDG ships postgresql<N>-devel per EL major. Postgres 19 is EL9+ only
	# (no RHEL 8 build), so it is built in the manylinux_2_34 (EL9) image;
	# older majors build in manylinux_2_28 (EL8). Match the repo RPM and
	# the CRB/powertools repo name to the running EL major.
	el=$(sed -n 's/^VERSION_ID="\?\([0-9]*\).*/\1/p' /etc/os-release | head -1)
	dnf -q -y install "https://download.postgresql.org/pub/repos/yum/reporpms/EL-$el-x86_64/pgdg-redhat-repo-latest.noarch.rpm"
	dnf -q -y module disable postgresql
	# postgresql<N>-devel needs perl-IPC-Run, from powertools (EL8) or crb
	# (EL9); cargo-pgrx needs openssl-devel.
	crb=powertools; [ "$el" -ge 9 ] && crb=crb
	dnf -q -y "--enablerepo=$crb" install "postgresql$pg-devel" clang clang-devel openssl-devel
fi
cargo install -q cargo-pgrx --version "$(sed -n 's/^pgrx = "=\{0,1\}\([0-9.]*\)"/\1/p' "$crate/Cargo.toml")" --locked
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
# PG 19 builds on EL9 (glibc 2.34, GCC 11's GLIBCXX 3.4.29); older majors on
# EL8 (glibc 2.28, GLIBCXX 3.4.25). The .so's floor matches the server's EL.
if [ "$pg" -ge 19 ]; then
	"$root/ci/glibc-floor.sh" 2.34 3.4.29 "$dlib/pg_improv.so"
else
	"$root/ci/glibc-floor.sh" 2.28 3.4.25 "$dlib/pg_improv.so"
fi
tar czf "$out/pg_improv-$ver-pg$pg-x86_64-linux.tar.gz" -C "$work/dist" .
tar tzf "$out/pg_improv-$ver-pg$pg-x86_64-linux.tar.gz"
