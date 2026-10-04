//! Line-by-line parsing of codex's rollout (rules in TRD section 11.4).
//!
//! In 0.40 files each message appears **twice**: as `response_item` (message) and as `event_msg.item_completed` (UserMessage /
//! AgentMessage). The text is identical each time (verified on all 17 files that contain a conversation); the order is sometimes "R then E" (user)
//! and sometimes "E first, then R after a reasoning item" (assistant). So: the copy that arrives first emits the Item, and the later copy with the same role and text is skipped as a duplicate;
//! the dedup record is kept only within the last few records (when a user really sends the same message twice in a row, the second one must not be swallowed).
//! If a later version has only one copy, the first to arrive is the only one and is still displayed.

use serde_json::Value;

use super::model::{Item, ItemKind, Level, ToolResult};
use super::state::State;

/// How many records the dedup record is kept for: R and E were measured to be at most 3 records apart
const DEDUP_WINDOW: usize = 6;

#[derive(Default)]
pub(super) struct Dedup {
    pending: Vec<Pending>,
}

struct Pending {
    user: bool,
    text: String,
    from_event: bool,
    seq: usize,
}

pub(super) fn feed(st: &mut State, v: &Value, seq: usize) -> Vec<usize> {
    let time = v.get("timestamp").and_then(Value::as_str).map(String::from);
    let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
    let p = v.get("payload").cloned().unwrap_or(Value::Null);
    let pty = p.get("type").and_then(Value::as_str).unwrap_or("");
    match (ty, pty) {
        ("session_meta" | "turn_context", _) => vec![],
        ("response_item", "message") => {
            let role = p.get("role").and_then(Value::as_str).unwrap_or("");
            let text = join_texts(p.get("content"));
            match role {
                // Injected context (environment, AGENTS.md, etc.) is not said by the user
                "user" if !text.trim_start().starts_with("<environment_context>") && !text.trim_start().starts_with("<user_instructions>") => {
                    message(st, true, text, false, seq, time)
                }
                "assistant" => message(st, false, text, false, seq, time),
                _ => {}
            }
            vec![]
        }
        ("response_item", "reasoning") => {
            // Measured: reasoning is encrypted and the summary is empty -> with no displayable text, no Item is emitted
            let text = p
                .get("summary")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"))
                .unwrap_or_default();
            if !text.trim().is_empty() {
                add(st, seq, time, ItemKind::Thinking(text));
            }
            vec![]
        }
        ("response_item", "function_call") => {
            let call_id = p.get("call_id").and_then(Value::as_str).unwrap_or("").to_string();
            let name = p.get("name").and_then(Value::as_str).unwrap_or("").to_string();
            let args = p.get("arguments").and_then(Value::as_str).unwrap_or("");
            let input = serde_json::from_str::<Value>(args).unwrap_or_else(|_| Value::String(args.to_string()));
            let ix = add(st, seq, time, ItemKind::ToolCall { call_id: call_id.clone(), name, input, result: None });
            if !call_id.is_empty() {
                st.calls.insert(call_id, ix);
            }
            vec![]
        }
        ("response_item", "function_call_output") => {
            let id = p.get("call_id").and_then(Value::as_str).unwrap_or("");
            let raw = p.get("output").and_then(Value::as_str).unwrap_or("");
            let (text, is_error) = match serde_json::from_str::<Value>(raw) {
                Ok(Value::Object(o)) if o.get("output").is_some_and(Value::is_string) => {
                    let code = o.get("metadata").and_then(|m| m.get("exit_code")).and_then(Value::as_i64).unwrap_or(0);
                    (o["output"].as_str().unwrap_or("").to_string(), code != 0)
                }
                _ => (raw.to_string(), false),
            };
            st.attach_result(id, ToolResult::new(&text, is_error, 0)).into_iter().collect()
        }
        ("event_msg", "user_message") => {
            let text = p.get("message").and_then(Value::as_str).unwrap_or("").to_string();
            message(st, true, text, true, seq, time);
            vec![]
        }
        ("event_msg", "item_completed") => {
            let item = p.get("item").cloned().unwrap_or(Value::Null);
            match item.get("type").and_then(Value::as_str) {
                Some("UserMessage") => message(st, true, join_texts(item.get("content")), true, seq, time),
                Some("AgentMessage") => message(st, false, join_texts(item.get("content")), true, seq, time),
                Some(other) => st.count_unknown(format!("codex/item_completed/{other}")),
                None => {}
            }
            vec![]
        }
        ("event_msg", "turn_aborted") => {
            add(st, seq, time, ItemKind::Notice { level: Level::Warn, text: "已中断".into() });
            vec![]
        }
        ("event_msg", "task_started" | "task_complete" | "token_count" | "thread_settings_applied") => vec![],
        ("event_msg", other) => {
            st.count_unknown(format!("codex/event_msg/{other}"));
            vec![]
        }
        ("response_item", other) => {
            st.count_unknown(format!("codex/response_item/{other}"));
            vec![]
        }
        (other, _) => {
            st.count_unknown(other.to_string());
            vec![]
        }
    }
}

fn add(st: &mut State, seq: usize, time: Option<String>, kind: ItemKind) -> usize {
    st.push(None, Item { id: format!("c{seq}"), time, kind })
}

/// One message: if the other source just emitted the same one -> skip; otherwise emit an Item and note "waiting for the other copy to dedup"
fn message(st: &mut State, user: bool, text: String, from_event: bool, seq: usize, time: Option<String>) {
    if text.trim().is_empty() {
        return;
    }
    let d = &mut st.codex;
    d.pending.retain(|p| seq.saturating_sub(p.seq) <= DEDUP_WINDOW);
    if let Some(i) = d.pending.iter().position(|p| p.user == user && p.text == text && p.from_event != from_event) {
        d.pending.remove(i);
        return;
    }
    d.pending.push(Pending { user, text: text.clone(), from_event, seq });
    add(st, seq, time, if user { ItemKind::User(text) } else { ItemKind::Assistant(text) });
}

/// `content` is a list of blocks (`input_text` / `output_text` / `Text`...): concatenate those that have a text field
fn join_texts(content: Option<&Value>) -> String {
    match content {
        Some(Value::Array(a)) => a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}
