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

use std::collections::HashSet;

use makit_core::SessionMeta;

use crate::sidebar::groups::RunState;
use crate::workspace::model::{collect_containers, LayoutNode};

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
    /// Available memory ratio (#D-2118: free + reclaimable cache, not just strictly-free pages) has dropped
    /// below the configured floor.
    LowMemory,
    /// 1-minute load average is above the configured ceiling.
    HighLoad,
}

/// Resource thresholds, adjustable in settings. Defaults are a deliberately conservative first cut from tonight's
/// numbers (500+ load, 74M/32G free came from 40 sessions): leaves headroom rather than chasing the exact edge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapacityLimits {
    pub max_active: usize,
    pub min_available_mem_ratio: f32,
    pub max_load_1m: f32,
}

impl Default for CapacityLimits {
    fn default() -> Self {
        Self { max_active: 20, min_available_mem_ratio: 0.1, max_load_1m: 8.0 }
    }
}

/// Would opening one more running session right now hit a limit, and which one (#D-2066)? Checked before `open`
/// creates a new session or `resume` restarts an ended one's process; resuming a session that is already running
/// (#D-2044's `already_running` case) never reaches this check, since it starts no new process.
pub fn capacity_limit(active_count: usize, available_mem_ratio: f32, load_1m: f32, limits: CapacityLimits) -> Option<CapacityLimit> {
    if active_count >= limits.max_active {
        return Some(CapacityLimit::Count);
    }
    if available_mem_ratio < limits.min_available_mem_ratio {
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

impl CapacityLimit {
    /// A short, caller-facing reason (#D-2078): used in both the link.sock error and any local toast, so there is
    /// one place that decides the wording.
    pub fn reason(self) -> &'static str {
        match self {
            CapacityLimit::Count => "too many sessions are already running",
            CapacityLimit::LowMemory => "the system is low on free memory",
            CapacityLimit::HighLoad => "the system load is too high right now",
        }
    }
}

pub fn decide_eviction(limit: CapacityLimit, victim: Option<&ActiveCandidate>, policy: EvictionPolicy) -> EvictionDecision {
    match (policy, victim) {
        (EvictionPolicy::Off, _) | (_, None) => EvictionDecision::Blocked(limit),
        (EvictionPolicy::Auto, Some(v)) => EvictionDecision::AutoSleep { session_id: v.session_id.clone() },
        (EvictionPolicy::AskEachTime, Some(v)) => EvictionDecision::AskToSleep { session_id: v.session_id.clone() },
    }
}

/// Every session id that is some pane's current tab right now (#D-2090 接线阶段): a split layout can show more
/// than one at once, all "being looked at". Feeds `ActiveCandidate.focused` -- separate from `evictable` itself so
/// it can be tested against plain `LayoutNode` fixtures the same way `state::binding` does.
pub fn focused_session_ids(root: &LayoutNode) -> HashSet<String> {
    collect_containers(root)
        .into_iter()
        .filter_map(|c| c.tabs.iter().find(|t| t.id == c.active_tab_id))
        .filter_map(|t| t.session_id.clone())
        .collect()
}

/// Builds the eviction queue's candidate list from the live session list (#D-2090 接线阶段): only running sessions
/// count (a stopped tab is not occupying anything), everything else is a straight lookup against the two sets the
/// caller already has (pinned sessions, focused tab ids).
pub fn active_candidates(sessions: &[SessionMeta], pinned: &HashSet<String>, focused: &HashSet<String>) -> Vec<ActiveCandidate> {
    sessions
        .iter()
        .filter(|s| s.running)
        .map(|s| ActiveCandidate {
            session_id: s.session_id.clone(),
            run_state: crate::sidebar::groups::run_state(s.running, &s.status, &s.waiting_for),
            pinned: pinned.contains(&s.session_id),
            focused: focused.contains(&s.session_id),
            last_active: s.mtime,
        })
        .collect()
}

/// A settings-panel stepper's next value, clamped to `[min, max]`. Shared by the three capacity thresholds instead
/// of three near-identical functions -- the ranges/step sizes live with the UI that uses them.
pub fn step_u32(cur: u32, delta: i32, min: u32, max: u32) -> u32 {
    (cur as i64 + delta as i64).clamp(min as i64, max as i64) as u32
}

/// `step_u32`'s float counterpart (the load-average threshold isn't a whole number).
pub fn step_f32(cur: f32, delta: f32, min: f32, max: f32) -> f32 {
    (cur + delta).clamp(min, max)
}

/// Settings-panel ranges for the three thresholds (#D-2090): min/max/step, in the unit the panel displays.
pub const MAX_ACTIVE_RANGE: (u32, u32, i32) = (2, 60, 1);
pub const MIN_FREE_MEM_PERCENT_RANGE: (u32, u32, i32) = (0, 50, 5);
pub const MAX_LOAD_1M_RANGE: (f32, f32, f32) = (1.0, 32.0, 1.0);

/// Persisted prefs store the policy as a plain string (same convention as `pane_icons` / `sidebar.hover_mode`),
/// not a serde-derived enum: keeps `capacity`'s domain types free of a serde dependency it otherwise has no need
/// for. Any stored value other than "auto" / "off" falls back to the default `AskEachTime` -- a hand-edited or
/// future-version state file should fail safe toward asking, not toward silently auto-killing sessions.
pub fn policy_from_str(s: &str) -> EvictionPolicy {
    match s {
        "auto" => EvictionPolicy::Auto,
        "off" => EvictionPolicy::Off,
        _ => EvictionPolicy::AskEachTime,
    }
}

pub fn policy_to_str(p: EvictionPolicy) -> &'static str {
    match p {
        EvictionPolicy::Auto => "auto",
        EvictionPolicy::Off => "off",
        EvictionPolicy::AskEachTime => "ask",
    }
}

/// Persisted thresholds (plain numbers, percent instead of a ratio so the settings panel shows whole numbers) ->
/// the domain type `capacity_limit` actually takes.
pub fn limits_from_prefs(max_active: u32, min_free_mem_percent: u32, max_load_1m: f32) -> CapacityLimits {
    CapacityLimits { max_active: max_active as usize, min_available_mem_ratio: (min_free_mem_percent as f32 / 100.0).clamp(0.0, 1.0), max_load_1m }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits() -> CapacityLimits {
        CapacityLimits { max_active: 3, min_available_mem_ratio: 0.1, max_load_1m: 8.0 }
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

    #[test]
    fn every_limit_has_a_distinct_reason() {
        let reasons = [CapacityLimit::Count.reason(), CapacityLimit::LowMemory.reason(), CapacityLimit::HighLoad.reason()];
        assert_eq!(reasons.len(), std::collections::HashSet::from(reasons).len(), "reasons must not collide");
    }

    use crate::state::sessions::tests::session;
    use crate::workspace::model::{ContainerNode, PaneTab, SplitNode};

    fn tab(id: &str, sid: Option<&str>) -> PaneTab {
        PaneTab { id: id.into(), kind: crate::workspace::model::TabKind::Shell, cwd: "/w".into(), init_command: None, session_id: sid.map(String::from), session_short_id: None, label: String::new(), stopped: false, pinned: false }
    }

    fn container(id: &str, active: &str, tabs: Vec<PaneTab>) -> LayoutNode {
        LayoutNode::Container(ContainerNode { id: id.into(), active_tab_id: active.into(), tabs, tab_history: vec![] })
    }

    #[test]
    fn focused_session_ids_is_each_pane_s_active_tab() {
        let root = container("c1", "t1", vec![tab("t1", Some("a")), tab("t2", Some("b"))]);
        // only t1 is active in this single-pane layout: t2's session is not "being looked at"
        assert_eq!(focused_session_ids(&root), HashSet::from(["a".to_string()]));
    }

    #[test]
    fn focused_session_ids_covers_every_pane_in_a_split() {
        let left = container("c1", "t1", vec![tab("t1", Some("a"))]);
        let right = container("c2", "t2", vec![tab("t2", Some("b"))]);
        let root = LayoutNode::Split(SplitNode { dir: crate::workspace::model::Dir::V, ratio: 0.5, a: Box::new(left), b: Box::new(right) });
        assert_eq!(focused_session_ids(&root), HashSet::from(["a".to_string(), "b".to_string()]));
    }

    #[test]
    fn focused_session_ids_skips_tabs_with_no_session() {
        let root = container("c1", "t1", vec![tab("t1", None)]);
        assert!(focused_session_ids(&root).is_empty());
    }

    fn running(id: &str, status: &str, waiting: &str, mtime: i64) -> SessionMeta {
        let mut s = session(id);
        s.running = true;
        s.status = status.into();
        s.waiting_for = waiting.into();
        s.mtime = mtime;
        s
    }

    #[test]
    fn active_candidates_only_counts_running_sessions_and_looks_up_pin_and_focus() {
        let mut stopped = session("c");
        stopped.running = false;
        let sessions = [running("a", "idle", "", 10), running("b", "busy", "", 20), stopped];
        let pinned = HashSet::from(["b".to_string()]);
        let focused = HashSet::from(["a".to_string()]);
        let out = active_candidates(&sessions, &pinned, &focused);
        assert_eq!(out.len(), 2, "the stopped session is not occupying anything, so it is not a candidate at all");
        let a = out.iter().find(|c| c.session_id == "a").unwrap();
        assert_eq!(a.run_state, RunState::Idle);
        assert!(!a.pinned && a.focused && a.last_active == 10);
        let b = out.iter().find(|c| c.session_id == "b").unwrap();
        assert_eq!(b.run_state, RunState::Busy);
        assert!(b.pinned && !b.focused);
    }

    #[test]
    fn step_u32_and_step_f32_clamp_to_the_range() {
        assert_eq!(step_u32(20, 1, 2, 60), 21);
        assert_eq!(step_u32(60, 1, 2, 60), 60, "no stepping past the max");
        assert_eq!(step_u32(2, -1, 2, 60), 2, "no stepping past the min");
        assert_eq!(step_f32(8.0, 1.0, 1.0, 32.0), 9.0);
        assert_eq!(step_f32(1.0, -1.0, 1.0, 32.0), 1.0);
    }

    #[test]
    fn policy_str_round_trips_and_falls_back_to_ask_each_time() {
        for p in [EvictionPolicy::Auto, EvictionPolicy::AskEachTime, EvictionPolicy::Off] {
            assert_eq!(policy_from_str(policy_to_str(p)), p);
        }
        assert_eq!(policy_from_str("garbage"), EvictionPolicy::AskEachTime, "a hand-edited or future value fails safe toward asking, not auto-killing");
    }

    #[test]
    fn limits_from_prefs_converts_the_percent_and_clamps_it() {
        let l = limits_from_prefs(20, 10, 8.0);
        assert_eq!(l, CapacityLimits { max_active: 20, min_available_mem_ratio: 0.1, max_load_1m: 8.0 });
        assert_eq!(limits_from_prefs(20, 500, 8.0).min_available_mem_ratio, 1.0, "a corrupt percent clamps instead of letting the cap become unreachable");
    }
}
