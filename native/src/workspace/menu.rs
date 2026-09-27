//! 工作区右键菜单的**临时最小实现**（D 浮层包的通用菜单组件没好之前先顶着）。
//!
//! 长相照 App.css `.context-menu`（bg-soft、border-strong、圆角 6、内边距 4、最小宽 150、按钮 6×12、12px），
//! 行为照 ContextMenu.tsx：点外面关、点一项执行并关、disabled 灰掉不隐藏、靠屏幕边自动翻（`snap_to_window`）。
//! 还没做：Esc 关（要焦点，等 D 包的组件）。**D 包的组件好了之后把 `render_menu` 换成它**，菜单项数组不用改。

use std::rc::Rc;

use gpui::{anchored, deferred, div, point, prelude::*, px, AnyElement, App, BoxShadow, MouseButton, Pixels, Point, SharedString, Window};

use crate::theme::ActiveTheme;

pub type MenuAction = Rc<dyn Fn(&mut Window, &mut App)>;

pub enum MenuItem {
    Item { label: &'static str, disabled: bool, action: MenuAction },
    Sep,
}

pub fn item(label: &'static str, action: impl Fn(&mut Window, &mut App) + 'static) -> MenuItem {
    MenuItem::Item { label, disabled: false, action: Rc::new(action) }
}

pub fn item_if(enabled: bool, label: &'static str, action: impl Fn(&mut Window, &mut App) + 'static) -> MenuItem {
    MenuItem::Item { label, disabled: !enabled, action: Rc::new(action) }
}

/// 画在 `pos`（窗口坐标）。`close` 在点外面 / 点了某一项之后调
pub fn render_menu(pos: Point<Pixels>, items: Vec<MenuItem>, close: Rc<dyn Fn(&mut Window, &mut App)>, cx: &App) -> AnyElement {
    let t = cx.theme();
    let (fg, muted, hover, border) = (t.fg, t.fg_muted, t.bg_hover, t.border);
    let rows: Vec<AnyElement> = items
        .into_iter()
        .enumerate()
        .map(|(i, it)| match it {
            MenuItem::Sep => div().h(px(1.0)).mx(px(6.0)).my(px(4.0)).bg(border).into_any_element(),
            MenuItem::Item { label, disabled, action } => {
                let close = close.clone();
                div()
                    .id(SharedString::from(format!("ws-menu-{i}")))
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(4.0))
                    .text_size(px(12.0))
                    .text_color(if disabled { muted } else { fg })
                    .when(!disabled, |d| {
                        d.cursor_pointer().hover(move |s| s.bg(hover)).on_click(move |_, window, cx| {
                            action(window, cx);
                            close(window, cx);
                        })
                    })
                    .child(label)
                    .into_any_element()
            }
        })
        .collect();
    let close_out = close.clone();
    deferred(
        anchored().position(pos).snap_to_window().child(
            div()
                .id("ws-context-menu")
                .occlude()
                .min_w(px(150.0))
                .p(px(4.0))
                .bg(t.bg_soft)
                .border_1()
                .border_color(t.border_strong)
                .rounded(px(6.0))
                .shadow(vec![BoxShadow { color: t.var("--shadow-strong"), offset: point(px(0.0), px(8.0)), blur_radius: px(24.0), spread_radius: px(0.0) }])
                .on_mouse_down_out(move |_, window, cx| close_out(window, cx))
                // 菜单上右键：什么都不做（不冒到底下的 pane 再弹一个）
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .children(rows),
        ),
    )
    .with_priority(2)
    .into_any_element()
}
