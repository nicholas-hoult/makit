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
            Owner::F0 => "F0 地基",
            Owner::Terminal => "A 终端",
            Owner::Sidebar => "B 侧栏",
            Owner::Workspace => "C 工作区",
            Owner::Overlays => "D 浮层",
            Owner::Notify => "E 通知",
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
        sc("cmd-k", None, Overlays, "命令面板", overlays::TogglePalette),
        sc("cmd-f", None, Overlays, "当前终端内搜索", overlays::FindInTerminal),
        sc("cmd-shift-f", None, Sidebar, "聚焦侧栏搜索", sidebar::FocusSearch),
        sc("cmd-l", None, Sidebar, "侧栏定位当前会话", sidebar::RevealActive),
        sc("cmd-shift-e", None, Sidebar, "侧栏：展开 / 折叠全部分组", sidebar::ToggleAllGroups),
        // ---- B 侧栏：组件内部的键（D7）----
        sc("down", L, Sidebar, "侧栏：下一条", sidebar::SelectNext),
        sc("up", L, Sidebar, "侧栏：上一条", sidebar::SelectPrev),
        sc("enter", L, Sidebar, "侧栏：打开选中的会话", sidebar::OpenSelected),
        sc("cmd-enter", L, Sidebar, "侧栏：左右分屏打开", sidebar::OpenSelectedSplitRight),
        sc("shift-enter", L, Sidebar, "侧栏：上下分屏打开", sidebar::OpenSelectedSplitDown),
        sc("space", L, Sidebar, "侧栏：悬停卡开 / 关", sidebar::ToggleHoverCard),
        sc("left", L, Sidebar, "侧栏：折叠所在的组", sidebar::CollapseGroup),
        sc("right", L, Sidebar, "侧栏：展开所在的组", sidebar::ExpandGroup),
        sc("escape", L, Sidebar, "侧栏：清除选中，回到终端", sidebar::ClearSelection),
        sc("down", S, Sidebar, "搜索框：进入列表", sidebar::SearchToList),
        sc("backspace", S, Sidebar, "搜索框：删前一个字", sidebar::SearchBackspace),
        sc("delete", S, Sidebar, "搜索框：删后一个字", sidebar::SearchDelete),
        sc("left", S, Sidebar, "搜索框：光标左移", sidebar::SearchLeft),
        sc("right", S, Sidebar, "搜索框：光标右移", sidebar::SearchRight),
        sc("shift-left", S, Sidebar, "搜索框：向左选", sidebar::SearchSelectLeft),
        sc("shift-right", S, Sidebar, "搜索框：向右选", sidebar::SearchSelectRight),
        sc("cmd-a", S, Sidebar, "搜索框：全选", sidebar::SearchSelectAll),
        sc("home", S, Sidebar, "搜索框：到行首", sidebar::SearchHome),
        sc("cmd-left", S, Sidebar, "搜索框：到行首", sidebar::SearchHome),
        sc("end", S, Sidebar, "搜索框：到行尾", sidebar::SearchEnd),
        sc("cmd-right", S, Sidebar, "搜索框：到行尾", sidebar::SearchEnd),
        // 单行框里 ⌘↑↓ = 到开头 / 结尾（原生输入框行为）；不绑的话会落到全局去切标签（Tauri 在文本框里让给光标）
        sc("cmd-up", S, Sidebar, "搜索框：到行首", sidebar::SearchHome),
        sc("cmd-down", S, Sidebar, "搜索框：到行尾", sidebar::SearchEnd),
        sc("cmd-v", S, Sidebar, "搜索框：粘贴", sidebar::SearchPaste),
        sc("cmd-c", S, Sidebar, "搜索框：复制", sidebar::SearchCopy),
        sc("cmd-x", S, Sidebar, "搜索框：剪切", sidebar::SearchCut),
        sc("escape", M, Sidebar, "关闭侧栏菜单", sidebar::DismissMenu),
        sc("cmd-b", None, F0, "侧栏折叠 / 展开", app::ToggleSidebar),
        sc("cmd-r", None, F0, "刷新会话列表", app::Refresh),
        sc("cmd-q", None, F0, "退出", app::Quit),
        sc("cmd-t", None, Workspace, "新终端", w::NewTab),
        sc("cmd-w", None, Workspace, "关闭当前标签", w::CloseTab),
        sc("cmd-d", None, Workspace, "左右分屏", w::SplitRight),
        sc("cmd-shift-d", None, Workspace, "上下分屏", w::SplitDown),
        sc("alt-cmd-enter", None, Workspace, "当前 pane 最大化 / 还原", w::ToggleMaximize),
        sc("cmd-]", None, Workspace, "下一个标签", w::NextTab),
        sc("cmd-[", None, Workspace, "上一个标签", w::PrevTab),
        // ⌘⇧] / ⌘⇧[（Safari 惯例）：macOS 上 GPUI 报成 cmd-} / cmd-{，写 shift-] 永远匹配不上
        sc("cmd-}", None, Workspace, "下一个标签", w::NextTab),
        sc("cmd-{", None, Workspace, "上一个标签", w::PrevTab),
        sc("cmd-right", None, Workspace, "下一个标签", w::NextTab),
        sc("cmd-down", None, Workspace, "下一个标签", w::NextTab),
        sc("cmd-left", None, Workspace, "上一个标签", w::PrevTab),
        sc("cmd-up", None, Workspace, "上一个标签", w::PrevTab),
        sc("ctrl-tab", None, Workspace, "下一个标签", w::NextTab),
        sc("ctrl-shift-tab", None, Workspace, "上一个标签", w::PrevTab),
        sc("cmd-1", None, Workspace, "第 1 个标签", w::ActivateTab1),
        sc("cmd-2", None, Workspace, "第 2 个标签", w::ActivateTab2),
        sc("cmd-3", None, Workspace, "第 3 个标签", w::ActivateTab3),
        sc("cmd-4", None, Workspace, "第 4 个标签", w::ActivateTab4),
        sc("cmd-5", None, Workspace, "第 5 个标签", w::ActivateTab5),
        sc("cmd-6", None, Workspace, "第 6 个标签", w::ActivateTab6),
        sc("cmd-7", None, Workspace, "第 7 个标签", w::ActivateTab7),
        sc("cmd-8", None, Workspace, "第 8 个标签", w::ActivateTab8),
        sc("cmd-9", None, Workspace, "第 9 个标签", w::ActivateTab9),
        sc("alt-cmd-1", None, Workspace, "第 1 个 pane", w::FocusPane1),
        sc("alt-cmd-2", None, Workspace, "第 2 个 pane", w::FocusPane2),
        sc("alt-cmd-3", None, Workspace, "第 3 个 pane", w::FocusPane3),
        sc("alt-cmd-4", None, Workspace, "第 4 个 pane", w::FocusPane4),
        sc("alt-cmd-5", None, Workspace, "第 5 个 pane", w::FocusPane5),
        sc("alt-cmd-6", None, Workspace, "第 6 个 pane", w::FocusPane6),
        sc("alt-cmd-7", None, Workspace, "第 7 个 pane", w::FocusPane7),
        sc("alt-cmd-8", None, Workspace, "第 8 个 pane", w::FocusPane8),
        sc("alt-cmd-9", None, Workspace, "第 9 个 pane", w::FocusPane9),
        sc("alt-cmd-left", None, Workspace, "左边的 pane", w::FocusPaneLeft),
        sc("alt-cmd-right", None, Workspace, "右边的 pane", w::FocusPaneRight),
        sc("alt-cmd-up", None, Workspace, "上面的 pane", w::FocusPaneUp),
        sc("alt-cmd-down", None, Workspace, "下面的 pane", w::FocusPaneDown),
        sc("cmd-i", None, Notify, "通知中心", notify::ToggleNotificationCenter),
        sc("down", N, Notify, "通知中心：下一条", notify::SelectNext),
        sc("up", N, Notify, "通知中心：上一条", notify::SelectPrev),
        sc("enter", N, Notify, "通知中心：已读并跳转", notify::Confirm),
        sc("escape", N, Notify, "通知中心：关闭", notify::Dismiss),
        sc("cmd-,", None, Overlays, "设置", overlays::OpenSettings),
        // 字号：全局（同 Tauri，作用在当前标签的终端；焦点在侧栏 / 面板里也生效）。⌘+ = ⌘⇧=，macOS 报成 cmd-+
        sc("cmd-=", None, Terminal, "字号 +1", terminal::FontIncrease),
        sc("cmd-+", None, Terminal, "字号 +1", terminal::FontIncrease),
        sc("cmd--", None, Terminal, "字号 -1", terminal::FontDecrease),
        sc("cmd-0", None, Terminal, "字号重置为 13", terminal::FontReset),
        sc("cmd-c", T, Terminal, "复制", terminal::Copy),
        sc("cmd-v", T, Terminal, "粘贴", terminal::Paste),
        sc("shift-pageup", T, Terminal, "回看上翻一页", terminal::ScrollPageUp),
        sc("shift-pagedown", T, Terminal, "回看下翻一页", terminal::ScrollPageDown),
        // ---- D 浮层：面板内（context `Overlay`，所有浮层共用）----
        sc("escape", OV, Overlays, "关闭面板 / 取消", overlays::Dismiss),
        sc("enter", OV, Overlays, "打开 / 确认", overlays::Confirm),
        sc("cmd-enter", OV, Overlays, "命令面板：左右分屏打开", overlays::ConfirmSplitRight),
        sc("cmd-shift-enter", OV, Overlays, "命令面板：上下分屏打开", overlays::ConfirmSplitDown),
        sc("shift-enter", OV, Overlays, "搜索条：上一个", overlays::ConfirmReverse),
        sc("up", OV, Overlays, "上一项", overlays::SelectPrev),
        sc("down", OV, Overlays, "下一项", overlays::SelectNext),
        // ---- D 浮层：单行输入框的编辑键（context `TextInput`，设置页的快捷键表不展示这组）----
        sc("backspace", TI, Overlays, "删除前一个字", overlays::InputBackspace),
        sc("delete", TI, Overlays, "删除后一个字", overlays::InputDelete),
        sc("cmd-backspace", TI, Overlays, "删到行首", overlays::InputDeleteToStart),
        sc("left", TI, Overlays, "光标左移", overlays::InputLeft),
        sc("right", TI, Overlays, "光标右移", overlays::InputRight),
        sc("shift-left", TI, Overlays, "向左选择", overlays::InputSelectLeft),
        sc("shift-right", TI, Overlays, "向右选择", overlays::InputSelectRight),
        sc("cmd-a", TI, Overlays, "全选", overlays::InputSelectAll),
        sc("home", TI, Overlays, "行首", overlays::InputHome),
        sc("end", TI, Overlays, "行尾", overlays::InputEnd),
        sc("cmd-left", TI, Overlays, "行首", overlays::InputHome),
        sc("cmd-right", TI, Overlays, "行尾", overlays::InputEnd),
        sc("cmd-up", TI, Overlays, "行首", overlays::InputHome),
        sc("cmd-down", TI, Overlays, "行尾", overlays::InputEnd),
        sc("cmd-shift-left", TI, Overlays, "选到行首", overlays::InputSelectHome),
        sc("cmd-shift-right", TI, Overlays, "选到行尾", overlays::InputSelectEnd),
        sc("cmd-v", TI, Overlays, "粘贴", overlays::InputPaste),
        sc("cmd-c", TI, Overlays, "复制", overlays::InputCopy),
        sc("cmd-x", TI, Overlays, "剪切", overlays::InputCut),
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
