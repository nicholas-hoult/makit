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

/// 终端用的一套颜色：从 `Theme` 取（`cx.theme().ansi[..]` 等，同 TS 版 TerminalManager.readTheme），
/// 每帧 render 时刷新一份，主题一换下一帧就生效。前景 / 背景 / ANSI 16 色用 0xRRGGBB（对比度要算亮度）
#[derive(Clone, Debug, PartialEq)]
pub struct TermColors {
    pub bg: u32,
    pub fg: u32,
    /// 光标颜色 = fg（同 TS 版 readTheme 的 `cursor: --fg`）
    pub cursor: u32,
    pub ansi: [u32; 16],
    /// 选区（半透明，叠在文字下面）：聚焦 / 未聚焦
    pub selection: gpui::Hsla,
    pub selection_inactive: gpui::Hsla,
    /// ⌘F 命中：全部命中黄、当前命中橙，两级都描边（必须不透明）
    pub match_bg: u32,
    pub match_border: u32,
    pub active_match_bg: u32,
    pub active_match_border: u32,
}

pub fn hsla_to_u32(c: gpui::Hsla) -> u32 {
    let r = gpui::Rgba::from(c);
    let ch = |v: f32| ((v.clamp(0.0, 1.0) * 255.0).round() as u32) & 0xff;
    (ch(r.r) << 16) | (ch(r.g) << 8) | ch(r.b)
}

impl TermColors {
    pub fn from_theme(t: &crate::theme::Theme) -> Self {
        Self {
            bg: hsla_to_u32(t.bg),
            fg: hsla_to_u32(t.fg),
            cursor: hsla_to_u32(t.fg),
            ansi: std::array::from_fn(|i| hsla_to_u32(t.ansi[i])),
            selection: t.selection_bg,
            selection_inactive: t.selection_bg_inactive,
            match_bg: hsla_to_u32(t.search_match_bg),
            match_border: hsla_to_u32(t.var("--search-match-border")),
            active_match_bg: hsla_to_u32(t.search_match_active_bg),
            active_match_border: hsla_to_u32(t.var("--search-match-active-border")),
        }
    }

    /// 256 色下标：前 16 个走主题，其余是固定的 6×6×6 立方和灰阶
    pub fn indexed(&self, i: u8) -> u32 {
        if i < 16 { self.ansi[i as usize] } else { indexed(i) }
    }
}

impl Default for TermColors {
    fn default() -> Self {
        Self {
            bg: BG,
            fg: FG,
            cursor: FG,
            ansi: ANSI,
            selection: gpui::hsla(0.0, 0.0, 1.0, 0.22),
            selection_inactive: gpui::hsla(0.0, 0.0, 1.0, 0.12),
            match_bg: 0x5d5242,
            match_border: 0xe2c08d,
            active_match_bg: 0x885145,
            active_match_border: 0xe08a5d,
        }
    }
}

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
    fn theme_colors_follow_the_theme() {
        let t = crate::theme::Theme::by_id("没有这个", &[]);
        let c = TermColors::from_theme(&t);
        assert_eq!(c.bg, hsla_to_u32(t.bg));
        assert_eq!(c.ansi[1], hsla_to_u32(t.ansi[1]), "ANSI 16 色取主题的");
        assert_eq!(c.indexed(1), c.ansi[1]);
        assert_eq!(c.indexed(196), 0xff0000, "16 以上是固定立方");
        assert_eq!(c.cursor, c.fg, "光标颜色 = fg");
        assert_eq!(hsla_to_u32(gpui::rgb(0x123456).into()), 0x123456);
    }

    #[test]
    fn grayscale_ramp_matches_xterm() {
        assert_eq!(indexed(232), 0x080808);
        assert_eq!(indexed(255), 0xeeeeee);
    }
}
