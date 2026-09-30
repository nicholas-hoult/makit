//! 通知的界面：⌘I 抽屉（NotificationPanel.tsx）、标题栏铃铛（App.tsx:2093-2110）、窗口闪一下（App.css `.app-notify-flash`）。
//!
//! 尺寸 / 颜色照 App.css 的数值（行号见各处注释），颜色一律走 `cx.theme()`；
//! `color-mix(in srgb, A p%, B)` 用 `mix(A, B, p)` 算。

use std::time::{Duration, Instant};

use gpui::{
    anchored, canvas, deferred, div, point, prelude::*, px, svg, Animation, AnimationExt, AnyElement, App, BoxShadow, ClickEvent, Div,
    Entity, FontWeight, Hsla, MouseButton, Pixels, Rgba, SharedString, Window,
};

use crate::actions::notify as act;
use crate::theme::{ActiveTheme, Theme};

use super::center::{Notifier, FLASH_MS};
use super::model::Kind;

/// `color-mix(in srgb, a p, b)`：p 是 a 的比例
pub fn mix(a: Hsla, b: Hsla, p: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let m = |x: f32, y: f32| x * p + y * (1.0 - p);
    Rgba { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: m(a.a, b.a) }.into()
}

/// 任意宽度的边框（GPUI 只有整数档的 border_N）
fn border_px(mut d: Div, w: Pixels) -> Div {
    let s = d.style();
    s.border_widths.top = Some(w.into());
    s.border_widths.right = Some(w.into());
    s.border_widths.bottom = Some(w.into());
    s.border_widths.left = Some(w.into());
    d
}

/// 行里的状态色：需要处理的沿用 Tauri 版的 --warning；#215 新增的已完成用 --accent、出错用 --danger
pub fn kind_color(kind: Kind, t: &Theme) -> Hsla {
    match kind {
        Kind::NeedsPermission | Kind::NeedsInput => t.warning,
        Kind::TurnComplete => t.accent,
        Kind::Error => t.danger,
    }
}

/// 根视图里挂一次：抽屉 + 窗口闪一下。都是 deferred 浮层，没打开时什么都不画
pub fn layer(window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(n) = Notifier::try_global(cx) else { return div().into_any_element() };
    let (open, flash) = {
        let x = n.read(cx);
        (x.open, x.flash_until.filter(|t| *t > Instant::now()).map(|_| x.flash_seq))
    };
    div()
        .when_some(flash, |d, seq| d.child(flash_overlay(seq, cx)))
        .when(open, |d| d.child(drawer(n.clone(), window, cx)))
        .into_any_element()
}

/// 窗口边框亮一下：1.5px accent 边框、圆角 8，0.55s（App.css:63-76 `window-notify-flash`：0→18% 亮到 1→100% 淡出）
fn flash_overlay(seq: u64, cx: &App) -> AnyElement {
    let accent = cx.theme().accent;
    let frame = border_px(div(), px(1.5)).absolute().top_0().left_0().size_full().rounded(px(8.0)).border_color(accent);
    deferred(frame.with_animation(("notify-flash", seq), Animation::new(Duration::from_millis(FLASH_MS)), |el, t| {
        // CSS ease-out 近似
        let t = 1.0 - (1.0 - t) * (1.0 - t);
        el.opacity(if t < 0.18 { t / 0.18 } else { 1.0 - (t - 0.18) / 0.82 })
    }))
    .with_priority(10)
    .into_any_element()
}

fn drawer(n: Entity<Notifier>, window: &mut Window, cx: &mut App) -> AnyElement {
    let theme = cx.theme().clone();
    let vp = window.viewport_size();
    // App.css:1512-1529 .notif-drawer：top 40、left = 铃铛的 left（兜底 104）、height 66.67vh、width min(62vw, 680)
    let width = (vp.width * 0.62).min(px(680.0));
    let height = vp.height * 0.6667;
    let (left, records, selected, unread, focus, scroll, scrollbar) = {
        let x = n.read(cx);
        (x.bell_bounds.map(|b| b.origin.x).unwrap_or(px(104.0)), x.book.records.clone(), x.selected, x.unread_count(), x.focus.clone(), x.scroll.clone(), x.scrollbar.clone())
    };
    let now = crate::sidebar::now_secs();
    let count = records.len();

    let header = {
        let btn = |id: &'static str, label: &'static str, dim: bool| {
            // .notif-drawer-btn：padding 3/9、12px、accent、圆角 4；-dim：fg-muted、0.5（hover 1）
            div()
                .id(id)
                .px(px(9.0))
                .py(px(3.0))
                .rounded(px(4.0))
                .text_size(px(12.0))
                .cursor_pointer()
                .text_color(if dim { theme.fg_muted } else { theme.accent })
                .when(dim, |d| d.opacity(0.5).hover(|s| s.opacity(1.0).bg(theme.bg_hover)))
                .when(!dim, |d| d.hover(|s| s.bg(theme.bg_hover)))
                .child(label)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .pt(px(14.0))
            .pb(px(13.0))
            .px(px(16.0))
            .border_b_1()
            .border_color(theme.border)
            .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).text_color(theme.fg).child("通知"))
            .child(div().ml(px(8.0)).flex_1().text_size(px(11.0)).text_color(theme.fg_muted.opacity(0.35)).child("⌘I"))
            .child(
                div()
                    .flex()
                    .gap(px(4.0))
                    .when(unread > 0, |d| {
                        let n = n.clone();
                        d.child(btn("notif-read-all", "全部已读", false).on_click(move |_: &ClickEvent, _, cx| n.update(cx, |n, cx| n.mark_all_read(cx))))
                    })
                    .when(count > 0, |d| {
                        let n = n.clone();
                        d.child(btn("notif-clear-all", "全部清除", true).on_click(move |_: &ClickEvent, _, cx| n.update(cx, |n, cx| n.clear_all(cx))))
                    }),
            )
    };

    let list: AnyElement = if records.is_empty() {
        // App.css:1573-1596 空状态
        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .pb(px(60.0))
            .child(svg().path("icons/notif-empty-bell-off.svg").size(px(36.0)).text_color(theme.fg_muted.opacity(0.18)))
            .child(div().text_size(px(14.0)).font_weight(FontWeight::MEDIUM).text_color(theme.fg.opacity(0.4)).child("暂无通知"))
            .child(div().text_size(px(12.0)).text_color(theme.fg_muted.opacity(0.28)).child("桌面通知将在此处显示。"))
            .into_any_element()
    } else {
        let rows: Vec<AnyElement> = records
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let (name, project) = n.read(cx).names(&r.session_id, cx);
                row(n.clone(), i, r, name, project, selected == Some(i), i + 1 == count, now, &theme)
            })
            .collect();
        // 滚动条是滚动容器的同级、绝对定位（见 scrollbar.rs 文件头）
        div()
            .relative()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .child(div().id("notif-list").flex_1().min_h_0().overflow_y_scroll().track_scroll(&scroll).children(rows))
            .child(scrollbar)
            .into_any_element()
    };

    let on = |f: fn(&mut Notifier, &mut Window, &mut gpui::Context<Notifier>)| {
        let n = n.clone();
        move |window: &mut Window, cx: &mut App| n.update(cx, |x, cx| f(x, window, cx))
    };
    let (next, prev, confirm, dismiss) = (
        on(|x, _, cx| x.move_selection(1, cx)),
        on(|x, _, cx| x.move_selection(-1, cx)),
        on(|x, w, cx| {
            if let Some(i) = x.selected {
                x.activate_row(i, w, cx)
            }
        }),
        on(|x, w, cx| x.close(w, cx)),
    );
    let outside = {
        let n = n.clone();
        move |e: &gpui::MouseDownEvent, window: &mut Window, cx: &mut App| {
            // 点在铃铛上交给铃铛自己的开关（不然这里先关、铃铛再开，等于关不掉）
            if n.read(cx).bell_bounds.is_some_and(|b| b.contains(&e.position)) {
                return;
            }
            n.update(cx, |x, cx| x.close(window, cx));
        }
    };

    let panel = div()
        .id("notif-drawer")
        .key_context("NotificationCenter")
        .track_focus(&focus)
        // 键盘：↑↓ 循环、Enter 已读 + 跳转 + 关、Esc 关（清单 G）。焦点在抽屉上，键不会漏进终端
        .on_action(move |_: &act::SelectNext, w, cx| next(w, cx))
        .on_action(move |_: &act::SelectPrev, w, cx| prev(w, cx))
        .on_action(move |_: &act::Confirm, w, cx| confirm(w, cx))
        .on_action(move |_: &act::Dismiss, w, cx| dismiss(w, cx))
        .on_mouse_down_out(outside)
        .occlude()
        .relative()
        .w(width)
        .h(height)
        .flex()
        .flex_col()
        .overflow_hidden()
        .bg(theme.bg_soft)
        .border_1()
        .border_color(theme.border.opacity(0.7))
        .rounded(px(8.0))
        .shadow(vec![
            BoxShadow { color: theme.var("--shadow-strong"), offset: point(px(0.0), px(12.0)), blur_radius: px(48.0), spread_radius: px(0.0) },
            BoxShadow { color: theme.shadow, offset: point(px(0.0), px(2.0)), blur_radius: px(8.0), spread_radius: px(0.0) },
        ])
        .text_color(theme.fg)
        .font_family(".SystemUIFont")
        .child(header)
        .child(list)
        // notif-drawer-in：0.15s 从左 16px 滑入 + 淡入
        .with_animation("notif-drawer-in", Animation::new(Duration::from_millis(150)), |el, t| {
            let t = 1.0 - (1.0 - t) * (1.0 - t);
            el.opacity(t).left(px(-16.0 * (1.0 - t)))
        });

    deferred(anchored().position(point(left, px(40.0))).child(panel)).with_priority(5).into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn row(n: Entity<Notifier>, i: usize, r: &super::book::Record, name: String, project: String, active: bool, last: bool, now: i64, t: &Theme) -> AnyElement {
    let group = SharedString::from(format!("notif-row-{i}"));
    let read = r.read;
    let color = kind_color(r.kind, t);
    let sid = r.session_id.clone();
    // App.css:1599-1714
    div()
        .id(("notif-row", i))
        .group(group.clone())
        .relative()
        .flex()
        .items_start()
        .gap(px(10.0))
        .px(px(14.0))
        .py(px(11.0))
        .cursor_pointer()
        .when(!last, |d| d.border_b_1().border_color(t.border.opacity(0.5)))
        .when(active, |d| d.bg(mix(t.accent, t.bg_soft, 0.26)).hover(|s| s.bg(mix(t.accent, t.bg_soft, 0.34))))
        .when(!active, |d| d.hover(|s| s.bg(t.bg_hover)))
        // 选中态左侧 2px accent 竖条（CSS 是 inset box-shadow，不挤内容）
        .when(active, |d| d.child(div().absolute().left_0().top_0().bottom_0().w(px(2.0)).bg(t.accent)))
        .child(
            div()
                .flex_none()
                .w(px(8.0))
                .pt(px(5.0))
                .flex()
                .justify_center()
                .when(!read, |d| d.child(div().size(px(7.0)).rounded_full().bg(color))),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .justify_between()
                        .gap(px(4.0))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(t.fg.opacity(if read { 0.45 } else { 1.0 }))
                                .child(name),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(11.0))
                                .text_color(t.fg_muted.opacity(0.4))
                                .child(crate::sidebar::groups::relative_time(r.at / 1000, now)),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .mt(px(2.0))
                        .text_size(px(11.0))
                        .when(!project.is_empty(), |d| {
                            d.child(div().min_w_0().truncate().text_color(t.fg_muted.opacity(if read { 0.25 } else { 0.45 })).child(project))
                        })
                        .child(div().flex_none().text_color(color.opacity(if read { 0.25 } else { 0.8 })).child(r.kind.label())),
                ),
        )
        .child({
            let n = n.clone();
            let sid = sid.clone();
            div()
                .id(("notif-row-clear", i))
                .flex_none()
                .size(px(20.0))
                .mt(px(1.0))
                .rounded(px(4.0))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(15.0))
                .text_color(t.fg_muted)
                .opacity(if active { 0.3 } else { 0.0 })
                .group_hover(group, |s| s.opacity(0.3))
                .hover(|s| s.opacity(1.0).bg(t.bg_hover))
                .child("×")
                .tooltip(crate::tooltip::tip("移除"))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(move |_: &ClickEvent, _, cx| {
                    cx.stop_propagation();
                    n.update(cx, |x, cx| x.remove(&sid, cx));
                })
        })
        .on_click(move |_: &ClickEvent, window, cx| n.update(cx, |x, cx| x.activate_row(i, window, cx)))
        .into_any_element()
}

/// 标题栏铃铛（C 包在标题栏里 `.child(crate::notify::bell(cx))`）。App.css:1413-1434、1494-1510：
/// 24×22、圆角 4、fg-muted、平时 0.5 / 有未读或 hover 时 1；未读只画一个 5px 琥珀点（不显示数字），描一圈 bg-soft
pub fn bell(cx: &App) -> AnyElement {
    let Some(n) = Notifier::try_global(cx) else { return div().into_any_element() };
    let theme = cx.theme().clone();
    let unread = n.read(cx).unread_count();
    let tip: SharedString = if unread > 0 { format!("通知中心 (⌘I) · {unread} 条未读").into() } else { "通知中心 (⌘I)".into() };
    let n_bounds = n.clone();
    let n_click = n.clone();
    div()
        .id("notif-bell")
        .relative()
        .flex_none()
        .w(px(crate::workspace::titlebar::ICON_BTN_W))
        .h(px(crate::workspace::titlebar::ICON_BTN_H))
        .rounded(px(4.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .opacity(if unread > 0 { 1.0 } else { 0.5 })
        .hover(|s| s.opacity(1.0).bg(theme.bg_hover))
        .child(svg().path("icons/notif-bell.svg").size(px(13.0)).text_color(theme.fg_muted))
        .when(unread > 0, |d| {
            // 5px 点 + 1.5px bg-soft 描边 = 8px 圆，右上角 top 3 / right 3（按点的外沿对齐）
            d.child(
                border_px(div(), px(1.5))
                    .absolute()
                    .top(px(1.5))
                    .right(px(1.5))
                    .size(px(8.0))
                    .rounded_full()
                    .border_color(theme.bg_soft)
                    .bg(theme.warning),
            )
        })
        .child(
            canvas(
                move |bounds, _, cx| {
                    n_bounds.update(cx, |x, _| x.bell_bounds = Some(bounds));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .tooltip(crate::tooltip::tip(tip.clone()))
        .on_click(move |_: &ClickEvent, window, cx| n_click.update(cx, |x, cx| x.toggle(window, cx)))
        .into_any_element()
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn mix_matches_css_color_mix() {
        // color-mix(in srgb, #ff0000 26%, #000000) = rgb(66,0,0)
        let red: Hsla = Rgba { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }.into();
        let black: Hsla = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }.into();
        let m = mix(red, black, 0.26).to_rgb();
        assert!((m.r - 0.26).abs() < 1e-3 && m.g.abs() < 1e-3 && (m.a - 1.0).abs() < 1e-6);
    }
}
