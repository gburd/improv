//! Access policies for Improv models, in [Cedar](https://www.cedarpolicy.com).
//!
//! One schema ([`SCHEMA`]) and one evaluator, shared by every enforcement
//! point: pg_improv, `improv-server`, and the clients' capability hints.
//! The design, threat model and staged plan are in
//! `docs/reviews/2026-11-02-cedar-policies.md`; the user guide is
//! `docs/src/policies.md`.
//!
//! - [`Policies`]: a model's policy text and groups, parsed and validated
//!   against the schema. No policies means no restriction (0.12's
//!   behavior); with any, the default is deny and a forbid wins.
//! - [`Policies::authorize`]: one Cedar question, uncached.
//! - [`Session`]: one principal's view of one model under one policy
//!   version, with an exact decision cache and the composite rules (a cell
//!   is visible when its measure, its items and the cell are all readable).
//! - [`Sessions`]: sessions kept per policy version, model version and
//!   principal, least recently used dropped first.
//! - [`templates`]: policy text for the common roles.
//!
//! Policies name model objects by id (`Improv::Measure::"101"`), never by
//! name, so a rename cannot move a measure out from under a forbid.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, allow(unused_features))]

mod cache;
mod doc;
mod session;
pub mod templates;

pub use cache::{SessionKey, Sessions};
pub use doc::{Diagnostic, Groups, Invalid, Policies, MAX_GROUPS, MAX_POLICIES, MAX_TEXT};
pub use session::{Check, Decision, Reveals, Session};

use improv_core_model::{CategoryId, Coordinate, ItemId, MeasureId, ViewId};

/// The Cedar schema (Cedar's schema syntax) every policy is validated
/// against.
pub const SCHEMA: &str = include_str!("schema.cedarschema");

/// What a principal asks to do. See the user guide for what each allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    /// Open the model: its existence, category names, view names.
    ViewModel,
    /// A measure's name, type and dimensions (and its formula, when every
    /// measure the formula reads is readable too).
    ReadMeasure,
    /// An item's name and place in its category.
    ReadItem,
    /// A cell's value.
    ReadCell,
    /// Set or clear an input cell.
    EditCell,
    /// Add, redefine, format or drop a measure.
    DefineMeasure,
    /// Add, rename, reorder or remove categories and items.
    EditStructure,
    /// Save, change or drop views.
    ManageViews,
    /// Set the model's policies and groups.
    Admin,
}

impl Action {
    /// Every action, in schema order.
    pub const ALL: [Action; 9] = [
        Action::ViewModel,
        Action::ReadMeasure,
        Action::ReadItem,
        Action::ReadCell,
        Action::EditCell,
        Action::DefineMeasure,
        Action::EditStructure,
        Action::ManageViews,
        Action::Admin,
    ];

    /// Its name in the schema and in policies (`Improv::Action::"ReadCell"`).
    pub fn name(self) -> &'static str {
        match self {
            Action::ViewModel => "ViewModel",
            Action::ReadMeasure => "ReadMeasure",
            Action::ReadItem => "ReadItem",
            Action::ReadCell => "ReadCell",
            Action::EditCell => "EditCell",
            Action::DefineMeasure => "DefineMeasure",
            Action::EditStructure => "EditStructure",
            Action::ManageViews => "ManageViews",
            Action::Admin => "Admin",
        }
    }
}

impl std::str::FromStr for Action {
    type Err = String;
    /// An action by its name, any case.
    fn from_str(s: &str) -> Result<Self, String> {
        Action::ALL
            .into_iter()
            .find(|a| a.name().eq_ignore_ascii_case(s))
            .ok_or_else(|| {
                let names: Vec<&str> = Action::ALL.iter().map(|a| a.name()).collect();
                format!("unknown action '{s}'; the actions are {}", names.join(", "))
            })
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// What an action is on. `Model` is the model as a whole: ViewModel, Admin,
/// and adding a measure, category, item or view.
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

/// Who asks: an authenticated user and the roles its identity source
/// asserts (every role it has, transitively: Postgres's role memberships, a
/// token's claims). Its groups come from the model's [`Groups`].
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

/// Where a request is checked; policies see it as `context.channel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    /// The CLI's `policy check`, a local file.
    Local,
    /// pg_improv, in the database.
    Postgres,
    /// `improv-server`.
    Server,
}

impl Channel {
    /// `context.channel`'s value.
    pub fn name(self) -> &'static str {
        match self {
            Channel::Local => "local",
            Channel::Postgres => "postgres",
            Channel::Server => "server",
        }
    }
}

/// The Cedar entity id of the model, of every policy's `Improv::Model`.
pub const MODEL_ID: &str = "model";

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
