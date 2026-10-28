//! opencode sessions (#284).
//!
//! Unlike Claude (jsonl per session) and Codex (rollout jsonl), opencode keeps everything in one SQLite database:
//! `~/.local/share/opencode/opencode.db` (also on macOS; `XDG_DATA_HOME` and `OPENCODE_DB` override it).
//! Shape of the released version (1.19.3, measured): tables `session` / `message` / `part`, the content is JSON in the `data` column.
//!
//! - The database is opened **read-only** (opencode is writing it in WAL mode while we read; we get a consistent snapshot and never take a lock).
//! - The schema is opencode's, not ours, and has changed before. Anything unrecognised degrades to "no opencode sessions" with a log line, never a panic.
//! - Sub-agent sessions (`parent_id` set) and archived sessions are not listed, the same as opencode's own session list.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::TimeZone;
use rusqlite::{Connection, OpenFlags};

use crate::sessions::{get_git_root, humanize_duration, SessionMeta};

/// Directory that holds `opencode.db` (and its `-wal` / `-shm`).
pub fn data_dir() -> Option<PathBuf> {
    data_dir_from(
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from),
        dirs::home_dir(),
    )
}

fn data_dir_from(xdg_data_home: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    // Per the XDG spec a relative value is invalid and must be ignored
    let base = match xdg_data_home {
        Some(p) if p.is_absolute() => p,
        _ => home?.join(".local").join("share"),
    };
    Some(base.join("opencode"))
}

/// The database file. `OPENCODE_DB` (absolute path, or a file name inside the data directory) wins, as in opencode itself.
pub fn db_path() -> Option<PathBuf> {
    let dir = data_dir()?;
    match std::env::var_os("OPENCODE_DB").map(PathBuf::from) {
        Some(p) if p.is_absolute() => Some(p),
        Some(p) if !p.as_os_str().is_empty() => Some(dir.join(p)),
        _ => Some(dir.join("opencode.db")),
    }
}

/// Whether a changed path is the database or its WAL / shared-memory file (what the watcher reports for opencode).
pub fn is_db_file(path: &Path, db: &Path) -> bool {
    if path == db {
        return true;
    }
    let (Some(name), Some(db_name)) = (path.file_name().and_then(|n| n.to_str()), db.file_name().and_then(|n| n.to_str())) else {
        return false;
    };
    path.parent() == db.parent() && (name == format!("{db_name}-wal") || name == format!("{db_name}-shm"))
}

/// Command that resumes a session. Confirmed against `opencode --help` (`-s, --session  session id to continue`).
pub fn resume_command(session_id: &str) -> String {
    format!("opencode -s {session_id}")
}

/// Open read-only. A database that does not exist is an error here (the caller checks existence first).
pub fn open_read_only(db: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
        .map_err(|e| format!("open {}: {e}", db.display()))?;
    // opencode may be mid-write; a short wait beats an immediate SQLITE_BUSY
    let _ = conn.busy_timeout(std::time::Duration::from_millis(500));
    Ok(conn)
}

/// One row of `session`, plus what we derive from `message` / `part`.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSession {
    pub id: String,
    pub directory: String,
    pub title: String,
    /// Model id from the `model` JSON column (`{"id": "...", "providerID": "..."}`), empty when unknown
    pub model: String,
    pub time_updated_ms: i64,
    pub user_msg_count: u32,
    pub first_user_msg: String,
    pub last_user_msg: String,
}

/// opencode names a fresh session `New session - <ISO time>` until the model renames it; that is not a title worth showing.
pub fn is_placeholder_title(title: &str) -> bool {
    title.trim().is_empty() || title.starts_with("New session - ") || title.starts_with("Child session - ")
}

fn has_columns(conn: &Connection, table: &str, wanted: &[&str]) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
        .map_err(|e| e.to_string())?;
    let cols: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    for w in wanted {
        if !cols.iter().any(|c| c == w) {
            return Err(format!("table `{table}` has no column `{w}` (columns: {})", cols.join(", ")));
        }
    }
    Ok(cols)
}

/// Check that the database has the shape we know (the released v1 shape). Returns whether the optional `model` column exists.
fn check_schema(conn: &Connection) -> Result<bool, String> {
    let session_cols = has_columns(
        conn,
        "session",
        &["id", "directory", "title", "parent_id", "time_updated", "time_archived"],
    )?;
    has_columns(conn, "message", &["id", "session_id", "time_created", "data"])?;
    has_columns(conn, "part", &["id", "message_id", "data"])?;
    Ok(session_cols.iter().any(|c| c == "model"))
}

/// The user-visible text of a message: its first non-empty `text` part (JSON `{"type":"text","text":"..."}`), skipping synthetic parts.
fn message_text(conn: &Connection, message_id: &str) -> String {
    let Ok(mut stmt) = conn.prepare_cached("SELECT data FROM part WHERE message_id = ?1 ORDER BY id") else {
        return String::new();
    };
    let Ok(rows) = stmt.query_map([message_id], |r| r.get::<_, String>(0)) else {
        return String::new();
    };
    for data in rows.flatten() {
        if let Some(t) = text_of_part(&data) {
            return t;
        }
    }
    String::new()
}

fn text_of_part(data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    if v.get("type")?.as_str()? != "text" || v.get("synthetic").and_then(|s| s.as_bool()) == Some(true) {
        return None;
    }
    let t = v.get("text")?.as_str()?.trim();
    (!t.is_empty()).then(|| t.chars().take(120).collect())
}

fn user_message_ids(conn: &Connection, session_id: &str) -> Result<(u32, Option<String>, Option<String>), String> {
    // json_extract keeps the (potentially large) assistant messages out of Rust; the index on (session_id, time_created) orders it
    let count: u32 = conn
        .query_row(
            "SELECT count(*) FROM message WHERE session_id = ?1 AND json_extract(data, '$.role') = 'user'",
            [session_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if count == 0 {
        return Ok((0, None, None));
    }
    let pick = |order: &str| -> Option<String> {
        conn.query_row(
            &format!(
                "SELECT id FROM message WHERE session_id = ?1 AND json_extract(data, '$.role') = 'user' ORDER BY time_created {order}, id {order} LIMIT 1"
            ),
            [session_id],
            |r| r.get(0),
        )
        .ok()
    };
    Ok((count, pick("ASC"), pick("DESC")))
}

/// Read the top-level, non-archived sessions, newest first. `since_ms`: only sessions updated after it (incremental refresh).
pub fn read_sessions(conn: &Connection, since_ms: Option<i64>) -> Result<Vec<RawSession>, String> {
    let has_model = check_schema(conn)?;
    let sql = format!(
        "SELECT id, directory, title, {}, time_updated FROM session \
         WHERE parent_id IS NULL AND time_archived IS NULL AND time_updated > ?1 ORDER BY time_updated DESC",
        if has_model { "model" } else { "NULL" }
    );
    let mut stmt = conn.prepare(&sql).map_err(|e| e.to_string())?;
    let rows: Vec<(String, String, String, Option<String>, i64)> = stmt
        .query_map([since_ms.unwrap_or(i64::MIN)], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();

    let mut out = Vec::with_capacity(rows.len());
    for (id, directory, title, model, time_updated_ms) in rows {
        let (count, first_id, last_id) = user_message_ids(conn, &id)?;
        out.push(RawSession {
            first_user_msg: first_id.map(|m| message_text(conn, &m)).unwrap_or_default(),
            last_user_msg: last_id.map(|m| message_text(conn, &m)).unwrap_or_default(),
            user_msg_count: count,
            model: model_id(model.as_deref()),
            id,
            directory,
            title,
            time_updated_ms,
        });
    }
    Ok(out)
}

fn model_id(json: Option<&str>) -> String {
    json.and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok())
        .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(String::from))
        .unwrap_or_default()
}

/// `ses_e7d75541bffe...` -> `e7d75541` (the `ses_` prefix is the same on every id, useless as a short id)
fn short_id_of(id: &str) -> String {
    id.strip_prefix("ses_").unwrap_or(id).chars().take(8).collect()
}

/// RawSession -> SessionMeta (pure apart from the git lookup, which goes through the caller's cache).
pub fn to_meta(raw: &RawSession, now: i64, git_cache: &mut HashMap<String, Option<String>>) -> SessionMeta {
    let mtime = raw.time_updated_ms / 1000;
    let short_id = short_id_of(&raw.id);
    let (display_name, name_source) = if !is_placeholder_title(&raw.title) {
        (raw.title.trim().to_string(), "title")
    } else if !raw.first_user_msg.is_empty() {
        (raw.first_user_msg.clone(), "first_msg")
    } else {
        (short_id.clone(), "first_msg")
    };
    let mtime_display = chrono::Local
        .timestamp_opt(mtime, 0)
        .single()
        .unwrap_or_else(chrono::Local::now)
        .format("%m-%d %H:%M")
        .to_string();
    SessionMeta {
        session_id: raw.id.clone(),
        short_id,
        cwd: raw.directory.clone(),
        last_cwd: raw.directory.clone(),
        git_branch: String::new(),
        user_msg_count: raw.user_msg_count,
        first_user_msg: raw.first_user_msg.clone(),
        last_user_msg: raw.last_user_msg.clone(),
        mtime,
        mtime_display,
        humanize: humanize_duration(now - mtime),
        is_worktree: false,
        git_root: get_git_root(&raw.directory, git_cache).unwrap_or_default(),
        display_name,
        name_source: name_source.into(),
        running: false,
        status: "idle".into(),
        waiting_for: String::new(),
        pid: 0,
        child_processes: vec![],
        archived: false,
        storage_folder: String::new(),
        pty_id: String::new(),
        tool: "opencode".into(),
    }
}

/// Newest `time_updated` (ms) seen by the last scan: the incremental refresh only asks for what changed after it.
static WATERMARK_MS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// opencode's sessions as SessionMeta. No database / unreadable / unknown schema -> empty, with the reason logged.
/// `full`: everything (startup, manual refresh). Otherwise only sessions updated since the previous scan (watcher refresh).
pub fn list_sessions(full: bool, now: i64, git_cache: &mut HashMap<String, Option<String>>) -> Vec<SessionMeta> {
    use std::sync::atomic::Ordering;
    let Some(db) = db_path().filter(|p| p.exists()) else {
        return vec![];
    };
    // minus 1 ms: two writes in the same millisecond must not hide the second one
    let since = (!full).then(|| WATERMARK_MS.load(Ordering::Relaxed).saturating_sub(1));
    match open_read_only(&db).and_then(|c| read_sessions(&c, since)) {
        Ok(raws) => {
            if let Some(newest) = raws.iter().map(|r| r.time_updated_ms).max() {
                WATERMARK_MS.fetch_max(newest, Ordering::Relaxed);
            }
            raws.iter().map(|r| to_meta(r, now, git_cache)).collect()
        }
        Err(e) => {
            log::warn!(target: "opencode", "cannot read the opencode sessions (not shown): {e}");
            vec![]
        }
    }
}

/// What the environment check needs to know about the database.
#[derive(Debug, Clone, PartialEq)]
pub enum DbProbe {
    Missing,
    /// Readable, with this many top-level sessions
    Ok(usize),
    /// Opens, but the tables / columns are not the ones we know
    BadSchema(String),
    OpenFailed(String),
}

pub fn probe(db: &Path) -> DbProbe {
    if !db.exists() {
        return DbProbe::Missing;
    }
    match open_read_only(db) {
        Err(e) => DbProbe::OpenFailed(e),
        Ok(conn) => match read_candidates(&conn) {
            Ok(c) => DbProbe::Ok(c.len()),
            Err(e) => DbProbe::BadSchema(e),
        },
    }
}

/// A transcript reader for an opencode session, if the database has that session.
pub fn transcript_reader(session_id: &str) -> Option<crate::transcript::TranscriptReader> {
    let db = db_path().filter(|p| p.exists())?;
    let conn = open_read_only(&db).ok()?;
    let found = conn.query_row("SELECT 1 FROM session WHERE id = ?1", [session_id], |_| Ok(())).is_ok();
    found.then(|| crate::transcript::TranscriptReader::opencode(db, session_id.to_string()))
}

// --------------- running state and tab binding ---------------
//
// opencode writes no "I am running" file (claude does), and keeps no session file open (codex does). What exists:
// - a live `opencode` process; its command line may carry `-s <session id>` (what makit's resume command passes), and its working directory is the
//   directory the session was started in;
// - the database: a session created by this process has `directory` = that working directory and `time_created` after the process started;
// - whether it is busy: the newest assistant message has no `time.completed` while a reply is being produced.

/// A live opencode terminal UI process.
#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    /// Full command line, including the program name
    pub args: String,
    pub cwd: String,
    /// When the process started (ms since the epoch)
    pub started_ms: i64,
}

/// What matching needs to know about a stored session.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub directory: String,
    pub time_created_ms: i64,
    pub time_updated_ms: i64,
}

/// Subcommands that are not the terminal UI (`opencode serve` is a headless server, `opencode run` a one-shot, ...).
const NON_TUI_SUBCOMMANDS: &[&str] = &[
    "completion", "acp", "mcp", "attach", "run", "debug", "providers", "auth", "agent", "upgrade", "uninstall", "serve", "web",
    "models", "stats", "export", "import", "github", "pr", "session", "plugin", "plug", "db",
];

/// Whether this command line is the interactive terminal UI (the default command), not a server / one-shot subcommand.
pub fn is_tui_command(args: &str) -> bool {
    match args.split_whitespace().nth(1) {
        Some(first) => !NON_TUI_SUBCOMMANDS.contains(&first),
        None => true,
    }
}

/// `-s <id>` / `--session <id>` / `--session=<id>` in the command line.
pub fn session_arg(args: &str) -> Option<String> {
    let mut it = args.split_whitespace().skip(1);
    while let Some(t) = it.next() {
        if let Some(v) = t.strip_prefix("--session=") {
            return (!v.is_empty()).then(|| v.to_string());
        }
        if t == "-s" || t == "--session" {
            return it.next().map(String::from);
        }
    }
    None
}

fn has_continue_flag(args: &str) -> bool {
    args.split_whitespace().skip(1).any(|t| t == "-c" || t == "--continue")
}

/// The session a terminal UI process is working in, if it can be told.
/// 1. explicit `-s <id>`;
/// 2. a session created by this very process: same directory, created after it started (the newest such one);
/// 3. `-c`: the newest session in its directory.
/// Otherwise unknown (e.g. the user switched session inside the UI of a process started without arguments): better no answer than a wrong one.
pub fn session_of_proc(p: &Proc, candidates: &[Candidate]) -> Option<String> {
    if let Some(id) = session_arg(&p.args) {
        return Some(id);
    }
    // 2 s of slack: the process clock and opencode's timestamps are not taken at the same instant
    let newest = |pred: &dyn Fn(&Candidate) -> bool| {
        candidates
            .iter()
            .filter(|c| c.directory == p.cwd && pred(c))
            .max_by_key(|c| c.time_updated_ms)
            .map(|c| c.id.clone())
    };
    newest(&|c| c.time_created_ms >= p.started_ms - 2_000).or_else(|| if has_continue_flag(&p.args) { newest(&|_| true) } else { None })
}

/// `ps -o etime` -> seconds. Formats: `SS`-less `MM:SS`, `HH:MM:SS`, `D-HH:MM:SS`.
pub fn parse_etime(s: &str) -> Option<u64> {
    let s = s.trim();
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let mut secs = 0u64;
    let parts: Vec<&str> = rest.split(':').collect();
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    for p in parts {
        secs = secs * 60 + p.parse::<u64>().ok()?;
    }
    Some(days * 86_400 + secs)
}

/// Top-level, non-archived sessions as matching candidates.
pub fn read_candidates(conn: &Connection) -> Result<Vec<Candidate>, String> {
    check_schema(conn)?;
    has_columns(conn, "session", &["time_created"])?;
    let mut stmt = conn
        .prepare("SELECT id, directory, time_created, time_updated FROM session WHERE parent_id IS NULL AND time_archived IS NULL")
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Candidate { id: r.get(0)?, directory: r.get(1)?, time_created_ms: r.get(2)?, time_updated_ms: r.get(3)? })
        })
        .map_err(|e| e.to_string())?
        .flatten()
        .collect();
    Ok(rows)
}

/// A reply is being produced: the newest assistant message has not completed. (Only meaningful while the process is alive:
/// a crash leaves an incomplete message behind forever.)
pub fn is_busy(conn: &Connection, session_id: &str) -> bool {
    conn.query_row(
        "SELECT json_extract(data, '$.time.completed') IS NULL FROM message \
         WHERE session_id = ?1 AND json_extract(data, '$.role') = 'assistant' ORDER BY time_created DESC, id DESC LIMIT 1",
        [session_id],
        |r| r.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// Running opencode terminal UIs on this machine (any terminal, not only makit's tabs). unix only (`pgrep` / `ps` / `lsof`);
/// elsewhere it is empty without a log line, as it would repeat every few seconds.
#[cfg(unix)]
pub fn live_processes(now_ms: i64) -> Vec<Proc> {
    use std::process::Command;
    let Ok(out) = Command::new("pgrep").args(["-x", "opencode"]).output() else { return vec![] };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<u32>().ok())
        .filter_map(|pid| {
            let ps = Command::new("ps").args(["-o", "etime=,args=", "-p", &pid.to_string()]).output().ok()?;
            let text = String::from_utf8_lossy(&ps.stdout).trim().to_string();
            let (etime, args) = text.split_once(char::is_whitespace)?;
            let args = args.trim().to_string();
            if !is_tui_command(&args) {
                return None;
            }
            let started_ms = now_ms - parse_etime(etime)? as i64 * 1000;
            let lsof = Command::new("lsof").args(["-a", "-p", &pid.to_string(), "-d", "cwd", "-Fn"]).output().ok()?;
            let cwd = String::from_utf8_lossy(&lsof.stdout).lines().find_map(|l| l.strip_prefix('n').map(String::from))?;
            Some(Proc { pid, args, cwd, started_ms })
        })
        .collect()
}

#[cfg(not(unix))]
pub fn live_processes(_now_ms: i64) -> Vec<Proc> {
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database with opencode 1.19.3's released shape (only the columns we care about and the ones that matter for the filters).
    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE session (id text PRIMARY KEY, project_id text NOT NULL, parent_id text, directory text NOT NULL,
                title text NOT NULL, model text, time_created integer NOT NULL, time_updated integer NOT NULL, time_archived integer);
             CREATE TABLE message (id text PRIMARY KEY, session_id text NOT NULL, time_created integer NOT NULL,
                time_updated integer NOT NULL, data text NOT NULL);
             CREATE TABLE part (id text PRIMARY KEY, message_id text NOT NULL, session_id text NOT NULL,
                time_created integer NOT NULL, time_updated integer NOT NULL, data text NOT NULL);",
        )
        .unwrap();
        c
    }

    fn session(c: &Connection, id: &str, parent: Option<&str>, title: &str, updated: i64, archived: Option<i64>) {
        c.execute(
            "INSERT INTO session VALUES (?1, 'p', ?2, '/Users/me/proj', ?3, '{\"id\":\"big-pickle\",\"providerID\":\"opencode\"}', 1, ?4, ?5)",
            rusqlite::params![id, parent, title, updated, archived],
        )
        .unwrap();
    }

    fn msg(c: &Connection, id: &str, session: &str, t: i64, role: &str, text: Option<&str>) {
        c.execute(
            "INSERT INTO message VALUES (?1, ?2, ?3, ?3, ?4)",
            rusqlite::params![id, session, t, format!("{{\"role\":\"{role}\",\"time\":{{\"created\":{t}}}}}")],
        )
        .unwrap();
        // every message gets a step-start first, like the real data: the text must be found past it
        c.execute(
            "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, '{\"type\":\"step-start\"}')",
            rusqlite::params![format!("{id}-a"), id, session, t],
        )
        .unwrap();
        if let Some(text) = text {
            c.execute(
                "INSERT INTO part VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
                rusqlite::params![
                    format!("{id}-b"),
                    id,
                    session,
                    t,
                    serde_json::json!({"type": "text", "text": text}).to_string()
                ],
            )
            .unwrap();
        }
    }

    /// Why: this is the sidebar row. Wrong here = a session shows the wrong name, count or order.
    #[test]
    fn reads_title_dir_model_counts_and_first_last_user_text() {
        let c = db();
        session(&c, "ses_aaaa1111bbbb", None, "Fix the bug", 5_000, None);
        msg(&c, "m1", "ses_aaaa1111bbbb", 10, "user", Some("first question"));
        msg(&c, "m2", "ses_aaaa1111bbbb", 20, "assistant", Some("an answer"));
        msg(&c, "m3", "ses_aaaa1111bbbb", 30, "user", Some("last question"));
        let got = read_sessions(&c, None).unwrap();
        assert_eq!(got.len(), 1);
        let r = &got[0];
        assert_eq!(
            (r.id.as_str(), r.directory.as_str(), r.title.as_str(), r.model.as_str(), r.time_updated_ms),
            ("ses_aaaa1111bbbb", "/Users/me/proj", "Fix the bug", "big-pickle", 5_000)
        );
        assert_eq!((r.user_msg_count, r.first_user_msg.as_str(), r.last_user_msg.as_str()), (2, "first question", "last question"));
    }

    /// Why: sub-agent sessions and archived sessions would flood the sidebar with rows opencode itself hides.
    #[test]
    fn skips_child_and_archived_sessions_and_sorts_newest_first() {
        let c = db();
        session(&c, "old", None, "old", 1_000, None);
        session(&c, "new", None, "new", 9_000, None);
        session(&c, "kid", Some("new"), "sub agent", 9_500, None);
        session(&c, "gone", None, "archived", 9_900, Some(9_900));
        let ids: Vec<String> = read_sessions(&c, None).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["new", "old"]);
    }

    /// Why: the watcher fires on every opencode write; a full re-read each time would be wasteful.
    #[test]
    fn since_returns_only_newer_sessions() {
        let c = db();
        session(&c, "old", None, "old", 1_000, None);
        session(&c, "new", None, "new", 9_000, None);
        let ids: Vec<String> = read_sessions(&c, Some(1_000)).unwrap().into_iter().map(|r| r.id).collect();
        assert_eq!(ids, vec!["new"], "strictly newer: the watermark itself was already seen");
    }

    /// Why: a brand-new session has no text yet and a placeholder title; the row must still have a name and not crash.
    #[test]
    fn session_without_messages_has_zero_count_and_falls_back_to_short_id() {
        let c = db();
        session(&c, "ses_e7d75541bffe", None, "New session - 2026-10-28T12:00:00.000Z", 2_000, None);
        let raw = read_sessions(&c, None).unwrap().remove(0);
        assert_eq!((raw.user_msg_count, raw.first_user_msg.as_str()), (0, ""));
        let meta = to_meta(&raw, 3, &mut HashMap::new());
        assert_eq!((meta.display_name.as_str(), meta.short_id.as_str(), meta.tool.as_str()), ("e7d75541", "e7d75541", "opencode"));
    }

    /// Why: until the model renames the session the title is a timestamp; the first question is what you'd recognise it by.
    #[test]
    fn placeholder_title_falls_back_to_first_user_message() {
        let c = db();
        session(&c, "s", None, "New session - 2026-10-28T12:00:00.000Z", 2_000, None);
        msg(&c, "m1", "s", 10, "user", Some("  how do I add a column?  "));
        let meta = to_meta(&read_sessions(&c, None).unwrap().remove(0), 3, &mut HashMap::new());
        assert_eq!((meta.display_name.as_str(), meta.name_source.as_str()), ("how do I add a column?", "first_msg"));
    }

    #[test]
    fn real_title_wins() {
        let c = db();
        session(&c, "s", None, "Refactor sidebar", 2_000, None);
        msg(&c, "m1", "s", 10, "user", Some("hello"));
        let meta = to_meta(&read_sessions(&c, None).unwrap().remove(0), 3, &mut HashMap::new());
        assert_eq!((meta.display_name.as_str(), meta.name_source.as_str()), ("Refactor sidebar", "title"));
        assert_eq!((meta.mtime, meta.cwd.as_str(), meta.last_cwd.as_str()), (2, "/Users/me/proj", "/Users/me/proj"));
    }

    /// Why: opencode owns the schema. If a future version renames a column we must say "can't read it", not crash or show garbage.
    #[test]
    fn unknown_schema_is_an_error_not_a_panic() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE session (id text, title text);").unwrap();
        let err = read_sessions(&c, None).unwrap_err();
        assert!(err.contains("`session`") && err.contains("directory"), "{err}");
        // not even a session table
        let c = Connection::open_in_memory().unwrap();
        assert!(read_sessions(&c, None).is_err());
    }

    #[test]
    fn model_column_is_optional() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE session (id text, parent_id text, directory text, title text, time_updated integer, time_archived integer);
             CREATE TABLE message (id text, session_id text, time_created integer, data text);
             CREATE TABLE part (id text, message_id text, data text);
             INSERT INTO session VALUES ('s', NULL, '/d', 't', 5, NULL);",
        )
        .unwrap();
        let got = read_sessions(&c, None).unwrap();
        assert_eq!((got.len(), got[0].model.as_str()), (1, ""));
    }

    /// Why: the same open call opencode's own WAL database goes through. Read-only must work on a real file, and must never write to it.
    #[test]
    fn opens_a_real_wal_database_read_only() {
        let dir = std::env::temp_dir().join(format!("makit-oc-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("opencode.db");
        {
            let w = Connection::open(&path).unwrap();
            w.pragma_update(None, "journal_mode", "WAL").unwrap();
            w.execute_batch(
                "CREATE TABLE session (id text, project_id text, parent_id text, directory text, title text, model text, time_created integer, time_updated integer, time_archived integer);
                 CREATE TABLE message (id text, session_id text, time_created integer, time_updated integer, data text);
                 CREATE TABLE part (id text, message_id text, session_id text, time_created integer, time_updated integer, data text);
                 INSERT INTO session VALUES ('s','p',NULL,'/d','t',NULL,1,7,NULL);",
            )
            .unwrap();
            // the writer stays open, like a running opencode
            let r = open_read_only(&path).unwrap();
            assert_eq!(read_sessions(&r, None).unwrap().len(), 1);
            assert!(r.execute("DELETE FROM session", []).is_err(), "a read-only connection must refuse writes");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn data_dir_follows_xdg_and_ignores_relative_values() {
        let home = Some(PathBuf::from("/Users/me"));
        assert_eq!(data_dir_from(None, home.clone()), Some(PathBuf::from("/Users/me/.local/share/opencode")));
        assert_eq!(data_dir_from(Some("/x/data".into()), home.clone()), Some(PathBuf::from("/x/data/opencode")));
        assert_eq!(data_dir_from(Some("relative".into()), home), Some(PathBuf::from("/Users/me/.local/share/opencode")));
    }

    /// Why: the watcher must treat the database and its WAL / shm as "opencode changed" and nothing else in that directory.
    #[test]
    fn recognises_the_database_and_its_wal_files() {
        let db = PathBuf::from("/h/.local/share/opencode/opencode.db");
        for p in ["opencode.db", "opencode.db-wal", "opencode.db-shm"] {
            assert!(is_db_file(&db.with_file_name(p), &db), "{p}");
        }
        assert!(!is_db_file(Path::new("/h/.local/share/opencode/auth.json"), &db));
        assert!(!is_db_file(Path::new("/h/.local/share/opencode/log/opencode.db"), &db), "other directory");
    }

    #[test]
    fn resume_command_uses_dash_s() {
        assert_eq!(resume_command("ses_1"), "opencode -s ses_1");
    }

    /// Real-data smoke test (needs opencode installed on this machine): `cargo test opencode::tests::real_database -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_database() {
        let metas = list_sessions(true, 0, &mut HashMap::new());
        for m in &metas {
            println!("{} | {} | {} | n={} | {:?}", m.short_id, m.display_name, m.cwd, m.user_msg_count, m.first_user_msg);
        }
        assert!(!metas.is_empty());
    }

    fn proc(args: &str, cwd: &str, started_ms: i64) -> Proc {
        Proc { pid: 1, args: args.into(), cwd: cwd.into(), started_ms }
    }
    fn cand(id: &str, dir: &str, created: i64, updated: i64) -> Candidate {
        Candidate { id: id.into(), directory: dir.into(), time_created_ms: created, time_updated_ms: updated }
    }

    /// Why: makit's resume command passes `-s <id>`; that is the one case where the process names its session outright.
    #[test]
    fn session_arg_forms() {
        assert_eq!(session_arg("opencode -s ses_1"), Some("ses_1".into()));
        assert_eq!(session_arg("opencode --session ses_2 --model x"), Some("ses_2".into()));
        assert_eq!(session_arg("opencode --session=ses_3"), Some("ses_3".into()));
        assert_eq!(session_arg("opencode /some/project"), None);
        assert_eq!(session_arg("opencode -s"), None, "a dangling flag names nothing");
    }

    /// Why: `opencode serve` (a headless server, also run by the desktop app) is not a session in a terminal; counting it would mark random sessions running.
    #[test]
    fn only_the_terminal_ui_counts() {
        assert!(is_tui_command("opencode"));
        assert!(is_tui_command("opencode -s ses_1"));
        assert!(is_tui_command("opencode /path/to/project"));
        for sub in ["opencode serve --port 4096", "opencode run hello", "opencode web", "opencode mcp list", "opencode db path"] {
            assert!(!is_tui_command(sub), "{sub}");
        }
    }

    /// Why: a session started by typing `opencode` in a tab has no id on the command line; the tab would never get its title / status dot.
    #[test]
    fn new_session_is_the_newest_one_created_in_the_cwd_after_the_process_started() {
        let cands = [
            cand("old", "/p", 100, 9_999), // created before the process: an older session, even though recently updated
            cand("mine", "/p", 10_500, 11_000),
            cand("mine2", "/p", 10_600, 12_000), // newer still
            cand("elsewhere", "/q", 10_700, 13_000),
        ];
        assert_eq!(session_of_proc(&proc("opencode", "/p", 10_000), &cands), Some("mine2".into()));
        assert_eq!(session_of_proc(&proc("opencode", "/p", 20_000), &cands), None, "nothing created since: unknown, not a guess");
        assert_eq!(session_of_proc(&proc("opencode", "/nowhere", 10_000), &cands), None);
    }

    #[test]
    fn explicit_id_beats_everything_and_continue_picks_the_newest_in_cwd() {
        let cands = [cand("a", "/p", 100, 200), cand("b", "/p", 100, 300)];
        assert_eq!(session_of_proc(&proc("opencode -s zzz", "/p", 0), &cands), Some("zzz".into()));
        assert_eq!(session_of_proc(&proc("opencode -c", "/p", 10_000), &cands), Some("b".into()));
        assert_eq!(session_of_proc(&proc("opencode", "/p", 10_000), &cands), None);
    }

    #[test]
    fn etime_formats() {
        assert_eq!(parse_etime("05:07"), Some(307));
        assert_eq!(parse_etime(" 01:02:03 "), Some(3723));
        assert_eq!(parse_etime("2-00:00:10"), Some(172_810));
        assert_eq!(parse_etime("garbage"), None);
    }

    /// Why: this is what turns the status dot from "idle" to "running a reply".
    #[test]
    fn busy_means_the_newest_assistant_message_has_not_completed() {
        let c = db();
        session(&c, "s", None, "t", 1, None);
        let asst = |id: &str, t: i64, completed: bool| {
            let data = if completed {
                format!("{{\"role\":\"assistant\",\"time\":{{\"created\":{t},\"completed\":{}}}}}", t + 1)
            } else {
                format!("{{\"role\":\"assistant\",\"time\":{{\"created\":{t}}}}}")
            };
            c.execute("INSERT INTO message VALUES (?1, 's', ?2, ?2, ?3)", rusqlite::params![id, t, data]).unwrap();
        };
        assert!(!is_busy(&c, "s"), "no assistant message at all");
        asst("a1", 10, true);
        assert!(!is_busy(&c, "s"));
        asst("a2", 20, false);
        assert!(is_busy(&c, "s"), "newest one is still streaming");
        asst("a3", 30, true);
        assert!(!is_busy(&c, "s"), "only the newest counts");
    }

    #[test]
    fn candidates_are_top_level_and_not_archived() {
        let c = db();
        session(&c, "a", None, "t", 5, None);
        session(&c, "kid", Some("a"), "t", 6, None);
        session(&c, "gone", None, "t", 7, Some(7));
        let got = read_candidates(&c).unwrap();
        assert_eq!(got.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), vec!["a"]);
    }
}
