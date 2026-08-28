//! Pure refresh-scheduling decisions: given each external-sourced measure's
//! [`RefreshPolicy`] and when it last ran, decide which
//! measures are *due* to refresh now. This is intentionally I/O-free and
//! clock-free (times are plain seconds) so a daemon can drive it and tests can
//! exercise it deterministically. The daemon (CLI `serve-refresh`) does the
//! actual query/eval; this only picks what and when.

use crate::{MeasureId, RefreshPolicy};
use std::collections::HashMap;

/// Decide whether a single measure is due to refresh.
///
/// * `Manual` — never scheduled (only `refresh`/`refresh-all` run it).
/// * `OnLoad` — due once, when it has never run in this daemon session
///   (`last_run` is `None`).
/// * `Interval { secs }` — due when it has never run, or when `now_secs` is at
///   least `secs` past `last_run`.
pub fn is_due(policy: RefreshPolicy, now_secs: u64, last_run: Option<u64>) -> bool {
    match policy {
        RefreshPolicy::Manual => false,
        RefreshPolicy::OnLoad => last_run.is_none(),
        RefreshPolicy::Interval { secs } => match last_run {
            None => true,
            Some(last) => now_secs.saturating_sub(last) >= secs,
        },
    }
}

/// Return the ids of every measure whose policy makes it due at `now_secs`,
/// given the policy per measure and the last run time per measure. Ids are
/// returned sorted for determinism.
pub fn due_measures(
    policies: &HashMap<MeasureId, RefreshPolicy>,
    now_secs: u64,
    last_run: &HashMap<MeasureId, u64>,
) -> Vec<MeasureId> {
    let mut due: Vec<MeasureId> = policies
        .iter()
        .filter(|(id, &p)| is_due(p, now_secs, last_run.get(id).copied()))
        .map(|(id, _)| *id)
        .collect();
    due.sort_by_key(|m| m.0);
    due
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_is_never_due() {
        assert!(!is_due(RefreshPolicy::Manual, 100, None));
        assert!(!is_due(RefreshPolicy::Manual, 100, Some(0)));
    }

    #[test]
    fn on_load_is_due_only_once() {
        assert!(is_due(RefreshPolicy::OnLoad, 0, None));
        assert!(!is_due(RefreshPolicy::OnLoad, 999, Some(0)));
    }

    #[test]
    fn interval_respects_elapsed() {
        let p = RefreshPolicy::Interval { secs: 30 };
        assert!(is_due(p, 0, None)); // never run -> due
        assert!(!is_due(p, 20, Some(0))); // only 20s elapsed
        assert!(is_due(p, 30, Some(0))); // exactly 30s -> due
        assert!(is_due(p, 61, Some(30))); // 31s since last -> due
    }

    #[test]
    fn due_measures_filters_and_sorts() {
        let mut policies = HashMap::new();
        policies.insert(MeasureId(3), RefreshPolicy::OnLoad);
        policies.insert(MeasureId(1), RefreshPolicy::Manual);
        policies.insert(MeasureId(2), RefreshPolicy::Interval { secs: 10 });
        let mut last = HashMap::new();
        last.insert(MeasureId(2), 100u64); // ran at 100
                                           // now=105: measure 2 not yet due (5s<10), measure 3 due (never ran),
                                           // measure 1 manual -> never.
        assert_eq!(due_measures(&policies, 105, &last), vec![MeasureId(3)]);
        // now=115: measure 2 now due too; sorted.
        assert_eq!(
            due_measures(&policies, 115, &last),
            vec![MeasureId(2), MeasureId(3)]
        );
    }
}
