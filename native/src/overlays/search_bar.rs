//! ⌘F 终端内搜索条的**外观和输入交互**（照 App.tsx:2271-2322 + App.css `.terminal-search-*`）。
//! 搜索本身（在哪个终端里找、怎么高亮、结果计数）归 A 终端包，通过下面这组接口接进来：
//!
//! ```ignore
//! // A 包启动时装一次
//! overlays::search_bar::set_hooks(SearchHooks {
//!     search: Rc::new(|term, dir, window, cx| { /* 在当前终端里找 term；dir = Incremental / Next / Prev */ }),
//!     clear: Rc::new(|window, cx| { /* 清掉高亮 */ }),
//!     anchor: Rc::new(|cx| /* 当前 pane 在窗口里的矩形，拿不到给 None */ None),
//! }, cx);
//! // 结果变了（含切 tab 时清零）就推一次；None = 还没有结果
//! overlays::search_bar::report_progress(Some(SearchProgress { index: 2, count: 17 }), cx);
//! ```
//! 没装 hooks 时搜索条照样能打开、能输入，只是不搜（开发期各包并行）。

use crate::ts;
use std::rc::Rc;

use gpui::{div, prelude::*, px, svg, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Global, Pixels, Window};

use super::icons;
use super::search_count::{format_search_count, SearchProgress};
use super::style::*;
use super::text_input::{TextInput, TextInputEvent};
use crate::actions::overlays as act;
use crate::theme::ActiveTheme;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchDir {
    /// 输入变化时（从当前位置找第一个）
    Incremental,
    Next,
    Prev,
}

#[derive(Clone)]
pub struct SearchHooks {
    pub search: Rc<dyn Fn(&str, SearchDir, &mut Window, &mut App)>,
    pub clear: Rc<dyn Fn(&mut Window, &mut App)>,
    /// 当前 pane 的窗口坐标矩形（搜索条贴它的右上角）
    pub anchor: Rc<dyn Fn(&App) -> Option<Bounds<Pixels>>>,
}

impl Global for SearchHooks {}

pub fn set_hooks(h: SearchHooks, cx: &mut App) {
    cx.set_global(h);
}

/// A 包推搜索进度（`onDidChangeResults` 的等价物）
pub fn report_progress(p: Option<SearchProgress>, cx: &mut App) {
    if let Some(h) = super::host(cx) {
        let bar = h.read(cx).search_bar();
        if let Some(bar) = bar {
            bar.update(cx, |b, cx| {
                b.progress = p;
                cx.notify();
            });
        }
    }
}

pub enum SearchBarEvent {
    Close,
}

pub struct SearchBar {
    input: Entity<TextInput>,
    progress: Option<SearchProgress>,
}

impl EventEmitter<SearchBarEvent> for SearchBar {}

fn hooks(cx: &App) -> Option<SearchHooks> {
    cx.try_global::<SearchHooks>().cloned()
}

impl SearchBar {
    pub fn new(initial: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new(ts!("search.placeholder_terminal"), cx));
        cx.subscribe_in(&input, window, |this, input, _: &TextInputEvent, window, cx| {
            let term = input.read(cx).text().to_string();
            if let Some(h) = hooks(cx) {
                if term.is_empty() {
                    (h.clear)(window, cx);
                } else {
                    (h.search)(&term, SearchDir::Incremental, window, cx);
                }
            }
            if term.is_empty() {
                this.progress = None;
            }
            cx.notify();
        })
        .detach();
        let bar = Self { input, progress: None };
        if let Some(t) = initial {
            bar.input.update(cx, |i, cx| i.set_text(t, cx));
        }
        bar.focus(window, cx);
        bar
    }

    /// 再按一次 ⌘F：聚焦并全选
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.input.read(cx).focus_handle(cx));
        self.input.update(cx, |i, cx| i.select_all_text(cx));
    }

    pub fn set_text(&self, t: String, cx: &mut App) {
        self.input.update(cx, |i, cx| i.set_text(t, cx));
    }

    pub fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }

    fn go(&mut self, dir: SearchDir, window: &mut Window, cx: &mut Context<Self>) {
        let term = self.input.read(cx).text().to_string();
        if let (Some(h), false) = (hooks(cx), term.is_empty()) {
            (h.search)(&term, dir, window, cx);
        }
    }

    /// 关闭：清高亮、清搜索词（Esc / ×）
    fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(h) = hooks(cx) {
            (h.clear)(window, cx);
        }
        cx.emit(SearchBarEvent::Close);
    }
}

use gpui::Focusable as _;

impl Render for SearchBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let term = self.input.read(cx).text().to_string();
        let count = format_search_count(&term, self.progress);
        let vp = window.viewport_size();
        // 贴当前 pane 的右上角（top+4, right+8）；拿不到时固定位置兜底（top 12, right 24）
        let (top, right) = match hooks(cx).and_then(|h| (h.anchor)(cx)) {
            Some(b) => (b.top() + px(4.0), vp.width - b.right() + px(8.0)),
            None => (px(12.0), px(24.0)),
        };
        let btn = |id: &'static str, glyph: &'static str| {
            let (hover, fg) = (theme.bg_hover, theme.fg);
            div()
                .id(id)
                .px(px(6.0))
                .py(px(2.0))
                .rounded(px(RADIUS_SM))
                .text_size(px(12.0))
                .line_height(px(14.0))
                .text_color(theme.fg_muted)
                .cursor_pointer()
                .hover(move |s| s.bg(hover).text_color(fg))
                .child(glyph)
        };
        div()
            .id("terminal-search-bar")
            .key_context("Overlay TerminalSearch")
            .on_action(cx.listener(|this, _: &act::Dismiss, window, cx| this.close(window, cx)))
            .on_action(cx.listener(|this, _: &act::Confirm, window, cx| this.go(SearchDir::Next, window, cx)))
            .on_action(cx.listener(|this, _: &act::ConfirmReverse, window, cx| this.go(SearchDir::Prev, window, cx)))
            .occlude()
            .absolute()
            .top(top)
            .right(right)
            .flex()
            .items_center()
            .gap(px(4.0))
            .px(px(8.0))
            .py(px(6.0))
            .bg(theme.bg_soft)
            .border_1()
            .border_color(theme.border)
            .rounded(px(RADIUS_MD))
            .shadow(vec![shadow(4.0, 16.0, theme.shadow)])
            .text_color(theme.fg_muted)
            .child(svg().path(icons::SEARCH).size(px(14.0)).flex_none().text_color(theme.fg_muted))
            .child(div().w(px(220.0)).px(px(4.0)).py(px(2.0)).text_size(px(12.0)).line_height(px(16.0)).text_color(theme.fg).flex().child(self.input.clone()))
            // 计数：空串时不渲染（否则 gap 留一道缝）；min-width 5ch 右对齐，数字变化时按钮不跳
            .children(count.map(|c| div().text_size(px(11.0)).text_color(theme.fg_muted).min_w(px(33.0)).pl(px(2.0)).flex().justify_end().whitespace_nowrap().child(c)))
            .child(btn("search-prev", "↑").on_click(cx.listener(|this, _, window, cx| this.go(SearchDir::Prev, window, cx))))
            .child(btn("search-next", "↓").on_click(cx.listener(|this, _, window, cx| this.go(SearchDir::Next, window, cx))))
            .child(btn("search-close", "×").on_click(cx.listener(|this, _, window, cx| this.close(window, cx))))
    }
}
