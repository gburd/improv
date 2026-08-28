#!/usr/bin/env bash
# GUI screencast helper: builds the demo model and prints a click-by-click
# script for capturing a screencast (the actual screen recording is a manual
# step — run your recorder, then follow the numbered beats below).
#
#   docs/demo/gui-screencast.sh [out.db]
#   # then, with a screen recorder running (e.g. wf-recorder / OBS):
#   nix develop -c cargo run -p improv_gui -- out.db
set -euo pipefail

DB="${1:-screencast.db}"
here="$(cd "$(dirname "$0")" && pwd)"
IMPROV="${IMPROV:-cargo run --quiet -p improv_cli --}" \
  bash "$here/sample-model.sh" "$DB" >/dev/null
echo "Built model: $DB"
cat <<'BEATS'

Screencast beats (record these, ~60s):

  1. Launch: nix develop -c cargo run -p improv_gui -- screencast.db
     — NeXTSTEP gray window: tool palette (left), Model explorer, formula bar
       (top), pivot grid (center), Inspector (right).
  2. Click "Revenue" in Measures. The formula bar shows "Revenue = Price times
     Quantity"; the grid shows Product across columns, Time down rows, Region
     paged (North [1/2]).
  3. Drag the "Region" tile from the Pages margin onto the Rows margin.
     — Rows become the stacked product Time x Region (2024Q1 North/South, ...),
       the multi-category-per-axis feature.
  4. Click a Quantity-derived cell? No — pick an INPUT measure ("Quantity"),
     double-click a cell, type a new number, press Enter.
  5. Re-select "Revenue" (and "Margin"): the edited cell's dependents have
     recomputed incrementally.
  6. Toggle "Chart" (top or the tool palette): a bar chart of the selection.
  7. Type a name in Views -> "Save view"; click it to reload the layout.
  8. Expand "Filters", uncheck a Product item; it disappears from the grid.

Save the capture to docs/src/img/gui-screencast.<mp4|gif|webm> and reference it in
the README next to the screenshots.
BEATS
