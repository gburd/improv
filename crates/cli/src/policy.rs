//! `improv policy …`: a model's access policies (`improv_policy`), stored
//! with a local Mentat model.

use std::sync::Arc;

use improv_core_model::{CategoryId, Coordinate, ItemId, MeasureId, Model, ViewId};
use improv_policy::templates::{self, Who};
use improv_policy::{Action, Channel, Policies, Principal, Resource, Reveals, Session};

use crate::args::PolicyCmd;
use crate::{coordinate, open, resolve_coord, resolve_measure};

pub(crate) fn run(cmd: PolicyCmd) -> Result<(), String> {
    match cmd {
        PolicyCmd::Validate { db, file, groups } => validate(&db, &file, groups.as_deref()),
        PolicyCmd::Set { db, file, groups } => set(&db, &file, groups.as_deref()),
        PolicyCmd::Show { db, groups } => show(&db, groups),
        PolicyCmd::Clear { db } => clear(&db),
        PolicyCmd::Check {
            db,
            user,
            role,
            action,
            measure,
            item,
            category,
            view,
            at,
            policies,
            groups,
        } => {
            let target = Target {
                measure,
                item,
                category,
                view,
                at,
            };
            let policies = policies.map(|p| (p, groups));
            check(&db, &user, role, &action, &target, policies)
        }
        PolicyCmd::Template {
            db,
            name,
            user,
            group,
            measure,
            item,
        } => template(&db, &name, user, group, &measure, &item),
        PolicyCmd::Lint { db, user, role } => lint(&db, &user, role),
    }
}

fn read(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

/// Parse a policy file and groups file, and report the validator's
/// warnings and the ids the model lacks on stderr.
fn parse(model: &Model, file: &str, groups: Option<&str>) -> Result<Policies, String> {
    let text = read(file)?;
    let groups = groups.map(read).transpose()?.unwrap_or_default();
    let p = Policies::parse(&text, &groups).map_err(|e| format!("{file}:\n{e}"))?;
    for w in p.warnings() {
        eprintln!("warning: {w}");
    }
    for n in p.unknown_names(model) {
        eprintln!(
            "warning: the policies name {n}, which this model does not have \
             (policies name measures, items, categories and views by id)"
        );
    }
    Ok(p)
}

fn validate(db: &str, file: &str, groups: Option<&str>) -> Result<(), String> {
    let model = open(db)?.load_model().map_err(|e| e.to_string())?;
    let p = parse(&model, file, groups)?;
    println!("{} policies are valid", p.len());
    Ok(())
}

fn set(db: &str, file: &str, groups: Option<&str>) -> Result<(), String> {
    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;
    let p = parse(&model, file, groups)?;
    let groups_json = serde_json::to_string(p.groups()).map_err(|e| e.to_string())?;
    let version = store
        .save_policies(Some((p.text(), &groups_json)))
        .map_err(|e| e.to_string())?;
    println!("set {} policies (version {version})", p.len());
    Ok(())
}

fn show(db: &str, groups: bool) -> Result<(), String> {
    match open(db)?.load_policies().map_err(|e| e.to_string())? {
        None => println!("no policies: the model is unrestricted"),
        Some(p) if groups => println!("{}", p.groups),
        Some(p) => {
            println!("// version {}", p.version);
            print!("{}", p.text);
            if !p.text.ends_with('\n') {
                println!();
            }
        }
    }
    Ok(())
}

fn clear(db: &str) -> Result<(), String> {
    let v = open(db)?.save_policies(None).map_err(|e| e.to_string())?;
    println!("cleared the policies (version {v}): the model is unrestricted");
    Ok(())
}

/// The stored policies, parsed (a store holds only validated text).
fn stored(store: &mut improv_store::Store) -> Result<Option<Policies>, String> {
    let Some(s) = store.load_policies().map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    Policies::parse(&s.text, &s.groups).map(Some).map_err(|e| {
        format!(
            "the stored policies (version {}) do not validate:\n{e}",
            s.version
        )
    })
}

struct Target {
    measure: Option<String>,
    item: Option<String>,
    category: Option<String>,
    view: Option<String>,
    at: Vec<String>,
}

/// An item given as `Category=Item`.
fn item(model: &Model, spec: &str) -> Result<ItemId, String> {
    let (cat, name) = spec
        .split_once('=')
        .ok_or_else(|| format!("--item wants Category=Item, got '{spec}'"))?;
    let c = model
        .category_by_name(cat)
        .ok_or_else(|| format!("no category named '{cat}'"))?;
    (model.items.values())
        .find(|i| i.category == c.id && i.name.0 == name)
        .map(|i| i.id)
        .ok_or_else(|| format!("no item named '{name}' in category '{cat}'"))
}

fn resource(model: &Model, t: &Target) -> Result<Resource, String> {
    let given = [
        t.measure.is_some(),
        t.item.is_some(),
        t.category.is_some(),
        t.view.is_some(),
    ];
    if given.iter().filter(|g| **g).count() > 1 {
        return Err("give one of --measure, --item, --category and --view".into());
    }
    if !t.at.is_empty() && t.measure.is_none() {
        return Err("--at names a cell of a --measure".into());
    }
    if let Some(m) = &t.measure {
        let mid = resolve_measure(model, m)?;
        return Ok(match t.at.is_empty() {
            true => Resource::Measure(mid),
            false => Resource::Cell(mid, resolve_coord(model, mid, &coordinate(&t.at, &[])?)?),
        });
    }
    if let Some(i) = &t.item {
        return item(model, i).map(Resource::Item);
    }
    if let Some(c) = &t.category {
        return model
            .category_by_name(c)
            .map(|c| Resource::Category(c.id))
            .ok_or_else(|| format!("no category named '{c}'"));
    }
    if let Some(v) = &t.view {
        return model
            .view_by_name(v)
            .map(|v| Resource::View(v.id))
            .ok_or_else(|| format!("no view named '{v}'"));
    }
    Ok(Resource::Model)
}

/// A resource as the user named it, with its id: `measure Price (100)`.
fn describe(model: &Model, r: &Resource) -> String {
    let measure = |m: &MeasureId| {
        model
            .measures
            .get(m)
            .map_or(format!("measure {}", m.0), |x| {
                format!("measure {} ({})", x.name, m.0)
            })
    };
    let item = |i: &ItemId| {
        model.items.get(i).map_or(format!("item {}", i.0), |x| {
            format!("item {} ({})", x.name, i.0)
        })
    };
    match r {
        Resource::Model => "the model".into(),
        Resource::Category(CategoryId(c)) => match model.categories.get(&CategoryId(*c)) {
            Some(x) => format!("category {} ({c})", x.name),
            None => format!("category {c}"),
        },
        Resource::Item(i) => item(i),
        Resource::Measure(m) => measure(m),
        Resource::View(ViewId(v)) => match model.views.get(&ViewId(*v)) {
            Some(x) => format!("view {} ({v})", x.name),
            None => format!("view {v}"),
        },
        Resource::Cell(m, at) => format!("cell {}[{}]", measure(m), cell(model, at)),
    }
}

fn cell(model: &Model, at: &Coordinate) -> String {
    let pairs: Vec<String> = (at.dims.iter())
        .map(|(c, i)| {
            let cat = model
                .categories
                .get(c)
                .map_or(c.0.to_string(), |x| x.name.0.clone());
            let it = model
                .items
                .get(i)
                .map_or(i.0.to_string(), |x| x.name.0.clone());
            format!("{cat}={it}")
        })
        .collect();
    pairs.join(", ")
}

fn check(
    db: &str,
    user: &str,
    roles: Vec<String>,
    action: &str,
    target: &Target,
    file: Option<(String, Option<String>)>,
) -> Result<(), String> {
    let action: Action = action.parse()?;
    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;
    let policies = match file {
        Some((f, g)) => Some(parse(&model, &f, g.as_deref())?),
        None => stored(&mut store)?,
    };
    let r = resource(&model, target)?;
    let who = Principal {
        user: user.into(),
        roles,
    };
    let session = Session::new(policies.map(Arc::new), &model, &who, Channel::Local);
    let steps = session.explain(action, &r)?;
    let allow = steps.iter().all(|s| s.decision.allow);
    println!(
        "{}: {user} {action} {}",
        if allow { "allow" } else { "deny" },
        describe(&model, &r)
    );
    if !session.restricted() {
        println!("  the model has no policies: everything is allowed");
    }
    for s in steps.iter().filter(|_| session.restricted()) {
        let by = match (s.decision.allow, s.decision.policies.is_empty()) {
            (_, false) => format!("by {}", s.decision.policies.join(", ")),
            (true, true) => String::new(),
            (false, true) => "no policy permits it".into(),
        };
        let mark = if s.decision.allow { "allow" } else { "deny " };
        println!(
            "  {mark} {} {} {by}",
            s.action,
            describe(&model, &s.resource)
        );
        for e in &s.decision.errors {
            println!("        {e}");
        }
    }
    if !allow {
        // The decision is the answer, not a failure of the command; exit 1
        // so scripts can test it, without an error line.
        std::process::exit(1);
    }
    Ok(())
}

fn template(
    db: &str,
    name: &str,
    user: Option<String>,
    group: Option<String>,
    measures: &[String],
    items: &[String],
) -> Result<(), String> {
    let model = open(db)?.load_model().map_err(|e| e.to_string())?;
    let who = match (user, group) {
        (Some(u), _) => Who::User(u),
        (None, Some(g)) => Who::Group(g),
        (None, None) => return Err("give --user or --group".into()),
    };
    let measures = (measures.iter())
        .map(|m| resolve_measure(&model, m))
        .collect::<Result<Vec<_>, _>>()?;
    if name == "editor" {
        if let Some(m) = measures.iter().find(|m| !model.measures[m].is_input()) {
            return Err(format!(
                "measure {} is derived: its cells are computed, not edited",
                model.measures[m].name
            ));
        }
    }
    let items = (items.iter())
        .map(|i| item(&model, i))
        .collect::<Result<Vec<_>, _>>()?;
    let text = templates::render(name, &who, &measures, &items)?;
    // Name what the ids are, for whoever edits the text.
    let mut notes: Vec<String> = (measures.iter())
        .map(|m| format!("// measure {} = {}", m.0, model.measures[m].name))
        .collect();
    notes.extend(
        items
            .iter()
            .map(|i| format!("// item {} = {}", i.0, model.items[i].name)),
    );
    for n in notes {
        println!("{n}");
    }
    print!("{text}");
    Ok(())
}

fn lint(db: &str, user: &str, roles: Vec<String>) -> Result<(), String> {
    let mut store = open(db)?;
    let model = store.load_model().map_err(|e| e.to_string())?;
    let Some(policies) = stored(&mut store)? else {
        println!("no policies: nothing is hidden");
        return Ok(());
    };
    let who = Principal {
        user: user.into(),
        roles,
    };
    let session = Session::new(Some(Arc::new(policies)), &model, &who, Channel::Local);
    let found = session.lint();
    if found.is_empty() {
        println!("{user} can infer nothing hidden through the measures they can read");
    }
    for (m, why) in found {
        let what = describe(&model, &Resource::Measure(m));
        match why {
            Reveals::Measure(h) => println!(
                "{what} reads {}, hidden from {user}",
                describe(&model, &Resource::Measure(h))
            ),
            Reveals::Items(c, items) => {
                let names: Vec<String> = (items.iter())
                    .map(|i| {
                        model
                            .items
                            .get(i)
                            .map_or(i.0.to_string(), |x| x.name.0.clone())
                    })
                    .collect();
                println!(
                    "{what} totals over {}, including {}, hidden from {user}: \
                     the total less the visible items is their sum",
                    describe(&model, &Resource::Category(c)),
                    names.join(", ")
                );
            }
        }
    }
    Ok(())
}
