//! 全部 action 和快捷键表 —— **单一数据源**（修 #224 里「两份快捷键表」）。
//!
//! - action 按归属包分组声明在这里（`actions!`），处理函数在各包自己的视图里 `.on_action(...)`。
//! - `keymap()` 是唯一的快捷键表：启动时整张绑定（`bind_all`），欢迎卡 / 设置里展示快捷键也读它。
//! - 还没实现的 action 在根视图里挂了占位处理（打一行「[未实现] …归 X 包」），各包实现时把占位删掉、
//!   在自己的视图上 `.on_action` —— GPUI 按焦点链冒泡，离焦点近的先处理。
//!
//! **加一个快捷键**：① 在对应包的 `actions!` 列表里加 action；② 在 `keymap()` 里加一行
//! `sc("cmd-x", None, Owner::Xxx, "说明", Xxx)`；③ 在处理它的视图上 `.on_action(cx.listener(...))`。
//! 组件内部的键（侧栏 ↑↓、面板 Enter……）也写进这张表，用 `context` 限定到组件的 key_context。
//! `tests` 会检查同一 context 下不重键。

use gpui::{Action, App, KeyBinding};

/// 快捷键归哪个包实现（对应 TRD #226 的并行拆分）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// 地基：已实现
    F0,
    /// A 终端
    Terminal,
    /// B 侧栏
    Sidebar,
    /// C 工作区（F0 已实现基本分屏 / 标签切换，细节归 C）
    Workspace,
    /// D 浮层（⌘K、搜索条、设置、详情……）
    Overlays,
    /// E 通知
    Notify,
}

impl Owner {
    pub fn label(self) -> &'static str {
        match self {
            Owner::F0 => "F0 foundation",
            Owner::Terminal => "A terminal",
            Owner::Sidebar => "B sidebar",
            Owner::Workspace => "C workspace",
            Owner::Overlays => "D overlays",
            Owner::Notify => "E notify",
        }
    }
}

/// 应用级（F0）
pub mod app {
    gpui::actions!(makit, [Quit, Refresh, ToggleSidebar]);
}

/// 工作区（C；F0 已实现的在 workspace/mod.rs）
pub mod workspace {
    gpui::actions!(
        workspace,
        [
            NewTab, CloseTab, SplitRight, SplitDown, NextTab, PrevTab, ToggleMaximize,
            ActivateTab1, ActivateTab2, ActivateTab3, ActivateTab4, ActivateTab5, ActivateTab6, ActivateTab7, ActivateTab8, ActivateTab9,
            FocusPane1, FocusPane2, FocusPane3, FocusPane4, FocusPane5, FocusPane6, FocusPane7, FocusPane8, FocusPane9,
            FocusPaneLeft, FocusPaneRight, FocusPaneUp, FocusPaneDown,
        ]
    );
}

/// 侧栏（B）
pub mod sidebar {
    gpui::actions!(
        sidebar,
        [
            FocusSearch, RevealActive, ToggleAllGroups,
            // 会话列表（context "SessionList"，只在侧栏列表有焦点时生效，D7）
            SelectNext, SelectPrev, OpenSelected, OpenSelectedSplitRight, OpenSelectedSplitDown, ToggleHoverCard,
            CollapseGroup, ExpandGroup, ClearSelection,
            // 搜索框（context "SidebarSearch"）
            SearchToList, SearchBackspace, SearchDelete, SearchLeft, SearchRight, SearchSelectLeft, SearchSelectRight,
            SearchSelectAll, SearchHome, SearchEnd, SearchPaste, SearchCopy, SearchCut,
            // 侧栏自己的浮层（显示选项 / 右键菜单 / 工具选择器，context "SidebarMenu"）
            DismissMenu,
        ]
    );
}

/// 浮层（D）。`Dismiss` / `Confirm` / `Select*` 是所有浮层共用的面板内键（context `Overlay`，
/// 每个浮层的 key_context 里都带 `Overlay`，各自 `.on_action` 决定怎么响应）；
/// `Input*` 是单行输入框（`overlays::text_input`，context `TextInput`）的编辑键。
pub mod overlays {
    gpui::actions!(
        overlays,
        [
            TogglePalette, OpenSettings, FindInTerminal,
            Dismiss, Confirm, ConfirmSplitRight, ConfirmSplitDown, ConfirmReverse, SelectPrev, SelectNext,
            InputBackspace, InputDelete, InputDeleteToStart, InputLeft, InputRight, InputSelectLeft, InputSelectRight,
            InputSelectAll, InputHome, InputEnd, InputSelectHome, InputSelectEnd, InputPaste, InputCopy, InputCut,
        ]
    );
}

/// 通知（E）。SelectNext / SelectPrev / Confirm / Dismiss 只在抽屉有焦点时（context `NotificationCenter`）
pub mod notify {
    gpui::actions!(notify, [ToggleNotificationCenter, SelectNext, SelectPrev, Confirm, Dismiss]);
}

/// 终端（A）。Copy / Paste / 翻页沿用原型在 terminal 模块里的声明
pub mod terminal {
    pub use crate::terminal::{Copy, Paste, ScrollPageDown, ScrollPageUp};
    gpui::actions!(terminal, [FontIncrease, FontDecrease, FontReset]);
}

pub struct Shortcut {
    /// GPUI 键位写法：`cmd-shift-d`、`alt-cmd-left`
    pub keys: &'static str,
    /// None = 全局；Some("Terminal") = 只在终端获得焦点时
    pub context: Option<&'static str>,
    pub owner: Owner,
    pub desc: &'static str,
    pub action: Box<dyn Action>,
    binding: KeyBinding,
}

fn sc<A: Action + Clone>(keys: &'static str, context: Option<&'static str>, owner: Owner, desc: &'static str, a: A) -> Shortcut {
    Shortcut { keys, context, owner, desc, action: a.boxed_clone(), binding: KeyBinding::new(keys, a, context) }
}

/// 快捷键总表（界面清单 O 节）。顺序即展示顺序。
pub fn keymap() -> Vec<Shortcut> {
    use workspace as w;
    use Owner::*;
    const T: Option<&str> = Some("Terminal");
    // B 侧栏的三个组件 context（见 sidebar/mod.rs）
    const L: Option<&str> = Some("SessionList");
    const S: Option<&str> = Some("SidebarSearch");
    const M: Option<&str> = Some("SidebarMenu");
    const OV: Option<&str> = Some("Overlay");
    const TI: Option<&str> = Some("TextInput");
    const N: Option<&str> = Some("NotificationCenter");
    vec![
        sc("cmd-k", None, Overlays, "shortcut.command_palette", overlays::TogglePalette),
        sc("cmd-f", None, Overlays, "shortcut.search_in_the_current_terminal", overlays::FindInTerminal),
        sc("cmd-shift-f", None, Sidebar, "shortcut.focus_sidebar_search", sidebar::FocusSearch),
        sc("cmd-l", None, Sidebar, "shortcut.locate_the_current_session_in_the_sidebar", sidebar::RevealActive),
        sc("cmd-shift-e", None, Sidebar, "shortcut.sidebar_expand_collapse_all_groups", sidebar::ToggleAllGroups),
        // ---- B 侧栏：组件内部的键（D7）----
        sc("down", L, Sidebar, "shortcut.sidebar_next_item", sidebar::SelectNext),
        sc("up", L, Sidebar, "shortcut.sidebar_previous_item", sidebar::SelectPrev),
        sc("enter", L, Sidebar, "shortcut.sidebar_open_the_selected_session", sidebar::OpenSelected),
        sc("cmd-enter", L, Sidebar, "shortcut.sidebar_open_in_a_left_right_split", sidebar::OpenSelectedSplitRight),
        sc("shift-enter", L, Sidebar, "shortcut.sidebar_open_in_a_top_bottom_split", sidebar::OpenSelectedSplitDown),
        sc("space", L, Sidebar, "shortcut.sidebar_toggle_the_hover_card", sidebar::ToggleHoverCard),
        sc("left", L, Sidebar, "shortcut.sidebar_collapse_the_current_group", sidebar::CollapseGroup),
        sc("right", L, Sidebar, "shortcut.sidebar_expand_the_current_group", sidebar::ExpandGroup),
        sc("escape", L, Sidebar, "shortcut.sidebar_clear_the_selection_and_return_to_the_terminal", sidebar::ClearSelection),
        sc("down", S, Sidebar, "shortcut.search_box_go_to_the_list", sidebar::SearchToList),
        sc("backspace", S, Sidebar, "shortcut.search_box_delete_previous_character", sidebar::SearchBackspace),
        sc("delete", S, Sidebar, "shortcut.search_box_delete_next_character", sidebar::SearchDelete),
        sc("left", S, Sidebar, "shortcut.search_box_cursor_left", sidebar::SearchLeft),
        sc("right", S, Sidebar, "shortcut.search_box_cursor_right", sidebar::SearchRight),
        sc("shift-left", S, Sidebar, "shortcut.search_box_select_left", sidebar::SearchSelectLeft),
        sc("shift-right", S, Sidebar, "shortcut.search_box_select_right", sidebar::SearchSelectRight),
        sc("cmd-a", S, Sidebar, "shortcut.search_box_select_all", sidebar::SearchSelectAll),
        sc("home", S, Sidebar, "shortcut.search_box_to_line_start", sidebar::SearchHome),
        sc("cmd-left", S, Sidebar, "shortcut.search_box_to_line_start", sidebar::SearchHome),
        sc("end", S, Sidebar, "shortcut.search_box_to_line_end", sidebar::SearchEnd),
        sc("cmd-right", S, Sidebar, "shortcut.search_box_to_line_end", sidebar::SearchEnd),
        // 单行框里 ⌘↑↓ = 到开头 / 结尾（原生输入框行为）；不绑的话会落到全局去切标签（Tauri 在文本框里让给光标）
        sc("cmd-up", S, Sidebar, "shortcut.search_box_to_line_start", sidebar::SearchHome),
        sc("cmd-down", S, Sidebar, "shortcut.search_box_to_line_end", sidebar::SearchEnd),
        sc("cmd-v", S, Sidebar, "shortcut.search_box_paste", sidebar::SearchPaste),
        sc("cmd-c", S, Sidebar, "shortcut.search_box_copy", sidebar::SearchCopy),
        sc("cmd-x", S, Sidebar, "shortcut.search_box_cut", sidebar::SearchCut),
        sc("escape", M, Sidebar, "shortcut.close_the_sidebar_menu", sidebar::DismissMenu),
        sc("cmd-b", None, F0, "shortcut.collapse_expand_the_sidebar", app::ToggleSidebar),
        sc("cmd-r", None, F0, "shortcut.refresh_the_session_list", app::Refresh),
        sc("cmd-q", None, F0, "shortcut.quit", app::Quit),
        sc("cmd-t", None, Workspace, "shortcut.new_terminal", w::NewTab),
        sc("cmd-w", None, Workspace, "shortcut.close_the_current_tab", w::CloseTab),
        sc("cmd-d", None, Workspace, "shortcut.split_left_right", w::SplitRight),
        sc("cmd-shift-d", None, Workspace, "shortcut.split_top_bottom", w::SplitDown),
        sc("alt-cmd-enter", None, Workspace, "shortcut.maximize_restore_the_current_pane", w::ToggleMaximize),
        sc("cmd-]", None, Workspace, "shortcut.next_tab", w::NextTab),
        sc("cmd-[", None, Workspace, "shortcut.previous_tab", w::PrevTab),
        // ⌘⇧] / ⌘⇧[（Safari 惯例）：macOS 上 GPUI 报成 cmd-} / cmd-{，写 shift-] 永远匹配不上
        sc("cmd-}", None, Workspace, "shortcut.next_tab", w::NextTab),
        sc("cmd-{", None, Workspace, "shortcut.previous_tab", w::PrevTab),
        sc("cmd-right", None, Workspace, "shortcut.next_tab", w::NextTab),
        sc("cmd-down", None, Workspace, "shortcut.next_tab", w::NextTab),
        sc("cmd-left", None, Workspace, "shortcut.previous_tab", w::PrevTab),
        sc("cmd-up", None, Workspace, "shortcut.previous_tab", w::PrevTab),
        sc("ctrl-tab", None, Workspace, "shortcut.next_tab", w::NextTab),
        sc("ctrl-shift-tab", None, Workspace, "shortcut.previous_tab", w::PrevTab),
        sc("cmd-1", None, Workspace, "shortcut.tab_1", w::ActivateTab1),
        sc("cmd-2", None, Workspace, "shortcut.tab_2", w::ActivateTab2),
        sc("cmd-3", None, Workspace, "shortcut.tab_3", w::ActivateTab3),
        sc("cmd-4", None, Workspace, "shortcut.tab_4", w::ActivateTab4),
        sc("cmd-5", None, Workspace, "shortcut.tab_5", w::ActivateTab5),
        sc("cmd-6", None, Workspace, "shortcut.tab_6", w::ActivateTab6),
        sc("cmd-7", None, Workspace, "shortcut.tab_7", w::ActivateTab7),
        sc("cmd-8", None, Workspace, "shortcut.tab_8", w::ActivateTab8),
        sc("cmd-9", None, Workspace, "shortcut.tab_9", w::ActivateTab9),
        sc("alt-cmd-1", None, Workspace, "shortcut.pane_1", w::FocusPane1),
        sc("alt-cmd-2", None, Workspace, "shortcut.pane_2", w::FocusPane2),
        sc("alt-cmd-3", None, Workspace, "shortcut.pane_3", w::FocusPane3),
        sc("alt-cmd-4", None, Workspace, "shortcut.pane_4", w::FocusPane4),
        sc("alt-cmd-5", None, Workspace, "shortcut.pane_5", w::FocusPane5),
        sc("alt-cmd-6", None, Workspace, "shortcut.pane_6", w::FocusPane6),
        sc("alt-cmd-7", None, Workspace, "shortcut.pane_7", w::FocusPane7),
        sc("alt-cmd-8", None, Workspace, "shortcut.pane_8", w::FocusPane8),
        sc("alt-cmd-9", None, Workspace, "shortcut.pane_9", w::FocusPane9),
        sc("alt-cmd-left", None, Workspace, "shortcut.pane_to_the_left", w::FocusPaneLeft),
        sc("alt-cmd-right", None, Workspace, "shortcut.pane_to_the_right", w::FocusPaneRight),
        sc("alt-cmd-up", None, Workspace, "shortcut.pane_above", w::FocusPaneUp),
        sc("alt-cmd-down", None, Workspace, "shortcut.pane_below", w::FocusPaneDown),
        sc("cmd-i", None, Notify, "shortcut.notification_center", notify::ToggleNotificationCenter),
        sc("down", N, Notify, "shortcut.notification_center_next_item", notify::SelectNext),
        sc("up", N, Notify, "shortcut.notification_center_previous_item", notify::SelectPrev),
        sc("enter", N, Notify, "shortcut.notification_center_mark_as_read_and_jump", notify::Confirm),
        sc("escape", N, Notify, "shortcut.notification_center_close", notify::Dismiss),
        sc("cmd-,", None, Overlays, "shortcut.settings", overlays::OpenSettings),
        // 字号：全局（同 Tauri，作用在当前标签的终端；焦点在侧栏 / 面板里也生效）。⌘+ = ⌘⇧=，macOS 报成 cmd-+
        sc("cmd-=", None, Terminal, "shortcut.font_size_1", terminal::FontIncrease),
        sc("cmd-+", None, Terminal, "shortcut.font_size_1", terminal::FontIncrease),
        sc("cmd--", None, Terminal, "shortcut.font_size_1_2", terminal::FontDecrease),
        sc("cmd-0", None, Terminal, "shortcut.reset_font_size_to_13", terminal::FontReset),
        sc("cmd-c", T, Terminal, "shortcut.copy", terminal::Copy),
        sc("cmd-v", T, Terminal, "shortcut.paste", terminal::Paste),
        sc("shift-pageup", T, Terminal, "shortcut.scrollback_page_up", terminal::ScrollPageUp),
        sc("shift-pagedown", T, Terminal, "shortcut.scrollback_page_down", terminal::ScrollPageDown),
        // ---- D 浮层：面板内（context `Overlay`，所有浮层共用）----
        sc("escape", OV, Overlays, "shortcut.close_panel_cancel", overlays::Dismiss),
        sc("enter", OV, Overlays, "shortcut.open_confirm", overlays::Confirm),
        sc("cmd-enter", OV, Overlays, "shortcut.command_palette_open_in_a_left_right_split", overlays::ConfirmSplitRight),
        sc("cmd-shift-enter", OV, Overlays, "shortcut.command_palette_open_in_a_top_bottom_split", overlays::ConfirmSplitDown),
        sc("shift-enter", OV, Overlays, "shortcut.search_bar_previous_match", overlays::ConfirmReverse),
        sc("up", OV, Overlays, "shortcut.previous_item", overlays::SelectPrev),
        sc("down", OV, Overlays, "shortcut.next_item", overlays::SelectNext),
        // ---- D 浮层：单行输入框的编辑键（context `TextInput`，设置页的快捷键表不展示这组）----
        sc("backspace", TI, Overlays, "shortcut.delete_previous_character", overlays::InputBackspace),
        sc("delete", TI, Overlays, "shortcut.delete_next_character", overlays::InputDelete),
        sc("cmd-backspace", TI, Overlays, "shortcut.delete_to_line_start", overlays::InputDeleteToStart),
        sc("left", TI, Overlays, "shortcut.cursor_left", overlays::InputLeft),
        sc("right", TI, Overlays, "shortcut.cursor_right", overlays::InputRight),
        sc("shift-left", TI, Overlays, "shortcut.select_left", overlays::InputSelectLeft),
        sc("shift-right", TI, Overlays, "shortcut.select_right", overlays::InputSelectRight),
        sc("cmd-a", TI, Overlays, "shortcut.select_all", overlays::InputSelectAll),
        sc("home", TI, Overlays, "shortcut.line_start", overlays::InputHome),
        sc("end", TI, Overlays, "shortcut.line_end", overlays::InputEnd),
        sc("cmd-left", TI, Overlays, "shortcut.line_start", overlays::InputHome),
        sc("cmd-right", TI, Overlays, "shortcut.line_end", overlays::InputEnd),
        sc("cmd-up", TI, Overlays, "shortcut.line_start", overlays::InputHome),
        sc("cmd-down", TI, Overlays, "shortcut.line_end", overlays::InputEnd),
        sc("cmd-shift-left", TI, Overlays, "shortcut.select_to_line_start", overlays::InputSelectHome),
        sc("cmd-shift-right", TI, Overlays, "shortcut.select_to_line_end", overlays::InputSelectEnd),
        sc("cmd-v", TI, Overlays, "shortcut.paste", overlays::InputPaste),
        sc("cmd-c", TI, Overlays, "shortcut.copy", overlays::InputCopy),
        sc("cmd-x", TI, Overlays, "shortcut.cut", overlays::InputCut),
    ]
}

/// 启动时调一次：整张表绑进 GPUI
pub fn bind_all(cx: &mut App) {
    cx.bind_keys(keymap().into_iter().map(|s| s.binding));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_duplicate_keys_in_the_same_context() {
        let km = keymap();
        let mut seen = std::collections::HashSet::new();
        for s in &km {
            assert!(seen.insert((s.keys, s.context)), "{} 在 {:?} 里绑了两次", s.keys, s.context);
        }
    }

    /// macOS 上按 ⇧ + 符号键，GPUI 报出来的是「移位后的字符、去掉 shift」：⌘⇧] → `cmd-}`、⌘⇧= → `cmd-+`
    /// （gpui-0.2.2 platform/mac/events.rs:437-446；按着 ⌘ 时没有 key_char 兜底，只按 key + 修饰键精确比）。
    /// 所以写成 `shift-]` / `shift-=` 的绑定永远按不出来。错了在界面上就是「⌘⇧] 切标签、⌘+ 放大字号没反应」
    #[test]
    fn no_shift_plus_symbol_bindings_that_macos_never_reports() {
        const SHIFTABLE: &str = "`1234567890-=[]\\;',./";
        for s in keymap() {
            let parts: Vec<&str> = s.keys.split('-').collect();
            // 「cmd--」这种键本身是 '-' 的：最后两段是 "" 和 ""
            let key = if s.keys.ends_with("--") { "-" } else { parts.last().copied().unwrap_or("") };
            let shift = parts.iter().rev().skip(1).any(|p| *p == "shift");
            assert!(!(shift && key.chars().count() == 1 && SHIFTABLE.contains(key)), "{} 在 macOS 上按不出来，要写成移位后的字符（如 cmd-}}）", s.keys);
        }
        let km = keymap();
        let action_of = |k: &str| km.iter().find(|s| s.keys == k).map(|s| s.action.name());
        assert_eq!(action_of("cmd-}"), Some("workspace::NextTab"));
        assert_eq!(action_of("cmd-{"), Some("workspace::PrevTab"));
        assert_eq!(action_of("cmd-+"), Some("terminal::FontIncrease"));
    }

    #[test]
    fn covers_the_checklist_o_table() {
        let km = keymap();
        let has = |k: &str| km.iter().any(|s| s.keys == k);
        for k in [
            "cmd-k", "cmd-f", "cmd-shift-f", "cmd-w", "cmd-d", "cmd-shift-d", "cmd-t", "cmd-l", "cmd-1", "cmd-9", "alt-cmd-1",
            "alt-cmd-9", "alt-cmd-left", "alt-cmd-down", "alt-cmd-enter", "cmd-left", "cmd-down", "cmd-[", "cmd-]",
            // ⌘⇧[ / ⌘⇧]：macOS 上 GPUI 报成 cmd-{ / cmd-}（见下一个测试）
            "cmd-{", "cmd-}", "ctrl-tab", "ctrl-shift-tab", "cmd-i", "cmd-,", "cmd-r", "cmd-b", "cmd-=", "cmd--", "cmd-0",
        ] {
            assert!(has(k), "清单 O 节的 {k} 没进快捷键表");
        }
        // 同一个 action 的多个键位描述一致（欢迎卡 / 设置按 action 分组展示）
        for a in &km {
            for b in &km {
                if a.action.partial_eq(b.action.as_ref()) {
                    assert_eq!(a.desc, b.desc, "{} 和 {} 是同一个 action", a.keys, b.keys);
                }
            }
        }
    }
}
