//! 工作区视图：把 `AppState.workspace`（递归分割树，纯逻辑在 model.rs）画出来，并持有每个标签的终端实体。
//!
//! F0 只做到「能用」：递归画分屏（按 ratio 等比，无分割线拖动）、标签条（点击切换 / × 关闭）、
//! 最大化、终端懒创建（标签第一次被激活才起 PTY）、终端退出自动关标签。
//! 归 C 工作区包的：分割线拖动（#203 行列分离）、标签拖放、落点浮层、pane 闪牌、欢迎卡、右键菜单、
//! 标签图标 / 状态点、滚动到可见……都在这个目录里继续加。
//!
//! 约定：
//! - 改树一律 `state.update(cx, |s, cx| { s.workspace.xxx(..); s.workspace_changed(cx) })`，
//!   这个视图 observe 了 AppState，会自己重画、自己把焦点给当前标签的终端。
//! - 关标签返回的 tab id 必须交给 `shutdown_tabs` 杀终端（模型不碰终端）。

pub mod drop;
pub mod flash;
pub mod labels;
pub mod model;
pub mod splitter;
pub mod welcome;

use std::collections::HashMap;
use std::time::Instant;

use gpui::{
    div, prelude::*, px, relative, AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, MouseButton,
    SharedString, Subscription, Window,
};

use crate::actions::workspace as act;
use crate::state::AppState;
use crate::terminal::{SpawnSpec, TerminalEvent, TerminalView};
use crate::theme::ActiveTheme;
use model::{find_nearest_container, layout_tree, ContainerNode, Dir, Direction, LayoutNode, Rect, TabKind};

const TAB_H: f32 = 32.0;

struct Term {
    view: Entity<TerminalView>,
    _sub: Subscription,
}

pub struct WorkspaceView {
    state: Entity<AppState>,
    terminals: HashMap<String, Term>,
    focus: FocusHandle,
    /// 上次把焦点给了哪个标签：当前标签变了才重新给（不和侧栏 / 浮层抢焦点）
    focused_tab: Option<String>,
    _observe: Subscription,
}

impl WorkspaceView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        cx.on_app_quit(|this, cx| {
            this.shutdown_all(cx);
            async {}
        })
        .detach();
        Self { state, terminals: HashMap::new(), focus: cx.focus_handle(), focused_tab: None, _observe: observe }
    }

    fn update_ws(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut model::Workspace) -> Vec<String>) {
        let closed = self.state.update(cx, |s, cx| {
            let closed = f(&mut s.workspace);
            s.workspace_changed(cx);
            closed
        });
        self.shutdown_tabs(&closed, cx);
    }

    /// 杀掉这些标签的终端（关标签 / 关 pane 之后调）
    pub fn shutdown_tabs(&mut self, ids: &[String], cx: &mut Context<Self>) {
        for id in ids {
            if let Some(t) = self.terminals.remove(id) {
                t.view.update(cx, |v, _| v.shutdown());
            }
        }
    }

    fn shutdown_all(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.terminals.keys().cloned().collect();
        self.shutdown_tabs(&ids, cx);
    }

    fn active_container_id(&self, cx: &App) -> String {
        self.state.read(cx).workspace.state.active_container_id.clone()
    }

    // ---- 动作（根视图把全局 action 转发到这里）----

    pub fn new_tab(&mut self, cx: &mut Context<Self>) {
        let cid = self.active_container_id(cx);
        self.update_ws(cx, |w| {
            w.new_shell_in(&cid);
            vec![]
        });
    }

    pub fn close_active_tab(&mut self, cx: &mut Context<Self>) {
        let Some((cid, tid)) = self.state.read(cx).workspace.active_container().map(|c| (c.id.clone(), c.active_tab_id.clone())) else { return };
        if tid.is_empty() {
            return;
        }
        self.close_tab(&cid, &tid, cx);
    }

    pub fn close_tab(&mut self, cid: &str, tid: &str, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| w.close_tab(cid, tid));
    }

    pub fn split(&mut self, dir: Dir, cx: &mut Context<Self>) {
        let cid = self.active_container_id(cx);
        self.update_ws(cx, |w| {
            w.split(&cid, dir);
            vec![]
        });
    }

    pub fn cycle_tab(&mut self, delta: i32, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            w.cycle_tab(delta);
            vec![]
        });
    }

    pub fn activate_nth_tab(&mut self, n: usize, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            w.activate_nth_tab(n);
            vec![]
        });
    }

    pub fn focus_nth_pane(&mut self, n: usize, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            if let Some(id) = w.nth_container(n) {
                w.set_active(&id);
            }
            vec![]
        });
    }

    pub fn focus_pane_dir(&mut self, dir: Direction, cx: &mut Context<Self>) {
        self.update_ws(cx, |w| {
            // 方向只看相对位置，按比例布局在单位矩形里算就够了
            let l = layout_tree(&w.state.root, Rect { x: 0.0, y: 0.0, width: 1000.0, height: 1000.0 });
            if let Some(id) = find_nearest_container(&l, &w.state.active_container_id, dir) {
                w.set_active(&id);
            }
            vec![]
        });
    }

    pub fn toggle_maximize(&mut self, cx: &mut Context<Self>) {
        let cid = self.active_container_id(cx);
        self.update_ws(cx, |w| {
            w.toggle_maximize(&cid);
            vec![]
        });
    }

    // ---- 终端 ----

    fn ensure_terminal(&mut self, c: &ContainerNode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = c.tabs.iter().find(|t| t.id == c.active_tab_id) else { return };
        if self.terminals.contains_key(&tab.id) {
            return;
        }
        let spec = SpawnSpec {
            pty_id: Some(tab.id.clone()),
            cwd: Some(tab.cwd.clone()).filter(|c| !c.is_empty()),
            init_command: tab.init_command.clone(),
            session_id: tab.session_id.clone(),
            kind: match tab.kind {
                TabKind::Resume => "resume",
                TabKind::New => "new",
                TabKind::Shell => "shell",
            },
            clicked_at: Instant::now(),
        };
        let tab_id = tab.id.clone();
        let view = cx.new(|cx| TerminalView::new(spec, window, cx));
        let sub = cx.subscribe(&view, move |this: &mut Self, _, ev: &TerminalEvent, cx| match ev {
            TerminalEvent::Exited => {
                let loc = this.state.read(cx).workspace.locate_tab(&tab_id).map(|(c, _)| c.id.clone());
                if let Some(cid) = loc {
                    this.close_tab(&cid, &tab_id, cx);
                }
            }
            TerminalEvent::TitleChanged => cx.notify(),
        });
        self.terminals.insert(tab.id.clone(), Term { view, _sub: sub });
    }

    fn tab_title(&self, tab: &model::PaneTab, cx: &App) -> String {
        if let Some(sid) = &tab.session_id {
            if let Some(m) = self.state.read(cx).session(sid) {
                return crate::sidebar::groups::session_title(&m.display_name, &m.first_user_msg, &m.short_id);
            }
        }
        if tab.kind == TabKind::Shell {
            if let Some(t) = self.terminals.get(&tab.id) {
                let title = t.view.read(cx).title.clone();
                if !title.is_empty() {
                    return title;
                }
            }
        }
        if tab.label.is_empty() { "终端".into() } else { tab.label.clone() }
    }

    // ---- 渲染 ----

    fn render_node(&mut self, node: &LayoutNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match node {
            LayoutNode::Container(c) => self.render_container(c, window, cx),
            LayoutNode::Split(s) => {
                let a = self.render_node(&s.a, window, cx);
                let b = self.render_node(&s.b, window, cx);
                let border = cx.theme().border;
                let (ra, rb) = (s.ratio as f32, 1.0 - s.ratio as f32);
                match s.dir {
                    Dir::V => div()
                        .flex()
                        .flex_row()
                        .size_full()
                        .child(div().h_full().w(relative(ra)).min_w_0().child(a))
                        .child(div().h_full().w(relative(rb)).min_w_0().border_l_1().border_color(border).child(b))
                        .into_any_element(),
                    Dir::H => div()
                        .flex()
                        .flex_col()
                        .size_full()
                        .child(div().w_full().h(relative(ra)).min_h_0().child(a))
                        .child(div().w_full().h(relative(rb)).min_h_0().border_t_1().border_color(border).child(b))
                        .into_any_element(),
                }
            }
        }
    }

    fn render_container(&mut self, c: &ContainerNode, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.ensure_terminal(c, window, cx);
        let theme = cx.theme().clone();
        let ws = &self.state.read(cx).workspace.state;
        let is_active = ws.active_container_id == c.id;
        let multi = matches!(ws.root, LayoutNode::Split(_)) && ws.maximized_container_id.is_none();
        let tabs: Vec<AnyElement> = c
            .tabs
            .iter()
            .map(|t| {
                let active = t.id == c.active_tab_id;
                let (cid, tid) = (c.id.clone(), t.id.clone());
                let (cid2, tid2) = (c.id.clone(), t.id.clone());
                div()
                    .id(SharedString::from(format!("tab-{}", t.id)))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_3()
                    .h_full()
                    .max_w(px(220.0))
                    .border_r_1()
                    .border_color(theme.border)
                    .text_size(px(12.0))
                    .cursor_pointer()
                    .when(active, |d| d.bg(theme.bg).text_color(theme.fg))
                    .when(!active, |d| d.bg(theme.bg_soft).text_color(theme.fg_muted))
                    .child(div().truncate().child(self.tab_title(t, cx)))
                    .child(
                        div()
                            .id(SharedString::from(format!("close-{}", t.id)))
                            .px_1()
                            .rounded(px(3.0))
                            .text_color(theme.fg_subtle)
                            .hover(|d| d.bg(theme.bg_hover))
                            .child("×")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(&cid, &tid, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.update_ws(cx, |w| {
                            w.tab_click(&cid2, &tid2);
                            vec![]
                        });
                    }))
                    .into_any_element()
            })
            .collect();
        let body: AnyElement = match self.terminals.get(&c.active_tab_id) {
            Some(t) => div().flex_1().min_h_0().child(t.view.clone()).into_any_element(),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.fg_subtle)
                .text_size(px(13.0))
                .child("点左侧会话恢复，或 ⌘T 新建终端，⌘D 左右分屏，⌘⇧D 上下分屏")
                .into_any_element(),
        };
        let cid = c.id.clone();
        div()
            .id(SharedString::from(format!("pane-{}", c.id)))
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex()
                    .h(px(TAB_H))
                    .flex_none()
                    .bg(theme.bg_soft)
                    .when(is_active && multi, |d| d.border_t_2().border_color(theme.accent))
                    .children(tabs),
            )
            .child(body)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if this.active_container_id(cx) != cid {
                        let cid = cid.clone();
                        this.update_ws(cx, |w| {
                            w.set_active(&cid);
                            vec![]
                        });
                    }
                }),
            )
            .into_any_element()
    }

    /// 当前标签变了就把焦点给它的终端
    fn sync_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.state.read(cx).workspace.active_tab().map(|t| t.id.clone());
        if active == self.focused_tab {
            return;
        }
        match active.as_ref().and_then(|id| self.terminals.get(id)) {
            Some(t) => t.view.read(cx).focus_handle(cx).focus(window),
            None => self.focus.focus(window),
        }
        self.focused_tab = active;
    }
}

impl Focusable for WorkspaceView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ws = self.state.read(cx).workspace.state.clone();
        let content = match ws.maximized_container_id.as_deref().and_then(|id| model::find_container(&ws.root, id)) {
            Some(c) => self.render_container(c, window, cx),
            None => self.render_node(&ws.root, window, cx),
        };
        // 被关掉 / 已不在树上的标签的终端（比如别处改了树）一并收掉
        let alive = model::collect_all_tab_ids(&ws.root);
        let dead: Vec<String> = self.terminals.keys().filter(|k| !alive.contains(k)).cloned().collect();
        self.shutdown_tabs(&dead, cx);
        self.sync_focus(window, cx);
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .size_full()
            .bg(cx.theme().bg)
            .child(content)
    }
}

/// 根视图把工作区 action 全部转发过来（action 在 actions::workspace 里声明，F0 实现了这些）
pub fn register_actions<V: 'static>(el: gpui::Stateful<gpui::Div>, ws: Entity<WorkspaceView>, cx: &mut Context<V>) -> gpui::Stateful<gpui::Div> {
    let _ = cx;
    macro_rules! fwd {
        ($el:expr, $action:ty, |$w:ident, $cx:ident| $body:expr) => {{
            let ws = ws.clone();
            $el.on_action(move |_: &$action, _window, app| ws.update(app, |$w, $cx| $body))
        }};
    }
    let el = fwd!(el, act::NewTab, |w, cx| w.new_tab(cx));
    let el = fwd!(el, act::CloseTab, |w, cx| w.close_active_tab(cx));
    let el = fwd!(el, act::SplitRight, |w, cx| w.split(Dir::V, cx));
    let el = fwd!(el, act::SplitDown, |w, cx| w.split(Dir::H, cx));
    let el = fwd!(el, act::NextTab, |w, cx| w.cycle_tab(1, cx));
    let el = fwd!(el, act::PrevTab, |w, cx| w.cycle_tab(-1, cx));
    let el = fwd!(el, act::ToggleMaximize, |w, cx| w.toggle_maximize(cx));
    let el = fwd!(el, act::ActivateTab1, |w, cx| w.activate_nth_tab(0, cx));
    let el = fwd!(el, act::ActivateTab2, |w, cx| w.activate_nth_tab(1, cx));
    let el = fwd!(el, act::ActivateTab3, |w, cx| w.activate_nth_tab(2, cx));
    let el = fwd!(el, act::ActivateTab4, |w, cx| w.activate_nth_tab(3, cx));
    let el = fwd!(el, act::ActivateTab5, |w, cx| w.activate_nth_tab(4, cx));
    let el = fwd!(el, act::ActivateTab6, |w, cx| w.activate_nth_tab(5, cx));
    let el = fwd!(el, act::ActivateTab7, |w, cx| w.activate_nth_tab(6, cx));
    let el = fwd!(el, act::ActivateTab8, |w, cx| w.activate_nth_tab(7, cx));
    let el = fwd!(el, act::ActivateTab9, |w, cx| w.activate_nth_tab(8, cx));
    let el = fwd!(el, act::FocusPane1, |w, cx| w.focus_nth_pane(0, cx));
    let el = fwd!(el, act::FocusPane2, |w, cx| w.focus_nth_pane(1, cx));
    let el = fwd!(el, act::FocusPane3, |w, cx| w.focus_nth_pane(2, cx));
    let el = fwd!(el, act::FocusPane4, |w, cx| w.focus_nth_pane(3, cx));
    let el = fwd!(el, act::FocusPane5, |w, cx| w.focus_nth_pane(4, cx));
    let el = fwd!(el, act::FocusPane6, |w, cx| w.focus_nth_pane(5, cx));
    let el = fwd!(el, act::FocusPane7, |w, cx| w.focus_nth_pane(6, cx));
    let el = fwd!(el, act::FocusPane8, |w, cx| w.focus_nth_pane(7, cx));
    let el = fwd!(el, act::FocusPane9, |w, cx| w.focus_nth_pane(8, cx));
    let el = fwd!(el, act::FocusPaneLeft, |w, cx| w.focus_pane_dir(Direction::Left, cx));
    let el = fwd!(el, act::FocusPaneRight, |w, cx| w.focus_pane_dir(Direction::Right, cx));
    let el = fwd!(el, act::FocusPaneUp, |w, cx| w.focus_pane_dir(Direction::Up, cx));
    fwd!(el, act::FocusPaneDown, |w, cx| w.focus_pane_dir(Direction::Down, cx))
}
