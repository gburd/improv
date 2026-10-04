#!/usr/bin/env bash
# GUI screenshot regression (review plan item D5): run scripts/gui-shot.sh and
# compare its capture against the committed golden.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
golden=$root/docs/reviews/golden/gui-1400x900.png
out=$root/target/gui-shot
max=${GUI_SHOT_MAX_AE:-200}

case "${1:-}" in
-h | --help)
	cat <<EOF
Usage: scripts/gui-shot-check.sh [--update]

Runs scripts/gui-shot.sh at 1400x900 and counts pixels that differ from
docs/reviews/golden/gui-1400x900.png by more than 2% (magick compare -metric
AE -fuzz 2%). Fails above GUI_SHOT_MAX_AE (default 200) and leaves the
highlighted difference in target/gui-shot/diff.png.
  --update   overwrite the golden with the new capture instead
EOF
	exit 0
	;;
--update | '') ;;
*) echo "gui-shot-check: unknown option $1" >&2 && exit 2 ;;
esac

GUI_SHOT_GEOMETRY=1400x900 "$root/scripts/gui-shot.sh" "$out" >/dev/null
if [[ ${1:-} == --update ]]; then
	cp "$out/gui.png" "$golden"
	echo "updated $golden"
	exit 0
fi
# compare exits 1 when the images differ; the metric goes to stderr.
ae=$(magick compare -metric AE -fuzz 2% "$out/gui.png" "$golden" "$out/diff.png" 2>&1) || true
ae=${ae%% *}
[[ $ae =~ ^[0-9]+(\.[0-9]+)?(e\+?[0-9]+)?$ ]] || {
	echo "gui-shot-check: compare failed: $ae" >&2
	exit 1
}
echo "gui-shot-check: $ae pixels differ (max $max); diff in $out/diff.png"
awk -v ae="$ae" -v max="$max" 'BEGIN { exit !(ae + 0 <= max + 0) }'
