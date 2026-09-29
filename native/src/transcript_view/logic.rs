//! 阅读视图的纯逻辑：正文拆块、工具调用一行摘要、reader 的变化 → 列表操作。
//!
//! 为什么单独测：错了在界面上是「代码块把后面的正文吞掉 / 折叠行看不出调用了什么 / 新消息插到错误位置、高度不刷新」。

use std::ops::Range;

use makit_core::transcript::Changes;
use serde_json::Value;

/// 展开的工具输入最多显示多少字符
pub const MAX_INPUT_CHARS: usize = 4000;

#[derive(Clone, Debug, PartialEq)]
pub enum TextBlock {
    Para(String),
    Code { lang: String, text: String },
}

/// 把 markdown 原文拆成段落和围栏代码块。围栏没闭合（流式输出中途常见）时，到文末都算代码
pub fn split_blocks(text: &str) -> Vec<TextBlock> {
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    // 正在收集的代码块：(围栏反引号数, 语言, 行)
    let mut code: Option<(usize, String, Vec<&str>)> = None;
    fn flush_para(para: &mut Vec<&str>, out: &mut Vec<TextBlock>) {
        let joined = para.join("\n");
        if !joined.trim().is_empty() {
            out.push(TextBlock::Para(joined.trim_matches('\n').to_string()));
        }
        para.clear();
    }
    for line in text.lines() {
        let trimmed = line.trim_start();
        let ticks = trimmed.chars().take_while(|&c| c == '`').count();
        match &mut code {
            Some((open, lang, lines)) => {
                // 收尾围栏：至少和开头一样多的反引号，后面没有别的字
                if ticks >= *open && trimmed[ticks..].trim().is_empty() {
                    out.push(TextBlock::Code { lang: std::mem::take(lang), text: lines.join("\n") });
                    code = None;
                } else {
                    lines.push(line);
                }
            }
            None if ticks >= 3 => {
                flush_para(&mut para, &mut out);
                code = Some((ticks, trimmed[ticks..].trim().to_string(), Vec::new()));
            }
            None => para.push(line),
        }
    }
    if let Some((_, lang, lines)) = code {
        out.push(TextBlock::Code { lang, text: lines.join("\n") });
    }
    flush_para(&mut para, &mut out);
    out
}

/// 折叠行的摘要：`Bash(cargo test)`。没有能代表调用的参数就只有工具名
pub fn tool_summary(name: &str, input: &Value) -> String {
    const MAX_ARG_CHARS: usize = 80;
    // 常见工具里最能说明「在干什么」的字段，按优先级
    const KEYS: &[&str] = &["command", "file_path", "pattern", "path", "description", "query", "url"];
    let arg = KEYS.iter().find_map(|k| arg_text(input.get(*k)?)).or_else(|| {
        input.as_object()?.values().find_map(|v| v.as_str().filter(|s| !s.trim().is_empty()).map(String::from))
    });
    let Some(arg) = arg else { return name.to_string() };
    let one_line = arg.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > MAX_ARG_CHARS {
        format!("{name}({}…)", one_line.chars().take(MAX_ARG_CHARS).collect::<String>())
    } else {
        format!("{name}({one_line})")
    }
}

/// 字符串直接用；字符串数组（codex 的 argv）拼起来，`bash -lc "…"` 只留脚本
fn arg_text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Array(a) => {
            let parts: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            match parts.as_slice() {
                [] => None,
                [sh, "-lc" | "-c", script] if sh.ends_with("sh") => Some((*script).to_string()),
                _ => Some(parts.join(" ")),
            }
        }
        _ => None,
    }
}

/// 展开后显示的工具输入：有 `command` 字符串（或 argv）就直接显示命令本身，否则是缩进的 JSON；超长按字符截断
pub fn tool_input_text(input: &Value) -> String {
    let text = match input.get("command").and_then(arg_text) {
        Some(cmd) => cmd,
        None if input.is_null() => return String::new(),
        None => serde_json::to_string_pretty(input).unwrap_or_default(),
    };
    if text.chars().count() > MAX_INPUT_CHARS {
        format!("{}\n…（已截断，共 {} 字符）", text.chars().take(MAX_INPUT_CHARS).collect::<String>(), text.chars().count())
    } else {
        text
    }
}

/// 一轮用时：`2m 56s` / `45s` / `0.8s`（TUI 里的「Baked for 2m 56s」）
pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    match secs {
        0 => format!("{:.1}s", ms as f64 / 1000.0),
        1..=59 => format!("{secs}s"),
        60..=3599 => format!("{}m {}s", secs / 60, secs % 60),
        _ => format!("{}h {}m", secs / 3600, secs % 3600 / 60),
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ListOp {
    Reset(usize),
    Splice { old: Range<usize>, count: usize },
}

/// reader 报的变化 → `ListState` 的操作。`prev_len` 是变化前列表项数，`total` 是变化后可见 Item 总数，
/// `reversed` 为新→旧（追加的项落在列表头部）
pub fn list_ops(changes: &Changes, prev_len: usize, total: usize, reversed: bool) -> Vec<ListOp> {
    if changes.reset {
        return vec![ListOp::Reset(total)];
    }
    let mut ops = Vec::new();
    let added = changes.appended.len();
    if added > 0 {
        let at = if reversed { 0 } else { prev_len };
        ops.push(ListOp::Splice { old: at..at, count: added });
    }
    for &i in &changes.updated {
        let at = if reversed { total - 1 - i } else { i };
        ops.push(ListOp::Splice { old: at..at + 1, count: 1 });
    }
    ops
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn para(s: &str) -> TextBlock {
        TextBlock::Para(s.into())
    }
    fn code(lang: &str, s: &str) -> TextBlock {
        TextBlock::Code { lang: lang.into(), text: s.into() }
    }

    #[test]
    fn plain_text_is_one_paragraph() {
        assert_eq!(split_blocks("你好\n世界"), vec![para("你好\n世界")]);
        assert!(split_blocks("").is_empty());
        assert!(split_blocks("\n\n  \n").is_empty(), "只有空白不出空段落");
    }

    #[test]
    fn fenced_code_splits_the_text() {
        let t = "先看代码：\n```rust\nfn main() {}\n```\n然后是说明";
        assert_eq!(split_blocks(t), vec![para("先看代码："), code("rust", "fn main() {}"), para("然后是说明")]);
    }

    #[test]
    fn unclosed_fence_swallows_to_the_end_only_as_code() {
        // 流式输出到一半：围栏还没闭合，前面的正文不能被吞
        let t = "说明\n```\nlet a = 1;\nlet b";
        assert_eq!(split_blocks(t), vec![para("说明"), code("", "let a = 1;\nlet b")]);
    }

    #[test]
    fn code_keeps_inner_blank_lines_and_indentation() {
        let t = "```py\ndef f():\n\n    return 1\n```";
        assert_eq!(split_blocks(t), vec![code("py", "def f():\n\n    return 1")]);
    }

    #[test]
    fn longer_fence_is_not_closed_by_a_shorter_one() {
        let t = "````md\n```\ninner\n```\n````\n完";
        assert_eq!(split_blocks(t), vec![code("md", "```\ninner\n```"), para("完")]);
    }

    #[test]
    fn summary_picks_the_representative_argument() {
        assert_eq!(tool_summary("Bash", &json!({"command": "cargo test", "description": "跑测试"})), "Bash(cargo test)");
        assert_eq!(tool_summary("Read", &json!({"file_path": "/a/b.rs", "limit": 10})), "Read(/a/b.rs)");
        assert_eq!(tool_summary("Grep", &json!({"pattern": "foo", "path": "."})), "Grep(foo)");
        assert_eq!(tool_summary("Task", &json!({"description": "查根因", "prompt": "很长…"})), "Task(查根因)");
        assert_eq!(tool_summary("Mystery", &json!({"x": 1, "note": "hi"})), "Mystery(hi)", "不认识的工具取第一个字符串字段");
        assert_eq!(tool_summary("TodoWrite", &json!({"todos": []})), "TodoWrite");
        assert_eq!(tool_summary("X", &Value::Null), "X");
    }

    #[test]
    fn summary_is_one_line_and_bounded() {
        let s = tool_summary("Bash", &json!({"command": "echo 1\necho 2"}));
        assert!(!s.contains('\n'), "多行命令折成一行：{s}");
        let long = "字".repeat(300);
        let s = tool_summary("Bash", &json!({"command": long}));
        assert!(s.chars().count() <= 100, "超长要截断：{}", s.chars().count());
        assert!(s.ends_with("…)"), "{s}");
    }

    #[test]
    fn summary_understands_codex_shell_argv() {
        assert_eq!(tool_summary("shell", &json!({"command": ["bash", "-lc", "ls -la"]})), "shell(ls -la)");
        assert_eq!(tool_summary("shell", &json!({"command": ["git", "status"]})), "shell(git status)");
    }

    #[test]
    fn input_text_prefers_the_command_itself() {
        assert_eq!(tool_input_text(&json!({"command": "ls -la\ncd /", "description": "x"})), "ls -la\ncd /");
        assert_eq!(tool_input_text(&json!({"command": ["bash", "-lc", "ls"]})), "ls");
        let s = tool_input_text(&json!({"file_path": "/a", "old_string": "x"}));
        assert!(s.contains("\"file_path\": \"/a\"") && s.contains('\n'), "缩进 JSON：{s}");
        assert_eq!(tool_input_text(&Value::Null), "");
    }

    #[test]
    fn input_text_is_bounded() {
        let s = tool_input_text(&json!({"command": "字".repeat(5000)}));
        assert!(s.chars().count() <= MAX_INPUT_CHARS + 20, "{}", s.chars().count());
        assert!(s.contains("已截断"));
    }

    #[test]
    fn duration_reads_like_the_tui() {
        assert_eq!(format_duration(800), "0.8s");
        assert_eq!(format_duration(45_000), "45s");
        assert_eq!(format_duration(176_000), "2m 56s");
        assert_eq!(format_duration(3_600_000), "1h 0m");
        assert_eq!(format_duration(3_725_000), "1h 2m");
    }

    fn changes(appended: Range<usize>, updated: Vec<usize>, reset: bool) -> Changes {
        Changes { appended, updated, reset }
    }

    #[test]
    fn append_goes_to_the_tail_when_oldest_first() {
        assert_eq!(list_ops(&changes(5..8, vec![], false), 5, 8, false), vec![ListOp::Splice { old: 5..5, count: 3 }]);
    }

    #[test]
    fn append_goes_to_the_head_when_newest_first() {
        assert_eq!(list_ops(&changes(5..8, vec![], false), 5, 8, true), vec![ListOp::Splice { old: 0..0, count: 3 }]);
    }

    #[test]
    fn updated_items_are_remeasured_at_their_display_position() {
        // 第 2 项拿到了工具结果；共 8 项。旧→新在 2，新→旧在 8-1-2=5
        assert_eq!(list_ops(&changes(8..8, vec![2], false), 8, 8, false), vec![ListOp::Splice { old: 2..3, count: 1 }]);
        assert_eq!(list_ops(&changes(8..8, vec![2], false), 8, 8, true), vec![ListOp::Splice { old: 5..6, count: 1 }]);
    }

    #[test]
    fn update_position_uses_the_length_after_append() {
        // 追加 2 项后共 10 项；第 3 项被更新。新→旧下它在 10-1-3=6
        let ops = list_ops(&changes(8..10, vec![3], false), 8, 10, true);
        assert_eq!(ops, vec![ListOp::Splice { old: 0..0, count: 2 }, ListOp::Splice { old: 6..7, count: 1 }]);
    }

    #[test]
    fn reset_rebuilds_everything() {
        assert_eq!(list_ops(&changes(0..12, vec![], true), 8, 12, true), vec![ListOp::Reset(12)]);
    }

    #[test]
    fn nothing_changed_nothing_to_do() {
        assert!(list_ops(&changes(8..8, vec![], false), 8, 8, true).is_empty());
    }
}
