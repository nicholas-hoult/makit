//! 窗口怎么摆（#256 A7）：首次大小不超过屏幕、记住上次的位置和大小，还原时按**当前**显示器校正。
//!
//! 为什么单独测：这里算错了，用户就会看到窗口一半在屏幕外、拔掉外接屏后窗口找不到、小屏上窗口比屏幕还大——
//! 而且这些都发生在「用户第一次打开 / 换了显示器」的时候，肉眼回归很难覆盖。所以纯几何放在这里，GPUI 的接线在 `app.rs`。
//! `MAKIT_DISPLAY=1280x800` 可以在 dev 里覆盖显示器列表（见 `parse_displays`），不用真换屏或拔线。

use crate::persist::state::SavedWindow;

/// 默认窗口大小（大屏上的上限）和最小大小
pub const DEFAULT_W: f32 = 1400.0;
pub const DEFAULT_H: f32 = 900.0;
pub const MIN_W: f32 = 900.0;
pub const MIN_H: f32 = 560.0;
/// 默认大小占屏幕的比例（GPUI 给的是整屏，不扣菜单栏和 Dock，所以要留余量）
const W_RATIO: f32 = 0.92;
const H_RATIO: f32 = 0.88;
/// 窗口顶边至少离显示器顶边这么远（菜单栏）
const MENU_BAR: f32 = 40.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub rect: Rect,
    pub maximized: bool,
}

/// 没有保存过时的大小：1400×900，但小屏上收缩到屏幕的 92% × 88%；也不小于最小尺寸（除非屏幕本身更小）
pub fn default_size(display: Rect) -> (f32, f32) {
    // 比例给出的大小，不超过 1400×900；但够放最小尺寸时不低于它；再不超出屏幕（顶部留出菜单栏）
    let usable_h = (display.h - MENU_BAR).max(1.0);
    let w = (display.w * W_RATIO).min(DEFAULT_W).max(MIN_W.min(display.w)).min(display.w);
    let h = (display.h * H_RATIO).min(DEFAULT_H).max(MIN_H.min(usable_h)).min(usable_h);
    (w, h)
}

/// 在显示器可用区域里居中放一个 `w`×`h` 的窗口（顶部让出菜单栏）
fn centered_in(display: Rect, w: f32, h: f32) -> Rect {
    Rect { x: display.x + (display.w - w) / 2.0, y: display.y + MENU_BAR + (display.h - MENU_BAR - h) / 2.0, w, h }
}

/// 把保存的窗口校正到当前的显示器上。`displays[0]` 是主屏
pub fn fit_window(saved: Option<&SavedWindow>, displays: &[Rect]) -> Placement {
    let primary = displays.first().copied().unwrap_or(Rect { x: 0.0, y: 0.0, w: 1440.0, h: 900.0 });
    // 保存的值不可信：任何一个不是有限数、或宽高不是正数 → 当没保存过
    let valid = saved.filter(|s| [s.x, s.y, s.w, s.h].iter().all(|v| v.is_finite()) && s.w > 0.0 && s.h > 0.0);
    let Some(s) = valid else {
        let (w, h) = default_size(primary);
        return Placement { rect: centered_in(primary, w, h), maximized: false };
    };
    // 窗口中心落在哪块屏上；都不在（外接屏拔了）→ 挪到主屏居中
    let (cx, cy) = (s.x + s.w / 2.0, s.y + s.h / 2.0);
    let found = displays.iter().find(|d| d.contains(cx, cy)).copied();
    let disp = found.unwrap_or(primary);
    let usable_h = (disp.h - MENU_BAR).max(1.0);
    let w = s.w.max(MIN_W.min(disp.w)).min(disp.w);
    let h = s.h.max(MIN_H.min(usable_h)).min(usable_h);
    let rect = if found.is_some() {
        // 位置拉回屏内（负坐标是合法的：外接屏在主屏左边）
        Rect { x: s.x.clamp(disp.x, disp.x + disp.w - w), y: s.y.clamp(disp.y + MENU_BAR, disp.y + disp.h - h), w, h }
    } else {
        centered_in(disp, w, h)
    };
    Placement { rect, maximized: s.maximized }
}

/// `MAKIT_DISPLAY` 的值：`1280x800` 或 `1280x800,1920x1080@-1920,0`（`@x,y` 是这块屏左上角的全局坐标）。解析不了返回 None
pub fn parse_displays(value: &str) -> Option<Vec<Rect>> {
    let parts: Vec<String> = value.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
    let size = |s: &str| -> Option<(f32, f32)> {
        let (w, h) = s.split_once('x')?;
        let (w, h) = (w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?);
        (w > 0.0 && h > 0.0).then_some((w, h))
    };
    let mut out = Vec::new();
    let mut i = 0;
    while i < parts.len() {
        // `WxH@x,y`：坐标里的逗号会被上面拆开，所以带 @ 的要把下一段当 y
        if let Some((sz, x)) = parts[i].split_once('@') {
            let (w, h) = size(sz)?;
            let x = x.trim().parse::<f32>().ok()?;
            let y = parts.get(i + 1)?.parse::<f32>().ok()?;
            out.push(Rect { x, y, w, h });
            i += 2;
        } else {
            let (w, h) = size(&parts[i])?;
            out.push(Rect { x: 0.0, y: 0.0, w, h });
            i += 1;
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }
    fn saved(x: f32, y: f32, w: f32, h: f32, maximized: bool) -> SavedWindow {
        SavedWindow { x, y, w, h, maximized }
    }
    fn inside(r: &Rect, screen: &Rect) -> bool {
        r.x >= screen.x && r.y >= screen.y + MENU_BAR - 0.01 && r.x + r.w <= screen.x + screen.w + 0.01 && r.y + r.h <= screen.y + screen.h + 0.01
    }
    const BIG: Rect = Rect { x: 0.0, y: 0.0, w: 2560.0, h: 1440.0 };
    const SMALL: Rect = Rect { x: 0.0, y: 0.0, w: 1280.0, h: 800.0 };

    // ---- 默认大小 ----

    /// 为什么要测：小屏上窗口比屏幕还大，用户够不到下边的输入框和标题栏之外的东西
    #[test]
    fn default_size_shrinks_on_small_screens_and_caps_on_big_ones() {
        assert_eq!(default_size(BIG), (DEFAULT_W, DEFAULT_H), "大屏仍是 1400×900");
        let (w, h) = default_size(SMALL);
        assert!((w - 1280.0 * 0.92).abs() < 0.5 && (h - 800.0 * 0.88).abs() < 0.5, "小屏按比例：{w}×{h}");
        assert!(w <= SMALL.w && h <= SMALL.h);
    }

    #[test]
    fn default_size_never_exceeds_a_tiny_screen_even_below_the_minimum() {
        let tiny = d(0.0, 0.0, 800.0, 500.0);
        let (w, h) = default_size(tiny);
        assert!(w <= 800.0 && h <= 500.0, "屏幕比最小尺寸还小：宁可小于最小尺寸也不超出屏幕：{w}×{h}");
        let mid = d(0.0, 0.0, 1024.0, 640.0);
        let (w, h) = default_size(mid);
        assert!(w >= MIN_W && h >= MIN_H, "够放最小尺寸时不低于它：{w}×{h}");
    }

    // ---- fit_window ----

    #[test]
    fn nothing_saved_on_a_small_screen_gives_a_shrunk_centered_window() {
        let p = fit_window(None, &[SMALL]);
        assert!(inside(&p.rect, &SMALL), "{p:?}");
        assert!(!p.maximized);
        let (w, h) = default_size(SMALL);
        assert!((p.rect.w - w).abs() < 0.5 && (p.rect.h - h).abs() < 0.5);
        assert!((p.rect.x - (SMALL.w - p.rect.w) / 2.0).abs() < 0.5, "水平居中");
    }

    #[test]
    fn nothing_saved_on_a_big_screen_is_the_default_size() {
        let p = fit_window(None, &[BIG]);
        assert_eq!((p.rect.w, p.rect.h), (DEFAULT_W, DEFAULT_H));
        assert!(inside(&p.rect, &BIG));
    }

    /// 为什么要测：外接屏拔掉后，保存的位置在一块不存在的屏上，窗口会开在屏幕外、用户找不到
    #[test]
    fn saved_on_a_disconnected_display_moves_to_the_primary() {
        let s = saved(3000.0, 200.0, 1200.0, 800.0, false); // 原来在右边的外接屏上
        let p = fit_window(Some(&s), &[BIG]);
        assert!(inside(&p.rect, &BIG), "{p:?}");
        assert_eq!((p.rect.w, p.rect.h), (1200.0, 800.0), "尺寸保留");
    }

    #[test]
    fn saved_larger_than_the_display_is_clamped_inside() {
        let s = saved(0.0, 0.0, 1400.0, 900.0, false);
        let p = fit_window(Some(&s), &[SMALL]);
        assert!(inside(&p.rect, &SMALL), "{p:?}");
        assert!(p.rect.w <= SMALL.w && p.rect.h <= SMALL.h);
    }

    #[test]
    fn saved_with_only_a_corner_on_screen_is_pulled_back() {
        let s = saved(1000.0, 700.0, 1200.0, 800.0, false); // 大半在屏幕右下外面
        let p = fit_window(Some(&s), &[BIG]);
        assert!(inside(&p.rect, &BIG), "{p:?}");
        let s2 = saved(-900.0, -300.0, 1200.0, 800.0, false); // 左上角越界，中心在屏外
        let p2 = fit_window(Some(&s2), &[BIG]);
        assert!(inside(&p2.rect, &BIG), "{p2:?}");
    }

    /// 为什么要测：外接屏在主屏左边时坐标是负的。把负坐标当成「越界」拉回主屏，用户每次启动窗口都跑回主屏
    #[test]
    fn negative_coordinates_on_a_left_display_are_valid() {
        let left = d(-1920.0, 0.0, 1920.0, 1080.0);
        let s = saved(-1700.0, 100.0, 1200.0, 800.0, false);
        let p = fit_window(Some(&s), &[BIG, left]);
        assert_eq!((p.rect.x, p.rect.y, p.rect.w, p.rect.h), (-1700.0, 100.0, 1200.0, 800.0), "原样保留在左边那块屏上");
    }

    #[test]
    fn window_on_the_second_display_stays_there() {
        let right = d(2560.0, 0.0, 1920.0, 1080.0);
        let s = saved(2700.0, 120.0, 1000.0, 700.0, false);
        let p = fit_window(Some(&s), &[BIG, right]);
        assert_eq!((p.rect.x, p.rect.y), (2700.0, 120.0));
        assert!(inside(&p.rect, &right));
    }

    #[test]
    fn maximized_flag_and_restore_size_are_kept() {
        let s = saved(100.0, 80.0, 1100.0, 700.0, true);
        let p = fit_window(Some(&s), &[BIG]);
        assert!(p.maximized, "最大化保留");
        assert_eq!((p.rect.w, p.rect.h), (1100.0, 700.0), "rect 是还原大小");
    }

    #[test]
    fn zero_or_garbage_sizes_fall_back_to_the_default() {
        for s in [saved(0.0, 0.0, 0.0, 0.0, false), saved(10.0, 10.0, -5.0, 300.0, false), saved(f32::NAN, 0.0, 800.0, 600.0, false), saved(0.0, 0.0, f32::INFINITY, 600.0, false)] {
            let p = fit_window(Some(&s), &[BIG]);
            assert_eq!((p.rect.w, p.rect.h), (DEFAULT_W, DEFAULT_H), "{s:?} → {p:?}");
            assert!(inside(&p.rect, &BIG));
        }
    }

    #[test]
    fn tiny_saved_size_is_raised_to_the_minimum() {
        let s = saved(100.0, 100.0, 300.0, 200.0, false);
        let p = fit_window(Some(&s), &[BIG]);
        assert_eq!((p.rect.w, p.rect.h), (MIN_W, MIN_H));
    }

    #[test]
    fn no_displays_at_all_still_returns_something_sane() {
        let p = fit_window(None, &[]);
        assert!(p.rect.w > 0.0 && p.rect.h > 0.0);
    }

    // ---- MAKIT_DISPLAY ----

    #[test]
    fn display_override_parsing() {
        assert_eq!(parse_displays("1280x800"), Some(vec![d(0.0, 0.0, 1280.0, 800.0)]));
        assert_eq!(parse_displays("1280x800,1920x1080@-1920,0"), Some(vec![d(0.0, 0.0, 1280.0, 800.0), d(-1920.0, 0.0, 1920.0, 1080.0)]));
        assert_eq!(parse_displays(" 1280X800 "), Some(vec![d(0.0, 0.0, 1280.0, 800.0)]), "忽略空白和大小写");
        for bad in ["", "乱写", "1280", "0x800", "1280x-5", "1280x800@1"] {
            assert_eq!(parse_displays(bad), None, "{bad:?}");
        }
    }
}
