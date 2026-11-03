//! [`Sessions`]: sessions kept per policy version, model version, principal
//! and channel, so a server builds one per user per change, not per request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use improv_core_model::Model;

use crate::{Channel, Policies, Principal, Session};

/// A session's key: a new policy version or model version is a new
/// session, so a cached decision never outlives what it was made from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionKey {
    /// The stored policies' version (0: none).
    pub policy_version: i64,
    /// The model's version (the store's high-water).
    pub model_version: i64,
    pub principal: Principal,
    pub channel: Channel,
}

/// At most `capacity` sessions, the least recently used dropped first.
pub struct Sessions {
    capacity: usize,
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    tick: u64,
    map: HashMap<SessionKey, (u64, Arc<Session>)>,
}

impl Sessions {
    /// Room for `capacity` sessions (at least one).
    pub fn new(capacity: usize) -> Sessions {
        Sessions {
            capacity: capacity.max(1),
            inner: Mutex::new(Inner::default()),
        }
    }

    /// The session for `key`, built from `policies` and `model` when it is
    /// not kept. The caller passes the policies and model that `key`'s
    /// versions name.
    pub fn get(
        &self,
        key: &SessionKey,
        policies: Option<&Arc<Policies>>,
        model: &Model,
    ) -> Arc<Session> {
        let mut inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.tick += 1;
        let tick = inner.tick;
        if let Some((used, s)) = inner.map.get_mut(key) {
            *used = tick;
            return s.clone();
        }
        // shortcut: an O(n) scan per eviction; fine at the server's 1,024.
        if inner.map.len() >= self.capacity {
            let oldest = (inner.map.iter())
                .min_by_key(|(_, (used, _))| *used)
                .map(|(k, _)| k.clone());
            if let Some(k) = oldest {
                inner.map.remove(&k);
            }
        }
        let s = Arc::new(Session::new(
            policies.cloned(),
            model,
            &key.principal,
            key.channel,
        ));
        inner.map.insert(key.clone(), (tick, s.clone()));
        s
    }

    /// How many sessions are kept.
    pub fn len(&self) -> usize {
        let inner = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        inner.map.len()
    }

    /// Whether none are kept.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
