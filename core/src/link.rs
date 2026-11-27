//! Local session link (#299): lets another program on this machine list makit's sessions and read their conversations.
//!
//! Protocol v1: unix socket `~/.makit/link.sock` (directory 0700, socket 0600), one JSON line in, one JSON line out, then the
//! connection closes. Stateless, so polling is enough and a restart of either side needs no reconnect logic.
//! `transcript` takes `after` (items after that index) or `tail` (only the last N, for a first look at a long conversation).
//! Ops: `hello` / `sessions` / `transcript` / `screen` / `session_cwds` / `last_running` (all read-only) and
//! `input` (reply text, approve, deny, interrupt), `keys` (whitelisted keys), `open` (new session of a known tool
//! in a directory, optionally with its first message), `resume` (an ended session), `archive` (a session that is
//! not running), `stop` / `close` (#D-2299/#D-2300, see below), `update` (`check` is read-only, `install` restarts
//! the app, #D-2122). The writing ops are a separate opt-in in makit's settings and refused while it is off, see
//! [`Backend::write_enabled`].
//!
//! `resume`'s `allow_new_tab` (#D-2138, bool, default `true`): whether to reuse a tab the session already has
//! there is always makit's call, but opening a brand-new one when there is nothing to reuse is the caller's --
//! `false` gets back `{"ok":false,"reason":"no_tab",...}` instead of a tab actually being opened. A successful
//! `resume` reports which branch it took as `"action"`: `"reused_tab"` or `"opened_new_tab"`.
//! Each entry in `sessions`' list carries `has_tab` (#D-2143): whether makit currently has a tab for it at all,
//! so a caller can batch-check which sessions `resume { allow_new_tab: false }` would actually revive without
//! calling `resume` on each one and causing side effects.
//! `subscribe` is the one op that keeps its connection open: it pushes the ids of sessions that changed.
//!
//! `stop` (#D-2299, same action as the sidebar's "Stop") kills the session's tab's process but keeps the tab
//! itself, marked stopped; `close` (#D-2300, same action as the sidebar's "Close") kills the process *and* drops
//! the tab outright. Both take an optional `force` (bool, default `false`): a session that is busy or waiting
//! for approval is refused with `{"ok":false,"reason":"needs_force",...}` unless `force` is `true` -- the UI's
//! own equivalent asks with a confirm dialog first, which a remote caller could never answer, so it is refused
//! outright instead. Each entry in `sessions`' list also carries `tab_state` (#D-2300): `"open"` / `"stopped"` /
//! `"none"`, so a caller can tell which of `stop` or `close` even applies without guessing from `has_tab` alone.
//!
//! `input` with `kind: "text"` while the session sits idle at its prompt (#D-2161): a successful write to the
//! PTY does not mean Claude Code's TUI actually pulled it into a real turn -- an Enter that lands at the wrong
//! moment (mid-repaint, TUI still settling a paste) gets swallowed and the text sits unsent in the input box,
//! with the earlier reply already having said `ok`. This now confirms against the session's own transcript
//! tail before replying: if the sent text has not shown up as a new user message after a short poll, it
//! retries once with one more bare Enter; if that still does not confirm, the reply is
//! `{"ok":false,"error":"...","reason":"not_submitted"}` instead of a false `ok`. Text sent to a *busy* session
//! (appended into the turn that is already running) skips this confirmation: it is not expected to show up as
//! its own transcript entry on any short timescale, so checking for one would misjudge perfectly normal input
//! as having failed.
//!
//! `open`'s `trust` (#D-2168, bool, default `false`, claude only): pre-accept Claude Code's own trust-this-
//! directory dialog for `cwd`, but only when it is under one of the user's own configured auto-trust roots
//! (makit's own setting, default empty) -- otherwise `{"ok":false,"reason":"not_trusted_root",...}` and nothing
//! is opened. `open` and `resume` both also take `permission_mode` (#D-2168, string, claude only): passed
//! straight through as Claude Code's own `--permission-mode` (see [`crate::claude_config::PERMISSION_MODES`]
//! for the real choices); omitted keeps today's behavior. Both claude-only params are refused for any other
//! `tool` with a plain error, not a structured `reason` (no caller has asked for one yet). `resume`'s
//! `permission_mode` only ever takes effect when a *new* tab actually gets opened -- reviving an existing one
//! just restarts its own already-fixed launch command, so it is silently ignored there.
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
use std::time::Duration;

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
    /// Whether makit currently has a tab for this session, running or not (#D-2143): lets a caller batch-check,
    /// with no side effect, which sessions a `resume { allow_new_tab: false }` would actually be able to revive.
    /// Always `false` from [`CoreBackend`] (it has no notion of tabs); [`Backend::sessions`]'s real implementation
    /// (makit's own GPUI workspace) fills this in.
    pub has_tab: bool,
    /// `"open"` (has a tab, not stopped) / `"stopped"` (has a tab, explicitly stopped) / `"none"` (no tab at all,
    /// equivalent to `has_tab: false`), #D-2300: lets a caller tell `stop` (process is running, tab stays) apart
    /// from `close` (tab goes away too) without guessing from `has_tab` alone. Always `"none"` from [`CoreBackend`]
    /// for the same reason `has_tab` is always `false` there.
    pub tab_state: String,
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

/// Which of the three capacity thresholds (#316) refused an `open` / `resume` -- a structured tag alongside the
/// human-readable message, so an automated caller (#D-2099: 总管's own scripts) can tell "too many sessions" apart
/// from "low memory" from "high load" without parsing English text that is free to reword later.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityLimitKind {
    Count,
    LowMemory,
    HighLoad,
}

impl CapacityLimitKind {
    /// The `capacity_limit` field's value in the JSON reply.
    fn tag(self) -> &'static str {
        match self {
            CapacityLimitKind::Count => "count",
            CapacityLimitKind::LowMemory => "low_memory",
            CapacityLimitKind::HighLoad => "high_load",
        }
    }
}

/// What a successful `resume` actually did (#D-2138): the caller controls whether a new tab may be opened via
/// `allow_new_tab`, so it gets told which branch was taken instead of just "ok".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResumeAction {
    /// Reused a tab the session already had there (a `stopped` placeholder, or one whose terminal simply exited).
    ReusedTab,
    /// No tab to reuse; opened a brand-new one (only reachable when `allow_new_tab` was `true`).
    OpenedNewTab,
}

impl ResumeAction {
    /// The `action` field's value in the JSON reply.
    fn tag(self) -> &'static str {
        match self {
            ResumeAction::ReusedTab => "reused_tab",
            ResumeAction::OpenedNewTab => "opened_new_tab",
        }
    }
}

/// An `open` / `resume` failure. Every other [`Backend`] method keeps the plain `Result<_, String>` it always had --
/// this richer type exists only where #D-2099 asked for a structured reason, so `message` is still there for every
/// caller that just wants to display or log something, and `capacity` is `Some` only when a capacity threshold
/// (rather than, say, "no such directory" or "unknown tool") was the cause. `no_tab` is the same idea for #D-2138:
/// `resume` with `allow_new_tab: false` refusing because there is nothing to reuse and it is not allowed to open one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenError {
    pub message: String,
    pub capacity: Option<CapacityLimitKind>,
    pub no_tab: bool,
    /// #D-2168: `open` had `trust: true` for a directory that is not under any of the user's configured
    /// auto-trust roots -- refused rather than silently opening untrusted, or quietly ignoring `trust`.
    pub not_trusted_root: bool,
    /// #D-2299/#D-2300: `stop`/`close` refused because the session is busy or waiting for approval and the
    /// caller did not pass `force: true` -- the same "ask first" judgment the UI makes with a confirm dialog,
    /// just refused outright instead of popping one a remote caller could never answer.
    pub needs_force: bool,
}

impl OpenError {
    pub fn capacity(kind: CapacityLimitKind, message: impl Into<String>) -> Self {
        Self { message: message.into(), capacity: Some(kind), no_tab: false, not_trusted_root: false, needs_force: false }
    }

    /// #D-2138: `resume` had `allow_new_tab: false` and this session has no tab to reuse.
    pub fn no_tab(message: impl Into<String>) -> Self {
        Self { message: message.into(), capacity: None, no_tab: true, not_trusted_root: false, needs_force: false }
    }

    /// #D-2168: see [`OpenError::not_trusted_root`] field doc.
    pub fn not_trusted_root(message: impl Into<String>) -> Self {
        Self { message: message.into(), capacity: None, no_tab: false, not_trusted_root: true, needs_force: false }
    }

    /// #D-2299/#D-2300: see [`OpenError::needs_force`] field doc.
    pub fn needs_force(message: impl Into<String>) -> Self {
        Self { message: message.into(), capacity: None, no_tab: false, not_trusted_root: false, needs_force: true }
    }
}

impl From<String> for OpenError {
    fn from(message: String) -> Self {
        Self { message, capacity: None, no_tab: false, not_trusted_root: false, needs_force: false }
    }
}

impl From<&str> for OpenError {
    fn from(message: &str) -> Self {
        Self { message: message.into(), capacity: None, no_tab: false, not_trusted_root: false, needs_force: false }
    }
}

/// Lets `?` downgrade an [`OpenError`] back to a plain message wherever a caller only ever wanted `String` (the
/// `write` / `screen` requests forwarded through the same main-thread channel as `open` / `resume`, #D-2099).
impl From<OpenError> for String {
    fn from(e: OpenError) -> String {
        e.message
    }
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
    /// Start `tool` in a new tab in `cwd`, typing `text` as its first message when given (so the session writes its
    /// file right away instead of waiting at an empty prompt). Only called after [`plan_open`] / [`validate_open_text`]
    /// accepted the request and the directory exists.
    ///
    /// `trust` (#D-2168, claude only): pre-accept Claude Code's own trust-this-directory dialog for `cwd` before
    /// starting it there -- but only when `cwd` is under one of the caller's own configured auto-trust roots;
    /// otherwise this must return `Err(OpenError::not_trusted_root(..))` without opening anything. `permission_mode`
    /// (claude only) is passed straight through as Claude Code's own `--permission-mode`; `None` keeps today's
    /// behavior. Both are no-ops/unsupported for any other `tool` -- `handle_line` already refuses those before
    /// a `Backend` ever sees them, so an implementation does not need to re-check `tool` for these two itself.
    fn open(&self, _cwd: &str, _tool: &str, _text: Option<&str>, _trust: bool, _permission_mode: Option<&str>) -> Result<(), OpenError> {
        Err("open not supported".into())
    }
    /// Resume an ended session in its own directory. Whether to reuse a tab the session still has there is
    /// makit's call either way; whether to open a *new* one when there is nothing to reuse is the caller's
    /// (#D-2138: that policy belongs to whoever is orchestrating across sessions, not to makit). When
    /// `allow_new_tab` is `false` and there is no tab to revive, this must return `Err(OpenError::no_tab(..))`
    /// without opening anything.
    ///
    /// `permission_mode` (#D-2168, claude only, see [`Backend::open`]): only takes effect when a *new* tab ends
    /// up getting opened -- reviving an existing tab just restarts its own already-fixed launch command, so
    /// there is nothing to pass it to; implementations may silently ignore it on that path.
    fn resume(&self, _session_id: &str, _allow_new_tab: bool, _permission_mode: Option<&str>) -> Result<ResumeAction, OpenError> {
        Err("resume not supported".into())
    }
    /// Archive a session that is not running (same action as the sidebar's archive). Only called after the caller
    /// checked the session exists and is not running.
    fn archive(&self, _session_id: &str) -> Result<(), String> {
        Err("archive not supported".into())
    }
    /// Stop the session's tab's process, keeping the tab itself (same action as the sidebar's "Stop", #D-2299):
    /// equivalent to pulling the plug, not a graceful shutdown. A session that is busy or waiting for approval is
    /// refused with [`OpenError::needs_force`] unless `force` is `true` -- the UI's own equivalent asks with a
    /// confirm dialog first; a remote caller gets refused outright instead, since it could never answer one.
    /// Idle sessions, and ones with no tab to stop at all, are implementation-defined (a no-op or an error -- the
    /// point of `force` is only to skip the busy/waiting confirmation, not to change what "nothing to stop" means).
    fn stop(&self, _session_id: &str, _force: bool) -> Result<(), OpenError> {
        Err("stop not supported".into())
    }
    /// Close the session's tab outright (same action as the sidebar's "Close", #D-2300): the tab is gone, not left
    /// behind `stopped`, and its process is killed the same way closing any tab does (#3's MAKIT_PTY_ID cleanup).
    /// Same busy/waiting-needs-`force` rule as [`Backend::stop`].
    fn close(&self, _session_id: &str, _force: bool) -> Result<(), OpenError> {
        Err("close not supported".into())
    }
    /// Every distinct `cwd` the session's directory moved through (#313: an external tool attributing a session to
    /// a project needs more than just the starting directory).
    fn session_cwds(&self, _session_id: &str) -> Result<CwdHistory, String> {
        Err("session_cwds not supported".into())
    }
    /// Whether a session with this id exists at all (#D-2078). Override this with a direct lookup when one is
    /// available: the default falls back to [`Backend::sessions`]'s list, which is capped at [`MAX_LIMIT`] and can
    /// come back short under load (file-descriptor pressure, a very large project history) -- `resume` / `archive`
    /// must still work on a session the list happened to leave out.
    fn session_exists(&self, session_id: &str) -> bool {
        self.sessions(MAX_LIMIT).is_ok_and(|list| list.iter().any(|s| s.session_id == session_id))
    }
    /// The sessions that were running right before makit's last quit (#D-2122). Read-only, no live state behind
    /// it -- just the on-disk snapshot ([`crate::last_running`]) -- so the default here is the real answer for
    /// every backend, not a placeholder; `NativeBackend` never needs to override it.
    fn last_running(&self) -> Vec<crate::last_running::LastRunning> {
        crate::last_running::read()
    }
    /// What the running instance currently knows about a newer release (#D-2122). Not a forced synchronous
    /// network check: a socket request cannot sit through a GitHub round trip, and the periodic background check
    /// already keeps this current. May still kick off a fresh check in the background before answering.
    fn update_check(&self) -> Result<UpdateStatus, String> {
        Err("update not supported".into())
    }
    /// Starts installing the currently-available release and restarting (#D-2122). `after_restart`, when given,
    /// is what the restarted instance should type into that session once it goes idle again -- 总管's own `/loop`
    /// uses this to reconnect to itself without hardcoding any session. Errors when there is no update available,
    /// or this machine cannot apply it in place (homebrew-managed install, dev instance).
    fn update_install(&self, _after_restart: Option<AfterRestart>) -> Result<(), String> {
        Err("update not supported".into())
    }
}

/// [`Backend::update_check`]'s answer.
#[derive(Clone, Debug, PartialEq)]
pub struct UpdateStatus {
    pub current: String,
    /// "idle" | "checking" | "available" | "installing"
    pub state: String,
    pub latest: Option<String>,
    pub notes: String,
    pub page_url: String,
}

/// What to type into which session once the restarted instance sees it go idle (#D-2122's `update install`
/// parameter of the same name).
#[derive(Clone, Debug, PartialEq)]
pub struct AfterRestart {
    pub session_id: String,
    pub text: String,
}

/// A session's directory history, as reported by the `session_cwds` op (#313).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CwdHistory {
    /// Every distinct `cwd` seen, in first-seen order, paired with how many records had it.
    pub trail: Vec<(String, u32)>,
    /// The very first `cwd` the session ran in.
    pub first: String,
    /// The most recent `cwd`, which may not be `trail`'s last entry: a session that moves back to an earlier
    /// directory still counts against that directory's existing trail entry instead of adding a new one.
    pub last: String,
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

/// One step of carrying out a planned write: either bytes to send, or a pause before the next step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WriteStep {
    Bytes(Vec<u8>),
    Wait(Duration),
}

/// How long to wait after a bracketed paste before sending Enter. Claude Code's TUI takes a beat to pull a paste
/// into its input box; Enter sent right behind the paste's end marker can arrive before that happens and gets
/// swallowed, leaving the message sitting in the input box unsent (seen repeatedly in practice).
pub const PASTE_SETTLE: Duration = Duration::from_millis(200);

/// Same as [`PASTE_SETTLE`], but for a session that is still busy (#312). A busy TUI is also consuming its own
/// streamed output when the paste lands, so it gets to reading our input later than an idle one would; 200ms isn't
/// always enough there and real reports showed text sitting unsent. This is an experience-based margin (roughly 3x
/// the idle wait), not a measured one — there was no way to reproduce the exact busy timing in a test. If it still
/// isn't enough in practice, the more principled fix is polling `Backend::screen` for the pasted text instead of
/// guessing a bigger constant.
pub const PASTE_SETTLE_BUSY: Duration = Duration::from_millis(600);

/// What `hello` lists, so the other side can tell an older makit (fewer ops) from this one.
/// "type" is not an op of its own: it says `input` takes kind `type` and replies while the session works.
pub const OPS: &[&str] = &["hello", "sessions", "transcript", "input", "screen", "keys", "open", "resume", "archive", "stop", "close", "subscribe", "type", "session_cwds", "last_running", "update"];

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
/// every newline would submit on its own). No Enter: the caller adds it. The `bool` says whether a paste was used,
/// so the caller knows whether Enter needs [`PASTE_SETTLE`] first.
fn text_bytes(text: &str) -> Result<(Vec<u8>, bool), String> {
    let text = text.replace("\r\n", "\n");
    if text.chars().count() > MAX_INPUT_CHARS {
        return Err("text too long".into());
    }
    if text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err("control characters".into());
    }
    if !text.contains('\n') {
        return Ok((text.into_bytes(), false));
    }
    let mut out = b"\x1b[200~".to_vec();
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    Ok((out, true))
}

/// Check `open`'s optional first-message text: same rules as a reply's text (non-empty, length, no control
/// characters other than newline/tab). Unlike [`text_bytes`] this returns the text itself, not PTY bytes: `open`
/// embeds it as one quoted shell argument rather than writing it into an already-running terminal.
pub fn validate_open_text(text: &str) -> Result<String, String> {
    let text = text.replace("\r\n", "\n");
    if text.trim().is_empty() {
        return Err("empty text".into());
    }
    if text.chars().count() > MAX_INPUT_CHARS {
        return Err("text too long".into());
    }
    if text.chars().any(|c| c.is_control() && c != '\n' && c != '\t') {
        return Err("control characters".into());
    }
    Ok(text)
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
pub fn plan_input(state: &str, waiting_for: &str, kind: &InputKind) -> Result<Vec<WriteStep>, String> {
    // "waiting" + "input needed" is just Claude Code sitting at its prompt, not a real dialog (#308):
    // settle it to idle before any of the checks below, same as the sidebar and notifications do
    let (state, waiting_for) = crate::running::settle_prompt_wait(state, waiting_for);
    if state == "ended" {
        return Err("session ended".into());
    }
    let at_approval = state == "waiting" && waiting_for != "user";
    match kind {
        // Esc while a turn runs stops it; at a prompt it would do nothing useful, in a dialog it would answer it
        InputKind::Interrupt if state == "busy" => Ok(vec![WriteStep::Bytes(vec![0x1b])]),
        InputKind::Interrupt => Err("not busy".into()),
        InputKind::Approve if at_approval => Ok(vec![WriteStep::Bytes(b"\r".to_vec())]),
        InputKind::Deny if at_approval => Ok(vec![WriteStep::Bytes(vec![0x1b])]),
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
            let (bytes, pasted) = text_bytes(text)?;
            Ok(enter_after(bytes, pasted, state == "busy"))
        }
        InputKind::Type(text) => {
            if text.is_empty() {
                return Err("empty text".into());
            }
            let (bytes, _) = text_bytes(text)?;
            Ok(vec![WriteStep::Bytes(bytes)])
        }
    }
}

/// Carry out a plan from [`plan_input`]: write each `Bytes` step, really wait out each `Wait` one. The waits are why
/// this is not folded into `plan_input` itself: that function stays pure and instant, so tests can check the planned
/// steps without paying for the real delay.
fn run_write_steps(backend: &dyn Backend, session_id: &str, steps: Vec<WriteStep>) -> Result<(), String> {
    for step in steps {
        match step {
            WriteStep::Bytes(bytes) => backend.write(session_id, bytes)?,
            WriteStep::Wait(d) => std::thread::sleep(d),
        }
    }
    Ok(())
}

/// How often to re-check the transcript after writing a text reply, and how many times, before concluding it
/// did not submit and retrying with one more bare Enter (#D-2161). 8 × 150ms = 1.2s of budget per attempt --
/// long enough for Claude Code to notice the Enter and write the new line to its own transcript file, short
/// enough that a genuinely swallowed Enter does not leave the caller hanging for long before it hears back.
const SUBMIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);
const SUBMIT_POLL_ATTEMPTS: u32 = 8;

/// What [`submit_text_and_confirm`] actually managed.
enum SubmitOutcome {
    Submitted,
    /// Wrote (possibly retried once), but the transcript never showed the text as a new user message.
    NotSubmitted,
    /// A write itself failed (session closed, PTY gone, ...) -- not a submission question at all.
    WriteFailed(String),
}

/// Pure: does `after`'s transcript tail show `sent` as a message that was not there in `before_total`'s snapshot
/// (#D-2161)? Split out from the polling loop below so the judgment itself -- what counts as "it submitted" --
/// can be tested without paying for a single real sleep.
fn text_submitted(before_total: usize, after: &TranscriptPage, sent: &str) -> bool {
    let wanted = normalize_for_match(sent);
    // `after` holds only the items written after `before_total`; if the transcript was rewritten, `from` is 0 and we
    // cannot tell old items from new ones, so we do not count it (#334)
    after.total > before_total
        && after.from == before_total
        && !wanted.is_empty()
        && after.items.iter().any(|v| v["kind"] == "user" && normalize_for_match(v["text"].as_str().unwrap_or("")).contains(&wanted))
}

/// Compares a message the way it is worth comparing (#334): Claude Code wraps a pasted block in a
/// `<pasted_content id="…">…</pasted_content>` tag in the transcript, and the transcript's whitespace need not match
/// what we sent, so both sides drop the tags and fold runs of whitespace.
fn normalize_for_match(s: &str) -> String {
    let mut out = s.to_string();
    while let Some(start) = out.find("<pasted_content") {
        match out[start..].find('>') {
            Some(len) => out.replace_range(start..start + len + 1, ""),
            None => break,
        }
    }
    out.replace("</pasted_content>", "").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Writes `steps` (from [`plan_input`] for an `InputKind::Text`), then confirms against the session's own
/// transcript that the text actually became a new user message -- not just that the bytes reached the PTY
/// (#D-2161). Retries with one bare Enter if the first attempt never confirms. `poll_interval` / `poll_attempts`
/// are parameters (not the `SUBMIT_POLL_*` constants directly) so tests can shrink the real sleeps to near zero
/// without touching the retry logic itself.
fn submit_text_and_confirm(backend: &dyn Backend, session_id: &str, steps: Vec<WriteStep>, text: &str, poll_interval: Duration, poll_attempts: u32) -> SubmitOutcome {
    let before_total = backend.transcript(session_id, Cursor::Tail(1)).map(|p| p.total).unwrap_or(0);
    if let Err(e) = run_write_steps(backend, session_id, steps) {
        return SubmitOutcome::WriteFailed(e);
    }
    let confirmed = |backend: &dyn Backend| {
        for _ in 0..poll_attempts {
            std::thread::sleep(poll_interval);
            // Everything written since the write: a busy session can answer or log other records before our message is
            // the last one, so the last item alone is not enough to judge (#334)
            if let Ok(page) = backend.transcript(session_id, Cursor::After(before_total)) {
                if text_submitted(before_total, &page, text) {
                    return true;
                }
            }
        }
        false
    };
    if confirmed(backend) {
        return SubmitOutcome::Submitted;
    }
    // 没确认提交：可能是那次 Enter 被吞了，补一次单独的回车重试
    if let Err(e) = backend.write(session_id, b"\r".to_vec()) {
        return SubmitOutcome::WriteFailed(e);
    }
    if confirmed(backend) {
        return SubmitOutcome::Submitted;
    }
    SubmitOutcome::NotSubmitted
}

/// `bytes` then Enter. A paste needs [`PASTE_SETTLE`] in between: without it, Enter can land before Claude Code's
/// TUI has pulled the pasted text into its input box and gets swallowed, so the message never submits. `busy` picks
/// [`PASTE_SETTLE_BUSY`] instead (#312): a busy TUI reads our input later than an idle one.
fn enter_after(bytes: Vec<u8>, pasted: bool, busy: bool) -> Vec<WriteStep> {
    if pasted {
        let settle = if busy { PASTE_SETTLE_BUSY } else { PASTE_SETTLE };
        vec![WriteStep::Bytes(bytes), WriteStep::Wait(settle), WriteStep::Bytes(b"\r".to_vec())]
    } else {
        let mut out = bytes;
        out.push(b'\r');
        vec![WriteStep::Bytes(out)]
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

/// Same as [`fail`], plus the structured `capacity_limit` field when the error was the capacity cap (#D-2099),
/// or `reason: "no_tab"` when it was `resume` refusing to open a new tab (#D-2138). Added as extra fields only:
/// `hello.ops` and the protocol version do not change, so a caller that does not look for them sees exactly the
/// same reply it always did.
fn fail_open(e: OpenError) -> String {
    let mut v = json!({"ok": false, "error": e.message});
    if let Some(k) = e.capacity {
        v.as_object_mut().expect("object").insert("capacity_limit".into(), json!(k.tag()));
    }
    if e.no_tab {
        v.as_object_mut().expect("object").insert("reason".into(), json!("no_tab"));
    }
    if e.not_trusted_root {
        v.as_object_mut().expect("object").insert("reason".into(), json!("not_trusted_root"));
    }
    if e.needs_force {
        v.as_object_mut().expect("object").insert("reason".into(), json!("needs_force"));
    }
    v.to_string()
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
            let text = match req.get("text").and_then(Value::as_str) {
                Some(t) => match validate_open_text(t) {
                    Ok(t) => Some(t),
                    Err(e) => return fail(e),
                },
                None => None,
            };
            if !Path::new(cwd).is_dir() {
                return fail("no such directory");
            }
            // #D-2168: both claude-only, and makit itself (not the Backend) refuses them for any other tool so
            // an implementation never has to re-check `tool` for these two.
            let trust = req.get("trust").and_then(Value::as_bool).unwrap_or(false);
            let permission_mode = req.get("permission_mode").and_then(Value::as_str);
            if (trust || permission_mode.is_some()) && tool != "claude" {
                return fail("trust and permission_mode are only supported for claude");
            }
            if let Some(mode) = permission_mode {
                if !crate::claude_config::is_known_permission_mode(mode) {
                    return fail(format!("unknown permission_mode: {mode}"));
                }
            }
            // Without `text` the new session's id is only known once the tool writes its first record on its own
            // (which can take a while at an empty prompt): the other side picks the newest session of this tool in
            // this directory that started at or after `started_at`. `text` gives the tool something to write right
            // away, so that poll finds it quickly.
            let started_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
            match backend.open(cwd, tool, text.as_deref(), trust, permission_mode) {
                Ok(()) => json!({"ok": true, "started_at": started_at}).to_string(),
                Err(e) => fail_open(e),
            }
        }
        Some("archive") => {
            if !backend.write_enabled() {
                return fail("write disabled");
            }
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            if backend.session_state(id).is_some() {
                return fail("session running");
            }
            if !backend.session_exists(id) {
                return fail("session not found");
            }
            match backend.archive(id) {
                Ok(()) => json!({"ok": true}).to_string(),
                Err(e) => fail(e),
            }
        }
        Some("stop") => {
            if !backend.write_enabled() {
                return fail("write disabled");
            }
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            if !backend.session_exists(id) {
                return fail("session not found");
            }
            // #D-2299: omitted = `false`, so a caller that has never heard of `force` gets the safe refusal
            // (structured `reason:"needs_force"`) on a busy/waiting session instead of silently killing it.
            let force = req.get("force").and_then(Value::as_bool).unwrap_or(false);
            match backend.stop(id, force) {
                Ok(()) => json!({"ok": true}).to_string(),
                Err(e) => fail_open(e),
            }
        }
        Some("close") => {
            if !backend.write_enabled() {
                return fail("write disabled");
            }
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            if !backend.session_exists(id) {
                return fail("session not found");
            }
            let force = req.get("force").and_then(Value::as_bool).unwrap_or(false);
            match backend.close(id, force) {
                Ok(()) => json!({"ok": true}).to_string(),
                Err(e) => fail_open(e),
            }
        }
        Some("resume") => {
            if !backend.write_enabled() {
                return fail("write disabled");
            }
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            // Already running is not an error here (#D-2044): the caller asking to resume may simply not know
            // yet that it already is, so it gets back a quiet ok instead of having to treat this as a failure.
            if backend.session_state(id).is_some() {
                return json!({"ok": true, "already_running": true}).to_string();
            }
            // Checked directly against disk (#D-2078), not against `sessions`'s list: that list is capped and can
            // come back short under load, and a session it happened to leave out must still be resumable.
            if !backend.session_exists(id) {
                return fail("session not found");
            }
            // #D-2138: the tab-reuse-vs-open-new policy is the caller's, not makit's; omitted = `true`, so a
            // caller that does not know about this yet keeps today's behaviour (open one when there is nothing
            // to reuse) instead of silently losing the ability to resume at all.
            let allow_new_tab = req.get("allow_new_tab").and_then(Value::as_bool).unwrap_or(true);
            // #D-2168: whether this session's tool is claude isn't known at this layer (only `sessions`'s list
            // carries `tool`, and fetching the whole list just to check one session isn't worth it) -- the
            // `Backend` itself silently ignores this when it doesn't apply, same as it already does on the
            // revive-an-existing-tab path.
            let permission_mode = req.get("permission_mode").and_then(Value::as_str);
            if let Some(mode) = permission_mode {
                if !crate::claude_config::is_known_permission_mode(mode) {
                    return fail(format!("unknown permission_mode: {mode}"));
                }
            }
            match backend.resume(id, allow_new_tab, permission_mode) {
                Ok(action) => json!({"ok": true, "action": action.tag()}).to_string(),
                Err(e) => fail_open(e),
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
        Some("session_cwds") => {
            let Some(id) = req.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
                return fail("session_id required");
            };
            match backend.session_cwds(id) {
                Ok(h) => {
                    let cwds: Vec<Value> = h.trail.iter().map(|(cwd, n)| json!({"cwd": cwd, "n": n})).collect();
                    json!({"ok": true, "cwds": cwds, "first": h.first, "last": h.last}).to_string()
                }
                Err(e) => fail(e),
            }
        }
        Some("last_running") => {
            let sessions: Vec<Value> = backend
                .last_running()
                .into_iter()
                .map(|e| json!({"session_id": e.session_id, "name": e.name, "cwd": e.cwd, "last_active": e.last_active}))
                .collect();
            json!({"ok": true, "sessions": sessions}).to_string()
        }
        Some("update") => match req.get("cmd").and_then(Value::as_str) {
            Some("check") => match backend.update_check() {
                Ok(s) => json!({"ok": true, "current": s.current, "state": s.state, "latest": s.latest, "notes": s.notes, "page_url": s.page_url}).to_string(),
                Err(e) => fail(e),
            },
            Some("install") => {
                if !backend.write_enabled() {
                    return fail("write disabled");
                }
                let after_restart = match req.get("after_restart") {
                    None | Some(Value::Null) => None,
                    Some(v) => {
                        let sid = v.get("session_id").and_then(Value::as_str).filter(|s| !s.is_empty());
                        let text = v.get("text").and_then(Value::as_str).filter(|s| !s.is_empty());
                        match (sid, text) {
                            (Some(session_id), Some(text)) => Some(AfterRestart { session_id: session_id.into(), text: text.into() }),
                            _ => return fail("after_restart needs non-empty session_id and text"),
                        }
                    }
                };
                match backend.update_install(after_restart) {
                    Ok(()) => json!({"ok": true}).to_string(),
                    Err(e) => fail(e),
                }
            }
            _ => fail("cmd must be \"check\" or \"install\""),
        },
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
            let steps = match plan_input(&state, &waiting_for, &kind) {
                Ok(s) => s,
                Err(e) => return fail(e),
            };
            // 只有真正的回话文字、且会话是坐在空 prompt 前（idle）时才需要确认提交（#D-2161）：approve/deny/
            // interrupt/type 各自的字节一次写进去就是完整的动作，没有"写进去了但没当成一次真正提交"这个中间
            // 状态；busy 时追加的文字是塞进正在跑的那一轮里（`plan_input` 文档里写的"the tool takes it into
            // the running turn"），不会立刻单独变成一条 transcript 里能看到的 user 消息，用短轮询去确认只会把
            // 这种本来就正常的输入错判成"没提交"再去补一次多余的回车——这正是两条既有回归测试
            // （`a_multiline_reply_to_a_busy_session_settles_longer_end_to_end` 等）第一次改的时候被破坏、后来
            // 揭出来的问题
            if let InputKind::Text(text) = &kind {
                let (settled_state, _) = crate::running::settle_prompt_wait(&state, &waiting_for);
                if settled_state == "idle" {
                    return match submit_text_and_confirm(backend, id, steps, text, SUBMIT_POLL_INTERVAL, SUBMIT_POLL_ATTEMPTS) {
                        SubmitOutcome::Submitted => json!({"ok": true}).to_string(),
                        SubmitOutcome::NotSubmitted => {
                            let mut v = json!({"ok": false, "error": "text did not submit (the Enter may have been swallowed)"});
                            v.as_object_mut().expect("object").insert("reason".into(), json!("not_submitted"));
                            v.to_string()
                        }
                        SubmitOutcome::WriteFailed(e) => fail(e),
                    };
                }
            }
            match run_write_steps(backend, id, steps) {
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
                has_tab: false,
                tab_state: "none".into(),
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

    fn archive(&self, session_id: &str) -> Result<(), String> {
        crate::archive::archive_session(session_id.to_string())
    }

    fn session_cwds(&self, session_id: &str) -> Result<CwdHistory, String> {
        let (path, _tool) = crate::sessions::locate_session_file(session_id).ok_or("session not found")?;
        let state = crate::scan_cache::scan_session_file(&path).ok_or("failed to read session file")?;
        Ok(CwdHistory { trail: state.cwd_trail, first: state.start_cwd, last: state.last_cwd })
    }

    fn session_exists(&self, session_id: &str) -> bool {
        crate::sessions::locate_session_file(session_id).is_some()
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
        /// What `open` received: (cwd, tool, text)
        /// (cwd, tool, text, trust, permission_mode)
        opened: Mutex<Vec<(String, String, Option<String>, bool, Option<String>)>>,
        /// What `resume` received each time: (session id, `allow_new_tab`, `permission_mode`)
        resumed: Mutex<Vec<(String, bool, Option<String>)>>,
        /// What `resume` returns when not blocked by the capacity cap (#D-2138).
        resume_result: Mutex<Result<ResumeAction, OpenError>>,
        /// What `open` returns when not blocked by the capacity cap (#D-2168: lets a test simulate `not_trusted_root`).
        open_result: Mutex<Result<(), OpenError>>,
        archived: Mutex<Vec<String>>,
        /// What `stop` received each time: (session id, `force`)
        stopped: Mutex<Vec<(String, bool)>>,
        /// What `close` received each time: (session id, `force`)
        closed: Mutex<Vec<(String, bool)>>,
        stop_result: Mutex<Result<(), OpenError>>,
        close_result: Mutex<Result<(), OpenError>>,
        /// Session ids that exist (so `session_exists` is true for them) but are not in `sessions`, simulating a
        /// session the capped/truncated list leaves out (#D-2078).
        existing_but_unlisted: Vec<String>,
        /// Set to simulate `open` / `resume` refusing because of the capacity cap (#D-2099), tagged with which
        /// threshold tripped.
        capacity_block: Mutex<Option<CapacityLimitKind>>,
        last_running_snapshot: Vec<crate::last_running::LastRunning>,
        update_check_result: Result<UpdateStatus, String>,
        update_install_result: Result<(), String>,
        /// What `update_install` received each time it was called.
        update_installs: Mutex<Vec<Option<AfterRestart>>>,
        /// #D-2161：会话 "sub" 的 transcript 要在收到这么多次 `write` 之后，tail 里才"冒出"那条用户消息
        /// （`None` = 一直不冒出，模拟回车被吞死连重试也没用）；写入次数看 `written`，不是真的解析字节内容
        submits_after_write_n: Mutex<Option<usize>>,
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
            has_tab: false,
            tab_state: "none".into(),
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
        fn open(&self, cwd: &str, tool: &str, text: Option<&str>, trust: bool, permission_mode: Option<&str>) -> Result<(), OpenError> {
            if let Some(kind) = *self.capacity_block.lock().unwrap() {
                return Err(OpenError::capacity(kind, "at the cap"));
            }
            self.opened.lock().unwrap().push((cwd.to_string(), tool.to_string(), text.map(String::from), trust, permission_mode.map(String::from)));
            self.open_result.lock().unwrap().clone()
        }
        fn resume(&self, id: &str, allow_new_tab: bool, permission_mode: Option<&str>) -> Result<ResumeAction, OpenError> {
            if let Some(kind) = *self.capacity_block.lock().unwrap() {
                return Err(OpenError::capacity(kind, "at the cap"));
            }
            self.resumed.lock().unwrap().push((id.to_string(), allow_new_tab, permission_mode.map(String::from)));
            self.resume_result.lock().unwrap().clone()
        }
        fn archive(&self, id: &str) -> Result<(), String> {
            self.archived.lock().unwrap().push(id.to_string());
            Ok(())
        }
        fn stop(&self, id: &str, force: bool) -> Result<(), OpenError> {
            self.stopped.lock().unwrap().push((id.to_string(), force));
            self.stop_result.lock().unwrap().clone()
        }
        fn close(&self, id: &str, force: bool) -> Result<(), OpenError> {
            self.closed.lock().unwrap().push((id.to_string(), force));
            self.close_result.lock().unwrap().clone()
        }
        fn transcript(&self, id: &str, cursor: Cursor) -> Result<TranscriptPage, String> {
            if id == "sub" {
                let writes = self.written.lock().unwrap().iter().filter(|(sid, _)| sid == "sub").count();
                let submitted = self.submits_after_write_n.lock().unwrap().is_some_and(|n| writes >= n);
                let items: Vec<Value> = if submitted { vec![json!({"kind": "user", "text": "hi there", "id": "u1", "time": null})] } else { vec![] };
                let total = items.len();
                let from = cursor_start(total, cursor);
                return Ok(TranscriptPage { total, from, items: items[from..].to_vec() });
            }
            if id != "s1" {
                return Err("session not found".into());
            }
            let from = cursor_start(self.total, cursor);
            Ok(TranscriptPage { total: self.total, from, items: (from..self.total).map(|i| json!({"id": format!("i{i}")})).collect() })
        }
        fn session_cwds(&self, id: &str) -> Result<CwdHistory, String> {
            if id != "s1" {
                return Err("session not found".into());
            }
            Ok(CwdHistory { trail: vec![("/a".into(), 2), ("/b".into(), 1)], first: "/a".into(), last: "/a".into() })
        }
        fn session_exists(&self, id: &str) -> bool {
            self.sessions.iter().any(|s| s.session_id == id) || self.existing_but_unlisted.iter().any(|s| s == id)
        }
        fn last_running(&self) -> Vec<crate::last_running::LastRunning> {
            self.last_running_snapshot.clone()
        }
        fn update_check(&self) -> Result<UpdateStatus, String> {
            self.update_check_result.clone()
        }
        fn update_install(&self, after_restart: Option<AfterRestart>) -> Result<(), String> {
            self.update_installs.lock().unwrap().push(after_restart);
            self.update_install_result.clone()
        }
    }

    fn fake() -> Fake {
        Fake {
            sessions: vec![info("s1", "busy"), info("s2", "waiting"), info("s3", "ended")],
            total: 3,
            write_on: false,
            written: Mutex::new(Vec::new()),
            opened: Mutex::new(Vec::new()),
            resumed: Mutex::new(Vec::new()),
            resume_result: Mutex::new(Ok(ResumeAction::OpenedNewTab)),
            open_result: Mutex::new(Ok(())),
            archived: Mutex::new(Vec::new()),
            stopped: Mutex::new(Vec::new()),
            closed: Mutex::new(Vec::new()),
            stop_result: Mutex::new(Ok(())),
            close_result: Mutex::new(Ok(())),
            existing_but_unlisted: Vec::new(),
            capacity_block: Mutex::new(None),
            last_running_snapshot: Vec::new(),
            update_check_result: Err("update not supported".into()),
            update_install_result: Ok(()),
            update_installs: Mutex::new(Vec::new()),
            submits_after_write_n: Mutex::new(None),
        }
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

    /// #D-2143: a batch check, no side effect -- `has_tab` is just whatever the `Backend` reports per session.
    #[test]
    fn sessions_reports_has_tab_from_the_backend() {
        let mut b = fake();
        b.sessions[0].has_tab = true;
        let r = ask(&b, r#"{"op":"sessions"}"#);
        let by_id = |id: &str| r["sessions"].as_array().unwrap().iter().find(|s| s["session_id"] == id).unwrap().clone();
        assert_eq!(by_id("s1")["has_tab"], true);
        assert_eq!(by_id("s2")["has_tab"], false);
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

    /// One `Bytes` step, no wait: what a single-line reply or keystroke plans.
    fn one(bytes: &[u8]) -> Vec<WriteStep> {
        vec![WriteStep::Bytes(bytes.to_vec())]
    }

    #[test]
    fn plan_text_goes_to_a_session_at_its_prompt_with_enter() {
        assert_eq!(plan_input("idle", "", &text("继续")).unwrap(), one("继续\r".as_bytes()));
        // Waiting for the user's answer is also a prompt
        assert_eq!(plan_input("waiting", "user", &text("好")).unwrap(), one("好\r".as_bytes()));
        // While it works the tool takes the message into the running turn (claude queues what is typed meanwhile), like
        // typing in makit itself (replying while the agent works, as when typing in makit)
        assert_eq!(plan_input("busy", "", &text("顺便看下日志")).unwrap(), one("顺便看下日志\r".as_bytes()));
        // Several lines: one bracketed paste, a settling wait, then Enter on its own — not glued right behind the
        // paste's end marker, or Claude Code's TUI can swallow it (#299 bug report)
        assert_eq!(
            plan_input("idle", "", &text("a\r\nb")).unwrap(),
            vec![WriteStep::Bytes(b"\x1b[200~a\nb\x1b[201~".to_vec()), WriteStep::Wait(PASTE_SETTLE), WriteStep::Bytes(b"\r".to_vec())],
        );
        // Waiting for the user's answer is also a prompt, not a busy TUI: short settle
        assert_eq!(
            plan_input("waiting", "user", &text("a\nb")).unwrap(),
            vec![WriteStep::Bytes(b"\x1b[200~a\nb\x1b[201~".to_vec()), WriteStep::Wait(PASTE_SETTLE), WriteStep::Bytes(b"\r".to_vec())],
        );
    }

    #[test]
    fn a_multiline_reply_settles_longer_while_the_session_is_busy() {
        // #312: a busy TUI is still consuming its own streamed output when our paste lands, so it gets around to
        // reading the paste into the input box later than an idle one would. The idle PASTE_SETTLE isn't always
        // enough there, and real reports showed Enter landing before the paste was in the box: text sat unsent.
        assert_eq!(
            plan_input("busy", "", &text("a\nb")).unwrap(),
            vec![WriteStep::Bytes(b"\x1b[200~a\nb\x1b[201~".to_vec()), WriteStep::Wait(PASTE_SETTLE_BUSY), WriteStep::Bytes(b"\r".to_vec())],
        );
        assert!(PASTE_SETTLE_BUSY > PASTE_SETTLE, "busy must wait strictly longer, not just a different number");
    }

    #[test]
    fn plan_text_goes_through_when_claude_is_just_sitting_at_its_prompt() {
        // #308: Claude Code writes waiting_for = "input needed" after every turn, not just when it's really
        // asking for a permission or an answer. A reply must go through like at any other idle prompt, and
        // Approve / Deny / Interrupt must all still refuse (there is no real dialog to answer, nothing to stop)
        assert_eq!(plan_input("waiting", "input needed", &text("继续")).unwrap(), one("继续\r".as_bytes()));
        assert_eq!(plan_input("waiting", "input needed", &typed("/compact")).unwrap(), one(b"/compact"));
        assert_eq!(plan_input("waiting", "input needed", &InputKind::Approve).unwrap_err(), "not waiting for approval");
        assert_eq!(plan_input("waiting", "input needed", &InputKind::Deny).unwrap_err(), "not waiting for approval");
        assert_eq!(plan_input("waiting", "input needed", &InputKind::Interrupt).unwrap_err(), "not busy");
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
            assert_eq!(plan_input(state, waiting_for, &typed("/compact")).unwrap(), one(b"/compact"), "{state}");
        }
        // Several lines go in as one paste, still without Enter — and so without a settling wait either, nothing
        // follows the paste that could be swallowed
        assert_eq!(plan_input("idle", "", &typed("a\nb")).unwrap(), one(b"\x1b[200~a\nb\x1b[201~"));
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
        assert_eq!(plan_input("busy", "", &typed(" ")).unwrap(), one(b" "));
    }

    #[test]
    fn type_request_is_parsed() {
        let b = Fake { write_on: true, ..fake() };
        assert_eq!(ask(&b, r#"{"op":"input","session_id":"s1","kind":"type","text":"/model"}"#)["ok"], true);
        assert_eq!(b.written.lock().unwrap().last().unwrap().1, b"/model".to_vec());
    }

    #[test]
    fn plan_approve_and_deny_only_at_an_approval_prompt() {
        assert_eq!(plan_input("waiting", "approval", &InputKind::Approve).unwrap(), one(b"\r"));
        assert_eq!(plan_input("waiting", "", &InputKind::Approve).unwrap(), one(b"\r"));
        assert_eq!(plan_input("waiting", "approval", &InputKind::Deny).unwrap(), one(&[0x1b]));
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

    // Why this is real-time and not mocked: the bug it guards against is exactly a timing one (#299) — Enter sent
    // right behind a paste's end marker can be swallowed by Claude Code's TUI. Proving the fix means proving the two
    // writes are genuinely two separate `write` calls with the settling wait actually elapsed in between, not just
    // that `plan_input` planned it that way.
    #[test]
    fn a_multiline_reply_is_written_in_two_steps_with_the_settle_wait_elapsed_between_them() {
        let b = Fake { write_on: true, ..fake() };
        let before = std::time::Instant::now();
        let r = ask(&b, r#"{"op":"input","session_id":"s1","kind":"text","text":"line one\nline two"}"#);
        assert_eq!(r["ok"], true, "{r}");
        assert!(before.elapsed() >= PASTE_SETTLE, "the response must not come back before the wait is over");
        assert_eq!(
            *b.written.lock().unwrap(),
            vec![("s1".to_string(), b"\x1b[200~line one\nline two\x1b[201~".to_vec()), ("s1".to_string(), b"\r".to_vec())],
        );
    }

    // #312: same shape as the test above, but against a busy session. Proves the longer settle is a real elapsed
    // wait between the two writes, not just a bigger number `plan_input` happens to return.
    #[test]
    fn a_multiline_reply_to_a_busy_session_settles_longer_end_to_end() {
        let b = Fake { write_on: true, ..fake() }; // s1 is "busy" in fake()
        let before = std::time::Instant::now();
        let r = ask(&b, r#"{"op":"input","session_id":"s1","kind":"text","text":"line one\nline two"}"#);
        assert_eq!(r["ok"], true, "{r}");
        assert!(before.elapsed() >= PASTE_SETTLE_BUSY, "a busy session must really wait the longer settle, not the idle one");
        assert_eq!(
            *b.written.lock().unwrap(),
            vec![("s1".to_string(), b"\x1b[200~line one\nline two\x1b[201~".to_vec()), ("s1".to_string(), b"\r".to_vec())],
        );
    }

    #[test]
    fn text_submitted_needs_a_new_matching_user_message_not_just_a_bigger_total() {
        let page = |total: usize, last: Option<Value>| TranscriptPage { total, from: 0, items: last.into_iter().collect() };
        assert!(text_submitted(0, &page(1, Some(json!({"kind": "user", "text": "hi there"}))), "hi there"), "new, matches, counts as submitted");
        assert!(text_submitted(0, &page(1, Some(json!({"kind": "user", "text": "  hi there  "}))), "hi there"), "whitespace around it doesn't matter");
        assert!(!text_submitted(1, &page(1, Some(json!({"kind": "user", "text": "hi there"}))), "hi there"), "total didn't grow: this is the same old message, not a new one");
        assert!(!text_submitted(0, &page(1, Some(json!({"kind": "assistant", "text": "hi there"}))), "hi there"), "wrong kind");
        assert!(!text_submitted(0, &page(1, Some(json!({"kind": "user", "text": "something else"}))), "hi there"), "a new message, but not ours");
        assert!(!text_submitted(0, &page(0, None), "hi there"), "nothing there at all");
    }

    /// #334 复现：长文本会被 Claude Code 包成 `<pasted_content>` 块写进转录，转录里的 text 不等于发送的原文
    #[test]
    fn text_submitted_sees_through_a_pasted_block_wrapper() {
        let page = |items: Vec<Value>| TranscriptPage { total: items.len(), from: 0, items };
        let sent = "【派工】第一行\n第二行，中文\n第三行";
        let wrapped = format!("<pasted_content id=\"b80e\">\n{sent}\n</pasted_content>");
        assert!(text_submitted(0, &page(vec![json!({"kind": "user", "text": wrapped})]), sent), "粘贴块包裹的原文应判为已提交");
    }

    /// #334 复现：会话忙时，用户消息之后很快会跟着 assistant 的条目；确认不能只看最后一条
    #[test]
    fn text_submitted_finds_the_message_even_when_a_reply_came_after_it() {
        let page = |items: Vec<Value>| TranscriptPage { total: items.len(), from: 0, items };
        let items = vec![json!({"kind": "user", "text": "好"}), json!({"kind": "assistant", "text": "收到"})];
        assert!(text_submitted(0, &page(items), "好"), "用户消息后面跟了 assistant，仍然算已提交");
    }

    /// #334: 转录被重写（from 不等于调用方给的起点）时，无法分辨哪些是新消息，不能把旧消息当成这次的提交
    #[test]
    fn text_submitted_refuses_a_rewritten_transcript() {
        let rewritten = TranscriptPage { total: 6, from: 0, items: vec![json!({"kind": "user", "text": "好"})] };
        assert!(!text_submitted(5, &rewritten, "好"), "总数涨了，但 from 对不上（文件被重写过），不能算");
    }

    /// #D-2161: 复现——`write` 把字节送进 PTY 这件事本身就回 ok，不代表 Claude Code 真的把它当一次提交；这组用
    /// `submit_text_and_confirm` 直接测（注入极短的轮询参数，不用真等 150ms × 8），覆盖三种结果
    #[test]
    fn submit_text_and_confirm_covers_first_try_retry_and_giving_up() {
        let tiny = (Duration::from_millis(1), 3);
        // 第一次就提交：不用补那次重试回车
        let b = Fake { sessions: vec![info("sub", "idle")], submits_after_write_n: Mutex::new(Some(1)), ..fake() };
        let steps = plan_input("idle", "", &InputKind::Text("hi there".into())).unwrap();
        assert!(matches!(submit_text_and_confirm(&b, "sub", steps, "hi there", tiny.0, tiny.1), SubmitOutcome::Submitted));
        assert_eq!(b.written.lock().unwrap().len(), 1, "一次写入就确认了，没有补发回车");

        // 第一次的回车被吞了（transcript 一直没冒出来），补一次回车后才确认
        let b = Fake { sessions: vec![info("sub", "idle")], submits_after_write_n: Mutex::new(Some(2)), ..fake() };
        let steps = plan_input("idle", "", &InputKind::Text("hi there".into())).unwrap();
        assert!(matches!(submit_text_and_confirm(&b, "sub", steps, "hi there", tiny.0, tiny.1), SubmitOutcome::Submitted));
        assert_eq!(*b.written.lock().unwrap(), vec![("sub".to_string(), b"hi there\r".to_vec()), ("sub".to_string(), b"\r".to_vec())], "补发的是单独一个回车，不是整句话重打一遍");

        // 两次都没能确认：老实报没提交，不是假的 ok
        let b = Fake { sessions: vec![info("sub", "idle")], submits_after_write_n: Mutex::new(None), ..fake() };
        let steps = plan_input("idle", "", &InputKind::Text("hi there".into())).unwrap();
        assert!(matches!(submit_text_and_confirm(&b, "sub", steps, "hi there", tiny.0, tiny.1), SubmitOutcome::NotSubmitted));
    }

    /// 走完整的 `handle_line` 路径，确认 `reason:"not_submitted"` 这个额外字段真的接上了，不只是在帮手函数里对
    #[test]
    fn input_text_reports_a_structured_reason_when_it_never_confirms() {
        let b = Fake { write_on: true, sessions: vec![info("sub", "idle")], submits_after_write_n: Mutex::new(None), ..fake() };
        let r = ask(&b, r#"{"op":"input","session_id":"sub","kind":"text","text":"hi there"}"#);
        assert_eq!((r["ok"].as_bool(), r["reason"].as_str()), (Some(false), Some("not_submitted")));
        assert!(r["error"].as_str().is_some_and(|e| !e.is_empty()));
        // approve / deny / interrupt / type 不走这条确认逻辑，没有理由因为同一个假后端的 transcript 一直是空的而失败
        let b2 = Fake { write_on: true, sessions: vec![info("sub", "waiting")], submits_after_write_n: Mutex::new(None), ..fake() };
        assert_eq!(ask(&b2, r#"{"op":"input","session_id":"sub","kind":"approve"}"#)["ok"], true);
    }

    /// 回归：busy 会话追加文字也不走确认逻辑（第一版把所有 text 都套上确认流程，误伤了这个本来就正常的场景，
    /// 靠这条测试和下面两条既有的 multiline 测试一起发现、补上）。用"sub"这个 transcript 永远不会"冒出"新消息
    /// 的假后端：如果误判成走了确认流程，这里一定会失败或多补一次裸回车
    #[test]
    fn input_text_to_a_busy_session_skips_the_confirmation_entirely() {
        let b = Fake { write_on: true, sessions: vec![info("sub", "busy")], submits_after_write_n: Mutex::new(None), ..fake() };
        let r = ask(&b, r#"{"op":"input","session_id":"sub","kind":"text","text":"hi there"}"#);
        assert_eq!(r["ok"], true, "{r}");
        // A single-line text with no Enter settle wait is the text plus `\r` written in one step (`enter_after`
        // with `pasted: false`); the point of this assertion is that there is no *extra* bare `\r` tacked on
        // afterward by the confirmation path's retry, which is what would happen if busy wrongly went through it
        assert_eq!(*b.written.lock().unwrap(), vec![("sub".to_string(), b"hi there\r".to_vec())]);
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
        assert_eq!(plan_input("busy", "", &InputKind::Interrupt).unwrap(), one(&[0x1b]));
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
    fn open_text_is_validated_like_a_reply() {
        assert_eq!(validate_open_text("hi there").unwrap(), "hi there");
        assert_eq!(validate_open_text("line one\r\nline two").unwrap(), "line one\nline two");
        assert_eq!(validate_open_text("").unwrap_err(), "empty text");
        assert_eq!(validate_open_text("   ").unwrap_err(), "empty text");
        assert_eq!(validate_open_text(&"a".repeat(MAX_INPUT_CHARS + 1)).unwrap_err(), "text too long");
        assert_eq!(validate_open_text("hi\x07there").unwrap_err(), "control characters");
        // Newline and tab are not control characters here: a first message may have several lines
        assert!(validate_open_text("line one\nline two\tindented").is_ok());
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
        // keys / open / resume / archive / interrupt are writes
        for req in [
            r#"{"op":"keys","session_id":"s1","keys":["enter"]}"#,
            r#"{"op":"open","cwd":"/","tool":"claude"}"#,
            r#"{"op":"resume","session_id":"s3"}"#,
            r#"{"op":"archive","session_id":"s3"}"#,
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
    fn keys_interrupt_open_resume_and_archive_with_write_on() {
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
        // open with a first message: it is validated and passed through so the session writes right away
        let r2 = ask(&b, &json!({"op": "open", "cwd": dir, "tool": "claude", "text": "hello there"}).to_string());
        assert_eq!(r2["ok"], true, "{r2}");
        assert_eq!(
            *b.opened.lock().unwrap(),
            vec![
                (dir.to_string_lossy().to_string(), "codex".to_string(), None, false, None),
                (dir.to_string_lossy().to_string(), "claude".to_string(), Some("hello there".to_string()), false, None),
            ],
        );
        assert_eq!(ask(&b, &json!({"op": "open", "cwd": dir, "tool": "claude", "text": ""}).to_string())["error"], "empty text");
        assert_eq!(ask(&b, &json!({"op": "open", "cwd": dir, "tool": "claude", "text": "a\x07b"}).to_string())["error"], "control characters");
        assert_eq!(ask(&b, r#"{"op":"open","cwd":"/no/such/dir/here","tool":"claude"}"#)["error"], "no such directory");
        assert_eq!(ask(&b, r#"{"op":"open","cwd":"/","tool":"zsh"}"#)["error"], "unknown tool");
        // resume: an ended session actually resumes; an already-running one is a no-op reported as such (#D-2044),
        // not an error -- the other side may not know it is already running when it asks
        assert_eq!(ask(&b, r#"{"op":"resume","session_id":"s3"}"#)["ok"], true);
        let r = ask(&b, r#"{"op":"resume","session_id":"s1"}"#);
        assert_eq!((r["ok"].as_bool(), r["already_running"].as_bool()), (Some(true), Some(true)));
        assert_eq!(ask(&b, r#"{"op":"resume","session_id":"nope"}"#)["error"], "session not found");
        assert_eq!(*b.resumed.lock().unwrap(), vec![("s3".to_string(), true, None)], "the already-running one must not call resume again; allow_new_tab omitted defaults to true");
        // archive: only a session that is not running
        assert_eq!(ask(&b, r#"{"op":"archive","session_id":"s3"}"#)["ok"], true);
        assert_eq!(ask(&b, r#"{"op":"archive","session_id":"s1"}"#)["error"], "session running");
        assert_eq!(ask(&b, r#"{"op":"archive","session_id":"nope"}"#)["error"], "session not found");
        assert_eq!(ask(&b, r#"{"op":"archive"}"#)["error"], "session_id required");
        assert_eq!(*b.archived.lock().unwrap(), vec!["s3".to_string()]);
    }

    /// #D-2078: `sessions`'s list is capped and can come back short under load (file-descriptor pressure was the
    /// real trigger tonight); resume/archive checked existence against that list and wrongly reported "session not
    /// found" for a session that genuinely exists but the list happened to leave out.
    #[test]
    fn resume_and_archive_work_on_a_session_the_capped_list_left_out() {
        let b = Fake { write_on: true, existing_but_unlisted: vec!["s9".into()], ..fake() };
        assert_eq!(ask(&b, r#"{"op":"sessions"}"#)["sessions"].as_array().unwrap().iter().any(|s| s["session_id"] == "s9"), false, "s9 is not in the list");
        assert_eq!(ask(&b, r#"{"op":"resume","session_id":"s9"}"#)["ok"], true);
        assert_eq!(*b.resumed.lock().unwrap(), vec![("s9".to_string(), true, None)]);
        assert_eq!(ask(&b, r#"{"op":"archive","session_id":"s9"}"#)["ok"], true);
        assert_eq!(*b.archived.lock().unwrap(), vec!["s9".to_string()]);
    }

    /// #D-2299/#D-2300: `stop`/`close` need write enabled, a real `session_id`, and a session that actually
    /// exists on disk -- the same basic gate every other write op goes through.
    #[test]
    fn stop_and_close_validate_like_every_other_write_op() {
        let b = Fake { write_on: false, ..fake() };
        assert_eq!(ask(&b, r#"{"op":"stop","session_id":"s1"}"#)["error"], "write disabled");
        assert_eq!(ask(&b, r#"{"op":"close","session_id":"s1"}"#)["error"], "write disabled");

        let b = Fake { write_on: true, ..fake() };
        assert_eq!(ask(&b, r#"{"op":"stop"}"#)["error"], "session_id required");
        assert_eq!(ask(&b, r#"{"op":"stop","session_id":""}"#)["error"], "session_id required");
        assert_eq!(ask(&b, r#"{"op":"close"}"#)["error"], "session_id required");
        assert_eq!(ask(&b, r#"{"op":"stop","session_id":"nope"}"#)["error"], "session not found");
        assert_eq!(ask(&b, r#"{"op":"close","session_id":"nope"}"#)["error"], "session not found");
        assert!(b.stopped.lock().unwrap().is_empty());
        assert!(b.closed.lock().unwrap().is_empty());
    }

    /// #D-2299: `force` is `false` when omitted (the safe default -- a caller that has never heard of it must not
    /// start silently killing busy sessions), reaches the backend exactly as given, and a refusal because the
    /// session needed it surfaces as the structured `reason:"needs_force"`, the same shape as `no_tab` /
    /// `not_trusted_root`.
    #[test]
    fn stop_passes_through_force_and_reports_needs_force() {
        let b = Fake { write_on: true, ..fake() };
        let r = ask(&b, r#"{"op":"stop","session_id":"s1"}"#);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(*b.stopped.lock().unwrap(), vec![("s1".to_string(), false)], "omitted force defaults to false");

        let r = ask(&b, r#"{"op":"stop","session_id":"s1","force":true}"#);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(b.stopped.lock().unwrap().last(), Some(&("s1".to_string(), true)));

        *b.stop_result.lock().unwrap() = Err(OpenError::needs_force("s1 is busy; pass force:true to stop it anyway"));
        let r = ask(&b, r#"{"op":"stop","session_id":"s1"}"#);
        assert_eq!((r["ok"].as_bool(), r["reason"].as_str()), (Some(false), Some("needs_force")));
        assert!(r["error"].as_str().is_some_and(|e| !e.is_empty()));
        assert!(r.get("capacity_limit").is_none(), "needs_force must not also look like a capacity refusal");
    }

    /// #D-2300: same contract as `stop`, independently, since `close` is its own `Backend` method with its own
    /// default implementation and its own JSON parsing in `handle_line`.
    #[test]
    fn close_passes_through_force_and_reports_needs_force() {
        let b = Fake { write_on: true, ..fake() };
        let r = ask(&b, r#"{"op":"close","session_id":"s1","force":true}"#);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(*b.closed.lock().unwrap(), vec![("s1".to_string(), true)]);

        *b.close_result.lock().unwrap() = Err(OpenError::needs_force("s1 is waiting for approval; pass force:true to close it anyway"));
        let r = ask(&b, r#"{"op":"close","session_id":"s1"}"#);
        assert_eq!((r["ok"].as_bool(), r["reason"].as_str()), (Some(false), Some("needs_force")));
    }

    /// #D-2300: `sessions`' `tab_state` lets a caller tell "has a tab, running" apart from "has a tab, stopped"
    /// apart from "no tab at all" without guessing from `has_tab` alone.
    #[test]
    fn sessions_reports_tab_state() {
        let mut open = info("s1", "busy");
        open.tab_state = "open".into();
        let mut stopped = info("s2", "ended");
        stopped.tab_state = "stopped".into();
        let none = info("s3", "ended"); // tab_state defaults to "none" from `info()`
        let b = Fake { sessions: vec![open, stopped, none], write_on: true, ..fake() };
        let sessions = ask(&b, r#"{"op":"sessions"}"#)["sessions"].clone();
        let state_of = |id: &str| sessions.as_array().unwrap().iter().find(|s| s["session_id"] == id).unwrap()["tab_state"].clone();
        assert_eq!(state_of("s1"), "open");
        assert_eq!(state_of("s2"), "stopped");
        assert_eq!(state_of("s3"), "none");
    }

    /// #D-2099 (总管's own automation asked for this): `open` / `resume` refused by the capacity cap carry a
    /// structured `capacity_limit` field, not just the human-readable message, so a caller can switch on which
    /// threshold tripped. Every other refusal (missing directory, unknown tool, session not found, ...) must not
    /// carry the field at all -- it is specific to the capacity cap, not a general-purpose error code.
    #[test]
    fn open_and_resume_report_which_capacity_threshold_tripped() {
        let b = Fake { write_on: true, ..fake() };
        *b.capacity_block.lock().unwrap() = Some(CapacityLimitKind::LowMemory);
        let r = ask(&b, r#"{"op":"open","cwd":"/","tool":"claude"}"#);
        assert_eq!((r["ok"].as_bool(), r["capacity_limit"].as_str()), (Some(false), Some("low_memory")));
        assert!(r["error"].as_str().is_some_and(|e| !e.is_empty()));
        let r = ask(&b, r#"{"op":"resume","session_id":"s3"}"#);
        assert_eq!((r["ok"].as_bool(), r["capacity_limit"].as_str()), (Some(false), Some("low_memory")));
        *b.capacity_block.lock().unwrap() = Some(CapacityLimitKind::Count);
        assert_eq!(ask(&b, r#"{"op":"open","cwd":"/","tool":"claude"}"#)["capacity_limit"], "count");
        *b.capacity_block.lock().unwrap() = Some(CapacityLimitKind::HighLoad);
        assert_eq!(ask(&b, r#"{"op":"open","cwd":"/","tool":"claude"}"#)["capacity_limit"], "high_load");
        // A refusal that is not the capacity cap must not carry the field at all
        *b.capacity_block.lock().unwrap() = None;
        assert!(ask(&b, r#"{"op":"open","cwd":"/no/such/dir","tool":"claude"}"#).get("capacity_limit").is_none());
        assert!(ask(&b, r#"{"op":"resume","session_id":"nope"}"#).get("capacity_limit").is_none());
    }

    /// #D-2138/#D-2140: the tab-reuse-vs-open-new policy belongs to the caller (总管's loop), not makit.
    /// `allow_new_tab` must reach the backend exactly as given (default `true` when omitted, so an old caller
    /// that has never heard of this keeps today's behaviour), and the reply must say which branch was actually
    /// taken, or a structured `reason` when refused for lack of a tab to reuse.
    #[test]
    fn resume_passes_through_allow_new_tab_and_reports_what_happened() {
        let b = Fake { write_on: true, ..fake() };

        *b.resume_result.lock().unwrap() = Ok(ResumeAction::ReusedTab);
        let r = ask(&b, r#"{"op":"resume","session_id":"s3"}"#);
        assert_eq!((r["ok"].as_bool(), r["action"].as_str()), (Some(true), Some("reused_tab")));
        assert_eq!(b.resumed.lock().unwrap().last(), Some(&("s3".to_string(), true, None)), "omitted allow_new_tab defaults to true");

        *b.resume_result.lock().unwrap() = Ok(ResumeAction::OpenedNewTab);
        let r = ask(&b, r#"{"op":"resume","session_id":"s3","allow_new_tab":true}"#);
        assert_eq!(r["action"], "opened_new_tab");
        assert_eq!(b.resumed.lock().unwrap().last(), Some(&("s3".to_string(), true, None)));

        // 总管 defaults to allow_new_tab:false: a session with nothing to reuse comes back as a structured
        // refusal, and makit must not have opened anything (nothing new in `resumed`'s outcome either -- the
        // fake's `resume_result` here simulates the backend itself refusing, same as the real one would)
        *b.resume_result.lock().unwrap() = Err(OpenError::no_tab("no tab for this session and allow_new_tab is false"));
        let r = ask(&b, r#"{"op":"resume","session_id":"s3","allow_new_tab":false}"#);
        assert_eq!((r["ok"].as_bool(), r["reason"].as_str()), (Some(false), Some("no_tab")));
        assert!(r.get("capacity_limit").is_none(), "no_tab must not also look like a capacity refusal");
        assert_eq!(b.resumed.lock().unwrap().last(), Some(&("s3".to_string(), false, None)), "the false the caller sent must reach the backend unchanged");
    }

    /// #D-2168: `trust` and `permission_mode` are claude-only, and `permission_mode`'s value must be one of
    /// claude's own real `--permission-mode` choices -- both refused by `handle_line` itself, before the
    /// `Backend` ever sees a bad one.
    #[test]
    fn open_rejects_trust_and_permission_mode_for_non_claude_tools_and_unknown_modes() {
        let b = Fake { write_on: true, ..fake() };
        let dir = std::env::temp_dir();
        let dir_s = dir.to_string_lossy();

        let r = ask(&b, &json!({"op": "open", "cwd": &*dir_s, "tool": "codex", "trust": true}).to_string());
        assert_eq!((r["ok"].as_bool(), r["error"].as_str().is_some()), (Some(false), true), "trust is claude-only");
        let r = ask(&b, &json!({"op": "open", "cwd": &*dir_s, "tool": "codex", "permission_mode": "acceptEdits"}).to_string());
        assert_eq!(r["ok"], false, "permission_mode is claude-only");
        let r = ask(&b, &json!({"op": "open", "cwd": &*dir_s, "tool": "claude", "permission_mode": "default"}).to_string());
        assert_eq!(r["ok"], false, "\"default\" sounds plausible but is not one of claude's real choices");
        assert!(b.opened.lock().unwrap().is_empty(), "none of the above should have ever reached the backend");

        let r = ask(&b, &json!({"op": "open", "cwd": &*dir_s, "tool": "claude", "trust": true, "permission_mode": "acceptEdits"}).to_string());
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(b.opened.lock().unwrap().last(), Some(&(dir_s.to_string(), "claude".to_string(), None, true, Some("acceptEdits".to_string()))));
    }

    /// #D-2168: when the `Backend` itself refuses `trust` (the directory is not under any configured auto-trust
    /// root), that must surface as the structured `reason:"not_trusted_root"`, the same shape as `no_tab`.
    #[test]
    fn open_surfaces_not_trusted_root_as_a_structured_reason() {
        let b = Fake { write_on: true, ..fake() };
        *b.open_result.lock().unwrap() = Err(OpenError::not_trusted_root("cwd is not under any configured auto-trust root"));
        let dir = std::env::temp_dir();
        let r = ask(&b, &json!({"op": "open", "cwd": dir.to_string_lossy(), "tool": "claude", "trust": true}).to_string());
        assert_eq!((r["ok"].as_bool(), r["reason"].as_str()), (Some(false), Some("not_trusted_root")));
        assert!(r.get("capacity_limit").is_none());
    }

    /// #D-2168: `permission_mode` on `resume` cannot be tool-checked at this layer (only `sessions`'s list
    /// carries `tool`), so `handle_line` only validates the *value*, and passes a known one straight through.
    #[test]
    fn resume_validates_and_passes_through_permission_mode() {
        let b = Fake { write_on: true, ..fake() };
        let r = ask(&b, r#"{"op":"resume","session_id":"s3","permission_mode":"not-a-real-mode"}"#);
        assert_eq!(r["ok"], false);
        assert!(b.resumed.lock().unwrap().is_empty());

        *b.resume_result.lock().unwrap() = Ok(ResumeAction::OpenedNewTab);
        let r = ask(&b, r#"{"op":"resume","session_id":"s3","permission_mode":"bypassPermissions"}"#);
        assert_eq!(r["ok"], true, "{r}");
        assert_eq!(b.resumed.lock().unwrap().last(), Some(&("s3".to_string(), true, Some("bypassPermissions".to_string()))));
    }

    #[test]
    fn session_cwds_reports_the_trail_with_first_and_last() {
        let b = fake();
        let r = ask(&b, r#"{"op":"session_cwds","session_id":"s1"}"#);
        assert_eq!(r["ok"], true);
        assert_eq!(r["cwds"], json!([{"cwd": "/a", "n": 2}, {"cwd": "/b", "n": 1}]));
        assert_eq!((r["first"].as_str(), r["last"].as_str()), (Some("/a"), Some("/a")));
        assert_eq!(ask(&b, r#"{"op":"session_cwds","session_id":"nope"}"#)["error"], "session not found");
        assert_eq!(ask(&b, r#"{"op":"session_cwds"}"#)["error"], "session_id required");
    }

    #[test]
    fn last_running_reports_the_on_disk_snapshot_as_is() {
        let b = Fake {
            last_running_snapshot: vec![
                crate::last_running::LastRunning { session_id: "s1".into(), name: "总管".into(), cwd: "/Users/me/p".into(), last_active: 100 },
            ],
            ..fake()
        };
        let r = ask(&b, r#"{"op":"last_running"}"#);
        assert_eq!(r["ok"], true);
        assert_eq!(r["sessions"], json!([{"session_id": "s1", "name": "总管", "cwd": "/Users/me/p", "last_active": 100}]));
    }

    #[test]
    fn update_check_reports_current_state_and_a_newer_release_when_there_is_one() {
        let b = Fake {
            update_check_result: Ok(UpdateStatus { current: "0.2.19".into(), state: "available".into(), latest: Some("0.2.20".into()), notes: "fixes".into(), page_url: "https://example.invalid".into() }),
            ..fake()
        };
        let r = ask(&b, r#"{"op":"update","cmd":"check"}"#);
        assert_eq!(r["ok"], true);
        assert_eq!((r["current"].as_str(), r["state"].as_str(), r["latest"].as_str()), (Some("0.2.19"), Some("available"), Some("0.2.20")));
        assert_eq!(r["notes"], "fixes");

        let unknown = ask(&fake(), r#"{"op":"update","cmd":"nope"}"#);
        assert_eq!(unknown["error"], "cmd must be \"check\" or \"install\"");
    }

    #[test]
    fn update_install_needs_write_enabled_and_validates_after_restart() {
        let off = fake();
        assert_eq!(ask(&off, r#"{"op":"update","cmd":"install"}"#)["error"], "write disabled");

        let on = Fake { write_on: true, ..fake() };
        let r = ask(&on, r#"{"op":"update","cmd":"install"}"#);
        assert_eq!(r["ok"], true);
        assert_eq!(on.update_installs.lock().unwrap().as_slice(), [None]);

        let r = ask(&on, r#"{"op":"update","cmd":"install","after_restart":{"session_id":"s1","text":"继续"}}"#);
        assert_eq!(r["ok"], true);
        assert_eq!(on.update_installs.lock().unwrap().last().unwrap(), &Some(AfterRestart { session_id: "s1".into(), text: "继续".into() }));

        // half the shape given: refused rather than silently dropping the continuation
        let r = ask(&on, r#"{"op":"update","cmd":"install","after_restart":{"session_id":"s1"}}"#);
        assert_eq!(r["error"], "after_restart needs non-empty session_id and text");

        let failing = Fake { write_on: true, update_install_result: Err("no update available".into()), ..fake() };
        assert_eq!(ask(&failing, r#"{"op":"update","cmd":"install"}"#)["error"], "no update available");
    }

    #[test]
    fn hello_lists_the_ops_so_the_other_side_can_tell_old_versions() {
        let ops = ask(&fake(), r#"{"op":"hello"}"#)["ops"].clone();
        // "type": input kind `type` (terminal keyboard) and replies accepted while the session works
        for op in ["sessions", "transcript", "input", "screen", "keys", "open", "resume", "archive", "stop", "close", "subscribe", "type", "session_cwds", "last_running", "update"] {
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
