//! 全 app 唯一的右键菜单（照搬 `src/ContextMenu.tsx` + `src/menuPosition.ts`）。
//!
//! 外壳负责每个菜单都要做的四件事：屏幕边界翻转（`clamp_menu_position`）、点外面关闭、
//! Esc 关闭、统一长相（`.context-menu` 的尺寸）。调用方只给一个「生成菜单项」的闭包 ——
//! 用法见 `overlays/mod.rs` 文件头。
//!
//! 菜单项**每次渲染都重新算**（闭包存在菜单里，不是存一份快照）：菜单开着的几秒里后台事件
//! 照样在跑，点下去时看到的应该是当下的状态（App.tsx:1302-1306）。

use std::rc::Rc;

use gpui::{px, App, SharedString, Window};

/// 菜单和窗口边缘之间留的空隙（`menuPosition.ts` 的 MARGIN）
pub const MENU_MARGIN: f32 = 8.0;

// ---- 尺寸（`.context-menu` 一族，App.css:2536-2570）----
/// 外圈 padding
pub const MENU_PAD: f32 = 4.0;
/// 最小宽度
pub const MENU_MIN_W: f32 = 150.0;
/// 一项的高度：上下 padding 6 + 行高 14
pub const ITEM_H: f32 = 26.0;
/// 一项的左右 padding
pub const ITEM_PAD_X: f32 = 12.0;
/// 分隔线：1px + 上下各 4px margin
pub const SEP_H: f32 = 9.0;
/// 字号
pub const FONT: f32 = 12.0;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 一个菜单项。disabled 而不是「不显示」：菜单项数量跟着上下文跳的话，用户每次都要重新找
/// 那一项在第几行；灰掉能同时说「有这个功能」和「这次用不了」。
#[derive(Clone)]
pub enum MenuItem {
    Separator,
    Action { label: SharedString, disabled: bool, handler: Handler },
}

impl MenuItem {
    pub fn action(label: impl Into<SharedString>, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        MenuItem::Action { label: label.into(), disabled: false, handler: Rc::new(handler) }
    }

    pub fn separator() -> Self {
        MenuItem::Separator
    }

    /// `MenuItem::action(..).disabled(条件)`
    pub fn disabled(self, yes: bool) -> Self {
        match self {
            MenuItem::Action { label, handler, .. } => MenuItem::Action { label, disabled: yes, handler },
            s => s,
        }
    }
}

/// 生成菜单项的闭包（每帧调一次）
pub type MenuBuilder = Rc<dyn Fn(&App) -> Vec<MenuItem>>;

/// 把「鼠标坐标」修正成「不出窗口的菜单左上角」。
///
/// 顺序有讲究：**先推回来，再压住上 / 左边界**。菜单比视口还高时第一步会算出负数，第二步把它
/// 按回 MARGIN —— 此时菜单底部仍然超出，由 max-height + 滚动接管：滚动比「顶部被切掉」好，
/// 第一项通常是最常用的那个。
pub fn clamp_menu_position(x: f32, y: f32, width: f32, height: f32, vw: f32, vh: f32) -> (f32, f32) {
    let (mut nx, mut ny) = (x, y);
    if nx + width > vw - MENU_MARGIN {
        nx = vw - width - MENU_MARGIN;
    }
    if ny + height > vh - MENU_MARGIN {
        ny = vh - height - MENU_MARGIN;
    }
    if nx < MENU_MARGIN {
        nx = MENU_MARGIN;
    }
    if ny < MENU_MARGIN {
        ny = MENU_MARGIN;
    }
    (nx, ny)
}

/// 菜单外框高度（含外圈 padding 和 1px 边框），用来算落点
pub fn menu_height(items: &[MenuItem]) -> f32 {
    let inner: f32 = items.iter().map(|i| if matches!(i, MenuItem::Separator) { SEP_H } else { ITEM_H }).sum();
    inner + MENU_PAD * 2.0 + 2.0
}

/// 菜单外框宽度：最长一项的文字宽 + 左右 padding，不小于 min-width
pub fn menu_width(max_label_w: f32) -> f32 {
    (max_label_w + ITEM_PAD_X * 2.0).max(MENU_MIN_W) + MENU_PAD * 2.0 + 2.0
}

/// 量一段菜单文字的宽度（界面字体、12px）
pub fn measure(label: &str, window: &mut Window) -> f32 {
    let style = window.text_style();
    let run = gpui::TextRun { len: label.len(), font: style.font(), color: style.color, background_color: None, underline: None, strikethrough: None };
    let line = window.text_system().shape_line(SharedString::from(label.to_string()), px(FONT), &[run], None);
    f32::from(line.width)
}

#[cfg(test)]
mod tests {
    //! 落点修正这条规则原来只有侧栏菜单有，pane 菜单没有 —— 屏幕右下角右键，菜单一半在屏幕外、
    //! 够不着最后一项。测试向量照搬 `scripts/test-context-menu.ts` 的 clampMenuPosition 部分。
    use super::*;

    const VW: f32 = 1000.0;
    const VH: f32 = 800.0;
    const W: f32 = 160.0;
    const H: f32 = 200.0;

    #[test]
    fn clamp_matches_ts_vectors() {
        assert_eq!(clamp_menu_position(300.0, 400.0, W, H, VW, VH), (300.0, 400.0), "屏幕中间：原样不动");
        assert_eq!(clamp_menu_position(900.0, 100.0, W, H, VW, VH), (VW - W - 8.0, 100.0), "贴右边：往左推");
        assert_eq!(clamp_menu_position(300.0, 700.0, W, H, VW, VH), (300.0, VH - H - 8.0), "贴下边：往上推");
        assert_eq!(clamp_menu_position(980.0, 790.0, W, H, VW, VH), (VW - W - 8.0, VH - H - 8.0), "右下角：两个方向都推");
        assert_eq!(
            clamp_menu_position(VW - W - 8.0, VH - H - 8.0, W, H, VW, VH),
            (VW - W - 8.0, VH - H - 8.0),
            "正好留够 8px 空隙：不动"
        );
        assert_eq!(clamp_menu_position(100.0, 300.0, W, 900.0, VW, VH), (100.0, 8.0), "比视口还高：压在上边界而不是负数");
        assert_eq!(clamp_menu_position(100.0, 300.0, 1200.0, H, VW, VH).0, 8.0, "比视口还宽：压在左边界");
        assert_eq!(clamp_menu_position(-50.0, -50.0, W, H, VW, VH), (8.0, 8.0), "鼠标坐标为负：夹回左上");
    }

    #[test]
    fn size_follows_items() {
        let noop = || MenuItem::action("x", |_, _| {});
        let items = vec![noop(), MenuItem::separator(), noop()];
        assert_eq!(menu_height(&items), ITEM_H * 2.0 + SEP_H + MENU_PAD * 2.0 + 2.0);
        assert_eq!(menu_width(10.0), MENU_MIN_W + MENU_PAD * 2.0 + 2.0, "短文字用 min-width");
        assert_eq!(menu_width(200.0), 200.0 + ITEM_PAD_X * 2.0 + MENU_PAD * 2.0 + 2.0);
    }

    #[test]
    fn disabled_builder_keeps_label() {
        let MenuItem::Action { label, disabled, .. } = MenuItem::action("复制", |_, _| {}).disabled(true) else { panic!() };
        assert_eq!(label.as_ref(), "复制");
        assert!(disabled);
        assert!(matches!(MenuItem::separator().disabled(true), MenuItem::Separator));
    }
}
