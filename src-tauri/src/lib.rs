use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

mod ai_provider;
mod hook_server;
mod pty;
mod worktree;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProcessInfo {
    pub pid: u32,
    pub ppid: u32,
    pub command: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SessionMeta {
    pub session_id: String,
    pub short_id: String,
    pub cwd: String,
    pub last_cwd: String,
    pub git_branch: String,
    pub user_msg_count: u32,
    pub first_user_msg: String,
    pub last_user_msg: String,
    pub mtime: i64,
    pub mtime_display: String,
    pub humanize: String,
    pub is_worktree: bool,
    pub git_root: String,
    pub display_name: String,
    pub name_source: String,
    pub running: bool,
    pub status: String,
    pub waiting_for: String,
    pub pid: u32,
    pub child_processes: Vec<ProcessInfo>,
    pub archived: bool,
    pub storage_folder: String,
    pub pty_id: String,
    pub tool: String,
}

#[derive(Clone, Debug)]
struct RunningInfo {
    name: String,
    status: String,
    pty_id: String,
    waiting_for: String,
    pid: u32,
}

fn projects_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("projects"))
}

fn encode_project_path(path: &str) -> String {
    path.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}

#[tauri::command]
fn ensure_session_symlink(session_id: String, cwd: String, storage_folder: String) -> Result<String, String> {
    let dir = projects_dir().ok_or("无法定位 home 目录")?;
    let encoded = encode_project_path(&cwd);
    if encoded == storage_folder {
        return Ok("无需处理".into());
    }
    let session_file = format!("{}.jsonl", session_id);
    let target_dir = dir.join(&encoded);
    let target_file = target_dir.join(&session_file);
    if target_file.exists() {
        return Ok("已存在".into());
    }
    let source_file = dir.join(&storage_folder).join(&session_file);
    if !source_file.exists() {
        return Err(format!("源文件不存在: {}/{}", storage_folder, session_file));
    }
    #[cfg(unix)]
    {
        // 目标目录不存在 → symlink 整个目录；已存在 → symlink 单个文件
        if !target_dir.exists() {
            let source_dir = dir.join(&storage_folder);
            std::os::unix::fs::symlink(&source_dir, &target_dir)
                .map_err(|e| format!("创建目录 symlink 失败: {}", e))?;
        } else {
            std::os::unix::fs::symlink(&source_file, &target_file)
                .map_err(|e| format!("创建文件 symlink 失败: {}", e))?;
        }
    }
    Ok(format!("symlink: {}/{}", encoded, session_file))
}

#[tauri::command]
fn list_worktrees(git_root: String) -> Result<Vec<worktree::WorktreeInfo>, String> {
    Ok(worktree::list_worktrees_for(&git_root))
}

fn is_real_user_msg(text: &str) -> bool {
    let t = text.trim_start();
    if t.is_empty() {
        return false;
    }
    if t.starts_with("<system-reminder") {
        return false;
    }
    if t.starts_with("<command-") {
        return false;
    }
    if t.starts_with("[Request interrupted") {
        return false;
    }
    if t.starts_with("Caveat:") {
        return false;
    }
    true
}

fn extract_text(value: &serde_json::Value) -> String {
    if let Some(s) = value.as_str() {
        return s.trim().to_string();
    }
    if let Some(arr) = value.as_array() {
        let mut parts: Vec<String> = Vec::new();
        for item in arr {
            if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                if let Some(t) = item.get("text").and_then(|v| v.as_str()) {
                    parts.push(t.to_string());
                }
            }
        }
        return parts.join(" ").trim().to_string();
    }
    String::new()
}

pub fn humanize_duration(delta_secs: i64) -> String {
    if delta_secs < 0 || delta_secs < 60 {
        return "刚刚".into();
    }
    if delta_secs < 3600 {
        return format!("{}分钟前", delta_secs / 60);
    }
    if delta_secs < 86400 {
        return format!("{}小时前", delta_secs / 3600);
    }
    if delta_secs < 86400 * 30 {
        return format!("{}天前", delta_secs / 86400);
    }
    if delta_secs < 86400 * 365 {
        return format!("{}个月前", delta_secs / (86400 * 30));
    }
    format!("{}年前", delta_secs / (86400 * 365))
}

fn is_worktree_path(cwd: &str) -> bool {
    cwd.contains("/.worktrees/")
        || cwd.ends_with("/.worktrees")
        || cwd.contains("/.claude/worktrees/")
        || cwd.ends_with("/.claude/worktrees")
}

pub fn get_git_root(cwd: &str, cache: &mut HashMap<String, Option<String>>) -> Option<String> {
    cache.entry(cwd.to_string()).or_insert_with(|| detect_git_root(cwd)).clone()
}

fn detect_git_root(cwd: &str) -> Option<String> {
    let mut current = PathBuf::from(cwd);
    for _ in 0..10 {
        let git = current.join(".git");
        if git.is_dir() {
            return Some(current.to_string_lossy().to_string());
        }
        if git.is_file() {
            if let Ok(content) = fs::read_to_string(&git) {
                let trimmed = content.trim();
                if let Some(gitdir) = trimmed.strip_prefix("gitdir: ") {
                    if let Some(pos) = gitdir.find("/.git/worktrees/") {
                        return Some(gitdir[..pos].to_string());
                    }
                }
            }
            return Some(current.to_string_lossy().to_string());
        }
        if !current.pop() { break; }
    }
    None
}

fn resolve_git_root_cached(start_cwd: &str, last_cwd: &str, cwd_mode: &str, cache: &mut HashMap<String, Option<String>>) -> String {
    let start_root = cache.entry(start_cwd.to_string()).or_insert_with(|| detect_git_root(start_cwd)).clone();

    match cwd_mode {
        "start" => return start_root.unwrap_or_default(),
        "last" => {
            let last_root = if !last_cwd.is_empty() {
                cache.entry(last_cwd.to_string()).or_insert_with(|| detect_git_root(last_cwd)).clone()
            } else {
                start_root.clone()
            };
            return last_root.unwrap_or_default();
        }
        _ => {} // "smart" — fall through
    }

    // smart: last 优先，冲突时信任 start
    let last_root = if last_cwd != start_cwd && !last_cwd.is_empty() {
        cache.entry(last_cwd.to_string()).or_insert_with(|| detect_git_root(last_cwd)).clone()
    } else {
        start_root.clone()
    };
    match (&last_root, &start_root) {
        (Some(l), Some(s)) => {
            if l == s { l.clone() } else { s.clone() }
        }
        (Some(l), None) => l.clone(),
        (None, Some(s)) => s.clone(),
        (None, None) => String::new(),
    }
}

fn parse_session(
    path: &Path,
    mtime: i64,
    now: i64,
    running_info: &HashMap<String, RunningInfo>,
    git_cache: &mut HashMap<String, Option<String>>,
    cwd_mode: &str,
) -> Option<SessionMeta> {
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);
    let mut start_cwd = String::new();
    let mut last_cwd = String::new();
    let mut git_branch = String::new();
    let mut user_count: u32 = 0;
    let mut first_msg = String::new();
    let mut last_msg = String::new();
    let mut slug = String::new();
    let mut custom_title = String::new();
    let mut agent_name = String::new();

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
        if slug.is_empty() {
            if let Some(s) = rec.get("slug").and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    slug = s.to_string();
                }
            }
        }
        // claude code 后期写入的真实标题：覆盖 slug
        // type:"custom-title" {customTitle: "..."}  / type:"agent-name" {agentName: "..."}
        // 多次重命名取最后一次
        let rec_type = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if rec_type == "custom-title" {
            if let Some(s) = rec.get("customTitle").and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    custom_title = s.to_string();
                }
            }
            continue;
        }
        if rec_type == "agent-name" {
            if let Some(s) = rec.get("agentName").and_then(|v| v.as_str()) {
                if !s.is_empty() {
                    agent_name = s.to_string();
                }
            }
            continue;
        }
        if rec_type != "user" {
            continue;
        }
        if rec.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        if let Some(c) = rec.get("cwd").and_then(|v| v.as_str()) {
            if !c.is_empty() {
                if start_cwd.is_empty() {
                    start_cwd = c.to_string();
                }
                last_cwd = c.to_string();
            }
        }
        if let Some(gb) = rec.get("gitBranch").and_then(|v| v.as_str()) {
            if !gb.is_empty() && gb != "HEAD" {
                git_branch = gb.to_string();
            }
        }
        let content = rec.pointer("/message/content").cloned().unwrap_or(serde_json::Value::Null);
        let text = extract_text(&content);
        if !is_real_user_msg(&text) {
            continue;
        }
        user_count += 1;
        if first_msg.is_empty() {
            first_msg = text.clone();
        }
        last_msg = text;
    }

    if user_count == 0 {
        return None;
    }

    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    let short_id = session_id.split('-').next().unwrap_or("").to_string();
    let dt: DateTime<Local> = Local.timestamp_opt(mtime, 0).single()?;
    let mtime_display = dt.format("%Y-%m-%d %H:%M").to_string();
    let humanize_str = humanize_duration(now - mtime);
    let cwd = start_cwd.clone();
    let is_worktree = is_worktree_path(&cwd);
    let git_root = resolve_git_root_cached(&cwd, &last_cwd, cwd_mode, git_cache);

    let running_meta = running_info.get(&session_id).cloned();
    let running = running_meta.is_some();
    let rename_name = running_meta
        .as_ref()
        .map(|r| r.name.clone())
        .filter(|s| !s.is_empty());
    // 优先级：rename（运行中重命名）> customTitle / agentName（用户主动改名）> slug（claude 启动时随机）
    let (display_name, name_source) = if let Some(n) = rename_name {
        (n, "rename".into())
    } else if !custom_title.is_empty() {
        (custom_title.clone(), "custom-title".into())
    } else if !agent_name.is_empty() {
        (agent_name.clone(), "agent-name".into())
    } else if !slug.is_empty() {
        (slug.clone(), "slug".into())
    } else {
        (String::new(), "".into())
    };
    let status = running_meta
        .as_ref()
        .map(|r| r.status.clone())
        .unwrap_or_default();
    let waiting_for = running_meta
        .as_ref()
        .map(|r| r.waiting_for.clone())
        .unwrap_or_default();
    let pid = running_meta.as_ref().map(|r| r.pid).unwrap_or(0);
    let pty_id = running_meta.as_ref().map(|r| r.pty_id.clone()).unwrap_or_default();

    let storage_folder = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();

    Some(SessionMeta {
        session_id,
        short_id,
        cwd,
        last_cwd,
        git_branch,
        user_msg_count: user_count,
        first_user_msg: first_msg,
        last_user_msg: last_msg,
        mtime,
        mtime_display,
        humanize: humanize_str,
        is_worktree,
        git_root,
        display_name,
        name_source,
        running,
        status,
        waiting_for,
        pid,
        child_processes: Vec::new(),
        archived: false,
        storage_folder,
        pty_id,
        tool: "claude".into(),
    })
}

fn archived_store_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("makit").join("archived.json"))
}

fn load_archived() -> std::collections::HashSet<String> {
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

fn save_archived(set: &std::collections::HashSet<String>) -> Result<(), String> {
    let path = archived_store_path().ok_or_else(|| "无法定位 home".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut list: Vec<&String> = set.iter().collect();
    list.sort();
    let s = serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?;
    fs::write(&path, s).map_err(|e| e.to_string())
}

#[tauri::command]
fn archive_session(session_id: String) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("session_id 为空".into());
    }
    let mut set = load_archived();
    set.insert(session_id);
    save_archived(&set)
}

#[tauri::command]
fn unarchive_session(session_id: String) -> Result<(), String> {
    if session_id.is_empty() {
        return Err("session_id 为空".into());
    }
    let mut set = load_archived();
    set.remove(&session_id);
    save_archived(&set)
}

fn current_branch_for(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let out = Command::new("git")
        .args(["-C", cwd, "branch", "--show-current"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn load_running_info() -> HashMap<String, RunningInfo> {
    let mut map = HashMap::new();
    let dir = match dirs::home_dir() {
        Some(h) => h.join(".claude").join("sessions"),
        None => return map,
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return map,
    };
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let v: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let session_id = match v.get("sessionId").and_then(|x| x.as_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        let name = v
            .get("name")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let status = v
            .get("status")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let waiting_for = v
            .get("waitingFor")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        // 检查 claude 进程的环境变量获取 CCS_PTY_ID
        let pty_id = if pid > 0 {
            get_env_var_of_pid(pid, "CCS_PTY_ID").unwrap_or_default()
        } else {
            String::new()
        };
        map.insert(
            session_id,
            RunningInfo {
                name,
                status,
                waiting_for,
                pid,
                pty_id,
            },
        );
    }
    map
}

#[cfg(unix)]
fn get_env_var_of_pid(pid: u32, var_name: &str) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-xeww", "-o", "command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let marker = format!("{}=", var_name);
    for part in text.split_whitespace() {
        if let Some(val) = part.strip_prefix(&marker) {return Some(val.to_string());
        }
    }
    // 也检查用空格分隔的情况
    if let Some(pos) = text.find(&marker) {
        let after = &text[pos + marker.len()..];
        let val = after.split_whitespace().next().unwrap_or("");
        if !val.is_empty() {
            return Some(val.to_string());
        }
    }
    None
}

#[cfg(not(unix))]
fn get_env_var_of_pid(_pid: u32, _var_name: &str) -> Option<String> {
    None
}

fn collect_process_table() -> HashMap<u32, (u32, String)> {
    let mut table = HashMap::new();
    let out = match Command::new("ps")
        .args(["-eo", "pid=,ppid=,command="])
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return table,
    };
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let line = line.trim_start();
        let mut parts = line.split_whitespace();
        let pid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let ppid: u32 = match parts.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let cmd: String = parts.collect::<Vec<_>>().join(" ");
        table.insert(pid, (ppid, cmd));
    }
    table
}

// 扫 PPID=1 的孤儿进程，检查 env 里是否有 CCS_SESSION_ID=<session_id>
// 返回 (session_id, ProcessInfo) 列表
fn collect_orphan_by_env(table: &HashMap<u32, (u32, String)>) -> Vec<(String, ProcessInfo)> {
    let orphans: Vec<u32> = table
        .iter()
        .filter(|(_, (ppid, cmd))| {
            *ppid == 1 && {
                let exe = cmd.split_whitespace().next().unwrap_or("");
                let base = exe.rsplit('/').next().unwrap_or(exe);
                // 只扫服务类进程（避免对所有 393 个孤儿跑 ps eww）
                ["java", "python", "python3", "node", "go", "gradle", "mvn", "ruby", "cargo"]
                    .iter()
                    .any(|k| base == *k)
            }
        })
        .map(|(&pid, _)| pid)
        .collect();
    if orphans.is_empty() {
        return Vec::new();
    }
    // 批量 ps eww 拿环境变量
    let pid_args: Vec<String> = orphans.iter().map(|p| p.to_string()).collect();
    let out = match Command::new("ps")
        .arg("eww")
        .arg("-p")
        .arg(pid_args.join(","))
        .output()
    {
        Ok(o) if o.status.success() => o,
        _ => return Vec::new(),
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut result = Vec::new();
    for line in text.lines().skip(1) {
        // 从 env 部分提取 CCS_SESSION_ID=xxx
        if let Some(pos) = line.find("CCS_SESSION_ID=") {
            let after = &line[pos + 15..];
            let session_id = after.split(|c: char| c.is_whitespace() || c == '\0')
                .next()
                .unwrap_or("")
                .to_string();
            if session_id.is_empty() {
                continue;
            }
            // 从行首提取 PID
            let pid: u32 = match line.trim_start().split_whitespace().next().and_then(|s| s.parse().ok()) {
                Some(v) => v,
                None => continue,
            };
            if let Some((ppid, cmd)) = table.get(&pid) {
                result.push((session_id, ProcessInfo {
                    pid,
                    ppid: *ppid,
                    command: cmd.clone(),
                }));
            }
        }
    }
    result
}

fn descendants_of(root: u32, table: &HashMap<u32, (u32, String)>) -> Vec<ProcessInfo> {
    if root == 0 {
        return Vec::new();
    }
    let mut by_parent: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &(ppid, _)) in table.iter() {
        by_parent.entry(ppid).or_default().push(pid);
    }
    let mut out = Vec::new();
    let mut queue = vec![root];
    while let Some(p) = queue.pop() {
        if let Some(children) = by_parent.get(&p) {
            for &c in children {
                if let Some((ppid, cmd)) = table.get(&c) {
                    out.push(ProcessInfo {
                        pid: c,
                        ppid: *ppid,
                        command: cmd.clone(),
                    });
                    queue.push(c);
                }
            }
        }
    }
    out
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConversationMessage {
    pub role: String,
    pub text: String,
    pub timestamp: String,
    pub cwd: String,
    pub git_branch: String,
    pub tool_uses: Vec<String>,
}

fn extract_tool_uses(content: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(arr) = content.as_array() {
        for item in arr {
            if item.get("type").and_then(|v| v.as_str()) == Some("tool_use") {
                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                    out.push(name.to_string());
                }
            }
        }
    }
    out
}

#[tauri::command]
fn read_session_messages(session_id: String) -> Result<Vec<ConversationMessage>, String> {
    let dir = projects_dir().ok_or_else(|| "无法定位 home 目录".to_string())?;
    let mut found_path: Option<PathBuf> = None;
    if let Ok(entries) = fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if !p.is_dir() {
                continue;
            }
            let candidate = p.join(format!("{}.jsonl", session_id));
            if candidate.exists() {
                found_path = Some(candidate);
                break;
            }
        }
    }
    let path = found_path.ok_or_else(|| format!("找不到 session: {}", session_id))?;
    let file = fs::File::open(&path).map_err(|e| e.to_string())?;
    let reader = BufReader::new(file);
    let mut out: Vec<ConversationMessage> = Vec::new();

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
        let typ = rec.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if typ != "user" && typ != "assistant" {
            continue;
        }
        if rec.get("isSidechain").and_then(|v| v.as_bool()) == Some(true) {
            continue;
        }
        let content = rec
            .pointer("/message/content")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let text = extract_text(&content);
        if typ == "user" && !is_real_user_msg(&text) {
            continue;
        }
        let tool_uses = if typ == "assistant" {
            extract_tool_uses(&content)
        } else {
            Vec::new()
        };
        if text.is_empty() && tool_uses.is_empty() {
            continue;
        }
        let timestamp = rec
            .get("timestamp")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let cwd = rec
            .get("cwd")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let git_branch = rec
            .get("gitBranch")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        out.push(ConversationMessage {
            role: typ.to_string(),
            text,
            timestamp,
            cwd,
            git_branch,
            tool_uses,
        });
    }
    Ok(out)
}

#[tauri::command]
fn list_sessions(cwd_mode: Option<String>) -> Result<Vec<SessionMeta>, String> {
    let now = chrono::Local::now().timestamp();
    let running_info = load_running_info();
    let archived_set = load_archived();
    let mode = cwd_mode.as_deref().unwrap_or("smart");

    // ── Phase 1: 收集所有工具的 raw sessions ────────────────────────────
    // 新增工具只需在这里追加一个 scan 块，后处理自动覆盖。
    let mut raw: Vec<SessionMeta> = Vec::new();

    // Claude: ~/.claude/projects/**/*.jsonl
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".claude").join("projects")) {
        if dir.exists() {
            let mut files: Vec<(PathBuf, i64)> = Vec::new();
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if !path.is_dir() || path.read_link().is_ok() { continue; }
                for f in fs::read_dir(&path).into_iter().flatten().flatten() {
                    let p = f.path();
                    if p.extension().and_then(|e| e.to_str()) != Some("jsonl") { continue; }
                    let mtime = f.metadata().and_then(|m| m.modified()).ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64).unwrap_or(0);
                    files.push((p, mtime));
                }
            }
            files.sort_by(|a, b| b.1.cmp(&a.1));
            let mut git_cache: HashMap<String, Option<String>> = HashMap::new();
            for (p, m) in files {
                if let Some(meta) = parse_session(&p, m, now, &running_info, &mut git_cache, mode) {
                    raw.push(meta);
                }
            }
        }
    }

    // Codex: ~/.codex/sessions/**/*.jsonl
    if let Some(codex_dir) = ai_provider::AiTool::Codex.sessions_dir() {
        if codex_dir.exists() {
            let mut codex_files = ai_provider::collect_codex_files(&codex_dir);
            codex_files.sort_by(|a, b| b.1.cmp(&a.1));
            let mut git_cache: HashMap<String, Option<String>> = HashMap::new();
            for (p, m) in codex_files {
                if let Some(meta) = ai_provider::parse_codex_session(&p, m, now, &running_info, &mut git_cache) {
                    raw.push(meta);
                }
            }
        }
    }

    // ── Phase 2: 统一后处理（对所有工具生效）────────────────────────────

    // 归档标记
    for meta in &mut raw {
        if archived_set.contains(&meta.session_id) {
            meta.archived = true;
        }
    }

    // 子进程收集（running session）
    let proc_table = collect_process_table();
    for meta in &mut raw {
        if meta.running && meta.pid > 0 {
            meta.child_processes = descendants_of(meta.pid, &proc_table);
        }
    }

    // 孤儿进程关联（通过 CCS_SESSION_ID 环境变量，PPID=1 的 detach 进程）
    let orphans = collect_orphan_by_env(&proc_table);
    if !orphans.is_empty() {
        for meta in &mut raw {
            if !meta.running { continue; }
            for (env_session_id, proc_info) in &orphans {
                if env_session_id == &meta.session_id
                    && !meta.child_processes.iter().any(|p| p.pid == proc_info.pid)
                {
                    meta.child_processes.push(proc_info.clone());
                }
            }
        }
    }

    // 实时 git 分支覆盖
    let mut branch_cache: HashMap<String, Option<String>> = HashMap::new();
    for meta in &mut raw {
        let cwd_key = if !meta.last_cwd.is_empty() { meta.last_cwd.clone() } else { meta.cwd.clone() };
        if cwd_key.is_empty() { continue; }
        if let Some(b) = branch_cache.entry(cwd_key.clone()).or_insert_with(|| current_branch_for(&cwd_key)).clone() {
            meta.git_branch = b;
        }
    }

    // ── Phase 3: 按 mtime 排序 ───────────────────────────────────────────
    raw.sort_by(|a, b| b.mtime.cmp(&a.mtime));

    Ok(raw)
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ResumeResult {
    pub success: bool,
    pub message: String,
    pub action: String,
}

fn session_marker(session_id: &str) -> String {
    format!("ccs:{}", session_id)
}

fn resolve_对标产品() -> Result<PathBuf, String> {
    let mut search: Vec<PathBuf> = Vec::new();
    if let Ok(path) = std::env::var("PATH") {
        for dir in path.split(':') {
            if !dir.is_empty() {
                search.push(PathBuf::from(dir));
            }
        }
    }
    let known = [
        "/Applications/Transporter.app/Contents/Resources/bin",
        "/Applications/对标产品.app/Contents/Resources/bin",
        "/opt/homebrew/bin",
        "/usr/local/bin",
        "/usr/bin",
    ];
    for p in &known {
        let pb = PathBuf::from(p);
        if !search.contains(&pb) {
            search.push(pb);
        }
    }
    if let Some(home) = dirs::home_dir() {
        let candidates = [
            home.join(".cargo/bin"),
            home.join(".local/bin"),
            home.join(".对标产品/bin"),
        ];
        for c in candidates {
            if !search.contains(&c) {
                search.push(c);
            }
        }
    }
    for dir in &search {
        let candidate = dir.join("对标产品");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    // 兜底：调登录 shell 取 PATH
    if let Ok(out) = Command::new("zsh")
        .args(["-lic", "command -v 对标产品"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                let pb = PathBuf::from(&s);
                if pb.is_file() {
                    return Ok(pb);
                }
            }
        }
    }
    Err(format!(
        "找不到 对标产品 命令，尝试过的目录: {}",
        search
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn find_existing_workspace(对标产品_path: &Path, session_id: &str) -> Option<String> {
    let output = Command::new(对标产品_path)
        .args(["list-workspaces", "--json"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let ws = v.get("workspaces")?.as_array()?;
    let marker = session_marker(session_id);
    for w in ws {
        if let Some(desc) = w.get("description").and_then(|d| d.as_str()) {
            if desc == marker {
                return w.get("ref").and_then(|r| r.as_str()).map(|s| s.to_string());
            }
        }
    }
    None
}

#[tauri::command]
fn resume_session(session_id: String, cwd: String) -> ResumeResult {
    if session_id.is_empty() || cwd.is_empty() {
        return ResumeResult {
            success: false,
            message: "session_id 或 cwd 为空".into(),
            action: "error".into(),
        };
    }

    let 对标产品_path = match resolve_对标产品() {
        Ok(p) => p,
        Err(e) => {
            return ResumeResult {
                success: false,
                message: e,
                action: "error".into(),
            }
        }
    };

    if let Some(ws_ref) = find_existing_workspace(&对标产品_path, &session_id) {
        let output = Command::new(&对标产品_path)
            .args(["select-workspace", "--workspace", &ws_ref])
            .output();
        return match output {
            Ok(o) if o.status.success() => ResumeResult {
                success: true,
                message: format!("跳转到现有 workspace {}", ws_ref),
                action: "focused".into(),
            },
            Ok(o) => ResumeResult {
                success: false,
                message: format!("对标产品 select 失败: {}", String::from_utf8_lossy(&o.stderr)),
                action: "error".into(),
            },
            Err(e) => ResumeResult {
                success: false,
                message: format!("无法调用 对标产品 select: {}", e),
                action: "error".into(),
            },
        };
    }

    let short_id = session_id.split('-').next().unwrap_or("").to_string();
    let name = format!("ccs-{}", short_id);
    let description = session_marker(&session_id);
    let command_text = format!("claude -r {}", session_id);
    let output = Command::new(&对标产品_path)
        .arg("new-workspace")
        .arg("--name")
        .arg(&name)
        .arg("--description")
        .arg(&description)
        .arg("--cwd")
        .arg(&cwd)
        .arg("--command")
        .arg(&command_text)
        .arg("--focus")
        .arg("true")
        .output();
    match output {
        Ok(o) if o.status.success() => ResumeResult {
            success: true,
            message: format!("已在 对标产品 新开 {}", name),
            action: "created".into(),
        },
        Ok(o) => ResumeResult {
            success: false,
            message: format!(
                "对标产品 new-workspace 退出码 {}: {}",
                o.status,
                String::from_utf8_lossy(&o.stderr)
            ),
            action: "error".into(),
        },
        Err(e) => ResumeResult {
            success: false,
            message: format!("无法调用 对标产品: {}", e),
            action: "error".into(),
        },
    }
}

// 按需归因：扫 ~/.claude/projects/*/*.jsonl，找在 tab 启动之后「新建」(birthtime > after_ts) 且 cwd 匹配的
// 用 birthtime 而非 mtime —— mtime 会被任何写入刷新，旧 session 也会被误命中
// 用途：new/shell tab 启动后想反查到 claude 写出的 session 文件
#[tauri::command]
fn find_session_in_cwd_after(cwd: String, after_ts: i64) -> Result<Option<String>, String> {
    let dir = projects_dir().ok_or_else(|| "无法定位 home 目录".to_string())?;
    if !dir.exists() {
        return Ok(None);
    }
    let mut candidates: Vec<(PathBuf, i64)> = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let sub = match fs::read_dir(&p) {
            Ok(s) => s,
            Err(_) => continue,
        };
        for f in sub {
            let f = match f {
                Ok(f) => f,
                Err(_) => continue,
            };
            let fp = f.path();
            if fp.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let meta = match f.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            // 优先用 birthtime（macOS APFS 支持）；不支持时退化到 mtime
            let ctime = match meta.created() {
                Ok(t) => t
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                Err(_) => match meta.modified() {
                    Ok(t) => t
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                    Err(_) => 0,
                },
            };
            // 给 1s 余量：tab.startedAt 取秒级，文件 birthtime 也是秒级，可能同秒
            if ctime + 1 < after_ts {
                continue;
            }
            candidates.push((fp, ctime));
        }
    }
    // 创建时间新的优先匹配
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    for (p, _mtime) in &candidates {
        let file = match fs::File::open(p) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        // 扫前 30 行，找任何带 cwd 字段的记录
        // claude 启动期间 SessionStart hook 写的 attachment 行就含 cwd，不必等 user 行
        for (i, line) in reader.lines().enumerate() {
            if i >= 30 {
                break;
            }
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
            if let Some(c) = rec.get("cwd").and_then(|v| v.as_str()) {
                if !c.is_empty() {
                    if c == cwd {
                        let session_id = p
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("")
                            .to_string();
                        if !session_id.is_empty() {
                            return Ok(Some(session_id));}
                    }
                    // cwd 不匹配，本文件无希望
                    break;
                }
            }
        }
    }
    Ok(None)
}

// 增量读单个 session 的 meta，避免归因后重拉整个 list_sessions（用户可见的卡顿主要来自后者）
#[tauri::command]
fn read_session_meta(session_id: String) -> Result<Option<SessionMeta>, String> {
    let dir = projects_dir().ok_or_else(|| "无法定位 home 目录".to_string())?;
    if !dir.exists() {
        return Ok(None);
    }
    let now = chrono::Local::now().timestamp();
    let running_info = load_running_info();
    let archived_set = load_archived();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())? {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let p = entry.path();
        if !p.is_dir() {
            continue;
        }
        let jsonl = p.join(format!("{}.jsonl", session_id));
        if !jsonl.exists() {
            continue;
        }
        let mtime = match jsonl.metadata().and_then(|m| m.modified()) {
            Ok(t) => t
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            Err(_) => 0,
        };
        let mut git_cache: HashMap<String, Option<String>> = HashMap::new();
        if let Some(mut meta) = parse_session(&jsonl, mtime, now, &running_info, &mut git_cache, "smart") {
            if archived_set.contains(&meta.session_id) {
                meta.archived = true;
            }
            return Ok(Some(meta));
        }
    }
    Ok(None)
}

// Zellij 集成：通过 CLI 控制 zellij session
// 在 macOS 上打开本地路径：默认在 Finder 中显示（reveal），文件用默认 app 打开
// reveal=true → open -R（Finder 高亮），false → open（用默认 app 打开）
#[tauri::command]
async fn open_path(path: String, reveal: bool) -> Result<(), String> {
    // 展开 ~ 为 $HOME
    let expanded = if path.starts_with("~/") || path == "~" {
        let home = std::env::var("HOME").map_err(|_| "无法获取 HOME".to_string())?;
        if path == "~" {
            home
        } else {
            format!("{}/{}", home, &path[2..])
        }
    } else {
        path.clone()
    };
    let p = std::path::Path::new(&expanded);
    if !p.exists() {
        return Err(format!("路径不存在: {}", expanded));
    }
    let mut cmd = Command::new("open");
    if reveal {
        cmd.arg("-R");
    }
    cmd.arg(&expanded);
    cmd.spawn().map_err(|e| format!("open 失败: {}", e))?;
    Ok(())
}

#[tauri::command]
async fn zellij_action(action: String, args: Vec<String>) -> Result<String, String> {
    let zellij = find_zellij_binary();
    let mut cmd_args = vec!["action".to_string(), action];
    cmd_args.extend(args);
    let output = Command::new(&zellij)
        .args(&cmd_args)
        .arg("--session")
        .arg("ccs-main")
        .output()
        .map_err(|e| format!("zellij action failed: {}", e))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).to_string())
    }
}

#[tauri::command]
async fn zellij_ensure_running() -> Result<String, String> {
    let zellij = find_zellij_binary();
    // 检查 session 是否存在
    let check = Command::new(&zellij)
        .args(["list-sessions"])
        .output()
        .map_err(|e| e.to_string())?;
    let sessions = String::from_utf8_lossy(&check.stdout);
    if !sessions.contains("ccs-main") {
        return Err("zellij session 'ccs-main' not running. Please start it manually: zellij --session ccs-main".to_string());
    }
    // 检查 web server
    let web_check = Command::new(&zellij)
        .args(["web", "--status"])
        .output()
        .map_err(|e| e.to_string())?;
    let web_status = String::from_utf8_lossy(&web_check.stdout);
    if web_status.contains("offline") {
        // 启动 web server
        Command::new(&zellij)
            .args(["web", "--start", "--port", "8082", "--daemonize"])
            .output()
            .map_err(|e| e.to_string())?;
    }
    Ok("ok".to_string())
}

fn find_zellij_binary() -> String {
    // 优先 PATH，然后常见位置
    for path in &[
        "/opt/homebrew/bin/zellij",
        "/usr/local/bin/zellij",
        "/usr/bin/zellij",
    ] {
        if std::path::Path::new(path).exists() {
            return path.to_string();
        }
    }
    "zellij".to_string()
}

// fs.watch 增量推送：
// - sessions/ 变化 → emit "running-changed"（只需重读运行状态，轻量）
// - projects/ 变化 → emit "sessions-changed"（全量刷新，但低频——只在新 session 创建时触发）
fn start_session_watcher(app_handle: tauri::AppHandle) {
    use notify_debouncer_mini::{new_debouncer, notify::RecursiveMode};
    use tauri::Emitter;
    use std::time::Duration;

    let home = match dirs::home_dir() { Some(h) => h, None => return };
    let sessions_dir = home.join(".claude").join("sessions");
    let projects_dir = home.join(".claude").join("projects");
    let codex_sessions_dir = home.join(".codex").join("sessions");

    let _ = std::fs::create_dir_all(&sessions_dir);
    let _ = std::fs::create_dir_all(&projects_dir);

    let sessions_dir_clone = sessions_dir.clone();

    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = match new_debouncer(Duration::from_millis(500), tx) {
            Ok(d) => d,
            Err(e) => { eprintln!("watcher init failed: {}", e); return; }
        };
        let _ = debouncer.watcher().watch(&sessions_dir, RecursiveMode::NonRecursive);
        let _ = debouncer.watcher().watch(&projects_dir, RecursiveMode::Recursive);
        if codex_sessions_dir.exists() {
            let _ = debouncer.watcher().watch(&codex_sessions_dir, RecursiveMode::Recursive);
        }

        for result in rx {
            if let Ok(events) = result {
                let mut has_session_event = false;
                let mut has_project_event = false;
                for ev in &events {
                    if ev.path.starts_with(&sessions_dir_clone) {
                        has_session_event = true;
                    } else {
                        has_project_event = true;
                    }
                }
                // sessions/ 变化 = running 状态更新（轻量：前端只刷 running 状态）
                if has_session_event {
                    let _ = app_handle.emit("running-changed", ());
                }
                // projects/ 变化 = 新 session 产生（低频：前端全量刷新）
                if has_project_event {
                    let _ = app_handle.emit("sessions-changed", ());
                }
            }
        }
    });
}

// 轻量扫描：只读 running 状态（~/.claude/sessions/*.json），不扫 projects
#[tauri::command]
fn list_running_sessions() -> Vec<RunningMeta> {
    let home = match dirs::home_dir() { Some(h) => h, None => return vec![] };
    let sessions_dir = home.join(".claude").join("sessions");
    let mut result = vec![];
    if let Ok(entries) = std::fs::read_dir(&sessions_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map_or(true, |e| e != "json") { continue; }
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    let session_id = v.get("sessionId").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let waiting_for = v.get("waitingFor").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
                    if !session_id.is_empty() {
                        result.push(RunningMeta { session_id, status, waiting_for, pid });
                    }
                }
            }
        }
    }
    result
}

#[derive(serde::Serialize)]
struct RunningMeta {
    session_id: String,
    status: String,
    waiting_for: String,
    pid: u32,
}

#[tauri::command]
fn show_notification(title: String, body: String) {
    let script = format!(
        "display notification \"{}\" with title \"{}\" sound name \"Glass\"",
        body.replace('"', "'"),
        title.replace('"', "'")
    );
    let _ = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .spawn();
}

/// 获取 AI 工具 logo（base64 data URL）：本地缓存优先，不存在则下载
#[tauri::command]
async fn get_tool_logo(tool: String) -> Result<String, String> {
    let (url, ext, mime) = match tool.as_str() {
        "claude" => ("https://www.anthropic.com/favicon.ico", "ico", "image/x-icon"),
        "codex"  => ("https://openai.com/favicon.ico", "ico", "image/x-icon"),
        other    => return Err(format!("unknown tool: {}", other)),
    };

    let cache_dir = dirs::home_dir()
        .ok_or("no home dir")?
        .join(".claude").join("makit").join("logos");
    fs::create_dir_all(&cache_dir).map_err(|e| e.to_string())?;

    let local_path = cache_dir.join(format!("{}.{}", tool, ext));

    let bytes: Vec<u8> = if local_path.exists() {
        fs::read(&local_path).map_err(|e| e.to_string())?
    } else {
        let b = reqwest::get(url).await
            .map_err(|e| e.to_string())?
            .bytes().await
            .map_err(|e| e.to_string())?
            .to_vec();
        fs::write(&local_path, &b).map_err(|e| e.to_string())?;
        b
    };

    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{};base64,{}", mime, encoded))
}

/// 创建 hook 脚本并注入 ~/.claude/settings.json 的 Notification hook
#[tauri::command]
fn install_claude_hook() -> Result<String, String> {
    let home = dirs::home_dir().ok_or("无法定位 home 目录")?;

    // 1. write hook script
    let hooks_dir = home.join(".claude").join("hooks");
    fs::create_dir_all(&hooks_dir).map_err(|e| e.to_string())?;

    let hook_script = hooks_dir.join("ccs-hook.sh");
    let sock_path = home.join(".claude").join("makit").join("hook.sock");
    let script = format!(
        "#!/bin/sh\nSOCK=\"{}\"\nif [ -S \"$SOCK\" ]; then\n    cat | nc -U \"$SOCK\" 2>/dev/null\nelse\n    echo '{{}}'\nfi\n",
        sock_path.display()
    );
    fs::write(&hook_script, &script).map_err(|e| e.to_string())?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_script).map_err(|e| e.to_string())?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_script, perms).map_err(|e| e.to_string())?;
    }

    // 2. update ~/.claude/settings.json
    let settings_path = home.join(".claude").join("settings.json");
    let mut settings: serde_json::Value = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path).map_err(|e| e.to_string())?;
        serde_json::from_str(&raw).unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // check if already installed
    let already = settings
        .pointer("/hooks/Notification")
        .and_then(|n| n.as_array())
        .map(|groups| {
            groups.iter().any(|g| {
                g.get("hooks")
                    .and_then(|h| h.as_array())
                    .map(|cmds| {
                        cmds.iter().any(|c| {
                            c.get("command")
                                .and_then(|v| v.as_str())
                                .map(|s| s.contains("ccs-hook.sh"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);

    if already {
        return Ok("already_installed".into());
    }

    let hook_cmd = format!("{} 2>/dev/null || echo '{{}}'", hook_script.display());
    let new_group = serde_json::json!({
        "hooks": [{"type": "command", "command": hook_cmd}]
    });

    let hooks = settings
        .as_object_mut()
        .ok_or("settings.json 格式错误")?
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let notif = hooks
        .as_object_mut()
        .ok_or("hooks 格式错误")?
        .entry("Notification")
        .or_insert_with(|| serde_json::json!([]));

    notif
        .as_array_mut()
        .ok_or("Notification 不是数组")?
        .push(new_group);

    let out = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    fs::write(&settings_path, out).map_err(|e| e.to_string())?;

    Ok("installed".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_shell::init())
        .manage(pty::PtyState::default())
        .setup(|app| {
            // 清理已废弃的 session-index.json（v2 扁平化方案不再使用）
            if let Some(home) = dirs::home_dir() {
                let legacy = home.join(".claude").join("makit").join("session-index.json");
                if legacy.exists() {
                    let _ = std::fs::remove_file(&legacy);
                }
            }
            start_session_watcher(app.handle().clone());
            hook_server::start(app.handle().clone());

            // 监听主窗口焦点变化，emit 给前端
            use tauri::Manager;
            if let Some(win) = app.handle().get_webview_window("main") {
                let win2 = win.clone();
                win.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(focused) = event {
                        use tauri::Emitter;
                        let _ = win2.emit("window-focus-changed", focused);
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_sessions,
            resume_session,
            read_session_messages,
            archive_session,
            unarchive_session,
            find_session_in_cwd_after,
            read_session_meta,
            pty::pty_spawn,
            pty::pty_write,
            pty::pty_resize,
            pty::pty_kill,
            pty::kill_pids,
            zellij_action,
            zellij_ensure_running,
            open_path,
            list_running_sessions,
            ensure_session_symlink,
            list_worktrees,
            show_notification,
            install_claude_hook,
            get_tool_logo
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // app 退出 / 窗口关闭时杀掉所有 PTY 进程组并退出
    // macOS：红叉关窗不退出 app（Dock 还在），必须主动 exit 确保进程清理
    app.run(|app_handle, event| {
        use tauri::{Emitter, Manager};
        match event {
            tauri::RunEvent::ExitRequested { .. } => {
                let state = app_handle.state::<pty::PtyState>();
                pty::kill_all_ptys(&state);
            }
            tauri::RunEvent::WindowEvent { event: tauri::WindowEvent::CloseRequested { .. }, .. } => {
                let state = app_handle.state::<pty::PtyState>();
                pty::kill_all_ptys(&state);
                app_handle.exit(0);
            }
            _ => {}
        }
    });
}
