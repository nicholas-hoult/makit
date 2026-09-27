//! 滚轮换算，照原生终端 对标终端 的做法（#189，TS 版 `src/terminalWheel.ts`）。
//!
//! 照搬三件事：
//! 1. 「越用力越快」由 macOS 自己给 —— 系统送来的位移已经按手速加速过，终端不再做非线性加速；
//! 2. 精确滚动（触控板，含惯性阶段）位移 × 2 —— 对标终端 `SurfaceView_AppKit.swift` 的做法；
//! 3. 像素 → 行：对标终端 `Surface.zig` scrollCallback —— 累积到满一行才滚，截断取整，余数留给下一次，
//!    方向一反丢掉旧余数。
//! 离散滚轮（鼠标一格一格）每格 3 行，同 对标终端 `discrete:3`（也是 xterm 的默认）。
//!
//! 设备区分：WebView 里单个事件分不清「触控板正好 40px」和「鼠标 1 格」，TS 版要靠 `WheelClassifier`
//! 看事件流上下文猜；原生这边 NSEvent 自带 `hasPreciseScrollingDeltas`，GPUI 已经分成
//! `ScrollDelta::Pixels`（精确）和 `ScrollDelta::Lines`（离散），不用再猜 —— 所以没有移植分类器。
//!
//! 符号：这里的正数 = 往上看历史（和 GPUI 的 `delta.y` 同号：手指往下滑是正），
//! 和 alacritty 的 `Scroll::Delta` 同号，结果可以直接喂进去。
//!
//! 为什么单独测：慢慢推时一格格走、不丢也不多，全靠这几条边界；错了在手感上一眼就能感觉到，
//! 但界面上没法断言。

/// 精确滚动的固定放大（同 对标终端）
pub const PRECISE_BOOST: f32 = 2.0;
/// 离散滚轮每格滚几行（同 对标终端 `discrete:3`、xterm 默认）
pub const DISCRETE_LINES: f32 = 3.0;

/// 对标终端 的精确滚动换算。`speed` 是额外的滚动速度倍率（默认 1），留给以后的设置项。
/// 返回 (这次要滚的行数, 留给下一次的余数像素)。
pub fn precise_pixels_to_rows(pending: f32, delta_px: f32, cell_h: f32, speed: f32) -> (i32, f32) {
    if cell_h <= 0.0 {
        return (0, 0.0);
    }
    let px = delta_px * PRECISE_BOOST * speed;
    // 方向一反丢掉旧余数（对标终端 Surface.zig）
    let carried = if pending != 0.0 && pending.signum() != px.signum() { 0.0 } else { pending };
    let total = carried + px;
    let rows = (total / cell_h).trunc();
    (rows as i32, total - rows * cell_h)
}

/// 离散滚轮：`lines` 是 NSEvent 给的格数（可能被系统加速成小数 / 多格），× 3 后同样累积取整
pub fn discrete_lines_to_rows(pending: f32, lines: f32) -> (i32, f32) {
    let v = lines * DISCRETE_LINES;
    let carried = if pending != 0.0 && pending.signum() != v.signum() { 0.0 } else { pending };
    let total = carried + v;
    let rows = total.trunc();
    (rows as i32, total - rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: f32 = 17.0;

    fn r(p: f32, d: f32) -> (i32, f32) {
        precise_pixels_to_rows(p, d, H, 1.0)
    }
    fn near(a: (i32, f32), b: (i32, f32)) -> bool {
        a.0 == b.0 && (a.1 - b.1).abs() < 1e-4
    }

    // 向量照搬 scripts/test-terminal-wheel.ts（行高 17，放大 2 倍，speed 1）
    #[test]
    fn precise_reference_vectors() {
        assert!(near(r(0.0, 5.0), (0, 10.0)), "不满一行：不滚，全留作余数");
        assert!(near(r(10.0, 5.0), (1, 3.0)), "余数累积到满一行才滚");
        assert!(near(r(0.0, 40.0), (4, 12.0)), "一次多行：截断取整，余数保留");
        assert!(near(r(0.0, -40.0), (-4, -12.0)), "反方向对称");
        assert!(near(r(12.0, -5.0), (0, -10.0)), "方向反转：丢掉旧余数");
        assert_eq!(r(0.0, 840.0).0, 98, "840px 快划 = 98 行");
        assert!(near(precise_pixels_to_rows(0.0, 34.0, H, 0.5), (2, 0.0)), "speed 0.5 抵消 boost = 1:1");
        assert!(near(precise_pixels_to_rows(0.0, 40.0, 0.0, 1.0), (0, 0.0)), "行高未知（0）：不滚");
    }

    #[test]
    fn discrete_three_lines_per_notch() {
        assert!(near(discrete_lines_to_rows(0.0, 1.0), (3, 0.0)), "一格 3 行");
        assert!(near(discrete_lines_to_rows(0.0, -2.0), (-6, 0.0)), "反向两格");
        let (n, p) = discrete_lines_to_rows(0.0, 0.1);
        assert_eq!(n, 0, "系统给的零点几格先攒着");
        let (n, _) = discrete_lines_to_rows(p, 0.3);
        assert_eq!(n, 1, "攒满一行再滚");
        assert!(near(discrete_lines_to_rows(0.9, -1.0), (-3, 0.0)), "反向丢掉旧余数");
    }
}
