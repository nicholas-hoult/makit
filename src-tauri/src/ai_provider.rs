use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::TimeZone;

use crate::{ConversationMessage, SessionMeta};

/// 「这条会话属于哪个 AI 工具」—— **为统一 claude / codex 两条路预留的抽象，目前只接了一半。**
///
/// 现在实际在用的只有 `sessions_dir()`，而且只有 `Codex` 那一支（lib.rs 扫 codex 会话时）。
/// claude 的两件事都还各走各的路：
///   - 目录：`~/.claude/projects` 在 lib.rs 里硬编码；
///   - 恢复命令：在前端 `src/workspace-types.ts` 的 `resumeCmd` 里拼。
/// 所以 `Claude` 这个变体从没被构造过，`all()` / `name()` / `spawn_cmd()` 也没有调用点，
/// 编译器如实报了 dead_code。这里挂 `#[allow(dead_code)]` 而不是删：它们是把上面两条路
/// 收进来时要用的形状，删了下次还得原样写一遍。
///
/// 把这个抽象做完的 todo 记在 docs/任务进度.md 待做里。
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

    /// 这个抽象里唯一有调用点的方法（lib.rs 扫 codex 会话时用 `Codex` 那一支）。
    /// 故意不挂 allow：它要是哪天也没人调了，我想收到警告。
    pub fn sessions_dir(&self) -> Option<PathBuf> {
        let home = dirs::home_dir()?;
        match self {
            AiTool::Claude => Some(home.join(".claude").join("projects")),
            AiTool::Codex => Some(home.join(".codex").join("sessions")),
        }
    }

    /// 原来的签名多带一个 `cwd`，函数体里从没用过 —— 命令行里确实不需要它：
    /// 工作目录是起 pty 时设的，不是拼进 `claude -r <id>` 的。多余的参数删掉，
    /// 免得以后按它去实现。
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

/// 按会话 id 在 Codex 会话目录里找文件（#209）。
///
/// 常规情况下 Codex 的文件名末尾就是会话 id（`rollout-<时间>-<id>.jsonl`），先按文件名找；
/// 找不到再逐个读第一条 `session_meta` 比对 —— 和扫描时 `parse_codex_session` 认 id 的方式一致，
/// 两边认出来的 id 必须是同一个，否则列表里有、点开详情却找不到。
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

/// 把一个 Codex 会话文件解析成详情页要的消息列表（#209），输出和 Claude 那边同一个结构。
///
/// - 用户发言：`response_item` / `message` / `user` 的 `input_text`；开头是 `<environment_context>`
///   或 `<user_instructions>` 的是 Codex 自己注入的上下文，不是用户说的话，跳过
/// - 助手回复：`response_item` / `message` / `assistant` 的 `output_text`
/// - 工具调用：`response_item` / `function_call` 的 `name`。挂到紧邻的助手消息上；前面不是助手消息就单开一条
/// - cwd / 分支：只在开头的 `session_meta` 里有一份，填到每条消息上（和 Claude 那边字段对齐）
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

/// Codex 会话详情的解析（#209）。
///
/// 为什么单独测：详情接口原来只在 `~/.claude/projects/` 里找，Codex 会话一点开就是「找不到 session」——
/// 0.1 在 README 里写了支持 Codex，用户一点就撞。Codex 的 jsonl 和 Claude 完全是两套格式，
/// 这里错了在 UI 上的样子是：详情空白、把 `<environment_context>` 这种注入内容当成用户发言显示、
/// 或者工具调用丢了。测试数据是编的，字段名照本机真实文件（2026-09-24 核对）。
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
        // ① 文件名里带 id（Codex 的常规命名）
        fs::write(day.join("rollout-2026-09-24T01-00-00-11111111-2222-3333-4444-555555555555.jsonl"), SAMPLE).unwrap();
        // ② 文件名不带 id，只能读 session_meta 才认得出
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
