//! 侧栏视图（F0 版：原型的两种分组 + 折叠 + 点击恢复，改成读 AppState）。
//!
//! 归 B 侧栏包的（界面清单 D）：搜索 / 过滤 / 排序、hover 卡、右键菜单、键盘导航、⌘L 定位、
//! 拖会话、日期分段、项目组分页、logo……都在这个目录里继续加；纯逻辑放 `groups.rs`（或新文件）并带测试。
//!
//! 数据：`AppState.sessions`（已按 mtime 降序）+ `AppState.prefs.sidebar`（视图 / 折叠集合，改了会存盘）。
//! 打开会话：`state.update(cx, |s, cx| { s.workspace.open_session(..); s.workspace_changed(cx) })`，
//! 工作区视图会自己起终端、给焦点。

pub mod groups;

use gpui::{div, prelude::*, px, uniform_list, AnyElement, App, ClickEvent, Context, Entity, MouseButton, SharedString, Subscription, Window};

use crate::state::AppState;
use crate::theme::ActiveTheme;
use crate::workspace::model::opened_order;
use groups::{project_groups, project_name, relative_time, run_state, session_title, status_groups, Group, Row, RunState};

const ROW_H: f32 = 44.0;

#[derive(Clone)]
enum Item {
    Header { id: String, label: String, count: usize, collapsed: bool },
    /// `AppState.sessions` 的下标
    Session(usize),
}

pub struct SidebarView {
    state: Entity<AppState>,
    rows: Vec<Row>,
    items: Vec<Item>,
    _observe: Subscription,
}

impl SidebarView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |this, _, cx| {
            this.rebuild(cx);
            cx.notify();
        });
        let mut this = Self { state, rows: Vec::new(), items: Vec::new(), _observe: observe };
        this.rebuild(cx);
        this
    }

    fn by_project(&self, cx: &App) -> bool {
        self.state.read(cx).prefs.sidebar.view == "project"
    }

    fn rebuild(&mut self, cx: &App) {
        let s = self.state.read(cx);
        self.rows = s.sessions.iter().map(to_row).collect();
        let groups: Vec<Group> = if s.prefs.sidebar.view == "project" {
            project_groups(&self.rows)
        } else {
            status_groups(&self.rows, &opened_order(&s.workspace.state))
        };
        let collapsed = &s.prefs.sidebar.group_collapsed;
        let mut items = Vec::new();
        for g in groups {
            let is_collapsed = collapsed.contains(&g.id);
            items.push(Item::Header { id: g.id.clone(), label: g.label.clone(), count: g.rows.len(), collapsed: is_collapsed });
            if !is_collapsed {
                items.extend(g.rows.into_iter().map(Item::Session));
            }
        }
        self.items = items;
    }

    fn toggle_group(&mut self, id: String, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.update_prefs(cx, |p| {
                let c = &mut p.sidebar.group_collapsed;
                if let Some(i) = c.iter().position(|x| *x == id) {
                    c.remove(i);
                } else {
                    c.push(id);
                }
            })
        });
    }

    fn set_view(&mut self, project: bool, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.view = if project { "project".into() } else { "status".into() }));
    }

    pub fn open_session(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(m) = self.state.read(cx).sessions.get(idx).cloned() else { return };
        let title = self.rows.get(idx).map(|r| r.title.clone());
        self.state.update(cx, |s, cx| {
            let tool = (m.tool == "codex").then_some("codex");
            s.workspace.open_session(&m.session_id, &m.short_id, &m.cwd, title.as_deref(), tool);
            s.workspace_changed(cx);
        });
    }

    fn new_shell(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.workspace.open_shell("~");
            s.workspace_changed(cx);
        });
    }

    fn render_item(&self, i: usize, now: i64, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        match self.items[i].clone() {
            Item::Header { id, label, count, collapsed } => div()
                .id(SharedString::from(format!("h-{id}")))
                .h(px(ROW_H))
                .flex()
                .items_end()
                .pb(px(6.0))
                .px_3()
                .gap_1()
                .text_size(px(11.0))
                .text_color(theme.fg_muted)
                .cursor_pointer()
                .child(if collapsed { "▸" } else { "▾" })
                .child(label)
                .child(div().text_color(theme.fg_subtle).child(count.to_string()))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle_group(id.clone(), cx)))
                .into_any_element(),
            Item::Session(idx) => {
                let r = &self.rows[idx];
                let (dot, color) = match run_state(r.running, &r.status) {
                    RunState::Waiting => ("●", theme.warning),
                    RunState::Busy => ("●", theme.accent),
                    RunState::Idle => ("●", theme.success),
                    RunState::Stopped => ("○", theme.fg_subtle),
                };
                let opened = self.state.read(cx).workspace.active_tab().and_then(|t| t.session_id.as_deref()) == Some(r.session_id.as_str());
                div()
                    .id(SharedString::from(format!("s-{}", r.session_id)))
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .cursor_pointer()
                    .when(opened, |d| d.bg(theme.bg_active))
                    .hover(|d| d.bg(theme.bg_hover))
                    .child(div().w(px(10.0)).text_size(px(10.0)).text_color(color).child(dot))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_size(px(13.0)).text_color(theme.fg).truncate().child(r.title.clone()))
                            .child(
                                div()
                                    .flex()
                                    .text_size(px(11.0))
                                    .text_color(theme.fg_muted)
                                    .child(div().flex_1().truncate().child(r.project.clone()))
                                    .child(relative_time(r.mtime, now)),
                            ),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| this.open_session(idx, cx)))
                    .into_any_element()
            }
        }
    }
}

impl Render for SidebarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let by_project = self.by_project(cx);
        let (loaded, count, width) = {
            let s = self.state.read(cx);
            (s.loaded, s.sessions.len(), s.prefs.sidebar.width)
        };
        let seg = |label: &'static str, on: bool, id: &'static str| {
            div()
                .id(id)
                .px_2()
                .py(px(3.0))
                .rounded(px(4.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(on, |d| d.bg(theme.bg_active).text_color(theme.fg))
                .when(!on, |d| d.text_color(theme.fg_muted))
                .child(label)
        };
        div()
            .id("sidebar")
            .key_context("Sidebar")
            .flex()
            .flex_col()
            .w(px(width))
            .flex_none()
            .h_full()
            .bg(theme.bg_soft)
            .border_r_1()
            .border_color(theme.border_strong)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h(px(36.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(seg("按状态", !by_project, "by-status").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_view(false, cx))))
                    .child(seg("按项目", by_project, "by-project").on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.set_view(true, cx))))
                    .child(div().flex_1())
                    .child(div().text_size(px(11.0)).text_color(theme.fg_subtle).child(format!("{count} 个会话")))
                    .child(
                        div()
                            .id("new-shell")
                            .ml_1()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .text_size(px(14.0))
                            .text_color(theme.fg)
                            .cursor_pointer()
                            .hover(|d| d.bg(theme.bg_hover))
                            .child("+")
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.new_shell(cx))),
                    ),
            )
            .child(if !loaded {
                div().p_3().text_size(px(12.0)).text_color(theme.fg_subtle).child("正在读取会话…").into_any_element()
            } else {
                uniform_list(
                    "sessions",
                    self.items.len(),
                    cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                        let now = chrono_now();
                        range.map(|i| this.render_item(i, now, cx)).collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .into_any_element()
            })
    }
}

pub fn chrono_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn to_row(m: &makit_core::SessionMeta) -> Row {
    Row {
        session_id: m.session_id.clone(),
        title: session_title(&m.display_name, &m.first_user_msg, &m.short_id),
        project: project_name(&m.git_root, &m.cwd),
        cwd: m.cwd.clone(),
        mtime: m.mtime,
        running: m.running,
        status: m.status.clone(),
    }
}
