//! 浮层共用的尺寸令牌和小部件（按钮、键帽、勾选框……）。数值一律照 `src/App.css`，
//! 注释里写着对应的 CSS 选择器，改的时候两边对得上。颜色全部从 `cx.theme()` 取。

use gpui::{div, point, prelude::*, px, BoxShadow, Div, Hsla, Pixels, SharedString};

use crate::theme::Theme;

/// `--radius-sm`：徽章、键帽
pub const RADIUS_SM: f32 = 3.0;
/// `--radius`：按钮、输入框、列表项
pub const RADIUS: f32 = 4.0;
/// `--radius-md`：浮层、菜单
pub const RADIUS_MD: f32 = 6.0;
/// `--radius-lg`：命令面板、模态框
pub const RADIUS_LG: f32 = 8.0;

/// 界面等宽字体（CSS `ui-monospace, SFMono-Regular, Menlo, monospace`）
pub const MONO: &str = "Menlo";

/// `box-shadow: 0 <y>px <blur>px <color>`
pub fn shadow(y: f32, blur: f32, color: Hsla) -> BoxShadow {
    BoxShadow { color, offset: point(px(0.0), px(y)), blur_radius: px(blur), spread_radius: px(0.0) }
}

/// 带 x 偏移的影子（详情面板从右侧滑出：`-8px 0 24px`）
pub fn shadow_xy(x: f32, y: f32, blur: f32, color: Hsla) -> BoxShadow {
    BoxShadow { color, offset: point(px(x), px(y)), blur_radius: px(blur), spread_radius: px(0.0) }
}

/// CSS `color-mix(in srgb, <c> <pct>%, transparent)`：同色、按比例降不透明度
pub fn mix_alpha(c: Hsla, pct: f32) -> Hsla {
    Hsla { a: c.a * pct, ..c }
}

/// 键帽（`.palette-footer kbd` / `.settings-shortcuts kbd`）
pub fn kbd(theme: &Theme, text: impl Into<SharedString>, fg: Hsla, pad_x: f32) -> Div {
    div()
        .font_family(MONO)
        .text_size(px(10.0))
        .line_height(px(14.0))
        .bg(theme.bg)
        .text_color(fg)
        .px(px(pad_x))
        .py(px(1.0))
        .rounded(px(RADIUS_SM))
        .border_1()
        .border_color(theme.border)
        .child(text.into())
}

/// `.btn`：5px 12px、12px 字、--bg 底、--border 边；disabled 半透明
pub fn btn(theme: &Theme, id: impl Into<SharedString>, label: impl Into<SharedString>, disabled: bool) -> gpui::Stateful<Div> {
    let el = div()
        .id(gpui::ElementId::Name(id.into()))
        .px(px(12.0))
        .py(px(5.0))
        .text_size(px(12.0))
        .line_height(px(15.0))
        .bg(theme.bg)
        .text_color(theme.fg)
        .border_1()
        .border_color(theme.border)
        .rounded(px(RADIUS))
        .flex_none()
        .child(label.into());
    if disabled {
        el.opacity(0.5)
    } else {
        let hover = theme.bg_hover;
        el.cursor_pointer().hover(move |s| s.bg(hover))
    }
}

/// `.settings-action-btn` / `.settings-file-btn`：--bg-hover 底，hover 变 --bg-active
pub fn action_btn(theme: &Theme, id: impl Into<SharedString>, label: impl Into<SharedString>) -> gpui::Stateful<Div> {
    let active = theme.bg_active;
    div()
        .id(gpui::ElementId::Name(id.into()))
        .px(px(10.0))
        .py(px(5.0))
        .text_size(px(12.0))
        .line_height(px(15.0))
        .bg(theme.bg_hover)
        .text_color(theme.fg)
        .border_1()
        .border_color(theme.border)
        .rounded(px(RADIUS))
        .cursor_pointer()
        .flex_none()
        .hover(move |s| s.bg(active))
        .child(label.into())
}

/// 方形图标钮（`.palette-close` / `.palette-filter-header button`：22×22、--fg-subtle，hover 变 --fg + --bg-hover）
pub fn icon_btn(theme: &Theme, id: impl Into<SharedString>, glyph: &'static str, size: f32) -> gpui::Stateful<Div> {
    let (fg, hover_bg) = (theme.fg, theme.bg_hover);
    div()
        .id(gpui::ElementId::Name(id.into()))
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .text_color(theme.fg_subtle)
        .text_size(px(16.0))
        .line_height(px(16.0))
        .cursor_pointer()
        .hover(move |s| s.text_color(fg).bg(hover_bg))
        .child(glyph)
}

/// 勾选框 / 单选钮。原生控件交给系统画、`accent-color` 用主题色；这里自己画，勾选态 = accent 底。
/// `size` 13（筛选栏）或 16（设置页 `.settings-toggle-row`）
pub fn check_box(theme: &Theme, checked: bool, radio: bool, size: f32) -> Div {
    let r: Pixels = if radio { px(size / 2.0) } else { px(3.0) };
    let el = div().size(px(size)).flex_none().rounded(r).flex().items_center().justify_center();
    if checked {
        let mark = if radio {
            div().size(px(size * 0.4)).rounded(px(size)).bg(theme.accent_fg)
        } else {
            div().text_size(px(size * 0.75)).line_height(px(size)).text_color(theme.accent_fg).child("✓")
        };
        el.bg(theme.accent).child(mark)
    } else {
        el.bg(theme.bg).border_1().border_color(theme.border_strong)
    }
}
