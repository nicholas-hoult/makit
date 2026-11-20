//! Keyword search over a session's own messages (#212 一期: full-text search, keyword phase only -- the
//! second-phase "meaning" search described in the issue is a prompt-side trick for the skill, not more code here).
//!
//! Scope on purpose: this is the search *algorithm* only (matching + snippet extraction over items already in
//! memory). Scanning every session's transcript and caching their text is a separate piece (the issue's own
//! "正文缓存" step), left for when the ⌘K / sidebar entry points are wired up.
//!
//! Why char-based, not byte-based: the content is routinely Chinese, and `str::to_lowercase()` can change a
//! string's byte length (some characters expand under case folding), which would make a byte offset found in a
//! lowercased copy land on the wrong -- possibly non-boundary -- position in the original. Everything here works
//! in `Vec<char>` instead, with a lowercase lookup table built one-char-at-a-time so every position lines up
//! exactly between the original text and the search key. The one thing this gives up is multi-character lowercase
//! expansions (e.g. Turkish İ) correctly matching their multi-char form; for code and Chinese/English prose, the
//! only content this project searches, that case never comes up.

use crate::transcript::{Item, ItemKind};

/// Characters of context kept on each side of a match.
pub const SNIPPET_RADIUS: usize = 60;

/// One matching message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub item_id: String,
    pub snippet: String,
}

/// Case-insensitive keyword search across `items`' user / assistant text (#212: not tool output, thinking, or
/// notices -- those are not "what I said or what it said back"). Returns one `Hit` per matching item, in
/// document order. A blank `query` (empty or all whitespace) matches nothing -- it is not "show everything".
pub fn search_items(items: &[Item], query: &str) -> Vec<Hit> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let q_lower = lower_chars(q);
    items
        .iter()
        .filter_map(|it| {
            let text = match &it.kind {
                ItemKind::User(t) | ItemKind::Assistant(t) => t.as_str(),
                _ => return None,
            };
            snippet_around(text, &q_lower).map(|snippet| Hit { item_id: it.id.clone(), snippet })
        })
        .collect()
}

/// One message's text, pre-lowered once (#D-2134: a cross-session cache that outlives a single search must not
/// redo `lower_chars` on every one of its messages for every keystroke). Built by [`indexed_messages`].
pub struct Indexed {
    pub item_id: String,
    pub text: String,
    lower: Vec<char>,
}

impl Indexed {
    pub fn new(item_id: String, text: String) -> Self {
        let lower = lower_chars(&text);
        Self { item_id, text, lower }
    }
}

/// [`Indexed`] for `items`' user / assistant text only -- same filter as [`search_items`], pulled out so a cache
/// that keeps these around indefinitely never has to also keep the other kinds (tool output, thinking, notices)
/// just to re-derive this list later; it can drop an `Item` right after indexing it.
pub fn indexed_messages(items: &[Item]) -> Vec<Indexed> {
    items
        .iter()
        .filter_map(|it| match &it.kind {
            ItemKind::User(t) | ItemKind::Assistant(t) => Some(Indexed::new(it.id.clone(), t.clone())),
            _ => None,
        })
        .collect()
}

/// Same search as [`search_items`], over pre-indexed messages: only the query itself gets lowered here, not every
/// message's text (that already happened once in [`Indexed::new`]).
pub fn search_indexed(messages: &[Indexed], query: &str) -> Vec<Hit> {
    let q = query.trim();
    if q.is_empty() {
        return Vec::new();
    }
    let q_lower = lower_chars(q);
    messages.iter().filter_map(|m| snippet_from_lower(&m.text, &m.lower, &q_lower).map(|snippet| Hit { item_id: m.item_id.clone(), snippet })).collect()
}

/// One lowercase char per input char (see the module doc for why this, not `str::to_lowercase()`).
fn lower_chars(s: &str) -> Vec<char> {
    s.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

/// A snippet around the first case-insensitive occurrence of `query_lower` (already run through [`lower_chars`])
/// in `text`, padded to [`SNIPPET_RADIUS`] characters each side with an ellipsis wherever that cuts the text off.
/// `None` when there is no match.
pub fn snippet_around(text: &str, query_lower: &[char]) -> Option<String> {
    snippet_from_lower(text, &lower_chars(text), query_lower)
}

/// [`snippet_around`]'s actual work, taking `text`'s lowered form instead of computing it -- the one piece
/// [`search_indexed`] needed pulled out of `snippet_around` so a cached message's lowering is reused across every
/// query against it instead of being redone each time (#D-2134). `text.chars().collect()` still happens here, but
/// only once a match is already confirmed, not once per candidate message scanned.
fn snippet_from_lower(text: &str, lower: &[char], query_lower: &[char]) -> Option<String> {
    if query_lower.is_empty() || query_lower.len() > lower.len() {
        return None;
    }
    let match_idx = lower.windows(query_lower.len()).position(|w| w == query_lower)?;
    let chars: Vec<char> = text.chars().collect();
    let start = match_idx.saturating_sub(SNIPPET_RADIUS);
    let end = (match_idx + query_lower.len() + SNIPPET_RADIUS).min(chars.len());
    let slice: String = chars[start..end].iter().collect();
    let prefix = if start > 0 { "…" } else { "" };
    let suffix = if end < chars.len() { "…" } else { "" };
    Some(format!("{prefix}{slice}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(id: &str, text: &str) -> Item {
        Item { id: id.into(), time: None, kind: ItemKind::User(text.into()) }
    }
    fn assistant(id: &str, text: &str) -> Item {
        Item { id: id.into(), time: None, kind: ItemKind::Assistant(text.into()) }
    }

    #[test]
    fn finds_a_plain_match_with_context_on_both_sides() {
        let text = "the quick brown fox jumps over the lazy dog, again and again and again and again, repeatedly";
        let s = snippet_around(text, &lower_chars("fox")).unwrap();
        assert!(s.contains("fox"), "{s}");
        assert!(!s.starts_with('…'), "match is near the start, nothing was cut there: {s}");
    }

    #[test]
    fn is_case_insensitive() {
        assert!(snippet_around("The Quick Brown Fox", &lower_chars("quick")).is_some());
        assert!(snippet_around("the quick brown fox", &lower_chars("QUICK")).is_some());
    }

    #[test]
    fn no_match_returns_none() {
        assert_eq!(snippet_around("nothing relevant here", &lower_chars("xyz")), None);
    }

    /// `snippet_around` itself only rejects a truly empty query (an actual char to search for, even a space, is
    /// a legitimate match target); rejecting a *blank* query (whitespace-only) is `search_items`' policy, tested
    /// separately below, not this lower-level function's.
    #[test]
    fn empty_query_never_matches() {
        assert_eq!(snippet_around("anything", &lower_chars("")), None);
    }

    #[test]
    fn query_longer_than_the_text_does_not_panic_and_does_not_match() {
        assert_eq!(snippet_around("short", &lower_chars("this query is much longer than the text")), None);
    }

    /// Why: Chinese has no case, but it is multi-byte UTF-8 -- a naive byte-offset snippet would either panic
    /// slicing mid-character or silently corrupt the text. This is the primary thing this module exists to get right.
    #[test]
    fn chinese_text_matches_and_slices_on_character_boundaries_not_bytes() {
        let text = "总管说这次的任务是修复会话恢复假成功的问题，顺便把内存误判也一起解决了，改动都已经合并到主分支";
        let s = snippet_around(text, &lower_chars("假成功")).unwrap();
        assert!(s.contains("假成功"), "{s}");
        // every char in the snippet must be a real character we can round-trip, not a half-decoded byte
        assert!(s.chars().count() > 0 && std::str::from_utf8(s.as_bytes()).is_ok());
    }

    /// 真实对话里最常见的形状：中文句子里混着英文术语（函数名、issue 号、工具名）。既要在混排文本里按大小写不敏感
    /// 搜到英文词，也要搜中文词时不受旁边英文的影响；片段必须是合法 UTF-8、不切坏任何一边
    #[test]
    fn mixed_chinese_and_english_matches_either_script_without_corrupting_the_other() {
        let text = "总管说这次的任务是修复 Resume 这个 Bug，顺便看一下 #212 的 ⌘K search 要不要一起改，改完 cargo test 要全绿";
        for (needle, must_contain) in [("bug", "Bug"), ("RESUME", "Resume"), ("总管", "总管"), ("cargo test", "cargo test"), ("#212", "#212")] {
            let s = snippet_around(text, &lower_chars(needle)).expect(needle);
            assert!(s.contains(must_contain), "query {needle:?} -> {s:?} should still contain {must_contain:?}");
            assert!(std::str::from_utf8(s.as_bytes()).is_ok(), "query {needle:?} produced invalid utf8: {s:?}");
        }
        // 贴着中英文边界的词：确认边界两侧都完整
        let s = snippet_around(text, &lower_chars("务是修复 Resume 这个")).unwrap();
        assert!(s.contains("务是修复 Resume 这个"));
    }

    #[test]
    fn match_at_the_very_start_or_end_has_no_ellipsis_on_that_side() {
        let short = "fox";
        let s = snippet_around(short, &lower_chars("fox")).unwrap();
        assert_eq!(s, "fox", "whole text is the match, no ellipsis either side: {s:?}");
    }

    #[test]
    fn long_text_gets_truncated_with_an_ellipsis_on_the_far_side() {
        let text = format!("{}needle{}", "a".repeat(200), "b".repeat(200));
        let s = snippet_around(&text, &lower_chars("needle")).unwrap();
        assert!(s.starts_with('…') && s.ends_with('…'), "{s}");
        assert!(s.len() < text.len(), "actually truncated: {} vs {}", s.len(), text.len());
    }

    #[test]
    fn search_items_only_looks_at_user_and_assistant_text() {
        let items = vec![
            user("u1", "where did I leave the makit resume fix"),
            Item { id: "t1".into(), time: None, kind: ItemKind::ToolCall { call_id: "c1".into(), name: "Bash".into(), input: serde_json::json!({"cmd": "resume fix"}), result: None } },
            Item { id: "n1".into(), time: None, kind: ItemKind::Notice { level: crate::transcript::Level::Info, text: "resume fix notice".into() } },
            assistant("a1", "the resume fix is in link_switch.rs"),
        ];
        let hits = search_items(&items, "resume fix");
        assert_eq!(hits.iter().map(|h| h.item_id.as_str()).collect::<Vec<_>>(), vec!["u1", "a1"], "tool calls and notices are not searched even though they contain the same words");
    }

    #[test]
    fn search_items_preserves_document_order_and_a_blank_query_matches_nothing() {
        let items = vec![user("u1", "alpha"), assistant("a1", "beta"), user("u2", "alpha again")];
        let hits = search_items(&items, "alpha");
        assert_eq!(hits.iter().map(|h| h.item_id.as_str()).collect::<Vec<_>>(), vec!["u1", "u2"]);
        assert_eq!(search_items(&items, ""), vec![]);
        assert_eq!(search_items(&items, "   "), vec![]);
    }

    #[test]
    fn indexed_messages_filters_the_same_way_as_search_items_and_search_indexed_agrees_with_search_items() {
        let items = vec![
            user("u1", "where did I leave the makit resume fix"),
            Item { id: "t1".into(), time: None, kind: ItemKind::ToolCall { call_id: "c1".into(), name: "Bash".into(), input: serde_json::json!({"cmd": "resume fix"}), result: None } },
            assistant("a1", "the resume fix is in link_switch.rs"),
        ];
        let indexed = indexed_messages(&items);
        assert_eq!(indexed.iter().map(|m| m.item_id.as_str()).collect::<Vec<_>>(), vec!["u1", "a1"], "same user/assistant-only filter as search_items");
        assert_eq!(search_indexed(&indexed, "resume fix"), search_items(&items, "resume fix"), "pre-indexing must not change what a query finds");
        assert_eq!(search_indexed(&indexed, ""), vec![]);
        assert_eq!(search_indexed(&indexed, "   "), vec![]);
    }

    #[test]
    fn indexed_lowering_happens_once_and_every_later_query_still_sees_it() {
        // 片段里会有中文；确认预先小写的那份对后面每一次查询都仍然有效，而且不会因为反复查询而出问题
        let m = Indexed::new("u1".into(), "总管说 Resume 这个 Bug 最急，先修".into());
        assert_eq!(search_indexed(std::slice::from_ref(&m), "resume").len(), 1);
        assert_eq!(search_indexed(std::slice::from_ref(&m), "BUG").len(), 1);
        assert_eq!(search_indexed(std::slice::from_ref(&m), "总管").len(), 1);
        assert_eq!(search_indexed(std::slice::from_ref(&m), "没有的词").len(), 0);
    }

    /// #212 验收标准："Claude 和 Codex 的会话都能搜到"——一直只在代码路径上说得通（`open_transcript`/
    /// `TranscriptReader` 两种工具通用），没有拿真实 Codex 会话跑过。`#[ignore]`，和 `transcript` 模块自己那条
    /// 真实文件测试一个套路：测的是本机真实 `~/.codex/sessions`，不是 fixture，不进 CI。
    /// 跑：`cargo test --manifest-path core/Cargo.toml codex_session_content_is_searchable -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn codex_session_content_is_searchable_on_real_data() {
        use crate::transcript::{Tool, TranscriptReader};
        use std::path::PathBuf;

        let Some(home) = dirs::home_dir() else { return };
        let codex_dir = home.join(".codex").join("sessions");
        if !codex_dir.exists() {
            return;
        }
        let mut files: Vec<PathBuf> = Vec::new();
        fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
            let Ok(entries) = std::fs::read_dir(dir) else { return };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                    out.push(p);
                }
            }
        }
        walk(&codex_dir, &mut files);
        if files.is_empty() {
            return;
        }

        // 每个会话都扫一遍：只要有任意一个会话能搜到任意一个在它自己正文里真实出现过的词，就证明
        // Codex 的内容确实进得了 search_items 这条路径，不是只在 Claude 侧测过
        let mut sessions_checked = 0usize;
        let mut found_any_hit = false;
        for path in &files {
            let mut reader = TranscriptReader::new(path.clone(), Tool::Codex);
            if reader.poll().is_err() {
                continue;
            }
            let items: Vec<_> = (0..reader.len()).map(|i| reader.item(i).clone()).collect();
            if items.is_empty() {
                continue;
            }
            sessions_checked += 1;
            // 从这个会话自己的 user 文本里挑一个真实出现过的词（取第一个够长的词），用它反过来搜索这同一个会话
            let word = items.iter().find_map(|it| match &it.kind {
                ItemKind::User(t) => t.split_whitespace().find(|w| w.chars().count() >= 4),
                _ => None,
            });
            if let Some(w) = word {
                let hits = search_items(&items, w);
                if !hits.is_empty() {
                    found_any_hit = true;
                    println!("codex session {:?}: query {:?} -> {} hit(s)", path.file_name().unwrap(), w, hits.len());
                    break;
                }
            }
        }
        println!("checked {sessions_checked} real codex sessions, found_any_hit={found_any_hit}");
        assert!(sessions_checked > 0, "no codex session on this machine could be read at all");
        assert!(found_any_hit, "none of the {sessions_checked} codex sessions checked produced a self-match -- search_items may not actually work on codex's own Item shapes");
    }
}
