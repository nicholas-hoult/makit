//! ⌘K 命令面板的纯逻辑（照 `src/CommandPalette.tsx` + App.tsx 的 `paletteItems`）：
//! 生成条目、筛选、分组排序、每组上限、扁平索引、命中高亮切分、搜索历史、筛选持久化。
//!
//! 为什么单独测：面板上看到的「这条为什么出现 / 为什么排在这 / 为什么这一组是空的」全由这里决定，
//! 错了在界面上是「搜得到但没高亮」「置顶的沉到下面」「上次的筛选忘了清、打开一片空白」——
//! 肉眼回归不了，TS 版也没有测试钉住。

use crate::ts;
use std::ops::Range;

use makit_core::SessionMeta;
use serde_json::{json, Value};

use crate::sidebar::groups::session_title;

// ---------- 状态 ----------

/// 状态的唯一一套叫法（`sessionStatus.ts` 的 STATUS_LABEL，#187：侧栏和 ⌘K 同一套词）

/// 条目上的状态（决定状态点颜色、药丸）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemStatus {
    Waiting,
    Busy,
    Idle,
    Stopped,
    Archived,
}

/// 状态筛选的六档（持久化成 TS 版的字符串）
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusKey {
    WaitingApproval,
    WaitingUser,
    Busy,
    Idle,
    Stopped,
    Archived,
}

impl StatusKey {
    /// 筛选面板里的顺序
    pub const ALL: [StatusKey; 6] =
        [StatusKey::WaitingApproval, StatusKey::WaitingUser, StatusKey::Busy, StatusKey::Idle, StatusKey::Stopped, StatusKey::Archived];

    pub fn as_str(self) -> &'static str {
        match self {
            StatusKey::WaitingApproval => "waiting_approval",
            StatusKey::WaitingUser => "waiting_user",
            StatusKey::Busy => "busy",
            StatusKey::Idle => "idle",
            StatusKey::Stopped => "stopped",
            StatusKey::Archived => "archived",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    pub fn label(self) -> String {
        match self {
            StatusKey::WaitingApproval => crate::sidebar::groups::status_label::waiting_approval(),
            StatusKey::WaitingUser => crate::sidebar::groups::status_label::waiting_user(),
            StatusKey::Busy => crate::sidebar::groups::status_label::busy(),
            StatusKey::Idle => crate::sidebar::groups::status_label::idle(),
            StatusKey::Stopped => crate::sidebar::groups::status_label::stopped(),
            StatusKey::Archived => crate::sidebar::groups::status_label::archived(),
        }
    }

    /// 筛选行前面的定宽字形（`palette-filter-glyph`）
    pub fn glyph(self) -> &'static str {
        match self {
            StatusKey::WaitingApproval => "⚠",
            StatusKey::WaitingUser => "?",
            StatusKey::Busy => "▶",
            StatusKey::Idle => "○",
            StatusKey::Stopped => "·",
            StatusKey::Archived => "▤",
        }
    }
}

/// 状态药丸只留「需要人动手」的 waiting + 归档（`showsStatusPill`）
pub fn shows_status_pill(s: ItemStatus) -> bool {
    matches!(s, ItemStatus::Waiting | ItemStatus::Archived)
}

// ---------- 排序 / 时间 ----------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    Recent,
    Count,
    FirstMsg,
}

impl SortKey {
    pub fn parse(s: Option<&str>) -> Self {
        Self::ALL.into_iter().find(|k| Some(k.as_str()) == s).unwrap_or_default()
    }
    pub fn as_str(self) -> &'static str {
        match self {
            SortKey::Recent => "recent",
            SortKey::Count => "count",
            SortKey::FirstMsg => "firstMsg",
        }
    }
    pub fn label(self) -> String {
        match self {
            SortKey::Recent => ts!("sidebar.options.sort_recent"),
            SortKey::Count => ts!("sidebar.options.sort_count"),
            SortKey::FirstMsg => ts!("sidebar.options.sort_first"),
        }
    }
    pub const ALL: [SortKey; 3] = [SortKey::Recent, SortKey::Count, SortKey::FirstMsg];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TimeFilter {
    #[default]
    All,
    Today,
    Week,
    Month,
}

impl TimeFilter {
    pub const ALL: [TimeFilter; 4] = [TimeFilter::All, TimeFilter::Today, TimeFilter::Week, TimeFilter::Month];
    pub fn as_str(self) -> &'static str {
        match self {
            TimeFilter::All => "all",
            TimeFilter::Today => "today",
            TimeFilter::Week => "week",
            TimeFilter::Month => "month",
        }
    }
    pub fn label(self) -> String {
        match self {
            TimeFilter::All => ts!("palette.filter.all"),
            TimeFilter::Today => ts!("time.today"),
            TimeFilter::Week => ts!("palette.time.week"),
            TimeFilter::Month => ts!("palette.time.month"),
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s)
    }
}

// ---------- 筛选（持久化 `makit-palette-filter`）----------

/// `{type, project, time, status[], pinnedOnly}`，形状和 TS 版一致（首启从 localStorage 原样导入）
#[derive(Clone, Debug, PartialEq)]
pub struct PaletteFilter {
    /// 顶部 type 标签（目前只有 Session 一种，标签行不显示）
    pub type_: String,
    /// 项目根目录；空 = 全部
    pub project: String,
    pub time: TimeFilter,
    /// 空 = 全部
    pub status: Vec<StatusKey>,
    pub pinned_only: bool,
}

impl Default for PaletteFilter {
    fn default() -> Self {
        Self { type_: "全部".into(), project: String::new(), time: TimeFilter::All, status: Vec::new(), pinned_only: false }
    }
}

impl PaletteFilter {
    /// 宽松解析：缺字段 / 认不出的值用默认，不因为一个坏字段丢掉整份
    pub fn from_json(v: Option<&Value>) -> Self {
        let mut f = Self::default();
        let Some(o) = v.and_then(|v| v.as_object()) else { return f };
        if let Some(t) = o.get("type").and_then(|x| x.as_str()) {
            f.type_ = t.to_string();
        }
        if let Some(p) = o.get("project").and_then(|x| x.as_str()) {
            f.project = p.to_string();
        }
        if let Some(t) = o.get("time").and_then(|x| x.as_str()).and_then(TimeFilter::parse) {
            f.time = t;
        }
        if let Some(a) = o.get("status").and_then(|x| x.as_array()) {
            f.status = a.iter().filter_map(|x| x.as_str().and_then(StatusKey::parse)).collect();
        }
        if let Some(b) = o.get("pinnedOnly").and_then(|x| x.as_bool()) {
            f.pinned_only = b;
        }
        f
    }

    pub fn to_json(&self) -> Value {
        json!({
            "type": self.type_,
            "project": self.project,
            "time": self.time.as_str(),
            "status": self.status.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            "pinnedOnly": self.pinned_only,
        })
    }

    /// 筛选按钮上的小圆点、空状态里的「点这里清除」都看这个（type 标签不算）
    pub fn has_filters(&self) -> bool {
        !self.project.is_empty() || self.time != TimeFilter::All || !self.status.is_empty() || self.pinned_only
    }

    /// 清除全部筛选：**不重置 type**（那是顶部标签的状态，和侧栏这几项不是一组）
    pub fn clear(&mut self) {
        self.project.clear();
        self.time = TimeFilter::All;
        self.status.clear();
        self.pinned_only = false;
    }

    pub fn toggle_status(&mut self, k: StatusKey) {
        if let Some(i) = self.status.iter().position(|x| *x == k) {
            self.status.remove(i);
        } else {
            self.status.push(k);
        }
    }
}

// ---------- 条目 ----------

#[derive(Clone, Debug, PartialEq)]
pub struct PaletteItem {
    /// `s:<session_id>:<组名>`
    pub id: String,
    pub session_id: String,
    pub title: String,
    /// `项目 · short_id · @branch`
    pub subtitle: String,
    /// humanize 时间
    pub hint: String,
    pub group: String,
    pub item_type: String,
    /// git_root‖cwd（项目筛选用）
    pub project_root: String,
    pub status: ItemStatus,
    pub status_label: String,
    pub waiting_for: String,
    pub running: bool,
    pub pinned: bool,
    pub archived: bool,
    pub mtime: i64,
    pub msg_count: u32,
}

/// `sortSessions`：置顶永远最前，再按排序键；稳定排序
pub fn sort_sessions<'a>(mut list: Vec<&'a SessionMeta>, key: SortKey, pinned: &[String]) -> Vec<&'a SessionMeta> {
    let is_pinned = |m: &SessionMeta| pinned.iter().any(|p| *p == m.session_id);
    list.sort_by(|a, b| {
        is_pinned(b).cmp(&is_pinned(a)).then_with(|| match key {
            SortKey::Recent => b.mtime.cmp(&a.mtime),
            SortKey::Count => b.user_msg_count.cmp(&a.user_msg_count),
            // TS 用 localeCompare；这里按码点比。中文标题的相对顺序可能和 WebView 版不同（不影响分组和置顶）
            SortKey::FirstMsg => a.first_user_msg.cmp(&b.first_user_msg),
        })
    });
    list
}

/// 已停止 / 已归档两组最多各 30 条
pub const GROUP_CAP: usize = 30;

/// App.tsx `paletteItems`：分组顺序 等待回答 → 等待审批 → 进行中 → 空闲 → 已停止（≤30）→ 已归档（≤30）。
/// 组名同时是折叠状态的持久化键。
pub fn build_items(sessions: &[SessionMeta], pinned: &[String], sort: SortKey) -> Vec<PaletteItem> {
    let active: Vec<&SessionMeta> = sessions.iter().filter(|s| !s.archived).collect();
    let waiting = |user: bool| active.iter().copied().filter(move |s| s.running && s.status == "waiting" && (s.waiting_for == "user") == user).collect::<Vec<_>>();
    let running: Vec<&SessionMeta> = active.iter().copied().filter(|s| s.running && s.status != "waiting").collect();
    let busy: Vec<&SessionMeta> = running.iter().copied().filter(|s| s.status == "busy").collect();
    let idle: Vec<&SessionMeta> = running.iter().copied().filter(|s| s.status != "busy").collect();
    let stopped: Vec<&SessionMeta> = active.iter().copied().filter(|s| !s.running).collect();
    let archived: Vec<&SessionMeta> = sessions.iter().filter(|s| s.archived).collect();

    let mut items = Vec::new();
    let mut push = |s: &SessionMeta, group: &str| {
        let status = if s.archived {
            ItemStatus::Archived
        } else if s.status == "waiting" {
            ItemStatus::Waiting
        } else if s.status == "busy" {
            ItemStatus::Busy
        } else if s.running {
            ItemStatus::Idle
        } else {
            ItemStatus::Stopped
        };
        let status_label = match status {
            ItemStatus::Waiting if s.waiting_for == "user" => crate::sidebar::groups::status_label::waiting_user(),
            ItemStatus::Waiting => crate::sidebar::groups::status_label::waiting_approval(),
            ItemStatus::Busy => crate::sidebar::groups::status_label::busy(),
            ItemStatus::Idle => crate::sidebar::groups::status_label::idle(),
            ItemStatus::Stopped => crate::sidebar::groups::status_label::stopped(),
            ItemStatus::Archived => crate::sidebar::groups::status_label::archived(),
        };
        let root = s.cwd.rsplit('/').next().unwrap_or("");
        let branch = if s.git_branch.is_empty() { String::new() } else { format!(" · @{}", s.git_branch) };
        items.push(PaletteItem {
            id: format!("s:{}:{group}", s.session_id),
            session_id: s.session_id.clone(),
            title: session_title(&s.display_name, &s.first_user_msg, &s.short_id),
            subtitle: format!("{root} · {}{branch}", s.short_id),
            hint: s.humanize.clone(),
            group: group.to_string(),
            item_type: "Session".into(),
            project_root: if s.git_root.is_empty() { s.cwd.clone() } else { s.git_root.clone() },
            status,
            status_label: status_label.into(),
            waiting_for: s.waiting_for.clone(),
            running: s.running,
            pinned: pinned.iter().any(|p| *p == s.session_id),
            archived: s.archived,
            mtime: s.mtime,
            msg_count: s.user_msg_count,
        });
    };
    for s in sort_sessions(waiting(true), sort, pinned) {
        push(s, &crate::sidebar::groups::status_label::waiting_user());
    }
    for s in sort_sessions(waiting(false), sort, pinned) {
        push(s, &crate::sidebar::groups::status_label::waiting_approval());
    }
    for s in sort_sessions(busy, sort, pinned) {
        push(s, &crate::sidebar::groups::status_label::busy());
    }
    for s in sort_sessions(idle, sort, pinned) {
        push(s, &crate::sidebar::groups::status_label::idle());
    }
    for s in sort_sessions(stopped, sort, pinned).into_iter().take(GROUP_CAP) {
        push(s, &crate::sidebar::groups::status_label::stopped());
    }
    for s in sort_sessions(archived, sort, pinned).into_iter().take(GROUP_CAP) {
        push(s, &crate::sidebar::groups::status_label::archived());
    }
    items
}

/// 筛选面板的项目下拉：各会话的 git_root‖cwd 去重，最近活跃的在前
pub fn project_roots(sessions: &[SessionMeta]) -> Vec<String> {
    let mut latest: Vec<(String, i64)> = Vec::new();
    for s in sessions {
        let root = if s.git_root.is_empty() { &s.cwd } else { &s.git_root };
        if root.is_empty() {
            continue;
        }
        match latest.iter_mut().find(|(r, _)| r == root) {
            Some(e) => e.1 = e.1.max(s.mtime),
            None => latest.push((root.clone(), s.mtime)),
        }
    }
    latest.sort_by(|a, b| b.1.cmp(&a.1));
    latest.into_iter().map(|(r, _)| r).collect()
}

/// 下拉里显示的项目名（路径最后一段）
pub fn project_label(root: &str) -> &str {
    root.trim_end_matches('/').rsplit('/').next().unwrap_or(root)
}

// ---------- 搜索 / 筛选 / 分组 ----------

/// 大小写折叠（逐字符，一对一，保证下标不变 —— 高亮切分和过滤必须用同一条规则）
fn fold(c: char) -> char {
    let mut l = c.to_lowercase();
    match (l.next(), l.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// 大小写不敏感的 includes
pub fn contains_ci(hay: &str, needle: &str) -> bool {
    let n: Vec<char> = needle.chars().map(fold).collect();
    if n.is_empty() {
        return true;
    }
    let h: Vec<char> = hay.chars().map(fold).collect();
    h.windows(n.len()).any(|w| w == n.as_slice())
}

/// 命中的子串（字节区间，不重叠，从左到右）。needle 为空或没命中返回空
pub fn highlight_ranges(text: &str, needle: &str) -> Vec<Range<usize>> {
    let n: Vec<char> = needle.chars().map(fold).collect();
    if n.is_empty() {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = text.char_indices().map(|(i, c)| (i, fold(c))).collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i + n.len() <= chars.len() {
        if chars[i..i + n.len()].iter().map(|(_, c)| *c).eq(n.iter().copied()) {
            let start = chars[i].0;
            let end = chars.get(i + n.len()).map(|(b, _)| *b).unwrap_or(text.len());
            out.push(start..end);
            i += n.len();
        } else {
            i += 1;
        }
    }
    out
}

/// 应用全部筛选 + 搜索（`filtered`）。`now` 是秒级时间戳
pub fn filter_items<'a>(items: &'a [PaletteItem], f: &PaletteFilter, query: &str, now: i64) -> Vec<&'a PaletteItem> {
    let q = query.trim();
    let window = match f.time {
        TimeFilter::All => None,
        TimeFilter::Today => Some(86400),
        TimeFilter::Week => Some(86400 * 7),
        TimeFilter::Month => Some(86400 * 30),
    };
    items
        .iter()
        .filter(|it| {
            if f.type_ != "全部" && it.item_type != f.type_ {
                return false;
            }
            if !f.project.is_empty() && it.project_root != f.project {
                return false;
            }
            if !f.status.is_empty() {
                let k = match it.status {
                    ItemStatus::Waiting if it.waiting_for == "user" => StatusKey::WaitingUser,
                    ItemStatus::Waiting => StatusKey::WaitingApproval,
                    ItemStatus::Busy => StatusKey::Busy,
                    ItemStatus::Idle => StatusKey::Idle,
                    ItemStatus::Stopped => StatusKey::Stopped,
                    ItemStatus::Archived => StatusKey::Archived,
                };
                if !f.status.contains(&k) {
                    return false;
                }
            }
            if f.pinned_only && !it.pinned {
                return false;
            }
            if let Some(w) = window {
                // TS：`it.mtime && !timeFilter(it.mtime)` —— mtime 为 0 的不参与时间筛选
                if it.mtime != 0 && now - it.mtime >= w {
                    return false;
                }
            }
            if !q.is_empty() {
                let hay = format!("{} {} {} {}", it.title, it.subtitle, it.group, it.item_type);
                if !contains_ci(&hay, q) {
                    return false;
                }
            }
            true
        })
        .collect()
}

pub struct Group<'a> {
    pub name: String,
    pub items: Vec<&'a PaletteItem>,
}

/// 按组名分组（组的顺序 = 第一次出现的顺序），组内：置顶优先，再按排序键
pub fn group_items<'a>(filtered: Vec<&'a PaletteItem>, sort: SortKey) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> = Vec::new();
    for it in filtered {
        match groups.iter_mut().find(|g| g.name == it.group) {
            Some(g) => g.items.push(it),
            None => groups.push(Group { name: it.group.clone(), items: vec![it] }),
        }
    }
    for g in &mut groups {
        g.items.sort_by(|a, b| {
            b.pinned.cmp(&a.pinned).then_with(|| match sort {
                SortKey::Count => b.msg_count.cmp(&a.msg_count),
                SortKey::FirstMsg => a.title.cmp(&b.title),
                SortKey::Recent => b.mtime.cmp(&a.mtime),
            })
        });
    }
    groups
}

/// 没有搜索词时每组最多显示 10 条，有搜索词时全部显示
pub fn max_per_group(query: &str) -> usize {
    if query.trim().is_empty() { 10 } else { 999 }
}

/// 键盘上下选择用的扁平序列：去掉折叠组，每组截到上限
pub fn visible_flat<'a>(groups: &[Group<'a>], collapsed: &[String], query: &str) -> Vec<&'a PaletteItem> {
    let cap = max_per_group(query);
    groups
        .iter()
        .filter(|g| !collapsed.contains(&g.name))
        .flat_map(|g| g.items.iter().take(cap).copied())
        .collect()
}

/// ↑↓：不循环，夹在 [0, len-1]
pub fn step_index(cur: usize, len: usize, delta: i32) -> usize {
    if len == 0 {
        return 0;
    }
    (cur as i64 + delta as i64).clamp(0, len as i64 - 1) as usize
}

// ---------- 搜索历史（`makit-palette-history`）----------

pub const MAX_HISTORY: usize = 8;

/// 执行某一项时存一次：去重提到最前，最多 8 条；空白不存
pub fn push_history(history: &[String], q: &str) -> Vec<String> {
    if q.trim().is_empty() {
        return history.to_vec();
    }
    std::iter::once(q.to_string()).chain(history.iter().filter(|x| *x != q).cloned()).take(MAX_HISTORY).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str) -> SessionMeta {
        SessionMeta {
            session_id: format!("{id}-0000-0000"),
            short_id: id.into(),
            cwd: format!("/Users/me/proj/{id}"),
            git_root: String::new(),
            first_user_msg: String::new(),
            mtime: 1000,
            humanize: "3 分钟前".into(),
            ..crate::state::sessions::tests::session(id)
        }
    }

    fn with(id: &str, f: impl FnOnce(&mut SessionMeta)) -> SessionMeta {
        let mut m = meta(id);
        f(&mut m);
        m
    }

    fn groups_of(items: &[PaletteItem]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for it in items {
            if out.last() != Some(&it.group) {
                out.push(it.group.clone());
            }
        }
        out
    }

    fn sample() -> Vec<SessionMeta> {
        vec![
            with("stop", |m| m.running = false),
            with("arch", |m| {
                m.archived = true;
                m.running = true; // 归档的即使在跑也只进「已归档」
            }),
            with("idle", |m| {
                m.running = true;
                m.status = "idle".into();
            }),
            with("busy", |m| {
                m.running = true;
                m.status = "busy".into();
            }),
            with("appr", |m| {
                m.running = true;
                m.status = "waiting".into();
                m.waiting_for = "tool".into();
            }),
            with("user", |m| {
                m.running = true;
                m.status = "waiting".into();
                m.waiting_for = "user".into();
            }),
        ]
    }

    #[test]
    fn build_items_group_order_and_status() {
        let items = build_items(&sample(), &[], SortKey::Recent);
        assert_eq!(groups_of(&items), ["等待回答", "等待审批", "进行中", "空闲", "已停止", "已归档"]);
        let by = |g: &str| items.iter().find(|i| i.group == g).unwrap();
        assert_eq!(by("等待回答").status, ItemStatus::Waiting);
        assert_eq!(by("等待回答").status_label, "等待回答");
        assert_eq!(by("等待审批").status_label, "等待审批", "waiting_for 不是 user 的都算等待审批");
        assert_eq!(by("进行中").status, ItemStatus::Busy);
        assert_eq!(by("空闲").status, ItemStatus::Idle);
        assert_eq!(by("已停止").status, ItemStatus::Stopped);
        assert_eq!(by("已归档").status, ItemStatus::Archived);
        assert_eq!(by("已归档").status_label, "已归档");
        assert_eq!(items.len(), 6, "每条会话只进一个组");
    }

    #[test]
    fn build_items_fields() {
        let s = vec![with("abc", |m| {
            m.cwd = "/Users/me/code/makit".into();
            m.git_root = "/Users/me/code".into();
            m.git_branch = "feat".into();
            m.display_name = "  修复\n滚动  ".into();
            m.user_msg_count = 7;
            m.mtime = 42;
        })];
        let it = &build_items(&s, &["abc-0000-0000".into()], SortKey::Recent)[0];
        assert_eq!(it.id, "s:abc-0000-0000:已停止");
        assert_eq!(it.title, "修复 滚动", "标题同 deriveSessionTabLabel（压平空白）");
        assert_eq!(it.subtitle, "makit · abc · @feat");
        assert_eq!(it.hint, "3 分钟前");
        assert_eq!(it.project_root, "/Users/me/code", "项目根优先 git_root");
        assert_eq!(it.item_type, "Session");
        assert!(it.pinned);
        assert_eq!((it.mtime, it.msg_count), (42, 7));
        let no_branch = &build_items(&[meta("x")], &[], SortKey::Recent)[0];
        assert_eq!(no_branch.subtitle, "x · x", "没分支时不带 @ 段");
        assert_eq!(no_branch.project_root, "/Users/me/proj/x", "没 git_root 用 cwd");
        let untitled = &build_items(&[meta("q")], &[], SortKey::Recent)[0];
        assert_eq!(untitled.title, "[q]");
    }

    #[test]
    fn stopped_and_archived_capped_at_30_by_sort_key() {
        let mut s: Vec<SessionMeta> = (0..40).map(|i| with(&format!("s{i:02}"), |m| m.mtime = i)).collect();
        s.extend((0..35).map(|i| with(&format!("a{i:02}"), |m| {
            m.archived = true;
            m.mtime = i;
        })));
        let items = build_items(&s, &[], SortKey::Recent);
        let stopped: Vec<_> = items.iter().filter(|i| i.group == "已停止").collect();
        assert_eq!(stopped.len(), 30);
        assert_eq!(stopped[0].mtime, 39, "最近的在前");
        assert_eq!(stopped[29].mtime, 10, "最旧的 10 条被截掉");
        assert_eq!(items.iter().filter(|i| i.group == "已归档").count(), 30);
        // 按消息数排时截的是消息最少的
        let mut c: Vec<SessionMeta> = (0..31).map(|i| with(&format!("c{i:02}"), |m| m.user_msg_count = i)).collect();
        c[0].mtime = 99999; // 最近但消息最少
        let items = build_items(&c, &[], SortKey::Count);
        assert!(!items.iter().any(|i| i.session_id.starts_with("c00")), "count 排序截掉的是消息最少的，不看时间");
    }

    #[test]
    fn sort_sessions_pinned_first_then_key() {
        let a = with("a", |m| {
            m.mtime = 1;
            m.user_msg_count = 9;
            m.first_user_msg = "b".into();
        });
        let b = with("b", |m| {
            m.mtime = 3;
            m.user_msg_count = 1;
            m.first_user_msg = "a".into();
        });
        let c = with("c", |m| {
            m.mtime = 2;
            m.user_msg_count = 5;
            m.first_user_msg = "c".into();
        });
        let ids = |v: Vec<&SessionMeta>| v.iter().map(|m| m.short_id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(sort_sessions(vec![&a, &b, &c], SortKey::Recent, &[])), ["b", "c", "a"]);
        assert_eq!(ids(sort_sessions(vec![&a, &b, &c], SortKey::Count, &[])), ["a", "c", "b"]);
        assert_eq!(ids(sort_sessions(vec![&a, &b, &c], SortKey::FirstMsg, &[])), ["b", "a", "c"]);
        assert_eq!(ids(sort_sessions(vec![&a, &b, &c], SortKey::Recent, &["a-0000-0000".into()])), ["a", "b", "c"], "置顶永远最前");
    }

    #[test]
    fn filter_by_status_project_pinned_time_and_query() {
        let mut s = sample();
        s[0].mtime = 1000 - 2 * 86400; // stop：两天前
        s[2].git_root = "/r/other".into(); // idle
        let items = build_items(&s, &["busy-0000-0000".into()], SortKey::Recent);
        let now = 1000;
        let ids = |v: Vec<&PaletteItem>| v.iter().map(|i| i.session_id[..4].to_string()).collect::<Vec<_>>();

        let f = PaletteFilter { status: vec![StatusKey::WaitingUser], ..Default::default() };
        assert_eq!(ids(filter_items(&items, &f, "", now)), ["user"]);
        let f = PaletteFilter { status: vec![StatusKey::WaitingApproval, StatusKey::Stopped], ..Default::default() };
        assert_eq!(ids(filter_items(&items, &f, "", now)), ["appr", "stop"]);
        let f = PaletteFilter { status: vec![StatusKey::Archived], ..Default::default() };
        assert_eq!(ids(filter_items(&items, &f, "", now)), ["arch"]);

        let f = PaletteFilter { project: "/r/other".into(), ..Default::default() };
        assert_eq!(ids(filter_items(&items, &f, "", now)), ["idle"]);

        let f = PaletteFilter { pinned_only: true, ..Default::default() };
        assert_eq!(ids(filter_items(&items, &f, "", now)), ["busy"]);

        let f = PaletteFilter { time: TimeFilter::Today, ..Default::default() };
        assert!(!ids(filter_items(&items, &f, "", now)).contains(&"stop".to_string()), "两天前的不算今天");
        let f = PaletteFilter { time: TimeFilter::Week, ..Default::default() };
        assert!(ids(filter_items(&items, &f, "", now)).contains(&"stop".to_string()));
        let mut zero = items.clone();
        zero.iter_mut().find(|i| i.session_id.starts_with("stop")).unwrap().mtime = 0;
        let f = PaletteFilter { time: TimeFilter::Today, ..Default::default() };
        assert_eq!(filter_items(&zero, &f, "", now).len(), items.len(), "mtime=0 不参与时间筛选");

        let f = PaletteFilter { type_: "Other".into(), ..Default::default() };
        assert!(filter_items(&items, &f, "", now).is_empty(), "type 标签不是全部时按 type 过滤");

        // 搜索：title + subtitle + group + type，大小写不敏感，前后空白忽略
        let all = PaletteFilter::default();
        assert_eq!(ids(filter_items(&items, &all, "  IDLE ", now)), ["idle"], "命中 subtitle 里的 short_id");
        assert_eq!(ids(filter_items(&items, &all, "等待审批", now)), ["appr"], "命中组名");
        assert_eq!(filter_items(&items, &all, "session", now).len(), items.len(), "命中 type");
        assert!(filter_items(&items, &all, "不存在的词", now).is_empty());
    }

    #[test]
    fn group_items_keeps_group_order_and_sorts_inside() {
        let mk = |id: &str, g: &str, pinned: bool, mtime: i64, cnt: u32, title: &str| PaletteItem {
            id: id.into(),
            session_id: id.into(),
            title: title.into(),
            subtitle: String::new(),
            hint: String::new(),
            group: g.into(),
            item_type: "Session".into(),
            project_root: String::new(),
            status: ItemStatus::Stopped,
            status_label: String::new(),
            waiting_for: String::new(),
            running: false,
            pinned,
            archived: false,
            mtime,
            msg_count: cnt,
        };
        let items = vec![
            mk("1", "B", false, 1, 5, "z"),
            mk("2", "A", false, 9, 1, "y"),
            mk("3", "B", true, 0, 0, "x"),
            mk("4", "B", false, 5, 9, "a"),
        ];
        let g = group_items(items.iter().collect(), SortKey::Recent);
        assert_eq!(g.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["B", "A"], "组顺序 = 第一次出现的顺序");
        fn ids(g: &Group) -> Vec<String> {
            g.items.iter().map(|i| i.id.clone()).collect()
        }
        assert_eq!(ids(&g[0]), ["3", "4", "1"], "置顶优先，再按时间倒序");
        let g = group_items(items.iter().collect(), SortKey::Count);
        assert_eq!(ids(&g[0]), ["3", "4", "1"]);
        let g = group_items(items.iter().collect(), SortKey::FirstMsg);
        assert_eq!(ids(&g[0]), ["3", "4", "1"], "首条消息按标题升序（a < z）");
    }

    #[test]
    fn per_group_cap_and_flat_index_skip_collapsed() {
        assert_eq!(max_per_group(""), 10);
        assert_eq!(max_per_group("   "), 10, "只有空白等于没搜索");
        assert!(max_per_group("x") >= 999, "有搜索词时全部显示");
        let s: Vec<SessionMeta> = (0..15).map(|i| with(&format!("s{i:02}"), |m| m.mtime = i)).collect();
        let mut a = with("run", |m| m.running = true);
        a.status = "busy".into();
        let mut all = s.clone();
        all.push(a);
        let items = build_items(&all, &[], SortKey::Recent);
        let groups = group_items(items.iter().collect(), SortKey::Recent);
        assert_eq!(visible_flat(&groups, &[], "").len(), 1 + 10, "已停止组截到 10");
        assert_eq!(visible_flat(&groups, &["已停止".into()], "").len(), 1, "折叠的组不进扁平序列");
        assert_eq!(visible_flat(&groups, &[], "s").len(), 16 - 0, "有搜索词不截（这里 16 条都命中 s）");
    }

    #[test]
    fn step_index_does_not_wrap() {
        assert_eq!(step_index(0, 5, -1), 0);
        assert_eq!(step_index(4, 5, 1), 4);
        assert_eq!(step_index(2, 5, 1), 3);
        assert_eq!(step_index(0, 0, 1), 0, "空列表停在 0");
    }

    #[test]
    fn highlight_uses_the_same_rule_as_search() {
        assert_eq!(highlight_ranges("Makit makit", "MAK"), vec![0..3, 6..9], "大小写不敏感、全部命中");
        assert_eq!(highlight_ranges("aaaa", "aa"), vec![0..2, 2..4], "不重叠");
        assert_eq!(highlight_ranges("修复滚动条滚动", "滚动"), vec![6..12, 15..21], "中文按字节区间");
        assert!(highlight_ranges("abc", "").is_empty());
        assert!(highlight_ranges("abc", "x").is_empty());
        assert!(contains_ci("Hello 世界", "hello 世"));
        assert!(!contains_ci("abc", "abd"));
        assert!(contains_ci("abc", ""), "空搜索词算命中");
        // 每个高亮命中都必须能被搜索找到（反之亦然）
        for (t, q) in [("ÄBC", "äb"), ("SessionX", "sionx"), ("İx", "x")] {
            assert_eq!(contains_ci(t, q), !highlight_ranges(t, q).is_empty(), "{t} / {q}");
        }
    }

    #[test]
    fn history_dedupes_and_caps() {
        let h: Vec<String> = (0..8).map(|i| format!("q{i}")).collect();
        let n = push_history(&h, "q3");
        assert_eq!(n[0], "q3");
        assert_eq!(n.len(), 8);
        assert_eq!(n.iter().filter(|x| *x == "q3").count(), 1);
        let n = push_history(&h, "new");
        assert_eq!(n.len(), 8, "最多 8 条");
        assert_eq!(n[0], "new");
        assert!(!n.contains(&"q7".to_string()), "最旧的挤掉");
        assert_eq!(push_history(&h, "   "), h, "空白不存");
    }

    #[test]
    fn filter_json_roundtrip_and_lenient_parse() {
        let v: Value = serde_json::from_str(r#"{"type":"全部","project":"/p","time":"week","status":["busy","waiting_user","???"],"pinnedOnly":true}"#).unwrap();
        let f = PaletteFilter::from_json(Some(&v));
        assert_eq!(f.project, "/p");
        assert_eq!(f.time, TimeFilter::Week);
        assert_eq!(f.status, vec![StatusKey::Busy, StatusKey::WaitingUser], "认不出的状态跳过");
        assert!(f.pinned_only);
        assert_eq!(PaletteFilter::from_json(Some(&f.to_json())), f, "往返");
        assert_eq!(PaletteFilter::from_json(None), PaletteFilter::default());
        let partial: Value = serde_json::from_str(r#"{"time":"bogus","project":3}"#).unwrap();
        assert_eq!(PaletteFilter::from_json(Some(&partial)), PaletteFilter::default(), "坏字段用默认");
    }

    #[test]
    fn has_filters_and_clear_keep_type() {
        let mut f = PaletteFilter::default();
        assert!(!f.has_filters());
        f.type_ = "Session".into();
        assert!(!f.has_filters(), "type 标签不算筛选");
        f.time = TimeFilter::Month;
        assert!(f.has_filters());
        f.pinned_only = true;
        f.project = "/p".into();
        f.toggle_status(StatusKey::Idle);
        f.clear();
        assert!(!f.has_filters());
        assert_eq!(f.type_, "Session", "清除筛选不重置 type");
    }

    #[test]
    fn misc_small_rules() {
        assert_eq!(SortKey::parse(Some("count")), SortKey::Count);
        assert_eq!(SortKey::parse(Some("firstMsg")), SortKey::FirstMsg);
        assert_eq!(SortKey::parse(Some("weird")), SortKey::Recent);
        assert_eq!(SortKey::parse(None), SortKey::Recent);
        assert!(shows_status_pill(ItemStatus::Waiting) && shows_status_pill(ItemStatus::Archived));
        assert!(!shows_status_pill(ItemStatus::Busy) && !shows_status_pill(ItemStatus::Idle) && !shows_status_pill(ItemStatus::Stopped));
        assert_eq!(project_label("/Users/me/code/makit"), "makit");
        let s = vec![
            with("a", |m| {
                m.git_root = "/r/1".into();
                m.mtime = 5;
            }),
            with("b", |m| {
                m.git_root = "/r/2".into();
                m.mtime = 9;
            }),
            with("c", |m| {
                m.git_root = "/r/1".into();
                m.mtime = 1;
            }),
        ];
        assert_eq!(project_roots(&s), ["/r/2", "/r/1"], "去重、最近活跃的项目在前");
    }
}
