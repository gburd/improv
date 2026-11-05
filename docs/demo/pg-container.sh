#!/usr/bin/env bash
# Run a Postgres demo script inside the pg17 image (docs/demo/pg17.Containerfile),
# from the repo root:
#   docs/demo/pg-container.sh docs/demo/pg-demo.sh
# The checkout is mounted at /src (the script writes its .cast there), and
# CACHE (default ~/.cache/improv-demo) keeps the cargo registry and the
# target dir between runs. In the container, as the calling user, this
# installs pg_improv from this checkout (cargo pgrx install --release, as
# ci/pg_improv.sh does), builds improv and improv-tui, and runs the script
# with BIN, PGBIN and IMPROV_DEMO_IN_CONTAINER=1 set. Needs podman
# (PODMAN=docker may work); the first run builds for about 10 minutes.
set -euo pipefail
script=${1:?usage: docs/demo/pg-container.sh SCRIPT}
PODMAN=${PODMAN:-podman}
CACHE=${CACHE:-$HOME/.cache/improv-demo}
mkdir -p "$CACHE/registry" "$CACHE/target" "$CACHE/home"
"$PODMAN" build -q -t improv-demo-pg17 -f docs/demo/pg17.Containerfile docs/demo >/dev/null
"$PODMAN" run --rm --userns=keep-id --user "$(id -u):$(id -g)" \
	-v "$PWD:/src" -v "$CACHE/registry:/usr/local/cargo/registry" \
	-v "$CACHE/target:/cache/target" -v "$CACHE/home:/cache/home" \
	-e HOME=/cache/home -e CARGO_TARGET_DIR=/cache/target ${CARGO_BUILD_JOBS:+-e CARGO_BUILD_JOBS="$CARGO_BUILD_JOBS"} \
	-w /src improv-demo-pg17 bash -c '
set -euo pipefail
pgc=/usr/lib/postgresql/17/bin/pg_config
cargo pgrx init --pg17 "$pgc" >/dev/null
(cd crates/pg_improv && cargo pgrx install --release --no-default-features --features pg17 --pg-config "$pgc" >/dev/null)
cargo build -q --release -p improv_cli -p improv_tui
BIN=/cache/target/release PGBIN=/usr/lib/postgresql/17/bin IMPROV_DEMO_IN_CONTAINER=1 bash "$0"
' "$script"
