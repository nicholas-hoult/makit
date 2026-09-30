//! 单个 `Item` 的画法：沿用 Claude Code TUI 的视觉语法（`>` 用户、`●` 回答 / 工具、`⎿` 结果、`✻` 思考），
//! 等宽字体、无气泡。工具调用和思考默认折叠成一行，点开看全文（#231 TRD §13）。

use std::rc::Rc;

use gpui::{div, prelude::*, px, AnyElement, App, Font, Hsla, SharedString, Window};
use makit_core::transcript::{DividerKind, Item, ItemKind, Level, ToolResult};

use super::logic::{format_duration, split_blocks, tool_input_text, tool_summary, TextBlock};
use crate::overlays::style::{mix_alpha, RADIUS};
use crate::theme::Theme;

/// 点折叠行时调用：参数是 Item 的 id
pub type Toggle = Rc<dyn Fn(&str, &mut Window, &mut App)>;

const TEXT_PX: f32 = 12.0;
const LINE_PX: f32 = 18.0;

/// 左边一列符号 + 右边正文
fn row(prefix: &str, color: Hsla, body: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .items_start()
        .gap(px(8.0))
        .child(div().flex_none().w(px(14.0)).text_color(color).child(prefix.to_string()))
        .child(div().flex_1().min_w_0().child(body))
}

fn plain(th: &Theme, text: &str) -> gpui::Div {
    div().text_color(th.fg).child(text.to_string())
}

/// 助手回复：段落照常折行，代码块不折行、超宽横向滚动
fn markdown(item_id: &str, th: &Theme, text: &str) -> gpui::Div {
    let mut col = div().flex().flex_col().gap(px(6.0));
    for (i, b) in split_blocks(text).into_iter().enumerate() {
        col = col.child(match b {
            TextBlock::Para(t) => plain(th, &t).into_any_element(),
            TextBlock::Code { text, .. } => div()
                .id(SharedString::from(format!("code-{item_id}-{i}")))
                .overflow_x_scroll()
                .bg(th.bg_soft)
                .rounded(px(RADIUS))
                .px(px(8.0))
                .py(px(6.0))
                .child(div().whitespace_nowrap().text_color(th.fg).child(text))
                .into_any_element(),
        });
    }
    col
}

fn fold_header(id: &str, th: &Theme, dot: Hsla, label: String, expanded: bool, toggle: &Toggle) -> gpui::Stateful<gpui::Div> {
    let (toggle, item_id) = (toggle.clone(), id.to_string());
    div()
        .id(SharedString::from(format!("fold-{id}")))
        .cursor_pointer()
        .flex()
        .items_start()
        .gap(px(8.0))
        .hover(|s| s.bg(mix_alpha(th.fg, 0.05)))
        .on_click(move |_, window, cx| toggle(&item_id, window, cx))
        .child(div().flex_none().w(px(14.0)).text_color(dot).child("●"))
        .child(div().flex_1().min_w_0().text_color(th.fg).child(label))
        .child(div().flex_none().text_color(th.fg_subtle).child(if expanded { "▾" } else { "▸" }))
}

fn result_block(th: &Theme, r: &ToolResult) -> gpui::Div {
    let color = if r.is_error { th.danger } else { th.fg_muted };
    let mut body = div().flex().flex_col();
    if !r.text.is_empty() {
        body = body.child(div().text_color(color).child(r.text.clone()));
    }
    if r.images > 0 {
        body = body.child(div().text_color(th.fg_subtle).child(format!("[{} 张图片]", r.images)));
    }
    if r.truncated {
        body = body.child(div().text_color(th.fg_subtle).child(format!("…（已截断，共 {} 字节）", r.total_len)));
    }
    if r.text.is_empty() && r.images == 0 {
        body = body.child(div().text_color(th.fg_subtle).child("（无输出）"));
    }
    row("⎿", th.fg_subtle, body)
}

pub fn render_item(item: &Item, expanded: bool, th: &Theme, font: &Font, toggle: &Toggle) -> AnyElement {
    let accent = th.var("--accent-text");
    let body: gpui::Div = match &item.kind {
        ItemKind::User(t) => row(">", accent, plain(th, t)).bg(th.bg_soft).rounded(px(RADIUS)).px(px(8.0)).py(px(6.0)),
        ItemKind::Image { media_type, bytes } => row(">", accent, div().text_color(th.fg_muted).child(format!("[图片 {media_type} · {} KB]", bytes / 1024))),
        ItemKind::Command { name, args } => row(">", accent, div().text_color(th.fg).child(format!("/{}{}", name.trim_start_matches('/'), if args.is_empty() { String::new() } else { format!(" {args}") }))),
        ItemKind::LocalOutput(t) => row("⎿", th.fg_subtle, div().text_color(th.fg_muted).child(t.clone())),
        ItemKind::BashInput(t) => row("!", accent, plain(th, t)),
        ItemKind::Assistant(t) => row("●", th.fg, markdown(&item.id, th, t)),
        ItemKind::Thinking(t) if t.trim().is_empty() => row("✻", th.fg_subtle, div().text_color(th.fg_subtle).child("思考（内容已加密）")),
        ItemKind::Thinking(t) => {
            let head = fold_header(&item.id, th, th.fg_subtle, "思考".into(), expanded, toggle);
            let mut d = div().flex().flex_col().child(head);
            if expanded {
                d = d.child(div().pl(px(22.0)).text_color(th.fg_muted).child(t.clone()));
            }
            d
        }
        ItemKind::ToolCall { name, input, result, .. } => {
            let dot = match result {
                None => th.fg_subtle,
                Some(r) if r.is_error => th.danger,
                Some(_) => th.success,
            };
            let mut d = div().flex().flex_col().child(fold_header(&item.id, th, dot, tool_summary(name, input), expanded, toggle));
            if expanded {
                let mut inner = div().pl(px(22.0)).flex().flex_col().gap(px(4.0));
                let text = tool_input_text(input);
                if !text.is_empty() {
                    inner = inner.child(div().text_color(th.fg_muted).child(text));
                }
                inner = inner.child(match result {
                    Some(r) => result_block(th, r),
                    None => row("⎿", th.fg_subtle, div().text_color(th.fg_subtle).child("（没有结果：进行中或被打断）")),
                });
                d = d.child(inner);
            }
            d
        }
        ItemKind::Notice { level, text } => {
            let c = match level {
                Level::Info => th.fg_muted,
                Level::Warn => th.warning,
                Level::Error => th.danger,
            };
            row("※", c, div().text_color(c).child(text.clone()))
        }
        ItemKind::Divider(DividerKind::Compacted) => div().flex().justify_center().py(px(4.0)).text_color(th.fg_subtle).child("──── 对话已压缩 ────"),
        ItemKind::TurnDuration(ms) => row("✻", th.fg_subtle, div().text_color(th.fg_subtle).child(format!("用时 {}", format_duration(*ms)))),
    };
    div()
        .pb(px(10.0))
        .font(font.clone())
        .text_size(px(TEXT_PX))
        .line_height(px(LINE_PX))
        .child(body)
        .into_any_element()
}
