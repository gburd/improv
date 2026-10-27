#!/usr/bin/env bash
# Run a command in the manylinux_2_28 build container: AlmaLinux 8 (glibc
# 2.28, GCC 8's libstdc++) with gcc-toolset-14, whose libstdc++.so links the
# symbols newer than GCC 8 statically. What it builds needs at most glibc
# 2.28 and GLIBCXX_3.4.25, so it runs on RHEL 8 and every later
# distribution. The Linux release artifacts are built here.
#
#   ci/manylinux.sh COMMAND [ARG]...
#   ci/manylinux.sh cargo build --release -p improv_cli
#   ci/manylinux.sh ci/pg_improv-dist.sh 17 dist
#
# The repository is mounted at its own path and COMMAND runs in the current
# directory, which must be inside it. Rust is rustup's stable, as
# dtolnay/rust-toolchain@stable gives the host jobs. CARGO_TARGET_DIR is
# <repo>/target/manylinux (nested in the host's target dir, so a rust-cache
# step on the host caches it), rustup and cargo's bin dir (cargo-pgrx) are
# in <repo>/target/manylinux-home, and the crate registry is the host's
# ($CARGO_HOME/registry), so it is downloaded and cached once. Never point a container build at a host build's
# target dir: cargo's fingerprints do not cover the C compiler, so it would
# reuse C objects (bundled SQLite, DuckDB) compiled against the host's newer
# glibc. Runs as the container's root, so dnf works; under rootless podman
# that is the calling user.
set -euo pipefail
# 2026.10.26-1. A newer image is fine; ci/glibc-floor.sh checks its output.
image=quay.io/pypa/manylinux_2_28_x86_64@sha256:e0adf18521ad35192df733a6c62d35df7801957d24e0470238aacecfab2188a3
root=$(cd "$(dirname "$0")/.." && pwd)
target=$root/target/manylinux home=$root/target/manylinux-home
registry=${CARGO_HOME:-$HOME/.cargo}/registry
[ $# -gt 0 ] || { echo "usage: $0 COMMAND [ARG]..." >&2; exit 2; }
case $PWD/ in "$root"/*) ;; *) echo "manylinux.sh: run it from inside $root" >&2; exit 2 ;; esac
mkdir -p "$target" "$home/cargo/registry" "$registry"
# shellcheck disable=SC2016 # expanded in the container
setup='
export PATH=$CARGO_HOME/bin:$PATH
if [ ! -x "$CARGO_HOME/bin/rustup" ]; then
	curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs |
		sh -s -- -y -q --profile minimal --default-toolchain none --no-modify-path >/dev/null
fi
rustup -q toolchain install stable --profile minimal >/dev/null
rustup -q default stable
rustc --version
exec "$@"'
exec podman run --rm -v "$root:$root" -v "$registry:$home/cargo/registry" -w "$PWD" \
	-e CARGO_HOME="$home/cargo" -e RUSTUP_HOME="$home/rustup" -e CARGO_TARGET_DIR="$target" \
	-e CARGO_BUILD_JOBS -e CARGO_TERM_COLOR \
	"$image" bash -c "set -euo pipefail; $setup" bash "$@"
