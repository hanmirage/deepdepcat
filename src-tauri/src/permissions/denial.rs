//! Denial tracker — tracks consecutive permission denials to prevent
//! the agent from repeatedly trying denied operations.
//!
//! The counter is keyed by session: one session hitting the denial limit
//! (or the unattended scheduler being refused) must never trip another
//! session's gate. Sessions share the tracker but keep isolated budgets.
//!
//! A tripped session is NOT locked out forever: after [`TRIP_COOLDOWN`]
//! the counter auto-recovers, so the agent can resume (or the user can
//! guide it) instead of being stuck for the rest of the app run — the
//! pre-cooldown behavior had no reset path at all.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a tripped session stays locked before the counter auto-resets.
/// Long enough to stop the agent from hammering a denied operation, short
/// enough that a cooldown, not a restart, is the recovery path.
const TRIP_COOLDOWN: Duration = Duration::from_secs(60);

/// Tracks consecutive permission denials per session.
pub struct DenialTracker {
    consecutive_denials: Mutex<HashMap<String, u32>>,
    /// When each session first tripped the limit (cooldown anchor).
    tripped_at: Mutex<HashMap<String, Instant>>,
    max_consecutive: u32,
}

impl DenialTracker {
    pub fn new(max_consecutive: u32) -> Self {
        Self {
            consecutive_denials: Mutex::new(HashMap::new()),
            tripped_at: Mutex::new(HashMap::new()),
            max_consecutive,
        }
    }

    /// Record a denial for a session.
    pub fn record_denial(&self, session_id: &str) {
        if let Ok(mut counts) = self.consecutive_denials.lock() {
            let entry = counts.entry(session_id.to_string()).or_insert(0);
            *entry += 1;
            if *entry >= self.max_consecutive {
                if let Ok(mut tripped) = self.tripped_at.lock() {
                    tripped
                        .entry(session_id.to_string())
                        .or_insert_with(Instant::now);
                }
            }
        }
    }

    /// Record a success for a session (resets its counter).
    pub fn record_success(&self, session_id: &str) {
        if let Ok(mut counts) = self.consecutive_denials.lock() {
            counts.remove(session_id);
        }
        if let Ok(mut tripped) = self.tripped_at.lock() {
            tripped.remove(session_id);
        }
    }

    /// Check if the denial limit has been exceeded for a session.
    ///
    /// A tripped session auto-recovers once [`TRIP_COOLDOWN`] has elapsed
    /// since it first tripped — the lock is temporary, never permanent.
    pub fn exceeded_limit(&self, session_id: &str) -> bool {
        // A threshold of 0 disables the gate entirely (never trip).
        if self.max_consecutive == 0 {
            return false;
        }
        let (count, tripped_at) = {
            let counts = self
                .consecutive_denials
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let count = counts.get(session_id).copied().unwrap_or(0);
            let tripped = self
                .tripped_at
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(session_id)
                .copied();
            (count, tripped)
        };
        if count < self.max_consecutive {
            return false;
        }
        if let Some(t) = tripped_at {
            if t.elapsed() >= TRIP_COOLDOWN {
                // Cooldown expired — recover automatically.
                self.record_success(session_id);
                return false;
            }
        }
        true
    }

    /// The configured maximum (used by PermissionChecker::clone to build an
    /// independent tracker with the same threshold).
    pub fn max_consecutive(&self) -> u32 {
        self.max_consecutive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_resets_counter() {
        let t = DenialTracker::new(3);
        t.record_denial("s1");
        t.record_denial("s1");
        assert!(!t.exceeded_limit("s1"));
        t.record_success("s1");
        t.record_denial("s1");
        assert!(!t.exceeded_limit("s1"), "success must reset the streak");
    }

    #[test]
    fn limit_trips_and_cooldown_recovers() {
        let t = DenialTracker::new(2);
        t.record_denial("s1");
        assert!(!t.exceeded_limit("s1"));
        t.record_denial("s1");
        assert!(t.exceeded_limit("s1"), "limit reached — tripped");
        // Simulate the cooldown expiring: backdate the tripped anchor.
        {
            let mut tripped = t.tripped_at.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(anchor) = tripped.get_mut("s1") {
                *anchor = Instant::now() - TRIP_COOLDOWN - Duration::from_secs(1);
            }
        }
        assert!(
            !t.exceeded_limit("s1"),
            "cooldown elapsed — the session must recover"
        );
        // And the recovery reset the counter for real.
        assert!(!t.exceeded_limit("s1"));
    }

    #[test]
    fn sessions_are_isolated() {
        let t = DenialTracker::new(2);
        t.record_denial("s1");
        t.record_denial("s1");
        assert!(t.exceeded_limit("s1"));
        assert!(!t.exceeded_limit("s2"), "other sessions stay open");
    }

    #[test]
    fn zero_max_never_trips() {
        // A disabled gate (max=0) must not trip on any denial count.
        let t = DenialTracker::new(0);
        t.record_denial("s1");
        t.record_denial("s1");
        assert!(!t.exceeded_limit("s1"));
    }
}
