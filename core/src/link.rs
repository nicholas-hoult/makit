//! Local session link (#299): lets another program on this machine list makit's sessions and read their conversations.
//!
//! Protocol v1: unix socket `~/.makit/link.sock` (directory 0700, socket 0600), one JSON line in, one JSON line out, then the
//! connection closes. Stateless, so polling is enough and a restart of either side needs no reconnect logic.
//! `transcript` takes `after` (items after that index) or `tail` (only the last N, for a first look at a long conversation).
//! Read-only in this version (`hello` / `sessions` / `transcript`).
//!
//! Layout: [`handle_line`] is a pure function over a [`Backend`] (tests use a fake one); [`CoreBackend`] is the real
//! backend on top of `sessions` / `transcript`; [`start`] / [`start_at`] put it on a socket.
//! No UI framework and no async runtime, same as `hook`.

use crate::transcript::{Item, ItemKind, Level, Tool, TranscriptReader};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const PROTO: u32 = 1;
/// A request line longer than this is refused (a request is a few dozen bytes).
pub const MAX_REQUEST: usize = 64 * 1024;
/// How many sessions `sessions` returns when the request gives no limit / a bigger one.
pub const DEFAULT_LIMIT: usize = 50;
pub const MAX_LIMIT: usize = 200;
/// How many items one `transcript` response carries at most (the rest comes with the next poll).
pub const PAGE_ITEMS: usize = 300;
/// How many transcript readers `CoreBackend` keeps; the least recently used one is dropped beyond this.
const MAX_READERS: usize = 8;

/// First index to return for `cursor` when the conversation has `total` items.
pub fn cursor_start(total: usize, cursor: Cursor) -> usize {
    match cursor {
        Cursor::After(a) if a > total => 0,
        Cursor::After(a) => a,
        Cursor::Tail(n) => total.saturating_sub(n),
    }
}

pub fn socket_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".makit").join("link.sock"))
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub session_id: String,
    /// `claude` / `codex`
    pub tool: String,
    pub name: String,
    pub cwd: String,
    pub branch: String,
    /// `busy` / `waiting` / `idle` / `ended`, see [`state_of`]
    pub state: String,
    /// Only meaningful when `state == "waiting"`: `user` = waiting for your answer, anything else = waiting for approval
    pub waiting_for: String,
    pub running: bool,
    pub pid: u32,
    pub mtime: i64,
    pub last_user_msg: String,
}

pub struct TranscriptPage {
    /// How many items the conversation has now
    pub total: usize,
    /// Index of the first returned item (0 when the file was rewritten and the client must start over)
    pub from: usize,
    pub items: Vec<Value>,
}

/// Where a `transcript` response starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    /// Items after this index; an index past the end means the file was rewritten, start over
    After(usize),
    /// Only the last N items
    Tail(usize),
}

pub trait Backend: Send + Sync {
    fn sessions(&self, limit: usize) -> Result<Vec<SessionInfo>, String>;
    fn transcript(&self, session_id: &str, cursor: Cursor) -> Result<TranscriptPage, String>;
}

/// The session state the other side sees. A process that is gone is `ended` whatever the file last said.
pub fn state_of(running: bool, status: &str) -> &'static str {
    if !running {
        return "ended";
    }
    match status {
        "waiting" => "waiting",
        "busy" => "busy",
        _ => "idle",
    }
}

fn level_str(l: Level) -> &'static str {
    match l {
        Level::Info => "info",
        Level::Warn => "warn",
        Level::Error => "error",
    }
}

/// One conversation item as JSON: `{id, time, kind, ...}`.
pub fn item_json(item: &Item) -> Value {
    let mut v = match &item.kind {
        ItemKind::User(t) => json!({"kind": "user", "text": t}),
        ItemKind::Image { media_type, bytes } => json!({"kind": "image", "media_type": media_type, "bytes": bytes}),
        ItemKind::Command { name, args } => json!({"kind": "command", "name": name, "args": args}),
        ItemKind::LocalOutput(t) => json!({"kind": "local_output", "text": t}),
        ItemKind::BashInput(t) => json!({"kind": "bash_input", "text": t}),
        ItemKind::Assistant(t) => json!({"kind": "assistant", "text": t}),
        ItemKind::Thinking(t) => json!({"kind": "thinking", "text": t}),
        ItemKind::ToolCall { call_id, name, input, result } => json!({
            "kind": "tool_call", "call_id": call_id, "name": name, "input": input,
            "result": result.as_ref().map(|r| json!({
                "text": r.text, "truncated": r.truncated, "total_len": r.total_len, "is_error": r.is_error, "images": r.images,
            })),
        }),
        ItemKind::Notice { level, text } => json!({"kind": "notice", "level": level_str(*level), "text": text}),
        ItemKind::Divider(_) => json!({"kind": "divider", "what": "compacted"}),
        ItemKind::TurnDuration(ms) => json!({"kind": "turn_duration", "ms": ms}),
    };
    let obj = v.as_object_mut().expect("object");
    obj.insert("id".into(), json!(item.id));
    obj.insert("time".into(), json!(item.time));
    v
}

fn fail(msg: impl Into<String>) -> String {
    json!({"ok": false, "error": msg.into()}).to_string()
}

/// Answer one request line (no trailing newline) with one response line (no trailing newline).
pub fn handle_line(backend: &dyn Backend, line: &str) -> String {
    if line.len() > MAX_REQUEST {
        return fail("request too long");
    }
    let Ok(req) = serde_json::from_str::<Value>(line) else {
        return fail("bad json");
    };
    match req.get("op").and_then(Value::as_str) {
        Some("hello") => json!({
            "ok": true, "app": "makit", "proto": PROTO, "version": env!("CARGO_PKG_VERSION"), "write": false,
        })
        .to_string(),
        Some("sessions") => {
            let limit = req.get("limit").and_then(Value::as_u64).map_or(DEFAULT_LIMIT, |n| (n as usize).clamp(1, MAX_LIMIT));
            match backend.sessions(limit) {
                Ok(list) => json!({"ok": true, "sessions": list}).to_string(),
                Err(e) => fail(e),
            }
        }
        Some("transcript") => {
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            let cursor = match req.get("tail").and_then(Value::as_u64) {
                Some(n) => Cursor::Tail((n as usize).min(PAGE_ITEMS)),
                None => Cursor::After(req.get("after").and_then(Value::as_u64).unwrap_or(0) as usize),
            };
            match backend.transcript(id, cursor) {
                Ok(p) => json!({"ok": true, "total": p.total, "from": p.from, "items": p.items}).to_string(),
                Err(e) => fail(e),
            }
        }
        Some(other) => fail(format!("unknown op: {other}")),
        None => fail("op required"),
    }
}

/// The real backend: sessions from `sessions::list_sessions`, conversations from cached `TranscriptReader`s.
#[derive(Default)]
pub struct CoreBackend {
    /// session id -> (reader, last use tick)
    readers: Mutex<HashMap<String, (TranscriptReader, u64)>>,
    tick: Mutex<u64>,
}

impl CoreBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pick the requested items from a reader that has just been polled.
    fn page(reader: &TranscriptReader, cursor: Cursor) -> TranscriptPage {
        let total = reader.len();
        let from = cursor_start(total, cursor);
        let to = total.min(from + PAGE_ITEMS);
        TranscriptPage { total, from, items: (from..to).map(|i| item_json(reader.item(i))).collect() }
    }
}

impl Backend for CoreBackend {
    fn sessions(&self, limit: usize) -> Result<Vec<SessionInfo>, String> {
        let mut all = crate::sessions::list_sessions(None)?;
        all.retain(|s| !s.archived);
        all.sort_by(|a, b| b.mtime.cmp(&a.mtime));
        Ok(all
            .into_iter()
            .take(limit)
            .map(|s| SessionInfo {
                state: state_of(s.running, &s.status).to_string(),
                session_id: s.session_id,
                tool: s.tool,
                name: s.display_name,
                cwd: s.cwd,
                branch: s.git_branch,
                waiting_for: s.waiting_for,
                running: s.running,
                pid: s.pid,
                mtime: s.mtime,
                last_user_msg: s.last_user_msg,
            })
            .collect())
    }

    fn transcript(&self, session_id: &str, cursor: Cursor) -> Result<TranscriptPage, String> {
        let tick = {
            let mut t = self.tick.lock().expect("tick");
            *t += 1;
            *t
        };
        let mut readers = self.readers.lock().expect("readers");
        if !readers.contains_key(session_id) {
            let (path, tool): (PathBuf, Tool) = crate::sessions::locate_session_file(session_id).ok_or("session not found")?;
            if readers.len() >= MAX_READERS {
                if let Some(oldest) = readers.iter().min_by_key(|(_, (_, used))| *used).map(|(k, _)| k.clone()) {
                    readers.remove(&oldest);
                }
            }
            readers.insert(session_id.to_string(), (TranscriptReader::new(path, tool), tick));
        }
        let entry = readers.get_mut(session_id).expect("just inserted");
        entry.1 = tick;
        entry.0.poll().map_err(|e| e.to_string())?;
        Ok(Self::page(&entry.0, cursor))
    }
}

fn handle_conn(stream: UnixStream, backend: &dyn Backend) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
    let mut line = String::new();
    // Read at most MAX_REQUEST + 1 bytes: a longer line is refused instead of buffered without bound
    let n = BufReader::new((&stream).take(MAX_REQUEST as u64 + 1)).read_line(&mut line);
    let reply = match n {
        Ok(0) | Err(_) => return,
        Ok(_) => handle_line(backend, line.trim()),
    };
    let mut w = &stream;
    let _ = w.write_all(reply.as_bytes()).and_then(|_| w.write_all(b"\n"));
}

/// A running link. The link is optional in makit (off by default), so it can be switched off again at runtime:
/// [`Handle::stop`] closes the listener and removes the socket file, so nobody can connect any more.
pub struct Handle {
    path: PathBuf,
    stop: Arc<AtomicBool>,
}

impl Handle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        // The accept loop is blocked in `accept`: one throwaway connection wakes it so it sees the flag and exits
        #[cfg(unix)]
        let _ = UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start the link on `~/.makit/link.sock`. A bind failure is logged and returns `None`, never fatal: makit works without it.
pub fn start(backend: Arc<dyn Backend>) -> Option<Handle> {
    let path = socket_path()?;
    match start_at(&path, backend) {
        Ok(h) => Some(h),
        Err(e) => {
            log::error!(target: "link", "bind error: {e}");
            None
        }
    }
}

/// Testable version of [`start`]: the socket path can be injected.
#[cfg(unix)]
pub fn start_at(path: &Path, backend: Arc<dyn Backend>) -> std::io::Result<Handle> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    // A socket file left by the last run: bind fails unless it is removed
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming() {
            if flag.load(Ordering::SeqCst) {
                break;
            }
            match conn {
                Ok(stream) => {
                    let b = backend.clone();
                    std::thread::spawn(move || handle_conn(stream, &*b));
                }
                Err(e) => log::warn!(target: "link", "accept error: {e}"),
            }
        }
    });
    Ok(Handle { path: path.to_path_buf(), stop })
}

#[cfg(not(unix))]
pub fn start_at(path: &Path, _backend: Arc<dyn Backend>) -> std::io::Result<Handle> {
    Ok(Handle { path: path.to_path_buf(), stop: Arc::new(AtomicBool::new(true)) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::ToolResult;

    // Why it is tested on its own: this is the contract another program relies on. A wrong `state` shows a session as
    // "idle" while it waits for approval (the phone never tells you), a wrong `from` after a rewritten file makes the
    // other side show a conversation with a hole in it, and a request that crashes the handler takes the whole link down.

    struct Fake {
        sessions: Vec<SessionInfo>,
        total: usize,
    }

    fn info(id: &str, state: &str) -> SessionInfo {
        SessionInfo {
            session_id: id.into(),
            tool: "claude".into(),
            name: "demo".into(),
            cwd: "/Users/me/p".into(),
            branch: "main".into(),
            state: state.into(),
            waiting_for: String::new(),
            running: state != "ended",
            pid: 7,
            mtime: 100,
            last_user_msg: "hi".into(),
        }
    }

    impl Backend for Fake {
        fn sessions(&self, limit: usize) -> Result<Vec<SessionInfo>, String> {
            Ok(self.sessions.iter().take(limit).cloned().collect())
        }
        fn transcript(&self, id: &str, cursor: Cursor) -> Result<TranscriptPage, String> {
            if id != "s1" {
                return Err("session not found".into());
            }
            let from = cursor_start(self.total, cursor);
            Ok(TranscriptPage { total: self.total, from, items: (from..self.total).map(|i| json!({"id": format!("i{i}")})).collect() })
        }
    }

    fn fake() -> Fake {
        Fake { sessions: vec![info("s1", "busy"), info("s2", "waiting"), info("s3", "ended")], total: 3 }
    }

    fn ask(b: &dyn Backend, line: &str) -> Value {
        serde_json::from_str(&handle_line(b, line)).expect("response is json")
    }

    #[test]
    fn state_of_ended_wins_over_a_stale_status() {
        assert_eq!(state_of(false, "waiting"), "ended");
        assert_eq!(state_of(false, "busy"), "ended");
        assert_eq!(state_of(true, "waiting"), "waiting");
        assert_eq!(state_of(true, "busy"), "busy");
        assert_eq!(state_of(true, "idle"), "idle");
        assert_eq!(state_of(true, ""), "idle");
        assert_eq!(state_of(true, "something-new"), "idle");
    }

    #[test]
    fn hello_says_who_and_that_it_is_read_only() {
        let r = ask(&fake(), r#"{"op":"hello"}"#);
        assert_eq!(r["ok"], true);
        assert_eq!(r["app"], "makit");
        assert_eq!(r["proto"], PROTO);
        assert_eq!(r["write"], false);
        assert!(r["version"].as_str().is_some_and(|v| !v.is_empty()));
    }

    #[test]
    fn sessions_respects_limit_and_clamps_it() {
        let b = fake();
        assert_eq!(ask(&b, r#"{"op":"sessions"}"#)["sessions"].as_array().unwrap().len(), 3);
        assert_eq!(ask(&b, r#"{"op":"sessions","limit":2}"#)["sessions"].as_array().unwrap().len(), 2);
        // 0 is raised to 1, a huge value is capped (the fake only has 3 anyway)
        assert_eq!(ask(&b, r#"{"op":"sessions","limit":0}"#)["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(ask(&b, r#"{"op":"sessions","limit":999999}"#)["sessions"].as_array().unwrap().len(), 3);
        let first = &ask(&b, r#"{"op":"sessions"}"#)["sessions"][0];
        assert_eq!(first["session_id"], "s1");
        assert_eq!(first["state"], "busy");
        assert_eq!(first["cwd"], "/Users/me/p");
    }

    #[test]
    fn transcript_returns_items_after_the_cursor() {
        let b = fake();
        let r = ask(&b, r#"{"op":"transcript","session_id":"s1","after":1}"#);
        assert_eq!((r["total"].as_u64(), r["from"].as_u64()), (Some(3), Some(1)));
        assert_eq!(r["items"].as_array().unwrap().len(), 2);
        let r = ask(&b, r#"{"op":"transcript","session_id":"s1","after":3}"#);
        assert_eq!(r["items"].as_array().unwrap().len(), 0);
        assert_eq!(r["from"], 3);
    }

    #[test]
    fn transcript_cursor_past_the_end_starts_over() {
        let r = ask(&fake(), r#"{"op":"transcript","session_id":"s1","after":50}"#);
        assert_eq!(r["from"], 0);
        assert_eq!(r["items"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn tail_returns_only_the_last_items_and_never_more_than_a_page() {
        let b = Fake { sessions: vec![], total: 10 };
        let r = ask(&b, r#"{"op":"transcript","session_id":"s1","tail":4}"#);
        assert_eq!((r["total"].as_u64(), r["from"].as_u64()), (Some(10), Some(6)));
        assert_eq!(r["items"][0]["id"], "i6");
        // Asking for more than exists is the whole conversation; tail wins over after when both are given
        let r = ask(&b, r#"{"op":"transcript","session_id":"s1","tail":99,"after":9}"#);
        assert_eq!(r["from"], 0);
        // A huge tail is capped at one page: 1000 items, 300 per page -> starts at 700
        let big = Fake { sessions: vec![], total: 1000 };
        assert_eq!(ask(&big, r#"{"op":"transcript","session_id":"s1","tail":100000}"#)["from"], (1000 - PAGE_ITEMS));
        assert_eq!(cursor_start(5, Cursor::Tail(100_000)), 0);
        assert_eq!(cursor_start(5, Cursor::After(9)), 0);
    }

    #[test]
    fn bad_requests_get_an_error_line_not_a_crash() {
        let b = fake();
        for bad in ["", "not json", "[]", "{}", r#"{"op":5}"#, r#"{"op":"nope"}"#, r#"{"op":"transcript"}"#, r#"{"op":"transcript","session_id":""}"#] {
            let r = ask(&b, bad);
            assert_eq!(r["ok"], false, "{bad}");
            assert!(r["error"].as_str().is_some_and(|e| !e.is_empty()), "{bad}");
        }
        assert_eq!(ask(&b, r#"{"op":"transcript","session_id":"nope"}"#)["error"], "session not found");
        let long = format!(r#"{{"op":"hello","pad":"{}"}}"#, "x".repeat(MAX_REQUEST));
        assert_eq!(ask(&b, &long)["error"], "request too long");
    }

    #[test]
    fn item_json_covers_every_kind() {
        let it = |kind| Item { id: "u1".into(), time: Some("t".into()), kind };
        let v = item_json(&it(ItemKind::User("hello".into())));
        assert_eq!((v["kind"].as_str(), v["text"].as_str(), v["id"].as_str(), v["time"].as_str()), (Some("user"), Some("hello"), Some("u1"), Some("t")));
        assert_eq!(item_json(&it(ItemKind::Assistant("a".into())))["kind"], "assistant");
        assert_eq!(item_json(&it(ItemKind::Thinking("t".into())))["kind"], "thinking");
        assert_eq!(item_json(&it(ItemKind::LocalOutput("o".into())))["kind"], "local_output");
        assert_eq!(item_json(&it(ItemKind::BashInput("ls".into())))["kind"], "bash_input");
        assert_eq!(item_json(&it(ItemKind::Command { name: "/x".into(), args: "y".into() }))["name"], "/x");
        assert_eq!(item_json(&it(ItemKind::Image { media_type: "image/png".into(), bytes: 9 }))["bytes"], 9);
        assert_eq!(item_json(&it(ItemKind::Notice { level: Level::Warn, text: "w".into() }))["level"], "warn");
        assert_eq!(item_json(&it(ItemKind::Divider(crate::transcript::DividerKind::Compacted)))["kind"], "divider");
        assert_eq!(item_json(&it(ItemKind::TurnDuration(1500)))["ms"], 1500);
        let call = item_json(&it(ItemKind::ToolCall {
            call_id: "c1".into(),
            name: "Bash".into(),
            input: json!({"command": "ls"}),
            result: Some(ToolResult::new("out", true, 0)),
        }));
        assert_eq!((call["name"].as_str(), call["input"]["command"].as_str()), (Some("Bash"), Some("ls")));
        assert_eq!((call["result"]["text"].as_str(), call["result"]["is_error"].as_bool()), (Some("out"), Some(true)));
        // A call whose result has not arrived yet
        let pending = item_json(&it(ItemKind::ToolCall { call_id: "c2".into(), name: "Read".into(), input: json!({}), result: None }));
        assert!(pending["result"].is_null());
    }

    #[cfg(unix)]
    fn tmp_sock(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("makit-link-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("link.sock")
    }

    #[cfg(unix)]
    fn roundtrip(path: &Path, req: &str) -> Value {
        let mut s = UnixStream::connect(path).expect("connect");
        s.write_all(req.as_bytes()).unwrap();
        s.write_all(b"\n").unwrap();
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line).unwrap();
        serde_json::from_str(line.trim()).expect("one json line")
    }

    #[cfg(unix)]
    #[test]
    fn socket_roundtrip_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = tmp_sock("rt");
        start_at(&path, Arc::new(fake())).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600, "socket must be private");
        assert_eq!(mode(path.parent().unwrap()), 0o700, "directory must be private");
        assert_eq!(roundtrip(&path, r#"{"op":"hello"}"#)["app"], "makit");
        assert_eq!(roundtrip(&path, r#"{"op":"sessions","limit":1}"#)["sessions"][0]["session_id"], "s1");
        // Several connections in a row, and a bad one in between, still work
        assert_eq!(roundtrip(&path, "garbage")["ok"], false);
        assert_eq!(roundtrip(&path, r#"{"op":"transcript","session_id":"s1","after":2}"#)["items"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    // Why it matters: the link is an opt-in switch in settings. If "off" did not really close the socket, the user would
    // believe nothing can read their sessions while the socket is still answering.
    #[cfg(unix)]
    #[test]
    fn stop_closes_the_socket_and_it_can_be_started_again() {
        let path = tmp_sock("stop");
        let h = start_at(&path, Arc::new(fake())).unwrap();
        assert_eq!(roundtrip(&path, r#"{"op":"hello"}"#)["ok"], true);
        h.stop();
        assert!(!path.exists(), "socket file must be gone");
        assert!(UnixStream::connect(&path).is_err(), "nobody may connect after stop");
        // Switched on again: works as before
        let h2 = start_at(&path, Arc::new(fake())).unwrap();
        assert_eq!(roundtrip(&path, r#"{"op":"hello"}"#)["ok"], true);
        h2.stop();
        // Stopping twice is harmless
        h2.stop();
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn restart_replaces_a_stale_socket_file() {
        let path = tmp_sock("stale");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"left over").unwrap();
        start_at(&path, Arc::new(fake())).unwrap();
        assert_eq!(roundtrip(&path, r#"{"op":"hello"}"#)["ok"], true);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
