#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# A release build of improv-cosmic, as distributed: from a vendored tree in
# which libcosmic's GPL-3.0-only src/localize.rs is replaced by Improv's own
# (release/vendor-libcosmic.sh, pop-os/libcosmic#1500), offline and --locked.
# Developer builds (`cargo build`) keep upstream's file. Run in any directory:
#
#   release/build.sh [cargo args...]     e.g. test, to run the tests over it
#
# The default is `build --release`; other arguments replace it, and run
# against the same vendored tree. The tree is target/release-vendor (under
# CARGO_TARGET_DIR when set), re-vendored when Cargo.lock or release/
# changes.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
crate=$(dirname "$here")
cd "$crate"
target=${CARGO_TARGET_DIR:-$crate/target}
v=$target/release-vendor
config=$target/release-vendor.toml
mkdir -p "$target"

stamp=$(cat Cargo.lock "$here"/localize.rs "$here"/license-allow.txt "$here"/*.sh "$here"/*.py | sha256sum)
if [ "$stamp" != "$(cat "$v.stamp" 2>/dev/null)" ]; then
	"$here/vendor-libcosmic.sh" "$v" >"$config"
	# cargo takes vendored sources as immutable: it would keep a libcosmic
	# built from an older tree at this path.
	cargo --config "$config" clean --offline -p libcosmic
	cargo --config "$config" clean --offline -p libcosmic --release
	echo "$stamp" >"$v.stamp"
fi

[ $# -gt 0 ] || set -- build --release
cargo --config "$config" "$1" --locked --offline "${@:2}"
