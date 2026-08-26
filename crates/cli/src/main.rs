//! Improv CLI (Phase 2, headless subset).
//!
//! A thin front-end over `improv_storage_mentat::ModelStore`. Each subcommand
//! opens the store, mutates the loaded `Model`, and saves it back.
//!
//! Coordinates (`--at`) resolve category *names* and item *names* to ids via
//! the loaded model, e.g. `--at Time=2025,Product=WidgetA`.

use improv_core_model::{
    parser, CategoryId, Coordinate, ItemId, Measure, MeasureId, MeasureKind, Model, Name, Value,
    ValueType,
};
use improv_storage_mentat::ModelStore;

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
        "set" => cmd_set(rest),
        "list" => cmd_list(rest),
        "show" => cmd_show(rest),
        "eval" => cmd_eval(rest),
        "export" => cmd_export(rest),
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
}
