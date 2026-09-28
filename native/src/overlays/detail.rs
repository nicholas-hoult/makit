//! 会话详情面板（照 App.tsx:1667-1688, 2194-2252 + App.css `.detail-*` / `.msg-*`）。
//! 右侧滑出、占 50vw（最多 720，用户 2026-09-29 反馈原来 70vw/960 太宽），点遮罩 / 「关闭」/ **Esc** 关（#224：Tauri 版没有 Esc）。
//! 消息用 GPUI 的 `list`（变高虚拟列表）：长会话几千条也只排版看得见的那几条。

use std::rc::Rc;

use gpui::{div, list, prelude::*, px, Context, EventEmitter, FocusHandle, FontWeight, ListAlignment, ListState, Window};
use makit_core::sessions::ConversationMessage;
use makit_core::SessionMeta;

use super::detail_logic::{display_order, format_ts, subtitle};
use super::style::*;
use crate::actions::overlays as act;
use crate::theme::ActiveTheme;

pub enum DetailEvent {
    Close,
}

enum Load {
    Loading,
    Error(String),
    Done(Rc<Vec<ConversationMessage>>),
}

pub struct DetailView {
    session: SessionMeta,
    load: Load,
    /// 默认倒序（新 → 旧），不持久化
    reversed: bool,
    list: ListState,
    focus: FocusHandle,
}

impl EventEmitter<DetailEvent> for DetailView {}

impl DetailView {
    pub fn new(session: SessionMeta, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let id = session.session_id.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { makit_core::sessions::read_session_messages(id) }).await;
            let _ = this.update(cx, |d, cx| {
                d.load = match r {
                    Ok(m) => {
                        d.list.reset(m.len());
                        Load::Done(Rc::new(m))
                    }
                    Err(e) => Load::Error(e),
                };
                cx.notify();
            });
        })
        .detach();
        Self { session, load: Load::Loading, reversed: true, list: ListState::new(0, ListAlignment::Top, px(600.0)), focus }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DetailView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let s = &self.session;
        let n = match &self.load {
            Load::Done(m) => m.len(),
            _ => 0,
        };
        let vp = window.viewport_size();
        let mono = MONO;
        let accent_text = theme.var("--accent-text");

        let body: gpui::AnyElement = match &self.load {
            Load::Loading => div().p(px(24.0)).flex().justify_center().text_color(theme.fg_muted).child("加载中…").into_any_element(),
            Load::Error(e) => div().px(px(12.0)).py(px(8.0)).text_color(theme.danger).child(e.clone()).into_any_element(),
            Load::Done(m) if m.is_empty() => div().p(px(24.0)).flex().justify_center().text_color(theme.fg_muted).child("无对话内容").into_any_element(),
            Load::Done(m) => {
                let (msgs, order, th) = (m.clone(), Rc::new(display_order(m.len(), self.reversed)), theme.clone());
                list(self.list.clone(), move |ix, _, _| {
                    let m = &msgs[order[ix]];
                    let user = m.role == "user";
                    div()
                        .pb(px(14.0))
                        .child(
                            div()
                                .px(px(12.0))
                                .py(px(10.0))
                                .rounded(px(RADIUS_MD))
                                .bg(th.bg_soft)
                                .border_l(px(3.0))
                                .border_color(if user { th.var("--accent-text") } else { th.border })
                                .child(
                                    div()
                                        .flex()
                                        .flex_wrap()
                                        .gap(px(10.0))
                                        .mb(px(6.0))
                                        .text_size(px(11.0))
                                        .text_color(th.fg_muted)
                                        .child(div().font_weight(FontWeight::SEMIBOLD).child(if user { "👤 用户" } else { "🤖 助手" }))
                                        .when(!m.timestamp.is_empty(), |d| d.child(format_ts(&m.timestamp)))
                                        .when(!m.tool_uses.is_empty(), |d| {
                                            d.child(div().font_family(mono).text_color(th.var("--accent-text")).child(format!("tool: {}", m.tool_uses.join(", "))))
                                        }),
                                )
                                .when(!m.text.is_empty(), |d| {
                                    d.child(div().text_size(px(12.0)).font_family(mono).line_height(px(18.0)).text_color(th.fg).child(m.text.clone()))
                                }),
                        )
                        .into_any_element()
                })
                .size_full()
                .into_any_element()
            }
        };

        div()
            .id("detail-overlay")
            .key_context("Overlay Detail")
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &act::Dismiss, _, cx| cx.emit(DetailEvent::Close)))
            .absolute()
            .size_full()
            .bg(theme.var("--shadow-strong"))
            .flex()
            .justify_end()
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|_, _, _, cx| cx.emit(DetailEvent::Close)))
            .child(
                div()
                    .id("detail-panel")
                    .occlude()
                    .w((vp.width * 0.5).min(px(720.0)))
                    .h_full()
                    .bg(theme.bg)
                    .flex()
                    .flex_col()
                    .shadow(vec![shadow_xy(-8.0, 0.0, 24.0, theme.shadow)])
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .justify_between()
                            .gap(px(12.0))
                            .px(px(18.0))
                            .py(px(14.0))
                            .border_b_1()
                            .border_color(theme.border)
                            .bg(theme.bg_soft)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(8.0))
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(format!("[{}]", s.short_id))
                                            .when(!s.display_name.is_empty(), |d| d.child(div().font_weight(FontWeight::MEDIUM).text_color(accent_text).child(s.display_name.clone()))),
                                    )
                                    .child(div().text_size(px(11.0)).text_color(theme.fg_muted).mt(px(4.0)).child(subtitle(s, n)))
                                    .child(div().text_size(px(11.0)).text_color(theme.fg_muted).font_family(mono).mt(px(2.0)).child(s.cwd.clone())),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.0))
                                    .flex_none()
                                    .child(btn(&theme, "detail-order", if self.reversed { "↓ 新→旧" } else { "↑ 旧→新" }, false).on_click(cx.listener(|this, _, _, cx| {
                                        this.reversed = !this.reversed;
                                        let n = if let Load::Done(m) = &this.load { m.len() } else { 0 };
                                        this.list.reset(n);
                                        cx.notify();
                                    })))
                                    .child(btn(&theme, "detail-close", "关闭", false).on_click(cx.listener(|_, _, _, cx| cx.emit(DetailEvent::Close)))),
                            ),
                    )
                    .child(div().flex_1().min_h_0().px(px(18.0)).pt(px(14.0)).child(body)),
            )
    }
}
