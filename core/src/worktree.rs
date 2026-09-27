use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct WorktreeInfo {
    pub name: String,
    pub branch: String,
    pub path: String,
}

pub fn list_worktrees_for(git_root: &str) -> Vec<WorktreeInfo> {
    let output = match Command::new("git")
        .args(["-C", git_root, "worktree", "list", "--porcelain"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return Vec::new(),
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut result = Vec::new();
    let mut first = true;
    for block in text.split("\n\n") {
        if block.trim().is_empty() {
            continue;
        }
        if first {
            first = false;
            continue; // skip main worktree
        }
        let mut path = String::new();
        let mut branch = String::new();
        for line in block.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                path = p.to_string();
            }
            if let Some(b) = line.strip_prefix("branch refs/heads/") {
                branch = b.to_string();
            }
        }
        if path.is_empty() {
            continue;
        }
        let name = path.rsplit('/').next().unwrap_or("").to_string();
        result.push(WorktreeInfo { name, branch, path });
    }
    result
}
