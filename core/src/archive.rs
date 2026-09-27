//! 归档：`~/.claude/makit/archived.json`，排好序的 session_id 数组。

use std::fs;
use std::path::PathBuf;

pub fn archived_store_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("archived.json"))
}

pub fn load_archived() -> std::collections::HashSet<String> {
    let mut set = std::collections::HashSet::new();
    let path = match archived_store_path() {
        Some(p) => p,
        None => return set,
    };
    if !path.exists() {
        return set;
    }
    let content = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return set,
    };
    let v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return set,
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
    let path = archived_store_path().ok_or_else(|| "无法定位 home".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut list: Vec<&String> = set.iter().collect();
    list.sort();
    let s = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    fs::write(&path, s).map_err(|e| e.to_string())
}

pub fn archive_session(session_id: String) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("session_id 为空".into());
    }
    let mut set = load_archived();
    set.insert(session_id);
    save_archived(&set)
}

pub fn unarchive_session(session_id: String) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("session_id 为空".into());
    }
    let mut set = load_archived();
    set.remove(&session_id);
    save_archived(&set)
}
