//! The `improv` command line.
//!
//! A thin front-end over `improv_store::Store` (a Mentat file or a
//! `postgres://…?model=` Postgres model). Each subcommand
//! opens the store, mutates the loaded `Model`, and saves it back.
//!
//! Coordinates (`--at`) resolve category *names* and item *names* to ids via
//! the loaded model, e.g. `--at Time=2025,Product=WidgetA`. The arguments
//! are declared in [`args`], which `--help` and the man pages come from.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

use improv_core_model::{
    parser, parser::Definition, CategoryId, Coordinate, ExternalCall, ItemId, MatrixPlacement,
    Measure, MeasureId, MeasureKind, Model, Name, RefreshPolicy, Scenario, ScenarioId, Value,
    ValueType, View, ViewId,
};
use improv_storage_sql::{
    add_sql_measure, export_measure, refresh_sql_measure, DimensionMapping, ImportSpec,
};
use improv_store::Store;
use std::collections::HashMap;

// Declarations only (the clap derives); the value parsers below are tested.
#[cfg_attr(coverage_nightly, coverage(off))]
mod args;

use args::{Cli, Cmd};
use clap::{CommandFactory, Parser};

fn main() {
    let cli = Cli::try_parse().unwrap_or_else(|e| exit_usage(e));
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

/// Exit for a parse error: `--help`/`--version` print and exit 0; a usage
/// error prints the reason, in the words the hand parser used, and exits 2.
fn exit_usage(e: clap::Error) -> ! {
    use clap::error::ErrorKind;
    match e.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => e.exit(),
        _ => {
            eprintln!("error: {}", usage_message(&e));
            if let Some(clap::error::ContextValue::StyledStr(u)) =
                e.get(clap::error::ContextKind::Usage)
            {
                eprintln!("\n{u}");
            }
            eprintln!("\nFor more information, try '--help'.");
            std::process::exit(2);
        }
    }
}

/// A clap error as one line, in the hand parser's words where tests and
/// docs quote them.
fn usage_message(e: &clap::Error) -> String {
    use clap::error::{ContextKind, ContextValue, ErrorKind};
    let ctx = |k| match e.get(k) {
        Some(ContextValue::String(s)) => vec![s.clone()],
        Some(ContextValue::Strings(v)) => v.clone(),
        _ => Vec::new(),
    };
    let arg = ctx(ContextKind::InvalidArg).join(" ");
    let first_line = || {
        let text = e.render().to_string();
        let line = text.lines().next().unwrap_or_default();
        line.trim_start_matches("error: ").to_string()
    };
    match e.kind() {
        ErrorKind::InvalidSubcommand => format!(
            "unknown command '{}'",
            ctx(ContextKind::InvalidSubcommand).join(" ")
        ),
        ErrorKind::MissingRequiredArgument => ctx(ContextKind::InvalidArg)
            .iter()
            .map(|a| match a.trim_end_matches("...") {
                "<dim-col:cat-id:cat-name>" => "at least one dimension mapping is required".into(),
                "<target-measure>" => "stream needs at least one target measure".into(),
                a => format!("missing argument {a}"),
            })
            .collect::<Vec<_>>()
            .join("; "),
        // A flag without its value: `--tick` rather than `--tick 5`.
        ErrorKind::InvalidValue if ctx(ContextKind::InvalidValue).concat().is_empty() => {
            match arg.split(' ').next().unwrap_or_default() {
                "--at" => "--at needs an argument, e.g. Time=2025".into(),
                "--scenario" => "--scenario needs a name".into(),
                "--tick" => "--tick needs a value in seconds".into(),
                flag => format!("{flag} needs a value"),
            }
        }
        // Our value parsers' own message, as the hand parser printed it.
        ErrorKind::ValueValidation => {
            std::error::Error::source(e).map_or_else(first_line, |why| why.to_string())
        }
        ErrorKind::UnknownArgument => format!("unexpected argument '{arg}'"),
        _ => first_line(),
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let Some(cmd) = cli.command else {
        // No command: print the help, as `improv help` does.
        Cli::command()
            .print_long_help()
            .map_err(|e| e.to_string())?;
        return Ok(());
    };
    match cmd {
        Cmd::Init { db } => cmd_init(&db),
        Cmd::AddCategory { db, id, name } => cmd_add_category(&db, id, &name),
        Cmd::AddItem {
            db,
            id,
            category,
            name,
        } => cmd_add_item(&db, id, category, &name),
        Cmd::AddMeasure {
            db,
            id,
            name,
            value_type,
            kind,
            categories,
        } => cmd_add_measure(&db, id, &name, value_type, &kind, &categories),
        Cmd::AddDerived {
            db,
            id,
            name,
            formula,
        } => cmd_add_derived(&db, id, &name, &formula),
        Cmd::Scenario {
            db,
            scenario,
            measure,
            value,
            at,
            stray,
        } => cmd_scenario(&db, &scenario, measure, &value, &coordinate(&at, &stray)?),
        Cmd::Define {
            db,
            id,
            definition,
            refresh,
        } => cmd_define(&db, id, &definition, refresh),
        Cmd::RegisterExt {
            db,
            name,
            arity,
            body,
        } => cmd_register_ext(&db, &name, arity, &body),
        Cmd::RefreshExt { db, measure } => cmd_refresh_ext(&db, measure),
        Cmd::Set {
            db,
            measure,
            value,
            at,
            stray,
        } => cmd_set(&db, measure, &value, &coordinate(&at, &stray)?),
        Cmd::List { db } => cmd_list(&db),
        Cmd::SaveView {
            db,
            id,
            name,
            measure,
            rows,
            cols,
        } => cmd_save_view(&db, id, &name, &measure, rows.as_deref(), cols.as_deref()),
        Cmd::Show { db, measure } => cmd_show(&db, measure),
        Cmd::Eval {
            db,
            measure,
            scenario,
        } => cmd_eval(&db, measure, scenario.as_deref()),
        Cmd::Export { db } => cmd_export(&db),
        Cmd::ImportSql(a) => cmd_import_sql(a),
        Cmd::RefreshSql {
            db,
            source,
            measure,
        } => cmd_refresh_sql(&db, &source, measure),
        Cmd::RefreshAll { db, source } => cmd_refresh_all(&db, source.as_deref()),
        Cmd::ServeRefresh { db, source, tick } => cmd_serve_refresh(&db, source.as_deref(), tick),
        Cmd::ExportSql {
            db,
            target,
            measure,
            table,
            value_col,
        } => cmd_export_sql(&db, &target, measure, &table, &value_col),
        Cmd::ImportCsv(a) => cmd_import_csv(a),
        Cmd::ExportCsv {
            db,
            target,
            measure,
            tsv,
        } => cmd_export_csv(&db, &target, measure, tsv),
        Cmd::Stream { db, targets } => cmd_stream(&db, &targets),
    }
}

/// The `--at` pairs of `set`/`scenario`, as the hand parser saw them: a
/// token clap left over (`stray`) is the `--at` the user forgot.
fn coordinate(at: &[String], stray: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut tokens: Vec<String> = stray.to_vec();
    if !at.is_empty() {
        tokens.push("--at".into());
        tokens.extend(at.iter().cloned());
    }
    parse_at_flag(&tokens)
}

// --- argument value parsers ---

/// A value parser for a number that errors as "NAME must be a number, got 'x'".
pub(crate) fn number<T: std::str::FromStr>(
    name: &'static str,
) -> impl Fn(&str) -> Result<T, String> + Clone + Send + Sync + 'static {
    move |s| {
        s.parse()
            .map_err(|_| format!("{name} must be a number, got '{s}'"))
    }
}

/// `number`, `boolean` or `text`.
pub fn value_type(s: &str) -> Result<ValueType, String> {
    match s {
        "number" => Ok(ValueType::Number),
        "boolean" => Ok(ValueType::Boolean),
        "text" => Ok(ValueType::Text),
        other => Err(format!("type must be number|boolean|text, got '{other}'")),
    }
}

/// `manual`, `on-load` or `interval:SECS`.
pub fn refresh_policy(s: &str) -> Result<RefreshPolicy, String> {
    match s {
        "manual" => Ok(RefreshPolicy::Manual),
        "on-load" => Ok(RefreshPolicy::OnLoad),
        other => match other.strip_prefix("interval:") {
            Some(secs) => secs
                .parse::<u64>()
                .map(|secs| RefreshPolicy::Interval { secs })
                .map_err(|_| "interval:SECS must be a number".to_string()),
            None => Err(format!(
                "unknown refresh policy '{other}': use manual | on-load | interval:SECS"
            )),
        },
    }
}

/// `col:cat-id:cat-name`, as import-sql and import-csv map a column.
pub fn dim_spec(spec: &str) -> Result<(String, CategoryId, String), String> {
    let parts: Vec<&str> = spec.splitn(3, ':').collect();
    let [col, id, name] = parts[..] else {
        return Err(format!(
            "dimension spec '{spec}' must be <col>:<cat-id>:<cat-name>"
        ));
    };
    let id = number::<u32>("<cat-id>")(id)?;
    Ok((col.to_string(), CategoryId(id), name.to_string()))
}

// --- helpers ---

fn open(db: &str) -> Result<Store, String> {
    Store::open(db).map_err(|e| e.to_string())
}

// --- commands ---

fn cmd_init(db: &str) -> Result<(), String> {
    // Opening installs the schema; that is all `init` does. It must not save
    // an empty model: `save_model` retracts stored cells the model lacks, so
    // re-running `init` on a populated store would delete every cell.
    open(db)?;
    println!("initialized store at '{db}'");
    Ok(())
}

fn cmd_add_category(db: &str, id: u32, name: &str) -> Result<(), String> {
    let id = CategoryId(id);
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    model.add_category(id, name);
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("added category {} '{name}'", id.0);
    Ok(())
}

fn cmd_add_item(db: &str, id: u32, cat: u32, name: &str) -> Result<(), String> {
    let (id, cat) = (ItemId(id), CategoryId(cat));
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    if !model.categories.contains_key(&cat) {
        return Err(format!("no category with id {}", cat.0));
    }
    model.add_item(id, cat, name);
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("added item {} '{name}' in category {}", id.0, cat.0);
    Ok(())
}

fn cmd_add_measure(
    db: &str,
    id: u32,
    name: &str,
    value_type: ValueType,
    kind: &str,
    category_names: &[String],
) -> Result<(), String> {
    let id = MeasureId(id);
    if kind != "input" {
        return Err(format!("only 'input' measures are supported, got '{kind}'"));
    }
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    // Optional trailing args name the categories this measure ranges over,
    // resolved by name against the model, e.g. `... input Time Product`.
    let mut categories = Vec::new();
    for cat_name in category_names {
        let c = model
            .category_by_name(cat_name)
            .ok_or_else(|| format!("unknown category '{cat_name}'"))?;
        categories.push(c.id);
    }

    model.add_measure(Measure {
        id,
        name: Name(name.to_string()),
        value_type,
        categories,
        kind: MeasureKind::Input,
        description: None,
    });
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("added input measure {} '{name}'", id.0);
    Ok(())
}

fn cmd_add_derived(db: &str, id: u32, name: &str, formula_text: &str) -> Result<(), String> {
    let id = MeasureId(id);

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    // Parse the RHS expression, resolving measure/category names against the model.
    let formula_text = improv_core_model::Locale::from_env().canonicalize(formula_text);
    let formula = parser::parse_expr(&model.with_provisional(id, name), &formula_text)
        .map_err(|e| e.to_string())?;

    // The measure's type and categories, as the engine computes them (an
    // aggregation drops the categories it reduces).
    let (value_type, cats) = improv_engine::compiler::derived_shape(&model, id, name, &formula)
        .map_err(|e| e.to_string())?;
    model.add_measure(Measure {
        id,
        name: Name(name.to_string()),
        value_type,
        categories: cats,
        kind: MeasureKind::Derived(formula),
        description: None,
    });
    check_acyclic(&model)?;
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("added derived measure {} '{name}'", id.0);
    Ok(())
}

/// `define <db> <id> "<Target = ...>"` — create a measure from a definition
/// string. Supports an ordinary formula (`Target = Price * Quantity`) and the
/// external-call source form (`Target = CALL(func, ArgMeasure, ...)`). The SQL
/// source form is handled by `import-sql` (it needs a column→dimension mapping
/// beyond the query string). The target name is taken from the definition's LHS.
fn cmd_define(db: &str, id: u32, text: &str, refresh_policy: RefreshPolicy) -> Result<(), String> {
    let id = MeasureId(id);

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    let text = improv_core_model::Locale::from_env().canonicalize(text);
    // A new measure may refer to itself (a PREV recurrence): parse with its
    // name provisionally known.
    let target = parser::parse_formula(&model, &text)
        .map(|f| f.target.0)
        .or_else(|_| {
            text.split('=')
                .next()
                .map(|t| t.trim().to_string())
                .ok_or(())
        })
        .unwrap_or_default();
    let parse_model = model.with_provisional(id, &target);
    match parser::parse_definition(&parse_model, &text).map_err(|e| e.to_string())? {
        Definition::Formula(ft) => {
            let (value_type, cats) =
                improv_engine::compiler::derived_shape(&model, id, &ft.target.0, &ft.formula)
                    .map_err(|e| e.to_string())?;
            model.add_measure(Measure {
                id,
                name: ft.target.clone(),
                value_type,
                categories: cats,
                kind: MeasureKind::Derived(ft.formula),
                description: None,
            });
            println!("defined derived measure {} '{}'", id.0, ft.target.0);
        }
        Definition::Call { target, func, args } => {
            if !model.external_fns.contains_key(&func) {
                return Err(format!("external function '{func}' is not registered"));
            }
            // Resolve argument measure names to ids; the call's categories are
            // the union of its argument measures' categories.
            let mut arg_ids = Vec::new();
            let mut cats: Vec<CategoryId> = Vec::new();
            for a in &args {
                let m = model
                    .measures
                    .values()
                    .find(|m| m.name == *a)
                    .ok_or_else(|| format!("unknown argument measure '{}'", a.0))?;
                arg_ids.push(m.id);
                for c in &m.categories {
                    if !cats.contains(c) {
                        cats.push(*c);
                    }
                }
            }
            cats.sort_by_key(|c| c.0);
            // The measure is an Input measure populated by a host-side refresh
            // (`engine::external::refresh_external_measure`).
            model.add_measure(Measure {
                id,
                name: target.clone(),
                value_type: ValueType::Number,
                categories: cats,
                kind: MeasureKind::Input,
                description: None,
            });
            model.external_calls.insert(
                id,
                ExternalCall {
                    func: func.clone(),
                    arg_measures: arg_ids,
                    refresh_policy,
                },
            );
            println!(
                "defined external-call measure {} '{}' = {func}(...); run 'refresh-ext' to populate",
                id.0, target.0
            );
        }
        Definition::Sql { .. } => {
            return Err(
                "SQL(...) form: use 'import-sql' (it needs a column-to-dimension mapping)"
                    .to_string(),
            );
        }
        Definition::Lambda(lt) => {
            // The id names the lambda for good (calls store it); redefining
            // a name keeps its id.
            let id = model
                .lambda_by_name(&lt.name.0)
                .map_or(id.0, |(old, _)| old);
            if model.measure_by_name(&lt.name.0).is_some() {
                return Err(format!(
                    "'{}' is a measure; name the LAMBDA differently",
                    lt.name.0
                ));
            }
            if model.lambdas.get(&id).is_some_and(|l| l.name != lt.name) {
                return Err(format!("LAMBDA id {id} is taken"));
            }
            let params = lt.lambda.params.join(", ");
            model.lambdas.insert(id, lt.lambda);
            println!("defined LAMBDA {id} '{}'({params})", lt.name.0);
        }
    }
    check_acyclic(&model)?;
    store.save_model(&model).map_err(|e| e.to_string())?;
    Ok(())
}

/// Refuse to save formulas that depend on each other in a loop: the engine
/// could not build the model, so every later `eval` would fail.
fn check_acyclic(model: &Model) -> Result<(), String> {
    let derived: Vec<MeasureId> = model
        .measures
        .values()
        .filter(|m| m.is_derived())
        .map(|m| m.id)
        .collect();
    improv_engine::dataflow::derived_build_order(model, &derived)
        .map(drop)
        .map_err(|e| e.to_string())
}

/// `register-ext <db> <name> <arity> "<python body>"` — register a pure Python
/// external function (all Number args + Number result; the body binds `args`
/// and sets `result`). Enough to make `define ... CALL(name, ...)` usable from
/// the CLI; richer type signatures are a future flag.
fn cmd_register_ext(db: &str, name: &str, arity: u32, body: &str) -> Result<(), String> {
    use improv_core_model::{ExternalFn, Language};
    let arity = arity as usize;

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    model.external_fns.insert(
        name.to_string(),
        ExternalFn {
            name: name.to_string(),
            language: Language::Python,
            body: body.to_string(),
            arg_types: vec![ValueType::Number; arity],
            return_type: ValueType::Number,
            pure: true,
        },
    );
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("registered external function '{name}' (arity {arity})");
    Ok(())
}

/// `refresh-all <db> [source.sqlite]` — refresh every external-sourced measure:
/// all external-function (`CALL`) measures, plus all SQL-backed measures when a
/// SQLite source is given. This honors nothing about `RefreshPolicy` timing
/// itself — it is the manual "do it now" batch; a scheduler that consults the
/// policy is future work. Policy metadata is reported so it is visible.
fn cmd_refresh_all(db: &str, source: Option<&str>) -> Result<(), String> {
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    let mut total = 0usize;
    // External-function measures need no external connection.
    // In id order, so the report is the same on every run.
    let mut ext_ids: Vec<MeasureId> = model.external_calls.keys().copied().collect();
    ext_ids.sort_by_key(|m| m.0);
    for mid in ext_ids {
        let n = improv_engine::external::refresh_external_measure(
            &mut model,
            improv_engine::external::DEFAULT_TIMEOUT,
            mid,
        )
        .map_err(|e| e.to_string())?;
        println!("refreshed external measure {} ({n} cells)", mid.0);
        total += n;
    }
    // SQL measures need the source database.
    let mut sql_ids: Vec<MeasureId> = model.sql_sources.keys().copied().collect();
    sql_ids.sort_by_key(|m| m.0);
    if !sql_ids.is_empty() {
        match source {
            Some(src) => {
                let conn = rusqlite::Connection::open(src).map_err(|e| e.to_string())?;
                for mid in sql_ids {
                    let policy = model.sql_sources.get(&mid).map(|s| s.refresh_policy);
                    let n =
                        refresh_sql_measure(&conn, &mut model, mid).map_err(|e| e.to_string())?;
                    println!("refreshed SQL measure {} ({n} cells) [{policy:?}]", mid.0);
                    total += n;
                }
            }
            None => {
                eprintln!(
                    "note: {} SQL-backed measure(s) skipped (pass a source.sqlite to refresh them)",
                    sql_ids.len()
                );
            }
        }
    }
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("refresh-all: {total} cells refreshed");
    Ok(())
}

/// `serve-refresh <db> [source.sqlite] [--tick SECS]` — a long-running daemon
/// that honors each measure's `RefreshPolicy`: on each tick it refreshes the
/// measures that are *due* (OnLoad once, Interval every N seconds), reloading
/// and saving the model each cycle. SQL measures are refreshed only if a
/// source.sqlite is given; external (CALL) measures always can be. Ctrl-C to
/// stop. The scheduling decision is the pure `core_model::schedule::due_measures`.
fn cmd_serve_refresh(db: &str, source: Option<&str>, tick_secs: u64) -> Result<(), String> {
    use std::time::{Duration, Instant};

    let start = Instant::now();
    // last_run per measure, in whole seconds since `start`.
    let mut last_run: HashMap<MeasureId, u64> = HashMap::new();

    eprintln!("serve-refresh: honoring refresh policies every {tick_secs}s (Ctrl-C to stop)");
    loop {
        serve_refresh_tick(db, source, start.elapsed().as_secs(), &mut last_run)?;
        std::thread::sleep(Duration::from_secs(tick_secs));
    }
}

/// One `serve-refresh` cycle at `now_secs`: reload the model, refresh every
/// measure whose policy is due (recording it in `last_run`), and save. SQL
/// measures are skipped when no `source` is given.
fn serve_refresh_tick(
    db: &str,
    source: Option<&str>,
    now_secs: u64,
    last_run: &mut HashMap<MeasureId, u64>,
) -> Result<(), String> {
    use improv_core_model::schedule;

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    // Gather policies from both source kinds.
    let mut policies: HashMap<MeasureId, RefreshPolicy> = HashMap::new();
    for (id, s) in &model.sql_sources {
        policies.insert(*id, s.refresh_policy);
    }
    for (id, c) in &model.external_calls {
        policies.insert(*id, c.refresh_policy);
    }

    let due = schedule::due_measures(&policies, now_secs, last_run);
    if !due.is_empty() {
        let conn = match source {
            Some(src) => Some(rusqlite::Connection::open(src).map_err(|e| e.to_string())?),
            None => None,
        };
        for mid in due {
            let refreshed = if model.external_calls.contains_key(&mid) {
                improv_engine::external::refresh_external_measure(
                    &mut model,
                    improv_engine::external::DEFAULT_TIMEOUT,
                    mid,
                )
                .map(Some)
                .map_err(|e| e.to_string())?
            } else if let Some(c) = &conn {
                refresh_sql_measure(c, &mut model, mid)
                    .map(Some)
                    .map_err(|e| e.to_string())?
            } else {
                None // SQL measure but no source given; skip quietly
            };
            if let Some(n) = refreshed {
                last_run.insert(mid, now_secs);
                eprintln!("[{now_secs}s] refreshed measure {} ({n} cells)", mid.0);
            }
        }
        store.save_model(&model).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `refresh-ext <db> <measure-id>` — (re)populate an external-call measure by
/// running its function over its argument measures, host-side.
fn cmd_refresh_ext(db: &str, mid: u32) -> Result<(), String> {
    let mid = MeasureId(mid);

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    let n = improv_engine::external::refresh_external_measure(
        &mut model,
        improv_engine::external::DEFAULT_TIMEOUT,
        mid,
    )
    .map_err(|e| e.to_string())?;
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("refreshed external measure {} ({n} cells)", mid.0);
    Ok(())
}

/// `scenario <db> <scenario-name> <measure-id> <value> [--at Cat=Item,...]`
/// — create or update a what-if scenario by adding one input-cell override.
/// Evaluate under it with `eval <db> <measure-id> --scenario <name>`.
fn cmd_scenario(
    db: &str,
    sname: &str,
    mid: u32,
    value_arg: &str,
    at: &[(String, String)],
) -> Result<(), String> {
    let (sname, mid) = (sname.to_string(), MeasureId(mid));

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    let vt = model
        .measures
        .get(&mid)
        .ok_or_else(|| format!("no measure with id {}", mid.0))?
        .value_type;
    let value = parse_value(value_arg, vt)?;
    let coord = resolve_coord(&model, mid, at)?;

    // Find an existing scenario by name, else mint a new id.
    let existing = model
        .scenarios
        .values()
        .find(|s| s.name.0 == sname)
        .map(|s| s.id);
    let sid = existing.unwrap_or_else(|| {
        ScenarioId(
            model
                .scenarios
                .keys()
                .map(|s| s.0)
                .max()
                .map(|m| m + 1)
                .unwrap_or(1),
        )
    });
    let scenario = model.scenarios.entry(sid).or_insert_with(|| Scenario {
        id: sid,
        name: Name(sname.clone()),
        overrides: std::collections::HashMap::new(),
    });
    scenario.overrides.insert((mid, coord), value);
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("scenario '{sname}': override on measure {} recorded", mid.0);
    Ok(())
}

fn cmd_set(db: &str, mid: u32, value_arg: &str, at: &[(String, String)]) -> Result<(), String> {
    let mid = MeasureId(mid);

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    let vt = model
        .measures
        .get(&mid)
        .ok_or_else(|| format!("no measure with id {}", mid.0))?
        .value_type;
    let value = parse_value(value_arg, vt)?;
    let coord = resolve_coord(&model, mid, at)?;

    model.set_input(mid, coord, value);
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("set measure {}", mid.0);
    Ok(())
}

fn cmd_list(db: &str) -> Result<(), String> {
    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;

    println!("categories:");
    for c in sorted_by(model.categories.values(), |c| c.id.0) {
        println!("  {} {}", c.id.0, c.name);
    }
    println!("items:");
    for it in sorted_by(model.items.values(), |i| i.id.0) {
        println!("  {} {} (category {})", it.id.0, it.name, it.category.0);
    }
    println!("measures:");
    for m in sorted_by(model.measures.values(), |m| m.id.0) {
        let kind = if m.is_input() { "input" } else { "derived" };
        println!("  {} {} [{}] {:?}", m.id.0, m.name, kind, m.value_type);
    }
    println!("views:");
    for v in sorted_by(model.views.values(), |v| v.id.0) {
        let measure = model
            .measures
            .get(&v.measure)
            .map_or("?", |m| m.name.0.as_str());
        let n = v.matrices().len();
        println!("  {} {} (measure {measure}, {n} matrices)", v.id.0, v.name);
    }
    println!("inputs:");
    for line in input_lines(&model) {
        println!("  {line}");
    }
    Ok(())
}

/// `save-view <db> <view-id> <name> <measure> [--rows ..] [--cols ..]`: write
/// a single-matrix `View` filled the way the GUI's `build_view` fills it:
/// `axis_order` = rows ++ cols ++ the measure's remaining categories (pages),
/// each page pinned to its lowest-id item, no filters, default geometry.
fn cmd_save_view(
    db: &str,
    id: u32,
    name: &str,
    measure: &str,
    rows: Option<&str>,
    cols: Option<&str>,
) -> Result<(), String> {
    let (id, name) = (ViewId(id), name.trim());
    if name.is_empty() {
        return Err("view name is required".into());
    }
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    let mid = resolve_measure(&model, measure)?;
    let cats = model.measures[&mid].categories.clone();
    // Rows then cols: named categories, or the next unused one by default.
    let mut order: Vec<CategoryId> = Vec::new();
    let mut counts = [0; 2];
    for (k, spec) in [rows, cols].into_iter().enumerate() {
        let picked: Vec<CategoryId> = match spec {
            Some(s) => s
                .split(',')
                .filter(|n| !n.is_empty())
                .map(|n| {
                    model
                        .category_by_name(n)
                        .map(|c| c.id)
                        .filter(|c| cats.contains(c))
                        .ok_or_else(|| format!("'{n}' is not a category of measure {}", mid.0))
                })
                .collect::<Result<_, _>>()?,
            None => cats
                .iter()
                .find(|c| !order.contains(c))
                .into_iter()
                .copied()
                .collect(),
        };
        for c in picked {
            if order.contains(&c) {
                return Err(format!("category {} is used twice", c.0));
            }
            order.push(c);
            counts[k] += 1;
        }
    }
    let pages: Vec<CategoryId> = cats
        .iter()
        .filter(|c| !order.contains(c))
        .copied()
        .collect();
    let mut primary = MatrixPlacement::new(mid);
    primary.page_items = pages
        .iter()
        .filter_map(|c| {
            Some((
                *c,
                *model.categories.get(c)?.items.iter().min_by_key(|i| i.0)?,
            ))
        })
        .collect();
    order.extend(pages);
    primary.axis_order = order;
    [primary.n_rows, primary.n_cols] = counts;
    model.add_view(View::from_matrices(
        id,
        Name(name.to_string()),
        primary,
        Vec::new(),
    ));
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("saved view {} '{name}'", id.0);
    Ok(())
}

fn cmd_show(db: &str, mid: u32) -> Result<(), String> {
    let mid = MeasureId(mid);
    let mut store = open(db)?;
    let model = store.load_partial(&[mid]).map_err(|e| e.to_string())?;
    let m = model
        .measures
        .get(&mid)
        .ok_or_else(|| format!("no measure with id {}", mid.0))?;

    let kind = if m.is_input() { "input" } else { "derived" };
    println!(
        "measure {} {} [{}] {:?}",
        m.id.0, m.name, kind, m.value_type
    );
    if let Some(d) = &m.description {
        println!("  description: {d}");
    }
    println!("  cells:");
    for line in input_lines(&model) {
        if line.starts_with(&format!("{}::", mid.0)) {
            println!("    {line}");
        }
    }
    Ok(())
}

fn cmd_export(db: &str) -> Result<(), String> {
    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&model).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

/// Resolve a measure argument that may be a numeric id or a measure name.
fn resolve_measure(model: &Model, s: &str) -> Result<MeasureId, String> {
    if let Ok(n) = s.parse::<u32>() {
        let mid = MeasureId(n);
        if model.measures.contains_key(&mid) {
            return Ok(mid);
        }
    }
    model
        .measure_by_name(s)
        .map(|m| m.id)
        .ok_or_else(|| format!("no measure named or numbered '{s}'"))
}

/// `stream <db> <target-measure> [<target-measure> ...]`
///
/// Reads lines from stdin, one per input-cell edit:
///   `<measure> <value> [Cat=Item,Cat=Item,...]`
/// (`<measure>` is a numeric id or a measure name; the coordinate is omitted
/// for a scalar, 0-dimension measure). Each line applies to the live
/// incremental engine (a delta, not a full re-evaluation), and every CHANGED
/// cell of the target measure(s) is printed to stdout as:
///   `<measure> <Cat=Item,...> = <value>`
/// so a shell pipeline can produce edits and consume recomputed results as
/// they land: `producer | improv stream model.db Revenue | consumer`.
///
/// A malformed line is reported on stderr and skipped (does not abort the
/// stream -- a long-running pipe shouldn't die on one bad line). On EOF the
/// final model state is saved back to `<db>`, exactly like `set` persists.
fn cmd_stream(db: &str, target_names: &[String]) -> Result<(), String> {
    use std::io::BufRead;

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    let targets: Vec<MeasureId> = target_names
        .iter()
        .map(|s| resolve_measure(&model, s))
        .collect::<Result<_, _>>()?;
    for t in &targets {
        let m = &model.measures[t];
        if !m.is_derived() {
            eprintln!(
                "warning: target measure {} '{}' is an input measure, not derived -- \
                 it will never appear in a recompute snapshot",
                t.0, m.name
            );
        }
    }

    let (mut engine, mut snapshot) =
        improv_engine::session::Engine::new(&model, &targets).map_err(|e| e.to_string())?;

    let stdin = std::io::stdin();
    for (lineno, line) in stdin.lock().lines().enumerate() {
        let line = line.map_err(|e| e.to_string())?;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match apply_stream_line(&mut model, &mut engine, line) {
            Ok(new_snapshot) => {
                print_stream_diff(&model, &targets, &snapshot, &new_snapshot);
                snapshot = new_snapshot;
            }
            Err(e) => eprintln!("stream: line {}: {e}", lineno + 1),
        }
    }

    store.save_model(&model).map_err(|e| e.to_string())?;
    Ok(())
}

/// Parse one `<measure> <value> [Cat=Item,...]` stream line, apply it to both
/// `model` (so the final state can be saved) and the live `engine` (for
/// incremental recompute), and return the new snapshot.
fn apply_stream_line(
    model: &mut Model,
    engine: &mut improv_engine::session::Engine,
    line: &str,
) -> Result<improv_engine::session::Snapshot, String> {
    let mut parts = line.split_whitespace();
    let measure_tok = parts.next().ok_or("empty line")?;
    let value_tok = parts.next().ok_or("missing <value>")?;
    let coord_tok = parts.next(); // optional
    if parts.next().is_some() {
        return Err("too many fields (expected: <measure> <value> [Cat=Item,...])".into());
    }

    let mid = resolve_measure(model, measure_tok)?;
    let vt = model
        .measures
        .get(&mid)
        .ok_or_else(|| format!("no measure {}", mid.0))?
        .value_type;
    let value = parse_value(value_tok, vt)?;
    let coord = match coord_tok {
        Some(spec) => resolve_coord(model, mid, &parse_pairs(spec)?)?,
        None => Coordinate::new(),
    };
    let num = value
        .as_number()
        .ok_or("stream currently supports Number-valued input cells only")?;

    model.set_input(mid, coord.clone(), value);
    let key = improv_engine::encode_coord(&coord);
    engine.set(mid, key, num).map_err(|e| e.to_string())
}

/// Print every cell that changed (or is new) in `targets` between `before` and
/// `after`: one line each, `<measure> <coord> = <value>`. A cell present
/// before but absent after (a retraction with nothing replacing it) is not
/// printed -- `stream` reports new/changed values, not deletions.
fn print_stream_diff(
    model: &Model,
    targets: &[MeasureId],
    before: &improv_engine::session::Snapshot,
    after: &improv_engine::session::Snapshot,
) {
    let empty = std::sync::Arc::default();
    for mid in targets {
        let before_m = before.get(mid).unwrap_or(&empty);
        let after_m = after.get(mid).unwrap_or(&empty);
        let mut rows: Vec<(&improv_engine::CoordKey, &improv_engine::CellValue)> = after_m
            .iter()
            .filter(|(k, v)| before_m.get(*k) != Some(*v))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        let name = model
            .measures
            .get(mid)
            .map(|m| m.name.0.clone())
            .unwrap_or_else(|| mid.0.to_string());
        for (key, val) in rows {
            println!("{name} {} = {val}", render_coord_key(model, key));
        }
    }
}

fn cmd_eval(db: &str, mid: u32, scenario: Option<&str>) -> Result<(), String> {
    let mid = MeasureId(mid);
    let mut store = open(db)?;
    // load_partial loads only mid's dependency closure (its formula chain /
    // external-call args), not every cell of every measure in the store --
    // see .agent/steering/AGENT_OUT_OF_CORE_DESIGN.md §4.
    let base = store.load_partial(&[mid]).map_err(|e| e.to_string())?;

    // Optional `--scenario NAME`: evaluate under a what-if overlay.
    let model = match scenario {
        Some(name) => {
            let sid = base
                .scenario_by_name(name)
                .ok_or_else(|| format!("no scenario named '{name}'"))?
                .id;
            base.with_scenario(sid)
        }
        None => base,
    };

    let m = model
        .measures
        .get(&mid)
        .ok_or_else(|| format!("no measure with id {}", mid.0))?;
    if !m.is_derived() {
        return Err(format!(
            "measure {} '{}' is an input measure; use `show` to see its cells",
            mid.0, m.name
        ));
    }
    let name = m.name.to_string();

    let out = improv_engine::dataflow::evaluate(&model, &[mid]).map_err(|e| e.to_string())?;
    let cells = out
        .get(&mid)
        .ok_or_else(|| "engine produced no result for this measure".to_string())?;

    // Render each coordinate key (Vec<(cat_id, item_id)>) with readable names,
    // and each value via CellValue's Display (number / bool / text / #ERR).
    let mut rows: Vec<(String, String)> = cells
        .iter()
        .map(|(k, v)| (render_coord_key(&model, k), v.to_string()))
        .collect();
    rows.sort_by(|a, b| a.0.cmp(&b.0));

    println!("eval {} '{name}' ({} cells):", mid.0, rows.len());
    for (coord, v) in rows {
        println!("  {name}[{coord}] = {v}");
    }
    Ok(())
}

/// Render an engine coordinate key `[(cat_id, item_id), ...]` as
/// `Cat=Item, Cat=Item` using model names (falling back to ids).
fn render_coord_key(model: &Model, key: &[(u32, u32)]) -> String {
    key.iter()
        .map(|(c, i)| {
            let cat = model
                .categories
                .get(&CategoryId(*c))
                .map(|c| c.name.0.clone())
                .unwrap_or_else(|| c.to_string());
            let item = model
                .items
                .get(&ItemId(*i))
                .map(|it| it.name.0.clone())
                .unwrap_or_else(|| i.to_string());
            format!("{cat}={item}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn cmd_import_sql(a: args::ImportSql) -> Result<(), String> {
    let measure_id = MeasureId(a.measure);
    let (source, measure_name) = (a.source.as_str(), a.name);
    let (query, value_column, refresh_policy) = (a.query, a.value_col, a.refresh);
    let dimensions = a
        .dims
        .into_iter()
        .map(|(column, category_id, category_name)| DimensionMapping {
            column,
            category_id,
            category_name,
        })
        .collect();
    let db = a.db.as_str();

    let conn = rusqlite::Connection::open(source).map_err(|e| e.to_string())?;
    let spec = ImportSpec {
        query,
        dimensions,
        value_column,
        measure_id,
        measure_name: measure_name.clone(),
        item_id_base: 1_000_000, // above hand-assigned ids; avoids collisions
        refresh_policy,
    };

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    // add_sql_measure imports AND records a refreshable SqlSource on the model.
    let n = add_sql_measure(&conn, &mut model, &spec).map_err(|e| e.to_string())?;
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!(
        "imported {n} cells into SQL-backed measure {} '{measure_name}' (refresh with refresh-sql)",
        measure_id.0
    );
    Ok(())
}

fn cmd_refresh_sql(db: &str, source: &str, measure_id: u32) -> Result<(), String> {
    let measure_id = MeasureId(measure_id);

    let conn = rusqlite::Connection::open(source).map_err(|e| e.to_string())?;
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    let n = refresh_sql_measure(&conn, &mut model, measure_id).map_err(|e| e.to_string())?;
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("refreshed measure {} from SQL: {n} cells", measure_id.0);
    Ok(())
}

fn cmd_export_sql(
    db: &str,
    target: &str,
    measure_id: u32,
    table: &str,
    value_column: &str,
) -> Result<(), String> {
    let measure_id = MeasureId(measure_id);

    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;
    let conn = rusqlite::Connection::open(target).map_err(|e| e.to_string())?;
    let n = export_measure(&conn, &model, measure_id, table, value_column)
        .map_err(|e| e.to_string())?;
    println!(
        "exported {n} cells from measure {} to table '{table}'",
        measure_id.0
    );
    Ok(())
}

/// `import-csv <db> <file.csv> <measure-id> <measure-name> <value-col> \
///     <dim-col:cat-id:cat-name> [...] [--tsv] [--no-header]`
/// Mirrors import-sql's dimension-mapping syntax. `<value-col>`/`<dim-col>` are
/// header names by default (or 0-based indices with `--no-header`). Delimiter
/// is comma unless `--tsv` is given or the file ends in `.tsv`.
fn cmd_import_csv(a: args::ImportCsv) -> Result<(), String> {
    let (db, path) = (a.db.as_str(), a.file.as_str());
    let measure_id = MeasureId(a.measure);
    let (measure_name, value_col) = (a.name, a.value_col.as_str());
    let has_header = !a.no_header;
    let tsv = a.tsv || path.ends_with(".tsv");
    let delimiter = if tsv { b'\t' } else { b',' };
    let col_ref = |s: &str| -> improv_storage_csv::ColumnRef {
        if has_header {
            improv_storage_csv::ColumnRef::Name(s.to_string())
        } else {
            s.parse::<usize>()
                .map(improv_storage_csv::ColumnRef::Index)
                .unwrap_or_else(|_| improv_storage_csv::ColumnRef::Name(s.to_string()))
        }
    };
    let dimensions = a
        .dims
        .iter()
        .map(
            |(column, category_id, name)| improv_storage_csv::DimensionMapping {
                column: col_ref(column),
                category_id: *category_id,
                category_name: name.clone(),
            },
        )
        .collect();

    let spec = improv_storage_csv::ImportSpec {
        path: path.into(),
        delimiter,
        has_header,
        measure_id,
        measure_name: measure_name.clone(),
        value_type: ValueType::Number,
        value_column: col_ref(value_col),
        dimensions,
        item_id_base: 1_000_000,
        max_records: None,
    };

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    let n = improv_storage_csv::import_csv(&mut model, &spec).map_err(|e| e.to_string())?;
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!(
        "imported {n} cells into measure {} '{measure_name}'",
        measure_id.0
    );
    Ok(())
}

/// `export-csv <db> <target.csv> <measure-id> [--tsv]`
fn cmd_export_csv(db: &str, target: &str, measure_id: u32, tsv: bool) -> Result<(), String> {
    let measure_id = MeasureId(measure_id);
    let tsv = tsv || target.ends_with(".tsv");
    let delimiter = if tsv { b'\t' } else { b',' };

    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;
    let n = improv_storage_csv::export_measure_csv(
        &model,
        measure_id,
        std::path::Path::new(target),
        delimiter,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "exported {n} rows from measure {} to {target}",
        measure_id.0
    );
    Ok(())
}

// --- parsing / resolution ---

fn parse_value(s: &str, vt: ValueType) -> Result<Value, String> {
    match vt {
        ValueType::Number => s
            .parse::<f64>()
            .map(Value::Number)
            .map_err(|_| format!("value must be a number, got '{s}'")),
        ValueType::Boolean => match s {
            "true" => Ok(Value::Boolean(true)),
            "false" => Ok(Value::Boolean(false)),
            other => Err(format!("value must be true|false, got '{other}'")),
        },
        ValueType::Text => Ok(Value::Text(s.to_string())),
        other => Err(format!("cannot set a {other:?} measure from the CLI")),
    }
}

/// Collect the `--at` coordinate pairs. Absent flag => empty (scalar cell).
///
/// Accepts every spelling of the same coordinate, merged in order:
/// `--at A=x,B=y`, `--at A=x --at B=y`, and `--at A=x B=y` (each `--at` takes
/// the following `Cat=Item` tokens up to the next flag). A category named twice
/// is an error, not a silent last-wins.
///
/// Rejects any other argument rather than ignoring it. Silently discarding
/// stray tokens meant `set db 100 10 Product=A` (the `--at` forgotten) reported
/// success and wrote a cell at the EMPTY coordinate — a dimensionally invalid
/// cell on a measure declared over `Product`, created with no diagnostic.
fn parse_at_flag(rest: &[String]) -> Result<Vec<(String, String)>, String> {
    let is_pair = |t: &str| t.contains('=') && !t.starts_with("--");
    let mut out: Vec<(String, String)> = Vec::new();
    let mut it = rest.iter().peekable();
    while let Some(tok) = it.next() {
        if tok != "--at" {
            return Err(if out.is_empty() && is_pair(tok) {
                format!("unexpected argument '{tok}' — did you mean '--at {tok}'?")
            } else {
                format!("unexpected argument '{tok}' (expected --at Cat=Item,...)")
            });
        }
        let mut took = false;
        while let Some(spec) = it.next_if(|t| is_pair(t)) {
            took = true;
            for (cat, item) in parse_pairs(spec)? {
                if out.iter().any(|(c, _)| *c == cat) {
                    return Err(format!("--at names category '{cat}' more than once"));
                }
                out.push((cat, item));
            }
        }
        if !took {
            return Err("--at needs an argument, e.g. Time=2025".to_string());
        }
    }
    Ok(out)
}

fn parse_pairs(spec: &str) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let (cat, item) = part
            .split_once('=')
            .ok_or_else(|| format!("--at pair must be Cat=Item, got '{part}'"))?;
        out.push((cat.trim().to_string(), item.trim().to_string()));
    }
    Ok(out)
}

/// Resolve name pairs to a `Coordinate` using the model.
/// Resolve `Cat=Item` name pairs into a `Coordinate`, checking it names exactly
/// the dimensions `measure` is declared over.
///
/// Without that check a wrong or missing `--at` wrote a cell at a coordinate the
/// measure has no cell for — it saved, reported success, and then evaluated to
/// nothing, because the engine joins on the declared dimensions.
fn resolve_coord(
    model: &Model,
    measure: MeasureId,
    pairs: &[(String, String)],
) -> Result<Coordinate, String> {
    let mut coord = Coordinate::new();
    for (cat_name, item_name) in pairs {
        let cat = model
            .category_by_name(cat_name)
            .ok_or_else(|| format!("no category named '{cat_name}'"))?;
        let item = model
            .items
            .values()
            .find(|i| i.category == cat.id && i.name.0 == *item_name)
            .ok_or_else(|| format!("no item named '{item_name}' in category '{cat_name}'"))?;
        coord = coord.with(cat.id, item.id);
    }

    let Some(m) = model.measures.get(&measure) else {
        return Ok(coord);
    };
    // An empty `categories` means the measure never DECLARED its dimensions
    // (`add-measure ... input` with no trailing category names) — they are
    // inferred from use, so any coordinate is legitimate and there is nothing to
    // check against. Only validate when the declaration exists.
    if m.categories.is_empty() {
        return Ok(coord);
    }
    let name_of = |c: &CategoryId| {
        model
            .categories
            .get(c)
            .map(|c| c.name.0.clone())
            .unwrap_or_else(|| format!("category {}", c.0))
    };
    let given: Vec<CategoryId> = coord.categories().collect();
    let missing: Vec<String> = m
        .categories
        .iter()
        .filter(|c| !given.contains(c))
        .map(&name_of)
        .collect();
    let extra: Vec<String> = given
        .iter()
        .filter(|c| !m.categories.contains(c))
        .map(&name_of)
        .collect();
    if !missing.is_empty() || !extra.is_empty() {
        let mut why = Vec::new();
        if !missing.is_empty() {
            why.push(format!("missing {}", missing.join(", ")));
        }
        if !extra.is_empty() {
            why.push(format!("not a dimension of it: {}", extra.join(", ")));
        }
        return Err(format!(
            "measure {} '{}' is declared over [{}], but --at gave [{}] — {}",
            measure.0,
            m.name,
            m.categories
                .iter()
                .map(&name_of)
                .collect::<Vec<_>>()
                .join(", "),
            given.iter().map(&name_of).collect::<Vec<_>>().join(", "),
            why.join("; "),
        ));
    }
    Ok(coord)
}

// --- formatting ---

fn sorted_by<'a, T, K: Ord>(
    values: impl Iterator<Item = &'a T>,
    key: impl Fn(&T) -> K,
) -> Vec<&'a T> {
    let mut v: Vec<&T> = values.collect();
    v.sort_by_key(|x| key(x));
    v
}

/// One line per input cell: `MID::{coord} = value` with names where possible.
fn input_lines(model: &Model) -> Vec<String> {
    let mut lines: Vec<String> = model
        .inputs
        .iter()
        .map(|((mid, coord), val)| {
            let coord_str = fmt_coord(model, coord);
            format!("{}::{{{}}} = {}", mid.0, coord_str, fmt_value(val))
        })
        .collect();
    lines.sort();
    lines
}

fn fmt_coord(model: &Model, coord: &Coordinate) -> String {
    coord
        .dims
        .iter()
        .map(|(cat, item)| {
            let cn = model
                .categories
                .get(cat)
                .map(|c| c.name.0.clone())
                .unwrap_or_else(|| cat.0.to_string());
            let inm = model
                .items
                .get(item)
                .map(|i| i.name.0.clone())
                .unwrap_or_else(|| item.0.to_string());
            format!("{cn}={inm}")
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn fmt_value(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Text(s) => format!("{s:?}"),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// Parse `args` as the command line and run it.
    fn run_args(args: &[String]) -> Result<(), String> {
        let argv = std::iter::once("improv".to_string()).chain(args.iter().cloned());
        run(Cli::try_parse_from(argv).map_err(|e| usage_message(&e))?)
    }

    #[test]
    fn refresh_flag_parses_all_forms() {
        assert_eq!(refresh_policy("manual").unwrap(), RefreshPolicy::Manual);
        assert_eq!(refresh_policy("on-load").unwrap(), RefreshPolicy::OnLoad);
        assert_eq!(
            refresh_policy("interval:30").unwrap(),
            RefreshPolicy::Interval { secs: 30 }
        );
        assert!(refresh_policy("hourly").is_err());
        assert!(refresh_policy("interval:x").is_err());
    }

    /// Usage errors keep the hand parser's wording.
    #[test]
    fn usage_errors_read_as_before() {
        let cases: &[(&[&str], &str)] = &[
            (&["frob"], "unknown command 'frob'"),
            (&["init"], "missing argument <db>"),
            (
                &["add-measure", "m", "1", "X"],
                "missing argument <type>; missing argument <input>",
            ),
            (&["stream", "m"], "stream needs at least one target measure"),
            (
                &["import-csv", "m", "f", "1", "n", "v"],
                "at least one dimension mapping is required",
            ),
            (
                &["set", "m", "1", "2", "--at"],
                "--at needs an argument, e.g. Time=2025",
            ),
            (&["eval", "m", "1", "--scenario"], "--scenario needs a name"),
            (
                &["serve-refresh", "m", "--tick"],
                "--tick needs a value in seconds",
            ),
            (
                &["save-view", "m", "1", "v", "x", "--rows"],
                "--rows needs a value",
            ),
            (
                &["show", "m", "x"],
                "<measure-id> must be a number, got 'x'",
            ),
            (
                &["serve-refresh", "m", "--tick", "x"],
                "--tick SECS must be a number, got 'x'",
            ),
            (
                &["import-sql", "m", "s", "1", "n", "q", "v", "t:x:T"],
                "<cat-id> must be a number",
            ),
            (
                &["add-measure", "m", "1", "X", "date", "input"],
                "got 'date'",
            ),
            (&["list", "m", "extra"], "unexpected argument 'extra'"),
            (&["export-csv", "m", "t", "1", "--tsv=yes"], "--tsv"),
        ];
        for (args, want) in cases {
            let err = run_args(&s(args)).unwrap_err();
            assert!(err.contains(want), "{args:?}: want '{want}' in '{err}'");
        }
    }

    fn tmp_db() -> String {
        let p = std::env::temp_dir().join(format!(
            "improv_cli_test_{}_{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        p.to_string_lossy().into_owned()
    }

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn full_round_trip() {
        let db = tmp_db();
        let _guard = scopeguard(&db);

        run_args(&s(&["init", &db])).unwrap();
        run_args(&s(&["add-category", &db, "1", "Product"])).unwrap();
        run_args(&s(&["add-item", &db, "20", "1", "WidgetA"])).unwrap();
        run_args(&s(&["add-measure", &db, "100", "Price", "number", "input"])).unwrap();
        run_args(&s(&["set", &db, "100", "10.5", "--at", "Product=WidgetA"])).unwrap();

        let mut store = Store::open(&db).unwrap();
        let model = store.load_model().unwrap();

        assert!(model.category_by_name("Product").is_some());
        assert!(model
            .items
            .values()
            .any(|i| (i.name.0.as_str(), i.category) == ("WidgetA", CategoryId(1))));
        let m = model.measure_by_name("Price").unwrap();
        assert_eq!(m.value_type as u8, ValueType::Number as u8);
        assert!(m.is_input());

        let coord = Coordinate::from_pairs([(CategoryId(1), ItemId(20))]);
        assert_eq!(
            model.input(MeasureId(100), &coord),
            Some(&Value::Number(10.5))
        );
    }

    /// Re-running `init` on a populated store must not wipe its cells.
    #[test]
    fn init_on_existing_store_keeps_cells() {
        let db = tmp_db();
        let _guard = scopeguard(&db);
        run_args(&s(&["init", &db])).unwrap();
        run_args(&s(&["add-category", &db, "1", "Product"])).unwrap();
        run_args(&s(&["add-item", &db, "20", "1", "WidgetA"])).unwrap();
        run_args(&s(&["add-measure", &db, "100", "Price", "number", "input"])).unwrap();
        run_args(&s(&["set", &db, "100", "10.5", "--at", "Product=WidgetA"])).unwrap();
        run_args(&s(&["init", &db])).unwrap();
        let model = Store::open(&db).unwrap().load_model().unwrap();
        assert_eq!(model.inputs.len(), 1);
    }

    #[test]
    fn bad_input_does_not_panic() {
        let db = tmp_db();
        let _guard = scopeguard(&db);
        run_args(&s(&["init", &db])).unwrap();
        run_args(&s(&["add-measure", &db, "1", "M", "number", "input"])).unwrap();

        // non-numeric value for a number measure -> Err, not panic.
        assert!(run_args(&s(&["set", &db, "1", "notanumber"])).is_err());
        // unknown category name -> Err.
        run_args(&s(&["add-measure", &db, "2", "M2", "number", "input"])).unwrap();
        assert!(run_args(&s(&["set", &db, "2", "1", "--at", "Nope=X"])).is_err());
        // unknown command -> Err.
        assert!(run_args(&s(&["frobnicate"])).is_err());
    }

    fn parse_value_covers_types() {
        assert!(matches!(
            parse_value("1.5", ValueType::Number),
            Ok(Value::Number(_))
        ));
        assert!(matches!(
            parse_value("true", ValueType::Boolean),
            Ok(Value::Boolean(true))
        ));
        assert!(matches!(
            parse_value("hi", ValueType::Text),
            Ok(Value::Text(_))
        ));
        assert!(parse_value("nope", ValueType::Boolean).is_err());
    }

    #[test]
    fn value_parsing() {
        parse_value_covers_types();
    }

    // tiny RAII temp-file cleanup, no external crate.
    struct Guard(String);
    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    fn scopeguard(db: &str) -> Guard {
        Guard(db.to_string())
    }

    #[test]
    fn derived_formula_eval() {
        // Full v1 flow through the CLI: dimensioned inputs, a derived measure
        // defined from a textual formula, evaluated through the engine.
        let db = tmp_db();
        let _guard = scopeguard(&db);

        run_args(&s(&["init", &db])).unwrap();
        run_args(&s(&["add-category", &db, "1", "Time"])).unwrap();
        run_args(&s(&["add-category", &db, "2", "Product"])).unwrap();
        run_args(&s(&["add-item", &db, "10", "1", "Y2025"])).unwrap();
        run_args(&s(&["add-item", &db, "20", "2", "WidgetA"])).unwrap();
        run_args(&s(&[
            "add-measure",
            &db,
            "100",
            "Price",
            "number",
            "input",
            "Product",
        ]))
        .unwrap();
        run_args(&s(&[
            "add-measure",
            &db,
            "101",
            "Quantity",
            "number",
            "input",
            "Time",
            "Product",
        ]))
        .unwrap();
        run_args(&s(&["set", &db, "100", "10", "--at", "Product=WidgetA"])).unwrap();
        run_args(&s(&[
            "set",
            &db,
            "101",
            "7",
            "--at",
            "Time=Y2025,Product=WidgetA",
        ]))
        .unwrap();

        // Define a derived measure from a textual formula.
        run_args(&s(&[
            "add-derived",
            &db,
            "102",
            "Revenue",
            "Price * Quantity",
        ]))
        .unwrap();

        // The derived measure exists, is derived, and inferred its categories.
        let mut store = Store::open(&db).unwrap();
        let model = store.load_model().unwrap();
        let rev = model.measure_by_name("Revenue").unwrap();
        assert!(rev.is_derived());
        assert_eq!(rev.categories.len(), 2);

        // The engine computes Revenue[Y2025,WidgetA] = 10 * 7 = 70.
        let out = improv_engine::dataflow::evaluate(&model, &[MeasureId(102)]).unwrap();
        let cells = out.get(&MeasureId(102)).unwrap();
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(
            cells.get(key.as_slice()).and_then(|v| v.as_num()),
            Some(70.0)
        );

        // `eval` on an input measure is a clear error, not a crash.
        assert!(run_args(&s(&["eval", &db, "100"])).is_err());
    }

    #[test]
    fn import_sql_creates_measure_and_cells() {
        let db = tmp_db();
        let _guard = scopeguard(&db);
        // A source SQLite DB with a sales table.
        let src = format!("{db}.src.sqlite");
        let _sguard = scopeguard(&src);
        {
            let conn = rusqlite::Connection::open(&src).unwrap();
            conn.execute_batch(
                "CREATE TABLE sales(t TEXT,p TEXT,r REAL);
                 INSERT INTO sales VALUES('2025','A',1000.0),('2025','B',500.0);",
            )
            .unwrap();
        }

        run_args(&s(&["init", &db])).unwrap();
        run_args(&s(&[
            "import-sql",
            &db,
            &src,
            "100",
            "Revenue",
            "SELECT t,p,r FROM sales",
            "r",
            "t:1:Time",
            "p:2:Product",
        ]))
        .unwrap();

        let mut store = Store::open(&db).unwrap();
        let model = store.load_model().unwrap();
        let m = model.measure_by_name("Revenue").unwrap();
        assert!(m.is_input());
        assert_eq!(m.categories.len(), 2);
        assert_eq!(model.inputs.len(), 2);
        assert_eq!(model.category_by_name("Time").unwrap().items.len(), 1); // just 2025
        assert_eq!(model.category_by_name("Product").unwrap().items.len(), 2);
    }

    fn stream_test_model() -> Model {
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
            kind: MeasureKind::Derived(improv_core_model::Formula::new(
                improv_core_model::Expr::BinaryOp(
                    improv_core_model::BinaryOp::Mul,
                    Box::new(improv_core_model::Expr::Ref(
                        MeasureId(100),
                        improv_core_model::DimensionSpec::default(),
                    )),
                    Box::new(improv_core_model::Expr::Ref(
                        MeasureId(101),
                        improv_core_model::DimensionSpec::default(),
                    )),
                ),
            )),
            description: None,
        });
        m.set_input(
            MeasureId(100),
            Coordinate::from_pairs([(p, ItemId(20))]),
            Value::Number(10.0),
        );
        m.set_input(
            MeasureId(101),
            Coordinate::from_pairs([(t, ItemId(10)), (p, ItemId(20))]),
            Value::Number(5.0),
        );
        m
    }

    #[test]
    fn resolve_measure_by_id_or_name() {
        let m = stream_test_model();
        assert_eq!(resolve_measure(&m, "100").unwrap(), MeasureId(100));
        assert_eq!(resolve_measure(&m, "Price").unwrap(), MeasureId(100));
        assert!(resolve_measure(&m, "nope").is_err());
        assert!(resolve_measure(&m, "9999").is_err()); // numeric but unknown
    }

    #[test]
    fn stream_line_applies_incrementally_and_diff_reports_only_changes() {
        let mut model = stream_test_model();
        let (mut engine, snap0) =
            improv_engine::session::Engine::new(&model, &[MeasureId(102)]).unwrap();
        // Base Revenue = 10 * 5 = 50.
        let key = improv_engine::encode_coord(&Coordinate::from_pairs([
            (CategoryId(1), ItemId(10)),
            (CategoryId(2), ItemId(20)),
        ]));
        assert_eq!(
            snap0.get(&MeasureId(102)).unwrap().get(&key),
            Some(&improv_engine::CellValue::num(50.0))
        );

        // Price -> 15 by name, no coordinate token needed... but Price DOES have
        // a dimension (Product), so the coordinate is required here.
        let snap1 = apply_stream_line(&mut model, &mut engine, "Price 15 Product=WidgetA").unwrap();
        assert_eq!(
            snap1.get(&MeasureId(102)).unwrap().get(&key),
            Some(&improv_engine::CellValue::num(75.0)) // 15*5
        );
        // The model's own input cell reflects the edit too (for the eventual save).
        assert_eq!(
            model.input(
                MeasureId(100),
                &Coordinate::from_pairs([(CategoryId(2), ItemId(20))])
            ),
            Some(&Value::Number(15.0))
        );

        // Quantity -> 8 by numeric id.
        let snap2 =
            apply_stream_line(&mut model, &mut engine, "101 8 Time=2025,Product=WidgetA").unwrap();
        assert_eq!(
            snap2.get(&MeasureId(102)).unwrap().get(&key),
            Some(&improv_engine::CellValue::num(120.0)) // 15*8
        );

        // The diff between snap1 and snap2 must report exactly the one changed
        // Revenue cell, not the whole snapshot.
        let mid = MeasureId(102);
        let before = snap1.get(&mid).cloned().unwrap_or_default();
        let after = snap2.get(&mid).cloned().unwrap_or_default();
        let changed: Vec<_> = after
            .iter()
            .filter(|(k, v)| before.get(*k) != Some(*v))
            .collect();
        assert_eq!(changed.len(), 1, "exactly one Revenue cell changed");
    }

    #[test]
    fn stream_line_errors_are_clear_not_panics() {
        let mut model = stream_test_model();
        let (mut engine, _) =
            improv_engine::session::Engine::new(&model, &[MeasureId(102)]).unwrap();
        assert!(apply_stream_line(&mut model, &mut engine, "").is_err());
        assert!(apply_stream_line(&mut model, &mut engine, "NoSuchMeasure 5").is_err());
        assert!(
            apply_stream_line(&mut model, &mut engine, "Price notanumber Product=WidgetA").is_err()
        );
        assert!(apply_stream_line(&mut model, &mut engine, "Price").is_err()); // missing value
        assert!(
            apply_stream_line(&mut model, &mut engine, "Price 15 Product=WidgetA extra").is_err()
        ); // too many fields
    }
    /// A forgotten `--at` used to report success and write a cell at the EMPTY
    /// coordinate, so a measure declared over `Product` gained a dimensionless
    /// cell that then evaluated to nothing. Stray tokens are now rejected, and
    /// the message points at the flag the user meant.
    #[test]
    fn stray_coordinate_argument_is_rejected_not_ignored() {
        let err = parse_at_flag(&["Product=A".to_string()]).unwrap_err();
        assert!(err.contains("--at Product=A"), "unhelpful message: {err}");

        // A non-pair stray token is still an error, just without the hint.
        assert!(parse_at_flag(&["junk".to_string()]).is_err());

        // Trailing junk after a valid --at is also caught.
        assert!(parse_at_flag(&[
            "--at".to_string(),
            "Product=A".to_string(),
            "leftover".to_string(),
        ])
        .is_err());

        // No arguments at all remains legal: a scalar measure has no coordinate.
        assert_eq!(parse_at_flag(&[]).unwrap(), Vec::new());

        // And the normal form still parses.
        assert_eq!(
            parse_at_flag(&["--at".to_string(), "Product=A".to_string()]).unwrap(),
            vec![("Product".to_string(), "A".to_string())]
        );
    }

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn tp() -> Vec<(String, String)> {
        vec![
            ("Time".to_string(), "Q1".to_string()),
            ("Product".to_string(), "Widget".to_string()),
        ]
    }

    #[test]
    fn at_flag_accepts_comma_repeated_and_space_separated_forms() {
        for form in [
            "--at Time=Q1,Product=Widget",
            "--at Time=Q1 --at Product=Widget",
            "--at Time=Q1 Product=Widget",
        ] {
            assert_eq!(parse_at_flag(&args(form)).unwrap(), tp(), "form: {form}");
        }
    }

    #[test]
    fn at_flag_rejects_duplicate_category() {
        for form in [
            "--at Time=Q1,Time=Q2",
            "--at Time=Q1 --at Time=Q2",
            "--at Time=Q1 Time=Q2",
        ] {
            let err = parse_at_flag(&args(form)).unwrap_err();
            assert!(err.contains("'Time' more than once"), "{form}: {err}");
        }
    }

    #[test]
    fn at_flag_still_rejects_garbage() {
        // Non-pair token after the pairs, bare --at, --at followed by a flag.
        for form in [
            "--at Time=Q1 junk",
            "--at Time=Q1 Product=Widget junk",
            "--at",
            "--at Time=Q1 --at",
            "--at --at Time=Q1",
            "--at Time=Q1 --bogus",
            "junk --at Time=Q1",
        ] {
            assert!(parse_at_flag(&args(form)).is_err(), "accepted: {form}");
        }
    }

    /// `--at` naming the wrong dimensions used to save a cell the measure has no
    /// cell for: it reported success, then evaluated to nothing because the
    /// engine joins on the DECLARED dimensions.
    #[test]
    fn coordinate_must_match_the_measures_declared_dimensions() {
        let model = stream_test_model();
        let price = MeasureId(100); // declared over Product only

        // Correct: exactly the declared dimensions.
        let ok = resolve_coord(
            &model,
            price,
            &[("Product".to_string(), "WidgetA".to_string())],
        );
        assert!(ok.is_ok(), "valid coordinate rejected: {ok:?}");

        // Empty when a dimension is required.
        let err = resolve_coord(&model, price, &[]).unwrap_err();
        assert!(err.contains("missing Product"), "got: {err}");

        // A category that is not one of its dimensions.
        let err =
            resolve_coord(&model, price, &[("Time".to_string(), "2025".to_string())]).unwrap_err();
        assert!(err.contains("not a dimension"), "got: {err}");

        // A measure that never declared its dimensions accepts any coordinate:
        // they are inferred from use, so there is nothing to validate against.
        let undeclared = MeasureId(900);
        let mut m2 = model.clone();
        m2.add_measure(Measure {
            id: undeclared,
            name: Name("Undeclared".into()),
            value_type: ValueType::Number,
            categories: vec![],
            kind: MeasureKind::Input,
            description: None,
        });
        assert!(
            resolve_coord(
                &m2,
                undeclared,
                &[("Product".to_string(), "WidgetA".to_string())]
            )
            .is_ok(),
            "an undeclared measure must still accept a coordinate"
        );
    }

    fn sql_of(path: &str, sql: &str) {
        rusqlite::Connection::open(path)
            .unwrap()
            .execute_batch(sql)
            .unwrap();
    }

    fn cell(db: &str, mid: u32, cat: u32, item: u32) -> Option<Value> {
        let model = Store::open(db).unwrap().load_model().unwrap();
        let coord = Coordinate::from_pairs([(CategoryId(cat), ItemId(item))]);
        model.input(MeasureId(mid), &coord).cloned()
    }

    /// Whether a `Restricted` Python body can run here (python3 + a working
    /// `bwrap`); see `tests/commands.rs`.
    fn python_sandbox() -> bool {
        use improv_core_model::{ExternalFn, Language, ValueType};
        let probe = ExternalFn {
            name: "probe".into(),
            language: Language::Python,
            body: "result = 1".into(),
            arg_types: vec![],
            return_type: ValueType::Number,
            pure: true,
        };
        let ok = improv_extfn::eval(&probe, &[], std::time::Duration::from_secs(5)).is_ok();
        if !ok {
            println!("skipped: python3 cannot run in the extfn sandbox here");
        }
        ok
    }

    /// A store with SQL measure 400 `Rev` (interval:60) over Region{A} and the
    /// CALL measure 200 `D = dbl(Rev)` (on-load). Returns (db, source, A's id).
    fn serve_fixture(db: &str, src: &str) -> u32 {
        sql_of(
            src,
            "CREATE TABLE sales(p TEXT, r REAL); INSERT INTO sales VALUES ('A', 1.0);",
        );
        run_args(&s(&["init", db])).unwrap();
        run_args(&s(&[
            "import-sql",
            db,
            src,
            "400",
            "Rev",
            "SELECT p,r FROM sales",
            "r",
            "p:6:Region",
            "--refresh",
            "interval:60",
        ]))
        .unwrap();
        run_args(&s(&[
            "register-ext",
            db,
            "dbl",
            "1",
            "result = args[0] * 2",
        ]))
        .unwrap();
        run_args(&s(&[
            "define",
            db,
            "200",
            "D = CALL(dbl, Rev)",
            "--refresh",
            "on-load",
        ]))
        .unwrap();
        let model = Store::open(db).unwrap().load_model().unwrap();
        model.items.values().find(|i| i.name.0 == "A").unwrap().id.0
    }

    #[test]
    fn serve_refresh_tick_refreshes_only_due_measures() {
        if !python_sandbox() {
            return;
        }
        let db = tmp_db();
        let src = format!("{db}.src.sqlite");
        let (_g, _s) = (scopeguard(&db), scopeguard(&src));
        let a = serve_fixture(&db, &src);
        let mut last_run = HashMap::new();

        // t=0: both are due (never run); the CALL runs over Rev's cells.
        serve_refresh_tick(&db, Some(&src), 0, &mut last_run).unwrap();
        assert_eq!(
            last_run,
            HashMap::from([(MeasureId(200), 0), (MeasureId(400), 0)])
        );
        assert_eq!(cell(&db, 200, 6, a), Some(Value::Number(2.0)));

        sql_of(&src, "UPDATE sales SET r = 5.0;");
        // t=30: on-load already ran, the interval has not elapsed: nothing due.
        serve_refresh_tick(&db, Some(&src), 30, &mut last_run).unwrap();
        assert_eq!(cell(&db, 400, 6, a), Some(Value::Number(1.0)));
        // t=60 with no source: the due SQL measure is skipped, not recorded.
        serve_refresh_tick(&db, None, 60, &mut last_run).unwrap();
        assert_eq!(last_run[&MeasureId(400)], 0);
        assert_eq!(cell(&db, 400, 6, a), Some(Value::Number(1.0)));
        // t=60 with the source: refreshed.
        serve_refresh_tick(&db, Some(&src), 60, &mut last_run).unwrap();
        assert_eq!(last_run[&MeasureId(400)], 60);
        assert_eq!(cell(&db, 400, 6, a), Some(Value::Number(5.0)));
    }

    #[test]
    fn serve_refresh_tick_propagates_every_failure() {
        let db = tmp_db();
        let src = format!("{db}.src.sqlite");
        let empty = format!("{db}.empty.sqlite");
        let (_g, _s, _e) = (scopeguard(&db), scopeguard(&src), scopeguard(&empty));
        serve_fixture(&db, &src);
        let only_sql_due = || HashMap::from([(MeasureId(200), 0)]);

        // Unopenable source.
        let e = serve_refresh_tick(
            &db,
            Some("/nonexistent-improv-dir/s.db"),
            0,
            &mut HashMap::new(),
        );
        assert!(e.is_err());
        // Source without the table: the SQL refresh fails.
        sql_of(&empty, "");
        let e = serve_refresh_tick(&db, Some(&empty), 0, &mut only_sql_due()).unwrap_err();
        assert!(e.contains("sales"), "{e}");
        // The CALL fails once its function's arity no longer matches.
        run_args(&s(&["register-ext", &db, "dbl", "2", "result = 0"])).unwrap();
        let e = serve_refresh_tick(&db, None, 0, &mut HashMap::new()).unwrap_err();
        assert!(e.contains("dbl"), "{e}");

        // Save failure: new source data, but every new datom is blocked.
        sql_of(&src, "UPDATE sales SET r = 7.0;");
        sql_of(
            &db,
            "CREATE TRIGGER t BEFORE INSERT ON datoms \
             WHEN NEW.a <> (SELECT e FROM idents WHERE v = ':db/txInstant') \
             BEGIN SELECT RAISE(ABORT, 'blocked'); END;",
        );
        let e = serve_refresh_tick(&db, Some(&src), 0, &mut only_sql_due()).unwrap_err();
        assert!(e.contains("mentat"), "{e}");
        // Load failure: corrupt the stored external-functions JSON.
        sql_of(
            &db,
            "UPDATE datoms SET v = 'not json' \
             WHERE a = (SELECT e FROM idents WHERE v = ':meta/external-fns');",
        );
        let e = serve_refresh_tick(&db, None, 0, &mut HashMap::new()).unwrap_err();
        assert!(e.contains("serialization"), "{e}");
    }

    #[test]
    fn names_fall_back_to_ids_when_absent_from_the_model() {
        let empty = Model::new();
        assert_eq!(render_coord_key(&empty, &[(7, 8), (9, 1)]), "7=8, 9=1");
        let coord = Coordinate::from_pairs([(CategoryId(7), ItemId(8))]);
        assert_eq!(fmt_coord(&empty, &coord), "7=8");

        let mut m = stream_test_model();
        m.set_input(MeasureId(5), coord, Value::Enum(3));
        assert!(input_lines(&m).contains(&"5::{7=8} = Enum(3)".to_string()));

        // A declared dimension the model has no category for is named by id.
        m.add_measure(Measure {
            id: MeasureId(901),
            name: Name("Ghost".into()),
            value_type: ValueType::Number,
            categories: vec![CategoryId(99)],
            kind: MeasureKind::Input,
            description: None,
        });
        let err = resolve_coord(&m, MeasureId(901), &[]).unwrap_err();
        assert!(err.contains("missing category 99"), "{err}");
        // An unknown measure has no declared dimensions to check against.
        let pairs = [("Product".to_string(), "WidgetA".to_string())];
        assert!(resolve_coord(&m, MeasureId(12345), &pairs).is_ok());
    }

    #[test]
    fn parse_value_rejects_types_the_cli_cannot_set() {
        let e = parse_value("1", ValueType::DateTime).unwrap_err();
        assert!(e.contains("cannot set a DateTime measure"), "{e}");
        assert_eq!(fmt_value(&Value::Boolean(false)), "false");
    }
}
