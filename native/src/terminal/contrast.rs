//! 最低对比度（#150，对齐清单不变量 25）：按**每个格子的实际背景**把前景色调到至少 4.5:1，dim 文字只要求一半。
//!
//! 照抄 xterm 6.1 `src/common/Color.ts` 的 `ensureContrastRatio`（DOM / WebGL 渲染器共用）：
//! 前景比背景暗就每步降 10% 亮度，否则每步升 10%；撞到纯黑 / 纯白还不够就反方向再试，取对比度高的那个。
//! 为什么不在主题里静态兜底：上游浅色配色里 ANSI 槽位不可见是常态（gruvbox-light 的 black 就等于 bg，1.00:1），
//! 而 SGR dim、256 色立方体、程序自己写的真彩色都不过主题色板 —— 只能按格子算。
//!
//! 为什么单独测：算法有「撞墙反向、取更好的那个」这种分支，错了在界面上是浅色主题里某些字「有点淡」，
//! 说不出哪里不对。期望值由 xterm 原算法（逐字抄成 JS 跑出来）给出。

/// 正文 WCAG AA。要整体调只改这一个数，不要去改 25 套配色
pub const MIN_CONTRAST: f64 = 4.5;
/// dim（SGR 2）文字：xterm 的 DIM_OPACITY
pub const DIM_OPACITY: f64 = 0.5;

pub fn relative_luminance(rgb: u32) -> f64 {
    luminance3((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff)
}

fn luminance3(r: u32, g: u32, b: u32) -> f64 {
    let f = |c: u32| {
        let s = c as f64 / 255.0;
        if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
    };
    f(r) * 0.2126 + f(g) * 0.7152 + f(b) * 0.0722
}

fn split(c: u32) -> (u32, u32, u32) {
    ((c >> 16) & 0xff, (c >> 8) & 0xff, c & 0xff)
}

/// 每步降 10% 亮度直到够了或到纯黑（xterm `reduceLuminance`）
fn reduce_luminance(bg: u32, fg: u32, ratio: f64) -> u32 {
    let bl = relative_luminance(bg);
    let (mut r, mut g, mut b) = split(fg);
    let ceil10 = |v: u32| ((v as f64) * 0.1).ceil() as u32;
    while contrast_ratio(luminance3(r, g, b), bl) < ratio && (r > 0 || g > 0 || b > 0) {
        r -= ceil10(r);
        g -= ceil10(g);
        b -= ceil10(b);
    }
    (r << 16) | (g << 8) | b
}

/// 每步升 10% 亮度直到够了或到纯白（xterm `increaseLuminance`）
fn increase_luminance(bg: u32, fg: u32, ratio: f64) -> u32 {
    let bl = relative_luminance(bg);
    let (mut r, mut g, mut b) = split(fg);
    let up = |v: u32| (v + (((255 - v) as f64) * 0.1).ceil() as u32).min(255);
    while contrast_ratio(luminance3(r, g, b), bl) < ratio && (r < 255 || g < 255 || b < 255) {
        r = up(r);
        g = up(g);
        b = up(b);
    }
    (r << 16) | (g << 8) | b
}

pub fn contrast_ratio(l1: f64, l2: f64) -> f64 {
    if l1 < l2 { (l2 + 0.05) / (l1 + 0.05) } else { (l1 + 0.05) / (l2 + 0.05) }
}

/// 已经够了返回 None，否则返回调整后的前景色（0xRRGGBB）
pub fn ensure_contrast_ratio(bg: u32, fg: u32, ratio: f64) -> Option<u32> {
    let bl = relative_luminance(bg);
    let fl = relative_luminance(fg);
    if contrast_ratio(bl, fl) >= ratio {
        return None;
    }
    let (first, second): (fn(u32, u32, f64) -> u32, fn(u32, u32, f64) -> u32) =
        if fl < bl { (reduce_luminance, increase_luminance) } else { (increase_luminance, reduce_luminance) };
    let a = first(bg, fg, ratio);
    let ar = contrast_ratio(bl, relative_luminance(a));
    if ar >= ratio {
        return Some(a);
    }
    let b = second(bg, fg, ratio);
    let br = contrast_ratio(bl, relative_luminance(b));
    Some(if ar > br { a } else { b })
}

/// 方框 / 块元素 / Powerline 字形当背景色用，不做对比度调整（xterm `treatGlyphAsBackgroundColor`）
pub fn glyph_is_background(c: char) -> bool {
    let u = c as u32;
    // Powerline（xterm isPowerlineGlyph）+ 方框 / 块元素（isBoxOrBlockGlyph）
    (0xe0a0..=0xe0d6).contains(&u) || (0x2500..=0x259f).contains(&u)
}

/// 一个格子最终画出来的前景色：先按背景保证对比度（dim 只要求一半），dim 再按 50% 透明混进背景
pub fn cell_fg(bg: u32, fg: u32, dim: bool, c: char) -> u32 {
    let adjusted = if glyph_is_background(c) {
        fg
    } else {
        ensure_contrast_ratio(bg, fg, MIN_CONTRAST / if dim { 2.0 } else { 1.0 }).unwrap_or(fg)
    };
    if !dim {
        return adjusted;
    }
    // dim 按 50% 透明混进背景（xterm rgba.blend：bg + round((fg - bg) × a)，JS 的 round）
    let (fr, fg_, fb) = split(adjusted);
    let (br, bgg, bb) = split(bg);
    let mix = |f: u32, b: u32| (b as f64 + ((f as f64 - b as f64) * DIM_OPACITY + 0.5).floor()) as u32;
    (mix(fr, br) << 16) | (mix(fg_, bgg) << 8) | mix(fb, bb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xterm_vectors() {
        // 期望值来自 xterm Color.ts 原算法
        assert_eq!(ensure_contrast_ratio(0xfbf1c7, 0xfbf1c7, 4.5), Some(0x696553), "gruvbox-light 的 black = bg（1.00:1）");
        assert_eq!(ensure_contrast_ratio(0x1e1e1e, 0x444444, 4.5), Some(0x868686), "暗底上的暗灰提亮");
        assert_eq!(ensure_contrast_ratio(0xffffff, 0xcccccc, 4.5), Some(0x767676), "白底上的浅灰压暗");
        assert_eq!(ensure_contrast_ratio(0xffffff, 0xcccccc, 2.25), Some(0xa4a4a4), "dim 只要求一半");
        assert_eq!(ensure_contrast_ratio(0x1e1e1e, 0xd4d4d4, 4.5), None, "已经够了就不动");
        assert_eq!(ensure_contrast_ratio(0x808080, 0x7f7f7f, 4.5), Some(0x161616), "中灰底：升亮撞墙后反向压暗，取更好的");
        assert_eq!(ensure_contrast_ratio(0xd5c4a1, 0x928374, 4.5), Some(0x544b42));
    }

    #[test]
    fn luminance_and_ratio() {
        assert!((contrast_ratio(relative_luminance(0xffffff), relative_luminance(0x000000)) - 21.0).abs() < 1e-9);
        assert!((contrast_ratio(relative_luminance(0x777777), relative_luminance(0x777777)) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cell_color() {
        // 不 dim：同 ensure
        assert_eq!(cell_fg(0xffffff, 0xcccccc, false, 'a'), 0x767676);
        // dim：先按 2.25 调成 a4a4a4，再 50% 混进白底 → (a4+ff)/2 四舍五入
        assert_eq!(cell_fg(0xffffff, 0xcccccc, true, 'a'), 0xd2d2d2);
        // 方框字形不调（它们是当背景块画的）
        assert_eq!(cell_fg(0xfbf1c7, 0xfbf1c7, false, '█'), 0xfbf1c7);
        assert!(glyph_is_background('─') && glyph_is_background('\u{e0b0}') && !glyph_is_background('a'));
    }
}
