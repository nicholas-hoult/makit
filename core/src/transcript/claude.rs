//! claude 的 jsonl 逐行解析（规则见 TRD §11.4）。

use serde_json::Value;

use super::model::{DividerKind, Item, ItemKind, Level, ToolResult};
use super::state::{Rec, State};

/// 没有 uuid 的元数据 / 状态记录：认识，不是对话内容
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
        st.count_unknown(if ty.is_empty() { "<无 type>".into() } else { ty.into() });
        return vec![];
    }
    // 子代理侧链：主对话里只留那个工具调用和它的最终结果
    if v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return vec![];
    }
    let Some(uuid) = v.get("uuid").and_then(Value::as_str) else { return vec![] };
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
    let mut logical_parent = s("logicalParentUuid");
    // 压缩边界的 logicalParentUuid 有时指向边界之后才出现的记录（它的祖先链又绕回边界 → 成环，压缩前的对话全丢）。
    // 这种情况改接文件里在它前面的最后一条对话记录；只对压缩边界这么做，用户根记录的回退是正当的
    let is_boundary = ty == "system" && matches!(s("subtype").as_deref(), Some("compact_boundary" | "microcompact_boundary"));
    if is_boundary && s("parentUuid").is_none() && !logical_parent.as_ref().is_some_and(|l| st.recs.contains_key(l)) {
        logical_parent = st.leaf.clone();
    }
    st.recs.insert(uuid.to_string(), Rec { parent: s("parentUuid"), logical_parent, conv: matches!(ty, "user" | "assistant") });
    if matches!(ty, "user" | "assistant") {
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
        _ => {} // attachment / progress：不是对话内容
    }
    out.updated
}

/// 往 `all` 里追加 Item 的小帮手：同一条记录里第 n 个 Item 的 id 是 `uuid#n`
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
        // 给模型看的，不是用户说的
    } else if t.starts_with("[Request interrupted") {
        o.add(ItemKind::Notice { level: Level::Warn, text: "已中断".into() });
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
                    _ => {} // tool_reference 等：没有可显示的文字
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
                // 很多 thinking 块只有签名、没有文字：没有可显示的就不出 Item
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
            o.add(ItemKind::Notice { level: Level::Warn, text: format!("API 请求出错，正在重试 {n}/{max}") });
        }
        "informational" => {
            if !content.is_empty() {
                o.add(ItemKind::Notice { level: Level::Info, text: content });
            }
        }
        // 同一条命令在 user 记录里已经有了；hook 摘要给人看的价值不大
        "local_command" | "stop_hook_summary" => {}
        other => o.st.count_unknown(format!("system/{other}")),
    }
}

/// `<tag>内容</tag>` 里的内容
fn tag(s: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let a = s.find(&open)? + open.len();
    let b = s[a..].find(&close)? + a;
    Some(s[a..b].to_string())
}

/// base64 解码后的字节数（不真解码）
fn base64_decoded_len(data: &str) -> usize {
    let n = data.trim_end_matches('=').len();
    n * 3 / 4
}
