//! Archive: `~/.claude/makit/archived.json`, a sorted array of session_ids.

use crate::ts;
use std::fs;
use std::path::PathBuf;

pub fn archived_store_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("archived.json"))
}

pub fn load_archived() -> std::collections::HashSet<String> {
    match archived_store_path() {
        Some(p) => load_archived_from(&p),
        None => std::collections::HashSet::new(),
    }
}

fn load_archived_from(path: &std::path::Path) -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    if !path.exists() {
        return set;
    }
    let content = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            log::warn!(target: "archive", "cannot read {}: {e}; treating as empty", path.display());
            return set;
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            log::warn!(target: "archive", "cannot parse {}: {e}; treating as empty", path.display());
            return set;
        }
    };
    if let Some(arr) = v.as_array() {
        for x in arr {
            if let Some(id) = x.as_str() {
                set.insert(id.to_string());
            }
        }
    }
    set
}

pub fn save_archived(set: &std::collections::HashSet<String>) -> Result<(), String> {
    let res = write_archived(set);
    if let Err(e) = &res {
        log::warn!(target: "archive", "cannot save the archive list: {e}");
    }
    res
}

fn write_archived(set: &std::collections::HashSet<String>) -> Result<(), String> {
    let path = archived_store_path().ok_or_else(|| ts!("core.err.no_home").to_string())?;
    write_archived_to(&path, set)
}

fn write_archived_to(path: &std::path::Path, set: &std::collections::HashSet<String>) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut list: Vec<&String> = set.iter().collect();
    list.sort();
    let s = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    fs::write(path, s).map_err(|e| e.to_string())
}

pub fn archive_session(session_id: String) -> Result<(), String> {
    if session_id.is_empty() {
        return Err(ts!("core.err.empty_session_id").into());
    }
    let mut set = load_archived();
    set.insert(session_id);
    save_archived(&set)
}

/// Archive many sessions with one read and one write of archived.json (#294). Returns how many were newly archived.
pub fn archive_sessions(ids: &[String]) -> Result<usize, String> {
    set_archived(ids, true)
}

/// Unarchive many sessions with one write (#294). Returns how many were archived before.
pub fn unarchive_sessions(ids: &[String]) -> Result<usize, String> {
    set_archived(ids, false)
}

fn set_archived(ids: &[String], archived: bool) -> Result<usize, String> {
    let path = archived_store_path().ok_or_else(|| ts!("core.err.no_home").to_string())?;
    let res = set_archived_at(&path, ids, archived);
    if let Err(e) = &res {
        log::warn!(target: "archive", "cannot save the archive list: {e}");
    }
    res
}

/// Add (`archived`) or remove `ids` in the list at `path`; writes only when something changed. Any empty id
/// rejects the whole batch before the file is touched.
fn set_archived_at(path: &std::path::Path, ids: &[String], archived: bool) -> Result<usize, String> {
    if ids.iter().any(|id| id.is_empty()) {
        return Err(ts!("core.err.empty_session_id").into());
    }
    let mut set = load_archived_from(path);
    let changed = ids
        .iter()
        .filter(|id| if archived { set.insert((*id).clone()) } else { set.remove(*id) })
        .count();
    if changed > 0 {
        write_archived_to(path, &set)?;
    }
    Ok(changed)
}

pub fn unarchive_session(session_id: String) -> Result<(), String> {
    if session_id.is_empty() {
        return Err(ts!("core.err.empty_session_id").into());
    }
    let mut set = load_archived();
    set.remove(&session_id);
    save_archived(&set)
}

#[cfg(test)]
mod tests {
    //! Batch archive (#294) is tested on its own because the sidebar archives many sessions with one write:
    //! if the batch drops ids or clobbers unrelated ones, sessions silently reappear (or vanish) after a restart.
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("makit-archive-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir.join("archived.json")
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn sorted(p: &std::path::Path) -> Vec<String> {
        let mut v: Vec<String> = load_archived_from(p).into_iter().collect();
        v.sort();
        v
    }

    #[test]
    fn batch_archive_adds_all_and_keeps_existing() {
        let p = tmp("add");
        set_archived_at(&p, &ids(&["old"]), true).unwrap();
        let changed = set_archived_at(&p, &ids(&["b", "a", "old"]), true).unwrap();
        assert_eq!(changed, 2, "already archived ids are not counted");
        assert_eq!(sorted(&p), ids(&["a", "b", "old"]));
        let raw = fs::read_to_string(&p).unwrap();
        assert!(raw.find("\"a\"").unwrap() < raw.find("\"b\"").unwrap(), "file stays a sorted array");
    }

    #[test]
    fn batch_unarchive_removes_only_the_given_ids() {
        let p = tmp("remove");
        set_archived_at(&p, &ids(&["a", "b", "c"]), true).unwrap();
        let changed = set_archived_at(&p, &ids(&["a", "c", "missing"]), false).unwrap();
        assert_eq!(changed, 2);
        assert_eq!(sorted(&p), ids(&["b"]));
    }

    #[test]
    fn empty_ids_are_rejected_and_nothing_is_written() {
        let p = tmp("empty");
        assert!(set_archived_at(&p, &ids(&["a", ""]), true).is_err());
        assert!(!p.exists(), "a rejected batch must not touch the file");
    }

    #[test]
    fn a_batch_with_no_change_does_not_rewrite() {
        let p = tmp("noop");
        set_archived_at(&p, &ids(&["a"]), true).unwrap();
        fs::write(&p, "[\"a\"]").unwrap();
        assert_eq!(set_archived_at(&p, &ids(&["a"]), true).unwrap(), 0);
        assert_eq!(fs::read_to_string(&p).unwrap(), "[\"a\"]", "untouched when nothing changed");
    }
}
