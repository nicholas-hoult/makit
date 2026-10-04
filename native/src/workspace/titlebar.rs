//! 自绘全局标题栏（App.tsx `.app-titlebar` + App.css 同名段落）。
//!
//! 窗口用 `appears_transparent` 标题栏（= Tauri 的 `titleBarStyle: Overlay` + `hiddenTitle`）：
//! 红绿灯留在系统原位，内容从 y=0 画起，标题栏这一条由这里画。
//!
//! ```text
//! | 左段：宽 = 侧栏宽（折叠时 78）| 2px 分隔 | 工作区段：标题居中「项目 · 分支」 |
//!          ↑ x=68 起绝对定位：[折叠侧栏按钮][铃铛挂点]
//! ```
//! - 拖动窗口：macOS 原生（全尺寸内容视图下标题栏条带仍由 AppKit 负责拖动）。GPUI 的 `start_window_move`
//!   在 mac 上是空操作，所以不像 TS 版那样显式调
//! - 双击：`titlebar_double_click`（按系统「双击标题栏」设置，默认缩放 = TS 的 toggleMaximize）；点在按钮上不算
//! - 右键：什么都不弹（GPUI 没有 WebView 那种默认菜单，天然满足）
//!
//! **挂点**（别的包往标题栏里放东西，不用改这个文件）：
//! - 铃铛（E 通知包）：`crate::notify::bell(cx)`，画在折叠按钮右边；
//!   它的左边界 = `BELL_LEFT`（通知抽屉按这个定位，同 TS 的 `bellBtnRef.getBoundingClientRect().left`）
//! - 侧栏分隔线高亮（B 侧栏包）：拖 / 悬停侧栏 resizer 时 `cx.set_global(TitlebarResizerHot(true))`

use crate::ts;
use gpui::{div, prelude::*, px, svg, AnyElement, App, ClickEvent, Entity, Global, Window};

use super::labels::titlebar_text;
use crate::actions::app as app_act;
use crate::state::AppState;
use crate::theme::ActiveTheme;

pub const TITLEBAR_H: f32 = 32.0;
/// 侧栏折叠时左段只留红绿灯的宽度
pub const TRAFFIC_LIGHTS_W: f32 = 78.0;
/// 按钮组的左边界（`.app-titlebar-controls { left: 68px }`）
pub const CONTROLS_LEFT: f32 = 68.0;
/// 标题栏图标按钮 20×22（TS 版是 24×22，#238 起图标之间收紧）
pub const ICON_BTN_W: f32 = 20.0;
pub const ICON_BTN_H: f32 = 22.0;
/// 铃铛按钮的左边界（窗口坐标）：折叠按钮右边
pub const BELL_LEFT: f32 = CONTROLS_LEFT + ICON_BTN_W;

/// B 侧栏包：侧栏 resizer 悬停 / 拖动中
#[derive(Clone, Copy, Default)]
pub struct TitlebarResizerHot(pub bool);
impl Global for TitlebarResizerHot {}

/// 标题栏图标按钮的外壳（24×22、圆角 4、0.5 透明度，hover 到 1 并加底色）。给 E 包画铃铛也可以用
pub fn icon_button(id: &'static str, icon: &'static str, size: f32, cx: &App) -> gpui::Stateful<gpui::Div> {
    let t = cx.theme();
    div()
        .id(id)
        .relative()
        .w(px(ICON_BTN_W))
        .h(px(ICON_BTN_H))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.0))
        .cursor_pointer()
        .opacity(0.5)
        .hover(|s| s.opacity(1.0).bg(t.bg_hover))
        .child(svg().path(icon).size(px(size)).text_color(t.fg_muted))
}

pub fn render_titlebar(state: &Entity<AppState>, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = cx.theme().clone();
    let s = state.read(cx);
    let collapsed = s.prefs.sidebar.collapsed;
    let sidebar_w = s.prefs.sidebar.width;
    let tab = s.workspace.active_tab();
    let meta = tab.and_then(|t| t.session_id.as_deref()).and_then(|id| s.session(id));
    let title = titlebar_text(tab, meta);
    let hot = cx.try_global::<TitlebarResizerHot>().map(|h| h.0).unwrap_or(false);
    let bell = crate::notify::bell(cx);
    let _fullscreen = window.is_fullscreen(); // 全屏时 TS 只去掉 `.tab` 的装饰线，工作区这边没有对应的线，不用改

    div()
        .id("app-titlebar")
        .relative()
        .flex()
        .flex_none()
        .h(px(TITLEBAR_H))
        .w_full()
        .on_click(|ev: &ClickEvent, window, _| {
            if ev.click_count() == 2 {
                window.titlebar_double_click();
            }
        })
        // 左段：镜像侧栏宽度，承接侧栏的右边框，让竖线从 y=0 贯穿
        .child(
            div()
                .flex_none()
                .h_full()
                .w(px(if collapsed { TRAFFIC_LIGHTS_W } else { sidebar_w }))
                .when(!collapsed, |d| d.bg(t.bg_soft).border_r_1().border_color(t.border_strong))
                .when(collapsed, |d| d.bg(t.bg)),
        )
        .when(!collapsed, |d| d.child(div().flex_none().w(px(2.0)).h_full().when(hot, |d| d.bg(t.accent))))
        // 工作区段：标题居中
        .child(
            div().flex_1().min_w_0().h_full().flex().items_center().justify_center().bg(t.bg).child(
                div()
                    .px(px(12.0))
                    .max_w_full()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    // dev 实例（dev.sh 传了 MAKIT_APP_NAME）：标题前面挂一个醒目的名字标签，免得和线上的窗口关错（#242）
                    .when(crate::app::is_dev_instance(), |d| {
                        d.child(
                            div()
                                .flex_none()
                                .px(px(6.0))
                                .rounded(px(4.0))
                                .bg(t.warning)
                                .text_color(t.bg)
                                .text_size(px(11.0))
                                .font_weight(gpui::FontWeight::BOLD)
                                .child(crate::app::app_name()),
                        )
                    })
                    .child(div().min_w_0().truncate().text_size(px(12.0)).text_color(t.fg_muted).opacity(0.7).child(title)),
            ),
        )
        // 按钮组：x=68 绝对定位，侧栏展开 / 折叠都看得见
        .child(
            div()
                .absolute()
                .left(px(CONTROLS_LEFT))
                .top_0()
                .h(px(TITLEBAR_H))
                .flex()
                .items_center()
                .child(
                    icon_button("titlebar-sidebar-toggle", "icons/titlebar-sidebar-toggle.svg", 14.0, cx)
                        .tooltip(crate::tooltip::tip(ts!("titlebar.collapse_sidebar")))
                        .on_click(|_, window, cx| {
                            cx.stop_propagation();
                            window.dispatch_action(Box::new(app_act::ToggleSidebar), cx);
                        }),
                )
                .child(bell),
        )
        .into_any_element()
}
