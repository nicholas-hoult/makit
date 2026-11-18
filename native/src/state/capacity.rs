//! Active-session cap and eviction queue (#D-2066): how many sessions may run at once, and which one gets put to
//! sleep to make room for a new one. Triggered by a real incident -- 40 running sessions plus a simulator and a
//! compile left a 32G machine with 74M free and load past 500, CPU 2% idle.
//!
//! "Sleep" is not a new concept: it is #287's existing "stop but keep the tab" (kill the process, tab stays,
//! marked stopped), and waking it back up is #D-2044's `resume_session` (reuse the kept tab instead of opening a
//! second one). This module only decides *whether* a new session may open and *which* running one to put to sleep
//! if not; the actual killing and resuming reuse those two paths unchanged.
//!
//! Why tested separately: the judgment here runs right before every `open` / `resume`. Wrong in either direction is
//! bad in a different way -- too loose and the next incident repeats; too eager and a session the user is
//! mid-conversation with (or one they pinned) gets killed out from under them.

use crate::sidebar::groups::RunState;

/// One running session's standing for the cap / eviction queue.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveCandidate {
    pub session_id: String,
    pub run_state: RunState,
    /// Reuses the existing sidebar pin (`AppState::is_pinned`): a session the user already cares enough about to
    /// pin is also one they do not want silently put to sleep. No separate "never evict" flag.
    pub pinned: bool,
    /// Its tab is the active tab of its pane right now. A split layout can have more than one such session at
    /// once (one per visible pane) -- all of them count as "being looked at".
    pub focused: bool,
    /// Last activity (session file mtime): older = further back in the queue = evicted first.
    pub last_active: i64,
}

/// Why a new session was refused (#D-2066). Reported to both the UI and link.sock (`open` / `resume`), as an
/// added field only -- the protocol does not change shape for callers that do not look for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityLimit {
    /// At the configured cap on how many sessions may run at once.
    Count,
    /// Free memory ratio has dropped below the configured floor.
    LowMemory,
    /// 1-minute load average is above the configured ceiling.
    HighLoad,
}

/// Resource thresholds, adjustable in settings. Defaults are a deliberately conservative first cut from tonight's
/// numbers (500+ load, 74M/32G free came from 40 sessions): leaves headroom rather than chasing the exact edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapacityLimits {
    pub max_active: usize,
    pub min_free_mem_ratio: f32,
    pub max_load_1m: f32,
}

impl Default for CapacityLimits {
    fn default() -> Self {
        Self { max_active: 20, min_free_mem_ratio: 0.1, max_load_1m: 8.0 }
    }
}

/// Would opening one more running session right now hit a limit, and which one (#D-2066)? Checked before `open`
/// creates a new session or `resume` restarts an ended one's process; resuming a session that is already running
/// (#D-2044's `already_running` case) never reaches this check, since it starts no new process.
pub fn capacity_limit(active_count: usize, free_mem_ratio: f32, load_1m: f32, limits: CapacityLimits) -> Option<CapacityLimit> {
    if active_count >= limits.max_active {
        return Some(CapacityLimit::Count);
    }
    if free_mem_ratio < limits.min_free_mem_ratio {
        return Some(CapacityLimit::LowMemory);
    }
    if load_1m > limits.max_load_1m {
        return Some(CapacityLimit::HighLoad);
    }
    None
}

/// A session may be put to sleep only when it is idle, unpinned, and not the one the user is currently looking at:
/// busy/waiting sessions are mid-something, a pinned one is the user's explicit "leave this alone", and sleeping
/// the tab in front of the user would be jarring even if it happens to be idle.
pub fn evictable(c: &ActiveCandidate) -> bool {
    c.run_state == RunState::Idle && !c.pinned && !c.focused
}

/// The eviction victim: the evictable candidate that has sat idle the longest ("队尾最久没动的那个"). `None` when
/// nothing qualifies (everything is busy, waiting, pinned, or in front of the user) -- the cap then just blocks.
pub fn eviction_victim(candidates: &[ActiveCandidate]) -> Option<&ActiveCandidate> {
    candidates.iter().filter(|c| evictable(c)).min_by_key(|c| c.last_active)
}

/// How the eviction queue may be used when the cap is hit, user-chosen in settings. Default is `AskEachTime`:
/// sleeping a session without asking can surprise the user, and never offering it defeats having a queue at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvictionPolicy {
    Auto,
    AskEachTime,
    Off,
}

impl Default for EvictionPolicy {
    fn default() -> Self {
        EvictionPolicy::AskEachTime
    }
}

/// What to do about a new session that wants to open while at the cap (#D-2066).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvictionDecision {
    /// Nothing can be freed (policy is `Off`, or nothing in the queue is evictable): refuse, with the reason.
    Blocked(CapacityLimit),
    /// Put this session to sleep (#287's stop-but-keep-tab), then let the new one open.
    AutoSleep { session_id: String },
    /// Ask the user before sleeping this session.
    AskToSleep { session_id: String },
}

pub fn decide_eviction(limit: CapacityLimit, victim: Option<&ActiveCandidate>, policy: EvictionPolicy) -> EvictionDecision {
    match (policy, victim) {
        (EvictionPolicy::Off, _) | (_, None) => EvictionDecision::Blocked(limit),
        (EvictionPolicy::Auto, Some(v)) => EvictionDecision::AutoSleep { session_id: v.session_id.clone() },
        (EvictionPolicy::AskEachTime, Some(v)) => EvictionDecision::AskToSleep { session_id: v.session_id.clone() },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> CapacityLimits {
        CapacityLimits { max_active: 3, min_free_mem_ratio: 0.1, max_load_1m: 8.0 }
    }

    #[test]
    fn capacity_limit_checks_count_then_memory_then_load() {
        let l = limits();
        assert_eq!(capacity_limit(2, 0.5, 1.0, l), None, "under every threshold");
        assert_eq!(capacity_limit(3, 0.5, 1.0, l), Some(CapacityLimit::Count));
        assert_eq!(capacity_limit(2, 0.05, 1.0, l), Some(CapacityLimit::LowMemory));
        assert_eq!(capacity_limit(2, 0.5, 9.0, l), Some(CapacityLimit::HighLoad));
        // right at the floor/ceiling is still fine; only strictly past it trips
        assert_eq!(capacity_limit(2, 0.1, 8.0, l), None);
    }

    fn candidate(id: &str, state: RunState, pinned: bool, focused: bool, last_active: i64) -> ActiveCandidate {
        ActiveCandidate { session_id: id.into(), run_state: state, pinned, focused, last_active }
    }

    #[test]
    fn only_idle_unpinned_unfocused_sessions_are_evictable() {
        assert!(evictable(&candidate("a", RunState::Idle, false, false, 0)));
        assert!(!evictable(&candidate("b", RunState::Busy, false, false, 0)), "mid-turn, leave it alone");
        assert!(!evictable(&candidate("c", RunState::Waiting, false, false, 0)), "waiting on the user, not a good time");
        assert!(!evictable(&candidate("d", RunState::Idle, true, false, 0)), "pinned: user's explicit do-not-touch");
        assert!(!evictable(&candidate("e", RunState::Idle, false, true, 0)), "the tab the user is looking at right now");
        // already stopped sessions are not running at all, so they never even reach the candidate list in
        // practice, but the predicate itself should not treat them as fair game either
        assert!(!evictable(&candidate("f", RunState::Stopped, false, false, 0)));
    }

    #[test]
    fn eviction_victim_is_the_evictable_one_idle_the_longest() {
        let candidates = vec![
            candidate("recent", RunState::Idle, false, false, 100),
            candidate("oldest", RunState::Idle, false, false, 10),
            candidate("busy", RunState::Busy, false, false, 1), // oldest by number, but not evictable
            candidate("pinned", RunState::Idle, true, false, 1),
        ];
        assert_eq!(eviction_victim(&candidates).map(|c| c.session_id.as_str()), Some("oldest"));
    }

    #[test]
    fn eviction_victim_is_none_when_nothing_qualifies() {
        let candidates = vec![candidate("busy", RunState::Busy, false, false, 1), candidate("pinned", RunState::Idle, true, false, 2)];
        assert_eq!(eviction_victim(&candidates), None);
    }

    #[test]
    fn decide_eviction_follows_the_chosen_policy() {
        let c = CapacityLimit::Count;
        let v = candidate("v", RunState::Idle, false, false, 1);
        assert_eq!(decide_eviction(c, Some(&v), EvictionPolicy::Auto), EvictionDecision::AutoSleep { session_id: "v".into() });
        assert_eq!(decide_eviction(c, Some(&v), EvictionPolicy::AskEachTime), EvictionDecision::AskToSleep { session_id: "v".into() });
        assert_eq!(decide_eviction(c, Some(&v), EvictionPolicy::Off), EvictionDecision::Blocked(c));
        // policy says to free one up, but there is nobody eligible: blocked regardless of policy
        assert_eq!(decide_eviction(c, None, EvictionPolicy::Auto), EvictionDecision::Blocked(c));
    }

    #[test]
    fn default_policy_is_ask_each_time() {
        assert_eq!(EvictionPolicy::default(), EvictionPolicy::AskEachTime);
    }
}
