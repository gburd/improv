//! GUI application state and rendering.
//!
//! State: the loaded `Model`, a live `session::Engine` (built over all derived
//! measures), the current derived-measure snapshot, and the selected measure.
//! The panels (model explorer, pivot grid, formula editor, inspector) are all
//! rendered each frame from this state. The GUI is a pure *view/controller*
//! over `improv_engine` + the model; it adds no modeling semantics (see
//! `.agent/steering/AGENT_GUI_STEERING.md` §9.3).
//!
//! The engine is rebuilt (a structural rebuild) only when the measure structure
//! changes — a formula edit or a new derived measure. Plain cell-value edits go
//! through `engine.set` incrementally.

use std::borrow::Cow;
use std::collections::HashMap;

use improv_core_model::{
    parser, BinaryOp, CanvasRect, CategoryId, Expr, Filter, FuncId, ItemId, MatrixPlacement,
    Measure, MeasureId, MeasureKind, Model, Name, ParseError, UnaryOp, Value, ValueType, View,
    ViewId,
};
use improv_engine::session::{Engine, MeasureValues};
use improv_engine::{encode_coord, CellValue, CoordKey};
use improv_nl_formula::{describe_formula, parse_nl_formula, NlContext};
use improv_storage_mentat::ModelStore;

use crate::csv_wizard::{self, ExportForm, ImportForm};

/// One matrix on the canvas: a measure, that matrix's OWN pivot / filter /
/// cursor / cell-editor state, and where it sits (`rect`).
///
/// This is the GUI-side twin of [`improv_core_model::MatrixPlacement`] (which is
/// the persisted form): a placement plus the transient bits a placement has no
/// business storing — the keyboard cursor, the open cell editor, and the
/// measured gutter geometry. Several of these live on one canvas
/// (`ImprovApp::matrices`), each independently pivotable, which is plan Step 3
/// (`docs/reviews/2026-09-22-gui-reconstruction-plan.md`).
///
/// Every method here takes `&Model` rather than holding one: a matrix is pure
/// presentation over the shared model, exactly as the per-view state was before
/// it became a list.
#[derive(Debug, Clone, Default)]
pub(crate) struct Matrix {
    /// The measure this matrix shows. `None` only for an empty model.
    measure: Option<MeasureId>,
    /// A permutation of `measure`'s categories: the first `n_rows` are stacked
    /// on rows (outer→inner), the next `n_cols` on columns, the rest are pages.
    /// Pivoting reorders this without touching formulas. Reset to the measure's
    /// natural order when this matrix's measure changes.
    axis_order: Vec<CategoryId>,
    /// How many leading `axis_order` categories are stacked on the ROW axis,
    /// and how many (after those) on the COLUMN axis. The rest are pages.
    /// Default 1/1 (one category per axis); increasing them stacks categories
    /// on an axis (nested group headers over the Cartesian product of items).
    n_rows: usize,
    n_cols: usize,
    /// Selected item index for each page (extra) dimension, positionally by
    /// page dim (i.e. `axis_order[n_rows + n_cols + i]`).
    page_idx: Vec<usize>,
    /// The measure `axis_order`/`page_idx` currently describe (so the pivot
    /// state resets when this matrix's measure changes).
    axis_for: Option<MeasureId>,
    /// Active per-category display filters for this matrix. Presentation only
    /// (hides items from the grid; never touches data). Captured when saving a
    /// view; reset on measure switch.
    filters: Vec<Filter>,
    /// Keyboard cell cursor into this matrix's grid (row/col indices), clamped
    /// to its dimensions.
    cursor_row: usize,
    cursor_col: usize,
    /// The cell currently being edited in THIS matrix, and its text buffer.
    editing: Option<(MeasureId, CoordKey)>,
    edit_buf: String,
    /// Where this matrix sits on the canvas (persisted in the view).
    rect: CanvasRect,
    /// Where THIS matrix's margin gutters and table ended up in the last
    /// laid-out frame (see [`GutterRects`]). `None` until it has rendered once.
    /// Layout output, not state.
    gutters: Option<GutterRects>,
    /// The row and column category stacks THIS matrix's grid was last rendered
    /// with, as `(rows, columns)`. `None` until it has rendered once.
    ///
    /// Render output, the same role `gutters` plays for the geometry — and for
    /// the same reason: it makes "each matrix renders its OWN pivot" checkable
    /// instead of merely intended. Asserting on `axis_order` alone cannot see a
    /// renderer that ignores its matrix index and draws the focused matrix's
    /// layout N times.
    rendered_axes: Option<(Vec<CategoryId>, Vec<CategoryId>)>,
}

/// The running GUI application.
pub struct ImprovApp {
    /// The store path (empty = in-memory scratch); used for saving.
    db: String,
    model: Model,
    /// Live incremental engine over all derived measures, plus its snapshot.
    engine: Option<Engine>,
    snapshot: HashMap<MeasureId, MeasureValues>,
    /// The matrices on the canvas, in canvas order. **Always non-empty**: index
    /// 0 is the view's primary matrix (see [`View`]'s flat fields), the rest are
    /// its `placements`.
    matrices: Vec<Matrix>,
    /// Which matrix has the keyboard: the only one that consumes grid shortcuts
    /// and the one every "selected measure" surface (formula bar, inspector,
    /// chart, status readout) reads from. Always a valid index into `matrices`.
    focus: usize,
    /// The saved view this canvas was loaded from, if any — the highlighted
    /// document tab (see [`ImprovApp::document_tabs`]).
    current_view: Option<ViewId>,
    status: String,

    // --- transient UI edit buffers (view state, not model state) ---
    /// The formula-editor text for the selected derived measure.
    formula_buf: String,
    /// The measure whose formula `formula_buf` currently holds (so we reload
    /// the buffer when the selection changes).
    formula_for: Option<MeasureId>,
    /// The byte offset of the last `commit_formula` parse error, if any (for
    /// the formula bar's inline red-underline highlight + error label). Set on
    /// a failed commit; cleared on a successful commit or when the buffer is
    /// edited again.
    formula_error_pos: Option<usize>,
    /// The message of the last `commit_formula` parse error, shown directly
    /// under the formula bar. Cleared alongside `formula_error_pos`.
    formula_error_msg: String,
    /// New-derived-measure form: name + formula text.
    new_name: String,
    new_formula: String,
    /// Text buffer for the "Save view" name field.
    view_name: String,

    /// Whether the read-only chart panel is shown, and its bar/line toggle.
    show_chart: bool,
    chart_line: bool,

    /// CSV/TSV import/export wizard: whether the panel is shown, and its two
    /// forms (see `csv_wizard`).
    show_csv_wizard: bool,
    import_form: ImportForm,
    export_form: ExportForm,

    /// Undo/redo history: whole-`Model` snapshots, oldest first. `undo_stack`
    /// holds the states BEFORE each recorded mutation; `redo_stack` the states
    /// undone away. See [`ImprovApp::undo`] and `UNDO_DEPTH`.
    undo_stack: Vec<Model>,
    redo_stack: Vec<Model>,

    /// Where each formula-list row landed in the last laid-out frame, as
    /// `(measure, row rect)` in display order. Recorded by
    /// [`ImprovApp::formula_list_panel`]; empty until the pane has rendered.
    /// Layout output, not model state — the same role `gutters` plays for the
    /// grid, and what lets a headless test click a row where a user would.
    formula_row_rects: Vec<(MeasureId, egui::Rect)>,
}

/// The measured screen geometry of the grid's margin gutters and the table they
/// frame, as laid out in the last frame.
///
/// The gutters are *docked to the grid's edges* — the signature Improv pivot
/// surface (`docs/reviews/refs/improv-pivot-gesture.jpg`): row-axis tiles in the
/// gutter along the table's left edge, column-axis tiles in the gutter along its
/// top edge, page/unplaced tiles in the bottom-left corner well. Recording the
/// rects makes that adjacency *checkable* instead of merely apparent — see
/// [`gutters_frame_table`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GutterRects {
    /// The column-axis gutter, spanning the table's top edge.
    top: egui::Rect,
    /// The row-axis gutter, spanning the table's left edge.
    left: egui::Rect,
    /// The table (header + cells) the two gutters frame.
    table: egui::Rect,
    /// The bottom-left corner well: page (unplaced) categories.
    well: egui::Rect,
}

/// Tolerance, in points, for the gutter-adjacency invariant. The edges are
/// computed from the same panel cursor so they agree exactly; half a point of
/// slack only guards against rounding in egui's pixel alignment.
const EDGE_EPS: f32 = 0.5;

/// Whether the gutters genuinely *frame* the table: the row gutter's right edge
/// **is** the table's left edge, the column gutter's bottom edge **is** the
/// table's top edge, each gutter spans the table along its other axis, and the
/// corner well sits below the table at the row gutter's left edge.
///
/// This is the geometric property the old horizontal "axis shelf" failed: three
/// drop zones side by side above the grid touch no grid edge at all. It is
/// `debug_assert!`ed every frame (when the window has room — see
/// [`gutters_have_room`]) and asserted on recorded rects in the headless layout
/// test.
fn gutters_frame_table(g: &GutterRects) -> bool {
    let adjoins = |a: f32, b: f32| (a - b).abs() <= EDGE_EPS;
    adjoins(g.left.max.x, g.table.min.x)
        && adjoins(g.top.max.y, g.table.min.y)
        // The row gutter runs alongside the table, not merely up to a corner.
        && g.left.min.y <= g.table.min.y + EDGE_EPS
        && g.left.max.y + EDGE_EPS >= g.table.min.y
        // The column gutter spans across the table.
        && g.top.min.x <= g.table.min.x + EDGE_EPS
        && g.top.max.x + EDGE_EPS >= g.table.min.x
        // The well is the bottom-left corner of the framed region: below the row
        // gutter (whose height IS the framed region's height — the table's own
        // `min_rect` can overflow that when its content does not fit), and flush
        // with the row gutter's left edge.
        && g.well.min.y + EDGE_EPS >= g.left.max.y
        && adjoins(g.well.min.x, g.left.min.x)
}

/// Whether the window left the gutters enough room to be laid out at all.
///
/// A panel clamps its extent to what is available, so in an absurdly small
/// window (a few dozen points of central panel) the bottom well is squeezed up
/// past the table and [`gutters_frame_table`] cannot hold — through no fault of
/// the layout. The per-frame `debug_assert!` is gated on this so shrinking the
/// window can never panic a debug build; the invariant itself stays strict, and
/// the layout test asserts it at a real window size.
fn gutters_have_room(g: &GutterRects) -> bool {
    g.well.min.y >= g.top.max.y && g.left.height() > 0.0
}

/// Thickness of the row (left) gutter, in points. The blank corner box above it
/// is the same width, so the two line up exactly.
///
/// ponytail: a constant, not a measurement of the widest category name — tile
/// labels truncate instead. Measure the names (`Context::fonts` +
/// `layout_no_wrap`) if long category names turn out to be common.
const GUTTER_W: f32 = 120.0;

/// How far a newly added matrix is offset from the last one, in points, so a
/// fresh matrix never lands exactly on top of its predecessor (the classic
/// cascade). See [`ImprovApp::add_matrix`].
const CASCADE: f32 = 28.0;

/// Height of a matrix's title bar, in points — the reference's teal
/// `Property Financials: Virginia Ave` band across the top of each matrix
/// (plan Step 5). See [`ImprovApp::matrix_title_bar`].
const TITLE_H: f32 = 22.0;

/// Slack around the matrices on the canvas surface, in points: room to drag a
/// matrix into beyond the right/bottom-most one.
const CANVAS_MARGIN: f32 = 80.0;

/// Size of a matrix's bottom-right resize grip, in points.
const GRIP: f32 = 12.0;

/// Smallest a matrix can be dragged to. Below roughly this it has no room for
/// its title bar, its gutters and a cell, so shrinking further would only hide
/// its own chrome — [`gutters_have_room`] is the same judgement about the
/// window.
const MATRIX_MIN_W: f32 = 240.0;
const MATRIX_MIN_H: f32 = 160.0;

/// How many model states the undo (and redo) stack keeps; older entries are
/// evicted.
///
/// ponytail: history is a bounded stack of FULL `Model` clones — one clone per
/// mutation, up to 50 resident copies. That is honest and obviously correct
/// (the model already round-trips and compares by value), but the ceiling is
/// memory: 50 × model size. If models get big enough for that to hurt, replace
/// the snapshots with a command/delta log (per-edit inverse operations) behind
/// the same `undo`/`redo`/`record_history` API — the call sites do not care
/// which it is.
const UNDO_DEPTH: usize = 50;

/// Which way a gutter's drop zone stretches to cover the gutter: its **fixed**
/// axis. See [`ImprovApp::gutter_drop_zone`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FillAxis {
    /// Stretch across the gutter's width (the top gutter and the corner well,
    /// whose heights come from their tiles).
    Horizontal,
    /// Stretch down the gutter's height (the left gutter, whose width is
    /// [`GUTTER_W`]).
    Vertical,
}

/// Which grid axis a category is assigned to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Rows,
    Columns,
    Pages,
}

/// The resolved pivot for the chart: the ROW category stack, the COLUMN stack,
/// and the pinned `(category, item)` for every PAGE dimension.
pub(crate) type ChartAxes = (Vec<CategoryId>, Vec<CategoryId>, Vec<(CategoryId, ItemId)>);

impl Matrix {
    /// A matrix showing `measure` with its natural axis order and the default
    /// canvas geometry.
    fn new(model: &Model, measure: Option<MeasureId>) -> Matrix {
        let axis_order = natural_axis_order(model, measure);
        Matrix {
            measure,
            n_rows: 1.min(axis_order.len()),
            n_cols: 1.min(axis_order.len().saturating_sub(1)),
            axis_order,
            axis_for: measure,
            ..Matrix::default()
        }
    }

    /// A matrix restoring a saved [`MatrixPlacement`] verbatim: its measure,
    /// axis split, filters and geometry, with the page pins resolved
    /// positionally by page dimension (an item that no longer exists falls back
    /// to the first one).
    ///
    /// `None` when the placement's measure is gone — the caller decides whether
    /// that is an error (a whole view) or one matrix to drop.
    fn from_placement(model: &Model, p: &MatrixPlacement) -> Option<Matrix> {
        if !model.measures.contains_key(&p.measure) {
            return None;
        }
        let mut m = Matrix {
            measure: Some(p.measure),
            axis_order: if p.axis_order.is_empty() {
                natural_axis_order(model, Some(p.measure))
            } else {
                p.axis_order.clone()
            },
            axis_for: Some(p.measure),
            filters: p.filters.clone(),
            rect: p.rect,
            ..Matrix::default()
        };
        // Clamp the axis split to the restored order's length.
        let len = m.axis_order.len();
        m.n_rows = p.n_rows.min(len);
        m.n_cols = p.n_cols.min(len.saturating_sub(m.n_rows));
        // Page selections, positionally by page dimension.
        let page_cats = m.page_cats(model);
        m.page_idx = vec![0; page_cats.len()];
        for (pi, cat) in page_cats.iter().enumerate() {
            if let Some((_, it)) = p.page_items.iter().find(|(c, _)| c == cat) {
                if let Some(idx) = m
                    .sorted_items(model, *cat)
                    .iter()
                    .position(|(id, _)| id == it)
                {
                    if let Some(slot) = m.page_idx.get_mut(pi) {
                        *slot = idx;
                    }
                }
            }
        }
        m.clamp_cursor(model);
        Some(m)
    }

    /// This matrix as a persistable [`MatrixPlacement`] — everything a saved
    /// view needs to reproduce it, and nothing transient (no cursor, no open
    /// editor). `None` when no measure is shown: there is no matrix to save.
    fn to_placement(&self, model: &Model) -> Option<MatrixPlacement> {
        Some(MatrixPlacement {
            measure: self.measure?,
            axis_order: self.axis_order.clone(),
            n_rows: self.n_rows,
            n_cols: self.n_cols,
            page_items: self.pinned_pages(model),
            filters: self.filters.clone(),
            rect: self.rect,
        })
    }

    /// Move this matrix on the canvas by `(dx, dy)` — the title-bar drag (the
    /// reference's free-form placement).
    ///
    /// Clamped at the canvas's top-left: a matrix at a negative offset would sit
    /// outside the scrollable surface, which cannot scroll to it, so it would be
    /// unreachable and unrecoverable except by editing the saved view.
    fn move_by(&mut self, dx: f32, dy: f32) {
        self.rect.x = (self.rect.x + dx).max(0.0);
        self.rect.y = (self.rect.y + dy).max(0.0);
    }

    /// Resize this matrix by `(dw, dh)` — the bottom-right grip drag.
    ///
    /// Clamped at [`MATRIX_MIN_W`] x [`MATRIX_MIN_H`]: smaller than that and the
    /// matrix has no room for its own title bar, gutters and a cell, so dragging
    /// further would only hide its chrome — and a zero or negative size would
    /// make its rect degenerate.
    fn resize_by(&mut self, dw: f32, dh: f32) {
        self.rect.w = (self.rect.w + dw).max(MATRIX_MIN_W);
        self.rect.h = (self.rect.h + dh).max(MATRIX_MIN_H);
    }

    /// Show `measure` in this matrix, dropping any open cell editor. The pivot
    /// state re-homes on the next [`Matrix::sync_axis_state`].
    fn select(&mut self, measure: Option<MeasureId>) {
        self.measure = measure;
        self.editing = None;
    }

    /// Reset this matrix's pivot state to its measure's natural order when its
    /// measure has changed. Called each frame before rendering.
    fn sync_axis_state(&mut self, model: &Model) {
        if self.axis_for != self.measure {
            self.axis_for = self.measure;
            self.axis_order = natural_axis_order(model, self.measure);
            self.n_rows = 1.min(self.axis_order.len());
            self.n_cols = 1.min(self.axis_order.len().saturating_sub(self.n_rows));
            self.page_idx = vec![0; self.page_cats(model).len()];
            self.filters.clear();
            self.cursor_row = 0;
            self.cursor_col = 0;
        } else if self.page_idx.len() != self.page_cats(model).len() {
            // Keep page_idx sized to the current page-dimension count.
            let n = self.page_cats(model).len();
            self.page_idx.resize(n, 0);
        }
        self.clamp_cursor(model);
    }

    /// This matrix's measure's own dimensions, in declared order.
    ///
    /// `axis_order` is not guaranteed to match them: restoring a view keeps a
    /// saved order verbatim, and a CSV re-import can overwrite
    /// `measure.categories`, so a stale category can linger in `axis_order`
    /// while no longer being a dimension of the measure on screen. Such a
    /// category must not influence this matrix's grid at all — without this
    /// filter, a stale PAGE category filtered to zero items made
    /// `pinned_pages_opt` return `None` and blanked a grid whose every cell was
    /// fully specified.
    ///
    /// This is distinct from the case 6a4b862 fixed: a category that IS a real
    /// dimension of the measure and is filtered to nothing still yields zero
    /// lines, because those coordinates genuinely would be under-specified.
    fn measure_dims(&self, model: &Model) -> Vec<CategoryId> {
        natural_axis_order(model, self.measure)
    }

    /// `axis_order` restricted to the measure's dimensions, preserving the
    /// user's axis placement/order. The `n_rows`/`n_cols` split indexes
    /// `axis_order`, so the split is applied first and each slice is filtered.
    fn live_axis_slice(&self, model: &Model, skip: usize, take: usize) -> Vec<CategoryId> {
        let dims = self.measure_dims(model);
        self.axis_order
            .iter()
            .skip(skip)
            .take(take)
            .filter(|c| dims.contains(c))
            .copied()
            .collect()
    }

    fn row_cats(&self, model: &Model) -> Vec<CategoryId> {
        self.live_axis_slice(model, 0, self.n_rows)
    }
    fn col_cats(&self, model: &Model) -> Vec<CategoryId> {
        self.live_axis_slice(model, self.n_rows, self.n_cols)
    }
    fn page_cats(&self, model: &Model) -> Vec<CategoryId> {
        self.live_axis_slice(model, self.n_rows + self.n_cols, usize::MAX)
    }

    /// The pinned (category, item) for each page dimension, from `page_idx`.
    /// A page category filtered to zero items pins nothing and is simply
    /// absent, so the result can be SHORTER than `page_cats()` — grid/cursor
    /// code must use `pinned_pages_opt`, which rejects that case.
    fn pinned_pages(&self, model: &Model) -> Vec<(CategoryId, ItemId)> {
        let mut pinned = Vec::new();
        for (pi, c) in self.page_cats(model).iter().enumerate() {
            let its = self.sorted_items(model, *c);
            if its.is_empty() {
                continue;
            }
            let sel = self
                .page_idx
                .get(pi)
                .copied()
                .unwrap_or(0)
                .min(its.len() - 1);
            pinned.push((*c, its[sel].0));
        }
        pinned
    }

    /// The pinned page items, or `None` if any page category is filtered to zero
    /// items. In that case no item can be pinned for it, so every cell
    /// coordinate would omit that category — under-specified for the measure's
    /// dimensions. Render nothing rather than a cell at such a key.
    fn pinned_pages_opt(&self, model: &Model) -> Option<Vec<(CategoryId, ItemId)>> {
        let pinned = self.pinned_pages(model);
        (pinned.len() == self.page_cats(model).len()).then_some(pinned)
    }

    /// The Cartesian product of `cats`' filtered items, outer category first.
    /// Each returned element is one axis line: a tuple of `(ItemId, name)` in
    /// `cats` order. An empty `cats` yields a single empty tuple (a 1-line
    /// axis, i.e. a scalar in that direction). Any empty category collapses the
    /// product to nothing (no lines).
    fn axis_tuples(&self, model: &Model, cats: &[CategoryId]) -> Vec<Vec<(ItemId, String)>> {
        let mut out: Vec<Vec<(ItemId, String)>> = vec![Vec::new()];
        for c in cats {
            let items = self.sorted_items(model, *c);
            if items.is_empty() {
                return Vec::new();
            }
            let mut next = Vec::with_capacity(out.len() * items.len());
            for prefix in &out {
                for it in &items {
                    let mut t = prefix.clone();
                    t.push(it.clone());
                    next.push(t);
                }
            }
            out = next;
        }
        out
    }

    /// The item lists (sorted, filtered) for each of `cats`, in order. Used to
    /// virtualize the row axis: with these lists we can compute the total row
    /// count as a product of lengths and decode the i-th row tuple on demand
    /// (`nth_tuple`) without materializing the whole Cartesian product.
    fn axis_item_lists(&self, model: &Model, cats: &[CategoryId]) -> Vec<Vec<(ItemId, String)>> {
        cats.iter().map(|c| self.sorted_items(model, *c)).collect()
    }

    /// A category's items sorted by id, honoring THIS matrix's filters (shared
    /// by paging and grid rendering). A category without a filter shows all
    /// items; filtering is presentation only — it never touches model data.
    fn sorted_items(&self, model: &Model, c: CategoryId) -> Vec<(ItemId, String)> {
        let keep = |id: ItemId| match self.filters.iter().find(|f| f.category == c) {
            Some(f) => f.items.contains(&id),
            None => true,
        };
        let mut v: Vec<(ItemId, String)> = model
            .categories
            .get(&c)
            .map(|cat| {
                cat.items
                    .iter()
                    .filter(|id| keep(**id))
                    .filter_map(|id| model.items.get(id).map(|it| (*id, it.name.0.clone())))
                    .collect()
            })
            .unwrap_or_default();
        v.sort_by_key(|(id, _)| id.0);
        v
    }

    /// Move `category` to `axis` in THIS matrix, appending it as the innermost
    /// entry of that axis (so categories *stack*: dropping a second category on
    /// Rows nests it under the first). Removes it from its previous axis. No-op
    /// if the category is not among this measure's dimensions. Pivoting is
    /// formula-free re-projection (the Improv/Quantrix signature move).
    fn set_axis(&mut self, model: &Model, category: CategoryId, axis: Axis) {
        if !self.axis_order.contains(&category) {
            return;
        }
        let (mut rows, mut cols, mut pages) = (
            self.row_cats(model),
            self.col_cats(model),
            self.page_cats(model),
        );
        for v in [&mut rows, &mut cols, &mut pages] {
            v.retain(|c| *c != category);
        }
        match axis {
            Axis::Rows => rows.push(category),
            Axis::Columns => cols.push(category),
            Axis::Pages => pages.push(category),
        }
        self.rebuild_axis_order(model, rows, cols, pages);
        self.clamp_cursor(model);
    }

    /// Flatten the three axis groups back into `axis_order` + `n_rows`/`n_cols`,
    /// and resize `page_idx` to the new page count.
    fn rebuild_axis_order(
        &mut self,
        model: &Model,
        rows: Vec<CategoryId>,
        cols: Vec<CategoryId>,
        pages: Vec<CategoryId>,
    ) {
        self.n_rows = rows.len();
        self.n_cols = cols.len();
        self.axis_order = rows;
        self.axis_order.extend(cols);
        self.axis_order.extend(pages);
        self.page_idx = vec![0; self.page_cats(model).len()];
    }

    /// Pivot: swap this matrix's entire row stack with its entire column stack
    /// (Rows ↔ Columns), keeping pages put. For the classic one-per-axis case
    /// this is the familiar row/column swap; with stacked categories it swaps
    /// the two groups. No-op if there is nothing on either of rows/columns.
    fn pivot_rotate(&mut self, model: &Model) {
        let rows = self.row_cats(model);
        let cols = self.col_cats(model);
        if rows.is_empty() && cols.is_empty() {
            return;
        }
        let pages = self.page_cats(model);
        // Swap: old columns become rows, old rows become columns.
        self.rebuild_axis_order(model, cols, rows, pages);
        self.clamp_cursor(model);
    }

    /// Set the pinned item index for page dimension `dim_index` (its position
    /// among this matrix's page dims), clamped to that dimension's item count.
    /// No-op if out of range.
    fn set_page(&mut self, model: &Model, dim_index: usize, item_index: usize) {
        let pages = self.page_cats(model);
        let Some(cat) = pages.get(dim_index).copied() else {
            return;
        };
        let count = self.sorted_items(model, cat).len();
        if count == 0 {
            return;
        }
        if self.page_idx.len() != pages.len() {
            self.page_idx.resize(pages.len(), 0);
        }
        if let Some(slot) = self.page_idx.get_mut(dim_index) {
            *slot = item_index.min(count - 1);
        }
    }

    /// Cycle this matrix's first page dimension by `delta` (wrapping).
    fn page_first(&mut self, model: &Model, delta: isize) {
        let Some(cat) = self.page_cats(model).first().copied() else {
            return;
        };
        let count = self.sorted_items(model, cat).len();
        if count == 0 {
            return;
        }
        let cur = self.page_idx.first().copied().unwrap_or(0).min(count - 1);
        let next = (cur as isize + delta).rem_euclid(count as isize) as usize;
        self.set_page(model, 0, next);
    }

    /// Toggle whether `item` of `category` is shown in THIS matrix. On first
    /// toggle the filter starts from all items minus this one; toggling so all
    /// items are kept drops the filter. Presentation only.
    fn toggle_filter_item(&mut self, model: &Model, category: CategoryId, item: ItemId) {
        let all: Vec<ItemId> = model
            .categories
            .get(&category)
            .map(|c| c.items.clone())
            .unwrap_or_default();
        match self.filters.iter().position(|f| f.category == category) {
            None => {
                let items: Vec<ItemId> = all.into_iter().filter(|i| *i != item).collect();
                self.filters.push(Filter { category, items });
            }
            Some(i) => {
                let f = &mut self.filters[i];
                if let Some(p) = f.items.iter().position(|x| *x == item) {
                    f.items.remove(p);
                } else {
                    f.items.push(item);
                }
                if f.items.len() == all.len() && all.iter().all(|x| f.items.contains(x)) {
                    self.filters.remove(i);
                }
            }
        }
        self.clamp_cursor(model);
    }

    /// Clear this matrix's filters, showing every item again.
    fn clear_filters(&mut self, model: &Model) {
        self.filters.clear();
        self.clamp_cursor(model);
    }

    /// This matrix's (row_count, col_count). An axis with NO categories is
    /// genuinely scalar and has ONE line; an axis whose category is filtered to
    /// zero items has ZERO lines (an empty grid with headers — never a synthetic
    /// line whose coordinate would omit that category). If a PAGE category is
    /// filtered to zero items nothing is addressable at all, so both counts are
    /// 0. Matches `render_grid`.
    fn grid_dims(&self, model: &Model) -> (usize, usize) {
        if self.pinned_pages_opt(model).is_none() {
            return (0, 0);
        }
        let rows = product_len(&self.axis_item_lists(model, &self.row_cats(model)));
        let cols = product_len(&self.axis_item_lists(model, &self.col_cats(model)));
        (rows, cols)
    }

    /// Move this matrix's cursor by `(drow, dcol)`, clamped to its grid (never
    /// out of range). Mirrors the TUI's `move_cursor`.
    fn move_cursor(&mut self, model: &Model, drow: isize, dcol: isize) {
        let (rows, cols) = self.grid_dims(model);
        let max_row = rows.saturating_sub(1) as isize;
        let max_col = cols.saturating_sub(1) as isize;
        self.cursor_row = (self.cursor_row as isize + drow).clamp(0, max_row) as usize;
        self.cursor_col = (self.cursor_col as isize + dcol).clamp(0, max_col) as usize;
    }

    /// Clamp the cursor into this matrix's grid (called after a pivot / measure
    /// switch that may have shrunk it).
    fn clamp_cursor(&mut self, model: &Model) {
        let (rows, cols) = self.grid_dims(model);
        self.cursor_row = self.cursor_row.min(rows.saturating_sub(1));
        self.cursor_col = self.cursor_col.min(cols.saturating_sub(1));
    }

    /// The `CoordKey` of the cell under this matrix's cursor, or `None` when
    /// there is no cell there: any axis (row, column, or page) category filtered
    /// to zero items means no coordinate fully specifies the measure's
    /// dimensions, so there is nothing to address. An axis with no categories at
    /// all is scalar in that direction and still has one line.
    fn cursor_key(&self, model: &Model) -> Option<CoordKey> {
        let pinned = self.pinned_pages_opt(model)?;
        let row_cats = self.row_cats(model);
        let col_cats = self.col_cats(model);
        let row_lists = self.axis_item_lists(model, &row_cats);
        let col_lists = self.axis_item_lists(model, &col_cats);
        let (n_rows, n_cols) = (product_len(&row_lists), product_len(&col_lists));
        if n_rows == 0 || n_cols == 0 {
            return None;
        }
        // Decode only the cursor's row/col line (never the whole product).
        // `product_len` of an empty list set is 1, the scalar axis -> empty tuple.
        let row_tuple = if row_lists.is_empty() {
            Vec::new()
        } else {
            nth_tuple(&row_lists, self.cursor_row.min(n_rows - 1))
        };
        let col_tuple = if col_lists.is_empty() {
            Vec::new()
        } else {
            nth_tuple(&col_lists, self.cursor_col.min(n_cols - 1))
        };
        Some(cell_key_multi(
            &row_cats, &row_tuple, &col_cats, &col_tuple, &pinned,
        ))
    }

    /// Resolved axes for this matrix: (first row cat, first col cat, pinned page
    /// dims). Test-only: rendering and the chart use the stacked (`_cats`) form.
    #[cfg(test)]
    fn resolved_axes(
        &self,
        model: &Model,
    ) -> (
        Option<CategoryId>,
        Option<CategoryId>,
        Vec<(CategoryId, ItemId)>,
    ) {
        let row_cat = self.axis_order.first().copied();
        let col_cat = self.axis_order.get(self.n_rows).copied();
        (row_cat, col_cat, self.pinned_pages(model))
    }
}

impl ImprovApp {
    /// Load a model from the store at `db` (`""` = fresh in-memory model) and
    /// build the live engine over its derived measures.
    pub fn load(db: &str) -> Result<ImprovApp, String> {
        let model = if db.is_empty() {
            Model::new()
        } else {
            let mut store = ModelStore::open(db).map_err(|e| e.to_string())?;
            store.load_model().map_err(|e| e.to_string())?
        };

        let (engine, snapshot) = build_engine(&model);
        let selected = pick_default_measure(&model);
        let matrices = vec![Matrix::new(&model, selected)];

        Ok(ImprovApp {
            db: db.to_string(),
            model,
            engine,
            snapshot,
            matrices,
            focus: 0,
            current_view: None,
            status: String::new(),
            formula_buf: String::new(),
            formula_for: None,
            formula_error_pos: None,
            formula_error_msg: String::new(),
            new_name: String::new(),
            new_formula: String::new(),
            view_name: String::new(),
            show_chart: false,
            chart_line: false,
            show_csv_wizard: false,
            import_form: ImportForm::default(),
            export_form: ExportForm::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            formula_row_rects: Vec::new(),
        })
    }

    // -- the canvas: N matrices, one focused -------------------------------

    /// The focused matrix — the one the keyboard, the formula bar, the
    /// inspector, the chart and the selection readout all follow.
    ///
    /// `matrices` is never empty (an empty model still has one matrix, showing
    /// no measure) and `focus` is clamped here rather than trusted, so removing
    /// a matrix can never leave a dangling focus.
    pub(crate) fn focused(&self) -> &Matrix {
        &self.matrices[self.focus.min(self.matrices.len() - 1)]
    }

    /// The focused matrix, mutably (see [`Self::focused`]).
    pub(crate) fn focused_mut(&mut self) -> &mut Matrix {
        let i = self.focus.min(self.matrices.len() - 1);
        &mut self.matrices[i]
    }

    /// Give the keyboard to matrix `i` (clamped). Focus is what makes grid
    /// shortcuts unambiguous with several matrices on one canvas.
    pub fn set_focus(&mut self, i: usize) {
        self.focus = i.min(self.matrices.len() - 1);
    }

    /// Which matrix has the keyboard.
    pub fn focus_index(&self) -> usize {
        self.focus.min(self.matrices.len() - 1)
    }

    /// How many matrices are on the canvas (always >= 1).
    pub fn matrix_count(&self) -> usize {
        self.matrices.len()
    }

    /// Add a matrix showing `measure` to the canvas, cascaded down-right from
    /// the last one so it does not land exactly on top of it, and focus it.
    /// Returns its index.
    ///
    /// Canvas layout only — the model is untouched, so this is deliberately NOT
    /// an undo point; see [`Self::undo`].
    pub fn add_matrix(&mut self, measure: Option<MeasureId>) -> usize {
        let mut m = Matrix::new(&self.model, measure);
        let last = self.matrices.last().map(|m| m.rect).unwrap_or_default();
        m.rect = CanvasRect {
            x: last.x + CASCADE,
            y: last.y + CASCADE,
            w: last.w,
            h: last.h,
        };
        self.matrices.push(m);
        self.focus = self.matrices.len() - 1;
        self.focus
    }

    /// Remove matrix `i` from the canvas. The LAST matrix is never removed (a
    /// canvas with none has nowhere to show a measure) — that is a no-op
    /// returning `false`. Layout only, like [`Self::add_matrix`].
    pub fn remove_matrix(&mut self, i: usize) -> bool {
        if self.matrices.len() <= 1 || i >= self.matrices.len() {
            return false;
        }
        self.matrices.remove(i);
        self.focus = self.focus.min(self.matrices.len() - 1);
        true
    }

    // -- read-only accessors for the chart module (crate-internal) ---------

    pub(crate) fn selected(&self) -> Option<MeasureId> {
        self.focused().measure
    }
    /// Row/column category stacks and pinned pages for the current pivot — the
    /// general (stacked) form the chart needs. Row/col tuples come from
    /// `axis_tuples_pub`; keys from `cell_key_multi_pub`.
    ///
    /// `None` when a PAGE category is filtered to zero items: nothing can be
    /// pinned for it, so every cell key would omit that category. Same rule as
    /// the grid (`grid_dims`/`cursor_key`) — chart nothing rather than plot
    /// under-specified keys.
    ///
    /// Reads the FOCUSED matrix: with several matrices on the canvas the chart
    /// follows the keyboard, as every other single-measure surface does.
    pub(crate) fn chart_axes_pub(&self) -> Option<ChartAxes> {
        let m = self.focused();
        Some((
            m.row_cats(&self.model),
            m.col_cats(&self.model),
            m.pinned_pages_opt(&self.model)?,
        ))
    }
    /// The Cartesian product of `cats`' filtered items (see `axis_tuples`),
    /// exposed for the chart. Empty `cats` -> one empty tuple.
    pub(crate) fn axis_tuples_pub(&self, cats: &[CategoryId]) -> Vec<Vec<(ItemId, String)>> {
        self.focused().axis_tuples(&self.model, cats)
    }
    /// The sorted `CoordKey` for a stacked cell (see `cell_key_multi`).
    pub(crate) fn cell_key_multi_pub(
        &self,
        row_cats: &[CategoryId],
        row_tuple: &[(ItemId, String)],
        col_cats: &[CategoryId],
        col_tuple: &[(ItemId, String)],
        pinned: &[(CategoryId, ItemId)],
    ) -> CoordKey {
        cell_key_multi(row_cats, row_tuple, col_cats, col_tuple, pinned)
    }
    pub(crate) fn values_for_pub(&self, measure: MeasureId) -> HashMap<CoordKey, f64> {
        self.values_for(measure)
    }
    pub(crate) fn category_name_pub(&self, c: CategoryId) -> Option<String> {
        self.model.categories.get(&c).map(|x| x.name.0.clone())
    }

    // -- pure state logic (unit-tested; no rendering) ----------------------

    /// The numeric value map for a measure (input cells, or the derived
    /// snapshot), projected to numbers for the CHART, which can only plot
    /// numbers. Non-numeric cells are absent (a gap).
    ///
    /// The GRID must not use this: a Text/Boolean/DateTime cell would render
    /// blank. Display goes through [`Self::cell_text`], which is type-aware.
    fn values_for(&self, measure: MeasureId) -> HashMap<CoordKey, f64> {
        let is_derived = self.model.measures.get(&measure).map(|m| m.is_derived());
        match is_derived {
            Some(true) => self
                .snapshot
                .get(&measure)
                .map(|m| {
                    m.iter()
                        .filter_map(|(k, v)| v.as_num().map(|n| (k.clone(), n)))
                        .collect()
                })
                .unwrap_or_default(),
            _ => self
                .model
                .inputs
                .iter()
                .filter(|((mid, _), _)| *mid == measure)
                .filter_map(|((_, coord), v)| match v {
                    Value::Number(n) => Some((encode_coord(coord), *n)),
                    _ => None,
                })
                .collect(),
        }
    }

    /// The typed value of a cell: the engine snapshot for a derived measure,
    /// the model's stored `Value` (mapped to a `CellValue`) for an input one.
    /// `None` = the cell genuinely holds nothing.
    fn cell_value(&self, measure: MeasureId, key: &CoordKey) -> Option<CellValue> {
        if self
            .model
            .measures
            .get(&measure)
            .is_some_and(|m| m.is_derived())
        {
            return self
                .snapshot
                .get(&measure)
                .and_then(|m| m.get(key))
                .cloned();
        }
        self.model
            .input(measure, &decode(key))
            .and_then(CellValue::from_model_value)
    }

    /// The display text for any cell — derived OR input — via `CellValue`'s
    /// `Display`: numbers as numbers, text as text, booleans as `true`/`false`,
    /// dates as RFC3339, error cells as `#ERR`. A cell is blank only when it
    /// holds no value, never merely because its value is not a number.
    fn cell_text(&self, measure: MeasureId, key: &CoordKey) -> Option<String> {
        self.cell_value(measure, key).map(|v| v.to_string())
    }

    /// Set an input cell to a TYPED value and push the edit through the live
    /// engine, refreshing the snapshot.
    ///
    /// `value`'s type must match the measure's DECLARED `value_type`: a Text
    /// measure refuses a `Value::Number` (and vice versa) rather than silently
    /// replacing a typed cell with a number. Editing UI must parse by declared
    /// type — see [`Self::commit_cell_text`].
    ///
    /// Returns an error string on failure (derived cell, type mismatch, or a
    /// failed autosave); on failure the model, engine, and snapshot are left as
    /// they were.
    pub fn set_cell(
        &mut self,
        measure: MeasureId,
        coord: CoordKey,
        value: Value,
    ) -> Result<(), String> {
        self.write_cell(measure, coord, Some(value))
    }

    /// Remove an input cell's value (the empty-commit path) and push the
    /// retraction through the live engine. Same atomicity as [`Self::set_cell`].
    pub fn clear_cell(&mut self, measure: MeasureId, coord: CoordKey) -> Result<(), String> {
        self.write_cell(measure, coord, None)
    }

    /// The one write path for input cells: `Some(v)` sets, `None` clears.
    fn write_cell(
        &mut self,
        measure: MeasureId,
        coord: CoordKey,
        value: Option<Value>,
    ) -> Result<(), String> {
        let m = self
            .model
            .measures
            .get(&measure)
            .ok_or_else(|| format!("no measure with id {}", measure.0))?;
        if m.is_derived() {
            return Err("derived cells are computed, not editable".into());
        }
        // The declared type is the contract: never coerce, never overwrite a
        // typed cell with a value of another type.
        if let Some(v) = &value {
            if v.type_of() != Some(m.value_type) {
                return Err(format!(
                    "measure '{}' is declared {:?}; refusing to store {v:?}",
                    m.name.0, m.value_type
                ));
            }
        }
        // Precise rollback rather than publish()'s clone-and-swap: a single cell
        // edit goes through the INCREMENTAL engine (`engine.set`), so cloning the
        // whole model and rebuilding the graph per keystroke would throw away the
        // very thing that makes editing cheap. Instead remember the prior value
        // and undo both the model and the engine if anything downstream fails, so
        // a reported failure never leaves memory diverged from the store.
        let key = decode(&coord);
        let prior = self.model.input(measure, &key).cloned();
        let prior_snapshot = self.snapshot.clone();
        // The undo point: a clone of the model as it is right now. Taken before
        // the write, recorded only once the write (engine + save) succeeded.
        let prior_model = self.model.clone();

        match &value {
            Some(v) => self.model.set_input(measure, key.clone(), v.clone()),
            None => {
                self.model.inputs.remove(&(measure, key.clone()));
            }
        }
        let outcome = (|| -> Result<(), String> {
            self.push_engine(measure, &coord, value.as_ref())?;
            self.save()
        })();

        if let Err(e) = outcome {
            // Restore the model, then push the restored value back through the
            // engine so its graph and our snapshot agree with the model again.
            match &prior {
                Some(v) => self.model.set_input(measure, key, v.clone()),
                None => {
                    self.model.inputs.remove(&(measure, key));
                }
            }
            if self.push_engine(measure, &coord, prior.as_ref()).is_err() {
                self.snapshot = prior_snapshot;
            }
            return Err(e);
        }
        self.record_history(prior_model);
        Ok(())
    }

    /// Push one input-cell edit through the live engine and adopt the recomputed
    /// snapshot. No engine (no derived measures) -> nothing to do.
    ///
    /// Only NUMBERS enter the dataflow's numeric lane, so a typed
    /// (text/boolean/date/error) value CLEARS the engine's cell — exactly what a
    /// reload would seed (`Engine::new` seeds numeric inputs only). Without
    /// that, retyping a numeric cell as text would leave derived measures
    /// computing with a number the cell no longer holds.
    fn push_engine(
        &mut self,
        measure: MeasureId,
        coord: &CoordKey,
        value: Option<&Value>,
    ) -> Result<(), String> {
        let Some(engine) = &mut self.engine else {
            return Ok(());
        };
        let snapshot = match value.and_then(|v| v.as_number()) {
            Some(n) => engine.set(measure, coord.clone(), n),
            None => engine.clear(measure, coord.clone()),
        }
        .map_err(|e| e.to_string())?;
        self.snapshot = snapshot;
        Ok(())
    }

    /// Commit a grid edit buffer for `measure[coord]`, interpreting the text by
    /// the measure's DECLARED `value_type`. Returns the status message.
    ///
    /// * An EMPTY (or whitespace-only) buffer **clears the cell**. Escape
    ///   already cancels an edit, so an empty commit is the GUI's only way to
    ///   delete a value; treating it as a second cancel would make cells
    ///   un-clearable.
    /// * Otherwise the declared type wins over the text's shape: `"42"` typed
    ///   into a Text measure stores `Value::Text("42")`, never a number.
    /// * Text that does not parse as the declared type is REJECTED (`Err`); the
    ///   prior typed value stays exactly as it was.
    pub fn commit_cell_text(
        &mut self,
        measure: MeasureId,
        coord: CoordKey,
        text: &str,
    ) -> Result<String, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            self.clear_cell(measure, coord)?;
            return Ok("cell cleared".into());
        }
        let declared = self
            .model
            .measures
            .get(&measure)
            .map(|m| m.value_type)
            .ok_or_else(|| format!("no measure with id {}", measure.0))?;
        let value = parse_typed(&self.model, declared, trimmed)?;
        self.set_cell(measure, coord, value)?;
        Ok("cell updated".into())
    }

    // -- pivot / page state: delegated to the FOCUSED matrix ---------------
    //
    // Each matrix on the canvas owns its own axis order, filters and cursor
    // (see [`Matrix`]). These wrappers keep the app-level API the panels and
    // tests use and route it at the focused matrix; the per-matrix rendering
    // path calls the `Matrix` methods directly with its own index.

    /// Run `f` on the focused matrix with the model alongside it. Two disjoint
    /// field borrows (`matrices` mutably, `model` immutably), which is why the
    /// per-matrix methods take `&Model` instead of holding one.
    fn with_focused<R>(&mut self, f: impl FnOnce(&mut Matrix, &Model) -> R) -> R {
        let i = self.focus.min(self.matrices.len() - 1);
        f(&mut self.matrices[i], &self.model)
    }

    /// Re-home every matrix's pivot state whose measure changed, and clamp the
    /// focus. Called each frame before rendering.
    fn sync_axis_state(&mut self) {
        // `matrices` is never empty, so `focus` always has something to point at.
        if self.matrices.is_empty() {
            self.matrices.push(Matrix::new(&self.model, None));
        }
        self.focus = self.focus.min(self.matrices.len() - 1);
        let model = &self.model;
        for m in &mut self.matrices {
            m.sync_axis_state(model);
        }
    }

    /// Pivot the focused matrix: swap its row and column stacks (the toolbar's
    /// and the grid header's `Pivot` button).
    pub fn pivot_rotate(&mut self) {
        self.with_focused(|m, model| m.pivot_rotate(model));
    }

    // The rest of the focused matrix's pivot surface reads the same way, and is
    // reached per-matrix-index from the rendering path (a click drives THAT
    // matrix, never the focused one). These focused-matrix wrappers exist for
    // the tests, which assert app-level behavior; production code calls the
    // [`Matrix`] methods with the index it is rendering.
    #[cfg(test)]
    fn resolved_axes(
        &self,
    ) -> (
        Option<CategoryId>,
        Option<CategoryId>,
        Vec<(CategoryId, ItemId)>,
    ) {
        self.focused().resolved_axes(&self.model)
    }
    #[cfg(test)]
    fn row_cats(&self) -> Vec<CategoryId> {
        self.focused().row_cats(&self.model)
    }
    #[cfg(test)]
    fn col_cats(&self) -> Vec<CategoryId> {
        self.focused().col_cats(&self.model)
    }
    #[cfg(test)]
    fn page_cats(&self) -> Vec<CategoryId> {
        self.focused().page_cats(&self.model)
    }
    #[cfg(test)]
    fn axis_tuples(&self, cats: &[CategoryId]) -> Vec<Vec<(ItemId, String)>> {
        self.focused().axis_tuples(&self.model, cats)
    }
    #[cfg(test)]
    fn axis_item_lists(&self, cats: &[CategoryId]) -> Vec<Vec<(ItemId, String)>> {
        self.focused().axis_item_lists(&self.model, cats)
    }
    #[cfg(test)]
    fn sorted_items(&self, c: CategoryId) -> Vec<(ItemId, String)> {
        self.focused().sorted_items(&self.model, c)
    }
    /// Move `category` to `axis` in the FOCUSED matrix (see
    /// [`Matrix::set_axis`]). Pivoting is formula-free re-projection.
    #[cfg(test)]
    fn set_axis(&mut self, category: CategoryId, axis: Axis) {
        self.with_focused(|m, model| m.set_axis(model, category, axis));
    }
    /// Set the focused matrix's pinned item for page dimension `dim_index`.
    #[cfg(test)]
    fn set_page(&mut self, dim_index: usize, item_index: usize) {
        self.with_focused(|m, model| m.set_page(model, dim_index, item_index));
    }

    // -- views & filters (pure; unit-tested without egui) ------------------

    /// Build a `View` capturing the WHOLE canvas: every matrix's measure, axis
    /// order, pinned page items, filters and geometry. Presentation only.
    ///
    /// Matrix 0 becomes the view's *primary* matrix (its flat fields), the rest
    /// its `placements` — which is exactly what makes a one-matrix canvas save
    /// as a view a pre-canvas Improv would also understand. A matrix with no
    /// measure (an empty model) contributes nothing; if that leaves no matrices
    /// at all there is no view to build (`None`).
    pub fn build_view(&self, id: ViewId, name: &str) -> Option<View> {
        let mut placements: Vec<MatrixPlacement> = self
            .matrices
            .iter()
            .filter_map(|m| m.to_placement(&self.model))
            .collect();
        if placements.is_empty() {
            return None;
        }
        let primary = placements.remove(0);
        Some(View::from_matrices(
            id,
            Name(name.to_string()),
            primary,
            placements,
        ))
    }

    /// The smallest unused view id (>= 1).
    fn next_view_id(&self) -> ViewId {
        ViewId(
            self.model
                .views
                .keys()
                .map(|v| v.0)
                .max()
                .map(|m| m + 1)
                .unwrap_or(1),
        )
    }

    /// Save the current canvas as a named view: mint an id, add it to the
    /// model, and autosave. Returns the id, or `None` when nothing was saved
    /// (no measure on the canvas, blank name, or a failed store write — in which
    /// case the view is rolled back and `status` holds the error).
    pub fn save_view(&mut self, name: &str) -> Option<ViewId> {
        let name = name.trim();
        if name.is_empty() {
            self.status = "view name is required".into();
            return None;
        }
        let id = self.next_view_id();
        let view = self.build_view(id, name)?;
        let prior_model = self.model.clone();
        self.model.add_view(view);
        if let Err(e) = self.save() {
            self.model.views.remove(&id);
            self.status = e;
            return None;
        }
        self.record_history(prior_model);
        self.current_view = Some(id);
        self.status = format!("saved view '{name}'");
        Some(id)
    }

    /// Apply a saved `View` to the canvas: rebuild the matrices from its
    /// placements (measure, axis order, page items, filters, geometry), focus
    /// the first, and mark it the current document tab. Presentation only —
    /// measures and data untouched.
    ///
    /// **Legacy views load unchanged.** A view written before canvases has no
    /// `placements`, so `View::matrices()` yields exactly one matrix — its flat
    /// fields — and the canvas is the single-matrix canvas it always was, down
    /// to the default geometry.
    ///
    /// A placement whose measure no longer exists is dropped rather than shown
    /// blank; if that leaves nothing the view is refused and the canvas is left
    /// exactly as it was.
    pub fn apply_view(&mut self, view: &View) {
        let matrices: Vec<Matrix> = view
            .matrices()
            .iter()
            .filter_map(|p| Matrix::from_placement(&self.model, p))
            .collect();
        if matrices.is_empty() {
            self.status = "view's measure no longer exists".into();
            return;
        }
        let dropped = view.matrices().len() - matrices.len();
        self.matrices = matrices;
        self.focus = 0;
        self.current_view = Some(view.id);
        self.status = if dropped == 0 {
            format!("view: {}", view.name.0)
        } else {
            format!(
                "view: {} ({dropped} matrix(es) skipped: measure gone)",
                view.name.0
            )
        };
    }

    /// The saved views as document tabs, in id order: `(id, name, is current)`.
    /// The reference's `Welcome / Concepts / P&L Canvas` strip — switching tabs
    /// switches canvases via [`Self::apply_view`].
    pub fn document_tabs(&self) -> Vec<(ViewId, String, bool)> {
        let mut ids: Vec<ViewId> = self.model.views.keys().copied().collect();
        ids.sort_by_key(|v| v.0);
        ids.into_iter()
            .map(|id| {
                (
                    id,
                    self.model.views[&id].name.0.clone(),
                    self.current_view == Some(id),
                )
            })
            .collect()
    }

    /// Switch to the saved view `id` (a document-tab click). No-op on an
    /// unknown id.
    pub fn open_view(&mut self, id: ViewId) {
        if let Some(v) = self.model.views.get(&id).cloned() {
            self.apply_view(&v);
        }
    }

    /// Toggle whether `item` of `category` is shown in the FOCUSED matrix. On
    /// first toggle the filter starts from all items minus this one; toggling so
    /// all items are kept drops the filter. Presentation only. (The filter shelf
    /// drives [`Matrix::toggle_filter_item`] on the matrix it belongs to.)
    #[cfg(test)]
    fn toggle_filter_item(&mut self, category: CategoryId, item: ItemId) {
        self.with_focused(|m, model| m.toggle_filter_item(model, category, item));
    }

    // -- CSV/TSV import/export wizard (pure orchestration; unit-tested) ----

    /// Toggle the CSV/TSV wizard panel. On first open, prefill the import
    /// form's measure id with the next free one (mirrors `next_view_id`'s
    /// auto-assign pattern) if the field is still blank.
    pub fn toggle_csv_wizard(&mut self) {
        self.show_csv_wizard = !self.show_csv_wizard;
        if self.show_csv_wizard && self.import_form.measure_id.trim().is_empty() {
            self.import_form.measure_id = self.next_measure_id().to_string();
        }
    }

    /// Run the import wizard: build an `ImportSpec` from `self.import_form`,
    /// import it into the live model, rebuild the engine (structure changed),
    /// autosave, and report the cell count (or the error) in `self.status`.
    /// Never panics — every failure path (bad form, bad file, `CsvError`)
    /// lands in `self.status` as a clear message.
    pub fn run_csv_import(&mut self) {
        let spec = match csv_wizard::build_import_spec(&self.import_form) {
            Ok(s) => s,
            Err(e) => {
                self.status = format!("import error: {e}");
                return;
            }
        };
        // `import_csv` is read-only on the model until it commits, so a failed
        // import leaves nothing to undo; the snapshot is kept only on success.
        let prior_model = self.model.clone();
        match improv_storage_csv::import_csv(&mut self.model, &spec) {
            Ok(n) => {
                self.rebuild_engine();
                self.focused_mut().select(Some(spec.measure_id));
                // The model changed whether or not the store write worked, so
                // the import is undoable either way.
                self.record_history(prior_model);
                // A failed autosave is reported as the failure it is; the
                // imported cells are already in the live model.
                self.status = match self.save() {
                    Ok(()) => format!(
                        "imported {n} cell(s) into measure {} '{}'",
                        spec.measure_id.0, spec.measure_name
                    ),
                    Err(e) => e,
                };
            }
            Err(e) => self.status = format!("import error: {e}"),
        }
    }

    /// Run the export wizard: resolve `self.export_form` and write the
    /// selected measure's cells to disk, reporting the row count (or the
    /// error) in `self.status`. Never panics.
    pub fn run_csv_export(&mut self) {
        let (measure, path, delimiter) = match csv_wizard::build_export_args(&self.export_form) {
            Ok(a) => a,
            Err(e) => {
                self.status = format!("export error: {e}");
                return;
            }
        };
        match improv_storage_csv::export_measure_csv(&self.model, measure, &path, delimiter) {
            Ok(n) => {
                self.status = format!("exported {n} row(s) to {}", path.display());
            }
            Err(e) => self.status = format!("export error: {e}"),
        }
    }

    /// Rebuild the live engine after a structural change (formula edit / new
    /// derived measure) and refresh the snapshot.
    fn rebuild_engine(&mut self) {
        let (engine, snapshot) = build_engine(&self.model);
        self.engine = engine;
        self.snapshot = snapshot;
    }

    // -- keyboard cell cursor: delegated to the FOCUSED matrix -------------

    /// The focused matrix's (row_count, col_count) (see [`Matrix::grid_dims`]).
    #[cfg(test)]
    fn grid_dims(&self) -> (usize, usize) {
        self.focused().grid_dims(&self.model)
    }

    /// Move the focused matrix's cursor by `(drow, dcol)`, clamped to its grid.
    /// Mirrors the TUI's `move_cursor`.
    pub fn move_cursor(&mut self, drow: isize, dcol: isize) {
        self.with_focused(|m, model| m.move_cursor(model, drow, dcol));
    }

    /// The `CoordKey` of the cell under the focused matrix's cursor (see
    /// [`Matrix::cursor_key`]).
    pub fn cursor_key(&self) -> Option<CoordKey> {
        self.focused().cursor_key(&self.model)
    }

    /// True if the focused matrix's cursor cell is an editable input cell: its
    /// measure is an input measure AND the cursor addresses a real,
    /// fully-specified coordinate (see `cursor_key`). Derived measures are
    /// read-only.
    pub fn cursor_is_editable(&self) -> bool {
        self.selected()
            .and_then(|m| self.model.measures.get(&m))
            .is_some_and(|m| !m.is_derived())
            && self.cursor_key().is_some()
    }

    /// Begin editing the focused matrix's cursor cell if it is editable, seeding
    /// its buffer with the current value. On a derived cell, sets the status
    /// message instead. Mirrors the TUI's `begin_edit`.
    fn begin_edit_cursor(&mut self) {
        let Some(measure) = self.selected() else {
            return;
        };
        // Order matters: report a missing cell (empty filtered axis) before the
        // derived check, so the message names the real reason.
        let Some(key) = self.cursor_key() else {
            self.status = "no cell here: an axis category is filtered to nothing".into();
            return;
        };
        if !self.cursor_is_editable() {
            self.status = "derived cells are computed, not editable".into();
            return;
        }
        let seed = self.cell_text(measure, &key).unwrap_or_default();
        let m = self.focused_mut();
        m.editing = Some((measure, key));
        m.edit_buf = seed;
    }

    /// The editable source text the formula bar shows for `measure`: symbolic
    /// DSL (`Price * Quantity`) whenever the v1 grammar can spell the formula
    /// exactly, else the controlled-English description (`Price times
    /// Quantity`).
    ///
    /// `None` when there is nothing editable to show: `measure` is unknown or
    /// an input measure, or *neither* surface language can spell the formula.
    /// Since plan Step 4b the printer quotes awkward names (`'Unit Price'`), so
    /// a CSV-imported measure name is no longer a reason to go read-only; what
    /// remains unspellable is a formula *shape* the v1 grammar has no form for
    /// (see [`formula_dsl`]) whose controlled-English rendering also fails to
    /// round-trip. The bar renders that read-only instead of inviting a commit
    /// of text that no parser accepts.
    ///
    /// Whatever this returns, [`Self::commit_formula`] accepts unchanged and
    /// leaves the identical AST — the invariant this pair exists to keep. It is
    /// *checked* here, by reparsing the candidate text through the very same
    /// [`Self::parse_formula_text`] `commit_formula` uses, so no printer/parser
    /// drift can quietly violate it.
    pub fn formula_source(&self, measure: MeasureId) -> Option<String> {
        let Some(MeasureKind::Derived(f)) = self.model.measures.get(&measure).map(|m| &m.kind)
        else {
            return None;
        };
        let dsl = formula_dsl(&self.model, f);
        let cnl = describe_formula(&NlContext::new(&self.model), f);
        dsl.into_iter()
            .chain(std::iter::once(cnl))
            .find(|text| self.parse_formula_text(text).ok().as_ref() == Some(f))
    }

    /// Parse `text` as the RHS expression for an existing measure and make it
    /// derived (replacing any prior formula/input kind). Rebuilds the engine
    /// (structure changed), refreshes the snapshot, and autosaves.
    ///
    /// **Atomic:** the change is validated on a *candidate* model (parse, then
    /// a real engine build) and only published once the engine is known good
    /// and the store write succeeded. On any failure the previous model,
    /// engine, and snapshot are all left intact and the error is returned; on a
    /// parse error `formula_error_pos`/`formula_error_msg` are also set (for the
    /// formula bar's inline highlight). They are cleared the moment `text`
    /// parses — a later save failure is a save error, not an inline position in
    /// text that has no error at that offset.
    ///
    /// `text` may be either the symbolic DSL (`Price * Quantity`) or the
    /// controlled English the formula bar displays (`Price times Quantity`);
    /// see [`Self::formula_source`].
    pub fn commit_formula(&mut self, measure: MeasureId, text: &str) -> Result<(), String> {
        let formula = match self.parse_formula_text(text) {
            Ok(f) => f,
            Err(e) => {
                self.formula_error_pos = e.position;
                self.formula_error_msg = e.to_string();
                return Err(e.to_string());
            }
        };
        // `text` parsed: any previous inline parse error no longer describes
        // it. Clear BEFORE publishing, so a failed save does not leave a red
        // underline at an offset where this text is perfectly fine.
        self.formula_error_pos = None;
        self.formula_error_msg.clear();
        // Validate on a candidate copy: a formula that parses can still fail to
        // build (cycle, type/dimension error), which would otherwise leave the
        // app with a saved-but-broken model and a dead engine.
        let mut candidate = self.model.clone();
        let m = candidate
            .measures
            .get_mut(&measure)
            .ok_or_else(|| format!("no measure with id {}", measure.0))?;
        m.kind = MeasureKind::Derived(formula);
        self.publish(candidate)?;
        Ok(())
    }

    /// Whether `measure`'s formula is currently enabled (the reference's `✓`).
    /// True for an input measure too — it has no formula to disable, so there is
    /// no unchecked state for it to be in.
    pub fn formula_enabled(&self, measure: MeasureId) -> bool {
        self.model
            .measures
            .get(&measure)
            .is_some_and(|m| !formula_disabled(m))
    }

    /// Enable or disable `measure`'s formula — the reference's per-formula
    /// checkmark, as a real **model** operation (see [`DISABLED_MARKER`]).
    ///
    /// Disabled, the measure keeps its formula but computes nothing, and every
    /// dependent recomputes without it. Because the flag lives on the measure's
    /// `description`, it survives save/reload unchanged.
    ///
    /// Atomic and undoable on exactly the same terms as [`Self::commit_formula`]
    /// — it goes through [`Self::publish`], so a toggle that cannot build or
    /// cannot save leaves the model, engine and snapshot untouched.
    ///
    /// A no-op (still `Ok`) on an input measure or an unknown id: neither has a
    /// formula, so there is nothing to toggle and nothing to report.
    pub fn set_formula_enabled(&mut self, measure: MeasureId, enabled: bool) -> Result<(), String> {
        let Some(m) = self.model.measures.get(&measure) else {
            return Ok(());
        };
        // Nothing to toggle: an input measure has no formula, and a formula
        // already in the requested state needs no rebuild (and must not become
        // a spurious undo point).
        if !m.is_derived() || !formula_disabled(m) == enabled {
            return Ok(());
        }
        let mut candidate = self.model.clone();
        let m = candidate
            .measures
            .get_mut(&measure)
            .ok_or_else(|| format!("no measure with id {}", measure.0))?;
        // Preserve the user's own description text across the toggle.
        let (_, user) = split_marker(m.description.as_deref());
        m.description = join_marker(!enabled, user);
        self.publish(candidate)
    }

    /// Every formula in the model, in id order, as the formula pane lists them
    /// (see [`FormulaRow`] and [`Self::formula_list_panel`]). Numbering is the
    /// caller's: row `i` is displayed as `i + 1`, matching the reference's
    /// 1-based list.
    ///
    /// Derived measures only — an input measure has no formula, so it has no row.
    pub fn formula_rows(&self) -> Vec<FormulaRow> {
        let mut ids: Vec<MeasureId> = self
            .model
            .measures
            .values()
            .filter(|m| m.is_derived())
            .map(|m| m.id)
            .collect();
        ids.sort_by_key(|m| m.0);
        ids.into_iter()
            .filter_map(|id| {
                let m = self.model.measures.get(&id)?;
                let MeasureKind::Derived(f) = &m.kind else {
                    return None;
                };
                // Prefer the editable spelling the bar would show; fall back to
                // the controlled English so a row is never blank.
                let text = self
                    .formula_source(id)
                    .unwrap_or_else(|| describe_formula(&NlContext::new(&self.model), f));
                Some(FormulaRow {
                    measure: id,
                    target: m.name.0.clone(),
                    text,
                    enabled: !formula_disabled(m),
                    error: self.formula_row_error(id, f),
                })
            })
            .collect()
    }

    /// The inline error for one formula row, or `None` if the formula is sound.
    ///
    /// Two distinct failures, both of which the reference's list shows in place
    /// rather than hiding:
    ///
    /// * It does not **build** — a type/dimension error or a cycle. Found by
    ///   compiling this one formula, not by rebuilding the whole engine, so
    ///   listing N formulas costs N compiles and no dataflow graphs.
    /// * It does not **round-trip** — no surface spelling of it parses back to
    ///   the identical AST, so the text shown is a description, not source. Said
    ///   plainly, because that row is the read-only one in the editor above.
    ///
    /// A DISABLED formula is still checked: an unchecked row with a broken
    /// formula should say so, not look clean because it is not running.
    fn formula_row_error(&self, id: MeasureId, f: &improv_core_model::Formula) -> Option<String> {
        let ctx = improv_engine::compiler::CompileContext::new(&self.model.measures);
        if let Err(e) = improv_engine::compiler::compile_formula(&ctx, id, f) {
            return Some(e.to_string());
        }
        self.formula_source(id)
            .is_none()
            .then(|| "no editable spelling (shown as description)".to_string())
    }

    /// Parse formula text in either supported surface language: the symbolic
    /// DSL first (the primary language, and the one whose error positions the
    /// formula bar highlights), then the controlled English of
    /// `improv_nl_formula`. The DSL error is what surfaces if both fail.
    fn parse_formula_text(&self, text: &str) -> Result<improv_core_model::Formula, ParseError> {
        match parser::parse_expr(&self.model, text) {
            Ok(f) => Ok(f),
            Err(dsl_err) => {
                parse_nl_formula(&NlContext::new(&self.model), text).map_err(|_| dsl_err)
            }
        }
    }

    /// Replace the model with `candidate`, but only if it builds a working
    /// engine and persists: build first, save second, publish third. On failure
    /// nothing is touched (`self` keeps its model, engine, and snapshot). On
    /// success the replaced model becomes an undo point.
    ///
    /// ponytail: validation is a full model clone + a full engine rebuild per
    /// structural edit (briefly two live engines). That is the price of
    /// atomicity at GUI edit rates; if formula editing ever needs to be
    /// interactive on huge models, validate against a compile-only pass
    /// (`engine::compiler::compile_formula` + `derived_build_order`) instead of
    /// a real `Engine::new`.
    fn publish(&mut self, candidate: Model) -> Result<(), String> {
        let previous = self.swap_model(candidate)?;
        self.record_history(previous);
        Ok(())
    }

    /// The atomic model swap behind [`Self::publish`] and undo/redo: build an
    /// engine for `candidate`, save it, and only then adopt it (model, engine,
    /// snapshot together). Returns the model it replaced. Records no history —
    /// callers decide (an undo must not become an undo point of its own).
    fn swap_model(&mut self, candidate: Model) -> Result<Model, String> {
        let (engine, snapshot) = try_build_engine(&candidate)?;
        let previous = std::mem::replace(&mut self.model, candidate);
        if let Err(e) = self.save() {
            self.model = previous;
            return Err(e);
        }
        self.engine = engine;
        self.snapshot = snapshot;
        Ok(previous)
    }

    // -- undo / redo -------------------------------------------------------

    /// Record `prior` (the model as it was *before* the mutation that just
    /// succeeded) as an undo point, and drop the redo stack — a fresh mutation
    /// makes the undone future unreachable (standard semantics).
    fn record_history(&mut self, prior: Model) {
        push_bounded(&mut self.undo_stack, prior);
        self.redo_stack.clear();
    }

    /// True when there is a recorded state to undo (for enabling UI).
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// True when there is an undone state to redo.
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Undo the last model mutation: restore the previous model, rebuild the
    /// engine and snapshot from it, and persist it (so undo-then-quit does not
    /// resurrect the undone state). The current model moves to the redo stack.
    ///
    /// An empty history is a no-op, not an error. A failed restore (a store
    /// write failure) leaves everything as it was and keeps the undo point, so
    /// the step is not lost; the error is returned for the caller to surface.
    pub fn undo(&mut self) -> Result<(), String> {
        let Some(prior) = self.undo_stack.pop() else {
            return Ok(());
        };
        let current = self.model.clone();
        match self.swap_model(prior) {
            Ok(_) => {
                push_bounded(&mut self.redo_stack, current);
                self.after_history_restore();
                Ok(())
            }
            Err(e) => {
                // Nothing was swapped, so the undo point still applies.
                self.undo_stack.push(current);
                Err(e)
            }
        }
    }

    /// Redo the last undone mutation (the mirror of [`Self::undo`]).
    pub fn redo(&mut self) -> Result<(), String> {
        let Some(next) = self.redo_stack.pop() else {
            return Ok(());
        };
        let current = self.model.clone();
        match self.swap_model(next) {
            Ok(_) => {
                push_bounded(&mut self.undo_stack, current);
                self.after_history_restore();
                Ok(())
            }
            Err(e) => {
                self.redo_stack.push(current);
                Err(e)
            }
        }
    }

    /// [`Self::undo`], with the outcome reported in the status line (the UI
    /// path). A store-write failure during undo is surfaced, never swallowed;
    /// an empty history says so rather than looking like a broken key.
    fn undo_with_status(&mut self) {
        let had = self.can_undo();
        self.status = match self.undo() {
            Ok(()) if had => "undo".into(),
            Ok(()) => "nothing to undo".into(),
            Err(e) => format!("undo failed: {e}"),
        };
    }

    /// [`Self::redo`], with the outcome reported in the status line.
    fn redo_with_status(&mut self) {
        let had = self.can_redo();
        self.status = match self.redo() {
            Ok(()) if had => "redo".into(),
            Ok(()) => "nothing to redo".into(),
            Err(e) => format!("redo failed: {e}"),
        };
    }

    /// Re-point the transient UI state at the restored model: a measure the
    /// restored model no longer has (an undone CSV import, an undone new
    /// derived measure) cannot stay on a matrix or half-edited. Applied to
    /// EVERY matrix on the canvas, not just the focused one — an undo that
    /// removes a measure must not leave another matrix pointing at it.
    fn after_history_restore(&mut self) {
        let fallback = pick_default_measure(&self.model);
        for m in &mut self.matrices {
            m.editing = None;
            if !m
                .measure
                .is_some_and(|id| self.model.measures.contains_key(&id))
            {
                m.measure = fallback;
            }
        }
        // Force the formula bar to reload from the restored model.
        self.formula_for = None;
        self.formula_error_pos = None;
        self.formula_error_msg.clear();
        self.sync_axis_state();
    }

    /// Create a new derived measure named `name` with RHS `text`. Categories
    /// are inferred as the union of the referenced measures' categories (same
    /// rule as the CLI's `add-derived`). Rebuilds the engine and autosaves. On
    /// any failure (parse error, duplicate name, engine build failure, failed
    /// store write) the model, engine, and snapshot are unchanged.
    pub fn add_derived_measure(&mut self, name: &str, text: &str) -> Result<MeasureId, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("measure name is required".into());
        }
        if self.model.measure_by_name(name).is_some() {
            return Err(format!("a measure named {name:?} already exists"));
        }
        let formula = self.parse_formula_text(text).map_err(|e| e.to_string())?;

        // Infer categories: union of referenced measures' categories.
        let mut cats: Vec<CategoryId> = Vec::new();
        for m in formula.referenced_measures() {
            if let Some(measure) = self.model.measures.get(&m) {
                for c in &measure.categories {
                    if !cats.contains(c) {
                        cats.push(*c);
                    }
                }
            }
        }
        cats.sort_by_key(|c| c.0);

        let id = MeasureId(self.next_measure_id());
        let mut candidate = self.model.clone();
        candidate.add_measure(Measure {
            id,
            name: Name(name.to_string()),
            value_type: ValueType::Number,
            categories: cats,
            kind: MeasureKind::Derived(formula),
            description: None,
        });
        self.publish(candidate)?;
        Ok(id)
    }

    /// The smallest unused measure id (>= 1).
    fn next_measure_id(&self) -> u32 {
        self.model
            .measures
            .keys()
            .map(|m| m.0)
            .max()
            .map(|m| m + 1)
            .unwrap_or(1)
    }

    /// Autosave the model to the store when `db` is set. In-memory (`""`)
    /// models skip saving. Returns the failure message so callers can report
    /// it — never overwrite a save failure with a success message.
    #[must_use = "a failed save must be surfaced, not reported as success"]
    fn save(&mut self) -> Result<(), String> {
        if self.db.is_empty() {
            return Ok(());
        }
        ModelStore::open(&self.db)
            .and_then(|mut s| s.save_model(&self.model).map(|_| ()))
            .map_err(|e| format!("save failed: {e}"))
    }

    /// Read-only inspector facts for `measure` (see `inspector` panel).
    fn inspector_data(&self, measure: MeasureId) -> Option<InspectorData> {
        let m = self.model.measures.get(&measure)?;
        let dimensions = m
            .categories
            .iter()
            .map(|c| {
                self.model
                    .categories
                    .get(c)
                    .map(|cat| cat.name.0.clone())
                    .unwrap_or_else(|| format!("category {}", c.0))
            })
            .collect();
        let (dependencies, formula_english) = match &m.kind {
            MeasureKind::Derived(f) => {
                let mut deps: Vec<String> = f
                    .referenced_measures()
                    .into_iter()
                    .map(|id| {
                        self.model
                            .measures
                            .get(&id)
                            .map(|dm| dm.name.0.clone())
                            .unwrap_or_else(|| format!("measure {}", id.0))
                    })
                    .collect();
                deps.dedup();
                (
                    deps,
                    Some(describe_formula(&NlContext::new(&self.model), f)),
                )
            }
            MeasureKind::Input => (Vec::new(), None),
        };
        let error_cells = self
            .snapshot
            .get(&measure)
            .map(|m| {
                m.values()
                    .filter(|v| matches!(v, CellValue::Err(_)))
                    .count()
            })
            .unwrap_or(0);
        Some(InspectorData {
            id: measure,
            name: m.name.0.clone(),
            is_derived: m.is_derived(),
            value_type: m.value_type,
            dimensions,
            dependencies,
            formula_english,
            error_cells,
        })
    }
}

/// Push `state` onto a bounded history stack, evicting the oldest entry once
/// [`UNDO_DEPTH`] is reached.
fn push_bounded(stack: &mut Vec<Model>, state: Model) {
    if stack.len() >= UNDO_DEPTH {
        stack.remove(0);
    }
    stack.push(state);
}

/// Whether the grid's single-key bindings (cursor motion, paging, measure
/// cycling, undo/redo) should act this frame.
///
/// * `editing` — a GRID CELL editor is open: the cell's own text field owns the
///   keyboard (its Enter/Esc are handled where it is rendered).
/// * `other_focus` — some egui widget owns keyboard focus. While a cell is
///   being edited that widget IS the cell editor, so the two flags overlap;
///   either one suppresses the bindings. Any *other* focused widget (the
///   formula bar, the new-measure name/formula fields, the CSV wizard's text
///   fields, the view-name box) must get the keystroke instead of the grid —
///   otherwise typing `n` cycles measures and `h`/`j`/`k`/`l` moves the cursor
///   mid-word.
///
/// A pure predicate so the decision is unit-testable without a window; the
/// focus flag itself comes from `egui::Memory::focused()`, which is `None`
/// again as soon as focus is released (so the gate is never sticky).
fn grid_keys_enabled(editing: bool, other_focus: bool) -> bool {
    !editing && !other_focus
}

/// Read-only facts about a measure, assembled for the inspector panel.
#[derive(Debug, PartialEq)]
struct InspectorData {
    id: MeasureId,
    name: String,
    is_derived: bool,
    value_type: ValueType,
    dimensions: Vec<String>,
    dependencies: Vec<String>,
    formula_english: Option<String>,
    error_cells: usize,
}

/// One row of the formula pane: a single derived measure's formula as the
/// reference's numbered, checkable list shows it (`✓ 1. Gross Margin = Sum of
/// Revenue - Sum of Cost of Sales`).
///
/// Built by [`ImprovApp::formula_rows`] in measure-id order; the display number
/// is the row's 1-based position, not an identity, so inserting a formula
/// renumbers the list exactly as the reference does.
#[derive(Debug, Clone, PartialEq)]
pub struct FormulaRow {
    /// The measure this formula defines (clicking the row selects it).
    pub measure: MeasureId,
    /// The target measure's name — the left side of the displayed `Target = …`.
    pub target: String,
    /// The formula's source text: the editable spelling when one exists, else
    /// the controlled-English description (`error` then says so).
    pub text: String,
    /// Whether the formula is enabled (the reference's `✓`). See
    /// [`DISABLED_MARKER`].
    pub enabled: bool,
    /// Why this row is not sound, shown inline beneath it. `None` when the
    /// formula compiles and has an editable spelling.
    pub error: Option<String>,
}

/// Build a live engine over all derived measures in `model`, plus its initial
/// snapshot. Falls back to no engine (inputs still render) on build failure.
fn build_engine(model: &Model) -> (Option<Engine>, HashMap<MeasureId, MeasureValues>) {
    match try_build_engine(model) {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("improv-gui: engine build failed: {e}");
            (None, HashMap::new())
        }
    }
}

/// The reserved leading marker in [`Measure::description`] that flags a derived
/// measure's formula as **disabled** — the reference's cleared `✓` (see
/// `docs/reviews/2026-09-22-gui-reconstruction-plan.md` Step 2).
///
/// Disabling is a *model-level* fact, not GUI state: it must survive save and
/// reload, and the engine must honor it. Two properties chose this
/// representation over the alternative (flipping the measure to `Input` and
/// stashing its formula somewhere):
///
/// * **The formula is never moved or re-encoded.** A disabled measure stays
///   `MeasureKind::Derived(f)`, so its formula persists through the store's
///   existing `:measure/formula` attribute, and re-enabling is exact. Stashing
///   it as *text* instead would have to survive [`formula_dsl`] returning
///   `None` for shapes the v1 grammar cannot spell — those formulas would be
///   destroyed by a disable/enable cycle.
/// * **One bit, on the measure it describes.** The marker rides
///   `:measure/description`, which the store already reads and writes, so it
///   round-trips with no schema change; and because it lives *on the measure*,
///   deleting or renaming that measure carries the flag with it (a side table
///   keyed by `MeasureId` would need garbage collection).
///
/// A user-written description is preserved verbatim after the marker; see
/// [`split_marker`] / [`join_marker`].
///
/// The engine honors it via [`engine_model`], which flattens disabled measures
/// to `Input` in the model handed to `Engine::new`, so `derived_build_order`
/// stops traversing them and every dependent recomputes *without* them — which
/// is exactly what clearing the checkmark means.
///
/// ponytail: a reserved prefix in `description` is a sidecar in a user-visible
/// field (the CLI prints descriptions), and it is what a GUI-only change can
/// persist. Promote it to a real `Measure.enabled: bool` plus a
/// `:measure/enabled` store attribute (serde/schema default true, so existing
/// databases keep loading) as soon as `core_model`/`storage_mentat` can be
/// touched in the same change; [`split_marker`], [`join_marker`] and
/// [`formula_disabled`] are the only three places that would move.
const DISABLED_MARKER: &str = "[improv:formula-disabled]";

/// Separator between the marker and the user's own description text.
///
/// A single space, not a newline: `storage_mentat`'s EDN writer emits `\n` as a
/// two-character escape that Mentat's reader does not interpret, so a newline
/// does not survive a save/reload at all (it comes back as a bare `n`, silently
/// corrupting the text). A space round-trips exactly. The marker is
/// bracket-delimited so the boundary is unambiguous without needing a character
/// the store cannot carry.
const MARKER_SEP: char = ' ';

/// Split a measure description into `(formula disabled, the user's own text)`.
///
/// The marker counts only when the description IS it, or begins with it
/// followed by [`MARKER_SEP`] — so a user description that merely starts with
/// the same characters (`"[improv:formula-disabled]ish"`) is left strictly
/// alone.
fn split_marker(description: Option<&str>) -> (bool, Option<&str>) {
    let Some(d) = description else {
        return (false, None);
    };
    match d.strip_prefix(DISABLED_MARKER) {
        Some("") => (true, None),
        Some(rest) if rest.starts_with(MARKER_SEP) => (true, Some(&rest[MARKER_SEP.len_utf8()..])),
        _ => (false, Some(d)),
    }
}

/// Rebuild a description from the two parts [`split_marker`] takes apart — its
/// inverse for every input `split_marker` can produce.
fn join_marker(disabled: bool, user: Option<&str>) -> Option<String> {
    match (disabled, user) {
        (false, user) => user.map(str::to_string),
        (true, None) => Some(DISABLED_MARKER.to_string()),
        (true, Some(user)) => Some(format!("{DISABLED_MARKER}{MARKER_SEP}{user}")),
    }
}

/// Whether `m`'s formula is disabled (see [`DISABLED_MARKER`]). Always false
/// for an input measure, which has no formula to disable.
fn formula_disabled(m: &Measure) -> bool {
    m.is_derived() && split_marker(m.description.as_deref()).0
}

/// `model` as the ENGINE must see it: every measure whose formula is disabled
/// flattened to `MeasureKind::Input`, keeping its (still-stored) formula out of
/// the dataflow entirely.
///
/// This is the single point that makes disabling real rather than cosmetic. A
/// flattened measure leaves `derived_build_order`, so it computes nothing (its
/// cells go blank) *and* every dependent recomputes against its absence instead
/// of through it.
///
/// Borrows when nothing is disabled — the overwhelmingly common case — so the
/// projection costs nothing on models that never touch the feature.
fn engine_model(model: &Model) -> Cow<'_, Model> {
    if !model.measures.values().any(formula_disabled) {
        return Cow::Borrowed(model);
    }
    let mut out = model.clone();
    for m in out.measures.values_mut() {
        if formula_disabled(m) {
            m.kind = MeasureKind::Input;
        }
    }
    Cow::Owned(out)
}

/// Build a live engine over every *enabled* derived measure in `model`,
/// propagating a build failure (cycle, type/dimension error) instead of
/// swallowing it. A measure whose formula is disabled ([`DISABLED_MARKER`]) is
/// excluded, via [`engine_model`].
///
/// `Ok(None)` (no engine, successfully) when nothing is left to compute — a
/// model with no derived measures, or one whose formulas are all disabled.
fn try_build_engine(
    model: &Model,
) -> Result<(Option<Engine>, HashMap<MeasureId, MeasureValues>), String> {
    let model = engine_model(model);
    let derived: Vec<MeasureId> = model
        .measures
        .values()
        .filter(|m| m.is_derived())
        .map(|m| m.id)
        .collect();
    if derived.is_empty() {
        return Ok((None, HashMap::new()));
    }
    let (e, snap) = Engine::new(&model, &derived).map_err(|e| e.to_string())?;
    Ok((Some(e), snap))
}

/// Render `formula` as symbolic-DSL source text that `parser::parse_expr` parses
/// back to the *identical* AST, or `None` when the v1 grammar has no exact
/// spelling for the shape (a bare ref carrying `over`/`except`, a
/// multi-category `OVER`, an empty measure/category name, an
/// enum/error/negative/exponent literal, an unknown function id). Callers fall
/// back to the controlled-English description for those, which
/// `commit_formula` also accepts.
///
/// Awkward *names* are no longer a refusal reason: [`ident`] quotes them
/// (`'Unit Price'`, `'Price/Unit'`, `'Over'`) per plan Step 4b.
fn formula_dsl(model: &Model, formula: &improv_core_model::Formula) -> Option<String> {
    expr_dsl(model, &formula.expr)
}

/// Binding tightness, mirroring `parser`'s grammar levels (higher binds
/// tighter). Used to emit exactly the parentheses needed to round-trip.
fn prec(e: &Expr) -> u8 {
    match e {
        Expr::BinaryOp(op, _, _) => match op {
            BinaryOp::Or => 1,
            BinaryOp::And => 2,
            BinaryOp::Eq
            | BinaryOp::Ne
            | BinaryOp::Lt
            | BinaryOp::Le
            | BinaryOp::Gt
            | BinaryOp::Ge => 3,
            BinaryOp::Add | BinaryOp::Sub => 4,
            BinaryOp::Mul | BinaryOp::Div => 5,
        },
        Expr::UnaryOp(_, _) => 6,
        _ => 7,
    }
}

/// `expr_dsl`, parenthesized when `e` binds more loosely than its context.
fn child_dsl(model: &Model, e: &Expr, min_prec: u8) -> Option<String> {
    let s = expr_dsl(model, e)?;
    Some(if prec(e) < min_prec {
        format!("({s})")
    } else {
        s
    })
}

fn expr_dsl(model: &Model, e: &Expr) -> Option<String> {
    match e {
        Expr::Literal(v) => literal_dsl(v),
        Expr::Ref(id, spec) => {
            // `over`/`except` have no bare-ref spelling: `OVER` exists only
            // inside an aggregation (handled below) and `except` not at all.
            if !spec.over.is_empty() || !spec.except.is_empty() {
                return None;
            }
            ref_dsl(model, *id, &spec.by)
        }
        Expr::UnaryOp(op, inner) => {
            // Both unary operators take a Primary, so anything looser than a
            // unary chain needs parentheses.
            let s = child_dsl(model, inner, 6)?;
            Some(match op {
                UnaryOp::Neg => format!("-{s}"),
                UnaryOp::Not => format!("NOT {s}"),
            })
        }
        Expr::BinaryOp(op, l, r) => {
            let p = prec(e);
            let sym = match op {
                BinaryOp::Add => "+",
                BinaryOp::Sub => "-",
                BinaryOp::Mul => "*",
                BinaryOp::Div => "/",
                BinaryOp::And => "AND",
                BinaryOp::Or => "OR",
                BinaryOp::Eq => "==",
                BinaryOp::Ne => "<>",
                BinaryOp::Lt => "<",
                BinaryOp::Le => "<=",
                BinaryOp::Gt => ">",
                BinaryOp::Ge => ">=",
            };
            // Left-associative levels keep a same-level LEFT child bare and
            // parenthesize a same-level RIGHT child; comparisons are
            // non-associative, so a same-level child needs parens on either
            // side (`a < b < c` does not parse).
            let (lmin, rmin) = if p == 3 { (4, 4) } else { (p, p + 1) };
            Some(format!(
                "{} {sym} {}",
                child_dsl(model, l, lmin)?,
                child_dsl(model, r, rmin)?
            ))
        }
        Expr::Call(func, args) => call_dsl(model, *func, args),
    }
}

fn call_dsl(model: &Model, func: FuncId, args: &[Expr]) -> Option<String> {
    let name = func_name(func)?;
    // Aggregation: `FUNC(MeasureRef OVER Category)` is the ONLY spelling the v1
    // grammar has for an aggregating func id, and `parse_aggregation` demands
    // it (a ref arg, exactly one OVER category, no `except`). Anything else
    // must NOT fall through to the generic call printer: `SUM(Quantity)` and
    // `SUM(Price * Quantity)` are rejected by `parse_expr`. Return `None` so
    // the caller falls back to the controlled-English description.
    if is_aggregation(func) {
        let [Expr::Ref(id, spec)] = args else {
            return None;
        };
        if spec.over.len() != 1 || !spec.except.is_empty() {
            return None;
        }
        return Some(format!(
            "{name}({} OVER {})",
            ref_dsl(model, *id, &spec.by)?,
            ident(category_name(model, spec.over[0])?)?
        ));
    }
    let rendered: Option<Vec<String>> = args.iter().map(|a| expr_dsl(model, a)).collect();
    Some(format!("{name}({})", rendered?.join(", ")))
}

/// Whether `func` is one of the v1 aggregating built-ins (SUM/AVG/MIN/MAX),
/// whose only DSL spelling is the `OVER` form. Ids come from `parser`'s public
/// constants, the same table `engine::compiler::is_aggregation` mirrors.
fn is_aggregation(func: FuncId) -> bool {
    matches!(
        func,
        parser::FUNC_SUM | parser::FUNC_AVG | parser::FUNC_MIN | parser::FUNC_MAX
    )
}

/// The DSL name of a built-in function id. Scalar names are resolved *through*
/// `parser::scalar_func` so the id table stays single-sourced there.
fn func_name(func: FuncId) -> Option<&'static str> {
    const SCALARS: &[&str] = &[
        "ABS", "ROUND", "FLOOR", "CEIL", "SQRT", "NEG", "MIN2", "MAX2",
    ];
    match func {
        parser::FUNC_SUM => Some("SUM"),
        parser::FUNC_AVG => Some("AVG"),
        parser::FUNC_MIN => Some("MIN"),
        parser::FUNC_MAX => Some("MAX"),
        _ => SCALARS
            .iter()
            .copied()
            .find(|n| parser::scalar_func(n).map(|(id, _)| id) == Some(func)),
    }
}

fn ref_dsl(model: &Model, id: MeasureId, by: &[CategoryId]) -> Option<String> {
    let name = ident(model.measures.get(&id).map(|m| m.name.0.as_str())?)?;
    if by.is_empty() {
        return Some(name);
    }
    let cats: Option<Vec<String>> = by
        .iter()
        .map(|c| category_name(model, *c).and_then(ident))
        .collect();
    Some(format!("{name}[{}]", cats?.join(", ")))
}

fn category_name(model: &Model, c: CategoryId) -> Option<&str> {
    model.categories.get(&c).map(|cat| cat.name.0.as_str())
}

/// The DSL spelling of `name` as an identifier: the **bare** form when the
/// tokenizer reads it back as a single non-keyword identifier, else the
/// **quoted** form (`'Unit Price'`) the grammar gained in plan Step 4a.
///
/// Bare is preferred whenever it works, so ordinary formulas keep printing as
/// `Price * Quantity` rather than `'Price' * 'Quantity'`.
///
/// `None` only for the **empty** name: `''` is an explicit parse error ("empty
/// quoted name: '' names nothing"), so an unnamed measure/category is the one
/// name this grammar genuinely cannot spell. Everything else — spaces,
/// slashes, hyphens, parentheses, digits-first, non-ASCII, and the keywords
/// (`Over`, `AND`) — is spellable quoted, which is what makes a CSV-derived
/// measure like `Unit Price` editable instead of read-only.
///
/// A literal `'` inside the name is escaped by **doubling** it (`Bob''s Rate`),
/// matching `parser`'s tokenizer exactly. There is no other escape to get
/// wrong: the quoted form takes every other character verbatim.
fn ident(name: &str) -> Option<String> {
    if is_bare_ident(name) {
        return Some(name.to_string());
    }
    // `''` names nothing (a parse error), so an empty name stays unspellable.
    if name.is_empty() {
        return None;
    }
    Some(format!("'{}'", name.replace('\'', "''")))
}

/// Whether `name` survives a round trip through the tokenizer as one *bare*
/// identifier that the grammar will not mistake for a keyword.
fn is_bare_ident(name: &str) -> bool {
    let mut chars = name.chars();
    let head_ok = chars.next().is_some_and(|c| c.is_alphabetic() || c == '_');
    if !head_ok || !chars.all(|c| c.is_alphanumeric() || c == '_') {
        return false;
    }
    // A bare keyword is consumed as grammar, never as a name. (`SUM`/`AVG` and
    // the scalar-function names are NOT keywords here: the parser only treats
    // them as functions when directly followed by `(`, which a printed
    // measure reference never is.)
    const KEYWORDS: &[&str] = &["NOT", "AND", "OR", "OVER", "TRUE", "FALSE"];
    !KEYWORDS.iter().any(|k| name.eq_ignore_ascii_case(k))
}

fn literal_dsl(v: &Value) -> Option<String> {
    match v {
        // The number tokenizer reads digits and '.' only: no sign, no exponent.
        Value::Number(n) => {
            let s = format!("{n}");
            (n.is_finite() && *n >= 0.0 && s.chars().all(|c| c.is_ascii_digit() || c == '.'))
                .then_some(s)
        }
        Value::Boolean(b) => Some(if *b { "TRUE".into() } else { "FALSE".into() }),
        // The v1 string literal has no escapes, so only text that survives
        // verbatim between quotes is expressible.
        Value::Text(t) => (!t.contains('"') && !t.contains('\\') && !t.contains(char::is_control))
            .then(|| format!("\"{t}\"")),
        Value::DateTime(dt) => Some(format!("#{}#", dt.to_rfc3339())),
        Value::Enum(_) | Value::Error(_) => None,
    }
}

/// Parse cell text as `declared`, the measure's declared type. The declared
/// type WINS over the text's shape: `"42"` for a Text measure is text, not a
/// number. Unparsable text is an error (never a coerced value), so a bad edit
/// leaves the prior typed cell untouched.
///
/// Dates and booleans reuse the formula grammar's own literal forms (via
/// `parser::parse_expr` on `#...#` / `TRUE`|`FALSE`), so a date a formula
/// accepts is a date a cell accepts. Numbers use `f64::from_str` instead: the
/// formula tokenizer reads digits and `.` only, and a cell must accept `-3.5`
/// and `1e9`.
fn parse_typed(model: &Model, declared: ValueType, text: &str) -> Result<Value, String> {
    let literal = |src: String| match parser::parse_expr(model, &src) {
        Ok(f) => match f.expr {
            Expr::Literal(v) if v.type_of() == Some(declared) => Some(v),
            _ => None,
        },
        Err(_) => None,
    };
    match declared {
        ValueType::Number => text
            .parse::<f64>()
            .map(Value::Number)
            .map_err(|_| format!("not a number: {text:?}")),
        // Text takes the buffer verbatim (trimmed by the caller) — no parsing,
        // so nothing a user can type is rejected.
        ValueType::Text => Ok(Value::Text(text.to_string())),
        ValueType::Boolean => literal(text.to_ascii_uppercase())
            .ok_or_else(|| format!("not a boolean (use true/false): {text:?}")),
        ValueType::DateTime => literal(format!("#{text}#"))
            .ok_or_else(|| format!("not a date (use YYYY-MM-DD or RFC3339): {text:?}")),
        ValueType::Enum => text
            .parse::<u32>()
            .map(Value::Enum)
            .map_err(|_| format!("not an enum index: {text:?}")),
    }
}

fn decode(k: &CoordKey) -> improv_core_model::Coordinate {
    improv_core_model::Coordinate::from_pairs(k.iter().map(|(c, i)| (CategoryId(*c), ItemId(*i))))
}

fn pick_default_measure(model: &Model) -> Option<MeasureId> {
    let mut ids: Vec<MeasureId> = model.measures.keys().copied().collect();
    ids.sort_by_key(|m| m.0);
    ids.iter()
        .find(|m| {
            model
                .measures
                .get(m)
                .map(|x| x.is_derived())
                .unwrap_or(false)
        })
        .copied()
        .or_else(|| ids.first().copied())
}

/// The measure's categories in natural (declared) order (empty if none/absent).
fn natural_axis_order(model: &Model, measure: Option<MeasureId>) -> Vec<CategoryId> {
    measure
        .and_then(|m| model.measures.get(&m))
        .map(|m| m.categories.clone())
        .unwrap_or_default()
}

impl eframe::App for ImprovApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_axis_state();
        self.handle_history_keys(ctx);
        // Panel order IS layout order: a top/bottom panel claims its strip from
        // the outside in. The document tabs go above everything (the reference's
        // browser-tab strip), the status bar below everything, and the canvas
        // takes what is left.
        self.document_tab_bar(ctx);
        self.formula_bar(ctx);
        self.status_bar(ctx);
        self.formula_panel(ctx);
        self.formula_list_panel(ctx);
        self.tool_palette(ctx);
        self.explorer_panel(ctx);
        self.inspector_panel(ctx);
        self.chart_panel(ctx);
        self.csv_wizard_panel(ctx);
        self.canvas_panel(ctx);
    }
}

impl ImprovApp {
    /// App-wide undo/redo bindings: Ctrl/Cmd+Z undoes, Ctrl/Cmd+Shift+Z and
    /// Ctrl/Cmd+Y redo. Gated by the same [`grid_keys_enabled`] predicate the
    /// grid uses, so the chords never fire while a cell editor or any other
    /// text field owns the keyboard. Handled here (not in `handle_grid_keys`)
    /// because the grid isn't rendered when no measure is selected — which is
    /// exactly the state an undone import can leave behind.
    fn handle_history_keys(&mut self, ctx: &egui::Context) {
        let other_focus = ctx.memory(|m| m.focused()).is_some();
        if !grid_keys_enabled(self.focused().editing.is_some(), other_focus) {
            return;
        }
        let (command, shift, z, y) = ctx.input(|i| {
            (
                i.modifiers.command,
                i.modifiers.shift,
                i.key_pressed(egui::Key::Z),
                i.key_pressed(egui::Key::Y),
            )
        });
        if !command {
            return;
        }
        if z {
            if shift {
                self.redo_with_status();
            } else {
                self.undo_with_status();
            }
        } else if y {
            self.redo_with_status();
        }
    }

    /// A NeXTSTEP-style **tool palette**: a narrow left column of beveled
    /// buttons for the common operations (pivot, chart, save model, save view).
    /// Always visible, like the tear-off palettes in NeXTSTEP apps.
    fn tool_palette(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("tools")
            .resizable(false)
            .exact_width(44.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.vertical_centered(|ui| {
                    let btn = |ui: &mut egui::Ui, glyph: &str, tip: &str| {
                        ui.add_sized([32.0, 28.0], egui::Button::new(glyph))
                            .on_hover_text(tip)
                            .clicked()
                    };
                    if btn(ui, "↻", "Pivot (rotate axes)") {
                        self.pivot_rotate();
                    }
                    if btn(ui, "↶", "Undo (Ctrl+Z)") {
                        self.undo_with_status();
                    }
                    if btn(ui, "↷", "Redo (Ctrl+Shift+Z)") {
                        self.redo_with_status();
                    }
                    if btn(ui, "☉", "Toggle chart") {
                        self.show_chart = !self.show_chart;
                    }
                    if btn(ui, "⇄", "Import/export CSV or TSV") {
                        self.toggle_csv_wizard();
                    }
                    ui.add_space(6.0);
                    if btn(ui, "▤", "Save view") {
                        // Save under the current name box, or a generated name.
                        let name = if self.view_name.trim().is_empty() {
                            format!("view {}", self.model.views.len() + 1)
                        } else {
                            self.view_name.clone()
                        };
                        if self.save_view(&name).is_some() {
                            self.view_name.clear();
                            self.status = format!("saved view '{name}'");
                        }
                    }
                    if btn(ui, "⬇", "Save model to store") {
                        self.status = match self.save() {
                            Ok(()) => "saved model".into(),
                            Err(e) => e,
                        };
                    }
                });
            });
    }

    /// Left: model explorer grouped into Categories (with their items) and
    /// Measures (input vs derived). Clicking a measure selects it.
    fn explorer_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("explorer")
            .resizable(true)
            .default_width(220.0)
            .show(ctx, |ui| {
                ui.heading("Model");
                ui.separator();

                egui::CollapsingHeader::new("Categories")
                    .default_open(true)
                    .show(ui, |ui| {
                        let mut cats: Vec<CategoryId> =
                            self.model.categories.keys().copied().collect();
                        cats.sort_by_key(|c| c.0);
                        for cid in cats {
                            let cat = &self.model.categories[&cid];
                            egui::CollapsingHeader::new(&cat.name.0)
                                .id_salt(("cat", cid.0))
                                .show(ui, |ui| {
                                    let mut items = cat.items.clone();
                                    items.sort_by_key(|i| i.0);
                                    for iid in items {
                                        if let Some(it) = self.model.items.get(&iid) {
                                            ui.label(&it.name.0);
                                        }
                                    }
                                });
                        }
                    });

                egui::CollapsingHeader::new("Measures")
                    .default_open(true)
                    .show(ui, |ui| {
                        let mut ids: Vec<MeasureId> = self.model.measures.keys().copied().collect();
                        ids.sort_by_key(|m| m.0);
                        for id in ids {
                            let m = &self.model.measures[&id];
                            let tag = if m.is_derived() { "= " } else { "· " };
                            let label = format!("{tag}{}", m.name.0);
                            if ui
                                .selectable_label(self.selected() == Some(id), label)
                                .clicked()
                            {
                                self.focused_mut().select(Some(id));
                            }
                        }
                    });

                self.views_section(ui);
            });
    }

    /// Views section in the explorer: a name field + "Save view" button that
    /// captures the current CANVAS, and a list of saved views (click to load).
    ///
    /// This is the NeXTSTEP reference's model navigator
    /// (`docs/reviews/refs/improv-next-nested-headers.jpg`: a `Model of
    /// Divisional Report` window listing `Worksheet 1` / `Worksheet 2`), and the
    /// same set the Quantrix reference puts in the document-tab strip
    /// ([`Self::document_tab_bar`]). Both are kept because the references have
    /// both: a navigator that lists everything, and tabs for the quick switch.
    /// The current view is highlighted in both.
    fn views_section(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Views")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.text_edit_singleline(&mut self.view_name);
                    if ui.button("Save view").clicked() {
                        let name = self.view_name.clone();
                        if self.save_view(&name).is_some() {
                            self.view_name.clear();
                        }
                    }
                });
                let mut ids: Vec<ViewId> = self.model.views.keys().copied().collect();
                if ids.is_empty() {
                    ui.weak("(no saved views)");
                }
                ids.sort_by_key(|v| v.0);
                let mut load: Option<View> = None;
                for id in ids {
                    let v = &self.model.views[&id];
                    if ui
                        .selectable_label(self.current_view == Some(id), &v.name.0)
                        .clicked()
                    {
                        load = Some(v.clone());
                    }
                }
                if let Some(v) = load {
                    self.apply_view(&v);
                }
            });
    }

    /// Right: inspector for the selected measure.
    fn inspector_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(260.0)
            .show(ctx, |ui| {
                ui.heading("Inspector");
                ui.separator();
                let data = self.selected().and_then(|m| self.inspector_data(m));
                match data {
                    None => {
                        ui.label("No measure selected.");
                    }
                    Some(d) => {
                        ui.label(format!("id: {}", d.id.0));
                        ui.label(format!("name: {}", d.name));
                        ui.label(format!(
                            "kind: {}",
                            if d.is_derived { "derived" } else { "input" }
                        ));
                        ui.label(format!("value type: {:?}", d.value_type));
                        ui.label(format!(
                            "dimensions: {}",
                            if d.dimensions.is_empty() {
                                "(scalar)".to_string()
                            } else {
                                d.dimensions.join(" x ")
                            }
                        ));
                        if d.is_derived {
                            ui.separator();
                            ui.label(format!("depends on: {}", d.dependencies.join(", ")));
                            if let Some(eng) = &d.formula_english {
                                ui.label(format!("formula: {eng}"));
                            }
                            if d.error_cells > 0 {
                                ui.colored_label(
                                    egui::Color32::from_rgb(200, 60, 60),
                                    format!("{} cell(s) have errors (#ERR)", d.error_cells),
                                );
                            }
                        }
                    }
                }
            });
    }

    /// Bottom: formula editor for the selected derived measure, plus a form to
    /// add a new derived measure.
    /// The Lotus Improv **formula bar**: a compact single-line bar across the
    /// top that shows and edits the *selected measure's* formula. Input
    /// measures show a hint. Committing re-typechecks and rebuilds the engine.
    fn formula_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("formula_bar").show(ctx, |ui| {
            // Reload the buffer when the selection changes, clearing any
            // stale inline error from the previously selected measure.
            if self.formula_for != self.selected() {
                self.formula_for = self.selected();
                self.formula_buf = self
                    .selected()
                    .and_then(|m| self.formula_source(m))
                    .unwrap_or_default();
                self.formula_error_pos = None;
                self.formula_error_msg.clear();
            }
            ui.horizontal(|ui| match self.selected() {
                Some(mid)
                    if self.model.measures.get(&mid).map(|m| m.is_derived()) == Some(true) =>
                {
                    ui.strong(format!("{} =", self.model.measures[&mid].name.0));
                    // A disabled formula is still shown and still editable — it
                    // is part of the model's logic — but the bar says so, so a
                    // blank grid is never a mystery.
                    if !self.formula_enabled(mid) {
                        ui.weak("(disabled)").on_hover_text(
                            "This formula is switched off in the formula list \
                             below, so it computes nothing.",
                        );
                    }
                    // No spelling either surface language accepts (e.g. a
                    // measure named "Unit Price" from a CSV header): show the
                    // formula read-only rather than invite a commit of text
                    // that cannot parse. See `formula_source`.
                    //
                    // ponytail: this reparses the one-line formula per frame.
                    // Cache it next to `formula_buf` (same invalidation as
                    // `formula_for`) if a profile ever shows it.
                    if self.formula_source(mid).is_none() {
                        let english = self
                            .model
                            .measures
                            .get(&mid)
                            .and_then(|m| match &m.kind {
                                MeasureKind::Derived(f) => {
                                    Some(describe_formula(&NlContext::new(&self.model), f))
                                }
                                MeasureKind::Input => None,
                            })
                            .unwrap_or_default();
                        ui.weak(english);
                        ui.weak(
                            "(not editable here: this formula has no editable spelling \u{2014} \
                             rename the measure(s)/category(ies) it uses to simple identifiers \
                             to edit it)",
                        );
                        return;
                    }
                    let error_pos = self.formula_error_pos;
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    let mut layouter = move |ui: &egui::Ui, text: &str, wrap_width: f32| {
                        let mut job = crate::formula_highlight::highlight_formula(
                            text,
                            font.clone(),
                            error_pos,
                        );
                        job.wrap.max_width = wrap_width;
                        ui.fonts(|f| f.layout_job(job))
                    };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.formula_buf)
                            .desired_width(f32::INFINITY)
                            .hint_text("e.g. Price * Quantity")
                            .layouter(&mut layouter),
                    );
                    if resp.changed() {
                        // The user edited the buffer: the previous error no
                        // longer describes what's on screen.
                        self.formula_error_pos = None;
                        self.formula_error_msg.clear();
                    }
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Commit").clicked() || enter {
                        let text = self.formula_buf.clone();
                        match self.commit_formula(mid, &text) {
                            Ok(()) => self.status = "formula updated".into(),
                            Err(e) => self.status = format!("formula error: {e}"),
                        }
                    }
                }
                Some(mid) => {
                    ui.strong(format!("{} ", self.model.measures[&mid].name.0));
                    ui.weak("(input measure — edit cells in the grid)");
                }
                None => {
                    ui.weak("No measure selected.");
                }
            });
            // Inline error label directly under the formula bar (in addition
            // to the general status line), cleared alongside the highlight.
            if !self.formula_error_msg.is_empty() {
                ui.colored_label(
                    crate::formula_highlight::ERROR_COLOR,
                    format!("⚠ {}", self.formula_error_msg),
                );
            }
        });
    }

    /// **The Step 2 formula pane** (plan
    /// `docs/reviews/2026-09-22-gui-reconstruction-plan.md`): the whole model's
    /// logic as one persistent, numbered, individually-checkable list — the
    /// shape every reference shows (`✓ 1. Gross Margin = Sum of Revenue - Sum of
    /// Cost of Sales`), and the thing a single-line bar for the selected measure
    /// alone can never be.
    ///
    /// Reference-faithful in the parts that carry meaning:
    ///
    /// * a `Formulas` label and an operator palette (`= + - * / ^ ( ) [ ]`)
    ///   across the top, each button inserting its token into the editor above;
    /// * one row per derived measure: a checkbox (the `✓`), a **bold 1-based
    ///   number**, then `Target = formula`;
    /// * the selected row highlighted, and clicking any row selecting that
    ///   measure so the grid follows;
    /// * inline error text under a row whose formula does not compile.
    ///
    /// The editor for the selected row stays the top [`Self::formula_bar`], as
    /// the reference also has both (a `Formula:` edit line *and* the list).
    fn formula_list_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("formula_list")
            .resizable(true)
            .default_height(140.0)
            .show(ctx, |ui| {
                self.operator_palette(ui);
                ui.separator();
                let rows = self.formula_rows();
                if rows.is_empty() {
                    self.formula_row_rects.clear();
                    ui.weak("No formulas yet — define a derived measure below.");
                    return;
                }
                // What the user clicked, applied after the loop so the model is
                // not mutated while `rows` borrows nothing from it but the UI
                // still reads `self.selected` per row.
                let mut select: Option<MeasureId> = None;
                let mut toggle: Option<(MeasureId, bool)> = None;
                let mut rects: Vec<(MeasureId, egui::Rect)> = Vec::new();
                egui::ScrollArea::vertical()
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        for (i, row) in rows.iter().enumerate() {
                            let selected = self.selected() == Some(row.measure);
                            ui.horizontal(|ui| {
                                let mut on = row.enabled;
                                if ui
                                    .add(egui::Checkbox::without_text(&mut on))
                                    .on_hover_text(
                                        "Enabled. Uncheck to stop this formula \
                                         computing (dependents recompute without it).",
                                    )
                                    .changed()
                                {
                                    toggle = Some((row.measure, on));
                                }
                                // `1.` `2.` … bold, as the reference numbers them.
                                ui.strong(format!("{}.", i + 1));
                                // A disabled formula is dimmed, not hidden: it is
                                // still part of the model's logic.
                                let label = format!("{} = {}", row.target, row.text);
                                let text = if row.enabled {
                                    egui::RichText::new(label)
                                } else {
                                    egui::RichText::new(label).weak().strikethrough()
                                };
                                let resp = ui.selectable_label(selected, text);
                                rects.push((row.measure, resp.rect));
                                if resp.clicked() {
                                    select = Some(row.measure);
                                }
                            });
                            if let Some(err) = &row.error {
                                ui.horizontal(|ui| {
                                    // Indent under the row's text, past the
                                    // checkbox and number.
                                    ui.add_space(44.0);
                                    ui.colored_label(
                                        crate::formula_highlight::ERROR_COLOR,
                                        format!("⚠ {err}"),
                                    );
                                });
                            }
                        }
                    });
                self.formula_row_rects = rects;
                if let Some(id) = select {
                    self.focused_mut().select(Some(id));
                }
                if let Some((id, on)) = toggle {
                    match self.set_formula_enabled(id, on) {
                        Ok(()) => {
                            self.status = format!(
                                "formula {} {}",
                                self.model
                                    .measures
                                    .get(&id)
                                    .map(|m| m.name.0.clone())
                                    .unwrap_or_default(),
                                if on { "enabled" } else { "disabled" }
                            );
                        }
                        Err(e) => self.status = format!("toggle failed: {e}"),
                    }
                }
            });
    }

    /// The reference's operator palette above the formula list: each button
    /// appends its token to the formula editor's buffer, so the mouse can build
    /// an expression without the keyboard.
    ///
    /// Inert (greyed) unless the selected measure's formula is actually editable
    /// — appending to a buffer the bar is showing read-only would be a lie.
    ///
    /// Skipped from the reference's palette: `‥ : ::` (range/scope operators this
    /// grammar does not have), `In`/`Skip` (Quantrix scope keywords), `[THIS]`,
    /// and the `ƒ` function browser (plan Step 5).
    fn operator_palette(&mut self, ui: &mut egui::Ui) {
        const TOKENS: &[&str] = &["=", "+", "-", "*", "/", "(", ")", "[", "]"];
        let editable = self
            .selected()
            .is_some_and(|m| self.formula_source(m).is_some());
        ui.horizontal(|ui| {
            ui.strong("Formulas");
            ui.separator();
            for tok in TOKENS {
                // `=` is the target separator the bar prints itself, so the
                // palette inserts the operators only.
                if ui
                    .add_enabled(editable, egui::Button::new(*tok).small())
                    .clicked()
                {
                    if !self.formula_buf.is_empty() && !self.formula_buf.ends_with(' ') {
                        self.formula_buf.push(' ');
                    }
                    self.formula_buf.push_str(tok);
                }
            }
        });
    }

    /// Bottom panel: the "new derived measure" definition form. (The selected
    /// measure's formula is edited in the top formula bar; the whole model's
    /// formulas are listed in [`Self::formula_list_panel`]; the status message
    /// and the selection aggregate are in [`Self::status_bar`].)
    fn formula_panel(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("definitions")
            .resizable(true)
            .default_height(80.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.strong("New derived measure:");
                    ui.label("name");
                    ui.text_edit_singleline(&mut self.new_name);
                    ui.label("=");
                    ui.text_edit_singleline(&mut self.new_formula);
                    if ui.button("Add").clicked() {
                        let (name, text) = (self.new_name.clone(), self.new_formula.clone());
                        match self.add_derived_measure(&name, &text) {
                            Ok(id) => {
                                self.status = format!("added derived measure {}", id.0);
                                self.focused_mut().select(Some(id));
                                self.new_name.clear();
                                self.new_formula.clear();
                            }
                            Err(e) => self.status = format!("add failed: {e}"),
                        }
                    }
                });
            });
    }

    /// Bottom-right chart panel (shown only when the "Chart" toggle is on): a
    /// read-only bar chart of the selected measure with a bar/line toggle.
    fn chart_panel(&mut self, ctx: &egui::Context) {
        if !self.show_chart {
            return;
        }
        egui::SidePanel::right("chart")
            .resizable(true)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Chart");
                    ui.checkbox(&mut self.chart_line, "line");
                });
                ui.separator();
                let data = self.chart_series();
                crate::chart::render_chart(ui, &data, self.chart_line);
            });
    }

    /// A toggle-able window: CSV/TSV import (left) + export (right), mirroring
    /// the CLI's `import-csv`/`export-csv` exactly. All fields are plain text
    /// (no file-dialog dependency, same pattern as the "Save view" name box) —
    /// see `csv_wizard::build_import_spec`/`build_export_args` for the pure
    /// validation this panel drives.
    fn csv_wizard_panel(&mut self, ctx: &egui::Context) {
        if !self.show_csv_wizard {
            return;
        }
        let mut open = true;
        egui::Window::new("Import / Export CSV")
            .open(&mut open)
            .default_width(480.0)
            .show(ctx, |ui| {
                ui.columns(2, |cols| {
                    self.csv_import_form(&mut cols[0]);
                    self.csv_export_form(&mut cols[1]);
                });
            });
        if !open {
            self.show_csv_wizard = false;
        }
    }

    /// The import half of the wizard: file path, delimiter/header toggles,
    /// measure id (auto-assigned)/name, value column, and a repeatable list
    /// of dimension-mapping rows with +/- controls.
    fn csv_import_form(&mut self, ui: &mut egui::Ui) {
        ui.heading("Import");
        ui.horizontal(|ui| {
            ui.label("file");
            ui.text_edit_singleline(&mut self.import_form.path);
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.import_form.tsv, "TSV");
            ui.checkbox(&mut self.import_form.has_header, "has header row");
        });
        ui.horizontal(|ui| {
            ui.label("measure id");
            ui.add(
                egui::TextEdit::singleline(&mut self.import_form.measure_id).desired_width(60.0),
            );
            ui.label("name");
            ui.text_edit_singleline(&mut self.import_form.measure_name);
        });
        ui.horizontal(|ui| {
            ui.label("value column");
            ui.text_edit_singleline(&mut self.import_form.value_column);
        });
        ui.separator();
        ui.label("dimensions (column, category id, category name):");
        let mut remove: Option<usize> = None;
        for (i, row) in self.import_form.dimensions.iter_mut().enumerate() {
            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut row.column);
                ui.add(egui::TextEdit::singleline(&mut row.category_id).desired_width(40.0));
                ui.text_edit_singleline(&mut row.category_name);
                if ui.small_button("-").clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            self.import_form.dimensions.remove(i);
        }
        if ui.small_button("+ dimension").clicked() {
            self.import_form
                .dimensions
                .push(csv_wizard::DimRow::default());
        }
        ui.separator();
        if ui.button("Import").clicked() {
            self.run_csv_import();
        }
    }

    /// The export half of the wizard: a measure combo box (from the current
    /// model, like the explorer's measure list), a target path, and a
    /// delimiter toggle.
    fn csv_export_form(&mut self, ui: &mut egui::Ui) {
        ui.heading("Export");
        let mut ids: Vec<MeasureId> = self.model.measures.keys().copied().collect();
        ids.sort_by_key(|m| m.0);
        let selected_label = self
            .export_form
            .measure_id
            .and_then(|m| self.model.measures.get(&m))
            .map(|m| m.name.0.clone())
            .unwrap_or_else(|| "(choose a measure)".to_string());
        egui::ComboBox::from_label("measure")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                for id in &ids {
                    let name = self.model.measures[id].name.0.clone();
                    ui.selectable_value(&mut self.export_form.measure_id, Some(*id), name);
                }
            });
        ui.horizontal(|ui| {
            ui.label("file");
            ui.text_edit_singleline(&mut self.export_form.path);
        });
        ui.checkbox(&mut self.export_form.tsv, "TSV");
        ui.separator();
        if ui.button("Export").clicked() {
            self.run_csv_export();
        }
    }

    /// The reference's **document tabs** (`Welcome / Concepts / P&L Canvas …`):
    /// one tab per saved view across the top, the current one highlighted.
    /// Clicking a tab switches canvases via [`Self::open_view`].
    ///
    /// A model with no saved views shows a hint instead of an empty strip, and
    /// `+ Canvas` saves the canvas on screen as a new view (which is what makes
    /// the second tab reachable at all).
    fn document_tab_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("document_tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let tabs = self.document_tabs();
                let mut open: Option<ViewId> = None;
                if tabs.is_empty() {
                    ui.weak("(no saved views \u{2014} \"+ Canvas\" saves this one)");
                }
                for (id, name, current) in tabs {
                    if ui.selectable_label(current, name).clicked() {
                        open = Some(id);
                    }
                }
                if let Some(id) = open {
                    self.open_view(id);
                }
                ui.separator();
                if ui
                    .button("+ Canvas")
                    .on_hover_text("save the matrices on screen as a new view")
                    .clicked()
                {
                    let name = if self.view_name.trim().is_empty() {
                        format!("Canvas {}", self.model.views.len() + 1)
                    } else {
                        self.view_name.clone()
                    };
                    if self.save_view(&name).is_some() {
                        self.view_name.clear();
                    }
                }
                if ui
                    .button("+ Matrix")
                    .on_hover_text("place another matrix on this canvas")
                    .clicked()
                {
                    let m = self
                        .selected()
                        .or_else(|| pick_default_measure(&self.model));
                    self.add_matrix(m);
                }
                if self.matrix_count() > 1
                    && ui
                        .button("\u{2212} Matrix")
                        .on_hover_text("remove the focused matrix from this canvas")
                        .clicked()
                {
                    let i = self.focus_index();
                    self.remove_matrix(i);
                }
            });
        });
    }

    /// The **status bar** (plan Step 5): the status message on the left and the
    /// selection-aggregate readout on the right, which is where Quantrix puts
    /// its `Sum`.
    fn status_bar(&self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(self.selection_readout())
                        .on_hover_text("aggregate of the focused matrix's selection");
                });
            });
        });
    }

    /// The selection aggregate for the focused matrix, as the status bar shows
    /// it: the reference's `Sum`.
    ///
    /// Selection is ONE cell today (there is no range selection yet), so the sum
    /// over the selection is that cell's number. Written as a sum rather than as
    /// "the cell's value" because that is what it means and what a range
    /// selection will widen to: [`Self::selection_sum`] takes the cursor cell's
    /// numeric value, and a non-numeric or absent cell contributes nothing, so
    /// the readout says `Sum \u2014` rather than inventing a zero.
    pub fn selection_readout(&self) -> String {
        match self.selection_sum() {
            Some(v) => format!("Sum {v}"),
            None => "Sum \u{2014}".to_string(),
        }
    }

    /// The numeric sum over the focused matrix's selection, or `None` when
    /// nothing numeric is selected (no measure, no addressable cell, an empty
    /// cell, or a text/boolean/date/error cell). See [`Self::selection_readout`].
    pub fn selection_sum(&self) -> Option<f64> {
        let measure = self.selected()?;
        let key = self.cursor_key()?;
        self.cell_value(measure, &key).and_then(|v| v.as_num())
    }

    /// The canvas: **N matrices freely placed on one scrollable surface**, which
    /// is plan Step 3 (`docs/reviews/2026-09-22-gui-reconstruction-plan.md`) and
    /// the shape the Quantrix reference shows \u2014 several matrices, each with its
    /// own title bar and its own pivot, laid out where the author put them.
    ///
    /// **Container choice: manually placed child `Ui`s, not [`egui::Window`].**
    /// Three reasons, in order of weight:
    ///
    /// 1. A `Window` is an [`egui::Area`] \u2014 a *top-level* floating layer. It
    ///    neither scrolls with a parent [`egui::ScrollArea`] nor clips to it, so
    ///    "a scrollable canvas holding matrices" cannot be built out of windows:
    ///    the matrices would hover over the scrolling canvas instead of living
    ///    on it.
    /// 2. The reference's matrices are *document content* (they scroll with the
    ///    notes and headings around them), not OS-style windows with a
    ///    close/collapse chrome.
    /// 3. A child `Ui` placed with [`egui::Ui::new_child`] allocates nothing in
    ///    its parent, so the canvas extent stays a pure function of the matrix
    ///    rects. A container that allocated space per matrix would grow the
    ///    scroll content every frame \u2014 exactly the ratchet Step 1's
    ///    `gutters_do_not_creep_across_frames_or_repivots` exists to catch.
    ///
    /// Each matrix keeps everything Step 1 built: its own margin gutters framing
    /// its own table, its own tiles, virtualization, nested headers, typed
    /// editing and empty-axis handling \u2014 because [`Self::gutter_frame`] is
    /// called once per matrix on that matrix's child `Ui`, and nested panels
    /// work in any `Ui`.
    fn canvas_panel(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.model.measures.is_empty() {
                ui.label("No measures. Open a model store with `improv-gui <db>`.");
                return;
            }
            egui::ScrollArea::both()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    // Claim the canvas extent up front: a pure function of the
                    // matrix rects (plus room to drag into), so it never grows
                    // frame over frame.
                    let extent = self.canvas_extent(ui.available_size());
                    let (_id, canvas) = ui.allocate_space(extent);
                    for mi in 0..self.matrices.len() {
                        self.matrix_frame(ui, canvas.min, mi);
                    }
                });
        });
    }

    /// The canvas surface size: enough to hold every matrix (plus a margin to
    /// drag into) and at least the visible area, so the scroll view fills.
    fn canvas_extent(&self, available: egui::Vec2) -> egui::Vec2 {
        let mut w: f32 = 0.0;
        let mut h: f32 = 0.0;
        for m in &self.matrices {
            w = w.max(m.rect.x + m.rect.w);
            h = h.max(m.rect.y + m.rect.h);
        }
        egui::vec2(
            (w + CANVAS_MARGIN).max(available.x),
            (h + CANVAS_MARGIN).max(available.y),
        )
    }

    /// One matrix on the canvas at `origin + its rect`: title bar, filter shelf,
    /// then the gutter-framed grid, all inside a child `Ui` clipped to its own
    /// rectangle (see [`Self::canvas_panel`] for why a child `Ui` and not a
    /// window).
    fn matrix_frame(&mut self, ui: &mut egui::Ui, origin: egui::Pos2, mi: usize) {
        let Some(m) = self.matrices.get(mi) else {
            return;
        };
        let r = m.rect;
        let rect = egui::Rect::from_min_size(
            origin + egui::vec2(r.x, r.y),
            egui::vec2(r.w.max(MATRIX_MIN_W), r.h.max(MATRIX_MIN_H)),
        );
        let focused = self.focus_index() == mi;

        // The matrix's own beveled shell, so several matrices on one canvas read
        // as distinct objects (the reference frames each one).
        ui.painter().rect(
            rect,
            egui::Rounding::ZERO,
            crate::theme::NEXT_GRAY,
            egui::Stroke::new(
                if focused { 2.0_f32 } else { 1.0_f32 },
                if focused {
                    crate::theme::NEXT_BLUE
                } else {
                    crate::theme::BEVEL_SHADOW
                },
            ),
        );

        // Clicking anywhere in the matrix focuses it. Registered BEFORE the
        // contents, so interior widgets (cells, tiles, buttons) still win the
        // click; this only catches the background.
        let bg = ui.interact(
            rect,
            ui.id().with(("matrix_bg", mi)),
            egui::Sense::click_and_drag(),
        );
        if bg.clicked() || bg.drag_started() {
            self.set_focus(mi);
        }

        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt(("matrix", mi))
                .max_rect(rect.shrink(2.0))
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        child.set_clip_rect(rect.intersect(ui.clip_rect()));
        self.matrix_title_bar(&mut child, mi);
        self.filter_shelf(&mut child, mi);
        child.separator();
        self.gutter_frame(&mut child, mi);

        // Bottom-right resize grip: drag to resize this matrix.
        let grip = egui::Rect::from_min_max(rect.max - egui::vec2(GRIP, GRIP), rect.max);
        let gr = ui.interact(grip, ui.id().with(("matrix_grip", mi)), egui::Sense::drag());
        ui.painter().rect_filled(
            grip,
            egui::Rounding::ZERO,
            if gr.hovered() {
                crate::theme::BEVEL_LIGHT
            } else {
                crate::theme::NEXT_DARK
            },
        );
        if gr.dragged() {
            let d = gr.drag_delta();
            if let Some(m) = self.matrices.get_mut(mi) {
                m.resize_by(d.x, d.y);
            }
            self.set_focus(mi);
        }
    }

    /// A matrix's **title bar** (plan Step 5): the reference's
    /// `Property Financials: Virginia Ave` band \u2014 the measure's name, then the
    /// pinned page items that say *which slice* this matrix shows, which is
    /// precisely what the reference's `: Virginia Ave` is.
    ///
    /// It is also the drag handle: dragging the bar moves the matrix on the
    /// canvas (the reference's free-form placement). Colors come from
    /// `theme.rs`; the accent band is the NeXT selection blue for the focused
    /// matrix and the groove gray otherwise, so which matrix has the keyboard is
    /// visible at a glance.
    fn matrix_title_bar(&mut self, ui: &mut egui::Ui, mi: usize) {
        let focused = self.focus_index() == mi;
        let title = self.matrix_title(mi);
        let (_id, bar) = ui.allocate_space(egui::vec2(ui.available_width(), TITLE_H));
        let resp = ui.interact(
            bar,
            ui.id().with(("title", mi)),
            egui::Sense::click_and_drag(),
        );
        ui.painter().rect_filled(
            bar,
            egui::Rounding::ZERO,
            if focused {
                crate::theme::NEXT_BLUE
            } else {
                crate::theme::NEXT_DARK
            },
        );
        ui.painter().text(
            bar.center(),
            egui::Align2::CENTER_CENTER,
            &title,
            egui::TextStyle::Body.resolve(ui.style()),
            if focused {
                crate::theme::BEVEL_LIGHT
            } else {
                egui::Color32::from_gray(0x10)
            },
        );
        if resp.dragged() {
            let d = resp.drag_delta();
            if let Some(m) = self.matrices.get_mut(mi) {
                m.move_by(d.x, d.y);
            }
        }
        if resp.clicked() || resp.drag_started() {
            self.set_focus(mi);
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
    }

    /// A matrix's title text: its measure's name, plus the pinned page items
    /// after a colon (`Sales: North`) \u2014 the reference's
    /// `Property Financials: Virginia Ave`.
    pub fn matrix_title(&self, mi: usize) -> String {
        let Some(m) = self.matrices.get(mi) else {
            return String::new();
        };
        let name = m
            .measure
            .and_then(|id| self.model.measures.get(&id))
            .map(|x| x.name.0.clone())
            .unwrap_or_else(|| "(no measure)".to_string());
        let slice: Vec<String> = m
            .pinned_pages(&self.model)
            .iter()
            .filter_map(|(_, it)| self.model.items.get(it).map(|x| x.name.0.clone()))
            .collect();
        if slice.is_empty() {
            name
        } else {
            format!("{name}: {}", slice.join(", "))
        }
    }

    /// The grid inside its **margin gutters** — the Improv pivot surface.
    ///
    /// Reference: `docs/reviews/refs/improv-pivot-gesture.jpg`. Category tiles
    /// are docked in gutters that *frame* the table, and the pivot gesture IS
    /// dragging a tile from one gutter to another:
    ///
    /// ```text
    ///   +- corner --+- TOP gutter: column-axis tiles, stacked ---+
    ///   | (blank)   |  || Travel v                               |
    ///   |           |  || Hours  v                               |
    ///   +-----------+----------------------------------------------+
    ///   | LEFT      |                                              |
    ///   | gutter:   |    the table (chiseled headers + cells)      |
    ///   | row-axis  |                                              |
    ///   | tiles     |                                              |
    ///   +-----------+----------------------------------------------+
    ///   | WELL (bottom-left, inline with the horizontal scrollbar  |
    ///   | in the reference): page/unplaced tiles + page selectors  |
    ///   +----------------------------------------------------------+
    /// ```
    ///
    /// Adjacency is **constructed, not hoped for**: the gutters are nested
    /// [`egui::SidePanel`]/[`egui::TopBottomPanel`]s inside this `Ui`, and a
    /// panel's contract is that it consumes its strip and moves the parent
    /// cursor to its far edge, so whatever is laid out next starts exactly at
    /// that edge. Each panel's own rect and the table's `min_rect` are recorded
    /// in that MATRIX's own `gutters` field, `debug_assert!`ed against
    /// [`gutters_frame_table`] every frame, and asserted in the headless layout
    /// test — per matrix, so every matrix on the canvas frames its own table.
    ///
    /// Gutter *thickness* along the docked axis is left to the panels: they size
    /// to their content (a panel re-reads its extent from the previous frame's
    /// `PanelState`), so stacking another tile makes the gutter grow rather than
    /// clip it against a hard-coded height. The row gutter is the one fixed
    /// width, [`GUTTER_W`], so the grid's left edge does not jump as category
    /// names change.
    fn gutter_frame(&mut self, ui: &mut egui::Ui, mi: usize) {
        let Some(m) = self.matrices.get(mi) else {
            return;
        };
        let Some(measure) = m.measure else {
            ui.weak("(no measure)");
            return;
        };
        let mut moves: Vec<(CategoryId, Axis)> = Vec::new();
        let col_stack = m.col_cats(&self.model);
        let row_stack = m.row_cats(&self.model);
        let page_cats = m.page_cats(&self.model);

        // Bottom-left corner well first: it claims the bottom strip of the whole
        // frame, so the two edge gutters and the table share the space above it
        // (the reference puts it inline with the horizontal scrollbar).
        let well = egui::TopBottomPanel::bottom(ui.id().with("gutter_well"))
            .frame(Self::gutter_style())
            .show_inside(ui, |ui| {
                Self::gutter_drop_zone(
                    ui,
                    Axis::Pages,
                    FillAxis::Horizontal,
                    &mut moves,
                    |ui, moves| {
                        ui.horizontal(|ui| {
                            if page_cats.is_empty() {
                                ui.weak("(drop to unplace)");
                            }
                            for c in &page_cats {
                                Self::tile(ui, &self.model, mi, *c, Axis::Pages, moves);
                            }
                        });
                    },
                );
                self.page_selectors(ui, mi);
            })
            .response
            .rect;

        // The column gutter: the top strip, spanning the table's full width.
        // Stacked column categories read top-to-bottom, as the reference's
        // "Result" panel shows (`Travel` above `Hours`).
        let top = egui::TopBottomPanel::top(ui.id().with("gutter_top"))
            .frame(Self::gutter_style())
            .show_inside(ui, |ui| {
                // Skip the true corner: the column tiles start one row-gutter
                // width in, leaving a blank box over the row gutter (the
                // reference shows an empty box there). Painted below rather than
                // nested as a panel — a nested panel fills the strip's height,
                // which would latch this self-sizing gutter at its tallest.
                ui.horizontal(|ui| {
                    ui.add_space(GUTTER_W);
                    Self::gutter_drop_zone(
                        ui,
                        Axis::Columns,
                        FillAxis::Horizontal,
                        &mut moves,
                        |ui, moves| {
                            ui.vertical(|ui| {
                                if col_stack.is_empty() {
                                    ui.weak("(drop a category on Columns)");
                                }
                                for c in &col_stack {
                                    Self::tile(ui, &self.model, mi, *c, Axis::Columns, moves);
                                }
                            });
                        },
                    );
                });
            })
            .response
            .rect;

        // The row gutter: the left strip of what remains, so its right edge is
        // the table's left edge.
        let left = egui::SidePanel::left(ui.id().with("gutter_left"))
            .frame(Self::gutter_style())
            .exact_width(GUTTER_W)
            .show_inside(ui, |ui| {
                Self::gutter_drop_zone(
                    ui,
                    Axis::Rows,
                    FillAxis::Vertical,
                    &mut moves,
                    |ui, moves| {
                        ui.vertical(|ui| {
                            if row_stack.is_empty() {
                                ui.weak("(drop a category on Rows)");
                            }
                            for c in &row_stack {
                                Self::tile(ui, &self.model, mi, *c, Axis::Rows, moves);
                            }
                        });
                    },
                );
            })
            .response
            .rect;

        // The blank corner box at the true corner: the part of the column gutter
        // sitting over the row gutter.
        ui.painter().rect_stroke(
            egui::Rect::from_min_max(top.min, egui::pos2(left.max.x, top.max.y)).shrink(2.0),
            egui::Rounding::ZERO,
            egui::Stroke::new(1.0_f32, crate::theme::NEXT_DARK),
        );

        // Whatever is left is the table: it starts at the row gutter's right
        // edge and the column gutter's bottom edge by panel construction.
        let table = ui
            .scope(|ui| {
                self.render_grid(ui, mi, measure);
            })
            .response
            .rect;

        let rects = GutterRects {
            top,
            left,
            table,
            well,
        };
        debug_assert!(
            !gutters_have_room(&rects) || gutters_frame_table(&rects),
            "matrix {mi}'s gutters must frame its table: {rects:?}"
        );
        if let Some(m) = self.matrices.get_mut(mi) {
            m.gutters = Some(rects);
        }

        // A drop re-pivots THIS matrix, not the focused one: dragging a tile in
        // one matrix must not move another matrix's axis.
        if !moves.is_empty() {
            let model = &self.model;
            if let Some(m) = self.matrices.get_mut(mi) {
                for (c, axis) in moves {
                    m.set_axis(model, c, axis);
                }
            }
        }
    }

    /// The NeXT-groove frame a gutter is painted with.
    fn gutter_style() -> egui::Frame {
        egui::Frame::default()
            .fill(crate::theme::NEXT_GRAY)
            .inner_margin(egui::Margin::same(2.0))
            .stroke(egui::Stroke::new(1.0_f32, crate::theme::NEXT_DARK))
    }

    /// Make the whole of `ui` a drop target for category tiles: a category
    /// released here is recorded as a move to `axis`, which
    /// [`ImprovApp::set_axis`] applies (appending to that axis' stack). The zone
    /// stretches across the gutter so the drop target IS the gutter, not just the
    /// tiles in it — an empty axis must still be droppable.
    ///
    /// `fill` says which way to stretch. It must be the gutter's **fixed** axis:
    /// a zone that claims all the available space along a *self-sizing* panel's
    /// own axis ratchets that panel wider every frame, because next frame's
    /// "available" includes what the zone claimed last frame.
    fn gutter_drop_zone(
        ui: &mut egui::Ui,
        axis: Axis,
        fill: FillAxis,
        moves: &mut Vec<(CategoryId, Axis)>,
        contents: impl FnOnce(&mut egui::Ui, &mut Vec<(CategoryId, Axis)>),
    ) {
        let (_, dropped) = ui.dnd_drop_zone::<CategoryId, ()>(egui::Frame::default(), |ui| {
            match fill {
                FillAxis::Horizontal => ui.set_min_width(ui.available_width()),
                FillAxis::Vertical => ui.set_min_height(ui.available_height()),
            }
            contents(ui, moves);
        });
        if let Some(c) = dropped {
            moves.push((*c, axis));
        }
    }

    /// One category tile: the Quantrix `|| Name v` chip — a grip glyph to drag by,
    /// the category name, and a dropdown arrow (its menu of per-category actions
    /// — filter / sort / collapse — arrives in Step 4; for now the arrow is the
    /// mouse-only *cycle* affordance, rows→columns→pages, which is also the
    /// keyboard-free fallback for re-pivoting without a drag).
    ///
    /// The whole chip is a drag source, so dragging it into another gutter
    /// re-pivots (the drop is handled by [`ImprovApp::gutter_drop_zone`]).
    fn tile(
        ui: &mut egui::Ui,
        model: &Model,
        mi: usize,
        c: CategoryId,
        from: Axis,
        moves: &mut Vec<(CategoryId, Axis)>,
    ) {
        let name = model
            .categories
            .get(&c)
            .map(|x| x.name.0.clone())
            .unwrap_or_else(|| format!("category {}", c.0));
        let next = match from {
            Axis::Rows => Axis::Columns,
            Axis::Columns => Axis::Pages,
            Axis::Pages => Axis::Rows,
        };
        // The id is per MATRIX as well as per category: two matrices showing
        // the same category are two distinct drag sources.
        ui.dnd_drag_source(egui::Id::new(("tile", mi, c.0)), c, |ui| {
            egui::Frame::default()
                .fill(crate::theme::NEXT_LIGHT)
                .stroke(egui::Stroke::new(1.0_f32, crate::theme::BEVEL_SHADOW))
                .inner_margin(egui::Margin::symmetric(4.0, 2.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Grip: egui's default font has no `⋮`/`⁞`, so the grip is
                        // a pair of broken bars, which it does have.
                        ui.add(
                            egui::Label::new(egui::RichText::new("¦¦").monospace().weak())
                                .selectable(false),
                        );
                        ui.add(
                            egui::Label::new(egui::RichText::new(&name).strong())
                                .selectable(false)
                                .truncate(),
                        );
                        if ui
                            .small_button("⏷")
                            .on_hover_text(format!("move {name} to {next:?}"))
                            .clicked()
                        {
                            moves.push((c, next));
                        }
                    });
                });
        });
    }

    /// Page selectors: for each page (extra) dimension, a ` <label> [i/n] < > `
    /// control that pins which item the grid slices to. Mirrors the TUI paging.
    fn page_selectors(&mut self, ui: &mut egui::Ui, mi: usize) {
        let Some(m) = self.matrices.get(mi) else {
            return;
        };
        let page_cats: Vec<CategoryId> = m.page_cats(&self.model);
        if page_cats.is_empty() {
            return;
        }
        let mut set: Option<(usize, usize)> = None;
        ui.horizontal(|ui| {
            for (i, c) in page_cats.iter().enumerate() {
                let items = m.sorted_items(&self.model, *c);
                if items.is_empty() {
                    continue;
                }
                let cur = m.page_idx.get(i).copied().unwrap_or(0).min(items.len() - 1);
                let cname = self
                    .model
                    .categories
                    .get(c)
                    .map(|x| x.name.0.clone())
                    .unwrap_or_default();
                ui.push_id(("page", mi, c.0), |ui| {
                    ui.group(|ui| {
                        ui.label(&cname);
                        if ui.small_button("<").clicked() {
                            let prev = (cur + items.len() - 1) % items.len();
                            set = Some((i, prev));
                        }
                        ui.label(format!("{}  [{}/{}]", items[cur].1, cur + 1, items.len()));
                        if ui.small_button(">").clicked() {
                            set = Some((i, (cur + 1) % items.len()));
                        }
                    });
                });
            }
        });
        if let Some((dim, idx)) = set {
            let model = &self.model;
            if let Some(m) = self.matrices.get_mut(mi) {
                m.set_page(model, dim, idx);
            }
        }
    }

    /// Filter shelf for matrix `mi`: for each category on an axis, a collapsing
    /// checkbox list of its items. Unchecking an item hides it from THIS matrix's
    /// grid (presentation only); a "Clear filters" button restores all. Mirrors
    /// the TUI's f/F.
    fn filter_shelf(&mut self, ui: &mut egui::Ui, mi: usize) {
        let Some(matrix) = self.matrices.get(mi) else {
            return;
        };
        let cats: Vec<CategoryId> = matrix.axis_order.clone();
        let filters = matrix.filters.clone();
        if cats.is_empty() {
            return;
        }
        let mut toggles: Vec<(CategoryId, ItemId)> = Vec::new();
        let mut clear = false;
        ui.collapsing("Filters", |ui| {
            for c in &cats {
                let cname = self
                    .model
                    .categories
                    .get(c)
                    .map(|x| x.name.0.clone())
                    .unwrap_or_default();
                // Full (unfiltered) item list so hidden items can be re-shown.
                let mut items: Vec<(ItemId, String)> = self
                    .model
                    .categories
                    .get(c)
                    .map(|cat| {
                        cat.items
                            .iter()
                            .filter_map(|id| {
                                self.model.items.get(id).map(|it| (*id, it.name.0.clone()))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                items.sort_by_key(|(id, _)| id.0);
                let f = filters.iter().find(|f| f.category == *c);
                egui::CollapsingHeader::new(&cname)
                    .id_salt(("filter", mi, c.0))
                    .show(ui, |ui| {
                        for (id, name) in &items {
                            let shown = match f {
                                Some(f) => f.items.contains(id),
                                None => true,
                            };
                            let mut checked = shown;
                            if ui.checkbox(&mut checked, name).changed() {
                                toggles.push((*c, *id));
                            }
                        }
                    });
            }
            if !filters.is_empty() && ui.button("Clear filters").clicked() {
                clear = true;
            }
        });
        let model = &self.model;
        if let Some(m) = self.matrices.get_mut(mi) {
            for (c, i) in toggles {
                m.toggle_filter_item(model, c, i);
            }
            if clear {
                m.clear_filters(model);
            }
        }
    }

    /// Handle keyboard navigation for the grid. Arrow keys (and h/j/k/l) move
    /// the cursor; Enter/F2 begin editing the cursor cell; `[`/`]` and
    /// PageUp/PageDown page the first page dimension; `n`/`N` cycle the
    /// selected measure; Ctrl/Cmd+Z undoes and Ctrl/Cmd+Shift+Z (or Ctrl+Y)
    /// redoes. Swallowed while a cell text field is open, or while ANY other
    /// egui widget owns keyboard focus (so typing in the formula bar, the
    /// new-measure form, the CSV wizard, or the view-name box never also drives
    /// the grid). `n`/`N` (not Tab) drive measure cycling because egui reserves
    /// Tab for widget focus.
    ///
    /// **Only the FOCUSED matrix consumes them** (plan Step 3): `mi` is the
    /// matrix being rendered, and a matrix that does not have the keyboard
    /// returns immediately — otherwise one arrow key would move every matrix's
    /// cursor at once.
    fn handle_grid_keys(&mut self, ui: &egui::Ui, mi: usize) {
        if mi != self.focus_index() {
            return;
        }
        // `memory().focused()` is egui 0.29's "which widget owns the keyboard";
        // it is `None` again as soon as focus is released, so this gate is
        // transient, never sticky. While a CELL is being edited the focused
        // widget is the grid's own editor, which `editing` already covers
        // (Enter/Esc are handled in the cell rendering below).
        let other_focus = ui.ctx().memory(|m| m.focused()).is_some();
        if !grid_keys_enabled(self.focused().editing.is_some(), other_focus) {
            return;
        }
        use egui::Key;
        let k = |key: Key| ui.input(|i| i.key_pressed(key));
        let modifiers = ui.input(|i| i.modifiers);

        // Undo/redo (Ctrl/Cmd chords) are handled app-wide in
        // `handle_history_keys`, not here — the grid is not rendered at all when
        // no measure is selected, and undoing must still work there. Leaving
        // every modified chord alone also keeps e.g. Ctrl+N from cycling
        // measures.
        if modifiers.command {
            return;
        }

        if k(Key::ArrowUp) || k(Key::K) {
            self.move_cursor(-1, 0);
        }
        if k(Key::ArrowDown) || k(Key::J) {
            self.move_cursor(1, 0);
        }
        if k(Key::ArrowLeft) || k(Key::H) {
            self.move_cursor(0, -1);
        }
        if k(Key::ArrowRight) || k(Key::L) {
            self.move_cursor(0, 1);
        }
        if k(Key::Enter) || k(Key::F2) {
            self.begin_edit_cursor();
        }
        // Page the first page dimension, if any.
        if k(Key::CloseBracket) || k(Key::PageDown) {
            self.page_first(1);
        }
        if k(Key::OpenBracket) || k(Key::PageUp) {
            self.page_first(-1);
        }
        // Cycle measures with n / N (Tab is taken by egui focus).
        if k(Key::N) {
            let shift = ui.input(|i| i.modifiers.shift);
            self.cycle_measure(if shift { -1 } else { 1 });
        }
    }

    /// Cycle the focused matrix's first page dimension by `delta` (wrapping).
    fn page_first(&mut self, delta: isize) {
        self.with_focused(|m, model| m.page_first(model, delta));
    }

    /// Cycle the focused matrix's measure by `delta` (wrapping) in id order.
    fn cycle_measure(&mut self, delta: isize) {
        let mut ids: Vec<MeasureId> = self.model.measures.keys().copied().collect();
        if ids.is_empty() {
            return;
        }
        ids.sort_by_key(|m| m.0);
        let cur = self
            .selected()
            .and_then(|s| ids.iter().position(|m| *m == s))
            .unwrap_or(0);
        let next = (cur as isize + delta).rem_euclid(ids.len() as isize) as usize;
        self.focused_mut().select(Some(ids[next]));
    }

    /// Render matrix `mi`'s `measure` as a 2-D pivot grid using THAT MATRIX's
    /// axis order: its row stack on rows, its column stack on columns, the rest
    /// pinned to its selected page items. Input cells are editable; derived
    /// read-only.
    ///
    /// Every read here goes through matrix `mi`, so two matrices showing the same
    /// measure with different pivots render differently, and only the focused one
    /// takes keystrokes (see [`Self::handle_grid_keys`]).
    fn render_grid(&mut self, ui: &mut egui::Ui, mi: usize, measure: MeasureId) {
        self.handle_grid_keys(ui, mi);
        let Some(m) = self.matrices.get(mi) else {
            return;
        };
        let cursor = (m.cursor_row, m.cursor_col);
        let editing = m.editing.clone();
        let mut edit_buf = m.edit_buf.clone();
        let is_derived = self
            .model
            .measures
            .get(&measure)
            .map(|m| m.is_derived())
            .unwrap_or(false);

        // Cartesian product of stacked categories per axis. Columns are
        // materialized up front (they become egui table columns, which the
        // TableBuilder needs before the body, and are few in practice). Rows
        // are VIRTUALIZED: we hold only the per-category item lists and decode
        // the i-th row tuple on demand (see `nth_tuple`), so a grid with
        // millions of row lines never allocates them all.
        let row_cats = m.row_cats(&self.model);
        let col_cats = m.col_cats(&self.model);
        // Record what this matrix is being drawn with (see `Matrix::rendered_axes`).
        if let Some(m) = self.matrices.get_mut(mi) {
            m.rendered_axes = Some((row_cats.clone(), col_cats.clone()));
        }
        let Some(m) = self.matrices.get(mi) else {
            return;
        };
        // A page category filtered to zero items pins nothing, so no coordinate
        // would fully specify the measure's dimensions: render no cells at all.
        let pinned = m.pinned_pages_opt(&self.model);
        let pages_ok = pinned.is_some();
        let pinned = pinned.unwrap_or_default();
        let row_lists = m.axis_item_lists(&self.model, &row_cats);
        // Total row lines: product of the row categories' filtered item counts.
        // No row categories -> 1 (a genuinely scalar axis). Any row category
        // filtered to zero items -> 0 lines: an empty grid with headers, never a
        // synthetic line (that line's coordinate would omit the category, and
        // `nth_tuple` would divide by a zero radix).
        let total_rows = if pages_ok { product_len(&row_lists) } else { 0 };
        // Column lines, materialized. `axis_tuples` already yields one empty
        // tuple for a scalar axis (no col categories) and nothing at all for a
        // category filtered to zero items — keep both as-is.
        let col_lines = if pages_ok {
            m.axis_tuples(&self.model, &col_cats)
        } else {
            Vec::new()
        };
        let n_row_stub = row_cats.len().max(1); // stub columns (one per row cat)
        let n_col_hdr = col_cats.len().max(1); // header rows (one per col cat)

        // Decode the i-th row tuple on demand (empty when there are no row
        // categories -> the single scalar row).
        let row_tuple_at = |lists: &[Vec<(ItemId, String)>], i: usize| -> Vec<(ItemId, String)> {
            if lists.is_empty() {
                Vec::new()
            } else {
                nth_tuple(lists, i)
            }
        };

        // Edits collected during rendering, applied after the table closure so
        // we don't borrow `self` mutably inside it.
        let mut commit: Option<(CoordKey, String)> = None;
        let mut clicked_derived = false;
        let mut cancel = false;
        // The cursor cell a click chose, applied after the closure.
        let mut clicked_cell: Option<(usize, usize)> = None;
        let mut begin_edit: Option<(CoordKey, String)> = None;

        // Chiseled header cell (raised bevel) matching NeXTSTEP Improv.
        fn header_cell(ui: &mut egui::Ui, text: &str) {
            egui::Frame::default()
                .fill(crate::theme::NEXT_LIGHT)
                .stroke(egui::Stroke::new(1.0_f32, crate::theme::BEVEL_SHADOW))
                .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                .show(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(text).strong()).truncate());
                });
        }
        let cat_name = |app: &ImprovApp, c: CategoryId| {
            app.model
                .categories
                .get(&c)
                .map(|x| x.name.0.clone())
                .unwrap_or_default()
        };

        use egui_extras::{Column, TableBuilder};
        // Salt the table id with the matrix index: two matrices on one canvas are
        // two tables, and sharing an id would make them fight over column widths
        // and scroll offsets.
        let mut table = TableBuilder::new(ui).id_salt(("grid", mi)).striped(true);
        // One stub column per stacked row category, then one column per col line.
        for _ in 0..n_row_stub {
            table = table.column(Column::auto().resizable(true));
        }
        for _ in &col_lines {
            table = table.column(Column::auto().resizable(true));
        }

        table
            .header(22.0 * n_col_hdr as f32, |mut header| {
                // Corner stub: the row category names stacked, then a header
                // block spanning the column-category rows. With egui_extras we
                // render the stacked column-category labels inside one tall
                // header cell per column line (outer→inner, top→bottom).
                for (si, rc) in row_cats.iter().enumerate() {
                    let _ = si;
                    header.col(|ui| {
                        header_cell(ui, &cat_name(self, *rc));
                    });
                }
                if row_cats.is_empty() {
                    header.col(|ui| {
                        header_cell(ui, "");
                    });
                }
                for line in &col_lines {
                    header.col(|ui| {
                        ui.vertical(|ui| {
                            if line.is_empty() {
                                header_cell(ui, "");
                            }
                            for (it, name) in line {
                                let _ = it;
                                header_cell(ui, name);
                            }
                        });
                    });
                }
            })
            .body(|body| {
                // Virtualized rows: `body.rows` only invokes the closure for the
                // rows currently visible in the viewport. Because rows are not
                // built contiguously, group outlining can't rely on a running
                // `prev` tracker — for row `ri` we decode row `ri-1`'s tuple on
                // demand and blank an outer stub cell when it matches.
                body.rows(20.0, total_rows, |mut row| {
                    let ri = row.index();
                    let row_line = row_tuple_at(&row_lists, ri);
                    let prev_line = if ri > 0 {
                        Some(row_tuple_at(&row_lists, ri - 1))
                    } else {
                        None
                    };
                    // Stub columns: one per stacked row category.
                    for si in 0..n_row_stub {
                        let cell = row_line.get(si);
                        row.col(|ui| {
                            match cell {
                                Some((_id, name)) => {
                                    // Show the label on the innermost stub always,
                                    // and on an outer stub only when this level or
                                    // any enclosing outer level changed from the
                                    // row above — group outlining.
                                    let inner = si + 1 == n_row_stub;
                                    let changed = match &prev_line {
                                        None => true,
                                        Some(p) => (0..=si).any(|k| {
                                            row_line.get(k).map(|(id, _)| id)
                                                != p.get(k).map(|(id, _)| id)
                                        }),
                                    };
                                    if inner || changed {
                                        header_cell(ui, name);
                                    } else {
                                        header_cell(ui, "");
                                    }
                                }
                                None => header_cell(ui, ""),
                            }
                        });
                    }
                    for (ci, col_line) in col_lines.iter().enumerate() {
                        let key =
                            cell_key_multi(&row_cats, &row_line, &col_cats, col_line, &pinned);
                        let is_cursor = cursor == (ri, ci);
                        row.col(|ui| {
                            let mut frame = egui::Frame::default();
                            if is_cursor {
                                frame = frame.fill(ui.visuals().selection.bg_fill).stroke(
                                    egui::Stroke::new(1.0_f32, ui.visuals().selection.stroke.color),
                                );
                            }
                            frame.show(ui, |ui| {
                                if is_derived {
                                    let text = self.cell_text(measure, &key).unwrap_or_default();
                                    if ui.label(text).clicked() {
                                        clicked_cell = Some((ri, ci));
                                        clicked_derived = true;
                                    }
                                } else if editing.as_ref() == Some(&(measure, key.clone())) {
                                    let resp = ui.add(
                                        egui::TextEdit::singleline(&mut edit_buf)
                                            .desired_width(f32::INFINITY),
                                    );
                                    resp.request_focus();
                                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                                    let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                                    if esc {
                                        cancel = true;
                                    } else if resp.lost_focus() || enter {
                                        commit = Some((key.clone(), edit_buf.clone()));
                                    }
                                } else {
                                    // Typed display: text, booleans, dates, and
                                    // #ERR all render as themselves (never blank
                                    // just because they are not numbers).
                                    let text = self.cell_text(measure, &key).unwrap_or_default();
                                    if ui.button(text.clone()).clicked() {
                                        clicked_cell = Some((ri, ci));
                                        begin_edit = Some((key.clone(), text));
                                    }
                                }
                            });
                        });
                    }
                });
            });

        // Apply what the closure recorded. A click in ANY matrix also focuses it,
        // so typing follows the mouse rather than staying on whichever matrix had
        // the keyboard.
        if let Some(m) = self.matrices.get_mut(mi) {
            m.edit_buf = edit_buf;
            if let Some((ri, ci)) = clicked_cell {
                m.cursor_row = ri;
                m.cursor_col = ci;
            }
            if let Some((key, text)) = begin_edit {
                m.editing = Some((measure, key));
                m.edit_buf = text;
            }
            if cancel || commit.is_some() {
                m.editing = None;
            }
        }
        if clicked_cell.is_some() {
            self.set_focus(mi);
        }
        if cancel {
            self.status = "edit cancelled".into();
        }
        if clicked_derived {
            self.status = "derived cells are computed, not editable".into();
        }
        if let Some((key, text)) = commit {
            self.status = match self.commit_cell_text(measure, key, &text) {
                Ok(msg) => msg,
                Err(e) => format!("edit error: {e}"),
            };
        }
    }
}

/// The sorted `CoordKey` for a cell when categories are STACKED on each axis:
/// `row_cats[i]` binds to `row_tuple[i]`, `col_cats[j]` to `col_tuple[j]`, plus
/// the pinned page dims. Tuples come from `axis_tuples` so they align with the
/// category list. The general form of `cell_key`.
fn cell_key_multi(
    row_cats: &[CategoryId],
    row_tuple: &[(ItemId, String)],
    col_cats: &[CategoryId],
    col_tuple: &[(ItemId, String)],
    pinned: &[(CategoryId, ItemId)],
) -> CoordKey {
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for (c, (it, _)) in row_cats.iter().zip(row_tuple.iter()) {
        pairs.push((c.0, it.0));
    }
    for (c, (it, _)) in col_cats.iter().zip(col_tuple.iter()) {
        pairs.push((c.0, it.0));
    }
    for (c, i) in pinned {
        pairs.push((c.0, i.0));
    }
    pairs.sort();
    pairs
}

/// The number of axis lines the Cartesian product of `lists` produces: the
/// product of each list's length (empty `lists` -> 1, the scalar axis; any
/// empty list -> 0, no lines). Mirrors `axis_tuples(..).len()` but is O(k) in
/// the number of categories, never materializing the product.
fn product_len(lists: &[Vec<(ItemId, String)>]) -> usize {
    lists.iter().map(|l| l.len()).product()
}

/// The i-th line of the Cartesian product of `lists` (outer category first),
/// by mixed-radix decoding of `i` across the list lengths — the inner (last)
/// category is the least-significant digit, matching `axis_tuples`' ordering
/// (which increments the inner category fastest). Returns the bound
/// `(ItemId, name)` per category.
///
/// # Panics
///
/// `i` must be `< product_len(lists)`, which implies every list is non-empty.
/// A list filtered to zero items means the axis has NO lines, so there is no
/// `i`-th line to decode; callers (`grid_dims`, `render_grid`, `cursor_key`)
/// render zero lines in that case rather than calling here. The check is a
/// hard `assert!` (not `debug_assert!`) because `i % 0` is a divide-by-zero
/// crash in release builds.
///
/// This is what lets the grid virtualize rows: instead of holding every row
/// tuple in a Vec, we decode line `i` (and, for group outlining, line `i-1`)
/// only when that row is actually painted.
fn nth_tuple(lists: &[Vec<(ItemId, String)>], mut i: usize) -> Vec<(ItemId, String)> {
    let mut out: Vec<(ItemId, String)> = vec![(ItemId(0), String::new()); lists.len()];
    for (d, list) in lists.iter().enumerate().rev() {
        let radix = list.len();
        assert!(radix > 0, "empty category has no lines");
        out[d] = list[i % radix].clone();
        i /= radix;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use improv_core_model::{
        BinaryOp, DimensionSpec, Expr, Formula, Measure, MeasureKind, Name, ValueType,
    };

    fn revenue_model() -> Model {
        let mut m = Model::new();
        let (t, p) = (CategoryId(1), CategoryId(2));
        m.add_category(t, "Time");
        m.add_category(p, "Product");
        m.add_item(ItemId(10), t, "2025");
        m.add_item(ItemId(20), p, "WidgetA");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
        m.set_input(MeasureId(100), c(&[(p, ItemId(20))]), Value::Number(10.0));
        m.set_input(
            MeasureId(101),
            c(&[(t, ItemId(10)), (p, ItemId(20))]),
            Value::Number(7.0),
        );
        m
    }

    /// A revenue model without the Revenue derived measure (inputs only), for
    /// exercising the formula-commit / add-derived flows.
    fn inputs_only_model() -> Model {
        let mut m = revenue_model();
        m.measures.remove(&MeasureId(102));
        m
    }

    #[test]
    fn loads_and_computes_derived() {
        let app = build_app(revenue_model());
        // Revenue is derived and selected by default.
        assert_eq!(app.selected(), Some(MeasureId(102)));
        let vals = app.values_for(MeasureId(102));
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&70.0)); // 10 * 7
    }

    #[test]
    fn editing_recomputes_derived() {
        let mut app = build_app(revenue_model());
        // Set Quantity[2025,WidgetA] = 9 -> Revenue = 90.
        let mut qkey = vec![(1u32, 10u32), (2u32, 20u32)];
        qkey.sort();
        app.set_cell(MeasureId(101), qkey.clone(), Value::Number(9.0))
            .unwrap();
        let rev = app.values_for(MeasureId(102));
        assert_eq!(rev.get(&qkey), Some(&90.0));
    }

    #[test]
    fn editing_derived_is_rejected() {
        let mut app = build_app(revenue_model());
        let key = vec![(1u32, 10u32), (2u32, 20u32)];
        assert!(app
            .set_cell(MeasureId(102), key, Value::Number(1.0))
            .is_err());
    }

    #[test]
    fn commit_formula_creates_and_recomputes() {
        // Turn an input measure into a derived one via a formula string, then
        // confirm the snapshot recomputes.
        let mut app = build_app(inputs_only_model());
        // Add a fresh target measure to hold the formula.
        let rev = app
            .add_derived_measure("Revenue", "Price * Quantity")
            .expect("add derived");
        let vals = app.values_for(rev);
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&70.0)); // 10 * 7
                                                 // The measure is derived and its categories are the union {Time, Product}.
        let m = &app.model.measures[&rev];
        assert!(m.is_derived());
        assert_eq!(m.categories, vec![CategoryId(1), CategoryId(2)]);
    }

    #[test]
    fn commit_formula_updates_existing_derived() {
        let mut app = build_app(revenue_model());
        // Redefine Revenue = Price + Quantity -> 10 + 7 = 17.
        app.commit_formula(MeasureId(102), "Price + Quantity")
            .expect("commit");
        let vals = app.values_for(MeasureId(102));
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&17.0));
    }

    #[test]
    fn bad_formula_leaves_model_unchanged() {
        let mut app = build_app(revenue_model());
        let before = app.model.clone();
        let err = app.commit_formula(MeasureId(102), "Price * Widgets");
        assert!(err.is_err());
        assert_eq!(app.model, before, "model unchanged on parse error");
        // Snapshot still computes the original Revenue.
        let vals = app.values_for(MeasureId(102));
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(vals.get(&key), Some(&70.0));
    }

    #[test]
    fn commit_formula_error_sets_and_clears_error_pos() {
        let mut app = build_app(revenue_model());
        assert_eq!(app.formula_error_pos, None);

        // A failing commit records the parser's reported byte offset.
        let text = "Price * Widgets";
        let err = app.commit_formula(MeasureId(102), text).unwrap_err();
        let expected_pos = parser::parse_expr(&app.model, text).unwrap_err().position;
        assert!(expected_pos.is_some());
        assert_eq!(app.formula_error_pos, expected_pos);
        assert!(!app.formula_error_msg.is_empty());
        assert!(err.contains("Widgets") || err.contains("measure"));

        // Clearing the error (what the formula bar does when the buffer is
        // edited again) resets both fields.
        app.formula_error_pos = None;
        app.formula_error_msg.clear();
        assert_eq!(app.formula_error_pos, None);

        // A successful commit clears any leftover error state too.
        app.formula_error_pos = Some(999);
        app.formula_error_msg = "stale".into();
        app.commit_formula(MeasureId(102), "Price + Quantity")
            .expect("commit");
        assert_eq!(app.formula_error_pos, None);
        assert!(app.formula_error_msg.is_empty());
    }

    #[test]
    fn bad_add_derived_leaves_model_unchanged() {
        let mut app = build_app(revenue_model());
        let before = app.model.clone();
        assert!(app
            .add_derived_measure("Junk", "does not parse [[")
            .is_err());
        assert!(app.add_derived_measure("Revenue", "Price").is_err()); // dup name
        assert_eq!(app.model, before);
    }

    // -- BUG 2: the displayed formula must commit unchanged ----------------

    /// Every formula shape the formula bar can display, for the round-trip.
    /// `SQL(...)`/`CALL(...)` are measure *definitions* whose measures stay
    /// `MeasureKind::Input` (see `cli::cmd_define`), so the formula bar shows
    /// them the input hint, never editable formula text — nothing to round-trip.
    fn displayable_formulas() -> Vec<(&'static str, Formula)> {
        let (t, p) = (CategoryId(1), CategoryId(2));
        let price = || Expr::Ref(MeasureId(100), DimensionSpec::default());
        let qty = || Expr::Ref(MeasureId(101), DimensionSpec::default());
        let bin = |op, l: Expr, r: Expr| Expr::BinaryOp(op, Box::new(l), Box::new(r));
        vec![
            (
                "binary op",
                Formula::new(bin(BinaryOp::Mul, price(), qty())),
            ),
            (
                "nested binary ops needing parens",
                Formula::new(bin(
                    BinaryOp::Mul,
                    bin(BinaryOp::Add, price(), qty()),
                    bin(BinaryOp::Sub, qty(), Expr::Literal(Value::Number(2.0))),
                )),
            ),
            (
                "right-nested same-precedence",
                Formula::new(bin(
                    BinaryOp::Sub,
                    price(),
                    bin(BinaryOp::Sub, qty(), Expr::Literal(Value::Number(1.0))),
                )),
            ),
            (
                "aggregation with OVER",
                Formula::new(Expr::Call(
                    FuncId(1), // SUM
                    vec![Expr::Ref(
                        MeasureId(101),
                        DimensionSpec {
                            by: vec![],
                            over: vec![t],
                            except: vec![],
                        },
                    )],
                )),
            ),
            (
                "scalar call",
                Formula::new(Expr::Call(FuncId(10), vec![price()])), // ABS
            ),
            (
                "two-arg scalar call",
                Formula::new(Expr::Call(FuncId(20), vec![price(), qty()])), // MIN2
            ),
            (
                "comparison",
                Formula::new(bin(BinaryOp::Gt, price(), qty())),
            ),
            (
                "logical over comparisons",
                Formula::new(bin(
                    BinaryOp::And,
                    bin(BinaryOp::Gt, price(), qty()),
                    Expr::UnaryOp(
                        UnaryOp::Not,
                        Box::new(bin(BinaryOp::Eq, qty(), Expr::Literal(Value::Number(0.0)))),
                    ),
                )),
            ),
            (
                "unary negation of a group",
                Formula::new(Expr::UnaryOp(
                    UnaryOp::Neg,
                    Box::new(bin(BinaryOp::Add, price(), qty())),
                )),
            ),
            (
                "dimension-projected ref",
                Formula::new(bin(
                    BinaryOp::Mul,
                    Expr::Ref(
                        MeasureId(101),
                        DimensionSpec {
                            by: vec![t, p],
                            over: vec![],
                            except: vec![],
                        },
                    ),
                    price(),
                )),
            ),
            (
                "date literals",
                Formula::new(bin(
                    BinaryOp::Gt,
                    // Built through the parser so the test needs no chrono dep.
                    parser::parse_expr(&revenue_model(), "#2025-01-01#")
                        .expect("date literal parses")
                        .expr,
                    parser::parse_expr(&revenue_model(), "#2024-06-30T12:00:00Z#")
                        .expect("timestamp literal parses")
                        .expr,
                )),
            ),
            (
                "boolean and text literals",
                Formula::new(bin(
                    BinaryOp::Or,
                    Expr::Literal(Value::Boolean(true)),
                    bin(
                        BinaryOp::Eq,
                        Expr::Literal(Value::Text("hello world".into())),
                        Expr::Literal(Value::Text("x".into())),
                    ),
                )),
            ),
        ]
    }

    /// The invariant: the text the formula bar puts in its buffer commits
    /// successfully UNCHANGED, leaving the AST and the computed snapshot alone.
    #[test]
    fn displayed_formula_commits_unchanged() {
        for (label, formula) in displayable_formulas() {
            let mut model = revenue_model();
            model.measures.get_mut(&MeasureId(102)).unwrap().kind =
                MeasureKind::Derived(formula.clone());
            let mut app = build_app(model);
            select(&mut app, MeasureId(102));

            // Exactly what `formula_bar` seeds the buffer with.
            let buf = app
                .formula_source(MeasureId(102))
                .unwrap_or_else(|| panic!("{label}: nothing displayed"));
            assert!(!buf.is_empty(), "{label}: nothing displayed");

            let before = app.snapshot.clone();
            app.commit_formula(MeasureId(102), &buf)
                .unwrap_or_else(|e| panic!("{label}: displayed {buf:?} did not commit: {e}"));

            let after = match &app.model.measures[&MeasureId(102)].kind {
                MeasureKind::Derived(f) => f.clone(),
                MeasureKind::Input => panic!("{label}: measure stopped being derived"),
            };
            assert_eq!(
                after, formula,
                "{label}: AST changed by round-trip ({buf:?})"
            );
            assert!(app.engine.is_some(), "{label}: engine lost");
            assert_eq!(app.snapshot, before, "{label}: snapshot changed ({buf:?})");
        }
    }

    #[test]
    fn displayed_dsl_is_the_symbolic_language_not_prose() {
        // The plain revenue model's formula shows as DSL, minimally parenthesized.
        let app = build_app(revenue_model());
        assert_eq!(
            app.formula_source(MeasureId(102)).as_deref(),
            Some("Price * Quantity")
        );
        // Input measures have no formula text.
        assert_eq!(app.formula_source(MeasureId(100)), None);
        assert_eq!(app.formula_source(MeasureId(999)), None);
    }

    #[test]
    fn shapes_without_a_dsl_spelling_fall_back_to_cnl_and_still_commit() {
        // `SUM(x OVER Time AND Product)` has no v1 DSL spelling (one category
        // per OVER), so the bar shows controlled English — which commits.
        let mut model = revenue_model();
        let f = Formula::new(Expr::Call(
            FuncId(1),
            vec![Expr::Ref(
                MeasureId(101),
                DimensionSpec {
                    by: vec![],
                    over: vec![CategoryId(1), CategoryId(2)],
                    except: vec![],
                },
            )],
        ));
        model.measures.get_mut(&MeasureId(102)).unwrap().kind = MeasureKind::Derived(f.clone());
        let mut app = build_app(model);

        let buf = app.formula_source(MeasureId(102)).expect("displayable");
        assert_eq!(buf, "the sum of Quantity over Time and Product");
        app.commit_formula(MeasureId(102), &buf).expect("commits");
        match &app.model.measures[&MeasureId(102)].kind {
            MeasureKind::Derived(got) => assert_eq!(*got, f),
            MeasureKind::Input => panic!("stopped being derived"),
        }
    }

    #[test]
    fn dsl_printer_refuses_inexpressible_shapes() {
        let model = revenue_model();
        // A bare ref carrying `over` (no aggregation) and `except` have no
        // spelling; so does an unknown function id and an enum literal.
        let bare_over = Formula::new(Expr::Ref(
            MeasureId(100),
            DimensionSpec {
                by: vec![],
                over: vec![CategoryId(1)],
                except: vec![],
            },
        ));
        assert_eq!(formula_dsl(&model, &bare_over), None);
        let excepted = Formula::new(Expr::Ref(
            MeasureId(100),
            DimensionSpec {
                by: vec![],
                over: vec![],
                except: vec![CategoryId(1)],
            },
        ));
        assert_eq!(formula_dsl(&model, &excepted), None);
        let unknown_fn = Formula::new(Expr::Call(
            FuncId(4242),
            vec![Expr::Ref(MeasureId(100), DimensionSpec::default())],
        ));
        assert_eq!(formula_dsl(&model, &unknown_fn), None);
        assert_eq!(
            formula_dsl(&model, &Formula::new(Expr::Literal(Value::Enum(3)))),
            None
        );
        // A negative literal has no token either (only unary minus does).
        assert_eq!(
            formula_dsl(&model, &Formula::new(Expr::Literal(Value::Number(-1.0)))),
            None
        );
    }

    // -- BUG 1: failed saves and bad formulas ------------------------------

    #[test]
    fn cell_edit_and_formula_commit_report_save_failure() {
        // A store path under a directory that does not exist: every save fails.
        let db = std::env::temp_dir()
            .join(format!("improv_gui_no_such_dir_{}", std::process::id()))
            .join("model.db")
            .to_string_lossy()
            .into_owned();
        let mut app = build_app(revenue_model());
        app.db = db;

        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        let err = app
            .set_cell(MeasureId(101), key, Value::Number(9.0))
            .expect_err("unwritable store must fail the edit");
        assert!(err.starts_with("save failed:"), "got {err:?}");

        let err = app
            .commit_formula(MeasureId(102), "Price + Quantity")
            .expect_err("unwritable store must fail the commit");
        assert!(err.starts_with("save failed:"), "got {err:?}");

        let err = app
            .add_derived_measure("Margin", "Price - Quantity")
            .expect_err("unwritable store must fail the add");
        assert!(err.starts_with("save failed:"), "got {err:?}");

        // The view save reports the failure in the status line and rolls back.
        assert_eq!(app.save_view("L1"), None);
        assert!(app.status.starts_with("save failed:"), "{}", app.status);
        assert!(app.model.views.is_empty(), "view rolled back");

        // A failed formula commit did not publish the new formula, so the model
        // still holds (and computes) the original Revenue = Price * Quantity.
        assert_eq!(
            app.formula_source(MeasureId(102)).as_deref(),
            Some("Price * Quantity")
        );
        assert!(app.model.measure_by_name("Margin").is_none());
    }

    #[test]
    fn formula_that_fails_to_build_is_not_published() {
        let mut app = build_app(revenue_model());
        let before_model = app.model.clone();
        let before_snapshot = app.snapshot.clone();

        // Revenue = Revenue + Price parses fine but is a dependency cycle, so
        // the engine cannot build.
        let err = app
            .commit_formula(MeasureId(102), "Revenue + Price")
            .expect_err("a cyclic formula must be rejected");
        assert!(err.contains("cyclic"), "got {err:?}");

        assert_eq!(app.model, before_model, "model preserved");
        assert_eq!(app.snapshot, before_snapshot, "snapshot preserved");
        assert!(app.engine.is_some(), "engine preserved");
        assert_eq!(
            app.formula_source(MeasureId(102)).as_deref(),
            Some("Price * Quantity")
        );
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(app.values_for(MeasureId(102)).get(&key), Some(&70.0));

        // Same for a type error the parser accepts but the compiler rejects
        // (comparison result fed to arithmetic).
        let err = app
            .commit_formula(MeasureId(102), "(Price > Quantity) * Price")
            .expect_err("a type error must be rejected");
        assert!(!err.is_empty());
        assert_eq!(app.model, before_model, "model preserved");
        assert!(app.engine.is_some(), "engine preserved");

        // And for add_derived_measure.
        let err = app
            .add_derived_measure("Bad", "(Price > Quantity) * Price")
            .expect_err("a type error must be rejected");
        assert!(!err.is_empty());
        assert_eq!(app.model, before_model, "model preserved");
        assert!(app.engine.is_some(), "engine preserved");
    }

    #[test]
    fn inspector_data_is_correct() {
        let app = build_app(revenue_model());
        let d = app.inspector_data(MeasureId(102)).expect("data");
        assert_eq!(d.name, "Revenue");
        assert!(d.is_derived);
        assert_eq!(
            d.dimensions,
            vec!["Time".to_string(), "Product".to_string()]
        );
        let mut deps = d.dependencies.clone();
        deps.sort();
        assert_eq!(deps, vec!["Price".to_string(), "Quantity".to_string()]);
        assert!(d.formula_english.is_some());
        assert_eq!(d.error_cells, 0);

        // Input measure: no deps, no formula.
        let di = app.inspector_data(MeasureId(100)).expect("data");
        assert!(!di.is_derived);
        assert!(di.dependencies.is_empty());
        assert!(di.formula_english.is_none());
        assert_eq!(di.dimensions, vec!["Product".to_string()]);
    }

    /// Build an app directly from a model (bypassing the store) for tests: one
    /// matrix on the canvas, showing the default measure, as a fresh
    /// `ImprovApp::load` would.
    fn build_app(model: Model) -> ImprovApp {
        let (engine, snapshot) = build_engine(&model);
        let selected = pick_default_measure(&model);
        let matrices = vec![Matrix::new(&model, selected)];
        ImprovApp {
            db: String::new(),
            model,
            engine,
            snapshot,
            matrices,
            focus: 0,
            current_view: None,
            status: String::new(),
            formula_buf: String::new(),
            formula_for: None,
            formula_error_pos: None,
            formula_error_msg: String::new(),
            new_name: String::new(),
            new_formula: String::new(),
            view_name: String::new(),
            show_chart: false,
            chart_line: false,
            show_csv_wizard: false,
            import_form: ImportForm::default(),
            export_form: ExportForm::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            formula_row_rects: Vec::new(),
        }
    }

    /// Point the app's FOCUSED matrix at `measure` (what tests used to do by
    /// assigning `app.selected`). Kept as a helper so the many tests that select
    /// a measure read the same as before.
    fn select(app: &mut ImprovApp, measure: MeasureId) {
        app.focused_mut().select(Some(measure));
    }

    // A 3-D input measure Sales[Time, Product, Region] for paging tests
    // (mirrors the TUI's paging fixture).
    fn sales_3d_model() -> Model {
        let mut m = Model::new();
        let (time, product, region) = (CategoryId(1), CategoryId(2), CategoryId(3));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_category(region, "Region");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(30), region, "North");
        m.add_item(ItemId(31), region, "South");
        m.add_measure(Measure {
            id: MeasureId(200),
            name: Name("Sales".into()),
            value_type: ValueType::Number,
            categories: vec![time, product, region],
            kind: MeasureKind::Input,
            description: None,
        });
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
        m.set_input(
            MeasureId(200),
            c(&[
                (time, ItemId(10)),
                (product, ItemId(20)),
                (region, ItemId(30)),
            ]),
            Value::Number(100.0),
        );
        m.set_input(
            MeasureId(200),
            c(&[
                (time, ItemId(10)),
                (product, ItemId(20)),
                (region, ItemId(31)),
            ]),
            Value::Number(250.0),
        );
        m
    }

    #[test]
    fn set_axis_moves_category_from_columns_to_rows() {
        // Quantity[Time, Product]: natural rows=Time(1), cols=Product(2).
        let mut app = build_app(revenue_model());
        select(&mut app, MeasureId(101));
        app.sync_axis_state();
        let (r, c, _) = app.resolved_axes();
        assert_eq!(r, Some(CategoryId(1))); // Time on rows
        assert_eq!(c, Some(CategoryId(2))); // Product on cols

        // Dragging Product onto Rows STACKS it under Time (both on rows), which
        // is the Improv semantics; columns become empty.
        app.set_axis(CategoryId(2), Axis::Rows);
        assert_eq!(app.row_cats(), vec![CategoryId(1), CategoryId(2)]);
        assert!(app.col_cats().is_empty());
        let (r, c, _) = app.resolved_axes();
        assert_eq!(r, Some(CategoryId(1)), "primary row is still Time");
        assert_eq!(c, None, "no column category after stacking both on rows");
    }

    #[test]
    fn stacked_rows_form_cartesian_product_with_correct_keys() {
        // Quantity[Time, Product] with BOTH categories stacked on rows.
        let mut app = build_app(revenue_model());
        select(&mut app, MeasureId(101));
        app.sync_axis_state();
        app.set_axis(CategoryId(2), Axis::Rows); // rows = [Time, Product]
        assert_eq!(app.row_cats(), vec![CategoryId(1), CategoryId(2)]);

        // The row axis is the product of Time items x Product items.
        let tuples = app.axis_tuples(&app.row_cats());
        // revenue_model has Time={2025(10)}, Product={WidgetA(20)} (1x1) here,
        // so a single tuple binding both categories.
        assert_eq!(tuples.len(), 1);
        assert_eq!(tuples[0].len(), 2, "tuple binds both stacked categories");
        // The cell key for that row (no columns) binds Time AND Product, sorted.
        let key = cell_key_multi(&app.row_cats(), &tuples[0], &[], &[], &[]);
        assert_eq!(key, vec![(1, 10), (2, 20)]);
        // Grid dims: 1 row line, 1 col line (no col categories).
        assert_eq!(app.grid_dims(), (1, 1));
    }

    #[test]
    fn nth_tuple_decodes_mixed_radix_and_matches_axis_tuples() {
        // Row cats Time(4 items) x Region(2 items): 8 lines, inner (Region)
        // varies fastest — same order as axis_tuples.
        let time: Vec<(ItemId, String)> =
            (0..4).map(|k| (ItemId(10 + k), format!("T{k}"))).collect();
        let region: Vec<(ItemId, String)> =
            (0..2).map(|k| (ItemId(30 + k), format!("R{k}"))).collect();
        let lists = vec![time.clone(), region.clone()];
        assert_eq!(product_len(&lists), 8);

        // line 0 = (T0, R0); line 1 = (T0, R1); line 7 = (T3, R1).
        assert_eq!(
            nth_tuple(&lists, 0),
            vec![time[0].clone(), region[0].clone()]
        );
        assert_eq!(
            nth_tuple(&lists, 1),
            vec![time[0].clone(), region[1].clone()]
        );
        assert_eq!(
            nth_tuple(&lists, 7),
            vec![time[3].clone(), region[1].clone()]
        );

        // Iterating 0..total reproduces the full Cartesian product built by the
        // reference product-builder (same ordering as axis_tuples).
        let mut reference: Vec<Vec<(ItemId, String)>> = vec![Vec::new()];
        for list in &lists {
            let mut next = Vec::new();
            for prefix in &reference {
                for it in list {
                    let mut t = prefix.clone();
                    t.push(it.clone());
                    next.push(t);
                }
            }
            reference = next;
        }
        let decoded: Vec<_> = (0..product_len(&lists))
            .map(|i| nth_tuple(&lists, i))
            .collect();
        assert_eq!(decoded, reference);
    }

    #[test]
    fn nth_tuple_matches_apps_axis_tuples() {
        // Cross-check against the app's own axis_tuples on the 2x2 model.
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        app.set_axis(CategoryId(2), Axis::Rows); // rows = [Time, Product] stacked
        let cats = app.row_cats();
        let lists = app.axis_item_lists(&cats);
        let want = app.axis_tuples(&cats);
        assert_eq!(product_len(&lists), want.len());
        for (i, w) in want.iter().enumerate() {
            assert_eq!(nth_tuple(&lists, i), *w, "line {i}");
        }
    }

    #[test]
    fn large_grid_row_count_is_correct_and_does_not_panic() {
        // A synthetic model with a category of a few thousand items so the row
        // product is large; the pure helpers must give the right count and
        // decode any line without materializing the whole product.
        let mut m = Model::new();
        let (big, small) = (CategoryId(1), CategoryId(2));
        m.add_category(big, "Big");
        m.add_category(small, "Small");
        let n_big = 5_000u32;
        for k in 0..n_big {
            m.add_item(ItemId(1_000 + k), big, format!("b{k}"));
        }
        for k in 0..3u32 {
            m.add_item(ItemId(10 + k), small, format!("s{k}"));
        }
        m.add_measure(Measure {
            id: MeasureId(300),
            name: Name("M".into()),
            value_type: ValueType::Number,
            categories: vec![big, small],
            kind: MeasureKind::Input,
            description: None,
        });
        let mut app = build_app(m);
        select(&mut app, MeasureId(300));
        app.sync_axis_state();
        // Stack both categories on rows -> 5000 * 3 = 15000 row lines.
        app.set_axis(small, Axis::Rows);
        assert_eq!(app.row_cats(), vec![big, small]);

        let lists = app.axis_item_lists(&app.row_cats());
        let total = product_len(&lists);
        assert_eq!(total, (n_big as usize) * 3);
        // grid_dims reports the same total (no column category -> 1 col).
        assert_eq!(app.grid_dims(), (total, 1));

        // Decode the first, a middle, and the last line without panic.
        let first = nth_tuple(&lists, 0);
        assert_eq!(first[0].0, ItemId(1_000));
        assert_eq!(first[1].0, ItemId(10));
        let last = nth_tuple(&lists, total - 1);
        assert_eq!(last[0].0, ItemId(1_000 + n_big - 1));
        assert_eq!(last[1].0, ItemId(12));
        // A cursor deep in the grid resolves its key without materializing rows.
        app.focused_mut().cursor_row = total - 1;
        app.focused_mut().cursor_col = 0;
        let key = app.cursor_key().expect("cursor addresses a cell");
        let mut want = vec![(big.0, 1_000 + n_big - 1), (small.0, 12)];
        want.sort();
        assert_eq!(key, want);
    }

    #[test]
    fn pivot_rotate_swaps_axes_and_back() {
        let mut app = build_app(revenue_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        let (r0, c0, _) = app.resolved_axes();
        assert_eq!((r0, c0), (Some(CategoryId(1)), Some(CategoryId(2))));

        app.pivot_rotate();
        let (r1, c1, _) = app.resolved_axes();
        assert_eq!((r1, c1), (Some(CategoryId(2)), Some(CategoryId(1))));

        app.pivot_rotate(); // back to start for a 2-D measure
        let (r2, c2, _) = app.resolved_axes();
        assert_eq!((r2, c2), (Some(CategoryId(1)), Some(CategoryId(2))));
    }

    #[test]
    fn set_page_changes_pinned_item_and_cell_value() {
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        // One page dim (Region), pinned to North (index 0) by default.
        let (_, _, pinned) = app.resolved_axes();
        assert_eq!(pinned, vec![(CategoryId(3), ItemId(30))]); // North

        // Cell [2025, WidgetA, North] = 100.
        let vals = app.values_for(MeasureId(200));
        let mut north = vec![(1u32, 10u32), (2, 20), (3, 30)];
        north.sort();
        assert_eq!(vals.get(&north), Some(&100.0));

        // Page to South (index 1): pinned item and the visible value change.
        app.set_page(0, 1);
        let (_, _, pinned) = app.resolved_axes();
        assert_eq!(pinned, vec![(CategoryId(3), ItemId(31))]); // South
        let mut south = vec![(1u32, 10u32), (2, 20), (3, 31)];
        south.sort();
        assert_eq!(vals.get(&south), Some(&250.0));
    }

    #[test]
    fn switching_measure_resets_axis_order() {
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        // Pivot away from natural order.
        app.pivot_rotate();
        assert_ne!(
            app.focused().axis_order,
            vec![CategoryId(1), CategoryId(2), CategoryId(3)]
        );

        // Add a 1-D measure and select it: axis order resets to its natural order.
        app.model.add_measure(Measure {
            id: MeasureId(201),
            name: Name("Tax".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(2)],
            kind: MeasureKind::Input,
            description: None,
        });
        select(&mut app, MeasureId(201));
        app.sync_axis_state();
        assert_eq!(app.focused().axis_order, vec![CategoryId(2)]);
        assert!(app.focused().page_idx.is_empty());

        // Back to Sales: natural order restored (not the pivoted one).
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        assert_eq!(
            app.focused().axis_order,
            vec![CategoryId(1), CategoryId(2), CategoryId(3)]
        );
    }

    /// A 2x2 input measure Quantity[Time(2025,2026), Product(WidgetA,WidgetB)]
    /// plus Revenue = Price * Quantity, for cursor navigation/edit tests.
    fn grid_2x2_model() -> Model {
        let mut m = Model::new();
        let (t, p) = (CategoryId(1), CategoryId(2));
        m.add_category(t, "Time");
        m.add_category(p, "Product");
        m.add_item(ItemId(10), t, "2025");
        m.add_item(ItemId(11), t, "2026");
        m.add_item(ItemId(20), p, "WidgetA");
        m.add_item(ItemId(21), p, "WidgetB");
        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
        m.set_input(MeasureId(100), c(&[(p, ItemId(20))]), Value::Number(10.0));
        m.set_input(MeasureId(100), c(&[(p, ItemId(21))]), Value::Number(20.0));
        for (ti, pi, q) in [
            (ItemId(10), ItemId(20), 100.0),
            (ItemId(10), ItemId(21), 50.0),
            (ItemId(11), ItemId(20), 120.0),
            (ItemId(11), ItemId(21), 80.0),
        ] {
            m.set_input(MeasureId(101), c(&[(t, ti), (p, pi)]), Value::Number(q));
        }
        m
    }

    #[test]
    fn cursor_clamps_at_all_edges_and_after_pivot_shrink() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product], 2x2
        app.sync_axis_state();

        // Off the top-left: clamps to (0, 0).
        app.move_cursor(-5, -5);
        assert_eq!((app.focused().cursor_row, app.focused().cursor_col), (0, 0));
        // Off the bottom-right: clamps to (1, 1).
        app.move_cursor(100, 100);
        assert_eq!((app.focused().cursor_row, app.focused().cursor_col), (1, 1));

        // Switch to Price[Product]: 2 rows, 1 synthetic col -> column re-clamps.
        select(&mut app, MeasureId(100));
        app.sync_axis_state(); // resets cursor to (0,0) on measure switch
        app.move_cursor(5, 5);
        assert_eq!(
            app.focused().cursor_col,
            0,
            "single-column grid clamps col to 0"
        );
        assert_eq!(app.focused().cursor_row, 1, "two rows -> max row 1");

        // Sales 3-D: put the cursor at the far corner, then pivot to a shape
        // where the cursor would be out of range; clamp_cursor must fix it.
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        // Region on cols has 2 items; move to the far cell.
        app.pivot_rotate(); // rows=Product, cols=Region (2 cols)
        app.move_cursor(100, 100);
        let (r, c) = (app.focused().cursor_row, app.focused().cursor_col);
        let (rows, cols) = app.grid_dims();
        assert!(r < rows && c < cols, "cursor within {rows}x{cols}");
        // Pivot again (rows=Region -> single-item axes elsewhere) and confirm
        // the cursor never goes out of range.
        app.pivot_rotate();
        let (rows, cols) = app.grid_dims();
        assert!(app.focused().cursor_row < rows && app.focused().cursor_col < cols);
    }

    #[test]
    fn cursor_maps_to_expected_coord_key() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        // [1,1] = Quantity[2026, WidgetB] = Time(11), Product(21).
        app.focused_mut().cursor_row = 1;
        app.focused_mut().cursor_col = 1;
        let mut expect = vec![(1u32, 11u32), (2, 21)];
        expect.sort();
        assert_eq!(app.cursor_key(), Some(expect));

        // [0,1] = Quantity[2025, WidgetB] = Time(10), Product(21).
        app.focused_mut().cursor_row = 0;
        app.focused_mut().cursor_col = 1;
        let mut expect = vec![(1u32, 10u32), (2, 21)];
        expect.sort();
        assert_eq!(app.cursor_key(), Some(expect));
    }

    #[test]
    fn move_then_edit_routes_through_set_cell_and_recomputes() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        assert!(app.cursor_is_editable());

        // Move to Quantity[2025, WidgetA] = [0,0], set it to 200.
        app.focused_mut().cursor_row = 0;
        app.focused_mut().cursor_col = 0;
        let key = app.cursor_key().expect("cursor addresses a cell");
        app.set_cell(MeasureId(101), key, Value::Number(200.0))
            .unwrap();

        // Revenue[2025, WidgetA] = Price(10) * 200 = 2000 in the snapshot.
        let mut rkey = vec![(1u32, 10u32), (2, 20)];
        rkey.sort();
        let rev = app.values_for(MeasureId(102));
        assert_eq!(rev.get(&rkey), Some(&2000.0));

        // Derived measure: cursor cell is not editable (status set, not enter).
        select(&mut app, MeasureId(102));
        app.sync_axis_state();
        assert!(!app.cursor_is_editable());
        app.begin_edit_cursor();
        assert!(app.focused().editing.is_none());
        assert_eq!(app.status, "derived cells are computed, not editable");
    }

    #[test]
    fn filter_hides_an_item_from_the_rendered_rows() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        // Rows = Time (2025, 2026); hide 2026 (ItemId 11).
        let (row_cat, _, _) = app.resolved_axes();
        assert_eq!(row_cat, Some(CategoryId(1)));
        assert_eq!(app.sorted_items(CategoryId(1)).len(), 2);
        app.toggle_filter_item(CategoryId(1), ItemId(11));
        let rows: Vec<ItemId> = app
            .sorted_items(CategoryId(1))
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(rows, vec![ItemId(10)], "2026 filtered out of the row axis");
        // Columns (unfiltered Product) still show both.
        assert_eq!(app.sorted_items(CategoryId(2)).len(), 2);
        // Re-showing 2026 drops the filter (all items kept).
        app.toggle_filter_item(CategoryId(1), ItemId(11));
        assert!(app.focused().filters.is_empty());
        assert_eq!(app.sorted_items(CategoryId(1)).len(), 2);
    }

    /// Filter `cat` down to zero visible items (the state the audit found
    /// crashing: `nth_tuple`'s `i % 0` divide-by-zero in release builds).
    fn hide_all_items(app: &mut ImprovApp, cat: CategoryId) {
        let items: Vec<ItemId> = app
            .model
            .categories
            .get(&cat)
            .map(|c| c.items.clone())
            .unwrap_or_default();
        for it in items {
            app.toggle_filter_item(cat, it);
        }
        assert!(
            app.sorted_items(cat).is_empty(),
            "category {cat:?} should be filtered to nothing"
        );
    }

    #[test]
    fn row_category_filtered_to_empty_renders_zero_lines_not_one() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state(); // rows=Time, cols=Product
        assert_eq!(app.grid_dims(), (2, 2));

        hide_all_items(&mut app, CategoryId(1)); // Time -> nothing on rows
        let (rows, cols) = app.grid_dims();
        assert_eq!(rows, 0, "an empty row category must render ZERO row lines");
        assert_eq!(cols, 2, "the column axis is untouched");
        // No cell is addressable, so nothing is editable and no under-specified
        // key can be produced.
        assert_eq!(app.cursor_key(), None);
        assert!(!app.cursor_is_editable());
        app.begin_edit_cursor();
        assert!(
            app.focused().editing.is_none(),
            "no edit may start on a missing cell"
        );

        // Showing one item back restores exactly one line, with a complete key.
        app.toggle_filter_item(CategoryId(1), ItemId(10));
        assert_eq!(app.grid_dims(), (1, 2));
        let key = app.cursor_key().expect("cell exists again");
        assert_eq!(key.len(), 2, "key binds both Time and Product");
    }

    #[test]
    fn column_category_filtered_to_empty_renders_zero_lines_not_one() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101));
        app.sync_axis_state(); // rows=Time, cols=Product
        hide_all_items(&mut app, CategoryId(2)); // Product -> nothing on cols
        let (rows, cols) = app.grid_dims();
        assert_eq!(cols, 0, "an empty column category renders ZERO columns");
        assert_eq!(rows, 2, "the row axis is untouched");
        assert_eq!(app.cursor_key(), None);
        assert!(!app.cursor_is_editable());
        app.begin_edit_cursor();
        assert!(app.focused().editing.is_none());
    }

    #[test]
    fn page_category_filtered_to_empty_leaves_no_addressable_cell() {
        // Sales[Time, Product, Region]: Region is the page dimension. With no
        // Region item pinnable, every key would omit Region — under-specified.
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        assert_eq!(app.page_cats(), vec![CategoryId(3)]);
        let full = app.cursor_key().expect("a cell before filtering");
        assert_eq!(full.len(), 3, "key binds Time, Product AND Region");

        hide_all_items(&mut app, CategoryId(3)); // Region -> nothing pinnable
        assert_eq!(
            app.grid_dims(),
            (0, 0),
            "no page item pinnable -> nothing to render"
        );
        assert_eq!(
            app.cursor_key(),
            None,
            "must not yield a Region-less (under-specified) key"
        );
        assert!(!app.cursor_is_editable());
        app.begin_edit_cursor();
        assert!(app.focused().editing.is_none());
        assert!(app.status.contains("filtered"), "status: {}", app.status);
    }

    #[test]
    fn stacked_axis_with_one_empty_category_renders_zero_lines() {
        // Both categories stacked on rows; emptying the INNER one zeroes the
        // whole product (the mixed-radix decode has no valid line).
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101));
        app.sync_axis_state();
        app.set_axis(CategoryId(2), Axis::Rows); // rows = [Time, Product]
        assert_eq!(app.grid_dims(), (4, 1));
        hide_all_items(&mut app, CategoryId(2));
        assert_eq!(app.grid_dims(), (0, 1));
        assert_eq!(app.cursor_key(), None);
    }

    #[test]
    fn scalar_axis_with_no_categories_still_has_one_line() {
        // The distinction the fix turns on: `product_len(&[]) == 1` (an axis
        // with NO categories is scalar in that direction — one legitimate line)
        // vs a category present but filtered to zero items (no lines at all).
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(100)); // Price[Product] only
        app.sync_axis_state();
        assert_eq!(app.col_cats(), vec![], "no column category: scalar axis");
        assert_eq!(
            app.grid_dims(),
            (2, 1),
            "2 Product rows x 1 scalar column line"
        );
        // That single scalar line has a complete key (Price's only dimension).
        let key = app.cursor_key().expect("scalar column line has a cell");
        assert_eq!(key, vec![(2u32, 20u32)]);
        assert!(app.cursor_is_editable());

        // Same model, but now the ROW category is emptied: 0 rows, while the
        // scalar column axis stays at 1.
        hide_all_items(&mut app, CategoryId(2));
        assert_eq!(app.grid_dims(), (0, 1));
        assert_eq!(app.cursor_key(), None);
    }

    #[test]
    fn cursor_never_decodes_a_line_of_an_empty_axis() {
        // A stale cursor (left over from before the filter) must not reach
        // `nth_tuple` with a zero radix: that is `i % 0`, a release-mode crash.
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101));
        app.sync_axis_state();
        app.move_cursor(10, 10); // bottom-right of the 2x2 grid
        assert_eq!((app.focused().cursor_row, app.focused().cursor_col), (1, 1));
        hide_all_items(&mut app, CategoryId(1));
        // clamp_cursor ran via toggle_filter_item: the row axis has no lines, so
        // the row index collapses to 0 (the column axis is unaffected).
        assert_eq!(app.focused().cursor_row, 0);
        assert_eq!(app.cursor_key(), None);
        // Even a forced out-of-range cursor resolves to "no cell", not a panic.
        app.focused_mut().cursor_row = 7;
        app.focused_mut().cursor_col = 7;
        assert_eq!(app.cursor_key(), None);
        app.move_cursor(1, 1);
        assert_eq!(
            app.focused().cursor_row,
            0,
            "an empty axis cannot be moved into"
        );
    }

    #[test]
    fn save_view_captures_measure_axis_order_and_filters() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();
        app.pivot_rotate(); // rows=Product, cols=Time
        app.toggle_filter_item(CategoryId(2), ItemId(21)); // hide WidgetB

        let id = app.save_view("L1").expect("saved");
        let v = app.model.views.get(&id).expect("view stored");
        assert_eq!(v.measure, MeasureId(101));
        assert_eq!(v.axis_order, vec![CategoryId(2), CategoryId(1)]);
        let f = v
            .filters
            .iter()
            .find(|f| f.category == CategoryId(2))
            .unwrap();
        assert_eq!(f.items, vec![ItemId(20)]); // only WidgetA kept
        assert_eq!(app.model.view_by_name("L1").map(|v| v.id), Some(id));
    }

    #[test]
    fn apply_view_restores_axis_order_and_filters() {
        // Build a view from one app, apply it to a fresh app -> layout matches.
        let mut src = build_app(grid_2x2_model());
        select(&mut src, MeasureId(101));
        src.sync_axis_state();
        src.pivot_rotate(); // rows=Product, cols=Time
        src.toggle_filter_item(CategoryId(2), ItemId(21)); // hide WidgetB
        let v = src.build_view(ViewId(1), "L1").expect("view");

        let mut dst = build_app(grid_2x2_model());
        // dst starts on Revenue (derived) with natural axes and no filters.
        assert_eq!(dst.selected(), Some(MeasureId(102)));
        dst.apply_view(&v);
        dst.sync_axis_state(); // must not clobber the applied layout

        assert_eq!(dst.selected(), Some(MeasureId(101)));
        assert_eq!(dst.focused().axis_order, vec![CategoryId(2), CategoryId(1)]);
        assert_eq!(dst.focused().filters, v.filters);
        let (row_cat, col_cat, _) = dst.resolved_axes();
        assert_eq!(row_cat, Some(CategoryId(2))); // Product on rows
        assert_eq!(col_cat, Some(CategoryId(1))); // Time on cols
                                                  // Filter reflected: only WidgetA on the (row) Product axis.
        let rows: Vec<ItemId> = dst
            .sorted_items(CategoryId(2))
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(rows, vec![ItemId(20)]);
    }

    #[test]
    fn apply_view_restores_page_item() {
        let mut src = build_app(sales_3d_model());
        select(&mut src, MeasureId(200));
        src.sync_axis_state();
        // Page Region (dim 0) to South (index 1).
        src.set_page(0, 1);
        let (_, _, pinned) = src.resolved_axes();
        assert_eq!(pinned, vec![(CategoryId(3), ItemId(31))]); // South
        let v = src.build_view(ViewId(1), "south").expect("view");
        assert_eq!(v.page_items, vec![(CategoryId(3), ItemId(31))]);

        let mut dst = build_app(sales_3d_model());
        select(&mut dst, MeasureId(200));
        dst.sync_axis_state();
        let (_, _, pinned) = dst.resolved_axes();
        assert_eq!(pinned, vec![(CategoryId(3), ItemId(30))]); // North default
        dst.apply_view(&v);
        let (_, _, pinned) = dst.resolved_axes();
        assert_eq!(
            pinned,
            vec![(CategoryId(3), ItemId(31))],
            "page item restored"
        );
    }

    // -- chart_series (pure; no egui) --------------------------------------

    #[test]
    fn chart_series_yields_labels_and_oracle_values() {
        // Revenue[Time, Product], natural axes: rows=Time -> x, cols=Product ->
        // one series each. Oracle: WidgetA = 1000/1200, WidgetB = 1000/1600.
        let app = build_app(grid_2x2_model());
        assert_eq!(app.selected(), Some(MeasureId(102))); // Revenue selected
        let d = app.chart_series();
        assert_eq!(d.x_title, "Time");
        assert_eq!(d.x_labels, vec!["2025".to_string(), "2026".to_string()]);
        assert_eq!(d.series.len(), 2);
        assert_eq!(d.series[0].name, "WidgetA");
        assert_eq!(d.series[0].points, vec![Some(1000.0), Some(1200.0)]);
        assert_eq!(d.series[1].name, "WidgetB");
        assert_eq!(d.series[1].points, vec![Some(1000.0), Some(1600.0)]);
        // y-range spans 0..1600 (0 always included).
        assert_eq!(d.y_range(), (0.0, 1600.0));
    }

    #[test]
    fn chart_series_stacked_rows_join_tuple_labels_and_match_cell_keys() {
        // Revenue[Time, Product]: stack BOTH categories on rows, leaving no
        // column category. x-labels become the joined row tuples
        // ("2025 / WidgetA", ...) and there is one unnamed series whose values
        // match cell_key_multi lookups.
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(102)); // Revenue
        app.sync_axis_state();
        app.set_axis(CategoryId(2), Axis::Rows); // rows = [Time, Product]
        assert_eq!(app.row_cats(), vec![CategoryId(1), CategoryId(2)]);
        assert!(app.col_cats().is_empty());

        let d = app.chart_series();
        assert_eq!(d.x_title, "Time / Product");
        assert_eq!(
            d.x_labels,
            vec![
                "2025 / WidgetA".to_string(),
                "2025 / WidgetB".to_string(),
                "2026 / WidgetA".to_string(),
                "2026 / WidgetB".to_string(),
            ]
        );
        // One series (no column categories), unnamed.
        assert_eq!(d.series.len(), 1);
        assert_eq!(d.series[0].name, "");

        // Each point matches a direct cell_key_multi lookup on the same tuples.
        let values = app.values_for(MeasureId(102));
        let row_tuples = app.axis_tuples(&app.row_cats());
        let want: Vec<Option<f64>> = row_tuples
            .iter()
            .map(|t| {
                let key = cell_key_multi(&app.row_cats(), t, &[], &[], &[]);
                values.get(&key).copied()
            })
            .collect();
        assert_eq!(d.series[0].points, want);
        // Oracle: WidgetA prices 10/20; Quantities 100,50,120,80 ->
        // 1000, 1000, 1200, 1600.
        assert_eq!(
            d.series[0].points,
            vec![Some(1000.0), Some(1000.0), Some(1200.0), Some(1600.0)]
        );
    }

    #[test]
    fn chart_series_stacked_columns_form_one_series_per_col_tuple() {
        // Revenue[Time, Product]: rows=Time, columns stack both? Instead stack
        // Product on rows AND keep Time on cols to exercise a multi-tuple
        // column axis. Here: rows=Product (2), cols=Time (2) -> 2 x-labels,
        // 2 series named by the (single-item) column tuples.
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(102));
        app.sync_axis_state();
        app.pivot_rotate(); // rows=Product, cols=Time
        assert_eq!(app.row_cats(), vec![CategoryId(2)]);
        assert_eq!(app.col_cats(), vec![CategoryId(1)]);

        let d = app.chart_series();
        assert_eq!(d.x_title, "Product");
        assert_eq!(
            d.x_labels,
            vec!["WidgetA".to_string(), "WidgetB".to_string()]
        );
        // One series per Time item (single-element column tuples).
        assert_eq!(d.series.len(), 2);
        assert_eq!(d.series[0].name, "2025");
        assert_eq!(d.series[1].name, "2026");
        // 2025: WidgetA=1000, WidgetB=1000; 2026: WidgetA=1200, WidgetB=1600.
        assert_eq!(d.series[0].points, vec![Some(1000.0), Some(1000.0)]);
        assert_eq!(d.series[1].points, vec![Some(1200.0), Some(1600.0)]);
    }

    #[test]
    fn chart_series_1d_single_series() {
        // Price[Product]: 1-D grid -> a single unnamed series over Product.
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(100)); // Price[Product]
        app.sync_axis_state();
        let d = app.chart_series();
        assert_eq!(d.x_title, "Product");
        assert_eq!(
            d.x_labels,
            vec!["WidgetA".to_string(), "WidgetB".to_string()]
        );
        assert_eq!(d.series.len(), 1);
        assert_eq!(d.series[0].name, "");
        assert_eq!(d.series[0].points, vec![Some(10.0), Some(20.0)]);
    }

    #[test]
    fn chart_filter_removes_a_bar() {
        // Hide 2026 on the Time (x) axis: its label and points drop out.
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(102));
        app.sync_axis_state();
        app.toggle_filter_item(CategoryId(1), ItemId(11)); // hide 2026
        let d = app.chart_series();
        assert_eq!(d.x_labels, vec!["2025".to_string()], "2026 filtered out");
        assert_eq!(d.series[0].points, vec![Some(1000.0)]); // WidgetA
        assert_eq!(d.series[1].points, vec![Some(1000.0)]); // WidgetB
    }

    #[test]
    fn chart_non_numeric_cell_is_a_gap_not_a_panic() {
        // Overwrite one Revenue-input cell so a derived cell errors, and set a
        // Text input on another measure: both surface as gaps, not panics.
        let mut app = build_app(grid_2x2_model());
        // Make Quantity[2026, WidgetA] a Text value -> Revenue[2026, WidgetA]
        // becomes an Error (type mismatch), which values_for skips (gap).
        let c = |pairs: &[(CategoryId, ItemId)]| {
            improv_core_model::Coordinate::from_pairs(pairs.iter().copied())
        };
        app.model.set_input(
            MeasureId(101),
            c(&[(CategoryId(1), ItemId(11)), (CategoryId(2), ItemId(20))]),
            Value::Text("oops".into()),
        );
        app.rebuild_engine();
        select(&mut app, MeasureId(102));
        app.sync_axis_state();
        let d = app.chart_series(); // must not panic
                                    // WidgetA series: 2025 numeric, 2026 is a gap (None).
        assert_eq!(d.series[0].name, "WidgetA");
        assert_eq!(d.series[0].points, vec![Some(1000.0), None]);
        // WidgetB series unaffected.
        assert_eq!(d.series[1].points, vec![Some(1000.0), Some(1600.0)]);
        // y-range still valid, 0 included.
        let (lo, hi) = d.y_range();
        assert_eq!(lo, 0.0);
        assert!(hi >= 1600.0);
    }

    // -- regression: the displayed-text-commits-unchanged invariant ---------

    /// Every aggregation arg shape, systematically: `over` of length 0/1/2/3,
    /// crossed with an empty/non-empty `except` and a `by` list, plus non-`Ref`
    /// args (a literal, a binary op, a nested call) — for each aggregating func
    /// id. Whatever `formula_source` shows as editable MUST re-commit unchanged;
    /// what it cannot spell must be `None`, never unparsable text.
    ///
    /// (`proptest` is not a dev-dependency of this crate, so this is the
    /// systematic table over the arg space rather than a generator.)
    fn aggregation_arg_space() -> Vec<(String, Formula)> {
        let cats = [CategoryId(1), CategoryId(2), CategoryId(3)];
        let mut out = Vec::new();
        for func in [
            parser::FUNC_SUM,
            parser::FUNC_AVG,
            parser::FUNC_MIN,
            parser::FUNC_MAX,
        ] {
            // Ref args: every (over, except, by) subset combination.
            for over_n in 0..=3usize {
                for except_n in 0..=2usize {
                    for by_n in 0..=2usize {
                        let spec = DimensionSpec {
                            by: cats[..by_n].to_vec(),
                            over: cats[..over_n].to_vec(),
                            except: cats[..except_n].to_vec(),
                        };
                        out.push((
                            format!(
                                "f{}(Quantity over={over_n} except={except_n} by={by_n})",
                                func.0
                            ),
                            Formula::new(Expr::Call(func, vec![Expr::Ref(MeasureId(101), spec)])),
                        ));
                    }
                }
            }
            // Non-`Ref` args: the parser's aggregation rule accepts none of them.
            let price = || Expr::Ref(MeasureId(100), DimensionSpec::default());
            let qty = || Expr::Ref(MeasureId(101), DimensionSpec::default());
            for (what, arg) in [
                ("literal", Expr::Literal(Value::Number(1.0))),
                (
                    "product",
                    Expr::BinaryOp(BinaryOp::Mul, Box::new(price()), Box::new(qty())),
                ),
                ("negated ref", Expr::UnaryOp(UnaryOp::Neg, Box::new(qty()))),
                ("scalar call", Expr::Call(FuncId(10), vec![qty()])),
                (
                    "nested agg",
                    Expr::Call(
                        parser::FUNC_SUM,
                        vec![Expr::Ref(
                            MeasureId(101),
                            DimensionSpec {
                                by: vec![],
                                over: vec![CategoryId(1)],
                                except: vec![],
                            },
                        )],
                    ),
                ),
                ("no args", Expr::Literal(Value::Boolean(true))),
            ] {
                out.push((
                    format!("f{}({what})", func.0),
                    Formula::new(Expr::Call(func, vec![arg])),
                ));
            }
            // Zero args and two args (arity the aggregation rule cannot spell).
            out.push((
                format!("f{}() no args at all", func.0),
                Formula::new(Expr::Call(func, vec![])),
            ));
            out.push((
                format!("f{}(two refs)", func.0),
                Formula::new(Expr::Call(
                    func,
                    vec![
                        Expr::Ref(MeasureId(100), DimensionSpec::default()),
                        Expr::Ref(MeasureId(101), DimensionSpec::default()),
                    ],
                )),
            ));
            // The aggregation inside a larger expression (the printer recurses
            // through `child_dsl`, which must not leak unparsable text either).
            out.push((
                format!("f{}(Quantity) * Price", func.0),
                Formula::new(Expr::BinaryOp(
                    BinaryOp::Mul,
                    Box::new(Expr::Call(
                        func,
                        vec![Expr::Ref(MeasureId(101), DimensionSpec::default())],
                    )),
                    Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                )),
            ));
        }
        out
    }

    /// DEFECT 1: an aggregation whose arg is not `Ref`-with-exactly-one-`over`
    /// has no DSL spelling. `formula_dsl` must return `None` (so the CNL
    /// fallback runs) instead of printing `SUM(Quantity)`, which
    /// `parse_expr` rejects ("expected 'OVER' in aggregation").
    #[test]
    fn every_aggregation_arg_shape_displays_only_committable_text() {
        let mut failures: Vec<String> = Vec::new();
        for (label, formula) in aggregation_arg_space() {
            let mut model = revenue_model();
            model.add_category(CategoryId(3), "Region");
            model.add_item(ItemId(30), CategoryId(3), "North");
            model.measures.get_mut(&MeasureId(102)).unwrap().kind =
                MeasureKind::Derived(formula.clone());
            let mut app = build_app(model);
            select(&mut app, MeasureId(102));

            // Whatever the DSL printer emits must parse as the same AST.
            if let Some(dsl) = formula_dsl(&app.model, &formula) {
                match parser::parse_expr(&app.model, &dsl) {
                    Ok(back) if back == formula => {}
                    Ok(back) => failures.push(format!(
                        "[{label}] DSL {dsl:?} reparsed to a DIFFERENT ast: {:?}",
                        back.expr
                    )),
                    Err(e) => failures.push(format!("[{label}] DSL {dsl:?} DOES NOT PARSE: {e}")),
                }
            }
            // And whatever the bar shows as editable must re-commit unchanged.
            let Some(shown) = app.formula_source(MeasureId(102)) else {
                continue; // read-only: nothing is offered for editing
            };
            match app.commit_formula(MeasureId(102), &shown) {
                Ok(()) => match &app.model.measures[&MeasureId(102)].kind {
                    MeasureKind::Derived(got) if *got == formula => {}
                    other => failures.push(format!(
                        "[{label}] displayed {shown:?} committed to a DIFFERENT formula: {other:?}"
                    )),
                },
                Err(e) => failures.push(format!(
                    "[{label}] displayed {shown:?} does not commit: {e}"
                )),
            }
        }
        assert!(
            failures.is_empty(),
            "{} shape(s) display text that does not re-commit:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// DEFECT 1, the three-keystroke user path: type controlled English that
    /// builds an aggregation with an empty `over`, commit, then commit the text
    /// the bar now shows without editing it.
    #[test]
    fn cnl_aggregation_without_over_recommits_from_the_bar() {
        let mut app = build_app(revenue_model());
        select(&mut app, MeasureId(102));
        app.commit_formula(MeasureId(102), "the sum of Quantity")
            .expect("CNL commits");
        let committed = match &app.model.measures[&MeasureId(102)].kind {
            MeasureKind::Derived(f) => f.clone(),
            MeasureKind::Input => panic!("stopped being derived"),
        };
        // The bar reloads its buffer from `formula_source`; Commit again.
        let shown = app
            .formula_source(MeasureId(102))
            .expect("an editable spelling exists (CNL)");
        app.commit_formula(MeasureId(102), &shown)
            .unwrap_or_else(|e| panic!("bar text {shown:?} does not commit: {e}"));
        match &app.model.measures[&MeasureId(102)].kind {
            MeasureKind::Derived(f) => assert_eq!(*f, committed, "AST changed on re-commit"),
            MeasureKind::Input => panic!("stopped being derived"),
        }
    }

    /// DEFECT 2, **as plan Step 4b resolves it**: an awkward measure name is no
    /// longer a reason to go read-only. The printer now spells it with a quoted
    /// identifier (`'Unit Price'`), the form the grammar gained in Step 4a, and
    /// [`ImprovApp::formula_source`]'s self-verification — reparse the candidate
    /// text and demand the IDENTICAL `Formula` — confirms it round-trips.
    ///
    /// The self-verification is untouched and still the gate: this test asserts
    /// what it now *accepts*, not that it was relaxed. Every name shown as
    /// editable is re-committed here and the AST compared.
    ///
    /// Exactly ONE name remains genuinely unspellable: the **empty** name. `''`
    /// is an explicit parse error ("empty quoted name: '' names nothing"), so
    /// there is no text for it in either language — and it must still be
    /// refused rather than shown.
    #[test]
    fn awkward_measure_names_are_spelled_with_quoted_identifiers() {
        // Bare where bare works (no needless quoting), quoted where it does not.
        let bare = ["UnitPrice", "Unit_Price", "Price2024", "_Price"];
        // The nine names the pre-Step-4b test listed as unspellable. Eight are
        // now editable; `""` is the lone holdout.
        let quoted = [
            "Unit Price",
            "Price/Unit",
            "Price-2024",
            "Q1 Revenue",
            "Over",
            "over",
            "AND",
            "Price(net)",
        ];
        // Plus the escape rule (`''` is one literal quote) and a dotted name,
        // which quoting keeps out of the deliberately-unresolved qualified form.
        let extra = ["Bob's Rate", "2024Price", "Ünit", "SUM", "Price.Unit"];
        let unspellable = [""];

        for name in bare
            .iter()
            .chain(quoted.iter())
            .chain(extra.iter())
            .chain(unspellable.iter())
        {
            let mut model = revenue_model();
            model.measures.get_mut(&MeasureId(100)).unwrap().name = Name((*name).into());
            let mut app = build_app(model);
            select(&mut app, MeasureId(102));
            let formula = match &app.model.measures[&MeasureId(102)].kind {
                MeasureKind::Derived(f) => f.clone(),
                MeasureKind::Input => panic!("Revenue is derived"),
            };
            match app.formula_source(MeasureId(102)) {
                // Shown as editable => it MUST commit unchanged. This is the
                // invariant `formula_source` self-verifies; re-check it end to
                // end through the real commit path.
                Some(shown) => {
                    assert!(
                        !unspellable.contains(name),
                        "name {name:?} must stay unspellable but the bar shows {shown:?}"
                    );
                    // Bare names print bare; awkward ones print quoted.
                    if bare.contains(name) {
                        assert!(
                            shown.starts_with(&format!("{name} ")),
                            "name {name:?} needs no quoting, got {shown:?}"
                        );
                    } else if quoted.contains(name) {
                        assert!(
                            shown.starts_with(&format!("'{name}'")),
                            "name {name:?} must print quoted, got {shown:?}"
                        );
                    }
                    app.commit_formula(MeasureId(102), &shown)
                        .unwrap_or_else(|e| {
                            panic!("name {name:?}: displayed {shown:?} does not commit: {e}")
                        });
                    match &app.model.measures[&MeasureId(102)].kind {
                        MeasureKind::Derived(f) => {
                            assert_eq!(*f, formula, "name {name:?}: AST changed ({shown:?})")
                        }
                        MeasureKind::Input => panic!("stopped being derived"),
                    }
                }
                // Not editable => neither parser may accept the DSL/CNL text,
                // which is exactly why we refuse to offer it.
                None => {
                    assert!(
                        unspellable.contains(name),
                        "name {name:?} is spellable but the bar refuses to show it"
                    );
                    let cnl = describe_formula(&NlContext::new(&app.model), &formula);
                    assert_ne!(
                        app.parse_formula_text(&cnl).ok().as_ref(),
                        Some(&formula),
                        "name {name:?}: CNL {cnl:?} DOES round-trip; it should be editable"
                    );
                }
            }
        }
    }

    /// The doubling escape, end to end: a name containing a literal `'` prints
    /// as `'Bob''s Rate'` and reparses to the same measure. (`parser`'s own
    /// tests cover the tokenizer; this pins the PRINTER's half of the pair.)
    #[test]
    fn an_embedded_quote_is_printed_doubled() {
        let mut model = revenue_model();
        model.measures.get_mut(&MeasureId(100)).unwrap().name = Name("Bob's Rate".into());
        let app = build_app(model);
        let shown = app
            .formula_source(MeasureId(102))
            .expect("a quote in a name is spellable by doubling it");
        assert_eq!(shown, "'Bob''s Rate' * Quantity");
    }

    /// Category names inside `[...]` and after `OVER` go through the same
    /// [`ident`], so an awkward CATEGORY name is spellable too — the other half
    /// of the read-only defect (a CSV import names categories from headers as
    /// well).
    #[test]
    fn awkward_category_names_are_quoted_in_dim_lists_and_over() {
        let mut model = revenue_model();
        model.categories.get_mut(&CategoryId(1)).unwrap().name = Name("Fiscal Year".into());
        // Revenue = SUM(Quantity OVER 'Fiscal Year')
        model.measures.get_mut(&MeasureId(102)).unwrap().kind =
            MeasureKind::Derived(Formula::new(Expr::Call(
                parser::FUNC_SUM,
                vec![Expr::Ref(
                    MeasureId(101),
                    DimensionSpec {
                        by: vec![],
                        over: vec![CategoryId(1)],
                        except: vec![],
                    },
                )],
            )));
        let mut app = build_app(model);
        let shown = app
            .formula_source(MeasureId(102))
            .expect("a quoted category name is spellable");
        assert_eq!(shown, "SUM(Quantity OVER 'Fiscal Year')");
        // And it round-trips through the real commit path.
        let before = match &app.model.measures[&MeasureId(102)].kind {
            MeasureKind::Derived(f) => f.clone(),
            MeasureKind::Input => panic!("derived"),
        };
        app.commit_formula(MeasureId(102), &shown).expect("commits");
        match &app.model.measures[&MeasureId(102)].kind {
            MeasureKind::Derived(f) => assert_eq!(*f, before),
            MeasureKind::Input => panic!("stopped being derived"),
        }
    }

    /// DEFECT 5: with an empty ROW axis the grid renders zero lines; the chart
    /// must not invent an x line labelled `""`.
    #[test]
    fn chart_with_an_empty_row_axis_has_no_x_lines() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(102));
        app.sync_axis_state();
        assert_eq!(app.grid_dims(), (2, 2));
        // Hide every Time item: rows -> empty.
        for it in [ItemId(10), ItemId(11)] {
            app.toggle_filter_item(CategoryId(1), it);
        }
        assert_eq!(app.grid_dims(), (0, 2), "grid renders no rows");
        let d = app.chart_series();
        assert!(
            d.x_labels.is_empty() && d.series.is_empty(),
            "chart invented {} x line(s) / {} series for an empty row axis: {d:?}",
            d.x_labels.len(),
            d.series.len()
        );
    }

    /// DEFECT 5: an empty COLUMN axis likewise yields no series, and an
    /// unpinnable PAGE category yields nothing at all (the grid is (0,0)) —
    /// never a full chart drawn at keys that omit the page category.
    #[test]
    fn chart_with_an_empty_column_or_page_axis_has_no_data() {
        // Empty column axis.
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(102));
        app.sync_axis_state();
        for it in [ItemId(20), ItemId(21)] {
            app.toggle_filter_item(CategoryId(2), it);
        }
        assert_eq!(app.grid_dims(), (2, 0), "grid renders no columns");
        let d = app.chart_series();
        assert!(
            d.x_labels.is_empty() && d.series.is_empty(),
            "chart drew data for an empty column axis: {d:?}"
        );

        // Empty page axis (3-D measure: Region pages).
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        assert_eq!(app.page_cats(), vec![CategoryId(3)]);
        for it in [ItemId(30), ItemId(31)] {
            app.toggle_filter_item(CategoryId(3), it);
        }
        assert_eq!(app.grid_dims(), (0, 0), "grid renders nothing");
        let d = app.chart_series();
        assert_eq!(
            d,
            crate::chart::ChartData::default(),
            "chart plotted at Region-less (under-specified) keys: {d:?}"
        );
    }

    /// DEFECT 7: a parse error sets the inline red underline; a subsequent
    /// VALID formula whose SAVE fails must not leave that underline pointing
    /// into text which parses cleanly.
    #[test]
    fn stale_parse_error_is_cleared_when_a_later_save_fails() {
        let mut app = build_app(revenue_model());
        // Step 1: a parse error sets the inline highlight.
        app.commit_formula(MeasureId(102), "Price *** Quantity")
            .expect_err("a parse error");
        assert!(app.formula_error_pos.is_some(), "parse error marked");
        assert!(!app.formula_error_msg.is_empty());

        // Step 2: a VALID formula whose save fails (unwritable store path).
        app.db = std::env::temp_dir()
            .join(format!("improv_gui_no_such_dir_{}", std::process::id()))
            .join("model.db")
            .to_string_lossy()
            .into_owned();
        let err = app
            .commit_formula(MeasureId(102), "Price + Quantity")
            .expect_err("unwritable store must fail the commit");
        assert!(err.starts_with("save failed:"), "got {err:?}");
        assert_eq!(
            app.formula_error_pos, None,
            "stale inline position survived a save failure"
        );
        assert!(
            app.formula_error_msg.is_empty(),
            "stale inline parse error survived a save failure: {:?}",
            app.formula_error_msg
        );
    }
    /// A category left in `axis_order` that is NOT a dimension of the selected
    /// measure must not affect that measure's grid. Regression for the review's
    /// defect #4: a stale PAGE category filtered to zero items blanked a grid
    /// whose every cell was fully specified.
    #[test]
    fn stale_non_dimension_page_category_does_not_blank_the_grid() {
        let mut m = Model::new();
        let (t, p, r) = (CategoryId(1), CategoryId(2), CategoryId(3));
        for (c, n) in [(t, "Time"), (p, "Product"), (r, "Region")] {
            m.add_category(c, n);
        }
        m.add_item(ItemId(10), t, "2025");
        m.add_item(ItemId(20), p, "W");
        m.add_item(ItemId(30), r, "North");
        // Sales ranges over Time x Product ONLY; Region is not a dimension.
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("Sales".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Input,
            description: None,
        });
        let cell = improv_core_model::Coordinate::from_pairs([(t, ItemId(10)), (p, ItemId(20))]);
        m.set_input(MeasureId(1), cell, Value::Number(42.0));

        let mut app = build_app(m);
        select(&mut app, MeasureId(1));
        app.sync_axis_state();
        // What apply_view leaves behind after the measure was re-imported over
        // fewer dimensions: a stale Region page category.
        app.focused_mut().axis_order = vec![t, p, r];
        app.focused_mut().n_rows = 1;
        app.focused_mut().n_cols = 1;
        app.focused_mut().page_idx = vec![0];
        app.focused_mut().filters = vec![Filter {
            category: r,
            items: vec![],
        }];

        assert!(
            app.page_cats().is_empty(),
            "a non-dimension category must not count as a page axis"
        );
        assert_eq!(
            app.grid_dims(),
            (1, 1),
            "grid must still render its one cell"
        );
        assert!(
            app.cursor_key().is_some(),
            "cell coordinate is fully specified"
        );

        // Contrast, and the 6a4b862 behavior that must NOT regress: a category
        // that IS a real dimension, filtered to empty, still yields zero lines.
        app.focused_mut().filters = vec![Filter {
            category: p,
            items: vec![],
        }];
        assert_eq!(
            app.grid_dims().1,
            0,
            "a real dimension filtered empty => no lines"
        );
    }

    /// Regression for the review's defect #6: a failed save must not leave the
    /// in-memory model diverged from the store.
    #[test]
    fn failed_cell_save_rolls_back_the_in_memory_edit() {
        let mut app = build_app(revenue_model());
        // An unwritable store path: save() must fail.
        app.db = "/nonexistent-dir-improv/cannot-write.db".to_string();
        let measure = MeasureId(101); // Quantity (input)
        let coord = vec![(1u32, 10u32), (2u32, 20u32)];
        let before = app.model.input(measure, &decode(&coord)).cloned();

        let res = app.set_cell(measure, coord.clone(), Value::Number(999.0));
        assert!(res.is_err(), "save to an unwritable path must fail");
        assert_eq!(
            app.model.input(measure, &decode(&coord)).cloned(),
            before,
            "defect #6: rejected value must not remain in the model"
        );
    }

    // -- typed (non-numeric) cell display + editing -------------------------

    /// A `Value::DateTime` built through the same literal grammar cell editing
    /// uses (so tests need no direct chrono dependency).
    fn date(text: &str) -> Value {
        parse_typed(&Model::new(), ValueType::DateTime, text).expect("fixture date")
    }

    /// One input measure per declared type, all over a single category so each
    /// grid is 1x1 and the coordinate is trivial.
    fn typed_model() -> Model {
        let mut m = Model::new();
        let p = CategoryId(1);
        m.add_category(p, "Product");
        m.add_item(ItemId(10), p, "WidgetA");
        for (id, name, vt) in [
            (1u32, "Qty", ValueType::Number),
            (2, "Label", ValueType::Text),
            (3, "Active", ValueType::Boolean),
            (4, "Shipped", ValueType::DateTime),
        ] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: vt,
                categories: vec![p],
                kind: MeasureKind::Input,
                description: None,
            });
        }
        let cell = improv_core_model::Coordinate::from_pairs([(p, ItemId(10))]);
        m.set_input(MeasureId(1), cell.clone(), Value::Number(7.0));
        m.set_input(MeasureId(2), cell.clone(), Value::Text("hello".into()));
        m.set_input(MeasureId(3), cell.clone(), Value::Boolean(true));
        m.set_input(MeasureId(4), cell, date("2025-03-04"));
        m
    }

    fn typed_key() -> CoordKey {
        vec![(1u32, 10u32)]
    }

    /// Every declared type RENDERS its value. Pre-fix the grid projected input
    /// cells through `f64`, so Text/Boolean/DateTime cells were blank.
    #[test]
    fn typed_input_cells_render_their_values() {
        let app = build_app(typed_model());
        let key = typed_key();
        assert_eq!(app.cell_text(MeasureId(1), &key).as_deref(), Some("7"));
        assert_eq!(app.cell_text(MeasureId(2), &key).as_deref(), Some("hello"));
        assert_eq!(app.cell_text(MeasureId(3), &key).as_deref(), Some("true"));
        assert_eq!(
            app.cell_text(MeasureId(4), &key).as_deref(),
            Some("2025-03-04T00:00:00+00:00")
        );
        // A cell with no value is still blank (absence, not type).
        assert_eq!(app.cell_text(MeasureId(2), &vec![(1u32, 99u32)]), None);
    }

    /// Each declared type round-trips through an edit: type-appropriate text in,
    /// the matching `Value` variant stored, the display reading it back.
    #[test]
    fn every_declared_type_round_trips_through_an_edit() {
        let mut app = build_app(typed_model());
        let key = typed_key();
        let coord = decode(&key);
        let cases: Vec<(MeasureId, &str, Value)> = vec![
            (MeasureId(1), "12.5", Value::Number(12.5)),
            (
                MeasureId(2),
                "widget label",
                Value::Text("widget label".into()),
            ),
            (MeasureId(3), "false", Value::Boolean(false)),
            (MeasureId(4), "2026-01-02", date("2026-01-02")),
        ];
        for (m, text, want) in cases {
            assert_eq!(
                app.commit_cell_text(m, key.clone(), text).as_deref(),
                Ok("cell updated"),
                "measure {} rejected {text:?}",
                m.0
            );
            assert_eq!(
                app.model.input(m, &coord),
                Some(&want),
                "measure {} stored the wrong variant",
                m.0
            );
            assert_eq!(
                app.cell_text(m, &key),
                Some(
                    CellValue::from_model_value(&want)
                        .expect("typed cell")
                        .to_string()
                ),
                "measure {} reads back differently than it stored",
                m.0
            );
        }
    }

    /// The DECLARED type wins over the text's shape: numeric-looking text typed
    /// into a Text measure stays Text. Pre-fix every commit parsed `f64` and
    /// stored `Value::Number`, corrupting the cell's type.
    #[test]
    fn text_measure_never_stores_a_number() {
        let mut app = build_app(typed_model());
        let key = typed_key();
        let coord = decode(&key);
        for text in ["hello", "42"] {
            app.commit_cell_text(MeasureId(2), key.clone(), text)
                .expect("text commit");
            assert_eq!(
                app.model.input(MeasureId(2), &coord),
                Some(&Value::Text(text.into())),
                "{text:?} must stay Text for a Text-declared measure"
            );
        }
        // And the typed API refuses a mismatched variant outright.
        let err = app
            .set_cell(MeasureId(2), key, Value::Number(42.0))
            .expect_err("a Text measure must refuse a Number");
        assert!(err.contains("declared Text"), "got {err:?}");
    }

    /// Text that does not parse as the declared type is rejected: the error is
    /// surfaced and the prior typed value is untouched.
    #[test]
    fn invalid_typed_input_is_rejected_and_prior_value_survives() {
        let mut app = build_app(typed_model());
        let key = typed_key();
        let coord = decode(&key);
        let cases = [
            (MeasureId(1), "abc", Value::Number(7.0), "not a number"),
            (MeasureId(3), "maybe", Value::Boolean(true), "not a boolean"),
            (MeasureId(4), "not-a-date", date("2025-03-04"), "not a date"),
        ];
        for (m, bad, prior, msg) in cases {
            let err = app
                .commit_cell_text(m, key.clone(), bad)
                .expect_err("invalid input must be rejected");
            assert!(err.contains(msg), "measure {} said {err:?}", m.0);
            assert_eq!(
                app.model.input(m, &coord),
                Some(&prior),
                "measure {} lost its prior value to a rejected edit",
                m.0
            );
        }
    }

    /// The chosen empty-commit behavior: an empty buffer CLEARS the cell
    /// (Escape already cancels, so this is the only way to delete a value).
    #[test]
    fn empty_commit_clears_the_cell() {
        let mut app = build_app(typed_model());
        let key = typed_key();
        let coord = decode(&key);
        assert_eq!(
            app.commit_cell_text(MeasureId(2), key.clone(), "   ")
                .as_deref(),
            Ok("cell cleared")
        );
        assert_eq!(app.model.input(MeasureId(2), &coord), None);
        assert_eq!(app.cell_text(MeasureId(2), &key), None, "cleared = blank");
        // Clearing an already-empty cell is a no-op success.
        assert!(app.commit_cell_text(MeasureId(2), key, "").is_ok());
    }

    /// Clearing a numeric cell must retract it from the live engine too, so
    /// derived measures stop computing with a value the cell no longer holds.
    #[test]
    fn clearing_a_numeric_cell_retracts_it_from_the_engine() {
        let mut app = build_app(revenue_model());
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(
            app.values_for(MeasureId(102)).get(&key),
            Some(&70.0),
            "Revenue = Price(10) * Quantity(7)"
        );
        app.clear_cell(MeasureId(101), key.clone())
            .expect("clear input cell");
        assert_eq!(
            app.values_for(MeasureId(102)).get(&key),
            None,
            "a cleared input must not leave a stale derived cell"
        );
    }

    /// Retyping a cell as a non-numeric value must also retract it from the
    /// engine's numeric lane (only numbers flow there), so no derived cell keeps
    /// computing from a number the cell no longer holds.
    #[test]
    fn retyping_a_numeric_cell_as_text_retracts_it_from_the_engine() {
        // Build the engine while Quantity is numeric, so it really holds the
        // cell (declaring Text up front makes `Price * Quantity` fail to
        // compile, leaving no engine and testing nothing), THEN redeclare the
        // measure as Text so a text edit is the legal one.
        let mut app = build_app(revenue_model());
        assert!(app.engine.is_some(), "the fixture needs a live engine");
        app.model
            .measures
            .get_mut(&MeasureId(101))
            .expect("Quantity")
            .value_type = ValueType::Text;
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(
            app.values_for(MeasureId(102)).get(&key),
            Some(&70.0),
            "the engine starts out computing from the numeric cell"
        );
        app.commit_cell_text(MeasureId(101), key.clone(), "n/a")
            .expect("text commit");
        assert_eq!(
            app.cell_text(MeasureId(101), &key).as_deref(),
            Some("n/a"),
            "the text value is what the grid shows"
        );
        assert_eq!(
            app.values_for(MeasureId(102)).get(&key),
            None,
            "derived cell must not keep computing from the retracted number"
        );
    }

    /// Defect-6 atomicity on the TYPED path: a failed save rolls the in-memory
    /// typed edit back (set AND clear), prior value intact.
    #[test]
    fn failed_save_rolls_back_a_typed_edit() {
        let mut app = build_app(typed_model());
        app.db = "/nonexistent-dir-improv/cannot-write.db".to_string();
        let key = typed_key();
        let coord = decode(&key);

        let err = app
            .commit_cell_text(MeasureId(2), key.clone(), "replacement")
            .expect_err("save to an unwritable path must fail");
        assert!(err.starts_with("save failed:"), "got {err:?}");
        assert_eq!(
            app.model.input(MeasureId(2), &coord),
            Some(&Value::Text("hello".into())),
            "a failed save must not leave the typed edit in memory"
        );

        let err = app
            .commit_cell_text(MeasureId(2), key, "")
            .expect_err("save must fail");
        assert!(err.starts_with("save failed:"), "got {err:?}");
        assert_eq!(
            app.model.input(MeasureId(2), &coord),
            Some(&Value::Text("hello".into())),
            "a failed save must not leave the cell cleared in memory"
        );
    }

    /// Begin-edit seeds the buffer with the cell's TYPED display text, so a
    /// text/date cell is tweaked rather than retyped from blank.
    #[test]
    fn begin_edit_seeds_the_typed_display_text() {
        let mut app = build_app(typed_model());
        select(&mut app, MeasureId(2)); // Label (Text)
        app.sync_axis_state();
        app.begin_edit_cursor();
        assert_eq!(
            app.focused().editing.as_ref().map(|(m, _)| *m),
            Some(MeasureId(2))
        );
        assert_eq!(app.focused().edit_buf, "hello", "text cell seeded blank");
    }

    // -- ITEM 1: grid shortcuts must not steal keystrokes ------------------

    /// The gate is a pure predicate so it can be checked exhaustively: grid
    /// bindings run only when NOTHING owns the keyboard. Pre-fix the decision
    /// was `!editing` alone, so the `(false, true)` row — some *other* text
    /// field focused — wrongly enabled them.
    #[test]
    fn grid_keys_are_enabled_only_when_nothing_owns_the_keyboard() {
        assert!(grid_keys_enabled(false, false), "idle grid: keys act");
        assert!(!grid_keys_enabled(true, false), "cell editor open");
        assert!(
            !grid_keys_enabled(false, true),
            "another text field is focused: it must get the keystroke"
        );
        assert!(!grid_keys_enabled(true, true), "cell editor focused");
    }

    /// Feed one frame of key presses through `handle_grid_keys` with `focus`
    /// optionally held by a NON-grid widget id, and report what the grid state
    /// became: (selected measure, cursor, first page index).
    ///
    /// The keys are delivered to the FOCUSED matrix (index `app.focus_index()`),
    /// which is what the canvas does; `frame_with_keys_for` drives a specific
    /// matrix, for asserting that an unfocused one ignores them.
    fn frame_with_keys(
        app: &mut ImprovApp,
        keys: &[egui::Key],
        focus: Option<&str>,
    ) -> (Option<MeasureId>, (usize, usize), usize) {
        let mi = app.focus_index();
        frame_with_keys_for(app, mi, keys, focus)
    }

    /// [`frame_with_keys`], addressed at matrix `mi` rather than the focused one.
    fn frame_with_keys_for(
        app: &mut ImprovApp,
        mi: usize,
        keys: &[egui::Key],
        focus: Option<&str>,
    ) -> (Option<MeasureId>, (usize, usize), usize) {
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            events: keys
                .iter()
                .map(|k| egui::Event::Key {
                    key: *k,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect(),
            ..Default::default()
        };
        let _ = ctx.run(raw, |ctx| {
            if let Some(id) = focus {
                // Exactly what a focused TextEdit elsewhere in the app does.
                ctx.memory_mut(|m| m.request_focus(egui::Id::new(id)));
            }
            egui::CentralPanel::default().show(ctx, |ui| {
                app.handle_grid_keys(ui, mi);
            });
        });
        (
            app.selected(),
            (app.focused().cursor_row, app.focused().cursor_col),
            app.focused().page_idx.first().copied().unwrap_or(0),
        )
    }

    /// Typing in the formula bar / new-measure form / CSV wizard / view-name
    /// box must not drive the grid: with focus elsewhere, `n`, `j`/`l` and `]`
    /// change nothing. Pre-fix each of them cycled the measure, moved the cell
    /// cursor, and paged a dimension mid-word.
    #[test]
    fn focused_text_field_elsewhere_swallows_grid_shortcuts() {
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        let before = (
            app.selected(),
            (app.focused().cursor_row, app.focused().cursor_col),
            0,
        );

        use egui::Key;
        let keys = [Key::N, Key::J, Key::L, Key::CloseBracket, Key::Enter];
        for field in ["formula_bar", "new_measure_name", "csv_path", "view_name"] {
            let after = frame_with_keys(&mut app, &keys, Some(field));
            assert_eq!(after, before, "{field} focused, yet the grid reacted");
            assert!(
                app.focused().editing.is_none(),
                "{field} focused, yet Enter opened a cell editor"
            );
        }
    }

    /// The gate is transient, not sticky: the very next frame after focus is
    /// released, the same keys work again.
    #[test]
    fn grid_shortcuts_work_again_once_focus_is_released() {
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200)); // Sales[Time, Product, Region]
        app.sync_axis_state();

        // Frame 1: focused elsewhere -> `]` must not page.
        use egui::Key;
        let (_, _, page) = frame_with_keys(&mut app, &[Key::CloseBracket], Some("some_text_field"));
        assert_eq!(page, 0, "paged while a text field had focus");

        // Frame 2: focus released -> the very same key works again (the gate is
        // transient, not a latch).
        let (_, cursor, page) = frame_with_keys(&mut app, &[Key::CloseBracket], None);
        assert_eq!(page, 1, "`]` must page the first page dimension again");
        assert_eq!(cursor, (0, 0));
    }

    /// Cell editing is untouched by the gate: `editing.is_some()` already
    /// suppressed the bindings, and the editor's own focus must not change that.
    #[test]
    fn cell_editing_still_works_while_editing() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.sync_axis_state();

        // Enter opens the editor (no focus held).
        use egui::Key;
        frame_with_keys(&mut app, &[Key::Enter], None);
        let editing = app.focused().editing.clone();
        assert!(
            editing.is_some(),
            "Enter must begin editing the cursor cell"
        );

        // While editing, the cell editor holds focus. The grid bindings stay
        // out of the way and the edit buffer survives the frame.
        app.focused_mut().edit_buf = "7".into();
        let after = frame_with_keys(&mut app, &[Key::N, Key::J, Key::L], Some("cell_editor"));
        assert_eq!(after.1, (0, 0), "cursor moved under the open editor");
        assert_eq!(
            app.focused().editing,
            editing,
            "the open editor was disturbed"
        );
        assert_eq!(app.focused().edit_buf, "7");

        // And committing the buffer still writes the cell.
        let key = app.cursor_key().expect("cursor addresses a cell");
        let msg = app
            .commit_cell_text(MeasureId(101), key.clone(), &app.focused().edit_buf.clone())
            .expect("commit");
        assert_eq!(msg, "cell updated");
        assert_eq!(app.cell_text(MeasureId(101), &key).as_deref(), Some("7"));
    }

    /// Ctrl+Z goes through the same gate: it must not fire while a text field
    /// is focused (Ctrl+Z belongs to that field's own editing), and must fire
    /// when nothing is focused.
    #[test]
    fn undo_shortcut_honors_the_focus_gate() {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101));
        app.sync_axis_state();
        let key = app.cursor_key().expect("cell");
        app.set_cell(MeasureId(101), key.clone(), Value::Number(999.0))
            .expect("edit");
        assert!(app.can_undo());

        let press_undo = |app: &mut ImprovApp, focus: Option<&str>| {
            let ctx = egui::Context::default();
            let raw = egui::RawInput {
                modifiers: egui::Modifiers::COMMAND,
                events: vec![egui::Event::Key {
                    key: egui::Key::Z,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }],
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                if let Some(id) = focus {
                    ctx.memory_mut(|m| m.request_focus(egui::Id::new(id)));
                }
                app.handle_history_keys(ctx);
            });
        };

        press_undo(&mut app, Some("formula_bar"));
        assert_eq!(
            app.cell_text(MeasureId(101), &key).as_deref(),
            Some("999"),
            "Ctrl+Z fired while typing in another field"
        );

        press_undo(&mut app, None);
        assert_eq!(
            app.cell_text(MeasureId(101), &key).as_deref(),
            Some("100"),
            "Ctrl+Z with no focus must undo"
        );
    }

    // -- ITEM 2: undo / redo of MODEL state -------------------------------

    /// The oracle cell + its Revenue key for the 2x2 fixture.
    fn q_and_rev_keys() -> (CoordKey, CoordKey) {
        let mut k = vec![(1u32, 10u32), (2, 20)];
        k.sort();
        (k.clone(), k)
    }

    #[test]
    fn undo_restores_the_cell_and_the_engine_snapshot() {
        let mut app = build_app(grid_2x2_model());
        let (qkey, rkey) = q_and_rev_keys();
        // Quantity[2025, WidgetA] = 100, Price[WidgetA] = 10 -> Revenue 1000.
        assert_eq!(app.values_for(MeasureId(102)).get(&rkey), Some(&1000.0));

        app.set_cell(MeasureId(101), qkey.clone(), Value::Number(200.0))
            .expect("edit");
        assert_eq!(app.values_for(MeasureId(102)).get(&rkey), Some(&2000.0));

        app.undo().expect("undo");
        assert_eq!(
            app.cell_text(MeasureId(101), &qkey).as_deref(),
            Some("100"),
            "undo must restore the prior cell value"
        );
        assert_eq!(
            app.values_for(MeasureId(102)).get(&rkey),
            Some(&1000.0),
            "the engine snapshot must agree with the restored model"
        );

        app.redo().expect("redo");
        assert_eq!(app.cell_text(MeasureId(101), &qkey).as_deref(), Some("200"));
        assert_eq!(app.values_for(MeasureId(102)).get(&rkey), Some(&2000.0));
    }

    #[test]
    fn undo_restores_a_cleared_cell() {
        let mut app = build_app(grid_2x2_model());
        let (qkey, rkey) = q_and_rev_keys();
        app.clear_cell(MeasureId(101), qkey.clone()).expect("clear");
        assert_eq!(app.cell_text(MeasureId(101), &qkey), None);

        app.undo().expect("undo");
        assert_eq!(app.cell_text(MeasureId(101), &qkey).as_deref(), Some("100"));
        assert_eq!(app.values_for(MeasureId(102)).get(&rkey), Some(&1000.0));
    }

    #[test]
    fn undo_a_formula_commit_and_a_new_derived_measure() {
        let mut app = build_app(revenue_model());
        let mut rkey = vec![(1u32, 10u32), (2, 20)];
        rkey.sort();
        assert_eq!(app.values_for(MeasureId(102)).get(&rkey), Some(&70.0));

        app.commit_formula(MeasureId(102), "Price + Quantity")
            .expect("commit");
        assert_eq!(app.values_for(MeasureId(102)).get(&rkey), Some(&17.0));

        app.undo().expect("undo");
        assert_eq!(
            app.formula_source(MeasureId(102)).as_deref(),
            Some("Price * Quantity"),
            "undo must restore the prior formula"
        );
        assert_eq!(
            app.values_for(MeasureId(102)).get(&rkey),
            Some(&70.0),
            "the engine must be rebuilt from the restored formula"
        );

        // A new derived measure is undone away entirely.
        let id = app
            .add_derived_measure("Margin", "Price - Quantity")
            .expect("add");
        assert!(app.model.measures.contains_key(&id));
        app.undo().expect("undo");
        assert!(
            app.model.measure_by_name("Margin").is_none(),
            "undo must remove the added measure"
        );
        assert!(
            !app.snapshot.contains_key(&id),
            "the snapshot must not keep values for a measure that is gone"
        );
        assert_eq!(app.selected(), Some(MeasureId(102)), "selection re-pointed");
    }

    #[test]
    fn undo_a_view_save() {
        let mut app = build_app(revenue_model());
        let id = app.save_view("L1").expect("saved");
        assert!(app.model.views.contains_key(&id));
        app.undo().expect("undo");
        assert!(app.model.views.is_empty(), "undo must drop the saved view");
    }

    /// A CSV import creates a measure, a category, and items. Undo must take
    /// all of them away, not just the cells.
    #[test]
    fn undo_a_csv_import_removes_what_it_created() {
        let path = std::env::temp_dir().join(format!(
            "improv_gui_undo_import_{}_{}.csv",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::write(&path, "region,amount\nNorth,5\nSouth,7\n").expect("write csv");
        struct Cleanup(std::path::PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(path.clone());

        let mut app = build_app(inputs_only_model());
        let before = app.model.clone();
        app.import_form = ImportForm {
            path: path.to_string_lossy().into_owned(),
            tsv: false,
            has_header: true,
            measure_id: "500".into(),
            measure_name: "Imported".into(),
            value_column: "amount".into(),
            dimensions: vec![csv_wizard::DimRow {
                column: "region".into(),
                category_id: "9".into(),
                category_name: "Region".into(),
            }],
        };
        app.run_csv_import();
        assert!(
            app.status.starts_with("imported 2 cell(s)"),
            "import failed: {}",
            app.status
        );
        assert!(app.model.measures.contains_key(&MeasureId(500)));
        assert!(app.model.categories.contains_key(&CategoryId(9)));
        let imported_items = app.model.items.len();
        assert!(imported_items > before.items.len(), "items were minted");

        app.undo().expect("undo");
        assert!(
            !app.model.measures.contains_key(&MeasureId(500)),
            "undo must remove the imported measure"
        );
        assert!(
            !app.model.categories.contains_key(&CategoryId(9)),
            "undo must remove the category the import created"
        );
        assert_eq!(app.model, before, "undo must restore the pre-import model");
        assert_eq!(
            app.selected(),
            pick_default_measure(&before),
            "selection must leave the measure that no longer exists"
        );

        app.redo().expect("redo");
        assert!(app.model.measures.contains_key(&MeasureId(500)));
        assert_eq!(app.model.items.len(), imported_items);
    }

    #[test]
    fn undo_at_the_bottom_of_the_stack_is_a_no_op() {
        let mut app = build_app(revenue_model());
        let before = app.model.clone();
        assert!(!app.can_undo());
        app.undo().expect("undo on an empty history is a no-op");
        app.redo().expect("redo on an empty history is a no-op");
        assert_eq!(app.model, before);
        // And after undoing the only recorded step.
        let (qkey, _) = q_and_rev_keys();
        app.set_cell(MeasureId(101), qkey, Value::Number(1.0))
            .expect("edit");
        app.undo().expect("undo");
        assert!(!app.can_undo());
        app.undo().expect("still a no-op, not a panic");
        assert_eq!(app.model, before);
    }

    #[test]
    fn history_depth_is_bounded_and_evicts_the_oldest() {
        let mut app = build_app(grid_2x2_model());
        let (qkey, _) = q_and_rev_keys();
        // UNDO_DEPTH + 5 edits: the five oldest undo points are evicted.
        for i in 1..=(UNDO_DEPTH + 5) {
            app.set_cell(
                MeasureId(101),
                qkey.clone(),
                Value::Number(1000.0 + i as f64),
            )
            .expect("edit");
        }
        assert_eq!(app.undo_stack.len(), UNDO_DEPTH, "depth bound not enforced");

        // Undo everything we can: we land on the state after edit #5 (the
        // oldest still-recorded point), never the original 100.
        while app.can_undo() {
            app.undo().expect("undo");
        }
        assert_eq!(
            app.cell_text(MeasureId(101), &qkey).as_deref(),
            Some("1005"),
            "the oldest retained undo point should be the state after edit 5"
        );
    }

    #[test]
    fn a_new_mutation_after_undo_clears_the_redo_stack() {
        let mut app = build_app(grid_2x2_model());
        let (qkey, _) = q_and_rev_keys();
        app.set_cell(MeasureId(101), qkey.clone(), Value::Number(200.0))
            .expect("edit");
        app.undo().expect("undo");
        assert!(app.can_redo(), "undo must make the step redoable");

        app.set_cell(MeasureId(101), qkey.clone(), Value::Number(300.0))
            .expect("edit");
        assert!(
            !app.can_redo(),
            "a fresh mutation must make the undone future unreachable"
        );
        app.undo().expect("undo");
        assert_eq!(app.cell_text(MeasureId(101), &qkey).as_deref(), Some("100"));
    }

    /// Undo must PERSIST: undo then quit must not resurrect the undone state.
    #[test]
    fn undo_is_saved_to_the_store() {
        let db = std::env::temp_dir()
            .join(format!(
                "improv_gui_undo_persist_{}_{}.db",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ))
            .to_string_lossy()
            .into_owned();
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(db.clone());

        ModelStore::open(&db)
            .and_then(|mut s| s.save_model(&grid_2x2_model()))
            .expect("seed the store");
        let mut app = ImprovApp::load(&db).expect("load");
        let (qkey, _) = q_and_rev_keys();

        app.set_cell(MeasureId(101), qkey.clone(), Value::Number(200.0))
            .expect("edit");
        app.undo().expect("undo");

        let reloaded = ModelStore::open(&db)
            .and_then(|mut s| s.load_model())
            .expect("reload");
        assert_eq!(
            reloaded.input(MeasureId(101), &decode(&qkey)),
            Some(&Value::Number(100.0)),
            "the undone value was left in the store"
        );
    }

    /// A store write that fails during undo is surfaced, and the step is not
    /// lost: the undo point stays, so the user can retry.
    #[test]
    fn a_failed_save_during_undo_is_reported_and_keeps_the_step() {
        let mut app = build_app(grid_2x2_model());
        let (qkey, _) = q_and_rev_keys();
        app.set_cell(MeasureId(101), qkey.clone(), Value::Number(200.0))
            .expect("edit");

        app.db = std::env::temp_dir()
            .join(format!("improv_gui_no_such_dir_{}", std::process::id()))
            .join("model.db")
            .to_string_lossy()
            .into_owned();
        let err = app
            .undo()
            .expect_err("an unwritable store must fail the undo");
        assert!(err.starts_with("save failed:"), "got {err:?}");
        assert_eq!(
            app.cell_text(MeasureId(101), &qkey).as_deref(),
            Some("200"),
            "a failed undo must leave the live state alone"
        );
        assert!(app.can_undo(), "the undo point must survive a failed undo");

        // The UI path reports it instead of claiming success.
        app.undo_with_status();
        assert!(app.status.starts_with("undo failed:"), "{}", app.status);
    }

    // -- STEP 1: margin gutters (docs/reviews/2026-09-22-gui-reconstruction-plan.md)

    /// A headless egui `Context` sized like a desktop window, plus a `pass`
    /// closure that lays out one real frame of the whole app and returns the
    /// gutter/table geometry the grid recorded.
    ///
    /// This is a genuine headless frame: `Context::run` over the same panel
    /// sequence `eframe::App::update` drives, so the rects are the ones a user
    /// would see, not a reconstruction. (`eframe::Frame` cannot be built outside
    /// eframe, but `update` only forwards it, so the panel calls are made
    /// directly here.) The `Context` is handed back so a caller can run MANY
    /// frames on ONE context: egui carries panel extents from frame to frame, so
    /// single-frame geometry cannot see a gutter that ratchets.
    fn layout_harness() -> (
        egui::Context,
        impl FnMut(&egui::Context, &mut ImprovApp) -> GutterRects,
    ) {
        let ctx = egui::Context::default();
        ctx.set_style(crate::theme::next_style());
        let mut time = 0.0_f64;
        let pass = move |ctx: &egui::Context, app: &mut ImprovApp| {
            time += 1.0 / 60.0;
            let raw = egui::RawInput {
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                app.sync_axis_state();
                app.document_tab_bar(ctx);
                app.formula_bar(ctx);
                app.status_bar(ctx);
                app.formula_panel(ctx);
                app.formula_list_panel(ctx);
                app.tool_palette(ctx);
                app.explorer_panel(ctx);
                app.inspector_panel(ctx);
                app.chart_panel(ctx);
                app.csv_wizard_panel(ctx);
                app.canvas_panel(ctx);
            });
            app.focused()
                .gutters
                .expect("the focused matrix must record its gutters")
        };
        (ctx, pass)
    }

    /// Lay out the whole app for several frames at a desktop window size and
    /// return EVERY matrix's recorded gutter geometry, in canvas order.
    ///
    /// The multi-matrix counterpart of [`layout_frame`]: a canvas holding N
    /// matrices must have N framed tables, not one.
    fn layout_all(app: &mut ImprovApp) -> Vec<GutterRects> {
        let (ctx, mut pass) = layout_harness();
        for _ in 0..8 {
            let _ = pass(&ctx, app);
        }
        app.matrices
            .iter()
            .enumerate()
            .map(|(i, m)| {
                m.gutters
                    .unwrap_or_else(|| panic!("matrix {i} recorded no gutters"))
            })
            .collect()
    }

    /// Lay out `app` until its geometry stops changing, and return it. A panel
    /// reads its extent from the previous frame's `PanelState`, so the frame
    /// right after a layout change still reports the old size; settling makes an
    /// assertion about *the* layout rather than about a transient.
    ///
    /// Panics if the geometry never settles within 20 frames — which is itself
    /// the check that no gutter ratchets open frame after frame.
    fn settle(
        ctx: &egui::Context,
        app: &mut ImprovApp,
        pass: &mut impl FnMut(&egui::Context, &mut ImprovApp) -> GutterRects,
    ) -> GutterRects {
        let mut prev = pass(ctx, app);
        for _ in 0..20 {
            let now = pass(ctx, app);
            if now == prev {
                return now;
            }
            prev = now;
        }
        panic!("the gutter layout never settled (it drifts every frame): {prev:?}");
    }

    /// Lay out one app on a fresh context and return its settled geometry.
    fn layout_frame(app: &mut ImprovApp) -> GutterRects {
        let (ctx, mut pass) = layout_harness();
        settle(&ctx, app, &mut pass)
    }

    /// A `Sales[Time, Product, Region]` app ready to lay out: `Time` on rows,
    /// `Product` on columns, `Region` in the well.
    fn gutter_app() -> ImprovApp {
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        app
    }

    /// Re-pivoting repeatedly on ONE long-lived context must not make any gutter
    /// creep: stacking a tile grows the column gutter, unstacking shrinks it back
    /// to exactly its old size, and the corner well returns to its old height.
    ///
    /// This is a regression the first draft of this step actually had: a drop
    /// zone claiming `available_size()` inside a self-sizing panel ratcheted the
    /// well taller every frame, because the next frame's "available" included
    /// what the zone claimed in the last one. A single-frame test cannot see it.
    #[test]
    fn gutters_do_not_creep_across_frames_or_repivots() {
        let mut app = gutter_app();
        let (ctx, mut pass) = layout_harness();
        let one = settle(&ctx, &mut app, &mut pass);

        app.set_axis(CategoryId(3), Axis::Columns);
        let two = settle(&ctx, &mut app, &mut pass);
        assert!(
            two.top.height() > one.top.height(),
            "a second stacked column tile must make the gutter taller"
        );

        app.set_axis(CategoryId(3), Axis::Pages);
        let back = settle(&ctx, &mut app, &mut pass);
        assert_eq!(
            back.top.height(),
            one.top.height(),
            "the column gutter must shrink back, not latch open"
        );
        assert_eq!(
            back.well.height(),
            one.well.height(),
            "the corner well must shrink back, not ratchet"
        );
        assert!(gutters_frame_table(&back), "{back:?}");
    }

    /// Shrinking the window must never panic a debug build, and must never claim
    /// the gutters frame the table when they have been squeezed flat. At a real
    /// window size the invariant holds; below the size where the panels still fit
    /// [`gutters_have_room`] reports that and the per-frame `debug_assert!`
    /// stands down.
    ///
    /// This drives [`ImprovApp::gutter_frame`]'s own `debug_assert!` at every
    /// size, so a regression that breaks adjacency at a usable window size fails
    /// here rather than at a user's first resize.
    #[test]
    fn shrinking_the_window_never_panics_and_never_lies_about_framing() {
        let sizes = [
            (1200.0, 800.0),
            (900.0, 600.0),
            (600.0, 400.0),
            (300.0, 200.0),
            (120.0, 80.0),
            (10.0, 10.0),
        ];
        let mut framed_at_least_once = false;
        for (w, h) in sizes {
            let mut app = gutter_app();
            let ctx = egui::Context::default();
            ctx.set_style(crate::theme::next_style());
            // Several frames so the panels settle at this size (and so the
            // per-frame debug assertion runs on the settled geometry too).
            for i in 0..8 {
                let raw = egui::RawInput {
                    time: Some(f64::from(i) / 60.0),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(w, h),
                    )),
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    app.sync_axis_state();
                    app.canvas_panel(ctx);
                });
            }
            let g = app
                .focused()
                .gutters
                .expect("gutters recorded at every size");
            if gutters_have_room(&g) {
                framed_at_least_once = true;
                assert!(
                    gutters_frame_table(&g),
                    "{w}x{h}: there was room, so the gutters must frame the table: {g:?}"
                );
            }
        }
        assert!(
            framed_at_least_once,
            "no size had room — the test would be vacuous"
        );
    }

    /// **The Step 1 acceptance test.** The row gutter's rect adjoins the
    /// table's LEFT edge and the column gutter's adjoins its TOP edge — the
    /// geometric check the old three-zone horizontal shelf failed (it sat
    /// entirely above the grid, touching neither edge).
    #[test]
    fn row_gutter_adjoins_the_tables_left_edge_and_column_gutter_its_top() {
        let g = layout_frame(&mut gutter_app());

        assert!(
            (g.left.max.x - g.table.min.x).abs() <= EDGE_EPS,
            "row gutter right edge {} must BE the table's left edge {} (gutters {g:?})",
            g.left.max.x,
            g.table.min.x
        );
        assert!(
            (g.top.max.y - g.table.min.y).abs() <= EDGE_EPS,
            "column gutter bottom edge {} must BE the table's top edge {} (gutters {g:?})",
            g.top.max.y,
            g.table.min.y
        );
        // The gutters run ALONGSIDE the table, not merely touch a corner.
        assert!(
            g.left.min.y <= g.table.min.y && g.left.max.y > g.table.min.y,
            "the row gutter must span the table vertically: {g:?}"
        );
        assert!(
            g.top.min.x <= g.table.min.x && g.top.max.x > g.table.min.x,
            "the column gutter must span the table horizontally: {g:?}"
        );
        // Both gutters are real, visible strips.
        assert!(g.left.width() > 0.0 && g.left.height() > 0.0, "{g:?}");
        assert!(g.top.width() > 0.0 && g.top.height() > 0.0, "{g:?}");
        // And the whole arrangement satisfies the framing invariant.
        assert!(gutters_frame_table(&g), "{g:?}");
    }

    /// The corner well is the BOTTOM-LEFT one the reference shows (`Country` /
    /// `Travel` inline with the horizontal scrollbar): below the table, flush
    /// with the row gutter's left edge.
    #[test]
    fn the_page_well_is_the_bottom_left_corner() {
        let g = layout_frame(&mut gutter_app());
        assert!(
            g.well.min.y >= g.left.max.y - EDGE_EPS,
            "the well must sit BELOW the framed region (the row gutter): {g:?}"
        );
        assert!(
            g.well.min.y >= g.table.min.y,
            "the well must be below the table's top, not beside it: {g:?}"
        );
        assert!(
            (g.well.min.x - g.left.min.x).abs() <= EDGE_EPS,
            "the well must be flush with the row gutter's left edge: {g:?}"
        );
    }

    /// The true corner (above the row gutter, left of the column tiles) is
    /// blank, as the reference shows — the column gutter starts to the RIGHT of
    /// the row gutter's width, never over it.
    #[test]
    fn the_column_gutter_leaves_a_blank_corner_over_the_row_gutter() {
        let g = layout_frame(&mut gutter_app());
        // The top gutter spans the full frame width (corner + tiles), and the
        // table begins one gutter-width in, so the corner box is exactly the
        // part of the top gutter left of the table.
        let corner = egui::Rect::from_min_max(g.top.min, egui::pos2(g.table.min.x, g.top.max.y));
        assert!(
            corner.width() >= GUTTER_W - 2.0 * EDGE_EPS - 4.0,
            "the blank corner must be a gutter-width wide, got {}: {g:?}",
            corner.width()
        );
    }

    /// Stacking two categories on the column axis makes the column gutter
    /// TALLER (tiles stack vertically, as the reference's "Result" panel shows
    /// `Travel` above `Hours`) while still adjoining the table's top edge.
    #[test]
    fn stacked_column_tiles_grow_the_gutter_downward_and_stay_docked() {
        let mut app = gutter_app();
        let one = layout_frame(&mut app);
        assert_eq!(app.col_cats().len(), 1);

        // Stack a second category on columns (the drop a drag would produce).
        app.set_axis(CategoryId(3), Axis::Columns);
        assert_eq!(app.col_cats().len(), 2, "two categories on the column axis");
        let two = layout_frame(&mut app);

        assert!(
            two.top.height() > one.top.height(),
            "a second stacked column tile must make the gutter taller: {} -> {}",
            one.top.height(),
            two.top.height()
        );
        assert!(
            (two.top.max.y - two.table.min.y).abs() <= EDGE_EPS,
            "the taller gutter must still adjoin the table's top edge: {two:?}"
        );
        assert!(gutters_frame_table(&two), "{two:?}");

        // ...and unstacking shrinks it back: the gutter must not latch at its
        // tallest (a drop zone sized to `available_size` inside a self-sizing
        // panel would ratchet the panel open forever).
        app.set_axis(CategoryId(3), Axis::Pages);
        assert_eq!(app.col_cats().len(), 1);
        let back = layout_frame(&mut app);
        assert!(
            back.top.height() <= one.top.height() + EDGE_EPS,
            "the column gutter latched open: {} vs {}",
            back.top.height(),
            one.top.height()
        );
        assert!(gutters_frame_table(&back), "{back:?}");
    }

    /// An axis filtered to zero lines still lays out gutters that frame the
    /// table (the empty-axis fix must not be regressed by the new geometry).
    #[test]
    fn gutters_still_frame_the_table_when_an_axis_is_empty() {
        let mut app = gutter_app();
        let rows = app.row_cats();
        hide_all_items(&mut app, rows[0]);
        assert_eq!(app.grid_dims().0, 0, "the row axis must render zero lines");
        let g = layout_frame(&mut app);
        assert!(gutters_frame_table(&g), "{g:?}");
    }

    /// Dragging a tile from one gutter to another re-pivots through `set_axis`:
    /// the drop is egui-internal, so drive the same payload/axis pair the drop
    /// zone produces and assert on the axis state.
    #[test]
    fn dropping_a_tile_in_another_gutter_repivots() {
        let mut app = gutter_app(); // Sales[Time, Product, Region]
        let (time, product, region) = (CategoryId(1), CategoryId(2), CategoryId(3));
        assert_eq!(app.row_cats(), vec![time]);
        assert_eq!(app.col_cats(), vec![product]);
        assert_eq!(app.page_cats(), vec![region]);

        // Drag `Time` from the row gutter into the column gutter: it leaves rows
        // and APPENDS to the column stack (the reference's `Travel` landing
        // under `Hours`).
        app.set_axis(time, Axis::Columns);
        assert!(app.row_cats().is_empty(), "Time must leave the row gutter");
        assert_eq!(
            app.col_cats(),
            vec![product, time],
            "Time must stack UNDER the existing column category"
        );

        // Drag `Region` out of the well into the row gutter.
        app.set_axis(region, Axis::Rows);
        assert_eq!(app.row_cats(), vec![region]);
        assert!(app.page_cats().is_empty(), "the well must be empty now");

        // And the layout still frames the table after the re-pivot.
        let g = layout_frame(&mut app);
        assert!(gutters_frame_table(&g), "{g:?}");
    }

    /// The per-tile dropdown arrow is the mouse-only fallback: it cycles the
    /// category rows -> columns -> pages -> rows through the same `set_axis`.
    #[test]
    fn the_tile_arrow_cycles_a_category_through_all_three_axes() {
        let mut app = gutter_app();
        let time = CategoryId(1);
        assert_eq!(app.row_cats(), vec![time]);
        // Rows -> Columns -> Pages -> Rows, the `tile` next-axis sequence.
        app.set_axis(time, Axis::Columns);
        assert!(app.col_cats().contains(&time));
        app.set_axis(time, Axis::Pages);
        assert!(app.page_cats().contains(&time));
        app.set_axis(time, Axis::Rows);
        assert_eq!(app.row_cats(), vec![time]);
    }

    /// `gutters_frame_table` must REJECT the old horizontal shelf: three zones
    /// laid out side by side above the grid touch neither edge. Without this the
    /// invariant could be vacuously true.
    #[test]
    fn the_old_horizontal_shelf_fails_the_framing_invariant() {
        let r = |x0, y0, x1, y1| egui::Rect::from_min_max(egui::pos2(x0, y0), egui::pos2(x1, y1));
        // Shelf: [Columns][Rows][Pages] in one row at y 0..30, grid below at y 40.
        let shelf = GutterRects {
            top: r(0.0, 0.0, 120.0, 30.0),
            left: r(126.0, 0.0, 246.0, 30.0),
            well: r(252.0, 0.0, 372.0, 30.0),
            table: r(0.0, 40.0, 800.0, 600.0),
        };
        assert!(
            !gutters_frame_table(&shelf),
            "the shelf must NOT count as framing the table"
        );
        // The gutter arrangement passes.
        let docked = GutterRects {
            top: r(0.0, 0.0, 800.0, 40.0),
            left: r(0.0, 40.0, 120.0, 600.0),
            table: r(120.0, 40.0, 800.0, 600.0),
            well: r(0.0, 600.0, 800.0, 634.0),
        };
        assert!(gutters_frame_table(&docked), "{docked:?}");
    }

    // -- STEP 2: the formula list pane
    // (docs/reviews/2026-09-22-gui-reconstruction-plan.md)

    /// A model with FOUR derived measures, one chained onto another, so the
    /// formula list has several rows and disabling one has a visible dependent:
    ///
    /// * `Revenue   = Price * Quantity`      (id 102)
    /// * `Tax       = Revenue * Rate`        (id 103, depends on Revenue)
    /// * `Net       = Revenue - Tax`         (id 104, depends on both)
    /// * `Doubled   = Quantity + Quantity`   (id 105, independent)
    fn four_formula_model() -> Model {
        let mut m = grid_2x2_model();
        let (t, p) = (CategoryId(1), CategoryId(2));
        m.add_measure(Measure {
            id: MeasureId(110),
            name: Name("Rate".into()),
            value_type: ValueType::Number,
            categories: vec![t, p],
            kind: MeasureKind::Input,
            description: None,
        });
        // Declared over the same dimensions as Revenue, with a cell at every
        // coordinate: this engine aligns a derived*input product on matching
        // dimensions, it does not broadcast a narrower input across them.
        for ti in [ItemId(10), ItemId(11)] {
            for pi in [ItemId(20), ItemId(21)] {
                m.set_input(
                    MeasureId(110),
                    improv_core_model::Coordinate::from_pairs([(t, ti), (p, pi)]),
                    Value::Number(0.1),
                );
            }
        }
        let refm = |id| Expr::Ref(id, DimensionSpec::default());
        for (id, name, expr) in [
            (
                103u32,
                "Tax",
                Expr::BinaryOp(
                    BinaryOp::Mul,
                    Box::new(refm(MeasureId(102))),
                    Box::new(refm(MeasureId(110))),
                ),
            ),
            (
                104,
                "Net",
                Expr::BinaryOp(
                    BinaryOp::Sub,
                    Box::new(refm(MeasureId(102))),
                    Box::new(refm(MeasureId(103))),
                ),
            ),
            (
                105,
                "Doubled",
                Expr::BinaryOp(
                    BinaryOp::Add,
                    Box::new(refm(MeasureId(101))),
                    Box::new(refm(MeasureId(101))),
                ),
            ),
        ] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: ValueType::Number,
                categories: vec![t, p],
                kind: MeasureKind::Derived(Formula::new(expr)),
                description: None,
            });
        }
        m
    }

    /// **Step 2 acceptance, part 1:** a model with several derived measures
    /// lists ALL of them, numbered, each with its target measure and formula
    /// text — the whole model's logic visible at once, which a single-line bar
    /// for the selected measure cannot show.
    #[test]
    fn the_formula_list_shows_every_formula_numbered() {
        let app = build_app(four_formula_model());
        let rows = app.formula_rows();

        // Every derived measure, and ONLY derived measures (Price/Quantity/Rate
        // are inputs: they have no formula, so they have no row).
        assert_eq!(
            rows.iter().map(|r| r.target.as_str()).collect::<Vec<_>>(),
            vec!["Revenue", "Tax", "Net", "Doubled"],
            "all four formulas, in measure-id order"
        );
        assert_eq!(
            app.model
                .measures
                .values()
                .filter(|m| m.is_derived())
                .count(),
            rows.len(),
            "one row per derived measure, no more"
        );

        // Each row carries its formula text, and it is the editable spelling.
        assert_eq!(rows[0].text, "Price * Quantity");
        assert_eq!(rows[1].text, "Revenue * Rate");
        assert_eq!(rows[2].text, "Revenue - Tax");
        assert_eq!(rows[3].text, "Quantity + Quantity");
        assert!(rows.iter().all(|r| r.error.is_none()), "{rows:?}");
        assert!(rows.iter().all(|r| r.enabled), "formulas start enabled");

        // The display number is the 1-based row position (the reference's
        // `1.` `2.` …): inserting a formula renumbers, it is not an identity.
        let numbered: Vec<String> = rows
            .iter()
            .enumerate()
            .map(|(i, r)| format!("{}. {} = {}", i + 1, r.target, r.text))
            .collect();
        assert_eq!(numbered[0], "1. Revenue = Price * Quantity");
        assert_eq!(numbered[3], "4. Doubled = Quantity + Quantity");
    }

    /// **Step 2 acceptance, part 2:** clicking a row selects that measure, so
    /// the grid follows the formula list. Driven through a real headless frame
    /// (a synthesized click at the row's own screen position), not by calling a
    /// handler — so a row that renders un-clickable fails here.
    #[test]
    fn clicking_a_formula_row_selects_its_measure() {
        let mut app = build_app(four_formula_model());
        select(&mut app, MeasureId(102));
        app.sync_axis_state();

        // Find where row 4 ("Doubled") landed, then click it.
        let target = MeasureId(105);
        assert_ne!(app.selected(), Some(target), "not already selected");
        let clicked = click_formula_row(&mut app, target);
        assert!(clicked, "row for measure {target:?} was never laid out");
        assert_eq!(
            app.selected(),
            Some(target),
            "clicking the row must select its measure so the grid follows"
        );
        // And the grid really did follow: the pivot state re-homed onto it.
        app.sync_axis_state();
        assert_eq!(app.focused().axis_for, Some(target));
    }

    /// Lay out `app` in real headless frames and click the formula-list row for
    /// `measure`, at the position the row actually occupied. Returns whether
    /// such a row was laid out.
    ///
    /// Two phases, because egui hit-tests a click against the rect a widget had
    /// in a PREVIOUS frame: settle the panels first (recording
    /// `formula_row_rects`), then deliver a press+release at that point. A row
    /// that renders un-clickable therefore fails this, where calling a handler
    /// directly would not.
    fn click_formula_row(app: &mut ImprovApp, measure: MeasureId) -> bool {
        let ctx = egui::Context::default();
        ctx.set_style(crate::theme::next_style());
        let frame = |app: &mut ImprovApp, events: Vec<egui::Event>, time: f64| {
            let raw = egui::RawInput {
                time: Some(time),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 800.0),
                )),
                events,
                ..Default::default()
            };
            let _ = ctx.run(raw, |ctx| {
                app.sync_axis_state();
                app.formula_bar(ctx);
                app.tool_palette(ctx);
                app.explorer_panel(ctx);
                app.inspector_panel(ctx);
                app.formula_panel(ctx);
                app.formula_list_panel(ctx);
                app.canvas_panel(ctx);
            });
        };

        // Settle the panels so the pane's rows have their final positions.
        for i in 0..4 {
            frame(app, Vec::new(), f64::from(i) / 60.0);
        }
        let Some(&(_, rect)) = app.formula_row_rects.iter().find(|(m, _)| *m == measure) else {
            return false;
        };
        let at = rect.center();

        // A real press+release at the row's center.
        frame(
            app,
            vec![
                egui::Event::PointerMoved(at),
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            1.0,
        );
        true
    }

    /// **Step 2 acceptance, part 3:** toggling a formula off recomputes
    /// dependents. Asserted on the SNAPSHOT, which is what the grid draws.
    ///
    /// `Tax = Revenue * Rate` and `Net = Revenue - Tax`. Disabling `Tax` must
    /// make `Tax` compute nothing *and* change `Net`, which is the real test: a
    /// GUI-only "hide this formula" flag would leave `Net` sitting at its old
    /// value.
    ///
    /// `Net` goes *absent* rather than falling back to `Revenue - 0`, because
    /// this engine JOINS its operands: a measure with no cells contributes no
    /// coordinates, so every dependent loses those coordinates too. That is the
    /// same behavior a dependent of an empty input measure already has —
    /// disabling a formula makes it indistinguishable from an input nobody has
    /// filled in, which is the honest reading of "this formula is not running".
    #[test]
    fn disabling_a_formula_recomputes_its_dependents() {
        let mut app = build_app(four_formula_model());
        let (qkey, _) = q_and_rev_keys();

        // Baseline: Revenue 1000, Tax 100, Net 900.
        assert_eq!(app.values_for(MeasureId(102)).get(&qkey), Some(&1000.0));
        assert_eq!(app.values_for(MeasureId(103)).get(&qkey), Some(&100.0));
        assert_eq!(app.values_for(MeasureId(104)).get(&qkey), Some(&900.0));

        app.set_formula_enabled(MeasureId(103), false)
            .expect("disable Tax");

        // Tax computes nothing now — it is no longer a dataflow output.
        assert!(
            !app.formula_enabled(MeasureId(103)),
            "the model records Tax as disabled"
        );
        assert_eq!(
            app.values_for(MeasureId(103)).get(&qkey),
            None,
            "a disabled formula must compute nothing"
        );
        // ...and the DEPENDENT recomputed rather than keeping its stale 900:
        // Net's operand vanished, so Net vanished with it.
        assert_eq!(
            app.values_for(MeasureId(104)).get(&qkey),
            None,
            "Net must recompute without Tax, not keep its old 900"
        );
        assert!(
            app.values_for(MeasureId(104)).is_empty(),
            "no coordinate of Net survives its operand being switched off"
        );
        // The untouched formulas are unaffected — only the dependents moved.
        assert_eq!(app.values_for(MeasureId(102)).get(&qkey), Some(&1000.0));
        assert_eq!(app.values_for(MeasureId(105)).get(&qkey), Some(&200.0));

        // Re-enabling restores every value exactly — the formula was kept, not
        // re-derived from text.
        app.set_formula_enabled(MeasureId(103), true)
            .expect("re-enable Tax");
        assert!(app.formula_enabled(MeasureId(103)));
        assert_eq!(app.values_for(MeasureId(103)).get(&qkey), Some(&100.0));
        assert_eq!(app.values_for(MeasureId(104)).get(&qkey), Some(&900.0));
    }

    /// **Step 2 acceptance, part 4:** the disable SURVIVES a save/reload round
    /// trip through `storage_mentat` — which is why it is a model-level fact and
    /// not a GUI flag. After reload the formula is still present, still
    /// disabled, and the reloaded engine still computes dependents without it.
    #[test]
    fn a_disabled_formula_survives_a_save_reload_round_trip() {
        let db = std::env::temp_dir()
            .join(format!(
                "improv_gui_disabled_persist_{}_{}.db",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ))
            .to_string_lossy()
            .into_owned();
        struct Cleanup(String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let _cleanup = Cleanup(db.clone());

        ModelStore::open(&db)
            .and_then(|mut s| s.save_model(&four_formula_model()))
            .expect("seed the store");
        let mut app = ImprovApp::load(&db).expect("load");
        let (qkey, _) = q_and_rev_keys();

        // Give Tax a real description too, so the round trip must preserve BOTH
        // the user's text and the disabled bit.
        app.model
            .measures
            .get_mut(&MeasureId(103))
            .expect("Tax")
            .description = Some("rate applied to revenue".into());
        app.set_formula_enabled(MeasureId(103), false)
            .expect("disable Tax");

        // Reload from the store: a fresh app, nothing carried in memory.
        let reloaded = ImprovApp::load(&db).expect("reload");
        assert!(
            !reloaded.formula_enabled(MeasureId(103)),
            "the disabled bit did not survive the round trip"
        );
        // The FORMULA itself survived (that is why the marker rides alongside it
        // rather than replacing it), and so did the user's own description text.
        let tax = &reloaded.model.measures[&MeasureId(103)];
        assert!(
            tax.is_derived(),
            "a disabled measure stays derived, keeping its formula"
        );
        assert_eq!(
            split_marker(tax.description.as_deref()).1,
            Some("rate applied to revenue"),
            "the user's description must survive the toggle"
        );
        // The reloaded ENGINE honors it: Tax blank, and Net — which depends on
        // Tax — recomputed away with it.
        assert_eq!(reloaded.values_for(MeasureId(103)).get(&qkey), None);
        assert_eq!(reloaded.values_for(MeasureId(104)).get(&qkey), None);
        // The list still shows it, unchecked rather than hidden.
        let rows = reloaded.formula_rows();
        let tax_row = rows
            .iter()
            .find(|r| r.measure == MeasureId(103))
            .expect("a disabled formula is still listed");
        assert!(!tax_row.enabled);
        assert_eq!(tax_row.text, "Revenue * Rate", "its formula is still shown");

        // And re-enabling in the reloaded app restores the original values.
        let mut reloaded = reloaded;
        reloaded
            .set_formula_enabled(MeasureId(103), true)
            .expect("re-enable");
        assert_eq!(
            reloaded.values_for(MeasureId(104)).get(&qkey),
            Some(&900.0),
            "re-enabling after a reload must restore the exact formula"
        );
    }

    /// **Step 2 acceptance, part 5:** a formula that does not build shows its
    /// error INLINE on its own row, rather than vanishing or silently listing as
    /// fine. (A cell-type mismatch: `Revenue * Label` where `Label` is Text.)
    #[test]
    fn a_formula_that_fails_to_build_shows_its_error_inline() {
        let mut model = four_formula_model();
        model.add_measure(Measure {
            id: MeasureId(120),
            name: Name("Label".into()),
            value_type: ValueType::Text,
            categories: vec![CategoryId(2)],
            kind: MeasureKind::Input,
            description: None,
        });
        // Broken = Revenue * Label — a type error the compiler rejects.
        model.add_measure(Measure {
            id: MeasureId(121),
            name: Name("Broken".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(1), CategoryId(2)],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(102), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(120), DimensionSpec::default())),
            ))),
            description: None,
        });
        let app = build_app(model);
        let rows = app.formula_rows();

        // It is LISTED, with its text, and carries an inline error.
        let broken = rows
            .iter()
            .find(|r| r.measure == MeasureId(121))
            .expect("a broken formula is still listed");
        assert_eq!(broken.text, "Revenue * Label");
        let err = broken
            .error
            .as_deref()
            .expect("a formula that does not build must show an error inline");
        assert!(
            err.to_lowercase().contains("type"),
            "the inline error must say what is wrong, got {err:?}"
        );
        // The SOUND formulas do not borrow its error.
        for r in rows.iter().filter(|r| r.measure != MeasureId(121)) {
            assert_eq!(r.error, None, "{r:?} must not report an error");
        }
    }

    /// A formula with no editable spelling says so inline, because the editor
    /// above shows that row read-only — the list must not imply it is editable.
    #[test]
    fn a_formula_with_no_editable_spelling_says_so_inline() {
        let mut model = four_formula_model();
        // An empty measure name is the one thing `ident` cannot spell (`''` is a
        // parse error), so Revenue's formula loses its DSL and CNL spellings.
        model.measures.get_mut(&MeasureId(100)).unwrap().name = Name(String::new());
        let app = build_app(model);
        let rows = app.formula_rows();
        let rev = rows
            .iter()
            .find(|r| r.measure == MeasureId(102))
            .expect("still listed");
        assert_eq!(
            rev.error.as_deref(),
            Some("no editable spelling (shown as description)"),
            "the list must admit this row is not editable"
        );
        assert!(!rev.text.is_empty(), "it still shows a description");
    }

    /// The disabled marker is a *reserved* description line, so it must not be
    /// confused with a user description that merely looks like it, and toggling
    /// must be an exact round trip in both directions.
    #[test]
    fn the_disabled_marker_never_eats_a_user_description() {
        // A description that starts with the marker's characters but is not it.
        let lookalike = format!("{DISABLED_MARKER}ish notes");
        assert_eq!(
            split_marker(Some(lookalike.as_str())),
            (false, Some(lookalike.as_str()))
        );
        // The marker alone, and the marker with text after it.
        assert_eq!(split_marker(Some(DISABLED_MARKER)), (true, None));
        let with_text = format!("{DISABLED_MARKER}{MARKER_SEP}keep me");
        assert_eq!(split_marker(Some(&with_text)), (true, Some("keep me")));
        // No description at all.
        assert_eq!(split_marker(None), (false, None));
        // join_marker inverts split_marker for each of those.
        for (disabled, user) in [
            (false, None),
            (false, Some("plain")),
            (true, None),
            (true, Some("keep me")),
        ] {
            let joined = join_marker(disabled, user);
            assert_eq!(
                split_marker(joined.as_deref()),
                (disabled, user),
                "join/split must round-trip ({disabled}, {user:?})"
            );
        }

        // End to end: a measure whose description is the lookalike is NOT
        // treated as disabled, and toggling it keeps that text.
        let mut model = four_formula_model();
        model.measures.get_mut(&MeasureId(103)).unwrap().description = Some(lookalike.clone());
        let mut app = build_app(model);
        assert!(app.formula_enabled(MeasureId(103)), "not actually disabled");
        app.set_formula_enabled(MeasureId(103), false).expect("off");
        app.set_formula_enabled(MeasureId(103), true).expect("on");
        assert_eq!(
            app.model.measures[&MeasureId(103)].description.as_deref(),
            Some(&*lookalike),
            "a disable/enable cycle must restore the description exactly"
        );
    }

    /// Toggling a formula is undoable and atomic on the same terms as a formula
    /// edit: it goes through `publish`, so it is an undo point, and a failed
    /// save leaves the model untouched.
    #[test]
    fn toggling_a_formula_is_undoable_and_atomic() {
        let mut app = build_app(four_formula_model());
        let (qkey, _) = q_and_rev_keys();
        app.set_formula_enabled(MeasureId(103), false).expect("off");
        assert!(app.can_undo(), "a toggle must be an undo point");
        app.undo().expect("undo the toggle");
        assert!(app.formula_enabled(MeasureId(103)), "undo re-enabled it");
        assert_eq!(
            app.values_for(MeasureId(104)).get(&qkey),
            Some(&900.0),
            "undo restored the dependent's value too"
        );
        // An unwritable store fails the toggle and changes nothing.
        let mut app = build_app(four_formula_model());
        app.db = std::env::temp_dir()
            .join(format!("improv_gui_no_dir_{}", std::process::id()))
            .join("model.db")
            .to_string_lossy()
            .into_owned();
        let err = app
            .set_formula_enabled(MeasureId(103), false)
            .expect_err("an unwritable store must fail the toggle");
        assert!(err.starts_with("save failed:"), "got {err:?}");
        assert!(
            app.formula_enabled(MeasureId(103)),
            "a failed toggle must leave the model alone"
        );
        assert_eq!(app.values_for(MeasureId(103)).get(&qkey), Some(&100.0));
    }

    /// Toggling is a no-op (not an error) where there is no formula to toggle,
    /// and disabling EVERY formula is a valid model that simply computes
    /// nothing — no engine, no panic, inputs still readable.
    #[test]
    fn toggling_without_a_formula_is_a_no_op_and_all_off_is_valid() {
        let mut app = build_app(four_formula_model());
        // An input measure and an unknown id: both no-ops.
        app.set_formula_enabled(MeasureId(101), false)
            .expect("input measure");
        app.set_formula_enabled(MeasureId(9999), false)
            .expect("unknown id");
        assert!(!app.can_undo(), "a no-op must not create an undo point");
        assert!(app.formula_enabled(MeasureId(101)));

        for id in [102u32, 103, 104, 105] {
            app.set_formula_enabled(MeasureId(id), false)
                .unwrap_or_else(|e| panic!("disable {id}: {e}"));
        }
        assert!(app.engine.is_none(), "nothing left to compute");
        let (qkey, _) = q_and_rev_keys();
        assert_eq!(app.values_for(MeasureId(102)).get(&qkey), None);
        // Inputs still read fine, and every row is still listed.
        assert_eq!(app.cell_text(MeasureId(101), &qkey).as_deref(), Some("100"));
        assert_eq!(app.formula_rows().len(), 4);
        assert!(app.formula_rows().iter().all(|r| !r.enabled));
    }

    /// The pane renders in a real frame without panicking, at a realistic size
    /// and squeezed flat — the same headless-frame discipline Step 1 established.
    #[test]
    fn the_formula_pane_lays_out_at_every_window_size() {
        for (w, h) in [
            (1200.0, 800.0),
            (800.0, 400.0),
            (300.0, 200.0),
            (60.0, 40.0),
        ] {
            let mut app = build_app(four_formula_model());
            let ctx = egui::Context::default();
            ctx.set_style(crate::theme::next_style());
            for i in 0..4 {
                let raw = egui::RawInput {
                    time: Some(f64::from(i) / 60.0),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(w, h),
                    )),
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    app.sync_axis_state();
                    app.formula_bar(ctx);
                    app.formula_panel(ctx);
                    app.formula_list_panel(ctx);
                    app.canvas_panel(ctx);
                });
            }
            // Still listing every formula after the frames.
            assert_eq!(app.formula_rows().len(), 4, "{w}x{h}");
        }
    }
    // -- STEP 3: N matrices on a canvas
    // (docs/reviews/2026-09-22-gui-reconstruction-plan.md)

    /// A canvas with TWO matrices over `grid_2x2_model()`: matrix 0 shows
    /// `Quantity` (an input), matrix 1 shows `Revenue = Price * Quantity` (its
    /// dependent). The shape the Quantrix reference has — several matrices side
    /// by side, each its own pivot — and the shape that makes "an edit in one
    /// recomputes the other" a real assertion.
    fn two_matrix_app() -> ImprovApp {
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101)); // Quantity on matrix 0
        let i = app.add_matrix(Some(MeasureId(102))); // Revenue on matrix 1
        assert_eq!(i, 1);
        app.set_focus(0);
        app.sync_axis_state();
        app
    }

    /// **Step 3 acceptance, part 1:** two matrices exist on one canvas at once,
    /// and each is INDEPENDENTLY pivotable — pivoting one leaves the other's axis
    /// state exactly as it was.
    ///
    /// Asserted on per-matrix axis state, because that is what "independently
    /// pivotable" means: before the refactor there was one `axis_order` for the
    /// whole app, so this could not even be expressed.
    #[test]
    fn two_matrices_are_independently_pivotable() {
        let mut app = two_matrix_app();
        let (t, p) = (CategoryId(1), CategoryId(2));
        assert_eq!(app.matrix_count(), 2);

        // Both start on their measure's natural axes: Time on rows, Product on
        // columns.
        for mi in 0..2 {
            let m = &app.matrices[mi];
            assert_eq!(m.row_cats(&app.model), vec![t], "matrix {mi} rows");
            assert_eq!(m.col_cats(&app.model), vec![p], "matrix {mi} cols");
        }
        assert_eq!(app.matrices[0].measure, Some(MeasureId(101)));
        assert_eq!(app.matrices[1].measure, Some(MeasureId(102)));

        // Pivot matrix 1 only (the focused matrix is 0, so this must go through
        // the matrix itself — exactly as a click in matrix 1 does).
        let model = app.model.clone();
        app.matrices[1].pivot_rotate(&model);

        assert_eq!(
            app.matrices[1].row_cats(&app.model),
            vec![p],
            "matrix 1 pivoted: Product on rows"
        );
        assert_eq!(app.matrices[1].col_cats(&app.model), vec![t]);
        assert_eq!(
            app.matrices[0].row_cats(&app.model),
            vec![t],
            "matrix 0 must be untouched by matrix 1's pivot"
        );
        assert_eq!(app.matrices[0].col_cats(&app.model), vec![p]);

        // And stacking on one matrix does not stack on the other.
        app.matrices[0].set_axis(&model, p, Axis::Rows);
        assert_eq!(app.matrices[0].row_cats(&app.model), vec![t, p]);
        assert!(app.matrices[0].col_cats(&app.model).is_empty());
        assert_eq!(
            app.matrices[1].row_cats(&app.model),
            vec![p],
            "matrix 1 still has its own layout"
        );

        // Filters are per matrix too: hiding an item in one does not hide it in
        // the other.
        app.matrices[0].toggle_filter_item(&model, t, ItemId(11));
        assert_eq!(app.matrices[0].sorted_items(&app.model, t).len(), 1);
        assert_eq!(
            app.matrices[1].sorted_items(&app.model, t).len(),
            2,
            "a filter is presentation on ONE matrix, not the model"
        );
        // ...and so are cursors.
        app.matrices[1].move_cursor(&model, 5, 5);
        assert_eq!(
            (app.matrices[0].cursor_row, app.matrices[0].cursor_col),
            (0, 0),
            "matrix 0's cursor must not follow matrix 1's"
        );

        // Finally: the two layouts are not merely *stored* separately, they are
        // RENDERED separately. A real headless frame, then each matrix's own
        // record of what its grid was drawn with (see `Matrix::rendered_axes`).
        // Without this a renderer that ignored its matrix index and drew the
        // focused matrix's pivot twice would pass everything above.
        let _ = layout_all(&mut app);
        let drew = |app: &ImprovApp, mi: usize| {
            app.matrices[mi]
                .rendered_axes
                .clone()
                .unwrap_or_else(|| panic!("matrix {mi} never rendered"))
        };
        assert_eq!(
            drew(&app, 0),
            (vec![t, p], vec![]),
            "matrix 0 must be drawn with ITS stacked rows"
        );
        assert_eq!(
            drew(&app, 1),
            (vec![p], vec![t]),
            "matrix 1 must be drawn with ITS pivot, not matrix 0's"
        );
        assert_ne!(
            drew(&app, 0),
            drew(&app, 1),
            "two matrices with different pivots must render differently"
        );
    }

    /// **Step 3 acceptance, part 2:** an edit in ONE matrix recomputes a
    /// dependent measure displayed in ANOTHER. The canvas is a view over one
    /// shared engine, so `Quantity` edited on matrix 0 must change what matrix 1
    /// (`Revenue = Price * Quantity`) draws at that coordinate.
    ///
    /// Asserted through `cell_text` at matrix 1's own cursor key — the string the
    /// second matrix actually renders — not through the snapshot, so a canvas
    /// that rendered stale values from a per-matrix cache would fail here.
    #[test]
    fn an_edit_in_one_matrix_recomputes_a_dependent_in_another() {
        let mut app = two_matrix_app();
        // Both matrices sit on [2025, WidgetA] (cursor 0,0). Quantity = 100,
        // Price = 10, so Revenue = 1000.
        let qkey = app.matrices[0]
            .cursor_key(&app.model)
            .expect("matrix 0 addresses a cell");
        let rkey = app.matrices[1]
            .cursor_key(&app.model)
            .expect("matrix 1 addresses a cell");
        assert_eq!(qkey, rkey, "both matrices are on the same coordinate");
        assert_eq!(app.cell_text(MeasureId(101), &qkey).as_deref(), Some("100"));
        assert_eq!(
            app.cell_text(MeasureId(102), &rkey).as_deref(),
            Some("1000")
        );

        // Edit Quantity through matrix 0's cell editor path.
        app.commit_cell_text(MeasureId(101), qkey.clone(), "250")
            .expect("commit the edit");

        assert_eq!(
            app.cell_text(MeasureId(101), &qkey).as_deref(),
            Some("250"),
            "matrix 0 shows the edit"
        );
        assert_eq!(
            app.cell_text(MeasureId(102), &rkey).as_deref(),
            Some("2500"),
            "matrix 1's dependent measure must recompute: 10 * 250"
        );
        // The other coordinates of Revenue are untouched (an incremental
        // recompute, not a wholesale reset).
        let mut other = vec![(1u32, 11u32), (2, 20)];
        other.sort();
        assert_eq!(
            app.cell_text(MeasureId(102), &other).as_deref(),
            Some("1200"),
            "an unrelated coordinate must keep its value"
        );
    }

    /// **Step 3 acceptance, part 3a:** saving and reloading round-trips the whole
    /// multi-matrix canvas — every matrix's measure, pivot, filters, page pins
    /// and geometry — through `storage_mentat` and back.
    #[test]
    fn save_reload_round_trips_a_multi_matrix_canvas() {
        let db = temp_db("multi_matrix");
        let _cleanup = Cleanup(db.clone());
        ModelStore::open(&db)
            .and_then(|mut s| s.save_model(&sales_3d_model()))
            .expect("seed the store");

        let mut app = ImprovApp::load(&db).expect("load");
        select(&mut app, MeasureId(200));
        app.sync_axis_state();
        // Matrix 0: pivoted, its page dimension advanced one item.
        let model = app.model.clone();
        app.matrices[0].pivot_rotate(&model);
        app.matrices[0].set_page(&model, 0, 1);
        app.matrices[0].rect = CanvasRect {
            x: 12.0,
            y: 34.0,
            w: 500.0,
            h: 300.0,
        };
        // Matrix 1: the same measure, with its column category STACKED onto its
        // row axis instead, placed elsewhere.
        //
        // Which category that is, is read from the matrix rather than hardcoded:
        // `load_measure_categories` queries datalog, which does not promise an
        // order, so a measure's declared category order after a reload is not
        // fixed. Naming a category id here made this test pass or fail depending
        // on which one happened to land on the column axis.
        app.add_matrix(Some(MeasureId(200)));
        app.sync_axis_state();
        let stack = app.matrices[1].col_cats(&app.model)[0];
        app.matrices[1].set_axis(&model, stack, Axis::Rows);
        assert_eq!(app.matrices[1].n_rows, 2, "the fixture must really stack");
        app.matrices[1].rect = CanvasRect {
            x: 600.0,
            y: 40.0,
            w: 420.0,
            h: 260.0,
        };
        let want: Vec<MatrixPlacement> = app
            .matrices
            .iter()
            .map(|m| m.to_placement(&app.model).expect("placement"))
            .collect();

        let id = app.save_view("Canvas A").expect("saved");
        // The view holds BOTH matrices: the primary flat fields plus one extra.
        let stored = app.model.views.get(&id).expect("view stored");
        assert_eq!(
            stored.placements.len(),
            1,
            "one EXTRA matrix beyond primary"
        );
        assert_eq!(stored.matrices(), want, "the view describes both matrices");

        // Reload from the store and apply the view: the canvas comes back.
        let mut reloaded = ImprovApp::load(&db).expect("reload");
        assert_eq!(reloaded.matrix_count(), 1, "a fresh app starts single");
        let v = reloaded.model.views[&id].clone();
        reloaded.apply_view(&v);
        reloaded.sync_axis_state(); // must not clobber the applied layout

        assert_eq!(reloaded.matrix_count(), 2, "both matrices restored");
        let got: Vec<MatrixPlacement> = reloaded
            .matrices
            .iter()
            .map(|m| m.to_placement(&reloaded.model).expect("placement"))
            .collect();
        assert_eq!(got, want, "every matrix round-tripped exactly");

        // Spot-check the parts that are easy to lose, against what the live
        // canvas had rather than against absolute category ids: the store does
        // not promise to preserve a measure's DECLARED category order, so
        // "Time ends up on rows" is not a property of this round trip. That each
        // matrix comes back with the pivot, page pin and geometry IT had is.
        for (mi, w) in want.iter().enumerate() {
            let got = &reloaded.matrices[mi];
            assert_eq!(
                got.row_cats(&reloaded.model),
                w.axis_order[..w.n_rows].to_vec(),
                "matrix {mi}'s row stack"
            );
            assert_eq!(
                got.col_cats(&reloaded.model),
                w.axis_order[w.n_rows..w.n_rows + w.n_cols].to_vec(),
                "matrix {mi}'s column stack"
            );
            assert_eq!(
                got.pinned_pages(&reloaded.model),
                w.page_items,
                "matrix {mi}'s page pin"
            );
        }
        // The two matrices really do differ, so the loop above is not comparing
        // one layout with itself: matrix 1 has two categories stacked on rows.
        assert_eq!(reloaded.matrices[0].n_rows, 1);
        assert_eq!(reloaded.matrices[1].n_rows, 2);
        assert!(
            reloaded.matrices[1]
                .row_cats(&reloaded.model)
                .contains(&stack),
            "matrix 1's stacked category came back on its row axis"
        );
        assert_eq!(reloaded.matrices[0].rect.x, 12.0, "geometry per matrix");
        assert_eq!(reloaded.matrices[1].rect.x, 600.0);
    }

    /// **Step 3 acceptance, part 3b:** a LEGACY single-matrix view — one written
    /// before canvases, with no `placements` and no geometry — still loads
    /// correctly, and loads as exactly one matrix with its saved pivot.
    ///
    /// The view here is built the pre-canvas way (flat fields only, `placements`
    /// empty, `rect` defaulted), which is what `serde` produces for an old stored
    /// blob; `storage_mentat` has its own test that such a blob deserializes, and
    /// this is the GUI honoring it.
    #[test]
    fn a_legacy_single_matrix_view_still_loads() {
        let (time, product) = (CategoryId(1), CategoryId(2));
        // Exactly the shape a pre-canvas save produced.
        let legacy = View {
            id: ViewId(7),
            name: Name("Legacy".into()),
            measure: MeasureId(101),
            axis_order: vec![product, time],
            n_rows: 1,
            n_cols: 1,
            page_items: vec![],
            filters: vec![Filter {
                category: product,
                items: vec![ItemId(20)],
            }],
            rect: CanvasRect::default(),
            placements: vec![],
        };
        assert!(
            legacy.placements.is_empty(),
            "the fixture must be a pre-canvas view, or it proves nothing"
        );

        // Apply it to a canvas that currently has TWO matrices: a one-matrix view
        // must reduce the canvas to one, not leave a stray matrix behind.
        let mut app = two_matrix_app();
        assert_eq!(app.matrix_count(), 2);
        app.apply_view(&legacy);
        app.sync_axis_state();

        assert_eq!(
            app.matrix_count(),
            1,
            "a legacy view is a ONE-matrix canvas"
        );
        let m = &app.matrices[0];
        assert_eq!(m.measure, Some(MeasureId(101)));
        assert_eq!(
            m.row_cats(&app.model),
            vec![product],
            "saved pivot restored"
        );
        assert_eq!(m.col_cats(&app.model), vec![time]);
        assert_eq!(m.filters, legacy.filters, "saved filter restored");
        assert_eq!(
            m.rect,
            CanvasRect::default(),
            "no geometry saved -> the default rectangle, visible on the canvas"
        );
        // The filter really is in effect on the restored matrix.
        assert_eq!(
            m.sorted_items(&app.model, product)
                .iter()
                .map(|(id, _)| *id)
                .collect::<Vec<_>>(),
            vec![ItemId(20)]
        );
        // And re-saving it yields the same one-matrix shape (no phantom extras).
        let again = app.build_view(ViewId(8), "again").expect("view");
        assert!(
            again.placements.is_empty(),
            "a one-matrix canvas must still save as a one-matrix view"
        );
        assert_eq!(again.measure, MeasureId(101));
    }

    /// A view whose measure is gone is refused and leaves the canvas alone; a
    /// view where only SOME matrices' measures are gone loads the rest and says
    /// so, rather than showing a blank matrix.
    #[test]
    fn a_view_with_a_missing_measure_is_handled_not_shown_blank() {
        let mut app = two_matrix_app();
        let two = app.build_view(ViewId(1), "two").expect("view");
        // Drop the measure the EXTRA matrix shows.
        app.model.measures.remove(&MeasureId(102));
        app.apply_view(&two);
        assert_eq!(app.matrix_count(), 1, "the dead matrix was dropped");
        assert_eq!(app.matrices[0].measure, Some(MeasureId(101)));
        assert!(app.status.contains("skipped"), "got {:?}", app.status);

        // Now drop the primary one too: nothing is applicable, so nothing moves.
        let before = app.matrices.clone();
        app.model.measures.remove(&MeasureId(101));
        app.apply_view(&two);
        assert_eq!(app.matrix_count(), before.len());
        assert_eq!(app.status, "view's measure no longer exists");
    }

    /// **Step 3 acceptance, part 4:** only the FOCUSED matrix consumes grid
    /// shortcuts. An arrow key must move one cursor, not every cursor on the
    /// canvas — driven through `handle_grid_keys` in a real frame, per matrix
    /// index, which is how the canvas calls it.
    #[test]
    fn only_the_focused_matrix_consumes_grid_shortcuts() {
        let mut app = two_matrix_app();
        app.set_focus(0);
        assert_eq!(app.focus_index(), 0);

        use egui::Key;
        // Deliver the keys AT matrix 0 (the focused one): its cursor moves.
        frame_with_keys_for(&mut app, 0, &[Key::ArrowDown, Key::ArrowRight], None);
        assert_eq!(
            (app.matrices[0].cursor_row, app.matrices[0].cursor_col),
            (1, 1),
            "the focused matrix must take the keystroke"
        );
        assert_eq!(
            (app.matrices[1].cursor_row, app.matrices[1].cursor_col),
            (0, 0),
            "the unfocused matrix must not move"
        );

        // Deliver the SAME keys at matrix 1 while matrix 0 still has the focus:
        // nothing happens at all (the gate is focus, not which Ui rendered).
        //
        // Matrix 0's cursor is put back to the top-left first, so this also
        // catches the subtler bug: a handler that ignored `mi` and simply drove
        // "the focused matrix" would move matrix 0 from (0,0) here. (Asserting
        // against (1,1) could not see that — the 2x2 grid clamps there.)
        app.matrices[0].cursor_row = 0;
        app.matrices[0].cursor_col = 0;
        frame_with_keys_for(&mut app, 1, &[Key::ArrowDown, Key::ArrowRight], None);
        assert_eq!(
            (app.matrices[1].cursor_row, app.matrices[1].cursor_col),
            (0, 0),
            "an unfocused matrix must ignore keys aimed at it"
        );
        assert_eq!(
            (app.matrices[0].cursor_row, app.matrices[0].cursor_col),
            (0, 0),
            "...and must not forward them to the focused matrix either"
        );
        // Put it back where the first delivery left it for the rest of the test.
        app.matrices[0].cursor_row = 1;
        app.matrices[0].cursor_col = 1;

        // Move the focus: now matrix 1 takes them, and matrix 0 stops.
        app.set_focus(1);
        frame_with_keys_for(&mut app, 1, &[Key::ArrowDown], None);
        assert_eq!(
            (app.matrices[1].cursor_row, app.matrices[1].cursor_col),
            (1, 0),
            "the newly focused matrix takes the keystroke"
        );
        assert_eq!(
            (app.matrices[0].cursor_row, app.matrices[0].cursor_col),
            (1, 1),
            "the previously focused matrix froze where it was"
        );

        // Enter opens the cell editor on the focused matrix ONLY. Matrix 1 shows
        // derived Revenue, so focus matrix 0 (input Quantity) for this.
        app.set_focus(0);
        // Enter aimed at the UNFOCUSED matrix opens nothing anywhere.
        frame_with_keys_for(&mut app, 1, &[Key::Enter], None);
        assert!(
            app.matrices[0].editing.is_none() && app.matrices[1].editing.is_none(),
            "Enter aimed at an unfocused matrix must open no editor at all"
        );
        // Aimed at the focused one it opens that one's editor, and only that one's.
        frame_with_keys_for(&mut app, 0, &[Key::Enter], None);
        assert!(
            app.matrices[0].editing.is_some(),
            "Enter must open the focused matrix's editor"
        );
        assert!(
            app.matrices[1].editing.is_none(),
            "...and no other matrix's"
        );
    }

    /// **Step 3 acceptance, part 5:** each matrix's margin gutters frame ITS OWN
    /// table — Step 1's invariant, now per matrix, asserted on the rects a real
    /// headless frame recorded.
    ///
    /// Two matrices means two independent framed regions: this fails if the
    /// canvas renders one set of gutters, or if the second matrix's gutters land
    /// on the first matrix's table.
    #[test]
    fn every_matrix_on_the_canvas_frames_its_own_table() {
        let mut app = two_matrix_app();
        // Place them far apart so overlapping rects cannot make the assertion
        // accidentally true.
        app.matrices[0].rect = CanvasRect {
            x: 0.0,
            y: 0.0,
            w: 460.0,
            h: 300.0,
        };
        app.matrices[1].rect = CanvasRect {
            x: 0.0,
            y: 340.0,
            w: 460.0,
            h: 300.0,
        };
        let all = layout_all(&mut app);
        assert_eq!(all.len(), 2, "one gutter set per matrix");

        for (i, g) in all.iter().enumerate() {
            assert!(
                gutters_have_room(g),
                "matrix {i} must have room at this size: {g:?}"
            );
            assert!(
                gutters_frame_table(g),
                "matrix {i}'s gutters must frame ITS table: {g:?}"
            );
            // Step 1's two headline adjacencies, spelled out per matrix.
            assert!(
                (g.left.max.x - g.table.min.x).abs() <= EDGE_EPS,
                "matrix {i}: row gutter must adjoin its table's left edge: {g:?}"
            );
            assert!(
                (g.top.max.y - g.table.min.y).abs() <= EDGE_EPS,
                "matrix {i}: column gutter must adjoin its table's top edge: {g:?}"
            );
        }
        // The two framed regions are genuinely distinct: matrix 1's table is
        // below matrix 0's, not the same rect reported twice.
        assert!(
            all[1].table.min.y > all[0].table.min.y + 100.0,
            "the second matrix must frame a DIFFERENT table: {all:?}"
        );
        assert_ne!(all[0].top, all[1].top, "each matrix has its own gutters");
    }

    /// The CANVAS must not creep either: with N matrices on a scrollable surface,
    /// the geometry has to settle and stay put frame after frame.
    ///
    /// This is Step 1's `gutters_do_not_creep_across_frames_or_repivots`
    /// discipline applied to the container it now lives in — the reason
    /// `canvas_extent` is a pure function of the matrix rects rather than
    /// something derived from last frame's content size. A scroll area whose
    /// content grew by what it claimed last frame would drift forever, and a
    /// single-frame assertion cannot see it.
    #[test]
    fn the_canvas_does_not_creep_across_frames() {
        let mut app = two_matrix_app();
        let (ctx, mut pass) = layout_harness();
        // Settle (a panel reads its extent from the previous frame), then assert
        // the settled geometry is genuinely stationary.
        let settled = settle(&ctx, &mut app, &mut pass);
        for i in 0..10 {
            let now = pass(&ctx, &mut app);
            assert_eq!(now, settled, "the canvas drifted on frame {i}");
        }
        // Every matrix's geometry is stationary, not just the focused one's.
        let first: Vec<GutterRects> = app.matrices.iter().filter_map(|m| m.gutters).collect();
        assert_eq!(first.len(), 2);
        for _ in 0..5 {
            let _ = pass(&ctx, &mut app);
        }
        let again: Vec<GutterRects> = app.matrices.iter().filter_map(|m| m.gutters).collect();
        assert_eq!(first, again, "some matrix's geometry drifted");

        // And the canvas surface itself does not grow: it is a function of the
        // matrix rects, so asking twice with the same rects gives the same size.
        let avail = egui::vec2(800.0, 600.0);
        let a = app.canvas_extent(avail);
        let b = app.canvas_extent(avail);
        assert_eq!(a, b);
        // Moving a matrix right/down DOES grow it (the surface must follow the
        // content), which is what makes the constancy above meaningful.
        app.matrices[1].rect.x += 500.0;
        assert!(
            app.canvas_extent(avail).x > a.x,
            "the canvas must grow to hold a matrix moved off its right edge"
        );
    }

    /// Dragging a matrix can never put it somewhere unreachable: `move_by` stops
    /// at the canvas's top-left (the scroll surface starts there, so a negative
    /// offset could not be scrolled to) and `resize_by` stops at the minimum
    /// size (below which the matrix has no room for its own chrome, and at zero
    /// its rect would be degenerate).
    #[test]
    fn a_matrix_cannot_be_dragged_or_resized_out_of_reach() {
        let mut m = Matrix {
            rect: CanvasRect {
                x: 10.0,
                y: 20.0,
                w: 400.0,
                h: 300.0,
            },
            ..Matrix::default()
        };

        // Ordinary drags move it exactly as asked, in both directions.
        m.move_by(30.0, 40.0);
        assert_eq!((m.rect.x, m.rect.y), (40.0, 60.0));
        m.move_by(-15.0, -25.0);
        assert_eq!((m.rect.x, m.rect.y), (25.0, 35.0));

        // A drag past the top-left corner stops AT the corner, never past it.
        m.move_by(-1000.0, -1000.0);
        assert_eq!(
            (m.rect.x, m.rect.y),
            (0.0, 0.0),
            "a matrix must not be draggable off the canvas"
        );
        // ...and it is still draggable back out, not stuck.
        m.move_by(50.0, 60.0);
        assert_eq!((m.rect.x, m.rect.y), (50.0, 60.0));

        // Resizing shrinks and grows, but never below the minimum.
        m.resize_by(-50.0, -50.0);
        assert_eq!((m.rect.w, m.rect.h), (350.0, 250.0));
        m.resize_by(-10_000.0, -10_000.0);
        assert_eq!(
            (m.rect.w, m.rect.h),
            (MATRIX_MIN_W, MATRIX_MIN_H),
            "a matrix must not be shrinkable past its own chrome"
        );
        assert!(m.rect.w > 0.0 && m.rect.h > 0.0, "never a degenerate rect");
        m.resize_by(100.0, 100.0);
        assert_eq!(
            (m.rect.w, m.rect.h),
            (MATRIX_MIN_W + 100.0, MATRIX_MIN_H + 100.0),
            "...and growing again still works"
        );
    }

    /// The canvas lays out at every window size without panicking, and the
    /// per-frame framing `debug_assert!` inside `gutter_frame` runs on every
    /// matrix at every size (the Step 1 discipline, extended to N matrices).
    #[test]
    fn the_canvas_lays_out_at_every_window_size() {
        for (w, h) in [
            (1400.0, 900.0),
            (1000.0, 700.0),
            (600.0, 400.0),
            (200.0, 150.0),
            (40.0, 30.0),
        ] {
            let mut app = two_matrix_app();
            let ctx = egui::Context::default();
            ctx.set_style(crate::theme::next_style());
            for i in 0..6 {
                let raw = egui::RawInput {
                    time: Some(f64::from(i) / 60.0),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(w, h),
                    )),
                    ..Default::default()
                };
                let _ = ctx.run(raw, |ctx| {
                    app.sync_axis_state();
                    app.status_bar(ctx);
                    app.document_tab_bar(ctx);
                    app.canvas_panel(ctx);
                });
            }
            assert_eq!(app.matrix_count(), 2, "{w}x{h}: both matrices survive");
        }
    }

    /// Adding and removing matrices is canvas layout, never a model change:
    /// nothing becomes an undo point, and the last matrix cannot be removed.
    ///
    /// **The undo decision** (recorded here because a test is the only place it
    /// cannot rot): undo/redo snapshots `Model`, and canvas layout lives in the
    /// `View`, not the `Model` — so placing, moving, resizing or removing a
    /// matrix is deliberately NOT undoable. Saving the canvas as a view IS a
    /// model change and IS undoable, which is what makes a layout recoverable:
    /// save the canvas, and Ctrl+Z reaches it like any other model edit.
    #[test]
    fn canvas_layout_is_not_an_undo_point_but_saving_it_is() {
        let mut app = build_app(grid_2x2_model());
        assert!(!app.can_undo());

        app.add_matrix(Some(MeasureId(101)));
        assert_eq!(app.matrix_count(), 2);
        assert!(
            !app.can_undo(),
            "placing a matrix is layout, not a model change"
        );

        // Moving and resizing likewise.
        app.matrices[1].rect.x += 40.0;
        app.matrices[1].rect.w += 60.0;
        assert!(!app.can_undo());

        assert!(app.remove_matrix(1));
        assert_eq!(app.matrix_count(), 1);
        assert!(!app.can_undo(), "removing a matrix is layout too");

        // The last matrix is never removed: there would be nowhere to show a
        // measure, and `focused()` must always have a matrix to return.
        assert!(!app.remove_matrix(0), "the last matrix must stay");
        assert_eq!(app.matrix_count(), 1);
        assert!(!app.remove_matrix(9), "an out-of-range index is a no-op");

        // Saving the canvas as a view IS a model change, so it IS undoable —
        // which is how a layout becomes recoverable.
        app.add_matrix(Some(MeasureId(102)));
        let id = app.save_view("Canvas").expect("saved");
        assert!(app.can_undo(), "saving a view must be an undo point");
        app.undo().expect("undo the save");
        assert!(
            !app.model.views.contains_key(&id),
            "undo removed the saved view"
        );
        assert_eq!(
            app.matrix_count(),
            2,
            "undoing the SAVE does not tear down the canvas it described"
        );
    }

    /// Removing a matrix never leaves the focus dangling: `focused()` must keep
    /// returning a real matrix, whichever one went away.
    #[test]
    fn focus_survives_removing_matrices() {
        let mut app = build_app(grid_2x2_model());
        app.add_matrix(Some(MeasureId(101)));
        app.add_matrix(Some(MeasureId(102)));
        assert_eq!(app.matrix_count(), 3);
        assert_eq!(app.focus_index(), 2, "a new matrix takes the focus");

        // Remove the focused (last) one: focus falls back inside the range.
        app.remove_matrix(2);
        assert_eq!(app.focus_index(), 1);
        assert_eq!(app.focused().measure, app.matrices[1].measure);

        // Remove an EARLIER one while focused on a later one.
        app.set_focus(1);
        app.remove_matrix(0);
        assert_eq!(app.matrix_count(), 1);
        assert_eq!(app.focus_index(), 0);
        // set_focus clamps rather than panicking on a bogus index.
        app.set_focus(99);
        assert_eq!(app.focus_index(), 0);
    }

    /// The formula bar, inspector and chart all follow the FOCUSED matrix: they
    /// are single-measure surfaces, and on a canvas "the selected measure" can
    /// only mean "the one with the keyboard".
    #[test]
    fn single_measure_surfaces_follow_the_focus() {
        let mut app = two_matrix_app();
        app.set_focus(0);
        assert_eq!(app.selected(), Some(MeasureId(101)), "Quantity");
        assert_eq!(
            app.inspector_data(app.selected().unwrap()).unwrap().name,
            "Quantity"
        );
        // Quantity is an input measure: no chart-worthy formula, but a chart.
        let q_chart = app.chart_series();

        app.set_focus(1);
        assert_eq!(app.selected(), Some(MeasureId(102)), "Revenue");
        let d = app.inspector_data(app.selected().unwrap()).unwrap();
        assert_eq!(d.name, "Revenue");
        assert!(d.is_derived, "the inspector moved to the derived measure");
        assert_eq!(
            app.formula_source(MeasureId(102)).as_deref(),
            Some("Price * Quantity")
        );
        // The chart followed too: different measure, different numbers.
        let r_chart = app.chart_series();
        assert_ne!(
            q_chart.series[0].points, r_chart.series[0].points,
            "the chart must plot the focused matrix's measure"
        );
    }

    // -- STEP 5: chrome (title bars, selection readout, document tabs)

    /// **Step 5:** each matrix has its own title bar naming its measure, and —
    /// like the reference's `Property Financials: Virginia Ave` — the slice it
    /// shows, i.e. its pinned page items.
    #[test]
    fn each_matrix_has_its_own_title_naming_its_slice() {
        let mut app = build_app(sales_3d_model());
        select(&mut app, MeasureId(200)); // Sales[Time, Product, Region]
        app.sync_axis_state();
        // One page dim (Region) pinned to North.
        assert_eq!(app.matrix_title(0), "Sales: North");

        // Page to South: the title follows the slice, which is the point of
        // putting it there.
        let model = app.model.clone();
        app.matrices[0].set_page(&model, 0, 1);
        assert_eq!(app.matrix_title(0), "Sales: South");

        // A second matrix on the same measure, pinned differently, gets its own
        // title — two matrices are two slices, and the title bars say which.
        app.add_matrix(Some(MeasureId(200)));
        app.sync_axis_state();
        assert_eq!(app.matrix_title(1), "Sales: North");
        assert_eq!(app.matrix_title(0), "Sales: South");

        // With no page dimension there is no slice to name, so the title is just
        // the measure.
        let mut app = build_app(grid_2x2_model());
        select(&mut app, MeasureId(101));
        app.sync_axis_state();
        assert_eq!(app.matrix_title(0), "Quantity");
        assert_eq!(app.matrix_title(99), "", "an out-of-range index is empty");
    }

    /// **Step 5:** the status bar's selection-aggregate readout (the reference's
    /// bottom-right `Sum`), computed from the FOCUSED matrix's selection.
    ///
    /// Selection is one cell today, so the sum is that cell's number; a
    /// non-numeric or absent cell reads `Sum —` rather than a fabricated 0.
    #[test]
    fn the_status_bar_reads_out_the_focused_selection_sum() {
        let mut app = two_matrix_app();
        // Matrix 0 = Quantity, cursor on [2025, WidgetA] = 100.
        app.set_focus(0);
        assert_eq!(app.selection_sum(), Some(100.0));
        assert_eq!(app.selection_readout(), "Sum 100");

        // Move that matrix's cursor: the readout follows it.
        let model = app.model.clone();
        app.matrices[0].move_cursor(&model, 1, 0); // [2026, WidgetA] = 120
        assert_eq!(app.selection_readout(), "Sum 120");

        // Focus the OTHER matrix: the readout switches to its selection, which is
        // a derived cell (Revenue[2025, WidgetA] = 10 * 100 = 1000).
        app.set_focus(1);
        assert_eq!(app.selection_sum(), Some(1000.0));
        assert_eq!(app.selection_readout(), "Sum 1000");

        // An empty cell has nothing to sum.
        app.model
            .inputs
            .remove(&(MeasureId(101), decode(&app.cursor_key().unwrap())));
        app.rebuild_engine();
        assert_eq!(app.selection_sum(), None);
        assert_eq!(app.selection_readout(), "Sum \u{2014}");
    }

    /// A non-numeric selection reads `Sum —`: the aggregate is a SUM, and text
    /// does not sum. (Its cell still renders its text in the grid — see
    /// `typed_input_cells_render_their_values`.)
    #[test]
    fn a_non_numeric_selection_has_no_sum() {
        let mut app = build_app(typed_model());
        select(&mut app, MeasureId(2)); // Label (Text)
        app.sync_axis_state();
        assert_eq!(
            app.cell_text(MeasureId(2), &typed_key()).as_deref(),
            Some("hello")
        );
        assert_eq!(app.selection_sum(), None, "text has no sum");
        assert_eq!(app.selection_readout(), "Sum \u{2014}");
    }

    /// **Step 5:** document tabs, one per saved view (the reference's
    /// `Welcome / Concepts / P&L Canvas`), and clicking one switches CANVASES —
    /// not just a measure.
    #[test]
    fn document_tabs_switch_between_saved_canvases() {
        let mut app = build_app(grid_2x2_model());
        assert!(app.document_tabs().is_empty(), "no views, no tabs");

        // Canvas A: one matrix, Quantity.
        select(&mut app, MeasureId(101));
        app.sync_axis_state();
        let a = app.save_view("Canvas A").expect("saved A");
        // Canvas B: two matrices, the second showing Revenue pivoted.
        app.add_matrix(Some(MeasureId(102)));
        app.sync_axis_state();
        let model = app.model.clone();
        app.matrices[1].pivot_rotate(&model);
        let b = app.save_view("Canvas B").expect("saved B");

        // Both tabs are listed, in id order, with B current (just saved).
        let tabs = app.document_tabs();
        assert_eq!(
            tabs.iter().map(|(_, n, _)| n.as_str()).collect::<Vec<_>>(),
            vec!["Canvas A", "Canvas B"]
        );
        assert_eq!(
            tabs.iter()
                .filter(|(_, _, cur)| *cur)
                .map(|(id, _, _)| *id)
                .collect::<Vec<_>>(),
            vec![b],
            "the just-saved view is the current tab"
        );

        // Click tab A: the canvas becomes A's — ONE matrix.
        app.open_view(a);
        app.sync_axis_state();
        assert_eq!(app.matrix_count(), 1, "Canvas A has one matrix");
        assert_eq!(app.matrices[0].measure, Some(MeasureId(101)));
        assert!(
            app.document_tabs()
                .iter()
                .any(|(id, _, cur)| *id == a && *cur),
            "tab A is now current"
        );

        // Click tab B: two matrices again, with the pivot B was saved with.
        app.open_view(b);
        app.sync_axis_state();
        assert_eq!(app.matrix_count(), 2, "Canvas B has two matrices");
        assert_eq!(
            app.matrices[1].row_cats(&app.model),
            vec![CategoryId(2)],
            "B's second matrix came back pivoted"
        );

        // An unknown id is a no-op, not a panic or an empty canvas.
        let before = app.matrix_count();
        app.open_view(ViewId(999));
        assert_eq!(app.matrix_count(), before);
    }

    /// A unique temp-db path for a store-backed test.
    fn temp_db(tag: &str) -> String {
        std::env::temp_dir()
            .join(format!(
                "improv_gui_{tag}_{}_{}.db",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .expect("clock")
                    .as_nanos()
            ))
            .to_string_lossy()
            .into_owned()
    }

    /// Delete a temp db when the test ends, pass or panic.
    struct Cleanup(String);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
}
