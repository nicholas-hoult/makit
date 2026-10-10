//! Tests for the conversation model (#231 phase 1, plan in TRD section 11.6).
//!
//! Why test this: if parsing is wrong, the screen shows "a stretch of conversation missing / extra, tool results attached to the wrong place, an old conversation from before a rewind popping up /
//! the same sentence appearing twice". The fixtures are records **hand-written following the structure of real files, with placeholder text throughout** (no real content at all).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::*;

// ------------- building records -------------

fn line(v: Value) -> String {
    v.to_string()
}

fn base(ty: &str, uuid: &str, parent: Option<&str>) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("type".into(), json!(ty));
    m.insert("uuid".into(), json!(uuid));
    m.insert("parentUuid".into(), parent.map(Value::from).unwrap_or(Value::Null));
    m.insert("isSidechain".into(), json!(false));
    m.insert("sessionId".into(), json!("s1"));
    m.insert("timestamp".into(), json!("2026-09-29T10:00:00.000Z"));
    m
}

fn with(mut m: serde_json::Map<String, Value>, extra: Value) -> String {
    for (k, v) in extra.as_object().unwrap() {
        m.insert(k.clone(), v.clone());
    }
    line(Value::Object(m))
}

fn user_str(uuid: &str, parent: Option<&str>, text: &str) -> String {
    with(base("user", uuid, parent), json!({"message": {"role": "user", "content": text}}))
}

fn user_blocks(uuid: &str, parent: Option<&str>, blocks: Value) -> String {
    with(base("user", uuid, parent), json!({"message": {"role": "user", "content": blocks}}))
}

fn tool_result(uuid: &str, parent: Option<&str>, tool_use_id: &str, content: Value, is_error: bool) -> String {
    user_blocks(uuid, parent, json!([{"type": "tool_result", "tool_use_id": tool_use_id, "content": content, "is_error": is_error}]))
}

fn asst(uuid: &str, parent: Option<&str>, msg_id: &str, block: Value) -> String {
    with(base("assistant", uuid, parent), json!({"message": {"id": msg_id, "role": "assistant", "content": [block]}}))
}

fn text_block(t: &str) -> Value {
    json!({"type": "text", "text": t})
}

fn tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({"type": "tool_use", "id": id, "name": name, "input": input})
}

fn sys(uuid: &str, parent: Option<&str>, subtype: &str, extra: Value) -> String {
    with(base("system", uuid, parent), extra_with_subtype(subtype, extra))
}

fn extra_with_subtype(subtype: &str, extra: Value) -> Value {
    let mut o = extra.as_object().cloned().unwrap_or_default();
    o.insert("subtype".into(), json!(subtype));
    Value::Object(o)
}

fn parse(tool: Tool, lines: &[String]) -> Vec<Item> {
    let mut s = State::new(tool);
    for l in lines {
        s.feed_line(l);
    }
    s.recompute_visible();
    (0..s.visible_len()).map(|i| s.visible(i).clone()).collect()
}

fn claude(lines: &[String]) -> Vec<Item> {
    parse(Tool::Claude, lines)
}

fn kinds(items: &[Item]) -> Vec<ItemKind> {
    items.iter().map(|i| i.kind.clone()).collect()
}

fn call<'a>(name: &'a str, result: Option<&'a str>) -> impl Fn(&ItemKind) -> bool + 'a {
    move |k| match k {
        ItemKind::ToolCall { name: n, result: r, .. } => n == name && r.as_ref().map(|r| r.text.as_str()) == result,
        _ => false,
    }
}

// ───────────── claude ─────────────

#[test]
fn plain_question_and_answer() {
    let items = claude(&[
        line(json!({"type": "ai-title", "aiTitle": "标题"})),
        user_str("u1", None, "问题一"),
        asst("a1", Some("u1"), "m1", text_block("回答一")),
        sys("s1", Some("a1"), "turn_duration", json!({"durationMs": 2500})),
    ]);
    assert_eq!(kinds(&items), vec![ItemKind::User("问题一".into()), ItemKind::Assistant("回答一".into()), ItemKind::TurnDuration(2500)]);
    assert_eq!(items[0].id, "u1");
    assert_eq!(items[0].time.as_deref(), Some("2026-09-29T10:00:00.000Z"));
}

#[test]
fn non_conversation_records_are_skipped_and_unknown_types_are_counted() {
    let mut s = State::new(Tool::Claude);
    let lines = vec![
        // metadata without a uuid, recognized: does not count as unknown
        line(json!({"type": "queue-operation", "operation": "enqueue"})),
        line(json!({"type": "mode", "mode": "auto"})),
        line(json!({"type": "custom-title", "customTitle": "x"})),
        line(json!({"type": "file-history-snapshot"})),
        // conversation record to skip
        with(base("user", "m1", None), json!({"isMeta": true, "message": {"role": "user", "content": "<local-command-caveat>Caveat: x</local-command-caveat>"}})),
        with(base("user", "sc1", Some("m1")), json!({"isSidechain": true, "message": {"role": "user", "content": "子代理内部的话"}})),
        with(base("user", "cs1", Some("m1")), json!({"isCompactSummary": true, "message": {"role": "user", "content": "This session is being continued…"}})),
        with(base("attachment", "at1", Some("m1")), json!({"attachment": {"type": "date"}})),
        with(base("progress", "pg1", Some("m1")), json!({"data": {"type": "bash_progress"}})),
        sys("h1", Some("m1"), "stop_hook_summary", json!({"hookCount": 1})),
        user_str("u1", Some("m1"), "真问题"),
        // completely unrecognized type (with a uuid): skipped and counted
        with(base("brand-new-thing", "n1", Some("u1")), json!({})),
    ];
    for l in &lines {
        s.feed_line(l);
    }
    s.recompute_visible();
    let got: Vec<_> = (0..s.visible_len()).map(|i| s.visible(i).kind.clone()).collect();
    assert_eq!(got, vec![ItemKind::User("真问题".into())]);
    let unknown: BTreeMap<String, usize> = s.unknown_kinds().clone();
    assert_eq!(unknown.get("brand-new-thing"), Some(&1));
    assert_eq!(unknown.len(), 1, "认识的类型不能算未知：{unknown:?}");
}

#[test]
fn parallel_tool_calls_keep_results_even_though_they_are_off_the_active_chain() {
    // two parallel tool calls in one reply: a3.parent=a2; r1.parent=a2 (not on the final chain), r2.parent=a3
    let items = claude(&[
        user_str("u1", None, "问题"),
        asst("a1", Some("u1"), "m1", json!({"type": "thinking", "thinking": "先看看"})),
        asst("a2", Some("a1"), "m1", tool_use("toolu_1", "Read", json!({"file_path": "/w/a.rs"}))),
        asst("a3", Some("a2"), "m1", tool_use("toolu_2", "Grep", json!({"pattern": "x"}))),
        tool_result("r1", Some("a2"), "toolu_1", json!("文件内容"), false),
        tool_result("r2", Some("a3"), "toolu_2", json!("匹配结果"), false),
        asst("t1", Some("r2"), "m2", text_block("答案")),
    ]);
    let k = kinds(&items);
    assert_eq!(k.len(), 5, "{k:?}");
    assert_eq!(k[0], ItemKind::User("问题".into()));
    assert_eq!(k[1], ItemKind::Thinking("先看看".into()));
    assert!(call("Read", Some("文件内容"))(&k[2]), "{:?}", k[2]);
    assert!(call("Grep", Some("匹配结果"))(&k[3]), "{:?}", k[3]);
    assert_eq!(k[4], ItemKind::Assistant("答案".into()));
    if let ItemKind::ToolCall { input, call_id, .. } = &k[2] {
        assert_eq!(call_id, "toolu_1");
        assert_eq!(input["file_path"], "/w/a.rs");
    }
}

#[test]
fn parallel_tool_results_arriving_one_by_one_never_reset_the_view() {
    // Live reading: a2 / a3 are two parallel tool calls; r1 (a2's result) arrives first, r2 (a3's result) later.
    // When r1 arrives the leaf must not move back to a2 -- otherwise a3 temporarily vanishes from the visible chain, the reader reports reset, and the view is rebuilt entirely (a visible flash)
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("mk-live-{}.jsonl", std::process::id()));
    std::fs::write(&path, "").unwrap();
    let mut r = TranscriptReader::new(path.clone(), Tool::Claude);
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    let lines = [
        user_str("u1", None, "问题"),
        asst("a2", Some("u1"), "m1", tool_use("toolu_1", "Read", json!({}))),
        asst("a3", Some("a2"), "m1", tool_use("toolu_2", "Grep", json!({}))),
        tool_result("r1", Some("a2"), "toolu_1", json!("文件内容"), false),
        tool_result("r2", Some("a3"), "toolu_2", json!("匹配结果"), false),
        asst("t1", Some("r2"), "m2", text_block("答案")),
    ];
    let mut seen = 0;
    for l in &lines {
        writeln!(f, "{l}").unwrap();
        let ch = r.poll().unwrap();
        assert!(!ch.reset, "写入 {l:.60} 之后不该 reset");
        seen += ch.appended.len();
    }
    assert_eq!(seen, 4, "问题 + 两个调用 + 答案");
    assert_eq!(r.len(), 4);
    std::fs::remove_file(&path).ok();
}

#[test]
fn tool_result_arriving_last_does_not_hide_the_parallel_sibling() {
    let items = claude(&[
        user_str("u1", None, "问题"),
        asst("a2", Some("u1"), "m1", tool_use("toolu_1", "Read", json!({}))),
        asst("a3", Some("a2"), "m1", tool_use("toolu_2", "Grep", json!({}))),
        tool_result("r1", Some("a2"), "toolu_1", json!("文件内容"), false),
    ]);
    assert_eq!(items.len(), 3, "{:?}", kinds(&items));
}

#[test]
fn later_block_of_the_same_response_skipping_a_sibling_keeps_the_sibling_visible() {
    // Shape in a real file: the same reply (m1) is split into three blocks a2 / a3 / a4; a4's parent is r1 (a2's result), skipping a3.
    // a3 belongs to the same reply and is not an abandoned branch, so it must stay visible throughout, and the reader must not reset either
    use std::io::Write;
    let path = std::env::temp_dir().join(format!("mk-live2-{}.jsonl", std::process::id()));
    std::fs::write(&path, "").unwrap();
    let mut r = TranscriptReader::new(path.clone(), Tool::Claude);
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    let lines = [
        user_str("u1", None, "问题"),
        asst("a2", Some("u1"), "m1", tool_use("toolu_1", "Read", json!({}))),
        asst("a3", Some("a2"), "m1", tool_use("toolu_2", "Grep", json!({}))),
        tool_result("r1", Some("a2"), "toolu_1", json!("文件内容"), false),
        asst("a4", Some("r1"), "m1", tool_use("toolu_3", "Bash", json!({}))),
        tool_result("r2", Some("a3"), "toolu_2", json!("匹配结果"), false),
    ];
    for l in &lines {
        writeln!(f, "{l}").unwrap();
        let ch = r.poll().unwrap();
        assert!(!ch.reset, "写入 {l:.60} 之后不该 reset");
    }
    assert_eq!(r.len(), 4, "问题 + Read + Grep + Bash");
    assert!(call("Grep", Some("匹配结果"))(&r.item(2).kind), "{:?}", r.item(2).kind);
    std::fs::remove_file(&path).ok();
}

#[test]
fn tool_result_content_shapes() {
    let items = claude(&[
        user_str("u1", None, "q"),
        asst("a1", Some("u1"), "m1", tool_use("t_str", "Bash", json!({}))),
        asst("a2", Some("a1"), "m1", tool_use("t_list", "Bash", json!({}))),
        asst("a3", Some("a2"), "m1", tool_use("t_img", "Bash", json!({}))),
        asst("a4", Some("a3"), "m1", tool_use("t_ref", "ToolSearch", json!({}))),
        asst("a5", Some("a4"), "m1", tool_use("t_err", "Bash", json!({}))),
        tool_result("r1", Some("a1"), "t_str", json!("字符串结果"), false),
        tool_result("r2", Some("a2"), "t_list", json!([{"type": "text", "text": "第一段"}, {"type": "text", "text": "第二段"}]), false),
        tool_result("r3", Some("a3"), "t_img", json!([{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}, {"type": "text", "text": "图注"}]), false),
        tool_result("r4", Some("a4"), "t_ref", json!([{"type": "tool_reference", "tool_name": "X"}]), false),
        tool_result("r5", Some("a5"), "t_err", json!("Exit code 1\nboom"), true),
    ]);
    let res = |i: usize| match &items[i].kind {
        ItemKind::ToolCall { result: Some(r), .. } => r.clone(),
        other => panic!("{i}: {other:?}"),
    };
    assert_eq!(res(1).text, "字符串结果");
    assert_eq!(res(2).text, "第一段\n第二段", "文本块用换行拼");
    let img = res(3);
    assert_eq!((img.text.as_str(), img.images), ("图注", 1), "图片只计数，文字保留");
    assert_eq!(res(4).text, "", "tool_reference 没有可显示的文字");
    let err = res(5);
    assert!(err.is_error && err.text.starts_with("Exit code 1"));
    assert!(!res(1).is_error);
}

#[test]
fn one_api_message_split_over_lines_keeps_order_and_unique_ids() {
    let items = claude(&[
        user_str("u1", None, "q"),
        asst("a1", Some("u1"), "m1", json!({"type": "thinking", "thinking": "想"})),
        asst("a2", Some("a1"), "m1", text_block("说")),
        asst("a3", Some("a2"), "m1", tool_use("t1", "Read", json!({}))),
    ]);
    let ids: Vec<_> = items.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, vec!["u1", "a1", "a2", "a3"]);
    // when one record has several blocks, later ids carry #n
    let items = claude(&[with(
        base("assistant", "x1", None),
        json!({"message": {"id": "m", "role": "assistant", "content": [text_block("一"), text_block("二"), tool_use("t", "Bash", json!({}))]}}),
    )]);
    let ids: Vec<_> = items.iter().map(|i| i.id.clone()).collect();
    assert_eq!(ids, vec!["x1", "x1#1", "x1#2"]);
}

#[test]
fn assistant_blank_text_is_dropped() {
    let items = claude(&[user_str("u1", None, "q"), asst("a1", Some("u1"), "m1", text_block("  \n")), asst("a2", Some("a1"), "m1", text_block("有内容"))]);
    assert_eq!(kinds(&items), vec![ItemKind::User("q".into()), ItemKind::Assistant("有内容".into())]);
}

#[test]
fn tagged_user_strings() {
    let items = claude(&[
        user_str("u1", None, "<command-name>/model</command-name>\n            <command-message>model</command-message>\n            <command-args></command-args>"),
        user_str("u2", Some("u1"), "<local-command-stdout>Set model to X</local-command-stdout>"),
        user_str("u3", Some("u2"), "<bash-input>ls</bash-input>"),
        user_str("u4", Some("u3"), "<bash-stdout>a.txt</bash-stdout><bash-stderr></bash-stderr>"),
        user_str("u5", Some("u4"), "<task-notification>\n<task-id>b1</task-id>\n<status>completed</status>\n<summary>Background command \"构建\" completed (exit code 0)</summary>\n</task-notification>\nRead the output file"),
        user_str("u6", Some("u5"), "<system-reminder>别显示我</system-reminder>"),
        user_str("u7", Some("u6"), "[Request interrupted by user]"),
        user_str("u8", Some("u7"), "<command-name>/rename</command-name><command-message>rename</command-message><command-args>新名字</command-args>"),
    ]);
    assert_eq!(
        kinds(&items),
        vec![
            ItemKind::Command { name: "/model".into(), args: "".into() },
            ItemKind::LocalOutput("Set model to X".into()),
            ItemKind::BashInput("ls".into()),
            ItemKind::LocalOutput("a.txt".into()),
            ItemKind::Notice { level: Level::Info, text: "Background command \"构建\" completed (exit code 0)".into() },
            ItemKind::Notice { level: Level::Warn, text: "已中断".into() },
            ItemKind::Command { name: "/rename".into(), args: "新名字".into() },
        ]
    );
}

#[test]
fn user_block_lists_text_and_image() {
    let items = claude(&[user_blocks(
        "u1",
        None,
        json!([{"type": "text", "text": "看这张图"}, {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}, {"type": "text", "text": "[Image #1]"}]),
    )]);
    assert_eq!(
        kinds(&items),
        vec![ItemKind::User("看这张图".into()), ItemKind::Image { media_type: "image/png".into(), bytes: 3 }, ItemKind::User("[Image #1]".into())]
    );
}

#[test]
fn system_records() {
    let items = claude(&[
        user_str("u1", None, "q"),
        sys("s1", Some("u1"), "away_summary", json!({"content": "做完了 X"})),
        sys("s2", Some("s1"), "informational", json!({"content": "提示一句"})),
        sys("s3", Some("s2"), "api_error", json!({"retryAttempt": 1, "maxRetries": 10, "retryInMs": 560.7})),
        sys("s4", Some("s3"), "local_command", json!({"content": "<command-name>/rename</command-name>"})),
        sys("s5", Some("s4"), "stop_hook_summary", json!({})),
        sys("s6", Some("s5"), "turn_duration", json!({"durationMs": 176000})),
    ]);
    let k = kinds(&items);
    assert_eq!(k[1], ItemKind::Notice { level: Level::Info, text: "recap: 做完了 X".into() });
    assert_eq!(k[2], ItemKind::Notice { level: Level::Info, text: "提示一句".into() });
    match &k[3] {
        ItemKind::Notice { level: Level::Warn, text } => assert!(text.contains("1/10"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert_eq!(k[4], ItemKind::TurnDuration(176000), "local_command（user 记录里已有）和 stop_hook_summary 都跳过");
    assert_eq!(k.len(), 5);
}

#[test]
fn compaction_keeps_the_history_before_it_via_logical_parent() {
    let items = claude(&[
        user_str("u1", None, "压缩前的问题"),
        asst("a1", Some("u1"), "m1", text_block("压缩前的回答")),
        // compaction boundary: parentUuid is empty, reconnect via logicalParentUuid
        with(base("system", "c1", None), json!({"subtype": "compact_boundary", "content": "Conversation compacted", "logicalParentUuid": "a1", "compactMetadata": {"trigger": "manual", "preTokens": 100}})),
        with(base("user", "cs", Some("c1")), json!({"isCompactSummary": true, "message": {"role": "user", "content": "摘要…"}})),
        user_str("u2", Some("cs"), "压缩后的问题"),
        asst("a2", Some("u2"), "m2", text_block("压缩后的回答")),
    ]);
    assert_eq!(
        kinds(&items),
        vec![
            ItemKind::User("压缩前的问题".into()),
            ItemKind::Assistant("压缩前的回答".into()),
            ItemKind::Divider(DividerKind::Compacted),
            ItemKind::User("压缩后的问题".into()),
            ItemKind::Assistant("压缩后的回答".into()),
        ]
    );
}

#[test]
fn compaction_whose_logical_parent_points_forward_falls_back_to_the_previous_message() {
    // In a real file, 5 of 22 compaction boundaries have a logicalParentUuid pointing at a record that appears **after the boundary** (att1, whose own ancestor chain passes through the boundary)
    // -> following it would form a cycle and lose the whole pre-compaction conversation. In that case re-attach to "the last conversation record before it in the file"
    let items = claude(&[
        user_str("u1", None, "压缩前的问题"),
        asst("a1", Some("u1"), "m1", text_block("压缩前的回答")),
        with(base("system", "c1", None), json!({"subtype": "compact_boundary", "content": "Conversation compacted", "logicalParentUuid": "att1"})),
        with(base("user", "cs", Some("c1")), json!({"isCompactSummary": true, "message": {"role": "user", "content": "摘要…"}})),
        with(base("attachment", "att1", Some("cs")), json!({"attachment": {"type": "date"}})),
        user_str("u2", Some("att1"), "压缩后的问题"),
        asst("a2", Some("u2"), "m2", text_block("压缩后的回答")),
    ]);
    assert_eq!(
        kinds(&items),
        vec![
            ItemKind::User("压缩前的问题".into()),
            ItemKind::Assistant("压缩前的回答".into()),
            ItemKind::Divider(DividerKind::Compacted),
            ItemKind::User("压缩后的问题".into()),
            ItemKind::Assistant("压缩后的回答".into()),
        ]
    );
}

#[test]
fn rewind_hides_the_abandoned_branch() {
    // u2 is the old question; after the rewind, u3 is asked instead (both have a1 as parent)
    let items = claude(&[
        user_str("u1", None, "开头"),
        asst("a1", Some("u1"), "m1", text_block("回答一")),
        user_str("u2", Some("a1"), "旧问法"),
        asst("a2", Some("u2"), "m2", text_block("旧回答")),
        sys("s2", Some("a2"), "turn_duration", json!({"durationMs": 1})),
        user_str("u3", Some("a1"), "改后的问法"),
        asst("a3", Some("u3"), "m3", text_block("新回答")),
    ]);
    assert_eq!(
        kinds(&items),
        vec![ItemKind::User("开头".into()), ItemKind::Assistant("回答一".into()), ItemKind::User("改后的问法".into()), ItemKind::Assistant("新回答".into())]
    );
}

#[test]
fn long_tool_output_is_truncated_on_a_char_boundary() {
    let big = "汉".repeat(10_000); // 30000 bytes
    let items = claude(&[user_str("u1", None, "q"), asst("a1", Some("u1"), "m", tool_use("t", "Bash", json!({}))), tool_result("r", Some("a1"), "t", json!(big), false)]);
    let ItemKind::ToolCall { result: Some(r), .. } = &items[1].kind else { panic!() };
    assert!(r.truncated);
    assert_eq!(r.total_len, 30_000);
    assert!(r.text.len() <= MAX_TOOL_TEXT && r.text.len() % 3 == 0, "在字符边界上截：{}", r.text.len());
    let small = ToolResult::new("短", false, 0);
    assert!(!small.truncated && small.total_len == 3);
}

#[test]
fn garbage_lines_do_not_panic() {
    let mut s = State::new(Tool::Claude);
    for l in ["", "   ", "{oops", "[1,2]", "null", "{\"type\": 5}"] {
        s.feed_line(l);
    }
    s.recompute_visible();
    assert_eq!(s.visible_len(), 0);
}

// ───────────── codex ─────────────

fn cx(kind: &str, payload: Value) -> String {
    line(json!({"timestamp": "2026-09-29T10:00:00.000Z", "type": kind, "payload": payload}))
}

fn cx_user(text: &str) -> String {
    cx("response_item", json!({"type": "message", "role": "user", "content": [{"type": "input_text", "text": text}]}))
}

fn cx_asst(text: &str) -> String {
    cx("response_item", json!({"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": text}]}))
}

fn cx_done(kind: &str, text: &str) -> String {
    cx("event_msg", json!({"type": "item_completed", "item": {"type": kind, "id": "i", "content": [{"type": "Text", "text": text}]}}))
}

fn codex(lines: &[String]) -> Vec<Item> {
    parse(Tool::Codex, lines)
}

#[test]
fn codex_basic_conversation_dedups_the_two_generations_of_records() {
    // In a real 0.40 file: the response_item message and the item_completed message are two copies of the same batch of events, with identical text,
    // and the order is sometimes "R then E" (user) and sometimes "E then R after a reasoning item" (assistant)
    let items = codex(&[
        cx("session_meta", json!({"session_id": "s", "cwd": "/w", "cli_version": "0.40.0"})),
        cx_user("<environment_context>\n  <cwd>/w</cwd>\n</environment_context>"),
        cx_user("你好"),
        cx_done("UserMessage", "你好"),
        cx_done("AgentMessage", "你好，我是助手"),
        cx("response_item", json!({"type": "reasoning", "summary": [], "content": null, "encrypted_content": "gAAAA"})),
        cx_asst("你好，我是助手"),
        cx("event_msg", json!({"type": "token_count", "info": {}})),
        cx("event_msg", json!({"type": "task_complete", "turn_id": "t"})),
    ]);
    assert_eq!(kinds(&items), vec![ItemKind::User("你好".into()), ItemKind::Assistant("你好，我是助手".into())], "reasoning 加密、没有可显示的文字 → 不产生 Item");
}

#[test]
fn codex_reasoning_with_summary_text_becomes_thinking() {
    let items = codex(&[cx("response_item", json!({"type": "reasoning", "summary": [{"type": "summary_text", "text": "先看目录"}], "content": null, "encrypted_content": "x"}))]);
    assert_eq!(kinds(&items), vec![ItemKind::Thinking("先看目录".into())]);
}

#[test]
fn codex_a_genuinely_repeated_message_is_not_swallowed() {
    // the user really sent the same message twice: four records R,E,R,E -> two user messages
    let items = codex(&[cx_user("再试一次"), cx_done("UserMessage", "再试一次"), cx_user("再试一次"), cx_done("UserMessage", "再试一次")]);
    assert_eq!(kinds(&items), vec![ItemKind::User("再试一次".into()), ItemKind::User("再试一次".into())]);
}

#[test]
fn codex_only_one_generation_present_still_works() {
    // suppose a later version has only item_completed (or only response_item): both must be displayable
    let only_e = codex(&[cx_done("UserMessage", "问"), cx_done("AgentMessage", "答")]);
    assert_eq!(kinds(&only_e), vec![ItemKind::User("问".into()), ItemKind::Assistant("答".into())]);
    let only_r = codex(&[cx_user("问"), cx_asst("答")]);
    assert_eq!(kinds(&only_r), vec![ItemKind::User("问".into()), ItemKind::Assistant("答".into())]);
}

#[test]
fn codex_function_call_pairs_with_its_output() {
    let items = codex(&[
        cx_user("列一下文件"),
        cx("response_item", json!({"type": "function_call", "name": "shell", "arguments": "{\"command\":[\"bash\",\"-lc\",\"ls -la\"],\"timeout_ms\":120000}", "call_id": "call_1"})),
        cx("response_item", json!({"type": "function_call", "name": "update_plan", "arguments": "{\"plan\":[]}", "call_id": "call_2"})),
        cx("response_item", json!({"type": "function_call_output", "call_id": "call_1", "output": "{\"output\":\"a.txt\\nb.txt\",\"metadata\":{\"exit_code\":0,\"duration_seconds\":0.1}}"})),
        cx("response_item", json!({"type": "function_call_output", "call_id": "call_2", "output": "{\"output\":\"boom\",\"metadata\":{\"exit_code\":2}}"})),
    ]);
    let k = kinds(&items);
    assert_eq!(k.len(), 3);
    match &k[1] {
        ItemKind::ToolCall { call_id, name, input, result: Some(r) } => {
            assert_eq!((call_id.as_str(), name.as_str()), ("call_1", "shell"));
            assert_eq!(input["command"][2], "ls -la");
            assert_eq!(r.text, "a.txt\nb.txt");
            assert!(!r.is_error);
        }
        other => panic!("{other:?}"),
    }
    match &k[2] {
        ItemKind::ToolCall { result: Some(r), .. } => assert!(r.is_error && r.text == "boom", "退出码非零算出错：{r:?}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn codex_plain_text_output_and_bad_arguments() {
    let items = codex(&[
        cx("response_item", json!({"type": "function_call", "name": "shell", "arguments": "不是 json", "call_id": "c"})),
        cx("response_item", json!({"type": "function_call_output", "call_id": "c", "output": "纯文本输出"})),
    ]);
    match &items[0].kind {
        ItemKind::ToolCall { input, result: Some(r), .. } => {
            assert_eq!(input, &json!("不是 json"), "参数解析失败保留原字符串");
            assert_eq!(r.text, "纯文本输出");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn codex_aborted_turn_and_ignored_records() {
    let items = codex(&[
        cx("turn_context", json!({"cwd": "/w"})),
        cx("event_msg", json!({"type": "task_started", "turn_id": "t"})),
        cx("event_msg", json!({"type": "thread_settings_applied", "thread_id": "x", "thread_settings": {}})),
        cx("event_msg", json!({"type": "turn_aborted", "turn_id": "t", "reason": "interrupted"})),
        cx("event_msg", json!({"type": "brand_new_event"})),
    ]);
    assert_eq!(kinds(&items), vec![ItemKind::Notice { level: Level::Warn, text: "已中断".into() }]);
}

#[test]
fn codex_user_message_event_is_a_user_message() {
    // a few files have only event_msg.user_message
    let items = codex(&[cx("event_msg", json!({"type": "user_message", "message": "只有事件", "kind": "plain"}))]);
    assert_eq!(kinds(&items), vec![ItemKind::User("只有事件".into())]);
}

// ------------- incremental reader -------------

use std::fs;
use std::io::Write;
use std::path::PathBuf;

fn tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("makit-transcript-{}-{tag}.jsonl", std::process::id()));
    let _ = fs::remove_file(&p);
    p
}

fn append(p: &PathBuf, bytes: &[u8]) {
    let mut f = fs::OpenOptions::new().create(true).append(true).open(p).unwrap();
    f.write_all(bytes).unwrap();
}

fn reader_items(r: &TranscriptReader) -> Vec<ItemKind> {
    (0..r.len()).map(|i| r.item(i).kind.clone()).collect()
}

fn joined(lines: &[String]) -> String {
    let mut s = lines.join("\n");
    s.push('\n');
    s
}

fn sample_session() -> Vec<String> {
    vec![
        line(json!({"type": "ai-title", "aiTitle": "标题"})),
        // first an old question that was rewound away (same parent as u1: the root)
        user_str("u0", None, "被回退的旧问法"),
        asst("a0", Some("u0"), "m0", text_block("被回退的旧回答")),
        user_str("u1", None, "问题一：中文也要能被切在中间"),
        asst("a1", Some("u1"), "m1", json!({"type": "thinking", "thinking": "想一想"})),
        asst("a2", Some("a1"), "m1", tool_use("t1", "Read", json!({"file_path": "/w/a"}))),
        asst("a3", Some("a2"), "m1", tool_use("t2", "Grep", json!({"pattern": "x"}))),
        tool_result("r1", Some("a2"), "t1", json!("内容一"), false),
        tool_result("r2", Some("a3"), "t2", json!("内容二"), false),
        asst("a4", Some("r2"), "m2", text_block("回答")),
        sys("s1", Some("a4"), "turn_duration", json!({"durationMs": 1234})),
    ]
}

#[test]
fn incremental_read_equals_one_shot_at_every_split_point() {
    let text = joined(&sample_session());
    let bytes = text.as_bytes();
    let want = claude(&sample_session()).into_iter().map(|i| i.kind).collect::<Vec<_>>();
    assert!(want.len() >= 6, "夹具本身要有内容：{want:?}");
    let p = tmp("split");
    for k in (0..=bytes.len()).step_by(5).chain([bytes.len()]) {
        let _ = fs::remove_file(&p);
        append(&p, &bytes[..k]);
        let mut r = TranscriptReader::new(p.clone(), Tool::Claude);
        r.poll().unwrap();
        append(&p, &bytes[k..]);
        r.poll().unwrap();
        assert_eq!(reader_items(&r), want, "在字节 {k} 处切开读，结果要和一次读完相同");
    }
    let _ = fs::remove_file(&p);
}

#[test]
fn half_written_line_waits_for_its_newline() {
    let p = tmp("half");
    let l = user_str("u1", None, "半行");
    append(&p, l.as_bytes());
    let mut r = TranscriptReader::new(p.clone(), Tool::Claude);
    let c = r.poll().unwrap();
    assert_eq!((r.len(), c.appended.clone(), c.reset), (0, 0..0, false), "没写完换行的行不能吃");
    append(&p, b"\n");
    let c = r.poll().unwrap();
    assert_eq!((r.len(), c.appended, c.reset), (1, 0..1, false));
    let _ = fs::remove_file(&p);
}

#[test]
fn appended_range_and_updates_are_reported() {
    let p = tmp("delta");
    append(&p, joined(&[user_str("u1", None, "q"), asst("a1", Some("u1"), "m", tool_use("t1", "Bash", json!({})))]).as_bytes());
    let mut r = TranscriptReader::new(p.clone(), Tool::Claude);
    let c = r.poll().unwrap();
    assert_eq!((c.appended, c.updated.clone(), c.reset), (0..2, vec![], false));
    // the result arrives late: update the earlier ToolCall (index 1), not a new addition
    append(&p, joined(&[tool_result("r1", Some("a1"), "t1", json!("输出"), false)]).as_bytes());
    let c = r.poll().unwrap();
    assert_eq!((c.appended, c.updated, c.reset), (2..2, vec![1], false));
    assert!(call("Bash", Some("输出"))(&r.item(1).kind));
    // then a new answer: append only
    append(&p, joined(&[asst("a2", Some("r1"), "m2", text_block("完"))]).as_bytes());
    let c = r.poll().unwrap();
    assert_eq!((c.appended, c.updated, c.reset), (2..3, vec![], false));
    // nothing changed
    let c = r.poll().unwrap();
    assert_eq!((c.appended, c.updated, c.reset), (3..3, vec![], false));
    let _ = fs::remove_file(&p);
}

#[test]
fn rewind_appended_later_resets_the_view() {
    let p = tmp("rewind");
    append(&p, joined(&[user_str("u1", None, "开头"), asst("a1", Some("u1"), "m1", text_block("一")), user_str("u2", Some("a1"), "旧问法"), asst("a2", Some("u2"), "m2", text_block("旧答"))]).as_bytes());
    let mut r = TranscriptReader::new(p.clone(), Tool::Claude);
    r.poll().unwrap();
    assert_eq!(r.len(), 4);
    append(&p, joined(&[user_str("u3", Some("a1"), "新问法"), asst("a3", Some("u3"), "m3", text_block("新答"))]).as_bytes());
    let c = r.poll().unwrap();
    assert!(c.reset, "当前分支变了，旧的两条要消失，只能整体重建");
    assert_eq!(reader_items(&r), vec![ItemKind::User("开头".into()), ItemKind::Assistant("一".into()), ItemKind::User("新问法".into()), ItemKind::Assistant("新答".into())]);
    let _ = fs::remove_file(&p);
}

#[test]
fn truncated_or_replaced_file_resets() {
    let p = tmp("trunc");
    append(&p, joined(&[user_str("u1", None, "很长很长的第一版内容内容内容"), asst("a1", Some("u1"), "m", text_block("答一")), asst("a2", Some("a1"), "m", text_block("答二"))]).as_bytes());
    let mut r = TranscriptReader::new(p.clone(), Tool::Claude);
    r.poll().unwrap();
    assert_eq!(r.len(), 3);
    fs::write(&p, joined(&[user_str("x1", None, "新")])).unwrap();
    let c = r.poll().unwrap();
    assert!(c.reset);
    assert_eq!(reader_items(&r), vec![ItemKind::User("新".into())]);
    let _ = fs::remove_file(&p);
}

#[test]
fn missing_file_is_not_an_error() {
    let mut r = TranscriptReader::new(tmp("nonexistent"), Tool::Codex);
    let c = r.poll().unwrap();
    assert_eq!((r.len(), c.appended, c.reset), (0, 0..0, false));
}

#[test]
fn codex_reader_works_too() {
    let p = tmp("codex");
    append(&p, joined(&[cx_user("<environment_context>x</environment_context>"), cx_user("你好"), cx_done("UserMessage", "你好")]).as_bytes());
    let mut r = TranscriptReader::new(p.clone(), Tool::Codex);
    r.poll().unwrap();
    assert_eq!(reader_items(&r), vec![ItemKind::User("你好".into())]);
    append(&p, joined(&[cx_done("AgentMessage", "答")]).as_bytes());
    let c = r.poll().unwrap();
    assert_eq!((c.appended, c.reset), (1..2, false));
    let _ = fs::remove_file(&p);
}

// ------------- smoke test on real files (run manually: cargo test -p makit-core --release -- --ignored --nocapture real_files) -------------

/// Scan all claude / codex session files on this machine: no panic, every non-empty file yields Items, and summarize unknown record types and timing.
/// This is the first thing to run after upgrading claude / codex: if the format changed, it reports unknown types first.
#[test]
#[ignore]
fn real_files_parse_without_panics() {
    let home = dirs::home_dir().unwrap();
    let mut files: Vec<(Tool, PathBuf)> = Vec::new();
    for e in glob_files(&home.join(".claude/projects"), 2) {
        files.push((Tool::Claude, e));
    }
    for e in glob_files(&home.join(".codex/sessions"), 4) {
        files.push((Tool::Codex, e));
    }
    let mut unknown: BTreeMap<String, usize> = BTreeMap::new();
    let (mut total_items, mut total_bytes, mut empty, mut invalid) = (0usize, 0u64, 0usize, 0usize);
    let (mut calls, mut calls_with_result) = (0usize, 0usize);
    let mut slowest = (0.0f64, String::new());
    let t_all = std::time::Instant::now();
    for (tool, path) in &files {
        let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let t = std::time::Instant::now();
        let mut r = TranscriptReader::new(path.clone(), *tool);
        r.poll().unwrap();
        let dt = t.elapsed().as_secs_f64();
        if dt > slowest.0 {
            slowest = (dt, format!("{} ({} MB)", path.file_name().unwrap().to_string_lossy(), size / 1_000_000));
        }
        total_items += r.len();
        total_bytes += size;
        for i in 0..r.len() {
            if let ItemKind::ToolCall { result, .. } = &r.item(i).kind {
                calls += 1;
                calls_with_result += result.is_some() as usize;
            }
        }
        // A codex session that has only session_meta + injected environment context (opened and closed right away) is indeed empty (a few hundred bytes); only a large file parsing to 0 Items is a problem
        if r.is_empty() && size > 4096 {
            empty += 1;
            eprintln!("空：{}", path.display());
        }
        invalid += r.invalid_lines();
        for (k, n) in r.unknown_kinds() {
            *unknown.entry(format!("{tool:?}:{k}")).or_default() += n;
        }
    }
    eprintln!(
        "文件 {}，{} MB，Item {}，空文件 {}，坏行 {}，总耗时 {:.2}s，最慢 {:.2}s {}",
        files.len(),
        total_bytes / 1_000_000,
        total_items,
        empty,
        invalid,
        t_all.elapsed().as_secs_f64(),
        slowest.0,
        slowest.1
    );
    eprintln!("未知记录类型：{unknown:?}");
    eprintln!("工具调用 {calls}，其中挂上结果的 {calls_with_result}（没有结果的多半是进行中 / 被打断的）");
    assert!(calls == 0 || calls_with_result * 100 >= calls * 95, "挂上结果的工具调用不到 95%，配对规则可能有漏");
    assert_eq!(empty, 0, "有大文件解析出 0 个 Item");
}

fn glob_files(dir: &std::path::Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth > 0 {
            out.extend(glob_files(&p, depth - 1));
        } else if p.extension().is_some_and(|x| x == "jsonl") {
            out.push(p);
        }
    }
    out
}

/// Forensics (#231): "write" a real session file into a temp file line by line, poll after each line, and count resets / appends / updates.
/// With a live-open detail panel, every reset rebuilds the whole list -> a visible flash on screen
#[test]
#[ignore]
fn live_replay_counts_resets() {
    use std::io::Write;
    let Ok(src) = std::env::var("REPLAY_FILE") else { return };
    let text = std::fs::read_to_string(&src).unwrap();
    let dir = std::env::temp_dir().join(format!("mk-replay-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::write(&path, "").unwrap();
    let mut r = TranscriptReader::new(path.clone(), Tool::Claude);
    let (mut resets, mut appends, mut updates, mut polls) = (0, 0, 0, 0);
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    for (n, line) in text.lines().enumerate() {
        writeln!(f, "{line}").unwrap();
        let ch = r.poll().unwrap();
        polls += 1;
        if ch.reset {
            resets += 1;
            if resets <= 8 {
                eprintln!("reset @行{n}: {}", &line[..line.len().min(160)]);
            }
        }
        appends += ch.appended.len();
        updates += ch.updated.len();
    }
    eprintln!("polls {polls} resets {resets} appended {appends} updated {updates} final {}", r.len());
    std::fs::remove_dir_all(&dir).ok();
}

/// Forensics (#231): how long a cold read of one real file takes, and how long each later "new content" poll takes
#[test]
#[ignore]
fn cold_and_steady_poll_timing() {
    use std::io::Write;
    let Ok(src) = std::env::var("REPLAY_FILE") else { return };
    let dir = std::env::temp_dir().join(format!("mk-time-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("s.jsonl");
    std::fs::copy(&src, &path).unwrap();
    let size = std::fs::metadata(&path).unwrap().len();
    let t = std::time::Instant::now();
    let mut r = TranscriptReader::new(path.clone(), Tool::Claude);
    let ch = r.poll().unwrap();
    eprintln!("冷读 {} MB：{:?}，{} 项", size / 1_000_000, t.elapsed(), ch.appended.len());
    let last = std::fs::read_to_string(&path).unwrap().lines().rev().find(|l| l.contains("\"type\":\"user\"") || l.contains("\"type\":\"assistant\"")).map(String::from).unwrap();
    // Each new record hangs off the previous one, like a live session growing (a sibling of the last record would be a rewind)
    let mut prev = serde_json::from_str::<serde_json::Value>(&last).unwrap()["uuid"].as_str().unwrap().to_string();
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    let mut worst = std::time::Duration::ZERO;
    let mut total = std::time::Duration::ZERO;
    for i in 0..20 {
        let mut v: serde_json::Value = serde_json::from_str(&last).unwrap();
        let uuid = format!("timing-{i}");
        if std::env::var("SIBLING").is_err() {
            v["parentUuid"] = json!(prev);
        }
        v["uuid"] = json!(uuid);
        prev = uuid;
        writeln!(f, "{v}").unwrap();
        let t = std::time::Instant::now();
        r.poll().unwrap();
        let d = t.elapsed();
        worst = worst.max(d);
        total += d;
    }
    eprintln!("稳态 poll（有新内容）：平均 {:?}，最慢 {:?}", total / 20, worst);
    std::fs::remove_dir_all(&dir).ok();
}

// ───────────── incremental visibility (#269) ─────────────

/// Every way the visible set can change in one file: plain appends, tool results that do not move the leaf, two blocks of one reply,
/// a system record, a rewind, a compaction boundary, and a later block of a reply whose earlier sibling is off the chain.
fn tricky_transcript() -> Vec<String> {
    vec![
        user_str("u1", None, "start"),
        asst("a1", Some("u1"), "m1", text_block("one")),
        asst("a2", Some("a1"), "m2", tool_use("t1", "Read", json!({}))),
        asst("a3", Some("a2"), "m2", tool_use("t2", "Read", json!({}))),
        tool_result("r1", Some("a3"), "t1", json!("out1"), false),
        tool_result("r2", Some("a3"), "t2", json!("out2"), false),
        sys("s1", Some("a3"), "turn_duration", json!({"durationMs": 1})),
        asst("a4", Some("r2"), "m3", text_block("two")),
        user_str("u2", Some("a4"), "old question"),
        asst("a5", Some("u2"), "m4", text_block("old answer")),
        sys("s2", Some("a5"), "turn_duration", json!({"durationMs": 2})),
        user_str("u3", Some("a4"), "rewritten question"),
        asst("a6", Some("u3"), "m5", text_block("new answer")),
        sys("b1", None, "compact_boundary", json!({"logicalParentUuid": "a6"})),
        user_str("u4", Some("b1"), "after compaction"),
        asst("a7", Some("u4"), "m6", text_block("reply part 1")),
        asst("a8", Some("a7"), "m7", text_block("sibling")),
        asst("a9", Some("a7"), "m7", text_block("sibling again")),
        user_str("u5", Some("a9"), "more"),
        asst("a10", Some("u5"), "m8", text_block("done")),
    ]
}

fn visible_ixs(s: &State) -> Vec<usize> {
    (0..s.visible_len()).map(|i| s.visible_index(i)).collect()
}

#[test]
fn incremental_visibility_matches_a_full_recompute_at_every_step() {
    let lines = tricky_transcript();
    for batch in [1usize, 2, 3, 5] {
        let mut inc = State::new(Tool::Claude);
        let mut fed = 0;
        while fed < lines.len() {
            let before = visible_ixs(&inc);
            let to = (fed + batch).min(lines.len());
            for l in &lines[fed..to] {
                inc.feed_line(l);
            }
            fed = to;
            let prefix_kept = inc.recompute_visible();

            let mut full = State::new(Tool::Claude);
            for l in &lines[..fed] {
                full.feed_line(l);
            }
            full.recompute_visible();

            let now = visible_ixs(&inc);
            assert_eq!(now, visible_ixs(&full), "batch {batch}, after {fed} lines");
            let is_prefix = before.len() <= now.len() && now[..before.len()] == before[..];
            assert_eq!(prefix_kept, is_prefix, "batch {batch}, after {fed} lines: prefix flag");
        }
    }
}

#[test]
fn append_only_growth_does_not_recompute_the_whole_visible_set() {
    let mut s = State::new(Tool::Claude);
    let mut parent: Option<String> = None;
    for i in 0..200 {
        let (u, a) = (format!("u{i}"), format!("a{i}"));
        s.feed_line(&user_str(&u, parent.as_deref(), "q"));
        s.feed_line(&asst(&a, Some(&u), &format!("m{i}"), text_block("answer")));
        assert!(s.recompute_visible(), "appending must never invalidate the old prefix");
        parent = Some(a);
    }
    assert_eq!(s.visible_len(), 400);
    assert_eq!(s.full_recomputes(), 1, "only the first call (no chain known yet) may walk everything");
}
