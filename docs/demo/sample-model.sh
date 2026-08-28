#!/usr/bin/env bash
# Build a richer 3-D sales model for exploring the GUI (stacking, pivoting,
# paging, filters, views, charts). Run from the repo root:
#   docs/demo/sample-model.sh [out.db]
# Then open it:  nix develop -c cargo run -p improv_gui -- out.db
set -euo pipefail

IMPROV="${IMPROV:-cargo run --quiet -p improv_cli --}"
DB="${1:-sample.db}"
rm -f "$DB"

q() { $IMPROV "$@" >/dev/null; }

echo "Building 3-D sales model -> $DB"
q init "$DB"

# --- Dimensions: Time (quarters) x Product x Region ---
q add-category "$DB" 1 Time
q add-item "$DB" 10 1 2024Q1
q add-item "$DB" 11 1 2024Q2
q add-item "$DB" 12 1 2024Q3
q add-item "$DB" 13 1 2024Q4

q add-category "$DB" 2 Product
q add-item "$DB" 20 2 Widget
q add-item "$DB" 21 2 Gadget
q add-item "$DB" 22 2 Gizmo

q add-category "$DB" 3 Region
q add-item "$DB" 30 3 North
q add-item "$DB" 31 3 South

# --- Input measures ---
# Price varies by Product only (broadcast over Time and Region).
q add-measure "$DB" 100 Price number input Product
q set "$DB" 100 12.00 --at Product=Widget
q set "$DB" 100 25.00 --at Product=Gadget
q set "$DB" 100 8.50  --at Product=Gizmo

# UnitCost varies by Product only.
q add-measure "$DB" 101 UnitCost number input Product
q set "$DB" 101 7.00  --at Product=Widget
q set "$DB" 101 15.00 --at Product=Gadget
q set "$DB" 101 5.00  --at Product=Gizmo

# Quantity varies across all three dimensions.
q add-measure "$DB" 102 Quantity number input Time Product Region

# Deterministic-ish quantities: base per product, ramps by quarter, region split.
set_qty() { q set "$DB" 102 "$4" --at "Time=$1,Product=$2,Region=$3"; }
for prod in Widget Gadget Gizmo; do
  case $prod in
    Widget) base=100 ;;
    Gadget) base=40  ;;
    Gizmo)  base=220 ;;
  esac
  qi=0
  for tq in 2024Q1 2024Q2 2024Q3 2024Q4; do
    # north gets 60%, south 40%, with a per-quarter ramp
    ramp=$(( qi * 10 ))
    north=$(( (base + ramp) * 6 / 10 ))
    south=$(( (base + ramp) * 4 / 10 ))
    set_qty "$tq" "$prod" North "$north"
    set_qty "$tq" "$prod" South "$south"
    qi=$(( qi + 1 ))
  done
done

# --- Derived measures (formulas over names, not cells) ---
q define "$DB" 200 "Revenue   = Price * Quantity"
q define "$DB" 201 "COGS      = UnitCost * Quantity"
q define "$DB" 202 "Margin    = Revenue - COGS"
# Aggregations collapse a dimension.
q define "$DB" 203 "RevByProduct = SUM(Revenue OVER Time)"
q define "$DB" 204 "RevByRegion  = SUM(Revenue OVER Time)"

echo
echo "Done. Explore it:"
echo "  nix develop -c cargo run -p improv_gui -- $DB"
echo
echo "Try in the GUI:"
echo "  * Select 'Revenue' (3-D: Time x Product x Region)."
echo "  * Drag the Region tile onto the Rows margin to STACK it under Time"
echo "    (nested row headers = the multi-category-per-axis feature)."
echo "  * Or drag Region to Pages and use the page selector to slice by region."
echo "  * Toggle the Chart; save a View; filter Product to a subset."
$IMPROV eval "$DB" 200 | head -6
