//! 全 app 唯一的 tooltip（Tauri 版用的是 WebView 原生的 `title` 属性，只有一种长相）。
//!
//! 之前侧栏、标题栏 / 工作区、通知抽屉各写了一份，长相各不一样（字号 11 / 12、边框 border / border-strong、
//! 有的没设字体）—— 鼠标悬停在不同地方，提示框看起来像来自不同的软件。现在只此一份，
//! 用法：`.tooltip(tip("文字"))`，文字可以是 `&str` / `String` / `SharedString`。

use gpui::{div, prelude::*, px, AnyView, App, Context, IntoElement, Render, SharedString, Window};

use crate::theme::ActiveTheme;

pub struct Tip(pub SharedString);

impl Render for Tip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme();
        div()
            .px(px(8.))
            .py(px(4.))
            .bg(t.bg_soft)
            .border_1()
            .border_color(t.border_strong)
            .rounded(px(4.))
            .text_size(px(11.))
            .line_height(px(15.))
            .text_color(t.fg)
            .font_family(".SystemUIFont")
            .child(self.0.clone())
    }
}

/// 给 `.tooltip(...)` 用的构造器
pub fn tip(text: impl Into<SharedString>) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text: SharedString = text.into();
    move |_, cx| cx.new(|_| Tip(text.clone())).into()
}
