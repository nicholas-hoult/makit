//! makit 的 GPUI 原生界面原型（#221 实验）。
//! 侧栏会话 + 多标签终端（左右分屏），业务逻辑用 makit-core（不依赖 src-tauri）。

mod perf;
mod terminal;
mod workspace;

use gpui::{
    App, Application, Bounds, KeyBinding, SharedString, TitlebarOptions, WindowBounds,
    WindowOptions, prelude::*, px, size,
};

use workspace::Workspace;

fn main() {
    perf::mark("main 开始");
    Application::new().run(|cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("cmd-t", workspace::NewTab, None),
            KeyBinding::new("cmd-w", workspace::CloseTab, None),
            KeyBinding::new("cmd-d", workspace::SplitRight, None),
            KeyBinding::new("cmd-shift-]", workspace::NextTab, None),
            KeyBinding::new("cmd-shift-[", workspace::PrevTab, None),
            KeyBinding::new("ctrl-tab", workspace::NextTab, None),
            KeyBinding::new("ctrl-shift-tab", workspace::PrevTab, None),
            KeyBinding::new("cmd-]", workspace::FocusOtherPane, None),
            KeyBinding::new("cmd-q", workspace::Quit, None),
            KeyBinding::new("cmd-c", terminal::Copy, Some("Terminal")),
            KeyBinding::new("cmd-v", terminal::Paste, Some("Terminal")),
            KeyBinding::new("shift-pageup", terminal::ScrollPageUp, Some("Terminal")),
            KeyBinding::new("shift-pagedown", terminal::ScrollPageDown, Some("Terminal")),
        ]);
        let bounds = Bounds::centered(None, size(px(1400.0), px(900.0)), cx);
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some(SharedString::from("makit (GPUI 原型)")),
                    ..Default::default()
                }),
                window_min_size: Some(size(px(900.0), px(560.0))),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| Workspace::new(window, cx)),
        )
        .expect("窗口创建失败");
        perf::mark("窗口创建");
        // 调试用：屏幕锁着 / 窗口被挡住时 GPUI 会停掉 display link、一帧都不画。
        // 设了这个变量就每 16ms 手动 draw 一次（不上屏），让布局 / 绘制代码照样跑，好在无人值守时抓 panic、看网格内容
        if std::env::var_os("MAKIT_NATIVE_FORCE_DRAW").is_some() {
            let any: gpui::AnyWindowHandle = handle.into();
            cx.spawn(async move |cx| loop {
                cx.background_executor().timer(std::time::Duration::from_millis(16)).await;
                if cx.update_window(any, |_, window, cx| window.draw(cx).clear()).is_err() {
                    break;
                }
            })
            .detach();
        }
        cx.on_window_closed(|cx| cx.quit()).detach();
        cx.activate(true);
    });
}
