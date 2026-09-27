//! pane 落点闪牌（⌥⌘方向键 / ⌥⌘数字 / 从通知跳转）：目标 pane 中央浮出「图标 + 名字」的药丸，0.7s 淡出。
//!
//! 照搬 `src/paneIcons.ts`（图标套）和 `App.tsx` 的 `flashContainer`：
//! - 图标**按 pane 序号固定**（序号就是 ⌥⌘N 的 N-1）：第 1 个 pane 永远是这套里的第 1 个图标；
//!   超过 9 个 pane 没有对应的数字键，图标留空（宁可不给，也不给一个和 ⌥⌘N 对不上的）；「不显示」那套同理
//! - 名字取 active 标签的完整标题，没有就是 "pane"
//! - 连续触发直接替换（不排队）
//!
//! 为什么单独测：序号和图标一旦错位，图标就从「pane 的身份」退化成装饰噪点，而且是静默错的。

/// 一套 pane 图标（九个同类，对应 ⌥⌘1–⌥⌘9）
pub struct PaneIconSet {
    pub id: &'static str,
    pub name: &'static str,
    /// 空数组 = 不显示图标
    pub icons: &'static [&'static str],
}

pub const PANE_ICON_SETS: &[PaneIconSet] = &[
    PaneIconSet { id: "beasts", name: "灵兽", icons: &["🐉", "🐯", "🦊", "🐳", "🦉", "🐝", "🦄", "🐙", "🐺"] },
    PaneIconSet { id: "flowers", name: "花木", icons: &["🌸", "🌹", "🌻", "🌷", "🌺", "🌼", "🪷", "💐", "🌾"] },
    PaneIconSet { id: "fruits", name: "果园", icons: &["🍎", "🍊", "🍋", "🍇", "🍓", "🍑", "🥝", "🍒", "🥭"] },
    PaneIconSet { id: "dots", name: "色点", icons: &["🔴", "🟠", "🟡", "🟢", "🔵", "🟣", "🟤", "⚫️", "⚪️"] },
    PaneIconSet { id: "none", name: "不显示", icons: &[] },
];

/// 闪牌总时长（App.css `pane-flash-in 700ms`）
pub const FLASH_MS: u64 = 700;

/// 按 id 取图标套，未知 id 回退到第一套（灵兽）
pub fn pane_icons_for(id: &str) -> &'static [&'static str] {
    PANE_ICON_SETS.iter().find(|s| s.id == id).unwrap_or(&PANE_ICON_SETS[0]).icons
}

/// 第 `pane_index` 个 pane（从 0 数）的图标
pub fn flash_icon(set_id: &str, pane_index: usize) -> Option<&'static str> {
    pane_icons_for(set_id).get(pane_index).copied()
}

/// 牌上的名字：active 标签的标题，空了就是 "pane"
pub fn flash_name(active_tab_title: Option<&str>) -> String {
    match active_tab_title.map(str::trim) {
        Some(t) if !t.is_empty() => t.to_string(),
        _ => "pane".into(),
    }
}

/// 整张牌的透明度随时间（0–1）：10% 淡入到位、65% 前保持、之后淡出（`@keyframes pane-flash-in`）
pub fn card_opacity(t: f32) -> f32 {
    if t < 0.10 {
        t / 0.10
    } else if t < 0.65 {
        1.0
    } else {
        (1.0 - (t - 0.65) / 0.35).max(0.0)
    }
}

/// 整张牌的缩放：0% 时 0.96，10% 到 1（`pane-flash-in` 的 scale）
pub fn card_scale(t: f32) -> f32 {
    if t < 0.10 {
        0.96 + 0.04 * (t / 0.10)
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_follows_pane_index() {
        assert_eq!(flash_icon("beasts", 0), Some("🐉"), "第 1 个 pane = 第 1 个图标");
        assert_eq!(flash_icon("beasts", 8), Some("🐺"));
        assert_eq!(flash_icon("fruits", 1), Some("🍊"));
    }

    #[test]
    fn no_icon_beyond_nine_panes_or_for_none() {
        assert_eq!(flash_icon("beasts", 9), None, "超过 9 个 pane 没有 ⌥⌘N，图标留空");
        assert_eq!(flash_icon("none", 0), None, "「不显示」那套是空数组");
    }

    #[test]
    fn unknown_set_falls_back_to_beasts() {
        assert_eq!(flash_icon("没有这套", 0), Some("🐉"));
    }

    #[test]
    fn every_set_has_nine_or_zero() {
        for s in PANE_ICON_SETS {
            assert!(s.icons.len() == 9 || s.icons.is_empty(), "{} 应该是 9 个（对应 ⌥⌘1–9）", s.id);
        }
    }

    #[test]
    fn name_falls_back_to_pane() {
        assert_eq!(flash_name(Some("makit · main")), "makit · main");
        assert_eq!(flash_name(Some("  ")), "pane");
        assert_eq!(flash_name(None), "pane");
    }

    #[test]
    fn keyframes_match_the_css() {
        assert_eq!(card_opacity(0.0), 0.0);
        assert_eq!(card_opacity(0.10), 1.0);
        assert_eq!(card_opacity(0.5), 1.0);
        assert!((card_opacity(0.825) - 0.5).abs() < 1e-4, "65%→100% 线性淡出，中点一半");
        assert_eq!(card_opacity(1.0), 0.0);
        assert!((card_scale(0.0) - 0.96).abs() < 1e-6);
        assert_eq!(card_scale(0.2), 1.0);
    }
}
