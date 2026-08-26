//! Improv CLI (Phase 2, headless subset).
//!
//! A thin front-end over `improv_storage_mentat::ModelStore`. Each subcommand
//! opens the store, mutates the loaded `Model`, and saves it back.
//!
//! Coordinates (`--at`) resolve category *names* and item *names* to ids via
//! the loaded model, e.g. `--at Time=2025,Product=WidgetA`.

use improv_core_model::{
    CategoryId, Coordinate, ItemId, Measure, MeasureId, MeasureKind, Model, Name, Value, ValueType,
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

    add-measure <db> <id> <name> <number|boolean|text> input
        Add an input measure. The type declares how `set` parses values.

    set <db> <measure-id> <value> [--at Cat=Item,Cat=Item ...]
        Set an input cell. --at maps category NAMES to item NAMES.
        Value is parsed by the measure's declared type.

    list <db>
        Print categories, items, measures, and input cells.

    show <db> <measure-id>
        Print one measure and its input cells.

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
        "set" => cmd_set(rest),
        "list" => cmd_list(rest),
        "show" => cmd_show(rest),
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
    model.add_measure(Measure {
        id,
        name: Name(name.to_string()),
        value_type,
        categories: Vec::new(),
        kind: MeasureKind::Input,
        description: None,
    });
    store.save_model(&model).map_err(|e| e.to_string())?;
    println!("added input measure {} '{name}'", id.0);
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
}
