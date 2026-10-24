//! App state and the pure pivot-grid logic (terminal-free, unit-tested).
//!
//! A measure is a tensor over N categories. We render the first two categories
//! as the row/column axes; any remaining categories are "pages" pinned to their
//! first item (v1: fixed, shown on a status line). Values come from the model's
//! input cells (input measures) or from `engine::evaluate` (derived measures),
//! both keyed by `CoordKey = Vec<(u32, u32)>`.

use crate::complete;
use crate::edit::LineEdit;
use crossterm::event::KeyCode;
#[cfg(test)]
use improv_core_model::Coordinate;
use improv_core_model::{
    parser, parser::Definition, CategoryId, Filter, Formula, ItemId, Measure, MeasureId,
    MeasureKind, Model, Name, ParseError, Value, ValueType, View, ViewId,
};
use improv_engine::session::Engine;
use improv_engine::{decode_coord, encode_coord, CellValue, CoordKey};
use improv_storage_csv::{export_measure_csv, import_csv, ColumnRef, DimensionMapping, ImportSpec};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Live derived-measure values, keyed by measure then coordinate. Kept in
/// `App` and refreshed incrementally by the `Engine` on each committed edit;
/// used to render derived measures instead of a fresh full `evaluate()`.
pub type Snapshot = improv_engine::session::Snapshot;

/// One rendered pivot grid for a single measure.
pub struct Grid {
    pub measure: MeasureId,
    pub measure_name: String,
    /// Category on the rows axis (None if the measure has 0 categories).
    pub row_cat: Option<(CategoryId, String)>,
    /// Category on the columns axis (None if the measure has < 2 categories).
    pub col_cat: Option<(CategoryId, String)>,
    /// Row header items: (id, name). Length >= 1 (a synthetic single row when
    /// there is no row category).
    pub rows: Vec<(ItemId, String)>,
    /// Column header items: (id, name). Length >= 1.
    pub cols: Vec<(ItemId, String)>,
    /// Categories beyond the first two, pinned to a chosen item ("pages").
    pub pages: Vec<PageDim>,
    /// Cell values indexed `[row][col]`. `None` = empty cell. Typed
    /// (`CellValue`) so non-numeric cells (Text/Bool/Date/#ERR) render
    /// correctly, not just numbers.
    pub cells: Vec<Vec<Option<CellValue>>>,
}

/// An extra dimension not on the row/column axes, pinned to one item (a
/// "page"). `item_index`/`item_count` let the UI page through the dimension's
/// items with `[` / `]`.
pub struct PageDim {
    pub cat: CategoryId,
    pub cat_name: String,
    pub item_name: String,
    pub item: ItemId,
    pub item_index: usize,
    pub item_count: usize,
}

/// Geometry of the last-rendered grid, recorded by `ui::render` so mouse
/// clicks (screen col/row) can be mapped back to a grid cell.
#[derive(Clone, Copy, Debug)]
pub struct GridGeom {
    /// Screen coords of the top-left DATA cell (row 0, col 0).
    pub x0: u16,
    pub y0: u16,
    /// Width of each data column and the inter-column spacing.
    pub col_w: u16,
    pub spacing: u16,
    /// First visible data row/column (the scroll offset).
    pub row0: usize,
    pub col0: usize,
    /// How many data rows/columns the window shows.
    pub rows: usize,
    pub cols: usize,
}

impl Grid {
    pub fn n_rows(&self) -> usize {
        self.rows.len()
    }
    pub fn n_cols(&self) -> usize {
        self.cols.len()
    }
    /// The numeric value at `[row][col]`, if the cell holds a number (for the
    /// edit-seed path). Non-numeric cells return `None`.
    pub fn value_at(&self, row: usize, col: usize) -> Option<f64> {
        self.cell_at(row, col).and_then(|v| v.as_num())
    }
    /// The typed cell value at `[row][col]`.
    pub fn cell_at(&self, row: usize, col: usize) -> Option<&CellValue> {
        self.cells
            .get(row)
            .and_then(|r| r.get(col))
            .and_then(|c| c.as_ref())
    }
    /// The display string for `[row][col]` (empty for an empty cell).
    pub fn display_at(&self, row: usize, col: usize) -> String {
        match self.cell_at(row, col) {
            Some(v) => v.to_string(),
            None => String::new(),
        }
    }
    /// The sorted `CoordKey` for the cell at `[row][col]`.
    pub fn coord_key(&self, row: usize, col: usize) -> CoordKey {
        cell_key(
            &self.row_cat,
            &self.col_cat,
            &self.rows,
            &self.cols,
            &self.pages,
            row,
            col,
        )
    }
}

/// Item names for a category, in the category's declared item order, keeping
/// only items that pass `filters` (a category absent from `filters` is
/// unfiltered). Presentation-only: it hides items, it never touches data.
fn items_of(model: &Model, cat: CategoryId, filters: &[Filter]) -> Vec<(ItemId, String)> {
    let keep = |id: ItemId| match filters.iter().find(|f| f.category == cat) {
        Some(f) => f.items.contains(&id),
        None => true,
    };
    model
        .categories
        .get(&cat)
        .map(|c| {
            c.items
                .iter()
                .filter(|id| keep(**id))
                .map(|id| {
                    let name = model
                        .items
                        .get(id)
                        .map(|it| it.name.0.clone())
                        .unwrap_or_else(|| id.0.to_string());
                    (*id, name)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The value map for a measure: `CoordKey -> CellValue`.
///
/// Input measures read straight from `model.inputs`; derived measures come from
/// the live engine `snapshot` (empty map -> grid renders blank). Values keep
/// their type so the grid can render numbers, text, booleans, dates, and #ERR.
fn values_for(
    model: &Model,
    measure: MeasureId,
    snapshot: &Snapshot,
) -> HashMap<CoordKey, CellValue> {
    let is_derived = model.measures.get(&measure).map(|m| m.is_derived());
    match is_derived {
        // Derived: read the live engine snapshot (already typed CellValues).
        Some(true) => snapshot
            .get(&measure)
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
        // Input: read model cells, mapping each typed model Value to a CellValue.
        _ => model
            .inputs
            .iter()
            .filter(|((mid, _), _)| *mid == measure)
            .filter_map(|((_, coord), v)| {
                CellValue::from_model_value(v).map(|cv| (encode_coord(coord), cv))
            })
            .collect(),
    }
}

/// Build the pivot grid for `measure`. Uses the measure's first two categories
/// as the row/column axes and pins the rest to their first item.
pub fn build_grid(model: &Model, measure: MeasureId, snapshot: &Snapshot) -> Grid {
    build_grid_paged(model, measure, snapshot, &[])
}

/// Like [`build_grid`] but pins each extra (page) dimension to the item at the
/// given index (clamped). `page_idx[i]` selects the item for the i-th extra
/// dimension (categories beyond the first two); a missing/short entry defaults
/// to 0. Enables paging through a 3+ dimensional measure.
pub fn build_grid_paged(
    model: &Model,
    measure: MeasureId,
    snapshot: &Snapshot,
    page_idx: &[usize],
) -> Grid {
    build_grid_pivoted(model, measure, snapshot, page_idx, None, &[])
}

/// The most general grid builder. `axis_order`, when `Some`, is a permutation
/// of the measure's categories that sets which category is on rows (first),
/// columns (second), and pages (rest) — this is how the UI *pivots* without
/// touching formulas. `None` uses the measure's natural category order.
/// `filters` restricts which items of a category are shown (presentation only;
/// unlisted categories show all items).
pub fn build_grid_pivoted(
    model: &Model,
    measure: MeasureId,
    snapshot: &Snapshot,
    page_idx: &[usize],
    axis_order: Option<&[CategoryId]>,
    filters: &[Filter],
) -> Grid {
    let m = model.measures.get(&measure);
    let measure_name = m
        .map(|m| m.name.0.clone())
        .unwrap_or_else(|| measure.0.to_string());
    let natural: Vec<CategoryId> = m.map(|m| m.categories.clone()).unwrap_or_default();
    // Use the requested axis order if it is a valid permutation of the measure's
    // categories; otherwise fall back to natural order (defensive).
    let cats: Vec<CategoryId> = match axis_order {
        Some(order)
            if order.len() == natural.len() && natural.iter().all(|c| order.contains(c)) =>
        {
            order.to_vec()
        }
        _ => natural,
    };

    let cat_name = |c: CategoryId| {
        model
            .categories
            .get(&c)
            .map(|x| x.name.0.clone())
            .unwrap_or_else(|| c.0.to_string())
    };

    let row_cat = cats.first().map(|c| (*c, cat_name(*c)));
    let col_cat = cats.get(1).map(|c| (*c, cat_name(*c)));

    // Row / column header items (synthetic single entry when the axis is absent).
    let rows = match row_cat {
        Some((c, _)) => items_of(model, c, filters),
        None => vec![(ItemId(0), String::new())],
    };
    let cols = match col_cat {
        Some((c, _)) => items_of(model, c, filters),
        None => vec![(ItemId(0), String::new())],
    };

    // Extra dims pinned to the selected page item (default first).
    let mut pages = Vec::new();
    for c in cats.iter().skip(2) {
        let its = items_of(model, *c, filters);
        if its.is_empty() {
            continue;
        }
        // Indexed by rendered page (empty dims are skipped), matching how
        // `App` fills `page_idx` from `grid.pages`.
        let sel = page_idx
            .get(pages.len())
            .copied()
            .unwrap_or(0)
            .min(its.len() - 1);
        let (id, name) = its[sel].clone();
        pages.push(PageDim {
            cat: *c,
            cat_name: cat_name(*c),
            item_name: name,
            item: id,
            item_index: sel,
            item_count: its.len(),
        });
    }

    let values = values_for(model, measure, snapshot);

    // Fill cells by looking up each cell's coordinate key.
    let mut cells = Vec::with_capacity(rows.len());
    for r in 0..rows.len() {
        let mut row_cells = Vec::with_capacity(cols.len());
        for c in 0..cols.len() {
            let key = cell_key(&row_cat, &col_cat, &rows, &cols, &pages, r, c);
            row_cells.push(values.get(&key).cloned());
        }
        cells.push(row_cells);
    }

    Grid {
        measure,
        measure_name,
        row_cat,
        col_cat,
        rows,
        cols,
        pages,
        cells,
    }
}

/// The sorted `CoordKey` for the cell at `[row][col]` given the axes/pages.
/// Shared by grid fill and cursor->coordinate mapping so they can't drift.
#[allow(clippy::too_many_arguments)]
fn cell_key(
    row_cat: &Option<(CategoryId, String)>,
    col_cat: &Option<(CategoryId, String)>,
    rows: &[(ItemId, String)],
    cols: &[(ItemId, String)],
    pages: &[PageDim],
    row: usize,
    col: usize,
) -> CoordKey {
    let mut key = CoordKey::new();
    if let (Some((c, _)), Some((item, _))) = (row_cat, rows.get(row)) {
        key.push((c.0, item.0));
    }
    if let (Some((c, _)), Some((item, _))) = (col_cat, cols.get(col)) {
        key.push((c.0, item.0));
    }
    for p in pages {
        key.push((p.cat.0, p.item.0));
    }
    key.sort();
    key
}

/// Status shown when the cursor is not on a real cell (empty/filtered axis).
const NO_CELL: &str = "no cell here (an axis is empty or fully filtered)";

/// Which CSV command is active in the command-prompt (`self.command`).
/// Mirrors the cell-edit buffer (`self.edit`) but for a full command line
/// instead of a single value, and covers the two storage_csv entry points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    ImportCsv,
    ExportCsv,
    /// `c`: `Name` or `Name: item, item, …`, a category and its items.
    Category,
    /// `n`: `Name` or `Name: Category, …`, a number input measure.
    Input,
}

/// Shown in place of the measure line while the model has no measures.
pub const EMPTY: &str =
    "no measures yet: c adds a category and its items, n an input measure, = a formula";

/// Split a `Name: a, b, …` prompt line into the trimmed name and the
/// non-empty trimmed entries after the colon.
fn name_and_list(line: &str) -> (&str, Vec<&str>) {
    let (name, rest) = line.split_once(':').unwrap_or((line, ""));
    let list = rest.split(',').map(str::trim).filter(|s| !s.is_empty());
    (name.trim(), list.collect())
}

/// The smallest id above every one of `ids` (1 for none).
fn next_id(ids: impl Iterator<Item = u32>) -> u32 {
    ids.max().map_or(1, |m| m + 1)
}

/// The name a definition line defines: the text before its `=` (or its
/// target dimension list), unquoted. Parsing needs it before it parses, so
/// the new measure can refer to itself.
fn definition_target(text: &str) -> String {
    let lhs = text.split(['=', '[']).next().unwrap_or_default();
    lhs.trim().trim_matches('\'').to_string()
}

/// Parse an `import-csv>` prompt line into an [`ImportSpec`]. Mirrors the
/// CLI's `import-csv <db> <file.csv> <measure-id> <measure-name> <value-col>
/// <dim-col:cat-id:cat-name> [...] [--tsv] [--no-header]` syntax, minus the
/// leading `<db>` (the TUI's already-open model plays that role). Whitespace-
/// separated, like the CLI's own argv; no quoting support (a filename with a
/// space isn't representable, matching the CLI's own simplicity).
///
/// Terminal-free -> unit-testable without a running TUI.
pub fn parse_import_line(line: &str) -> Result<ImportSpec, String> {
    let has_header = !line.split_whitespace().any(|t| t == "--no-header");
    let tsv_flag = line.split_whitespace().any(|t| t == "--tsv");
    let positional: Vec<&str> = line
        .split_whitespace()
        .filter(|t| *t != "--tsv" && *t != "--no-header")
        .collect();
    if positional.len() < 5 {
        return Err("usage: <file.csv> <measure-id> <measure-name> <value-col> \
             <dim-col:cat-id:cat-name> [...] [--tsv] [--no-header]"
            .to_string());
    }
    let path = positional[0];
    let measure_id = positional[1]
        .parse::<u32>()
        .map_err(|_| format!("<measure-id> must be a number, got '{}'", positional[1]))?;
    let measure_name = positional[2].to_string();
    let value_col = positional[3];

    let tsv = tsv_flag || path.ends_with(".tsv");
    let delimiter = if tsv { b'\t' } else { b',' };
    let col_ref = |s: &str| -> ColumnRef {
        if has_header {
            ColumnRef::Name(s.to_string())
        } else {
            s.parse::<usize>()
                .map(ColumnRef::Index)
                .unwrap_or_else(|_| ColumnRef::Name(s.to_string()))
        }
    };

    let mut dimensions = Vec::new();
    for spec in &positional[4..] {
        let parts: Vec<&str> = spec.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(format!(
                "dimension spec '{spec}' must be <col>:<cat-id>:<cat-name>"
            ));
        }
        let cat_id = parts[1]
            .parse::<u32>()
            .map_err(|_| format!("<cat-id> must be a number, got '{}'", parts[1]))?;
        dimensions.push(DimensionMapping {
            column: col_ref(parts[0]),
            category_id: CategoryId(cat_id),
            category_name: parts[2].to_string(),
        });
    }

    Ok(ImportSpec {
        path: PathBuf::from(path),
        delimiter,
        has_header,
        measure_id: MeasureId(measure_id),
        measure_name,
        value_type: ValueType::Number,
        value_column: col_ref(value_col),
        dimensions,
        item_id_base: 1_000_000, // above hand-assigned ids; avoids collisions
        max_records: None,
    })
}

/// Parse an `export-csv>` prompt line into `(target-path, measure-id,
/// delimiter)`. Mirrors the CLI's `export-csv <db> <target.csv>
/// <measure-id> [--tsv]`, minus `<db>`.
pub fn parse_export_line(line: &str) -> Result<(String, MeasureId, u8), String> {
    let tsv_flag = line.split_whitespace().any(|t| t == "--tsv");
    let positional: Vec<&str> = line.split_whitespace().filter(|t| *t != "--tsv").collect();
    if positional.len() < 2 {
        return Err("usage: <target.csv> <measure-id> [--tsv]".to_string());
    }
    let target = positional[0].to_string();
    let measure_id = positional[1]
        .parse::<u32>()
        .map_err(|_| format!("<measure-id> must be a number, got '{}'", positional[1]))?;
    let tsv = tsv_flag || target.ends_with(".tsv");
    let delimiter = if tsv { b'\t' } else { b',' };
    Ok((target, MeasureId(measure_id), delimiter))
}

/// Parse an edit buffer as a `vt`-typed value (mirrors the CLI's `set`).
pub fn parse_typed(text: &str, vt: ValueType) -> Result<Value, String> {
    match vt {
        ValueType::Number => text
            .parse()
            .map(Value::Number)
            .map_err(|_| format!("not a number: '{text}'")),
        ValueType::Boolean => match text {
            "true" => Ok(Value::Boolean(true)),
            "false" => Ok(Value::Boolean(false)),
            _ => Err(format!("not true|false: '{text}'")),
        },
        ValueType::Text => Ok(Value::Text(text.to_string())),
        other => Err(format!("cannot edit a {other:?} cell here")),
    }
}

/// Parse formula text as the symbolic DSL (`Price * Quantity`), else as
/// controlled English (`Price times Quantity`, what the status line shows).
/// If both fail, the DSL's error: it carries the position.
pub fn parse_formula_text(model: &Model, text: &str) -> Result<Formula, ParseError> {
    parser::parse_expr(model, text).or_else(|e| {
        improv_nl_formula::parse_nl_formula(&improv_nl_formula::NlContext::new(model), text)
            .map_err(|_| e)
    })
}

/// Measures sorted for stable cycling: derived first, then by id.
pub fn measure_order(model: &Model) -> Vec<MeasureId> {
    let mut ids: Vec<MeasureId> = model.measures.keys().copied().collect();
    ids.sort_by_key(|id| {
        let derived = model
            .measures
            .get(id)
            .map(|m| m.is_derived())
            .unwrap_or(false);
        (!derived, id.0) // derived (false sorts first) then ascending id
    });
    ids
}

/// A live engine over every derived measure of `model`; none when it has no
/// derived measures. Fails with the engine's error when a formula does not
/// build (a cycle, a type error).
fn build_engine(model: &Model) -> Result<(Option<Engine>, Snapshot), String> {
    let derived: Vec<MeasureId> = measure_order(model)
        .into_iter()
        .filter(|id| model.measures[id].is_derived())
        .collect();
    if derived.is_empty() {
        return Ok(Default::default());
    }
    let (e, s) = Engine::new(model, &derived).map_err(|e| e.to_string())?;
    Ok((Some(e), s))
}

/// How many changes `u` can undo.
///
/// shortcut: history is whole-`Model` clones (the GUI's approach), so memory
/// is up to 50 × the model; switch to per-edit inverse operations behind
/// `record_history`/`undo`/`redo` if big models make that hurt.
const UNDO_DEPTH: usize = 50;

/// Push `state` onto a bounded history stack, dropping the oldest entry once
/// [`UNDO_DEPTH`] is reached.
fn push_bounded(stack: &mut Vec<Model>, state: Model) {
    if stack.len() >= UNDO_DEPTH {
        stack.remove(0);
    }
    stack.push(state);
}

/// The whole application state.
///
/// The live `Engine` tracks **all** derived measures in the model up front
/// (built once in `new`), so switching the viewed measure never rebuilds it and
/// there is no per-measure rebuild logic. Editing an input cell pushes a delta
/// through the engine and the returned snapshot is kept here to render derived
/// measures. A model with zero derived measures gets no engine (`None`); input
/// edits still work, there is just nothing to recompute.
pub struct App {
    pub model: Model,
    pub measures: Vec<MeasureId>,
    pub selected: usize, // index into `measures`
    pub grid: Grid,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub should_quit: bool,
    /// Live incremental engine over all derived measures (None if there are none).
    pub engine: Option<Engine>,
    /// Latest derived-measure values from the engine.
    pub snapshot: Snapshot,
    /// The last saved view applied via `cycle_view`, so cycling advances.
    pub applied_view: Option<ViewId>,
    /// When `Some`, edit mode is active and this holds the in-progress text:
    /// a cell value for an input measure, the formula for a derived one.
    pub edit: Option<LineEdit>,
    /// Transient status/error message (e.g. "derived cells are computed").
    pub status: Option<String>,
    /// The store high-water the model was loaded/last saved at; saves refuse
    /// to clobber another process's writes (`StoreError::Conflict`).
    pub save_token: improv_store::SaveToken,
    /// The model as last loaded or saved (`None`: never), so a save writes
    /// only what differs from it ([`improv_store::ChangeSet::between`]).
    pub saved: Option<Model>,
    /// Selected item index for each extra (page) dimension of the viewed
    /// measure; `[` / `]` cycle the first one. Reset when the measure changes.
    pub page_idx: Vec<usize>,
    /// Current axis order (a permutation of the viewed measure's categories):
    /// element 0 is on rows, 1 on columns, the rest are pages. Pivoting mutates
    /// this; it resets to the measure's natural order on measure switch.
    pub axis_order: Vec<CategoryId>,
    /// Active per-category display filters for the current layout (which items
    /// of a category are shown). Presentation only — never changes model data.
    /// Captured when saving a view; reset on measure switch.
    pub filters: Vec<Filter>,
    /// Geometry of the last render, for mapping mouse clicks to cells.
    pub grid_geom: Option<GridGeom>,
    /// Fixed corner of a Shift+arrow range selection; `None` = one cell.
    pub anchor: Option<(usize, usize)>,
    /// When `Some`, the CSV import/export command prompt is active: which
    /// command and its in-progress typed line. Mirrors `edit`'s shape but for
    /// a whole command line rather than a single cell value.
    pub command: Option<(CommandKind, String)>,
    /// When `Some`, the function help overlay (`?` / F1) is open.
    pub help: Option<crate::fnhelp::FnHelp>,
    /// The candidates a repeated Tab cycles through; reset by any other key.
    pub tab_cycle: Option<complete::Cycle>,
    /// Where the last formula commit failed to parse, while its text is
    /// unchanged: the edit line underlines the span and shows the message.
    pub diagnostic: Option<crate::diagnose::Diagnostic>,
    /// Undo/redo history: whole-`Model` snapshots, oldest first, as the GUI
    /// keeps them. `undo_stack` holds the model before each change,
    /// `redo_stack` the models undone away. See [`App::undo`].
    pub undo_stack: Vec<Model>,
    pub redo_stack: Vec<Model>,
    /// The edit line is a `define>` line (`=`), not the viewed cell or formula.
    pub defining: bool,
}

impl App {
    pub fn new(model: Model) -> Result<App, String> {
        let measures = measure_order(&model);

        // Track every derived measure in one engine, built once.
        let (engine, snapshot) = build_engine(&model)?;

        let selected = 0;
        let natural_axes = |mid: MeasureId| -> Vec<CategoryId> {
            model
                .measures
                .get(&mid)
                .map(|m| m.categories.clone())
                .unwrap_or_default()
        };
        // An empty model shows a placeholder grid for a measure that is not there.
        let first = measures.first().copied().unwrap_or(MeasureId(0));
        let axis_order = natural_axes(first);
        let grid = build_grid(&model, first, &snapshot);
        let page_idx = vec![0; grid.pages.len()];
        Ok(App {
            model,
            measures,
            selected,
            grid,
            cursor_row: 0,
            cursor_col: 0,
            should_quit: false,
            engine,
            snapshot,
            applied_view: None,
            edit: None,
            status: None,
            save_token: improv_store::SaveToken::default(),
            saved: None,
            page_idx,
            axis_order,
            filters: Vec::new(),
            grid_geom: None,
            anchor: None,
            command: None,
            help: None,
            tab_cycle: None,
            diagnostic: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            defining: false,
        })
    }

    fn reselect(&mut self) {
        self.grid = build_grid_pivoted(
            &self.model,
            self.viewed_measure(),
            &self.snapshot,
            &self.page_idx,
            Some(&self.axis_order),
            &self.filters,
        );
        self.cursor_row = self.cursor_row.min(self.grid.n_rows().saturating_sub(1));
        self.cursor_col = self.cursor_col.min(self.grid.n_cols().saturating_sub(1));
        self.anchor = None;
    }

    /// The viewed measure's categories in natural (declared) order.
    fn natural_axis_order(&self) -> Vec<CategoryId> {
        self.model
            .measures
            .get(&self.viewed_measure())
            .map(|m| m.categories.clone())
            .unwrap_or_default()
    }

    /// Pivot: rotate the axis order left, so rows->pages, cols->rows, first
    /// page->cols. A single discoverable key repeatedly cycles which category
    /// sits on each axis — the Improv/Quantrix "move a category to another
    /// axis" gesture, without touching formulas. No-op for < 2 categories.
    pub fn pivot(&mut self) {
        if self.axis_order.len() < 2 {
            self.status = Some("nothing to pivot (measure has < 2 dimensions)".into());
            return;
        }
        self.axis_order.rotate_left(1);
        // Axis roles changed; reset page selection to first items.
        self.page_idx.clear();
        self.reselect();
        self.page_idx = self.grid.pages.iter().map(|p| p.item_index).collect();
        let names: Vec<String> = self
            .axis_order
            .iter()
            .map(|c| {
                self.model
                    .categories
                    .get(c)
                    .map(|x| x.name.0.clone())
                    .unwrap_or_else(|| c.0.to_string())
            })
            .collect();
        self.status = Some(format!(
            "pivot: rows={} cols={}",
            names[0],
            names.get(1).cloned().unwrap_or_default()
        ));
    }

    /// Cycle the first page (extra) dimension's selected item by `delta`
    /// (wrapping). No-op if the viewed measure has no extra dimensions.
    pub fn page(&mut self, delta: isize) {
        let Some(pd) = self.grid.pages.first() else {
            self.status = Some("no extra dimensions to page".into());
            return;
        };
        // Resync from the grid (an empty page dim is skipped, so indices
        // follow the rendered pages), then step the first one.
        let next = (pd.item_index as isize + delta).rem_euclid(pd.item_count as isize) as usize;
        self.page_idx = self.grid.pages.iter().map(|p| p.item_index).collect();
        self.page_idx[0] = next;
        self.reselect();
    }

    /// Cycle to the next measure (wraps).
    pub fn next_measure(&mut self) {
        self.selected = (self.selected + 1) % self.measures.len().max(1);
        // A new measure has its own dimensions; reset axis order, paging, filters.
        self.axis_order = self.natural_axis_order();
        self.page_idx.clear();
        self.filters.clear();
        self.reselect();
        // Size page_idx to the new grid so `[`/`]` work immediately.
        self.page_idx = self.grid.pages.iter().map(|p| p.item_index).collect();
    }

    /// An arrow step: `extend` (Shift held) keeps or sets the range anchor
    /// at the cursor before moving; a plain step collapses the range.
    pub fn step(&mut self, drow: isize, dcol: isize, extend: bool) {
        self.anchor = extend.then(|| self.anchor.unwrap_or((self.cursor_row, self.cursor_col)));
        self.move_cursor(drow, dcol);
    }

    /// True if `[row][col]` is inside the selection (anchor..=cursor).
    pub fn is_selected(&self, row: usize, col: usize) -> bool {
        let (ar, ac) = self.anchor.unwrap_or((self.cursor_row, self.cursor_col));
        let (cr, cc) = (self.cursor_row, self.cursor_col);
        (ar.min(cr)..=ar.max(cr)).contains(&row) && (ac.min(cc)..=ac.max(cc)).contains(&col)
    }

    /// Sum of the numeric cells in the selection; `None` if there are none
    /// (text/empty cells contribute nothing, as in the GUI's `Sum` readout).
    pub fn selection_sum(&self) -> Option<f64> {
        let g = &self.grid;
        let nums: Vec<f64> = (0..g.n_rows())
            .flat_map(|r| (0..g.n_cols()).map(move |c| (r, c)))
            .filter(|&(r, c)| self.is_selected(r, c))
            .filter_map(|(r, c)| g.value_at(r, c))
            .collect();
        (!nums.is_empty()).then(|| nums.iter().sum())
    }

    /// The viewed derived measure's formula as controlled English
    /// (`Price times Quantity`); `None` for input measures.
    pub fn formula_text(&self) -> Option<String> {
        match &self.model.measures.get(&self.grid.measure)?.kind {
            improv_core_model::MeasureKind::Derived(f) => {
                Some(improv_nl_formula::describe_formula(
                    &improv_nl_formula::NlContext::new(&self.model),
                    f,
                ))
            }
            _ => None,
        }
    }

    /// Cursor movement, clamped to the grid (never goes out of range).
    pub fn move_cursor(&mut self, drow: isize, dcol: isize) {
        let max_row = self.grid.n_rows().saturating_sub(1) as isize;
        let max_col = self.grid.n_cols().saturating_sub(1) as isize;
        self.cursor_row = (self.cursor_row as isize + drow).clamp(0, max_row) as usize;
        self.cursor_col = (self.cursor_col as isize + dcol).clamp(0, max_col) as usize;
    }

    /// Handle a left-click at screen `(col, row)`: if it lands on a data cell,
    /// move the cursor there. Uses the geometry recorded by the last render.
    /// Out-of-grid clicks are ignored. (Enables mouse-driven navigation; the
    /// terminal must support mouse reporting, which `improv-tui` enables.)
    pub fn click_at(&mut self, col: u16, row: u16) {
        let Some(g) = self.grid_geom else { return };
        if row < g.y0 || col < g.x0 {
            return;
        }
        // Which window row: each grid row is one terminal line from y0.
        let r = (row - g.y0) as usize;
        // Which window column: columns are col_w + spacing wide, from x0.
        let stride = (g.col_w + g.spacing) as usize;
        let c = ((col - g.x0) as usize) / stride.max(1);
        let (r, c) = (g.row0 + r, g.col0 + c);
        let in_window = r < g.row0 + g.rows && c < g.col0 + g.cols;
        if in_window && r < self.grid.n_rows() && c < self.grid.n_cols() {
            self.anchor = None;
            self.cursor_row = r;
            self.cursor_col = c;
        }
    }

    /// True if `[row][col]` names a full coordinate: one item for every
    /// category of the measure. False when an axis or page is empty (a
    /// category with no items, or every item filtered out); writing there
    /// would store a cell with a missing dimension.
    fn addressable(&self, row: usize, col: usize) -> bool {
        let dims = self
            .model
            .measures
            .get(&self.grid.measure)
            .map_or(0, |m| m.categories.len());
        self.grid.coord_key(row, col).len() == dims
    }

    /// True if the currently viewed measure is an input measure (editable).
    pub fn viewed_is_input(&self) -> bool {
        self.model
            .measures
            .get(&self.grid.measure)
            .map(|m| m.is_input())
            .unwrap_or(false)
    }

    /// True while the edit line holds a formula: a definition, or the viewed
    /// measure is derived.
    pub fn editing_formula(&self) -> bool {
        self.edit.is_some() && (self.defining || !self.viewed_is_input())
    }

    /// Open the `define>` line (`=`): `Name = formula` adds a derived
    /// measure (or gives an existing one a new formula), `Name =
    /// LAMBDA(p, …, body)` a named LAMBDA.
    pub fn begin_define(&mut self) {
        self.edit = Some(LineEdit::default());
        self.defining = true;
        self.status = None;
    }

    /// Enter edit mode: on the cell under the cursor for an input measure, on
    /// the formula (seeded with its controlled-English text) for a derived one.
    pub fn begin_edit(&mut self) {
        if !self.model.measures.contains_key(&self.grid.measure) {
            self.status = Some(EMPTY.into());
            return;
        }
        if !self.viewed_is_input() {
            self.edit = Some(self.formula_text().unwrap_or_default().into());
            self.status = None;
            return;
        }
        if !self.addressable(self.cursor_row, self.cursor_col) {
            self.status = Some(NO_CELL.into());
            return;
        }
        // Seed the buffer with the current value so editing is a tweak, not a
        // retype. Blank cell -> empty buffer.
        let seed = match self.grid.value_at(self.cursor_row, self.cursor_col) {
            Some(n) if n.fract() == 0.0 => format!("{n:.0}"),
            Some(n) => format!("{n}"),
            // Text/boolean cells seed with their display text.
            None => self.grid.display_at(self.cursor_row, self.cursor_col),
        };
        self.edit = Some(seed.into());
        self.status = None;
    }

    /// A key in the edit line. In a formula, Tab completes the name before
    /// the caret ([`complete::tab`]); any other key edits, and a change
    /// clears the message about the old text.
    pub fn edit_key(&mut self, code: KeyCode) {
        let formula = self.editing_formula();
        let Some(e) = &mut self.edit else { return };
        if code == KeyCode::Tab {
            if formula {
                let before = e.text.clone();
                self.status = complete::tab(&self.model, e, &mut self.tab_cycle);
                if e.text != before {
                    self.diagnostic = None;
                }
            }
            return;
        }
        self.tab_cycle = None;
        if e.key(code) {
            self.status = None;
            self.diagnostic = None;
        }
    }

    /// Cancel edit mode without committing.
    pub fn cancel_edit(&mut self) {
        self.edit = None;
        self.defining = false;
        self.tab_cycle = None;
        self.diagnostic = None;
        self.status = None;
    }

    /// Commit the in-progress edit buffer. Parses it per the measure's value
    /// type (or as a formula for a derived measure), applies it to the model +
    /// engine, refreshes the grid, and exits edit mode. On a parse error the
    /// buffer stays open with a status message.
    pub fn commit_edit(&mut self) {
        let Some(buf) = self.edit.as_ref().map(|e| e.text.clone()) else {
            return;
        };
        if self.defining {
            self.commit_definition(&buf);
            return;
        }
        if !self.viewed_is_input() {
            self.diagnostic = None;
            match parse_formula_text(&self.model, &buf) {
                Err(e) => {
                    self.status = None;
                    self.diagnostic = Some(crate::diagnose::diagnose(&self.model, &buf, &e));
                }
                Ok(f) => match self.set_formula(f) {
                    Ok(()) => {
                        self.edit = None;
                        self.tab_cycle = None;
                        self.status = None;
                    }
                    Err(e) => self.status = Some(e),
                },
            }
            return;
        }
        let vt = self
            .model
            .measures
            .get(&self.grid.measure)
            .map_or(ValueType::Number, |m| m.value_type);
        let value = match parse_typed(buf.trim(), vt) {
            Ok(v) => v,
            Err(e) => {
                self.status = Some(e);
                return;
            }
        };
        match self.apply_value(self.cursor_row, self.cursor_col, value) {
            Ok(()) => {
                self.edit = None;
                self.status = None;
            }
            Err(e) => self.status = Some(e),
        }
    }

    /// Pure edit-apply: set input `[row][col]` to `value`, updating both the
    /// in-memory model and the live engine, then rebuild the grid from the new
    /// snapshot. Returns `Err` (no state change committed) if the viewed
    /// measure is derived. Terminal-free -> unit-testable.
    #[cfg(test)]
    pub fn apply_edit(&mut self, row: usize, col: usize, value: f64) -> Result<(), String> {
        self.apply_value(row, col, Value::Number(value))
    }

    /// Set input `[row][col]` to a typed `value`, pushed through the live
    /// engine as one incremental edit. A value of the measure's type enters
    /// the engine as that cell; anything else clears it, exactly what a
    /// rebuild would seed ([`improv_engine::session::input_cell`]).
    pub fn apply_value(&mut self, row: usize, col: usize, value: Value) -> Result<(), String> {
        if !self.viewed_is_input() {
            return Err("derived cells are computed, not editable".into());
        }
        let measure = self.grid.measure;
        if !self.addressable(row, col) {
            return Err(NO_CELL.into());
        }
        let key = self.grid.coord_key(row, col);
        let coord = decode_coord(&key);

        let cell = self
            .model
            .measures
            .get(&measure)
            .and_then(|m| improv_engine::session::input_cell(m.value_type, &value));
        self.record_history(self.model.clone());
        self.model.set_input(measure, coord, value);

        // Push the delta through the engine (if any derived measures exist) and
        // keep the recomputed snapshot for rendering.
        if let Some(engine) = &mut self.engine {
            self.snapshot = engine
                .apply(vec![improv_engine::session::Edit {
                    measure,
                    coord: key,
                    value: cell,
                }])
                .map_err(|e| e.to_string())?;
        }
        self.reselect();
        Ok(())
    }

    /// Replace the viewed derived measure's formula. Validated on a candidate
    /// model (a real engine build, which catches cycles and type errors) and
    /// only then published; on failure nothing changes and the error is
    /// returned. The old model is an undo point.
    pub fn set_formula(&mut self, formula: Formula) -> Result<(), String> {
        let mut candidate = self.model.clone();
        candidate
            .measures
            .get_mut(&self.grid.measure)
            .ok_or("the measure no longer exists")?
            .kind = MeasureKind::Derived(formula);
        self.commit_model(candidate)?;
        self.reselect();
        Ok(())
    }

    /// Commit the `define>` line, as `improv define` takes it. A parse error
    /// is located like a formula edit's, any other failure goes to the
    /// status line; either way the line stays open and nothing changes.
    fn commit_definition(&mut self, text: &str) {
        self.diagnostic = None;
        let target = definition_target(text);
        let existing = self
            .model
            .measure_by_name(&target)
            .map(|m| (m.id, m.is_input()));
        let id = existing.map_or_else(
            || MeasureId(next_id(self.model.measures.keys().map(|m| m.0))),
            |(id, _)| id,
        );
        let parsed = parser::parse_definition(&self.model.with_provisional(id, &target), text);
        let done = match parsed {
            Err(e) => {
                self.status = None;
                self.diagnostic = Some(crate::diagnose::diagnose(&self.model, text, &e));
                return;
            }
            Ok(def) => self.define(def, id, existing.map(|(_, input)| input)),
        };
        match done {
            Ok(msg) => {
                self.cancel_edit();
                self.status = Some(msg);
            }
            Err(e) => self.status = Some(e),
        }
    }

    /// Add a parsed definition: a derived measure `id` (an existing one
    /// keeps its id; `existing` says whether it is an input measure), or a
    /// named LAMBDA. The model with it must build, and it is then shown.
    fn define(
        &mut self,
        def: Definition,
        id: MeasureId,
        existing: Option<bool>,
    ) -> Result<String, String> {
        let mut model = self.model.clone();
        let (show, msg) = match def {
            Definition::Formula(ft) => {
                if existing == Some(true) {
                    return Err(format!("'{}' is an input measure", ft.target.0));
                }
                let (value_type, categories) = improv_engine::compiler::derived_shape(
                    &self.model,
                    id,
                    &ft.target.0,
                    &ft.formula,
                )
                .map_err(|e| e.to_string())?;
                let msg = format!("defined {}", ft.target.0);
                model.add_measure(Measure {
                    id,
                    name: ft.target,
                    value_type,
                    categories,
                    kind: MeasureKind::Derived(ft.formula),
                    description: None,
                });
                (Some(id), msg)
            }
            Definition::Lambda(lt) => {
                if existing.is_some() {
                    return Err(format!(
                        "'{}' is a measure; name the LAMBDA differently",
                        lt.name.0
                    ));
                }
                // Calls store the id, so redefining a name keeps it.
                let lid = model
                    .lambda_by_name(&lt.name.0)
                    .map_or_else(|| next_id(model.lambdas.keys().copied()), |(old, _)| old);
                let msg = format!(
                    "defined LAMBDA {}({})",
                    lt.name.0,
                    lt.lambda.params.join(", ")
                );
                model.lambdas.insert(lid, lt.lambda);
                (None, msg)
            }
            Definition::Sql { .. } | Definition::Call { .. } => {
                return Err(
                    "SQL(…) and CALL(…) sources need the CLI: improv import-sql, improv define"
                        .into(),
                )
            }
        };
        self.commit_model(model)?;
        self.relayout(show);
        Ok(msg)
    }

    /// Make `candidate` the model and the replaced one an undo point. It is
    /// built into a fresh engine first, so on failure nothing changes. The
    /// caller refreshes the grid.
    fn commit_model(&mut self, candidate: Model) -> Result<(), String> {
        (self.engine, self.snapshot) = build_engine(&candidate)?;
        let prior = std::mem::replace(&mut self.model, candidate);
        self.record_history(prior);
        Ok(())
    }

    // -- CSV import/export command prompt (pure; terminal-free -> unit-testable) --

    /// Enter import-csv command-prompt mode (bound to `I`). Mirrors
    /// `begin_edit`: starts an empty typed-line buffer.
    pub fn begin_import_csv(&mut self) {
        self.begin_command(CommandKind::ImportCsv);
    }

    /// Enter export-csv command-prompt mode (bound to `E`).
    pub fn begin_export_csv(&mut self) {
        self.begin_command(CommandKind::ExportCsv);
    }

    /// Open the command prompt for `kind`, empty.
    pub fn begin_command(&mut self, kind: CommandKind) {
        self.command = Some((kind, String::new()));
        self.status = None;
    }

    /// Cancel command-prompt mode without side effects.
    pub fn cancel_command(&mut self) {
        self.command = None;
        self.status = None;
    }

    /// Commit the in-progress command line: parse it per the active
    /// `CommandKind`, run the storage_csv import/export, and set a status
    /// message (success with a count, or the parse/IO error). Always exits
    /// command mode — a malformed line reports the error rather than looping
    /// (matching the TUI's existing "generated view name" simplicity).
    pub fn commit_command(&mut self) {
        let Some((kind, line)) = self.command.take() else {
            return;
        };
        self.status = Some(match kind {
            CommandKind::ImportCsv => self.run_import_csv(&line),
            CommandKind::ExportCsv => self.run_export_csv(&line),
            CommandKind::Category => self.run_add_category(&line),
            CommandKind::Input => self.run_add_input(&line),
        });
    }

    /// `c`: add category `Name` if it is new, and the listed items it does
    /// not have yet, each with the next free id.
    fn run_add_category(&mut self, line: &str) -> String {
        let (name, items) = name_and_list(line);
        if name.is_empty() {
            return "category: type Name, or Name: item, item, …".into();
        }
        let mut model = self.model.clone();
        let cat = match model.category_by_name(name) {
            Some(c) => c.id,
            None => {
                let id = CategoryId(next_id(model.categories.keys().map(|c| c.0)));
                model.add_category(id, name);
                id
            }
        };
        for it in items {
            let have = model.categories[&cat].items.iter();
            if !have.clone().any(|i| model.items[i].name.0 == it) {
                model.add_item(ItemId(next_id(model.items.keys().map(|i| i.0))), cat, it);
            }
        }
        if model == self.model {
            return format!("category: '{name}' has those items");
        }
        let n = model.categories[&cat].items.len();
        self.adopt(model, None, format!("category {name}: {n} item(s)"))
    }

    /// `n`: add a number input measure `Name` over the listed categories,
    /// with the next free id, and show it.
    fn run_add_input(&mut self, line: &str) -> String {
        let (name, cats) = name_and_list(line);
        if name.is_empty() {
            return "input: type Name, or Name: Category, Category, …".into();
        }
        if self.model.measure_by_name(name).is_some() || self.model.lambda_by_name(name).is_some() {
            return format!("input: '{name}' is taken");
        }
        let categories: Result<Vec<CategoryId>, String> = cats
            .iter()
            .map(|c| {
                self.model
                    .category_by_name(c)
                    .map(|c| c.id)
                    .ok_or(format!("input: no category '{c}' (c adds one)"))
            })
            .collect();
        let categories = match categories {
            Ok(c) => c,
            Err(e) => return e,
        };
        let id = MeasureId(next_id(self.model.measures.keys().map(|m| m.0)));
        let mut model = self.model.clone();
        model.add_measure(Measure {
            id,
            name: Name(name.into()),
            value_type: ValueType::Number,
            categories,
            kind: MeasureKind::Input,
            description: None,
        });
        self.adopt(model, Some(id), format!("added input measure {name}"))
    }

    /// Make `model` current after a structural change, the old one an undo
    /// point, and show `show` (else the viewed measure). Returns `done`, and
    /// that the formulas no longer build if they do not.
    fn adopt(&mut self, model: Model, show: Option<MeasureId>, done: String) -> String {
        let prior = std::mem::replace(&mut self.model, model);
        self.record_history(prior);
        match self.rebuild(show) {
            Ok(()) => done,
            Err(e) => format!("{done}; formulas not recomputed: {e}"),
        }
    }

    /// Parse and run an import-csv command line against `self.model`, and
    /// rebuild the engine/grid so the new measure shows up immediately
    /// (mirrors `next_measure`'s structural-rebuild path, since importing
    /// adds a new measure — the engine's fixed structure must be rebuilt).
    fn run_import_csv(&mut self, line: &str) -> String {
        let spec = match parse_import_line(line) {
            Ok(s) => s,
            Err(e) => return format!("import-csv: {e}"),
        };
        let measure_name = spec.measure_name.clone();
        // Into a copy: a failed import leaves the model as it was.
        let mut model = self.model.clone();
        match import_csv(&mut model, &spec) {
            Ok(n) => {
                let msg = format!("imported {n} cell(s) into measure '{measure_name}'");
                self.adopt(model, None, msg)
            }
            Err(e) => format!("import-csv: {e}"),
        }
    }

    /// Parse and run an export-csv command line against `self.model`.
    fn run_export_csv(&mut self, line: &str) -> String {
        let (target, measure_id, delimiter) = match parse_export_line(line) {
            Ok(t) => t,
            Err(e) => return format!("export-csv: {e}"),
        };
        match export_measure_csv(&self.model, measure_id, Path::new(&target), delimiter) {
            Ok(n) => format!(
                "exported {n} row(s) from measure {} to {target}",
                measure_id.0
            ),
            Err(e) => format!("export-csv: {e}"),
        }
    }

    /// Rebuild `measures`/`engine`/`grid` from the current `self.model` after a
    /// structural change (a new measure/category/item), showing `show`, else
    /// the viewed measure when it still exists. The live `Engine`'s tracked
    /// structure is fixed at construction, so a new measure needs a new
    /// engine. If the formulas no longer build (an import changed a measure
    /// they use), the rest is still rebuilt, with no engine, and the error is
    /// returned for the status.
    fn rebuild(&mut self, show: Option<MeasureId>) -> Result<(), String> {
        let built = build_engine(&self.model);
        let err = built.as_ref().err().cloned();
        (self.engine, self.snapshot) = built.unwrap_or_default();
        self.relayout(show);
        err.map_or(Ok(()), Err)
    }

    /// Re-list the measures and show `show`, else the viewed measure if it
    /// still exists, else the first one, in its natural layout.
    fn relayout(&mut self, show: Option<MeasureId>) {
        let want = show.unwrap_or(self.viewed_measure());
        self.measures = measure_order(&self.model);
        self.selected = self.measures.iter().position(|m| *m == want).unwrap_or(0);
        self.axis_order = self.natural_axis_order();
        self.page_idx.clear();
        self.filters.clear();
        self.reselect();
        self.page_idx = self.grid.pages.iter().map(|p| p.item_index).collect();
    }

    // -- undo/redo ---------------------------------------------------------

    /// Record `prior`, the model before a change, as an undo point. A new
    /// change makes the undone future unreachable.
    fn record_history(&mut self, prior: Model) {
        push_bounded(&mut self.undo_stack, prior);
        self.redo_stack.clear();
    }

    /// Undo the last change to the model: the model before it comes back,
    /// rebuilt into a fresh engine.
    pub fn undo(&mut self) {
        self.travel(true);
    }

    /// Redo the last undone change.
    pub fn redo(&mut self) {
        self.travel(false);
    }

    /// Swap in the model from the undo (`back`) or the redo stack, moving
    /// the current one onto the other. The layout stays while the viewed
    /// measure still has the same categories.
    fn travel(&mut self, back: bool) {
        let (from, to, verb, done) = if back {
            (&mut self.undo_stack, &mut self.redo_stack, "undo", "undone")
        } else {
            (&mut self.redo_stack, &mut self.undo_stack, "redo", "redone")
        };
        let Some(mut model) = from.pop() else {
            self.status = Some(format!("nothing to {verb}"));
            return;
        };
        // Views are layout, saved as they are made: undo leaves them.
        model.views = self.model.views.clone();
        push_bounded(to, std::mem::replace(&mut self.model, model));
        let viewed = self.viewed_measure();
        let layout = (
            self.axis_order.clone(),
            self.filters.clone(),
            self.page_idx.clone(),
        );
        let built = self.rebuild(None);
        let natural = self.natural_axis_order();
        if self.viewed_measure() == viewed
            && natural.len() == layout.0.len()
            && natural.iter().all(|c| layout.0.contains(c))
        {
            (self.axis_order, self.filters, self.page_idx) = layout;
            self.reselect();
        }
        self.status = Some(match built {
            Ok(()) => done.to_string(),
            Err(e) => format!("{done}; formulas not recomputed: {e}"),
        });
    }

    // -- views & filters (pure; terminal-free -> unit-testable) ------------

    /// The viewed measure's id.
    /// A model with no measures views `MeasureId(0)`, which is then not
    /// there either.
    fn viewed_measure(&self) -> MeasureId {
        self.measures
            .get(self.selected)
            .copied()
            .unwrap_or(MeasureId(0))
    }

    /// Build a `View` capturing the current layout: viewed measure, axis order,
    /// pinned page items, and active filters. Presentation only — no data.
    pub fn build_view(&self, id: ViewId, name: &str) -> View {
        let page_items = self.grid.pages.iter().map(|p| (p.cat, p.item)).collect();
        View {
            id,
            name: Name(name.to_string()),
            measure: self.viewed_measure(),
            axis_order: self.axis_order.clone(),
            n_rows: 1,
            n_cols: 1,
            page_items,
            filters: self.filters.clone(),
            // Canvas geometry/extra matrices are GUI-canvas concepts (plan
            // Step 3); the TUI always saves a single-matrix view.
            rect: Default::default(),
            placements: vec![],
        }
    }

    /// The smallest unused view id (>= 1).
    fn next_view_id(&self) -> ViewId {
        ViewId(next_id(self.model.views.keys().map(|v| v.0)))
    }

    /// Save the current layout as a named view: mint an id, add it to the
    /// model, and (when `path` is set) persist immediately. Returns the id.
    pub fn save_view(&mut self, name: &str, path: Option<&str>) -> ViewId {
        let id = self.next_view_id();
        let view = self.build_view(id, name);
        self.model.add_view(view);
        self.status = Some(match path.map(|p| self.save(p)) {
            Some(Err(e)) => format!("save failed: {e}"),
            _ => format!("saved view '{name}'"),
        });
        id
    }

    /// Save the model to the store at `path`, refusing to overwrite another
    /// writer's changes. Only what changed since the last load or save is
    /// written; the next save diffs against this one.
    pub fn save(&mut self, path: &str) -> Result<(), String> {
        let changes = match &self.saved {
            Some(saved) => improv_store::ChangeSet::between(saved, &self.model),
            None => improv_store::ChangeSet {
                structure: true,
                ..Default::default()
            },
        };
        let mut s = improv_store::Store::open(path).map_err(|e| e.to_string())?;
        s.adopt_save_token(std::mem::take(&mut self.save_token));
        let r = if self.saved.is_some() {
            s.save_changes(&self.model, &changes)
        } else {
            s.save_model(&self.model)
        };
        // On failure this is the adopted token back, unchanged.
        self.save_token = s.take_save_token();
        r.map_err(|e| e.to_string())?;
        self.saved = Some(self.model.clone());
        Ok(())
    }

    /// `w`: save now, as quitting does, and say how it went.
    pub fn write(&mut self, path: &str) {
        self.status = Some(match self.save(path) {
            Ok(()) => "saved".into(),
            Err(e) => format!("save failed: {e}"),
        });
    }

    /// Apply a saved `View` to the live layout: select its measure, restore the
    /// axis order, page items, and filters, then re-render. Presentation only
    /// — measures and data are untouched. No-op if the measure is gone.
    pub fn apply_view(&mut self, view: &View) {
        let Some(idx) = self.measures.iter().position(|m| *m == view.measure) else {
            self.status = Some("view's measure no longer exists".into());
            return;
        };
        self.selected = idx;
        self.axis_order = if view.axis_order.is_empty() {
            self.natural_axis_order()
        } else {
            view.axis_order.clone()
        };
        self.filters = view.filters.clone();
        self.reselect();
        // Restore pinned page items positionally by page dimension.
        self.page_idx = self
            .grid
            .pages
            .iter()
            .map(|p| {
                view.page_items
                    .iter()
                    .find(|(c, _)| *c == p.cat)
                    .and_then(|(_, it)| {
                        items_of(&self.model, p.cat, &self.filters)
                            .iter()
                            .position(|(id, _)| id == it)
                    })
                    .unwrap_or(p.item_index)
            })
            .collect();
        self.reselect();
    }

    /// Cycle to the next saved view (by id order) and apply it. No-op if there
    /// are no saved views.
    pub fn cycle_view(&mut self) {
        let mut ids: Vec<ViewId> = self.model.views.keys().copied().collect();
        if ids.is_empty() {
            self.status = Some("no saved views (press S to save one)".into());
            return;
        }
        ids.sort_by_key(|v| v.0);
        // Advance past the last-applied view if it is still present.
        let start = self
            .applied_view
            .and_then(|cur| ids.iter().position(|v| *v == cur))
            .map(|p| (p + 1) % ids.len())
            .unwrap_or(0);
        let id = ids[start];
        self.applied_view = Some(id);
        let view = self.model.views[&id].clone();
        self.apply_view(&view);
        self.status = Some(format!("view: {}", view.name.0));
    }

    /// Toggle whether `item` of `category` is shown. Building the filter from
    /// the full item set on first toggle, then removing/re-adding the item.
    /// Removing the last shown item leaves an explicit empty filter (nothing
    /// shown for that category). Presentation only.
    pub fn toggle_filter_item(&mut self, category: CategoryId, item: ItemId) {
        let all: Vec<ItemId> = self
            .model
            .categories
            .get(&category)
            .map(|c| c.items.clone())
            .unwrap_or_default();
        let pos = self.filters.iter().position(|f| f.category == category);
        match pos {
            None => {
                // First toggle: start from all items, then hide this one.
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
                // If the filter now keeps everything, drop it (unfiltered).
                if f.items.len() == all.len() && all.iter().all(|i| f.items.contains(i)) {
                    self.filters.remove(i);
                }
            }
        }
        self.reselect();
    }

    /// Clear all active filters, showing every item again.
    pub fn clear_filters(&mut self) {
        self.filters.clear();
        self.reselect();
    }
    /// The `(measure, Coordinate)` of the cell under the cursor.
    #[cfg(test)]
    pub fn cursor_coord(&self) -> (MeasureId, Coordinate) {
        let key = self.grid.coord_key(self.cursor_row, self.cursor_col);
        (self.grid.measure, decode_coord(&key))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use improv_core_model::{
        BinaryOp, DimensionSpec, Expr, Formula, Measure, MeasureKind, Name, ValueType,
    };

    // Canonical Time x Product revenue model (mirrors engine's fixture).
    pub(crate) fn revenue_model() -> Model {
        let mut m = Model::new();
        let (time, product) = (CategoryId(1), CategoryId(2));
        m.add_category(time, "Time");
        m.add_category(product, "Product");
        m.add_item(ItemId(10), time, "2025");
        m.add_item(ItemId(11), time, "2026");
        m.add_item(ItemId(20), product, "WidgetA");
        m.add_item(ItemId(21), product, "WidgetB");

        m.add_measure(Measure {
            id: MeasureId(100),
            name: Name("Price".into()),
            value_type: ValueType::Number,
            categories: vec![product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(101),
            name: Name("Quantity".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Input,
            description: None,
        });
        m.add_measure(Measure {
            id: MeasureId(102),
            name: Name("Revenue".into()),
            value_type: ValueType::Number,
            categories: vec![time, product],
            kind: MeasureKind::Derived(Formula::new(Expr::BinaryOp(
                BinaryOp::Mul,
                Box::new(Expr::Ref(MeasureId(100), DimensionSpec::default())),
                Box::new(Expr::Ref(MeasureId(101), DimensionSpec::default())),
            ))),
            description: None,
        });

        let coord = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
        m.set_input(
            MeasureId(100),
            coord(&[(product, ItemId(20))]),
            Value::Number(10.0),
        );
        m.set_input(
            MeasureId(100),
            coord(&[(product, ItemId(21))]),
            Value::Number(20.0),
        );
        for (t, p, q) in [
            (ItemId(10), ItemId(20), 100.0),
            (ItemId(10), ItemId(21), 50.0),
            (ItemId(11), ItemId(20), 120.0),
            (ItemId(11), ItemId(21), 80.0),
        ] {
            m.set_input(
                MeasureId(101),
                coord(&[(time, t), (product, p)]),
                Value::Number(q),
            );
        }
        m
    }

    #[test]
    fn derived_grid_has_right_dims_and_values() {
        let model = revenue_model();
        let app = App::new(model).unwrap();
        let grid = build_grid(&app.model, MeasureId(102), &app.snapshot); // Revenue[Time, Product]
        assert_eq!(grid.n_rows(), 2, "two Time items");
        assert_eq!(grid.n_cols(), 2, "two Product items");
        assert_eq!(grid.row_cat.as_ref().unwrap().1, "Time");
        assert_eq!(grid.col_cat.as_ref().unwrap().1, "Product");

        // Rows follow declared item order: 2025, 2026. Cols: WidgetA, WidgetB.
        // Revenue[2025, WidgetA] = 10 * 100 = 1000.
        assert_eq!(grid.value_at(0, 0), Some(1000.0));
        // Revenue[2026, WidgetB] = 20 * 80 = 1600.
        assert_eq!(grid.value_at(1, 1), Some(1600.0));
    }

    #[test]
    fn text_input_cell_renders_typed_not_nan() {
        // A Text input measure must render its text (not NaN / blank).
        use improv_core_model::{Measure, MeasureKind, Name, ValueType};
        let mut model = Model::new();
        let p = CategoryId(2);
        model.add_category(p, "Product");
        model.add_item(ItemId(20), p, "WidgetA");
        model.add_measure(Measure {
            id: MeasureId(300),
            name: Name("Label".into()),
            value_type: ValueType::Text,
            categories: vec![p],
            kind: MeasureKind::Input,
            description: None,
        });
        model.set_input(
            MeasureId(300),
            Coordinate::from_pairs([(p, ItemId(20))]),
            Value::Text("hello".into()),
        );
        let app = App::new(model).unwrap();
        let grid = build_grid(&app.model, MeasureId(300), &app.snapshot);
        assert_eq!(grid.display_at(0, 0), "hello");
        assert_eq!(grid.value_at(0, 0), None, "text cell has no numeric value");
    }

    #[test]
    fn input_grid_single_axis() {
        let model = revenue_model();
        let grid = build_grid(&model, MeasureId(100), &Snapshot::default()); // Price[Product]
        assert_eq!(
            grid.n_rows(),
            2,
            "two Product items on the single (row) axis"
        );
        assert_eq!(
            grid.n_cols(),
            1,
            "no column axis => single synthetic column"
        );
        assert!(grid.col_cat.is_none());
        assert_eq!(grid.value_at(0, 0), Some(10.0)); // WidgetA
        assert_eq!(grid.value_at(1, 0), Some(20.0)); // WidgetB
    }

    #[test]
    fn measure_cycling_wraps_and_derived_first() {
        let model = revenue_model();
        let mut app = App::new(model).unwrap();
        // Derived (Revenue, 102) sorts first.
        assert_eq!(app.grid.measure, MeasureId(102));
        app.next_measure();
        assert_eq!(app.grid.measure, MeasureId(100)); // then Price (id 100)
        app.next_measure();
        assert_eq!(app.grid.measure, MeasureId(101)); // then Quantity (id 101)
        app.next_measure();
        assert_eq!(app.grid.measure, MeasureId(102)); // wraps back
    }

    #[test]
    fn cursor_stays_in_bounds() {
        let model = revenue_model();
        let mut app = App::new(model).unwrap();
        // Revenue grid is 2x2. Push cursor past every edge.
        app.move_cursor(-5, -5);
        assert_eq!((app.cursor_row, app.cursor_col), (0, 0));
        app.move_cursor(100, 100);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));

        // Switching to Price (2 rows, 1 col) must re-clamp the column.
        app.selected = app
            .measures
            .iter()
            .position(|m| *m == MeasureId(100))
            .unwrap();
        app.reselect();
        assert_eq!(app.cursor_col, 0, "column re-clamped to the narrower grid");
        assert!(app.cursor_row <= 1);
    }

    // Position the app on a specific measure by id.
    pub(crate) fn view(app: &mut App, measure: MeasureId) {
        app.selected = app.measures.iter().position(|m| *m == measure).unwrap();
        app.reselect();
    }

    #[test]
    fn editing_input_recomputes_derived_via_engine() {
        let mut app = App::new(revenue_model()).unwrap();
        // View Quantity[Time, Product]; rows=Time(2025,2026), cols=Product(WidgetA,WidgetB).
        view(&mut app, MeasureId(101));
        // Cell [0,0] = Quantity[2025, WidgetA]; confirm the mapping first.
        let (m, coord) = {
            app.cursor_row = 0;
            app.cursor_col = 0;
            app.cursor_coord()
        };
        assert_eq!(m, MeasureId(101));
        assert_eq!(coord.get(CategoryId(1)), Some(ItemId(10))); // Time=2025
        assert_eq!(coord.get(CategoryId(2)), Some(ItemId(20))); // Product=WidgetA

        // Set Quantity[2025, WidgetA] = 200 (was 100).
        app.apply_edit(0, 0, 200.0).unwrap();
        assert_eq!(
            app.model.input(MeasureId(101), &coord),
            Some(&Value::Number(200.0)),
            "model input updated"
        );

        // Revenue[2025, WidgetA] = Price(10) * 200 = 2000 in the live snapshot.
        let rev_key = {
            let mut k = vec![(1u32, 10u32), (2, 20)];
            k.sort();
            CoordKey::from_vec(k)
        };
        assert_eq!(
            app.snapshot
                .get(&MeasureId(102))
                .unwrap()
                .get(&rev_key)
                .and_then(|v| v.as_num()),
            Some(2000.0),
            "derived Revenue recomputed incrementally"
        );
    }

    #[test]
    fn editing_derived_cell_is_rejected() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(102)); // Revenue is derived
        let before = app.model.clone();
        let err = app.apply_edit(0, 0, 42.0);
        assert!(err.is_err(), "derived edit must be rejected");
        assert_eq!(app.model, before, "model unchanged on rejected edit");
        // No edit open: not editing a formula, and edit keys do nothing.
        assert!(!app.editing_formula());
        app.edit_key(KeyCode::Char('x'));
        assert!(app.edit.is_none());

        // begin_edit opens the formula instead, seeded with its text.
        app.begin_edit();
        assert!(app.editing_formula());
        assert_eq!(app.edit.as_deref(), Some("Price times Quantity"));
    }

    #[test]
    fn editing_a_formula_recomputes_or_reports_the_error() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(102));
        app.begin_edit();
        // The seeded controlled English commits back unchanged.
        app.commit_edit();
        assert!(app.edit.is_none());
        assert!(app.status.is_none());
        assert_eq!(app.grid.display_at(0, 0), "1000");
        app.begin_edit();
        app.edit = Some("Price * Quantity * 2".into());
        app.commit_edit();
        assert!(app.edit.is_none());
        assert_eq!(app.grid.display_at(0, 0), "2000");
        assert_eq!(
            app.formula_text().as_deref(),
            Some("Price times Quantity times 2")
        );
        // A parse error keeps the edit open, located, and changes nothing.
        let before = app.model.clone();
        app.begin_edit();
        app.edit = Some("Price * Quantty".into());
        app.commit_edit();
        assert!(app.editing_formula());
        let d = app.diagnostic.clone().unwrap();
        assert_eq!(
            (d.span, d.message.as_str()),
            (Some(8..15), "unknown measure: Quantty")
        );
        assert_eq!(app.model, before);
        // Tab that changes nothing keeps it; an edit clears it.
        app.edit_key(KeyCode::Tab);
        assert!(app.diagnostic.is_some());
        app.edit_key(KeyCode::Left);
        assert!(app.diagnostic.is_some(), "moving the caret keeps it");
        app.edit_key(KeyCode::Char('i'));
        assert!(app.diagnostic.is_none());
        app.edit = Some("Price * Quantty".into());
        app.commit_edit();
        app.edit.as_mut().unwrap().caret = 12;
        app.edit_key(KeyCode::Tab);
        assert_eq!(app.edit.as_deref(), Some("Price * Quantitytty"));
        assert!(app.diagnostic.is_none(), "a completion edits the text");
        app.cancel_edit();
        // A formula that parses but cannot build (a cycle) reports, unlocated.
        app.begin_edit();
        app.edit = Some("Revenue + 1".into());
        app.commit_edit();
        assert!(app.editing_formula(), "{:?}", app.status);
        assert!(app.status.is_some());
        assert!(app.diagnostic.is_none());
        assert_eq!(app.model, before);
        // A value edit is not a formula edit.
        view(&mut app, MeasureId(101));
        app.begin_edit();
        assert!(app.edit.is_some());
        assert!(!app.editing_formula());
    }

    #[test]
    fn cursor_maps_to_expected_coordinate() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // Quantity[Time, Product], 2x2

        // [1,1] = Quantity[2026, WidgetB] = Time(11), Product(21).
        app.cursor_row = 1;
        app.cursor_col = 1;
        let key = app.grid.coord_key(1, 1);
        let mut expect = vec![(1u32, 11u32), (2, 21)];
        expect.sort();
        assert_eq!(key.as_slice(), expect, "cursor cell -> CoordKey");

        // Round-trip: key -> Coordinate -> back matches cursor_coord.
        let (_m, coord) = app.cursor_coord();
        assert_eq!(improv_engine::encode_coord(&coord), key);
        assert_eq!(coord.get(CategoryId(1)), Some(ItemId(11)));
        assert_eq!(coord.get(CategoryId(2)), Some(ItemId(21)));
    }

    #[test]
    fn paging_changes_the_pinned_extra_dimension() {
        // A 3-D input measure Sales[Time, Product, Region]; Time/Product are the
        // grid axes, Region is a page dimension navigated with [ / ].
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
        let c = |pairs: &[(CategoryId, ItemId)]| Coordinate::from_pairs(pairs.iter().copied());
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

        let mut app = App::new(m).unwrap();
        // One page dimension (Region), starting on North (index 0).
        assert_eq!(app.grid.pages.len(), 1);
        assert_eq!(app.grid.pages[0].item_name, "North");
        assert_eq!(app.grid.pages[0].item_count, 2);
        assert_eq!(app.grid.value_at(0, 0), Some(100.0)); // North cell

        // Page to the next Region: South, and the visible cell changes.
        app.page(1);
        assert_eq!(app.grid.pages[0].item_name, "South");
        assert_eq!(app.grid.value_at(0, 0), Some(250.0));

        // Wraps back to North.
        app.page(1);
        assert_eq!(app.grid.pages[0].item_name, "North");
        assert_eq!(app.grid.value_at(0, 0), Some(100.0));

        // Changing measure (here back to the only one) starts on the first
        // page again; a pivot moves Time to the page, also from its first item.
        app.page(1);
        app.next_measure();
        assert_eq!(app.grid.pages[0].item_name, "North");
        assert_eq!(app.page_idx, vec![0]);
        app.pivot();
        assert_eq!(
            app.status.as_deref(),
            Some("pivot: rows=Product cols=Region")
        );
        assert_eq!(app.grid.pages[0].cat_name, "Time");
        assert_eq!(app.page_idx, vec![0]);
    }

    /// With no formulas there is no engine; value edits still land. A
    /// formula that cannot build fails `App::new` with the engine's error.
    #[test]
    fn engine_only_for_formulas_that_build() {
        let mut m = revenue_model();
        m.measures.remove(&MeasureId(102));
        let mut app = App::new(m).unwrap();
        assert!(app.engine.is_none());
        view(&mut app, MeasureId(101));
        app.apply_edit(0, 0, 7.0).unwrap();
        assert_eq!(app.grid.value_at(0, 0), Some(7.0));

        let mut m = revenue_model();
        m.measures.get_mut(&MeasureId(102)).unwrap().kind = MeasureKind::Derived(Formula::new(
            Expr::Ref(MeasureId(102), DimensionSpec::default()),
        ));
        let want = Engine::new(&m, &[MeasureId(102)])
            .err()
            .unwrap()
            .to_string();
        assert_eq!(App::new(m).err(), Some(want));
    }

    #[test]
    fn pivot_swaps_axes() {
        // Revenue[Time, Product]: rows=Time, cols=Product initially.
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // Quantity[Time, Product] input, 2x2
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Time");
        assert_eq!(app.grid.col_cat.as_ref().unwrap().1, "Product");

        // Pivot: rotate axes -> rows=Product, cols=Time.
        app.pivot();
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Product");
        assert_eq!(app.grid.col_cat.as_ref().unwrap().1, "Time");

        // Pivot again on a 2-D measure returns to the original orientation.
        app.pivot();
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Time");
        assert_eq!(app.grid.col_cat.as_ref().unwrap().1, "Product");
    }

    #[test]
    fn text_and_boolean_cells_are_editable() {
        // Regression: commit_edit only parsed f64, so a Text/Boolean input
        // measure could never be edited ("not a number: 'hi'").
        let mut m = revenue_model();
        let time = CategoryId(1);
        for (id, name, vt) in [
            (200, "Note", ValueType::Text),
            (201, "Flag", ValueType::Boolean),
        ] {
            m.add_measure(Measure {
                id: MeasureId(id),
                name: Name(name.into()),
                value_type: vt,
                categories: vec![time],
                kind: MeasureKind::Input,
                description: None,
            });
        }
        let mut app = App::new(m).unwrap();
        let at = Coordinate::from_pairs([(time, ItemId(10))]);
        view(&mut app, MeasureId(200));
        app.edit = Some("hi there".into());
        app.commit_edit();
        assert_eq!(app.status, None);
        assert_eq!(
            app.model.input(MeasureId(200), &at),
            Some(&Value::Text("hi there".into()))
        );
        app.begin_edit(); // seeds with the text, not blank
        assert_eq!(app.edit.as_deref(), Some("hi there"));

        view(&mut app, MeasureId(201));
        app.edit = Some("maybe".into());
        app.commit_edit();
        assert_eq!(app.status.as_deref(), Some("not true|false: 'maybe'"));
        assert!(app.edit.is_some(), "bad input keeps the buffer open");
        app.edit = Some("true".into());
        app.commit_edit();
        assert_eq!(
            app.model.input(MeasureId(201), &at),
            Some(&Value::Boolean(true))
        );
        // Derived values survive a non-number edit.
        view(&mut app, MeasureId(102));
        assert_eq!(app.grid.value_at(0, 0), Some(1000.0));
        assert!(parse_typed("x", ValueType::DateTime).is_err());
    }

    /// A boolean edit flows through the live engine as one delta (no rebuild):
    /// a derived `Hot = NOT Flag` follows it, and the untouched `Revenue`
    /// keeps its shared snapshot (a rebuild would compute it afresh).
    #[test]
    fn typed_edits_are_incremental() {
        let mut m = revenue_model();
        let time = CategoryId(1);
        m.add_measure(Measure {
            id: MeasureId(201),
            name: Name("Flag".into()),
            value_type: ValueType::Boolean,
            categories: vec![time],
            kind: MeasureKind::Input,
            description: None,
        });
        let at = Coordinate::from_pairs([(time, ItemId(10))]);
        m.set_input(MeasureId(201), at.clone(), Value::Boolean(false));
        let hot = improv_core_model::parser::parse_formula(&m, "Hot = NOT Flag").unwrap();
        m.add_measure(Measure {
            id: MeasureId(202),
            name: Name("Hot".into()),
            value_type: ValueType::Boolean,
            categories: vec![time],
            kind: MeasureKind::Derived(hot.formula),
            description: None,
        });
        let mut app = App::new(m).unwrap();
        let key = encode_coord(&at);
        let hot_at = |app: &App| app.snapshot[&MeasureId(202)].get(&key).cloned();
        assert_eq!(hot_at(&app), Some(CellValue::Bool(true)));
        let revenue = app.snapshot[&MeasureId(102)].clone();
        view(&mut app, MeasureId(201));
        app.edit = Some("true".into());
        app.commit_edit();
        assert_eq!(app.status, None);
        assert_eq!(hot_at(&app), Some(CellValue::Bool(false)));
        assert!(std::sync::Arc::ptr_eq(
            &revenue,
            &app.snapshot[&MeasureId(102)]
        ));
    }

    /// Revenue model plus `Sales[Time, Product, Empty, Region]`: `Empty` has
    /// no items, so it is skipped as a page and `Region` is the first page.
    fn model_with_empty_page() -> Model {
        let mut m = revenue_model();
        m.add_category(CategoryId(3), "Empty");
        m.add_category(CategoryId(4), "Region");
        m.add_item(ItemId(40), CategoryId(4), "North");
        m.add_item(ItemId(41), CategoryId(4), "South");
        m.add_measure(Measure {
            id: MeasureId(300),
            name: Name("Sales".into()),
            value_type: ValueType::Number,
            categories: (1..=4).map(CategoryId).collect(),
            kind: MeasureKind::Input,
            description: None,
        });
        m
    }

    #[test]
    fn paging_past_an_empty_page_dim_wraps_and_edits_are_refused() {
        let mut app = App::new(model_with_empty_page()).unwrap();
        view(&mut app, MeasureId(300));
        app.page_idx.clear(); // stale/short page_idx is resynced
        app.page(-1);
        assert_eq!(app.grid.pages.len(), 1);
        assert_eq!(app.grid.pages[0].item_name, "South", "wrapped backwards");
        app.page(1);
        assert_eq!(app.grid.pages[0].item_name, "North");
        // No full coordinate exists (Empty has no item): editing is refused.
        app.begin_edit();
        assert_eq!(app.status.as_deref(), Some(NO_CELL));
        // A buffer committed there anyway stays open, with the reason.
        app.edit = Some("5".into());
        app.status = None;
        app.commit_edit();
        assert_eq!(app.edit.as_deref(), Some("5"));
        assert_eq!(app.status.as_deref(), Some(NO_CELL));
    }

    #[test]
    fn small_branches_pivot_click_seed_and_no_buffer_commits() {
        let mut app = App::new(revenue_model()).unwrap();
        // Pivot on a 1-D measure is a no-op with a message.
        view(&mut app, MeasureId(100));
        app.axis_order = app.natural_axis_order();
        app.pivot();
        assert!(app
            .status
            .as_deref()
            .unwrap()
            .starts_with("nothing to pivot"));
        // Clicks before any render, left of the grid, or past the window.
        app.click_at(50, 50);
        app.grid_geom = Some(GridGeom {
            x0: 10,
            y0: 5,
            col_w: 12,
            spacing: 1,
            row0: 0,
            col0: 0,
            rows: 1,
            cols: 1,
        });
        app.click_at(5, 6);
        app.click_at(10, 6); // row 1 is outside the 1-row window
        assert_eq!((app.cursor_row, app.cursor_col), (0, 0));
        // Fractional values seed the edit buffer verbatim.
        view(&mut app, MeasureId(101));
        app.apply_edit(0, 0, 2.5).unwrap();
        app.begin_edit();
        assert_eq!(app.edit.as_deref(), Some("2.5"));
        app.edit = Some("abc".into());
        app.commit_edit();
        assert_eq!(app.status.as_deref(), Some("not a number: 'abc'"));
        app.edit = Some("false".into());
        assert_eq!(
            parse_typed("false", ValueType::Boolean),
            Ok(Value::Boolean(false))
        );
        // Commits with no active buffer do nothing.
        app.edit = None;
        app.status = None;
        app.commit_edit();
        app.commit_command();
        assert_eq!(app.status, None);
        // A derived measure's edit line is its formula.
        view(&mut app, MeasureId(102));
        app.edit = Some("1".into());
        app.commit_edit();
        assert_eq!(app.status, None);
        assert_eq!(app.formula_text().as_deref(), Some("1"));
    }

    #[test]
    fn views_cycle_wrap_and_tolerate_missing_measures() {
        let mut app = App::new(revenue_model()).unwrap();
        app.cycle_view();
        assert!(app.status.as_deref().unwrap().starts_with("no saved views"));
        view(&mut app, MeasureId(101));
        let v1 = app.save_view("one", None);
        app.next_measure();
        let v2 = app.save_view("two", None);
        assert_eq!(v2.0, v1.0 + 1);
        app.cycle_view();
        assert_eq!(app.status.as_deref(), Some("view: one"));
        app.cycle_view();
        assert_eq!(app.status.as_deref(), Some("view: two"));
        app.cycle_view();
        assert_eq!(app.status.as_deref(), Some("view: one"), "wraps");
        // A view with an empty axis order falls back to natural order; one
        // whose measure is gone is refused.
        let mut v = app.build_view(ViewId(99), "bare");
        v.axis_order.clear();
        app.apply_view(&v);
        assert_eq!(app.axis_order, app.natural_axis_order());
        v.measure = MeasureId(9999);
        app.apply_view(&v);
        assert_eq!(
            app.status.as_deref(),
            Some("view's measure no longer exists")
        );
        // A save to an unwritable path reports the failure.
        app.save_view("bad", Some("/nonexistent/dir/x.db"));
        assert!(app.status.as_deref().unwrap().starts_with("save failed"));
    }

    #[test]
    fn re_showing_every_filtered_item_drops_the_filter() {
        let mut app = App::new(revenue_model()).unwrap();
        let time = CategoryId(1);
        app.toggle_filter_item(time, ItemId(10));
        app.toggle_filter_item(time, ItemId(11));
        app.toggle_filter_item(time, ItemId(10));
        assert_eq!(app.filters.len(), 1, "one item still hidden");
        app.toggle_filter_item(time, ItemId(11));
        assert!(app.filters.is_empty(), "all shown again => unfiltered");
        // An item from another category does not count as re-showing one.
        app.toggle_filter_item(time, ItemId(10));
        app.toggle_filter_item(time, ItemId(20));
        assert_eq!(app.filters.len(), 1, "2025 is still hidden");
    }

    #[test]
    fn grid_falls_back_on_bad_axis_order_and_unknown_ids() {
        let m = revenue_model();
        let s = Snapshot::default();
        // Not a permutation of the measure's categories: natural order wins.
        let g = build_grid_pivoted(&m, MeasureId(101), &s, &[], Some(&[CategoryId(2)]), &[]);
        assert_eq!(g.row_cat.unwrap().1, "Time");
        // Unknown measure: id as name, a single synthetic cell.
        let g = build_grid(&m, MeasureId(77), &s);
        assert_eq!(
            (g.measure_name.as_str(), g.n_rows(), g.n_cols()),
            ("77", 1, 1)
        );
        // Dangling category/item ids render as their numbers.
        let mut m = m;
        m.categories
            .get_mut(&CategoryId(2))
            .unwrap()
            .items
            .push(ItemId(29));
        let mut meas = m.measures[&MeasureId(101)].clone();
        meas.id = MeasureId(400);
        meas.categories = vec![CategoryId(2), CategoryId(8)];
        m.add_measure(meas);
        let g = build_grid(&m, MeasureId(400), &s);
        assert_eq!(g.rows.last().unwrap().1, "29");
        assert_eq!(g.col_cat.unwrap().1, "8");
        let mut app = App::new(m).unwrap();
        view(&mut app, MeasureId(400));
        app.axis_order = app.natural_axis_order();
        app.pivot();
        assert_eq!(app.status.as_deref(), Some("pivot: rows=8 cols=Product"));
    }

    #[test]
    fn shift_arrows_select_a_range_and_sum_it() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // [[100, 50], [120, 80]]
        assert_eq!(app.selection_sum(), Some(100.0));
        app.step(1, 0, true);
        app.step(0, 1, true);
        assert_eq!(app.anchor, Some((0, 0)));
        assert!(app.is_selected(1, 0));
        assert!(app.is_selected(0, 1));
        assert_eq!(app.selection_sum(), Some(350.0));
        app.step(-1, 0, false);
        assert_eq!((app.anchor, app.selection_sum()), (None, Some(50.0)));
        app.step(0, -1, true);
        app.clear_filters(); // any re-layout collapses the range
        assert_eq!(app.anchor, None);
        // Formula text for derived measures only; blank cells sum to None.
        assert_eq!(app.formula_text(), None);
        view(&mut app, MeasureId(102));
        assert_eq!(app.formula_text().as_deref(), Some("Price times Quantity"));
        let mut m = revenue_model();
        m.inputs.clear();
        assert_eq!(App::new(m).unwrap().selection_sum(), None);
    }

    #[test]
    fn edit_on_fully_filtered_axis_is_refused_not_written() {
        // Regression: filtering out every row left a cursor on "no row"; an
        // edit then stored a cell whose coordinate lacked the row category.
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        let time = CategoryId(1);
        app.toggle_filter_item(time, ItemId(10));
        app.toggle_filter_item(time, ItemId(11));
        assert!(app.grid.rows.is_empty());
        let before = app.model.inputs.len();
        app.begin_edit();
        assert!(app.edit.is_none());
        assert_eq!(app.status.as_deref(), Some(NO_CELL));
        assert_eq!(app.apply_edit(0, 0, 7.0), Err(NO_CELL.to_string()));
        assert_eq!(app.model.inputs.len(), before);
    }

    #[test]
    fn click_moves_cursor_to_cell() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // 2x2
                                        // Simulate the geometry a render would record.
        app.grid_geom = Some(GridGeom {
            x0: 2,
            y0: 5,
            col_w: 12,
            spacing: 1,
            row0: 0,
            col0: 0,
            rows: 10,
            cols: 10,
        });
        // Click in data row 1, column 1: x within the 2nd column (stride 13).
        app.click_at(2 + 13, 5 + 1);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
        // A click above the grid (in the status area) is ignored.
        app.click_at(2, 0);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
        // Inside the window but below or right of the 2x2 grid: ignored.
        app.click_at(2, 5 + 5);
        app.click_at(2 + 13 * 5, 5);
        assert_eq!((app.cursor_row, app.cursor_col), (1, 1));
    }

    #[test]
    fn filter_hides_a_row_item_from_the_grid() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // Quantity[Time, Product]: rows=Time(2025,2026)
        assert_eq!(app.grid.n_rows(), 2);
        // Hide 2026 (ItemId 11) from the Time row axis.
        app.toggle_filter_item(CategoryId(1), ItemId(11));
        let rows: Vec<ItemId> = app.grid.rows.iter().map(|(id, _)| *id).collect();
        assert_eq!(rows, vec![ItemId(10)], "2026 filtered out of rows");
        // Columns (unfiltered Product) still show both.
        assert_eq!(app.grid.n_cols(), 2);
        // Toggling it back restores both rows (filter dropped when it keeps all).
        app.toggle_filter_item(CategoryId(1), ItemId(11));
        assert_eq!(app.grid.n_rows(), 2);
        assert!(app.filters.is_empty(), "unfiltered again");
    }

    #[test]
    fn save_view_captures_axis_order_and_filters() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101)); // Quantity[Time, Product]
        app.pivot(); // rows=Product, cols=Time
        app.toggle_filter_item(CategoryId(2), ItemId(21)); // hide WidgetB

        let id = app.save_view("L1", None);
        let v = app.model.views.get(&id).expect("view saved");
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
        let mut src = App::new(revenue_model()).unwrap();
        view(&mut src, MeasureId(101));
        src.pivot(); // rows=Product, cols=Time
        src.toggle_filter_item(CategoryId(2), ItemId(21)); // hide WidgetB
        let v = src.build_view(ViewId(1), "L1");

        let mut dst = App::new(revenue_model()).unwrap();
        assert_ne!(dst.grid.measure, MeasureId(101)); // starts on derived Revenue
        dst.apply_view(&v);

        assert_eq!(dst.grid.measure, MeasureId(101));
        assert_eq!(dst.axis_order, vec![CategoryId(2), CategoryId(1)]);
        assert_eq!(dst.filters, v.filters);
        // The filter is reflected in the grid: WidgetB gone from the row axis.
        let rows: Vec<ItemId> = dst.grid.rows.iter().map(|(id, _)| *id).collect();
        assert_eq!(rows, vec![ItemId(20)], "only WidgetA on rows");
        assert_eq!(dst.grid.row_cat.as_ref().unwrap().1, "Product");
        assert_eq!(dst.grid.col_cat.as_ref().unwrap().1, "Time");
    }

    #[test]
    fn apply_view_restores_page_item() {
        // 3-D Sales[Time, Product, Region]: save on South, apply to a fresh app.
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
        let c = |p: &[(CategoryId, ItemId)]| Coordinate::from_pairs(p.iter().copied());
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

        let mut src = App::new(m.clone()).unwrap();
        src.page(1); // page Region to South
        assert_eq!(src.grid.pages[0].item_name, "South");
        let v = src.build_view(ViewId(1), "south");
        assert_eq!(v.page_items, vec![(region, ItemId(31))]);

        let mut dst = App::new(m).unwrap();
        assert_eq!(dst.grid.pages[0].item_name, "North");
        dst.apply_view(&v);
        assert_eq!(dst.grid.pages[0].item_name, "South", "page item restored");
    }

    // -- CSV import/export command prompt -----------------------------------

    pub(crate) fn tmp_path(suffix: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "improv_tui_test_{}_{}{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            suffix
        ))
    }

    struct TmpFile(PathBuf);
    impl Drop for TmpFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    #[test]
    fn parse_import_line_builds_expected_spec() {
        let spec = parse_import_line("sales.csv 100 Revenue revenue time:1:Time product:2:Product")
            .unwrap();
        assert_eq!(spec.path, PathBuf::from("sales.csv"));
        assert_eq!(spec.delimiter, b',');
        assert!(spec.has_header);
        assert_eq!(spec.measure_id, MeasureId(100));
        assert_eq!(spec.measure_name, "Revenue");
        assert_eq!(spec.value_column, ColumnRef::Name("revenue".into()));
        assert_eq!(spec.dimensions.len(), 2);
        assert_eq!(spec.dimensions[0].column, ColumnRef::Name("time".into()));
        assert_eq!(spec.dimensions[0].category_id, CategoryId(1));
        assert_eq!(spec.dimensions[0].category_name, "Time");
        assert_eq!(spec.dimensions[1].column, ColumnRef::Name("product".into()));
        assert_eq!(spec.dimensions[1].category_id, CategoryId(2));
        assert_eq!(spec.dimensions[1].category_name, "Product");
    }

    #[test]
    fn parse_import_line_flags_flip_delimiter_and_header() {
        let spec = parse_import_line("sales.tsv 100 Revenue revenue time:1:Time --tsv --no-header")
            .unwrap();
        assert_eq!(spec.delimiter, b'\t');
        assert!(!spec.has_header);
        // Without a header, bare column refs parse as 0-based indices.
        assert_eq!(spec.value_column, ColumnRef::Name("revenue".into()));
    }

    #[test]
    fn parse_import_line_no_header_uses_index_columns() {
        let spec = parse_import_line("sales.csv 100 Revenue 2 0:1:Time --no-header").unwrap();
        assert_eq!(spec.value_column, ColumnRef::Index(2));
        assert_eq!(spec.dimensions[0].column, ColumnRef::Index(0));
    }

    #[test]
    fn parse_import_line_rejects_malformed_input() {
        assert!(parse_import_line("").is_err());
        assert!(parse_import_line("sales.csv 100 Revenue revenue").is_err()); // no dims
        assert!(parse_import_line("sales.csv notanumber Revenue revenue time:1:Time").is_err());
        assert!(parse_import_line("sales.csv 100 Revenue revenue time:notanumber:Time").is_err());
        assert!(parse_import_line("sales.csv 100 Revenue revenue badspec").is_err());
    }

    #[test]
    fn parse_export_line_builds_expected_tuple() {
        let (target, mid, delim) = parse_export_line("out.csv 100").unwrap();
        assert_eq!(target, "out.csv");
        assert_eq!(mid, MeasureId(100));
        assert_eq!(delim, b',');

        let (_, _, delim_tsv) = parse_export_line("out.csv 100 --tsv").unwrap();
        assert_eq!(delim_tsv, b'\t');
    }

    #[test]
    fn parse_export_line_rejects_malformed_input() {
        assert!(parse_export_line("").is_err());
        assert!(parse_export_line("out.csv").is_err());
        assert!(parse_export_line("out.csv notanumber").is_err());
    }

    /// A minimal starter model with one placeholder input measure, so
    /// `App::new` succeeds (it requires >= 1 measure) before import-csv adds
    /// the real one under test.
    fn starter_model() -> Model {
        let mut m = Model::new();
        let cat = CategoryId(9);
        m.add_category(cat, "Placeholder");
        m.add_measure(Measure {
            id: MeasureId(1),
            name: Name("Placeholder".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Input,
            description: None,
        });
        m
    }

    #[test]
    fn import_command_end_to_end_adds_measure_and_cells() {
        let csv_path = tmp_path(".csv");
        let _guard = TmpFile(csv_path.clone());
        std::fs::write(
            &csv_path,
            "time,product,revenue\n2025,WidgetA,1000\n2025,WidgetB,500\n2026,WidgetA,1200\n",
        )
        .unwrap();

        let mut app = App::new(starter_model()).unwrap();
        app.begin_import_csv();
        assert_eq!(app.command.as_ref().unwrap().0, CommandKind::ImportCsv);
        app.command.as_mut().unwrap().1 = format!(
            "{} 100 Revenue revenue time:1:Time product:2:Product",
            csv_path.display()
        );
        app.commit_command();

        assert!(app.command.is_none(), "command mode exited");
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("imported 3"), "got: {status}");

        let m = app.model.measure_by_name("Revenue").unwrap();
        assert!(m.is_input());
        assert_eq!(app.model.inputs.len(), 3);
        // The new measure is now selectable/viewable through the App.
        assert!(app.measures.contains(&MeasureId(100)));
    }

    /// An import that changes a measure a formula uses reports that the
    /// formulas no longer build, instead of a bare success (the derived
    /// measures would silently go blank). One that does not keeps them live.
    #[test]
    fn import_reports_formulas_it_breaks() {
        let csv_path = tmp_path(".csv");
        let _guard = TmpFile(csv_path.clone());
        std::fs::write(&csv_path, "region,v\nNorth,1\nSouth,2\n").unwrap();
        let import = |spec: &str| {
            let mut app = App::new(revenue_model()).unwrap();
            app.begin_import_csv();
            app.command.as_mut().unwrap().1 = format!("{} {spec}", csv_path.display());
            app.commit_command();
            app
        };
        // Price becomes Price[Region]: Revenue = Price * Quantity breaks.
        let app = import("100 Price v region:7:Region");
        let s = app.status.clone().unwrap();
        assert!(
            s.starts_with("imported 2 cell(s) into measure 'Price'; formulas not recomputed: "),
            "{s}"
        );
        assert!(app.engine.is_none());
        // A new measure leaves Revenue computing.
        let mut app = import("200 Sales v region:7:Region");
        assert_eq!(
            app.status.as_deref(),
            Some("imported 2 cell(s) into measure 'Sales'")
        );
        assert!(app.engine.is_some());
        view(&mut app, MeasureId(102));
        assert_eq!(app.grid.display_at(0, 0), "1000");
        // The viewed measure stays, re-paged from its first item.
        let mut app = App::new(model_with_empty_page()).unwrap();
        view(&mut app, MeasureId(300));
        app.page(1);
        assert_eq!(app.grid.pages[0].item_name, "South");
        app.begin_import_csv();
        app.command.as_mut().unwrap().1 =
            format!("{} 201 Other v region:4:Region", csv_path.display());
        app.commit_command();
        assert_eq!(
            app.status.as_deref(),
            Some("imported 2 cell(s) into measure 'Other'")
        );
        assert_eq!(app.grid.measure, MeasureId(300));
        assert_eq!(app.grid.pages[0].item_name, "North");
        assert_eq!(app.page_idx, vec![0]);
    }

    #[test]
    fn undo_and_redo_walk_cell_and_formula_changes() {
        let mut app = App::new(revenue_model()).unwrap();
        let start = app.model.clone();
        app.undo();
        assert_eq!(app.status.as_deref(), Some("nothing to undo"));
        app.redo();
        assert_eq!(app.status.as_deref(), Some("nothing to redo"));
        view(&mut app, MeasureId(102));
        app.begin_edit();
        app.edit = Some("Price * Quantity * 2".into());
        app.commit_edit();
        // A cell edit in a pivoted grid: the undo keeps the layout.
        view(&mut app, MeasureId(101));
        app.pivot();
        app.apply_edit(0, 0, 7.0).unwrap();
        let edited = app.model.clone();
        app.undo();
        assert_eq!(app.status.as_deref(), Some("undone"));
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Product");
        assert_eq!(app.grid.value_at(0, 0), Some(100.0));
        app.undo();
        assert_eq!(app.model, start);
        view(&mut app, MeasureId(102));
        assert_eq!(app.grid.display_at(0, 0), "1000", "the engine was rebuilt");
        app.redo();
        app.redo();
        assert_eq!(app.status.as_deref(), Some("redone"));
        assert_eq!(app.model, edited);
        assert_eq!(app.grid.display_at(0, 0), "140", "Price * 7 * 2");
        // A view saved after a change survives undoing it.
        app.save_view("kept", None);
        app.undo();
        assert_eq!(app.model.views.len(), 1);
        app.redo();
        assert_eq!(app.model.views.len(), 1);
        // A new change after an undo drops the undone future.
        app.undo();
        view(&mut app, MeasureId(101));
        app.apply_edit(1, 1, 1.0).unwrap();
        app.redo();
        assert_eq!(app.status.as_deref(), Some("nothing to redo"));
    }

    #[test]
    fn categories_items_and_input_measures_from_nothing() {
        let mut app = App::new(Model::new()).unwrap();
        assert!(app.measures.is_empty());
        app.begin_edit();
        assert_eq!(app.status.as_deref(), Some(EMPTY));
        app.next_measure();
        app.pivot();
        let run = |app: &mut App, kind, line: &str| {
            app.begin_command(kind);
            app.command.as_mut().unwrap().1 = line.into();
            app.commit_command();
            app.status.clone().unwrap()
        };
        assert!(run(&mut app, CommandKind::Category, " : x").starts_with("category: type Name"));
        assert_eq!(
            run(&mut app, CommandKind::Category, "Time: 2025, , 2026"),
            "category Time: 2 item(s)"
        );
        assert_eq!(
            run(&mut app, CommandKind::Category, "Time: 2026"),
            "category: 'Time' has those items"
        );
        assert_eq!(
            run(&mut app, CommandKind::Category, "Time: 2027"),
            "category Time: 3 item(s)"
        );
        assert_eq!(
            run(&mut app, CommandKind::Category, "Product"),
            "category Product: 0 item(s)"
        );
        assert!(run(&mut app, CommandKind::Input, "").starts_with("input: type Name"));
        assert_eq!(
            run(&mut app, CommandKind::Input, "Units: Time, Region"),
            "input: no category 'Region' (c adds one)"
        );
        assert_eq!(
            run(&mut app, CommandKind::Input, "Units: Time"),
            "added input measure Units"
        );
        assert_eq!(
            run(&mut app, CommandKind::Input, "Units"),
            "input: 'Units' is taken"
        );
        let time = app.model.category_by_name("Time").unwrap();
        assert_eq!(time.id, CategoryId(1));
        let names: Vec<&str> = time
            .items
            .iter()
            .map(|i| app.model.items[i].name.0.as_str())
            .collect();
        assert_eq!(names, ["2025", "2026", "2027"]);
        assert_eq!(app.model.items.len(), 3);
        // The new measure is shown, and its cells take values.
        assert_eq!(app.grid.measure_name, "Units");
        assert_eq!(app.grid.n_rows(), 3);
        app.apply_edit(2, 0, 9.0).unwrap();
        // A scalar measure; ids continue above the others.
        assert_eq!(
            run(&mut app, CommandKind::Input, "Rate"),
            "added input measure Rate"
        );
        assert_eq!(app.grid.measure, MeasureId(2));
        assert!(app.model.measures[&MeasureId(2)].categories.is_empty());
        // Every step undoes.
        app.undo();
        app.undo();
        assert_eq!(app.grid.measure_name, "Units");
        assert!(app.grid.display_at(2, 0).is_empty());
        for _ in 0..4 {
            app.undo();
        }
        assert!(app.measures.is_empty());
        assert!(app.model.categories.is_empty());
        // With formulas that no longer build, adding still works and says so.
        let mut m = revenue_model();
        m.measures.get_mut(&MeasureId(102)).unwrap().kind = MeasureKind::Derived(Formula::new(
            Expr::Ref(MeasureId(102), DimensionSpec::default()),
        ));
        let mut app = App::new(revenue_model()).unwrap();
        app.model = m;
        app.begin_command(CommandKind::Category);
        app.command.as_mut().unwrap().1 = "Region".into();
        app.commit_command();
        let s = app.status.clone().unwrap();
        assert!(
            s.starts_with("category Region: 0 item(s); formulas not recomputed"),
            "{s}"
        );
    }

    #[test]
    fn define_line_adds_measures_and_lambdas() {
        let mut app = App::new(revenue_model()).unwrap();
        let define = |app: &mut App, text: &str| {
            app.begin_define();
            app.edit = Some(text.into());
            app.commit_edit();
        };
        define(&mut app, "Profit = Revenue - Price");
        assert_eq!(app.status.as_deref(), Some("defined Profit"));
        assert!(app.edit.is_none());
        assert!(!app.defining);
        assert_eq!(app.grid.measure_name, "Profit");
        assert_eq!(app.grid.display_at(0, 0), "990");
        let profit = app.grid.measure;
        assert_eq!(profit, MeasureId(103));
        // A new formula for an existing derived measure keeps its id.
        define(&mut app, "Revenue = Price * Quantity * 2");
        assert_eq!(app.grid.measure, MeasureId(102));
        assert_eq!(app.grid.display_at(0, 0), "2000");
        view(&mut app, profit);
        assert_eq!(app.grid.display_at(0, 0), "1990", "dependents recompute");
        // A recurrence names itself; the quoted name is unquoted.
        define(
            &mut app,
            "'Run Total' = PREV('Run Total', Time, 1, 0) + Quantity",
        );
        assert_eq!(
            app.status.as_deref(),
            Some("defined Run Total"),
            "{:?}",
            app.diagnostic
        );
        // Named LAMBDAs: defined, called, redefined in place.
        define(&mut app, "Margin = LAMBDA(p, q, p - q / 2)");
        assert_eq!(app.status.as_deref(), Some("defined LAMBDA Margin(p, q)"));
        assert_eq!(app.model.lambdas.keys().copied().collect::<Vec<_>>(), [1]);
        define(&mut app, "Net = Margin(Price, 2)");
        assert_eq!(app.grid.display_at(0, 0), "9");
        let net = app.grid.measure;
        define(&mut app, "Margin = LAMBDA(p, q, p - q)");
        assert_eq!(app.model.lambdas.keys().copied().collect::<Vec<_>>(), [1]);
        view(&mut app, net);
        assert_eq!(app.grid.display_at(0, 0), "8");
        // Each one undoes.
        app.undo();
        view(&mut app, net);
        assert_eq!(app.grid.display_at(0, 0), "9");
        // Refusals keep the line open and change nothing.
        for (line, want) in [
            ("Hours: Time", "added input measure Hours"),
            ("Margin", "input: 'Margin' is taken"),
        ] {
            app.begin_command(CommandKind::Input);
            app.command.as_mut().unwrap().1 = line.into();
            app.commit_command();
            assert_eq!(app.status.as_deref(), Some(want));
        }
        let before = app.model.clone();
        for (text, want) in [
            ("Price = 1", "'Price' is an input measure"),
            (
                "Quantity = LAMBDA(x, x)",
                "'Quantity' is a measure; name the LAMBDA differently",
            ),
            (
                "Feed = SQL(\"select 1\")",
                "SQL(…) and CALL(…) sources need the CLI",
            ),
            ("Revenue = Profit", "cycle"),
            ("Odd = Hours + Price", "neither operand dim"),
        ] {
            define(&mut app, text);
            let s = app.status.clone().unwrap_or_default();
            assert!(app.defining, "{text}");
            assert!(s.contains(want), "{text}: {s} {:?}", app.diagnostic);
            assert_eq!(app.model, before, "{text}");
        }
        // A parse error is located in the line, like a formula edit's.
        define(&mut app, "Odd = Price * Quantty");
        let d = app.diagnostic.clone().unwrap();
        assert_eq!(d.span, Some(14..21));
        assert!(app.status.is_none());
        // The line completes names, and Esc leaves it.
        app.edit = Some("Odd = Pri".into());
        app.edit_key(KeyCode::Tab);
        assert_eq!(app.edit.as_deref(), Some("Odd = Price"));
        assert!(app.editing_formula());
        app.cancel_edit();
        assert!(!app.defining);
    }

    /// A save writes what changed since the last one; a failed save keeps
    /// its changes for the next, so the store ends equal to the model.
    #[test]
    fn saves_write_what_changed_and_a_failed_one_loses_nothing() {
        let path = tmp_path("tui-incr.db");
        let _guard = TmpFile(path.clone());
        let p = path.to_str().unwrap();
        let mut app = App::new(revenue_model()).unwrap();
        app.save(p).unwrap();
        view(&mut app, MeasureId(101));
        app.apply_edit(0, 0, 5.0).unwrap();
        assert!(app.save("/nonexistent/dir/x.db").is_err());
        app.save_view("kept", None);
        app.apply_edit(0, 0, 7.0).unwrap();
        app.undo();
        app.save(p).unwrap();
        let stored = || improv_store::Store::open(p).unwrap().load_model().unwrap();
        assert_eq!(stored(), app.model);
        app.redo();
        app.save(p).unwrap();
        assert_eq!(stored(), app.model);
        assert_eq!(app.saved.as_ref(), Some(&app.model));
    }

    #[test]
    fn write_saves_and_reports_failure() {
        let path = tmp_path("tui-write.db");
        let _guard = TmpFile(path.clone());
        let p = path.to_str().unwrap();
        let mut app = App::new(revenue_model()).unwrap();
        app.write(p);
        assert_eq!(app.status.as_deref(), Some("saved"));
        view(&mut app, MeasureId(101));
        app.apply_edit(0, 0, 5.0).unwrap();
        app.write(p);
        app.write("/nonexistent/dir/x.db");
        assert!(app.status.as_deref().unwrap().starts_with("save failed: "));
        let mut s = improv_store::Store::open(p).unwrap();
        let m = s.load_model().unwrap();
        assert_eq!(m.inputs, app.model.inputs, "a second save in one session");
    }

    #[test]
    fn imports_undo_and_a_failed_one_changes_nothing() {
        let csv_path = tmp_path(".csv");
        let _guard = TmpFile(csv_path.clone());
        std::fs::write(&csv_path, "region,v\nNorth,1\nSouth,2\n").unwrap();
        let mut app = App::new(revenue_model()).unwrap();
        let start = app.model.clone();
        let import = |app: &mut App, line: String| {
            app.begin_import_csv();
            app.command.as_mut().unwrap().1 = line;
            app.commit_command();
        };
        // Price becomes Price[Region]: the formulas no longer build.
        view(&mut app, MeasureId(100));
        import(
            &mut app,
            format!("{} 100 Price v region:7:Region", csv_path.display()),
        );
        assert!(app.engine.is_none());
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Region");
        app.undo();
        assert_eq!(app.model, start);
        assert!(app.engine.is_some());
        // The undo changed Price's categories: its natural layout is back.
        assert_eq!(app.grid.row_cat.as_ref().unwrap().1, "Product");
        app.redo();
        let s = app.status.clone().unwrap();
        assert!(s.starts_with("redone; formulas not recomputed: "), "{s}");
        app.undo();
        let missing = tmp_path(".csv");
        import(
            &mut app,
            format!("{} 300 X v region:7:Region", missing.display()),
        );
        assert!(app.status.as_deref().unwrap().starts_with("import-csv:"));
        assert_eq!(app.model, start);
        assert_eq!(app.redo_stack.len(), 1, "a failed import is not a change");
        // Quantity[Time, Product] becomes Quantity[Region], and back.
        view(&mut app, MeasureId(101));
        import(
            &mut app,
            format!("{} 101 Quantity v region:7:Region", csv_path.display()),
        );
        assert_eq!(app.axis_order, [CategoryId(7)]);
        app.undo();
        assert_eq!(app.axis_order, [CategoryId(1), CategoryId(2)]);
    }

    #[test]
    fn undo_history_is_bounded() {
        let mut app = App::new(revenue_model()).unwrap();
        view(&mut app, MeasureId(101));
        for i in 0..UNDO_DEPTH + 5 {
            app.apply_edit(0, 0, i as f64).unwrap();
        }
        assert_eq!(app.undo_stack.len(), UNDO_DEPTH);
        // The five oldest points are gone: undoing it all stops at 4.
        for _ in 0..UNDO_DEPTH {
            app.undo();
        }
        assert_eq!(app.grid.value_at(0, 0), Some(4.0));
        app.undo();
        assert_eq!(app.status.as_deref(), Some("nothing to undo"));
    }

    #[test]
    fn import_command_reports_parse_error_and_exits_mode() {
        let mut app = App::new(starter_model()).unwrap();
        app.begin_import_csv();
        app.command.as_mut().unwrap().1 = "sales.csv notanumber Revenue revenue".to_string();
        app.commit_command();
        assert!(app.command.is_none());
        assert!(app
            .status
            .as_deref()
            .unwrap_or("")
            .starts_with("import-csv:"));
        // A well-formed line naming a missing file: the I/O error, no measure.
        let missing = tmp_path(".missing.csv");
        app.begin_import_csv();
        app.command.as_mut().unwrap().1 = format!("{} 100 Revenue v t:9:P", missing.display());
        app.commit_command();
        assert!(app.status.as_deref().unwrap().starts_with("import-csv: "));
        assert!(app.model.measure_by_name("Revenue").is_none());
    }

    #[test]
    fn export_command_end_to_end_round_trips() {
        // First import into a fresh measure, then export it and read the file back.
        let csv_path = tmp_path(".csv");
        let _csv_guard = TmpFile(csv_path.clone());
        std::fs::write(
            &csv_path,
            "time,product,revenue\n2025,WidgetA,1000\n2026,WidgetA,1200\n",
        )
        .unwrap();

        let mut app = App::new(starter_model()).unwrap();
        app.begin_import_csv();
        app.command.as_mut().unwrap().1 = format!(
            "{} 100 Revenue revenue time:1:Time product:2:Product",
            csv_path.display()
        );
        app.commit_command();
        assert!(app.status.as_deref().unwrap_or("").contains("imported 2"));

        let out_path = tmp_path(".out.csv");
        let _out_guard = TmpFile(out_path.clone());
        app.begin_export_csv();
        app.command.as_mut().unwrap().1 = format!("{} 100", out_path.display());
        app.commit_command();
        assert!(app.command.is_none());
        let status = app.status.clone().unwrap_or_default();
        assert!(status.contains("exported 2"), "got: {status}");

        let contents = std::fs::read_to_string(&out_path).unwrap();
        assert!(contents.contains("Time"));
        assert!(contents.contains("Product"));
        assert!(contents.contains("Revenue"));
        assert_eq!(contents.lines().count(), 3); // header + 2 rows
    }

    #[test]
    fn export_command_reports_error_for_unknown_measure() {
        let mut app = App::new(starter_model()).unwrap();
        app.begin_export_csv();
        app.command.as_mut().unwrap().1 = "out.csv 999".to_string();
        app.commit_command();
        assert!(app.command.is_none());
        assert!(app
            .status
            .as_deref()
            .unwrap_or("")
            .starts_with("export-csv:"));
    }

    #[test]
    fn cancel_command_clears_state_without_side_effects() {
        let mut app = App::new(starter_model()).unwrap();
        app.begin_import_csv();
        app.command.as_mut().unwrap().1 = "whatever".to_string();
        app.cancel_command();
        assert!(app.command.is_none());
        assert!(app.status.is_none());
    }
}
