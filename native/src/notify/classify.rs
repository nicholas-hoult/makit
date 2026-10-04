//! 分类（纯函数，#215 第 1、2 节）：hook 事件 / 状态文件变化 → `Signal`，以及两路来源的主从。
//!
//! 为什么单独测：分错了在 UI 上就是「等回答被当成等审批、绕过默认关闭的开关弹出来」（#215 问题 4）、
//! 「同一次等待弹两遍」（问题 5）、「任务跑完没提醒 / 后台任务还在跑就提醒完成」。
//! Claude Code 以后改 hook 字段时，这里的用例钉住当前字段，兜底规则也有用例。

use std::collections::HashSet;

use serde_json::Value;

use super::model::{Kind, Signal};

/// 横幅 / 通知中心正文的最长字数（照 对标产品 截 180 字左右）
pub const MESSAGE_MAX_CHARS: usize = 180;

/// 压平空白、截到 `MESSAGE_MAX_CHARS` 个字符（超了加 …）
pub fn truncate_message(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MESSAGE_MAX_CHARS {
        return flat;
    }
    let mut t: String = flat.chars().take(MESSAGE_MAX_CHARS).collect();
    t.push('…');
    t
}

/// hook 的一行 JSON → (会话 id, 信号)。认不出 / 不该通知的返回 None。
///
/// | 事件 | 结果 |
/// |---|---|
/// | Notification + notification_type=permission_prompt | NeedsPermission |
/// | Notification + idle_prompt / elicitation_dialog / 其他需要输入的 | NeedsInput |
/// | Notification + auth_success | None（不是要你处理的事） |
/// | Notification 没有 notification_type（老版本） | 按文本：含 permission → NeedsPermission，否则 NeedsInput |
/// | Stop，stop_hook_active 为假、没有后台任务 | TurnComplete（正文 last_assistant_message） |
/// | Stop，还有后台任务 / stop_hook_active | None（Stop hook 在后台任务没清空时不算「完成」） |
/// | StopFailure，或 Stop 带 error | Error |
/// | UserPromptSubmit / SessionEnd | Clear |
pub fn classify_hook_event(v: &Value) -> Option<(String, Signal)> {
    let str_of = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
    let sid = str_of("session_id");
    if sid.is_empty() {
        return None;
    }
    let notify = |kind, text: &str| Some(Signal::Notify { kind, message: truncate_message(text) });
    let error = str_of("error");
    let signal = match str_of("hook_event_name") {
        "Notification" => {
            let msg = str_of("message");
            match str_of("notification_type") {
                "permission_prompt" => notify(Kind::NeedsPermission, msg),
                "auth_success" => None,
                "" if msg.to_lowercase().contains("permission") => notify(Kind::NeedsPermission, msg),
                _ => notify(Kind::NeedsInput, msg),
            }
        }
        "StopFailure" => notify(Kind::Error, if error.is_empty() { str_of("message") } else { error }),
        "Stop" if !error.is_empty() => notify(Kind::Error, error),
        "Stop" => {
            let hook_active = v.get("stop_hook_active").and_then(Value::as_bool).unwrap_or(false);
            let bg_busy = v.get("background_tasks").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
            if hook_active || bg_busy {
                None
            } else {
                notify(Kind::TurnComplete, str_of("last_assistant_message"))
            }
        }
        "UserPromptSubmit" | "SessionEnd" => Some(Signal::Clear),
        _ => None,
    }?;
    Some((sid.to_string(), signal))
}

/// 状态文件（`~/.claude/sessions/<pid>.json` 合并进会话列表的 status / waiting_for）
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunStatus {
    pub status: String,
    pub waiting_for: String,
}

impl RunStatus {
    pub fn new(status: &str, waiting_for: &str) -> Self {
        Self { status: status.into(), waiting_for: waiting_for.into() }
    }
}

/// 同一个会话前后两次的运行状态 → 信号。`None` = 这个会话没在跑（没有状态文件）。
///
/// - 进入等待（或等待类型变了）→ Notify（waitingFor=user 是等回答，其余是等审批，同 Tauri 版）
/// - 离开等待（包括进程没了）→ Resolved
/// - busy → idle → Notify TurnComplete（没有正文：状态文件里没有 Claude 的原话）
/// - 第一次看到（prev = None）只认等待；busy / idle 不算「完成」
pub fn classify_status_change(prev: Option<&RunStatus>, now: Option<&RunStatus>) -> Option<Signal> {
    fn waiting(s: Option<&RunStatus>) -> Option<&RunStatus> {
        s.filter(|s| s.status == "waiting")
    }
    match (waiting(prev), waiting(now)) {
        (_, Some(n)) if waiting(prev) != Some(n) => {
            let kind = if n.waiting_for == "user" { Kind::NeedsInput } else { Kind::NeedsPermission };
            Some(Signal::Notify { kind, message: String::new() })
        }
        (_, Some(_)) => None,
        (Some(_), None) => Some(Signal::Resolved),
        (None, None) => match (prev.map(|s| s.status.as_str()), now.map(|s| s.status.as_str())) {
            (Some("busy"), Some("idle")) => Some(Signal::Notify { kind: Kind::TurnComplete, message: String::new() }),
            _ => None,
        },
    }
}

/// 两路来源各自管的范围：等待类（审批 / 回答）和结束类（完成 / 出错）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Waiting,
    Done,
}

pub fn category(sig: &Signal) -> Option<Category> {
    match sig {
        Signal::Notify { kind, .. } if kind.needs_action() => Some(Category::Waiting),
        Signal::Notify { .. } => Some(Category::Done),
        _ => None,
    }
}

/// 主从（#215 第 1 节）：某个会话在某一类上收到过 hook，这一类就以 hook 为准，
/// 状态文件那路对它只用来「清除」（Resolved / Clear 永远放行），不再产生通知。
///
/// 按类分开而不是整个会话一刀切：用户全局 settings 里现在只装了 Notification hook（Tauri 版的「安装 hook」），
/// 没有 Stop —— 整会话一刀切的话，装了旧 hook 的会话就永远收不到「已完成」。
#[derive(Default, Debug)]
pub struct Arbiter {
    hook_seen: HashSet<(String, Category)>,
}

impl Arbiter {
    /// 收到一条 hook 信号时调
    pub fn note_hook(&mut self, session_id: &str, sig: &Signal) {
        if let Some(c) = category(sig) {
            self.hook_seen.insert((session_id.to_string(), c));
        }
    }

    /// 状态文件这路的信号要不要放行
    pub fn allow_status(&self, session_id: &str, sig: &Signal) -> bool {
        match category(sig) {
            Some(c) => !self.hook_seen.contains(&(session_id.to_string(), c)),
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn notify(kind: Kind, msg: &str) -> Signal {
        Signal::Notify { kind, message: msg.into() }
    }

    #[test]
    fn notification_type_decides_permission_vs_input() {
        let v = json!({"hook_event_name":"Notification","session_id":"s1","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"});
        assert_eq!(classify_hook_event(&v), Some(("s1".into(), notify(Kind::NeedsPermission, "Claude needs your permission to use Bash"))));
        let v = json!({"hook_event_name":"Notification","session_id":"s1","notification_type":"idle_prompt","message":"Claude is waiting for your input"});
        assert_eq!(classify_hook_event(&v), Some(("s1".into(), notify(Kind::NeedsInput, "Claude is waiting for your input"))));
        let v = json!({"hook_event_name":"Notification","session_id":"s1","notification_type":"elicitation_dialog","message":"q"});
        assert_eq!(classify_hook_event(&v).unwrap().1, notify(Kind::NeedsInput, "q"), "未知的「要输入」类型按等回答");
        let v = json!({"hook_event_name":"Notification","session_id":"s1","notification_type":"auth_success","message":"ok"});
        assert_eq!(classify_hook_event(&v), None, "登录成功不是要你处理的事");
    }

    #[test]
    fn old_notification_without_type_falls_back_to_text() {
        // #215 问题 4：Tauri 版把 hook 来的一律当等审批，还绕过了默认关闭的「等待回答」开关
        let v = json!({"hook_event_name":"Notification","session_id":"s","message":"Claude needs your permission to use Write"});
        assert_eq!(classify_hook_event(&v).unwrap().1, notify(Kind::NeedsPermission, "Claude needs your permission to use Write"));
        let v = json!({"hook_event_name":"Notification","session_id":"s","message":"Claude is waiting for your input"});
        assert_eq!(classify_hook_event(&v).unwrap().1, notify(Kind::NeedsInput, "Claude is waiting for your input"));
        let v = json!({"hook_event_name":"Notification","session_id":"s"});
        assert_eq!(classify_hook_event(&v).unwrap().1, notify(Kind::NeedsInput, ""), "分不出来按等回答");
    }

    #[test]
    fn stop_is_turn_complete_only_when_idle() {
        let v = json!({"hook_event_name":"Stop","session_id":"s","stop_hook_active":false,"last_assistant_message":"改好了，测试全绿。"});
        assert_eq!(classify_hook_event(&v).unwrap().1, notify(Kind::TurnComplete, "改好了，测试全绿。"));
        let v = json!({"hook_event_name":"Stop","session_id":"s","stop_hook_active":true});
        assert_eq!(classify_hook_event(&v), None, "stop hook 自己续跑的那次不算完成");
        let v = json!({"hook_event_name":"Stop","session_id":"s","background_tasks":[{"id":"b1"}]});
        assert_eq!(classify_hook_event(&v), None, "还有后台任务先不发（whenIdle）");
        let v = json!({"hook_event_name":"Stop","session_id":"s","background_tasks":[]});
        assert_eq!(classify_hook_event(&v).unwrap().1, notify(Kind::TurnComplete, ""), "空的后台任务列表 = 空闲");
    }

    #[test]
    fn errors_and_clears() {
        let v = json!({"hook_event_name":"StopFailure","session_id":"s","error":"API Error: 529 overloaded"});
        assert_eq!(classify_hook_event(&v).unwrap().1, notify(Kind::Error, "API Error: 529 overloaded"));
        let v = json!({"hook_event_name":"Stop","session_id":"s","error":"boom"});
        assert_eq!(classify_hook_event(&v).unwrap().1, notify(Kind::Error, "boom"), "带 error 的 Stop 是出错不是完成");
        for ev in ["UserPromptSubmit", "SessionEnd"] {
            let v = json!({"hook_event_name":ev,"session_id":"s"});
            assert_eq!(classify_hook_event(&v), Some(("s".into(), Signal::Clear)), "{ev}");
        }
    }

    #[test]
    fn junk_is_ignored() {
        assert_eq!(classify_hook_event(&json!({"hook_event_name":"Notification"})), None, "没有 session_id");
        assert_eq!(classify_hook_event(&json!({"hook_event_name":"Notification","session_id":""})), None);
        assert_eq!(classify_hook_event(&json!({"hook_event_name":"PreToolUse","session_id":"s"})), None);
        assert_eq!(classify_hook_event(&json!("字符串")), None);
    }

    #[test]
    fn message_is_flattened_and_truncated() {
        assert_eq!(truncate_message("  两行\n\n文字  "), "两行 文字");
        let long: String = "字".repeat(300);
        let t = truncate_message(&long);
        assert_eq!(t.chars().count(), MESSAGE_MAX_CHARS + 1);
        assert!(t.ends_with('…'));
        assert_eq!(truncate_message(&"a".repeat(MESSAGE_MAX_CHARS)), "a".repeat(MESSAGE_MAX_CHARS), "正好 180 不加省略号");
        let v = json!({"hook_event_name":"Stop","session_id":"s","last_assistant_message":long});
        match classify_hook_event(&v).unwrap().1 {
            Signal::Notify { message, .. } => assert_eq!(message.chars().count(), MESSAGE_MAX_CHARS + 1),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn status_changes() {
        let idle = RunStatus::new("idle", "");
        let busy = RunStatus::new("busy", "");
        let wait_perm = RunStatus::new("waiting", "permission");
        let wait_user = RunStatus::new("waiting", "user");
        let c = |p: Option<&RunStatus>, n: Option<&RunStatus>| classify_status_change(p, n);

        assert_eq!(c(Some(&busy), Some(&wait_perm)), Some(notify(Kind::NeedsPermission, "")));
        assert_eq!(c(Some(&busy), Some(&wait_user)), Some(notify(Kind::NeedsInput, "")));
        assert_eq!(c(None, Some(&wait_perm)), Some(notify(Kind::NeedsPermission, "")), "第一次看到就在等待");
        assert_eq!(c(Some(&wait_perm), Some(&wait_perm)), None, "一直在等不重复");
        assert_eq!(c(Some(&wait_perm), Some(&wait_user)), Some(notify(Kind::NeedsInput, "")), "等待类型变了");
        assert_eq!(c(Some(&wait_perm), Some(&busy)), Some(Signal::Resolved), "批准了");
        assert_eq!(c(Some(&wait_user), Some(&idle)), Some(Signal::Resolved));
        assert_eq!(c(Some(&wait_user), None), Some(Signal::Resolved), "进程没了也算离开等待");
        assert_eq!(c(Some(&busy), Some(&idle)), Some(notify(Kind::TurnComplete, "")), "一轮跑完");
        assert_eq!(c(None, Some(&idle)), None, "第一次看到是空闲不算完成");
        assert_eq!(c(None, Some(&busy)), None);
        assert_eq!(c(Some(&busy), None), None, "跑到一半进程没了：不编造完成");
        assert_eq!(c(Some(&idle), Some(&busy)), None);
        assert_eq!(c(None, None), None);
    }

    #[test]
    fn hook_is_primary_per_category() {
        let mut a = Arbiter::default();
        let perm = notify(Kind::NeedsPermission, "");
        let done = notify(Kind::TurnComplete, "");
        assert!(a.allow_status("s", &perm), "没收到过 hook：状态文件照常");
        assert!(a.allow_status("s", &done));

        a.note_hook("s", &notify(Kind::NeedsInput, "x"));
        assert!(!a.allow_status("s", &perm), "等待类以 hook 为准");
        assert!(!a.allow_status("s", &notify(Kind::NeedsInput, "")));
        assert!(a.allow_status("s", &done), "只装了 Notification hook 的会话，完成仍靠状态文件");
        assert!(a.allow_status("s", &Signal::Resolved), "清除永远放行");
        assert!(a.allow_status("other", &perm), "按会话分");

        a.note_hook("s", &notify(Kind::Error, "e"));
        assert!(!a.allow_status("s", &done), "收到过 Stop / 出错之后，完成也以 hook 为准");
        a.note_hook("t", &Signal::Clear);
        assert!(a.allow_status("t", &perm), "Clear 不算「收到过这一类」");
    }
}
