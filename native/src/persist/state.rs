//! 持久化的数据形状（`~/.claude/makit/native-state.json`）。
//!
//! **加新字段的规矩**：加在对应的子结构里，带 `#[serde(default)]`（整个结构体已经是 default），
//! 老文件读出来缺这个字段时就用默认值 —— 不需要版本迁移。字段改名 / 改类型才需要升 `version`。
//!
//! 为什么单独测：数据形状错了是每次启动都丢设置。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::theme::derive::ThemeSource;
use crate::workspace::model::WorkspaceState;

pub const STATE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct NativeState {
    pub version: u32,
    /// `makit-workspace`（旧格式 `makit-open-tabs` 在导入时迁移）。None = 还没有布局，启动时建默认的
    pub workspace: Option<WorkspaceState>,
    /// `makit-pinned-sessions`
    pub pinned_sessions: Vec<String>,
    /// 会话 id → 恢复时在选择器里选定的目录（#239）。启动目录不在了再打开会话时优先用它
    pub session_cwd_overrides: BTreeMap<String, String>,
    pub sidebar: SidebarPrefs,
    pub theme: ThemePrefs,
    /// `makit-pane-icons`：beasts / flowers / fruits / dots / none
    pub pane_icons: String,
    /// 对话视图：false = 终端（默认，和原来一样）；true = 可重排（历史来自会话文件，活动区来自真终端，#231）。全局，所有绑定了会话的标签一起跟着变
    pub reflow_view: bool,
    /// Whole-interface scale (#277): one of `ui_scale::STEPS`, 1.0 = design size. The terminal font zoom is separate
    pub ui_scale: f32,
    /// UI language (#71): `en` (default) / `zh` / `system` (follows the system language)
    pub language: String,
    pub palette: PalettePrefs,
    pub notify: NotifyPrefs,
    /// Update check bookkeeping (#200)
    pub update: UpdatePrefs,
    /// `makit-notifications`：通知记录（最多 100 条，每会话一条）。形状是 `notify::book::Record`
    /// （`{session_id, kind, message, at, read}`，不存名字，#8）。
    /// 这里保持原样存 JSON，好让坏一条不影响整份状态文件
    pub notifications: Vec<serde_json::Value>,
    /// 上次退出 / 失去焦点时的窗口位置和大小（#256 A7）。None = 还没记过，按屏幕大小给默认；还原时一律用 `window_placement::fit_window` 校正
    pub window: Option<SavedWindow>,
}

/// 保存的窗口位置和大小（逻辑像素，坐标是全局的：外接屏在主屏左边时 x 为负，合法）。
/// `maximized` 时 x/y/w/h 是「还原大小」。全屏不保存（回到普通窗口，见 TRD #256 §5.3）
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct SavedWindow {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    #[serde(default)]
    pub maximized: bool,
}

impl Default for NativeState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            workspace: None,
            pinned_sessions: Vec::new(),
            session_cwd_overrides: BTreeMap::new(),
            sidebar: SidebarPrefs::default(),
            theme: ThemePrefs::default(),
            pane_icons: "beasts".into(),
            reflow_view: false,
            ui_scale: 1.0,
            language: crate::i18n::PREF_EN.into(),
            palette: PalettePrefs::default(),
            notify: NotifyPrefs::default(),
            update: UpdatePrefs::default(),
            notifications: Vec::new(),
            window: None,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct SidebarPrefs {
    /// `makit-project-list-collapsed`（"1"/"0"）
    pub collapsed: bool,
    /// `makit-project-list-width`（px，180–480）
    pub width: f32,
    /// `makit-sidebar-view`：status / project
    pub view: String,
    /// `makit-tree-sort`：recent / count / firstMsg
    pub sort: String,
    /// `makit-tree-show-archived`
    pub show_archived: bool,
    /// `makit-row-short-id`
    pub row_short_id: bool,
    /// `makit-row-branch`
    pub row_branch: bool,
    /// 行内显示 claude / codex 图标（#238，GPUI 版新增，默认开）
    pub row_logo: bool,
    /// 行内显示项目名（#238，默认开）
    pub row_project: bool,
    /// 行右上角显示活跃时间（#238，默认开）
    pub row_time: bool,
    /// `makit-hover-mode`：always / cmd / off
    pub hover_mode: String,
    /// `makit-proj-collapsed`：项目组折叠集合（colKey 与 `__expanded__`+colKey 两种）
    pub proj_collapsed: Vec<String>,
    /// `makit-group-collapsed`：状态组 / 日期段折叠的 id 集合
    pub group_collapsed: Vec<String>,
    /// `makit-section-collapsed`
    pub section_collapsed: Vec<String>,
    /// `makit-tree-pinned-only`
    pub pinned_only: bool,
    /// `makit-tree-running-only`
    pub running_only: bool,
}

impl Default for SidebarPrefs {
    fn default() -> Self {
        Self {
            collapsed: false,
            width: 280.0,
            view: "status".into(),
            sort: "recent".into(),
            show_archived: false,
            row_short_id: false,
            row_branch: false,
            row_logo: true,
            row_project: true,
            row_time: true,
            hover_mode: "always".into(),
            proj_collapsed: Vec::new(),
            group_collapsed: Vec::new(),
            section_collapsed: Vec::new(),
            pinned_only: false,
            running_only: false,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct ThemePrefs {
    /// `makit-theme`
    pub id: String,
    /// `makit-imported-themes`
    pub imported: Vec<ThemeSource>,
    /// 用户有没有**主动选过**主题（#256 A6）。没选过就一直跟着系统外观；旧状态文件里没有这个字段，读出来按「选过」算（保持老用户现在的主题）
    #[serde(default = "default_true")]
    pub chosen: bool,
}

fn default_true() -> bool {
    true
}

impl Default for ThemePrefs {
    /// 全新用户：没选过
    fn default() -> Self {
        Self { id: crate::theme::builtin::DEFAULT_THEME_ID.into(), imported: Vec::new(), chosen: false }
    }
}

/// Update check bookkeeping (#200)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct UpdatePrefs {
    /// Version the user chose "Skip this version" for (without `v`); empty = none
    pub skipped: String,
    /// Unix seconds of the last completed check; 0 = never
    pub last_check: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct PalettePrefs {
    /// `makit-palette-history`（最多 8 条）
    pub history: Vec<String>,
    /// `makit-palette-collapsed`
    pub collapsed: Vec<String>,
    /// `makit-palette-filter`：{type, project, time, status[], pinnedOnly}，形状归浮层包（overlays/）
    pub filter: Option<serde_json::Value>,
    /// `makit-palette-sort`：recent / count / firstMsg
    pub sort: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct NotifyPrefs {
    /// `makit-notif-system`（默认开）
    pub system: bool,
    /// `makit-notif-approval`（默认开）
    pub approval: bool,
    /// `makit-notif-user`（默认关）
    pub user: bool,
    /// 「已完成」通知（#215 新增，Tauri 版没有；默认开，静音横幅）
    pub completed: bool,
    /// 需要处理的通知（审批 / 回答 / 出错）横幅响铃（#215 新增，默认开）
    pub sound: bool,
}

impl Default for NotifyPrefs {
    fn default() -> Self {
        Self { system: true, approval: true, user: false, completed: true, sound: true }
    }
}

#[cfg(test)]
mod tests {
    /// The interface language defaults to English; "follow system" is an explicit choice.
    #[test]
    fn language_defaults_to_english() {
        assert_eq!(NativeState::default().language, "en");
        let loaded: NativeState = serde_json::from_str("{}").unwrap();
        assert_eq!(loaded.language, "en", "a state file without the field also gets English");
    }

    use super::*;

    #[test]
    fn old_state_file_missing_new_fields_still_loads() {
        let s: NativeState = serde_json::from_str(r#"{"version":1,"pinned_sessions":["x"],"sidebar":{"width":300}}"#).unwrap();
        assert_eq!(s.pinned_sessions, ["x"]);
        assert_eq!(s.sidebar.width, 300.0);
        assert_eq!(s.sidebar.view, "status", "缺的字段用默认值");
        assert!(s.notify.system);
        // 往返
        let back: NativeState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    /// 为什么要测（#256 A6）：老用户的状态文件里没有 `chosen`。读出来如果是 false，升级后他们选好的主题会被系统外观盖掉
    #[test]
    fn old_state_files_count_as_chosen_and_new_users_do_not() {
        let old: NativeState = serde_json::from_str(r#"{"theme":{"id":"dracula"}}"#).unwrap();
        assert!(old.theme.chosen, "旧文件：保持原主题");
        assert_eq!(old.theme.id, "dracula");
        assert!(!NativeState::default().theme.chosen, "全新用户：没选过，跟系统");
        let fresh = NativeState::default();
        let back: NativeState = serde_json::from_str(&serde_json::to_string(&fresh).unwrap()).unwrap();
        assert!(!back.theme.chosen, "写出去再读回来不能丢成 true");
    }

    /// 为什么要测（#256 A7）：加了 `window` 字段后，老状态文件（没有它）必须照常加载；保存的窗口要能原样往返
    #[test]
    fn window_field_is_optional_and_round_trips() {
        let old: NativeState = serde_json::from_str(r#"{"theme":{"id":"nord","chosen":true}}"#).unwrap();
        assert_eq!(old.window, None, "旧文件：没记过窗口");
        let mut s = NativeState::default();
        s.window = Some(SavedWindow { x: -1700.0, y: 100.0, w: 1200.0, h: 800.0, maximized: true });
        let back: NativeState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.window, s.window, "负坐标、最大化标志都不能丢");
        let no_flag: NativeState = serde_json::from_str(r#"{"window":{"x":1,"y":2,"w":900,"h":600}}"#).unwrap();
        assert!(!no_flag.window.unwrap().maximized, "没有 maximized 字段按 false");
    }
}
