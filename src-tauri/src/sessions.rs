//! 会话扫描：枚举 `~/.claude/projects/**/*.jsonl`（和 Codex），解析成 `SessionMeta`，
//! 以及单个会话的详情 / meta / 按 cwd 反查。增量游标见 `scan_cache`。

use chrono::{DateTime, Local, TimeZone};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::ai_provider;
use crate::archive::load_archived;
use crate::paths::projects_dir;
use crate::perf;
use crate::process::{collect_orphan_by_env, collect_process_table, descendants_of, ProcessInfo};
use crate::running::{load_running_info, RunningInfo};
use crate::scan_cache::{save_scan_cache_throttled, scan_session_file, ScanState};

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

/// 枚举 `projects/<key>/` 里的 transcript 时该不该扫这个条目。
///
/// 除了扩展名，还必须**跳过 symlink**。「指到新位置」会让同一份 transcript 在两个键下
/// 可见（原存储目录 + 新 cwd 的键），只看扩展名的话同一个 session_id 会被扫成两条，
/// 侧栏里冒出一对孪生会话。目录级 symlink 本来就被跳过（见 `list_sessions` 里那段注释），
/// 这里把文件级补上，两者规则一致：**只认真身，软链一律不算**。
pub fn is_scannable_jsonl(entry: &fs::DirEntry) -> bool {
    if entry.path().extension().and_then(|e| e.to_str()) != Some("jsonl") {
        return false;
    }
    entry.file_type().map(|t| t.is_file()).unwrap_or(false)
}

pub fn is_real_user_msg(text: &str) -> bool {
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

pub fn extract_text(value: &serde_json::Value) -> String {
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

pub fn is_worktree_path(cwd: &str) -> bool {
    cwd.contains("/.worktrees/")
        || cwd.ends_with("/.worktrees")
        || cwd.contains("/.claude/worktrees/")
        || cwd.ends_with("/.claude/worktrees")
}

pub fn get_git_root(cwd: &str, cache: &mut HashMap<String, Option<String>>) -> Option<String> {
    cache.entry(cwd.to_string()).or_insert_with(|| detect_git_root(cwd)).clone()
}

pub fn detect_git_root(cwd: &str) -> Option<String> {
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

pub fn resolve_git_root_cached(start_cwd: &str, last_cwd: &str, cwd_mode: &str, cache: &mut HashMap<String, Option<String>>) -> String {
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

pub fn parse_session(
    path: &Path,
    mtime: i64,
    now: i64,
    running_info: &HashMap<String, RunningInfo>,
    git_cache: &mut HashMap<String, Option<String>>,
    cwd_mode: &str,
) -> Option<SessionMeta> {
    let ScanState {
        start_cwd,
        last_cwd,
        git_branch,
        user_count,
        first_msg,
        last_msg,
        custom_title,
        agent_name,
    } = scan_session_file(path)?;

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

/// 找到 cwd 所属仓库的 git 目录。`.git` 是目录就是它自己；是文件则跟着里面的
/// `gitdir:` 跳走（linked worktree 指向 `<主仓>/.git/worktrees/<名>`，submodule
/// 指向 `<父仓>/.git/modules/<名>`），两种情况下 HEAD 都在跳到的那个目录里。
///
/// 和 `detect_git_root` 不是一回事：那个要的是"归属哪个项目"，worktree 会被折回主仓根；
/// 这里要的是"HEAD 文件在哪"，必须停在 worktree 自己的 git 目录上。
pub fn git_dir_for(cwd: &str) -> Option<PathBuf> {
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
pub fn current_branch_for(cwd: &str) -> Option<String> {
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

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ConversationMessage {
    pub role: String,
    pub text: String,
    pub timestamp: String,
    pub cwd: String,
    pub git_branch: String,
    pub tool_uses: Vec<String>,
}

pub fn extract_tool_uses(content: &serde_json::Value) -> Vec<String> {
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

#[tauri::command(async)]
pub fn read_session_messages(session_id: String) -> Result<Vec<ConversationMessage>, String> {
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
    let path = match found_path {
        Some(p) => p,
        None => {
            // 不是 Claude 的会话，再去 Codex 那边找（#209）。以前只找 Claude，Codex 会话一点开详情就报错
            if let Some(codex_dir) = ai_provider::AiTool::Codex.sessions_dir() {
                if let Some(p) = ai_provider::find_codex_session_file(&codex_dir, &session_id) {
                    let file = fs::File::open(&p).map_err(|e| e.to_string())?;
                    return Ok(ai_provider::parse_codex_messages(BufReader::new(file)));
                }
            }
            return Err(format!("找不到 session: {}", session_id));
        }
    };
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
pub fn parallel_scan<T, S, R>(
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
///
/// 各阶段耗时同时收进 `stages`，由 `record` 写进 perf.log（#218）—— 打包版也能看。
pub struct PhaseTimer {
    on: bool,
    start: std::time::Instant,
    last: std::time::Instant,
    stages: Vec<(String, f64)>,
}
impl PhaseTimer {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Self { on: std::env::var_os("MAKIT_TIMING").is_some(), start: now, last: now, stages: Vec::new() }
    }
    fn mark(&mut self, label: &str) {
        let now = std::time::Instant::now();
        let ms = now.duration_since(self.last).as_secs_f64() * 1000.0;
        self.stages.push((label.to_string(), (ms * 10.0).round() / 10.0));
        if self.on {
            eprintln!("[timing] {:<22} {:>8.1}ms  (累计 {:.1}ms)",
                label, ms, now.duration_since(self.start).as_secs_f64() * 1000.0);
        }
        self.last = now;
    }
    fn total_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
    /// 写一条 perf.log：总耗时 + 各阶段
    fn record(&self, kind: &str, extra: serde_json::Value) {
        let mut ev = serde_json::json!({
            "kind": kind,
            "ms": self.total_ms().round(),
            "stages": self.stages.iter().map(|(k, v)| serde_json::json!([k, v])).collect::<Vec<_>>(),
        });
        if let (Some(obj), serde_json::Value::Object(more)) = (ev.as_object_mut(), extra) {
            obj.extend(more);
        }
        perf::record(ev);
    }
}

#[tauri::command(async)]
pub fn list_sessions(cwd_mode: Option<String>) -> Result<Vec<SessionMeta>, String> {
    let mut timer = PhaseTimer::new();
    // macOS 的 ps 本身要 0.3s（启动时抢 CPU 能到 0.55s），和下面的解析互不依赖 —— 并行跑（#216）
    let proc_table_job = std::thread::spawn(collect_process_table);
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
    let proc_table = proc_table_job.join().unwrap_or_default();
    timer.mark("进程表（等并行的 ps）");
    for meta in &mut raw {
        if meta.running && meta.pid > 0 {
            meta.child_processes = descendants_of(meta.pid, &proc_table);
        }
    }

    // 孤儿进程关联（通过 MAKIT_SESSION_ID 环境变量，PPID=1 的 detach 进程）
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

    save_scan_cache_throttled(true);
    timer.mark("缓存写盘");
    timer.record("list_sessions", serde_json::json!({ "sessions": raw.len() }));
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
#[tauri::command(async)]
pub fn list_sessions_by_paths(
    paths: Vec<String>,
    cwd_mode: Option<String>,
) -> Result<Vec<SessionMeta>, String> {
    let timer = PhaseTimer::new();
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

    // 会话输出时每秒多一次，只记慢的，免得日志刷屏（#218）
    if timer.total_ms() > 100.0 {
        timer.record("list_sessions_by_paths", serde_json::json!({ "files": paths.len() }));
    }
    save_scan_cache_throttled(false);
    Ok(out)
}

#[cfg(test)]
mod git_head_tests {
    use std::process::Command;

    // 对照用 `symbolic-ref --short -q HEAD` 而不是 `branch --show-current`：后者 git 2.22 才有，
    // PATH 上排在前面的若是老 git（本机 /usr/local/bin/git 是 2.15）会直接报错返回 None，
    // 测试就把「对照组坏了」误报成「快路径错了」。两者语义相同：分支名，或 HEAD 游离时为空。
    fn via_subprocess(cwd: &str) -> Option<String> {
        let out = Command::new("git")
            .args(["-C", cwd, "symbolic-ref", "--short", "-q", "HEAD"])
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

// 按需归因：扫 ~/.claude/projects/*/*.jsonl，找在 tab 启动之后「新建」(birthtime > after_ts) 且 cwd 匹配的
// 用 birthtime 而非 mtime —— mtime 会被任何写入刷新，旧 session 也会被误命中
// 用途：new/shell tab 启动后想反查到 claude 写出的 session 文件
#[tauri::command(async)]
pub fn find_session_in_cwd_after(cwd: String, after_ts: i64) -> Result<Option<String>, String> {
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
#[tauri::command(async)]
pub fn read_session_meta(session_id: String) -> Result<Option<SessionMeta>, String> {
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
