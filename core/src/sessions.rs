//! Session scanning: enumerates `~/.claude/projects/**/*.jsonl` (and Codex), parses into `SessionMeta`,
//! plus single-session details / meta / reverse lookup by cwd. For the incremental cursor see `scan_cache`.

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

/// Whether this entry should be scanned when enumerating transcripts in `projects/<key>/`.
///
/// Beyond the extension, **symlinks must be skipped**. "Point to new location" makes the same transcript
/// visible under two keys (the original storage directory + the new cwd's key); looking only at the extension would scan the same session_id as two entries,
/// and a pair of twin sessions would pop up in the sidebar. Directory-level symlinks were already skipped (see the comment in `list_sessions`),
/// and the file level is added here so the two rules agree: **only real files count, symlinks never do**.
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

    // smart: last takes priority; on conflict, trust start
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
    // Priority: rename (rename while running) > customTitle / agentName (the real title claude wrote) > empty.
    // Empty does not mean "no name"; it means "there is no better name here than the first user message" -- the frontend falls back with
    // `display_name || first_user_msg`. And first_user_msg is always non-empty: above,
    // `user_count == 0` returns None directly, and user_count and first_msg are assigned in the same branch.
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

/// Find the git directory of the repository a cwd belongs to. If `.git` is a directory, that is it; if it is a file, follow the
/// `gitdir:` inside it (a linked worktree points to `<main repo>/.git/worktrees/<name>`, a submodule
/// to `<parent repo>/.git/modules/<name>`); in both cases HEAD is in the directory jumped to.
///
/// Not the same thing as `detect_git_root`: that one wants "which project it belongs to", and a worktree is folded back to the main repo root;
/// this one wants "where the HEAD file is", and must stop at the worktree's own git directory.
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

/// Equivalent to `git branch --show-current`, but without forking a child process -- what that command does is
/// read `.git/HEAD` and strip the `refs/heads/` prefix; one file read is enough.
///
/// Measured: each fork+exec takes about 38ms; spread over a few dozen distinct directories for 189 sessions that is 2.3s,
/// 65% of the total time of list_sessions -- the first-screen latency of the whole session list was mainly this.
/// With a detached HEAD, HEAD holds a bare SHA with no `ref: ` prefix, and None is returned,
/// matching the behaviour of `--show-current` printing an empty string.
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

/// Session id -> the session file and which tool it belongs to (look in claude's projects directory first, then codex's sessions directory, #209)
pub fn locate_session_file(session_id: &str) -> Option<(PathBuf, crate::transcript::Tool)> {
    if let Some(dir) = projects_dir() {
        if let Ok(entries) = fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    let candidate = p.join(format!("{}.jsonl", session_id));
                    if candidate.exists() {
                        return Some((candidate, crate::transcript::Tool::Claude));
                    }
                }
            }
        }
    }
    let codex_dir = ai_provider::AiTool::Codex.sessions_dir()?;
    ai_provider::find_codex_session_file(&codex_dir, session_id).map(|p| (p, crate::transcript::Tool::Codex))
}

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
            // Not a Claude session; look on the Codex side (#209). It used to look only at Claude, so opening the details of a Codex session always errored
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

/// Distributes items among N threads that grab them. Each thread first builds its own S (thread-local cache and the like),
/// then calls f on each item it grabbed, pushes the results into its own Vec, and finally they are concatenated in thread order.
///
/// An atomic cursor is used to grab tasks rather than splitting evenly by index: at both call sites the items' costs differ by several orders of magnitude
/// (a jsonl is up to 95MB but mostly a few dozen KB; the number of files in a project directory ranges from 1 to over a hundred),
/// and an even split would let the thread that gets the heaviest share alone decide the total time.
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

/// Per-phase timing. Fully silent by default (one env read + one bool check),
/// run with `MAKIT_TIMING=1` when the numbers are needed. Without it you can only guess "where it feels slow".
///
/// The per-phase times are also collected into `stages` and written to perf.log by `record` (#218) -- visible in the packaged build too.
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
    /// Write one perf.log entry: total time + each phase
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

pub fn list_sessions(cwd_mode: Option<String>) -> Result<Vec<SessionMeta>, String> {
    let mut timer = PhaseTimer::new();
    // macOS's ps itself takes 0.3s (up to 0.55s when competing for CPU at startup), and does not depend on the parsing below -- run in parallel (#216)
    let proc_table_job = std::thread::spawn(collect_process_table);
    let now = chrono::Local::now().timestamp();
    let running_info = load_running_info();
    let archived_set = load_archived();
    let mode = cwd_mode.as_deref().unwrap_or("smart");

    // -- Phase 1: collect the raw sessions of all tools ------------------------
    // To add a tool, just append a scan block here; post-processing covers it automatically.
    let mut raw: Vec<SessionMeta> = Vec::new();

    // Claude: ~/.claude/projects/**/*.jsonl
    if let Some(dir) = dirs::home_dir().map(|h| h.join(".claude").join("projects")) {
        if dir.exists() {
            // file_type() uses the d_type that readdir gives directly, with no extra syscall;
            // the original `path.is_dir() || path.read_link().is_ok()` was a stat + readlink, two calls.
            // Semantically equivalent: symlinked directories are skipped under both forms (is_dir follows the link and is true, and read_link is true as well;
            // file_type does not follow links, and is_dir on a symlink is simply false).
            let project_dirs: Vec<PathBuf> = fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect();
            // This section is deliberately kept serial. mtime can only be obtained via stat, which measured locally takes about 0.7ms each
            // (far above the normal microsecond level; the kernel side is probably being interfered with by EDR / Spotlight),
            // about 0.5s in total for 308 files. But splitting by project directory across threads was tried: 495-890ms, jitterier than serial and
            // the mean did not drop -- this part is not CPU bound, it is serialized inside the kernel, and more threads only add noise.
            // The real way out is a persistent mtime cache (#144), out of scope here.
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

    // -- Phase 2: unified post-processing (applies to all tools) ---------------

    // archive marker
    for meta in &mut raw {
        if archived_set.contains(&meta.session_id) {
            meta.archived = true;
        }
    }

    // child process collection (running session)
    let proc_table = proc_table_job.join().unwrap_or_default();
    timer.mark("进程表（等并行的 ps）");
    for meta in &mut raw {
        if meta.running && meta.pid > 0 {
            meta.child_processes = descendants_of(meta.pid, &proc_table);
        }
    }

    // orphan process association (via the MAKIT_SESSION_ID environment variable, detached processes with PPID=1)
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

    // live git branch override
    let mut branch_cache: HashMap<String, Option<String>> = HashMap::new();
    for meta in &mut raw {
        let cwd_key = if !meta.last_cwd.is_empty() { meta.last_cwd.clone() } else { meta.cwd.clone() };
        if cwd_key.is_empty() { continue; }
        if let Some(b) = branch_cache.entry(cwd_key.clone()).or_insert_with(|| current_branch_for(&cwd_key)).clone() {
            meta.git_branch = b;
        }
    }

    timer.mark("git 分支覆盖");

    // -- Phase 3: sort by mtime -------------------------------------------------
    raw.sort_by(|a, b| b.mtime.cmp(&a.mtime));

    save_scan_cache_throttled(true);
    timer.mark("缓存写盘");
    timer.record("list_sessions", serde_json::json!({ "sessions": raw.len() }));
    Ok(raw)
}

/// Incremental parse: parses only the few jsonl files reported by the watcher and does not touch any other file.
///
/// Why list_sessions cannot be called directly: it stats + fully parses ~220 files (771MB in total),
/// while projects/ is watched **recursively** and fires on every appended message -- which is exactly why the frontend originally wired
/// "sessions-changed" to a no-op. Converted, the cost here is about 1/220 of a full pass.
///
/// Post-processing deliberately does one thing less: child_processes would spawn `ps -eo` for the full table, which is skipped here,
/// and the frontend keeps the old value when merging (a new session's child process list is filled in at the next full load).
/// The rest (archived marker, live git branch override) matches Phase 2 of list_sessions.
pub fn list_sessions_by_paths(
    paths: Vec<String>,
    cwd_mode: Option<String>,
) -> Result<Vec<SessionMeta>, String> {
    let timer = PhaseTimer::new();
    let now = chrono::Local::now().timestamp();
    // pty_id is obtained in here (one `ps -p` per running session). The frontend relies on it to bind
    // a new session to an already opened tab, so this cost cannot be skipped.
    let running_info = load_running_info();
    let archived_set = load_archived();
    let mode = cwd_mode.as_deref().unwrap_or("smart");
    let codex_dir = ai_provider::AiTool::Codex.sessions_dir();

    let mut git_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut branch_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut out: Vec<SessionMeta> = Vec::new();

    for p in &paths {
        let path = Path::new(p);
        // When the file has been deleted metadata fails, and the parse right after it fails too -> naturally skipped.
        // That is, "a session file was deleted" cannot be synced here; it has to wait for a full load (manual refresh).
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

    // While a session is producing output this runs once more per second; only slow ones are recorded, to avoid flooding the log (#218)
    if timer.total_ms() > 100.0 {
        timer.record("list_sessions_by_paths", serde_json::json!({ "files": paths.len() }));
    }
    save_scan_cache_throttled(false);
    Ok(out)
}

#[cfg(test)]
mod git_head_tests {
    use std::process::Command;

    // The control uses `symbolic-ref --short -q HEAD` rather than `branch --show-current`: the latter only exists since git 2.22,
    // and if an old git comes first on PATH (e.g. 2.15) it errors straight out and returns None,
    // and the test would misreport "the control group is broken" as "the fast path is wrong". Both have the same semantics: the branch name, or empty when HEAD is detached.
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

    /// Regression: the fast path reading .git/HEAD must match `git branch --show-current` byte for byte.
    /// It uses session cwds that really exist locally as the corpus, covering ordinary repositories / linked worktrees /
    /// non-repository directories / deleted paths.
    #[test]
    fn head_read_matches_git_cli() {
        let mut dirs: Vec<String> = vec![
            env!("CARGO_MANIFEST_DIR").to_string(),
            "/tmp".to_string(),
            "/definitely/not/a/path".to_string(),
        ];
        // An empty cwd is kept out of the comparison corpus: `git -C "" ...` degrades to "run in the current process directory",
        // so the old implementation would stick the branch of the repository makit itself lives in onto a session whose cwd is unknown.
        // The caller already filters out empty values; here the correct behaviour is pinned down directly.
        assert_eq!(super::current_branch_for(""), None);
        // real corpus: the cwds of all sessions
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
    /// How to run: MAKIT_TIMING=1 cargo test --release -- --ignored --nocapture bench_list_sessions
    /// release rather than dev: under dev serde_json is not inlined, so what gets measured is the compiler, not the algorithm.
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
    // the real origin of slug: measured, of 8 sessions with a slug, all 8 were written by this record
    const COMPACT_WITH_SLUG: &str =
        r#"{"type":"system","subtype":"compact_boundary","slug":"clever-swimming-flute","content":"Conversation compacted"}"#;
    const CUSTOM_TITLE: &str = r#"{"type":"custom-title","customTitle":"oc-install-pack脚本编写"}"#;

    /// Regression: `/compact` must not change a session's display name.
    ///
    /// claude's `slug` (a random three-word codename like `clever-swimming-flute`) is written **only** on
    /// `type:"system", subtype:"compact_boundary"` records and has nothing to do with the session content.
    /// It used to be stuffed into `display_name`, and the frontend treats display_name as authoritative
    /// (`display_name || first_user_msg`), so the same session, **after being compacted once, had its title degrade from
    /// "build the current project" to "clever-swimming-flute"** -- the more someone uses /compact, the worse it hits them.
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
        // the frontend's `display_name || first_user_msg` must be able to fall back to the first message
        assert_eq!(after.first_user_msg, "build当前项目");
    }

    /// Reverse guard: the real title claude wrote (customTitle / agentName) must still override everything,
    /// including the compact slug. The test above must not pass by "clearing display_name unconditionally".
    #[test]
    fn real_title_still_wins() {
        let m = parse(&write_jsonl("title", &[USER_MSG, COMPACT_WITH_SLUG, CUSTOM_TITLE]));
        assert_eq!(m.display_name, "oc-install-pack脚本编写");
        assert_eq!(m.name_source, "custom-title");
    }
}

// On-demand attribution: scan ~/.claude/projects/*/*.jsonl for files "newly created" after the tab started (birthtime > after_ts) whose cwd matches
// birthtime is used rather than mtime -- mtime is refreshed by any write, and old sessions would be hit by mistake
// Purpose: after a new/shell tab starts, look up the session file claude wrote
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
            // prefer birthtime (supported by macOS APFS); fall back to mtime when unsupported
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
            // leave 1s of slack: tab.startedAt is second-granular and the file birthtime is second-granular too, so they may fall in the same second
            if ctime + 1 < after_ts {
                continue;
            }
            candidates.push((fp, ctime));
        }
    }
    // newest creation time matches first
    candidates.sort_by(|a, b| b.1.cmp(&a.1));
    for (p, _mtime) in &candidates {
        let file = match fs::File::open(p) {
            Ok(f) => f,
            Err(_) => continue,
        };
        let reader = BufReader::new(file);
        // scan the first 30 lines for any record with a cwd field
        // the attachment line written by the SessionStart hook while claude starts already contains cwd, no need to wait for the user line
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
                    // cwd does not match, this file has no hope
                    break;
                }
            }
        }
    }
    Ok(None)
}

// Incrementally read a single session's meta, avoiding re-pulling the whole list_sessions after attribution (the visible stall mostly came from the latter)
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
