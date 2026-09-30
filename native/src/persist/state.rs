//! 持久化的数据形状（`~/.claude/makit/native-state.json`），以及从 Tauri 版 localStorage 的转换。
//!
//! 对应界面清单 Q 节的 localStorage 键；每个字段的注释写着它来自哪个键。
//! **加新字段的规矩**：加在对应的子结构里，带 `#[serde(default)]`（整个结构体已经是 default），
//! 老文件读出来缺这个字段时就用默认值 —— 不需要版本迁移。字段改名 / 改类型才需要升 `version`。
//!
//! 为什么单独测：导入错了是「升级到 GPUI 版后布局、置顶、主题全没了」，而且只发生一次（只导一次），
//! 事后没法重来；数据形状错了是每次启动都丢设置。

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::theme::derive::ThemeSource;
use crate::workspace::model::{load_workspace, WorkspaceState};

pub const STATE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct NativeState {
    pub version: u32,
    /// `makit-workspace`（旧格式 `makit-open-tabs` 在导入时迁移）。None = 还没有布局，启动时建默认的
    pub workspace: Option<WorkspaceState>,
    /// `makit-pinned-sessions`
    pub pinned_sessions: Vec<String>,
    pub sidebar: SidebarPrefs,
    pub theme: ThemePrefs,
    /// `makit-pane-icons`：beasts / flowers / fruits / dots / none
    pub pane_icons: String,
    pub palette: PalettePrefs,
    pub notify: NotifyPrefs,
    /// `makit-notifications`：通知记录（最多 100 条，每会话一条）。形状是 `notify::book::Record`
    /// （`{session_id, kind, message, at, read}`，不存名字，#8）；导入的 Tauri 旧形状由 `Book::from_saved` 转换。
    /// 这里保持原样存 JSON，好让坏一条不影响整份状态文件
    pub notifications: Vec<serde_json::Value>,
    /// 从哪个 localStorage 文件导入过（只导一次；None = 没导过 / 没找到）
    pub imported_from: Option<String>,
}

impl Default for NativeState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            workspace: None,
            pinned_sessions: Vec::new(),
            sidebar: SidebarPrefs::default(),
            theme: ThemePrefs::default(),
            pane_icons: "beasts".into(),
            palette: PalettePrefs::default(),
            notify: NotifyPrefs::default(),
            notifications: Vec::new(),
            imported_from: None,
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
}

impl Default for ThemePrefs {
    fn default() -> Self {
        Self { id: crate::theme::builtin::DEFAULT_THEME_ID.into(), imported: Vec::new() }
    }
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

// ---------- 从 localStorage 转换 ----------

/// WebKit localStorage 的值是 UTF-16LE。奇数字节 / 非法代理对按有损处理（不让一个坏值挡住整次导入）
pub fn decode_utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

fn hex_to_bytes(h: &str) -> Option<Vec<u8>> {
    if h.len() % 2 != 0 {
        return None;
    }
    (0..h.len()).step_by(2).map(|i| u8::from_str_radix(&h[i..i + 2], 16).ok()).collect()
}

/// `sqlite3 … "select key, hex(value) from ItemTable"` 的输出（每行 `key|HEX`）→ 键值表。
/// 坏行跳过。只收 `makit-` 开头的键。
pub fn parse_sqlite_hex_dump(out: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in out.lines() {
        let Some((k, h)) = line.split_once('|') else { continue };
        if !k.starts_with("makit-") {
            continue;
        }
        if let Some(b) = hex_to_bytes(h.trim()) {
            map.insert(k.to_string(), decode_utf16le(&b));
        }
    }
    map
}

/// localStorage 键值 → NativeState。每个键单独解析，坏一个不影响别的（用默认值）。
pub fn from_local_storage(ls: &BTreeMap<String, String>) -> NativeState {
    let mut s = NativeState::default();
    let get = |k: &str| ls.get(k).map(|v| v.as_str());
    let json = |k: &str| get(k).and_then(|v| serde_json::from_str::<serde_json::Value>(v).ok());
    let strings = |k: &str| -> Option<Vec<String>> { serde_json::from_str(get(k)?).ok() };
    let boolean = |k: &str| get(k).map(|v| v == "true");

    if get("makit-workspace").is_some() || get("makit-open-tabs").is_some() {
        s.workspace = Some(load_workspace(get("makit-workspace"), get("makit-open-tabs")));
    }
    if let Some(v) = strings("makit-pinned-sessions") {
        s.pinned_sessions = v;
    }

    let sb = &mut s.sidebar;
    if let Some(v) = get("makit-project-list-collapsed") {
        sb.collapsed = v == "1";
    }
    if let Some(w) = get("makit-project-list-width").and_then(|v| v.trim().parse::<f32>().ok()) {
        sb.width = w.clamp(180.0, 480.0);
    }
    if let Some(v) = get("makit-sidebar-view").filter(|v| matches!(*v, "status" | "project")) {
        sb.view = v.into();
    }
    if let Some(v) = get("makit-tree-sort").filter(|v| matches!(*v, "recent" | "count" | "firstMsg")) {
        sb.sort = v.into();
    }
    if let Some(v) = get("makit-hover-mode").filter(|v| matches!(*v, "always" | "cmd" | "off")) {
        sb.hover_mode = v.into();
    }
    sb.show_archived = boolean("makit-tree-show-archived").unwrap_or(sb.show_archived);
    sb.row_short_id = boolean("makit-row-short-id").unwrap_or(sb.row_short_id);
    sb.row_branch = boolean("makit-row-branch").unwrap_or(sb.row_branch);
    sb.pinned_only = boolean("makit-tree-pinned-only").unwrap_or(sb.pinned_only);
    sb.running_only = boolean("makit-tree-running-only").unwrap_or(sb.running_only);
    if let Some(v) = strings("makit-proj-collapsed") {
        sb.proj_collapsed = v;
    }
    if let Some(v) = strings("makit-group-collapsed") {
        sb.group_collapsed = v;
    }
    if let Some(v) = strings("makit-section-collapsed") {
        sb.section_collapsed = v;
    }

    if let Some(v) = get("makit-theme").filter(|v| !v.is_empty()) {
        s.theme.id = v.into();
    }
    if let Some(v) = get("makit-imported-themes").and_then(|v| serde_json::from_str::<Vec<ThemeSource>>(v).ok()) {
        s.theme.imported = v;
    }
    if let Some(v) = get("makit-pane-icons").filter(|v| !v.is_empty()) {
        s.pane_icons = v.into();
    }

    if let Some(v) = strings("makit-palette-history") {
        s.palette.history = v;
    }
    if let Some(v) = strings("makit-palette-collapsed") {
        s.palette.collapsed = v;
    }
    s.palette.filter = json("makit-palette-filter").filter(|v| v.is_object());
    s.palette.sort = get("makit-palette-sort").filter(|v| !v.is_empty()).map(String::from);

    // 通知开关：前端的读法是「存的不是 "false" 就算开」（system / approval 默认开），user 默认关
    if let Some(v) = get("makit-notif-system") {
        s.notify.system = v != "false";
    }
    if let Some(v) = get("makit-notif-approval") {
        s.notify.approval = v != "false";
    }
    if let Some(v) = get("makit-notif-user") {
        s.notify.user = v == "true";
    }
    if let Some(serde_json::Value::Array(a)) = json("makit-notifications") {
        s.notifications = a;
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16_hex(s: &str) -> String {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).map(|b| format!("{b:02X}")).collect()
    }

    #[test]
    fn decodes_sqlite_hex_dump_of_utf16_values() {
        let out = format!(
            "makit-theme|{}\nmakit-section-collapsed|{}\nother-app|{}\nbroken line\nmakit-odd|ABC\n",
            utf16_hex("snazzy"),
            utf16_hex("[\"最近\",\"活跃\"]"),
            utf16_hex("x")
        );
        let m = parse_sqlite_hex_dump(&out);
        assert_eq!(m.get("makit-theme").map(|s| s.as_str()), Some("snazzy"));
        assert_eq!(m.get("makit-section-collapsed").map(|s| s.as_str()), Some("[\"最近\",\"活跃\"]"), "中文按 UTF-16LE 解");
        assert!(!m.contains_key("other-app"), "只收 makit- 的键");
        assert!(!m.contains_key("makit-odd"), "奇数位 hex 跳过");
    }

    #[test]
    fn maps_every_known_key() {
        let ls: BTreeMap<String, String> = [
            ("makit-tree-sort", "count"),
            ("makit-tree-pinned-only", "true"),
            ("makit-palette-filter", r#"{"type":"全部","project":"","time":"all","status":[],"pinnedOnly":false}"#),
            ("makit-pinned-sessions", r#"["a","b"]"#),
            ("makit-tree-running-only", "false"),
            ("makit-tree-show-archived", "true"),
            ("makit-section-collapsed", r#"["最近"]"#),
            ("makit-project-list-width", "215"),
            ("makit-theme", "snazzy"),
            ("makit-sidebar-view", "project"),
            ("makit-row-short-id", "true"),
            ("makit-row-branch", "true"),
            ("makit-project-list-collapsed", "1"),
            ("makit-group-collapsed", r#"["running"]"#),
            ("makit-proj-collapsed", r#"["k","__expanded__k2"]"#),
            ("makit-hover-mode", "cmd"),
            ("makit-pane-icons", "fruits"),
            ("makit-palette-history", r#"["foo"]"#),
            ("makit-palette-collapsed", r#"["置顶"]"#),
            ("makit-palette-sort", "firstMsg"),
            ("makit-notif-system", "false"),
            ("makit-notif-approval", "true"),
            ("makit-notif-user", "true"),
            ("makit-notifications", r#"[{"id":"x","sessionId":"s","kind":"waiting","isRead":false}]"#),
            ("makit-imported-themes", r##"[{"id":"imported:A","name":"A","bg":"#000000","fg":"#ffffff","ansi":["#111111"]}]"##),
            (
                "makit-workspace",
                r#"{"root":{"kind":"container","id":"c_1","tabs":[{"id":"t_1","kind":"shell","cwd":"/Users/me","initCommand":null,"sessionId":null,"sessionShortId":null,"label":""}],"activeTabId":"t_1","tabHistory":[]},"activeContainerId":"c_1"}"#,
            ),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let s = from_local_storage(&ls);
        assert_eq!(s.sidebar.sort, "count");
        assert!(s.sidebar.pinned_only && !s.sidebar.running_only && s.sidebar.show_archived);
        assert!(s.sidebar.row_short_id && s.sidebar.row_branch && s.sidebar.collapsed);
        assert_eq!(s.sidebar.width, 215.0);
        assert_eq!(s.sidebar.view, "project");
        assert_eq!(s.sidebar.hover_mode, "cmd");
        assert_eq!(s.sidebar.group_collapsed, ["running"]);
        assert_eq!(s.sidebar.proj_collapsed, ["k", "__expanded__k2"]);
        assert_eq!(s.sidebar.section_collapsed, ["最近"]);
        assert_eq!(s.pinned_sessions, ["a", "b"]);
        assert_eq!(s.theme.id, "snazzy");
        assert_eq!(s.theme.imported[0].id, "imported:A");
        assert_eq!(s.pane_icons, "fruits");
        assert_eq!(s.palette.history, ["foo"]);
        assert_eq!(s.palette.collapsed, ["置顶"]);
        assert_eq!(s.palette.sort.as_deref(), Some("firstMsg"));
        assert_eq!(s.palette.filter.as_ref().unwrap()["time"], "all");
        assert!(!s.notify.system && s.notify.approval && s.notify.user);
        assert_eq!(s.notifications.len(), 1);
        let ws = s.workspace.unwrap();
        assert_eq!(ws.active_container_id, "c_1");
    }

    #[test]
    fn bad_values_fall_back_to_defaults_one_by_one() {
        let ls: BTreeMap<String, String> = [
            ("makit-pinned-sessions", "不是 json"),
            ("makit-project-list-width", "9999"),
            ("makit-sidebar-view", "weird"),
            ("makit-theme", "dracula"),
            ("makit-workspace", "{坏"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let s = from_local_storage(&ls);
        assert!(s.pinned_sessions.is_empty());
        assert_eq!(s.sidebar.width, 480.0, "宽度夹在 180–480");
        assert_eq!(s.sidebar.view, "status");
        assert_eq!(s.theme.id, "dracula", "一个坏值不影响别的键");
        assert!(s.workspace.is_some(), "布局坏了给默认布局，不是 None");
        assert!(s.notify.system && s.notify.approval && !s.notify.user, "没存的开关用默认值");
    }

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
}
