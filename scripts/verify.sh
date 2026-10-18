#!/usr/bin/env bash
# Check the Verus proofs in crates/verified (improv_verified).
#
# Needs the pinned Verus release on PATH (or VERUS_DIR pointing at its
# unpacked directory) and the Rust toolchain that release names, which rustup
# installs on first use:
#
#   curl -LO https://github.com/verus-lang/verus/releases/download/release/0.2026.10.04.426d8b0/verus-0.2026.10.04.426d8b0-x86-linux.zip
#   unzip verus-0.2026.10.04.426d8b0-x86-linux.zip   # -> verus-x86-linux/
#   VERUS_DIR=$PWD/verus-x86-linux scripts/verify.sh
#
# The vstd pin in the workspace Cargo.toml must match this release.
set -euo pipefail
cd "$(dirname "$0")/.."
if [ -n "${VERUS_DIR:-}" ]; then
    PATH="$VERUS_DIR:$PATH"
fi
command -v cargo-verus >/dev/null || {
    echo "cargo-verus not found: set VERUS_DIR or put the Verus release on PATH" >&2
    exit 1
}
exec cargo-verus verify -p improv_verified "$@"
