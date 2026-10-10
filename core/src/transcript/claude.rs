//! Line-by-line parsing of claude's jsonl (rules in TRD section 11.4).

use crate::ts;
use serde_json::Value;

use super::model::{DividerKind, Item, ItemKind, Level, ToolResult};
use super::state::{Rec, State};

/// Metadata / status records without a uuid: recognized, but not conversation content
const IGNORED: &[&str] = &[
    "ai-title",
    "custom-title",
    "agent-name",
    "mode",
    "permission-mode",
    "last-prompt",
    "queue-operation",
    "file-history-snapshot",
    "file-history-delta",
    "atis-latch",
    "cost-state",
    "frame-link",
    "bridge-session",
    "artifact-autoreact-ledger",
    "artifact-comment-monitor",
    "summary",
];

pub(super) fn feed(st: &mut State, v: &Value) -> Vec<usize> {
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    if IGNORED.contains(&ty) {
        return vec![];
    }
    if !matches!(ty, "user" | "assistant" | "system" | "attachment" | "progress") {
        st.count_unknown(if ty.is_empty() { "<no type>".into() } else { ty.into() });
        return vec![];
    }
    // Subagent sidechain: the main conversation keeps only that tool call and its final result
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return vec![];
    }
    let Some(uuid) = v.get("uuid").and_then(Value::as_str) else { return vec![] };
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    let mut logical_parent = s("logicalParentUuid");
    // A compaction boundary's logicalParentUuid sometimes points at a record that only appears after the boundary (its ancestor chain loops back to the boundary -> a cycle, and the whole pre-compaction conversation is lost).
    // In that case, re-attach to the last conversation record before it in the file; only done for compaction boundaries, since a user root record's rewind is legitimate
    let is_boundary = ty == "system" && matches!(s("subtype").as_deref(), Some("compact_boundary" | "microcompact_boundary"));
    if is_boundary && s("parentUuid").is_none() && !logical_parent.as_ref().is_some_and(|l| st.recs.contains_key(l)) {
        logical_parent = st.leaf.clone();
    }
    let msg = if ty == "assistant" { v.pointer("/message/id").and_then(Value::as_str).map(String::from) } else { None };
    st.insert_rec(uuid, Rec { parent: s("parentUuid"), logical_parent, conv: matches!(ty, "user" | "assistant"), msg, seq: 0 });
    // A user record that is purely tool results does not move the leaf: with parallel calls, the result that arrives first hangs off an earlier branch, and moving the leaf back would
    // temporarily kick the later calls out of the visible chain (during live reading the reader then reports reset and the view is rebuilt entirely, i.e. a visible flash). Results attach back to calls via tool_use_id and do not depend on the chain
    if matches!(ty, "user" | "assistant") && !only_tool_results(v) {
        st.leaf = Some(uuid.to_string());
    }
    let time = s("timestamp");
    let flag = |k: &str| v.get(k).and_then(Value::as_bool) == Some(true);
    let mut out = Items { st, uuid, time, n: 0, updated: vec![] };
    match ty {
        "user" => {
            if !flag("isMeta") && !flag("isCompactSummary") {
                user(&mut out, v);
            }
        }
        "assistant" => assistant(&mut out, v),
        "system" => system(&mut out, v),
        _ => {} // attachment / progress: not conversation content
    }
    out.updated
}

/// Small helper that appends an Item to `all`: the n-th Item from the same record has the id `uuid#n`
struct Items<'a> {
    st: &'a mut State,
    uuid: &'a str,
    time: Option<String>,
    n: usize,
    updated: Vec<usize>,
}

impl Items<'_> {
    fn add(&mut self, kind: ItemKind) -> usize {
        let id = if self.n == 0 { self.uuid.to_string() } else { format!("{}#{}", self.uuid, self.n) };
        self.n += 1;
        self.st.push(Some(self.uuid), Item { id, time: self.time.clone(), kind })
    }
}

fn user(o: &mut Items, v: &Value) {
    let content = v.pointer("/message/content");
    match content {
        Some(Value::String(s)) => user_text(o, s),
        Some(Value::Array(blocks)) => {
            for b in blocks {
                match b.get("type").and_then(Value::as_str) {
                    Some("text") => user_text(o, b.get("text").and_then(Value::as_str).unwrap_or("")),
                    Some("image") => {
                        let src = b.get("source");
                        let media = src.and_then(|s| s.get("media_type")).and_then(Value::as_str).unwrap_or("image").to_string();
                        let data = src.and_then(|s| s.get("data")).and_then(Value::as_str).unwrap_or("");
                        o.add(ItemKind::Image { media_type: media, bytes: base64_decoded_len(data) });
                    }
                    Some("tool_result") => tool_result(o, b),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn user_text(o: &mut Items, s: &str) {
    let t = s.trim();
    if t.is_empty() {
        return;
    }
    if t.starts_with("<command-name>") {
        let name = tag(t, "command-name").unwrap_or_default();
        let args = tag(t, "command-args").unwrap_or_default();
        o.add(ItemKind::Command { name: name.trim().to_string(), args: args.trim().to_string() });
    } else if t.starts_with("<local-command-stdout>") || t.starts_with("<local-command-stderr>") {
        let out = tag(t, "local-command-stdout").or_else(|| tag(t, "local-command-stderr")).unwrap_or_default();
        if !out.trim().is_empty() {
            o.add(ItemKind::LocalOutput(out));
        }
    } else if t.starts_with("<bash-input>") {
        o.add(ItemKind::BashInput(tag(t, "bash-input").unwrap_or_default()));
    } else if t.starts_with("<bash-stdout>") {
        let mut out = tag(t, "bash-stdout").unwrap_or_default();
        if let Some(err) = tag(t, "bash-stderr").filter(|e| !e.trim().is_empty()) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&err);
        }
        if !out.trim().is_empty() {
            o.add(ItemKind::LocalOutput(out));
        }
    } else if t.starts_with("<task-notification>") {
        if let Some(sum) = tag(t, "summary") {
            o.add(ItemKind::Notice { level: Level::Info, text: sum.trim().to_string() });
        }
    } else if t.starts_with("<system-reminder>") || t.starts_with("<local-command-caveat>") {
        // For the model, not said by the user
    } else if t.starts_with("[Request interrupted") {
        o.add(ItemKind::Notice { level: Level::Warn, text: ts!("core.transcript.interrupted").into() });
    } else {
        o.add(ItemKind::User(s.to_string()));
    }
}

fn tool_result(o: &mut Items, b: &Value) {
    let Some(id) = b.get("tool_use_id").and_then(Value::as_str) else { return };
    let is_error = b.get("is_error").and_then(Value::as_bool) == Some(true);
    let (text, images) = match b.get("content") {
        Some(Value::String(s)) => (s.clone(), 0),
        Some(Value::Array(parts)) => {
            let mut texts = Vec::new();
            let mut images = 0;
            for p in parts {
                match p.get("type").and_then(Value::as_str) {
                    Some("text") => texts.push(p.get("text").and_then(Value::as_str).unwrap_or("").to_string()),
                    Some("image") => images += 1,
                    _ => {} // tool_reference etc.: no displayable text
                }
            }
            (texts.join("\n"), images)
        }
        _ => (String::new(), 0),
    };
    if let Some(ix) = o.st.attach_result(id, ToolResult::new(&text, is_error, images)) {
        o.updated.push(ix);
    }
}

fn assistant(o: &mut Items, v: &Value) {
    let Some(blocks) = v.pointer("/message/content").and_then(Value::as_array) else { return };
    for b in blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("thinking") => {
                // Many thinking blocks carry only a signature and no text: with nothing displayable, no Item is emitted
                let t = b.get("thinking").and_then(Value::as_str).unwrap_or("");
                if !t.trim().is_empty() {
                    o.add(ItemKind::Thinking(t.to_string()));
                }
            }
            Some("text") => {
                let t = b.get("text").and_then(Value::as_str).unwrap_or("");
                if !t.trim().is_empty() {
                    o.add(ItemKind::Assistant(t.to_string()));
                }
            }
            Some("tool_use") => {
                let call_id = b.get("id").and_then(Value::as_str).unwrap_or("").to_string();
                let name = b.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                let input = b.get("input").cloned().unwrap_or(Value::Null);
                let ix = o.add(ItemKind::ToolCall { call_id: call_id.clone(), name, input, result: None });
                if !call_id.is_empty() {
                    o.st.calls.insert(call_id, ix);
                }
            }
            _ => {}
        }
    }
}

fn system(o: &mut Items, v: &Value) {
    let sub = v.get("subtype").and_then(Value::as_str).unwrap_or("");
    let content = v.get("content").and_then(Value::as_str).unwrap_or("").trim().to_string();
    match sub {
        "turn_duration" => {
            if let Some(ms) = v.get("durationMs").and_then(Value::as_u64) {
                o.add(ItemKind::TurnDuration(ms));
            }
        }
        "away_summary" => {
            if !content.is_empty() {
                o.add(ItemKind::Notice { level: Level::Info, text: format!("recap: {content}") });
            }
        }
        "compact_boundary" | "microcompact_boundary" => {
            o.add(ItemKind::Divider(DividerKind::Compacted));
        }
        "api_error" => {
            let n = v.get("retryAttempt").and_then(Value::as_u64).unwrap_or(0);
            let max = v.get("maxRetries").and_then(Value::as_u64).unwrap_or(0);
            o.add(ItemKind::Notice { level: Level::Warn, text: ts!("core.transcript.api_retry", n = n, max = max) });
        }
        "informational" => {
            if !content.is_empty() {
                o.add(ItemKind::Notice { level: Level::Info, text: content });
            }
        }
        // The same command already appears in the user record; the hook summary is of little value to a person
        "local_command" | "stop_hook_summary" => {}
        other => o.st.count_unknown(format!("system/{other}")),
    }
}

/// The content inside `<tag>content</tag>`
fn tag(s: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&close)? + a;
    Some(s[a..b].to_string())
}

/// Number of bytes after base64 decoding (without actually decoding)
fn base64_decoded_len(data: &str) -> usize {
    let n = data.trim_end_matches('=').len();
    n * 3 / 4
}

/// Whether the message content consists solely of tool_result blocks (no user text / images at all)
fn only_tool_results(v: &Value) -> bool {
    let blocks = v.pointer("/message/content").and_then(Value::as_array);
    blocks.is_some_and(|b| !b.is_empty() && b.iter().all(|x| x.get("type").and_then(Value::as_str) == Some("tool_result")))
}
