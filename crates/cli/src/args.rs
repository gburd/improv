//! The `improv` command line, declared once. The parser, `--help`, the man
//! pages and the shell completions (`tests/man.rs`) all come from it. The
//! value parsers it names are in `main.rs`.

use crate::{dim_spec, number, refresh_policy, value_type};
use clap::{Args, Parser, Subcommand};
use improv_core_model::{CategoryId, RefreshPolicy, ValueType};

const DB: &str = "The model store: a file, or postgres://USER@HOST/DB?model=NAME";
const MEASURE_ID: &str = "A measure's numeric id";
const AT: &str = "The cell's coordinate: Category=Item pairs, comma-joined, \
                  space-separated, or in repeated --at flags";
const DIMS: &str = "COLUMN:CATEGORY-ID:CATEGORY-NAME: a column, and the category \
                    its distinct values become items of";
const REFRESH: &str = "When serve-refresh re-runs it: manual, on-load or interval:SECS";

#[derive(Parser, Debug)]
#[command(
    name = "improv",
    // Not argv[0]: "improv.exe" on Windows would change every help text.
    bin_name = "improv",
    version,
    about = "headless multidimensional spreadsheet model CLI",
    long_about = "Build, edit and evaluate Improv models from the shell. Each command \
                  opens a model store, reads or changes the model, and saves it back. \
                  Run 'improv help COMMAND' for one command.",
    args_override_self = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Cmd>,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Create or open a store and install the schema.
    ///
    /// An existing model is kept. An empty string is an in-memory store.
    Init {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
    },
    /// Add a category (a dimension, such as Time).
    AddCategory {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The category's numeric id
        #[arg(value_name = "id", value_parser = number::<u32>("<id>"))]
        id: u32,
        /// The category's name
        #[arg(value_name = "name", allow_hyphen_values = true)]
        name: String,
    },
    /// Add an item (a member of a category).
    ///
    /// The category must exist.
    AddItem {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The item's numeric id
        #[arg(value_name = "id", value_parser = number::<u32>("<id>"))]
        id: u32,
        /// The id of the item's category
        #[arg(value_name = "category-id", value_parser = number::<u32>("<category-id>"))]
        category: u32,
        /// The item's name
        #[arg(value_name = "name", allow_hyphen_values = true)]
        name: String,
    },
    /// Add an input measure.
    ///
    /// Its type says how set parses its values. The word input is required.
    /// The categories it ranges over follow by name, e.g. input Time Product.
    AddMeasure {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The measure's numeric id
        #[arg(value_name = "id", value_parser = number::<u32>("<id>"))]
        id: u32,
        /// The measure's name
        #[arg(value_name = "name", allow_hyphen_values = true)]
        name: String,
        /// number, boolean or text
        #[arg(value_name = "type", value_parser = value_type)]
        value_type: ValueType,
        /// The word input
        #[arg(allow_hyphen_values = true, value_name = "input")]
        kind: String,
        /// The categories the measure ranges over, by name
        #[arg(allow_hyphen_values = true, value_name = "category")]
        categories: Vec<String>,
    },
    /// Add a derived (formula) measure.
    ///
    /// The formula is the right-hand side only, e.g. "Price * Quantity" or
    /// "SUM(Revenue OVER Time)", with names resolved against the model. The
    /// measure's categories come from the measures it references. A formula
    /// cycle is refused.
    AddDerived {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The measure's numeric id
        #[arg(value_name = "id", value_parser = number::<u32>("<id>"))]
        id: u32,
        /// The measure's name
        #[arg(value_name = "name", allow_hyphen_values = true)]
        name: String,
        /// The formula text (in IMPROV_LOCALE's spelling)
        #[arg(value_name = "formula", allow_hyphen_values = true)]
        formula: String,
    },
    /// Create a measure or LAMBDA from a definition, "Target = ...".
    ///
    /// Three forms: a formula, "Revenue = Price * Quantity"; an external call,
    /// "H = CALL(hypot, Price, Quantity)", which refresh-ext populates; or a
    /// named LAMBDA, "Margin = LAMBDA(p, c, p - c)", which formulas then call
    /// as Margin(Price, Cost). The id names a LAMBDA, and redefining one keeps
    /// its first id. The SQL(...) form needs a column-to-dimension map: use
    /// import-sql. A formula cycle is refused.
    Define {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The measure's (or LAMBDA's) numeric id
        #[arg(value_name = "id", value_parser = number::<u32>("<id>"))]
        id: u32,
        /// The definition: Target = formula, CALL(...) or LAMBDA(...)
        #[arg(value_name = "definition", allow_hyphen_values = true)]
        definition: String,
        /// A CALL(...) measure's refresh policy
        #[arg(long, value_name = "policy", value_parser = refresh_policy,
              default_value = "manual", long_help = REFRESH)]
        refresh: RefreshPolicy,
    },
    /// Register a Python external function.
    #[command(long_about = "Register a Python external function.\n\n\
        Number arguments and a Number result: the body reads the list args and \
        sets result, e.g. register-ext m.db hypot 2 \
        \"result = (args[0]**2 + args[1]**2) ** 0.5\".")]
    RegisterExt {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The function's name, as CALL(...) names it
        #[arg(allow_hyphen_values = true, value_name = "name")]
        name: String,
        /// How many arguments it takes
        #[arg(value_name = "arity", value_parser = number::<u32>("<arity>"))]
        arity: u32,
        /// The Python body
        #[arg(value_name = "python-body", allow_hyphen_values = true)]
        body: String,
    },
    /// Run a CALL(...) measure's function and store its cells.
    ///
    /// The function runs host-side, over its argument measures; external calls
    /// stay off the dataflow engine.
    RefreshExt {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        #[arg(value_name = "measure-id", help = MEASURE_ID,
              value_parser = number::<u32>("<measure-id>"))]
        measure: u32,
    },
    /// Set an input cell.
    ///
    /// The value is parsed by the measure's type: a number, true or false, or
    /// verbatim text. --at maps category names to item names; omit it for a
    /// scalar cell. Pairs may be comma-joined (--at Time=Q1,Product=A),
    /// space-separated (--at Time=Q1 Product=A) or in repeated flags; they are
    /// merged, and a category may appear only once.
    Set {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        #[arg(value_name = "measure-id", help = MEASURE_ID,
              value_parser = number::<u32>("<measure-id>"))]
        measure: u32,
        /// The cell's value
        #[arg(value_name = "value", allow_hyphen_values = true)]
        value: String,
        #[arg(long, value_name = "Cat=Item", num_args = 1.., help = AT)]
        at: Vec<String>,
        /// A coordinate given without --at (refused, with a hint)
        #[arg(hide = true, allow_hyphen_values = true)]
        stray: Vec<String>,
    },
    /// Print the categories, items, measures, views and input cells.
    List {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
    },
    /// Save a named view of a measure, as the GUI does.
    ///
    /// --rows and --cols stack category names outer to inner; each defaults to
    /// the next unused category of the measure. The other categories are pages,
    /// pinned to their first item. A view with the same id is replaced.
    SaveView {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The view's numeric id
        #[arg(value_name = "view-id", value_parser = number::<u32>("<view-id>"))]
        id: u32,
        /// The view's name
        #[arg(value_name = "name", allow_hyphen_values = true)]
        name: String,
        /// The measure, by id or name
        #[arg(allow_hyphen_values = true, value_name = "measure")]
        measure: String,
        /// The row categories, comma-separated, outer first
        #[arg(long, value_name = "Cat,...", allow_hyphen_values = true)]
        rows: Option<String>,
        /// The column categories, comma-separated, outer first
        #[arg(long, value_name = "Cat,...", allow_hyphen_values = true)]
        cols: Option<String>,
    },
    /// Print one measure and its input cells.
    Show {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        #[arg(value_name = "measure-id", help = MEASURE_ID,
              value_parser = number::<u32>("<measure-id>"))]
        measure: u32,
    },
    /// Compute a derived measure and print its cells.
    ///
    /// With --scenario, under that what-if scenario's overrides.
    Eval {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        #[arg(value_name = "measure-id", help = MEASURE_ID,
              value_parser = number::<u32>("<measure-id>"))]
        measure: u32,
        /// The scenario to evaluate under
        #[arg(long, value_name = "name", allow_hyphen_values = true)]
        scenario: Option<String>,
    },
    /// Add an input-cell override to a what-if scenario.
    ///
    /// The scenario is created if it is new. The base model is untouched;
    /// evaluate under it with eval ... --scenario NAME. --at takes the same
    /// forms as for set.
    Scenario {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The scenario's name
        #[arg(allow_hyphen_values = true, value_name = "scenario-name")]
        scenario: String,
        #[arg(value_name = "measure-id", help = MEASURE_ID,
              value_parser = number::<u32>("<measure-id>"))]
        measure: u32,
        /// The overriding value
        #[arg(value_name = "value", allow_hyphen_values = true)]
        value: String,
        #[arg(long, value_name = "Cat=Item", num_args = 1.., help = AT)]
        at: Vec<String>,
        /// A coordinate given without --at (refused, with a hint)
        #[arg(hide = true, allow_hyphen_values = true)]
        stray: Vec<String>,
    },
    /// Print the whole model as JSON.
    Export {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
    },
    /// Import a SQLite query into a new SQL-backed input measure.
    ///
    /// Each dimension spec maps a result column to a category (its distinct
    /// values become items); the value column holds the numbers. Re-run the
    /// query with refresh-sql. Example: improv import-sql m.db sales.db 100
    /// Revenue "SELECT t,p,r FROM sales" r t:1:Time p:2:Product
    ImportSql(ImportSql),
    /// Re-run an SQL-backed measure's query and replace its cells.
    ///
    /// New dimension values become new items.
    RefreshSql {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The SQLite database to query
        #[arg(allow_hyphen_values = true, value_name = "source.sqlite")]
        source: String,
        #[arg(value_name = "measure-id", help = MEASURE_ID,
              value_parser = number::<u32>("<measure-id>"))]
        measure: u32,
    },
    /// Refresh every external measure now.
    ///
    /// Every CALL(...) measure, and every SQL-backed measure when a source is
    /// given. Each measure's refresh policy is reported, not applied.
    RefreshAll {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The SQLite database the SQL-backed measures query
        #[arg(allow_hyphen_values = true, value_name = "source.sqlite")]
        source: Option<String>,
    },
    /// Refresh measures as their refresh policies fall due, until interrupted.
    ///
    /// On every tick: on-load measures once, interval:N measures every N
    /// seconds. SQL-backed measures refresh only when a source is given.
    ServeRefresh {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The SQLite database the SQL-backed measures query
        #[arg(allow_hyphen_values = true, value_name = "source.sqlite")]
        source: Option<String>,
        /// Seconds between ticks
        #[arg(long, value_name = "SECS", default_value = "5",
              value_parser = number::<u64>("--tick SECS"))]
        tick: u64,
    },
    /// Write a measure's input cells to a SQLite table.
    ///
    /// One column per dimension category, then the value column; the table is
    /// created if absent. Identifiers are validated and values bound as
    /// parameters.
    ExportSql {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The SQLite database to write
        #[arg(allow_hyphen_values = true, value_name = "target.sqlite")]
        target: String,
        #[arg(value_name = "measure-id", help = MEASURE_ID,
              value_parser = number::<u32>("<measure-id>"))]
        measure: u32,
        /// The table
        #[arg(allow_hyphen_values = true, value_name = "table")]
        table: String,
        /// The value column's name
        #[arg(allow_hyphen_values = true, value_name = "value-col")]
        value_col: String,
    },
    /// Import a CSV or TSV file into a new input measure.
    ///
    /// Columns are header names, or 0-based indices with --no-header. The
    /// delimiter is a comma unless --tsv is given or the file ends in .tsv.
    /// Example: improv import-csv m.db sales.csv 100 Revenue r t:1:Time
    /// p:2:Product
    ImportCsv(ImportCsv),
    /// Write a measure's cells to a CSV or TSV file.
    ///
    /// An input measure's stored cells or a derived measure's computed ones:
    /// one column per dimension category, then the value; header row first.
    /// Text a spreadsheet would run as a formula is prefixed with a quote.
    ExportCsv {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The file to write (tab-separated if it ends in .tsv)
        #[arg(allow_hyphen_values = true, value_name = "target.csv")]
        target: String,
        #[arg(value_name = "measure-id", help = MEASURE_ID,
              value_parser = number::<u32>("<measure-id>"))]
        measure: u32,
        /// Tab-separated
        #[arg(long)]
        tsv: bool,
    },
    /// Apply input edits from stdin and print the cells they change.
    ///
    /// One edit per line, MEASURE VALUE [Cat=Item,...] (the measure by id or
    /// name; no coordinate for a scalar measure). Each applies to a live
    /// incremental engine, and every changed cell of a target measure prints
    /// as MEASURE Cat=Item,... = VALUE. A malformed line is reported on stderr
    /// and skipped. At end of input the model is saved. Example: printf 'Price
    /// 12 Product=WidgetA\n' | improv stream m.db Revenue
    Stream {
        #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
        db: String,
        /// The measures whose changes to print, by id or name
        #[arg(
            allow_hyphen_values = true,
            value_name = "target-measure",
            required = true
        )]
        targets: Vec<String>,
    },
}

/// `import-sql`'s arguments.
#[derive(Args, Debug)]
pub struct ImportSql {
    #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
    pub db: String,
    /// The SQLite database to query
    #[arg(allow_hyphen_values = true, value_name = "source.sqlite")]
    pub source: String,
    #[arg(value_name = "measure-id", help = "The new measure's numeric id",
          value_parser = number::<u32>("<measure-id>"))]
    pub measure: u32,
    /// The new measure's name
    #[arg(value_name = "measure-name", allow_hyphen_values = true)]
    pub name: String,
    /// The query
    #[arg(value_name = "SELECT", allow_hyphen_values = true)]
    pub query: String,
    /// The result column holding the values
    #[arg(allow_hyphen_values = true, value_name = "value-col")]
    pub value_col: String,
    #[arg(value_name = "dim-col:cat-id:cat-name", required = true,
          value_parser = dim_spec, help = DIMS)]
    pub dims: Vec<(String, CategoryId, String)>,
    /// The measure's refresh policy
    #[arg(long, value_name = "policy", value_parser = refresh_policy,
          default_value = "manual", long_help = REFRESH)]
    pub refresh: RefreshPolicy,
}

/// `import-csv`'s arguments.
#[derive(Args, Debug)]
pub struct ImportCsv {
    #[arg(allow_hyphen_values = true, value_name = "db", help = DB)]
    pub db: String,
    /// The file to read
    #[arg(allow_hyphen_values = true, value_name = "file.csv")]
    pub file: String,
    #[arg(value_name = "measure-id", help = "The new measure's numeric id",
          value_parser = number::<u32>("<measure-id>"))]
    pub measure: u32,
    /// The new measure's name
    #[arg(value_name = "measure-name", allow_hyphen_values = true)]
    pub name: String,
    /// The column holding the values
    #[arg(allow_hyphen_values = true, value_name = "value-col")]
    pub value_col: String,
    #[arg(value_name = "dim-col:cat-id:cat-name", required = true,
          value_parser = dim_spec, help = DIMS)]
    pub dims: Vec<(String, CategoryId, String)>,
    /// Tab-separated
    #[arg(long)]
    pub tsv: bool,
    /// The file has no header row: columns are 0-based indices
    #[arg(long)]
    pub no_header: bool,
}
