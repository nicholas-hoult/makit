//! Snapshot of the sessions that were running right before makit's last quit (#D-2122). Written once as part of
//! the normal quit sequence -- a plain quit, a restart, or the restart an `update install` triggers all go
//! through the same path, so one write covers all three. Read back by the `last_running` local-session link op:
//! 总管 uses it to decide which sessions to resume after a restart and in what order; makit itself never acts on
//! it, which is why this is just a file, not a queue or a trigger.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LastRunning {
    pub session_id: String,
    pub name: String,
    pub cwd: String,
    /// Epoch seconds, same clock as [`crate::sessions::SessionMeta::mtime`] -- "last active", not "quit time".
    pub last_active: i64,
}

fn path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".makit").join("last_running.json"))
}

/// Overwrites the snapshot. Best-effort on purpose: a write failure here (disk full, odd permissions) must not
/// hold up quitting.
pub fn write(entries: &[LastRunning]) {
    let Some(p) = path() else { return };
    if let Ok(json) = serde_json::to_string(entries) {
        let _ = std::fs::write(p, json);
    }
}

/// Empty when there is no snapshot yet (first ever launch) or it cannot be read -- never an error the other side
/// has to handle, same fail-open stance as the rest of the local-session link.
pub fn read() -> Vec<LastRunning> {
    let Some(p) = path() else { return Vec::new() };
    std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let entries = vec![
            LastRunning { session_id: "s1".into(), name: "总管".into(), cwd: "/Users/me/p".into(), last_active: 1700000000 },
            LastRunning { session_id: "s2".into(), name: "刀冢".into(), cwd: "/Users/me/q".into(), last_active: 1700000500 },
        ];
        let json = serde_json::to_string(&entries).unwrap();
        let back: Vec<LastRunning> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entries);
    }
}
