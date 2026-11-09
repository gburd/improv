#!/usr/bin/env bash
# The COSMIC GUI's CI job (crates/gui_cosmic, its own Cargo workspace), as
# GitHub and a local run execute it:
#
#   ci/gui_cosmic.sh
#
# fmt, clippy -D warnings, the tests, rustdoc, cargo-deny with the crate's
# own deny.toml (libcosmic's git sources are allowed there, never in the
# root's), and the release vendoring with its licence scan. Every cargo
# command that resolves is --locked: Cargo.lock is
# the only pin libcosmic and its git companions have (crates/gui_cosmic
# README, "Pins"), so a resolve that moved it must fail, not build.
#
# Needs: cargo; on Linux, libxkbcommon-dev and libwayland-dev (what
# libcosmic's own CI installs); cargo-deny and python3 (3.11+) when
# GUI_COSMIC_DENY is not 0. GUI_COSMIC_SCAN_DIR: where to vendor for the
# scan (default: a temporary directory, removed after).
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root/crates/gui_cosmic"

step() { echo; echo "::group::$*" 2>/dev/null || true; echo "== $*"; }
end() { echo "::endgroup::" 2>/dev/null || true; }

step "fmt + clippy"
cargo fmt -- --check
cargo clippy --locked --all-targets -- -D warnings
end

step "test"
cargo test --locked
end

step "rustdoc"
RUSTDOCFLAGS=-Dwarnings cargo doc --locked --no-deps
end

if [ "${GUI_COSMIC_DENY:-1}" != 0 ]; then
	# A release build's vendored tree: libcosmic's GPL localize.rs replaced
	# (pop-os/libcosmic#1500), and no other GPL or AGPL file, nor an LGPL one
	# that is unlisted or modified (release/license-allow.txt). Host
	# independent, so once, with cargo-deny; first, so a failing advisory
	# cannot hide it.
	step "release vendoring + licence scan (release/vendor-libcosmic.sh)"
	scan=${GUI_COSMIC_SCAN_DIR:-}
	if [ -z "$scan" ]; then
		scan=$(mktemp -d)
		# shellcheck disable=SC2064 # expand now: $scan is local to this block
		trap "rm -rf '$scan'" EXIT
	fi
	python3 release/license-scan.py selftest
	release/vendor-libcosmic.sh "$scan/vendor" >/dev/null
	end

	step "cargo-deny (crates/gui_cosmic/deny.toml)"
	# No --config: cargo-deny reads the deny.toml beside the manifest, and
	# where --config may go differs between cargo-deny releases.
	cargo deny --locked --manifest-path Cargo.toml check
	end
fi
