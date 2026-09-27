//! 会话详情面板的纯逻辑：头部副标题、消息时间戳、正序 / 倒序（照 App.tsx:2194-2252）。

use makit_core::SessionMeta;

/// 时间戳取前 19 位、T 换成空格（`2026-09-27T10:11:12.345Z` → `2026-09-27 10:11:12`）
pub fn format_ts(ts: &str) -> String {
    ts.chars().take(19).collect::<String>().replacen('T', " ", 1)
}

/// 「mtime (humanize) · N 条用户消息 · M 条对话 · @branch · worktree」
pub fn subtitle(s: &SessionMeta, n_messages: usize) -> String {
    let mut out = format!("{} ({}) · {} 条用户消息 · {} 条对话", s.mtime_display, s.humanize, s.user_msg_count, n_messages);
    if !s.git_branch.is_empty() {
        out.push_str(&format!(" · @{}", s.git_branch));
    }
    if s.is_worktree {
        out.push_str(" · worktree");
    }
    out
}

/// 显示顺序的下标。默认倒序（新 → 旧）
pub fn display_order(len: usize, reversed: bool) -> Vec<usize> {
    if reversed { (0..len).rev().collect() } else { (0..len).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_is_cut_to_seconds() {
        assert_eq!(format_ts("2026-09-27T10:11:12.345Z"), "2026-09-27 10:11:12");
        assert_eq!(format_ts("2026-09-27"), "2026-09-27", "不够 19 位原样");
        assert_eq!(format_ts(""), "");
        assert_eq!(format_ts("时间时间时间时间时间时间时间时间时间时间"), "时间时间时间时间时间时间时间时间时间时", "按字符截，不切坏 UTF-8");
    }

    #[test]
    fn subtitle_has_optional_parts() {
        let mut s = crate::state::sessions::tests::session("x");
        s.mtime_display = "09-27 10:00".into();
        s.humanize = "3 分钟前".into();
        s.user_msg_count = 4;
        assert_eq!(subtitle(&s, 9), "09-27 10:00 (3 分钟前) · 4 条用户消息 · 9 条对话");
        s.git_branch = "main".into();
        s.is_worktree = true;
        assert_eq!(subtitle(&s, 9), "09-27 10:00 (3 分钟前) · 4 条用户消息 · 9 条对话 · @main · worktree");
    }

    #[test]
    fn order_defaults_new_first() {
        assert_eq!(display_order(3, true), vec![2, 1, 0]);
        assert_eq!(display_order(3, false), vec![0, 1, 2]);
        assert!(display_order(0, true).is_empty());
    }
}
