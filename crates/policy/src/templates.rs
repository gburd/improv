//! Policy text for the common roles: `improv policy template` prints it, to
//! store as it is or edit first. Each grants to one user or one group, and
//! each policy's `@id` names the template and the principal, so several
//! templates combine in one document.

use improv_core_model::{ItemId, MeasureId};

/// Whom a template's policies apply to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    User(String),
    Group(String),
}

/// The templates, as `improv policy template` names them.
pub const NAMES: [&str; 5] = ["viewer", "editor", "analyst", "hide-measures", "admin"];

/// A Cedar string literal: `"` and `\` escaped, and the control characters
/// Cedar's escapes cover.
fn quoted(s: &str) -> String {
    let mut out = String::from('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\0' => out.push_str("\\0"),
            c if c.is_control() => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

impl Who {
    /// The policy scope's principal clause.
    fn scope(&self) -> String {
        match self {
            Who::User(u) => format!("principal == Improv::User::{}", quoted(u)),
            Who::Group(g) => format!("principal in Improv::Group::{}", quoted(g)),
        }
    }

    /// `user:alice` or `group:analysts`, for policy ids.
    fn tag(&self) -> String {
        match self {
            Who::User(u) => format!("user:{u}"),
            Who::Group(g) => format!("group:{g}"),
        }
    }
}

fn list(ty: &str, ids: impl Iterator<Item = u32>) -> String {
    let ids: Vec<String> = ids.map(|i| format!("Improv::{ty}::\"{i}\"")).collect();
    format!("[{}]", ids.join(", "))
}

fn policy(id: &str, who: &Who, effect: &str, action: &str, when: Option<String>) -> String {
    let when = when.map_or_else(String::new, |w| format!("\nwhen {{ {w} }}"));
    format!(
        "@id({})\n{effect} (\n  {},\n  {action},\n  resource\n){when};\n",
        quoted(&format!("{id}:{}", who.tag())),
        who.scope()
    )
}

/// The viewer's policy: see everything.
fn viewer(who: &Who) -> String {
    policy(
        "viewer",
        who,
        "permit",
        "action in Improv::Action::\"Read\"",
        None,
    )
}

/// Template `name`'s policy text for `who`. `measures` and `items` are what
/// `editor` (the input measures it may edit), `analyst` (the items it may
/// not see) and `hide-measures` (the measures it may not see) take; the
/// others take none, and those three need at least one.
pub fn render(
    name: &str,
    who: &Who,
    measures: &[MeasureId],
    items: &[ItemId],
) -> Result<String, String> {
    let (takes_measures, takes_items) = match name {
        "viewer" | "admin" => (false, false),
        "editor" | "hide-measures" => (true, false),
        "analyst" => (false, true),
        _ => {
            return Err(format!(
                "unknown template '{name}'; the templates are {}",
                NAMES.join(", ")
            ))
        }
    };
    if takes_measures != !measures.is_empty() {
        return Err(match takes_measures {
            true => format!("template {name} needs at least one --measure"),
            false => format!("template {name} takes no --measure"),
        });
    }
    if takes_items != !items.is_empty() {
        return Err(match takes_items {
            true => format!("template {name} needs at least one --item"),
            false => format!("template {name} takes no --item"),
        });
    }
    let measures = || list("Measure", measures.iter().map(|m| m.0));
    let items = || list("Item", items.iter().map(|i| i.0));
    Ok(match name {
        "viewer" => viewer(who),
        "editor" => {
            let edit = policy(
                "editor",
                who,
                "permit",
                "action == Improv::Action::\"EditCell\"",
                Some(format!("resource in {}", measures())),
            );
            format!("{}\n{edit}", viewer(who))
        }
        // The forbid names the items' cells too, so even a raw question
        // about one of their cells is denied, not only the composite rule.
        "analyst" => {
            let hide = policy(
                "analyst",
                who,
                "forbid",
                "action in [Improv::Action::\"ReadItem\", Improv::Action::\"ReadCell\", \
                 Improv::Action::\"EditCell\"]",
                Some(format!("resource in {}", items())),
            );
            format!("{}\n{hide}", viewer(who))
        }
        "hide-measures" => policy(
            "hide-measures",
            who,
            "forbid",
            "action in [Improv::Action::\"ReadMeasure\", Improv::Action::\"ReadCell\", \
             Improv::Action::\"EditCell\", Improv::Action::\"DefineMeasure\"]",
            Some(format!("resource in {}", measures())),
        ),
        _ => policy("admin", who, "permit", "action", None),
    })
}
