//! 终端网格的几何换算（#221）：像素 ↔ 行列、滚轮像素 → 行数。
//!
//! 为什么单独测：尺寸算错一行，界面上是 claude 全屏界面最底下那行被裁掉、或输入框
//! 跑到窗口外；点选换算错一格，选区和复制出来的文字就差一个字符；滚轮累计错了，
//! 触控板慢慢滑会完全不动（小于一行的增量被丢掉）。

/// 可用区域能放下多少列 / 行。向下取整；至少 2 列 1 行（PTY 不接受 0）
pub fn grid_size(width: f32, height: f32, cell_w: f32, line_h: f32) -> (u16, u16) {
    if cell_w <= 0.0 || line_h <= 0.0 {
        return (2, 1);
    }
    let cols = (width / cell_w).floor().max(2.0).min(u16::MAX as f32) as u16;
    let rows = (height / line_h).floor().max(1.0).min(u16::MAX as f32) as u16;
    (cols, rows)
}

/// 相对终端左上角的像素坐标 → (列, 行)，夹在网格范围内
pub fn point_to_cell(x: f32, y: f32, cell_w: f32, line_h: f32, cols: u16, rows: u16) -> (usize, usize) {
    let cell = |v: f32, size: f32, n: u16| {
        if size <= 0.0 || v <= 0.0 {
            return 0;
        }
        ((v / size).floor() as usize).min(n.saturating_sub(1) as usize)
    };
    (cell(x, cell_w, cols), cell(y, line_h, rows))
}

/// 滚轮：把像素增量累计起来，满一行才滚。返回 (要滚的行数, 新的余量)。
/// 正数 = 往上看历史（和 GPUI 的 delta.y 同号：手指往下滑是正）
pub fn wheel_lines(accum: f32, delta_px: f32, line_h: f32) -> (i32, f32) {
    if line_h <= 0.0 {
        return (0, 0.0);
    }
    let total = accum + delta_px;
    let lines = (total / line_h).trunc();
    (lines as i32, total - lines * line_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_size_floors_and_has_minimum() {
        assert_eq!(grid_size(805.0, 410.0, 8.0, 20.0), (100, 20));
        assert_eq!(grid_size(0.0, 0.0, 8.0, 20.0), (2, 1));
        assert_eq!(grid_size(800.0, 400.0, 0.0, 0.0), (2, 1), "字体还没量出来时不能除零");
    }

    #[test]
    fn point_to_cell_clamps() {
        assert_eq!(point_to_cell(17.0, 41.0, 8.0, 20.0, 100, 20), (2, 2));
        assert_eq!(point_to_cell(-5.0, -5.0, 8.0, 20.0, 100, 20), (0, 0));
        assert_eq!(point_to_cell(9999.0, 9999.0, 8.0, 20.0, 100, 20), (99, 19));
    }

    #[test]
    fn wheel_accumulates_sub_line_deltas() {
        let (n, acc) = wheel_lines(0.0, 7.0, 20.0);
        assert_eq!(n, 0);
        let (n, acc) = wheel_lines(acc, 7.0, 20.0);
        assert_eq!(n, 0);
        let (n, acc) = wheel_lines(acc, 7.0, 20.0);
        assert_eq!(n, 1);
        assert!((acc - 1.0).abs() < 1e-4);
        let (n, _) = wheel_lines(0.0, -45.0, 20.0);
        assert_eq!(n, -2);
    }
}
