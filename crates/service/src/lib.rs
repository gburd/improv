//! The authority a GUI drives ([`authority::Authority`]): one model, its
//! live engine, and every `/v1` operation, each checked by a
//! [`policy::Policy`] and answered with only what the caller may see.
//! `improv-server` serves it over HTTP. [`local::Local`] is the
//! [`ModelService`](improv_client::wire::ModelService) over it, in process,
//! for a front end that holds the model itself.
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
// The attribute is only used inside #[cfg(test)] modules, so non-test
// coverage builds would otherwise warn that the feature is unused.
#![cfg_attr(coverage_nightly, allow(unused_features))]

pub mod authority;
pub mod cedar;
pub mod local;
pub mod policy;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
