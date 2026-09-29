//! codex 的 rollout 逐行解析（规则见 TRD §11.4）。
//!
//! 0.40 的文件里，消息有**两份**：`response_item`（message）和 `event_msg.item_completed`（UserMessage /
//! AgentMessage），文本逐条一致（实测 17 个有对话的文件全部一致），顺序有时是「R 后 E」（用户）有时是
//! 「E 先、隔着 reasoning 再 R」（助手）。所以：先到的那份出 Item，后到的同角色同文本的那份判重跳过；
//! 判重记录只留最近几条记录之内（用户真的连发两遍同样的话时，第二遍不能被吞掉）。
//! 以后的版本如果只剩一份，先到的就是唯一的一份，照样能显示。

use serde_json::Value;

use super::model::{Item, ItemKind, Level, ToolResult};
use super::state::State;

/// 判重记录保留多少条记录：R 与 E 实测相隔不超过 3 条
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
                // 注入的上下文（环境、AGENTS.md 等）不是用户说的
                "user" if !text.trim_start().starts_with("<environment_context>") && !text.trim_start().starts_with("<user_instructions>") => {
                    message(st, true, text, false, seq, time)
                }
                "assistant" => message(st, false, text, false, seq, time),
                _ => {}
            }
            vec![]
        }
        ("response_item", "reasoning") => {
            // 实测 reasoning 是加密的、summary 为空 → 没有可显示的文字就不出 Item
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

/// 一条消息：另一份来源里刚出过同样的 → 跳过；否则出 Item 并记下「等另一份来判重」
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

/// `content` 是块列表（`input_text` / `output_text` / `Text`…）：把有 text 字段的拼起来
fn join_texts(content: Option<&Value>) -> String {
    match content {
        Some(Value::Array(a)) => a.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    }
}
