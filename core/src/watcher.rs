//! Session directory watching.
//!
//! No UI framework dependency: the caller passes a callback. The Tauri version emits an event to the frontend inside it,
//! and the GPUI version sends the event into its own channel.
//!
//! Incremental push from fs.watch:
//! - a change in sessions/ -> `RunningChanged` (only the running state needs re-reading; lightweight)
//! - a change in projects/ -> `SessionsChanged(paths)`; **the payload is the list of changed jsonl paths**.
//!   This used to be sent with an empty payload, with a comment saying "low frequency, only fires when a new session is created", but projects/ is
//!   watched recursively: it fires on every appended message. The frontend could therefore only wire it to a no-op, and a new session did not
//!   show up until a manual refresh (#168). With the paths attached, the frontend can parse just those files (list_sessions_by_paths).

use std::path::Path;

/// The events derived from one batch (500ms debounce) of file changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchEvent {
    /// Something changed under `~/.claude/sessions/`: running state / rename; the caller re-reads `list_running_sessions`
    RunningChanged,
    /// These jsonl files changed (sorted and deduplicated): the caller parses them incrementally with `list_sessions_by_paths`
    SessionsChanged(Vec<String>),
}

/// Condense a batch of changed paths into events. When both kinds are present in one batch, `RunningChanged` comes first (matching the original emit order).
pub fn classify<'a>(paths: impl IntoIterator<Item = &'a Path>, sessions_dir: &Path) -> Vec<WatchEvent> {
    let mut has_session_event = false;
    let mut changed_jsonl: Vec<String> = Vec::new();
    for path in paths {
        if path.starts_with(sessions_dir) {
            has_session_event = true;
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            // Report only jsonl: directory creation, lock files and the like carry no information for the frontend.
            // A new session always writes its own jsonl as well, so nothing is missed.
            changed_jsonl.push(path.to_string_lossy().into_owned());
        }
    }
    let mut out = Vec::new();
    // a change in sessions/ = running state update (lightweight: the frontend only refreshes running state)
    if has_session_event {
        out.push(WatchEvent::RunningChanged);
    }
    // a change in projects/ = some session's content changed (created / new message / renamed)
    if !changed_jsonl.is_empty() {
        changed_jsonl.sort();
        changed_jsonl.dedup();
        out.push(WatchEvent::SessionsChanged(changed_jsonl));
    }
    out
}

/// Start a background thread that watches `~/.claude/sessions` (non-recursive), `~/.claude/projects` (recursive),
/// and `~/.codex/sessions` (recursive, when it exists). 500ms debounce; each batch of changes is condensed by `classify` and the callback is invoked per event.
/// The thread lives as long as the process and has no stop interface (both sides call this only once at startup).
pub fn start(on_event: impl Fn(WatchEvent) + Send + 'static) {
    use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
    use std::time::Duration;

    let home = match dirs::home_dir() { Some(h) => h, None => return };
    let sessions_dir = home.join(".claude").join("sessions");
    let projects_dir = home.join(".claude").join("projects");
    let codex_sessions_dir = home.join(".codex").join("sessions");

    let _ = std::fs::create_dir_all(&sessions_dir);
    let _ = std::fs::create_dir_all(&projects_dir);

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = match new_debouncer(Duration::from_millis(500), tx) {
            Ok(d) => d,
            Err(e) => { log::error!(target: "watcher", "failed to initialise the watcher: {e}"); return; }
        };
        let _ = debouncer.watcher().watch(&sessions_dir, RecursiveMode::NonRecursive);
        let _ = debouncer.watcher().watch(&projects_dir, RecursiveMode::Recursive);
        if codex_sessions_dir.exists() {
            let _ = debouncer.watcher().watch(&codex_sessions_dir, RecursiveMode::Recursive);
        }

        for result in rx {
            if let Ok(events) = result {
                for ev in classify(events.iter().map(|e| e.path.as_path()), &sessions_dir) {
                    on_event(ev);
                }
            }
        }
    });
}

/// After the callback-style refactor (#226), the "a batch of changes -> what to send" decision was split out of emit and is shared by Tauri and GPUI.
/// What goes wrong on screen if this breaks: the sidebar running state does not refresh (RunningChanged lost), or new messages / new sessions
/// only appear after a manual refresh (jsonl paths not reported), or every message triggers a full rescan (non-jsonl noise was reported too).
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn classifies_running_and_jsonl_changes() {
        let sessions = PathBuf::from("/Users/me/.claude/sessions");
        let paths = [
            PathBuf::from("/Users/me/.claude/projects/-a/b.jsonl"),
            PathBuf::from("/Users/me/.claude/sessions/123.json"),
            PathBuf::from("/Users/me/.claude/projects/-a/a.jsonl"),
            PathBuf::from("/Users/me/.claude/projects/-a/b.jsonl"),
            PathBuf::from("/Users/me/.claude/projects/-a"),
            PathBuf::from("/Users/me/.claude/projects/-a/x.lock"),
        ];
        let got = classify(paths.iter().map(|p| p.as_path()), &sessions);
        assert_eq!(
            got,
            vec![
                WatchEvent::RunningChanged,
                WatchEvent::SessionsChanged(vec![
                    "/Users/me/.claude/projects/-a/a.jsonl".into(),
                    "/Users/me/.claude/projects/-a/b.jsonl".into(),
                ]),
            ],
            "运行状态在前；jsonl 排序去重；目录和锁文件不报"
        );
    }

    #[test]
    fn nothing_interesting_means_no_event() {
        let sessions = PathBuf::from("/Users/me/.claude/sessions");
        let paths = [PathBuf::from("/Users/me/.claude/projects/-a")];
        assert!(classify(paths.iter().map(|p| p.as_path()), &sessions).is_empty());
        // Only changes under sessions/: report running state only (a .jsonl under sessions/ is not treated as session content either)
        let paths = [PathBuf::from("/Users/me/.claude/sessions/9.jsonl")];
        assert_eq!(classify(paths.iter().map(|p| p.as_path()), &sessions), vec![WatchEvent::RunningChanged]);
    }
}
