//! 终端调色板（#221）：ANSI 16 色 + xterm 256 色，默认主题同 WebView 版的「VS Code Dark」。
//!
//! 为什么单独测：256 色的 6×6×6 立方和灰阶是算出来的，公式错了界面上是颜色整体偏掉
//! （claude 的代码高亮、diff 红绿全错），肉眼只会觉得「有点怪」，说不出哪里错。

pub const BG: u32 = 0x1e1e1e;
pub const FG: u32 = 0xd4d4d4;
pub const CURSOR: u32 = 0xaeafad;
pub const SELECTION: u32 = 0x264f78;

/// ANSI 16 色：black red green yellow blue magenta cyan white，再 bright 同序
pub const ANSI: [u32; 16] = [
    0x1e1e1e, 0xf44747, 0x89d185, 0xd7ba7d, 0x569cd6, 0xc586c0, 0x4ec9b0, 0xd4d4d4, //
    0x808080, 0xf14c4c, 0x73c991, 0xe2c08d, 0x6cb6ff, 0xd2a8ff, 0x58d1c9, 0xe5e5e5,
];

/// xterm 256 色下标 → 0xRRGGBB
pub fn indexed(i: u8) -> u32 {
    let i = i as u32;
    if i < 16 {
        return ANSI[i as usize];
    }
    if i < 232 {
        let n = i - 16;
        let level = |v: u32| if v == 0 { 0 } else { 55 + v * 40 };
        let (r, g, b) = (level(n / 36), level((n / 6) % 6), level(n % 6));
        return (r << 16) | (g << 8) | b;
    }
    let v = 8 + (i - 232) * 10;
    (v << 16) | (v << 8) | v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_range_uses_theme() {
        assert_eq!(indexed(1), 0xf44747);
        assert_eq!(indexed(15), 0xe5e5e5);
    }

    #[test]
    fn color_cube_matches_xterm() {
        assert_eq!(indexed(16), 0x000000);
        assert_eq!(indexed(21), 0x0000ff);
        assert_eq!(indexed(196), 0xff0000);
        assert_eq!(indexed(231), 0xffffff);
        assert_eq!(indexed(67), 0x5f87af);
    }

    #[test]
    fn grayscale_ramp_matches_xterm() {
        assert_eq!(indexed(232), 0x080808);
        assert_eq!(indexed(255), 0xeeeeee);
    }
}
