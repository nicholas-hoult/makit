//! 分割线拖动（`WorkspaceView.tsx` 的 `SplitResizer`）的纯逻辑 + 给终端看的「正在拖分割线」标记。
//!
//! 为什么单独测：ratio 算错 = 拖的时候线不跟手或者跳；命中区算错 = 分割线抓不住或者压住标签条。

use super::model::{Dir, Rect};

/// 命中区厚度：分割线本身 0 宽（RESIZER_PX = 0），给一个横跨边界的 10px 抓取盒
pub const HIT_PX: f64 = 10.0;
pub const MIN_RATIO: f64 = 0.1;
pub const MAX_RATIO: f64 = 0.9;

/// 拖动中的新 ratio：起点 ratio + 位移 / (外框尺寸 - 6)，限制在 0.1–0.9。
/// `- 6` 照搬 TS（历史上分割线占 6px 时留下的，保持同样的手感）
pub fn drag_ratio(start_ratio: f64, start_pos: f64, cur_pos: f64, outer_size: f64) -> f64 {
    let total = (outer_size - 6.0).max(1.0);
    (start_ratio + (cur_pos - start_pos) / total).clamp(MIN_RATIO, MAX_RATIO)
}

/// 分割线的抓取盒（工作区坐标）：左右分时是一条 10px 宽的竖条，上下分时是 10px 高的横条，都以边界为中线
pub fn hit_rect(dir: Dir, line: Rect) -> Rect {
    match dir {
        Dir::V => Rect { x: line.x - HIT_PX / 2.0, y: line.y, width: HIT_PX, height: line.height },
        Dir::H => Rect { x: line.x, y: line.y - HIT_PX / 2.0, width: line.width, height: HIT_PX },
    }
}

/// 「正在拖分割线」（#203 行列分离）。拖动中置 true，松手置 false 并 `cx.refresh_windows()`。
///
/// **给 A 终端包的接口**：终端在 prepaint 算出新行列数时读 `cx.try_global::<PaneResizing>()`：
/// 为 true 时行数立即跟、列数按 33ms 节奏跟（`resizePlan.ts` 的 `planResize` + `createColsFollower`）；
/// 变回 false 的那一帧（松手）把延后的列数立即做掉（= TS 的 `flushResize`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaneResizing(pub bool);
impl gpui::Global for PaneResizing {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ratio_follows_the_mouse_and_clamps() {
        // 外框 1006px → 有效 1000px，右移 100px = +0.1
        assert!((drag_ratio(0.5, 300.0, 400.0, 1006.0) - 0.6).abs() < 1e-9);
        assert!((drag_ratio(0.5, 300.0, 200.0, 1006.0) - 0.4).abs() < 1e-9);
        assert_eq!(drag_ratio(0.5, 0.0, 5000.0, 1006.0), 0.9, "上限 0.9");
        assert_eq!(drag_ratio(0.5, 5000.0, 0.0, 1006.0), 0.1, "下限 0.1");
        assert_eq!(drag_ratio(0.5, 10.0, 10.0, 1006.0), 0.5, "没动就不变");
    }

    #[test]
    fn tiny_outer_size_does_not_divide_by_zero() {
        let r = drag_ratio(0.5, 0.0, 3.0, 4.0);
        assert!(r.is_finite() && (MIN_RATIO..=MAX_RATIO).contains(&r));
    }

    #[test]
    fn hit_rect_straddles_the_boundary() {
        let v = hit_rect(Dir::V, Rect { x: 500.0, y: 0.0, width: 0.0, height: 800.0 });
        assert_eq!(v, Rect { x: 495.0, y: 0.0, width: 10.0, height: 800.0 });
        let h = hit_rect(Dir::H, Rect { x: 0.0, y: 300.0, width: 600.0, height: 0.0 });
        assert_eq!(h, Rect { x: 0.0, y: 295.0, width: 600.0, height: 10.0 });
    }
}
