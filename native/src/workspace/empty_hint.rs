//! 首次打开 / 还没有任何会话时，欢迎卡顶部那块提示（#197）：告诉用户是「还没装 Claude Code / Codex」还是「装了但还没用过」，
//! 以及下一步做什么。
//!
//! 为什么单独测：这块文字错了，用户第一眼就被误导——明明装了却说没找到（以为 makit 坏了），或没装却让人去「新建会话」。
//! 另外**不能说「没安装」**：我们只能检测到命令在不在常见位置、会话目录在不在，检测不到不等于没装（可能装在 shell 配置的路径里）。

use crate::ts;
use makit_core::environment::{ErrKind, ToolPresence};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmptyHint {
    pub title: String,
    /// 说明，每条一段
    pub lines: Vec<String>,
    /// 检测结果一行：「Claude Code：已检测到　Codex：没检测到」
    pub status: String,
}

fn mark(found: bool) -> String {
    if found { ts!("hint.detected") } else { ts!("hint.not_detected") }
}

/// 没加载完、或已经有会话 → 不显示。`new_tab_key` 是「新建终端」的键帽文字（从快捷键表取，不写死）
pub fn empty_hint(tools: &ToolPresence, session_count: usize, loaded: bool, new_tab_key: &str) -> Option<EmptyHint> {
    if !loaded || session_count > 0 {
        return None;
    }
    let status = format!("Claude Code：{}　Codex：{}", mark(tools.claude()), mark(tools.codex()));
    // 读不了最优先：这时「没有会话」是误导（#256 B1）。原因不明时只说「读取失败」，不断言是权限
    let unreadable = tools.unreadable_dirs();
    if !unreadable.is_empty() {
        let lines = unreadable
            .iter()
            .map(|(path, kind)| match kind {
                ErrKind::PermissionDenied => ts!("hint.read_denied", path = path),
                ErrKind::Other => ts!("hint.read_failed", path = path),
            })
            .collect();
        return Some(EmptyHint { title: ts!("sidebar.empty.unreadable").into(), lines, status });
    }
    if !tools.any() {
        return Some(EmptyHint {
            title: ts!("hint.not_found.title").into(),
            lines: vec![
                ts!("hint.not_found.line1").into(),
                ts!("hint.not_found.line2").into(),
                ts!("hint.not_found.line3").into(),
            ],
            status,
        });
    }
    Some(EmptyHint {
        title: ts!("hint.no_sessions.title").into(),
        lines: vec![ts!("hint.no_sessions.line", key = new_tab_key)],
        status,
    })
}

#[cfg(test)]
mod unreadable_tests {
    use super::*;
    use makit_core::environment::{DirState, ErrKind};

    /// 为什么要测（#256 B1）：目录读不了时，如果界面说「没有会话」，用户会以为自己没有会话，
    /// 而不是知道是系统拒绝了读取、该去查权限
    #[test]
    fn unreadable_dir_is_explained_not_called_empty() {
        let t = ToolPresence { claude_dir: DirState::Unreadable(ErrKind::PermissionDenied), ..Default::default() };
        let h = empty_hint(&t, 0, true, "⌘T").unwrap();
        assert_eq!(h.title, "读不了会话目录");
        let all = h.lines.join("\n");
        assert!(all.contains("~/.claude/projects") && all.contains("权限"), "{all}");
        assert!(!h.title.contains("没有会话") && !all.contains("还没有会话"), "不能说成没有会话：{h:?}");
    }

    #[test]
    fn other_read_errors_do_not_claim_permission() {
        let t = ToolPresence { codex_dir: DirState::Unreadable(ErrKind::Other), ..Default::default() };
        let h = empty_hint(&t, 0, true, "⌘T").unwrap();
        let all = h.lines.join("\n");
        assert!(all.contains("~/.codex/sessions") && all.contains("读取失败"), "{all}");
        assert!(!all.contains("权限"), "原因不明时别断言是权限：{all}");
    }

    #[test]
    fn both_unreadable_lists_both() {
        let t = ToolPresence {
            claude_dir: DirState::Unreadable(ErrKind::PermissionDenied),
            codex_dir: DirState::Unreadable(ErrKind::PermissionDenied),
            ..Default::default()
        };
        let all = empty_hint(&t, 0, true, "⌘T").unwrap().lines.join("\n");
        assert!(all.contains("~/.claude/projects") && all.contains("~/.codex/sessions"));
    }

    #[test]
    fn unreadable_is_still_hidden_while_loading_or_when_sessions_exist() {
        let t = ToolPresence { claude_dir: DirState::Unreadable(ErrKind::PermissionDenied), ..Default::default() };
        assert_eq!(empty_hint(&t, 0, false, "⌘T"), None, "还在加载");
        assert_eq!(empty_hint(&t, 5, true, "⌘T"), None, "另一个工具有会话时不打扰用户");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(claude: bool, codex: bool) -> ToolPresence {
        ToolPresence { claude_bin: claude, codex_bin: codex, ..Default::default() }
    }

    #[test]
    fn nothing_to_show_while_loading_or_when_there_are_sessions() {
        assert_eq!(empty_hint(&tools(false, false), 0, false, "⌘T"), None, "还没加载完：会话可能马上就出来，别先吓人");
        assert_eq!(empty_hint(&tools(true, true), 3, true, "⌘T"), None);
    }

    #[test]
    fn nothing_detected_tells_the_user_to_install_one_first() {
        let h = empty_hint(&tools(false, false), 0, true, "⌘T").unwrap();
        assert!(h.title.contains("还没有找到"), "{h:?}");
        let all = h.lines.join("\n");
        assert!(all.contains("安装") && all.contains("claude") && all.contains("codex"), "{all}");
        assert!(h.status.contains("Claude Code：没检测到") && h.status.contains("Codex：没检测到"), "{}", h.status);
    }

    #[test]
    fn installed_but_no_sessions_yet_points_to_new_terminal() {
        let h = empty_hint(&tools(true, false), 0, true, "⌘T").unwrap();
        assert_eq!(h.title, "还没有会话");
        assert!(h.lines.join("\n").contains("⌘T"), "要说怎么开始：{:?}", h.lines);
        assert!(h.status.contains("Claude Code：已检测到") && h.status.contains("Codex：没检测到"), "{}", h.status);
    }

    #[test]
    fn session_directory_alone_counts_as_installed() {
        let t = ToolPresence { codex_dir: makit_core::environment::DirState::Empty, ..Default::default() };
        let h = empty_hint(&t, 0, true, "⌘T").unwrap();
        assert_eq!(h.title, "还没有会话", "有 ~/.codex/sessions 就说明用过 Codex");
        assert!(h.status.contains("Codex：已检测到"));
    }

    #[test]
    fn never_claims_not_installed() {
        for t in [tools(false, false), tools(true, false), tools(false, true)] {
            let h = empty_hint(&t, 0, true, "⌘T").unwrap();
            let all = format!("{} {} {}", h.title, h.lines.join(" "), h.status);
            assert!(!all.contains("未安装") && !all.contains("没安装") && !all.contains("没有安装"), "检测不到不等于没装：{all}");
        }
    }
}
