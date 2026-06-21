use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::TimeZone;

use crate::SessionMeta;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AiTool {
    Claude,
    Codex,
}

impl AiTool {
    pub fn all() -> &'static [AiTool] {
        &[AiTool::Claude, AiTool::Codex]
    }

    pub fn name(&self) -> &'static str {
        match self {
            AiTool::Claude => "claude",
            AiTool::Codex => "codex",
        }
    }

    pub fn sessions_dir(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        match self {
            AiTool::Claude => Some(home.join(".claude").join("projects")),
            AiTool::Codex => Some(home.join(".codex").join("sessions")),
        }
    }

    pub fn spawn_cmd(&self, cwd: &str, session_id: Option<&str>) -> String {
        match self {
            AiTool::Claude => session_id
                .map(|id| format!("claude -r {id}"))
                .unwrap_or_else(|| "claude".into()),
            AiTool::Codex => session_id
                .map(|id| format!("codex resume {id}"))
                .unwrap_or_else(|| "codex".into()),
        }
    }
}

// 递归收集 Codex sessions 目录下所有 .jsonl 文件（按年/月/日分层）
pub fn collect_codex_files(dir: &Path) -> Vec<(PathBuf, i64)> {
    let mut files = Vec::new();
    collect_recursive(dir, &mut files);
    files
}

fn collect_recursive(dir: &Path, out: &mut Vec<(PathBuf, i64)>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.is_dir() {
            collect_recursive(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.push((path, mtime));
        }
    }
}

// 解析单个 Codex JSONL 文件 → SessionMeta
pub fn parse_codex_session(
    path: &Path,
    mtime: i64,
    now: i64,
    running_info: &HashMap<String, crate::RunningInfo>,
    git_cache: &mut HashMap<String, Option<String>>,
) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut session_id = String::new();
    let mut cwd = String::new();
    let mut git_branch = String::new();
    let mut user_count: u32 = 0;
    let mut first_msg = String::new();
    let mut last_msg = String::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        let rec: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let rec_type = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let payload = rec.get("payload").cloned().unwrap_or(serde_json::Value::Null);

        match rec_type {
            "session_meta" => {
                if session_id.is_empty() {
                    if let Some(id) = payload.get("id").and_then(|v| v.as_str()) {
                        session_id = id.to_string();
                    }
                }
                if cwd.is_empty() {
                    if let Some(c) = payload.get("cwd").and_then(|v| v.as_str()) {
                        cwd = c.to_string();
                    }
                }
                if git_branch.is_empty() {
                    if let Some(b) = payload
                        .pointer("/git/branch")
                        .and_then(|v| v.as_str())
                    {
                        if !b.is_empty() && b != "HEAD" {
                            git_branch = b.to_string();
                        }
                    }
                }
            }
            "response_item" => {
                let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                if role != "user" {
                    continue;
                }
                // 提取文本内容
                let text = payload
                    .pointer("/content/0/text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // 跳过 environment_context 注入消息
                if text.trim_start().starts_with("<environment_context>") {
                    continue;
                }
                let trimmed = text.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                user_count += 1;
                if first_msg.is_empty() {
                    first_msg = trimmed.chars().take(120).collect();
                }
                last_msg = trimmed.chars().take(120).collect();
            }
            _ => {}
        }
    }

    if session_id.is_empty() {
        // fallback: 从文件名提取 UUID（rollout-{ts}-{uuid}.jsonl）
        let stem = path.file_stem()?.to_string_lossy().to_string();
        // 取最后一段（uuid 部分）
        if let Some(uuid) = stem.split('-').rev().take(5).collect::<Vec<_>>().into_iter().rev().next() {
            let _ = uuid; // 实际上需要重组
        }
        // 简单处理：取 stem 最后 36 字符作为 id（UUID 标准长度）
        let s = stem.len();
        if s >= 36 {
            session_id = stem[s - 36..].to_string();
        }
    }
    if session_id.is_empty() || cwd.is_empty() {
        return None;
    }

    let short_id = session_id.chars().take(8).collect::<String>();

    // git_root: 使用 git_cache 查询
    let git_root = crate::get_git_root(&cwd, git_cache).unwrap_or_default();

    // 运行状态：纯进程检测，Codex 无 JSONL status 字段
    let ri = running_info.get(&session_id);
    let running = ri.is_some();let status = ri.map(|r| r.status.clone()).unwrap_or_else(|| "idle".into());
    let waiting_for = ri.map(|r| r.waiting_for.clone()).unwrap_or_default();
    let pid = ri.map(|r| r.pid).unwrap_or(0);
    let pty_id = ri.map(|r| r.pty_id.clone()).unwrap_or_default();

    let display_name = if !first_msg.is_empty() {
        first_msg.clone()
    } else {
        short_id.clone()
    };

    let humanize = crate::humanize_duration(now - mtime);
    let mtime_display = {
        let dt = chrono::Local
            .timestamp_opt(mtime, 0)
            .single()
            .unwrap_or_else(chrono::Local::now);
        dt.format("%m-%d %H:%M").to_string()
    };

    Some(SessionMeta {
        session_id,
        short_id,
        cwd: cwd.clone(),
        last_cwd: cwd,
        git_branch,
        user_msg_count: user_count,
        first_user_msg: first_msg,
        last_user_msg: last_msg,
        mtime,
        mtime_display,
        humanize,
        is_worktree: false,
        git_root,
        display_name,
        name_source: "first_msg".into(),
        running,
        status,
        waiting_for,
        pid,
        child_processes: vec![],
        archived: false,
        storage_folder: String::new(),
        pty_id,
        tool: "codex".into(),
    })
}
