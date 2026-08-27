//! Improv CLI (Phase 2, headless subset).
//!
//! A thin front-end over `improv_storage_mentat::ModelStore`. Each subcommand
//! opens the store, mutates the loaded `Model`, and saves it back.
//!
//! Coordinates (`--at`) resolve category *names* and item *names* to ids via
//! the loaded model, e.g. `--at Time=2025,Product=WidgetA`.

use improv_core_model::{
    parser, parser::Definition, CategoryId, Coordinate, ExternalCall, ItemId, Measure, MeasureId,
    MeasureKind, Model, Name, RefreshPolicy, Value, ValueType,
};
use improv_storage_mentat::ModelStore;
use improv_storage_sql::{
    add_sql_measure, export_measure, refresh_sql_measure, DimensionMapping, ImportSpec,
};

const USAGE: &str = "\
improv — headless spreadsheet model CLI

USAGE:
    improv <command> [args]

COMMANDS:
    init <db>
        Create/open the store (installs schema) and save an empty model.

    add-category <db> <id> <name>
        Add a category (dimension). <id> is a number.

    add-item <db> <id> <category-id> <name>
        Add an item (member of a category). <id>, <category-id> are numbers.

    add-measure <db> <id> <name> <number|boolean|text> input [Category ...]
        Add an input measure. The type declares how `set` parses values.
        Trailing category NAMES declare the dimensions it ranges over.

    add-derived <db> <id> <name> <formula>
        Add a derived (formula) measure, e.g.
        add-derived m.db 102 Revenue \"Price * Quantity\".
        Categories are inferred from the referenced measures.

    define <db> <id> <definition>
        Create a measure from a definition string. Two forms:
          Formula: define m.db 102 \"Revenue = Price * Quantity\"
          External call: define m.db 200 \"H = CALL(hypot, Price, Quantity)\"
        (SQL(...) form: use import-sql, which needs a column-to-dimension map.)

    register-ext <db> <name> <arity> <python-body>
        Register a pure Python external function (Number args + Number result;
        the body binds a list `args` and sets `result`), e.g.
        register-ext m.db hypot 2 \"result = (args[0]**2 + args[1]**2) ** 0.5\".

    refresh-ext <db> <measure-id>
        (Re)populate a CALL(...) measure by running its function over its
        argument measures, host-side (external calls stay off the engine path).

    set <db> <measure-id> <value> [--at Cat=Item,Cat=Item ...]
        Set an input cell. --at maps category NAMES to item NAMES.
        Value is parsed by the measure's declared type.

    list <db>
        Print categories, items, measures, and input cells.

    show <db> <measure-id>
        Print one measure and its input cells.

    eval <db> <measure-id>
        Compute a derived measure via the engine and print its cells.

    export <db>
        Print the whole model as pretty JSON.

    import-sql <db> <source.sqlite> <measure-id> <measure-name> <SELECT> \
               <value-col> <dim-col:cat-id:cat-name> [<dim-col:cat-id:cat-name> ...]
        Import a SQLite query into a new input measure. Each dim-col maps a
        result column to a category (distinct values become items); value-col
        is the numeric measure value. Example:
        import-sql m.db sales.db 100 Revenue \"SELECT t,p,r FROM sales\" r \
                   t:1:Time p:2:Product
        The measure is SQL-backed and can be re-run with refresh-sql.
        Add --refresh <manual|on-load|interval:SECS> to record a refresh policy.

    refresh-sql <db> <source.sqlite> <measure-id>
        Re-run an SQL-backed measure's stored query and replace its cells with
        the fresh result (new dimension values become new items).

    refresh-all <db> [source.sqlite]
        Refresh every external-sourced measure at once: all CALL(...) measures,
        plus all SQL-backed measures when a source.sqlite is given. Reports each
        measure's refresh policy. (Timing policies are advisory metadata; this
        is the manual \"do it now\" batch.)

    export-sql <db> <target.sqlite> <measure-id> <table> <value-col>
        Write a measure's input cells to a SQLite table (one column per
        dimension category + the value column; created if absent).

    help | --help
        Show this help.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(e) = run(&args) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let cmd = args.first().map(String::as_str).unwrap_or("help");
    let rest = &args[1.min(args.len())..];
    match cmd {
        "help" | "--help" | "-h" => {
            print!("{USAGE}");
            Ok(())
        }
        "init" => cmd_init(rest),
        "add-category" => cmd_add_category(rest),
        "add-item" => cmd_add_item(rest),
        "add-measure" => cmd_add_measure(rest),
        "add-derived" => cmd_add_derived(rest),
        "define" => cmd_define(rest),
        "register-ext" => cmd_register_ext(rest),
        "refresh-ext" => cmd_refresh_ext(rest),
        "set" => cmd_set(rest),
        "list" => cmd_list(rest),
        "show" => cmd_show(rest),
        "eval" => cmd_eval(rest),
        "export" => cmd_export(rest),
        "import-sql" => cmd_import_sql(rest),
        "refresh-sql" => cmd_refresh_sql(rest),
        "refresh-all" => cmd_refresh_all(rest),
        "export-sql" => cmd_export_sql(rest),
        other => Err(format!("unknown command '{other}'\n\n{USAGE}")),
    }
}

// --- helpers ---

fn open(db: &str) -> Result<ModelStore, String> {
    ModelStore::open(db).map_err(|e| e.to_string())
}

fn arg<'a>(rest: &'a [String], i: usize, name: &str) -> Result<&'a str, String> {
    rest.get(i)
        .map(String::as_str)
        .ok_or_else(|| format!("missing argument <{name}>"))
}

fn parse_u32(s: &str, name: &str) -> Result<u32, String> {
    s.parse::<u32>()
        .map_err(|_| format!("<{name}> must be a number, got '{s}'"))
}

// --- commands ---

fn cmd_init(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let mut store = open(db)?;
    store.save_model(&Model::new()).map_err(|e| e.to_string())?;
    println!("initialized store at '{db}'");
    Ok(())
}

fn cmd_add_category(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let id = CategoryId(parse_u32(arg(rest, 1, "id")?, "id")?);
    let name = arg(rest, 2, "name")?;
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    model.add_category(id, name);
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("added category {} '{name}'", id.0);
    Ok(())
}

fn cmd_add_item(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let id = ItemId(parse_u32(arg(rest, 1, "id")?, "id")?);
    let cat = CategoryId(parse_u32(arg(rest, 2, "category-id")?, "category-id")?);
    let name = arg(rest, 3, "name")?;
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

fn cmd_add_measure(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let id = MeasureId(parse_u32(arg(rest, 1, "id")?, "id")?);
    let name = arg(rest, 2, "name")?;
    let value_type = parse_value_type(arg(rest, 3, "type")?)?;
    let kind = arg(rest, 4, "input")?;
    if kind != "input" {
        return Err(format!("only 'input' measures are supported, got '{kind}'"));
    }
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    // Optional trailing args name the categories this measure ranges over,
    // resolved by name against the model, e.g. `... input Time Product`.
    let mut categories = Vec::new();
    for cat_name in &rest[5.min(rest.len())..] {
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

fn cmd_add_derived(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let id = MeasureId(parse_u32(arg(rest, 1, "id")?, "id")?);
    let name = arg(rest, 2, "name")?;
    let formula_text = arg(rest, 3, "formula")?;

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    // Parse the RHS expression, resolving measure/category names against the model.
    let formula = parser::parse_expr(&model, formula_text).map_err(|e| e.to_string())?;

    // Infer the derived measure's categories as the union of the categories of
    // every measure the formula references (aggregation collapses are handled
    // by the engine at evaluation time; this is the declared shape).
    let mut cats: Vec<CategoryId> = Vec::new();
    for m in formula.referenced_measures() {
        if let Some(measure) = model.measures.get(&m) {
            for c in &measure.categories {
                if !cats.contains(c) {
                    cats.push(*c);
                }
            }
        }
    }
    cats.sort_by_key(|c| c.0);

    model.add_measure(Measure {
        id,
        name: Name(name.to_string()),
        value_type: ValueType::Number,
        categories: cats,
        kind: MeasureKind::Derived(formula),
        description: None,
    });
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("added derived measure {} '{name}'", id.0);
    Ok(())
}

/// `define <db> <id> "<Target = ...>"` — create a measure from a definition
/// string. Supports an ordinary formula (`Target = Price * Quantity`) and the
/// external-call source form (`Target = CALL(func, ArgMeasure, ...)`). The SQL
/// source form is handled by `import-sql` (it needs a column→dimension mapping
/// beyond the query string). The target name is taken from the definition's LHS.
fn cmd_define(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let id = MeasureId(parse_u32(arg(rest, 1, "id")?, "id")?);
    let text = arg(rest, 2, "definition")?;

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    match parser::parse_definition(&model, text).map_err(|e| e.to_string())? {
        Definition::Formula(ft) => {
            let mut cats: Vec<CategoryId> = Vec::new();
            for m in ft.formula.referenced_measures() {
                if let Some(measure) = model.measures.get(&m) {
                    for c in &measure.categories {
                        if !cats.contains(c) {
                            cats.push(*c);
                        }
                    }
                }
            }
            cats.sort_by_key(|c| c.0);
            model.add_measure(Measure {
                id,
                name: ft.target.clone(),
                value_type: ValueType::Number,
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
    }
    store.save_model(&model).map_err(|e| e.to_string())?;
    Ok(())
}

/// `register-ext <db> <name> <arity> "<python body>"` — register a pure Python
/// external function (all Number args + Number result; the body binds `args`
/// and sets `result`). Enough to make `define ... CALL(name, ...)` usable from
/// the CLI; richer type signatures are a future flag.
fn cmd_register_ext(rest: &[String]) -> Result<(), String> {
    use improv_core_model::{ExternalFn, Language};
    let db = arg(rest, 0, "db")?;
    let name = arg(rest, 1, "name")?;
    let arity = parse_u32(arg(rest, 2, "arity")?, "arity")? as usize;
    let body = arg(rest, 3, "python-body")?;

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

/// Parse an optional `--refresh <manual|on-load|interval:SECS>` flag from an
/// argument list; absent means `Manual`.
fn parse_refresh_flag(rest: &[String]) -> Result<RefreshPolicy, String> {
    let Some(pos) = rest.iter().position(|a| a == "--refresh") else {
        return Ok(RefreshPolicy::Manual);
    };
    let val = rest
        .get(pos + 1)
        .ok_or("--refresh needs a value: manual | on-load | interval:SECS")?;
    match val.as_str() {
        "manual" => Ok(RefreshPolicy::Manual),
        "on-load" => Ok(RefreshPolicy::OnLoad),
        other => {
            if let Some(secs) = other.strip_prefix("interval:") {
                let secs = secs
                    .parse::<u64>()
                    .map_err(|_| "interval:SECS must be a number")?;
                Ok(RefreshPolicy::Interval { secs })
            } else {
                Err(format!(
                    "unknown refresh policy '{other}': use manual | on-load | interval:SECS"
                ))
            }
        }
    }
}

/// `refresh-all <db> [source.sqlite]` — refresh every external-sourced measure:
/// all external-function (`CALL`) measures, plus all SQL-backed measures when a
/// SQLite source is given. This honors nothing about `RefreshPolicy` timing
/// itself — it is the manual "do it now" batch; a scheduler that consults the
/// policy is future work. Policy metadata is reported so it is visible.
fn cmd_refresh_all(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let source = rest.get(1).cloned();

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    let mut total = 0usize;
    // External-function measures need no external connection.
    let ext_ids: Vec<MeasureId> = model.external_calls.keys().copied().collect();
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
    let sql_ids: Vec<MeasureId> = model.sql_sources.keys().copied().collect();
    if !sql_ids.is_empty() {
        match &source {
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

/// `refresh-ext <db> <measure-id>` — (re)populate an external-call measure by
/// running its function over its argument measures, host-side.
fn cmd_refresh_ext(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let mid = MeasureId(parse_u32(arg(rest, 1, "measure-id")?, "measure-id")?);

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

fn cmd_set(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let mid = MeasureId(parse_u32(arg(rest, 1, "measure-id")?, "measure-id")?);
    let value_arg = arg(rest, 2, "value")?;
    let at = parse_at_flag(&rest[3.min(rest.len())..])?;

    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;

    let vt = model
        .measures
        .get(&mid)
        .ok_or_else(|| format!("no measure with id {}", mid.0))?
        .value_type;
    let value = parse_value(value_arg, vt)?;
    let coord = resolve_coord(&model, &at)?;

    model.set_input(mid, coord, value);
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("set measure {}", mid.0);
    Ok(())
}

fn cmd_list(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
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
    println!("inputs:");
    for line in input_lines(&model) {
        println!("  {line}");
    }
    Ok(())
}

fn cmd_show(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let mid = MeasureId(parse_u32(arg(rest, 1, "measure-id")?, "measure-id")?);
    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;
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

fn cmd_export(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(&model).map_err(|e| e.to_string())?;
    println!("{json}");
    Ok(())
}

fn cmd_eval(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let mid = MeasureId(parse_u32(arg(rest, 1, "measure-id")?, "measure-id")?);
    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;

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

fn cmd_import_sql(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let source = arg(rest, 1, "source.sqlite")?;
    let measure_id = MeasureId(parse_u32(arg(rest, 2, "measure-id")?, "measure-id")?);
    let measure_name = arg(rest, 3, "measure-name")?.to_string();
    let query = arg(rest, 4, "SELECT")?.to_string();
    let value_column = arg(rest, 5, "value-col")?.to_string();

    // Remaining args: dim-col:cat-id:cat-name, plus an optional
    // `--refresh <manual|on-load|interval:SECS>` flag anywhere after them.
    let refresh_policy = parse_refresh_flag(rest)?;
    let mut dimensions = Vec::new();
    let mut i = 6;
    while i < rest.len() {
        let spec = &rest[i];
        if spec == "--refresh" {
            i += 2; // skip flag + its value
            continue;
        }
        let parts: Vec<&str> = spec.splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(format!(
                "dimension spec '{spec}' must be <col>:<cat-id>:<cat-name>"
            ));
        }
        dimensions.push(DimensionMapping {
            column: parts[0].to_string(),
            category_id: CategoryId(parse_u32(parts[1], "cat-id")?),
            category_name: parts[2].to_string(),
        });
        i += 1;
    }
    if dimensions.is_empty() {
        return Err("at least one dimension mapping is required".into());
    }

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

fn cmd_refresh_sql(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let source = arg(rest, 1, "source.sqlite")?;
    let measure_id = MeasureId(parse_u32(arg(rest, 2, "measure-id")?, "measure-id")?);

    let conn = rusqlite::Connection::open(source).map_err(|e| e.to_string())?;
    let mut store = open(db)?;
    let mut model = store.load_model().map_err(|e| e.to_string())?;
    let n = refresh_sql_measure(&conn, &mut model, measure_id).map_err(|e| e.to_string())?;
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("refreshed measure {} from SQL: {n} cells", measure_id.0);
    Ok(())
}

fn cmd_export_sql(rest: &[String]) -> Result<(), String> {
    let db = arg(rest, 0, "db")?;
    let target = arg(rest, 1, "target.sqlite")?;
    let measure_id = MeasureId(parse_u32(arg(rest, 2, "measure-id")?, "measure-id")?);
    let table = arg(rest, 3, "table")?;
    let value_column = arg(rest, 4, "value-col")?;

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

// --- parsing / resolution ---

fn parse_value_type(s: &str) -> Result<ValueType, String> {
    match s {
        "number" => Ok(ValueType::Number),
        "boolean" => Ok(ValueType::Boolean),
        "text" => Ok(ValueType::Text),
        other => Err(format!("type must be number|boolean|text, got '{other}'")),
    }
}

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

/// Collect the `--at Cat=Item,...` pairs. Absent flag => empty (scalar cell).
fn parse_at_flag(rest: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut it = rest.iter();
    while let Some(tok) = it.next() {
        if tok == "--at" {
            let spec = it
                .next()
                .ok_or_else(|| "--at needs an argument, e.g. Time=2025".to_string())?;
            return parse_pairs(spec);
        }
    }
    Ok(Vec::new())
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
fn resolve_coord(model: &Model, pairs: &[(String, String)]) -> Result<Coordinate, String> {
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
mod tests {
    use super::*;

    #[test]
    fn refresh_flag_parses_all_forms() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_refresh_flag(&s(&[])).unwrap(), RefreshPolicy::Manual);
        assert_eq!(
            parse_refresh_flag(&s(&["--refresh", "on-load"])).unwrap(),
            RefreshPolicy::OnLoad
        );
        assert_eq!(
            parse_refresh_flag(&s(&["x", "--refresh", "interval:30"])).unwrap(),
            RefreshPolicy::Interval { secs: 30 }
        );
        assert!(parse_refresh_flag(&s(&["--refresh", "hourly"])).is_err());
        assert!(parse_refresh_flag(&s(&["--refresh"])).is_err()); // missing value
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

        run(&s(&["init", &db])).unwrap();
        run(&s(&["add-category", &db, "1", "Product"])).unwrap();
        run(&s(&["add-item", &db, "20", "1", "WidgetA"])).unwrap();
        run(&s(&["add-measure", &db, "100", "Price", "number", "input"])).unwrap();
        run(&s(&["set", &db, "100", "10.5", "--at", "Product=WidgetA"])).unwrap();

        let mut store = ModelStore::open(&db).unwrap();
        let model = store.load_model().unwrap();

        assert!(model.category_by_name("Product").is_some());
        assert!(model
            .items
            .values()
            .any(|i| i.name.0 == "WidgetA" && i.category == CategoryId(1)));
        let m = model.measure_by_name("Price").unwrap();
        assert_eq!(m.value_type as u8, ValueType::Number as u8);
        assert!(m.is_input());

        let coord = Coordinate::from_pairs([(CategoryId(1), ItemId(20))]);
        assert_eq!(
            model.input(MeasureId(100), &coord),
            Some(&Value::Number(10.5))
        );
    }

    #[test]
    fn bad_input_does_not_panic() {
        let db = tmp_db();
        let _guard = scopeguard(&db);
        run(&s(&["init", &db])).unwrap();
        run(&s(&["add-measure", &db, "1", "M", "number", "input"])).unwrap();

        // non-numeric value for a number measure -> Err, not panic.
        assert!(run(&s(&["set", &db, "1", "notanumber"])).is_err());
        // unknown category name -> Err.
        run(&s(&["add-measure", &db, "2", "M2", "number", "input"])).unwrap();
        assert!(run(&s(&["set", &db, "2", "1", "--at", "Nope=X"])).is_err());
        // unknown command -> Err.
        assert!(run(&s(&["frobnicate"])).is_err());
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

        run(&s(&["init", &db])).unwrap();
        run(&s(&["add-category", &db, "1", "Time"])).unwrap();
        run(&s(&["add-category", &db, "2", "Product"])).unwrap();
        run(&s(&["add-item", &db, "10", "1", "Y2025"])).unwrap();
        run(&s(&["add-item", &db, "20", "2", "WidgetA"])).unwrap();
        run(&s(&[
            "add-measure",
            &db,
            "100",
            "Price",
            "number",
            "input",
            "Product",
        ]))
        .unwrap();
        run(&s(&[
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
        run(&s(&["set", &db, "100", "10", "--at", "Product=WidgetA"])).unwrap();
        run(&s(&[
            "set",
            &db,
            "101",
            "7",
            "--at",
            "Time=Y2025,Product=WidgetA",
        ]))
        .unwrap();

        // Define a derived measure from a textual formula.
        run(&s(&[
            "add-derived",
            &db,
            "102",
            "Revenue",
            "Price * Quantity",
        ]))
        .unwrap();

        // The derived measure exists, is derived, and inferred its categories.
        let mut store = ModelStore::open(&db).unwrap();
        let model = store.load_model().unwrap();
        let rev = model.measure_by_name("Revenue").unwrap();
        assert!(rev.is_derived());
        assert_eq!(rev.categories.len(), 2);

        // The engine computes Revenue[Y2025,WidgetA] = 10 * 7 = 70.
        let out = improv_engine::dataflow::evaluate(&model, &[MeasureId(102)]).unwrap();
        let cells = out.get(&MeasureId(102)).unwrap();
        let mut key = vec![(1u32, 10u32), (2u32, 20u32)];
        key.sort();
        assert_eq!(cells.get(&key).and_then(|v| v.as_num()), Some(70.0));

        // `eval` on an input measure is a clear error, not a crash.
        assert!(run(&s(&["eval", &db, "100"])).is_err());
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

        run(&s(&["init", &db])).unwrap();
        run(&s(&[
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

        let mut store = ModelStore::open(&db).unwrap();
        let model = store.load_model().unwrap();
        let m = model.measure_by_name("Revenue").unwrap();
        assert!(m.is_input());
        assert_eq!(m.categories.len(), 2);
        assert_eq!(model.inputs.len(), 2);
        assert_eq!(model.category_by_name("Time").unwrap().items.len(), 1); // just 2025
        assert_eq!(model.category_by_name("Product").unwrap().items.len(), 2);
    }
}
