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

use crate::actions::{self, app as app_act, notify as notify_act, overlays as ov, sidebar as sb};
use crate::overlays::OverlayHost;
use crate::persist;
use crate::perf;
use crate::sidebar::{SidebarEvent, SidebarView};
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
    /// 工作区视图（自检 / 别的包要调工作区方法时用，比如 E 包从通知跳转后闪牌）
    pub fn workspace_view(&self) -> Entity<WorkspaceView> {
        self.workspace.clone()
    }

    fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let sidebar = cx.new(|cx| SidebarView::new(state.clone(), cx));
        let workspace = cx.new(|cx| WorkspaceView::new(state.clone(), cx));
        let overlays = OverlayHost::install(state.clone(), workspace.clone(), cx);
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        // B 侧栏：Esc 把焦点还给当前终端
        cx.subscribe(&sidebar, |this: &mut Self, _, ev: &SidebarEvent, cx| match ev {
            SidebarEvent::ReturnFocus => this.workspace.update(cx, |w, cx| w.refocus(cx)),
        })
        .detach();
        // E 通知 → B 侧栏：跳转的会话不在任何标签里时，在侧栏定位它（只定位，不打开）
        cx.subscribe(&crate::notify::Notifier::global(cx), |this: &mut Self, _, ev: &crate::notify::NotifyEvent, cx| {
            if let crate::notify::NotifyEvent::RevealSession { session_id } = ev {
                this.state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.collapsed = false));
                let id = Some(session_id.clone());
                this.sidebar.update(cx, |s, cx| s.reveal_session(id, cx));
            }
        })
        .detach();
        Self { state, sidebar, workspace, overlays, focus: cx.focus_handle(), first_frame_marked: false, startup_reported: false }
    }
}

impl Root {
    /// 自检用（selftest.rs 的 sidebar 模式）
    pub fn sidebar(&self) -> Entity<SidebarView> {
        self.sidebar.clone()
    }
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
            // ---- E 通知 ----
            .on_action(|_: &notify_act::ToggleNotificationCenter, window, cx| {
                crate::notify::Notifier::global(cx).update(cx, |n, cx| n.toggle(window, cx))
            })
            .on_action({
                let state = state.clone();
                move |_: &app_act::ToggleSidebar, _, cx| state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.collapsed = !p.sidebar.collapsed))
            })
            // ---- D 浮层 ----
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
            // ---- B 侧栏：先展开侧栏（折叠时侧栏不渲染，收不到 action），再转给侧栏 ----
            .on_action({
                let (state, sidebar) = (state.clone(), self.sidebar.clone());
                move |_: &sb::FocusSearch, _, cx| {
                    state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.collapsed = false));
                    sidebar.update(cx, |s, cx| s.request_focus_search(cx));
                }
            })
            .on_action({
                let (state, sidebar) = (state.clone(), self.sidebar.clone());
                move |_: &sb::RevealActive, _, cx| {
                    state.update(cx, |s, cx| s.update_prefs(cx, |p| p.sidebar.collapsed = false));
                    sidebar.update(cx, |s, cx| s.reveal_active(cx));
                }
            });
        // A 终端：⌘= / ⌘- / ⌘0 由 TerminalView 自己处理（快捷键表里限定在 Terminal 上下文），不再挂占位
        let el = workspace::register_actions(el, self.workspace.clone(), cx);
        // C 工作区：自绘标题栏在最上面一条（红绿灯 / 折叠按钮 / 标题），下面是侧栏 + 工作区
        let titlebar = workspace::titlebar::render_titlebar(&self.state, window, cx);
        el.flex_col()
            .child(titlebar)
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(!collapsed, |d| d.child(self.sidebar.clone()))
                    .child(div().flex_1().min_w_0().h_full().child(self.workspace.clone())),
            )
            // E 通知：⌘I 抽屉 + 窗口闪一下（deferred 浮层）
            .child(crate::notify::layer(window, cx))
            .child(self.overlays.clone())
    }
}

/// E 通知 → D 设置页：授权状态、请求授权、测试通知
fn wire_notify_settings(cx: &mut App) {
    use crate::notify::{system::Permission, Notifier};
    crate::overlays::settings::set_notify_hooks(
        crate::overlays::settings::NotifyHooks {
            permission: std::rc::Rc::new(|cx: &App| match Notifier::global(cx).read(cx).permission {
                Permission::Unknown => None,
                Permission::Granted => Some(true),
                _ => Some(false),
            }),
            request_permission: std::rc::Rc::new(|_, cx| Notifier::global(cx).read(cx).request_permission()),
            send_test: std::rc::Rc::new(|window, cx| {
                if !Notifier::global(cx).read(cx).send_test(cx) {
                    // Tauri 版：总开关关着 / 没授权时弹 warning，不静默
                    let _ = window.prompt(
                        gpui::PromptLevel::Warning,
                        "没有发出测试通知",
                        Some("系统通知总开关关着，或 makit 还没拿到通知权限（需要装在「应用程序」里的签名包）。"),
                        &["好"],
                        cx,
                    );
                }
            }),
        },
        cx,
    );
}

/// 应用入口（main.rs 只调这个）
pub fn run() {
    perf::mark("main 开始");
    Application::new().with_assets(crate::assets::Assets).run(|cx: &mut App| {
        let prefs = match persist::state_path() {
            Some(p) => persist::load_or_import(&p, persist::webkit::default_root().as_deref()),
            None => persist::NativeState::default(),
        };
        cx.set_global(Theme::by_id(&prefs.theme.id, &prefs.theme.imported));
        let saver = persist::state_path().map(persist::Saver::new);
        let state = AppState::init(prefs, saver, cx);
        crate::notify::Notifier::init(state.clone(), cx);
        wire_notify_settings(cx);
        actions::bind_all(cx);

        let bounds = Bounds::centered(None, size(px(1400.0), px(900.0)), cx);
        let root_state = state.clone();
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    // C 工作区：透明标题栏（= Tauri 的 titleBarStyle Overlay + hiddenTitle），标题栏由 workspace::titlebar 自绘
                    titlebar: Some(TitlebarOptions { title: Some(SharedString::from("makit")), appears_transparent: true, ..Default::default() }),
                    window_min_size: Some(size(px(900.0), px(560.0))),
                    // 自检时用：不抢用户正在用的键盘焦点（B 侧栏包加的，见 sidebar/selftest.rs）
                    focus: std::env::var_os("MAKIT_NATIVE_BACKGROUND").is_none(),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| Root::new(root_state, cx)),
            )
            .expect("窗口创建失败");
        perf::mark("窗口创建");
        crate::notify::attach_window(handle, cx);

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
        if std::env::var_os("MAKIT_NATIVE_BACKGROUND").is_none() {
            cx.activate(true);
        }
    });
}
