//! 主窗口：左侧会话侧栏 + 右侧终端区（多标签，最多左右两栏）（#221）。
//!
//! 会话数据直接调 Tauri 版后端的 `makit_lib::api::list_sessions` / `list_running_sessions`，
//! 分组 / 标题 / 相对时间走 `makit_native::sessions`（照搬 WebView 前端的规则）。

use std::collections::HashSet;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ClickEvent, Context, Entity, FocusHandle, Focusable, Hsla, IntoElement,
    MouseButton, SharedString, Subscription, Window, actions, div, prelude::*, px, rgb,
    uniform_list,
};
use makit_lib::SessionMeta;
use makit_native::sessions::{
    Group, Row, RunState, project_groups, project_name, relative_time, run_state, session_title,
    status_groups,
};

use crate::perf;
use crate::terminal::{SpawnSpec, TerminalEvent, TerminalView};

actions!(makit, [NewTab, CloseTab, SplitRight, NextTab, PrevTab, FocusOtherPane, Quit]);

const SIDEBAR_W: f32 = 280.0;
const ROW_H: f32 = 44.0;

fn c(v: u32) -> Hsla {
    rgb(v).into()
}

struct Tab {
    view: Entity<TerminalView>,
    session_id: Option<String>,
    label: String,
    _sub: Subscription,
}

#[derive(Default)]
struct Pane {
    tabs: Vec<Tab>,
    active: usize,
}

#[derive(Clone)]
enum Item {
    Header { id: String, label: String, count: usize, collapsed: bool },
    Session(usize),
}

pub struct Workspace {
    focus: FocusHandle,
    metas: Vec<SessionMeta>,
    rows: Vec<Row>,
    loaded: bool,
    by_project: bool,
    collapsed: HashSet<String>,
    items: Vec<Item>,
    panes: Vec<Pane>,
    active_pane: usize,
    first_frame_marked: bool,
    startup_reported: bool,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let this = Self {
            focus: cx.focus_handle(),
            metas: Vec::new(),
            rows: Vec::new(),
            loaded: false,
            by_project: false,
            collapsed: HashSet::new(),
            items: Vec::new(),
            panes: vec![Pane::default()],
            active_pane: 0,
            first_frame_marked: false,
            startup_reported: false,
        };
        this.focus.focus(window);
        cx.on_app_quit(|ws, cx| {
            for pane in &ws.panes {
                for t in &pane.tabs {
                    t.view.update(cx, |v, _| v.shutdown());
                }
            }
            async {}
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let t = Instant::now();
            let list = cx
                .background_executor()
                .spawn(async { makit_lib::api::list_sessions(None).unwrap_or_default() })
                .await;
            perf::mark("会话列表返回");
            eprintln!("list_sessions: {} 条，{:?}", list.len(), t.elapsed());
            let _ = this.update(cx, |ws, cx| {
                ws.set_sessions(list);
                cx.notify();
            });
            // 运行状态每 2 秒轮询一次（只读 ~/.claude/sessions/*.json，很轻）
            loop {
                cx.background_executor().timer(Duration::from_secs(2)).await;
                let running = cx.background_executor().spawn(async { makit_lib::api::list_running_sessions() }).await;
                if this.update(cx, |ws, cx| ws.apply_running(running, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // 自动化测量用：启动后自动开一个 shell 标签并执行这条命令
        if let Ok(cmd) = std::env::var("MAKIT_NATIVE_AUTORUN") {
            let clicked = Instant::now();
            this_open_later(cmd, clicked, window, cx);
        }
        this
    }

    fn set_sessions(&mut self, list: Vec<SessionMeta>) {
        self.rows = list.iter().map(to_row).collect();
        self.metas = list;
        self.loaded = true;
        self.rebuild_items();
    }

    fn apply_running(&mut self, running: Vec<makit_lib::RunningMeta>, cx: &mut Context<Self>) {
        let mut changed = false;
        for (row, meta) in self.rows.iter_mut().zip(self.metas.iter_mut()) {
            let r = running.iter().find(|r| r.session_id == row.session_id);
            let (is_running, status) = match r {
                Some(r) => (true, r.status.clone()),
                None => (false, String::new()),
            };
            if row.running != is_running || row.status != status {
                row.running = is_running;
                row.status = status.clone();
                meta.running = is_running;
                meta.status = status;
                changed = true;
            }
        }
        if changed {
            self.rebuild_items();
            cx.notify();
        }
    }

    fn opened_ids(&self) -> Vec<String> {
        self.panes
            .iter()
            .flat_map(|p| p.tabs.iter().filter_map(|t| t.session_id.clone()))
            .collect()
    }

    fn rebuild_items(&mut self) {
        let groups: Vec<Group> = if self.by_project {
            project_groups(&self.rows)
        } else {
            status_groups(&self.rows, &self.opened_ids())
        };
        let mut items = Vec::new();
        for g in groups {
            let collapsed = self.collapsed.contains(&g.id);
            items.push(Item::Header { id: g.id.clone(), label: g.label.clone(), count: g.rows.len(), collapsed });
            if !collapsed {
                items.extend(g.rows.into_iter().map(Item::Session));
            }
        }
        self.items = items;
    }

    fn toggle_group(&mut self, id: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&id) {
            self.collapsed.insert(id);
        }
        self.rebuild_items();
        cx.notify();
    }

    fn set_by_project(&mut self, v: bool, cx: &mut Context<Self>) {
        self.by_project = v;
        self.rebuild_items();
        cx.notify();
    }

    fn open_session(&mut self, idx: usize, window: &mut Window, cx: &mut Context<Self>) {
        let clicked = Instant::now();
        let Some(meta) = self.metas.get(idx).cloned() else { return };
        // 已经开着就切过去
        for (pi, pane) in self.panes.iter_mut().enumerate() {
            if let Some(ti) = pane.tabs.iter().position(|t| t.session_id.as_deref() == Some(&meta.session_id)) {
                pane.active = ti;
                self.active_pane = pi;
                let v = pane.tabs[ti].view.clone();
                v.read(cx).focus_handle(cx).focus(window);
                cx.notify();
                return;
            }
        }
        let cwd = if meta.cwd.is_empty() { None } else { Some(meta.cwd.clone()) };
        let spec = SpawnSpec {
            cwd,
            init_command: Some(format!("clear && claude -r {}", meta.session_id)),
            session_id: Some(meta.session_id.clone()),
            kind: "resume",
            clicked_at: clicked,
        };
        let label = self.rows[idx].title.clone();
        self.add_tab(spec, Some(meta.session_id.clone()), label, window, cx);
    }

    pub fn new_shell(&mut self, init: Option<String>, clicked: Instant, window: &mut Window, cx: &mut Context<Self>) {
        let spec = SpawnSpec { cwd: None, init_command: init, session_id: None, kind: "shell", clicked_at: clicked };
        self.add_tab(spec, None, "终端".to_string(), window, cx);
    }

    fn add_tab(&mut self, spec: SpawnSpec, session_id: Option<String>, label: String, window: &mut Window, cx: &mut Context<Self>) {
        let view = cx.new(|cx| TerminalView::new(spec, window, cx));
        let sub = cx.subscribe_in(&view, window, |ws: &mut Workspace, view, ev: &TerminalEvent, window, cx| match ev {
            TerminalEvent::Exited => ws.close_view(view, window, cx),
            TerminalEvent::TitleChanged => cx.notify(),
        });
        let pane = &mut self.panes[self.active_pane];
        pane.tabs.push(Tab { view, session_id, label, _sub: sub });
        pane.active = pane.tabs.len() - 1;
        self.rebuild_items();
        cx.notify();
    }

    fn close_view(&mut self, view: &Entity<TerminalView>, window: &mut Window, cx: &mut Context<Self>) {
        for pi in 0..self.panes.len() {
            if let Some(ti) = self.panes[pi].tabs.iter().position(|t| &t.view == view) {
                self.close_tab_at(pi, ti, window, cx);
                return;
            }
        }
    }

    fn close_tab_at(&mut self, pi: usize, ti: usize, window: &mut Window, cx: &mut Context<Self>) {
        let pane = &mut self.panes[pi];
        let tab = pane.tabs.remove(ti);
        tab.view.update(cx, |v, _| v.shutdown());
        if pane.active >= pane.tabs.len() {
            pane.active = pane.tabs.len().saturating_sub(1);
        }
        if pane.tabs.is_empty() && self.panes.len() > 1 {
            self.panes.remove(pi);
            self.active_pane = 0;
        }
        self.active_pane = self.active_pane.min(self.panes.len() - 1);
        self.focus_active(window, cx);
        self.rebuild_items();
        cx.notify();
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        let pane = &self.panes[self.active_pane];
        if let Some(t) = pane.tabs.get(pane.active) {
            t.view.read(cx).focus_handle(cx).focus(window);
        } else {
            self.focus.focus(window);
        }
    }

    fn activate(&mut self, pi: usize, ti: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.active_pane = pi;
        self.panes[pi].active = ti;
        self.focus_active(window, cx);
        cx.notify();
    }

    // ── 快捷键 ──
    fn on_new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        self.new_shell(None, Instant::now(), window, cx);
    }

    fn on_close_tab(&mut self, _: &CloseTab, window: &mut Window, cx: &mut Context<Self>) {
        let pi = self.active_pane;
        if !self.panes[pi].tabs.is_empty() {
            let ti = self.panes[pi].active;
            self.close_tab_at(pi, ti, window, cx);
        }
    }

    fn on_split(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.len() == 1 {
            self.panes.push(Pane::default());
        }
        self.active_pane = 1;
        self.new_shell(None, Instant::now(), window, cx);
    }

    fn on_next_tab(&mut self, _: &NextTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(1, window, cx);
    }

    fn on_prev_tab(&mut self, _: &PrevTab, window: &mut Window, cx: &mut Context<Self>) {
        self.cycle(-1, window, cx);
    }

    fn cycle(&mut self, d: i32, window: &mut Window, cx: &mut Context<Self>) {
        let pane = &mut self.panes[self.active_pane];
        let n = pane.tabs.len() as i32;
        if n > 0 {
            pane.active = ((pane.active as i32 + d).rem_euclid(n)) as usize;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    fn on_focus_other(&mut self, _: &FocusOtherPane, window: &mut Window, cx: &mut Context<Self>) {
        if self.panes.len() > 1 {
            self.active_pane = 1 - self.active_pane;
            self.focus_active(window, cx);
            cx.notify();
        }
    }

    // ── 渲染 ──

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let seg = |label: &'static str, on: bool, id: &'static str| {
            div()
                .id(id)
                .px_2()
                .py(px(3.0))
                .rounded(px(4.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(on, |d| d.bg(c(0x37373d)).text_color(c(0xffffff)))
                .when(!on, |d| d.text_color(c(0x9d9d9d)))
                .child(label)
        };
        let count = self.rows.len();
        div()
            .flex()
            .flex_col()
            .w(px(SIDEBAR_W))
            .flex_none()
            .h_full()
            .bg(c(0x252526))
            .border_r_1()
            .border_color(c(0x3c3c3c))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h(px(36.0))
                    .border_b_1()
                    .border_color(c(0x3c3c3c))
                    .child(seg("按状态", !self.by_project, "by-status").on_click(cx.listener(|ws, _: &ClickEvent, _, cx| ws.set_by_project(false, cx))))
                    .child(seg("按项目", self.by_project, "by-project").on_click(cx.listener(|ws, _: &ClickEvent, _, cx| ws.set_by_project(true, cx))))
                    .child(div().flex_1())
                    .child(div().text_size(px(11.0)).text_color(c(0x808080)).child(format!("{count} 个会话")))
                    .child(
                        div()
                            .id("new-shell")
                            .ml_1()
                            .px_2()
                            .py(px(2.0))
                            .rounded(px(4.0))
                            .text_size(px(14.0))
                            .text_color(c(0xcccccc))
                            .cursor_pointer()
                            .hover(|d| d.bg(c(0x37373d)))
                            .child("+")
                            .on_click(cx.listener(|ws, _: &ClickEvent, window, cx| ws.new_shell(None, Instant::now(), window, cx))),
                    ),
            )
            .child(if !self.loaded {
                div().p_3().text_size(px(12.0)).text_color(c(0x808080)).child("正在读取会话…").into_any_element()
            } else {
                uniform_list(
                    "sessions",
                    self.items.len(),
                    cx.processor(|ws, range: std::ops::Range<usize>, _window, cx| {
                        let now = chrono_now();
                        range.map(|i| ws.render_item(i, now, cx)).collect::<Vec<_>>()
                    }),
                )
                .flex_1()
                .into_any_element()
            })
    }

    fn render_item(&self, i: usize, now: i64, cx: &mut Context<Self>) -> AnyElement {
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
                .text_color(c(0x9d9d9d))
                .cursor_pointer()
                .child(if collapsed { "▸" } else { "▾" })
                .child(label)
                .child(div().text_color(c(0x6a6a6a)).child(count.to_string()))
                .on_click(cx.listener(move |ws, _: &ClickEvent, _, cx| ws.toggle_group(id.clone(), cx)))
                .into_any_element(),
            Item::Session(idx) => {
                let r = &self.rows[idx];
                let st = run_state(r.running, &r.status);
                let (dot, color) = match st {
                    RunState::Waiting => ("●", 0xe2c08d),
                    RunState::Busy => ("●", 0x6cb6ff),
                    RunState::Idle => ("●", 0x73c991),
                    RunState::Stopped => ("○", 0x6a6a6a),
                };
                let opened = self.panes.iter().any(|p| {
                    p.tabs.get(p.active).and_then(|t| t.session_id.as_deref()) == Some(r.session_id.as_str())
                });
                div()
                    .id(SharedString::from(format!("s-{}", r.session_id)))
                    .h(px(ROW_H))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .cursor_pointer()
                    .when(opened, |d| d.bg(c(0x37373d)))
                    .hover(|d| d.bg(c(0x2a2d2e)))
                    .child(div().w(px(10.0)).text_size(px(10.0)).text_color(c(color)).child(dot))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.0))
                                    .text_color(c(0xcccccc))
                                    .truncate()
                                    .child(r.title.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .text_size(px(11.0))
                                    .text_color(c(0x808080))
                                    .child(div().flex_1().truncate().child(r.project.clone()))
                                    .child(relative_time(r.mtime, now)),
                            ),
                    )
                    .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, window, cx| ws.open_session(idx, window, cx)))
                    .into_any_element()
            }
        }
    }

    fn render_pane(&self, pi: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let pane = &self.panes[pi];
        let is_active_pane = pi == self.active_pane;
        let tabs = pane.tabs.iter().enumerate().map(|(ti, t)| {
            let active = ti == pane.active;
            let title = if t.session_id.is_none() && !t.view.read(cx).title.is_empty() {
                t.view.read(cx).title.clone()
            } else {
                t.label.clone()
            };
            div()
                .id(SharedString::from(format!("tab-{pi}-{ti}")))
                .flex()
                .items_center()
                .gap_1()
                .px_3()
                .h_full()
                .max_w(px(200.0))
                .border_r_1()
                .border_color(c(0x252526))
                .text_size(px(12.0))
                .cursor_pointer()
                .when(active, |d| d.bg(c(0x1e1e1e)).text_color(c(0xffffff)))
                .when(!active, |d| d.bg(c(0x2d2d2d)).text_color(c(0x969696)))
                .child(div().truncate().child(title))
                .child(
                    div()
                        .id(SharedString::from(format!("close-{pi}-{ti}")))
                        .px_1()
                        .rounded(px(3.0))
                        .text_color(c(0x808080))
                        .hover(|d| d.bg(c(0x454545)))
                        .child("×")
                        .on_click(cx.listener(move |ws, _: &ClickEvent, window, cx| {
                            cx.stop_propagation();
                            ws.close_tab_at(pi, ti, window, cx)
                        })),
                )
                .on_click(cx.listener(move |ws, _: &ClickEvent, window, cx| ws.activate(pi, ti, window, cx)))
        });
        let body: AnyElement = match pane.tabs.get(pane.active) {
            Some(t) => div().flex_1().min_h_0().child(t.view.clone()).into_any_element(),
            None => div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(c(0x6a6a6a))
                .text_size(px(13.0))
                .child("点左侧会话恢复，或 ⌘T 新建终端，⌘D 右侧分屏")
                .into_any_element(),
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .when(pi > 0, |d| d.border_l_1().border_color(c(0x3c3c3c)))
            .child(
                div()
                    .flex()
                    .h(px(32.0))
                    .flex_none()
                    .bg(c(0x252526))
                    .when(is_active_pane && self.panes.len() > 1, |d| d.border_t_2().border_color(c(0x007acc)))
                    .children(tabs),
            )
            .child(body)
            .on_mouse_down(MouseButton::Left, cx.listener(move |ws, _, _, cx| {
                if ws.active_pane != pi {
                    ws.active_pane = pi;
                    cx.notify();
                }
            }))
    }
}

fn this_open_later(cmd: String, clicked: Instant, window: &mut Window, cx: &mut Context<Workspace>) {
    cx.spawn_in(window, async move |this, cx| {
        let _ = this.update_in(cx, |ws, window, cx| ws.new_shell(Some(cmd), clicked, window, cx));
    })
    .detach();
}

fn chrono_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn to_row(m: &SessionMeta) -> Row {
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

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_frame_marked {
            self.first_frame_marked = true;
            window.on_next_frame(|_, _| perf::mark("首帧画出"));
        }
        if self.loaded && !self.startup_reported {
            self.startup_reported = true;
            let n = self.rows.len();
            window.on_next_frame(move |_, _| {
                perf::mark("侧栏有数据");
                perf::report_startup(n);
            });
        }
        let panes: Vec<_> = (0..self.panes.len()).map(|pi| self.render_pane(pi, cx).into_any_element()).collect();
        div()
            .id("workspace")
            .key_context("Workspace")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::on_new_tab))
            .on_action(cx.listener(Self::on_close_tab))
            .on_action(cx.listener(Self::on_split))
            .on_action(cx.listener(Self::on_next_tab))
            .on_action(cx.listener(Self::on_prev_tab))
            .on_action(cx.listener(Self::on_focus_other))
            .on_action(|_: &Quit, _, cx| cx.quit())
            .flex()
            .size_full()
            .bg(c(0x1e1e1e))
            .text_color(c(0xcccccc))
            .font_family(".SystemUIFont")
            .child(self.render_sidebar(cx))
            .child(div().flex().flex_1().min_w_0().h_full().children(panes))
    }
}
