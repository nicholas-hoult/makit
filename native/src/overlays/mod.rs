//! 浮层（D 包）：通用右键菜单、toast、⌘K 命令面板、⌘F 搜索条、会话详情、恢复 cwd 对话框、设置 ⌘,。
//!
//! 全部浮层挂在一个 `OverlayHost` 实体上（根视图最后一个孩子，铺满窗口、画在最上层），
//! 它同时挂成全局 `GlobalOverlays`，别的包用下面这几个自由函数调用，不需要拿到实体。
//!
//! ## 右键菜单（B 侧栏、C 工作区用）
//!
//! ```ignore
//! use crate::overlays::{self, MenuItem};
//!
//! div().on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
//!     let sid = sid.clone();
//!     overlays::show_context_menu(ev.position, window, cx, move |cx| {
//!         // 闭包每帧都会被调一次：这里读 AppState 拿**当下**的状态，不要在外面先算好
//!         let pinned = AppState::global(cx).read(cx).is_pinned(&sid);
//!         vec![
//!             MenuItem::action(if pinned { "取消置顶" } else { "置顶" }, {
//!                 let sid = sid.clone();
//!                 move |_window, cx| overlays::session_ops::toggle_pin(&sid, cx)
//!             }),
//!             MenuItem::separator(),
//!             MenuItem::action("关闭右侧", |_, _| {}).disabled(true), // 灰掉而不是不显示
//!         ]
//!     });
//!     cx.stop_propagation();
//! }))
//! ```
//!
//! - 落点：传鼠标的窗口坐标（`MouseDownEvent::position`），菜单自己量尺寸、按 `clamp_menu_position` 贴边
//! - 关闭：点菜单外（左 / 右键）、Esc、点了某一项（先关菜单再调 handler）。关闭后焦点还给打开前的元素
//! - handler 签名 `Fn(&mut Window, &mut App)`：里面可以再开别的浮层（详情、确认框……）
//!
//! ## 其他入口
//!
//! | 函数 | 用途 |
//! |---|---|
//! | `show_toast(text, ms, cx)` | 底部居中的提示；时长常量见 `toast::` |
//! | `open_detail(session_id, window, cx)` | 会话详情面板（侧栏右键「查看对话」） |
//! | `session_ops::*` | 置顶 / 归档（运行中先弹系统确认框）/ 复制 / Finder 显示 / 打开会话 |
//! | `recover::cwd_missing(tab_id, cwd, window, cx)` | 终端发现启动目录不在时调（A 包） |
//! | `search_bar::report_progress(..)` / `search_bar::set_target(..)` | ⌘F 搜索条和终端之间的接口（A 包） |
//!
//! 面板内的键（Esc / Enter / ↑↓ / ⌘Enter……）在 `actions::overlays` 里，context `Overlay`；
//! 每个浮层的 key_context 都带 `Overlay`。输入框是 `text_input::TextInput`（带输入法），别的包也可以用。

pub mod context_menu;
pub mod detail;
pub mod detail_logic;
pub mod icons;
pub mod palette;
pub mod palette_logic;
pub mod recover;
pub mod recover_logic;
pub mod search_bar;
pub mod search_count;
pub mod selftest;
pub mod session_ops;
pub mod settings;
pub mod shortcuts;
pub mod style;
pub mod text_input;
pub mod toast;

use gpui::{
    div, prelude::*, px, App, Context, Entity, FocusHandle, Global, MouseButton, Pixels, Point, SharedString, Subscription, Task,
    Window,
};

use crate::actions::overlays as act;
use crate::state::AppState;
use crate::theme::ActiveTheme;
use crate::workspace::WorkspaceView;
pub use context_menu::{MenuBuilder, MenuItem};
use style::*;

pub struct GlobalOverlays(pub Entity<OverlayHost>);
impl Global for GlobalOverlays {}

/// 拿全局的浮层宿主。启动早期（根视图建好之前）是 None
pub fn host(cx: &App) -> Option<Entity<OverlayHost>> {
    cx.try_global::<GlobalOverlays>().map(|g| g.0.clone())
}

/// 在鼠标处弹右键菜单。用法见文件头
pub fn show_context_menu(position: Point<Pixels>, window: &mut Window, cx: &mut App, build: impl Fn(&App) -> Vec<MenuItem> + 'static) {
    if let Some(h) = host(cx) {
        h.update(cx, |h, cx| h.open_menu(position, std::rc::Rc::new(build), window, cx));
    }
}

/// 打开会话详情面板（侧栏右键「查看对话」）
pub fn open_detail(session_id: &str, window: &mut Window, cx: &mut App) {
    if let Some(h) = host(cx) {
        h.update(cx, |h, cx| h.open_detail(session_id, window, cx));
    }
}

/// 打开 ⌘F 搜索条并填入文字（终端右键「搜索选中内容」）
pub fn find_in_terminal(initial: Option<String>, window: &mut Window, cx: &mut App) {
    if let Some(h) = host(cx) {
        h.update(cx, |h, cx| h.open_search(initial, window, cx));
    }
}

/// 打开设置（侧栏菜单「设置…」；⌘, 走 action）
pub fn open_settings(window: &mut Window, cx: &mut App) {
    if let Some(h) = host(cx) {
        h.update(cx, |h, cx| h.open_settings(window, cx));
    }
}

/// 底部 toast，`ms` 毫秒后消失；新的直接替换旧的
pub fn show_toast(text: impl Into<SharedString>, ms: u64, cx: &mut App) {
    if let Some(h) = host(cx) {
        h.update(cx, |h, cx| h.toast(text.into(), ms, cx));
    }
}

/// 一个打开着的浮层视图 + 打开前的焦点（关掉后还回去）
struct Open<V> {
    view: Entity<V>,
    prev: Option<FocusHandle>,
    _sub: Subscription,
}

impl<V> Open<V> {
    fn close(self, window: &mut Window) {
        if let Some(p) = self.prev {
            window.focus(&p);
        }
    }
}

struct MenuState {
    pos: Point<Pixels>,
    build: MenuBuilder,
    focus: FocusHandle,
    /// 打开前焦点在哪，关掉后还回去
    prev: Option<FocusHandle>,
}

pub struct OverlayHost {
    state: Entity<AppState>,
    pub(crate) workspace: Entity<WorkspaceView>,
    menu: Option<MenuState>,
    toast: Option<SharedString>,
    toast_task: Option<Task<()>>,
    palette: Option<Open<palette::PaletteView>>,
    settings: Option<Open<settings::SettingsView>>,
    detail: Option<Open<detail::DetailView>>,
    recover: Option<Open<recover::RecoverDialog>>,
    search: Option<Open<search_bar::SearchBar>>,
}

/// 关掉某个槽位里的浮层（焦点还回去）
macro_rules! close_slot {
    ($this:ident . $slot:ident, $window:ident, $cx:ident) => {
        if let Some(o) = $this.$slot.take() {
            o.close($window);
            $cx.notify();
        }
    };
}

impl OverlayHost {
    /// 根视图建好侧栏 / 工作区之后调；同时挂成全局
    pub fn install(state: Entity<AppState>, workspace: Entity<WorkspaceView>, cx: &mut App) -> Entity<Self> {
        let host = cx.new(|cx| {
            cx.observe(&state, |_, _, cx| cx.notify()).detach();
            Self {
                state,
                workspace,
                menu: None,
                toast: None,
                toast_task: None,
                palette: None,
                settings: None,
                detail: None,
                recover: None,
                search: None,
            }
        });
        cx.set_global(GlobalOverlays(host.clone()));
        host
    }

    // ---- ⌘K ----

    /// ⌘K：开着就关，关着就开（每次打开都是新实体 = 重置 query / 光标 / 筛选栏 / 历史）
    pub fn toggle_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.palette.take() {
            p.close(window);
        } else {
            let prev = window.focused(cx);
            let state = self.state.clone();
            let view = cx.new(|cx| palette::PaletteView::new(state, window, cx));
            let sub = cx.subscribe_in(&view, window, |this, _, ev: &palette::PaletteEvent, window, cx| match ev {
                palette::PaletteEvent::Close => {
                    if let Some(p) = this.palette.take() {
                        p.close(window);
                        cx.notify();
                    }
                }
            });
            self.palette = Some(Open { view, prev, _sub: sub });
        }
        cx.notify();
    }

    // ---- 设置 ⌘, ----

    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = &self.settings {
            window.focus(&s.view.read(cx).focus_handle());
            return;
        }
        let prev = window.focused(cx);
        let state = self.state.clone();
        let view = cx.new(|cx| settings::SettingsView::new(state, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &settings::SettingsEvent, window, cx| match ev {
            settings::SettingsEvent::Close => close_slot!(this.settings, window, cx),
        });
        self.settings = Some(Open { view, prev, _sub: sub });
        cx.notify();
    }

    // ---- 会话详情 ----

    pub fn open_detail(&mut self, session_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let prev = match self.detail.take() {
            Some(o) => o.prev,
            None => window.focused(cx),
        };
        let Some(meta) = self.state.read(cx).session(session_id).cloned() else { return };
        let view = cx.new(|cx| detail::DetailView::new(meta, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &detail::DetailEvent, window, cx| match ev {
            detail::DetailEvent::Close => close_slot!(this.detail, window, cx),
        });
        self.detail = Some(Open { view, prev, _sub: sub });
        cx.notify();
    }

    // ---- 恢复 cwd 对话框 ----

    /// 同一时间只弹一个：已经有一个开着就什么都不做（那个 pane 的终端里留着提示）
    pub fn open_recover(&mut self, tab_id: String, cwd: String, session_id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if self.recover.is_some() {
            return;
        }
        let prev = window.focused(cx);
        let state = self.state.clone();
        let view = cx.new(|cx| recover::RecoverDialog::new(state, tab_id, cwd, session_id, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &recover::RecoverEvent, window, cx| match ev {
            recover::RecoverEvent::Close => close_slot!(this.recover, window, cx),
        });
        self.recover = Some(Open { view, prev, _sub: sub });
        cx.notify();
    }

    // ---- ⌘F 搜索条 ----

    pub fn search_bar(&self) -> Option<Entity<search_bar::SearchBar>> {
        self.search.as_ref().map(|o| o.view.clone())
    }

    pub fn open_search(&mut self, initial: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(o) = &self.search {
            let v = o.view.clone();
            v.update(cx, |b, cx| {
                if let Some(t) = initial {
                    b.set_text(t, cx);
                }
                b.focus(window, cx);
            });
            return;
        }
        let prev = window.focused(cx);
        let view = cx.new(|cx| search_bar::SearchBar::new(initial, window, cx));
        let sub = cx.subscribe_in(&view, window, |this, _, ev: &search_bar::SearchBarEvent, window, cx| match ev {
            search_bar::SearchBarEvent::Close => close_slot!(this.search, window, cx),
        });
        self.search = Some(Open { view, prev, _sub: sub });
        cx.notify();
    }

    // ---- 右键菜单 ----

    pub fn open_menu(&mut self, pos: Point<Pixels>, build: MenuBuilder, window: &mut Window, cx: &mut Context<Self>) {
        // 菜单开着时再右键：先关旧的（旧的焦点记录作废，沿用最早那次的）
        let prev = match self.menu.take() {
            Some(m) => m.prev,
            None => window.focused(cx),
        };
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.menu = Some(MenuState { pos, build, focus, prev });
        cx.notify();
    }

    pub fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(m) = self.menu.take() {
            if let Some(p) = m.prev {
                window.focus(&p);
            }
            cx.notify();
        }
    }

    pub fn menu_open(&self) -> bool {
        self.menu.is_some()
    }

    /// 自检用：现在开着哪些浮层（逗号分隔）
    pub fn debug_open(&self) -> String {
        let mut v = Vec::new();
        for (on, name) in [
            (self.palette.is_some(), "palette"),
            (self.settings.is_some(), "settings"),
            (self.search.is_some(), "search"),
            (self.detail.is_some(), "detail"),
            (self.recover.is_some(), "recover"),
            (self.menu.is_some(), "menu"),
            (self.toast.is_some(), "toast"),
        ] {
            if on {
                v.push(name);
            }
        }
        v.join(",")
    }

    fn render_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        use context_menu::*;
        let m = self.menu.as_ref()?;
        let items = (m.build)(cx);
        let theme = cx.theme().clone();
        let max_label = items
            .iter()
            .filter_map(|i| match i {
                MenuItem::Action { label, .. } | MenuItem::Header(label) => Some(measure(label, window)),
                MenuItem::Separator => None,
            })
            .fold(0.0_f32, f32::max);
        let check_slot = has_checks(&items);
        let max_label = max_label + if check_slot { CHECK_W } else { 0.0 };
        let (w, h) = (menu_width(max_label), menu_height(&items));
        let vp = window.viewport_size();
        let (x, y) = clamp_menu_position(f32::from(m.pos.x), f32::from(m.pos.y), w, h, f32::from(vp.width), f32::from(vp.height));
        let host = cx.entity();
        let rows = items.into_iter().enumerate().map(|(i, item)| match item {
            MenuItem::Separator => div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(theme.border).into_any_element(),
            MenuItem::Header(label) => div()
                .h(px(HEADER_H))
                .px(px(ITEM_PAD_X))
                .pt(px(6.0))
                .text_size(px(11.0))
                .line_height(px(14.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.fg_subtle)
                .whitespace_nowrap()
                .child(label)
                .into_any_element(),
            MenuItem::Action { label, disabled, checked, handler } => {
                let host = host.clone();
                let hover = theme.bg_hover;
                div()
                    .id(("menu-item", i))
                    .w_full()
                    .h(px(ITEM_H))
                    .px(px(ITEM_PAD_X))
                    .flex()
                    .items_center()
                    .rounded(px(RADIUS))
                    .text_size(px(FONT))
                    .line_height(px(14.0))
                    .whitespace_nowrap()
                    .text_color(if disabled { theme.fg_muted } else { theme.fg })
                    .when(!disabled, |d| {
                        d.cursor_pointer().hover(move |s| s.bg(hover)).on_click(move |_, window, cx| {
                            host.update(cx, |h, cx| h.close_menu(window, cx));
                            handler(window, cx);
                        })
                    })
                    .when(check_slot, |d| d.child(div().w(px(CHECK_W)).flex_none().text_color(theme.accent).child(if checked { "✓" } else { "" })))
                    .child(label)
                    .into_any_element()
            }
        });
        let close = {
            let host = host.clone();
            move |_: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| host.update(cx, |h, cx| h.close_menu(window, cx))
        };
        let focus = m.focus.clone();
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    // 背板：点外面（左 / 右键）只关菜单，底下的元素收不到这一下
                    div()
                        .id("menu-backdrop")
                        .absolute()
                        .size_full()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, close.clone())
                        .on_mouse_down(MouseButton::Right, close),
                )
                .child(
                    div()
                        .id("context-menu")
                        .key_context("Overlay ContextMenu")
                        .track_focus(&focus)
                        .on_action(cx.listener(|this, _: &act::Dismiss, window, cx| this.close_menu(window, cx)))
                        .occlude()
                        .absolute()
                        .left(px(x))
                        .top(px(y))
                        .w(px(w))
                        .max_h(vp.height - px(16.0))
                        .overflow_y_scroll()
                        .p(px(MENU_PAD))
                        .bg(theme.bg_soft)
                        .border_1()
                        .border_color(theme.border_strong)
                        .rounded(px(RADIUS_MD))
                        .shadow(vec![shadow(8.0, 24.0, theme.var("--shadow-strong"))])
                        .children(rows),
                )
                .into_any_element(),
        )
    }

    // ---- toast ----

    pub fn toast(&mut self, text: SharedString, ms: u64, cx: &mut Context<Self>) {
        self.toast = Some(text);
        cx.notify();
        // 新 toast 替换旧的：旧的计时任务随 Task 一起丢掉
        self.toast_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(ms)).await;
            let _ = this.update(cx, |h, cx| {
                h.toast = None;
                cx.notify();
            });
        }));
    }

    fn render_toast(&self, cx: &App) -> Option<gpui::AnyElement> {
        let text = self.toast.clone()?;
        let theme = cx.theme();
        // `.toast`：底部 16px 居中，--fg 底 / --bg 字（反色），8px 14px，--radius-md，12px
        Some(
            div()
                .absolute()
                .bottom(px(16.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .bg(theme.fg)
                        .text_color(theme.bg)
                        .px(px(14.0))
                        .py(px(8.0))
                        .rounded(px(RADIUS_MD))
                        .text_size(px(12.0))
                        .shadow(vec![shadow(4.0, 12.0, theme.shadow)])
                        .child(text),
                )
                .into_any_element(),
        )
    }
}

impl Render for OverlayHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toast = self.render_toast(cx);
        let menu = self.render_menu(window, cx);
        // 铺满窗口的透明层；自己不挂任何鼠标处理，不挡底下的点击 —— 只有打开的浮层才挡。
        // 叠放顺序照 CSS 的 z-index：搜索条(150) < ⌘K / 设置 / 恢复(200) < toast(1000) < 详情(2000) < 右键菜单(9999)
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(self.search.as_ref().map(|p| p.view.clone()))
            .children(self.palette.as_ref().map(|p| p.view.clone()))
            .children(self.settings.as_ref().map(|p| p.view.clone()))
            .children(self.recover.as_ref().map(|p| p.view.clone()))
            .children(toast)
            .children(self.detail.as_ref().map(|p| p.view.clone()))
            .children(menu)
    }
}
