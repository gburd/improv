//! A model's policy document: Cedar policy text and the model's groups,
//! parsed and validated against [`crate::SCHEMA`].

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::str::FromStr;
use std::sync::OnceLock;

use cedar_policy::{EntityUid, PolicyId, PolicySet, Schema, ValidationMode, Validator};
use improv_core_model::{CategoryId, ItemId, MeasureId, Model, ViewId};
use serde::{Deserialize, Serialize};

/// The most bytes of policy text a document may hold.
pub const MAX_TEXT: usize = 1 << 20;
/// The most policies a document may hold.
pub const MAX_POLICIES: usize = 10_000;
/// The most group memberships (users and groups listed) a document may hold.
pub const MAX_GROUPS: usize = 100_000;

/// The schema, parsed once.
pub(crate) fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        Schema::from_cedarschema_str(crate::SCHEMA)
            .map(|(s, _)| s)
            .expect("the built-in schema parses")
    })
}

/// One problem with a document: which policy (its `@id`, or `policyN`, the
/// Nth from 0), the line it is on, and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub policy: Option<String>,
    pub line: Option<usize>,
    pub message: String,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(line) = self.line {
            write!(f, "line {line}: ")?;
        }
        if let Some(p) = &self.policy {
            write!(f, "policy {p}: ")?;
        }
        f.write_str(&self.message)
    }
}

/// A document that cannot be used: every error found.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}", .0.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n"))]
pub struct Invalid(pub Vec<Diagnostic>);

impl Invalid {
    fn one(message: impl Into<String>) -> Self {
        Invalid(vec![Diagnostic {
            policy: None,
            line: None,
            message: message.into(),
        }])
    }
}

/// The model's groups: each group's member users and member groups.
/// Stored as JSON, `{"analysts": {"users": ["alice"], "groups": ["interns"]}}`.
/// Groups nest; a cycle is allowed (its groups then contain each other).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Groups(pub BTreeMap<String, Members>);

/// One group's members.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Members {
    #[serde(default)]
    pub users: Vec<String>,
    #[serde(default)]
    pub groups: Vec<String>,
}

impl Groups {
    /// Parse and check the JSON: every member group is defined, no name is
    /// empty, and the whole is within [`MAX_GROUPS`]. Empty text is no
    /// groups.
    pub fn parse(json: &str) -> Result<Groups, Invalid> {
        if json.trim().is_empty() {
            return Ok(Groups::default());
        }
        let groups: Groups =
            serde_json::from_str(json).map_err(|e| Invalid::one(format!("groups: {e}")))?;
        let size: usize = groups
            .0
            .values()
            .map(|m| m.users.len() + m.groups.len())
            .sum();
        if size + groups.0.len() > MAX_GROUPS {
            return Err(Invalid::one(format!(
                "groups: more than {MAX_GROUPS} groups and members"
            )));
        }
        let mut errors = Vec::new();
        for (name, m) in &groups.0 {
            if name.is_empty() || m.users.iter().any(String::is_empty) {
                errors.push(format!(
                    "groups: group '{name}' has an empty name or member"
                ));
            }
            for g in m.groups.iter().filter(|g| !groups.0.contains_key(*g)) {
                errors.push(format!(
                    "groups: group '{name}' lists group '{g}', which is not defined"
                ));
            }
        }
        match errors.is_empty() {
            true => Ok(groups),
            false => Err(Invalid(
                errors
                    .into_iter()
                    .map(|message| Diagnostic {
                        policy: None,
                        line: None,
                        message,
                    })
                    .collect(),
            )),
        }
    }

    /// Every group `user` is in, directly or through nested groups.
    pub fn of(&self, user: &str) -> BTreeSet<String> {
        let mut found: BTreeSet<String> = (self.0.iter())
            .filter(|(_, m)| m.users.iter().any(|u| u == user))
            .map(|(g, _)| g.clone())
            .collect();
        let mut todo: Vec<String> = found.iter().cloned().collect();
        while let Some(g) = todo.pop() {
            for (parent, m) in &self.0 {
                if m.groups.contains(&g) && found.insert(parent.clone()) {
                    todo.push(parent.clone());
                }
            }
        }
        found
    }
}

/// A model's policies, valid against the schema. With none stored a model
/// is unrestricted; with these, everything they do not permit is denied.
#[derive(Debug, Clone)]
pub struct Policies {
    text: String,
    groups: Groups,
    pub(crate) set: PolicySet,
    /// Every entity some policy names, as (type, id): the decision cache's
    /// classes (see [`crate::Session`]).
    pub(crate) named: HashSet<(String, String)>,
    warnings: Vec<Diagnostic>,
}

impl Policies {
    /// Parse `text` (Cedar policies) and `groups` (JSON, see [`Groups`]),
    /// and validate the policies strictly against the schema. Refused, with
    /// every reason: a syntax or type error, a template (a policy with
    /// `?principal` or `?resource`), a policy naming an `Improv::Cell`, two
    /// policies with one `@id`, no policies at all, or past the limits.
    /// Validator warnings (a policy that can never apply) are kept, in
    /// [`Policies::warnings`].
    pub fn parse(text: &str, groups: &str) -> Result<Policies, Invalid> {
        if text.len() > MAX_TEXT {
            return Err(Invalid::one(format!(
                "the policies are larger than {MAX_TEXT} bytes"
            )));
        }
        let groups = Groups::parse(groups)?;
        let parsed = PolicySet::from_str(text).map_err(|e| {
            Invalid(
                e.errors_as_strings_with_lines(text)
                    .into_iter()
                    .map(|(line, message)| Diagnostic {
                        policy: None,
                        line,
                        message,
                    })
                    .collect(),
            )
        })?;
        if parsed.num_of_templates() > 0 {
            return Err(Invalid::one(
                "a policy has a slot (?principal or ?resource): store linked policies, not templates",
            ));
        }
        let n = parsed.num_of_policies();
        if n == 0 {
            return Err(Invalid::one(
                "there are no policies (to remove a model's policies, clear them)",
            ));
        }
        if n > MAX_POLICIES {
            return Err(Invalid::one(format!("more than {MAX_POLICIES} policies")));
        }
        let set = renamed(&parsed, text)?;
        let mut errors = Vec::new();
        let mut named = HashSet::new();
        for p in set.policies() {
            for uid in p.entity_literals() {
                let ty = uid.type_name().to_string();
                if ty == "Improv::Cell" {
                    errors.push(Diagnostic {
                        policy: Some(p.id().to_string()),
                        line: line_of(text, p),
                        message: "names an Improv::Cell; name its measure or items instead".into(),
                    });
                }
                named.insert((ty, uid.id().unescaped().to_string()));
            }
        }
        let result = Validator::new(schema().clone()).validate(&set, ValidationMode::Strict);
        let at = |d: &dyn miette::Diagnostic, msg: String| Diagnostic {
            policy: None,
            line: d
                .labels()
                .and_then(|mut l| l.next())
                .map(|l| line_at(text, l.offset())),
            message: msg,
        };
        errors.extend(result.validation_errors().map(|e| at(e, e.to_string())));
        if !errors.is_empty() {
            return Err(Invalid(errors));
        }
        let warnings = result
            .validation_warnings()
            .map(|w| at(w, w.to_string()))
            .collect();
        Ok(Policies {
            text: text.to_string(),
            groups,
            set,
            named,
            warnings,
        })
    }

    /// The policy text, as given.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The groups.
    pub fn groups(&self) -> &Groups {
        &self.groups
    }

    /// The validator's warnings (a policy that never applies, say).
    pub fn warnings(&self) -> &[Diagnostic] {
        &self.warnings
    }

    /// How many policies.
    pub fn len(&self) -> usize {
        self.set.num_of_policies()
    }

    /// Never true: a document holds at least one policy.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The ids of the policies, in text order where they have no `@id`.
    pub fn ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.set.policies().map(|p| p.id().to_string()).collect();
        ids.sort();
        ids
    }

    /// What the policies name that `model` does not have: a measure,
    /// category, item or view id it lacks, or a model other than
    /// [`crate::MODEL_ID`]. Such a policy matches nothing today; one
    /// written by name (`"Revenue"` for `"101"`) is a common mistake.
    pub fn unknown_names(&self, model: &Model) -> Vec<String> {
        let id = |s: &str| s.parse::<u32>().ok();
        let mut out: Vec<String> = (self.named.iter())
            .filter(|(ty, s)| match ty.as_str() {
                "Improv::Model" => s != crate::MODEL_ID,
                "Improv::Measure" => {
                    id(s).is_none_or(|n| !model.measures.contains_key(&MeasureId(n)))
                }
                "Improv::Category" => {
                    id(s).is_none_or(|n| !model.categories.contains_key(&CategoryId(n)))
                }
                "Improv::Item" => id(s).is_none_or(|n| !model.items.contains_key(&ItemId(n))),
                "Improv::View" => id(s).is_none_or(|n| !model.views.contains_key(&ViewId(n))),
                _ => false,
            })
            .map(|(ty, s)| format!("{ty}::{s:?}"))
            .collect();
        out.sort();
        out
    }
}

/// `set` with each policy's id taken from its `@id("…")` annotation, when
/// it has one, so decisions name policies as their authors did.
fn renamed(set: &PolicySet, text: &str) -> Result<PolicySet, Invalid> {
    let mut policies: Vec<_> = set.policies().collect();
    // Text order, so a document's policyN ids follow it.
    let nth = |p: &cedar_policy::Policy| {
        let id = p.id().to_string();
        id.strip_prefix("policy")
            .and_then(|n| n.parse::<usize>().ok())
    };
    policies.sort_by_key(|p| nth(p));
    let mut out = PolicySet::new();
    let mut seen = HashSet::new();
    let mut errors = Vec::new();
    for p in policies {
        let id = p
            .annotation("id")
            .map_or_else(|| p.id().to_string(), str::to_string);
        if !seen.insert(id.clone()) {
            errors.push(Diagnostic {
                policy: Some(id.clone()),
                line: line_of(text, p),
                message: "another policy has this id".into(),
            });
            continue;
        }
        if out.add(p.new_id(PolicyId::new(&id))).is_err() {
            errors.push(Diagnostic {
                policy: Some(id),
                line: line_of(text, p),
                message: "could not add the policy".into(),
            });
        }
    }
    match errors.is_empty() {
        true => Ok(out),
        false => Err(Invalid(errors)),
    }
}

/// The 1-based line of byte `offset` in `text`.
fn line_at(text: &str, offset: usize) -> usize {
    text.as_bytes()[..offset.min(text.len())]
        .iter()
        .filter(|b| **b == b'\n')
        .count()
        + 1
}

/// The line policy `p` starts on.
// shortcut: finds the policy's first line in the text, so of two identical
// policies both report the first; use Cedar's source spans if it exposes them.
fn line_of(text: &str, p: &cedar_policy::Policy) -> Option<usize> {
    let src = p.to_string();
    let head = src.lines().next()?;
    text.find(head).map(|at| line_at(text, at))
}

/// Parse errors as (line, message) pairs.
trait WithLines {
    fn errors_as_strings_with_lines(&self, text: &str) -> Vec<(Option<usize>, String)>;
}

impl WithLines for cedar_policy::ParseErrors {
    fn errors_as_strings_with_lines(&self, text: &str) -> Vec<(Option<usize>, String)> {
        self.iter()
            .map(|e| {
                let line = miette::Diagnostic::labels(e)
                    .and_then(|mut l| l.next())
                    .map(|l| line_at(text, l.offset()));
                (line, e.to_string())
            })
            .collect()
    }
}

/// The uid of a Cedar entity of `ty` (`"Measure"`) with `id`.
pub(crate) fn uid(ty: &str, id: &str) -> EntityUid {
    let ty = cedar_policy::EntityTypeName::from_str(&format!("Improv::{ty}"))
        .expect("schema type names parse");
    EntityUid::from_type_name_and_id(ty, cedar_policy::EntityId::new(id))
}
