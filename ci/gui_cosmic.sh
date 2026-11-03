#!/usr/bin/env bash
# The COSMIC GUI's CI job (crates/gui_cosmic, its own Cargo workspace), as
# GitHub and a local run execute it:
#
#   ci/gui_cosmic.sh
#
# fmt, clippy -D warnings, the tests, rustdoc, and cargo-deny with the
# crate's own deny.toml (libcosmic's git sources are allowed there, never in
# the root's). Every cargo command that resolves is --locked: Cargo.lock is
# the only pin libcosmic and its git companions have (crates/gui_cosmic
# README, "Pins"), so a resolve that moved it must fail, not build.
#
# Needs: cargo; on Linux, libxkbcommon-dev and libwayland-dev (what
# libcosmic's own CI installs); cargo-deny when GUI_COSMIC_DENY is not 0.
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
	step "cargo-deny (crates/gui_cosmic/deny.toml)"
	cargo deny --config deny.toml --locked --manifest-path Cargo.toml check
	end
fi
