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

/// claude 的存储键：把 cwd 逐字符编码成 `~/.claude/projects/` 下的目录名。
///
/// **先 canonicalize 再编码**。claude 是拿自己进程的 cwd 算这个键的，而进程 cwd 永远是
/// 解析过软链的实路径（macOS 上 `/var` = `/private/var`、`/tmp` = `/private/tmp`）。
/// 不解析就会算出一个 claude 永远不会用的键 —— 建 symlink 会建到错的地方，「恢复会话」
/// 静默失效（实测踩过，见 `session_recovery_tests`）。
///
/// 路径不存在时 canonicalize 失败，退回原样编码。这正是「原目录已被删」的场合，而那种
/// 场合退回原样是对的：jsonl 里记的 cwd 本身就是 claude 当年写下的实路径。
fn encode_project_path(path: &str) -> String {
    let resolved = std::fs::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_string());
    resolved
        .chars()
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

/// 枚举 `projects/<key>/` 里的 transcript 时该不该扫这个条目。
///
/// 除了扩展名，还必须**跳过 symlink**。「指到新位置」会让同一份 transcript 在两个键下
/// 可见（原存储目录 + 新 cwd 的键），只看扩展名的话同一个 session_id 会被扫成两条，
/// 侧栏里冒出一对孪生会话。目录级 symlink 本来就被跳过（见 `list_sessions` 里那段注释），
/// 这里把文件级补上，两者规则一致：**只认真身，软链一律不算**。
fn is_scannable_jsonl(entry: &fs::DirEntry) -> bool {
    if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return false;
    }
    entry.file_type().map(|t| t.is_file()).unwrap_or(false)
}

#[derive(Serialize, Debug)]
struct RecoveredSession {
    /// 恢复之后该用哪个 cwd 去 resume
    cwd: String,
    /// 到底动了什么（要能对用户交代清楚，不能只说"成功"）
    detail: String,
}

/// 启动目录被删之后恢复会话。`projects_root` 可注入，测试才不会写进真的 `~/.claude`。
///
/// 本质：会话数据一个字节都没丢，丢的只是**一把钥匙**。`claude -r <id>` 只会去
/// `projects/<encode(realpath(cwd))>/<id>.jsonl` 找，目录被删就意味着你再也算不出
/// 那个键。所以两个动作都只做一件事 —— **让钥匙对上**：
///
/// - `recreate`：把原目录建回来（空的）。键本来就是按原路径算的，建回来就自然对上，
///   `projects/` 一个字节都不用碰。代码没了，但会话能接着聊。
/// - `relink`：代码搬家了，指到新目录。在新键下放一个指向原 transcript 的
///   **文件级** symlink。不用目录级：那会把老项目下所有会话一并暴露到新键下。
///
/// 两种模式都**绝不拷贝** jsonl —— 拷贝会产出同一个 session id 的两份分叉。
fn recover_session_cwd_in(
    projects_root: &Path,
    mode: &str,
    session_id: &str,
    original_cwd: &str,
    target_cwd: &str,
    storage_folder: &str,
) -> Result<RecoveredSession, String> {
    match mode {
        "recreate" => {
            // 只补**最后一级**，父目录必须已经存在。用 `create_dir_all` 会把缺失的父目录
            // 全部造出来，而「一整棵父树都没了」通常不是「删了一个项目目录」，是外置盘
            // 没挂载 / 整个上级被搬走。那种情况下在挂载点上造一个真目录后果很实：
            // 那块盘之后会被 macOS 挂成「X 1」，而且这事没有任何提示。
            let parent = Path::new(original_cwd).parent();
            match parent {
                Some(p) if !p.as_os_str().is_empty() && !p.is_dir() => {
                    return Err(format!(
                        "父目录也不存在：{}。这看起来不只是删掉了一个项目目录（可能是外置盘没挂载，或整个上级被搬走了），\
                         没有替你把整棵目录树造出来。挂上盘、或改用「指到新位置」",
                        p.display()
                    ));
                }
                _ => {}
            }
            fs::create_dir_all(original_cwd).map_err(|e| format!("重建目录失败: {}", e))?;
            Ok(RecoveredSession {
                cwd: original_cwd.to_string(),
                detail: format!("已重建空目录 {}（未改动 ~/.claude）", original_cwd),
            })
        }
        "relink" => {
            if !Path::new(target_cwd).is_dir() {
                return Err(format!("目标目录不存在: {}", target_cwd));
            }
            // codex 的会话按**日期**分层存在 `~/.codex/sessions/年/月/日/` 下，**不是 cwd 键**
            // （`ai_provider.rs` 里 codex 的 `storage_folder` 恒为空串就是这个意思）。
            // 也就是说 codex 根本没有这把钥匙可丢 —— `codex resume <id>` 在任何目录都能找到会话。
            // 所以这里没有键要修，「指到新位置」就只是换个工作目录，一个 symlink 都不该建。
            if storage_folder.is_empty() {
                return Ok(RecoveredSession {
                    cwd: target_cwd.to_string(),
                    detail: format!("已切到 {}（这个工具的会话不按目录索引，无需改动存储）", target_cwd),
                });
            }
            let session_file = format!("{}.jsonl", session_id);
            let source = projects_root.join(storage_folder).join(&session_file);
            // 先确认源在，再动手 —— 否则会留下一个悬空 symlink，比报错更难查
            if !source.exists() {
                return Err(format!("找不到会话记录: {}/{}", storage_folder, session_file));
            }
            let key = encode_project_path(target_cwd);
            if key == storage_folder {
                return Ok(RecoveredSession {
                    cwd: target_cwd.to_string(),
                    detail: "钥匙本来就对得上，无需处理".into(),
                });
            }
            let target_dir = projects_root.join(&key);
            // 真目录 + 里面放文件级 symlink，不建目录级 symlink
            fs::create_dir_all(&target_dir).map_err(|e| format!("创建目录失败: {}", e))?;
            let target_file = target_dir.join(&session_file);
            if !target_file.exists() {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&source, &target_file)
                    .map_err(|e| format!("创建 symlink 失败: {}", e))?;
            }
            Ok(RecoveredSession {
                cwd: target_cwd.to_string(),
                detail: format!("已在 {}/ 下建软链指向原会话记录", key),
            })
        }
        other => Err(format!("未知恢复方式: {}", other)),
    }
}

/// 目录还在不在。刻意**不做**成 `SessionMeta.cwd_exists`：`list_sessions` 里每次 stat
/// 本机实测约 0.7ms、300 多个文件合计约 0.5s，已经是现存的性能痛点（#144），
/// 每个会话再加一次 stat 是往已知热路径上加钱。而这个信息只有「用户点开会话的那一刻」
/// 才用得到，那时候一次 stat 就够。
#[tauri::command]
fn dir_exists(path: String) -> bool {
    !path.is_empty() && Path::new(&path).is_dir()
}

#[tauri::command]
fn recover_session_cwd(
    mode: String,
    session_id: String,
    original_cwd: String,
    target_cwd: String,
    storage_folder: String,
) -> Result<RecoveredSession, String> {
    let root = projects_dir().ok_or("无法定位 home 目录")?;
    recover_session_cwd_in(&root, &mode, &session_id, &original_cwd, &target_cwd, &storage_folder)
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
        // claude code 写入的真实标题。多次重命名取最后一次。
        // type:"custom-title" {customTitle: "..."}  / type:"agent-name" {agentName: "..."}
        //
        // 这里**故意不读** `slug`。claude 的 slug 是 `clever-swimming-flute` 这种随机三词
        // 代号，只写在 `type:"system", subtype:"compact_boundary"` 记录上（实测 8 个带
        // slug 的会话 8 个都是这个来源），和会话内容没有任何关系。它一旦进了 display_name，
        // 前端的 `display_name || first_user_msg` 就会优先它，结果是**会话被 compact 一次，
        // 标题就从首条消息退化成随机词组**。没有真标题时留空更好：交给前端兜到首条消息。
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
    // 优先级：rename（运行中重命名）> customTitle / agentName（claude 写的真标题）> 留空。
    // 留空不是「没名字」，是「这里没有比首条用户消息更好的名字」—— 前端拿
    // `display_name || first_user_msg` 兜。而 first_user_msg 一定非空：上面
    // `user_count == 0` 直接 return None，而 user_count 和 first_msg 在同一个分支里赋值。
    let (display_name, name_source) = if let Some(n) = rename_name {
        (n, "rename".into())
    } else if !custom_title.is_empty() {
        (custom_title.clone(), "custom-title".into())
    } else if !agent_name.is_empty() {
        (agent_name.clone(), "agent-name".into())
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

/// 找到 cwd 所属仓库的 git 目录。`.git` 是目录就是它自己；是文件则跟着里面的
/// `gitdir:` 跳走（linked worktree 指向 `<主仓>/.git/worktrees/<名>`，submodule
/// 指向 `<父仓>/.git/modules/<名>`），两种情况下 HEAD 都在跳到的那个目录里。
///
/// 和 `detect_git_root` 不是一回事：那个要的是"归属哪个项目"，worktree 会被折回主仓根；
/// 这里要的是"HEAD 文件在哪"，必须停在 worktree 自己的 git 目录上。
fn git_dir_for(cwd: &str) -> Option<PathBuf> {
    let mut current = PathBuf::from(cwd);
    for _ in 0..10 {
        let git = current.join(".git");
        if git.is_dir() {
            return Some(git);
        }
        if git.is_file() {
            let content = fs::read_to_string(&git).ok()?;
            let gitdir = PathBuf::from(content.trim().strip_prefix("gitdir: ")?);
            return Some(if gitdir.is_absolute() { gitdir } else { current.join(gitdir) });
        }
        if !current.pop() {
            break;
        }
    }
    None
}

/// 等价于 `git branch --show-current`，但不 fork 子进程 —— 那条命令做的事就是
/// 读 `.git/HEAD` 再剥掉 `refs/heads/` 前缀，一次文件读足够。
///
/// 实测：每次 fork+exec 约 38ms，189 个 session 摊到几十个不同目录就是 2.3s，
/// 占 list_sessions 总耗时的 65% —— 整个 session 列表的首屏延迟主要是这个。
/// detached HEAD 时 HEAD 里是裸 SHA、没有 `ref: ` 前缀，返回 None，
/// 和 `--show-current` 输出空串的行为一致。
fn current_branch_for(cwd: &str) -> Option<String> {
    if cwd.is_empty() {
        return None;
    }
    let head = fs::read_to_string(git_dir_for(cwd)?.join("HEAD")).ok()?;
    let name = head.trim().strip_prefix("ref: refs/heads/")?;
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
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
    // flag 只能是 `-Eww`：macOS 的 ps 里 `-e` 是 `-A` 的同义词（显示所有进程），
    // 显示环境变量的是大写 `-E`；`-x`（把无控制终端的进程并进来）更不能加 ——
    // BSD ps 的选择条件是 OR，加一个就把 `-p` 淹掉，返回整张进程表。
    // 两者叠在一起的老写法 `-xeww` 是"756 行、零个环境变量"，恒定返回 None。
    // `-ww` 保留：不加会按终端宽度截断，环境变量正好在末尾，最先被切掉。
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-Eww", "-o", "command="])
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

/// items 分给 N 个线程抢着做。每个线程先建一份自己的 S（线程本地缓存之类），
/// 再对抢到的每个 item 调 f，结果推进自己的 Vec，最后按线程顺序拼起来。
///
/// 用原子游标抢任务而不是按下标均分：两个调用点的 item 耗时都差几个数量级
/// （jsonl 最大 95MB、多数几十 KB；项目目录里的文件数从 1 到上百），
/// 均分下标会让拿到最重那一份的线程单独决定总耗时。
fn parallel_scan<T, S, R>(
    items: &[T],
    mk_state: impl Fn() -> S + Sync,
    f: impl Fn(&T, &mut S, &mut Vec<R>) + Sync,
) -> Vec<R>
where
    T: Sync,
    S: Send,
    R: Send,
{
    use std::sync::atomic::{AtomicUsize, Ordering};
    if items.is_empty() {
        return Vec::new();
    }
    let nthreads = std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(4)
        .min(items.len());
    let cursor = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..nthreads)
            .map(|_| {
                let (cursor, mk_state, f) = (&cursor, &mk_state, &f);
                scope.spawn(move || {
                    let mut state = mk_state();
                    let mut out = Vec::new();
                    loop {
                        let i = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        f(item, &mut state, &mut out);
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    })
}

/// 分阶段计时。默认完全静默（一次 env 读 + 一次 bool 判断），
/// 需要看数字时跑 `MAKIT_TIMING=1`。没有它就只能靠"感觉哪里慢"猜。
struct PhaseTimer {
    on: bool,
    start: std::time::Instant,
    last: std::time::Instant,
}
impl PhaseTimer {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self { on: std::env::var_os("MAKIT_TIMING").is_some(), start: now, last: now }
    }
    fn mark(&mut self, label: &str) {
        if !self.on { return; }
        let now = std::time::Instant::now();
        eprintln!("[timing] {:<22} {:>8.1}ms  (累计 {:.1}ms)",
            label,
            now.duration_since(self.last).as_secs_f64() * 1000.0,
            now.duration_since(self.start).as_secs_f64() * 1000.0);
        self.last = now;
    }
}

#[tauri::command]
fn list_sessions(cwd_mode: Option<String>) -> Result<Vec<SessionMeta>, String> {
    let mut timer = PhaseTimer::new();
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
            // file_type() 用的是 readdir 直接给出的 d_type，不额外走 syscall；
            // 原来的 `path.is_dir() || path.read_link().is_ok()` 是 stat + readlink 两次。
            // 语义等价：符号链接目录在两种写法下都被跳过（is_dir 跟随链接后为真但 read_link 也为真；
            // file_type 不跟随链接，符号链接的 is_dir 直接为假）。
            let project_dirs: Vec<PathBuf> = fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect();
            // 这一段刻意保持串行。mtime 只能靠 stat 拿，本机实测每次约 0.7ms
            // （远超正常的微秒级，内核态大概被 EDR / Spotlight 插了一手），
            // 308 个文件合计约 0.5s。但试过按项目目录切给多线程：495–890ms，比串行更抖、
            // 均值没降 —— 这部分不是 CPU 受限，是在内核里被串行化了，加线程只是加噪声。
            // 真正的出路是持久化 mtime 缓存（#144），不在本次范围内。
            let mut files: Vec<(PathBuf, i64)> = Vec::new();
            for proj in &project_dirs {
                for f in fs::read_dir(proj).into_iter().flatten().flatten() {
                    if !is_scannable_jsonl(&f) { continue; }
                    let p = f.path();
                    let mtime = f.metadata().and_then(|m| m.modified()).ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_secs() as i64).unwrap_or(0);
                    files.push((p, mtime));
                }
            }
            files.sort_by(|a, b| b.1.cmp(&a.1));
            timer.mark("claude 枚举文件");
            raw.extend(parallel_scan(
                &files,
                HashMap::<String, Option<String>>::new,
                |(p, m), git_cache, out| {
                    if let Some(meta) = parse_session(p, *m, now, &running_info, git_cache, mode) {
                        out.push(meta);
                    }
                },
            ));
            timer.mark("claude 解析");
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
            timer.mark("codex 解析");
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
    timer.mark("进程表");
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

    timer.mark("git 分支覆盖");

    // ── Phase 3: 按 mtime 排序 ───────────────────────────────────────────
    raw.sort_by(|a, b| b.mtime.cmp(&a.mtime));

    Ok(raw)
}

/// 增量解析：只解析 watcher 报上来的这几个 jsonl，其余文件一个都不碰。
///
/// 为什么不能直接调 list_sessions：它要 stat + 全量解析 ~220 个文件（合计 771MB），
/// 而 projects/ 是**递归**监听、每追加一条消息就触发一次 —— 这正是前端原来把
/// "sessions-changed" 接成空函数的原因。代价换算下来这里约是全量的 1/220。
///
/// 后处理刻意少做一件事：child_processes 要起 `ps -eo` 拿全表，这里省掉，
/// 由前端合并时沿用旧值（新 session 的子进程列表等下一次全量 load 补齐）。
/// 其余（archived 标记、实时 git 分支覆盖）和 list_sessions 的 Phase 2 一致。
#[tauri::command]
fn list_sessions_by_paths(
    paths: Vec<String>,
    cwd_mode: Option<String>,
) -> Result<Vec<SessionMeta>, String> {
    let now = chrono::Local::now().timestamp();
    // pty_id 在这里面拿（每个 running session 一次 `ps -p`）。前端要靠它把
    // 新 session 绑到已打开的 tab 上，所以这一份开销不能省。
    let running_info = load_running_info();
    let archived_set = load_archived();
    let mode = cwd_mode.as_deref().unwrap_or("smart");
    let codex_dir = ai_provider::AiTool::Codex.sessions_dir();

    let mut git_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut branch_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut out: Vec<SessionMeta> = Vec::new();

    for p in &paths {
        let path = Path::new(p);
        // 文件已被删除时 metadata 失败，紧跟着的 parse 也会失败 → 自然跳过。
        // 也就是说"session 文件被删"这件事这里同步不了，要等一次全量 load（⌘R）。
        let mtime = path
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);

        let is_codex = codex_dir.as_ref().map(|d| path.starts_with(d)).unwrap_or(false);
        let parsed = if is_codex {
            ai_provider::parse_codex_session(path, mtime, now, &running_info, &mut git_cache)
        } else {
            parse_session(path, mtime, now, &running_info, &mut git_cache, mode)
        };

        if let Some(mut meta) = parsed {
            if archived_set.contains(&meta.session_id) {
                meta.archived = true;
            }
            let cwd_key = if !meta.last_cwd.is_empty() { meta.last_cwd.clone() } else { meta.cwd.clone() };
            if !cwd_key.is_empty() {
                if let Some(b) = branch_cache
                    .entry(cwd_key.clone())
                    .or_insert_with(|| current_branch_for(&cwd_key))
                    .clone()
                {
                    meta.git_branch = b;
                }
            }
            out.push(meta);
        }
    }

    Ok(out)
}

#[cfg(test)]
mod git_head_tests {
    use std::process::Command;

    fn via_subprocess(cwd: &str) -> Option<String> {
        let out = Command::new("git")
            .args(["-C", cwd, "branch", "--show-current"])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if s.is_empty() { None } else { Some(s) }
    }

    /// 回归：读 .git/HEAD 的快路径必须和 `git branch --show-current` 逐字节一致。
    /// 拿本机真实存在的 session cwd 当语料，覆盖普通仓库 / linked worktree /
    /// 非仓库目录 / 已删除路径几种形态。
    #[test]
    fn head_read_matches_git_cli() {
        let mut dirs: Vec<String> = vec![
            env!("CARGO_MANIFEST_DIR").to_string(),
            "/tmp".to_string(),
            "/definitely/not/a/path".to_string(),
        ];
        // 空 cwd 不进对比语料：`git -C "" ...` 会退化成"在当前进程目录跑"，
        // 于是旧实现会把 makit 自己所在仓库的分支贴到一个 cwd 未知的 session 上。
        // 调用方本来就挡掉了空值，这里直接钉住正确行为。
        assert_eq!(super::current_branch_for(""), None);
        // 真实语料：所有 session 的 cwd
        if let Ok(sessions) = super::list_sessions(Some("smart".into())) {
            for s in sessions.iter().take(80) {
                if !s.cwd.is_empty() { dirs.push(s.cwd.clone()); }
                if !s.last_cwd.is_empty() { dirs.push(s.last_cwd.clone()); }
            }
        }
        dirs.sort();
        dirs.dedup();

        let mut checked = 0;
        for d in &dirs {
            if !d.is_empty() && !std::path::Path::new(d).is_dir() { continue; }
            let fast = super::current_branch_for(d);
            let slow = via_subprocess(d);
            assert_eq!(fast, slow, "分支解析不一致 @ {d}");
            checked += 1;
        }
        eprintln!("对比通过：{checked} 个目录", );
        assert!(checked > 3, "语料太少，测试没有意义（只比了 {checked} 个）");
    }
}

#[cfg(test)]
#[cfg(unix)]
mod env_of_pid_tests {
    /// 回归：`get_env_var_of_pid` 必须真的读到**指定那个 pid** 的环境变量。
    ///
    /// 原来的实现是 `ps -p PID -xeww -o command=`，两个 flag 都错了：
    /// macOS 的 `ps` 里 `-e` 是 `-A` 的同义词（"显示所有进程"），显示环境变量的是
    /// **大写 `-E`**；而 `-x` 又会把没有控制终端的进程（全部 daemon）并进来 ——
    /// BSD ps 的选择条件是 OR 而不是 AND，所以 `-p` 被彻底淹没。实测这条命令返回
    /// 756 行整张进程表、且一个环境变量都没有，函数于是恒定返回 None。
    ///
    /// 后果不是"偶尔取不到"而是"永远取不到"：`pty_id` 恒为空串 → App.tsx 里
    /// `sessions.filter(s => s.running && s.pty_id)` 全被空串的 falsy 过滤掉 →
    /// 在新建 shell 里手敲 claude 的那个 tab 永远绑不上 session，标题一直停在
    /// "新会话"（⌘R 也救不回来，全量 load 走的是同一条命令）。
    ///
    /// 用自己的 pid 当被测对象：测试二进制是本项目的编译产物，不是 Apple 平台
    /// 二进制，环境变量读得到（SIP 只挡 /bin/* 那类签名平台二进制）。
    /// 拿 PATH 做**全等**比较而不是判非空 —— 只判非空的话，"从整张进程表里捞到
    /// 别人的 PATH" 这种串台也能过。
    /// 取样变量刻意挑"值里没有空白"的那一个：ps 输出里环境变量之间就是用空格
    /// 分隔的，值本身含空格（比如 PATH 里有 `/Library/Application Support/...`）
    /// 根本无法无歧义还原 —— 这是 ps 输出格式的固有限制，不是可修的 bug。
    /// 唯一的调用方只读 CCS_PTY_ID，值形如 `t_ms5nflsm3a0g`，不受影响。
    #[test]
    fn reads_env_of_the_requested_pid() {
        let me = std::process::id();
        let (key, expected) = std::env::vars()
            .find(|(k, v)| {
                !v.is_empty() && !v.contains(char::is_whitespace) && !k.contains('=')
            })
            .expect("测试环境里至少得有一个值不含空白的环境变量");
        assert_eq!(
            super::get_env_var_of_pid(me, &key),
            Some(expected),
            "没读到本进程的 {key}（要么 ps 没显示环境变量，要么串到了别的进程）"
        );
        // 没设过的变量必须是 None，而不是从别处捞一个回来
        assert_eq!(
            super::get_env_var_of_pid(me, "MAKIT_DEFINITELY_UNSET_VAR_9f3a"),
            None
        );
    }
}

/// #173「会话启动目录被删了怎么恢复」。
///
/// 本质：会话数据一个字节都没丢，丢的只是**一把钥匙**。`claude -r <id>` 找会话的唯一
/// 方式是去 `~/.claude/projects/<encode(realpath(cwd))>/<id>.jsonl` 里找 —— 目录被删、
/// 你在别处启动，算出来的键就不一样，claude 报「No conversation found」，而 transcript
/// 还老老实实躺在原存储目录里。所以「恢复会话」= **让钥匙对上**，不是找回数据；任何
/// 拷贝 jsonl 的做法都会产出同一个 session id 的两份分叉，方向就是错的。
///
/// 实测（隔离沙箱，`CLAUDE_CONFIG_DIR` 指向临时目录，没碰真实 `~/.claude`）：
///   - 真 id、键不匹配 → `No conversation found with session ID`
///   - 同一个 id，**全新的空目录** + transcript 放在该目录的编码键下 → 找到了
///     （报错变成「Provide a prompt to continue」）
/// 也就是说**原目录完全不需要存在**，任何一个存在的目录都行。
#[cfg(test)]
mod session_recovery_tests {
    /// 回归：编码存储键前**必须先 canonicalize**。
    ///
    /// claude 是拿自己进程的 cwd 算这个键的，而进程 cwd 永远是解析过软链的实路径 ——
    /// macOS 上 `/var` 就是 `/private/var` 的软链，`/tmp` 是 `/private/tmp`。不 canonicalize
    /// 就会给含软链的路径算出一个 claude 永远不会用的键，symlink 建在错的地方，
    /// 「恢复」静默失效。
    ///
    /// 这个坑是实测踩出来的：第一次做恢复实验时手工造的键用的是 `/var/folders/…`，
    /// claude 算的是 `/private/var/folders/…`，于是明明 transcript 就在那儿也报
    /// 「No conversation found」，白跑一轮才发现不是机制不对、是键算错了。
    #[test]
    fn encode_canonicalizes_before_encoding() {
        let raw = std::env::temp_dir().join(format!("makit-enc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&raw);
        std::fs::create_dir_all(&raw).unwrap();
        let raw_str = raw.to_string_lossy().into_owned();
        let canonical = std::fs::canonicalize(&raw).unwrap().to_string_lossy().into_owned();

        // 前提：本机的临时目录确实经过软链，否则这条测试什么都没测到
        assert_ne!(
            raw_str, canonical,
            "本机 temp_dir 不含软链（{raw_str}），这条测试在这台机器上无效，需换一个含软链的路径"
        );

        assert_eq!(
            super::encode_project_path(&raw_str),
            super::encode_project_path(&canonical),
            "软链路径和实路径必须编码成同一个键（claude 用的是实路径那个）"
        );

        let _ = std::fs::remove_dir_all(&raw);
    }

    /// 路径不存在时 canonicalize 会失败，此时必须退回逐字符编码原样 —— 而这恰恰是
    /// 「原目录已被删」的场合。这种场合下退回原样是**正确**的：jsonl 里记的 cwd 本身
    /// 就是 claude 当年写下的实路径，不需要也无法再解析。
    #[test]
    fn encode_falls_back_to_raw_when_path_is_gone() {
        assert_eq!(
            super::encode_project_path("/definitely/not/a/path/ai-claw.studio"),
            "-definitely-not-a-path-ai-claw-studio"
        );
    }

    const SID: &str = "4110cea1-8771-4e89-b521-b93f5a677c5a";

    /// 造一个假的 `projects/` 根 + 一个装着 transcript 的存储目录。
    /// 绝不碰真实 `~/.claude`：所有恢复逻辑都收口成 `*_in(projects_root, …)`，
    /// 就是为了让测试能指到别处。
    fn sandbox(tag: &str) -> (std::path::PathBuf, String) {
        let root = std::env::temp_dir().join(format!("makit-recover-{}-{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let projects = root.join("projects");
        // 原 cwd 刻意放在 root 下面并且**不创建**：模拟「启动目录已被删」
        let original_cwd = root.join("gone-project").to_string_lossy().into_owned();
        let storage = super::encode_project_path(&original_cwd);
        std::fs::create_dir_all(projects.join(&storage)).unwrap();
        std::fs::write(
            projects.join(&storage).join(format!("{SID}.jsonl")),
            "{\"cwd\":\"x\",\"type\":\"user\"}\n",
        )
        .unwrap();
        (root, original_cwd)
    }

    /// 动作 A「重建原目录」：键本来就是按原路径算的，把空目录建回来钥匙立刻对上。
    /// 关键断言是**一个 symlink 都不许建** —— 这是三条恢复路径里唯一完全不碰
    /// claude 数据目录的一条，它的价值就在这儿。
    #[test]
    fn recreate_matches_the_key_without_touching_claude_dir() {
        let (root, original_cwd) = sandbox("recreate");
        let projects = root.join("projects");
        let before: Vec<_> = std::fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name()).collect();

        let r = super::recover_session_cwd_in(&projects, "recreate", SID, &original_cwd, "", "").unwrap();

        assert!(std::path::Path::new(&r.cwd).is_dir(), "原目录必须被建回来");
        assert_eq!(r.cwd, original_cwd, "重建之后就该在原路径上 resume");
        let after: Vec<_> = std::fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(before, after, "重建原目录这条路不该在 projects/ 下留下任何东西");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 动作 B「指到新位置」：代码搬家了，要在新目录里继续。
    /// 用**文件级** symlink 而不是目录级：目录级会把老项目下所有会话一并暴露到新键下，
    /// 而且 `list_sessions` 的枚举会踩上去。
    #[test]
    fn relink_makes_a_brand_new_dir_resumable() {
        let (root, original_cwd) = sandbox("relink");
        let projects = root.join("projects");
        let storage = super::encode_project_path(&original_cwd);
        let new_dir = root.join("moved-here");
        std::fs::create_dir_all(&new_dir).unwrap();
        let new_dir_s = new_dir.to_string_lossy().into_owned();

        let r = super::recover_session_cwd_in(&projects, "relink", SID, &original_cwd, &new_dir_s, &storage)
            .unwrap();

        assert_eq!(r.cwd, new_dir_s);
        // 实测过的不变量：transcript 只要在「新 cwd 的编码键」下可达，claude 就能找到
        let key = super::encode_project_path(&new_dir_s);
        let landed = projects.join(&key).join(format!("{SID}.jsonl"));
        assert!(landed.exists(), "新键下必须能看到 transcript");
        assert_eq!(
            std::fs::read_to_string(&landed).unwrap(),
            "{\"cwd\":\"x\",\"type\":\"user\"}\n",
            "读到的必须是同一份原文件，不是副本"
        );
        assert!(
            std::fs::symlink_metadata(&landed).unwrap().file_type().is_symlink(),
            "必须是 symlink —— 拷贝会产出同一个 session id 的两份分叉"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 两条拒绝路径：宁可报错，也不留一个悬空 symlink 或者假装成功。
    #[test]
    fn relink_refuses_bad_input() {
        let (root, original_cwd) = sandbox("refuse");
        let projects = root.join("projects");
        let storage = super::encode_project_path(&original_cwd);

        let gone = root.join("not-created").to_string_lossy().into_owned();
        assert!(
            super::recover_session_cwd_in(&projects, "relink", SID, &original_cwd, &gone, &storage).is_err(),
            "目标目录不存在时必须拒绝"
        );
        assert!(
            super::recover_session_cwd_in(&projects, "relink", "no-such-session", &original_cwd, &root.to_string_lossy(), &storage).is_err(),
            "transcript 不存在时必须拒绝，不能建悬空 symlink"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// codex 的会话**不按 cwd 索引**（`~/.codex/sessions/年/月/日/`，`storage_folder` 恒为空串），
    /// 所以它压根没有「钥匙丢了」这个问题 —— `codex resume <id>` 在任何目录都能找到会话。
    /// 这条测的是：这种会话走「指到新位置」时只换工作目录，**不许**在 projects/ 下动任何东西。
    ///
    /// 不加这个分支的话，空 `storage_folder` 会让 `projects_root.join("")` 落回 projects 根身上，
    /// 源文件永远找不到 → 永远报「找不到会话记录」→ 用户为一个不存在的问题卡在对话框里。
    #[test]
    fn relink_is_a_noop_for_tools_that_dont_key_by_cwd() {
        let (root, original_cwd) = sandbox("codex");
        let projects = root.join("projects");
        let new_dir = root.join("anywhere");
        std::fs::create_dir_all(&new_dir).unwrap();
        let new_dir_s = new_dir.to_string_lossy().into_owned();
        let before: Vec<_> = std::fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name()).collect();

        let r = super::recover_session_cwd_in(&projects, "relink", SID, &original_cwd, &new_dir_s, "")
            .expect("storage_folder 为空不是错误，是「这个工具不需要修键」");

        assert_eq!(r.cwd, new_dir_s, "就用用户给的新目录");
        let after: Vec<_> = std::fs::read_dir(&projects).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(before, after, "不按 cwd 索引的工具，一个 symlink 都不该建");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 「重建原目录」只补最后一级，父目录不存在必须拒绝。
    ///
    /// `create_dir_all` 会把缺失的父目录全造出来，而整棵父树都没了通常意味着外置盘没挂载。
    /// 在挂载点上造一个真目录之后，那块盘会被 macOS 静默挂成「X 1」——用户的路径全指错，
    /// 而且没有任何提示。宁可报错让人去挂盘。
    #[test]
    fn recreate_refuses_when_the_whole_parent_tree_is_gone() {
        let root = std::env::temp_dir().join(format!("makit-recover-parent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let projects = root.join("projects");
        std::fs::create_dir_all(&projects).unwrap();

        // 父目录 missing-volume 也不存在 → 这不是「删了一个项目目录」
        let deep = root.join("missing-volume").join("proj");
        let deep_s = deep.to_string_lossy().into_owned();
        let err = super::recover_session_cwd_in(&projects, "recreate", SID, &deep_s, "", "")
            .expect_err("父目录不存在时必须拒绝");
        assert!(err.contains("父目录也不存在"), "报错要说清为什么拒绝，实际: {err}");
        assert!(!deep.exists(), "拒绝了就不许留下任何半成品目录");
        assert!(
            !root.join("missing-volume").exists(),
            "尤其不许在挂载点位置造出真目录"
        );

        // 对照：只缺最后一级 → 正常重建
        let shallow = root.join("normal-proj").to_string_lossy().into_owned();
        super::recover_session_cwd_in(&projects, "recreate", SID, &shallow, "", "")
            .expect("只缺最后一级是最常见的场合，必须能重建");
        assert!(std::path::Path::new(&shallow).is_dir());

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 回归：`list_sessions` 枚举 jsonl 时必须跳过 symlink。
    ///
    /// 加了「指到新位置」之后，同一份 transcript 会在两个键下可见（原存储目录 + 新键）。
    /// 枚举时只看扩展名的话，同一个 session_id 会被扫成**两条**，侧栏里出现一对孪生会话。
    /// 目录级 symlink 本来就被跳过（`file_type()` 不跟随软链，见枚举那段注释），文件级
    /// 这条以前没管 —— 因为以前没人成规模地建过文件级 symlink。
    #[test]
    fn enumeration_skips_symlinked_jsonl() {
        let dir = std::env::temp_dir().join(format!("makit-enum-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("real.jsonl");
        std::fs::write(&real, "{}\n").unwrap();
        std::os::unix::fs::symlink(&real, dir.join("linked.jsonl")).unwrap();

        let picked: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| super::is_scannable_jsonl(e))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();

        assert_eq!(picked, vec!["real.jsonl".to_string()], "symlink 的 jsonl 必须被跳过");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod pty_binding_tests {
    use std::fs;

    /// 回归 #169 的第二半：「新建 shell 里手敲 claude」的 tab 标题不同步。
    ///
    /// 原来的绑定挂在 `sessions` 数组上（`list_sessions` → 扫
    /// `~/.claude/projects/**/*.jsonl`），而那个 jsonl **要等用户发出第一条消息之后
    /// 才被创建**。于是新起的 claude 在发消息前根本不在 sessions 里，
    /// `s.pty_id === t.id` 无从匹配，tab.sessionId 一直是 null，标题停在「新会话」。
    /// ⌘R 走的还是 `list_sessions`，所以"刷新也没修复"；resume 出来的 tab 早就有
    /// jsonl，所以"恢复的是可以的"——用户给的三句话是同一个根因的三个侧面。
    ///
    /// 真正需要的事实只有 `(pty_id, session_id)`，claude 启动那一刻就写进了
    /// `~/.claude/sessions/<pid>.json`。这里测的就是"只读那个目录也够"。
    ///
    /// 实测证据（修之前）：pid 86431 的 CCS_PTY_ID=t_msngclks3eo5、session 32318d5f
    /// 活了近 2 小时，而 localStorage 里 id 为 t_msngclks3eo5 的 tab 仍是
    /// `kind:"new" / label:"新会话" / sessionId:null` —— 因为 32318d5f.jsonl 不存在。
    fn write(dir: &std::path::Path, file: &str, json: &str) {
        fs::write(dir.join(file), json).unwrap();
    }

    #[test]
    fn binds_from_sessions_dir_without_any_jsonl() {
        let dir = std::env::temp_dir().join(format!("makit-bind-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 想绑的那个：pid 4321 → t_wanted
        write(
            &dir,
            "4321.json",
            r#"{"pid":4321,"sessionId":"32318d5f-1111-2222-3333-444455556666","name":"ai-claw-studio-6d"}"#,
        );
        // 别人家的 pty，不能串台
        write(
            &dir,
            "4322.json",
            r#"{"pid":4322,"sessionId":"deadbeef-0000-0000-0000-000000000000","name":"别人"}"#,
        );
        // pid=0（claude 没写 pid）：必须跳过，且不该去查环境变量
        write(
            &dir,
            "4323.json",
            r#"{"pid":0,"sessionId":"aaaaaaaa-0000-0000-0000-000000000000"}"#,
        );
        // 不是 json 后缀 / 坏 json：都不能让整个扫描挂掉
        write(&dir, "notes.txt", "t_wanted");
        write(&dir, "broken.json", "{ this is not json");

        let asked = std::cell::RefCell::new(Vec::<u32>::new());
        let pty_of = |pid: u32| -> Option<String> {
            asked.borrow_mut().push(pid);
            match pid {
                4321 => Some("t_wanted".into()),
                4322 => Some("t_someone_else".into()),
                _ => None,
            }
        };

        let out = super::resolve_bindings_in(&dir, &["t_wanted".to_string()], &pty_of);

        assert_eq!(out.len(), 1, "只该绑上 t_wanted 那一个");
        assert_eq!(out[0].pty_id, "t_wanted");
        assert_eq!(out[0].session_id, "32318d5f-1111-2222-3333-444455556666");
        assert_eq!(out[0].short_id, "32318d5f", "short_id 取第一段 uuid");
        assert_eq!(
            out[0].name, "ai-claw-studio-6d",
            "要把 claude 写的会话名带回去，否则 tab 只能显示 [shortId]"
        );
        assert!(
            !asked.borrow().contains(&0),
            "pid=0 不该触发环境变量查询（每次查询是一个 ps 进程）"
        );

        // pty_ids 为空：一次目录读都不该发生 —— 前端没有待绑定 tab 时的零成本路径
        let never = |_pid: u32| -> Option<String> {
            panic!("pty_ids 为空时不该查任何 pid");
        };
        assert!(super::resolve_bindings_in(&dir, &[], &never).is_empty());

        // 匹配不上就是空，而不是"随便挑一个回去"
        let out = super::resolve_bindings_in(&dir, &["t_nobody".to_string()], &pty_of);
        assert!(out.is_empty(), "没有命中的 pty_id 时必须返回空");

        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod perf_tests {
    /// 跑法：MAKIT_TIMING=1 cargo test --release -- --ignored --nocapture bench_list_sessions
    /// release 而不是 dev：dev 下 serde_json 没内联，测出来的是编译器而不是算法。
    #[test]
    #[ignore = "手动性能基线，不进常规 test"]
    fn bench_list_sessions() {
        for i in 0..3 {
            let t = std::time::Instant::now();
            let r = super::list_sessions(Some("smart".into())).unwrap();
            eprintln!("── run{}: {} sessions, 总计 {:.1}ms\n", i, r.len(), t.elapsed().as_secs_f64() * 1000.0);
        }
    }
}

#[cfg(test)]
mod session_name_tests {
    use std::collections::HashMap;
    use std::fs;
    use std::path::{Path, PathBuf};

    fn write_jsonl(name: &str, lines: &[&str]) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!("makit-name-test-{}-{}", std::process::id(), name));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("11111111-2222-3333-4444-555555555555.jsonl");
        fs::write(&f, lines.join("\n") + "\n").unwrap();
        f
    }

    fn parse(path: &Path) -> super::SessionMeta {
        let mut cache = HashMap::new();
        super::parse_session(path, 1_700_000_000, 1_700_000_100, &HashMap::new(), &mut cache, "smart")
            .expect("parse_session 返回了 None")
    }

    const USER_MSG: &str = r#"{"type":"user","cwd":"/tmp","isSidechain":false,"message":{"role":"user","content":"build当前项目"}}"#;
    // slug 的真实出处：实测 8 个带 slug 的会话，8 个都是这条记录写进去的
    const COMPACT_WITH_SLUG: &str =
        r#"{"type":"system","subtype":"compact_boundary","slug":"clever-swimming-flute","content":"Conversation compacted"}"#;
    const CUSTOM_TITLE: &str = r#"{"type":"custom-title","customTitle":"oc-install-pack脚本编写"}"#;

    /// 回归：`/compact` 不能改会话的显示名。
    ///
    /// claude 的 `slug`（`clever-swimming-flute` 这种随机三词代号）**只**写在
    /// `type:"system", subtype:"compact_boundary"` 记录上，和会话内容毫无关系。
    /// 原来它被塞进 `display_name`，而前端把 display_name 当权威
    /// （`display_name || first_user_msg`），于是同一个会话**被 compact 一次，标题就从
    /// 「build当前项目」退化成「clever-swimming-flute」** —— 越常用 /compact 的人越受害。
    #[test]
    fn compact_must_not_rename_session() {
        let before = parse(&write_jsonl("before", &[USER_MSG]));
        let after = parse(&write_jsonl("after", &[USER_MSG, COMPACT_WITH_SLUG]));
        assert_eq!(
            before.display_name, after.display_name,
            "compact 前后 display_name 变了：{:?} → {:?}",
            before.display_name, after.display_name
        );
        assert_eq!(before.name_source, after.name_source, "compact 前后 name_source 变了");
        // 前端的 `display_name || first_user_msg` 得能落到首条消息上
        assert_eq!(after.first_user_msg, "build当前项目");
    }

    /// 反向护栏：claude 写的真标题（customTitle / agentName）必须照旧压过一切，
    /// 包括压过 compact 的 slug。上面那个测试不能靠「把 display_name 一律清空」来过。
    #[test]
    fn real_title_still_wins() {
        let m = parse(&write_jsonl("title", &[USER_MSG, COMPACT_WITH_SLUG, CUSTOM_TITLE]));
        assert_eq!(m.display_name, "oc-install-pack脚本编写");
        assert_eq!(m.name_source, "custom-title");
    }
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
// - projects/ 变化 → emit "sessions-changed"，**载荷是变化的 jsonl 路径列表**。
//   这里以前发的是空载荷、注释写"低频，只在新 session 创建时触发"，但 projects/ 是
//   递归监听：每追加一条消息都会触发。前端因此只能把它接成空函数，新建会话要 ⌘R
//   才出现（#168）。带上路径之后前端可以只解析这几个文件（list_sessions_by_paths）。
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
                let mut changed_jsonl: Vec<String> = Vec::new();
                for ev in &events {
                    if ev.path.starts_with(&sessions_dir_clone) {
                        has_session_event = true;
                    } else if ev.path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                        // 只报 jsonl：目录创建、锁文件之类的事件对前端没有信息量。
                        // 新 session 一定会连带写出自己的 jsonl，不会漏。
                        changed_jsonl.push(ev.path.to_string_lossy().into_owned());
                    }
                }
                // sessions/ 变化 = running 状态更新（轻量：前端只刷 running 状态）
                if has_session_event {
                    let _ = app_handle.emit("running-changed", ());
                }
                // projects/ 变化 = 某个 session 的内容变了（新建 / 新消息 / 改名）
                if !changed_jsonl.is_empty() {
                    changed_jsonl.sort();
                    changed_jsonl.dedup();
                    let _ = app_handle.emit("sessions-changed", changed_jsonl);
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
                    // name 必须带上：改名写的就是这个文件，而写它只会触发 `running-changed`。
                    // 少了这个字段，那条路就**结构上**搬不了标题，改名要 ⌘R 才生效（#4 / #8）。
                    let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    if !session_id.is_empty() {
                        result.push(RunningMeta { session_id, status, waiting_for, pid, name });
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
    /// claude 自己写的会话名（`~/.claude/sessions/<pid>.json` 的 `name`）。可能为空。
    /// 和 `RunningInfo.name` 同源；`parse_session` 里它是 display_name 的**最高优先级**。
    name: String,
}

#[derive(serde::Serialize)]
pub struct PtyBinding {
    pty_id: String,
    session_id: String,
    short_id: String,
    /// claude 自己写的会话名（`~/.claude/sessions/<pid>.json` 的 `name`）。可能为空。
    name: String,
}

/// 把「还没绑上 session 的 tab」和「正在跑的 claude」对上：pty_id → session_id。
///
/// 为什么不复用 `list_sessions`：绑定需要的事实只有 `(pty_id, session_id)`，它在 claude
/// 启动的那一瞬间就写进了 `~/.claude/sessions/<pid>.json`。而 `list_sessions` 扫的是
/// `~/.claude/projects/**/*.jsonl` —— **那个文件要等用户发出第一条消息之后才存在**。
/// 于是「新建 shell、手打 claude」的 tab 在发消息之前根本不在 sessions 数组里，
/// `s.pty_id === t.id` 无从匹配，标题一直停在「新会话」；⌘R 走的还是 `list_sessions`，
/// 所以刷新同样无效；而 resume 出来的 tab 早就有 jsonl，看着"是好的"。
/// 三种表现是同一个根因：绑定挂在了一个比它自己晚出现的数据源上。
///
/// 只对前端传进来的 pty_ids 干活：没有待绑定 tab 时前端不会调这个命令，一次 ps 都不跑。
/// 匹配满了就早退，正常情况（一个新 tab）只查到第一个命中就结束。
#[tauri::command]
fn resolve_pty_bindings(pty_ids: Vec<String>) -> Vec<PtyBinding> {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return vec![],
    };
    resolve_bindings_in(
        &home.join(".claude").join("sessions"),
        &pty_ids,
        // 「这个 pid 现在归哪个 pty」合成一问：进程已经没了就当没有 marker。
        // sessions/*.json 不保证在 claude 退出时被清掉，先 kill(0) 挡掉死 pid，
        // 省一次 ps。
        &|pid| {
            if !pid_alive(pid) {
                return None;
            }
            get_env_var_of_pid(pid, "CCS_PTY_ID")
        },
    )
}

/// `resolve_pty_bindings` 的纯逻辑部分：目录 + 想要的 pty_ids + 「pid → pty_id」查询。
/// 抽出来是为了能测 —— 真实实现要一个活着的、带 CCS_PTY_ID 的非 Apple 签名进程，
/// 在单测里造不出来。
fn resolve_bindings_in(
    dir: &Path,
    pty_ids: &[String],
    pty_of_pid: &dyn Fn(u32) -> Option<String>,
) -> Vec<PtyBinding> {
    if pty_ids.is_empty() {
        return vec![];
    }
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };
    let mut result: Vec<PtyBinding> = vec![];
    for entry in entries.flatten() {
        // 每个 tab 只能绑一个 session，配额满了就不必再扫（常见情况是只有一个新 tab）
        if result.len() == pty_ids.len() {
            break;
        }
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let v: serde_json::Value = match fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => continue,
        };
        let session_id = match v.get("sessionId").and_then(|x| x.as_str()) {
            Some(s) if !s.is_empty() => s.to_string(),
            _ => continue,
        };
        let pid = v.get("pid").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
        if pid == 0 {
            continue;
        }
        let pty_id = match pty_of_pid(pid) {
            Some(p) if pty_ids.iter().any(|want| *want == p) => p,
            _ => continue,
        };
        result.push(PtyBinding {
            pty_id,
            short_id: session_id.split('-').next().unwrap_or("").to_string(),
            session_id,
            name: v
                .get("name")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
        });
    }
    result
}

#[cfg(unix)]
fn pid_alive(pid: u32) -> bool {
    // signal 0：只做存在性/权限检查，不真的发信号
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

#[cfg(not(unix))]
fn pid_alive(_pid: u32) -> bool {
    true
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
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
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
            resolve_pty_bindings,
            list_sessions_by_paths,
            ensure_session_symlink,
            dir_exists,
            recover_session_cwd,
            list_worktrees,
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
