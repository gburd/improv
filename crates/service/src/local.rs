//! The [`ModelService`] in process: an [`Authority`] the caller owns, for a
//! front end on a local, single-user model. Replies are ready at once.

use improv_client::wire::{Event, ModelService, Request};

use crate::authority::Authority;
use crate::policy::Principal;

/// A model service over an authority in this process.
pub struct Local {
    pub authority: Authority,
    who: Principal,
    next: u64,
    seen: String,
    out: Vec<Event>,
}

impl Local {
    pub fn new(authority: Authority, who: Principal) -> Self {
        let seen = authority.version();
        Local {
            authority,
            who,
            next: 0,
            seen,
            out: Vec::new(),
        }
    }
}

impl ModelService for Local {
    fn send(&mut self, req: Request) -> u64 {
        self.next += 1;
        let r = self
            .authority
            .serve(&self.who, "local", req)
            .map_err(|r| r.body);
        self.out.push(Event::Reply(self.next, r));
        self.next
    }

    fn poll(&mut self) -> Vec<Event> {
        let _ = self.authority.refresh();
        if let Some(c) = self.authority.changes_since(&self.who, &self.seen) {
            self.out.push(Event::Changed(c));
        }
        self.seen = self.authority.version();
        std::mem::take(&mut self.out)
    }
}
