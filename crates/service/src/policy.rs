//! The one place an access decision is made. The authority asks a
//! [`Policy`] for a principal's [`Access`] once per request (its decisions
//! over the model's current structure), then asks that for every measure,
//! item and cell it touches. [`Open`] allows everything. Cedar policies
//! (`docs/reviews/2026-11-02-cedar-policies.md`, `improv_policy`) plug in
//! here: `improv_policy::Session` is an `Access` (its `allowed` and
//! `restricted`), and `improv_policy::Sessions` keyed by policy version,
//! structure version and principal is the `Policy`. The names below are
//! that crate's, one to one.
//!
//! **Cost per cell.** An `Access` answers with the contract's composite
//! rules (a cell is visible when its measure, each of its items and the
//! cell are readable) from a cache by class (a measure and the
//! policy-named items of a coordinate), so a cell check is a few hash
//! lookups. And when [`Access::restricted`] is false (a model without
//! policies) the authority skips the per-cell checks altogether.

use std::sync::Arc;

use improv_core_model::{CategoryId, Coordinate, ItemId, MeasureId, MeasureKind, Model, ViewId};

/// Who asks: an authenticated user and the roles its identity source
/// asserts (a token's name has none yet).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Principal {
    pub user: String,
    pub roles: Vec<String>,
}

impl Principal {
    /// A user with no roles.
    pub fn user(name: impl Into<String>) -> Self {
        Principal {
            user: name.into(),
            roles: Vec::new(),
        }
    }
}

/// What a principal asks to do: the Cedar schema's actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    ViewModel,
    ReadMeasure,
    ReadItem,
    ReadCell,
    EditCell,
    DefineMeasure,
    EditStructure,
    ManageViews,
    Admin,
}

/// What an action is on. `Model` is the model as a whole: ViewModel, and
/// adding a measure, category, item or view.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Resource {
    Model,
    Category(CategoryId),
    Item(ItemId),
    Measure(MeasureId),
    View(ViewId),
    /// One cell: a measure and an item for each of its categories.
    Cell(MeasureId, Coordinate),
}

/// Where decisions for a principal come from.
pub trait Policy: Send + Sync {
    /// `who`'s decisions over `model`. `structure` names the model's
    /// structure (it changes when a category, item, measure or view does,
    /// not when a cell does), so an implementation can keep one per
    /// (policy version, structure, principal) and reuse it.
    fn session(&self, model: &Model, structure: u64, who: &Principal) -> Arc<dyn Access>;
}

/// One principal's decisions over one model structure.
pub trait Access: Send + Sync {
    /// Whether `action` on `what` is allowed, by the composite rules: every
    /// action needs ViewModel; ReadCell and EditCell need the cell's
    /// measure and every item of its coordinate readable, and EditCell an
    /// input measure; DefineMeasure on a measure needs it readable,
    /// EditStructure on an item the item. False for what is not in the
    /// model.
    fn allowed(&self, action: Action, what: &Resource) -> bool;
    /// Whether anything is restricted; when false every check is skipped.
    fn restricted(&self) -> bool;
}

/// No policy: everyone may do everything (Improv 0.12's behavior, and a
/// model with no stored policies under Cedar).
#[derive(Debug, Clone, Copy, Default)]
pub struct Open;

impl Policy for Open {
    fn session(&self, _: &Model, _: u64, _: &Principal) -> Arc<dyn Access> {
        Arc::new(Open)
    }
}

impl Access for Open {
    fn allowed(&self, _: Action, _: &Resource) -> bool {
        true
    }
    fn restricted(&self) -> bool {
        false
    }
}

/// An [`Access`] over a model, with the questions the authority asks.
pub struct Gate<'a> {
    pub access: &'a dyn Access,
    pub model: &'a Model,
}

impl Gate<'_> {
    fn open(&self) -> bool {
        !self.access.restricted()
    }

    pub fn allows(&self, action: Action, what: &Resource) -> bool {
        self.open() || self.access.allowed(action, what)
    }

    /// Opening the model at all.
    pub fn sees_model(&self) -> bool {
        self.allows(Action::ViewModel, &Resource::Model)
    }

    pub fn sees_measure(&self, m: MeasureId) -> bool {
        self.model.measures.contains_key(&m)
            && self.allows(Action::ReadMeasure, &Resource::Measure(m))
    }

    pub fn sees_item(&self, i: ItemId) -> bool {
        self.model.items.contains_key(&i) && self.allows(Action::ReadItem, &Resource::Item(i))
    }

    /// A cell of `m` at `at` may be shown.
    pub fn sees_cell(&self, m: MeasureId, at: &Coordinate) -> bool {
        self.open()
            || self
                .access
                .allowed(Action::ReadCell, &Resource::Cell(m, at.clone()))
    }

    /// An input cell may be set or cleared.
    pub fn edits_cell(&self, m: MeasureId, at: &Coordinate) -> bool {
        let input = self.model.measures.get(&m).is_some_and(|x| x.is_input());
        input
            && (self.open()
                || self
                    .access
                    .allowed(Action::EditCell, &Resource::Cell(m, at.clone())))
    }

    /// A formula may be shown: its measure and every measure it reads are
    /// readable.
    pub fn sees_formula(&self, m: MeasureId) -> bool {
        let Some(x) = self.model.measures.get(&m) else {
            return false;
        };
        let reads = match &x.kind {
            MeasureKind::Derived(f) => f.referenced_measures(),
            MeasureKind::Input => Vec::new(),
        };
        self.sees_measure(m) && reads.iter().all(|r| self.sees_measure(*r))
    }
}

/// A stand-in policy for tests until Cedar's lands: everything is allowed
/// except the `(user, action, resource)` triples it lists, under the
/// composite rules.
#[derive(Default)]
pub struct Deny(pub Vec<(String, Action, Resource)>);

impl Policy for Deny {
    fn session(&self, model: &Model, _: u64, who: &Principal) -> Arc<dyn Access> {
        let mine = self
            .0
            .iter()
            .filter(|(w, ..)| *w == who.user)
            .map(|(_, a, r)| (*a, r.clone()))
            .collect();
        Arc::new(DenyAccess {
            denied: mine,
            model: model.clone(),
        })
    }
}

struct DenyAccess {
    denied: Vec<(Action, Resource)>,
    model: Model,
}

impl DenyAccess {
    fn raw(&self, a: Action, r: &Resource) -> bool {
        !self.denied.iter().any(|(x, y)| *x == a && y == r)
    }
}

impl Access for DenyAccess {
    fn allowed(&self, action: Action, what: &Resource) -> bool {
        if !self.raw(Action::ViewModel, &Resource::Model) {
            return false;
        }
        match (action, what) {
            (Action::ReadCell | Action::EditCell, Resource::Cell(m, at)) => {
                let input = self.model.measures.get(m).is_some_and(|x| x.is_input());
                (action == Action::ReadCell || input)
                    && self.raw(Action::ReadMeasure, &Resource::Measure(*m))
                    && at
                        .dims
                        .values()
                        .all(|i| self.raw(Action::ReadItem, &Resource::Item(*i)))
                    && self.raw(Action::ReadCell, what)
                    && self.raw(action, what)
            }
            (Action::DefineMeasure, Resource::Measure(m)) => {
                self.raw(Action::ReadMeasure, &Resource::Measure(*m)) && self.raw(action, what)
            }
            _ => self.raw(action, what),
        }
    }
    fn restricted(&self) -> bool {
        !self.denied.is_empty()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;

    fn at(pairs: &[(u32, u32)]) -> Coordinate {
        Coordinate::from_pairs(pairs.iter().map(|(c, i)| (CategoryId(*c), ItemId(*i))))
    }

    #[test]
    fn the_composite_rules_need_every_part() {
        let m = crate::tests::revenue_model();
        let ann = Principal::user("ann");
        let open = Open.session(&m, 0, &ann);
        let g = Gate {
            access: &*open,
            model: &m,
        };
        let a = at(&[(1, 10), (2, 20)]);
        assert!(g.sees_model() && g.sees_cell(MeasureId(101), &a));
        assert!(g.edits_cell(MeasureId(101), &a));
        assert!(!g.edits_cell(MeasureId(102), &a), "derived");
        assert!(!g.sees_measure(MeasureId(999)) && !g.sees_item(ItemId(999)));
        assert!(g.sees_formula(MeasureId(102)) && !g.sees_formula(MeasureId(999)));
        let deny = |a, r| Deny(vec![("ann".into(), a, r)]);
        let cell = Resource::Cell(MeasureId(101), a.clone());
        for (p, sees, edits) in [
            (
                deny(Action::ReadItem, Resource::Item(ItemId(20))),
                false,
                false,
            ),
            (
                deny(Action::ReadMeasure, Resource::Measure(MeasureId(101))),
                false,
                false,
            ),
            (deny(Action::ReadCell, cell.clone()), false, false),
            (deny(Action::EditCell, cell.clone()), true, false),
            (deny(Action::ViewModel, Resource::Model), false, false),
        ] {
            let s = p.session(&m, 0, &ann);
            assert!(s.restricted());
            let g = Gate {
                access: &*s,
                model: &m,
            };
            assert_eq!(g.sees_cell(MeasureId(101), &a), sees);
            assert_eq!(g.edits_cell(MeasureId(101), &a), edits);
        }
        let p = deny(Action::ReadMeasure, Resource::Measure(MeasureId(100)));
        let s = p.session(&m, 0, &ann);
        let g = Gate {
            access: &*s,
            model: &m,
        };
        assert!(!g.sees_formula(MeasureId(102)), "it reads a hidden measure");
        assert!(!g.allows(Action::DefineMeasure, &Resource::Measure(MeasureId(100))));
        let bob = p.session(&m, 0, &Principal::user("bob"));
        assert!(!bob.restricted(), "the rule is ann's");
    }
}
