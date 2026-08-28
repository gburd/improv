# The Desktop GUI

`improv-gui` is the egui/eframe desktop front-end, styled after the original
NeXTSTEP Lotus Improv: a light neutral-gray desktop, chiseled/beveled controls,
a top formula bar, a left tool palette, on-grid category tiles for pivoting, and
an inspector.

Run it (from the Nix dev shell, which provides the GUI runtime libraries):

```sh
bash docs/demo/sample-model.sh sample.db
nix develop -c cargo run -p improv_gui -- sample.db
```

## The pivot grid

![Improv GUI — pivot grid](./img/gui-pivot-grid.png)

The window has five regions:

- **Tool palette** (far left) — beveled buttons: pivot (rotate axes), toggle
  chart, save view, save model.
- **Model explorer** — Categories (with their items), Measures (`·` input,
  `=` derived), and saved Views.
- **Formula bar** (top) — the selected measure's formula
  (`Revenue = Price times Quantity`); editing it re-typechecks and recomputes.
- **Pivot grid** (center) — the selected measure projected onto the current
  axes. Here Revenue is shown with **Product** on columns, **Time** on rows,
  and **Region** paged (`North [1/2]`, steppable). The **category tiles** at the
  Columns / Rows / Pages margins are drag sources; drag one to another margin to
  re-pivot without touching the formula.
- **Inspector** (right) — the measure's id, kind, value type, dimensions,
  dependencies, and formula.

## Stacked axes

Drag the **Region** tile from the Pages margin onto the **Rows** margin and the
row axis becomes the *stacked* product of Time × Region:

![Improv GUI — stacked axes](./img/gui-stacked-axes.png)

Each Time group (2024Q1, 2024Q2, …) now expands into its Region rows
(North / South), with the outer Time label printed once per group. Any number of
categories can stack on either axis; the grid renders the full Cartesian
product, and rows are **virtualized** so a stacked axis of millions of lines
stays responsive.

## Incremental recalculation

Editing an input cell propagates as a delta through differential dataflow — only
the touched cells and their dependents recompute. The same behavior on the CLI:

![Incremental recalculation](./img/recalc-demo.svg)

## Capturing a screencast

`docs/demo/gui-screencast.sh` builds the demo model and prints a click-by-click
script (select Revenue → stack Region onto rows → edit a Quantity cell → watch
Revenue/Margin update → toggle the chart → save a view → filter a product) for
recording a walkthrough with your screen recorder of choice.
