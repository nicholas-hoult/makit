//! 侧栏视图（界面清单 D 节，B 侧栏包）。对照源：`src/SessionTree.tsx` / `SessionTree.css`。
//!
//! 分层：
//! - 纯逻辑（带测试）：`groups`（分组链 / 日期分段 / 项目分组 / 过滤排序 / 标题）、`collapse`（项目组折叠编码）、
//!   `tree`（拍平 + ↑↓ 顺序 + 虚拟列表几何）、`hover`（悬停卡的行和时序）
//! - 视图：本文件（状态 + 动作）、`render`（头部 / 列表行 / 滚动条）、`popups`（显示选项、右键菜单、
//!   工具选择器、悬停卡）、`search_input`（搜索框）
//!
//! **虚拟列表选型**（#219 TRD 对标 VS Code）：行高不统一（组头 19 / 会话 43 / 项目头 26 / 间距），
//! `uniform_list` 不适用；用 GPUI 的 `list` + `ListState`（只布局视口内的行，Zed 的聊天 / 大纲都用它），
//! 但**行高按类型固定成常量**（`tree.rs`），top 表自己算：⌘L 居中、↑↓ 贴边、滚动条都靠它，
//! 不依赖 ListState 里「量过没量过」。数据变化时按行高序列做前后缀 diff，只 splice 变了的一段
//! （`tree::splice_plan`），滚动位置和已量的行都保住。
//!
//! 数据：`AppState.sessions`（按 mtime 降序）+ `AppState.prefs.sidebar` / `pinned_sessions`（改了会存盘）。
//! 打开会话：改 `state.workspace` 后 `workspace_changed`，工作区视图会自己起终端、给焦点。
//!
//! 和别的包的接口（见报告）：
//! - `SidebarEvent::ReturnFocus` → 根视图调 `WorkspaceView::refocus`（Esc 把焦点还给终端）
//! - `DraggedSession`：会话行拖出去的数据，C 工作区包在 pane / 标签条上 `on_drop::<DraggedSession>`
//! - toast、「查看对话」、通用右键菜单归 D 浮层包；这里先用最小实现（`toast` 只打日志）

pub mod collapse;
pub mod groups;
pub mod hover;
mod popups;
mod render;
pub mod search_input;
pub mod selftest;
pub mod tree;

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use gpui::{
    px, App, AppContext, Context, Entity, EventEmitter, FocusHandle, Focusable, ListAlignment, ListOffset, ListState, Pixels, Point,
    Subscription, Task, WeakEntity, Window,
};
use makit_core::SessionMeta;

use crate::state::{AppEvent, AppState};
use crate::workspace::model::{opened_order, resume_cmd, resume_init_command, Dir, Side, TabKind, TabSpec};
use groups::{day_buckets, filter_sessions, meta_state, project_groups, session_title, status_groups, RunState, SortKey};
use search_input::{SearchEvent, SearchInput};
use tree::{flatten, GroupRef, Tree, TreeGroup, TreeInput, TreeProject};

/// 侧栏发给外面的事件
pub enum SidebarEvent {
    /// Esc：把焦点还给当前终端
    ReturnFocus,
}

/// 从侧栏拖出去的会话（C 工作区包接：`on_drop::<DraggedSession>`，用 `tab_spec()` 开 pane）
#[derive(Clone, Debug)]
pub struct DraggedSession {
    pub session_id: String,
    pub short_id: String,
    pub cwd: String,
    /// 拖影上显示的名字（同 tab 标题）
    pub label: String,
    /// "claude" / "codex"
    pub tool: String,
}

impl DraggedSession {
    pub fn from_meta(m: &SessionMeta) -> Self {
        Self {
            session_id: m.session_id.clone(),
            short_id: m.short_id.clone(),
            cwd: m.cwd.clone(),
            label: session_title(&m.display_name, &m.first_user_msg, &m.short_id),
            tool: m.tool.clone(),
        }
    }

    fn tool_opt(&self) -> Option<&str> {
        (self.tool == "codex").then_some("codex")
    }

    /// 落到工作区时开的 tab（kind=resume、initCommand=`clear && <恢复命令>`，同 TS 的 PANE_SPEC_MIME）
    pub fn tab_spec(&self) -> TabSpec {
        TabSpec {
            kind: TabKind::Resume,
            cwd: self.cwd.clone(),
            init_command: Some(resume_init_command(&self.session_id, self.tool_opt())),
            session_id: Some(self.session_id.clone()),
            session_short_id: Some(self.short_id.clone()),
        }
    }
}

/// 侧栏宽度拖动条被拖着
#[derive(Clone)]
struct ResizeDrag;

/// 滚动条滑块被拖着
#[derive(Clone)]
struct ThumbDrag;

/// 弹出的东西（同一时刻只有一个）
#[derive(Clone)]
enum Popup {
    /// 「显示选项」菜单
    Options,
    /// 会话行右键
    SessionMenu { at: Point<Pixels>, session_id: String },
    /// 项目头右键
    ProjectMenu { at: Point<Pixels>, key: String, cwd: String },
    /// 项目头「+」：选 Claude / Codex
    ToolPicker { at: Point<Pixels>, cwd: String },
}

/// 悬停卡：哪条会话、左上角在哪
#[derive(Clone)]
struct HoverCard {
    session_id: String,
    at: Point<Pixels>,
}

pub struct SidebarView {
    state: Entity<AppState>,
    search: Entity<SearchInput>,
    query: String,
    list_focus: FocusHandle,
    menu_focus: FocusHandle,
    list: ListState,
    tree: Tree,
    /// 每项的 top（tree::tops），最后一个是总高
    tops: Vec<f32>,
    heights: Vec<f32>,
    /// 项目视图里全部项目组的折叠键（「只看这个项目」用）
    project_keys: Vec<String>,
    /// 键盘选中（和「当前打开的」是两个正交状态）
    selected: Option<String>,
    popup: Option<Popup>,
    /// 点在菜单外关掉菜单的那次按下的位置：同一次按下落在「显示选项」按钮上时不要又打开
    closed_by_outside_at: Option<Point<Pixels>>,
    hover: Option<HoverCard>,
    hover_task: Option<Task<()>>,
    refreshing: bool,
    resizing: bool,
    /// 鼠标在拖宽条上悬停（标题栏的竖线跟着亮，见 workspace::titlebar::TitlebarResizerHot）
    resizer_hovered: bool,
    /// 滚动条显形（滚动后 900ms 淡出，同 scrollActivity.ts）
    scroll_active: bool,
    scroll_task: Option<Task<()>>,
    thumb_dragging: bool,
    /// 按下滑块时鼠标在滑块里的纵向偏移
    thumb_grab: f32,
    /// 侧栏本体在窗口里的位置（上一帧），浮层定位用
    aside_bounds: std::rc::Rc<std::cell::Cell<gpui::Bounds<Pixels>>>,
    pending_focus_search: bool,
    /// ⌘L 之后等行真的布局出来再滚（最多重试几帧，同 TS 的 10 帧重试）
    pending_reveal: Option<(String, u8)>,
    pending_focus_list: bool,
    _subs: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for SidebarView {}

impl Focusable for SidebarView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.list_focus.clone()
    }
}

/// toast 走 D 浮层包（时长常量在 overlays::toast，照 Tauri 版）
fn toast(msg: String, ms: u64, cx: &mut App) {
    crate::overlays::show_toast(msg, ms, cx);
}

impl SidebarView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(SearchInput::new);
        let list = ListState::new(0, ListAlignment::Top, px(200.));
        let weak: WeakEntity<Self> = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            if let Some(v) = weak.upgrade() {
                v.update(cx, |this, cx| this.on_scrolled(cx));
            }
        });
        let subs = vec![
            cx.observe(&state, |this, _, cx| {
                this.rebuild(cx);
                cx.notify();
            }),
            cx.subscribe(&state, |this, _, ev: &AppEvent, cx| {
                if matches!(ev, AppEvent::SessionsChanged) && this.refreshing {
                    this.refreshing = false;
                    cx.notify();
                }
            }),
            cx.subscribe(&search, |this, _, ev: &SearchEvent, cx| match ev {
                SearchEvent::Changed(q) => {
                    this.query = q.clone();
                    this.rebuild(cx);
                    cx.notify();
                }
                SearchEvent::ToList => {
                    this.pending_focus_list = true;
                    this.selected = this.tree.order.first().and_then(|&r| this.session_id_at(r, cx));
                    this.scroll_selected_into_view(cx);
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            state,
            search,
            query: String::new(),
            list_focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            list,
            tree: Tree::default(),
            tops: vec![0.0],
            heights: Vec::new(),
            project_keys: Vec::new(),
            selected: None,
            popup: None,
            closed_by_outside_at: None,
            hover: None,
            hover_task: None,
            refreshing: false,
            resizing: false,
            resizer_hovered: false,
            scroll_active: false,
            scroll_task: None,
            thumb_dragging: false,
            thumb_grab: 0.0,
            aside_bounds: Default::default(),
            pending_focus_search: false,
            pending_reveal: None,
            pending_focus_list: false,
            _subs: subs,
        };
        this.rebuild(cx);
        this
    }

    fn session_id_at(&self, row: usize, cx: &App) -> Option<String> {
        self.state.read(cx).sessions.get(row).map(|m| m.session_id.clone())
    }

    fn row_of(&self, id: &str, cx: &App) -> Option<usize> {
        self.state.read(cx).sessions.iter().position(|m| m.session_id == id)
    }

    fn active_session_id(&self, cx: &App) -> Option<String> {
        self.state.read(cx).workspace.active_tab().and_then(|t| t.session_id.clone())
    }

    // ---------- 数据 → 拍平的列表 ----------

    /// 重算分组 / 拍平，并把变化的那段告诉虚拟列表
    fn rebuild(&mut self, cx: &App) {
        let s = self.state.read(cx);
        let p = &s.prefs.sidebar;
        let list = &s.sessions;
        let filtered = filter_sessions(list, &self.query, p.show_archived);
        let opened: HashMap<String, usize> = opened_order(&s.workspace.state).into_iter().enumerate().map(|(i, id)| (id, i)).collect();
        let pinned: HashSet<String> = s.prefs.pinned_sessions.iter().cloned().collect();
        let sort = SortKey::parse(&p.sort);
        let g = status_groups(list, &filtered, &opened, &pinned, sort);
        let project_view = p.view == "project";
        let group = |id: &str, label: &str, warn: bool, rows: &[usize]| TreeGroup { id: id.into(), label: label.into(), warn, rows: rows.to_vec() };
        let top = vec![group("pinned", "置顶", false, &g.pinned), group("opened", "已打开", false, &g.opened)];
        let (mut labeled, mut history, mut projects) = (Vec::new(), Vec::new(), Vec::new());
        self.project_keys.clear();
        if project_view {
            let rest: Vec<usize> = filtered.iter().copied().filter(|&i| !opened.contains_key(&list[i].session_id) && !pinned.contains(&list[i].session_id)).collect();
            for pg in project_groups(list, &rest) {
                let key = pg.key();
                self.project_keys.push(key.clone());
                projects.push(TreeProject {
                    collapsed: collapse::is_project_collapsed(&p.proj_collapsed, &key, pg.has_active),
                    cwd: pg.cwd(list),
                    key,
                    name: pg.name,
                    has_active: pg.has_active,
                    rows: pg.rows,
                });
            }
        } else {
            labeled = vec![
                group("attention", "需要回应", true, &g.attention),
                group("busy", groups::status_label::BUSY, false, &g.busy),
                group("idle", groups::status_label::IDLE, false, &g.idle),
            ];
            // 历史按日期分段只在「最近活动」排序时做；别的排序是一个没有标题、不可折叠的组
            history = if sort == SortKey::Recent {
                day_buckets(list, &g.history, now_secs()).into_iter().map(|b| TreeGroup { id: b.id, label: b.label, warn: false, rows: b.rows }).collect()
            } else {
                vec![group("history", "", false, &g.history)]
            };
        }
        let empty_text = if !s.loaded {
            "加载中…"
        } else if !self.query.trim().is_empty() {
            "无匹配 session"
        } else {
            "无 session"
        };
        let alive = |i: usize| meta_state(&list[i]) != RunState::Stopped;
        let tree = flatten(&TreeInput {
            top,
            project_view,
            labeled,
            history,
            projects,
            collapsed_groups: &p.group_collapsed,
            filtered_count: filtered.len(),
            empty_text: empty_text.into(),
            alive: &alive,
        });
        let heights: Vec<f32> = tree.items.iter().map(|i| i.height()).collect();
        if let Some((range, count)) = tree::splice_plan(&self.heights, &heights) {
            self.list.splice(range, count);
        }
        self.tops = tree::tops(&tree.items);
        self.heights = heights;
        self.tree = tree;
    }

    // ---------- 偏好 ----------

    fn update_prefs(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut crate::persist::state::SidebarPrefs)) {
        self.state.update(cx, |s, cx| s.update_prefs(cx, |p| f(&mut p.sidebar)));
    }

    fn toggle_group(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_string();
        self.update_prefs(cx, |p| {
            if let Some(i) = p.group_collapsed.iter().position(|x| *x == id) {
                p.group_collapsed.remove(i);
            } else {
                p.group_collapsed.push(id);
            }
        });
    }

    fn toggle_project(&mut self, key: &str, has_active: bool, cx: &mut Context<Self>) {
        let key = key.to_string();
        self.update_prefs(cx, |p| p.proj_collapsed = collapse::toggle_project_collapsed(&p.proj_collapsed, &key, has_active));
    }

    fn only_this_project(&mut self, key: &str, cx: &mut Context<Self>) {
        let (keys, key) = (self.project_keys.clone(), key.to_string());
        self.update_prefs(cx, |p| p.proj_collapsed = collapse::collapse_other_projects(&p.proj_collapsed, &keys, &key));
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.refreshing = true;
        self.state.update(cx, |s, cx| s.refresh(cx));
        cx.notify();
    }

    // ---------- 打开会话 ----------

    /// 同 App.tsx 的 openResumeTab：已经开着就切过去；在外面跑着（有 pid）就拒绝重复启动；
    /// 否则先补存储路径的软链再开 resume tab（刻意不预检 cwd 在不在，由终端那道闸统一判）
    pub fn open_session(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(m) = self.state.read(cx).session(id).cloned() else { return };
        let already_open = opened_order(&self.state.read(cx).workspace.state).iter().any(|x| *x == m.session_id);
        if !already_open && m.running && m.pid != 0 {
            toast(format!("该 session 正在运行中（PID {}），不能重复启动", m.pid), crate::overlays::toast::ALREADY_RUNNING, cx);
            return;
        }
        if !already_open && !m.cwd.is_empty() && !m.storage_folder.is_empty() {
            let _ = makit_core::recovery::ensure_session_symlink(m.session_id.clone(), m.cwd.clone(), m.storage_folder.clone());
        }
        let title = session_title(&m.display_name, &m.first_user_msg, &m.short_id);
        let tool = (m.tool == "codex").then_some("codex");
        self.state.update(cx, |s, cx| {
            s.workspace.open_session(&m.session_id, &m.short_id, &m.cwd, Some(&title), tool);
            s.workspace_changed(cx);
        });
    }

    /// ⌘Enter 左右 / ⇧Enter 上下分屏打开（同 treeOnOpenSessionInSplit：在当前 pane 后面分出一个）
    fn open_split(&mut self, id: &str, dir: Dir, cx: &mut Context<Self>) {
        let Some(m) = self.state.read(cx).session(id).cloned() else { return };
        let spec = DraggedSession::from_meta(&m).tab_spec();
        self.state.update(cx, |s, cx| {
            let cid = s.workspace.state.active_container_id.clone();
            s.workspace.split_with_session(&cid, dir, Side::After, &spec);
            s.workspace_changed(cx);
        });
    }

    fn new_session_in(&mut self, cwd: &str, tool: &str, cx: &mut Context<Self>) {
        let tool = (tool == "codex").then_some("codex");
        self.state.update(cx, |s, cx| {
            s.workspace.open_new_session(cwd, tool);
            s.workspace_changed(cx);
        });
    }

    fn new_shell_in(&mut self, cwd: &str, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.workspace.open_shell(cwd);
            s.workspace_changed(cx);
        });
    }

    /// 归档 / 取消归档（App.tsx handleToggleArchive）：在跑的先弹原生确认；归档后关掉它的所有标签
    /// （连同子进程），乐观地把 running 置 false
    fn toggle_archive(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(m) = self.state.read(cx).session(id).cloned() else { return };
        if m.archived {
            match makit_core::archive::unarchive_session(m.session_id.clone()) {
                Ok(()) => self.patch_session(&m.session_id, cx, |x| x.archived = false),
                Err(e) => toast(format!("归档失败: {e}"), crate::overlays::toast::ARCHIVE_FAIL, cx),
            }
            return;
        }
        if !m.running {
            self.do_archive(m, cx);
            return;
        }
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "确定归档？",
            Some(&format!("该 session 正在运行中（PID {}），归档将关闭终端并杀死子进程。", m.pid)),
            &["归档", "取消"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await.ok() == Some(0) {
                let _ = this.update(cx, |this, cx| this.do_archive(m, cx));
            }
        })
        .detach();
    }

    fn do_archive(&mut self, m: SessionMeta, cx: &mut Context<Self>) {
        let id = m.session_id.clone();
        self.state.update(cx, |s, _| {
            s.archiving.insert(id.clone());
        });
        let res = makit_core::archive::archive_session(id.clone());
        self.state.update(cx, |s, _| {
            s.archiving.remove(&id);
        });
        if let Err(e) = res {
            toast(format!("归档失败: {e}"), crate::overlays::toast::ARCHIVE_FAIL, cx);
            return;
        }
        // 关它的全部标签。子进程先杀（closeTabWithChildren：逃出进程组的那些不杀会留在机器上）
        let pids: Vec<u32> = m.child_processes.iter().map(|p| p.pid).collect();
        if !pids.is_empty() {
            makit_core::process::kill_pids(&pids);
        }
        self.state.update(cx, |s, cx| {
            let tabs: Vec<(String, String)> = crate::workspace::model::collect_containers(&s.workspace.state.root)
                .iter()
                .flat_map(|c| c.tabs.iter().filter(|t| t.session_id.as_deref() == Some(id.as_str())).map(|t| (c.id.clone(), t.id.clone())))
                .collect();
            for (cid, tid) in tabs.into_iter().rev() {
                s.workspace.close_tab(&cid, &tid);
            }
            s.workspace_changed(cx);
        });
        self.patch_session(&m.session_id, cx, |x| {
            x.archived = true;
            x.running = false;
            x.status = "idle".into();
        });
    }

    /// 乐观地改一条会话（归档状态），下一次增量 / 全量扫描会覆盖成真值
    fn patch_session(&mut self, id: &str, cx: &mut Context<Self>, f: impl FnOnce(&mut SessionMeta)) {
        self.state.update(cx, |s, cx| {
            if let Some(x) = s.sessions.iter_mut().find(|x| x.session_id == id) {
                f(x);
                s.sessions_changed(cx);
            }
        });
    }

    fn toggle_pin(&mut self, id: &str, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.toggle_pin(id, cx));
    }

    fn copy(&self, text: String, cx: &mut Context<Self>) {
        // 侧栏菜单里的复制是静默的，不弹 toast（清单 M 节）
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
    }

    fn reveal_in_finder(&self, path: String) {
        let _ = makit_core::paths::open_path(path, true);
    }

    /// 复制恢复命令：`cd <cwd> && <恢复命令>`（cwd 优先用 last_cwd）
    fn resume_command_line(m: &SessionMeta) -> String {
        let cwd = if m.last_cwd.is_empty() { &m.cwd } else { &m.last_cwd };
        format!("cd {cwd} && {}", resume_cmd(&m.session_id, (m.tool == "codex").then_some("codex")))
    }

    // ---------- 键盘导航（D7）----------

    fn move_selection(&mut self, down: bool, cx: &mut Context<Self>) {
        let cur = self.selected.as_deref().and_then(|id| self.row_of(id, cx));
        let next = tree::move_selection(&self.tree.order, cur, down);
        self.selected = next.and_then(|r| self.session_id_at(r, cx));
        self.scroll_selected_into_view(cx);
        cx.notify();
    }

    fn viewport_h(&self) -> f32 {
        f32::from(self.list.viewport_bounds().size.height)
    }

    fn scroll_top(&self) -> f32 {
        let o = self.list.logical_scroll_top();
        self.tops.get(o.item_ix).copied().unwrap_or(0.0) + f32::from(o.offset_in_item)
    }

    fn set_scroll_top(&mut self, y: f32) {
        let (ix, off) = tree::offset_to_item(&self.tops, y);
        self.list.scroll_to(ListOffset { item_ix: ix, offset_in_item: px(off) });
    }

    /// 选中行滚进视野（block: nearest）
    fn scroll_selected_into_view(&mut self, cx: &App) {
        let Some(row) = self.selected.as_deref().and_then(|id| self.row_of(id, cx)) else { return };
        let Some(ix) = self.tree.item_of_row(row) else { return };
        if let Some(y) = tree::reveal_nearest(&self.tops, ix, self.scroll_top(), self.viewport_h()) {
            self.set_scroll_top(y);
        }
    }

    fn selected_meta(&self, cx: &App) -> Option<(usize, SessionMeta)> {
        let id = self.selected.as_deref()?;
        let row = self.row_of(id, cx)?;
        // 只认过滤后还在列表里的（折叠组里的也算，它仍在 membership 里）
        self.tree.membership.contains_key(&row).then(|| (row, self.state.read(cx).sessions[row].clone()))
    }

    /// ← / →：折叠 / 展开选中项所在的组；已经是目标状态就不动；不分段的历史组是 no-op。
    /// ← 之后**故意不清选中**：紧接着按 → 还能沿同一条路找回这个组
    fn collapse_selected_group(&mut self, want_collapsed: bool, cx: &mut Context<Self>) {
        let Some((row, _)) = self.selected_meta(cx) else { return };
        match self.tree.membership.get(&row).cloned() {
            Some(GroupRef::Project { key, has_active }) => {
                let now = collapse::is_project_collapsed(&self.state.read(cx).prefs.sidebar.proj_collapsed, &key, has_active);
                if now != want_collapsed {
                    self.toggle_project(&key, has_active, cx);
                }
            }
            Some(GroupRef::Status(id)) => {
                let now = self.state.read(cx).prefs.sidebar.group_collapsed.contains(&id);
                if now != want_collapsed {
                    self.toggle_group(&id, cx);
                }
            }
            _ => {}
        }
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.hover = None;
        self.hover_task = None;
        cx.emit(SidebarEvent::ReturnFocus);
        cx.notify();
    }

    // ---------- ⌘⇧F / ⌘L（根视图转发过来）----------

    /// ⌘⇧F：展开侧栏（根视图做）、聚焦搜索框并全选。侧栏可能这一帧才重新出现，所以挪到 render 里做
    pub fn request_focus_search(&mut self, cx: &mut Context<Self>) {
        self.pending_focus_search = true;
        cx.notify();
    }

    /// ⌘L（revealSidebarSession + revealTrigger）：清空搜索和「显示已归档」，选中当前会话、焦点进侧栏，
    /// 展开它所在的项目组（`expand_project_for_reveal`，默认展开的不写显式态）和被折叠的状态组，滚到正中
    pub fn reveal_active(&mut self, cx: &mut Context<Self>) {
        let id = self.active_session_id(cx);
        self.reveal_session(id, cx);
    }

    /// 定位到指定会话（⌘L 传当前会话；通知中心跳转时会话不在任何标签里，传那条通知的会话）
    pub fn reveal_session(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.set_text("", cx));
        self.query.clear();
        if self.state.read(cx).prefs.sidebar.show_archived {
            self.update_prefs(cx, |p| p.show_archived = false);
        }
        let Some(id) = id else {
            self.rebuild(cx);
            cx.notify();
            return;
        };
        self.rebuild(cx);
        if let Some(row) = self.row_of(&id, cx) {
            match self.tree.membership.get(&row).cloned() {
                Some(GroupRef::Project { key, has_active }) => {
                    let set = self.state.read(cx).prefs.sidebar.proj_collapsed.clone();
                    if let Some(next) = collapse::expand_project_for_reveal(&set, &key, has_active) {
                        self.update_prefs(cx, |p| p.proj_collapsed = next);
                    }
                }
                Some(GroupRef::Status(gid)) if self.state.read(cx).prefs.sidebar.group_collapsed.contains(&gid) => {
                    self.update_prefs(cx, |p| p.group_collapsed.retain(|x| *x != gid));
                }
                _ => {}
            }
        }
        self.selected = Some(id.clone());
        self.pending_reveal = Some((id, 10));
        self.pending_focus_list = true;
        toast("已定位 session".into(), crate::overlays::toast::REVEALED, cx);
        cx.notify();
    }

    /// render 里跑：把 ⌘L 的目标行滚到正中。行还没出现（侧栏刚展开、视口高还是 0）就下一帧再试
    fn apply_pending_reveal(&mut self, cx: &mut Context<Self>) {
        let Some((id, tries)) = self.pending_reveal.take() else { return };
        let ix = self.row_of(&id, cx).and_then(|r| self.tree.item_of_row(r));
        let vh = self.viewport_h();
        match ix {
            Some(ix) if vh > 0.0 => {
                let y = tree::center_scroll_top(&self.tops, ix, vh);
                self.set_scroll_top(y);
            }
            _ if tries > 0 => {
                self.pending_reveal = Some((id, tries - 1));
                cx.notify();
            }
            _ => {}
        }
    }

    // ---------- 悬停卡（D5）----------

    fn row_hovered(&mut self, item_ix: usize, session_id: String, hovered: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !hovered {
            // 离开行：200ms 后关（同一个定时器，顺带取消还没出来的卡片）
            self.schedule_hover_close(cx);
            return;
        }
        let mode = self.state.read(cx).prefs.sidebar.hover_mode.clone();
        let Some(delay) = hover::hover_delay_ms(&mode, window.modifiers().platform) else { return };
        self.hover_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(delay)).await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.show_hover(item_ix, session_id, window, cx);
            });
        }));
    }

    fn schedule_hover_close(&mut self, cx: &mut Context<Self>) {
        self.hover_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(hover::HOVER_CLOSE_MS)).await;
            let _ = this.update(cx, |this, cx| {
                this.hover = None;
                cx.notify();
            });
        }));
    }

    fn show_hover(&mut self, item_ix: usize, session_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.list.bounds_for_item(item_ix) else { return };
        // 行的 li 左右各缩进 4px：卡片贴 li 右边 +4 = 列表项右边
        let (x, y) = hover::card_origin(f32::from(b.right()) - 4.0, f32::from(b.top()), f32::from(window.viewport_size().height));
        self.hover = Some(HoverCard { session_id, at: Point::new(px(x), px(y)) });
        cx.notify();
    }

    /// Space：在选中行旁打开 / 关闭卡片（位置取行的矩形，不是鼠标）
    fn toggle_hover_for_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((row, m)) = self.selected_meta(cx) else { return };
        if self.hover.as_ref().is_some_and(|h| h.session_id == m.session_id) {
            self.hover = None;
            cx.notify();
            return;
        }
        let Some(ix) = self.tree.item_of_row(row) else { return };
        self.hover_task = None;
        self.show_hover(ix, m.session_id, window, cx);
    }

    // ---------- 滚动 ----------

    fn on_scrolled(&mut self, cx: &mut Context<Self>) {
        // 卡片打开后滚动就关掉（行的位置变了）
        if self.hover.take().is_some() {
            self.hover_task = None;
        }
        self.scroll_active = true;
        self.scroll_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(900)).await;
            let _ = this.update(cx, |this, cx| {
                this.scroll_active = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    // ---------- 弹出菜单 ----------

    fn open_popup(&mut self, p: Popup, window: &mut Window, cx: &mut Context<Self>) {
        self.popup = Some(p);
        self.hover = None;
        self.hover_task = None;
        self.menu_focus.focus(window);
        cx.notify();
    }

    fn close_popup(&mut self, cx: &mut Context<Self>) {
        if self.popup.take().is_some() {
            cx.notify();
        }
    }
}

impl SidebarView {
    /// 自检用的摘要：`行数 / 会话行数 / 选中 / 搜索词 / 组头`
    pub fn debug_summary(&self, cx: &App) -> String {
        let headers: Vec<String> = self
            .tree
            .items
            .iter()
            .filter_map(|i| match i {
                tree::Item::GroupHeader { label, count, collapsed, .. } => Some(format!("{label}{}{count}", if *collapsed { "-" } else { "+" })),
                tree::Item::ProjectHeader { name, count, collapsed, .. } => Some(format!("P:{name}{}{count}", if *collapsed { "-" } else { "+" })),
                _ => None,
            })
            .collect();
        let sel = self.selected.as_deref().and_then(|id| self.state.read(cx).session(id)).map(|m| m.short_id.clone());
        format!(
            "items={} rows={} selected={:?} query={:?} popup={} hover={} [{}]",
            self.tree.items.len(),
            self.tree.order.len(),
            sel,
            self.query,
            self.popup.is_some(),
            self.hover.is_some(),
            headers.join(" ")
        )
    }

    /// 自检：滚动（同滚轮），并走一遍滚动后的处理（关悬停卡、滚动条显形）
    pub fn debug_scroll_by(&mut self, dy: f32, cx: &mut Context<Self>) {
        self.list.scroll_by(px(dy));
        self.on_scrolled(cx);
    }

    pub fn debug_focus_list(&self, window: &mut Window) {
        self.list_focus.focus(window);
    }

    pub fn debug_rows(&self) -> usize {
        self.tree.order.len()
    }

    pub fn debug_selected(&self) -> Option<String> {
        self.selected.clone()
    }

    pub fn debug_list_focused(&self, window: &Window) -> bool {
        self.list_focus.is_focused(window)
    }

    pub fn debug_search_focused(&self, window: &Window, cx: &App) -> bool {
        self.search.read(cx).is_focused(window)
    }

    /// 自检：直接往搜索框填字（dispatch_keystroke 不走输入法那条插字的路）
    pub fn debug_set_query(&mut self, q: &str, cx: &mut Context<Self>) {
        self.search.update(cx, |s, cx| s.set_text(q, cx));
    }

    /// 自检：打开某条会话的右键菜单 / 悬停卡
    pub fn debug_open_menu(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_popup(Popup::SessionMenu { at: Point::new(px(100.), px(100.)), session_id: id.into() }, window, cx);
    }

    pub fn debug_open_options(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_popup(Popup::Options, window, cx);
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
