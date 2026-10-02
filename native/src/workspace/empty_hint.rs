//! 首次打开 / 还没有任何会话时，欢迎卡顶部那块提示（#197）：告诉用户是「还没装 Claude Code / Codex」还是「装了但还没用过」，
//! 以及下一步做什么。
//!
//! 为什么单独测：这块文字错了，用户第一眼就被误导——明明装了却说没找到（以为 makit 坏了），或没装却让人去「新建会话」。
//! 另外**不能说「没安装」**：我们只能检测到命令在不在常见位置、会话目录在不在，检测不到不等于没装（可能装在 shell 配置的路径里）。

use makit_core::environment::ToolPresence;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EmptyHint {
    pub title: String,
    /// 说明，每条一段
    pub lines: Vec<String>,
    /// 检测结果一行：「Claude Code：已检测到　Codex：没检测到」
    pub status: String,
}

fn mark(found: bool) -> &'static str {
    if found { "已检测到" } else { "没检测到" }
}

/// 没加载完、或已经有会话 → 不显示。`new_tab_key` 是「新建终端」的键帽文字（从快捷键表取，不写死）
pub fn empty_hint(tools: &ToolPresence, session_count: usize, loaded: bool, new_tab_key: &str) -> Option<EmptyHint> {
    if !loaded || session_count > 0 {
        return None;
    }
    let status = format!("Claude Code：{}　Codex：{}", mark(tools.claude()), mark(tools.codex()));
    if !tools.any() {
        return Some(EmptyHint {
            title: "还没有找到 Claude Code 或 Codex".into(),
            lines: vec![
                "makit 管理本机的 Claude Code 和 Codex 会话，请先安装其中一个。".into(),
                "装好后在终端里运行 claude 或 codex 开始第一个会话，它会自动出现在左侧。".into(),
                "已经装过？可能装在我们没有检索的位置：直接在 makit 的终端里运行 claude / codex 也行，会话出现后会被识别。".into(),
            ],
            status,
        });
    }
    Some(EmptyHint {
        title: "还没有会话".into(),
        lines: vec![format!("按 {new_tab_key} 新建一个终端，运行 claude 或 codex 开始第一个会话，它会自动出现在左侧。")],
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tools(claude: bool, codex: bool) -> ToolPresence {
        ToolPresence { claude_bin: claude, codex_bin: codex, claude_data: false, codex_data: false }
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
        let t = ToolPresence { codex_data: true, ..Default::default() };
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
