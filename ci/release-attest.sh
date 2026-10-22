#!/usr/bin/env bash
# Release attestation for release.yml's `release` job: a CycloneDX SBOM per
# shipped binary, SHA256SUMS over every artifact (SBOMs included), and a
# cosign keyless signature of SHA256SUMS.
#
# Usage: ci/release-attest.sh TAG DIST
#   TAG   the release tag (vX.Y.Z) the archives are named for
#   DIST  the directory of release archives; the SBOMs, SHA256SUMS and
#         SHA256SUMS.sigstore.json are written next to them
# ATTEST_SIGN=keyless signs with cosign through the GitHub Actions OIDC token
# (only in Actions, with `id-token: write`); the default, none, does not sign.
# Needs cargo-cyclonedx and sha256sum, and cosign to sign.
set -euo pipefail
export LC_ALL=C

usage="usage: ci/release-attest.sh TAG DIST"
tag=${1:?$usage}
dist=$(cd "${2:?$usage}" && pwd)
ver=${tag#v}
root=$(cd "$(dirname "$0")/.." && pwd)
sign=${ATTEST_SIGN:-none}
linux=x86_64-unknown-linux-gnu
# The SBOMs' timestamp is the commit's, so a rerun writes the same files.
SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$root" log -1 --format=%ct)}
export SOURCE_DATE_EPOCH

die() {
	echo "release-attest: $*" >&2
	exit 1
}

case $sign in none | keyless) ;; *) die "ATTEST_SIGN=$sign: want keyless or none" ;; esac

# cdx DIR TARGET [ARGS]: SBOMs for DIR's binaries and cdylibs, written by
# cargo cyclonedx into each crate's directory as <name>_<kind>_<TARGET>.cdx.json.
cdx() {
	local dir=$1 target=$2
	shift 2
	cargo cyclonedx -q --manifest-path "$root/$dir/Cargo.toml" --describe binaries \
		--format json --spec-version 1.5 --target "$target" --target-in-filename "$@"
}

# take FILE NAME: move one generated SBOM into DIST as NAME.
take() {
	[[ -f $1 ]] || die "cargo cyclonedx wrote no ${1#"$root"/}"
	mv "$1" "$dist/$2"
}

# Drop the SBOMs cargo cyclonedx wrote that do not ship (build helpers).
tidy() {
	find "$root/crates" -maxdepth 2 -name "*_$1.cdx.json" -delete
}

shopt -s nullglob
n=0
for f in "$dist"/*; do
	name=${f##*/}
	case $name in
	SHA256SUMS* | *.cdx.json) continue ;; # this script's output, on a rerun
	"improv-$ver-duckdb-linux_amd64.tar.gz")
		cdx crates/duckdb_improv $linux
		take "$root/crates/duckdb_improv/duckdb_improv_cdylib_$linux.cdx.json" "${name%.tar.gz}.cdx.json"
		;;
	"pg_improv-$ver-pg"*-x86_64-linux.tar.gz)
		pg=${name#"pg_improv-$ver-pg"}
		pg=${pg%%-*}
		cdx crates/pg_improv $linux --no-default-features --features "pg$pg"
		take "$root/crates/pg_improv/pg_improv_cdylib-lib_$linux.cdx.json" "${name%.tar.gz}.cdx.json"
		;;
	"improv-$tag-"*.tar.gz | "improv-$tag-"*.zip)
		target=${name#"improv-$tag-"}
		target=${target%.tar.gz}
		target=${target%.zip}
		cdx . "$target"
		for crate_bin in cli:improv tui:improv-tui server:improv-server gui:improv-gui; do
			bin=${crate_bin#*:}
			take "$root/crates/${crate_bin%%:*}/${bin}_bin_$target.cdx.json" "$bin-$tag-$target.cdx.json"
		done
		;;
	*) die "$name: not a release artifact this script knows (add an SBOM rule)" ;;
	esac
	tidy "${target:-$linux}"
	unset target
	n=$((n + 1))
done
((n)) || die "no release artifacts in $dist"

cd "$dist"
files=()
for f in *; do
	case $f in SHA256SUMS*) ;; *) files+=("$f") ;; esac
done
sha256sum -- "${files[@]}" >SHA256SUMS
sha256sum --quiet -c SHA256SUMS
echo "release-attest: $n artifacts, ${#files[@]} files in SHA256SUMS"

if [[ $sign == keyless ]]; then
	cosign sign-blob --yes --bundle SHA256SUMS.sigstore.json SHA256SUMS
	# Check it as a user will (docs/src/running.md, "Verifying a release").
	cosign verify-blob SHA256SUMS --bundle SHA256SUMS.sigstore.json \
		--certificate-identity "${GITHUB_SERVER_URL:?}/${GITHUB_WORKFLOW_REF:?}" \
		--certificate-oidc-issuer https://token.actions.githubusercontent.com
else
	echo "release-attest: not signing (ATTEST_SIGN=$sign)"
fi
