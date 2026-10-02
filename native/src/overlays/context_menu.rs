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
/// 小标题一行的高度（11px 字，上 6 下 2）
pub const HEADER_H: f32 = 22.0;
/// ✓ 的槽宽
pub const CHECK_W: f32 = 16.0;
/// ✕（可删除的项）的槽宽
pub const REMOVE_W: f32 = 22.0;
/// 色块预览的宽度（含和标签之间的空隙）
pub const PREVIEW_W: f32 = 52.0;
/// 带搜索框的菜单最高多少（项多时出滚动条，不撑满整个窗口）
pub const SEARCH_MENU_MAX_H: f32 = 400.0;

type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 一个菜单项。disabled 而不是「不显示」：菜单项数量跟着上下文跳的话，用户每次都要重新找
/// 那一项在第几行；灰掉能同时说「有这个功能」和「这次用不了」。
#[derive(Clone)]
pub enum MenuItem {
    Separator,
    /// 不可点的小标题（下拉选择里的「深色 / 浅色 / 导入」分组）
    Header(SharedString),
    /// `checked` 为真时前面画 ✓（下拉选择的当前项）；菜单里只要有一项打了勾，所有项都留出勾的位置
    Action { label: SharedString, disabled: bool, checked: bool, handler: Handler, remove: Option<Handler>, preview: Option<Vec<gpui::Hsla>> },
}

impl MenuItem {
    pub fn action(label: impl Into<SharedString>, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        MenuItem::Action { label: label.into(), disabled: false, checked: false, handler: Rc::new(handler), remove: None, preview: None }
    }

    pub fn header(label: impl Into<SharedString>) -> Self {
        MenuItem::Header(label.into())
    }

    pub fn separator() -> Self {
        MenuItem::Separator
    }

    /// `MenuItem::action(..).disabled(条件)`
    pub fn disabled(mut self, yes: bool) -> Self {
        if let MenuItem::Action { disabled, .. } = &mut self {
            *disabled = yes;
        }
        self
    }

    /// `MenuItem::action(..).checked(是当前值)`
    pub fn checked(mut self, yes: bool) -> Self {
        if let MenuItem::Action { checked, .. } = &mut self {
            *checked = yes;
        }
        self
    }

    /// 这一行右侧加一个 ✕（鼠标移到这一行才显示），点它执行 `handler` 但**不关菜单**——
    /// 要连着删好几项时不用每次重新打开（导入的主题）。菜单项每次渲染重新算，删完列表自己会更新
    pub fn removable(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        if let MenuItem::Action { remove, .. } = &mut self {
            *remove = Some(Rc::new(handler));
        }
        self
    }

    /// 标签前面的小色块预览：第一个颜色是底色，其余画成圆点（主题的底色 + 红绿黄蓝紫青）
    pub fn preview(mut self, colors: Vec<gpui::Hsla>) -> Self {
        if let MenuItem::Action { preview, .. } = &mut self {
            *preview = Some(colors);
        }
        self
    }
}

/// 有没有可删除的项（有就给所有项留 ✕ 的位置，免得菜单宽度跟着鼠标跳）
pub fn has_removable(items: &[MenuItem]) -> bool {
    items.iter().any(|i| matches!(i, MenuItem::Action { remove: Some(_), .. }))
}

/// 搜索框里输入文字后留下的菜单项：不区分大小写的子串匹配（空查询 = 全部）。
/// 分组标题只在它下面还有命中的项时保留；过滤时分隔线一律去掉（分组被挖空后剩下的线没有意义）
pub fn filter_menu_items(items: &[MenuItem], query: &str) -> Vec<MenuItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return items.to_vec();
    }
    let mut out: Vec<MenuItem> = Vec::new();
    // 当前分组的标题先挂着，等到有命中的项时才放进结果
    let mut pending_header: Option<MenuItem> = None;
    for item in items {
        match item {
            MenuItem::Separator => {}
            MenuItem::Header(_) => pending_header = Some(item.clone()),
            MenuItem::Action { label, .. } if label.to_lowercase().contains(&q) => {
                out.extend(pending_header.take());
                out.push(item.clone());
            }
            MenuItem::Action { .. } => {}
        }
    }
    out
}

/// ↑↓ 在可选项里移动一格：`count` 个可选项，到头绕回另一头；没有可选项返回 0
pub fn step_selection(count: usize, sel: usize, delta: isize) -> usize {
    if count == 0 {
        return 0;
    }
    (sel as isize + delta).rem_euclid(count as isize) as usize
}

/// 可选项（没灰掉的 Action）在整个菜单项列表里的下标，↑↓ / 回车按这个走
pub fn selectable(items: &[MenuItem]) -> Vec<usize> {
    items.iter().enumerate().filter(|(_, i)| matches!(i, MenuItem::Action { disabled: false, .. })).map(|(n, _)| n).collect()
}

/// 带搜索框的菜单顶部那一行的高度（输入框 + 下边线）
pub const SEARCH_H: f32 = 34.0;

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
    let inner: f32 = items
        .iter()
        .map(|i| match i {
            MenuItem::Separator => SEP_H,
            MenuItem::Header(_) => HEADER_H,
            MenuItem::Action { .. } => ITEM_H,
        })
        .sum();
    inner + MENU_PAD * 2.0 + 2.0
}

/// 有没有打勾的项（有就给所有项留勾的位置）
pub fn has_checks(items: &[MenuItem]) -> bool {
    items.iter().any(|i| matches!(i, MenuItem::Action { checked: true, .. }))
}

/// 菜单外框宽度：最长一项的文字宽（含勾槽）+ 左右 padding，不小于 min-width
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
        let items = vec![MenuItem::header("组"), noop(), MenuItem::separator(), noop()];
        assert_eq!(menu_height(&items), HEADER_H + ITEM_H * 2.0 + SEP_H + MENU_PAD * 2.0 + 2.0);
        assert!(!has_checks(&items));
        assert!(has_checks(&[noop().checked(true)]));
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

    fn labels(items: &[MenuItem]) -> Vec<String> {
        items
            .iter()
            .map(|i| match i {
                MenuItem::Action { label, .. } => label.to_string(),
                MenuItem::Header(l) => format!("# {l}"),
                MenuItem::Separator => "---".to_string(),
            })
            .collect()
    }

    /// 为什么要测：主题有几十套，错了在界面上就是「搜 night 找不到 Tokyo Night」或「搜完留着一个空的分组标题」
    fn themes() -> Vec<MenuItem> {
        vec![
            MenuItem::header("深色"),
            MenuItem::action("Tokyo Night", |_, _| {}),
            MenuItem::action("Nord", |_, _| {}),
            MenuItem::header("浅色"),
            MenuItem::action("GitHub Light", |_, _| {}),
            MenuItem::separator(),
            MenuItem::header("导入"),
            MenuItem::action("Night Owl", |_, _| {}),
        ]
    }

    #[test]
    fn empty_query_keeps_everything() {
        assert_eq!(labels(&filter_menu_items(&themes(), "")).len(), 8);
        assert_eq!(labels(&filter_menu_items(&themes(), "   ")).len(), 8, "只有空白也算空");
    }

    #[test]
    fn filter_is_case_insensitive_substring_and_keeps_only_non_empty_groups() {
        let out = labels(&filter_menu_items(&themes(), "NIGHT"));
        assert_eq!(out, ["# 深色", "Tokyo Night", "# 导入", "Night Owl"], "浅色分组没有命中，标题和分隔线都去掉");
    }

    #[test]
    fn filter_with_no_match_is_empty() {
        assert!(filter_menu_items(&themes(), "zzz").is_empty());
    }

    #[test]
    fn filter_matches_chinese_labels() {
        let items = vec![MenuItem::action("重建原目录", |_, _| {}), MenuItem::action("取消", |_, _| {})];
        assert_eq!(labels(&filter_menu_items(&items, "目录")), ["重建原目录"]);
    }

    #[test]
    fn selection_wraps_around_both_ends() {
        assert_eq!(step_selection(3, 0, 1), 1);
        assert_eq!(step_selection(3, 2, 1), 0, "最后一项再往下回到第一项");
        assert_eq!(step_selection(3, 0, -1), 2, "第一项再往上回到最后一项");
        assert_eq!(step_selection(0, 0, 1), 0, "没有可选项不越界");
    }

    #[test]
    fn selectable_skips_headers_separators_and_disabled() {
        let items = vec![MenuItem::header("组"), MenuItem::action("a", |_, _| {}), MenuItem::separator(), MenuItem::action("b", |_, _| {}).disabled(true), MenuItem::action("c", |_, _| {})];
        assert_eq!(selectable(&items), [1, 4]);
    }
}
