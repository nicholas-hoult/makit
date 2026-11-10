//! Local session link (#299): lets another program on this machine list makit's sessions and read their conversations.
//!
//! Protocol v1: unix socket `~/.makit/link.sock` (directory 0700, socket 0600), one JSON line in, one JSON line out, then the
//! connection closes. Stateless, so polling is enough and a restart of either side needs no reconnect logic.
//! `transcript` takes `after` (items after that index) or `tail` (only the last N, for a first look at a long conversation).
//! Ops: `hello` / `sessions` / `transcript` / `screen` (read-only) and `input` (reply text, approve, deny, interrupt),
//! `keys` (whitelisted keys), `open` (new session of a known tool in a directory), `resume` (an ended session). The
//! writing ops are a separate opt-in in makit's settings and refused while it is off, see [`Backend::write_enabled`].
//! `subscribe` is the one op that keeps its connection open: it pushes the ids of sessions that changed.
//!
//! Layout: [`handle_line`] is a pure function over a [`Backend`] (tests use a fake one); [`CoreBackend`] is the real
//! backend on top of `sessions` / `transcript`; [`start`] / [`start_at`] put it on a socket.
//! No UI framework and no async runtime, same as `hook`.

use crate::transcript::{Item, ItemKind, Level, Tool, TranscriptReader};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
#[cfg(unix)]
use std::io::{BufRead, BufReader, Read};
use std::io::Write;
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
    /// Whether the user allowed writing into sessions (a switch of its own, off by default).
    fn write_enabled(&self) -> bool {
        false
    }
    /// The live `(state, waiting_for)` of a running session, `None` if there is no such session.
    fn session_state(&self, _session_id: &str) -> Option<(String, String)> {
        None
    }
    /// Put bytes into the terminal of the session. Only called after [`plan_input`] accepted the request.
    fn write(&self, _session_id: &str, _bytes: Vec<u8>) -> Result<(), String> {
        Err("write not supported".into())
    }
    /// Snapshot of the terminal the session runs in (only sessions open in a makit tab have one).
    fn screen(&self, _session_id: &str) -> Result<Screen, String> {
        Err("screen not supported".into())
    }
    /// Start `tool` in a new tab in `cwd`. Only called after [`plan_open`] accepted it and the directory exists.
    fn open(&self, _cwd: &str, _tool: &str) -> Result<(), String> {
        Err("open not supported".into())
    }
    /// Resume an ended session in a tab, in its own directory.
    fn resume(&self, _session_id: &str) -> Result<(), String> {
        Err("resume not supported".into())
    }
}

/// What the other side wants to put into a session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputKind {
    /// Reply text, submitted with Enter
    Text(String),
    /// Accept the permission prompt that is showing (Enter: its default choice is "Yes")
    Approve,
    /// Refuse it (Esc)
    Deny,
    /// Stop the turn that is running (Esc), like ACP `session/cancel`
    Interrupt,
    /// Type text without Enter, in any live state: the terminal view's keyboard, where the user sees the screen
    /// (slash commands with a menu, answers to a tool's own dialog)
    Type(String),
}

/// What `hello` lists, so the other side can tell an older makit (fewer ops) from this one.
/// "type" is not an op of its own: it says `input` takes kind `type` and replies while the session works.
pub const OPS: &[&str] = &["hello", "sessions", "transcript", "input", "screen", "keys", "open", "resume", "subscribe", "type"];

/// Most keys one `keys` request may carry.
pub const MAX_KEYS: usize = 16;

/// Tools `open` may start. Never a command line from the request.
pub const KNOWN_TOOLS: &[&str] = &["claude", "codex"];

/// Bytes for one whitelisted key name; `None` for anything else.
pub fn key_bytes(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "enter" => b"\r",
        "esc" => b"\x1b",
        "tab" => b"\t",
        "shift_tab" => b"\x1b[Z",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "backspace" => b"\x7f",
        "space" => b" ",
        "ctrl_c" => b"\x03",
        "1" => b"1",
        "2" => b"2",
        "3" => b"3",
        "4" => b"4",
        "5" => b"5",
        "6" => b"6",
        "7" => b"7",
        "8" => b"8",
        "9" => b"9",
        _ => return None,
    })
}

/// The bytes for a `keys` request: a running session, a short list, every key whitelisted (else nothing is sent).
/// Why a whitelist and not "any text": this is the escape hatch of the terminal view, for menus and dialogs the
/// conversation view does not understand; free text goes through `input` with its own checks.
pub fn plan_keys(state: &str, keys: &[String]) -> Result<Vec<u8>, String> {
    if state == "ended" {
        return Err("session ended".into());
    }
    if keys.is_empty() {
        return Err("no keys".into());
    }
    if keys.len() > MAX_KEYS {
        return Err("too many keys".into());
    }
    let mut out = Vec::new();
    for k in keys {
        out.extend_from_slice(key_bytes(k).ok_or_else(|| format!("unknown key: {k}"))?);
    }
    Ok(out)
}

/// Text as terminal bytes: length and control-character checks, several lines as one bracketed paste (otherwise
/// every newline would submit on its own). No Enter: the caller adds it.
fn text_bytes(text: &str) -> Result<Vec<u8>, String> {
    let text = text.replace("\r\n", "\n");
    if text.chars().count() > MAX_INPUT_CHARS {
        return Err("text too long".into());
    }
    if text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err("control characters".into());
    }
    if !text.contains('\n') {
        return Ok(text.into_bytes());
    }
    let mut out = b"\x1b[200~".to_vec();
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    Ok(out)
}

/// Check an `open` request before anything is started. The directory's existence is checked by the caller.
pub fn plan_open(tool: &str, cwd: &str) -> Result<(), String> {
    if !KNOWN_TOOLS.contains(&tool) {
        return Err("unknown tool".into());
    }
    if cwd.chars().any(char::is_control) {
        return Err("control characters".into());
    }
    if !cwd.starts_with('/') {
        return Err("cwd must be absolute".into());
    }
    Ok(())
}

/// Ids of sessions that are new, changed (state, waiting_for, content time, name, running) or gone, in `next` order
/// then the gone ones. Drives `subscribe`.
pub fn changed_sessions(prev: &[SessionInfo], next: &[SessionInfo]) -> Vec<String> {
    let old: HashMap<&str, &SessionInfo> = prev.iter().map(|s| (s.session_id.as_str(), s)).collect();
    let mut out: Vec<String> = next
        .iter()
        .filter(|n| match old.get(n.session_id.as_str()) {
            None => true,
            Some(o) => o.state != n.state || o.waiting_for != n.waiting_for || o.mtime != n.mtime || o.name != n.name || o.running != n.running,
        })
        .map(|n| n.session_id.clone())
        .collect();
    let now: std::collections::HashSet<&str> = next.iter().map(|s| s.session_id.as_str()).collect();
    out.extend(prev.iter().filter(|p| !now.contains(p.session_id.as_str())).map(|p| p.session_id.clone()));
    out
}

/// A terminal colour as the screen snapshot carries it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScreenColor {
    #[default]
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// One cell of the terminal grid, independent of the terminal library (makit's native side fills it from alacritty).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScreenCell {
    pub ch: char,
    pub fg: ScreenColor,
    pub bg: ScreenColor,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Default for ScreenCell {
    fn default() -> Self {
        Self { ch: ' ', fg: ScreenColor::Default, bg: ScreenColor::Default, bold: false, dim: false, italic: false, underline: false, inverse: false }
    }
}

impl ScreenCell {
    fn style(&self) -> (ScreenColor, ScreenColor, bool, bool, bool, bool, bool) {
        (self.fg, self.bg, self.bold, self.dim, self.italic, self.underline, self.inverse)
    }

    fn plain(&self) -> bool {
        self.style() == ScreenCell::default().style()
    }

    fn sgr(&self) -> String {
        let mut codes = vec!["0".to_string()];
        for (on, code) in [(self.bold, "1"), (self.dim, "2"), (self.italic, "3"), (self.underline, "4"), (self.inverse, "7")] {
            if on {
                codes.push(code.into());
            }
        }
        for (c, base) in [(self.fg, 38), (self.bg, 48)] {
            match c {
                ScreenColor::Default => {}
                ScreenColor::Indexed(i) => codes.push(format!("{base};5;{i}")),
                ScreenColor::Rgb(r, g, b) => codes.push(format!("{base};2;{r};{g};{b}")),
            }
        }
        format!("\x1b[{}m", codes.join(";"))
    }
}

/// One screen row as text with SGR codes: a code only where the style changes, a reset after the last styled run,
/// plain trailing blanks dropped. The other side writes the rows into a terminal emulator (xterm) as they are.
pub fn render_ansi_line(cells: &[ScreenCell]) -> String {
    let end = cells.iter().rposition(|c| !(c.plain() && (c.ch == ' ' || c.ch == '\0'))).map_or(0, |i| i + 1);
    let mut out = String::new();
    let mut styled = false;
    let mut last: Option<(ScreenColor, ScreenColor, bool, bool, bool, bool, bool)> = None;
    for c in &cells[..end] {
        if last != Some(c.style()) {
            if c.plain() {
                if styled {
                    out.push_str("\x1b[0m");
                }
                styled = false;
            } else {
                out.push_str(&c.sgr());
                styled = true;
            }
            last = Some(c.style());
        }
        out.push(if c.ch == '\0' { ' ' } else { c.ch });
    }
    if styled {
        out.push_str("\x1b[0m");
    }
    out
}

/// A snapshot of the terminal a session runs in.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub cursor_x: usize,
    pub cursor_y: usize,
    /// One string per row, see [`render_ansi_line`]
    pub lines: Vec<String>,
}

/// Longest reply text accepted, in characters.
pub const MAX_INPUT_CHARS: usize = 4000;

/// Decide whether `kind` may be written to a session in `state` / `waiting_for`, and which bytes that is.
/// Errors are short fixed messages the other side may show as they are.
///
/// Why this is a pure function of its own: it is the safety gate for typing into someone's terminal. Replying to a
/// session that is busy garbles its work, Esc sent to a session that is not at a prompt interrupts a running turn, and
/// a stray control character in the text could drive the terminal itself instead of the program in it.
pub fn plan_input(state: &str, waiting_for: &str, kind: &InputKind) -> Result<Vec<u8>, String> {
    if state == "ended" {
        return Err("session ended".into());
    }
    let at_approval = state == "waiting" && waiting_for != "user";
    match kind {
        // Esc while a turn runs stops it; at a prompt it would do nothing useful, in a dialog it would answer it
        InputKind::Interrupt if state == "busy" => Ok(vec![0x1b]),
        InputKind::Interrupt => Err("not busy".into()),
        InputKind::Approve if at_approval => Ok(b"\r".to_vec()),
        InputKind::Deny if at_approval => Ok(vec![0x1b]),
        InputKind::Approve | InputKind::Deny => Err("not waiting for approval".into()),
        InputKind::Text(text) => {
            // A reply goes to a session at its prompt (idle), waiting for the user's answer, or working (the tool takes
            // it into the running turn, as when typing in makit); not while it asks for approval (typing there would
            // answer the dialog)
            if at_approval {
                return Err("waiting for approval".into());
            }
            if text.trim().is_empty() {
                return Err("empty text".into());
            }
            let mut out = text_bytes(text)?;
            out.push(b'\r');
            Ok(out)
        }
        InputKind::Type(text) => {
            if text.is_empty() {
                return Err("empty text".into());
            }
            text_bytes(text)
        }
    }
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
            "ok": true, "app": "makit", "proto": PROTO, "version": env!("CARGO_PKG_VERSION"), "write": backend.write_enabled(), "ops": OPS,
        })
        .to_string(),
        Some("screen") => {
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            match backend.screen(id) {
                Ok(s) => {
                    let mut v = json!(s);
                    v.as_object_mut().expect("object").insert("ok".into(), json!(true));
                    v.to_string()
                }
                Err(e) => fail(e),
            }
        }
        Some("keys") => {
            if !backend.write_enabled() {
                return fail("write disabled");
            }
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            let Some(keys) = req.get("keys").and_then(Value::as_array) else {
                return fail("keys must be a list");
            };
            let keys: Vec<String> = keys.iter().map(|k| k.as_str().unwrap_or("").to_string()).collect();
            let Some((state, _)) = backend.session_state(id) else {
                return fail("session not found");
            };
            match plan_keys(&state, &keys).and_then(|bytes| backend.write(id, bytes)) {
                Ok(()) => json!({"ok": true}).to_string(),
                Err(e) => fail(e),
            }
        }
        Some("open") => {
            if !backend.write_enabled() {
                return fail("write disabled");
            }
            let tool = req.get("tool").and_then(Value::as_str).unwrap_or_default();
            let cwd = req.get("cwd").and_then(Value::as_str).unwrap_or_default();
            if let Err(e) = plan_open(tool, cwd) {
                return fail(e);
            }
            if !Path::new(cwd).is_dir() {
                return fail("no such directory");
            }
            // The new session's id is only known once the tool writes its first record: the other side picks the
            // newest session of this tool in this directory that started at or after `started_at`
            let started_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
            match backend.open(cwd, tool) {
                Ok(()) => json!({"ok": true, "started_at": started_at}).to_string(),
                Err(e) => fail(e),
            }
        }
        Some("resume") => {
            if !backend.write_enabled() {
                return fail("write disabled");
            }
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            if backend.session_state(id).is_some() {
                return fail("session running");
            }
            match backend.sessions(MAX_LIMIT) {
                Ok(list) if list.iter().any(|s| s.session_id == id) => {}
                Ok(_) => return fail("session not found"),
                Err(e) => return fail(e),
            }
            match backend.resume(id) {
                Ok(()) => json!({"ok": true}).to_string(),
                Err(e) => fail(e),
            }
        }
        Some("subscribe") => fail("subscribe keeps the connection open, send it as the only request on a connection"),
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
        Some("input") => {
            if !backend.write_enabled() {
                return fail("write disabled");
            }
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            let kind = match req.get("kind").and_then(Value::as_str) {
                Some("text") => InputKind::Text(req.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
                Some("approve") => InputKind::Approve,
                Some("deny") => InputKind::Deny,
                Some("interrupt") => InputKind::Interrupt,
                Some("type") => InputKind::Type(req.get("text").and_then(Value::as_str).unwrap_or_default().to_string()),
                _ => return fail("kind must be text, approve, deny, interrupt or type"),
            };
            // Checked against the live state right before writing, not against what the other side saw earlier
            let Some((state, waiting_for)) = backend.session_state(id) else {
                return fail("session not found");
            };
            match plan_input(&state, &waiting_for, &kind).and_then(|bytes| backend.write(id, bytes)) {
                Ok(()) => json!({"ok": true}).to_string(),
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

    fn session_state(&self, session_id: &str) -> Option<(String, String)> {
        // `load_running_info` only lists sessions whose process is alive
        let info = crate::running::load_running_info();
        let r = info.get(session_id)?;
        Some((state_of(true, &r.status).to_string(), r.waiting_for.clone()))
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

/// How often `subscribe` looks for changes, and every how many looks it sends a ping (so a dead peer is noticed).
#[cfg(unix)]
const SUBSCRIBE_TICK: std::time::Duration = std::time::Duration::from_secs(1);
#[cfg(unix)]
const SUBSCRIBE_PING_EVERY: u32 = 15;

/// The `subscribe` connection: `{"event":"ready"}`, then `{"event":"changed","sessions":[ids]}` whenever sessions
/// change (state, content, new, gone) and `{"event":"ping"}` now and then, until the reader goes away or `stop` is set.
/// The other side re-reads what changed with the normal ops; events carry ids only, never content.
/// `on_tick` runs before every look (tests use it to change the backend; the real loop passes a no-op).
pub fn subscribe_loop(
    backend: &dyn Backend,
    out: &mut dyn Write,
    stop: &AtomicBool,
    tick: std::time::Duration,
    ping_every: u32,
    mut on_tick: impl FnMut(u32),
) {
    let send = |out: &mut dyn Write, v: Value| out.write_all(format!("{v}\n").as_bytes()).and_then(|_| out.flush());
    if send(out, json!({"event": "ready"})).is_err() {
        return;
    }
    let mut prev = backend.sessions(DEFAULT_LIMIT).unwrap_or_default();
    let mut n: u32 = 0;
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(tick);
        n += 1;
        on_tick(n);
        let Ok(next) = backend.sessions(DEFAULT_LIMIT) else { continue };
        let changed = changed_sessions(&prev, &next);
        if !changed.is_empty() && send(out, json!({"event": "changed", "sessions": changed})).is_err() {
            return;
        }
        prev = next;
        if ping_every > 0 && n % ping_every == 0 && send(out, json!({"event": "ping"})).is_err() {
            return;
        }
    }
}

// The socket is a unix socket: on other systems the link does not start (see the `not(unix)` `start_at`)
#[cfg(unix)]
fn handle_conn(stream: UnixStream, backend: &dyn Backend, stop: &AtomicBool) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(5)));
    let mut line = String::new();
    // Read at most MAX_REQUEST + 1 bytes: a longer line is refused instead of buffered without bound
    let n = BufReader::new((&stream).take(MAX_REQUEST as u64 + 1)).read_line(&mut line);
    let reply = match n {
        Ok(0) | Err(_) => return,
        Ok(_) if serde_json::from_str::<Value>(line.trim()).ok().and_then(|v| v.get("op").and_then(Value::as_str).map(str::to_string)).as_deref() == Some("subscribe") => {
            let mut w = &stream;
            subscribe_loop(backend, &mut w, stop, SUBSCRIBE_TICK, SUBSCRIBE_PING_EVERY, |_| {});
            return;
        }
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
                    let f = flag.clone();
                    std::thread::spawn(move || handle_conn(stream, &*b, &f));
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
        write_on: bool,
        /// What `write` received: (session id, bytes)
        written: Mutex<Vec<(String, Vec<u8>)>>,
        /// What `open` received: (cwd, tool)
        opened: Mutex<Vec<(String, String)>>,
        resumed: Mutex<Vec<String>>,
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
        fn write_enabled(&self) -> bool {
            self.write_on
        }
        fn session_state(&self, id: &str) -> Option<(String, String)> {
            self.sessions.iter().find(|s| s.session_id == id && s.running).map(|s| (s.state.clone(), s.waiting_for.clone()))
        }
        fn write(&self, id: &str, bytes: Vec<u8>) -> Result<(), String> {
            self.written.lock().unwrap().push((id.to_string(), bytes));
            Ok(())
        }
        fn open(&self, cwd: &str, tool: &str) -> Result<(), String> {
            self.opened.lock().unwrap().push((cwd.to_string(), tool.to_string()));
            Ok(())
        }
        fn resume(&self, id: &str) -> Result<(), String> {
            self.resumed.lock().unwrap().push(id.to_string());
            Ok(())
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
        Fake { sessions: vec![info("s1", "busy"), info("s2", "waiting"), info("s3", "ended")], total: 3, write_on: false, written: Mutex::new(Vec::new()), opened: Mutex::new(Vec::new()), resumed: Mutex::new(Vec::new()) }
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
        let b = Fake { sessions: vec![], total: 10, ..fake() };
        let r = ask(&b, r#"{"op":"transcript","session_id":"s1","tail":4}"#);
        assert_eq!((r["total"].as_u64(), r["from"].as_u64()), (Some(10), Some(6)));
        assert_eq!(r["items"][0]["id"], "i6");
        // Asking for more than exists is the whole conversation; tail wins over after when both are given
        let r = ask(&b, r#"{"op":"transcript","session_id":"s1","tail":99,"after":9}"#);
        assert_eq!(r["from"], 0);
        // A huge tail is capped at one page: 1000 items, 300 per page -> starts at 700
        let big = Fake { sessions: vec![], total: 1000, ..fake() };
        assert_eq!(ask(&big, r#"{"op":"transcript","session_id":"s1","tail":100000}"#)["from"], (1000 - PAGE_ITEMS));
        assert_eq!(cursor_start(5, Cursor::Tail(100_000)), 0);
        assert_eq!(cursor_start(5, Cursor::After(9)), 0);
    }

    fn text(s: &str) -> InputKind {
        InputKind::Text(s.to_string())
    }

    #[test]
    fn plan_text_goes_to_a_session_at_its_prompt_with_enter() {
        assert_eq!(plan_input("idle", "", &text("继续")).unwrap(), "继续\r".as_bytes());
        // Waiting for the user's answer is also a prompt
        assert_eq!(plan_input("waiting", "user", &text("好")).unwrap(), "好\r".as_bytes());
        // While it works the tool takes the message into the running turn (claude queues what is typed meanwhile), like
        // typing in makit itself (replying while the agent works, as when typing in makit)
        assert_eq!(plan_input("busy", "", &text("顺便看下日志")).unwrap(), "顺便看下日志\r".as_bytes());
        // Several lines: one bracketed paste, then Enter
        assert_eq!(plan_input("idle", "", &text("a\r\nb")).unwrap(), b"\x1b[200~a\nb\x1b[201~\r".to_vec());
    }

    #[test]
    fn plan_text_is_refused_when_it_would_disturb_the_session() {
        assert_eq!(plan_input("ended", "", &text("hi")).unwrap_err(), "session ended");
        // Typing into an approval dialog would answer it: refuse
        assert_eq!(plan_input("waiting", "approval", &text("hi")).unwrap_err(), "waiting for approval");
        assert_eq!(plan_input("waiting", "", &text("hi")).unwrap_err(), "waiting for approval");
    }

    #[test]
    fn plan_text_rejects_empty_long_and_control_characters() {
        assert_eq!(plan_input("idle", "", &text("  \n ")).unwrap_err(), "empty text");
        assert_eq!(plan_input("idle", "", &text(&"x".repeat(MAX_INPUT_CHARS + 1))).unwrap_err(), "text too long");
        assert!(plan_input("idle", "", &text(&"x".repeat(MAX_INPUT_CHARS))).is_ok());
        // Escape sequences and other control bytes could drive the terminal itself
        for bad in ["a\x1bb", "a\x03", "a\x00b", "a\rb", "\x1b[200~"] {
            assert_eq!(plan_input("idle", "", &text(bad)).unwrap_err(), "control characters", "{bad:?}");
        }
        // A tab inside the text is fine
        assert!(plan_input("idle", "", &text("a\tb")).is_ok());
    }

    fn typed(s: &str) -> InputKind {
        InputKind::Type(s.to_string())
    }

    #[test]
    fn plan_type_writes_text_without_enter_in_any_live_state() {
        // The terminal view's keyboard: the user sees the screen and decides, so any live state is fine
        for (state, waiting_for) in [("idle", ""), ("busy", ""), ("waiting", "approval"), ("waiting", "user")] {
            assert_eq!(plan_input(state, waiting_for, &typed("/compact")).unwrap(), b"/compact".to_vec(), "{state}");
        }
        // Several lines go in as one paste, still without Enter
        assert_eq!(plan_input("idle", "", &typed("a\nb")).unwrap(), b"\x1b[200~a\nb\x1b[201~".to_vec());
        assert_eq!(plan_input("ended", "", &typed("x")).unwrap_err(), "session ended");
    }

    #[test]
    fn plan_type_has_the_same_text_checks_as_a_reply() {
        assert_eq!(plan_input("busy", "", &typed("")).unwrap_err(), "empty text");
        assert_eq!(plan_input("busy", "", &typed(&"x".repeat(MAX_INPUT_CHARS + 1))).unwrap_err(), "text too long");
        for bad in ["a\x1bb", "a\x03", "a\rb"] {
            assert_eq!(plan_input("busy", "", &typed(bad)).unwrap_err(), "control characters", "{bad:?}");
        }
        // A lone space is a valid thing to type (it toggles options in some menus)
        assert_eq!(plan_input("busy", "", &typed(" ")).unwrap(), b" ".to_vec());
    }

    #[test]
    fn type_request_is_parsed() {
        let b = Fake { write_on: true, ..fake() };
        assert_eq!(ask(&b, r#"{"op":"input","session_id":"s1","kind":"type","text":"/model"}"#)["ok"], true);
        assert_eq!(b.written.lock().unwrap().last().unwrap().1, b"/model".to_vec());
    }

    #[test]
    fn plan_approve_and_deny_only_at_an_approval_prompt() {
        assert_eq!(plan_input("waiting", "approval", &InputKind::Approve).unwrap(), b"\r".to_vec());
        assert_eq!(plan_input("waiting", "", &InputKind::Approve).unwrap(), b"\r".to_vec());
        assert_eq!(plan_input("waiting", "approval", &InputKind::Deny).unwrap(), vec![0x1b]);
        // Esc at any other time would interrupt a running turn; Enter would submit an empty message
        for (state, waiting_for) in [("idle", ""), ("busy", ""), ("waiting", "user")] {
            assert_eq!(plan_input(state, waiting_for, &InputKind::Approve).unwrap_err(), "not waiting for approval", "{state}");
            assert_eq!(plan_input(state, waiting_for, &InputKind::Deny).unwrap_err(), "not waiting for approval", "{state}");
        }
        assert_eq!(plan_input("ended", "", &InputKind::Deny).unwrap_err(), "session ended");
    }

    #[test]
    fn input_is_refused_while_write_is_off_and_nothing_is_written() {
        let b = fake();
        let r = ask(&b, r#"{"op":"input","session_id":"s2","kind":"approve"}"#);
        assert_eq!((r["ok"].clone(), r["error"].clone()), (json!(false), json!("write disabled")));
        assert!(b.written.lock().unwrap().is_empty());
        assert_eq!(ask(&b, r#"{"op":"hello"}"#)["write"], false);
    }

    #[test]
    fn input_checks_the_live_state_then_writes_the_planned_bytes() {
        let b = Fake { write_on: true, ..fake() };
        assert_eq!(ask(&b, r#"{"op":"hello"}"#)["write"], true);
        // s2 is waiting for approval: approve writes Enter into s2 and nowhere else
        let r = ask(&b, r#"{"op":"input","session_id":"s2","kind":"approve"}"#);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(*b.written.lock().unwrap(), vec![("s2".to_string(), b"\r".to_vec())]);
        // A reply to s2 while it waits for approval is refused and nothing more is written
        let r = ask(&b, r#"{"op":"input","session_id":"s2","kind":"text","text":"hi"}"#);
        assert_eq!(r["error"], "waiting for approval");
        // s3 has ended (not running): the same as no such session
        assert_eq!(ask(&b, r#"{"op":"input","session_id":"s3","kind":"deny"}"#)["error"], "session not found");
        assert_eq!(ask(&b, r#"{"op":"input","session_id":"nope","kind":"deny"}"#)["error"], "session not found");
        assert_eq!(b.written.lock().unwrap().len(), 1);
    }

    #[test]
    fn input_rejects_malformed_requests() {
        let b = Fake { write_on: true, ..fake() };
        for bad in [r#"{"op":"input","kind":"approve"}"#, r#"{"op":"input","session_id":"","kind":"approve"}"#, r#"{"op":"input","session_id":"s2"}"#, r#"{"op":"input","session_id":"s2","kind":"rm -rf"}"#] {
            assert_eq!(ask(&b, bad)["ok"], false, "{bad}");
        }
        assert!(b.written.lock().unwrap().is_empty());
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

    // ---------- external-agent additions (#299 phase 3): interrupt, keys, screen, open, resume, subscribe ----------
    // Why they are tested on their own: they are what lets another program drive a terminal it cannot see. A key outside
    // the whitelist could type anything into the user's shell, an interrupt sent at the wrong time throws away a reply,
    // and a missed change in `subscribe` leaves the phone showing "working" for a session that waits for approval.

    #[test]
    fn plan_interrupt_only_while_a_turn_runs() {
        assert_eq!(plan_input("busy", "", &InputKind::Interrupt).unwrap(), vec![0x1b]);
        for (state, waiting_for) in [("idle", ""), ("waiting", "user"), ("waiting", "approval")] {
            assert_eq!(plan_input(state, waiting_for, &InputKind::Interrupt).unwrap_err(), "not busy", "{state}");
        }
        assert_eq!(plan_input("ended", "", &InputKind::Interrupt).unwrap_err(), "session ended");
    }

    #[test]
    fn keys_are_a_short_whitelist() {
        let k = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(plan_keys("waiting", &k(&["down", "enter"])).unwrap(), b"\x1b[B\r".to_vec());
        assert_eq!(plan_keys("busy", &k(&["esc"])).unwrap(), vec![0x1b]);
        assert_eq!(plan_keys("idle", &k(&["ctrl_c", "tab", "shift_tab", "up", "left", "right", "backspace", "space"])).unwrap(), b"\x03\t\x1b[Z\x1b[A\x1b[D\x1b[C\x7f ".to_vec());
        // Menu choices in permission dialogs are digits
        assert_eq!(plan_keys("waiting", &k(&["2"])).unwrap(), b"2".to_vec());
        assert_eq!(plan_keys("ended", &k(&["enter"])).unwrap_err(), "session ended");
        assert_eq!(plan_keys("idle", &k(&[])).unwrap_err(), "no keys");
        assert_eq!(plan_keys("idle", &k(&["enter"; MAX_KEYS + 1])).unwrap_err(), "too many keys");
        // Anything else is refused as a whole, nothing is half-sent
        for bad in ["a", "0", "10", "rm", "\x1b", "ctrl_d", "Enter", ""] {
            assert_eq!(plan_keys("idle", &k(&["enter", bad])).unwrap_err(), format!("unknown key: {bad}"), "{bad:?}");
        }
    }

    #[test]
    fn open_takes_a_known_tool_and_an_absolute_directory() {
        assert_eq!(plan_open("claude", "/Users/me/p"), Ok(()));
        assert_eq!(plan_open("codex", "/"), Ok(()));
        assert_eq!(plan_open("bash", "/Users/me/p").unwrap_err(), "unknown tool");
        assert_eq!(plan_open("claude; rm -rf ~", "/Users/me/p").unwrap_err(), "unknown tool");
        assert_eq!(plan_open("claude", "relative/dir").unwrap_err(), "cwd must be absolute");
        assert_eq!(plan_open("claude", "").unwrap_err(), "cwd must be absolute");
        assert_eq!(plan_open("claude", "/a\nb").unwrap_err(), "control characters");
    }

    #[test]
    fn changed_sessions_reports_new_changed_and_gone_ones() {
        let a = info("a", "busy");
        let b = info("b", "idle");
        assert!(changed_sessions(&[a.clone(), b.clone()], &[a.clone(), b.clone()]).is_empty());
        // New, changed state, new content (mtime), gone
        let c = info("c", "idle");
        let b2 = SessionInfo { state: "waiting".into(), waiting_for: "approval".into(), ..b.clone() };
        assert_eq!(changed_sessions(&[a.clone(), b.clone()], &[a.clone(), b2.clone(), c.clone()]), vec!["b", "c"]);
        let a2 = SessionInfo { mtime: 101, ..a.clone() };
        assert_eq!(changed_sessions(&[a.clone()], &[a2]), vec!["a"]);
        assert_eq!(changed_sessions(&[a.clone(), b.clone()], &[b.clone()]), vec!["a"]);
    }

    fn cell(ch: char) -> ScreenCell {
        ScreenCell { ch, ..ScreenCell::default() }
    }

    #[test]
    fn screen_lines_become_ansi_with_as_few_codes_as_possible() {
        // Plain text: no codes, trailing blanks trimmed
        assert_eq!(render_ansi_line(&[cell('h'), cell('i'), cell(' '), cell(' ')]), "hi");
        assert_eq!(render_ansi_line(&[]), "");
        // A styled run opens once and is reset at its end
        let red = |ch| ScreenCell { ch, fg: ScreenColor::Indexed(1), bold: true, ..ScreenCell::default() };
        assert_eq!(render_ansi_line(&[red('o'), red('k'), cell('!')]), "\x1b[0;1;38;5;1mok\x1b[0m!");
        // Truecolor background, inverse; a styled blank at the end is kept (it is visible)
        let bg = ScreenCell { ch: ' ', bg: ScreenColor::Rgb(1, 2, 3), inverse: true, ..ScreenCell::default() };
        assert_eq!(render_ansi_line(&[cell('x'), bg.clone()]), "x\x1b[0;7;48;2;1;2;3m \x1b[0m");
        // dim / italic / underline
        let s = ScreenCell { ch: 'y', dim: true, italic: true, underline: true, ..ScreenCell::default() };
        assert_eq!(render_ansi_line(&[s]), "\x1b[0;2;3;4my\x1b[0m");
        // Wide characters are one cell in the input (the spacer is dropped by the caller)
        assert_eq!(render_ansi_line(&[cell('中'), cell('文')]), "中文");
    }

    #[test]
    fn screen_and_writes_go_through_the_switches() {
        let b = fake();
        // screen is read-only: allowed while write is off; the fake has no terminal
        assert_eq!(ask(&b, r#"{"op":"screen","session_id":"s1"}"#)["error"], "screen not supported");
        assert_eq!(ask(&b, r#"{"op":"screen"}"#)["error"], "session_id required");
        // keys / open / resume / interrupt are writes
        for req in [
            r#"{"op":"keys","session_id":"s1","keys":["enter"]}"#,
            r#"{"op":"open","cwd":"/","tool":"claude"}"#,
            r#"{"op":"resume","session_id":"s3"}"#,
            r#"{"op":"input","session_id":"s1","kind":"interrupt"}"#,
        ] {
            assert_eq!(ask(&b, req)["error"], "write disabled", "{req}");
        }
        assert!(b.written.lock().unwrap().is_empty());
    }

    // The link only runs on unix (see the `not(unix)` `start_at`), and `open` takes unix paths: a real directory here
    // is a unix-style absolute path
    #[cfg(unix)]
    #[test]
    fn keys_interrupt_open_and_resume_with_write_on() {
        let b = Fake { write_on: true, ..fake() };
        assert_eq!(ask(&b, r#"{"op":"keys","session_id":"s2","keys":["down","enter"]}"#)["ok"], true);
        assert_eq!(ask(&b, r#"{"op":"input","session_id":"s1","kind":"interrupt"}"#)["ok"], true);
        assert_eq!(*b.written.lock().unwrap(), vec![("s2".to_string(), b"\x1b[B\r".to_vec()), ("s1".to_string(), vec![0x1b])]);
        assert_eq!(ask(&b, r#"{"op":"keys","session_id":"s3","keys":["enter"]}"#)["error"], "session not found");
        assert_eq!(ask(&b, r#"{"op":"keys","session_id":"s2","keys":"enter"}"#)["error"], "keys must be a list");
        // open: the directory must exist
        let dir = std::env::temp_dir();
        let r = ask(&b, &json!({"op": "open", "cwd": dir, "tool": "codex"}).to_string());
        assert_eq!(r["ok"], true, "{r}");
        assert!(r["started_at"].as_i64().is_some_and(|t| t > 0));
        assert_eq!(*b.opened.lock().unwrap(), vec![(dir.to_string_lossy().to_string(), "codex".to_string())]);
        assert_eq!(ask(&b, r#"{"op":"open","cwd":"/no/such/dir/here","tool":"claude"}"#)["error"], "no such directory");
        assert_eq!(ask(&b, r#"{"op":"open","cwd":"/","tool":"zsh"}"#)["error"], "unknown tool");
        // resume: only an ended session
        assert_eq!(ask(&b, r#"{"op":"resume","session_id":"s3"}"#)["ok"], true);
        assert_eq!(ask(&b, r#"{"op":"resume","session_id":"s1"}"#)["error"], "session running");
        assert_eq!(ask(&b, r#"{"op":"resume","session_id":"nope"}"#)["error"], "session not found");
        assert_eq!(*b.resumed.lock().unwrap(), vec!["s3".to_string()]);
    }

    #[test]
    fn hello_lists_the_ops_so_the_other_side_can_tell_old_versions() {
        let ops = ask(&fake(), r#"{"op":"hello"}"#)["ops"].clone();
        // "type": input kind `type` (terminal keyboard) and replies accepted while the session works
        for op in ["sessions", "transcript", "input", "screen", "keys", "open", "resume", "subscribe", "type"] {
            assert!(ops.as_array().unwrap().iter().any(|o| o == op), "{op}");
        }
    }

    /// A backend whose session list can change while a subscriber is connected.
    struct Moving(Mutex<Vec<SessionInfo>>);
    impl Backend for Moving {
        fn sessions(&self, _limit: usize) -> Result<Vec<SessionInfo>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn transcript(&self, _: &str, _: Cursor) -> Result<TranscriptPage, String> {
            Err("n/a".into())
        }
    }

    #[test]
    fn subscribe_pushes_changes_and_pings_until_the_reader_goes_away() {
        let b = Moving(Mutex::new(vec![info("a", "busy")]));
        let stop = AtomicBool::new(false);
        let mut out = Vec::new();
        let mut ticks = 0;
        subscribe_loop(&b, &mut out, &stop, std::time::Duration::from_millis(1), 3, |_| {
            ticks += 1;
            match ticks {
                2 => b.0.lock().unwrap()[0].state = "waiting".into(),
                5 => b.0.lock().unwrap().push(info("n", "idle")),
                7 => stop.store(true, Ordering::SeqCst),
                _ => {}
            }
        });
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines[0]["event"], "ready");
        let changed: Vec<&Value> = lines.iter().filter(|l| l["event"] == "changed").collect();
        assert_eq!(changed.len(), 2, "{lines:?}");
        assert_eq!(changed[0]["sessions"], json!(["a"]));
        assert_eq!(changed[1]["sessions"], json!(["n"]));
        assert!(lines.iter().any(|l| l["event"] == "ping"), "a quiet connection still pings: {lines:?}");
    }

    #[test]
    fn subscribe_ends_when_the_reader_is_gone() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        // Returns instead of looping forever on a closed connection
        subscribe_loop(&fake(), &mut Broken, &AtomicBool::new(false), std::time::Duration::from_millis(1), 3, |_| {});
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
    fn subscribe_over_the_socket_says_ready_and_stops_with_the_link() {
        let path = tmp_sock("sub");
        let h = start_at(&path, Arc::new(fake())).unwrap();
        let mut s = UnixStream::connect(&path).unwrap();
        s.write_all(b"{\"op\":\"subscribe\"}\n").unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        let mut r = BufReader::new(s);
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        assert_eq!(serde_json::from_str::<Value>(line.trim()).unwrap()["event"], "ready");
        // Normal requests still work beside an open subscription
        assert_eq!(roundtrip(&path, r#"{"op":"hello"}"#)["ok"], true);
        // Switching the link off ends the subscription: the reader sees the connection close
        h.stop();
        let mut rest = String::new();
        let _ = r.read_to_string(&mut rest);
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
