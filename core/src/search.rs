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

/// One lowercase char per input char (see the module doc for why this, not `str::to_lowercase()`).
fn lower_chars(s: &str) -> Vec<char> {
    s.chars().map(|c| c.to_lowercase().next().unwrap_or(c)).collect()
}

/// A snippet around the first case-insensitive occurrence of `query_lower` (already run through [`lower_chars`])
/// in `text`, padded to [`SNIPPET_RADIUS`] characters each side with an ellipsis wherever that cuts the text off.
/// `None` when there is no match.
pub fn snippet_around(text: &str, query_lower: &[char]) -> Option<String> {
    if query_lower.is_empty() {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    if query_lower.len() > chars.len() {
        return None;
    }
    let lower = lower_chars(text);
    let match_idx = lower.windows(query_lower.len()).position(|w| w == query_lower)?;

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
}
