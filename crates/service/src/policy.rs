//! The one place an access decision is made. Every request the authority
//! serves asks [`Policy::allows`]; today's [`Open`] says yes to everything.
//! Cedar policies (`docs/reviews/2026-11-02-cedar-policies.md`) plug in
//! here: an implementation over `improv_policy` replaces `Open` and nothing
//! else changes.
//!
//! The composite rules are the contract's: a cell is visible when its
//! measure, every item of its coordinate and the cell are readable; it is
//! editable when it is visible, its measure is an input and EditCell
//! allows it. [`Gate`] applies them, so no caller can skip a step.

use improv_core_model::{CategoryId, ItemId, MeasureId, Model};

/// Who is asking: the authenticated identity's name (a token's name, or
/// `anonymous` in open mode).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Principal(pub String);

/// What is being asked, named as the Cedar schema names its actions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    ViewModel,
    ReadMeasure,
    ReadItem,
    ReadCell,
    EditCell,
    DefineMeasure,
    EditStructure,
    ManageViews,
}

/// What it is asked of.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Resource {
    Model,
    Measure(MeasureId),
    Category(CategoryId),
    Item(ItemId),
    View(u32),
    /// A cell: its measure and its coordinate's items.
    Cell(MeasureId, Vec<(CategoryId, ItemId)>),
}

/// An access decision procedure.
pub trait Policy: Send + Sync {
    /// Whether `who` may do `action` to `what` in `model`.
    fn allows(&self, model: &Model, who: &Principal, action: Action, what: &Resource) -> bool;
}

/// No policy: everyone may do everything (Improv 0.12's behavior, and a
/// model with no stored policies under Cedar).
#[derive(Debug, Clone, Copy, Default)]
pub struct Open;

impl Policy for Open {
    fn allows(&self, _: &Model, _: &Principal, _: Action, _: &Resource) -> bool {
        true
    }
}

/// A policy bound to one model and one principal, with the contract's
/// composite rules.
pub struct Gate<'a> {
    pub policy: &'a dyn Policy,
    pub model: &'a Model,
    pub who: &'a Principal,
}

impl Gate<'_> {
    pub fn allows(&self, action: Action, what: &Resource) -> bool {
        self.policy.allows(self.model, self.who, action, what)
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

    /// A cell of `m` at `at` is visible: its measure, every item and the
    /// cell are readable.
    pub fn sees_cell(&self, m: MeasureId, at: &[(CategoryId, ItemId)]) -> bool {
        self.sees_measure(m)
            && at.iter().all(|(_, i)| self.sees_item(*i))
            && self.allows(Action::ReadCell, &Resource::Cell(m, at.to_vec()))
    }

    /// A cell may be set: visible, an input, and EditCell allows it.
    pub fn edits_cell(&self, m: MeasureId, at: &[(CategoryId, ItemId)]) -> bool {
        self.sees_cell(m, at)
            && self.model.measures.get(&m).is_some_and(|x| x.is_input())
            && self.allows(Action::EditCell, &Resource::Cell(m, at.to_vec()))
    }

    /// A formula is shown when its measure and every measure it reads are
    /// readable.
    pub fn sees_formula(&self, m: MeasureId) -> bool {
        let Some(x) = self.model.measures.get(&m) else {
            return false;
        };
        let reads = match &x.kind {
            improv_core_model::MeasureKind::Derived(f) => f.referenced_measures(),
            improv_core_model::MeasureKind::Input => Vec::new(),
        };
        self.sees_measure(m) && reads.iter().all(|r| self.sees_measure(*r))
    }
}

/// A stand-in policy for tests until Cedar's lands: everything is allowed
/// except the `(principal, action, resource)` triples it lists.
#[derive(Default)]
pub struct Deny(pub Vec<(String, Action, Resource)>);

impl Policy for Deny {
    fn allows(&self, _: &Model, who: &Principal, a: Action, r: &Resource) -> bool {
        !self
            .0
            .iter()
            .any(|(w, x, y)| *w == who.0 && *x == a && y == r)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_composite_rules_need_every_part() {
        let m = crate::tests::revenue_model();
        let who = Principal("ann".into());
        let open = Gate {
            policy: &Open,
            model: &m,
            who: &who,
        };
        let at = [(CategoryId(1), ItemId(10)), (CategoryId(2), ItemId(20))];
        assert!(open.sees_model() && open.sees_cell(MeasureId(101), &at));
        assert!(open.edits_cell(MeasureId(101), &at));
        assert!(!open.edits_cell(MeasureId(102), &at), "derived");
        assert!(!open.sees_measure(MeasureId(999)) && !open.sees_item(ItemId(999)));
        assert!(open.sees_formula(MeasureId(102)) && !open.sees_formula(MeasureId(999)));
        let deny = |a, r| Deny(vec![("ann".into(), a, r)]);
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
            (
                deny(
                    Action::ReadCell,
                    Resource::Cell(MeasureId(101), at.to_vec()),
                ),
                false,
                false,
            ),
            (
                deny(
                    Action::EditCell,
                    Resource::Cell(MeasureId(101), at.to_vec()),
                ),
                true,
                false,
            ),
        ] {
            let g = Gate {
                policy: &p,
                model: &m,
                who: &who,
            };
            assert_eq!(g.sees_cell(MeasureId(101), &at), sees);
            assert_eq!(g.edits_cell(MeasureId(101), &at), edits);
        }
        let p = deny(Action::ReadMeasure, Resource::Measure(MeasureId(100)));
        let g = Gate {
            policy: &p,
            model: &m,
            who: &who,
        };
        assert!(!g.sees_formula(MeasureId(102)), "it reads a hidden measure");
        let bob = Principal("bob".into());
        let g = Gate {
            policy: &p,
            model: &m,
            who: &bob,
        };
        assert!(g.sees_formula(MeasureId(102)), "the rule is ann's");
    }
}
