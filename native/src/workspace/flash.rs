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
    PaneIconSet { id: "cosmos", name: "宇宙", icons: &["🚀", "🛸", "🪐", "🌌", "☄️", "🌋", "🌪️", "⚡️", "🔥"] },
    PaneIconSet { id: "arms", name: "兵器", icons: &["⚔️", "🛡️", "🏹", "🔱", "🪓", "🗡️", "👑", "💎", "🔮"] },
    PaneIconSet { id: "wild", name: "夸张表情", icons: &["🤯", "🥶", "🥵", "😈", "👻", "🤡", "💀", "👽", "🤖"] },
    PaneIconSet { id: "none", name: "不显示", icons: &[] },
];

/// 闪牌总时长。比 TS 版的 700ms 长一点，给「弹出 → 回弹 → 光晕散开」留够时间（#201）
pub const FLASH_MS: u64 = 850;

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

/// 整张牌的透明度随时间（0–1）：按键后**立刻**半透明可见，4% 内（约 35ms）到位，65% 前保持，之后淡出。
/// 旧曲线照 CSS 抄的（10% 才到位，850ms 里就是 85ms 的「慢半拍」），实测用户能感觉到显示延迟（#201）
pub fn card_opacity(t: f32) -> f32 {
    if t < 0.04 {
        0.5 + 0.5 * (t / 0.04)
    } else if t < 0.65 {
        1.0
    } else {
        (1.0 - (t - 0.65) / 0.35).max(0.0)
    }
}

/// 图标的「弹出」缩放（相对基准大小）：从 0.55 冲到 1.14 左右再回弹到 1.0，前 38% 完成，之后保持 1.0。
/// GPUI 的 div 没有 transform scale，调用方拿它去乘字号
pub fn punch_scale(t: f32) -> f32 {
    const END: f32 = 0.38;
    const FROM: f32 = 0.55;
    const C1: f32 = 3.5; // easeOutBack 的过冲系数：峰值约 1.14
    if t >= END {
        return 1.0;
    }
    let u = (t / END).max(0.0) - 1.0;
    let ease = 1.0 + (C1 + 1.0) * u * u * u + C1 * u * u;
    FROM + (1.0 - FROM) * ease
}

/// 卡片外圈光晕：(扩散半径 px, 透明度 0–1)。从卡片边缘向外扩散并淡掉，前 70% 完成
pub fn halo(t: f32) -> (f32, f32) {
    const END: f32 = 0.7;
    const MAX_SPREAD: f32 = 34.0;
    const START_ALPHA: f32 = 0.55;
    let p = (t / END).clamp(0.0, 1.0);
    let out = 1.0 - (1.0 - p) * (1.0 - p); // easeOutQuad：先快后慢地散开
    (MAX_SPREAD * out, START_ALPHA * (1.0 - p))
}

/// 目标 pane 整圈边框强调色闪一下的透明度：开头最亮，约 60% 处淡完
pub fn ring_opacity(t: f32) -> f32 {
    const END: f32 = 0.6;
    const START: f32 = 0.9;
    START * (1.0 - (t / END).clamp(0.0, 1.0))
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

    /// 为什么要测：按键到牌出现的延迟用户能感觉到。淡入的第一帧就得看得见，且很快到位
    #[test]
    fn card_is_visible_at_once_and_fully_shown_within_40ms() {
        assert!(card_opacity(0.0) >= 0.5, "第一帧就半透明可见，不是从 0 开始");
        assert_eq!(card_opacity(0.04), 1.0);
        assert!(0.04 * FLASH_MS as f32 <= 40.0, "淡入到位不超过 40ms");
        assert_eq!(card_opacity(0.5), 1.0);
        assert!((card_opacity(0.825) - 0.5).abs() < 1e-4, "65%→100% 线性淡出，中点一半");
        assert_eq!(card_opacity(1.0), 0.0);
    }

    /// 为什么要测：弹出动画的数值错了，在界面上就是图标一直是 0 号 / 负数字号（消失或崩）、停不回原大小（落点后图标是歪的）、
    /// 或者光晕一直不散。这些在 GPUI 里没有 CSS 兜底，必须自己守住
    #[test]
    fn punch_overshoots_then_settles_exactly_at_one() {
        assert!(punch_scale(0.0) < 0.7, "从小处弹出");
        let peak = (0..=100).map(|i| punch_scale(i as f32 / 100.0)).fold(0.0_f32, f32::max);
        assert!(peak > 1.08 && peak < 1.3, "要有过冲但不能夸张到溢出：{peak}");
        assert_eq!(punch_scale(0.38), 1.0, "38% 起稳定在 1.0");
        assert_eq!(punch_scale(1.0), 1.0);
        assert!((0..=100).all(|i| punch_scale(i as f32 / 100.0) > 0.0), "字号不能是 0 或负数");
    }

    #[test]
    fn halo_expands_and_fades_out() {
        let (r0, a0) = halo(0.0);
        let (r1, a1) = halo(0.35);
        let (r2, a2) = halo(0.7);
        assert!(r0 < r1 && r1 < r2, "半径单调变大");
        assert!(a0 > a1 && a1 > a2, "透明度单调变淡");
        assert_eq!(a2, 0.0, "70% 后完全消失");
        assert!(a0 > 0.3 && a0 <= 1.0, "起点要看得见");
        assert_eq!(halo(1.0), (halo(0.7).0, 0.0), "之后保持散尽的状态");
    }

    #[test]
    fn ring_flashes_bright_then_gone() {
        assert!(ring_opacity(0.0) > 0.6, "开头最亮");
        assert!(ring_opacity(0.3) < ring_opacity(0.0));
        assert_eq!(ring_opacity(0.6), 0.0, "60% 后没了");
        assert_eq!(ring_opacity(1.0), 0.0);
    }

    #[test]
    fn new_icon_sets_exist_and_are_distinct() {
        for id in ["cosmos", "arms", "wild"] {
            assert_eq!(pane_icons_for(id).len(), 9, "{id}");
        }
        let mut all: Vec<&str> = PANE_ICON_SETS.iter().flat_map(|s| s.icons.iter().copied()).collect();
        let n = all.len();
        all.sort();
        all.dedup();
        assert_eq!(all.len(), n, "各套之间不重复，不然换套看不出区别");
    }
}
