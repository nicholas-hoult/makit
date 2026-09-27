//! 启动流程 + 根视图。
//!
//! 启动：读持久化（首启从 WebKit localStorage 导入）→ 装 Theme 全局 → 建 AppState 全局 →
//! 绑整张快捷键表 → 开窗口，根视图 = 侧栏 + 工作区。
//!
//! 根视图是全局 action 的落脚点：焦点在哪都能收到（GPUI 沿焦点链冒泡到根）。
//! 工作区 action 转发给 `WorkspaceView`；还没实现的 action 挂占位（打一行日志，写明归哪个包）——
//! 各包实现时删掉对应占位，在自己的视图上 `.on_action`。

use std::time::Duration;

use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, Entity, SharedString, TitlebarOptions, Window, WindowBounds,
    WindowOptions,
};

use crate::actions::{self, app as app_act, notify as notify_act, overlays as ov, sidebar as sb, terminal as term_act, Owner};
use crate::overlays::OverlayHost;
use crate::persist;
use crate::perf;
use crate::sidebar::SidebarView;
use crate::state::AppState;
use crate::theme::{ActiveTheme, Theme};
use crate::workspace::{self, WorkspaceView};

pub struct Root {
    state: Entity<AppState>,
    sidebar: Entity<SidebarView>,
    workspace: Entity<WorkspaceView>,
    /// D 浮层：铺满窗口的最上层（右键菜单、toast、⌘K、设置……）
    overlays: Entity<OverlayHost>,
    /// 兜底焦点：窗口里什么都没聚焦时（空工作区、浮层刚关掉）键盘事件的派发路径只有根节点，
    /// 挂在根 div 上的全局 action（⌘K、⌘T……）收不到。给根 div 一个焦点、没人聚焦时落到它上
    focus: gpui::FocusHandle,
    first_frame_marked: bool,
    startup_reported: bool,
}

impl Root {
    fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| SidebarView::new(state.clone(), cx));
        let workspace = cx.new(|cx| WorkspaceView::new(state.clone(), cx));
        let overlays = OverlayHost::install(state.clone(), workspace.clone(), cx);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self { state, sidebar, workspace, overlays, focus: cx.focus_handle(), first_frame_marked: false, startup_reported: false }
    }
}

fn todo_action(what: &str, owner: Owner) {
    eprintln!("[未实现] {what}（归 {} 包）", owner.label());
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.first_frame_marked {
            self.first_frame_marked = true;
            window.on_next_frame(|_, _| perf::mark("首帧画出"));
        }
        let (loaded, n, collapsed) = {
            let s = self.state.read(cx);
            (s.loaded, s.sessions.len(), s.prefs.sidebar.collapsed)
        };
        if loaded && !self.startup_reported {
            self.startup_reported = true;
            window.on_next_frame(move |_, _| {
                perf::mark("侧栏有数据");
                perf::report_startup(n);
            });
        }
        if window.focused(cx).is_none() {
            window.focus(&self.focus);
        }
        let theme = cx.theme().clone();
        let state = self.state.clone();
        let el = div()
            .id("root")
            .key_context("Root")
            .track_focus(&self.focus)
            .flex()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.fg)
            .font_family(".SystemUIFont")
            // ---- F0 已实现 ----
            .on_action(|_: &app_act::Quit, _, cx| cx.quit())
            .on_action({
                let state = state.clone();
                move |_: &app_act::Refresh, _, cx| state.update(cx, |s, cx| s.refresh(cx))
            })
            .on_action({
                let state = state.clone();
                move |_: &app_act::ToggleSidebar, _, cx| state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.collapsed = !p.sidebar.collapsed))
            })
            // ---- 占位：各包实现后删掉 ----
            .on_action({
                let host = self.overlays.clone();
                move |_: &ov::TogglePalette, window, cx| host.update(cx, |o, cx| o.toggle_palette(window, cx))
            })
            .on_action({
                let host = self.overlays.clone();
                move |_: &ov::FindInTerminal, window, cx| host.update(cx, |o, cx| o.open_search(None, window, cx))
            })
            .on_action({
                let host = self.overlays.clone();
                move |_: &ov::OpenSettings, window, cx| host.update(cx, |o, cx| o.open_settings(window, cx))
            })
            .on_action(|_: &sb::FocusSearch, _, _| todo_action("⌘⇧F 侧栏搜索", Owner::Sidebar))
            .on_action(|_: &sb::RevealActive, _, _| todo_action("⌘L 侧栏定位", Owner::Sidebar))
            .on_action(|_: &notify_act::ToggleNotificationCenter, _, _| todo_action("⌘I 通知中心", Owner::Notify))
            .on_action(|_: &term_act::FontIncrease, _, _| todo_action("⌘= 字号 +1", Owner::Terminal))
            .on_action(|_: &term_act::FontDecrease, _, _| todo_action("⌘- 字号 -1", Owner::Terminal))
            .on_action(|_: &term_act::FontReset, _, _| todo_action("⌘0 字号重置", Owner::Terminal));
        let el = workspace::register_actions(el, self.workspace.clone(), cx);
        el.when(!collapsed, |d| d.child(self.sidebar.clone()))
            .child(div().flex_1().min_w_0().h_full().child(self.workspace.clone()))
            .child(self.overlays.clone())
    }
}

/// 应用入口（main.rs 只调这个）
pub fn run() {
    perf::mark("main 开始");
    Application::new().with_assets(crate::overlays::icons::Assets).run(|cx: &mut App| {
        let prefs = match persist::state_path() {
            Some(p) => persist::load_or_import(&p, persist::webkit::default_root().as_deref()),
            None => persist::NativeState::default(),
        };
        cx.set_global(Theme::by_id(&prefs.theme.id, &prefs.theme.imported));
        let saver = persist::state_path().map(persist::Saver::new);
        let state = AppState::init(prefs, saver, cx);
        actions::bind_all(cx);

        let bounds = Bounds::centered(None, size(px(1400.0), px(900.0)), cx);
        let root_state = state.clone();
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: Some(TitlebarOptions { title: Some(SharedString::from("makit")), ..Default::default() }),
                    window_min_size: Some(size(px(900.0), px(560.0))),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| Root::new(root_state, cx)),
            )
            .expect("窗口创建失败");
        perf::mark("窗口创建");

        // 退出前把防抖中的状态写掉
        let quit_state = state.clone();
        cx.on_app_quit(move |cx| {
            quit_state.read(cx).flush();
            async {}
        })
        .detach();

        // 调试用：屏幕锁着 / 窗口被挡住时 GPUI 会停掉 display link、一帧都不画。
        // 设了这个变量就每 16ms 手动 draw 一次（不上屏），让布局 / 绘制代码照样跑，好在无人值守时抓 panic
        if std::env::var_os("MAKIT_NATIVE_FORCE_DRAW").is_some() {
            let any: gpui::AnyWindowHandle = handle.into();
            cx.spawn(async move |cx| loop {
                cx.background_executor().timer(Duration::from_millis(16)).await;
                if cx.update_window(any, |_, window, cx| window.draw(cx).clear()).is_err() {
                    break;
                }
            })
            .detach();
        }
        if let Ok(mode) = std::env::var("MAKIT_NATIVE_SELFTEST") {
            crate::selftest::run(mode, handle, state, cx);
        }
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
