//! [`CedarPolicy`]: the [`Policy`] backed by the model's stored Cedar
//! policies (`improv policy set`, `improv_policy`).
//!
//! Its sessions are `improv_policy::Session`s, kept per (policy version,
//! structure version, principal). The policy version is read from the
//! store on every [`Policy::session`] call (one indexed query), so a set,
//! a clear or a change of groups by any writer, this process or another,
//! is seen by the next request: nothing has to remember to invalidate.

use std::sync::{Arc, Mutex, PoisonError};

use improv_core_model::Model;
use improv_policy::{Channel, Policies, Session, SessionKey, Sessions};
use improv_store::Store;

use crate::policy::{Access, Action, Policy, Principal, Resource};

/// Sessions kept at once (the design note's 1,024), least recently used
/// dropped first.
pub const SESSIONS: usize = 1024;

impl Access for Session {
    fn allowed(&self, action: Action, what: &Resource) -> bool {
        match (action, what) {
            // The cell path, without the Resource's clone of the coordinate.
            (Action::ReadCell | Action::EditCell, Resource::Cell(m, at)) => {
                Session::cell(self, action, *m, at)
            }
            _ => Session::allowed(self, action, what),
        }
    }

    fn restricted(&self) -> bool {
        Session::restricted(self)
    }
}

/// Fails closed: everything is denied. A session made while the stored
/// policies cannot be read or do not validate.
struct Closed;

impl Access for Closed {
    fn allowed(&self, _: Action, _: &Resource) -> bool {
        false
    }
    fn restricted(&self) -> bool {
        true
    }
}

/// What the stored policies were at their version.
struct Loaded {
    version: i64,
    policies: Option<Arc<Policies>>,
}

/// The model's stored policies, as a [`Policy`].
pub struct CedarPolicy {
    target: String,
    channel: Channel,
    store: Mutex<Option<Store>>,
    loaded: Mutex<Option<Loaded>>,
    sessions: Sessions,
}

impl CedarPolicy {
    /// The policies stored with the model at `target` (a store path or a
    /// `postgres://` URL), for requests checked at `channel`.
    pub fn new(target: &str, channel: Channel) -> Self {
        CedarPolicy {
            target: target.to_owned(),
            channel,
            store: Mutex::new(None),
            loaded: Mutex::new(None),
            sessions: Sessions::new(SESSIONS),
        }
    }

    /// The stored policies' version now, and the policies at it: read
    /// again only when the version moved. `Err` when they cannot be read,
    /// or no longer validate (an error a request must fail closed on).
    fn current(&self) -> Result<(i64, Option<Arc<Policies>>), String> {
        let mut store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        if store.is_none() {
            *store = Some(Store::open(&self.target).map_err(|e| e.to_string())?);
        }
        let s = store.as_mut().expect("opened above");
        let version = match s.policy_version() {
            Ok(v) => v,
            Err(e) => {
                *store = None;
                return Err(e.to_string());
            }
        };
        let mut loaded = self.loaded.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(l) = loaded.as_ref().filter(|l| l.version == version) {
            return Ok((version, l.policies.clone()));
        }
        let policies = match s.load_policies().map_err(|e| e.to_string())? {
            None => None,
            Some(p) => Some(Arc::new(
                Policies::parse(&p.text, &p.groups).map_err(|e| e.to_string())?,
            )),
        };
        *loaded = Some(Loaded {
            version,
            policies: policies.clone(),
        });
        Ok((version, policies))
    }
}

impl Policy for CedarPolicy {
    fn session(&self, model: &Model, structure: u64, who: &Principal) -> Arc<dyn Access> {
        let (version, policies) = match self.current() {
            Ok(v) => v,
            Err(e) => {
                eprintln!("policies of {}: {e}; every request is refused", self.target);
                return Arc::new(Closed);
            }
        };
        let key = SessionKey {
            policy_version: version,
            model_version: structure as i64,
            principal: who.clone(),
            channel: self.channel,
        };
        self.sessions.get(&key, policies.as_ref(), model)
    }
}
