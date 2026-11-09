#!/usr/bin/env bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Vendor improv-cosmic's crates for a release build, with libcosmic's one
# GPL-3.0-only file, src/localize.rs, replaced by Improv's own MIT OR
# Apache-2.0 implementation (release/localize.rs) until upstream settles its
# licence (https://github.com/pop-os/libcosmic/issues/1500), then run the
# licence scan (license-scan.py, license-allow.txt) over the whole tree.
# Once upstream relicenses the file, the patch and its check are dropped.
#
#   release/vendor-libcosmic.sh VENDOR_DIR             vendor, patch, scan;
#                                                      print the config
#   release/vendor-libcosmic.sh VENDOR_DIR --patch-only
#
# The first form prints the .cargo/config.toml source replacement that
# builds from VENDOR_DIR. --patch-only takes a tree already vendored and
# snapshotted (`license-scan.py snapshot`), which something else edited in
# between (ci/solnix-build.sh), and only patches and scans it.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
die() { echo "vendor-libcosmic: $*" >&2; exit 1; }

[ $# -ge 1 ] || die "usage: vendor-libcosmic.sh VENDOR_DIR [--patch-only]"
mkdir -p "$1"
vendor=$(cd "$1" && pwd)

if [ "${2:-}" = --patch-only ]; then
	config=
else
	config=$(cd "$here/.." && cargo vendor --locked "$vendor")
	python3 "$here/license-scan.py" snapshot "$vendor"
fi

# The header the file had when the decision was made: any other first line
# (relicensed, rewritten) needs a human look first, and maybe no patch.
expect='// SPDX-License-Identifier: GPL-3.0-only'
target=$vendor/libcosmic/src/localize.rs
[ -f "$target" ] || die "$target is missing: libcosmic moved or dropped it; re-check pop-os/libcosmic#1500"
first=$(head -n 1 "$target")
if [ "$first" != "$expect" ] && ! cmp -s "$target" "$here/localize.rs"; then
	die "$target starts '$first', not '$expect': re-check pop-os/libcosmic#1500 before patching"
fi
cp "$here/localize.rs" "$target"
# Re-stamp the file's checksum; "package" stays as cargo vendor wrote it
# (null for a git source).
python3 - "$vendor/libcosmic/.cargo-checksum.json" "$target" <<'PY'
import hashlib, json, sys
sums, target = sys.argv[1:]
with open(sums, encoding="utf-8") as f:
    d = json.load(f)
if "src/localize.rs" not in d["files"]:
    sys.exit("vendor-libcosmic: no src/localize.rs entry in " + sums)
with open(target, "rb") as f:
    d["files"]["src/localize.rs"] = hashlib.sha256(f.read()).hexdigest()
with open(sums, "w", encoding="utf-8") as f:
    json.dump(d, f)
PY
echo "vendor-libcosmic: replaced libcosmic/src/localize.rs with Improv's (pop-os/libcosmic#1500)" >&2

python3 "$here/license-scan.py" check "$vendor"
[ -z "$config" ] || printf '%s\n' "$config"
