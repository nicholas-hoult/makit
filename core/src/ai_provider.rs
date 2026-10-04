use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::TimeZone;

use crate::running::RunningInfo;
use crate::sessions::{get_git_root, humanize_duration, ConversationMessage, SessionMeta};

/// "Which AI tool does this session belong to" -- **an abstraction reserved for unifying the claude / codex paths; so far only half wired up.**
///
/// Currently only `sessions_dir()` is actually used, and only its `Codex` branch (lib.rs when scanning codex sessions).
/// Both claude concerns still go their own way:
///   - directory: `~/.claude/projects` is hardcoded in lib.rs;
///   - resume command: assembled in the frontend's `resumeCmd` in `src/workspace-types.ts`.
/// So the `Claude` variant has never been constructed, and `all()` / `name()` / `spawn_cmd()` have no call sites either,
/// and the compiler rightly reports dead_code. `#[allow(dead_code)]` is attached here rather than deleting them: they are the shapes needed when folding the two paths above in,
/// and deleting them would mean writing them again as-is.
///
/// Finishing this abstraction is tracked as a to-do in the project's task list.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AiTool {
    Claude,
    Codex,
}

impl AiTool {
    #[allow(dead_code)]
    pub fn all() -> &'static [AiTool] {
        &[AiTool::Claude, AiTool::Codex]
    }

    #[allow(dead_code)]
    pub fn name(&self) -> &'static str {
        match self {
            AiTool::Claude => "claude",
            AiTool::Codex => "codex",
        }
    }

    /// The only method in this abstraction with a call site (lib.rs uses its `Codex` branch when scanning codex sessions).
    /// Deliberately no allow: if nobody calls it someday either, a warning is wanted.
    pub fn sessions_dir(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        match self {
            AiTool::Claude => Some(home.join(".claude").join("projects")),
            AiTool::Codex => Some(home.join(".codex").join("sessions")),
        }
    }

    /// The original signature took an extra `cwd` that the body never used -- the command line really does not need it:
    /// the working directory is set when the pty is started, not spliced into `claude -r <id>`. The superfluous parameter was removed,
    /// so nobody implements against it later.
    #[allow(dead_code)]
    pub fn spawn_cmd(&self, session_id: Option<&str>) -> String {
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

// Recursively collect all .jsonl files under the Codex sessions directory (layered by year/month/day)
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

// Parse a single Codex JSONL file -> SessionMeta
pub fn parse_codex_session(
    path: &Path,
    mtime: i64,
    now: i64,
    running_info: &HashMap<String, RunningInfo>,
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
                // extract the text content
                let text = payload
                    .pointer("/content/0/text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // skip injected environment_context messages
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
        // fallback: extract the UUID from the file name (rollout-{ts}-{uuid}.jsonl)
        let stem = path.file_stem()?.to_string_lossy().to_string();
        // take the last segment (the uuid part)
        if let Some(uuid) = stem.split('-').rev().take(5).collect::<Vec<_>>().into_iter().rev().next() {
            let _ = uuid; // actually needs reassembly
        }
        // simple handling: take the last 36 characters of the stem as the id (the standard UUID length)
        let s = stem.len();
        if s >= 36 {
            session_id = stem[s - 36..].to_string();
        }
    }
    if session_id.is_empty() || cwd.is_empty() {
        return None;
    }

    let short_id = session_id.chars().take(8).collect::<String>();

    // git_root: look up via git_cache
    let git_root = get_git_root(&cwd, git_cache).unwrap_or_default();

    // running state: pure process detection, Codex has no JSONL status field
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

    let humanize = humanize_duration(now - mtime);
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

/// Find the file in the Codex session directory by session id (#209).
///
/// Normally a Codex file name ends with the session id (`rollout-<time>-<id>.jsonl`), so look up by file name first;
/// if not found, read each file's first `session_meta` and compare -- this matches how `parse_codex_session` recognizes the id during scanning,
/// and both sides must recognize the same id, otherwise a session is in the list but cannot be found when its details are opened.
pub fn find_codex_session_file(dir: &Path, session_id: &str) -> Option<PathBuf> {
    let files = collect_codex_files(dir);
    if let Some((p, _)) = files.iter().find(|(p, _)| {
        p.file_name().map(|n| n.to_string_lossy().contains(session_id)).unwrap_or(false)
    }) {
        return Some(p.clone());
    }
    for (p, _) in &files {
        let Ok(f) = fs::File::open(p) else { continue };
        let first = BufReader::new(f).lines().map_while(Result::ok).find(|l| !l.trim().is_empty());
        let Some(line) = first else { continue };
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        if rec.get("type").and_then(|v| v.as_str()) == Some("session_meta")
            && rec.pointer("/payload/id").and_then(|v| v.as_str()) == Some(session_id)
        {
            return Some(p.clone());
        }
    }
    None
}

/// Parse a Codex session file into the message list the detail page needs (#209), output in the same structure as on the Claude side.
///
/// - User speech: the `input_text` of `response_item` / `message` / `user`; ones starting with `<environment_context>`
///   or `<user_instructions>` are context injected by Codex itself, not what the user said, and are skipped
/// - Assistant reply: the `output_text` of `response_item` / `message` / `assistant`
/// - Tool call: the `name` of `response_item` / `function_call`. Attached to the immediately preceding assistant message; if what precedes is not an assistant message, it gets its own entry
/// - cwd / branch: present only once in the leading `session_meta`, filled into every message (aligned with the fields on the Claude side)
pub fn parse_codex_messages<R: BufRead>(reader: R) -> Vec<ConversationMessage> {
    let mut out: Vec<ConversationMessage> = Vec::new();
    let (mut cwd, mut branch) = (String::new(), String::new());
    let texts = |payload: &serde_json::Value, kind: &str| -> String {
        payload
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some(kind))
                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    };
    for line in reader.lines().map_while(Result::ok) {
        let Ok(rec) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
        let payload = rec.get("payload").cloned().unwrap_or(serde_json::Value::Null);
        let timestamp = rec.get("timestamp").and_then(|v| v.as_str()).unwrap_or("").to_string();
        match rec.get("type").and_then(|v| v.as_str()).unwrap_or("") {
            "session_meta" => {
                cwd = payload.get("cwd").and_then(|v| v.as_str()).unwrap_or("").to_string();
                branch = payload.pointer("/git/branch").and_then(|v| v.as_str()).unwrap_or("").to_string();
            }
            "response_item" => match payload.get("type").and_then(|v| v.as_str()).unwrap_or("") {
                "message" => {
                    let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                    let text = match role {
                        "user" => texts(&payload, "input_text"),
                        "assistant" => texts(&payload, "output_text"),
                        _ => continue,
                    };
                    let t = text.trim();
                    if t.is_empty() || t.starts_with("<environment_context>") || t.starts_with("<user_instructions>") {
                        continue;
                    }
                    out.push(ConversationMessage {
                        role: role.to_string(),
                        text: t.to_string(),
                        timestamp,
                        cwd: String::new(),
                        git_branch: String::new(),
                        tool_uses: Vec::new(),
                    });
                }
                "function_call" => {
                    let name = payload.get("name").and_then(|v| v.as_str()).unwrap_or("tool").to_string();
                    match out.last_mut() {
                        Some(m) if m.role == "assistant" => m.tool_uses.push(name),
                        _ => out.push(ConversationMessage {
                            role: "assistant".to_string(),
                            text: String::new(),
                            timestamp,
                            cwd: String::new(),
                            git_branch: String::new(),
                            tool_uses: vec![name],
                        }),
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    for m in &mut out {
        m.cwd = cwd.clone();
        m.git_branch = branch.clone();
    }
    out
}

/// Parsing of Codex session details (#209).
///
/// Why it is tested separately: the detail interface used to look only in `~/.claude/projects/`, so opening a Codex session always gave "session not found" --
/// 0.1's README claimed Codex support, and users hit this on the first click. Codex's jsonl is a completely different format from Claude's,
/// and what goes wrong on screen here: a blank detail page, injected content such as `<environment_context>` shown as user speech,
/// or lost tool calls. The test data is made up, with field names following real files (checked 2026-09-24).
#[cfg(test)]
mod codex_detail_tests {
    use super::*;
    use std::io::Cursor;

    const SAMPLE: &str = r#"{"timestamp":"2026-09-24T01:00:00Z","type":"session_meta","payload":{"id":"11111111-2222-3333-4444-555555555555","cwd":"/Users/me/proj","git":{"branch":"main"}}}
{"timestamp":"2026-09-24T01:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/Users/me/proj</cwd>\n</environment_context>"}]}}
{"timestamp":"2026-09-24T01:00:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"帮我看下为什么没生效"}]}}
{"timestamp":"2026-09-24T01:00:03Z","type":"turn_context","payload":{"cwd":"/Users/me/proj","model":"x"}}
{"timestamp":"2026-09-24T01:00:04Z","type":"response_item","payload":{"type":"reasoning","summary":[],"content":null}}
{"timestamp":"2026-09-24T01:00:05Z","type":"response_item","payload":{"type":"function_call","name":"shell","arguments":"{}","call_id":"c1"}}
{"timestamp":"2026-09-24T01:00:06Z","type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"..."}}
{"timestamp":"2026-09-24T01:00:07Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"原因是配置没重新加载。"}]}}
not json at all
{"timestamp":"2026-09-24T01:00:08Z","type":"event_msg","payload":{"type":"token_count"}}
"#;

    #[test]
    fn parses_user_and_assistant_and_skips_injected_context() {
        let msgs = parse_codex_messages(Cursor::new(SAMPLE));
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "assistant"], "消息序列不对: {:?}", msgs.iter().map(|m| (&m.role, &m.text)).collect::<Vec<_>>());
        assert_eq!(msgs[0].text, "帮我看下为什么没生效");
        assert!(msgs.iter().all(|m| !m.text.contains("environment_context")), "注入的环境信息不能当成用户发言");
    }

    #[test]
    fn tool_calls_become_tool_uses_and_cwd_branch_come_from_meta() {
        let msgs = parse_codex_messages(Cursor::new(SAMPLE));
        assert_eq!(msgs[1].tool_uses, vec!["shell".to_string()], "function_call 要变成工具调用");
        assert_eq!(msgs[2].text, "原因是配置没重新加载。");
        for m in &msgs {
            assert_eq!(m.cwd, "/Users/me/proj");
            assert_eq!(m.git_branch, "main");
        }
        assert_eq!(msgs[0].timestamp, "2026-09-24T01:00:02Z");
    }

    #[test]
    fn finds_session_file_by_id_in_name_or_meta() {
        let dir = std::env::temp_dir().join(format!("makit-codex-find-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let day = dir.join("2026/09/24");
        fs::create_dir_all(&day).unwrap();
        // (1) the file name contains the id (Codex's normal naming)
        fs::write(day.join("rollout-2026-09-24T01-00-00-11111111-2222-3333-4444-555555555555.jsonl"), SAMPLE).unwrap();
        // (2) the file name has no id; it can only be recognized by reading session_meta
        fs::write(day.join("renamed.jsonl"),
            r#"{"timestamp":"t","type":"session_meta","payload":{"id":"aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee","cwd":"/x"}}"#).unwrap();
        let a = find_codex_session_file(&dir, "11111111-2222-3333-4444-555555555555");
        let b = find_codex_session_file(&dir, "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
        let c = find_codex_session_file(&dir, "ffffffff-0000-0000-0000-000000000000");
        let _ = fs::remove_dir_all(&dir);
        assert!(a.map(|p| p.to_string_lossy().contains("rollout-")).unwrap_or(false), "按文件名没找到");
        assert!(b.map(|p| p.ends_with("renamed.jsonl")).unwrap_or(false), "按 session_meta 没找到");
        assert!(c.is_none(), "不存在的 id 不该找到任何文件");
    }
}
