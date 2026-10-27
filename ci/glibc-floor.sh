#!/usr/bin/env bash
# Fail when an ELF file needs a newer glibc or libstdc++ than a floor:
#
#   ci/glibc-floor.sh MAX_GLIBC MAX_GLIBCXX FILE...
#   ci/glibc-floor.sh 2.28 3.4.25 improv improv-tui pg_improv.so
#
# Reads the version-needed (verneed) entries, weak ones included: the loader
# refuses a missing non-weak version and warns about a missing weak one.
# Prints each file's newest GLIBC_ and GLIBCXX_ need, and exits 1 if any is
# above its maximum. 2.28 / 3.4.25 is RHEL 8 (glibc 2.28, GCC 8's libstdc++).
set -euo pipefail
[ $# -ge 3 ] || { echo "usage: $0 MAX_GLIBC MAX_GLIBCXX FILE..." >&2; exit 2; }
max_glibc=$1 max_glibcxx=$2
shift 2
newest() { { grep -oE "Name: $1_[0-9][0-9.]*" || true; } | cut -d_ -f2- | sort -uV | tail -n 1; }
# 0 when version $1 <= $2.
le() { [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$2" ]; }
fail=0
for f in "$@"; do
	needs=$(readelf -V --wide "$f") || { echo "glibc-floor: cannot read $f" >&2; exit 2; }
	g=$(newest GLIBC <<<"$needs") x=$(newest GLIBCXX <<<"$needs")
	[ -n "$g" ] || { echo "glibc-floor: $f needs no GLIBC version (not a dynamic ELF?)" >&2; exit 2; }
	bad=""
	le "$g" "$max_glibc" || bad=" GLIBC_$g>$max_glibc"
	if [ -n "$x" ] && ! le "$x" "$max_glibcxx"; then bad="$bad GLIBCXX_$x>$max_glibcxx"; fi
	printf '%s: GLIBC_%s%s  %s\n' "$f" "$g" "${x:+ GLIBCXX_$x}" "${bad:+FAIL:}${bad:-ok}"
	[ -z "$bad" ] || fail=1
done
exit $fail
