#!/usr/bin/env bash
# Install-test the Linux packages (ci/packages.sh's output) on every
# distribution the release supports, each in a clean podman container:
#
#   ci/install-test.sh <ver> <pkg-dir> [TEST...]
#   ci/install-test.sh 0.12.0 pkg                  # the whole matrix
#   ci/install-test.sh 0.12.0 pkg deb:debian:12 pg-rpm:rockylinux:9:17
#
# Tests (the default runs them all):
#   deb:<image>         the improv .deb: debian:12, debian:13, ubuntu:22.04, ubuntu:24.04
#   rpm:<image>         the improv .rpm: rockylinux:8, rockylinux:9, almalinux:10, fedora:latest
#   pg-deb:<image>:<N>  postgresql-<N>-improv on apt.postgresql.org's postgresql-<N>
#   pg-rpm:<image>:<N>  postgresql<N>-improv on yum.postgresql.org's postgresql<N>-server
# pg_improv runs PG 17 on debian:12, ubuntu:22.04, rockylinux:8 and
# rockylinux:9, and PG 15, 16 and 18 on debian:12 and rockylinux:8.
#
# improv: --version, a model built and evaluated with the CLI, improv-tui
# --version, improv-server --help, improv-gui --version (it parses its
# arguments before it needs a display). pg_improv: initdb, start, CREATE
# EXTENSION, a model over a table, recalc, its result read back. Both
# formats carry the same pg_improv .so (built on EL8 by
# ci/pg_improv-dist.sh), so this also proves it loads into Debian's and
# EL's server builds of the major.
#
# In the container this script runs itself as the check:
#   install-test.sh --check improv <ver>
#   install-test.sh --check pg <ver> <pg-bin-dir>    (as the postgres user)
set -euo pipefail

if [ "${1:-}" = --check ]; then
	what=$2 ver=$3
	os=$(sed -n "s/^PRETTY_NAME=//p" /etc/os-release | tr -d '"')
	case $what in
	improv)
		test "$(improv --version)" = "improv $ver"
		d=$(mktemp -d) && m=$d/m.db
		improv init "$m" >/dev/null
		improv add-category "$m" 1 Time >/dev/null
		improv add-item "$m" 10 1 Q1 >/dev/null
		improv add-measure "$m" 100 Price number input Time >/dev/null
		improv set "$m" 100 7.5 --at Time=Q1 >/dev/null
		improv add-derived "$m" 101 Twice "Price * 2" >/dev/null
		out=$(improv eval "$m" 101)
		grep -qx '  Twice\[Time=Q1\] = 15' <<<"$out" || { echo "improv eval: $out" >&2; exit 1; }
		test "$(improv-tui --version)" = "improv-tui $ver"
		improv-server --help >/dev/null
		test "$(improv-gui --version)" = "improv-gui $ver"
		echo "ok: improv $ver on $os, $(ldd --version | head -1)"
		;;
	pg)
		bin=$4 d=$(mktemp -d)
		"$bin/initdb" -D "$d/data" -A trust -U improv >/dev/null
		"$bin/pg_ctl" -D "$d/data" -l "$d/log" -o "-k $d -c listen_addresses='' -c wal_level=logical" -w start >/dev/null ||
			{ cat "$d/log" >&2; exit 1; }
		q() { "$bin/psql" -h "$d" -U improv -d postgres -qAtX -v ON_ERROR_STOP=1 -c "$1"; }
		q "CREATE EXTENSION pg_improv"
		q "CREATE TABLE sales (quarter text, product text, price float8, qty float8, PRIMARY KEY (quarter, product))"
		q "INSERT INTO sales VALUES ('Q1','Widget',10,100), ('Q1','Gadget',20,30)"
		for s in "create_model('plan')" "set_sync_mode('plan','manual')" \
			"bind_category('plan','Time','sales','quarter')" "bind_category('plan','Product','sales','product')" \
			"bind_measure('plan','Price','sales','price')" "bind_measure('plan','Quantity','sales','qty')" \
			"define('plan','Revenue = Price * Quantity')" "recalc('plan')"; do
			q "SELECT improv.$s" >/dev/null
		done
		v=$(q "SELECT value FROM improv_plan.revenue WHERE time='Q1' AND product='Gadget'")
		[ "$v" = 600 ] || { echo "revenue: got '$v', want 600" >&2; tail -20 "$d/log" >&2; exit 1; }
		e=$(q "SELECT extversion FROM pg_extension WHERE extname = 'pg_improv'")
		[ "$e" = "$ver" ] || { echo "extversion: got '$e', want $ver" >&2; exit 1; }
		echo "ok: pg_improv $e on $(q "SELECT version()" | cut -d, -f1), $os"
		"$bin/pg_ctl" -D "$d/data" -m fast -w stop >/dev/null
		;;
	*) echo "install-test: unknown check $what" >&2; exit 2 ;;
	esac
	exit 0
fi

usage="usage: ci/install-test.sh <ver> <pkg-dir> [TEST...]"
ver=${1:?$usage}
pkg=$(cd "${2:?$usage}" && pwd)
ci=$(cd "$(dirname "$0")" && pwd)
shift 2
tests=("$@")
if [ ${#tests[@]} -eq 0 ]; then
	tests=(deb:debian:12 deb:debian:13 deb:ubuntu:22.04 deb:ubuntu:24.04
		rpm:rockylinux:8 rpm:rockylinux:9 rpm:almalinux:10 rpm:fedora:latest
		pg-deb:debian:12:17 pg-deb:ubuntu:22.04:17 pg-rpm:rockylinux:8:17 pg-rpm:rockylinux:9:17
		pg-deb:debian:12:15 pg-deb:debian:12:16 pg-deb:debian:12:18
		pg-rpm:rockylinux:8:15 pg-rpm:rockylinux:8:16 pg-rpm:rockylinux:8:18)
fi
image() {
	case $1 in
	rockylinux:*) echo "docker.io/rockylinux/rockylinux:${1#*:}" ;; # Docker Hub's library image stops at 9
	fedora:*) echo "registry.fedoraproject.org/$1" ;;
	*) echo "docker.io/library/$1" ;;
	esac
}
apt='export DEBIAN_FRONTEND=noninteractive; apt-get update -qq; apt-get install -y -qq --no-install-recommends'
check="bash /ci/install-test.sh --check"
fail=0
for t in "${tests[@]}"; do
	kind=${t%%:*} rest=${t#*:}
	case $kind in
	deb) img=$rest script="$apt /pkg/improv_${ver}-1_amd64.deb >/dev/null; $check improv $ver" ;;
	rpm) img=$rest script="dnf -q -y install /pkg/improv-${ver}-1.x86_64.rpm >/dev/null; $check improv $ver" ;;
	pg-deb)
		pg=${rest##*:} img=${rest%:*}
		script="$apt postgresql-common ca-certificates gnupg >/dev/null
			/usr/share/postgresql-common/pgdg/apt.postgresql.org.sh -y >/dev/null
			$apt postgresql-$pg /pkg/postgresql-${pg}-improv_${ver}-1_amd64.deb >/dev/null
			su postgres -s /bin/bash -c '$check pg $ver /usr/lib/postgresql/$pg/bin'"
		;;
	pg-rpm)
		pg=${rest##*:} img=${rest%:*}
		el=${img#*:} && el=${el%%.*}
		script="dnf -q -y install https://download.postgresql.org/pub/repos/yum/reporpms/EL-$el-x86_64/pgdg-redhat-repo-latest.noarch.rpm >/dev/null
			{ dnf -q -y module disable postgresql || true; } >/dev/null 2>&1
			dnf -q -y install postgresql${pg}-server /pkg/postgresql${pg}-improv-${ver}-1.x86_64.rpm >/dev/null
			su postgres -s /bin/bash -c '$check pg $ver /usr/pgsql-$pg/bin'"
		;;
	*) echo "install-test: unknown test $t" >&2; exit 2 ;;
	esac
	echo "== $t"
	podman run --rm -v "$pkg:/pkg:ro" -v "$ci:/ci:ro" "$(image "$img")" bash -c "set -euo pipefail; $script" ||
		{ echo "FAIL: $t"; fail=1; }
done
exit $fail
